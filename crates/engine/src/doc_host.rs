//! DocHost — per-chat `SessionDoc` handles: snapshot persistence (debounced), edge room
//! sync (offline-tolerant), and the HOST-ONLY durable command executor.
//!
//! Pragmatic port of commutator's `session-docs.ts` + the `main.ts` executor (spec:
//! feature-inventory §3.3, ARCHITECTURE §2 "command plane"):
//! - the doc IS the outbox: commands and user entries commit locally and sync whenever a
//!   room connection exists; the engine is fully functional with sync disabled;
//! - on every doc change (local commit or remote import) the handle re-emits the joined
//!   transcript to watchers, drains pending commands, and schedules a snapshot save;
//! - command drain: evaluate via `evaluate_command` (with the DocsStore processed
//!   ledger), mark processed BEFORE execute, execute through the sessions engine, then
//!   write the outcome status back into the doc as the sole outcome writer.
//!
//! Chat ownership is gated on the workspace doc (`chats[chat_id].deviceId`), with
//! claim-on-first-command for unknown chats. Queueing a command for a chat hosted on
//! another device POSTs a durable nudge to that device's room (§7 cold-chat delivery);
//! the host's relay receives it and warm-opens the doc, which drains the queue.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

use base64::Engine as _;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use commutator_doc::{
    CommandBasedOn, CommandDisposition, DocError, EvaluationContext,
    MessagePart, MessageRole, MessageStatus, QueueDeliveryGate, QueuedMessage, SessionCommandEntry,
    SessionCommandPayload, SessionCommandStatus, SessionDoc, SessionMessageEntry, SubagentStatus,
    evaluate_command,
};
use commutator_proto::{ConversationSourceContext, HarnessId, UserInputAnswer, UserInputQuestion};
use commutator_sync::DocsStore;

use crate::project_actions::{
    ProjectActionSetupHandoff, ProjectActionsStore, launch_project_setup_action,
};
use crate::sessions::{SessionsEngine, SteerOutcome};
use crate::workspace_host::WorkspaceHost;
use crate::{EngineError, Terminals, new_id, now_ms};

/// Debounce window for local snapshot saves after a doc change.
const SNAPSHOT_DEBOUNCE_MS: u64 = 1_000;

/// An edit client renews every 20s. Sixty seconds tolerates two missed
/// heartbeats without turning a vanished client into an invisible permanent
/// lock. Expiry fails closed into ReviewRequired rather than releasing.
pub const QUEUE_EDIT_LEASE_MS: i64 = 60_000;

/// A Cloud machine woken on the person's say-so takes a while to boot and read
/// its chat: a waiting message whose clock runs out within this long goes
/// again with a fresh one rather than expire on the way.
const WAKE_BOOT_MARGIN_MS: i64 = 60 * 60 * 1000;

/// Warm-doc LRU: how many unwatched, run-less docs stay fully open. Everything
/// beyond this (and beyond [`commutator_doc::DOC_LRU_BYTE_BUDGET`]) is evicted
/// oldest-access-first. A real chat's doc is large (a long one 15-80 MB, 3-24M
/// ops), and reopening one from its SQLite snapshot reads its whole state:
/// 45 ms for 15 MB, 280 ms for 80 MB (M5 Max, 2026-11-17), past the 400 ms
/// that shows a skeleton on slower Macs. Three kept (0.3.8) made switching
/// among a few chats freeze and show skeletons; the interface shows a chat
/// from its own rows only while it keeps them. Each kept doc's memory is the
/// price until reopening is cheap of its own accord.
const WARM_DOC_CAP: usize = 12;

/// A stored transcript this large (its snapshot's bytes) takes a while to
/// read whole: ~3 ms a MB (an 80 MB chat's 250 ms, M5 Max). A viewer holding
/// its rows is sent what changed among its newest entries first
/// ([`ChatDocHandle::transcript_is_long`]).
const LONG_TRANSCRIPT_BYTES: usize = 1 << 20;

/// Resident-memory estimate per compressed snapshot byte. Loro snapshots are
/// columnar+compressed; the in-memory doc plus mirror runs well above the blob
/// size. A rough multiplier is enough here — the budget is a safety ceiling,
/// the count cap does the day-to-day work.
const RESIDENT_BYTES_PER_SNAPSHOT_BYTE: usize = 6;

/// Floor per open doc (room socket buffers, tasks) regardless of content size.
const DOC_RESIDENT_FLOOR_BYTES: usize = 512 * 1024;

const ACTIVE_SYNC_CAP: usize = 28;
/// The least time between two sync dispatcher passes, and the retry delay
/// while a pass leaves work in flight.
const SYNC_PASS_SPACING: std::time::Duration = std::time::Duration::from_millis(100);

// All live-transport scenarios share the process budget, including tests in
// sibling modules. Hold through shutdown so one fixture cannot rotate another.
#[cfg(test)]
static SYNC_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Queued-attachment transfer pacing: chunk pushes are bounded per call (a
/// stalled-but-open relay link never fails on its own) and a timeout marks
/// the link suspect; attempts retry on this backoff, cut short by the online
/// bus / system wake.
const TRANSFER_CHUNK_B64: usize = 60_000;
const TRANSFER_CHUNK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const TRANSFER_COMMIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const TRANSFER_BACKOFF_BASE: std::time::Duration = std::time::Duration::from_secs(2);
const TRANSFER_BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(30);
/// A command whose attachment bytes are still in transit waits at most this
/// long before the drain rejects it loudly (and the transfer task gives up on
/// the same clock) — a chat must never wedge behind bytes that aren't coming.
/// Bytes safely in the edge's store ARE coming: a Cloud host checks there
/// before this cap applies (doc_host_attachments.rs), however long it slept.
const ATTACHMENT_WAIT_MAX: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const ATTACHMENT_WAIT_MAX_MS: i64 = ATTACHMENT_WAIT_MAX.as_millis() as i64;
/// Re-check cadence while a chat's queue is deferred on in-transit bytes
/// (the happy path is event-driven — UploadCommit kicks the drain — this
/// timer only covers the give-up transition and missed kicks).
const ATTACHMENT_WAIT_RECHECK: std::time::Duration = std::time::Duration::from_secs(30);

/// Transfer attempt outcome: transient failures retry (the link may heal),
/// permanent ones stop (the host actively refused, or the bytes are gone).
enum TransferError {
    Transient(String),
    Permanent(String),
}

/// Peer-relay delivery fallback pacing (`spawn_command_delivery`): the grace
/// the normal rows→edge path gets before the relay road opens, the poll while
/// waiting, the relay retry curve, its per-call deadline, and the give-up cap
/// (the command stays durably queued in the doc regardless).
const ROWS_GRACE: std::time::Duration = std::time::Duration::from_secs(10);
const ROWS_POLL: std::time::Duration = std::time::Duration::from_secs(1);
const RELAY_BACKOFF_BASE: std::time::Duration = std::time::Duration::from_secs(5);
const RELAY_BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(30);
const RELAY_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const RELAY_GIVE_UP: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Hosts below this stamped engine version don't serve `RelayCommand`.
const RELAY_MIN_VERSION: (u64, u64, u64) = (0, 2, 12);


#[derive(Debug, Clone)]
pub struct DocHostConfig {
    pub device_id: String,
    /// Harness for doc-command runs on chats without a workspace `config` row.
    pub default_harness: HarnessId,
}

struct DocHostInner {
    /// Opens direct links to other devices. Set only with the relay layer on.
    store: Arc<DocsStore>,
    config: DocHostConfig,
    /// Set-once (first wins), cleared by `shutdown_workers`: sessions and
    /// doc-host reference each other through Arcs, so a retired runtime's
    /// graph only drops once this back-edge is severed.
    sessions: Mutex<Option<SessionsEngine>>,
    /// Work is held ([`DocHost::hold_work`], while an update is tried out):
    /// drains leave new work pending until [`DocHost::lift_quiesce`].
    quiesced: AtomicBool,
    boot_recovery_finished: AtomicBool,
    /// Drains between their quiesce check and the end of their dispatch (a
    /// run they start is registered — and Working — by then).
    dispatching: Arc<AtomicUsize>,
    workspace: OnceLock<WorkspaceHost>,
    /// Worktree materialization for Run commands (see `set_repos`).
    repos: OnceLock<crate::repos::Repos>,
    project_action_runtime: OnceLock<(ProjectActionsStore, Terminals)>,
    /// Cancels every worker spawned through `spawn_worker` — the loops'
    /// own exit conditions (weak handle death, closed channels) don't cover
    /// runtime replacement, where Edge-capable tasks must stop doing
    /// network work even while something still pins the graph.
    shutdown: CancellationToken,
    edge_disconnected: AtomicBool,
    /// Wakes the sync dispatcher ([`DocHost::spawn_sync_scheduler`]) when
    /// something it serves changed outside the store: a sync request, a
    /// failed publication, a new handle, the workspace being wired. Store
    /// changes arrive through [`DocsStore::watch_sync_jobs`]/`watch_outbox`.
    /// Shared rather than owned, so the waiting loops hold no strong ref to
    /// this graph; dropping it notifies both so they see it gone and end.
    sync_kick: Arc<tokio::sync::Notify>,
    /// Passes the sync dispatcher has run (idle tests count them).
    #[cfg(test)]
    scheduler_passes: AtomicUsize,
    /// Tracks every spawned worker so `shutdown_workers` can await them.
    tasks: TaskTracker,
    handles: Mutex<HashMap<String, Arc<ChatDocHandle>>>,
    document_loads: AtomicU64,
    focus_clock: AtomicU64,
    /// Serialize cold opens without blocking access to already-live handles.
    opening: Mutex<()>,
    /// chat2 seeds in flight (one per chat — reopen storms must not race
    /// duplicate rebuild+checkpoint POSTs; benign server-side, wasteful).
    seeding: Mutex<HashSet<String>>,
    /// chat2 quiet-waiters armed (one per chat): the cutover watcher re-arms
    /// on every registry change, and a long run would stack a tick loop per
    /// change without this.
    seed_waiting: Mutex<HashSet<String>>,
    /// Attachment-wait re-drain timers armed (one per chat): a command
    /// deferred on in-transit attachment bytes re-checks on a cadence, and
    /// each deferral must not stack another timer.
    drain_waiting: Mutex<HashSet<String>>,
    /// Uploads store (engine assembly) — resolves `pending://` attachment
    /// refs and jails transfer reads to the uploads dir.
    uploads: OnceLock<crate::uploads::Uploads>,
    /// Connectivity watch (`WatchConnectivity`): lazily-started monitor
    /// publishes the edge posture on change (see `watch_connectivity`).
    connectivity: OnceLock<watch::Sender<commutator_proto::Connectivity>>,
    /// Told as the workspace is wired (`set_workspace`): a connectivity
    /// monitor started before then learns whether there is an edge to watch.
    workspace_wired: tokio::sync::Notify,
    /// In-flight queued-attachment transfers, published per landed chunk
    /// (see `watch_transfers`). Entries live exactly as long as bytes are
    /// moving: added when a file's push starts, removed on commit or failure
    /// (a retry re-adds), so consumers can render a real percent while the
    /// relay leg runs and fall back to indeterminate otherwise.
    transfers: watch::Sender<Vec<commutator_proto::TransferProgress>>,
    /// Command ids currently BETWEEN mark-processed and their resolution in a
    /// drain. Distinguishes "executing right now" from "consumed by the
    /// ledger but dead" (a crash between mark and resolve): the drain
    /// terminalizes the latter as Rejected instead of leaving a forever-
    /// Pending entry no retry could ever reach (2026-08-19 swallowed-send).
    executing: Mutex<HashSet<String>>,
    /// Peer links (engine assembly, edge runtimes only) — the transport that
    /// pushes queued attachment bytes to a remote host.
    links: OnceLock<Arc<commutator_rpc::LinkCache>>,
    /// Chats with a worker draining their held sends (doc_host_attachments.rs).
    held_active: Mutex<HashSet<String>>,
    /// Releasing a held send and removing one exclude each other: a removed
    /// send never goes out, and a send that went out reads as not removed.
    held_gate: Mutex<()>,
    /// Cuts a held send's backoff short on the user's retry.
    held_kick: tokio::sync::Notify,
    /// The outbox as the local interface shows it.
    held_status: watch::Sender<Vec<commutator_proto::HeldSendStatus>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The queued-attachment transfers a command's `pending://` refs imply —
/// shared by the retry escort and the retry re-issue.
fn command_transfers(entry: &SessionCommandEntry) -> Vec<crate::uploads::AttachmentTransfer> {
    let refs: Vec<String> = match &entry.payload {
        SessionCommandPayload::Run { request, .. } => request
            .attachments
            .iter()
            .filter(|p| crate::uploads::is_pending_ref(p))
            .cloned()
            .collect(),
        SessionCommandPayload::Steer { prompt, .. } => crate::uploads::pending_refs_in(prompt),
        _ => Vec::new(),
    };
    refs.iter()
        .filter_map(|r| crate::uploads::parse_pending_ref(r))
        .map(
            |(upload_id, file_name)| crate::uploads::AttachmentTransfer {
                upload_id: upload_id.to_string(),
                file_name: file_name.to_string(),
            },
        )
        .collect()
}

/// Retires a transfer's progress entry on drop — the one exit point for
/// `push_attachments`' many returns (commit landed, chunk timeout, host
/// refusal), so no failure path can leave a phantom ring behind.
struct TransferProgressGuard<'a> {
    host: &'a DocHost,
    upload_id: &'a str,
}

impl Drop for TransferProgressGuard<'_> {
    fn drop(&mut self) {
        self.host.transfer_progress_clear(self.upload_id);
    }
}

#[derive(Clone)]
pub struct DocHost {
    inner: Arc<DocHostInner>,
}

/// A [`DocHost`] that doesn't keep it alive: for what outlives the engine
/// graph (a process-wide hook), so a shut-down graph can still be freed.
#[derive(Clone)]
pub(crate) struct WeakDocHost(Weak<DocHostInner>);

impl WeakDocHost {
    pub(crate) fn upgrade(&self) -> Option<DocHost> {
        self.0.upgrade().map(|inner| DocHost { inner })
    }
}

/// How a taken queue row reaches the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueSend {
    /// Nothing is running: start a turn with it.
    NextTurn,
    /// Something is running and can take input mid-turn: steer it in.
    Steer,
    /// The user said now: stop what is running first.
    Interrupt,
}

const ATTACHMENT_ONLY_PROMPT: &str = "See the attached image(s).";
const ATTACHMENT_PROMPT_HEADER: &str = "Attached images (local files — open them to view):";

/// Queue rows keep the user's editable text separate from attachment paths.
/// Rebuild the transcript/harness transport only when the row is dispatched.
///
/// Older clients stored the already-expanded prompt in `text`; recognize the
/// exact trailer implied by `attachments` so those rows are not expanded a
/// second time after an upgrade.
fn queued_message_prompt(text: &str, attachments: &[String]) -> String {
    if attachments.is_empty() {
        return text.to_string();
    }
    let refs = attachments
        .iter()
        .map(|path| format!("- {path}"))
        .collect::<Vec<_>>()
        .join("\n");
    let trailer = format!("\n\n{ATTACHMENT_PROMPT_HEADER}\n{refs}");
    let body = text.strip_suffix(&trailer).unwrap_or(text);
    let body = if body.trim().is_empty() {
        ATTACHMENT_ONLY_PROMPT
    } else {
        body
    };
    format!("{body}{trailer}")
}

/// What a queue row says: its words, or for files with no words, what a run
/// of them says. A row needs text on every device (one without is refused and
/// dropped as read), so files alone must not come to an empty one.
fn queued_row_text(text: &str, has_files: bool) -> &str {
    if has_files && text.trim().is_empty() {
        ATTACHMENT_ONLY_PROMPT
    } else {
        text
    }
}

