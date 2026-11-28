//! Agents' browser tools (Commutator MCP's `browser_*`): the engine hands this
//! window each call (`WatchBrowserRequests`), carried out here on the calling
//! chat's browser tabs, and takes the answer back (`BrowserReply`). A chat's
//! agent sees only its own chat's tabs.
use super::*;
use crate::browser::agent;
use gpui::WeakEntity;
use serde_json::{Value, json};
use std::time::Duration;

/// How long `browser_open` waits for its page to load.
const LOAD_WITHIN: Duration = Duration::from_secs(30);
/// How often a page that loads, or an eval that runs, is looked at again.
const POLL: Duration = Duration::from_millis(150);

impl Shell {
    /// Once a window has an engine: watches its agents' browser calls, again
    /// after the engine goes or restarts. An older engine refuses the watch;
    /// it is asked again now and then, quietly.
    pub(super) fn watch_browser_agents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.browser_agents.is_some() {
            return;
        }
        self.browser_agents = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let handle = this
                    .update(cx, |this, cx| this.state.read(cx).engine().cloned())
                    .ok()
                    .flatten();
                if let Some(handle) = handle {
                    let watch = handle
                        .client()
                        .subscribe(commutator_rpc::methods::WATCH_BROWSER_REQUESTS, json!({}))
                        .await;
                    if let Ok(mut asked) = watch {
                        while let Some(call) = asked.recv().await {
                            let (this, handle, cx) = (this.clone(), handle.clone(), cx.clone());
                            cx.spawn(async move |cx| {
                                let id = call["id"].as_u64().unwrap_or_default();
                                let chat = call["chatId"].as_str().unwrap_or_default().to_string();
                                let answer = carry_out(&this, chat, call["request"].clone(), cx).await;
                                let reply = match answer {
                                    Ok(ok) => json!({ "id": id, "ok": ok }),
                                    Err(error) => json!({ "id": id, "error": error }),
                                };
                                let _ = handle
                                    .client()
                                    .call(commutator_rpc::methods::BROWSER_REPLY, reply)
                                    .await;
                            })
                            .detach();
                        }
                    }
                }
                if this.upgrade().is_none() {
                    return;
                }
                cx.background_executor().timer(Duration::from_secs(5)).await;
            }
        }));
    }

    /// Chat `chat`'s browser tabs, in their panel's order.
    fn chat_browsers(&self, chat: &str) -> Vec<(u64, Entity<crate::browser::BrowserSurface>)> {
        self.right_tabs
            .get(chat)
            .into_iter()
            .flatten()
            .filter_map(|surface| match surface {
                RightSurface::Browser(id) => self.browsers.get(id).map(|b| (*id, b.clone())),
                _ => None,
            })
            .collect()
    }

    /// The tab a call names (`tab-3`), else the one the chat's panel shows,
    /// else its newest.
    fn agent_tab(
        &self,
        chat: &str,
        tab: Option<&str>,
        cx: &App,
    ) -> Result<(u64, Entity<crate::browser::BrowserSurface>), String> {
        let tabs = self.chat_browsers(chat);
        if let Some(name) = tab {
            let id = tab_id(name).ok_or_else(|| format!("no tab {name}: browser_tabs lists them"))?;
            return tabs
                .into_iter()
                .find(|(i, _)| *i == id)
                .ok_or_else(|| format!("no tab {name} in this chat: browser_tabs lists them"));
        }
        let shown = match self.panels.get(chat).right_active {
            RightSurface::Browser(id) => Some(id),
            _ => None,
        };
        let _ = cx;
        tabs.iter()
            .find(|(i, _)| Some(*i) == shown)
            .or(tabs.last())
            .cloned()
            .ok_or_else(|| "this chat has no browser tab: browser_open opens one".into())
    }

    fn tab_json(&self, chat: &str, id: u64, browser: &Entity<crate::browser::BrowserSurface>, cx: &App) -> Value {
        let page = &browser.read(cx).page;
        let shown = self.panels.get(chat).right_active == RightSurface::Browser(id);
        json!({
            "id": format!("tab-{id}"),
            "url": page.url,
            "title": page.title,
            "loading": page.loading,
            "error": page.error,
            "shown": shown,
        })
    }

    /// Closes chat `chat`'s tab `id`, whichever chat is on screen.
    fn close_agent_tab(&mut self, chat: &str, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let surface = RightSurface::Browser(id);
        if chat == self.panel_key(cx) {
            return self.close_right_surface(surface, window, cx);
        }
        if let Some(tabs) = self.right_tabs.get_mut(chat) {
            tabs.retain(|s| *s != surface);
        }
        if let Some(browser) = self.browsers.remove(&id) {
            browser.update(cx, |browser, cx| browser.close(cx));
        }
        self.browser_subs.remove(&id);
        let next = self.chat_browsers(chat).last().map(|(i, _)| RightSurface::Browser(*i));
        self.panels.update(chat, |p| {
            if p.right_active == surface {
                p.right_active = next.unwrap_or_default();
            }
        });
        cx.notify();
    }
}

