//! The server's own page: sign in with Google in a browser, then see the
//! account's devices (which machines are online) and sign any of them out.
//!
//! A browser gets a session cookie (`HttpOnly`, `SameSite=Lax`, 30 days), not
//! a device: the server keeps only its SHA-256. Lax keeps the cookie off
//! cross-site POSTs, and the POSTs here also check `Origin`, so another site
//! can't remove a device through a signed-in browser.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};

use crate::{App, DeviceView, Kind, SIGN_IN_TTL, SignIn, lock, now, random, sha256_hex, to_google};

const COOKIE: &str = "vardoger_session";

/// Vardoger's mark: a figure and its double.
const LOGO: &str = r##"<svg class="logo" viewBox="0 0 48 48" aria-label="Vardoger" role="img"><circle cx="20" cy="24" r="11" fill="#ededed"/><circle cx="29" cy="24" r="11" fill="none" stroke="#ededed" stroke-width="2.5" stroke-dasharray="3 3"/></svg>"##;

/// Google's "G", in its own colours.
const GOOGLE: &str = r##"<svg viewBox="0 0 48 48" aria-hidden="true"><path fill="#EA4335" d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z"/><path fill="#4285F4" d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z"/><path fill="#FBBC05" d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z"/><path fill="#34A853" d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z"/></svg>"##;
const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

impl App {
    /// The signed-in user of this browser, if its session is live.
    fn web_user(&self, headers: &HeaderMap) -> Option<String> {
        let token = headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .find_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))?;
        lock(&self.db)
            .query_row(
                "SELECT user_id FROM web_sessions WHERE token_hash = ?1 AND expires > ?2",
                rusqlite::params![sha256_hex(token), now()],
                |row| row.get(0),
            )
            .ok()
    }

    fn cookie(&self, value: &str, max_age: u64) -> HeaderValue {
        let secure = if self.config.public_url.starts_with("https://") {
            "; Secure"
        } else {
            ""
        };
        HeaderValue::from_str(&format!(
            "{COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}"
        ))
        .expect("plain cookie")
    }

    /// A POST from this server's own page: same `Origin`, or none (some
    /// browsers omit it on same-origin form posts).
    fn same_origin(&self, headers: &HeaderMap) -> bool {
        headers
            .get(header::ORIGIN)
            .is_none_or(|origin| origin.to_str().ok() == Some(self.config.public_url.as_str()))
    }
}

/// Google said who this is: open a browser session and go home.
pub(crate) fn signed_in(app: &App, user: &str) -> Response {
    let token = random();
    let stored = lock(&app.db).execute(
        "INSERT INTO web_sessions (token_hash, user_id, expires) VALUES (?1, ?2, ?3)",
        rusqlite::params![
            sha256_hex(&token),
            user,
            now() + SESSION_TTL.as_secs() as i64
        ],
    );
    if let Err(err) = stored {
        tracing::error!(error = %err, "storing web session");
        return (StatusCode::INTERNAL_SERVER_ERROR, "could not sign in").into_response();
    }
    let mut response = Redirect::to("/").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        app.cookie(&token, SESSION_TTL.as_secs()),
    );
    response
}

pub(crate) async fn start(State(app): State<Arc<App>>) -> Response {
    to_google(
        &app,
        SignIn {
            challenge: String::new(),
            name: String::new(),
            kind: Kind::Client,
            redirect: None,
            expires: Instant::now() + SIGN_IN_TTL,
            web: true,
        },
    )
}

#[derive(serde::Deserialize)]
pub(crate) struct SignedInQuery {
    error: Option<String>,
}

/// Where a computer's `vardoger login` sends the browser once it has its
/// token (or failed to get one). Apps on a phone never see it: their
/// sign-in sheet closes on the `vardoger://` redirect.
pub(crate) async fn signed_in_page(Query(q): Query<SignedInQuery>) -> Response {
    let (title, text) = if q.error.is_some() {
        (
            "Sign-in didn't finish",
            "Vardoger couldn't finish signing in. The terminal says why.",
        )
    } else {
        (
            "You're signed in",
            "Return to Vardoger. You can close this tab.",
        )
    };
    page(
        &format!("{title} · Vardoger"),
        &format!(
            r#"<main>
  {LOGO}
  <h1>{title}</h1>
  <section class="card"><p class="note first">{text}</p></section>
</main>
<script>window.close()</script>"#
        ),
    )
}

