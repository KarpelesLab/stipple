//! Nine-slice images (nine-patch, CSS `border-image`) and image backgrounds:
//! a framed bitmap stretched to any size with its corners kept as drawn. See
//! [`NineSlice`], [`Scene::draw_nine_slice`] and [`BackgroundImage`].

use crate::geometry::{Insets, Rect};
use crate::render::{Color, Filter, ImageFit, ImageOptions, ImageSource, Pixmap, Scene};
use std::sync::Arc;

/// How a [`NineSlice`]'s edges or center cover the length they are given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SliceFill {
    /// Stretched to the length (the default).
    #[default]
    Stretch,
    /// Repeated at its own size from the start; the last copy is cut short.
    Tile,
    /// Repeated a whole number of times, each copy stretched or squeezed a
    /// little so they fit exactly (CSS's `round`): what an ornate border
    /// whose pattern mustn't be cut wants.
    Round,
    /// Not drawn: a frame with a hollow middle.
    Hidden,
}

/// A bitmap cut into nine parts by `insets`: the four corners are drawn at
/// their own size, the edges are stretched (or tiled) along their length, and
/// the center fills what is left. The frame of a panel, a button, a dialogue
/// box or a bar drawn once, at any size.
///
/// The insets and [`region`](Self::region) are in the bitmap's pixels.
/// [`scale`](Self::scale) is how many logical pixels one of them covers: 1 by
/// default; 2 or 3 for small pixel art (with [`nearest`](Self::nearest)
/// sampling); 0.5 for an image drawn at twice the size for high-density
/// screens. Draw one with [`Scene::draw_nine_slice`], or make it an element's
/// background with [`Element::background_image`](crate::core::Element::background_image).
///
/// ```
/// use std::sync::Arc;
/// use stipple::geometry::{Insets, PhysicalSize, Rect};
/// use stipple::render::{NineSlice, Pixmap, SliceFill};
///
/// let frame = Arc::new(Pixmap::new(PhysicalSize::new(48, 48)));
/// let slice = NineSlice::new(frame, Insets::uniform(12.0))
///     .region(Rect::from_xywh(0.0, 0.0, 24.0, 24.0)) // a part of an atlas
///     .edges(SliceFill::Tile)
///     .scale(2.0)
///     .nearest();
/// // An element with it as its background keeps its content off the frame.
/// assert_eq!(slice.content_padding(), Insets::uniform(24.0));
/// ```
///
/// Equality compares the bitmap by identity (the same [`Arc`]), not pixels.
#[derive(Clone)]
#[non_exhaustive]
pub struct NineSlice {
    /// The bitmap (or the atlas holding it).
    pub image: Arc<Pixmap>,
    /// The part of [`image`](Self::image) to slice, in its pixels; `None` for
    /// all of it.
    pub region: Option<Rect>,
    /// Where the cuts are, in source pixels from each side of the region.
    pub insets: Insets,
    /// How the four edges cover their length.
    pub edges: SliceFill,
    /// How the center covers the middle.
    pub center: SliceFill,
    /// Logical pixels per source pixel.
    pub scale: f64,
    /// How the bitmap is sampled when it is scaled.
    pub filter: Filter,
    /// Multiplies every pixel (white leaves them as they are).
    pub tint: Color,
    /// Fades the whole image, `0.0..=1.0`.
    pub opacity: f32,
    /// How far an element with this background keeps its content from its
    /// edges, in logical pixels; `None` for the scaled insets.
    pub padding: Option<Insets>,
}

impl std::fmt::Debug for NineSlice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NineSlice")
            .field("image", &self.image.size())
            .field("region", &self.region)
            .field("insets", &self.insets)
            .field("edges", &self.edges)
            .field("center", &self.center)
            .field("scale", &self.scale)
            .field("filter", &self.filter)
            .field("tint", &self.tint)
            .field("opacity", &self.opacity)
            .field("padding", &self.padding)
            .finish()
    }
}

