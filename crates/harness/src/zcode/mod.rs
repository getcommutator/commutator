//! Z.ai's ZCode agent, driven natively over its app-server protocol:
//! `zcode app-server --stdio` (the process the ZCode desktop app spawns),
//! newline-delimited JSON without the JSON-RPC version field. The wire lives
//! in [`protocol`]; the session in [`session`].
//!
//! - WHERE. `ZCODE_EXECUTABLE`, then the standalone CLI (`~/.local/bin/zcode`
//!   from the official installer, or `zcode` on PATH), then the CLI the
//!   desktop app bundles (`ZCode.app/Contents/Resources/glm/zcode.cjs`, run by
//!   the app's own Electron as Node). The desktop one is reached through a
//!   small launcher script Commutator writes (see [`desktop_launcher`]) so
//!   every caller — runs, version probes, sign-in, updates — sees one
//!   executable.
//! - VERSION. At least [`protocol::MIN_VERSION`], read from `zcode version`
//!   (the app-server has no handshake that names it).
//! - MODELS come from a throwaway session: `session/create` with deferred
//!   persistence in an empty temporary directory, its `settings.model`, then
//!   `session/close` — nothing is left in ZCode's history.
//! - ACCESS. Supervised runs in ZCode's `build` mode (it asks before risky
//!   or side-effecting tools), auto-accept edits in `edit`, full access in
//!   `yolo`, plan in `plan`. What it asks reaches the person as a yes/no;
//!   ZCode's "always allow" options are not offered.
mod protocol;
mod session;

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use serde_json::Value;
use tokio::sync::mpsc;

use commutator_proto::{
    AccessMode, AgentEvent, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode, ToolCall,
    UserInputQuestion,
};

use crate::process::{Command, Stdio, owned::Child};
use crate::{Harness, HarnessError, RunControls};

const INSTALL_HINT: &str = "ZCode CLI: install the ZCode desktop app (zcode.z.ai) or the CLI, \
                            or set ZCODE_EXECUTABLE";

pub struct ZcodeHarness {
    executable: Option<PathBuf>,
    models_cache: crate::catalog::Catalog,
    interrupt_grace: Duration,
    kill_grace: Duration,
}

impl Default for ZcodeHarness {
    fn default() -> Self {
        Self {
            executable: None,
            models_cache: Default::default(),
            interrupt_grace: Duration::from_secs(5),
            kill_grace: Duration::from_secs(3),
        }
    }
}

impl ZcodeHarness {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    pub fn resolve_executable(&self) -> Result<PathBuf, HarnessError> {
        if let Some(path) = self
            .executable
            .clone()
            .or_else(|| std::env::var_os("ZCODE_EXECUTABLE").map(PathBuf::from))
        {
            return crate::executable::validate_native_override(&path);
        }
        let home = crate::executable::home_or_current_dir();
        if let Some(cli) =
            crate::executable::find_on_paths("zcode", vec![home.join(".local/bin/zcode")])
        {
            return Ok(cli);
        }
        desktop_apps(&home)
            .into_iter()
            .find_map(|app| desktop_launcher(&app))
            .ok_or_else(|| HarnessError::NotInstalled(INSTALL_HINT.into()))
    }

    /// The executable, refused below [`protocol::MIN_VERSION`].
    fn checked_executable(&self) -> Result<PathBuf, HarnessError> {
        let exe = self.resolve_executable()?;
        if let Some(found) = cli_version(&exe)
            && found < protocol::MIN_VERSION
        {
            let (a, b, c) = found;
            let (x, y, z) = protocol::MIN_VERSION;
            return Err(HarnessError::NotInstalled(format!(
                "ZCode CLI {a}.{b}.{c} at {} is older than {x}.{y}.{z}, the oldest Commutator \
                 supports: update ZCode",
                exe.display()
            )));
        }
        Ok(exe)
    }

