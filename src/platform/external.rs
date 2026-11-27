//! Images that live in another API's memory, drawn without a copy: a
//! `dma-buf` (Linux, Android), an `IOSurface` (macOS, iOS), a DXGI shared
//! handle (Windows) or an `AHardwareBuffer` (Android).
//!
//! An [`ExternalImage`] is a handle to such a buffer plus what an importer
//! needs to read it: its size, its [`ExternalFormat`], each plane's offset
//! and row stride, and (for a `dma-buf`) the DRM format modifier. A scene
//! draws one with [`Scene::draw_external`](crate::render::Scene::draw_external)
//! (or a region of it, rotated and mirrored, with
//! [`draw_external_region`](crate::render::Scene::draw_external_region)), in
//! place of a [`Pixmap`]: an embedded viewport's content
//! ([`App::set_viewport_external`](crate::App::set_viewport_external)), a
//! video frame, the camera's preview.
//!
//! The GPU renderers import the buffer as a texture, cached by the image's
//! identity like a `Pixmap`'s, with the API made for it:
//!
//! | Handle | Renderer | Import |
//! |---|---|---|
//! | `dma-buf` | GLES (Linux) | `EGL_EXT_image_dma_buf_import`, an `EGLImage` per plane, `glEGLImageTargetTexture2DOES` |
//! | `dma-buf` | Vulkan (Linux) | `VK_EXT_external_memory_dma_buf` (+ `VK_EXT_image_drm_format_modifier` for a modifier other than linear) |
//! | `IOSurface` | Metal | `newTextureWithDescriptor:iosurface:plane:`, a texture per plane |
//! | DXGI shared handle | Direct3D 11 | `ID3D11Device1::OpenSharedResource1` (NT handle) or `OpenSharedResource` |
//! | `AHardwareBuffer` | GLES (Android) | `eglGetNativeClientBufferANDROID`, `EGL_NATIVE_BUFFER_ANDROID` |
//!
//! Two-plane YUV ([`ExternalFormat::Nv12`]) is imported as two textures,
//! luma and chroma, and converted in the quad shader (GLES and Metal); a
//! renderer or a format with no import, and the software renderer, read the
//! buffer on the CPU instead ([`ExternalImage::pixmap`]: a `dma-buf` with a
//! linear layout is mapped, an `IOSurface` or `AHardwareBuffer` locked) and
//! draw that, converted once per image.
//!
//! **Lifetime and fences.** The image owns its handle: the `dma-buf` file
//! descriptors are closed, the `IOSurface` and `AHardwareBuffer` released,
//! an NT handle closed, when the last `Arc` to it is dropped, after the
//! callback given to [`on_release`](ExternalImage::on_release) runs (which
//! gives the buffer back to its producer: the camera's pool, a content
//! process). A renderer keeps the images of a frame alive until the GPU is
//! done with that frame, so dropping one after handing a scene over is safe.
//! A producer still writing the buffer on its GPU can attach a `sync_file`
//! ([`acquire_fence`](ExternalImage::acquire_fence), Linux and Android): it
//! is waited on before the buffer is imported or read. Beyond that,
//! `dma-buf`'s implicit synchronization, `IOSurface`'s and DXGI's own
//! orders the producer's writes before the reads.

#![allow(unsafe_code)] // the handles' FFI

use crate::geometry::PhysicalSize;
use crate::render::Pixmap;
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(target_arch = "wasm32")]
pub(crate) mod web;
mod yuv;
pub use yuv::YuvColorSpace;

#[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

/// How an [`ExternalImage`]'s pixels are laid out. Colors are straight
/// alpha (as a [`Pixmap`]'s); the formats without alpha are opaque.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExternalFormat {
    /// 8-bit R, G, B, A in memory order (DRM `ABGR8888`, Metal
    /// `RGBA8Unorm`, DXGI `R8G8B8A8_UNORM`).
    Rgba8,
    /// 8-bit B, G, R, A in memory order (DRM `ARGB8888`, `kCVPixelFormatType_32BGRA`,
    /// DXGI `B8G8R8A8_UNORM`).
    Bgra8,
    /// [`Rgba8`](Self::Rgba8) with the alpha byte ignored (DRM `XBGR8888`).
    Rgbx8,
    /// [`Bgra8`](Self::Bgra8) with the alpha byte ignored (DRM `XRGB8888`).
    Bgrx8,
    /// Two-plane YUV 4:2:0: a plane of luma bytes, then one of interleaved
    /// Cb, Cr pairs at half the width and height (DRM `NV12`,
    /// `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange`, DXGI `NV12`). Full
    /// range BT.601 (as the camera's frames) unless the image says otherwise
    /// ([`ExternalImage::color_space`]).
    Nv12,
    /// YUV 4:2:0 laid out as only its producer's driver knows: an Android
    /// camera's `AHardwareBuffer` (`AHARDWAREBUFFER_FORMAT_Y8Cb8Cr8_420`).
    /// The GPU samples it through an external texture that converts it
    /// (GLES `samplerExternalOES`); the CPU reads its planes through
    /// `AHardwareBuffer_lockPlanes` (Android 10). It has no DRM fourcc.
    Yuv420,
}

/// DRM fourcc codes (`drm_fourcc.h`).
#[allow(dead_code)] // for the GPU executors built in
pub(crate) mod fourcc {
    const fn code(s: &[u8; 4]) -> u32 {
        u32::from_le_bytes(*s)
    }
    pub const ABGR8888: u32 = code(b"AB24");
    pub const ARGB8888: u32 = code(b"AR24");
    pub const XBGR8888: u32 = code(b"XB24");
    pub const XRGB8888: u32 = code(b"XR24");
    pub const NV12: u32 = code(b"NV12");
    pub const R8: u32 = code(b"R8  ");
    pub const GR88: u32 = code(b"GR88");
}

/// The DRM format modifier of a buffer laid out row after row.
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;
/// "No explicit modifier": the layout the driver implies (linear for a
/// buffer from `udmabuf` or a dumb buffer).
pub const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

impl ExternalFormat {
    /// How many planes it has.
    pub fn planes(self) -> usize {
        match self {
            ExternalFormat::Nv12 => 2,
            _ => 1,
        }
    }

    /// Its DRM fourcc (what a `dma-buf`'s producer reports).
    pub fn drm_fourcc(self) -> u32 {
        match self {
            ExternalFormat::Rgba8 => fourcc::ABGR8888,
            ExternalFormat::Bgra8 => fourcc::ARGB8888,
            ExternalFormat::Rgbx8 => fourcc::XBGR8888,
            ExternalFormat::Bgrx8 => fourcc::XRGB8888,
            ExternalFormat::Nv12 => fourcc::NV12,
            ExternalFormat::Yuv420 => 0,
        }
    }

    /// The format with DRM fourcc `code`, if it's one of these.
    pub fn from_drm_fourcc(code: u32) -> Option<Self> {
        Some(match code {
            fourcc::ABGR8888 => ExternalFormat::Rgba8,
            fourcc::ARGB8888 => ExternalFormat::Bgra8,
            fourcc::XBGR8888 => ExternalFormat::Rgbx8,
            fourcc::XRGB8888 => ExternalFormat::Bgrx8,
            fourcc::NV12 => ExternalFormat::Nv12,
            _ => return None,
        })
    }

    /// The format of an `IOSurface` or `CVPixelBuffer` of pixel format
    /// `code` (a Core Video four-character code), if it's one of these.
    pub fn from_core_video(code: u32) -> Option<Self> {
        Some(match &code.to_be_bytes() {
            b"BGRA" => ExternalFormat::Bgra8,
            b"RGBA" => ExternalFormat::Rgba8,
            b"420f" => ExternalFormat::Nv12,
            _ => return None,
        })
    }

