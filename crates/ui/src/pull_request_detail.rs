//! Native PR inspection and explicit comment submission.
#[path = "pull_request_code.rs"]
mod code;
#[path = "pull_request_handoff.rs"]
mod handoff;
#[path = "pull_request_interactions.rs"]
mod interactions;
use crate::icons::{self, icon};
use crate::{
    settings::{self, PullRequestDestination, widgets},
    state::AppState,
    theme::Theme,
};
use commutator_proto::change_request_assessment::{CiState, Facts, check_failed, check_status};
use commutator_proto::{ChangeRequestDetail, ChangeRequestListItem, ChangeRequestState};
use commutator_rpc::methods;
use gpui::{
    Action, AnyElement, App, Context, Entity, Focusable, IntoElement, Render, SharedString,
    Subscription, Task, Window, div, prelude::*, px,
};
#[cfg(test)]
use std::time::Duration;
use std::{cell::RefCell, rc::Rc, sync::Arc, time::Instant};

#[derive(Clone, PartialEq, Action)]
#[action(namespace = shell, no_json)]
pub struct OpenPullRequest(pub String, pub Option<String>);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = shell, no_json)]
pub struct ClosePullRequest;

#[derive(Clone, PartialEq, Action)]
#[action(namespace = shell, no_json)]
pub struct OpenPrImage(pub String);

/// Open a new session with this prompt staged in the composer (not sent).
#[derive(Clone, PartialEq, Action)]
#[action(namespace = shell, no_json)]
pub struct StartPullRequestSession {
    pub prompt: String,
    pub project_id: String,
    pub device: String,
    pub repository: String,
}

#[derive(Clone, PartialEq, Action)]
#[action(namespace = shell, no_json)]
pub struct AddPullRequestProject(pub String);

/// What went wrong and what to do, in words. Error codes and transport
/// details never reach the reader.
fn failure_reason(error: &commutator_rpc::RpcError) -> &'static str {
    use commutator_rpc::{RpcError, capability_errors as codes};
    match error {
        RpcError::Capability(code) if code == codes::PULL_REQUESTS_CLI_UNAVAILABLE => {
            "Install GitHub CLI (gh) on this device and sign in."
        }
        RpcError::Capability(code) if code == codes::PULL_REQUESTS_AUTHENTICATION => {
            "Run gh auth login on this device, then refresh."
        }
        RpcError::Capability(code) if code == codes::PULL_REQUESTS_RATE_LIMITED => {
            "GitHub’s rate limit was reached. Try again in 15 minutes."
        }
        RpcError::Capability(code) if code == codes::PULL_REQUESTS_TIMEOUT => {
            "GitHub didn’t respond in time. Try again."
        }
        RpcError::Capability(code) if code == codes::PULL_REQUESTS_DECODE => {
            "GitHub’s response was too large or unreadable. Open it on GitHub instead."
        }
        RpcError::UnknownMethod(_) => "Update Commutator on the selected device.",
        _ => "Check the link and your connection, then try again.",
    }
}

/// The canonical `https://github.com/owner/repo/pull/N` a link points into,
/// e.g. from its Files tab or a review comment; `None` for anything else.
pub(crate) fn pull_request_link(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let mut segments = url.path_segments()?;
    let (owner, repository) = (segments.next()?, segments.next()?);
    let number = (segments.next()? == "pull")
        .then(|| segments.next())
        .flatten()?
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)?;
    if owner.is_empty() || repository.is_empty() {
        return None;
    }
    Some(format!(
        "https://github.com/{owner}/{repository}/pull/{number}"
    ))
}

/// Whether pull requests open in Commutator's own view, per Settings.
pub(crate) fn opens_natively(cx: &App) -> bool {
    settings::current(cx).pull_request_destination != PullRequestDestination::External
}

pub fn open(url: &str, window: &mut Window, cx: &mut App) {
    open_on_device(url, None, window, cx);
}

pub fn open_on_device(url: &str, device: Option<String>, window: &mut Window, cx: &mut App) {
    // The native view reads github.com through `gh`; pull requests on other
    // hosts open in the browser.
    let on_github = url::Url::parse(url).is_ok_and(|url| url.host_str() == Some("github.com"));
    if !on_github
        || settings::current(cx).pull_request_destination == PullRequestDestination::External
    {
        cx.open_url(url);
    } else {
        window.dispatch_action(Box::new(OpenPullRequest(url.to_owned(), device)), cx);
    }
}

/// The view's tabs under the pull request's head, as Shepherd's review
/// panel has them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Summary,
    Timeline,
    Code,
    Checks,
}

const TABS: [(Tab, &str); 4] = [
    (Tab::Summary, "Summary"),
    (Tab::Timeline, "Timeline"),
    (Tab::Code, "Code"),
    (Tab::Checks, "Checks"),
];

/// The body's side padding.
const PAD: f32 = 16.0;
/// Room under the Code tab's stream and the comment dock.
const NAV_CLEARANCE: f32 = 12.0;
/// Below this page width (a side pane at its narrowest) the content column
/// tightens its gutters.
const NARROW_PAGE: f32 = 480.0;
/// The content column's gutter on narrow pages.
const NARROW_PAD_X: f32 = 16.0;

/// What a patch row stands for in the review stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowRole {
    File,
    Notice,
    Hunk,
    Line,
}

struct CodeRow {
    role: RowRole,
    text: SharedString,
    old: Option<u32>,
    new: Option<u32>,
    kind: crate::changes::LineKind,
    spans: Vec<commutator_syntax::HighlightSpan>,
}

fn code_rows(patch: &str) -> (Vec<CodeRow>, Vec<(String, usize)>) {
    use crate::changes::LineKind;
    let mut rows = Vec::new();
    let mut files = Vec::new();
    let mut parsed = crate::changes::parse_patch(patch);
    // Directory order, so each folder appears once in the tree and the
    // stream reads in the same order as the tree.
    parsed.sort_by(|a, b| {
        let key = |path: &str| match path.rsplit_once('/') {
            Some((directory, name)) => (1, directory.to_owned(), name.to_owned()),
            None => (0, String::new(), path.to_owned()),
        };
        key(&a.path).cmp(&key(&b.path))
    });
    for file in parsed {
        let highlights = commutator_syntax::language_for_path(&file.path)
            .and_then(|language| crate::changes::excerpt_highlights(&file, language));
        files.push((file.path.clone(), rows.len()));
        rows.push(CodeRow {
            role: RowRole::File,
            text: file.path.clone().into(),
            old: None,
            new: None,
            kind: LineKind::Meta,
            spans: Vec::new(),
        });
        for notice in crate::changes::file_notices(&file) {
            rows.push(CodeRow {
                role: RowRole::Notice,
                text: notice.into(),
                old: None,
                new: None,
                kind: LineKind::Meta,
                spans: Vec::new(),
            });
        }
        for hunk in file.hunks {
            rows.push(CodeRow {
                role: RowRole::Hunk,
                text: hunk.header.into(),
                old: None,
                new: None,
                kind: LineKind::Meta,
                spans: Vec::new(),
            });
            rows.extend(hunk.lines.into_iter().map(|line| {
                CodeRow {
                    role: RowRole::Line,
                    spans: highlights
                        .as_ref()
                        .map(|h| h.spans(&line).to_vec())
                        .unwrap_or_default(),
                    text: line.text.into(),
                    old: line.old_no,
                    new: line.new_no,
                    kind: line.kind,
                }
            }));
        }
    }
    (rows, files)
}

struct ParsedDiff {
    patch: String,
    rows: Vec<CodeRow>,
    files: Vec<(String, usize)>,
    pairs: Vec<Vec<crate::changes::LinePair>>,
    /// Per-file (additions, deletions), counted from the patch itself.
    stats: Vec<(u64, u64)>,
    /// Widest code line, in display columns, across the whole patch.
    columns: usize,
    gutter: f32,
}

impl ParsedDiff {
    fn new(patch: String) -> Arc<Self> {
        use crate::changes::LineKind;
        let (rows, files) = code_rows(&patch);
        let stats = files
            .iter()
            .enumerate()
            .map(|(file, (_, start))| {
                let end = files.get(file + 1).map_or(rows.len(), |(_, end)| *end);
                rows[*start..end]
                    .iter()
                    .fold((0, 0), |(add, del), row| match row.kind {
                        LineKind::Add if row.role == RowRole::Line => (add + 1, del),
                        LineKind::Del if row.role == RowRole::Line => (add, del + 1),
                        _ => (add, del),
                    })
            })
            .collect();
        let columns = rows
            .iter()
            .filter(|row| row.role == RowRole::Line)
            .map(|row| crate::changes::visual_columns(&row.text))
            .max()
            .unwrap_or(0);
        Arc::new(Self {
            pairs: code::split_files(&rows, &files),
            gutter: code::code_gutter(&rows),
            patch,
            rows,
            files,
            stats,
            columns,
        })
    }
}

#[derive(Clone)]
struct DetailSnapshot {
    detail: ChangeRequestDetail,
    body: crate::markdown::BlockTree,
    activity: Vec<crate::markdown::BlockTree>,
    fetched: Instant,
    diff: Option<Arc<ParsedDiff>>,
    /// Keep the engine-cache invalidation alive across closing/reopening.
    diff_refresh_owed: bool,
}

/// Window/profile scoped, bounded cache. Device remains part of the identity.
#[derive(Default)]
pub(crate) struct PullRequestCache {
    entries: Vec<((Option<String>, String), DetailSnapshot)>,
    /// Unsent comments, with the reason the last send failed.
    drafts: std::collections::HashMap<(Option<String>, String), (String, Option<String>)>,
    /// A post belongs to the window/profile, not to a particular detail view.
    submissions: std::collections::HashMap<(Option<String>, String), Entity<CommentSubmission>>,
}

struct CommentSubmission {
    body: String,
    result: Option<
        Result<
            (
                commutator_proto::ChangeRequestComment,
                crate::markdown::BlockTree,
            ),
            String,
        >,
    >,
}

impl PullRequestCache {
    fn get(&mut self, target: &Option<String>, url: &str) -> Option<DetailSnapshot> {
        let index = self
            .entries
            .iter()
            .position(|((device, entry), _)| device == target && entry == url)?;
        let entry = self.entries.remove(index);
        let snapshot = entry.1.clone();
        self.entries.push(entry);
        Some(snapshot)
    }

    fn put(&mut self, target: Option<String>, url: String, snapshot: DetailSnapshot) {
        self.entries
            .retain(|((device, entry), _)| device != &target || entry != &url);
        self.entries.push(((target, url), snapshot));
        if self.entries.len() > 12 {
            self.entries.remove(0);
        }
    }

    fn evict(&mut self, target: &Option<String>, url: &str) {
        self.entries
            .retain(|((device, entry), _)| device != target || entry != url);
    }

    fn set_draft(&mut self, target: &Option<String>, url: &str, text: &str, error: Option<String>) {
        let key = (target.clone(), url.to_owned());
        if text.is_empty() {
            self.drafts.remove(&key);
        } else {
            self.drafts.insert(key, (text.to_owned(), error));
        }
    }
}

/// Mutable navigation and layout state for the immutable parsed patch.
struct CodeReview {
    split: bool,
    horizontal: gpui::ScrollHandle,
    /// The review stream: every file's header and (unless folded) body.
    list: gpui::ListState,
    stream: Rc<Vec<code::StreamRow>>,
    ranges: Vec<std::ops::Range<usize>>,
    collapsed: std::collections::HashSet<usize>,
    /// A file picked from the tree stays selected until the reader scrolls;
    /// the last files can be too short to reach the top of the viewport.
    jumped: Option<usize>,
    tree_scroll: gpui::ScrollHandle,
}

impl Default for CodeReview {
    fn default() -> Self {
        Self {
            split: false,
            horizontal: gpui::ScrollHandle::new(),
            list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(1024.0)),
            stream: Default::default(),
            ranges: Vec::new(),
            collapsed: Default::default(),
            jumped: None,
            tree_scroll: gpui::ScrollHandle::new(),
        }
    }
}

pub struct PullRequestDetailPage {
    state: Entity<AppState>,
    pub url: String,
    target: Option<String>,
    detail: Option<ChangeRequestDetail>,
    body: Option<crate::markdown::BlockTree>,
    activity_bodies: Vec<crate::markdown::BlockTree>,
    selection_prefix: String,
    cache: Rc<RefCell<PullRequestCache>>,
    preview: Option<ChangeRequestListItem>,
    /// A newly observed board revision, deferred while reading or posting.
    board_refresh_owed: bool,
    error: Option<String>,
    loading: bool,
    fetched: Option<Instant>,
    task: Option<Task<()>>,
    diff_task: Option<Task<()>>,
    copy_reset: Option<Task<()>>,
    copied_link: bool,
    diff: Option<Arc<ParsedDiff>>,
    review: CodeReview,
    diff_error: Option<String>,
    /// Refresh invalidated the patch. Keep bypassing the engine cache until
    /// a diff load succeeds, including across errors and reopening.
    diff_refresh_owed: bool,
    tab: Tab,
    /// The footer's or menu's action under way: `approve`, `merge`, ….
    acting: Option<&'static str>,
    act_task: Option<Task<()>>,
    /// Merge was pressed once: a second press within three seconds merges.
    merge_armed: Option<Instant>,
    merge_disarm: Option<Task<()>>,
    menu: crate::popover::Popup<()>,
    /// The file tree is one Tab stop; arrow keys move between files.
    tree_focus: gpui::FocusHandle,
    /// The narrow layout's file picker trigger, refocused when it closes.
    files_focus: gpui::FocusHandle,
    /// Focus to restore on the next frame, from handlers without a window.
    return_focus: Option<gpui::FocusHandle>,
    file_search: Entity<crate::composer::ComposerInput>,
    file_search_subscription: Option<Subscription>,
    file_query: String,
    files_expanded: bool,
    code_pane_width: Option<f32>,
    /// The page's own width, measured after layout (a frame late).
    page_width: Option<f32>,
    scroll: widgets::PageScroll,
    comment_input: Entity<crate::composer::ComposerInput>,
    comment_subscription: Option<Subscription>,
    /// A comment is being posted.
    submission: Option<Entity<CommentSubmission>>,
    submission_subscription: Option<Subscription>,
    comment_error: Option<String>,
    handoff_task: Option<Task<()>>,
    handoff_error: Option<String>,
    handoff_device: Option<String>,
    mention_token: Option<crate::composer::MentionToken>,
    mention_choices: Vec<String>,
    mention_index: usize,
    image_preview: Option<crate::attachments::PreviewImage>,
    image_focus: gpui::FocusHandle,
    /// Held by the reading area once clicked, so Cmd/Ctrl+C reaches it.
    content_focus: gpui::FocusHandle,
    image_task: Option<Task<()>>,
    image_cached: Option<(String, crate::attachments::PreviewImage)>,
    image_previous_focus: Option<gpui::FocusHandle>,
    /// The image that failed to load, offered in the browser instead.
    image_failed: Option<String>,
}