impl PartialEq for NineSlice {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.image, &other.image)
            && self.region == other.region
            && self.insets == other.insets
            && self.edges == other.edges
            && self.center == other.center
            && self.scale == other.scale
            && self.filter == other.filter
            && self.tint == other.tint
            && self.opacity == other.opacity
            && self.padding == other.padding
    }
}

impl NineSlice {
    /// All of `image`, cut `insets` source pixels in from each side, its
    /// edges and center stretched, at one logical pixel per source pixel.
    pub fn new(image: impl Into<Arc<Pixmap>>, insets: Insets) -> Self {
        Self {
            image: image.into(),
            region: None,
            insets,
            edges: SliceFill::Stretch,
            center: SliceFill::Stretch,
            scale: 1.0,
            filter: Filter::Linear,
            tint: Color::WHITE,
            opacity: 1.0,
            padding: None,
        }
    }

    /// Slice only `region` of the image (in its pixels): one frame of an
    /// atlas holding many.
    pub fn region(mut self, region: Rect) -> Self {
        self.region = Some(region);
        self
    }

    /// Cover the edges' length with `fill`.
    pub fn edges(mut self, fill: SliceFill) -> Self {
        self.edges = fill;
        self
    }

    /// Cover the center with `fill` ([`SliceFill::Hidden`] for a hollow frame).
    pub fn center(mut self, fill: SliceFill) -> Self {
        self.center = fill;
        self
    }

    /// Draw `scale` logical pixels per source pixel.
    pub fn scale(mut self, scale: f64) -> Self {
        self.scale = scale;
        self
    }

    /// Sample with `filter`.
    pub fn filter(mut self, filter: Filter) -> Self {
        self.filter = filter;
        self
    }

    /// Sample nearest neighbor: pixel art stays hard-edged when scaled.
    pub fn nearest(self) -> Self {
        self.filter(Filter::Nearest)
    }

    /// Multiply every pixel by `color` (one gray frame, tinted per use).
    pub fn tint(mut self, color: Color) -> Self {
        self.tint = color;
        self
    }

    /// Fade the image to `opacity`.
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    /// Keep an element's content `padding` (logical pixels) from its edges,
    /// instead of the scaled insets: for a frame whose drawn border is thinner
    /// or thicker than its cuts.
    pub fn padding(mut self, padding: Insets) -> Self {
        self.padding = Some(padding);
        self
    }

    /// How far an element with this background keeps its content from its
    /// edges: [`padding`](Self::padding) if set, else the insets times the
    /// scale.
    pub fn content_padding(&self) -> Insets {
        self.padding.unwrap_or_else(|| {
            let s = self.scale.max(0.0);
            Insets::new(
                self.insets.top * s,
                self.insets.right * s,
                self.insets.bottom * s,
                self.insets.left * s,
            )
        })
    }

    /// The source region in whole pixels, inside the image.
    fn source(&self) -> [f64; 4] {
        let size = self.image.size();
        let (w, h) = (size.width as f64, size.height as f64);
        let r = self
            .region
            .unwrap_or_else(|| Rect::from_xywh(0.0, 0.0, w, h));
        [
            r.min_x().round().clamp(0.0, w),
            r.min_y().round().clamp(0.0, h),
            r.max_x().round().clamp(0.0, w),
            r.max_y().round().clamp(0.0, h),
        ]
    }

    fn options(&self) -> ImageOptions {
        ImageOptions::new()
            .filter(self.filter)
            .tint(self.tint)
            .opacity(self.opacity)
    }
}

/// The most pieces a tiled edge is cut into; past it the tiles grow.
const MAX_TILES: usize = 1024;
/// The most pieces a tiled center is cut into.
const MAX_CENTER_TILES: usize = 16384;

/// One piece of an axis: source `s0..s1` drawn over `d0..d1`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Span {
    s0: f64,
    s1: f64,
    d0: f64,
    d1: f64,
}

