//! Signing a device in to a Commutator server (`apps/commutator-server`).
//!
//! The server runs Google sign-in with PKCE: a device opens
//! `{server}/auth/google?challenge&name&kind[&redirect]` in a browser, the
//! server hands a one-time code to `redirect` (a loopback callback, or the
//! apps' `commutator://auth`) or shows it to paste, and only the holder of the
//! verifier can trade it (`POST /auth/redeem`) for the device's bearer token.

use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The apps' callback: `commutator://auth?code=…`.
pub const APP_REDIRECT: &str = "commutator://auth";

/// The hosted Commutator server every app signs in to.
pub const DEFAULT_SERVER_URL: &str = "https://commutator.freethinkel.dev";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("could not reach the server: {0}")]
    Network(String),
    #[error("the server refused: {0}")]
    Refused(String),
}

/// What a device signs in as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Runs the engine and hosts its own device room.
    Machine,
    /// A phone or an app: drives the account's machines.
    Client,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Machine => "machine",
            Kind::Client => "client",
        }
    }
}

/// PKCE S256: `base64url(sha256(verifier))`, unpadded.
pub fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// A redeemed sign-in: this device's bearer and who it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redeemed {
    pub token: String,
    pub device_id: String,
    pub email: String,
    pub name: Option<String>,
    /// The account's profile photo URL.
    pub picture: Option<String>,
}

/// A started sign-in: open [`Self::url`] in a browser, then [`Self::redeem`]
/// the code it comes back with.
#[derive(Debug, Clone)]
pub struct SignIn {
    server_url: String,
    verifier: String,
    url: String,
}

impl SignIn {
    /// `name` is the device's name on the account; `redirect` where the code
    /// goes (`None`: the server shows it to paste).
    pub fn start(
        server_url: &str,
        name: &str,
        kind: Kind,
        redirect: Option<&str>,
    ) -> Result<Self, Error> {
        let server_url = server_url.trim().trim_end_matches('/').to_owned();
        let verifier = commutator_rpc::ipc_auth::generate_token()
            .map_err(|err| Error::Invalid(err.to_string()))?;
        let mut query = vec![
            ("challenge", pkce_challenge(&verifier)),
            ("name", name.to_owned()),
            ("kind", kind.as_str().to_owned()),
        ];
        if let Some(redirect) = redirect {
            query.push(("redirect", redirect.to_owned()));
        }
        let url = reqwest::Url::parse_with_params(&format!("{server_url}/auth/google"), &query)
            .map_err(|err| Error::Invalid(format!("{server_url}: {err}")))?
            .to_string();
        Ok(Self {
            server_url,
            verifier,
            url,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    /// Trade the one-time code for this device's token. `device_id` keeps an
    /// id the device already has (a machine's engine device id).
    pub async fn redeem(&self, code: &str, device_id: Option<&str>) -> Result<Redeemed, Error> {
        let response = reqwest::Client::new()
            .post(format!("{}/auth/redeem", self.server_url))
            .json(&serde_json::json!({
                "code": code.trim(),
                "verifier": self.verifier,
                "deviceId": device_id,
            }))
            .timeout(Duration::from_secs(15))
            .send()
            .await;
        redeemed(response).await
    }
}

/// A one-time code a new machine signs in to `token`'s account with
/// (`POST /devices/pair`; the desktop's Add a server).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairCode {
    pub code: String,
    /// Seconds it stays good for.
    pub expires_in: u64,
}

pub async fn pair_code(server_url: &str, token: &str) -> Result<PairCode, Error> {
    let response = reqwest::Client::new()
        .post(format!("{}/devices/pair", server_url.trim_end_matches('/')))
        .bearer_auth(token)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|err| Error::Network(err.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Refused(status.to_string()));
    }
    response
        .json()
        .await
        .map_err(|err| Error::Network(err.to_string()))
}

/// Sign this machine in with a [`PairCode`]'s code, no browser
/// (`POST /auth/pair`; `commutator login --code`).
pub async fn redeem_pair(
    server_url: &str,
    code: &str,
    name: &str,
    device_id: Option<&str>,
) -> Result<Redeemed, Error> {
    let response = reqwest::Client::new()
        .post(format!("{}/auth/pair", server_url.trim().trim_end_matches('/')))
        .json(&serde_json::json!({
            "code": code.trim(),
            "name": name,
            "deviceId": device_id,
        }))
        .timeout(Duration::from_secs(15))
        .send()
        .await;
    redeemed(response).await
}

/// `/auth/redeem`'s and `/auth/pair`'s answer.
async fn redeemed(response: reqwest::Result<reqwest::Response>) -> Result<Redeemed, Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        token: String,
        device_id: String,
        user: User,
    }
    #[derive(Deserialize)]
    struct User {
        email: String,
        name: Option<String>,
        #[serde(default)]
        picture: Option<String>,
    }
    let response = response.map_err(|err| Error::Network(err.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(Error::Refused(format!("{status}: {}", body.trim())));
    }
    let body: Body = response
        .json()
        .await
        .map_err(|err| Error::Network(err.to_string()))?;
    Ok(Redeemed {
        token: body.token,
        device_id: body.device_id,
        email: body.user.email,
        name: body.user.name,
        picture: body.user.picture,
    })
}

