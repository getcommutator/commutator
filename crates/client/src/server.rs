//! The account's Commutator server (`apps/commutator-server`): which machines the
//! account has, before a [`crate::Credentials::Remote`] client picks one.

use std::time::Duration;

use serde::Deserialize;

use crate::error::{ClientError, Result};

/// One of the account's devices, as `GET /devices` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDevice {
    pub id: String,
    pub name: String,
    /// `machine` (runs an engine; can be a host) or `client` (a phone, an app).
    pub kind: String,
    /// A machine whose engine holds its device room right now.
    pub online: bool,
    /// Epoch seconds.
    pub last_seen: i64,
    /// The device asking (this one).
    pub current: bool,
}

impl ServerDevice {
    pub fn is_machine(&self) -> bool {
        self.kind == "machine"
    }
}

/// The account's devices, oldest first. `token` is this device's bearer from
/// its sign-in (`POST /auth/redeem`).
pub async fn devices(server_url: &str, token: &str) -> Result<Vec<ServerDevice>> {
    let url = format!("{}/devices", server_url.trim_end_matches('/'));
    let token = token.to_owned();
    crate::runtime::run(async move {
        let response = reqwest::Client::new()
            .get(url)
            .bearer_auth(token)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ClientError::Auth("the server no longer knows this device".into()));
        }
        if !status.is_success() {
            return Err(ClientError::Network(format!("GET /devices: {status}")));
        }
        response
            .json()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))
    })
    .await
}
