//! Pixel art at a fixed virtual resolution, scaled by whole numbers.

use crate::geometry::{PhysicalSize, Point, Rect, Size};
use crate::render::{Color, ImageOptions, Pixmap, Scene, SoftwareRenderer};
use std::cell::Cell;
use std::sync::Arc;

/// A fixed low resolution (say 320×180) that a pixel-art game draws at,
/// shown as large as the largest whole-number scale that fits, centered, with
/// bars of [`bar`](Self::bar) color around it. Every virtual pixel becomes
/// an exact square block of device pixels: no blur, no uneven rows.
///
/// [`paint`](Self::paint) draws a frame: the game draws into a [`Scene`] of
/// the virtual size, which is rasterized at that size (sprites drawn with
/// [`Scene::draw_image_region`] take the software renderer's fast blits) and
/// then blitted, nearest-neighbor, into the canvas.
/// [`to_virtual`](Self::to_virtual) turns a pointer position on the canvas
/// into virtual pixels.
///
/// ```
/// use stipple::game::PixelViewport;
/// use stipple::geometry::{Point, Rect, Size};
///
/// let vp = PixelViewport::new(320, 180);
/// // A 1000×600 canvas at 1×: 3× (960×540) fits, centered.
/// let l = vp.layout(Size::new(1000.0, 600.0), 1.0);
/// assert_eq!(l.factor, 3);
/// assert_eq!(l.rect, Rect::from_xywh(20.0, 30.0, 960.0, 540.0));
/// assert_eq!(vp.to_virtual(Point::new(23.5, 30.0), &l), Some(Point::new(1.1666666666666667, 0.0)));
/// assert_eq!(vp.to_virtual(Point::new(5.0, 5.0), &l), None); // on a bar
/// ```
#[derive(Debug)]
pub struct PixelViewport {
    width: u32,
    height: u32,
    /// The color of the bars around the picture.
    pub bar: Color,
    /// What the virtual frame is cleared to before the game draws.
    pub background: Color,
    last: Cell<Option<PixelLayout>>,
}

/// Where a [`PixelViewport`]'s picture lands on a canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelLayout {
    /// The picture, in the canvas's (logical) coordinates.
    pub rect: Rect,
    /// Device pixels per virtual pixel: a whole number when the canvas is at
    /// least the virtual size, else 0 (the picture is shrunk to fit).
    pub factor: u32,
    /// Logical units per virtual pixel.
    pub scale: f64,
}

