//! The Metal scene executor: draws a lowered [`Plan`] with Metal, offscreen
//! (and read back) or into a window's `CAMetalLayer` (and presented).
//!
//! It is the GLES2 executor (`gl/scene.rs`) translated: the same shaders in
//! MSL, compiled when the executor is created (`newLibraryWithSource:`, no
//! offline toolchain or precompiled library), the same premultiplied blending,
//! stencil-and-cover paths at 4×4 supersampling into a scratch target, layers
//! in pooled offscreen targets, scissor and stencil clips. Every target is a
//! `BGRA8Unorm` color texture with a `Stencil8` one beside it.
//!
//! Colors are blended as the software renderer blends them, on the encoded
//! sRGB values, and the window's layer is tagged sRGB so the window server
//! matches them to the display as it does the CPU frames.
//!
//! A frame is one command buffer; each change of target (a layer pushed or
//! popped, a path's scratch pass) is a new render pass. Vertices go into
//! shared buffers the command buffer keeps alive; images are textures cached
//! by the image's identity, like the GL backend's, and each glyph atlas page
//! is a texture brought up to date at the start of the frame by a blit of
//! what changed in it. Each batch of boxes and textured quads
//! (`super::batch`) is one draw call. Raw `objc_msgSend` FFI, as in the rest
//! of the macOS and iOS code.
//!
//! A window's frames are drawn into a target of the executor's own that
//! persists from frame to frame, then copied into the layer's drawable and
//! presented: a `CAMetalLayer` has no partial present, and its drawables'
//! contents aren't kept, so the kept target is what lets a frame redraw only
//! what changed since the one before ([`Plan::limit`]).

#![allow(unsafe_code)]

use super::atlas::{AtlasPage, Upload};
use super::batch::{self, Cmd, TexRef};
use super::canvas::rekey;
use super::ops::{Clip, Op, Plan, Resample};
use super::renderer::{Api, Executor, GpuError, KeptFrame};
use crate::geometry::{PhysicalSize, ScaleFactor};
use crate::platform::GpuLayer;
use crate::render::Pixmap;
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::{Arc, Weak};

type Id = *mut c_void;
type Sel = *mut c_void;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(ctx: *mut c_void);
}

#[link(name = "Metal", kind = "framework")]
unsafe extern "C" {
    fn MTLCreateSystemDefaultDevice() -> Id;
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: *const c_void;
    fn CGColorSpaceCreateWithName(name: *const c_void) -> *mut c_void;
    fn CGColorSpaceRelease(space: *mut c_void);
}

fn sel(name: &CStr) -> Sel {
    unsafe { sel_registerName(name.as_ptr()) }
}

fn class(name: &CStr) -> Id {
    unsafe { objc_getClass(name.as_ptr()) }
}

/// `objc_msgSend` cast to the method's signature:
/// `send!(Ret; receiver, c"selector:", arg; ArgType, ...)`.
macro_rules! send {
    ($ret:ty; $obj:expr, $sel:expr $(, $arg:expr; $ty:ty)*) => {{
        let f: unsafe extern "C" fn(Id, Sel $(, $ty)*) -> $ret =
            std::mem::transmute(objc_msgSend as *const c_void);
        f($obj, sel($sel) $(, $arg)*)
    }};
}

// External images (`IOSurface`s) bound as textures; after `send!`, which
// it uses.
mod external;

// Metal's enum values.
const PIXEL_R8: u64 = 10;
const PIXEL_RGBA8: u64 = 70;
const PIXEL_BGRA8: u64 = 80;
const PIXEL_STENCIL8: u64 = 253;
const USAGE_SHADER_READ: u64 = 1;
const USAGE_RENDER_TARGET: u64 = 4;
const STORAGE_PRIVATE: u64 = 2;
const LOAD: u64 = 1;
const CLEAR: u64 = 2;
const STORE: u64 = 1;
const TRIANGLE: u64 = 3;
const BLEND_ONE: u64 = 1;
const BLEND_ONE_MINUS_SRC_ALPHA: u64 = 5;
const COMPARE_EQUAL: u64 = 2;
const COMPARE_NOT_EQUAL: u64 = 5;
const COMPARE_ALWAYS: u64 = 7;
const KEEP: u64 = 0;
const INCR_CLAMP: u64 = 3;
const DECR_CLAMP: u64 = 4;
const INVERT: u64 = 5;
const INCR_WRAP: u64 = 6;
const DECR_WRAP: u64 = 7;
const STATUS_ERROR: u64 = 5;
/// The largest texture side every Metal GPU takes.
const MAX_SIZE: i32 = 16384;
/// Vertex buffers are allocated at least this big.
const ARENA: usize = 256 * 1024;
/// Textures of images not drawn for this many frames are freed.
const TEXTURE_KEEP_FRAMES: u64 = 120;

