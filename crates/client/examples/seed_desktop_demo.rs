//! Seed a data dir with the demo workspace for desktop screenshots:
//!
//!   cargo run -p commutator-client --example seed_desktop_demo -- /tmp/cdemo
//!   COMMUTATOR_DATA_DIR=/tmp/cdemo COMMUTATOR_IPC_PORT=27799 \
//!     COMMUTATOR_WORKTREES_DIR=/tmp/cdemo/wt ./target/release/commutator
//!
//! The window opens past first-run setup, without the star banner, in Nord.

fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).expect("usage: seed_desktop_demo <data dir>");
    let dir = std::path::Path::new(&dir);
    commutator_client::write_desktop_demo(dir)?;
    std::fs::write(
        dir.join("ui-settings.json"),
        r#"{"onboardingCompleted":true,"githubStarBannerDismissed":true,"appearance":"dark",
"themeSelection":{"light":"github-light","dark":"nord"},"sidebarView":"projects"}"#,
    )?;
    Ok(())
}
