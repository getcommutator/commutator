//! vardoger — the command line with the engine linked in: `vardoger headless` runs
//! the engine alone, and the engine's other subcommands run here. It starts
//! local-only without credentials. `vardoger login` and `vardoger logout` select
//! the profile used by the next engine start without mutating a live runtime.
//!
//! The command line is `vardoger_cli`'s (shared with the engine helper and the
//! zgx interface). The desktop app is the zgx interface (apps/zgx), which
//! ships its own `vardoger`: this one opens no window.

use vardoger_cli::{Engines, Launch};

fn main() -> anyhow::Result<()> {
    // The engine runs its own executable as an agent's stdio MCP forwarder
    // (`vardoger headless`'s agents): first, before any setup, to stay tiny.
    if std::env::args().nth(1).as_deref() == Some("mcp-proxy") {
        return vardoger_mcp::proxy::run();
    }
    match vardoger_cli::run(vardoger_update::current_version(), Engines::InProcess) {
        Launch::Done(result) => result,
        Launch::Engine(command) => vardoger_app::run_engine_command(command),
        #[cfg(target_os = "linux")]
        Launch::Appshot => anyhow::bail!("Appshots are the desktop app's: open Vardoger and use its shortcut"),
        Launch::Window { .. } => anyhow::bail!(
            "this `vardoger` has no window: open the Vardoger app, or run `vardoger headless` for the engine alone"
        ),
    }
}