#[repr(C)]
#[derive(Clone, Copy)]
struct ClearColor {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ScissorRect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Size3 {
    w: usize,
    h: usize,
    d: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Region {
    origin: Size3,
    size: Size3,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq)]
struct CgSize {
    w: f64,
    h: f64,
}

/// The shaders, compiled when the executor is created. Each matches its GLSL
/// twin in `gl/scene.rs`.
const MSL: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct VOut { float4 pos [[position]]; float2 aux; };

// Every program shares one vertex shader: `xy` in device pixels mapped to
// clip space by `xf` (scale.xy, offset.zw), and a pass-through `aux`.
vertex VOut vs(uint vid [[vertex_id]], const device float4* v [[buffer(0)]],
               constant float4& xf [[buffer(1)]]) {
    float4 d = v[vid];
    VOut o;
    o.pos = float4(d.xy * xf.xy + xf.zw, 0.0, 1.0);
    o.aux = d.zw;
    return o;
}

// Batched boxes and textured quads (`batch.rs`): every input is a vertex
// attribute, `a0`..`a4`, so a run of them is one draw.
struct QV { float4 a0; float4 a1; float4 a2; float4 a3; float4 a4; };
struct QOut { float4 pos [[position]]; float2 p; float4 a1; float4 a2; float4 a3; float mode; };
vertex QOut quad_vs(uint vid [[vertex_id]], const device QV* v [[buffer(0)]],
                    constant float4& xf [[buffer(1)]]) {
    QV d = v[vid];
    QOut o;
    o.pos = float4(d.a0.xy * xf.xy + xf.zw, 0.0, 1.0);
    o.p = d.a0.zw;
    o.a1 = d.a1;
    o.a2 = d.a2;
    o.a3 = d.a3;
    o.mode = d.a4.x;
    return o;
}

// A box: `p` is the device position, `a1` the rect, `a2` the color, `a3`
// the radius and stroke. Coverage is the exact overlap of the pixel with a
// sharp box (or ring), or a signed distance for rounded corners.
static float span(float p, float h) { return max(min(h, p + 0.5) - max(-h, p - 0.5), 0.0); }
static float boxcov(float2 p, float2 h) { h = max(h, float2(0.0)); return span(p.x, h.x) * span(p.y, h.y); }
static float sdf(float2 p, float2 h, float r) {
    float2 d = abs(p) - (h - float2(r));
    return length(max(d, float2(0.0))) + min(max(d.x, d.y), 0.0) - r;
}
static float4 quad_box(QOut in) {
    float4 rect = in.a1;
    float radius = in.a3.x;
    float stroke = in.a3.y;
    float2 c = (rect.xy + rect.zw) * 0.5;
    float2 h = (rect.zw - rect.xy) * 0.5;
    float2 p = in.p - c;
    float w = stroke * 0.5;
    float cov;
    if (radius <= 0.0) {
        cov = stroke > 0.0 ? boxcov(p, h + float2(w)) - boxcov(p, h - float2(w)) : boxcov(p, h);
    } else {
        float r = min(radius, min(h.x, h.y));
        float d = sdf(p, h, r);
        cov = stroke > 0.0 ? clamp(0.5 - (d - w), 0.0, 1.0) - clamp(0.5 - (d + w), 0.0, 1.0)
                           : clamp(0.5 - d, 0.0, 1.0);
    }
    return in.a2 * cov;
}

// Boxes only (a batch without a texture): textured quads are left out.
fragment float4 quadbox_fs(QOut in [[stage_in]]) {
    return in.mode < 0.5 ? quad_box(in) : float4(0.0);
}

// Boxes and textured quads: `p` is the texture coordinate, kept within
// `a2`; `a1` the distances to the edges in device pixels (for their
// coverage); `a3` the tint times the opacity (premultiplied).
fragment float4 quad_fs(QOut in [[stage_in]], texture2d<float> t [[texture(0)]],
                        constant float& nearest [[buffer(0)]]) {
    if (in.mode < 0.5) { return quad_box(in); }
    constexpr sampler lin(filter::linear, address::clamp_to_edge);
    constexpr sampler near(filter::nearest, address::clamp_to_edge);
    float2 uv = clamp(in.p, in.a2.xy, in.a2.zw);
    float4 c = nearest > 0.5 ? t.sample(near, uv) : t.sample(lin, uv);
    if (in.mode < 1.5) { c.rgb *= c.a; }
    c *= in.a3;
    float cov = clamp(min(in.a1.x, in.a1.z) + 0.5, 0.0, 1.0)
              * clamp(min(in.a1.y, in.a1.w) + 0.5, 0.0, 1.0);
    return c * cov;
}

// A flat color: the stencil and cover passes, and stencil clips.
fragment float4 flat_fs(constant float4& color [[buffer(0)]]) { return color; }

// Resolving a supersampled path coverage: `aux` is the position in scratch
// texels; four bilinear taps average the pixel's ss×ss block.
struct ResolveU { float4 color; float2 inv; float tap; };
fragment float4 resolve_fs(VOut in [[stage_in]], texture2d<float> t [[texture(0)]],
                           constant ResolveU& u [[buffer(0)]]) {
    constexpr sampler lin(filter::linear, address::clamp_to_edge);
    float c = t.sample(lin, (in.aux + float2(-u.tap, -u.tap)) * u.inv).r
            + t.sample(lin, (in.aux + float2( u.tap, -u.tap)) * u.inv).r
            + t.sample(lin, (in.aux + float2(-u.tap,  u.tap)) * u.inv).r
            + t.sample(lin, (in.aux + float2( u.tap,  u.tap)) * u.inv).r;
    return u.color * (c * 0.25);
}
"#;

#[repr(C)]
struct ResolveU {
    color: [f32; 4],
    inv: [f32; 2],
    tap: f32,
    _pad: f32,
}

/// A color texture with a stencil texture beside it, owning both.
struct Target {
    color: Id,
    stencil: Id,
    w: i32,
    h: i32,
}

impl Target {
    unsafe fn new(dev: Id, w: i32, h: i32, format: u64) -> Result<Self, GpuError> {
        unsafe {
            let color = texture(dev, format, w, h, USAGE_RENDER_TARGET | USAGE_SHADER_READ)?;
            let stencil = match texture(dev, PIXEL_STENCIL8, w, h, USAGE_RENDER_TARGET) {
                Ok(s) => s,
                Err(e) => {
                    release(color);
                    return Err(e);
                }
            };
            Ok(Target {
                color,
                stencil,
                w,
                h,
            })
        }
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        unsafe {
            release(self.color);
            release(self.stencil);
        }
    }
}

/// A private texture for the GPU's own use.
unsafe fn texture(dev: Id, format: u64, w: i32, h: i32, usage: u64) -> Result<Id, GpuError> {
    unsafe {
        let desc = send!(Id; class(c"MTLTextureDescriptor"),
            c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:",
            format; u64, w as u64; u64, h as u64; u64, false; bool);
        send!((); desc, c"setUsage:", usage; u64);
        send!((); desc, c"setStorageMode:", STORAGE_PRIVATE; u64);
        let tex = send!(Id; dev, c"newTextureWithDescriptor:", desc; Id);
        if tex.is_null() {
            return Err(GpuError::Lost(format!("no {w}x{h} texture")));
        }
        Ok(tex)
    }
}

unsafe fn release(obj: Id) {
    if !obj.is_null() {
        unsafe { send!((); obj, c"release") };
    }
}

/// An `NSError`'s description.
unsafe fn describe(err: Id) -> String {
    if err.is_null() {
        return "unknown error".into();
    }
    unsafe {
        let s = send!(Id; err, c"localizedDescription");
        let c = send!(*const c_char; s, c"UTF8String");
        if c.is_null() {
            "unknown error".into()
        } else {
            CStr::from_ptr(c).to_string_lossy().into_owned()
        }
    }
}

fn nsstring(s: &CStr) -> Id {
    unsafe { send!(Id; class(c"NSString"), c"stringWithUTF8String:", s.as_ptr(); *const c_char) }
}

/// A cached image texture.
struct Texture {
    image: Weak<Pixmap>,
    tex: Id,
    used: u64,
    bytes: usize,
}

/// An atlas page's texture, holding `version` of it.
struct AtlasTex {
    tex: Id,
    side: u32,
    version: u64,
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

/// What an encoder draws into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum On {
    Frame,
    Layer(usize),
    Scratch,
}

/// One frame being encoded.
struct Frame {
    cmd: Id,
    /// The open render encoder, if any, and what it draws into.
    enc: Id,
    on: Option<On>,
    /// The frame's target: an offscreen texture or a drawable's.
    color: Id,
    stencil: Id,
    w: i32,
    h: i32,
    /// The frame target's stencil is yet to be cleared (by its first pass).
    fresh: bool,
    /// The vertex buffer being filled.
    buf: Id,
    ptr: *mut u8,
    cap: usize,
    used: usize,
}

impl Frame {
    unsafe fn end(&mut self) {
        if !self.enc.is_null() {
            unsafe { send!((); self.enc, c"endEncoding") };
            self.enc = std::ptr::null_mut();
            self.on = None;
        }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        unsafe {
            self.end();
            release(self.buf);
        }
    }
}

/// The Metal executor: device, queue, pipelines, targets and caches.
pub(crate) struct MetalExecutor {
    device: Id,
    queue: Id,
    srgb: *mut c_void,
    /// Batched quads: boxes only, and boxes with textured quads.
    quad_boxes: Id,
    quads: Id,
    /// Textured quads sampling NV12 (an external image's two planes).
    quads_yuv: Id,
    /// External images bound as textures (`external`).
    externals: external::Imports,
    resolve: Id,
    /// Stencil-only on the frame and layers (clip polygons).
    clip: Id,
    /// Stencil-only and cover on the scratch target (paths).
    wind: Id,
    cover: Id,
    ds_test: Id,
    ds_clip_inc: Id,
    ds_clip_dec: Id,
    ds_nonzero: Id,
    ds_evenodd: Id,
    ds_cover_nz: Id,
    ds_cover_eo: Id,
    /// The frame target (read-backs, and windows' frames before they're
    /// copied into the drawable), layers, the path scratch.
    frame: Option<Target>,
    /// Which window's frame `frame` holds.
    kept: KeptFrame,
    layers: Vec<Target>,
    scratch: Option<Target>,
    textures: HashMap<usize, Texture>,
    /// The atlas pages' textures, by page id.
    atlas: HashMap<u64, AtlasTex>,
    /// The quad vertices of the frame being encoded (layer composites).
    scratch_quads: Vec<f32>,
    /// The last presented frame's command buffer (retained), to see whether
    /// the device failed it.
    last: Id,
    count: u64,
    uploads: usize,
    draws: usize,
}

impl Drop for MetalExecutor {
    fn drop(&mut self) {
        self.frame = None;
        self.layers.clear();
        self.scratch = None;
        unsafe {
            for t in self.textures.values() {
                release(t.tex);
            }
            for t in self.atlas.values() {
                release(t.tex);
            }
            self.externals.clear();
            for obj in [
                self.last,
                self.quad_boxes,
                self.quads,
                self.quads_yuv,
                self.resolve,
                self.clip,
                self.wind,
                self.cover,
                self.ds_test,
                self.ds_clip_inc,
                self.ds_clip_dec,
                self.ds_nonzero,
                self.ds_evenodd,
                self.ds_cover_nz,
                self.ds_cover_eo,
                self.queue,
                self.device,
            ] {
                release(obj);
            }
            if !self.srgb.is_null() {
                CGColorSpaceRelease(self.srgb);
            }
        }
    }
}

/// Run `f` inside an autorelease pool.
fn pooled<T>(f: impl FnOnce() -> T) -> T {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let r = f();
        objc_autoreleasePoolPop(pool);
        r
    }
}

