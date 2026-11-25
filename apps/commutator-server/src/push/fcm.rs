//! Firebase Cloud Messaging (HTTP v1) for the Android app. A service
//! account's key signs a JWT that Google trades for an hour-long access
//! token, reused until shortly before it runs out.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde::Deserialize;

use super::Notification;
use crate::now;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";

#[derive(Deserialize)]
struct ServiceAccount {
    project_id: String,
    client_email: String,
    private_key: String,
}

pub(crate) struct Fcm {
    project_id: String,
    client_email: String,
    key: ring::signature::RsaKeyPair,
    /// (access token, when it stops being reused).
    access: tokio::sync::Mutex<Option<(String, i64)>>,
    http: reqwest::Client,
}

impl Fcm {
    /// `COMMUTATOR_FCM_SERVICE_ACCOUNT_FILE`: the service account JSON from
    /// Firebase › Project settings › Service accounts.
    pub(crate) fn from_env() -> Option<Self> {
        let path = std::env::var("COMMUTATOR_FCM_SERVICE_ACCOUNT_FILE")
            .ok()
            .filter(|v| !v.trim().is_empty())?;
        match std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|json| Self::new(&json))
        {
            Ok(fcm) => Some(fcm),
            Err(err) => {
                tracing::error!(error = %err, "FCM not configured");
                None
            }
        }
    }

    pub(crate) fn new(json: &str) -> anyhow::Result<Self> {
        let account: ServiceAccount = serde_json::from_str(json)?;
        let der: String = account
            .private_key
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect();
        let key = ring::signature::RsaKeyPair::from_pkcs8(&STANDARD.decode(der.trim())?)
            .map_err(|err| anyhow::anyhow!("FCM key: {err}"))?;
        Ok(Self {
            project_id: account.project_id,
            client_email: account.client_email,
            key,
            access: tokio::sync::Mutex::new(None),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()?,
        })
    }

    /// The signed assertion Google trades for an access token.
    fn assertion(&self, at: i64) -> anyhow::Result<String> {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "iss": self.client_email,
                "scope": SCOPE,
                "aud": TOKEN_URL,
                "iat": at,
                "exp": at + 3600,
            })
            .to_string(),
        );
        let input = format!("{header}.{claims}");
        let mut signature = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(
                &ring::signature::RSA_PKCS1_SHA256,
                &ring::rand::SystemRandom::new(),
                input.as_bytes(),
                &mut signature,
            )
            .map_err(|err| anyhow::anyhow!("FCM sign: {err}"))?;
        Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature)))
    }

    async fn access_token(&self) -> anyhow::Result<String> {
        let mut cached = self.access.lock().await;
        let at = now();
        if let Some((token, until)) = cached.as_ref()
            && at < *until
        {
            return Ok(token.clone());
        }
        #[derive(Deserialize)]
        struct Granted {
            access_token: String,
            expires_in: i64,
        }
        let granted: Granted = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &self.assertion(at)?),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        // Five minutes' margin before Google's own expiry.
        *cached = Some((granted.access_token.clone(), at + granted.expires_in - 300));
        Ok(granted.access_token)
    }

    /// One push, as a data message: the app shows it (or not, in front) and
    /// opens the chat on a tap. `Ok(false)`: the token is dead, drop it.
    pub(crate) async fn send(&self, token: &str, note: &Notification) -> anyhow::Result<bool> {
        let data = serde_json::json!({
            "chatId": note.chat_id,
            "kind": note.kind,
            "title": note.title,
            "body": note.body,
        });
        self.send_data(token, &data).await
    }

    /// A data message (string values only, as FCM wants).
    pub(crate) async fn send_data(&self, token: &str, data: &serde_json::Value) -> anyhow::Result<bool> {
        let message = serde_json::json!({
            "message": {
                "token": token,
                "data": data,
                "android": { "priority": "high" },
            }
        });
        let response = self
            .http
            .post(format!(
                "https://fcm.googleapis.com/v1/projects/{}/messages:send",
                self.project_id
            ))
            .bearer_auth(self.access_token().await?)
            .json(&message)
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(true);
        }
        let reason = response.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::NOT_FOUND || reason.contains("UNREGISTERED") {
            return Ok(false);
        }
        anyhow::bail!("FCM {status}: {reason}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_assertion_is_an_rs256_jwt_for_the_messaging_scope() {
        // A throwaway RSA key in a service-account-shaped JSON.
        let pem = include_str!("../../tests/fixtures/rsa-test-key.pem");
        let json = serde_json::json!({
            "project_id": "demo-project",
            "client_email": "push@demo-project.iam.gserviceaccount.com",
            "private_key": pem,
        });
        let fcm = Fcm::new(&json.to_string()).unwrap();
        let jwt = fcm.assertion(1_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "push@demo-project.iam.gserviceaccount.com");
        assert_eq!(claims["scope"], SCOPE);
        assert_eq!(claims["exp"], 4_600);
        let public = ring::signature::KeyPair::public_key(&fcm.key).as_ref().to_vec();
        ring::signature::UnparsedPublicKey::new(&ring::signature::RSA_PKCS1_2048_8192_SHA256, public)
            .verify(
                format!("{}.{}", parts[0], parts[1]).as_bytes(),
                &URL_SAFE_NO_PAD.decode(parts[2]).unwrap(),
            )
            .expect("signature verifies");
    }
}
