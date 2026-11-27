//! Commutator's own small server: Google sign-in, the account's devices, and the
//! device-room relay the engine's `HostRelay` and `LinkCache` already speak
//! (`commutator_rpc::device_room`).
//!
//! Every device (a machine running the engine, a phone, a desktop app) signs
//! in once and gets its own bearer token; the server keeps only its SHA-256.
//! Sign-in is Google's authorization-code flow run by the server, handed to
//! the device as a one-time code it redeems with a PKCE verifier, so a code
//! caught on the way (a deep link another app also claims, a pasted code
//! shoulder-surfed) is useless on its own.
//!
//! How each client signs in (all PKCE: the client keeps the verifier):
//! - a computer, `commutator login`: `redirect` is a loopback callback; the
//!   engine redeems the code, then sends the browser on to `/signed-in`
//!   (`--paste`/SSH: no `redirect`, the code is shown to paste);
//! - a phone or desktop app: opens `/auth/google?...&kind=client&redirect=
//!   commutator://auth` in the system sign-in sheet (iOS
//!   `ASWebAuthenticationSession`, Android Custom Tabs), which closes on
//!   that redirect and hands the app `?code=`; the app redeems it.
//!
//! Routes:
//! - `GET  /`: the web page: sign in with Google, see and remove devices
//!   (`web.rs`; `/auth/web`, `/web/devices/{id}/remove`, `/web/sign-out`),
//!   and `/signed-in`, where `commutator login` leaves the browser.
//! - `GET  /auth/google?challenge&name&kind[&redirect]`: start sign-in.
//! - `GET  /auth/google/callback`: Google's redirect; sends the one-time code
//!   to `redirect`, or shows it to paste.
//! - `POST /auth/redeem {code, verifier}` → `{token, deviceId, user}`.
//! - `POST /devices/pair` (bearer) → `{code, expiresIn}`: a one-time code a
//!   new machine signs in with, no browser (the desktop's Add a server,
//!   `install.sh --code`); `POST /auth/pair {code, name, deviceId?}` redeems it
//!   → `{token, deviceId, user}`. No PKCE: the code goes from the signed-in
//!   device to the machine by hand, so it is the secret; one use, short-lived.
//! - `GET  /install.sh`: `scripts/install.sh` with this server's address, for
//!   `curl -fsSL {server}/install.sh | sh -s -- --code {pair code}`.
//! - `GET  /me`, `GET /devices`, `DELETE /devices/{id}` (bearer).
//! - `PUT|DELETE /push/target`, `POST /push` (bearer): session
//!   notifications on the account's phones (`push.rs`).
//! - `POST /activity`, `PUT|DELETE /activity/target` (bearer): their Live
//!   Activities (`push/activity.rs`).
//! - `GET  /device/{id}/ws?role=host|client[&connId]` (bearer): the relay.
//! - `GET  /preview/ws?device={id}` (bearer): WebRTC signaling between the
//!   account's devices (`signal.rs`).

mod relay;
mod signal;
mod push;
mod web;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const GOOGLE_AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO: &str = "https://openidconnect.googleapis.com/v1/userinfo";
/// How long a started sign-in waits for Google, and a one-time code for its redeem.
const SIGN_IN_TTL: Duration = Duration::from_secs(10 * 60);
const CODE_TTL: Duration = Duration::from_secs(5 * 60);
/// How long a pair code waits for the machine to install and redeem it.
const PAIR_TTL: Duration = Duration::from_secs(10 * 60);

pub struct Config {
    /// Where devices and Google reach this server (`https://…`, no trailing `/`).
    pub public_url: String,
    pub google_client_id: String,
    pub google_client_secret: String,
    /// Lowercased; only these Google accounts may sign in.
    pub allowed_emails: Vec<String>,
    /// Handed to every device on its signaling socket, replacing its own
    /// default (public STUN). Empty: devices keep their default.
    pub ice_servers: Vec<IceServer>,
}

pub use signal::IceServer;