fn queue_text_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BeginQueueEditOutcome {
    Acquired {
        lease_id: String,
        text: String,
        attachments: Vec<String>,
        base_text_hash: String,
        expires_at_ms: i64,
    },
    Locked {
        owner_device_id: String,
        expires_at_ms: i64,
    },
    Missing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RenewQueueEditOutcome {
    Renewed { expires_at_ms: i64 },
    Lost,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishQueueEditAction {
    Commit,
    Cancel,
    Discard,
    ReleaseUnchanged,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FinishQueueEditOutcome {
    Committed,
    Cancelled,
    Discarded,
    Released,
    Conflict { current_text: String },
    Lost,
    Missing,
}

/// Content and its historical presentation cutoff travel atomically, even
/// when the watch coalesces several backfill and live commits.
#[derive(Clone, Default)]
pub struct TranscriptSnapshot {
    /// Shared with the doc's entry cache: a commit copies none of them.
    pub entries: Arc<Vec<Arc<SessionMessageEntry>>>,
    pub replay_baseline: Arc<commutator_doc::TranscriptBaseline>,
}

/// One open chat doc: the `SessionDoc`, its change plumbing, and the room client.
pub struct ChatDocHandle {
    chat_id: String,
    device_id: String,
    doc: Arc<SessionDoc>,
    /// The host's [`DocHostInner::sync_kick`]: a connected chat losing its
    /// last viewer or writer may now close, so the dispatcher looks again.
    sync_kick: Arc<tokio::sync::Notify>,
    /// A task waits for this chat's viewers to leave ([`Self::note_viewer`]).
    unwatch_waiter: Arc<AtomicBool>,
    /// Cancelled as the handle drops: that waiter holds sender clones, and
    /// must let them go then, so its viewers' streams end (and resubscribe)
    /// exactly as if it weren't there.
    unwatch_cancel: CancellationToken,
    /// The runtime this handle was opened on, for that waiter: a view may
    /// attach from a plain thread.
    runtime: Option<tokio::runtime::Handle>,
    messages_tx: watch::Sender<TranscriptSnapshot>,
    /// Serialize historical imports with publication so an async doc-change
    /// task cannot publish recovered content before its presentation cutoff.
    transcript_import: Mutex<()>,
    transcript_history: Arc<Mutex<crate::transcript_history::TranscriptHistory>>,
    /// Pending-message queue watch (WatchQueue). Cheap to rebuild — a handful
    /// of short rows — so unlike the transcript mirror it publishes on every
    /// change without a dirty flag.
    queue_tx: watch::Sender<Vec<QueuedMessage>>,
    /// Bumped on every change to the doc (`WatchWaitingMessages` follows it).
    changes: watch::Receiver<u64>,
    /// Serializes everything that TAKES from the queue. Both the doc-change
    /// task and the turn-end status watcher call `drain_queue`, and nothing
    /// keeps those two apart: without this they interleave across the
    /// `dispatch` await, each taking a different head and each sending, so a
    /// queue meant to release one message releases all of them.
    ///
    /// It also covers the gap a send-now's interrupt opens — between stopping
    /// the turn and starting its own the chat reads Idle, and an idle chat with
    /// a queue is exactly what the flush drains.
    drain_lock: tokio::sync::Mutex<()>,
    /// Serialize prompt commands while still allowing interrupt/input controls.
    command_drain_lock: tokio::sync::Mutex<()>,
    /// Queue rows held as explicit steers for a turn-boundary agent. They
    /// lead ordinary queued rows, in the order they were steered.
    steered_rows: Mutex<Vec<String>>,
    /// Sends held for a turn-boundary agent's turn end (rather than queued
    /// by the user), with the run configuration each was sent with: the row
    /// carries only its text and images.
    held_sends: Mutex<HashMap<String, commutator_proto::RunRequest>>,
    /// The latest Stop command that has acted. A prompt sent before it that
    /// only executes now (a loaded host, a slow agent) was cancelled by it:
    /// running it would start the stopped work again in a fresh runtime.
    stopped_by: Mutex<Option<String>>,
    /// An explicit user interrupt freezes automatic queue delivery. The next
    /// explicit prompt or queue send resumes it; incidental doc/status changes
    /// must not turn Cancel into "send the next row".
    queue_paused: AtomicBool,
    /// True when the doc changed while nobody watched: the mirror rebuild is
    /// deferred to the next `watch_messages` attach instead of paid per commit.
    mirror_dirty: AtomicBool,
    /// Epoch ms of the last open/watch touch — the LRU eviction key.
    last_access: AtomicI64,
    /// User navigation only. Opens, watches, writes and reconnects never bump it.
    last_focus: AtomicU64,
    /// Last known snapshot blob size — the eviction budget estimate's input.
    snapshot_bytes: AtomicUsize,
    /// The sync generation this handle was BUILT for (1 = legacy s2,
    /// 2 = chat2). Gen-1 handles no longer join any room (the s2 client is
    /// gone); they serve the local fat doc read-only until the host's seed
    /// flips the chat to chat2. The staleness checks compare this against
    /// the registry — inferring mode from `chat2_local_sub` misread
    /// edge-less chat2 handles (no subscription is ever installed offline)
    /// as stale s2 and retired them on every open, dropping the doc out
    /// from under live runs.
    room_gen: u32,
    /// Set when a chat2 seed replaced this handle's lineage on disk: every
    /// further snapshot save from this handle is a stale FAT doc that would
    /// clobber the thin one — retired handles never persist again (unless no
    /// thin lineage exists on disk at all; `save_snapshot` double-checks, so
    /// a doc that was never seeded can't lose its only copy).
    retired: AtomicBool,
    /// chat2 relay client (docs/chat2-sync.md C3) — populated once the
    /// registry names roomGen 2 for this chat and the join resolves.
    chat2: Mutex<Option<commutator_sync::ChatClient>>,
    sync_started: AtomicBool,
    sync_requested: AtomicBool,
    sync_background: AtomicBool,
    sync_wait_since: AtomicI64,
    sync_cancel: Mutex<CancellationToken>,
    writers: Arc<AtomicUsize>,
    pub(crate) persistence: Option<Arc<crate::chat_persistence::ChatPersistence>>,
    publication_failed: AtomicBool,
    /// Doc subscription (drop = unsubscribe) — bumps the change watch on every commit.
    _sub: loro::Subscription,
}

/// One drain counted in [`DocHostInner::dispatching`] until dropped.
struct DispatchGuard(Arc<AtomicUsize>);
impl Drop for DispatchGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct DocWriter {
    doc: Arc<SessionDoc>,
    writers: Arc<AtomicUsize>,
    sync_kick: Arc<tokio::sync::Notify>,
}
impl std::ops::Deref for DocWriter {
    type Target = SessionDoc;
    fn deref(&self) -> &Self::Target {
        &self.doc
    }
}
impl Drop for DocWriter {
    fn drop(&mut self) {
        if self.writers.fetch_sub(1, Ordering::AcqRel) == 1 {
            // The last writer: its connection may close now.
            self.sync_kick.notify_one();
        }
    }
}
impl Drop for DocHostInner {
    fn drop(&mut self) {
        // The dispatch loops wait on these alone when idle: let them see the
        // host gone and end, as their old polling did.
        self.sync_kick.notify_one();
    }
}

impl Drop for ChatDocHandle {
    fn drop(&mut self) {
        lock(&self.sync_cancel).cancel();
        self.unwatch_cancel.cancel();
    }
}

/// The provider the chat's own latest switch seam moved to: `None` when the
/// chat never switched, `Some(None)` for a seam naming no known harness.
/// Seams above a fork's seam were copied from the source chat and don't
/// count: the fork's own provider history starts there.
pub(crate) fn latest_switch(
    entries: &[impl std::borrow::Borrow<SessionMessageEntry>],
) -> Option<Option<HarnessId>> {
    for entry in entries.iter().rev().map(std::borrow::Borrow::borrow) {
        for part in &entry.parts {
            match part {
                MessagePart::Switch { to_harness, .. } => {
                    return Some(to_harness.as_deref().and_then(HarnessId::from_slug));
                }
                MessagePart::Fork { .. } => return None,
                _ => {}
            }
        }
    }
    None
}

impl ChatDocHandle {
    /// The outbox entries a join should upload. With the relay layer off, all.
    pub(crate) fn uploadable_updates(&self, pending: Vec<(String, Vec<u8>)>) -> Vec<(String, Vec<u8>)> {
        pending
    }

    /// This host is storing a chat: what it imports must go into rows as well.
    pub(crate) fn republishes_imports(&self) -> bool {
        false
    }

    pub(crate) fn republish(&self, bytes: Vec<u8>) {
        if let Some(client) = &*lock(&self.chat2) {
            client.enqueue_batch(uuid::Uuid::new_v4().to_string(), bytes);
        }
    }

    /// The turn ending now is followed at once by a message the user has
    /// already sent: one still on its way through the command log (a burst
    /// the agent outran), or one held for a turn-boundary agent's turn end.
    /// The work goes on, so the end is not a completion to notify.
    pub(crate) fn continues_with_sent_message(&self) -> bool {
        // Not yet in the transcript: delivery writes the user's message
        // before dispatching it, so the command that started the turn now
        // ending (still marked pending a moment longer) never counts.
        let in_flight = self.doc.read_commands().is_ok_and(|commands| {
            let pending: Vec<&str> = commands
                .iter()
                .filter(|c| c.status == SessionCommandStatus::Pending)
                .filter_map(|c| match &c.payload {
                    SessionCommandPayload::Run { message_id, .. } => Some(message_id.as_str()),
                    SessionCommandPayload::Steer { message_id, .. } => {
                        Some(message_id.as_deref().unwrap_or(c.id.as_str()))
                    }
                    _ => None,
                })
                .collect();
            !pending.is_empty()
                && self.doc.read_entries_shared().is_ok_and(|entries| {
                    pending
                        .iter()
                        .any(|id| !entries.iter().any(|e| e.id == *id))
                })
        });
        if in_flight {
            return true;
        }
        if self.queue_paused.load(Ordering::Acquire) {
            return false;
        }
        let Ok(Some(head)) = self.doc.read_queue().map(|q| q.into_iter().next()) else {
            return false;
        };
        head.delivery_gate.is_none()
            && (lock(&self.held_sends).contains_key(&head.id)
                || lock(&self.steered_rows).contains(&head.id))
    }

    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    pub fn doc(&self) -> &SessionDoc {
        &self.doc
    }

    pub fn writer(&self) -> DocWriter {
        self.writers.fetch_add(1, Ordering::AcqRel);
        DocWriter {
            doc: self.doc.clone(),
            writers: self.writers.clone(),
            sync_kick: self.sync_kick.clone(),
        }
    }

    pub fn doc_arc(&self) -> Arc<SessionDoc> {
        self.doc.clone()
    }

    /// Joined transcript watch — re-sent on every doc change (WatchDocMessages).
    ///
    /// Attach-time refresh: the mirror is only maintained while watched, so a
    /// doc that changed unwatched materializes here, once, instead of on every
    /// commit it sat through in the background.
    pub fn watch_messages(&self) -> watch::Receiver<TranscriptSnapshot> {
        self.touch();
        // Attach is a user signal: verify a quiet room is actually alive
        // (a doc-wedged DO keeps answering pings while delivering nothing,
        // and the background probe cadence can be hours out). Coalescing
        // no-op on a healthy or recently-active room.
        if let Some(chat2) = lock(&self.chat2).as_ref() {
            chat2.probe();
        }
        // Subscribe BEFORE the dirty check: a commit racing this attach then
        // sees a live receiver and publishes, instead of re-marking dirty
        // after our refresh and leaving the new watcher a cleared mirror.
        let _import = lock(&self.transcript_import);
        let rx = {
            if self.messages_tx.receiver_count() == 0 {
                // A new viewing session must not inherit the former viewer's
                // live-part protection, even if no commit happened while away.
                *lock(&self.transcript_history) = Default::default();
            }
            self.messages_tx.subscribe()
        };
        self.note_viewer();
        // Watched: its every commit reads the transcript, so reads keep it.
        self.doc.set_keep_read_cache(true);
        if self.mirror_dirty.load(Ordering::Acquire) {
            self.publish_messages_locked();
        }
        rx
    }

    /// Queue watch — the composer's held messages, re-sent on every doc change.
    pub fn watch_queue(&self) -> watch::Receiver<Vec<QueuedMessage>> {
        self.touch();
        let rx = self.queue_tx.subscribe();
        self.note_viewer();
        self.publish_queue();
        rx
    }

    /// A viewer protects this chat's connection; when the last one leaves,
    /// kick the dispatcher, which no longer polls for it. One waiter per
    /// chat at a time, however often views attach.
    fn note_viewer(&self) {
        let Some(runtime) = tokio::runtime::Handle::try_current()
            .ok()
            .or_else(|| self.runtime.clone())
        else {
            return;
        };
        if self.unwatch_waiter.swap(true, Ordering::AcqRel) {
            return;
        }
        let (messages, queue) = (self.messages_tx.clone(), self.queue_tx.clone());
        let (waiter, kick) = (self.unwatch_waiter.clone(), self.sync_kick.clone());
        let gone = self.unwatch_cancel.clone();
        runtime.spawn(async move {
            loop {
                // The handle dropping ends the waiter (and its sender
                // clones) at once: the streams must close with the handle.
                tokio::select! {
                    _ = gone.cancelled() => return,
                    _ = async { messages.closed().await; queue.closed().await } => {}
                }
                waiter.store(false, Ordering::Release);
                kick.notify_one();
                // A view that attached meanwhile saw the waiter still set and
                // started none: stay for it, unless one has since started.
                if (messages.receiver_count() > 0 || queue.receiver_count() > 0)
                    && !waiter.swap(true, Ordering::AcqRel)
                {
                    continue;
                }
                break;
            }
        });
    }

    /// Where a newly steered row goes: after the rows already steered, ahead
    /// of every ordinary row. Records `id` as steered.
    fn steer_slot(&self, id: &str) -> Result<usize, DocError> {
        let mut steered = lock(&self.steered_rows);
        let queue = self.doc.read_queue()?;
        steered.retain(|row| queue.iter().any(|q| &q.id == row));
        let slot = queue
            .iter()
            .take_while(|row| steered.contains(&row.id))
            .count();
        steered.push(id.to_string());
        Ok(slot)
    }

    fn publish_queue(&self) {
        match self.doc.read_queue() {
            Ok(items) => {
                self.queue_tx.send_if_modified(|slot| {
                    if *slot == items {
                        false
                    } else {
                        *slot = items;
                        true
                    }
                });
            }
            Err(err) => {
                tracing::warn!(chat = %self.chat_id, error = %err, "queue read failed");
            }
        }
    }

    fn touch(&self) {
        self.last_access.store(now_ms(), Ordering::Relaxed);
    }

    pub fn connected(&self) -> bool {
        lock(&self.chat2).is_some()
    }

    /// Write a complete user message entry, idempotent by id (the client-minted message
    /// id — a re-executed command or optimistic echo never duplicates the entry).
    pub fn write_user_message(
        &self,
        message_id: &str,
        text: &str,
        created_at: i64,
    ) -> Result<(), DocError> {
        if self.doc.read_entries_shared()?.iter().any(|e| e.id == message_id) {
            return Ok(());
        }
        self.doc.push_message(&SessionMessageEntry {
            id: message_id.to_string(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.to_string(),
            }],
            created_at,
            device_id: self.device_id.clone(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        })
    }

    /// Record that the chat moved from provider `from` to provider `to`: a
    /// system entry carrying the [`MessagePart::Switch`] seam, plus the
    /// `providerSwitched` meta flag that makes the next fresh provider session
    /// owe the transcript. Idempotent against a retried dispatch: nothing is
    /// written while the chat's latest seam already points at `to`. Returns
    /// whether a seam was written.
    pub fn write_provider_switch(
        &self,
        from: (HarnessId, &str),
        to: (HarnessId, &str),
        created_at: i64,
        before_message_id: Option<&str>,
    ) -> Result<bool, DocError> {
        let ((from_id, from), (to_id, to)) = (from, to);
        let entries = self.doc.read_entries_shared()?;
        if entries.is_empty() {
            return Ok(false);
        }
        if latest_switch(&entries) == Some(Some(to_id)) {
            return Ok(false);
        }
        let id = format!("switch:{created_at}");
        // Place the seam directly above the message that triggers this switch.
        // If that message is already in the transcript (e.g., written by the
        // client before dispatch), insert right before it; otherwise append at
        // the end so the message written next lands right below the seam. Never
        // walk back past an earlier unanswered user message that belonged to
        // the outgoing provider (such as an interrupted turn).
        let before = match before_message_id {
            Some(msg_id) => entries
                .iter()
                .rposition(|entry| entry.id == msg_id)
                .map_or(0, |ix| entries.len() - ix),
            None => entries
                .iter()
                .rev()
                .take_while(|entry| entry.role == MessageRole::User)
                .count(),
        };
        self.doc.insert_message_before_last(before, &SessionMessageEntry {
            id: id.clone(),
            role: MessageRole::System,
            parts: vec![MessagePart::Switch {
                id,
                from: from.to_string(),
                to: to.to_string(),
                from_harness: Some(from_id.as_str().to_owned()),
                to_harness: Some(to_id.as_str().to_owned()),
            }],
            created_at,
            device_id: self.device_id.clone(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        })?;
        self.doc.mark_provider_switched()?;
        Ok(true)
    }

    /// Recovery sweep: settle this device's running subagent chips (including
    /// chips in completed parent turns), then stamp abandoned `streaming`
    /// entries `aborted`, appending
    /// `note` as a visible error part so the transcript says WHY the turn
    /// ended (commutator folded "Run interrupted by backend restart" the same
    /// way). Returns the stamped entries' `(id, created_at)` — recovery uses
    /// them for the resume-freshness check.
    pub fn mark_abandoned_streams(&self, note: &str) -> Result<Vec<(String, i64)>, DocError> {
        let mut stamped = Vec::new();
        let mut chips_changed = false;
        for entry in self.doc.read_entries_shared()? {
            if entry.role != MessageRole::Assistant || entry.device_id != self.device_id {
                continue;
            }
            for part in &entry.parts {
                if let MessagePart::Tool {
                    id,
                    subagent_status: Some(SubagentStatus::Running),
                    ..
                } = part
                {
                    chips_changed |=
                        self.doc
                            .update_subagent_chip(id, None, Some("failed"), None)?;
                }
            }
            if entry.status == Some(MessageStatus::Streaming)
                && self
                    .doc
                    .set_message_status(&entry.id, MessageStatus::Aborted)?
            {
                let part_id = format!("{}-recovery", entry.id);
                if let Err(err) = self.doc.append_error_part(&entry.id, &part_id, note) {
                    tracing::warn!(chat = %self.chat_id, error = %err, "recovery note append failed");
                }
                stamped.push((entry.id.clone(), entry.created_at));
            }
        }
        if chips_changed || !stamped.is_empty() {
            self.publish_messages();
        }
        Ok(stamped)
    }

    /// A turn ended with nothing in the transcript to say so (its reply never
    /// reached this disk, or it never got to run): write that it failed, so
    /// the chat doesn't just stop at the user's message. Idempotent per `key`.
    pub fn note_turn_failure(&self, key: &str, note: &str) -> Result<(), DocError> {
        let id = format!("{key}-interrupted");
        if self.doc.read_entries_shared()?.iter().any(|entry| entry.id == id) {
            return Ok(());
        }
        self.doc.push_message(&SessionMessageEntry {
            id: id.clone(),
            role: MessageRole::Assistant,
            parts: Vec::new(),
            created_at: now_ms(),
            device_id: self.device_id.clone(),
            status: Some(MessageStatus::Aborted),
            continuation_of: None,
            duration_ms: None,
        })?;
        self.doc
            .append_error_part(&id, &format!("{id}-note"), note)?;
        self.publish_messages();
        Ok(())
    }

    pub(crate) fn publish_messages(&self) {
        let _import = lock(&self.transcript_import);
        self.publish_messages_locked();
    }

    // Caller holds transcript_import, shared with attach and mirror clearing.
    fn publish_messages_locked(&self) {
        self.mirror_dirty.store(false, Ordering::Release);
        match self.doc.read_entries_shared() {
            Ok(entries) => {
                let replay_baseline =
                    lock(&self.transcript_history).snapshot(self.doc.doc(), &entries);
                let joined = commutator_doc::join_continuation_shared(entries);
                // send_replace: update the watch even with no subscribers yet, so a
                // late subscriber's first borrow sees the current transcript.
                self.messages_tx.send_replace(TranscriptSnapshot {
                    entries: Arc::new(joined),
                    replay_baseline: replay_baseline.clone(),
                });
            }
            Err(err) => {
                tracing::warn!(chat = %self.chat_id, error = %err, "transcript read failed");
            }
        }
    }

    pub(crate) fn import_transcript<T>(&self, import: impl FnOnce() -> T) -> T {
        let _guard = lock(&self.transcript_import);
        import()
    }

    /// Per-commit publish path: unwatched docs just mark the mirror dirty —
    /// rebuilding a full transcript nobody reads was a per-tick cost on every
    /// open doc (and kept a second transcript copy hot).
    fn publish_messages_if_watched(&self) {
        // Serialize the receiver check AND clear with attach. Otherwise an
        // unwatched worker can clear the mirror after a new watcher rebuilt it.
        let _import = lock(&self.transcript_import);
        if self.messages_tx.receiver_count() == 0 {
            self.mirror_dirty.store(true, Ordering::Release);
            // Shrink the stale mirror: watch_messages rebuilds on attach.
            self.messages_tx.send_replace(TranscriptSnapshot::default());
            // And the doc's read cache, which the mirror shared: a chat
            // nobody shows holds its transcript no more than it used to.
            self.doc.set_keep_read_cache(false);
            *lock(&self.transcript_history) = Default::default();
        } else {
            self.publish_messages_locked();
        }
    }

    /// Whether this chat's transcript is long enough that what goes with all
    /// of it takes a while: reading it whole (a doc just opened, or one
    /// nobody has watched since it changed, reads every entry from the doc),
    /// and even with its entries kept, summing each to tell a reader what
    /// changed. A viewer holding rows is sent what changed among the newest
    /// entries first (`WatchDocMessages`).
    pub(crate) fn transcript_is_long(&self) -> bool {
        let bytes = self
            .snapshot_bytes
            .load(Ordering::Relaxed)
            .max(self.persistence.as_ref().map_or(0, |p| p.snapshot_bytes()));
        bytes >= LONG_TRANSCRIPT_BYTES
    }

    /// Rough resident cost for the LRU budget.
    fn resident_estimate(&self) -> usize {
        let bytes = self
            .snapshot_bytes
            .load(Ordering::Relaxed)
            .max(self.persistence.as_ref().map_or(0, |p| p.snapshot_bytes()));
        // (The read cache a watched chat keeps comes on top.)
        (bytes * RESIDENT_BYTES_PER_SNAPSHOT_BYTE).max(DOC_RESIDENT_FLOOR_BYTES)
            + self.doc.cached_bytes()
    }
}

impl DocHost {
    pub(crate) fn downgrade(&self) -> WeakDocHost {
        WeakDocHost(Arc::downgrade(&self.inner))
    }

    pub fn new(store: Arc<DocsStore>, config: DocHostConfig) -> Self {
        let host = Self {
            inner: Arc::new(DocHostInner {
                store,
                config,
                sessions: Mutex::new(None),
                quiesced: AtomicBool::new(false),
                boot_recovery_finished: AtomicBool::new(false),
                dispatching: Arc::new(AtomicUsize::new(0)),
                workspace: OnceLock::new(),
                repos: OnceLock::new(),
                project_action_runtime: OnceLock::new(),
                shutdown: CancellationToken::new(),
                edge_disconnected: AtomicBool::new(false),
                sync_kick: Arc::new(tokio::sync::Notify::new()),
                #[cfg(test)]
                scheduler_passes: AtomicUsize::new(0),
                tasks: TaskTracker::new(),
                handles: Mutex::new(HashMap::new()),
                document_loads: AtomicU64::new(0),
                focus_clock: AtomicU64::new(0),
                opening: Mutex::new(()),
                seeding: Mutex::new(HashSet::new()),
                seed_waiting: Mutex::new(HashSet::new()),
                drain_waiting: Mutex::new(HashSet::new()),
                uploads: OnceLock::new(),
                connectivity: OnceLock::new(),
                workspace_wired: tokio::sync::Notify::new(),
                transfers: watch::channel(Vec::new()).0,
                held_active: Mutex::new(HashSet::new()),
                held_gate: Mutex::new(()),
                held_kick: tokio::sync::Notify::new(),
                held_status: watch::channel(Vec::new()).0,
                executing: Mutex::new(HashSet::new()),
                links: OnceLock::new(),
            }),
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            host.spawn_sync_scheduler();
        }
        host
    }

    /// Every background task rides the tracker, raced against the shutdown
    /// token: the loops' own exits stay authoritative in normal operation;
    /// the token is the retirement override.
    fn spawn_worker(&self, fut: impl std::future::Future<Output = ()> + Send + 'static) {
        let cancel = self.inner.shutdown.clone();
        self.inner.tasks.spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = fut => {}
            }
        });
    }

    /// `spawn_worker` for sites that pre-resolve a runtime handle (callers
    /// reachable from bare sync contexts, where `tasks.spawn` would panic).
    fn spawn_worker_on(
        &self,
        runtime: &tokio::runtime::Handle,
        fut: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let cancel = self.inner.shutdown.clone();
        self.inner.tasks.spawn_on(
            async move {
                tokio::select! {
                    _ = cancel.cancelled() => {}
                    _ = fut => {}
                }
            },
            runtime,
        );
    }

    /// The sessions engine, once wired. `None` before assembly or after
    /// `shutdown_workers` — callers treat both as "executor unavailable".
    fn sessions(&self) -> Option<SessionsEngine> {
        lock(&self.inner.sessions).clone()
    }

    /// Entered by a drain that may start work; `None` while quiesced. Count
    /// first, check second (both SeqCst): a quiesce that set its flag then
    /// either sees this drain counted or this drain sees the flag.
    fn dispatch_guard(&self) -> Option<DispatchGuard> {
        self.inner.dispatching.fetch_add(1, Ordering::SeqCst);
        let guard = DispatchGuard(self.inner.dispatching.clone());
        (!self.inner.quiesced.load(Ordering::SeqCst)).then_some(guard)
    }

    /// Start no new work until [`Self::lift_quiesce`]: a new version trying out
    /// an update, whose changes may yet be undone. Controls keep working, as
    /// they do in a quiesce, and what queues meanwhile runs when it's lifted.
    pub fn hold_work(&self) {
        self.inner.quiesced.store(true, Ordering::SeqCst);
    }

    /// End a quiesce and run everything that queued meanwhile. A boot that held
    /// work has done its recovery by now, whatever it was holding for.
    pub fn lift_quiesce(&self) {
        self.inner.boot_recovery_finished.store(true, Ordering::Release);
        if !self.inner.quiesced.swap(false, Ordering::SeqCst) {
            return;
        }
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            let host = self.clone();
            self.spawn_worker(async move {
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
            });
        }
    }

    /// Wire the sessions engine (engine assembly; see `SessionsEngine::set_doc_host`).
    pub fn set_sessions(&self, sessions: SessionsEngine) {
        let statuses = sessions.watch_sessions();
        {
            // First set wins (the OnceLock contract this slot replaced).
            let mut slot = lock(&self.inner.sessions);
            if slot.is_none() {
                *slot = Some(sessions);
            }
        }
        // Commands may already be pending in warm-opened docs.
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            let host = self.clone();
            self.spawn_worker(async move {
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
            });
        }
        self.spawn_queue_flush_watcher(statuses);
    }

    /// The turn-end hook for held messages. Doc changes drive `drain_queue` for
    /// everything else, but a turn ENDING is not a doc change the queue can
    /// see — so watch session status instead and re-drain every warm chat.
    /// `drain_queue` is a cheap no-op for empty queues and busy agents, which
    /// is why this can afford to be indiscriminate.
    fn spawn_queue_flush_watcher(&self, mut statuses: watch::Receiver<Vec<commutator_proto::Session>>) {
        let host = self.clone();
        self.spawn_worker(async move {
            while statuses.changed().await.is_ok() {
                let handles: Vec<_> = lock(&host.inner.handles).values().cloned().collect();
                for handle in handles {
                    host.drain_queue(&handle).await;
                }
            }
        });
    }

    /// Retire this host's workers (runtime replacement, e.g. sign-out): cancel
    /// and await every spawned task, drop every open chat handle (ending the
    /// weak-keyed room/join loops and watcher streams), and sever the sessions
    /// back-edge so the replaced engine graph can actually drop. Idempotent.
    pub async fn shutdown_workers(&self) {
        self.inner.shutdown.cancel();
        self.inner.tasks.close();
        self.inner.tasks.wait().await;
        // Stop room actors before the final snapshot so shutdown does not
        // leave a scheduled debounce behind an already-dropped document.
        let clients: Vec<_> = lock(&self.inner.handles)
            .values()
            .filter_map(|handle| lock(&handle.chat2).take())
            .collect();
        futures::future::join_all(clients.into_iter().map(|client| client.shutdown())).await;
        // Snapshot open docs BEFORE releasing their handles: the handles map
        // holds the only strong doc refs, and an unflushed doc dies with it.
        let host = self.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || host.flush_all()).await {
            tracing::error!(%error, "shutdown snapshot flush failed");
        }
        // Take the map under the lock, drop the handles outside it.
        let handles = std::mem::take(&mut *lock(&self.inner.handles));
        drop(handles);
        lock(&self.inner.seeding).clear();
        lock(&self.inner.seed_waiting).clear();
        lock(&self.inner.sessions).take();
    }

    /// Freeze every open queue before settling live runs during shutdown.
    /// Interrupting a run publishes Idle, which normally wakes the turn-end
    /// queue drainer; without this barrier quitting the host could promote a
    /// queued row in the narrow window before workers are retired.
    pub fn pause_all_queues(&self) {
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            handle.queue_paused.store(true, Ordering::Release);
        }
    }

    /// Test-only retirement sentinel: reports true once the doc-host graph
    /// has actually been freed.
    #[doc(hidden)]
    pub fn retirement_probe(&self) -> Box<dyn Fn() -> bool + Send + Sync> {
        let weak = Arc::downgrade(&self.inner);
        Box::new(move || weak.upgrade().is_none())
    }

    /// Wire the repos engine (engine assembly) — worktree materialization for
    /// Run commands carrying a [`commutator_proto::WorktreeSpec`].
    pub fn set_repos(&self, repos: crate::repos::Repos) {
        let _ = self.inner.repos.set(repos);
    }

    pub fn set_project_action_runtime(
        &self,
        project_actions: ProjectActionsStore,
        terminals: Terminals,
    ) {
        let _ = self
            .inner
            .project_action_runtime
            .set((project_actions, terminals));
    }

    /// Wire the uploads store (engine assembly) — `pending://` ref resolution
    /// and the transfer-read jail.
    pub fn set_uploads(&self, uploads: crate::uploads::Uploads) {
        let _ = self.inner.uploads.set(uploads);
        // Sends still held for their attachment bytes pick up where they were.
        // Here because they read those bytes from this store.
        if tokio::runtime::Handle::try_current().is_ok() {
            self.resume_held_sends();
        }
    }

    /// Wire the peer-link cache (engine assembly, edge runtimes only) — the
    /// transport for queued attachment transfers to a remote host.
    pub fn set_links(&self, links: Arc<commutator_rpc::LinkCache>) {
        let _ = self.inner.links.set(links);
    }

    /// Boot recovery has been through every chat it cut off, and each it means
    /// to pick back up is marked: queues it restored can go now, or after those
    /// turns.
    pub fn boot_recovery_finished(&self) {
        self.inner.boot_recovery_finished.store(true, Ordering::Release);
        self.kick_drains();
    }

    /// Re-evaluate every open chat's commands and queue NOW. Called after an
    /// upload commit lands bytes on this device: a Run or queued row deferred
    /// on those bytes (`pending://` refs not yet on disk) becomes executable
    /// the moment its transfer completes — event-driven, not timer luck.
    pub fn kick_drains(&self) {
        let handles: Vec<Arc<ChatDocHandle>> =
            lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            let host = self.clone();
            self.spawn_worker(async move {
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
            });
        }
    }

    /// Wire the workspace host (engine assembly) — the source of chat-ownership rows.
    pub fn set_workspace(&self, workspace: WorkspaceHost) {
        let chats = workspace.watch_chats();
        if self.inner.workspace.set(workspace).is_ok() {
            self.inner.workspace_wired.notify_one();
            self.spawn_cutover_watcher(chats);
            self.kick_sync();
        }
    }

    /// Live cutover convergence: when a registry chat row flips to roomGen 2
    /// while this device holds an s2-mode handle, retire it NOW — a viewer
    /// watching the chat at flip time otherwise stays frozen on the dead s2
    /// room until they happen to reopen (the host writes only to chat2 from
    /// the flip on). Dropping the handle ends the watch streams cleanly; the
    /// UI's transcript/standing watches resubscribe and the fresh open takes
    /// the chat2 adopt path. A handle with a LIVE local writer (a running
    /// turn's doc ref) is left alone — the host never flips mid-run, and a
    /// racing writer must never lose its doc out from under it.
    fn spawn_cutover_watcher(&self, mut chats: watch::Receiver<Vec<commutator_proto::Chat>>) {
        let host = self.clone();
        self.spawn_worker(async move {
            loop {
                if chats.changed().await.is_err() {
                    return; // workspace host gone (shutdown)
                }
                let flipped: Vec<String> = chats
                    .borrow_and_update()
                    .iter()
                    .filter(|c| c.room_gen.unwrap_or(1) >= 2)
                    .map(|c| c.id.clone())
                    .collect();
                if flipped.is_empty() {
                    continue;
                }
                let mut dropped: Vec<String> = Vec::new();
                {
                    let mut handles = lock(&host.inner.handles);
                    for chat_id in flipped {
                        let Some(handle) = handles.get(&chat_id) else {
                            continue;
                        };
                        if handle.room_gen >= 2 {
                            continue; // already chat2-mode
                        }
                        let live_writer = Arc::strong_count(&handle.doc) > 1;
                        if live_writer {
                            // A run is writing into this s2 doc while the
                            // registry already says chat2 (the born-gen2
                            // race): retiring it would drop the doc's only
                            // persistence. Leave it live.
                            continue;
                        }
                        handle.retired.store(true, Ordering::Relaxed);
                        handles.remove(&chat_id);
                        tracing::info!(chat = %chat_id,
                            "s2 handle dropped on chat2 cutover; watchers resubscribe onto the new room");
                        dropped.push(chat_id);
                    }
                }
                // Watchers resubscribe on their own — but a NUDGE-opened
                // handle has none, and its s2 room never carried the queued
                // command anyway (the sender pushed to chat2). On a born-
                // chat2 chat the nudge beats the registry row by design
                // (direct HTTP vs room sync), so the first open lands here
                // and dying silently strands the first message until the
                // next nudge — every new remote session's first send sat
                // ~30s+ until the user re-sent (user report). If we host
                // the chat, reopen NOW: the fresh open dials the chat2
                // room and the change-driven drain executes the command.
                for chat_id in dropped {
                    if !host.is_host(&chat_id) {
                        continue;
                    }
                    match host.open(&chat_id) {
                        Ok(_) => tracing::info!(chat = %chat_id,
                            "reopened as chat2 after cutover drop (host, pending work possible)"),
                        Err(err) => tracing::warn!(chat = %chat_id, error = %err,
                            "chat2 reopen after cutover drop failed"),
                    }
                }
            }
        });
    }

    /// The workspace host, once wired (tests may assemble a DocHost without one).
    pub fn workspace(&self) -> Option<&WorkspaceHost> {
        self.inner.workspace.get()
    }

    pub fn device_id(&self) -> &str {
        &self.inner.config.device_id
    }

    /// Open (or return) the chat's doc handle: load the local snapshot (or init fresh),
    /// start the change-driven task, and request budgeted sync when configured.
    pub fn open(&self, chat_id: &str) -> Result<Arc<ChatDocHandle>, EngineError> {
        let handle = self.open_local(chat_id)?;
        self.activate_sync(&handle);
        Ok(handle)
    }

    /// Explicit viewport navigation; automatic watch retries and MCP reads use
    /// `open` instead. The sequence also orders focuses in the same millisecond.
    pub fn focus_chat(&self, chat_id: &str) -> Result<(), EngineError> {
        let handle = self.open_local(chat_id)?;
        let focus = self.inner.focus_clock.fetch_add(1, Ordering::AcqRel) + 1;
        handle.last_focus.fetch_max(focus, Ordering::AcqRel);
        self.activate_sync(&handle);
        Ok(())
    }

    /// Materialize one authoritative local document without acquiring a network
    /// connection. Durable publication is installed before exposing any writer.
    pub fn open_local(&self, chat_id: &str) -> Result<Arc<ChatDocHandle>, EngineError> {
        // A chat with no id has no room to sync with: it dialed one forever.
        if chat_id.is_empty() {
            return Err(EngineError::Other("a chat needs an id".into()));
        }
        // The registry names the sync room generation (docs/chat2-sync.md
        // M2): absent row / absent field = legacy s2. Read it BEFORE the
        // cached-handle check — a cached s2-mode handle for a chat another
        // device has since cut over to chat2 would otherwise serve its frozen
        // fat lineage forever (the host writes only to the chat2 room now;
        // this device's s2 room has gone permanently silent).
        let chat_row = self
            .workspace()
            .and_then(|w| w.chat(chat_id).ok().flatten());
        // A row that EXISTS without `roomGen` is a pre-cutover legacy chat
        // (gen 1). A MISSING row is a chat being born right now: its
        // CreateChat mint (which stamps roomGen 2) is racing this open —
        // the composer attaches the transcript watch before its own mutate
        // lands, and a nudge beats registry sync by design. Defaulting the
        // absent row to 1 minted brand-new s2 rooms post-cutover: the host
        // ran the whole session against a room no other device reads (they
        // follow the row's gen 2 to an empty chat2 room), the run's live doc
        // ref blocked every heal, and the transcript never synced anywhere
        // (2026-08-11).
        let registry_gen = match chat_row.as_ref() {
            Some(row) => row.room_gen.unwrap_or(1),
            None => 2,
        };
        {
            let mut handles = lock(&self.inner.handles);
            if let Some(handle) = handles.get(chat_id) {
                let stale = registry_gen >= 2 && handle.room_gen < 2;
                if (stale || handle.retired.load(Ordering::Relaxed)) && !self.pinned(handle) {
                    // A seed flipped this chat under a cached fat handle
                    // (review B1): drop it so this open converges onto the
                    // thin lineage + chat2 room. Retire only at the drop:
                    // marking a PINNED stale handle retired while it kept
                    // serving suppressed the only persistence a stuck-live
                    // doc had and aborted its quiet-seed (born-gen2 race,
                    // 2026-08-11) — the cutover watcher and the seed itself
                    // own converging pinned handles.
                    handle.retired.store(true, Ordering::Relaxed);
                    handles.remove(chat_id);
                } else {
                    handle.touch();
                    return Ok(handle.clone());
                }
            }
        }
        let opening = lock(&self.inner.opening);
        if let Some(handle) = lock(&self.inner.handles).get(chat_id) {
            handle.touch();
            return Ok(handle.clone());
        }
        // B2/M5 guard: the LOCAL epoch is the second cutover signal. A crash
        // between the thin save and the registry flip (or a not-yet-synced
        // registry) must NOT route an epoch-2 doc back onto s2 — the s2
        // room's fat doc would merge into the unrelated thin lineage and
        // duplicate every message. Local epoch >= 2 forces the chat2 branch
        // and best-effort completes the flip.
        self.inner.document_loads.fetch_add(1, Ordering::Relaxed);
        let stored = self.inner.store.load_snapshot_with_cursor(chat_id)?;
        let stored_epoch = stored.as_ref().map(|(_, _, e)| *e).unwrap_or(0);
        let room_gen = if stored_epoch >= crate::chat2_host::CHAT2_DOC_EPOCH {
            if registry_gen < 2
                && let Some(ws) = self.workspace()
            {
                let _ = ws.set_chat_room_gen(chat_id, 2);
                tracing::info!(chat = %chat_id,
                    "completed interrupted chat2 flip (local epoch 2, registry said s2)");
            }
            2
        } else {
            registry_gen
        };
        let deferred_adoption = room_gen >= 2
            && stored_epoch < crate::chat2_host::CHAT2_DOC_EPOCH
            && stored.is_some();
        let mut snapshot_len = 0usize;
        let mut chat2_cursor = 0u64;
        let doc = if room_gen >= 2 {
            match stored {
                Some((bytes, cursor, epoch)) if epoch >= crate::chat2_host::CHAT2_DOC_EPOCH => {
                    snapshot_len = bytes.len();
                    chat2_cursor = cursor;
                    let raw = loro::LoroDoc::new();
                    raw.import(&bytes)
                        .map_err(|e| EngineError::Other(format!("snapshot import failed: {e}")))?;
                    SessionDoc::from_doc(raw)
                }
                Some((bytes, _cursor, epoch)) => {
                    // A pre-chat2 doc stays as it is: with no room to catch
                    // up from, adopting would blank a readable transcript.
                    tracing::info!(chat = %chat_id, old_epoch = epoch,
                        "chat2 adopt deferred (no room to catch up from)");
                    snapshot_len = bytes.len();
                    let raw = loro::LoroDoc::new();
                    raw.import(&bytes)
                        .map_err(|e| EngineError::Other(format!("snapshot import failed: {e}")))?;
                    SessionDoc::from_doc(raw)
                }
                None => {
                    // Born on chat2 (or a cold reader's first open): stamp
                    // the epoch-2 lineage NOW. Plain snapshot saves preserve
                    // an existing row's epoch but default a NEW row to 0 —
                    // without this stamp, the next open reads "pre-chat2
                    // doc" and the M3 adopt DISCARDS everything written
                    // since (caught by the restart_resume suite: first-turn
                    // transcripts vanished on reopen).
                    let doc = SessionDoc::init(chat_id)?;
                    if let Ok(snapshot) = doc.export_snapshot() {
                        let _ = self.inner.store.save_snapshot_with_cursor(
                            chat_id,
                            &snapshot,
                            0,
                            crate::chat2_host::CHAT2_DOC_EPOCH,
                        );
                    }
                    doc
                }
            }
        } else {
            match stored {
                Some((bytes, _, _)) => {
                    snapshot_len = bytes.len();
                    let raw = loro::LoroDoc::new();
                    raw.import(&bytes)
                        .map_err(|e| EngineError::Other(format!("snapshot import failed: {e}")))?;
                    SessionDoc::from_doc(raw)
                }
                None => SessionDoc::init(chat_id)?,
            }
        };
        // Parked rows replayed in full: retire them after a save covers them.
        let mut parked_retirement: Option<(String, i64)> = None;
        // Recover committed outgoing operations even when the snapshot debounce
        // did not run before a crash. Imported updates do not echo as local writes.
        if room_gen >= 2 {
            for (_, bytes) in self.inner.store.pending_chat_updates(chat_id)? {
                doc.import(&bytes)
                    .map_err(|e| EngineError::Other(e.to_string()))?;
            }
            // The same for what a crash left unsaved of what came by relay. A row
            // this build can't read is skipped, as a stored row would be.
            let (received, _) = self.inner.store.pending_chat_inbox(chat_id)?;
            for bytes in received {
                if let Err(err) = doc.import(&bytes) {
                    tracing::warn!(chat = %chat_id, error = %err, "chat relay: couldn't replay a kept update");
                }
            }
            // Updates that came ahead of their history. Once all of them are in, none
            // is needed again; until then all stay.
            let parked_key = crate::chat2_host::parked_inbox(chat_id);
            let (parked, newest) = self.inner.store.pending_chat_inbox(&parked_key)?;
            if !parked.is_empty() {
                let mut waiting = false;
                for bytes in &parked {
                    match doc.import(bytes) {
                        Ok(status) => waiting |= status.pending.is_some(),
                        Err(err) => tracing::warn!(chat = %chat_id, error = %err, "chat relay: couldn't replay a parked update"),
                    }
                }
                // Retired only once a save holds them (below): until then
                // these ops are in memory only, and the rows their one copy.
                if !waiting {
                    parked_retirement = Some((parked_key, newest));
                }
            }
        }
        let doc = Arc::new(doc);
        let persistence = (room_gen >= 2 && !deferred_adoption).then(|| {
            crate::chat_persistence::ChatPersistence::new(
                &doc,
                self.inner.store.clone(),
                chat_id.to_string(),
                chat2_cursor,
            )
        });
        if let Some((parked_key, newest)) = parked_retirement {
            // Imports at open precede the change subscription, so nothing
            // marks the doc dirty: save now, and let the rows go only if that
            // save landed (else they replay again at the next open).
            match &persistence {
                Some(persistence) => {
                    persistence.dirty(true);
                    persistence.flush_sync();
                    if persistence.is_clean() {
                        self.inner.store.retire_chat_inbox(&parked_key, newest)?;
                    }
                }
                None => tracing::info!(chat = %chat_id, "parked updates replayed; kept until a chat2 save holds them"),
            }
        }
        let changed_persistence = persistence.clone();

        let (changed_tx, changed_rx) = watch::channel(0u64);
        let (messages_tx, _) = watch::channel(TranscriptSnapshot::default());
        let transcript_history = Arc::new(Mutex::new(
            crate::transcript_history::TranscriptHistory::default(),
        ));
        let history = transcript_history.clone();
        let watched = messages_tx.clone();
        let weak_doc = Arc::downgrade(&doc);
        let sub = doc.doc().subscribe_root(Arc::new(move |diff| {
            // This callback runs before the change worker can publish. The
            // import origin belongs to the event, so concurrent local commits
            // cannot inherit a remote replay's presentation classification.
            if watched.receiver_count() > 0 {
                if let Some(doc) = weak_doc.upgrade() {
                    lock(&history).observe(doc.doc(), &diff);
                }
            } else {
                *lock(&history) = Default::default();
            }
            if let Some(persistence) = &changed_persistence {
                persistence.dirty(false);
            }
            changed_tx.send_modify(|v| *v = v.wrapping_add(1));
        }));
        // The mirror starts dirty and empty; watch_messages materializes it
        // once on attach instead of maintaining an unwatched transcript.
        let initial_queue = doc.read_queue().unwrap_or_default();
        // A queue already present when a handle is materialized came from a
        // persisted snapshot (or a synced checkpoint), not from a prompt the
        // user submitted to this live engine. Keep that recovered work frozen
        // until an explicit prompt / Send now / Steer action thaws it. Rows
        // appended after the handle exists retain the normal automatic drain.
        let recovered_queue_pending = !initial_queue.is_empty();
        let (queue_tx, _) = watch::channel(initial_queue);

        // Reads keep the transcript's entries only while someone watches it
        // (`watch_messages`); until then each read reads it whole, as ever.
        doc.set_keep_read_cache(false);
        let handle = Arc::new(ChatDocHandle {
            chat_id: chat_id.to_string(),
            device_id: self.inner.config.device_id.clone(),
            doc: doc.clone(),
            sync_kick: self.inner.sync_kick.clone(),
            unwatch_waiter: Arc::new(AtomicBool::new(false)),
            unwatch_cancel: CancellationToken::new(),
            runtime: tokio::runtime::Handle::try_current().ok(),
            messages_tx,
            transcript_import: Mutex::default(),
            transcript_history,
            queue_tx,
            changes: changed_rx.clone(),
            drain_lock: tokio::sync::Mutex::new(()),
            command_drain_lock: tokio::sync::Mutex::new(()),
            steered_rows: Mutex::new(Vec::new()),
            held_sends: Mutex::new(HashMap::new()),
            stopped_by: Mutex::new(None),
            queue_paused: AtomicBool::new(recovered_queue_pending),
            mirror_dirty: AtomicBool::new(true),
            last_access: AtomicI64::new(now_ms()),
            last_focus: AtomicU64::new(0),
            snapshot_bytes: AtomicUsize::new(snapshot_len),
            room_gen,
            retired: AtomicBool::new(false),
            chat2: Mutex::new(None),
            sync_started: AtomicBool::new(false),
            sync_requested: AtomicBool::new(false),
            sync_background: AtomicBool::new(false),
            sync_wait_since: AtomicI64::new(0),
            sync_cancel: Mutex::new(CancellationToken::new()),
            writers: Arc::new(AtomicUsize::new(0)),
            persistence,
            publication_failed: AtomicBool::new(false),
            _sub: sub,
        });
        // Snapshot recovery may restore several independently edited rows.
        // Each row needs its own checked expiry wake; otherwise a non-head
        // edit could remain displayed as live indefinitely.
        self.arm_existing_queue_edit_expiries(&handle);

        // Publish only after the durable subscription and bootstrap are installed.
        lock(&self.inner.handles).insert(chat_id.to_string(), handle.clone());
        drop(opening);
        self.kick_sync();
        self.spawn_worker(chat_task(self.clone(), Arc::downgrade(&handle), changed_rx));
        self.evict_over_budget();
        Ok(handle)
    }

    /// Connection lifetime is independent of the document and its outbox.
    /// Concurrent callers activate at most one supervisor for this handle.
    /// Called on the device control link. Acceptance never cold-opens a chat.
    pub fn enqueue_wakeup(&self, chat_id: &str) -> Result<(), EngineError> {
        self.inner
            .store
            .schedule_sync_job(chat_id, if chat_id == "*" { "reconcile" } else { "wake" })?;
        Ok(())
    }

    pub fn activate_sync(&self, handle: &Arc<ChatDocHandle>) {
        handle.sync_background.store(false, Ordering::Release);
        if !handle.sync_started.load(Ordering::Acquire) {
            let _ = handle.sync_wait_since.compare_exchange(
                0,
                now_ms(),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }

    /// Run the sync dispatcher's next pass (at most one per
    /// [`SYNC_PASS_SPACING`]). Call after changing anything it reads that
    /// the store's own change signals don't cover.
    pub(crate) fn kick_sync(&self) {
        self.inner.sync_kick.notify_one();
    }

    /// Keeps the open chat docs under their memory budget: a pass now, on
    /// every [`Self::kick_sync`], and again [`SYNC_PASS_SPACING`] later while
    /// a pinned set stays over budget.
    fn spawn_sync_scheduler(&self) {
        let weak = Arc::downgrade(&self.inner);
        let kick = self.inner.sync_kick.clone();
        self.spawn_worker(async move {
            loop {
                let busy = {
                    let Some(inner) = weak.upgrade() else { return };
                    #[cfg(test)]
                    inner.scheduler_passes.fetch_add(1, Ordering::Relaxed);
                    Self { inner }.evict_over_budget()
                };
                tokio::select! {
                    _ = tokio::time::sleep(SYNC_PASS_SPACING), if busy => {}
                    _ = kick.notified() => {}
                }
            }
        });
    }

    /// Boot-time transcript salvage (born-gen2 aftermath, 2026-08-11): a chat
    /// we host whose chat2 doc has NO message entries while its run journal
    /// has events lost its transcript to a stuck s2 handle (the retired flag
    /// suppressed every snapshot save; the post-restart reopen born a blank
    /// lineage). The full fat doc still exists in the legacy s2 room — the
    /// stuck engine pushed every op into it until it died — and sometimes in
    /// a `.pre-chat2` rollback on disk. Re-append its entries (thinned) into
    /// the LIVE chat2 lineage as ordinary incremental updates: no lineage
    /// replacement, no checkpoint surgery, every device converges through
    /// the normal room flow. Idempotent: a doc with any message entry is
    /// never touched, and the salvage only runs on the hosting device.
    pub fn spawn_transcript_salvage(&self, journals_dir: std::path::PathBuf) {
        let host = self.clone();
        self.spawn_worker(async move {
            // Let boot settle (registry load, room joins) before sweeping.
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let Some(ws) = host.workspace() else { return };
            let chats: Vec<commutator_proto::Chat> = ws.watch_chats().borrow().clone();
            for chat in chats {
                if chat.device_id != host.inner.config.device_id {
                    continue; // only the host owns its chats' history
                }
                if chat.room_gen.unwrap_or(1) < 2 {
                    continue; // still s2-mode: transcript lives in its room
                }
                let journal = journals_dir.join(format!("{}.jsonl", chat.id));
                let journaled = std::fs::metadata(&journal)
                    .map(|m| m.len() > 0)
                    .unwrap_or(false);
                if !journaled {
                    continue; // never ran here — an empty doc is just new
                }
                if let Err(err) = host.salvage_chat_transcript(&chat.id).await {
                    let _ = host.inner.store.schedule_sync_job(&chat.id, "recovery");
                    tracing::warn!(chat = %chat.id, error = %err, "transcript salvage failed");
                }
            }
            let mut after = String::new();
            let mut jobs_changed = host.inner.store.watch_sync_jobs();
            loop {
                // With no recovery owed, nothing to retry: sleep until a
                // job is scheduled (a salvage failed) rather than looking
                // every half minute. One owed is tried a half minute apart.
                if after.is_empty() {
                    jobs_changed.borrow_and_update();
                    let owed = host
                        .inner
                        .store
                        .pending_sync_jobs("recovery", "", 1)
                        .map_or(true, |jobs| !jobs.is_empty());
                    if !owed {
                        if jobs_changed.changed().await.is_err() {
                            return;
                        }
                        continue;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                let jobs = host
                    .inner
                    .store
                    .pending_sync_jobs("recovery", &after, 1)
                    .unwrap_or_default();
                let Some(chat) = jobs.into_iter().next() else {
                    after.clear();
                    continue;
                };
                after = chat.clone();
                let version = host
                    .inner
                    .store
                    .sync_job_version(&chat, "recovery")
                    .ok()
                    .flatten();
                if host.salvage_chat_transcript(&chat).await.is_ok() {
                    if let Some(version) = version {
                        let _ = host
                            .inner
                            .store
                            .complete_sync_job(&chat, "recovery", version);
                    }
                }
            }
        });
    }

    async fn salvage_chat_transcript(&self, chat_id: &str) -> Result<(), String> {
        // Inspection must not register a handle or start sync. A healthy
        // history used to open one room per journal during every boot.
        let stored = self
            .inner
            .store
            .load_snapshot_with_cursor(chat_id)
            .map_err(|e| e.to_string())?;
        if let Some((bytes, _, _)) = &stored {
            {
                let raw = loro::LoroDoc::new();
                raw.import(bytes).map_err(|e| e.to_string())?;
                for (_, update) in self
                    .inner
                    .store
                    .pending_chat_updates(chat_id)
                    .map_err(|e| e.to_string())?
                {
                    raw.import(&update).map_err(|e| e.to_string())?;
                }
                if !SessionDoc::from_doc(raw)
                    .read_entries()
                    .map_err(|e| e.to_string())?
                    .is_empty()
                {
                    return Ok(());
                }
            }
        }
        let rollback_id = format!("{chat_id}.pre-chat2");
        let source = self
            .inner
            .store
            .load_snapshot(&rollback_id)
            .map_err(|e| e.to_string())?
            .or_else(|| {
                stored
                    .filter(|(_, _, epoch)| *epoch < crate::chat2_host::CHAT2_DOC_EPOCH)
                    .map(|(bytes, _, _)| bytes)
            });
        let Some(bytes) = source else { return Ok(()) };
        let raw = loro::LoroDoc::new();
        raw.import(&bytes).map_err(|e| e.to_string())?;
        let fat = SessionDoc::from_doc(raw);
        let rebuilt = commutator_doc::rebuild::rebuild_thin_doc(&fat).map_err(|e| e.to_string())?;
        let entries = rebuilt.doc.read_entries().map_err(|e| e.to_string())?;
        if entries.is_empty() {
            return Ok(());
        }
        let handle = self.open(chat_id).map_err(|e| e.to_string())?;
        if !handle
            .doc()
            .read_entries()
            .map_err(|e| e.to_string())?
            .is_empty()
        {
            return Ok(()); // transcript present — nothing lost
        }
        // Fat source: the M3 adopt's rollback copy on disk. (The other
        // source — the legacy s2 room — went away with the s2 client; any
        // transcript that existed only there was salvaged by earlier
        // releases or is reachable in the room's storage server-side.)
        // Thin before appending (docs/chat2-sync.md A2): full outputs are
        // parked, exactly like a seed — they survive in the rollback copy
        // saved below and the run journal.
        if matches!(self.inner.store.load_snapshot(&rollback_id), Ok(None)) {
            let _ = self.inner.store.save_snapshot(&rollback_id, &bytes);
        }
        // Re-check emptiness at the last instant: a run that started during
        // the room fetch must not get history interleaved under it.
        let _drain = handle.drain_lock.lock().await;
        let _import = lock(&handle.transcript_import);
        if Arc::strong_count(&handle.doc) > 1
            || !handle
                .doc()
                .read_entries()
                .map_err(|e| e.to_string())?
                .is_empty()
        {
            return Err("doc gained entries mid-salvage; aborted".into());
        }
        for entry in &entries {
            handle
                .doc()
                .push_message(entry)
                .map_err(|e| e.to_string())?;
        }
        tracing::info!(chat = %chat_id, entries = entries.len(),
            "transcript salvaged into chat2 lineage");
        Ok(())
    }

    /// LRU eviction: while the warm set exceeds [`WARM_DOC_CAP`] or the
    /// resident estimate exceeds `DOC_LRU_BYTE_BUDGET`, close the
    /// least-recently-touched unpinned docs. Pinned (never evicted):
    /// - watched docs (`messages_tx` has receivers — a UI transcript);
    /// - docs with a live writer (`Arc<SessionDoc>` held outside the handle —
    ///   a run streaming into it);
    /// - host-side docs with pending commands (the executor owes them work).
    ///
    /// Eviction flushes a final snapshot, so reopen loses nothing; missed
    /// remote updates re-arrive through the room join's VV backfill.
    ///
    /// True when the warm set is still over budget: every doc that could
    /// close was pinned, so the dispatcher retries while that lasts.
    fn evict_over_budget(&self) -> bool {
        // Do not let a cold reopen race the retiring handle's final flush.
        let _opening = lock(&self.inner.opening);
        let mut by_age: Vec<(i64, String)> = {
            let handles = lock(&self.inner.handles);
            handles
                .values()
                .map(|h| (h.last_access.load(Ordering::Relaxed), h.chat_id.clone()))
                .collect()
        };
        by_age.sort_unstable();
        for (_, chat_id) in by_age {
            let (count, estimate) = {
                let handles = lock(&self.inner.handles);
                (
                    handles.len(),
                    handles
                        .values()
                        .map(|h| h.resident_estimate())
                        .sum::<usize>(),
                )
            };
            if count <= WARM_DOC_CAP && estimate <= commutator_doc::DOC_LRU_BYTE_BUDGET {
                return false;
            }
            let evicted = {
                let mut handles = lock(&self.inner.handles);
                match handles.get(&chat_id) {
                    Some(handle) if !self.pinned(handle) && Arc::strong_count(handle) == 1 => {
                        handles.remove(&chat_id)
                    }
                    _ => None,
                }
            };
            if let Some(handle) = evicted {
                // Final flush outside the map lock; ≤1s of changes could be
                // pending in the snapshot debounce.
                self.save_snapshot_compacted(&handle);
                tracing::debug!(chat = %handle.chat_id, "doc evicted (LRU)");
            }
        }
        let handles = lock(&self.inner.handles);
        handles.len() > WARM_DOC_CAP
            || handles.values().map(|h| h.resident_estimate()).sum::<usize>()
                > commutator_doc::DOC_LRU_BYTE_BUDGET
    }

    fn pinned(&self, handle: &Arc<ChatDocHandle>) -> bool {
        if handle.sync_started.load(Ordering::Acquire) {
            return true;
        }
        // Durable batches may outlive this handle. Only failed disk writes
        // require retaining the in-memory copy until persistence recovers.
        if handle.publication_failed.load(Ordering::Acquire) {
            return true;
        }
        if handle.messages_tx.receiver_count() > 0 || handle.queue_tx.receiver_count() > 0 {
            return true;
        }
        // The handle itself holds one doc ref; more means a live writer.
        if Arc::strong_count(&handle.doc) > 1 {
            return true;
        }
        if self.is_host(&handle.chat_id) {
            let is_processed = |id: &str| self.inner.store.is_processed(id).unwrap_or(false);
            match handle.doc.read_commands() {
                Ok(commands) => commands
                    .iter()
                    .any(|c| c.status == SessionCommandStatus::Pending && !is_processed(&c.id)),
                // Unreadable ledger: keep the doc, never evict blind.
                Err(_) => true,
            }
        } else {
            false
        }
    }

    /// Probe every open chat's room (window-focus liveness sweep). Each
    /// room ignores the hint unless it has been broadcast-quiet ≥30s.
    pub fn probe_open_chats(&self) {
        let handles: Vec<Arc<ChatDocHandle>> =
            lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            // chat2 rooms verify liveness on user signals — a
            // deaf-but-ponging DO otherwise freezes a watched transcript
            // for the whole background probe quiet window.
            if let Some(chat2) = lock(&handle.chat2).as_ref() {
                chat2.probe();
            }
        }
    }

    /// The in-flight queued-attachment transfer set: current entries first,
    /// then a fresh snapshot per landed chunk (see `push_attachments`).
    pub fn watch_transfers(&self) -> watch::Receiver<Vec<commutator_proto::TransferProgress>> {
        self.inner.transfers.subscribe()
    }

    /// Publish one transfer's progress (upserted by uploadId).
    fn transfer_progress_set(&self, upload_id: &str, file_name: &str, done: u64, total: u64) {
        self.inner.transfers.send_modify(|list| {
            match list.iter_mut().find(|t| t.upload_id == upload_id) {
                Some(t) => {
                    t.done = done;
                    t.total = total;
                }
                None => list.push(commutator_proto::TransferProgress {
                    upload_id: upload_id.to_string(),
                    file_name: file_name.to_string(),
                    done,
                    total,
                }),
            }
        });
    }

    /// Retire a transfer's progress entry (commit landed, or the attempt
    /// failed and the retry will re-publish).
    fn transfer_progress_clear(&self, upload_id: &str) {
        self.inner.transfers.send_modify(|list| {
            list.retain(|t| t.upload_id != upload_id);
        });
    }

    /// The connectivity stream: chats live on this device, so it is the
    /// local posture for good.
    pub fn watch_connectivity(&self) -> watch::Receiver<commutator_proto::Connectivity> {
        self.inner
            .connectivity
            .get_or_init(|| watch::channel(commutator_proto::Connectivity::default()).0)
            .subscribe()
    }

    pub fn chat_sync_state(&self, chat_id: &str) -> commutator_proto::ChatSyncState {
        use commutator_proto::ChatSyncState as S;
        let handle = lock(&self.inner.handles).get(chat_id).cloned();
        let Some(handle) = handle else {
            return S::Local;
        };
        if handle.publication_failed.load(Ordering::Acquire) {
            return S::StorageError;
        }
        S::Local
    }

    pub fn sync_resources(&self) -> serde_json::Value {
        let handles = lock(&self.inner.handles);
        let mut reasons =
            serde_json::json!({"views":0,"writers":0,"storageFailures":0,"connections":0});
        let mut waiting = 0usize;
        let mut oldest = 0i64;
        for h in handles.values() {
            if h.messages_tx.receiver_count() > 0 || h.queue_tx.receiver_count() > 0 {
                reasons["views"] = (reasons["views"].as_u64().unwrap() + 1).into();
            }
            if h.writers.load(Ordering::Acquire) > 0 {
                reasons["writers"] = (reasons["writers"].as_u64().unwrap() + 1).into();
            }
            if h.publication_failed.load(Ordering::Acquire) {
                reasons["storageFailures"] =
                    (reasons["storageFailures"].as_u64().unwrap() + 1).into();
            }
            if h.sync_started.load(Ordering::Acquire) {
                reasons["connections"] = (reasons["connections"].as_u64().unwrap() + 1).into();
            } else if h.sync_requested.load(Ordering::Acquire) {
                waiting += 1;
                oldest = oldest.max(now_ms() - h.last_access.load(Ordering::Relaxed));
            }
        }
        #[cfg(target_os = "linux")]
        let open_fds = std::fs::read_dir("/proc/self/fd")
            .ok()
            .map(|fds| fds.count());
        #[cfg(not(target_os = "linux"))]
        let open_fds: Option<usize> = None;
        #[cfg(unix)]
        let fd_limit = unsafe {
            let mut limit: libc::rlimit = std::mem::zeroed();
            (libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0).then_some(limit.rlim_cur as u64)
        };
        #[cfg(not(unix))]
        let fd_limit: Option<u64> = None;
        serde_json::json!({
            "budget": commutator_sync::budget::shared().stats(),
            "activeClientLimit": ACTIVE_SYNC_CAP,
            "openDocuments": handles.len(), "waitingDocuments": waiting,
            "documentLoads": self.inner.document_loads.load(Ordering::Relaxed),
            "oldestWaitingAccessAgeMs": oldest, "retainedBy": reasons,
            "openFileDescriptors": open_fds, "fileDescriptorLimit": fd_limit,
            "durable": self.inner.store.sync_work_counts().ok().map(|(batches, jobs)| serde_json::json!({"pendingBatches": batches, "pendingJobs": jobs})),
        })
    }

    /// Per-open-chat room introspection for SyncStatus / `commutator sync`.
    /// `None` room = still dialing (join retry loop) or edge-less.
    pub fn sync_statuses(&self) -> Vec<(String, Option<commutator_sync::ChatStatsSnapshot>)> {
        let handles: Vec<Arc<ChatDocHandle>> =
            lock(&self.inner.handles).values().cloned().collect();
        let mut rows: Vec<(String, Option<commutator_sync::ChatStatsSnapshot>)> = handles
            .iter()
            .map(|h| {
                (
                    h.chat_id.clone(),
                    lock(&h.chat2).as_ref().map(|client| client.stats()),
                )
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }

    /// Drop a chat's doc unconditionally and delete its local snapshot — the
    /// chat is gone (DeleteChat / DeleteSpace cascade). Watchers see the
    /// stream end; a racing writer keeps its orphaned doc until the run ends.
    pub fn purge_chat(&self, chat_id: &str) {
        let removed = lock(&self.inner.handles).remove(chat_id);
        drop(removed);
        if let Err(err) = self.inner.store.delete_snapshot(chat_id) {
            tracing::warn!(chat = %chat_id, error = %err, "snapshot delete failed");
        }
    }

    /// Composer path: append an immutable pending command entry (rule 1). Durable by
    /// construction — the change subscription kicks the drain, so a local host executes
    /// immediately and an offline doc simply holds the entry until it syncs.
    pub fn queue_command(
        &self,
        chat_id: &str,
        payload: SessionCommandPayload,
    ) -> Result<String, EngineError> {
        self.queue_command_with_transfers(chat_id, payload, Vec::new())
    }

    /// [`Self::queue_command`] plus queued-attachment transfers: the command's
    /// `pending://` refs name bytes already committed to THIS device's uploads
    /// dir; when another device hosts the chat, a background task pushes them
    /// over the peer link (retry until they land) while the command is
    /// already durably queued. The send is a local write — attachment bytes
    /// chase it, never gate it (2026-08-19 incident).
    pub fn queue_command_with_transfers(
        &self,
        chat_id: &str,
        payload: SessionCommandPayload,
        transfers: Vec<crate::uploads::AttachmentTransfer>,
    ) -> Result<String, EngineError> {
        tracing::debug!(target: "first_word", chat = %chat_id, "queue command");
        let handle = self.open(chat_id)?;
        // A chat hosted on Cloud: the bytes go to the edge's store first, and
        // the message enters the chat only once they're there.
        if matches!(
            payload,
            SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
        ) && let Some(id) = self.hold_if_needed(chat_id, &payload, &transfers)?
        {
            self.unarchive_on_send(chat_id);
            return Ok(id);
        }
        let id = new_id();
        let now = now_ms();
        let based_on = handle.doc.read_entries_shared()?.last().map(|m| CommandBasedOn {
            turn_id: Some(m.id.clone()),
            frontier: None,
        });
        let is_message = matches!(
            payload,
            SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
        );
        let entry = SessionCommandEntry {
            id: id.clone(),
            payload,
            issued_by: self.inner.config.device_id.clone(),
            issued_at: now,
            based_on,
            expires_at: Some(now + commutator_doc::COMMAND_DEFAULT_TTL_MS),
            status: SessionCommandStatus::Pending,
            resolution: None,
        };
        handle.doc.queue_command(&entry)?;
        // Sending a message revives an archived chat: the user is acting in it
        // again, so the LWW row flips back to active on every device. Best-
        // effort — the command itself is durable regardless.
        if is_message {
            self.unarchive_on_send(chat_id);
        }
        // The cold host must discover the command even after its room rows
        // are ACKed. Persist that wake independently and recover it on restart.
        self.spawn_command_delivery(chat_id, entry, transfers);
        Ok(id)
    }

    /// A send revives an archived chat on every device (best-effort).
    fn unarchive_on_send(&self, chat_id: &str) {
        let Some(workspace) = self.workspace() else {
            return;
        };
        match workspace.chat(chat_id) {
            Ok(Some(chat)) if chat.archived => {
                if let Err(err) = workspace.set_chat_archived(chat_id, false) {
                    tracing::warn!(chat = %chat_id, error = %err, "unarchive on send failed");
                }
            }
            _ => {}
        }
    }

    /// Hold a message for later: append it to the doc's queue. Any device may
    /// write here (unlike `messages`), and the change subscription kicks
    /// [`Self::drain_queue`], so a queue that lands while the agent is already
    /// idle goes straight out instead of waiting for a turn that never comes.
    pub fn queue_message(
        &self,
        chat_id: &str,
        text: &str,
        attachments: Vec<String>,
    ) -> Result<String, EngineError> {
        self.queue_message_with_behavior(chat_id, text, attachments, false)
    }

    /// Append a message while preserving the submitter's active-turn policy
    /// on the synchronized row. This matters when another device hosts the
    /// chat: the host, not the submitting UI, decides when to drain it.
    pub fn queue_message_with_behavior(
        &self,
        chat_id: &str,
        text: &str,
        attachments: Vec<String>,
        hold_for_turn_end: bool,
    ) -> Result<String, EngineError> {
        self.queue_message_with_transfers(
            chat_id,
            text,
            attachments,
            hold_for_turn_end,
            Vec::new(),
        )
    }

    /// [`Self::queue_message_with_behavior`] plus the row's `pending://` bytes,
    /// already committed to THIS device's uploads dir: a Cloud-hosted chat's
    /// row waits until they're in the edge's store, like a command does.
    pub fn queue_message_with_transfers(
        &self,
        chat_id: &str,
        text: &str,
        attachments: Vec<String>,
        hold_for_turn_end: bool,
        transfers: Vec<crate::uploads::AttachmentTransfer>,
    ) -> Result<String, EngineError> {
        self.queue_message_with_config(
            chat_id,
            text,
            attachments,
            hold_for_turn_end,
            transfers,
            None,
        )
    }

    /// [`Self::queue_message_with_transfers`] plus an optional `ChatConfig`
    /// selected when the message was queued (e.g., switching provider or model
    /// while the previous turn is still running).
    pub fn queue_message_with_config(
        &self,
        chat_id: &str,
        text: &str,
        attachments: Vec<String>,
        hold_for_turn_end: bool,
        transfers: Vec<crate::uploads::AttachmentTransfer>,
        config: Option<commutator_proto::ChatConfig>,
    ) -> Result<String, EngineError> {
        let handle = self.open(chat_id)?;
        let id = new_id();
        let item = QueuedMessage {
            id: id.clone(),
            text: queued_row_text(text, !attachments.is_empty()).to_string(),
            attachments,
            hold_for_turn_end,
            issued_by: self.inner.config.device_id.clone(),
            issued_at: now_ms(),
            edited_at: None,
            delivery_gate: None,
            config,
        };
        if self.hold_row_if_needed(chat_id, &item, &transfers)? {
            self.unarchive_on_send(chat_id);
            return Ok(id);
        }
        handle.doc.push_queued(&item)?;
        handle.publish_queue();
        self.spawn_row_transfers(chat_id, transfers);
        // Same reasoning as a command: the user is acting in this chat again.
        self.unarchive_on_send(chat_id);
        Ok(id)
    }

    /// Retype a queued message. Empty text deletes the row — emptying the box
    /// is how you say "drop it". `false` when the row is already gone.
    pub fn update_queued_message(
        &self,
        chat_id: &str,
        id: &str,
        text: &str,
    ) -> Result<bool, EngineError> {
        let handle = self.open(chat_id)?;
        let changed = handle.doc.set_queued_text(id, text, now_ms())?;
        if changed {
            handle.publish_queue();
        }
        Ok(changed)
    }

    /// Reorder a queued message (drag, or the up/down buttons).
    pub fn move_queued_message(
        &self,
        chat_id: &str,
        id: &str,
        to_index: usize,
    ) -> Result<bool, EngineError> {
        let handle = self.open(chat_id)?;
        let changed = handle.doc.move_queued(id, to_index)?;
        if changed {
            handle.publish_queue();
        }
        Ok(changed)
    }

    /// Cancel one queued message at the chat host. Removal shares the same
    /// lock as automatic and explicit delivery, so the acknowledgement is the
    /// linearization point: `true` guarantees this host did not take the row.
    pub async fn remove_queued_message(
        &self,
        chat_id: &str,
        id: &str,
    ) -> Result<bool, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        let _drain = handle.drain_lock.lock().await;
        let removed = handle.doc.remove_queued(id)?;
        if removed {
            handle.publish_queue();
        }
        Ok(removed)
    }

    /// Acquire the host-side right to edit one queued row. This operation and
    /// every queue take share `drain_lock`, making the ACK the linearization
    /// point: after Acquired the row cannot race into the agent.
    pub async fn begin_queued_message_edit(
        &self,
        chat_id: &str,
        id: &str,
        owner_device_id: &str,
        owner_instance_id: &str,
    ) -> Result<BeginQueueEditOutcome, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        let _drain = handle.drain_lock.lock().await;
        let now = now_ms();
        let Some(item) = handle
            .doc
            .read_queue()?
            .into_iter()
            .find(|item| item.id == id)
        else {
            return Ok(BeginQueueEditOutcome::Missing);
        };
        // The same editor asking again (its Acquired reply was lost to a
        // dropped app socket, or it reopened the chat) takes its own lease
        // over: refusing it locked a person out of their own message for a
        // whole lease, told it was "being edited on another device".
        if let Some(QueueDeliveryGate::Editing {
            owner_device_id: held_device,
            owner_instance_id: held_instance,
            expires_at_ms,
            ..
        }) = &item.delivery_gate
            && *expires_at_ms > now
            && !(held_device == owner_device_id && held_instance == owner_instance_id)
        {
            return Ok(BeginQueueEditOutcome::Locked {
                owner_device_id: held_device.clone(),
                expires_at_ms: *expires_at_ms,
            });
        }

        let lease_id = new_id();
        let expires_at_ms = now + QUEUE_EDIT_LEASE_MS;
        let gate = QueueDeliveryGate::Editing {
            lease_id: lease_id.clone(),
            owner_device_id: owner_device_id.to_string(),
            owner_instance_id: owner_instance_id.to_string(),
            acquired_at_ms: now,
            expires_at_ms,
            base_text_hash: queue_text_hash(&item.text),
        };
        let base_text_hash = queue_text_hash(&item.text);
        if !handle.doc.set_queued_delivery_gate(id, Some(&gate))? {
            return Ok(BeginQueueEditOutcome::Missing);
        }
        handle.publish_queue();
        // A crash immediately after the client sees Acquired must not reopen
        // the row as sendable from a pre-lease snapshot.
        self.save_snapshot(&handle);
        self.arm_queue_edit_expiry(&handle, id, &lease_id, expires_at_ms);
        Ok(BeginQueueEditOutcome::Acquired {
            lease_id,
            text: item.text,
            attachments: item.attachments,
            base_text_hash,
            expires_at_ms,
        })
    }

    /// Extend an edit lease. An already-expired generation is never revived;
    /// it remains blocked and will be surfaced as ReviewRequired.
    pub async fn renew_queued_message_edit(
        &self,
        chat_id: &str,
        id: &str,
        lease_id: &str,
    ) -> Result<RenewQueueEditOutcome, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        let _drain = handle.drain_lock.lock().await;
        let now = now_ms();
        let Some(item) = handle
            .doc
            .read_queue()?
            .into_iter()
            .find(|item| item.id == id)
        else {
            return Ok(RenewQueueEditOutcome::Missing);
        };
        let QueueDeliveryGate::Editing {
            lease_id: current,
            owner_device_id,
            owner_instance_id,
            acquired_at_ms,
            expires_at_ms,
            base_text_hash,
        } = item
            .delivery_gate
            .unwrap_or(QueueDeliveryGate::ReviewRequired {
                previous_lease_id: String::new(),
                owner_device_id: String::new(),
                since_ms: now,
                base_text_hash: String::new(),
            })
        else {
            return Ok(RenewQueueEditOutcome::Lost);
        };
        if current != lease_id || expires_at_ms <= now {
            if current == lease_id && expires_at_ms <= now {
                let review = QueueDeliveryGate::ReviewRequired {
                    previous_lease_id: current,
                    owner_device_id,
                    since_ms: now,
                    base_text_hash,
                };
                let _ = handle.doc.set_queued_delivery_gate(id, Some(&review));
                handle.publish_queue();
                self.save_snapshot(&handle);
            }
            return Ok(RenewQueueEditOutcome::Lost);
        }
        let expires_at_ms = now + QUEUE_EDIT_LEASE_MS;
        let renewed = QueueDeliveryGate::Editing {
            lease_id: current,
            owner_device_id,
            owner_instance_id,
            acquired_at_ms,
            expires_at_ms,
            base_text_hash,
        };
        if !handle.doc.set_queued_delivery_gate(id, Some(&renewed))? {
            return Ok(RenewQueueEditOutcome::Missing);
        }
        handle.publish_queue();
        self.arm_queue_edit_expiry(&handle, id, lease_id, expires_at_ms);
        Ok(RenewQueueEditOutcome::Renewed { expires_at_ms })
    }

    /// Resolve an edit lease. A late finish may still resolve the matching
    /// ReviewRequired generation, but can never affect a newer lease.
    pub async fn finish_queued_message_edit(
        &self,
        chat_id: &str,
        id: &str,
        lease_id: &str,
        action: FinishQueueEditAction,
        text: Option<&str>,
        expected_text_hash: Option<&str>,
    ) -> Result<FinishQueueEditOutcome, EngineError> {
        self.finish_queued_message_edit_with_attachments(
            chat_id,
            id,
            lease_id,
            action,
            text,
            expected_text_hash,
            None,
        )
        .await
    }

    pub async fn finish_queued_message_edit_with_attachments(
        &self,
        chat_id: &str,
        id: &str,
        lease_id: &str,
        action: FinishQueueEditAction,
        text: Option<&str>,
        expected_text_hash: Option<&str>,
        attachments: Option<&[String]>,
    ) -> Result<FinishQueueEditOutcome, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        if action == FinishQueueEditAction::Commit
            && (text.is_none() || expected_text_hash.is_none())
        {
            return Err(EngineError::Other(
                "commit requires text and expectedTextHash".into(),
            ));
        }
        let handle = self.open(chat_id)?;
        let outcome = {
            let _drain = handle.drain_lock.lock().await;
            let Some(item) = handle
                .doc
                .read_queue()?
                .into_iter()
                .find(|item| item.id == id)
            else {
                return Ok(FinishQueueEditOutcome::Missing);
            };
            let (current_lease, base_text_hash) = match &item.delivery_gate {
                Some(QueueDeliveryGate::Editing {
                    lease_id,
                    base_text_hash,
                    ..
                }) => (lease_id, base_text_hash),
                Some(QueueDeliveryGate::ReviewRequired {
                    previous_lease_id,
                    base_text_hash,
                    ..
                }) => (previous_lease_id, base_text_hash),
                None => return Ok(FinishQueueEditOutcome::Lost),
            };
            if current_lease != lease_id {
                return Ok(FinishQueueEditOutcome::Lost);
            }
            if action == FinishQueueEditAction::Commit
                && (expected_text_hash != Some(base_text_hash.as_str())
                    || queue_text_hash(&item.text) != *base_text_hash)
            {
                return Ok(FinishQueueEditOutcome::Conflict {
                    current_text: item.text,
                });
            }

            let replacement = match action {
                FinishQueueEditAction::Commit => Some(queued_row_text(
                    text.unwrap_or_default(),
                    attachments.map_or(!item.attachments.is_empty(), |kept| !kept.is_empty()),
                )),
                FinishQueueEditAction::Cancel | FinishQueueEditAction::ReleaseUnchanged => None,
                FinishQueueEditAction::Discard => Some(""),
            };
            if !handle.doc.finish_queued_edit_with_attachments(
                id,
                replacement,
                if action == FinishQueueEditAction::Commit {
                    attachments
                } else {
                    None
                },
                now_ms(),
            )? {
                return Ok(FinishQueueEditOutcome::Missing);
            }
            handle.publish_queue();
            self.save_snapshot(&handle);
            match action {
                FinishQueueEditAction::Commit => FinishQueueEditOutcome::Committed,
                FinishQueueEditAction::Cancel => FinishQueueEditOutcome::Cancelled,
                FinishQueueEditAction::Discard => FinishQueueEditOutcome::Discarded,
                FinishQueueEditAction::ReleaseUnchanged => FinishQueueEditOutcome::Released,
            }
        };
        // Turn-end may already have happened while the edit was open.
        self.drain_queue(&handle).await;
        Ok(outcome)
    }

    fn arm_existing_queue_edit_expiries(&self, handle: &Arc<ChatDocHandle>) {
        let Ok(queue) = handle.doc.read_queue() else {
            return;
        };
        for item in queue {
            if let Some(QueueDeliveryGate::Editing {
                lease_id,
                expires_at_ms,
                ..
            }) = item.delivery_gate
            {
                self.arm_queue_edit_expiry(handle, &item.id, &lease_id, expires_at_ms);
            }
        }
    }

    fn arm_queue_edit_expiry(
        &self,
        handle: &Arc<ChatDocHandle>,
        id: &str,
        lease_id: &str,
        expires_at_ms: i64,
    ) {
        let delay_ms = expires_at_ms.saturating_sub(now_ms()).max(0) as u64;
        let host = self.clone();
        let handle = handle.clone();
        let id = id.to_string();
        let lease_id = lease_id.to_string();
        self.spawn_worker(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            host.expire_queued_message_edit(&handle, &id, &lease_id, expires_at_ms)
                .await;
        });
    }

    /// Expire exactly the lease generation that scheduled this wake. A stale
    /// timer from before a renewal observes a different deadline and no-ops;
    /// timers for other rows are completely independent.
    async fn expire_queued_message_edit(
        &self,
        handle: &Arc<ChatDocHandle>,
        id: &str,
        lease_id: &str,
        scheduled_expires_at_ms: i64,
    ) {
        let changed = {
            let _drain = handle.drain_lock.lock().await;
            let Ok(queue) = handle.doc.read_queue() else {
                return;
            };
            let Some(item) = queue.into_iter().find(|item| item.id == id) else {
                return;
            };
            let Some(QueueDeliveryGate::Editing {
                lease_id: current_lease_id,
                owner_device_id,
                expires_at_ms,
                base_text_hash,
                ..
            }) = item.delivery_gate
            else {
                return;
            };
            if current_lease_id != lease_id
                || expires_at_ms != scheduled_expires_at_ms
                || expires_at_ms > now_ms()
            {
                return;
            }
            let review = QueueDeliveryGate::ReviewRequired {
                previous_lease_id: current_lease_id,
                owner_device_id,
                since_ms: now_ms(),
                base_text_hash,
            };
            let Ok(changed) = handle.doc.set_queued_delivery_gate(id, Some(&review)) else {
                return;
            };
            if changed {
                handle.publish_queue();
                self.save_snapshot(handle);
            }
            changed
        };
        if changed {
            // If this was the head, the drain now observes ReviewRequired. If
            // it was not, publishing still updates every client's row state.
            self.drain_queue(handle).await;
        }
    }

    /// "Send this one now": take it out of the queue and put it in front of the
    /// agent, interrupting whatever is running. Deliberately blunt — it is the
    /// explicit override. The empty-composer Enter gesture reaches this path
    /// only when the selected provider cannot steer the row. `false` when
    /// another device already took it.
    pub async fn send_queued_now(&self, chat_id: &str, id: &str) -> Result<bool, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        // Sending one now sends ONE: the lock keeps the flush out of the idle
        // window the interrupt opens, and out of the take itself.
        let _drain = handle.drain_lock.lock().await;
        let Some(candidate) = handle
            .doc
            .read_queue()?
            .into_iter()
            .find(|item| item.id == id)
        else {
            return Ok(false);
        };
        if candidate.delivery_gate.is_some() {
            return Err(EngineError::Other(
                "queued message is blocked for editing or review".into(),
            ));
        }
        let Some(item) = handle.doc.take_queued(id)? else {
            return Ok(false);
        };
        let was_paused = handle.queue_paused.swap(false, Ordering::AcqRel);
        handle.publish_queue();
        if let Err(err) = self
            .dispatch_queued(&handle, &item, QueueSend::Interrupt)
            .await
        {
            if was_paused {
                handle.queue_paused.store(true, Ordering::Release);
            }
            // Put it back rather than swallowing what the user typed — at the
            // head, because the user just said this one was the urgent one.
            let _ = handle.doc.insert_queued(0, &item);
            handle.publish_queue();
            return Err(err);
        }
        Ok(true)
    }

    /// Promote one held row without ever interrupting a turn. A live,
    /// steerable turn receives it as steering; if that turn has already ended,
    /// it starts normally as the next turn. Turn-boundary providers retain it
    /// in their mailbox until ready. A row's files go with it as the steer's
    /// attachments (images as images); one whose files from another device
    /// haven't landed yet stays queued, and the call says why.
    pub async fn steer_queued_now(&self, chat_id: &str, id: &str) -> Result<bool, EngineError> {
        if !self.is_host(chat_id) {
            return Err(EngineError::Other(format!(
                "device {} does not host chat {chat_id}",
                self.inner.config.device_id
            )));
        }
        let handle = self.open(chat_id)?;
        let _drain = handle.drain_lock.lock().await;
        let Some(candidate) = handle
            .doc
            .read_queue()?
            .into_iter()
            .find(|item| item.id == id)
        else {
            return Ok(false);
        };
        // Its files ride the steer, as images where they are. Ones still on
        // their way from another device can't go yet (the drain waits for
        // them; a steer says so).
        if !self.missing_row_attachments(&candidate).is_empty() {
            return Err(EngineError::Other(
                "this message's images are still uploading".into(),
            ));
        }
        if candidate.delivery_gate.is_some() {
            return Err(EngineError::Other(
                "queued message is blocked for editing or review".into(),
            ));
        }
        if self.sessions().is_some_and(|sessions| {
            sessions.defers_to_turn_end(chat_id, None)
                || candidate
                    .config
                    .as_ref()
                    .is_some_and(|cfg| !sessions.live_run_matches_config(chat_id, cfg))
        }) {
            // Send next: lead the ordinary rows; the drain delivers it the
            // moment the current turn ends (and applies its target config).
            let Some(item) = handle.doc.take_queued(id)? else {
                return Ok(false);
            };
            handle
                .doc
                .insert_queued(handle.steer_slot(&item.id)?, &item)?;
            handle.queue_paused.store(false, Ordering::Release);
            handle.publish_queue();
            return Ok(true);
        }
        let Some(item) = handle.doc.take_queued(id)? else {
            return Ok(false);
        };
        let was_paused = handle.queue_paused.swap(false, Ordering::AcqRel);
        handle.publish_queue();
        // Always attempt the non-interrupting path first. If no turn exists,
        // `dispatch_queued` falls through to NextTurn; if a new turn appeared
        // after our capability check, the prompt steers that turn instead.
        if let Err(err) = self.dispatch_queued(&handle, &item, QueueSend::Steer).await {
            if was_paused {
                handle.queue_paused.store(true, Ordering::Release);
            }
            let _ = handle.doc.insert_queued(0, &item);
            handle.publish_queue();
            return Err(err);
        }
        Ok(true)
    }

    /// Host-only: hand queued messages to the agent when there is somewhere to
    /// put them.
    ///
    /// - Idle: send the head as the next turn (and loop — the agent is free).
    /// - Turn in flight: hold. The turn-end watcher comes back for it.
    ///
    /// One at a time by design: each send changes the status this reads.
    pub async fn drain_queue(&self, handle: &Arc<ChatDocHandle>) {
        let Some(_dispatch) = self.dispatch_guard() else {
            return; // work is held; lifting it re-drains
        };
        let Some(sessions) = self.sessions() else {
            return; // executor not wired yet; the set_sessions kick re-drains
        };
        if !self.is_host(&handle.chat_id) {
            return;
        }
        // One drain at a time per chat. Waiters are cheap: whoever takes the
        // lock next re-reads the queue and the status, so a drain that became
        // unnecessary while it waited simply finds nothing to do.
        let _drain = handle.drain_lock.lock().await;
        if handle.queue_paused.load(Ordering::Acquire) {
            return;
        }
        loop {
            let Ok(Some(head)) = handle.doc.read_queue().map(|q| q.into_iter().next()) else {
                return;
            };
            match &head.delivery_gate {
                Some(QueueDeliveryGate::Editing {
                    lease_id,
                    owner_device_id,
                    expires_at_ms,
                    base_text_hash,
                    ..
                }) if *expires_at_ms <= now_ms() => {
                    let review = QueueDeliveryGate::ReviewRequired {
                        previous_lease_id: lease_id.clone(),
                        owner_device_id: owner_device_id.clone(),
                        since_ms: now_ms(),
                        base_text_hash: base_text_hash.clone(),
                    };
                    let _ = handle.doc.set_queued_delivery_gate(&head.id, Some(&review));
                    handle.publish_queue();
                    self.save_snapshot(handle);
                    return;
                }
                Some(QueueDeliveryGate::Editing {
                    lease_id,
                    expires_at_ms,
                    ..
                }) => {
                    self.arm_queue_edit_expiry(handle, &head.id, lease_id, *expires_at_ms);
                    return;
                }
                Some(QueueDeliveryGate::ReviewRequired { .. }) => return,
                None => {}
            }
            // In flight, not just Working: an agent parked on a question owns
            // the turn too, and the composer queues on the same reading. Taking
            // `AwaitingInput` for idle would send the follow-up as a fresh turn
            // and abandon the question.
            if sessions.turn_in_flight(&handle.chat_id) {
                return; // All queued messages wait, including rows from older clients.
            }
            // A row from another device names its images by `pending://` ref
            // while the bytes chase it over the peer link. Hold it (in order)
            // until they land — UploadCommit re-drains — rather than handing
            // the agent refs it cannot open.
            let missing = self.missing_row_attachments(&head);
            if !missing.is_empty() {
                tracing::info!(chat = %handle.chat_id, row = %head.id,
                    "queued row held: attachment bytes in transit");
                self.arm_attachment_wait(handle);
                return;
            }
            let send = QueueSend::NextTurn;
            // Take it only once we know it is going out — a row that stays in
            // the queue on a failed send is recoverable; a vanished one is not.
            let Ok(Some(item)) = handle.doc.take_queued(&head.id) else {
                return;
            };
            lock(&handle.steered_rows).retain(|row| row != &item.id);
            handle.publish_queue();
            if let Err(err) = self.dispatch_queued(handle, &item, send).await {
                tracing::warn!(chat = %handle.chat_id, error = %err, "queued send failed");
                handle.queue_paused.store(true, Ordering::Release);
                let _ = handle.doc.insert_queued(0, &item);
                handle.publish_queue();
                return;
            }
        }
    }

    /// A Stop acts on everything sent before it, in the order the user sent
    /// it. Controls bypass the prompt drain (so a stalled provider stays
    /// stoppable), which let a Stop execute before a send queued ahead of it
    /// had dispatched: with no turn yet it did nothing and the turn then ran
    /// in full; mid-turn, the earlier steer landed after the Stop and read as
    /// the user's next message, re-running in the fresh runtime. Wait for
    /// earlier prompts to dispatch (milliseconds) — bounded short while a
    /// turn is in flight, so a provider wedged on backpressure still stops.
    async fn await_earlier_prompts(
        &self,
        sessions: &SessionsEngine,
        handle: &Arc<ChatDocHandle>,
        stop_id: &str,
    ) {
        let started = tokio::time::Instant::now();
        loop {
            let bound = if sessions.turn_in_flight(&handle.chat_id) {
                std::time::Duration::from_secs(2)
            } else {
                std::time::Duration::from_secs(15)
            };
            if started.elapsed() >= bound {
                return;
            }
            let Ok(commands) = handle.doc.read_commands() else {
                return;
            };
            let Some(at) = commands.iter().position(|c| c.id == stop_id) else {
                return;
            };
            let earlier_prompt = commands[..at].iter().any(|c| {
                c.status == SessionCommandStatus::Pending
                    && matches!(
                        c.payload,
                        SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
                    )
            });
            if !earlier_prompt {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// A prompt queued after a control that is still executing (a Stop
    /// tearing the runtime down) waits for it: running ahead of it put the
    /// user's next message into the runtime the Stop was killing.
    async fn await_earlier_controls(&self, handle: &Arc<ChatDocHandle>, prompt_id: &str) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        while tokio::time::Instant::now() < deadline {
            let Ok(commands) = handle.doc.read_commands() else {
                return;
            };
            let Some(at) = commands.iter().position(|c| c.id == prompt_id) else {
                return;
            };
            let earlier_control = {
                let executing = lock(&self.inner.executing);
                commands[..at].iter().any(|c| {
                    c.status == SessionCommandStatus::Pending
                        && !matches!(
                            c.payload,
                            SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
                        )
                        && executing.contains(&c.id)
                })
            };
            if !earlier_control {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// Stop the active turn without treating the resulting Idle transition as
    /// permission to release the next queued message. The same lock used by
    /// drains closes the race between clicking Cancel and the status watcher.
    /// A parked runtime still running background work is stopped too: the
    /// idle reaper spares it, so Stop is what ends that work.
    async fn interrupt_and_pause_queue(
        &self,
        sessions: &SessionsEngine,
        handle: &Arc<ChatDocHandle>,
    ) -> Result<bool, EngineError> {
        let _drain = handle.drain_lock.lock().await;
        if !sessions.turn_in_flight(&handle.chat_id)
            && !sessions.holds_background_work(&handle.chat_id)
        {
            return Ok(false);
        }
        handle.queue_paused.store(true, Ordering::Release);
        self.cancel_held_sends(handle);
        match sessions.interrupt(&handle.chat_id).await {
            Ok(true) => Ok(true),
            Ok(false) => {
                handle.queue_paused.store(false, Ordering::Release);
                Ok(false)
            }
            Err(err) => {
                handle.queue_paused.store(false, Ordering::Release);
                Err(err)
            }
        }
    }

    /// A Stop holds back the messages sent during the turn it stops: the
    /// sends and steers waiting for that turn's end stay in the queue, as
    /// the person's own rows do, paused with them — never to start the
    /// stopped work again by themselves, and never out of reach (sent, edited
    /// or removed there like any other row). They're no longer the turn's:
    /// a row goes on its own once the person sends again.
    fn cancel_held_sends(&self, handle: &Arc<ChatDocHandle>) {
        lock(&handle.held_sends).clear();
        lock(&handle.steered_rows).clear();
        handle.publish_queue();
    }

    /// Send one taken queue row.
    async fn dispatch_queued(
        &self,
        handle: &Arc<ChatDocHandle>,
        item: &QueuedMessage,
        send: QueueSend,
    ) -> Result<(), EngineError> {
        let Some(sessions) = self.sessions() else {
            return Err(EngineError::Other("sessions engine not wired".into()));
        };
        let chat_id = &handle.chat_id;
        // Keep the queue row's identity when it becomes a real user message.
        // The submitting viewport learns this id from QueueMessage and can
        // therefore wait without disturbing the active turn's runway, then
        // anchor the prompt only once this exact row reaches the transcript.
        let message_id = item.id.clone();
        if !self.missing_row_attachments(item).is_empty() {
            return Err(EngineError::Other(
                "this message's images are still uploading".into(),
            ));
        }
        let mut attachments = item.attachments.clone();
        // A send held for the turn end goes out exactly as it was sent: its
        // text never carried the queue's image note.
        let held = lock(&handle.held_sends).remove(&item.id);
        let mut prompt = if held.is_some() {
            item.text.clone()
        } else {
            queued_message_prompt(&item.text, &attachments)
        };
        self.resolve_attachment_refs(&mut prompt, &mut attachments);
        if send == QueueSend::Steer
            && item
                .config
                .as_ref()
                .is_none_or(|cfg| sessions.live_run_matches_config(chat_id, cfg))
        {
            match sessions
                .steer_now(
                    chat_id,
                    &prompt,
                    attachments.clone(),
                    Some(message_id.clone()),
                    now_ms(),
                )
                .await?
            {
                SteerOutcome::Accepted => return Ok(()),
                SteerOutcome::DeferredByUpdate => {
                    return Err(EngineError::Other(
                        "agent update pending; the message remains queued".into(),
                    ));
                }
                // The run died under us between the status read and the send;
                // fall through and start a fresh turn with it.
                SteerOutcome::NotSteerable => {}
            }
        }
        // Same reading of "busy" as the drain: a turn parked on a question is
        // still a turn, and it has to be stopped before this one starts.
        // The message replaces the turn, not the agent: its background work
        // keeps running (only the user's Stop tears that down).
        if send == QueueSend::Interrupt && sessions.turn_in_flight(chat_id) {
            sessions.stop_turn(chat_id).await?;
        }
        let previous = sessions.last_request(chat_id);
        // ... and on its model, effort and options, not the chat's saved ones.
        let request = held.or_else(|| {
            self.request_from_chat_row(chat_id, &prompt)
                .map(|mut current| {
                    if let Some(previous) = &previous {
                        current.auto_approve = previous.auto_approve;
                        current.worktree = previous.worktree.clone();
                    }
                    current
                })
                .or(previous)
        });
        let Some(mut request) = request else {
            return Err(EngineError::Other(
                "no live run and no prior run config".into(),
            ));
        };
        if let Some(cfg) = &item.config {
            request.harness = Some(cfg.harness);
            request.model = cfg.model.clone();
            request.reasoning = cfg.reasoning;
            request.model_options = cfg.model_options.clone();
        }
        request.prompt = prompt;
        request.resume = None; // dispatch re-derives the harness session
        request.attachments = attachments;
        let harness = self.harness_for_request(chat_id, &request);
        self.dispatch_with_source_context(&sessions, chat_id, harness, request, Some(message_id))
            .await?;
        if let Some(cfg) = &item.config
            && let Some(ws) = self.workspace()
        {
            let mut updated = ws.chat_config(chat_id).unwrap_or_else(|| cfg.clone());
            updated.harness = cfg.harness;
            updated.model = cfg.model.clone();
            updated.reasoning = cfg.reasoning;
            updated.model_options = cfg.model_options.clone();
            if let Err(err) = ws.set_chat_config(chat_id, &updated) {
                tracing::warn!(chat = %chat_id, error = %err, "queued row chat config update failed");
            }
        }
        Ok(())
    }

    /// The chat's host device when it is NOT this engine (mirrors
    /// chat row).
    fn remote_host_for(&self, chat_id: &str) -> Option<String> {
        let workspace = self.workspace()?;
        let host_device = match workspace.chat(chat_id) {
            Ok(Some(chat)) => chat.device_id,
            _ => return None,
        };
        (host_device != self.inner.config.device_id).then_some(host_device)
    }

    /// Durable-delivery escort for one queued command aimed at a REMOTE host:
    ///
    /// 1. push any queued attachment bytes over the peer link (retry until
    ///    they land — the relayed command must never outrun its bytes);
    /// 2. give the normal path (chat2 rows → edge → host's room) a short
    ///    grace to ack;
    /// 3. rows still not at the edge but the peer link alive → relay-forward
    ///    the entry itself ([`commutator_rpc::methods::RELAY_COMMAND`]). The
    ///    host's processed ledger claims the client-minted id, so the doc
    ///    row arriving later dedupes to a no-op — exactly-once by
    ///    construction (the 2026-08-18 03:45 incident shape: nudges flowed,
    ///    rows didn't; there was no second road for the command).
    ///
    /// Stops the moment any path lands. No-op for locally-hosted chats.
    fn spawn_command_delivery(
        &self,
        chat_id: &str,
        entry: SessionCommandEntry,
        transfers: Vec<crate::uploads::AttachmentTransfer>,
    ) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return; // bare sync callers (unit tests) skip rather than panic
        };
        let host = self.clone();
        let chat = chat_id.to_string();
        self.spawn_worker_on(&runtime, async move {
            let Some(target) = host.remote_host_for(&chat) else {
                return; // local host (or no row yet claimed remotely)
            };
            if !transfers.is_empty() && !host.deliver_attachments(&chat, &transfers).await {
                return; // gave up; the drain's wait cap surfaces the failure
            }
            let mut wake = commutator_sync::wake::subscribe();
            let mut online = commutator_sync::wake::subscribe_online();
            let give_up = tokio::time::Instant::now() + RELAY_GIVE_UP;
            let grace_end = tokio::time::Instant::now() + ROWS_GRACE;
            while tokio::time::Instant::now() < grace_end {
                if host.rows_flushed(&chat) {
                    return; // rows on the edge — the normal path has it
                }
                tokio::time::sleep(ROWS_POLL).await;
            }
            let mut backoff = RELAY_BACKOFF_BASE;
            loop {
                if host.rows_flushed(&chat) {
                    return; // the normal path won while we were retrying
                }
                match host.relay_command(&target, &chat, &entry).await {
                    Ok(outcome) => {
                        tracing::info!(chat = %chat, device = %target, command = %entry.id,
                            outcome, "command delivered via peer relay");
                        return;
                    }
                    Err(err) => {
                        tracing::warn!(chat = %chat, device = %target, error = %err,
                            backoff_ms = backoff.as_millis() as u64,
                            "peer-relay delivery retrying");
                    }
                }
                if tokio::time::Instant::now() >= give_up {
                    tracing::warn!(chat = %chat, command = %entry.id,
                        "peer-relay delivery gave up; command remains queued in the doc");
                    return;
                }
                while wake.try_recv().is_ok() {}
                while online.try_recv().is_ok() {}
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = wake.recv() => {}
                    _ = online.recv() => {}
                }
                backoff = (backoff * 2).min(RELAY_BACKOFF_CAP);
            }
        });
    }

    /// Step 1 of the escort: push staged bytes until they land (event-driven
    /// backoff), `true` on success. Re-resolves the host each attempt (a
    /// claim can move the chat).
    async fn deliver_attachments(
        &self,
        chat: &str,
        transfers: &[crate::uploads::AttachmentTransfer],
    ) -> bool {
        let mut wake = commutator_sync::wake::subscribe();
        let mut online = commutator_sync::wake::subscribe_online();
        let mut backoff = TRANSFER_BACKOFF_BASE;
        let deadline = tokio::time::Instant::now() + ATTACHMENT_WAIT_MAX;
        loop {
            let Some(target) = self.remote_host_for(chat) else {
                return true; // became locally hosted: bytes already here
            };
            match self.push_attachments(&target, transfers).await {
                Ok(()) => {
                    tracing::info!(chat = %chat, device = %target,
                        count = transfers.len(), "queued attachments delivered");
                    // The bytes beat the drain's next look — kick it via the
                    // durable nudge (the host's UploadCommit already kicked
                    // its local drains too).
                    return true;
                }
                Err(TransferError::Permanent(err)) => {
                    tracing::warn!(chat = %chat, device = %target, error = %err,
                        "queued attachment transfer failed permanently");
                    return false;
                }
                Err(TransferError::Transient(err)) => {
                    tracing::warn!(chat = %chat, device = %target, error = %err,
                        backoff_ms = backoff.as_millis() as u64,
                        "queued attachment transfer retrying");
                }
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(chat = %chat, "queued attachment transfer gave up (wait cap)");
                return false;
            }
            while wake.try_recv().is_ok() {}
            while online.try_recv().is_ok() {}
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = wake.recv() => {}
                _ = online.recv() => {}
            }
            backoff = (backoff * 2).min(TRANSFER_BACKOFF_CAP);
        }
    }

    /// Every local chat2 batch acked while connected — our rows are ON the
    /// edge, and the host's own room connection will deliver them. (A room
    /// that hasn't joined keeps pre-join updates in a local buffer, so
    /// `connected` is load-bearing here, not just the empty queue.)
    fn rows_flushed(&self, chat_id: &str) -> bool {
        let handle = lock(&self.inner.handles).get(chat_id).cloned();
        handle
            .and_then(|h| lock(&h.chat2).as_ref().map(|c| c.stats()))
            .is_some_and(|s| s.connected && s.pending_pushes == 0)
    }

    /// One relay attempt: version-gate the host, then forward the entry over
    /// the peer link. Timeouts mark the link suspect (drop + redial next
    /// attempt); a host-side refusal is permanent for THIS attempt but the
    /// escort keeps retrying until the give-up cap (the refusal may be
    /// "attachments not landed yet").
    async fn relay_command(
        &self,
        target: &str,
        chat_id: &str,
        entry: &SessionCommandEntry,
    ) -> Result<&'static str, String> {
        let supported = self
            .workspace()
            .and_then(|ws| ws.read_devices().ok())
            .into_iter()
            .flatten()
            .find(|d| d.id == target)
            .and_then(|d| d.version.as_deref().and_then(commutator_proto::version_triple))
            .is_some_and(|v| v >= RELAY_MIN_VERSION);
        if !supported {
            return Err("host does not support relay delivery (version gate)".into());
        }
        let links = self
            .inner
            .links
            .get()
            .ok_or_else(|| "peer links not wired".to_string())?;
        let client = links
            .client(target)
            .await
            .map_err(|e| format!("peer link: {e}"))?;
        let params = serde_json::json!({ "chatId": chat_id, "entry": entry });
        let call = client.call(commutator_rpc::methods::RELAY_COMMAND, params);
        match tokio::time::timeout(RELAY_CALL_TIMEOUT, call).await {
            Err(_) => {
                links.invalidate(target);
                Err("relay call timed out; peer link suspect".into())
            }
            Ok(Err(commutator_rpc::RpcError::Failed(err))) => Err(format!("host refused: {err}")),
            Ok(Err(err)) => {
                links.invalidate(target);
                Err(format!("relay call failed: {err}"))
            }
            Ok(Ok(reply)) => Ok(match reply.get("outcome").and_then(|v| v.as_str()) {
                Some("executed") => "executed",
                Some("duplicate") => "duplicate",
                Some("expired") => "expired",
                Some("superseded") => "superseded",
                _ => "accepted",
            }),
        }
    }

    /// User-driven retry (the failed-send affordance): re-kick every
    /// delivery road for a chat whose queued sends haven't been adopted —
    /// fresh chat2 socket (a zombie room is the usual suspect), host nudge,
    /// a local drain pass, and a fresh delivery escort per still-pending
    /// command with its attachment transfers re-derived from the entries'
    /// `pending://` refs (idempotent: re-pushing landed bytes re-commits the
    /// same file; the processed ledger keeps execution exactly-once).
    pub fn retry_delivery(&self, chat_id: &str) -> Result<(), EngineError> {
        self.retry_delivery_within(chat_id, 0)
    }

    /// The person woke a chat's Cloud machine (a size picked, a fresh
    /// machine): what waited for it while it couldn't be woken goes now,
    /// however long that was. A message's clock only runs once its machine can
    /// take it, so one whose clock ran out while it waited — or would while
    /// the machine boots — goes again with a fresh one, as a retry would send
    /// it, and every other road is re-kicked as a retry does.
    /// (A chat this device has no row for has nothing of ours waiting: its
    /// doc isn't opened, which could mint it in the wrong room.)
    pub fn restart_waiting_clocks(&self, chat_id: &str) -> Result<(), EngineError> {
        let known = self
            .workspace()
            .and_then(|w| w.chat(chat_id).ok().flatten())
            .is_some();
        if !known {
            return Ok(());
        }
        self.retry_delivery_within(chat_id, WAKE_BOOT_MARGIN_MS)
    }

    /// [`Self::retry_delivery`], also re-issuing a pending attempt whose clock
    /// runs out within `ending_within` ms: it can't execute (or won't, by the
    /// time its host reads it), and re-sending it is what the caller asked.
    fn retry_delivery_within(&self, chat_id: &str, ending_within: i64) -> Result<(), EngineError> {
        let handle = self.open(chat_id)?;
        if let Some(chat2) = lock(&handle.chat2).as_ref() {
            chat2.redial();
        }
        // A message still in the outbox isn't there for the host to find, and a
        // woken machine bills: the send wakes it once its bytes are stored.
        let held_messages = self.held_message_ids(chat_id);
        if held_messages.is_empty() {
        }
        self.retry_held_sends(chat_id)?;
        let peer_transfers = self.peer_transfers_needed(chat_id);
        let commands = handle.doc.read_commands()?;
        let ends_by = now_ms() + ending_within;
        let ending = |c: &SessionCommandEntry| {
            c.status == SessionCommandStatus::Pending && c.effective_expiry() <= ends_by
        };
        let pending: Vec<SessionCommandEntry> = commands
            .iter()
            .filter(|c| {
                c.status == SessionCommandStatus::Pending
                    && !ending(c)
                    && !self.inner.store.is_processed(&c.id).unwrap_or(false)
            })
            .cloned()
            .collect();
        for entry in pending {
            let transfers = if peer_transfers {
                command_transfers(&entry)
            } else {
                Vec::new()
            };
            self.spawn_command_delivery(chat_id, entry, transfers);
        }
        // Dead attempts: a Run/Steer whose user message never landed and
        // whose command can never execute again — Rejected (execute failed,
        // or the dead-command sweep terminalized it), or consumed by the
        // ledger with no outcome and not currently executing (crash between
        // mark and resolve). Exactly-once is per command ID, so a
        // user-driven retry mints a FRESH attempt: new id, same payload and
        // message id (the executor's user-entry pre-write dedupes by message
        // id). One re-issue per message — the LATEST attempt speaks for it.
        let messages = handle.doc.read_entries_shared().unwrap_or_default();
        let message_landed = |mid: &str| messages.iter().any(|m| m.id == mid);
        let mut latest_dead: HashMap<String, &SessionCommandEntry> = HashMap::new();
        for c in &commands {
            let Some(mid) = (match &c.payload {
                SessionCommandPayload::Run { message_id, .. } => Some(message_id.as_str()),
                SessionCommandPayload::Steer { message_id, .. } => message_id.as_deref(),
                _ => None,
            }) else {
                continue;
            };
            if message_landed(mid) || held_messages.iter().any(|held| held == mid) {
                continue;
            }
            let dead = match c.status {
                // Rejected: execute failed or the sweep terminalized a crash
                // window. Expired: the entry outlived its TTL undelivered —
                // an explicit user retry is exactly the consent to re-send.
                SessionCommandStatus::Rejected | SessionCommandStatus::Expired => true,
                // Its clock ran out unexecuted (its host would mark it
                // Expired), or is about to: the same consent applies.
                SessionCommandStatus::Pending if ending(c) => {
                    !lock(&self.inner.executing).contains(&c.id)
                }
                SessionCommandStatus::Pending => {
                    self.inner.store.is_processed(&c.id).unwrap_or(false)
                        && !lock(&self.inner.executing).contains(&c.id)
                }
                _ => false,
            };
            if !dead {
                continue;
            }
            // A LIVE pending attempt for the same message (queued or being
            // escorted above) makes a re-issue a duplicate — skip.
            let live_attempt = commands.iter().any(|o| {
                o.id != c.id
                    && o.status == SessionCommandStatus::Pending
                    && !ending(o)
                    && !self.inner.store.is_processed(&o.id).unwrap_or(false)
                    && match (&o.payload, &c.payload) {
                        (
                            SessionCommandPayload::Run { message_id: a, .. },
                            SessionCommandPayload::Run { message_id: b, .. },
                        ) => a == b,
                        (
                            SessionCommandPayload::Steer { message_id: a, .. },
                            SessionCommandPayload::Steer { message_id: b, .. },
                        ) => a == b,
                        _ => false,
                    }
            });
            if live_attempt {
                continue;
            }
            match latest_dead.entry(mid.to_string()) {
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    if c.issued_at > slot.get().issued_at {
                        slot.insert(c);
                    }
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(c);
                }
            }
        }
        for old in latest_dead.values() {
            if old.status == SessionCommandStatus::Pending {
                // Terminalize the consumed-but-dead original so the doc tells
                // the truth and the next retry pass doesn't see it again.
                let why = if ending(old) {
                    "its time ran out while it waited — superseded by retry"
                } else {
                    "interrupted before completion — superseded by retry"
                };
                self.resolve_command(&handle, &old.id, SessionCommandStatus::Rejected, Some(why));
            }
            let now = now_ms();
            let reissue = SessionCommandEntry {
                id: new_id(),
                payload: old.payload.clone(),
                issued_by: self.inner.config.device_id.clone(),
                issued_at: now,
                based_on: messages.last().map(|m| CommandBasedOn {
                    turn_id: Some(m.id.clone()),
                    frontier: None,
                }),
                expires_at: Some(now + commutator_doc::COMMAND_DEFAULT_TTL_MS),
                status: SessionCommandStatus::Pending,
                resolution: None,
            };
            tracing::info!(chat = %chat_id, old = %old.id, new = %reissue.id,
                "retry re-issues a dead send attempt");
            let transfers = command_transfers(&reissue);
            if self.hold_reissue(chat_id, &reissue, &transfers)? {
                continue;
            }
            handle.doc.queue_command(&reissue)?;
            self.spawn_command_delivery(chat_id, reissue, transfers);
        }
        // Locally-hosted (or already-synced) commands: a drain pass is the
        // whole retry.
        if tokio::runtime::Handle::try_current().is_ok() {
            let host = self.clone();
            let handle = handle.clone();
            self.spawn_worker(async move { host.drain_commands(&handle).await });
        }
        Ok(())
    }

    /// Host side of [`commutator_rpc::methods::RELAY_COMMAND`]: evaluate the
    /// forwarded entry against OUR doc (dedupe/TTL/supersede rules apply
    /// unchanged), claim its client-minted id in the processed ledger, then
    /// execute. The claim is what makes the doc row arriving later — over
    /// chat2 sync — a no-op in the drain: exactly-once across both roads.
    pub async fn ingest_relayed_command(
        &self,
        chat_id: &str,
        entry: SessionCommandEntry,
    ) -> Result<&'static str, EngineError> {
        let handle = self.open(chat_id)?;
        // The sender sequences attachment transfers BEFORE the relay; refuse
        // (retryably) rather than run without the images.
        let missing = self.missing_attachments(&entry);
        if !missing.is_empty() {
            return Err(EngineError::Other("attachments not landed yet".into()));
        }
        let sessions = self
            .sessions()
            .ok_or_else(|| EngineError::Other("executor unavailable".into()))?;
        let _prompt_guard = if matches!(
            entry.payload,
            SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
        ) {
            Some(handle.command_drain_lock.lock().await)
        } else {
            None
        };
        let commands = handle.doc.read_commands()?;
        let messages = handle.doc.read_entries_shared().unwrap_or_default();
        let current_turn_id = messages.last().map(|m| m.id.clone());
        let turn_is_past = |turn_id: &str| messages.iter().any(|m| m.id == turn_id);
        let is_processed = |id: &str| self.inner.store.is_processed(id).unwrap_or(false);
        let disposition = evaluate_command(
            &entry,
            &EvaluationContext {
                is_processed: &is_processed,
                now_ms: now_ms(),
                entries: &commands,
                current_turn_id: current_turn_id.as_deref(),
                turn_is_past: &turn_is_past,
            },
        );
        if matches!(disposition, CommandDisposition::Skip) {
            return Ok("duplicate");
        }
        // In-flight claim first (the drain's dead-command sweep must see this
        // id as alive, not crashed, while the execute below runs).
        if !lock(&self.inner.executing).insert(entry.id.clone()) {
            return Ok("duplicate");
        }
        // Claim BEFORE executing (the drain's own mark-before-execute rule).
        let marked = self.inner.store.mark_processed(&entry.id);
        let result = match marked {
            Err(err) => Err(err.into()),
            Ok(false) => Ok("duplicate"),
            Ok(true) => match disposition {
                CommandDisposition::Skip => Ok("duplicate"),
                CommandDisposition::Expired => Ok("expired"),
                CommandDisposition::Superseded => Ok("superseded"),
                CommandDisposition::Execute => match {
                    if _prompt_guard.is_some() {
                        self.await_earlier_controls(&handle, &entry.id).await;
                    }
                    self.execute(&sessions, &handle, &entry).await
                } {
                    Ok(_) => Ok("executed"),
                    Err(err) => Err(err),
                },
            },
        };
        lock(&self.inner.executing).remove(&entry.id);
        result
    }

    /// One transfer attempt: chunked `UploadChunk` + `UploadCommit` straight
    /// over the peer link (same wire the UI's legacy path used, so old and
    /// new engines interoperate). Timeouts mark the link suspect —
    /// `invalidate` drops the cached socket so the retry dials fresh instead
    /// of feeding a zombie pipe forever (2026-08-19 incident).
    async fn push_attachments(
        &self,
        target: &str,
        transfers: &[crate::uploads::AttachmentTransfer],
    ) -> Result<(), TransferError> {
        use TransferError::{Permanent, Transient};
        let Some(links) = self.inner.links.get() else {
            return Err(Permanent("peer links not wired".into()));
        };
        let Some(uploads) = self.inner.uploads.get() else {
            return Err(Permanent("uploads not wired".into()));
        };
        let client = links
            .client(target)
            .await
            .map_err(|e| Transient(format!("peer link: {e}")))?;
        for transfer in transfers {
            // Bytes come from the uploads jail only — a transfer names an
            // upload identity, never an arbitrary path.
            let source = uploads.pending_target(&transfer.upload_id, &transfer.file_name);
            let bytes = tokio::fs::read(&source)
                .await
                .map_err(|e| Permanent(format!("staged attachment missing: {e}")))?;
            // Progress entry for the sender's thumbnail ring, updated per
            // landed chunk. The guard retires it on EVERY exit — commit,
            // timeout, refusal — so a dead attempt falls back to the
            // indeterminate spinner and the retry re-publishes from 0.
            let total = bytes.len() as u64;
            self.transfer_progress_set(&transfer.upload_id, &transfer.file_name, 0, total);
            let _progress = TransferProgressGuard {
                host: self,
                upload_id: &transfer.upload_id,
            };
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let mut start = 0usize;
            let mut seq = 0u64;
            loop {
                let end = (start + TRANSFER_CHUNK_B64).min(b64.len());
                let params = serde_json::json!({
                    "uploadId": transfer.upload_id, "seq": seq, "data": &b64[start..end],
                });
                let call = client.call(commutator_rpc::methods::UPLOAD_CHUNK, params);
                match tokio::time::timeout(TRANSFER_CHUNK_TIMEOUT, call).await {
                    Err(_) => {
                        links.invalidate(target);
                        return Err(Transient("chunk push timed out; peer link suspect".into()));
                    }
                    Ok(Err(commutator_rpc::RpcError::Failed(err))) => {
                        return Err(Permanent(format!("host refused chunk: {err}")));
                    }
                    Ok(Err(err)) => {
                        links.invalidate(target);
                        return Err(Transient(format!("chunk push failed: {err}")));
                    }
                    Ok(Ok(_)) => {}
                }
                start = end;
                seq += 1;
                // b64 → raw: 4 chars carry 3 bytes; min-clamp absorbs the
                // final chunk's padding overshoot.
                let done = ((start as u64) * 3 / 4).min(total);
                self.transfer_progress_set(&transfer.upload_id, &transfer.file_name, done, total);
                if start >= b64.len() {
                    break;
                }
            }
            let params = serde_json::json!({
                "uploadId": transfer.upload_id, "fileName": transfer.file_name,
            });
            let call = client.call(commutator_rpc::methods::UPLOAD_COMMIT, params);
            match tokio::time::timeout(TRANSFER_COMMIT_TIMEOUT, call).await {
                Err(_) => {
                    links.invalidate(target);
                    return Err(Transient("commit timed out; peer link suspect".into()));
                }
                Ok(Err(commutator_rpc::RpcError::Failed(err))) => {
                    return Err(Permanent(format!("host refused commit: {err}")));
                }
                Ok(Err(err)) => {
                    links.invalidate(target);
                    return Err(Transient(format!("commit failed: {err}")));
                }
                Ok(Ok(_)) => {}
            }
        }
        Ok(())
    }

    /// Fetch a sidecar blob by its doc-resident ref (`{chatId}/{partId}` or
    /// `…​.diff`) — the UI's lazy "Show full output" path, served over RPC
    /// because the UI crate has no HTTP client or edge bearer.
    pub async fn fetch_tool_blob(&self, blob_ref: &str) -> Result<String, EngineError> {
        // Same shape `apply_sidecar_refs` writes; anything else is a forged ref.
        let valid = blob_ref.split_once('/').is_some_and(|(chat, part)| {
            !chat.is_empty()
                && chat.len() <= 128
                && chat
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                && !part.is_empty()
                && part.len() <= 200
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:#~-".contains(&b))
        });
        if !valid {
            return Err(EngineError::Other(format!("bad blob ref: {blob_ref}")));
        }
        // Full outputs were kept at the edge; this device keeps only the summary.
        Err(EngineError::Other("full output isn't kept on this device".into()))
    }

    /// §2.2 writer discipline: we host a chat iff its workspace row's `deviceId` is
    /// ours; a chat with no row is claimable (claim-on-first-command). Without a
    /// wired workspace host (bare-DocHost tests) every open chat is ours — M2's
    /// behavior, now the degenerate case.
    fn is_host(&self, chat_id: &str) -> bool {
        self.workspace().is_none_or(|ws| ws.is_host(chat_id))
    }

    /// Chat-config harness when the workspace row carries one, else the default.
    pub(crate) fn harness_for(&self, chat_id: &str) -> HarnessId {
        self.workspace()
            .and_then(|ws| ws.chat_config(chat_id))
            .map(|config| config.harness)
            .unwrap_or(self.inner.config.default_harness)
    }

    /// The harness a request dispatches on: the request's own pick when it
    /// carries one (rides the command plane, immune to registry-row races),
    /// else [`Self::harness_for`].
    pub(crate) fn harness_for_request(
        &self,
        chat_id: &str,
        request: &commutator_proto::RunRequest,
    ) -> HarnessId {
        request.harness.unwrap_or_else(|| self.harness_for(chat_id))
    }

    /// Drain pending commands (host-only): evaluate → mark processed BEFORE execute →
    /// execute → write the outcome as the sole outcome writer.
    pub async fn drain_commands(&self, handle: &Arc<ChatDocHandle>) {
        self.drain_command_kind(handle, false).await;
    }

    async fn drain_command_kind(&self, handle: &Arc<ChatDocHandle>, controls_only: bool) {
        // A quiesced engine still serves controls (interrupt, answers): only
        // new work waits for the machine to wake.
        let _dispatch = if controls_only {
            None
        } else {
            match self.dispatch_guard() {
                Some(guard) => Some(guard),
                None => return, // work is held; lifting it re-drains
            }
        };
        let Some(sessions) = self.sessions() else {
            return; // executor not wired yet (or retired); the set_sessions kick re-drains
        };
        if !self.is_host(&handle.chat_id) {
            return;
        }
        // Do not let another drain overtake a prompt waiting for mailbox
        // capacity (or preflight). Controls bypass this lock so a stalled
        // provider can still be interrupted or have its question answered.
        let mut prompt_guard = if controls_only {
            None
        } else {
            handle.command_drain_lock.try_lock().ok()
        };
        // Entries this pass decided to leave alone (processed dedupe hits).
        let mut skipped: HashSet<String> = HashSet::new();
        loop {
            let commands = match handle.doc.read_commands() {
                Ok(commands) => commands,
                Err(err) => {
                    tracing::warn!(chat = %handle.chat_id, error = %err, "command read failed");
                    return;
                }
            };
            let is_processed = |id: &str| self.inner.store.is_processed(id).unwrap_or(false);
            // Dead-command sweep: Pending in the doc, consumed by the ledger,
            // and NOT mid-execution in this process — the crash window
            // between mark-processed and the outcome write. Left alone it is
            // a send no drain or retry can ever reach ("Sending…" forever,
            // 2026-08-19); terminalize it so the truth lands in the doc and
            // a user retry can mint a fresh attempt. `commands` may predate a
            // racing drain that has since resolved the command and released
            // its claim (resolve comes first), so once no claim is seen the
            // status is read again rather than overwriting that outcome.
            let still_pending = |id: &str| {
                handle.doc.read_commands().is_ok_and(|now| {
                    now.iter()
                        .any(|c| c.id == id && c.status == SessionCommandStatus::Pending)
                })
            };
            for c in &commands {
                if c.status == SessionCommandStatus::Pending
                    && !skipped.contains(&c.id)
                    && is_processed(&c.id)
                    && !lock(&self.inner.executing).contains(&c.id)
                    && still_pending(&c.id)
                {
                    tracing::warn!(chat = %handle.chat_id, command = %c.id,
                        "command consumed but never resolved (crash mid-execute?); rejecting");
                    self.resolve_command(
                        handle,
                        &c.id,
                        SessionCommandStatus::Rejected,
                        Some("interrupted before completion — retry to send again"),
                    );
                    skipped.insert(c.id.clone());
                }
            }
            let Some(entry) = commands
                .iter()
                .find(|c| {
                    c.status == SessionCommandStatus::Pending
                        && !skipped.contains(&c.id)
                        && !is_processed(&c.id)
                        && (prompt_guard.is_some()
                            || !matches!(
                                c.payload,
                                SessionCommandPayload::Run { .. }
                                    | SessionCommandPayload::Steer { .. }
                            ))
                })
                .cloned()
            else {
                if prompt_guard.is_none() && !controls_only {
                    // Wait after handling controls, then re-read: simply
                    // returning here could miss a newly appended prompt.
                    prompt_guard = Some(handle.command_drain_lock.lock().await);
                    continue;
                }
                return;
            };
            let messages = handle.doc.read_entries_shared().unwrap_or_default();
            let current_turn_id = messages.last().map(|m| m.id.clone());
            let turn_is_past = |turn_id: &str| messages.iter().any(|m| m.id == turn_id);
            let disposition = evaluate_command(
                &entry,
                &EvaluationContext {
                    is_processed: &is_processed,
                    now_ms: now_ms(),
                    entries: &commands,
                    current_turn_id: current_turn_id.as_deref(),
                    turn_is_past: &turn_is_past,
                },
            );
            // Queued-attachment gate (BEFORE the processed mark — a deferred
            // command must stay eligible): a Run/Steer naming `pending://`
            // refs whose bytes haven't landed on this device yet waits for
            // the transfer instead of running without its images. The wait is
            // bounded unless they're safely stored; past it the command fails
            // loudly.
            if matches!(disposition, CommandDisposition::Execute) {
                let missing = self.missing_attachments(&entry);
                if !missing.is_empty() {
                    if now_ms().saturating_sub(entry.issued_at) < ATTACHMENT_WAIT_MAX_MS {
                        tracing::info!(chat = %handle.chat_id, command = %entry.id,
                            missing = missing.len(), "command deferred: attachment bytes in transit");
                        self.arm_attachment_wait(handle);
                        return; // preserve order; UploadCommit / the wait timer re-kick
                    }
                    // In-flight claim, as for an execute: between the mark and
                    // the outcome, a racing drain's dead-command sweep would
                    // otherwise take this for a crash and write its own reason.
                    if !lock(&self.inner.executing).insert(entry.id.clone()) {
                        skipped.insert(entry.id.clone());
                        continue;
                    }
                    if let Err(err) = self.inner.store.mark_processed(&entry.id) {
                        tracing::error!(chat = %handle.chat_id, error = %err,
                            "processed-ledger write failed; halting drain");
                        lock(&self.inner.executing).remove(&entry.id);
                        return;
                    }
                    tracing::warn!(chat = %handle.chat_id, command = %entry.id,
                        "command rejected: attachments never arrived");
                    self.resolve_command(
                        handle,
                        &entry.id,
                        SessionCommandStatus::Rejected,
                        Some("attachments never arrived"),
                    );
                    lock(&self.inner.executing).remove(&entry.id);
                    continue;
                }
            }
            // In-flight claim: guards the dead-command sweep (an id in
            // `executing` is alive, not crashed) and serializes racing
            // drains on the same entry.
            if !lock(&self.inner.executing).insert(entry.id.clone()) {
                skipped.insert(entry.id.clone());
                continue;
            }
            // Mark BEFORE executing: a crash mid-execution must never double-run a
            // command whose side effect may already have happened.
            if let Err(err) = self.inner.store.mark_processed(&entry.id) {
                tracing::error!(chat = %handle.chat_id, error = %err, "processed-ledger write failed; halting drain");
                lock(&self.inner.executing).remove(&entry.id);
                return;
            }
            match disposition {
                CommandDisposition::Skip => {
                    skipped.insert(entry.id.clone());
                }
                CommandDisposition::Expired => {
                    self.resolve_command(handle, &entry.id, SessionCommandStatus::Expired, None);
                }
                CommandDisposition::Superseded => {
                    self.resolve_command(handle, &entry.id, SessionCommandStatus::Superseded, None);
                }
                CommandDisposition::Execute => {
                    if matches!(
                        entry.payload,
                        SessionCommandPayload::Run { .. } | SessionCommandPayload::Steer { .. }
                    ) {
                        tracing::debug!(target: "first_word", chat = %handle.chat_id, "drain picked the run");
                        self.await_earlier_controls(handle, &entry.id).await;
                    }
                    let (status, resolution) = match self.execute(&sessions, handle, &entry).await {
                        Ok(outcome) => outcome,
                        Err(err) => (SessionCommandStatus::Rejected, Some(err.to_string())),
                    };
                    self.resolve_command(handle, &entry.id, status, resolution.as_deref());
                }
            }
            lock(&self.inner.executing).remove(&entry.id);
        }
    }

    /// The command's `pending://` attachment refs whose bytes are NOT on this
    /// device's disk yet. Empty when the command names none, when everything
    /// has landed, or when no uploads store is wired (tests) — absence of the
    /// subsystem must never wedge a queue.
    fn missing_attachments(&self, entry: &SessionCommandEntry) -> Vec<String> {
        let refs: Vec<String> = match &entry.payload {
            SessionCommandPayload::Run { request, .. } => request
                .attachments
                .iter()
                .filter(|p| crate::uploads::is_pending_ref(p))
                .cloned()
                .collect(),
            SessionCommandPayload::Steer { prompt, .. } => crate::uploads::pending_refs_in(prompt),
            _ => Vec::new(),
        };
        if refs.is_empty() {
            return refs;
        }
        let Some(uploads) = self.inner.uploads.get() else {
            return Vec::new();
        };
        refs.into_iter()
            .filter(|r| uploads.resolve_pending(r).is_none())
            .collect()
    }

    /// Arm (once per chat) the deferred-command re-check loop: while a
    /// pending unprocessed command still waits on attachment bytes, re-drain
    /// on a cadence so the bounded wait actually expires even if every
    /// event-driven kick was missed.
    fn arm_attachment_wait(&self, handle: &Arc<ChatDocHandle>) {
        let chat = handle.chat_id.clone();
        if !lock(&self.inner.drain_waiting).insert(chat.clone()) {
            return;
        }
        let weak = Arc::downgrade(handle);
        let host = self.clone();
        self.spawn_worker(async move {
            loop {
                tokio::time::sleep(ATTACHMENT_WAIT_RECHECK).await;
                let Some(handle) = weak.upgrade() else { break };
                if !host.awaiting_attachments(&handle) {
                    break;
                }
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
                let Some(handle) = weak.upgrade() else { break };
                if !host.awaiting_attachments(&handle) {
                    break;
                }
            }
            lock(&host.inner.drain_waiting).remove(&chat);
        });
    }

    /// True while some pending, unprocessed command still waits on bytes.
    fn awaiting_attachments(&self, handle: &Arc<ChatDocHandle>) -> bool {
        let commands = handle.doc.read_commands().unwrap_or_default();
        let command_waiting = commands.iter().any(|c| {
            c.status == SessionCommandStatus::Pending
                && !self.inner.store.is_processed(&c.id).unwrap_or(false)
                && !self.missing_attachments(c).is_empty()
        });
        command_waiting
            || handle
                .doc
                .read_queue()
                .ok()
                .and_then(|q| q.into_iter().next())
                .is_some_and(|head| !self.missing_row_attachments(&head).is_empty())
    }

    /// Rewrite a request's landed `pending://` refs to this device's absolute
    /// paths — in the attachments list AND the prompt text — so the harness
    /// (and the persisted user entry) see ordinary local files, exactly like
    /// the legacy pre-upload flow produced.
    fn resolve_request_attachments(&self, request: &mut commutator_proto::RunRequest) {
        self.resolve_attachment_refs(&mut request.prompt, &mut request.attachments);
    }

    /// Rewrite landed `pending://` refs in `attachments` (and wherever the
    /// prompt names them) to this device's absolute paths.
    fn resolve_attachment_refs(&self, prompt: &mut String, attachments: &mut [String]) {
        let Some(uploads) = self.inner.uploads.get() else {
            return;
        };
        for path in attachments.iter_mut() {
            if let Some(abs) = uploads.localize(path) {
                *prompt = crate::uploads::rewrite_pending_ref(prompt, path, &abs);
                *path = abs;
            }
        }
        // Paths only the prompt names (older clients sent no list).
        for path in crate::uploads::attachment_lines_in(&prompt.clone()) {
            if let Some(abs) = uploads.localize(&path) {
                *prompt = crate::uploads::rewrite_pending_ref(prompt, &path, &abs);
            }
        }
    }

    /// A queue row's `pending://` refs whose bytes are not on this device
    /// yet ([`Self::missing_attachments`] for rows). Covers refs in the
    /// row's text too: older clients inlined the attachment trailer.
    fn missing_row_attachments(&self, item: &QueuedMessage) -> Vec<String> {
        let Some(uploads) = self.inner.uploads.get() else {
            return Vec::new();
        };
        let mut refs: Vec<String> = item
            .attachments
            .iter()
            .filter(|p| crate::uploads::is_pending_ref(p))
            .cloned()
            .collect();
        for r in crate::uploads::pending_refs_in(&item.text) {
            if !refs.contains(&r) {
                refs.push(r);
            }
        }
        refs.into_iter()
            .filter(|r| uploads.resolve_pending(r).is_none())
            .collect()
    }

    /// [`Self::resolve_request_attachments`] for a bare prompt (Steer).
    fn resolve_prompt_attachments(&self, prompt: &str) -> String {
        let Some(uploads) = self.inner.uploads.get() else {
            return prompt.to_string();
        };
        let mut out = prompt.to_string();
        let mut refs = crate::uploads::pending_refs_in(prompt);
        for path in crate::uploads::attachment_lines_in(prompt) {
            if !refs.contains(&path) {
                refs.push(path);
            }
        }
        for r in refs {
            if let Some(abs) = uploads.localize(&r) {
                out = crate::uploads::rewrite_pending_ref(&out, &r, &abs);
            }
        }
        out
    }

    /// Host-only outcome write (ledger rule 2).
    fn resolve_command(
        &self,
        handle: &ChatDocHandle,
        command_id: &str,
        status: SessionCommandStatus,
        resolution: Option<&str>,
    ) {
        if let Err(err) = handle
            .doc
            .set_command_status(command_id, status, resolution)
        {
            tracing::warn!(
                chat = %handle.chat_id,
                command = %command_id,
                error = %err,
                "command outcome write failed"
            );
        }
    }

    /// `entry` is a prompt sent before the Stop that has already acted on
    /// this chat: the Stop cancelled it. Returns its message id.
    fn cancelled_by_stop(
        &self,
        handle: &Arc<ChatDocHandle>,
        entry: &SessionCommandEntry,
    ) -> Option<String> {
        let message_id = match &entry.payload {
            SessionCommandPayload::Run { message_id, .. } => message_id.clone(),
            SessionCommandPayload::Steer { message_id, .. } => {
                message_id.clone().unwrap_or_else(|| entry.id.clone())
            }
            _ => return None,
        };
        let stop = lock(&handle.stopped_by).clone()?;
        let commands = handle.doc.read_commands().ok()?;
        let at = commands.iter().position(|c| c.id == entry.id)?;
        let stop_at = commands.iter().position(|c| c.id == stop)?;
        (at < stop_at).then_some(message_id)
    }

    async fn execute(
        &self,
        sessions: &SessionsEngine,
        handle: &Arc<ChatDocHandle>,
        entry: &SessionCommandEntry,
    ) -> Result<(SessionCommandStatus, Option<String>), EngineError> {
        let chat_id = &handle.chat_id;
        if let Some(message_id) = self.cancelled_by_stop(handle, entry) {
            // Still the user's message: it waits at the head of the paused
            // queue, to send, edit or remove — never stranded unrun in the
            // transcript, where nothing would ever answer it.
            let (prompt, attachments) = match &entry.payload {
                SessionCommandPayload::Run { request, .. } => {
                    (request.prompt.clone(), request.attachments.clone())
                }
                SessionCommandPayload::Steer { prompt, .. } => (prompt.clone(), Vec::new()),
                _ => (String::new(), Vec::new()),
            };
            let row = QueuedMessage {
                id: message_id,
                text: prompt,
                attachments,
                hold_for_turn_end: false,
                issued_by: entry.issued_by.clone(),
                issued_at: entry.issued_at.min(now_ms()),
                edited_at: None,
                delivery_gate: None,
                config: None,
            };
            let queued = handle.doc.read_queue().is_ok_and(|q| q.iter().any(|r| r.id == row.id));
            if !queued && let Err(err) = handle.doc.insert_queued(0, &row) {
                tracing::warn!(chat = %chat_id, error = %err, "message cancelled by a Stop not kept in the queue");
            }
            handle.queue_paused.store(true, Ordering::Release);
            handle.publish_queue();
            tracing::info!(chat = %chat_id, "prompt sent before a Stop that has acted: held in the queue");
            return Ok((
                SessionCommandStatus::Applied,
                Some("cancelled by the Stop sent after it".into()),
            ));
        }
        match &entry.payload {
            SessionCommandPayload::Run {
                request,
                message_id,
            } => {
                let mut request = request.clone();
                // Queued-attachment refs (`pending://`) resolve to this
                // host's absolute paths before anything persists or
                // dispatches — the drain already gated on the bytes being
                // present, so every ref resolves here.
                self.resolve_request_attachments(&mut request);
                // Worktree directive (WorktreeSpec): materialize on THIS host at
                // drain time — the durable command plane replaces the sender's
                // old blocking CreateWorktree relay RPC, whose lost reply wedged
                // the composer on "Sending…" while the run proceeded anyway.
                // `take()` resolves the request before dispatch, so the journal
                // and steer→new-turn fallbacks reuse the created path instead of
                // minting another checkout.
                let worktree_spec = request.worktree.take();
                let fresh_worktree = match &worktree_spec {
                    Some(spec) => {
                        let (cwd, fresh) = self.materialize_worktree(chat_id, &spec).await?;
                        request.cwd = cwd;
                        fresh
                    }
                    None => None,
                };
                // Claim-on-first-command: a run for a chat with no workspace row
                // creates the row under our device id (we are about to host it).
                if let Some(ws) = self.workspace() {
                    ws.claim_chat(chat_id, Some(&request.cwd))?;
                    // Older clients send `~` for a projectless run; a chat
                    // created with a scratch folder runs there instead.
                    if commutator_proto::projectless::is_home_cwd(&request.cwd)
                        && let Ok(Some(chat)) = ws.chat(chat_id)
                        && let Some(cwd) = chat.cwd.filter(|cwd| {
                            chat.space_id.is_none() && commutator_proto::projectless::is_scratch_cwd(cwd)
                        })
                    {
                        request.cwd = cwd;
                    }
                    // A pre-existing row (the client's createChat raced ahead)
                    // still carries the repo folder — repoint it at the fresh
                    // worktree, and stamp the actual `commutator/<name>` branch so
                    // the footer and the title-rename flow see it.
                    if let Some(wt) = &fresh_worktree {
                        if let Err(err) = ws.set_chat_cwd(chat_id, &wt.path) {
                            tracing::warn!(chat = %chat_id, error = %err, "worktree cwd stamp failed");
                        }
                        if let Err(err) = ws.set_chat_branch(chat_id, &wt.branch) {
                            tracing::warn!(chat = %chat_id, error = %err, "worktree branch stamp failed");
                        }
                    }
                }
                if let Some(spec) = worktree_spec.as_ref()
                    && spec.space_id.is_some()
                {
                    self.complete_worktree_setup_handoff(
                        &entry.id,
                        chat_id,
                        spec,
                        fresh_worktree.as_ref(),
                    );
                }
                let harness = self.harness_for_request(chat_id, &request);
                // A row with no config renders no harness glyph (and every
                // later dispatch falls back to the engine default), so stamp
                // what this run actually executes with. Claimed rows and
                // catalog-not-loaded createChats both land here; the racing
                // real createChat carries the same picked values.
                if let Some(ws) = self.workspace()
                    && ws.chat_config(chat_id).is_none()
                {
                    let config = commutator_proto::ChatConfig {
                        harness,
                        model: request.model.clone(),
                        reasoning: request.reasoning,
                        model_options: request.model_options.clone(),
                        sandbox: request.sandbox,
                        access: request.access,
                        plan: request.plan,
                    };
                    if let Err(err) = ws.set_chat_config(chat_id, &config) {
                        tracing::warn!(chat = %chat_id, error = %err, "run-config backfill failed");
                    }
                }
                // A run on another provider than the live turn's (a stale
                // client, another device's switch) waits for the turn: the
                // provider never changes by killing a turn mid-flight.
                let other_provider = sessions.live_harness(chat_id).is_some_and(|live| live != harness);
                // Sent before the turn now running had started: the sender never
                // saw that turn, so it can't be a steer into it. Two messages sent
                // to a sleeping Cloud machine (or an offline host) start one turn
                // each; steering the second into the first cut the first off with
                // an empty reply.
                // Only when an earlier send started that turn: nothing else changes.
                let sent_before_turn = sessions
                    .turn_started_ms(chat_id)
                    .is_some_and(|started| entry.issued_at < started)
                    && self.live_turn_sent_earlier(handle, entry, message_id);
                if other_provider
                    || sent_before_turn
                    || sessions.defers_to_turn_end(chat_id, Some((harness, &request)))
                {
                    self.hold_until_turn_end(
                        handle,
                        message_id,
                        &request.prompt,
                        entry.issued_at,
                        false,
                        Some(&request),
                    )?;
                    return Ok((
                        SessionCommandStatus::Applied,
                        Some("held until the turn ends".into()),
                    ));
                }
                // Timestamp canonicalization: the user message lands in
                // history at the moment the user SENT it (the entry's
                // issued_at, clamped against clock skew) — not whenever this
                // host got around to draining a queued command. Idempotent by
                // id, so the dispatch path's own execution-time write dedupes
                // to a no-op. A provider switch's seam lands with it, above.
                sessions.note_provider_switch(chat_id, harness, handle, Some(message_id));
                if let Err(err) = handle.write_user_message(
                    message_id,
                    &request.prompt,
                    entry.issued_at.min(now_ms()),
                ) {
                    tracing::warn!(chat = %chat_id, error = %err, "canonical user-message write failed");
                }
                self.dispatch_with_source_context(
                    sessions,
                    chat_id,
                    harness,
                    request,
                    Some(message_id.clone()),
                )
                .await?;
                // A fresh user-authored turn is the deliberate action that
                // thaws a queue frozen by Cancel. Clear only after dispatch
                // succeeds so a failed send cannot silently unfreeze it.
                handle.queue_paused.store(false, Ordering::Release);
                Ok((SessionCommandStatus::Applied, None))
            }
            SessionCommandPayload::Steer { prompt, message_id } => {
                self.deliver_prompt(
                    sessions,
                    handle,
                    prompt,
                    message_id.clone(),
                    entry.issued_at,
                )
                .await
            }
            SessionCommandPayload::Interrupt {} => {
                self.await_earlier_prompts(sessions, handle, &entry.id)
                    .await;
                *lock(&handle.stopped_by) = Some(entry.id.clone());
                // Sends before this Stop that are still on their way (a slow
                // dispatch the wait above gave up on) are cancelled with it.
                if let Ok(commands) = handle.doc.read_commands()
                    && let Some(at) = commands.iter().position(|c| c.id == entry.id)
                {
                    let earlier = commands[..at]
                        .iter()
                        .filter(|c| c.status == SessionCommandStatus::Pending)
                        .filter_map(|c| match &c.payload {
                            SessionCommandPayload::Run { message_id, .. } => {
                                Some(message_id.clone())
                            }
                            SessionCommandPayload::Steer { message_id, .. } => {
                                Some(message_id.clone().unwrap_or_else(|| c.id.clone()))
                            }
                            _ => None,
                        })
                        .collect();
                    sessions.cancel_sent_messages(&handle.chat_id, earlier);
                }
                self.interrupt_and_pause_queue(sessions, handle).await?;
                Ok((SessionCommandStatus::Applied, None))
            }
            SessionCommandPayload::RespondInput {
                request_id,
                answers,
            } => {
                if sessions.respond_input(chat_id, request_id, answers.clone())? {
                    return Ok((SessionCommandStatus::Applied, None));
                }
                // No live resolver. Only a request id the doc shows as an
                // OPEN question on a SETTLED entry gets the orphan fallback:
                // a mismatched or already-resolved id is a stale/buggy answer
                // and must still reject, and a still-streaming entry's
                // question belongs to the live run (a just-consumed resolver
                // racing a second answer must not spawn a duplicate turn).
                let questions = handle.doc.read_entries_shared().ok().and_then(|entries| {
                    entries
                        .iter()
                        .rev()
                        .filter(|e| e.status != Some(MessageStatus::Streaming))
                        .find_map(|e| {
                            e.parts.iter().find_map(|p| match p {
                                MessagePart::Input {
                                    request_id: rid,
                                    questions,
                                    resolved: false,
                                    ..
                                } if rid == request_id => Some(questions.clone()),
                                _ => None,
                            })
                        })
                });
                let Some(questions) = questions else {
                    return Ok((
                        SessionCommandStatus::Rejected,
                        Some("no pending input request".into()),
                    ));
                };
                // Closed without an answer: there is nothing to deliver, and
                // a resumed turn would only say so. Record it and stop.
                if answers.is_empty() {
                    handle.doc.resolve_input(request_id, answers)?;
                    return Ok((
                        SessionCommandStatus::Applied,
                        Some("closed unanswered".into()),
                    ));
                }
                // The run died under the question (engine restart, crash).
                // The question is still open in the doc and the command is
                // durable, so honor it anyway — stamp the part resolved and
                // deliver the answers as the next (resumed) turn, the same
                // fallback a dead-run steer takes. The question UI stays up
                // until the user answers (user requirement); this is what
                // makes that answer still WORK.
                let request = sessions
                    .last_request(chat_id)
                    .or_else(|| self.request_from_chat_row(chat_id, ""));
                let Some(mut request) = request else {
                    return Ok((
                        SessionCommandStatus::Rejected,
                        Some("no pending input request and no prior run config".into()),
                    ));
                };
                request.prompt = respond_input_prompt(&questions, answers);
                request.resume = None; // dispatch re-derives the harness session
                request.attachments = Vec::new();
                if let Err(err) = handle.doc.resolve_input(request_id, answers) {
                    tracing::warn!(chat = %chat_id, request = %request_id, error = %err,
                        "orphaned input resolve failed");
                }
                let harness = self.harness_for_request(chat_id, &request);
                self.dispatch_with_source_context(sessions, chat_id, harness, request, None)
                    .await?;
                Ok((
                    SessionCommandStatus::Applied,
                    Some("answered as new turn".into()),
                ))
            }
        }
    }

    /// Park a prompt for a turn-boundary agent in the visible queue instead
    /// of its mailbox, keeping the message id so the transcript entry written
    /// at delivery is the same message. Steers lead ordinary rows.
    /// The running turn is the latest message in the transcript, sent as a run no
    /// later than `entry` and not `entry`'s own: a second send that went out while
    /// the host was away, now arriving behind the first.
    fn live_turn_sent_earlier(
        &self,
        handle: &Arc<ChatDocHandle>,
        entry: &SessionCommandEntry,
        message_id: &str,
    ) -> bool {
        let Some(live) = handle.doc.read_entries_shared().ok().and_then(|entries| {
            entries
                .iter()
                .rev()
                .find(|e| e.role == MessageRole::User)
                .map(|e| e.id.clone())
        }) else {
            return false;
        };
        if live == message_id {
            return false;
        }
        handle.doc.read_commands().is_ok_and(|commands| {
            commands.iter().any(|c| {
                c.id != entry.id
                    && c.issued_at <= entry.issued_at
                    && matches!(&c.payload, SessionCommandPayload::Run { message_id, .. } if *message_id == live)
            })
        })
    }

    fn hold_until_turn_end(
        &self,
        handle: &Arc<ChatDocHandle>,
        message_id: &str,
        prompt: &str,
        issued_at: i64,
        steer: bool,
        request: Option<&commutator_proto::RunRequest>,
    ) -> Result<(), EngineError> {
        let item = QueuedMessage {
            id: message_id.to_string(),
            text: prompt.to_string(),
            attachments: request.map(|r| r.attachments.clone()).unwrap_or_default(),
            hold_for_turn_end: false,
            issued_by: self.inner.config.device_id.clone(),
            issued_at: issued_at.min(now_ms()),
            edited_at: None,
            delivery_gate: None,
            config: None,
        };
        if handle.doc.read_queue()?.iter().any(|row| row.id == item.id) {
            return Ok(()); // a redelivered command: already held
        }
        if let Some(request) = request {
            lock(&handle.held_sends).insert(item.id.clone(), request.clone());
        }
        if steer {
            handle
                .doc
                .insert_queued(handle.steer_slot(&item.id)?, &item)?;
        } else {
            handle.doc.push_queued(&item)?;
        }
        // Sending is the deliberate action that thaws a queue frozen by Cancel.
        handle.queue_paused.store(false, Ordering::Release);
        handle.publish_queue();
        Ok(())
    }

    /// Put a typed prompt in front of a live agent: steer it in, or — with no
    /// live steerable run — deliver the durable command as the next turn.
    /// After an engine restart `last_request` is empty too, so rebuild the run
    /// config from the chat's workspace row (commutator derived dispatch config from
    /// the chat row the same way — sessions.ts:601-620); dispatch's engine-owned
    /// resume then reattaches the prior harness conversation.
    ///
    /// Turn-boundary drivers retain explicit steers until their next boundary.
    async fn deliver_prompt(
        &self,
        sessions: &SessionsEngine,
        handle: &Arc<ChatDocHandle>,
        prompt: &str,
        message_id: Option<String>,
        issued_at: i64,
    ) -> Result<(SessionCommandStatus, Option<String>), EngineError> {
        let chat_id = &handle.chat_id;
        // Explicit steering uses the run mailbox when the agent reads it
        // mid-turn. A turn-boundary agent would read it only after the turn,
        // so it waits in the queue, ahead of ordinary rows, and reaches the
        // transcript when it is actually delivered. Never interrupt to hurry
        // steering.
        let prompt = self.resolve_prompt_attachments(prompt);
        // A live turn without a mailbox can't take it either: the fresh
        // dispatch below would interrupt it.
        let unsteerable_turn =
            sessions.turn_in_flight(chat_id) && !sessions.live_run_steerable(chat_id);
        if !prompt.trim().is_empty()
            && (unsteerable_turn || sessions.defers_to_turn_end(chat_id, None))
        {
            let id = message_id.unwrap_or_else(new_id);
            self.hold_until_turn_end(handle, &id, &prompt, issued_at, true, None)?;
            return Ok((
                SessionCommandStatus::Applied,
                Some("held until the turn ends".into()),
            ));
        }
        // The transcript shows the send while mailbox backpressure holds it.
        // A pending agent update instead holds it in the queue below, where a
        // transcript copy would duplicate it until the update finishes.
        if !sessions.live_run_update_pending(chat_id)
            && let Some(message_id) = message_id.as_deref()
            && let Err(err) =
                handle.write_user_message(message_id, &prompt, issued_at.min(now_ms()))
        {
            tracing::warn!(chat = %chat_id, error = %err, "canonical user-message write failed");
        }
        // A steer is read at once: while a command runs, its turn stops in
        // place first (the runtime and its background work stay).
        match sessions
            .steer_now(chat_id, &prompt, Vec::new(), message_id.clone(), issued_at)
            .await?
        {
            SteerOutcome::Accepted => {
                handle.queue_paused.store(false, Ordering::Release);
                Ok((SessionCommandStatus::Applied, None))
            }
            SteerOutcome::NotSteerable => {
                if let Some(message_id) = message_id.as_deref() {
                    handle.write_user_message(message_id, &prompt, issued_at.min(now_ms()))?;
                }
                let request = sessions
                    .last_request(chat_id)
                    .or_else(|| self.request_from_chat_row(chat_id, &prompt));
                let Some(mut request) = request else {
                    return Ok((
                        SessionCommandStatus::Rejected,
                        Some("no live run and no prior run config".into()),
                    ));
                };
                request.prompt = prompt;
                request.resume = None; // dispatch re-derives the harness session
                // A reused config must not re-inline the PREVIOUS turn's
                // images; this prompt's own refs (if any) ride its text.
                request.attachments = Vec::new();
                let harness = self.harness_for_request(chat_id, &request);
                self.dispatch_with_source_context(sessions, chat_id, harness, request, message_id)
                    .await?;
                handle.queue_paused.store(false, Ordering::Release);
                Ok((
                    SessionCommandStatus::Applied,
                    Some("queued as new turn".into()),
                ))
            }
            SteerOutcome::DeferredByUpdate => {
                let id = message_id.unwrap_or_else(new_id);
                self.hold_until_turn_end(handle, &id, &prompt, issued_at, true, None)?;
                // The completed turn's status publication normally re-drains
                // this queue. Also cover completion racing the enqueue itself.
                self.drain_queue(handle).await;
                Ok((
                    SessionCommandStatus::Applied,
                    Some("held until the agent update finishes".into()),
                ))
            }
        }
    }

    async fn capture_source_context(&self, cwd: &str) -> Option<ConversationSourceContext> {
        let repos = self.inner.repos.get()?;
        let path = Path::new(cwd);
        // Three `git` processes: side by side, not one after another.
        let (identity, branch, head_sha) = tokio::join!(
            repos.checkout_identity(path),
            repos.current_branch(path),
            repos.head_sha(path),
        );
        let identity = identity.ok()?;
        let branch = branch.ok()?;
        let head_sha = head_sha.ok().flatten();
        Some(ConversationSourceContext {
            checkout_id: identity.id,
            repo_root: identity.root.to_string_lossy().into_owned(),
            cwd: cwd.to_string(),
            branch,
            head_sha,
            observed_at: chrono::Utc::now(),
        })
    }

    /// Every fresh harness dispatch crosses this boundary, including
    /// steer/input fallbacks and crash recovery. The conversation records
    /// the checkout the harness will actually observe, rather than whichever
    /// checkout state was current on an earlier turn: read as the dispatch
    /// starts, beside it rather than before it. Its `git` processes took
    /// ~25 ms (M5 Max; more on slower Macs) in front of every agent start,
    /// and the agent reads nothing of its checkout for far longer than that
    /// (its own startup is ~250 ms before its first request).
    pub(crate) async fn dispatch_with_source_context(
        &self,
        sessions: &SessionsEngine,
        chat_id: &str,
        harness: HarnessId,
        request: commutator_proto::RunRequest,
        message_id: Option<String>,
    ) -> Result<String, EngineError> {
        // Stamped on a task of its own: the drain resolves the command as
        // soon as the run is dispatched, never behind the `git` processes.
        if self.workspace().is_some() {
            let host = self.clone();
            let chat_id = chat_id.to_owned();
            let cwd = request.cwd.clone();
            self.spawn_worker(async move {
                if let Some(workspace) = host.workspace()
                    && let Some(context) = host.capture_source_context(&cwd).await
                    && let Err(err) = workspace.set_chat_source_context(&chat_id, &context)
                {
                    tracing::warn!(chat = %chat_id, error = %err, "conversation source stamp failed");
                }
                tracing::debug!(target: "first_word", chat = %chat_id, "source context: stamped");
            });
        }
        sessions
            .dispatch(chat_id, harness, request, message_id)
            .await
    }

    async fn materialize_worktree(
        &self,
        chat_id: &str,
        spec: &commutator_proto::WorktreeSpec,
    ) -> Result<(String, Option<commutator_proto::Worktree>), EngineError> {
        if let Some(ws) = self.workspace()
            && let Ok(Some(chat)) = ws.chat(chat_id)
            && let Some(cwd) = chat.cwd
            && crate::workspace_host::linked_worktree_root(std::path::Path::new(&cwd)).is_some_and(
                |root| same_file::is_same_file(&root, &spec.repo_path).unwrap_or(false),
            )
        {
            tracing::info!(chat = %chat_id, cwd = %cwd, "worktree spec: reusing the chat's existing worktree");
            return Ok((cwd, None));
        }
        let repos = self
            .inner
            .repos
            .get()
            .ok_or_else(|| EngineError::Other("repos engine not wired".into()))?;
        let worktree = repos
            .create_worktree(std::path::Path::new(&spec.repo_path), &spec.base)
            .await?;
        tracing::info!(
            chat = %chat_id,
            path = %worktree.path,
            branch = %worktree.branch,
            "worktree materialized for run"
        );
        Ok((worktree.path.clone(), Some(worktree)))
    }

    fn complete_worktree_setup_handoff(
        &self,
        command_id: &str,
        chat_id: &str,
        spec: &commutator_proto::WorktreeSpec,
        fresh_worktree: Option<&commutator_proto::Worktree>,
    ) {
        let Some((project_actions, terminals)) = self.inner.project_action_runtime.get() else {
            return;
        };
        let outcome = match (spec.space_id.as_deref(), fresh_worktree) {
            (Some(space_id), Some(worktree)) => self
                .resolve_and_launch_worktree_setup(
                    project_actions,
                    terminals,
                    space_id,
                    spec,
                    worktree,
                )
                .unwrap_or_else(|err| ProjectActionSetupHandoff {
                    setup_action: None,
                    setup_error: Some(err.to_string()),
                }),
            _ => ProjectActionSetupHandoff {
                setup_action: None,
                setup_error: None,
            },
        };
        if let Some(run) = &outcome.setup_action {
            let _ = terminals.associate_chat(&run.terminal.id, chat_id);
        }
        project_actions.complete_setup_handoff(command_id, chat_id, outcome);
    }

    fn resolve_and_launch_worktree_setup(
        &self,
        project_actions: &ProjectActionsStore,
        terminals: &Terminals,
        space_id: &str,
        spec: &commutator_proto::WorktreeSpec,
        worktree: &commutator_proto::Worktree,
    ) -> Result<ProjectActionSetupHandoff, EngineError> {
        let workspace = self
            .workspace()
            .ok_or_else(|| EngineError::Other("workspace host not wired".into()))?;
        let space = workspace
            .space(space_id)?
            .ok_or_else(|| EngineError::Other("Project not found".into()))?;
        if space.device_id != self.inner.config.device_id {
            return Err(EngineError::Other(
                "Project belongs to another device".into(),
            ));
        }
        let project_root = std::fs::canonicalize(&space.path)?;
        let requested_root = std::fs::canonicalize(&spec.repo_path)?;
        if project_root != requested_root {
            return Err(EngineError::Other(
                "Project path does not match worktree repository".into(),
            ));
        }
        // The store keys configuration by the original Space path, which may
        // be a symlink. Keep canonical paths for validation and execution only.
        let setup_action = project_actions
            .setup_action(space_id, std::path::Path::new(&space.path))?
            .map(|action| {
                launch_project_setup_action(
                    terminals,
                    &action,
                    &project_root,
                    std::path::Path::new(&worktree.path),
                    &worktree.branch,
                    120,
                    32,
                )
            })
            .transpose()?;
        Ok(ProjectActionSetupHandoff {
            setup_action,
            setup_error: None,
        })
    }

    /// A steer-turned-run with no in-process `last_request` (engine restarted
    /// since the last turn): rebuild the run config from the chat's workspace
    /// row — cwd from the row, model/reasoning/options/sandbox from its config
    /// (composer defaults otherwise). `None` without a workspace host or row.
    // (Also the RespondInput dead-run fallback's config source.)
    pub(crate) fn request_from_chat_row(
        &self,
        chat_id: &str,
        prompt: &str,
    ) -> Option<commutator_proto::RunRequest> {
        let workspace = self.workspace()?;
        let chat = match workspace.chat(chat_id) {
            Ok(chat) => chat?,
            Err(err) => {
                tracing::warn!(chat = %chat_id, error = %err, "workspace chat read failed");
                return None;
            }
        };
        let config = chat.config;
        Some(commutator_proto::RunRequest {
            mcp: None,
            prompt: prompt.to_string(),
            harness: config.as_ref().map(|c| c.harness),
            model: config.as_ref().and_then(|c| c.model.clone()),
            reasoning: config.as_ref().and_then(|c| c.reasoning),
            model_options: config
                .as_ref()
                .map(|c| c.model_options.clone())
                .unwrap_or_default(),
            cwd: chat.cwd.unwrap_or_default(),
            sandbox: config
                .as_ref()
                .map(|c| c.sandbox)
                .unwrap_or(commutator_proto::SandboxLevel::WorkspaceWrite),
            auto_approve: false,
            access: config.as_ref().map(|c| c.access).unwrap_or_default(),
            plan: config.as_ref().is_some_and(|c| c.plan),
            code_mode: false,
            attachments: Vec::new(),
            resume: None,
            worktree: None,
            env: Default::default(),
            extra_mcp: Vec::new(),
        })
    }

    fn save_snapshot(&self, handle: &ChatDocHandle) {
        self.persist(handle, false);
    }

    /// Save with any snapshot deltas folded in: the handle is going away
    /// (eviction, shutdown), so the next open reads one blob.
    fn save_snapshot_compacted(&self, handle: &ChatDocHandle) {
        self.persist(handle, true);
    }

    fn persist(&self, handle: &ChatDocHandle, compact: bool) {
        if handle.retired.load(Ordering::Relaxed) {
            // A chat2 seed replaced this lineage on disk; persisting this
            // handle's fat doc would clobber the thin one. But retired with
            // NO thin lineage on disk (a stuck handle from the born-gen2
            // race) means this doc is its transcript's only copy — skipping
            // the save turned an app quit into total loss (2026-08-11);
            // persist it, and let the adopt path convert it on reopen.
            // The epoch column alone: no blob read or delta merge.
            let thin_on_disk = matches!(
                self.inner.store.snapshot_epoch(&handle.chat_id),
                Ok(epoch) if epoch >= crate::chat2_host::CHAT2_DOC_EPOCH
            );
            if thin_on_disk {
                return;
            }
        }
        if let Some(persistence) = &handle.persistence {
            if compact {
                persistence.compact_sync();
            } else {
                persistence.flush_sync();
            }
            return;
        }
        match handle.doc.export_snapshot() {
            Ok(bytes) => {
                handle.snapshot_bytes.store(bytes.len(), Ordering::Relaxed);
                if let Err(err) = self.inner.store.save_snapshot(&handle.chat_id, &bytes) {
                    tracing::warn!(chat = %handle.chat_id, error = %err, "snapshot save failed");
                }
            }
            Err(err) => {
                tracing::warn!(chat = %handle.chat_id, error = %err, "snapshot export failed");
            }
        }
    }

    /// A fork must be durable before publishing its discoverable registry row.
    pub(crate) fn persist_fork(&self, handle: &ChatDocHandle) -> Result<(), EngineError> {
        let bytes = handle.doc.export_snapshot()?;
        self.inner.store.save_snapshot(&handle.chat_id, &bytes)?;
        handle.snapshot_bytes.store(bytes.len(), Ordering::Relaxed);
        Ok(())
    }

    /// Persist every open doc now (shutdown path; bypasses the debounce).
    /// The store's WAL folded into its file ([`DocsStore::flush_for_sleep`]):
    /// before a Cloud sleep's snapshot, and at shutdown.
    pub fn checkpoint_store(&self) -> bool {
        self.inner.store.flush_for_sleep()
    }

    /// The store this host writes (its path, and what its open recovered).
    pub fn store(&self) -> &Arc<DocsStore> {
        &self.inner.store
    }

    /// Before a restart into another version: no more snapshot deltas, every
    /// open chat saved whole (deltas folded in), and the WAL in the file. An
    /// older version ignores the deltas table, so whatever this leaves there
    /// it would never see; after this there is nothing.
    pub fn prepare_for_version_change(&self) {
        self.inner.store.stop_snapshot_deltas();
        self.flush_all();
        self.checkpoint_store();
    }

    pub fn flush_all(&self) {
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            self.save_snapshot_compacted(&handle);
        }
    }

    /// Close all account-scoped room memberships before graceful engine
    /// draining. Auth-aware join supervisors will not install a late client.
    pub fn disconnect_edge(&self) {
        self.inner.edge_disconnected.store(true, Ordering::Release);
        let handles: Vec<_> = lock(&self.inner.handles).values().cloned().collect();
        for handle in handles {
            lock(&handle.chat2).take();
            // Retain the durable subscription through agent shutdown cleanup.
        }
    }
}

