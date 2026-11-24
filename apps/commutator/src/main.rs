//! commutator — the command line with the engine linked in: `commutator headless` runs
//! the engine alone, and the engine's other subcommands run here. It starts
//! local-only without credentials. `commutator login` and `commutator logout` select
//! the profile used by the next engine start without mutating a live runtime.
//!
//! The command line is `commutator_cli`'s (shared with the engine helper). With
//! no subcommand it opens the desktop window (`commutator_ui`, Zeron 0.2's GPUI
//! interface), which connects to a running engine or embeds one.

use commutator_cli::{Engines, Launch};

fn main() -> anyhow::Result<()> {
    // The engine runs its own executable as an agent's stdio MCP forwarder
    // (`commutator headless`'s agents): first, before any setup, to stay tiny.
    if std::env::args().nth(1).as_deref() == Some("mcp-proxy") {
        return commutator_mcp::proxy::run();
    }
    match commutator_cli::run(commutator_update::current_version(), Engines::InProcess) {
        Launch::Done(result) => result,
        Launch::Engine(command) => commutator_app::run_engine_command(command),
        #[cfg(target_os = "linux")]
        Launch::Appshot => commutator_ui::appshots::request_running_appshot(
            &commutator_app::engine_config_from_env().data_dir,
        )
        .map_err(anyhow::Error::msg),
        Launch::Window { open_url, safe_mode: _ } => {
            let engine = commutator_app::engine_config_from_env();
            commutator_ui::run_app(commutator_ui::UiConfig {
                data_dir: engine.data_dir,
                ipc_port: engine.ipc_port,
                default_harness: engine.default_harness,
                initial_url: open_url,
            });
            Ok(())
        }
    }
}
