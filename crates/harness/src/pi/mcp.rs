//! Load the existing Vardoger delegation tools into this Pi process only.
use crate::{HarnessError, process::Command, scratch::ScratchDir};

/// The bridge extension reads its server from this variable.
const CONFIG_ENV: &str = "VARDOGER_PI_MCP";

pub(super) fn configure(
    cmd: &mut Command,
    config: &vardoger_proto::McpServer,
) -> Result<ScratchDir, HarnessError> {
    let scratch = ScratchDir::new("pi-mcp")?;
    let extension = scratch.path().join("vardoger-mcp.mjs");
    std::fs::write(&extension, include_str!("mcp.mjs"))?;
    cmd.arg("--extension").arg(extension).env(
        CONFIG_ENV,
        serde_json::to_string(config).expect("serializable MCP config"),
    );
    Ok(scratch)
}

/// [`configure`] for Vardoger's own server plus the plugins' native servers
/// ([`vardoger_proto::RunRequest::extra_mcp`]). The bridge drives one server
/// per extension, so each extra gets its own copy reading its own variable
/// (`VARDOGER_PI_MCP_1`, …). `None` when there is no server at all.
pub(super) fn configure_all(
    cmd: &mut Command,
    mcp: Option<&vardoger_proto::McpServer>,
    extras: &[vardoger_proto::McpServer],
) -> Result<Option<ScratchDir>, HarnessError> {
    let mut servers = mcp.into_iter().chain(extras);
    let Some(first) = servers.next() else {
        return Ok(None);
    };
    let scratch = configure(cmd, first)?;
    for (index, config) in servers.enumerate().map(|(i, c)| (i + 1, c)) {
        let var = format!("{CONFIG_ENV}_{index}");
        let source = include_str!("mcp.mjs").replace(
            &format!("process.env.{CONFIG_ENV}"),
            &format!("process.env.{var}"),
        );
        let extension = scratch.path().join(format!("vardoger-mcp-{index}.mjs"));
        std::fs::write(&extension, source)?;
        cmd.arg("--extension").arg(extension).env(
            var,
            serde_json::to_string(config).expect("serializable MCP config"),
        );
    }
    Ok(Some(scratch))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn configure_all_loads_one_bridge_per_plugin_server() {
        assert!(
            include_str!("mcp.mjs").contains("process.env.VARDOGER_PI_MCP)"),
            "the bridge reads its config from VARDOGER_PI_MCP"
        );
        let vardoger = vardoger_proto::McpServer {
            name: "vardoger".into(),
            command: "/opt/vardoger/vardoger".into(),
            args: vec!["mcp".into()],
            env: Default::default(),
            http: None,
        };
        let extra = vardoger_proto::McpServer {
            name: "plugin-focus".into(),
            command: "/data/plugins/.loaded/focus/0123/bin/server".into(),
            args: vec![],
            env: Default::default(),
            http: None,
        };
        let mut cmd = Command::new("pi");
        let scratch = configure_all(&mut cmd, Some(&vardoger), std::slice::from_ref(&extra))
            .unwrap()
            .expect("two servers");
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.iter().filter(|a| *a == "--extension").count(), 2);
        let envs: std::collections::HashMap<String, String> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        let first: vardoger_proto::McpServer = serde_json::from_str(&envs["VARDOGER_PI_MCP"]).unwrap();
        assert_eq!(first, vardoger);
        let second: vardoger_proto::McpServer = serde_json::from_str(&envs["VARDOGER_PI_MCP_1"]).unwrap();
        assert_eq!(second, extra);
        let copy = std::fs::read_to_string(scratch.path().join("vardoger-mcp-1.mjs")).unwrap();
        assert!(copy.contains("process.env.VARDOGER_PI_MCP_1)"));
        assert!(!copy.contains("process.env.VARDOGER_PI_MCP)"));

        let mut bare = Command::new("pi");
        assert!(configure_all(&mut bare, None, &[]).unwrap().is_none());
        assert_eq!(bare.as_std().get_args().count(), 0);
    }
}
