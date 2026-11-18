//! Custom-paint elements: [`Element::canvas`], [`CanvasId`] and the paint
//! callback they carry.
//!
//! A canvas is a leaf that takes part in layout and hit-testing like any other
//! element, and whose pixels come from an app callback drawing into a
//! [`Canvas`] in the element's own coordinates. Its content changes without
//! the view being rebuilt: [`AppHandle::invalidate_canvas`] (and
//! [`invalidate_canvas_rect`](AppHandle::invalidate_canvas_rect)) repaint all
//! of it, or just a rectangle — the rows a terminal changed — and only that
//! area is re-rasterized and presented.
//!
//! [`AppHandle::invalidate_canvas`]: crate::core::AppHandle::invalidate_canvas
//! [`AppHandle`]: crate::core::AppHandle

use crate::core::element::{BoxStyle, Element, ElementKind};
use crate::core::runtime::{LayoutNode, NodeContent};
use crate::geometry::{Affine, Rect, Vec2};
use crate::render::{Canvas, CanvasPaint};
use std::rc::Rc;

/// A **caller-chosen** handle to a [canvas](Element::canvas): the value the
/// app invalidates it by (see
/// [`AppHandle::invalidate_canvas`](crate::core::AppHandle::invalidate_canvas)).
/// Keep it the same from build to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanvasId(pub u32);

/// A canvas's paint callback, as an element carries it.
///
/// Two of them always compare equal: a rebuilt view makes a new closure every
/// time, and what decides whether the canvas repaints is its invalidations and
/// [revision](Element::revision), not the closure's identity.
#[derive(Clone)]
pub struct CanvasPainter(pub CanvasPaint);

impl PartialEq for CanvasPainter {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for CanvasPainter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CanvasPainter(..)")
    }
}

impl Element {
    /// A custom-paint leaf: `paint` draws it into a [`Canvas`] whose
    /// coordinates start at the element's top-left corner, clipped to its box.
    ///
    /// Like a [viewport](Element::viewport), it has no size of its own: give
    /// it one with [`width`](Element::width)/[`height`](Element::height) or
    /// [`grow`](Element::grow). It takes handlers, a transform and an opacity
    /// like any element.
    ///
    /// `paint` runs when the canvas is rasterized, once per area being
    /// rasterized, and should draw at least [`Canvas::dirty`]. It isn't told
    /// the app state: it reads what it draws from data it shares with the app
    /// (an `Rc<RefCell<…>>`, or an `Arc<Mutex<…>>` a background thread
    /// writes). After changing that data, call
    /// [`AppHandle::invalidate_canvas`](crate::core::AppHandle::invalidate_canvas)
    /// or [`invalidate_canvas_rect`](crate::core::AppHandle::invalidate_canvas_rect)
    /// with `id`: the canvas repaints without the view being rebuilt. A
    /// rebuilt view doesn't repaint it by itself (the closure isn't compared);
    /// change its [`revision`](Element::revision) for that.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// use stipple::core::CanvasId;
    ///
    /// let grid = Element::canvas(CanvasId(1), |c: &mut Canvas| {
    ///     let size = c.size();
    ///     c.fill_rect(Rect::from_xywh(0.0, 0.0, size.width, size.height), Color::BLACK);
    ///     // A red diagonal across whatever size layout gave it.
    ///     c.draw_line(Point::ORIGIN, Point::new(size.width, size.height), Color::rgb(255, 0, 0), 1.0);
    /// })
    /// .width(320.0)
    /// .height(200.0);
    /// # let _ = grid;
    /// ```
    pub fn canvas(id: CanvasId, paint: impl Fn(&mut Canvas<'_>) + 'static) -> Element {
        Element {
            kind: ElementKind::Canvas {
                id,
                paint: CanvasPainter(Rc::new(paint)),
                revision: 0,
            },
            ..Element::boxed(BoxStyle::default())
        }
    }

    /// For a [canvas](Element::canvas): a version of what it shows. When a
    /// rebuilt view gives the canvas a different revision than the one on
    /// screen, all of it repaints — the way to repaint a canvas from a view
    /// rebuild (say, a counter bumped in the state) rather than through
    /// [`AppHandle::invalidate_canvas`](crate::core::AppHandle::invalidate_canvas).
    /// No effect on other elements.
    pub fn revision(mut self, revision: u64) -> Self {
        if let ElementKind::Canvas { revision: r, .. } = &mut self.kind {
            *r = revision;
        }
        self
    }
}

/// A change to a canvas queued on the [`AppHandle`]: all of it (`None`) or a
/// rectangle of it to repaint, or the pixels in a rectangle moved.
///
/// [`AppHandle`]: crate::core::AppHandle
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CanvasChange {
    Repaint(Option<Rect>),
    Scroll { rect: Rect, delta: Vec2 },
}

/// A change to canvas `id`, in its own coordinates.
pub(crate) type CanvasInvalidation = (CanvasId, CanvasChange);

/// Pixels of a window's frame moved by a canvas scroll: the device pixels
/// `dst` (`[x0, y0, x1, y1]`) take those `delta` device pixels back
/// (`dst - delta`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameBlit {
    pub dst: [i32; 4],
    pub delta: (i32, i32),
}

