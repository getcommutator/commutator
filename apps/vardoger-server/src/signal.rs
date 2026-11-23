//! Preview signaling: what lets the account's devices open WebRTC
//! connections to each other (`vardoger_preview::signaling` is the other
//! end; the protocol is Zeron's edge `PreviewRoom`'s). One room per account;
//! only presence, preview catalogs and SDP cross it, never preview bytes.
//!
//! `GET /preview/ws?device={id}` (bearer, or `?token=`), `device` being the
//! device the token belongs to. Text frames, JSON:
//!
//! - device → server: `{"type":"catalog","services":[…]}` (its own preview
//!   services; each `deviceId` must be its own), `{"type":"signal","to",
//!   "signal":{"kind":"connect"|"offer"|"answer","session","sdp"?}}`;
//! - server → device: `{"type":"catalog","device","services"}` (another
//!   device joined or changed; `[]` on join), `{"type":"gone","device"}`
//!   (it left, or a signal's target isn't here), `{"type":"signal","from",
//!   "signal"}`, and, when the server has ICE servers configured,
//!   `{"type":"ice","servers":[{"urls","username"?,"credential"?}]}` first;
//! - `ping` is answered `pong`.
//!
//! Abuse limits, per socket: 1 MiB messages, 120 messages per 10 s, 45 s of
//! silence ends it (devices ping every 10 s); 16 devices per account. A
//! device's reconnect replaces its old socket.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::{App, lock};

const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_SDP: usize = 64 * 1024;
const RATE_WINDOW: Duration = Duration::from_secs(10);
const RATE_MESSAGES: u32 = 120;
const SILENCE: Duration = Duration::from_secs(45);
const DEVICE_LIMIT: usize = 16;

/// A STUN or TURN server devices gather candidates from.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

type Tx = mpsc::Sender<Message>;

struct Member {
    conn: u64,
    tx: Tx,
    services: Value,
}

/// account → device → its socket.
#[derive(Default)]
pub(crate) struct Signaling {
    accounts: std::sync::Mutex<HashMap<String, HashMap<String, Member>>>,
    conns: AtomicU64,
}

fn text(value: Value) -> Message {
    Message::Text(value.to_string())
}

fn close(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

impl Signaling {
    /// Join `user`'s room as `device`: its connection number, or `None` at
    /// the device limit. Announces it to the others and sends it their catalogs.
    fn join(&self, user: &str, device: &str, tx: &Tx) -> Option<u64> {
        let conn = self.conns.fetch_add(1, Ordering::Relaxed);
        let mut accounts = lock(&self.accounts);
        let room = accounts.entry(user.to_owned()).or_default();
        if !room.contains_key(device) && room.len() >= DEVICE_LIMIT {
            return None;
        }
        let joined = text(json!({"type": "catalog", "device": device, "services": []}));
        for (other, member) in room.iter() {
            if other == device {
                let _ = member.tx.try_send(close(1000, "device reconnected"));
                continue;
            }
            let _ = member.tx.try_send(joined.clone());
            let _ = tx.try_send(text(json!({
                "type": "catalog", "device": other, "services": member.services,
            })));
        }
        room.insert(
            device.to_owned(),
            Member {
                conn,
                tx: tx.clone(),
                services: json!([]),
            },
        );
        Some(conn)
    }

    /// Connection `conn` of `device` left; the others hear it's gone.
    fn leave(&self, user: &str, device: &str, conn: u64) {
        let mut accounts = lock(&self.accounts);
        let Some(room) = accounts.get_mut(user) else {
            return;
        };
        if room.get(device).is_none_or(|m| m.conn != conn) {
            return; // replaced by its reconnect
        }
        room.remove(device);
        let gone = text(json!({"type": "gone", "device": device}));
        for member in room.values() {
            let _ = member.tx.try_send(gone.clone());
        }
        if room.is_empty() {
            accounts.remove(user);
        }
    }

    fn set_catalog(&self, user: &str, device: &str, services: Value) {
        let mut accounts = lock(&self.accounts);
        let Some(room) = accounts.get_mut(user) else {
            return;
        };
        let message = text(json!({"type": "catalog", "device": device, "services": services}));
        for (other, member) in room.iter_mut() {
            if other == device {
                member.services = services.clone();
            } else {
                let _ = member.tx.try_send(message.clone());
            }
        }
    }

    /// Hand `signal` to `to` in `user`'s room; `false` if it isn't there.
    fn forward(&self, user: &str, from: &str, to: &str, signal: Value) -> bool {
        let target = lock(&self.accounts)
            .get(user)
            .and_then(|room| room.get(to))
            .map(|member| member.tx.clone());
        // ponytail: a full queue drops the signal; the peer re-pairs after its pacing.
        target.is_some_and(|tx| {
            tx.try_send(text(
                json!({"type": "signal", "from": from, "signal": signal}),
            ))
            .is_ok()
        })
    }

    /// A signed-out device: close its socket.
    pub(crate) fn close_device(&self, device: &str) {
        for room in lock(&self.accounts).values() {
            if let Some(member) = room.get(device) {
                let _ = member.tx.try_send(close(4401, "signed out"));
            }
        }
    }
}

fn valid_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A device's own catalog: at most 256 objects, each its own.
fn valid_catalog(services: &Value, device: &str) -> bool {
    services.as_array().is_some_and(|services| {
        services.len() <= 256
            && services
                .iter()
                .all(|s| s.is_object() && s["deviceId"].as_str() == Some(device))
    })
}

/// The signal as forwarded, rebuilt from the fields peers use; `None` if malformed.
fn clean_signal(signal: &Value) -> Option<Value> {
    let session = signal["session"].as_str().filter(|s| valid_id(s))?;
    let kind = signal["kind"].as_str()?;
    if kind == "connect" && signal.get("sdp").is_none_or(Value::is_null) {
        return Some(json!({"kind": "connect", "session": session}));
    }
    if kind != "offer" && kind != "answer" {
        return None;
    }
    let sdp = &signal["sdp"];
    let body = sdp["sdp"].as_str()?;
    let ok = sdp["type"].as_str() == Some(kind)
        && body.len() <= MAX_SDP
        && body.starts_with("v=0\r\n")
        && body.contains("a=fingerprint:sha-256 ");
    ok.then(|| json!({"kind": kind, "session": session, "sdp": {"type": kind, "sdp": body}}))
}

#[derive(Deserialize)]
pub(crate) struct SignalQuery {
    device: Option<String>,
    token: Option<String>,
}

pub(crate) async fn preview_ws(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<SignalQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let caller = match app.authenticate(&headers, q.token.as_deref()) {
        Ok(device) => device,
        Err(status) => return status.into_response(),
    };
    // A device signals only as itself: the id its peers dial must be the
    // one the server vouches for.
    if q.device.is_some_and(|d| d != caller.id) {
        return (StatusCode::FORBIDDEN, "device is not this token's").into_response();
    }
    ws.max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| member(app, caller.user, caller.id, socket))
}

