//! Scroll bars: the track and thumb a scroll container draws along its edge
//! while its content overflows.
//!
//! Everything here works from the laid-out tree, after scroll offsets are
//! applied: a container's viewport is its bounds, its content's extent is
//! where its children reach (the spacers of a virtualized list stand in for
//! the rows it didn't build, so its extent is known without them), and its
//! offset is the app's. From those, [`Geometry`] places the bar's strip on
//! the container's trailing edge (the left one when it is laid out right to
//! left; the bottom one when it scrolls sideways), sizes the thumb by the
//! share of the content in view and places it by the offset. [`stamp`]
//! writes each container's bar (as [`BarPaint`]) on its node for this frame,
//! colored by the theme and the pointer's state; the paint pass draws it over
//! the container's children, and the frame diff repaints it when it changes.
//! [`bar_at`] finds the bar a press lands on, for the app to drag the thumb
//! or page through the content.
//!
//! A classic (desktop) bar takes a gutter beside the content: a scroll
//! container whose content overflows lays its children out that much
//! narrower (or shorter, sideways), so the bar never covers them. The layout
//! pass reads that width from [`gutter`], which the app sets from its theme
//! before each build. An overlay bar takes none.

use std::cell::Cell;
use std::collections::HashMap;

use crate::core::runtime::{LayoutNode, ScrollId, clipped_out, takes_press};
use crate::geometry::{Affine, Insets, Point, Rect};
use crate::render::{Color, Scene};
use crate::style::{Palette, ScrollBarLook};

thread_local! {
    static GUTTER: Cell<f64> = const { Cell::new(0.0) };
}

/// The gutter an overflowing scroll container keeps for its bar in the
/// layouts built on this thread (see [`set_gutter`]); 0 until set.
pub(crate) fn gutter() -> f64 {
    GUTTER.with(Cell::get)
}

/// Set the [`gutter`] for the layouts built from now on, on this thread: the
/// app sets its theme's before each build.
pub(crate) fn set_gutter(width: f64) {
    GUTTER.with(|g| g.set(width.max(0.0)));
}

/// A scroll bar as one frame draws it, in its container's layout
/// coordinates.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BarPaint {
    /// The track (the bar's whole strip) and its color, when one is drawn.
    pub(crate) track: Option<(Rect, Color)>,
    /// The thumb, drawn as a pill.
    pub(crate) thumb: Rect,
    pub(crate) thumb_color: Color,
}

/// Draw `bar` (over its container's children).
pub(crate) fn paint(bar: &BarPaint, scene: &mut Scene) {
    if let Some((track, color)) = bar.track {
        scene.fill_rect(track, color);
    }
    let t = bar.thumb;
    if t.width() > 0.0 && t.height() > 0.0 && bar.thumb_color.a > 0 {
        scene.fill_round_rect(t, t.width().min(t.height()) / 2.0, bar.thumb_color);
    }
}

/// Where a press on a scroll bar landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    /// On the thumb: dragging it scrolls.
    Thumb,
    /// On the track before the thumb (above it, or towards the start): pages
    /// back.
    Before,
    /// On the track after the thumb: pages on.
    After,
}

/// A scroll container's bar, laid out: where its strip is and where the
/// thumb sits in it for the container's offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geometry {
    /// The bar's strip along the container's edge, `look.width` across.
    pub(crate) strip: Rect,
    /// Whether the container scrolls sideways.
    pub(crate) horizontal: bool,
    /// Whether its offset counts from the right (a right-to-left sideways
    /// container): the thumb starts at the strip's right end.
    pub(crate) reversed: bool,
    /// The viewport's length along the axis.
    pub(crate) viewport: f64,
    /// The largest offset (the content's length less the viewport's).
    pub(crate) max: f64,
    /// The current offset, `0..=max`.
    pub(crate) offset: f64,
    /// Where the thumb's travel starts and ends along the strip (a little
    /// in from its ends), as distances from the strip's start.
    pub(crate) start: f64,
    pub(crate) end: f64,
    /// The thumb's length.
    pub(crate) thumb_len: f64,
}

impl Geometry {
    /// How far the thumb moves from one end of the content to the other.
    pub(crate) fn travel(&self) -> f64 {
        (self.end - self.start - self.thumb_len).max(0.0)
    }

