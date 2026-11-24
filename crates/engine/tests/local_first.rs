//! A signed-out engine starts on its local profile and needs nothing else.

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::sync::atomic::{AtomicUsize, Ordering};

use commutator_engine::{Engine, EngineConfig, EngineInfo, HarnessId, WorkspaceScope};
use commutator_rpc::{connect_ipc, connect_ws, memory_client, methods};

fn config(data_dir: &std::path::Path) -> EngineConfig {
    EngineConfig {
        data_dir: data_dir.to_path_buf(),
        ipc_port: 0,
        default_harness: HarnessId::Mock,
    }
}

async fn rejecting_edge() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut request = [0u8; 4096];
                let n = stream.read(&mut request).await.unwrap_or(0);
                // A `HEAD /` is preview discovery probing this listener on
                // localhost (seen from the Commutator engine already running on a
                // developer's Mac), not a request to the edge.
                if !request[..n].starts_with(b"HEAD / ") {
                    seen.fetch_add(1, Ordering::SeqCst);
                }
                let body = r#"{"error":"revoked"}"#;
                let response = format!(
                    "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    (format!("http://127.0.0.1:{port}"), requests, task)
}

#[tokio::test]
async fn a_signed_out_boot_serves_local_data() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let auth = Engine::build_auth(&config);
    let scope = Engine::initial_workspace_scope(&auth);
    let profile = Engine::resolve_profile(&config, &auth, scope)
        .unwrap()
        .expect("local profile is ready without auth");

    assert_eq!(scope, WorkspaceScope::Local);
    let runtime = Engine::assemble_runtime(&config, auth, profile)
        .await
        .unwrap();
    let client = memory_client(runtime.core().rpc_service());
    let info: EngineInfo = client
        .call_as(methods::ENGINE_INFO, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(info.workspace_scope, WorkspaceScope::Local);
    assert!(
        client
            .call(methods::LIST_HARNESSES, serde_json::json!({}))
            .await
            .unwrap()
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(dir.path().join("profiles/local").is_dir());
    assert!(!dir.path().join("orgs/dev-org/dev-user").exists());
    assert!(runtime.core().links().is_none());
    runtime.shutdown().await;
}

#[tokio::test]
async fn local_runtime_checks_public_releases_without_starting_edge_links() {
    let dir = tempfile::tempdir().unwrap();
    // A stand-in for the GitHub API (debug builds only). No other test here
    // checks for releases.
    let (releases_url, release_requests, releases_task) = rejecting_edge().await;
    // SAFETY: no other test in this binary reads it.
    unsafe { std::env::set_var("COMMUTATOR_RELEASES_API", &releases_url) };
    let config = config(dir.path());
    let auth = Engine::build_auth(&config);
    let scope = Engine::initial_workspace_scope(&auth);
    let profile = Engine::resolve_profile(&config, &auth, scope)
        .unwrap()
        .expect("local profile is ready");

    let runtime = Engine::assemble_runtime(&config, auth, profile)
        .await
        .unwrap();

    assert_eq!(scope, WorkspaceScope::Local);
    assert!(runtime.core().links().is_none());
    let updater = runtime
        .core()
        .updater()
        .expect("local runtime starts the public release checker");
    assert_eq!(release_requests.load(Ordering::SeqCst), 0);

    updater.check_now();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while release_requests.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("release checker asked GitHub for the latest release");
    assert!(runtime.core().links().is_none());

    runtime.shutdown().await;
    let stopped_at = release_requests.load(Ordering::SeqCst);
    updater.check_now();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(release_requests.load(Ordering::SeqCst), stopped_at);
    releases_task.abort();
}

#[tokio::test]
async fn headless_stop_rpc_drains_the_daemon_and_releases_ipc() {
    let dir = tempfile::tempdir().unwrap();
    let port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    let mut engine_config = config(dir.path());
    engine_config.ipc_port = port;
    let daemon = tokio::spawn(Engine::new(engine_config).run());

    let client = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(client) = connect_ipc(port, Some(dir.path())).await {
                break client;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("headless IPC did not start");

    // The daemon published an owner-only bearer and refuses dials without it.
    let token_file = commutator_rpc::ipc_auth::token_path(dir.path());
    assert!(
        token_file.is_file(),
        "headless start must publish its IPC token"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&token_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "IPC token must be owner-only");
    }
    let url = format!("ws://127.0.0.1:{port}");
    assert!(
        connect_ws(&url, None).await.is_err(),
        "a dial without the token must be refused"
    );
    assert!(
        connect_ws(&url, Some("0000")).await.is_err(),
        "a dial with a wrong token must be refused"
    );

    assert_eq!(
        client
            .call(methods::STOP_ENGINE, serde_json::json!({}))
            .await
            .unwrap(),
        serde_json::json!({ "ok": true })
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), daemon)
        .await
        .expect("headless engine did not stop")
        .expect("headless task panicked")
        .expect("headless shutdown failed");

    tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("headless IPC port remained occupied after shutdown");
}
