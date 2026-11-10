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

/// An invalidation of canvas `id`: all of it (`None`) or a rectangle in its
/// own coordinates.
pub(crate) type CanvasInvalidation = (CanvasId, Option<Rect>);

/// Where the invalidated parts of canvases are drawn in the window (logical
/// pixels), for the canvases in `tree`: each one's rect, clipped to the
/// canvas's box and moved through every transform on the way to it. Canvases
/// not in the tree are skipped.
pub(crate) fn invalidated_rects(tree: &LayoutNode, pending: &[CanvasInvalidation]) -> Vec<Rect> {
    let mut out = Vec::new();
    for &(id, rect) in pending {
        let is =
            |n: &LayoutNode| matches!(&n.content, NodeContent::Canvas { id: c, .. } if *c == id);
        let Some((node, to_window, _)) = crate::core::node_to_window(tree, &is) else {
            continue;
        };
        let b = node.bounds;
        let area = match rect {
            None => Some(b),
            Some(r) => r.translate(Vec2::new(b.min_x(), b.min_y())).intersection(b),
        };
        let Some(area) = area else {
            continue;
        };
        out.push(drawn(to_window, area));
    }
    out
}

/// Where `rect` (mapped by `m`) is drawn: itself when untransformed, else the
/// box around it, a pixel wider for anti-aliasing.
fn drawn(m: Affine, rect: Rect) -> Rect {
    if m.is_identity() {
        rect
    } else {
        let r = m.map_rect(rect);
        Rect::from_xywh(
            r.min_x() - 1.0,
            r.min_y() - 1.0,
            r.width() + 2.0,
            r.height() + 2.0,
        )
    }
}
