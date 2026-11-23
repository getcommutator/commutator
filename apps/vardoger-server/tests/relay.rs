//! The engine's own relay ends (`HostRelay`, `LinkCache`) through this server.

use std::sync::Arc;
use std::time::Duration;

use vardoger_rpc::{
    HostRelay, HostRelayConfig, LinkCache, LinkCacheConfig, RpcError, RpcReply, RpcService,
    StaticToken,
};
use vardoger_server::{App, Config, Kind};

/// Answers every call with the method and the peer the relay stamped.
struct Echo;

#[async_trait::async_trait]
impl RpcService for Echo {
    async fn handle(&self, method: &str, _params: serde_json::Value) -> Result<RpcReply, RpcError> {
        Ok(RpcReply::Value(serde_json::json!({
            "method": method,
            "caller": vardoger_rpc::caller().record(),
        })))
    }
}

async fn serve(db: &std::path::Path) -> (Arc<App>, String) {
    let app = App::open(
        Config {
            public_url: "http://unused".into(),
            google_client_id: "unused".into(),
            google_client_secret: "unused".into(),
            allowed_emails: vec!["me@example.com".into()],
            ice_servers: Vec::new(),
        },
        db,
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(axum::serve(listener, vardoger_server::router(app.clone())).into_future());
    (app, url)
}

async fn devices(url: &str, token: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{url}/devices"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_calls_its_machine_through_the_relay() {
    let dir = tempfile::tempdir().unwrap();
    let (app, url) = serve(&dir.path().join("db")).await;
    let me = app.upsert_user("sub-me", "me@example.com", None).unwrap();
    let (machine, machine_token) = app
        .create_device(&me, "laptop", Kind::Machine, None)
        .unwrap();
    let (phone, phone_token) = app.create_device(&me, "phone", Kind::Client, None).unwrap();
    let stranger = app
        .upsert_user("sub-other", "other@example.com", None)
        .unwrap();
    let (_, stranger_token) = app
        .create_device(&stranger, "theirs", Kind::Client, None)
        .unwrap();

    // Not up yet: the phone sees the machine, offline.
    let listed = devices(&url, &phone_token).await;
    let laptop = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == machine.as_str())
        .unwrap();
    assert_eq!(laptop["online"], false);

    let _host = HostRelay::spawn(
        HostRelayConfig::new(
            url.clone(),
            machine.clone(),
            Arc::new(StaticToken(machine_token)),
        ),
        Arc::new(Echo),
        Arc::new(|_| false),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let listed = devices(&url, &phone_token).await;
            if listed
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["id"] == machine.as_str() && d["online"] == true)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the machine came online");

    let links = LinkCache::new(LinkCacheConfig::new(
        url.clone(),
        Arc::new(StaticToken(phone_token)),
    ));
    let answer = links
        .client(&machine)
        .await
        .unwrap()
        .call("EngineInfo", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(answer["method"], "EngineInfo");
    // Served as the phone, as the server vouched for it.
    assert_eq!(answer["caller"]["device"], phone.as_str());

    // Another account's device can't reach it.
    let theirs = LinkCache::new(LinkCacheConfig::new(
        url.clone(),
        Arc::new(StaticToken(stranger_token)),
    ));
    assert!(theirs.client(&machine).await.is_err());

    // Nor a revoked one (signed out from another of my devices).
    let (_, desktop_token) = app
        .create_device(&me, "desktop", Kind::Client, None)
        .unwrap();
    let revoke = reqwest::Client::new()
        .delete(format!("{url}/devices/{phone}"))
        .bearer_auth(desktop_token)
        .send()
        .await
        .unwrap();
    assert_eq!(revoke.status(), 204);
    links.close_all();
    assert!(links.client(&machine).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_may_host_only_itself() {
    let dir = tempfile::tempdir().unwrap();
    let (app, url) = serve(&dir.path().join("db")).await;
    let me = app.upsert_user("sub-me", "me@example.com", None).unwrap();
    let (machine, _) = app
        .create_device(&me, "laptop", Kind::Machine, None)
        .unwrap();
    let (_, other_token) = app
        .create_device(&me, "desktop", Kind::Machine, None)
        .unwrap();
    let ws = url.replacen("http", "ws", 1);
    let refused = tokio_tungstenite::connect_async(format!(
        "{ws}/device/{machine}/ws?role=host&token={other_token}"
    ))
    .await;
    match refused {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), 403)
        }
        other => panic!("hosted someone else's room: {other:?}"),
    }
}
