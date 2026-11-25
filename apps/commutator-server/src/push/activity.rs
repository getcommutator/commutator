//! Live Activities: one card per phone with what the account's agents are
//! doing, as in T3 Code. The phone starts the card itself when it sends work
//! and registers it here; from then on every machine's chat phases (posted by
//! the engine, `desktop_notify`) keep it current, and once nothing has moved
//! for a while the server ends it.
//!
//! - `POST   /activity {chatId, title, project?, phase}` (a machine's bearer):
//!   `working`, `input`, `failed`, `done`, or `idle` (off the card).
//! - `PUT    /activity/target {platform, token, environment}` (a phone): the
//!   card's ActivityKit push token (`apns`, hex), or the phone's FCM token
//!   (`fcm`: Android shows it as an ongoing notification).
//! - `DELETE /activity/target`: the card went away on the phone.
//!
//! iOS gets `apns-push-type: liveactivity` pushes whose `content-state` is
//! [`State`]; Android an FCM data message `{type: "activity", state: <State as
//! JSON>}`, or `{type: "activity_end"}`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State as Axum};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::{App, Kind, TokenQuery, fail, lock, now};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS activity_targets (
    device_id TEXT PRIMARY KEY,
    platform TEXT NOT NULL,
    token TEXT NOT NULL,
    environment TEXT NOT NULL
);";

/// How long a finished (or failed) chat stays on the card, and how long a
/// chat that stopped reporting stays there at all.
const FINISHED_FOR: i64 = 15 * 60;
const WORKING_FOR: i64 = 2 * 60 * 60;
const WAITING_FOR: i64 = 24 * 60 * 60;
/// Rows a card shows (a push carries at most 4 KB).
const ROWS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    title: String,
    project: Option<String>,
    phase: String,
    updated: i64,
}

/// Every account's chats on its cards: user → (machine, chat) → row. In
/// memory: after a restart the next phase change fills it again.
#[derive(Default)]
pub(crate) struct Board(Mutex<HashMap<String, HashMap<(String, String), Row>>>);

/// The card's content (ActivityKit's `content-state`; the Android payload).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// Chats working or waiting on you.
    pub active: usize,
    /// Chats waiting on you.
    pub attention: usize,
    pub rows: Vec<StateRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateRow {
    pub chat_id: String,
    pub title: String,
    pub project: Option<String>,
    pub phase: String,
}