impl PullRequestDetailPage {
    pub(crate) fn new(
        state: Entity<AppState>,
        url: String,
        target: Option<String>,
        cache: Rc<RefCell<PullRequestCache>>,
        preview: Option<ChangeRequestListItem>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut page = Self {
            state,
            url,
            target,
            detail: None,
            body: None,
            activity_bodies: Vec::new(),
            selection_prefix: format!("pr-detail-{}-", cx.entity_id().as_u64()),
            cache,
            preview,
            board_refresh_owed: false,
            error: None,
            loading: false,
            fetched: None,
            task: None,
            diff_task: None,
            copy_reset: None,
            copied_link: false,
            diff: None,
            // Open on the layout last chosen here or in the Changes pane.
            review: CodeReview {
                split: settings::current(cx).diff_split,
                ..CodeReview::default()
            },
            diff_error: None,
            diff_refresh_owed: false,
            tab: Tab::Summary,
            acting: None,
            act_task: None,
            merge_armed: None,
            merge_disarm: None,
            menu: crate::popover::Popup::default(),
            tree_focus: cx.focus_handle().tab_stop(true),
            files_focus: cx.focus_handle().tab_stop(true),
            return_focus: None,
            file_search: cx.new(|cx| {
                crate::composer::ComposerInput::with_context(
                    "Find a changed file…",
                    "PaletteSearch",
                    cx,
                )
                .with_single_line()
                .with_text_metrics(12.0, 16.0)
                .with_tab_stop()
            }),
            file_search_subscription: None,
            file_query: String::new(),
            files_expanded: false,
            code_pane_width: None,
            page_width: None,
            scroll: widgets::PageScroll::default(),
            comment_input: cx.new(|cx| {
                crate::composer::ComposerInput::with_context(
                    "Write a comment… Type @ to mention someone",
                    crate::composer::MESSAGE_COMPOSER_CONTEXT,
                    cx,
                )
                .with_viewport_height(120.0)
                .with_tab_stop()
            }),
            comment_subscription: None,
            submission: None,
            submission_subscription: None,
            comment_error: None,
            handoff_task: None,
            handoff_error: None,
            handoff_device: None,
            mention_token: None,
            mention_choices: Vec::new(),
            mention_index: 0,
            image_preview: None,
            image_focus: cx.focus_handle(),
            content_focus: cx.focus_handle(),
            image_task: None,
            image_cached: None,
            image_previous_focus: None,
            image_failed: None,
        };
        cx.on_release(|page: &mut Self, _| {
            crate::markdown::render::clear_selection_surface(&page.selection_prefix);
        })
        .detach();
        page.file_search_subscription =
            Some(
                cx.subscribe(&page.file_search, |page: &mut Self, input, event, cx| {
                    if matches!(event, crate::composer::ComposerInputEvent::Edited) {
                        page.file_query = input.read(cx).text().to_lowercase();
                        cx.notify();
                    } else if matches!(event, crate::composer::ComposerInputEvent::Submitted) {
                        if let Some(index) =
                            page.parsed_diff().files.iter().position(|(path, _)| {
                                path.to_lowercase().contains(&page.file_query)
                            })
                        {
                            page.select_code_file(index, cx);
                            if page.files_expanded {
                                page.toggle_files(cx);
                                page.return_focus = Some(page.files_focus.clone());
                            }
                        }
                    }
                }),
            );
        let page_handle = cx.weak_entity();
        page.review.list.set_scroll_handler(move |_, _, cx| {
            // Scrolling hands the tree selection back to the scroll position
            // and re-pins the sticky header.
            let _ = page_handle.update(cx, |page, cx| {
                page.review.jumped = None;
                cx.notify();
            });
        });
        page.comment_subscription =
            Some(cx.subscribe(&page.comment_input, |page, _, event, cx| {
                page.comment_event(event, cx)
            }));
        let cached = page.cache.borrow_mut().get(&page.target, &page.url);
        let stale = cached.as_ref().is_some_and(|snapshot| {
            page.preview.as_ref().is_some_and(|preview| {
                !preview.head_ref_oid.is_empty()
                    && preview.head_ref_oid != snapshot.detail.head_ref_oid
            })
        });
        if stale {
            page.cache.borrow_mut().evict(&page.target, &page.url);
            page.diff_refresh_owed = true;
        }
        if let Some(snapshot) = cached.filter(|_| !stale) {
            page.fetched = Some(snapshot.fetched);
            page.detail = Some(snapshot.detail);
            page.body = Some(snapshot.body);
            page.activity_bodies = snapshot.activity;
            page.diff_refresh_owed = snapshot.diff_refresh_owed;
            if let Some(diff) = snapshot.diff {
                page.install_diff(diff, cx);
            }
        }
        let draft = page
            .cache
            .borrow()
            .drafts
            .get(&(page.target.clone(), page.url.clone()))
            .cloned();
        if let Some((text, error)) = draft {
            page.comment_input
                .update(cx, |input, cx| input.set_text(text, cx));
            page.comment_error = error;
        }
        let pending = page
            .cache
            .borrow()
            .submissions
            .get(&(page.target.clone(), page.url.clone()))
            .cloned();
        if let Some(pending) = pending {
            page.observe_submission(pending, cx);
        }
        if page.detail.is_none() {
            page.load(stale, cx);
        }
        page
    }

    /// The pull request's number and title, from the details or else the
    /// board's preview row.
    pub(crate) fn identity(&self) -> (Option<u64>, String) {
        self.detail
            .as_ref()
            .map(|detail| (detail.number, detail.title.clone()))
            .or_else(|| {
                self.preview
                    .as_ref()
                    .map(|item| (item.number, item.title.clone()))
            })
            .map_or_else(
                || (None, "Pull request".to_owned()),
                |(number, title)| (Some(number), title),
            )
    }

    /// The details are loading, e.g. for the tab's working indicator.
    pub(crate) fn is_loading(&self) -> bool {
        self.loading
    }

