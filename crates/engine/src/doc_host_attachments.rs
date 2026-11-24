//! Attachment bytes through the edge's store, for chats hosted on Cloud
//! (docs/design/cloud-attachments.md).
//!
//! A peer link can't reach a sleeping machine, and a sender may be gone by the
//! time the machine wakes. So:
//!
//! - **Sender** ([`DocHost::hold_if_needed`], [`DocHost::run_held_sends`]): the
//!   send waits in a durable outbox while its bytes go up to the store; only
//!   then does the message enter the chat, so the host never sees a message
//!   whose bytes aren't safe. Sends of one chat leave in order.
//! - **Host** ([`DocHost::fetch_from_store`]): a Cloud machine holding a message
//!   whose bytes aren't on its disk looks in the store. Once they are, the same
//!   re-drain an `UploadCommit` does runs the message, and the object is deleted.
//!
//! Everything else keeps the peer path: chats on a user's own computers, hosts
//! too old to look in the store, edges too old to have one.

use commutator_doc::{
    CommandBasedOn, QueuedMessage, SessionCommandEntry,
    SessionCommandPayload, SessionCommandStatus,
};
use commutator_proto::{HeldSendStatus, WaitingMessage};
use commutator_sync::attachment_store::{
    self, HeldBody, HeldPayload, HeldSend, HeldUpload, PutError,
};

use super::{
    DocHost, EngineError, TRANSFER_BACKOFF_BASE, TRANSFER_BACKOFF_CAP,
    lock, new_id, now_ms,
};
use crate::uploads::AttachmentTransfer;

/// How a chat's attachment bytes reach its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachmentRoute {
    /// Over a peer link, after the message (the original flow).
    Peer,
    /// Through the store, before the message. `also_peer`: the host's version
    /// isn't known, so push over the link as well in case it's too old to look.
    Store { also_peer: bool },
}

impl DocHost {

    /// A chat's messages not yet taken by its agent: unresolved runs and steers
    /// in its doc, then what this device still holds for it.
    pub fn waiting_messages(&self, chat_id: &str) -> Result<Vec<WaitingMessage>, EngineError> {
        let handle = self.open(chat_id)?;
        let pending: Vec<_> = handle
            .doc
            .read_commands()?
            .into_iter()
            .filter(|c| c.status == SessionCommandStatus::Pending)
            .collect();
        // (Read on every change to the chat while it's open: the
        // transcript's ids only when a command waits, and shared, not
        // copied. A long chat's every entry was copied here per commit.)
        let landed: std::collections::HashSet<String> = if pending.is_empty() {
            Default::default()
        } else {
            handle
                .doc
                .read_entries_shared()
                .unwrap_or_default()
                .iter()
                .map(|entry| entry.id.clone())
                .collect()
        };
        let mut waiting: Vec<WaitingMessage> = pending
            .into_iter()
            .filter_map(|c| {
                let (message_id, prompt) = match &c.payload {
                    SessionCommandPayload::Run {
                        request,
                        message_id,
                    } => (Some(message_id.clone()), request.prompt.clone()),
                    SessionCommandPayload::Steer { prompt, message_id } => {
                        (message_id.clone(), prompt.clone())
                    }
                    _ => return None,
                };
                if message_id.as_ref().is_some_and(|id| landed.contains(id)) {
                    return None;
                }
                Some(WaitingMessage {
                    expires_at: Some(c.effective_expiry()),
                    id: c.id,
                    chat_id: chat_id.to_string(),
                    message_id,
                    prompt,
                    issued_by: c.issued_by,
                    sent_at: c.issued_at,
                    held: false,
                    failure: None,
                })
            })
            .collect();
        for send in self.inner.store.held_sends_for(chat_id)? {
            let (message_id, prompt) = match &send.payload.body {
                HeldBody::Row { item } => (
                    Some(item.id.clone()),
                    super::queued_message_prompt(&item.text, &item.attachments),
                ),
                HeldBody::Command { .. } => (
                    send.payload.message_id().map(str::to_string),
                    send.payload.prompt().to_string(),
                ),
            };
            waiting.push(WaitingMessage {
                id: send.id,
                chat_id: chat_id.to_string(),
                message_id,
                prompt,
                issued_by: self.inner.config.device_id.clone(),
                sent_at: send.created_at,
                expires_at: None,
                held: true,
                failure: send.failure,
            });
        }
        Ok(waiting)
    }

    /// [`Self::waiting_messages`], again on every change to the chat's doc or
    /// to this device's outbox.
    pub fn watch_waiting_messages(
        &self,
        chat_id: &str,
    ) -> Result<
        (
            tokio::sync::watch::Receiver<u64>,
            tokio::sync::watch::Receiver<Vec<HeldSendStatus>>,
        ),
        EngineError,
    > {
        let handle = self.open(chat_id)?;
        Ok((handle.changes.clone(), self.watch_held_sends()))
    }

