//! The edge and sign-in client `vardoger` resolves from its environment.

/// Production edge (Cloudflare Worker + Durable Objects on the zeron.sh zone).
/// `VARDOGER_EDGE_URL` overrides (local dev / self-hosting).
pub const DEFAULT_EDGE_URL: &str = "https://edge.zeron.sh";

pub fn edge_url_from_env() -> String {
    std::env::var("VARDOGER_EDGE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_EDGE_URL.into())
}

/// The engine's IPC port: `VARDOGER_IPC_PORT`, else 27754.
pub fn ipc_port_from_env() -> u16 {
    ipc_port_from(std::env::var("VARDOGER_IPC_PORT").ok().as_deref())
}

/// [`ipc_port_from_env`] from the variable's value.
pub fn ipc_port_from(value: Option<&str>) -> u16 {
    value.and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_IPC_PORT)
}

/// The production engine's IPC port.
pub const DEFAULT_IPC_PORT: u16 = 27754;