    /// The pane's toolbar row: glyph, title and the pull request's actions.
    pub(crate) fn toolbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let url = self.url.clone();
        let copy_url = self.url.clone();
        let (number, title) = self.identity();
        div()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .id("pr-toolbar-title")
                    .debug_selector(|| "pr-toolbar-title".into())
                    .role(gpui::Role::Heading)
                    .aria_label(match number {
                        Some(number) => format!("Pull request {number}: {title}"),
                        None => title.clone(),
                    })
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .px(px(4.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        crate::icons::icon(crate::icons::PULL_REQUEST)
                            .size(px(14.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(12.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text.opacity(0.85))
                            .child(title),
                    ),
            )
            .child(
                action("pr-detail-refresh", "Refresh pull request", &theme)
                    .when(self.loading, |el| el.opacity(0.4))
                    .on_click(cx.listener(|page, _, _, cx| {
                        cx.stop_propagation();
                        page.refresh(cx);
                    })),
            )
            .child(
                action(
                    "pr-copy-url",
                    if self.copied_link {
                        "Link copied"
                    } else {
                        "Copy link"
                    },
                    &theme,
                )
                .on_click(cx.listener(move |page, _, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_url.clone()));
                    page.copied_link = true;
                    page.copy_reset = Some(cx.spawn(async move |page, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(2))
                            .await;
                        let _ = page.update(cx, |page, cx| {
                            page.copied_link = false;
                            cx.notify();
                        });
                    }));
                    cx.notify();
                })),
            )
            .child(
                action("pr-external", "Open in default browser", &theme).on_click(
                    move |_, _, cx| {
                        cx.stop_propagation();
                        cx.open_url(&url);
                    },
                ),
            )
            .into_any_element()
    }

    /// Reconcile already-loaded board data when this tab becomes active.
    /// Observe each head once: a stale board or a failed refresh must not
    /// turn tab switching into repeated network requests.
    pub(crate) fn update_board_preview(
        &mut self,
        preview: ChangeRequestListItem,
        cx: &mut Context<Self>,
    ) {
        if preview.url != self.url
            || !commutator_proto::change_request_assessment::valid_head_oid(&preview.head_ref_oid)
        {
            return;
        }
        let changed = self
            .preview
            .as_ref()
            .is_none_or(|old| old.head_ref_oid != preview.head_ref_oid);
        self.preview = Some(preview);
        if changed {
            self.board_refresh_owed = true;
            self.refresh_from_board(cx);
        }
    }

    /// Drain a deferred revision after a read or comment finishes. The read
    /// may already have caught up, in which case no extra request is needed.
    fn refresh_from_board(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.board_refresh_owed || self.loading || self.submission.is_some() {
            return false;
        }
        self.board_refresh_owed = false;
        let Some(head) = self
            .preview
            .as_ref()
            .map(|preview| preview.head_ref_oid.clone())
        else {
            return false;
        };
        if self
            .detail
            .as_ref()
            .is_some_and(|detail| detail.head_ref_oid == head)
        {
            return false;
        }
        self.refresh_with_head(Some(head), cx);
        true
    }

    /// Reload the details, and the diff now or on the next Code visit.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_with_head(None, cx);
    }

    fn refresh_with_head(&mut self, head: Option<String>, cx: &mut Context<Self>) {
        if self.loading || self.submission.is_some() {
            return;
        }
        // A pending patch belongs to the pre-refresh details. Drop its task
        // before starting the detail read, regardless of the active tab.
        self.diff_task = None;
        self.diff_error = None;
        self.diff = None;
        crate::markdown::render::clear_selection_surface(&self.selection_prefix);
        self.diff_refresh_owed = true;
        let cached = self.cache.borrow_mut().get(&self.target, &self.url);
        if let Some(mut snapshot) = cached {
            snapshot.diff = None;
            snapshot.diff_refresh_owed = true;
            snapshot.fetched = Instant::now();
            self.cache
                .borrow_mut()
                .put(self.target.clone(), self.url.clone(), snapshot);
        }
        let mut params = self.params(true);
        if let Some(head) = head {
            // A new board revision must not join an older in-flight engine
            // read; the detail response supplies its matching base revision.
            params["headRefOid"] = head.into();
            params.as_object_mut().unwrap().remove("baseRefOid");
        }
        self.load_with_params(params, cx);
    }

    fn params(&self, refresh: bool) -> serde_json::Value {
        let mut params = serde_json::json!({"url": self.url, "refresh": refresh});
        if let Some(target) = &self.target {
            params["targetDeviceId"] = target.clone().into();
        }
        if let Some(head) = self
            .detail
            .as_ref()
            .map(|detail| &detail.head_ref_oid)
            .or_else(|| self.preview.as_ref().map(|preview| &preview.head_ref_oid))
            .filter(|head| commutator_proto::change_request_assessment::valid_head_oid(head))
        {
            params["headRefOid"] = head.clone().into();
        }
        if let Some(base) = self
            .detail
            .as_ref()
            .map(|detail| &detail.base_ref_oid)
            .filter(|base| commutator_proto::change_request_assessment::valid_head_oid(base))
        {
            params["baseRefOid"] = base.clone().into();
        }
        params
    }

    fn load(&mut self, refresh: bool, cx: &mut Context<Self>) {
        self.load_with_params(self.params(refresh), cx);
    }

    fn load_with_params(&mut self, params: serde_json::Value, cx: &mut Context<Self>) {
        if self.submission.is_some() && self.detail.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.error = Some("Connect to your device to load this pull request.".into());
            return;
        };
        self.loading = true;
        self.error = None;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::GET_CHANGE_REQUEST, params).await
                .map_err(|error| format!("Couldn’t load this pull request. {}", failure_reason(&error)))
                .and_then(|value| serde_json::from_value::<ChangeRequestDetail>(value).map_err(|_| "The device sent a pull request this version can’t read. Update Commutator on the selected device.".to_owned()));
            // Large descriptions and review threads must not stall the UI thread.
            let result = cx.background_executor().spawn(async move {
                result.map(|detail| DetailSnapshot {
                    body: super::pull_request_media::parse_description(&detail.body),
                    activity: detail.activity_comments()
                        .map(|comment| super::pull_request_media::parse_description(&comment.body)).collect(),
                    detail, fetched: Instant::now(), diff: None, diff_refresh_owed: false,
                })
            }).await;
            let _ = this.update(cx, |page, cx| {
                page.loading = false;
                let loaded = result.is_ok();
                match result {
                    Ok(mut snapshot) => {
                        if let Some(preview) = &page.preview
                            && !snapshot.detail.head_ref_oid.is_empty()
                            && preview.head_ref_oid == snapshot.detail.head_ref_oid
                        {
                            snapshot.detail.viewer_did_author = snapshot.detail.viewer_did_author.or(preview.viewer_did_author);
                            snapshot.detail.viewer_review_requested = snapshot.detail.viewer_review_requested.or(preview.viewer_review_requested);
                        }
                        if page.detail.as_ref().is_some_and(|detail| {
                            detail.head_ref_oid != snapshot.detail.head_ref_oid
                                || detail.base_ref_oid != snapshot.detail.base_ref_oid
                        }) {
                            page.diff_task = None;
                            page.diff = None;
                            page.diff_refresh_owed = true;
                        }
                        page.fetched = Some(snapshot.fetched);
                        snapshot.diff = page.diff.clone();
                        snapshot.diff_refresh_owed = page.diff_refresh_owed;
                        page.body = Some(snapshot.body.clone());
                        page.activity_bodies = snapshot.activity.clone();
                        page.detail = Some(snapshot.detail.clone());
                        page.cache.borrow_mut().put(page.target.clone(), page.url.clone(), snapshot);
                    }
                    Err(error) => page.error = Some(error),
                }
                if !page.refresh_from_board(cx)
                    && loaded
                    && page.tab == Tab::Code
                    && page.diff.is_none()
                {
                    page.load_diff(false, cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn load_diff(&mut self, refresh: bool, cx: &mut Context<Self>) {
        // Wait for the detail's exact revisions before fetching its patch.
        if self.loading || self.detail.is_none() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.diff_error = Some("Connect to your device to load the diff.".into());
            return;
        };
        self.diff_error = None;
        let refresh = refresh || self.diff_refresh_owed;
        let params = self.params(refresh);
        let detail = self.detail.as_ref().unwrap();
        let revisions = (detail.base_ref_oid.clone(), detail.head_ref_oid.clone());
        self.diff_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::GET_CHANGE_REQUEST_DIFF, params)
                .await
                .map_err(|error| format!("Couldn’t load the diff. {}", failure_reason(&error)))
                .and_then(|value| {
                    serde_json::from_value::<String>(value)
                        .map_err(|_| "The device returned an invalid diff.".to_owned())
                });
            let result = cx
                .background_executor()
                .spawn(async move { result.map(ParsedDiff::new) })
                .await;
            let _ = this.update(cx, |page, cx| {
                // Cancellation is the first line of defense; the response
                // must also belong to the revisions currently being shown.
                if page.loading
                    || !page.detail.as_ref().is_some_and(|detail| {
                        (&detail.base_ref_oid, &detail.head_ref_oid) == (&revisions.0, &revisions.1)
                    })
                {
                    return;
                }
                page.diff_task = None;
                match result {
                    Ok(diff) => {
                        page.diff_refresh_owed = false;
                        let cached = page.cache.borrow_mut().get(&page.target, &page.url);
                        if let Some(mut snapshot) = cached
                            && (
                                snapshot.detail.base_ref_oid.as_str(),
                                snapshot.detail.head_ref_oid.as_str(),
                            ) == (revisions.0.as_str(), revisions.1.as_str())
                        {
                            snapshot.diff = Some(diff.clone());
                            snapshot.diff_refresh_owed = false;
                            page.cache.borrow_mut().put(
                                page.target.clone(),
                                page.url.clone(),
                                snapshot,
                            );
                        }
                        page.install_diff(diff, cx);
                    }
                    Err(error) => page.diff_error = Some(error),
                }
                cx.notify();
            });
        }));
    }

    /// Rendering may run while a diff is loading. Its empty view shares the
    /// same representation as a loaded patch, without parallel fallback fields.
    fn parsed_diff(&self) -> &ParsedDiff {
        static EMPTY: std::sync::LazyLock<Arc<ParsedDiff>> =
            std::sync::LazyLock::new(|| ParsedDiff::new(String::new()));
        self.diff.as_deref().unwrap_or(&EMPTY)
    }

    fn install_diff(&mut self, diff: Arc<ParsedDiff>, cx: &mut Context<Self>) {
        self.diff = Some(diff);
        self.review.collapsed.clear();
        self.review.jumped = None;
        self.rebuild_stream(cx);
    }

    fn toggle_files(&mut self, cx: &mut Context<Self>) {
        self.files_expanded = !self.files_expanded;
        cx.notify();
    }

    fn select_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        crate::markdown::render::clear_selection_surface(&self.selection_prefix);
        self.tab = tab;
        self.scroll.scroll.set_offset(gpui::Point::default());
        if tab == Tab::Code && self.diff.is_none() && self.diff_task.is_none() {
            self.load_diff(false, cx);
        }
        cx.notify();
    }

    /// Approve, merge, mark ready, close or reopen, then read it again.
    fn act(
        &mut self,
        action: serde_json::Value,
        busy: &'static str,
        done: &'static str,
        cx: &mut Context<Self>,
    ) {
        if self.acting.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            crate::toast::error(cx, "Connect to the pull request’s device first.");
            return;
        };
        let mut params = action;
        params["url"] = self
            .detail
            .as_ref()
            .map_or(&self.url, |d| &d.url)
            .clone()
            .into();
        if let Some(target) = &self.target {
            params["targetDeviceId"] = target.clone().into();
        }
        self.acting = Some(busy);
        self.merge_armed = None;
        self.act_task = Some(cx.spawn(async move |page, cx| {
            let result = engine
                .client()
                .call(methods::ACT_ON_CHANGE_REQUEST, params)
                .await;
            let _ = page.update(cx, |page, cx| {
                page.acting = None;
                match result {
                    Ok(_) => crate::toast::success(cx, done),
                    Err(commutator_rpc::RpcError::Failed(why)) => crate::toast::error(cx, why),
                    Err(commutator_rpc::RpcError::UnknownMethod(_)) => crate::toast::error(
                        cx,
                        "Update Commutator on the pull request’s device to do this here.",
                    ),
                    Err(error) => crate::toast::error(cx, failure_reason(&error)),
                }
                page.refresh(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// The head: the project and number (a click opens it on the web) with
    /// its state; the title; the author, when it last changed and the
    /// command that checks it out (a click copies it); the branches and how
    /// much it changes.
    fn render_head(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (detail, preview) = (self.detail.clone(), self.preview.clone());
        let (detail, preview) = (detail.as_ref(), preview.as_ref());
        let (number, title) = self.identity();
        let number = number.map(|n| format!("#{n}")).unwrap_or_default();
        let url = detail.map_or(self.url.clone(), |d| d.url.clone());
        let repo = handoff::repository(&url);
        let state = detail.map(|d| d.state.to_ascii_uppercase());
        let state = match state.as_deref() {
            Some("MERGED") => ChangeRequestState::Merged,
            Some("CLOSED") => ChangeRequestState::Closed,
            Some(_) => ChangeRequestState::Open,
            None => preview.map_or(ChangeRequestState::Open, |p| p.state),
        };
        let draft = detail.map_or(preview.is_some_and(|p| p.is_draft), |d| d.is_draft);
        let (glyph_path, color, label) = crate::pull_requests::state_glyph(state, draft, theme);
        let open = state == ChangeRequestState::Open;
        let conflicts = detail.is_some_and(|d| d.mergeable.eq_ignore_ascii_case("CONFLICTING"));
        let approved = detail.is_some_and(|d| d.review_decision.eq_ignore_ascii_case("APPROVED"));
        let (text, accent) = (theme.text, theme.accent);
        let project = div()
            .id("pr-head-project")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(theme.text_muted)
            .hover(move |s| s.text_color(text))
            .child(div().min_w_0().truncate().child(repo))
            .child(div().flex_none().text_color(accent).child(number.clone()))
            .child(
                icon(icons::LUCIDE_SQUARE_ARROW_OUT_UP_RIGHT)
                    .size(px(12.0))
                    .flex_none()
                    .text_color(accent),
            )
            .tooltip(widgets::text_tooltip("Open in the browser"))
            .on_click(move |_, _, cx| cx.open_url(&url));
        let menu_open = self.menu.is_open();
        let mut menu_button = div()
            .id("pr-head-menu")
            .debug_selector(|| "pr-head-menu".into())
            .size(px(24.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.0))
            .cursor_pointer()
            .when(menu_open, |el| el.bg(theme.element_hover))
            .hover({
                let hover = theme.element_hover;
                move |s| s.bg(hover)
            })
            .tooltip(widgets::text_tooltip("More"))
            .child(
                icon(icons::MORE_HORIZONTAL)
                    .size(px(14.0))
                    .text_color(theme.text_muted),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|page, _, _, _| page.menu.note_trigger_press()),
            )
            .on_click(cx.listener(|page, _, _, cx| {
                cx.stop_propagation();
                if page.menu.take_press_was_open() {
                    page.close_menu(cx);
                } else {
                    page.menu.open(());
                }
                cx.notify();
            }));
        if self.menu.get().is_some() {
            let card = self.render_menu(theme, cx);
            menu_button = menu_button.child(crate::popover::anchored_menu_below_end(
                "pr-head-menu-card",
                card,
                self.menu.closing_since(),
            ));
        }
        let states = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .when(open && conflicts, |el| {
                el.child(pill(
                    icons::LUCIDE_TRIANGLE_ALERT,
                    "Conflicts",
                    theme.danger,
                ))
            })
            .when(open && approved, |el| {
                el.child(pill(icons::LUCIDE_CIRCLE_CHECK, "Approved", theme.success))
            })
            .child(pill(glyph_path, label, color))
            .child(menu_button);
        let author = detail
            .map(|d| &d.author)
            .or(preview.map(|p| &p.author))
            .cloned()
            .unwrap_or_default();
        let provider = preview.map_or("github", |p| p.provider.as_str()).to_owned();
        let updated = detail
            .and_then(|d| chrono::DateTime::parse_from_rfc3339(&d.updated_at).ok())
            .map(|at| at.with_timezone(&chrono::Utc))
            .or(preview.map(|p| p.updated_at))
            .map(|at| {
                format!(
                    "· updated {}",
                    crate::pull_requests::ago(at, chrono::Utc::now())
                )
            });
        let checkout = detail.map(|d| handoff::checkout_command(d, &self.url));
        let mono = theme.font_mono.clone();
        let byline = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w_0()
            .text_size(px(12.0))
            .text_color(theme.text_faint)
            .when(!author.login.is_empty(), |el| {
                el.child(crate::pull_requests::avatar(
                    author.avatar(&provider),
                    &author.login,
                    18.0,
                    theme,
                ))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text)
                        .child(author.login.clone()),
                )
            })
            .children(updated.map(|u| div().flex_none().child(u)))
            .child(div().flex_1())
            .children(checkout.map(|command| {
                let shown = command.clone();
                div()
                    .id("pr-head-checkout")
                    .flex_none()
                    .max_w(px(260.0))
                    .truncate()
                    .font_family(mono.clone())
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(move |s| s.text_color(text))
                    .tooltip(widgets::text_tooltip("Copy"))
                    .child(shown)
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(command.clone()));
                        crate::toast::info(cx, "Copied the checkout command");
                    })
            }));
        let branches = detail.map(|d| {
            let stats = div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    icon(icons::LUCIDE_FILE_TEXT)
                        .size(px(13.0))
                        .text_color(theme.text_faint),
                )
                .child(div().text_color(theme.text_muted).child(format!(
                    "{} file{}",
                    crate::pull_requests::thousands(d.files.len() as u64),
                    if d.files.len() == 1 { "" } else { "s" }
                )))
                .child(
                    div()
                        .font_family(mono.clone())
                        .text_color(theme.success)
                        .child(format!("+{}", crate::pull_requests::thousands(d.additions))),
                )
                .child(
                    div()
                        .font_family(mono.clone())
                        .text_color(theme.danger)
                        .child(format!("−{}", crate::pull_requests::thousands(d.deletions))),
                );
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .text_size(px(12.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .font_family(mono.clone())
                        .text_color(theme.text_faint)
                        .child(branch_chip("pr-head-base", &d.base_ref_name, theme).flex_none())
                        .when(!d.head_ref_name.is_empty(), |el| {
                            el.child(div().flex_none().child("←")).child(
                                branch_chip("pr-head-branch", &d.head_ref_name, theme).min_w_0(),
                            )
                        }),
                )
                .child(stats)
        });
        div()
            .id("pr-head")
            .debug_selector(|| "pr-head".into())
            .flex_none()
            .px(px(PAD))
            .pt(px(12.0))
            .pb(px(12.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(project)
                    .child(states),
            )
            .child(
                div()
                    .id("pr-detail-title")
                    .debug_selector(|| "pr-detail-title".into())
                    .text_size(px(15.0))
                    .line_height(px(20.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(if title.is_empty() { number } else { title }),
            )
            .child(byline)
            .children(branches)
            .into_any_element()
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.begin_close() {
            crate::popover::reap_popup(cx, |page: &mut Self| &mut page.menu);
        }
        cx.notify();
    }

    /// The "…" menu: the web, copies, an agent's session, closing.
    fn render_menu(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let item = |id: &'static str, glyph: &'static str, label: String| {
            let hover = theme.element_hover;
            div()
                .id(id)
                .h(px(28.0))
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(px(6.0))
                .text_size(px(13.0))
                .text_color(theme.text)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(
                    icon(glyph)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(div().flex_1().min_w_0().truncate().child(label))
        };
        let url = self
            .detail
            .as_ref()
            .map_or(self.url.clone(), |d| d.url.clone());
        let (web, link) = (url.clone(), url.clone());
        let mut card = crate::popover::popover_card(theme)
            .w(px(240.0))
            .flex()
            .flex_col()
            .on_mouse_down_out(cx.listener(|page, _, _, cx| page.close_menu(cx)))
            .child(
                item(
                    "pr-menu-web",
                    icons::LUCIDE_SQUARE_ARROW_OUT_UP_RIGHT,
                    "Open in the browser".into(),
                )
                .on_click(cx.listener(move |page, _, _, cx| {
                    cx.open_url(&web);
                    page.close_menu(cx);
                })),
            )
            .child(
                item("pr-menu-link", icons::COPY, "Copy link".into()).on_click(cx.listener(
                    move |page, _, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(link.clone()));
                        page.close_menu(cx);
                    },
                )),
            );
        if let Some(detail) = self.detail.clone() {
            let kinds = handoff::handoffs(&detail);
            if !kinds.is_empty() {
                card = card
                    .child(crate::popover::menu_separator())
                    .child(crate::popover::menu_heading(theme, "Hand off to an agent"));
            }
            for kind in kinds {
                let prompt = handoff::prompt(kind, &detail, &self.url);
                card = card.child(
                    item(kind.id(), kind.glyph(), kind.label().into())
                        .tooltip(widgets::text_tooltip(kind.summary()))
                        .on_click(cx.listener(move |page, _, window, cx| {
                            page.close_menu(cx);
                            page.prepare_handoff(prompt.clone(), window, cx);
                        })),
                );
            }
            let state = detail.state.to_ascii_uppercase();
            if state == "OPEN" {
                card = card.child(crate::popover::menu_separator()).child(
                    item(
                        "pr-menu-close",
                        icons::LUCIDE_GIT_PULL_REQUEST_CLOSED,
                        "Close pull request".into(),
                    )
                    .text_color(theme.danger)
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.close_menu(cx);
                        page.act(
                            serde_json::json!({"action": "close"}),
                            "close",
                            "Closed",
                            cx,
                        );
                    })),
                );
            } else if state == "CLOSED" {
                card = card.child(crate::popover::menu_separator()).child(
                    item(
                        "pr-menu-reopen",
                        icons::LUCIDE_GIT_PULL_REQUEST_ARROW,
                        "Reopen pull request".into(),
                    )
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.close_menu(cx);
                        page.act(
                            serde_json::json!({"action": "reopen"}),
                            "reopen",
                            "Reopened",
                            cx,
                        );
                    })),
                );
            }
        }
        card.into_any_element()
    }

    /// The tabs as one segmented control, and how the checks went beside
    /// them (a click shows them).
    fn render_tabs(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (text, muted, on) = (theme.text, theme.text_muted, theme.surface_raised);
        let tray = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(8.0))
            .bg(theme.element_hover)
            .children(TABS.iter().map(|&(tab, label)| {
                let selected = self.tab == tab;
                div()
                    .id(SharedString::from(format!("pr-tab-{label}")))
                    .debug_selector(move || format!("pr-tab-{label}"))
                    .role(gpui::Role::Tab)
                    .aria_selected(selected)
                    .h(px(24.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .map(|el| {
                        if selected {
                            el.bg(on)
                                .text_color(text)
                                .shadow(crate::theme::card_selected_shadows())
                        } else {
                            el.text_color(muted)
                                .cursor_pointer()
                                .hover(move |s| s.text_color(text))
                        }
                    })
                    .child(label)
                    .on_click(cx.listener(move |page, _, _, cx| page.select_tab(tab, cx)))
            }));
        let ci = self.detail.as_ref().and_then(|d| ci_line(d, theme));
        div()
            .flex_none()
            .px(px(PAD))
            .py(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .border_b_1()
            .border_color(theme.border)
            .child(tray)
            .child(div().flex_1())
            .children(ci.map(|(glyph, color, words)| {
                div()
                    .id("pr-ci")
                    .debug_selector(|| "pr-ci".into())
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(12.0))
                    .text_color(muted)
                    .cursor_pointer()
                    .hover(move |s| s.text_color(text))
                    .child(icon(glyph).size(px(14.0)).flex_none().text_color(color))
                    .child(div().min_w_0().truncate().child(words))
                    .on_click(cx.listener(|page, _, _, cx| page.select_tab(Tab::Checks, cx)))
            }))
            .into_any_element()
    }

    /// The Summary: who reviews it and what they said, its labels, its open
    /// threads; then the description.
    fn render_summary(
        &mut self,
        detail: &ChangeRequestDetail,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let none = || {
            div()
                .text_color(theme.text_faint)
                .child("None")
                .into_any_element()
        };
        let row = |glyph: &'static str, name: &'static str, value: AnyElement| {
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .child(
                    div()
                        .w(px(96.0))
                        .h(px(22.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_color(theme.text_muted)
                        .child(
                            icon(glyph)
                                .size(px(14.0))
                                .flex_none()
                                .text_color(theme.text_faint),
                        )
                        .child(name),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h(px(22.0))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.0))
                        .child(value),
                )
        };
        let reviewers = reviewers(detail);
        let reviewers = if reviewers.is_empty() {
            none()
        } else {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.0))
                .children(reviewers.into_iter().map(|(name, verdict)| {
                    let (glyph, color, said) = match verdict {
                        Verdict::Approved => {
                            (icons::LUCIDE_CIRCLE_CHECK, theme.success, "approved")
                        }
                        Verdict::Changes => {
                            (icons::LUCIDE_CIRCLE_X, theme.danger, "requested changes")
                        }
                        Verdict::Commented => {
                            (icons::LUCIDE_MESSAGE_SQUARE, theme.text_muted, "commented")
                        }
                        Verdict::Pending => {
                            (icons::CLOCK_CIRCLE, theme.text_faint, "hasn’t reviewed yet")
                        }
                    };
                    div()
                        .id(SharedString::from(format!("pr-reviewer-{name}")))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_color(theme.text)
                        .tooltip(widgets::text_tooltip(format!("{name} {said}")))
                        .child(icon(glyph).size(px(14.0)).flex_none().text_color(color))
                        .child(name)
                }))
                .into_any_element()
        };
        let labels = if detail.labels.is_empty() {
            none()
        } else {
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.0))
                .children(
                    detail
                        .labels
                        .iter()
                        .map(|l| crate::pull_requests::chip(l, theme).h(px(20.0))),
                )
                .into_any_element()
        };
        let threads = detail.review_threads.as_ref().map(|threads| {
            match threads.iter().filter(|t| !t.is_resolved).count() {
                0 if threads.is_empty() => div().text_color(theme.text_faint).child("None"),
                0 => div().text_color(theme.text_faint).child("All resolved"),
                n => div()
                    .text_color(theme.warning)
                    .child(format!("{n} unresolved")),
            }
        });
        let description = match self
            .body
            .as_ref()
            .filter(|_| !detail.body.trim().is_empty())
        {
            Some(body) => rich_text(
                body,
                format!("{}description", self.selection_prefix),
                &self.url,
                theme,
                crate::settings::math_style(cx),
                window,
                cx.weak_entity(),
            ),
            None => div()
                .text_color(theme.text_muted)
                .child("No description provided.")
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(
                div()
                    .id("pr-summary")
                    .debug_selector(|| "pr-summary".into())
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .text_size(px(12.0))
                    .child(row(icons::LUCIDE_USERS, "Reviewers", reviewers))
                    .child(row(icons::TAG, "Labels", labels))
                    .children(threads.map(|value| {
                        row(
                            icons::LUCIDE_MESSAGE_SQUARE,
                            "Threads",
                            value.into_any_element(),
                        )
                    })),
            )
            .child(
                div()
                    .id("pr-description")
                    .debug_selector(|| "pr-description".into())
                    .text_size(px(13.0))
                    .child(description),
            )
            .into_any_element()
    }

    /// The Timeline: what happened, oldest first. A comment, or a review
    /// that says something, is a card with its text; the rest are a line
    /// each, a push with its commits under it.
    fn render_timeline(
        &mut self,
        detail: &ChangeRequestDetail,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mono = theme.font_mono.clone();
        let events = timeline(detail);
        if events.is_empty() {
            return div()
                .id("pr-activity-empty")
                .py(px(32.0))
                .flex()
                .justify_center()
                .text_color(theme.text_muted)
                .child("Nothing yet: no comments, reviews or commits.")
                .into_any_element();
        }
        let mut column = div()
            .id("pr-timeline")
            .debug_selector(|| "pr-timeline".into())
            .flex()
            .flex_col()
            .gap(px(14.0));
        for (n, event) in events.into_iter().enumerate() {
            let (glyph, color, said): (&'static str, gpui::Hsla, String) = match &event.what {
                Happened::Opened => (
                    icons::LUCIDE_GIT_PULL_REQUEST_ARROW,
                    theme.success,
                    "opened this".into(),
                ),
                Happened::Commented(_) => (
                    icons::LUCIDE_MESSAGE_SQUARE,
                    theme.text_muted,
                    "commented".into(),
                ),
                Happened::Reviewed(_, state) => match state.as_str() {
                    "APPROVED" => (icons::LUCIDE_CIRCLE_CHECK, theme.success, "approved".into()),
                    "CHANGES_REQUESTED" => (
                        icons::LUCIDE_CIRCLE_X,
                        theme.danger,
                        "requested changes".into(),
                    ),
                    _ => (icons::EYE, theme.text_muted, "reviewed".into()),
                },
                Happened::Commits(c) => (
                    icons::LUCIDE_GIT_COMMIT,
                    theme.text_muted,
                    format!(
                        "pushed {} commit{}",
                        c.len(),
                        if c.len() == 1 { "" } else { "s" }
                    ),
                ),
                Happened::Merged => (
                    icons::LUCIDE_GIT_MERGE,
                    crate::pull_requests::merged_color(theme),
                    "merged this".into(),
                ),
                Happened::Closed => (
                    icons::LUCIDE_GIT_PULL_REQUEST_CLOSED,
                    theme.danger,
                    "closed this".into(),
                ),
            };
            let body = match &event.what {
                Happened::Commented(i) | Happened::Reviewed(i, _) => self
                    .activity_bodies
                    .get(*i)
                    .filter(|_| {
                        detail
                            .activity_comments()
                            .nth(*i)
                            .is_some_and(|c| !c.body.trim().is_empty())
                    })
                    .map(|body| (*i, body.clone())),
                _ => None,
            };
            let card = body.is_some();
            let who = if event.who.is_empty() {
                String::new()
            } else {
                format!("{} ", event.who)
            };
            let name = gpui::HighlightStyle {
                color: Some(theme.text),
                font_weight: Some(gpui::FontWeight::MEDIUM),
                ..Default::default()
            };
            let line = div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(12.0))
                .text_color(theme.text_muted)
                .child(if card {
                    crate::pull_requests::avatar(event.avatar.clone(), &event.who, 20.0, theme)
                } else {
                    div()
                        .size(px(20.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(glyph).size(px(14.0)).text_color(color))
                        .into_any_element()
                })
                .child(
                    div().flex_1().min_w_0().truncate().child(
                        gpui::StyledText::new(format!("{who}{said}"))
                            .with_highlights([(0..who.len(), name)]),
                    ),
                )
                .when(
                    card && !matches!(event.what, Happened::Commented(_)),
                    |el| el.child(icon(glyph).size(px(14.0)).flex_none().text_color(color)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.text_faint)
                        .child(event.ago.clone()),
                );
            let commits = match &event.what {
                Happened::Commits(c) => Some(
                    div()
                        .pl(px(28.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .text_size(px(12.0))
                        .children(c.iter().map(|(sha, headline)| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .flex_none()
                                        .font_family(mono.clone())
                                        .text_color(theme.text_faint)
                                        .child(sha.clone()),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(theme.text)
                                        .child(headline.clone()),
                                )
                        })),
                ),
                _ => None,
            };
            let thread = match &event.what {
                Happened::Commented(i) => inline_thread(detail, *i).map(|t| {
                    let location = t
                        .line
                        .or(t.original_line)
                        .map(|l| format!("{}:{l}", t.path))
                        .unwrap_or_else(|| t.path.clone());
                    let mut words = vec![if t.is_resolved {
                        "Resolved"
                    } else {
                        "Unresolved"
                    }];
                    if t.is_outdated {
                        words.push("Outdated");
                    }
                    let i = *i;
                    div()
                        .debug_selector(move || format!("pr-inline-thread-{i}"))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(11.5))
                        .text_color(theme.text_muted)
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family(mono.clone())
                                .child(location),
                        )
                        .child(div().flex_none().child(words.join(" · ")))
                }),
                _ => None,
            };
            column = column.child(
                div()
                    .id(SharedString::from(format!("pr-message-{n}")))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(line)
                    .children(commits)
                    .children(body.map(|(i, body)| {
                        div()
                            .ml(px(28.0))
                            .p(px(10.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.surface_card)
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .text_size(px(13.0))
                            .text_color(theme.text)
                            .children(thread)
                            .child(rich_text(
                                &body,
                                format!("{}activity-{i}", self.selection_prefix),
                                &self.url,
                                theme,
                                crate::settings::math_style(cx),
                                window,
                                cx.weak_entity(),
                            ))
                    })),
            );
        }
        column.into_any_element()
    }

    /// The Checks: how many failed, run and passed, then each, its state and
    /// a click to its page.
    fn render_checks(&self, detail: &ChangeRequestDetail, theme: &Theme) -> AnyElement {
        let checks = &detail.status_check_rollup;
        if checks.is_empty() {
            let why = match Facts::from_detail(detail).assess().ci.state {
                CiState::Unknown => "Checks couldn’t be read. Refresh to try again.",
                _ => "Nothing has run on this branch yet.",
            };
            return div()
                .py(px(32.0))
                .flex()
                .justify_center()
                .text_color(theme.text_muted)
                .child(why)
                .into_any_element();
        }
        let states: Vec<CiState> = checks.iter().map(check_state).collect();
        let count = |s: CiState| states.iter().filter(|x| **x == s).count();
        let summary: Vec<String> = [
            (CiState::Failed, "failed"),
            (CiState::Pending, "running"),
            (CiState::Passed, "passed"),
        ]
        .into_iter()
        .filter(|(s, _)| count(*s) > 0)
        .map(|(s, what)| format!("{} {what}", count(s)))
        .collect();
        let hover = theme.element_hover;
        div()
            .id("pr-checks")
            .debug_selector(|| "pr-checks".into())
            .flex()
            .flex_col()
            .gap(px(2.0))
            .text_size(px(12.0))
            .child(
                div()
                    .pb(px(4.0))
                    .flex()
                    .items_center()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_faint)
                    .child(div().flex_1().child("Checks"))
                    .child(summary.join(" · ")),
            )
            .children(
                checks
                    .iter()
                    .zip(states)
                    .enumerate()
                    .map(|(i, (check, state))| {
                        let name = if check.name.is_empty() {
                            check.context.clone()
                        } else {
                            check.name.clone()
                        };
                        let link = [&check.details_url, &check.target_url]
                            .into_iter()
                            .find(|u| !u.is_empty())
                            .and_then(|u| url::Url::parse(u).ok())
                            .filter(|u| {
                                u.scheme() == "https"
                                    && u.username().is_empty()
                                    && u.password().is_none()
                            })
                            .map(String::from);
                        let status = [&check.conclusion, &check.state, &check.status]
                            .into_iter()
                            .find(|v| !v.is_empty())
                            .map(|v| v.replace('_', " ").to_lowercase())
                            .unwrap_or_else(|| "pending".into());
                        let (glyph, color) = ci_glyph(state, theme);
                        div()
                            .id(SharedString::from(format!("pr-check-{i}")))
                            .debug_selector(move || format!("pr-check-{i}"))
                            .h(px(28.0))
                            .px(px(6.0))
                            .mx(px(-6.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .rounded(px(6.0))
                            .child(icon(glyph).size(px(14.0)).flex_none().text_color(color))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(theme.text)
                                    .child(name),
                            )
                            .child(div().flex_none().text_color(theme.text_faint).child(status))
                            .when_some(link, |el, link| {
                                el.cursor_pointer()
                                    .hover(move |s| s.bg(hover))
                                    .on_click(move |_, _, cx| cx.open_url(&link))
                            })
                    }),
            )
            .into_any_element()
    }

    /// The bar under an open pull request: Approve and Merge, or a draft's
    /// Mark as ready. A button that is off says why.
    fn render_footer(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let detail = self.detail.as_ref()?;
        if !detail.state.eq_ignore_ascii_case("OPEN") {
            return None;
        }
        let bar = div()
            .flex_none()
            .px(px(PAD))
            .py(px(10.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(12.0));
        let acting = self.acting;
        let hover = theme.element_hover;
        if detail.is_draft {
            let busy = acting == Some("ready");
            let ready = footer_button(
                "pr-ready",
                icons::CHECK,
                if busy { "Marking…" } else { "Mark as ready" },
                theme.text,
                None,
                busy,
            )
            .border_1()
            .border_color(theme.border)
            .when(acting.is_none(), |el| {
                el.hover(move |s| s.bg(hover))
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.act(
                            serde_json::json!({"action": "ready"}),
                            "ready",
                            "Marked as ready for review",
                            cx,
                        )
                    }))
            });
            return Some(
                bar.child(div().flex_1().text_color(theme.text_faint).child("Draft"))
                    .child(ready)
                    .into_any_element(),
            );
        }
        let viewer = detail.viewer_login.as_deref();
        let approved_by_viewer = viewer.is_some_and(|me| {
            latest_reviews(detail)
                .iter()
                .any(|(who, state)| who.eq_ignore_ascii_case(me) && state == "APPROVED")
        });
        let no_approve = if approved_by_viewer {
            Some("You approved it; dismiss the review on GitHub to take it back")
        } else if detail.viewer_did_author == Some(true) {
            Some("You can’t approve your own pull request")
        } else {
            None
        };
        let approving = acting == Some("approve");
        let (glyph, ink, edge) = if approved_by_viewer {
            (
                icons::LUCIDE_CIRCLE_CHECK,
                theme.success,
                theme.success.opacity(0.5),
            )
        } else {
            (icons::CHECK, theme.text, theme.border)
        };
        let approve = footer_button(
            "pr-approve",
            glyph,
            match (approved_by_viewer, approving) {
                (true, _) => "Approved",
                (false, true) => "Approving…",
                (false, false) => "Approve",
            },
            ink,
            no_approve.filter(|_| !approved_by_viewer),
            approving,
        )
        .border_1()
        .border_color(edge)
        .when(approved_by_viewer, |el| {
            el.bg(theme.success.opacity(0.12))
                .tooltip(widgets::text_tooltip(no_approve.unwrap_or_default()))
        })
        .when(no_approve.is_none() && acting.is_none(), |el| {
            el.hover(move |s| s.bg(hover))
                .on_click(cx.listener(|page, _, _, cx| {
                    page.act(
                        serde_json::json!({"action": "approve"}),
                        "approve",
                        "Approved",
                        cx,
                    )
                }))
        });
        let merging = acting == Some("merge");
        let running = Facts::from_detail(detail).assess().ci.state == CiState::Pending;
        let merge = if detail.auto_merge_request.is_some() {
            footer_button(
                "pr-merge",
                icons::CLOCK_CIRCLE,
                if merging {
                    "Cancelling…"
                } else {
                    "Auto-merge on"
                },
                theme.success,
                None,
                merging,
            )
            .border_1()
            .border_color(theme.success.opacity(0.5))
            .bg(theme.success.opacity(0.12))
            .when(acting.is_none(), |el| {
                el.hover(move |s| s.bg(hover))
                    .tooltip(widgets::text_tooltip("Cancel auto-merge"))
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.act(
                            serde_json::json!({"action": "disableAutoMerge"}),
                            "merge",
                            "Auto-merge cancelled",
                            cx,
                        )
                    }))
            })
        } else {
            let conflicts = detail.mergeable.eq_ignore_ascii_case("CONFLICTING");
            let armed = self
                .merge_armed
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(3));
            let label = match (merging, armed, running) {
                (true, _, _) => "Merging…",
                (false, true, _) => "Confirm merge",
                (false, false, true) => "Auto-merge",
                (false, false, false) => "Squash and merge",
            };
            footer_button(
                "pr-merge",
                if running { icons::CLOCK_CIRCLE } else { icons::LUCIDE_GIT_MERGE },
                label,
                theme.on_accent,
                conflicts.then_some("Resolve the conflicts first"),
                merging,
            )
            .bg(theme.success)
            .when(running && !conflicts && !armed, |el| el.tooltip(widgets::text_tooltip("Merge once the checks pass")))
            .when(!conflicts && acting.is_none(), |el| {
                el.on_click(cx.listener(move |page, _, _, cx| {
                    // A second press within three seconds merges: t3code's
                    // inline confirm.
                    if page.merge_armed.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(3)) {
                        page.act(serde_json::json!({"action": "merge", "method": "squash", "auto": running}), "merge", if running { "Merges once the checks pass" } else { "Merged" }, cx);
                    } else {
                        page.merge_armed = Some(Instant::now());
                        page.merge_disarm = Some(cx.spawn(async move |page, cx| {
                            cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
                            let _ = page.update(cx, |page, cx| {
                                page.merge_armed = None;
                                cx.notify();
                            });
                        }));
                        cx.notify();
                    }
                }))
            })
        };
        Some(
            bar.child(div().flex_1())
                .child(approve)
                .child(merge)
                .into_any_element(),
        )
    }
}