    fn attachment_route(&self, chat_id: &str) -> AttachmentRoute {
        // Bytes go to the host over the peer link, after the message.
        let _ = chat_id;
        AttachmentRoute::Peer
    }

    /// Hold this send when its bytes must reach the store first, or when an
    /// earlier send of the chat is still held (it mustn't be overtaken).
    /// `Some(id)` = held; the caller writes nothing to the chat.
    pub(super) fn hold_if_needed(
        &self,
        chat_id: &str,
        payload: &SessionCommandPayload,
        transfers: &[AttachmentTransfer],
    ) -> Result<Option<String>, EngineError> {
        let behind = self.inner.store.has_active_held_send(chat_id)?;
        if !behind && transfers.is_empty() {
            return Ok(None);
        }
        let route = self.attachment_route(chat_id);
        if !behind && route == AttachmentRoute::Peer {
            return Ok(None);
        }
        let id = new_id();
        self.hold(
            &id,
            chat_id,
            HeldBody::Command {
                payload: payload.clone(),
            },
            transfers,
            matches!(route, AttachmentRoute::Store { also_peer: true }),
        )?;
        Ok(Some(id))
    }

    /// A queue row waits like a command does: for its own bytes, and behind an
    /// earlier held send, so it keeps its place. `true` = held.
    pub(super) fn hold_row_if_needed(
        &self,
        chat_id: &str,
        item: &QueuedMessage,
        transfers: &[AttachmentTransfer],
    ) -> Result<bool, EngineError> {
        let behind = self.inner.store.has_active_held_send(chat_id)?;
        if !behind && transfers.is_empty() {
            return Ok(false);
        }
        let route = self.attachment_route(chat_id);
        if !behind && route == AttachmentRoute::Peer {
            return Ok(false);
        }
        self.hold(
            &item.id,
            chat_id,
            HeldBody::Row { item: item.clone() },
            transfers,
            matches!(route, AttachmentRoute::Store { also_peer: true }),
        )?;
        Ok(true)
    }

    /// A row's bytes over the peer link, once the row is in the chat.
    pub(super) fn spawn_row_transfers(&self, chat_id: &str, transfers: Vec<AttachmentTransfer>) {
        if transfers.is_empty() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let host = self.clone();
        let chat = chat_id.to_string();
        self.spawn_worker_on(&runtime, async move {
            if host.remote_host_for(&chat).is_some() {
                host.deliver_attachments(&chat, &transfers).await;
            }
        });
    }

    fn hold(
        &self,
        id: &str,
        chat_id: &str,
        body: HeldBody,
        transfers: &[AttachmentTransfer],
        also_peer: bool,
    ) -> Result<(), EngineError> {
        let payload = HeldPayload {
            body,
            uploads: transfers
                .iter()
                .map(|t| HeldUpload {
                    upload_id: t.upload_id.clone(),
                    file_name: t.file_name.clone(),
                })
                .collect(),
            also_peer,
        };
        self.inner
            .store
            .hold_send(id, chat_id, &payload, now_ms())?;
        self.publish_held_sends();
        self.spawn_held_sends(chat_id);
        Ok(())
    }

    /// Engine start: pick up what was still on its way. A failed send stays,
    /// however old, until its person retries or removes it.
    pub(super) fn resume_held_sends(&self) {
        let sends = match self.inner.store.held_sends() {
            Ok(sends) => sends,
            Err(err) => {
                tracing::warn!(%err, "held sends unreadable");
                return;
            }
        };
        self.publish_held_sends();
        let mut started = std::collections::HashSet::new();
        for send in sends {
            if send.failure.is_none() && started.insert(send.chat_id.clone()) {
                tracing::info!(chat = %send.chat_id, "resuming held sends");
                self.spawn_held_sends(&send.chat_id);
            }
        }
    }

    /// The user's retry: failed sends try again, and anything backing off
    /// tries now.
    pub(super) fn retry_held_sends(&self, chat_id: &str) -> Result<(), EngineError> {
        let revived = self.inner.store.retry_held_sends(chat_id, now_ms())?;
        if revived > 0 {
            self.publish_held_sends();
        }
        self.inner.held_kick.notify_waiters();
        self.spawn_held_sends(chat_id);
        Ok(())
    }

