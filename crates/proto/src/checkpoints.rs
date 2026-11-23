//! Per-turn git checkpoints of a chat's checkout (`ListCheckpoints`,
//! `GetCheckpointDiff`, `RollbackToCheckpoint`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// When a checkpoint was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckpointPhase {
    /// As a user prompt started the turn.
    TurnStart,
    /// As the turn's `Done` closed it.
    TurnEnd,
    /// The working tree a rollback replaced: rolling back to it undoes the rollback.
    BeforeRollback,
}

/// One snapshot of a chat's working tree (tracked + untracked, not ignored).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    /// Per chat, increasing.
    pub id: u64,
    /// The chat's turn it belongs to, from 1.
    pub turn: u32,
    pub phase: CheckpointPhase,
    /// `refs/vardoger/checkpoints/{chatId}/{id}` in the engine's checkpoint
    /// store for this repository (not the checkout's own refs).
    pub git_ref: String,
    pub commit: String,
    /// Worktree root the snapshot was taken of.
    pub cwd: String,
    pub created_at: DateTime<Utc>,
    /// Paths that differ from the chat's previous checkpoint in the same
    /// checkout; `None` for the first one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_changed: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListCheckpointsRequest {
    pub chat_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListCheckpointsResponse {
    /// Oldest first.
    pub checkpoints: Vec<Checkpoint>,
}

/// Answered with a [`crate::CheckoutDiff`] (3 MiB patch cap).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetCheckpointDiffRequest {
    pub chat_id: String,
    pub from: u64,
    /// Another checkpoint; absent: the current working tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackToCheckpointRequest {
    pub chat_id: String,
    pub checkpoint_id: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackToCheckpointResponse {
    pub restored: Checkpoint,
    /// The tree the rollback replaced ([`CheckpointPhase::BeforeRollback`]).
    pub before: Checkpoint,
    /// Files written back from the checkpoint.
    pub files_restored: u32,
    /// Files removed because the checkpoint did not have them.
    pub files_deleted: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shape_is_camel_case() {
        let value = serde_json::to_value(RollbackToCheckpointRequest {
            chat_id: "c".into(),
            checkpoint_id: 3,
        })
        .unwrap();
        assert_eq!(value, serde_json::json!({"chatId": "c", "checkpointId": 3}));
        assert_eq!(
            serde_json::to_value(CheckpointPhase::BeforeRollback).unwrap(),
            "beforeRollback"
        );
    }
}
