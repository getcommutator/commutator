//! Commutator mobile core — the UniFFI surface shared by the iOS and Android apps.
//!
//! - [`client_ffi`]: account, workspace and session state (wraps `commutator-client`).
//! - [`layout`]: analytic transcript layout — markdown → measured display lists
//!   (wraps `commutator-markdown` + `commutator-text`).

uniffi::setup_scaffolding!("commutator_core");

mod client_ffi;
pub mod layout;
pub mod terminal;
pub mod wallpaper;
pub mod visual;

/// Version handshake: the Swift/Kotlin bindings must match the linked library.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}
