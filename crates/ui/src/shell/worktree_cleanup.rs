//! The Projects sidebar's project and worktree rows' context menu (New chat,
//! Create worktree…, Reveal in Finder, Delete worktree…), the Create worktree
//! dialog, and removing a linked worktree once nothing active runs in it:
//! offered after its last active chat is deleted, and from its row's menu.
//! Archiving never asks — an archived chat can come back.
use super::*;
use crate::composer::{ComposerInput, ComposerInputEvent};
use commutator_proto::{ChangeRequestState, Chat};

/// A project or worktree row's context menu.
#[derive(Clone, Debug)]
pub(super) struct RowMenu {
    pub space_id: String,
    pub device_id: String,
    /// What Reveal in Finder opens: the project's folder or the worktree's.
    pub folder: String,
    /// A worktree row: the branch its New chat runs on (when known) and what
    /// Delete removes.
    pub worktree: Option<(Option<String>, WorktreeTarget)>,
}

/// The Create worktree dialog, off a project row's menu.
pub(super) struct NewWorktree {
    space_id: String,
    device_id: String,
    repo_path: String,
    name: Entity<ComposerInput>,
    creating: bool,
    error: Option<SharedString>,
    focus_pending: bool,
    _events: gpui::Subscription,
}

/// What the dialog's field becomes: spaces to dashes, so "login fix" works.
fn worktree_name(text: &str) -> String {
    text.trim().split_whitespace().collect::<Vec<_>>().join("-")
}

/// A linked worktree of `repo_path` on `device_id`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WorktreeTarget {
    pub device_id: String,
    pub repo_path: String,
    pub worktree_path: String,
    /// Known from the sidebar's ref list; `None` until `ListRefs` names it.
    pub branch: Option<String>,
}

pub(super) struct WorktreeCleanup {
    target: WorktreeTarget,
    branch: String,
    /// Files with uncommitted changes; `None` when the check failed.
    dirty_files: Option<usize>,
    archived_chats: Vec<String>,
    /// Chats still active in it (the sidebar's open worktree): archived once
    /// it's gone, so none is left in a folder that isn't there.
    active_chats: Vec<String>,
    delete_archived: bool,
    deleting: bool,
    error: Option<SharedString>,
}

fn trim_path(path: &str) -> &str {
    path.trim().trim_end_matches(['/', '\\'])
}

fn same_path(a: &str, b: &str) -> bool {
    trim_path(a) == trim_path(b)
}

/// The worktree `deleted` leaves with no active chat: its cwd when that is
/// not the project's own folder and no other unarchived chat on its device
/// runs there. Whether the folder is a linked worktree at all is `ListRefs`'
/// call.
fn orphaned_worktree<'a>(chats: &[Chat], deleted: &'a Chat, space_path: &str) -> Option<&'a str> {
    if deleted.archived {
        return None;
    }
    let cwd = deleted
        .cwd
        .as_deref()
        .filter(|cwd| !trim_path(cwd).is_empty())?;
    if same_path(cwd, space_path) {
        return None;
    }
    let busy = chats.iter().any(|chat| {
        chat.id != deleted.id
            && !chat.archived
            && chat.device_id == deleted.device_id
            && chat
                .cwd
                .as_deref()
                .is_some_and(|other| same_path(other, cwd))
    });
    (!busy).then_some(cwd)
}

impl Shell {
    /// A project row's menu: its space, when it has one (not "No project").
    pub(super) fn project_row_menu(&self, chat: &Chat, cx: &App) -> Option<RowMenu> {
        let space = self.state.read(cx).space_for_chat(chat)?;
        Some(RowMenu {
            space_id: space.id.clone(),
            device_id: space.device_id.clone(),
            folder: space.path.clone(),
            worktree: None,
        })
    }

