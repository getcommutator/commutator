//! Width-dependent presentation; selection offsets always refer to original text.
use super::render::FlatText;
use gpui::{SharedString, TextRun};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Maps offsets between a stage's input text and its shown text. `omissions`
/// pairs `(input range, shown range)` in order; the shown span can be SHORTER
/// (link truncation) or LONGER (a reserved glyph slot). Stages chain
/// through `prior`: a slot insertion map sits under a truncation map so
/// displayed coordinates resolve all the way back to the source text.
#[derive(Clone, Debug, Default)]
pub struct OffsetMap {
    pub omissions: Vec<(Range<usize>, Range<usize>)>,
    /// The previous stage's map (closer to the source text), if any.
    pub prior: Option<Box<OffsetMap>>,
}
impl OffsetMap {
    /// This stage only: input-text offset for a shown offset.
    fn local_original(&self, displayed: usize) -> usize {
        // The LAST overlapping-preceding omission's end delta already carries
        // every earlier omission's effect (the coordinate systems meet
        // there), so assigning — not summing — is the transitive shift.
        let mut shift: isize = 0;
        for (original, shown) in &self.omissions {
            if displayed < shown.start {
                break;
            }
            if displayed < shown.end {
                return original.start;
            }
            shift = original.end as isize - shown.end as isize;
        }
        displayed.saturating_add_signed(shift)
    }
    /// This stage only: shown offset for an input-text offset.
    fn local_displayed(&self, original: usize) -> usize {
        let mut shift: isize = 0;
        for (source, shown) in &self.omissions {
            if original < source.start {
                break;
            }
            if original < source.end {
                return shown.start;
            }
            shift = source.end as isize - shown.end as isize;
        }
        original.saturating_add_signed(-shift)
    }
    /// This stage only: shown range for an input range — an input range that
    /// overlaps a replaced span expands to cover the replacement.
    pub(crate) fn local_displayed_range(&self, range: Range<usize>) -> Range<usize> {
        let mut result = self.local_displayed(range.start)..self.local_displayed(range.end);
        for (source, shown) in &self.omissions {
            if range.start < source.end && range.end > source.start {
                result.start = result.start.min(shown.start);
                result.end = result.end.max(shown.end);
            }
        }
        result
    }
    /// Shown offset → source-text offset (all stages).
    pub fn original(&self, displayed: usize) -> usize {
        let local = self.local_original(displayed);
        self.prior
            .as_ref()
            .map_or(local, |prior| prior.original(local))
    }
    /// Source-text offset → shown offset (all stages).
    pub fn displayed(&self, original: usize) -> usize {
        let mid = self
            .prior
            .as_ref()
            .map_or(original, |prior| prior.displayed(original));
        self.local_displayed(mid)
    }
    /// Source-text range → shown range (all stages).
    pub fn displayed_range(&self, range: Range<usize>) -> Range<usize> {
        let mid = match &self.prior {
            Some(prior) => prior.displayed_range(range.clone()),
            None => range,
        };
        self.local_displayed_range(mid)
    }
}
#[derive(Clone)]
pub struct OriginalText {
    pub text: SharedString,
    pub offsets: OffsetMap,
}

pub(crate) fn slice_runs(runs: &[TextRun], range: Range<usize>) -> Vec<TextRun> {
    let mut at = 0;
    runs.iter()
        .filter_map(|run| {
            let start = at;
            at += run.len;
            let len = at.min(range.end).saturating_sub(start.max(range.start));
            (len > 0).then(|| {
                let mut run = run.clone();
                run.len = len;
                run
            })
        })
        .collect()
}

