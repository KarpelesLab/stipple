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
//! share of the content in view and places it by the offset, between the
//! arrow buttons when the look has them. [`stamp`] writes each container's
//! bar (as [`BarPaint`]) on its node for this frame, colored by the theme
//! (or dressed in its skin's images, or the OS's parts under the native
//! theme) and the pointer's state; the paint pass draws it over the
//! container's children, and the frame diff repaints it when it changes.
//! [`bar_at`] finds the bar a press lands on, for the app to drag the thumb,
//! page through the content or step by an arrow.
//!
//! A classic (desktop) bar takes a gutter beside the content: a scroll
//! container whose content overflows lays its children out that much
//! narrower (or shorter, sideways), so the bar never covers them. The layout
//! pass reads that width from [`gutter`], which the app sets from its theme
//! before each build. An overlay bar takes none. A container that scrolls
//! both ways has two bars, each stopping short of the corner square where
//! they meet; one made with [`widgets::scroll_both`](crate::widgets::scroll_both)
//! keeps both gutters (its content scrolls that much further each way, so
//! neither bar covers its last row or column).

use std::cell::Cell;
use std::collections::HashMap;

use crate::core::runtime::{LayoutNode, ScrollId, clipped_out, takes_press};
use crate::geometry::{Affine, Insets, Point, Rect};
use crate::platform::native::{ArrowDir, NativeState, ScrollPart};
use crate::render::{Color, Path, Scene};
use crate::style::{Palette, ScrollBarLook, ScrollBarSkin};

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

/// What a bar's parts are drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Dress {
    /// The theme's colors.
    Plain,
    /// The skin's images, for the parts it has one for (the colors for the
    /// others).
    Skin(&'static ScrollBarSkin),
    /// The OS's own parts (under the native theme, where the OS draws
    /// them), in its dark appearance or not; the colors where it can't.
    Native { dark: bool },
}

/// An arrow button as one frame draws it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ArrowPaint {
    pub(crate) rect: Rect,
    /// Where it points.
    pub(crate) dir: ArrowDir,
    /// [`Part::ArrowBack`] or [`Part::ArrowForward`].
    pub(crate) part: Part,
    /// The arrow's color, and the button's fill (under the pointer, or
    /// pressed).
    pub(crate) glyph: Color,
    pub(crate) fill: Option<Color>,
}

/// A scroll bar as one frame draws it, in its container's layout
/// coordinates.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BarPaint {
    /// The bar's whole strip.
    pub(crate) strip: Rect,
    pub(crate) horizontal: bool,
    /// The track (the strip) and its color, when one is drawn.
    pub(crate) track: Option<(Rect, Color)>,
    /// The thumb, drawn as a pill.
    pub(crate) thumb: Rect,
    /// The thumb's box across the whole strip (where a skin's or the OS's
    /// thumb is drawn).
    pub(crate) thumb_box: Rect,
    pub(crate) thumb_color: Color,
    /// The arrow buttons, back then forward, when the look has them.
    pub(crate) arrows: Vec<ArrowPaint>,
    /// The square between this (vertical) bar and the container's
    /// horizontal one, and its color.
    pub(crate) corner: Option<(Rect, Color)>,
    /// The part under the pointer, and the part held down.
    pub(crate) hovered: Option<Part>,
    pub(crate) pressed: Option<Part>,
    pub(crate) dress: Dress,
    /// How visible the bar is (an overlay bar fades): the colors above are
    /// already faded; images are drawn this opaque.
    pub(crate) opacity: f64,
}

impl BarPaint {
    /// Everything the bar covers.
    pub(crate) fn covers(&self) -> Rect {
        let mut r = self.track.map_or(self.thumb, |(t, _)| t.union(self.thumb));
        if self.dress != Dress::Plain {
            r = r.union(self.strip);
        }
        for a in &self.arrows {
            r = r.union(a.rect);
        }
        if let Some((c, _)) = self.corner {
            r = r.union(c);
        }
        r
    }

