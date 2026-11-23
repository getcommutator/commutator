//! [`Client`] — the account-scoped root object.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use tokio_util::sync::CancellationToken;
use vardoger_doc::RegistryDoc;
use vardoger_proto::{Chat, ChatConfig, SidebarPinChange, SidebarSectionChange};

use crate::attachments::{self, AttachmentCache};
use crate::catalog::{self, HarnessInfo, ModelInfo};
use crate::config::{ClientConfig, Credentials};
use crate::connectivity::{
    Connectivity, ConnectivityState, ConnectivityTracker, RawConnectivity, SendState,
};
use crate::demo::DemoHost;
use crate::error::{ClientError, Result};
use crate::events::{ClientEvent, ClientListener, EventPump};
use crate::local::{EngineLink, LocalHost};
use crate::rpc::{FolderListing, ProgressFn, QUEUED_ATTACHMENTS_MIN, RepoRef, capability};
use crate::session::{HostCapabilities, OutgoingAttachment, RoomState, SessionCore, SessionHandle};
use crate::workspace::{SearchHit, WorkspaceSnapshot, WorkspaceStore};
use crate::{lock, now_ms, read, write};

/// Attachment read cache budget.
const ATTACHMENT_CACHE_BYTES: usize = 48 * 1024 * 1024;
/// Time-driven re-derivation cadence (staleness, presence, send grace).
const TICK: Duration = Duration::from_secs(1);
/// Detached sessions kept warm (doc + watches) before the least recently used
/// is evicted. On-screen sessions, streaming ones and ones with unadopted
/// sends are never evicted.
pub const WARM_SESSION_CAP: usize = 6;
/// Sessions `preload_sessions` warms (front page order).
pub const PRELOAD_CAP: usize = 4;

/// Where a new session runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTarget {
    /// In a project: host = the project's owning device, cwd = its folder
    /// (or `cwd` override, e.g. a picked worktree).
    Project { space_id: String },
    /// No project: runs in the chosen host's home folder (`~`).
    Projectless { device_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewSession {
    pub target: SessionTarget,
    pub config: Option<ChatConfig>,
    /// Branch to stamp from the first frame (picked ref).
    pub branch: Option<String>,
    /// Checkout override (an existing worktree's path).
    pub cwd: Option<String>,
    pub title: Option<String>,
}

pub(crate) enum Backend {
    Demo(Arc<DemoHost>),
    /// An engine: this machine's over IPC, or another's through the server
    /// relay ([`crate::local`]).
    Local(Arc<LocalHost>),
}

pub(crate) struct ClientInner {
    pub(crate) config: ClientConfig,
    pub(crate) credentials: Credentials,
    pub(crate) events: Arc<EventPump>,
    pub(crate) workspace: WorkspaceStore,
    pub(crate) attachment_cache: AttachmentCache,
    backend: OnceLock<Backend>,
    sessions: Mutex<HashMap<String, Arc<SessionCore>>>,
    pub(crate) cancel: CancellationToken,
    connectivity_tracker: Mutex<ConnectivityTracker>,
    connectivity: RwLock<Connectivity>,
    path_online: AtomicBool,
    foreground: AtomicBool,
    synced: AtomicBool,
    pub(crate) harness_catalogs: Mutex<HashMap<String, Vec<HarnessInfo>>>,
    /// Terminals on the host, over its RPC connection.
    pub(crate) remote_terminals: vardoger_term_link::remote::RemoteTerminals,
    /// Wakes terminal streams waiting to reconnect (foreground, network back).
    pub(crate) terminals_wake: Arc<tokio::sync::Notify>,
    /// Remote mode: WebRTC to the account's devices, once started ([`crate::direct`]).
    pub(crate) direct: Mutex<Option<vardoger_preview::viewer::ViewerPeers>>,
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        // Released without `shutdown()`: stop every background task.
        self.cancel.cancel();
    }
}

impl ClientInner {
    pub(crate) fn backend(&self) -> &Backend {
        self.backend.get().expect("backend initialized")
    }

    pub(crate) fn demo(&self) -> Option<&Arc<DemoHost>> {
        match self.backend() {
            Backend::Demo(demo) => Some(demo),
            _ => None,
        }
    }

    pub(crate) fn is_demo(&self) -> bool {
        self.demo().is_some()
    }

    pub(crate) fn local(&self) -> Option<&Arc<LocalHost>> {
        match self.backend.get()? {
            Backend::Local(local) => Some(local),
            _ => None,
        }
    }

    /// Local mode: the engine listed its chats (the workspace is current).
    pub(crate) fn set_synced(&self) {
        self.synced.store(true, Ordering::Release);
    }

    pub(crate) fn session_core(&self, chat_id: &str) -> Option<Arc<SessionCore>> {
        lock(&self.sessions).get(chat_id).cloned()
    }

    pub(crate) fn cores(&self) -> Vec<Arc<SessionCore>> {
        lock(&self.sessions).values().cloned().collect()
    }

    pub(crate) fn network_online(&self) -> bool {
        self.path_online.load(Ordering::Acquire)
    }

    /// Re-derive the workspace snapshot; on change, notify and refresh every
    /// open composer (host presence / live status feed them).
    pub(crate) fn recompute_workspace(self: &Arc<Self>) {
        let send_states: HashMap<String, SendState> = self
            .cores()
            .iter()
            .filter_map(|core| core.send_state().map(|s| (core.chat_id.clone(), s)))
            .collect();
        let synced = self.synced.load(Ordering::Acquire);
        if let Some(revision) =
            self.workspace
                .recompute(&self.config.device_id, &send_states, synced)
        {
            self.events.workspace(revision);
            // Host presence / live status feed every open session's snapshot
            // (working flag) and composer; `refresh` is O(1) when no doc
            // event is pending.
            for core in self.cores() {
                core.refresh();
            }
        }
    }

    /// After any local registry write: re-derive (the engine gets its own
    /// copy through `Mutate`).
    pub(crate) fn after_registry_write(self: &Arc<Self>) {
        self.recompute_workspace();
    }

    pub(crate) fn registry_write<R>(
        self: &Arc<Self>,
        f: impl FnOnce(&mut RegistryDoc) -> std::result::Result<R, vardoger_doc::DocError>,
    ) -> Result<R> {
        let result = self.workspace.mutate(f)?;
        self.after_registry_write();
        Ok(result)
    }

