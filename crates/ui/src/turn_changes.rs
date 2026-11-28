//! The model behind the turn-end "changed files" card (t3code's
//! `ChangedFilesCard`): which turn of the transcript changed which files, as
//! the engine's per-turn git checkpoints (`ListCheckpoints` /
//! `GetCheckpointDiff`) recorded them, and the compact folder tree the card
//! lists them in.
//!
//! Pure: the transcript fetches, this module decides. A turn whose checkout
//! has no checkpoints (a cwd outside git, an engine that predates them)
//! falls back to the counts its file tools reported.

use std::collections::{BTreeMap, HashMap};

use commutator_proto::CheckoutDiff;
use commutator_proto::checkpoints::{Checkpoint, CheckpointPhase};

/// One changed file on the card. Paths are repository-relative for
/// checkpoint diffs, as the tools spelled them otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
}

/// Where a card's files came from, and so what "open its diff" opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesSource {
    /// The chat's checkpoint diff `from`..`to` (turn `turn`'s start and end).
    Checkpoint {
        chat_id: String,
        turn: u32,
        from: u64,
        to: u64,
    },
    /// The counts the turn's file tools reported (no checkpoint recorded).
    Tools,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnChanges {
    pub files: Vec<ChangedFile>,
    pub additions: u64,
    pub deletions: u64,
    pub source: ChangesSource,
}

impl TurnChanges {
    fn new(files: Vec<ChangedFile>, source: ChangesSource) -> Option<Self> {
        if files.is_empty() {
            return None;
        }
        Some(Self {
            additions: files.iter().map(|f| f.additions).sum(),
            deletions: files.iter().map(|f| f.deletions).sum(),
            files,
            source,
        })
    }

    /// A checkpoint diff's files; `None` when the turn changed nothing.
    pub fn from_checkout_diff(diff: &CheckoutDiff, source: ChangesSource) -> Option<Self> {
        let files = diff
            .files
            .iter()
            .map(|file| ChangedFile {
                path: file.path.clone(),
                additions: file.additions.into(),
                deletions: file.deletions.into(),
                binary: file.binary,
            })
            .collect();
        Self::new(files, source)
    }

    /// The tool-reported fallback; `None` when no file tool ran.
    pub fn from_tool_stats(stats: &[commutator_doc::ToolDiffStat]) -> Option<Self> {
        let files = stats
            .iter()
            .map(|stat| ChangedFile {
                path: stat.path.clone(),
                additions: stat.additions,
                deletions: stat.deletions,
                binary: false,
            })
            .collect();
        Self::new(files, ChangesSource::Tools)
    }

    /// Row diff key: changes whenever anything the card shows does.
    pub fn fingerprint(&self) -> u64 {
        let mut acc: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for b in bytes {
                acc ^= u64::from(*b);
                acc = acc.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        for file in &self.files {
            eat(file.path.as_bytes());
            eat(&file.additions.to_le_bytes());
            eat(&file.deletions.to_le_bytes());
        }
        if let ChangesSource::Checkpoint { from, to, .. } = &self.source {
            eat(&from.to_le_bytes());
            eat(&to.to_le_bytes());
        }
        // The top bit is the transcript's timestamp flag.
        acc & !(1 << 62)
    }

    /// Whether any listed path sits in a folder (the card then shows a tree).
    pub fn has_directories(&self) -> bool {
        self.files.iter().any(|f| f.path.contains('/'))
    }
}

// ---------------------------------------------------------------------------
// Folder tree (t3code `buildTurnDiffTree`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeNode {
    Dir {
        /// Display name; a compacted chain reads `a/b/c`.
        name: String,
        /// Full path of the deepest folder in the chain (the fold key).
        path: String,
        additions: u64,
        deletions: u64,
        children: Vec<TreeNode>,
    },
    File {
        name: String,
        path: String,
        additions: u64,
        deletions: u64,
    },
}

#[derive(Default)]
struct MutDir {
    dirs: BTreeMap<String, MutDir>,
    files: Vec<(String, String, u64, u64)>,
    additions: u64,
    deletions: u64,
}