    async fn discover_models(&self) -> Result<Vec<Model>, HarnessError> {
        let exe = self.checked_executable()?;
        // An empty directory of its own: the throwaway session must not
        // touch (or be listed under) the user's project.
        let dir =
            std::env::temp_dir().join(format!("commutator-zcode-models-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir)?;
        let cwd = dir.to_string_lossy().into_owned();
        let env = crate::catalog_env::get(HarnessId::Zcode);
        let mut host = Host::spawn(&exe, &dir, &env)?;
        let grace = self.kill_grace;
        let result = tokio::time::timeout(Duration::from_secs(25), async {
            let create = protocol::create_params(&protocol::Create {
                cwd: &cwd,
                mcp: vec![],
                mode: "plan",
                model: None,
                thought_level: None,
                deferred: true,
            });
            let snapshot = host.call(protocol::CREATE, create).await?;
            if let Some(session) = protocol::snapshot_session(&snapshot) {
                let _ = host
                    .call(protocol::CLOSE, protocol::session_params(session))
                    .await;
            }
            Ok::<_, HarnessError>(snapshot)
        })
        .await
        .unwrap_or_else(|_| {
            Err(HarnessError::Protocol(
                "ZCode model discovery timed out".into(),
            ))
        });
        host.child.shutdown(grace).await;
        let _ = std::fs::remove_dir_all(&dir);
        let models = catalog(&result?);
        if models.is_empty() {
            return Err(HarnessError::Protocol(
                "ZCode offers no models: sign in with `zcode login` or configure a provider".into(),
            ));
        }
        Ok(models)
    }

    fn start(
        &self,
        request: RunRequest,
        controls: RunControls,
        read_only: bool,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let exe = self.checked_executable()?;
        let host = Host::spawn(&exe, Path::new(&request.cwd), &request.env)?;
        let (tx, rx) = mpsc::channel(256);
        let kill_grace = self.kill_grace;
        let interrupt_grace = self.interrupt_grace;
        tokio::spawn(async move {
            let Host {
                mut child,
                client,
                incoming,
                tail,
                ..
            } = host;
            let mut runner = session::Runner::new(client.clone(), incoming, tx, request, read_only);
            let RunControls {
                execution_lease: _lease,
                request_input,
                mut steering,
                interrupt,
                turn,
            } = controls;
            runner.input = Some(std::sync::Arc::from(request_input));
            runner.turn = turn;
            let consumer = runner.tx.clone();
            let result = tokio::select! {
                result = runner.run(&mut steering) => result,
                _ = interrupt.cancelled() => { runner.interrupt(interrupt_grace).await; Ok(()) },
                _ = consumer.closed() => Ok(()),
            };
            if let Err(error) = result {
                let status = tokio::time::timeout(Duration::from_millis(250), child.wait())
                    .await
                    .ok()
                    .and_then(Result::ok);
                runner.error = Some(if status.is_some() {
                    tail.wait_closed().await;
                    crate::crash_message("ZCode", status, &tail)
                } else {
                    error.to_string()
                });
            }
            client.close_stdin_for_teardown();
            child.shutdown(kill_grace).await;
            let _ = runner.finish().await;
        });
        Ok(
            futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) })
                .boxed(),
        )
    }
}

