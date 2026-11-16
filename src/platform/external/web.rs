//! External images in a browser: a source the page holds (a `<video>`, a
//! `VideoFrame`, an `ImageBitmap`, a canvas), registered with
//! `web/stipple.js`'s `externalSource(source)`, which answers its id. The
//! GPU executor's page code (`web/gpu.js`) draws it straight from the
//! source: WebGPU imports a video or `VideoFrame` as a `texture_external`
//! (`importExternalTexture`), WebGL2 hands it to `texImage2D`. The
//! software renderer reads it once (`external_read`, drawn into a 2D
//! canvas and read back).

use super::{DRM_FORMAT_MOD_INVALID, ExternalFormat, ExternalImage, Handle, PlaneLayout};
use crate::geometry::PhysicalSize;
use crate::render::Pixmap;

#[link(wasm_import_module = "stipple")]
unsafe extern "C" {
    /// Draw source `id` (scaled to `w`×`h`) and copy its straight-alpha
    /// RGBA into `ptr`; 0 if there's no such source or it has no frame.
    fn external_read(id: u32, ptr: *mut u8, w: u32, h: u32) -> u32;
    /// Let go of source `id`.
    fn external_release(id: u32);
}

impl ExternalImage {
    /// The source `id` the page registered (`externalSource(source)` in
    /// `web/stipple.js`: a `<video>`, a `VideoFrame`, an `ImageBitmap` or a
    /// canvas), of `size` pixels: drawn by the GPU renderer straight from
    /// the source (WebGPU `importExternalTexture`, WebGL2 `texImage2D`),
    /// read once on the CPU by the software renderer. The page lets go of
    /// the source when the image is dropped.
    pub fn web_source(id: u32, size: PhysicalSize) -> Self {
        Self::web(id, size, true)
    }

    /// A source the page keeps (the camera's `<video>`), not let go of
    /// with the image.
    pub(crate) fn web(id: u32, size: PhysicalSize, owned: bool) -> Self {
        Self::with_handle(
            Handle::Web { id, owned },
            size,
            ExternalFormat::Rgbx8,
            vec![PlaneLayout {
                offset: 0,
                stride: size.width * 4,
            }],
            DRM_FORMAT_MOD_INVALID,
        )
    }

    /// The page's id of its source, for a web image.
    #[cfg_attr(not(feature = "gpu"), allow(dead_code))]
    pub(crate) fn web_id(&self) -> Option<u32> {
        match self.handle {
            Handle::Web { id, .. } => Some(id),
            _ => None,
        }
    }
}

/// Source `id`'s pixels at `size`.
pub(super) fn read(id: u32, size: PhysicalSize) -> Option<Pixmap> {
    let mut bytes = vec![0u8; size.width as usize * size.height as usize * 4];
    let ok = unsafe { external_read(id, bytes.as_mut_ptr(), size.width, size.height) };
    (ok != 0).then(|| Pixmap::from_rgba8(size, bytes))
}

/// The page can let go of source `id`.
pub(super) fn release(id: u32) {
    unsafe { external_release(id) }
}