    /// Its person removed a send from the outbox, failed or still uploading:
    /// it never goes out. `false` when it had already gone out (or been removed).
    pub fn remove_held_send(&self, chat_id: &str, id: &str) -> Result<bool, EngineError> {
        {
            let _gate = lock(&self.inner.held_gate);
            let held = self.inner.store.held_send(id)?;
            if !held.is_some_and(|send| send.chat_id == chat_id) {
                return Ok(false);
            }
            self.inner.store.remove_held_send(id)?;
        }
        tracing::info!(chat = %chat_id, send = %id, "held send removed by its person");
        self.publish_held_sends();
        Ok(true)
    }

    /// Message ids still waiting in this chat's outbox (a retry mustn't mint
    /// a second attempt for one).
    pub(super) fn held_message_ids(&self, chat_id: &str) -> Vec<String> {
        self.inner
            .store
            .held_sends_for(chat_id)
            .unwrap_or_default()
            .iter()
            .filter_map(|send| send.payload.message_id().map(str::to_string))
            .collect()
    }

    /// A retried command whose host never got the bytes goes through the store
    /// again, when this device still has them. (`false`: they were sent from
    /// elsewhere, or this chat doesn't use the store; carry on as before.)
    pub(super) fn hold_reissue(
        &self,
        chat_id: &str,
        entry: &SessionCommandEntry,
        transfers: &[AttachmentTransfer],
    ) -> Result<bool, EngineError> {
        let route = self.attachment_route(chat_id);
        if transfers.is_empty() || route == AttachmentRoute::Peer {
            return Ok(false);
        }
        let Some(uploads) = self.inner.uploads.get() else {
            return Ok(false);
        };
        if !transfers
            .iter()
            .all(|t| uploads.pending_target(&t.upload_id, &t.file_name).is_file())
        {
            return Ok(false);
        }
        self.hold(
            &entry.id,
            chat_id,
            HeldBody::Command {
                payload: entry.payload.clone(),
            },
            transfers,
            matches!(route, AttachmentRoute::Store { also_peer: true }),
        )?;
        Ok(true)
    }

    /// Whether a retry's peer escort should carry this chat's bytes: not when
    /// the host fetches them from the store on its own.
    pub(super) fn peer_transfers_needed(&self, chat_id: &str) -> bool {
        self.attachment_route(chat_id) != AttachmentRoute::Store { also_peer: false }
    }

    /// The outbox as the local interface shows it (`WatchHeldSends`).
    pub fn watch_held_sends(&self) -> tokio::sync::watch::Receiver<Vec<HeldSendStatus>> {
        self.inner.held_status.subscribe()
    }

    fn publish_held_sends(&self) {
        let sends = match self.inner.store.held_sends() {
            Ok(sends) => sends,
            Err(err) => {
                tracing::warn!(%err, "held sends unreadable");
                return;
            }
        };
        self.inner.held_status.send_replace(
            sends
                .iter()
                .map(|send| {
                    // A row's id becomes its message's, and its attachment
                    // trailer is only added when it's dispatched.
                    let (message_id, prompt, queue_row) = match &send.payload.body {
                        HeldBody::Row { item } => (
                            Some(item.id.clone()),
                            super::queued_message_prompt(&item.text, &item.attachments),
                            true,
                        ),
                        HeldBody::Command { .. } => (
                            send.payload.message_id().map(str::to_string),
                            send.payload.prompt().to_string(),
                            false,
                        ),
                    };
                    HeldSendStatus {
                        id: send.id.clone(),
                        chat_id: send.chat_id.clone(),
                        message_id,
                        prompt,
                        queue_row,
                        created_at: send.created_at,
                        failure: send.failure.clone(),
                    }
                })
                .collect(),
        );
    }

