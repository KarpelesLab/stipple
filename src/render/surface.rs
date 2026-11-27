use crate::geometry::{PhysicalSize, Rect};

/// A CPU pixel buffer in straight (non-premultiplied) RGBA8, row-major,
/// tightly packed (`stride == width * 4`).
///
/// This is the payload handed to a [`Surface`] for display. It is what the
/// software backend produces today; a future GPU backend may bypass it.
#[derive(Clone, Debug, PartialEq)]
pub struct Pixmap {
    size: PhysicalSize,
    data: Vec<u8>,
}

impl Pixmap {
    /// Allocate a fully transparent pixmap of `size`.
    pub fn new(size: PhysicalSize) -> Self {
        Self {
            size,
            data: vec![0u8; size.pixel_count() as usize * 4],
        }
    }

    /// Wrap existing RGBA8 bytes. Panics if `data.len() != width*height*4`.
    pub fn from_rgba8(size: PhysicalSize, data: Vec<u8>) -> Self {
        assert_eq!(
            data.len(),
            size.pixel_count() as usize * 4,
            "pixmap byte length must equal width*height*4"
        );
        Self { size, data }
    }

    #[inline]
    pub fn size(&self) -> PhysicalSize {
        self.size
    }

    /// Bytes per row.
    #[inline]
    pub fn stride(&self) -> usize {
        self.size.width as usize * 4
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    #[inline]
    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Copy `src` into this pixmap with its top-left at `(dst_x, dst_y)`,
    /// clipped to this pixmap's bounds. A straight row-by-row replace (no alpha
    /// blend) — used to composite an opaque area-repaint render (see
    /// [`SoftwareRenderer::render_region`]) into the retained full-window buffer.
    ///
    /// [`SoftwareRenderer::render_region`]: crate::render::SoftwareRenderer::render_region
    pub fn blit(&mut self, src: &Pixmap, dst_x: u32, dst_y: u32) {
        let dst_w = self.size.width;
        let dst_h = self.size.height;
        // Rows/cols of `src` that land inside this pixmap.
        let copy_w = src.size.width.min(dst_w.saturating_sub(dst_x));
        let copy_h = src.size.height.min(dst_h.saturating_sub(dst_y));
        if copy_w == 0 || copy_h == 0 {
            return;
        }
        let (src_stride, dst_stride) = (src.stride(), self.stride());
        let row_bytes = copy_w as usize * 4;
        for row in 0..copy_h as usize {
            let s = row * src_stride;
            let d = (dst_y as usize + row) * dst_stride + dst_x as usize * 4;
            self.data[d..d + row_bytes].copy_from_slice(&src.data[s..s + row_bytes]);
        }
    }

    /// The four bytes `[r, g, b, a]` at `(x, y)`, or `None` if out of bounds.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.size.width || y >= self.size.height {
            return None;
        }
        let i = y as usize * self.stride() + x as usize * 4;
        Some([
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ])
    }

    /// Encode as a PNG file (8-bit RGBA): a frame from
    /// [`App::render_once`](crate::App::render_once) as a screenshot, say.
    pub fn encode_png(&self) -> Vec<u8> {
        let (w, h) = (self.size.width, self.size.height);
        if w == 0 || h == 0 {
            // PNG has no empty image: an empty pixmap saves as one
            // transparent pixel.
            return Pixmap::new(PhysicalSize::new(1, 1)).encode_png();
        }
        oxideav_png::encode_rgba8(w, h, &self.data, &Default::default())
            .expect("the PNG encoder takes any non-empty RGBA8 image")
    }

    /// Write the pixmap to `path` as a PNG file (see
    /// [`encode_png`](Self::encode_png)).
    pub fn save_png(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        std::fs::write(path, self.encode_png())
    }
}

