//! The Direct3D 11 scene executor: draws a lowered [`Plan`] with D3D11,
//! offscreen (and read back) or into a window's flip-model DXGI swap chain
//! (and presented).
//!
//! It is the Metal executor (`metal_scene.rs`) translated: the same shaders
//! in HLSL (`shaders/scene.hlsl`), compiled with `D3DCompile` when the
//! executor is created, each batch of boxes and textured quads
//! (`crate::gpu::batch`) one draw, the same premultiplied blending,
//! stencil-and-cover
//! paths at 4×4 supersampling into a scratch target, layers in pooled
//! offscreen targets, scissor and stencil clips. Every target is a
//! `B8G8R8A8_UNORM` color texture with a `D24_UNORM_S8_UINT` one beside it
//! (D3D11 has no stencil-only format; the depth half is unused).
//!
//! Colors are blended on the encoded sRGB values, as the software renderer
//! blends them; the swap chain is `UNORM`, which the desktop compositor takes
//! as sRGB, as it does the CPU frames.
//!
//! The device is a hardware one when there is one, WARP otherwise. All
//! drawing goes through the immediate context: vertices into a dynamic
//! vertex buffer used as a ring (`WRITE_NO_OVERWRITE`, `WRITE_DISCARD` when it
//! wraps), each draw's constants into one dynamic constant buffer
//! (`WRITE_DISCARD`), images into immutable textures cached by the image's
//! identity, like the other executors', and each glyph atlas page into a
//! texture brought up to date (`UpdateSubresource`, only what changed) at
//! the start of the frame.
//!
//! Raw COM through vtable slots (`abi.rs`), as in the rest of the Windows
//! code; no `windows` crate.

use super::{D3D11CreateDevice, blob_bytes, compile_source, release, vmethod};
use crate::geometry::{PhysicalSize, ScaleFactor};
use crate::gpu::atlas::{AtlasPage, Upload};
use crate::gpu::batch::{self, Cmd, QUAD_FLOATS, TexRef};
use crate::gpu::canvas::rekey;
use crate::gpu::d3d_abi::*;
use crate::gpu::ops::{Clip, Op, Plan, Resample};
use crate::gpu::renderer::{Api, DamageLog, Executor, GpuError};
use crate::platform::GpuLayer;
use crate::render::Pixmap;
use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

use crate::platform::backend::windows::vsync;

#[link(name = "user32")]
unsafe extern "system" {
    fn IsWindow(hwnd: *mut c_void) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CloseHandle(handle: *mut c_void) -> i32;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    /// Whether the process is exiting (`ExitProcess` has begun detaching
    /// the DLLs). Returns a `BOOLEAN`.
    fn RtlDllShutdownInProgress() -> u8;
}

/// Whether the process is exiting, so nothing may be released.
///
/// The renderer lives in a thread-local. The main thread's thread-locals are
/// destroyed from the TLS callback for `DLL_PROCESS_DETACH`, after
/// `ExitProcess` has killed every other thread, the D3D runtime's and
/// WARP's workers among them, and while the system DLLs are detaching.
/// Releasing the device then (and the `ClearState` + `Flush` before it)
/// calls into a runtime whose threads are gone: it crashed the process at
/// exit on CI (exit code 127 once `main` had returned). So at process exit
/// every COM reference and handle is left to the OS, which frees all of it
/// with the process anyway. Threads that end normally, and renderers dropped
/// while the app runs (a lost device), release as usual.
fn exiting() -> bool {
    unsafe { RtlDllShutdownInProgress() != 0 }
}

/// Call COM method `slot` of `obj`, typed as
/// `com!(obj, SLOT, (ArgTypes...) -> Ret, args...)`.
macro_rules! com {
    ($obj:expr, $slot:expr, ($($ty:ty),*) -> $ret:ty $(, $arg:expr)* $(,)?) => {{
        let obj: *mut c_void = $obj;
        let f: unsafe extern "system" fn(*mut c_void $(, $ty)*) -> $ret =
            core::mem::transmute(vmethod(obj, $slot));
        f(obj $(, $arg)*)
    }};
}

// DXGI shared handles opened as textures; after `com!`, which it uses.
mod external;

/// Vertex buffers are allocated at least this big.
const ARENA: usize = 1024 * 1024;
/// Textures of images not drawn for this many frames are freed.
const TEXTURE_KEEP_FRAMES: u64 = 120;
/// How long to wait for a swap chain to take a new frame before skipping it.
const FRAME_WAIT_MS: u32 = 100;
/// The shaders.
const HLSL: &str = include_str!("../shaders/scene.hlsl");

/// An owned COM reference, released when dropped.
struct Com(*mut c_void);

impl Com {
    fn null() -> Self {
        Com(std::ptr::null_mut())
    }
    fn ptr(&self) -> *mut c_void {
        self.0
    }
    fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() && !exiting() {
            unsafe { release(self.0) };
        }
    }
}

/// A failed call as an error. Whether the device was removed or a call
/// failed mid-frame, the frame is unfinished: the renderer is set up again
/// (once), and the frame drawn on the CPU.
fn failed(hr: Hresult, what: &str) -> GpuError {
    let lost = if outcome(hr) == Outcome::Lost {
        ", device lost"
    } else {
        ""
    };
    GpuError::Lost(format!("{what} failed (0x{:08x}{lost})", hr as u32))
}

fn check(hr: Hresult, what: &str) -> Result<(), GpuError> {
    if hr < 0 {
        Err(failed(hr, what))
    } else {
        Ok(())
    }
}

/// A depth-stencil buffer and its view.
struct Stencil {
    _tex: Com,
    dsv: Com,
    w: i32,
    h: i32,
}

/// A color texture with a stencil buffer beside it, and their views.
struct Target {
    color: Com,
    rtv: Com,
    srv: Com,
    stencil: Stencil,
    w: i32,
    h: i32,
}

/// A cached image texture.
struct Texture {
    image: Weak<Pixmap>,
    tex: Com,
    srv: Com,
    used: u64,
    bytes: usize,
}

/// An atlas page's texture, holding `version` of it.
struct AtlasTex {
    tex: Com,
    srv: Com,
    side: u32,
    version: u64,
}

/// A window's swap chain.
struct Chain {
    swap: Com,
    /// The frame-latency waitable object, when the swap chain has one.
    waitable: *mut c_void,
    effect: u32,
    flags: u32,
    w: i32,
    h: i32,
    stencil: Option<Stencil>,
    /// Frames presented since it was made or resized (see
    /// [`back_buffer_age`]), and what they changed.
    presents: u64,
    log: DamageLog,
}

impl Drop for Chain {
    fn drop(&mut self) {
        if !self.waitable.is_null() && !exiting() {
            vsync::stop_pacing(self.waitable);
            unsafe { CloseHandle(self.waitable) };
        }
    }
}

/// A clip pushed on a target, and how to undo it.
enum Undo {
    Scissor([i32; 4]),
    Stencil(Vec<[f32; 2]>),
}

/// A render target in use: the frame or a layer.
struct Level {
    /// Index into the layer pool, or `None` for the frame.
    layer: Option<usize>,
    opacity: f32,
    scissor: [i32; 4],
    depth: u32,
    undo: Vec<Undo>,
    /// A resampled layer's composite ([`Op::PushResampled`]).
    resample: Option<Resample>,
}

impl Level {
    fn on(&self) -> On {
        self.layer.map_or(On::Frame, On::Layer)
    }
}

/// What the context draws into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum On {
    Frame,
    Layer(usize),
    Scratch,
}

/// One frame being drawn: its target, and which target is bound.
struct Frame {
    rtv: *mut c_void,
    dsv: *mut c_void,
    w: i32,
    h: i32,
    on: Option<On>,
}

/// A pixel shader and the blend state it draws with.
#[derive(Clone, Copy)]
struct Prog {
    ps: *mut c_void,
    blend: *mut c_void,
}

/// The Direct3D 11 executor: device, shaders, states, targets and caches.
pub(crate) struct D3dExecutor {
    device: Com,
    ctx: Com,
    /// The largest texture side the device takes.
    max: i32,
    /// The adapter's name (`Microsoft Basic Render Driver` for WARP).
    device_name: String,
    vs: Com,
    layout: Com,
    /// The batched quads' vertex shader and input layout, and their pixel
    /// shaders: boxes only, and boxes with textured quads.
    quad_vs: Com,
    quad_layout: Com,
    ps_quadbox: Com,
    ps_quad: Com,
    /// Textured quads sampling NV12 (an external image's two planes).
    ps_quad_yuv: Com,
    /// Whether the quads' vertex shader and layout are bound (else the
    /// paths').
    quad_bound: bool,
    ps_flat: Com,
    ps_resolve: Com,
    /// Premultiplied source-over; plain writes; no color writes.
    blend_premul: Com,
    blend_opaque: Com,
    blend_none: Com,
    raster: Com,
    bilinear: Com,
    nearest: Com,
    ds_test: Com,
    ds_clip_inc: Com,
    ds_clip_dec: Com,
    ds_nonzero: Com,
    ds_evenodd: Com,
    ds_cover_nz: Com,
    ds_cover_eo: Com,
    constants: Com,
    vbuf: Com,
    vcap: usize,
    vused: usize,
    /// The offscreen frame (read-back), its staging copy, layers, the path
    /// scratch.
    frame: Option<Target>,
    staging: Option<(Com, i32, i32)>,
    layers: Vec<Target>,
    scratch: Option<Target>,
    /// Swap chains, by window handle.
    chains: HashMap<usize, Chain>,
    textures: HashMap<usize, Texture>,
    /// External images opened as textures (`external`).
    externals: external::Imports,
    /// The atlas pages' textures, by page id.
    atlas: HashMap<u64, AtlasTex>,
    /// The layer composites' quad vertices.
    scratch_quads: Vec<f32>,
    count: u64,
    uploads: usize,
    draws: usize,
}

