//! Images: decoding bitmaps into a [`Pixmap`], converting to and from
//! `oxideav` frames, rasterizing SVG documents, and the fit math for drawing
//! either into a box.
//!
//! All decoding goes through the `oxideav` stack: bitmaps through the
//! `oxideav-image` gateway over a format registry (see [`image_formats`]), SVG
//! through `oxideav-svg`. Bitmaps decode once ([`Pixmap::decode`]) and are
//! shared by [`Arc`]. An [`Svg`] stays vector: it is rasterized at the physical
//! size it is shown at (so it is sharp at any scale) and the last raster is
//! cached. Both are drawn through [`Scene::draw_image_source`], which places
//! them per an [`ImageFit`].

use crate::geometry::{PhysicalSize, Rect, Size};
use crate::render::{Pixmap, Scene};
use oxideav_core::{PixelFormat, RuntimeContext, VectorFrame, VideoFrame, VideoPlane, ViewBox};
use oxideav_raster::{AspectRatioAlign, MeetOrSlice, PreserveAspectRatio, Renderer};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

/// Why an image couldn't be loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// The bytes aren't a format any registered `oxideav` crate recognizes.
    UnknownFormat,
    /// The format was recognized but the data is malformed or unsupported, or
    /// a frame couldn't be converted to RGBA.
    Decode(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::UnknownFormat => f.write_str("unrecognized image format"),
            ImageError::Decode(e) => write!(f, "image decode failed: {e}"),
        }
    }
}

impl std::error::Error for ImageError {}

impl From<oxideav_image::ImageError> for ImageError {
    fn from(e: oxideav_image::ImageError) -> Self {
        match e {
            oxideav_image::ImageError::UnknownFormat(_) => ImageError::UnknownFormat,
            e => ImageError::Decode(e.to_string()),
        }
    }
}

/// The `oxideav` format registry [`Pixmap::decode`] uses: PNG / APNG
/// (`oxideav-png`), JPEG (`oxideav-mjpeg`), GIF (`oxideav-gif`), WebP
/// (`oxideav-webp`) and BMP (`oxideav-bmp`). Built once, on first use.
///
/// To decode other formats the stack supports (HEIF, AVIF, TIFF, …), build your
/// own [`RuntimeContext`], register those crates into it, and call
/// [`Pixmap::decode_with`].
pub fn image_formats() -> &'static RuntimeContext {
    static FORMATS: OnceLock<RuntimeContext> = OnceLock::new();
    FORMATS.get_or_init(|| {
        let mut ctx = RuntimeContext::new();
        oxideav_png::register(&mut ctx);
        oxideav_mjpeg::register(&mut ctx);
        oxideav_gif::register(&mut ctx);
        oxideav_webp::register(&mut ctx);
        oxideav_bmp::register(&mut ctx);
        ctx
    })
}

impl Pixmap {
    /// Decode an image file held in memory into a straight-alpha RGBA pixmap,
    /// sniffing the format through the `oxideav` registry (see
    /// [`image_formats`] for the formats). Animated formats yield their first
    /// frame; images without alpha decode fully opaque.
    ///
    /// ```
    /// # use stipple::render::{ImageError, Pixmap};
    /// assert_eq!(Pixmap::decode(b"not an image"), Err(ImageError::UnknownFormat));
    /// ```
    pub fn decode(bytes: &[u8]) -> Result<Pixmap, ImageError> {
        Self::decode_with(image_formats(), bytes)
    }

    /// [`decode`](Pixmap::decode) through a caller-built `oxideav` registry, for
    /// formats beyond the built-in set.
    pub fn decode_with(formats: &RuntimeContext, bytes: &[u8]) -> Result<Pixmap, ImageError> {
        let file = oxideav_image::decode_bytes(formats, bytes)?;
        Self::from_oxideav_image(file.primary())
    }

    /// Convert a decoded `oxideav-image` picture (any pixel layout the stack
    /// converts from) to RGBA.
    pub fn from_oxideav_image(image: &oxideav_image::Image) -> Result<Pixmap, ImageError> {
        let (w, h) = (image.width(), image.height());
        let data = image.to_rgba8()?;
        if data.len() != w as usize * h as usize * 4 {
            return Err(ImageError::Decode("converted buffer size mismatch".into()));
        }
        Ok(Pixmap::from_rgba8(PhysicalSize::new(w, h), data))
    }