#[async_trait]
impl Harness for ZcodeHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Zcode
    }
    fn display_name(&self) -> &str {
        "ZCode"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    /// A prompt sent mid-turn is queued and runs as the next turn.
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    /// Levels are per model (`settings.thoughtLevel`).
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    fn access_modes(&self) -> &[AccessMode] {
        &[
            AccessMode::Supervised,
            AccessMode::AutoAcceptEdits,
            AccessMode::FullAccess,
        ]
    }
    fn supports_plan(&self) -> bool {
        true
    }
    fn installed(&self) -> bool {
        self.resolve_executable().is_ok()
    }
    fn executable_path(&self) -> Option<PathBuf> {
        self.resolve_executable().ok()
    }
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    fn stops_turn_in_place(&self) -> bool {
        true
    }
    /// Model, effort and mode move in place (`session/setModel`,
    /// `session/setMode`); the rest is read at launch.
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        launch_matches(live, next)
    }
    fn same_runtime(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.model == next.model
            && live.reasoning == next.reasoning
            && live.access == next.access
            && live.plan == next.plan
            && launch_matches(live, next)
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(self.model_catalog(false).await?.models)
    }
    fn model_context(&self) -> Result<Option<crate::ModelContext>, HarnessError> {
        let home = crate::executable::home_or_current_dir();
        // The login and provider configuration ZCode keeps (a sign-in, or a
        // provider added, is a different catalog).
        let config: Vec<PathBuf> = std::fs::read_dir(home.join(".zcode/v2"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        crate::model_context::context(HarnessId::Zcode, &self.resolve_executable()?, &config)
            .map(Some)
    }
    async fn model_catalog(&self, force: bool) -> Result<crate::ModelCatalog, HarnessError> {
        self.models_cache
            .get_with_timeout(
                force,
                Duration::from_secs(30),
                || Ok(self.model_context()?.expect("ZCode model context").key()),
                || self.discover_models(),
            )
            .await
    }
    async fn run_title(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.start(request, controls, true)
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.start(request, controls, false)
    }
}

fn launch_matches(live: &RunRequest, next: &RunRequest) -> bool {
    live.cwd == next.cwd
        && live.env == next.env
        && live.mcp == next.mcp
        && live.extra_mcp == next.extra_mcp
}

pub(super) fn full_access(request: &RunRequest) -> bool {
    request.auto_approve || request.access.is_full_access()
}

/// One `zcode app-server --stdio` process.
struct Host {
    child: Child,
    client: crate::jsonrpc::RpcClient,
    incoming: mpsc::Receiver<crate::jsonrpc::Incoming>,
    tail: crate::StderrTail,
    _stderr: tokio::task::JoinHandle<()>,
}

impl Host {
    fn spawn(
        exe: &Path,
        cwd: &Path,
        run_env: &std::collections::BTreeMap<String, String>,
    ) -> Result<Self, HarnessError> {
        let mut cmd = Command::new(exe);
        crate::process::owned::configure(&mut cmd);
        // The standalone CLI is a `node` script: the login shell's PATH
        // finds the user's Node when Commutator was started from Finder.
        crate::compose_child_path(&mut cmd, exe);
        crate::apply_run_env(&mut cmd, run_env);
        cmd.args(["app-server", "--stdio"])
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = Child::new(crate::process::spawn_agent(&mut cmd).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HarnessError::NotInstalled(crate::executable::binary_hint(exe))
            } else {
                HarnessError::Io(e)
            }
        })?);
        let tail = crate::StderrTail::default();
        let mut lines =
            crate::lossy_lines::LossyLines::new(child.stderr.take().expect("piped stderr"));
        let stderr = tail.clone();
        let stderr_task = tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                stderr.push(&line);
            }
            stderr.close();
        });
        let (client, incoming) = crate::jsonrpc::RpcClient::without_version_field(
            child.stdin.take().expect("piped stdin"),
            child.stdout.take().expect("piped stdout"),
        );
        Ok(Self {
            child,
            client,
            incoming,
            tail,
            _stderr: stderr_task,
        })
    }

    /// A call outside a run (model discovery): server requests answered as
    /// a run would, notifications dropped.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, HarnessError> {
        let pending = self.client.request_now(method, params);
        tokio::pin!(pending);
        loop {
            tokio::select! {
                result = &mut pending => return result,
                frame = self.incoming.recv() => match frame {
                    Some(crate::jsonrpc::Incoming::Request { id, method, .. }) => {
                        if method == "session/requestRuntimePreferences" {
                            self.client.respond(&id, protocol::runtime_preferences());
                        } else {
                            self.client.respond_error(&id, -32601, &format!("Method not found: {method}"));
                        }
                    }
                    Some(crate::jsonrpc::Incoming::Eof) | None => {
                        return Err(HarnessError::Protocol(format!("{method}: ZCode exited")));
                    }
                    Some(_) => {}
                },
            }
        }
    }
}

