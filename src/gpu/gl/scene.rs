//! The GLES2 scene renderer: executes a lowered [`Plan`](crate::gpu::ops::Plan)
//! on a persistent context, surfaceless for read-backs, or on a window's EGL
//! surface to present.
//!
//! Presenting draws the frame into its offscreen target exactly as for a
//! read-back, then draws that target into the window surface (the default
//! framebuffer: one textured quad, flipped to the window's rows) and swaps.
//! The offscreen target persists, so a window whose last frame it still holds
//! redraws only what changed ([`Plan::limit`]); the quad still covers the
//! whole window, and the swap tells the compositor what changed
//! (`eglSwapBuffersWithDamageKHR`/`EXT`, where EGL has it).
//! The window surfaces need an EGL display on the window system: X11 through
//! EGL's XCB platform on a libxcb connection of the GPU's own (`gpu::xcb`),
//! Android on its one display. The first frame presented moves the context
//! there (its caches start over once).
//!
//! Everything is drawn in premultiplied alpha into framebuffer objects that
//! carry a stencil buffer: boxes (analytic coverage) and textured quads
//! (images, image regions, glyphs from the atlas) batched by
//! `crate::gpu::batch`, each batch one draw of the quad program; paths by
//! stencil-and-cover into a supersampled scratch target that is then resolved
//! (averaged) into the frame; layers into pooled offscreen targets composited
//! at their opacity. Clips are scissor rectangles or stencil polygons.
//!
//! The context, programs, targets and textures live in a thread-local, so a
//! renderer set with `App::render_with` keeps them from frame to frame.
//! Textures for images are cached by the image's identity (its `Arc`): kept
//! while the image is alive and drawn, re-uploaded if it is replaced, and
//! freed once it is dropped or hasn't been drawn for a while. Each glyph
//! atlas page is a texture brought up to date at the start of a frame with
//! only what changed in it (`glTexSubImage2D`).

use super::external::{self, Imports, Planes};
use super::*;
use crate::gpu::atlas::{AtlasPage, Upload};
use crate::gpu::batch::{self, Cmd, QUAD_FLOATS, TexRef};
use crate::gpu::canvas::rekey;
use crate::gpu::ops::{Clip, Op, Plan, Resample, Sample};
use crate::gpu::renderer::{Api, Executor, GpuError, KeptFrame};
use crate::platform::GpuLayer;
use oxideav_core::Transform2D;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::{Arc, Weak};

// Presenting to Wayland windows through dma-bufs.
#[cfg(target_os = "linux")]
mod dmabuf;
// Transparent windows' surfaces.
mod alpha;

const GL_SCISSOR_TEST: GLenum = 0x0C11;
const GL_STENCIL_TEST: GLenum = 0x0B90;
const GL_STENCIL_BUFFER_BIT: GLbitfield = 0x0400;
const GL_RENDERBUFFER: GLenum = 0x8D41;
const GL_STENCIL_INDEX8: GLenum = 0x8D48;
const GL_STENCIL_ATTACHMENT: GLenum = 0x8D20;
const GL_ALWAYS: GLenum = 0x0207;
const GL_EQUAL: GLenum = 0x0202;
const GL_NOTEQUAL: GLenum = 0x0205;
const GL_KEEP: GLenum = 0x1E00;
const GL_ZERO: GLenum = 0;
const GL_INCR: GLenum = 0x1E02;
const GL_DECR: GLenum = 0x1E03;
const GL_INVERT: GLenum = 0x150A;
const GL_INCR_WRAP: GLenum = 0x8507;
const GL_DECR_WRAP: GLenum = 0x8508;
const GL_FRONT: GLenum = 0x0404;
const GL_BACK: GLenum = 0x0405;
const GL_ONE: GLenum = 1;
const GL_MAX_TEXTURE_SIZE: GLenum = 0x0D33;
const GL_RENDERER: GLenum = 0x1F01;
const GL_MAX_RENDERBUFFER_SIZE: GLenum = 0x84E8;
const GL_TRIANGLE_FAN: GLenum = 0x0006;
const GL_STREAM_DRAW: GLenum = 0x88E0;
const GL_UNPACK_ALIGNMENT: GLenum = 0x0CF5;
const GL_PACK_ALIGNMENT: GLenum = 0x0D05;
const GL_CULL_FACE: GLenum = 0x0B44;
const EGL_NO_CONTEXT: EGLContext = core::ptr::null_mut();
const EGL_NO_SURFACE: EGLSurface = core::ptr::null_mut();
const GL_NO_ERROR: GLenum = 0;

#[link(name = "GLESv2")]
unsafe extern "C" {
    fn glScissor(x: GLint, y: GLint, width: GLsizei, height: GLsizei);
    fn glDisable(cap: GLenum);
    fn glColorMask(r: u8, g: u8, b: u8, a: u8);
    fn glStencilFunc(func: GLenum, reference: GLint, mask: GLuint);
    fn glStencilOp(fail: GLenum, zfail: GLenum, zpass: GLenum);
    fn glStencilOpSeparate(face: GLenum, fail: GLenum, zfail: GLenum, zpass: GLenum);
    fn glStencilMask(mask: GLuint);
    fn glClearStencil(s: GLint);
    fn glGenRenderbuffers(n: GLsizei, renderbuffers: *mut GLuint);
    fn glBindRenderbuffer(target: GLenum, renderbuffer: GLuint);
    fn glRenderbufferStorage(target: GLenum, format: GLenum, width: GLsizei, height: GLsizei);
    fn glFramebufferRenderbuffer(
        target: GLenum,
        attachment: GLenum,
        renderbuffertarget: GLenum,
        renderbuffer: GLuint,
    );
    fn glDeleteTextures(n: GLsizei, textures: *const GLuint);
    fn glDeleteFramebuffers(n: GLsizei, framebuffers: *const GLuint);
    fn glDeleteRenderbuffers(n: GLsizei, renderbuffers: *const GLuint);
    fn glGetIntegerv(pname: GLenum, data: *mut GLint);
    fn glGetString(name: GLenum) -> *const u8;
    fn glPixelStorei(pname: GLenum, param: GLint);
    fn glDisableVertexAttribArray(index: GLuint);
    #[allow(clippy::too_many_arguments)]
    fn glTexSubImage2D(
        target: GLenum,
        level: GLint,
        xoffset: GLint,
        yoffset: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
        ty: GLenum,
        pixels: *const c_void,
    );
}

/// Vertex attribute slots cleared before each draw (the quad program uses
/// five).
const MAX_ATTRIBS: GLuint = 8;

// The path, clip and resolve programs share one vertex shader: `a_pos` in
// device pixels mapped to clip space by `u_xf` (scale.xy, offset.zw), and a
// pass-through `a_aux`.
const VS: &[u8] = b"attribute vec2 a_pos;\n\
attribute vec2 a_aux;\n\
uniform vec4 u_xf;\n\
varying vec2 v_aux;\n\
void main() {\n\
  v_aux = a_aux;\n\
  gl_Position = vec4(a_pos * u_xf.xy + u_xf.zw, 0.0, 1.0);\n\
}\n\0";

const PRECISION: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH\nprecision highp float;\n#else\nprecision mediump float;\n#endif\n";

// Batched boxes and textured quads (`crate::gpu::batch`; the MSL `quad_vs`
// in `metal_scene.rs`): every input is a vertex attribute, `a0`..`a4`, so a
// run of them is one draw. `a0.xy` in device pixels is mapped to clip space
// by `u_xf`; the rest is passed through.
const QUAD_VS: &[u8] = b"attribute vec4 a0;\n\
attribute vec4 a1;\n\
attribute vec4 a2;\n\
attribute vec4 a3;\n\
attribute vec4 a4;\n\
uniform vec4 u_xf;\n\
varying vec2 v_p;\n\
varying vec4 v_a1;\n\
varying vec4 v_a2;\n\
varying vec4 v_a3;\n\
varying float v_mode;\n\
void main() {\n\
  v_p = a0.zw;\n\
  v_a1 = a1;\n\
  v_a2 = a2;\n\
  v_a3 = a3;\n\
  v_mode = a4.x;\n\
  gl_Position = vec4(a0.xy * u_xf.xy + u_xf.zw, 0.0, 1.0);\n\
}\n\0";

