use anyhow::Context;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let env = |key: &str| std::env::var(key).with_context(|| format!("{key} is not set"));
    let config = commutator_server::Config {
        public_url: env("COMMUTATOR_SERVER_URL")?.trim_end_matches('/').to_owned(),
        google_client_id: env("GOOGLE_CLIENT_ID")?,
        google_client_secret: env("GOOGLE_CLIENT_SECRET")?,
        allowed_emails: env("COMMUTATOR_ALLOWED_EMAILS")?
            .split(',')
            .map(|email| email.trim().to_lowercase())
            .filter(|email| !email.is_empty())
            .collect(),
        ice_servers: ice_servers(),
    };
    anyhow::ensure!(
        !config.allowed_emails.is_empty(),
        "COMMUTATOR_ALLOWED_EMAILS lists no one"
    );
    let db = std::env::var("COMMUTATOR_SERVER_DB").unwrap_or_else(|_| "commutator-server.db".into());
    let addr = std::env::var("COMMUTATOR_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let app = commutator_server::App::open(config, std::path::Path::new(&db))?;
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, commutator_server::router(app)).await?;
    Ok(())
}

/// `COMMUTATOR_ICE_URLS` (comma-separated `stun:`/`turn:`/`turns:` URLs), the
/// TURN ones signed in with `COMMUTATOR_TURN_USERNAME`/`COMMUTATOR_TURN_CREDENTIAL`.
/// Unset: devices keep their default public STUN.
fn ice_servers() -> Vec<commutator_server::IceServer> {
    let urls = std::env::var("COMMUTATOR_ICE_URLS").unwrap_or_default();
    let (turn, stun): (Vec<String>, Vec<String>) = urls
        .split(',')
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
        .partition(|url| url.starts_with("turn"));
    let mut servers = Vec::new();
    if !stun.is_empty() {
        servers.push(commutator_server::IceServer {
            urls: stun,
            username: None,
            credential: None,
        });
    }
    if !turn.is_empty() {
        servers.push(commutator_server::IceServer {
            urls: turn,
            username: std::env::var("COMMUTATOR_TURN_USERNAME").ok(),
            credential: std::env::var("COMMUTATOR_TURN_CREDENTIAL").ok(),
        });
    }
    servers
}
