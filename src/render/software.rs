//! The software rendering backend: rasterizes a [`Scene`] on the CPU via
//! `oxideav-raster` into a [`Pixmap`] ready for any [`Surface`] to present.

use crate::geometry::{PhysicalSize, Rect, ScaleFactor};
use crate::render::{Color, Pixmap, Scene};
use oxideav_raster::Renderer;
use std::sync::atomic::{AtomicU64, Ordering};

/// Pixels rasterized and rasterizer runs since the process started (or the
/// last [`RasterStats::take`]).
static PIXELS: AtomicU64 = AtomicU64::new(0);
static RUNS: AtomicU64 = AtomicU64::new(0);

/// How much CPU rasterizing has happened: every [`SoftwareRenderer`] render
/// (full, area or band) adds its pixels. A diagnostic: an app can check that
/// a change re-rasterized only the area it touched.
///
/// ```
/// use stipple::geometry::{Rect, ScaleFactor, Size};
/// use stipple::render::{Color, RasterStats, Scene, SoftwareRenderer};
///
/// RasterStats::take();
/// let mut scene = Scene::new(Size::new(10.0, 10.0));
/// scene.fill_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), Color::WHITE);
/// SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);
/// assert!(RasterStats::take().pixels >= 100);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RasterStats {
    /// Pixels rasterized.
    pub pixels: u64,
    /// Rasterizer runs (a full frame, one area of an area repaint, or one band
    /// of a banded full frame).
    pub runs: u64,
}

impl RasterStats {
    /// The totals so far, without resetting them.
    pub fn get() -> Self {
        Self {
            pixels: PIXELS.load(Ordering::Relaxed),
            runs: RUNS.load(Ordering::Relaxed),
        }
    }

    /// The totals so far, resetting them to zero.
    pub fn take() -> Self {
        Self {
            pixels: PIXELS.swap(0, Ordering::Relaxed),
            runs: RUNS.swap(0, Ordering::Relaxed),
        }
    }
}

/// A full frame with more primitives than this (or a canvas) is rasterized in
/// horizontal bands of [`BAND`] rows, each with only what crosses it: the
/// rasterizer's cost per primitive grows with the canvas it draws on, so a
/// screenful of glyphs is far cheaper band by band.
const BAND_PRIMITIVES: usize = 512;
const BAND: u32 = 32;

/// Rasterizes scenes on the CPU. Cheap to construct and reusable across
/// frames; the underlying `oxideav` renderer also caches per-subtree work.
#[derive(Debug, Default)]
pub struct SoftwareRenderer {
    background: Color,
}

