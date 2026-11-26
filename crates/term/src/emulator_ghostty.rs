//! Adapted from Zeron's crates/ui/src/terminal/emulator.rs (MIT, see assets/NOTICE).
//! The terminal emulator core: Ghostty's libghostty-vt (the `ghostty-vt` crate, built from Ghostty's source with
//! Zig 0.16) wrapped as a pure state machine.
//!
//! Bytes in ([`Emulator::feed`] — the decoded `SubscribeTerminal` Data frames),
//! grid snapshots out ([`Emulator::line`], [`Emulator::cursor`]). No I/O, no
//! timers, no gpui: the panel owns RPC and scheduling, the view owns paint.
//! That split makes the whole escape-sequence surface unit-testable with
//! scripted byte strings.
//!
//! Selection lives here too ([`Emulator::start_selection`] and friends): the
//! terminal holds the installed selection and keeps it on its text as output
//! moves the grid; word and line bounds are Ghostty's own rules.
//!
//! API notes for the pinned `ghostty-vt 0.7.0` (Ghostty `b40acce`):
//! - Snapshots read through a `RenderState` (the C API's render path); the
//!   handles are `!Send`, which suits an emulator living inside a gpui entity.
//! - Query responses (DSR/DA/…) surface as `Event::PtyWrite` after `vt_write`;
//!   [`Emulator::feed`] returns them so the panel can write them back.

use std::cell::RefCell;
use std::sync::Arc;

use ghostty_vt::kitty::graphics::{
    ImageFormat, PlacementIterator, RustPngDecoder, set_png_decoder,
};
use ghostty_vt::render::{CellIterator, CursorVisualStyle, Dirty, RowIterator};
use ghostty_vt::screen::{CellContentTag, CellWide, GridRef, Screen};
use ghostty_vt::search::Search;
use ghostty_vt::selection::{
    FormatOptions, SelectLineOptions, SelectWordBetweenOptions, Selection,
};
use ghostty_vt::style::{Style, StyleColor, Underline};
use ghostty_vt::terminal::{Mode, Point, PointCoordinate, PointSpace, ScrollViewport};
use ghostty_vt::{Error, Event, RenderState, Terminal, TerminalOptions, key, mouse};

/// A cell in viewport coordinates (row 0 = top). herdr owns scrollback, so the
/// viewport is the whole local grid and a point needs no history offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GridPoint {
    pub row: usize,
    pub col: usize,
}

/// Which half of a cell the pointer is on: a drag starting on a cell's right
/// half leaves that cell out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Selection granularity: a drag, a double-click word, a triple-click row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionType {
    Simple,
    Semantic,
    Lines,
}

/// A cell's paint color, decoupled from the palette: the view resolves these
/// against the theme (default fg/bg, 256-color index, or direct RGB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellColor {
    /// Default foreground.
    Foreground,
    /// Default background.
    Background,
    /// Indexed color: 0-15 ANSI, 16-231 color cube, 232-255 grayscale ramp.
    Indexed(u8),
    /// Direct 24-bit color.
    Rgb(u8, u8, u8),
}

fn map_color(color: StyleColor, default: CellColor) -> CellColor {
    match color {
        StyleColor::None => default,
        StyleColor::Palette(ix) => CellColor::Indexed(ix.0),
        StyleColor::Rgb(c) => CellColor::Rgb(c.r, c.g, c.b),
    }
}

/// The line under an underlined cell, as SGR 4:n names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnderlineStyle {
    #[default]
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// One rendered cell: char + colors + the flags paint cares about.
#[derive(Debug, Clone, PartialEq)]
pub struct CellSnapshot {
    pub ch: char,
    /// The whole grapheme cluster, `ch` first, when the cell holds more than `ch`: a flag, a skin tone, a ZWJ
    /// sequence, a letter and its combining marks.
    pub cluster: Option<std::sync::Arc<str>>,
    pub fg: CellColor,
    pub bg: CellColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    /// How `underline` is drawn (SGR 4:1 to 4:5); curly is for spelling and diagnostics.
    pub underline_style: UnderlineStyle,
    pub strikethrough: bool,
    pub inverse: bool,
    pub hidden: bool,
    /// A double-width char (occupies this cell plus the next spacer cell).
    pub wide: bool,
    /// The spacer half of a wide char — never shaped, only background-painted.
    pub wide_spacer: bool,
    /// Inside the active selection: the view paints a wash over this cell.
    pub selected: bool,
    /// Typed locally and shown before the remote echo confirms it.
    pub predicted: bool,
}

impl CellSnapshot {
    /// Effective paint colors after INVERSE/HIDDEN resolution.
    pub fn display_colors(&self) -> (CellColor, CellColor) {
        let (fg, bg) = if self.inverse {
            (self.bg, self.fg)
        } else {
            (self.fg, self.bg)
        };
        if self.hidden { (bg, bg) } else { (fg, bg) }
    }
}

/// Cursor position in viewport coordinates (row 0 = top of the visible grid).
pub use crate::emulator::CursorSnapshot;

/// How the cursor paints, as the program set it (DECSCUSR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Beam,
    Underline,
    HollowBlock,
}

/// Which pointer events the program asks the panel to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseTracking {
    Click,
    Drag,
    Motion,
}

/// The render-path handles, reused across snapshots. Behind a `RefCell`
/// because updating them mutates, while snapshots are `&self` reads.
struct Render {
    state: RenderState,
    rows: RowIterator,
    cells: CellIterator,
    /// The rows as last read: a row Ghostty did not mark dirty since is taken from here, not read cell by cell again.
    /// Shared with the frames that read them, so a clean row is not copied.
    lines: Vec<Arc<Vec<CellSnapshot>>>,
}

/// The selection being made: its granularity and where it started.
#[derive(Debug, Clone, Copy)]
struct Anchor {
    ty: SelectionType,
    point: GridPoint,
    side: Side,
}

/// A selection in screen points, for [`Emulator::restore_selection`]: the anchor of one being made and the
/// installed ends, top-left first.
#[derive(Debug, Clone, Copy)]
pub struct SelectionState {
    anchor: Option<Anchor>,
    ends: Option<(GridPoint, GridPoint)>,
}

/// The emulator: a pure fold of PTY bytes into a renderable grid.
pub struct Emulator {
    term: Terminal,
    render: RefCell<Render>,
    anchor: Option<Anchor>,
    bell: bool,
    /// Fed a program's own bytes rather than herdr's frames ([`Self::raw`]).
    raw: bool,
    /// The open scrollback search, when Cmd+F is looking for something.
    search: RefCell<Option<Search>>,
    /// Decoded Kitty image pixels, cached by `(image id, generation)`: the
    /// storage's borrow ends at the next `vt_write`, so pixels are copied
    /// once per generation, not once per frame.
    images: RefCell<std::collections::HashMap<(u32, u64), std::sync::Arc<ImagePixels>>>,
}

