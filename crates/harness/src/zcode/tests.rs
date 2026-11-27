use super::protocol::{self, Ask, Ended, Event};
use super::*;
use serde_json::json;

/// Recorded frames: `{"dir": ">" | "<", "frame": {...}}` per line.
fn frames(recording: &str) -> Vec<(String, Value)> {
    recording
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let line: Value = serde_json::from_str(l).unwrap();
            (
                line["dir"].as_str().unwrap().to_owned(),
                line["frame"].clone(),
            )
        })
        .collect()
}

/// Every event the runner would act on, in order, for the recorded session.
fn events(recording: &str) -> (String, Vec<Event>) {
    let frames = frames(recording);
    let session = frames
        .iter()
        .find_map(|(_, f)| protocol::snapshot_session(&f["result"]).map(str::to_owned))
        .expect("a create snapshot");
    let events = frames
        .iter()
        .filter(|(dir, f)| dir == "<" && f.get("id").is_none())
        .filter_map(|(_, f)| protocol::decode(f["method"].as_str()?, &f["params"], &session))
        .collect();
    (session, events)
}

const V0_16_9: &str = include_str!("fixtures/turn_failed_0.16.9.jsonl");
const V0_18_0: &str = include_str!("fixtures/turn_failed_0.18.0.jsonl");

#[test]
fn a_turn_without_a_model_fails_with_zcodes_reason_on_both_versions() {
    let (_, old) = events(V0_16_9);
    // 0.16.9 sends no turn.started before failing at model creation.
    assert_eq!(
        old,
        vec![Event::TurnEnded {
            turn: Some("turn_3b66d021-f764-4136-adce-1b5d5678089f".into()),
            ended: Ended::Failed("Select a model before continuing".into()),
            response: None,
        }]
    );
    let (_, new) = events(V0_18_0);
    assert!(matches!(new.first(), Some(Event::TurnStarted { input, .. }) if input == "say hi"));
    assert!(matches!(
        new.last(),
        Some(Event::TurnEnded { ended: Ended::Failed(m), .. }) if m == "Select a model before continuing"
    ));
    assert!(explain_failure("Select a model before continuing").contains("zcode login"));
}

#[test]
fn other_sessions_and_telemetry_are_ignored() {
    let (session, _) = events(V0_18_0);
    for (_, frame) in frames(V0_18_0) {
        let Some(method) = frame["method"].as_str() else {
            continue;
        };
        if method != "session/event" {
            assert_eq!(
                protocol::decode(method, &frame["params"], &session),
                None,
                "{method}"
            );
        }
        assert_eq!(
            protocol::decode(method, &frame["params"], "sess_other"),
            None
        );
    }
}

#[test]
fn server_requests_in_the_recordings_are_answered_by_their_string_id() {
    for recording in [V0_16_9, V0_18_0] {
        let asks: Vec<_> = frames(recording)
            .into_iter()
            .filter(|(dir, f)| dir == "<" && f["id"].is_string() && f["method"].is_string())
            .collect();
        assert!(!asks.is_empty());
        for (_, ask) in asks {
            assert_eq!(
                ask["id"].as_str().map(|id| id.starts_with("server-")),
                Some(true)
            );
            assert!(matches!(
                protocol::decode_request(ask["method"].as_str().unwrap(), &ask["params"]),
                Ask::RuntimePreferences
            ));
        }
    }
    // The answer's schema is strict: only runtime switches.
    let prefs = protocol::runtime_preferences();
    assert_eq!(prefs["askUserQuestionAutoResolutionEnabled"], false);
    assert!(prefs.get("mode").is_none() && prefs.get("model").is_none());
}

