//! Settings → Browser: the Chrome extensions in the browser tabs
//! (`crate::browser::extensions`), each a row with its icon, name, source and
//! version, a switch and a remove button behind a confirmation. The Install
//! dialog takes a Chrome Web Store link or id, or a folder.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Entity, Focusable, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};

use crate::browser::extensions as bext;
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover;
use crate::settings::{self, BrowserExtension, SavePolicy, widgets};
use crate::theme::Theme;

/// An extension's manifest as read for its row, with its icon ready to draw;
/// or why it couldn't be read.
type Shown = Result<(bext::Info, Option<Arc<gpui::Image>>), String>;

/// Adds (or re-adds, switched on) the extension unpacked in `path`. Also used
/// when a Web Store page's "Add to Commutator" installs one.
pub fn add_extension(cx: &mut App, path: PathBuf) {
    settings::update(SavePolicy::Immediate, cx, |s| {
        s.browser_extensions.retain(|e| e.path != path);
        s.browser_extensions.push(BrowserExtension { path, on: true });
    });
}

struct InstallDialog {
    input: Entity<ComposerInput>,
    note: Option<String>,
    _events: Subscription,
}

pub struct BrowserSettingsPage {
    scroll: widgets::PageScroll,
    info: HashMap<PathBuf, Shown>,
    install: Option<InstallDialog>,
    /// A Web Store download is running.
    busy: bool,
    /// The extension whose row asks "Remove …?".
    remove: Option<PathBuf>,
    note: Option<SharedString>,
    task: Option<Task<()>>,
}

