# Vardoger for Android

A Jetpack Compose viewport onto one of your machines, built on the same Rust
mobile core as the iOS app (`crates/mobile`). As on iOS, it signs in to your
Vardoger server (Google, PKCE, back to `vardoger://auth`), lists the account's
machines and drives the picked one's engine through the server's relay. Cut
from Zeron's app, with no backend here: Cloud, Providers/agent accounts,
onboarding, voice, direct terminals and the local-network permission. **Rust decides what to paint and
where; Kotlin paints, scrolls and handles gestures.** The transcript's text
measurement, markdown layout, prefix-sum virtualization and display lists are
the exact code iOS runs — see [`docs/mobile-rewrite.md`](../../docs/mobile-rewrite.md).

The UI is Material 3 Expressive (`MaterialExpressiveTheme`, expressive motion,
flexible app bars, shape-morphing loading indicators, connected button groups,
segmented lists) themed with Vardoger's own palette and Geist type.

## Build & run

Requires JDK 21, the Android SDK (platform 37, NDK 29.0.14206865), and Rust
with `cargo install cargo-ndk` and
`rustup target add aarch64-linux-android x86_64-linux-android`.

```sh
cd apps/android
./gradlew :app:installDebug
```

The `buildCore` task runs `scripts/android/build-core.sh`, which builds
`crates/mobile` for Android (`jniLibs`) and generates its Kotlin bindings into
`target/android-core/`. `-PvardogerSkipCore` reuses the last build while
iterating on Kotlin; `VARDOGER_ANDROID_ABIS=arm64-v8a` builds one ABI.
`scripts/android/gen-icons.sh` rasterizes the shared tool/file SVG icons
(needs `rsvg-convert`). The Geist fonts are read straight from the iOS app's
`Fonts/` folder, so both platforms measure and draw the same bytes.

## Releases

Zeron's release workflow (APK on zeron.sh, Play uploads) was not ported.

## Layout

```
core/        AppModel (owns CoreClient, republishes snapshots as flows),
             CredentialStore, Fonts + AndroidMeasurer (Minikin fallback
             measurement for glyphs Geist lacks)
design/      VardogerTheme (Material 3 Expressive), transcript palette
transcript/  TranscriptState (layout engine + viewport: anchoring, follow the
             tail), Transcript (virtualized rows over LayoutFrame), RowModel
             (canvas painter for Rust display lists, streaming veil, fades),
             Widgets (copy, disclosures, tool rail, shimmer, images…)
terminal/    TerminalView (a shell on the chat's host: paints the Rust
             emulator's frames, IME and hardware keys, scrollback fling,
             pinch to resize, selection), TerminalGlyphs (box drawing, blocks)
ui/          Sign-in and the machine picker, sessions (a Material 3 bottom
             app bar with the New session FAB), session + composer,
             companions (subagents, side chats, terminals: chips above the
             composer and a sheet), subagent transcripts, terminal + key bar,
             new session, search, settings.
             TabletShell: large screens (≥ 700dp wide) put the sessions
             sidebar beside the open session and its companions in an
             inspector column (iPad's split); Settings is a dialog there.
```

The core links C++ (`libc++_shared.so` ships beside it: `build-core.sh` passes
`--link-libcxx-shared` to cargo-ndk).

## Launch extras

Mirrors the iOS launch arguments:

```sh
adb shell am start -n dev.freethinkel.vardoger.android/.MainActivity \
  --ez demo true --es route chat:chat-veil
```

| Extra | Effect |
| --- | --- |
| `--ez demo true` | Offline demo workspace (Rust `DemoHost`) |
| `--ez fast true` / `--ez longreply true` | Demo stream speed / reply length |
| `--ez big true` / `--ez huge true` | Demo transcripts with 120 / 600 turns |
| `--es route chat:<id>` / `new` / `search` / `settings` (`more`) / `archived` | Open a screen at launch (with nobody signed in, on the demo) |
| `--es companion companions` / `terminal` / `side-chat:<id>` / `subagent:<tool id>` / `model-picker` | With `route chat:<id>`, open the companions sheet, a terminal, a side chat, a subagent or the model picker over it (`model-picker` also works with `route new`) |
| `--ez terminal-demo true` | The opened terminal types a short script (screenshots) |
| `--ez signedout true` | Clear stored credentials |
| `--es account "<server url> <token> <device id>"` `[--es host <machine id>]` | A device token minted on a local server (`cargo run -p vardoger-client --example remote_stack`; from the emulator the server is `http://10.0.2.2:<port>`), as if signed in (debuggable builds) |
| `--es wallpaper <path>` / `none` | Set (or clear) the wallpaper from a file the app can read, e.g. `adb push art.jpg /data/local/tmp/ && adb shell run-as dev.freethinkel.vardoger.android cp /data/local/tmp/art.jpg files/` then `--es wallpaper /data/user/0/dev.freethinkel.vardoger.android/files/art.jpg` |
| `--es wallpaper-effect <none\|dither\|ascii\|halftone\|scanlines>` | Wallpaper effect |