    pub(crate) fn mark_seen(self: &Arc<Self>, chat_id: &str) {
        let _ = self.registry_write(|doc| doc.set_chat_seen(chat_id, Utc::now()));
    }

    pub(crate) fn recompute_connectivity(self: &Arc<Self>) {
        let raw = RawConnectivity {
            path_offline: !self.network_online(),
            // The engine's connection stands in for the old registry room.
            registry_connected: self.local().is_none_or(|local| local.connected()),
            registry_retry_at_ms: None,
            last_failure: None,
            chat_rooms: Vec::new(),
        };
        let next = lock(&self.connectivity_tracker).compute(&raw, now_ms());
        let changed = {
            let mut current = write(&self.connectivity);
            if *current == next {
                false
            } else {
                *current = next.clone();
                true
            }
        };
        if changed {
            self.events.ordered(ClientEvent::ConnectivityChanged(next));
            for core in self.cores() {
                core.refresh();
            }
            if let Some(demo) = self.demo()
                && self.network_online()
            {
                demo.network_recovered();
            }
        }
    }

    pub(crate) fn connectivity(&self) -> Connectivity {
        read(&self.connectivity).clone()
    }

    /// Whether a send to this chat would queue rather than deliver promptly
    /// (legacy `chatDeliveryDegraded`): OS offline, the engine unreachable,
    /// or the host presence-dark.
    pub(crate) fn chat_delivery_degraded(&self, chat_id: &str) -> bool {
        let connectivity = self.connectivity();
        if connectivity.state == ConnectivityState::Offline {
            return true;
        }
        if connectivity.degraded_chats.iter().any(|c| c == chat_id) {
            return true;
        }
        if !matches!(
            connectivity.state,
            ConnectivityState::Connected | ConnectivityState::Disabled
        ) {
            return true;
        }
        self.workspace
            .snapshot()
            .session(chat_id)
            .is_some_and(|row| !row.device_online)
    }

    pub(crate) fn room_state(&self, chat_id: &str) -> RoomState {
        let connectivity = self.connectivity();
        let degraded = connectivity.degraded_chats.iter().any(|c| c == chat_id);
        RoomState {
            connected: connectivity.state != ConnectivityState::Offline,
            retry_at_ms: connectivity.retry_at_ms,
            degraded,
        }
    }

    pub(crate) fn host_capabilities(
        &self,
        device_id: &str,
        harness: Option<&str>,
    ) -> HostCapabilities {
        let workspace = self.workspace.snapshot();
        let Some(device) = workspace.device(device_id) else {
            return HostCapabilities::default();
        };
        let has = |cap: &str| device.capabilities.iter().any(|c| c == cap);
        let mid_turn_steering = harness.and_then(|harness| {
            lock(&self.harness_catalogs)
                .get(device_id)
                .and_then(|list| list.iter().find(|h| h.id == harness))
                .and_then(HarnessInfo::mid_turn_steering)
        });
        HostCapabilities {
            message_queue: has(capability::MESSAGE_QUEUE_V1),
            queue_actions: has(capability::MESSAGE_QUEUE_ACTIONS_V1),
            queue_attachments: has(capability::MESSAGE_QUEUE_ATTACHMENTS_V1),
            clean_attachment_text: has(capability::MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1),
            queue_edit_lease: has(capability::MESSAGE_QUEUE_EDIT_LEASE_V1),
            queued_attachments: device
                .version
                .as_deref()
                .and_then(vardoger_proto::version_triple)
                .is_some_and(|v| v >= QUEUED_ATTACHMENTS_MIN),
            mid_turn_steering,
            provider_switch: has(capability::PROVIDER_SWITCH_V1),
            attachment_store: has(capability::ATTACHMENT_STORE_V1),
        }
    }

    /// Mint `pending://` refs for picked images and keep their bytes local
    /// (the echo renders them immediately).
    pub(crate) fn stage_attachments(
        &self,
        device_id: &str,
        attachments: &[OutgoingAttachment],
    ) -> Result<Vec<String>> {
        if let Some(too_big) = attachments
            .iter()
            .find(|a| a.data.len() > attachments::MAX_ATTACHMENT_BYTES)
        {
            return Err(ClientError::InvalidArgument(format!(
                "{} is larger than 24 MB",
                too_big.name
            )));
        }
        attachments
            .iter()
            .map(|attachment| {
                let upload_id = crate::new_id();
                let name = attachments::upload_file_name(&attachment.name);
                let reference = attachments::pending_ref(&upload_id, &name);
                self.attachment_cache
                    .put(device_id, &reference, Arc::new(attachment.data.clone()));
                Ok(reference)
            })
            .collect()
    }

    /// A command/queue row was written: hand it to the host.
    pub(crate) fn after_command(self: &Arc<Self>, core: &Arc<SessionCore>) {
        match self.backend() {
            Backend::Demo(demo) => demo.on_command(&core.chat_id),
            Backend::Local(local) => local.on_command(core),
        }
    }

    pub(crate) async fn host_rpc(
        self: &Arc<Self>,
        device_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        match self.backend() {
            Backend::Demo(demo) => demo.host_rpc(device_id, method, params).await,
            Backend::Local(local) => local.call(method, params).await,
        }
    }

    /// A host stream (terminal output, a subagent's transcript): the
    /// engine's subscription, the demo host's channel offline.
    pub(crate) async fn host_subscribe(
        self: &Arc<Self>,
        device_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<crate::host_stream::HostStream> {
        match self.backend() {
            Backend::Demo(demo) => demo.host_subscribe(device_id, method, params).await,
            Backend::Local(local) => local.subscribe(method, params).await,
        }
    }

    /// Drop the least recently used detached, quiet sessions past the cap.
    pub(crate) fn evict_sessions(&self) {
        let mut sessions = lock(&self.sessions);
        let mut idle: Vec<(i64, String)> = sessions
            .values()
            .filter(|core| {
                !core.view_attached() && !core.has_pending_sends() && !core.snapshot().streaming
            })
            .map(|core| (core.touched_ms(), core.chat_id.clone()))
            .collect();
        if idle.len() <= WARM_SESSION_CAP {
            return;
        }
        idle.sort();
        let excess = idle.len() - WARM_SESSION_CAP;
        for (_, chat_id) in idle.into_iter().take(excess) {
            tracing::debug!(chat = %chat_id, "evicting warm session");
            sessions.remove(&chat_id);
        }
    }

    fn tick(self: &Arc<Self>) {
        self.recompute_connectivity();
        self.recompute_workspace();
        for core in self.cores() {
            if core.has_pending_sends() {
                core.refresh();
            }
            // An empty transcript's patience runs out with no event to say so.
            if core.view_attached() {
                core.recompute_composer(self);
            }
        }
    }
}

/// One signed-in account (or Demo mode). Cheap to clone.
#[derive(Clone)]
pub struct Client {
    pub(crate) inner: Arc<ClientInner>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("device_id", &self.inner.config.device_id)
            .field("demo", &self.inner.is_demo())
            .finish()
    }
}