// A box: `v_p` is the device position, `v_a1` the rect, `v_a2` the color,
// `v_a3` the radius and stroke. Coverage is the exact overlap of the pixel
// with a sharp box (or ring), or a signed distance for rounded corners.
const QUAD_BOX: &str = "varying vec2 v_p;\n\
varying vec4 v_a1;\n\
varying vec4 v_a2;\n\
varying vec4 v_a3;\n\
varying float v_mode;\n\
float span(float p, float h) {\n\
  return max(min(h, p + 0.5) - max(-h, p - 0.5), 0.0);\n\
}\n\
float boxcov(vec2 p, vec2 h) {\n\
  h = max(h, vec2(0.0));\n\
  return span(p.x, h.x) * span(p.y, h.y);\n\
}\n\
float sdf(vec2 p, vec2 h, float r) {\n\
  vec2 d = abs(p) - (h - vec2(r));\n\
  return length(max(d, vec2(0.0))) + min(max(d.x, d.y), 0.0) - r;\n\
}\n\
vec4 quadBox() {\n\
  float radius = v_a3.x;\n\
  float stroke = v_a3.y;\n\
  vec2 c = (v_a1.xy + v_a1.zw) * 0.5;\n\
  vec2 h = (v_a1.zw - v_a1.xy) * 0.5;\n\
  vec2 p = v_p - c;\n\
  float w = stroke * 0.5;\n\
  float cov;\n\
  if (radius <= 0.0) {\n\
    if (stroke > 0.0) {\n\
      cov = boxcov(p, h + vec2(w)) - boxcov(p, h - vec2(w));\n\
    } else {\n\
      cov = boxcov(p, h);\n\
    }\n\
  } else {\n\
    float r = min(radius, min(h.x, h.y));\n\
    float d = sdf(p, h, r);\n\
    if (stroke > 0.0) {\n\
      cov = clamp(0.5 - (d - w), 0.0, 1.0) - clamp(0.5 - (d + w), 0.0, 1.0);\n\
    } else {\n\
      cov = clamp(0.5 - d, 0.0, 1.0);\n\
    }\n\
  }\n\
  return v_a2 * cov;\n\
}\n";

// Boxes only (a batch without a texture): textured quads are left out.
const QUADBOX_MAIN: &str = "void main() {\n\
  gl_FragColor = v_mode < 0.5 ? quadBox() : vec4(0.0);\n\
}\n";

// Boxes and textured quads. A textured quad: `v_p` is the texture
// coordinate, kept within `v_a2` (so a neighbor in the atlas doesn't bleed
// in); `v_a1` the distances to its edges in device pixels (for their
// coverage); `v_a3` the tint times the opacity (premultiplied).
const QUAD_MAIN: &str = "uniform sampler2D u_tex;\n\
void main() {\n\
  if (v_mode < 0.5) {\n\
    gl_FragColor = quadBox();\n\
  } else {\n\
    vec4 t = texture2D(u_tex, clamp(v_p, v_a2.xy, v_a2.zw));\n\
    if (v_mode < 1.5) { t.rgb *= t.a; }\n\
    t *= v_a3;\n\
    float cov = clamp(min(v_a1.x, v_a1.z) + 0.5, 0.0, 1.0)\n\
              * clamp(min(v_a1.y, v_a1.w) + 0.5, 0.0, 1.0);\n\
    gl_FragColor = t * cov;\n\
  }\n\
}\n";

// A flat color: the stencil and cover passes, and stencil clips.
const FLAT_FS: &str = "uniform vec4 u_color;\n\
void main() { gl_FragColor = u_color; }\n";

// Resolving a supersampled path coverage: `v_aux` is the position in scratch
// texels; four bilinear taps average the pixel's `ss`×`ss` block.
const RESOLVE_FS: &str = "varying vec2 v_aux;\n\
uniform sampler2D u_tex;\n\
uniform vec2 u_inv;\n\
uniform float u_tap;\n\
uniform vec4 u_color;\n\
void main() {\n\
  float c = texture2D(u_tex, (v_aux + vec2(-u_tap, -u_tap)) * u_inv).r\n\
          + texture2D(u_tex, (v_aux + vec2( u_tap, -u_tap)) * u_inv).r\n\
          + texture2D(u_tex, (v_aux + vec2(-u_tap,  u_tap)) * u_inv).r\n\
          + texture2D(u_tex, (v_aux + vec2( u_tap,  u_tap)) * u_inv).r;\n\
  gl_FragColor = u_color * (c * 0.25);\n\
}\n";

/// The path, clip and resolve programs' attributes: `(name, floats)`, packed
/// in a vertex in this order.
const PLAIN_ATTRIBS: &[(&CStr, GLint)] = &[(c"a_pos", 2), (c"a_aux", 2)];
/// The quad programs' attributes.
const QUAD_ATTRIBS: &[(&CStr, GLint)] =
    &[(c"a0", 4), (c"a1", 4), (c"a2", 4), (c"a3", 4), (c"a4", 4)];

/// A linked program and the locations it uses.
struct Program {
    id: GLuint,
    /// Each attribute's location (if the program uses it), floats and
    /// offset in floats.
    attribs: Vec<(Option<GLuint>, GLint, usize)>,
    /// Floats per vertex.
    stride: usize,
    u: HashMap<&'static str, GLint>,
}

impl Program {
    unsafe fn new(fs_body: &str, uniforms: &[&'static str]) -> Result<Self, String> {
        unsafe { Self::with(VS, PLAIN_ATTRIBS, fs_body, uniforms) }
    }

    unsafe fn with(
        vs_src: &[u8],
        layout: &[(&CStr, GLint)],
        fs_body: &str,
        uniforms: &[&'static str],
    ) -> Result<Self, String> {
        unsafe {
            let vs = compile(GL_VERTEX_SHADER, vs_src)?;
            // `#extension` lines go before everything else.
            let ext_end: usize = fs_body
                .split_inclusive('\n')
                .take_while(|l| l.starts_with("#extension"))
                .map(str::len)
                .sum();
            let (ext, body) = fs_body.split_at(ext_end);
            let src = format!("{ext}{PRECISION}{body}\0");
            let fs = compile(GL_FRAGMENT_SHADER, src.as_bytes())?;
            let id = glCreateProgram();
            glAttachShader(id, vs);
            glAttachShader(id, fs);
            glLinkProgram(id);
            let mut ok: GLint = 0;
            glGetProgramiv(id, GL_LINK_STATUS, &mut ok);
            if ok == 0 {
                return Err("scene program link failed".into());
            }
            let mut attribs = Vec::new();
            let mut at = 0;
            for &(name, n) in layout {
                let loc = glGetAttribLocation(id, name.as_ptr());
                attribs.push(((loc >= 0).then_some(loc as GLuint), n, at));
                at += n as usize;
            }
            if attribs[0].0.is_none() {
                return Err("scene program: no position attribute".into());
            }
            let mut u = HashMap::new();
            for name in uniforms.iter().copied().chain(["u_xf"]) {
                let c = std::ffi::CString::new(name).unwrap();
                u.insert(name, glGetUniformLocation(id, c.as_ptr()));
            }
            Ok(Self {
                id,
                attribs,
                stride: at,
                u,
            })
        }
    }

    fn loc(&self, name: &str) -> GLint {
        self.u.get(name).copied().unwrap_or(-1)
    }
}

/// A color texture with a stencil buffer, drawn into through an FBO.
struct Target {
    fbo: GLuint,
    tex: GLuint,
    stencil: GLuint,
    w: i32,
    h: i32,
}

impl Target {
    unsafe fn new(w: i32, h: i32) -> Result<Self, String> {
        unsafe {
            let mut tex = 0;
            glGenTextures(1, &mut tex);
            glBindTexture(GL_TEXTURE_2D, tex);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            glTexImage2D(
                GL_TEXTURE_2D,
                0,
                GL_RGBA as GLint,
                w,
                h,
                0,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                core::ptr::null(),
            );
            let mut stencil = 0;
            glGenRenderbuffers(1, &mut stencil);
            glBindRenderbuffer(GL_RENDERBUFFER, stencil);
            glRenderbufferStorage(GL_RENDERBUFFER, GL_STENCIL_INDEX8, w, h);
            let mut fbo = 0;
            glGenFramebuffers(1, &mut fbo);
            glBindFramebuffer(GL_FRAMEBUFFER, fbo);
            glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, tex, 0);
            glFramebufferRenderbuffer(
                GL_FRAMEBUFFER,
                GL_STENCIL_ATTACHMENT,
                GL_RENDERBUFFER,
                stencil,
            );
            let target = Target {
                fbo,
                tex,
                stencil,
                w,
                h,
            };
            if glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE {
                target.delete();
                return Err("scene framebuffer (color + stencil) incomplete".into());
            }
            Ok(target)
        }
    }

    unsafe fn delete(&self) {
        unsafe {
            glDeleteFramebuffers(1, &self.fbo);
            glDeleteTextures(1, &self.tex);
            glDeleteRenderbuffers(1, &self.stencil);
        }
    }
}

/// A cached image texture.
struct Texture {
    image: Weak<Pixmap>,
    tex: GLuint,
    used: u64,
    bytes: usize,
}

/// An atlas page's texture, holding `version` of it.
struct AtlasTex {
    tex: GLuint,
    side: u32,
    version: u64,
}

/// Textures of images not drawn for this many frames are freed.
const TEXTURE_KEEP_FRAMES: u64 = 120;

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

/// Which EGL display a context is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Native {
    /// Offscreen only (Mesa's surfaceless platform; Android's display
    /// without a window config).
    Offscreen,
    /// EGL's XCB platform, for X11 windows.
    Xcb,
    /// Android's display, with a window config.
    Android,
}

