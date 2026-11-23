//! The device room: one machine's engine holds a `host` socket, the
//! account's other devices dial `client` sockets, and frames cross between
//! them in the engine's codec (`vardoger_rpc::device_room`):
//!
//! - client → host: stamped `from = connId` and `fromDevice` = the device the
//!   client's token belongs to (so the host serves it as that peer);
//! - host → client: routed by `to`, which is stripped;
//! - control frames (`" relay"`, payload `{"error": code}`): `host_offline`
//!   to a client dialing an absent host, `host_closed` to every client when
//!   the host goes, `client_closed` (`from`) to the host when a client goes,
//!   and `client_gone` (`to`) bounced to the host for an unknown target;
//! - text `ping` is answered `pong` (both ends' keepalive).
//!
//! The newest host wins: a second host socket for the same machine closes
//! the first with 4409, which the engine's `HostRelay` retries after.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use vardoger_rpc::device_room::{
    CLIENT_CLOSED, CLIENT_GONE, HOST_CLOSED, HOST_OFFLINE, RELAY_KIND,
};
use vardoger_rpc::{DeviceFrameHeader, decode_device_frame, encode_device_frame};

use crate::{App, Kind, lock};

type Tx = mpsc::Sender<Message>;

#[derive(Default)]
pub(crate) struct Rooms {
    rooms: std::sync::Mutex<HashMap<String, Room>>,
    epochs: AtomicU64,
}

#[derive(Default)]
struct Room {
    host: Option<(u64, Tx)>,
    clients: HashMap<String, Tx>,
}

impl Rooms {
    pub(crate) fn host_online(&self, device: &str) -> bool {
        lock(&self.rooms)
            .get(device)
            .is_some_and(|room| room.host.is_some())
    }

    fn host(&self, device: &str) -> Option<Tx> {
        lock(&self.rooms)
            .get(device)?
            .host
            .as_ref()
            .map(|(_, tx)| tx.clone())
    }

    fn client(&self, device: &str, conn: &str) -> Option<Tx> {
        lock(&self.rooms).get(device)?.clients.get(conn).cloned()
    }

    /// Become the host; the one it replaces, if any, is returned to close.
    fn set_host(&self, device: &str, tx: Tx) -> (u64, Option<Tx>) {
        let epoch = self.epochs.fetch_add(1, Ordering::Relaxed);
        let mut rooms = lock(&self.rooms);
        let old = rooms
            .entry(device.to_owned())
            .or_default()
            .host
            .replace((epoch, tx));
        (epoch, old.map(|(_, tx)| tx))
    }

    /// The host at `epoch` left: its clients, if it was still the host.
    fn drop_host(&self, device: &str, epoch: u64) -> Vec<Tx> {
        let mut rooms = lock(&self.rooms);
        let Some(room) = rooms.get_mut(device) else {
            return Vec::new();
        };
        if room
            .host
            .as_ref()
            .is_none_or(|(current, _)| *current != epoch)
        {
            return Vec::new();
        }
        room.host = None;
        let clients = room.clients.drain().map(|(_, tx)| tx).collect();
        rooms.remove(device);
        clients
    }

    fn add_client(&self, device: &str, conn: &str, tx: Tx) -> Option<Tx> {
        let mut rooms = lock(&self.rooms);
        let room = rooms.get_mut(device)?;
        let (_, host) = room.host.as_ref()?;
        let host = host.clone();
        room.clients.insert(conn.to_owned(), tx);
        Some(host)
    }

    /// The client left; the host to tell, if both are still there.
    fn remove_client(&self, device: &str, conn: &str, tx: &Tx) -> Option<Tx> {
        let mut rooms = lock(&self.rooms);
        let room = rooms.get_mut(device)?;
        if room
            .clients
            .get(conn)
            .is_some_and(|current| current.same_channel(tx))
        {
            room.clients.remove(conn);
        }
        room.host.as_ref().map(|(_, tx)| tx.clone())
    }

    /// A signed-out machine: close its host socket (its clients follow).
    pub(crate) fn close_device(&self, device: &str) {
        if let Some(tx) = self.host(device) {
            let _ = tx.try_send(close(4401, "signed out"));
        }
    }
}

