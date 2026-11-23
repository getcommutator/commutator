use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use vardoger_proto::checkpoints::CheckpointPhase;

use super::{Checkpoints, Inner};
use crate::repos::Repos;

async fn git(cwd: &Path, args: &[&str]) -> String {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .await
        .expect("git spawns");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Every file under `root` but `.git`, with its contents.
fn files(root: &Path) -> BTreeMap<String, String> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// A repo with `a.txt`, `b.txt` and `.gitignore` (`*.log`) committed, plus
/// an untracked `u.txt` and an ignored `keep.log`.
async fn fixture(tmp: &Path) -> (PathBuf, Checkpoints) {
    let root = tmp.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]).await;
    write(&root, "a.txt", "a1\n");
    write(&root, "b.txt", "b1\n");
    write(&root, ".gitignore", "*.log\n");
    git(&root, &["add", "."]).await;
    git(&root, &["commit", "-q", "-m", "initial"]).await;
    write(&root, "u.txt", "u1\n");
    write(&root, "keep.log", "log1\n");
    let root = std::fs::canonicalize(&root).unwrap();
    let data_dir = tmp.join("data");
    let repos = Repos::with_worktrees_root(&data_dir, "device-test", data_dir.join("worktrees"));
    (root, Checkpoints::new(repos))
}

fn with_cap(checkpoints: Checkpoints, cap: usize) -> Checkpoints {
    let inner = Arc::try_unwrap(checkpoints.inner).ok().expect("sole owner");
    Checkpoints {
        inner: Arc::new(Inner { cap, ..inner }),
    }
}

#[tokio::test]
async fn rollback_restores_the_exact_tree_and_leaves_ignored_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, checkpoints) = fixture(tmp.path()).await;
    let cwd = root.to_string_lossy().into_owned();
    let at_start = files(&root);

    checkpoints.note_turn_start("chat-1", &cwd);
    // The turn: edit, delete, add tracked-dir and untracked files, touch ignored ones.
    let list = checkpoints.list("chat-1").await.unwrap();
    assert_eq!(list.len(), 1);
    write(&root, "a.txt", "a2\n");
    std::fs::remove_file(root.join("b.txt")).unwrap();
    write(&root, "u.txt", "u2\n");
    write(&root, "new.txt", "new\n");
    write(&root, "dir/deep/new2.txt", "new2\n");
    write(&root, "keep.log", "log2\n");
    write(&root, "dir/fresh.log", "fresh\n");
    checkpoints.note_turn_end("chat-1");
    let after_turn = files(&root);

    let list = checkpoints.list("chat-1").await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].phase, CheckpointPhase::TurnStart);
    assert_eq!(list[1].phase, CheckpointPhase::TurnEnd);
    assert_eq!((list[0].turn, list[1].turn), (1, 1));
    assert!(list[1].git_ref.starts_with("refs/vardoger/checkpoints/chat-1/"));
    // a.txt, b.txt, u.txt, new.txt, dir/deep/new2.txt.
    assert_eq!(list[1].files_changed, Some(5));

    let head = git(&root, &["rev-parse", "HEAD"]).await;
    let index = std::fs::read(root.join(".git/index")).unwrap();
    let outcome = checkpoints.rollback("chat-1", list[0].id).await.unwrap();
    assert_eq!(outcome.restored.id, list[0].id);
    assert_eq!(outcome.before.phase, CheckpointPhase::BeforeRollback);
    assert_eq!((outcome.files_restored, outcome.files_deleted), (3, 2));

    let mut expected = at_start.clone();
    // Ignored files are not the checkpoint's: they stay as the turn left them.
    expected.insert("keep.log".into(), "log2\n".into());
    expected.insert("dir/fresh.log".into(), "fresh\n".into());
    assert_eq!(files(&root), expected);
    assert!(!root.join("dir/deep").exists(), "emptied directories go");
    assert_eq!(git(&root, &["rev-parse", "HEAD"]).await, head);
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);

    // The tree the rollback replaced is a checkpoint too: rolling back to it undoes it.
    checkpoints.rollback("chat-1", outcome.before.id).await.unwrap();
    assert_eq!(files(&root), after_turn);
}