impl MetalExecutor {
    /// Set up on the system's default device, compiling the shaders.
    pub(crate) fn new() -> Result<Self, GpuError> {
        pooled(|| unsafe { Self::create() })
    }

    unsafe fn create() -> Result<Self, GpuError> {
        unsafe {
            let device = MTLCreateSystemDefaultDevice();
            if device.is_null() {
                return Err(GpuError::Unavailable("no Metal device".into()));
            }
            let queue = send!(Id; device, c"newCommandQueue");
            let mut me = MetalExecutor {
                device,
                queue,
                srgb: CGColorSpaceCreateWithName(kCGColorSpaceSRGB),
                quad_boxes: std::ptr::null_mut(),
                quads: std::ptr::null_mut(),
                quads_yuv: std::ptr::null_mut(),
                externals: external::Imports::default(),
                resolve: std::ptr::null_mut(),
                clip: std::ptr::null_mut(),
                wind: std::ptr::null_mut(),
                cover: std::ptr::null_mut(),
                ds_test: std::ptr::null_mut(),
                ds_clip_inc: std::ptr::null_mut(),
                ds_clip_dec: std::ptr::null_mut(),
                ds_nonzero: std::ptr::null_mut(),
                ds_evenodd: std::ptr::null_mut(),
                ds_cover_nz: std::ptr::null_mut(),
                ds_cover_eo: std::ptr::null_mut(),
                frame: None,
                kept: KeptFrame::default(),
                layers: Vec::new(),
                scratch: None,
                textures: HashMap::new(),
                atlas: HashMap::new(),
                scratch_quads: Vec::new(),
                last: std::ptr::null_mut(),
                count: 0,
                uploads: 0,
                draws: 0,
            };
            if queue.is_null() {
                return Err(GpuError::Unavailable("no Metal command queue".into()));
            }
            // With the NV12 shader; without it should it not compile (the
            // camera's frames are then converted on the CPU).
            let compile = |src: String, err: &mut Id| {
                let src = std::ffi::CString::new(src).unwrap();
                send!(Id; device, c"newLibraryWithSource:options:error:",
                    nsstring(&src); Id, std::ptr::null_mut(); Id, err; *mut Id)
            };
            let mut err: Id = std::ptr::null_mut();
            let mut lib = compile(format!("{MSL}{}", external::MSL_YUV), &mut err);
            if lib.is_null() {
                lib = compile(MSL.to_string(), &mut err);
            }
            if lib.is_null() {
                return Err(GpuError::Unavailable(format!(
                    "the Metal shaders didn't compile: {}",
                    describe(err)
                )));
            }
            let func = |name: &CStr| send!(Id; lib, c"newFunctionWithName:", nsstring(name); Id);
            let vs = func(c"vs");
            let quad_vs = func(c"quad_vs");
            let fns = [
                func(c"quadbox_fs"),
                func(c"quad_fs"),
                func(c"flat_fs"),
                func(c"resolve_fs"),
                func(c"quad_yuv_fs"),
            ];
            let [quadbox_fs, quad_fs, flat_fs, resolve_fs, quad_yuv_fs] = fns;
            let built = (|| -> Result<(), GpuError> {
                let dev = device;
                me.quad_boxes = pipeline(dev, quad_vs, quadbox_fs, PIXEL_BGRA8, true, true)?;
                me.quads = pipeline(dev, quad_vs, quad_fs, PIXEL_BGRA8, true, true)?;
                me.quads_yuv = if quad_yuv_fs.is_null() {
                    std::ptr::null_mut()
                } else {
                    pipeline(dev, quad_vs, quad_yuv_fs, PIXEL_BGRA8, true, true)
                        .unwrap_or(std::ptr::null_mut())
                };
                me.resolve = pipeline(dev, vs, resolve_fs, PIXEL_BGRA8, true, true)?;
                me.clip = pipeline(dev, vs, flat_fs, PIXEL_BGRA8, false, false)?;
                me.wind = pipeline(dev, vs, flat_fs, PIXEL_R8, false, false)?;
                me.cover = pipeline(dev, vs, flat_fs, PIXEL_R8, false, true)?;
                let always = |op| (COMPARE_ALWAYS, op);
                me.ds_test =
                    depth_stencil(dev, (COMPARE_EQUAL, KEEP), (COMPARE_EQUAL, KEEP), 0xff, 0);
                me.ds_clip_inc = depth_stencil(
                    dev,
                    (COMPARE_EQUAL, INCR_CLAMP),
                    (COMPARE_EQUAL, INCR_CLAMP),
                    0xff,
                    0xff,
                );
                me.ds_clip_dec = depth_stencil(
                    dev,
                    (COMPARE_EQUAL, DECR_CLAMP),
                    (COMPARE_EQUAL, DECR_CLAMP),
                    0xff,
                    0xff,
                );
                me.ds_nonzero =
                    depth_stencil(dev, always(INCR_WRAP), always(DECR_WRAP), 0xff, 0xff);
                me.ds_evenodd = depth_stencil(dev, always(INVERT), always(INVERT), 0xff, 0xff);
                let ne = (COMPARE_NOT_EQUAL, KEEP);
                me.ds_cover_nz = depth_stencil(dev, ne, ne, 0xff, 0);
                me.ds_cover_eo = depth_stencil(dev, ne, ne, 1, 0);
                Ok(())
            })();
            for f in [
                vs,
                quad_vs,
                quadbox_fs,
                quad_fs,
                flat_fs,
                resolve_fs,
                quad_yuv_fs,
                lib,
            ] {
                release(f);
            }
            built?;
            Ok(me)
        }
    }

