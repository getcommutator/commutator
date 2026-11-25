//! Ghostty's sprite font: box drawing, blocks, braille, branch/git-graph,
//! geometric shapes, Powerline and Legacy Computing glyphs drawn to the cell
//! on the pixel grid instead of taken from the font.
//!
//! Ported from Ghostty's src/font/sprite/ @ main, 2026-10 (MIT, see
//! `THIRD_PARTY_NOTICES.md`): `canvas.zig` → [`canvas`], `draw/*.zig` → the
//! submodules.
//! z2d is replaced by tiny-skia. Cursor and underline sprites
//! (`special.zig`) are not ported: the terminal paints those itself.
//! The source renderer is independent of terminal backends, so the same glyph
//! grid is used for each platform's terminal frame.

pub mod canvas;
pub mod common;

mod block;
mod box_drawing;
mod braille;
mod branch;
mod geometric_shapes;
mod legacy_computing;
mod legacy_computing_supplement;
mod powerline;

use canvas::Canvas;

/// Grid metrics, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Metrics {
    pub cell_width: u32,
    pub cell_height: u32,
    /// Light line thickness; heavy is 2×. Ghostty uses the font's underline
    /// thickness.
    pub box_thickness: u32,
}

/// Draws codepoint `cp` on a canvas of `width`×`height` (the sprite size).
pub type DrawFn = fn(cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics);

/// Codepoint ranges and their drawing functions, ascending.
const RANGES: &[(u32, u32, DrawFn)] = &[
    (0x2500, 0x257F, box_drawing::draw_2500_257f),
    (0x2580, 0x259F, block::draw_2580_259f),
    (0x25E2, 0x25E5, geometric_shapes::draw_25e2_25e5),
    (0x25F8, 0x25FA, geometric_shapes::draw_25f8_25fa),
    (0x25FF, 0x25FF, geometric_shapes::draw_25ff),
    (0x2800, 0x28FF, braille::draw_2800_28ff),
    (0xE0B0, 0xE0B0, powerline::draw_e0b0),
    (0xE0B1, 0xE0B1, powerline::draw_e0b1),
    (0xE0B2, 0xE0B2, powerline::draw_e0b2),
    (0xE0B3, 0xE0B3, powerline::draw_e0b3),
    (0xE0B4, 0xE0B4, powerline::draw_e0b4),
    (0xE0B5, 0xE0B5, powerline::draw_e0b5),
    (0xE0B6, 0xE0B6, powerline::draw_e0b6),
    (0xE0B7, 0xE0B7, powerline::draw_e0b7),
    (0xE0B8, 0xE0B8, powerline::draw_e0b8),
    (0xE0B9, 0xE0B9, powerline::draw_e0b9),
    (0xE0BA, 0xE0BA, powerline::draw_e0ba),
    (0xE0BB, 0xE0BB, powerline::draw_e0bb),
    (0xE0BC, 0xE0BC, powerline::draw_e0bc),
    (0xE0BD, 0xE0BD, powerline::draw_e0bd),
    (0xE0BE, 0xE0BE, powerline::draw_e0be),
    (0xE0BF, 0xE0BF, powerline::draw_e0bf),
    (0xE0D2, 0xE0D2, powerline::draw_e0d2),
    (0xE0D4, 0xE0D4, powerline::draw_e0d4),
    (0xF5D0, 0xF60D, branch::draw_f5d0_f60d),
    (
        0x1CC1B,
        0x1CC1E,
        legacy_computing_supplement::draw_1cc1b_1cc1e,
    ),
    (
        0x1CC21,
        0x1CC2F,
        legacy_computing_supplement::draw_1cc21_1cc2f,
    ),
    (
        0x1CC30,
        0x1CC3F,
        legacy_computing_supplement::draw_1cc30_1cc3f,
    ),
    (
        0x1CD00,
        0x1CDE5,
        legacy_computing_supplement::draw_1cd00_1cde5,
    ),
    (0x1CE00, 0x1CE00, legacy_computing_supplement::draw_1ce00),
    (0x1CE01, 0x1CE01, legacy_computing_supplement::draw_1ce01),
    (0x1CE0B, 0x1CE0B, legacy_computing_supplement::draw_1ce0b),
    (0x1CE0C, 0x1CE0C, legacy_computing_supplement::draw_1ce0c),
    (
        0x1CE16,
        0x1CE19,
        legacy_computing_supplement::draw_1ce16_1ce19,
    ),
    (
        0x1CE51,
        0x1CE8F,
        legacy_computing_supplement::draw_1ce51_1ce8f,
    ),
    (
        0x1CE90,
        0x1CEAF,
        legacy_computing_supplement::draw_1ce90_1ceaf,
    ),
    (0x1FB00, 0x1FB3B, legacy_computing::draw_1fb00_1fb3b),
    (0x1FB3C, 0x1FB67, legacy_computing::draw_1fb3c_1fb67),
    (0x1FB68, 0x1FB6F, legacy_computing::draw_1fb68_1fb6f),
    (0x1FB70, 0x1FB75, legacy_computing::draw_1fb70_1fb75),
    (0x1FB76, 0x1FB7B, legacy_computing::draw_1fb76_1fb7b),
    (0x1FB7C, 0x1FB97, legacy_computing::draw_1fb7c_1fb97),
    (0x1FB98, 0x1FB98, legacy_computing::draw_1fb98),
    (0x1FB99, 0x1FB99, legacy_computing::draw_1fb99),
    (0x1FB9A, 0x1FB9F, legacy_computing::draw_1fb9a_1fb9f),
    (0x1FBA0, 0x1FBAE, legacy_computing::draw_1fba0_1fbae),
    (0x1FBAF, 0x1FBAF, legacy_computing::draw_1fbaf),
    (0x1FBBD, 0x1FBBD, legacy_computing::draw_1fbbd),
    (0x1FBBE, 0x1FBBE, legacy_computing::draw_1fbbe),
    (0x1FBBF, 0x1FBBF, legacy_computing::draw_1fbbf),
    (0x1FBCE, 0x1FBCE, legacy_computing::draw_1fbce),
    (0x1FBCF, 0x1FBCF, legacy_computing::draw_1fbcf),
    (0x1FBD0, 0x1FBDF, legacy_computing::draw_1fbd0_1fbdf),
    (0x1FBE0, 0x1FBEF, legacy_computing::draw_1fbe0_1fbef),
];