impl Client {
    /// Dedicated ephemeral call handle; never uses durable commands or RPC retries.
    pub async fn voice_transport(
        &self,
        device_id: &str,
    ) -> Result<Arc<vardoger_voice_session::RpcTransport>> {
        let local = self.inner.local().ok_or_else(|| {
            ClientError::Unsupported("voice needs an engine".into())
        })?;
        Ok(Arc::new(vardoger_voice_session::RpcTransport {
            client: local.rpc_client().await?,
            host: device_id.into(),
        }))
    }
    /// Sign-out/shutdown closes every call created by this client.
    pub fn voice_cancellation(&self) -> CancellationToken {
        self.inner.cancel.child_token()
    }

    /// Build and start. Never blocks on the network: an engine connects in
    /// the background; Demo seeds its dataset.
    pub fn new(
        config: ClientConfig,
        credentials: Credentials,
        listener: Arc<dyn ClientListener>,
    ) -> Result<Self> {
        if config.device_id.trim().is_empty() {
            return Err(ClientError::InvalidArgument("device_id is required".into()));
        }
        let events = EventPump::new(listener);
        // Nothing acks a write to this replica (the engine keeps its own),
        // so it folds writes straight into its rows.
        let mut registry = RegistryDoc::new(config.device_id.clone());
        registry.set_local_only(true);
        let inner = Arc::new(ClientInner {
            workspace: WorkspaceStore::new(registry),
            events: events.clone(),
            attachment_cache: AttachmentCache::new(ATTACHMENT_CACHE_BYTES),
            backend: OnceLock::new(),
            sessions: Mutex::new(HashMap::new()),
            cancel: CancellationToken::new(),
            connectivity_tracker: Mutex::new(ConnectivityTracker::default()),
            connectivity: RwLock::new(Connectivity {
                state: if credentials.is_demo() {
                    ConnectivityState::Connected
                } else {
                    ConnectivityState::Reconnecting
                },
                ..Default::default()
            }),
            path_online: AtomicBool::new(true),
            foreground: AtomicBool::new(true),
            synced: AtomicBool::new(false),
            harness_catalogs: Mutex::new(HashMap::new()),
            remote_terminals: Default::default(),
            terminals_wake: Arc::new(tokio::sync::Notify::new()),
            direct: Mutex::new(None),
            credentials: credentials.clone(),
            config,
        });
        let backend = match &credentials {
            Credentials::Demo(options) => {
                let demo = DemoHost::new(&inner, options.clone());
                demo.seed(&inner)?;
                inner.synced.store(true, Ordering::Release);
                Backend::Demo(demo)
            }
            Credentials::Local {
                port,
                engine_data_dir,
            } => Backend::Local(LocalHost::new(
                &inner,
                EngineLink::Ipc {
                    port: *port,
                    data_dir: engine_data_dir.clone(),
                },
            )),
            Credentials::Remote {
                server_url,
                token,
                host_device_id,
            } => {
                let links = {
                    let _runtime = crate::runtime::handle().enter();
                    vardoger_rpc::LinkCache::new(vardoger_rpc::LinkCacheConfig::new(
                        server_url.trim_end_matches('/'),
                        Arc::new(vardoger_rpc::StaticToken(token.clone())),
                    ))
                };
                Backend::Local(LocalHost::new(
                    &inner,
                    EngineLink::Relay {
                        links,
                        device_id: host_device_id.clone(),
                    },
                ))
            }
        };
        if !credentials.is_demo() {
            catalog::DiskCatalog::new(&inner.config.data_dir).warm_labels();
        }
        let _ = inner.backend.set(backend);
        events.start(inner.cancel.clone());
        inner.recompute_workspace();
        match inner.backend() {
            Backend::Demo(demo) => demo.start(&inner, inner.cancel.clone()),
            Backend::Local(local) => local.start(),
        }
        // A previous account may have left the process at rest.
        if !inner.is_demo() {
            vardoger_sync::wake::set_resting(false);
        }
        let ticker = Arc::downgrade(&inner);
        let cancel = inner.cancel.clone();
        crate::runtime::shared().spawn(async move {
            let mut interval = tokio::time::interval(TICK);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = interval.tick() => {}
                }
                let Some(inner) = ticker.upgrade() else {
                    return;
                };
                if inner.foreground.load(Ordering::Acquire) {
                    inner.tick();
                }
            }
        });
        Ok(Self { inner })
    }

    pub fn config(&self) -> &ClientConfig {
        &self.inner.config
    }

    pub fn is_demo(&self) -> bool {
        self.inner.is_demo()
    }

    pub fn device_id(&self) -> &str {
        &self.inner.config.device_id
    }

    pub fn user_id(&self) -> &str {
        self.inner.credentials.user_id()
    }

    pub fn org_id(&self) -> &str {
        self.inner.credentials.org_id()
    }

    /// Stop every background task (sign-out). Snapshots stay readable.
    pub fn shutdown(&self) {
        match self.inner.backend() {
            Backend::Demo(demo) => demo.stop(),
            Backend::Local(local) => local.stop(),
        }
        self.inner.cancel.cancel();
        lock(&self.inner.sessions).clear();
        self.inner.attachment_cache.clear();
    }

    // ── workspace reads ────────────────────────────────────────────────────

    /// O(1): the current derived workspace.
    pub fn workspace(&self) -> Arc<WorkspaceSnapshot> {
        self.inner.workspace.snapshot()
    }

    pub fn connectivity(&self) -> Connectivity {
        self.inner.connectivity()
    }

    /// The chat's run configuration (harness/model/effort/options/sandbox).
    pub fn session_config(&self, chat_id: &str) -> Option<ChatConfig> {
        self.inner.workspace.chat(chat_id).and_then(|c| c.config)
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        self.workspace().search(query, limit)
    }

    // ── workspace writes ───────────────────────────────────────────────────

    /// Mint a chat row (born on chat2: `roomGen: 2`). The row is written
    /// before any session doc opens, so the host never reads it as legacy.
    pub fn create_session(&self, new: NewSession) -> Result<String> {
        let (state, _) = self.inner.workspace.state();
        let id = crate::new_id();
        let now = Utc::now();
        let (device_id, space_id, cwd) = match &new.target {
            SessionTarget::Project { space_id } => {
                let space = state
                    .spaces
                    .iter()
                    .find(|s| &s.id == space_id)
                    .ok_or_else(|| ClientError::NotFound(space_id.clone()))?;
                (
                    space.device_id.clone(),
                    Some(space.id.clone()),
                    new.cwd.clone().unwrap_or_else(|| space.path.clone()),
                )
            }
            SessionTarget::Projectless { device_id } => {
                let host = state
                    .devices
                    .iter()
                    .find(|d| &d.id == device_id)
                    .ok_or_else(|| ClientError::NotFound(device_id.clone()))?;
                if matches!(host.platform.as_str(), "ios" | "android" | "ipados") {
                    return Err(ClientError::InvalidArgument(
                        "that device can't host sessions".into(),
                    ));
                }
                // Its own folder on the host, created when the first run
                // spawns; a host that predates scratch folders runs it in `~`.
                let cwd = if host
                    .capabilities
                    .iter()
                    .any(|c| c == crate::rpc::capability::PROJECTLESS_SCRATCH_V1)
                {
                    vardoger_proto::projectless::scratch_cwd(&id, now)
                } else {
                    "~".to_owned()
                };
                (device_id.clone(), None, cwd)
            }
        };
        let chat = Chat {
            id,
            device_id,
            title: new.title.filter(|t| !t.trim().is_empty()),
            archived: false,
            cwd: Some(cwd),
            branch: new.branch,
            checkout_id: None,
            source_context: None,
            config: new.config,
            last_message_preview: None,
            last_message_at: None,
            created_at: now,
            harness_session_id: None,
            harness_session_cwd: None,
            harness_session_harness: None,
            space_id,
            last_seen_at: None,
            room_gen: Some(2),
            parent_chat_id: None,
            child_kind: None,
            sleep: None,
        };
        let id = chat.id.clone();
        self.inner.registry_write(|doc| doc.upsert_chat(&chat))?;
        if let Some(local) = self.inner.local() {
            local.chat_created(&chat);
        }
        Ok(id)
    }

    /// A fresh side chat under `parent_id`: same host, folder, checkout and
    /// agent settings as its parent, an empty conversation (the desktop's
    /// "New side chat"; agent-spawned side chats have the same shape).
    pub fn create_side_chat(&self, parent_id: &str) -> Result<String> {
        let parent = self
            .inner
            .workspace
            .chat(parent_id)
            .ok_or_else(|| ClientError::NotFound(parent_id.to_owned()))?;
        // One level deep: a side chat's own side chat hangs off the root.
        let root = parent.parent_chat_id.clone().unwrap_or_else(|| parent.id.clone());
        let mut chat = parent;
        chat.id = crate::new_id();
        chat.parent_chat_id = Some(root);
        chat.title = None;
        chat.archived = false;
        chat.created_at = Utc::now();
        chat.last_message_at = None;
        chat.last_message_preview = None;
        chat.last_seen_at = None;
        chat.harness_session_id = None;
        chat.harness_session_cwd = None;
        chat.harness_session_harness = None;
        chat.room_gen = Some(2);
        let id = chat.id.clone();
        self.inner.registry_write(|doc| doc.upsert_chat(&chat))?;
        if let Some(local) = self.inner.local() {
            local.chat_created(&chat);
        }
        Ok(id)
    }

    /// Fork `source_id` through its latest completed reply into a side chat
    /// (`ForkSideChat`, answered by the source's host, which copies the
    /// conversation). A side chat forks as a sibling, under the same parent.
    pub async fn fork_side_chat(&self, source_id: &str) -> Result<String> {
        let source = self
            .inner
            .workspace
            .chat(source_id)
            .ok_or_else(|| ClientError::NotFound(source_id.to_owned()))?;
        let parent = source.parent_chat_id.clone().unwrap_or_else(|| source.id.clone());
        let chat_id = crate::new_id();
        let reply = self
            .inner
            .host_rpc(
                &source.device_id,
                vardoger_rpc::methods::FORK_SIDE_CHAT,
                serde_json::json!({
                    "chatId": chat_id,
                    "sourceChatId": source.id,
                    "parentChatId": parent,
                }),
            )
            .await?;
        // The host writes the fork's row; it reaches this device through
        // registry sync on its own time. Hold the reply's copy now so the
        // fork opens at once (the same row converges when sync lands).
        match serde_json::from_value::<Chat>(reply) {
            Ok(chat) => {
                if self.inner.workspace.chat(&chat.id).is_none() {
                    self.inner.registry_write(|doc| doc.upsert_chat(&chat))?;
                }
                Ok(chat.id)
            }
            Err(_) => Ok(chat_id),
        }
    }

    fn chat_write(
        &self,
        chat_id: &str,
        f: impl FnOnce(&mut RegistryDoc) -> std::result::Result<bool, vardoger_doc::DocError>,
    ) -> Result<()> {
        if self.inner.registry_write(f)? {
            Ok(())
        } else {
            Err(ClientError::NotFound(chat_id.to_owned()))
        }
    }

    pub fn archive_session(&self, chat_id: &str) -> Result<()> {
        self.chat_write(chat_id, |doc| doc.set_chat_archived(chat_id, true))?;
        self.local_mutate(
            serde_json::json!({"op": "setChatArchived", "chatId": chat_id, "archived": true}),
        );
        Ok(())
    }

    pub fn unarchive_session(&self, chat_id: &str) -> Result<()> {
        self.chat_write(chat_id, |doc| doc.set_chat_archived(chat_id, false))?;
        self.local_mutate(
            serde_json::json!({"op": "setChatArchived", "chatId": chat_id, "archived": false}),
        );
        Ok(())
    }

    /// Local mode: the engine takes the workspace write the replica just did.
    fn local_mutate(&self, params: serde_json::Value) {
        if let Some(local) = self.inner.local() {
            local.mutate(params);
        }
    }

    /// Put a chat to sleep until `until_ms` (Unix ms; the Activity view's
    /// Asleep), or wake it now (`None`). Synced per field, on every device;
    /// the chat's host wakes it early the moment it asks for the person or
    /// fails.
    pub fn sleep_session(&self, chat_id: &str, until_ms: Option<i64>) -> Result<()> {
        let until = match until_ms {
            Some(at) => Some(
                Utc.timestamp_millis_opt(at)
                    .single()
                    .ok_or_else(|| ClientError::InvalidArgument("sleep: no such time".into()))?,
            ),
            None => None,
        };
        self.chat_write(chat_id, |doc| doc.set_chat_sleep(chat_id, until, Utc::now()))
    }

    /// Stop waiting on the background processes a chat's agent said it
    /// waits on (its host's `StopWaiting`): its turn then reads as finished.
    /// `false` when it was no longer waiting.
    pub async fn stop_waiting(&self, chat_id: &str) -> Result<bool> {
        let chat = self
            .inner
            .workspace
            .chat(chat_id)
            .ok_or_else(|| ClientError::NotFound(chat_id.to_owned()))?;
        let reply = self
            .inner
            .host_rpc(
                &chat.device_id,
                vardoger_rpc::methods::STOP_WAITING,
                serde_json::json!({ "chatId": chat_id }),
            )
            .await?;
        Ok(reply["stopped"].as_bool().unwrap_or(false))
    }

    pub fn rename_session(&self, chat_id: &str, title: &str) -> Result<()> {
        self.chat_write(chat_id, |doc| doc.rename_chat(chat_id, title.trim()))?;
        self.local_mutate(
            serde_json::json!({"op": "renameChat", "chatId": chat_id, "title": title.trim()}),
        );
        Ok(())
    }

    pub fn mark_seen(&self, chat_id: &str) {
        self.inner.mark_seen(chat_id);
    }

    pub fn set_session_config(&self, chat_id: &str, config: &ChatConfig) -> Result<()> {
        // A provider change needs a host that switches: an older one would
        // resume the chat's stored session under the new provider.
        if let Some(chat) = self.inner.workspace.chat(chat_id)
            && chat
                .config
                .as_ref()
                .is_some_and(|current| current.harness != config.harness)
            && !self
                .inner
                .host_capabilities(&chat.device_id, None)
                .provider_switch
        {
            return Err(ClientError::Unsupported(
                "This chat's computer needs a newer Vardoger to switch provider.".into(),
            ));
        }
        self.chat_write(chat_id, |doc| doc.set_chat_config(chat_id, config))?;
        self.local_mutate(
            serde_json::json!({"op": "setChatConfig", "chatId": chat_id, "config": config}),
        );
        Ok(())
    }

    /// Remove the chat row (the engine deletes its own).
    pub fn delete_session(&self, chat_id: &str) -> Result<()> {
        self.chat_write(chat_id, |doc| doc.delete_chat(chat_id))?;
        self.local_mutate(serde_json::json!({"op": "deleteChat", "chatId": chat_id}));
        self.forget_local_session(chat_id);
        Ok(())
    }

    /// Drop a deleted chat's open session.
    fn forget_local_session(&self, chat_id: &str) {
        lock(&self.inner.sessions).remove(chat_id);
    }

    /// Mid-session ref switch: retarget onto the ref's existing worktree (row
    /// writes only), else `git checkout` in the session's own folder.
    pub async fn switch_session_ref(&self, chat_id: &str, reference: RepoRef) -> Result<()> {
        let chat = self
            .inner
            .workspace
            .chat(chat_id)
            .ok_or_else(|| ClientError::NotFound(chat_id.to_owned()))?;
        let cwd = chat
            .cwd
            .clone()
            .ok_or_else(|| ClientError::InvalidArgument("session has no working folder".into()))?;
        if let Some(worktree) = reference.worktree_path.clone() {
            if worktree != cwd {
                self.inner.registry_write(|doc| {
                    doc.set_chat_cwd(chat_id, &worktree)?;
                    doc.set_chat_branch(chat_id, &reference.name)
                })?;
            }
            return Ok(());
        }
        self.switch_ref(&chat.device_id, &cwd, &reference.name)
            .await?;
        self.inner
            .registry_write(|doc| doc.set_chat_branch(chat_id, &reference.name))?;
        Ok(())
    }

    fn pin_change(&self, change: SidebarPinChange) -> Result<()> {
        if !self.workspace().pins_ready {
            return Err(ClientError::Unsupported("pins are not synced yet".into()));
        }
        self.inner
            .registry_write(|doc| doc.change_sidebar_pin(&change))
    }

    pub fn pin_session(&self, chat_id: &str) -> Result<()> {
        let after = self
            .inner
            .workspace
            .state()
            .1
            .and_then(|p| p.pinned_session_ids.last().cloned())
            .filter(|last| last != chat_id);
        self.pin_change(SidebarPinChange::Pin {
            session_id: chat_id.to_owned(),
            after,
            before: None,
        })
    }

    /// Pin between neighbours (either may be `None` at the ends).
    pub fn pin_session_at(
        &self,
        chat_id: &str,
        after: Option<String>,
        before: Option<String>,
    ) -> Result<()> {
        self.pin_change(SidebarPinChange::Pin {
            session_id: chat_id.to_owned(),
            after,
            before,
        })
    }

    pub fn unpin_session(&self, chat_id: &str) -> Result<()> {
        self.pin_change(SidebarPinChange::Unpin {
            session_id: chat_id.to_owned(),
        })
    }

    /// Reorder a pin between neighbours (either may be `None` at the ends).
    pub fn move_pin(
        &self,
        chat_id: &str,
        after: Option<String>,
        before: Option<String>,
    ) -> Result<()> {
        self.pin_change(SidebarPinChange::Move {
            session_id: chat_id.to_owned(),
            after,
            before,
        })
    }

    fn section_change(&self, change: SidebarSectionChange) -> Result<()> {
        self.pin_change(SidebarPinChange::Section { change })
    }

    pub fn create_section(&self, name: &str) -> Result<String> {
        let id = crate::new_id();
        self.section_change(SidebarSectionChange::Create {
            id: id.clone(),
            name: name.trim().to_owned(),
        })?;
        Ok(id)
    }

    pub fn rename_section(&self, section_id: &str, name: &str) -> Result<()> {
        self.section_change(SidebarSectionChange::Rename {
            id: section_id.to_owned(),
            name: name.trim().to_owned(),
        })
    }

    /// Delete a section (its sessions return to the recent list).
    pub fn delete_section(&self, section_id: &str) -> Result<()> {
        self.section_change(SidebarSectionChange::Delete {
            id: section_id.to_owned(),
        })
    }

    pub fn set_section_collapsed(&self, section_id: &str, collapsed: bool) -> Result<()> {
        self.section_change(SidebarSectionChange::Collapse {
            id: section_id.to_owned(),
            collapsed,
        })
    }

    /// Move a session into a section (`None` = back to the recent list).
    pub fn assign_section(&self, chat_id: &str, section_id: Option<String>) -> Result<()> {
        self.place_session(chat_id, section_id, None, None)
    }

    /// Move a session into a section between its members (either neighbour
    /// may be `None` at the ends; both `None` appends).
    pub fn place_session(
        &self,
        chat_id: &str,
        section_id: Option<String>,
        after: Option<String>,
        before: Option<String>,
    ) -> Result<()> {
        self.section_change(SidebarSectionChange::Assign {
            session_id: chat_id.to_owned(),
            section_id,
            after,
            before,
        })
    }

    /// Create a project on `device_id` (deduped on device + path). With an
    /// engine, asks the owning host (`Mutate {createSpace}`) and falls back
    /// to a local row write when it is unreachable.
    pub async fn create_project(
        &self,
        device_id: &str,
        path: &str,
        git_detected: bool,
    ) -> Result<String> {
        let (state, _) = self.inner.workspace.state();
        if let Some(existing) = state
            .spaces
            .iter()
            .find(|s| s.device_id == device_id && s.path == path)
        {
            return Ok(existing.id.clone());
        }
        let space = vardoger_proto::Space {
            id: crate::new_id(),
            device_id: device_id.to_owned(),
            path: path.to_owned(),
            name: None,
            git_detected,
            git_checked_at: None,
            checkout_id: None,
            repository_id: None,
            github_repo: None,
            icon: None,
            created_at: Utc::now(),
        };
        let id = space.id.clone();
        // The owning host writes the row itself (it knows git state and runs
        // the project's setup); an unreachable host gets a local row write
        // it will adopt later.
        if !self.inner.is_demo() {
            let asked = self
                .inner
                .host_rpc(
                    device_id,
                    vardoger_rpc::methods::MUTATE,
                    serde_json::json!({
                        "op": "createSpace",
                        "spaceId": id,
                        "deviceId": device_id,
                        "path": path,
                        "gitDetected": git_detected,
                    }),
                )
                .await;
            match asked {
                Ok(_) => {
                    // Wait (briefly) for the host's row so the caller can
                    // start a session in it right away.
                    for _ in 0..50 {
                        if self
                            .inner
                            .workspace
                            .state()
                            .0
                            .spaces
                            .iter()
                            .any(|s| s.id == id)
                        {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    return Ok(id);
                }
                Err(err) => {
                    tracing::info!(error = %err, "createSpace via host failed; writing locally")
                }
            }
        }
        self.inner.registry_write(|doc| doc.upsert_space(&space))?;
        Ok(id)
    }

    pub fn rename_project(&self, space_id: &str, name: Option<&str>) -> Result<()> {
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        if self
            .inner
            .registry_write(|doc| doc.rename_space(space_id, name))?
        {
            Ok(())
        } else {
            Err(ClientError::NotFound(space_id.to_owned()))
        }
    }

    /// Delete a project and cascade its chats' rows.
    pub fn delete_project(&self, space_id: &str) -> Result<()> {
        let deleted = self.inner.registry_write(|doc| doc.delete_space(space_id))?;
        let mut sessions = lock(&self.inner.sessions);
        for chat_id in deleted.chat_ids {
            sessions.remove(&chat_id);
        }
        Ok(())
    }

    // ── sessions ───────────────────────────────────────────────────────────

    /// Open (or return the already-open) session. Instant: the local
    /// snapshot renders first; sync follows in the background.
    pub fn open_session(&self, chat_id: &str) -> Result<SessionHandle> {
        if let Some(core) = self.inner.session_core(chat_id) {
            return Ok(SessionHandle { core });
        }
        if self.inner.workspace.chat(chat_id).is_none() {
            return Err(ClientError::NotFound(chat_id.to_owned()));
        }
        let (doc, hydrated) = match self.inner.backend() {
            Backend::Demo(demo) => (demo.session_doc(chat_id)?, true),
            Backend::Local(local) => (local.session_doc(chat_id)?, false),
        };
        let core = SessionCore::new(chat_id, &self.inner, doc);
        let core = {
            let mut sessions = lock(&self.inner.sessions);
            sessions
                .entry(chat_id.to_owned())
                .or_insert_with(|| core.clone())
                .clone()
        };
        if hydrated {
            core.set_hydrated();
        }
        core.touch();
        core.refresh();
        if let Some(demo) = self.inner.demo() {
            demo.session_opened(&core);
        }
        if let Some(local) = self.inner.local() {
            local.session_opened(&core);
        }
        self.inner.evict_sessions();
        Ok(SessionHandle { core })
    }

    /// An already-open session, if any.
    pub fn session(&self, chat_id: &str) -> Option<SessionHandle> {
        self.inner
            .session_core(chat_id)
            .map(|core| SessionHandle { core })
    }

    /// The view closed. The session stays warm (streaming continues) until
    /// the eviction policy reclaims it.
    pub fn close_session(&self, chat_id: &str) {
        if let Some(handle) = self.session(chat_id) {
            handle.set_view_attached(false);
        }
        self.inner.evict_sessions();
    }

    // ── host RPC ───────────────────────────────────────────────────────────

    /// Harness catalog of an execution device: live (cached to disk), else
    /// the last cached list, else the static fallback. Cached in memory for
    /// capability gating (mid-turn steering).
    pub async fn list_harnesses(&self, device_id: &str) -> Vec<HarnessInfo> {
        let list = match self.inner.backend() {
            Backend::Demo(demo) => {
                crate::providers::with_demo_toggles(true, demo.list_harnesses(device_id).await)
            }
            Backend::Local(_) => {
                let cache = catalog::DiskCatalog::new(&self.inner.config.data_dir);
                let reply = self
                    .inner
                    .host_rpc(
                        device_id,
                        vardoger_rpc::methods::LIST_HARNESSES,
                        serde_json::json!({}),
                    )
                    .await
                    .and_then(|v| {
                        serde_json::from_value::<Vec<HarnessInfo>>(v)
                            .map_err(|e| ClientError::HostError(e.to_string()))
                    });
                match reply {
                    Ok(list) => {
                        let list: Vec<HarnessInfo> =
                            list.into_iter().filter(|h| h.id != "mock").collect();
                        cache.put_harnesses(device_id, &list);
                        list
                    }
                    Err(err) => {
                        tracing::debug!(device = %device_id, error = %err, "ListHarnesses failed; using cache");
                        cache
                            .harnesses(device_id)
                            .unwrap_or_else(catalog::fallback_harnesses)
                    }
                }
            }
        };
        lock(&self.inner.harness_catalogs).insert(device_id.to_owned(), list.clone());
        self.inner.recompute_workspace();
        for core in self.inner.cores() {
            core.recompute_composer(&self.inner);
        }
        list
    }

    /// Model catalog for `harness` on `device_id` (normalized live reply,
    /// else the cached one, else the curated static list).
    pub async fn list_models(&self, device_id: &str, harness: &str) -> Vec<ModelInfo> {
        match self.inner.backend() {
            Backend::Demo(demo) => demo.list_models(harness).await,
            Backend::Local(_) => {
                let cache = catalog::DiskCatalog::new(&self.inner.config.data_dir);
                let reply = self
                    .inner
                    .host_rpc(
                        device_id,
                        vardoger_rpc::methods::LIST_MODELS,
                        serde_json::json!({ "harness": harness }),
                    )
                    .await
                    .and_then(|v| {
                        serde_json::from_value::<Vec<ModelInfo>>(v)
                            .map_err(|e| ClientError::HostError(e.to_string()))
                    });
                match reply {
                    Ok(list) if !list.is_empty() => {
                        let list = catalog::normalize_models(harness, list);
                        cache.put_models(device_id, harness, &list);
                        // Rows and chips pick up newly learned model labels.
                        self.inner.recompute_workspace();
                        list
                    }
                    _ => match cache.models(device_id, harness) {
                        Some(list) => {
                            catalog::learn_labels(harness, &list);
                            list
                        }
                        None => catalog::fallback_models(harness),
                    },
                }
            }
        }
    }

    pub async fn list_refs(&self, device_id: &str, repo_path: &str) -> Result<Vec<RepoRef>> {
        match self.inner.backend() {
            Backend::Demo(demo) => demo.list_refs(repo_path).await,
            Backend::Local(_) => {
                let value = self
                    .inner
                    .host_rpc(
                        device_id,
                        vardoger_rpc::methods::LIST_REFS,
                        serde_json::json!({ "repoPath": repo_path }),
                    )
                    .await?;
                serde_json::from_value(value).map_err(|e| ClientError::HostError(e.to_string()))
            }
        }
    }

    /// Workspace file search for composer `@` mentions (host `SearchFiles`,
    /// gitignore-aware; an empty query returns the chat's featured files).
    pub async fn search_files(
        &self,
        device_id: &str,
        chat_id: Option<String>,
        space_id: Option<String>,
        query: &str,
    ) -> Result<Vec<vardoger_proto::FileSearchMatch>> {
        match self.inner.backend() {
            Backend::Demo(_) => {
                const FILES: &[&str] = &[
                    "crates/mobile/src/layout/mod.rs",
                    "crates/mobile/src/layout/markdown.rs",
                    "crates/mobile/src/layout/rows.rs",
                    "crates/text/src/layout.rs",
                    "crates/text/src/prepare.rs",
                    "crates/markdown/src/parser.rs",
                    "apps/ios/Vardoger/Transcript/TranscriptListView.swift",
                    "apps/ios/Vardoger/Composer/ComposerBar.swift",
                    "docs/mobile-rewrite.md",
                    "README.md",
                ];
                let q = query.to_lowercase();
                Ok(FILES
                    .iter()
                    .filter(|p| q.is_empty() || p.to_lowercase().contains(&q))
                    .map(|p| vardoger_proto::FileSearchMatch {
                        path: (*p).to_owned(),
                        is_dir: false,
                    })
                    .collect())
            }
            Backend::Local(_) => {
                let value = self
                    .inner
                    .host_rpc(
                        device_id,
                        vardoger_rpc::methods::SEARCH_FILES,
                        serde_json::json!({ "query": query, "chatId": chat_id, "spaceId": space_id }),
                    )
                    .await?;
                serde_json::from_value(value).map_err(|e| ClientError::HostError(e.to_string()))
            }
        }
    }

    /// Browse folders on a device (`None` = its home folder).
    pub async fn list_folders(
        &self,
        device_id: &str,
        path: Option<String>,
    ) -> Result<FolderListing> {
        match self.inner.backend() {
            Backend::Demo(demo) => demo.list_folders(device_id, path).await,
            Backend::Local(_) => {
                let params = match path.filter(|p| !p.is_empty()) {
                    Some(path) => serde_json::json!({ "path": path }),
                    None => serde_json::json!({}),
                };
                let value = self
                    .inner
                    .host_rpc(device_id, vardoger_rpc::methods::LIST_FOLDERS, params)
                    .await?;
                serde_json::from_value(value).map_err(|e| ClientError::HostError(e.to_string()))
            }
        }
    }

    /// `git checkout <ref>` in `repo_path` on the device.
    pub async fn switch_ref(&self, device_id: &str, repo_path: &str, ref_name: &str) -> Result<()> {
        match self.inner.backend() {
            Backend::Demo(demo) => demo.switch_ref(repo_path, ref_name).await,
            Backend::Local(_) => self
                .inner
                .host_rpc(
                    device_id,
                    vardoger_rpc::methods::SWITCH_REF,
                    serde_json::json!({ "repoPath": repo_path, "refName": ref_name }),
                )
                .await
                .map(|_| ()),
        }
    }

    /// Create a worktree for `branch` (an existing ref); returns its path.
    pub async fn create_worktree(
        &self,
        device_id: &str,
        space_id: &str,
        repo_path: &str,
        branch: &str,
    ) -> Result<String> {
        match self.inner.backend() {
            Backend::Demo(demo) => demo.create_worktree(repo_path, branch).await,
            Backend::Local(_) => {
                let mut params = serde_json::json!({ "repoPath": repo_path, "branch": branch });
                if !space_id.is_empty() {
                    params["spaceId"] = serde_json::Value::String(space_id.to_owned());
                }
                let value = self
                    .inner
                    .host_rpc(device_id, vardoger_rpc::methods::CREATE_WORKTREE, params)
                    .await?;
                value
                    .get("path")
                    .and_then(|p| p.as_str())
                    .map(str::to_owned)
                    .ok_or_else(|| ClientError::HostError("CreateWorktree returned no path".into()))
            }
        }
    }

    /// Chunked upload of one file to the host; returns its durable path there.
    pub async fn upload_attachment(
        &self,
        device_id: &str,
        name: &str,
        data: Vec<u8>,
        progress: Option<ProgressFn>,
    ) -> Result<String> {
        if data.len() > attachments::MAX_ATTACHMENT_BYTES {
            return Err(ClientError::InvalidArgument(format!(
                "{name} is larger than 24 MB"
            )));
        }
        match self.inner.backend() {
            Backend::Demo(demo) => {
                demo.upload(&self.inner, device_id, name, data, progress)
                    .await
            }
            Backend::Local(local) => {
                let upload_id = crate::new_id();
                let file_name = attachments::upload_file_name(name);
                let path = local.upload(&upload_id, &file_name, &data, progress).await?;
                self.inner
                    .attachment_cache
                    .put(device_id, &path, Arc::new(data));
                Ok(path)
            }
        }
    }

    /// Attachment bytes (LRU-cached). Accepts host paths and this device's
    /// own `pending://` refs.
    pub async fn read_attachment(&self, device_id: &str, path: &str) -> Result<Arc<Vec<u8>>> {
        if let Some(hit) = self.inner.attachment_cache.get(device_id, path) {
            return Ok(hit);
        }
        let bytes = match self.inner.backend() {
            Backend::Demo(demo) => demo.read_attachment(path)?,
            Backend::Local(local) => local.read_attachment(path).await?,
        };
        self.inner
            .attachment_cache
            .put(device_id, path, bytes.clone());
        Ok(bytes)
    }

    // ── lifecycle ──────────────────────────────────────────────────────────

    /// OS network path (NWPathMonitor): `false` only for a definitive
    /// "unsatisfied" — ambiguity stays online.
    pub fn set_network_online(&self, online: bool) {
        let was = self.inner.path_online.swap(online, Ordering::AcqRel);
        if !self.inner.is_demo() {
            // Parks/un-parks every sync backoff in the process (vardoger-sync).
            vardoger_sync::wake::set_path_online(online);
        }
        if was != online {
            self.inner.recompute_connectivity();
            if online {
                self.network_path_changed();
            }
        }
    }

    /// The OS network path changed (another interface, a new address):
    /// terminals waiting to reconnect try now.
    pub fn network_path_changed(&self) {
        self.inner.terminals_wake.notify_waiters();
        if let Some(direct) = lock(&self.inner.direct).as_ref() {
            direct.network_changed();
        }
    }

    /// App returned to the foreground: resume ticking.
    pub fn on_foreground(&self) {
        self.inner.foreground.store(true, Ordering::Release);
        if !self.inner.is_demo() {
            vardoger_sync::wake::set_resting(false);
        }
        self.inner.terminals_wake.notify_waiters();
        if let Some(direct) = lock(&self.inner.direct).as_ref() {
            // ponytail: time away isn't tracked, so connections are kept (a dead one
            // ends by silence); pass the real time once on_foreground takes it.
            direct.foreground(Duration::ZERO);
        }
        self.inner.tick();
    }

    /// App is backgrounding: pause time-driven work.
    pub fn on_background(&self) {
        self.inner.foreground.store(false, Ordering::Release);
    }

    /// Still in the background on a platform that doesn't suspend the
    /// process: sync backoffs rest until [`Self::on_foreground`].
    pub fn on_background_idle(&self) {
        if self.inner.is_demo() || self.inner.foreground.load(Ordering::Acquire) {
            return;
        }
        vardoger_sync::wake::set_resting(true);
    }

    /// Warm the most relevant sessions (front page order: pinned, sections,
    /// recent), up to [`PRELOAD_CAP`].
    pub fn preload_sessions(&self) {
        let workspace = self.workspace();
        let front = &workspace.front;
        let candidates = front
            .pinned
            .iter()
            .chain(front.sections.iter().flat_map(|s| s.sessions.iter()))
            .chain(front.recent.iter());
        for row in candidates.take(PRELOAD_CAP) {
            if self.inner.session_core(&row.id).is_none() {
                let _ = self.open_session(&row.id);
            }
        }
    }

    /// Ids of the sessions currently open (warm), most recently used first.
    pub fn open_session_ids(&self) -> Vec<String> {
        let mut cores = self.inner.cores();
        cores.sort_by_key(|c| std::cmp::Reverse(c.touched_ms()));
        cores.iter().map(|c| c.chat_id.clone()).collect()
    }
}

/// Seed helpers shared with the demo host.
pub(crate) fn ms(at: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(at)
        .single()
        .unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_replica_is_local_only_and_never_queues() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = ClientConfig::new(dir.path());
        config.device_id = "ios-test".into();
        let client = Client::new(
            config,
            Credentials::Demo(Default::default()),
            Arc::new(crate::events::NullListener),
        )
        .unwrap();
        let chat_id = client
            .inner
            .workspace
            .mutate(|doc| doc.read_chats())
            .unwrap()
            .into_iter()
            .find(|c| c.parent_chat_id.is_none())
            .expect("seeded chat")
            .id;
        for i in 0..20 {
            client
                .rename_session(&chat_id, &format!("title {i}"))
                .unwrap();
        }
        let (pending, title) = client.inner.workspace.mutate(|doc| {
            (
                doc.pending_len(),
                doc.chat(&chat_id).unwrap().unwrap().title,
            )
        });
        assert_eq!(pending, 0);
        assert_eq!(title.as_deref(), Some("title 19"));
    }
}