pub fn truncate(
    flat: &FlatText,
    width: f32,
    measure: impl Fn(&str, &[TextRun]) -> f32,
) -> FlatText {
    let mut omissions = Vec::new();
    for (range, url) in &flat.links {
        if super::links::LinkTarget::new("", url).navigation.is_err() {
            continue;
        }
        let label = &flat.text[range.clone()];
        let runs = slice_runs(&flat.runs, range.clone());
        if measure(label, &runs) <= width {
            continue;
        }
        // A cut never lands inside a formula's slot: the whole formula
        // shows or goes into the ellipsis.
        let boundaries: Vec<_> = label
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .filter(|&i| !inside_slot(flat, range.start + i))
            .collect();
        let candidate = |end: usize| {
            let text = format!("{}…", &label[..end]);
            let mut runs = slice_runs(&runs, 0..end);
            runs.push(ellipsis_run(flat, range.start + end..range.end));
            (text, runs)
        };
        let mut low = 0;
        let mut high = boundaries.len();
        while low < high {
            let middle = (low + high) / 2;
            let (text, runs) = candidate(boundaries[middle]);
            if measure(&text, &runs) <= width {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let prefix = boundaries[low.saturating_sub(1)];
        // Never make a short label longer just to show an ellipsis.
        if range.end - (range.start + prefix) > '…'.len_utf8() {
            omissions.push(range.start + prefix..range.end);
        }
    }
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut offsets = OffsetMap::default();
    let mut at = 0;
    for omitted in omissions {
        text.push_str(&flat.text[at..omitted.start]);
        runs.extend(slice_runs(&flat.runs, at..omitted.start));
        let start = text.len();
        text.push('…');
        runs.push(ellipsis_run(flat, omitted.clone()));
        at = omitted.end;
        offsets.omissions.push((omitted, start..text.len()));
    }
    text.push_str(&flat.text[at..]);
    runs.extend(slice_runs(&flat.runs, at..flat.text.len()));
    // This stage's ranges are in `flat.text` coordinates — map them locally;
    // `prior` then chains through any earlier substitution to the source.
    offsets.prior = flat
        .original
        .as_ref()
        .map(|original| Box::new(original.offsets.clone()));
    let shown = |range: &Range<usize>| offsets.local_displayed_range(range.clone());
    FlatText {
        text: text.into(),
        runs,
        links: flat
            .links
            .iter()
            .map(|(r, url)| (shown(r), url.clone()))
            .collect(),
        code_ranges: flat.code_ranges.iter().map(shown).collect(),
        file_glyphs: flat
            .file_glyphs
            .iter()
            .map(|(r, path)| (shown(r), path.clone()))
            .collect(),
        // A formula cut away with its label leaves no slot to paint over.
        math: flat
            .math
            .iter()
            .filter(|slot| {
                !offsets
                    .omissions
                    .iter()
                    .any(|(omitted, _)| omitted.contains(&slot.range.start))
            })
            .map(|slot| super::math::MathSlot {
                range: shown(&slot.range),
                ..slot.clone()
            })
            .collect(),
        original: Some(OriginalText {
            text: flat
                .original
                .as_ref()
                .map_or_else(|| flat.text.clone(), |original| original.text.clone()),
            offsets,
        }),
    }
}

/// Whether `at` falls strictly inside a formula's slot.
fn inside_slot(flat: &FlatText, at: usize) -> bool {
    // Slots are in text order and disjoint: the first ending past `at` is the
    // only one that can hold it (asked per character, so no scan).
    let i = flat.math.partition_point(|slot| slot.range.end <= at);
    flat.math
        .get(i)
        .is_some_and(|slot| slot.range.start < at && at < slot.range.end)
}

/// The run an ellipsis standing for `omitted` is styled with: the first
/// omitted text that isn't a formula's slot (whose pinned font and missing
/// underline are the slot's own), else the first omitted run.
fn ellipsis_run(flat: &FlatText, omitted: Range<usize>) -> TextRun {
    // Slots are in text order, so adjacent ones chain.
    let mut at = omitted.start;
    for slot in &flat.math {
        if slot.range.start == at {
            at = slot.range.end;
        }
    }
    let style = if at < omitted.end { at } else { omitted.start };
    let mut run = slice_runs(&flat.runs, style..omitted.end).remove(0);
    run.len = '…'.len_utf8();
    run
}

use super::render::{INLINE_CODE_INSET_Y, RenderOptions, range_rects};
use crate::theme::Theme;
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, Pixels, Size, TextLayout, Window, div, point, prelude::*, px,
    size,
};
use std::rc::Rc;

pub struct ResponsiveText {
    pub flat: Rc<FlatText>,
    pub opts: RenderOptions,
    pub theme: Theme,
    pub ix: usize,
}
pub(super) fn present(
    flat: &FlatText,
    width: Pixels,
    font_size: Pixels,
    window: &Window,
    opts: &RenderOptions,
) -> FlatText {
    let (padded, pads) = with_code_pads(with_file_link_glyphs(flat, opts));
    // A pad's NBSP advance per point of its (monospace) face.
    let pad_advance = pads.first().and_then(|pad| {
        let run = slice_runs(&padded.runs, pad.clone()).into_iter().next()?;
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&run.font);
        let advance = f32::from(text_system.layout_width(font_id, px(100.0), '\u{00A0}')) / 100.0;
        (advance > 0.0).then_some(advance)
    });
    let with_glyphs = with_code_size(padded, &pads, pad_advance, font_size);
    truncate(&with_glyphs, f32::from(width), |text, runs| {
        window
            .text_system()
            .shape_text(text.to_owned().into(), font_size, runs, None, None)
            .map(|lines| {
                lines
                    .iter()
                    .map(|line| f32::from(line.size(font_size).width))
                    .fold(0., f32::max)
            })
            .unwrap_or(f32::INFINITY)
    })
}

