//! Experimental GPU present path: route a software [`Pixmap`] through OpenGL
//! ES 2 (raw EGL + GLES FFI — no `ash`/`glutin`/`khronos` crate; linking the
//! OS's GL driver fits the "close to the OS" policy in `ROADMAP.md` §1).
//!
//! Two paths share the seam: [`present_offscreen`] composites a finished CPU
//! [`Pixmap`] on the GPU (a pass-through shader), while [`render_scene`] renders
//! a live stipple [`Scene`] **GPU-natively**: boxes, paths (stencil-and-cover),
//! images and glyphs (textures cached by identity), clips, and layers with
//! transforms and group opacity, matching the software renderer to within a
//! small tolerance (see [`render_scene_scaled`] and `docs/gpu.md`). Both read
//! back to a `Pixmap`, the GPU half of the [`Surface`](crate::render::Surface)
//! seam. The scene lowering (`ops`) is backend-neutral; executing it on
//! Vulkan/Metal/D3D/WebGPU is future work. The EGL
//! context is **surfaceless**, so it runs headlessly under Mesa's software GL in
//! CI.
//!
//! Linux-only for now (EGL/GLESv2); other targets return an error so the
//! workspace still builds everywhere.

#![allow(unsafe_code)] // raw GPU FFI

use crate::geometry::{PhysicalSize, Rect, ScaleFactor};
use crate::render::{Color, Font, Pixmap, Scene};

// The scene lowering is backend-neutral; only the GL backend executes it yet.
#[cfg_attr(not(all(target_os = "linux", feature = "gl")), allow(dead_code))]
mod ops;

#[cfg(all(target_os = "linux", feature = "gl"))]
mod gl;

#[cfg(all(target_os = "linux", feature = "vk"))]
mod vulkan;

#[cfg(all(target_os = "windows", feature = "d3d"))]
mod d3d;

#[cfg(all(target_os = "macos", feature = "mtl"))]
mod metal;

/// Enumerate the Vulkan physical devices reachable through `libvulkan` (raw FFI,
/// no helper crate) — the foundation for a future GPU-native Vulkan backend.
/// Returns each device's name (e.g. `"llvmpipe (...)"` under Mesa lavapipe).
/// Errors if the `vk` feature is off or no Vulkan loader/ICD is available.
pub fn vulkan_devices() -> Result<Vec<String>, String> {
    #[cfg(all(target_os = "linux", feature = "vk"))]
    {
        vulkan::devices()
    }
    #[cfg(not(all(target_os = "linux", feature = "vk")))]
    {
        Err("stipple::gpu: built without the `vk` feature (Linux-only Vulkan FFI)".to_string())
    }
}

/// Create a Vulkan logical device with a graphics queue on the first physical
/// device (raw FFI) — the core a Vulkan render backend builds on. Returns a
/// one-line summary. Errors if the `vk` feature is off or device creation fails.
pub fn vulkan_init_device() -> Result<String, String> {
    #[cfg(all(target_os = "linux", feature = "vk"))]
    {
        vulkan::init_device()
    }
    #[cfg(not(all(target_os = "linux", feature = "vk")))]
    {
        Err("stipple::gpu: built without the `vk` feature (Linux-only Vulkan FFI)".to_string())
    }
}

/// Allocate a `width`×`height` `R8G8B8A8` color-attachment Vulkan image backed by
/// `DEVICE_LOCAL` device memory (raw FFI) — the offscreen render target a Vulkan
/// render pass draws into. Returns a one-line summary. Errors if the `vk` feature
/// is off or allocation fails.
pub fn vulkan_init_image(width: u32, height: u32) -> Result<String, String> {
    #[cfg(all(target_os = "linux", feature = "vk"))]
    {
        vulkan::init_image(width, height)
    }
    #[cfg(not(all(target_os = "linux", feature = "vk")))]
    {
        let _ = (width, height);
        Err("stipple::gpu: built without the `vk` feature (Linux-only Vulkan FFI)".to_string())
    }
}

/// Build the Vulkan render-target objects for a `width`×`height` offscreen frame
/// (raw FFI): an image view over a color image, a single-attachment render pass
/// (clear→store, ending transfer-readable), and a framebuffer binding them — what
/// a render pass draws into. Returns a one-line summary. Errors if the `vk`
/// feature is off or creation fails.
pub fn vulkan_init_framebuffer(width: u32, height: u32) -> Result<String, String> {
    #[cfg(all(target_os = "linux", feature = "vk"))]
    {
        vulkan::init_framebuffer(width, height)
    }
    #[cfg(not(all(target_os = "linux", feature = "vk")))]
    {
        let _ = (width, height);
        Err("stipple::gpu: built without the `vk` feature (Linux-only Vulkan FFI)".to_string())
    }
}

