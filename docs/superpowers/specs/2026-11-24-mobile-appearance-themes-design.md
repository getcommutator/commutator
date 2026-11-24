# Mobile Appearance page and themes

## Intent

The phone apps (iOS and Android) get a separate **Appearance** page in Settings that holds
everything about how the app looks: the light/dark mode, the theme, and the wallpaper. Themes
are new on the phone: the same built-in set the desktop has, defined once in `crates/theme`, so a
theme looks like itself on every device and new themes are added in one place.

## Goals

- Settings → **Appearance** page on iOS and Android with three parts: Mode, Theme, Wallpaper.
- Mode: System / Light / Dark (what Settings has today).
- Theme: a light theme and a dark theme, picked independently, like the desktop's
  `ThemeSelection`. Mode decides which of the two is on screen.
- The whole app follows the theme: chrome (backgrounds, text, accents, rules), the transcript
  (Rust-laid-out rows: text, code, syntax colours, bubbles, chips), the terminal, and the core's
  visuals (charts).
- Wallpaper moves here unchanged (choose / effect / remove; Android also its "Wallpaper colors"
  toggle).
- Themes come from `crates/theme`'s built-in registry, the same list as the desktop.

## Non-goals

- Syncing the selection between devices or with the desktop. It is stored on the device.
- Custom or imported themes, theme editing.
- Changing the desktop.

## Anti-goals

- A second copy of theme colours anywhere (generated Swift/Kotlin, JSON in the app bundles).
- Per-platform mapping from desktop roles to phone roles: the mapping exists once, in Rust.
- The default look changing: Commutator Light / Dark must render exactly as today.

## Constraints

- `crates/theme` is the only source of theme data. `crates/mobile` depends on it.
- iOS and Android stay at parity; both get colours from the same FFI.
- Existing settings carry over: iOS keeps its `appearance` UserDefaults key for the mode, Android
  its `ThemeMode`; wallpaper state is untouched.

## Approaches

1. **Themes through the core** (picked). `crates/mobile` reads `crates/theme` and hands the apps a
   resolved phone palette over FFI. One source, one mapping, the core's own visuals follow too,
   custom themes later land in one place. Cost: FFI work and moving both apps off hard-coded
   colours.
2. **Generated files.** A script exports `crates/theme` to Swift/Kotlin/JSON committed in each
   app. Faster to start, but the role mapping is written twice, the core's visuals don't follow,
   and generated copies live next to the source. Rejected: against the "one place" goal.

The user picked per-device selection, no sync.

## Design

### Core (`crates/mobile`)

- Add `commutator-theme` as a dependency.
- New FFI module (e.g. `client_ffi/themes.rs`), free functions, no client needed:
  - `theme_families() -> Vec<ThemeFamilyInfo>`: `id`, `name`, `variants`; each variant `id`,
    `name`, `dark: bool`, and a few swatch colours (background, text, accent, and one more) for
    the picker.
  - `theme_palette(variant_id) -> PhonePalette`: everything the apps paint with, resolved for that
    variant:
    - app roles, the ones in `apps/ios/Commutator/Design/Palette.swift` today (background,
      elevated, text, secondary, tertiary, hairline, accent, onAccent, toggleOn, controlFill,
      accentSoft, danger, success, warning, userBubble, codeBackground, codeBorder, chip,
      rowActive, …);
    - one colour per transcript `ColorRole` (`crates/mobile/src/layout/display.rs`, syntax roles
      included), indexed by the role, so the iOS and Android role tables fill from it;
    - terminal: background, foreground, selection, 16 ANSI colours.
  - `set_theme_selection(light_id, dark_id)`: tells the core which variants the visuals paint with.
- One mapping function, `fn phone_palette(&ThemeVariant) -> PhonePalette`. Desktop roles map
  directly where they exist (`background`, `raised`, `text`, `text_muted`, `text_faint`, `border`,
  `solid`/`on_solid`, `danger`/`warning`/`success`, `syntax`, `terminal`). Phone-only roles
  (user bubble, chip, code background, control fill, toggle track, active row) are derived from
  theme colours by fixed rules. If a rule can't reproduce today's Commutator values exactly,
  the mapping may carry explicit values for the two Commutator variants.