pub(crate) async fn home(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = app.web_user(&headers) else {
        return page(
            "Sign in · Vardoger",
            &format!(
                r#"<main>
  {LOGO}
  <h1>Sign in</h1>
  <section class="card">
    <a class="provider" href="/auth/web">{GOOGLE}<span>Continue with Google</span></a>
    <p class="note">Your coding agents, from any of your devices.</p>
  </section>
</main>"#
            ),
        );
    };
    let email = app.user_view(&user).map(|u| u.email).unwrap_or_default();
    let devices = match app.devices_of(&user, "") {
        Ok(devices) => devices,
        Err(err) => return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    };
    let list = if devices.is_empty() {
        format!(
            r#"<p class="empty">No devices yet. On a computer, run<br><code>vardoger login {}</code></p>"#,
            escape(&app.config.public_url)
        )
    } else {
        let now = now();
        devices.iter().map(|d| row(d, now)).collect::<String>()
    };
    page(
        "Devices · Vardoger",
        &format!(
            r#"<main>
  {LOGO}
  <h1>Devices</h1>
  <section class="card list"><ul>{list}</ul></section>
  <form class="account" method="post" action="/web/sign-out">
    <span>{email}</span><span aria-hidden="true">·</span><button class="link">Sign out</button>
  </form>
</main>"#,
            email = escape(&email),
        ),
    )
}

fn row(device: &DeviceView, now: i64) -> String {
    let status = match (device.kind, device.online) {
        (Kind::Machine, true) => "Online".to_owned(),
        _ => format!("Seen {}", ago(now - device.last_seen)),
    };
    let kind = match device.kind {
        Kind::Machine => "Computer",
        Kind::Client => "App",
    };
    format!(
        r#"<li>
  <span class="dot{online}" aria-hidden="true"></span>
  <div class="info"><strong>{name}</strong><span class="muted">{kind} · {status}</span></div>
  <form method="post" action="/web/devices/{id}/remove" onsubmit="return confirm('Sign {name_js} out? It will have to sign in again.')">
    <button class="link danger">Remove</button>
  </form>
</li>"#,
        online = if device.online { " on" } else { "" },
        name = escape(&device.name),
        name_js = escape(&device.name.replace(['\'', '\\'], "")),
        id = escape(&device.id),
    )
}

/// "just now", "5 minutes ago", "3 hours ago", "2 days ago".
fn ago(seconds: i64) -> String {
    let unit = |n: i64, what: &str| format!("{n} {what}{} ago", if n == 1 { "" } else { "s" });
    match seconds.max(0) {
        s if s < 60 => "just now".to_owned(),
        s if s < 3600 => unit(s / 60, "minute"),
        s if s < 86400 => unit(s / 3600, "hour"),
        s => unit(s / 86400, "day"),
    }
}

