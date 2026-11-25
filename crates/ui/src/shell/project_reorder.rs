//! Dragging a project, with its worktrees, to a new place in the Projects
//! sidebar, as Shepherd's sidebar reorders its workspaces: the row lifts and
//! follows the pointer up and down the list (never out of it), a placeholder
//! holds the slot it would land in, the other projects spring around it, and
//! let go of, it flies into its slot. An unfolded project folds while it is
//! dragged and unfolds where it lands.
use super::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// A project springing to a new slot (Shepherd's `reorder::SLIDE`).
const SLIDE: Duration = Duration::from_millis(380);
/// How long a dropped order is shown, at most, before the list's own is it.
const PENDING: Duration = Duration::from_millis(1500);

/// Shepherd's spring: damped by 0.7, a little past the end and back.
fn spring(t: f32) -> f32 {
    if t >= 1.0 {
        return 1.0;
    }
    let (zeta, omega) = (0.7f32, 14f32);
    let damped = omega * (1.0 - zeta * zeta).sqrt();
    1.0 - (-zeta * omega * t).exp()
        * ((damped * t).cos() + zeta * omega / damped * (damped * t).sin())
}

#[derive(Clone, Copy, Debug)]
struct Slide {
    from: f32,
    to: f32,
    at: Instant,
}

impl Slide {
    fn now(&self, now: Instant, over: Duration) -> f32 {
        if over.is_zero() {
            return self.to;
        }
        let t = now.saturating_duration_since(self.at).as_secs_f32() / over.as_secs_f32();
        self.from + (self.to - self.from) * spring(t)
    }

    fn done(&self, now: Instant, over: Duration) -> bool {
        now.saturating_duration_since(self.at) >= over
    }
}

/// What gpui drags. The sidebar draws the row itself, held to its column.
#[derive(Clone)]
pub(super) struct DraggedProject {
    list: Rc<RefCell<ProjectReorder>>,
    key: String,
}

impl Render for DraggedProject {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The project being dragged: where the pointer holds its row from the row's
/// top, the slot its placeholder holds, its top in the list, and whether it
/// was folded for the drag (so it unfolds again where it lands).
#[derive(Debug)]
struct Drag {
    key: String,
    grab: f32,
    slot: usize,
    top: f32,
    folded: bool,
    unfold: bool,
}

/// A drag let go of: the project, whether it unfolds, and the order it asks
/// for when it moved.
#[derive(Debug, PartialEq)]
pub(super) struct Dropped {
    pub key: String,
    pub unfold: bool,
    pub order: Option<Vec<String>>,
}

#[derive(Default)]
pub(super) struct ProjectReorder {
    /// The list's left edge and top in the window, its width, and the
    /// pointer, as last seen.
    origin: gpui::Point<Pixels>,
    width: f32,
    pointer_y: f32,
    /// Each project's top in the list as last laid out, before any slide.
    tops: HashMap<String, f32>,
    /// Each project's top on its way to its slot, and its place in the order
    /// drawn.
    slides: HashMap<String, (Slide, usize)>,
    drag: Option<Drag>,
    /// The order a drop asked for, shown until the list's own is it.
    pending: Option<(Vec<String>, Instant)>,
    /// The project let go of, flying into its slot.
    landing: Option<String>,
}

/// A frame of the list: each project's offset from where the list lays it
/// out, the lifted one's window position, whether anything still moves, and
/// a drag that ended just now.
pub(super) struct Frame {
    pub offsets: Vec<f32>,
    pub lifted: Option<(usize, gpui::Point<Pixels>)>,
    pub moving: bool,
    pub dropped: Option<Dropped>,
}

impl ProjectReorder {
    /// Anything under way: the shell's own resort glide stays out of it.
    pub(super) fn busy(&self, now: Instant) -> bool {
        self.drag.is_some()
            || self.landing.is_some()
            || self.pending.is_some()
            || self.slides.values().any(|(slide, _)| !slide.done(now, SLIDE))
    }