    /// Whether the alpha channel counts.
    pub fn has_alpha(self) -> bool {
        matches!(self, ExternalFormat::Rgba8 | ExternalFormat::Bgra8)
    }

    /// Plane `i`'s size in texels for an image of `size`, and its bytes per
    /// texel.
    #[allow(dead_code)] // for the GPU executors built in
    pub(crate) fn plane_extent(self, i: usize, size: PhysicalSize) -> (u32, u32, u32) {
        match (self, i) {
            (ExternalFormat::Nv12, 0) => (size.width, size.height, 1),
            (ExternalFormat::Nv12, _) => (size.width.div_ceil(2), size.height.div_ceil(2), 2),
            (ExternalFormat::Yuv420, _) => (size.width, size.height, 1),
            _ => (size.width, size.height, 4),
        }
    }

    /// The single-plane DRM formats its planes are imported as, one image
    /// (and texture) each: luma as `R8` and chroma as `GR88` for NV12, which
    /// every driver that imports a `dma-buf` takes, and samples as `.r` and
    /// `.rg`.
    #[allow(dead_code)] // for the GPU executors built in
    pub(crate) fn plane_fourccs(self) -> &'static [u32] {
        match self {
            ExternalFormat::Nv12 => &[fourcc::R8, fourcc::GR88],
            ExternalFormat::Rgba8 => &[fourcc::ABGR8888],
            ExternalFormat::Bgra8 => &[fourcc::ARGB8888],
            ExternalFormat::Rgbx8 => &[fourcc::XBGR8888],
            ExternalFormat::Bgrx8 => &[fourcc::XRGB8888],
            ExternalFormat::Yuv420 => &[],
        }
    }
}

/// Where one plane is in its buffer.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneLayout {
    /// Bytes from the start of the buffer (its `dma-buf`) to the plane.
    pub offset: u64,
    /// Bytes from one row to the next.
    pub stride: u32,
}

/// One plane of a `dma-buf` handed to [`ExternalImage::dma_buf`]: the file
/// descriptor (several planes may share one buffer; each comes with its own
/// descriptor, a `dup` of it then), and where the plane is in it.
#[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
#[non_exhaustive]
#[derive(Debug)]
pub struct DmaBufPlane {
    pub fd: OwnedFd,
    pub offset: u64,
    pub stride: u32,
}

#[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
impl DmaBufPlane {
    /// The plane at byte `offset` of the buffer `fd`, rows `stride` bytes
    /// apart.
    pub fn new(fd: OwnedFd, offset: u64, stride: u32) -> Self {
        Self { fd, offset, stride }
    }
}

/// Why an [`ExternalImage`] couldn't be made.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalError {
    /// The handle's format isn't one [`ExternalFormat`] has.
    UnsupportedFormat(u32),
    /// Not as many planes as the format has, or one that doesn't fit.
    BadPlanes,
    /// A null handle, an empty size, or a call the OS refused.
    Invalid(String),
}

impl std::fmt::Display for ExternalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExternalError::UnsupportedFormat(code) => {
                write!(f, "unsupported pixel format {code:#010x}")
            }
            ExternalError::BadPlanes => f.write_str("the planes don't match the format"),
            ExternalError::Invalid(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ExternalError {}

/// The OS handle an [`ExternalImage`] owns.
pub(crate) enum Handle {
    /// Only the pixels given with [`ExternalImage::cpu_pixels`] (tests,
    /// a producer that has nothing else).
    #[cfg_attr(not(test), allow(dead_code))]
    Pixels,
    /// One descriptor per plane.
    #[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
    DmaBuf(Vec<OwnedFd>),
    /// An `IOSurfaceRef`, retained.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    IoSurface(*mut std::ffi::c_void),
    /// A DXGI shared `HANDLE`; an NT handle (`nt`) is owned and closed.
    #[cfg(target_os = "windows")]
    Dxgi {
        handle: *mut std::ffi::c_void,
        nt: bool,
    },
    /// An `AHardwareBuffer*`, acquired.
    #[cfg(target_os = "android")]
    HardwareBuffer(*mut std::ffi::c_void),
    /// A source the page holds (`web/stipple.js`'s `externalSource`: a
    /// `<video>`, a `VideoFrame`, an `ImageBitmap`, a canvas), by its id;
    /// an `owned` one is let go of with the image.
    #[cfg(target_arch = "wasm32")]
    Web { id: u32, owned: bool },
}

type ReleaseFn = Box<dyn FnOnce() + Send>;

/// An image in another API's memory, drawn without a copy: a `dma-buf`
/// ([`dma_buf`](Self::dma_buf)), an `IOSurface`, a DXGI shared handle or an
/// `AHardwareBuffer`, with its size, [`ExternalFormat`], planes and DRM
/// format modifier.
///
/// Draw it with [`Scene::draw_external`](crate::render::Scene::draw_external),
/// or show it in a viewport with
/// [`App::set_viewport_external`](crate::App::set_viewport_external). The
/// GPU renderers import it as a texture (GLES: `dma-buf`, `AHardwareBuffer`;
/// Vulkan: `dma-buf`; Metal: `IOSurface`; Direct3D 11: shared handles); the
/// others, and the software renderer, read it on the CPU once
/// ([`pixmap`](Self::pixmap)). `docs/gpu.md` has the details.
///
/// It owns its handle, released when the last `Arc` goes, after the
/// [`on_release`](Self::on_release) callback. Renderers hold the images a
/// frame drew until the GPU is done with it. A new frame is a new image,
/// even of the same buffer. Share it as an `Arc`; the scene, the renderers
/// and the producer can each hold one.
pub struct ExternalImage {
    handle: Handle,
    size: PhysicalSize,
    format: ExternalFormat,
    planes: Vec<PlaneLayout>,
    modifier: u64,
    color: YuvColorSpace,
    #[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
    acquire: Mutex<Option<OwnedFd>>,
    pixmap: OnceLock<Option<Arc<Pixmap>>>,
    release: Mutex<Option<ReleaseFn>>,
}

// The handles are reference-counted OS objects (an `IOSurfaceRef`, an
// `AHardwareBuffer*`, a `HANDLE`) that any thread may use and release.
unsafe impl Send for ExternalImage {}
unsafe impl Sync for ExternalImage {}

impl std::fmt::Debug for ExternalImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.handle {
            Handle::Pixels => "pixels",
            #[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
            Handle::DmaBuf(_) => "dma-buf",
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            Handle::IoSurface(_) => "IOSurface",
            #[cfg(target_os = "windows")]
            Handle::Dxgi { .. } => "DXGI shared handle",
            #[cfg(target_os = "android")]
            Handle::HardwareBuffer(_) => "AHardwareBuffer",
            #[cfg(target_arch = "wasm32")]
            Handle::Web { .. } => "web source",
        };
        f.debug_struct("ExternalImage")
            .field("handle", &kind)
            .field("size", &self.size)
            .field("format", &self.format)
            .field("planes", &self.planes)
            .field("modifier", &format_args!("{:#x}", self.modifier))
            .field("color", &self.color)
            .finish_non_exhaustive()
    }
}

impl ExternalImage {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn with_handle(
        handle: Handle,
        size: PhysicalSize,
        format: ExternalFormat,
        planes: Vec<PlaneLayout>,
        modifier: u64,
    ) -> Self {
        Self {
            handle,
            size,
            format,
            planes,
            modifier,
            color: YuvColorSpace::default(),
            #[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
            acquire: Mutex::new(None),
            pixmap: OnceLock::new(),
            release: Mutex::new(None),
        }
    }

