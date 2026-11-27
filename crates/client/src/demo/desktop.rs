//! The demo dataset written into an engine's data dir, so a desktop window on
//! that engine shows it (screenshots: `cargo run -p commutator-client
//! --example seed_desktop_demo -- <dir>`). The local engine plays the MacBook
//! Pro; the other machines' chats read as remote, the phone as this account's.

use std::path::Path;

use commutator_doc::{MessageRole, MessageStatus, REGISTRY_DOC_ID, RegistryDoc, SessionDoc};
use commutator_sync::DocsStore;

use super::{fixtures, transcripts};
use crate::config::DemoFixture;

/// The session docs' lineage epoch the engine opens `room_gen: 2` chats at
/// (`commutator_engine::chat2_host::CHAT2_DOC_EPOCH`).
const CHAT2_DOC_EPOCH: u32 = 2;

pub fn write_desktop_demo(data_dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(data_dir.join("device-id"), fixtures::MAC)?;
    let store = DocsStore::open(data_dir.join("profiles/local"))?;

    let mut registry = RegistryDoc::new(fixtures::MAC);
    fixtures::seed(&mut registry, DemoFixture::Standard, "dev-iphone", "iPhone")?;
    store.save_snapshot(REGISTRY_DOC_ID, &registry.to_bytes()?)?;

    let now = crate::now_ms();
    for chat in fixtures::chats() {
        let doc = SessionDoc::init(chat.id)?;
        // No viewer finishes a reply here (the phone's DemoHost does): a
        // half-streamed one would freeze mid-sentence.
        let entries = transcripts::fixture(chat.id, chat.device, now - chat.last_ago_ms);
        for entry in entries.iter().filter(|e| {
            e.role != MessageRole::Assistant || e.status == Some(MessageStatus::Complete)
        }) {
            doc.push_message(entry)?;
        }
        store.save_snapshot_with_cursor(chat.id, &doc.export_snapshot()?, 0, CHAT2_DOC_EPOCH)?;
    }
    Ok(())
}
