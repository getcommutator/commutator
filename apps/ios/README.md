# Commutator for iOS

A UIKit viewport onto one of your machines, built on the Rust mobile core. The
phone signs in to your Commutator server (Google, PKCE, back to
`commutator://auth`), picks one of the account's machines, and drives that
machine's engine through the server's relay (`commutator-client`'s
`Credentials::Remote`). No agent runs on the phone.

Ported from Zeron's iOS app. Cut, with no backend in Commutator: Cloud (machines,
GitHub, billing, vault logins, onboarding), the agent-accounts / Providers
screens (tangled with Cloud; an error card's "Log in" says to sign in on the
machine), voice and its Live Activity, push notifications (the server has no
push API yet), and direct WebRTC terminals (terminals run over the relay).

**Rust decides what to paint and where; Swift paints, scrolls and handles
gestures.** Everything in Rust is platform-neutral — the future Android app
links the same library. See [`docs/mobile-rewrite.md`](../../docs/mobile-rewrite.md).

## Build & run

Requires Xcode 26+ and a Rust toolchain with the iOS targets:

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cd apps/ios
xcodebuild -project Commutator.xcodeproj -scheme Commutator \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' build
```

The Commutator target's **Rust core** build phase runs `scripts/ios/build-core.sh`,
which builds `crates/mobile` for the active platform (always optimized — the
`mobile` cargo profile) and refreshes the committed UniFFI bindings in
`Commutator/Core/Generated/`. `COMMUTATOR_SKIP_CORE=1` reuses the last built library
when iterating on Swift only.

## Layout

```
Commutator/
  App/         AppDelegate, SceneDelegate, AppModel (owns CoreClient; maps
               workspace snapshots to view models; Keychain credentials)
  Core/        Generated UniFFI bindings; Fonts (the exact bytes Rust measures,
               registered with CoreText) + CoreText fallback measurer
  Design/      Palette (Commutator color roles → light/dark), Glass helpers,
               cell-grid status glyph, harness brand marks
  Transcript/  TranscriptListView (virtualized scroll host over LayoutFrame),
               RowView/RowModel (CoreText painter for Rust display lists,
               streaming veil), FrameRelay
  Session/     SessionViewController, SessionSource (Core/Fixture), new-session
               canvas
  Companions/  What works alongside a session: the companions panel (sheet on
               iPhone, inspector column beside the session on iPad / unfolded
               iPhone Duo), subagent transcripts, side chats, terminals (grid
               painter + key bar over the core's emulator)
  Composer/    ComposerBar (glass capsule ⇄ card with inline context chips;
               send/queue/steer/stop), question and queue panels, attachments
  Threads/     Sessions (foldable sidebar sections)/folder/search lists,
               cells, new-project folder browser
  Shell/       Tab bar (Sessions, Settings, search; "New session" accessory
               with live summary), sign-in, machine picker
  Debug/       Transcript lab + hitch meter
```

### Companions

A session's subagents, side chats and terminals (`Companions/`):

- **Subagents** come off the transcript (`SessionHandle.subagents()`): a
  spawn's card opens its own transcript, read-only — the host's live doc
  while it runs, its uploaded copy once done (`TranscriptView.attachSubagent`).
- **Side chats** are child chats: a fresh one shares its parent's host,
  folder and agent (`CoreClient.createSideChat`; closed unused, it's
  deleted), a fork copies the conversation through its latest reply
  (`forkSideChat`, answered by the chat's host).
- **Terminals** are PTYs on the chat's host. `crates/mobile/src/terminal`
  runs the desktop's emulator and echo predictor (`crates/term`) over the
  client's resumable `TerminalLink`; Swift paints frames and sends keys.
  Terminals persist per chat and reattach after a relaunch.

### Transcript pipeline

1. `commutator-client` applies session-doc updates incrementally (O(changed
   entries)) and publishes immutable snapshots.
2. `TranscriptView.attach(client, chatId)` subscribes **in Rust**; the layout
   thread turns entries into rows (one per markdown block / user message / tool
   group), reparses streaming markdown incrementally, and measures every row
   with `commutator-text` at the viewport width.
3. Each pass publishes a `LayoutFrame`: exact heights + prefix-sum offsets.
   `rowsIn(y0, y1)` is a binary search; `display(i)` builds a display list
   (text runs at exact positions, boxes, links, scrollers, widgets).
4. `TranscriptListView` positions reusable `RowView`s at those offsets and
   paints runs with CoreText at the Rust coordinates — measurement and
   rendering can't disagree. Display models for rows beyond the viewport are
   prefetched off the main thread.
5. Agent visuals (`show_visual` Luau scripts) get a worker thread each
   (`crates/mobile/src/visual`): it runs the script, lays the node tree out,
   draws canvases and charts as vector paint, renders scenes and shaders with
   wgpu, and reports its height back to the row. `VisualView` paints what
   arrives (rasterized on the worker thread) and forwards touches; the demo's
   "Agent visuals" chat (`-demo -route chat:chat-visuals`) has one of each.

## Launch arguments

| Arg | Effect |
| --- | --- |
| `-demo` | Offline demo workspace (Rust `DemoHost`: registry, docs, streaming replies) |
| `-fast` / `-longreply` | Demo stream speed / reply length |
| `-big` / `-huge` | Demo transcripts with 120 / 600 turns |
| `-route chat:<id>` / `new` / `more` / `search` | Open a screen at launch |
| `-send-on-open <text>` | With `-route chat:<id>`: send this once the session is open, for rigs driving a real stack (DEBUG) |
| `-network-offline` | Report no network path whatever the OS says: a simulator shares its Mac's network (DEBUG) |
| `-no-relay-layer` | Behave as a build without the relay layer does: an old phone in an account of new devices (DEBUG) |
| `-companions` / `-terminal [-terminal-demo]` / `-subagent <tool id>` / `-side-chat <id>` | With `-route chat:<id>`: open its companions panel, a terminal (typing a scripted demo), a subagent, or a side chat (DEBUG) |
| `-signedout` | Clear stored credentials |
| `-account <server url> <token> <device id> [-host <machine id>]` | A device token minted on a local server (`remote_stack`, below), as if signed in (DEBUG) |
| `-harness <id>` | Default harness for new sessions (`mock` for live-stack tests) |
| `-bench` (with `-lab`) | Display-link fling benchmark; writes `Documents/bench.json` |
| `-measureopen` | Session open latency; writes `Documents/open.json` |
| `-lab [-turns N] [-autostream] [-autoscroll] [-top] [-meter]` | Transcript lab over fixture markdown, with an on-screen hitch meter |

## Tests

```sh
# Rust
cargo test -p commutator-text -p commutator-markdown -p commutator-client -p commutator-mobile
cargo test --release -p commutator-mobile --lib bench_layout -- --ignored --nocapture

# Line-break accuracy vs CoreText (same font bytes), flows, hitch benchmarks
xcodebuild … -only-testing:CommutatorTests/LineBreakAccuracyTests test
xcodebuild … -only-testing:CommutatorUITests/SessionFlowTests test
xcodebuild … -only-testing:CommutatorUITests/ScrollPerformanceTests test
```

Live stack (a local commutator-server + an engine signed in to it, mock harness;
Google can't sign in a server on localhost, so the example mints the phone's
token and prints the launch arguments):

```sh
cargo run -p commutator-client --example remote_stack -- /tmp/stack 8797
# → ios launch args: -account http://127.0.0.1:8797 <token> <id> -host <machine> -harness mock
TEST_RUNNER_COMMUTATOR_LIVE_ACCOUNT="http://127.0.0.1:8797 <token> <id>" TEST_RUNNER_COMMUTATOR_LIVE_HOST=<machine> \
  xcodebuild … -only-testing:CommutatorUITests/LiveStackTests test
```

The test launches the app with `-harness mock`: a chat's configured harness
wins over the engine's `COMMUTATOR_HARNESS` default, so without it a real agent
would run.

## Session notifications

Not yet: commutator-server has no push API, so the app registers no APNs token
(Zeron's PushNotifications.swift and the `aps-environment` entitlement were
cut).

## TestFlight release

Run the **TestFlight** workflow from GitHub Actions on `main`. It installs the
Rust iOS targets, selects the next build number from App Store Connect,
archives (device, Release — the Rust core builds in the archive) with
automatic signing, and uploads a TestFlight build (internal, or external with
beta review). Secrets: `AC_API_KEY_P8`, `AC_API_KEY_ID`,
`AC_API_ISSUER_ID`.