    /// An image with no OS buffer behind it, only `pixels` (each renderer
    /// uploads them as an image): for tests of the scene's plumbing, and a
    /// software video decoder's frames.
    #[cfg_attr(
        not(any(test, all(feature = "video", not(target_arch = "wasm32")))),
        allow(dead_code)
    )]
    pub(crate) fn from_pixels(pixels: Arc<Pixmap>) -> Self {
        let size = pixels.size();
        let me = Self::with_handle(
            Handle::Pixels,
            size,
            ExternalFormat::Rgba8,
            vec![PlaneLayout {
                offset: 0,
                stride: pixels.stride() as u32,
            }],
            DRM_FORMAT_MOD_LINEAR,
        );
        let _ = me.pixmap.set(Some(pixels));
        me
    }

    /// A `dma-buf` of `size` pixels in `format`, with `planes` (one per plane
    /// of the format) and the DRM format `modifier` its producer reported
    /// ([`DRM_FORMAT_MOD_LINEAR`], [`DRM_FORMAT_MOD_INVALID`] or a tiled one).
    /// The descriptors are the image's from now on.
    #[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
    pub fn dma_buf(
        size: PhysicalSize,
        format: ExternalFormat,
        modifier: u64,
        planes: Vec<DmaBufPlane>,
    ) -> Result<Self, ExternalError> {
        if size.width == 0 || size.height == 0 {
            return Err(ExternalError::Invalid("an empty dma-buf".into()));
        }
        if format == ExternalFormat::Yuv420 {
            return Err(ExternalError::UnsupportedFormat(0));
        }
        if planes.len() != format.planes() {
            return Err(ExternalError::BadPlanes);
        }
        for (i, p) in planes.iter().enumerate() {
            let (w, _, bpp) = format.plane_extent(i, size);
            if (p.stride as u64) < w as u64 * bpp as u64 && modifier_is_linear(modifier) {
                return Err(ExternalError::BadPlanes);
            }
        }
        let layout = planes
            .iter()
            .map(|p| PlaneLayout {
                offset: p.offset,
                stride: p.stride,
            })
            .collect();
        let fds = planes.into_iter().map(|p| p.fd).collect();
        Ok(Self::with_handle(
            Handle::DmaBuf(fds),
            size,
            format,
            layout,
            modifier,
        ))
    }

    /// The `IOSurfaceRef` `surface` (retained here, released when the image
    /// is dropped): a `CVPixelBuffer`'s (`CVPixelBufferGetIOSurface`), a
    /// surface looked up by its global id, one a Metal texture was made
    /// from. Its pixel format must be `BGRA`, `RGBA`, `420f` or `420v` (NV12
    /// in the video range: [`YuvColorSpace::Bt601Limited`]).
    ///
    /// # Safety
    ///
    /// `surface` must be a valid `IOSurfaceRef`.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub unsafe fn io_surface(surface: *mut std::ffi::c_void) -> Result<Self, ExternalError> {
        if surface.is_null() {
            return Err(ExternalError::Invalid("a null IOSurfaceRef".into()));
        }
        unsafe {
            let code = apple::IOSurfaceGetPixelFormat(surface);
            let video_range = code == u32::from_be_bytes(*b"420v");
            let format = if video_range {
                ExternalFormat::Nv12
            } else {
                ExternalFormat::from_core_video(code)
                    .ok_or(ExternalError::UnsupportedFormat(code))?
            };
            let size = PhysicalSize::new(
                apple::IOSurfaceGetWidth(surface) as u32,
                apple::IOSurfaceGetHeight(surface) as u32,
            );
            if size.width == 0 || size.height == 0 {
                return Err(ExternalError::Invalid("an empty IOSurface".into()));
            }
            let planes = if format.planes() == 1 {
                vec![PlaneLayout {
                    offset: 0,
                    stride: apple::IOSurfaceGetBytesPerRow(surface) as u32,
                }]
            } else {
                if apple::IOSurfaceGetPlaneCount(surface) < format.planes() {
                    return Err(ExternalError::BadPlanes);
                }
                (0..format.planes())
                    .map(|i| PlaneLayout {
                        offset: 0,
                        stride: apple::IOSurfaceGetBytesPerRowOfPlane(surface, i) as u32,
                    })
                    .collect()
            };
            apple::CFRetain(surface);
            let image = Self::with_handle(
                Handle::IoSurface(surface),
                size,
                format,
                planes,
                DRM_FORMAT_MOD_INVALID,
            );
            Ok(if video_range {
                image.color_space(YuvColorSpace::Bt601Limited)
            } else {
                image
            })
        }
    }

    /// A DXGI shared handle to a texture of `size` pixels in `format`
    /// (`Rgba8`, `Bgra8` and their X forms, or `Nv12`: a `DXGI_FORMAT_NV12`
    /// texture, as a video decoder gives, its planes sampled through an `R8`
    /// and an `R8G8` view and converted in the shader): an NT handle (`nt`, from
    /// `IDXGIResource1::CreateSharedHandle`; the image owns it and closes
    /// it) or a legacy one (`IDXGIResource::GetSharedHandle`; not owned).
    ///
    /// The software renderer can't read such a texture without a device of
    /// its own: give it pixels with [`cpu_pixels`](Self::cpu_pixels) where
    /// it might draw one.
    ///
    /// # Safety
    ///
    /// `handle` must be a shared handle to such a texture.
    #[cfg(target_os = "windows")]
    pub unsafe fn dxgi_shared_handle(
        handle: *mut std::ffi::c_void,
        nt: bool,
        size: PhysicalSize,
        format: ExternalFormat,
    ) -> Result<Self, ExternalError> {
        if handle.is_null() || size.width == 0 || size.height == 0 {
            return Err(ExternalError::Invalid("a null shared handle".into()));
        }
        if format == ExternalFormat::Yuv420 {
            return Err(ExternalError::UnsupportedFormat(0));
        }
        let planes = (0..format.planes())
            .map(|i| {
                let (w, _, bpp) = format.plane_extent(i, size);
                PlaneLayout {
                    offset: 0,
                    stride: w * bpp,
                }
            })
            .collect();
        Ok(Self::with_handle(
            Handle::Dxgi { handle, nt },
            size,
            format,
            planes,
            DRM_FORMAT_MOD_INVALID,
        ))
    }

    /// The `AHardwareBuffer*` `buffer` (acquired here, released when the
    /// image is dropped). Its format must be `R8G8B8A8_UNORM`,
    /// `R8G8B8X8_UNORM` or `Y8Cb8Cr8_420` (a camera's YUV frames:
    /// [`ExternalFormat::Yuv420`], sampled through an external texture).
    ///
    /// # Safety
    ///
    /// `buffer` must be a valid `AHardwareBuffer*`.
    #[cfg(target_os = "android")]
    pub unsafe fn hardware_buffer(buffer: *mut std::ffi::c_void) -> Result<Self, ExternalError> {
        if buffer.is_null() {
            return Err(ExternalError::Invalid("a null AHardwareBuffer".into()));
        }
        let lib = ahb::lib().ok_or_else(|| {
            ExternalError::Invalid("AHardwareBuffer needs Android 8 (API 26)".into())
        })?;
        unsafe {
            let mut d = ahb::Desc::default();
            lib.describe(buffer, &mut d);
            let format = match d.format {
                ahb::FORMAT_R8G8B8A8 => ExternalFormat::Rgba8,
                ahb::FORMAT_R8G8B8X8 => ExternalFormat::Rgbx8,
                ahb::FORMAT_Y8CB8CR8_420 => ExternalFormat::Yuv420,
                other => return Err(ExternalError::UnsupportedFormat(other)),
            };
            if d.width == 0 || d.height == 0 {
                return Err(ExternalError::Invalid("an empty AHardwareBuffer".into()));
            }
            lib.acquire(buffer);
            Ok(Self::with_handle(
                Handle::HardwareBuffer(buffer),
                PhysicalSize::new(d.width, d.height),
                format,
                vec![PlaneLayout {
                    offset: 0,
                    stride: d.stride * format.plane_extent(0, PhysicalSize::new(1, 1)).2,
                }],
                DRM_FORMAT_MOD_INVALID,
            ))
        }
    }

    /// The YUV color space of an [`Nv12`](ExternalFormat::Nv12) image (what
    /// its producer said: BT.601 or BT.709, full or limited range); full
    /// range BT.601 if not given. The other formats ignore it.
    pub fn color_space(mut self, color: YuvColorSpace) -> Self {
        self.color = color;
        self
    }

    /// Its YUV color space ([`color_space`](Self::color_space)).
    pub fn yuv_color_space(&self) -> YuvColorSpace {
        self.color
    }

    /// Pixels for a renderer that can't import the buffer (the software
    /// renderer, a DXGI texture under it): kept as [`pixmap`](Self::pixmap)
    /// instead of reading the buffer.
    pub fn cpu_pixels(self, pixels: Arc<Pixmap>) -> Self {
        let _ = self.pixmap.set(Some(pixels));
        self
    }

    /// Run `f` when the image is dropped (before its handle is released):
    /// to give the buffer back to whoever lent it.
    pub fn on_release(self, f: impl FnOnce() + Send + 'static) -> Self {
        *self.release.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
        self
    }

    /// A `sync_file` that signals when the producer's GPU is done writing
    /// the buffer: waited on before it is imported or read.
    #[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
    pub fn acquire_fence(self, fence: OwnedFd) -> Self {
        *self.acquire.lock().unwrap_or_else(|e| e.into_inner()) = Some(fence);
        self
    }

    /// Its size in pixels.
    pub fn size(&self) -> PhysicalSize {
        self.size
    }

    /// How its pixels are laid out.
    pub fn format(&self) -> ExternalFormat {
        self.format
    }

    /// Each plane's place in the buffer.
    pub fn planes(&self) -> &[PlaneLayout] {
        &self.planes
    }

    /// The DRM format modifier (a `dma-buf`'s; [`DRM_FORMAT_MOD_INVALID`]
    /// for the other handles).
    pub fn modifier(&self) -> u64 {
        self.modifier
    }

    /// The pixels, read from the buffer on the CPU once and kept (or the
    /// ones given with [`cpu_pixels`](Self::cpu_pixels)), converted to
    /// straight RGBA: what the software renderer draws, and a GPU renderer
    /// that can't import the buffer. `None` when it can't be read here (a
    /// tiled `dma-buf`, a DXGI texture).
    pub fn pixmap(&self) -> Option<Arc<Pixmap>> {
        self.pixmap
            .get_or_init(|| {
                self.wait_acquire();
                self.read().map(Arc::new)
            })
            .clone()
    }

    /// Wait (a little) for the producer's fence, if it gave one, and drop
    /// it: once signaled it says nothing more.
    pub(crate) fn wait_acquire(&self) {
        #[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
        if let Some(fence) = self
            .acquire
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            unix::wait_fence(fence.as_raw_fd(), FENCE_TIMEOUT_MS);
        }
    }

    #[allow(dead_code)] // for the GPU executors built in
    pub(crate) fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Each plane's descriptor, offset and stride, for a `dma-buf`.
    #[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
    #[allow(dead_code)] // for the GPU executors built in
    pub(crate) fn dma_buf_planes(&self) -> Option<Vec<(RawFd, u64, u32)>> {
        match &self.handle {
            Handle::DmaBuf(fds) => Some(
                fds.iter()
                    .zip(&self.planes)
                    .map(|(fd, p)| (fd.as_raw_fd(), p.offset, p.stride))
                    .collect(),
            ),
            _ => None,
        }
    }

    /// Read the buffer into straight RGBA, if it can be read here.
    fn read(&self) -> Option<Pixmap> {
        match &self.handle {
            Handle::Pixels => None,
            #[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
            Handle::DmaBuf(fds) => {
                if !modifier_is_linear(self.modifier) {
                    return None;
                }
                unix::read_dma_buf(fds, &self.planes, self.format, self.size, self.color)
            }
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            Handle::IoSurface(s) => unsafe { apple::read(*s, self.format, self.size, self.color) },
            #[cfg(target_os = "windows")]
            Handle::Dxgi { .. } => None,
            #[cfg(target_os = "android")]
            Handle::HardwareBuffer(b) => unsafe { ahb::read(*b, self.format, self.size) },
            #[cfg(target_arch = "wasm32")]
            Handle::Web { id, .. } => web::read(*id, self.size),
        }
    }
}

