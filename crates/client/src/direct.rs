//! Direct connections from this device to the account's machines
//! ([`Credentials::Remote`] only): the same WebRTC peers every engine runs
//! (`commutator-preview`), with this device a pure viewer, paired through the
//! server's signaling (`GET /preview/ws`). Terminals ride them when they can
//! ([`crate::terminal`]); previews open only over them ([`Client::open_preview`]).
//! A Local client needs none: its engine pairs for it.

use std::sync::Arc;

use serde::Deserialize;
use commutator_preview::viewer::{Forward, ViewerPeers};
use commutator_proto::PreviewService;

use crate::client::{Client, ClientInner};
use crate::config::Credentials;
use crate::error::{ClientError, Result};
use crate::lock;

struct Token(String);

#[async_trait::async_trait]
impl commutator_preview::signaling::TokenSource for Token {
    async fn token(&self) -> anyhow::Result<String> {
        Ok(self.0.clone())
    }
}

/// This device's direct peers, started on first use. `None` in Demo and
/// Local mode; an error when the server can't say who this device is.
pub(crate) async fn peers(inner: &Arc<ClientInner>) -> Result<Option<ViewerPeers>> {
    if let Some(peers) = lock(&inner.direct).clone() {
        return Ok(Some(peers));
    }
    let Credentials::Remote {
        server_url, token, ..
    } = &inner.credentials
    else {
        return Ok(None);
    };
    let server_url = server_url.trim_end_matches('/').to_owned();
    // Signaling admits a device only as the id its token belongs to.
    let device_id = me(&server_url, token).await?;
    let peers = ViewerPeers::start(
        &crate::runtime::handle(),
        device_id,
        inner.config.device_name.clone(),
        inner.config.data_dir.join("direct-peers.json"),
        commutator_preview::signaling::Config {
            server_url,
            tokens: Arc::new(Token(token.clone())),
        },
    )
    .map_err(|e| ClientError::Storage(e.to_string()))?;
    let mut slot = lock(&inner.direct);
    if let Some(raced) = slot.as_ref() {
        peers.stop();
        return Ok(Some(raced.clone()));
    }
    *slot = Some(peers.clone());
    drop(slot);
    let (cancel, stopping) = (inner.cancel.clone(), peers.clone());
    crate::runtime::shared().spawn(async move {
        cancel.cancelled().await;
        stopping.stop();
    });
    Ok(Some(peers))
}

/// The device id the server knows this token as (`GET /me`).
async fn me(server_url: &str, token: &str) -> Result<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Me {
        device_id: String,
    }
    let response = reqwest::Client::new()
        .get(format!("{server_url}/me"))
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| ClientError::Network(e.to_string()))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(ClientError::Auth(
            "the server no longer knows this device".into(),
        ));
    }
    let me: Me = response
        .error_for_status()
        .map_err(|e| ClientError::Network(e.to_string()))?
        .json()
        .await
        .map_err(|e| ClientError::Network(e.to_string()))?;
    Ok(me.device_id)
}

/// A preview of another device's dev server, served on this device's
/// loopback while it's held ([`Client::open_preview`]).
pub struct PreviewTunnel(Forward);

impl PreviewTunnel {
    /// What a web view loads: `http://127.0.0.1:{port}/`.
    pub fn url(&self) -> String {
        self.0.url()
    }
}

impl Client {
    async fn direct_peers(&self) -> Result<ViewerPeers> {
        let inner = self.inner.clone();
        crate::runtime::run(async move { peers(&inner).await })
            .await?
            .ok_or_else(|| {
                ClientError::Unsupported("previews of other devices need a Remote client".into())
            })
    }

    /// The account's machines' preview services (dev servers their chats
    /// started), as signaling last listed them. The first call joins
    /// signaling, so it may still be empty; [`Self::previews_changed`]
    /// resolves when the list changes.
    pub async fn preview_services(&self) -> Result<Vec<PreviewService>> {
        Ok(self.direct_peers().await?.services())
    }

    /// Resolves at the next change of [`Self::preview_services`].
    pub async fn previews_changed(&self) -> Result<()> {
        let mut changes = self.direct_peers().await?.watch_services();
        crate::runtime::run(async move { changes.changed().await.map_err(|_| ClientError::Closed) })
            .await
    }

    /// Open `service` (a [`PreviewService::id`]) of machine `device_id`:
    /// pairs with it directly and serves the preview on this device's
    /// loopback until the tunnel is dropped. Fails when the devices can't
    /// connect directly (no relay fallback for previews).
    pub async fn open_preview(&self, device_id: &str, service: &str) -> Result<PreviewTunnel> {
        let peers = self.direct_peers().await?;
        let (device_id, service) = (device_id.to_owned(), service.to_owned());
        crate::runtime::run(async move {
            // Pair now, so a failure shows here rather than as a blank page.
            peers
                .open(&device_id, &service)
                .await
                .map_err(|e| ClientError::HostUnavailable(e.to_string()))?;
            peers
                .forward(&device_id, &service)
                .await
                .map(PreviewTunnel)
                .map_err(|e| ClientError::Internal(e.to_string()))
        })
        .await
    }
}