    /// Drain a chat's outbox; one worker per chat.
    fn spawn_held_sends(&self, chat_id: &str) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return; // bare sync callers (unit tests); the next start resumes it
        };
        if !lock(&self.inner.held_active).insert(chat_id.to_string()) {
            return;
        }
        let host = self.clone();
        let chat = chat_id.to_string();
        self.spawn_worker_on(&runtime, async move {
            loop {
                host.run_held_sends(&chat).await;
                lock(&host.inner.held_active).remove(&chat);
                // A send held between the last look and the line above found
                // this worker still marked running, and left it to us.
                let more = host
                    .inner
                    .store
                    .next_held_send(&chat)
                    .is_ok_and(|next| next.is_some());
                if !more || !lock(&host.inner.held_active).insert(chat.clone()) {
                    return;
                }
            }
        });
    }

    async fn run_held_sends(&self, chat: &str) {
        let mut wake = commutator_sync::wake::subscribe();
        let mut online = commutator_sync::wake::subscribe_online();
        let mut backoff = TRANSFER_BACKOFF_BASE;
        loop {
            let send = match self.inner.store.next_held_send(chat) {
                Ok(Some(send)) => send,
                Ok(None) => return,
                Err(err) => {
                    tracing::warn!(chat = %chat, %err, "held sends unreadable");
                    return;
                }
            };
            // Never given up on: only its person removes it. Every try uploads
            // all its files again, so none ages out at the edge meanwhile.
            let outcome = self.store_uploads(&send).await;
            let cap = attachment_store::held_backoff_cap(
                now_ms().saturating_sub(send.created_at),
                TRANSFER_BACKOFF_CAP,
            );
            match outcome {
                Ok(peer) => {
                    if let Err(err) = self.release_held(&send, peer) {
                        // The doc write failed: the row stays, and so does the
                        // message. Retries are all that can help.
                        tracing::warn!(chat = %chat, send = %send.id, %err, "held send not released");
                    } else {
                        backoff = TRANSFER_BACKOFF_BASE;
                        continue;
                    }
                }
                Err(PutError::Permanent(why)) => {
                    tracing::warn!(chat = %chat, send = %send.id, %why,
                        "attachment upload failed permanently");
                    if let Err(err) = self.inner.store.fail_held_send(&send.id, &why) {
                        tracing::warn!(chat = %chat, %err, "held send failure not recorded");
                    }
                    self.publish_held_sends();
                    continue;
                }
                // `store_uploads` turns this one into the peer path itself.
                Err(PutError::Unsupported) => {}
                Err(PutError::Transient(why)) => {
                    tracing::warn!(chat = %chat, send = %send.id, %why,
                        backoff_ms = backoff.as_millis() as u64, "attachment upload retrying");
                }
            }
            while wake.try_recv().is_ok() {}
            while online.try_recv().is_ok() {}
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = wake.recv() => {}
                _ = online.recv() => {}
                _ = self.inner.held_kick.notified() => {}
            }
            backoff = (backoff * 2).min(cap);
        }
    }

    /// Put the send's files in the store, as far as its route asks. Answers
    /// whether the bytes must also travel the peer link.
    async fn store_uploads(&self, send: &HeldSend) -> Result<bool, PutError> {
        // No store to put them in: the peer link carries them.
        let _ = send;
        Ok(true)
    }

    /// The bytes are safe: the message enters the chat, and that's what the
    /// host sees and wakes for. Safe to repeat after a crash between the two
    /// writes: the doc write checks for its own id.
    fn release_held(&self, send: &HeldSend, peer: bool) -> Result<(), EngineError> {
        let gate = lock(&self.inner.held_gate);
        if self.inner.store.held_send(&send.id)?.is_none() {
            // Removed while it uploaded: it never goes out.
            drop(gate);
            return Ok(());
        }
        let handle = self.open(&send.chat_id)?;
        let transfers: Vec<AttachmentTransfer> = if peer {
            send.payload
                .uploads
                .iter()
                .map(|u| AttachmentTransfer {
                    upload_id: u.upload_id.clone(),
                    file_name: u.file_name.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut escort = None;
        match &send.payload.body {
            HeldBody::Command { payload } => {
                let commands = handle.doc.read_commands()?;
                if !commands.iter().any(|c| c.id == send.id) {
                    let now = now_ms();
                    let based_on = handle.doc.read_entries()?.last().map(|m| CommandBasedOn {
                        turn_id: Some(m.id.clone()),
                        frontier: None,
                    });
                    let entry = SessionCommandEntry {
                        id: send.id.clone(),
                        payload: payload.clone(),
                        issued_by: self.inner.config.device_id.clone(),
                        // The clock of a message starts when it goes out.
                        issued_at: now,
                        based_on,
                        expires_at: Some(now + commutator_doc::COMMAND_DEFAULT_TTL_MS),
                        status: SessionCommandStatus::Pending,
                        resolution: None,
                    };
                    handle.doc.queue_command(&entry)?;
                    escort = Some(entry);
                }
            }
            HeldBody::Row { item } => {
                if !handle.doc.read_queue()?.iter().any(|q| q.id == item.id) {
                    let mut item = item.clone();
                    item.issued_at = now_ms();
                    handle.doc.push_queued(&item)?;
                    handle.publish_queue();
                }
            }
        }
        self.inner.store.remove_held_send(&send.id)?;
        drop(gate);
        self.publish_held_sends();
        if let Some(entry) = escort {
            self.spawn_command_delivery(&send.chat_id, entry, transfers);
        } else if matches!(send.payload.body, HeldBody::Row { .. }) {
            self.spawn_row_transfers(&send.chat_id, transfers);
        }
        Ok(())
    }

    // ---- the host's side ----

}
