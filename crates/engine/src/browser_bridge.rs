//! Agents' calls on the app's browser tabs. The browser lives in the app's
//! window, not here: the app watches the requests ([`BrowserBridge::watch`]),
//! carries each out on the chat's tabs and answers ([`BrowserBridge::reply`]);
//! an agent's `BrowserCall` waits for that answer.
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Long enough for a page to load or a script to settle; the app answers
/// sooner for anything that doesn't wait on the page.
const ANSWER_WITHIN: Duration = Duration::from_secs(60);

#[derive(Clone, Default)]
pub struct BrowserBridge(Arc<Mutex<Inner>>);

#[derive(Default)]
struct Inner {
    next: u64,
    /// The app windows watching, newest last: the newest answers.
    viewers: Vec<(u64, mpsc::UnboundedSender<Value>)>,
    pending: HashMap<u64, oneshot::Sender<Result<Value, String>>>,
}

/// Takes its viewer out once the app's stream is dropped.
struct Viewer(BrowserBridge, u64);

impl Drop for Viewer {
    fn drop(&mut self) {
        self.0.lock().viewers.retain(|(id, _)| *id != self.1);
    }
}

impl BrowserBridge {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The app's stream of `{id, chatId, request}`.
    pub fn watch(&self) -> futures::stream::BoxStream<'static, Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        let id = {
            let mut inner = self.lock();
            inner.next += 1;
            let id = inner.next;
            inner.viewers.push((id, tx));
            id
        };
        let viewer = Viewer(self.clone(), id);
        futures::stream::unfold((rx, viewer), |(mut rx, viewer)| async move {
            rx.recv().await.map(|item| (item, (rx, viewer)))
        })
        .boxed()
    }

    /// Hands `request` for chat `chat_id`'s tabs to the newest app window and
    /// waits for its answer.
    pub async fn call(&self, chat_id: &str, request: Value) -> Result<Value, String> {
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut inner = self.lock();
            inner.viewers.retain(|(_, viewer)| !viewer.is_closed());
            let Some((_, viewer)) = inner.viewers.last().cloned() else {
                return Err("No Commutator window is open on this device to hold the browser.".into());
            };
            inner.next += 1;
            let id = inner.next;
            inner.pending.insert(id, tx);
            let _ = viewer.send(json!({ "id": id, "chatId": chat_id, "request": request }));
            id
        };
        let answer = tokio::time::timeout(ANSWER_WITHIN, rx).await;
        self.lock().pending.remove(&id);
        match answer {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err("The app went away before it answered.".into()),
            Err(_) => Err(format!("The browser didn't answer within {}s.", ANSWER_WITHIN.as_secs())),
        }
    }

    /// The app's answer to request `id`; a late one finds no one waiting.
    pub fn reply(&self, id: u64, answer: Result<Value, String>) {
        if let Some(waiting) = self.lock().pending.remove(&id) {
            let _ = waiting.send(answer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_call_goes_to_the_newest_window_and_waits_for_its_answer() {
        let bridge = BrowserBridge::default();
        assert!(bridge.call("c", json!({})).await.unwrap_err().contains("No Commutator window"));
        let _old = bridge.watch();
        let mut new = bridge.watch();
        let call = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.call("c", json!({ "op": "tabs" })).await }
        });
        let asked = new.next().await.unwrap();
        assert_eq!((asked["chatId"].as_str(), asked["request"]["op"].as_str()), (Some("c"), Some("tabs")));
        bridge.reply(asked["id"].as_u64().unwrap(), Ok(json!(["t1"])));
        assert_eq!(call.await.unwrap(), Ok(json!(["t1"])));
        // A window closed is no longer asked.
        drop(new);
        drop(_old);
        assert!(bridge.call("c", json!({})).await.is_err());
    }
}
