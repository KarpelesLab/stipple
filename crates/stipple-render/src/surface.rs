use stipple_geometry::{PhysicalSize, Rect};

/// A CPU pixel buffer in straight (non-premultiplied) RGBA8, row-major,
/// tightly packed (`stride == width * 4`).
///
/// This is the payload handed to a [`Surface`] for display. It is what the
/// software backend produces today; a future GPU backend may bypass it.
#[derive(Clone, Debug)]
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
    /// [`SoftwareRenderer::render_region`]: crate::SoftwareRenderer::render_region
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