fn rank(phase: &str) -> u8 {
    match phase {
        "input" => 0,
        "failed" => 1,
        "working" => 2,
        _ => 3,
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

impl Board {
    /// Drop rows past their time; whether any went.
    fn prune(rows: &mut HashMap<(String, String), Row>, at: i64) -> bool {
        let before = rows.len();
        rows.retain(|_, row| {
            let keep = match row.phase.as_str() {
                "working" => WORKING_FOR,
                "input" => WAITING_FOR,
                _ => FINISHED_FOR,
            };
            at - row.updated < keep
        });
        rows.len() != before
    }

    fn state(rows: &HashMap<(String, String), Row>) -> State {
        let mut all: Vec<(&(String, String), &Row)> = rows.iter().collect();
        all.sort_by(|(_, a), (_, b)| rank(&a.phase).cmp(&rank(&b.phase)).then(b.updated.cmp(&a.updated)));
        State {
            active: rows.values().filter(|r| matches!(r.phase.as_str(), "working" | "input")).count(),
            attention: rows.values().filter(|r| r.phase == "input").count(),
            rows: all
                .into_iter()
                .take(ROWS)
                .map(|((_, chat), row)| StateRow {
                    chat_id: chat.clone(),
                    title: clip(&row.title, 60),
                    project: row.project.as_deref().map(|p| clip(p, 40)),
                    phase: row.phase.clone(),
                })
                .collect(),
        }
    }

    /// `user`'s card now; `None` when it has nothing left to show.
    fn state_of(&self, user: &str) -> Option<State> {
        let mut board = lock(&self.0);
        let rows = board.get_mut(user)?;
        Self::prune(rows, now());
        (!rows.is_empty()).then(|| Self::state(rows))
    }

    /// Apply one machine's chat phase; whether the card changed.
    fn set(&self, user: &str, machine: &str, row: &Phase) -> bool {
        let mut board = lock(&self.0);
        let rows = board.entry(user.to_owned()).or_default();
        let key = (machine.to_owned(), row.chat_id.clone());
        if row.phase == "idle" {
            return rows.remove(&key).is_some();
        }
        let next = Row {
            title: row.title.clone(),
            project: row.project.clone(),
            phase: row.phase.clone(),
            updated: now(),
        };
        let same = rows.get(&key).is_some_and(|r| r.phase == next.phase && r.title == next.title);
        rows.insert(key, next);
        !same
    }

    /// Prune every account; the ones whose card changed (with `None` for a
    /// card with nothing left: end it).
    fn sweep(&self, at: i64) -> Vec<(String, Option<State>)> {
        let mut board = lock(&self.0);
        let mut changed = Vec::new();
        board.retain(|user, rows| {
            if Self::prune(rows, at) {
                changed.push((user.clone(), (!rows.is_empty()).then(|| Self::state(rows))));
            }
            !rows.is_empty()
        });
        changed
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Phase {
    chat_id: String,
    title: String,
    project: Option<String>,
    phase: String,
}

#[derive(Deserialize)]
pub(crate) struct Target {
    platform: String,
    token: String,
    #[serde(default)]
    environment: String,
}

/// A registered card: (device, platform, token, environment).
pub type ActivityTarget = (String, String, String, String);

impl App {
    pub fn activity_targets_of(&self, user: &str) -> rusqlite::Result<Vec<ActivityTarget>> {
        let db = lock(&self.db);
        let mut stmt = db.prepare(
            "SELECT a.device_id, a.platform, a.token, a.environment FROM activity_targets a
             JOIN devices d ON d.id = a.device_id WHERE d.user_id = ?1",
        )?;
        stmt.query_map([user], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .collect()
    }

    /// `user`'s card now, as phones would get it.
    pub fn activities_of(&self, user: &str) -> Option<State> {
        self.activities.state_of(user)
    }

    fn remove_activity_target(&self, device: &str) {
        let _ = lock(&self.db).execute("DELETE FROM activity_targets WHERE device_id = ?1", [device]);
    }

    /// Bring `targets` up to date with `state`, or end their cards (`None`).
    /// In the background; best effort.
    fn deliver(self: &Arc<Self>, targets: Vec<ActivityTarget>, state: Option<State>) {
        if targets.is_empty() {
            return;
        }
        let app = self.clone();
        tokio::spawn(async move {
            let at = now();
            let apns_payload = match &state {
                Some(state) => serde_json::json!({ "aps": {
                    "timestamp": at,
                    "event": "update",
                    "content-state": state,
                    // Past this, a card that heard nothing more says so.
                    "stale-date": at + WORKING_FOR,
                }}),
                None => serde_json::json!({ "aps": {
                    "timestamp": at,
                    "event": "end",
                    "content-state": State { active: 0, attention: 0, rows: Vec::new() },
                    "dismissal-date": at,
                }}),
            };
            let fcm_payload = match &state {
                Some(state) => serde_json::json!({
                    "type": "activity",
                    "state": serde_json::to_string(state).unwrap_or_default(),
                }),
                None => serde_json::json!({ "type": "activity_end" }),
            };
            for (device, platform, token, environment) in targets {
                let sent = match (platform.as_str(), &app.push.apns, &app.push.fcm) {
                    ("apns", Some(apns), _) => apns.send_activity(&token, &environment, &apns_payload).await,
                    ("fcm", _, Some(fcm)) => fcm.send_data(&token, &fcm_payload).await,
                    _ => continue,
                };
                match sent {
                    // An ended card is gone for good: so is its target.
                    Ok(true) if state.is_none() => app.remove_activity_target(&device),
                    Ok(true) => {}
                    Ok(false) => app.remove_activity_target(&device),
                    Err(err) => tracing::warn!(%device, error = %err, "activity push failed"),
                }
            }
        });
    }

    /// Every minute: cards whose rows ran out change or end.
    pub fn sweep_activities(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                for (user, state) in self.activities.sweep(now()) {
                    if let Ok(targets) = self.activity_targets_of(&user) {
                        self.deliver(targets, state);
                    }
                }
            }
        });
    }
}

pub(crate) async fn post_activity(
    Axum(app): Axum<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
    Json(phase): Json<Phase>,
) -> Response {
    let device = match app.authenticate(&headers, q.token.as_deref()) {
        Ok(device) => device,
        Err(status) => return status.into_response(),
    };
    if device.kind != Kind::Machine {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !matches!(phase.phase.as_str(), "working" | "input" | "failed" | "done" | "idle") || phase.chat_id.len() > 200 {
        return fail(StatusCode::BAD_REQUEST, "unknown phase");
    }
    if app.activities.set(&device.user, &device.id, &phase) {
        let state = app.activities.state_of(&device.user);
        match app.activity_targets_of(&device.user) {
            Ok(targets) => app.deliver(targets, state),
            Err(err) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
        }
    }
    StatusCode::ACCEPTED.into_response()
}

pub(crate) async fn put_target(
    Axum(app): Axum<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
    Json(target): Json<Target>,
) -> Response {
    let device = match app.authenticate(&headers, q.token.as_deref()) {
        Ok(device) => device,
        Err(status) => return status.into_response(),
    };
    let token = target.token.as_bytes();
    let ok = match target.platform.as_str() {
        "apns" => {
            !token.is_empty()
                && token.len() <= 400
                && token.iter().all(u8::is_ascii_hexdigit)
                && matches!(target.environment.as_str(), "sandbox" | "production")
        }
        "fcm" => {
            !token.is_empty()
                && token.len() <= 4096
                && token.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':'))
        }
        _ => false,
    };
    if !ok {
        return fail(StatusCode::BAD_REQUEST, "not a push token");
    }
    let saved = lock(&app.db).execute(
        "INSERT INTO activity_targets (device_id, platform, token, environment) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(device_id) DO UPDATE SET platform = excluded.platform,
             token = excluded.token, environment = excluded.environment",
        rusqlite::params![device.id, target.platform, target.token, target.environment],
    );
    if let Err(err) = saved {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string());
    }
    // What the other chats are doing, at once: the phone only knew its own.
    if let Some(state) = app.activities.state_of(&device.user) {
        app.deliver(vec![(device.id, target.platform, target.token, target.environment)], Some(state));
    }
    StatusCode::NO_CONTENT.into_response()
}