impl PixelViewport {
    /// A `width`×`height` virtual screen with black bars.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
            bar: Color::BLACK,
            background: Color::BLACK,
            last: Cell::new(None),
        }
    }

    /// Set the bars' color.
    pub fn bar_color(mut self, color: Color) -> Self {
        self.bar = color;
        self
    }

    /// Set the color the virtual frame is cleared to.
    pub fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    /// The virtual resolution.
    pub fn size(&self) -> Size {
        Size::new(self.width as f64, self.height as f64)
    }

    /// Where the picture goes on a canvas of `area` (logical units) shown at
    /// `scale` device pixels per logical unit: the largest whole-number
    /// scale (in device pixels) that fits, centered on whole device pixels.
    pub fn layout(&self, area: Size, scale: f64) -> PixelLayout {
        let scale = if scale > 0.0 && scale.is_finite() {
            scale
        } else {
            1.0
        };
        let (dw, dh) = (area.width * scale, area.height * scale);
        let (vw, vh) = (self.width as f64, self.height as f64);
        let fit = (dw / vw).min(dh / vh);
        let (factor, k) = if fit >= 1.0 {
            let f = fit.floor();
            (f as u32, f)
        } else {
            (0, fit.max(0.0))
        };
        let (pw, ph) = (vw * k, vh * k);
        let x = ((dw - pw) / 2.0).floor().max(0.0);
        let y = ((dh - ph) / 2.0).floor().max(0.0);
        PixelLayout {
            rect: Rect::from_xywh(x / scale, y / scale, pw / scale, ph / scale),
            factor,
            scale: k / scale,
        }
    }

    /// The virtual pixel position under `p` (canvas coordinates), or `None`
    /// on the bars.
    pub fn to_virtual(&self, p: Point, layout: &PixelLayout) -> Option<Point> {
        let r = layout.rect;
        if layout.scale <= 0.0 || !r.contains(p) {
            return None;
        }
        Some(Point::new(
            (p.x - r.min_x()) / layout.scale,
            (p.y - r.min_y()) / layout.scale,
        ))
    }

    /// [`to_virtual`](Self::to_virtual) with the layout of the last
    /// [`paint`](Self::paint): what a pointer handler on the canvas calls.
    pub fn pointer_to_virtual(&self, p: Point) -> Option<Point> {
        self.to_virtual(p, &self.last.get()?)
    }

    /// The layout the last [`paint`](Self::paint) used.
    pub fn last_layout(&self) -> Option<PixelLayout> {
        self.last.get()
    }

    /// Rasterize a frame at the virtual resolution: `draw` draws into a scene
    /// of [`size`](Self::size), cleared to [`background`](Self::background).
    pub fn render(&self, draw: impl FnOnce(&mut Scene)) -> Pixmap {
        let mut scene = Scene::new(self.size());
        draw(&mut scene);
        SoftwareRenderer::new()
            .with_background(self.background)
            .render_at(scene, PhysicalSize::new(self.width, self.height))
    }

    /// Show `frame` (a picture at the virtual resolution) on `scene` (a
    /// canvas): letterboxed by the bars, scaled nearest-neighbor.
    pub fn present(&self, scene: &mut Scene, frame: &Arc<Pixmap>) -> PixelLayout {
        let area = scene.logical_size();
        let layout = self.layout(area, scene.scale());
        self.last.set(Some(layout));
        let r = layout.rect;
        let bars = [
            Rect::from_xywh(0.0, 0.0, area.width, r.min_y()),
            Rect::from_xywh(0.0, r.max_y(), area.width, area.height - r.max_y()),
            Rect::from_xywh(0.0, r.min_y(), r.min_x(), r.height()),
            Rect::from_xywh(r.max_x(), r.min_y(), area.width - r.max_x(), r.height()),
        ];
        for bar in bars {
            if bar.width() > 0.0 && bar.height() > 0.0 {
                scene.fill_rect(bar, self.bar);
            }
        }
        scene.draw_image_with(r, frame, &ImageOptions::new().nearest());
        layout
    }

    /// Draw a frame: [`render`](Self::render) it at the virtual resolution
    /// with `draw`, then [`present`](Self::present) it on `scene`.
    ///
    /// ```
    /// use stipple::game::PixelViewport;
    /// use stipple::geometry::{Rect, ScaleFactor, Size};
    /// use stipple::render::{Color, Scene, SoftwareRenderer};
    ///
    /// let vp = PixelViewport::new(4, 2).bar_color(Color::rgb(0, 0, 255));
    /// let mut scene = Scene::new(Size::new(10.0, 6.0));
    /// vp.paint(&mut scene, |s| {
    ///     s.fill_rect(Rect::from_xywh(0.0, 0.0, 1.0, 1.0), Color::rgb(255, 0, 0));
    /// });
    /// let out = SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);
    /// // 2× fits: the picture is 8×4 at (1, 1); its first pixel is a 2×2 block.
    /// assert_eq!(out.pixel(0, 0), Some([0, 0, 255, 255]));
    /// assert_eq!(out.pixel(2, 2), Some([255, 0, 0, 255]));
    /// assert_eq!(out.pixel(3, 2), Some([0, 0, 0, 255]));
    /// ```
    pub fn paint(&self, scene: &mut Scene, draw: impl FnOnce(&mut Scene)) -> PixelLayout {
        let frame = Arc::new(self.render(draw));
        self.present(scene, &frame)
    }
}
