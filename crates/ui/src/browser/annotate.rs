//! Annotations: the toolbar's button injects the picker (`annotate.js`),
//! polled for picks only while it runs. Each pick goes to the chat beside the
//! page as an `Annotation N` chip; sending the message hands the model the
//! element as a labelled block (`commutator_proto::annotation`).
use super::{BrowserEvent, BrowserSurface};
use commutator_proto::annotation::BrowserAnnotation;
use gpui::{Context, Window};
use serde::Deserialize;
use std::hash::BuildHasher;
use std::time::Duration;

const SCRIPT: &str = include_str!("annotate.js");
/// How often an active pick is polled. Nothing polls otherwise.
const POLL: Duration = Duration::from_millis(100);
/// One session's picks: the page's script answers the polls, so a hostile
/// page could otherwise stream picks into the composer without a click.
const MAX_PICKS: usize = 24;
/// Unreadable polls (about two seconds' worth) before the session gives up.
const MAX_MISSES: u32 = 20;

/// While the user picks on the page. Dropping it stops the poll.
pub(super) struct Session {
    nonce: u32,
    picks: usize,
    _poll: gpui::Task<()>,
}

/// One pick as a poll returns it: the annotation and the page's marker for it.
#[derive(Debug, Deserialize)]
struct Pick {
    #[serde(flatten)]
    annotation: BrowserAnnotation,
    #[serde(default)]
    marker: u32,
}

/// What a poll said: the picker's state (`active` while picking) and the
/// picks since the last poll.
#[derive(Debug, Deserialize)]
struct Poll {
    state: String,
    #[serde(default)]
    picks: Vec<Pick>,
}

impl Poll {
    /// From a script's result, a JSON string holding the picker's JSON.
    fn parse(result: &str) -> Option<Poll> {
        serde_json::from_str(&serde_json::from_str::<String>(result).ok()?).ok()
    }
}

/// The overlay's palette and type, so the page chrome reads as Commutator's.
fn palette(cx: &gpui::App) -> serde_json::Value {
    let theme = crate::theme::Theme::of(cx);
    let css = |color: gpui::Hsla| {
        let rgba: gpui::Rgba = color.into();
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}{:02x}", byte(rgba.r), byte(rgba.g), byte(rgba.b), byte(rgba.a))
    };
    serde_json::json!({
        "accent": css(theme.accent),
        "surface": css(theme.surface),
        "border": css(theme.border),
        "text": css(theme.text),
        "muted": css(theme.text_muted),
        "faint": css(theme.text_faint),
        "chip": css(crate::theme::wash(0.09)),
        "dark": theme.appearance.is_dark(),
    })
}

impl BrowserSurface {
    pub fn annotating(&self) -> bool {
        self.annotations.is_some()
    }

    /// A loaded page, where the picker can run.
    pub(super) fn can_annotate(&self) -> bool {
        cfg!(target_os = "macos")
            && self.page.error.is_none()
            && self.page.url.as_deref().is_some_and(super::model::allowed_navigation)
    }