    /// Convert an `oxideav` [`VideoFrame`] of `size` pixels in `format` to an
    /// RGBA pixmap. Packed RGBA is copied row by row (dropping any stride
    /// padding); every other layout goes through `oxideav`'s pixel-format
    /// conversion (YUV, RGB, gray, palettes, 16-bit, …).
    pub fn from_video_frame(
        frame: &VideoFrame,
        size: PhysicalSize,
        format: PixelFormat,
    ) -> Result<Pixmap, ImageError> {
        let (w, h) = (size.width, size.height);
        if format == PixelFormat::Rgba
            && let [plane] = frame.planes.as_slice()
        {
            let row = w as usize * 4;
            let need = (h as usize).saturating_sub(1) * plane.stride + row;
            if plane.stride < row || plane.data.len() < need {
                return Err(ImageError::Decode("RGBA plane too small".into()));
            }
            let mut data = Vec::with_capacity(row * h as usize);
            for y in 0..h as usize {
                data.extend_from_slice(&plane.data[y * plane.stride..y * plane.stride + row]);
            }
            return Ok(Pixmap::from_rgba8(size, data));
        }
        let image = oxideav_image::Image::from_planes(w, h, format, frame.planes.clone())?;
        Self::from_oxideav_image(&image)
    }

    /// This pixmap as a single-plane packed RGBA ([`PixelFormat::Rgba`])
    /// `oxideav` [`VideoFrame`], for handing to the rest of the stack.
    pub fn to_video_frame(&self) -> VideoFrame {
        VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: self.stride(),
                data: self.as_bytes().to_vec(),
            }],
        }
    }
}

/// How an image is placed in a box whose aspect ratio may differ from its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageFit {
    /// Stretch to the box exactly, ignoring the aspect ratio.
    Fill,
    /// Scale uniformly to fit inside the box, centered (letterboxed).
    #[default]
    Contain,
    /// Scale uniformly to cover the box, centered; the overflow is clipped.
    Cover,
}

impl ImageFit {
    /// Where an image of `natural` size lands when fitted into `bounds`. For
    /// [`Cover`](ImageFit::Cover) the result extends past `bounds` (the caller
    /// clips it). A degenerate natural size fills `bounds`.
    pub fn place(self, natural: Size, bounds: Rect) -> Rect {
        if natural.width <= 0.0 || natural.height <= 0.0 || self == ImageFit::Fill {
            return bounds;
        }
        let sx = bounds.width() / natural.width;
        let sy = bounds.height() / natural.height;
        let s = match self {
            ImageFit::Contain => sx.min(sy),
            _ => sx.max(sy),
        };
        let (w, h) = (natural.width * s, natural.height * s);
        Rect::from_xywh(
            bounds.min_x() + (bounds.width() - w) / 2.0,
            bounds.min_y() + (bounds.height() - h) / 2.0,
            w,
            h,
        )
    }

    fn aspect(self) -> PreserveAspectRatio {
        match self {
            ImageFit::Fill => PreserveAspectRatio {
                align: AspectRatioAlign::None,
                meet_or_slice: MeetOrSlice::Meet,
            },
            ImageFit::Contain => PreserveAspectRatio::default(),
            ImageFit::Cover => PreserveAspectRatio {
                align: AspectRatioAlign::XMidYMid,
                meet_or_slice: MeetOrSlice::Slice,
            },
        }
    }
}

/// A parsed SVG document, rasterized on demand at the size it's drawn at.
///
/// Supports the subset `oxideav-svg` parses (shapes, paths, gradients,
/// transforms, `<use>`, clip paths and masks, CSS styling). `<text>` needs a
/// font resolver installed in `oxideav-svg` and is otherwise skipped.
pub struct Svg {
    frame: VectorFrame,
    size: Size,
    /// The last raster, keyed by its physical size and fit, so repainting an
    /// unchanged layout doesn't re-rasterize.
    cache: Mutex<Option<(PhysicalSize, ImageFit, Arc<Pixmap>)>>,
}

impl fmt::Debug for Svg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Svg").field("size", &self.size).finish()
    }
}