    /// Start a chat in `space_id`, in the worktree that has `branch` checked
    /// out when given.
    fn new_chat_in(&mut self, space_id: String, branch: Option<String>, cx: &mut Context<Self>) {
        self.worktree_menu = None;
        self.open_new_session(Some(space_id.clone()), cx);
        if let Some(branch) = branch {
            let pickers = self.composer.read(cx).pickers().clone();
            pickers.update(cx, |pickers, cx| pickers.use_worktree(space_id, branch, cx));
        }
        cx.notify();
    }

    fn open_new_worktree(&mut self, menu: RowMenu, cx: &mut Context<Self>) {
        self.worktree_menu = None;
        let name = cx.new(|cx| ComposerInput::new("Name (optional)", cx));
        let events = cx.subscribe(&name, |this: &mut Shell, _, event, cx| match event {
            ComposerInputEvent::Submitted => this.create_new_worktree(cx),
            ComposerInputEvent::Edited => {
                if let Some(dialog) = this.new_worktree.as_mut() {
                    dialog.error = None;
                }
                cx.notify();
            }
            _ => {}
        });
        self.new_worktree = Some(NewWorktree {
            space_id: menu.space_id,
            device_id: menu.device_id,
            repo_path: menu.folder,
            name,
            creating: false,
            error: None,
            focus_pending: true,
            _events: events,
        });
        cx.notify();
    }

