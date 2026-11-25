//! The terminal emulator core shared by the desktop panel and the mobile
//! apps: [`emulator`] folds PTY bytes into a grid; [`predict`] paints typing
//! ahead of a remote host's echo once the host has proven it echoes.

#[cfg(any(target_os = "ios", target_os = "android"))]
pub mod emulator;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[path = "emulator_ghostty.rs"]
pub mod emulator;
pub mod predict;