pub(crate) async fn remove_device(
    State(app): State<Arc<App>>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = app.web_user(&headers).filter(|_| app.same_origin(&headers)) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    match app.remove_device_of(&user, &id) {
        Ok(_) => Redirect::to("/").into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

pub(crate) async fn sign_out(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !app.same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(user) = app.web_user(&headers) {
        // ponytail: ends every browser session of the account, not just this one.
        let _ = lock(&app.db).execute("DELETE FROM web_sessions WHERE user_id = ?1", [&user]);
    }
    let mut response = Redirect::to("/").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, app.cookie("", 0));
    response
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn page(title: &str, body: &str) -> Response {
    let mut response = Html(format!(
        r#"<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>
:root {{ color-scheme: dark; --bg:#000; --card:#1b1b1b; --line:#2e2e2e; --fg:#ededed; --muted:#8f8f8f; --hover:#262626; --ok:#3ecf8e; --danger:#ff6b6b; --focus:#7c6cf2; }}
* {{ box-sizing: border-box; }}
body {{ margin:0; min-height:100vh; background:var(--bg); color:var(--fg); font:15px/1.5 Inter, ui-sans-serif, system-ui, -apple-system, sans-serif; -webkit-font-smoothing:antialiased; }}
main {{ width:100%; max-width:440px; margin:0 auto; padding:96px 16px 48px; display:flex; flex-direction:column; align-items:center; }}
.logo {{ width:48px; height:48px; }}
h1 {{ margin:16px 0 24px; font-size:22px; font-weight:500; letter-spacing:-.01em; }}
.card {{ width:100%; background:var(--card); border-radius:16px; padding:48px; }}
.card.list {{ padding:8px; }}
.provider {{ display:flex; align-items:center; justify-content:center; gap:10px; height:40px; border:1px solid var(--line); border-radius:6px; color:var(--fg); text-decoration:none; font-weight:500; transition:background .15s; }}
.provider:hover {{ background:var(--hover); }}
.provider svg {{ width:16px; height:16px; }}
a:focus-visible, button:focus-visible {{ outline:2px solid var(--focus); outline-offset:2px; }}
.note.first {{ margin:0; }}
.note {{ margin:24px 0 0; text-align:center; color:var(--muted); font-size:14px; }}
ul {{ list-style:none; margin:0; padding:0; }}
li {{ display:flex; align-items:center; gap:14px; padding:12px 16px; border-radius:10px; }}
li:hover {{ background:var(--hover); }}
li form {{ margin:0 0 0 auto; }}
.info {{ display:flex; flex-direction:column; min-width:0; }}
.info strong {{ font-weight:500; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }}
.muted {{ color:var(--muted); font-size:13px; }}
.dot {{ flex:none; width:8px; height:8px; border-radius:50%; background:#4a4a4a; }}
.dot.on {{ background:var(--ok); box-shadow:0 0 0 3px rgba(62,207,142,.15); }}
button.link {{ background:none; border:0; padding:4px; font:inherit; font-size:14px; color:var(--muted); cursor:pointer; }}
button.link:hover {{ color:var(--fg); }}
button.danger:hover {{ color:var(--danger); }}
.empty {{ margin:0; padding:32px 24px; text-align:center; color:var(--muted); }}
code {{ display:inline-block; margin-top:8px; font:13px ui-monospace, SFMono-Regular, monospace; color:var(--fg); background:#000; padding:4px 8px; border-radius:6px; }}
.account {{ display:flex; gap:8px; align-items:center; margin-top:24px; color:var(--muted); font-size:14px; }}
@media (max-width:480px) {{ main {{ padding-top:64px; }} .card {{ padding:32px 24px; }} }}
</style>
<body>
{body}
</body>
</html>"#
    ))
    .into_response();
    // The page lists who's signed in: never cache it.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(dir: &std::path::Path) -> Arc<App> {
        App::open(
            crate::Config {
                public_url: "https://vardoger.example".into(),
                google_client_id: "id".into(),
                google_client_secret: "secret".into(),
                allowed_emails: vec!["me@example.com".into()],
            },
            &dir.join("db"),
        )
        .unwrap()
    }

    async fn body(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn a_signed_in_browser_sees_and_removes_its_devices() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let me = app.upsert_user("sub", "me@example.com", None).unwrap();
        let (laptop, _) = app
            .create_device(&me, "<b>laptop</b>", Kind::Machine, None)
            .unwrap();

        // Signed out: the sign-in page.
        let page = body(home(State(app.clone()), HeaderMap::new()).await).await;
        assert!(page.contains("Continue with Google"));

        let signed = signed_in(&app, &me);
        let cookie = signed.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(
            cookie.contains("HttpOnly")
                && cookie.contains("Secure")
                && cookie.contains("SameSite=Lax")
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(cookie.split(';').next().unwrap()).unwrap(),
        );

        let page = body(home(State(app.clone()), headers.clone()).await).await;
        assert!(page.contains("&lt;b&gt;laptop&lt;/b&gt;") && !page.contains("<b>laptop"));

        // Another site's form can't remove it.
        let mut cross = headers.clone();
        cross.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        let refused = remove_device(State(app.clone()), UrlPath(laptop.clone()), cross).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        assert!(app.owner_of(&laptop).is_some());

        let mut own = headers.clone();
        own.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://vardoger.example"),
        );
        remove_device(State(app.clone()), UrlPath(laptop.clone()), own.clone()).await;
        assert!(app.owner_of(&laptop).is_none());

        sign_out(State(app.clone()), own).await;
        assert!(app.web_user(&headers).is_none());
    }

    #[test]
    fn names_are_escaped_and_ages_read_naturally() {
        assert_eq!(
            escape(r#"<a href="x">'"#),
            "&lt;a href=&quot;x&quot;&gt;&#39;"
        );
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(7200), "2 hours ago");
        assert_eq!(ago(3 * 86400), "3 days ago");
    }
}