    /// `CreateWorktree` off the project's current checkout, then a new chat
    /// in it, as its faint row's click starts. An empty name lets the host
    /// pick one.
    fn create_new_worktree(&mut self, cx: &mut Context<Self>) {
        let (engine, local) = {
            let state = self.state.read(cx);
            (state.engine().cloned(), state.local_device_id.clone())
        };
        let Some(dialog) = self.new_worktree.as_mut().filter(|d| !d.creating) else {
            return;
        };
        let Some(engine) = engine else {
            dialog.error = Some("Engine not connected".into());
            cx.notify();
            return;
        };
        let name = worktree_name(dialog.name.read(cx).text());
        let mut params = serde_json::Map::new();
        params.insert("repoPath".into(), dialog.repo_path.clone().into());
        params.insert("branch".into(), "HEAD".into());
        params.insert("spaceId".into(), dialog.space_id.clone().into());
        if !name.is_empty() {
            params.insert("name".into(), name.into());
        }
        if local.as_deref() != Some(dialog.device_id.as_str()) {
            params.insert("targetDeviceId".into(), dialog.device_id.clone().into());
        }
        dialog.creating = true;
        dialog.error = None;
        let space_id = dialog.space_id.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::CREATE_WORKTREE, serde_json::Value::Object(params))
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<commutator_proto::CreateWorktreeOutcome>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |this, cx| {
                let Some(dialog) = this.new_worktree.as_mut() else {
                    return;
                };
                let outcome = match result {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        dialog.creating = false;
                        dialog.error = Some(format!("Couldn’t create the worktree: {error}").into());
                        cx.notify();
                        return;
                    }
                };
                this.new_worktree = None;
                // Re-list the project's worktrees so its row shows up.
                this.sidebar_worktrees_for = None;
                this.new_chat_in(space_id, Some(outcome.worktree.branch), cx);
            })
            .ok();
        })
        .detach();
    }

    /// Cancel or Escape: the dialog closes unless it's mid-create.
    pub(super) fn dismiss_new_worktree(&mut self, cx: &mut Context<Self>) {
        if self.new_worktree.as_ref().is_some_and(|d| !d.creating) {
            self.new_worktree = None;
            cx.notify();
        }
    }

    /// After a chat is deleted: offer its worktree if that was its last
    /// active chat.
    pub(super) fn offer_worktree_cleanup(&mut self, deleted: &Chat, cx: &mut Context<Self>) {
        let target = {
            let state = self.state.read(cx);
            let Some(space) = state.space_for_chat(deleted).filter(|s| s.git_detected) else {
                return;
            };
            let Some(path) = orphaned_worktree(&state.chats, deleted, &space.path) else {
                return;
            };
            WorktreeTarget {
                device_id: deleted.device_id.clone(),
                repo_path: space.path.clone(),
                worktree_path: path.to_string(),
                branch: None,
            }
        };
        self.begin_worktree_cleanup(target, cx);
    }

    /// The faint row's target: a worktree of `space_id` the sidebar listed.
    pub(super) fn closed_worktree_target(
        &self,
        space_id: &str,
        worktree_path: &str,
        branch: &str,
        cx: &App,
    ) -> Option<WorktreeTarget> {
        if trim_path(worktree_path).is_empty() {
            return None;
        }
        let space = self
            .state
            .read(cx)
            .spaces
            .iter()
            .find(|s| s.id == space_id)?;
        Some(WorktreeTarget {
            device_id: space.device_id.clone(),
            repo_path: space.path.clone(),
            worktree_path: worktree_path.to_string(),
            branch: Some(branch.to_string()),
        })
    }

    /// A merged pull request from one of the worktree's chats: the safest
    /// sign it has served its purpose.
    pub(super) fn worktree_merged(&self, target: &WorktreeTarget, cx: &App) -> bool {
        let state = self.state.read(cx);
        state.chats.iter().any(|chat| {
            chat.device_id == target.device_id
                && chat
                    .cwd
                    .as_deref()
                    .is_some_and(|cwd| same_path(cwd, &target.worktree_path))
                && state
                    .change_request_for_chat(chat)
                    .is_some_and(|pr| pr.state == ChangeRequestState::Merged)
        })
    }

    /// Confirm the worktree is a linked one (when the branch is unknown),
    /// count its uncommitted files, then show the dialog.
    pub(super) fn begin_worktree_cleanup(
        &mut self,
        target: WorktreeTarget,
        cx: &mut Context<Self>,
    ) {
        self.worktree_menu = None;
        if self.worktree_cleanup.is_some() {
            return;
        }
        let (engine, local) = {
            let state = self.state.read(cx);
            (state.engine().cloned(), state.local_device_id.clone())
        };
        let Some(engine) = engine else {
            return;
        };
        let remote =
            (local.as_deref() != Some(target.device_id.as_str())).then(|| target.device_id.clone());
        cx.spawn(async move |this, cx| {
            let with_target = |mut params: serde_json::Map<String, serde_json::Value>| {
                if let Some(device) = &remote {
                    params.insert("targetDeviceId".into(), device.clone().into());
                }
                serde_json::Value::Object(params)
            };
            let branch = match target.branch.clone() {
                Some(branch) => branch,
                None => {
                    let mut params = serde_json::Map::new();
                    params.insert("repoPath".into(), target.repo_path.clone().into());
                    let Ok(value) = engine
                        .client()
                        .call(methods::LIST_REFS, with_target(params))
                        .await
                    else {
                        return;
                    };
                    let Ok(refs) = serde_json::from_value::<Vec<commutator_proto::RepoRef>>(value)
                    else {
                        return;
                    };
                    // Not a linked worktree (a subfolder, the main checkout):
                    // nothing to offer.
                    let Some(found) = refs.into_iter().find(|r| {
                        r.worktree_path
                            .as_deref()
                            .is_some_and(|path| same_path(path, &target.worktree_path))
                    }) else {
                        return;
                    };
                    found.name
                }
            };
            let mut params = serde_json::Map::new();
            params.insert("cwd".into(), target.worktree_path.clone().into());
            let dirty_files = engine
                .client()
                .call(methods::GET_CHECKOUT_DIFF, with_target(params))
                .await
                .ok()
                .and_then(|value| {
                    serde_json::from_value::<commutator_proto::CheckoutDiff>(value).ok()
                })
                .map(|diff| diff.files.len());
            this.update(cx, |this, cx| {
                if this.worktree_cleanup.is_some() {
                    return;
                }
                let (archived_chats, active_chats): (Vec<_>, Vec<_>) = this
                    .state
                    .read(cx)
                    .chats
                    .iter()
                    .filter(|chat| {
                        chat.device_id == target.device_id
                            && chat
                                .cwd
                                .as_deref()
                                .is_some_and(|cwd| same_path(cwd, &target.worktree_path))
                    })
                    .partition(|chat| chat.archived);
                let ids = |chats: Vec<&Chat>| chats.into_iter().map(|c| c.id.clone()).collect();
                let (archived_chats, active_chats) = (ids(archived_chats), ids(active_chats));
                this.worktree_cleanup = Some(WorktreeCleanup {
                    target,
                    branch,
                    dirty_files,
                    archived_chats,
                    active_chats,
                    delete_archived: false,
                    deleting: false,
                    error: None,
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn confirm_worktree_cleanup(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.worktree_cleanup.as_mut().filter(|d| !d.deleting) else {
            return;
        };
        let (engine, local) = {
            let state = self.state.read(cx);
            (state.engine().cloned(), state.local_device_id.clone())
        };
        let Some(engine) = engine else {
            dialog.error = Some("Engine not connected".into());
            cx.notify();
            return;
        };
        dialog.deleting = true;
        dialog.error = None;
        let target = dialog.target.clone();
        let mut params = serde_json::Map::new();
        params.insert("repoPath".into(), target.repo_path.clone().into());
        params.insert("worktreePath".into(), target.worktree_path.clone().into());
        if local.as_deref() != Some(target.device_id.as_str()) {
            params.insert("targetDeviceId".into(), target.device_id.clone().into());
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::DELETE_WORKTREE, serde_json::Value::Object(params))
                .await;
            this.update(cx, |this, cx| {
                let Some(dialog) = this.worktree_cleanup.as_mut() else {
                    return;
                };
                if let Err(error) = result {
                    dialog.deleting = false;
                    dialog.error = Some(format!("Couldn’t delete the worktree: {error}").into());
                    cx.notify();
                    return;
                }
                let dialog = this.worktree_cleanup.take().unwrap();
                for chat_id in dialog.active_chats {
                    this.set_chat_archived(chat_id, true, cx);
                }
                if dialog.delete_archived {
                    for chat_id in dialog.archived_chats {
                        this.delete_chat(chat_id, cx);
                    }
                }
                // Drop its faint row now and re-list the project's worktrees.
                for refs in this.sidebar_worktrees.values_mut() {
                    refs.retain(|r| {
                        r.worktree_path
                            .as_deref()
                            .is_none_or(|path| !same_path(path, &target.worktree_path))
                    });
                }
                this.sidebar_worktrees_for = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_row_menu(&self, menu: RowMenu, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let item = |id: &'static str, glyph: &'static str, label: &'static str| {
            popover::menu_row_nav(theme, false, false, id)
                .id(id)
                .child(icon(glyph).size(px(14.0)).text_color(theme.text_muted))
                .child(label)
        };
        let local = self.state.read(cx).local_device_id.as_deref() == Some(menu.device_id.as_str());
        let reveal_label = if cfg!(target_os = "macos") {
            "Reveal in Finder"
        } else {
            "Show in File Manager"
        };
        // A worktree whose branch isn't known yet can't be picked for a chat.
        let new_chat = match &menu.worktree {
            Some((None, _)) => None,
            Some((Some(branch), _)) => Some(Some(branch.clone())),
            None => Some(None),
        };
        let mut card = popover::popover_card(theme)
            .w(px(200.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.worktree_menu = None;
                cx.notify();
            }));
        if let Some(branch) = new_chat {
            let space_id = menu.space_id.clone();
            card = card.child(item("row-menu-new-chat", icons::PEN_NEW_SQUARE, "New chat").on_click(
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.new_chat_in(space_id.clone(), branch.clone(), cx);
                }),
            ));
        }
        if menu.worktree.is_none() {
            let open = menu.clone();
            card = card.child(
                item("row-menu-new-worktree", icons::WORKTREE, "Create worktree…").on_click(
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_new_worktree(open.clone(), cx);
                    }),
                ),
            );
        }
        // Finder opens this Mac's folders only.
        if local {
            let folder = menu.folder.clone();
            card = card.child(item("row-menu-reveal", icons::FOLDER, reveal_label).on_click(
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.worktree_menu = None;
                    cx.reveal_path(std::path::Path::new(&folder));
                    cx.notify();
                }),
            ));
        }
        if let Some((_, target)) = menu.worktree {
            card = card.child(popover::menu_separator()).child(
                popover::menu_row_nav(theme, false, false, "row-menu-delete")
                    .id("row-menu-delete")
                    .text_color(theme.danger)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.begin_worktree_cleanup(target.clone(), cx);
                        cx.notify();
                    }))
                    .child(
                        icon(icons::TRASH_BIN_MINIMALISTIC)
                            .size(px(14.0))
                            .text_color(theme.danger),
                    )
                    .child("Delete worktree…"),
            );
        }
        card.into_any_element()
    }

    fn render_new_worktree_dialog(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.new_worktree.as_mut()?;
        if std::mem::take(&mut dialog.focus_pending) {
            window.focus(&dialog.name.focus_handle(cx), cx);
        }
        let theme = Theme::of(cx).for_popup();
        let creating = dialog.creating;
        let error = dialog.error.clone();
        let name = dialog.name.clone();
        let project = trim_path(&dialog.repo_path)
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_string();
        let card = popover::dialog_card(&theme)
            .w(px(400.0))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.dismiss_new_worktree(cx);
                }
            }))
            .child(popover::dialog_title(&theme, "Create worktree"))
            .child(div().mt(px(6.0)).child(popover::dialog_body(
                &theme,
                format!(
                    "A new checkout of \u{201C}{project}\u{201D} on its own commutator/<name> branch, off the current one. Leave the name empty for a generated one."
                ),
            )))
            .child(
                div()
                    .mt(px(12.0))
                    .child(popover::dialog_field(name.into_any_element())),
            )
            .children(error.map(|error| {
                div()
                    .mt(px(10.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.danger)
                    .child(error)
            }))
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        popover::btn_ghost(&theme, "Cancel", "new-worktree-cancel")
                            .id("new-worktree-cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss_new_worktree(cx))),
                    )
                    .child(
                        popover::btn_primary(&theme, if creating { "Creating…" } else { "Create" })
                            .id("new-worktree-create")
                            .opacity(if creating { 0.6 } else { 1.0 })
                            .on_click(cx.listener(|this, _, _, cx| this.create_new_worktree(cx))),
                    ),
            )
            .into_any_element();
        Some(popover::modal("new-worktree-dialog", viewport, card))
    }

    pub(super) fn render_worktree_cleanup_overlays(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let mut overlays = Vec::new();
        if let Some((menu, position)) = self.worktree_menu.clone() {
            let card = self.render_row_menu(menu, &theme, cx);
            overlays.push(popover::menu_at("worktree-context-menu", position, card, None));
        }
        overlays.extend(self.render_new_worktree_dialog(viewport, window, cx));
        let Some(dialog) = self.worktree_cleanup.as_ref() else {
            return overlays;
        };
        let folder = trim_path(&dialog.target.worktree_path)
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_string();
        let branch_line = if dialog.branch.starts_with("commutator/") {
            format!("Its branch {} is deleted too.", dialog.branch)
        } else {
            format!("The branch {} stays.", dialog.branch)
        };
        let chats_line = match dialog.active_chats.len() {
            0 => String::new(),
            1 => " Its chat is archived.".to_string(),
            n => format!(" Its {n} chats are archived."),
        };
        let body = format!(
            "\u{201C}{folder}\u{201D} is removed from disk ({}). {branch_line}{chats_line}",
            dialog.target.worktree_path
        );
        let warning: Option<SharedString> = match dialog.dirty_files {
            Some(0) => None,
            Some(1) => Some("1 file with uncommitted changes will be lost.".into()),
            Some(n) => Some(format!("{n} files with uncommitted changes will be lost.").into()),
            None => Some("Couldn’t check for uncommitted changes.".into()),
        };
        let archived = dialog.archived_chats.len();
        let delete_archived = dialog.delete_archived;
        let deleting = dialog.deleting;
        let error = dialog.error.clone();
        let checkbox = (archived > 0).then(|| {
            div()
                .id("worktree-cleanup-archived")
                .mt(px(14.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .cursor_pointer()
                .role(gpui::Role::CheckBox)
                .aria_toggled(if delete_archived {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(dialog) = this.worktree_cleanup.as_mut() {
                        dialog.delete_archived = !dialog.delete_archived;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .size(px(14.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(if delete_archived {
                            theme.accent
                        } else {
                            theme.border_strong
                        })
                        .when(delete_archived, |el| {
                            el.bg(theme.accent)
                                .child(icon(icons::CHECK).size(px(10.0)).text_color(theme.surface))
                        }),
                )
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(13.0))
                        .text_color(theme.text)
                        .child(if archived == 1 {
                            "Also delete its archived chat".to_string()
                        } else {
                            format!("Also delete its {archived} archived chats")
                        }),
                )
        });
        let card = popover::dialog_card(&theme)
            .child(popover::dialog_title(&theme, "Delete worktree?"))
            .child(div().mt(px(6.0)).child(popover::dialog_body(&theme, body)))
            .children(warning.map(|warning| {
                div()
                    .mt(px(10.0))
                    .text_size(crate::typography::ui_rems(13.0))
                    .text_color(theme.danger)
                    .child(warning)
            }))
            .children(checkbox)
            .children(error.map(|error| {
                div()
                    .mt(px(10.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.danger)
                    .child(error)
            }))
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        popover::btn_ghost(&theme, "Keep", "worktree-cleanup-keep")
                            .id("worktree-cleanup-keep")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.worktree_cleanup = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        popover::btn_danger(
                            &theme,
                            if deleting {
                                "Deleting…"
                            } else {
                                "Delete worktree"
                            },
                        )
                        .id("worktree-cleanup-confirm")
                        .opacity(if deleting { 0.6 } else { 1.0 })
                        .on_click(cx.listener(|this, _, _, cx| this.confirm_worktree_cleanup(cx))),
                    ),
            )
            .into_any_element();
        overlays.push(popover::modal("worktree-cleanup-dialog", viewport, card));
        overlays
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str, cwd: &str, archived: bool) -> Chat {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": id, "deviceId": "mac", "archived": archived,
            "cwd": cwd, "createdAt": "2026-11-25T00:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn a_typed_worktree_name_dashes_its_spaces() {
        assert_eq!(worktree_name("  login  fix "), "login-fix");
        assert_eq!(worktree_name("   "), "");
    }

    #[test]
    fn only_the_last_active_chat_in_a_worktree_orphans_it() {
        let repo = "/src/app";
        let wt = "/wt/app/fix";
        let a = chat("a", wt, false);
        // Another active chat there keeps it busy; an archived one doesn't.
        let chats = vec![a.clone(), chat("b", "/wt/app/fix/", false)];
        assert_eq!(orphaned_worktree(&chats, &a, repo), None);
        let chats = vec![a.clone(), chat("b", wt, true)];
        assert_eq!(orphaned_worktree(&chats, &a, repo), Some(wt));
        // The project's own folder is never offered, nor an archived chat.
        let main = chat("m", "/src/app/", false);
        assert_eq!(orphaned_worktree(&[main.clone()], &main, repo), None);
        let old = chat("o", wt, true);
        assert_eq!(orphaned_worktree(&[old.clone()], &old, repo), None);
    }
}