impl FrameBlit {
    /// The moved pixels, as a logical rect at `scale`.
    pub(crate) fn logical(&self, scale: f64) -> Rect {
        let [x0, y0, x1, y1] = self.dst;
        Rect::from_xywh(
            x0 as f64 / scale,
            y0 as f64 / scale,
            (x1 - x0) as f64 / scale,
            (y1 - y0) as f64 / scale,
        )
    }

    /// Move the pixels in `pixmap` (which holds the window's frame).
    pub(crate) fn apply(&self, pixmap: &mut crate::render::Pixmap) {
        let size = pixmap.size();
        let [x0, y0, x1, y1] = self.dst;
        let (dx, dy) = self.delta;
        // Both ends inside the frame.
        let x0 = x0.max(0).max(dx);
        let y0 = y0.max(0).max(dy);
        let x1 = x1.min(size.width as i32).min(size.width as i32 + dx);
        let y1 = y1.min(size.height as i32).min(size.height as i32 + dy);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let stride = pixmap.stride();
        let row = (x1 - x0) as usize * 4;
        let buf = pixmap.as_bytes_mut();
        // Rows in the order that reads each before it's overwritten.
        let rows: Box<dyn Iterator<Item = i32>> = if dy > 0 {
            Box::new((y0..y1).rev())
        } else {
            Box::new(y0..y1)
        };
        for y in rows {
            let to = y as usize * stride + x0 as usize * 4;
            let from = (y - dy) as usize * stride + (x0 - dx) as usize * 4;
            buf.copy_within(from..from + row, to);
        }
    }
}

/// A canvas scroll, for the GPU renderers' canvas textures
/// (`gpu::canvas`): the pixels in `rect` (canvas coordinates) moved by
/// `delta`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CanvasScroll {
    #[cfg_attr(not(feature = "gpu"), allow(dead_code))]
    pub id: CanvasId,
    #[cfg_attr(not(feature = "gpu"), allow(dead_code))]
    pub rect: Rect,
    #[cfg_attr(not(feature = "gpu"), allow(dead_code))]
    pub delta: Vec2,
}

/// What a window does about its canvases' changes: first move the pixels of
/// its frame `blits` says, then repaint `repaint` (logical rects). `scrolls`
/// are the same moves for canvases kept as textures.
#[derive(Debug, Default)]
pub(crate) struct ContentDamage {
    pub repaint: Vec<Rect>,
    pub blits: Vec<FrameBlit>,
    pub scrolls: Vec<CanvasScroll>,
}

/// Where a canvas is drawn in a window.
struct Placement {
    /// Its box, in its own layout coordinates.
    node_bounds: Rect,
    /// Its box, in window coordinates.
    bounds: Rect,
    /// How its own coordinates map to the window's.
    to_window: Affine,
    /// Drawn upright, unscaled and opaque: its pixels can be moved.
    straight: bool,
    /// What its ancestors clip it to (window coordinates).
    clip: Rect,
    /// Boxes of what is drawn over it later (window coordinates).
    over: Vec<Rect>,
}

/// How far past its box a node drawn over a canvas is assumed to draw.
const OVER_MARGIN: f64 = crate::core::render::CULL_MARGIN;

/// Whether `node` draws something itself (not just through its children).
fn paints(node: &LayoutNode) -> bool {
    !matches!(node.content, NodeContent::None)
        || node.decoration.fill.is_some()
        || node.decoration.border.is_some()
        || node.background.is_some()
        || node.native.is_some()
        || node.scroll_bar.is_some()
}

fn inflate(r: Rect, by: f64) -> Rect {
    Rect::from_xywh(
        r.min_x() - by,
        r.min_y() - by,
        r.width() + 2.0 * by,
        r.height() + 2.0 * by,
    )
}

