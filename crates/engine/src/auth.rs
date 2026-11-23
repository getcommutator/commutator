//! Who this machine is signed in as: its sign-in to a Vardoger server
//! ([`crate::account`]), read when the engine starts. `vardoger login` signs
//! in; signing out here ([`Auth::sign_out`]) removes this machine on the
//! server too. Signed out, the engine works on its own as always.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use vardoger_rpc::TokenError;

use crate::account::Account;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// AuthStatus stream payload, in the [`vardoger_proto::AuthState`] wire shape.
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

    pub fn to_proto(&self) -> vardoger_proto::AuthState {
        match self {
            AuthState::SignedOut => vardoger_proto::AuthState::SignedOut,
            AuthState::SignedIn { user } => vardoger_proto::AuthState::SignedIn {
                user: vardoger_proto::UserProfile {
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
impl vardoger_rpc::TokenSource for Auth {
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
}
