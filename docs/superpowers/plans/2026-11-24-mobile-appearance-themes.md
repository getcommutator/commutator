# Mobile Appearance page and themes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The phone apps get a Settings → Appearance page (mode, light/dark theme, wallpaper), with the desktop's built-in themes applied to the whole app.

**Architecture:** `crates/mobile` reads `crates/theme`'s registry and resolves a `PhonePalette` (app roles, transcript `ColorRole`s, terminal, Android Material slots) for a variant in one Rust mapping. FFI hands it to iOS and Android, which only copy colours. The core keeps the selection so its own visuals and terminal cells follow it.

**Tech Stack:** Rust + UniFFI (`crates/mobile`), UIKit (iOS), Jetpack Compose + Material 3 (Android).

**Spec:** `docs/superpowers/specs/2026-11-24-mobile-appearance-themes-design.md`

## Global Constraints

- Theme data only from `crates/theme` (`commutator_theme::builtins::builtin_registry()`); no colour copies in Swift/Kotlin/JSON.
- The role mapping exists once, in Rust (`crates/mobile/src/theme/`).
- Commutator Light (`commutator-light`) / Dark (`commutator-dark`) render exactly as today.
- Selection is per device, no sync. iOS: UserDefaults `theme.light` / `theme.dark`; Android: `settings` keys `themeLight` / `themeDark`. Defaults `commutator-light` / `commutator-dark`.
- Mode storage unchanged: iOS `appearance` (Int, UIUserInterfaceStyle raw), Android `theme` (`ThemeMode` name).
- Unknown variant id → Commutator Light / Dark for that appearance.
- Colours cross FFI as `u32` 0xAARRGGBB.
- Never `cargo fmt` a whole crate: `rustfmt` only touched files.

## Review Focus

