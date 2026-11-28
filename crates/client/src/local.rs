//! The client against one engine, as its JSON-RPC client: on this machine
//! over its IPC (`ws://127.0.0.1:{port}`, the bearer from
//! `{data_dir}/ipc-token`), or on another of the account's machines through
//! the Commutator server's relay ([`EngineLink`]). Same logic either way.
//!
//! Shaped like the demo host: the registry is a local-only replica and every
//! open chat a local [`SessionDoc`], so the same session pipeline (snapshot,
//! composer, pending echoes) runs unchanged. The engine is the host:
//!
//! - registry: `WatchChats` / `WatchDevices` / `WatchSpaces` / `WatchSessions`
//!   (whole lists) folded into the replica; the engine's device beats presence
//!   while the socket is up;
//! - transcript: `WatchDocMessages` frames applied with
//!   [`commutator_doc::transcript_delta::apply_transcript_frame`], then written
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
use commutator_doc::transcript_delta::{TranscriptUpdate, apply_transcript_frame};
use commutator_doc::{QueuedMessage, SessionCommandStatus, SessionDoc, SessionMessageEntry};
use commutator_proto::{Chat, Device, Session, SidebarPinChange, SidebarPreferencesState, Space};
use commutator_rpc::{LinkCache, RpcClient, RpcError, methods};

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
            EngineLink::Ipc { port, data_dir } => commutator_rpc::connect_ipc(*port, Some(data_dir))
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
    /// Pin writes the engine hasn't answered yet, and its newest pin list
    /// held back meanwhile so it can't undo the optimistic replica.
    pins: Mutex<EnginePins>,
    cancel: CancellationToken,
}

#[derive(Default)]
struct EnginePins {
    in_flight: usize,
    latest: Option<SidebarPreferencesState>,
}

/// Default unary deadline; long ones below.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Raw bytes per UploadChunk slice (≈680k base64 chars: under a 1 MiB
/// WebSocket message with headroom; a multiple of 3 so slices encode alone).
const UPLOAD_SLICE_BYTES: usize = 510_000;