    /// Where the thumb starts, along the strip from its start.
    pub(crate) fn thumb_start(&self) -> f64 {
        let at = if self.max > 0.0 {
            self.travel() * (self.offset / self.max).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.start + at
    }

    /// The offset that puts the thumb's start at `thumb_start` (along the
    /// strip), clamped to the content.
    pub(crate) fn offset_for(&self, thumb_start: f64) -> f64 {
        let travel = self.travel();
        if travel <= 0.0 {
            return self.offset;
        }
        ((thumb_start - self.start) / travel * self.max).clamp(0.0, self.max)
    }

    /// `p` (in the container's layout coordinates) as a distance along the
    /// strip from its start: down, right, or left in a reversed one.
    pub(crate) fn along(&self, p: Point) -> f64 {
        let s = self.strip;
        match (self.horizontal, self.reversed) {
            (false, _) => p.y - s.min_y(),
            (true, false) => p.x - s.min_x(),
            (true, true) => s.max_x() - p.x,
        }
    }

    /// The thumb, `thickness` across, centered in the strip.
    pub(crate) fn thumb(&self, thickness: f64) -> Rect {
        let s = self.strip;
        let (a, len) = (self.thumb_start(), self.thumb_len);
        if self.horizontal {
            let t = thickness.min(s.height());
            let y = s.min_y() + (s.height() - t) / 2.0;
            let x = if self.reversed {
                s.max_x() - a - len
            } else {
                s.min_x() + a
            };
            Rect::from_xywh(x, y, len, t)
        } else {
            let t = thickness.min(s.width());
            let x = s.min_x() + (s.width() - t) / 2.0;
            Rect::from_xywh(x, s.min_y() + a, t, len)
        }
    }

    /// The part of the bar at `p` (the container's layout coordinates), if
    /// it is on the strip.
    pub(crate) fn part_at(&self, p: Point) -> Option<Part> {
        if !self.strip.contains(p) {
            return None;
        }
        let at = self.along(p);
        let start = self.thumb_start();
        Some(if at < start {
            Part::Before
        } else if at > start + self.thumb_len {
            Part::After
        } else {
            Part::Thumb
        })
    }

    /// The offset a press on the track pages to: a viewport back (`Before`)
    /// or on (`After`), clamped. The thumb stays put.
    pub(crate) fn paged(&self, part: Part) -> f64 {
        // A viewport, less a little so a line stays in view across the jump.
        let page = (self.viewport - (self.viewport * 0.1).min(40.0)).max(1.0);
        match part {
            Part::Before => (self.offset - page).max(0.0),
            Part::After => (self.offset + page).min(self.max),
            Part::Thumb => self.offset,
        }
    }
}

/// Scroll container `node`'s bar at `offset` (the offset its children are
/// shifted by), or `None` when its content fits.
pub(crate) fn geometry(node: &LayoutNode, offset: f64, look: &ScrollBarLook) -> Option<Geometry> {
    node.scroll?;
    let max = crate::core::scroll_range(node, offset);
    let horizontal = node.scroll_horizontal;
    strip_geometry(
        node,
        horizontal,
        horizontal && node.rtl,
        max,
        offset,
        look,
        0.0,
    )
}

/// The bars of scroll container `node` at `offsets`, with the id each
/// scrolls: its own, and the sideways one of a container that also
/// [scrolls sideways](LayoutNode::scroll_x) (both shortened to leave the
/// corner between them when both show).
pub(crate) fn bars(
    node: &LayoutNode,
    offsets: &HashMap<ScrollId, f64>,
    look: &ScrollBarLook,
) -> [Option<(ScrollId, Geometry)>; 2] {
    let Some(id) = node.scroll else {
        return [None, None];
    };
    let off = offsets.get(&id).copied().unwrap_or(0.0);
    let Some(x) = node.scroll_x else {
        return [geometry(node, off, look).map(|g| (id, g)), None];
    };
    let x_off = offsets.get(&x).copied().unwrap_or(0.0);
    let max = crate::core::scroll_range(node, off);
    let max_x = crate::core::render::scroll_range_x(node, x_off);
    let corner = if max >= 0.5 && max_x >= 0.5 {
        look.width.max(0.0)
    } else {
        0.0
    };
    [
        strip_geometry(node, false, false, max, off, look, corner).map(|g| (id, g)),
        strip_geometry(node, true, false, max_x, x_off, look, corner).map(|g| (x, g)),
    ]
}

/// The bar of `node` that scrolls `id`.
pub(crate) fn geometry_for(
    node: &LayoutNode,
    id: ScrollId,
    offsets: &HashMap<ScrollId, f64>,
    look: &ScrollBarLook,
) -> Option<Geometry> {
    bars(node, offsets, look)
        .into_iter()
        .flatten()
        .find(|(i, _)| *i == id)
        .map(|(_, g)| g)
}

/// A bar along `node`'s bottom (`horizontal`) or trailing edge, for
/// content `max` past the viewport at `offset`, its strip `corner` short
/// of the end.
fn strip_geometry(
    node: &LayoutNode,
    horizontal: bool,
    reversed: bool,
    max: f64,
    offset: f64,
    look: &ScrollBarLook,
    corner: f64,
) -> Option<Geometry> {
    let b = node.bounds;
    let (viewport, length) = if horizontal {
        (b.width(), (b.width() - corner).max(0.0))
    } else {
        (b.height(), (b.height() - corner).max(0.0))
    };
    if max < 0.5 || viewport <= 0.0 {
        return None;
    }
    let w = look
        .width
        .max(0.0)
        .min(if horizontal { b.height() } else { b.width() });
    let strip = if horizontal {
        Rect::from_xywh(b.min_x(), b.max_y() - w, length, w)
    } else if node.rtl {
        Rect::from_xywh(b.min_x(), b.min_y(), w, length)
    } else {
        Rect::from_xywh(b.max_x() - w, b.min_y(), w, length)
    };
    // The thumb's ends stay as far in from the strip's as its sides.
    let margin = ((w - look.thumb) / 2.0).clamp(0.0, length / 4.0);
    let (start, end) = (margin, length - margin);
    let room = (end - start).max(0.0);
    let content = viewport + max;
    let thumb_len = (room * viewport / content)
        .max(look.min_thumb.min(room / 2.0))
        .min(room);
    Some(Geometry {
        strip,
        horizontal,
        reversed,
        viewport,
        max,
        offset: offset.clamp(0.0, max),
        start,
        end,
        thumb_len,
    })
}

/// What [`stamp`] draws each bar from.
pub(crate) struct Stamp<'a> {
    pub(crate) look: ScrollBarLook,
    pub(crate) palette: Palette,
    pub(crate) offsets: &'a HashMap<ScrollId, f64>,
    /// The bar under the pointer, and the part of it.
    pub(crate) hover: Option<(ScrollId, Part)>,
    /// The bar whose thumb is being dragged.
    pub(crate) pressed: Option<ScrollId>,
    /// How visible each bar is, 0 to 1 (an overlay bar fades; a classic one
    /// is always 1).
    pub(crate) level: &'a dyn Fn(ScrollId) -> f64,
}