impl Native {
    fn of(layer: &GpuLayer) -> Option<Native> {
        match layer {
            GpuLayer::X11 { .. } => Some(Native::Xcb),
            GpuLayer::Android { .. } => Some(Native::Android),
            _ => None,
        }
    }
}

/// The key of a window's EGL surface.
fn layer_key(layer: &GpuLayer) -> Option<usize> {
    match *layer {
        GpuLayer::X11 { window, .. } => Some(window as usize),
        GpuLayer::Android { window, .. } => Some(window as usize),
        _ => None,
    }
}

struct State {
    dpy: EGLDisplay,
    ctx: EGLContext,
    native: Native,
    /// The context's config (window-capable unless `Offscreen`).
    config: EGLConfig,
    /// The configs that make window surfaces, for opaque windows and for
    /// transparent ones (see `alpha`), once found.
    window_configs: [Option<EGLConfig>; 2],
    /// The 1×1 pbuffer the context is current on when it draws to no
    /// window, where EGL takes no context without a surface (else null).
    idle: EGLSurface,
    /// Window surfaces made, by window.
    windows: HashMap<usize, EGLSurface>,
    /// The windows whose surface has been swapped since it was made (a
    /// first swap shows all of it).
    swapped: std::collections::HashSet<usize>,
    /// Which window's frame `frame_target` holds.
    kept: KeptFrame,
    /// `eglSwapBuffersWithDamage{KHR,EXT}` on this display, once looked up.
    swap_damage: Option<Option<SwapWithDamage>>,
    /// Batched quads: boxes only, and boxes with textured quads.
    quad_boxes: Program,
    quads: Program,
    /// Textured quads sampling NV12 (an external image's two planes).
    quads_yuv: Program,
    /// Textured quads sampling an external texture (`samplerExternalOES`:
    /// an Android camera's YUV buffer), where the context has one.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    quads_ext: Option<Program>,
    /// External images imported (`super::external`).
    externals: Imports,
    flat: Program,
    resolve: Program,
    vbo: GLuint,
    max_size: i32,
    /// `GL_RENDERER`.
    renderer: String,
    frame_target: Option<Target>,
    layers: Vec<Target>,
    scratch: Option<Target>,
    textures: HashMap<usize, Texture>,
    /// The atlas pages' textures, by page id.
    atlas: HashMap<u64, AtlasTex>,
    frame: u64,
    /// Texture uploads in the last frame (for tests).
    uploads: usize,
    /// Draw calls in the last frame.
    draws: usize,
    /// Wayland windows' dma-buf rings.
    #[cfg(target_os = "linux")]
    wayland: dmabuf::Wayland,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// `eglSwapBuffersWithDamageKHR` / `EXT`: rectangles of `[x, y, w, h]`,
/// bottom-left origin.
type SwapWithDamage =
    unsafe extern "C" fn(EGLDisplay, EGLSurface, *const EGLint, EGLint) -> EGLBoolean;

/// The display's swap-with-damage entry point, if it has one.
unsafe fn swap_with_damage(dpy: EGLDisplay) -> Option<SwapWithDamage> {
    unsafe {
        let exts = eglQueryString(dpy, EGL_EXTENSIONS);
        if exts.is_null() {
            return None;
        }
        let exts = std::ffi::CStr::from_ptr(exts).to_string_lossy();
        let has = |name: &str| exts.split(' ').any(|e| e == name);
        let name: &std::ffi::CStr = if has("EGL_KHR_swap_buffers_with_damage") {
            c"eglSwapBuffersWithDamageKHR"
        } else if has("EGL_EXT_swap_buffers_with_damage") {
            c"eglSwapBuffersWithDamageEXT"
        } else {
            return None;
        };
        let f = eglGetProcAddress(name.as_ptr());
        (!f.is_null()).then(|| std::mem::transmute::<*const c_void, SwapWithDamage>(f))
    }
}

impl State {
    /// The offscreen context.
    unsafe fn new() -> Result<Self, String> {
        unsafe {
            let dpy = egl_init()?;
            // On the Wayland compositor's GPU, where it named one.
            #[cfg(target_os = "linux")]
            let dpy = dmabuf::on_main_device(dpy);
            let ctx = eglGetCurrentContext();
            let mut state = Self::build(dpy, ctx, Native::Offscreen, core::ptr::null_mut())?;
            // Current on a pbuffer where it can't go without a surface.
            state.idle = eglGetCurrentSurface(EGL_DRAW);
            Ok(state)
        }
    }

    /// A context on the window system `layer` is on, with a config its
    /// window surface takes, and that surface, current.
    unsafe fn on_window(layer: &GpuLayer) -> Result<(Self, EGLSurface), String> {
        unsafe {
            let native = Native::of(layer).ok_or("not an EGL window")?;
            let dpy = window_display(native)?;
            if eglInitialize(dpy, core::ptr::null_mut(), core::ptr::null_mut()) == 0 {
                return Err(format!("eglInitialize failed: {:#x}", eglGetError()));
            }
            eglBindAPI(EGL_OPENGL_ES_API);
            // A config that makes pbuffers too where the context can't go
            // without a surface (the Android emulator's EGL), for drawing
            // to no window ([`make_current_offscreen`]).
            let surfaceless = super::has_egl_extension(dpy, "EGL_KHR_surfaceless_context");
            let kinds: &[EGLint] = if surfaceless {
                &[EGL_WINDOW_BIT]
            } else {
                &[EGL_WINDOW_BIT | EGL_PBUFFER_BIT, EGL_WINDOW_BIT]
            };
            // The first config the window takes (on X11, the one of the
            // window's visual), with an alpha channel for a transparent
            // window.
            let transparent = alpha::layer_alpha(layer);
            let mut found = None;
            for &kind in kinds {
                let configs = alpha::window_configs(dpy, kind, transparent)?;
                found = alpha::first_surface(dpy, &configs, layer);
                if found.is_some() {
                    break;
                }
            }
            let (config, surface) = found.ok_or_else(|| {
                format!(
                    "no EGL config makes a surface on the window: {:#x}",
                    eglGetError()
                )
            })?;
            let ctx_attribs: [EGLint; 3] = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
            let ctx = eglCreateContext(dpy, config, core::ptr::null_mut(), ctx_attribs.as_ptr());
            if ctx.is_null() || eglMakeCurrent(dpy, surface, surface, ctx) == 0 {
                let e = eglGetError();
                eglDestroySurface(dpy, surface);
                return Err(format!("no context on the window's display: {e:#x}"));
            }
            match Self::build(dpy, ctx, native, config) {
                Ok(mut s) => {
                    s.window_configs[transparent as usize] = Some(config);
                    Ok((s, surface))
                }
                Err(e) => {
                    eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
                    eglDestroySurface(dpy, surface);
                    eglDestroyContext(dpy, ctx);
                    Err(e)
                }
            }
        }
    }

    /// Make the context current drawing to no window (surfaceless, or on
    /// its pbuffer).
    unsafe fn make_current_offscreen(&mut self) -> bool {
        unsafe { make_current_offscreen(self.dpy, self.config, self.ctx, &mut self.idle) }
    }

