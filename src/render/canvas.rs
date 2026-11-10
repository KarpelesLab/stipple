//! [`Canvas`]: what a custom-paint element's callback draws into.

use crate::geometry::{Rect, Size};
use crate::render::Scene;
use std::rc::Rc;

/// A canvas's paint callback (see [`Scene::draw_canvas`] and
/// [`Element::canvas`](crate::core::Element::canvas)).
pub type CanvasPaint = Rc<dyn Fn(&mut Canvas<'_>)>;

/// The surface a custom-paint callback draws into: a [`Scene`] in the
/// canvas's own coordinates, with `(0, 0)` at its top-left corner and
/// [`size`](Canvas::size) at its bottom-right, clipped to that box.
///
/// Every [`Scene`] method is available on it (it dereferences to one): rects,
/// rounded rects, [paths](Scene::fill_path) and [lines](Scene::draw_line),
/// [bitmaps](Scene::draw_image), [text](Scene::fill_text) and
/// [glyph runs](Scene::fill_glyph_run).
///
/// The callback runs when the canvas is rasterized — once per area being
/// rasterized, which may be several per frame — and should draw at least
/// [`dirty`](Canvas::dirty). Drawing more is harmless (it's culled), so
/// skipping what lies outside it is purely an optimization: a terminal paints
/// only the rows that cross it.
pub struct Canvas<'a> {
    scene: &'a mut Scene,
    dirty: Rect,
}

impl std::fmt::Debug for Canvas<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canvas")
            .field("size", &self.size())
            .field("dirty", &self.dirty)
            .finish_non_exhaustive()
    }
}

impl<'a> Canvas<'a> {
    /// A canvas drawing into `scene` (whose logical size is the canvas's),
    /// asked to paint `dirty`.
    pub fn new(scene: &'a mut Scene, dirty: Rect) -> Self {
        Self { scene, dirty }
    }

    /// The canvas's size in logical pixels.
    pub fn size(&self) -> Size {
        self.scene.logical_size()
    }

    /// The canvas's box in its own coordinates: `(0, 0)` to its size.
    pub fn bounds(&self) -> Rect {
        Rect::new(crate::geometry::Point::ORIGIN, self.size())
    }

    /// The area this call must paint, in canvas coordinates.
    pub fn dirty(&self) -> Rect {
        self.dirty
    }

    /// Whether `rect` (canvas coordinates) crosses the area this call must
    /// paint: what to test before drawing a row or a cell.
    pub fn needs_paint(&self, rect: Rect) -> bool {
        rect.intersection(self.dirty).is_some()
    }
}

impl std::ops::Deref for Canvas<'_> {
    type Target = Scene;
    fn deref(&self) -> &Scene {
        self.scene
    }
}

impl std::ops::DerefMut for Canvas<'_> {
    fn deref_mut(&mut self) -> &mut Scene {
        self.scene
    }
}