/// The integer pixel rects of `src` that `damage` (physical pixels) covers,
/// as `(x, y, width, height)`, rounded outwards and clipped to it; the whole
/// pixmap when `damage` is empty.
#[cfg_attr(
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "fullrust",
        test
    )),
    allow(dead_code)
)]
pub(crate) fn damage_rects(size: PhysicalSize, damage: &[Rect]) -> Vec<(u32, u32, u32, u32)> {
    let (w, h) = (size.width, size.height);
    if damage.is_empty() {
        return if w > 0 && h > 0 {
            vec![(0, 0, w, h)]
        } else {
            Vec::new()
        };
    }
    damage
        .iter()
        .filter_map(|r| {
            let x0 = (r.min_x().floor().max(0.0) as u32).min(w);
            let y0 = (r.min_y().floor().max(0.0) as u32).min(h);
            let x1 = (r.max_x().ceil().max(0.0) as u32).min(w);
            let y1 = (r.max_y().ceil().max(0.0) as u32).min(h);
            (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
        })
        .collect()
}

/// Copy the `rects` (from [`damage_rects`]) of `src` into `dst`, a buffer of
/// the same size in Core Graphics' and Win32's native order (`B, G, R, A`:
/// 32-bit little-endian, alpha first), resizing `dst` (to zeros) if it's the
/// wrong size. With `alpha`, for a transparent window, the alpha is kept and
/// the colors premultiplied by it, as compositors take them; without, every
/// pixel is opaque (alpha 255, the colors as they are).
#[cfg_attr(
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "fullrust",
        test
    )),
    allow(dead_code)
)]
pub(crate) fn copy_as_bgra(
    src: &Pixmap,
    dst: &mut Vec<u8>,
    rects: &[(u32, u32, u32, u32)],
    alpha: bool,
) {
    let len = src.as_bytes().len();
    if dst.len() != len {
        dst.clear();
        dst.resize(len, 0);
    }
    let stride = src.stride();
    let bytes = src.as_bytes();
    for &(x, y, w, h) in rects {
        for row in y as usize..(y + h) as usize {
            let start = row * stride + x as usize * 4;
            let end = start + w as usize * 4;
            let from = bytes[start..end].as_chunks::<4>().0;
            let to = dst[start..end].as_chunks_mut::<4>().0;
            if alpha {
                for (d, s) in to.iter_mut().zip(from) {
                    *d = premultiplied_bgra(*s);
                }
            } else {
                for (d, s) in to.iter_mut().zip(from) {
                    *d = [s[2], s[1], s[0], 0xff];
                }
            }
        }
    }
}

/// A straight-alpha `[r, g, b, a]` pixel as premultiplied `[b, g, r, a]`, the
/// form a compositor blends a translucent window's pixels in.
#[cfg_attr(
    not(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux",
        target_os = "fullrust",
        test
    )),
    allow(dead_code)
)]
#[inline]
pub(crate) fn premultiplied_bgra(s: [u8; 4]) -> [u8; 4] {
    let a = s[3] as u32;
    if a == 255 {
        return [s[2], s[1], s[0], 255];
    }
    let m = |c: u8| ((c as u32 * a + 127) / 255) as u8;
    [m(s[2]), m(s[1]), m(s[0]), s[3]]
}

/// A presentable destination owned by the platform layer (a window's drawable,
/// a canvas, a layer-backed view, …).
///
/// This trait is the **GPU-readiness seam**: the software backend presents a
/// [`Pixmap`] by blitting; a future GPU backend can implement the same trait
/// by uploading or compositing directly. `damage` lists the regions (in
/// physical pixels) that actually changed since the last present — an empty
/// slice means "assume everything changed."
pub trait Surface {
    /// Resize the backing store to `size` physical pixels.
    fn resize(&mut self, size: PhysicalSize);

    /// The current backing-store size in physical pixels.
    fn size(&self) -> PhysicalSize;