impl Drop for ExternalImage {
    fn drop(&mut self) {
        if let Some(f) = self
            .release
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            f();
        }
        match &self.handle {
            Handle::Pixels => {}
            #[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
            Handle::DmaBuf(_) => {} // the `OwnedFd`s close themselves
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            Handle::IoSurface(s) => unsafe { apple::CFRelease(*s) },
            #[cfg(target_os = "windows")]
            Handle::Dxgi { handle, nt } => {
                if *nt {
                    unsafe { win::CloseHandle(*handle) };
                }
            }
            #[cfg(target_os = "android")]
            Handle::HardwareBuffer(b) => {
                if let Some(lib) = ahb::lib() {
                    unsafe { lib.release(*b) };
                }
            }
            #[cfg(target_arch = "wasm32")]
            Handle::Web { id, owned } => {
                if *owned {
                    web::release(*id);
                }
            }
        }
    }
}

/// How long a producer's fence is waited on before the buffer is read
/// anyway (a frame or two at 60 Hz: a fence later than that is a producer
/// that stalled, and a torn frame beats a frozen window).
#[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
const FENCE_TIMEOUT_MS: i32 = 32;

/// Whether `modifier` means rows in order, so the CPU can read it.
#[allow(dead_code)] // for the GPU executors built in
pub(crate) fn modifier_is_linear(modifier: u64) -> bool {
    modifier == DRM_FORMAT_MOD_LINEAR || modifier == DRM_FORMAT_MOD_INVALID
}

// ---- Telling producers what the renderer imports ---------------------------

/// What the renderer that drew the last frame imports without a copy, as a
/// bit per [`ExternalFormat`], and when it said so.
static NATIVE: Mutex<(u32, Option<crate::platform::time::Instant>)> = Mutex::new((0, None));

/// Held by tests that set what's imported natively.
#[cfg(test)]
pub(crate) static NATIVE_TEST: Mutex<()> = Mutex::new(());

fn bit(format: ExternalFormat) -> u32 {
    1 << (format as u32)
}

/// A GPU renderer drew a frame and imports `formats` without a copy.
#[allow(dead_code)] // by the GPU executors built in
pub(crate) fn note_native(formats: &[ExternalFormat]) {
    let mask = formats.iter().fold(0, |m, f| m | bit(*f));
    *NATIVE.lock().unwrap_or_else(|e| e.into_inner()) = (mask, Some(crate::platform::time::now()));
}

/// Whether frames are being drawn (in the last half second) by a renderer
/// that imports `format` from this platform's buffers without a copy: a
/// producer that would otherwise convert each frame on the CPU (the camera)
/// hands the buffer over instead.
pub fn imported_natively(format: ExternalFormat) -> bool {
    let (mask, at) = *NATIVE.lock().unwrap_or_else(|e| e.into_inner());
    mask & bit(format) != 0
        && at.is_some_and(|t| {
            crate::platform::time::now().saturating_duration_since(t)
                < std::time::Duration::from_millis(500)
        })
}

// ---- Converting on the CPU -------------------------------------------------