/// A footer button: icon and label in `ink`, dimmed with `off` (its reason,
/// as the tooltip) or while busy.
fn footer_button(
    id: &'static str,
    glyph: &'static str,
    label: &'static str,
    ink: gpui::Hsla,
    off: Option<&'static str>,
    busy: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .role(gpui::Role::Button)
        .h(px(28.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .rounded(px(8.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(ink)
        .child(icon(glyph).size(px(13.0)).flex_none().text_color(ink))
        .child(label)
        .map(|el| match off {
            Some(why) => el.opacity(0.45).tooltip(widgets::text_tooltip(why)),
            None if busy => el.opacity(0.7),
            None => el.cursor_pointer(),
        })
}

/// A state pill: its icon and label in `color`, on a wash of it.
fn pill(glyph: &'static str, label: &'static str, color: gpui::Hsla) -> gpui::Div {
    div()
        .flex_none()
        .h(px(22.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(color.opacity(0.35))
        .bg(color.opacity(0.1))
        .text_color(color)
        .text_size(px(12.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(icon(glyph).size(px(13.0)).text_color(color))
        .child(label)
}

/// A branch's name; a click copies it.
fn branch_chip(id: &'static str, name: &str, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    let (text, name) = (theme.text, name.to_owned());
    div()
        .id(id)
        .truncate()
        .cursor_pointer()
        .hover(move |s| s.text_color(text))
        .tooltip(widgets::text_tooltip("Copy branch name"))
        .child(name.clone())
        .on_click(move |_, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(name.clone()));
            crate::toast::info(cx, format!("Copied {name}"));
        })
}

/// A check's state, from whichever of its fields the provider filled.
fn check_state(check: &commutator_proto::ChangeRequestCheck) -> CiState {
    let status = check_status(check);
    match status.as_str() {
        s if check_failed(s) => CiState::Failed,
        "SUCCESS" | "COMPLETED" | "NEUTRAL" => CiState::Passed,
        "SKIPPED" => CiState::Skipped,
        _ => CiState::Pending,
    }
}

fn ci_glyph(state: CiState, theme: &Theme) -> (&'static str, gpui::Hsla) {
    match state {
        CiState::Passed => (icons::LUCIDE_CIRCLE_CHECK, theme.success),
        CiState::Failed => (icons::LUCIDE_CIRCLE_X, theme.danger),
        CiState::Pending => (icons::LUCIDE_CIRCLE_DOT, theme.warning),
        _ => (icons::LUCIDE_CIRCLE_SLASH, theme.text_faint),
    }
}

/// How the checks went, as the tabs' line says it.
fn ci_line(
    detail: &ChangeRequestDetail,
    theme: &Theme,
) -> Option<(&'static str, gpui::Hsla, String)> {
    let state = Facts::from_detail(detail).assess().ci.state;
    let states: Vec<CiState> = detail.status_check_rollup.iter().map(check_state).collect();
    let some = |s: CiState, what: &str| {
        let n = states.iter().filter(|x| **x == s).count();
        format!("{n} check{} {what}", if n == 1 { "" } else { "s" })
    };
    let words = match state {
        CiState::Passed => "All checks passed".to_owned(),
        CiState::Failed => some(CiState::Failed, "failed"),
        CiState::Pending => some(CiState::Pending, "running"),
        CiState::Skipped => "Checks skipped".to_owned(),
        CiState::NoChecks | CiState::Unknown => return None,
    };
    let (glyph, color) = ci_glyph(state, theme);
    Some((glyph, color, words))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Approved,
    Changes,
    Commented,
    Pending,
}

/// Each reviewer's latest review state, in the order they first reviewed.
fn latest_reviews(detail: &ChangeRequestDetail) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for review in &detail.reviews {
        let who = &review.author.login;
        if who.is_empty() || review.state.eq_ignore_ascii_case("PENDING") {
            continue;
        }
        let state = review.state.to_ascii_uppercase();
        match out.iter_mut().find(|(w, _)| w == who) {
            // A comment after a verdict doesn't take the verdict back.
            Some((_, last)) if state == "COMMENTED" && last != "COMMENTED" => {}
            Some((_, last)) => *last = state,
            None => out.push((who.clone(), state)),
        }
    }
    out
}

/// Who reviews it: those who did, with their latest verdict, then those
/// asked who haven't yet.
fn reviewers(detail: &ChangeRequestDetail) -> Vec<(String, Verdict)> {
    let mut out: Vec<(String, Verdict)> = latest_reviews(detail)
        .into_iter()
        .filter(|(who, _)| !who.eq_ignore_ascii_case(&detail.author.login))
        .map(|(who, state)| {
            let verdict = match state.as_str() {
                "APPROVED" => Verdict::Approved,
                "CHANGES_REQUESTED" => Verdict::Changes,
                _ => Verdict::Commented,
            };
            (who, verdict)
        })
        .collect();
    for asked in &detail.review_requests {
        let name = if asked.login.is_empty() {
            asked.name.clone()
        } else {
            asked.login.clone()
        };
        if !name.is_empty() && !out.iter().any(|(who, _)| *who == name) {
            out.push((name, Verdict::Pending));
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
enum Happened {
    Opened,
    /// An activity comment, by its index in `activity_comments`.
    Commented(usize),
    Reviewed(usize, String),
    /// Short sha and headline of each commit in one push.
    Commits(Vec<(String, String)>),
    Merged,
    Closed,
}

#[derive(Debug, Clone, PartialEq)]
struct Event {
    who: String,
    avatar: Option<String>,
    at: String,
    ago: String,
    what: Happened,
}

/// The pull request's events, oldest first; commits by one author in a row
/// make one push.
fn timeline(detail: &ChangeRequestDetail) -> Vec<Event> {
    let ago = |raw: &str| {
        chrono::DateTime::parse_from_rfc3339(raw)
            .map(|t| crate::pull_requests::ago(t.with_timezone(&chrono::Utc), chrono::Utc::now()))
            .unwrap_or_default()
    };
    let event = |who: &commutator_proto::ChangeRequestActor, at: &str, what: Happened| Event {
        who: who.login.clone(),
        avatar: who.avatar("github"),
        at: at.to_owned(),
        ago: ago(at),
        what,
    };
    let mut events = Vec::new();
    if !detail.created_at.is_empty() {
        events.push(event(&detail.author, &detail.created_at, Happened::Opened));
    }
    let reviews = detail.comments.len();
    for (i, comment) in detail.activity_comments().enumerate() {
        let at = if comment.created_at.is_empty() {
            &comment.submitted_at
        } else {
            &comment.created_at
        };
        let review = i >= reviews && i < reviews + detail.reviews.len();
        // A review with nothing to say beyond its comments on lines.
        if review
            && comment.body.trim().is_empty()
            && comment.state.eq_ignore_ascii_case("COMMENTED")
        {
            continue;
        }
        let what = if review {
            Happened::Reviewed(i, comment.state.to_ascii_uppercase())
        } else {
            Happened::Commented(i)
        };
        events.push(event(&comment.author, at, what));
    }
    for commit in &detail.commits {
        let author = commit.authors.first().cloned().unwrap_or_default();
        let sha: String = commit.oid.chars().take(7).collect();
        let line = (sha, commit.message_headline.clone());
        events.push(event(
            &author,
            &commit.committed_date,
            Happened::Commits(vec![line]),
        ));
    }
    if !detail.merged_at.is_empty() {
        events.push(Event {
            who: String::new(),
            avatar: None,
            at: detail.merged_at.clone(),
            ago: ago(&detail.merged_at),
            what: Happened::Merged,
        });
    } else if !detail.closed_at.is_empty() {
        events.push(Event {
            who: String::new(),
            avatar: None,
            at: detail.closed_at.clone(),
            ago: ago(&detail.closed_at),
            what: Happened::Closed,
        });
    }
    // RFC 3339 in UTC sorts as text; the stable sort keeps a review's
    // comments after it.
    events.sort_by(|a, b| a.at.cmp(&b.at));
    let mut out: Vec<Event> = Vec::new();
    for e in events {
        if let (Some(last), Happened::Commits(more)) = (out.last_mut(), &e.what)
            && let Happened::Commits(run) = &mut last.what
            && last.who == e.who
        {
            run.extend(more.iter().cloned());
            last.at = e.at.clone();
            last.ago = e.ago.clone();
            continue;
        }
        out.push(e);
    }
    out
}

#[cfg(feature = "pull-request-fixture")]
impl PullRequestDetailPage {
    /// 0 Summary, 1 Timeline, 2 Code, 3 Checks.
    pub fn fixture_select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.select_tab(TABS[index.min(TABS.len() - 1)].0, cx);
    }

    pub fn fixture_select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        self.select_code_file(index, cx);
    }

    /// Scroll the review stream so a file's header is pinned.
    pub fn fixture_scroll_code(&mut self, distance: f32, cx: &mut Context<Self>) {
        self.review.list.scroll_by(px(distance));
        self.review.jumped = None;
        cx.notify();
    }
}

fn rich_text(
    body: &crate::markdown::BlockTree,
    key: String,
    url: &str,
    theme: &Theme,
    math: crate::markdown::math::MathStyle,
    window: &mut Window,
    owner: gpui::WeakEntity<PullRequestDetailPage>,
) -> AnyElement {
    let options = crate::markdown::render::RenderOptions {
        tasks: None,
        media: Some(super::pull_request_media::media(
            url,
            Rc::new(move |source, window, cx| {
                let _ = owner.update(cx, |page, cx| page.open_image(source, window, cx));
            }),
        )),
        row_key: key.into(),
        veil: None,
        cache: None,
        now: Instant::now(),
        link: None,
        workspace_root: None,
        code: None,
        math,
        copy: Some(crate::markdown::render::CopyUi {
            handler: Rc::new(|_, code, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(code.to_string()))
            }),
            copied_ix: None,
        }),
    };
    div()
        .debug_selector(|| "pr-rich-text".into())
        .child(super::pull_request_media::render_description(
            body, &options, theme, window,
        ))
        .into_any_element()
}

fn inline_thread(
    detail: &ChangeRequestDetail,
    index: usize,
) -> Option<&commutator_proto::ChangeRequestReviewThread> {
    let mut index = index.checked_sub(detail.comments.len() + detail.reviews.len())?;
    for thread in detail.review_threads.iter().flatten() {
        if index < thread.comments.len() {
            return Some(thread);
        }
        index -= thread.comments.len();
    }
    None
}

fn action(id: &'static str, label: &'static str, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    let icon_only = matches!(
        id,
        "pr-external"
            | "pr-copy-url"
            | "pr-copy-patch"
            | "pr-copy-path"
            | "pr-copy-checkout"
            | "pr-files"
            | "pr-fold-all"
            | "pr-detail-refresh"
            | "pr-previous-file"
            | "pr-next-file"
            | "pr-split"
    );
    let glyph = match id {
        "pr-split" => Some(crate::icons::SPLIT_COLUMNS),
        "pr-previous-file" => Some(crate::icons::ALT_ARROW_UP),
        "pr-next-file" => Some(crate::icons::ALT_ARROW_DOWN),
        "pr-fold-all" => Some(crate::icons::FOLD_VERTICAL),
        "pr-copy-url" if label == "Link copied" => Some(crate::icons::CHECK),
        "pr-copy-url" | "pr-copy-patch" | "pr-copy-path" | "pr-copy-checkout" => {
            Some(crate::icons::COPY)
        }
        "pr-detail-refresh" | "pr-retry" | "pr-retry-diff" => Some(crate::icons::REFRESH),
        "pr-external" => Some(crate::icons::ARROW_UP_RIGHT),
        "pr-files" => Some(crate::icons::FILE_TREE),
        _ => None,
    };
    widgets::ghost_action(theme)
        .id(id)
        .debug_selector(move || id.to_owned())
        .role(gpui::Role::Button)
        .aria_label(label)
        .tab_index(0)
        .border_1()
        .border_color(gpui::transparent_black())
        .cursor_pointer()
        .when(icon_only, |el| {
            el.size(px(24.0))
                .flex_none()
                .rounded(px(6.0))
                .occlude()
                .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                    window.prevent_default()
                })
                .px_0()
                .py_0()
                .justify_center()
                .tooltip(widgets::text_tooltip(label))
        })
        .children(glyph.map(|glyph| {
            crate::icons::icon(glyph)
                .size(px(14.0))
                .text_color(theme.text_muted)
        }))
        .when(!icon_only, |el| el.child(label))
}

impl crate::popover::ScrollRailHost for PullRequestDetailPage {
    fn rail_bar(&mut self) -> &mut crate::popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }
    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for PullRequestDetailPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(focus) = self.return_focus.take() {
            window.focus(&focus, cx);
        }
        crate::toast::take_error(&mut self.handoff_error, cx);
        let theme = Theme::of(cx).clone();
        let head = self.render_head(&theme, cx);
        let tabs = self.detail.is_some().then(|| self.render_tabs(&theme, cx));
        let mut body = div()
            .id("pr-content-column")
            .debug_selector(|| "pr-content-column".into())
            .w_full()
            .px(px(PAD))
            .pt(px(16.0))
            .pb(px(24.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .text_size(px(13.0))
            .text_color(theme.text);
        if let Some(error) = &self.error {
            body = body
                .child(widgets::error_strip(&theme, error.clone()))
                .child(
                    div().flex().child(
                        action("pr-retry", "Try again", &theme)
                            .on_click(cx.listener(|page, _, _, cx| page.refresh(cx))),
                    ),
                );
        }
        let code = self.tab == Tab::Code && self.detail.is_some();
        if let Some(detail) = self.detail.clone() {
            body =
                body.child(match self.tab {
                    Tab::Summary => self.render_summary(&detail, &theme, window, cx),
                    Tab::Timeline => self.render_timeline(&detail, &theme, window, cx),
                    Tab::Checks => self.render_checks(&detail, &theme),
                    Tab::Code => match &self.diff_error {
                        Some(error) => div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(widgets::error_strip(&theme, error.clone()))
                            .child(div().flex().child(
                                action("pr-retry-diff", "Retry diff", &theme).on_click(
                                    cx.listener(|page, _, _, cx| page.load_diff(true, cx)),
                                ),
                            ))
                            .into_any_element(),
                        // Loading renders the same workspace with placeholders.
                        None => self.code_workspace(&theme, window, cx),
                    },
                });
        } else if self.loading {
            body = body.child(div().py(px(48.0)).flex().justify_center().child(
                crate::loaders::ring_spinner("pr-detail-loading", 18.0, theme.text_muted),
            ));
        }
        let content = if code {
            // The diff scrolls itself, under its toolbar.
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(body.h_full().min_h_0().pb_0())
                .into_any_element()
        } else {
            let scroll = self.scroll.scroll.clone();
            let rail = crate::popover::rail(self, "pr-detail-scrollbar", &theme, cx);
            div()
                .id("pr-detail-scroll-host")
                .flex_1()
                .min_h_0()
                .relative()
                // Selecting text leaves no input focused: take focus on press
                // so Cmd/Ctrl+C reaches the copy below.
                .track_focus(&self.content_focus)
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|page, _, window, cx| window.focus(&page.content_focus, cx)),
                )
                .on_key_down(|event: &gpui::KeyDownEvent, _, cx| {
                    let keystroke = &event.keystroke;
                    if keystroke.key == "c"
                        && (keystroke.modifiers.platform || keystroke.modifiers.control)
                        && !keystroke.modifiers.shift
                        && !keystroke.modifiers.alt
                        && let Some(text) = crate::markdown::selection::selected_text()
                    {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                        cx.stop_propagation();
                    }
                })
                .on_hover(cx.listener(|page, hovered: &bool, _, cx| {
                    if page.scroll.set_list_hovered(*hovered) {
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .id("pr-detail-scroll")
                        .debug_selector(|| "pr-detail-scroll".into())
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&scroll)
                        .child(body),
                )
                .children(rail)
                .into_any_element()
        };
        let composer = (self.tab == Tab::Timeline && self.detail.is_some())
            .then(|| self.comment_composer(&theme, cx));
        let footer = self.render_footer(&theme, cx);
        // The pane sizes the page; layouts that depend on it read last
        // frame's width, like the Code tab's file tree.
        let measure = {
            let page = cx.weak_entity();
            gpui::canvas(
                move |bounds, window, cx| {
                    let width = f32::from(bounds.size.width);
                    window.defer(cx, move |_, cx| {
                        let _ = page.update(cx, |page, cx| {
                            if page.page_width.is_none_or(|old| (old - width).abs() > 0.5) {
                                page.page_width = Some(width);
                                cx.notify();
                            }
                        });
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };
        let mut root = div()
            .size_full()
            .relative()
            .child(measure)
            .on_action(cx.listener(|page, action: &OpenPrImage, window, cx| {
                page.open_image(&action.0, window, cx)
            }))
            .flex()
            .flex_col()
            .overflow_hidden()
            // Paint before Markdown registers this frame's text geometry.
            .child(crate::markdown::render::selection_frame_reset_for(
                cx.entity_id().as_u64(),
            ))
            .child(head)
            .children(tabs)
            .child(content)
            .children(composer)
            .children(footer);
        if let Some(preview) = &self.image_preview {
            let weak = cx.weak_entity();
            root = root.child(crate::attachments::lightbox(
                window,
                preview,
                &self.image_focus,
                move |window, cx| {
                    let _ = weak.update(cx, |page, cx| {
                        page.image_preview = None;
                        if let Some(focus) = page.image_previous_focus.take() {
                            window.focus(&focus, cx);
                        }
                        cx.notify();
                    });
                },
                cx,
            ));
        }
        if self.image_task.is_some() || self.image_failed.is_some() {
            root = root.child(
                div()
                    .id("pr-image-loading")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .track_focus(&self.image_focus)
                    .bg(crate::popover::scrim_alpha(0.7))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(gpui::white())
                    .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            page.close_image(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .on_click(cx.listener(|page, _, window, cx| {
                        if let Some(source) = &page.image_failed {
                            cx.open_url(source);
                        }
                        page.close_image(window, cx)
                    }))
                    .child(if self.image_failed.is_some() {
                        "Couldn’t load image. Click to open it in your browser · Escape to close"
                    } else {
                        "Loading image… · Escape to cancel"
                    }),
            );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request_test_support::{self as fixture, ScriptedRpc};

    fn snapshot(title: &str) -> DetailSnapshot {
        DetailSnapshot {
            detail: ChangeRequestDetail {
                title: title.into(),
                ..Default::default()
            },
            body: crate::markdown::parse_full("**Rich description**"),
            activity: vec![crate::markdown::parse_full("### Review\n\n- **important**")],
            fetched: Instant::now(),
            diff: None,
            diff_refresh_owed: false,
        }
    }

    #[gpui::test]
    fn pull_request_selection_tracks_repaint_and_clears_on_leaving(cx: &mut gpui::TestAppContext) {
        use crate::markdown::{render, selection};
        let _selection = selection::test_state_lock();
        fixture::init(cx);
        let (page, cx) = cx.add_window_view(|window, cx| {
            let state = fixture::state(cx, None);
            let cache = Rc::new(RefCell::new(PullRequestCache::default()));
            let mut cached = snapshot("Selection");
            cached.detail.body =
                "First paragraph for selection.\n\nSecond paragraph stays separate.".into();
            cached.body = crate::markdown::parse_full(&cached.detail.body);
            let url = "https://github.com/a/b/pull/1";
            cache.borrow_mut().put(None, url.into(), cached);
            PullRequestDetailPage::new(state, url.into(), None, cache, None, window, cx)
        });
        cx.simulate_resize(gpui::size(px(900.0), px(1400.0)));
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        let prefix = page.read_with(cx, |page, _| page.selection_prefix.clone());
        let key = format!("{prefix}description-block-0:0");
        assert_eq!(render::selection_test_entry_count(&prefix), 2);
        // Layout changes and extra paints must replace, not accumulate,
        // geometry. Click + zero-distance move must remain an empty selection.
        cx.simulate_resize(gpui::size(px(760.0), px(1400.0)));
        for _ in 0..3 {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear();
            });
        }
        assert_eq!(render::selection_test_entry_count(&prefix), 2);
        let position = render::selection_test_bounds(&key).origin + gpui::point(px(8.0), px(8.0));
        cx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position,
            click_count: 1,
            ..Default::default()
        });
        cx.simulate_event(gpui::MouseMoveEvent {
            position,
            pressed_button: Some(gpui::MouseButton::Left),
            ..Default::default()
        });
        assert!(selection::selected_text().is_none_or(|text| text.is_empty()));
        selection::end_active_drag();
        selection::begin_with_span(&key, "First paragraph", 0..5);
        // Clicking the text focused the reading area, so Cmd+C copies it.
        assert!(
            cx.update(|window, cx| page.read(cx).content_focus.is_focused(window)),
            "pressing the text focuses the reading area"
        );
        cx.simulate_keystrokes("cmd-c");
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("First")
        );
        page.update(cx, |page, cx| page.select_tab(Tab::Timeline, cx));
        assert!(selection::selected_text().is_none());
        assert_eq!(render::selection_test_entry_count(&prefix), 0);
        // A different PR instance has a disjoint namespace, including when
        // both pages coexist during navigation.
        let other = cx.update(|window, cx| {
            cx.new(|cx| {
                PullRequestDetailPage::new(
                    fixture::state(cx, None),
                    "https://github.com/a/b/pull/2".into(),
                    None,
                    Default::default(),
                    None,
                    window,
                    cx,
                )
            })
        });
        let other_prefix = other.read_with(cx, |page, _| page.selection_prefix.clone());
        assert_ne!(prefix, other_prefix);
        let other_key = format!("{other_prefix}description:0");
        selection::begin_with_span(&other_key, "Other PR", 0..5);
        drop(other);
        cx.update(|_, _| {}); // GPUI releases dropped entities after an update.
        cx.run_until_parked();
        assert!(
            selection::selected_text().is_none(),
            "releasing the view clears its selection"
        );
    }

    #[test]
    fn pull_request_cache_is_bounded_and_separates_devices() {
        let mut cache = PullRequestCache::default();
        cache.put(None, "same".into(), snapshot("Local"));
        cache.put(Some("remote".into()), "same".into(), snapshot("Remote"));
        assert_eq!(cache.get(&None, "same").unwrap().detail.title, "Local");
        assert_eq!(
            cache
                .get(&Some("remote".into()), "same")
                .unwrap()
                .detail
                .title,
            "Remote"
        );
        for index in 0..12 {
            cache.put(None, format!("url-{index}"), snapshot("PR"));
        }
        assert_eq!(cache.entries.len(), 12);
        assert!(cache.get(&None, "same").is_none());
        cache.get(&None, "url-0");
        cache.put(None, "new".into(), snapshot("New"));
        assert!(cache.get(&None, "url-0").is_some());
        assert!(cache.get(&None, "url-1").is_none());
    }

    #[gpui::test]
    fn pull_request_reopening_uses_cached_content_without_a_network_request(
        cx: &mut gpui::TestAppContext,
    ) {
        fixture::init(cx);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let state = fixture::state(cx, None);
            let cache = Rc::new(RefCell::new(PullRequestCache::default()));
            let url = "https://github.com/a/b/pull/1";
            let diff = ParsedDiff::new(
                "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n".into(),
            );
            let reused = diff.clone();
            let mut cached = snapshot("Already loaded");
            cached.diff = Some(diff);
            cached.fetched = Instant::now() - Duration::from_secs(3600);
            cache.borrow_mut().put(None, url.into(), cached);
            let page = PullRequestDetailPage::new(state, url.into(), None, cache, None, window, cx);
            // No engine exists in this fixture. Attempting a fetch would set an error.
            assert!(!page.loading && page.error.is_none());
            assert_eq!(page.detail.as_ref().unwrap().title, "Already loaded");
            assert_eq!(page.activity_bodies.len(), 1);
            assert!(
                Arc::ptr_eq(page.diff.as_ref().unwrap(), &reused),
                "cached diff rows are reused without parsing or copying"
            );
            page
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn pull_request_activity_renders_inline_feedback_and_replies(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        fixture::init(cx);
        let rpc = ScriptedRpc::new(|method, _| async move {
            assert_eq!(method, methods::GET_CHANGE_REQUEST);
            commutator_rpc::RpcReply::value(&ChangeRequestDetail {
                number: 1,
                title: "Inline review".into(),
                review_decision: "CHANGES_REQUESTED".into(),
                review_threads: Some(vec![commutator_proto::ChangeRequestReviewThread {
                    path: "src/lib.rs".into(),
                    line: Some(12),
                    comments: vec![
                        commutator_proto::ChangeRequestComment {
                            id: "first".into(),
                            body: "Preserve newer drafts".into(),
                            ..Default::default()
                        },
                        commutator_proto::ChangeRequestComment {
                            id: "reply".into(),
                            body: "Covered by the reopen test".into(),
                            reply_to: Some(commutator_proto::ChangeRequestCommentReference {
                                id: "first".into(),
                            }),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }]),
                ..Default::default()
            })
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let state = fixture::state(cx, Some(rpc.client()));
            PullRequestDetailPage::new(
                state,
                "https://github.com/a/b/pull/1".into(),
                None,
                Default::default(),
                None,
                window,
                cx,
            )
        });
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| !page.loading)
        });
        page.update(cx, |page, cx| {
            assert_eq!(page.activity_bodies.len(), 2);
            assert_eq!(page.detail.as_ref().unwrap().activity_comments().count(), 2);
            page.select_tab(Tab::Timeline, cx);
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("pr-inline-thread-0").is_some());
        assert!(cx.debug_bounds("pr-inline-thread-1").is_some());
        assert!(cx.debug_bounds("pr-activity-empty").is_none());
    }

    #[gpui::test]
    fn pull_request_board_revision_invalidates_cached_detail_and_diff(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        fixture::init(cx);
        let head = "b".repeat(40);
        let base = "c".repeat(40);
        let rpc = ScriptedRpc::new(move |method, params| {
            let head = head.clone();
            let base = base.clone();
            async move {
                assert_eq!(params["headRefOid"], head);
                assert_eq!(params["refresh"], true);
                match method.as_str() {
                    methods::GET_CHANGE_REQUEST => {
                        commutator_rpc::RpcReply::value(&ChangeRequestDetail {
                            head_ref_oid: head,
                            base_ref_oid: base,
                            ..Default::default()
                        })
                    }
                    methods::GET_CHANGE_REQUEST_DIFF => {
                        assert_eq!(params["baseRefOid"], base);
                        commutator_rpc::RpcReply::value(&"new patch")
                    }
                    _ => panic!("unexpected method"),
                }
            }
        });
        let cache = Rc::new(RefCell::new(PullRequestCache::default()));
        let url = "https://github.com/a/b/pull/1";
        let mut cached = snapshot("Old");
        cached.detail.head_ref_oid = "a".repeat(40);
        cached.diff = Some(ParsedDiff::new("old patch".into()));
        cache.borrow_mut().put(None, url.into(), cached);
        let window = cx.add_window(|window, cx| {
            let state = fixture::state(cx, Some(rpc.client()));
            let page = PullRequestDetailPage::new(
                state,
                url.into(),
                None,
                cache.clone(),
                Some(ChangeRequestListItem {
                    head_ref_oid: "b".repeat(40),
                    provider: "github".into(),
                    repository: "a/b".into(),
                    author: Default::default(),
                    ci: Default::default(),
                    viewer_did_author: None,
                    viewer_review_requested: None,
                    number: 1,
                    title: "Updated".into(),
                    url: url.into(),
                    state: commutator_proto::ChangeRequestState::Open,
                    is_draft: false,
                    review_decision: Default::default(),
                    additions: 1,
                    deletions: 1,
                    mergeability: commutator_proto::ChangeRequestMergeability::Unknown,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    labels: Vec::new(),
                    head_ref_name: String::new(),
                }),
                window,
                cx,
            );
            assert!(page.detail.is_none() && page.diff.is_none());
            page
        });
        window
            .update(cx, |page, _, cx| page.select_tab(Tab::Code, cx))
            .unwrap();
        rpc.settle(cx, &runtime, |cx| {
            window.update(cx, |page, _, _| page.diff.is_some()).unwrap()
        });
        window
            .update(cx, |page, _, _| {
                assert_eq!(page.detail.as_ref().unwrap().head_ref_oid, "b".repeat(40));
                assert_eq!(page.diff.as_ref().unwrap().patch, "new patch");
            })
            .unwrap();
        assert_eq!(rpc.completed(), 2);
    }

    fn board_preview(head: &str) -> ChangeRequestListItem {
        ChangeRequestListItem {
            head_ref_oid: head.into(),
            provider: "github".into(),
            repository: "a/b".into(),
            author: Default::default(),
            ci: Default::default(),
            viewer_did_author: None,
            viewer_review_requested: None,
            number: 1,
            title: "Updated".into(),
            url: "https://github.com/a/b/pull/1".into(),
            state: commutator_proto::ChangeRequestState::Open,
            is_draft: false,
            review_decision: Default::default(),
            additions: 1,
            deletions: 1,
            mergeability: commutator_proto::ChangeRequestMergeability::Unknown,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            labels: Vec::new(),
            head_ref_name: String::new(),
        }
    }

    #[gpui::test]
    fn pull_request_board_updates_live_tabs_once_and_keeps_drafts(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        fixture::init(cx);
        let rpc = ScriptedRpc::new(|method, params| async move {
            assert_eq!(params["refresh"], true);
            match method.as_str() {
                methods::GET_CHANGE_REQUEST => {
                    assert_eq!(params["headRefOid"], "b".repeat(40));
                    assert!(params.get("baseRefOid").is_none());
                    // GitHub can already be ahead of the board's snapshot.
                    commutator_rpc::RpcReply::value(&ChangeRequestDetail {
                        head_ref_oid: "c".repeat(40),
                        base_ref_oid: "d".repeat(40),
                        ..Default::default()
                    })
                }
                methods::GET_CHANGE_REQUEST_DIFF => {
                    assert_eq!(params["headRefOid"], "c".repeat(40));
                    assert_eq!(params["baseRefOid"], "d".repeat(40));
                    commutator_rpc::RpcReply::value(&"new patch")
                }
                _ => panic!("unexpected RPC"),
            }
        });
        let cache = Rc::new(RefCell::new(PullRequestCache::default()));
        let url = "https://github.com/a/b/pull/1";
        let mut cached = snapshot("Before push");
        cached.detail.head_ref_oid = "a".repeat(40);
        cached.diff = Some(ParsedDiff::new("old patch".into()));
        cache.borrow_mut().put(None, url.into(), cached);
        let (page, cx) = cx.add_window_view(|window, cx| {
            PullRequestDetailPage::new(
                fixture::state(cx, Some(rpc.client())),
                url.into(),
                None,
                cache.clone(),
                None,
                window,
                cx,
            )
        });
        page.update(cx, |page, cx| {
            page.comment_input
                .update(cx, |input, cx| input.set_text("Keep my draft", cx));
            page.select_tab(Tab::Timeline, cx);
            page.update_board_preview(board_preview(&"a".repeat(40)), cx);
            page.update_board_preview(board_preview(""), cx);
            assert!(!page.loading);
            page.update_board_preview(board_preview(&"b".repeat(40)), cx);
            assert!(page.loading && page.diff.is_none());
            // Repeated activation while loading must not start more reads.
            page.update_board_preview(board_preview(&"b".repeat(40)), cx);
        });
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| !page.loading)
        });
        assert_eq!(rpc.completed(), 1, "the diff waits for Code");
        page.update(cx, |page, cx| {
            assert!(page.tab == Tab::Timeline);
            assert_eq!(page.comment_input.read(cx).text(), "Keep my draft");
            assert_eq!(page.detail.as_ref().unwrap().head_ref_oid, "c".repeat(40));
            for _ in 0..3 {
                page.update_board_preview(board_preview(&"b".repeat(40)), cx);
                assert!(
                    !page.loading,
                    "the older board must not trigger a refresh loop"
                );
            }
            page.select_tab(Tab::Code, cx);
        });
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| page.diff.is_some())
        });
        page.read_with(cx, |page, cx| {
            assert_eq!(page.diff.as_ref().unwrap().patch, "new patch");
            assert_eq!(page.comment_input.read(cx).text(), "Keep my draft");
        });
        assert_eq!(rpc.completed(), 2);
    }

    #[gpui::test]
    fn pull_request_board_revision_during_a_read_is_not_lost(cx: &mut gpui::TestAppContext) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        fixture::init(cx);
        let release = Arc::new(tokio::sync::Notify::new());
        let reads = Arc::new(AtomicUsize::new(0));
        let rpc = ScriptedRpc::new({
            let release = release.clone();
            let reads = reads.clone();
            move |method, params| {
                let release = release.clone();
                let reads = reads.clone();
                async move {
                    assert_eq!(method, methods::GET_CHANGE_REQUEST);
                    let call = reads.fetch_add(1, Ordering::SeqCst);
                    let head = if call == 0 {
                        release.notified().await;
                        "b".repeat(40)
                    } else {
                        assert_eq!(call, 1, "only the newest deferred revision is read");
                        assert_eq!(params["headRefOid"], "c".repeat(40));
                        "c".repeat(40)
                    };
                    commutator_rpc::RpcReply::value(&ChangeRequestDetail {
                        head_ref_oid: head,
                        ..Default::default()
                    })
                }
            }
        });
        let cache = Rc::new(RefCell::new(PullRequestCache::default()));
        let url = "https://github.com/a/b/pull/1";
        let mut cached = snapshot("Before push");
        cached.detail.head_ref_oid = "a".repeat(40);
        cache.borrow_mut().put(None, url.into(), cached);
        let (page, cx) = cx.add_window_view(|window, cx| {
            PullRequestDetailPage::new(
                fixture::state(cx, Some(rpc.client())),
                url.into(),
                None,
                cache.clone(),
                None,
                window,
                cx,
            )
        });
        page.update(cx, |page, cx| {
            page.update_board_preview(board_preview(&"b".repeat(40)), cx);
        });
        rpc.settle(cx, &runtime, |_| reads.load(Ordering::SeqCst) == 1);
        page.update(cx, |page, cx| {
            page.update_board_preview(board_preview(&"c".repeat(40)), cx);
            assert!(page.loading && page.board_refresh_owed);
        });
        release.notify_one();
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| {
                !page.loading && page.detail.as_ref().unwrap().head_ref_oid == "c".repeat(40)
            })
        });
        assert_eq!(rpc.completed(), 2);
    }

    #[gpui::test]
    fn pull_request_board_refresh_waits_for_comment_and_does_not_retry_errors(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        fixture::init(cx);
        let rpc = ScriptedRpc::new(|method, params| async move {
            assert_eq!(method, methods::GET_CHANGE_REQUEST);
            assert_eq!(params["headRefOid"], "b".repeat(40));
            Err(commutator_rpc::RpcError::Failed("offline".into()))
        });
        let cache = Rc::new(RefCell::new(PullRequestCache::default()));
        let url = "https://github.com/a/b/pull/1";
        let mut cached = snapshot("Before push");
        cached.detail.head_ref_oid = "a".repeat(40);
        cache.borrow_mut().put(None, url.into(), cached);
        let (page, cx) = cx.add_window_view(|window, cx| {
            PullRequestDetailPage::new(
                fixture::state(cx, Some(rpc.client())),
                url.into(),
                None,
                cache.clone(),
                None,
                window,
                cx,
            )
        });
        let submission = page.update(cx, |page, cx| {
            page.comment_input
                .update(cx, |input, cx| input.set_text("Unsent comment", cx));
            let submission = cx.new(|_| CommentSubmission {
                body: "Unsent comment".into(),
                result: None,
            });
            page.observe_submission(submission.clone(), cx);
            page.update_board_preview(board_preview(&"b".repeat(40)), cx);
            assert!(!page.loading && page.board_refresh_owed);
            submission
        });
        assert_eq!(rpc.completed(), 0);
        submission.update(cx, |submission, cx| {
            submission.result = Some(Err("Comment failed".into()));
            cx.notify();
        });
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| page.error.is_some() && !page.loading)
        });
        page.update(cx, |page, cx| {
            assert!(page.submission.is_none());
            assert_eq!(page.comment_input.read(cx).text(), "Unsent comment");
            assert_eq!(page.comment_error.as_deref(), Some("Comment failed"));
            for _ in 0..3 {
                page.update_board_preview(board_preview(&"b".repeat(40)), cx);
                assert!(!page.loading);
            }
        });
        assert_eq!(rpc.completed(), 1);
    }

    #[gpui::test]
    fn pull_request_refresh_discards_pending_diff_in_either_response_order(
        cx: &mut gpui::TestAppContext,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        fixture::init(cx);
        for old_finishes_first in [true, false] {
            let release_old = Arc::new(tokio::sync::Notify::new());
            let release_detail = Arc::new(tokio::sync::Notify::new());
            let diff_calls = Arc::new(AtomicUsize::new(0));
            let rpc = ScriptedRpc::new({
                let release_old = release_old.clone();
                let release_detail = release_detail.clone();
                let diff_calls = diff_calls.clone();
                move |method, params| {
                    let release_old = release_old.clone();
                    let release_detail = release_detail.clone();
                    let diff_calls = diff_calls.clone();
                    async move {
                        match method.as_str() {
                            methods::GET_CHANGE_REQUEST => {
                                release_detail.notified().await;
                                commutator_rpc::RpcReply::value(&ChangeRequestDetail {
                                    head_ref_oid: "b".repeat(40),
                                    base_ref_oid: "d".repeat(40),
                                    ..Default::default()
                                })
                            }
                            methods::GET_CHANGE_REQUEST_DIFF => {
                                let call = diff_calls.fetch_add(1, Ordering::SeqCst);
                                if call == 0 {
                                    assert_eq!(params["headRefOid"], "a".repeat(40));
                                    release_old.notified().await;
                                    commutator_rpc::RpcReply::value(&"outdated patch")
                                } else {
                                    assert_eq!(call, 1);
                                    assert_eq!(params["headRefOid"], "b".repeat(40));
                                    assert_eq!(params["baseRefOid"], "d".repeat(40));
                                    assert_eq!(params["refresh"], true);
                                    commutator_rpc::RpcReply::value(&"current patch")
                                }
                            }
                            _ => panic!("unexpected RPC"),
                        }
                    }
                }
            });
            let cache = Rc::new(RefCell::new(PullRequestCache::default()));
            let url = "https://github.com/a/b/pull/1";
            let mut cached = snapshot("Before push");
            cached.detail.head_ref_oid = "a".repeat(40);
            cached.detail.base_ref_oid = "c".repeat(40);
            cache.borrow_mut().put(None, url.into(), cached);
            let window = cx.add_window(|window, cx| {
                PullRequestDetailPage::new(
                    fixture::state(cx, Some(rpc.client())),
                    url.into(),
                    None,
                    cache.clone(),
                    None,
                    window,
                    cx,
                )
            });
            window
                .update(cx, |page, _, cx| page.select_tab(Tab::Code, cx))
                .unwrap();
            rpc.settle(cx, &runtime, |_| diff_calls.load(Ordering::SeqCst) == 1);
            window.update(cx, |page, _, cx| page.refresh(cx)).unwrap();
            if old_finishes_first {
                release_old.notify_one();
                rpc.settle(cx, &runtime, |_| rpc.completed() == 1);
                window
                    .update(cx, |page, _, _| {
                        assert!(page.loading && page.diff.is_none());
                    })
                    .unwrap();
            }
            release_detail.notify_one();
            rpc.settle(cx, &runtime, |cx| {
                window.update(cx, |page, _, _| page.diff.is_some()).unwrap()
            });
            if !old_finishes_first {
                release_old.notify_one();
                rpc.settle(cx, &runtime, |_| rpc.completed() == 3);
            }
            window
                .update(cx, |page, window, _| {
                    assert_eq!(page.diff.as_ref().unwrap().patch, "current patch");
                    assert_eq!(page.detail.as_ref().unwrap().head_ref_oid, "b".repeat(40));
                    let snapshot = cache.borrow_mut().get(&None, url).unwrap();
                    assert_eq!(snapshot.diff.unwrap().patch, "current patch");
                    assert_eq!(snapshot.detail.head_ref_oid, "b".repeat(40));
                    assert!(!snapshot.diff_refresh_owed);
                    window.remove_window();
                })
                .unwrap();
            cx.run_until_parked();
            assert_eq!(diff_calls.load(Ordering::SeqCst), 2);
        }
    }

    #[gpui::test]
    fn pull_request_refresh_survives_reopening_and_a_failed_diff(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        fixture::init(cx);
        let diffs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls = diffs.clone();
        let rpc = ScriptedRpc::new(move |method, params| {
            let calls = calls.clone();
            async move {
                match method.as_str() {
                    methods::GET_CHANGE_REQUEST => {
                        commutator_rpc::RpcReply::value(&ChangeRequestDetail {
                            title: "Fresh".into(),
                            head_ref_oid: "new-head".into(),
                            ..Default::default()
                        })
                    }
                    methods::GET_CHANGE_REQUEST_DIFF => {
                        let mut calls = calls.lock().unwrap();
                        let refresh = params["refresh"] == true;
                        calls.push(refresh);
                        if calls.len() == 1 {
                            return Err(commutator_rpc::RpcError::Failed("offline".into()));
                        }
                        // A non-refresh request before a successful refresh
                        // sees the engine's old cached patch.
                        let patch = if refresh || calls.len() > 2 {
                            "new patch"
                        } else {
                            "old patch"
                        };
                        commutator_rpc::RpcReply::value(&patch)
                    }
                    other => Err(commutator_rpc::RpcError::UnknownMethod(other.into())),
                }
            }
        });
        let url = "https://github.com/a/b/pull/1";
        let cache = Rc::new(RefCell::new(PullRequestCache::default()));
        let mut cached = snapshot("Cached");
        cached.detail.head_ref_oid = "old-head".into();
        cached.diff = Some(ParsedDiff::new("old patch".into()));
        cache.borrow_mut().put(None, url.into(), cached);
        let state = cx.update(|cx| fixture::state(cx, Some(rpc.client())));
        let open = |cx: &mut gpui::TestAppContext| {
            cx.add_window(|window, cx| {
                PullRequestDetailPage::new(
                    state.clone(),
                    url.into(),
                    None,
                    cache.clone(),
                    None,
                    window,
                    cx,
                )
            })
        };
        let window = open(cx);
        window
            .update(cx, |page, _, cx| {
                page.select_tab(Tab::Code, cx);
                assert_eq!(page.diff.as_ref().unwrap().patch, "old patch");
                page.select_tab(Tab::Summary, cx);
                page.refresh(cx);
            })
            .unwrap();
        rpc.settle(cx, &runtime, |cx| {
            window.update(cx, |page, _, _| !page.loading).unwrap()
        });
        window
            .update(cx, |page, window, _| {
                assert_eq!(page.detail.as_ref().unwrap().head_ref_oid, "new-head");
                window.remove_window();
            })
            .unwrap();
        cx.run_until_parked();

        // Reopen, fail to fetch the refreshed diff, then reopen and retry.
        // Neither destroying the view nor an error may consume invalidation.
        for succeeds in [false, true] {
            let window = open(cx);
            window
                .update(cx, |page, _, cx| {
                    assert_eq!(page.detail.as_ref().unwrap().head_ref_oid, "new-head");
                    assert!(page.diff.is_none());
                    page.select_tab(Tab::Code, cx);
                })
                .unwrap();
            rpc.settle(cx, &runtime, |cx| {
                window
                    .update(cx, |page, _, _| page.diff_task.is_none())
                    .unwrap()
            });
            window
                .update(cx, |page, window, _| {
                    if succeeds {
                        assert_eq!(page.diff.as_ref().unwrap().patch, "new patch");
                    } else {
                        assert!(page.diff_error.is_some());
                    }
                    window.remove_window();
                })
                .unwrap();
            cx.run_until_parked();
        }
        let window = open(cx);
        window
            .update(cx, |page, _, cx| {
                assert_eq!(page.diff.as_ref().unwrap().patch, "new patch");
                page.select_tab(Tab::Code, cx);
                assert!(
                    page.diff_task.is_none(),
                    "reopening reuses the fresh parsed patch"
                );
                page.select_tab(Tab::Summary, cx);
                page.diff = None;
                page.select_tab(Tab::Code, cx);
            })
            .unwrap();
        rpc.settle(cx, &runtime, |cx| {
            window
                .update(cx, |page, _, _| page.diff_task.is_none())
                .unwrap()
        });
        assert_eq!(
            *diffs.lock().unwrap(),
            [true, true, false],
            "invalidation persists until a successful diff load"
        );
    }

    #[test]
    fn pull_request_failures_read_as_words_not_codes() {
        use commutator_rpc::{RpcError, capability_errors as codes};
        for error in [
            RpcError::Capability(codes::PULL_REQUESTS_AUTHENTICATION.into()),
            RpcError::Capability(codes::PULL_REQUESTS_DECODE.into()),
            RpcError::Transport("socket reset by peer".into()),
            RpcError::UnknownMethod(methods::GET_CHANGE_REQUEST.into()),
        ] {
            let reason = failure_reason(&error);
            assert!(!reason.contains("pull_requests."), "{reason}");
            assert!(!reason.contains("socket"), "{reason}");
            assert!(!reason.contains("GetChangeRequest"), "{reason}");
        }
        assert_eq!(
            failure_reason(&RpcError::Capability(
                codes::PULL_REQUESTS_AUTHENTICATION.into()
            )),
            "Run gh auth login on this device, then refresh."
        );
    }

    #[test]
    fn pr_view_reads_checks_reviewers_and_timeline() {
        use commutator_proto::{
            ChangeRequestActor, ChangeRequestCheck, ChangeRequestComment, ChangeRequestCommit,
        };
        let actor = |login: &str| ChangeRequestActor {
            login: login.into(),
            ..Default::default()
        };
        let check = |conclusion: &str, status: &str| ChangeRequestCheck {
            name: "ci".into(),
            conclusion: conclusion.into(),
            status: status.into(),
            ..Default::default()
        };
        assert_eq!(check_state(&check("FAILURE", "COMPLETED")), CiState::Failed);
        assert_eq!(check_state(&check("SUCCESS", "COMPLETED")), CiState::Passed);
        assert_eq!(check_state(&check("", "IN_PROGRESS")), CiState::Pending);
        let review = |who: &str, state: &str, at: &str| ChangeRequestComment {
            author: actor(who),
            state: state.into(),
            submitted_at: at.into(),
            ..Default::default()
        };
        let commit = |oid: &str, at: &str| ChangeRequestCommit {
            oid: oid.into(),
            message_headline: format!("commit {oid}"),
            committed_date: at.into(),
            authors: vec![actor("ann")],
        };
        let detail = ChangeRequestDetail {
            author: actor("ann"),
            created_at: "2026-01-01T00:00:00Z".into(),
            reviews: vec![
                review("bob", "CHANGES_REQUESTED", "2026-01-02T00:00:00Z"),
                review("bob", "COMMENTED", "2026-01-03T00:00:00Z"),
                review("cy", "APPROVED", "2026-01-04T00:00:00Z"),
            ],
            review_requests: vec![actor("dee"), actor("cy")],
            commits: vec![
                commit("aaaaaaaa", "2026-01-01T01:00:00Z"),
                commit("bbbbbbbb", "2026-01-01T02:00:00Z"),
            ],
            merged_at: "2026-01-05T00:00:00Z".into(),
            status_check_rollup: vec![check("FAILURE", "COMPLETED"), check("SUCCESS", "COMPLETED")],
            ..Default::default()
        };
        // A comment after a verdict doesn't take it back; asked reviewers who
        // already reviewed aren't listed twice.
        assert_eq!(
            reviewers(&detail),
            [
                ("bob".into(), Verdict::Changes),
                ("cy".into(), Verdict::Approved),
                ("dee".into(), Verdict::Pending)
            ]
        );
        let events = timeline(&detail);
        let kinds: Vec<_> = events
            .iter()
            .map(|e| std::mem::discriminant(&e.what))
            .collect();
        assert_eq!(
            events.len(),
            5,
            "opened, one push, two verdicts, merged: {events:?}"
        );
        assert_eq!(kinds[0], std::mem::discriminant(&Happened::Opened));
        assert_eq!(
            events[1].what,
            Happened::Commits(vec![
                ("aaaaaaa".into(), "commit aaaaaaaa".into()),
                ("bbbbbbb".into(), "commit bbbbbbbb".into()),
            ])
        );
        assert_eq!(events[4].what, Happened::Merged);
    }

    #[test]
    fn pull_request_code_rows_preserve_sides_and_file_jump_offsets() {
        let patch = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n context\n-old\n+new\n";
        let (rows, files) = code_rows(patch);
        assert_eq!(files, vec![("a.rs".into(), 0)]);
        let added = rows
            .iter()
            .find(|row| row.kind == crate::changes::LineKind::Add)
            .unwrap();
        assert_eq!(added.old, None);
        assert_eq!(added.new, Some(2));
        let removed = rows
            .iter()
            .find(|row| row.kind == crate::changes::LineKind::Del)
            .unwrap();
        assert_eq!(removed.old, Some(2));
        assert_eq!(removed.new, None);
    }

    #[test]
    fn pull_request_diff_reuses_syntax_highlighting() {
        let (rows, _) = code_rows(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-fn old() {}\n+fn new() {}\n",
        );
        assert!(
            rows.iter()
                .filter(|row| row.kind != crate::changes::LineKind::Meta)
                .all(|row| !row.spans.is_empty())
        );
    }

    #[test]
    fn pull_request_links_resolve_to_their_pull_request() {
        for link in [
            "https://github.com/a/b/pull/12",
            "https://github.com/a/b/pull/12/files",
            "https://github.com/a/b/pull/12#discussion_r1",
            "https://github.com/a/b/pull/12/?tab=checks",
        ] {
            assert_eq!(
                pull_request_link(link).as_deref(),
                Some("https://github.com/a/b/pull/12"),
                "{link}"
            );
        }
        for link in [
            "http://github.com/a/b/pull/12",
            "https://gitlab.com/a/b/pull/12",
            "https://github.com/a/b/issues/12",
            "https://github.com/a/b/pull/0",
            "https://github.com/a/b/pull/x",
            "https://github.com/a/b",
            "https://user@github.com/a/b/pull/12",
            "https://github.com:8443/a/b/pull/12",
        ] {
            assert_eq!(pull_request_link(link), None, "{link}");
        }
    }

    #[gpui::test]
    fn pull_request_code_opens_on_the_saved_split_preference(cx: &mut gpui::TestAppContext) {
        let mut saved = settings::UiSettings::default();
        saved.diff_split = true;
        let _settings = fixture::settings(cx, saved);
        let (host, cx) = cx.add_window_view(|window, cx| DetailHost::new(window, cx, true));
        let page = host.read_with(cx, |host, _| host.page.clone());
        page.update(cx, |page, cx| {
            assert!(page.review.split);
            let patch =
                "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
            page.install_diff(ParsedDiff::new(patch.into()), cx);
            page.select_tab(Tab::Code, cx);
            assert!(
                page.review
                    .stream
                    .iter()
                    .any(|row| matches!(row, code::StreamRow::Pair(..))),
                "the first visit renders split rows"
            );
        });
    }

    #[gpui::test]
    fn pull_request_code_stream_navigates_folds_and_follows_layout(cx: &mut gpui::TestAppContext) {
        fixture::init(cx);
        let (host, cx) = cx.add_window_view(|window, cx| DetailHost::new(window, cx, true));
        let page = host.read_with(cx, |host, _| host.page.clone());
        page.update(cx, |page, cx| {
            let patch = ["a.rs", "nested/b.rs", "c.rs"].iter().map(|name| format!(
                "diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -1 +1 @@\n-old\n+{}\n", "new ".repeat(100)
            )).collect::<String>();
            page.install_diff(ParsedDiff::new(patch), cx);
            let cached = page.diff.clone().unwrap();
            page.select_tab(Tab::Code, cx);
            assert_eq!(page.review.ranges.len(), 3, "every file stays in the stream");
            for index in [2, 0, 1] {
                page.select_code_file(index, cx);
                assert_eq!(page.active_file(), Some(index));
                assert!(Arc::ptr_eq(&cached, page.diff.as_ref().unwrap()));
            }
            page.select_code_file(99, cx);
            assert_eq!(page.active_file(), Some(1));
        });
        cx.simulate_resize(gpui::size(px(1200.0), px(850.0)));
        page.update(cx, |page, cx| {
            page.file_search
                .update(cx, |input, cx| input.set_text("NESTED", cx));
        });
        cx.run_until_parked();
        page.read_with(cx, |page, _| assert_eq!(page.file_query, "nested"));
        assert!(cx.debug_bounds("pr-file-navigation").is_some());
        // Root files lead, so `nested/b.rs` is the last file.
        assert!(cx.debug_bounds("pr-file-0").is_none());
        assert!(cx.debug_bounds("pr-file-1").is_none());
        assert!(cx.debug_bounds("pr-file-2").is_some());
        page.update(cx, |page, cx| {
            page.file_search
                .update(cx, |input, cx| input.set_text("", cx));
        });
        for (width, height) in [(320.0, 600.0), (900.0, 400.0), (1200.0, 850.0)] {
            cx.simulate_resize(gpui::size(px(width), px(height)));
            cx.run_until_parked();
            let viewport = cx.debug_bounds("pr-code-viewport").unwrap();
            let toolbar = cx.debug_bounds("pr-file-navigation").unwrap();
            assert!(viewport.size.height >= px(60.0), "{viewport:?}");
            assert!(viewport.bottom() <= px(height));
            assert!(toolbar.bottom() <= viewport.top());
            assert_eq!(
                toolbar.left(),
                viewport.left(),
                "toolbar and stream share an edge"
            );
            let scroll = cx.debug_bounds("pr-code-scroll").unwrap();
            // Rows run flush to the card's 1px border; its rounded clip
            // handles the corners.
            assert_eq!(scroll.left(), viewport.left() + px(1.0));
            assert_eq!(scroll.right(), viewport.right() - px(1.0));
            assert_eq!(scroll.top(), viewport.top() + px(1.0));
            assert_eq!(scroll.bottom(), viewport.bottom() - px(1.0));
            let header = cx.debug_bounds("pr-file-header-0").unwrap();
            assert_eq!(header.size.height, px(crate::changes::FILE_HEADER_HEIGHT));
            if width >= 900.0 {
                let browser = cx.debug_bounds("pr-file-browser").unwrap();
                assert!(
                    browser.right() < viewport.left(),
                    "files stay beside the diff"
                );
                assert_eq!(
                    browser.top(),
                    toolbar.top(),
                    "tree header aligns with the toolbar"
                );
                let search = cx.debug_bounds("pr-file-search").unwrap();
                assert_eq!(search.size.height, px(crate::surface_chrome::CONTROL_SIZE));
                let files = cx.debug_bounds("pr-file-list").unwrap();
                assert!(files.top() >= search.bottom() && files.size.height > px(40.0));
                assert!(
                    files.bottom() > viewport.bottom(),
                    "the tree runs to the window edge while the diff clears the navigation"
                );
                assert!(cx.debug_bounds("pr-files").is_none());
            } else {
                assert!(
                    cx.debug_bounds("pr-file-browser").is_none(),
                    "compact stream keeps its width"
                );
                let files = cx.debug_bounds("pr-files").unwrap();
                cx.simulate_mouse_down(
                    files.center(),
                    gpui::MouseButton::Left,
                    gpui::Modifiers::default(),
                );
                cx.simulate_mouse_up(
                    files.center(),
                    gpui::MouseButton::Left,
                    gpui::Modifiers::default(),
                );
                cx.run_until_parked();
                let picker = cx.debug_bounds("pr-file-picker").unwrap();
                assert!(picker.top() >= toolbar.bottom() && picker.right() <= px(width));
                let file = cx.debug_bounds("pr-file-2").unwrap();
                cx.simulate_mouse_down(
                    file.center(),
                    gpui::MouseButton::Left,
                    gpui::Modifiers::default(),
                );
                cx.simulate_mouse_up(
                    file.center(),
                    gpui::MouseButton::Left,
                    gpui::Modifiers::default(),
                );
                cx.run_until_parked();
                page.read_with(cx, |page, _| {
                    assert_eq!(page.active_file(), Some(2));
                    assert!(!page.files_expanded, "picking a file closes the picker");
                });
            }
            let toggle = cx.debug_bounds("pr-split").unwrap();
            cx.simulate_mouse_down(
                toggle.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            cx.simulate_mouse_up(
                toggle.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            page.update(cx, |page, cx| page.select_code_file(0, cx));
            cx.run_until_parked();
            page.read_with(cx, |page, _| assert!(page.review.split));
            // Stream rows: header, hunk, then the first paired line.
            let left = cx.debug_bounds("pr-split-cell-2-true").unwrap();
            let right = cx.debug_bounds("pr-split-cell-2-false").unwrap();
            assert!(left.left() >= viewport.left());
            assert!(right.right() <= viewport.right());
            assert!((left.size.width - right.size.width).abs() < px(1.0));
            assert!(left.right() <= right.left());
            cx.simulate_mouse_down(
                toggle.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            cx.simulate_mouse_up(
                toggle.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            cx.run_until_parked();
            page.read_with(cx, |page, _| assert!(!page.review.split));
            for selector in [
                "pr-copy-patch",
                "pr-previous-file",
                "pr-next-file",
                "pr-split",
                "pr-fold-all",
            ] {
                let control = cx.debug_bounds(selector).unwrap();
                assert!(
                    control.left() >= px(PAD) && control.right() <= px(width - PAD),
                    "{selector}: {control:?}"
                );
            }
        }
        let file = cx.debug_bounds("pr-file-2").unwrap();
        cx.simulate_mouse_down(
            file.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            file.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| assert_eq!(page.active_file(), Some(2)));
        assert!(
            cx.debug_bounds("pr-file-browser").is_some(),
            "wide file navigator remains available after selection"
        );
        page.update(cx, |page, cx| page.select_code_file(0, cx));
        cx.run_until_parked();
        let header = cx.debug_bounds("pr-file-header-0").unwrap();
        cx.simulate_mouse_down(
            header.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            header.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(page.review.collapsed.contains(&0));
            assert_eq!(
                page.review.ranges[0],
                0..1,
                "a folded file keeps only its header"
            );
        });
        let fold_all = cx.debug_bounds("pr-fold-all").unwrap();
        cx.simulate_mouse_down(
            fold_all.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            fold_all.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| assert_eq!(page.review.stream.len(), 3));
    }

    #[gpui::test]
    fn pull_request_file_jump_hides_the_header_divider_above_the_scroll_viewport(
        cx: &mut gpui::TestAppContext,
    ) {
        fixture::init(cx);
        let (host, cx) = cx.add_window_view(|window, cx| DetailHost::new(window, cx, true));
        let page = host.read_with(cx, |host, _| host.page.clone());
        cx.simulate_resize(gpui::size(px(1200.0), px(700.0)));
        page.update(cx, |page, cx| {
            let body = (0..80).map(|n| format!("+line {n}\n")).collect::<String>();
            let patch = ["a.rs", "b.rs", "c.rs"]
                .iter()
                .map(|name| format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -0,0 +1,80 @@\n{body}"))
                .collect::<String>();
            page.install_diff(ParsedDiff::new(patch), cx);
            page.select_tab(Tab::Code, cx);
        });
        cx.run_until_parked();
        page.update(cx, |page, cx| page.select_code_file(1, cx));
        cx.run_until_parked();
        let viewport = cx.debug_bounds("pr-code-scroll").unwrap();
        let header = cx.debug_bounds("pr-file-header-1").unwrap();
        assert_eq!(
            header.top() + px(1.0),
            viewport.top(),
            "the jumped-to header's divider hides above the scroll viewport"
        );
        assert!(
            cx.debug_bounds("pr-sticky-file-1").is_none(),
            "no pinned copy doubles the header"
        );
    }

    struct DetailHost {
        page: Entity<PullRequestDetailPage>,
        _subscription: Subscription,
        returned: bool,
    }

    impl DetailHost {
        fn new(window: &mut Window, cx: &mut Context<Self>, loaded: bool) -> Self {
            Self::build(window, cx, loaded, None)
        }

        fn with_client(
            window: &mut Window,
            cx: &mut Context<Self>,
            client: Option<commutator_rpc::RpcClient>,
        ) -> Self {
            Self::build(window, cx, true, client)
        }

        fn build(
            window: &mut Window,
            cx: &mut Context<Self>,
            loaded: bool,
            client: Option<commutator_rpc::RpcClient>,
        ) -> Self {
            let state = fixture::state(cx, client);
            let page = cx.new(|cx| {
                let mut page = PullRequestDetailPage::new(
                    state, "https://github.com/a/b/pull/1".into(),
                    Some("remote-device".into()),
                    Rc::new(RefCell::new(PullRequestCache::default())), None, window, cx,
                );
                if loaded {
                    page.error = None;
                    page.detail = Some(ChangeRequestDetail {
                        title: "Inspect a pull request with a very long title that must leave room for all actions".into(),
                        number: 1, body: "A description".into(), state: "OPEN".into(),
                        url: "https://github.com/a/b/pull/1".into(),
                        status_check_rollup: vec![commutator_proto::ChangeRequestCheck {
                            name: "linux-browser".into(), conclusion: "SUCCESS".into(),
                            details_url: "https://github.com/a/b/actions/runs/1".into(),
                            ..Default::default()
                        }], ..Default::default()
                    });
                    page.body = Some(crate::markdown::parse_full("A description"));
                    let review = "### Review notes\n\n**Strong** text and [a link](https://github.com).\n\n```rust\nlet answer = 42;\n```";
                    page.detail.as_mut().unwrap().reviews.push(commutator_proto::ChangeRequestComment {
                        body: review.into(), ..Default::default()
                    });
                    page.detail.as_mut().unwrap().comments.push(commutator_proto::ChangeRequestComment {
                        viewer_did_author: true, body: "My comment".into(),
                        author: commutator_proto::ChangeRequestActor { login: "viewer".into(), ..Default::default() }, ..Default::default()
                    });
                    page.activity_bodies = vec![
                        super::super::pull_request_media::parse_description("My comment"),
                        super::super::pull_request_media::parse_description(review),
                    ];
                    let patch = format!("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+{}\n", "long_expression_".repeat(30));
                    page.install_diff(ParsedDiff::new(patch), cx);
                }
                page
            });
            let subscription = cx.observe(&page, |_, _, cx| cx.notify());
            Self {
                page,
                _subscription: subscription,
                returned: false,
            }
        }
    }

    impl Render for DetailHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // The pull request pane's shape: the toolbar row, then the page.
            let header = self.page.update(cx, |page, cx| page.toolbar(cx));
            let theme = Theme::of(cx).clone();
            div()
                .size_full()
                .flex()
                .flex_col()
                .on_action(cx.listener(|host, _: &ClosePullRequest, _, cx| {
                    host.returned = true;
                    cx.notify();
                }))
                .child(crate::surface_chrome::toolbar(&theme).child(header))
                .child(div().flex_1().min_h_0().child(self.page.clone()))
        }
    }

    #[gpui::test]
    fn pr_view_tabs_switch_and_the_footer_approves(cx: &mut gpui::TestAppContext) {
        fixture::init(cx);
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let acted = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = acted.clone();
        let rpc = ScriptedRpc::new(move |method, params| {
            let seen = seen.clone();
            async move {
                if method == methods::ACT_ON_CHANGE_REQUEST {
                    seen.lock().unwrap().push(params);
                    return commutator_rpc::RpcReply::value(&serde_json::json!({}));
                }
                Err(commutator_rpc::RpcError::Failed("offline".into()))
            }
        });
        let client = rpc.client();
        let (host, cx) =
            cx.add_window_view(|window, cx| DetailHost::with_client(window, cx, Some(client)));
        let page = host.read_with(cx, |host, _| host.page.clone());
        cx.simulate_resize(gpui::size(px(520.0), px(800.0)));
        cx.run_until_parked();
        for selector in [
            "pr-head",
            "pr-detail-title",
            "pr-summary",
            "pr-description",
            "pr-approve",
            "pr-merge",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let click = |cx: &mut gpui::VisualTestContext, selector: &'static str| {
            let at = cx.debug_bounds(selector).unwrap().center();
            cx.simulate_click(at, gpui::Modifiers::default());
            cx.run_until_parked();
        };
        click(cx, "pr-tab-Timeline");
        page.read_with(cx, |page, _| assert_eq!(page.tab, Tab::Timeline));
        click(cx, "pr-tab-Checks");
        assert!(cx.debug_bounds("pr-check-0").is_some());
        click(cx, "pr-approve");
        rpc.settle(cx, &runtime, |_| !acted.lock().unwrap().is_empty());
        let params = acted.lock().unwrap()[0].clone();
        assert_eq!(params["action"], "approve");
        assert_eq!(params["url"], "https://github.com/a/b/pull/1");
        assert_eq!(params["targetDeviceId"], "remote-device");
    }

    #[gpui::test]
    fn pull_request_error_keeps_retry_and_browser_actions(cx: &mut gpui::TestAppContext) {
        fixture::init(cx);
        let (_, cx) = cx.add_window_view(|window, cx| DetailHost::new(window, cx, false));
        cx.simulate_resize(gpui::size(px(320.0), px(800.0)));
        cx.run_until_parked();
        for selector in ["pr-detail-refresh", "pr-external"] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
    }
}