#[tokio::test]
#[cfg(unix)]
async fn answers_keep_the_servers_string_id_and_no_version_field() {
    let mut child = tokio::process::Command::new("sh")
        .args(["-c", r#"echo '{"id":"server-1","method":"session/requestRuntimePreferences","params":{}}'; read line; echo "$line" >&2; sleep 2"#])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let (client, mut incoming) = crate::jsonrpc::RpcClient::without_version_field(
        child.stdin.take().unwrap(),
        child.stdout.take().unwrap(),
    );
    let Some(crate::jsonrpc::Incoming::Request { id, method, .. }) = incoming.recv().await else {
        panic!("a server request")
    };
    assert_eq!(method, "session/requestRuntimePreferences");
    client.respond(&id, protocol::runtime_preferences());
    let mut stderr = tokio::io::BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    tokio::io::AsyncBufReadExt::read_line(&mut stderr, &mut line)
        .await
        .unwrap();
    let answer: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(answer["id"], "server-1");
    assert!(answer.get("jsonrpc").is_none());
}

fn permission(options: Value) -> protocol::Permission {
    match protocol::decode_request(
        "interaction/requestPermission",
        &json!({"toolName": "Bash", "reason": "runs a command", "input": {"command": "rm -rf build"}, "options": options}),
    ) {
        Ask::Permission(permission) => permission,
        other => panic!("{other:?}"),
    }
}

#[test]
fn permissions_answer_with_zcodes_own_option_responses() {
    // As `buildProtocolPermissionOptions` offers them.
    let p = permission(json!([
        {"kind": "allow_once", "name": "Allow once", "optionId": "allow_once",
         "response": {"decision": "allow", "reason": "Approved once"}},
        {"kind": "allow_always", "name": "Always allow in this project", "optionId": "allow_project",
         "response": {"decision": "allow", "reason": "Approved for this project",
                      "permissionUpdates": [{"type": "addRules", "behavior": "allow", "rules": [{"toolName": "Bash"}]}]}},
        {"kind": "deny", "name": "Deny", "optionId": "deny",
         "response": {"decision": "deny", "reason": "The user doesn't want to proceed"}},
    ]));
    assert_eq!(
        protocol::permission_answer(&p, true),
        json!({"decision": "allow", "reason": "Approved once"})
    );
    assert_eq!(
        protocol::permission_answer(&p, false),
        json!({"decision": "deny", "reason": "The user doesn't want to proceed"})
    );
    assert_eq!(
        permission_question(&p).question,
        "ZCode wants to run `rm -rf build`. Allow it?"
    );
    // Nothing offered: a bare decision.
    let bare = permission(json!([]));
    assert_eq!(
        protocol::permission_answer(&bare, true),
        json!({"decision": "allow"})
    );
    assert_eq!(
        protocol::permission_answer(&bare, false),
        json!({"decision": "deny"})
    );
}

fn question(text: &str, multi: bool) -> protocol::Question {
    protocol::Question {
        question: text.into(),
        header: "h".into(),
        options: vec!["A".into(), "B".into()],
        multi_select: multi,
    }
}

#[test]
fn answers_go_by_question_text_and_index_like_the_cli_reads_them() {
    let s = |v: &[&str]| v.iter().map(|l| l.to_string()).collect::<Vec<_>>();
    // One pick, several picks (an array the CLI joins), and free text.
    assert_eq!(
        protocol::questions_answer(
            &[
                question("Which?", false),
                question("Which ones?", true),
                question("Why?", false)
            ],
            &[s(&["A"]), s(&["A", "B"]), s(&["  because  "])]
        ),
        json!({"action": "accept", "content": {
            "answers": {"Which?": "A", "Which ones?": ["A", "B"], "Why?": "because"},
            "answer_0": "A", "answer_1": ["A", "B"], "answer_2": "because",
        }})
    );
    // The same text twice: each copy reads its own `answer_<index>`.
    assert_eq!(
        protocol::questions_answer(
            &[question("Sure?", false), question("Sure?", false)],
            &[s(&["A"]), s(&["B"])]
        ),
        json!({"action": "accept", "content": {"answer_0": "A", "answer_1": "B"}})
    );
    // Nothing answered declines.
    assert_eq!(
        protocol::questions_answer(&[question("Which?", false)], &[s(&[" "])]),
        json!({"action": "decline"})
    );
}

#[test]
fn turn_and_tool_events_decode_from_their_schema_shapes() {
    let event = |kind: &str, payload: Value| {
        protocol::decode(
            "session/event",
            &json!({"sessionId": "s", "turnId": "t", "type": kind, "payload": payload}),
            "s",
        )
    };
    assert_eq!(
        event(
            "part.delta",
            json!({"messageId": "m", "partId": "p", "field": "text", "delta": "hi"})
        ),
        Some(Event::Text("hi".into()))
    );
    assert_eq!(
        event(
            "part.delta",
            json!({"messageId": "m", "partId": "p", "field": "reasoning", "delta": "hm"})
        ),
        Some(Event::Reasoning("hm".into()))
    );
    assert_eq!(
        event(
            "turn.completed",
            json!({"resultType": "cancelled", "response": ""})
        ),
        Some(Event::TurnEnded {
            turn: Some("t".into()),
            ended: Ended::Cancelled,
            response: Some(String::new())
        })
    );
    assert!(matches!(
        event("tool.updated", json!({"kind": "scheduled", "toolCallId": "c", "toolName": "Bash", "input": {"command": "ls"}})),
        Some(Event::ToolCall { name, .. }) if name == "Bash"
    ));
    // A subagent's mirrored call stays out of the parent feed.
    assert_eq!(
        event(
            "tool.updated",
            json!({"kind": "scheduled", "toolCallId": "c", "toolName": "Bash", "parentToolCallId": "p"})
        ),
        None
    );
    assert_eq!(
        tool_call("Bash", &json!({"command": "ls"})),
        ToolCall::Exec {
            command: "ls".into()
        }
    );
}

#[test]
fn modes_and_models_map_both_ways() {
    assert_eq!(protocol::mode(AccessMode::Supervised, false), "build");
    assert_eq!(protocol::mode(AccessMode::AutoAcceptEdits, false), "edit");
    assert_eq!(protocol::mode(AccessMode::FullAccess, false), "yolo");
    assert_eq!(protocol::mode(AccessMode::FullAccess, true), "plan");
    assert_eq!(
        protocol::model_selection("zai/glm-4.6", Some("high")),
        Some(
            json!({"providerId": "zai", "modelId": "glm-4.6", "options": {"reasoningLevel": "high"}})
        )
    );
    assert_eq!(protocol::model_selection("glm", None), None);
    assert_eq!(
        thought_level(Some(ReasoningLevel::XHigh)).as_deref(),
        Some("xhigh")
    );
    let models = catalog(&json!({"settings": {
        "model": {"available": [{"ref": {"providerId": "zai", "modelId": "glm-4.6"}, "label": "GLM-4.6"}]},
        "thoughtLevel": {"available": [{"label": "high", "value": "high"}], "enabled": true},
    }}));
    assert_eq!(models[0].id, "zai/glm-4.6");
    assert_eq!(models[0].reasoning_levels, vec![ReasoningLevel::High]);
}

#[test]
fn versions_read_from_zcode_version() {
    assert_eq!(protocol::parse_version("0.16.9\n"), Some((0, 16, 9)));
    assert_eq!(protocol::parse_version("zcode 0.18.0"), Some((0, 18, 0)));
    assert!(protocol::parse_version("0.16.8").unwrap() < protocol::MIN_VERSION);
}

#[test]
#[cfg(unix)]
fn the_desktop_launcher_quotes_paths_and_follows_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("Z Code's.app");
    std::fs::create_dir_all(fake.join("Contents/MacOS")).unwrap();
    std::fs::create_dir_all(fake.join("Contents/Resources/glm")).unwrap();
    // A fake "Electron" that prints its environment and arguments.
    std::fs::write(
        fake.join("Contents/MacOS/ZCode"),
        "#!/bin/sh\necho \"$ELECTRON_RUN_AS_NODE|$ZCODE_BUILTIN_PROVIDER_CONFIG_FILE|$*\"\n",
    )
    .unwrap();
    std::fs::write(fake.join("Contents/Resources/glm/zcode.cjs"), "").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        fake.join("Contents/MacOS/ZCode"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let config = fake.join("Contents/Resources/config/provider/zcode-builtin.json");
    let script = launcher_script(
        &fake.join("Contents/MacOS/ZCode"),
        &fake.join("Contents/Resources/glm/zcode.cjs"),
        Some(&config),
    );
    let launcher = dir.path().join("launcher");
    std::fs::write(&launcher, script).unwrap();
    let out = std::process::Command::new("sh")
        .arg(&launcher)
        .args(["app-server", "--stdio"])
        .output()
        .unwrap();
    let out = String::from_utf8(out.stdout).unwrap();
    let bundle = fake.join("Contents/Resources/glm/zcode.cjs");
    assert_eq!(
        out.trim(),
        format!(
            "1|{}|{} app-server --stdio",
            config.display(),
            bundle.display()
        )
    );
}