/// Convert `planes` (each its bytes from the plane's first row, and its
/// stride) of a `size` image in `format` (YUV in `color`) to straight RGBA.
#[allow(dead_code)] // for the GPU executors built in
pub(crate) fn to_rgba(
    format: ExternalFormat,
    size: PhysicalSize,
    planes: &[(&[u8], usize)],
    color: YuvColorSpace,
) -> Option<Pixmap> {
    let (w, h) = (size.width as usize, size.height as usize);
    if planes.len() < format.planes() {
        return None;
    }
    let mut out = vec![0u8; w * h * 4];
    match format {
        ExternalFormat::Nv12 => {
            let ((y, ys), (uv, uvs)) = (planes[0], planes[1]);
            for row in 0..h {
                let yr = y.get(row * ys..row * ys + w)?;
                let cr = uv.get((row / 2) * uvs..(row / 2) * uvs + w.div_ceil(2) * 2)?;
                for (x, px) in out[row * w * 4..(row + 1) * w * 4]
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .enumerate()
                {
                    let [r, g, b] = color.to_rgb(yr[x], cr[(x / 2) * 2], cr[(x / 2) * 2 + 1]);
                    *px = [r, g, b, 255];
                }
            }
        }
        // Read through its planes' own layout (`ahb::read`), never here.
        ExternalFormat::Yuv420 => return None,
        _ => {
            let (src, stride) = planes[0];
            let (bgr, opaque) = match format {
                ExternalFormat::Rgba8 => (false, false),
                ExternalFormat::Bgra8 => (true, false),
                ExternalFormat::Rgbx8 => (false, true),
                _ => (true, true),
            };
            for row in 0..h {
                let s = src.get(row * stride..row * stride + w * 4)?;
                let d = &mut out[row * w * 4..(row + 1) * w * 4];
                d.copy_from_slice(s);
                for px in d.as_chunks_mut::<4>().0 {
                    if bgr {
                        px.swap(0, 2);
                    }
                    if opaque {
                        px[3] = 255;
                    }
                }
            }
        }
    }
    Some(Pixmap::from_rgba8(size, out))
}

// ---- Import descriptors (pure, so tested on every host) --------------------

/// `EGL_EXT_image_dma_buf_import`'s attribute names.
#[allow(dead_code)] // for the GPU executors built in
pub(crate) mod egl {
    pub const WIDTH: i32 = 0x3057;
    pub const HEIGHT: i32 = 0x3056;
    pub const NONE: i32 = 0x3038;
    pub const LINUX_DRM_FOURCC: i32 = 0x3271;
    pub const PLANE0_FD: i32 = 0x3272;
    pub const PLANE0_OFFSET: i32 = 0x3273;
    pub const PLANE0_PITCH: i32 = 0x3274;
    pub const PLANE0_MODIFIER_LO: i32 = 0x3443;
    pub const PLANE0_MODIFIER_HI: i32 = 0x3444;
    pub const PLANE1_FD: i32 = 0x3275;
    pub const PLANE1_OFFSET: i32 = 0x3276;
    pub const PLANE1_PITCH: i32 = 0x3277;
    pub const PLANE1_MODIFIER_LO: i32 = 0x3445;
    pub const PLANE1_MODIFIER_HI: i32 = 0x3446;
    pub const YUV_COLOR_SPACE_HINT: i32 = 0x327B;
    pub const SAMPLE_RANGE_HINT: i32 = 0x327C;
    pub const ITU_REC601: i32 = 0x327F;
    pub const ITU_REC709: i32 = 0x3280;
    pub const YUV_FULL_RANGE: i32 = 0x3282;
    pub const YUV_NARROW_RANGE: i32 = 0x3283;
    pub const CHROMA_HORIZONTAL_SITING_HINT: i32 = 0x327D;
    pub const CHROMA_VERTICAL_SITING_HINT: i32 = 0x327E;
    pub const CHROMA_SITING_0_5: i32 = 0x3285;
}

/// The attribute list for `eglCreateImageKHR(EGL_LINUX_DMA_BUF_EXT)` of all
/// of `image`'s planes (`planes`: each one's descriptor, offset and stride)
/// as one image: its DRM fourcc and modifier, and for NV12 its color space
/// and range as hints for the driver's conversion. `None` for more than two
/// planes.
#[allow(dead_code)] // for the GPU executors built in
pub(crate) fn egl_dma_buf_image_attribs(
    image: &ExternalImage,
    planes: &[(i32, u64, u32)],
) -> Option<Vec<i32>> {
    let size = image.size;
    let (fd, offset, stride) = *planes.first()?;
    let mut a = egl_dma_buf_attribs(
        size.width,
        size.height,
        image.format.drm_fourcc(),
        fd,
        offset,
        stride,
        image.modifier,
    );
    a.pop(); // NONE
    match planes {
        [_] => {}
        [_, (fd, offset, stride)] => {
            a.extend([
                egl::PLANE1_FD,
                *fd,
                egl::PLANE1_OFFSET,
                *offset as i32,
                egl::PLANE1_PITCH,
                *stride as i32,
            ]);
            if image.modifier != DRM_FORMAT_MOD_INVALID {
                a.extend([
                    egl::PLANE1_MODIFIER_LO,
                    image.modifier as u32 as i32,
                    egl::PLANE1_MODIFIER_HI,
                    (image.modifier >> 32) as u32 as i32,
                ]);
            }
        }
        _ => return None,
    }
    if image.format == ExternalFormat::Nv12 {
        use YuvColorSpace::*;
        let (space, range) = match image.yuv_color_space() {
            Bt601Full => (egl::ITU_REC601, egl::YUV_FULL_RANGE),
            Bt601Limited => (egl::ITU_REC601, egl::YUV_NARROW_RANGE),
            Bt709Full => (egl::ITU_REC709, egl::YUV_FULL_RANGE),
            Bt709Limited => (egl::ITU_REC709, egl::YUV_NARROW_RANGE),
        };
        // Chroma between its luma samples, as the plane textures sample it.
        a.extend([
            egl::YUV_COLOR_SPACE_HINT,
            space,
            egl::SAMPLE_RANGE_HINT,
            range,
            egl::CHROMA_HORIZONTAL_SITING_HINT,
            egl::CHROMA_SITING_0_5,
            egl::CHROMA_VERTICAL_SITING_HINT,
            egl::CHROMA_SITING_0_5,
        ]);
    }
    a.push(egl::NONE);
    Some(a)
}

/// The attribute list for `eglCreateImageKHR(EGL_LINUX_DMA_BUF_EXT)` of one
/// single-plane image: `width`×`height` texels of DRM `fourcc` at `offset`
/// with rows `stride` bytes apart in `fd`, with the `modifier` echoed (the
/// layout of a tiled buffer; left out when it's
/// [`DRM_FORMAT_MOD_INVALID`]).
#[allow(dead_code)] // for the GPU executors built in
pub(crate) fn egl_dma_buf_attribs(
    width: u32,
    height: u32,
    fourcc: u32,
    fd: i32,
    offset: u64,
    stride: u32,
    modifier: u64,
) -> Vec<i32> {
    let mut a = vec![
        egl::WIDTH,
        width as i32,
        egl::HEIGHT,
        height as i32,
        egl::LINUX_DRM_FOURCC,
        fourcc as i32,
        egl::PLANE0_FD,
        fd,
        egl::PLANE0_OFFSET,
        offset as i32,
        egl::PLANE0_PITCH,
        stride as i32,
    ];
    if modifier != DRM_FORMAT_MOD_INVALID {
        a.extend([
            egl::PLANE0_MODIFIER_LO,
            modifier as u32 as i32,
            egl::PLANE0_MODIFIER_HI,
            (modifier >> 32) as u32 as i32,
        ]);
    }
    a.push(egl::NONE);
    a
}