/// Execute a real Vulkan command-buffer round-trip (raw FFI): record a primary
/// command buffer that runs a render pass clearing a `width`×`height` color
/// image, submit it to the graphics queue, and block on a fence until the GPU
/// finishes — the first GPU execution the draw pipeline will extend. Returns a
/// one-line summary. Errors if the `vk` feature is off or any step fails.
pub fn vulkan_clear(width: u32, height: u32) -> Result<String, String> {
    #[cfg(all(target_os = "linux", feature = "vk"))]
    {
        vulkan::init_clear(width, height)
    }
    #[cfg(not(all(target_os = "linux", feature = "vk")))]
    {
        let _ = (width, height);
        Err("stipple::gpu: built without the `vk` feature (Linux-only Vulkan FFI)".to_string())
    }
}

/// Render a `width`×`height` frame entirely through Vulkan (raw FFI) and read it
/// back to the CPU: run a clearing render pass on the GPU, copy the result image
/// into a host-visible buffer, fence-wait, and return the RGBA pixels — an actual
/// GPU-rendered frame, the foundation for a Vulkan `Surface`. The draw pipeline
/// will replace the clear with real draw calls; this readback path is reused.
/// Errors if the `vk` feature is off or any step fails.
pub fn vulkan_render_clear(width: u32, height: u32) -> Result<Vec<u8>, String> {
    #[cfg(all(target_os = "linux", feature = "vk"))]
    {
        vulkan::render_clear(width, height)
    }
    #[cfg(not(all(target_os = "linux", feature = "vk")))]
    {
        let _ = (width, height);
        Err("stipple::gpu: built without the `vk` feature (Linux-only Vulkan FFI)".to_string())
    }
}

/// Draw a triangle through a full Vulkan graphics pipeline (raw FFI): two SPIR-V
/// shader modules, a graphics pipeline, and a `vkCmdDraw` over a dark-cleared
/// `width`×`height` target, then read the frame back to the CPU. The center
/// pixel comes out stipple green — proof that real shaders ran on the GPU. Returns
/// the RGBA pixels. Errors if the `vk` feature is off or any step fails.
pub fn vulkan_render_triangle(width: u32, height: u32) -> Result<Vec<u8>, String> {
    #[cfg(all(target_os = "linux", feature = "vk"))]
    {
        vulkan::render_triangle(width, height)
    }
    #[cfg(not(all(target_os = "linux", feature = "vk")))]
    {
        let _ = (width, height);
        Err("stipple::gpu: built without the `vk` feature (Linux-only Vulkan FFI)".to_string())
    }
}

/// Create a Direct3D 11 device on **WARP** (the software rasterizer shipped with
/// Windows, raw FFI — no `windows` crate) and return its feature level — the
/// foundation for a GPU-native D3D backend, the Windows analog of
/// [`vulkan_init_device`]. Errors if the `d3d` feature is off or the device
/// can't be created.
pub fn d3d11_device() -> Result<String, String> {
    #[cfg(all(target_os = "windows", feature = "d3d"))]
    {
        d3d::device()
    }
    #[cfg(not(all(target_os = "windows", feature = "d3d")))]
    {
        Err("stipple::gpu: built without the `d3d` feature (Windows-only Direct3D FFI)".to_string())
    }
}

/// Create a shareable Direct3D 11 texture on WARP and return its cross-process
/// **shared handle** — the Windows analog of exporting a `dma-buf` for the
/// compositor's content path (the content process shares the texture; the UI
/// process re-opens the handle with `OpenSharedResource`). Errors if the `d3d`
/// feature is off or the shared texture/handle can't be created.
pub fn d3d11_export_shared_handle(width: u32, height: u32) -> Result<String, String> {
    #[cfg(all(target_os = "windows", feature = "d3d"))]
    {
        d3d::export_shared_handle(width, height)
    }
    #[cfg(not(all(target_os = "windows", feature = "d3d")))]
    {
        let _ = (width, height);
        Err("stipple::gpu: built without the `d3d` feature (Windows-only Direct3D FFI)".to_string())
    }
}

