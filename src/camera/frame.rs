//! A camera frame as the app's callback sees it ([`Frame`]), its conversion
//! to an upright RGBA [`Pixmap`] for the preview, and the mapping between
//! frame and preview coordinates ([`PreviewMapping`]).

// Without a camera backend (Windows, Linux) only the tests convert frames.
#![cfg_attr(
    not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "android",
        target_arch = "wasm32"
    )),
    allow(dead_code)
)]

use crate::geometry::{PhysicalSize, Point, Size};
use crate::render::Pixmap;
use std::time::Duration;

/// How a [`Frame`]'s pixels are laid out.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameFormat {
    /// YUV 4:2:0, full range (BT.601): `planes[0]` is luminance (Y), one
    /// byte per pixel; `planes[1]` and `planes[2]` are the blue (U, Cb) and
    /// red (V, Cr) differences at half the width and height. When the two
    /// are interleaved (NV12, as iOS and macOS deliver, and most Android
    /// devices), each has a [`pixel_stride`](Plane::pixel_stride) of 2 and
    /// they overlap in memory. iOS, macOS and Android.
    Yuv420,
    /// Packed RGBA, 8 bits a channel, in `planes[0]`. The web.
    Rgba,
}

/// One plane of a [`Frame`]: rows of `row_stride` bytes, a pixel every
/// `pixel_stride` bytes along a row.
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct Plane<'a> {
    /// The plane's bytes, from its first pixel (borrowed from the camera's
    /// buffer: copy what must outlive the callback).
    pub data: &'a [u8],
    /// Bytes from one row to the next (at least the row's width in bytes).
    pub row_stride: usize,
    /// Bytes from one pixel to the next along a row: 1 for Y, 2 for
    /// interleaved chroma, 4 for RGBA.
    pub pixel_stride: usize,
}

impl<'a> Plane<'a> {
    pub(crate) fn new(data: &'a [u8], row_stride: usize, pixel_stride: usize) -> Self {
        Self {
            data,
            row_stride,
            pixel_stride,
        }
    }

    /// The byte at pixel (`x`, `y`), if inside the data.
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> Option<u8> {
        self.data
            .get(y * self.row_stride + x * self.pixel_stride)
            .copied()
    }
}

/// A frame from the camera, handed to [`Camera::on_frame`](super::Camera::on_frame)'s
/// callback on the capture thread. Its planes borrow the camera's own buffer
/// (no copy is made); the buffer goes back to the camera when the callback
/// returns.
///
/// The pixels are as the sensor delivered them: turn them by
/// [`rotation`](Self::rotation) to see them upright on the screen as it is
/// now. A barcode decoder can ignore the rotation; an overlay drawn over the
/// preview maps the frame's coordinates with
/// [`Camera::preview_mapping`](super::Camera::preview_mapping).
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    /// Width and height in pixels, before rotation.
    pub size: PhysicalSize,
    /// How the planes are laid out.
    pub format: FrameFormat,
    /// The planes: Y, U and V for [`Yuv420`](FrameFormat::Yuv420), one for
    /// [`Rgba`](FrameFormat::Rgba).
    pub planes: &'a [Plane<'a>],
    /// When it was captured, on the camera's clock (only differences between
    /// frames mean anything).
    pub timestamp: Duration,
    /// Degrees (0, 90, 180 or 270) to turn the frame clockwise to see it
    /// upright on the screen as it's turned now.
    pub rotation: u32,
    /// Whether the preview shows it mirrored (after the rotation), as for a
    /// camera facing the user.
    pub mirrored: bool,
}