/// Write the bar of every scroll container in `node`'s subtree on its node,
/// or clear it where the content fits (or the bar has faded out). Where a
/// bar changed, the container's box (in window coordinates, `outer` mapping
/// `node`'s parent's to the window's) goes in `changed`.
pub(crate) fn stamp(node: &mut LayoutNode, s: &Stamp<'_>, outer: Affine, changed: &mut Vec<Rect>) {
    let [main, sideways] = bars(node, s.offsets, &s.look);
    let paint = |bar: Option<(ScrollId, Geometry)>| {
        let (id, g) = bar?;
        let level = (s.level)(id).clamp(0.0, 1.0);
        if level <= 0.0 {
            return None;
        }
        let over = s.hover.filter(|(h, _)| *h == id).map(|(_, p)| p);
        let pressed = s.pressed == Some(id);
        let (thumb, track) = s.look.colors(
            &s.palette,
            over == Some(Part::Thumb),
            pressed,
            over.is_some(),
        );
        let fade = |c: Color| Color {
            a: (c.a as f64 * level).round() as u8,
            ..c
        };
        let thickness = if over.is_some() || pressed {
            s.look.thumb_hover
        } else {
            s.look.thumb
        };
        Some(Box::new(BarPaint {
            track: track.map(|c| (g.strip, fade(c))),
            thumb: g.thumb(thickness),
            thumb_color: fade(thumb),
        }))
    };
    let (bar, bar_x) = (paint(main), paint(sideways));
    let m = crate::core::diff::to_window(node, outer);
    // What either bar covers, a pixel wider for anti-aliasing.
    let covers = |b: &Option<Box<BarPaint>>| {
        b.as_ref()
            .map(|b| b.track.map_or(b.thumb, |(t, _)| t.union(b.thumb)))
    };
    let bounds = node.bounds;
    let mut repaint = |old: &Option<Box<BarPaint>>, new: &Option<Box<BarPaint>>| {
        let area = match (covers(old), covers(new)) {
            (Some(a), Some(b)) => a.union(b),
            (a, b) => a.or(b).unwrap_or(bounds),
        };
        changed.push(m.map_rect(area.inset(Insets::uniform(-1.0))));
    };
    if bar != node.scroll_bar {
        repaint(&node.scroll_bar, &bar);
        node.scroll_bar = bar;
    }
    if bar_x != node.scroll_bar_x {
        repaint(&node.scroll_bar_x, &bar_x);
        node.scroll_bar_x = bar_x;
    }
    for child in &mut node.children {
        stamp(child, s, m, changed);
    }
}