pub struct App {
    config: Config,
    db: Mutex<rusqlite::Connection>,
    /// `state` → a sign-in waiting for Google.
    signing_in: Mutex<HashMap<String, SignIn>>,
    /// One-time code → a signed-in user waiting for the device to redeem.
    codes: Mutex<HashMap<String, Grant>>,
    /// Pair codes → (user, expiry).
    pairs: Mutex<HashMap<String, (String, Instant)>>,
    push: push::Senders,
    activities: push::activity::Board,
    rooms: relay::Rooms,
    signaling: signal::Signaling,
    http: reqwest::Client,
}

struct SignIn {
    challenge: String,
    name: String,
    kind: Kind,
    redirect: Option<String>,
    expires: Instant,
    /// Started from the web page: ends in a browser session, not a code.
    web: bool,
}

struct Grant {
    user: String,
    challenge: String,
    name: String,
    kind: Kind,
    expires: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Runs the engine: hosts its own device room.
    Machine,
    /// A phone or desktop app: only dials machines.
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

/// A signed-in device, as its bearer token resolves.
#[derive(Clone, Debug)]
pub struct Device {
    pub id: String,
    pub user: String,
    pub kind: Kind,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

fn random() -> String {
    commutator_rpc::ipc_auth::generate_token().expect("OS randomness")
}

fn sha256_hex(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub use commutator_account::pkce_challenge;

/// Where a one-time code may be sent: this machine's loopback (the CLI's
/// callback server) or the apps' own scheme. Anything else would hand the
/// code to whoever owns that site, so it shows a page to paste from instead.
fn redirect_allowed(redirect: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(redirect) else {
        return false;
    };
    match url.scheme() {
        "commutator" => true,
        "http" => matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
        _ => false,
    }
}

impl App {
    pub fn open(config: Config, db: &Path) -> anyhow::Result<Arc<Self>> {
        let db = rusqlite::Connection::open(db)?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS users (
                 id TEXT PRIMARY KEY,
                 google_sub TEXT NOT NULL UNIQUE,
                 email TEXT NOT NULL,
                 name TEXT,
                 created INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS devices (
                 id TEXT PRIMARY KEY,
                 user_id TEXT NOT NULL REFERENCES users(id),
                 name TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 token_hash TEXT NOT NULL UNIQUE,
                 created INTEGER NOT NULL,
                 last_seen INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS web_sessions (
                 token_hash TEXT PRIMARY KEY,
                 user_id TEXT NOT NULL REFERENCES users(id),
                 expires INTEGER NOT NULL
             );",
        )?;
        db.execute_batch(push::SCHEMA)?;
        let _ = db.execute(push::PLATFORM_COLUMN, []);
        db.execute_batch(push::activity::SCHEMA)?;
        // ponytail: added after launch; fails harmlessly once the column exists.
        let _ = db.execute("ALTER TABLE users ADD COLUMN picture TEXT", []);
        Ok(Arc::new(Self {
            config,
            db: Mutex::new(db),
            signing_in: Mutex::default(),
            codes: Mutex::default(),
            pairs: Mutex::default(),
            // ponytail: from the env, not `Config`, so the tests' configs stay as they are.
            push: push::Senders::from_env(),
            activities: push::activity::Board::default(),
            rooms: relay::Rooms::default(),
            signaling: signal::Signaling::default(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()?,
        }))
    }

    /// The user with this Google account, created on first sign-in.
    pub fn upsert_user(
        &self,
        google_sub: &str,
        email: &str,
        name: Option<&str>,
    ) -> anyhow::Result<String> {
        let db = lock(&self.db);
        db.execute(
            "INSERT INTO users (id, google_sub, email, name, created) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(google_sub) DO UPDATE SET email = excluded.email, name = excluded.name",
            rusqlite::params![random(), google_sub, email, name, now()],
        )?;
        Ok(db.query_row(
            "SELECT id FROM users WHERE google_sub = ?1",
            [google_sub],
            |row| row.get(0),
        )?)
    }

    /// The profile photo `user` signed in with last.
    pub fn set_picture(&self, user: &str, picture: Option<&str>) -> anyhow::Result<()> {
        lock(&self.db).execute(
            "UPDATE users SET picture = ?2 WHERE id = ?1",
            rusqlite::params![user, picture],
        )?;
        Ok(())
    }

    /// A new device of `user`: its id and bearer token (shown only here).
    /// `id` keeps an id the device already has (an engine's own device id),
    /// replacing that device's earlier sign-in; the caller checks it's `user`'s.
    pub fn create_device(
        &self,
        user: &str,
        name: &str,
        kind: Kind,
        id: Option<&str>,
    ) -> anyhow::Result<(String, String)> {
        let (id, token) = (id.map_or_else(random, str::to_owned), random());
        let db = lock(&self.db);
        db.execute(
            "DELETE FROM devices WHERE id = ?1 AND user_id = ?2",
            rusqlite::params![id, user],
        )?;
        db.execute(
            "INSERT INTO devices (id, user_id, name, kind, token_hash, created, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            rusqlite::params![id, user, name, kind.as_str(), sha256_hex(&token), now()],
        )?;
        Ok((id, token))
    }

    fn device_by_token(&self, token: &str) -> Option<Device> {
        lock(&self.db)
            .query_row(
                "SELECT id, user_id, kind FROM devices WHERE token_hash = ?1",
                [sha256_hex(token)],
                |row| {
                    let kind: String = row.get(2)?;
                    Ok(Device {
                        id: row.get(0)?,
                        user: row.get(1)?,
                        kind: if kind == "machine" {
                            Kind::Machine
                        } else {
                            Kind::Client
                        },
                    })
                },
            )
            .ok()
    }

    /// Who owns device `id`, if it exists.
    fn owner_of(&self, id: &str) -> Option<String> {
        lock(&self.db)
            .query_row("SELECT user_id FROM devices WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .ok()
    }

    fn touch(&self, id: &str) {
        let _ = lock(&self.db).execute(
            "UPDATE devices SET last_seen = ?2 WHERE id = ?1",
            rusqlite::params![id, now()],
        );
    }

    /// The bearer from `Authorization: Bearer …`, else `?token=` (the
    /// engine's relay dials put it in the URL).
    fn authenticate(
        &self,
        headers: &HeaderMap,
        query_token: Option<&str>,
    ) -> Result<Device, StatusCode> {
        let header = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        let token = header.or(query_token).ok_or(StatusCode::UNAUTHORIZED)?;
        let device = self
            .device_by_token(token)
            .ok_or(StatusCode::UNAUTHORIZED)?;
        self.touch(&device.id);
        Ok(device)
    }
}

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        // `/` is the landing page (site/, its own Caddy behind the tunnel); this
        // is the browser sign-in and the account's devices.
        .route("/account", get(web::home))
        .route("/", get(|| async { Redirect::to("/account") }))
        .route("/icon.png", get(web::icon))
        .route("/auth/web", get(web::start))
        .route("/signed-in", get(web::signed_in_page))
        .route("/web/devices/:id/remove", post(web::remove_device))
        .route("/web/sign-out", post(web::sign_out))
        .route("/auth/google", get(start_sign_in))
        .route("/auth/google/callback", get(google_callback))
        .route("/auth/redeem", post(redeem))
        .route("/auth/pair", post(redeem_pair))
        .route("/install.sh", get(install_script))
        .route("/devices/pair", post(start_pair))
        .route("/me", get(me))
        .route("/devices", get(list_devices))
        .route("/devices/:id", delete(remove_device))
        .route("/device/:id/ws", get(relay::device_ws))
        .route("/preview/ws", get(signal::preview_ws))
        .route("/push/target", axum::routing::put(push::put_target).delete(push::delete_target))
        .route("/push", post(push::post_push))
        .route("/activity", post(push::activity::post_activity))
        .route(
            "/activity/target",
            axum::routing::put(push::activity::put_target).delete(push::activity::delete_target),
        )
        .with_state(app)
}

fn fail(status: StatusCode, message: &str) -> Response {
    (status, message.to_owned()).into_response()
}

#[derive(Deserialize)]
struct StartSignIn {
    challenge: String,
    name: String,
    kind: Kind,
    redirect: Option<String>,
}

async fn start_sign_in(State(app): State<Arc<App>>, Query(q): Query<StartSignIn>) -> Response {
    if q.challenge.len() < 43 || q.name.trim().is_empty() || q.name.len() > 100 {
        return fail(StatusCode::BAD_REQUEST, "challenge and name are required");
    }
    if q.redirect.as_deref().is_some_and(|r| !redirect_allowed(r)) {
        return fail(
            StatusCode::BAD_REQUEST,
            "redirect must be loopback or commutator://",
        );
    }
    to_google(
        &app,
        SignIn {
            challenge: q.challenge,
            name: q.name.trim().to_owned(),
            kind: q.kind,
            redirect: q.redirect,
            expires: Instant::now() + SIGN_IN_TTL,
            web: false,
        },
    )
}

/// Remember `sign_in` and send the browser to Google's account chooser.
fn to_google(app: &App, sign_in: SignIn) -> Response {
    let state = random();
    {
        let mut signing_in = lock(&app.signing_in);
        signing_in.retain(|_, s| s.expires > Instant::now());
        signing_in.insert(state.clone(), sign_in);
    }
    let url = reqwest::Url::parse_with_params(
        GOOGLE_AUTHORIZE,
        [
            ("client_id", app.config.google_client_id.as_str()),
            (
                "redirect_uri",
                &format!("{}/auth/google/callback", app.config.public_url),
            ),
            ("response_type", "code"),
            ("scope", "openid email profile"),
            ("state", &state),
            ("prompt", "select_account"),
        ],
    )
    .expect("static URL");
    Redirect::to(url.as_str()).into_response()
}

#[derive(Deserialize)]
struct Callback {
    code: Option<String>,
    state: String,
    error: Option<String>,
}

#[derive(Deserialize)]
struct GoogleUser {
    sub: String,
    email: String,
    #[serde(default)]
    email_verified: bool,
    name: Option<String>,
    /// The account's profile photo URL.
    picture: Option<String>,
}

async fn google_callback(State(app): State<Arc<App>>, Query(q): Query<Callback>) -> Response {
    let Some(sign_in) = lock(&app.signing_in).remove(&q.state) else {
        return fail(StatusCode::BAD_REQUEST, "this sign-in expired; start again");
    };
    if sign_in.expires <= Instant::now() {
        return fail(StatusCode::BAD_REQUEST, "this sign-in expired; start again");
    }
    let Some(code) = q.code else {
        return fail(
            StatusCode::BAD_REQUEST,
            q.error.as_deref().unwrap_or("Google sent no code"),
        );
    };
    let user = match google_user(&app, &code).await {
        Ok(user) => user,
        Err(err) => {
            tracing::warn!(error = %err, "google sign-in failed");
            return fail(StatusCode::BAD_GATEWAY, "Google sign-in failed");
        }
    };
    let email = user.email.to_lowercase();
    if !user.email_verified || !app.config.allowed_emails.contains(&email) {
        tracing::warn!(%email, "sign-in refused");
        return fail(
            StatusCode::FORBIDDEN,
            "this Google account may not sign in here",
        );
    }
    let picture = user.picture.clone();
    let user = match app.upsert_user(&user.sub, &email, user.name.as_deref()) {
        Ok(id) => id,
        Err(err) => {
            tracing::error!(error = %err, "storing user");
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not store the account",
            );
        }
    };
    if let Err(err) = app.set_picture(&user, picture.as_deref()) {
        tracing::warn!(error = %err, "storing the profile photo");
    }
    if sign_in.web {
        return web::signed_in(&app, &user);
    }
    let code = random();
    {
        let mut codes = lock(&app.codes);
        codes.retain(|_, g| g.expires > Instant::now());
        codes.insert(
            code.clone(),
            Grant {
                user,
                challenge: sign_in.challenge,
                name: sign_in.name,
                kind: sign_in.kind,
                expires: Instant::now() + CODE_TTL,
            },
        );
    }
    match sign_in.redirect {
        Some(redirect) => {
            let mut url = reqwest::Url::parse(&redirect).expect("checked at start");
            url.query_pairs_mut().append_pair("code", &code);
            Redirect::to(url.as_str()).into_response()
        }
        None => Html(format!(
            "<!doctype html><meta name=viewport content='width=device-width'>\
             <title>Commutator sign-in</title>\
             <body style='font:16px system-ui;margin:2rem'>\
             <p>Signed in. Paste this code where Commutator asked for it:</p>\
             <pre style='font-size:14px;word-break:break-all;white-space:pre-wrap'>{code}</pre>"
        ))
        .into_response(),
    }
}

async fn google_user(app: &App, code: &str) -> anyhow::Result<GoogleUser> {
    #[derive(Deserialize)]
    struct Tokens {
        access_token: String,
    }
    let tokens: Tokens = app
        .http
        .post(GOOGLE_TOKEN)
        .form(&[
            ("code", code),
            ("client_id", &app.config.google_client_id),
            ("client_secret", &app.config.google_client_secret),
            (
                "redirect_uri",
                &format!("{}/auth/google/callback", app.config.public_url),
            ),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    // Straight from Google over TLS with the token it just issued us, so the
    // answer needs no signature check of its own.
    Ok(app
        .http
        .get(GOOGLE_USERINFO)
        .bearer_auth(tokens.access_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Redeem {
    code: String,
    verifier: String,
    /// A machine's own engine device id, so it's one id everywhere.
    device_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Redeemed {
    token: String,
    device_id: String,
    user: UserView,
}

#[derive(Serialize)]
struct UserView {
    email: String,
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    picture: Option<String>,
}

async fn redeem(State(app): State<Arc<App>>, Json(body): Json<Redeem>) -> Response {
    // One try per code: a wrong verifier burns it too.
    let Some(grant) = lock(&app.codes).remove(body.code.trim()) else {
        return fail(StatusCode::BAD_REQUEST, "unknown or used code");
    };
    if grant.expires <= Instant::now() || pkce_challenge(&body.verifier) != grant.challenge {
        return fail(StatusCode::BAD_REQUEST, "unknown or used code");
    }
    let wanted = body.device_id.as_deref();
    if !app.id_free_for(wanted, &grant.user) {
        return fail(StatusCode::CONFLICT, "that device id is taken");
    }
    let (device_id, token) = match app.create_device(&grant.user, &grant.name, grant.kind, wanted) {
        Ok(created) => created,
        Err(err) => {
            tracing::error!(error = %err, "creating device");
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not add the device",
            );
        }
    };
    match app.user_view(&grant.user) {
        Some(user) => Json(Redeemed {
            token,
            device_id,
            user,
        })
        .into_response(),
        None => fail(StatusCode::INTERNAL_SERVER_ERROR, "account vanished"),
    }
}

/// The engine installer for a Linux server, signing in to this server.
async fn install_script(State(app): State<Arc<App>>) -> Response {
    let script = include_str!("../../../scripts/install.sh").replace("__SERVER__", &app.config.public_url);
    ([(axum::http::header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")], script).into_response()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PairCode {
    code: String,
    /// Seconds.
    expires_in: u64,
}

/// A code for a new machine of the caller's account.
async fn start_pair(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Response {
    let device = match app.authenticate(&headers, q.token.as_deref()) {
        Ok(device) => device,
        Err(status) => return status.into_response(),
    };
    let code = random();
    {
        let mut pairs = lock(&app.pairs);
        pairs.retain(|_, (_, expires)| *expires > Instant::now());
        pairs.insert(code.clone(), (device.user, Instant::now() + PAIR_TTL));
    }
    Json(PairCode {
        code,
        expires_in: PAIR_TTL.as_secs(),
    })
    .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RedeemPair {
    code: String,
    /// The machine's name (its hostname).
    name: String,
    /// The machine's own engine device id, as for `/auth/redeem`.
    device_id: Option<String>,
}

async fn redeem_pair(State(app): State<Arc<App>>, Json(body): Json<RedeemPair>) -> Response {
    let Some((user, expires)) = lock(&app.pairs).remove(body.code.trim()) else {
        return fail(StatusCode::BAD_REQUEST, "unknown or used code");
    };
    if expires <= Instant::now() {
        return fail(StatusCode::BAD_REQUEST, "unknown or used code");
    }
    let name = body.name.trim();
    if name.is_empty() || name.len() > 128 {
        return fail(StatusCode::BAD_REQUEST, "a name is needed");
    }
    let wanted = body.device_id.as_deref();
    if !app.id_free_for(wanted, &user) {
        return fail(StatusCode::CONFLICT, "that device id is taken");
    }
    let (device_id, token) = match app.create_device(&user, name, Kind::Machine, wanted) {
        Ok(created) => created,
        Err(err) => {
            tracing::error!(error = %err, "creating device");
            return fail(StatusCode::INTERNAL_SERVER_ERROR, "could not add the device");
        }
    };
    match app.user_view(&user) {
        Some(user) => Json(Redeemed {
            token,
            device_id,
            user,
        })
        .into_response(),
        None => fail(StatusCode::INTERNAL_SERVER_ERROR, "account vanished"),
    }
}

impl App {
    /// A device id a sign-in may keep: none asked, or well formed and not another account's.
    fn id_free_for(&self, wanted: Option<&str>, user: &str) -> bool {
        wanted.is_none_or(|id| {
            !id.is_empty()
                && id.len() <= 64
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && self.owner_of(id).is_none_or(|owner| owner == user)
        })
    }

    fn user_view(&self, user: &str) -> Option<UserView> {
        lock(&self.db)
            .query_row(
                "SELECT email, name, picture FROM users WHERE id = ?1",
                [user],
                |row| {
                    Ok(UserView {
                        email: row.get(0)?,
                        name: row.get(1)?,
                        picture: row.get(2)?,
                    })
                },
            )
            .ok()
    }
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Me {
    user: UserView,
    device_id: String,
}

async fn me(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Response {
    let device = match app.authenticate(&headers, q.token.as_deref()) {
        Ok(device) => device,
        Err(status) => return status.into_response(),
    };
    match app.user_view(&device.user) {
        Some(user) => Json(Me {
            user,
            device_id: device.id,
        })
        .into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// A machine whose engine holds its device room right now.
    pub online: bool,
    pub last_seen: i64,
    /// The device asking.
    pub current: bool,
}

async fn list_devices(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Response {
    let device = match app.authenticate(&headers, q.token.as_deref()) {
        Ok(device) => device,
        Err(status) => return status.into_response(),
    };
    match app.devices_of(&device.user, &device.id) {
        Ok(views) => Json(views).into_response(),
        Err(err) => fail(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

impl App {
    /// `user`'s devices, oldest first; `current` is the one asking.
    fn devices_of(&self, user: &str, current: &str) -> rusqlite::Result<Vec<DeviceView>> {
        let rows: Vec<(String, String, String, i64)> = {
            let db = lock(&self.db);
            let mut stmt = db.prepare(
                "SELECT id, name, kind, last_seen FROM devices WHERE user_id = ?1 ORDER BY created",
            )?;
            stmt.query_map([user], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        Ok(rows
            .into_iter()
            .map(|(id, name, kind, last_seen)| DeviceView {
                online: self.rooms.host_online(&id),
                current: id == current,
                kind: if kind == "machine" {
                    Kind::Machine
                } else {
                    Kind::Client
                },
                id,
                name,
                last_seen,
            })
            .collect())
    }

    /// Sign `user`'s device `id` out; `false` if it isn't theirs.
    fn remove_device_of(&self, user: &str, id: &str) -> rusqlite::Result<bool> {
        if self.owner_of(id).as_deref() != Some(user) {
            return Ok(false);
        }
        lock(&self.db).execute("DELETE FROM devices WHERE id = ?1", [id])?;
        self.rooms.close_device(id);
        self.signaling.close_device(id);
        Ok(true)
    }
}

/// Sign a device out for good: its token stops working and, for a machine,
/// its device room closes.
async fn remove_device(
    State(app): State<Arc<App>>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Response {
    let device = match app.authenticate(&headers, q.token.as_deref()) {
        Ok(device) => device,
        Err(status) => return status.into_response(),
    };
    match app.remove_device_of(&device.user, &id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => fail(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn codes_go_only_to_loopback_or_the_app_scheme() {
        assert!(redirect_allowed("http://127.0.0.1:53111/callback"));
        assert!(redirect_allowed("http://localhost:9/cb"));
        assert!(redirect_allowed("commutator://signed-in"));
        assert!(!redirect_allowed("https://evil.example/cb"));
        assert!(!redirect_allowed("http://127.0.0.1.evil.example/cb"));
        assert!(!redirect_allowed("not a url"));
    }
}
