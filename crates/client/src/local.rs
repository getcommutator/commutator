//! The client against one engine, as its JSON-RPC client: on this machine
//! over its IPC (`ws://127.0.0.1:{port}`, the bearer from
//! `{data_dir}/ipc-token`), or on another of the account's machines through
//! the Vardoger server's relay ([`EngineLink`]). Same logic either way.
//!
//! Shaped like the demo host: the registry is a local-only replica and every
//! open chat a local [`SessionDoc`], so the same session pipeline (snapshot,
//! composer, pending echoes) runs unchanged. The engine is the host:
//!
//! - registry: `WatchChats` / `WatchDevices` / `WatchSpaces` / `WatchSessions`
//!   (whole lists) folded into the replica; the engine's device beats presence
//!   while the socket is up;
//! - transcript: `WatchDocMessages` frames applied with
//!   [`vardoger_doc::transcript_delta::apply_transcript_frame`], then written
//!   into the chat's doc; the queue mirrors `WatchQueue`;
//! - writes: commands the session writes into its doc go out as `QueueCommand`,
//!   queue rows as `QueueMessage`, a new chat as `Mutate createChat` — through
//!   one ordered outbox, so a chat exists before its first message; every other
//!   host call (`ListHarnesses`, queue leases, …) is forwarded as is.
//!
//! This is how Zeron's desktop (zgx) read a local engine: JSON watches, not doc
//! sync — the engine serves no Loro updates over IPC.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use vardoger_doc::transcript_delta::{TranscriptUpdate, apply_transcript_frame};
use vardoger_doc::{QueuedMessage, SessionCommandStatus, SessionDoc, SessionMessageEntry};
use vardoger_proto::{Chat, Device, Session, Space};
use vardoger_rpc::{LinkCache, RpcClient, RpcError, methods};

use crate::client::ClientInner;
use crate::error::{ClientError, Result};
use crate::session::SessionCore;
use crate::{lock, now_ms};

const RETRY_BASE: Duration = Duration::from_millis(250);
const RETRY_CAP: Duration = Duration::from_secs(5);
/// Presence beat for the engine's device while its socket is up (fresh is 45 s).
const PRESENCE_BEAT: Duration = Duration::from_secs(10);

/// One write for the engine, in the order the client made it.
struct Out {
    method: &'static str,
    params: Value,
    /// A doc command to mark rejected when the engine refuses it.
    command: Option<(String, String)>,
}

/// How the client reaches its engine.
pub(crate) enum EngineLink {
    /// The engine on this machine: its IPC port and data dir (`ipc-token`).
    Ipc { port: u16, data_dir: PathBuf },
    /// An engine on another machine, through the server relay (`device_id`'s
    /// device room).
    Relay {
        links: Arc<LinkCache>,
        device_id: String,
    },
}

impl EngineLink {
    async fn dial(&self) -> std::result::Result<Arc<RpcClient>, RpcError> {
        match self {
            EngineLink::Ipc { port, data_dir } => vardoger_rpc::connect_ipc(*port, Some(data_dir))
                .await
                .map(Arc::new),
            EngineLink::Relay { links, device_id } => links.client(device_id).await,
        }
    }

    /// The connection `dial` gave closed: dial afresh next time.
    fn closed(&self) {
        if let EngineLink::Relay { links, device_id } = self {
            links.invalidate(device_id);
        }
    }
}

impl std::fmt::Display for EngineLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineLink::Ipc { port, .. } => write!(f, "local engine on 127.0.0.1:{port}"),
            EngineLink::Relay { device_id, .. } => write!(f, "engine {device_id} (relay)"),
        }
    }
}

pub(crate) struct LocalHost {
    client: Weak<ClientInner>,
    link: EngineLink,
    rpc: watch::Sender<Option<Arc<RpcClient>>>,
    device_id: Mutex<Option<String>>,
    outbox: mpsc::UnboundedSender<Out>,
    outbox_rx: Mutex<Option<mpsc::UnboundedReceiver<Out>>>,
    /// Doc commands / queue rows already handed to the outbox.
    forwarded: Mutex<HashSet<String>>,
    /// Per chat: the queue row ids the engine last reported.
    engine_queue: Mutex<HashMap<String, HashSet<String>>>,
    /// Chat ids the engine last listed (only those are pruned when gone).
    engine_chats: Mutex<HashSet<String>>,
    cancel: CancellationToken,
}

