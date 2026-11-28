#![cfg(unix)]
//! The ZCode driver against the real `zcode` on this machine — skipped unless
//! `ZCODE_LIVE=1` (it needs a signed-in ZCode with a model). `ZCODE_MODEL`
//! picks the model (`providerId/modelId`), `ZCODE_EXECUTABLE` the CLI.
//!
//! cargo test -p commutator-harness --test zcode_live -- --nocapture --test-threads 1
use commutator_harness::{
    CancellationToken, Harness, RunControls, SteerMessage, TurnControl, ZcodeHarness,
};
use commutator_proto::{
    AccessMode, AgentEvent, DoneStatus, RunRequest, SandboxLevel, UserInputAnswer,
};
use futures::StreamExt;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

fn live() -> bool {
    std::env::var("ZCODE_LIVE").is_ok_and(|v| v == "1")
}

fn request(
    cwd: &std::path::Path,
    prompt: &str,
    access: AccessMode,
    resume: Option<String>,
) -> RunRequest {
    RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: std::env::var("ZCODE_MODEL").ok(),
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        access,
        plan: false,
        code_mode: false,
        resume,
        attachments: vec![],
        worktree: None,
        mcp: None,
        env: Default::default(),
        extra_mcp: Vec::new(),
    }
}

/// Controls that answer every approval with `answer` and log what was asked.
fn controls(answer: &'static str) -> (RunControls, mpsc::Sender<SteerMessage>, TurnControl) {
    let (steer, steering) = mpsc::channel(8);
    let turn = TurnControl::default();
    (
        RunControls {
            execution_lease: None,
            steering,
            interrupt: CancellationToken::new(),
            request_input: Box::new(move |questions| {
                eprintln!("ASKED {questions:?}");
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(
                    questions
                        .iter()
                        .map(|q| UserInputAnswer {
                            question_id: q.id.clone(),
                            labels: vec![answer.into()],
                        })
                        .collect(),
                );
                rx
            }),
            turn: turn.clone(),
        },
        steer,
        turn,
    )
}

type Events =
    futures::stream::BoxStream<'static, Result<AgentEvent, commutator_harness::HarnessError>>;

async fn until_done(stream: &mut Events, mut on: impl FnMut(&AgentEvent)) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(180), stream.next())
            .await
            .expect("turn timed out")
            .expect("stream ended before Done")
            .unwrap();
        eprintln!("EVENT {event:?}");
        on(&event);
        let done = matches!(event, AgentEvent::Done { .. });
        events.push(event);
        if done {
            return events;
        }
    }
}

fn session_of(events: &[AgentEvent]) -> String {
    events
        .iter()
        .find_map(|e| match e {
            AgentEvent::SessionStarted { session_id, .. } => Some(session_id.clone()),
            _ => None,
        })
        .unwrap()
}

fn status(events: &[AgentEvent]) -> DoneStatus {
    match events.last() {
        Some(AgentEvent::Done { status, .. }) => *status,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn models_come_from_a_throwaway_session() {
    if !live() {
        return;
    }
    let models = ZcodeHarness::new().models().await.unwrap();
    eprintln!("MODELS {models:?}");
    assert!(!models.is_empty());
}

#[tokio::test]
async fn a_supervised_tool_turn_asks_then_resumes_with_context() {
    if !live() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("zcode-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let harness = ZcodeHarness::new();
    let (run_controls, _steer, _turn) = controls("Yes");
    let mut stream = harness
        .run(
            request(
                &dir,
                "Create a file named hello.txt containing exactly the word banana, using a shell command. Then reply DONE.",
                AccessMode::Supervised,
                None,
            ),
            run_controls,
        )
        .await
        .unwrap();
    let events = until_done(&mut stream, |_| {}).await;
    assert_eq!(status(&events), DoneStatus::Completed, "{events:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCall { .. }))
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("hello.txt"))
            .unwrap()
            .trim(),
        "banana"
    );
    let session = session_of(&events);
    drop(stream);
    tokio::time::sleep(Duration::from_secs(2)).await;

    // A new runtime resumes the session and remembers it.
    let (run_controls, _steer, _turn) = controls("Yes");
    let mut stream = harness
        .run(
            request(
                &dir,
                "What word did you write into hello.txt? Answer with the word only.",
                AccessMode::Supervised,
                Some(session.clone()),
            ),
            run_controls,
        )
        .await
        .unwrap();
    let events = until_done(&mut stream, |_| {}).await;
    assert_eq!(session_of(&events), session);
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.to_lowercase().contains("banana"), "{text}");
}

#[tokio::test]
async fn a_stop_ends_the_turn_in_place() {
    if !live() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("zcode-live-stop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (run_controls, _steer, turn) = controls("Yes");
    let mut stream = ZcodeHarness::new()
        .run(
            request(
                &dir,
                "Run the shell command `sleep 60` and then say done.",
                AccessMode::FullAccess,
                None,
            ),
            run_controls,
        )
        .await
        .unwrap();
    let mut stopped = false;
    let events = until_done(&mut stream, |event| {
        if !stopped && matches!(event, AgentEvent::ToolCall { .. }) {
            stopped = true;
            turn.stop_turn();
        }
    })
    .await;
    assert!(stopped);
    assert_eq!(status(&events), DoneStatus::Interrupted, "{events:?}");
}