/// Cut the source span `s0..s1` drawn over `d0..d1` into pieces per `fill`,
/// a source pixel being `unit` logical pixels at full size, at most `max`
/// pieces. Boundaries are snapped to `snap`.
fn spans(
    (s0, s1): (f64, f64),
    (d0, d1): (f64, f64),
    unit: f64,
    fill: SliceFill,
    max: usize,
    snap: &impl Fn(f64) -> f64,
) -> Vec<Span> {
    let (n, len) = (s1 - s0, d1 - d0);
    if n <= 0.0 || len <= 0.0 {
        return Vec::new();
    }
    let tile = n * unit;
    let whole = Span { s0, s1, d0, d1 };
    if fill == SliceFill::Stretch || tile <= 0.0 || !tile.is_finite() {
        return vec![whole];
    }
    let max = max.max(1);
    if fill == SliceFill::Round {
        let count = ((len / tile).round() as usize).clamp(1, max);
        let step = len / count as f64;
        return (0..count)
            .map(|i| Span {
                s0,
                s1,
                d0: snap(d0 + step * i as f64),
                d1: if i + 1 == count {
                    d1
                } else {
                    snap(d0 + step * (i + 1) as f64)
                },
            })
            .filter(|s| s.d1 > s.d0)
            .collect();
    }
    // Tile: whole copies from the start, the last cut short; too many, and
    // the tiles grow (stretched) to keep to `max`.
    let tile = if (len / tile).ceil() as usize > max {
        len / max as f64
    } else {
        tile
    };
    let mut out = Vec::new();
    let mut at = d0;
    while at < d1 - 1e-9 && out.len() < max + 1 {
        let end = (at + tile).min(d1);
        let (a, b) = (snap(at), if end >= d1 { d1 } else { snap(end) });
        // A cut-short copy shows the start of the source, as much of it as
        // fits (at least a pixel).
        let part = ((end - at) / tile * n).round().clamp(1.0, n);
        if b > a {
            out.push(Span {
                s0,
                s1: s0 + part,
                d0: a,
                d1: b,
            });
        }
        at = end;
    }
    out
}