/// `settings.model.available` as Commutator models.
fn catalog(snapshot: &Value) -> Vec<Model> {
    protocol::offered_models(snapshot)
        .into_iter()
        .map(|model| Model {
            default: false,
            id: model.id,
            label: model.label,
            provider: None,
            description: None,
            reasoning_levels: model.levels.iter().filter_map(|l| reasoning(l)).collect(),
            options: vec![],
        })
        .collect()
}

fn reasoning(level: &str) -> Option<ReasoningLevel> {
    serde_json::from_value(Value::String(level.to_ascii_lowercase())).ok()
}

/// Commutator's reasoning level as ZCode's thought level name.
pub(super) fn thought_level(level: Option<ReasoningLevel>) -> Option<String> {
    serde_json::to_value(level?)
        .ok()?
        .as_str()
        .map(str::to_owned)
}

/// The prompt with its staged attachments named (ZCode reads files by path).
pub(super) fn with_attachments(prompt: &str, attachments: &[String]) -> String {
    // ponytail: paths only; inline images through v4/attachment/* when needed.
    let missing: Vec<&String> = attachments
        .iter()
        .filter(|path| !prompt.contains(path.as_str()))
        .collect();
    if missing.is_empty() {
        return prompt.to_owned();
    }
    let list: Vec<String> = missing.iter().map(|path| format!("- {path}")).collect();
    format!("{prompt}\n\nAttached files:\n{}", list.join("\n"))
}

pub(super) fn cap(text: &str) -> String {
    commutator_proto::command_output_cap(text).unwrap_or_default()
}

/// A failed turn's message, with what to do when it's the missing login.
pub(super) fn explain_failure(message: &str) -> String {
    if message.contains("Select a model") {
        return format!(
            "{message}: ZCode has no model to run. Sign in with `zcode login` (or add a provider \
             in ZCode), then pick a model."
        );
    }
    message.to_owned()
}

pub(super) fn permission_question(permission: &protocol::Permission) -> UserInputQuestion {
    let input = &permission.input;
    let text = |keys: &[&str]| keys.iter().find_map(|key| input[*key].as_str());
    let name = permission.tool.to_ascii_lowercase();
    if let Some(command) =
        text(&["command", "cmd"]).filter(|_| name.contains("bash") || name.contains("shell"))
    {
        return crate::approval::command("ZCode", command);
    }
    if let Some(path) = text(&["file_path", "path", "filePath"]) {
        return crate::approval::edit("ZCode", &[path]);
    }
    crate::approval::tool("ZCode", &permission.tool, Some(&permission.reason))
}

/// A ZCode tool call as Commutator renders it.
pub(super) fn tool_call(name: &str, input: &Value) -> ToolCall {
    let text = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| input[*key].as_str())
            .map(str::to_owned)
    };
    let path = || text(&["file_path", "path", "filePath"]).unwrap_or_default();
    match name.to_ascii_lowercase().as_str() {
        "bash" | "shell" => ToolCall::Exec {
            command: text(&["command", "cmd"]).unwrap_or_default(),
        },
        "read" | "read_file" => ToolCall::ReadFile { path: path() },
        "write" | "write_file" => ToolCall::WriteFile {
            path: path(),
            content: text(&["content"]),
        },
        "edit" | "multiedit" | "str_replace" => ToolCall::EditFile {
            path: path(),
            old_string: text(&["old_string", "oldString"]),
            new_string: text(&["new_string", "newString"]),
        },
        "apply_patch" | "applypatch" => ToolCall::ApplyPatch {
            path: text(&["path", "file_path"]),
        },
        "grep" => ToolCall::Search {
            pattern: text(&["pattern", "query"]).unwrap_or_default(),
            path: text(&["path"]),
        },
        "glob" => ToolCall::Glob {
            pattern: text(&["pattern"]).unwrap_or_default(),
        },
        "webfetch" | "web_fetch" => ToolCall::WebFetch {
            url: text(&["url"]).unwrap_or_default(),
            prompt: text(&["prompt"]),
        },
        "websearch" | "web_search" => ToolCall::WebSearch {
            query: text(&["query"]).unwrap_or_default(),
        },
        _ => match name.split_once("__") {
            Some((server, tool)) if !server.is_empty() && !tool.is_empty() => ToolCall::Mcp {
                server: server
                    .trim_start_matches("mcp")
                    .trim_start_matches('_')
                    .into(),
                tool: tool.into(),
                input: Some(input.clone()),
            },
            _ => ToolCall::Unknown {
                name: name.into(),
                input: (!input.is_null()).then(|| input.clone()),
            },
        },
    }
}