/// Inline code a step under the text around it (13.5 on 15, as new Zeron's),
/// so a `name` in a sentence doesn't tower over it in a monospace face.
pub(crate) const INLINE_CODE_SCALE: f32 = 0.9;

/// Set the runs inside inline code at [`INLINE_CODE_SCALE`] of `font_size`
/// (the paragraph's, known only at layout), and its `pads` at the size that
/// makes their NBSP ([`CODE_PAD_EMS`] of the code) as wide, given its
/// advance per point. The line keeps its height.
fn with_code_size(
    mut flat: FlatText,
    pads: &[Range<usize>],
    pad_advance: Option<f32>,
    font_size: Pixels,
) -> FlatText {
    if flat.code_ranges.is_empty() {
        return flat;
    }
    // Quarter-point steps keep glyph rasters shared across nearby sizes.
    let quarter = |size: f32| px((size * 4.0).round() / 4.0);
    let code_size = f32::from(font_size) * INLINE_CODE_SCALE;
    let pad_size = pad_advance.map(|advance| quarter(CODE_PAD_EMS * code_size / advance));
    let mut at = 0;
    for run in &mut flat.runs {
        let range = at..at + run.len;
        at = range.end;
        if pads.contains(&range) {
            run.font.size = pad_size.or(Some(quarter(code_size)));
        } else if flat
            .code_ranges
            .iter()
            .any(|code| code.start <= range.start && range.end <= code.end)
        {
            run.font.size = Some(quarter(code_size));
        }
    }
    flat
}

/// The slot reserved before a resolved file link for its file-type icon (as
/// t3code's file chips): four NBSPs of text advance holding a 14px icon — a
/// reserved slot the shaped text never notices. Copy and selection map the
/// slot back to the link start, so the raw text survives.
const FILE_GLYPH_SLOT: &str = "\u{00A0}\u{00A0}\u{00A0}\u{00A0}";
pub(crate) const FILE_GLYPH_SIZE: f32 = 14.0;

/// Reserve a slot before every file link the surface's roots resolve. Web
/// links and targets no known root owns keep their shape, as does a
/// paragraph that is nothing but one file link — that renders as the sole
/// file row with its own icon tile.
fn with_file_link_glyphs(flat: &FlatText, opts: &RenderOptions) -> FlatText {
    let Some(ui) = opts.link.as_ref().filter(|ui| ui.file_roots.is_some()) else {
        return flat.clone();
    };
    if flat.links.is_empty() || paragraph_is_sole_file_link(flat, opts.workspace_root.as_deref()) {
        return flat.clone();
    }
    let (insertions, paths): (Vec<usize>, Vec<SharedString>) = flat
        .links
        .iter()
        .filter_map(|(range, target)| Some((range.start, ui.file_link(target)?.path.into())))
        .unzip();
    if insertions.is_empty() {
        return flat.clone();
    }
    let Some(slot_runs) = insertions
        .iter()
        .map(|at| slot_run(flat, *at, FILE_GLYPH_SLOT))
        .collect::<Option<Vec<_>>>()
    else {
        return flat.clone();
    };
    let (mut shown, slots) = insert_slots(flat, &insertions, &slot_runs, FILE_GLYPH_SLOT, false);
    shown.file_glyphs = slots.into_iter().zip(paths).collect();
    shown
}

