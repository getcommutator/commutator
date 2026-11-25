//! The engine's own relay ends (`HostRelay`, `LinkCache`) through this server.

use std::sync::Arc;
use std::time::Duration;

use commutator_rpc::{
    HostRelay, HostRelayConfig, LinkCache, LinkCacheConfig, RpcError, RpcReply, RpcService,
    StaticToken,
};
use commutator_server::{App, Config, Kind};

/// Answers every call with the method and the peer the relay stamped.
struct Echo;

#[async_trait::async_trait]
impl RpcService for Echo {
    async fn handle(&self, method: &str, _params: serde_json::Value) -> Result<RpcReply, RpcError> {
        Ok(RpcReply::Value(serde_json::json!({
            "method": method,
            "caller": commutator_rpc::caller().record(),
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
    tokio::spawn(axum::serve(listener, commutator_server::router(app.clone())).into_future());
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

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_code_signs_a_new_machine_in_once() {
    let dir = tempfile::tempdir().unwrap();
    let (app, url) = serve(&dir.path().join("db")).await;
    let me = app.upsert_user("sub-me", "me@example.com", None).unwrap();
    let (_, phone_token) = app.create_device(&me, "phone", Kind::Client, None).unwrap();
    let http = reqwest::Client::new();

    let pair: serde_json::Value = http
        .post(format!("{url}/devices/pair"))
        .bearer_auth(&phone_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let code = pair["code"].as_str().unwrap();
    let redeem = || {
        http.post(format!("{url}/auth/pair"))
            .json(&serde_json::json!({"code": code, "name": "vps", "deviceId": "engine-1"}))
            .send()
    };
    let signed_in: serde_json::Value = redeem().await.unwrap().json().await.unwrap();
    assert_eq!(signed_in["deviceId"], "engine-1");
    let listed = devices(&url, signed_in["token"].as_str().unwrap()).await;
    let vps = listed.as_array().unwrap().iter().find(|d| d["id"] == "engine-1").unwrap();
    assert_eq!(vps["name"], "vps");
    assert_eq!(vps["kind"], "machine");
    // One use only.
    assert_eq!(redeem().await.unwrap().status(), 400);
    // Only a signed-in device gets codes.
    let anon = http.post(format!("{url}/devices/pair")).send().await.unwrap();
    assert_eq!(anon.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_installer_signs_in_to_this_server() {
    let dir = tempfile::tempdir().unwrap();
    let (_, url) = serve(&dir.path().join("db")).await;
    let script = reqwest::get(format!("{url}/install.sh")).await.unwrap().text().await.unwrap();
    assert!(script.starts_with("#!/bin/sh"));
    assert!(script.contains("SERVER=\"${COMMUTATOR_SERVER_URL:-http://unused}\""));
}