impl Drop for D3dExecutor {
    fn drop(&mut self) {
        // Unbind everything and let the deferred releases happen, so the
        // windows' swap chains are really gone (a window can then get a new
        // one, or go back to GDI). Not at process exit: see `exiting`.
        if !self.ctx.is_null() && !exiting() {
            unsafe {
                com!(self.ctx.ptr(), VT_CTX_CLEAR_STATE, () -> ());
                com!(self.ctx.ptr(), VT_CTX_FLUSH, () -> ());
            }
        }
        self.chains.clear();
    }
}

/// The name of the adapter `device` is on, or an empty string.
unsafe fn adapter_name(device: *mut c_void) -> String {
    unsafe {
        let mut dxgi = std::ptr::null_mut();
        let hr = com!(device, VT_QUERY_INTERFACE,
            (*const Guid, *mut *mut c_void) -> Hresult, &IID_IDXGI_DEVICE, &mut dxgi);
        if hr < 0 || dxgi.is_null() {
            return String::new();
        }
        let dxgi = Com(dxgi);
        let mut adapter = std::ptr::null_mut();
        let hr = com!(dxgi.ptr(), VT_DXGI_DEVICE_GET_ADAPTER,
            (*mut *mut c_void) -> Hresult, &mut adapter);
        if hr < 0 || adapter.is_null() {
            return String::new();
        }
        let adapter = Com(adapter);
        let mut desc = std::mem::zeroed::<AdapterDesc>();
        let hr = com!(adapter.ptr(), VT_DXGI_ADAPTER_GET_DESC,
            (*mut AdapterDesc) -> Hresult, &mut desc);
        if hr < 0 { String::new() } else { desc.name() }
    }
}

/// Make a D3D11 device: a hardware one, or WARP.
unsafe fn create_device() -> Result<(Com, Com, u32), GpuError> {
    let levels = [
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    let mut last = 0;
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let mut device = std::ptr::null_mut();
        let mut ctx = std::ptr::null_mut();
        let mut level = 0u32;
        let hr = unsafe {
            D3D11CreateDevice(
                std::ptr::null_mut(),
                driver,
                std::ptr::null_mut(),
                0,
                levels.as_ptr(),
                levels.len() as u32,
                D3D11_SDK_VERSION,
                &mut device,
                &mut level,
                &mut ctx,
            )
        };
        if hr >= 0 && !device.is_null() && !ctx.is_null() {
            return Ok((Com(device), Com(ctx), level));
        }
        unsafe {
            release(ctx);
            release(device);
        }
        last = hr;
    }
    Err(GpuError::Unavailable(format!(
        "no Direct3D 11 device (0x{:08x})",
        last as u32
    )))
}

impl D3dExecutor {
    /// Set up on a hardware device (or WARP), compiling the shaders.
    pub(crate) fn new() -> Result<Self, GpuError> {
        unsafe { Self::create() }
    }

    unsafe fn create() -> Result<Self, GpuError> {
        unsafe {
            let (device, ctx, level) = create_device()?;
            let max = if level >= D3D_FEATURE_LEVEL_11_0 {
                D3D11_MAX_TEXTURE_SIZE_11
            } else {
                D3D10_MAX_TEXTURE_SIZE
            };
            let device_name = adapter_name(device.ptr());
            let mut me = D3dExecutor {
                device,
                ctx,
                max,
                device_name,
                vs: Com::null(),
                layout: Com::null(),
                quad_vs: Com::null(),
                quad_layout: Com::null(),
                ps_quadbox: Com::null(),
                ps_quad: Com::null(),
                ps_quad_yuv: Com::null(),
                quad_bound: false,
                ps_flat: Com::null(),
                ps_resolve: Com::null(),
                blend_premul: Com::null(),
                blend_opaque: Com::null(),
                blend_none: Com::null(),
                raster: Com::null(),
                bilinear: Com::null(),
                nearest: Com::null(),
                ds_test: Com::null(),
                ds_clip_inc: Com::null(),
                ds_clip_dec: Com::null(),
                ds_nonzero: Com::null(),
                ds_evenodd: Com::null(),
                ds_cover_nz: Com::null(),
                ds_cover_eo: Com::null(),
                constants: Com::null(),
                vbuf: Com::null(),
                vcap: 0,
                vused: 0,
                frame: None,
                staging: None,
                layers: Vec::new(),
                scratch: None,
                chains: HashMap::new(),
                textures: HashMap::new(),
                externals: external::Imports::default(),
                atlas: HashMap::new(),
                scratch_quads: Vec::new(),
                count: 0,
                uploads: 0,
                draws: 0,
            };
            me.build().map_err(|e| match e {
                GpuError::Lost(e) => GpuError::Unavailable(e),
                e => e,
            })?;
            Ok(me)
        }
    }