impl Svg {
    /// Parse an SVG (or gzip-compressed `.svgz`) document.
    pub fn parse(bytes: &[u8]) -> Result<Svg, ImageError> {
        if !oxideav_svg::probe(bytes) {
            return Err(ImageError::UnknownFormat);
        }
        let doc = oxideav_svg::parse(bytes).map_err(|e| ImageError::Decode(e.to_string()))?;
        let mut frame = VectorFrame::from(doc);
        // Always map an explicit view box onto the raster canvas, so the
        // document scales to whatever size it's drawn at.
        let vb = *frame.view_box.get_or_insert(ViewBox {
            min_x: 0.0,
            min_y: 0.0,
            width: frame.width,
            height: frame.height,
        });
        // The natural size is the document's width/height (CSS px), falling
        // back to the view box when they're missing.
        let (w, h) = if frame.width > 0.0 && frame.height > 0.0 {
            (frame.width, frame.height)
        } else {
            (vb.width, vb.height)
        };
        Ok(Svg {
            frame,
            size: Size::new(w.max(0.0) as f64, h.max(0.0) as f64),
            cache: Mutex::new(None),
        })
    }

    /// The document's natural size in logical pixels (its `width`/`height`, or
    /// its view box when those are absent).
    pub fn size(&self) -> Size {
        self.size
    }

    /// Rasterize the document into a pixmap of exactly `size` physical pixels,
    /// placing it per `fit` (a transparent background shows where it doesn't
    /// cover).
    pub fn rasterize(&self, size: PhysicalSize, fit: ImageFit) -> Pixmap {
        let (w, h) = (size.width.max(1), size.height.max(1));
        let renderer = Renderer::new(w, h).with_preserve_aspect_ratio(fit.aspect());
        let video = renderer.render(&self.frame);
        let plane = &video.planes[0];
        let row = w as usize * 4;
        let mut data = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            data.extend_from_slice(&plane.data[y * plane.stride..y * plane.stride + row]);
        }
        Pixmap::from_rgba8(PhysicalSize::new(w, h), data)
    }

    /// [`rasterize`](Svg::rasterize), reusing the previous raster when the size
    /// and fit match.
    pub fn rasterize_cached(&self, size: PhysicalSize, fit: ImageFit) -> Arc<Pixmap> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((s, f, pm)) = cache.as_ref()
            && *s == size
            && *f == fit
        {
            return pm.clone();
        }
        let pm = Arc::new(self.rasterize(size, fit));
        *cache = Some((size, fit, pm.clone()));
        pm
    }
}

/// What an image element shows: a decoded bitmap or an SVG document.
///
/// Equality is identity (the same [`Arc`]), not pixels: keep the `Arc` in your
/// state (or a `static`) and hand out clones, so an unchanged view compares
/// equal and a swapped image damages its area.
#[derive(Clone, Debug)]
pub enum ImageSource {
    Bitmap(Arc<Pixmap>),
    Svg(Arc<Svg>),
}