impl Frame<'_> {
    /// The luminance plane (Y) of a YUV frame: what a barcode decoder reads.
    /// `None` for RGBA frames.
    pub fn luma(&self) -> Option<Plane<'_>> {
        match self.format {
            FrameFormat::Yuv420 => self.planes.first().copied(),
            _ => None,
        }
    }

    /// The frame's size once turned upright.
    pub fn upright_size(&self) -> PhysicalSize {
        upright(self.size, self.rotation)
    }

    /// The frame as an RGBA pixmap, turned upright and mirrored as the
    /// preview shows it, at full size. A copy and a conversion: for a
    /// snapshot, not for every frame.
    pub fn to_pixmap(&self) -> Pixmap {
        let size = self.upright_size();
        self.convert(size)
    }

    /// The frame converted to a pixmap of `out` pixels (upright), sampling
    /// the nearest source pixel.
    pub(crate) fn convert(&self, out: PhysicalSize) -> Pixmap {
        let (ow, oh) = (out.width.max(1) as usize, out.height.max(1) as usize);
        let (w, h) = (self.size.width as usize, self.size.height as usize);
        let up = self.upright_size();
        let (uw, uh) = (up.width as usize, up.height as usize);
        let mut data = vec![0u8; ow * oh * 4];
        if w == 0 || h == 0 || uw == 0 || uh == 0 {
            return Pixmap::from_rgba8(PhysicalSize::new(ow as u32, oh as u32), data);
        }
        // Which upright column each output column samples (mirrored if so).
        let cols: Vec<usize> = (0..ow)
            .map(|ox| {
                let ux = (ox * uw / ow).min(uw - 1);
                if self.mirrored { uw - 1 - ux } else { ux }
            })
            .collect();
        let empty = Plane::new(&[], 0, 1);
        let p0 = self.planes.first().copied().unwrap_or(empty);
        let pu = self.planes.get(1).copied().unwrap_or(empty);
        let pv = self.planes.get(2).copied().unwrap_or(empty);
        for oy in 0..oh {
            let uy = (oy * uh / oh).min(uh - 1);
            let row = &mut data[oy * ow * 4..(oy + 1) * ow * 4];
            for (px, &ux) in row.as_chunks_mut::<4>().0.iter_mut().zip(&cols) {
                let (sx, sy) = match self.rotation {
                    90 => (uy, h - 1 - ux),
                    180 => (w - 1 - ux, h - 1 - uy),
                    270 => (w - 1 - uy, ux),
                    _ => (ux, uy),
                };
                match self.format {
                    FrameFormat::Rgba => {
                        let i = sy * p0.row_stride + sx * p0.pixel_stride;
                        if let Some(s) = p0.data.get(i..i + 4) {
                            px.copy_from_slice(s);
                            px[3] = 255;
                        }
                    }
                    FrameFormat::Yuv420 => {
                        let y = p0.at(sx, sy).unwrap_or(0);
                        let u = pu.at(sx / 2, sy / 2).unwrap_or(128);
                        let v = pv.at(sx / 2, sy / 2).unwrap_or(128);
                        let [r, g, b] = yuv_to_rgb(y, u, v);
                        px.copy_from_slice(&[r, g, b, 255]);
                    }
                }
            }
        }
        Pixmap::from_rgba8(PhysicalSize::new(ow as u32, oh as u32), data)
    }
}

/// One full-range BT.601 YUV pixel as RGB.
#[inline]
pub(crate) fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let (y, u, v) = (i32::from(y), i32::from(u) - 128, i32::from(v) - 128);
    let r = y + ((91_881 * v) >> 16);
    let g = y - ((22_554 * u + 46_802 * v) >> 16);
    let b = y + ((116_130 * u) >> 16);
    [
        r.clamp(0, 255) as u8,
        g.clamp(0, 255) as u8,
        b.clamp(0, 255) as u8,
    ]
}

/// `size` turned by `rotation` degrees.
pub(crate) fn upright(size: PhysicalSize, rotation: u32) -> PhysicalSize {
    if rotation % 180 == 90 {
        PhysicalSize::new(size.height, size.width)
    } else {
        size
    }
}

/// The size of the preview image for an upright frame of `up` pixels shown
/// in `wanted` physical pixels (covering it): no bigger than the frame, nor
/// than [`MAX_PREVIEW`] on its long side.
pub(crate) fn preview_size(up: PhysicalSize, wanted: PhysicalSize) -> PhysicalSize {
    let (uw, uh) = (f64::from(up.width.max(1)), f64::from(up.height.max(1)));
    let cover = (f64::from(wanted.width) / uw).max(f64::from(wanted.height) / uh);
    let k = cover.min(1.0).min(f64::from(MAX_PREVIEW) / uw.max(uh));
    PhysicalSize::new(
        ((uw * k).round() as u32).max(1),
        ((uh * k).round() as u32).max(1),
    )
}

/// The longest side of a preview image, in pixels: the software renderer
/// scales it up to fill the element. Converting each frame costs about this
/// squared (times the aspect) per frame.
pub(crate) const MAX_PREVIEW: u32 = 960;

/// How frame coordinates and a preview's coordinates correspond: the frame
/// turned upright, mirrored if the preview mirrors it, and scaled to cover
/// the preview (centered, the overflow cropped), as
/// [`camera_preview`](super::camera_preview) draws it. From
/// [`Camera::preview_mapping`](super::Camera::preview_mapping).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewMapping {
    /// The frame's size, before rotation.
    pub frame: PhysicalSize,
    /// The frame's [`rotation`](Frame::rotation).
    pub rotation: u32,
    /// Whether the preview mirrors it.
    pub mirrored: bool,
    /// The preview element's size (logical pixels).
    pub preview: Size,
}

impl PreviewMapping {
    /// The mapping for frames of `frame` pixels turned by `rotation` degrees
    /// (and `mirrored`) into a preview of `preview`.
    pub fn new(frame: PhysicalSize, rotation: u32, mirrored: bool, preview: Size) -> Self {
        Self {
            frame,
            rotation: rotation % 360,
            mirrored,
            preview,
        }
    }

    /// The upright size, the scale and the offset of the frame in the
    /// preview.
    fn fit(&self) -> (f64, f64, f64, f64, f64) {
        let up = upright(self.frame, self.rotation);
        let (uw, uh) = (f64::from(up.width.max(1)), f64::from(up.height.max(1)));
        let (pw, ph) = (self.preview.width, self.preview.height);
        let k = (pw / uw).max(ph / uh);
        (uw, uh, k, (pw - uw * k) / 2.0, (ph - uh * k) / 2.0)
    }