    /// Let go of the context (and everything made in it) and its window
    /// surfaces; the display stays, as other users may share it.
    unsafe fn destroy(mut self) {
        unsafe {
            self.externals.clear(self.dpy);
            #[cfg(target_os = "linux")]
            self.wayland.close();
            eglMakeCurrent(self.dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
            for &s in self.windows.values() {
                eglDestroySurface(self.dpy, s);
            }
            if !self.idle.is_null() {
                eglDestroySurface(self.dpy, self.idle);
            }
            eglDestroyContext(self.dpy, self.ctx);
        }
    }

    /// The programs and buffers, in the current context.
    unsafe fn build(
        dpy: EGLDisplay,
        ctx: EGLContext,
        native: Native,
        config: EGLConfig,
    ) -> Result<Self, String> {
        unsafe {
            let quad_boxes = Program::with(
                QUAD_VS,
                QUAD_ATTRIBS,
                &format!("{QUAD_BOX}{QUADBOX_MAIN}"),
                &[],
            )?;
            let quads = Program::with(
                QUAD_VS,
                QUAD_ATTRIBS,
                &format!("{QUAD_BOX}{QUAD_MAIN}"),
                &["u_tex"],
            )?;
            let quads_yuv = Program::with(
                QUAD_VS,
                QUAD_ATTRIBS,
                &format!("{QUAD_BOX}{}", external::QUAD_YUV_MAIN),
                &["u_tex", "u_tex2", "u_m0", "u_m1", "u_m2"],
            )?;
            // Android's YUV buffers, and dma-bufs a driver samples no other
            // way, are sampled through it.
            let quads_ext = if external::has_oes_external() {
                Program::with(
                    QUAD_VS,
                    QUAD_ATTRIBS,
                    &format!(
                        "{}{QUAD_BOX}{}",
                        external::OES_EXTENSION,
                        external::QUAD_EXT_MAIN
                    ),
                    &["u_tex"],
                )
                .ok()
            } else {
                None
            };
            let flat = Program::new(FLAT_FS, &["u_color"])?;
            let resolve = Program::new(RESOLVE_FS, &["u_tex", "u_inv", "u_tap", "u_color"])?;
            let mut vbo = 0;
            glGenBuffers(1, &mut vbo);
            let (mut max_tex, mut max_rb) = (0, 0);
            glGetIntegerv(GL_MAX_TEXTURE_SIZE, &mut max_tex);
            glGetIntegerv(GL_MAX_RENDERBUFFER_SIZE, &mut max_rb);
            let name = glGetString(GL_RENDERER);
            let renderer = if name.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(name as *const std::ffi::c_char)
                    .to_string_lossy()
                    .into_owned()
            };
            Ok(State {
                dpy,
                ctx,
                native,
                config,
                window_configs: [None; 2],
                idle: core::ptr::null_mut(),
                windows: HashMap::new(),
                swapped: std::collections::HashSet::new(),
                kept: KeptFrame::default(),
                swap_damage: None,
                quad_boxes,
                quads,
                quads_yuv,
                externals: Imports::new(dpy, quads_ext.is_some()),
                quads_ext,
                flat,
                resolve,
                vbo,
                max_size: max_tex.min(max_rb).max(64),
                renderer,
                frame_target: None,
                layers: Vec::new(),
                scratch: None,
                textures: HashMap::new(),
                atlas: HashMap::new(),
                frame: 0,
                uploads: 0,
                draws: 0,
                #[cfg(target_os = "linux")]
                wayland: dmabuf::Wayland::default(),
            })
        }
    }

    fn target(&self, level: &Level) -> &Target {
        match level.layer {
            Some(i) => &self.layers[i],
            None => self.frame_target.as_ref().unwrap(),
        }
    }

    /// Bind `level`'s target with its clip state.
    unsafe fn bind(&self, level: &Level) {
        unsafe {
            let t = self.target(level);
            glBindFramebuffer(GL_FRAMEBUFFER, t.fbo);
            glViewport(0, 0, t.w, t.h);
            let s = level.scissor;
            glEnable(GL_SCISSOR_TEST);
            glScissor(s[0], s[1], (s[2] - s[0]).max(0), (s[3] - s[1]).max(0));
            glStencilMask(0);
            if level.depth > 0 {
                glEnable(GL_STENCIL_TEST);
                glStencilFunc(GL_EQUAL, level.depth as GLint, 0xff);
                glStencilOp(GL_KEEP, GL_KEEP, GL_KEEP);
            } else {
                glDisable(GL_STENCIL_TEST);
            }
            glColorMask(1, 1, 1, 1);
            glEnable(GL_BLEND);
            glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
        }
    }

    /// Upload `verts` (laid out as `prog`'s attributes say: `[x, y, aux_x,
    /// aux_y]` each, or a quad vertex) and point `prog` at them. Attribute
    /// arrays it doesn't use are disabled, so none reads past the buffer.
    unsafe fn vertices(&self, prog: &Program, verts: &[f32]) {
        unsafe {
            glBindBuffer(GL_ARRAY_BUFFER, self.vbo);
            glBufferData(
                GL_ARRAY_BUFFER,
                std::mem::size_of_val(verts) as isize,
                verts.as_ptr() as *const c_void,
                GL_STREAM_DRAW,
            );
            for i in 0..MAX_ATTRIBS {
                glDisableVertexAttribArray(i);
            }
            let f = std::mem::size_of::<f32>();
            let stride = (prog.stride * f) as GLsizei;
            for &(loc, n, at) in &prog.attribs {
                if let Some(loc) = loc {
                    glEnableVertexAttribArray(loc);
                    glVertexAttribPointer(
                        loc,
                        n,
                        GL_FLOAT,
                        GL_FALSE,
                        stride,
                        (at * f) as *const c_void,
                    );
                }
            }
        }
    }

    /// Use `prog`, mapping device pixels of a `w`×`h` target to clip space.
    unsafe fn use_program(prog: &Program, w: i32, h: i32) {
        unsafe {
            glUseProgram(prog.id);
            glUniform4f(prog.loc("u_xf"), 2.0 / w as f32, 2.0 / h as f32, -1.0, -1.0);
        }
    }

    /// The texture for `image` unless it's past the size limit.
    unsafe fn image_texture(&mut self, image: &Arc<Pixmap>) -> Option<GLuint> {
        let size = image.size();
        (size.width as i32 <= self.max_size && size.height as i32 <= self.max_size)
            .then(|| unsafe { self.texture(image) })
    }

    /// The texture for `image`, uploading it unless it is cached.
    unsafe fn texture(&mut self, image: &Arc<Pixmap>) -> GLuint {
        unsafe {
            let key = Arc::as_ptr(image) as usize;
            let frame = self.frame;
            if let Some(t) = self.textures.get_mut(&key) {
                if t.image.upgrade().is_some_and(|i| Arc::ptr_eq(&i, image)) {
                    t.used = frame;
                    return t.tex;
                }
                // The address was reused by another image: upload anew.
                glDeleteTextures(1, &t.tex);
                self.textures.remove(&key);
            }
            let size = image.size();
            let mut tex = 0;
            glGenTextures(1, &mut tex);
            glBindTexture(GL_TEXTURE_2D, tex);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
            glTexImage2D(
                GL_TEXTURE_2D,
                0,
                GL_RGBA as GLint,
                size.width as GLsizei,
                size.height as GLsizei,
                0,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                image.as_bytes().as_ptr() as *const c_void,
            );
            self.uploads += 1;
            self.textures.insert(
                key,
                Texture {
                    image: Arc::downgrade(image),
                    tex,
                    used: frame,
                    bytes: size.width as usize * size.height as usize * 4,
                },
            );
            tex
        }
    }

    /// Update the textures `plan` changed in places (canvases kept as
    /// textures): `glTexSubImage2D` of each rectangle, its rows packed
    /// (GLES2 has no `GL_UNPACK_ROW_LENGTH`).
    unsafe fn update_textures(&mut self, plan: &Plan) {
        unsafe {
            for u in &plan.updates {
                let Some(t) = rekey(&mut self.textures, u, |t| &mut t.image) else {
                    continue;
                };
                glBindTexture(GL_TEXTURE_2D, t.tex);
                glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
                for &r in &u.rects {
                    let rows = u.rows(r);
                    glTexSubImage2D(
                        GL_TEXTURE_2D,
                        0,
                        r[0] as GLint,
                        r[1] as GLint,
                        r[2] as GLsizei,
                        r[3] as GLsizei,
                        GL_RGBA,
                        GL_UNSIGNED_BYTE,
                        rows.as_ptr() as *const c_void,
                    );
                }
            }
        }
    }

    /// Free textures whose image is gone or hasn't been drawn lately.
    unsafe fn sweep_textures(&mut self) {
        let frame = self.frame;
        self.textures.retain(|_, t| {
            let keep =
                t.image.strong_count() > 0 && frame.saturating_sub(t.used) < TEXTURE_KEEP_FRAMES;
            if !keep {
                unsafe { glDeleteTextures(1, &t.tex) };
            }
            keep
        });
    }

