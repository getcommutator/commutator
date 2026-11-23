//! Per-turn git checkpoints of a chat's checkout, and rollback to them (the
//! design of t3code's `CheckpointService`).
//!
//! When a turn starts and when its `Done` closes it, the checkout's working
//! tree (tracked + untracked, not ignored) is written as a git tree with the
//! same bounded temp-index snapshot as the "Latest turn" diff
//! ([`crate::diff_sync::snapshot_tree_kept`]), wrapped in a commit and kept
//! under `refs/vardoger/checkpoints/{chat}/{id}`.
//!
//! Those refs and objects live in a store of Vardoger's, not in the checkout:
//! a bare repository `{data_dir}/checkpoints/stores/{key}.git` per
//! repository whose `objects/info/alternates` names the checkout's objects.
//! The checkout's index, HEAD, refs, stash and object database are never
//! written (see `turn_snapshot` for why its object database stays clean).
//! Metadata goes in `{data_dir}/checkpoints/chats/{chat}.json`, the engine's
//! usual JSON-file persistence (`repos.json`, worktree settings).
//!
//! Capture never blocks a turn: it runs on a per-chat queue in the
//! background, and a failure only logs. A cwd outside git records nothing.
//!
//! Limits: blobs the snapshot found in the checkout's own database are not
//! copied, so a checkout gc that prunes an unreachable blob (content that
//! was only ever staged) makes the checkpoints that used it unrestorable.
//! Untracked paths over the snapshot budget are not recorded, and a rollback
//! leaves them as they are.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;
use vardoger_proto::checkpoints::{Checkpoint, CheckpointPhase, RollbackToCheckpointResponse};

use crate::EngineError;
use crate::diff_sync::{DiffSnapshot, TreeSnapshot, capture_tree_diff, snapshot_tree_kept};
use crate::repos::Repos;

/// Checkpoints kept per chat; older ones (and their refs) are dropped.
pub const MAX_CHECKPOINTS_PER_CHAT: usize = 100;
const REF_PREFIX: &str = "refs/vardoger/checkpoints";
/// Unreachable objects younger than this survive a prune: a capture in
/// flight has written its blobs but not yet its ref.
const PRUNE_EXPIRE: &str = "1.hour.ago";

/// Whether a turn is running in a chat.
pub type BusyProbe = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatLog {
    next_id: u64,
    checkpoints: Vec<Stored>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    #[serde(flatten)]
    meta: Checkpoint,
    tree: String,
    store: PathBuf,
    /// Untracked paths the snapshot left out (budget), as git spells them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skipped: Vec<Vec<u8>>,
}

impl Stored {
    fn snapshot(&self) -> TreeSnapshot {
        TreeSnapshot::kept(&self.store, self.tree.clone(), self.skipped.clone())
    }
}

#[derive(Clone)]
pub struct Checkpoints {
    inner: Arc<Inner>,
}