impl Scene {
    /// Draw `slice` (see [`NineSlice`]) over `dst`: the corners at their size
    /// (shrunk together, keeping their shape, when `dst` is smaller than
    /// two of them), the edges and center covering the rest per
    /// [`edges`](NineSlice::edges) and [`center`](NineSlice::center).
    ///
    /// Each piece is an [`draw_image_region`](Self::draw_image_region) whose
    /// edges land on whole physical pixels (at [`scale`](Self::scale)), so the
    /// pieces meet without seams. On the software renderer an unscaled or
    /// whole-number-scaled slice takes the region blit's fast path; on the GPU
    /// each piece is a textured quad.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use stipple::geometry::{Insets, PhysicalSize, Rect, ScaleFactor, Size};
    /// use stipple::render::{NineSlice, Pixmap, Scene, SoftwareRenderer};
    ///
    /// // A 3×3 frame: a red ring around a white middle.
    /// let mut px = vec![255, 0, 0, 255].repeat(9);
    /// px[16..20].copy_from_slice(&[255; 4]);
    /// let frame = Arc::new(Pixmap::from_rgba8(PhysicalSize::new(3, 3), px));
    /// let mut scene = Scene::new(Size::new(20.0, 10.0));
    /// let slice = NineSlice::new(frame, Insets::uniform(1.0));
    /// scene.draw_nine_slice(Rect::from_xywh(0.0, 0.0, 20.0, 10.0), &slice);
    /// let out = SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);
    /// assert_eq!(out.pixel(0, 5), Some([255, 0, 0, 255])); // the left edge
    /// assert_eq!(out.pixel(10, 5), Some([255, 255, 255, 255])); // the middle
    /// ```
    pub fn draw_nine_slice(&mut self, dst: Rect, slice: &NineSlice) {
        if dst.width() <= 0.0 || dst.height() <= 0.0 || slice.opacity <= 0.0 {
            return;
        }
        let unit = if slice.scale.is_finite() && slice.scale > 0.0 {
            slice.scale
        } else {
            1.0
        };
        let [sx0, sy0, sx1, sy1] = slice.source();
        let (sw, sh) = (sx1 - sx0, sy1 - sy0);
        if sw <= 0.0 || sh <= 0.0 {
            return;
        }
        // The cuts, inside the region (two that overlap share it).
        let fit = |a: f64, b: f64, room: f64| {
            let (a, b) = (a.max(0.0).round(), b.max(0.0).round());
            if a + b > room {
                let a = (a * room / (a + b)).floor();
                (a, room - a)
            } else {
                (a, b)
            }
        };
        let (l, r) = fit(slice.insets.left, slice.insets.right, sw);
        let (t, b) = fit(slice.insets.top, slice.insets.bottom, sh);
        // The corners on screen, all shrunk by one factor if they don't fit.
        let (mut cl, mut cr, mut ct, mut cb) = (l * unit, r * unit, t * unit, b * unit);
        let shrink = [
            if cl + cr > dst.width() {
                dst.width() / (cl + cr)
            } else {
                1.0
            },
            if ct + cb > dst.height() {
                dst.height() / (ct + cb)
            } else {
                1.0
            },
        ]
        .into_iter()
        .fold(1.0f64, f64::min);
        cl *= shrink;
        cr *= shrink;
        ct *= shrink;
        cb *= shrink;
        let scale = self.scale();
        let px = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let snap = move |v: f64| (v * px).round() / px;
        let (x0, x3) = (snap(dst.min_x()), snap(dst.max_x()));
        let (y0, y3) = (snap(dst.min_y()), snap(dst.max_y()));
        let x1 = snap(x0 + cl).min(x3);
        let x2 = snap(x3 - cr).clamp(x1, x3);
        let y1 = snap(y0 + ct).min(y3);
        let y2 = snap(y3 - cb).clamp(y1, y3);
        let src_x = [(sx0, sx0 + l), (sx0 + l, sx1 - r), (sx1 - r, sx1)];
        let src_y = [(sy0, sy0 + t), (sy0 + t, sy1 - b), (sy1 - b, sy1)];
        let dst_x = [(x0, x1), (x1, x2), (x2, x3)];
        let dst_y = [(y0, y1), (y1, y2), (y2, y3)];
        let tile_unit = unit * shrink;
        let opts = slice.options();
        for j in 0..3 {
            for i in 0..3 {
                let center = i == 1 && j == 1;
                let fill = if center { slice.center } else { slice.edges };
                if fill == SliceFill::Hidden && (i == 1 || j == 1) {
                    continue;
                }
                // Only the middle column and row repeat, along their length.
                let (fx, fy) = (
                    if i == 1 { fill } else { SliceFill::Stretch },
                    if j == 1 { fill } else { SliceFill::Stretch },
                );
                let (max_x, max_y) = if center {
                    let side = (MAX_CENTER_TILES as f64).sqrt() as usize;
                    (side, side)
                } else {
                    (MAX_TILES, MAX_TILES)
                };
                let cols = spans(src_x[i], dst_x[i], tile_unit, fx, max_x, &snap);
                let rows = spans(src_y[j], dst_y[j], tile_unit, fy, max_y, &snap);
                for row in &rows {
                    for col in &cols {
                        let src = Rect::from_xywh(col.s0, row.s0, col.s1 - col.s0, row.s1 - row.s0);
                        let to = Rect::from_xywh(col.d0, row.d0, col.d1 - col.d0, row.d1 - row.d0);
                        self.draw_image_region(src, to, &slice.image, &opts);
                    }
                }
            }
        }
    }

    /// Draw `background` over `bounds`: a nine-slice stretched to it, or an
    /// image placed per its fit.
    pub fn draw_background(&mut self, bounds: Rect, background: &BackgroundImage) {
        match background {
            BackgroundImage::NineSlice(slice) => self.draw_nine_slice(bounds, slice),
            BackgroundImage::Image { source, fit } => self.draw_image_source(bounds, source, *fit),
        }
    }
}