/// Ghostty's default scrollback cap (bytes, an estimate): far more than the lines either backend hands over.
const SCROLLBACK: usize = 10_000_000;

/// Kitty graphics storage cap: generous, but bounded.
const KITTY_STORAGE: u64 = 256 * 1024 * 1024;
/// How many decoded images stay cached past their last placement.
const IMAGE_CACHE_MAX: usize = 16;

/// Decoded Kitty image pixels, normalized to RGBA8.
pub struct ImagePixels {
    pub width: u32,
    pub height: u32,
    /// RGBA8, row-major.
    pub rgba: Vec<u8>,
}

/// One Kitty graphics placement in the viewport, ready to paint: the cells
/// its top-left corner sits at and occupies, and its pixels.
pub struct ImagePlacement {
    pub image_id: u32,
    /// Negative layers paint under the glyphs, the rest over them.
    pub z: i32,
    /// Viewport cell of the placement's top-left corner; negative rows are
    /// scrolled past the top edge.
    pub col: i32,
    pub row: i32,
    pub cols: u32,
    pub rows: u32,
    pub pixels: std::sync::Arc<ImagePixels>,
}

impl Emulator {
    pub fn new(cols: u16, rows: u16) -> Self {
        // Commutator receives a raw PTY byte stream, so Ghostty keeps local
        // scrollback and reflows wrapped lines on resize.
        Self::raw(cols, rows)
    }

    /// For a program's own output, as herdr-core streams it: this terminal is the real one, with its history and
    /// its reflow on a resize.
    pub fn raw(cols: u16, rows: u16) -> Self {
        Self {
            raw: true,
            ..Self::sized(cols, rows, SCROLLBACK)
        }
    }