/// `tab-3` → 3.
fn tab_id(name: &str) -> Option<u64> {
    name.strip_prefix("tab-").unwrap_or(name).parse().ok()
}

/// One call: `{op, tab?, …}` on chat `chat`'s tabs.
async fn carry_out(
    this: &WeakEntity<Shell>,
    chat: String,
    request: Value,
    cx: &mut gpui::AsyncWindowContext,
) -> Result<Value, String> {
    let op = request["op"].as_str().unwrap_or_default().to_string();
    let tab = request["tab"].as_str().map(str::to_string);
    let text = |key: &str| request[key].as_str().map(str::to_string);
    let gone = |_| "the Commutator window closed".to_string();
    match op.as_str() {
        "tabs" => this
            .update(cx, |this, cx| {
                let tabs: Vec<Value> = this
                    .chat_browsers(&chat)
                    .iter()
                    .map(|(id, b)| this.tab_json(&chat, *id, b, cx))
                    .collect();
                json!({ "tabs": tabs })
            })
            .map_err(gone),
        "open" => {
            let url = text("url").ok_or("browser_open needs a url")?;
            let url = crate::browser::model::normalize_address(&url).map_err(str::to_string)?;
            let (id, browser) = this
                .update_in(cx, |this, window, cx| {
                    let known = this.state.read(cx).chats.iter().any(|row| row.id == chat);
                    if !known {
                        return Err("this chat isn't in this Commutator window".to_string());
                    }
                    let found = match &tab {
                        Some(tab) => Some(this.agent_tab(&chat, Some(tab), cx)?),
                        None => None,
                    };
                    let (id, browser) = match found {
                        Some((id, browser)) => {
                            browser.update(cx, |b, cx| b.navigate(&url, window, cx));
                            (id, browser)
                        }
                        None => {
                            let id = this.add_browser_surface_in(chat.clone(), Some(url.clone()), window, cx);
                            let browser = this.browsers.get(&id).cloned().ok_or("the tab didn't open")?;
                            (id, browser)
                        }
                    };
                    // The person sees the agent's tab in the chat's panel.
                    this.panels.update(&chat, |p| p.changes_open = true);
                    cx.notify();
                    Ok((id, browser))
                })
                .map_err(gone)??;
            let started = std::time::Instant::now();
            loop {
                cx.background_executor().timer(POLL).await;
                let loading = browser.read_with(cx, |b, _| b.page.loading);
                if !loading || started.elapsed() > LOAD_WITHIN {
                    break;
                }
            }
            this.update(cx, |this, cx| this.tab_json(&chat, id, &browser, cx)).map_err(gone)
        }
        "close" => {
            let tab = tab.ok_or("browser_close needs a tab")?;
            this.update_in(cx, |this, window, cx| {
                let (id, _) = this.agent_tab(&chat, Some(&tab), cx)?;
                this.close_agent_tab(&chat, id, window, cx);
                Ok(json!({ "closed": tab }))
            })
            .map_err(gone)?
        }
        "screenshot" => {
            let (id, browser) = find(this, &chat, tab.as_deref(), cx)?;
            let shot = browser
                .read_with(cx, |b, _| b.screenshot())
                .ok_or("the tab has no page yet: browser_open loads one")?;
            let png = shot.await.map_err(|_| "the page went away".to_string())??;
            use base64::Engine as _;
            Ok(json!({
                "$image": { "data": base64::engine::general_purpose::STANDARD.encode(png), "mimeType": "image/png" },
                "tab": format!("tab-{id}"),
            }))
        }
        "eval" => {
            let code = text("code").ok_or("browser_eval needs code")?;
            let body = request["body"].as_bool().unwrap_or(false);
            let within = Duration::from_secs(request["timeout_secs"].as_u64().unwrap_or(30).clamp(1, 55));
            let (_, browser) = find(this, &chat, tab.as_deref(), cx)?;
            static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let started = run_js(&browser, &agent::eval_script(run, &code, body), cx).await?;
            if agent::js_string(&started).as_deref() != Some("started") {
                return Err("the code doesn't parse as an expression: pass `body: true` for statements that `return`".into());
            }
            let since = std::time::Instant::now();
            loop {
                cx.background_executor().timer(POLL).await;
                let polled = run_js(&browser, &agent::poll_script(run), cx).await?;
                let polled: Value = agent::js_string(&polled)
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default();
                if polled["gone"].as_bool() == Some(true) {
                    return Err("the page navigated before the code finished".into());
                }
                if let Some(text) = polled["text"].as_str() {
                    return match polled["ok"].as_bool() {
                        Some(true) => Ok(json!(text)),
                        _ => Err(text.to_string()),
                    };
                }
                if since.elapsed() > within {
                    return Err(format!("the code didn't finish within {}s", within.as_secs()));
                }
            }
        }
        "console" => {
            let (_, browser) = find(this, &chat, tab.as_deref(), cx)?;
            let clear = request["clear"].as_bool().unwrap_or(false);
            text_of(run_js(&browser, &agent::console_script(clear), cx).await?)
        }
        "network" => {
            let (_, browser) = find(this, &chat, tab.as_deref(), cx)?;
            let clear = request["clear"].as_bool().unwrap_or(false);
            let script = agent::network_script(request["request"].as_u64(), clear);
            text_of(run_js(&browser, &script, cx).await?)
        }
        "snapshot" | "act" => {
            let call = tools_call(&op, &request)?;
            let (_, browser) = find(this, &chat, tab.as_deref(), cx)?;
            let answer = run_js(&browser, &agent::tools_call(&call), cx).await?;
            let answer: Value = agent::js_string(&answer)
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let said = answer["text"].as_str().unwrap_or_default().to_string();
            match answer["ok"].as_bool() {
                Some(true) => Ok(json!(said)),
                _ if said.is_empty() => Err("the page didn't answer".into()),
                _ => Err(said),
            }
        }
        other => Err(format!("unknown browser call: {other}")),
    }
}