/// An image drawn behind an element's content, over its fill (see
/// [`Element::background_image`](crate::core::Element::background_image)):
/// a [`NineSlice`] frame stretched to the element, or a plain image (a
/// painted screen behind a menu) placed per an [`ImageFit`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum BackgroundImage {
    /// A frame stretched to the element.
    NineSlice(NineSlice),
    /// A bitmap or SVG placed in the element's box per `fit`
    /// ([`ImageFit::Cover`] clips what overflows).
    Image { source: ImageSource, fit: ImageFit },
}

impl BackgroundImage {
    /// `source` placed in the element's box per `fit`.
    pub fn image(source: impl Into<ImageSource>, fit: ImageFit) -> Self {
        BackgroundImage::Image {
            source: source.into(),
            fit,
        }
    }

    /// How far an element with this background keeps its content from its
    /// edges: a nine-slice's [`content_padding`](NineSlice::content_padding),
    /// none for a plain image.
    pub fn content_padding(&self) -> Insets {
        match self {
            BackgroundImage::NineSlice(slice) => slice.content_padding(),
            BackgroundImage::Image { .. } => Insets::ZERO,
        }
    }
}

impl From<NineSlice> for BackgroundImage {
    fn from(slice: NineSlice) -> Self {
        BackgroundImage::NineSlice(slice)
    }
}

impl From<&NineSlice> for BackgroundImage {
    fn from(slice: &NineSlice) -> Self {
        BackgroundImage::NineSlice(slice.clone())
    }
}

impl From<ImageSource> for BackgroundImage {
    /// The image stretched over the element ([`ImageFit::Cover`]).
    fn from(source: ImageSource) -> Self {
        BackgroundImage::image(source, ImageFit::Cover)
    }
}