impl PartialEq for ImageSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ImageSource::Bitmap(a), ImageSource::Bitmap(b)) => Arc::ptr_eq(a, b),
            (ImageSource::Svg(a), ImageSource::Svg(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl ImageSource {
    /// The natural size in logical pixels: one logical pixel per bitmap pixel,
    /// or the SVG's document size.
    pub fn size(&self) -> Size {
        match self {
            ImageSource::Bitmap(pm) => {
                let s = pm.size();
                Size::new(s.width as f64, s.height as f64)
            }
            ImageSource::Svg(svg) => svg.size(),
        }
    }
}

impl ImageSource {
    /// A bitmap source from an `oxideav` [`VideoFrame`] (see
    /// [`Pixmap::from_video_frame`]). The frame is converted once, here: keep
    /// the returned source and clone it into each view build.
    pub fn from_video_frame(
        frame: &VideoFrame,
        size: PhysicalSize,
        format: PixelFormat,
    ) -> Result<ImageSource, ImageError> {
        Pixmap::from_video_frame(frame, size, format).map(|pm| ImageSource::Bitmap(Arc::new(pm)))
    }
}

impl From<&ImageSource> for ImageSource {
    fn from(source: &ImageSource) -> Self {
        source.clone()
    }
}

impl From<Arc<Pixmap>> for ImageSource {
    fn from(pm: Arc<Pixmap>) -> Self {
        ImageSource::Bitmap(pm)
    }
}

impl From<&Arc<Pixmap>> for ImageSource {
    fn from(pm: &Arc<Pixmap>) -> Self {
        ImageSource::Bitmap(pm.clone())
    }
}

impl From<Arc<Svg>> for ImageSource {
    fn from(svg: Arc<Svg>) -> Self {
        ImageSource::Svg(svg)
    }
}

impl From<&Arc<Svg>> for ImageSource {
    fn from(svg: &Arc<Svg>) -> Self {
        ImageSource::Svg(svg.clone())
    }
}

impl Scene {
    /// Draw `source` into `bounds`, placed per `fit`. A bitmap is scaled into
    /// the fitted rect (clipped to `bounds` for [`ImageFit::Cover`]); an SVG is
    /// rasterized at `bounds`' physical size (see [`Scene::scale`]) so it stays
    /// sharp.
    pub fn draw_image_source(&mut self, bounds: Rect, source: &ImageSource, fit: ImageFit) {
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            return;
        }
        match source {
            ImageSource::Bitmap(pm) => {
                let rect = fit.place(source.size(), bounds);
                if fit == ImageFit::Cover {
                    self.push_clip(bounds);
                    self.draw_image(rect, pm.clone());
                    self.pop_clip();
                } else {
                    self.draw_image(rect, pm.clone());
                }
            }
            ImageSource::Svg(svg) => {
                let scale = self.scale();
                let size = PhysicalSize::new(
                    (bounds.width() * scale).round().max(1.0) as u32,
                    (bounds.height() * scale).round().max(1.0) as u32,
                );
                self.draw_image(bounds, svg.rasterize_cached(size, fit));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::ScaleFactor;
    use crate::render::{Color, DrawCmd, SoftwareRenderer};

    /// A 2x1 PNG (red, translucent blue) produced by `oxideav-png`'s encoder.
    fn tiny_png() -> Vec<u8> {
        let px = [255, 0, 0, 255, 0, 0, 255, 128];
        oxideav_png::encode_rgba8(2, 1, &px, &Default::default()).unwrap()
    }

    const SQUARE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20" viewBox="0 0 10 20"><rect x="0" y="0" width="10" height="20" fill="#00ff00"/></svg>"##;

    #[test]
    fn decodes_png() {
        let pm = Pixmap::decode(&tiny_png()).unwrap();
        assert_eq!(pm.size(), PhysicalSize::new(2, 1));
        assert_eq!(pm.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(pm.pixel(1, 0), Some([0, 0, 255, 128]));
    }

    #[test]
    fn decodes_jpeg() {
        let opts = oxideav_mjpeg::EncodeOptions::default().with_quality(90);
        let bytes = oxideav_mjpeg::encode_rgb8(8, 8, &[200; 8 * 8 * 3], &opts).unwrap();
        let pm = Pixmap::decode(&bytes).unwrap();
        assert_eq!(pm.size(), PhysicalSize::new(8, 8));
        let [r, _, _, a] = pm.pixel(4, 4).unwrap();
        assert!(r.abs_diff(200) < 6, "jpeg gray {r}");
        assert_eq!(a, 255);
    }

    #[test]
    fn video_frame_round_trips_and_converts() {
        let pm = Pixmap::decode(&tiny_png()).unwrap();
        let frame = pm.to_video_frame();
        let back = Pixmap::from_video_frame(&frame, pm.size(), PixelFormat::Rgba).unwrap();
        assert_eq!(back, pm);
        // A padded-stride RGBA frame drops the padding.
        let padded = VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: 12,
                data: vec![1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0],
            }],
        };
        let pm =
            Pixmap::from_video_frame(&padded, PhysicalSize::new(2, 1), PixelFormat::Rgba).unwrap();
        assert_eq!(pm.as_bytes(), &[1, 2, 3, 4, 5, 6, 7, 8]);
        // Other layouts convert through oxideav: packed RGB gains opaque alpha.
        let rgb = VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: 6,
                data: vec![10, 20, 30, 40, 50, 60],
            }],
        };
        let pm =
            Pixmap::from_video_frame(&rgb, PhysicalSize::new(2, 1), PixelFormat::Rgb24).unwrap();
        assert_eq!(pm.as_bytes(), &[10, 20, 30, 255, 40, 50, 60, 255]);
        let src = ImageSource::from_video_frame(&rgb, PhysicalSize::new(2, 1), PixelFormat::Rgb24);
        assert_eq!(src.unwrap().size(), Size::new(2.0, 1.0));
    }

    #[test]
    fn decodes_gif_through_the_registry() {
        // A 1x1 GIF89a: palette [red, black], one pixel of index 0.
        let gif: &[u8] = &[
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0xff,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
            0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
        ];
        let pm = Pixmap::decode(gif).unwrap();
        assert_eq!(pm.size(), PhysicalSize::new(1, 1));
        assert_eq!(pm.pixel(0, 0), Some([255, 0, 0, 255]));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(Pixmap::decode(b"hello"), Err(ImageError::UnknownFormat));
        let mut png = tiny_png();
        png.truncate(20);
        assert!(matches!(Pixmap::decode(&png), Err(ImageError::Decode(_))));
    }

    #[test]
    fn fit_modes_place_correctly() {
        let natural = Size::new(100.0, 50.0);
        let bounds = Rect::from_xywh(10.0, 10.0, 100.0, 100.0);
        assert_eq!(ImageFit::Fill.place(natural, bounds), bounds);
        assert_eq!(
            ImageFit::Contain.place(natural, bounds),
            Rect::from_xywh(10.0, 35.0, 100.0, 50.0)
        );
        assert_eq!(
            ImageFit::Cover.place(natural, bounds),
            Rect::from_xywh(-40.0, 10.0, 200.0, 100.0)
        );
        // A degenerate image fills the box.
        assert_eq!(ImageFit::Contain.place(Size::ZERO, bounds), bounds);
    }

    #[test]
    fn source_equality_is_identity() {
        let a = Arc::new(Pixmap::new(PhysicalSize::new(1, 1)));
        let b = Arc::new(Pixmap::new(PhysicalSize::new(1, 1)));
        assert_eq!(ImageSource::from(&a), ImageSource::from(&a));
        assert_ne!(ImageSource::from(&a), ImageSource::from(&b));
    }

    #[test]
    fn svg_rasterizes_at_requested_size() {
        let svg = Svg::parse(SQUARE_SVG).unwrap();
        assert_eq!(svg.size(), Size::new(10.0, 20.0));
        let pm = svg.rasterize(PhysicalSize::new(30, 60), ImageFit::Fill);
        assert_eq!(pm.size(), PhysicalSize::new(30, 60));
        assert_eq!(pm.pixel(15, 30), Some([0, 255, 0, 255]));
        assert_eq!(pm.pixel(29, 59), Some([0, 255, 0, 255]));
        // Contain in a wide box letterboxes: the sides stay transparent.
        let pm = svg.rasterize(PhysicalSize::new(60, 20), ImageFit::Contain);
        assert_eq!(pm.pixel(1, 10).unwrap()[3], 0);
        assert_eq!(pm.pixel(30, 10), Some([0, 255, 0, 255]));
        // The cache hands back the same raster for the same request.
        let a = svg.rasterize_cached(PhysicalSize::new(5, 5), ImageFit::Fill);
        let b = svg.rasterize_cached(PhysicalSize::new(5, 5), ImageFit::Fill);
        assert!(Arc::ptr_eq(&a, &b));
        assert!(Svg::parse(b"nope").is_err());
    }

    #[test]
    fn svg_draws_at_physical_resolution() {
        let svg = Arc::new(Svg::parse(SQUARE_SVG).unwrap());
        let mut scene = Scene::new(Size::new(20.0, 20.0));
        scene.set_scale(2.0);
        scene.draw_image_source(
            Rect::from_xywh(0.0, 0.0, 10.0, 20.0),
            &ImageSource::from(&svg),
            ImageFit::Fill,
        );
        let Some(DrawCmd::Image { image, .. }) = scene.commands().last() else {
            panic!("expected an image command");
        };
        assert_eq!(image.size(), PhysicalSize::new(20, 40));
        let out = SoftwareRenderer::new()
            .with_background(Color::rgb(0, 0, 0))
            .render(scene, ScaleFactor::new(2.0));
        assert_eq!(out.pixel(10, 20), Some([0, 255, 0, 255]));
        assert_eq!(out.pixel(30, 20), Some([0, 0, 0, 255]));
    }
}
