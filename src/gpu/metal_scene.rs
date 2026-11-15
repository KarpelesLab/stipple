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
//! by the image's identity, like the GL backend's. Raw `objc_msgSend` FFI, as
//! in the rest of the macOS and iOS code.

#![allow(unsafe_code)]

use super::ops::{Clip, Op, Plan, Sample};
use super::renderer::{Api, Executor, GpuError};
use crate::geometry::{PhysicalSize, ScaleFactor};
use crate::platform::GpuLayer;
use crate::render::Pixmap;
use oxideav_core::Transform2D;
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

// A box: `aux` is the device position. Coverage is the exact overlap of the
// pixel with a sharp box (or ring), or a signed distance for rounded corners.
struct BoxU { float4 rect; float4 color; float radius; float stroke; };
static float span(float p, float h) { return max(min(h, p + 0.5) - max(-h, p - 0.5), 0.0); }
static float boxcov(float2 p, float2 h) { h = max(h, float2(0.0)); return span(p.x, h.x) * span(p.y, h.y); }
static float sdf(float2 p, float2 h, float r) {
    float2 d = abs(p) - (h - float2(r));
    return length(max(d, float2(0.0))) + min(max(d.x, d.y), 0.0) - r;
}
fragment float4 box_fs(VOut in [[stage_in]], constant BoxU& u [[buffer(0)]]) {
    float2 c = (u.rect.xy + u.rect.zw) * 0.5;
    float2 h = (u.rect.zw - u.rect.xy) * 0.5;
    float2 p = in.aux - c;
    float w = u.stroke * 0.5;
    float cov;
    if (u.radius <= 0.0) {
        cov = u.stroke > 0.0 ? boxcov(p, h + float2(w)) - boxcov(p, h - float2(w)) : boxcov(p, h);
    } else {
        float r = min(u.radius, min(h.x, h.y));
        float d = sdf(p, h, r);
        cov = u.stroke > 0.0 ? clamp(0.5 - (d - w), 0.0, 1.0) - clamp(0.5 - (d + w), 0.0, 1.0)
                             : clamp(0.5 - d, 0.0, 1.0);
    }
    return u.color * cov;
}

// A textured quad: `aux` is the local position inside `rect`, `px` the device
// pixels per local unit along each axis (for edge coverage). `uv` is the part
// of the texture spread over the rect, kept within `clampr`; `tint`
// (premultiplied) multiplies every texel.
struct TexU { float4 rect; float4 uv; float4 clampr; float4 tint; float2 px;
              float opacity; float premul; float aa; float nearest; };
fragment float4 tex_fs(VOut in [[stage_in]], texture2d<float> t [[texture(0)]],
                       constant TexU& u [[buffer(0)]]) {
    constexpr sampler lin(filter::linear, address::clamp_to_edge);
    constexpr sampler near(filter::nearest, address::clamp_to_edge);
    float2 f = (in.aux - u.rect.xy) / (u.rect.zw - u.rect.xy);
    float2 uv = clamp(mix(u.uv.xy, u.uv.zw, f), u.clampr.xy, u.clampr.zw);
    float4 c = u.nearest > 0.5 ? t.sample(near, uv) : t.sample(lin, uv);
    if (u.premul < 0.5) { c.rgb *= c.a; }
    c *= u.tint;
    float cov = 1.0;
    if (u.aa > 0.5) {
        float2 d = min(in.aux - u.rect.xy, u.rect.zw - in.aux) * u.px;
        cov = clamp(d.x + 0.5, 0.0, 1.0) * clamp(d.y + 0.5, 0.0, 1.0);
    }
    return c * (u.opacity * cov);
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
struct BoxU {
    rect: [f32; 4],
    color: [f32; 4],
    radius: f32,
    stroke: f32,
    _pad: [f32; 2],
}

#[repr(C)]
struct TexU {
    rect: [f32; 4],
    uv: [f32; 4],
    clamp: [f32; 4],
    tint: [f32; 4],
    px: [f32; 2],
    opacity: f32,
    premul: f32,
    aa: f32,
    nearest: f32,
    _pad: [f32; 2],
}

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
    boxes: Id,
    tex: Id,
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
    /// The offscreen frame (read-back), the drawable's stencil, layers,
    /// the path scratch.
    frame: Option<Target>,
    drawable_stencil: Option<(Id, i32, i32)>,
    layers: Vec<Target>,
    scratch: Option<Target>,
    textures: HashMap<usize, Texture>,
    /// The last presented frame's command buffer (retained), to see whether
    /// the device failed it.
    last: Id,
    count: u64,
    uploads: usize,
}

