//! Terminal paint + input encoding.
//!
//! - theme-owned background/ANSI16 plus the xterm 256-color cube/grayscale;
//! - keystroke → PTY byte encoding (printables, control keys, arrows/nav
//!   escape sequences, Ctrl- combos, Alt prefixing);
//! - the ordered input queue and the 80 ms resize debounce constant (the
//!   panel drives the writes and timers; the buffer logic here is pure);
//! - [`TerminalElement`] — a custom gpui element that measures cell metrics
//!   from the real mono font (the "font probe"), reports the resulting
//!   cols×rows back to the panel, and paints the grid: background quads for
//!   non-default cells, column-pinned shaped rows with ligatures, Ghostty
//!   sprite glyphs, and the cursor block.

use gpui::{
    App, Bounds, Entity, GlobalElementId, Hsla, LayoutId, Modifiers, PaintQuad, PathBuilder,
    Pixels, RenderImage, ShapedLine, SharedString, Style, TextRun, Window, fill, font, outline,
    point, px, relative, size,
};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use crate::theme::{Appearance, Theme, rgb_to_hsl};

use super::emulator::{CellColor, CellSnapshot, CursorShape, ImagePlacement, Side};
use super::panel::TerminalPanel;
use super::sprite;

/// Default terminal font metrics; the rendered values come from the theme
/// (size) and the settings (row height).
pub const TERM_FONT_SIZE: f32 = 13.0;
/// Fallback row height in px, before settings load.
pub const TERM_LINE_HEIGHT: f32 = 18.0;
/// Inner padding of the grid area.
pub const TERM_PADDING: f32 = 2.0;

/// Nerd Font families commonly installed alongside a user's terminal font.
/// Keep the configured mono font first; these only fill glyphs it lacks.
const NERD_FONT_FALLBACKS: [&str; 3] = [
    "Symbols Nerd Font Mono",
    "IoskeleyMonoNL Nerd Font Mono",
    "FiraCode Nerd Font Mono",
];

/// Debounce for `ResizeTerminal` after viewport-driven size changes.
pub const RESIZE_DEBOUNCE_MS: u64 = 80;

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// The panel fill behind the grid. On glass the opaque tone thins to a
/// translucent wash so the blurred desktop reads through like the rest of the
/// chrome (same move as [`Theme::card_glass_bg`]); opaque platforms keep the
/// true tone. Explicit cell backgrounds (vim colorschemes etc.) still paint
/// their own opaque quads on top.
pub fn terminal_panel_bg(theme: &Theme) -> Hsla {
    if theme.is_glass() {
        theme.terminal.background.opacity(0.4)
    } else {
        theme.terminal.background
    }
}

fn rgb8(r: u8, g: u8, b: u8) -> Hsla {
    let (h, s, l) = rgb_to_hsl(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    gpui::hsla(h, s, l, 1.0)
}

/// xterm 256-color cube component levels.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// Resolve the xterm extended-color range to RGB components.
///
/// Three ranges, treated differently on purpose:
///
/// - **0-15** are named slots and are resolved directly from `Theme::terminal`.
/// - **16-231** is the 6×6×6 cube: a program asking for index 196 is asking for
///   `#ff0000` by arithmetic, and remapping it would be inventing colors the
///   caller did not pick. Left alone in both appearances, same as iTerm/Apple
///   Terminal light themes.
/// - **232-255** is the grayscale ramp, which tools use for *de-emphasis*
///   rather than for a specific grey. Its dark→light direction only reads as
///   "dim" on a dark background, so light mode mirrors the ramp: index 232
///   stays the faintest and 255 the strongest in both. Without this the ramp is
///   the single biggest legibility hole on white, because its bright end —
///   where most "dim hint" text lands — is the end that vanishes.
fn extended_indexed_rgb(appearance: Appearance, index: u8) -> (u8, u8, u8) {
    match index {
        0..=15 => unreachable!("ANSI16 is resolved from the active theme"),
        16..=231 => {
            let n = index as usize - 16;
            (
                CUBE_LEVELS[n / 36],
                CUBE_LEVELS[(n / 6) % 6],
                CUBE_LEVELS[n % 6],
            )
        }
        232..=255 => {
            let step = index - 232;
            let step = match appearance {
                Appearance::Dark => step,
                Appearance::Light => 23 - step,
            };
            let v = 8 + 10 * step;
            (v, v, v)
        }
    }
}

/// Resolve a cell color to paint against the theme.
pub fn resolve_color(color: CellColor, theme: &Theme) -> Hsla {
    match color {
        CellColor::Foreground => theme.terminal.foreground,
        CellColor::Background => theme.terminal.background,
        CellColor::Indexed(ix @ 0..=15) => theme.terminal.ansi[ix as usize],
        CellColor::Indexed(ix) => {
            let (r, g, b) = extended_indexed_rgb(theme.appearance, ix);
            rgb8(r, g, b)
        }
        CellColor::Rgb(r, g, b) => rgb8(r, g, b),
    }
}

// ---------------------------------------------------------------------------
// Pointer → cell
// ---------------------------------------------------------------------------

/// Minimum pointer travel before a press turns into a selection.
///
/// Without it, the click that focuses the panel starts a one-cell selection if
/// the hand moves a pixel — and once anything copies on selection change, that
/// silently clobbers the clipboard. Matches the threshold zed uses for the same
/// reason (`SELECTION_DRAG_THRESHOLD`), which is gpui's own `div` drag
/// threshold.
pub const SELECTION_DRAG_THRESHOLD: f32 = 2.0;

/// Which cell a pointer landed on, and which edge of it a selection anchors to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellHit {
    pub row: usize,
    pub col: usize,
    /// Selections anchor to a cell *edge*, not a cell: pressing on the left
    /// half of a glyph includes it, the right half excludes it.
    pub side: Side,
}

/// Map a position *relative to the grid's top-left glyph* onto a cell.
///
/// Positions outside the grid clamp to the nearest cell rather than returning
/// `None`, because that is what a drag needs: the pointer routinely leaves the
/// panel mid-gesture, and the selection should extend to the edge it left
/// through instead of freezing at the last sample taken inside.
///
/// Overshoot also *forces the side*, which clamping alone does not give you.
/// Dragging past the bottom should take the last line whole, even when the
/// pointer drifted left of where it started — deriving the side from x there
/// would stop the selection mid-row. Same rule going up, mirrored. This is the
/// behaviour alacritty and zed's `grid_point_and_side` both implement.
pub fn cell_at(x: f32, y: f32, cell_w: f32, line_h: f32, cols: usize, rows: usize) -> CellHit {
    // Degenerate metrics (a zero-size grid, or a font probe that returned NaN)
    // would otherwise divide into garbage cell indices.
    let usable = |v: f32| v.is_finite() && v > 0.0;
    if cols == 0 || rows == 0 || !usable(cell_w) || !usable(line_h) {
        return CellHit {
            row: 0,
            col: 0,
            side: Side::Left,
        };
    }
    let x = if x.is_finite() { x } else { 0.0 };
    let y = if y.is_finite() { y } else { 0.0 };
    let last_col = cols - 1;
    let last_row = rows - 1;

    let raw_col = (x / cell_w).floor();
    let mut side = if x.max(0.0) % cell_w > cell_w / 2.0 {
        Side::Right
    } else {
        Side::Left
    };
    let col = if raw_col > last_col as f32 {
        side = Side::Right;
        last_col
    } else {
        raw_col.max(0.0) as usize
    };

    let raw_row = (y / line_h).floor();
    let row = if raw_row > last_row as f32 {
        side = Side::Right;
        last_row
    } else if raw_row < 0.0 {
        side = Side::Left;
        0
    } else {
        raw_row as usize
    };

    CellHit { row, col, side }
}

// ---------------------------------------------------------------------------
// Keyboard → bytes
// ---------------------------------------------------------------------------

/// Encode a keystroke as PTY bytes. `None` means "not ours" — the event should
/// fall through (e.g. the platform-primary shortcuts that drive app actions).
///
/// `app_cursor` switches arrows/home/end from CSI to SS3 per DECCKM.
pub fn keystroke_bytes(
    key: &str,
    key_char: Option<&str>,
    mods: &Modifiers,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    // Platform-primary combos (Cmd on macOS, the super key elsewhere) belong
    // to the app keymap, never the PTY — except Cmd+Backspace: delete to the
    // line's start (Ctrl+U), and Cmd+Left/Right: to the line's start or end
    // (Ctrl+A, Ctrl+E), as Terminal, iTerm2 and Ghostty do.
    if mods.platform {
        return match key {
            "backspace" => Some(vec![0x15]),
            "left" => Some(vec![0x01]),
            "right" => Some(vec![0x05]),
            _ => None,
        };
    }
    if mods.alt {
        // Option+Left/Right: a word back or forward (ESC b, ESC f), which
        // shells and readline know, as Ghostty sends.
        match key {
            "left" if !mods.shift && !mods.control => return Some(b"\x1bb".to_vec()),
            "right" if !mods.shift && !mods.control => return Some(b"\x1bf".to_vec()),
            _ => {}
        }
        // ESC-prefix the same keystroke without alt.
        let inner = keystroke_bytes(
            key,
            key_char,
            &Modifiers {
                alt: false,
                ..*mods
            },
            app_cursor,
        )?;
        let mut out = vec![0x1b];
        out.extend(inner);
        return Some(out);
    }
    if mods.control {
        return control_bytes(key);
    }

    let seq = |csi: &[u8], ss3: &[u8]| {
        Some(if app_cursor {
            ss3.to_vec()
        } else {
            csi.to_vec()
        })
    };
    match key {
        "enter" => Some(b"\r".to_vec()),
        "backspace" => Some(vec![0x7f]),
        "tab" => Some(if mods.shift {
            b"\x1b[Z".to_vec()
        } else {
            b"\t".to_vec()
        }),
        "escape" => Some(vec![0x1b]),
        "space" => Some(b" ".to_vec()),
        "up" => seq(b"\x1b[A", b"\x1bOA"),
        "down" => seq(b"\x1b[B", b"\x1bOB"),
        "right" => seq(b"\x1b[C", b"\x1bOC"),
        "left" => seq(b"\x1b[D", b"\x1bOD"),
        "home" => seq(b"\x1b[H", b"\x1bOH"),
        "end" => seq(b"\x1b[F", b"\x1bOF"),
        "insert" => Some(b"\x1b[2~".to_vec()),
        "delete" => Some(b"\x1b[3~".to_vec()),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        "f1" => Some(b"\x1bOP".to_vec()),
        "f2" => Some(b"\x1bOQ".to_vec()),
        "f3" => Some(b"\x1bOR".to_vec()),
        "f4" => Some(b"\x1bOS".to_vec()),
        "f5" => Some(b"\x1b[15~".to_vec()),
        "f6" => Some(b"\x1b[17~".to_vec()),
        "f7" => Some(b"\x1b[18~".to_vec()),
        "f8" => Some(b"\x1b[19~".to_vec()),
        "f9" => Some(b"\x1b[20~".to_vec()),
        "f10" => Some(b"\x1b[21~".to_vec()),
        "f11" => Some(b"\x1b[23~".to_vec()),
        "f12" => Some(b"\x1b[24~".to_vec()),
        _ => {
            // Printable: prefer the typed character (IME/shift-aware).
            let text = key_char.filter(|c| !c.is_empty()).or({
                // Fall back to single-char key names ("a", "/", …).
                if key.chars().count() == 1 {
                    Some(key)
                } else {
                    None
                }
            })?;
            Some(text.as_bytes().to_vec())
        }
    }
}