    /// herdr's history as `pane.read` gives it (`source: recent`, `format: ansi`): the lines above its screen, then
    /// the screen, wrapped at this width, no newline after the last. The grid holds them all, viewed at the live
    /// bottom, so [`Self::scroll_to_offset`] walks back through them without asking herdr.
    pub fn with_history(text: &str, cols: u16, rows: u16) -> Self {
        let mut e = Self::sized(cols, rows, SCROLLBACK);
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                e.feed(b"\r\n");
            }
            e.feed(b"\x1b[0m");
            e.feed(line.strip_suffix('\r').unwrap_or(line).as_bytes());
        }
        e
    }

    /// `max_scrollback`: bytes of history the terminal keeps (0: none).
    fn sized(cols: u16, rows: u16, max_scrollback: usize) -> Self {
        let mut term = Terminal::new(TerminalOptions {
            cols: cols.max(2),
            rows: rows.max(1),
            max_scrollback,
        })
        .expect("libghostty-vt terminal");
        // Commutator's cursor blinks by default, and DECSCUSR 0 resets to that
        // (Ghostty's own built-in default is a steady block).
        term.set_default_cursor_blink(Some(true))
            .expect("libghostty-vt default cursor blink");
        // Kitty graphics: enable the storage (a zero limit would keep the
        // protocol off), its file-loading mediums, and the PNG decoder the
        // storage needs for PNG-transmitted images.
        term.set_kitty_image_storage_limit(KITTY_STORAGE)
            .expect("libghostty-vt kitty storage limit");
        // The file-loading mediums are an optimization (`kitty icat
        // --transfer-mode=file`); this build may not support them, and the
        // base64 and PNG paths work either way.
        let _ = term.set_kitty_image_from_file_allowed(true);
        let _ = term.set_kitty_image_from_temp_file_allowed(true);
        let _ = term.set_kitty_image_from_shared_mem_allowed(true);
        set_png_decoder(Some(Box::new(RustPngDecoder::new())))
            .expect("libghostty-vt png decoder");
        let render = Render {
            state: RenderState::new().expect("libghostty-vt render state"),
            rows: RowIterator::new().expect("libghostty-vt row iterator"),
            cells: CellIterator::new().expect("libghostty-vt cell iterator"),
            lines: Vec::new(),
        };
        Self {
            term,
            render: RefCell::new(render),
            anchor: None,
            bell: false,
            raw: false,
            search: RefCell::new(None),
            images: RefCell::new(std::collections::HashMap::new()),
        }
    }

    /// Advance the state machine over decoded PTY output. Returns bytes the
    /// terminal wants written back to the PTY (DSR/DA query responses etc.).
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.term.vt_write(bytes);
        let mut responses = Vec::new();
        for event in self.term.events() {
            match event {
                Event::PtyWrite(bytes) => responses.extend_from_slice(&bytes),
                Event::Bell => self.bell = true,
                _ => {}
            }
        }
        responses
    }

    /// Ghostty handles synchronized output inside `vt_write`; no external
    /// deadline or flush is required by this backend.
    pub fn sync_deadline(&self) -> Option<std::time::Instant> {
        None
    }

    pub fn finish_sync(&mut self) -> Vec<u8> {
        Vec::new()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        // ponytail: no pixel size; only CSI 14/16 t size reports would want it.
        if self.raw {
            let _ = self.term.resize(cols.max(2), rows.max(1), 0, 0);
            return;
        }
        // No reflow: herdr repaints the whole grid at the new size, and until then a reflow shows its frame at the
        // old width rewrapped (Ghostty reflows only under wraparound).
        let wrap = self.term.mode(Mode::WRAPAROUND).unwrap_or(true);
        let _ = self.term.set_mode(Mode::WRAPAROUND, false);
        let _ = self.term.resize(cols.max(2), rows.max(1), 0, 0);
        let _ = self.term.set_mode(Mode::WRAPAROUND, wrap);
    }

    pub fn cols(&self) -> usize {
        self.term.cols().map_or(0, usize::from)
    }

    pub fn rows(&self) -> usize {
        self.term.rows().map_or(0, usize::from)
    }

    /// OSC title, if the running program set one.
    pub fn title(&self) -> Option<&str> {
        self.term.title().ok().filter(|t| !t.is_empty())
    }

    /// True once a BEL arrived; reading clears it.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell)
    }

    /// Arrow keys should send SS3 (`ESC O A`) instead of CSI.
    pub fn app_cursor_mode(&self) -> bool {
        self.term.mode(Mode::DECCKM).unwrap_or(false)
    }

    /// The app is on the alternate screen (vim, less): it has no history of its own.
    pub fn alt_screen(&self) -> bool {
        self.term
            .active_screen()
            .is_ok_and(|s| s == Screen::Alternate)
    }

    pub fn alternate_scroll(&self) -> bool {
        self.term.mode(Mode::ALT_SCROLL).unwrap_or(false)
    }

    pub fn focus_reporting(&self) -> bool {
        self.term.mode(Mode::FOCUS_EVENT).unwrap_or(false)
    }

    pub fn cursor_style(&self) -> (CursorShape, bool) {
        let shape = match self
            .render
            .borrow_mut()
            .state
            .update(&self.term)
            .ok()
            .and_then(|s| s.cursor_visual_style().ok())
        {
            Some(CursorVisualStyle::Bar) => CursorShape::Beam,
            Some(CursorVisualStyle::Underline) => CursorShape::Underline,
            Some(CursorVisualStyle::BlockHollow) => CursorShape::HollowBlock,
            _ => CursorShape::Block,
        };
        (shape, self.term.mode(Mode::CURSOR_BLINKING).unwrap_or(true))
    }

    /// Pastes should be wrapped in `ESC [200~` / `ESC [201~`.
    /// The program reads the mouse.
    pub fn mouse_capture(&self) -> bool {
        self.term.is_mouse_tracking().unwrap_or(false)
    }

    pub fn mouse_tracking(&self) -> Option<MouseTracking> {
        if self.term.mode(Mode::ANY_MOUSE).unwrap_or(false) {
            Some(MouseTracking::Motion)
        } else if self.term.mode(Mode::BUTTON_MOUSE).unwrap_or(false) {
            Some(MouseTracking::Drag)
        } else if self.term.mode(Mode::NORMAL_MOUSE).unwrap_or(false)
            || self.term.mode(Mode::X10_MOUSE).unwrap_or(false)
        {
            Some(MouseTracking::Click)
        } else {
            None
        }
    }

    pub fn sgr_mouse(&self) -> bool {
        self.term.mode(Mode::SGR_MOUSE).unwrap_or(false)
    }

    /// The kitty keyboard flags the program asked for.
    pub fn keyboard_flags(&self) -> u16 {
        self.term
            .kitty_keyboard_flags()
            .map_or(0, |f| u16::from(f.bits()))
    }

    /// A mouse event as the program asked to hear it, nothing if it did not: `action` is `down`, `up`, `drag` or
    /// `move`, `button` `left`, `right`, `middle`, or `up` or `down` for the wheel; `mods` as herdr's (1 shift, 2
    /// ctrl, 4 alt). Empty for a program that does not read this.
    pub fn mouse_report(
        &self,
        action: &str,
        button: &str,
        col: u16,
        row: u16,
        mods: u8,
    ) -> Vec<u8> {
        // cells stand in for pixels: only the SGR-pixels format would tell them apart
        const CELL: u32 = 10;
        let (Ok(mut encoder), Ok(mut event)) = (mouse::Encoder::new(), mouse::Event::new()) else {
            return Vec::new();
        };
        encoder
            .set_options_from_terminal(&self.term)
            .set_any_button_pressed(action == "drag")
            .set_size(mouse::EncoderSize {
                screen_width: self.cols() as u32 * CELL,
                screen_height: self.rows() as u32 * CELL,
                cell_width: CELL,
                cell_height: CELL,
                padding_top: 0,
                padding_bottom: 0,
                padding_right: 0,
                padding_left: 0,
            });
        event.set_action(match action {
            "down" => mouse::Action::Press,
            "up" => mouse::Action::Release,
            _ => mouse::Action::Motion,
        });
        event.set_button(match (action, button) {
            ("move", _) => None,
            (_, "right") => Some(mouse::Button::Right),
            (_, "middle") => Some(mouse::Button::Middle),
            (_, "up") => Some(mouse::Button::Four),
            (_, "down") => Some(mouse::Button::Five),
            _ => Some(mouse::Button::Left),
        });
        let mut held = key::Mods::empty();
        held.set(key::Mods::SHIFT, mods & 1 != 0);
        held.set(key::Mods::CTRL, mods & 2 != 0);
        held.set(key::Mods::ALT, mods & 4 != 0);
        event.set_mods(held);
        let at = |cell: u16| (u32::from(cell) * CELL + CELL / 2) as f32;
        event.set_position(mouse::Position {
            x: at(col),
            y: at(row),
        });
        let mut bytes = Vec::new();
        let _ = encoder.encode_to_vec(&event, &mut bytes);
        bytes
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.term.mode(Mode::BRACKETED_PASTE).unwrap_or(false)
    }

    pub fn live_cursor(&self) -> CursorSnapshot {
        let (row, col) = self.cursor_cell().unwrap_or_default();
        CursorSnapshot { row, col }
    }

    pub fn live_char(&self, row: usize, col: usize) -> Option<char> {
        self.line(row).get(col).map(|cell| cell.ch)
    }

    /// Lines kept above the screen (0 but in a [`Self::with_history`] copy: herdr owns history).
    pub fn history_lines(&self) -> usize {
        self.term.scrollback_rows().unwrap_or(0)
    }

    /// Lines the view is scrolled back from the live bottom.
    pub fn display_offset(&self) -> usize {
        self.term
            .scrollbar()
            .map_or(0, |s| s.total.saturating_sub(s.offset + s.len) as usize)
    }

    /// Scroll the view to `offset` lines back from the live bottom (clamped to the history).
    pub fn scroll_to_offset(&mut self, offset: usize) {
        let top = self.history_lines().saturating_sub(offset);
        self.term.scroll_viewport(ScrollViewport::Row(top));
    }

    pub fn scroll(&mut self, delta: i32) {
        let next = if delta >= 0 {
            self.display_offset().saturating_add(delta as usize)
        } else {
            self.display_offset()
                .saturating_sub(delta.unsigned_abs() as usize)
        };
        self.scroll_to_offset(next);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_to_offset(0);
    }

    // ---- selection ----
    //
    // The terminal owns the installed selection and moves it with its text
    // as output scrolls the grid. What stays here is the anchor of the one
    // being made, so each pointer move rebuilds it from anchor to pointer.

    /// A cell of the viewport (row 0 = top of the visible area).
    fn viewport_ref(&self, row: usize, col: usize) -> Option<GridRef<'_>> {
        let (x, y) = (u16::try_from(col).ok()?, u32::try_from(row).ok()?);
        self.term
            .grid_ref(Point::Viewport(PointCoordinate { x, y }))
            .ok()
    }

    /// A cell by its screen point: the history and the screen, so it stays on its text as the viewport moves.
    fn grid_ref(&self, point: GridPoint) -> Option<GridRef<'_>> {
        let (x, y) = (
            u16::try_from(point.col).ok()?,
            u32::try_from(point.row).ok()?,
        );
        self.term
            .grid_ref(Point::Screen(PointCoordinate { x, y }))
            .ok()
    }

    fn point_of(&self, grid_ref: &GridRef<'_>) -> Option<GridPoint> {
        let c = self
            .term
            .point_from_grid_ref(grid_ref, PointSpace::Screen)
            .ok()??;
        Some(GridPoint {
            row: c.y as usize,
            col: c.x as usize,
        })
    }

    /// A selection's ends, top-left first.
    fn bounds(&self, selection: Option<Selection<'_>>) -> Option<(GridPoint, GridPoint)> {
        let selection = selection?;
        let (a, b) = (
            self.point_of(&selection.start())?,
            self.point_of(&selection.end())?,
        );
        Some((a.min(b), a.max(b)))
    }

    /// The screen point under a viewport cell. Selections keep these, so they hold on their text when the view
    /// scrolls.
    pub fn grid_point(&self, viewport_row: usize, col: usize) -> GridPoint {
        let col = col.min(self.cols().saturating_sub(1));
        self.viewport_ref(viewport_row, col)
            .and_then(|r| self.point_of(&r))
            .unwrap_or(GridPoint {
                row: viewport_row,
                col,
            })
    }

    /// The selection as it stands, in screen points: what [`Self::restore_selection`] carries to another grid.
    pub fn selection_state(&self) -> SelectionState {
        SelectionState {
            anchor: self.anchor,
            ends: self.bounds(self.term.selection().ok().flatten()),
        }
    }

    /// Installs a selection taken from another grid, its rows moved by `shift` (the history difference between
    /// the two). A row that falls off the top of this grid drops the selection.
    pub fn restore_selection(&mut self, state: SelectionState, shift: isize) {
        let moved = |p: GridPoint| {
            Some(GridPoint {
                row: p.row.checked_add_signed(shift)?,
                ..p
            })
        };
        self.anchor = state.anchor.and_then(|a| {
            Some(Anchor {
                point: moved(a.point)?,
                ..a
            })
        });
        let ends = state.ends.and_then(|(a, b)| Some((moved(a)?, moved(b)?)));
        let selection = ends.and_then(|(start, end)| {
            Some(Selection::new(
                self.grid_ref(start)?,
                self.grid_ref(end)?,
                false,
            ))
        });
        let _ = self.term.set_selection(selection.as_ref());
    }

    /// Begin a selection. `ty` picks the granularity: [`SelectionType::Simple`]
    /// for a drag, `Semantic` for a double-click word, `Lines` for a triple-
    /// click row.
    pub fn start_selection(&mut self, ty: SelectionType, point: GridPoint, side: Side) {
        self.anchor = Some(Anchor { ty, point, side });
        self.update_selection(point, side);
    }

    /// Extend the in-progress selection to `point`. No-op without one.
    pub fn update_selection(&mut self, point: GridPoint, side: Side) {
        let Some(anchor) = self.anchor else { return };
        let range = match anchor.ty {
            SelectionType::Simple => {
                simple_range((anchor.point, anchor.side), (point, side), self.cols())
            }
            // Ghostty's double-click drag: the nearest word from each end toward the other.
            SelectionType::Semantic => {
                let word = |from, to| {
                    let options =
                        SelectWordBetweenOptions::new(self.grid_ref(from)?, self.grid_ref(to)?);
                    self.bounds(self.term.select_word_between(options).ok()?)
                };
                union(word(anchor.point, point), word(point, anchor.point))
            }
            SelectionType::Lines => {
                let line = |at| {
                    self.bounds(
                        self.term
                            .select_line(SelectLineOptions::new(self.grid_ref(at)?))
                            .ok()?,
                    )
                };
                union(line(anchor.point), line(point))
            }
        };
        let selection = range.and_then(|(start, end)| {
            Some(Selection::new(
                self.grid_ref(start)?,
                self.grid_ref(end)?,
                false,
            ))
        });
        let _ = self.term.set_selection(selection.as_ref());
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
        let _ = self.term.set_selection(None);
    }

    /// The selected text, or `None` when there is no selection or it covers
    /// nothing (a click without a drag leaves an empty one behind).
    pub fn selection_text(&self) -> Option<String> {
        // Ghostty's copy: plain text, soft wraps joined, trailing blanks trimmed.
        let options = FormatOptions::new().with_unwrap(true).with_trim(true);
        let bytes = self.term.format_selection_alloc(options).ok()??;
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        // A row selection pastes as a row, line break included (Ghostty's trim drops it).
        if self.anchor.is_some_and(|a| a.ty == SelectionType::Lines) && !text.is_empty() {
            text.push('\n');
        }
        Some(text).filter(|s| !s.is_empty())
    }

    /// Whether a non-empty selection is active — drives the copy action and
    /// the "clear it" branch on the next click.
    pub fn has_selection(&self) -> bool {
        self.term.selection().is_ok_and(|s| s.is_some())
    }

    /// Snapshot one viewport row (0 = top).
    // ponytail: snapshots the whole viewport; only hover and tests ask for one row.
    pub fn line(&self, viewport_row: usize) -> Vec<CellSnapshot> {
        self.lines()
            .into_iter()
            .nth(viewport_row)
            .unwrap_or_default()
    }

    /// All viewport rows, top to bottom. A row that has not changed since the last read is the same `Arc`.
    pub fn lines(&self) -> Vec<Vec<CellSnapshot>> {
        self.read_lines()
            .unwrap_or_default()
            .into_iter()
            .map(Arc::unwrap_or_clone)
            .collect()
    }

    fn read_lines(&self) -> Result<Vec<Arc<Vec<CellSnapshot>>>, Error> {
        let mut render = self.render.borrow_mut();
        let Render {
            state,
            rows,
            cells,
            lines: cache,
        } = &mut *render;
        let plain = Style::default();
        let snapshot = state.update(&self.term)?;
        let full = snapshot.dirty()? == Dirty::Full;
        let mut lines = Vec::with_capacity(self.rows());
        {
            let mut row_iter = rows.update(&snapshot)?;
            while let Some(row) = row_iter.next() {
                let selected = row.selection()?.map(|s| s.start_x..=s.end_x);
                // the selection is taken afresh for a clean row too: a changed one may leave the row clean
                if let Some(mut line) = cache
                    .get(lines.len())
                    .filter(|l| !full && l.len() == self.cols() && !row.dirty().unwrap_or(true))
                    .cloned()
                {
                    let in_selection =
                        |col: usize| selected.as_ref().is_some_and(|r| r.contains(&(col as u16)));
                    if line
                        .iter()
                        .enumerate()
                        .any(|(col, cell)| cell.selected != in_selection(col))
                    {
                        for (col, cell) in Arc::make_mut(&mut line).iter_mut().enumerate() {
                            cell.selected = in_selection(col);
                        }
                    }
                    lines.push(line);
                    continue;
                }
                let mut line = Vec::with_capacity(self.cols());
                let mut cell_iter = cells.update(row)?;
                while let Some(cell) = cell_iter.next() {
                    let raw = cell.raw_cell()?;
                    let style = if cell.has_styling()? {
                        cell.style()?
                    } else {
                        plain
                    };
                    // An erase with a background set stores the color in the cell, not a style.
                    let tag = raw.content_tag()?;
                    let bg = match tag {
                        CellContentTag::BgColorPalette => {
                            CellColor::Indexed(raw.bg_color_palette()?.0)
                        }
                        CellContentTag::BgColorRgb => {
                            let c = raw.bg_color_rgb()?;
                            CellColor::Rgb(c.r, c.g, c.b)
                        }
                        _ => map_color(style.bg_color, CellColor::Background),
                    };
                    let wide = raw.wide()?;
                    let col = line.len() as u16;
                    let cluster = match tag {
                        CellContentTag::CodepointGrapheme => {
                            let mut text = String::new();
                            cell.graphemes_utf8(&mut text)?;
                            Some(text.into())
                        }
                        _ => None,
                    };
                    line.push(CellSnapshot {
                        ch: char::from_u32(raw.codepoint()?)
                            .filter(|&c| c != '\0')
                            .unwrap_or(' '),
                        cluster,
                        fg: map_color(style.fg_color, CellColor::Foreground),
                        bg,
                        bold: style.bold,
                        dim: style.faint,
                        italic: style.italic,
                        underline: style.underline != Underline::None,
                        underline_style: match style.underline {
                            Underline::Double => UnderlineStyle::Double,
                            Underline::Curly => UnderlineStyle::Curly,
                            Underline::Dotted => UnderlineStyle::Dotted,
                            Underline::Dashed => UnderlineStyle::Dashed,
                            _ => UnderlineStyle::Single,
                        },
                        strikethrough: style.strikethrough,
                        inverse: style.inverse,
                        hidden: style.invisible,
                        wide: wide == CellWide::Wide,
                        wide_spacer: matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead),
                        selected: selected.as_ref().is_some_and(|r| r.contains(&col)),
                        predicted: false,
                    });
                }
                lines.push(Arc::new(line));
            }
        }
        snapshot.clean()?;
        cache.clone_from(&lines);
        Ok(lines)
    }

    /// Cursor in viewport coordinates; `None` when hidden.
    pub fn cursor(&self) -> Option<CursorSnapshot> {
        if !self.term.is_cursor_visible().ok()? {
            return None;
        }
        let (row, col) = self.cursor_cell()?;
        Some(CursorSnapshot { row, col })
    }

    /// The cursor's (row, col) in viewport coordinates, shown or hidden: an IME composes there (agent TUIs hide the
    /// cursor and park it at their input). `None` while the viewport is scrolled back off the cursor's line: the
    /// terminal's own cursor_x/y are on the active screen and would pin it to a row of old history.
    pub fn cursor_cell(&self) -> Option<(usize, usize)> {
        let at = self
            .render
            .borrow_mut()
            .state
            .update(&self.term)
            .ok()?
            .cursor_viewport()
            .ok()??;
        Some((at.y as usize, at.x as usize))
    }

    fn hyperlink(&self, row: usize, col: usize) -> Option<String> {
        let grid_ref = self.viewport_ref(row, col)?;
        let mut buf = vec![0; 512];
        let len = match grid_ref.hyperlink_uri(&mut buf) {
            Err(Error::OutOfSpace { required }) => {
                buf.resize(required, 0);
                grid_ref.hyperlink_uri(&mut buf).ok()?
            }
            other => other.ok()?,
        };
        (len > 0).then(|| String::from_utf8_lossy(&buf[..len]).into_owned())
    }

    /// Whether a viewport row soft-wraps into the next: one line of the program's that the grid's width cut.
    pub fn wraps(&self, viewport_row: usize) -> bool {
        self.viewport_ref(viewport_row, 0)
            .and_then(|r| r.row().ok())
            .is_some_and(|r| r.is_wrapped().unwrap_or(false))
    }

    /// The shell's working directory as it last reported (OSC 7), for
    /// resolving a link's relative paths. `None` until the shell says.
    pub fn pwd(&self) -> Option<String> {
        self.term
            .pwd()
            .ok()
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
    }

    // ---- scrollback search ----

    /// Start (or restart) the scrollback search for `needle`; `None` or an
    /// empty one clears it. A needle change runs the whole search — the grid
    /// is local — and steps to the newest match, so the first Enter moves
    /// from somewhere sensible.
    pub fn search_set(&self, needle: Option<&str>) {
        let mut slot = self.search.borrow_mut();
        match needle.filter(|n| !n.is_empty()) {
            None => *slot = None,
            Some(needle) => {
                let search = slot
                    .get_or_insert_with(|| Search::new(&self.term).expect("libghostty-vt search"));
                let _ = search.set_needle(&self.term, Some(needle));
                let _ = search.run(&self.term);
                // From nothing, a step lands on the wrap-around match;
                // select_next wraps to the NEWEST, which is where a fresh
                // query should start.
                let _ = search.select_next(&self.term);
            }
        }
    }

    /// Step to the next match — older content for `older`, newer otherwise —
    /// scrolling it into view when it is off screen. False when there is
    /// nothing to step to.
    pub fn search_step(&self, older: bool) -> bool {
        let mut slot = self.search.borrow_mut();
        let Some(search) = slot.as_mut() else {
            return false;
        };
        let _ = search.feed(&self.term);
        if older {
            search.select_next(&self.term).unwrap_or(false)
        } else {
            search.select_prev(&self.term).unwrap_or(false)
        }
    }

    /// The search's tally for its counter: `(total, current-from-newest)`.
    /// `None` when no search is open; a current of `None` before the first
    /// step.
    pub fn search_count(&self) -> Option<(usize, Option<usize>)> {
        let slot = self.search.borrow();
        let search = slot.as_ref()?;
        Some((
            search.total_matches().ok()?,
            search.selected_index().ok().flatten(),
        ))
    }

    /// The search's matches on the viewport's pages, as `(row, cells,
    /// current)` spans for painting: a match a soft wrap cut across rows is
    /// one span per row; the stepped-to match is `current`.
    pub fn search_marks(&self) -> Vec<(usize, std::ops::Range<usize>, bool)> {
        let mut slot = self.search.borrow_mut();
        let Some(search) = slot.as_mut() else {
            return Vec::new();
        };
        let _ = search.feed(&self.term);
        let matches = search.viewport_matches(&self.term).unwrap_or_default();
        // The current match, matched by its screen coordinates: viewport
        // coordinates do not exist for a match that just scrolled off.
        let current = search
            .selected_match(&self.term)
            .ok()
            .flatten()
            .and_then(|selected| self.selection_key(&selected));
        let cols = self.cols() as u32;
        let mut marks = Vec::new();
        for hit in &matches {
            let now = self.selection_key(hit).is_some_and(|key| Some(key) == current);
            let (Some(start), Some(end)) = (
                self.term
                    .point_from_grid_ref(&hit.start(), PointSpace::Viewport)
                    .ok()
                    .flatten(),
                self.term
                    .point_from_grid_ref(&hit.end(), PointSpace::Viewport)
                    .ok()
                    .flatten(),
            ) else {
                continue;
            };
            // A match runs top-left to bottom-right; order defensively anyway.
            let ((top, left), (bottom, right)) = if (start.y, start.x) <= (end.y, end.x) {
                ((start.y, start.x as u32), (end.y, end.x as u32))
            } else {
                ((end.y, end.x as u32), (start.y, start.x as u32))
            };
            for row in top..=bottom {
                let range = if row == top && row == bottom {
                    left..right + 1
                } else if row == top {
                    left..cols
                } else if row == bottom {
                    0..right + 1
                } else {
                    0..cols
                };
                let range = range.start.min(cols)..range.end.min(cols);
                if range.start < range.end {
                    marks.push((row as usize, range.start as usize..range.end as usize, now));
                }
            }
        }
        marks
    }

    /// A selection's ends in screen coordinates, to compare two selections by.
    fn selection_key(&self, selection: &Selection<'_>) -> Option<(u32, u16, u32, u16)> {
        let start = self
            .term
            .point_from_grid_ref(&selection.start(), PointSpace::Screen)
            .ok()??;
        let end = self
            .term
            .point_from_grid_ref(&selection.end(), PointSpace::Screen)
            .ok()??;
        Some((start.y, start.x, end.y, end.x))
    }

    // ---- kitty graphics ----

    /// The Kitty graphics placements currently in the viewport, for
    /// painting: every visible one, its viewport cell, and its decoded
    /// pixels. Pixels are cached by `(image id, generation)`, so a frame
    /// after the first copies nothing.
    pub fn image_placements(&self) -> Vec<ImagePlacement> {
        let mut out: Vec<ImagePlacement> = Vec::new();
        let Ok(graphics) = self.term.kitty_graphics() else {
            return out;
        };
        let mut iterator = match PlacementIterator::new() {
            Ok(iterator) => iterator,
            Err(_) => return out,
        };
        let mut iter = match iterator.update(&graphics) {
            Ok(iter) => iter,
            Err(_) => return out,
        };
        while let Some(placement) = iter.next() {
            let (Ok(image_id), Ok(z), Ok(is_virtual)) =
                (placement.image_id(), placement.z(), placement.is_virtual())
            else {
                continue;
            };
            if is_virtual {
                continue;
            }
            let Some(image) = graphics.image(image_id) else {
                continue;
            };
            let (Ok(generation), Ok(width), Ok(height), Ok(format)) = (
                image.generation(),
                image.width(),
                image.height(),
                image.format(),
            ) else {
                continue;
            };
            let Ok(info) = placement.placement_render_info(&image, &self.term) else {
                continue;
            };
            if !info.viewport_visible {
                continue;
            }
            let pixels = {
                let mut cache = self.images.borrow_mut();
                if let Some(cached) = cache.get(&(image_id, generation)) {
                    cached.clone()
                } else {
                    let Ok(data) = image.data() else { continue };
                    let rgba = match format {
                        ImageFormat::Rgba => data.to_vec(),
                        ImageFormat::Rgb => data
                            .chunks_exact(3)
                            .flat_map(|px| [px[0], px[1], px[2], 255])
                            .collect(),
                        ImageFormat::Gray => {
                            data.iter().flat_map(|&g| [g, g, g, 255]).collect()
                        }
                        ImageFormat::GrayAlpha => data
                            .chunks_exact(2)
                            .flat_map(|px| [px[0], px[0], px[0], px[1]])
                            .collect(),
                        // The storage decodes PNG before storing; anything
                        // else is not a format we can paint.
                        _ => continue,
                    };
                    let pixels = std::sync::Arc::new(ImagePixels { width, height, rgba });
                    cache.insert((image_id, generation), pixels.clone());
                    pixels
                }
            };
            out.push(ImagePlacement {
                image_id,
                z,
                col: info.viewport_col,
                row: info.viewport_row,
                cols: info.grid_cols,
                rows: info.grid_rows,
                pixels,
            });
        }
        // Keep the cache from outliving the images it holds: drop entries
        // no live placement references once it grows past its cap.
        {
            let mut cache = self.images.borrow_mut();
            if cache.len() > IMAGE_CACHE_MAX {
                cache.retain(|_, cached| {
                    out.iter()
                        .any(|p| std::sync::Arc::ptr_eq(&p.pixels, cached))
                });
            }
        }
        out
    }

    /// The OSC 8 hyperlink at a viewport cell: the cells around it with the same URI, a range per row and top first,
    /// on into the rows a soft wrap joins it to; and the URI.
    pub fn hyperlink_at(
        &self,
        viewport_row: usize,
        col: usize,
    ) -> Option<(Vec<(usize, std::ops::Range<usize>)>, String)> {
        let uri = self.hyperlink(viewport_row, col)?;
        let cols = self.cols();
        let same = |row: usize, c: usize| self.hyperlink(row, c).as_ref() == Some(&uri);
        let span = |row: usize, c: usize| {
            let start = (0..c)
                .rev()
                .take_while(|&x| same(row, x))
                .last()
                .unwrap_or(c);
            let end = (c + 1..cols)
                .take_while(|&x| same(row, x))
                .last()
                .unwrap_or(c)
                + 1;
            start..end
        };
        let mut spans = vec![(viewport_row, span(viewport_row, col))];
        while let Some((row, cells)) = spans.first().cloned() {
            if cells.start != 0
                || row == 0
                || spans.len() >= MAX_WRAPPED_ROWS
                || !self.wraps(row - 1)
                || !same(row - 1, cols - 1)
            {
                break;
            }
            spans.insert(0, (row - 1, span(row - 1, cols - 1)));
        }
        while let Some((row, cells)) = spans.last().cloned() {
            if cells.end != cols
                || row + 1 >= self.rows()
                || spans.len() >= MAX_WRAPPED_ROWS
                || !self.wraps(row)
                || !same(row + 1, 0)
            {
                break;
            }
            spans.push((row + 1, span(row + 1, 0)));
        }
        Some((spans, uri))
    }

    /// Test/diagnostic helper: a viewport row as trimmed text (wide-char
    /// spacers skipped).
    pub fn row_text(&self, viewport_row: usize) -> String {
        let mut text: String = self
            .line(viewport_row)
            .iter()
            .filter(|c| !c.wide_spacer)
            .map(|c| c.ch)
            .collect();
        while text.ends_with(' ') {
            text.pop();
        }
        text
    }
}

