//! The server's own page: sign in with Google in a browser, then see the
//! account's devices (which machines are online) and sign any of them out.
//!
//! A browser gets a session cookie (`HttpOnly`, `SameSite=Lax`, 30 days), not
//! a device: the server keeps only its SHA-256. Lax keeps the cookie off
//! cross-site POSTs, and the POSTs here also check `Origin`, so another site
//! can't remove a device through a signed-in browser.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};

use crate::{App, DeviceView, Kind, SIGN_IN_TTL, SignIn, lock, now, random, sha256_hex, to_google};

const COOKIE: &str = "vardoger_session";
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

pub(crate) async fn home(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(user) = app.web_user(&headers) else {
        return page(
            "Vardoger",
            r#"<main class="center">
  <h1>Vardoger</h1>
  <p class="muted">Your coding agents, from any of your devices.</p>
  <a class="button" href="/auth/web">Continue with Google</a>
</main>"#,
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
            r#"<header>
  <span class="brand">Vardoger</span>
  <form method="post" action="/web/sign-out"><span class="muted">{email}</span> <button class="link">Sign out</button></form>
</header>
<main>
  <h1>Devices</h1>
  <ul>{list}</ul>
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
:root {{ --bg:#fafaf9; --fg:#1c1917; --muted:#78716c; --line:#e7e5e4; --card:#fff; --accent:#1c1917; --on-accent:#fff; --ok:#16a34a; --danger:#dc2626; color-scheme: light dark; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg:#0c0a09; --fg:#f5f5f4; --muted:#a8a29e; --line:#292524; --card:#1c1917; --accent:#f5f5f4; --on-accent:#0c0a09; --ok:#22c55e; --danger:#f87171; }} }}
* {{ box-sizing: border-box; }}
body {{ margin:0; background:var(--bg); color:var(--fg); font:16px/1.5 system-ui, -apple-system, sans-serif; }}
header {{ display:flex; justify-content:space-between; align-items:center; gap:1rem; padding:1rem 1.25rem; border-bottom:1px solid var(--line); }}
header form {{ margin:0; }}
.brand {{ font-weight:600; }}
main {{ max-width:40rem; margin:0 auto; padding:2rem 1.25rem; }}
main.center {{ min-height:100vh; display:flex; flex-direction:column; justify-content:center; align-items:center; text-align:center; }}
h1 {{ font-size:1.75rem; margin:0 0 .5rem; letter-spacing:-.01em; }}
.muted {{ color:var(--muted); }}
.button {{ display:inline-block; margin-top:1.5rem; padding:.75rem 1.25rem; border-radius:.5rem; background:var(--accent); color:var(--on-accent); text-decoration:none; font-weight:500; }}
.button:focus-visible, button:focus-visible {{ outline:2px solid var(--accent); outline-offset:2px; }}
ul {{ list-style:none; margin:1.5rem 0 0; padding:0; border:1px solid var(--line); border-radius:.75rem; background:var(--card); }}
li {{ display:flex; align-items:center; gap:.875rem; padding:.875rem 1rem; }}
li + li {{ border-top:1px solid var(--line); }}
li form {{ margin:0 0 0 auto; }}
.info {{ display:flex; flex-direction:column; min-width:0; }}
.info strong {{ font-weight:500; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }}
.info .muted {{ font-size:.875rem; }}
.dot {{ flex:none; width:.625rem; height:.625rem; border-radius:50%; background:var(--line); }}
.dot.on {{ background:var(--ok); }}
button.link {{ background:none; border:0; padding:.25rem; font:inherit; color:var(--muted); cursor:pointer; }}
button.link:hover {{ color:var(--fg); }}
button.danger:hover {{ color:var(--danger); }}
.empty {{ margin-top:1.5rem; color:var(--muted); }}
code {{ font:.875rem ui-monospace, monospace; background:var(--line); padding:.125rem .375rem; border-radius:.25rem; }}
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