fn rpc_err(method: &str, err: vardoger_rpc::RpcError) -> ClientError {
    crate::live::relay::map_rpc("engine", method, err)
}

impl LocalHost {
    pub(crate) fn new(client: &Arc<ClientInner>, link: EngineLink) -> Arc<Self> {
        let (outbox, outbox_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            client: Arc::downgrade(client),
            link,
            rpc: watch::Sender::new(None),
            device_id: Mutex::new(None),
            outbox,
            outbox_rx: Mutex::new(Some(outbox_rx)),
            forwarded: Mutex::new(HashSet::new()),
            engine_queue: Mutex::new(HashMap::new()),
            engine_chats: Mutex::new(HashSet::new()),
            cancel: client.cancel.child_token(),
        })
    }

    pub(crate) fn start(self: &Arc<Self>) {
        let rt = crate::runtime::shared();
        rt.spawn(self.clone().connect_loop());
        if let Some(rx) = lock(&self.outbox_rx).take() {
            rt.spawn(self.clone().drain_outbox(rx));
        }
        for method in [
            methods::WATCH_DEVICES,
            methods::WATCH_SPACES,
            methods::WATCH_CHATS,
            methods::WATCH_SESSIONS,
        ] {
            let host = Arc::downgrade(self);
            self.spawn_watch(self.cancel.clone(), method, json!({}), move |item| {
                if let Some(host) = host.upgrade() {
                    host.registry_frame(method, item);
                }
                true
            });
        }
    }

    pub(crate) fn stop(&self) {
        self.cancel.cancel();
    }

    /// The engine's device (the host of every chat it runs), once known.
    pub(crate) fn device_id(&self) -> Option<String> {
        lock(&self.device_id).clone()
    }

    /// The current connection, waiting for one (bounded) if it's down.
    async fn rpc(&self) -> Result<Arc<RpcClient>> {
        let mut rx = self.rpc.subscribe();
        let wait = rx.wait_for(Option::is_some);
        match tokio::time::timeout(RETRY_CAP * 2, wait).await {
            Ok(Ok(rpc)) => Ok(rpc.clone().expect("checked")),
            _ => Err(ClientError::HostUnavailable(format!("no {}", self.link))),
        }
    }

    /// Any host call, straight to the engine.
    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.rpc()
            .await?
            .call(method, params)
            .await
            .map_err(|e| rpc_err(method, e))
    }

    pub(crate) async fn subscribe(
        &self,
        method: &str,
        params: Value,
    ) -> Result<crate::host_stream::HostStream> {
        self.rpc()
            .await?
            .subscribe_scoped(method, params)
            .await
            .map(crate::host_stream::HostStream::Live)
            .map_err(|e| rpc_err(method, e))
    }

    // ── connection ─────────────────────────────────────────────────────────

    async fn connect_loop(self: Arc<Self>) {
        let mut wait = RETRY_BASE;
        loop {
            let rpc = tokio::select! {
                _ = self.cancel.cancelled() => return,
                rpc = self.link.dial() => rpc,
            };
            let rpc = match rpc {
                Ok(rpc) => rpc,
                Err(err) => {
                    tracing::debug!(engine = %self.link, error = %err, "engine dial failed");
                    tokio::select! {
                        _ = self.cancel.cancelled() => return,
                        _ = tokio::time::sleep(wait) => {}
                    }
                    wait = (wait * 2).min(RETRY_CAP);
                    continue;
                }
            };
            wait = RETRY_BASE;
            if let Ok(reply) = rpc.call(methods::LOCAL_DEVICE, json!({})).await
                && let Some(id) = reply["deviceId"].as_str()
            {
                *lock(&self.device_id) = Some(id.to_owned());
            }
            self.rpc.send_replace(Some(rpc.clone()));
            let mut beat = tokio::time::interval(PRESENCE_BEAT);
            loop {
                tokio::select! {
                    _ = self.cancel.cancelled() => return,
                    _ = rpc.closed() => break,
                    _ = beat.tick() => self.beat(),
                }
            }
            tracing::debug!(engine = %self.link, "engine connection closed");
            self.link.closed();
            self.rpc.send_replace(None);
        }
    }

    fn beat(&self) {
        let (Some(client), Some(device)) = (self.client.upgrade(), self.device_id()) else {
            return;
        };
        client.workspace.set_presence(&device, now_ms());
        client.recompute_workspace();
    }

    /// Run `method` for as long as `cancel` lives, resubscribing on every
    /// reconnect. `on_item` returning `false` resubscribes now (a desync).
    fn spawn_watch(
        self: &Arc<Self>,
        cancel: CancellationToken,
        method: &'static str,
        params: Value,
        mut on_item: impl FnMut(Value) -> bool + Send + 'static,
    ) {
        let host = Arc::downgrade(self);
        crate::runtime::shared().spawn(async move {
            loop {
                let Some(this) = host.upgrade() else { return };
                let mut connected = this.rpc.subscribe();
                drop(this);
                let rpc = tokio::select! {
                    _ = cancel.cancelled() => return,
                    rpc = connected.wait_for(Option::is_some) => match rpc {
                        Ok(rpc) => rpc.clone().expect("checked"),
                        Err(_) => return,
                    },
                };
                let mut stream = match rpc.subscribe_scoped(method, params.clone()).await {
                    Ok(stream) => stream,
                    Err(err) => {
                        tracing::debug!(method, error = %err, "local watch failed");
                        tokio::select! {
                            _ = cancel.cancelled() => return,
                            _ = tokio::time::sleep(RETRY_BASE) => continue,
                        }
                    }
                };
                loop {
                    let item = tokio::select! {
                        _ = cancel.cancelled() => return,
                        item = stream.recv() => item,
                    };
                    match item {
                        Some(item) => {
                            if !on_item(item) {
                                break;
                            }
                        }
                        // Ended (connection gone): wait for the next one.
                        None => {
                            tokio::select! {
                                _ = cancel.cancelled() => return,
                                _ = tokio::time::sleep(RETRY_BASE) => break,
                            }
                        }
                    }
                }
            }
        });
    }

    // ── registry ───────────────────────────────────────────────────────────

    fn registry_frame(&self, method: &str, item: Value) {
        let Some(client) = self.client.upgrade() else {
            return;
        };
        let written = match method {
            methods::WATCH_CHATS => {
                let Ok(chats) = serde_json::from_value::<Vec<Chat>>(item) else {
                    return;
                };
                let listed: HashSet<String> = chats.iter().map(|c| c.id.clone()).collect();
                let gone: Vec<String> = {
                    let mut known = lock(&self.engine_chats);
                    let gone = known.difference(&listed).cloned().collect();
                    *known = listed;
                    gone
                };
                let written = client.registry_write(|doc| {
                    for chat in &chats {
                        doc.upsert_chat(chat)?;
                    }
                    for id in &gone {
                        doc.delete_chat(id)?;
                    }
                    Ok(())
                });
                client.set_synced();
                written
            }
            methods::WATCH_DEVICES => {
                let Ok(devices) = serde_json::from_value::<Vec<Device>>(item) else {
                    return;
                };
                client.registry_write(|doc| devices.iter().try_for_each(|d| doc.upsert_device(d)))
            }
            methods::WATCH_SPACES => {
                let Ok(spaces) = serde_json::from_value::<Vec<Space>>(item) else {
                    return;
                };
                client.registry_write(|doc| spaces.iter().try_for_each(|s| doc.upsert_space(s)))
            }
            methods::WATCH_SESSIONS => {
                let Ok(sessions) = serde_json::from_value::<Vec<Session>>(item) else {
                    return;
                };
                client.registry_write(|doc| sessions.iter().try_for_each(|s| doc.upsert_session(s)))
            }
            _ => return,
        };
        if let Err(err) = written {
            tracing::warn!(method, error = %err, "local registry frame not applied");
        }
        self.beat();
    }

    /// A chat the client just minted: the engine creates it too.
    pub(crate) fn chat_created(&self, chat: &Chat) {
        let mut params = json!({
            "op": "createChat",
            "chatId": chat.id,
            "deviceId": chat.device_id,
            "spaceId": chat.space_id,
            "config": chat.config,
            "branch": chat.branch,
            "cwd": chat.cwd,
        });
        if let Some(parent) = &chat.parent_chat_id {
            params["parentChatId"] = json!(parent);
        }
        self.mutate(params);
        if let Some(title) = &chat.title {
            self.mutate(json!({ "op": "renameChat", "chatId": chat.id, "title": title }));
        }
    }

    /// A workspace write (`Mutate {op, …}`) the replica already holds.
    pub(crate) fn mutate(&self, params: Value) {
        let _ = self.outbox.send(Out {
            method: methods::MUTATE,
            params,
            command: None,
        });
    }

    // ── sessions ───────────────────────────────────────────────────────────

    pub(crate) fn session_doc(&self, chat_id: &str) -> Result<SessionDoc> {
        SessionDoc::init(chat_id).map_err(Into::into)
    }

    /// Mirror the chat's transcript and queue into its doc while it's open.
    pub(crate) fn session_opened(self: &Arc<Self>, core: &Arc<SessionCore>) {
        let cancel = core.cancel_token();
        let chat = core.chat_id.clone();

        let weak = Arc::downgrade(core);
        let mut entries: Vec<SessionMessageEntry> = Vec::new();
        self.spawn_watch(
            cancel.clone(),
            methods::WATCH_DOC_MESSAGES,
            json!({ "chatId": chat }),
            move |item| {
                let Some(core) = weak.upgrade() else {
                    return true;
                };
                let Ok(update) = serde_json::from_value::<TranscriptUpdate>(item) else {
                    return false;
                };
                let before = std::mem::take(&mut entries);
                let mut next = before.clone();
                if apply_transcript_frame(&mut next, update.frame).is_err() {
                    // Resubscribe: the next frame is a reset.
                    entries = before;
                    return false;
                }
                let written = core.write(|doc| {
                    write_entries(doc, &before, &next)?;
                    match &update.context_usage {
                        Some(usage) => doc.update_context_usage(usage.tokens, usage.window),
                        None => Ok(()),
                    }
                });
                if let Err(err) = written {
                    tracing::warn!(chat = %core.chat_id, error = %err, "transcript not mirrored");
                }
                entries = next;
                core.set_hydrated();
                core.schedule_refresh();
                true
            },
        );

        let weak = Arc::downgrade(core);
        let host = Arc::downgrade(self);
        self.spawn_watch(
            cancel,
            methods::WATCH_QUEUE,
            json!({ "chatId": chat }),
            move |item| {
                let (Some(core), Some(host)) = (weak.upgrade(), host.upgrade()) else {
                    return true;
                };
                let Ok(items) = serde_json::from_value::<Vec<QueuedMessage>>(item["items"].clone())
                else {
                    return true;
                };
                lock(&host.engine_queue).insert(
                    core.chat_id.clone(),
                    items.iter().map(|q| q.id.clone()).collect(),
                );
                let written = core.write(|doc| {
                    for row in doc.read_queue()? {
                        doc.remove_queued(&row.id)?;
                    }
                    items.iter().try_for_each(|q| doc.push_queued(q))
                });
                if let Err(err) = written {
                    tracing::warn!(chat = %core.chat_id, error = %err, "queue not mirrored");
                }
                true
            },
        );
    }

    /// The session wrote a command or queue row into its doc: hand the new
    /// ones to the engine.
    pub(crate) fn on_command(&self, core: &Arc<SessionCore>) {
        let chat = core.chat_id.clone();
        let commands = core.doc().read_commands().unwrap_or_default();
        for command in commands {
            if command.status != SessionCommandStatus::Pending
                || !lock(&self.forwarded).insert(command.id.clone())
            {
                continue;
            }
            let _ = core.write(|doc| {
                doc.set_command_status(&command.id, SessionCommandStatus::Applied, None)
            });
            let _ = self.outbox.send(Out {
                method: methods::QUEUE_COMMAND,
                params: json!({ "chatId": chat, "command": command.payload }),
                command: Some((chat.clone(), command.id.clone())),
            });
        }
        // ponytail: new rows only; a local reorder isn't forwarded (the next
        // `WatchQueue` frame puts the engine's order back). Moving goes
        // through `MoveQueuedMessage` when that's needed.
        let known = lock(&self.engine_queue)
            .get(&chat)
            .cloned()
            .unwrap_or_default();
        for row in core.doc().read_queue().unwrap_or_default() {
            if known.contains(&row.id) || !lock(&self.forwarded).insert(row.id.clone()) {
                continue;
            }
            let _ = self.outbox.send(Out {
                method: methods::QUEUE_MESSAGE,
                params: json!({
                    "chatId": chat,
                    "text": row.text,
                    "attachments": row.attachments,
                    "holdForTurnEnd": row.hold_for_turn_end,
                }),
                command: None,
            });
        }
    }

    async fn drain_outbox(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<Out>) {
        loop {
            let out = tokio::select! {
                _ = self.cancel.cancelled() => return,
                out = rx.recv() => match out { Some(out) => out, None => return },
            };
            // Kept in order across a reconnect: retried until the engine answers.
            let reply = loop {
                let rpc = match self.rpc().await {
                    Ok(rpc) => rpc,
                    Err(_) if self.cancel.is_cancelled() => return,
                    Err(_) => continue,
                };
                match rpc.call(out.method, out.params.clone()).await {
                    Err(vardoger_rpc::RpcError::Closed | vardoger_rpc::RpcError::Transport(_)) => {
                        tokio::time::sleep(RETRY_BASE).await;
                    }
                    reply => break reply,
                }
            };
            let Err(err) = reply else { continue };
            tracing::warn!(method = out.method, error = %err, "local engine refused a write");
            if let Some((chat, command)) = out.command
                && let Some(core) = self.client.upgrade().and_then(|c| c.session_core(&chat))
            {
                let reason = err.to_string();
                let _ = core.write(|doc| {
                    doc.set_command_status(&command, SessionCommandStatus::Rejected, Some(&reason))
                });
            }
        }
    }

    /// Attachment bytes: off disk when the engine is on this machine, else
    /// `ReadAttachmentChunk` until done.
    pub(crate) async fn read_attachment(&self, path: &str) -> Result<Arc<Vec<u8>>> {
        if let EngineLink::Ipc { .. } = self.link {
            return tokio::fs::read(path)
                .await
                .map(Arc::new)
                .map_err(|e| ClientError::NotFound(format!("{path}: {e}")));
        }
        let mut out = Vec::new();
        let mut offset: u64 = 0;
        // ponytail: 1000 chunks cap, as the relay read always had.
        for _ in 0..1000 {
            let reply = self
                .call(
                    methods::READ_ATTACHMENT_CHUNK,
                    json!({ "path": path, "offset": offset }),
                )
                .await?;
            let data = reply["data"].as_str().unwrap_or_default();
            out.extend(
                crate::live::b64::decode(data)
                    .ok_or_else(|| ClientError::HostError("bad attachment chunk".into()))?,
            );
            if reply["done"].as_bool().unwrap_or(true) {
                return Ok(Arc::new(out));
            }
            let next = reply["nextOffset"].as_u64().unwrap_or(offset);
            if next <= offset {
                return Err(ClientError::HostError(
                    "attachment read made no progress".into(),
                ));
            }
            offset = next;
        }
        Err(ClientError::HostError("attachment too large".into()))
    }
}