    /// Bring the atlas pages' textures up to date with `pages` (only what
    /// changed since the version each holds), and delete those of pages
    /// gone.
    unsafe fn sync_atlas(&mut self, pages: &[AtlasPage]) {
        unsafe {
            self.atlas.retain(|id, t| {
                let keep = pages.iter().any(|p| p.id == *id);
                if !keep {
                    glDeleteTextures(1, &t.tex);
                }
                keep
            });
            glPixelStorei(GL_UNPACK_ALIGNMENT, 4);
            for p in pages {
                let have = self.atlas.get(&p.id).map(|t| (t.side, t.version));
                match p.upload(have) {
                    Upload::Nothing => {}
                    Upload::Whole => {
                        if let Some(old) = self.atlas.remove(&p.id) {
                            glDeleteTextures(1, &old.tex);
                        }
                        let mut tex = 0;
                        glGenTextures(1, &mut tex);
                        glBindTexture(GL_TEXTURE_2D, tex);
                        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
                        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
                        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
                        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
                        glTexImage2D(
                            GL_TEXTURE_2D,
                            0,
                            GL_RGBA as GLint,
                            p.side as GLsizei,
                            p.side as GLsizei,
                            0,
                            GL_RGBA,
                            GL_UNSIGNED_BYTE,
                            p.pixels.as_ptr() as *const c_void,
                        );
                        self.uploads += 1;
                        let (side, version) = (p.side, p.version);
                        self.atlas.insert(p.id, AtlasTex { tex, side, version });
                    }
                    Upload::Region(r) => {
                        let t = self.atlas.get_mut(&p.id).unwrap();
                        // Packed rows: GLES2 has no UNPACK_ROW_LENGTH.
                        let bytes = p.region(r);
                        glBindTexture(GL_TEXTURE_2D, t.tex);
                        glTexSubImage2D(
                            GL_TEXTURE_2D,
                            0,
                            r[0] as GLint,
                            r[1] as GLint,
                            (r[2] - r[0]) as GLsizei,
                            (r[3] - r[1]) as GLsizei,
                            GL_RGBA,
                            GL_UNSIGNED_BYTE,
                            bytes.as_ptr() as *const c_void,
                        );
                        t.version = p.version;
                        self.uploads += 1;
                    }
                }
            }
        }
    }