/// `slot`'s run, styled as the text at `at` without its decorations.
fn slot_run(flat: &FlatText, at: usize, slot: &str) -> Option<TextRun> {
    let mut run = slice_runs(&flat.runs, at..flat.text.len())
        .into_iter()
        .next()
        .or_else(|| flat.runs.last().cloned())?;
    run.len = slot.len();
    run.underline = None;
    run.strikethrough = None;
    Some(run)
}

/// Insert `slot` at each of the ascending `insertions` (each in its run),
/// returning the shown text and the slots' ranges in it. Copy and selection
/// map a slot back to its insertion point, so the raw text survives. A
/// range edge on an insertion point keeps the slot out, except inline
/// code's when `code_takes_slots` (its padding sits inside its wash).
fn insert_slots(
    flat: &FlatText,
    insertions: &[usize],
    slot_runs: &[TextRun],
    slot: &str,
    code_takes_slots: bool,
) -> (FlatText, Vec<Range<usize>>) {
    // Positions after an insertion slide by the slot length; an insertion
    // point itself resolves to its "before" side so a range ending on it
    // never swallows its own slot.
    let shift_before =
        |offset: usize| insertions.iter().filter(|at| **at < offset).count() * slot.len();
    let shift_through =
        |offset: usize| insertions.iter().filter(|at| **at <= offset).count() * slot.len();
    let map_range = |range: &Range<usize>| {
        range.start + shift_through(range.start)..range.end + shift_before(range.end)
    };
    let map_code = |range: &Range<usize>| {
        if code_takes_slots {
            range.start + shift_before(range.start)..range.end + shift_through(range.end)
        } else {
            map_range(range)
        }
    };

    let mut text = String::with_capacity(flat.text.len() + insertions.len() * slot.len());
    let mut runs: Vec<TextRun> = Vec::new();
    let mut offsets = OffsetMap::default();
    let mut slots = Vec::with_capacity(insertions.len());
    let mut at = 0;
    for (ix, insert_at) in insertions.iter().enumerate() {
        text.push_str(&flat.text[at..*insert_at]);
        runs.extend(slice_runs(&flat.runs, at..*insert_at));
        let start = text.len();
        text.push_str(slot);
        runs.push(slot_runs[ix].clone());
        offsets
            .omissions
            .push((*insert_at..*insert_at, start..text.len()));
        slots.push(start..text.len());
        at = *insert_at;
    }
    text.push_str(&flat.text[at..]);
    runs.extend(slice_runs(&flat.runs, at..flat.text.len()));
    offsets.prior = flat
        .original
        .as_ref()
        .map(|original| Box::new(original.offsets.clone()));
    let shown = FlatText {
        text: text.into(),
        runs,
        links: flat
            .links
            .iter()
            .map(|(range, url)| (map_range(range), url.clone()))
            .collect(),
        code_ranges: flat.code_ranges.iter().map(map_code).collect(),
        file_glyphs: flat
            .file_glyphs
            .iter()
            .map(|(range, path)| (map_range(range), path.clone()))
            .collect(),
        math: flat
            .math
            .iter()
            .map(|slot| super::math::MathSlot {
                range: map_range(&slot.range),
                ..slot.clone()
            })
            .collect(),
        original: Some(OriginalText {
            text: flat
                .original
                .as_ref()
                .map_or_else(|| flat.text.clone(), |original| original.text.clone()),
            offsets,
        }),
    };
    (shown, slots)
}

/// Room inside inline code's wash at each end, as t3code's `0.35em`
/// padding: one NBSP each side, sized at layout to that width.
const CODE_PAD_SLOT: &str = "\u{00A0}";
const CODE_PAD_EMS: f32 = 0.35;

/// Reserve a padding slot at both ends of every inline code span.
fn with_code_pads(flat: FlatText) -> (FlatText, Vec<Range<usize>>) {
    let mut insertions = Vec::new();
    let mut slot_runs = Vec::new();
    for code in flat.code_ranges.iter().filter(|code| !code.is_empty()) {
        let (Some(first), Some(last)) = (
            slot_run(&flat, code.start, CODE_PAD_SLOT),
            slot_run(&flat, code.end - 1, CODE_PAD_SLOT),
        ) else {
            return (flat, Vec::new());
        };
        insertions.extend([code.start, code.end]);
        slot_runs.extend([first, last]);
    }
    if insertions.is_empty() {
        return (flat, Vec::new());
    }
    insert_slots(&flat, &insertions, &slot_runs, CODE_PAD_SLOT, true)
}