    /// A plain bar of just a track and a thumb, for tests.
    #[cfg(test)]
    pub(crate) fn test(track: Option<(Rect, Color)>, thumb: Rect, thumb_color: Color) -> Self {
        BarPaint {
            strip: track.map_or(thumb, |(t, _)| t),
            horizontal: false,
            track,
            thumb,
            thumb_box: thumb,
            thumb_color,
            arrows: Vec::new(),
            corner: None,
            hovered: None,
            pressed: None,
            dress: Dress::Plain,
            opacity: 1.0,
        }
    }

    fn state(&self, part: Part) -> NativeState {
        NativeState {
            hovered: self.hovered == Some(part),
            pressed: self.pressed == Some(part),
            ..Default::default()
        }
    }
}

/// Draw `bar` (over its container's children).
pub(crate) fn paint(bar: &BarPaint, scene: &mut Scene) {
    let skin = match bar.dress {
        Dress::Skin(s) => Some(s),
        _ => None,
    };
    let native = match bar.dress {
        Dress::Native { dark } => Some(dark),
        _ => None,
    };
    let vertical = !bar.horizontal;
    let os = |scene: &mut Scene, part: ScrollPart, rect: Rect, state: NativeState| {
        native.is_some_and(|dark| paint_native(scene, part, rect, state, dark, bar.opacity))
    };
    let nine = |scene: &mut Scene, rect: Rect, frame: &crate::render::NineSlice| {
        let mut f = frame.clone();
        f.opacity *= bar.opacity as f32;
        scene.draw_nine_slice(rect, &f);
    };
    // The track.
    if !os(
        scene,
        ScrollPart::Track { vertical },
        bar.strip,
        NativeState::default(),
    ) {
        if let Some(frame) = skin.and_then(|s| s.track.as_ref()) {
            nine(scene, bar.strip, frame);
        } else if let Some((track, color)) = bar.track {
            scene.fill_rect(track, color);
        }
    }
    // The arrow buttons.
    for a in &bar.arrows {
        let state = bar.state(a.part);
        if os(scene, ScrollPart::Arrow(a.dir), a.rect, state) {
            continue;
        }
        if let Some(arrows) = skin.and_then(|s| s.arrows.as_ref()) {
            let skin = match a.dir {
                ArrowDir::Up => &arrows.up,
                ArrowDir::Down => &arrows.down,
                ArrowDir::Left => &arrows.left,
                ArrowDir::Right => &arrows.right,
            };
            let frame = crate::style::skin::part_frame(skin, state.hovered, state.pressed);
            nine(scene, a.rect, frame);
            continue;
        }
        if let Some(fill) = a.fill {
            scene.fill_rect(a.rect, fill);
        }
        scene.fill_path(&arrow_glyph(a.rect, a.dir), a.glyph);
    }
    // The thumb.
    let state = bar.state(Part::Thumb);
    let t = bar.thumb_box;
    let drawn = t.width() > 0.0
        && t.height() > 0.0
        && (os(scene, ScrollPart::Thumb { vertical }, t, state)
            || skin.and_then(|s| s.thumb.as_ref()).is_some_and(|thumb| {
                let frame = crate::style::skin::part_frame(thumb, state.hovered, state.pressed);
                nine(scene, t, frame);
                true
            }));
    let t = bar.thumb;
    if !drawn && t.width() > 0.0 && t.height() > 0.0 && bar.thumb_color.a > 0 {
        scene.fill_round_rect(t, t.width().min(t.height()) / 2.0, bar.thumb_color);
    }
    // The corner.
    if let Some((corner, color)) = bar.corner
        && !os(scene, ScrollPart::Corner, corner, NativeState::default())
    {
        match skin.and_then(|s| s.corner.as_ref()) {
            Some(frame) => nine(scene, corner, frame),
            None => scene.fill_rect(corner, color),
        }
    }
}

/// Have the OS draw `part` at `rect`. Returns `false` where it can't.
fn paint_native(
    scene: &mut Scene,
    part: ScrollPart,
    rect: Rect,
    state: NativeState,
    dark: bool,
    opacity: f64,
) -> bool {
    let scale = scene.scale();
    let size = crate::geometry::PhysicalSize::new(
        (rect.width() * scale).round() as u32,
        (rect.height() * scale).round() as u32,
    );
    let Some(image) = crate::platform::native::render_scroll_part(part, state, dark, size, scale)
    else {
        return false;
    };
    if opacity < 1.0 {
        scene.push_layer(Affine::IDENTITY, opacity as f32);
        scene.draw_image(rect, image);
        scene.pop_layer();
    } else {
        scene.draw_image(rect, image);
    }
    true
}