    /// The dragged project, once, to fold for the drag; `was_open` says
    /// whether it was unfolded.
    pub(super) fn take_fold(&mut self) -> Option<String> {
        let drag = self.drag.as_mut().filter(|drag| !drag.folded)?;
        drag.folded = true;
        Some(drag.key.clone())
    }

    pub(super) fn set_unfold(&mut self, unfold: bool) {
        if let Some(drag) = self.drag.as_mut() {
            drag.unfold = unfold;
        }
    }

    /// The projects `blocks` (key, height; `gap` apart, in the list's order)
    /// as drawn now.
    pub(super) fn layout(
        &mut self,
        blocks: &[(String, f32)],
        gap: f32,
        dragging: bool,
        now: Instant,
        over: Duration,
    ) -> Frame {
        let height = |key: &str| {
            blocks
                .iter()
                .find(|(k, _)| k == key)
                .map_or(0.0, |(_, h)| *h)
        };
        let keys: Vec<String> = blocks.iter().map(|(k, _)| k.clone()).collect();
        let total = blocks.iter().map(|(_, h)| h).sum::<f32>()
            + gap * blocks.len().saturating_sub(1) as f32;
        self.tops.clear();
        let mut y = 0.0;
        for (key, h) in blocks {
            self.tops.insert(key.clone(), y);
            y += h + gap;
        }
        let mut dropped = None;
        let from = self
            .drag
            .as_ref()
            .and_then(|drag| keys.iter().position(|k| *k == drag.key));
        match (self.drag.as_mut(), from) {
            (Some(drag), Some(_)) if dragging => {
                let own = height(&drag.key);
                // Up and down its own list only.
                drag.top = (self.pointer_y - f32::from(self.origin.y) - drag.grab)
                    .clamp(0.0, (total - own).max(0.0));
                // The others laid out without it: it goes after each whose
                // middle is above its own.
                let mid = drag.top + own / 2.0;
                let (mut y, mut slot) = (0.0, 0);
                for (_, h) in blocks.iter().filter(|(k, _)| *k != drag.key) {
                    slot += usize::from(y + h / 2.0 < mid);
                    y += h + gap;
                }
                if drag.slot != slot {
                    crate::haptics::selection_step();
                }
                drag.slot = slot;
            }
            (Some(_), Some(from)) => {
                let drag = self.drag.take().expect("matched");
                let mut order: Vec<String> =
                    keys.iter().filter(|k| **k != drag.key).cloned().collect();
                order.insert(drag.slot.min(order.len()), drag.key.clone());
                let moved = drag.slot != from;
                if moved {
                    self.pending = Some((order.clone(), now));
                }
                // It flies from where it was let go.
                if let Some((slide, _)) = self.slides.get_mut(&drag.key) {
                    *slide = Slide {
                        from: drag.top,
                        to: slide.to,
                        at: now,
                    };
                }
                self.landing = Some(drag.key.clone());
                dropped = Some(Dropped {
                    key: drag.key,
                    unfold: drag.unfold,
                    order: moved.then_some(order),
                });
            }
            (Some(_), None) => {
                // Its project is gone from the list.
                let drag = self.drag.take().expect("matched");
                dropped = Some(Dropped {
                    key: drag.key,
                    unfold: drag.unfold,
                    order: None,
                });
            }
            (None, _) => {}
        }
        if let Some((order, at)) = &self.pending {
            let mut same: Vec<&String> = keys.iter().collect();
            same.sort();
            let mut asked: Vec<&String> = order.iter().collect();
            asked.sort();
            if *order == keys || same != asked || now.saturating_duration_since(*at) > PENDING {
                self.pending = None;
            }
        }
        let order: Vec<String> = match (&self.drag, &self.pending) {
            (Some(drag), _) => {
                let mut order: Vec<String> =
                    keys.iter().filter(|k| **k != drag.key).cloned().collect();
                order.insert(drag.slot.min(order.len()), drag.key.clone());
                order
            }
            (None, Some((order, _))) => order.clone(),
            (None, None) => keys.clone(),
        };
        // While a drag or a landing is under way a project that a fold above
        // it moved springs there too; otherwise it just moves along.
        let springy = self.drag.is_some() || self.landing.is_some();
        let mut moving = false;
        let mut y = 0.0;
        for (rank, key) in order.iter().enumerate() {
            let to = y;
            y += height(key) + gap;
            let (slide, was) = self.slides.entry(key.clone()).or_insert((
                Slide { from: to, to, at: now },
                rank,
            ));
            if *was != rank || (springy && slide.to != to) {
                *slide = Slide {
                    from: slide.now(now, over),
                    to,
                    at: now,
                };
                *was = rank;
            } else if slide.to != to {
                let by = to - slide.to;
                slide.from += by;
                slide.to = to;
            }
            moving |= !slide.done(now, over);
        }
        self.slides.retain(|key, _| keys.contains(key));
        if self.landing.as_ref().is_some_and(|key| {
            self.slides
                .get(key)
                .is_none_or(|(slide, _)| slide.done(now, over))
        }) {
            self.landing = None;
        }
        let lifted = self.drag.as_ref().and_then(|drag| {
            let ix = keys.iter().position(|k| *k == drag.key)?;
            Some((
                ix,
                gpui::point(self.origin.x, self.origin.y + px(drag.top)),
            ))
        });
        let offsets = keys
            .iter()
            .map(|key| {
                let natural = self.tops.get(key).copied().unwrap_or(0.0);
                self.slides
                    .get(key)
                    .map_or(0.0, |(slide, _)| slide.now(now, over) - natural)
            })
            .collect();
        Frame {
            offsets,
            lifted,
            moving: moving || self.drag.is_some(),
            dropped,
        }
    }
}

/// Makes a project row draggable, as key `key` of `list`, and has it measure
/// the list's width.
pub(super) fn draggable(
    row: gpui::Stateful<gpui::Div>,
    list: &Rc<RefCell<ProjectReorder>>,
    key: &str,
) -> gpui::Stateful<gpui::Div> {
    let dragged = DraggedProject {
        list: list.clone(),
        key: key.to_string(),
    };
    let measured = list.clone();
    let moved = list.clone();
    row.on_drag(dragged, |dragged, grab, window, cx| {
        let mut list = dragged.list.borrow_mut();
        let pointer = window.mouse_position();
        // The row's top is where the pointer is less where it holds it; the
        // list's is that less the row's place in it.
        let row_top = f32::from(pointer.y) - f32::from(grab.y);
        let in_list = list
            .slides
            .get(&dragged.key)
            .map_or_else(|| list.tops.get(&dragged.key).copied().unwrap_or(0.0), |(s, _)| s.to);
        list.origin = gpui::point(pointer.x - grab.x, px(row_top - in_list));
        list.pointer_y = f32::from(pointer.y);
        let slot = list
            .slides
            .get(&dragged.key)
            .map_or(0, |(_, rank)| *rank);
        list.drag = Some(Drag {
            key: dragged.key.clone(),
            grab: f32::from(grab.y),
            slot,
            top: in_list,
            folded: false,
            unfold: false,
        });
        drop(list);
        cx.new(|_| dragged.clone())
    })
    .on_drag_move::<DraggedProject>(move |event, window, _| {
        moved.borrow_mut().pointer_y = f32::from(event.event.position.y);
        window.refresh();
    })
    .child(
        gpui::canvas(
            move |bounds, _, _| measured.borrow_mut().width = f32::from(bounds.size.width),
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    )
}

impl Shell {
    /// Before the Projects rows are built: fold the project a drag just
    /// picked up, remembering to unfold it where it lands.
    pub(super) fn fold_dragged_project(&mut self) {
        let Some(key) = self.project_reorder.borrow_mut().take_fold() else {
            return;
        };
        let collapse_key = self.sidebar_group_collapse_key(&key);
        let was_open = self.sidebar_collapsed_groups.insert(collapse_key);
        self.project_reorder.borrow_mut().set_unfold(was_open);
    }

    /// The Projects rows (`rendered`, flat: each project's row then its
    /// worktrees') moved as a drag has them. `projects` are the top-level
    /// project keys; rows before the first one (pinned chats) stay put.
    pub(super) fn reorder_project_rows(
        &mut self,
        rendered: Vec<(String, f32, AnyElement)>,
        projects: &[String],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<(String, f32, AnyElement)> {
        let row_key = |key: &str| {
            key.strip_prefix("p:")
                .filter(|key| projects.iter().any(|p| p == key))
                .map(str::to_string)
        };
        // Each row's project block, from the first project on.
        let mut block_of: Vec<Option<usize>> = Vec::with_capacity(rendered.len());
        let mut blocks: Vec<(String, f32)> = Vec::new();
        for (key, height, _) in &rendered {
            if let Some(project) = row_key(key) {
                blocks.push((project, *height));
            } else if let Some(block) = blocks.last_mut() {
                block.1 += SIDEBAR_LIST_GAP + height;
            }
            block_of.push((!blocks.is_empty()).then(|| blocks.len() - 1));
        }
        let now = self.render_time.unwrap_or_else(Instant::now);
        let over = if self.reduced_motion {
            Duration::ZERO
        } else {
            SLIDE
        };
        let frame =
            self.project_reorder
                .borrow_mut()
                .layout(&blocks, SIDEBAR_LIST_GAP, cx.has_active_drag(), now, over);
        if frame.moving {
            self.motion_active.set(true);
        }
        if let Some(dropped) = frame.dropped {
            if dropped.unfold {
                let collapse_key = self.sidebar_group_collapse_key(&dropped.key);
                self.sidebar_collapsed_groups.remove(&collapse_key);
            }
            if let Some(order) = dropped.order {
                // Projects not listed now (no chats) keep their places after.
                let rest: Vec<String> = self
                    .settings
                    .space_order
                    .iter()
                    .filter(|key| !order.contains(key))
                    .cloned()
                    .collect();
                self.settings.space_order = order.into_iter().chain(rest).collect();
                self.schedule_save(cx);
            }
            self.motion_active.set(true);
            cx.notify();
        }
        let width = self.project_reorder.borrow().width;
        rendered
            .into_iter()
            .zip(block_of)
            .filter_map(|((key, height, element), block)| {
                let Some(block) = block else {
                    return Some((key, height, element));
                };
                match frame.lifted {
                    // The lifted project's row is drawn at the pointer, a
                    // placeholder of its block's height holding its slot;
                    // anything else of it is folding away.
                    Some((lifted, at)) if lifted == block => {
                        if row_key(&key).is_none() {
                            return None;
                        }
                        let placeholder = div()
                            .relative()
                            .h(px(blocks[block].1))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(theme.border)
                            .bg(crate::theme::wash(0.04))
                            .child(
                                gpui::deferred(
                                    gpui::anchored().position(at).child(
                                        div()
                                            .w(px(width))
                                            .rounded(px(8.0))
                                            .bg(theme.surface)
                                            .shadow_lg()
                                            .text_color(theme.text)
                                            .font_family(theme.font_sans.clone())
                                            .child(element),
                                    ),
                                )
                                .with_priority(1),
                            )
                            .into_any_element();
                        Some((key, blocks[block].1, placeholder))
                    }
                    _ => {
                        let offset = frame.offsets[block];
                        let element = if offset == 0.0 {
                            element
                        } else {
                            div()
                                .relative()
                                .top(px(offset))
                                .child(element)
                                .into_any_element()
                        };
                        Some((key, height, element))
                    }
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(blocks: &[(&str, f32)]) -> (ProjectReorder, Vec<(String, f32)>) {
        let mut reorder = ProjectReorder {
            origin: gpui::point(px(0.0), px(100.0)),
            ..Default::default()
        };
        let blocks: Vec<(String, f32)> = blocks.iter().map(|(k, h)| (k.to_string(), *h)).collect();
        reorder.layout(&blocks, 2.0, false, Instant::now(), SLIDE);
        (reorder, blocks)
    }

    fn drag(reorder: &mut ProjectReorder, key: &str, grab: f32) {
        reorder.drag = Some(Drag {
            key: key.into(),
            grab,
            slot: 0,
            top: 0.0,
            folded: true,
            unfold: true,
        });
    }

    #[test]
    fn a_project_dragged_past_the_next_ones_middle_takes_its_slot_and_moves_there_when_let_go() {
        // b is an unfolded project, taller with its worktrees.
        let (mut r, blocks) = list(&[("a", 32.0), ("b", 100.0), ("c", 32.0)]);
        let now = Instant::now();
        drag(&mut r, "a", 10.0);
        // 20 px down, a's middle (36) is above b's (50): it stays.
        r.pointer_y = 130.0;
        let frame = r.layout(&blocks, 2.0, true, now, SLIDE);
        assert_eq!(r.drag.as_ref().map(|d| d.slot), Some(0));
        assert_eq!(frame.lifted, Some((0, gpui::point(px(0.0), px(120.0)))));
        // 40 px down, past b's middle: after b, which springs up.
        r.pointer_y = 150.0;
        r.layout(&blocks, 2.0, true, now, SLIDE);
        assert_eq!(r.drag.as_ref().map(|d| d.slot), Some(1));
        let frame = r.layout(&blocks, 2.0, true, now + SLIDE, SLIDE);
        assert_eq!(frame.offsets[1], -34.0);
        // Let go: the order asked for, the project unfolding, and the order
        // held until the list's own is it.
        let frame = r.layout(&blocks, 2.0, false, now + SLIDE, SLIDE);
        let moved = ["b", "a", "c"].map(String::from).to_vec();
        assert_eq!(
            frame.dropped,
            Some(Dropped {
                key: "a".into(),
                unfold: true,
                order: Some(moved.clone()),
            })
        );
        let reordered: Vec<(String, f32)> = vec![
            ("b".into(), 100.0),
            ("a".into(), 32.0),
            ("c".into(), 32.0),
        ];
        let frame = r.layout(&reordered, 2.0, false, now + SLIDE * 3, SLIDE);
        assert!(frame.offsets.iter().all(|o| *o == 0.0) && !frame.moving);
        assert!(!r.busy(now + SLIDE * 3));
    }

    #[test]
    fn the_lifted_row_stays_inside_its_list() {
        let (mut r, blocks) = list(&[("a", 32.0), ("b", 32.0)]);
        drag(&mut r, "a", 10.0);
        r.pointer_y = -500.0;
        let frame = r.layout(&blocks, 2.0, true, Instant::now(), SLIDE);
        assert_eq!(frame.lifted.map(|(_, at)| at.y), Some(px(100.0)));
        r.pointer_y = 5000.0;
        let frame = r.layout(&blocks, 2.0, true, Instant::now(), SLIDE);
        // The list is 66 px tall; a's 32 px row tops out at 34.
        assert_eq!(frame.lifted.map(|(_, at)| at.y), Some(px(134.0)));
    }

    #[test]
    fn a_drop_in_place_asks_for_no_order() {
        let (mut r, blocks) = list(&[("a", 32.0), ("b", 32.0)]);
        drag(&mut r, "b", 10.0);
        r.pointer_y = 144.0;
        r.layout(&blocks, 2.0, true, Instant::now(), SLIDE);
        let frame = r.layout(&blocks, 2.0, false, Instant::now(), SLIDE);
        assert_eq!(frame.dropped.and_then(|d| d.order), None);
    }
}