/// Whether the whole paragraph is one workspace file link: it renders as the
/// sole-file row (file icon tile), which carries no trailing glyph.
fn paragraph_is_sole_file_link(flat: &FlatText, workspace_root: Option<&str>) -> bool {
    let Some(root) = workspace_root else {
        return false;
    };
    let [(range, target)] = flat.links.as_slice() else {
        return false;
    };
    flat.text[..range.start].trim().is_empty()
        && flat.text[range.end..].trim().is_empty()
        && crate::workspace_links::resolve_workspace_file_link(target, root).is_some()
}
impl IntoElement for ResponsiveText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for ResponsiveText {
    type RequestLayoutState = ();
    type PrepaintState = AnyElement;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, ()) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(
            style
                .line_height
                .to_pixels(font_size.into(), window.rem_size()),
        );
        let flat = self.flat.clone();
        let opts = self.opts.clone();
        let id = window.request_measured_layout(
            Default::default(),
            move |known, available, window, _| {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let shown = width.map(|width| present(&flat, width, font_size, window, &opts));
                let flat = shown.as_ref().unwrap_or(&flat);
                let lines = window
                    .text_system()
                    .shape_text(flat.text.clone(), font_size, &flat.runs, width, None)
                    .unwrap_or_default();
                let mut result: Size<Pixels> = Size::default();
                for line in lines.iter() {
                    let size = line.size(line_height);
                    result.width = result.width.max(size.width).ceil();
                    result.height += size.height;
                }
                // Keep the final width equal to the one that decided truncation.
                if let Some(width) = width {
                    result.width = width;
                }
                result
            },
        );
        (id, ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let flat = present(&self.flat, bounds.size.width, font_size, window, &self.opts);
        let mut child =
            super::render::flat_text_presented_element(&flat, self.ix, &self.opts, &self.theme);
        child.prepaint_as_root(
            bounds.origin,
            bounds.size.map(AvailableSpace::Definite),
            window,
            cx,
        );
        child
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        child: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        child.paint(window, cx);
    }
}

/// Paints the trailing open glyph of every resolved file link over the
/// shaped text — the same reserved-slot overlay pattern, so the NBSP slots
/// keep layout, selection and copy stable.
pub(crate) struct FileLinkGlyphs {
    pub id: SharedString,
    pub child: AnyElement,
    pub layout: TextLayout,
    pub slots: Vec<(Range<usize>, SharedString)>,
}

impl IntoElement for FileLinkGlyphs {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for FileLinkGlyphs {
    type RequestLayoutState = ();
    type PrepaintState = Vec<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone().into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        self.child.prepaint(window, cx);
        let theme = Theme::of(cx).clone();
        let mut overlays = Vec::new();
        for (slot, path) in &self.slots {
            let Some(rect) = range_rects(&self.layout, slot, 0.0, INLINE_CODE_INSET_Y)
                .first()
                .copied()
            else {
                continue;
            };
            let mut icon_el = div()
                .w(px(FILE_GLYPH_SIZE))
                .h(rect.size.height)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    crate::file_icons::icon(
                        crate::file_icons::FileIconIdentity::file(path),
                        theme.appearance,
                    )
                    .size(px(FILE_GLYPH_SIZE)),
                )
                .into_any_element();
            icon_el.prepaint_as_root(
                point(rect.origin.x, rect.origin.y),
                size(px(FILE_GLYPH_SIZE), rect.size.height).map(AvailableSpace::Definite),
                window,
                cx,
            );
            overlays.push(icon_el);
        }
        overlays
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        overlays: &mut Vec<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
        for overlay in overlays {
            overlay.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        parser::{InlineRun, InlineStyle},
        render::flatten_runs,
    };
    use super::*;
    fn link(text: &str) -> InlineRun {
        InlineRun {
            text: text.into(),
            style: InlineStyle {
                link: Some("https://example.com/destination".into()),
                ..Default::default()
            },
        }
    }
    fn measured(text: &str, _: &[TextRun]) -> f32 {
        text.graphemes(true).count() as f32
    }