fn draw_fn(cp: u32) -> Option<DrawFn> {
    let ix = RANGES.partition_point(|&(_, max, _)| max < cp);
    RANGES
        .get(ix)
        .filter(|&&(min, _, _)| min <= cp)
        .map(|&(_, _, f)| f)
}

/// Whether `ch` is drawn here rather than by the font.
pub fn has_char(ch: char) -> bool {
    draw_fn(ch as u32).is_some()
}

/// A drawn glyph: coverage for a surface larger than the cell by `padding`
/// on every side, so its top-left sits at (-padding_x, -padding_y).
pub struct Sprite {
    pub width: u32,
    pub height: u32,
    pub padding_x: u32,
    pub padding_y: u32,
    pub alpha: Vec<u8>,
}

/// Draw `ch` for a `width`×`height` sprite (`width` spans every cell the
/// glyph covers).
pub fn render(ch: char, width: u32, height: u32, metrics: &Metrics) -> Option<Sprite> {
    let draw = draw_fn(ch as u32)?;
    let (padding_x, padding_y) = (width / 4, height / 4);
    let mut canvas = Canvas::new(width, height, padding_x, padding_y);
    draw(ch as u32, &mut canvas, width, height, metrics);
    let (w, h) = (canvas.surface_width(), canvas.surface_height());
    Some(Sprite {
        width: w,
        height: h,
        padding_x,
        padding_y,
        alpha: canvas.into_alpha(),
    })
}
