//! Signing the phone in to the account's Vardoger server, and the account's
//! machines to pick a host from (`vardoger-account`, `vardoger_client::server`).

use std::sync::Arc;

use vardoger_client as zc;

use super::{CoreError, CoreResult, on_runtime};

impl From<vardoger_account::Error> for CoreError {
    fn from(err: vardoger_account::Error) -> Self {
        match err {
            vardoger_account::Error::Invalid(message) => CoreError::InvalidArgument { message },
            vardoger_account::Error::Network(message) => CoreError::Network { message },
            vardoger_account::Error::Refused(message) => CoreError::Auth { message },
        }
    }
}

/// This phone's sign-in: keep all of it (Keychain / Keystore).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ServerAccount {
    pub server_url: String,
    /// This device's bearer on the server.
    pub token: String,
    /// This device's id on the server (use it as `CoreConfig.device_id`).
    pub device_id: String,
    pub email: String,
    pub name: Option<String>,
}

/// One of the account's devices (`GET /devices`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ServerDevice {
    pub id: String,
    pub name: String,
    /// Runs an engine: can host chats.
    pub is_machine: bool,
    /// Its engine holds its relay room right now.
    pub online: bool,
    /// Epoch seconds.
    pub last_seen: i64,
    /// This phone.
    pub current: bool,
}

/// A started sign-in: open `url()` in a web-auth session whose callback
/// scheme is `auth_callback_scheme()`, then `finish` with the URL it
/// comes back to.
#[derive(uniffi::Object)]
pub struct ServerSignIn {
    inner: vardoger_account::SignIn,
}

#[uniffi::export]
impl ServerSignIn {
    /// `device_name` is how the account lists this phone.
    #[uniffi::constructor]
    pub fn new(server_url: String, device_name: String) -> CoreResult<Arc<Self>> {
        let inner = vardoger_account::SignIn::start(
            &server_url,
            &device_name,
            vardoger_account::Kind::Client,
            Some(vardoger_account::APP_REDIRECT),
        )?;
        Ok(Arc::new(Self { inner }))
    }

    pub fn url(&self) -> String {
        self.inner.url().to_owned()
    }

    /// Redeem the code `vardoger://auth?code=…` carries.
    pub async fn finish(&self, callback_url: String) -> CoreResult<ServerAccount> {
        let inner = self.inner.clone();
        on_runtime(async move {
            let code = vardoger_account::callback_code(&callback_url)
                .map_err(|e| zc::ClientError::Auth(e.to_string()))?;
            let redeemed = inner
                .redeem(&code, None)
                .await
                .map_err(|e| zc::ClientError::Auth(e.to_string()))?;
            Ok(ServerAccount {
                server_url: inner.server_url().to_owned(),
                token: redeemed.token,
                device_id: redeemed.device_id,
                email: redeemed.email,
                name: redeemed.name,
            })
        })
        .await
    }
}

/// The web-auth callback scheme (`vardoger`).
#[uniffi::export]
pub fn auth_callback_scheme() -> String {
    vardoger_account::APP_REDIRECT
        .split_once(':')
        .map(|(scheme, _)| scheme.to_owned())
        .unwrap_or_default()
}

/// The account's devices, oldest first.
#[uniffi::export]
pub async fn server_devices(server_url: String, token: String) -> CoreResult<Vec<ServerDevice>> {
    let devices = zc::server::devices(&server_url, &token).await?;
    Ok(devices
        .into_iter()
        .map(|d| ServerDevice {
            is_machine: d.is_machine(),
            id: d.id,
            name: d.name,
            online: d.online,
            last_seen: d.last_seen,
            current: d.current,
        })
        .collect())
}

/// Sign out: remove this device from the account. The platform forgets
/// the saved sign-in whether or not the server could be reached.
#[uniffi::export]
pub async fn server_sign_out(server_url: String, token: String, device_id: String) -> CoreResult<()> {
    on_runtime(async move {
        vardoger_account::remove_device(&server_url, &token, &device_id)
            .await
            .map_err(|e| zc::ClientError::Network(e.to_string()))
    })
    .await
}