    /// Where the frame's pixel `point` (in its own, unrotated coordinates)
    /// shows in the preview. May fall outside it, in the cropped overflow.
    pub fn to_preview(&self, point: Point) -> Point {
        let (w, h) = (f64::from(self.frame.width), f64::from(self.frame.height));
        let (uw, _, k, ox, oy) = self.fit();
        let (x, y) = (point.x, point.y);
        let (mut ux, uy) = match self.rotation {
            90 => (h - y, x),
            180 => (w - x, h - y),
            270 => (y, w - x),
            _ => (x, y),
        };
        if self.mirrored {
            ux = uw - ux;
        }
        Point::new(ux * k + ox, uy * k + oy)
    }

    /// The frame pixel shown at the preview's `point`: the inverse of
    /// [`to_preview`](Self::to_preview).
    pub fn to_frame(&self, point: Point) -> Point {
        let (w, h) = (f64::from(self.frame.width), f64::from(self.frame.height));
        let (uw, _, k, ox, oy) = self.fit();
        let mut ux = (point.x - ox) / k;
        let uy = (point.y - oy) / k;
        if self.mirrored {
            ux = uw - ux;
        }
        let (x, y) = match self.rotation {
            90 => (uy, h - ux),
            180 => (w - ux, h - uy),
            270 => (w - uy, ux),
            _ => (ux, uy),
        };
        Point::new(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4×2 YUV frame whose luminance is 10·x + 100·y.
    fn luma_frame<'a>(planes: &'a [Plane<'a>], rotation: u32, mirrored: bool) -> Frame<'a> {
        Frame {
            size: PhysicalSize::new(4, 2),
            format: FrameFormat::Yuv420,
            planes,
            timestamp: Duration::ZERO,
            rotation,
            mirrored,
        }
    }

    fn grays(p: &Pixmap) -> Vec<u8> {
        p.as_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|px| px[0])
            .collect()
    }

    #[test]
    fn frames_turn_upright() {
        let y = [0u8, 10, 20, 30, 100, 110, 120, 130];
        let uv = [128u8; 4];
        let planes = [
            Plane::new(&y, 4, 1),
            Plane::new(&uv, 4, 2),
            Plane::new(&uv[1..], 4, 2),
        ];
        let f = luma_frame(&planes, 0, false);
        assert_eq!(grays(&f.to_pixmap()), y.to_vec());
        // Clockwise: the bottom-left comes to the top-left.
        let f = luma_frame(&planes, 90, false);
        assert_eq!(f.upright_size(), PhysicalSize::new(2, 4));
        assert_eq!(
            grays(&f.to_pixmap()),
            vec![100, 0, 110, 10, 120, 20, 130, 30]
        );
        let f = luma_frame(&planes, 270, false);
        assert_eq!(
            grays(&f.to_pixmap()),
            vec![30, 130, 20, 120, 10, 110, 0, 100]
        );
        let f = luma_frame(&planes, 180, true);
        assert_eq!(
            grays(&f.to_pixmap()),
            vec![100, 110, 120, 130, 0, 10, 20, 30]
        );
        assert_eq!(f.luma().unwrap().at(3, 1), Some(130));
    }

    #[test]
    fn colors_convert() {
        assert_eq!(yuv_to_rgb(128, 128, 128), [128, 128, 128]);
        assert_eq!(yuv_to_rgb(255, 128, 128), [255, 255, 255]);
        let [r, g, b] = yuv_to_rgb(76, 85, 255);
        assert!(r > 250 && g < 5 && b < 5, "{r} {g} {b}");
    }

    #[test]
    fn previews_are_small() {
        let up = PhysicalSize::new(1080, 1920);
        assert_eq!(
            preview_size(up, PhysicalSize::new(540, 960)),
            PhysicalSize::new(540, 960)
        );
        // Never above the frame or the cap.
        let s = preview_size(up, PhysicalSize::new(3000, 3000));
        assert_eq!(s.height, MAX_PREVIEW);
        assert_eq!(s.width, 540);
    }

    #[test]
    fn mappings_round_trip() {
        for rotation in [0, 90, 180, 270] {
            for mirrored in [false, true] {
                let m = PreviewMapping::new(
                    PhysicalSize::new(1920, 1080),
                    rotation,
                    mirrored,
                    Size::new(400.0, 800.0),
                );
                let p = Point::new(300.0, 200.0);
                let back = m.to_frame(m.to_preview(p));
                assert!((back.x - p.x).abs() < 1e-6 && (back.y - p.y).abs() < 1e-6);
            }
        }
        // Turned upright (1080×1920 into 400×800, covering: scale 0.4167,
        // 25 px cropped off each side), the frame's top-left is at the top
        // right.
        let m = PreviewMapping::new(
            PhysicalSize::new(1920, 1080),
            90,
            false,
            Size::new(400.0, 800.0),
        );
        let p = m.to_preview(Point::new(0.0, 0.0));
        assert!((p.x - 425.0).abs() < 1e-6 && p.y.abs() < 1e-6, "{p:?}");
    }
}
