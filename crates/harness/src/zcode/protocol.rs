//! The ZCode app-server wire, kept in one place: method names, the params
//! Commutator sends, and the decoding of what the server sends back. The
//! session runner above speaks only [`Event`] and [`Ask`], so moving to the
//! v4 protocol (`packages/shared/src/zcode-protocol-v4`, still a draft) means
//! rewriting this file, not the driver.
//!
//! What this targets is the `session/*` surface `zcode app-server --stdio`
//! answers today (`apps/zcode-cli/packages/bootstrap/src/zcode-protocol/
//! server.ts` `dispatchRequest`), recorded live from CLI 0.16.9 (the desktop
//! app's bundle) and 0.18.0 (source build): newline-delimited JSON with the
//! JSON-RPC shape but no `jsonrpc` field (the server's zod schemas are
//! `.strict()` and refuse it), integer ids from us, string ids
//! (`"server-1"`) on the server's own requests.
use serde_json::{Value, json};

/// The oldest CLI this driver has been run against (the desktop app 3.14.0
/// bundles it). `zcode version` prints it; `--version` of the standalone
/// distribution prints its package version instead, so that is not used.
pub const MIN_VERSION: (u64, u64, u64) = (0, 16, 9);

pub const CREATE: &str = "session/create";
pub const RESUME: &str = "session/resume";
pub const SUBSCRIBE: &str = "session/subscribe";
pub const SEND: &str = "session/send";
pub const STOP: &str = "session/stop";
pub const CLOSE: &str = "session/close";
pub const SET_MODEL: &str = "session/setModel";
pub const SET_MODE: &str = "session/setMode";

/// The server answers "no such session" with this code (`Session is not
/// active` / not found) — a resume falls back to a fresh session on it.
pub const SESSION_UNAVAILABLE: i64 = -32004;

/// ZCode's permission modes, least to most trusting: `plan` (read-only),
/// `build` (asks before anything risky or with side effects), `edit` (file
/// edits pass), `yolo` (nothing asks). `guarded` and `auto` are not used.
pub fn mode(access: commutator_proto::AccessMode, plan: bool) -> &'static str {
    use commutator_proto::AccessMode::*;
    if plan {
        return "plan";
    }
    match access {
        FullAccess => "yolo",
        AutoAcceptEdits => "edit",
        Auto | Supervised => "build",
    }
}

/// The workspace ref every session call carries: path and key are the
/// directory itself (what the desktop sends for a local project).
pub fn workspace(cwd: &str) -> Value {
    json!({ "workspacePath": cwd, "workspaceKey": cwd })
}

/// A Commutator model id is `providerId/modelId`; ZCode wants the pair.
pub fn model_selection(id: &str, reasoning: Option<&str>) -> Option<Value> {
    let (provider, model) = id.split_once('/')?;
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    let mut selection = json!({ "providerId": provider, "modelId": model });
    if let Some(level) = reasoning {
        selection["options"] = json!({ "reasoningLevel": level });
    }
    Some(selection)
}

pub fn model_id(selection: &Value) -> Option<String> {
    let provider = selection["providerId"].as_str()?;
    let model = selection["modelId"].as_str()?;
    Some(format!("{provider}/{model}"))
}

/// `mcpServers` entries: stdio servers with `env` as name/value pairs
/// (`zcodeProtocolMcpServerSchema`, strict).
pub fn mcp_servers<'a>(
    servers: impl IntoIterator<Item = &'a commutator_proto::McpServer>,
) -> Vec<Value> {
    servers
        .into_iter()
        .map(|server| {
            let env: Vec<Value> = server
                .env
                .iter()
                .map(|(name, value)| json!({ "name": name, "value": value }))
                .collect();
            json!({
                "name": server.name,
                "command": server.command,
                "args": server.args,
                "env": env,
                "isolation": "session",
                "alwaysLoad": true,
            })
        })
        .collect()
}