/// A triangle pointing `dir`, centered in `rect`.
fn arrow_glyph(rect: Rect, dir: ArrowDir) -> Path {
    let (cx, cy) = (
        rect.min_x() + rect.width() / 2.0,
        rect.min_y() + rect.height() / 2.0,
    );
    // Half its base, and half its height.
    let s = (rect.width().min(rect.height()) * 0.25).max(1.5);
    let h = s * 0.6;
    let pts = match dir {
        ArrowDir::Up => [(cx - s, cy + h), (cx + s, cy + h), (cx, cy - h)],
        ArrowDir::Down => [(cx - s, cy - h), (cx + s, cy - h), (cx, cy + h)],
        ArrowDir::Left => [(cx + h, cy - s), (cx + h, cy + s), (cx - h, cy)],
        ArrowDir::Right => [(cx - h, cy - s), (cx - h, cy + s), (cx + h, cy)],
    };
    let mut path = Path::new();
    path.move_to(Point::new(pts[0].0, pts[0].1))
        .line_to(Point::new(pts[1].0, pts[1].1))
        .line_to(Point::new(pts[2].0, pts[2].1))
        .close();
    path
}

/// Where a press on a scroll bar landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Part {
    /// On the thumb: dragging it scrolls.
    Thumb,
    /// On the track before the thumb (above it, or towards the start): pages
    /// back.
    Before,
    /// On the track after the thumb: pages on.
    After,
    /// On the arrow button at the start: steps back.
    ArrowBack,
    /// On the arrow button at the end: steps on.
    ArrowForward,
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
    /// in from its ends, past the arrow buttons), as distances from the
    /// strip's start.
    pub(crate) start: f64,
    pub(crate) end: f64,
    /// The thumb's length.
    pub(crate) thumb_len: f64,
    /// Each arrow button's length along the strip (0 without them).
    pub(crate) arrow: f64,
}

impl Geometry {
    /// The strip's length along its axis.
    pub(crate) fn length(&self) -> f64 {
        if self.horizontal {
            self.strip.width()
        } else {
            self.strip.height()
        }
    }

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

    /// The part of the strip from `at` along it, `len` long and `thickness`
    /// across (centered).
    fn span(&self, at: f64, len: f64, thickness: f64) -> Rect {
        let s = self.strip;
        if self.horizontal {
            let t = thickness.min(s.height());
            let y = s.min_y() + (s.height() - t) / 2.0;
            let x = if self.reversed {
                s.max_x() - at - len
            } else {
                s.min_x() + at
            };
            Rect::from_xywh(x, y, len, t)
        } else {
            let t = thickness.min(s.width());
            let x = s.min_x() + (s.width() - t) / 2.0;
            Rect::from_xywh(x, s.min_y() + at, t, len)
        }
    }

    /// The thumb, `thickness` across, centered in the strip.
    pub(crate) fn thumb(&self, thickness: f64) -> Rect {
        self.span(self.thumb_start(), self.thumb_len, thickness)
    }

    /// The thumb's box across the whole strip, between the arrows' ends.
    pub(crate) fn thumb_box(&self) -> Rect {
        let lead = (self.start - self.arrow).max(0.0);
        let (at, len) = (self.thumb_start() - lead, self.thumb_len + 2.0 * lead);
        let at = at.max(self.arrow);
        let len = len.min(self.length() - self.arrow - at).max(0.0);
        self.span(at, len, f64::INFINITY)
    }

    /// The arrow button at the strip's start (`forward` false) or end.
    pub(crate) fn arrow_rect(&self, forward: bool) -> Option<Rect> {
        if self.arrow <= 0.0 {
            return None;
        }
        let at = if forward {
            self.length() - self.arrow
        } else {
            0.0
        };
        Some(self.span(at, self.arrow, f64::INFINITY))
    }