/// How a keydown reaches the PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRoute {
    /// Write these bytes now.
    Bytes(Vec<u8>),
    /// Typed text: let the platform's text input deliver it, so input
    /// methods, dead keys and keyboard layouts compose it first.
    Text,
    /// Not the terminal's (app shortcuts).
    NotOurs,
}

/// Route a keydown. Printable text goes through the platform text input;
/// named keys and modified keys are encoded here.
///
/// Option follows the layout: where Option types an ASCII symbol (`@`, `|`,
/// `\` on many European layouts) that symbol is typed; where it would type
/// something else (`∫` for Option-B on US layouts) it acts as Alt/Meta, so
/// shell word motions and editor bindings work without a setting.
pub fn route_key(
    key: &str,
    key_char: Option<&str>,
    mods: &Modifiers,
    app_cursor: bool,
) -> KeyRoute {
    if mods.platform {
        // Cmd+Backspace and Cmd+Left/Right are the PTY's (see
        // keystroke_bytes); every other platform-primary combo belongs to
        // the app keymap.
        return keystroke_bytes(key, key_char, mods, app_cursor)
            .map_or(KeyRoute::NotOurs, KeyRoute::Bytes);
    }
    let printable = key_char.filter(|c| !c.is_empty() && !c.chars().any(char::is_control));
    if mods.alt && !mods.control {
        if let Some(text) = printable
            && text.is_ascii()
            && !text.eq_ignore_ascii_case(key)
            && key.chars().count() == 1
        {
            return KeyRoute::Text;
        }
        // Meta: ESC + the key as typed without Option.
        let base = (key.chars().count() == 1).then(|| {
            if mods.shift {
                key.to_uppercase()
            } else {
                key.to_string()
            }
        });
        return keystroke_bytes(key, base.as_deref(), mods, app_cursor)
            .map_or(KeyRoute::NotOurs, KeyRoute::Bytes);
    }
    if !mods.control && !mods.function && printable.is_some() && !is_named_key(key) {
        return KeyRoute::Text;
    }
    keystroke_bytes(key, key_char, mods, app_cursor).map_or(KeyRoute::NotOurs, KeyRoute::Bytes)
}

fn is_named_key(key: &str) -> bool {
    matches!(
        key,
        "enter"
            | "backspace"
            | "tab"
            | "escape"
            | "space"
            | "up"
            | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "insert"
            | "delete"
            | "pageup"
            | "pagedown"
    ) || (key.len() >= 2 && key.starts_with('f') && key[1..].chars().all(|c| c.is_ascii_digit()))
}

/// Ctrl-key encoding (caret notation).
fn control_bytes(key: &str) -> Option<Vec<u8>> {
    let mut chars = key.chars();
    let (c, rest) = (chars.next()?, chars.next());
    if rest.is_some() {
        return match key {
            "space" => Some(vec![0x00]),
            "backspace" => Some(vec![0x08]),
            "enter" => Some(b"\r".to_vec()),
            _ => None,
        };
    }
    match c {
        'a'..='z' => Some(vec![c as u8 - b'a' + 1]),
        '@' => Some(vec![0x00]),
        '[' => Some(vec![0x1b]),
        '\\' => Some(vec![0x1c]),
        ']' => Some(vec![0x1d]),
        '^' => Some(vec![0x1e]),
        '_' | '/' => Some(vec![0x1f]),
        '?' => Some(vec![0x7f]),
        _ => None,
    }
}

/// Enter under the kitty keyboard protocol the program asked for (`flags`,
/// from the emulator): with Shift, Ctrl or Option it is `CSI 13;<mods> u`,
/// which the legacy `\r` has no room for (Claude Code's Shift+Enter newline);
/// plain Enter too once the app asks for every key as an escape code. `None`:
/// the legacy encoding.
pub fn kitty_enter(key: &str, mods: &Modifiers, flags: u16) -> Option<Vec<u8>> {
    const DISAMBIGUATE: u16 = 1;
    const ALL_AS_ESCAPES: u16 = 8;
    if key != "enter" || mods.platform || flags & DISAMBIGUATE == 0 {
        return None;
    }
    let m = 1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.control);
    match m {
        1 if flags & ALL_AS_ESCAPES != 0 => Some(b"\x1b[13u".to_vec()),
        1 => None,
        m => Some(format!("\x1b[13;{m}u").into_bytes()),
    }
}

/// What turns a click into opening a link: Cmd on macOS, Ctrl elsewhere.
pub fn link_modifier(mods: &Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        mods.platform
    } else {
        mods.control
    }
}

/// The terminal's own shortcuts (copy, paste, find): Cmd on macOS. Elsewhere
/// Ctrl+Shift, as GNOME Terminal, kitty and Ghostty have it, since a
/// terminal's Ctrl+letter belongs to the shell.
pub fn app_shortcut(mods: &Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        mods.platform
    } else {
        mods.control && mods.shift && !mods.alt && !mods.platform
    }
}

/// Cmd+Shift+G steps the search back on macOS; elsewhere Shift is part of
/// the shortcut, so it only goes forward.
pub fn search_back(mods: &Modifiers) -> bool {
    cfg!(target_os = "macos") && mods.shift
}

/// Wrap pasted text for the PTY (bracketed-paste aware; strips the one control
/// sequence a paste could inject).
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let sanitized = text.replace("\x1b[201~", "");
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(sanitized.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        sanitized.into_bytes()
    }
}

/// Files dropped on a terminal, as text: each path shell-escaped with
/// backslashes and space-separated, as Ghostty does, so a shell or an agent
/// TUI (Claude Code picks up image paths) reads them whole.
pub fn dropped_paths<P: AsRef<std::path::Path>>(paths: &[P]) -> String {
    let escape = |p: &P| {
        p.as_ref()
            .to_string_lossy()
            .chars()
            .fold(String::new(), |mut out, c| {
                if c.is_whitespace() || "\\'\"`$&|;<>()[]{}*?!#~".contains(c) {
                    out.push('\\');
                }
                out.push(c);
                out
            })
    };
    paths.iter().map(escape).collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Pointer and focus reports
// ---------------------------------------------------------------------------

/// Button codes for mouse reports.
pub const MOUSE_LEFT: u8 = 0;
pub const MOUSE_MIDDLE: u8 = 1;
pub const MOUSE_RIGHT: u8 = 2;
/// Legacy encoding's "some button released"; motion with nothing held.
pub const MOUSE_NONE: u8 = 3;
pub const MOUSE_WHEEL_UP: u8 = 64;
pub const MOUSE_WHEEL_DOWN: u8 = 65;
/// Added to a button code for motion reports.
pub const MOUSE_MOTION: u8 = 32;

/// Encode a pointer event for a program that asked for mouse reports.
/// `col`/`row` are 0-based viewport cells. SGR (1006) reports releases with
/// their button; the legacy encoding cannot name one and cannot address
/// cells past 223, so such events are dropped rather than sent garbled.
pub fn mouse_report(
    button: u8,
    col: usize,
    row: usize,
    release: bool,
    mods: &Modifiers,
    sgr: bool,
) -> Option<Vec<u8>> {
    let mut code = button;
    if mods.shift {
        code += 4;
    }
    if mods.alt {
        code += 8;
    }
    if mods.control {
        code += 16;
    }
    if sgr {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes());
    }
    if release {
        code = (code & !0b11) | MOUSE_NONE;
    }
    let encode = |v: usize| u8::try_from(v + 1 + 32).ok().filter(|b| *b < 255);
    Some(vec![
        0x1b,
        b'[',
        b'M',
        32 + code,
        encode(col)?,
        encode(row)?,
    ])
}

/// Focus change report (1004).
pub fn focus_report(focused: bool) -> &'static [u8] {
    if focused { b"\x1b[I" } else { b"\x1b[O" }
}

/// Wheel rows as arrow keys for a full-screen program without mouse
/// reporting (alternate scroll, 1007). Positive rows scroll up.
pub fn wheel_arrows(rows: i32, app_cursor: bool) -> Vec<u8> {
    let arrow: &[u8] = match (rows > 0, app_cursor) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1b[A",
        (false, true) => b"\x1bOB",
        (false, false) => b"\x1b[B",
    };
    arrow.repeat(rows.unsigned_abs() as usize)
}

// ---------------------------------------------------------------------------
// Contrast
// ---------------------------------------------------------------------------

/// Contrast floor for text the program colored itself (WCAG ratio). Themes
/// are tuned for their own default foreground; explicit colors are not
/// (blue on black, a 256-color grey on a dark panel), and below this they
/// stop being readable.
pub const MIN_CONTRAST: f32 = 3.0;

fn relative_luminance(color: Hsla) -> f32 {
    let rgb = color.to_rgb();
    let lin = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(rgb.r) + 0.7152 * lin(rgb.g) + 0.0722 * lin(rgb.b)
}

pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `fg`, moved in lightness just far enough from `bg` to reach `min`
/// contrast, keeping its hue. A foreground identical to its background is
/// deliberate (hidden blocks) and left alone.
pub fn ensure_contrast(fg: Hsla, bg: Hsla, min: f32) -> Hsla {
    if fg == bg || contrast_ratio(fg, bg) >= min {
        return fg;
    }
    let toward_light = relative_luminance(bg) < 0.18;
    let (mut lo, mut hi) = if toward_light {
        (fg.l, 1.0)
    } else {
        (0.0, fg.l)
    };
    let mut best = Hsla {
        l: if toward_light { 1.0 } else { 0.0 },
        ..fg
    };
    for _ in 0..16 {
        let mid = (lo + hi) / 2.0;
        let candidate = Hsla { l: mid, ..fg };
        if contrast_ratio(candidate, bg) >= min {
            best = candidate;
            if toward_light { hi = mid } else { lo = mid }
        } else if toward_light {
            lo = mid
        } else {
            hi = mid
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Input queue (pure buffer; the panel owns the write)
// ---------------------------------------------------------------------------

/// Keyboard bytes on their way to the PTY. A keystroke is written the moment
/// it lands; while that write is in flight, later keys gather here and ride
/// the next one. One write at a time keeps keys in typed order, and nothing
/// ever waits on a timer.
#[derive(Debug, Default)]
pub struct InputQueue {
    pending: Vec<u8>,
    in_flight: bool,
}

impl InputQueue {
    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    /// The next write, when one may start now: nothing in flight and
    /// something pending. The caller must [`Self::finish`] it.
    pub fn start(&mut self) -> Option<Vec<u8>> {
        if self.in_flight || self.pending.is_empty() {
            return None;
        }
        self.in_flight = true;
        Some(std::mem::take(&mut self.pending))
    }

    /// The in-flight write completed (or failed); the next may start.
    pub fn finish(&mut self) {
        self.in_flight = false;
    }

    #[cfg(test)]
    pub fn pending(&self) -> &[u8] {
        &self.pending
    }
}

// ---------------------------------------------------------------------------
// Grid element
// ---------------------------------------------------------------------------

/// Paints the active tab's grid. Cell metrics come from the resolved mono font
/// each frame (font probe): `em_advance` for the cell width, the fixed line
/// height for rows. The measured cols×rows feed back into the panel, which
/// resizes the emulator immediately and debounces the `ResizeTerminal` RPC.
pub struct TerminalElement {
    panel: Entity<TerminalPanel>,
    focused: bool,
}

impl TerminalElement {
    pub fn new(panel: Entity<TerminalPanel>, focused: bool) -> Self {
        Self { panel, focused }
    }
}

pub struct TerminalPrepaint {
    bg_quads: Vec<PaintQuad>,
    /// Connected, rounded selection shapes, painted over backgrounds and
    /// under glyphs.
    selection: Vec<(gpui::Path<Pixels>, Hsla)>,
    /// The open search's matches: every hit a soft wash, the stepped-to one
    /// stronger.
    marks: Vec<PaintQuad>,
    /// Kitty graphics: images under the glyphs (negative layers) and over
    /// them, painted around the text runs.
    images_below: Vec<(Bounds<Pixels>, Arc<RenderImage>)>,
    images_above: Vec<(Bounds<Pixels>, Arc<RenderImage>)>,
    /// Per row, its top edge and the shaped segments with the grid COLUMN
    /// each one starts at. Not one line per row: see [`shape_row`].
    lines: Vec<(Pixels, Vec<(usize, ShapedLine)>)>,
    /// Ghostty-style vector sprites for box drawing, Powerline, Braille,
    /// blocks, and legacy computing glyphs.
    glyphs: Vec<(Pixels, CustomGlyph)>,
    /// Underlines text runs cannot draw: double, dotted, and dashed.
    decor: Vec<(Pixels, Decor)>,
    sprite_stroke: f32,
    trail: Vec<(gpui::Path<Pixels>, gpui::Background)>,
    /// Grid cell advance, so paint can place segments by column.
    cell_w: Pixels,
    /// Grid row height, so paint places rows on the same baselines prepaint
    /// measured the grid with.
    line_h: Pixels,
    cursor: Option<PaintQuad>,
    cursor_under_text: bool,
    /// In-progress input-method composition at the cursor: its backdrop and
    /// shaped text.
    marked: Option<(PaintQuad, gpui::Point<Pixels>, ShapedLine)>,
}

/// An underline painted as quads under a cell, not by its text run.
struct Decor {
    col: usize,
    cells: usize,
    color: Hsla,
    style: super::emulator::UnderlineStyle,
}

/// The quads of a [`Decor`] in `cell` (the cell's whole height): 1 px lines near
/// its bottom, as gpui's own underline sits; dots and dashes restart at each
/// cell, so a run of them looks even at any width.
fn decor_quads(decor: &Decor, cell: Bounds<Pixels>) -> Vec<Bounds<Pixels>> {
    use super::emulator::UnderlineStyle as U;
    let (thick, bottom) = (px(1.0), cell.bottom() - px(2.0));
    let line = |y: Pixels, from: Pixels, len: Pixels| Bounds::new(point(from, y), size(len, thick));
    let step = |on: f32, period: f32| {
        let mut x = f32::from(cell.left());
        let mut out = Vec::new();
        while x < f32::from(cell.right()) {
            out.push(line(
                bottom,
                px(x),
                px(on.min(f32::from(cell.right()) - x)),
            ));
            x += period;
        }
        out
    };
    match decor.style {
        U::Double => vec![
            line(bottom, cell.left(), cell.size.width),
            line(bottom - px(2.0), cell.left(), cell.size.width),
        ],
        U::Dotted => step(1.0, 2.0),
        U::Dashed => step(
            (f32::from(cell.size.width) / decor.cells as f32 * 0.6).max(2.0),
            f32::from(cell.size.width) / decor.cells as f32,
        ),
        U::Single | U::Curly => vec![line(bottom, cell.left(), cell.size.width)],
    }
}

/// Beam and underline cursor thickness.
const CURSOR_BAR: f32 = 2.0;
/// Extra selection opacity at the instant of a copy (the flash eases out).
const COPY_FLASH_ALPHA: f32 = 0.3;
/// Rounded selection corners as a share of a terminal row.
const SELECTION_RADIUS: f32 = 0.15;

/// Merge row spans into connected selection shapes, retaining separate shapes
/// when adjacent rows only touch at an edge.
fn selection_outlines(rows: &[(usize, Range<usize>)]) -> Vec<Vec<gpui::Point<usize>>> {
    rows.chunk_by(|(a, x), (b, y)| a + 1 == *b && x.start < y.end && y.start < x.end)
        .map(|shape| {
            let side = |edge: fn(&Range<usize>) -> usize| {
                shape
                    .chunk_by(|(_, a), (_, b)| edge(a) == edge(b))
                    .flat_map(|run| {
                        [
                            point(edge(&run[0].1), run[0].0),
                            point(edge(&run[0].1), run[run.len() - 1].0 + 1),
                        ]
                    })
                    .collect::<Vec<_>>()
            };
            let mut corners = side(|cols| cols.end);
            corners.extend(side(|cols| cols.start).into_iter().rev());
            corners
        })
        .collect()
}

/// Filled path through grid corners, with a small curve at each corner.
fn rounded_selection_path(
    corners: &[gpui::Point<Pixels>],
    radius: Pixels,
) -> Option<gpui::Path<Pixels>> {
    let mut path = PathBuilder::fill();
    for (ix, &at) in corners.iter().enumerate() {
        let toward = |to: gpui::Point<Pixels>| {
            let t = (radius / ((to.x - at.x).abs() + (to.y - at.y).abs())).min(0.5);
            point(at.x + (to.x - at.x) * t, at.y + (to.y - at.y) * t)
        };
        let enter = toward(corners[(ix + corners.len() - 1) % corners.len()]);
        if ix == 0 {
            path.move_to(enter);
        } else {
            path.line_to(enter);
        }
        path.curve_to(toward(corners[(ix + 1) % corners.len()]), at);
    }
    path.close();
    path.build().ok()
}

#[derive(Default)]
struct CursorTrailState {
    at: Option<(Bounds<Pixels>, Instant)>,
    from: Option<Bounds<Pixels>>,
}

impl CursorTrailState {
    const DURATION: f32 = 0.2;
    const TRAIL_SIZE: f32 = 0.8;
    const MIN_DISTANCE: f32 = 1.5;
    const THICKNESS_X: f32 = 0.9;
    const CORNERS: [(f32, f32); 4] = [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)];

    fn track(
        &mut self,
        rect: Bounds<Pixels>,
        now: Instant,
    ) -> Option<(Bounds<Pixels>, Bounds<Pixels>, f32)> {
        if self.at.is_none_or(|(at, _)| at != rect) {
            self.from = self.at.map(|(at, _)| at);
            self.at = Some((rect, now));
        }
        Some((
            self.from?,
            self.at?.0,
            now.duration_since(self.at?.1).as_secs_f32(),
        ))
    }

    fn corners(rect: Bounds<Pixels>) -> [gpui::Point<f32>; 4] {
        let c = rect.center();
        let (hw, hh) = (
            f32::from(rect.size.width) * 0.5 * Self::THICKNESS_X,
            f32::from(rect.size.height) * 0.5,
        );
        Self::CORNERS.map(|(x, y)| point(f32::from(c.x) + x * hw, f32::from(c.y) + y * hh))
    }

    fn warp(from: Bounds<Pixels>, to: Bounds<Pixels>, t: f32) -> Option<[gpui::Point<Pixels>; 4]> {
        let (to, from) = (Self::corners(to), Self::corners(from));
        let (dx, dy) = (to[2].x - from[2].x, to[2].y - from[2].y);
        let height = to[2].y - to[1].y;
        if t >= Self::DURATION || dx.hypot(dy) <= height * Self::MIN_DISTANCE {
            return None;
        }
        let sign = |v: f32| if v.abs() < 0.5 { 0.0 } else { v.signum() };
        let (sx, sy) = (sign(dx), sign(dy));
        let lead = Self::DURATION * (1.0 - Self::TRAIL_SIZE);
        Some(std::array::from_fn(|i| {
            let (cx, cy) = Self::CORNERS[i];
            let dot = if sx != 0.0 && cx == sx {
                2.0
            } else {
                cx * sx + cy * sy
            };
            let duration = match dot {
                d if d >= 0.5 => lead,
                d if d >= -0.5 => (lead + Self::DURATION) / 2.0,
                _ => Self::DURATION,
            };
            let x = (t / duration).clamp(0.0, 1.0);
            let p = (1.0 - (x - 1.0).powi(2)).sqrt();
            point(
                px(from[i].x + (to[i].x - from[i].x) * p),
                px(from[i].y + (to[i].y - from[i].y) * p),
            )
        }))
    }
}

type TrailShape = (Vec<gpui::Point<f32>>, gpui::Background);

fn trail_shapes(
    effect: crate::settings::TerminalCursorEffect,
    from: Bounds<Pixels>,
    to: Bounds<Pixels>,
    t: f32,
    color: Hsla,
    ansi: &[Hsla; 16],
) -> Option<Vec<TrailShape>> {
    use crate::settings::TerminalCursorEffect::{
        Blaze, BlazeNoTrail, BlazeTapered, Comet, Frozen, Gradient, Rainbow, Sweep, Warp,
    };
    type P = gpui::Point<f32>;
    let quad = |r: Bounds<Pixels>| {
        let (o, s) = (r.origin.map(f32::from), r.size.map(f32::from));
        [
            o,
            point(o.x + s.width, o.y),
            point(o.x + s.width, o.y + s.height),
            point(o.x, o.y + s.height),
        ]
    };
    let lerp = |a: P, b: P, k: f32| point(a.x + (b.x - a.x) * k, a.y + (b.y - a.y) * k);
    let grow = |q: [P; 4], d: f32| {
        std::array::from_fn::<P, 4, _>(|i| {
            let (x, y) = CursorTrailState::CORNERS[i];
            point(q[i].x + x * d, q[i].y + y * d)
        })
    };
    let cubic = |x: f32| 1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3);
    let (a, b) = (quad(from), quad(to));
    let (ca, cb) = (lerp(a[0], a[2], 0.5), lerp(b[0], b[2], 0.5));
    let travel = (cb.x - ca.x).hypot(cb.y - ca.y);
    let (w, h) = (f32::from(to.size.width), f32::from(to.size.height));
    let back = |k: f32| b.map(|p| point(p.x + (ca.x - cb.x) * k, p.y + (ca.y - cb.y) * k));
    let angle = (cb.x - ca.x).atan2(ca.y - cb.y).to_degrees();
    Some(match effect {
        Warp => vec![(
            CursorTrailState::warp(from, to, t)?
                .map(|p| p.map(f32::from))
                .to_vec(),
            color.into(),
        )],
        Sweep if t < 0.2 && travel > h * 1.5 => vec![(
            hull(b.into_iter().chain(back(0.5 * (1.0 - cubic(t / 0.2))))),
            color.into(),
        )],
        Blaze | BlazeTapered | BlazeNoTrail | Frozen if t < 0.3 => {
            let e = (1.0 - t / 0.3).powi(3);
            let (core, glow) = if effect == Frozen {
                (ansi[6], ansi[4])
            } else {
                (color, color)
            };
            let tail: Vec<P> = match effect {
                BlazeNoTrail => Vec::new(),
                BlazeTapered => vec![lerp(cb, ca, e)],
                _ => back(e).to_vec(),
            };
            const HALO: f32 = 3.0;
            let halo = grow(b, HALO)
                .into_iter()
                .chain(tail.iter().flat_map(|&p| grow([p; 4], HALO)));
            let mut shapes = vec![(hull(halo), glow.opacity(0.5 * e).into())];
            if effect != BlazeNoTrail {
                shapes.push((hull(b.into_iter().chain(tail)), core.into()));
            }
            shapes
        }
        Comet if t < 0.32 && travel >= w * 1.5 => {
            let age = t / 0.32;
            let tail = lerp(ca, cb, cubic(age));
            let r = h * 0.42;
            let tint = Hsla {
                l: color.l + (1.0 - color.l) * 0.25,
                ..color
            };
            let ramp = |alpha: f32| {
                gpui::linear_gradient(
                    angle,
                    gpui::linear_color_stop(tint.opacity(0.0), 0.0),
                    gpui::linear_color_stop(tint.opacity(alpha), 1.0),
                )
            };
            let fade = 1.0 - age;
            vec![
                (capsule(tail, cb, r * 0.45, r * 2.2), ramp(0.3 * fade)),
                (capsule(tail, cb, r * 0.2, r), ramp(fade)),
            ]
        }
        Rainbow | Gradient if t < if effect == Rainbow { 0.5 } else { 0.25 } => {
            let duration = if effect == Rainbow { 0.5 } else { 0.25 };
            let sign = |v: f32| if v.abs() < 0.5 { 0.0 } else { v.signum() };
            let (sx, sy) = (sign(cb.x - ca.x), sign(cb.y - ca.y));
            let dots = CursorTrailState::CORNERS.map(|(x, y)| x * sx + y * sy);
            let least = dots.iter().copied().fold(f32::MAX, f32::min);
            let (slow, fast) = (cubic(t / duration), cubic(2.0 * t / duration));
            let lagging = (0..4).map(|i| {
                lerp(
                    a[i],
                    b[i],
                    if least < 0.0 && dots[i] == least {
                        slow
                    } else {
                        fast
                    },
                )
            });
            let fill = if effect == Rainbow {
                const RING: [usize; 6] = [1, 3, 2, 6, 4, 5];
                let at = (((cb.x + cb.y) / 600.0 + t * 2.0).fract() * 6.0) as usize;
                gpui::linear_gradient(
                    angle,
                    gpui::linear_color_stop(ansi[RING[at % 6]].opacity(0.0), 0.0),
                    gpui::linear_color_stop(ansi[RING[(at + 2) % 6]], 1.0),
                )
            } else {
                gpui::linear_gradient(
                    180.0,
                    gpui::linear_color_stop(ansi[3].opacity(0.8), 0.0),
                    gpui::linear_color_stop(ansi[4].opacity(0.8), 1.0),
                )
            }
            .color_space(gpui::ColorSpace::Oklab);
            vec![(hull(b.into_iter().chain(lagging)), fill)]
        }
        _ => return None,
    })
}

fn hull(points: impl IntoIterator<Item = gpui::Point<f32>>) -> Vec<gpui::Point<f32>> {
    let mut points: Vec<_> = points.into_iter().collect();
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    if points.len() < 3 {
        return points;
    }
    let turn = |o: gpui::Point<f32>, a: gpui::Point<f32>, b: gpui::Point<f32>| {
        (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
    };
    let mut hull = Vec::with_capacity(points.len() * 2);
    for &p in &points {
        while hull.len() >= 2 && turn(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    let lower = hull.len() + 1;
    for &p in points[..points.len() - 1].iter().rev() {
        while hull.len() >= lower && turn(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop();
    hull
}

fn capsule(
    tail: gpui::Point<f32>,
    head: gpui::Point<f32>,
    r0: f32,
    r1: f32,
) -> Vec<gpui::Point<f32>> {
    let along = (head.y - tail.y).atan2(head.x - tail.x);
    let arc = |c: gpui::Point<f32>, r: f32, start: f32| {
        (0..=8).map(move |i| {
            let a = start + std::f32::consts::PI * i as f32 / 8.0;
            point(c.x + r * a.cos(), c.y + r * a.sin())
        })
    };
    let half = std::f32::consts::FRAC_PI_2;
    arc(head, r1, along - half)
        .chain(arc(tail, r0, along + half))
        .collect()
}

fn trail_path(points: &[gpui::Point<f32>]) -> Option<gpui::Path<Pixels>> {
    let mut builder = PathBuilder::fill();
    let mut points = points.iter();
    let first = points.next()?;
    builder.move_to(point(px(first.x), px(first.y)));
    for p in points {
        builder.line_to(point(px(p.x), px(p.y)));
    }
    builder.close();
    builder.build().ok()
}

#[derive(Clone, Copy)]
struct CustomGlyph {
    col: usize,
    ch: char,
    cells: usize,
    color: Hsla,
}

#[derive(Default)]
struct CustomGlyphCache(HashMap<(char, u32, u32, u32, [u8; 4]), (Arc<RenderImage>, u32, u32)>);

impl gpui::Global for CustomGlyphCache {}

/// Uploaded Kitty graphics sprites, keyed by `(image id, pixels identity)`:
/// a retransmitted image gets new pixels (a new identity), so it uploads
/// once per generation.
#[derive(Default)]
struct ImageSpriteCache(HashMap<(u32, usize), Arc<RenderImage>>);

impl gpui::Global for ImageSpriteCache {}

/// The placements as paintable sprites, split by layer: negative z under the
/// glyphs, the rest over them. A placement's rect is the cells its corner
/// sits at stretched over the cells it occupies (the protocol's default
/// scaling); the grid's content mask clips what runs past the edges.
fn image_sprites(
    placements: &[ImagePlacement],
    origin: gpui::Point<Pixels>,
    cell_w: Pixels,
    line_h: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> (Vec<(Bounds<Pixels>, Arc<RenderImage>)>, Vec<(Bounds<Pixels>, Arc<RenderImage>)>) {
    let mut below = Vec::new();
    let mut above = Vec::new();
    if placements.is_empty() {
        return (below, above);
    }
    let cache = cx.default_global::<ImageSpriteCache>();
    for placement in placements {
        let key = (
            placement.image_id,
            Arc::as_ptr(&placement.pixels) as usize,
        );
        let image = match cache.0.get(&key) {
            Some(image) => image.clone(),
            None => {
                // gpui's `RenderImage` takes BGRA; the storage holds RGBA.
                let bgra: Vec<u8> = placement
                    .pixels
                    .rgba
                    .chunks_exact(4)
                    .flat_map(|px| [px[2], px[1], px[0], px[3]])
                    .collect();
                let Some(buffer) = image::RgbaImage::from_raw(
                    placement.pixels.width,
                    placement.pixels.height,
                    bgra,
                ) else {
                    continue;
                };
                let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
                if cache.0.len() >= 64 {
                    for (_, old) in cache.0.drain() {
                        let _ = window.drop_image(old);
                    }
                }
                cache.0.insert(key, image.clone());
                image
            }
        };
        let rect = Bounds::new(
            point(
                origin.x + cell_w * placement.col as f32,
                origin.y + line_h * placement.row as f32,
            ),
            size(
                cell_w * placement.cols as f32,
                line_h * placement.rows as f32,
            ),
        );
        if placement.z < 0 {
            below.push((rect, image));
        } else {
            above.push((rect, image));
        }
    }
    (below, above)
}

impl gpui::IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TerminalPrepaint;

    fn id(&self) -> Option<gpui::ElementId> {
        Some(("terminal-grid", self.panel.entity_id()).into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let theme = Theme::of(cx).clone();
        // Keep ligatures enabled, as in Shepherd. `shape_row` pins shaped
        // glyphs back to their source cells, so substitutions such as `->`
        // remain visually connected without shifting later columns.
        let mut mono = font(theme.font_terminal.clone());
        mono.fallbacks = Some(gpui::FontFallbacks::from_fonts(
            NERD_FONT_FALLBACKS
                .iter()
                .map(|family| (*family).to_string())
                .collect(),
        ));
        // Font probe: measure the actual advance of the resolved mono font so
        // cols/rows track real glyph metrics, not a guessed aspect ratio. The
        // row height is the user's line-height multiple (Shepherd parity),
        // falling back to the historic 18/13 rhythm before settings load.
        let settings = crate::settings::current(cx);
        let line_h = px(theme.terminal_font_size * settings.terminal_line_height);
        let font_size = px(theme.terminal_font_size);
        let font_id = window.text_system().resolve_font(&mono);
        let cell_w = window
            .text_system()
            .em_advance(font_id, font_size)
            .unwrap_or(px(theme.terminal_font_size * 0.6));

        let inner_w = f32::from(bounds.size.width) - 2.0 * TERM_PADDING;
        let inner_h = f32::from(bounds.size.height) - 2.0 * TERM_PADDING;
        let cols = ((inner_w / f32::from(cell_w)).floor() as i64).clamp(2, 500) as u16;
        let rows = ((inner_h / f32::from(line_h)).floor() as i64).clamp(1, 500) as u16;

        // Report the measured grid, then snapshot for painting. Safe: the
        // panel entity is not borrowed during element prepaint. The held
        // output feeds HERE, as one advance, so this frame paints the state
        // after every byte that arrived before it — a program's in-between
        // cursor positions (vim's redraws walk the cursor all over the
        // screen) never show.
        let origin = point(
            bounds.left() + px(TERM_PADDING),
            bounds.top() + px(TERM_PADDING),
        );
        let snapshot = self.panel.update(cx, |panel, cx| {
            panel.on_grid_metrics(
                super::panel::GridGeometry {
                    bounds,
                    origin,
                    cell_w: f32::from(cell_w),
                    line_h: f32::from(line_h),
                    cols,
                    rows,
                },
                cx,
            );
            panel.flush_active_output(cx);
            panel.active_grid_snapshot(cx)
        });
        let Some(snapshot) = snapshot else {
            return TerminalPrepaint {
                bg_quads: Vec::new(),
                selection: Vec::new(),
                marks: Vec::new(),
                images_below: Vec::new(),
                images_above: Vec::new(),
                lines: Vec::new(),
                glyphs: Vec::new(),
                decor: Vec::new(),
                sprite_stroke: 1.0,
                trail: Vec::new(),
                cell_w,
                line_h,
                cursor: None,
                cursor_under_text: false,
                marked: None,
            };
        };

        let mut bg_quads = Vec::new();
        let mut selection_spans = Vec::new();
        let mut lines = Vec::with_capacity(snapshot.lines.len());
        let mut glyphs = Vec::new();
        let mut decor = Vec::new();
        let row_count = snapshot.lines.len();
        let mut selection = theme.selection;
        selection.a = (selection.a + COPY_FLASH_ALPHA * snapshot.copy_flash).min(0.9);
        let focused_cursor = self.focused && snapshot.cursor_on;
        let cursor_under_text = focused_cursor && snapshot.cursor_shape == CursorShape::Block;
        let cursor_text = cursor_under_text.then_some(theme.on_accent);
        for (row_ix, row) in snapshot.lines.iter().enumerate() {
            let y = origin.y + line_h * row_ix as f32;
            if let (Some(start), Some(end)) = (
                row.iter().position(|cell| cell.selected),
                row.iter().rposition(|cell| cell.selected),
            ) {
                selection_spans.push((row_ix, start..end + 1));
            }
            // Merge consecutive non-default background cells into quads. A
            // run touching the grid's edge carries its color into the padding
            // beside it, so full-bleed program backgrounds (editors, status
            // lines) reach the panel edge instead of stopping a few px short.
            let mut run_start: Option<(usize, Hsla)> = None;
            let edge_quad = |start: usize, end: usize, color: Hsla| {
                let x0 = if start == 0 {
                    bounds.left()
                } else {
                    origin.x + cell_w * start as f32
                };
                let x1 = if end == row.len() {
                    bounds.right()
                } else {
                    origin.x + cell_w * end as f32
                };
                let y0 = if row_ix == 0 { bounds.top() } else { y };
                let y1 = if row_ix + 1 == row_count {
                    bounds.bottom()
                } else {
                    y + line_h
                };
                fill(Bounds::from_corners(point(x0, y0), point(x1, y1)), color)
            };
            for (col, color) in row
                .iter()
                .map(|cell| cell.display_colors().1)
                .chain(std::iter::once(CellColor::Background))
                .enumerate()
            {
                let paint = match color {
                    CellColor::Background => None,
                    other => Some(resolve_color(other, &theme)),
                };
                match (&run_start, paint) {
                    (None, Some(color)) => run_start = Some((col, color)),
                    (Some((start, current)), next) if next != Some(*current) => {
                        bg_quads.push(edge_quad(*start, col, *current));
                        run_start = next.map(|color| (col, color));
                    }
                    _ => {}
                }
            }
            let cursor_col = focused_cursor
                .then(|| {
                    snapshot
                        .cursor
                        .filter(|cursor| cursor.row == row_ix)
                        .map(|cursor| cursor.col)
                })
                .flatten();
            let shaped = shape_row(
                row,
                y,
                &theme,
                &mono,
                font_size,
                cell_w,
                cursor_col.map(|col| (col, cursor_text)),
                window,
                &mut glyphs,
                &mut decor,
            );
            lines.push((y, shaped));
        }

        let selection_radius = line_h * SELECTION_RADIUS;
        let selection_paths = selection_outlines(&selection_spans)
            .iter()
            .filter_map(|corners| {
                let at = |c: &gpui::Point<usize>| {
                    point(
                        origin.x + cell_w * c.x as f32,
                        origin.y + line_h * c.y as f32,
                    )
                };
                rounded_selection_path(
                    &corners.iter().map(at).collect::<Vec<_>>(),
                    selection_radius,
                )
                .map(|path| (path, selection))
            })
            .collect();

        // The open search's matches, every hit washed and the current one
        // stronger, under glyphs like the selection.
        let marks = snapshot
            .search_marks
            .iter()
            .filter(|&&(row, _, _)| row < row_count)
            .map(|&(row, ref cells, current)| {
                fill(
                    Bounds::new(
                        point(
                            origin.x + cell_w * cells.start as f32,
                            origin.y + line_h * row as f32,
                        ),
                        size(cell_w * (cells.end - cells.start) as f32, line_h),
                    ),
                    theme.warning.opacity(if current { 0.5 } else { 0.25 }),
                )
                .corner_radii(px(2.0))
            })
            .collect();

        // Kitty graphics placements, split by layer around the glyphs.
        let (images_below, images_above) =
            image_sprites(&snapshot.images, origin, cell_w, line_h, window, cx);

        let cursor_cell = snapshot.cursor.filter(|_| snapshot.cursor_on).map(|c| {
            let cell = Bounds::new(
                point(
                    origin.x + cell_w * c.col as f32,
                    origin.y + line_h * c.row as f32,
                ),
                size(cell_w, line_h),
            );
            match (self.focused, snapshot.cursor_shape) {
                (true, CursorShape::Beam) => Bounds::new(cell.origin, size(px(CURSOR_BAR), line_h)),
                (true, CursorShape::Underline) => Bounds::new(
                    point(cell.left(), cell.bottom() - px(CURSOR_BAR)),
                    size(cell_w, px(CURSOR_BAR)),
                ),
                _ => cell,
            }
        });
        let mut trail = Vec::new();
        if self.focused
            && !cx.reduce_motion()
            && settings.terminal_cursor_effect != crate::settings::TerminalCursorEffect::None
        {
            if let (Some(id), Some(rect)) = (id, cursor_cell) {
                let effect = settings.terminal_cursor_effect;
                let shapes =
                    window.with_element_state(id, |previous: Option<CursorTrailState>, window| {
                        let mut state = previous.unwrap_or_default();
                        let shapes = state.track(rect, Instant::now()).and_then(|(from, to, t)| {
                            trail_shapes(effect, from, to, t, theme.accent, &theme.terminal.ansi)
                        });
                        if shapes.is_some() {
                            window.request_animation_frame();
                        }
                        (shapes, state)
                    });
                trail.extend(
                    shapes
                        .into_iter()
                        .flatten()
                        .filter_map(|(points, fill)| trail_path(&points).map(|path| (path, fill))),
                );
            }
        }
        let cursor = cursor_cell.map(|cell| {
            let bar = px(CURSOR_BAR);
            match (self.focused, snapshot.cursor_shape) {
                (false, _) | (true, CursorShape::HollowBlock) => {
                    outline(cell, theme.accent, gpui::BorderStyle::Solid)
                }
                (true, CursorShape::Block) => fill(cell, theme.accent),
                (true, CursorShape::Beam) => fill(
                    Bounds::new(cell.origin, size(bar, line_h)),
                    theme.accent.opacity(1.0),
                ),
                (true, CursorShape::Underline) => fill(
                    Bounds::new(point(cell.left(), cell.bottom() - bar), size(cell_w, bar)),
                    theme.accent.opacity(1.0),
                ),
            }
            .corner_radii(px(1.0))
        });

        // Composition text sits over the cursor cell on an opaque backdrop,
        // underlined like every platform marks unconfirmed input.
        let marked = snapshot
            .marked_text
            .as_deref()
            .filter(|text| !text.is_empty())
            .zip(snapshot.cursor)
            .map(|(text, c)| {
                let at = point(
                    origin.x + cell_w * c.col as f32,
                    origin.y + line_h * c.row as f32,
                );
                let shaped = window.text_system().shape_line(
                    SharedString::from(text.to_string()),
                    font_size,
                    &[TextRun {
                        len: text.len(),
                        font: mono.clone(),
                        color: theme.terminal.foreground,
                        background_color: None,
                        underline: Some(gpui::UnderlineStyle {
                            color: Some(theme.terminal.foreground),
                            thickness: px(1.0),
                            wavy: false,
                        }),
                        strikethrough: None,
                    }],
                    None,
                );
                let backdrop = fill(
                    Bounds::new(at, size(shaped.width.max(cell_w), line_h)),
                    theme.terminal.background.opacity(1.0),
                );
                (backdrop, at, shaped)
            });

        TerminalPrepaint {
            bg_quads,
            selection: selection_paths,
            marks,
            images_below,
            images_above,
            lines,
            glyphs,
            decor,
            sprite_stroke: theme.terminal_font_size * 0.05,
            trail,
            cell_w,
            line_h,
            cursor,
            cursor_under_text,
            marked,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_h = prepaint.line_h;
        let origin = point(
            bounds.left() + px(TERM_PADDING),
            bounds.top() + px(TERM_PADDING),
        );
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for quad in prepaint.bg_quads.drain(..) {
                window.paint_quad(quad);
            }
            for (rect, image) in prepaint.images_below.drain(..) {
                let _ = window.paint_image(rect, Default::default(), image, 0, false);
            }
            for (path, color) in prepaint.selection.drain(..) {
                window.paint_path(path, color);
            }
            for quad in prepaint.marks.drain(..) {
                window.paint_quad(quad);
            }
            for (path, color) in prepaint.trail.drain(..) {
                window.paint_path(path, color);
            }
            if prepaint.cursor_under_text
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
            let cell_w = prepaint.cell_w;
            for (y, segments) in &prepaint.lines {
                let y = *y;
                for (col, line) in segments {
                    let _ = line.paint(
                        point(origin.x + cell_w * *col as f32, y),
                        line_h,
                        gpui::TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            }
            for (y, glyph) in &prepaint.glyphs {
                let left = origin.x + cell_w * glyph.col as f32;
                let bounds =
                    Bounds::new(point(left, *y), size(cell_w * glyph.cells as f32, line_h));
                paint_custom_glyph(glyph, bounds, prepaint.sprite_stroke, window, cx);
            }
            for (y, decor) in &prepaint.decor {
                let left = origin.x + cell_w * decor.col as f32;
                let cell =
                    Bounds::new(point(left, *y), size(cell_w * decor.cells as f32, line_h));
                for quad in decor_quads(decor, cell) {
                    window.paint_quad(fill(quad, decor.color));
                }
            }
            for (rect, image) in prepaint.images_above.drain(..) {
                let _ = window.paint_image(rect, Default::default(), image, 0, false);
            }
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
            if let Some((backdrop, at, text)) = prepaint.marked.take() {
                window.paint_quad(backdrop);
                let _ = text.paint(at, line_h, gpui::TextAlign::Left, None, window, cx);
            }
        });
        if self.panel.read(cx).copy_flash_running() {
            window.request_animation_frame();
        }
        // Typed text (and input-method composition) enters here.
        if self.focused {
            let focus = self.panel.read(cx).focus_handle();
            window.handle_input(
                &focus,
                gpui::ElementInputHandler::new(bounds, self.panel.clone()),
                cx,
            );
        }
    }
}

/// Paint a Ghostty sprite at the exact device-pixel dimensions of its grid
/// cell. Cache the tinted result so ordinary terminal redraws only reuse the
/// existing GPUI image.
fn paint_custom_glyph(
    glyph: &CustomGlyph,
    bounds: Bounds<Pixels>,
    stroke: f32,
    window: &mut Window,
    cx: &mut App,
) {
    let scale = window.scale_factor();
    let device = |v: Pixels| (f32::from(v) * scale).round();
    let (left, top) = (device(bounds.left()), device(bounds.top()));
    let w = (device(bounds.right()) - left).max(1.0) as u32;
    let h = (device(bounds.bottom()) - top).max(1.0) as u32;
    let thickness = (stroke * scale).ceil().max(1.0) as u32;
    let rgba = gpui::Rgba::from(glyph.color);
    let color = [rgba.r, rgba.g, rgba.b, rgba.a].map(|c| (c * 255.0).round() as u8);
    let key = (glyph.ch, w, h, thickness, color);

    let cache = cx.default_global::<CustomGlyphCache>();
    let (image, pad_x, pad_y) = match cache.0.get(&key) {
        Some(hit) => hit.clone(),
        None => {
            let metrics = sprite::Metrics {
                cell_width: w / glyph.cells as u32,
                cell_height: h,
                box_thickness: thickness,
            };
            let Some(drawn) = sprite::render(glyph.ch, w, h, &metrics) else {
                return;
            };
            let [r, g, b, a] = color;
            let bgra: Vec<u8> = drawn
                .alpha
                .iter()
                .flat_map(|&coverage| [b, g, r, (coverage as u16 * a as u16 / 255) as u8])
                .collect();
            let Some(buffer) = image::RgbaImage::from_raw(drawn.width, drawn.height, bgra) else {
                return;
            };
            let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
            if cache.0.len() >= 4096 {
                for (_, (old, _, _)) in cache.0.drain() {
                    let _ = window.drop_image(old);
                }
            }
            let hit = (image, drawn.padding_x, drawn.padding_y);
            cache.0.insert(key, hit.clone());
            hit
        }
    };
    let image_size = image.size(0);
    let logical = |v: f32| px(v / scale);
    let sprite_bounds = Bounds::new(
        point(logical(left - pad_x as f32), logical(top - pad_y as f32)),
        size(
            logical(image_size.width.0 as f32),
            logical(image_size.height.0 as f32),
        ),
    );
    let _ = window.paint_image(sprite_bounds, Default::default(), image, 0, false);
}

/// Shape a row with ligatures enabled, then snap shaped glyph clusters back to
/// their source grid columns. This keeps fallback glyph advances and ligature
/// substitutions from moving later terminal cells.
#[allow(clippy::too_many_arguments)]
fn shape_row(
    row: &[CellSnapshot],
    row_y: Pixels,
    theme: &Theme,
    mono: &gpui::Font,
    font_size: Pixels,
    cell_w: Pixels,
    cursor: Option<(usize, Option<Hsla>)>,
    window: &Window,
    glyphs: &mut Vec<(Pixels, CustomGlyph)>,
    decor: &mut Vec<(Pixels, Decor)>,
) -> Vec<(usize, ShapedLine)> {
    let mut segments = Vec::new();
    let mut segment = ShapedSegment::default();
    let mut rtl_run = false;
    for (col, cell) in row.iter().enumerate() {
        if cell.wide_spacer {
            continue;
        }
        let ch = if cell.hidden { ' ' } else { cell.ch };
        let (fg, bg) = cell.display_colors();
        let mut color = resolve_color(fg, theme);
        // The theme's own default pair is already tuned; anything the
        // program colored gets the contrast floor.
        if !(fg == CellColor::Foreground && bg == CellColor::Background) {
            color = ensure_contrast(color, resolve_color(bg, theme), MIN_CONTRAST);
        }
        if cell.dim {
            color.a *= 0.6;
        }
        let at_cursor = cursor.is_some_and(|(c, _)| c == col);
        if let Some((_, Some(cursor_text))) = cursor.filter(|_| at_cursor) {
            color = cursor_text;
        }
        if !ch.is_ascii() && sprite::has_char(ch) {
            let rtl_fit = rtl_run.then(|| cell_w * (col - segment.col) as f32);
            segment.flush(rtl_fit, &mut segments, font_size, cell_w, window);
            rtl_run = false;
            glyphs.push((
                row_y,
                CustomGlyph {
                    col,
                    ch,
                    cells: if cell.wide { 2 } else { 1 },
                    color,
                },
            ));
            continue;
        }
        let rtl = is_rtl(ch) && !cell.wide;
        let joins_rtl = rtl_run && (rtl || ch == ' ');
        if !joins_rtl && (at_cursor || rtl || rtl_run) {
            let rtl_fit = rtl_run.then(|| cell_w * (col - segment.col) as f32);
            segment.flush(rtl_fit, &mut segments, font_size, cell_w, window);
        }
        if segment.text.is_empty() {
            segment.col = col;
        }
        let font_weight = if cell.bold {
            gpui::FontWeight::BOLD
        } else {
            gpui::FontWeight::NORMAL
        };
        let font_style = if cell.italic {
            gpui::FontStyle::Italic
        } else {
            gpui::FontStyle::Normal
        };
        // gpui's text runs draw a straight or a wavy line only: double, dotted,
        // and dashed underlines are painted over the row as quads.
        use crate::terminal::emulator::UnderlineStyle as U;
        let drawn = matches!(cell.underline_style, U::Single | U::Curly);
        if cell.underline && !cell.predicted && !drawn {
            decor.push((
                row_y,
                Decor {
                    col,
                    cells: if cell.wide { 2 } else { 1 },
                    color,
                    style: cell.underline_style,
                },
            ));
        }
        // A predicted (not yet echoed) character carries a faint underline:
        // it reads as typed, while still owning that it is provisional.
        let underline = if cell.predicted {
            Some(gpui::UnderlineStyle {
                color: Some(color.opacity(0.45)),
                thickness: px(1.0),
                wavy: false,
            })
        } else {
            (cell.underline && drawn).then_some(gpui::UnderlineStyle {
                color: Some(color),
                thickness: px(1.0),
                wavy: cell.underline_style == U::Curly,
            })
        };
        let strikethrough = cell.strikethrough.then_some(gpui::StrikethroughStyle {
            color: Some(color),
            thickness: px(1.0),
        });
        let mut encoded = [0; 4];
        let text = cell.cluster.as_deref().filter(|_| !cell.hidden).unwrap_or_else(|| ch.encode_utf8(&mut encoded));
        let len = text.len();
        segment.text.push_str(text);
        segment.cols.extend(std::iter::repeat_n(col, len));
        match segment.runs.last_mut() {
            Some(last)
                if last.color == color
                    && last.font.weight == font_weight
                    && last.font.style == font_style
                    && last.underline == underline
                    && last.strikethrough == strikethrough =>
            {
                last.len += len;
            }
            _ => segment.runs.push(TextRun {
                len,
                font: gpui::Font {
                    weight: font_weight,
                    style: font_style,
                    ..mono.clone()
                },
                color,
                background_color: None,
                underline,
                strikethrough,
            }),
        }
        rtl_run = rtl || joins_rtl;
        if at_cursor && !rtl_run {
            segment.flush(None, &mut segments, font_size, cell_w, window);
        }
    }
    let rtl_fit = rtl_run.then(|| cell_w * (row.len().saturating_sub(segment.col)) as f32);
    segment.flush(rtl_fit, &mut segments, font_size, cell_w, window);
    segments
}

#[derive(Default)]
struct ShapedSegment {
    text: String,
    /// Source terminal column for every UTF-8 byte in `text`.
    cols: Vec<usize>,
    runs: Vec<TextRun>,
    col: usize,
}

impl ShapedSegment {
    fn flush(
        &mut self,
        rtl_fit: Option<Pixels>,
        out: &mut Vec<(usize, ShapedLine)>,
        font_size: Pixels,
        cell_w: Pixels,
        window: &Window,
    ) {
        if self.text.is_empty() {
            return;
        }
        let text = SharedString::from(std::mem::take(&mut self.text));
        let system = window.text_system();
        let mut shaped = system.shape_line(text.clone(), font_size, &self.runs, None);
        match rtl_fit {
            Some(fit) if shaped.width > fit => {
                shaped =
                    system.shape_line(text, font_size * (fit / shaped.width), &self.runs, None);
            }
            Some(_) => {}
            None => snap_to_cells(&mut shaped, &self.cols, self.col, cell_w),
        }
        out.push((self.col, shaped));
        self.runs.clear();
        self.cols.clear();
    }
}

/// Move shaped glyph clusters to their source terminal columns while keeping
/// the offsets between glyphs in the same cluster.
fn snap_to_cells(line: &mut ShapedLine, cols: &[usize], segment_col: usize, cell_w: Pixels) {
    let mut runs = line.runs.clone();
    let mut cluster = (usize::MAX, px(0.0));
    for glyph in runs.iter_mut().flat_map(|run| run.glyphs.iter_mut()) {
        let col = cols.get(glyph.index).copied().unwrap_or(segment_col);
        if col != cluster.0 {
            cluster = (col, glyph.position.x);
        }
        glyph.position.x = cell_w * (col - segment_col) as f32 + (glyph.position.x - cluster.1);
    }
    **line = Arc::new(gpui::LineLayout {
        font_size: line.font_size,
        width: line.width,
        ascent: line.ascent,
        descent: line.descent,
        runs,
        len: line.len,
    });
}

fn is_rtl(ch: char) -> bool {
    matches!(ch, '\u{0590}'..='\u{08FF}' | '\u{FB1D}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFC}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods() -> Modifiers {
        Modifiers::default()
    }

    #[test]
    fn dropped_paths_are_escaped_and_joined() {
        assert_eq!(
            dropped_paths(&["/tmp/a.png", "/Users/me/My Shot (1).png"]),
            r"/tmp/a.png /Users/me/My\ Shot\ \(1\).png"
        );
    }

    #[test]
    fn enter_with_modifiers_under_kitty_keyboard() {
        let shift = Modifiers {
            shift: true,
            ..mods()
        };
        let ctrl = Modifiers {
            control: true,
            ..mods()
        };
        let opt = Modifiers {
            alt: true,
            ..mods()
        };
        assert_eq!(kitty_enter("enter", &shift, 1), Some(b"\x1b[13;2u".to_vec()));
        assert_eq!(kitty_enter("enter", &ctrl, 1), Some(b"\x1b[13;5u".to_vec()));
        assert_eq!(kitty_enter("enter", &mods(), 1), None);
        assert_eq!(kitty_enter("enter", &mods(), 1 | 8), Some(b"\x1b[13u".to_vec()));
        assert_eq!(kitty_enter("enter", &opt, 1), Some(b"\x1b[13;3u".to_vec()));
        assert_eq!(kitty_enter("enter", &shift, 0), None, "no kitty protocol: legacy \\r");
        assert_eq!(kitty_enter("a", &shift, 1), None);
    }

    #[test]
    fn printables_prefer_key_char() {
        assert_eq!(
            keystroke_bytes("a", Some("a"), &mods(), false),
            Some(b"a".to_vec())
        );
        assert_eq!(
            keystroke_bytes(
                "a",
                Some("A"),
                &Modifiers {
                    shift: true,
                    ..mods()
                },
                false
            ),
            Some(b"A".to_vec())
        );
        // Multi-byte characters pass through as UTF-8.
        assert_eq!(
            keystroke_bytes("e", Some("é"), &mods(), false),
            Some("é".as_bytes().to_vec())
        );
        // Named single-char keys fall back to the key name.
        assert_eq!(
            keystroke_bytes("/", None, &mods(), false),
            Some(b"/".to_vec())
        );
        // Unknown multi-char keys are not ours.
        assert_eq!(keystroke_bytes("capslock", None, &mods(), false), None);
    }

    #[test]
    fn control_keys_and_sequences() {
        assert_eq!(
            keystroke_bytes("enter", None, &mods(), false),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            keystroke_bytes("backspace", None, &mods(), false),
            Some(vec![0x7f])
        );
        assert_eq!(
            keystroke_bytes("tab", None, &mods(), false),
            Some(b"\t".to_vec())
        );
        assert_eq!(
            keystroke_bytes(
                "tab",
                None,
                &Modifiers {
                    shift: true,
                    ..mods()
                },
                false
            ),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            keystroke_bytes("escape", None, &mods(), false),
            Some(vec![0x1b])
        );
        assert_eq!(
            keystroke_bytes("delete", None, &mods(), false),
            Some(b"\x1b[3~".to_vec())
        );
        assert_eq!(
            keystroke_bytes("pageup", None, &mods(), false),
            Some(b"\x1b[5~".to_vec())
        );
        assert_eq!(
            keystroke_bytes("f5", None, &mods(), false),
            Some(b"\x1b[15~".to_vec())
        );
    }

    #[test]
    fn arrows_respect_app_cursor_mode() {
        assert_eq!(
            keystroke_bytes("up", None, &mods(), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            keystroke_bytes("up", None, &mods(), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            keystroke_bytes("home", None, &mods(), false),
            Some(b"\x1b[H".to_vec())
        );
        assert_eq!(
            keystroke_bytes("end", None, &mods(), true),
            Some(b"\x1bOF".to_vec())
        );
    }

    #[test]
    fn ctrl_combos_map_to_control_bytes() {
        let ctrl = Modifiers {
            control: true,
            ..mods()
        };
        assert_eq!(
            keystroke_bytes("c", Some("c"), &ctrl, false),
            Some(vec![0x03])
        );
        assert_eq!(keystroke_bytes("z", None, &ctrl, false), Some(vec![0x1a]));
        assert_eq!(
            keystroke_bytes("space", None, &ctrl, false),
            Some(vec![0x00])
        );
        assert_eq!(keystroke_bytes("[", None, &ctrl, false), Some(vec![0x1b]));
        assert_eq!(keystroke_bytes("_", None, &ctrl, false), Some(vec![0x1f]));
        // Ctrl+1 has no caret encoding — not ours.
        assert_eq!(keystroke_bytes("1", Some("1"), &ctrl, false), None);
    }

    #[test]
    fn alt_prefixes_escape() {
        let alt = Modifiers {
            alt: true,
            ..mods()
        };
        assert_eq!(
            keystroke_bytes("b", Some("b"), &alt, false),
            Some(vec![0x1b, b'b'])
        );
        let alt_ctrl = Modifiers {
            alt: true,
            control: true,
            ..mods()
        };
        assert_eq!(
            keystroke_bytes("c", None, &alt_ctrl, false),
            Some(vec![0x1b, 0x03])
        );
    }

    #[test]
    fn platform_primary_combos_fall_through() {
        let cmd = Modifiers {
            platform: true,
            ..mods()
        };
        assert_eq!(keystroke_bytes("j", Some("j"), &cmd, false), None);
        assert_eq!(keystroke_bytes("enter", None, &cmd, false), None);
        // Cmd+Backspace deletes to the line's start; Cmd+Left/Right jump to
        // its ends — the platform-primary exceptions, as Shepherd sends.
        assert_eq!(
            keystroke_bytes("backspace", None, &cmd, false),
            Some(vec![0x15])
        );
        assert_eq!(keystroke_bytes("left", None, &cmd, false), Some(vec![0x01]));
        assert_eq!(
            keystroke_bytes("right", None, &cmd, false),
            Some(vec![0x05])
        );
        assert_eq!(
            route_key("backspace", None, &cmd, false),
            KeyRoute::Bytes(vec![0x15])
        );
        assert_eq!(
            route_key("left", None, &cmd, false),
            KeyRoute::Bytes(vec![0x01])
        );
    }

    #[test]
    fn printable_keys_go_through_text_input_and_named_keys_are_encoded() {
        let shift = Modifiers {
            shift: true,
            ..mods()
        };
        assert_eq!(route_key("a", Some("a"), &mods(), false), KeyRoute::Text);
        assert_eq!(route_key("a", Some("A"), &shift, false), KeyRoute::Text);
        assert_eq!(
            route_key("enter", Some("\r"), &mods(), false),
            KeyRoute::Bytes(b"\r".to_vec())
        );
        assert_eq!(
            route_key("up", None, &mods(), true),
            KeyRoute::Bytes(b"\x1bOA".to_vec())
        );
        assert_eq!(
            route_key(
                "c",
                Some("c"),
                &Modifiers {
                    control: true,
                    ..mods()
                },
                false
            ),
            KeyRoute::Bytes(vec![0x03])
        );
        assert_eq!(
            route_key(
                "j",
                Some("j"),
                &Modifiers {
                    platform: true,
                    ..mods()
                },
                false
            ),
            KeyRoute::NotOurs
        );
    }

    #[test]
    fn option_types_layout_symbols_and_otherwise_acts_as_meta() {
        let alt = Modifiers {
            alt: true,
            ..mods()
        };
        // US layout: Option-B would type `∫` — Meta-b for word motion.
        assert_eq!(
            route_key("b", Some("∫"), &alt, false),
            KeyRoute::Bytes(vec![0x1b, b'b'])
        );
        assert_eq!(
            route_key("b", Some("ı"), &Modifiers { shift: true, ..alt }, false),
            KeyRoute::Bytes(vec![0x1b, b'B'])
        );
        // German layout: Option-L types `@`, which is what the user meant.
        assert_eq!(route_key("l", Some("@"), &alt, false), KeyRoute::Text);
        // Option+arrows move by words (ESC b/ESC f), as Ghostty sends.
        assert_eq!(
            route_key("left", None, &alt, false),
            KeyRoute::Bytes(b"\x1bb".to_vec())
        );
        assert_eq!(
            route_key("right", None, &alt, false),
            KeyRoute::Bytes(b"\x1bf".to_vec())
        );
        // Other named keys keep their Meta prefix.
        assert_eq!(
            route_key("up", None, &alt, false),
            KeyRoute::Bytes(b"\x1b\x1b[A".to_vec())
        );
    }

    #[test]
    fn mouse_reports_in_both_encodings() {
        let none = mods();
        assert_eq!(
            mouse_report(MOUSE_LEFT, 4, 1, false, &none, true).unwrap(),
            b"\x1b[<0;5;2M"
        );
        assert_eq!(
            mouse_report(MOUSE_LEFT, 4, 1, true, &none, true).unwrap(),
            b"\x1b[<0;5;2m",
            "SGR names the released button"
        );
        let ctrl = Modifiers {
            control: true,
            ..mods()
        };
        assert_eq!(
            mouse_report(MOUSE_WHEEL_UP, 0, 0, false, &ctrl, true).unwrap(),
            b"\x1b[<80;1;1M"
        );
        assert_eq!(
            mouse_report(MOUSE_RIGHT, 0, 0, false, &none, false).unwrap(),
            vec![0x1b, b'[', b'M', 32 + 2, 33, 33]
        );
        assert_eq!(
            mouse_report(MOUSE_RIGHT, 0, 0, true, &none, false).unwrap(),
            vec![0x1b, b'[', b'M', 32 + 3, 33, 33],
            "legacy release is button 3"
        );
        assert_eq!(
            mouse_report(MOUSE_LEFT, 300, 0, false, &none, false),
            None,
            "legacy encoding cannot address column 301"
        );
    }

    #[test]
    fn focus_and_wheel_arrow_encodings() {
        assert_eq!(focus_report(true), b"\x1b[I");
        assert_eq!(focus_report(false), b"\x1b[O");
        assert_eq!(wheel_arrows(2, false), b"\x1b[A\x1b[A");
        assert_eq!(wheel_arrows(-1, true), b"\x1bOB");
    }

    #[test]
    fn low_contrast_program_colors_are_lifted_and_good_ones_kept() {
        let black = gpui::hsla(0.0, 0.0, 0.0, 1.0);
        let navy = rgb8(0, 0, 128);
        assert!(contrast_ratio(navy, black) < MIN_CONTRAST);
        let lifted = ensure_contrast(navy, black, MIN_CONTRAST);
        assert!(contrast_ratio(lifted, black) >= MIN_CONTRAST);
        assert!((lifted.h - navy.h).abs() < 1e-3, "hue is kept");
        let white = gpui::hsla(0.0, 0.0, 1.0, 1.0);
        assert_eq!(ensure_contrast(white, black, MIN_CONTRAST), white);
        // Same color on itself is a deliberate block, not a contrast bug.
        assert_eq!(ensure_contrast(navy, navy, MIN_CONTRAST), navy);
        // Light backgrounds darken the text instead.
        let pale = rgb8(250, 250, 160);
        let darkened = ensure_contrast(pale, white, MIN_CONTRAST);
        assert!(darkened.l < pale.l && contrast_ratio(darkened, white) >= MIN_CONTRAST);
    }

    #[test]
    fn paste_wraps_when_bracketed() {
        assert_eq!(paste_bytes("hi", false), b"hi".to_vec());
        assert_eq!(paste_bytes("hi", true), b"\x1b[200~hi\x1b[201~".to_vec());
        // Close-bracket injection is stripped.
        assert_eq!(
            paste_bytes("a\x1b[201~rm -rf", true),
            b"\x1b[200~arm -rf\x1b[201~".to_vec()
        );
    }

    #[test]
    fn input_queue_keeps_one_write_in_flight_and_order() {
        let mut q = InputQueue::default();
        assert_eq!(q.start(), None, "nothing pending");
        q.push(b"a");
        assert_eq!(q.start(), Some(b"a".to_vec()), "first key goes at once");
        q.push(b"b");
        q.push(b"c");
        assert_eq!(q.start(), None, "keys typed mid-write wait for it");
        q.finish();
        assert_eq!(q.start(), Some(b"bc".to_vec()), "then ride one write");
        q.finish();
        assert_eq!(q.start(), None);
    }

    #[test]
    fn cube_is_appearance_independent() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            // 16 = cube origin (0,0,0); 231 = cube max (255,255,255).
            assert_eq!(extended_indexed_rgb(appearance, 16), (0, 0, 0));
            assert_eq!(extended_indexed_rgb(appearance, 231), (255, 255, 255));
            // 196 = pure red corner: 16 + 36*5.
            assert_eq!(extended_indexed_rgb(appearance, 196), (255, 0, 0));
            // 21 = pure blue corner.
            assert_eq!(extended_indexed_rgb(appearance, 21), (0, 0, 255));
        }
    }

    #[test]
    fn grayscale_ramp_mirrors_in_light() {
        // Dark: 232 → 8 (faintest), 255 → 238 (strongest).
        assert_eq!(extended_indexed_rgb(Appearance::Dark, 232), (8, 8, 8));
        assert_eq!(extended_indexed_rgb(Appearance::Dark, 255), (238, 238, 238));
        // Light reverses it so the faint end stays faint against white.
        assert_eq!(
            extended_indexed_rgb(Appearance::Light, 232),
            (238, 238, 238)
        );
        assert_eq!(extended_indexed_rgb(Appearance::Light, 255), (8, 8, 8));
    }

    #[test]
    fn registered_theme_owns_terminal_background_foreground_selection_and_ansi() {
        let theme = Theme::for_selection(
            Appearance::Dark,
            "dracula",
            commutator_theme::SurfacePreference::ThemeDefault,
        );
        assert_eq!(terminal_panel_bg(&theme), theme.terminal.background);
        assert_eq!(
            resolve_color(CellColor::Foreground, &theme),
            theme.terminal.foreground
        );
        assert_eq!(
            resolve_color(CellColor::Background, &theme),
            theme.terminal.background
        );
        assert_eq!(
            resolve_color(CellColor::Indexed(1), &theme),
            theme.terminal.ansi[1]
        );
        assert!(theme.terminal.selection.a > 0.0 && theme.terminal.selection.a < 0.5);
    }

    #[test]
    fn every_registered_variant_resolves_all_ansi_slots_from_its_theme() {
        for variant in commutator_theme::ThemeRegistry::builtin()
            .families
            .iter()
            .flat_map(|family| &family.variants)
        {
            let appearance = match variant.appearance {
                commutator_theme::Appearance::Dark => Appearance::Dark,
                commutator_theme::Appearance::Light => Appearance::Light,
            };
            let theme = Theme::for_selection(
                appearance,
                &variant.id,
                commutator_theme::SurfacePreference::ThemeDefault,
            );
            for index in 0..16 {
                assert_eq!(
                    resolve_color(CellColor::Indexed(index), &theme),
                    theme.terminal.ansi[index as usize],
                    "{} ANSI {index}",
                    variant.id
                );
            }
        }
    }

    #[test]
    fn timing_constants_match_spec() {
        assert_eq!(RESIZE_DEBOUNCE_MS, 80);
    }

    // ---- pointer → cell ----

    /// 10x20 cells, an 8x4 grid: cols 0..7, rows 0..3.
    fn hit(x: f32, y: f32) -> CellHit {
        cell_at(x, y, 10.0, 20.0, 8, 4)
    }

    #[test]
    fn pointer_maps_to_the_cell_it_is_over() {
        assert_eq!(
            hit(0.0, 0.0),
            CellHit {
                row: 0,
                col: 0,
                side: Side::Left
            }
        );
        assert_eq!(
            hit(25.0, 45.0),
            CellHit {
                row: 2,
                col: 2,
                side: Side::Left
            }
        );
        // Last cell, exactly.
        assert_eq!(
            hit(70.0, 60.0),
            CellHit {
                row: 3,
                col: 7,
                side: Side::Left
            }
        );
    }

    #[test]
    fn side_splits_the_cell_at_its_midpoint() {
        // Cell 2 spans x 20..30, so the midpoint is 25.
        assert_eq!(hit(21.0, 0.0).side, Side::Left);
        assert_eq!(
            hit(25.0, 0.0).side,
            Side::Left,
            "the midpoint itself is left"
        );
        assert_eq!(hit(26.0, 0.0).side, Side::Right);
        // The cell is unaffected by which half.
        assert_eq!(hit(21.0, 0.0).col, 2);
        assert_eq!(hit(29.0, 0.0).col, 2);
    }

    /// Dragging out of the panel must extend to the edge it left through, not
    /// freeze at the last sample inside.
    #[test]
    fn overshoot_clamps_into_the_grid() {
        assert_eq!(hit(9_999.0, 0.0).col, 7);
        assert_eq!(hit(0.0, 9_999.0).row, 3);
        assert_eq!(hit(-50.0, 0.0).col, 0);
        assert_eq!(hit(0.0, -50.0).row, 0);
    }

    /// The part clamping alone does not give you: past the right or bottom
    /// edge the side is forced Right so the last cell is *included*, and above
    /// the top it is forced Left. Dragging below-and-left must still take the
    /// bottom row whole.
    #[test]
    fn overshoot_forces_the_side_to_the_edge() {
        assert_eq!(hit(9_999.0, 10.0).side, Side::Right);
        // x sits in cell 0's left half, but the row overshot — Right wins.
        assert_eq!(hit(1.0, 9_999.0).side, Side::Right);
        assert_eq!(hit(1.0, 9_999.0).col, 0);
        // Above the top, mirrored.
        assert_eq!(hit(75.0, -50.0).side, Side::Left);
    }

    #[test]
    fn degenerate_metrics_do_not_panic() {
        assert_eq!(
            cell_at(5.0, 5.0, 0.0, 20.0, 8, 4),
            CellHit {
                row: 0,
                col: 0,
                side: Side::Left
            }
        );
        assert_eq!(
            cell_at(5.0, 5.0, 10.0, 20.0, 0, 0),
            CellHit {
                row: 0,
                col: 0,
                side: Side::Left
            }
        );
        assert_eq!(cell_at(f32::NAN, f32::INFINITY, 10.0, 20.0, 8, 4).col, 0);
    }

    #[test]
    fn drag_threshold_matches_the_gpui_default() {
        assert_eq!(SELECTION_DRAG_THRESHOLD, 2.0);
    }
}
