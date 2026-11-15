//! Drawing part of a bitmap: a source rect of a sprite sheet or atlas into a
//! destination rect, flipped, rotated, scaled, tinted, faded, and sampled
//! smoothly or as hard pixels. See [`Scene::draw_image_region`].
//!
//! [`Scene::draw_image_region`]: crate::render::Scene::draw_image_region

use crate::geometry::{Affine, Point, Rect, Vec2};
use crate::render::{Color, Pixmap};
use oxideav_core::{ImageRef, Node, Rect as OxRect, Transform2D, VideoFrame, VideoPlane};
use std::sync::Arc;

/// How a bitmap is sampled when it is drawn at another size or position than
/// its own pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Filter {
    /// Bilinear: smooth, what photos and UI images want.
    #[default]
    Linear,
    /// Nearest neighbor: every source pixel stays a hard-edged block, what
    /// pixel art wants.
    Nearest,
}

/// How [`Scene::draw_image_region`](crate::render::Scene::draw_image_region)
/// draws: flips, a rotation and scale about a pivot, a tint, an opacity and
/// the sampling filter. The default draws the region as it is.
///
/// ```
/// use stipple::render::{Color, ImageOptions};
/// let opts = ImageOptions::new()
///     .flip_x(true)
///     .rotate(0.5)
///     .scale(2.0)
///     .tint(Color::rgb(255, 120, 120))
///     .opacity(0.8)
///     .nearest();
/// assert!(opts.flip_x && opts.scale.dx == 2.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageOptions {
    /// Mirror the region left to right (within the destination rect).
    pub flip_x: bool,
    /// Mirror the region top to bottom.
    pub flip_y: bool,
    /// A rotation about [`pivot`](Self::pivot), in radians, clockwise on
    /// screen.
    pub rotation: f64,
    /// A scale about [`pivot`](Self::pivot), on top of the destination rect's
    /// own size.
    pub scale: Vec2,
    /// The fixed point of the rotation and scale, as fractions of the
    /// destination rect (`(0.5, 0.5)`, its center, by default).
    pub pivot: Point,
    /// Multiplies every pixel's color and alpha (white, the default, leaves
    /// them as they are). A red tint flashes a hit enemy.
    pub tint: Color,
    /// Fades the region (`0.0..=1.0`).
    pub opacity: f32,
    /// How the region is sampled.
    pub filter: Filter,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            flip_x: false,
            flip_y: false,
            rotation: 0.0,
            scale: Vec2::new(1.0, 1.0),
            pivot: Point::new(0.5, 0.5),
            tint: Color::WHITE,
            opacity: 1.0,
            filter: Filter::Linear,
        }
    }
}

impl ImageOptions {
    /// The region as it is: no flip, rotation, scale or tint, opaque, linear.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mirror left to right.
    pub fn flip_x(mut self, on: bool) -> Self {
        self.flip_x = on;
        self
    }

    /// Mirror top to bottom.
    pub fn flip_y(mut self, on: bool) -> Self {
        self.flip_y = on;
        self
    }

    /// Rotate by `radians` (clockwise on screen) about the pivot.
    pub fn rotate(mut self, radians: f64) -> Self {
        self.rotation = radians;
        self
    }

    /// Scale by `s` about the pivot.
    pub fn scale(self, s: f64) -> Self {
        self.scale_xy(s, s)
    }

    /// Scale by `sx` across and `sy` down about the pivot.
    pub fn scale_xy(mut self, sx: f64, sy: f64) -> Self {
        self.scale = Vec2::new(sx, sy);
        self
    }

    /// Set the pivot, as fractions of the destination rect.
    pub fn pivot(mut self, x: f64, y: f64) -> Self {
        self.pivot = Point::new(x, y);
        self
    }

    /// Multiply every pixel by `color`.
    pub fn tint(mut self, color: Color) -> Self {
        self.tint = color;
        self
    }

    /// Fade to `opacity` (`0.0..=1.0`).
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    /// Sample with `filter`.
    pub fn filter(mut self, filter: Filter) -> Self {
        self.filter = filter;
        self
    }

    /// Sample nearest neighbor (hard pixels).
    pub fn nearest(self) -> Self {
        self.filter(Filter::Nearest)
    }

