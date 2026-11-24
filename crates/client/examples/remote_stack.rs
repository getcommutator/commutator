//! A local Commutator server and an engine signed in to it, for driving the
//! phone apps against (the same setup as tests/remote.rs, kept running).
//! Google can't sign in a server on localhost, so the server mints the
//! phone's device token directly here and prints the iOS launch arguments
//! that use it (`-account … -host …`, DEBUG builds only).
//!
//!   cargo run -p commutator-client --example remote_stack -- <dir> [port]
//!
//! `<dir>` keeps the server's database and the engine's data across runs.
//! The engine's default agent is the mock harness: no real agent runs.

use std::path::PathBuf;

use commutator_engine::account::Account;
use commutator_engine::{Engine, EngineConfig, HarnessId};
use commutator_server::{App, Config, Kind};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: remote_stack <dir> [port]"));
    let port: u16 = args.next().map(|p| p.parse()).transpose()?.unwrap_or(8787);
    let (server_dir, engine_dir) = (dir.join("server"), dir.join("engine"));
    std::fs::create_dir_all(&server_dir)?;
    std::fs::create_dir_all(&engine_dir)?;

    let server_url = format!("http://127.0.0.1:{port}");
    let app = App::open(
        Config {
            public_url: server_url.clone(),
            google_client_id: "unused".into(),
            google_client_secret: "unused".into(),
            allowed_emails: vec!["dev@localhost".into()],
            ice_servers: Vec::new(),
        },
        &server_dir.join("db"),
    )?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    tokio::spawn(axum::serve(listener, commutator_server::router(app.clone())).into_future());

    let me = app.upsert_user("dev-sub", "dev@localhost", Some("Local dev"))?;
    let machine = commutator_engine::device_id(&engine_dir)?;
    let (_, machine_token) =
        app.create_device(&me, "dev-machine", Kind::Machine, Some(&machine))?;
    let (phone, phone_token) = app.create_device(&me, "Simulator", Kind::Client, None)?;
    Account {
        server_url: server_url.clone(),
        token: machine_token,
        device_id: machine.clone(),
        email: "dev@localhost".into(),
        name: Some("Local dev".into()),
    }
    .save(&engine_dir)?;

    let ipc = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    println!("server  {server_url}");
    println!("machine {machine}");
    println!("ios launch args: -account {server_url} {phone_token} {phone} -host {machine} -harness mock");
    Engine::new(EngineConfig {
        data_dir: engine_dir,
        edge_url: "http://127.0.0.1:1".into(),
        ipc_port: ipc,
        default_harness: HarnessId::Mock,
    })
    .run()
    .await?;
    Ok(())
}
