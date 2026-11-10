//! The GLES2 scene renderer: executes a lowered [`Plan`](crate::gpu::ops::Plan)
//! on a persistent surfaceless context.
//!
//! Everything is drawn in premultiplied alpha into framebuffer objects that
//! carry a stencil buffer: boxes through an analytic-coverage shader, paths by
//! stencil-and-cover into a supersampled scratch target that is then resolved
//! (averaged) into the frame, images and glyph rasters as textured quads, and
//! layers into pooled offscreen targets composited at their opacity. Clips are
//! scissor rectangles or stencil polygons.
//!
//! The context, programs, targets and textures live in a thread-local, so a
//! renderer set with `App::render_with` keeps them from frame to frame.
//! Textures for images are cached by the image's identity (its `Arc`): kept
//! while the image is alive and drawn, re-uploaded if it is replaced, and
//! freed once it is dropped or hasn't been drawn for a while.

use super::*;
use crate::geometry::ScaleFactor;
use crate::gpu::ops::{self, Clip, Op, Plan, RasterCache};
use crate::render::Scene;
use oxideav_core::Transform2D;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

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
const GL_MAX_RENDERBUFFER_SIZE: GLenum = 0x84E8;
const GL_TRIANGLE_FAN: GLenum = 0x0006;
const GL_STREAM_DRAW: GLenum = 0x88E0;
const GL_UNPACK_ALIGNMENT: GLenum = 0x0CF5;
const GL_PACK_ALIGNMENT: GLenum = 0x0D05;
const GL_CULL_FACE: GLenum = 0x0B44;
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
    fn glPixelStorei(pname: GLenum, param: GLint);
}

// Every program shares one vertex shader: `a_pos` in device pixels mapped to
// clip space by `u_xf` (scale.xy, offset.zw), and a pass-through `a_aux`.
const VS: &[u8] = b"attribute vec2 a_pos;\n\
attribute vec2 a_aux;\n\
uniform vec4 u_xf;\n\
varying vec2 v_aux;\n\
void main() {\n\
  v_aux = a_aux;\n\
  gl_Position = vec4(a_pos * u_xf.xy + u_xf.zw, 0.0, 1.0);\n\
}\n\0";

const PRECISION: &str = "#ifdef GL_FRAGMENT_PRECISION_HIGH\nprecision highp float;\n#else\nprecision mediump float;\n#endif\n";

// A box: `v_aux` is the device position. Coverage is the exact overlap of the
// pixel with a sharp box (or ring), or a signed distance for rounded corners.
const BOX_FS: &str = "varying vec2 v_aux;\n\
uniform vec4 u_rect;\n\
uniform float u_radius;\n\
uniform float u_stroke;\n\
uniform vec4 u_color;\n\
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
void main() {\n\
  vec2 c = (u_rect.xy + u_rect.zw) * 0.5;\n\
  vec2 h = (u_rect.zw - u_rect.xy) * 0.5;\n\
  vec2 p = v_aux - c;\n\
  float w = u_stroke * 0.5;\n\
  float cov;\n\
  if (u_radius <= 0.0) {\n\
    if (u_stroke > 0.0) {\n\
      cov = boxcov(p, h + vec2(w)) - boxcov(p, h - vec2(w));\n\
    } else {\n\
      cov = boxcov(p, h);\n\
    }\n\
  } else {\n\
    float r = min(u_radius, min(h.x, h.y));\n\
    float d = sdf(p, h, r);\n\
    if (u_stroke > 0.0) {\n\
      cov = clamp(0.5 - (d - w), 0.0, 1.0) - clamp(0.5 - (d + w), 0.0, 1.0);\n\
    } else {\n\
      cov = clamp(0.5 - d, 0.0, 1.0);\n\
    }\n\
  }\n\
  gl_FragColor = u_color * cov;\n\
}\n";

// A textured quad: `v_aux` is the local position inside `u_rect`, `u_px` the
// device pixels per local unit along each axis (for edge coverage).
const TEX_FS: &str = "varying vec2 v_aux;\n\
uniform sampler2D u_tex;\n\
uniform vec4 u_rect;\n\
uniform vec2 u_px;\n\
uniform float u_opacity;\n\
uniform float u_premul;\n\
uniform float u_aa;\n\
void main() {\n\
  vec2 uv = (v_aux - u_rect.xy) / (u_rect.zw - u_rect.xy);\n\
  vec4 t = texture2D(u_tex, uv);\n\
  if (u_premul < 0.5) { t.rgb *= t.a; }\n\
  float cov = 1.0;\n\
  if (u_aa > 0.5) {\n\
    vec2 d = min(v_aux - u_rect.xy, u_rect.zw - v_aux) * u_px;\n\
    cov = clamp(d.x + 0.5, 0.0, 1.0) * clamp(d.y + 0.5, 0.0, 1.0);\n\
  }\n\
  gl_FragColor = t * (u_opacity * cov);\n\
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