#[tokio::test]
async fn capture_leaves_the_checkout_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, checkpoints) = fixture(tmp.path()).await;
    // Something staged, something not, something untracked.
    write(&root, "a.txt", "staged\n");
    git(&root, &["add", "a.txt"]).await;
    write(&root, "a.txt", "worktree\n");
    write(&root, "big-new.txt", "untracked content\n");

    let read_state = |root: PathBuf| async move {
        (
            std::fs::read(root.join(".git/index")).unwrap(),
            git(&root, &["rev-parse", "HEAD"]).await,
            git(&root, &["symbolic-ref", "HEAD"]).await,
            git(&root, &["for-each-ref"]).await,
            git(&root, &["stash", "list"]).await,
            git(&root, &["count-objects", "-v"]).await,
            git(&root, &["--no-optional-locks", "status", "--porcelain"]).await,
        )
    };
    let before = read_state(root.clone()).await;
    checkpoints.note_turn_start("chat-1", &root.to_string_lossy());
    checkpoints.note_turn_end("chat-1");
    let list = checkpoints.list("chat-1").await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(read_state(root.clone()).await, before);

    // And a diff from the start sees nothing changed since.
    let (diff_root, diff) = checkpoints.diff("chat-1", list[0].id, None).await.unwrap();
    assert_eq!(diff_root, root);
    assert!(diff.files.is_empty(), "{:?}", diff.files);
}

#[tokio::test]
async fn diffs_between_checkpoints_and_to_the_working_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, checkpoints) = fixture(tmp.path()).await;
    let cwd = root.to_string_lossy().into_owned();
    checkpoints.note_turn_start("chat-1", &cwd);
    // Captures run in the background; listing waits for them.
    checkpoints.list("chat-1").await.unwrap();
    write(&root, "a.txt", "a2\n");
    checkpoints.note_turn_end("chat-1");
    let list = checkpoints.list("chat-1").await.unwrap();
    write(&root, "later.txt", "later\n");

    let paths = |diff: &crate::diff_sync::DiffSnapshot| {
        diff.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>()
    };
    let (_, between) = checkpoints.diff("chat-1", list[0].id, Some(list[1].id)).await.unwrap();
    assert_eq!(paths(&between), ["a.txt"]);
    assert!(between.patch.contains("+a2"));
    let (_, to_now) = checkpoints.diff("chat-1", list[0].id, None).await.unwrap();
    assert_eq!(paths(&to_now), ["a.txt", "later.txt"]);
}

#[tokio::test]
async fn a_cwd_outside_git_records_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("x.txt"), "x").unwrap();
    let data_dir = tmp.path().join("data");
    let checkpoints = Checkpoints::new(Repos::with_worktrees_root(
        &data_dir,
        "device-test",
        data_dir.join("worktrees"),
    ));
    checkpoints.note_turn_start("chat-1", &plain.to_string_lossy());
    checkpoints.note_turn_end("chat-1");
    assert!(checkpoints.list("chat-1").await.unwrap().is_empty());
    assert!(!data_dir.join("checkpoints").join("stores").exists());
}

#[tokio::test]
async fn rollback_is_refused_while_a_turn_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, checkpoints) = fixture(tmp.path()).await;
    checkpoints.note_turn_start("chat-1", &root.to_string_lossy());
    let list = checkpoints.list("chat-1").await.unwrap();
    write(&root, "a.txt", "mid-turn\n");
    checkpoints.set_busy_probe(Arc::new(|chat| chat == "chat-1"));

    let error = checkpoints.rollback("chat-1", list[0].id).await.unwrap_err();
    assert!(error.to_string().contains("turn is running"), "{error}");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "mid-turn\n");
    assert_eq!(checkpoints.list("chat-1").await.unwrap().len(), 1);
}

#[tokio::test]
async fn old_checkpoints_are_trimmed_and_forget_drops_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, checkpoints) = fixture(tmp.path()).await;
    let checkpoints = with_cap(checkpoints, 3);
    let cwd = root.to_string_lossy().into_owned();
    for turn in 0..3 {
        checkpoints.note_turn_start("chat-1", &cwd);
        checkpoints.list("chat-1").await.unwrap();
        write(&root, "a.txt", &format!("turn {turn}\n"));
        checkpoints.note_turn_end("chat-1");
    }
    let list = checkpoints.list("chat-1").await.unwrap();
    assert_eq!(list.iter().map(|c| c.id).collect::<Vec<_>>(), [3, 4, 5]);
    assert_eq!(list[0].turn, 2);
    let store = tmp.path().join("data/checkpoints/stores");
    let store = std::fs::read_dir(&store).unwrap().next().unwrap().unwrap().path();
    let refs = git(&store, &["for-each-ref", "--format=%(refname)"]).await;
    assert_eq!(refs.lines().count(), 3, "{refs}");

    checkpoints.forget("chat-1").await.unwrap();
    assert!(checkpoints.list("chat-1").await.unwrap().is_empty());
    assert_eq!(git(&store, &["for-each-ref"]).await, "");
}
