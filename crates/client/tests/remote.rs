//! Remote mode end to end: the Commutator server in-process, a real engine
//! signed in to it (`account.json`) hosting its device room there, and the
//! client as the account's phone — list the machines, pick the engine, create
//! a chat on it, send, and see the engine's copy of the message come back.

use std::sync::Arc;
use std::time::{Duration, Instant};

use commutator_client::events::NullListener;
use commutator_client::{
    Client, ClientConfig, Credentials, MessageRole, NewSession, SendOutcome, SendRequest,
    SessionTarget,
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

#[test]
fn a_chat_round_trips_through_a_remote_engine() {
    let server_dir = tempfile::tempdir().unwrap();
    let engine_dir = tempfile::tempdir().unwrap();
    let client_dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    // Server and engine on their own runtime, as other machines would be.
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

    // The machine signs in under its own engine device id (what `commutator
    // login` does), the phone as a client.
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
    }
    .save(engine_dir.path())
    .unwrap();

    let engine = rt.spawn(
        Engine::new(EngineConfig {
            data_dir: engine_dir.path().to_path_buf(),
            edge_url: "http://127.0.0.1:1".into(),
            ipc_port: port,
            default_harness: HarnessId::Mock,
        })
        .run(),
    );

    // The phone lists the account's machines and waits for the laptop's
    // engine to hold its room.
    let client_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    wait_for("the machine online on the server", Duration::from_secs(30), || {
        let devices = client_rt
            .block_on(commutator_client::server::devices(&server_url, &phone_token))
            .unwrap();
        assert!(devices.iter().any(|d| d.id == phone && d.current));
        devices
            .iter()
            .any(|d| d.id == machine && d.is_machine() && d.online)
    });

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

    // The engine's registry, over the relay: its own device, online.
    wait_for("the engine's device", Duration::from_secs(30), || {
        client
            .workspace()
            .devices
            .iter()
            .any(|d| d.id == machine && d.is_execution_host && d.online)
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
            title: Some("Remote".into()),
        })
        .unwrap();
    let session = client.open_session(&chat_id).unwrap();
    let SendOutcome::Started { message_id } =
        session.send(SendRequest::text("hello, remote engine")).unwrap()
    else {
        panic!("an idle chat starts a turn");
    };

    wait_for("the engine's user message", Duration::from_secs(30), || {
        session.snapshot().transcript().iter().any(|e| {
            e.message.id == message_id
                && e.message.role == MessageRole::User
                && format!("{:?}", e.message.parts).contains("hello, remote engine")
        })
    });
    wait_for("the engine's chat row", Duration::from_secs(10), || {
        client
            .workspace()
            .session(&chat_id)
            .is_some_and(|row| row.title == "Remote")
    });

    client.shutdown();
    engine.abort();
    drop(rt);
}