struct Inner {
    repos: Repos,
    dir: PathBuf,
    /// Per chat, the last queued job: every job awaits the one before it, so
    /// a chat's captures, rollbacks and deletion run one at a time, in order.
    queues: Mutex<HashMap<String, JoinHandle<()>>>,
    busy: OnceLock<BusyProbe>,
    /// [`MAX_CHECKPOINTS_PER_CHAT`]; lower in tests.
    cap: usize,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Checkpoints {
    pub fn new(repos: Repos) -> Self {
        let dir = repos.data_dir().join("checkpoints");
        Self {
            inner: Arc::new(Inner {
                repos,
                dir,
                queues: Mutex::new(HashMap::new()),
                busy: OnceLock::new(),
                cap: MAX_CHECKPOINTS_PER_CHAT,
            }),
        }
    }

    /// Wire the running-turn check rollbacks refuse on (engine assembly).
    pub fn set_busy_probe(&self, probe: BusyProbe) {
        let _ = self.inner.busy.set(probe);
    }

    /// A turn starts in `cwd`: checkpoint the tree it starts from.
    pub fn note_turn_start(&self, chat_id: &str, cwd: &str) {
        let cwd = PathBuf::from(cwd);
        self.capture_in_background(chat_id, CheckpointPhase::TurnStart, Some(cwd));
    }

    /// The chat's turn ended: checkpoint the tree it left.
    pub fn note_turn_end(&self, chat_id: &str) {
        self.capture_in_background(chat_id, CheckpointPhase::TurnEnd, None);
    }

    fn capture_in_background(&self, chat_id: &str, phase: CheckpointPhase, cwd: Option<PathBuf>) {
        let inner = self.inner.clone();
        let chat = chat_id.to_string();
        drop(self.enqueue(chat_id, async move {
            if let Err(error) = capture(&inner, &chat, phase, cwd).await {
                tracing::warn!(chat = %chat, ?phase, %error, "checkpoint capture failed");
            }
        }));
    }

    /// Run `job` after everything queued for the chat before it.
    fn enqueue<T: Send + 'static>(
        &self,
        chat_id: &str,
        job: impl Future<Output = T> + Send + 'static,
    ) -> tokio::sync::oneshot::Receiver<T> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut queues = lock(&self.inner.queues);
        let previous = queues.remove(chat_id);
        let handle = tokio::spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            let _ = tx.send(job.await);
        });
        queues.insert(chat_id.to_string(), handle);
        rx
    }

    async fn queued<T: Send + 'static>(
        &self,
        chat_id: &str,
        job: impl Future<Output = Result<T, EngineError>> + Send + 'static,
    ) -> Result<T, EngineError> {
        self.enqueue(chat_id, job)
            .await
            .map_err(|_| EngineError::Other("checkpoint job did not finish".into()))?
    }

    /// The chat's checkpoints, oldest first.
    pub async fn list(&self, chat_id: &str) -> Result<Vec<Checkpoint>, EngineError> {
        let inner = self.inner.clone();
        let chat = chat_id.to_string();
        self.queued(chat_id, async move {
            let log = load(&inner.dir, &chat).await?;
            Ok(log.checkpoints.into_iter().map(|c| c.meta).collect())
        })
        .await
    }

    /// Diff from checkpoint `from` to checkpoint `to`, or to the working
    /// tree now. Answers the checkout root with it.
    pub async fn diff(
        &self,
        chat_id: &str,
        from: u64,
        to: Option<u64>,
    ) -> Result<(PathBuf, DiffSnapshot), EngineError> {
        let inner = self.inner.clone();
        let chat = chat_id.to_string();
        self.queued(chat_id, async move {
            let log = load(&inner.dir, &chat).await?;
            let from = find(&log, from)?;
            let root = PathBuf::from(&from.meta.cwd);
            let base = from.snapshot();
            let to = match to {
                Some(to) => {
                    let to = find(&log, to)?;
                    if to.store != from.store {
                        return Err(EngineError::Other(
                            "the checkpoints are of different repositories".into(),
                        ));
                    }
                    to.snapshot()
                }
                None => snapshot_tree_kept(&root, &from.store, Some(&base)).await?,
            };
            let diff =
                capture_tree_diff(&inner.repos, &root, base.tree(), to.tree(), &to).await?;
            Ok((root, diff))
        })
        .await
    }

    /// Set the chat's working tree back to checkpoint `id`: files it had are
    /// written back, files it did not have are deleted (untracked ones too),
    /// ignored files are left alone. The index, HEAD and branch stay as they
    /// are. The tree it replaces is kept as a `BeforeRollback` checkpoint.
    pub async fn rollback(
        &self,
        chat_id: &str,
        id: u64,
    ) -> Result<RollbackToCheckpointResponse, EngineError> {
        let inner = self.inner.clone();
        let chat = chat_id.to_string();
        self.queued(chat_id, async move { rollback(&inner, &chat, id).await })
            .await
    }

    /// Drop a chat's checkpoints: its refs, then whatever only they held.
    pub async fn forget(&self, chat_id: &str) -> Result<(), EngineError> {
        let inner = self.inner.clone();
        let chat = chat_id.to_string();
        let result = self
            .queued(chat_id, async move {
                let log = load(&inner.dir, &chat).await?;
                let mut stores: Vec<&PathBuf> = log.checkpoints.iter().map(|c| &c.store).collect();
                stores.sort();
                stores.dedup();
                drop_refs(&log.checkpoints).await;
                for store in stores {
                    let _ = store_git(store, &["prune", &format!("--expire={PRUNE_EXPIRE}")], None)
                        .await;
                }
                match tokio::fs::remove_file(log_path(&inner.dir, &chat)).await {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
                    _ => Ok(()),
                }
            })
            .await;
        lock(&self.inner.queues).remove(chat_id);
        result
    }

    /// Forget the checkpoints of every chat whose row leaves the workspace
    /// (a row seen and then gone, as [`crate::SessionsEngine::follow_chat_deletions`]).
    pub fn follow_chat_deletions(
        &self,
        mut chats: tokio::sync::watch::Receiver<Vec<vardoger_proto::Chat>>,
    ) {
        let this = self.clone();
        let mut known: std::collections::HashSet<String> =
            chats.borrow_and_update().iter().map(|c| c.id.clone()).collect();
        tokio::spawn(async move {
            while chats.changed().await.is_ok() {
                let now: std::collections::HashSet<String> =
                    chats.borrow_and_update().iter().map(|c| c.id.clone()).collect();
                for gone in known.difference(&now) {
                    if let Err(error) = this.forget(gone).await {
                        tracing::warn!(chat = %gone, %error, "dropping checkpoints failed");
                    }
                }
                known = now;
            }
        });
    }
}