/// The resumed-turn prompt for answers to a question whose run died: each
/// answer paired with its question text so the reattached conversation reads
/// naturally. Pure.
pub fn respond_input_prompt(
    questions: &[UserInputQuestion],
    answers: &[UserInputAnswer],
) -> String {
    let mut lines = vec!["Answering your earlier question:".to_string()];
    for answer in answers {
        let picked = answer.labels.join(", ");
        let question = questions
            .iter()
            .find(|q| q.id == answer.question_id)
            .map(|q| q.question.trim())
            .filter(|q| !q.is_empty());
        match question {
            Some(question) => lines.push(format!("{question} — {picked}")),
            None => lines.push(picked),
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod transfer_progress_tests {
    use super::{DocHost, DocHostConfig, TransferProgressGuard};
    use std::sync::Arc;

    fn host() -> (tempfile::TempDir, DocHost) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(commutator_sync::DocsStore::open(dir.path()).expect("store opens"));
        let host = DocHost::new(
            store,
            DocHostConfig {
                device_id: "dev-test".into(),
                default_harness: commutator_proto::HarnessId::Mock,
            },
        );
        (dir, host)
    }

    /// A commit in a long, watched chat (a reply streaming in a whale)
    /// publishes for the entry it changed, not by reading every entry
    /// again: V3 Release (7,900 entries, 49 MB) paid ~200 ms a commit, more
    /// than the 120 ms tick, and sends waited behind it.
    #[tokio::test]
    async fn a_commit_in_a_watched_whale_costs_what_it_changed() {
        let (_dir, host) = host();
        let source = commutator_doc::SessionDoc::init("watched-whale").unwrap();
        for i in 0..3000 {
            source
                .push_message(&commutator_doc::SessionMessageEntry {
                    id: format!("row-{i}"),
                    role: commutator_doc::MessageRole::User,
                    parts: vec![commutator_doc::MessagePart::Text {
                        id: "text".into(),
                        text: "x".repeat(2048),
                    }],
                    created_at: i,
                    device_id: "remote".into(),
                    status: None,
                    continuation_of: None,
                    duration_ms: None,
                })
                .unwrap();
        }
        host.inner
            .store
            .save_snapshot_with_cursor("watched-whale", &source.export_snapshot().unwrap(), 0, 2)
            .unwrap();
        drop(source);
        let handle = host.open("watched-whale").unwrap();
        let start = std::time::Instant::now();
        let rx = handle.watch_messages();
        let attach = start.elapsed();
        assert_eq!(rx.borrow().entries.len(), 3000);

        let mut writer = commutator_doc::SegmentWriter::begin(handle.doc(), "reply", "dev-test", 1).unwrap();
        let mut commits = std::time::Duration::ZERO;
        for n in 1..=5 {
            let words = "streamed ".repeat(n);
            writer
                .sync(&[commutator_doc::MessagePart::Text { id: "t".into(), text: words.clone() }])
                .unwrap();
            let start = std::time::Instant::now();
            handle.publish_messages_if_watched();
            commits += start.elapsed();
            let published = rx.borrow().entries.clone();
            assert_eq!(published.len(), 3001);
            assert_eq!(
                published.last().unwrap().parts,
                vec![commutator_doc::MessagePart::Text { id: "t".into(), text: words }],
            );
        }
        let commit = commits / 5;
        eprintln!("watched whale: attach {attach:?}, a commit {commit:?}");
        assert!(
            commit * 5 < attach,
            "a commit ({commit:?}) should cost far less than reading the whole chat ({attach:?})"
        );
        let published: Vec<_> = rx.borrow().entries.iter().map(|e| (**e).clone()).collect();
        assert_eq!(handle.doc().read_entries_whole().unwrap(), published);
    }

    /// The open chat's waiting-messages watch runs on every change to it:
    /// with nothing waiting it reads no transcript (beta.5 read and copied
    /// every entry per commit, on a runtime worker).
    #[tokio::test]
    async fn waiting_messages_in_a_whale_cost_what_waits() {
        let (_dir, host) = host();
        let source = commutator_doc::SessionDoc::init("waiting-whale").unwrap();
        for i in 0..3000 {
            source
                .push_message(&commutator_doc::SessionMessageEntry {
                    id: format!("row-{i}"),
                    role: commutator_doc::MessageRole::User,
                    parts: vec![commutator_doc::MessagePart::Text {
                        id: "text".into(),
                        text: "x".repeat(2048),
                    }],
                    created_at: i,
                    device_id: "remote".into(),
                    status: None,
                    continuation_of: None,
                    duration_ms: None,
                })
                .unwrap();
        }
        host.inner
            .store
            .save_snapshot_with_cursor("waiting-whale", &source.export_snapshot().unwrap(), 0, 2)
            .unwrap();
        drop(source);
        let handle = host.open("waiting-whale").unwrap();
        let start = std::time::Instant::now();
        let whole = handle.doc().read_entries_whole().unwrap();
        let read = start.elapsed();
        assert_eq!(whole.len(), 3000);
        assert!(host.waiting_messages("waiting-whale").unwrap().is_empty());
        let start = std::time::Instant::now();
        for _ in 0..5 {
            assert!(host.waiting_messages("waiting-whale").unwrap().is_empty());
        }
        let each = start.elapsed() / 5;
        eprintln!("waiting whale: whole read {read:?}, waiting_messages {each:?}");
        assert!(each * 5 < read, "waiting_messages ({each:?}) should cost far less than a whole read ({read:?})");
    }

    #[tokio::test]
    async fn whale_snapshot_opens_and_reopens_without_network() {
        let (_dir, host) = host();
        let source = commutator_doc::SessionDoc::init("persisted-whale").unwrap();
        for i in 0..2000 {
            source
                .push_message(&commutator_doc::SessionMessageEntry {
                    id: format!("row-{i}"),
                    role: commutator_doc::MessageRole::User,
                    parts: vec![commutator_doc::MessagePart::Text {
                        id: "text".into(),
                        text: "x".repeat(2048),
                    }],
                    created_at: i,
                    device_id: "remote".into(),
                    status: None,
                    continuation_of: None,
                    duration_ms: None,
                })
                .unwrap();
        }
        host.inner
            .store
            .save_snapshot_with_cursor("persisted-whale", &source.export_snapshot().unwrap(), 0, 2)
            .unwrap();
        drop(source);
        let start = std::time::Instant::now();
        let handle = host.open("persisted-whale").unwrap();
        let rx = handle.watch_messages();
        assert_eq!(rx.borrow().entries.len(), 2000);
        eprintln!("offline whale cold open: {:?}", start.elapsed());
        drop(rx);
        // An unwatched commit clears the mirror; attach still serves local data.
        handle.publish_messages_if_watched();
        let start = std::time::Instant::now();
        assert_eq!(handle.watch_messages().borrow().entries.len(), 2000);
        eprintln!("offline whale rebuilt mirror: {:?}", start.elapsed());
    }

    #[tokio::test]
    async fn transcript_attach_and_unwatched_clear_share_a_critical_section() {
        let (_dir, host) = host();
        let handle = host.open("cached").unwrap();
        handle
            .write_user_message("row", "locally persisted transcript", 0)
            .unwrap();
        let rx = handle.watch_messages();
        assert_eq!(rx.borrow().entries.len(), 1);
        drop(rx);

        // Freeze attach's critical section. An unwatched publisher must not
        // pass its receiver check and clear the mirror while attach owns it.
        let guard = super::lock(&handle.transcript_import);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker_handle = handle.clone();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            worker_handle.publish_messages_if_watched();
            done_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        let result = done_rx.recv_timeout(std::time::Duration::from_millis(100));
        // Simulate the subscription attaching before the worker can inspect it.
        let rx = handle.messages_tx.subscribe();
        drop(guard);
        worker.join().unwrap();
        assert!(
            matches!(result, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
            "unwatched clear escaped attach's critical section"
        );
        assert_eq!(rx.borrow().entries.len(), 1, "no empty reset after attach");
        drop(rx);
        handle.publish_messages_if_watched();
        assert!(handle.messages_tx.borrow().entries.is_empty());
        assert_eq!(
            handle.watch_messages().borrow().entries.len(),
            1,
            "offline reopen rebuilds from local content"
        );
    }

    #[test]
    fn set_upserts_by_upload_id_and_clear_retires_only_its_entry() {
        let (_dir, host) = host();
        let rx = host.watch_transfers();
        assert!(rx.borrow().is_empty());

        host.transfer_progress_set("u1", "a.png", 0, 1_000);
        host.transfer_progress_set("u2", "b.png", 0, 400);
        host.transfer_progress_set("u1", "a.png", 300, 1_000);
        let snapshot = rx.borrow().clone();
        assert_eq!(snapshot.len(), 2, "upsert must not duplicate u1");
        let u1 = snapshot.iter().find(|t| t.upload_id == "u1").unwrap();
        assert_eq!((u1.done, u1.total), (300, 1_000));

        host.transfer_progress_clear("u1");
        let snapshot = rx.borrow().clone();
        assert_eq!(snapshot.len(), 1, "u2 must survive u1's retirement");
        assert_eq!(snapshot[0].upload_id, "u2");
    }

    #[test]
    fn guard_retires_the_entry_on_every_exit_path() {
        let (_dir, host) = host();
        let rx = host.watch_transfers();
        host.transfer_progress_set("u1", "a.png", 0, 9);
        {
            let _guard = TransferProgressGuard {
                host: &host,
                upload_id: "u1",
            };
            assert_eq!(rx.borrow().len(), 1);
            // An early return / error propagation drops the guard here.
        }
        assert!(
            rx.borrow().is_empty(),
            "a failed attempt must not leave a phantom ring behind"
        );
    }

    #[test]
    fn late_subscriber_sees_current_set_first() {
        let (_dir, host) = host();
        host.transfer_progress_set("u1", "a.png", 750, 1_000);
        // watch_stream's contract: current value first, then changes — a UI
        // attaching mid-transfer must render the ring immediately.
        let rx = host.watch_transfers();
        assert_eq!(rx.borrow().len(), 1);
        assert_eq!(rx.borrow()[0].done, 750);
    }
}

#[cfg(test)]
mod source_context_tests {
    use super::{DocHost, DocHostConfig};
    use std::process::Command;
    use std::sync::Arc;

    fn git(repo: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    #[tokio::test]
    async fn capture_source_context_reads_the_dispatch_checkout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "feature/captured"]);
        git(&repo, &["config", "user.name", "Commutator Test"]);
        git(&repo, &["config", "user.email", "commutator@example.com"]);
        std::fs::write(repo.join("README.md"), "capture\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "capture"]);

        let store =
            Arc::new(commutator_sync::DocsStore::open(dir.path().join("docs")).expect("store opens"));
        let host = DocHost::new(
            store,
            DocHostConfig {
                device_id: "device-a".into(),
                default_harness: commutator_proto::HarnessId::Mock,
            },
        );
        host.set_repos(crate::repos::Repos::new(
            &dir.path().join("data"),
            "device-a",
        ));

        let context = host
            .capture_source_context(repo.to_str().unwrap())
            .await
            .expect("source context");
        assert_eq!(context.branch, "feature/captured");
        assert_eq!(context.cwd, repo.to_string_lossy());
        assert_eq!(
            context.repo_root,
            repo.canonicalize().unwrap().to_string_lossy()
        );
        assert!(context.head_sha.is_some());
        assert!(!context.checkout_id.is_empty());
    }
}

#[cfg(test)]
mod queued_message_prompt_tests {
    use super::{ATTACHMENT_ONLY_PROMPT, ATTACHMENT_PROMPT_HEADER, queued_message_prompt};

    #[test]
    fn dispatch_adds_the_attachment_transport_to_visible_queue_text() {
        let paths = vec!["/tmp/image.png".to_string()];
        assert_eq!(
            queued_message_prompt("inspect this", &paths),
            format!("inspect this\n\n{ATTACHMENT_PROMPT_HEADER}\n- /tmp/image.png")
        );
    }

    #[test]
    fn legacy_expanded_rows_are_not_expanded_twice() {
        let paths = vec!["/tmp/image.png".to_string()];
        let legacy = format!("inspect this\n\n{ATTACHMENT_PROMPT_HEADER}\n- /tmp/image.png");
        assert_eq!(queued_message_prompt(&legacy, &paths), legacy);
    }

    #[test]
    fn attachment_only_rows_get_a_non_empty_prompt_body() {
        let paths = vec!["/tmp/image.png".to_string()];
        assert_eq!(
            queued_message_prompt("", &paths),
            format!("{ATTACHMENT_ONLY_PROMPT}\n\n{ATTACHMENT_PROMPT_HEADER}\n- /tmp/image.png")
        );
    }
}

/// Per-chat background task: reacts to doc changes (local commits and remote imports)
/// by re-publishing the transcript watch, draining commands, and debouncing snapshots.
/// Holds only a weak handle so a dropped host tears the task down.
async fn chat_task(host: DocHost, weak: Weak<ChatDocHandle>, mut changed_rx: watch::Receiver<u64>) {
    // Prompt delivery may wait for mailbox capacity. Keep a separate watcher
    // for interrupt/question controls so that wait cannot block recovery.
    let control_host = host.clone();
    let control_weak = weak.clone();
    let mut control_changes = changed_rx.clone();
    host.spawn_worker(async move {
        loop {
            let Some(handle) = control_weak.upgrade() else {
                break;
            };
            control_host.drain_command_kind(&handle, true).await;
            drop(handle);
            if control_changes.changed().await.is_err() {
                break;
            }
        }
    });
    // Initial pass: the snapshot may already carry pending commands. The
    // mirror stays lazy — it materializes on the first watch attach.
    {
        let Some(handle) = weak.upgrade() else { return };
        host.drain_commands(&handle).await;
        host.drain_queue(&handle).await;
    }
    let mut save_deadline: Option<tokio::time::Instant> = None;
    loop {
        let sleep_until = save_deadline.unwrap_or_else(tokio::time::Instant::now);
        tokio::select! {
            changed = changed_rx.changed() => {
                if changed.is_err() {
                    break; // doc handle (and its change sender) is gone
                }
                let Some(handle) = weak.upgrade() else { break };
                let publishing = handle.clone();
                // The watch's publish runs beside the drains, not before them:
                // a send's command (or a queued row) is taken at once, never
                // behind a long chat's publish. The next pass waits for it.
                let published = tokio::task::spawn_blocking(move || {
                    publishing.publish_messages_if_watched();
                    publishing.publish_queue();
                });
                host.drain_commands(&handle).await;
                host.drain_queue(&handle).await;
                let _ = published.await;
                if save_deadline.is_none() {
                    save_deadline = Some(
                        tokio::time::Instant::now()
                            + std::time::Duration::from_millis(SNAPSHOT_DEBOUNCE_MS),
                    );
                }
            }
            _ = tokio::time::sleep_until(sleep_until), if save_deadline.is_some() => {
                save_deadline = None;
                let Some(handle) = weak.upgrade() else { break };
                // chat2 has its own coalescing blocking-pool persister. The
                // legacy worker must not duplicate every scheduled export.
                if handle.persistence.is_none() { host.save_snapshot(&handle); }
                // Post-quiesce eviction pass: sizes just refreshed.
                host.evict_over_budget();
            }
        }
    }
}

#[cfg(test)]
mod publication_eviction_tests {
    use super::*;

    /// A clean stop (flush_all: shutdown, Cloud sleep) and the step before a
    /// restart into another version leave no snapshot deltas: an older
    /// version ignores that table, so anything left there it would lose.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_clean_stop_leaves_zero_snapshot_deltas() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store.clone(),
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
            },
        );
        let ids = ["one", "two", "three"];
        let handles: Vec<_> = ids.iter().map(|id| host.open_local(id).unwrap()).collect();
        let edit = |round: usize| {
            for handle in &handles {
                let text = handle.doc.doc().get_text("body");
                text.insert(text.len_unicode(), &format!("round {round} ")).unwrap();
                handle.doc.doc().commit();
                handle.persistence.as_ref().unwrap().flush_sync();
            }
        };
        let deltas = || ids.iter().map(|id| store.snapshot_delta_stats(id).unwrap().0).sum::<u64>();
        for round in 0..3 {
            edit(round);
        }
        assert!(deltas() > 0, "the fixture made no deltas");
        host.flush_all();
        assert_eq!(deltas(), 0, "a clean stop left deltas behind");
        for handle in &handles {
            let (bytes, _, _) = store.load_snapshot_with_cursor(&handle.chat_id).unwrap().unwrap();
            let loaded = loro::LoroDoc::new();
            loaded.import(&bytes).unwrap();
            assert_eq!(loaded.get_deep_value(), handle.doc.doc().get_deep_value());
        }
        edit(3);
        assert!(deltas() > 0, "deltas resume after a clean stop that didn't end the process");
        host.prepare_for_version_change();
        assert_eq!(deltas(), 0, "a restart into another version left deltas behind");
        // Its last moments save whole: nothing new lands in the table.
        edit(4);
        edit(5);
        assert_eq!(deltas(), 0);
        drop(handles);
        host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn caller_and_watch_protect_the_open_to_attach_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let host = DocHost::new(
            Arc::new(DocsStore::open(dir.path()).unwrap()),
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
            },
        );
        let held = host.open_local("held").unwrap();
        for i in 0..30 {
            host.open_local(&format!("other-{i}")).unwrap();
        }
        assert!(lock(&host.inner.handles).contains_key("held"));
        let watch = held.watch_messages();
        drop(held);
        for i in 30..60 {
            host.open_local(&format!("other-{i}")).unwrap();
        }
        assert!(lock(&host.inner.handles).contains_key("held"));
        drop(watch);
        host.open_local("overflow").unwrap();
        assert!(!lock(&host.inner.handles).contains_key("held"));
        host.shutdown_workers().await;
    }

    /// Twelve chats open and read (as their drains read them), one watched:
    /// only the watched one keeps its transcript's entries, and the memory
    /// budget counts them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn only_a_watched_chat_keeps_its_read_cache() {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store,
            DocHostConfig { device_id: "here".into(), default_harness: HarnessId::Mock },
        );
        let handles: Vec<_> = (0..12).map(|i| host.open(&format!("chat-{i}")).unwrap()).collect();
        for handle in &handles {
            for m in 0..100 {
                handle.write_user_message(&format!("m{m}"), &"words ".repeat(40), m).unwrap();
            }
        }
        let watch = handles[0].watch_messages();
        for handle in &handles {
            assert_eq!(handle.doc().read_entries().unwrap().len(), 100);
            handle.write_user_message("later", "a change", 500).unwrap();
            handle.publish_messages_if_watched();
            assert_eq!(handle.doc().read_entries().unwrap().len(), 101);
        }
        assert_eq!(handles[0].doc().entries_cached(), 101, "the watched chat keeps its entries");
        let watched = handles[0].doc().cached_bytes();
        assert!(watched > 100 * 240, "its entries' bytes are counted: {watched}");
        for handle in &handles[1..] {
            assert_eq!(handle.doc().entries_cached(), 0, "an unwatched chat keeps no entries");
        }
        let floor = handles[1].resident_estimate();
        assert!(handles[0].resident_estimate() >= floor + watched, "the budget counts the watched chat's cache");
        // Let go of: on its next change it keeps nothing either.
        drop(watch);
        handles[0].write_user_message("after", "a change", 600).unwrap();
        handles[0].publish_messages_if_watched();
        assert_eq!(handles[0].doc().entries_cached(), 0);
        drop(handles);
        host.shutdown_workers().await;
    }

    /// A chat leaving the engine's cache takes its doc with it, read cache
    /// and all: what `read_entries` keeps lives in the doc, not beside it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_evicted_chat_frees_its_doc_and_read_cache() {
        let _budget_guard = SYNC_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store,
            DocHostConfig { device_id: "here".into(), default_harness: HarnessId::Mock },
        );
        let handle = host.open("big").unwrap();
        for i in 0..200 {
            handle.write_user_message(&format!("m{i}"), "words", i).unwrap();
        }
        // Watched and read, as a chat looked at, then let go.
        let watch = handle.watch_messages();
        assert_eq!(handle.doc().read_entries().unwrap().len(), 200);
        assert_eq!(handle.doc().entries_cached(), 200);
        drop(watch);
        // A change while nobody watches lets the read cache go.
        handle.write_user_message("unwatched", "words", 999).unwrap();
        handle.publish_messages_if_watched();
        assert_eq!(handle.doc().entries_cached(), 0, "an unwatched chat keeps no second copy");
        let doc = Arc::downgrade(&handle.doc_arc());
        drop(handle);
        for i in 0..WARM_DOC_CAP {
            host.open(&format!("other-{i}")).unwrap();
        }
        host.open("overflow").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                host.evict_over_budget();
                if !lock(&host.inner.handles).contains_key("big") && doc.upgrade().is_none() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the evicted chat's doc (and its read cache) is freed");
        host.shutdown_workers().await;
    }

}

#[path = "doc_host_attachments.rs"]
mod attachment_store;