    #[test]
    fn inline_code_runs_take_a_step_under_the_paragraph_size() {
        let run = |text: &str, code: bool| InlineRun {
            text: text.into(),
            style: InlineStyle {
                code,
                ..Default::default()
            },
        };
        let flat = flatten_runs(
            &[run("run ", false), run("cargo check", true), run(" now", false)],
            &Theme::dark(),
            false,
        );
        let sized = with_code_size(flat.clone(), &[], None, px(15.0));
        let sizes: Vec<_> = sized.runs.iter().map(|r| r.font.size).collect();
        assert_eq!(sizes, vec![None, Some(px(13.5)), None]);

        // Padded: a NBSP slot each side, inside the code's wash, sized so
        // it is 0.35em of the code wide (an NBSP 0.5em of its face here),
        // and copy still reads the source.
        let (padded, pads) = with_code_pads(flat);
        assert_eq!(padded.text.as_ref(), "run \u{a0}cargo check\u{a0} now");
        assert_eq!(pads.len(), 2);
        assert_eq!(padded.code_ranges, vec![pads[0].start..pads[1].end]);
        let map = &padded.original.as_ref().unwrap().offsets;
        assert_eq!(map.original(pads[0].end), 4);
        assert_eq!(map.original(pads[1].end), 15);
        let sized = with_code_size(padded, &pads, Some(0.5), px(15.0));
        let sizes: Vec<_> = sized.runs.iter().map(|r| r.font.size).collect();
        assert_eq!(
            sizes,
            vec![None, Some(px(9.5)), Some(px(13.5)), Some(px(9.5)), None]
        );
    }