    /// The transform the rotation and scale apply to a region drawn into
    /// `dst` (identity when there are none).
    pub fn transform(&self, dst: Rect) -> Affine {
        if self.rotation == 0.0 && self.scale == Vec2::new(1.0, 1.0) {
            return Affine::IDENTITY;
        }
        let p = Point::new(
            dst.min_x() + self.pivot.x * dst.width(),
            dst.min_y() + self.pivot.y * dst.height(),
        );
        Affine::translate(Vec2::new(-p.x, -p.y))
            .then(Affine::scale(self.scale.dx, self.scale.dy))
            .then(Affine::rotate(self.rotation))
            .then(Affine::translate(Vec2::new(p.x, p.y)))
    }
}

/// A region of a bitmap to draw, as recorded by the scene: what the software
/// compositor blits and the GPU path textures.
#[derive(Clone, Debug)]
pub(crate) struct SpriteDraw {
    pub image: Arc<Pixmap>,
    /// The source rect in whole pixels, `[x0, y0, x1, y1]`, inside the image
    /// and not empty.
    pub src: [u32; 4],
    pub flip_x: bool,
    pub flip_y: bool,
    /// The tint, straight RGBA.
    pub tint: [u8; 4],
    pub opacity: f32,
    pub filter: Filter,
}

impl SpriteDraw {
    /// `src` (pixels, rounded to whole ones and clipped to the image) of
    /// `image`, drawn with `opts`; `None` if nothing of it is left.
    pub fn new(image: Arc<Pixmap>, src: Rect, opts: &ImageOptions) -> Option<Self> {
        let (w, h) = (image.size().width as f64, image.size().height as f64);
        let x0 = src.min_x().round().clamp(0.0, w) as u32;
        let y0 = src.min_y().round().clamp(0.0, h) as u32;
        let x1 = src.max_x().round().clamp(0.0, w) as u32;
        let y1 = src.max_y().round().clamp(0.0, h) as u32;
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let opacity = if opacity_is_valid(opts.opacity) {
            opts.opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let t = opts.tint;
        Some(Self {
            image,
            src: [x0, y0, x1, y1],
            flip_x: opts.flip_x,
            flip_y: opts.flip_y,
            tint: [t.r, t.g, t.b, t.a],
            opacity,
            filter: opts.filter,
        })
    }

    pub fn width(&self) -> u32 {
        self.src[2] - self.src[0]
    }

    pub fn height(&self) -> u32 {
        self.src[3] - self.src[1]
    }

    /// Whether the tint and opacity leave the pixels as they are.
    pub fn plain(&self) -> bool {
        self.tint == [255; 4] && self.opacity >= 1.0
    }

    /// The region as a plain `oxideav` image node over `bounds`, drawn
    /// through `transform`: its pixels copied out, flipped and tinted, and,
    /// for nearest sampling, each repeated `upscale` times across and down so
    /// the rasterizer's bilinear filter keeps them as blocks. What a backend
    /// that can't draw a region itself gets.
    pub fn to_node(&self, bounds: OxRect, transform: Transform2D, upscale: u32) -> Node {
        let k = if self.filter == Filter::Nearest {
            upscale.clamp(1, 8) as usize
        } else {
            1
        };
        let (w, h) = (self.width() as usize, self.height() as usize);
        let stride = self.image.stride();
        let px = self.image.as_bytes();
        let (out_w, out_h) = (w * k, h * k);
        let mut data = vec![0u8; out_w * out_h * 4];
        let alpha = self.opacity;
        let t = self.tint;
        for oy in 0..out_h {
            let sy = oy / k;
            let sy = if self.flip_y { h - 1 - sy } else { sy };
            let row = (self.src[1] as usize + sy) * stride;
            for ox in 0..out_w {
                let sx = ox / k;
                let sx = if self.flip_x { w - 1 - sx } else { sx };
                let i = row + (self.src[0] as usize + sx) * 4;
                let o = (oy * out_w + ox) * 4;
                data[o] = mul8(px[i], t[0]);
                data[o + 1] = mul8(px[i + 1], t[1]);
                data[o + 2] = mul8(px[i + 2], t[2]);
                data[o + 3] = (mul8(px[i + 3], t[3]) as f32 * alpha).round() as u8;
            }
        }
        Node::Image(ImageRef {
            frame: Box::new(VideoFrame {
                pts: None,
                planes: vec![VideoPlane {
                    stride: out_w * 4,
                    data,
                }],
            }),
            bounds,
            transform,
        })
    }
}

fn opacity_is_valid(o: f32) -> bool {
    !o.is_nan()
}

/// `a · b / 255`, rounded.
#[inline]
pub(crate) fn mul8(a: u8, b: u8) -> u8 {
    ((a as u32 * b as u32 + 127) / 255) as u8
}