    /// Compile the shaders and make the fixed state.
    unsafe fn build(&mut self) -> Result<(), GpuError> {
        unsafe {
            let dev = self.device.ptr();
            let flags = D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3;
            let shader = |entry: &core::ffi::CStr, target: &core::ffi::CStr| {
                compile_source(HLSL.as_bytes(), entry, target, flags)
                    .map(Com)
                    .map_err(|e| GpuError::Unavailable(format!("the HLSL didn't compile: {e}")))
            };
            let vs_blob = shader(c"vs_main", c"vs_4_0")?;
            let (code, len) = blob_bytes(vs_blob.ptr());
            let mut vs = std::ptr::null_mut();
            check(
                com!(dev, VT_DEVICE_CREATE_VERTEX_SHADER,
                    (*const c_void, usize, *mut c_void, *mut *mut c_void) -> Hresult,
                    code, len, std::ptr::null_mut(), &mut vs),
                "CreateVertexShader",
            )?;
            self.vs = Com(vs);
            let element = InputElementDesc {
                SemanticName: c"POSITION".as_ptr() as *const u8,
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 0,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            };
            let mut layout = std::ptr::null_mut();
            check(
                com!(dev, VT_DEVICE_CREATE_INPUT_LAYOUT,
                    (*const InputElementDesc, u32, *const c_void, usize, *mut *mut c_void) -> Hresult,
                    &element, 1, code, len, &mut layout),
                "CreateInputLayout",
            )?;
            self.layout = Com(layout);

            let quad_blob = shader(c"quad_vs", c"vs_4_0")?;
            let (code, len) = blob_bytes(quad_blob.ptr());
            let mut vs = std::ptr::null_mut();
            check(
                com!(dev, VT_DEVICE_CREATE_VERTEX_SHADER,
                    (*const c_void, usize, *mut c_void, *mut *mut c_void) -> Hresult,
                    code, len, std::ptr::null_mut(), &mut vs),
                "CreateVertexShader (quads)",
            )?;
            self.quad_vs = Com(vs);
            let elements = quad_elements();
            let mut layout = std::ptr::null_mut();
            check(
                com!(dev, VT_DEVICE_CREATE_INPUT_LAYOUT,
                    (*const InputElementDesc, u32, *const c_void, usize, *mut *mut c_void) -> Hresult,
                    elements.as_ptr(), elements.len() as u32, code, len, &mut layout),
                "CreateInputLayout (quads)",
            )?;
            self.quad_layout = Com(layout);

            let pixel = |entry: &core::ffi::CStr| -> Result<Com, GpuError> {
                let blob = shader(entry, c"ps_4_0")?;
                let (code, len) = blob_bytes(blob.ptr());
                let mut ps = std::ptr::null_mut();
                check(
                    com!(dev, VT_DEVICE_CREATE_PIXEL_SHADER,
                        (*const c_void, usize, *mut c_void, *mut *mut c_void) -> Hresult,
                        code, len, std::ptr::null_mut(), &mut ps),
                    "CreatePixelShader",
                )?;
                Ok(Com(ps))
            };
            self.ps_quadbox = pixel(c"quadbox_ps")?;
            self.ps_quad = pixel(c"quad_ps")?;
            self.ps_quad_yuv = pixel(c"quad_yuv_ps")?;
            self.ps_flat = pixel(c"flat_ps")?;
            self.ps_resolve = pixel(c"resolve_ps")?;

            let blend = |enable: bool, mask: u8| -> Result<Com, GpuError> {
                let mut desc = BlendDesc::default();
                desc.RenderTarget[0] = RenderTargetBlendDesc {
                    BlendEnable: enable as i32,
                    SrcBlend: D3D11_BLEND_ONE,
                    DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                    BlendOp: D3D11_BLEND_OP_ADD,
                    SrcBlendAlpha: D3D11_BLEND_ONE,
                    DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                    BlendOpAlpha: D3D11_BLEND_OP_ADD,
                    RenderTargetWriteMask: mask,
                };
                let mut state = std::ptr::null_mut();
                check(
                    com!(dev, VT_DEVICE_CREATE_BLEND_STATE,
                        (*const BlendDesc, *mut *mut c_void) -> Hresult, &desc, &mut state),
                    "CreateBlendState",
                )?;
                Ok(Com(state))
            };
            self.blend_premul = blend(true, D3D11_COLOR_WRITE_ENABLE_ALL)?;
            self.blend_opaque = blend(false, D3D11_COLOR_WRITE_ENABLE_ALL)?;
            self.blend_none = blend(false, 0)?;

            let raster = RasterizerDesc {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                DepthClipEnable: 1,
                ScissorEnable: 1,
                ..Default::default()
            };
            let mut state = std::ptr::null_mut();
            check(
                com!(dev, VT_DEVICE_CREATE_RASTERIZER_STATE,
                    (*const RasterizerDesc, *mut *mut c_void) -> Hresult, &raster, &mut state),
                "CreateRasterizerState",
            )?;
            self.raster = Com(state);

            let sampler = |filter: u32| -> Result<Com, GpuError> {
                let desc = SamplerDesc {
                    Filter: filter,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MipLODBias: 0.0,
                    MaxAnisotropy: 1,
                    ComparisonFunc: D3D11_COMPARISON_NEVER,
                    BorderColor: [0.0; 4],
                    MinLOD: 0.0,
                    MaxLOD: D3D11_FLOAT32_MAX,
                };
                let mut state = std::ptr::null_mut();
                check(
                    com!(dev, VT_DEVICE_CREATE_SAMPLER_STATE,
                        (*const SamplerDesc, *mut *mut c_void) -> Hresult, &desc, &mut state),
                    "CreateSamplerState",
                )?;
                Ok(Com(state))
            };
            self.bilinear = sampler(D3D11_FILTER_MIN_MAG_MIP_LINEAR)?;
            self.nearest = sampler(D3D11_FILTER_MIN_MAG_MIP_POINT)?;

            // The stencil states, as the Metal executor's.
            let keep = D3D11_STENCIL_OP_KEEP;
            let eq = D3D11_COMPARISON_EQUAL;
            let always = D3D11_COMPARISON_ALWAYS;
            let ne = D3D11_COMPARISON_NOT_EQUAL;
            self.ds_test = depth_stencil(dev, (eq, keep), (eq, keep), 0xff, 0)?;
            self.ds_clip_inc = depth_stencil(
                dev,
                (eq, D3D11_STENCIL_OP_INCR_SAT),
                (eq, D3D11_STENCIL_OP_INCR_SAT),
                0xff,
                0xff,
            )?;
            self.ds_clip_dec = depth_stencil(
                dev,
                (eq, D3D11_STENCIL_OP_DECR_SAT),
                (eq, D3D11_STENCIL_OP_DECR_SAT),
                0xff,
                0xff,
            )?;
            self.ds_nonzero = depth_stencil(
                dev,
                (always, D3D11_STENCIL_OP_INCR),
                (always, D3D11_STENCIL_OP_DECR),
                0xff,
                0xff,
            )?;
            self.ds_evenodd = depth_stencil(
                dev,
                (always, D3D11_STENCIL_OP_INVERT),
                (always, D3D11_STENCIL_OP_INVERT),
                0xff,
                0xff,
            )?;
            self.ds_cover_nz = depth_stencil(dev, (ne, keep), (ne, keep), 0xff, 0)?;
            self.ds_cover_eo = depth_stencil(dev, (ne, keep), (ne, keep), 1, 0)?;

            self.constants = buffer(
                dev,
                CONSTANTS_BYTES,
                D3D11_BIND_CONSTANT_BUFFER,
                "the constant buffer",
            )?;
            Ok(())
        }
    }

    /// The texture view for `image`, uploading it unless it is cached.
    unsafe fn texture(&mut self, image: &Arc<Pixmap>) -> Option<*mut c_void> {
        unsafe {
            let key = Arc::as_ptr(image) as usize;
            let count = self.count;
            if let Some(t) = self.textures.get_mut(&key) {
                if t.image.upgrade().is_some_and(|i| Arc::ptr_eq(&i, image)) {
                    t.used = count;
                    return Some(t.srv.ptr());
                }
                // The address was reused by another image: upload anew.
                self.textures.remove(&key);
            }
            let size = image.size();
            let (w, h) = (size.width, size.height);
            if w == 0 || h == 0 || w > self.max as u32 || h > self.max as u32 {
                return None;
            }
            // Default rather than immutable usage: a canvas's texture is
            // updated in places (`update_textures`).
            let desc = Texture2dDesc::new(
                w,
                h,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                D3D11_USAGE_DEFAULT,
                D3D11_BIND_SHADER_RESOURCE,
                0,
            );
            let data = SubresourceData {
                pSysMem: image.as_bytes().as_ptr() as *const c_void,
                SysMemPitch: w * 4,
                SysMemSlicePitch: 0,
            };
            let mut tex = std::ptr::null_mut();
            let hr = com!(self.device.ptr(), VT_DEVICE_CREATE_TEXTURE2D,
                (*const Texture2dDesc, *const SubresourceData, *mut *mut c_void) -> Hresult,
                &desc, &data, &mut tex);
            if hr < 0 || tex.is_null() {
                return None;
            }
            let tex = Com(tex);
            let srv = view(self.device.ptr(), VT_DEVICE_CREATE_SRV, tex.ptr()).ok()?;
            self.uploads += 1;
            let ptr = srv.ptr();
            self.textures.insert(
                key,
                Texture {
                    image: Arc::downgrade(image),
                    tex,
                    srv,
                    used: count,
                    bytes: w as usize * h as usize * 4,
                },
            );
            Some(ptr)
        }
    }

    /// Update the textures `plan` changed in places (canvases kept as
    /// textures): `UpdateSubresource` of each rectangle as a box, straight
    /// from the image's rows.
    unsafe fn update_textures(&mut self, plan: &Plan) {
        unsafe {
            let ctx = self.ctx.ptr();
            for u in &plan.updates {
                let Some(t) = rekey(&mut self.textures, u, |t| &mut t.image) else {
                    continue;
                };
                let tex = t.tex.ptr();
                let bytes = u.image.as_bytes();
                for &r in &u.rects {
                    let b = D3d11Box {
                        left: r[0],
                        top: r[1],
                        front: 0,
                        right: r[0] + r[2],
                        bottom: r[1] + r[3],
                        back: 1,
                    };
                    let src = bytes[u.offset(r)..].as_ptr() as *const c_void;
                    com!(ctx, VT_CTX_UPDATE_SUBRESOURCE,
                        (*mut c_void, u32, *const D3d11Box, *const c_void, u32, u32) -> (),
                        tex, 0, &b, src, u.image.stride() as u32, 0);
                }
            }
        }
    }

    /// Free textures whose image is gone or hasn't been drawn lately.
    fn sweep_textures(&mut self) {
        let count = self.count;
        self.textures.retain(|_, t| {
            t.image.strong_count() > 0 && count.saturating_sub(t.used) < TEXTURE_KEEP_FRAMES
        });
    }

