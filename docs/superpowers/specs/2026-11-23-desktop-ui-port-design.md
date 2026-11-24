# Desktop UI: port Zeron 0.2's GPUI interface

## Intent

Commutator needs a desktop app. The chat in old Zeron (0.2.x, GPUI, `crates/ui`
in `~/Developer/forks/zeron` @ 0e1b891f9 / v0.2.122) is what the user wants:
projects sidebar, chat (transcript + composer), side menu, almost everything.
Port it as is; the user will reshape it afterwards. Zeron 0.3 dropped this UI
for zgx, which the user does not want.

## Goals

- `commutator` with no subcommand opens the window (the CLI's existing
  `Launch::Window` branch), engine in-process or over IPC as old Zeron did.
- Projects/sidebar, chat transcript, composer, pickers, history, changes,
  files, terminal panel, todo/queue, settings, theme/appearance, menus,
  toasts, onboarding work against the 0.3.16-based Commutator engine.

## Non-goals (cut now)

- Cloud, accounts, billing: `cloud.rs`, `account_usage.rs`,
  `settings/accounts.rs`, sign-in onboarding steps.
- Pull requests: `pull_request*.rs`, `pull_requests.rs`, `change_requests.rs`.
- Voice and visuals (plan step 3): `voice.rs`, `dictation.rs`, `orb/`,
  `visuals.rs`, `composer_dictation_tests.rs`, crate deps voice/orb/veil/visual.
- Browser and appshots: `browser/`, `appshots.rs`.
- Anything else whose only purpose was a removed engine feature (plugins,
  templates, edge rooms) goes with its compiler fallout.

## Anti-goals

- Redesigning or "cleaning up" the UI during the port. Behaviour and look
  stay Zeron 0.2's; only cuts and API adaptation.

## Approaches

1. **Import `crates/ui` whole, rename, cut, fix fallout** — chosen. Same
   method as the engine port; keeps the exact chat the user liked.
2. Port only transcript/composer/markdown and write a new shell on
   `commutator-client` — less code, but the shell and wiring get rewritten and
   no longer behave like Zeron.
3. GPUI painter over Zeron's mobile layout — not the chat the user liked.

## Decided how

- New crate `crates/ui` (`commutator-ui`) plus the support crates it needs from
  0.2.122 that we lack: `theme`, `term`, `zatex` (renamed `commutator-*`).
- GPUI from the user's zui fork, the Shepherd way: depend on
  `github.com/zeronsh/zui` and `[patch]` every zui crate to
  `github.com/freethinkel/zui` (Shepherd's rev 38b9e0f), `gpui-base` from
  `zeronsh/gpui-component` at Shepherd's rev, so both share one gpui.
- The UI keeps talking to the engine through `commutator-rpc` (`RpcClient`,
  `ClientFrame`, `TranscriptUpdate`); proto drift from 0.2→0.3.16 is fixed on
  the UI side, engine changes only where the UI needs something 0.3 removed.
- Self-update UI (`app_update.rs`) is rewired to `commutator-update` (GitHub
  Releases), not cut.
- Commits per step (import, rename, cut groups, compile, run), each with
  `cargo check --workspace --all-targets` passing; builds with
  `CARGO_INCREMENTAL=0` (disk is tight).

## Left to the builder

- Exact order of cuts; whether a module goes or gets stubbed when it is
  tangled with kept code (prefer delete, stub only to unblock).
- Which 0.2 tests survive; UI tests that need cut features go.
- macOS first; Linux/Windows must still compile but are not run.