/// Render a `width`×`height` frame through Direct3D 11 on WARP (raw FFI) and read
/// it back: clear a render-target texture to stipple blue, copy it into a staging
/// texture, map it, and return the RGBA pixels — an actual GPU-rendered frame,
/// the D3D analog of [`vulkan_render_clear`]. Errors if the `d3d` feature is off
/// or any step fails.
pub fn d3d11_render_clear(width: u32, height: u32) -> Result<Vec<u8>, String> {
    #[cfg(all(target_os = "windows", feature = "d3d"))]
    {
        d3d::render_clear(width, height)
    }
    #[cfg(not(all(target_os = "windows", feature = "d3d")))]
    {
        let _ = (width, height);
        Err("stipple::gpu: built without the `d3d` feature (Windows-only Direct3D FFI)".to_string())
    }
}

/// Draw a triangle through a full Direct3D 11 pipeline on WARP (raw FFI): compile
/// HLSL vertex+pixel shaders with `D3DCompile`, bind them, and `Draw` over a
/// dark-cleared `width`×`height` target, then read the frame back. The center
/// pixel comes out stipple green — proof real shaders ran, the D3D analog of
/// [`vulkan_render_triangle`]. Returns the RGBA pixels. Errors if the `d3d`
/// feature is off or any step fails.
pub fn d3d11_render_triangle(width: u32, height: u32) -> Result<Vec<u8>, String> {
    #[cfg(all(target_os = "windows", feature = "d3d"))]
    {
        d3d::render_triangle(width, height)
    }
    #[cfg(not(all(target_os = "windows", feature = "d3d")))]
    {
        let _ = (width, height);
        Err("stipple::gpu: built without the `d3d` feature (Windows-only Direct3D FFI)".to_string())
    }
}

/// Create the system default Metal device (raw `objc_msgSend` FFI — no `metal`
/// crate) and return its name — the foundation for a GPU-native Metal backend,
/// the macOS analog of [`vulkan_init_device`]. Errors if the `mtl` feature is off
/// or no Metal device is available.
pub fn metal_device() -> Result<String, String> {
    #[cfg(all(target_os = "macos", feature = "mtl"))]
    {
        metal::device()
    }
    #[cfg(not(all(target_os = "macos", feature = "mtl")))]
    {
        Err("stipple::gpu: built without the `mtl` feature (macOS-only Metal FFI)".to_string())
    }
}

/// Create a shareable BGRA8 **IOSurface** and return its global `IOSurfaceID` —
/// the macOS analog of exporting a `dma-buf` for the compositor's content path
/// (the content process creates the surface; the UI process re-opens the id with
/// `IOSurfaceLookup` and binds it as an `MTLTexture`). Errors if the `mtl`
/// feature is off or the surface can't be created.
pub fn metal_export_iosurface(width: u32, height: u32) -> Result<String, String> {
    #[cfg(all(target_os = "macos", feature = "mtl"))]
    {
        metal::export_iosurface(width, height)
    }
    #[cfg(not(all(target_os = "macos", feature = "mtl")))]
    {
        let _ = (width, height);
        Err("stipple::gpu: built without the `mtl` feature (macOS-only Metal FFI)".to_string())
    }
}

/// Render a `width`×`height` frame through Metal (raw FFI) and read it back: a
/// `MTLTexture` cleared to stipple blue by a render command encoder, then read to
/// the CPU with `getBytes` — an actual GPU-rendered frame, the Metal analog of
/// [`vulkan_render_clear`]. Returns the RGBA pixels. Errors if the `mtl` feature
/// is off or any step fails.
pub fn metal_render_clear(width: u32, height: u32) -> Result<Vec<u8>, String> {
    #[cfg(all(target_os = "macos", feature = "mtl"))]
    {
        metal::render_clear(width, height)
    }
    #[cfg(not(all(target_os = "macos", feature = "mtl")))]
    {
        let _ = (width, height);
        Err("stipple::gpu: built without the `mtl` feature (macOS-only Metal FFI)".to_string())
    }
}

/// Draw a triangle through a full Metal render pipeline (raw FFI): compile a
/// `.metal` source into a library, build a render pipeline, and `drawPrimitives`
/// over a dark-cleared `width`×`height` target, then read the frame back. The
/// center pixel comes out stipple green — proof real shaders ran on the GPU, the
/// Metal analog of [`vulkan_render_triangle`]. Returns the RGBA pixels. Errors if
/// the `mtl` feature is off or any step fails.
pub fn metal_render_triangle(width: u32, height: u32) -> Result<Vec<u8>, String> {
    #[cfg(all(target_os = "macos", feature = "mtl"))]
    {
        metal::render_triangle(width, height)
    }
    #[cfg(not(all(target_os = "macos", feature = "mtl")))]
    {
        let _ = (width, height);
        Err("stipple::gpu: built without the `mtl` feature (macOS-only Metal FFI)".to_string())
    }
}