impl BrowserSettingsPage {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            scroll: widgets::PageScroll::default(),
            info: HashMap::new(),
            install: None,
            busy: false,
            remove: None,
            note: None,
            task: None,
        }
    }

    /// Escape that reached Settings unclaimed closes the dialog or the remove
    /// confirmation first. Returns whether it did.
    pub(crate) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.install.take().is_none() && self.remove.take().is_none() {
            return false;
        }
        cx.notify();
        true
    }

    /// Reads the manifest of each extension not read yet: small files, once a
    /// visit (and again after an install).
    fn load_info(&mut self, list: &[BrowserExtension]) {
        for e in list {
            self.info.entry(e.path.clone()).or_insert_with(|| {
                bext::info(&e.path).map(|info| {
                    let icon = info.icon.clone().map(|png| {
                        Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png))
                    });
                    (info, icon)
                })
            });
        }
    }

    fn open_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| ComposerInput::new("chromewebstore.google.com/detail/…", cx));
        let events = cx.subscribe(&input, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.add_from_store(cx);
            }
        });
        window.focus(&input.focus_handle(cx), cx);
        self.install = Some(InstallDialog {
            input,
            note: None,
            _events: events,
        });
        cx.notify();
    }

    fn set_install_note(&mut self, note: String) {
        match self.install.as_mut() {
            Some(dialog) => dialog.note = Some(note),
            None => self.note = Some(note.into()),
        }
    }

    /// The dialog's Install: downloads the extension the field names from the
    /// Web Store.
    fn add_from_store(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.install.as_mut() else {
            return;
        };
        if self.busy {
            return;
        }
        let Some(id) = bext::store_id(dialog.input.read(cx).text()) else {
            dialog.note =
                Some("Paste a Chrome Web Store link, or the extension's id (32 letters).".into());
            return cx.notify();
        };
        (self.busy, dialog.note) = (true, None);
        let download = cx.background_spawn(async move { bext::install_from_store(&id) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = download.await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match result {
                    Ok(path) => page.added(path, cx),
                    Err(why) => page.set_install_note(why),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Choose folder: an unpacked extension (a folder with its
    /// `manifest.json`), loaded from where it is.
    fn add_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add Extension".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |page, cx| {
                match bext::info(&path) {
                    Ok(_) => page.added(path, cx),
                    Err(why) => page.set_install_note(why),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn added(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.info.remove(&path);
        let name = bext::info(&path).map(|i| i.name).unwrap_or_default();
        add_extension(cx, path);
        self.note = Some(format!("Added {name}. It runs in browser tabs from their next load.").into());
        self.install = None;
    }

    fn render_install_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let dialog = self.install.as_ref()?;
        let input = dialog.input.clone();
        let note = dialog.note.clone();
        let busy = self.busy;
        let button = |id: &'static str, tone, label: &'static str| {
            widgets::text_action(&theme, tone, label)
                .id(id)
                .tab_index(0)
                .role(gpui::Role::Button)
                .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
        };
        let card = popover::dialog_card(&theme)
            .w(px(420.0))
            .id("browser-extension-install-card")
            .role(gpui::Role::Dialog)
            .aria_label("Install a browser extension")
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.install = None;
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(popover::dialog_title(&theme, "Install a browser extension"))
            .child(popover::dialog_body(
                &theme,
                "Paste its Chrome Web Store link, or its id. It runs in every browser tab with all it asks for, so install only what you trust.",
            ).mt(px(8.0)))
            .child(
                div()
                    .mt(px(12.0))
                    .child(popover::dialog_field(input.into_any_element())),
            )
            .when_some(note, |el, note| {
                el.child(
                    div()
                        .mt(px(8.0))
                        .text_size(crate::typography::ui_rems(12.0))
                        .line_height(px(16.0))
                        .text_color(theme.danger)
                        .child(SharedString::from(note)),
                )
            })
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_row()
                    .gap(px(8.0))
                    .child(
                        button("browser-extension-folder", widgets::ActionTone::Quiet, "Choose folder…")
                            .on_click(cx.listener(|this, _, _, cx| this.add_folder(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        button("browser-extension-cancel", widgets::ActionTone::Quiet, "Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.install = None;
                                cx.notify();
                            })),
                    )
                    .child(if busy {
                        button("browser-extension-install", widgets::ActionTone::Solid, "Installing…")
                            .opacity(0.6)
                    } else {
                        button("browser-extension-install", widgets::ActionTone::Solid, "Install")
                            .on_click(cx.listener(|this, _, _, cx| this.add_from_store(cx)))
                    }),
            )
            .into_any_element();
        Some(popover::modal("browser-extension-install-dialog", viewport, card))
    }

    /// One extension: its icon, name, where it's from and its version, what it
    /// does (or why it didn't load), its switch and the remove button.
    fn row(&self, ix: usize, e: &BrowserExtension, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (name, version, description, icon) = match self.info.get(&e.path) {
            Some(Ok((info, icon))) => (
                info.name.clone(),
                info.version.clone(),
                info.description.clone(),
                icon.clone(),
            ),
            Some(Err(why)) => (
                e.path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                String::new(),
                why.clone(),
                None,
            ),
            None => Default::default(),
        };
        let tile = div()
            .flex_none()
            .size(px(32.0))
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center();
        let tile = match icon {
            Some(image) => tile.child(gpui::img(image).size(px(28.0))),
            None => tile
                .bg(theme.wash(0.08))
                .text_color(theme.text_muted)
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(SharedString::from(
                    name.chars().next().map(String::from).unwrap_or_default(),
                )),
        };
        let row = widgets::card_row(theme, ix == 0).child(tile);
        if self.remove.as_ref() == Some(&e.path) {
            let path = e.path.clone();
            let body = if path.starts_with(bext::dir()) {
                "Its files go with it."
            } else {
                "Its folder stays where it is."
            };
            return row
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.0))
                        .child(widgets::row_title(theme, format!("Remove {name}?")))
                        .child(widgets::meta_line(
                            theme,
                            vec![div().child(SharedString::from(body)).into_any_element()],
                        )),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            widgets::text_action(theme, widgets::ActionTone::Quiet, "Cancel")
                                .id(("browser-extension-remove-cancel", ix))
                                .tab_index(0)
                                .role(gpui::Role::Button)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.remove = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            widgets::text_action(theme, widgets::ActionTone::Danger, "Remove")
                                .id(("browser-extension-remove-yes", ix))
                                .tab_index(0)
                                .role(gpui::Role::Button)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove = None;
                                    let path = path.clone();
                                    settings::update(SavePolicy::Immediate, cx, |s| {
                                        s.browser_extensions.retain(|e| e.path != path)
                                    });
                                    bext::remove_files(&path);
                                    this.info.remove(&path);
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element();
        }
        let from = if e.path.starts_with(bext::dir()) {
            "Chrome Web Store".to_string()
        } else {
            let home = std::env::var_os("HOME").map(PathBuf::from);
            home.and_then(|home| e.path.strip_prefix(home).ok().map(|rest| format!("~/{}", rest.display())))
                .unwrap_or_else(|| e.path.display().to_string())
        };
        let mut meta = vec![div().child(SharedString::from(from)).into_any_element()];
        if !version.is_empty() {
            meta.push(div().child(SharedString::from(format!("v{version}"))).into_any_element());
        }
        let line = match e.on.then(|| bext::error(&e.path)).flatten() {
            Some(why) => Some((format!("Didn't load: {why}"), theme.danger)),
            None => (!description.is_empty()).then(|| (description, theme.text_muted)),
        };
        let on = e.on;
        let power_path = e.path.clone();
        let remove_path = e.path.clone();
        row.child(
            div()
                .flex_1()
                .min_w(px(160.0))
                .child(widgets::row_title(theme, name.clone()))
                .child(widgets::meta_line(theme, meta))
                .when_some(line, |el, (text, color)| {
                    el.child(
                        div()
                            .mt(px(2.0))
                            .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                            .line_height(crate::typography::ui_rems(16.0))
                            .text_color(color)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(SharedString::from(text)),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    widgets::toggle_switch(theme, on, format!("browser-extension-{ix}"))
                        .id(("browser-extension-power", ix))
                        .cursor_pointer()
                        .tab_index(0)
                        .role(gpui::Role::Switch)
                        .aria_label(format!("Load {name}"))
                        .aria_toggled(if on { gpui::Toggled::True } else { gpui::Toggled::False })
                        .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            settings::update(SavePolicy::Immediate, cx, |s| {
                                if let Some(e) =
                                    s.browser_extensions.iter_mut().find(|e| e.path == power_path)
                                {
                                    e.on = !e.on
                                }
                            });
                            cx.notify();
                        })),
                )
                .child(
                    widgets::ghost_action(theme)
                        .id(("browser-extension-remove", ix))
                        .tab_index(0)
                        .role(gpui::Role::Button)
                        .aria_label(format!("Remove {name}"))
                        .child(
                            crate::icons::icon(crate::icons::TRASH_BIN_MINIMALISTIC)
                                .size(px(16.0))
                                .text_color(theme.text_muted),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remove = Some(remove_path.clone());
                            cx.notify();
                        })),
                ),
        )
        .into_any_element()
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for BrowserSettingsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for BrowserSettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let list = settings::current(cx).browser_extensions;
        self.load_info(&list);
        let dialog = self.render_install_dialog(window.viewport_size(), cx);
        let unavailable = bext::unavailable();
        let rows: Vec<AnyElement> = list
            .iter()
            .enumerate()
            .map(|(ix, e)| self.row(ix, e, &theme, cx))
            .collect();
        let empty = match unavailable {
            Some(why) => Some(("Browser extensions aren't available", why)),
            None => rows.is_empty().then_some((
                "No browser extensions yet",
                "Install one from the Chrome Web Store, or an unpacked one from a folder.",
            )),
        };
        let install = unavailable.is_none().then(|| {
            widgets::ghost_action(&theme)
                .id("browser-extension-open-install")
                .tab_index(0)
                .role(gpui::Role::Button)
                .aria_label("Install a browser extension")
                .focus_visible(|s| s.border_2().border_color(theme.accent))
                .child(
                    crate::icons::icon(crate::icons::ADD_CIRCLE)
                        .size(px(14.0))
                        .text_color(theme.text_muted),
                )
                .child("Install")
                .on_click(cx.listener(|this, _, window, cx| this.open_install(window, cx)))
        });
        let block = widgets::section_card(&theme)
            .mt(px(8.0))
            .children(rows)
            .when_some(empty, |el, (title, body)| {
                el.child(
                    widgets::card_row(&theme, list.is_empty()).child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .child(widgets::row_title(&theme, title))
                            .child(widgets::meta_line(
                                &theme,
                                vec![div().child(SharedString::from(body)).into_any_element()],
                            )),
                    ),
                )
            });
        let section = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .mt(px(28.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .min_h(px(32.0))
                    .child(widgets::section_label(&theme, "Extensions"))
                    .children(install),
            )
            .child(block)
            .when_some(self.note.clone(), |el, note| {
                el.child(
                    div()
                        .mt(px(8.0))
                        .px(px(8.0))
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child(note),
                )
            });

        let scrollbar = popover::rail(self, "browser-settings-page-scrollbar", &theme, cx);
        div()
            .id("browser-settings-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("browser-settings-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(widgets::page_header(&theme, "Browser", None))
                                .child(widgets::page_subtitle(
                                    &theme,
                                    "Chrome extensions in the browser tabs.",
                                ))
                                .child(section),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
            .when_some(dialog, |el, dialog| el.child(dialog))
    }
}
