//! One ZCode session over one `zcode app-server --stdio`: open (or resume)
//! it, run its turns, carry Commutator's steers, stops and answers in.
//!
//! - A turn starts with `session/send`; it ends on the session's own
//!   `turn.completed` / `turn.failed` event, so the end is deterministic.
//! - ZCode refuses a `session/send` while a turn runs ("A prompt is already
//!   running", -32010): a prompt that arrives mid-turn waits here and is sent
//!   when the turn ends, as the next turn of the same run (turn boundary).
//! - A model is selected after the session exists (`session/setModel`), with
//!   a reasoning level always: the chosen one, else the model's own default
//!   from the snapshot — ZCode fails the turn ("Reasoning level is required")
//!   for a selection without one.
//! - A stop is `session/stop`; the turn then ends `cancelled` and the
//!   process (and its session) stays up.
//! - The server's own requests are answered here: runtime preferences at
//!   once, permissions and questions through [`RunControls::request_input`]
//!   (or decided by the access mode), the rest "method not found".
use super::protocol::{self, Ask, Ended, Event};
use crate::jsonrpc::{Incoming, RpcClient};
use crate::{HarnessError, SteerMessage, TurnControl};
use commutator_proto::{AgentEvent, DoneStatus, HarnessId, RunRequest, UserInputQuestion};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

pub(super) type Input = dyn Fn(
        Vec<UserInputQuestion>,
    ) -> tokio::sync::oneshot::Receiver<Vec<commutator_proto::UserInputAnswer>>
    + Send
    + Sync;

pub(super) struct Runner {
    pub client: RpcClient,
    pub incoming: mpsc::Receiver<Incoming>,
    pub tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
    pub request: RunRequest,
    pub input: Option<Arc<Input>>,
    pub turn: TurnControl,
    pub read_only: bool,
    session: String,
    assistant: String,
    /// A turn is running (or its `session/send` is in flight).
    active: bool,
    /// Prompts that arrived while a turn ran, oldest first.
    queued: std::collections::VecDeque<SteerMessage>,
    stopping: bool,
    pub error: Option<String>,
    lost_context: Option<String>,
    /// Notifications that arrived while a call of ours was pending, in order.
    backlog: std::collections::VecDeque<Incoming>,
}

impl Runner {
    pub fn new(
        client: RpcClient,
        incoming: mpsc::Receiver<Incoming>,
        tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
        request: RunRequest,
        read_only: bool,
    ) -> Self {
        Self {
            client,
            incoming,
            tx,
            request,
            input: None,
            turn: TurnControl::default(),
            read_only,
            session: String::new(),
            assistant: uuid::Uuid::new_v4().to_string(),
            active: false,
            queued: Default::default(),
            stopping: false,
            error: None,
            lost_context: None,
            backlog: Default::default(),
        }
    }

    async fn emit(&self, event: AgentEvent) -> Result<(), HarnessError> {
        self.tx
            .send(Ok(event))
            .await
            .map_err(|_| HarnessError::Protocol("ZCode event consumer closed".into()))
    }