/// What each plane of an image is imported as, one single-plane image (and
/// texture) per plane: `(width, height, DRM fourcc, offset, stride)`.
#[allow(dead_code)] // for the GPU executors built in
pub(crate) fn plane_imports(image: &ExternalImage) -> Vec<(u32, u32, u32, u64, u32)> {
    let f = image.format;
    f.plane_fourccs()
        .iter()
        .zip(&image.planes)
        .enumerate()
        .map(|(i, (&code, p))| {
            let (w, h, _) = f.plane_extent(i, image.size);
            (w, h, code, p.offset, p.stride)
        })
        .collect()
}

// ---- Linux and Android: reading a dma-buf, sync_files, udmabuf ---------------

#[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "android"))]
mod unix {
    use super::*;
    use std::ffi::{c_int, c_ulong, c_void};

    unsafe extern "C" {
        fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            off: i64,
        ) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> c_int;
        fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
        fn poll(fds: *mut PollFd, n: c_ulong, timeout: c_int) -> c_int;
    }

    #[repr(C)]
    pub(super) struct PollFd {
        fd: c_int,
        events: i16,
        revents: i16,
    }

    const PROT_READ: c_int = 1;
    const MAP_SHARED: c_int = 1;
    const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;
    const POLLIN: i16 = 1;
    /// `DMA_BUF_IOCTL_SYNC`: `_IOW('b', 0, struct dma_buf_sync)`.
    const DMA_BUF_IOCTL_SYNC: c_ulong = 0x4008_6200;
    const DMA_BUF_SYNC_READ: u64 = 1;
    const DMA_BUF_SYNC_END: u64 = 4;

    /// Wait up to `ms` for the `sync_file` `fd` to signal.
    pub(super) fn wait_fence(fd: RawFd, ms: i32) {
        let mut p = PollFd {
            fd,
            events: POLLIN,
            revents: 0,
        };
        unsafe { poll(&mut p, 1, ms) };
    }

    /// A read-only mapping of the first `len` bytes of a `dma-buf`, inside a
    /// CPU access window (`DMA_BUF_IOCTL_SYNC`) while it's alive.
    struct Map {
        fd: RawFd,
        ptr: *mut c_void,
        len: usize,
    }

    impl Map {
        fn new(fd: RawFd, len: usize) -> Option<Map> {
            let ptr = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ, MAP_SHARED, fd, 0) };
            if ptr == MAP_FAILED || ptr.is_null() {
                return None;
            }
            let start: u64 = DMA_BUF_SYNC_READ;
            unsafe { ioctl(fd, DMA_BUF_IOCTL_SYNC, &start) };
            Some(Map { fd, ptr, len })
        }

        fn bytes(&self) -> &[u8] {
            unsafe { std::slice::from_raw_parts(self.ptr as *const u8, self.len) }
        }
    }

    impl Drop for Map {
        fn drop(&mut self) {
            let end: u64 = DMA_BUF_SYNC_READ | DMA_BUF_SYNC_END;
            unsafe {
                ioctl(self.fd, DMA_BUF_IOCTL_SYNC, &end);
                munmap(self.ptr, self.len);
            }
        }
    }

    /// Map each plane of a linear `dma-buf` and convert it.
    pub(super) fn read_dma_buf(
        fds: &[OwnedFd],
        planes: &[PlaneLayout],
        format: ExternalFormat,
        size: PhysicalSize,
        color: YuvColorSpace,
    ) -> Option<Pixmap> {
        let mut maps = Vec::new();
        for (i, (fd, p)) in fds.iter().zip(planes).enumerate() {
            let (_, rows, _) = format.plane_extent(i, size);
            let len = p.offset as usize + p.stride as usize * rows as usize;
            maps.push((
                Map::new(fd.as_raw_fd(), len)?,
                p.offset as usize,
                p.stride as usize,
            ));
        }
        let views: Vec<(&[u8], usize)> = maps
            .iter()
            .map(|(m, off, stride)| (&m.bytes()[*off..], *stride))
            .collect();
        to_rgba(format, size, &views, color)
    }
}

/// A `dma-buf` holding `bytes`, made by the kernel's `udmabuf` driver from a
/// sealed `memfd` (`/dev/udmabuf`, Linux 5.2 and later): a buffer a CPU
/// producer (a content process drawing in software, a test) can hand to
/// anything that imports a `dma-buf`, with a linear layout. `bytes` is
/// padded to whole pages.
#[cfg(any(target_os = "linux", target_os = "fullrust"))]
pub fn udmabuf(bytes: &[u8]) -> std::io::Result<OwnedFd> {
    use std::ffi::{c_char, c_int, c_ulong};
    use std::io::{Error, ErrorKind};
    use std::os::fd::FromRawFd;
    unsafe extern "C" {
        fn memfd_create(name: *const c_char, flags: u32) -> c_int;
        fn ftruncate(fd: c_int, len: i64) -> c_int;
        fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
        fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
        fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        fn pwrite(fd: c_int, buf: *const u8, n: usize, off: i64) -> isize;
    }
    #[repr(C)]
    struct Create {
        memfd: u32,
        flags: u32,
        offset: u64,
        size: u64,
    }
    const MFD_CLOEXEC: u32 = 1;
    const MFD_ALLOW_SEALING: u32 = 2;
    const F_ADD_SEALS: c_int = 1033;
    const F_SEAL_SHRINK: c_int = 2;
    const O_RDWR: c_int = 2;
    const O_CLOEXEC: c_int = 0o2000000;
    /// `UDMABUF_CREATE`: `_IOW('u', 0x42, struct udmabuf_create)`.
    const UDMABUF_CREATE: c_ulong = 0x4018_7542;
    const UDMABUF_FLAGS_CLOEXEC: u32 = 1;
    const PAGE: usize = 4096;
    if bytes.is_empty() {
        return Err(Error::new(ErrorKind::InvalidInput, "an empty buffer"));
    }
    let len = bytes.len().next_multiple_of(PAGE);
    unsafe {
        let memfd = memfd_create(c"stipple-udmabuf".as_ptr(), MFD_CLOEXEC | MFD_ALLOW_SEALING);
        if memfd < 0 {
            return Err(Error::last_os_error());
        }
        let memfd = OwnedFd::from_raw_fd(memfd);
        if ftruncate(memfd.as_raw_fd(), len as i64) < 0
            || pwrite(memfd.as_raw_fd(), bytes.as_ptr(), bytes.len(), 0) != bytes.len() as isize
            || fcntl(memfd.as_raw_fd(), F_ADD_SEALS, F_SEAL_SHRINK) < 0
        {
            return Err(Error::last_os_error());
        }
        let dev = open(c"/dev/udmabuf".as_ptr(), O_RDWR | O_CLOEXEC);
        if dev < 0 {
            return Err(Error::last_os_error());
        }
        let dev = OwnedFd::from_raw_fd(dev);
        let create = Create {
            memfd: memfd.as_raw_fd() as u32,
            flags: UDMABUF_FLAGS_CLOEXEC,
            offset: 0,
            size: len as u64,
        };
        let buf = ioctl(dev.as_raw_fd(), UDMABUF_CREATE, &create);
        if buf < 0 {
            return Err(Error::last_os_error());
        }
        Ok(OwnedFd::from_raw_fd(buf))
    }
}