impl Drop for MetalExecutor {
    fn drop(&mut self) {
        self.frame = None;
        self.layers.clear();
        self.scratch = None;
        unsafe {
            if let Some((s, _, _)) = self.drawable_stencil.take() {
                release(s);
            }
            for t in self.textures.values() {
                release(t.tex);
            }
            for obj in [
                self.last,
                self.boxes,
                self.tex,
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
                boxes: std::ptr::null_mut(),
                tex: std::ptr::null_mut(),
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
                drawable_stencil: None,
                layers: Vec::new(),
                scratch: None,
                textures: HashMap::new(),
                last: std::ptr::null_mut(),
                count: 0,
                uploads: 0,
            };
            if queue.is_null() {
                return Err(GpuError::Unavailable("no Metal command queue".into()));
            }
            let src = std::ffi::CString::new(MSL).unwrap();
            let mut err: Id = std::ptr::null_mut();
            let lib = send!(Id; device, c"newLibraryWithSource:options:error:",
                nsstring(&src); Id, std::ptr::null_mut(); Id, &mut err; *mut Id);
            if lib.is_null() {
                return Err(GpuError::Unavailable(format!(
                    "the Metal shaders didn't compile: {}",
                    describe(err)
                )));
            }
            let func = |name: &CStr| send!(Id; lib, c"newFunctionWithName:", nsstring(name); Id);
            let vs = func(c"vs");
            let fns = [
                func(c"box_fs"),
                func(c"tex_fs"),
                func(c"flat_fs"),
                func(c"resolve_fs"),
            ];
            let [box_fs, tex_fs, flat_fs, resolve_fs] = fns;
            let built = (|| -> Result<(), GpuError> {
                let dev = device;
                me.boxes = pipeline(dev, vs, box_fs, PIXEL_BGRA8, true, true)?;
                me.tex = pipeline(dev, vs, tex_fs, PIXEL_BGRA8, true, true)?;
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
            for f in [vs, box_fs, tex_fs, flat_fs, resolve_fs, lib] {
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
                },
            );
            Some(tex)
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
            send!((); sa, c"setLoadAction:", if clear.is_some() { CLEAR } else { LOAD }; u64);
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

    unsafe fn draw(f: &Frame, count: usize) {
        unsafe {
            send!((); f.enc, c"drawPrimitives:vertexStart:vertexCount:",
                TRIANGLE; u64, 0u64; u64, count as u64; u64);
        }
    }

    /// Device pixels of a `w`×`h` target to clip space.
    fn xf(w: i32, h: i32) -> [f32; 4] {
        [2.0 / w as f32, -2.0 / h as f32, -1.0, 1.0]
    }

    unsafe fn draw_box(
        &self,
        f: &mut Frame,
        level: &Level,
        rect: [f32; 4],
        radius: f32,
        stroke: f32,
        color: [f32; 4],
    ) -> Result<(), GpuError> {
        unsafe {
            if !self.bind(f, level, self.boxes, self.ds_test, level.depth) {
                return Ok(());
            }
            let pad = 1.0 + stroke * 0.5;
            let (x0, y0, x1, y1) = (rect[0] - pad, rect[1] - pad, rect[2] + pad, rect[3] + pad);
            #[rustfmt::skip]
            let v = [
                x0, y0, x0, y0,  x1, y0, x1, y0,  x0, y1, x0, y1,
                x0, y1, x0, y1,  x1, y0, x1, y0,  x1, y1, x1, y1,
            ];
            self.vertices(f, &v, Self::xf(f.w, f.h))?;
            Self::uniforms(
                f,
                &BoxU {
                    rect,
                    color,
                    radius,
                    stroke,
                    _pad: [0.0; 2],
                },
            );
            Self::draw(f, 6);
            Ok(())
        }
    }

    /// Draw texture `tex` over the local `rect` through `m` (see the GL
    /// executor's `draw_texture`).
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_texture(
        &self,
        f: &mut Frame,
        level: &Level,
        tex: Id,
        m: &Transform2D,
        rect: [f32; 4],
        opacity: f32,
        premul: bool,
        aa: bool,
        sample: &Sample,
    ) -> Result<(), GpuError> {
        unsafe {
            if !self.bind(f, level, self.tex, self.ds_test, level.depth) {
                return Ok(());
            }
            let px = (m.a.hypot(m.b).max(1e-6), m.c.hypot(m.d).max(1e-6));
            let (pu, pv) = if aa {
                (1.0 / px.0, 1.0 / px.1)
            } else {
                (0.0, 0.0)
            };
            let (x0, y0, x1, y1) = (rect[0] - pu, rect[1] - pv, rect[2] + pu, rect[3] + pv);
            let corner = |x: f32, y: f32| {
                let d = m.apply(oxideav_core::Point { x, y });
                [d.x, d.y, x, y]
            };
            let (a, b, c, d) = (
                corner(x0, y0),
                corner(x1, y0),
                corner(x0, y1),
                corner(x1, y1),
            );
            let v: Vec<f32> = [a, b, c, c, b, d].concat();
            self.vertices(f, &v, Self::xf(f.w, f.h))?;
            send!((); f.enc, c"setFragmentTexture:atIndex:", tex; Id, 0usize; usize);
            let flag = |b: bool| if b { 1.0 } else { 0.0 };
            Self::uniforms(
                f,
                &TexU {
                    rect,
                    uv: sample.uv,
                    clamp: sample.clamp,
                    tint: sample.tint,
                    px: [px.0, px.1],
                    opacity,
                    premul: flag(premul),
                    aa: flag(aa),
                    nearest: flag(sample.nearest),
                    _pad: [0.0; 2],
                },
            );
            Self::draw(f, 6);
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
                Self::draw(f, fans.len() / 4);
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
            Self::draw(f, 6);

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
            Self::draw(f, 6);
            Ok(())
        }
    }

    /// Draw the polygon `pts` into the stencil of `level`'s target, where it
    /// is `from`, with `ds` (increment or decrement).
    unsafe fn stencil_polygon(
        &self,
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
            };
            if !self.bind(f, &full, self.clip, ds, from) {
                return Ok(());
            }
            let fans = fan_triangles(std::iter::once(pts));
            if !fans.is_empty() {
                self.vertices(f, &fans, Self::xf(w, h))?;
                Self::uniforms(f, &[0.0f32; 4]);
                Self::draw(f, fans.len() / 4);
            }
            Ok(())
        }
    }

    /// Composite the layer `layer` into `parent`.
    unsafe fn composite(
        &self,
        f: &mut Frame,
        layer: &Level,
        parent: &Level,
    ) -> Result<(), GpuError> {
        unsafe {
            let tex = self.layers[layer.layer.unwrap()].color;
            let full = [0.0, 0.0, f.w as f32, f.h as f32];
            self.draw_texture(
                f,
                parent,
                tex,
                &Transform2D::identity(),
                full,
                layer.opacity,
                true,
                false,
                &Sample::WHOLE,
            )
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
            let mut levels = vec![Level {
                layer: None,
                opacity: 1.0,
                scissor: [0, 0, w, h],
                depth: 0,
                undo: Vec::new(),
            }];
            let bg = plan.background;
            let a = bg.a as f32 / 255.0;
            let clear = [
                bg.r as f32 / 255.0 * a,
                bg.g as f32 / 255.0 * a,
                bg.b as f32 / 255.0 * a,
                a,
            ];
            self.open(f, On::Frame, Some(clear));

            for op in &plan.ops {
                match op {
                    Op::Box {
                        rect,
                        radius,
                        stroke,
                        color,
                    } => {
                        let level = levels.last().unwrap();
                        self.draw_box(f, level, *rect, *radius, *stroke, *color)?;
                    }
                    Op::Path {
                        contours,
                        even_odd,
                        color,
                        bbox,
                    } => {
                        let level = levels.last().unwrap();
                        self.draw_path(f, level, contours, *even_odd, *color, *bbox)?;
                    }
                    Op::Image {
                        image,
                        transform,
                        rect,
                        opacity,
                        uv,
                        tint,
                        nearest,
                    } => {
                        let Some(tex) = self.texture(image) else {
                            continue;
                        };
                        let size = image.size();
                        let sample = Sample::region(*uv, size.width, size.height, *tint, *nearest);
                        let level = levels.last().unwrap();
                        self.draw_texture(
                            f, level, tex, transform, *rect, *opacity, false, true, &sample,
                        )?;
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
                    Op::PushLayer { opacity } => {
                        let index = levels.iter().filter(|l| l.layer.is_some()).count();
                        if self.layers.len() <= index {
                            self.layers
                                .push(Target::new(self.device, w, h, PIXEL_BGRA8)?);
                        }
                        let scissor = levels.last().unwrap().scissor;
                        levels.push(Level {
                            layer: Some(index),
                            opacity: *opacity,
                            scissor,
                            depth: 0,
                            undo: Vec::new(),
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
            if self.frame.as_ref().is_none_or(|t| t.w != w || t.h != h) {
                self.frame = None;
                self.frame = Some(Target::new(self.device, w, h, PIXEL_BGRA8)?);
            }
            let (color, stencil) = {
                let t = self.frame.as_ref().unwrap();
                (t.color, t.stencil)
            };
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
        plan: &Plan,
        layer: &GpuLayer,
        _scale: ScaleFactor,
    ) -> Result<(), GpuError> {
        let GpuLayer::Metal(layer) = *layer;
        if layer.is_null() {
            return Err(GpuError::Unavailable("no Metal layer".into()));
        }
        pooled(|| unsafe { self.present_to(plan, layer) })
    }

    fn uploads(&self) -> usize {
        self.uploads
    }
}

impl MetalExecutor {
    unsafe fn present_to(&mut self, plan: &Plan, layer: Id) -> Result<(), GpuError> {
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
                send!((); layer, c"setFramebufferOnly:", true; bool);
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
            if self
                .drawable_stencil
                .is_none_or(|(_, sw, sh)| (sw, sh) != (w, h))
            {
                if let Some((s, _, _)) = self.drawable_stencil.take() {
                    release(s);
                }
                let s = texture(self.device, PIXEL_STENCIL8, w, h, USAGE_RENDER_TARGET)?;
                self.drawable_stencil = Some((s, w, h));
            }
            let stencil = self.drawable_stencil.unwrap().0;
            let mut f = self.frame(color, stencil, w, h)?;
            let encoded = self.encode(&mut f, plan);
            self.finish_frame(encoded)?;
            send!((); f.cmd, c"presentDrawable:", drawable; Id);
            send!((); f.cmd, c"commit");
            self.last = send!(Id; f.cmd, c"retain");
            Ok(())
        }
    }
}
