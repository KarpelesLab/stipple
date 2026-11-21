//! Decoded 4:2:0 pictures in memory (a platform decoder's NV12 or planar
//! output) as images: converted once to RGBA, the way the software decoder's
//! frames are.

use crate::geometry::PhysicalSize;
use crate::render::external::YuvColorSpace;
use crate::render::{ExternalFormat, ExternalImage};
use std::ffi::CStr;
use std::sync::Arc;

/// The color space a stream that doesn't say is taken to be in: BT.601 up
/// to 576 lines, BT.709 above, limited range (as nearly every video is).
#[cfg_attr(
    not(any(target_os = "windows", target_os = "android", target_os = "linux")),
    allow(dead_code)
)]
pub(super) fn default_color(height: u32) -> YuvColorSpace {
    if height > 576 {
        YuvColorSpace::Bt709Limited
    } else {
        YuvColorSpace::Bt601Limited
    }
}

/// A `size` NV12 picture (its luma rows `ys` bytes apart in `y`, its
/// interleaved chroma rows `uvs` apart in `uv`) as an image of its pixels.
#[cfg_attr(
    not(any(target_os = "windows", target_os = "android")),
    allow(dead_code)
)]
pub(super) fn image(
    size: PhysicalSize,
    (y, ys): (&[u8], usize),
    (uv, uvs): (&[u8], usize),
    color: YuvColorSpace,
) -> Option<Arc<ExternalImage>> {
    let pixels = crate::platform::external::to_rgba(
        ExternalFormat::Nv12,
        size,
        &[(y, ys), (uv, uvs)],
        color,
    )?;
    Some(Arc::new(ExternalImage::from_pixels(Arc::new(pixels))))
}

/// One chroma plane of a 4:2:0 picture: its bytes from its first sample,
/// the bytes between its rows and between its samples (1 planar, 2 when
/// the two planes are interleaved).
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
#[derive(Clone, Copy)]
pub(super) struct Chroma<'a> {
    pub(super) data: &'a [u8],
    pub(super) row_stride: usize,
    pub(super) pixel_stride: usize,
}

/// The `u` and `v` planes of a `w`×`h` picture interleaved as NV12's
/// chroma plane (`w` rounded up to even bytes a row); `None` if a plane is
/// shorter than its strides say.
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(super) fn interleave(u: Chroma<'_>, v: Chroma<'_>, w: usize, h: usize) -> Option<Vec<u8>> {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![0u8; cw * 2 * ch];
    for row in 0..ch {
        for x in 0..cw {
            let at = |p: &Chroma<'_>| p.data.get(row * p.row_stride + x * p.pixel_stride).copied();
            out[row * cw * 2 + x * 2] = at(&u)?;
            out[row * cw * 2 + x * 2 + 1] = at(&v)?;
        }
    }
    Some(out)
}

/// `COLOR_FormatYUV420Planar`, `...SemiPlanar` (the others are taken as
/// semi-planar: flexible YUV in a buffer is NV12 on every decoder seen).
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(super) const COLOR_FORMAT_PLANAR: i32 = 19;

/// NV12 planes: the luma and its stride, the chroma (borrowed, or gathered
/// from planar) and its.
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(super) type Nv12Planes<'a> = (&'a [u8], usize, std::borrow::Cow<'a, [u8]>, usize);

/// Where a decoder's own buffer puts a picture (`MediaCodec`'s `ByteBuffer`
/// mode).
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    pub(super) stride: usize,
    pub(super) slice_height: usize,
    pub(super) planar: bool,
    pub(super) left: usize,
    pub(super) top: usize,
    pub(super) width: usize,
    pub(super) height: usize,
}

#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
impl Layout {
    /// The layout of a `w`×`h` picture until the decoder says otherwise.
    pub(super) fn new(w: usize, h: usize) -> Self {
        Self {
            stride: w,
            slice_height: h,
            planar: false,
            left: 0,
            top: 0,
            width: w,
            height: h,
        }
    }

    /// The decoder's output format's: `get` reads its integer keys.
    pub(super) fn read(&mut self, get: impl Fn(&CStr) -> Option<i32>) {
        let pos = |k: &CStr| get(k).filter(|&v| v >= 0).map(|v| v as usize);
        let (w, h) = (pos(c"width"), pos(c"height"));
        if let (Some(w), Some(h)) = (w, h) {
            self.width = w;
            self.height = h;
            self.stride = w;
            self.slice_height = h;
        }
        if let Some(s) = pos(c"stride").filter(|&s| s >= self.width) {
            self.stride = s;
        }
        if let Some(s) = pos(c"slice-height").filter(|&s| s >= self.height) {
            self.slice_height = s;
        }
        if let Some(c) = get(c"color-format") {
            self.planar = c == COLOR_FORMAT_PLANAR;
        }
        if let (Some(l), Some(t), Some(r), Some(b)) = (
            pos(c"crop-left"),
            pos(c"crop-top"),
            pos(c"crop-right"),
            pos(c"crop-bottom"),
        ) && r >= l
            && b >= t
        {
            self.left = l;
            self.top = t;
            self.width = r - l + 1;
            self.height = b - t + 1;
        }
    }