pub struct Create<'a> {
    pub cwd: &'a str,
    pub mcp: Vec<Value>,
    pub mode: &'a str,
    pub model: Option<Value>,
    pub thought_level: Option<&'a str>,
    /// `deferred` keeps a session out of the history until it runs a turn
    /// (the model catalog's throwaway session).
    pub deferred: bool,
}

pub fn create_params(create: &Create) -> Value {
    let mut params = json!({ "workspace": workspace(create.cwd), "mode": create.mode });
    if let Some(model) = &create.model {
        params["model"] = model.clone();
    }
    if let Some(level) = create.thought_level {
        params["thoughtLevel"] = json!(level);
    }
    if create.deferred {
        params["persistence"] = json!("deferred");
    }
    if !create.mcp.is_empty() {
        params["mcpServers"] = json!(create.mcp);
    }
    params
}

pub fn resume_params(session: &str, cwd: &str, mcp: &[Value]) -> Value {
    let mut params = json!({ "sessionId": session, "workspace": workspace(cwd) });
    if !mcp.is_empty() {
        params["mcpServers"] = json!(mcp);
    }
    params
}

pub fn set_model_params(session: &str, model: Value) -> Value {
    json!({ "sessionId": session, "model": model })
}

pub fn set_mode_params(session: &str, mode: &str) -> Value {
    json!({ "sessionId": session, "mode": mode })
}

/// After the snapshot: the session's events stream as notifications.
pub fn subscribe_params(session: &str) -> Value {
    json!({ "sessionId": session, "deliveryKind": "desktop-continuous", "includeSnapshot": false })
}

pub fn send_params(session: &str, content: &str) -> Value {
    json!({ "sessionId": session, "content": content })
}

pub fn session_params(session: &str) -> Value {
    json!({ "sessionId": session })
}

/// The session id a create/resume snapshot names.
pub fn snapshot_session(snapshot: &Value) -> Option<&str> {
    snapshot["session"]["sessionId"].as_str()
}

/// One model a snapshot's `settings.model.available` offers.
pub struct OfferedModel {
    pub id: String,
    pub label: String,
    pub levels: Vec<String>,
}

/// `settings.model.available`. Entries name their selection as `ref` (the
/// CLI's `ModelOption`) or flat; both are read.
pub fn offered_models(snapshot: &Value) -> Vec<OfferedModel> {
    let settings = &snapshot["settings"];
    let current_levels: Vec<String> = settings["thoughtLevel"]["available"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|level| level["value"].as_str().map(str::to_owned))
        .collect();
    settings["model"]["available"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let selection = if entry["ref"].is_object() {
                &entry["ref"]
            } else {
                entry
            };
            let id = model_id(selection)?;
            let label = entry["label"]
                .as_str()
                .or_else(|| entry["name"].as_str())
                .or_else(|| selection["modelId"].as_str())
                .unwrap_or_default()
                .to_owned();
            let levels = entry["reasoning"]["levels"]
                .as_array()
                .map(|levels| {
                    levels
                        .iter()
                        .filter_map(|l| l.as_str().or_else(|| l["value"].as_str()))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_else(|| current_levels.clone());
            Some(OfferedModel { id, label, levels })
        })
        .collect()
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    Completed,
    Cancelled,
    Failed(String),
}

/// What the runner acts on, decoded from a notification.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    TurnStarted {
        turn: Option<String>,
        input: String,
    },
    Text(String),
    Reasoning(String),
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        is_error: bool,
        output: Option<String>,
    },
    TurnEnded {
        turn: Option<String>,
        ended: Ended,
        response: Option<String>,
    },
}

/// Decode a server notification for `session`. `None` for frames of other
/// sessions and for what the runner has no use for (telemetry, lifecycle,
/// storage progress, computer-use).
pub fn decode(method: &str, params: &Value, session: &str) -> Option<Event> {
    if params["sessionId"].as_str() != Some(session) {
        return None;
    }
    match method {
        "session/event" => decode_session_event(params),
        _ => None,
    }
}