fn close(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

fn relay_frame(to: Option<&str>, from: Option<&str>, code: &str) -> Message {
    let header = DeviceFrameHeader {
        s: RELAY_KIND.into(),
        k: RELAY_KIND.into(),
        to: to.map(str::to_owned),
        from: from.map(str::to_owned),
        from_device: None,
    };
    let payload = serde_json::json!({ "error": code }).to_string();
    Message::Binary(encode_device_frame(&header, payload.as_bytes()).expect("plain header"))
}

#[derive(Deserialize)]
pub(crate) struct WsQuery {
    role: String,
    #[serde(rename = "connId")]
    conn_id: Option<String>,
    token: Option<String>,
}

pub(crate) async fn device_ws(
    State(app): State<Arc<App>>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    Query(q): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let caller = match app.authenticate(&headers, q.token.as_deref()) {
        Ok(device) => device,
        Err(status) => return status.into_response(),
    };
    match q.role.as_str() {
        "host" => {
            if caller.id != id || caller.kind != Kind::Machine {
                return StatusCode::FORBIDDEN.into_response();
            }
            ws.on_upgrade(move |socket| host(app, id, socket))
        }
        "client" => {
            if app.owner_of(&id).as_deref() != Some(caller.user.as_str()) {
                return StatusCode::NOT_FOUND.into_response();
            }
            let Some(conn) = q.conn_id.filter(|c| {
                !c.is_empty()
                    && c.len() <= 64
                    && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            }) else {
                return (StatusCode::BAD_REQUEST, "connId is required").into_response();
            };
            let caller_device = caller.id.clone();
            let mut response =
                ws.on_upgrade(move |socket| client(app, id, conn, caller_device, socket));
            // The engine's LinkCache reads which device the room verified.
            if let Ok(value) = HeaderValue::from_str(&caller.id) {
                response
                    .headers_mut()
                    .insert("x-vardoger-caller-device", value);
            }
            response
        }
        _ => (StatusCode::BAD_REQUEST, "role must be host or client").into_response(),
    }
}

/// Write what `rx` carries to the socket until a close or the senders go.
fn writer(
    mut sink: futures::stream::SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<Message>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let closing = matches!(message, Message::Close(_));
            if sink.send(message).await.is_err() || closing {
                break;
            }
        }
        let _ = sink.close().await;
    })
}

async fn host(app: Arc<App>, device: String, socket: WebSocket) {
    let (sink, mut stream) = socket.split();
    let (tx, rx) = mpsc::channel(256);
    let writer = writer(sink, rx);
    let (epoch, replaced) = app.rooms.set_host(&device, tx.clone());
    if let Some(old) = replaced {
        let _ = old
            .send(close(4409, "superseded by new host connection"))
            .await;
    }
    tracing::info!(%device, "host joined");
    while let Some(Ok(message)) = stream.next().await {
        match message {
            Message::Text(text) if text == "ping" => {
                let _ = tx.send(Message::Text("pong".into())).await;
            }
            Message::Binary(bytes) => {
                let Ok((mut header, payload)) = decode_device_frame(&bytes) else {
                    continue;
                };
                // Frames without a target (nudge acks) are for an edge that syncs chats; not us.
                let Some(to) = header.to.take() else { continue };
                // ponytail: awaiting each client in turn, so one slow client stalls its
                // host's others; per-client queues that drop the laggard if it matters.
                let delivered = match app.rooms.client(&device, &to) {
                    Some(client) => match encode_device_frame(&header, &payload) {
                        Ok(frame) => client.send(Message::Binary(frame)).await.is_ok(),
                        Err(_) => continue,
                    },
                    None => false,
                };
                if !delivered {
                    let _ = tx.send(relay_frame(Some(&to), None, CLIENT_GONE)).await;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
        if writer.is_finished() {
            break; // closed from our side (superseded, signed out)
        }
    }
    for client in app.rooms.drop_host(&device, epoch) {
        let _ = client.send(relay_frame(None, None, HOST_CLOSED)).await;
        let _ = client.send(close(1000, "host left")).await;
    }
    app.touch(&device);
    tracing::info!(%device, "host left");
    drop(tx);
    let _ = writer.await;
}

async fn client(app: Arc<App>, device: String, conn: String, caller: String, socket: WebSocket) {
    let (sink, mut stream) = socket.split();
    let (tx, rx) = mpsc::channel(256);
    let writer = writer(sink, rx);
    if app.rooms.add_client(&device, &conn, tx.clone()).is_none() {
        let _ = tx.send(relay_frame(None, None, HOST_OFFLINE)).await;
        let _ = tx.send(close(1000, "host offline")).await;
        drop(tx);
        let _ = writer.await;
        return;
    }
    while let Some(Ok(message)) = stream.next().await {
        match message {
            Message::Text(text) if text == "ping" => {
                let _ = tx.send(Message::Text("pong".into())).await;
            }
            Message::Binary(bytes) => {
                let Ok((mut header, payload)) = decode_device_frame(&bytes) else {
                    continue;
                };
                header.to = None;
                header.from = Some(conn.clone());
                header.from_device = Some(caller.clone());
                let Ok(frame) = encode_device_frame(&header, &payload) else {
                    continue;
                };
                let sent = match app.rooms.host(&device) {
                    Some(host) => host.send(Message::Binary(frame)).await.is_ok(),
                    None => false,
                };
                if !sent {
                    let _ = tx.send(relay_frame(None, None, HOST_OFFLINE)).await;
                    let _ = tx.send(close(1000, "host offline")).await;
                    break;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
        if writer.is_finished() {
            break; // the host left and closed us
        }
    }
    if let Some(host) = app.rooms.remove_client(&device, &conn, &tx) {
        let _ = host
            .send(relay_frame(None, Some(&conn), CLIENT_CLOSED))
            .await;
    }
    drop(tx);
    let _ = writer.await;
}
