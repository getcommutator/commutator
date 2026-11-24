//! Remote mode's direct path: a real engine signed in to the in-process
//! server, and the client as the account's phone. Its terminal starts on
//! the relay or a direct WebRTC link, settles on the direct link, falls
//! back to the relay when the phone's network changes, and keeps every
//! keystroke and output through it.
//!
//! The engine gathers host candidates on every interface, so this needs one
//! besides loopback that the two processes' sockets can reach (any machine
//! with a network; not a sandbox with only `lo`).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use commutator_client::events::NullListener;
use commutator_client::{
    Client, ClientConfig, Credentials, NewSession, SessionTarget, TerminalPath, TerminalSink,
    TerminalUpdate,
};
use commutator_engine::account::Account;
use commutator_engine::{Engine, EngineConfig, HarnessId};
use commutator_server::{App, Config, Kind};

fn wait_for(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[derive(Default)]
struct Screen {
    text: Mutex<String>,
    paths: Mutex<Vec<TerminalPath>>,
}

impl TerminalSink for Screen {
    fn update(&self, update: TerminalUpdate) {
        match update {
            TerminalUpdate::Output(bytes) => self
                .text
                .lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&bytes)),
            TerminalUpdate::Path(path) => self.paths.lock().unwrap().push(path),
            _ => {}
        }
    }
}

impl Screen {
    fn shows(&self, needle: &str) -> bool {
        self.text.lock().unwrap().contains(needle)
    }
    fn path(&self) -> Option<TerminalPath> {
        self.paths.lock().unwrap().last().copied()
    }
}

#[test]
fn a_remote_terminal_goes_direct_and_falls_back_to_the_relay() {
    let server_dir = tempfile::tempdir().unwrap();
    let engine_dir = tempfile::tempdir().unwrap();
    let client_dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let app = App::open(
        Config {
            public_url: "http://unused".into(),
            google_client_id: "unused".into(),
            google_client_secret: "unused".into(),
            allowed_emails: vec!["me@example.com".into()],
            ice_servers: Vec::new(),
        },
        &server_dir.path().join("db"),
    )
    .unwrap();
    let server_url = rt.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(axum::serve(listener, commutator_server::router(app.clone())).into_future());
        url
    });
    let me = app.upsert_user("sub-me", "me@example.com", None).unwrap();
    let machine = commutator_engine::device_id(engine_dir.path()).unwrap();
    let (_, machine_token) = app
        .create_device(&me, "laptop", Kind::Machine, Some(&machine))
        .unwrap();
    let (phone, phone_token) = app.create_device(&me, "phone", Kind::Client, None).unwrap();
    Account {
        server_url: server_url.clone(),
        token: machine_token,
        device_id: machine.clone(),
        email: "me@example.com".into(),
        name: None,
        picture: None,
    }
    .save(engine_dir.path())
    .unwrap();
    let engine = rt.spawn(
        Engine::new(EngineConfig {
            data_dir: engine_dir.path().to_path_buf(),
            ipc_port: port,
            default_harness: HarnessId::Mock,
        })
        .run(),
    );

    let mut config = ClientConfig::new(client_dir.path());
    config.device_id = phone.clone();
    let client = Client::new(
        config,
        Credentials::Remote {
            server_url: server_url.clone(),
            token: phone_token,
            host_device_id: machine.clone(),
        },
        Arc::new(NullListener),
    )
    .unwrap();
    wait_for("the engine's device", Duration::from_secs(30), || {
        client
            .workspace()
            .devices
            .iter()
            .any(|d| d.id == machine && d.online)
    });
    let chat_id = client
        .create_session(NewSession {
            target: SessionTarget::Projectless {
                device_id: machine.clone(),
            },
            config: Some(
                serde_json::from_value(serde_json::json!({
                    "harness": "mock", "model": null, "sandbox": "workspace-write"
                }))
                .unwrap(),
            ),
            branch: None,
            cwd: None,
            title: Some("Terminal".into()),
        })
        .unwrap();
    wait_for("the chat on the engine", Duration::from_secs(30), || {
        client.workspace().session(&chat_id).is_some()
    });

    let client_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let info = {
        let start = Instant::now();
        loop {
            match client_rt.block_on(client.open_terminal(&chat_id, 80, 24)) {
                Ok(info) => break info,
                Err(error) => {
                    assert!(start.elapsed() < Duration::from_secs(30), "{error}");
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    };
    let screen = Arc::new(Screen::default());
    let link = client.attach_terminal(info, 0, screen.clone());
    link.write(b"echo fi''rst-$((1+1))\n".to_vec());
    wait_for("output", Duration::from_secs(30), || {
        screen.shows("first-2")
    });
    wait_for("the direct link", Duration::from_secs(60), || {
        screen.path() == Some(TerminalPath::Direct)
    });
    link.write(b"echo dir''ect-$((2+2))\n".to_vec());
    wait_for(
        "output over the direct link",
        Duration::from_secs(30),
        || screen.shows("direct-4"),
    );

    // The phone's network changed: the direct link is gone at once, and the
    // terminal carries on over the relay (until it pairs again).
    let before = screen.paths.lock().unwrap().len();
    client.network_path_changed();
    wait_for("the relay", Duration::from_secs(30), || {
        screen.paths.lock().unwrap()[before..].contains(&TerminalPath::Relay)
    });
    link.write(b"echo re''layed-$((3+3))\n".to_vec());
    wait_for("output after the fallback", Duration::from_secs(30), || {
        screen.shows("relayed-6")
    });
    assert_eq!(
        screen.text.lock().unwrap().matches("relayed-6").count(),
        1,
        "a keystroke runs once"
    );

    drop(link);
    client.shutdown();
    engine.abort();
    // The engine's shell is still running in a blocking reader: don't wait on it.
    rt.shutdown_timeout(Duration::from_secs(1));
}
