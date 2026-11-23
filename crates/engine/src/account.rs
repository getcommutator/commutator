//! This machine's sign-in to a Vardoger server (`apps/vardoger-server`):
//! what lets the account's other devices reach it through the server's
//! relay. Saved at `{data_dir}/account.json` (0600) as the server's URL,
//! this machine's bearer token, and who signed in.
//!
//! Signing in runs the server's Google flow with PKCE: the server hands back
//! a one-time code (to a loopback callback here, or shown to paste), and
//! only this process, holding the verifier, can redeem it for a token. The
//! machine keeps its own engine device id on the server, so it's one id
//! everywhere.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How long a sign-in waits for the browser.
const SIGN_IN_WAIT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub server_url: String,
    pub token: String,
    pub device_id: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("account.json")
}

impl Account {
    /// The saved sign-in, if any (an unreadable one counts as none).
    pub fn load(data_dir: &Path) -> Option<Self> {
        let raw = vardoger_rpc::ipc_auth::read_token(&path(data_dir)).ok()??;
        serde_json::from_str(&raw).ok()
    }

    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string(self).map_err(std::io::Error::other)?;
        vardoger_rpc::ipc_auth::write_token(&path(data_dir), &json)
    }

    /// Forget the sign-in here. The server keeps the device until it's
    /// removed there (`DELETE /devices/{id}`, which [`Self::sign_out`] does).
    pub fn forget(data_dir: &Path) -> std::io::Result<()> {
        match std::fs::remove_file(path(data_dir)) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }

    /// Remove this device on the server, then forget it here. A server
    /// that can't be reached still leaves this machine signed out.
    pub async fn sign_out(&self, data_dir: &Path) -> std::io::Result<Result<(), String>> {
        let removed = reqwest::Client::new()
            .delete(format!("{}/devices/{}", self.server_url, self.device_id))
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map(drop)
            .map_err(|err| err.to_string());
        Self::forget(data_dir)?;
        Ok(removed)
    }

    pub fn token_source(&self) -> std::sync::Arc<dyn vardoger_rpc::TokenSource> {
        std::sync::Arc::new(vardoger_rpc::StaticToken(self.token.clone()))
    }
}

/// How the one-time code comes back from the browser.
pub enum Callback {
    /// A loopback listener here; the browser must be on this machine.
    Loopback,
    /// The server shows it; the person pastes it ([`SignIn::redeem`]).
    Paste,
}

/// A started sign-in: open [`Self::url`], then [`Self::wait`] (loopback) or
/// [`Self::redeem`] the pasted code.
pub struct SignIn {
    server_url: String,
    device_id: String,
    verifier: String,
    url: String,
    listener: Option<tokio::net::TcpListener>,
}

pub fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

impl SignIn {
    pub async fn start(
        server_url: &str,
        device_id: &str,
        name: &str,
        callback: Callback,
    ) -> std::io::Result<Self> {
        let server_url = server_url.trim_end_matches('/').to_owned();
        let verifier = vardoger_rpc::ipc_auth::generate_token()?;
        let listener = match callback {
            Callback::Loopback => Some(tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?),
            Callback::Paste => None,
        };
        let mut query = vec![
            ("challenge", pkce_challenge(&verifier)),
            ("name", name.to_owned()),
            ("kind", "machine".to_owned()),
        ];
        if let Some(listener) = &listener {
            let port = listener.local_addr()?.port();
            query.push(("redirect", format!("http://127.0.0.1:{port}/callback")));
        }
        let url = reqwest::Url::parse_with_params(&format!("{server_url}/auth/google"), &query)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?
            .to_string();
        Ok(Self {
            server_url,
            device_id: device_id.to_owned(),
            verifier,
            url,
            listener,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The code the browser brings to the loopback callback, redeemed.
    pub async fn wait(&self) -> anyhow::Result<Account> {
        let listener = self
            .listener
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("this sign-in takes a pasted code"))?;
        let code = tokio::time::timeout(SIGN_IN_WAIT, async {
            loop {
                let (mut stream, _) = listener.accept().await?;
                if let Some(code) = callback_code(&mut stream).await {
                    return Ok::<_, std::io::Error>(code);
                }
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("no sign-in within 10 minutes"))??;
        self.redeem(&code).await
    }

    /// Trade the one-time code for this machine's token.
    pub async fn redeem(&self, code: &str) -> anyhow::Result<Account> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Redeemed {
            token: String,
            device_id: String,
            user: User,
        }
        #[derive(Deserialize)]
        struct User {
            email: String,
            name: Option<String>,
        }
        let response = reqwest::Client::new()
            .post(format!("{}/auth/redeem", self.server_url))
            .json(&serde_json::json!({
                "code": code.trim(),
                "verifier": self.verifier,
                "deviceId": self.device_id,
            }))
            .timeout(Duration::from_secs(15))
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("the server refused the code ({status}): {}", body.trim());
        }
        let redeemed: Redeemed = response.json().await?;
        Ok(Account {
            server_url: self.server_url.clone(),
            token: redeemed.token,
            device_id: redeemed.device_id,
            email: redeemed.user.email,
            name: redeemed.user.name,
        })
    }
}

/// Read one HTTP request off a loopback connection; answer it, and return
/// its `code` if it is the callback.
async fn callback_code(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = vec![0u8; 8192];
    let mut len = 0;
    loop {
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf[len..]))
            .await
            .ok()?
            .ok()?;
        if read == 0 {
            return None;
        }
        len += read;
        if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") || len == buf.len() {
            break;
        }
    }
    let request = String::from_utf8_lossy(&buf[..len]);
    let target = request.lines().next()?.split(' ').nth(1)?;
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    let code = (url.path() == "/callback")
        .then(|| {
            url.query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.into_owned())
        })
        .flatten();
    let body = if code.is_some() {
        "Signed in to Vardoger. You can close this tab."
    } else {
        "Not found"
    };
    let status = if code.is_some() {
        "200 OK"
    } else {
        "404 Not Found"
    };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_loopback_callback_hands_over_its_code() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            stream
                .write_all(b"GET /callback?code=abc%20123 HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .unwrap();
            let mut reply = String::new();
            stream.read_to_string(&mut reply).await.unwrap();
            reply
        });
        let (mut stream, _) = listener.accept().await.unwrap();
        assert_eq!(callback_code(&mut stream).await.as_deref(), Some("abc 123"));
        drop(stream);
        assert!(browser.await.unwrap().starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn saved_sign_in_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Account::load(dir.path()), None);
        let account = Account {
            server_url: "https://vardoger.example.com".into(),
            token: "t".into(),
            device_id: "d".into(),
            email: "me@example.com".into(),
            name: None,
        };
        account.save(dir.path()).unwrap();
        assert_eq!(Account::load(dir.path()), Some(account));
        Account::forget(dir.path()).unwrap();
        assert_eq!(Account::load(dir.path()), None);
    }
}