fn sort_key(name: &str) -> String {
    name.to_lowercase()
}

fn to_nodes(dir: MutDir, prefix: &str) -> Vec<TreeNode> {
    let mut dirs: Vec<(String, MutDir)> = dir.dirs.into_iter().collect();
    dirs.sort_by_key(|(name, _)| sort_key(name));
    let mut out: Vec<TreeNode> = dirs
        .into_iter()
        .map(|(name, sub)| {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let (additions, deletions) = (sub.additions, sub.deletions);
            let children = to_nodes(sub, &path);
            compact(TreeNode::Dir {
                name,
                path,
                additions,
                deletions,
                children,
            })
        })
        .collect();
    let mut files = dir.files;
    files.sort_by_key(|(name, ..)| sort_key(name));
    out.extend(
        files
            .into_iter()
            .map(|(name, path, additions, deletions)| TreeNode::File {
                name,
                path,
                additions,
                deletions,
            }),
    );
    out
}

/// A folder whose only child is a folder merges with it (`src/ui/chat`).
fn compact(node: TreeNode) -> TreeNode {
    let TreeNode::Dir {
        mut name,
        mut path,
        mut additions,
        mut deletions,
        mut children,
    } = node
    else {
        return node;
    };
    while children.len() == 1 && matches!(children[0], TreeNode::Dir { .. }) {
        let Some(TreeNode::Dir {
            name: child_name,
            path: child_path,
            additions: a,
            deletions: d,
            children: grand,
        }) = children.pop()
        else {
            unreachable!()
        };
        name = format!("{name}/{child_name}");
        path = child_path;
        additions = a;
        deletions = d;
        children = grand;
    }
    TreeNode::Dir {
        name,
        path,
        additions,
        deletions,
        children,
    }
}