/// A test frame: `size`×`size` pixels, a ring `border` wide in `edge`, the
/// corners `corner`, the middle `middle`.
#[cfg(test)]
pub(crate) fn test_frame(size: u32, border: u32, colors: [[u8; 4]; 3]) -> Arc<Pixmap> {
    let [corner, edge, middle] = colors;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let ex = x < border || x >= size - border;
            let ey = y < border || y >= size - border;
            data.extend_from_slice(match (ex, ey) {
                (true, true) => &corner,
                (false, false) => &middle,
                _ => &edge,
            });
        }
    }
    Arc::new(Pixmap::from_rgba8(
        crate::geometry::PhysicalSize::new(size, size),
        data,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{PhysicalSize, ScaleFactor, Size};
    use crate::render::{DrawCmd, SoftwareRenderer};

    const C: [u8; 4] = [255, 0, 0, 255];
    const E: [u8; 4] = [0, 0, 255, 255];
    const M: [u8; 4] = [0, 255, 0, 255];
    const BG: [u8; 4] = [0, 0, 0, 255];

    fn render(slice: &NineSlice, dst: Rect, size: Size, scale: f64) -> Pixmap {
        let mut scene = Scene::new(size);
        scene.set_scale(scale);
        scene.draw_nine_slice(dst, slice);
        SoftwareRenderer::new()
            .background(Color::BLACK)
            .render(scene, ScaleFactor::new(scale))
    }

    fn regions(slice: &NineSlice, dst: Rect, scale: f64) -> Vec<(Rect, Rect)> {
        let mut scene = Scene::new(Size::new(400.0, 400.0));
        scene.set_scale(scale);
        scene.draw_nine_slice(dst, slice);
        scene
            .commands()
            .iter()
            .filter_map(|c| match c {
                DrawCmd::ImageRegion { src, dst, .. } => Some((*src, *dst)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn corners_stay_unscaled_and_edges_stretch() {
        // 8×8, a 2-pixel ring: corners 2×2 red, edges blue, middle green.
        let slice = NineSlice::new(test_frame(8, 2, [C, E, M]), Insets::uniform(2.0)).nearest();
        let out = render(
            &slice,
            Rect::from_xywh(4.0, 4.0, 40.0, 24.0),
            Size::new(48.0, 32.0),
            1.0,
        );
        // Corners: exactly 2×2.
        for (x, y) in [(4, 4), (5, 5), (42, 4), (43, 5), (4, 26), (43, 27)] {
            assert_eq!(out.pixel(x, y), Some(C), "corner at {x},{y}");
        }
        assert_eq!(out.pixel(6, 4), Some(E), "top edge starts after the corner");
        assert_eq!(out.pixel(41, 5), Some(E));
        assert_eq!(out.pixel(4, 6), Some(E), "left edge");
        assert_eq!(out.pixel(43, 25), Some(E), "right edge");
        assert_eq!(out.pixel(6, 6), Some(M), "the middle");
        assert_eq!(out.pixel(41, 25), Some(M));
        assert_eq!(out.pixel(3, 3), Some(BG), "nothing outside");
        assert_eq!(out.pixel(44, 28), Some(BG));
    }

    #[test]
    fn a_hollow_center_is_left_out() {
        let slice = NineSlice::new(test_frame(8, 2, [C, E, M]), Insets::uniform(2.0))
            .center(SliceFill::Hidden);
        let out = render(
            &slice,
            Rect::from_xywh(0.0, 0.0, 20.0, 20.0),
            Size::new(20.0, 20.0),
            1.0,
        );
        assert_eq!(out.pixel(10, 10), Some(BG));
        assert_eq!(out.pixel(10, 0), Some(E));
        assert_eq!(
            regions(&slice, Rect::from_xywh(0.0, 0.0, 20.0, 20.0), 1.0).len(),
            8
        );
    }

    #[test]
    fn tiled_edges_repeat_at_their_size_and_cut_the_last() {
        // A 6×3 strip: corners 1 wide, a 4-pixel edge pattern between.
        let mut px = Vec::new();
        for _y in 0..3 {
            for x in 0..6 {
                px.extend_from_slice(match x {
                    0 | 5 => &C,
                    1 => &M,
                    _ => &E,
                });
            }
        }
        let image = Arc::new(Pixmap::from_rgba8(PhysicalSize::new(6, 3), px));
        let slice = NineSlice::new(image, Insets::new(1.0, 1.0, 1.0, 1.0))
            .edges(SliceFill::Tile)
            .nearest();
        // 1 + 10 + 1 wide: the edge is two copies of 4 and a cut one of 2.
        let dst = Rect::from_xywh(0.0, 0.0, 12.0, 3.0);
        let top: Vec<_> = regions(&slice, dst, 1.0)
            .into_iter()
            .filter(|(_, d)| d.min_y() == 0.0 && d.min_x() >= 1.0 && d.max_x() <= 11.0)
            .collect();
        assert_eq!(top.len(), 3, "{top:?}");
        assert_eq!(top[0].1, Rect::from_xywh(1.0, 0.0, 4.0, 1.0));
        assert_eq!(top[2].0, Rect::from_xywh(1.0, 0.0, 2.0, 1.0), "cut short");
        assert_eq!(top[2].1, Rect::from_xywh(9.0, 0.0, 2.0, 1.0));
        let out = render(&slice, dst, Size::new(12.0, 3.0), 1.0);
        // The pattern starts again at each copy (green first).
        for x in [1, 5, 9] {
            assert_eq!(out.pixel(x, 0), Some(M), "copy at {x}");
            assert_eq!(out.pixel(x + 1, 0), Some(E));
        }
    }

    #[test]
    fn round_fits_whole_copies() {
        let slice = NineSlice::new(test_frame(6, 1, [C, E, M]), Insets::uniform(1.0))
            .edges(SliceFill::Round);
        // 4-pixel edges over 9: two copies of 4.5.
        let dst = Rect::from_xywh(0.0, 0.0, 11.0, 6.0);
        let top: Vec<_> = regions(&slice, dst, 2.0)
            .into_iter()
            .filter(|(_, d)| d.min_y() == 0.0 && d.min_x() >= 1.0 && d.max_x() <= 10.0)
            .collect();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].1.width() + top[1].1.width(), 9.0);
        assert!(top.iter().all(|(s, _)| s.width() == 4.0), "whole copies");
    }

    #[test]
    fn small_boxes_shrink_the_corners_together() {
        let slice = NineSlice::new(test_frame(8, 2, [C, E, M]), Insets::uniform(2.0)).scale(4.0);
        // Corners 8×8 each; a 10×40 box fits 5 + 5 across.
        let pieces = regions(&slice, Rect::from_xywh(0.0, 0.0, 10.0, 40.0), 1.0);
        let corner = pieces[0].1;
        assert_eq!((corner.width(), corner.height()), (5.0, 5.0));
        assert!(pieces.iter().all(|(_, d)| d.max_x() <= 10.0));
    }

    #[test]
    fn pieces_land_on_physical_pixels_at_any_scale() {
        let slice = NineSlice::new(test_frame(9, 3, [C, E, M]), Insets::uniform(3.0)).scale(0.5);
        for scale in [1.0, 1.5, 2.0, 3.0] {
            for (_, d) in regions(&slice, Rect::from_xywh(0.3, 0.7, 31.3, 17.9), scale) {
                for v in [d.min_x(), d.min_y(), d.max_x(), d.max_y()] {
                    let p = v * scale;
                    assert!((p - p.round()).abs() < 1e-9, "{v} at {scale}");
                }
            }
        }
    }

    #[test]
    fn hi_dpi_pixel_art_doubles_cleanly() {
        // A 2× pixel-art frame on a 2× display: each source pixel is 4×4
        // physical pixels.
        let slice = NineSlice::new(test_frame(6, 1, [C, E, M]), Insets::uniform(1.0))
            .scale(2.0)
            .nearest();
        let out = render(
            &slice,
            Rect::from_xywh(0.0, 0.0, 20.0, 12.0),
            Size::new(20.0, 12.0),
            2.0,
        );
        for (x, y) in [(0, 0), (3, 3), (36, 0), (39, 3)] {
            assert_eq!(out.pixel(x, y), Some(C), "{x},{y}");
        }
        assert_eq!(out.pixel(4, 0), Some(E));
        assert_eq!(out.pixel(0, 4), Some(E));
        assert_eq!(out.pixel(4, 4), Some(M));
        assert_eq!(out.pixel(35, 19), Some(M));
        assert_eq!(out.pixel(35, 20), Some(E));
    }

    #[test]
    fn an_atlas_region_is_sliced_alone() {
        // Two 4×4 frames side by side; slice the second.
        let a = test_frame(4, 1, [C, E, M]);
        let b = test_frame(4, 1, [M, C, E]);
        let mut atlas = Pixmap::new(PhysicalSize::new(8, 4));
        atlas.blit(&a, 0, 0);
        atlas.blit(&b, 4, 0);
        let slice = NineSlice::new(atlas, Insets::uniform(1.0))
            .region(Rect::from_xywh(4.0, 0.0, 4.0, 4.0))
            .nearest();
        let out = render(
            &slice,
            Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            Size::new(10.0, 10.0),
            1.0,
        );
        assert_eq!(out.pixel(0, 0), Some(M));
        assert_eq!(out.pixel(5, 0), Some(C));
        assert_eq!(out.pixel(5, 5), Some(E));
    }

    #[test]
    fn content_padding_and_equality() {
        let image = test_frame(4, 1, [C, E, M]);
        let s = NineSlice::new(image.clone(), Insets::new(1.0, 2.0, 3.0, 4.0)).scale(2.0);
        assert_eq!(s.content_padding(), Insets::new(2.0, 4.0, 6.0, 8.0));
        let p = s.clone().padding(Insets::uniform(5.0));
        assert_eq!(p.content_padding(), Insets::uniform(5.0));
        assert_eq!(s, s.clone());
        assert_ne!(
            s,
            NineSlice::new(test_frame(4, 1, [C, E, M]), s.insets).scale(2.0)
        );
        let bg = BackgroundImage::from(&s);
        assert_eq!(bg.content_padding(), s.content_padding());
        let img = BackgroundImage::image(ImageSource::from(&image), ImageFit::Cover);
        assert_eq!(img.content_padding(), Insets::ZERO);
    }
}
