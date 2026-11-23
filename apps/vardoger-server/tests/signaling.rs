//! Preview signaling (`GET /preview/ws`): the raw protocol between the
//! account's devices, and a real WebRTC pair set up through it.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{Error as WsError, Message, client::IntoClientRequest};
use vardoger_server::{App, Config, IceServer, Kind};

async fn serve(ice_servers: Vec<IceServer>) -> (tempfile::TempDir, Arc<App>, String) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::open(
        Config {
            public_url: "http://unused".into(),
            google_client_id: "unused".into(),
            google_client_secret: "unused".into(),
            allowed_emails: vec!["me@example.com".into()],
            ice_servers,
        },
        &dir.path().join("db"),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(axum::serve(listener, vardoger_server::router(app.clone())).into_future());
    (dir, app, url)
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn dial(url: &str, device: &str, token: &str) -> Result<Socket, WsError> {
    let ws = url.replacen("http", "ws", 1);
    let mut request = format!("{ws}/preview/ws?device={device}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    Ok(tokio_tungstenite::connect_async(request).await?.0)
}

/// The next JSON message (skipping `pong`s).
async fn next(socket: &mut Socket) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await.expect("socket ended").unwrap() {
                Message::Text(text) if text == "pong" => {}
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                _ => {}
            }
        }
    })
    .await
    .expect("no message")
}

async fn send(socket: &mut Socket, value: Value) {
    socket.send(Message::Text(value.to_string())).await.unwrap();
}

fn status(result: Result<Socket, WsError>) -> u16 {
    match result {
        Err(WsError::Http(response)) => response.status().as_u16(),
        Err(other) => panic!("{other}"),
        Ok(_) => 101,
    }
}

const SDP: &str = "v=0\r\na=fingerprint:sha-256 AB:CD\r\n";

#[tokio::test(flavor = "multi_thread")]
async fn an_accounts_devices_signal_each_other_and_no_one_else() {
    let stun = IceServer {
        urls: vec!["stun:stun.example:3478".into()],
        username: None,
        credential: None,
    };
    let (_dir, app, url) = serve(vec![stun]).await;
    let me = app.upsert_user("sub-me", "me@example.com", None).unwrap();
    let (laptop, laptop_token) = app
        .create_device(&me, "laptop", Kind::Machine, None)
        .unwrap();
    let (phone, phone_token) = app.create_device(&me, "phone", Kind::Client, None).unwrap();
    let other = app
        .upsert_user("sub-other", "other@example.com", None)
        .unwrap();
    let (theirs, their_token) = app
        .create_device(&other, "theirs", Kind::Machine, None)
        .unwrap();

    // Who isn't signed in, or claims another device, is turned away.
    assert_eq!(status(dial(&url, &laptop, "nonsense").await), 401);
    assert_eq!(status(dial(&url, &laptop, &phone_token).await), 403);

    let mut a = dial(&url, &laptop, &laptop_token).await.unwrap();
    assert_eq!(
        next(&mut a).await["type"],
        "ice",
        "the server's ICE servers first"
    );
    let mut b = dial(&url, &phone, &phone_token).await.unwrap();
    assert_eq!(next(&mut b).await["type"], "ice");
    // Each hears of the other.
    assert_eq!(
        next(&mut a).await,
        json!({"type": "catalog", "device": phone, "services": []})
    );
    assert_eq!(
        next(&mut b).await,
        json!({"type": "catalog", "device": laptop, "services": []})
    );
    let mut c = dial(&url, &theirs, &their_token).await.unwrap();
    assert_eq!(next(&mut c).await["type"], "ice");

    // A catalog goes to the account's other devices.
    let services = json!([{"id": "web", "deviceId": laptop}]);
    send(&mut a, json!({"type": "catalog", "services": services})).await;
    assert_eq!(
        next(&mut b).await,
        json!({"type": "catalog", "device": laptop, "services": services})
    );

    // Offer and answer cross, stamped with who sent them.
    let offer = json!({"kind": "offer", "session": "s1", "sdp": {"type": "offer", "sdp": SDP}});
    send(
        &mut a,
        json!({"type": "signal", "to": phone, "signal": offer}),
    )
    .await;
    assert_eq!(
        next(&mut b).await,
        json!({"type": "signal", "from": laptop, "signal": offer})
    );
    let answer = json!({"kind": "answer", "session": "s1", "sdp": {"type": "answer", "sdp": SDP}});
    send(
        &mut b,
        json!({"type": "signal", "to": laptop, "signal": answer}),
    )
    .await;
    assert_eq!(
        next(&mut a).await,
        json!({"type": "signal", "from": phone, "signal": answer})
    );

    // Another account's device is out of reach both ways.
    send(
        &mut a,
        json!({"type": "signal", "to": theirs, "signal": offer}),
    )
    .await;
    assert_eq!(
        next(&mut a).await,
        json!({"type": "gone", "device": theirs})
    );
    send(
        &mut c,
        json!({"type": "signal", "to": laptop, "signal": offer}),
    )
    .await;
    assert_eq!(
        next(&mut c).await,
        json!({"type": "gone", "device": laptop})
    );

    // Keepalive.
    a.send(Message::Text("ping".into())).await.unwrap();
    let pong = tokio::time::timeout(Duration::from_secs(5), a.next())
        .await
        .unwrap();
    assert_eq!(pong.unwrap().unwrap(), Message::Text("pong".into()));

    // A device that leaves is gone for the others.
    drop(b);
    assert_eq!(next(&mut a).await, json!({"type": "gone", "device": phone}));
}

