//! `commutator-engine` — the desktop app's background engine, as a process of its
//! own ("Commutator Engine" in Activity Monitor): the app starts it and attaches;
//! sessions keep running with the app closed. Without the UI linked in.
//!
//! It also answers what the engine runs its own executable for: `mcp-proxy`
//! (an agent's Commutator MCP tools on stdio, served by the engine), `mcp` (the
//! standalone Commutator MCP server) and `--noop-browser` (browser
//! suppression for agents' sign-in flows). And it is the engine helper of an
//! interface that doesn't link the engine (the zgx app): every `commutator`
//! subcommand that runs the engine's code (`headless`, `login`, `status`,
//! `plugin`, …) is handed here with the same arguments, and parsed by the
//! same command line (`commutator_cli`).

#![cfg_attr(windows, windows_subsystem = "windows")]

use commutator_cli::{Engines, Launch};

fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--noop-browser") => Ok(()),
        // An agent's Commutator MCP tools on stdio, forwarded to the engine's own
        // endpoint. First, before any engine setup: it must stay tiny.
        Some("mcp-proxy") => commutator_mcp::proxy::run(),
        Some("--version" | "-V") => {
            println!("commutator-engine {}", commutator_update::current_version());
            Ok(())
        }
        // Which build this binary is (`EngineInfo.build` of an engine it
        // starts): a client compares it with the one running.
        Some("--build-id") => {
            println!("{}", commutator_engine::build_id());
            Ok(())
        }
        // How the app starts it: the background engine.
        None => commutator_app::background::run(),
        Some(_) => match commutator_cli::run(commutator_update::current_version(), Engines::InProcess) {
            Launch::Done(result) => result,
            Launch::Engine(command) => commutator_app::run_engine_command(command),
            Launch::Window { .. } => {
                anyhow::bail!("commutator-engine has no window: open the Commutator app")
            }
            #[cfg(target_os = "linux")]
            Launch::Appshot => anyhow::bail!("commutator-engine has no window: open the Commutator app"),
        },
    }
}