    /// Draw the quad vertices `verts` (see `crate::gpu::batch`) into
    /// `level` (bound) with one call: sampling `tex` (nearest-neighbor or
    /// bilinear), or boxes only without one.
    unsafe fn draw_quads(
        &mut self,
        level: &Level,
        verts: &[f32],
        tex: Option<GLuint>,
        nearest: bool,
    ) {
        unsafe {
            let s = level.scissor;
            if verts.is_empty() || s[2] <= s[0] || s[3] <= s[1] {
                return;
            }
            let t = self.target(level);
            let p = if tex.is_some() {
                &self.quads
            } else {
                &self.quad_boxes
            };
            Self::use_program(p, t.w, t.h);
            if let Some(tex) = tex {
                glActiveTexture(GL_TEXTURE0);
                glBindTexture(GL_TEXTURE_2D, tex);
                glUniform1i(p.loc("u_tex"), 0);
                let filter = if nearest { GL_NEAREST } else { GL_LINEAR };
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, filter);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, filter);
            }
            self.vertices(p, verts);
            glDrawArrays(GL_TRIANGLES, 0, (verts.len() / QUAD_FLOATS) as GLsizei);
            self.draws += 1;
        }
    }

    /// [`draw_quads`](Self::draw_quads) sampling NV12: luma from `y`,
    /// chroma from `uv`, converted in the shader.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_quads_yuv(
        &mut self,
        level: &Level,
        verts: &[f32],
        y: GLuint,
        uv: GLuint,
        color: crate::render::external::YuvColorSpace,
        nearest: bool,
    ) {
        unsafe {
            let s = level.scissor;
            if verts.is_empty() || s[2] <= s[0] || s[3] <= s[1] {
                return;
            }
            let t = self.target(level);
            let p = &self.quads_yuv;
            Self::use_program(p, t.w, t.h);
            let filter = if nearest { GL_NEAREST } else { GL_LINEAR };
            for (unit, tex, name) in [(1, uv, "u_tex2"), (0, y, "u_tex")] {
                glActiveTexture(GL_TEXTURE0 + unit);
                glBindTexture(GL_TEXTURE_2D, tex);
                glUniform1i(p.loc(name), unit as GLint);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, filter);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, filter);
            }
            let m = color.rows();
            for (i, name) in ["u_m0", "u_m1", "u_m2"].into_iter().enumerate() {
                let r = &m[i * 4..i * 4 + 4];
                glUniform4f(p.loc(name), r[0], r[1], r[2], r[3]);
            }
            self.vertices(p, verts);
            glDrawArrays(GL_TRIANGLES, 0, (verts.len() / QUAD_FLOATS) as GLsizei);
            self.draws += 1;
        }
    }

    /// [`draw_quads`](Self::draw_quads) sampling the external texture
    /// `tex` (`GL_TEXTURE_EXTERNAL_OES`, which converts YUV as it samples).
    unsafe fn draw_quads_ext(&mut self, level: &Level, verts: &[f32], tex: GLuint, nearest: bool) {
        unsafe {
            let s = level.scissor;
            if verts.is_empty() || s[2] <= s[0] || s[3] <= s[1] {
                return;
            }
            let t = self.target(level);
            let Some(p) = &self.quads_ext else {
                return;
            };
            Self::use_program(p, t.w, t.h);
            glActiveTexture(GL_TEXTURE0);
            glBindTexture(external::GL_TEXTURE_EXTERNAL_OES, tex);
            glUniform1i(p.loc("u_tex"), 0);
            let filter = if nearest { GL_NEAREST } else { GL_LINEAR };
            glTexParameteri(
                external::GL_TEXTURE_EXTERNAL_OES,
                GL_TEXTURE_MIN_FILTER,
                filter,
            );
            glTexParameteri(
                external::GL_TEXTURE_EXTERNAL_OES,
                GL_TEXTURE_MAG_FILTER,
                filter,
            );
            self.vertices(p, verts);
            glDrawArrays(GL_TRIANGLES, 0, (verts.len() / QUAD_FLOATS) as GLsizei);
            glBindTexture(external::GL_TEXTURE_EXTERNAL_OES, 0);
            self.draws += 1;
        }
    }

    /// Composite the layer `layer` into `parent` (bound) at its opacity.
    unsafe fn composite(&mut self, layer: &Level, parent: &Level, w: i32, h: i32) {
        unsafe {
            let tex = self.target(layer).tex;
            let mut v = Vec::with_capacity(batch::QUAD_VERTICES * QUAD_FLOATS);
            let resample = layer.resample.as_ref();
            let nearest = batch::layer_quad(&mut v, layer.opacity, resample, w as u32, h as u32);
            self.draw_quads(parent, &v, Some(tex), nearest);
        }
    }

    /// Fill `contours` by stencil-and-cover: stencil the winding into a
    /// supersampled scratch target, cover it to get coverage, then resolve
    /// that into `level` in `color`.
    unsafe fn draw_path(
        &mut self,
        level: &Level,
        contours: &[Vec<[f32; 2]>],
        even_odd: bool,
        color: [f32; 4],
        bbox: [i32; 4],
    ) -> Result<(), String> {
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
                .find(|ss| bw * ss <= self.max_size && bh * ss <= self.max_size)
                .unwrap_or(1);
            let (sw, sh) = (bw * ss, bh * ss);
            if self.scratch.as_ref().is_none_or(|t| t.w < sw || t.h < sh) {
                let (gw, gh) = self.scratch.as_ref().map_or((0, 0), |t| (t.w, t.h));
                let grow = |want: i32, have: i32| {
                    (want.max(have) as u32)
                        .next_power_of_two()
                        .clamp(64, self.max_size as u32) as i32
                };
                let (nw, nh) = (grow(sw, gw), grow(sh, gh));
                if let Some(old) = self.scratch.take() {
                    old.delete();
                }
                self.scratch = Some(Target::new(nw, nh)?);
            }
            let scratch = self.scratch.as_ref().unwrap();
            glBindFramebuffer(GL_FRAMEBUFFER, scratch.fbo);
            glViewport(0, 0, scratch.w, scratch.h);
            glEnable(GL_SCISSOR_TEST);
            glScissor(0, 0, sw, sh);
            glDisable(GL_BLEND);
            glStencilMask(0xff);
            glClearColor(0.0, 0.0, 0.0, 0.0);
            glClearStencil(0);
            glClear(GL_COLOR_BUFFER_BIT | GL_STENCIL_BUFFER_BIT);

            // Device pixels → scratch clip space.
            let f = ss as f32;
            let (vw, vh) = (scratch.w as f32, scratch.h as f32);
            let xf = [
                2.0 * f / vw,
                2.0 * f / vh,
                -2.0 * f * b[0] as f32 / vw - 1.0,
                -2.0 * f * b[1] as f32 / vh - 1.0,
            ];
            let p = &self.flat;
            glUseProgram(p.id);
            glUniform4f(p.loc("u_xf"), xf[0], xf[1], xf[2], xf[3]);

            // Stencil: count the winding of every fan triangle.
            glColorMask(0, 0, 0, 0);
            glEnable(GL_STENCIL_TEST);
            glDisable(GL_CULL_FACE);
            glStencilFunc(GL_ALWAYS, 0, 0xff);
            if even_odd {
                glStencilOp(GL_KEEP, GL_KEEP, GL_INVERT);
            } else {
                glStencilOpSeparate(GL_FRONT, GL_KEEP, GL_KEEP, GL_INCR_WRAP);
                glStencilOpSeparate(GL_BACK, GL_KEEP, GL_KEEP, GL_DECR_WRAP);
            }
            for contour in contours {
                let v: Vec<f32> = contour
                    .iter()
                    .flat_map(|q| [q[0], q[1], 0.0, 0.0])
                    .collect();
                self.vertices(p, &v);
                glDrawArrays(GL_TRIANGLE_FAN, 0, contour.len() as GLsizei);
                self.draws += 1;
            }

            // Cover: full coverage wherever the winding says inside.
            glColorMask(1, 1, 1, 1);
            glStencilFunc(GL_NOTEQUAL, 0, if even_odd { 1 } else { 0xff });
            glStencilOp(GL_ZERO, GL_ZERO, GL_ZERO);
            glUniform4f(p.loc("u_color"), 1.0, 1.0, 1.0, 1.0);
            let (x0, y0, x1, y1) = (b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32);
            #[rustfmt::skip]
            let quad = [
                x0, y0, 0.0, 0.0,  x1, y0, 0.0, 0.0,  x0, y1, 0.0, 0.0,
                x0, y1, 0.0, 0.0,  x1, y0, 0.0, 0.0,  x1, y1, 0.0, 0.0,
            ];
            self.vertices(p, &quad);
            glDrawArrays(GL_TRIANGLES, 0, 6);
            self.draws += 1;
            glDisable(GL_STENCIL_TEST);

            // Resolve into the level.
            self.bind(level);
            let t = self.target(level);
            let r = &self.resolve;
            Self::use_program(r, t.w, t.h);
            glActiveTexture(GL_TEXTURE0);
            glBindTexture(GL_TEXTURE_2D, scratch.tex);
            glUniform1i(r.loc("u_tex"), 0);
            glUniform2f(r.loc("u_inv"), 1.0 / vw, 1.0 / vh);
            glUniform1f(r.loc("u_tap"), if ss > 1 { f * 0.25 } else { 0.0 });
            glUniform4f(r.loc("u_color"), color[0], color[1], color[2], color[3]);
            let s = |x: f32, y: f32| [x, y, (x - x0) * f, (y - y0) * f];
            let v: Vec<f32> = [
                s(x0, y0),
                s(x1, y0),
                s(x0, y1),
                s(x0, y1),
                s(x1, y0),
                s(x1, y1),
            ]
            .concat();
            self.vertices(r, &v);
            glDrawArrays(GL_TRIANGLES, 0, 6);
            self.draws += 1;
            Ok(())
        }
    }

    /// Draw the polygon `pts` into the stencil of `level`'s target, from
    /// clip depth `from` with `op`.
    unsafe fn stencil_polygon(&mut self, level: &Level, pts: &[[f32; 2]], from: u32, op: GLenum) {
        unsafe {
            let t = self.target(level);
            let p = &self.flat;
            Self::use_program(p, t.w, t.h);
            glColorMask(0, 0, 0, 0);
            glEnable(GL_STENCIL_TEST);
            glStencilMask(0xff);
            glStencilFunc(GL_EQUAL, from as GLint, 0xff);
            glStencilOp(GL_KEEP, GL_KEEP, op);
            let v: Vec<f32> = pts.iter().flat_map(|q| [q[0], q[1], 0.0, 0.0]).collect();
            self.vertices(p, &v);
            glDrawArrays(GL_TRIANGLE_FAN, 0, pts.len() as GLsizei);
            self.draws += 1;
            glColorMask(1, 1, 1, 1);
        }
    }

    /// Render `plan` and read the frame back.
    unsafe fn render(&mut self, plan: &Plan) -> Result<Pixmap, String> {
        unsafe {
            // The target no longer holds a window's frame.
            self.kept.forget();
            let (w, h) = self.draw(plan)?;
            // Read back; device row 0 is GL row 0 here, so rows come out
            // top-first. Undo the premultiplication.
            glBindFramebuffer(GL_FRAMEBUFFER, self.frame_target.as_ref().unwrap().fbo);
            glPixelStorei(GL_PACK_ALIGNMENT, 4);
            let mut data = vec![0u8; w as usize * h as usize * 4];
            glReadPixels(
                0,
                0,
                w,
                h,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                data.as_mut_ptr() as *mut c_void,
            );
            let err = glGetError();
            if err != GL_NO_ERROR {
                return Err(format!("GL error {err:#x} rendering the scene"));
            }
            for px in data.as_chunks_mut::<4>().0 {
                let a = px[3];
                if a != 0 && a != 255 {
                    for c in &mut px[..3] {
                        *c = ((*c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
                    }
                }
            }
            Ok(Pixmap::from_rgba8(
                PhysicalSize::new(w as u32, h as u32),
                data,
            ))
        }
    }

    /// Render `plan` into window `key`'s surface `surface` (current) at
    /// `offset` in it, the rest the background, and swap. `dirty` is what
    /// changed since the window's last frame (see [`Executor::present`]).
    unsafe fn present(
        &mut self,
        plan: &mut Plan,
        key: usize,
        surface: EGLSurface,
        offset: (i32, i32),
        dirty: Option<[i32; 4]>,
    ) -> Result<(), String> {
        unsafe {
            // Only what changed, when the target holds the window's last
            // frame (a new size makes a new target, which holds none).
            let size = (plan.size.width as i32, plan.size.height as i32);
            if self
                .frame_target
                .as_ref()
                .is_some_and(|t| (t.w, t.h) == size)
            {
                self.kept.limit(key, plan, dirty);
            }
            let drawn = self.draw(plan);
            if drawn.is_err() {
                self.kept.forget();
            }
            let (w, h) = drawn?;
            let (mut sw, mut sh) = (0, 0);
            eglQuerySurface(self.dpy, surface, EGL_WIDTH, &mut sw);
            eglQuerySurface(self.dpy, surface, EGL_HEIGHT, &mut sh);
            if sw <= 0 || sh <= 0 {
                return Ok(());
            }
            glBindFramebuffer(GL_FRAMEBUFFER, 0);
            glViewport(0, 0, sw, sh);
            glDisable(GL_SCISSOR_TEST);
            glDisable(GL_STENCIL_TEST);
            glDisable(GL_BLEND);
            glColorMask(1, 1, 1, 1);
            let bg = plan.background;
            let a = bg.a as f32 / 255.0;
            glClearColor(
                bg.r as f32 / 255.0 * a,
                bg.g as f32 / 255.0 * a,
                bg.b as f32 / 255.0 * a,
                a,
            );
            glClear(GL_COLOR_BUFFER_BIT);
            // The frame as one quad of the quad program: device pixels of
            // the window to clip space with y up (the window's rows run
            // bottom-up), sampling the frame target (premultiplied, whose
            // row 0 is device row 0) nearest, its edges not antialiased.
            let p = &self.quads;
            glUseProgram(p.id);
            glUniform4f(p.loc("u_xf"), 2.0 / sw as f32, -2.0 / sh as f32, -1.0, 1.0);
            let mut v = Vec::with_capacity(batch::QUAD_VERTICES * QUAD_FLOATS);
            batch::tex_quad(
                &mut v,
                &Transform2D::translate(offset.0 as f32, offset.1 as f32),
                [0.0, 0.0, w as f32, h as f32],
                1.0,
                true,
                false,
                &Sample::WHOLE,
            );
            glActiveTexture(GL_TEXTURE0);
            glBindTexture(GL_TEXTURE_2D, self.frame_target.as_ref().unwrap().tex);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
            glUniform1i(p.loc("u_tex"), 0);
            self.vertices(p, &v);
            glDrawArrays(GL_TRIANGLES, 0, 6);
            let err = glGetError();
            if err != GL_NO_ERROR {
                return Err(format!("GL error {err:#x} presenting the scene"));
            }
            // The window shows all of a new surface's first frame, and only
            // what changed after that, where EGL takes the damage.
            if self.swap_damage.is_none() {
                self.swap_damage = Some(swap_with_damage(self.dpy));
            }
            let damage = dirty.filter(|_| self.swapped.contains(&key)).map(|r| {
                [
                    offset.0 + r[0],
                    sh - (offset.1 + r[3]),
                    r[2] - r[0],
                    r[3] - r[1],
                ]
            });
            let swapped = match (self.swap_damage.flatten(), damage) {
                (Some(swap), Some(rect)) => swap(self.dpy, surface, rect.as_ptr(), 1),
                _ => eglSwapBuffers(self.dpy, surface),
            };
            if swapped == 0 {
                self.kept.forget();
                return Err(format!("eglSwapBuffers failed: {:#x}", eglGetError()));
            }
            self.swapped.insert(key);
            self.kept.presented(key, (w as u32, h as u32), dirty);
            Ok(())
        }
    }

    /// Draw `plan` into the frame target; returns its size.
    unsafe fn draw(&mut self, plan: &Plan) -> Result<(i32, i32), String> {
        unsafe {
            let (w, h) = (plan.size.width as i32, plan.size.height as i32);
            if w <= 0 || h <= 0 {
                return Err("empty target".into());
            }
            if w > self.max_size || h > self.max_size {
                return Err(format!(
                    "{w}x{h} is past the GL size limit {}",
                    self.max_size
                ));
            }
            if self
                .frame_target
                .as_ref()
                .is_none_or(|t| t.w != w || t.h != h)
            {
                if let Some(old) = self.frame_target.take() {
                    old.delete();
                }
                for old in self.layers.drain(..) {
                    old.delete();
                }
                self.kept.forget();
                self.frame_target = Some(Target::new(w, h)?);
            }
            self.uploads = 0;
            self.draws = 0;
            self.externals.begin(self.dpy, self.frame);
            self.sync_atlas(&plan.atlas);
            self.update_textures(plan);
            let full = [0, 0, w, h];
            let mut levels = vec![Level {
                layer: None,
                opacity: 1.0,
                scissor: full,
                depth: 0,
                undo: Vec::new(),
                resample: None,
            }];
            self.bind(&levels[0]);
            let bg = plan.background;
            let a = bg.a as f32 / 255.0;
            glClearColor(
                bg.r as f32 / 255.0 * a,
                bg.g as f32 / 255.0 * a,
                bg.b as f32 / 255.0 * a,
                a,
            );
            glStencilMask(0xff);
            glClearStencil(0);
            // A partial frame keeps the colors the target holds.
            if plan.partial.is_some() {
                glClear(GL_STENCIL_BUFFER_BIT);
            } else {
                glClear(GL_COLOR_BUFFER_BIT | GL_STENCIL_BUFFER_BIT);
            }

            for cmd in &plan.cmds {
                let op = match cmd {
                    Cmd::Quads(q) => {
                        let mut chroma = None;
                        let mut oes = false;
                        let tex = match &q.texture {
                            None => None,
                            Some(TexRef::Image(image)) => self.image_texture(image),
                            Some(TexRef::Atlas(i)) => plan
                                .atlas
                                .get(*i)
                                .and_then(|p| self.atlas.get(&p.id))
                                .map(|t| t.tex),
                            Some(TexRef::External(image)) => {
                                match self.externals.get(self.dpy, image, self.frame) {
                                    Some(Planes::One(t)) => Some(t),
                                    Some(Planes::Yuv(y, uv, color)) => {
                                        chroma = Some((uv, color));
                                        Some(y)
                                    }
                                    Some(Planes::External(t)) => {
                                        oes = true;
                                        Some(t)
                                    }
                                    // Its pixels, read on the CPU.
                                    None => image.pixmap().and_then(|p| self.image_texture(&p)),
                                }
                            }
                        };
                        let n = QUAD_FLOATS;
                        let verts =
                            &plan.quads[q.first as usize * n..(q.first + q.count) as usize * n];
                        let level = levels.pop().unwrap();
                        match (tex, chroma) {
                            (Some(y), Some((uv, color))) => {
                                self.draw_quads_yuv(&level, verts, y, uv, color, q.nearest)
                            }
                            (Some(t), None) if oes => {
                                self.draw_quads_ext(&level, verts, t, q.nearest)
                            }
                            _ => self.draw_quads(&level, verts, tex, q.nearest),
                        }
                        levels.push(level);
                        continue;
                    }
                    Cmd::Op(i) => &plan.ops[*i],
                };
                let level = levels.last().unwrap();
                match op {
                    // Batched into `Cmd::Quads`.
                    Op::Box { .. } | Op::Image { .. } => {}
                    Op::Path {
                        contours,
                        even_odd,
                        color,
                        bbox,
                    } => {
                        let level = levels.pop().unwrap();
                        let r = self.draw_path(&level, contours, *even_odd, *color, *bbox);
                        levels.push(level);
                        r?;
                    }
                    Op::PushClip(Clip::Rect(r)) => {
                        let level = levels.last_mut().unwrap();
                        let s = level.scissor;
                        level.undo.push(Undo::Scissor(s));
                        // GL's scissor is in its own rows; ours run the same
                        // way (device row 0 is GL row 0 in these targets).
                        level.scissor = [
                            s[0].max(r[0]),
                            s[1].max(r[1]),
                            s[2].min(r[2]),
                            s[3].min(r[3]),
                        ];
                        if level.scissor[2] < level.scissor[0] {
                            level.scissor[2] = level.scissor[0];
                        }
                        if level.scissor[3] < level.scissor[1] {
                            level.scissor[3] = level.scissor[1];
                        }
                        let level = levels.last().unwrap();
                        self.bind(level);
                    }
                    Op::PushClip(Clip::Polygon(pts)) => {
                        let mut level = levels.pop().unwrap();
                        self.stencil_polygon(&level, pts, level.depth, GL_INCR);
                        level.depth += 1;
                        level.undo.push(Undo::Stencil(pts.clone()));
                        self.bind(&level);
                        levels.push(level);
                    }
                    Op::PopClip => {
                        let mut level = levels.pop().unwrap();
                        match level.undo.pop() {
                            Some(Undo::Scissor(s)) => level.scissor = s,
                            Some(Undo::Stencil(pts)) => {
                                self.stencil_polygon(&level, &pts, level.depth, GL_DECR);
                                level.depth -= 1;
                            }
                            None => {}
                        }
                        self.bind(&level);
                        levels.push(level);
                    }
                    Op::PushLayer { .. } | Op::PushResampled(_) => {
                        let index = levels.iter().filter(|l| l.layer.is_some()).count();
                        if self.layers.len() <= index {
                            self.layers.push(Target::new(w, h)?);
                        }
                        let (opacity, resample) = match op {
                            Op::PushResampled(r) => (1.0, Some(*r)),
                            Op::PushLayer { opacity } => (*opacity, None),
                            _ => unreachable!(),
                        };
                        // A resampled layer draws in its own pixels.
                        let scissor = resample.as_ref().map_or(level.scissor, Resample::scissor);
                        let layer = Level {
                            layer: Some(index),
                            opacity,
                            scissor,
                            depth: 0,
                            undo: Vec::new(),
                            resample,
                        };
                        self.bind(&layer);
                        glStencilMask(0xff);
                        glClearColor(0.0, 0.0, 0.0, 0.0);
                        glClear(GL_COLOR_BUFFER_BIT | GL_STENCIL_BUFFER_BIT);
                        glStencilMask(0);
                        levels.push(layer);
                    }
                    Op::PopLayer => {
                        if levels.len() < 2 {
                            continue;
                        }
                        let layer = levels.pop().unwrap();
                        let parent = levels.pop().unwrap();
                        self.bind(&parent);
                        self.composite(&layer, &parent, w, h);
                        levels.push(parent);
                    }
                }
            }
            // Close any layers left open.
            while levels.len() > 1 {
                let layer = levels.pop().unwrap();
                let parent = levels.pop().unwrap();
                self.bind(&parent);
                self.composite(&layer, &parent, w, h);
                levels.push(parent);
            }

            self.sweep_textures();
            self.frame += 1;
            Ok((w, h))
        }
    }
}

/// The GLES2 executor. Its context and caches are the calling thread's
/// (EGL contexts are bound per thread), shared by every executor on it.
pub(crate) struct GlExecutor;

impl GlExecutor {
    /// Set up (or reuse) the calling thread's context.
    pub(crate) fn new() -> Result<Self, GpuError> {
        with_state(|_| Ok(()))?;
        Ok(GlExecutor)
    }
}

/// Run `f` on the calling thread's GL state, made current.
fn with_state<T>(f: impl FnOnce(&mut State) -> Result<T, GpuError>) -> Result<T, GpuError> {
    STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(unsafe { State::new() }.map_err(GpuError::Unavailable)?);
        }
        let state = slot.as_mut().unwrap();
        unsafe {
            if !state.make_current_offscreen() {
                return Err(GpuError::Lost(format!(
                    "eglMakeCurrent failed: {:#x}",
                    eglGetError()
                )));
            }
        }
        f(state)
    })
}

