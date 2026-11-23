//! vardoger-mcp — a Model Context Protocol server over the running engine.
//!
//! `vardoger mcp` speaks MCP (JSON-RPC 2.0, newline-delimited) on stdin/stdout
//! and proxies every tool into the engine's localhost IPC — the same
//! `vardoger_rpc` WebSocket the headed app and `vardoger sync` dial. Nothing here
//! talks to the edge or touches the filesystem: the engine stays the single
//! authority for chats, devices, projects, and the command plane.
//!
//! The server is hand-rolled rather than an SDK: the MCP surface a stdio
//! tool server needs is five methods (`initialize`, `ping`, `tools/list`,
//! `tools/call`, and the `initialized` notification), and the repo already
//! owns JSON-RPC framing for the Codex and ACP drivers. Zero new crates.
//!
//! When the engine launches a harness it can inject this server into the
//! agent's MCP config with `VARDOGER_CHAT_ID` / `VARDOGER_DEVICE_ID` in the
//! environment. Every send from such an agent is then attributed to its
//! originating chat, and a chat can never message itself.

pub mod http;
mod jsonrpc;
pub mod proxy;
mod tools;
mod transcript;
mod vardoger;

pub use jsonrpc::serve_stdio;
pub use tools::{ListedTool, ToolDef, Tools};
pub use transcript::{RenderOptions, RenderedMessage, render_entries};
pub use vardoger::{Origin, Vardoger};

/// How `vardoger mcp` finds the engine and who it speaks for.
#[derive(Debug, Clone)]
pub struct McpConfig {
    /// Loopback IPC port of the engine to proxy (`VARDOGER_IPC_PORT`, default 27754).
    pub ipc_port: u16,
    /// The engine's data dir, whose `ipc-token-agent` authenticates the IPC
    /// dial (`VARDOGER_DATA_DIR`; the `vardoger` binary fills in its default). The
    /// token itself is resolved at every dial — the `VARDOGER_IPC_TOKEN_FILE`
    /// the engine injects, else `{data_dir}/ipc-token-agent`, never the
    /// app's full token — so a long-lived server follows the engine across
    /// restarts and reaches only what its tools need.
    pub data_dir: Option<std::path::PathBuf>,
    /// The chat whose agent spawned this server, when injected by the engine.
    pub origin: Origin,
}

impl McpConfig {
    /// Resolve from the process environment: `VARDOGER_IPC_PORT` /
    /// `VARDOGER_DATA_DIR` for the engine, `VARDOGER_CHAT_ID` / `VARDOGER_DEVICE_ID`
    /// for the originating chat.
    pub fn from_env() -> Self {
        let ipc_port = std::env::var("VARDOGER_IPC_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(27754);
        Self {
            ipc_port,
            data_dir: std::env::var_os("VARDOGER_DATA_DIR")
                .filter(|dir| !dir.is_empty())
                .map(std::path::PathBuf::from),
            origin: Origin::from_env(),
        }
    }
}

/// Run the MCP server on this process's stdin/stdout until stdin closes.
pub async fn run(config: McpConfig) -> anyhow::Result<()> {
    let vardoger = Vardoger::new(config.ipc_port, config.data_dir, config.origin);
    let tools = Tools::new(std::sync::Arc::new(vardoger));
    serve_stdio(std::sync::Arc::new(tools)).await
}
