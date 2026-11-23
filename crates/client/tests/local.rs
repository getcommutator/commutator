//! Local mode end to end: a real engine in-process on a temp profile and a
//! free port, the client over its IPC — no edge. Create a chat, open it, send,
//! and see the engine's own copy of the message in the session snapshot.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vardoger_client::events::NullListener;
use vardoger_client::{
    Client, ClientConfig, Credentials, MessageRole, NewSession, SendOutcome, SendRequest,
    SessionTarget,
};
use vardoger_engine::{Engine, EngineConfig, HarnessId};

fn wait_for(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_chat_round_trips_through_a_local_engine() {
    let engine_dir = tempfile::tempdir().unwrap();
    let client_dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    // The engine on its own runtime, as a separate process would be.
    let engine_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let engine = engine_rt.spawn(
        Engine::new(EngineConfig {
            data_dir: engine_dir.path().to_path_buf(),
            // Nothing listens here: a signed-out, local-only profile.
            edge_url: "http://127.0.0.1:1".into(),
            ipc_port: port,
            default_harness: HarnessId::Mock,
        })
        .run(),
    );

    let mut config = ClientConfig::new("http://127.0.0.1:1", client_dir.path());
    config.device_id = "desktop-test".into();
    let client = Client::new(
        config,
        Credentials::Local {
            port,
            engine_data_dir: engine_dir.path().to_path_buf(),
        },
        Arc::new(NullListener),
    )
    .unwrap();

    // The engine's own device shows up, online, from its registry watch.
    let mut host = String::new();
    wait_for("the engine's device", Duration::from_secs(30), || {
        let workspace = client.workspace();
        match workspace.devices.iter().find(|d| d.is_execution_host && d.online) {
            Some(device) => {
                host = device.id.clone();
                true
            }
            None => false,
        }
    });

    let chat_id = client
        .create_session(NewSession {
            target: SessionTarget::Projectless {
                device_id: host.clone(),
            },
            config: Some(serde_json::from_value(serde_json::json!({
                "harness": "mock", "model": null, "sandbox": "workspace-write"
            }))
            .unwrap()),
            branch: None,
            cwd: None,
            title: Some("Local".into()),
        })
        .unwrap();
    let session = client.open_session(&chat_id).unwrap();
    let SendOutcome::Started { message_id } =
        session.send(SendRequest::text("hello, engine")).unwrap()
    else {
        panic!("an idle chat starts a turn");
    };

    // The user message comes back from the engine's doc (the transcript part
    // of the snapshot, not the local echo), under the id the client minted.
    wait_for("the engine's user message", Duration::from_secs(30), || {
        session.snapshot().transcript().iter().any(|e| {
            e.message.id == message_id
                && e.message.role == MessageRole::User
                && format!("{:?}", e.message.parts).contains("hello, engine")
        })
    });
    // The engine lists the chat it created for us (its row, not just ours).
    wait_for("the engine's chat row", Duration::from_secs(10), || {
        client
            .workspace()
            .session(&chat_id)
            .is_some_and(|row| row.title == "Local")
    });

    client.shutdown();
    engine.abort();
    drop(engine_rt);
}