    /// Where the arrow at the start (`forward` false) or end points.
    pub(crate) fn arrow_dir(&self, forward: bool) -> ArrowDir {
        match (self.horizontal, forward != self.reversed) {
            (false, false) => ArrowDir::Up,
            (false, true) => ArrowDir::Down,
            (true, false) => ArrowDir::Left,
            (true, true) => ArrowDir::Right,
        }
    }

    /// The part of the bar at `p` (the container's layout coordinates), if
    /// it is on the strip.
    pub(crate) fn part_at(&self, p: Point) -> Option<Part> {
        if !self.strip.contains(p) {
            return None;
        }
        let at = self.along(p);
        if self.arrow > 0.0 {
            if at < self.arrow {
                return Some(Part::ArrowBack);
            }
            if at >= self.length() - self.arrow {
                return Some(Part::ArrowForward);
            }
        }
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
    #[cfg(test)]
    pub(crate) fn paged(&self, part: Part) -> f64 {
        self.pressed(part, 0.0)
    }

    /// The offset a press on `part` goes to: a page for the track, `step`
    /// for an arrow, clamped; nowhere for the thumb.
    pub(crate) fn pressed(&self, part: Part, step: f64) -> f64 {
        // A viewport, less a little so a line stays in view across the jump.
        let page = (self.viewport - (self.viewport * 0.1).min(40.0)).max(1.0);
        let step = step.max(1.0);
        match part {
            Part::Before => (self.offset - page).max(0.0),
            Part::After => (self.offset + page).min(self.max),
            Part::ArrowBack => (self.offset - step).max(0.0),
            Part::ArrowForward => (self.offset + step).min(self.max),
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
        strip_geometry(node, true, node.rtl, max_x, x_off, look, corner).map(|g| (x, g)),
    ]
}

/// The square where `node`'s two bars meet, when both show: at its bottom
/// right (bottom left, right to left).
pub(crate) fn corner(
    node: &LayoutNode,
    offsets: &HashMap<ScrollId, f64>,
    look: &ScrollBarLook,
) -> Option<Rect> {
    let [Some(_), Some(_)] = bars(node, offsets, look) else {
        return None;
    };
    let b = node.bounds;
    let w = look.width.max(0.0).min(b.width()).min(b.height());
    let x = if node.rtl { b.min_x() } else { b.max_x() - w };
    Some(Rect::from_xywh(x, b.max_y() - w, w, w))
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
/// of the end (the trailing one: the right, or the left right to left).
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
    let strip = if horizontal && node.rtl {
        Rect::from_xywh(b.min_x() + corner, b.max_y() - w, length, w)
    } else if horizontal {
        Rect::from_xywh(b.min_x(), b.max_y() - w, length, w)
    } else if node.rtl {
        Rect::from_xywh(b.min_x(), b.min_y(), w, length)
    } else {
        Rect::from_xywh(b.max_x() - w, b.min_y(), w, length)
    };
    // Arrow buttons: squares at the ends, smaller on a bar too short for
    // two of them.
    let arrow = if look.shows_arrows() {
        w.min(length / 2.0)
    } else {
        0.0
    };
    let inner = (length - 2.0 * arrow).max(0.0);
    // The thumb's ends stay as far in from the strip's as its sides.
    let margin = ((w - look.thumb) / 2.0).clamp(0.0, inner / 4.0);
    let (start, end) = (arrow + margin, length - arrow - margin);
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
        arrow,
    })
}