    /// Bring the atlas pages' textures up to date with `pages` (a new
    /// texture with the whole page, or the part that changed copied in),
    /// and release those of pages gone.
    unsafe fn sync_atlas(&mut self, pages: &[AtlasPage]) -> Result<(), GpuError> {
        unsafe {
            self.atlas.retain(|id, _| pages.iter().any(|p| p.id == *id));
            for p in pages {
                let have = self.atlas.get(&p.id).map(|t| (t.side, t.version));
                match p.upload(have) {
                    Upload::Nothing => {}
                    Upload::Whole => {
                        self.atlas.remove(&p.id);
                        if p.side > self.max as u32 {
                            continue;
                        }
                        let desc = Texture2dDesc::new(
                            p.side,
                            p.side,
                            DXGI_FORMAT_R8G8B8A8_UNORM,
                            D3D11_USAGE_DEFAULT,
                            D3D11_BIND_SHADER_RESOURCE,
                            0,
                        );
                        let data = SubresourceData {
                            pSysMem: p.pixels.as_ptr() as *const c_void,
                            SysMemPitch: p.side * 4,
                            SysMemSlicePitch: 0,
                        };
                        let mut tex = std::ptr::null_mut();
                        check(
                            com!(self.device.ptr(), VT_DEVICE_CREATE_TEXTURE2D,
                                (*const Texture2dDesc, *const SubresourceData, *mut *mut c_void) -> Hresult,
                                &desc, &data, &mut tex),
                            "CreateTexture2D (atlas)",
                        )?;
                        let tex = Com(tex);
                        let srv = view(self.device.ptr(), VT_DEVICE_CREATE_SRV, tex.ptr())?;
                        self.atlas.insert(
                            p.id,
                            AtlasTex {
                                tex,
                                srv,
                                side: p.side,
                                version: p.version,
                            },
                        );
                        self.uploads += 1;
                    }
                    Upload::Region(r) => {
                        let t = self.atlas.get_mut(&p.id).unwrap();
                        let bytes = p.region(r);
                        let b = D3dBox::pixels(r);
                        com!(self.ctx.ptr(), VT_CTX_UPDATE_SUBRESOURCE,
                            (*mut c_void, u32, *const D3dBox, *const c_void, u32, u32) -> (),
                            t.tex.ptr(), 0, &b, bytes.as_ptr() as *const c_void, (r[2] - r[0]) * 4, 0);
                        t.version = p.version;
                        self.uploads += 1;
                    }
                }
            }
            Ok(())
        }
    }

    /// `(rtv, dsv, w, h)` of `on`.
    fn target_of(&self, f: &Frame, on: On) -> (*mut c_void, *mut c_void, i32, i32) {
        let of = |t: &Target| (t.rtv.ptr(), t.stencil.dsv.ptr(), t.w, t.h);
        match on {
            On::Frame => (f.rtv, f.dsv, f.w, f.h),
            On::Layer(i) => of(&self.layers[i]),
            On::Scratch => of(self.scratch.as_ref().unwrap()),
        }
    }