/// The EGL display of the window system `native` names.
unsafe fn window_display(native: Native) -> Result<EGLDisplay, String> {
    unsafe {
        let dpy = match native {
            #[cfg(target_os = "linux")]
            Native::Xcb => {
                let (conn, screen) = crate::gpu::xcb::connection()?;
                let attribs = [
                    EGL_PLATFORM_XCB_SCREEN_EXT as EGLAttrib,
                    screen as EGLAttrib,
                    EGL_NONE as EGLAttrib,
                ];
                eglGetPlatformDisplay(EGL_PLATFORM_XCB_EXT, conn, attribs.as_ptr())
            }
            #[cfg(target_os = "android")]
            Native::Android => eglGetDisplay(core::ptr::null_mut()),
            _ => return Err("no EGL window system here".into()),
        };
        if dpy.is_null() {
            return Err(format!(
                "no EGL display for the window: {:#x}",
                eglGetError()
            ));
        }
        Ok(dpy)
    }
}

/// A window surface on `layer`'s window with `config`, or null.
unsafe fn create_window_surface(
    dpy: EGLDisplay,
    config: EGLConfig,
    layer: &GpuLayer,
) -> EGLSurface {
    unsafe {
        match *layer {
            #[cfg(target_os = "linux")]
            GpuLayer::X11 { window, .. } => {
                // The XCB platform takes a pointer to the `xcb_window_t`.
                let mut id = window;
                eglCreatePlatformWindowSurface(
                    dpy,
                    config,
                    (&mut id as *mut u32).cast(),
                    core::ptr::null(),
                )
            }
            #[cfg(target_os = "android")]
            GpuLayer::Android { window, .. } => {
                eglCreateWindowSurface(dpy, config, window, core::ptr::null())
            }
            _ => {
                let _ = (dpy, config);
                core::ptr::null_mut()
            }
        }
    }
}