fn decode_session_event(event: &Value) -> Option<Event> {
    let payload = &event["payload"];
    let turn = event["turnId"].as_str().map(str::to_owned);
    match event["type"].as_str()? {
        "turn.started" => Some(Event::TurnStarted {
            turn,
            input: payload["input"].as_str().unwrap_or_default().to_owned(),
        }),
        "part.delta" => {
            let delta = payload["delta"].as_str()?.to_owned();
            match payload["field"].as_str().unwrap_or("text") {
                "text" => Some(Event::Text(delta)),
                "reasoning" => Some(Event::Reasoning(delta)),
                _ => None,
            }
        }
        "tool.updated" => decode_tool(payload),
        "turn.completed" => {
            let ended = match payload["resultType"].as_str() {
                Some("cancelled") => Ended::Cancelled,
                Some("success") | None => Ended::Completed,
                Some(other) => Ended::Failed(format!("ZCode stopped the turn: {other}")),
            };
            Some(Event::TurnEnded {
                turn,
                ended,
                response: payload["response"].as_str().map(str::to_owned),
            })
        }
        "turn.failed" => {
            let error = &payload["error"];
            let message = error["message"]
                .as_str()
                .or_else(|| error["underlyingErrorMessage"].as_str())
                .unwrap_or("ZCode turn failed");
            Some(Event::TurnEnded {
                turn,
                ended: Ended::Failed(message.to_owned()),
                response: None,
            })
        }
        _ => None,
    }
}

fn decode_tool(payload: &Value) -> Option<Event> {
    // A subagent's mirrored calls belong to its own transcript.
    if payload["parentToolCallId"].is_string() || payload["source"] == "subagent" {
        return None;
    }
    let id = payload["toolCallId"].as_str()?.to_owned();
    match payload["kind"].as_str()? {
        "scheduled" => Some(Event::ToolCall {
            id,
            name: payload["toolName"].as_str().unwrap_or("tool").to_owned(),
            input: payload["input"].clone(),
        }),
        "result" => {
            let result = &payload["result"];
            let is_error = result["isError"].as_bool().unwrap_or(false)
                || result["status"] == "error"
                || result["status"] == "failed";
            Some(Event::ToolResult {
                id,
                is_error,
                output: tool_output(result),
            })
        }
        "error" => Some(Event::ToolResult {
            id,
            is_error: true,
            output: payload["error"]["message"].as_str().map(str::to_owned),
        }),
        _ => None,
    }
}

/// A tool result's text, whichever shape it came in.
fn tool_output(result: &Value) -> Option<String> {
    for key in ["content", "output", "stdout", "text", "message"] {
        match &result[key] {
            Value::String(text) => return Some(text.clone()),
            Value::Array(parts) => {
                let text: Vec<&str> = parts.iter().filter_map(|p| p["text"].as_str()).collect();
                if !text.is_empty() {
                    return Some(text.join("\n"));
                }
            }
            _ => {}
        }
    }
    None
}

/// A request the server sends us.
#[derive(Debug)]
pub enum Ask {
    /// Runtime switches for a session being materialized; answered at once.
    RuntimePreferences,
    Permission(Permission),
    Questions(Vec<Question>),
    /// Anything else (provider headers, browser control, automation, …):
    /// answered "method not found", which the server reads as an older host
    /// and falls back from.
    Other,
}