    /// Bind `on` as the render target, cleared to `clear` (premultiplied) and
    /// a zero stencil, or keeping what it holds.
    unsafe fn open(&self, f: &mut Frame, on: On, clear: Option<[f32; 4]>) {
        unsafe {
            let ctx = self.ctx.ptr();
            // A texture can't be read and drawn into at once: unbind the
            // last one read (a layer or the scratch) first.
            let none: *mut c_void = std::ptr::null_mut();
            com!(ctx, VT_CTX_PS_SET_SHADER_RESOURCES,
                (u32, u32, *const *mut c_void) -> (), 0, 1, &none);
            let (rtv, dsv, w, h) = self.target_of(f, on);
            com!(ctx, VT_CTX_OM_SET_RENDER_TARGETS,
                (u32, *const *mut c_void, *mut c_void) -> (), 1, &rtv, dsv);
            let vp = Viewport {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: w as f32,
                Height: h as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            com!(ctx, VT_CTX_RS_SET_VIEWPORTS, (u32, *const Viewport) -> (), 1, &vp);
            if let Some(c) = clear {
                com!(ctx, VT_CTX_CLEAR_RTV, (*mut c_void, *const [f32; 4]) -> (), rtv, &c);
                com!(ctx, VT_CTX_CLEAR_DSV, (*mut c_void, u32, f32, u8) -> (),
                    dsv, D3D11_CLEAR_STENCIL, 1.0, 0);
            }
            f.on = Some(on);
        }
    }

    /// Get ready to draw into `level` with `prog`, `ds` and stencil value
    /// `reference`. `false` when its scissor is empty (draw nothing).
    unsafe fn bind(
        &self,
        f: &mut Frame,
        level: &Level,
        prog: Prog,
        ds: &Com,
        reference: u32,
    ) -> bool {
        let s = level.scissor;
        if s[2] <= s[0] || s[3] <= s[1] {
            return false;
        }
        unsafe {
            if f.on != Some(level.on()) {
                self.open(f, level.on(), None);
            }
            self.state(prog, ds, reference, s);
        }
        true
    }

    unsafe fn state(&self, prog: Prog, ds: &Com, reference: u32, s: [i32; 4]) {
        unsafe {
            let ctx = self.ctx.ptr();
            com!(ctx, VT_CTX_PS_SET_SHADER,
                (*mut c_void, *const *mut c_void, u32) -> (), prog.ps, std::ptr::null(), 0);
            com!(ctx, VT_CTX_OM_SET_BLEND_STATE,
                (*mut c_void, *const [f32; 4], u32) -> (), prog.blend, std::ptr::null(), 0xffff_ffff);
            com!(ctx, VT_CTX_OM_SET_DEPTH_STENCIL_STATE,
                (*mut c_void, u32) -> (), ds.ptr(), reference);
            let r = scissor_rect(s);
            com!(ctx, VT_CTX_RS_SET_SCISSOR_RECTS, (u32, *const D3dRect) -> (), 1, &r);
        }
    }

    /// The fixed state every frame draws with (after `ClearState`, or
    /// whatever was bound before).
    unsafe fn fixed_state(&mut self) {
        unsafe {
            let ctx = self.ctx.ptr();
            self.quad_bound = true;
            self.use_quads(false);
            com!(ctx, VT_CTX_IA_SET_PRIMITIVE_TOPOLOGY, (u32) -> (),
                D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            let cb = self.constants.ptr();
            com!(ctx, VT_CTX_VS_SET_CONSTANT_BUFFERS,
                (u32, u32, *const *mut c_void) -> (), 0, 1, &cb);
            com!(ctx, VT_CTX_PS_SET_CONSTANT_BUFFERS,
                (u32, u32, *const *mut c_void) -> (), 0, 1, &cb);
            let samplers = [self.bilinear.ptr(), self.nearest.ptr()];
            com!(ctx, VT_CTX_PS_SET_SAMPLERS,
                (u32, u32, *const *mut c_void) -> (), 0, 2, samplers.as_ptr());
            com!(ctx, VT_CTX_RS_SET_STATE, (*mut c_void) -> (), self.raster.ptr());
        }
    }

    /// Bind the quads' vertex shader and input layout (`quads`), or the
    /// paths'.
    unsafe fn use_quads(&mut self, quads: bool) {
        if self.quad_bound == quads {
            return;
        }
        self.quad_bound = quads;
        let (layout, vs) = if quads {
            (self.quad_layout.ptr(), self.quad_vs.ptr())
        } else {
            (self.layout.ptr(), self.vs.ptr())
        };
        unsafe {
            let ctx = self.ctx.ptr();
            com!(ctx, VT_CTX_IA_SET_INPUT_LAYOUT, (*mut c_void) -> (), layout);
            com!(ctx, VT_CTX_VS_SET_SHADER,
                (*mut c_void, *const *mut c_void, u32) -> (), vs, std::ptr::null(), 0);
        }
    }

    fn prog(ps: &Com, blend: &Com) -> Prog {
        Prog {
            ps: ps.ptr(),
            blend: blend.ptr(),
        }
    }

    /// Draw `verts` (`[x, y, aux_x, aux_y]` each) as triangles with the
    /// shader constants `u` (`u[0]` the device-to-clip mapping).
    unsafe fn submit(&mut self, verts: &[f32], u: &[[f32; 4]]) -> Result<(), GpuError> {
        unsafe {
            if verts.is_empty() {
                return Ok(());
            }
            self.use_quads(false);
            self.draw_vertices(verts, 16, u)
        }
    }

    /// Draw `verts` (`stride` bytes each) as triangles with the shader
    /// constants `u`, through the bound vertex shader and layout.
    unsafe fn draw_vertices(
        &mut self,
        verts: &[f32],
        stride: u32,
        u: &[[f32; 4]],
    ) -> Result<(), GpuError> {
        unsafe {
            let ctx = self.ctx.ptr();
            let bytes = std::mem::size_of_val(verts);
            if bytes == 0 {
                return Ok(());
            }
            // Vertices into the ring: after what this frame already used, or
            // from the start of a fresh buffer when it wraps.
            let map = if self.vused + bytes > self.vcap {
                if bytes > self.vcap {
                    let cap = ARENA.max(bytes.next_power_of_two());
                    self.vbuf = Com::null();
                    self.vcap = 0;
                    self.vbuf = buffer(
                        self.device.ptr(),
                        cap,
                        D3D11_BIND_VERTEX_BUFFER,
                        "a vertex buffer",
                    )?;
                    self.vcap = cap;
                }
                self.vused = 0;
                D3D11_MAP_WRITE_DISCARD
            } else {
                D3D11_MAP_WRITE_NO_OVERWRITE
            };
            let mut m = MappedSubresource {
                pData: std::ptr::null_mut(),
                RowPitch: 0,
                DepthPitch: 0,
            };
            check(
                com!(ctx, VT_CTX_MAP,
                    (*mut c_void, u32, u32, u32, *mut MappedSubresource) -> Hresult,
                    self.vbuf.ptr(), 0, map, 0, &mut m),
                "Map (vertices)",
            )?;
            std::ptr::copy_nonoverlapping(
                verts.as_ptr() as *const u8,
                (m.pData as *mut u8).add(self.vused),
                bytes,
            );
            com!(ctx, VT_CTX_UNMAP, (*mut c_void, u32) -> (), self.vbuf.ptr(), 0);
            let (buf, offset) = (self.vbuf.ptr(), self.vused as u32);
            com!(ctx, VT_CTX_IA_SET_VERTEX_BUFFERS,
                (u32, u32, *const *mut c_void, *const u32, *const u32) -> (),
                0, 1, &buf, &stride, &offset);
            self.vused += bytes.next_multiple_of(16);

            // The constants.
            check(
                com!(ctx, VT_CTX_MAP,
                    (*mut c_void, u32, u32, u32, *mut MappedSubresource) -> Hresult,
                    self.constants.ptr(), 0, D3D11_MAP_WRITE_DISCARD, 0, &mut m),
                "Map (constants)",
            )?;
            let mut all: Constants = [[0.0; 4]; 8];
            all[..u.len()].copy_from_slice(u);
            std::ptr::copy_nonoverlapping(
                all.as_ptr() as *const u8,
                m.pData as *mut u8,
                CONSTANTS_BYTES,
            );
            com!(ctx, VT_CTX_UNMAP, (*mut c_void, u32) -> (), self.constants.ptr(), 0);

            let count = (bytes / stride as usize) as u32;
            com!(ctx, VT_CTX_DRAW, (u32, u32) -> (), count, 0);
            self.draws += 1;
            Ok(())
        }
    }

    unsafe fn bind_texture(&self, srv: *mut c_void) {
        unsafe {
            com!(self.ctx.ptr(), VT_CTX_PS_SET_SHADER_RESOURCES,
                (u32, u32, *const *mut c_void) -> (), 0, 1, &srv);
        }
    }

    /// Draw the quad vertices `verts` (see `crate::gpu::batch`) in one
    /// call: sampling `srv` (nearest-neighbor or bilinear), or boxes only
    /// without one.
    unsafe fn draw_quads(
        &mut self,
        f: &mut Frame,
        level: &Level,
        verts: &[f32],
        srv: Option<*mut c_void>,
        nearest: bool,
    ) -> Result<(), GpuError> {
        unsafe { self.draw_quads_with(f, level, verts, srv, None, nearest) }
    }

    /// [`draw_quads`](Self::draw_quads), with `chroma` NV12's chroma view
    /// (`srv` its luma) and the color space it's converted in.
    unsafe fn draw_quads_with(
        &mut self,
        f: &mut Frame,
        level: &Level,
        verts: &[f32],
        srv: Option<*mut c_void>,
        chroma: Option<(*mut c_void, crate::render::external::YuvColorSpace)>,
        nearest: bool,
    ) -> Result<(), GpuError> {
        unsafe {
            let ps = match (srv, chroma) {
                (Some(_), Some(_)) => &self.ps_quad_yuv,
                (Some(_), None) => &self.ps_quad,
                _ => &self.ps_quadbox,
            };
            let prog = Self::prog(ps, &self.blend_premul);
            if verts.is_empty() || !self.bind(f, level, prog, &self.ds_test, level.depth) {
                return Ok(());
            }
            if let Some(srv) = srv {
                self.bind_texture(srv);
            }
            self.use_quads(true);
            let xf = device_to_clip(f.w, f.h);
            let Some((uv, color)) = chroma else {
                let u = quad_constants(xf, nearest);
                return self.draw_vertices(verts, QUAD_STRIDE, &u);
            };
            let ctx = self.ctx.ptr();
            com!(ctx, VT_CTX_PS_SET_SHADER_RESOURCES,
                (u32, u32, *const *mut c_void) -> (), 1, 1, &uv);
            let u = quad_yuv_constants(xf, nearest, color.rows());
            let drawn = self.draw_vertices(verts, QUAD_STRIDE, &u);
            let none: *mut c_void = std::ptr::null_mut();
            com!(ctx, VT_CTX_PS_SET_SHADER_RESOURCES,
                (u32, u32, *const *mut c_void) -> (), 1, 1, &none);
            drawn
        }
    }

    /// Fill `contours` by stencil-and-cover: stencil the winding into the
    /// supersampled scratch target, cover it to get coverage, then resolve
    /// that into `level` in `color`.
    unsafe fn draw_path(
        &mut self,
        f: &mut Frame,
        level: &Level,
        contours: &[Vec<[f32; 2]>],
        even_odd: bool,
        color: [f32; 4],
        bbox: [i32; 4],
    ) -> Result<(), GpuError> {
        unsafe {
            let s = level.scissor;
            let b = [
                bbox[0].max(s[0]),
                bbox[1].max(s[1]),
                bbox[2].min(s[2]),
                bbox[3].min(s[3]),
            ];
            let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
            if bw <= 0 || bh <= 0 {
                return Ok(());
            }
            let max = self.max;
            let ss = [4, 2, 1]
                .into_iter()
                .find(|ss| bw * ss <= max && bh * ss <= max)
                .unwrap_or(1);
            let (sw, sh) = (bw * ss, bh * ss);
            if self.scratch.as_ref().is_none_or(|t| t.w < sw || t.h < sh) {
                let (gw, gh) = self.scratch.as_ref().map_or((0, 0), |t| (t.w, t.h));
                let grow = |want: i32, have: i32| {
                    (want.max(have) as u32)
                        .next_power_of_two()
                        .clamp(64, max as u32) as i32
                };
                self.scratch = None;
                self.scratch = Some(Target::new(
                    self.device.ptr(),
                    grow(sw, gw),
                    grow(sh, gh),
                    DXGI_FORMAT_R8_UNORM,
                )?);
            }
            let (vw, vh) = {
                let t = self.scratch.as_ref().unwrap();
                (t.w as f32, t.h as f32)
            };
            self.open(f, On::Scratch, Some([0.0; 4]));
            let fs = ss as f32;
            // Device pixels → scratch clip space.
            let xf = [
                2.0 * fs / vw,
                -2.0 * fs / vh,
                -2.0 * fs * b[0] as f32 / vw - 1.0,
                2.0 * fs * b[1] as f32 / vh + 1.0,
            ];
            let all = [0, 0, sw, sh];
            let white = [1.0f32; 4];

            // Stencil: count the winding of every fan triangle.
            let ds = if even_odd {
                &self.ds_evenodd
            } else {
                &self.ds_nonzero
            };
            self.state(Self::prog(&self.ps_flat, &self.blend_none), ds, 0, all);
            let fans = fan_triangles(contours.iter().map(|c| c.as_slice()));
            if !fans.is_empty() {
                self.submit(&fans, &flat_constants(xf, white))?;
            }

            // Cover: full coverage wherever the winding says inside.
            let ds = if even_odd {
                &self.ds_cover_eo
            } else {
                &self.ds_cover_nz
            };
            self.state(Self::prog(&self.ps_flat, &self.blend_opaque), ds, 0, all);
            let (x0, y0, x1, y1) = (b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32);
            #[rustfmt::skip]
            let quad = [
                x0, y0, 0.0, 0.0,  x1, y0, 0.0, 0.0,  x0, y1, 0.0, 0.0,
                x0, y1, 0.0, 0.0,  x1, y0, 0.0, 0.0,  x1, y1, 0.0, 0.0,
            ];
            self.submit(&quad, &flat_constants(xf, white))?;

            // Resolve into the level.
            let prog = Self::prog(&self.ps_resolve, &self.blend_premul);
            if !self.bind(f, level, prog, &self.ds_test, level.depth) {
                return Ok(());
            }
            let st = |x: f32, y: f32| [x, y, (x - x0) * fs, (y - y0) * fs];
            let v: Vec<f32> = [
                st(x0, y0),
                st(x1, y0),
                st(x0, y1),
                st(x0, y1),
                st(x1, y0),
                st(x1, y1),
            ]
            .concat();
            self.bind_texture(self.scratch.as_ref().unwrap().srv.ptr());
            let u = resolve_constants(
                device_to_clip(f.w, f.h),
                color,
                [1.0 / vw, 1.0 / vh],
                if ss > 1 { fs * 0.25 } else { 0.0 },
            );
            self.submit(&v, &u)
        }
    }

    /// Draw the polygon `pts` into the stencil of `level`'s target, where it
    /// is `from`, with `ds` (increment or decrement).
    unsafe fn stencil_polygon(
        &mut self,
        f: &mut Frame,
        level: &Level,
        pts: &[[f32; 2]],
        from: u32,
        dec: bool,
    ) -> Result<(), GpuError> {
        unsafe {
            let (w, h) = (f.w, f.h);
            let full = Level {
                layer: level.layer,
                opacity: 1.0,
                scissor: [0, 0, w, h],
                depth: 0,
                undo: Vec::new(),
                resample: None,
            };
            let ds = if dec {
                &self.ds_clip_dec
            } else {
                &self.ds_clip_inc
            };
            if !self.bind(
                f,
                &full,
                Self::prog(&self.ps_flat, &self.blend_none),
                ds,
                from,
            ) {
                return Ok(());
            }
            let fans = fan_triangles(std::iter::once(pts));
            self.submit(&fans, &flat_constants(device_to_clip(w, h), [0.0; 4]))
        }
    }

    /// Composite the layer `layer` into `parent`.
    unsafe fn composite(
        &mut self,
        f: &mut Frame,
        layer: &Level,
        parent: &Level,
    ) -> Result<(), GpuError> {
        unsafe {
            let srv = self.layers[layer.layer.unwrap()].srv.ptr();
            let mut v = std::mem::take(&mut self.scratch_quads);
            v.clear();
            let (w, h) = (f.w as u32, f.h as u32);
            let nearest = batch::layer_quad(&mut v, layer.opacity, layer.resample.as_ref(), w, h);
            let drawn = self.draw_quads(f, parent, &v, Some(srv), nearest);
            self.scratch_quads = v;
            drawn
        }
    }

    /// Draw `plan` into `f`'s target.
    unsafe fn encode(&mut self, f: &mut Frame, plan: &Plan) -> Result<(), GpuError> {
        unsafe {
            let (w, h) = (f.w, f.h);
            if self.layers.first().is_some_and(|l| l.w != w || l.h != h) {
                self.layers.clear();
            }
            self.uploads = 0;
            self.draws = 0;
            self.externals.begin(self.count);
            self.update_textures(plan);
            self.fixed_state();
            self.sync_atlas(&plan.atlas)?;
            let mut levels = vec![Level {
                layer: None,
                opacity: 1.0,
                scissor: [0, 0, w, h],
                depth: 0,
                undo: Vec::new(),
                resample: None,
            }];
            let bg = plan.background;
            let a = bg.a as f32 / 255.0;
            let clear = [
                bg.r as f32 / 255.0 * a,
                bg.g as f32 / 255.0 * a,
                bg.b as f32 / 255.0 * a,
                a,
            ];
            if plan.partial.is_some() {
                // Keep the colors the target holds; only the stencil starts
                // cleared.
                self.open(f, On::Frame, None);
                com!(self.ctx.ptr(), VT_CTX_CLEAR_DSV, (*mut c_void, u32, f32, u8) -> (),
                    f.dsv, D3D11_CLEAR_STENCIL, 1.0, 0);
            } else {
                self.open(f, On::Frame, Some(clear));
            }

            for cmd in &plan.cmds {
                let op = match cmd {
                    Cmd::Quads(q) => {
                        let mut chroma = None;
                        let srv = match &q.texture {
                            None => None,
                            Some(TexRef::Image(image)) => self.texture(image),
                            Some(TexRef::Atlas(i)) => plan
                                .atlas
                                .get(*i)
                                .and_then(|p| self.atlas.get(&p.id))
                                .map(|t| t.srv.ptr()),
                            Some(TexRef::External(image)) => {
                                let dev = self.device.ptr();
                                match self.externals.get(dev, image, self.count) {
                                    Some(v) => {
                                        chroma = v.chroma;
                                        Some(v.srv)
                                    }
                                    // Its pixels, if it has some for the CPU.
                                    None => image.pixmap().and_then(|p| self.texture(&p)),
                                }
                            }
                        };
                        let n = QUAD_FLOATS;
                        let verts =
                            &plan.quads[q.first as usize * n..(q.first + q.count) as usize * n];
                        let level = levels.last().unwrap();
                        self.draw_quads_with(f, level, verts, srv, chroma, q.nearest)?;
                        continue;
                    }
                    Cmd::Op(i) => &plan.ops[*i],
                };
                match op {
                    // Batched into `Cmd::Quads`.
                    Op::Box { .. } | Op::Image { .. } => {}
                    Op::Path {
                        contours,
                        even_odd,
                        color,
                        bbox,
                    } => {
                        let level = levels.last().unwrap();
                        self.draw_path(f, level, contours, *even_odd, *color, *bbox)?;
                    }
                    Op::PushClip(Clip::Rect(r)) => {
                        let level = levels.last_mut().unwrap();
                        let s = level.scissor;
                        level.undo.push(Undo::Scissor(s));
                        let mut n = [
                            s[0].max(r[0]),
                            s[1].max(r[1]),
                            s[2].min(r[2]),
                            s[3].min(r[3]),
                        ];
                        n[2] = n[2].max(n[0]);
                        n[3] = n[3].max(n[1]);
                        level.scissor = n;
                    }
                    Op::PushClip(Clip::Polygon(pts)) => {
                        let level = levels.last().unwrap();
                        self.stencil_polygon(f, level, pts, level.depth, false)?;
                        let level = levels.last_mut().unwrap();
                        level.depth += 1;
                        level.undo.push(Undo::Stencil(pts.clone()));
                    }
                    Op::PopClip => {
                        let level = levels.last_mut().unwrap();
                        match level.undo.pop() {
                            Some(Undo::Scissor(s)) => level.scissor = s,
                            Some(Undo::Stencil(pts)) => {
                                let level = levels.last().unwrap();
                                self.stencil_polygon(f, level, &pts, level.depth, true)?;
                                levels.last_mut().unwrap().depth -= 1;
                            }
                            None => {}
                        }
                    }
                    Op::PushLayer { .. } | Op::PushResampled(_) => {
                        let index = levels.iter().filter(|l| l.layer.is_some()).count();
                        if self.layers.len() <= index {
                            let t =
                                Target::new(self.device.ptr(), w, h, DXGI_FORMAT_B8G8R8A8_UNORM)?;
                            self.layers.push(t);
                        }
                        let (opacity, resample) = match op {
                            Op::PushResampled(r) => (1.0, Some(*r)),
                            Op::PushLayer { opacity } => (*opacity, None),
                            _ => unreachable!(),
                        };
                        // A resampled layer draws in its own pixels.
                        let scissor = resample
                            .as_ref()
                            .map_or(levels.last().unwrap().scissor, Resample::scissor);
                        levels.push(Level {
                            layer: Some(index),
                            opacity,
                            scissor,
                            depth: 0,
                            undo: Vec::new(),
                            resample,
                        });
                        self.open(f, On::Layer(index), Some([0.0; 4]));
                    }
                    Op::PopLayer => {
                        if levels.len() < 2 {
                            continue;
                        }
                        let layer = levels.pop().unwrap();
                        self.composite(f, &layer, levels.last().unwrap())?;
                    }
                }
            }
            // Close any layers left open.
            while levels.len() > 1 {
                let layer = levels.pop().unwrap();
                self.composite(f, &layer, levels.last().unwrap())?;
            }
            // Let go of the targets (a swap chain's buffer must not stay
            // bound when it's presented or resized).
            let ctx = self.ctx.ptr();
            let none: *mut c_void = std::ptr::null_mut();
            com!(ctx, VT_CTX_PS_SET_SHADER_RESOURCES,
                (u32, u32, *const *mut c_void) -> (), 0, 1, &none);
            com!(ctx, VT_CTX_OM_SET_RENDER_TARGETS,
                (u32, *const *mut c_void, *mut c_void) -> (), 0, std::ptr::null(), std::ptr::null_mut());
            Ok(())
        }
    }

    fn check_size(&self, plan: &Plan) -> Result<(i32, i32), GpuError> {
        let (w, h) = (plan.size.width as i32, plan.size.height as i32);
        if w <= 0 || h <= 0 || w > self.max || h > self.max {
            return Err(GpuError::Lost(format!(
                "{w}x{h} is outside Direct3D's texture sizes"
            )));
        }
        Ok((w, h))
    }

    /// The device's removal reason, as an error, if it was removed.
    unsafe fn removed(&self) -> Result<(), GpuError> {
        let hr =
            unsafe { com!(self.device.ptr(), VT_DEVICE_GET_DEVICE_REMOVED_REASON, () -> Hresult) };
        if hr < 0 {
            return Err(GpuError::Lost(format!(
                "the Direct3D device was removed (0x{:08x})",
                hr as u32
            )));
        }
        Ok(())
    }

    /// Draw `plan` into the offscreen frame target and copy it back
    /// (premultiplied BGRA).
    unsafe fn offscreen(&mut self, plan: &Plan) -> Result<Vec<u8>, GpuError> {
        unsafe {
            self.removed()?;
            let (w, h) = self.check_size(plan)?;
            if self.frame.as_ref().is_none_or(|t| t.w != w || t.h != h) {
                self.frame = None;
                self.frame = Some(Target::new(
                    self.device.ptr(),
                    w,
                    h,
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                )?);
            }
            let (rtv, dsv, color) = {
                let t = self.frame.as_ref().unwrap();
                (t.rtv.ptr(), t.stencil.dsv.ptr(), t.color.ptr())
            };
            let mut f = Frame {
                rtv,
                dsv,
                w,
                h,
                on: None,
            };
            let encoded = self.encode(&mut f, plan);
            self.finish_frame(encoded)?;

            if self.staging.as_ref().is_none_or(|s| (s.1, s.2) != (w, h)) {
                self.staging = None;
                let desc = Texture2dDesc::new(
                    w as u32,
                    h as u32,
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    D3D11_USAGE_STAGING,
                    0,
                    D3D11_CPU_ACCESS_READ,
                );
                let tex = texture2d(self.device.ptr(), &desc, "the staging texture")?;
                self.staging = Some((tex, w, h));
            }
            let ctx = self.ctx.ptr();
            let staging = self.staging.as_ref().unwrap().0.ptr();
            com!(ctx, VT_CTX_COPY_RESOURCE, (*mut c_void, *mut c_void) -> (), staging, color);
            let mut m = MappedSubresource {
                pData: std::ptr::null_mut(),
                RowPitch: 0,
                DepthPitch: 0,
            };
            // Mapping for reading waits for the GPU to finish the frame.
            let hr = com!(ctx, VT_CTX_MAP,
                (*mut c_void, u32, u32, u32, *mut MappedSubresource) -> Hresult,
                staging, 0, D3D11_MAP_READ, 0, &mut m);
            if hr < 0 || m.pData.is_null() {
                self.removed()?;
                return Err(failed(hr, "Map (read-back)"));
            }
            let row = w as usize * 4;
            let mut out = vec![0u8; row * h as usize];
            for y in 0..h as usize {
                std::ptr::copy_nonoverlapping(
                    (m.pData as *const u8).add(y * m.RowPitch as usize),
                    out.as_mut_ptr().add(y * row),
                    row,
                );
            }
            com!(ctx, VT_CTX_UNMAP, (*mut c_void, u32) -> (), staging, 0);
            Ok(out)
        }
    }

    /// End a frame's bookkeeping, whether `encoded` or not.
    fn finish_frame(&mut self, encoded: Result<(), GpuError>) -> Result<(), GpuError> {
        self.sweep_textures();
        self.count += 1;
        encoded
    }

    /// A swap chain for `hwnd`, `w`×`h`: flip-discard where there is one,
    /// with a frame-latency waitable object where there is one.
    unsafe fn create_chain(&self, hwnd: *mut c_void, w: i32, h: i32) -> Result<Chain, GpuError> {
        unsafe {
            // The factory that made the device's adapter.
            let mut dxgi = std::ptr::null_mut();
            check(
                com!(self.device.ptr(), VT_QUERY_INTERFACE,
                    (*const Guid, *mut *mut c_void) -> Hresult, &IID_IDXGI_DEVICE, &mut dxgi),
                "QueryInterface(IDXGIDevice)",
            )?;
            let dxgi = Com(dxgi);
            let mut adapter = std::ptr::null_mut();
            check(
                com!(dxgi.ptr(), VT_DXGI_DEVICE_GET_ADAPTER,
                    (*mut *mut c_void) -> Hresult, &mut adapter),
                "IDXGIDevice::GetAdapter",
            )?;
            let adapter = Com(adapter);
            let mut factory = std::ptr::null_mut();
            let hr = com!(adapter.ptr(), VT_DXGI_OBJECT_GET_PARENT,
                (*const Guid, *mut *mut c_void) -> Hresult, &IID_IDXGI_FACTORY2, &mut factory);
            if hr < 0 || factory.is_null() {
                return Err(GpuError::Unavailable(format!(
                    "no IDXGIFactory2 (0x{:08x})",
                    hr as u32
                )));
            }
            let factory = Com(factory);
            let mut last = 0;
            for (effect, flags) in SWAP_CHAIN_LADDER {
                let desc = swap_chain_desc(w as u32, h as u32, effect, flags);
                let mut swap = std::ptr::null_mut();
                let hr = com!(factory.ptr(), VT_FACTORY2_CREATE_SWAP_CHAIN_FOR_HWND,
                    (*mut c_void, *mut c_void, *const SwapChainDesc1, *const c_void, *mut c_void,
                        *mut *mut c_void) -> Hresult,
                    self.device.ptr(), hwnd, &desc, std::ptr::null(), std::ptr::null_mut(),
                    &mut swap);
                if hr < 0 || swap.is_null() {
                    last = hr;
                    continue;
                }
                let swap = Com(swap);
                // Alt+Enter doesn't take the window fullscreen: the app does that.
                com!(factory.ptr(), VT_FACTORY_MAKE_WINDOW_ASSOCIATION,
                    (*mut c_void, u32) -> Hresult, hwnd, DXGI_MWA_NO_ALT_ENTER);
                let mut waitable = std::ptr::null_mut();
                if flags & DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT != 0 {
                    let mut swap2 = std::ptr::null_mut();
                    let hr = com!(swap.ptr(), VT_QUERY_INTERFACE,
                        (*const Guid, *mut *mut c_void) -> Hresult,
                        &IID_IDXGI_SWAP_CHAIN2, &mut swap2);
                    if hr >= 0 && !swap2.is_null() {
                        let swap2 = Com(swap2);
                        // One frame queued at most: input to photons in a frame.
                        com!(swap2.ptr(), VT_SWAP2_SET_MAXIMUM_FRAME_LATENCY,
                            (u32) -> Hresult, 1);
                        waitable = com!(swap2.ptr(), VT_SWAP2_GET_FRAME_LATENCY_WAITABLE_OBJECT,
                            () -> *mut c_void);
                        // Its signal paces the frame clock from now on.
                        vsync::pace_by(waitable);
                    }
                }
                return Ok(Chain {
                    swap,
                    waitable,
                    effect,
                    flags,
                    w,
                    h,
                    stencil: None,
                    presents: 0,
                    log: DamageLog::default(),
                });
            }
            Err(GpuError::Unavailable(format!(
                "CreateSwapChainForHwnd failed (0x{:08x})",
                last as u32
            )))
        }
    }

    /// Unbind everything and run the deferred releases, so a swap chain's
    /// buffers have no references left (before resizing or dropping it).
    unsafe fn settle(&self) {
        unsafe {
            com!(self.ctx.ptr(), VT_CTX_CLEAR_STATE, () -> ());
            com!(self.ctx.ptr(), VT_CTX_FLUSH, () -> ());
        }
    }

    unsafe fn present_to(
        &mut self,
        plan: &mut Plan,
        hwnd: *mut c_void,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        unsafe {
            self.removed()?;
            let (w, h) = self.check_size(plan)?;
            // Swap chains of windows that are gone.
            let before = self.chains.len();
            self.chains.retain(|&k, _| IsWindow(k as *mut c_void) != 0);
            if self.chains.len() != before {
                self.settle();
            }
            let key = hwnd as usize;
            if !self.chains.contains_key(&key) {
                let chain = self.create_chain(hwnd, w, h)?;
                self.chains.insert(key, chain);
            }
            let (resize, flags) = {
                let c = &self.chains[&key];
                ((c.w, c.h) != (w, h), c.flags)
            };
            if resize {
                self.settle();
                let swap = self.chains[&key].swap.ptr();
                let hr = com!(swap, VT_SWAP_RESIZE_BUFFERS,
                    (u32, u32, u32, u32, u32) -> Hresult,
                    0, w as u32, h as u32, DXGI_FORMAT_UNKNOWN, flags);
                check(hr, "IDXGISwapChain::ResizeBuffers")?;
                let c = self.chains.get_mut(&key).unwrap();
                (c.w, c.h) = (w, h);
                // New buffers, holding no frame.
                c.presents = 0;
                c.log.clear();
            }
            // Wait until the swap chain takes another frame (at most one is
            // queued), so the frame drawn now is the next one shown.
            let waitable = self.chains[&key].waitable;
            // (The frame clock's thread may have waited for it already: then
            // this returns at once.)
            if !waitable.is_null()
                && !vsync::wait_for_slot(
                    waitable,
                    std::time::Duration::from_millis(FRAME_WAIT_MS.into()),
                )
            {
                return Err(GpuError::Skipped);
            }
            if self.chains[&key]
                .stencil
                .as_ref()
                .is_none_or(|s| (s.w, s.h) != (w, h))
            {
                let s = Stencil::new(self.device.ptr(), w, h)?;
                self.chains.get_mut(&key).unwrap().stencil = Some(s);
            }
            let swap = self.chains[&key].swap.ptr();
            let dsv = self.chains[&key].stencil.as_ref().unwrap().dsv.ptr();
            // Buffer 0 is always the one to draw into next.
            let mut back = std::ptr::null_mut();
            check(
                com!(swap, VT_SWAP_GET_BUFFER,
                    (u32, *const Guid, *mut *mut c_void) -> Hresult,
                    0, &IID_ID3D11_TEXTURE2D, &mut back),
                "IDXGISwapChain::GetBuffer",
            )?;
            let back = Com(back);
            let rtv = view(self.device.ptr(), VT_DEVICE_CREATE_RTV, back.ptr())?;
            let mut f = Frame {
                rtv: rtv.ptr(),
                dsv,
                w,
                h,
                on: None,
            };
            // Only what changed since the frame the buffer holds (the one
            // before last), when it holds one; then present just that.
            let region = {
                let c = &self.chains[&key];
                let age = back_buffer_age(c.effect, c.presents);
                let r = (plan.background.a == 255)
                    .then(|| c.log.region(age, (w as u32, h as u32), dirty))
                    .flatten();
                if let Some(r) = r {
                    plan.limit(r);
                }
                r
            };
            let encoded = self.encode(&mut f, plan);
            drop(rtv);
            drop(back);
            if encoded.is_err() {
                let c = self.chains.get_mut(&key).unwrap();
                c.presents = 0;
                c.log.clear();
            }
            self.finish_frame(encoded)?;
            let hr = match region {
                Some(r) => {
                    let rect = scissor_rect(r);
                    let params = PresentParameters {
                        DirtyRectsCount: 1,
                        pDirtyRects: &rect,
                        pScrollRect: std::ptr::null(),
                        pScrollOffset: std::ptr::null(),
                    };
                    com!(swap, VT_SWAP1_PRESENT1,
                        (u32, u32, *const PresentParameters) -> Hresult, 1, 0, &params)
                }
                None => com!(swap, VT_SWAP_PRESENT, (u32, u32) -> Hresult, 1, 0),
            };
            let c = self.chains.get_mut(&key).unwrap();
            if hr == S_OK {
                c.presents += 1;
                c.log.presented((w as u32, h as u32), dirty);
            } else {
                // Occluded, or failed: start over with a whole frame.
                c.presents = 0;
                c.log.clear();
            }
            match outcome(hr) {
                // Minimized, cloaked, the screen locked: shown nowhere.
                Outcome::Ok if hr == DXGI_STATUS_OCCLUDED => Err(GpuError::Occluded),
                Outcome::Ok => Ok(()),
                _ => Err(failed(hr, "IDXGISwapChain::Present")),
            }
        }
    }
}

/// A buffer of `bytes` the CPU writes and the GPU reads, bound as `bind`.
unsafe fn buffer(dev: *mut c_void, bytes: usize, bind: u32, what: &str) -> Result<Com, GpuError> {
    let desc = BufferDesc {
        ByteWidth: bytes as u32,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: bind,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut buf = std::ptr::null_mut();
    let hr = unsafe {
        com!(dev, VT_DEVICE_CREATE_BUFFER,
            (*const BufferDesc, *const SubresourceData, *mut *mut c_void) -> Hresult,
            &desc, std::ptr::null(), &mut buf)
    };
    if hr < 0 || buf.is_null() {
        return Err(failed(hr, &format!("CreateBuffer ({what})")));
    }
    Ok(Com(buf))
}

/// An uninitialized texture.
unsafe fn texture2d(dev: *mut c_void, desc: &Texture2dDesc, what: &str) -> Result<Com, GpuError> {
    let mut tex = std::ptr::null_mut();
    let hr = unsafe {
        com!(dev, VT_DEVICE_CREATE_TEXTURE2D,
            (*const Texture2dDesc, *const SubresourceData, *mut *mut c_void) -> Hresult,
            desc, std::ptr::null(), &mut tex)
    };
    if hr < 0 || tex.is_null() {
        return Err(failed(
            hr,
            &format!("CreateTexture2D ({what}, {}x{})", desc.Width, desc.Height),
        ));
    }
    Ok(Com(tex))
}

/// A view of `resource` made by the device method `slot` (render target,
/// shader resource or depth-stencil view), with the default description.
unsafe fn view(dev: *mut c_void, slot: usize, resource: *mut c_void) -> Result<Com, GpuError> {
    let mut v = std::ptr::null_mut();
    let hr = unsafe {
        com!(dev, slot, (*mut c_void, *const c_void, *mut *mut c_void) -> Hresult,
            resource, std::ptr::null(), &mut v)
    };
    if hr < 0 || v.is_null() {
        return Err(failed(hr, "creating a view"));
    }
    Ok(Com(v))
}

impl Stencil {
    unsafe fn new(dev: *mut c_void, w: i32, h: i32) -> Result<Self, GpuError> {
        unsafe {
            let desc = Texture2dDesc::new(
                w as u32,
                h as u32,
                DXGI_FORMAT_D24_UNORM_S8_UINT,
                D3D11_USAGE_DEFAULT,
                D3D11_BIND_DEPTH_STENCIL,
                0,
            );
            let tex = texture2d(dev, &desc, "a stencil buffer")?;
            let dsv = view(dev, VT_DEVICE_CREATE_DSV, tex.ptr())?;
            Ok(Stencil {
                _tex: tex,
                dsv,
                w,
                h,
            })
        }
    }
}

impl Target {
    unsafe fn new(dev: *mut c_void, w: i32, h: i32, format: u32) -> Result<Self, GpuError> {
        unsafe {
            let desc = Texture2dDesc::new(
                w as u32,
                h as u32,
                format,
                D3D11_USAGE_DEFAULT,
                D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE,
                0,
            );
            let color = texture2d(dev, &desc, "a render target")?;
            let rtv = view(dev, VT_DEVICE_CREATE_RTV, color.ptr())?;
            let srv = view(dev, VT_DEVICE_CREATE_SRV, color.ptr())?;
            let stencil = Stencil::new(dev, w, h)?;
            Ok(Target {
                color,
                rtv,
                srv,
                stencil,
                w,
                h,
            })
        }
    }
}

/// A depth-stencil state testing `(compare, pass operation)` on front and
/// back faces with `read` and `write` masks; no depth.
unsafe fn depth_stencil(
    dev: *mut c_void,
    front: (u32, u32),
    back: (u32, u32),
    read: u8,
    write: u8,
) -> Result<Com, GpuError> {
    let face = |(func, pass): (u32, u32)| StencilOpDesc {
        StencilFailOp: D3D11_STENCIL_OP_KEEP,
        StencilDepthFailOp: D3D11_STENCIL_OP_KEEP,
        StencilPassOp: pass,
        StencilFunc: func,
    };
    let desc = DepthStencilDesc {
        DepthEnable: 0,
        DepthWriteMask: D3D11_DEPTH_WRITE_MASK_ZERO,
        DepthFunc: D3D11_COMPARISON_ALWAYS,
        StencilEnable: 1,
        StencilReadMask: read,
        StencilWriteMask: write,
        FrontFace: face(front),
        BackFace: face(back),
    };
    let mut state = std::ptr::null_mut();
    let hr = unsafe {
        com!(dev, VT_DEVICE_CREATE_DEPTH_STENCIL_STATE,
            (*const DepthStencilDesc, *mut *mut c_void) -> Hresult, &desc, &mut state)
    };
    if hr < 0 || state.is_null() {
        return Err(failed(hr, "CreateDepthStencilState"));
    }
    Ok(Com(state))
}

impl Executor for D3dExecutor {
    fn api(&self) -> Api {
        Api::D3d11
    }

    fn read_back(&mut self, plan: &Plan) -> Result<Pixmap, GpuError> {
        let mut data = unsafe { self.offscreen(plan) }?;
        bgra_premul_to_rgba(&mut data);
        Ok(Pixmap::from_rgba8(
            PhysicalSize::new(plan.size.width, plan.size.height),
            data,
        ))
    }

    fn can_present(&self, layer: &GpuLayer) -> bool {
        matches!(layer, GpuLayer::Hwnd(h) if !h.is_null())
    }

    fn present(
        &mut self,
        plan: &mut Plan,
        layer: &GpuLayer,
        _scale: ScaleFactor,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        match *layer {
            GpuLayer::Hwnd(hwnd) if !hwnd.is_null() => unsafe {
                self.present_to(plan, hwnd, dirty)
            },
            _ => Err(GpuError::Unavailable("no window to present to".into())),
        }
    }

    fn uploads(&self) -> usize {
        self.uploads
    }

    fn draws(&self) -> usize {
        self.draws
    }

    fn texture_bytes(&self) -> usize {
        self.textures.values().map(|t| t.bytes).sum::<usize>()
            + self
                .atlas
                .values()
                .map(|a| a.side as usize * a.side as usize * 4)
                .sum::<usize>()
    }

    fn device(&self) -> String {
        self.device_name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The HLSL compiles and every state is made, on the hardware device or
    /// WARP (which every Windows has). The scenes themselves are compared
    /// with the software renderer in `gpu::compare`.
    #[test]
    fn the_executor_sets_up() {
        match D3dExecutor::new() {
            Ok(e) => assert_eq!(e.api(), Api::D3d11),
            Err(e) if std::env::var_os("STIPPLE_REQUIRE_GPU").is_some() => {
                panic!("no Direct3D 11 executor: {e}")
            }
            Err(e) => eprintln!("skipping: no Direct3D 11 executor: {e}"),
        }
    }
}
