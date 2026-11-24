//! Local mode end to end: a real engine in-process on a temp profile and a
//! free port, the client over its IPC — no edge. Create a chat, open it, send,
//! and see the engine's own copy of the message in the session snapshot;
//! the same for an image, and for its upload failing and being retried.

use std::sync::Arc;
use std::time::{Duration, Instant};

use commutator_client::events::NullListener;
use commutator_client::{
    Client, ClientConfig, Credentials, MessageRole, NewSession, OutgoingAttachment, SendOutcome,
    SendRequest, SendState, SessionHandle, SessionTarget,
};
use commutator_engine::{Engine, EngineConfig, HarnessId};

fn wait_for(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The engine on its own runtime, as a separate process would be. Dropping
/// the runtime stops it.
fn start_engine(dir: &std::path::Path, port: u16) -> tokio::runtime::Runtime {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    rt.spawn(
        Engine::new(EngineConfig {
            data_dir: dir.to_path_buf(),
            // Nothing listens here: a signed-out, local-only profile.
            ipc_port: port,
            default_harness: HarnessId::Mock,
        })
        .run(),
    );
    rt
}

fn connect(client_dir: &std::path::Path, engine_dir: &std::path::Path, port: u16) -> Client {
    let mut config = ClientConfig::new(client_dir);
    config.device_id = "desktop-test".into();
    Client::new(
        config,
        Credentials::Local {
            port,
            engine_data_dir: engine_dir.to_path_buf(),
        },
        Arc::new(NullListener),
    )
    .unwrap()
}

/// The engine's own device, once it shows up online.
fn engine_device(client: &Client) -> String {
    let mut host = String::new();
    wait_for("the engine's device", Duration::from_secs(30), || {
        let workspace = client.workspace();
        match workspace
            .devices
            .iter()
            .find(|d| d.is_execution_host && d.online)
        {
            Some(device) => {
                host = device.id.clone();
                true
            }
            None => false,
        }
    });
    host
}

fn mock_chat(client: &Client, host: &str, title: &str) -> String {
    client
        .create_session(NewSession {
            target: SessionTarget::Projectless {
                device_id: host.to_owned(),
            },
            config: Some(
                serde_json::from_value(serde_json::json!({
                    "harness": "mock", "model": null, "sandbox": "workspace-write"
                }))
                .unwrap(),
            ),
            branch: None,
            cwd: None,
            title: Some(title.into()),
        })
        .unwrap()
}

fn image(name: &str, data: Vec<u8>) -> SendRequest {
    SendRequest {
        text: "what is this".into(),
        attachments: vec![OutgoingAttachment {
            name: name.into(),
            mime_type: "image/png".into(),
            data,
        }],
        ..Default::default()
    }
}

/// The engine's own copy of the user message (not the local echo).
fn engine_message(session: &SessionHandle, message_id: &str) -> Option<String> {
    session.snapshot().transcript().iter().find_map(|e| {
        (e.message.id == message_id && e.message.role == MessageRole::User && e.echo.is_none())
            .then(|| format!("{:?}", e.message.parts))
    })
}

/// The engine ran the turn with the image as a file of its own: the prompt
/// names an absolute path (no `pending://` ref) holding the sent bytes.
fn assert_image_landed(session: &SessionHandle, message_id: &str, name: &str, bytes: &[u8]) {
    let mut text = String::new();
    wait_for(
        "the engine's user message",
        Duration::from_secs(30),
        || match engine_message(session, message_id) {
            Some(t) if !t.contains("pending://") && t.contains(name) => {
                text = t;
                true
            }
            _ => false,
        },
    );
    let path = text
        .split(['\n', '"', ' '])
        .chain(text.split("\\n"))
        .map(|w| w.trim_start_matches("- "))
        .find(|w| w.starts_with('/') && w.ends_with(name))
        .unwrap_or_else(|| panic!("no image path in {text}"));
    assert_eq!(std::fs::read(path).unwrap(), bytes, "{path}");
}

#[test]
fn a_chat_round_trips_through_a_local_engine() {
    let engine_dir = tempfile::tempdir().unwrap();
    let client_dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let engine_rt = start_engine(engine_dir.path(), port);
    let client = connect(client_dir.path(), engine_dir.path(), port);
    let host = engine_device(&client);

    let chat_id = mock_chat(&client, &host, "Local");
    let session = client.open_session(&chat_id).unwrap();
    let SendOutcome::Started { message_id } =
        session.send(SendRequest::text("hello, engine")).unwrap()
    else {
        panic!("an idle chat starts a turn");
    };

    // The user message comes back from the engine's doc (the transcript part
    // of the snapshot, not the local echo), under the id the client minted.
    wait_for("the engine's user message", Duration::from_secs(30), || {
        engine_message(&session, &message_id).is_some_and(|t| t.contains("hello, engine"))
    });
    // The engine lists the chat it created for us (its row, not just ours).
    wait_for("the engine's chat row", Duration::from_secs(10), || {
        client
            .workspace()
            .session(&chat_id)
            .is_some_and(|row| row.title == "Local")
    });

    client.shutdown();
    drop(engine_rt);
}

/// A send with an image echoes at once (with its `pending://` ref) and the
/// bytes upload behind it; the engine runs the turn on its own copy.
#[test]
fn an_image_reaches_the_agent_through_a_local_engine() {
    let engine_dir = tempfile::tempdir().unwrap();
    let client_dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let engine_rt = start_engine(engine_dir.path(), port);
    let client = connect(client_dir.path(), engine_dir.path(), port);
    let host = engine_device(&client);
    let session = client
        .open_session(&mock_chat(&client, &host, "Image"))
        .unwrap();

    let bytes = b"\x89PNG not really".to_vec();
    let SendOutcome::Started { message_id } =
        session.send(image("photo.png", bytes.clone())).unwrap()
    else {
        panic!("an idle chat starts a turn");
    };
    let echo = session.composer().pending_sends.clone();
    assert!(
        echo.iter()
            .any(|p| p.message_id == message_id
                && p.images.iter().any(|i| i.starts_with("pending://"))),
        "the echo shows the image right away: {echo:?}"
    );

    assert_image_landed(&session, &message_id, "photo.png", &bytes);
    client.shutdown();
    drop(engine_rt);
}

/// The upload can't reach the engine: the send fails with the reason instead
/// of waiting on its bytes forever, and a retry once the engine is back
/// uploads them again and delivers. Bytes the cache has since dropped can't
/// be retried, and say so.
#[test]
fn a_failed_image_upload_fails_the_send_and_retries() {
    let engine_dir = tempfile::tempdir().unwrap();
    let client_dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let engine_rt = start_engine(engine_dir.path(), port);
    let client = connect(client_dir.path(), engine_dir.path(), port);
    let host = engine_device(&client);
    let session = client
        .open_session(&mock_chat(&client, &host, "Retry"))
        .unwrap();
    wait_for("the chat on the engine", Duration::from_secs(10), || {
        client
            .workspace()
            .session(session.chat_id())
            .is_some_and(|r| r.title == "Retry")
    });
    engine_rt.shutdown_timeout(Duration::from_secs(5));

    let bytes = b"\x89PNG retry me".to_vec();
    let SendOutcome::Started { message_id } =
        session.send(image("retry.png", bytes.clone())).unwrap()
    else {
        panic!("an idle chat starts a turn");
    };
    wait_for("the send to fail", Duration::from_secs(30), || {
        let composer = session.composer();
        composer
            .pending_sends
            .iter()
            .any(|p| p.message_id == message_id && p.state == SendState::Failed)
            && composer
                .queue_error
                .as_deref()
                .is_some_and(|e| e.contains("Couldn't upload retry.png"))
    });

    let engine_rt = start_engine(engine_dir.path(), port);
    engine_device(&client);
    session.retry_delivery().unwrap();
    assert_eq!(session.composer().queue_error, None);
    assert_image_landed(&session, &message_id, "retry.png", &bytes);

    // Evicted bytes: the engine goes away again, the send fails, and then
    // bigger images push its bytes out of the attachment cache.
    // The agent's reply, not just "no turn": the turn may not have started yet.
    wait_for("the turn to end", Duration::from_secs(30), || {
        !session.composer().live.turn_running
            && session
                .snapshot()
                .transcript()
                .iter()
                .any(|e| e.message.role == MessageRole::Assistant)
    });
    engine_rt.shutdown_timeout(Duration::from_secs(5));
    let SendOutcome::Started { message_id } =
        session.send(image("gone.png", b"small".to_vec())).unwrap()
    else {
        panic!("the last turn ended");
    };
    wait_for("the second send to fail", Duration::from_secs(30), || {
        session
            .composer()
            .pending_sends
            .iter()
            .any(|p| p.message_id == message_id && p.state == SendState::Failed)
    });
    for i in 0..3 {
        let big = vec![0u8; 20 * 1024 * 1024];
        session.send(image(&format!("big{i}.png"), big)).unwrap();
    }
    let err = session.retry_delivery().unwrap_err().to_string();
    assert!(err.contains("no longer available"), "{err}");
    client.shutdown();
}