    /// The texture for `image`, uploading it unless it is cached.
    unsafe fn texture(&mut self, image: &Arc<Pixmap>) -> Option<Id> {
        unsafe {
            let key = Arc::as_ptr(image) as usize;
            let count = self.count;
            if let Some(t) = self.textures.get_mut(&key) {
                if t.image.upgrade().is_some_and(|i| Arc::ptr_eq(&i, image)) {
                    t.used = count;
                    return Some(t.tex);
                }
                // The address was reused by another image: upload anew.
                release(t.tex);
                self.textures.remove(&key);
            }
            let size = image.size();
            let (w, h) = (size.width as usize, size.height as usize);
            if w == 0 || h == 0 || w > MAX_SIZE as usize || h > MAX_SIZE as usize {
                return None;
            }
            let desc = send!(Id; class(c"MTLTextureDescriptor"),
                c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:",
                PIXEL_RGBA8; u64, w as u64; u64, h as u64; u64, false; bool);
            send!((); desc, c"setUsage:", USAGE_SHADER_READ; u64);
            let tex = send!(Id; self.device, c"newTextureWithDescriptor:", desc; Id);
            if tex.is_null() {
                return None;
            }
            let region = Region {
                origin: Size3 { w: 0, h: 0, d: 0 },
                size: Size3 { w, h, d: 1 },
            };
            send!((); tex, c"replaceRegion:mipmapLevel:withBytes:bytesPerRow:",
                region; Region, 0usize; usize, image.as_bytes().as_ptr() as *const c_void; *const c_void,
                w * 4; usize);
            self.uploads += 1;
            self.textures.insert(
                key,
                Texture {
                    image: Arc::downgrade(image),
                    tex,
                    used: count,
                    bytes: w * h * 4,
                },
            );
            Some(tex)
        }
    }

    /// Update the textures `plan` changed in places (canvases kept as
    /// textures), by a blit at the start of `f`'s command buffer: ordered
    /// after the frames still drawing from them.
    unsafe fn update_textures(&mut self, f: &mut Frame, plan: &Plan) {
        if plan.updates.is_empty() {
            return;
        }
        unsafe {
            f.end();
            let mut blit: Id = std::ptr::null_mut();
            for u in &plan.updates {
                let Some(t) = rekey(&mut self.textures, u, |t| &mut t.image) else {
                    continue;
                };
                let tex = t.tex;
                for &r in &u.rects {
                    let bytes = u.rows(r);
                    let buf = send!(Id; self.device, c"newBufferWithBytes:length:options:",
                        bytes.as_ptr() as *const c_void; *const c_void, bytes.len(); usize, 0u64; u64);
                    if buf.is_null() {
                        continue;
                    }
                    if blit.is_null() {
                        blit = send!(Id; f.cmd, c"blitCommandEncoder");
                    }
                    let size = Size3 {
                        w: r[2] as usize,
                        h: r[3] as usize,
                        d: 1,
                    };
                    let origin = Size3 {
                        w: r[0] as usize,
                        h: r[1] as usize,
                        d: 0,
                    };
                    send!((); blit,
                        c"copyFromBuffer:sourceOffset:sourceBytesPerRow:sourceBytesPerImage:sourceSize:toTexture:destinationSlice:destinationLevel:destinationOrigin:",
                        buf; Id, 0usize; usize, r[2] as usize * 4; usize, bytes.len(); usize,
                        size; Size3, tex; Id, 0usize; usize, 0usize; usize, origin; Size3);
                    release(buf);
                }
            }
            if !blit.is_null() {
                send!((); blit, c"endEncoding");
            }
        }
    }

    /// Free textures whose image is gone or hasn't been drawn lately.
    fn sweep_textures(&mut self) {
        let count = self.count;
        self.textures.retain(|_, t| {
            let keep =
                t.image.strong_count() > 0 && count.saturating_sub(t.used) < TEXTURE_KEEP_FRAMES;
            if !keep {
                unsafe { release(t.tex) };
            }
            keep
        });
    }

    /// Bring the atlas pages' textures up to date with `pages`, by blits
    /// in `f`'s command buffer (so a frame still drawing from a texture
    /// finishes first), and drop the textures of pages gone.
    unsafe fn sync_atlas(&mut self, f: &Frame, pages: &[AtlasPage]) -> Result<(), GpuError> {
        unsafe {
            self.atlas.retain(|id, t| {
                let keep = pages.iter().any(|p| p.id == *id);
                if !keep {
                    release(t.tex);
                }
                keep
            });
            let mut blit: Id = std::ptr::null_mut();
            for p in pages {
                let have = self.atlas.get(&p.id).map(|t| (t.side, t.version));
                let region = match p.upload(have) {
                    Upload::Nothing => continue,
                    Upload::Whole => {
                        if let Some(old) = self.atlas.remove(&p.id) {
                            release(old.tex);
                        }
                        let side = p.side as usize;
                        let desc = send!(Id; class(c"MTLTextureDescriptor"),
                            c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:",
                            PIXEL_RGBA8; u64, side as u64; u64, side as u64; u64, false; bool);
                        send!((); desc, c"setUsage:", USAGE_SHADER_READ; u64);
                        send!((); desc, c"setStorageMode:", STORAGE_PRIVATE; u64);
                        let tex = send!(Id; self.device, c"newTextureWithDescriptor:", desc; Id);
                        if tex.is_null() {
                            return Err(GpuError::Lost("no atlas texture".into()));
                        }
                        self.atlas.insert(
                            p.id,
                            AtlasTex {
                                tex,
                                side: p.side,
                                version: p.version,
                            },
                        );
                        [0, 0, p.side, p.side]
                    }
                    Upload::Region(r) => r,
                };
                let t = self.atlas.get_mut(&p.id).unwrap();
                t.version = p.version;
                let bytes = p.region(region);
                let buf = send!(Id; self.device, c"newBufferWithBytes:length:options:",
                    bytes.as_ptr() as *const c_void; *const c_void, bytes.len(); usize, 0u64; u64);
                if buf.is_null() {
                    return Err(GpuError::Lost("no atlas upload buffer".into()));
                }
                if blit.is_null() {
                    blit = send!(Id; f.cmd, c"blitCommandEncoder");
                }
                let (w, h) = (
                    (region[2] - region[0]) as usize,
                    (region[3] - region[1]) as usize,
                );
                let size = Size3 { w, h, d: 1 };
                let origin = Size3 {
                    w: region[0] as usize,
                    h: region[1] as usize,
                    d: 0,
                };
                send!((); blit,
                    c"copyFromBuffer:sourceOffset:sourceBytesPerRow:sourceBytesPerImage:sourceSize:toTexture:destinationSlice:destinationLevel:destinationOrigin:",
                    buf; Id, 0usize; usize, w * 4; usize, w * h * 4; usize, size; Size3,
                    t.tex; Id, 0usize; usize, 0usize; usize, origin; Size3);
                // The command buffer keeps it until it has run.
                release(buf);
                self.uploads += 1;
            }
            if !blit.is_null() {
                send!((); blit, c"endEncoding");
            }
            Ok(())
        }
    }

    fn target_of(&self, f: &Frame, on: On) -> (Id, Id) {
        match on {
            On::Frame => (f.color, f.stencil),
            On::Layer(i) => (self.layers[i].color, self.layers[i].stencil),
            On::Scratch => {
                let s = self.scratch.as_ref().unwrap();
                (s.color, s.stencil)
            }
        }
    }