#[derive(Debug, Clone)]
pub struct Permission {
    pub tool: String,
    pub reason: String,
    pub input: Value,
    /// The `response` of the `allow_once` / `deny` options, sent back
    /// verbatim so the model reads ZCode's own wording.
    pub allow_once: Option<Value>,
    pub deny: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct Question {
    pub question: String,
    pub header: String,
    pub options: Vec<String>,
    pub multi_select: bool,
}

pub fn decode_request(method: &str, params: &Value) -> Ask {
    match method {
        "session/requestRuntimePreferences" => Ask::RuntimePreferences,
        "interaction/requestPermission" => {
            let option = |kind: &str| {
                params["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|option| option["kind"] == kind)
                    .map(|option| option["response"].clone())
            };
            Ask::Permission(Permission {
                tool: params["toolName"].as_str().unwrap_or("a tool").to_owned(),
                reason: params["reason"].as_str().unwrap_or_default().to_owned(),
                input: params["input"].clone(),
                allow_once: option("allow_once"),
                deny: option("deny"),
            })
        }
        "interaction/requestUserInput" => {
            let mut questions: Vec<Question> = params["questions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|q| Question {
                    question: q["question"].as_str().unwrap_or_default().to_owned(),
                    header: q["header"].as_str().unwrap_or("Question").to_owned(),
                    options: q["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|o| o["label"].as_str().map(str::to_owned))
                        .collect(),
                    multi_select: q["multiSelect"].as_bool().unwrap_or(false),
                })
                .collect();
            if questions.is_empty()
                && let Some(prompt) = params["prompt"].as_str().filter(|p| !p.is_empty())
            {
                questions.push(Question {
                    question: prompt.to_owned(),
                    header: "Question".into(),
                    options: vec![],
                    multi_select: false,
                });
            }
            Ask::Questions(questions)
        }
        _ => Ask::Other,
    }
}

/// `session/requestRuntimePreferences`: the result schema is strict and
/// holds only runtime switches (mode, model and thought level go on
/// `session/create`). Questions must reach the person, not be auto-resolved.
pub fn runtime_preferences() -> Value {
    json!({ "nativeSearchEnhancementsEnabled": true, "askUserQuestionAutoResolutionEnabled": false })
}

/// The answer to a permission request: the option's own response, or a
/// bare decision when the server offered none of that kind.
pub fn permission_answer(permission: &Permission, allow: bool) -> Value {
    let offered = if allow {
        &permission.allow_once
    } else {
        &permission.deny
    };
    offered.clone().unwrap_or_else(|| {
        tracing::warn!(
            target: "commutator_harness::zcode",
            tool = %permission.tool,
            allow,
            "ZCode offered no matching permission option; answering with a bare decision"
        );
        json!({ "decision": if allow { "allow" } else { "deny" } })
    })
}

/// `{action: accept, content: {answers: {question: answer}, answer_<i>}}`,
/// as the CLI reads it (`interaction-broker.ts`
/// `normalizeAskUserQuestionAnswers`): `answers[question text]` first, then
/// `answer_<index>`. A question whose text repeats in the request is left
/// out of `answers` so each copy reads its own `answer_<index>`. Several
/// labels go as an array (the CLI joins them with ", "); a free-text answer
/// is just its text. Nothing answered declines.
pub fn questions_answer(questions: &[Question], answers: &[Vec<String>]) -> Value {
    let mut map = serde_json::Map::new();
    let mut content = serde_json::Map::new();
    for (index, (question, labels)) in questions.iter().zip(answers).enumerate() {
        let labels: Vec<&str> = labels
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let answer = match labels.as_slice() {
            [] => continue,
            [one] if !question.multi_select => json!(one),
            many => json!(many),
        };
        let repeated = questions
            .iter()
            .filter(|q| q.question == question.question)
            .count()
            > 1;
        if !repeated {
            map.insert(question.question.clone(), answer.clone());
        }
        content.insert(format!("answer_{index}"), answer);
    }
    if content.is_empty() {
        return json!({ "action": "decline" });
    }
    // An empty `answers` object means "continue with no answers" to the CLI,
    // overriding the `answer_<i>` keys: only send it with something in it.
    if !map.is_empty() {
        content.insert("answers".into(), Value::Object(map));
    }
    json!({ "action": "accept", "content": content })
}

/// Parse `zcode version` output (`0.16.9`).
pub fn parse_version(output: &str) -> Option<(u64, u64, u64)> {
    let word = output
        .split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let mut parts = word
        .split(|c: char| !c.is_ascii_digit())
        .map(|p| p.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
    ))
}