/// Rewrite the doc's `messages` from the first entry that changed (a streamed
/// reply touches only the last).
// ponytail: whole entries from the first change; per-part patches
// (`partPatches`) if a long streamed reply's rewrites ever show up in a profile.
fn write_entries(
    doc: &SessionDoc,
    before: &[SessionMessageEntry],
    next: &[SessionMessageEntry],
) -> std::result::Result<(), vardoger_doc::DocError> {
    let same = before.iter().zip(next).take_while(|(a, b)| a == b).count();
    let list = doc.doc().get_list("messages");
    if list.len() > same {
        list.delete(same, list.len() - same)?;
        doc.commit();
    }
    next[same..]
        .iter()
        .try_for_each(|entry| doc.push_message(entry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vardoger_doc::{MessagePart, MessageRole};

    fn entry(id: &str, text: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: format!("{id}-t"),
                text: text.into(),
            }],
            created_at: 1,
            device_id: "d".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        }
    }

    #[test]
    fn entries_rewrite_from_the_first_change() {
        let doc = SessionDoc::init("c").unwrap();
        let a = vec![entry("1", "a"), entry("2", "b")];
        write_entries(&doc, &[], &a).unwrap();
        let b = vec![entry("1", "a"), entry("2", "bb"), entry("3", "c")];
        write_entries(&doc, &a, &b).unwrap();
        assert_eq!(doc.read_entries().unwrap(), b);
        write_entries(&doc, &b, &b[..1]).unwrap();
        assert_eq!(doc.read_entries().unwrap(), b[..1].to_vec());
    }
}