pub(crate) async fn delete_target(
    Axum(app): Axum<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> Response {
    match app.authenticate(&headers, q.token.as_deref()) {
        Ok(device) => {
            app.remove_activity_target(&device.id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(status) => status.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phase(chat: &str, phase: &str) -> Phase {
        Phase { chat_id: chat.into(), title: format!("Chat {chat}"), project: None, phase: phase.into() }
    }

    #[test]
    fn the_card_puts_what_needs_you_first_and_lets_finished_chats_go() {
        let board = Board::default();
        assert!(board.set("me", "mac", &phase("a", "working")));
        assert!(!board.set("me", "mac", &phase("a", "working")), "nothing new");
        assert!(board.set("me", "mac", &phase("b", "input")));
        assert!(board.set("me", "linux", &phase("c", "done")));
        let state = board.state_of("me").unwrap();
        assert_eq!((state.active, state.attention), (2, 1));
        let order: Vec<&str> = state.rows.iter().map(|r| r.chat_id.as_str()).collect();
        assert_eq!(order, ["b", "a", "c"]);

        // Interrupted: off the card.
        assert!(board.set("me", "mac", &phase("a", "idle")));
        // Fifteen minutes on, the finished chat goes; the question stays.
        let later = now() + FINISHED_FOR + 1;
        assert_eq!(board.sweep(later).len(), 1);
        assert!(board.sweep(later).is_empty(), "already swept");
        let state = board.state_of("me").unwrap();
        assert_eq!(state.rows.len(), 1);
        // A day on, nothing: the card ends.
        assert_eq!(board.sweep(now() + WAITING_FOR + 1), vec![("me".to_owned(), None)]);
        assert!(board.state_of("me").is_none());
    }

    #[test]
    fn long_titles_fit_the_push() {
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(clip("abc", 4), "abc");
    }
}
