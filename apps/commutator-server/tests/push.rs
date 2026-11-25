//! Phones register for session notifications; the account's machines post them.

use commutator_server::{App, Config, Kind};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_notifies_its_own_accounts_phones() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::open(
        Config {
            public_url: "http://unused".into(),
            google_client_id: "unused".into(),
            google_client_secret: "unused".into(),
            allowed_emails: vec!["me@example.com".into()],
            ice_servers: Vec::new(),
        },
        &dir.path().join("db"),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(axum::serve(listener, commutator_server::router(app.clone())).into_future());

    let me = app.upsert_user("sub-me", "me@example.com", None).unwrap();
    let (_, machine) = app.create_device(&me, "laptop", Kind::Machine, None).unwrap();
    let (phone_id, phone) = app.create_device(&me, "phone", Kind::Client, None).unwrap();
    let other = app.upsert_user("sub-other", "other@example.com", None).unwrap();
    let (_, theirs) = app.create_device(&other, "theirs", Kind::Client, None).unwrap();

    let http = reqwest::Client::new();
    let put = |token: &str, body: serde_json::Value| {
        http.put(format!("{url}/push/target")).bearer_auth(token).json(&body).send()
    };
    let prefs = json!({"done": true, "input": false, "failed": true});
    let ok = put(&phone, json!({"token": "ab12", "environment": "sandbox", "prefs": prefs})).await.unwrap();
    assert_eq!(ok.status(), 204);
    put(&theirs, json!({"token": "cd34", "environment": "production", "prefs": prefs})).await.unwrap();
    let bad = put(&phone, json!({"token": "../x", "environment": "sandbox", "prefs": prefs})).await.unwrap();
    assert_eq!(bad.status(), 400, "only hex goes into the APNs path");

    assert_eq!(
        app.push_targets_for(&me, "done").unwrap(),
        vec![(phone_id.clone(), "apns".into(), "ab12".into(), "sandbox".into())],
        "only my phone, not the other account's"
    );
    assert!(app.push_targets_for(&me, "input").unwrap().is_empty(), "the phone turned input off");

    // An Android phone: an FCM token, colons and all.
    let (android_id, android) = app.create_device(&me, "pixel", Kind::Client, None).unwrap();
    let fcm = json!({"platform": "fcm", "token": "dA1-x_Y:APA91b", "environment": "production", "prefs": prefs});
    assert_eq!(put(&android, fcm).await.unwrap().status(), 204);
    let bogus = json!({"platform": "fcm", "token": "a/b", "environment": "production", "prefs": prefs});
    assert_eq!(put(&android, bogus).await.unwrap().status(), 400);
    assert!(app
        .push_targets_for(&me, "done")
        .unwrap()
        .contains(&(android_id, "fcm".into(), "dA1-x_Y:APA91b".into(), "production".into())));
    let gone = http.delete(format!("{url}/push/target")).bearer_auth(&android).send().await.unwrap();
    assert_eq!(gone.status(), 204);

    let push = |token: &str| {
        http.post(format!("{url}/push"))
            .bearer_auth(token)
            .json(&json!({"chatId": "c1", "kind": "done", "title": "Fix it", "body": "Run finished"}))
            .send()
    };
    assert_eq!(push(&machine).await.unwrap().status(), 202);
    assert_eq!(push(&phone).await.unwrap().status(), 403, "only machines post");

    let gone = http.delete(format!("{url}/push/target")).bearer_auth(&phone).send().await.unwrap();
    assert_eq!(gone.status(), 204);
    assert!(app.push_targets_for(&me, "done").unwrap().is_empty());
}