    /// Present `pixmap`, optionally limited to the `damage` regions.
    fn present(&mut self, pixmap: &Pixmap, damage: &[Rect]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_access_bounds() {
        let mut pm = Pixmap::new(PhysicalSize::new(2, 2));
        pm.as_bytes_mut()[4..8].copy_from_slice(&[10, 20, 30, 40]);
        assert_eq!(pm.pixel(1, 0), Some([10, 20, 30, 40]));
        assert_eq!(pm.pixel(2, 0), None);
        assert_eq!(pm.stride(), 8);
    }

    #[test]
    fn png_round_trips() {
        let mut pm = Pixmap::new(PhysicalSize::new(3, 2));
        pm.as_bytes_mut()[4..8].copy_from_slice(&[10, 20, 30, 40]);
        pm.as_bytes_mut()[20..24].copy_from_slice(&[255, 0, 128, 255]);
        let png = pm.encode_png();
        assert!(png.starts_with(b"\x89PNG"));
        assert_eq!(Pixmap::decode(&png).unwrap(), pm);
        let empty = Pixmap::new(PhysicalSize::new(0, 4)).encode_png();
        assert_eq!(
            Pixmap::decode(&empty).unwrap().size(),
            PhysicalSize::new(1, 1)
        );
    }

    #[test]
    fn damage_becomes_whole_pixels_inside_the_pixmap() {
        let size = PhysicalSize::new(10, 8);
        assert_eq!(damage_rects(size, &[]), [(0, 0, 10, 8)], "all of it");
        let d = [
            Rect::from_xywh(1.5, 2.25, 3.0, 1.5),
            Rect::from_xywh(8.0, 6.0, 5.0, 5.0),
            Rect::from_xywh(20.0, 0.0, 2.0, 2.0),
        ];
        assert_eq!(damage_rects(size, &d), [(1, 2, 4, 2), (8, 6, 2, 2)]);
    }

    #[test]
    fn copies_damage_as_bgra() {
        let mut src = Pixmap::new(PhysicalSize::new(3, 2));
        for px in src.as_bytes_mut().as_chunks_mut::<4>().0 {
            px.copy_from_slice(&[1, 2, 3, 4]);
        }
        let mut dst = Vec::new();
        copy_as_bgra(&src, &mut dst, &[(1, 1, 2, 1)], false);
        assert_eq!(dst.len(), 24, "sized to the pixmap");
        assert_eq!(&dst[..16], &[0; 16], "outside the damage: untouched");
        assert_eq!(&dst[16..], &[3, 2, 1, 255, 3, 2, 1, 255]);
        copy_as_bgra(&src, &mut dst, &damage_rects(src.size(), &[]), false);
        assert!(dst.as_chunks::<4>().0.iter().all(|p| *p == [3, 2, 1, 255]));
        // A transparent window's copy keeps the alpha, premultiplied.
        for px in src.as_bytes_mut().as_chunks_mut::<4>().0 {
            px.copy_from_slice(&[200, 100, 50, 128]);
        }
        copy_as_bgra(&src, &mut dst, &damage_rects(src.size(), &[]), true);
        assert!(
            dst.as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [25, 50, 100, 128])
        );
        assert_eq!(premultiplied_bgra([1, 2, 3, 255]), [3, 2, 1, 255]);
        assert_eq!(premultiplied_bgra([255, 255, 255, 0]), [0, 0, 0, 0]);
    }

    #[test]
    fn blit_composites_at_offset() {
        let mut dst = Pixmap::new(PhysicalSize::new(4, 4));
        let mut src = Pixmap::new(PhysicalSize::new(2, 2));
        for px in src.as_bytes_mut().as_chunks_mut::<4>().0 {
            px.copy_from_slice(&[1, 2, 3, 4]);
        }
        dst.blit(&src, 1, 1);
        // The 2x2 block at (1,1) is filled; (0,0) stays transparent.
        assert_eq!(dst.pixel(0, 0), Some([0, 0, 0, 0]));
        assert_eq!(dst.pixel(1, 1), Some([1, 2, 3, 4]));
        assert_eq!(dst.pixel(2, 2), Some([1, 2, 3, 4]));
        assert_eq!(dst.pixel(3, 3), Some([0, 0, 0, 0]));
    }

    #[test]
    fn blit_clips_to_destination_bounds() {
        let mut dst = Pixmap::new(PhysicalSize::new(2, 2));
        let mut src = Pixmap::new(PhysicalSize::new(2, 2));
        for px in src.as_bytes_mut().as_chunks_mut::<4>().0 {
            px.copy_from_slice(&[9, 9, 9, 9]);
        }
        // Origin near the far corner: only the (1,1) pixel lands inside.
        dst.blit(&src, 1, 1);
        assert_eq!(dst.pixel(1, 1), Some([9, 9, 9, 9]));
        assert_eq!(dst.pixel(0, 0), Some([0, 0, 0, 0]));
        // Fully out of bounds: no-op, no panic.
        dst.blit(&src, 5, 5);
    }
}
