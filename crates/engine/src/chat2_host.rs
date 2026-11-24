//! chat2 host wiring (docs/chat2-sync.md C3): the engine-side implementations
//! of [`commutator_sync::chat_client::ChatDocSink`] and
//! [`commutator_sync::chat_client::CheckpointFetcher`], binding a
//! [`crate::doc_host::ChatDocHandle`]'s live doc to a chat2 room.
//!
//! The C2 rule is enforced by the coalescing persister: doc content AND its
//! applied cursor commit in one transaction (at most once per second during replay), so a
//! restored backup can never disagree with its own cursor — the root cause
//! of the redownload-forever class the old s2 clients suffered.

use std::sync::Arc;


use commutator_doc::SessionDoc;
use commutator_sync::DocsStore;
use commutator_sync::chat_client::{ChatDocSink, RowImportOutcome};


/// Doc epoch stamped on every chat2-synced snapshot (docs/chat2-sync.md M1:
/// thin docs are lineage epoch 2; M3 readers discard-and-adopt below it).
pub const CHAT2_DOC_EPOCH: u32 = 2;

/// The journal key for relayed updates that arrived ahead of their history. Kept
/// apart from the chat's own journal, which a save retires, and replayed with it.
pub(crate) fn parked_inbox(chat_id: &str) -> String {
    format!("{chat_id}#parked")
}

/// [`ChatDocSink`] over a live [`SessionDoc`] + the cursor-bearing store.
///
/// Loro import of a remote row/checkpoint fires the doc's root subscription,
/// so the transcript watch, command drain, and debounced UI publish all ride
/// the existing change plumbing — this type only owns import + same-tx
/// persistence.
pub struct EngineChatSink {
    /// WEAK: the sink lives inside the handle's `ChatClient` for the
    /// client's whole life — a strong ref here kept
    /// `Arc::strong_count(&handle.doc) > 1` permanently, which reads as
    /// "live writer" to `pinned()` and made every chat2 handle immune to
    /// LRU eviction (unbounded warm-doc growth). Callbacks upgrade per
    /// call; a dead doc (evicted handle) is a no-op.
    doc: std::sync::Weak<SessionDoc>,
    store: Arc<DocsStore>,
    chat_id: String,
    handle: std::sync::Weak<crate::doc_host::ChatDocHandle>,
    persistence: Arc<crate::chat_persistence::ChatPersistence>,
}

impl EngineChatSink {
    pub fn new(doc: &Arc<SessionDoc>, store: Arc<DocsStore>, chat_id: impl Into<String>) -> Self {
        let chat_id = chat_id.into();
        let cursor = store.snapshot_cursor(&chat_id).unwrap_or(0);
        let persistence = crate::chat_persistence::ChatPersistence::new(
            doc,
            store.clone(),
            chat_id.clone(),
            cursor,
        );
        Self {
            doc: Arc::downgrade(doc),
            store,
            chat_id,
            persistence,
            handle: std::sync::Weak::new(),
        }
    }

    fn import_row(&self, bytes: &[u8], cursor: u64, replay: bool) -> RowImportOutcome {
        let Some(doc) = self.doc.upgrade() else {
            return RowImportOutcome::Applied;
        };
        let origin = if replay {
            crate::transcript_history::REPLAY_ORIGIN
        } else {
            ""
        };
        match doc.import_with(bytes, origin) {
            Ok(status) => {
                if status.pending.is_some() {
                    // Room sequence contiguity does not prove causal history
                    // is present. Snapshot export omits parked operations;
                    // advancing its cursor would lose them after restart.
                    tracing::warn!(chat = %self.chat_id, cursor,
                        "chat2 sink: row parked on missing deps; requesting repair");
                    return RowImportOutcome::PendingDependencies;
                }
            }
            Err(err) => {
                // Malformed remote bytes cost the row, never the doc (the same
                // skip-not-fail rule as transcript reads). The cursor still
                // advances: replaying a poison row forever is the wedge class.
                tracing::warn!(chat = %self.chat_id, error = %err,
                    "chat2 sink: row import failed; skipping row");
            }
        }
        self.persistence.applied(cursor, false);
        RowImportOutcome::Applied
    }

    /// Checkpoint/ACK boundaries bypass the debounce but still queue the
    /// export and transaction off the networking runtime.
    fn persist_with_cursor(&self, cursor: u64) {
        self.persistence.applied(cursor, true);
    }
}

impl ChatDocSink for EngineChatSink {
    fn cursor_is_verified(&self) -> bool {
        self.persistence.cursor_verified()
    }
    fn reset_cursor(&self, cursor: u64) {
        self.persistence.reset_cursor(cursor);
    }