async fn capture(
    inner: &Inner,
    chat_id: &str,
    phase: CheckpointPhase,
    cwd: Option<PathBuf>,
) -> Result<(), EngineError> {
    let mut log = load(&inner.dir, chat_id).await?;
    let last_turn = log.checkpoints.iter().map(|c| c.meta.turn).max().unwrap_or(0);
    let (turn, cwd) = match (phase, cwd) {
        (CheckpointPhase::TurnStart, Some(cwd)) => (last_turn + 1, cwd),
        _ => {
            // The end belongs to the latest started turn, in its checkout.
            let Some(start) = log
                .checkpoints
                .iter()
                .rev()
                .find(|c| c.meta.phase == CheckpointPhase::TurnStart)
            else {
                return Ok(());
            };
            (start.meta.turn, PathBuf::from(&start.meta.cwd))
        }
    };
    let Ok(identity) = inner.repos.checkout_identity(&cwd).await else {
        // Not a git checkout (or gone): nothing to checkpoint.
        return Ok(());
    };
    let store = ensure_store(&inner.dir, &identity.root).await?;
    let snapshot = snapshot_tree_kept(&identity.root, &store, None).await?;
    let root = identity.root.to_string_lossy().into_owned();
    let stored = record(&mut log, chat_id, phase, turn, &root, &store, snapshot).await?;
    if phase == CheckpointPhase::TurnEnd {
        // A turn has one end: a second Done replaces the first.
        let stale: Vec<Stored> = log
            .checkpoints
            .iter()
            .filter(|c| {
                c.meta.turn == turn
                    && c.meta.phase == CheckpointPhase::TurnEnd
                    && c.meta.id != stored.meta.id
            })
            .cloned()
            .collect();
        log.checkpoints
            .retain(|c| !stale.iter().any(|s| s.meta.id == c.meta.id));
        drop_refs(&stale).await;
    }
    trim(&mut log, inner.cap).await;
    save(&inner.dir, chat_id, &log).await?;
    // Packs loose objects once there are many, and drops what trimmed refs
    // alone held. In the foreground of this queue, never the turn's.
    let _ = store_git(
        &store,
        &[
            "-c",
            "gc.autoDetach=false",
            "-c",
            &format!("gc.pruneExpire={PRUNE_EXPIRE}"),
            "gc",
            "--auto",
            "--quiet",
        ],
        None,
    )
    .await;
    Ok(())
}