/// The scroll bar under `p` (window coordinates) in `node`'s subtree, and
/// the part of it: the topmost one, unless something drawn over it takes the
/// press there.
pub(crate) fn bar_at(
    node: &LayoutNode,
    p: Point,
    look: &ScrollBarLook,
    offsets: &HashMap<ScrollId, f64>,
) -> Option<(ScrollId, Part)> {
    enum Found {
        Bar(ScrollId, Part),
        Covered,
        Nothing,
    }
    fn walk(
        node: &LayoutNode,
        p: Point,
        look: &ScrollBarLook,
        offsets: &HashMap<ScrollId, f64>,
    ) -> Found {
        let Some(local) = node.to_local(p) else {
            return Found::Nothing;
        };
        if clipped_out(node, local) || node.ghost {
            return Found::Nothing;
        }
        // The bars are drawn over the container's children.
        for (id, g) in bars(node, offsets, look).into_iter().flatten() {
            if let Some(part) = g.part_at(local) {
                return Found::Bar(id, part);
            }
        }
        for child in node.children.iter().rev() {
            match walk(child, local, look, offsets) {
                Found::Nothing if takes_press(child, local) => return Found::Covered,
                Found::Nothing => {}
                found => return found,
            }
        }
        Found::Nothing
    }
    match walk(node, p, look, offsets) {
        Found::Bar(id, part) => Some((id, part)),
        _ => None,
    }
}

/// How visible an overlay bar is `since` seconds after its container last
/// scrolled (or the pointer last left it): fully for a moment, then fading
/// out along `motion`.
pub(crate) fn overlay_level(since: f64, motion: crate::anim::Motion) -> f64 {
    use crate::anim::Motion;
    let fade = since - OVERLAY_HOLD;
    if fade <= 0.0 {
        return 1.0;
    }
    let (duration, easing) = match motion {
        Motion::Tween { duration, easing } => (duration, easing),
        Motion::Spring(_) => (0.25, crate::anim::Easing::EaseOut),
        _ => return 0.0,
    };
    let duration = duration * crate::anim::motion_scale();
    if duration <= 0.0 || fade >= duration {
        return 0.0;
    }
    1.0 - easing.apply(fade / duration)
}