/// The page's `T` call for a snapshot or an action.
fn tools_call(op: &str, request: &Value) -> Result<String, String> {
    let target = request["target"].as_str();
    let text = request["text"].as_str();
    let need = |what: Option<&str>, name: &str| {
        what.map(agent::js).ok_or_else(|| format!("{name} is needed here"))
    };
    if op == "snapshot" {
        return Ok(format!("T.snapshot({})", target.map_or("undefined".into(), agent::js)));
    }
    let action = request["action"].as_str().unwrap_or_default();
    Ok(match action {
        "click" | "dblclick" | "hover" | "focus" => format!("T.{action}({})", need(target, "target")?),
        "fill" | "type" => format!("T.{action}({}, {})", need(target, "target")?, need(text, "text")?),
        "press" => match target {
            Some(t) => format!("(T.focus({}), T.press({}))", agent::js(t), need(text, "text")?),
            None => format!("T.press({})", need(text, "text")?),
        },
        "select" => {
            let values = serde_json::to_string(&request["values"]).unwrap_or_else(|_| "[]".into());
            format!("T.select({}, {values})", need(target, "target")?)
        }
        "check" | "uncheck" => format!("T.check({}, {})", need(target, "target")?, action == "check"),
        "scroll" => format!("T.scroll({})", agent::js(target.unwrap_or("down"))),
        other => return Err(format!("unknown action {other:?}")),
    })
}

fn find(
    this: &WeakEntity<Shell>,
    chat: &str,
    tab: Option<&str>,
    cx: &mut gpui::AsyncWindowContext,
) -> Result<(u64, Entity<crate::browser::BrowserSurface>), String> {
    this.update(cx, |this, cx| this.agent_tab(chat, tab, cx))
        .map_err(|_| "the Commutator window closed".to_string())?
}

async fn run_js(
    browser: &Entity<crate::browser::BrowserSurface>,
    script: &str,
    cx: &mut gpui::AsyncWindowContext,
) -> Result<String, String> {
    let answer = browser
        .read_with(cx, |b, _| b.agent_js(script))
        .ok_or("the tab has no page yet: browser_open loads one")?;
    answer.await.map_err(|_| "the page went away".to_string())
}

/// A script's string answer, as the tool's text.
fn text_of(raw: String) -> Result<Value, String> {
    Ok(json!(agent::js_string(&raw).unwrap_or(raw)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_become_the_pages_calls() {
        let call = |v: Value| tools_call("act", &v);
        assert_eq!(call(json!({"action": "click", "target": "e3"})).unwrap(), r#"T.click("e3")"#);
        assert_eq!(call(json!({"action": "fill", "target": "#q", "text": "hi"})).unwrap(), r##"T.fill("#q", "hi")"##);
        assert_eq!(call(json!({"action": "press", "text": "Enter"})).unwrap(), r#"T.press("Enter")"#);
        assert_eq!(call(json!({"action": "uncheck", "target": "e1"})).unwrap(), r#"T.check("e1", false)"#);
        assert_eq!(call(json!({"action": "scroll"})).unwrap(), r#"T.scroll("down")"#);
        assert_eq!(call(json!({"action": "select", "target": "e2", "values": ["M"]})).unwrap(), r#"T.select("e2", ["M"])"#);
        assert!(call(json!({"action": "fill", "target": "e3"})).is_err());
        assert!(call(json!({"action": "fly"})).is_err());
        assert_eq!(tools_call("snapshot", &json!({})).unwrap(), "T.snapshot(undefined)");
        assert_eq!((tab_id("tab-3"), tab_id("7"), tab_id("x")), (Some(3), Some(7), None));
    }
}
