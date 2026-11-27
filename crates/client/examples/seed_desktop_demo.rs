//! Seed a data dir with the demo workspace for desktop screenshots:
//!
//!   cargo run -p commutator-client --example seed_desktop_demo -- /tmp/cdemo
//!   COMMUTATOR_DATA_DIR=/tmp/cdemo COMMUTATOR_IPC_PORT=27799 \
//!     COMMUTATOR_WORKTREES_DIR=/tmp/cdemo/wt ./target/release/commutator

fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).expect("usage: seed_desktop_demo <data dir>");
    commutator_client::write_desktop_demo(std::path::Path::new(&dir))
}