    /// Start a render pass on `on`, cleared to `clear` (premultiplied) and a
    /// zero stencil, or keeping what it holds.
    unsafe fn open(&self, f: &mut Frame, on: On, clear: Option<[f32; 4]>) {
        unsafe {
            f.end();
            let (color, stencil) = self.target_of(f, on);
            let rpd = send!(Id; class(c"MTLRenderPassDescriptor"), c"renderPassDescriptor");
            let atts = send!(Id; rpd, c"colorAttachments");
            let a0 = send!(Id; atts, c"objectAtIndexedSubscript:", 0usize; usize);
            send!((); a0, c"setTexture:", color; Id);
            send!((); a0, c"setLoadAction:", if clear.is_some() { CLEAR } else { LOAD }; u64);
            send!((); a0, c"setStoreAction:", STORE; u64);
            if let Some(c) = clear {
                let cc = ClearColor {
                    r: c[0] as f64,
                    g: c[1] as f64,
                    b: c[2] as f64,
                    a: c[3] as f64,
                };
                send!((); a0, c"setClearColor:", cc; ClearColor);
            }
            let sa = send!(Id; rpd, c"stencilAttachment");
            send!((); sa, c"setTexture:", stencil; Id);
            // The frame's stencil starts cleared even when its colors are
            // kept (a partial frame).
            let fresh = on == On::Frame && std::mem::take(&mut f.fresh);
            send!((); sa, c"setLoadAction:", if clear.is_some() || fresh { CLEAR } else { LOAD }; u64);
            send!((); sa, c"setStoreAction:", STORE; u64);
            send!((); sa, c"setClearStencil:", 0u32; u32);
            f.enc = send!(Id; f.cmd, c"renderCommandEncoderWithDescriptor:", rpd; Id);
            f.on = Some(on);
        }
    }

    /// Get ready to draw into `level` with `pipe`, `ds` and stencil value
    /// `reference`. `false` when its scissor is empty (draw nothing).
    unsafe fn bind(&self, f: &mut Frame, level: &Level, pipe: Id, ds: Id, reference: u32) -> bool {
        let s = level.scissor;
        if s[2] <= s[0] || s[3] <= s[1] {
            return false;
        }
        unsafe {
            if f.on != Some(level.on()) || f.enc.is_null() {
                self.open(f, level.on(), None);
            }
            self.state(f, pipe, ds, reference, s);
        }
        true
    }

    unsafe fn state(&self, f: &Frame, pipe: Id, ds: Id, reference: u32, s: [i32; 4]) {
        unsafe {
            send!((); f.enc, c"setRenderPipelineState:", pipe; Id);
            send!((); f.enc, c"setDepthStencilState:", ds; Id);
            send!((); f.enc, c"setStencilReferenceValue:", reference; u32);
            let r = ScissorRect {
                x: s[0] as usize,
                y: s[1] as usize,
                w: (s[2] - s[0]) as usize,
                h: (s[3] - s[1]) as usize,
            };
            send!((); f.enc, c"setScissorRect:", r; ScissorRect);
        }
    }

    /// Hand `verts` (`[x, y, aux_x, aux_y]` each) and the device-to-clip
    /// mapping `xf` to the vertex shader.
    unsafe fn vertices(&self, f: &mut Frame, verts: &[f32], xf: [f32; 4]) -> Result<(), GpuError> {
        unsafe {
            let bytes = std::mem::size_of_val(verts);
            if f.used + bytes > f.cap {
                release(f.buf);
                let cap = ARENA.max(bytes);
                f.buf =
                    send!(Id; self.device, c"newBufferWithLength:options:", cap; usize, 0u64; u64);
                if f.buf.is_null() {
                    f.cap = 0;
                    return Err(GpuError::Lost("no vertex buffer".into()));
                }
                f.ptr = send!(*mut c_void; f.buf, c"contents") as *mut u8;
                f.cap = cap;
                f.used = 0;
            }
            std::ptr::copy_nonoverlapping(verts.as_ptr() as *const u8, f.ptr.add(f.used), bytes);
            send!((); f.enc, c"setVertexBuffer:offset:atIndex:", f.buf; Id, f.used; usize, 0usize; usize);
            send!((); f.enc, c"setVertexBytes:length:atIndex:",
                xf.as_ptr() as *const c_void; *const c_void, 16usize; usize, 1usize; usize);
            f.used += bytes.next_multiple_of(16);
            Ok(())
        }
    }

    unsafe fn uniforms<T>(f: &Frame, u: &T) {
        unsafe {
            send!((); f.enc, c"setFragmentBytes:length:atIndex:",
                u as *const T as *const c_void; *const c_void, std::mem::size_of::<T>(); usize, 0usize; usize);
        }
    }

    unsafe fn draw(&mut self, f: &Frame, count: usize) {
        self.draws += 1;
        unsafe {
            send!((); f.enc, c"drawPrimitives:vertexStart:vertexCount:",
                TRIANGLE; u64, 0u64; u64, count as u64; u64);
        }
    }

    /// Device pixels of a `w`×`h` target to clip space.
    fn xf(w: i32, h: i32) -> [f32; 4] {
        [2.0 / w as f32, -2.0 / h as f32, -1.0, 1.0]
    }