    #[test]
    fn file_link_glyphs_reserve_a_slot_only_for_resolved_files() {
        use super::super::render::{LinkOutcome, LinkUi};
        let run = |text: &str, link: Option<&str>| InlineRun {
            text: text.into(),
            style: InlineStyle {
                link: link.map(str::to_owned),
                ..Default::default()
            },
        };
        let flat = flatten_runs(
            &[
                run("see ", None),
                run("src/lib.rs", Some("src/lib.rs")),
                run(" and ", None),
                run("docs", Some("https://example.com/x")),
                run(" and ", None),
                run("elsewhere.rs", Some("../elsewhere.rs")),
            ],
            &Theme::dark(),
            false,
        );
        let mut opts = RenderOptions::settled("row".into());
        opts.workspace_root = Some("/repo".into());
        opts.link = Some(LinkUi {
            source_session: Some("chat".into()),
            source_local: true,
            file_roots: Some(Rc::new(vec![crate::workspace_links::FileLinkRoot {
                chat: Some("chat".into()),
                root: "/repo".into(),
                local: true,
            }])),
            handler: Rc::new(|_, _, _| LinkOutcome::Rejected),
        });
        let shown = with_file_link_glyphs(&flat, &opts);
        // Only the resolved file link grows a slot: the web link and the
        // escaped target keep their shape.
        assert_eq!(shown.file_glyphs.len(), 1);
        assert_eq!(&shown.text[shown.file_glyphs[0].0.clone()], FILE_GLYPH_SLOT);
        assert_eq!(shown.file_glyphs[0].1.as_ref(), "src/lib.rs");
        assert_eq!(shown.links.len(), 3);
        // The reserved slot sits right before the link and never reaches copy
        // or selection — both slot ends map back to the link's start.
        let slot = &shown.file_glyphs[0].0;
        assert_eq!(slot.end, shown.links[0].0.start);
        let map = &shown.original.as_ref().unwrap().offsets;
        assert_eq!(map.original(slot.start), map.original(slot.end));
        assert_eq!(map.original(slot.start), map.original(slot.start - 1) + 1);

        // A paragraph that is nothing but one file link stays the sole-file
        // row (icon tile), never the trailing glyph.
        let sole = flatten_runs(
            &[run("src/lib.rs", Some("src/lib.rs"))],
            &Theme::dark(),
            false,
        );
        assert!(with_file_link_glyphs(&sole, &opts).file_glyphs.is_empty());

        // Without a root snapshot no link grows a glyph (previews, web-only
        // surfaces).
        let mut no_roots = opts.clone();
        no_roots.link = Some(LinkUi {
            source_session: Some("chat".into()),
            source_local: false,
            file_roots: None,
            handler: Rc::new(|_, _, _| LinkOutcome::Rejected),
        });
        assert!(
            with_file_link_glyphs(&flat, &no_roots)
                .file_glyphs
                .is_empty()
        );
    }
    #[test]
    fn truncation_preserves_graphemes_destinations_and_original_selection() {
        let label = "https://example.com/á🙂👨‍👩‍👧‍👦界/very/long/path";
        let flat = flatten_runs(&[link(label)], &Theme::dark(), false);
        for width in [1., 12., 25., 32.] {
            let shown = truncate(&flat, width, measured);
            assert!(measured(&shown.text, &[]) <= width);
            let original = shown.original.as_ref().unwrap();
            assert_eq!(original.text.as_ref(), label);
            assert_eq!(original.offsets.original(shown.text.len()), label.len());
            assert_eq!(shown.links[0].1, flat.links[0].1);
            assert_eq!(
                shown.runs.iter().map(|r| r.len).sum::<usize>(),
                shown.text.len()
            );
            for (i, _) in shown.text.char_indices() {
                assert!(label.is_char_boundary(original.offsets.original(i)));
            }
        }
        let shown = truncate(&flat, 25., measured);
        assert!(shown.text.starts_with("https://example.com/"));
    }
    #[test]
    fn partial_and_cross_block_copies_map_back_to_source() {
        let source = "https://example.com/abcdefghijklmnopqrstuvwxyz";
        let flat = flatten_runs(
            &[
                link(source),
                InlineRun {
                    text: " between ".into(),
                    style: Default::default(),
                },
                link("https://second.example/long/path/to/resource"),
            ],
            &Theme::dark(),
            false,
        );
        let shown = truncate(&flat, 24., measured);
        let map = &shown.original.as_ref().unwrap().offsets;
        assert_eq!(map.omissions.len(), 2);
        let first = &shown.links[0].0;
        assert_eq!(
            &flat.text[map.original(first.start)..map.original(first.end)],
            source
        );
        assert_eq!(&flat.text[map.original(0)..map.original(5)], "https");
        let end = map.original(shown.text.len());
        let spans = super::super::selection::resolve_spans(
            &[("a", &flat.text), ("b", "next block")],
            (0, 0),
            (1, 4),
        );
        let copied = spans
            .iter()
            .map(|span| &span.text[span.range.clone()])
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(copied, format!("{}\nnext", &flat.text[..end]));
        let omission = &map.omissions[0];
        assert_eq!(
            map.displayed_range(omission.0.start + 1..omission.0.end - 1),
            omission.1
        );
    }
    #[test]
    fn widening_and_streaming_recompute_presentation_without_stale_offsets() {
        let flat = flatten_runs(
            &[link("https://example.com/a/long/link")],
            &Theme::dark(),
            false,
        );
        let narrow = truncate(&flat, 15., measured);
        let wide = truncate(&flat, 1000., measured);
        assert!(narrow.text.ends_with('…'));
        assert_eq!(wide.text, flat.text);
        assert!(wide.original.as_ref().unwrap().offsets.omissions.is_empty());
        let growing = flatten_runs(
            &[link("https://example.com/a/long/link/streaming-tail")],
            &Theme::dark(),
            false,
        );
        let next = truncate(&growing, 15., measured);
        assert_eq!(
            next.original.unwrap().offsets.original(next.text.len()),
            growing.text.len()
        );
    }
    #[test]
    fn styled_link_runs_are_trimmed_without_corrupting_offsets() {
        let mut bold = link("very-long-bold-suffix");
        bold.style.bold = true;
        let flat = flatten_runs(&[link("https://example.com/"), bold], &Theme::dark(), false);
        let shown = truncate(&flat, 24., measured);
        assert_eq!(shown.links.len(), 1);
        assert!(shown.runs.last().unwrap().font.weight.0 >= gpui::FontWeight::SEMIBOLD.0);
        assert_eq!(
            shown.runs.iter().map(|r| r.len).sum::<usize>(),
            shown.text.len()
        );
    }
}