    fn mode(&self) -> &'static str {
        if self.read_only {
            return "plan";
        }
        let access = if self.request.auto_approve {
            commutator_proto::AccessMode::FullAccess
        } else {
            self.request.access
        };
        protocol::mode(access, self.request.plan)
    }

    fn model_id(&self) -> Option<&str> {
        let id = self.request.model.as_deref()?.trim();
        (!id.is_empty() && id != "default").then_some(id)
    }

    /// The requested model with a reasoning level: the requested one, else
    /// the default `snapshot` lists for it.
    fn model(&self, snapshot: &Value) -> Option<Value> {
        let id = self.model_id()?;
        let level = super::thought_level(self.request.reasoning).or_else(|| {
            protocol::offered_models(snapshot)
                .into_iter()
                .find(|m| m.id == id)
                .and_then(|m| m.default_level)
        });
        protocol::model_selection(id, level.as_deref())
    }

    /// Select the requested model on the open session; refused selections
    /// (a model the provider doesn't serve) surface as ZCode words them.
    async fn select_model(&mut self, snapshot: &Value) -> Result<(), HarnessError> {
        let Some(model) = self.model(snapshot) else {
            return Ok(());
        };
        self.call(
            protocol::SET_MODEL,
            protocol::set_model_params(&self.session, model),
        )
        .await
        .map(drop)
    }

    fn mcp(&self) -> Vec<Value> {
        if self.read_only {
            return vec![];
        }
        protocol::mcp_servers(
            self.request
                .extra_mcp
                .iter()
                .chain(self.request.mcp.as_ref()),
        )
    }

    /// Answer server requests that arrive while a call of ours is pending.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, HarnessError> {
        let pending = self.client.request_now(method, params);
        tokio::pin!(pending);
        loop {
            tokio::select! {
                result = &mut pending => return result,
                frame = self.incoming.recv() => match frame {
                    Some(Incoming::Request { id, method, params }) => self.answer(id, &method, &params),
                    Some(Incoming::Eof) | None => {
                        return Err(HarnessError::Protocol(format!("{method}: ZCode exited")));
                    }
                    Some(frame) => self.backlog.push_back(frame),
                },
            }
        }
    }

    async fn open(&mut self) -> Result<Value, HarnessError> {
        let cwd = self.request.cwd.clone();
        let mcp = self.mcp();
        if let Some(id) = self.request.resume.clone() {
            match self
                .call(protocol::RESUME, protocol::resume_params(&id, &cwd, &mcp))
                .await
            {
                Ok(snapshot) => {
                    self.session = id;
                    self.call(
                        protocol::SET_MODE,
                        protocol::set_mode_params(&self.session, self.mode()),
                    )
                    .await?;
                    self.select_model(&snapshot).await?;
                    return Ok(snapshot);
                }
                Err(error) if gone(&error) => {
                    self.lost_context = Some(format!(
                        "ZCode could not restore session {id}; starting a new session without \
                         the previous context: {error}"
                    ));
                    self.request.resume = None;
                }
                Err(error) => return Err(error),
            }
        }
        let thought = super::thought_level(self.request.reasoning);
        let snapshot = self
            .call(
                protocol::CREATE,
                protocol::create_params(&protocol::Create {
                    cwd: &cwd,
                    mcp,
                    mode: self.mode(),
                    model: None,
                    thought_level: thought.as_deref(),
                    deferred: false,
                }),
            )
            .await?;
        self.session = protocol::snapshot_session(&snapshot)
            .ok_or_else(|| HarnessError::Protocol("ZCode created a session without an id".into()))?
            .to_owned();
        self.select_model(&snapshot).await?;
        Ok(snapshot)
    }

    pub async fn run(
        &mut self,
        steering: &mut mpsc::Receiver<SteerMessage>,
    ) -> Result<(), HarnessError> {
        let snapshot = self.open().await?;
        self.call(
            protocol::SUBSCRIBE,
            protocol::subscribe_params(&self.session),
        )
        .await?;
        let model = self
            .request
            .model
            .clone()
            .or_else(|| {
                snapshot["settings"]["model"]["current"]
                    .as_object()
                    .and_then(|m| protocol::model_id(&Value::Object(m.clone())))
            })
            .unwrap_or_else(|| "default".into());
        self.emit(AgentEvent::SessionStarted {
            harness: HarnessId::Zcode,
            model,
            tools: vec![],
            cwd: self.request.cwd.clone(),
            session_id: self.session.clone(),
            assistant_message_id: self.assistant.clone(),
        })
        .await?;
        if let Some(message) = self.lost_context.take() {
            self.emit(AgentEvent::Error { message }).await?;
        }
        let prompt = if self.read_only {
            format!(
                "{}\n\n<session_request>\n{}\n</session_request>",
                crate::TITLE_INSTRUCTIONS,
                self.request.prompt
            )
        } else {
            super::with_attachments(&self.request.prompt, &self.request.attachments)
        };
        self.active = true;
        self.send(&prompt).await?;
        let mut open = !self.read_only;
        loop {
            if let Some(frame) = self.backlog.pop_front() {
                self.frame(frame).await?;
                continue;
            }
            if !open && !self.active {
                return Ok(());
            }
            tokio::select! {
                frame = self.incoming.recv() => match frame {
                    Some(frame) => self.frame(frame).await?,
                    None => return Err(HarnessError::Protocol("ZCode disconnected".into())),
                },
                steer = steering.recv(), if open => match steer {
                    Some(steer) => self.steer(steer).await?,
                    None => open = false,
                },
                _ = self.turn.stop_requested() => self.stop(),
            }
        }
    }

    async fn send(&mut self, prompt: &str) -> Result<(), HarnessError> {
        let content = if prompt.trim().is_empty() {
            "(no message)"
        } else {
            prompt
        };
        let ack = self
            .call(
                protocol::SEND,
                protocol::send_params(&self.session, content),
            )
            .await?;
        if ack["accepted"] == false {
            return Err(HarnessError::Protocol(
                "ZCode did not take the message".into(),
            ));
        }
        Ok(())
    }

    async fn steer(&mut self, steer: SteerMessage) -> Result<(), HarnessError> {
        if self.active {
            self.queued.push_back(steer);
            return Ok(());
        }
        self.begin();
        self.deliver(steer).await
    }

    /// Send a prompt between turns, with the configuration it came with.
    async fn deliver(&mut self, steer: SteerMessage) -> Result<(), HarnessError> {
        if let Some(next) = steer.config {
            let model_changed =
                next.model != self.request.model || next.reasoning != self.request.reasoning;
            let mode_changed = next.access != self.request.access || next.plan != self.request.plan;
            self.request.model = next.model.clone();
            self.request.reasoning = next.reasoning;
            self.request.access = next.access;
            self.request.plan = next.plan;
            if model_changed {
                let snapshot = self
                    .call(protocol::READ, protocol::session_params(&self.session))
                    .await?;
                self.select_model(&snapshot).await?;
            }
            if mode_changed {
                let mode = self.mode();
                self.call(
                    protocol::SET_MODE,
                    protocol::set_mode_params(&self.session, mode),
                )
                .await?;
            }
        }
        let prompt = super::with_attachments(&steer.prompt, &steer.attachments);
        self.send(&prompt).await
    }

    fn begin(&mut self) {
        self.active = true;
        self.stopping = false;
        self.error = None;
    }

    fn stop(&mut self) {
        if !self.active || self.stopping {
            return;
        }
        self.stopping = true;
        // The ack is not awaited: the turn's `cancelled` end is what counts.
        drop(
            self.client
                .request_now(protocol::STOP, protocol::session_params(&self.session)),
        );
    }

    /// Runtime teardown: stop the turn and give ZCode `grace` to end it.
    pub async fn interrupt(&mut self, grace: std::time::Duration) {
        if !self.active || self.session.is_empty() {
            return;
        }
        self.stop();
        let _ = tokio::time::timeout(grace, async {
            while self.active {
                let frame = match self.backlog.pop_front() {
                    Some(frame) => frame,
                    None => match self.incoming.recv().await {
                        Some(frame) => frame,
                        None => break,
                    },
                };
                if self.frame(frame).await.is_err() {
                    break;
                }
            }
        })
        .await;
    }

    /// The run's last word for a turn still open (teardown, crash).
    pub async fn finish(&mut self) -> Result<(), HarnessError> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let status = if self.error.is_some() {
            DoneStatus::Errored
        } else {
            DoneStatus::Interrupted
        };
        self.emit(AgentEvent::Done {
            status,
            result: None,
            error: self.error.clone(),
            session_id: (!self.session.is_empty()).then(|| self.session.clone()),
        })
        .await
    }

    async fn frame(&mut self, frame: Incoming) -> Result<(), HarnessError> {
        match frame {
            Incoming::Notification { method, params } => {
                match protocol::decode(&method, &params, &self.session) {
                    Some(event) => self.event(event).await,
                    None => Ok(()),
                }
            }
            Incoming::Request { id, method, params } => {
                self.answer(id, &method, &params);
                Ok(())
            }
            Incoming::Eof => Err(HarnessError::Protocol("ZCode exited".into())),
        }
    }

    async fn event(&mut self, event: Event) -> Result<(), HarnessError> {
        match event {
            Event::TurnStarted { .. } => {
                if !self.active {
                    // A turn nobody here submitted (a background wake).
                    self.turn.note_wake();
                    self.begin();
                }
            }
            Event::Text(text) if self.active => self.emit(AgentEvent::TextDelta { text }).await?,
            Event::Reasoning(text) if self.active => {
                self.emit(AgentEvent::ReasoningDelta { text }).await?
            }
            Event::ToolCall { id, name, input } if self.active => {
                self.emit(AgentEvent::ToolCall {
                    id,
                    call: super::tool_call(&name, &input),
                })
                .await?
            }
            Event::ToolResult {
                id,
                is_error,
                output,
                exit_code,
            } if self.active => {
                self.emit(AgentEvent::ToolResult {
                    id,
                    is_error,
                    output: output.map(|o| super::cap(&o)),
                    diff: None,
                    preview_paths: vec![],
                    image_payloads: vec![],
                    exit_code,
                })
                .await?
            }
            Event::TurnEnded { ended, .. } => {
                // A prompt that waited for this turn runs next, in the same run.
                if ended == Ended::Completed
                    && let Some(steer) = self.queued.pop_front()
                {
                    let old =
                        std::mem::replace(&mut self.assistant, uuid::Uuid::new_v4().to_string());
                    self.emit(AgentEvent::Steered {
                        assistant_message_id: Some(old),
                        next_assistant_message_id: Some(self.assistant.clone()),
                    })
                    .await?;
                    return self.deliver(steer).await;
                }
                // A stop or a failure ends the run's turn; what waited is dropped.
                self.queued.clear();
                let (status, error) = match ended {
                    Ended::Completed => (DoneStatus::Completed, None),
                    Ended::Cancelled => (DoneStatus::Interrupted, None),
                    Ended::Failed(message) => {
                        (DoneStatus::Errored, Some(super::explain_failure(&message)))
                    }
                };
                self.active = false;
                self.stopping = false;
                self.emit(AgentEvent::Done {
                    status,
                    result: None,
                    error,
                    session_id: Some(self.session.clone()),
                })
                .await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Answer a server request; questions and supervised permissions are
    /// asked off the event loop.
    fn answer(&self, id: Value, method: &str, params: &Value) {
        let client = self.client.clone();
        match protocol::decode_request(method, params) {
            Ask::RuntimePreferences => client.respond(&id, protocol::runtime_preferences()),
            Ask::Permission(permission) => {
                let full = !self.read_only && super::full_access(&self.request);
                let input = self.input.clone().filter(|_| !self.read_only && !full);
                let Some(input) = input else {
                    client.respond(&id, protocol::permission_answer(&permission, full));
                    return;
                };
                let question = super::permission_question(&permission);
                let answers = input(vec![question.clone()]);
                tokio::spawn(async move {
                    let answers = answers.await.unwrap_or_default();
                    let allow = crate::approval::approved(&question, &answers);
                    client.respond(&id, protocol::permission_answer(&permission, allow));
                });
            }
            Ask::Questions(questions) => {
                let Some(input) = self
                    .input
                    .clone()
                    .filter(|_| !self.read_only && !questions.is_empty())
                else {
                    client.respond(&id, serde_json::json!({ "action": "decline" }));
                    return;
                };
                let asked: Vec<UserInputQuestion> = questions
                    .iter()
                    .enumerate()
                    .map(|(index, q)| UserInputQuestion {
                        note: None,
                        id: format!("q{index}"),
                        header: q.header.clone(),
                        question: q.question.clone(),
                        options: q.options.clone(),
                        multi_select: q.multi_select,
                        prefill: None,
                        multiline: q.options.is_empty(),
                    })
                    .collect();
                let answers = input(asked.clone());
                tokio::spawn(async move {
                    let answers = answers.await.unwrap_or_default();
                    let labels: Vec<Vec<String>> = asked
                        .iter()
                        .map(|q| {
                            answers
                                .iter()
                                .find(|a| a.question_id == q.id)
                                .map(|a| a.labels.clone())
                                .unwrap_or_default()
                        })
                        .collect();
                    client.respond(&id, protocol::questions_answer(&questions, &labels));
                });
            }
            Ask::Other => client.respond_error(&id, -32601, &format!("Method not found: {method}")),
        }
    }
}

/// The session the resume named is not there (any more).
fn gone(error: &HarnessError) -> bool {
    let text = error.to_string();
    text.contains(&format!("code {}", protocol::SESSION_UNAVAILABLE))
        || text.contains("not found")
        || text.contains("Session is not")
}