    /// Draw `count` quad vertices from `first` of `quads` (see
    /// `super::batch`) in one call: sampling `tex` (nearest-neighbor or
    /// bilinear), or boxes only without one; with `chroma`, `tex` is NV12's
    /// luma and `chroma` its chroma, converted in that color space.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_quads(
        &mut self,
        f: &mut Frame,
        level: &Level,
        quads: &[f32],
        first: u32,
        count: u32,
        tex: Option<Id>,
        chroma: Option<(Id, crate::render::external::YuvColorSpace)>,
        nearest: bool,
    ) -> Result<(), GpuError> {
        unsafe {
            let pipe = match (tex, chroma) {
                (Some(_), Some(_)) => self.quads_yuv,
                (Some(_), None) => self.quads,
                _ => self.quad_boxes,
            };
            if count == 0 || !self.bind(f, level, pipe, self.ds_test, level.depth) {
                return Ok(());
            }
            let n = batch::QUAD_FLOATS;
            let verts = &quads[first as usize * n..(first + count) as usize * n];
            self.vertices(f, verts, Self::xf(f.w, f.h))?;
            if let Some(tex) = tex {
                send!((); f.enc, c"setFragmentTexture:atIndex:", tex; Id, 0usize; usize);
                if let Some((uv, color)) = chroma {
                    send!((); f.enc, c"setFragmentTexture:atIndex:", uv; Id, 1usize; usize);
                    Self::uniforms(f, &external::yuv_uniforms(nearest, color));
                } else {
                    let flag: f32 = if nearest { 1.0 } else { 0.0 };
                    Self::uniforms(f, &flag);
                }
            }
            self.draw(f, count as usize);
            Ok(())
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
            let ss = [4, 2, 1]
                .into_iter()
                .find(|ss| bw * ss <= MAX_SIZE && bh * ss <= MAX_SIZE)
                .unwrap_or(1);
            let (sw, sh) = (bw * ss, bh * ss);
            if self.scratch.as_ref().is_none_or(|t| t.w < sw || t.h < sh) {
                let (gw, gh) = self.scratch.as_ref().map_or((0, 0), |t| (t.w, t.h));
                let grow = |want: i32, have: i32| {
                    (want.max(have) as u32)
                        .next_power_of_two()
                        .clamp(64, MAX_SIZE as u32) as i32
                };
                // The old one may still be in use by this frame: the command
                // buffer keeps it alive.
                self.scratch = None;
                self.scratch = Some(Target::new(
                    self.device,
                    grow(sw, gw),
                    grow(sh, gh),
                    PIXEL_R8,
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
                self.ds_evenodd
            } else {
                self.ds_nonzero
            };
            self.state(f, self.wind, ds, 0, all);
            let fans = fan_triangles(contours.iter().map(|c| c.as_slice()));
            if !fans.is_empty() {
                self.vertices(f, &fans, xf)?;
                Self::uniforms(f, &white);
                self.draw(f, fans.len() / 4);
            }

            // Cover: full coverage wherever the winding says inside.
            let ds = if even_odd {
                self.ds_cover_eo
            } else {
                self.ds_cover_nz
            };
            self.state(f, self.cover, ds, 0, all);
            let (x0, y0, x1, y1) = (b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32);
            #[rustfmt::skip]
            let quad = [
                x0, y0, 0.0, 0.0,  x1, y0, 0.0, 0.0,  x0, y1, 0.0, 0.0,
                x0, y1, 0.0, 0.0,  x1, y0, 0.0, 0.0,  x1, y1, 0.0, 0.0,
            ];
            self.vertices(f, &quad, xf)?;
            Self::uniforms(f, &white);
            self.draw(f, 6);

            // Resolve into the level.
            if !self.bind(f, level, self.resolve, self.ds_test, level.depth) {
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
            self.vertices(f, &v, Self::xf(f.w, f.h))?;
            let scratch = self.scratch.as_ref().unwrap().color;
            send!((); f.enc, c"setFragmentTexture:atIndex:", scratch; Id, 0usize; usize);
            Self::uniforms(
                f,
                &ResolveU {
                    color,
                    inv: [1.0 / vw, 1.0 / vh],
                    tap: if ss > 1 { fs * 0.25 } else { 0.0 },
                    _pad: 0.0,
                },
            );
            self.draw(f, 6);
            Ok(())
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
        ds: Id,
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
            if !self.bind(f, &full, self.clip, ds, from) {
                return Ok(());
            }
            let fans = fan_triangles(std::iter::once(pts));
            if !fans.is_empty() {
                self.vertices(f, &fans, Self::xf(w, h))?;
                Self::uniforms(f, &[0.0f32; 4]);
                self.draw(f, fans.len() / 4);
            }
            Ok(())
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
            let tex = self.layers[layer.layer.unwrap()].color;
            let mut v = std::mem::take(&mut self.scratch_quads);
            v.clear();
            let (w, h) = (f.w as u32, f.h as u32);
            let nearest = batch::layer_quad(&mut v, layer.opacity, layer.resample.as_ref(), w, h);
            let count = (v.len() / batch::QUAD_FLOATS) as u32;
            let drawn = self.draw_quads(f, parent, &v, 0, count, Some(tex), None, nearest);
            self.scratch_quads = v;
            drawn
        }
    }

    /// Encode `plan` into `f`'s target.
    unsafe fn encode(&mut self, f: &mut Frame, plan: &Plan) -> Result<(), GpuError> {
        unsafe {
            let (w, h) = (f.w, f.h);
            if self.layers.first().is_some_and(|l| l.w != w || l.h != h) {
                self.layers.clear();
            }
            self.uploads = 0;
            self.draws = 0;
            self.externals.begin(self.count, !self.quads_yuv.is_null());
            self.sync_atlas(f, &plan.atlas)?;
            self.update_textures(f, plan);
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
            self.open(f, On::Frame, plan.partial.is_none().then_some(clear));

            for cmd in &plan.cmds {
                let op = match cmd {
                    Cmd::Quads(q) => {
                        let mut chroma = None;
                        let tex = match &q.texture {
                            None => None,
                            Some(TexRef::Image(image)) => self.texture(image),
                            Some(TexRef::Atlas(i)) => plan
                                .atlas
                                .get(*i)
                                .and_then(|p| self.atlas.get(&p.id))
                                .map(|t| t.tex),
                            Some(TexRef::External(image)) => {
                                let yuv = !self.quads_yuv.is_null();
                                match self.externals.get(self.device, image, self.count, yuv) {
                                    Some((t, uv)) => {
                                        chroma = uv;
                                        Some(t)
                                    }
                                    // Its pixels, read on the CPU.
                                    None => image.pixmap().and_then(|p| self.texture(&p)),
                                }
                            }
                        };
                        let level = levels.last().unwrap();
                        let (first, n) = (q.first, q.count);
                        self.draw_quads(f, level, &plan.quads, first, n, tex, chroma, q.nearest)?;
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
                        let level = levels.last_mut().unwrap();
                        self.stencil_polygon(f, level, pts, level.depth, self.ds_clip_inc)?;
                        level.depth += 1;
                        level.undo.push(Undo::Stencil(pts.clone()));
                    }
                    Op::PopClip => {
                        let level = levels.last_mut().unwrap();
                        match level.undo.pop() {
                            Some(Undo::Scissor(s)) => level.scissor = s,
                            Some(Undo::Stencil(pts)) => {
                                self.stencil_polygon(
                                    f,
                                    level,
                                    &pts,
                                    level.depth,
                                    self.ds_clip_dec,
                                )?;
                                level.depth -= 1;
                            }
                            None => {}
                        }
                    }
                    Op::PushLayer { .. } | Op::PushResampled(_) => {
                        let index = levels.iter().filter(|l| l.layer.is_some()).count();
                        if self.layers.len() <= index {
                            self.layers
                                .push(Target::new(self.device, w, h, PIXEL_BGRA8)?);
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
            f.end();
            Ok(())
        }
    }

    /// A new frame drawing into `color`/`stencil`, `w`×`h`.
    unsafe fn frame(&self, color: Id, stencil: Id, w: i32, h: i32) -> Result<Frame, GpuError> {
        unsafe {
            let cmd = send!(Id; self.queue, c"commandBuffer");
            if cmd.is_null() {
                return Err(GpuError::Lost("no command buffer".into()));
            }
            Ok(Frame {
                cmd,
                enc: std::ptr::null_mut(),
                on: None,
                color,
                stencil,
                w,
                h,
                fresh: true,
                buf: std::ptr::null_mut(),
                ptr: std::ptr::null_mut(),
                cap: 0,
                used: 0,
            })
        }
    }

    fn check_size(plan: &Plan) -> Result<(i32, i32), GpuError> {
        let (w, h) = (plan.size.width as i32, plan.size.height as i32);
        if w <= 0 || h <= 0 || w > MAX_SIZE || h > MAX_SIZE {
            return Err(GpuError::Lost(format!(
                "{w}x{h} is outside Metal's texture sizes"
            )));
        }
        Ok((w, h))
    }

    /// Draw `plan` into the offscreen frame target and wait for the GPU;
    /// with `read`, copy the frame into a buffer and return its bytes
    /// (premultiplied BGRA).
    unsafe fn offscreen(&mut self, plan: &Plan, read: bool) -> Result<Vec<u8>, GpuError> {
        unsafe {
            let (w, h) = Self::check_size(plan)?;
            // The target no longer holds a window's frame.
            self.kept.forget();
            let (color, stencil) = self.frame_target(w, h)?;
            let mut f = self.frame(color, stencil, w, h)?;
            let encoded = self.encode(&mut f, plan);
            self.finish_frame(encoded)?;
            let len = w as usize * h as usize * 4;
            let mut out = Vec::new();
            let buf = if read {
                let buf =
                    send!(Id; self.device, c"newBufferWithLength:options:", len; usize, 0u64; u64);
                if buf.is_null() {
                    return Err(GpuError::Lost("no read-back buffer".into()));
                }
                let blit = send!(Id; f.cmd, c"blitCommandEncoder");
                let origin = Size3 { w: 0, h: 0, d: 0 };
                let size = Size3 {
                    w: w as usize,
                    h: h as usize,
                    d: 1,
                };
                send!((); blit,
                    c"copyFromTexture:sourceSlice:sourceLevel:sourceOrigin:sourceSize:toBuffer:destinationOffset:destinationBytesPerRow:destinationBytesPerImage:",
                    color; Id, 0usize; usize, 0usize; usize, origin; Size3, size; Size3,
                    buf; Id, 0usize; usize, w as usize * 4; usize, len; usize);
                send!((); blit, c"endEncoding");
                buf
            } else {
                std::ptr::null_mut()
            };
            send!((); f.cmd, c"commit");
            send!((); f.cmd, c"waitUntilCompleted");
            let status = send!(u64; f.cmd, c"status");
            if status == STATUS_ERROR {
                release(buf);
                let err = send!(Id; f.cmd, c"error");
                return Err(GpuError::Lost(describe(err)));
            }
            if read {
                let p = send!(*const u8; buf, c"contents");
                out.extend_from_slice(std::slice::from_raw_parts(p, len));
                release(buf);
            }
            Ok(out)
        }
    }

    /// The frame target, made `w`×`h` (a new one keeps no frame).
    unsafe fn frame_target(&mut self, w: i32, h: i32) -> Result<(Id, Id), GpuError> {
        unsafe {
            if self.frame.as_ref().is_none_or(|t| t.w != w || t.h != h) {
                self.kept.forget();
                self.frame = None;
                self.frame = Some(Target::new(self.device, w, h, PIXEL_BGRA8)?);
            }
            let t = self.frame.as_ref().unwrap();
            Ok((t.color, t.stencil))
        }
    }

    /// End a frame's bookkeeping, whether `encoded` or not.
    fn finish_frame(&mut self, encoded: Result<(), GpuError>) -> Result<(), GpuError> {
        self.sweep_textures();
        self.count += 1;
        encoded
    }
}

/// Each polygon as a triangle fan from its first point, flattened to a
/// triangle list of `[x, y, 0, 0]` vertices.
fn fan_triangles<'a>(polys: impl Iterator<Item = &'a [[f32; 2]]>) -> Vec<f32> {
    let mut v = Vec::new();
    for p in polys {
        if p.len() < 3 {
            continue;
        }
        for i in 1..p.len() - 1 {
            for q in [p[0], p[i], p[i + 1]] {
                v.extend_from_slice(&[q[0], q[1], 0.0, 0.0]);
            }
        }
    }
    v
}

/// A render pipeline from `vs` and `fs` drawing into `format` (with a
/// stencil attachment), premultiplied-alpha `blend`ed, or not writing color
/// at all unless `write`.
unsafe fn pipeline(
    dev: Id,
    vs: Id,
    fs: Id,
    format: u64,
    blend: bool,
    write: bool,
) -> Result<Id, GpuError> {
    unsafe {
        if vs.is_null() || fs.is_null() {
            return Err(GpuError::Unavailable("a Metal shader is missing".into()));
        }
        let desc = send!(Id; send!(Id; class(c"MTLRenderPipelineDescriptor"), c"alloc"), c"init");
        send!((); desc, c"setVertexFunction:", vs; Id);
        send!((); desc, c"setFragmentFunction:", fs; Id);
        let atts = send!(Id; desc, c"colorAttachments");
        let a0 = send!(Id; atts, c"objectAtIndexedSubscript:", 0usize; usize);
        send!((); a0, c"setPixelFormat:", format; u64);
        if !write {
            send!((); a0, c"setWriteMask:", 0u64; u64);
        }
        if blend {
            send!((); a0, c"setBlendingEnabled:", true; bool);
            send!((); a0, c"setSourceRGBBlendFactor:", BLEND_ONE; u64);
            send!((); a0, c"setDestinationRGBBlendFactor:", BLEND_ONE_MINUS_SRC_ALPHA; u64);
            send!((); a0, c"setSourceAlphaBlendFactor:", BLEND_ONE; u64);
            send!((); a0, c"setDestinationAlphaBlendFactor:", BLEND_ONE_MINUS_SRC_ALPHA; u64);
        }
        send!((); desc, c"setStencilAttachmentPixelFormat:", PIXEL_STENCIL8; u64);
        let mut err: Id = std::ptr::null_mut();
        let state = send!(Id; dev, c"newRenderPipelineStateWithDescriptor:error:", desc; Id, &mut err; *mut Id);
        release(desc);
        if state.is_null() {
            return Err(GpuError::Unavailable(format!(
                "a Metal pipeline failed: {}",
                describe(err)
            )));
        }
        Ok(state)
    }
}

/// A depth-stencil state testing `(compare, pass operation)` on front and
/// back faces with `read` and `write` masks; no depth.
unsafe fn depth_stencil(dev: Id, front: (u64, u64), back: (u64, u64), read: u32, write: u32) -> Id {
    unsafe {
        let face = |(compare, op): (u64, u64)| {
            let s = send!(Id; send!(Id; class(c"MTLStencilDescriptor"), c"alloc"), c"init");
            send!((); s, c"setStencilCompareFunction:", compare; u64);
            send!((); s, c"setStencilFailureOperation:", KEEP; u64);
            send!((); s, c"setDepthFailureOperation:", KEEP; u64);
            send!((); s, c"setDepthStencilPassOperation:", op; u64);
            send!((); s, c"setReadMask:", read; u32);
            send!((); s, c"setWriteMask:", write; u32);
            s
        };
        let (f, b) = (face(front), face(back));
        let d = send!(Id; send!(Id; class(c"MTLDepthStencilDescriptor"), c"alloc"), c"init");
        send!((); d, c"setFrontFaceStencil:", f; Id);
        send!((); d, c"setBackFaceStencil:", b; Id);
        let state = send!(Id; dev, c"newDepthStencilStateWithDescriptor:", d; Id);
        for o in [f, b, d] {
            release(o);
        }
        state
    }
}

impl Executor for MetalExecutor {
    fn api(&self) -> Api {
        Api::Metal
    }

    fn read_back(&mut self, plan: &Plan) -> Result<Pixmap, GpuError> {
        let mut data = pooled(|| unsafe { self.offscreen(plan, true) })?;
        // BGRA premultiplied → RGBA straight.
        for px in data.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
            let a = px[3];
            if a != 0 && a != 255 {
                for c in &mut px[..3] {
                    *c = ((*c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
                }
            }
        }
        Ok(Pixmap::from_rgba8(
            PhysicalSize::new(plan.size.width, plan.size.height),
            data,
        ))
    }

    fn finish(&mut self, plan: &Plan) -> Result<(), GpuError> {
        pooled(|| unsafe { self.offscreen(plan, false) }).map(drop)
    }

    fn can_present(&self, layer: &GpuLayer) -> bool {
        matches!(layer, GpuLayer::Metal(l) if !l.is_null())
    }

    fn present(
        &mut self,
        plan: &mut Plan,
        layer: &GpuLayer,
        _scale: ScaleFactor,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        let GpuLayer::Metal(layer) = *layer else {
            return Err(GpuError::Unavailable("not a Metal layer".into()));
        };
        if layer.is_null() {
            return Err(GpuError::Unavailable("no Metal layer".into()));
        }
        pooled(|| unsafe { self.present_to(plan, layer, dirty) })
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
        unsafe {
            let name = send!(Id; self.device, c"name");
            if name.is_null() {
                return String::new();
            }
            let c = send!(*const c_char; name, c"UTF8String");
            if c.is_null() {
                String::new()
            } else {
                CStr::from_ptr(c).to_string_lossy().into_owned()
            }
        }
    }
}

impl MetalExecutor {
    unsafe fn present_to(
        &mut self,
        plan: &mut Plan,
        layer: Id,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        unsafe {
            // The frame before this one: did the device fail it?
            if !self.last.is_null() {
                let status = send!(u64; self.last, c"status");
                if status == STATUS_ERROR {
                    let err = describe(send!(Id; self.last, c"error"));
                    return Err(GpuError::Lost(err));
                }
                release(self.last);
                self.last = std::ptr::null_mut();
            }
            let (w, h) = Self::check_size(plan)?;
            if send!(Id; layer, c"device") != self.device {
                send!((); layer, c"setDevice:", self.device; Id);
                send!((); layer, c"setPixelFormat:", PIXEL_BGRA8; u64);
                // The frame is copied in from the kept target.
                send!((); layer, c"setFramebufferOnly:", false; bool);
                send!((); layer, c"setColorspace:", self.srgb; *mut c_void);
            }
            let size = CgSize {
                w: w as f64,
                h: h as f64,
            };
            if send!(CgSize; layer, c"drawableSize") != size {
                send!((); layer, c"setDrawableSize:", size; CgSize);
            }
            let drawable = send!(Id; layer, c"nextDrawable");
            if drawable.is_null() {
                return Err(GpuError::Skipped);
            }
            let color = send!(Id; drawable, c"texture");
            let tw = send!(usize; color, c"width") as i32;
            let th = send!(usize; color, c"height") as i32;
            if (tw, th) != (w, h) {
                // The layer didn't take the new size yet: show it next time.
                return Err(GpuError::Skipped);
            }
            // Into the kept target (only what changed, when it holds this
            // window's last frame), then all of it into the drawable.
            let key = layer as usize;
            let (frame, stencil) = self.frame_target(w, h)?;
            self.kept.limit(key, plan, dirty);
            let mut f = self.frame(frame, stencil, w, h)?;
            let encoded = self.encode(&mut f, plan);
            if encoded.is_err() {
                self.kept.forget();
            }
            self.finish_frame(encoded)?;
            let blit = send!(Id; f.cmd, c"blitCommandEncoder");
            let origin = Size3 { w: 0, h: 0, d: 0 };
            let size = Size3 {
                w: w as usize,
                h: h as usize,
                d: 1,
            };
            send!((); blit,
                c"copyFromTexture:sourceSlice:sourceLevel:sourceOrigin:sourceSize:toTexture:destinationSlice:destinationLevel:destinationOrigin:",
                frame; Id, 0usize; usize, 0usize; usize, origin; Size3, size; Size3,
                color; Id, 0usize; usize, 0usize; usize, origin; Size3);
            send!((); blit, c"endEncoding");
            // A window showing live OS controls (views over the layer, moved
            // in the same pass) presents with Core Animation's transaction,
            // so the frame and the views' new places show together.
            if send!(bool; layer, c"presentsWithTransaction") {
                send!((); f.cmd, c"commit");
                send!((); f.cmd, c"waitUntilScheduled");
                send!((); drawable, c"present");
            } else {
                send!((); f.cmd, c"presentDrawable:", drawable; Id);
                send!((); f.cmd, c"commit");
            }
            self.last = send!(Id; f.cmd, c"retain");
            self.kept.presented(key, (w as u32, h as u32), dirty);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::corpus;
    use crate::gpu::ops::{RasterCache, plan};
    use crate::render::{Scene, SoftwareRenderer};

    /// The kept frame target's pixels, premultiplied BGRA.
    unsafe fn kept_pixels(me: &MetalExecutor) -> Vec<u8> {
        unsafe {
            let t = me.frame.as_ref().unwrap();
            let len = t.w as usize * t.h as usize * 4;
            let buf = send!(Id; me.device, c"newBufferWithLength:options:", len; usize, 0u64; u64);
            let cmd = send!(Id; me.queue, c"commandBuffer");
            let blit = send!(Id; cmd, c"blitCommandEncoder");
            let origin = Size3 { w: 0, h: 0, d: 0 };
            let size = Size3 {
                w: t.w as usize,
                h: t.h as usize,
                d: 1,
            };
            send!((); blit,
                c"copyFromTexture:sourceSlice:sourceLevel:sourceOrigin:sourceSize:toBuffer:destinationOffset:destinationBytesPerRow:destinationBytesPerImage:",
                t.color; Id, 0usize; usize, 0usize; usize, origin; Size3, size; Size3,
                buf; Id, 0usize; usize, t.w as usize * 4; usize, len; usize);
            send!((); blit, c"endEncoding");
            send!((); cmd, c"commit");
            send!((); cmd, c"waitUntilCompleted");
            let p = send!(*const u8; buf, c"contents");
            let out = std::slice::from_raw_parts(p, len).to_vec();
            release(buf);
            out
        }
    }

    /// Presenting to a layer (one not on screen): a frame whole, then the
    /// next with a change, which redraws only the change over the kept frame
    /// before it's copied into the drawable.
    #[test]
    fn a_window_frame_redraws_only_what_changed() {
        let Ok(mut me) = MetalExecutor::new() else {
            eprintln!("skipping: no Metal device");
            return;
        };
        pooled(|| {
            let layer = unsafe { send!(Id; class(c"CAMetalLayer"), c"new") };
            assert!(!layer.is_null());
            let gl = GpuLayer::Metal(layer);
            let (before, after, region) = corpus::partial();
            let bg = corpus::BACKGROUND;
            let size = PhysicalSize::new(320, 240);
            let mut rasters = RasterCache::default();
            let s = ScaleFactor::new(2.0);
            let r = corpus::device_region(region, 2.0);

            let mut a = plan(&before, bg, size, &mut rasters);
            me.present(&mut a, &gl, s, None).expect("the first frame");
            assert_eq!(a.partial, None, "the first frame is drawn whole");
            let mut b = plan(&after, bg, size, &mut rasters);
            me.present(&mut b, &gl, s, Some(r))
                .expect("the second frame");
            assert_eq!(b.partial, Some(r), "the second one only where it changed");
            unsafe { send!((); me.last, c"waitUntilCompleted") };

            let mut got = unsafe { kept_pixels(&me) };
            for px in got.as_chunks_mut::<4>().0 {
                px.swap(0, 2); // BGRA to RGBA; the frame is opaque
            }
            let cpu = |sc: &Scene| SoftwareRenderer::new().background(bg).render(sc.clone(), s);
            let want = corpus::composite(&cpu(&before), &cpu(&after), r);
            let mean = got
                .iter()
                .zip(want.as_bytes())
                .map(|(a, b)| a.abs_diff(*b) as f64)
                .sum::<f64>()
                / got.len() as f64;
            assert!(mean < 0.8, "the kept frame differs: mean {mean}");

            // Another window's frame takes the target: this one's next is
            // drawn whole.
            let other = unsafe { send!(Id; class(c"CAMetalLayer"), c"new") };
            let mut c = plan(&before, bg, size, &mut rasters);
            me.present(&mut c, &GpuLayer::Metal(other), s, None)
                .unwrap();
            let mut d = plan(&after, bg, size, &mut rasters);
            me.present(&mut d, &gl, s, Some(r)).unwrap();
            assert_eq!(d.partial, None, "the target held another window's frame");
            unsafe {
                send!((); me.last, c"waitUntilCompleted");
                release(layer);
                release(other);
            }
        });
    }
}