// ---- macOS and iOS: IOSurface ------------------------------------------------

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) mod apple {
    use super::*;
    use std::ffi::c_void;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFRetain(cf: *const c_void) -> *const c_void;
        pub fn CFRelease(cf: *const c_void);
    }

    #[link(name = "IOSurface", kind = "framework")]
    unsafe extern "C" {
        pub fn IOSurfaceGetWidth(s: *mut c_void) -> usize;
        pub fn IOSurfaceGetHeight(s: *mut c_void) -> usize;
        pub fn IOSurfaceGetPixelFormat(s: *mut c_void) -> u32;
        pub fn IOSurfaceGetBytesPerRow(s: *mut c_void) -> usize;
        pub fn IOSurfaceGetBaseAddress(s: *mut c_void) -> *mut c_void;
        pub fn IOSurfaceGetPlaneCount(s: *mut c_void) -> usize;
        pub fn IOSurfaceGetBytesPerRowOfPlane(s: *mut c_void, plane: usize) -> usize;
        pub fn IOSurfaceGetBaseAddressOfPlane(s: *mut c_void, plane: usize) -> *mut c_void;
        pub fn IOSurfaceLock(s: *mut c_void, options: u32, seed: *mut u32) -> i32;
        pub fn IOSurfaceUnlock(s: *mut c_void, options: u32, seed: *mut u32) -> i32;
    }

    const READ_ONLY: u32 = 1;

    /// Lock the surface read-only and convert it.
    pub(super) unsafe fn read(
        s: *mut c_void,
        format: ExternalFormat,
        size: PhysicalSize,
        color: YuvColorSpace,
    ) -> Option<Pixmap> {
        unsafe {
            if IOSurfaceLock(s, READ_ONLY, std::ptr::null_mut()) != 0 {
                return None;
            }
            let mut views = Vec::new();
            for i in 0..format.planes() {
                let (_, rows, _) = format.plane_extent(i, size);
                let (base, stride) = if format.planes() == 1 {
                    (IOSurfaceGetBaseAddress(s), IOSurfaceGetBytesPerRow(s))
                } else {
                    (
                        IOSurfaceGetBaseAddressOfPlane(s, i),
                        IOSurfaceGetBytesPerRowOfPlane(s, i),
                    )
                };
                if base.is_null() {
                    break;
                }
                views.push((
                    std::slice::from_raw_parts(base as *const u8, stride * rows as usize),
                    stride,
                ));
            }
            let out = to_rgba(format, size, &views, color);
            IOSurfaceUnlock(s, READ_ONLY, std::ptr::null_mut());
            out
        }
    }
}

// ---- Windows -----------------------------------------------------------------

#[cfg(target_os = "windows")]
mod win {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn CloseHandle(h: *mut std::ffi::c_void) -> i32;
    }
}

// ---- Android: AHardwareBuffer (API 26, looked up at run time) ------------------

#[cfg(target_os = "android")]
pub(crate) mod ahb {
    use super::*;
    use std::ffi::{c_int, c_void};

    pub const FORMAT_R8G8B8A8: u32 = 1;
    pub const FORMAT_R8G8B8X8: u32 = 2;
    /// `AHARDWAREBUFFER_FORMAT_Y8Cb8Cr8_420`: what an `AImageReader` of
    /// `YUV_420_888` images holds.
    pub const FORMAT_Y8CB8CR8_420: u32 = 0x23;
    const USAGE_CPU_READ_OFTEN: u64 = 3;