/// A device's connector: serves one HTTP page for any service id.
struct Page;

#[async_trait::async_trait]
impl vardoger_preview::mux::Connector for Page {
    async fn connect(&self, service: &str) -> anyhow::Result<vardoger_preview::mux::BoxIo> {
        anyhow::ensure!(service == "web", "unknown service {service}");
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            let (mut read, mut write) = tokio::io::split(ours);
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(read.read_u8().await.unwrap());
            }
            let body = "hello from the laptop";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            write.write_all(response.as_bytes()).await.unwrap();
            write.shutdown().await.unwrap();
        });
        Ok(Box::new(theirs))
    }
}

struct Token(String);

#[async_trait::async_trait]
impl vardoger_preview::signaling::TokenSource for Token {
    async fn token(&self) -> anyhow::Result<String> {
        Ok(self.0.clone())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_preview_opens_over_webrtc_paired_through_the_server() {
    use tokio_util::sync::CancellationToken;
    use vardoger_preview::{catalog::Catalog, peer::Peers, signaling, viewer::ViewerPeers};

    let (dir, app, url) = serve(Vec::new()).await;
    let me = app.upsert_user("sub-me", "me@example.com", None).unwrap();
    let (laptop, laptop_token) = app
        .create_device(&me, "laptop", Kind::Machine, None)
        .unwrap();
    let (phone, phone_token) = app.create_device(&me, "phone", Kind::Client, None).unwrap();
    let config = |token: &str| signaling::Config {
        server_url: url.clone(),
        tokens: Arc::new(Token(token.to_owned())),
    };
    let stop = CancellationToken::new();
    // Host candidates on loopback only: a host firewall never filters it.
    let loopback = |peers: &mut Peers| {
        peers.set_ice_servers(Vec::new()).unwrap();
        peers.set_udp_addrs(vec!["127.0.0.1:0".into()]).unwrap();
    };

    // The laptop: serves its page to peers.
    let laptop_catalog = Catalog::open(
        dir.path().join("laptop.json"),
        laptop.clone(),
        "laptop".into(),
    )
    .unwrap();
    let (mut laptop_peers, laptop_out) =
        Peers::new(laptop.clone(), Arc::new(Page), stop.child_token());
    loopback(&mut laptop_peers);
    tokio::spawn(signaling::run(
        config(&laptop_token),
        laptop_catalog,
        laptop_peers,
        laptop_out,
        stop.child_token(),
    ));

    // The phone: a viewer, opening the laptop's preview on its own loopback.
    let phone_catalog =
        Catalog::open(dir.path().join("phone.json"), phone.clone(), "phone".into()).unwrap();
    let (mut phone_peers, phone_out) =
        Peers::new(phone.clone(), ViewerPeers::refuse(), stop.child_token());
    loopback(&mut phone_peers);
    let viewer = ViewerPeers::start_with(
        &tokio::runtime::Handle::current(),
        phone_catalog,
        phone_peers,
        phone_out,
        stop.child_token(),
        config(&phone_token),
    );

    let body = tokio::time::timeout(Duration::from_secs(60), async {
        // Signaling may not have both devices yet: retry until it pairs.
        loop {
            if let Ok(forward) = viewer.forward(&laptop, "web").await {
                let fetched = reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .unwrap()
                    .get(forward.url())
                    .send()
                    .await;
                if let Ok(response) = fetched {
                    if response.status() == 200 {
                        return response.text().await.unwrap();
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .expect("the preview never opened");
    assert_eq!(body, "hello from the laptop");
    assert!(viewer.ready(&laptop).await, "it rode a direct connection");
    stop.cancel();
}