/// Round-trip `input` through the GPU (upload → draw → read back), returning the
/// rendered frame. Errors if the `gl` feature is off or no GL/EGL device is
/// available.
pub fn present_offscreen(input: &Pixmap) -> Result<Pixmap, String> {
    #[cfg(all(target_os = "linux", feature = "gl"))]
    {
        gl::present_offscreen(input)
    }
    #[cfg(not(all(target_os = "linux", feature = "gl")))]
    {
        let _ = input;
        Err("stipple::gpu: built without the `gl` feature (Linux-only GLES backend)".to_string())
    }
}

/// The device's EGL extension string — used to check for the dma-buf
/// import/export extensions before relying on them
/// (`EGL_EXT_image_dma_buf_import`, `EGL_MESA_image_dma_buf_export`). Errors if
/// the `gl` feature is off or EGL can't initialize.
pub fn dmabuf_extensions() -> Result<String, String> {
    #[cfg(all(target_os = "linux", feature = "gl"))]
    {
        gl::dmabuf_extensions()
    }
    #[cfg(not(all(target_os = "linux", feature = "gl")))]
    {
        Err("stipple::gpu: built without the `gl` feature (Linux-only GLES backend)".to_string())
    }
}

/// Prove the cross-process surface seam end to end in one process: render a
/// known pattern to a GPU texture, **export** it as a `dma-buf` (the handle a
/// browser content process would send the UI process), **re-import** that
/// `dma-buf` as a texture, and confirm the pixels survived. Returns the imported
/// pixels (top-first RGBA). Runs surfaceless (no window). Errors if the `gl`
/// feature is off, EGL can't initialize, or the dma-buf extensions are absent.
pub fn dmabuf_export_import_self_test() -> Result<Vec<u8>, String> {
    #[cfg(all(target_os = "linux", feature = "gl"))]
    {
        gl::dmabuf_export_import_self_test()
    }
    #[cfg(not(all(target_os = "linux", feature = "gl")))]
    {
        Err("stipple::gpu: built without the `gl` feature (Linux-only GLES backend)".to_string())
    }
}

/// Like [`dmabuf_export_import_self_test`] but bound to the GPU named by `drm_fd`
/// (a DRM device fd — e.g. from X11 `DRI3Open`, or an opened render node) via a
/// GBM device, so the rendered/exported buffers live on that exact GPU. This is
/// the device-binding the DRI3 + Present compositor path needs. Errors if the
/// `gl` feature is off or the device can't be used.
pub fn dmabuf_self_test_on_device(drm_fd: i32) -> Result<Vec<u8>, String> {
    #[cfg(all(target_os = "linux", feature = "gl"))]
    {
        gl::dmabuf_self_test_on_device(drm_fd)
    }
    #[cfg(not(all(target_os = "linux", feature = "gl")))]
    {
        let _ = drm_fd;
        Err("stipple::gpu: built without the `gl` feature (Linux-only GLES backend)".to_string())
    }
}

/// A descriptor of an exported dma-buf: the GPU buffer handle plus the geometry
/// and format an importer needs to interpret it. The fields map directly onto
/// what the X server's DRI3 `PixmapFromBuffers` needs to wrap the buffer as a
/// Pixmap (and what a browser content process would hand the UI process over a
/// socket): `(stride, offset)` is the single plane, and `modifier` must be
/// echoed on import or a tiled buffer decodes as garbage. The caller owns `fd`
/// and must close it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmabufExport {
    /// The dma-buf file descriptor (owned by the caller).
    pub fd: i32,
    pub width: u32,
    pub height: u32,
    /// Row stride in bytes.
    pub stride: u32,
    /// Byte offset of the plane within the buffer.
    pub offset: u32,
    /// DRM format modifier (tiling/layout) — echo it on import.
    pub modifier: u64,
    /// DRM fourcc of the pixel format (e.g. `AB24` = ABGR8888).
    pub fourcc: u32,
    /// Bits per pixel.
    pub bpp: u8,
}

/// Render a `width`×`height` frame on the GPU named by `drm_fd` (via GBM) and
/// **export it as a single-plane dma-buf**, returning its [`DmabufExport`]
/// descriptor — exactly the data needed to hand the buffer to the X server (DRI3
/// `PixmapFromBuffers` → Present `PresentPixmap`, no readback) or to a content
/// process. The fd is owned by the caller. Bind `drm_fd` to the server's GPU
/// (from X11 `DRI3Open`) so the server can import the result. Errors if the `gl`
/// feature is off, the device can't be used, or export is unsupported there.
pub fn export_dmabuf_on_device(
    drm_fd: i32,
    width: u32,
    height: u32,
) -> Result<DmabufExport, String> {
    #[cfg(all(target_os = "linux", feature = "gl"))]
    {
        gl::export_dmabuf_on_device(drm_fd, width, height)
    }
    #[cfg(not(all(target_os = "linux", feature = "gl")))]
    {
        let _ = (drm_fd, width, height);
        Err("stipple::gpu: built without the `gl` feature (Linux-only GLES backend)".to_string())
    }
}