/// Canvas `id` in `tree`, drawn in a window of `window` (logical rect).
fn locate(tree: &LayoutNode, id: CanvasId, window: Rect) -> Option<Placement> {
    struct Walk {
        id: CanvasId,
        found: Option<Placement>,
    }
    fn walk(node: &LayoutNode, outer: Affine, straight: bool, clip: Rect, w: &mut Walk) {
        let m = match node.affine() {
            Some(a) => a.then(outer),
            None => outer,
        };
        let [a, b, c, d, ..] = m.as_array();
        let straight = straight && node.opacity >= 1.0 && (a, b, c, d) == (1.0, 0.0, 0.0, 1.0);
        let drawn = m.map_rect(node.bounds);
        let canvas = matches!(&node.content, NodeContent::Canvas { id, .. } if *id == w.id);
        match &mut w.found {
            Some(p) => {
                if paints(node) {
                    p.over.push(inflate(drawn, OVER_MARGIN));
                }
            }
            None if canvas => {
                w.found = Some(Placement {
                    node_bounds: node.bounds,
                    bounds: drawn,
                    to_window: m,
                    straight,
                    clip,
                    over: Vec::new(),
                });
            }
            None => {}
        }
        let inner = if node.clip {
            clip.intersection(drawn).unwrap_or(Rect::ZERO)
        } else {
            clip
        };
        for child in &node.children {
            walk(child, m, straight, inner, w);
        }
        // A scroll container's bar is drawn over its content.
        if let (Some(p), Some(bar)) = (&mut w.found, &node.scroll_bar) {
            let mut r = bar.thumb;
            if let Some((track, _)) = bar.track {
                r = r.union(track);
            }
            p.over.push(inflate(m.map_rect(r), 1.0));
        }
    }
    let mut w = Walk { id, found: None };
    walk(tree, Affine::IDENTITY, true, window, &mut w);
    let mut p = w.found?;
    let b = p.bounds;
    p.over.retain(|r| r.intersection(b).is_some());
    Some(p)
}

/// `a` less `b`: up to four rects.
pub(crate) fn subtract(a: Rect, b: Rect) -> Vec<Rect> {
    let Some(i) = a.intersection(b) else {
        return vec![a];
    };
    let mut out = Vec::new();
    let band = |x0: f64, y0: f64, x1: f64, y1: f64| {
        (x1 > x0 && y1 > y0).then(|| Rect::from_xywh(x0, y0, x1 - x0, y1 - y0))
    };
    out.extend(band(a.min_x(), a.min_y(), a.max_x(), i.min_y()));
    out.extend(band(a.min_x(), i.max_y(), a.max_x(), a.max_y()));
    out.extend(band(a.min_x(), i.min_y(), i.min_x(), i.max_y()));
    out.extend(band(i.max_x(), i.min_y(), a.max_x(), i.max_y()));
    out
}

