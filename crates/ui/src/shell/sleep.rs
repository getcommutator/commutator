//! Sleep: put a chat away until a time (the phones' Activity Sleep). The
//! engine keeps it (`setChatSleep`, synced to every device) and wakes it early
//! when it asks for the person; the sidebar moves it to the Asleep shelf
//! meanwhile. Here: the chat menu's Sleep submenu (or Wake) and its times.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc};
use gpui::{Context, IntoElement, ParentElement, SharedString, Styled, div, prelude::*, px};

use super::{ChatMenuPage, Shell};
use crate::icons::{self, icon};
use crate::popover;
use crate::theme::Theme;

/// One of Sleep's times: its name, when it wakes as the menu says it, and when.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SleepPreset {
    pub id: &'static str,
    pub title: &'static str,
    pub detail: String,
    pub until: DateTime<Utc>,
}

/// Sleep's choices, soonest first, one a minute: in an hour, in three, this
/// evening (6 PM, while it's still ahead), tomorrow at 9, next Monday at 9.
/// Each says when it wakes: its time, and its day where its name doesn't.
pub(super) fn sleep_presets<Tz: TimeZone>(now: DateTime<Tz>) -> Vec<SleepPreset>
where
    Tz::Offset: std::fmt::Display,
{
    let tz = now.timezone();
    let at = |days: i64, hour: u32| {
        let day = now.date_naive() + Duration::days(days);
        tz.from_local_datetime(&day.and_time(NaiveTime::from_hms_opt(hour, 0, 0)?))
            .earliest()
    };
    let mut out: Vec<SleepPreset> = Vec::new();
    let mut add = |id, title, until: Option<DateTime<Tz>>, names_day: bool| {
        let Some(until) = until.filter(|until| *until > now) else {
            return;
        };
        let time = until.format("%-I:%M %p").to_string();
        let detail = if names_day || until.date_naive() == now.date_naive() {
            time
        } else {
            format!("{} {time}", until.format("%a"))
        };
        out.push(SleepPreset { id, title, detail, until: until.with_timezone(&Utc) });
    };
    add("hour", "In 1 Hour", Some(now.clone() + Duration::hours(1)), false);
    add("hours", "In 3 Hours", Some(now.clone() + Duration::hours(3)), false);
    add("evening", "This Evening", at(0, 18), true);
    add("tomorrow", "Tomorrow", at(1, 9), true);
    // Monday: a week on from a Monday, the next one from any other day.
    let to_monday = (8 - i64::from(now.weekday().number_from_monday())) % 7;
    add("week", "Next Week", at(if to_monday == 0 { 7 } else { to_monday }, 9), false);
    out.sort_by_key(|preset| preset.until);
    out.dedup_by_key(|preset| preset.until.timestamp() / 60);
    out
}

/// How soon `until` is, as an Asleep row's corner says it: "45m", "3h", "2d".
pub(super) fn wakes_in(until: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let left = (until - now).num_seconds().max(0);
    let up = |unit: i64| (left + unit - 1) / unit;
    if left < 3600 {
        format!("{}m", up(60).max(1))
    } else if left < 86_400 {
        format!("{}h", up(3600))
    } else {
        format!("{}d", up(86_400))
    }
}

impl Shell {
    /// Put `chat_id` to sleep until `until`, or wake it (`None`).
    pub(super) fn set_chat_sleep(
        &mut self,
        chat_id: String,
        until: Option<DateTime<Utc>>,
        cx: &mut Context<Self>,
    ) {
        self.close_chat_menu(cx);
        self.mutate(
            serde_json::json!({
                "op": "setChatSleep",
                "chatId": chat_id,
                "until": until.map(|until| until.timestamp_millis()),
            }),
            cx,
        );
        cx.notify();
    }