/// Commit `snapshot`, point the chat's next ref at it, and add it to `log`.
async fn record(
    log: &mut ChatLog,
    chat_id: &str,
    phase: CheckpointPhase,
    turn: u32,
    root: &str,
    store: &Path,
    snapshot: TreeSnapshot,
) -> Result<Stored, EngineError> {
    let id = log.next_id;
    let git_ref = format!("{REF_PREFIX}/{}/{id}", ref_component(chat_id));
    let message = format!("vardoger checkpoint\n\nchat: {chat_id}\nturn: {turn}\nphase: {phase:?}\n");
    let commit = store_git(
        store,
        &["-c", "commit.gpgSign=false", "commit-tree", snapshot.tree(), "-m", &message],
        None,
    )
    .await?;
    let commit = String::from_utf8_lossy(&commit).trim().to_string();
    store_git(store, &["update-ref", &git_ref, &commit], None).await?;
    let previous = log.checkpoints.iter().rev().find(|c| c.meta.cwd == root);
    let files_changed = match previous {
        Some(previous) => Some(
            changed_paths(store, &previous.tree, snapshot.tree())
                .await
                .map(|paths| paths.len() as u32)?,
        ),
        None => None,
    };
    let stored = Stored {
        meta: Checkpoint {
            id,
            turn,
            phase,
            git_ref,
            commit,
            cwd: root.to_string(),
            created_at: chrono::Utc::now(),
            files_changed,
        },
        tree: snapshot.tree().to_string(),
        store: store.to_path_buf(),
        skipped: snapshot.skipped().to_vec(),
    };
    log.next_id += 1;
    log.checkpoints.push(stored.clone());
    Ok(stored)
}

async fn trim(log: &mut ChatLog, cap: usize) {
    let excess = log.checkpoints.len().saturating_sub(cap);
    if excess > 0 {
        let dropped: Vec<Stored> = log.checkpoints.drain(..excess).collect();
        drop_refs(&dropped).await;
    }
}

async fn rollback(
    inner: &Inner,
    chat_id: &str,
    id: u64,
) -> Result<RollbackToCheckpointResponse, EngineError> {
    if inner.busy.get().is_some_and(|busy| busy(chat_id)) {
        return Err(EngineError::Other(
            "a turn is running in this chat; stop it before rolling back".into(),
        ));
    }
    let mut log = load(&inner.dir, chat_id).await?;
    let target = find(&log, id)?.clone();
    let root = PathBuf::from(&target.meta.cwd);
    let identity = inner.repos.checkout_identity(&root).await?;
    if identity.root != root {
        return Err(EngineError::Other("the checkpoint's checkout moved".into()));
    }
    let base = target.snapshot();
    // Layered on the target: paths the budget left out on either side keep
    // the target's state, so they never show as changes and are left alone.
    let current = snapshot_tree_kept(&root, &target.store, Some(&base)).await?;
    let changes = name_status(&target.store, &target.tree, current.tree()).await?;
    let turn = log.checkpoints.iter().map(|c| c.meta.turn).max().unwrap_or(0);
    let before = record(
        &mut log,
        chat_id,
        CheckpointPhase::BeforeRollback,
        turn,
        &target.meta.cwd,
        &target.store,
        current,
    )
    .await?;
    trim(&mut log, inner.cap).await;
    save(&inner.dir, chat_id, &log).await?;

    let (deletes, restores): (Vec<_>, Vec<_>) =
        changes.into_iter().partition(|(status, _)| *status == b'A');
    for (_, path) in &deletes {
        remove_and_prune(&root, path)?;
    }
    if !restores.is_empty() {
        checkout_paths(&root, &target, restores.iter().map(|(_, path)| path)).await?;
    }
    Ok(RollbackToCheckpointResponse {
        restored: target.meta,
        before: before.meta,
        files_restored: restores.len() as u32,
        files_deleted: deletes.len() as u32,
    })
}