impl SoftwareRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the canvas clear color (default: transparent).
    pub fn with_background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    /// Rasterize `scene` at the given `scale`, returning a physical-pixel
    /// [`Pixmap`].
    ///
    /// The scene's logical size combined with `scale` determines the output
    /// resolution; the rasterizer maps logical → physical via the frame's
    /// view box, so widgets are authored once in logical pixels and stay
    /// crisp at any DPI.
    pub fn render(&self, scene: Scene, scale: ScaleFactor) -> Pixmap {
        let physical = scale.to_physical(scene.logical_size());
        self.render_at(scene, physical)
    }

    /// Rasterize `scene` into a buffer of exactly `physical` pixels.
    ///
    /// A heavy scene (many primitives, or a canvas) is rasterized in bands,
    /// each with only what crosses it; the result is the same.
    pub fn render_at(&self, scene: Scene, physical: PhysicalSize) -> Pixmap {
        let heavy = scene.primitive_count() > BAND_PRIMITIVES || scene.has_canvas();
        if !heavy || physical.height <= BAND * 2 {
            let frame = scene.into_vector_frame();
            return self.rasterize_frame(frame, physical);
        }
        let logical = scene.logical_size();
        let (w, h) = (physical.width.max(1), physical.height.max(1));
        let (sx, sy) = (logical.width / w as f64, logical.height / h as f64);
        let mut out = Pixmap::new(PhysicalSize::new(w, h));
        let mut y = 0;
        while y < h {
            let bh = BAND.min(h - y);
            let view = Rect::from_xywh(0.0, y as f64 * sy, w as f64 * sx, bh as f64 * sy);
            let band = self.render_area(&scene, view, PhysicalSize::new(w, bh));
            out.blit(&band, 0, y);
            y += bh;
        }
        out
    }

    /// Rasterize only `view` — a logical sub-rect of `scene` — into a buffer of
    /// exactly `physical` pixels, like [`render_region`](Self::render_region)
    /// but without consuming the scene, so an area repaint of several rects
    /// lowers one scene several times instead of cloning it for each.
    pub fn render_area(&self, scene: &Scene, view: Rect, physical: PhysicalSize) -> Pixmap {
        self.rasterize_frame(scene.vector_frame_region(view), physical)
    }

    /// Rasterize only `view` — a logical sub-rect of `scene` — into a buffer of
    /// exactly `physical` pixels, which the caller picks to match the integer
    /// device-pixel rect it will blit/present (so there is no resampling seam).
    ///
    /// Pixels outside `view` are clipped by the canvas bounds; the background
    /// fill makes the region opaque, so the result can be composited with a
    /// straight copy (see [`Pixmap::blit`]). This is the area-repaint path: a
    /// hover change re-rasterizes two small button rects instead of the window.
    pub fn render_region(&self, scene: Scene, view: Rect, physical: PhysicalSize) -> Pixmap {
        let frame = scene.into_vector_frame_region(view);
        self.rasterize_frame(frame, physical)
    }

    /// Run a lowered frame through the rasterizer and copy the single packed
    /// RGBA plane into a tightly-packed [`Pixmap`] of `physical` pixels.
    fn rasterize_frame(&self, frame: oxideav_core::VectorFrame, physical: PhysicalSize) -> Pixmap {
        let (w, h) = (physical.width.max(1), physical.height.max(1));
        PIXELS.fetch_add(w as u64 * h as u64, Ordering::Relaxed);
        RUNS.fetch_add(1, Ordering::Relaxed);
        let mut renderer = Renderer::new(w, h);
        renderer.background = self.background.to_oxideav();
        let video = renderer.render(&frame);

        // `render` always produces a single packed-RGBA plane with
        // `stride == w * 4`; copy it into a tightly-packed Pixmap.
        let plane = &video.planes[0];
        let dst_stride = w as usize * 4;
        let total = dst_stride * h as usize;
        let mut data = vec![0u8; total];
        if plane.stride == dst_stride {
            data.copy_from_slice(&plane.data[..total]);
        } else {
            for y in 0..h as usize {
                let src = &plane.data[y * plane.stride..y * plane.stride + dst_stride];
                data[y * dst_stride..(y + 1) * dst_stride].copy_from_slice(src);
            }
        }
        Pixmap::from_rgba8(PhysicalSize::new(w, h), data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Rect, Size};

    #[test]
    fn fills_pixel_at_expected_location() {
        // 100x100 logical, 1x scale. Fill the whole canvas red, then check a
        // center pixel made it through the oxideav rasterizer.
        let mut scene = Scene::new(Size::new(100.0, 100.0));
        scene.fill_rect(
            Rect::from_xywh(0.0, 0.0, 100.0, 100.0),
            Color::rgb(255, 0, 0),
        );
        let pm = SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);

        assert_eq!(pm.size(), PhysicalSize::new(100, 100));
        let [r, g, b, a] = pm.pixel(50, 50).unwrap();
        assert_eq!((r, g, b, a), (255, 0, 0, 255));
    }

    #[test]
    fn hidpi_scale_doubles_resolution() {
        let scene = Scene::new(Size::new(100.0, 80.0));
        let pm = SoftwareRenderer::new().render(scene, ScaleFactor::new(2.0));
        assert_eq!(pm.size(), PhysicalSize::new(200, 160));
    }

    #[test]
    fn render_region_matches_full_render_within_the_region() {
        // A scene with two distinct colored squares far apart.
        let make = || {
            let mut s = Scene::new(Size::new(100.0, 100.0));
            s.fill_rect(
                Rect::from_xywh(10.0, 10.0, 20.0, 20.0),
                Color::rgb(255, 0, 0),
            );
            s.fill_rect(
                Rect::from_xywh(70.0, 70.0, 20.0, 20.0),
                Color::rgb(0, 0, 255),
            );
            s
        };
        let r = SoftwareRenderer::new().with_background(Color::rgb(0, 0, 0));
        let full = r.render(make(), ScaleFactor::IDENTITY);

        // Re-render just the 1x-scale region around the red square.
        let view = Rect::from_xywh(10.0, 10.0, 20.0, 20.0);
        let region = r.render_region(make(), view, PhysicalSize::new(20, 20));
        assert_eq!(region.size(), PhysicalSize::new(20, 20));

        // Every region pixel equals the full render at the same absolute spot.
        for y in 0..20u32 {
            for x in 0..20u32 {
                assert_eq!(
                    region.pixel(x, y),
                    full.pixel(10 + x, 10 + y),
                    "mismatch at region ({x},{y})"
                );
            }
        }
        // And the region actually captured the red square, not background.
        assert_eq!(region.pixel(10, 10), Some([255, 0, 0, 255]));
    }

    /// A heavy scene is rasterized band by band, each band with only what
    /// crosses it, and the result is the one-pass result — at a
    /// fractional scale, with anti-aliased edges, strokes and glyphs
    /// straddling the bands.
    #[test]
    fn banded_render_matches_one_pass() {
        use crate::geometry::Point;
        use crate::render::{Path, StrokeStyle};
        let font = crate::render::Font::system_default();
        let make = || {
            let mut s = Scene::new(Size::new(120.0, 200.0));
            for i in 0..600 {
                let (x, y) = ((i % 20) as f64 * 6.1, (i / 20) as f64 * 6.7);
                let c = Color::rgb((i * 7) as u8, (i * 13) as u8, 90);
                s.fill_round_rect(Rect::from_xywh(x + 0.3, y + 0.2, 5.2, 5.9), 1.5, c);
            }
            let mut wave = Path::new();
            wave.move_to(Point::new(0.0, 60.0));
            wave.cubic_to(
                Point::new(40.0, 20.0),
                Point::new(80.0, 120.0),
                Point::new(120.0, 64.0),
            );
            s.stroke_path(
                &wave,
                Color::WHITE,
                &StrokeStyle::new(2.5).dashed(&[3.0, 2.0]),
            );
            if let Some(font) = &font {
                s.fill_text(font, "Bands", Point::new(7.3, 41.6), 19.0, Color::BLACK);
            }
            s
        };
        assert!(make().primitive_count() > BAND_PRIMITIVES);
        let r = SoftwareRenderer::new().with_background(Color::rgb(30, 30, 30));
        let scale = ScaleFactor::new(1.5);
        let physical = scale.to_physical(Size::new(120.0, 200.0));
        let banded = r.render(make(), scale);
        let one_pass = r.rasterize_frame(make().into_vector_frame(), physical);
        assert_eq!(banded.size(), one_pass.size());
        // Equal up to float rounding: each band maps logical to physical
        // pixels through its own offset, which can move an edge's coverage by
        // a hair.
        let mut worst = 0;
        for y in 0..physical.height {
            for x in 0..physical.width {
                let (a, b) = (banded.pixel(x, y).unwrap(), one_pass.pixel(x, y).unwrap());
                for c in 0..4 {
                    worst = worst.max(a[c].abs_diff(b[c]));
                }
            }
        }
        assert!(worst <= 3, "banded and one-pass differ by up to {worst}");
    }
}
