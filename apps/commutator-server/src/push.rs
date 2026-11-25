//! Session notifications on the account's phones. A machine's engine decides
//! when one is due (a run finished, an agent waits on you, a run failed —
//! `desktop_notify` in the engine) and posts it here; the server hands it to
//! every phone of the account that registered a push token and wants that
//! kind.
//!
//! - `PUT    /push/target {platform?, token, environment, prefs}` (a phone's
//!   bearer): this phone's push token and which kinds it wants. `platform`
//!   `apns` (the default; `environment`: `sandbox` for Xcode builds,
//!   `production` for TestFlight/App Store) or `fcm` (Android).
//! - `DELETE /push/target`: this phone stops getting them.
//! - `POST   /push {chatId, kind, title, body}` (a machine's bearer).
//!
//! APNs is configured from the environment (`COMMUTATOR_APNS_KEY_FILE`, the
//! `.p8`; `COMMUTATOR_APNS_KEY_ID`; `COMMUTATOR_APNS_TEAM_ID`;
//! `COMMUTATOR_APNS_TOPIC`, the app's bundle id). Without it, targets are
//! still kept and pushes are dropped with a log line. FCM likewise, from
//! `COMMUTATOR_FCM_SERVICE_ACCOUNT_FILE` (`push/fcm.rs`).

pub(crate) mod activity;
mod fcm;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use crate::{App, Kind, TokenQuery, fail, lock, now};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS push_targets (
    device_id TEXT PRIMARY KEY,
    token TEXT NOT NULL,
    environment TEXT NOT NULL,
    prefs TEXT NOT NULL
);";
/// Added with FCM; fails harmlessly once the column exists.
pub(crate) const PLATFORM_COLUMN: &str =
    "ALTER TABLE push_targets ADD COLUMN platform TEXT NOT NULL DEFAULT 'apns'";

/// Where notifications go out: APNs (iOS) and FCM (Android), each only
/// when configured.
#[derive(Default)]
pub(crate) struct Senders {
    apns: Option<Arc<Apns>>,
    fcm: Option<Arc<fcm::Fcm>>,
}

impl Senders {
    pub(crate) fn from_env() -> Self {
        Self {
            apns: Apns::from_env(),
            fcm: fcm::Fcm::from_env().map(Arc::new),
        }
    }
}

/// A registered phone: (device, platform, token, environment).
pub type PushTarget = (String, String, String, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    pub done: bool,
    pub input: bool,
    pub failed: bool,
}