/// How long an overlay bar stays in full view after its container stops
/// scrolling, in seconds, before it fades.
pub(crate) const OVERLAY_HOLD: f64 = 0.8;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Axis, Element};
    use crate::geometry::Size;

    /// A 200×100 vertical scroll container over `rows` 20 px rows, laid out
    /// with a `gutter` and scrolled to `offset`.
    fn list(rows: usize, gutter: f64, offset: f64, rtl: bool) -> LayoutNode {
        set_gutter(gutter);
        let rows = (0..rows)
            .map(|_| Element::boxed(Default::default()).height(20.0))
            .collect();
        let el = Element::stack(Axis::Vertical, rows)
            .scrollable(ScrollId(0))
            .width(200.0)
            .height(100.0);
        let dir = if rtl {
            crate::layout::LayoutDirection::Rtl
        } else {
            crate::layout::LayoutDirection::Ltr
        };
        let mut node = crate::core::layout_with_direction(
            &el,
            Rect::from_xywh(0.0, 0.0, 200.0, 100.0),
            None,
            dir,
        );
        set_gutter(0.0);
        let mut offsets = HashMap::from([(ScrollId(0), offset)]);
        crate::core::apply_scroll(&mut node, &mut offsets);
        node
    }

    #[test]
    fn the_thumb_is_sized_by_the_viewport_and_placed_by_the_offset() {
        let look = ScrollBarLook::CLASSIC;
        // 50 rows: 1000 px of content in a 100 px viewport.
        let top = geometry(&list(50, 12.0, 0.0, false), 0.0, &look).unwrap();
        assert_eq!(top.strip, Rect::from_xywh(188.0, 0.0, 12.0, 100.0));
        assert_eq!(top.max, 900.0);
        // The thumb's travel runs 3 px in from each end: 94 px, a tenth of
        // it the thumb's, but no shorter than 24.
        assert_eq!(top.thumb_len, 24.0);
        assert_eq!(top.thumb(6.0), Rect::from_xywh(191.0, 3.0, 6.0, 24.0));
        let bottom = geometry(&list(50, 12.0, 900.0, false), 900.0, &look).unwrap();
        assert_eq!(bottom.thumb(6.0).max_y(), 97.0);
        let middle = geometry(&list(50, 12.0, 450.0, false), 450.0, &look).unwrap();
        let t = middle.thumb(6.0);
        assert!((t.min_y() - (3.0 + 70.0 / 2.0)).abs() < 1e-9, "{t:?}");
        // 8 rows: 160 px for 100, a thumb 100/160 of the travel.
        let short = geometry(&list(8, 12.0, 0.0, false), 0.0, &look).unwrap();
        assert!((short.thumb_len - 94.0 * 100.0 / 160.0).abs() < 1e-9);
        // Content that fits has no bar.
        assert!(geometry(&list(5, 12.0, 0.0, false), 0.0, &look).is_none());
    }

    #[test]
    fn the_gutter_narrows_the_content_only_when_it_overflows() {
        let long = list(50, 12.0, 0.0, false);
        assert_eq!(long.children[0].bounds.width(), 188.0);
        let short = list(5, 12.0, 0.0, false);
        assert_eq!(short.children[0].bounds.width(), 200.0);
        // Right to left, the bar and its gutter are on the left.
        let rtl = list(50, 12.0, 0.0, true);
        assert_eq!(rtl.children[0].bounds.min_x(), 12.0);
        let g = geometry(&rtl, 0.0, &ScrollBarLook::CLASSIC).unwrap();
        assert_eq!(g.strip.min_x(), 0.0);
    }

    #[test]
    fn dragging_maps_the_thumb_to_the_offset_and_the_track_pages() {
        let look = ScrollBarLook::CLASSIC;
        let g = geometry(&list(50, 12.0, 0.0, false), 0.0, &look).unwrap();
        assert_eq!(g.part_at(Point::new(194.0, 10.0)), Some(Part::Thumb));
        assert_eq!(g.part_at(Point::new(194.0, 60.0)), Some(Part::After));
        assert_eq!(g.part_at(Point::new(100.0, 60.0)), None);
        // The thumb's start at the end of its travel is the last offset.
        assert_eq!(g.offset_for(g.end - g.thumb_len), 900.0);
        assert_eq!(g.offset_for(g.start + g.travel() / 2.0), 450.0);
        assert_eq!(g.offset_for(-50.0), 0.0);
        // A page is the viewport less a tenth.
        assert_eq!(g.paged(Part::After), 90.0);
        assert_eq!(g.paged(Part::Before), 0.0);
    }

    #[test]
    fn a_sideways_strip_has_its_bar_along_the_bottom() {
        set_gutter(10.0);
        let chips = (0..10)
            .map(|_| Element::boxed(Default::default()).width(50.0).height(30.0))
            .collect();
        let el = Element::stack(Axis::Horizontal, chips)
            .scrollable(ScrollId(0))
            .width(200.0);
        // Measured with the gutter it wants below the 30 px chips.
        let size = crate::core::measure(&el, Size::new(400.0, 400.0), None);
        assert_eq!(size.height, 40.0);
        let node = crate::core::layout(&el, Rect::from_xywh(0.0, 0.0, 200.0, 40.0), None);
        set_gutter(0.0);
        assert_eq!(node.children[0].bounds.height(), 30.0);
        let g = geometry(&node, 0.0, &ScrollBarLook::CLASSIC.width(10.0)).unwrap();
        assert!(g.horizontal);
        assert_eq!(g.strip, Rect::from_xywh(0.0, 30.0, 200.0, 10.0));
        assert_eq!(g.max, 300.0);
    }

    #[test]
    fn a_right_to_left_strip_starts_its_thumb_at_the_right() {
        let chips = (0..10)
            .map(|_| Element::boxed(Default::default()).width(50.0).height(30.0))
            .collect();
        let el = Element::stack(Axis::Horizontal, chips)
            .scrollable(ScrollId(0))
            .width(200.0);
        let node = crate::core::layout_with_direction(
            &el,
            Rect::from_xywh(0.0, 0.0, 200.0, 40.0),
            None,
            crate::layout::LayoutDirection::Rtl,
        );
        let g = geometry(&node, 0.0, &ScrollBarLook::CLASSIC).unwrap();
        assert!(g.reversed);
        // At its start (offset 0) the thumb is at the right end.
        let t = g.thumb(6.0);
        assert_eq!(t.max_x(), 200.0 - g.start);
        // Dragging it left goes on: further from the right edge.
        let p = Point::new(t.min_x() - 30.0, t.min_y());
        assert!(g.along(p) > g.thumb_start() + g.thumb_len);
        assert_eq!(g.part_at(Point::new(10.0, 34.0)), Some(Part::After));
    }

    #[test]
    fn an_overlay_bar_holds_then_fades() {
        let m = crate::anim::Motion::tween(0.2, crate::anim::Easing::Linear);
        assert_eq!(overlay_level(0.0, m), 1.0);
        assert_eq!(overlay_level(OVERLAY_HOLD, m), 1.0);
        assert!((overlay_level(OVERLAY_HOLD + 0.1, m) - 0.5).abs() < 1e-9);
        assert_eq!(overlay_level(OVERLAY_HOLD + 0.3, m), 0.0);
        assert_eq!(
            overlay_level(OVERLAY_HOLD + 0.01, crate::anim::Motion::Instant),
            0.0
        );
    }

    #[test]
    fn a_container_scrolling_both_ways_has_two_bars_and_a_corner() {
        let look = ScrollBarLook::CLASSIC;
        let (down, across) = (ScrollId(0), ScrollId(1));
        // A 200×100 container over 500×1000 px of content.
        let content = Element::boxed(Default::default())
            .height(1000.0)
            .min_width(500.0);
        let el = Element::stack(Axis::Vertical, vec![content])
            .scrollable(down)
            .scrollable_x(across)
            .height(100.0);
        // Its width follows the space there is, not the content.
        let size = crate::core::measure(&el, Size::new(200.0, 400.0), None);
        assert_eq!(size, Size::new(200.0, 100.0));
        let mut node = crate::core::layout(&el, Rect::from_xywh(0.0, 0.0, 200.0, 100.0), None);
        assert_eq!(node.scroll_x, Some(across));
        let mut offsets = HashMap::from([(down, 40.0), (across, 1_000.0)]);
        crate::core::apply_scroll(&mut node, &mut offsets);
        // Clamped sideways to the content, and both offsets applied.
        assert_eq!(offsets[&across], 300.0);
        assert_eq!(node.children[0].bounds.origin, Point::new(-300.0, -40.0));
        assert_eq!(crate::core::render::scroll_range_x(&node, 300.0), 300.0);
        assert_eq!(crate::core::scroll_range_for(&node, across, 300.0), 300.0);
        assert_eq!(crate::core::scroll_range_for(&node, down, 40.0), 900.0);
        // Two bars, each short of the corner the other leaves.
        let [v, h] = bars(&node, &offsets, &look);
        let (v, h) = (v.unwrap(), h.unwrap());
        assert_eq!((v.0, h.0), (down, across));
        assert_eq!(v.1.strip, Rect::from_xywh(188.0, 0.0, 12.0, 88.0));
        assert_eq!(h.1.strip, Rect::from_xywh(0.0, 88.0, 188.0, 12.0));
        assert!(h.1.horizontal && !h.1.reversed);
        assert_eq!(h.1.max, 300.0);
        assert_eq!(geometry_for(&node, across, &offsets, &look), Some(h.1));
        // A press on the bottom bar is that bar's.
        let thumb = h.1.thumb(6.0);
        assert_eq!(
            bar_at(
                &node,
                Point::new(thumb.max_x() - 2.0, 94.0),
                &look,
                &offsets
            ),
            Some((across, Part::Thumb))
        );
        // The wheel finds both ways under the pointer.
        let t = crate::core::scroll_targets_at(&node, Point::new(50.0, 50.0));
        assert_eq!((t.vertical, t.horizontal), (Some(down), Some(across)));
        assert_eq!(
            crate::core::find_scroll(&node, across).map(|n| n.bounds),
            Some(node.bounds)
        );
    }
}