    fn pending_updates(&self) -> Result<Vec<(String, Vec<u8>)>, String> {
        let rejected = self
            .store
            .rejected_chat_updates(&self.chat_id)
            .map_err(|e| e.to_string())?;
        let pending = self
            .store
            .pending_chat_updates(&self.chat_id)
            .map_err(|e| e.to_string())?;
        let mut updates = Vec::new();
        for (id, bytes) in pending {
            if bytes.len() > commutator_sync::chat_client::MAX_PUSH_BYTES {
                self.store
                    .reject_chat_update(&self.chat_id, &id)
                    .map_err(|e| e.to_string())?;
            } else if !rejected.iter().any(|(r, _)| r == &id) {
                updates.push((id, bytes));
            }
        }
        if let Some(handle) = self.handle.upgrade() {
            updates = handle.uploadable_updates(updates);
        }
        Ok(updates)
    }
    fn reject_update(&self, batch_id: &str) -> Result<(), String> {
        self.store
            .reject_chat_update(&self.chat_id, batch_id)
            .map_err(|e| e.to_string())
    }
    fn persist_update(&self, batch_id: &str, bytes: &[u8]) -> Result<(), String> {
        self.store
            .enqueue_chat_update(&self.chat_id, batch_id, bytes)
            .map_err(|e| e.to_string())
    }
    fn acknowledge_update(&self, batch_id: &str) -> Result<(), String> {
        self.store
            .acknowledge_chat_update(&self.chat_id, batch_id)
            .map_err(|e| e.to_string())
    }

    fn apply_row(&self, bytes: &[u8], cursor: u64) -> RowImportOutcome {
        match self.handle.upgrade() {
            Some(handle) => handle.import_transcript(|| self.import_row(bytes, cursor, false)),
            None => self.import_row(bytes, cursor, false),
        }
    }

    fn apply_replay_row(&self, bytes: &[u8], cursor: u64) -> RowImportOutcome {
        match self.handle.upgrade() {
            Some(handle) => handle.import_transcript(|| self.import_row(bytes, cursor, true)),
            None => self.apply_row(bytes, cursor),
        }
    }

    fn apply_checkpoint(&self, bytes: &[u8], cursor: u64) -> Result<(), String> {
        let import = || {
            let doc = self.doc.upgrade().ok_or("doc evicted")?;
            let status = doc
                .import_with(bytes, crate::transcript_history::REPLAY_ORIGIN)
                .map_err(|e| format!("checkpoint import: {e}"))?;
            if status.pending.is_some() {
                return Err("checkpoint is missing causal dependencies".into());
            }
            self.persist_with_cursor(cursor);
            Ok(())
        };
        match self.handle.upgrade() {
            Some(handle) => handle.import_transcript(import),
            None => import(),
        }
    }

    fn contains_frontier(&self, frontier: &[u8]) -> bool {
        let Some(doc) = self.doc.upgrade() else {
            return true; // evicted: claim contained so the client idles, not refetches
        };
        // NOTE deliberately no empty-frontier shortcut: an empty payload on
        // a PRESENT checkpoint is unreadable provenance, not proof there is
        // nothing to fetch — that shortcut made every fresh reader of such a
        // room skip the chat's founding ops and park all dependent rows
        // invisibly ("Add Tweets" incident, 2026-08-18). Empty falls through
        // to the decode failure below: NOT contained, fetch the checkpoint —
        // always safe (full-state merge; an empty-doc seed applies as a
        // no-op), never silently skips history. Callers already short-circuit
        // the checkpointSize == 0 (no checkpoint at all) case.
        let Ok(vv) = loro::VersionVector::decode(frontier) else {
            // Unreadable frontier → claim NOT contained: the client then
            // fetches the checkpoint, which is always safe (full-state
            // merge), never silently skips history.
            tracing::info!(chat = %self.chat_id, bytes = frontier.len(),
                "chat2 frontier unreadable; fetching checkpoint");
            return false;
        };
        // A decoded-but-EMPTY version vector is the vacuous claim: every doc
        // "includes" empty, so the check would pass for readers that hold
        // NOTHING and they'd skip the chat's founding ops (the actual "Add
        // Tweets" poison, one representation deeper than zero-length bytes).
        // A checkpoint the callers care about (size > 0) claiming empty
        // state is a contradiction — fetch it.
        if vv.is_empty() {
            tracing::info!(chat = %self.chat_id,
                "chat2 frontier decodes empty (vacuous); fetching checkpoint");
            return false;
        }
        doc.doc().oplog_vv().includes_vv(&vv)
    }

    fn advance_cursor(&self, cursor: u64) {
        self.persist_with_cursor(cursor);
    }

    fn relay_version(&self) -> Option<Vec<u8>> {
        Some(self.doc.upgrade()?.doc().oplog_vv().encode())
    }

