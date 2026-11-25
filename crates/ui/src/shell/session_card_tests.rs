//! The session card and the chat menu's Copy submenu, against a whole shell.
use super::*;

fn test_shell(
    cx: &mut gpui::TestAppContext,
    path: &std::path::Path,
) -> gpui::WindowHandle<super::Shell> {
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::default());
        crate::app_menus::init(cx);
        crate::history::init(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            cx,
        );
    });
    cx.add_window(|_, cx| {
        let state = cx.new(|_| AppState::new());
        Shell::new(
            state,
            EngineBootConfig {
                data_dir: path.into(),
                ipc_port: 0,
                default_harness: commutator_proto::HarnessId::Mock,
            },
            cx,
        )
    })
}

fn chat(id: &str) -> commutator_proto::Chat {
    serde_json::from_value(serde_json::json!({"id":id,"title":id,"deviceId":"local","archived":false,"createdAt":"2026-09-20T00:00:00Z"})).unwrap()
}
fn prepare(shell: &mut Shell, cx: &mut Context<Shell>) {
    shell.state.update(cx, |state, _| {
        state.workspace_scope = Some(WorkspaceScope::Local);
        state.local_device_id = Some("local".into());
        state.chats_synced = true;
        state.chats = vec![chat("pin"), chat("regular"), chat("other")];
    });
    shell.settings.space_filter = None;
    shell.settings.sidebar_organization = SidebarOrganization::InOneList;
    shell
        .settings
        .sidebar_pins_mut("local".into())
        .push("pin".into());
}

#[gpui::test]
fn the_session_card_shows_beside_a_wide_transcript_and_opens_over_a_narrow_one(
    cx: &mut gpui::TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let window = test_shell(cx, dir.path());
    window
        .update(cx, |shell, _, cx| {
            prepare(shell, cx);
            shell.debug_gate = Some(super::GatePhase::Ready);
            shell.settings.sidebar_collapsed = true;
            shell
                .state
                .update(cx, |state, _| state.selected_chat = Some("regular".into()));
            // The card eases in and out; settle each change at once.
            crate::motion::set_reduced_motion(cx, true);
        })
        .unwrap();
    let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);

    // With room, it shows on its own, clear of the centered transcript.
    cx.simulate_resize(gpui::size(px(1600.0), px(900.0)));
    cx.run_until_parked();
    let card = cx
        .debug_bounds("session-info")
        .expect("a wide column shows the card unasked");
    assert!(card.right() <= px(1600.0));
    assert!(card.left() > px(800.0 + 736.0 / 2.0));
    let actions = cx.debug_bounds("session-info-side-chat").unwrap();
    assert!(card.contains(&actions.center()));

    // Without room it hides, and opening it floats it over the transcript.
    cx.simulate_resize(gpui::size(px(900.0), px(900.0)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("session-info").is_none());
    window
        .update(&mut cx, |shell, _, cx| shell.toggle_session_info(cx))
        .unwrap();
    cx.run_until_parked();
    let card = cx
        .debug_bounds("session-info")
        .expect("opening shows the card over a narrow column");
    assert!(card.right() <= px(900.0));

    // The toggle closes it again.
    window
        .update(&mut cx, |shell, _, cx| shell.toggle_session_info(cx))
        .unwrap();
    cx.run_until_parked();
    assert!(cx.debug_bounds("session-info").is_none());

    // Choosing one of its rows dismisses it too.
    window
        .update(&mut cx, |shell, _, cx| shell.toggle_session_info(cx))
        .unwrap();
    cx.run_until_parked();
    let fork = cx.debug_bounds("session-info-fork").unwrap();
    cx.simulate_click(fork.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("session-info").is_none());
}

#[gpui::test]
fn copying_from_the_copy_submenu_shows_a_toast(cx: &mut gpui::TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let window = test_shell(cx, dir.path());
    window
        .update(cx, |shell, _, cx| {
            prepare(shell, cx);
            shell.debug_gate = Some(super::GatePhase::Ready);
            shell.chat_menu.open(super::ChatMenuState {
                chat_id: "regular".into(),
                tab: None,
                position: gpui::point(px(40.0), px(120.0)),
                page: super::ChatMenuPage::Copy,
            });
            cx.notify();
        })
        .unwrap();
    let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    // The item sits in the submenu, outside the menu card: pressing it
    // must copy rather than dismiss the menu first.
    let item = cx
        .debug_bounds("chat-copy-commutator")
        .expect("the Copy submenu is open");
    cx.simulate_click(item.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(crate::toast::messages(cx).len(), 1, "one toast");
    });
    assert!(cx.debug_bounds("toaster").is_some(), "the shell paints it");
}