/// How many rows a link joined by soft wraps reaches, its own included: a link longer than that is cut, so a row
/// of one never walks the whole history.
pub const MAX_WRAPPED_ROWS: usize = 16;

/// A drag's cells, top-left first: the side of each end says whether its cell is in.
fn simple_range(
    a: (GridPoint, Side),
    b: (GridPoint, Side),
    cols: usize,
) -> Option<(GridPoint, GridPoint)> {
    let ((mut start, start_side), (mut end, end_side)) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    if start_side == Side::Right {
        start = if start.col + 1 < cols {
            GridPoint {
                col: start.col + 1,
                ..start
            }
        } else {
            GridPoint {
                row: start.row + 1,
                col: 0,
            }
        };
    }
    if end_side == Side::Left {
        end = match end.col {
            0 => GridPoint {
                row: end.row.checked_sub(1)?,
                col: cols.saturating_sub(1),
            },
            col => GridPoint {
                col: col - 1,
                ..end
            },
        };
    }
    (start <= end).then_some((start, end))
}

/// The span covering both ranges.
fn union(
    a: Option<(GridPoint, GridPoint)>,
    b: Option<(GridPoint, GridPoint)>,
) -> Option<(GridPoint, GridPoint)> {
    match (a, b) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
        (a, b) => a.or(b),
    }
}