    /// The visible picture in `data` (one output buffer) as NV12 planes:
    /// the luma and its stride, the chroma and its.
    pub(super) fn nv12<'a>(&self, data: &'a [u8]) -> Option<Nv12Planes<'a>> {
        let (w, h) = (self.width & !1, self.height & !1);
        let luma_len = self.stride * self.slice_height;
        let y = data.get(self.top * self.stride + self.left..)?;
        let chroma = data.get(luma_len..)?;
        let (ct, cl) = (self.top / 2, self.left / 2);
        if self.planar {
            let cs = self.stride.div_ceil(2);
            let plane = cs * self.slice_height.div_ceil(2);
            let u = chroma.get(ct * cs + cl..)?;
            let v = chroma.get(plane + ct * cs + cl..)?;
            let plane = |data| Chroma {
                data,
                row_stride: cs,
                pixel_stride: 1,
            };
            let uv = interleave(plane(u), plane(v), w, h)?;
            Some((y, self.stride, uv.into(), w.div_ceil(2) * 2))
        } else {
            let uv = chroma.get(ct * self.stride + cl * 2..)?;
            Some((y, self.stride, uv.into(), self.stride))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_and_semi_planar_chroma_interleave() {
        // 4×2 luma: a 2×1 chroma, planar.
        let (u, v) = ([10, 11], [20, 21]);
        let out = interleave(
            Chroma {
                data: &u,
                row_stride: 2,
                pixel_stride: 1,
            },
            Chroma {
                data: &v,
                row_stride: 2,
                pixel_stride: 1,
            },
            4,
            2,
        )
        .unwrap();
        assert_eq!(out, [10, 20, 11, 21]);
        // NV21 (V first), as a camera or codec may lay it out.
        let vu = [20u8, 10, 21, 11];
        let out = interleave(
            Chroma {
                data: &vu[1..],
                row_stride: 4,
                pixel_stride: 2,
            },
            Chroma {
                data: &vu,
                row_stride: 4,
                pixel_stride: 2,
            },
            4,
            2,
        )
        .unwrap();
        assert_eq!(out, [10, 20, 11, 21]);
        assert!(
            interleave(
                Chroma {
                    data: &u[..1],
                    row_stride: 2,
                    pixel_stride: 1
                },
                Chroma {
                    data: &v,
                    row_stride: 2,
                    pixel_stride: 1
                },
                4,
                2
            )
            .is_none()
        );
    }

    #[test]
    fn nv12_becomes_pixels() {
        // 2×2 of mid gray: Y 126, chroma neutral.
        let img = image(
            PhysicalSize::new(2, 2),
            (&[126; 4], 2),
            (&[128, 128], 2),
            YuvColorSpace::Bt601Limited,
        )
        .unwrap();
        let px = img.pixmap().unwrap();
        let p = px.pixel(1, 1).unwrap();
        assert!(
            p[0].abs_diff(128) <= 2 && p[0] == p[1] && p[1] == p[2],
            "{p:?}"
        );
        assert_eq!(default_color(1080), YuvColorSpace::Bt709Limited);
        assert_eq!(default_color(480), YuvColorSpace::Bt601Limited);
    }

    #[test]
    fn a_buffer_layout_follows_the_output_format() {
        let mut l = Layout::new(160, 120);
        let keys = [
            (c"width", 160),
            (c"height", 128),
            (c"stride", 176),
            (c"slice-height", 128),
            (c"color-format", COLOR_FORMAT_PLANAR),
            (c"crop-left", 0),
            (c"crop-top", 0),
            (c"crop-right", 159),
            (c"crop-bottom", 119),
        ];
        l.read(|k| keys.iter().find(|(n, _)| *n == k).map(|(_, v)| *v));
        assert_eq!(
            l,
            Layout {
                stride: 176,
                slice_height: 128,
                planar: true,
                left: 0,
                top: 0,
                width: 160,
                height: 120,
            }
        );
    }

    #[test]
    fn planar_and_semi_planar_buffers_give_nv12() {
        // 4×2, stride 4: luma 0..8, then the chroma.
        let mut semi = Layout::new(4, 2);
        let data: Vec<u8> = (0..8).chain([10, 20, 11, 21]).collect();
        let (y, ys, uv, uvs) = semi.nv12(&data).unwrap();
        assert_eq!(
            (&y[..8], ys, &uv[..4], uvs),
            (&data[..8], 4, &[10, 20, 11, 21][..], 4)
        );
        semi.planar = true;
        let data: Vec<u8> = (0..8).chain([10, 11, 20, 21]).collect();
        let (_, _, uv, uvs) = semi.nv12(&data).unwrap();
        assert_eq!((&uv[..], uvs), (&[10, 20, 11, 21][..], 4));
    }
}
