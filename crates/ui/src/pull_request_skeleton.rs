//! Loading placeholders shaped like the content they stand in for, so the
//! pull request view keeps their layout while GitHub answers. Bars pulse on the
//! shared loader clock (static under reduced motion).
use crate::theme::Theme;
use gpui::{AnyElement, App, EntityId, div, prelude::*, px, relative};

/// One pulsing placeholder bar; `index` staggers its phase down a column.
pub(crate) fn bar(
    width: impl Into<gpui::Length>,
    height: f32,
    index: usize,
    view: EntityId,
    theme: &Theme,
    cx: &mut App,
) -> gpui::Div {
    let delta = crate::motion::pulse_delta(&crate::motion::COMMUTATOR_PULSE, view, cx);
    let phase = crate::motion::staggered_phase(delta, index, 0.08);
    div()
        .w(width.into())
        // A fixed-width bar never overflows a narrow pane.
        .max_w_full()
        .h(px(height))
        .flex_none()
        .rounded(px(height / 2.0))
        .bg(theme.ink(0.07))
        .opacity(0.45 + 0.4 * crate::motion::pulse_wave(phase))
}

/// Deterministic width ladder so rows read as varied text, not slabs.
const WIDTHS: [f32; 6] = [0.62, 0.48, 0.71, 0.55, 0.39, 0.66];

/// Diff lines inside the stream card, under a file header.
pub(crate) fn diff(split: bool, view: EntityId, theme: &Theme, cx: &mut App) -> AnyElement {
    let header = div()
        .h(px(crate::changes::FILE_HEADER_HEIGHT))
        .flex_none()
        .px(px(Theme::SPACE_MD))
        .flex()
        .items_center()
        .gap(px(8.0))
        .bg(theme.ink(0.025))
        .child(bar(px(13.0), 13.0, 0, view, theme, cx))
        .child(bar(px(14.0), 14.0, 0, view, theme, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(bar(relative(0.55), 10.0, 0, view, theme, cx)),
        )
        .child(bar(px(64.0), 10.0, 0, view, theme, cx))
        .child(bar(px(24.0), 24.0, 0, view, theme, cx));
    let cell = |index: usize, cx: &mut App| {
        let gutter = |cx: &mut App| {
            div()
                .w(px(crate::changes::GUTTER_WIDTH))
                .flex_none()
                .pr(px(8.0))
                .flex()
                .justify_end()
                .child(bar(px(12.0), 8.0, index, view, theme, cx))
        };
        div()
            .h(px(crate::changes::diff_line_height(theme)))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .child(div().w(px(crate::changes::ACCENT_BAR_WIDTH)).flex_none())
            .child(gutter(cx))
            .child(
                div()
                    .w(px(if split {
                        crate::changes::SPLIT_MARKER_WIDTH
                    } else {
                        crate::changes::MARKER_WIDTH
                    }))
                    .flex_none(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pl(px(if split {
                        crate::changes::SPLIT_CODE_PADDING_LEFT
                    } else {
                        crate::changes::UNIFIED_CODE_PADDING_LEFT
                    }))
                    .child(bar(
                        relative(WIDTHS[index % WIDTHS.len()]),
                        8.0,
                        index,
                        view,
                        theme,
                        cx,
                    )),
            )
    };
    let lines = (0..14)
        .map(|index| {
            div()
                .h(px(crate::changes::diff_line_height(theme)))
                .flex_none()
                .flex()
                .child(cell(index, cx))
                .when(split, |row| {
                    row.child(
                        div()
                            .w(px(crate::changes::SPLIT_DIVIDER_WIDTH))
                            .self_stretch()
                            .flex_none()
                            .bg(theme.border),
                    )
                    .child(cell(index, cx))
                })
                .into_any_element()
        })
        .collect::<Vec<_>>();
    div()
        .id("pr-code-skeleton")
        .debug_selector(|| "pr-code-skeleton".into())
        .role(gpui::Role::ProgressIndicator)
        .aria_label("Loading diff")
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(
            div()
                .h(px(crate::changes::HUNK_HEADER_HEIGHT))
                .flex_none()
                .px(px(Theme::SPACE_MD))
                .flex()
                .items_center()
                .child(bar(px(180.0), 10.0, 0, view, theme, cx)),
        )
        .child(div().flex().flex_col().children(lines))
        .into_any_element()
}

/// Rows of the changed-file tree.
pub(crate) fn tree(view: EntityId, theme: &Theme, cx: &mut App) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(1.0))
        .children((0..12).map(|index| {
            let nested = index % 4 != 0;
            div()
                .h(px(28.0))
                .flex_none()
                .pl(px(if nested { 22.0 } else { 8.0 }))
                .pr(px(8.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(bar(px(14.0), 14.0, index, view, theme, cx))
                .child(bar(
                    relative(WIDTHS[index % WIDTHS.len()] * 0.7),
                    10.0,
                    index,
                    view,
                    theme,
                    cx,
                ))
        }))
        .into_any_element()
}