- `visual/palette.rs`: `Palette::new(dark)` builds from the selected variant's `PhonePalette`
  instead of literals (chart series still derived from the accent).
- The terminal is themed in the core: cell colours are resolved in Rust today
  (`terminal/color.rs`, `rgb(cell, dark)`), and `set_theme_selection` drives them from the
  selected variant's terminal palette. The FFI terminal fields are only for the apps' terminal
  chrome (the background around the grid).
- Android's Material `ColorScheme` slots (primary/secondary/tertiary and containers, the
  surfaceContainer levels, inverse, outline, error containers) are part of `PhonePalette` too,
  derived in the same Rust mapping, so Kotlin only copies them into a `ColorScheme`. The two
  Commutator variants carry today's exact values from `design/Theme.kt` (including its teal
  tertiary).
- What follows the theme: everything with a theme source, plus the roles that today are alpha
  over white/black (tool rails, agent cards, wells, sublines, …), which become the same alphas
  over the theme's `text` / `background`. Project dots (`projectDots`) stay fixed on every theme:
  they identify projects, not the look.
- `recommended_surface_treatment` is ignored on the phone for now: iOS keeps its glass chrome on
  every theme. A per-theme glass/solid switch is a later improvement.
- An unknown variant id falls back to Commutator Light / Dark.

### Storage (per device)

- iOS: UserDefaults `theme.light` / `theme.dark` (defaults `commutator-light` /
  `commutator-dark`). Mode stays in `appearance`.
- Android: next to the existing appearance settings in `AppModel` (same store as `ThemeMode`).
- On launch each app reads the selection, passes it to `set_theme_selection`, and loads the two
  palettes.

### iOS

- A small `ThemeStore` holds the two `PhonePalette`s and the selection, and posts a change
  notification.
- `Palette` colours become dynamic providers that read `ThemeStore` for the trait's light/dark
  style, so call sites (`Palette.text`, …) don't change. The transcript's role → colour table and
  the terminal's colours read the same store.
- A theme change must repaint every visible screen without a relaunch (dynamic colours only
  re-resolve on trait changes, so the store has to force that, e.g. by re-applying the window's
  style override, and the transcript repaints its rows).
- Settings: the Appearance and Wallpaper sections leave `MoreViewController`; one row
  **Appearance** (subtitle like "System · Commutator") pushes a new `AppearanceViewController`:
  - Mode: System / Light / Dark (checkmarks).
  - Theme: "Light theme ›" and "Dark theme ›", each opening a picker listing the variants of that
    appearance grouped by family, with swatches and a checkmark on the current one.
  - Wallpaper: today's rows, moved as they are.

### Android

- `design/Palette.kt` (the `ColorRole` table) fills from `PhonePalette`; `design/Theme.kt` builds
  the light and dark `ColorScheme` from it.
- "Wallpaper colors" (Material dynamic colour) keeps today's behaviour exactly: when on, the
  system's dynamic scheme replaces the theme's Material slots, and the transcript roles that take
  their colour from the Material scheme today (text, link/accent, quote bar, diff hunk, user
  bubble) keep doing so. That's a deliberate Android-only exception: the user asked for the
  wallpaper's palette. It moves to the Appearance page.
- Settings: one Appearance row navigates to a new `AppearanceScreen` route with the same three
  parts; the theme picker is its own screen or a sheet, same content as iOS.

## Testing

- Rust: every built-in variant maps to a complete `PhonePalette` (no panics, every role set);
  Commutator Light / Dark map to exactly today's values (the current literals in
  `Palette.swift` / `visual/palette.rs` as expected values); unknown id falls back.
- iOS: builds; a UI test opens Settings → Appearance, picks another dark theme, and the
  background colour changes.
- Android: builds (`compileDebugKotlin`), checked by hand.

## Left to the builder

- Exact derivation rules for the phone-only roles, and the swatch colours shown in the picker.
- How iOS forces the repaint on a theme change, within the requirement above.
- Whether the Android theme picker is a screen or a sheet.
