//! What `commutator` resolves from its environment.

/// The engine's IPC port: `COMMUTATOR_IPC_PORT`, else 27754.
pub fn ipc_port_from_env() -> u16 {
    ipc_port_from(std::env::var("COMMUTATOR_IPC_PORT").ok().as_deref())
}

/// [`ipc_port_from_env`] from the variable's value.
pub fn ipc_port_from(value: Option<&str>) -> u16 {
    value.and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_IPC_PORT)
}

/// The production engine's IPC port.
pub const DEFAULT_IPC_PORT: u16 = 27754;