/// Whether EGL can make surfaces on `layer`'s window system.
fn can_present_to(layer: &GpuLayer) -> bool {
    match *layer {
        #[cfg(target_os = "linux")]
        GpuLayer::X11 { .. } => {
            static XCB: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *XCB.get_or_init(|| unsafe {
                // Client extensions: EGL's XCB platform (Mesa 21 and later).
                let s = eglQueryString(core::ptr::null_mut(), EGL_EXTENSIONS);
                !s.is_null()
                    && std::ffi::CStr::from_ptr(s)
                        .to_string_lossy()
                        .split(' ')
                        .any(|e| e == "EGL_EXT_platform_xcb")
            }) && crate::gpu::xcb::connection().is_ok()
        }
        #[cfg(target_os = "android")]
        GpuLayer::Android { window, .. } => !window.is_null(),
        _ => false,
    }
}

/// Run `f` on the calling thread's GL state, current on `layer`'s window
/// surface (moving the context to the window's display first if it isn't
/// there).
fn with_window<T>(
    layer: &GpuLayer,
    f: impl FnOnce(&mut State, EGLSurface) -> Result<T, GpuError>,
) -> Result<T, GpuError> {
    let key = layer_key(layer).ok_or_else(|| GpuError::Unavailable("no window".into()))?;
    let native = Native::of(layer).ok_or_else(|| GpuError::Unavailable("no window".into()))?;
    STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        unsafe {
            if slot.as_ref().is_none_or(|s| s.native != native) {
                if let Some(old) = slot.take() {
                    old.destroy();
                }
                let (mut state, surface) = State::on_window(layer).map_err(|e| {
                    if alpha::layer_alpha(layer) {
                        alpha::refuse(layer, &e)
                    } else {
                        GpuError::Unavailable(e)
                    }
                })?;
                state.windows.insert(key, surface);
                *slot = Some(state);
            }
            let state = slot.as_mut().unwrap();
            let surface = match state.windows.get(&key) {
                Some(&s) => s,
                None => {
                    let s = state.window_surface(layer);
                    if s.is_null() && alpha::layer_alpha(layer) {
                        return Err(alpha::refuse(layer, "no EGL config with alpha"));
                    }
                    if s.is_null() {
                        return Err(GpuError::Unavailable(format!(
                            "no EGL surface on the window: {:#x}",
                            eglGetError()
                        )));
                    }
                    state.windows.insert(key, s);
                    state.swapped.remove(&key);
                    s
                }
            };
            if eglMakeCurrent(state.dpy, surface, surface, state.ctx) == 0 {
                let e = eglGetError();
                // The window is gone or broken: its surface goes too.
                state.windows.remove(&key);
                state.make_current_offscreen();
                eglDestroySurface(state.dpy, surface);
                // A surface with alpha the context can't draw into.
                if alpha::layer_alpha(layer) {
                    return Err(alpha::refuse(layer, &format!("eglMakeCurrent: {e:#x}")));
                }
                return Err(GpuError::Lost(format!("eglMakeCurrent failed: {e:#x}")));
            }
            f(state, surface)
        }
    })
}

impl Executor for GlExecutor {
    fn api(&self) -> Api {
        Api::Gl
    }

    fn read_back(&mut self, plan: &Plan) -> Result<Pixmap, GpuError> {
        with_state(|state| unsafe { state.render(plan) }.map_err(GpuError::Lost))
    }

    fn can_present(&self, layer: &GpuLayer) -> bool {
        #[cfg(target_os = "linux")]
        if let Some((_, sink)) = crate::gpu::dmabuf_present::sink_of(layer) {
            return with_state(|state| Ok(unsafe { state.can_present_dmabuf(sink) }))
                .unwrap_or(false);
        }
        can_present_to(layer) && !alpha::refused(layer)
    }

    fn present(
        &mut self,
        plan: &mut Plan,
        layer: &GpuLayer,
        _scale: crate::geometry::ScaleFactor,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        // A Wayland window: through a dma-buf, on the offscreen context.
        #[cfg(target_os = "linux")]
        if let Some((surface, sink)) = crate::gpu::dmabuf_present::sink_of(layer) {
            return with_state(|state| unsafe { state.present_dmabuf(plan, surface, sink, dirty) });
        }
        let offset = match *layer {
            GpuLayer::Android { content, .. } => (content[0], content[1]),
            _ => (0, 0),
        };
        let key = layer_key(layer).unwrap_or(0);
        with_window(layer, |state, surface| {
            unsafe { state.present(plan, key, surface, offset, dirty) }.map_err(GpuError::Lost)
        })
    }

    fn release(&mut self, layer: &GpuLayer) {
        #[cfg(target_os = "linux")]
        if let Some((surface, sink)) = crate::gpu::dmabuf_present::sink_of(layer) {
            let _ = with_state(|state| {
                unsafe { state.release_dmabuf(surface, sink) };
                Ok(())
            });
            return;
        }
        alpha::forget(layer);
        let Some(key) = layer_key(layer) else {
            return;
        };
        STATE.with(|cell| {
            if let Ok(mut slot) = cell.try_borrow_mut()
                && let Some(state) = slot.as_mut()
                && let Some(surface) = state.windows.remove(&key)
            {
                state.swapped.remove(&key);
                unsafe {
                    state.make_current_offscreen();
                    eglDestroySurface(state.dpy, surface);
                }
            }
        });
    }

    fn uploads(&self) -> usize {
        STATE.with(|cell| cell.borrow().as_ref().map_or(0, |s| s.uploads))
    }

    fn draws(&self) -> usize {
        STATE.with(|cell| cell.borrow().as_ref().map_or(0, |s| s.draws))
    }

    fn device(&self) -> String {
        STATE.with(|cell| {
            cell.borrow()
                .as_ref()
                .map_or_else(String::new, |s| s.renderer.clone())
        })
    }

    fn texture_bytes(&self) -> usize {
        STATE.with(|cell| {
            cell.borrow().as_ref().map_or(0, |s| {
                s.textures.values().map(|t| t.bytes).sum::<usize>()
                    + s.atlas
                        .values()
                        .map(|a| a.side as usize * a.side as usize * 4)
                        .sum::<usize>()
            })
        })
    }
}
