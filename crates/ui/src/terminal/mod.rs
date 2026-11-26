//! Terminal panel (feature-inventory §1.10): a Ghostty VT desktop emulator
//! (Alacritty remains the mobile backend) fed by the engine's PTY stream over
//! the generic RPC client.
//!
//! - [`emulator`] — terminal state machine (bytes in, grid out), with
//!   platform-specific implementations in `commutator-term`;
//! - [`view`] — cell palette, key/pointer/focus encodings, the ordered input
//!   queue, and the custom grid painter; [`sprite`] draws Ghostty-compatible
//!   box, Powerline, Braille, and legacy glyphs on the cell pixel grid;
//! - [`predict`] — local echo prediction for terminals on other devices
//!   (`commutator-term` too);
//! - [`panel`] — session-scoped tabs, subscriptions with reconnect backoff,
//!   drag-reorder, and the Cmd/Ctrl+J toggle action.
//!
//! Method names come from `commutator_rpc::methods` and wire types from
//! `commutator_proto` (`TerminalSession`, `TerminalEvent`) — the same contract the
//! engine serves (feature-inventory §2.1).

pub(crate) mod dock;
pub mod panel;
pub mod sprite;
pub mod view;
pub use commutator_term::{ghostty as emulator, predict};