impl Prefs {
    fn wants(&self, kind: &str) -> bool {
        match kind {
            "done" => self.done,
            "input" => self.input,
            "failed" => self.failed,
            _ => false,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct Target {
    #[serde(default = "apns")]
    platform: String,
    token: String,
    environment: String,
    prefs: Prefs,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Notification {
    chat_id: String,
    kind: String,
    title: String,
    body: String,
}

fn apns() -> String {
    "apns".into()
}

/// APNs token-based auth: a `.p8` key signs a JWT that stays good for an
/// hour; Apple asks for it to be reused, not minted per push.
pub(crate) struct Apns {
    key: ring::signature::EcdsaKeyPair,
    key_id: String,
    team_id: String,
    topic: String,
    jwt: Mutex<Option<(String, i64)>>,
    http: reqwest::Client,
}

impl Apns {
    fn from_env() -> Option<Arc<Self>> {
        let var = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
        let path = var("COMMUTATOR_APNS_KEY_FILE")?;
        let apns = (|| -> anyhow::Result<Self> {
            let pem = std::fs::read_to_string(&path)?;
            Self::new(
                &pem,
                &var("COMMUTATOR_APNS_KEY_ID").ok_or_else(|| anyhow::anyhow!("COMMUTATOR_APNS_KEY_ID is not set"))?,
                &var("COMMUTATOR_APNS_TEAM_ID").ok_or_else(|| anyhow::anyhow!("COMMUTATOR_APNS_TEAM_ID is not set"))?,
                &var("COMMUTATOR_APNS_TOPIC").unwrap_or_else(|| "dev.freethinkel.commutator".into()),
            )
        })();
        match apns {
            Ok(apns) => Some(Arc::new(apns)),
            Err(err) => {
                tracing::error!(error = %err, "APNs not configured");
                None
            }
        }
    }

    fn new(pem: &str, key_id: &str, team_id: &str, topic: &str) -> anyhow::Result<Self> {
        let der: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = STANDARD.decode(der.trim())?;
        let key = ring::signature::EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &der,
            &ring::rand::SystemRandom::new(),
        )
        .map_err(|err| anyhow::anyhow!("APNs key: {err}"))?;
        Ok(Self {
            key,
            key_id: key_id.to_owned(),
            team_id: team_id.to_owned(),
            topic: topic.to_owned(),
            jwt: Mutex::new(None),
            http: reqwest::Client::builder()
                .http2_prior_knowledge()
                .timeout(Duration::from_secs(15))
                .build()?,
        })
    }

    /// The provider token, minted again after 50 minutes (Apple rejects one
    /// older than an hour).
    fn jwt(&self) -> anyhow::Result<String> {
        let mut cached = lock(&self.jwt);
        let at = now();
        if let Some((jwt, issued)) = cached.as_ref()
            && at - issued < 50 * 60
        {
            return Ok(jwt.clone());
        }
        let header = URL_SAFE_NO_PAD.encode(format!(r#"{{"alg":"ES256","kid":"{}"}}"#, self.key_id));
        let claims = URL_SAFE_NO_PAD.encode(format!(r#"{{"iss":"{}","iat":{at}}}"#, self.team_id));
        let input = format!("{header}.{claims}");
        let signature = self
            .key
            .sign(&ring::rand::SystemRandom::new(), input.as_bytes())
            .map_err(|err| anyhow::anyhow!("APNs sign: {err}"))?;
        let jwt = format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
        *cached = Some((jwt.clone(), at));
        Ok(jwt)
    }

    /// One push. `Ok(false)`: the token is dead (unregistered, bad), drop it.
    async fn send(&self, token: &str, environment: &str, note: &Notification) -> anyhow::Result<bool> {
        let payload = serde_json::json!({
            "aps": {
                "alert": { "title": note.title, "body": note.body },
                "sound": "default",
                "thread-id": note.chat_id,
            },
            "chatId": note.chat_id,
        });
        self.post(token, environment, "alert", &self.topic, 10, &payload).await
    }

    /// A Live Activity update or end (`apns-push-type: liveactivity`).
    pub(crate) async fn send_activity(
        &self,
        token: &str,
        environment: &str,
        payload: &serde_json::Value,
    ) -> anyhow::Result<bool> {
        let topic = format!("{}.push-type.liveactivity", self.topic);
        self.post(token, environment, "liveactivity", &topic, 10, payload).await
    }

    async fn post(
        &self,
        token: &str,
        environment: &str,
        push_type: &str,
        topic: &str,
        priority: u8,
        payload: &serde_json::Value,
    ) -> anyhow::Result<bool> {
        let host = if environment == "sandbox" {
            "api.sandbox.push.apple.com"
        } else {
            "api.push.apple.com"
        };
        let response = self
            .http
            .post(format!("https://{host}/3/device/{token}"))
            .header("authorization", format!("bearer {}", self.jwt()?))
            .header("apns-topic", topic)
            .header("apns-push-type", push_type)
            .header("apns-priority", priority.to_string())
            .json(payload)
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(true);
        }
        let reason = response.text().await.unwrap_or_default();
        if status == StatusCode::GONE || reason.contains("BadDeviceToken") || reason.contains("DeviceTokenNotForTopic") {
            return Ok(false);
        }
        anyhow::bail!("APNs {status}: {reason}")
    }
}

impl App {
    fn set_push_target(&self, device: &str, target: &Target) -> anyhow::Result<()> {
        lock(&self.db).execute(
            "INSERT INTO push_targets (device_id, platform, token, environment, prefs)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(device_id) DO UPDATE SET platform = excluded.platform,
                 token = excluded.token, environment = excluded.environment, prefs = excluded.prefs",
            rusqlite::params![
                device,
                target.platform,
                target.token,
                target.environment,
                serde_json::to_string(&target.prefs)?
            ],
        )?;
        Ok(())
    }

    fn remove_push_target(&self, device: &str) {
        let _ = lock(&self.db).execute("DELETE FROM push_targets WHERE device_id = ?1", [device]);
    }

    /// `user`'s phones that want `kind`. The join drops targets of
    /// signed-out devices.
    pub fn push_targets_for(&self, user: &str, kind: &str) -> rusqlite::Result<Vec<PushTarget>> {
        let db = lock(&self.db);
        let mut stmt = db.prepare(
            "SELECT p.device_id, p.platform, p.token, p.environment, p.prefs FROM push_targets p
             JOIN devices d ON d.id = p.device_id WHERE d.user_id = ?1",
        )?;
        let rows = stmt
            .query_map([user], |row| {
                Ok((
                    (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?),
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<(PushTarget, String)>>>()?;
        Ok(rows
            .into_iter()
            .filter(|(_, prefs)| serde_json::from_str::<Prefs>(prefs).is_ok_and(|p| p.wants(kind)))
            .map(|(target, _)| target)
            .collect())
    }
}

pub(crate) async fn put_target(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
    Json(target): Json<Target>,
) -> Response {
    let device = match app.authenticate(&headers, q.token.as_deref()) {
        Ok(device) => device,
        Err(status) => return status.into_response(),
    };
    if let Err(problem) = valid(&target) {
        return fail(StatusCode::BAD_REQUEST, problem);
    }
    match app.set_push_target(&device.id, &target) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => fail(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

pub(crate) async fn delete_target(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Response {
    match app.authenticate(&headers, q.token.as_deref()) {
        Ok(device) => {
            app.remove_push_target(&device.id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(status) => status.into_response(),
    }
}

pub(crate) async fn post_push(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
    Json(note): Json<Notification>,
) -> Response {
    let device = match app.authenticate(&headers, q.token.as_deref()) {
        Ok(device) => device,
        Err(status) => return status.into_response(),
    };
    if device.kind != Kind::Machine {
        return StatusCode::FORBIDDEN.into_response();
    }
    let targets = match app.push_targets_for(&device.user, &note.kind) {
        Ok(targets) => targets,
        Err(err) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    };
    // Answer the engine at once; delivery is best effort.
    tokio::spawn(async move {
        for (device, platform, token, environment) in targets {
            let sent = match (platform.as_str(), &app.push.apns, &app.push.fcm) {
                ("apns", Some(apns), _) => apns.send(&token, &environment, &note).await,
                ("fcm", _, Some(fcm)) => fcm.send(&token, &note).await,
                _ => {
                    tracing::warn!(%device, %platform, "push dropped: not configured");
                    continue;
                }
            };
            match sent {
                Ok(true) => {}
                Ok(false) => app.remove_push_target(&device),
                Err(err) => tracing::warn!(%device, error = %err, "push failed"),
            }
        }
    });
    StatusCode::ACCEPTED.into_response()
}

/// A target the senders can use. An APNs token lands in the request path,
/// so only hex; an FCM registration token is base64url-ish with colons.
fn valid(target: &Target) -> Result<(), &'static str> {
    let token = target.token.as_bytes();
    match target.platform.as_str() {
        "apns" => {
            if token.is_empty() || token.len() > 200 || !token.iter().all(u8::is_ascii_hexdigit) {
                return Err("token must be hex");
            }
            if !matches!(target.environment.as_str(), "sandbox" | "production") {
                return Err("environment must be sandbox or production");
            }
            Ok(())
        }
        "fcm" => {
            let ok = |b: &u8| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':');
            if token.is_empty() || token.len() > 4096 || !token.iter().all(ok) {
                return Err("not an FCM token");
            }
            Ok(())
        }
        _ => Err("platform must be apns or fcm"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_provider_token_is_an_es256_jwt() {
        // A throwaway P-256 key, PKCS#8 like an App Store Connect `.p8`.
        let rng = ring::rand::SystemRandom::new();
        let der = ring::signature::EcdsaKeyPair::generate_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &rng,
        )
        .unwrap();
        let pem = format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", STANDARD.encode(der.as_ref()));
        let apns = Apns::new(&pem, "KEY123", "TEAM456", "dev.example").unwrap();
        let jwt = apns.jwt().unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header = String::from_utf8(URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header, r#"{"alg":"ES256","kid":"KEY123"}"#);
        let claims: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "TEAM456");
        let public = ring::signature::KeyPair::public_key(&apns.key).as_ref().to_vec();
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, public)
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &URL_SAFE_NO_PAD.decode(parts[2]).unwrap())
            .expect("signature verifies");
        assert_eq!(apns.jwt().unwrap(), jwt, "reused within the hour");
    }

    #[test]
    fn prefs_pick_the_kinds() {
        let prefs = Prefs { done: true, input: false, failed: true };
        assert!(prefs.wants("done") && prefs.wants("failed"));
        assert!(!prefs.wants("input") && !prefs.wants("bogus"));
    }
}