/// Write `paths` from the checkpoint's tree into the working tree, through a
/// throwaway index (the checkout's own index is not touched).
async fn checkout_paths(
    root: &Path,
    target: &Stored,
    paths: impl Iterator<Item = &Vec<u8>>,
) -> Result<(), EngineError> {
    let index = target.store.join(format!("index-{}", uuid::Uuid::new_v4()));
    let objects = target.store.join("objects");
    let run = |args: &'static [&'static str], tree: Option<&str>, stdin: Option<Vec<u8>>| {
        let mut cmd = git_command();
        cmd.arg("-C").arg(root).args(args);
        if let Some(tree) = tree {
            cmd.arg(tree);
        }
        cmd.env("GIT_INDEX_FILE", &index);
        cmd.env("GIT_OBJECT_DIRECTORY", &objects);
        run_git(cmd, stdin)
    };
    let mut list = Vec::new();
    for path in paths {
        list.extend_from_slice(path);
        list.push(0);
    }
    let result = async {
        run(&["read-tree"], Some(&target.tree), None).await?;
        run(&["checkout-index", "-f", "-z", "--stdin"], None, Some(list)).await
    }
    .await;
    let _ = tokio::fs::remove_file(&index).await;
    result.map(drop)
}

/// Delete a file the checkpoint did not have, then the directories it
/// leaves empty (never `root` itself).
fn remove_and_prune(root: &Path, path: &[u8]) -> Result<(), EngineError> {
    let full = root.join(path_from_git(path));
    match std::fs::remove_file(&full) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let mut dir = full.parent();
    while let Some(parent) = dir {
        if parent == root || !parent.starts_with(root) || std::fs::remove_dir(parent).is_err() {
            break;
        }
        dir = parent.parent();
    }
    Ok(())
}

#[cfg(unix)]
fn path_from_git(path: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(OsString::from_vec(path.to_vec()))
}

#[cfg(not(unix))]
fn path_from_git(path: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(path).replace('/', "\\"))
}

/// `(status, path)` of every path that differs between two trees; `A` means
/// only `to` has it. Submodules are not followed.
async fn name_status(
    store: &Path,
    from: &str,
    to: &str,
) -> Result<Vec<(u8, Vec<u8>)>, EngineError> {
    let out = store_git(
        store,
        &[
            "diff-tree",
            "-r",
            "-z",
            "--no-renames",
            "--ignore-submodules",
            "--name-status",
            from,
            to,
        ],
        None,
    )
    .await?;
    let fields: Vec<&[u8]> = out.split(|b| *b == 0).filter(|f| !f.is_empty()).collect();
    Ok(fields
        .chunks_exact(2)
        .map(|pair| (pair[0][0], pair[1].to_vec()))
        .collect())
}

async fn changed_paths(store: &Path, from: &str, to: &str) -> Result<Vec<Vec<u8>>, EngineError> {
    Ok(name_status(store, from, to)
        .await?
        .into_iter()
        .map(|(_, path)| path)
        .collect())
}

async fn drop_refs(checkpoints: &[Stored]) {
    let mut by_store: HashMap<&Path, Vec<u8>> = HashMap::new();
    for checkpoint in checkpoints {
        let input = by_store.entry(&checkpoint.store).or_default();
        input.extend_from_slice(format!("delete {}\n", checkpoint.meta.git_ref).as_bytes());
    }
    for (store, input) in by_store {
        if let Err(error) = store_git(store, &["update-ref", "--stdin"], Some(input)).await {
            tracing::warn!(store = %store.display(), %error, "dropping checkpoint refs failed");
        }
    }
}