/// Draw solid-color `rects` **GPU-natively** (as tessellated geometry through a
/// flat-color shader, not by compositing a CPU frame) on a `background`-cleared
/// target of `size`, returning the rendered frame — the first step toward a
/// GPU-native scene renderer. Each entry is
/// `(rect, color, corner_radius, border_width)`; the shader evaluates a
/// rounded-rectangle SDF (radius `0.0` = sharp), and a positive `border_width`
/// strokes the outline instead of filling. Errors if the `gl` feature is off or
/// no GL/EGL device is available.
pub fn fill_rects_offscreen(
    size: PhysicalSize,
    background: Color,
    rects: &[(Rect, Color, f32, f32)],
) -> Result<Pixmap, String> {
    #[cfg(all(target_os = "linux", feature = "gl"))]
    {
        gl::fill_rects_offscreen(size, background, rects)
    }
    #[cfg(not(all(target_os = "linux", feature = "gl")))]
    {
        let _ = (size, background, rects);
        Err("stipple::gpu: built without the `gl` feature (Linux-only GLES backend)".to_string())
    }
}

/// Render a scene **GPU-natively** — box primitives (rounded-rect SDF) plus
/// text composited from glyph-coverage masks — to an offscreen target of `size`
/// cleared to `background`, returning the rendered frame. Each text entry is
/// `(mask, dst, color)`: a white-on-black coverage raster of the glyphs, the
/// destination rectangle, and the ink color. Errors if the `gl` feature is off
/// or no GL/EGL device is available.
pub fn render_offscreen(
    size: PhysicalSize,
    background: Color,
    rects: &[(Rect, Color, f32, f32)],
    texts: &[(&Pixmap, Rect, Color)],
) -> Result<Pixmap, String> {
    #[cfg(all(target_os = "linux", feature = "gl"))]
    {
        gl::render_offscreen(size, background, rects, texts)
    }
    #[cfg(not(all(target_os = "linux", feature = "gl")))]
    {
        let _ = (size, background, rects, texts);
        Err("stipple::gpu: built without the `gl` feature (Linux-only GLES backend)".to_string())
    }
}

/// Render a live stipple [`Scene`] **GPU-natively** at `scale` (physical
/// pixels per logical pixel) on a target cleared to `background`, returning
/// the rendered frame. This is what a renderer set with
/// [`App::render_with`](crate::App::render_with) calls.
///
/// Everything a scene holds is drawn on the GPU (see `docs/gpu.md`): boxes
/// through an analytic-coverage shader, paths (fills, strokes, dashes) by
/// stencil-and-cover with 16x supersampled coverage, bitmaps (OS-drawn
/// controls under the native theme, decoded images, SVG rasters) as textured
/// quads whose textures are cached by the image's identity, text from its own
/// glyph outlines (each glyph rasterized once on the CPU and kept), canvases,
/// clips, and layers with transforms and group opacity (offscreen targets,
/// composited once). Only gradients, soft masks and concave clips are
/// rasterized on the CPU, and only for their own bounds. The output stays
/// within a small tolerance of [`SoftwareRenderer`](crate::render::SoftwareRenderer).
///
/// The GL context and caches are kept per thread, from frame to frame.
/// Errors if the `gl` feature is off or no GL/EGL device is available.
pub fn render_scene_scaled(
    scene: &Scene,
    background: Color,
    scale: ScaleFactor,
) -> Result<Pixmap, String> {
    #[cfg(all(target_os = "linux", feature = "gl"))]
    {
        gl::render_scene_native(scene, background, scale)
    }
    #[cfg(not(all(target_os = "linux", feature = "gl")))]
    {
        let _ = (scene, background, scale);
        Err("stipple::gpu: built without the `gl` feature (Linux-only GLES backend)".to_string())
    }
}

/// [`render_scene_scaled`] at scale 1. `font` is no longer used: text is
/// drawn from the scene's own glyphs, whatever font set them.
pub fn render_scene(scene: &Scene, background: Color, font: &Font) -> Result<Pixmap, String> {
    let _ = font;
    render_scene_scaled(scene, background, ScaleFactor::IDENTITY)
}

#[cfg(all(test, target_os = "linux", feature = "gl"))]
mod compare;