/// What the changes `pending` to the canvases in `tree` (drawn in a window
/// of `window`, a logical rect, at `scale` device pixels per logical pixel)
/// take: the areas to repaint, clipped to each canvas's box and moved
/// through every transform on the way to it, and the pixels to move first.
/// `over` are boxes drawn over the window's content besides the tree's (the
/// focus ring, hover highlights) and what changed in it since the frame on
/// screen. Canvases not in the tree are skipped.
pub(crate) fn content_damage(
    tree: &LayoutNode,
    pending: &[CanvasInvalidation],
    scale: f64,
    window: Rect,
    over: &[Rect],
) -> ContentDamage {
    let mut out = ContentDamage::default();
    let mut ids: Vec<CanvasId> = Vec::new();
    for (id, _) in pending {
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    for id in ids {
        let Some(p) = locate(tree, id, window) else {
            continue;
        };
        let nb = p.node_bounds;
        let local = Rect::new(crate::geometry::Point::ORIGIN, nb.size);
        // Each change's area, in the window: exact when the canvas is drawn
        // straight, else the box around it, a pixel wider for
        // anti-aliasing.
        let area = |r: Option<Rect>| -> Option<Rect> {
            let r = r.map_or(Some(local), |r| r.intersection(local))?;
            let r = p
                .to_window
                .map_rect(r.translate(Vec2::new(nb.min_x(), nb.min_y())));
            Some(if p.straight { r } else { inflate(r, 1.0) })
        };
        let mut dirty: Vec<Rect> = Vec::new();
        for change in pending.iter().filter(|c| c.0 == id).map(|c| c.1) {
            let (rect, delta) = match change {
                CanvasChange::Repaint(r) => {
                    dirty.extend(area(r));
                    continue;
                }
                CanvasChange::Scroll { rect, delta } => (rect, delta),
            };
            if let Some(r) = rect.intersection(local) {
                out.scrolls.push(CanvasScroll { id, rect: r, delta });
            }
            let Some(r) = area(Some(rect)) else {
                continue;
            };
            let moved = p.straight.then(|| blit(r, p.clip, delta, scale)).flatten();
            let Some(b) = moved else {
                dirty.push(r);
                continue;
            };
            let dst = b.logical(scale);
            // What was to repaint moves with the pixels.
            dirty = dirty
                .into_iter()
                .flat_map(|d| {
                    let mut v = subtract(d, r);
                    if let Some(m) = d
                        .intersection(r)
                        .and_then(|i| i.translate(delta).intersection(r))
                    {
                        v.push(m);
                    }
                    v
                })
                .collect();
            // What the move uncovered, and what it didn't move whole.
            dirty.extend(subtract(r, dst));
            // What is drawn over the canvas stays put: repainted where it
            // was moved from and to.
            for o in p.over.iter().chain(over) {
                dirty.extend(o.intersection(dst));
                dirty.extend(o.translate(delta).intersection(dst));
            }
            out.blits.push(b);
        }
        out.repaint.extend(dirty);
    }
    out
}

/// The pixels of the window's frame moved by scrolling `r` (window logical
/// coordinates; what shows of it within `clip`) by `delta`, at `scale`:
/// those that stay inside it, in whole device pixels. `None` when the move
/// isn't by whole device pixels, or nothing stays.
pub(crate) fn blit(r: Rect, clip: Rect, delta: Vec2, scale: f64) -> Option<FrameBlit> {
    let r = r.intersection(clip)?;
    let (dx, dy) = (delta.dx * scale, delta.dy * scale);
    let whole = |v: f64| (v - v.round()).abs() < 1e-3;
    if !whole(dx) || !whole(dy) {
        return None;
    }
    let (dx, dy) = (dx.round() as i32, dy.round() as i32);
    // The device pixels wholly inside it.
    let x0 = (r.min_x() * scale - 1e-3).ceil() as i32;
    let y0 = (r.min_y() * scale - 1e-3).ceil() as i32;
    let x1 = (r.max_x() * scale + 1e-3).floor() as i32;
    let y1 = (r.max_y() * scale + 1e-3).floor() as i32;
    // Those whose pixels come from inside it too.
    let dst = [
        x0.max(x0 + dx),
        y0.max(y0 + dy),
        x1.min(x1 + dx),
        y1.min(y1 + dy),
    ];
    (dst[2] > dst[0] && dst[3] > dst[1] && (dx, dy) != (0, 0)).then_some(FrameBlit {
        dst,
        delta: (dx, dy),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PhysicalSize;
    use crate::render::Pixmap;

    #[test]
    fn subtracting_leaves_the_bands_around() {
        let a = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        let b = Rect::from_xywh(2.0, 3.0, 4.0, 4.0);
        let parts = subtract(a, b);
        let area: f64 = parts.iter().map(|r| r.width() * r.height()).sum();
        assert_eq!(area, 100.0 - 16.0);
        assert!(parts.iter().all(|r| r.intersection(b).is_none()));
        assert_eq!(subtract(a, Rect::from_xywh(20.0, 0.0, 1.0, 1.0)), vec![a]);
        assert!(subtract(b, a).is_empty());
    }

    #[test]
    fn a_blit_moves_whole_device_pixels_and_keeps_the_rest() {
        let r = Rect::from_xywh(0.0, 0.0, 100.0, 50.0);
        let b = blit(r, r, Vec2::new(0.0, -10.0), 2.0).unwrap();
        assert_eq!(b.dst, [0, 0, 200, 80]);
        assert_eq!(b.delta, (0, -20));
        // Half a device pixel: no.
        assert!(blit(r, r, Vec2::new(0.0, -0.25), 2.0).is_none());
        // Moved past its size: nothing stays.
        assert!(blit(r, r, Vec2::new(0.0, 60.0), 1.0).is_none());
        // Clipped by an ancestor.
        let b = blit(
            r,
            Rect::from_xywh(0.0, 10.0, 100.0, 20.0),
            Vec2::new(0.0, 5.0),
            1.0,
        );
        assert_eq!(b.unwrap().dst, [0, 15, 100, 30]);
    }

    #[test]
    fn applying_a_blit_moves_the_pixels_down_or_up() {
        let rows = |p: &Pixmap| {
            (0..6)
                .map(|y| p.pixel(1, y).unwrap()[0])
                .collect::<Vec<_>>()
        };
        let mut data = Vec::new();
        for y in 0..6u8 {
            for _ in 0..3 {
                data.extend_from_slice(&[y * 10, 0, 0, 255]);
            }
        }
        let fresh = Pixmap::from_rgba8(PhysicalSize::new(3, 6), data);
        let mut p = fresh.clone();
        FrameBlit {
            dst: [0, 0, 3, 4],
            delta: (0, -2),
        }
        .apply(&mut p);
        assert_eq!(rows(&p), [20, 30, 40, 50, 40, 50]);
        let mut p = fresh.clone();
        FrameBlit {
            dst: [0, 2, 3, 6],
            delta: (0, 2),
        }
        .apply(&mut p);
        assert_eq!(rows(&p), [0, 10, 0, 10, 20, 30]);
    }
}
