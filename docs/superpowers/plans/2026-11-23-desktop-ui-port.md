# Desktop UI Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `vardoger` with no subcommand opens Zeron 0.2's GPUI window (sidebar, projects, chat) against the Vardoger engine.

**Architecture:** Import `crates/ui` and its missing support crates from Zeron 0.2.122, rename, cut whole modules, fix compiler fallout against Vardoger's 0.3.16-based proto/rpc. Compiler-driven port: each task's test is `cargo check` plus the surviving unit tests.

**Tech Stack:** Rust, GPUI (zeronsh/zui patched to freethinkel/zui), gpui-base, vardoger-rpc.

**Spec:** `docs/superpowers/specs/2026-11-23-desktop-ui-port-design.md`

## Global Constraints

- Source: `~/Developer/forks/zeron` @ 0e1b891f9 (v0.2.122). Never edit it.
- Work in worktree `~/Developers/vardoger-desktop`, branch `desktop`; build with `CARGO_INCREMENTAL=0`.
- Rename `zeron`→`vardoger` in crate names, idents, env vars, paths (`~/.vardoger`); keep Zeron's copyright in LICENSE/THIRD_PARTY_NOTICES.
- gpui: `gpui`, `gpui_platform`, `gpui_tokio` from `https://github.com/zeronsh/zui`, every zui crate `[patch]`ed to `https://github.com/freethinkel/zui` rev `38b9e0f47d28a90da3bfa01d3827c8685f660b89`; `gpui-base` from `zeronsh/gpui-component` rev `103f13da91ff2c7c52941bdc2b1910f29716dc19` (copy Shepherd's `Cargo.toml` block).
- No UI redesign; only cuts and API adaptation. Prefer deleting over stubbing.
- Each commit: `cargo check --workspace --all-targets` passes.

## Review Focus

- Fresh `~/.vardoger` (no projects, no engine running): window opens, onboarding/empty sidebar, no panic.
- Engine already running from `vardoger headless` on port 27754: window attaches to it instead of starting a second one.
- Sending a message in a new chat streams the reply into the transcript.
- Opening a project folder adds it to the sidebar and survives restart.
- Cut features leave no dead menu items or settings pages that panic when clicked.

---

### Task 1: Import and compile the UI crate

**Files:**
- Create: `crates/ui/**` (from zeron `crates/ui`), `crates/theme/**`, `crates/term/**`, `crates/zatex/**`
- Modify: `Cargo.toml` (members, workspace deps, gpui block + `[patch]`)

- [ ] Worktree + branch `desktop` from `main`.
- [ ] Copy the four crates, commit raw import ("Import ui, theme, term, zatex from Zeron 0.2.122").
- [ ] Rename to `vardoger-ui`, `vardoger-theme`, `vardoger-term`, `zatex`; add gpui deps/patch; commit.
- [ ] Delete cut modules: cloud, account_usage, settings/accounts, pull_request*, pull_requests, change_requests, voice, dictation, orb, visuals, composer_dictation_tests, browser, appshots; drop voice/orb/veil/visual deps.
- [ ] Fix fallout until `cargo check -p vardoger-ui --all-targets` passes: gpui API drift against freethinkel/zui, proto/rpc drift against Vardoger. Commit per coherent group.

### Task 2: Open the window from `vardoger`

**Files:**
- Modify: `apps/vardoger/src/main.rs`, `apps/vardoger/Cargo.toml`, `apps/vardoger/src/lib.rs`

**Interfaces:**
- Consumes: `vardoger_ui`'s entry (0.2's `zeron_ui::run` equivalent, see zeron `apps/zeron/src/main.rs`)
- Produces: `Launch::Window { .. }` → `vardoger_ui::run(...)`

- [ ] Port 0.2's `apps/zeron/src/main.rs` window launch (engine spawn/attach via daemon) into `apps/vardoger`.
- [ ] Rewire `app_update.rs` to `vardoger-update`.
- [ ] `cargo check --workspace --all-targets`; run surviving `vardoger-ui` unit tests.
- [ ] Commit.

### Task 3: Run it

- [ ] `cargo run -p vardoger` on macOS; walk the Review Focus list by hand; fix panics.
- [ ] Commit, push branch `desktop`, report what works and what was cut.