/// A linked program and the locations it uses.
struct Program {
    id: GLuint,
    a_pos: GLuint,
    a_aux: Option<GLuint>,
    u: HashMap<&'static str, GLint>,
}

impl Program {
    unsafe fn new(fs_body: &str, uniforms: &[&'static str]) -> Result<Self, String> {
        unsafe {
            let vs = compile(GL_VERTEX_SHADER, VS)?;
            let src = format!("{PRECISION}{fs_body}\0");
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
            let a_pos = glGetAttribLocation(id, c"a_pos".as_ptr());
            let a_aux = glGetAttribLocation(id, c"a_aux".as_ptr());
            if a_pos < 0 {
                return Err("scene program: no a_pos".into());
            }
            let mut u = HashMap::new();
            for name in uniforms.iter().copied().chain(["u_xf"]) {
                let c = std::ffi::CString::new(name).unwrap();
                u.insert(name, glGetUniformLocation(id, c.as_ptr()));
            }
            Ok(Self {
                id,
                a_pos: a_pos as GLuint,
                a_aux: (a_aux >= 0).then_some(a_aux as GLuint),
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
}

struct State {
    dpy: EGLDisplay,
    ctx: EGLContext,
    boxes: Program,
    tex: Program,
    flat: Program,
    resolve: Program,
    vbo: GLuint,
    max_size: i32,
    frame_target: Option<Target>,
    layers: Vec<Target>,
    scratch: Option<Target>,
    textures: HashMap<usize, Texture>,
    rasters: RasterCache,
    frame: u64,
    /// Texture uploads in the last frame (for tests).
    uploads: usize,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

impl State {
    unsafe fn new() -> Result<Self, String> {
        unsafe {
            let dpy = egl_init()?;
            let ctx = eglGetCurrentContext();
            let boxes = Program::new(BOX_FS, &["u_rect", "u_radius", "u_stroke", "u_color"])?;
            let tex = Program::new(
                TEX_FS,
                &["u_tex", "u_rect", "u_px", "u_opacity", "u_premul", "u_aa"],
            )?;
            let flat = Program::new(FLAT_FS, &["u_color"])?;
            let resolve = Program::new(RESOLVE_FS, &["u_tex", "u_inv", "u_tap", "u_color"])?;
            let mut vbo = 0;
            glGenBuffers(1, &mut vbo);
            let (mut max_tex, mut max_rb) = (0, 0);
            glGetIntegerv(GL_MAX_TEXTURE_SIZE, &mut max_tex);
            glGetIntegerv(GL_MAX_RENDERBUFFER_SIZE, &mut max_rb);
            Ok(State {
                dpy,
                ctx,
                boxes,
                tex,
                flat,
                resolve,
                vbo,
                max_size: max_tex.min(max_rb).max(64),
                frame_target: None,
                layers: Vec::new(),
                scratch: None,
                textures: HashMap::new(),
                rasters: RasterCache::default(),
                frame: 0,
                uploads: 0,
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

    /// Upload `verts` (`[x, y, aux_x, aux_y]` each) and point `prog` at them.
    unsafe fn vertices(&self, prog: &Program, verts: &[f32]) {
        unsafe {
            glBindBuffer(GL_ARRAY_BUFFER, self.vbo);
            glBufferData(
                GL_ARRAY_BUFFER,
                std::mem::size_of_val(verts) as isize,
                verts.as_ptr() as *const c_void,
                GL_STREAM_DRAW,
            );
            let stride = 4 * std::mem::size_of::<f32>() as GLsizei;
            glEnableVertexAttribArray(prog.a_pos);
            glVertexAttribPointer(prog.a_pos, 2, GL_FLOAT, GL_FALSE, stride, core::ptr::null());
            if let Some(aux) = prog.a_aux {
                glEnableVertexAttribArray(aux);
                glVertexAttribPointer(
                    aux,
                    2,
                    GL_FLOAT,
                    GL_FALSE,
                    stride,
                    (2 * std::mem::size_of::<f32>()) as *const c_void,
                );
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
                },
            );
            tex
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

    unsafe fn draw_box(
        &self,
        level: &Level,
        rect: [f32; 4],
        radius: f32,
        stroke: f32,
        c: [f32; 4],
    ) {
        unsafe {
            let t = self.target(level);
            let p = &self.boxes;
            Self::use_program(p, t.w, t.h);
            glUniform4f(p.loc("u_rect"), rect[0], rect[1], rect[2], rect[3]);
            glUniform1f(p.loc("u_radius"), radius);
            glUniform1f(p.loc("u_stroke"), stroke);
            glUniform4f(p.loc("u_color"), c[0], c[1], c[2], c[3]);
            let pad = 1.0 + stroke * 0.5;
            let (x0, y0, x1, y1) = (rect[0] - pad, rect[1] - pad, rect[2] + pad, rect[3] + pad);
            #[rustfmt::skip]
            let v = [
                x0, y0, x0, y0,  x1, y0, x1, y0,  x0, y1, x0, y1,
                x0, y1, x0, y1,  x1, y0, x1, y0,  x1, y1, x1, y1,
            ];
            self.vertices(p, &v);
            glDrawArrays(GL_TRIANGLES, 0, 6);
        }
    }

    /// Draw texture `tex` over the local `rect` through `m`; `premul` if its
    /// pixels are premultiplied, `aa` to antialias its edges.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_texture(
        &self,
        level: &Level,
        tex: GLuint,
        m: &Transform2D,
        rect: [f32; 4],
        opacity: f32,
        premul: bool,
        aa: bool,
    ) {
        unsafe {
            let t = self.target(level);
            let p = &self.tex;
            Self::use_program(p, t.w, t.h);
            // Device pixels per local unit along each local axis.
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
            glActiveTexture(GL_TEXTURE0);
            glBindTexture(GL_TEXTURE_2D, tex);
            glUniform1i(p.loc("u_tex"), 0);
            glUniform4f(p.loc("u_rect"), rect[0], rect[1], rect[2], rect[3]);
            glUniform2f(p.loc("u_px"), px.0, px.1);
            glUniform1f(p.loc("u_opacity"), opacity);
            glUniform1f(p.loc("u_premul"), if premul { 1.0 } else { 0.0 });
            glUniform1f(p.loc("u_aa"), if aa { 1.0 } else { 0.0 });
            self.vertices(p, &v);
            glDrawArrays(GL_TRIANGLES, 0, 6);
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
            Ok(())
        }
    }

    /// Draw the polygon `pts` into the stencil of `level`'s target, from
    /// clip depth `from` with `op`.
    unsafe fn stencil_polygon(&self, level: &Level, pts: &[[f32; 2]], from: u32, op: GLenum) {
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
            glColorMask(1, 1, 1, 1);
        }
    }

    /// Render `plan` and read the frame back.
    unsafe fn render(&mut self, plan: &Plan) -> Result<Pixmap, String> {
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
                self.frame_target = Some(Target::new(w, h)?);
            }
            self.uploads = 0;
            let full = [0, 0, w, h];
            let mut levels = vec![Level {
                layer: None,
                opacity: 1.0,
                scissor: full,
                depth: 0,
                undo: Vec::new(),
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
            glClear(GL_COLOR_BUFFER_BIT | GL_STENCIL_BUFFER_BIT);

            for op in &plan.ops {
                let level = levels.last().unwrap();
                match op {
                    Op::Box {
                        rect,
                        radius,
                        stroke,
                        color,
                    } => self.draw_box(level, *rect, *radius, *stroke, *color),
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
                    Op::Image {
                        image,
                        transform,
                        rect,
                        opacity,
                    } => {
                        let level = levels.pop().unwrap();
                        let tex = self.texture(image);
                        self.draw_texture(&level, tex, transform, *rect, *opacity, false, true);
                        levels.push(level);
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
                    Op::PushLayer { opacity } => {
                        let index = levels.iter().filter(|l| l.layer.is_some()).count();
                        if self.layers.len() <= index {
                            self.layers.push(Target::new(w, h)?);
                        }
                        let scissor = level.scissor;
                        let layer = Level {
                            layer: Some(index),
                            opacity: *opacity,
                            scissor,
                            depth: 0,
                            undo: Vec::new(),
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
                        let parent = levels.last().unwrap();
                        self.bind(parent);
                        let tex = self.target(&layer).tex;
                        let full_rect = [0.0, 0.0, w as f32, h as f32];
                        self.draw_texture(
                            parent,
                            tex,
                            &Transform2D::identity(),
                            full_rect,
                            layer.opacity,
                            true,
                            false,
                        );
                    }
                }
            }
            // Close any layers left open.
            while levels.len() > 1 {
                let layer = levels.pop().unwrap();
                let parent = levels.last().unwrap();
                self.bind(parent);
                let tex = self.target(&layer).tex;
                let full_rect = [0.0, 0.0, w as f32, h as f32];
                self.draw_texture(
                    parent,
                    tex,
                    &Transform2D::identity(),
                    full_rect,
                    layer.opacity,
                    true,
                    false,
                );
            }

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
            self.sweep_textures();
            self.frame += 1;
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
}

/// Render `scene` at `scale` through the persistent GL scene renderer.
pub(crate) fn render(
    scene: &Scene,
    background: Color,
    scale: ScaleFactor,
) -> Result<Pixmap, String> {
    STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(unsafe { State::new()? });
        }
        let state = slot.as_mut().unwrap();
        unsafe {
            if eglMakeCurrent(
                state.dpy,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                state.ctx,
            ) == 0
            {
                return Err(format!("eglMakeCurrent failed: {:#x}", eglGetError()));
            }
        }
        let physical = scale.to_physical(scene.logical_size());
        let plan = ops::plan(scene, background, physical, &mut state.rasters);
        unsafe { state.render(&plan) }
    })
}

/// Texture uploads in the calling thread's last frame (for tests).
#[cfg(test)]
pub(crate) fn last_uploads() -> usize {
    STATE.with(|cell| cell.borrow().as_ref().map_or(0, |s| s.uploads))
}