    /// The chat menu's Sleep row (a submenu of times), or Wake for a chat
    /// asleep.
    pub(super) fn render_chat_sleep_row(
        &mut self,
        chat_id: &str,
        open: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let asleep = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .and_then(|chat| crate::state::asleep_until(chat, Utc::now()))
            .is_some();
        if asleep {
            let id = chat_id.to_owned();
            return popover::menu_row(theme, false, format!("chat-menu-wake-{chat_id}"))
                .id("chat-menu-wake")
                .debug_selector(|| "chat-menu-wake".into())
                .on_click(cx.listener(move |this, _, _, cx| this.set_chat_sleep(id.clone(), None, cx)))
                .child(icon(icons::SUN).size(px(16.0)).text_color(theme.text_muted))
                .child(SharedString::from("Wake"))
                .into_any_element();
        }
        let submenu = open.then(|| self.render_chat_sleep_submenu(chat_id, theme, cx));
        self.render_chat_submenu_trigger(
            ChatMenuPage::Sleep,
            "chat-menu-sleep",
            icons::MOON,
            "Sleep",
            chat_id,
            open,
            submenu,
            theme,
            cx,
        )
    }

    /// Sleep's times, each saying when it wakes.
    fn render_chat_sleep_submenu(
        &mut self,
        chat_id: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut card = popover::popover_card(theme)
            .w(px(232.0))
            .relative()
            .flex()
            .flex_col()
            .gap(px(2.0));
        for preset in sleep_presets(Local::now()) {
            let id = chat_id.to_owned();
            let until = preset.until;
            let selector = format!("chat-sleep-{}", preset.id);
            card = card.child(
                popover::menu_row(theme, false, format!("{selector}-{chat_id}"))
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_chat_sleep(id.clone(), Some(until), cx)
                    }))
                    .child(div().flex_1().child(SharedString::from(preset.title)))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(preset.detail)),
                    ),
            );
        }
        self.track_chat_submenu_bounds(card, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<FixedOffset> {
        // 2026-11-23 is a Monday.
        FixedOffset::east_opt(3 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 11, day, hour, minute, 0)
            .unwrap()
    }

    fn ids(presets: &[SleepPreset]) -> Vec<&'static str> {
        presets.iter().map(|p| p.id).collect()
    }

    #[test]
    fn presets_run_soonest_first_and_say_when_they_wake() {
        let presets = sleep_presets(at(25, 14, 30)); // Wednesday afternoon
        assert_eq!(ids(&presets), ["hour", "hours", "evening", "tomorrow", "week"]);
        let detail: Vec<&str> = presets.iter().map(|p| p.detail.as_str()).collect();
        assert_eq!(detail, ["3:30 PM", "5:30 PM", "6:00 PM", "9:00 AM", "Mon 9:00 AM"]);
        assert_eq!(presets[4].until, at(30, 9, 0).with_timezone(&Utc));
    }

    #[test]
    fn evening_drops_once_past_and_monday_skips_a_week() {
        let presets = sleep_presets(at(23, 19, 0)); // Monday evening
        assert_eq!(ids(&presets), ["hour", "hours", "tomorrow", "week"]);
        assert_eq!(presets[1].detail, "10:00 PM");
        assert_eq!(presets[3].until, at(30, 9, 0).with_timezone(&Utc));
        // Late at night the three hours cross into tomorrow and say so.
        let late = sleep_presets(at(25, 23, 0));
        assert_eq!(late[1].detail, "Thu 2:00 AM");
    }

    #[test]
    fn presets_on_the_same_minute_collapse() {
        // 3 PM: in three hours is this evening.
        let presets = sleep_presets(at(25, 15, 0));
        assert_eq!(ids(&presets), ["hour", "hours", "tomorrow", "week"]);
    }

    #[test]
    fn wakes_in_rounds_up() {
        let now = Utc::now();
        assert_eq!(wakes_in(now + Duration::seconds(10), now), "1m");
        assert_eq!(wakes_in(now + Duration::minutes(45), now), "45m");
        assert_eq!(wakes_in(now + Duration::minutes(61), now), "2h");
        assert_eq!(wakes_in(now + Duration::hours(30), now), "2d");
        assert_eq!(wakes_in(now - Duration::hours(1), now), "1m");
    }
}