/// The code of a callback URL (`commutator://auth?code=…`, or a loopback
/// `/callback?code=…`), or the server's `error`.
pub fn callback_code(url: &str) -> Result<String, Error> {
    let url = reqwest::Url::parse(url).map_err(|err| Error::Invalid(err.to_string()))?;
    let param = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    match (param("code"), param("error")) {
        (Some(code), _) if !code.trim().is_empty() => Ok(code),
        (_, Some(error)) => Err(Error::Refused(error)),
        _ => Err(Error::Invalid("the callback carries no code".into())),
    }
}

/// Who a device's token belongs to now (`GET /me`): its email, name and
/// photo, which may have changed since sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Profile {
    pub email: String,
    pub name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
}

pub async fn me(server_url: &str, token: &str) -> Result<Profile, Error> {
    #[derive(Deserialize)]
    struct Me {
        user: Profile,
    }
    let response = reqwest::Client::new()
        .get(format!("{}/me", server_url.trim_end_matches('/')))
        .bearer_auth(token)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|err| Error::Network(err.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Refused(status.to_string()));
    }
    let me: Me = response
        .json()
        .await
        .map_err(|err| Error::Network(err.to_string()))?;
    Ok(me.user)
}

/// Remove device `device_id` from the account (sign-out).
pub async fn remove_device(server_url: &str, token: &str, device_id: &str) -> Result<(), Error> {
    let response = reqwest::Client::new()
        .delete(format!(
            "{}/devices/{device_id}",
            server_url.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|err| Error::Network(err.to_string()))?;
    match response.status() {
        status if status.is_success() => Ok(()),
        status => Err(Error::Refused(status.to_string())),
    }
}

/// Which session notifications a phone wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct PushPrefs {
    pub done: bool,
    pub input: bool,
    pub failed: bool,
}

/// A phone's session notifications (`PUT /push/target`): its APNs token
/// (hex), `sandbox` or `production`, and which kinds it wants.
pub async fn set_push_target(
    server_url: &str,
    token: &str,
    apns_token: &str,
    environment: &str,
    prefs: PushPrefs,
) -> Result<(), Error> {
    let body = serde_json::json!({ "token": apns_token, "environment": environment, "prefs": prefs });
    send(reqwest::Client::new().put(push_url(server_url, "/push/target")).json(&body), token).await
}

/// This phone stops getting session notifications (`DELETE /push/target`).
pub async fn remove_push_target(server_url: &str, token: &str) -> Result<(), Error> {
    send(reqwest::Client::new().delete(push_url(server_url, "/push/target")), token).await
}

/// A machine's session notification for the account's phones (`POST
/// /push`); `kind` is `done`, `input` or `failed`.
pub async fn push(
    server_url: &str,
    token: &str,
    chat_id: &str,
    kind: &str,
    title: &str,
    body: &str,
) -> Result<(), Error> {
    let note = serde_json::json!({ "chatId": chat_id, "kind": kind, "title": title, "body": body });
    send(reqwest::Client::new().post(push_url(server_url, "/push")).json(&note), token).await
}

fn push_url(server_url: &str, path: &str) -> String {
    format!("{}{path}", server_url.trim().trim_end_matches('/'))
}

async fn send(request: reqwest::RequestBuilder, token: &str) -> Result<(), Error> {
    let response = request
        .bearer_auth(token)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|err| Error::Network(err.to_string()))?;
    match response.status() {
        status if status.is_success() => Ok(()),
        status => Err(Error::Refused(status.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_in_url_carries_the_verifiers_challenge() {
        let sign_in = SignIn::start(
            "https://v.example/",
            "Egor's iPhone",
            Kind::Client,
            Some(APP_REDIRECT),
        )
        .unwrap();
        let url = reqwest::Url::parse(sign_in.url()).unwrap();
        assert_eq!(url.path(), "/auth/google");
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["challenge"], pkce_challenge(&sign_in.verifier));
        assert_eq!(q["challenge"].len(), 43);
        assert_eq!(q["name"], "Egor's iPhone");
        assert_eq!(q["kind"], "client");
        assert_eq!(q["redirect"], "commutator://auth");
        assert_eq!(sign_in.server_url(), "https://v.example");
    }

    #[test]
    fn callbacks_yield_their_code_or_error() {
        assert_eq!(callback_code("commutator://auth?code=a%20b").unwrap(), "a b");
        assert_eq!(
            callback_code("commutator://auth?error=denied"),
            Err(Error::Refused("denied".into()))
        );
        assert!(callback_code("commutator://auth").is_err());
    }
}