/// `zcode version` (the CLI's own version; cached per executable identity
/// by the shared probe would read `--version`, which the standalone
/// distribution answers with its package version).
fn cli_version(exe: &Path) -> Option<(u64, u64, u64)> {
    use std::sync::Mutex;
    static CACHE: Mutex<Vec<((PathBuf, Option<std::time::SystemTime>), (u64, u64, u64))>> =
        Mutex::new(Vec::new());
    let key = (
        exe.canonicalize().ok()?,
        exe.metadata().ok()?.modified().ok(),
    );
    if let Some((_, version)) = CACHE.lock().ok()?.iter().find(|(k, _)| *k == key) {
        return Some(*version);
    }
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("version").stdin(std::process::Stdio::null());
    let output = cmd.output().ok()?;
    let version = protocol::parse_version(&String::from_utf8_lossy(&output.stdout))?;
    CACHE.lock().ok()?.push((key, version));
    Some(version)
}

/// Where the desktop app may be installed.
fn desktop_apps(home: &Path) -> Vec<PathBuf> {
    if !cfg!(target_os = "macos") {
        // ponytail: macOS bundle layout only; Windows/Linux desktop installs not mapped yet.
        return vec![];
    }
    vec![
        PathBuf::from("/Applications/ZCode.app"),
        home.join("Applications/ZCode.app"),
    ]
}

/// A launcher for the CLI the desktop app bundles: the app's Electron run
/// as Node on `Contents/Resources/glm/zcode.cjs`, as the desktop's own host
/// spawns it (`zcodeAgentProcessManager.ts`, packaged branch). The one
/// variable set beside `ELECTRON_RUN_AS_NODE` is
/// `ZCODE_BUILTIN_PROVIDER_CONFIG_FILE`: the CLI looks for its bundled
/// provider config next to `zcode.cjs`, but the app keeps it in
/// `Resources/config/provider/` and passes it in (`desktopProviderConfig.ts`,
/// `services/src/node.ts`); given only this one, the CLI derives the rest
/// (`provider-runtime-env.ts`) exactly as the standalone CLI does.
/// Written under Commutator's adapters directory and rewritten when stale.
fn desktop_launcher(app: &Path) -> Option<PathBuf> {
    let electron = app.join("Contents/MacOS/ZCode");
    let bundle = app.join("Contents/Resources/glm/zcode.cjs");
    let config = app.join("Contents/Resources/config/provider/zcode-builtin.json");
    if !electron.is_file() || !bundle.is_file() {
        return None;
    }
    let script = launcher_script(
        &electron,
        &bundle,
        config.is_file().then_some(config.as_path()),
    );
    let dir = crate::adapter_install::adapters_root()?.join("zcode-desktop");
    let path = dir.join("zcode");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(script.as_str()) {
        std::fs::create_dir_all(&dir).ok()?;
        let staged = dir.join(format!(".zcode.{}", std::process::id()));
        std::fs::write(&staged, &script).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).ok()?;
        }
        std::fs::rename(&staged, &path).ok()?;
    }
    Some(path)
}

fn launcher_script(electron: &Path, bundle: &Path, config: Option<&Path>) -> String {
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"));
    let config = config
        .map(|config| format!("ZCODE_BUILTIN_PROVIDER_CONFIG_FILE={} ", quote(config)))
        .unwrap_or_default();
    format!(
        "#!/bin/sh\n# Written by Commutator: the CLI the ZCode desktop app bundles.\n\
         ELECTRON_RUN_AS_NODE=1 {config}exec {} {} \"$@\"\n",
        quote(electron),
        quote(bundle)
    )
}

#[cfg(test)]
mod tests;