/// Make the replica's pins the engine's list (`target`, in order). A pin
/// whose chat the replica doesn't have yet is left for the next frame.
fn mirror_pins(
    doc: &mut commutator_doc::RegistryDoc,
    target: &[String],
) -> std::result::Result<(), commutator_doc::DocError> {
    doc.initialize_sidebar_pins();
    let current = doc.sidebar_preferences().unwrap_or_default().pinned_session_ids;
    if current == target {
        return Ok(());
    }
    for id in current.iter().filter(|id| !target.contains(id)) {
        doc.change_sidebar_pin(&SidebarPinChange::Unpin { session_id: id.clone() })?;
    }
    let mut after = None;
    for id in target {
        let pin = SidebarPinChange::Pin { session_id: id.clone(), after: after.clone(), before: None };
        match doc.change_sidebar_pin(&pin) {
            Ok(()) => after = Some(id.clone()),
            Err(commutator_doc::DocError::Schema(_)) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn deadline(method: &str) -> Duration {
    match method {
        methods::CREATE_WORKTREE => Duration::from_secs(120),
        methods::LIST_MODELS => Duration::from_secs(100),
        methods::UPLOAD_COMMIT => Duration::from_secs(150),
        _ => CALL_TIMEOUT,
    }
}

fn rpc_err(method: &str, err: RpcError) -> ClientError {
    match err {
        RpcError::UnknownMethod(m) => ClientError::Unsupported(format!("{m} on the engine")),
        RpcError::BadParams(m) | RpcError::Failed(m) | RpcError::Capability(m) => {
            ClientError::HostError(format!("{method}: {m}"))
        }
        RpcError::Transport(m) => ClientError::HostUnavailable(format!("engine: {m}")),
        RpcError::Closed => ClientError::HostUnavailable("engine: connection closed".into()),
    }
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
            // Chats a cached replica brought along count as the engine's:
            // its first listing then drops the ones it no longer has.
            engine_chats: Mutex::new(
                client.workspace.state().0.chats.iter().map(|c| c.id.clone()).collect(),
            ),
            pins: Mutex::new(EnginePins::default()),
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
            methods::WATCH_SIDEBAR_PREFERENCES,
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

    /// Whether the engine's connection is up.
    pub(crate) fn connected(&self) -> bool {
        self.rpc.borrow().is_some()
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
        let rpc = self.rpc().await?;
        match tokio::time::timeout(deadline(method), rpc.call(method, params)).await {
            Ok(reply) => reply.map_err(|e| rpc_err(method, e)),
            Err(_) => Err(ClientError::HostUnavailable(format!("{method} timed out"))),
        }
    }

    /// Chunked upload (`UploadChunk` ×N with `seq`, then `UploadCommit`);
    /// returns the durable path on the engine. Idempotent per `upload_id`.
    // ponytail: one slice at a time; a few in parallel if large uploads drag.
    pub(crate) async fn upload(
        &self,
        upload_id: &str,
        file_name: &str,
        data: &[u8],
        progress: Option<crate::rpc::ProgressFn>,
    ) -> Result<String> {
        let slices: Vec<&[u8]> = if data.is_empty() {
            vec![&[][..]]
        } else {
            data.chunks(UPLOAD_SLICE_BYTES).collect()
        };
        for (seq, slice) in slices.iter().enumerate() {
            if let Some(progress) = &progress {
                progress((seq as f64 / slices.len() as f64).min(0.99));
            }
            self.call(
                methods::UPLOAD_CHUNK,
                json!({ "uploadId": upload_id, "seq": seq, "data": crate::b64::encode(slice) }),
            )
            .await?;
        }
        let reply = self
            .call(
                methods::UPLOAD_COMMIT,
                json!({ "uploadId": upload_id, "fileName": file_name }),
            )
            .await?;
        if let Some(progress) = &progress {
            progress(1.0);
        }
        reply["path"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ClientError::HostError("UploadCommit returned no path".into()))
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
                        // Not part of an upsert: without this the replica
                        // never sees a sleep set or woken elsewhere.
                        doc.mirror_chat_sleep(&chat.id, chat.sleep.as_ref())?;
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
            methods::WATCH_SIDEBAR_PREFERENCES => {
                let Ok(state) = serde_json::from_value::<SidebarPreferencesState>(item) else {
                    return;
                };
                self.engine_pins(Some(state), false);
                return;
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

    /// The engine's pin list, from its watch or a pin write's reply
    /// (`answered`: one fewer write in flight). Kept if newest; mirrored into
    /// the replica only once no pin write is in flight.
    fn engine_pins(&self, state: Option<SidebarPreferencesState>, answered: bool) {
        let apply = {
            let mut pins = lock(&self.pins);
            if answered {
                pins.in_flight = pins.in_flight.saturating_sub(1);
            }
            if let Some(state) = state
                && pins.latest.as_ref().is_none_or(|l| state.revision >= l.revision)
            {
                pins.latest = Some(state);
            }
            match &pins.latest {
                // Not `initialized`: an engine without a sync room never
                // flags it, yet its list is the only one there is.
                Some(latest) if pins.in_flight == 0 => {
                    Some(latest.pinned_session_ids.clone())
                }
                _ => None,
            }
        };
        let (Some(target), Some(client)) = (apply, self.client.upgrade()) else {
            return;
        };
        if let Err(err) = client.registry_write(|doc| mirror_pins(doc, &target)) {
            tracing::warn!(error = %err, "engine pins not mirrored");
        }
    }

    /// A workspace write (`Mutate {op, …}`) the replica already holds.
    pub(crate) fn mutate(&self, params: Value) {
        if params["op"] == "changeSidebarPin" {
            lock(&self.pins).in_flight += 1;
        }
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
                    Err(commutator_rpc::RpcError::Closed | commutator_rpc::RpcError::Transport(_)) => {
                        tokio::time::sleep(RETRY_BASE).await;
                    }
                    reply => break reply,
                }
            };
            if out.method == methods::MUTATE && out.params["op"] == "changeSidebarPin" {
                // Refused or not, the engine's list is the truth again.
                let state = reply.as_ref().ok().and_then(|r| {
                    serde_json::from_value(r["sidebarPreferences"].clone()).ok()
                });
                self.engine_pins(state, true);
            }
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
                crate::b64::decode(data)
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
) -> std::result::Result<(), commutator_doc::DocError> {
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
    use commutator_doc::{MessagePart, MessageRole};

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
    fn engine_pins_mirror_into_the_replica() {
        let mut doc = commutator_doc::RegistryDoc::new("phone");
        for id in ["a", "b", "c"] {
            let chat: Chat = serde_json::from_value(json!({
                "id": id, "deviceId": "mac", "archived": false,
                "createdAt": "2026-11-27T00:00:00Z", "updatedAt": "2026-11-27T00:00:00Z",
            }))
            .unwrap();
            doc.upsert_chat(&chat).unwrap();
        }
        let pins = |doc: &commutator_doc::RegistryDoc| {
            doc.sidebar_preferences().unwrap().pinned_session_ids
        };
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        mirror_pins(&mut doc, &[]).unwrap();
        assert!(pins(&doc).is_empty(), "an empty list still marks pins ready");
        mirror_pins(&mut doc, &ids(&["b", "a", "gone"])).unwrap();
        assert_eq!(pins(&doc), ids(&["b", "a"]));
        mirror_pins(&mut doc, &ids(&["c", "b"])).unwrap();
        assert_eq!(pins(&doc), ids(&["c", "b"]));
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
