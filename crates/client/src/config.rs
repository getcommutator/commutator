//! Construction inputs: who this device is and which engine it drives.
//! Token *persistence* stays on the platform (Keychain / Keystore).

use std::path::PathBuf;

/// Static per-install configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    /// Writable directory for registry/doc snapshots and caches. Scoped by the
    /// platform to the signed-in identity (sign-out wipes it).
    pub data_dir: PathBuf,
    /// This device's stable id (`ios-xxxxxxxx`) — stamped on every command
    /// and queue row this device writes.
    pub device_id: String,
    /// Human name for presence/attribution ("Wing's iPhone").
    pub device_name: String,
    /// `ios` / `android` — viewer platforms never host sessions.
    pub platform: String,
    /// App version string (diagnostics only).
    pub app_version: String,
}

impl ClientConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            device_id: format!("ios-{}", &crate::new_id()[..8]),
            device_name: "iPhone".into(),
            platform: "ios".into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

/// Which engine this client drives, and how it gets there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credentials {
    /// Fully offline, deterministic dataset with a simulated host.
    Demo(DemoOptions),
    /// The engine on this machine over its IPC (no sign-in):
    /// `port` it listens on (27754 by default) and its `data_dir`, where it
    /// writes the `ipc-token` a dial presents.
    Local { port: u16, engine_data_dir: PathBuf },
    /// An engine on another of the account's machines, through the relay on
    /// the account's Commutator server: its URL, this device's bearer `token`
    /// from its sign-in there, and the machine to drive (`host_device_id`,
    /// a machine from [`crate::server::devices`]).
    Remote {
        server_url: String,
        token: String,
        host_device_id: String,
    },
}

impl Credentials {
    pub fn is_demo(&self) -> bool {
        matches!(self, Credentials::Demo(_))
    }

    pub fn org_id(&self) -> &str {
        match self {
            Credentials::Demo(_) => "demo",
            Credentials::Local { .. } => "local",
            Credentials::Remote { .. } => "remote",
        }
    }

    pub fn user_id(&self) -> &str {
        match self {
            Credentials::Demo(_) => "demo",
            Credentials::Local { .. } => "local",
            Credentials::Remote { .. } => "remote",
        }
    }
}

/// Demo-mode knobs (the legacy app's `-demo` launch args).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoOptions {
    pub fixture: DemoFixture,
    /// Size of the flagship chat's transcript (`chat-veil`).
    pub transcript_scale: TranscriptScale,
    pub stream_speed: StreamSpeed,
    /// Repeat the scripted reply 12× (long streaming stress, `-longreply`).
    pub long_reply: bool,
}

impl Default for DemoOptions {
    fn default() -> Self {
        Self {
            fixture: DemoFixture::Standard,
            transcript_scale: TranscriptScale::Normal,
            stream_speed: StreamSpeed::Realistic,
            long_reply: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoFixture {
    /// Devices, projects, pinned sessions, PRs in every state.
    Standard,
    /// No projects and no sessions (`-no-projects`): empty-state screens.
    NoProjects,
    /// Only this phone — no execution hosts (`-ios-only`).
    IosOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptScale {
    /// Hand-written fixtures.
    Normal,
    /// 120 synthetic turns (`-big`).
    Big,
    /// 600 synthetic turns (`-huge`).
    Huge,
    /// Arbitrary synthetic size (benchmarks).
    Turns(u32),
}

impl TranscriptScale {
    pub fn turns(self) -> Option<u32> {
        match self {
            TranscriptScale::Normal => None,
            TranscriptScale::Big => Some(120),
            TranscriptScale::Huge => Some(600),
            TranscriptScale::Turns(n) => Some(n),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamSpeed {
    /// Word by word at 30–140ms — what a real harness feels like.
    Realistic,
    /// A word every ~4ms: stresses the per-update path (benchmarks).
    Fast,
}