/// Folders first, then files, each by name; folder counts sum their files.
pub fn build_tree(files: &[ChangedFile]) -> Vec<TreeNode> {
    let mut root = MutDir::default();
    for file in files {
        let segments: Vec<&str> = file.path.split('/').filter(|s| !s.is_empty()).collect();
        let Some((name, folders)) = segments.split_last() else {
            continue;
        };
        let mut dir = &mut root;
        for segment in folders {
            dir = dir.dirs.entry((*segment).to_string()).or_default();
            dir.additions += file.additions;
            dir.deletions += file.deletions;
        }
        dir.files.push((
            (*name).to_string(),
            segments.join("/"),
            file.additions,
            file.deletions,
        ));
    }
    to_nodes(root, "")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardRow {
    pub depth: usize,
    pub name: String,
    /// The file's path (what a click opens), or the folder's fold key.
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    /// `Some(expanded)` for a folder row.
    pub folder: Option<bool>,
}

/// The rows the card shows: folders follow `overrides`, else `all_expanded`.
pub fn visible_rows(
    tree: &[TreeNode],
    all_expanded: bool,
    overrides: &HashMap<String, bool>,
) -> Vec<CardRow> {
    fn walk(
        nodes: &[TreeNode],
        depth: usize,
        all_expanded: bool,
        overrides: &HashMap<String, bool>,
        out: &mut Vec<CardRow>,
    ) {
        for node in nodes {
            match node {
                TreeNode::Dir {
                    name,
                    path,
                    additions,
                    deletions,
                    children,
                } => {
                    let expanded = overrides.get(path).copied().unwrap_or(all_expanded);
                    out.push(CardRow {
                        depth,
                        name: name.clone(),
                        path: path.clone(),
                        additions: *additions,
                        deletions: *deletions,
                        folder: Some(expanded),
                    });
                    if expanded {
                        walk(children, depth + 1, all_expanded, overrides, out);
                    }
                }
                TreeNode::File {
                    name,
                    path,
                    additions,
                    deletions,
                } => out.push(CardRow {
                    depth,
                    name: name.clone(),
                    path: path.clone(),
                    additions: *additions,
                    deletions: *deletions,
                    folder: None,
                }),
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, 0, all_expanded, overrides, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Checkpoints → transcript turns
// ---------------------------------------------------------------------------

/// One finished turn the checkpoints cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRange {
    pub turn: u32,
    /// The turn's start and end checkpoint ids.
    pub from: u64,
    pub to: u64,
    pub started_ms: i64,
    pub ended_ms: i64,
    /// The end checkpoint said nothing differs from the start: no diff to fetch.
    pub unchanged: bool,
}

/// Turns with both a start and an end checkpoint, oldest first.
pub fn turn_ranges(checkpoints: &[Checkpoint]) -> Vec<TurnRange> {
    let mut out = Vec::new();
    for (ix, end) in checkpoints.iter().enumerate() {
        if end.phase != CheckpointPhase::TurnEnd {
            continue;
        }
        let Some((start_ix, start)) = checkpoints[..ix]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, c)| c.phase == CheckpointPhase::TurnStart && c.turn == end.turn)
        else {
            continue;
        };
        // `files_changed` counts against the previous checkpoint in the same
        // checkout; it speaks for the turn only when that one is its start.
        let previous_is_start = checkpoints[start_ix + 1..ix]
            .iter()
            .all(|c| c.cwd != end.cwd);
        out.push(TurnRange {
            turn: end.turn,
            from: start.id,
            to: end.id,
            started_ms: start.created_at.timestamp_millis(),
            ended_ms: end.created_at.timestamp_millis(),
            unchanged: previous_is_start && end.files_changed == Some(0),
        });
    }
    out.sort_by_key(|r| r.ended_ms);
    out
}

/// A settled assistant entry, as far as turn matching needs it.
#[derive(Debug, Clone, Copy)]
pub struct EntryStamp<'a> {
    pub id: &'a str,
    pub created_ms: i64,
}

/// The start checkpoint is written after its snapshot, so the turn's first
/// reply can predate it by that long.
const START_SLACK_MS: i64 = 120_000;

/// Each turn's card belongs to its last reply: the last settled assistant
/// entry created after the previous turn ended (and not long before this one
/// started) and before this turn's end checkpoint. `entries` are in
/// transcript order. A turn with no such entry gets no card.
pub fn assign_turns<'a>(
    ranges: &[TurnRange],
    entries: &[EntryStamp<'a>],
) -> Vec<(&'a str, TurnRange)> {
    let mut out: Vec<(&'a str, TurnRange)> = Vec::new();
    let mut previous_end = i64::MIN;
    for range in ranges {
        let lower = previous_end.max(range.started_ms.saturating_sub(START_SLACK_MS));
        let owner = entries
            .iter()
            .rev()
            .find(|e| e.created_ms > lower && e.created_ms <= range.ended_ms);
        if let Some(owner) = owner
            && !out.iter().any(|(id, _)| *id == owner.id)
        {
            out.push((owner.id, range.clone()));
        }
        previous_end = range.ended_ms;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;
    use commutator_proto::DiffFileSummary;

    fn file(path: &str, additions: u32, deletions: u32) -> DiffFileSummary {
        DiffFileSummary {
            path: path.into(),
            old_path: None,
            status: "modified".into(),
            additions,
            deletions,
            binary: false,
        }
    }

    fn checkpoint(
        id: u64,
        turn: u32,
        phase: CheckpointPhase,
        secs: i64,
        changed: Option<u32>,
    ) -> Checkpoint {
        Checkpoint {
            id,
            turn,
            phase,
            git_ref: format!("refs/commutator/checkpoints/c/{id}"),
            commit: "0".repeat(40),
            cwd: "/repo".into(),
            created_at: chrono::Utc.timestamp_opt(secs, 0).unwrap(),
            files_changed: changed,
        }
    }

    #[test]
    fn checkpoint_diff_becomes_card_rows_with_counts_and_compact_folders() {
        let diff = CheckoutDiff {
            checkout_id: "k".into(),
            device_id: "d".into(),
            cwd: "/repo".into(),
            patch: String::new(),
            files: vec![
                file("crates/ui/src/transcript.rs", 40, 12),
                file("crates/ui/src/settings.rs", 5, 0),
                file("crates/proto/src/lib.rs", 1, 1),
                file("README.md", 2, 3),
            ],
            additions: 48,
            deletions: 16,
            truncated: false,
            checksum: "x".into(),
            updated_at: chrono::Utc::now(),
        };
        let source = ChangesSource::Checkpoint {
            chat_id: "c".into(),
            turn: 2,
            from: 3,
            to: 4,
        };
        let changes = TurnChanges::from_checkout_diff(&diff, source).unwrap();
        assert_eq!(
            (changes.files.len(), changes.additions, changes.deletions),
            (4, 48, 16)
        );

        let tree = build_tree(&changes.files);
        let row = |depth, name: &str, path: &str, a, d, folder| CardRow {
            depth,
            name: name.into(),
            path: path.into(),
            additions: a,
            deletions: d,
            folder,
        };
        // Collapsed (the default): top-level folder plus root files.
        assert_eq!(
            visible_rows(&tree, false, &HashMap::new()),
            vec![
                row(0, "crates", "crates", 46, 13, Some(false)),
                row(0, "README.md", "README.md", 2, 3, None),
            ]
        );
        // Expanded: single-child chains read as one folder (`ui/src`).
        assert_eq!(
            visible_rows(&tree, true, &HashMap::new()),
            vec![
                row(0, "crates", "crates", 46, 13, Some(true)),
                row(1, "proto/src", "crates/proto/src", 1, 1, Some(true)),
                row(2, "lib.rs", "crates/proto/src/lib.rs", 1, 1, None),
                row(1, "ui/src", "crates/ui/src", 45, 12, Some(true)),
                row(2, "settings.rs", "crates/ui/src/settings.rs", 5, 0, None),
                row(
                    2,
                    "transcript.rs",
                    "crates/ui/src/transcript.rs",
                    40,
                    12,
                    None
                ),
                row(0, "README.md", "README.md", 2, 3, None),
            ]
        );
        // One folder folded back against "expand all".
        let overrides = HashMap::from([("crates/ui/src".to_string(), false)]);
        let rows = visible_rows(&tree, true, &overrides);
        assert_eq!(rows.len(), 5);
        assert_eq!(
            rows[3],
            row(1, "ui/src", "crates/ui/src", 45, 12, Some(false))
        );

        // A turn that changed nothing has no card.
        let empty = CheckoutDiff {
            files: Vec::new(),
            ..diff
        };
        assert!(TurnChanges::from_checkout_diff(&empty, ChangesSource::Tools).is_none());
    }

    #[test]
    fn turns_map_to_their_last_reply() {
        use CheckpointPhase::*;
        let checkpoints = vec![
            checkpoint(1, 1, TurnStart, 100, None),
            checkpoint(2, 1, TurnEnd, 160, Some(2)),
            checkpoint(3, 2, TurnStart, 200, Some(0)),
            checkpoint(4, 2, TurnEnd, 260, Some(0)),
            // Turn 3 still running: no end yet.
            checkpoint(5, 3, TurnStart, 300, None),
        ];
        let ranges = turn_ranges(&checkpoints);
        assert_eq!(ranges.len(), 2);
        assert!(!ranges[0].unchanged);
        assert!(ranges[1].unchanged);
        let entries = [
            EntryStamp {
                id: "a1",
                created_ms: 101_000,
            },
            EntryStamp {
                id: "a1b",
                created_ms: 130_000,
            },
            EntryStamp {
                id: "a2",
                created_ms: 199_500,
            },
            EntryStamp {
                id: "a3",
                created_ms: 301_000,
            },
        ];
        let assigned = assign_turns(&ranges, &entries);
        let ids: Vec<(&str, u64)> = assigned.iter().map(|(id, r)| (*id, r.to)).collect();
        assert_eq!(ids, vec![("a1b", 2), ("a2", 4)]);
    }
}