    fn relay_lacks(&self, version: &[u8]) -> bool {
        let (Some(doc), Ok(vv)) = (self.doc.upgrade(), loro::VersionVector::decode(version)) else {
            return false;
        };
        !doc.doc().oplog_vv().includes_vv(&vv)
    }

    fn relay_behind(&self, version: &[u8]) -> bool {
        let (Some(doc), Ok(vv)) = (self.doc.upgrade(), loro::VersionVector::decode(version)) else {
            return false;
        };
        !vv.includes_vv(&doc.doc().oplog_vv())
    }

    fn relay_export(&self, since: &[u8]) -> Option<Vec<u8>> {
        let doc = self.doc.upgrade()?;
        let vv = loro::VersionVector::decode(since).ok()?;
        if vv.includes_vv(&doc.doc().oplog_vv()) {
            return Some(Vec::new()); // nothing it lacks
        }
        // A fresh reader takes the snapshot: smaller, and quicker to import.
        if vv.is_empty() {
            return doc.export_snapshot().ok();
        }
        doc.export(loro::ExportMode::updates(&vv)).ok()
    }

    fn relay_import(&self, bytes: &[u8], history: bool) -> commutator_sync::chat_relay::RelayImport {
        use commutator_sync::chat_relay::RelayImport;
        let import = || {
            // An evicted handle: nothing took it in, so it must not count as read.
            let Some(doc) = self.doc.upgrade() else {
                return RelayImport::Unreadable;
            };
            let origin = if history {
                crate::transcript_history::REPLAY_ORIGIN
            } else {
                ""
            };
            let before = doc.doc().oplog_vv();
            match doc.import_with(bytes, origin) {
                Ok(status) if status.pending.is_some() => {
                    // Held in memory until its history arrives. Kept on disk too, apart
                    // from the journal a save retires: a save doesn't hold what's parked.
                    if let Err(err) = self.store.journal_chat_inbox(&parked_inbox(&self.chat_id), bytes) {
                        tracing::warn!(chat = %self.chat_id, %err, "chat relay: parked journal write failed");
                    }
                    return RelayImport::Parked;
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(chat = %self.chat_id, error = %err, "chat relay: import failed");
                    return RelayImport::Unreadable;
                }
            }
            // Nobody else may be able to give this back: the room didn't store it,
            // and the host may be gone by the time this device restarts.
            if doc.doc().oplog_vv() != before {
                match self.store.journal_chat_inbox(&self.chat_id, bytes) {
                    Ok(_) => {}
                    Err(err) => tracing::warn!(chat = %self.chat_id, %err, "chat relay: journal write failed"),
                }
            }
            // No row seq here: the save covers the content at the same cursor.
            self.persistence.dirty(false);
            // A host that's storing must store this too, or its next row parks on
            // every old reader. Only what's new, however often it's delivered.
            if let Some(handle) = self.handle.upgrade()
                && handle.republishes_imports()
                && doc.doc().oplog_vv() != before
                && let Ok(delta) = doc.export(loro::ExportMode::updates(&before))
            {
                handle.republish(delta);
            }
            RelayImport::Applied
        };
        match self.handle.upgrade() {
            Some(handle) => handle.import_transcript(import),
            None => import(),
        }
    }

    fn relay_durable(&self) -> bool {
        self.persistence.is_clean()
    }
}

#[cfg(test)]
mod relay_sink_tests {
    use super::*;
    use std::sync::Arc;
    use commutator_sync::chat_relay::RelayImport;

    fn sink() -> (EngineChatSink, Arc<SessionDoc>, Arc<DocsStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("chat").unwrap());
        (EngineChatSink::new(&doc, store.clone(), "chat"), doc, store, dir)
    }

    #[tokio::test]
    async fn a_relayed_update_for_an_evicted_doc_is_not_taken_as_read() {
        let (sink, doc, _store, _dir) = sink();
        drop(doc);
        let other = loro::LoroDoc::new();
        other.get_text("t").insert(0, "x").unwrap();
        other.commit();
        let bytes = other.export(loro::ExportMode::all_updates()).unwrap();
        assert_eq!(sink.relay_import(&bytes, false), RelayImport::Unreadable);
    }

    #[tokio::test]
    async fn a_relayed_update_that_arrives_ahead_of_its_history_is_kept() {
        let (sink, _doc, store, _dir) = sink();
        let author = loro::LoroDoc::new();
        author.get_text("t").insert(0, "first").unwrap();
        author.commit();
        let before = author.oplog_vv();
        author.get_text("t").insert(5, " second").unwrap();
        author.commit();
        let later = author.export(loro::ExportMode::updates(&before)).unwrap();
        assert_eq!(sink.relay_import(&later, false), RelayImport::Parked);
        let (kept, _) = store.pending_chat_inbox(&parked_inbox("chat")).unwrap();
        assert_eq!(kept, vec![later], "lost with this process otherwise");
    }
}
