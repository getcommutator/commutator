//! Who this machine is signed in as: its sign-in to a Commutator server
//! ([`crate::account`]), read when the engine starts. `commutator login` or
//! the app ([`Auth::sign_in`]) signs in; signing out here ([`Auth::sign_out`])
//! removes this machine on the server too. Signed out, the engine works on
//! its own as always.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use commutator_rpc::TokenError;

use crate::account::Account;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// AuthStatus stream payload, in the [`commutator_proto::AuthState`] wire shape.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    SignedOut,
    SignedIn { user: AuthUser },
}

impl AuthState {
    pub fn is_signed_in(&self) -> bool {
        matches!(self, AuthState::SignedIn { .. })
    }

    pub fn user(&self) -> Option<&AuthUser> {
        match self {
            AuthState::SignedIn { user } => Some(user),
            AuthState::SignedOut => None,
        }
    }

    pub fn to_proto(&self) -> commutator_proto::AuthState {
        match self {
            AuthState::SignedOut => commutator_proto::AuthState::SignedOut,
            AuthState::SignedIn { user } => commutator_proto::AuthState::SignedIn {
                user: commutator_proto::UserProfile {
                    id: user.id.clone(),
                    email: user.email.clone(),
                    name: user.name.clone(),
                },
                org_id: None,
            },
        }
    }
}

impl Serialize for AuthState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_proto().serialize(serializer)
    }
}

fn state_of(account: Option<&Account>) -> AuthState {
    match account {
        Some(account) => AuthState::SignedIn {
            user: AuthUser {
                id: account.email.clone(),
                email: account.email.clone(),
                name: account.name.clone(),
            },
        },
        None => AuthState::SignedOut,
    }
}

struct Inner {
    data_dir: PathBuf,
    account: std::sync::Mutex<Option<Account>>,
    state_tx: watch::Sender<AuthState>,
    /// The sign-in waiting on the browser; a new one replaces it.
    pending: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// The sign-in service — cheap to clone.
#[derive(Clone)]
pub struct Auth {
    inner: Arc<Inner>,
}

impl Auth {
    /// The sign-in saved in `data_dir`, if any.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        let account = Account::load(&data_dir);
        let (state_tx, _) = watch::channel(state_of(account.as_ref()));
        Self {
            inner: Arc::new(Inner {
                data_dir,
                account: std::sync::Mutex::new(account),
                state_tx,
                pending: std::sync::Mutex::new(None),
            }),
        }
    }