/// The repository's checkpoint store, created on first use.
async fn ensure_store(dir: &Path, root: &Path) -> Result<PathBuf, EngineError> {
    let mut cmd = git_command();
    cmd.arg("-C")
        .arg(root)
        .args(["rev-parse", "--path-format=absolute", "--git-path", "objects"]);
    let objects = run_git(cmd, None).await?;
    let objects = PathBuf::from(String::from_utf8_lossy(&objects).trim());
    let objects = std::fs::canonicalize(&objects).unwrap_or(objects);
    let key = crate::repos::hex(&Sha256::digest(objects.to_string_lossy().as_bytes()));
    let store = std::path::absolute(dir.join("stores").join(format!("{}.git", &key[..16])))?;
    if !store.join("objects").is_dir() {
        let mut cmd = git_command();
        cmd.args(["init", "--bare", "--quiet"]).arg(&store);
        run_git(cmd, None).await?;
    }
    let alternates = store.join("objects").join("info").join("alternates");
    let line = format!("{}\n", objects.to_string_lossy());
    if tokio::fs::read_to_string(&alternates).await.ok().as_deref() != Some(line.as_str()) {
        tokio::fs::create_dir_all(alternates.parent().unwrap_or(&store)).await?;
        tokio::fs::write(&alternates, line).await?;
    }
    Ok(store)
}

/// Chat ids are uuids; anything else is hashed into a valid ref component.
fn ref_component(chat_id: &str) -> String {
    let plain = !chat_id.is_empty()
        && !chat_id.starts_with('.')
        && chat_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if plain {
        chat_id.to_string()
    } else {
        crate::repos::hex(&Sha256::digest(chat_id.as_bytes()))
    }
}

fn find(log: &ChatLog, id: u64) -> Result<&Stored, EngineError> {
    log.checkpoints
        .iter()
        .find(|c| c.meta.id == id)
        .ok_or_else(|| EngineError::Other(format!("no checkpoint {id}")))
}

fn log_path(dir: &Path, chat_id: &str) -> PathBuf {
    dir.join("chats").join(format!("{}.json", ref_component(chat_id)))
}

async fn load(dir: &Path, chat_id: &str) -> Result<ChatLog, EngineError> {
    match tokio::fs::read(log_path(dir, chat_id)).await {
        Ok(raw) => serde_json::from_slice(&raw)
            .map_err(|e| EngineError::Other(format!("checkpoint log: {e}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ChatLog::default()),
        Err(error) => Err(error.into()),
    }
}

/// Write-then-rename, so a crash leaves the old log or the new one.
async fn save(dir: &Path, chat_id: &str, log: &ChatLog) -> Result<(), EngineError> {
    let path = log_path(dir, chat_id);
    let raw = serde_json::to_vec(log)
        .map_err(|e| EngineError::Other(format!("checkpoint log: {e}")))?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, raw).await?;
    tokio::fs::rename(&tmp, &path).await?;
    Ok(())
}

fn git_command() -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.as_std_mut().creation_flags(0x08000000);
        cmd.args(["-c", "core.longpaths=true"]);
    }
    // Commits in the store are Vardoger's, whoever the checkout's user is.
    for (key, value) in [
        ("GIT_AUTHOR_NAME", "Vardoger"),
        ("GIT_AUTHOR_EMAIL", "checkpoints@vardoger.invalid"),
        ("GIT_COMMITTER_NAME", "Vardoger"),
        ("GIT_COMMITTER_EMAIL", "checkpoints@vardoger.invalid"),
    ] {
        cmd.env(key, value);
    }
    cmd.kill_on_drop(true);
    cmd
}

async fn store_git(
    store: &Path,
    args: &[&str],
    stdin: Option<Vec<u8>>,
) -> Result<Vec<u8>, EngineError> {
    let mut cmd = git_command();
    cmd.arg("--git-dir").arg(store).args(args);
    run_git(cmd, stdin).await
}

async fn run_git(
    mut cmd: tokio::process::Command,
    stdin: Option<Vec<u8>>,
) -> Result<Vec<u8>, EngineError> {
    cmd.stdin(if stdin.is_some() {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::null()
    });
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| EngineError::Other(format!("git spawn failed: {e}")))?;
    let _tree = crate::git_tree::GitTree::adopt(&child);
    if let Some(input) = stdin
        && let Some(mut pipe) = child.stdin.take()
    {
        pipe.write_all(&input).await?;
        pipe.shutdown().await?;
    }
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        return Err(EngineError::Other(format!(
            "git: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

#[cfg(test)]
#[path = "checkpoints_tests.rs"]
mod tests;
