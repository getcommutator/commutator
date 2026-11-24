//! `commutator login` / `commutator logout` / `commutator status`: this machine's
//! sign-in to a Commutator server ([`commutator_engine::account`]), which lets
//! the account's other devices reach it. The engine reads it when it starts.

use std::io::IsTerminal;

use commutator_engine::account::{Account, Callback, SignIn, server_url};
use commutator_engine::{EngineConfig, InstanceLock};

/// `commutator login [SERVER] [--paste]`.
pub async fn login(
    config: EngineConfig,
    server: Option<String>,
    paste: bool,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(&config.data_dir)?;
    let saved = Account::load(&config.data_dir);
    let server = server_url(server, saved.as_ref());
    let device_id = commutator_engine::device_id(&config.data_dir)?;
    let name = commutator_engine::device_name(&device_id);
    // Over SSH the browser is on another machine: its callback can't reach here.
    let paste = paste || std::env::var_os("SSH_CONNECTION").is_some();
    let callback = if paste {
        Callback::Paste
    } else {
        Callback::Loopback
    };
    let sign_in = SignIn::start(&server, &device_id, &name, callback).await?;
    println!(
        "Sign in to Commutator as \"{name}\":\n\n  {}\n",
        sign_in.url()
    );
    let account = if paste {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!("commutator login --paste needs an interactive terminal");
        }
        println!("Then paste the code the page shows here and press enter.");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        sign_in.redeem(&line).await?
    } else {
        open_browser(sign_in.url());
        println!("Waiting for the browser…");
        sign_in.wait().await?
    };
    account.save(&config.data_dir)?;
    println!(
        "\nSigned in as {} on {}.",
        account.email, account.server_url
    );
    if InstanceLock::holder(&config.data_dir).is_some() {
        println!("Restart Commutator so your other devices can reach this machine.");
    }
    Ok(())
}

fn open_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// `commutator logout`: remove this machine on the server and forget it here.
pub async fn logout(config: EngineConfig) -> anyhow::Result<()> {
    let Some(account) = Account::load(&config.data_dir) else {
        Account::forget(&config.data_dir)?;
        println!("Not signed in.");
        return Ok(());
    };
    match account.sign_out(&config.data_dir).await? {
        Ok(()) => println!("Signed out {} from {}.", account.email, account.server_url),
        Err(err) => println!(
            "Signed out here, but {} wasn't told ({err}); remove this machine from your devices there.",
            account.server_url
        ),
    }
    if InstanceLock::holder(&config.data_dir).is_some() {
        println!("Restart Commutator to close its connection to the server.");
    }
    Ok(())
}

/// `commutator status`: the sign-in and whether the engine runs.
pub async fn status(config: EngineConfig) -> anyhow::Result<()> {
    println!("Data dir: {}", config.data_dir.display());
    match Account::load(&config.data_dir) {
        Some(account) => println!(
            "Account:  {} on {} (device {})",
            account.email, account.server_url, account.device_id
        ),
        None => println!(
            "Account:  not signed in (this machine only; `commutator login` to reach it from your other devices)"
        ),
    }
    match InstanceLock::holder(&config.data_dir) {
        Some(pid) => println!("Engine:   running (pid {pid})"),
        None => println!("Engine:   not running"),
    }
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], config.ipc_port));
    let ipc = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500));
    println!(
        "IPC:      {} 127.0.0.1:{}",
        if ipc.is_ok() {
            "listening on"
        } else {
            "not listening on"
        },
        config.ipc_port
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_named_server_wins_then_the_saved_one() {
        let saved = Account {
            server_url: "https://saved.example".into(),
            token: "t".into(),
            device_id: "d".into(),
            email: "me@example.com".into(),
            name: None,
            picture: None,
        };
        assert_eq!(
            server_url(Some("https://named.example/".into()), Some(&saved)),
            "https://named.example"
        );
        if std::env::var_os("COMMUTATOR_SERVER_URL").is_none() {
            assert_eq!(server_url(None, Some(&saved)), "https://saved.example");
            assert_eq!(server_url(None, None), commutator_engine::account::DEFAULT_SERVER_URL);
        }
    }
}