    /// `AHardwareBuffer_Plane`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Plane {
        data: *mut c_void,
        pixel_stride: u32,
        row_stride: u32,
    }

    /// `AHardwareBuffer_Planes`.
    #[repr(C)]
    struct Planes {
        count: u32,
        planes: [Plane; 4],
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct Desc {
        pub width: u32,
        pub height: u32,
        pub layers: u32,
        pub format: u32,
        pub usage: u64,
        pub stride: u32,
        pub rfu0: u32,
        pub rfu1: u64,
    }

    dlsym::library! {
        pub struct Lib {
            unsafe extern "C" {
                #[link_name = "AHardwareBuffer_acquire"]
                pub fn acquire(b: *mut c_void);
                #[link_name = "AHardwareBuffer_release"]
                pub fn release(b: *mut c_void);
                #[link_name = "AHardwareBuffer_describe"]
                pub fn describe(b: *const c_void, desc: *mut Desc);
                #[link_name = "AHardwareBuffer_lock"]
                fn lock(
                    b: *mut c_void,
                    usage: u64,
                    fence: i32,
                    rect: *const c_void,
                    out: *mut *mut c_void,
                ) -> c_int;
                #[link_name = "AHardwareBuffer_unlock"]
                fn unlock(b: *mut c_void, fence: *mut i32) -> c_int;
                /// Android 10, for YUV.
                #[optional]
                #[link_name = "AHardwareBuffer_lockPlanes"]
                fn lock_planes(
                    b: *mut c_void,
                    usage: u64,
                    fence: i32,
                    rect: *const c_void,
                    out: *mut Planes,
                ) -> c_int;
            }
        }
    }

    /// libnativewindow's `AHardwareBuffer_*`, where the system has them.
    pub fn lib() -> Option<&'static Lib> {
        static LIB: OnceLock<Option<Lib>> = OnceLock::new();
        LIB.get_or_init(|| {
            // SAFETY: a system library, trusted like the rest of the platform.
            unsafe { <Lib as dlsym::Bindings>::open("libnativewindow.so") }.ok()
        })
        .as_ref()
    }

    /// Lock the buffer for reading and convert it.
    pub(super) unsafe fn read(
        b: *mut c_void,
        format: ExternalFormat,
        size: PhysicalSize,
    ) -> Option<Pixmap> {
        let lib = lib()?;
        if format == ExternalFormat::Yuv420 {
            return unsafe { read_yuv(lib, b, size) };
        }
        unsafe {
            let mut d = Desc::default();
            lib.describe(b, &mut d);
            let mut ptr = std::ptr::null_mut();
            if lib.lock(b, USAGE_CPU_READ_OFTEN, -1, std::ptr::null(), &mut ptr) != 0
                || ptr.is_null()
            {
                return None;
            }
            let stride = d.stride as usize * 4;
            let bytes = std::slice::from_raw_parts(ptr as *const u8, stride * size.height as usize);
            let out = to_rgba(format, size, &[(bytes, stride)], YuvColorSpace::default());
            lib.unlock(b, std::ptr::null_mut());
            out
        }
    }

    /// Lock a YUV buffer's planes for reading and convert them (as the
    /// camera converts its frames: full range BT.601).
    unsafe fn read_yuv(lib: &Lib, b: *mut c_void, size: PhysicalSize) -> Option<Pixmap> {
        unsafe {
            let mut p = Planes {
                count: 0,
                planes: [Plane {
                    data: std::ptr::null_mut(),
                    pixel_stride: 0,
                    row_stride: 0,
                }; 4],
            };
            // `None` before Android 10.
            if lib.lock_planes(b, USAGE_CPU_READ_OFTEN, -1, std::ptr::null(), &mut p)? != 0 {
                return None;
            }
            let (w, h) = (size.width as usize, size.height as usize);
            let out =
                (p.count >= 3 && p.planes[..3].iter().all(|q| !q.data.is_null())).then(|| {
                    // Each plane's bytes, up to its last pixel.
                    let plane = |q: &Plane, cw: usize, ch: usize| {
                        let len = (ch - 1) * q.row_stride as usize
                            + (cw - 1) * q.pixel_stride as usize
                            + 1;
                        crate::camera::Plane::new(
                            std::slice::from_raw_parts(q.data as *const u8, len),
                            q.row_stride as usize,
                            q.pixel_stride.max(1) as usize,
                        )
                    };
                    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
                    let planes = [
                        plane(&p.planes[0], w, h),
                        plane(&p.planes[1], cw, ch),
                        plane(&p.planes[2], cw, ch),
                    ];
                    crate::camera::Frame::yuv(size, &planes).to_pixmap()
                });
            lib.unlock(b, std::ptr::null_mut());
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_map_to_drm_and_core_video_codes() {
        for f in [
            ExternalFormat::Rgba8,
            ExternalFormat::Bgra8,
            ExternalFormat::Rgbx8,
            ExternalFormat::Bgrx8,
            ExternalFormat::Nv12,
        ] {
            assert_eq!(ExternalFormat::from_drm_fourcc(f.drm_fourcc()), Some(f));
            assert_eq!(f.plane_fourccs().len(), f.planes());
        }
        assert_eq!(fourcc::ABGR8888, 0x3432_4241);
        assert_eq!(fourcc::NV12, 0x3231_564e);
        assert_eq!(fourcc::R8, 0x2020_3852);
        assert_eq!(
            ExternalFormat::from_core_video(u32::from_be_bytes(*b"420f")),
            Some(ExternalFormat::Nv12)
        );
        assert_eq!(
            ExternalFormat::from_core_video(u32::from_be_bytes(*b"BGRA")),
            Some(ExternalFormat::Bgra8)
        );
        assert_eq!(
            ExternalFormat::from_core_video(u32::from_be_bytes(*b"420v")),
            None
        );
        let s = PhysicalSize::new(5, 3);
        assert_eq!(ExternalFormat::Nv12.plane_extent(0, s), (5, 3, 1));
        assert_eq!(ExternalFormat::Nv12.plane_extent(1, s), (3, 2, 2));
        assert_eq!(ExternalFormat::Bgra8.plane_extent(0, s), (5, 3, 4));
    }

    #[test]
    fn egl_attributes_echo_the_modifier_unless_implicit() {
        let a = egl_dma_buf_attribs(4, 2, fourcc::ARGB8888, 7, 64, 16, DRM_FORMAT_MOD_INVALID);
        assert_eq!(
            a,
            vec![
                egl::WIDTH,
                4,
                egl::HEIGHT,
                2,
                egl::LINUX_DRM_FOURCC,
                fourcc::ARGB8888 as i32,
                egl::PLANE0_FD,
                7,
                egl::PLANE0_OFFSET,
                64,
                egl::PLANE0_PITCH,
                16,
                egl::NONE
            ]
        );
        let tiled = 0x0300_0000_0000_0014_u64; // an NVIDIA block-linear one
        let a = egl_dma_buf_attribs(4, 2, fourcc::ARGB8888, 7, 0, 16, tiled);
        assert_eq!(
            &a[12..],
            &[
                egl::PLANE0_MODIFIER_LO,
                0x14,
                egl::PLANE0_MODIFIER_HI,
                0x0300_0000,
                egl::NONE
            ]
        );
        assert!(modifier_is_linear(DRM_FORMAT_MOD_LINEAR));
        assert!(!modifier_is_linear(tiled));
    }

    #[test]
    fn nv12_is_imported_as_a_luma_and_a_chroma_plane() {
        let mut img = ExternalImage::from_pixels(Arc::new(Pixmap::new(PhysicalSize::new(6, 4))));
        img.format = ExternalFormat::Nv12;
        img.planes = vec![
            PlaneLayout {
                offset: 0,
                stride: 8,
            },
            PlaneLayout {
                offset: 32,
                stride: 8,
            },
        ];
        assert_eq!(
            plane_imports(&img),
            vec![(6, 4, fourcc::R8, 0, 8), (3, 2, fourcc::GR88, 32, 8)]
        );
    }

    #[test]
    fn cpu_conversion_swaps_bgr_drops_x_and_converts_yuv() {
        let s = PhysicalSize::new(2, 1);
        // Rows padded to 12 bytes.
        let bgra = [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0];
        let p = to_rgba(
            ExternalFormat::Bgra8,
            s,
            &[(&bgra, 12)],
            YuvColorSpace::default(),
        )
        .unwrap();
        assert_eq!(p.as_bytes(), &[3, 2, 1, 4, 7, 6, 5, 8]);
        let p = to_rgba(
            ExternalFormat::Rgbx8,
            s,
            &[(&bgra, 12)],
            YuvColorSpace::default(),
        )
        .unwrap();
        assert_eq!(p.as_bytes(), &[1, 2, 3, 255, 5, 6, 7, 255]);
        // NV12: gray luma, neutral chroma → gray; strong Cr → red.
        let y = [128, 200];
        let p = to_rgba(
            ExternalFormat::Nv12,
            s,
            &[(&y, 2), (&[128, 128], 2)],
            YuvColorSpace::default(),
        )
        .unwrap();
        assert_eq!(p.as_bytes(), &[128, 128, 128, 255, 200, 200, 200, 255]);
        let p = to_rgba(
            ExternalFormat::Nv12,
            s,
            &[(&[76, 76], 2), (&[85, 255], 2)],
            YuvColorSpace::default(),
        )
        .unwrap();
        let px = p.pixel(0, 0).unwrap();
        assert!(px[0] > 240 && px[1] < 20 && px[2] < 20, "{px:?}");
        // Limited range BT.709: video black and white.
        let lim = YuvColorSpace::Bt709Limited;
        let p = to_rgba(
            ExternalFormat::Nv12,
            s,
            &[(&[16, 235], 2), (&[128, 128], 2)],
            lim,
        );
        assert_eq!(p.unwrap().as_bytes(), &[0, 0, 0, 255, 255, 255, 255, 255]);
        // Too short a plane: nothing.
        assert!(
            to_rgba(
                ExternalFormat::Bgra8,
                s,
                &[(&bgra[..6], 12)],
                YuvColorSpace::default()
            )
            .is_none()
        );
    }

    #[test]
    fn the_release_callback_runs_once_when_the_last_reference_goes() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let n = Arc::new(AtomicU32::new(0));
        let c = n.clone();
        let img = Arc::new(
            ExternalImage::from_pixels(Arc::new(Pixmap::new(PhysicalSize::new(1, 1)))).on_release(
                move || {
                    c.fetch_add(1, Ordering::SeqCst);
                },
            ),
        );
        let other = img.clone();
        drop(img);
        assert_eq!(n.load(Ordering::SeqCst), 0);
        drop(other);
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn producers_hear_what_is_imported_natively() {
        let _lock = NATIVE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        note_native(&[ExternalFormat::Bgra8, ExternalFormat::Nv12]);
        assert!(imported_natively(ExternalFormat::Nv12));
        assert!(!imported_natively(ExternalFormat::Rgba8));
        note_native(&[]);
        assert!(!imported_natively(ExternalFormat::Nv12));
    }

    /// A real `dma-buf` (from `udmabuf`), mapped and read back.
    #[cfg(any(target_os = "linux", target_os = "fullrust"))]
    #[test]
    fn a_linear_dma_buf_is_read_on_the_cpu() {
        // NV12, 4×2: luma, then chroma at offset 8.
        let mut bytes = vec![0u8; 12];
        bytes[..8].copy_from_slice(&[128; 8]);
        bytes[8..].copy_from_slice(&[128, 128, 128, 128]);
        let fd = match udmabuf(&bytes) {
            Ok(fd) => fd,
            Err(e) => {
                eprintln!("skipped: no udmabuf ({e})");
                return;
            }
        };
        let fd2 = fd.try_clone().unwrap();
        let img = ExternalImage::dma_buf(
            PhysicalSize::new(4, 2),
            ExternalFormat::Nv12,
            DRM_FORMAT_MOD_LINEAR,
            vec![
                DmaBufPlane {
                    fd,
                    offset: 0,
                    stride: 4,
                },
                DmaBufPlane {
                    fd: fd2,
                    offset: 8,
                    stride: 4,
                },
            ],
        )
        .unwrap();
        let p = img.pixmap().expect("mapped");
        assert_eq!(p.pixel(3, 1), Some([128, 128, 128, 255]));
    }
}