    pub fn account(&self) -> Option<Account> {
        self.inner
            .account
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn watch_state(&self) -> watch::Receiver<AuthState> {
        self.inner.state_tx.subscribe()
    }

    pub fn state(&self) -> AuthState {
        self.inner.state_tx.borrow().clone()
    }

    /// The signed-in account's email; `None` signed out.
    pub fn user_id(&self) -> Option<String> {
        self.account().map(|account| account.email)
    }

    /// Start signing in to `server` (else `COMMUTATOR_SERVER_URL`, else the
    /// server signed in to before) and return the page to open. The browser
    /// comes back to a loopback listener here; the account is then saved and
    /// the state moves to signed in. The relay picks it up at the next start.
    pub async fn sign_in(&self, server: Option<String>) -> anyhow::Result<String> {
        let data_dir = self.inner.data_dir.clone();
        let Some(server) = crate::account::server_url(server, self.account().as_ref()) else {
            anyhow::bail!(
                "no Commutator server to sign in to: set COMMUTATOR_SERVER_URL or run `commutator login <server>` once"
            );
        };
        let device_id = crate::device_id(&data_dir)?;
        let name = crate::device_name(&device_id);
        let sign_in =
            crate::account::SignIn::start(&server, &device_id, &name, crate::account::Callback::Loopback)
                .await?;
        let url = sign_in.url().to_owned();
        let this = self.clone();
        let task = tokio::spawn(async move {
            match sign_in.wait().await {
                Ok(account) => {
                    if let Err(err) = account.save(&data_dir) {
                        tracing::warn!(error = %err, "auth: could not save the sign-in");
                    }
                    this.inner.state_tx.send_replace(state_of(Some(&account)));
                    *this
                        .inner
                        .account
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(account);
                }
                Err(err) => tracing::warn!(error = %err, "auth: sign-in did not finish"),
            }
        });
        if let Some(previous) = self
            .inner
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(task)
        {
            previous.abort();
        }
        Ok(url)
    }

    /// Forget the sign-in here at once, and remove this machine on its
    /// server in the background.
    pub fn sign_out(&self) {
        let account = self
            .inner
            .account
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Err(err) = Account::forget(&self.inner.data_dir) {
            tracing::warn!(error = %err, "auth: could not remove the saved sign-in");
        }
        self.inner.state_tx.send_replace(AuthState::SignedOut);
        if let Some(account) = account {
            let data_dir = self.inner.data_dir.clone();
            tokio::spawn(async move {
                if let Ok(Err(err)) = account.sign_out(&data_dir).await {
                    tracing::warn!(error = %err, "auth: the server wasn't told of the sign-out");
                }
            });
        }
    }
}

/// The server's bearer for this machine: what its relay dials with.
#[async_trait::async_trait]
impl commutator_rpc::TokenSource for Auth {
    async fn token(&self) -> Result<String, TokenError> {
        self.account()
            .map(|account| account.token)
            .ok_or(TokenError::SignedOut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signing_out_forgets_the_saved_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Auth::new(dir.path()).state(), AuthState::SignedOut);
        Account {
            // Nothing listens there: the server-side removal just fails.
            server_url: "http://127.0.0.1:1".into(),
            token: "t".into(),
            device_id: "d".into(),
            email: "me@example.com".into(),
            name: None,
        }
        .save(dir.path())
        .unwrap();
        let auth = Auth::new(dir.path());
        assert_eq!(auth.state().user().map(|u| u.email.as_str()), Some("me@example.com"));
        auth.sign_out();
        assert_eq!(auth.state(), AuthState::SignedOut);
        assert_eq!(Account::load(dir.path()), None);
    }

    /// A stand-in server that redeems any code for `me@example.com`.
    async fn redeeming_server() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                let mut buf = vec![0u8; 8192];
                let _ = stream.read(&mut buf).await;
                let body = r#"{"token":"tok","deviceId":"d","user":{"email":"me@example.com","name":null}}"#;
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        url
    }

    #[tokio::test]
    async fn signing_in_from_the_app_saves_the_account_when_the_browser_returns() {
        let dir = tempfile::tempdir().unwrap();
        let auth = Auth::new(dir.path());
        let server = redeeming_server().await;
        let page = auth.sign_in(Some(server.clone())).await.unwrap();
        assert!(page.starts_with(&format!("{server}/auth/google?")), "{page}");
        let redirect = reqwest::Url::parse(&page)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "redirect")
            .map(|(_, v)| v.into_owned())
            .expect("a loopback redirect");
        // The browser comes back with the one-time code.
        let _ = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
            .get(format!("{redirect}?code=c1"))
            .send()
            .await
            .unwrap();
        let mut state = auth.watch_state();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            state.wait_for(AuthState::is_signed_in),
        )
        .await
        .expect("signed in")
        .unwrap();
        assert_eq!(auth.user_id().as_deref(), Some("me@example.com"));
        assert_eq!(Account::load(dir.path()).map(|a| a.server_url), Some(server));
    }

    #[tokio::test]
    async fn signing_in_needs_a_server() {
        // SAFETY: no other test in this binary reads it.
        unsafe { std::env::remove_var("COMMUTATOR_SERVER_URL") };
        let dir = tempfile::tempdir().unwrap();
        let error = Auth::new(dir.path()).sign_in(None).await.unwrap_err();
        assert!(format!("{error}").contains("COMMUTATOR_SERVER_URL"), "{error}");
    }
}