async fn member(app: Arc<App>, user: String, device: String, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Message>(256);
    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let closing = matches!(message, Message::Close(_));
            if sink.send(message).await.is_err() || closing {
                break;
            }
        }
        let _ = sink.close().await;
    });
    if !app.config.ice_servers.is_empty() {
        let servers = &app.config.ice_servers;
        let _ = tx.try_send(text(json!({"type": "ice", "servers": servers})));
    }
    let Some(conn) = app.signaling.join(&user, &device, &tx) else {
        let _ = tx.try_send(close(1013, "device limit reached"));
        drop(tx);
        let _ = writer.await;
        return;
    };
    tracing::debug!(%device, "signaling joined");
    let (mut window, mut count) = (tokio::time::Instant::now(), 0u32);
    let ended = loop {
        let message = match tokio::time::timeout(SILENCE, stream.next()).await {
            Ok(Some(Ok(message))) => message,
            Ok(_) => break None,
            Err(_) => break Some(close(1001, "silent")),
        };
        if writer.is_finished() {
            break None; // replaced, or signed out
        }
        if window.elapsed() > RATE_WINDOW {
            (window, count) = (tokio::time::Instant::now(), 0);
        }
        count += 1;
        if count > RATE_MESSAGES {
            break Some(close(1008, "signaling rate exceeded"));
        }
        let text_message = match message {
            Message::Text(text) => text,
            Message::Close(_) => break None,
            Message::Binary(_) => break Some(close(1008, "signaling messages only")),
            _ => continue,
        };
        if text_message == "ping" {
            let _ = tx.try_send(Message::Text("pong".into()));
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&text_message) else {
            break Some(close(1008, "invalid signaling JSON"));
        };
        match value["type"].as_str() {
            Some("catalog") if valid_catalog(&value["services"], &device) => {
                app.signaling
                    .set_catalog(&user, &device, value["services"].clone());
            }
            Some("signal") => {
                let to = value["to"]
                    .as_str()
                    .filter(|to| valid_id(to) && *to != device);
                let (Some(to), Some(signal)) = (to, clean_signal(&value["signal"])) else {
                    break Some(close(1008, "invalid signal"));
                };
                if !app.signaling.forward(&user, &device, to, signal) {
                    let _ = tx.try_send(text(json!({"type": "gone", "device": to})));
                }
            }
            _ => break Some(close(1008, "unsupported signaling message")),
        }
    };
    app.signaling.leave(&user, &device, conn);
    if let Some(message) = ended {
        let _ = tx.try_send(message);
    }
    drop(tx);
    let _ = writer.await;
    tracing::debug!(%device, "signaling left");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_well_formed_signals_cross() {
        let sdp = "v=0\r\na=fingerprint:sha-256 AB:CD\r\n";
        assert_eq!(
            clean_signal(&json!({"kind": "connect", "session": "s1"})),
            Some(json!({"kind": "connect", "session": "s1"}))
        );
        assert!(
            clean_signal(&json!({"kind": "offer", "session": "s1", "sdp": {"type": "offer", "sdp": sdp}, "extra": 1}))
                .is_some()
        );
        // Kind and description disagree, no fingerprint, bad session.
        assert!(
            clean_signal(
                &json!({"kind": "offer", "session": "s1", "sdp": {"type": "answer", "sdp": sdp}})
            )
            .is_none()
        );
        assert!(clean_signal(&json!({"kind": "answer", "session": "s1", "sdp": {"type": "answer", "sdp": "v=0\r\n"}})).is_none());
        assert!(clean_signal(&json!({"kind": "connect", "session": "a/b"})).is_none());
        assert!(clean_signal(&json!({"kind": "bye", "session": "s1"})).is_none());
    }

    #[test]
    fn a_catalog_lists_only_its_own_devices_services() {
        assert!(valid_catalog(&json!([{"deviceId": "me"}]), "me"));
        assert!(valid_catalog(&json!([]), "me"));
        assert!(!valid_catalog(&json!([{"deviceId": "other"}]), "me"));
        assert!(!valid_catalog(&json!({"deviceId": "me"}), "me"));
    }
}