- A stored theme id that no longer exists (renamed/removed theme): app must start on Commutator, not crash or paint black. → Task 1 test `unknown_id_falls_back`, Task 3 launch path.
- A dark-only family (Dracula, Nord, …) in the light picker: the light picker lists only light variants, the dark picker only dark ones. → Task 2 test `families_split_by_appearance`.
- Switching theme while the app is running: chrome and transcript repaint without relaunch. → Task 4 UI test `testPickDarkTheme` (background pixel changes after picking Dracula).
- "Wallpaper colors" on, then a theme change on Android: Material slots stay dynamic; nothing crashes. → Task 5 manual check list.
- Low-contrast derived roles on unusual themes (e.g. SynthWave '84): text must stay readable on the user bubble and code background. → Task 1 test `derived_roles_keep_text_readable`.

---

### Task 1: Rust phone palette mapping

**Files:**
- Modify: `crates/mobile/Cargo.toml` (add `commutator-theme = { path = "../theme" }`, workspace style as the other deps)
- Create: `crates/mobile/src/theme/mod.rs` (types + `phone_palette`), `crates/mobile/src/theme/commutator.rs` (today's exact Commutator values), `crates/mobile/src/theme/tests.rs`
- Modify: `crates/mobile/src/lib.rs` (`mod theme;`)

**Interfaces:**
- Produces (all `#[derive(Debug, Clone, PartialEq, uniffi::Record)]`):
  - `PhonePalette { dark: bool, app: AppColors, roles: Vec<u32> /* index = ColorRole discriminant order in layout/display.rs */, terminal: TerminalColors, material: MaterialColors }`
  - `AppColors` — one `u32` field per colour in `apps/ios/Commutator/Design/Palette.swift` except `projectDots` (stays fixed in the apps): `background, row_active, elevated, text, secondary, tertiary, hairline, accent, toggle_on, on_accent, control_fill, accent_soft, danger, success, warning, user_bubble, code_background, code_border, chip, subline`, plus every other `static let` colour in that file (tool/agent roles), snake_cased.
  - `TerminalColors { background: u32, foreground: u32, selection: u32, ansi: Vec<u32> /* 16 */ }`
  - `MaterialColors` — one `u32` per slot `apps/android/.../design/Theme.kt` sets in `CommutatorLight`/`CommutatorDark`, camelCase → snake_case (`primary, on_primary, primary_container, …, surface_container_highest, inverse_surface, outline, outline_variant, error_container, …`).
  - `pub(crate) fn phone_palette(variant: &ThemeVariant) -> PhonePalette`
  - `pub(crate) fn variant_or_default(id: &str, dark: bool) -> &'static ThemeVariant` (registry lookup; unknown or wrong-appearance id → `commutator-dark` / `commutator-light`)

- [ ] **Step 1: Write the failing tests** in `theme/tests.rs`:
  - `commutator_matches_today`: `phone_palette(variant_or_default("commutator-dark", true))` equals `commutator::dark()` and light likewise; and spot-asserts literals from today's sources: `app.background == 0xFF060606` (dark) / `0xFFF3F3F5` (light), `app.text == 0xFFE8E8EA` / `0xFF27272C`, `terminal.background == 0xFF090909` / `0xFFFAFAFB`, `terminal.ansi[1] == 0xFFF87171` (dark) / `0xFFDC2626` (light).
  - `every_builtin_maps`: for every variant in `builtin_registry()`, `phone_palette(v).roles.len() == COLOR_ROLE_COUNT`, `terminal.ansi.len() == 16`, `dark == (v.appearance == Appearance::Dark)`.
  - `unknown_id_falls_back`: `variant_or_default("no-such-theme", true).id == "commutator-dark"`; `variant_or_default("commutator-dark", false).id == "commutator-light"`.
  - `derived_roles_keep_text_readable`: for every builtin, WCAG contrast of `app.text` on `app.user_bubble` and on `app.code_background` ≥ 4.5.

- [ ] **Step 2: Run** `cargo test -p commutator-mobile theme::` — expect compile failure (module missing).

- [ ] **Step 3: Implement.**
  - `commutator.rs`: `pub(super) fn light() -> PhonePalette`, `pub(super) fn dark() -> PhonePalette` with today's literal values copied from `Palette.swift`, iOS `ColorRole` switch (via `Palette.color(_:)`), Android `Theme.kt` (`CommutatorLight`/`CommutatorDark`) and `terminal/color.rs` (`ANSI_*`, `FG_*`, `BG_*`). These become the single copy; Task 3/5 delete the app-side literals.
  - `phone_palette`: Commutator family ids return the constants; other variants map directly (`background, raised→elevated, text, text_muted→secondary, text_faint→tertiary, border→hairline, solid→accent, on_solid→on_accent, danger/warning/success, syntax→Syntax* roles, terminal`) and derive phone-only roles as fixed alphas of `text` over `background` matching today's Commutator alphas (e.g. `control_fill = text @ 0.075`, `subline = secondary @ 0.5`), bubble/code/chip as `background` mixed toward `text` (pick mixes that hit the contrast test). Material slots derive from the same roles (primary = accent, surfaces = background/elevated ramp, error = danger).
  - `COLOR_ROLE_COUNT`: a const next to `ColorRole` (or an exhaustive `ColorRole::ALL` array) so the test and `roles` agree.

- [ ] **Step 4: Run** `cargo test -p commutator-mobile theme::` — expect PASS.

- [ ] **Step 5: Commit** `git add crates/mobile && git commit -m "Map the desktop themes to a phone palette in the mobile core"`

---

### Task 2: FFI and the core following the selection

**Files:**
- Create: `crates/mobile/src/client_ffi/themes.rs`
- Modify: `crates/mobile/src/client_ffi/mod.rs` (`mod themes; pub use themes::*;`)
- Modify: `crates/mobile/src/theme/mod.rs` (selection state)
- Modify: `crates/mobile/src/visual/palette.rs`, `crates/mobile/src/visual/mod.rs:600,674,687`, `crates/mobile/src/terminal/color.rs`, `crates/mobile/src/terminal/mod.rs:290-331`
- Test: `crates/mobile/src/theme/tests.rs`, existing `visual/tests.rs`, terminal tests

**Interfaces:**
- Consumes: Task 1 types and functions.
- Produces (`#[uniffi::export]` free functions):
  - `theme_families() -> Vec<ThemeFamilyInfo>`; `ThemeFamilyInfo { id: String, name: String, variants: Vec<ThemeVariantInfo> }`; `ThemeVariantInfo { id: String, name: String, dark: bool, swatches: Vec<u32> /* background, text, accent, success */ }`
  - `theme_palette(variant_id: String, dark: bool) -> PhonePalette` (via `variant_or_default`)
  - `set_theme_selection(light_id: String, dark_id: String)`
  - crate-internal: `pub(crate) fn current_palette(dark: bool) -> PhonePalette` (selection in a `static RwLock`, default Commutator)

- [ ] **Step 1: Failing tests:**
  - `families_split_by_appearance`: `theme_families()` has the same family ids, in registry order, as `builtin_registry()`; `"dracula"`'s variants are all `dark`; `"commutator"` has one light and one dark.
  - `selection_drives_core_palette`: `set_theme_selection("github-light", "dracula")` → `current_palette(true).app.background == phone_palette(dracula).app.background`; reset with `set_theme_selection("commutator-light", "commutator-dark")` at the end (tests share the static: guard with a `Mutex` in tests).
  - `terminal_ansi_follows_selection`: after selecting `dracula`, `terminal::color::rgb(CellColor::Indexed(1), true)` equals dracula's `ansi[1] & 0xFFFFFF`.
- [ ] **Step 2: Run** `cargo test -p commutator-mobile theme:: terminal visual` — expect FAIL.
- [ ] **Step 3: Implement.** Visual `Palette::new(dark)` reads `current_palette(dark)` (text, muted=secondary, faint=tertiary, accent, on_accent, success, warning, danger, border=hairline); terminal `rgb` takes ANSI from `current_palette(dark).terminal.ansi`, default fg/bg (`FG_*`/`BG_*` callers at `terminal/mod.rs:290`) from `terminal.foreground`/`background`; delete `ANSI_*`, `FG_*`, `BG_*` constants.
- [ ] **Step 4: Run** `cargo test -p commutator-mobile` — all PASS (existing visual/terminal tests unchanged: Commutator default).
- [ ] **Step 5: Commit** `git commit -m "Expose themes over FFI; core visuals and terminal follow the selection"`

---

### Task 3: iOS follows the theme

**Files:**
- Create: `apps/ios/Commutator/Design/ThemeStore.swift`
- Modify: `apps/ios/Commutator/Design/Palette.swift`, `apps/ios/Commutator/Companions/TerminalViewController.swift:21`, `apps/ios/Commutator/App/SceneDelegate.swift:14`
- Bindings: `apps/ios/Commutator/Core/Generated/` refresh via the target's Rust core build phase (`scripts/ios/build-core.sh`)
- Test: `apps/ios/CommutatorUITests/AppearanceTests.swift`

**Interfaces:**
- Consumes: `themeFamilies()`, `themePalette(variantId:dark:)`, `setThemeSelection(lightId:darkId:)`, `PhonePalette` (generated Swift names).
- Produces:
  - `enum ThemeStore { static var lightId: String; static var darkId: String; static func palette(dark: Bool) -> PhonePalette; static func select(lightId: String?, darkId: String?); static let didChange: Notification.Name }` — persists to UserDefaults `theme.light`/`theme.dark`, calls `setThemeSelection`, caches both palettes, posts `didChange`, then forces a repaint of every window (re-apply `overrideUserInterfaceStyle` around a trait flip, or `traitOverrides` — implementer's choice) .
  - `Palette.*` keep their names; each becomes `UIColor { ThemeStore.palette(dark: $0.userInterfaceStyle == .dark).app.<role> }` via a `color(_ keyPath:)` helper; `Palette.color(_ role: ColorRole)` reads `roles[role index]`; `projectDots` stay literal.
- [ ] **Step 1: Failing UI test** `testThemeAtLaunch`: launch in dark mode (`-appearance 2` as a UserDefaults launch argument) with a DEBUG launch arg `-theme-dark dracula`; a screenshot pixel of the sessions list background equals `themePalette("dracula", true).app.background` (Dracula's `#282A36`). Changing theme without a relaunch is Task 4's `testPickDarkTheme`.
- [ ] **Step 2: Run** `xcodebuild … -only-testing:CommutatorUITests/AppearanceTests test` — FAIL.
- [ ] **Step 3: Implement** ThemeStore, Palette, terminal chrome, SceneDelegate reads selection at launch (and the DEBUG `-theme-light/-theme-dark` args).
- [ ] **Step 4: Run** the UI test and the existing `CommutatorTests` — PASS; build for a device (`-destination 'generic/platform=iOS'`) — SUCCEEDED.
- [ ] **Step 5: Commit** `git commit -m "iOS: paint with the selected theme"`

---

### Task 4: iOS Appearance page

**Files:**
- Create: `apps/ios/Commutator/Shell/AppearanceViewController.swift` (mode, theme rows, wallpaper — the rows and handlers moved out of `MoreViewController`), `apps/ios/Commutator/Shell/ThemePickerViewController.swift`
- Modify: `apps/ios/Commutator/Shell/MoreViewController.swift:131-145, 175-191, 218-219` (remove Appearance + Wallpaper sections/handlers; add row `id: "appearance"`, title "Appearance", subtitle `"<Mode> · <dark or light theme family name>"`, disclosure)
- Test: `apps/ios/CommutatorUITests/AppearanceTests.swift`

**Interfaces:**
- Consumes: `ThemeStore` (Task 3), `WallpaperStore`, `WallpaperPicker` (existing).
- Produces: `AppearanceViewController(app: AppModel)`; `ThemePickerViewController(dark: Bool)` — list of `themeFamilies()` variants with `dark == self.dark`, grouped by family (section header = family name, row = variant name + swatch dots from `swatches`), checkmark on current, tap → `ThemeStore.select`.
- [ ] **Step 1: Failing UI test** `testPickDarkTheme`: Settings tab → "Appearance" → "Dark theme" → "Dracula" → back; the Appearance row reads "… · Dracula"; the visible background changed (pixel check as in Task 3). Accessibility ids: `settings-appearance`, `appearance-dark-theme`, `theme-dracula`.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** the two controllers in `MoreViewController`'s list style (`UICollectionLayoutListConfiguration(.insetGrouped)`); mode rows keep today's behaviour (`appearance` key + window override).
- [ ] **Step 4: Run** `AppearanceTests` — PASS.
- [ ] **Step 5: Commit** `git commit -m "iOS: an Appearance page with mode, themes and wallpaper"`

---

### Task 5: Android follows the theme

**Files:**
- Modify: `apps/android/app/src/main/java/dev/freethinkel/commutator/android/design/Theme.kt` (build `ColorScheme` from `PhonePalette.material`; delete `CommutatorLight`/`CommutatorDark` literals), `design/Palette.kt` (`TranscriptPalette` fills from `PhonePalette.roles`; keep the five Material-sourced roles from the scheme only when `dynamicColor` is on), `core/AppModel.kt:120-130` (`Appearance` gains `lightTheme: String, darkTheme: String`, persisted as `themeLight`/`themeDark`; `setAppearance` calls `setThemeSelection`), `ui/TerminalScreen.kt` (terminal chrome background/foreground from `PhonePalette.terminal`)

**Interfaces:**
- Consumes: Task 2 FFI (generated Kotlin: `themeFamilies()`, `themePalette(variantId, dark)`, `setThemeSelection(lightId, darkId)`).
- Produces: `data class Appearance(mode: ThemeMode, dynamicColor: Boolean, lightTheme: String = "commutator-light", darkTheme: String = "commutator-dark")`.
- [ ] **Step 1: Run** `ANDROID_HOME=$HOME/Library/Android/sdk ./gradlew :app:compileDebugKotlin` in `apps/android` on the current tree — note any pre-existing failures (a parallel session had one in `ComposerSurface.kt`) so they aren't blamed on this task.
- [ ] **Step 2: Implement** as listed. "Wallpaper colors" on → `dynamicLight/DarkColorScheme` replaces `material`, and TEXT, LINK/ACCENT, QUOTE_BAR, DIFF_HUNK, USER_BUBBLE come from that scheme as today.
- [ ] **Step 3: Run** the compile — PASS (or only the pre-existing failures).
- [ ] **Step 4: Manual check** on a device or emulator: Commutator looks unchanged; pick Dracula via `adb shell` prefs or Task 6's UI; turn "Wallpaper colors" on and off after a theme change — no crash, scheme switches.
- [ ] **Step 5: Commit** `git commit -m "Android: paint with the selected theme"`

---

### Task 6: Android Appearance page

**Files:**
- Create: `apps/android/app/src/main/java/dev/freethinkel/commutator/android/ui/AppearanceScreen.kt` (mode segmented buttons, "Light theme" / "Dark theme" rows, "Wallpaper colors" toggle, `WallpaperSettings` moved from `SettingsScreen.kt:283+`), theme picker as a `ModalBottomSheet` in the same file
- Modify: `ui/SettingsScreen.kt:118-161` (remove Appearance + Wallpaper sections; add an "Appearance" row with the same subtitle format as iOS), `ui/CommutatorRoot.kt` (route `Routes.APPEARANCE = "appearance"`, phone stack and large-screen settings pane like `PULL_REQUESTS`)

**Interfaces:**
- Consumes: `Appearance` (Task 5), `themeFamilies()`.
- Produces: `@Composable fun AppearanceScreen(model: AppModel, onBack: () -> Unit)`.
- [ ] **Step 1: Implement.** Picker lists variants with `dark == pickingDark`, grouped by family, swatch dots, radio on current.
- [ ] **Step 2: Run** `./gradlew :app:compileDebugKotlin` — PASS (or only pre-existing failures).
- [ ] **Step 3: Manual check:** Settings → Appearance → Dark theme → Dracula: app repaints; back shows "… · Dracula".
- [ ] **Step 4: Commit** `git commit -m "Android: an Appearance page with mode, themes and wallpaper"`