    pub(super) fn toggle_annotation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.annotating() {
            true => self.stop_annotation(),
            false => self.start_annotation(window, cx),
        }
        cx.notify();
    }

    fn start_annotation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_annotate() {
            return;
        }
        // The page takes the keyboard, for the picker's Esc.
        window.focus(&self.focus, cx);
        // Below 2^32, so the page's JavaScript numbers hold it exactly.
        let nonce = (std::collections::hash_map::RandomState::new().hash_one(0) as u32).max(1);
        self.eval(&format!(
            "{SCRIPT}\n;window.__commutatorAnnotator.start({nonce}, {}); 'started'",
            palette(cx)
        ));
        let poll = cx.spawn(async move |this, cx| {
            let mut misses = 0;
            loop {
                cx.background_executor().timer(POLL).await;
                let Ok(Some(answer)) = this.update(cx, |this, _| this.poll(nonce)) else {
                    return;
                };
                let result = answer.await.unwrap_or_default();
                let Some(poll) = Poll::parse(&result) else {
                    // A page briefly unreachable; only a run of these ends it.
                    misses += 1;
                    if misses >= MAX_MISSES {
                        let _ = this.update(cx, |this, cx| {
                            this.stop_annotation();
                            cx.notify();
                        });
                        return;
                    }
                    continue;
                };
                misses = 0;
                let going = this.update(cx, |this, cx| this.polled(nonce, poll, cx));
                if !matches!(going, Ok(true)) {
                    return;
                }
            }
        });
        self.annotations = Some(Session { nonce, picks: 0, _poll: poll });
    }

    pub(super) fn stop_annotation(&mut self) {
        if self.annotations.take().is_some() {
            self.eval("(() => { window.__commutatorAnnotator?.stop(); return 'stopped'; })()");
        }
    }

    /// Asks the picker for its picks; `None` once the session is over.
    fn poll(&self, nonce: u32) -> Option<futures::channel::oneshot::Receiver<String>> {
        self.annotations.as_ref().filter(|s| s.nonce == nonce)?;
        let (tx, rx) = futures::channel::oneshot::channel();
        // WebKit's callback is an `Fn`: it hands the one answer over once.
        let tx = std::sync::Mutex::new(Some(tx));
        let take = format!(
            "(() => {{ const a = window.__commutatorAnnotator; return a ? a.take({nonce}) : '{{\"state\":\"gone\"}}'; }})()"
        );
        #[cfg(target_os = "macos")]
        self.native
            .as_ref()?
            .handle()
            .borrow()
            .eval_with_callback(&take, move |result| {
                if let Some(tx) = tx.lock().ok().and_then(|mut tx| tx.take()) {
                    let _ = tx.send(result);
                }
            });
        #[cfg(not(target_os = "macos"))]
        drop((take, tx));
        Some(rx)
    }

    /// The picks go to the chat; a `gone` state means the picker left the
    /// page (it navigated), which ends the session.
    fn polled(&mut self, nonce: u32, poll: Poll, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.annotations.as_mut().filter(|s| s.nonce == nonce) else {
            return false;
        };
        let room = MAX_PICKS.saturating_sub(session.picks);
        let picks: Vec<Pick> = poll.picks.into_iter().take(room).collect();
        session.picks += picks.len();
        let finished = poll.state != "active" || session.picks >= MAX_PICKS;
        for pick in picks {
            cx.emit(BrowserEvent::Annotated {
                annotation: pick.annotation.bounded(),
                marker: pick.marker,
            });
        }
        if finished {
            // `stop` tidies the overlay when the cap, not the page, ended it.
            self.stop_annotation();
            cx.notify();
        }
        !finished
    }

    /// Numbers the page's marker for a pick as its chip in the composer.
    pub fn label_annotation(&self, marker: u32, index: u32) {
        self.eval(&format!(
            "(() => {{ window.__commutatorAnnotator?.label({marker}, {index}); return 'ok'; }})()"
        ));
    }

    fn eval(&self, script: &str) {
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            native.handle().borrow().eval_with_callback(script, |_| {});
        }
        #[cfg(not(target_os = "macos"))]
        let _ = script;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poll_reads_the_pickers_json_string_and_nothing_else() {
        let inner = r##"{"state":"active","picks":[{"url":"u","element":"a#x","selector":"#x","rect":[1,2,3,4],"viewport":[5,6],"marker":7,"area":true,"inside":["#y"]}]}"##;
        let poll = Poll::parse(&serde_json::to_string(inner).unwrap()).unwrap();
        assert_eq!(poll.state, "active");
        let pick = &poll.picks[0];
        assert_eq!((pick.marker, pick.annotation.selector.as_str()), (7, "#x"));
        assert!(pick.annotation.area && pick.annotation.inside == ["#y"]);
        assert!(Poll::parse("").is_none());
        assert!(Poll::parse("\"ok\"").is_none());
        assert!(Poll::parse(inner).is_none());
    }
}