impl std::fmt::Debug for Emulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emulator")
            .field("cols", &self.cols())
            .field("rows", &self.rows())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emu(cols: u16, rows: u16) -> Emulator {
        Emulator::new(cols, rows)
    }

    #[test]
    fn alt_screen_restores_primary_content() {
        let mut e = emu(20, 4);
        e.feed(b"primary");
        e.feed(b"\x1b[?1049h\x1b[H");
        assert!(e.alt_screen());
        e.feed(b"alt-content");
        assert_eq!(e.row_text(0), "alt-content");
        e.feed(b"\x1b[?1049l");
        assert!(!e.alt_screen());
        assert_eq!(e.row_text(0), "primary");
    }

    #[test]
    fn dsr_cursor_report_produces_pty_response() {
        let mut e = emu(20, 4);
        e.feed(b"\x1b[2;3H");
        let responses = e.feed(b"\x1b[6n");
        assert_eq!(String::from_utf8_lossy(&responses), "\x1b[2;3R");
    }

    #[test]
    fn osc_title_and_bell() {
        let mut e = emu(20, 2);
        assert_eq!(e.title(), None);
        e.feed(b"\x1b]0;my title\x07");
        assert_eq!(e.title(), Some("my title"));
        assert!(!e.take_bell());
        e.feed(b"\x07");
        assert!(e.take_bell());
        assert!(!e.take_bell(), "bell reads clear it");
    }

    #[test]
    fn app_cursor_and_bracketed_paste_modes_toggle() {
        let mut e = emu(10, 2);
        assert!(!e.app_cursor_mode());
        e.feed(b"\x1b[?1h");
        assert!(e.app_cursor_mode());
        e.feed(b"\x1b[?1l");
        assert!(!e.app_cursor_mode());
        e.feed(b"\x1b[?2004h");
        assert!(e.bracketed_paste_mode());
    }

    #[test]
    fn hidden_cursor_mode() {
        let mut e = emu(10, 2);
        e.feed(b"\x1b[?25l");
        assert_eq!(e.cursor(), None);
        e.feed(b"\x1b[?25h");
        assert!(e.cursor().is_some());
    }

    #[test]
    fn cursor_style_follows_decscusr() {
        let mut e = emu(10, 2);
        assert_eq!(e.cursor_style(), (CursorShape::Block, true));
        e.feed(b"\x1b[6 q");
        assert_eq!(e.cursor_style(), (CursorShape::Beam, false));
        e.feed(b"\x1b[3 q");
        assert_eq!(e.cursor_style(), (CursorShape::Underline, true));
        // DECSCUSR 0 resets to the default Commutator configured: a blinking block.
        e.feed(b"\x1b[0 q");
        assert_eq!(e.cursor_style(), (CursorShape::Block, true));
    }

    #[test]
    fn kitty_graphics_place_and_render() {
        let mut e = emu(10, 4);
        // A 2×1 RGBA image (a red then a green pixel), transmitted and
        // displayed at the cursor (`a=T`, the default action): two cells
        // wide, one row tall. Base64 of FF 00 00 FF 00 FF 00 FF.
        e.feed(b"\x1b_Gf=32,s=2,v=1,a=T,c=2,r=1;/wAA/wD/AP8=\x1b\\");
        let placements = e.image_placements();
        assert_eq!(placements.len(), 1);
        let p = &placements[0];
        assert_eq!((p.col, p.row, p.cols, p.rows), (0, 0, 2, 1));
        assert_eq!(p.pixels.rgba, vec![255, 0, 0, 255, 0, 255, 0, 255]);
        assert_eq!((p.pixels.width, p.pixels.height), (2, 1));
        // The second read is served from the pixel cache (same Arc).
        let again = e.image_placements();
        assert!(std::sync::Arc::ptr_eq(&again[0].pixels, &p.pixels));
        // Scrolling the image off the screen removes it from the viewport.
        e.feed(b"\r\n\r\n\r\n\r\n");
        assert!(e.image_placements().is_empty());
    }

    #[test]
    fn search_steps_marks_and_counts() {
        // Two rows of history above a two-row screen; "alpha" three times.
        let mut e = emu(20, 2);
        e.feed(b"alpha beta\r\nalpha gamma\r\nalpha again\r\n");
        assert_eq!(e.history_lines(), 2);
        e.search_set(Some("alpha"));
        // Setting the needle selects the newest match, on screen at row 0
        // (the empty last line pushed "alpha gamma" into history).
        assert_eq!(e.search_count(), Some((3, Some(0))));
        let marks = e.search_marks();
        assert_eq!(marks, vec![(0, 0..5, true)]);
        // Stepping older walks into history; the match scrolls into view.
        assert!(e.search_step(true));
        assert_eq!(e.search_count(), Some((3, Some(1))));
        assert!(e.display_offset() > 0);
        assert!(e.search_step(true));
        assert_eq!(e.search_count(), Some((3, Some(2))));
        // Wraps back to the newest.
        assert!(e.search_step(true));
        assert_eq!(e.search_count(), Some((3, Some(0))));
        // Stepping newer wraps the other way.
        assert!(e.search_step(false));
        assert_eq!(e.search_count(), Some((3, Some(2))));
        // Clearing drops the search.
        e.search_set(None);
        assert_eq!(e.search_count(), None);
        assert!(e.search_marks().is_empty());
        assert!(!e.search_step(true));
    }

    #[test]
    fn resize_preserves_content_and_reflows() {
        let mut e = emu(20, 5);
        e.feed(b"abcdef\r\nsecond");
        e.resize(4, 5);
        assert_eq!(e.cols(), 4);
        assert_eq!(e.rows(), 5);
        assert_eq!(e.row_text(0), "abcd");
        assert_eq!(e.row_text(1), "ef");
        assert_eq!(e.row_text(2), "seco");
        assert_eq!(e.row_text(3), "nd");
        e.resize(20, 5);
        assert_eq!(e.row_text(0), "abcdef");
        assert_eq!(e.row_text(1), "second");
    }

    #[test]
    fn wide_chars_occupy_two_cells_with_spacer() {
        let mut e = emu(10, 2);
        e.feed("宽w".as_bytes());
        let line = e.line(0);
        assert!(line[0].wide);
        assert_eq!(line[0].ch, '宽');
        assert!(line[1].wide_spacer);
        assert_eq!(line[2].ch, 'w');
        assert_eq!(e.row_text(0), "宽w");
        assert_eq!(e.cursor(), Some(CursorSnapshot { row: 0, col: 3 }));
    }

    #[test]
    fn simple_selection_yields_its_text_and_marks_its_cells() {
        let mut e = emu(20, 3);
        e.feed(b"hello world");
        assert!(!e.has_selection());
        assert_eq!(e.selection_text(), None);

        e.start_selection(SelectionType::Simple, e.grid_point(0, 0), Side::Left);
        e.update_selection(e.grid_point(0, 4), Side::Right);
        assert!(e.has_selection());
        assert_eq!(e.selection_text().as_deref(), Some("hello"));

        let line = e.line(0);
        assert!(line[..5].iter().all(|c| c.selected));
        assert!(!line[5].selected, "the space past the drag is not selected");

        e.clear_selection();
        assert!(!e.has_selection());
        assert!(e.line(0).iter().all(|c| !c.selected));
    }

    #[test]
    fn semantic_selection_expands_to_the_word() {
        let mut e = emu(30, 2);
        e.feed(b"alpha beta gamma");
        e.start_selection(SelectionType::Semantic, e.grid_point(0, 7), Side::Left);
        assert_eq!(e.selection_text().as_deref(), Some("beta"));
    }

    #[test]
    fn line_selection_takes_the_whole_row() {
        let mut e = emu(30, 3);
        e.feed(b"first row\r\nsecond row");
        e.start_selection(SelectionType::Lines, e.grid_point(1, 3), Side::Left);
        assert_eq!(e.selection_text().as_deref(), Some("second row\n"));
    }

    #[test]
    fn selection_spans_rows_with_a_newline() {
        let mut e = emu(10, 3);
        e.feed(b"ab\r\ncd");
        e.start_selection(SelectionType::Simple, e.grid_point(0, 0), Side::Left);
        e.update_selection(e.grid_point(1, 1), Side::Right);
        assert_eq!(e.selection_text().as_deref(), Some("ab\ncd"));
    }

    #[test]
    fn a_click_without_a_drag_selects_nothing() {
        let mut e = emu(20, 2);
        e.feed(b"hello");
        e.start_selection(SelectionType::Simple, e.grid_point(0, 2), Side::Left);
        assert_eq!(e.selection_text(), None);
        assert!(!e.has_selection());
    }

    #[test]
    fn utf8_split_across_feeds_reassembles() {
        let mut e = emu(10, 2);
        let bytes = "é".as_bytes();
        e.feed(&bytes[..1]);
        e.feed(&bytes[1..]);
        assert_eq!(e.row_text(0), "é");
    }

    #[test]
    fn underline_styles_map_from_sgr_4() {
        let mut e = emu(20, 2);
        e.feed(b"\x1b[4:1ma\x1b[4:2mb\x1b[4:3mc\x1b[4:4md\x1b[4:5me\x1b[0m");
        let line = e.line(0);
        assert_eq!(line[0].underline_style, UnderlineStyle::Single);
        assert_eq!(line[1].underline_style, UnderlineStyle::Double);
        assert_eq!(line[2].underline_style, UnderlineStyle::Curly);
        assert_eq!(line[3].underline_style, UnderlineStyle::Dotted);
        assert_eq!(line[4].underline_style, UnderlineStyle::Dashed);
    }

    #[test]
    fn scrollback_tracks_display_offset_line_by_line() {
        let mut e = emu(20, 4);
        for i in 1..=10 {
            if i > 1 {
                e.feed(b"\r\n");
            }
            e.feed(format!("l{i}").as_bytes());
        }
        assert_eq!(e.history_lines(), 6);
        assert_eq!(e.display_offset(), 0);
        assert_eq!(e.row_text(0), "l7");
        e.scroll(2);
        assert_eq!(e.display_offset(), 2);
        assert_eq!(e.row_text(0), "l5");
        e.scroll(-1);
        assert_eq!(e.display_offset(), 1);
        assert_eq!(e.row_text(0), "l6");
        e.scroll_to_bottom();
        assert_eq!(e.display_offset(), 0);
        assert_eq!(e.row_text(0), "l7");
    }

    #[test]
    fn an_osc8_hyperlink_spans_its_cells() {
        let mut e = emu(30, 2);
        e.feed(b"see \x1b]8;;https://a.b\x1b\\li\x1b[1mnk\x1b]8;;\x1b\\ ok");
        assert_eq!(
            e.hyperlink_at(0, 6),
            Some((vec![(0, 4..8)], "https://a.b".into()))
        );
        assert_eq!(e.hyperlink_at(0, 4).map(|h| h.0), Some(vec![(0, 4..8)]));
        assert_eq!(e.hyperlink_at(0, 2), None);
        assert_eq!(e.hyperlink_at(0, 9), None);
    }

    #[test]
    fn an_osc8_hyperlink_follows_a_soft_wrap() {
        let mut e = emu(10, 3);
        e.feed(
            b"see \x1b]8;;https://a.b\x1b\\0123456789\x1b]8;;\x1b\\ ok\r\n\x1b]8;;https://c.d\x1b\\x\x1b]8;;\x1b\\",
        );
        let link = Some((vec![(0, 4..10), (1, 0..4)], "https://a.b".into()));
        assert_eq!(e.hyperlink_at(1, 2), link);
        assert_eq!(e.hyperlink_at(0, 5), link);
        assert_eq!(e.hyperlink_at(2, 0).map(|h| h.0), Some(vec![(2, 0..1)]));
    }
}
