//! Edge bearer: the per-client bearer provider and the shared HTTP client.
//!
//! Live modes:
//! - Dev (`AUTH_MODE=dev` edge): the bearer IS `userId@orgId`.
//! - Demo and Local: no edge, so no bearer.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::error::{ClientError, Result};
use crate::events::EventPump;

pub(crate) fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client")
    })
}

enum Mode {
    Demo,
    Dev { bearer: String },
}

/// Per-client bearer source.
pub(crate) struct TokenProvider {
    mode: Mode,
}

impl TokenProvider {
    pub(crate) fn new(
        credentials: &crate::config::Credentials,
        _edge_url: &str,
        _events: Arc<EventPump>,
    ) -> Self {
        use crate::config::Credentials;
        let mode = match credentials {
            Credentials::Demo(_) | Credentials::Local { .. } => Mode::Demo,
            Credentials::Dev { user_id, org_id } => Mode::Dev {
                bearer: if org_id.is_empty() {
                    user_id.clone()
                } else {
                    format!("{user_id}@{org_id}")
                },
            },
        };
        Self { mode }
    }

    pub(crate) async fn bearer(&self) -> Result<String> {
        match &self.mode {
            Mode::Demo => Err(ClientError::Auth(
                "demo mode has no edge credentials".into(),
            )),
            Mode::Dev { bearer } => Ok(bearer.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dev_bearer_is_user_at_org() {
        let events = EventPump::new(Arc::new(crate::events::NullListener));
        let credentials = crate::config::Credentials::Dev {
            user_id: "wing".into(),
            org_id: "acme".into(),
        };
        let tokens = TokenProvider::new(&credentials, "http://unused", events);
        assert_eq!(tokens.bearer().await.unwrap(), "wing@acme");
    }
}