/// What [`stamp`] draws each bar from.
pub(crate) struct Stamp<'a> {
    pub(crate) look: ScrollBarLook,
    pub(crate) palette: Palette,
    pub(crate) dress: Dress,
    pub(crate) offsets: &'a HashMap<ScrollId, f64>,
    /// The bar under the pointer, and the part of it.
    pub(crate) hover: Option<(ScrollId, Part)>,
    /// The bar a press is held on, and the part it holds (the thumb, while
    /// dragged).
    pub(crate) pressed: Option<(ScrollId, Part)>,
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
    let corner = corner(node, s.offsets, &s.look).filter(|_| !s.look.overlay);
    let paint = |bar: Option<(ScrollId, Geometry)>, corner: Option<Rect>| {
        let (id, g) = bar?;
        let level = (s.level)(id).clamp(0.0, 1.0);
        if level <= 0.0 {
            return None;
        }
        let over = s.hover.filter(|(h, _)| *h == id).map(|(_, p)| p);
        let held = s.pressed.filter(|(h, _)| *h == id).map(|(_, p)| p);
        let dragged = held == Some(Part::Thumb);
        let (thumb, track) = s.look.colors(
            &s.palette,
            over == Some(Part::Thumb),
            dragged,
            over.is_some(),
        );
        let fade = |c: Color| Color {
            a: (c.a as f64 * level).round() as u8,
            ..c
        };
        let thickness = if over.is_some() || held.is_some() {
            s.look.thumb_hover
        } else {
            s.look.thumb
        };
        let arrows = [false, true]
            .into_iter()
            .filter_map(|forward| {
                let rect = g.arrow_rect(forward)?;
                let part = if forward {
                    Part::ArrowForward
                } else {
                    Part::ArrowBack
                };
                let (glyph, fill) = s.look.arrow_colors(
                    &s.palette,
                    over == Some(part),
                    held == Some(part),
                    g.offset <= 0.0 && !forward || g.offset >= g.max && forward,
                );
                Some(ArrowPaint {
                    rect,
                    dir: g.arrow_dir(forward),
                    part,
                    glyph: fade(glyph),
                    fill: fill.map(fade),
                })
            })
            .collect();
        Some(Box::new(BarPaint {
            strip: g.strip,
            horizontal: g.horizontal,
            track: track.map(|c| (g.strip, fade(c))),
            thumb: g.thumb(thickness),
            thumb_box: g.thumb_box(),
            thumb_color: fade(thumb),
            arrows,
            corner: corner.zip(track.map(fade)),
            hovered: over,
            pressed: held,
            dress: s.dress,
            opacity: level,
        }))
    };
    let (bar, bar_x) = (paint(main, corner), paint(sideways, None));
    let m = crate::core::diff::to_window(node, outer);
    // What either bar covers, a pixel wider for anti-aliasing.
    let covers = |b: &Option<Box<BarPaint>>| b.as_ref().map(|b| b.covers());
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
        // The corner between them takes the press, as the bars do.
        if !look.overlay && corner(node, offsets, look).is_some_and(|c| c.contains(local)) {
            return Found::Covered;
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

    #[test]
    fn arrows_take_the_strip_ends_and_mirror_right_to_left() {
        let look = ScrollBarLook::CLASSIC.arrows(true);
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
        let g = geometry(&node, 0.0, &look).unwrap();
        assert_eq!(g.arrow, 12.0);
        // The start is on the right: its button points right, and a press
        // there steps back; the left one steps on.
        assert_eq!(g.arrow_dir(false), ArrowDir::Right);
        assert_eq!(g.arrow_dir(true), ArrowDir::Left);
        let back = g.arrow_rect(false).unwrap();
        assert_eq!(back.max_x(), 200.0);
        assert_eq!(g.part_at(center(back)), Some(Part::ArrowBack));
        let on = g.arrow_rect(true).unwrap();
        assert_eq!(on.min_x(), 0.0);
        assert_eq!(g.part_at(center(on)), Some(Part::ArrowForward));
        // The thumb's travel stops short of both.
        let t = g.thumb(6.0);
        assert_eq!(t.max_x(), 200.0 - 12.0 - 3.0);
        assert_eq!(g.pressed(Part::ArrowForward, 40.0), 40.0);
        assert_eq!(g.pressed(Part::ArrowBack, 40.0), 0.0);
        // A bar too short for two whole buttons halves them, and has no
        // room for the thumb's travel left.
        let el = Element::stack(
            Axis::Vertical,
            vec![Element::boxed(Default::default()).height(100.0)],
        )
        .scrollable(ScrollId(0))
        .height(16.0)
        .width(50.0);
        let node = crate::core::layout(&el, Rect::from_xywh(0.0, 0.0, 50.0, 16.0), None);
        let g = geometry(&node, 0.0, &look).unwrap();
        assert_eq!(g.arrow, 8.0);
        assert_eq!(g.thumb_len, 0.0);
    }

    fn center(r: Rect) -> Point {
        Point::new(r.min_x() + r.width() / 2.0, r.min_y() + r.height() / 2.0)
    }
}
