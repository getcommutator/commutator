//! vardoger — the command line with the engine linked in: `vardoger headless` runs
//! the engine alone, and the engine's other subcommands run here. It starts
//! local-only without credentials. `vardoger login` and `vardoger logout` select
//! the profile used by the next engine start without mutating a live runtime.
//!
//! The command line is `vardoger_cli`'s (shared with the engine helper). With
//! no subcommand it opens the desktop window (`vardoger_ui`, Zeron 0.2's GPUI
//! interface), which connects to a running engine or embeds one.

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
        Launch::Appshot => vardoger_ui::appshots::request_running_appshot(
            &vardoger_app::engine_config_from_env().data_dir,
        )
        .map_err(anyhow::Error::msg),
        Launch::Window { open_url, safe_mode: _ } => {
            let engine = vardoger_app::engine_config_from_env();
            vardoger_ui::run_app(vardoger_ui::UiConfig {
                data_dir: engine.data_dir,
                ipc_port: engine.ipc_port,
                edge_url: engine.edge_url,
                default_harness: engine.default_harness,
                initial_url: open_url,
            });
            Ok(())
        }
    }
}
