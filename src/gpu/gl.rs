//! Raw EGL (surfaceless) + OpenGL ES 2 offscreen blitter. Linux-only.
#![allow(unsafe_code)]
#![allow(non_snake_case, non_upper_case_globals)]

use crate::geometry::{PhysicalSize, Rect};
use crate::render::{Color, Pixmap};
use core::ffi::{c_char, c_void};

mod scene;
#[cfg(test)]
pub(crate) use scene::last_uploads;
pub(crate) use scene::render as render_scene_native;

// ---- EGL / GL scalar types --------------------------------------------------

type EGLDisplay = *mut c_void;
type EGLConfig = *mut c_void;
type EGLContext = *mut c_void;
type EGLSurface = *mut c_void;
type EGLint = i32;
type EGLBoolean = u32;
type EGLenum = u32;
type EGLAttrib = isize;

type GLenum = u32;
type GLuint = u32;
type GLint = i32;
type GLsizei = i32;
type GLbitfield = u32;

const EGL_PLATFORM_SURFACELESS_MESA: EGLenum = 0x31DD;
const EGL_PLATFORM_GBM_KHR: EGLenum = 0x31D7;
const EGL_OPENGL_ES_API: EGLenum = 0x30A0;
const EGL_CONTEXT_CLIENT_VERSION: EGLint = 0x3098;
const EGL_NONE: EGLint = 0x3038;
const EGL_SURFACE_TYPE: EGLint = 0x3033;
const EGL_PBUFFER_BIT: EGLint = 0x0001;
const EGL_WINDOW_BIT: EGLint = 0x0004;
const EGL_RENDERABLE_TYPE: EGLint = 0x3040;
const EGL_OPENGL_ES2_BIT: EGLint = 0x0004;
const EGL_RED_SIZE: EGLint = 0x3024;
const EGL_GREEN_SIZE: EGLint = 0x3023;
const EGL_BLUE_SIZE: EGLint = 0x3022;

const GL_TEXTURE_2D: GLenum = 0x0DE1;
const GL_TEXTURE0: GLenum = 0x84C0;
const GL_RGBA: GLenum = 0x1908;
const GL_UNSIGNED_BYTE: GLenum = 0x1401;
const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
const GL_TEXTURE_MAG_FILTER: GLenum = 0x2800;
const GL_TEXTURE_WRAP_S: GLenum = 0x2802;
const GL_TEXTURE_WRAP_T: GLenum = 0x2803;
const GL_LINEAR: GLint = 0x2601;
const GL_NEAREST: GLint = 0x2600;
const GL_CLAMP_TO_EDGE: GLint = 0x812F;
const GL_FRAMEBUFFER: GLenum = 0x8D40;
const GL_COLOR_ATTACHMENT0: GLenum = 0x8CE0;
const GL_FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;
const GL_ARRAY_BUFFER: GLenum = 0x8892;
const GL_STATIC_DRAW: GLenum = 0x88E4;
const GL_FLOAT: GLenum = 0x1406;
const GL_FALSE: u8 = 0;
const GL_VERTEX_SHADER: GLenum = 0x8B31;
const GL_FRAGMENT_SHADER: GLenum = 0x8B30;
const GL_COMPILE_STATUS: GLenum = 0x8B81;
const GL_LINK_STATUS: GLenum = 0x8B82;
const GL_TRIANGLE_STRIP: GLenum = 0x0005;
const GL_TRIANGLES: GLenum = 0x0004;
const GL_COLOR_BUFFER_BIT: GLbitfield = 0x4000;
const GL_BLEND: GLenum = 0x0BE2;
const GL_SRC_ALPHA: GLenum = 0x0302;
const GL_ONE_MINUS_SRC_ALPHA: GLenum = 0x0303;

#[link(name = "EGL")]
unsafe extern "C" {
    fn eglGetPlatformDisplay(
        platform: EGLenum,
        native_display: *mut c_void,
        attrib_list: *const EGLAttrib,
    ) -> EGLDisplay;
    fn eglInitialize(dpy: EGLDisplay, major: *mut EGLint, minor: *mut EGLint) -> EGLBoolean;
    fn eglChooseConfig(
        dpy: EGLDisplay,
        attrib_list: *const EGLint,
        configs: *mut EGLConfig,
        config_size: EGLint,
        num_config: *mut EGLint,
    ) -> EGLBoolean;
    fn eglBindAPI(api: EGLenum) -> EGLBoolean;
    fn eglCreateContext(
        dpy: EGLDisplay,
        config: EGLConfig,
        share: EGLContext,
        attrib_list: *const EGLint,
    ) -> EGLContext;
    fn eglMakeCurrent(
        dpy: EGLDisplay,
        draw: EGLSurface,
        read: EGLSurface,
        ctx: EGLContext,
    ) -> EGLBoolean;
    fn eglGetError() -> EGLint;
    fn eglQueryString(dpy: EGLDisplay, name: EGLint) -> *const c_char;
    fn eglGetCurrentContext() -> EGLContext;
    /// Resolve an EGL/GL extension entry point by name (extensions aren't
    /// guaranteed to be directly linkable symbols).
    fn eglGetProcAddress(procname: *const c_char) -> *const c_void;
}

// Mesa's Generic Buffer Manager — turns a DRM device fd into the `gbm_device`
// EGL needs for a device-specific (`EGL_PLATFORM_GBM`) display, so rendered
// buffers live on the GPU we name (the X server's, via DRI3Open).
#[link(name = "gbm")]
unsafe extern "C" {
    fn gbm_create_device(fd: i32) -> *mut c_void;
}

#[link(name = "GLESv2")]
unsafe extern "C" {
    fn glGenTextures(n: GLsizei, textures: *mut GLuint);
    fn glBindTexture(target: GLenum, texture: GLuint);
    fn glTexParameteri(target: GLenum, pname: GLenum, param: GLint);
    fn glTexImage2D(
        target: GLenum,
        level: GLint,
        internalformat: GLint,
        width: GLsizei,
        height: GLsizei,
        border: GLint,
        format: GLenum,
        ty: GLenum,
        pixels: *const c_void,
    );
    fn glActiveTexture(texture: GLenum);
    fn glGenFramebuffers(n: GLsizei, framebuffers: *mut GLuint);
    fn glBindFramebuffer(target: GLenum, framebuffer: GLuint);
    fn glFramebufferTexture2D(
        target: GLenum,
        attachment: GLenum,
        textarget: GLenum,
        texture: GLuint,
        level: GLint,
    );
    fn glCheckFramebufferStatus(target: GLenum) -> GLenum;
    fn glViewport(x: GLint, y: GLint, width: GLsizei, height: GLsizei);
    fn glClearColor(r: f32, g: f32, b: f32, a: f32);
    fn glClear(mask: GLbitfield);
    fn glEnable(cap: GLenum);
    fn glBlendFunc(sfactor: GLenum, dfactor: GLenum);
    fn glCreateShader(ty: GLenum) -> GLuint;
    fn glShaderSource(
        shader: GLuint,
        count: GLsizei,
        string: *const *const c_char,
        length: *const GLint,
    );
    fn glCompileShader(shader: GLuint);
    fn glGetShaderiv(shader: GLuint, pname: GLenum, params: *mut GLint);
    fn glCreateProgram() -> GLuint;
    fn glAttachShader(program: GLuint, shader: GLuint);
    fn glLinkProgram(program: GLuint);
    fn glGetProgramiv(program: GLuint, pname: GLenum, params: *mut GLint);
    fn glUseProgram(program: GLuint);
    fn glGenBuffers(n: GLsizei, buffers: *mut GLuint);
    fn glBindBuffer(target: GLenum, buffer: GLuint);
    fn glBufferData(target: GLenum, size: isize, data: *const c_void, usage: GLenum);
    fn glGetAttribLocation(program: GLuint, name: *const c_char) -> GLint;
    fn glEnableVertexAttribArray(index: GLuint);
    fn glVertexAttribPointer(
        index: GLuint,
        size: GLint,
        ty: GLenum,
        normalized: u8,
        stride: GLsizei,
        pointer: *const c_void,
    );
    fn glGetUniformLocation(program: GLuint, name: *const c_char) -> GLint;
    fn glUniform1i(location: GLint, v0: GLint);
    fn glUniform1f(location: GLint, v0: f32);
    fn glUniform2f(location: GLint, v0: f32, v1: f32);
    fn glUniform4f(location: GLint, v0: f32, v1: f32, v2: f32, v3: f32);
    fn glDrawArrays(mode: GLenum, first: GLint, count: GLsizei);
    fn glReadPixels(
        x: GLint,
        y: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
        ty: GLenum,
        pixels: *mut c_void,
    );
    fn glFinish();
    fn glGetError() -> GLenum;
}

const VERTEX_SRC: &[u8] = b"attribute vec2 pos;\nattribute vec2 uv;\nvarying vec2 v_uv;\nvoid main() {\n  v_uv = uv;\n  gl_Position = vec4(pos, 0.0, 1.0);\n}\n\0";
const FRAGMENT_SRC: &[u8] = b"precision mediump float;\nvarying vec2 v_uv;\nuniform sampler2D tex;\nvoid main() {\n  gl_FragColor = texture2D(tex, v_uv);\n}\n\0";

// Flat-color program for GPU-native geometry. The fragment shader evaluates a
// rounded-rectangle signed-distance field per pixel (a radius of 0 is a sharp
// rect), so a single program fills both sharp and rounded rectangles.
const FLAT_VERTEX_SRC: &[u8] =
    b"attribute vec2 pos;\nvoid main() {\n  gl_Position = vec4(pos, 0.0, 1.0);\n}\n\0";
const FLAT_FRAGMENT_SRC: &[u8] = b"precision mediump float;\n\
uniform vec4 u_color;\n\
uniform vec2 u_center;\n\
uniform vec2 u_half;\n\
uniform float u_radius;\n\
uniform float u_border;\n\
void main() {\n\
  vec2 p = gl_FragCoord.xy - u_center;\n\
  vec2 d = abs(p) - (u_half - vec2(u_radius));\n\
  float dist = length(max(d, vec2(0.0))) - u_radius;\n\
  if (dist > 0.5) discard;\n\
  if (u_border > 0.0 && dist < -u_border) discard;\n\
  gl_FragColor = u_color;\n\
}\n\0";

// Text program: sample a coverage mask (white-on-black glyph raster, so the red
// channel is coverage) and emit `u_color` modulated by it, for alpha blending.
const TEXT_FRAGMENT_SRC: &[u8] = b"precision mediump float;\n\
varying vec2 v_uv;\n\
uniform sampler2D u_mask;\n\
uniform vec4 u_color;\n\
void main() {\n\
  float cov = texture2D(u_mask, v_uv).r;\n\
  gl_FragColor = vec4(u_color.rgb, u_color.a * cov);\n\
}\n\0";

unsafe fn compile(kind: GLenum, src: &[u8]) -> Result<GLuint, String> {
    unsafe {
        let sh = glCreateShader(kind);
        let ptr = src.as_ptr() as *const c_char;
        glShaderSource(sh, 1, &ptr, core::ptr::null());
        glCompileShader(sh);
        let mut ok: GLint = 0;
        glGetShaderiv(sh, GL_COMPILE_STATUS, &mut ok);
        if ok == 0 {
            return Err(format!("shader compile failed (kind {kind:#x})"));
        }
        Ok(sh)
    }
}

/// Create a surfaceless EGL display + GLES2 context and make it current. Shared
/// by the present and GPU-native-draw paths; runs headlessly under Mesa.
unsafe fn egl_make_current() -> Result<(), String> {
    unsafe { egl_init().map(|_| ()) }
}

/// Like [`egl_make_current`] but returns the [`EGLDisplay`], which the dma-buf
/// import/export path needs for `eglCreateImageKHR` / `eglExportDMABUFImageMESA`.
/// Surfaceless (no GPU device picked explicitly — Mesa chooses).
unsafe fn egl_init() -> Result<EGLDisplay, String> {
    unsafe {
        let dpy = eglGetPlatformDisplay(
            EGL_PLATFORM_SURFACELESS_MESA,
            core::ptr::null_mut(),
            core::ptr::null(),
        );
        if dpy.is_null() {
            return Err("eglGetPlatformDisplay failed (no surfaceless EGL?)".into());
        }
        // Surfaceless Mesa configs advertise PBUFFER_BIT.
        egl_finish(dpy, EGL_PBUFFER_BIT)
    }
}

/// Bring up EGL on a specific GPU via a GBM device created from a DRM fd — so the
/// buffers we render/export live on *that* device. The browser compositor uses
/// the DRM fd from X11 `DRI3Open` (the server's GPU) here, so the server can
/// import our dma-bufs. The `gbm_device` is intentionally leaked (process-lived).
unsafe fn egl_init_gbm(drm_fd: i32) -> Result<EGLDisplay, String> {
    unsafe {
        let gbm = gbm_create_device(drm_fd);
        if gbm.is_null() {
            return Err("gbm_create_device failed (not a DRM render node?)".into());
        }
        let dpy = eglGetPlatformDisplay(EGL_PLATFORM_GBM_KHR, gbm, core::ptr::null());
        if dpy.is_null() {
            return Err("eglGetPlatformDisplay(GBM) failed".into());
        }
        // GBM configs are scanout/window-capable, so match WINDOW_BIT.
        egl_finish(dpy, EGL_WINDOW_BIT)
    }
}

/// Initialize `dpy`, bind GLES2, choose a config (filtered by `surface_type` —
/// the bit the platform's configs advertise; we still render surfaceless to
/// FBOs), and make a surfaceless context current. Shared by both init paths.
unsafe fn egl_finish(dpy: EGLDisplay, surface_type: EGLint) -> Result<EGLDisplay, String> {
    unsafe {
        if eglInitialize(dpy, core::ptr::null_mut(), core::ptr::null_mut()) == 0 {
            return Err(format!("eglInitialize failed: {:#x}", eglGetError()));
        }
        eglBindAPI(EGL_OPENGL_ES_API);
        // eglChooseConfig defaults EGL_SURFACE_TYPE to WINDOW_BIT, so we must set
        // it explicitly to whatever the chosen platform's configs support.
        let cfg_attribs: [EGLint; 11] = [
            EGL_SURFACE_TYPE,
            surface_type,
            EGL_RENDERABLE_TYPE,
            EGL_OPENGL_ES2_BIT,
            EGL_RED_SIZE,
            8,
            EGL_GREEN_SIZE,
            8,
            EGL_BLUE_SIZE,
            8,
            EGL_NONE,
        ];
        let mut config: EGLConfig = core::ptr::null_mut();
        let mut num: EGLint = 0;
        if eglChooseConfig(dpy, cfg_attribs.as_ptr(), &mut config, 1, &mut num) == 0 || num == 0 {
            return Err(format!(
                "eglChooseConfig found no config: {:#x}",
                eglGetError()
            ));
        }
        let ctx_attribs: [EGLint; 3] = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
        let ctx = eglCreateContext(dpy, config, core::ptr::null_mut(), ctx_attribs.as_ptr());
        if ctx.is_null() {
            return Err(format!("eglCreateContext failed: {:#x}", eglGetError()));
        }
        if eglMakeCurrent(dpy, core::ptr::null_mut(), core::ptr::null_mut(), ctx) == 0 {
            return Err(format!(
                "eglMakeCurrent (surfaceless) failed: {:#x}",
                eglGetError()
            ));
        }
        Ok(dpy)
    }
}

/// Read the current framebuffer back as a top-first [`Pixmap`] (GL is
/// bottom-up, so rows are flipped).
unsafe fn read_back(w: GLsizei, h: GLsizei) -> Pixmap {
    unsafe {
        let row = (w as usize) * 4;
        let mut raw = vec![0u8; row * h as usize];
        glReadPixels(
            0,
            0,
            w,
            h,
            GL_RGBA,
            GL_UNSIGNED_BYTE,
            raw.as_mut_ptr() as *mut c_void,
        );
        let mut out = vec![0u8; raw.len()];
        for y in 0..h as usize {
            let src = &raw[y * row..(y + 1) * row];
            let dst_y = h as usize - 1 - y;
            out[dst_y * row..(dst_y + 1) * row].copy_from_slice(src);
        }
        Pixmap::from_rgba8(PhysicalSize::new(w as u32, h as u32), out)
    }
}

/// Upload `input` to a texture, draw it to an offscreen framebuffer with a
/// pass-through shader, and read the result back as a new [`Pixmap`].
pub fn present_offscreen(input: &Pixmap) -> Result<Pixmap, String> {
    let size = input.size();
    let (w, h) = (size.width as GLsizei, size.height as GLsizei);
    if w == 0 || h == 0 {
        return Err("empty pixmap".into());
    }

    unsafe {
        egl_make_current()?;

        // --- input texture ---
        let mut src_tex: GLuint = 0;
        glGenTextures(1, &mut src_tex);
        glBindTexture(GL_TEXTURE_2D, src_tex);
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
            input.as_bytes().as_ptr() as *const c_void,
        );

        // --- offscreen color target + FBO ---
        let mut dst_tex: GLuint = 0;
        glGenTextures(1, &mut dst_tex);
        glBindTexture(GL_TEXTURE_2D, dst_tex);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
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
        let mut fbo: GLuint = 0;
        glGenFramebuffers(1, &mut fbo);
        glBindFramebuffer(GL_FRAMEBUFFER, fbo);
        glFramebufferTexture2D(
            GL_FRAMEBUFFER,
            GL_COLOR_ATTACHMENT0,
            GL_TEXTURE_2D,
            dst_tex,
            0,
        );
        if glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE {
            return Err("framebuffer incomplete".into());
        }
        glViewport(0, 0, w, h);

        // --- program ---
        let vs = compile(GL_VERTEX_SHADER, VERTEX_SRC)?;
        let fs = compile(GL_FRAGMENT_SHADER, FRAGMENT_SRC)?;
        let prog = glCreateProgram();
        glAttachShader(prog, vs);
        glAttachShader(prog, fs);
        glLinkProgram(prog);
        let mut linked: GLint = 0;
        glGetProgramiv(prog, GL_LINK_STATUS, &mut linked);
        if linked == 0 {
            return Err("program link failed".into());
        }
        glUseProgram(prog);

        // --- fullscreen quad (pos.xy, uv.xy); uv row 0 = texture top ---
        #[rustfmt::skip]
        let verts: [f32; 16] = [
            -1.0, -1.0, 0.0, 1.0,
             1.0, -1.0, 1.0, 1.0,
            -1.0,  1.0, 0.0, 0.0,
             1.0,  1.0, 1.0, 0.0,
        ];
        let mut vbo: GLuint = 0;
        glGenBuffers(1, &mut vbo);
        glBindBuffer(GL_ARRAY_BUFFER, vbo);
        glBufferData(
            GL_ARRAY_BUFFER,
            std::mem::size_of_val(&verts) as isize,
            verts.as_ptr() as *const c_void,
            GL_STATIC_DRAW,
        );
        let stride = 4 * std::mem::size_of::<f32>() as GLsizei;
        let pos_loc = glGetAttribLocation(prog, c"pos".as_ptr());
        let uv_loc = glGetAttribLocation(prog, c"uv".as_ptr());
        if pos_loc < 0 || uv_loc < 0 {
            return Err("attribute location not found".into());
        }
        glEnableVertexAttribArray(pos_loc as GLuint);
        glVertexAttribPointer(
            pos_loc as GLuint,
            2,
            GL_FLOAT,
            GL_FALSE,
            stride,
            core::ptr::null(),
        );
        glEnableVertexAttribArray(uv_loc as GLuint);
        glVertexAttribPointer(
            uv_loc as GLuint,
            2,
            GL_FLOAT,
            GL_FALSE,
            stride,
            (2 * std::mem::size_of::<f32>()) as *const c_void,
        );

        // --- draw the textured quad ---
        glActiveTexture(GL_TEXTURE0);
        glBindTexture(GL_TEXTURE_2D, src_tex);
        let tex_loc = glGetUniformLocation(prog, c"tex".as_ptr());
        glUniform1i(tex_loc, 0);
        glClearColor(0.0, 0.0, 0.0, 1.0);
        glClear(GL_COLOR_BUFFER_BIT);
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
        glFinish();
        Ok(read_back(w, h))
    }
}

/// Convenience wrapper for a box-only GPU render (no text). See
/// [`render_offscreen`].
pub fn fill_rects_offscreen(
    size: PhysicalSize,
    background: Color,
    rects: &[(Rect, Color, f32, f32)],
) -> Result<Pixmap, String> {
    render_offscreen(size, background, rects, &[])
}

/// Render a scene **GPU-natively** to an offscreen framebuffer and read it back:
/// box primitives as tessellated geometry through a rounded-rect SDF shader, and
/// text by alpha-blending CPU-rasterized glyph coverage masks as colored quads.
///
/// Each rect is `(rect, color, corner_radius, border_width)` (radius `0.0` =
/// sharp, `border_width` `0.0` = solid fill, else a stroked outline). Each text
/// is `(mask, dst, color)` where `mask` is a white-on-black coverage raster of
/// the glyphs (its red channel is the coverage), `dst` the logical rectangle to
/// place it in, and `color` the ink color. All coordinates are logical pixels
/// with a top-left origin. A per-glyph atlas cache is the production
/// optimization; this composites whole coverage masks.
pub fn render_offscreen(
    size: PhysicalSize,
    background: Color,
    rects: &[(Rect, Color, f32, f32)],
    texts: &[(&Pixmap, Rect, Color)],
) -> Result<Pixmap, String> {
    render_scene_gl(size, background, rects, texts, None)
}

/// As [`render_offscreen`], but with an optional shared **glyph atlas**: one
/// coverage texture plus per-glyph `(src_px, dst, color)` quads, so repeated
/// glyphs reuse a single upload instead of one texture per text run.
#[allow(clippy::type_complexity)]
pub fn render_scene_gl(
    size: PhysicalSize,
    background: Color,
    rects: &[(Rect, Color, f32, f32)],
    texts: &[(&Pixmap, Rect, Color)],
    atlas: Option<(&Pixmap, &[(Rect, Rect, Color)])>,
) -> Result<Pixmap, String> {
    let (w, h) = (size.width as GLsizei, size.height as GLsizei);
    if w == 0 || h == 0 {
        return Err("empty target".into());
    }
    let (wf, hf) = (size.width as f32, size.height as f32);

    unsafe {
        egl_make_current()?;

        // --- offscreen color target + FBO ---
        let mut dst_tex: GLuint = 0;
        glGenTextures(1, &mut dst_tex);
        glBindTexture(GL_TEXTURE_2D, dst_tex);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
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
        let mut fbo: GLuint = 0;
        glGenFramebuffers(1, &mut fbo);
        glBindFramebuffer(GL_FRAMEBUFFER, fbo);
        glFramebufferTexture2D(
            GL_FRAMEBUFFER,
            GL_COLOR_ATTACHMENT0,
            GL_TEXTURE_2D,
            dst_tex,
            0,
        );
        if glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE {
            return Err("framebuffer incomplete".into());
        }
        glViewport(0, 0, w, h);

        // --- flat-color program ---
        let vs = compile(GL_VERTEX_SHADER, FLAT_VERTEX_SRC)?;
        let fs = compile(GL_FRAGMENT_SHADER, FLAT_FRAGMENT_SRC)?;
        let prog = glCreateProgram();
        glAttachShader(prog, vs);
        glAttachShader(prog, fs);
        glLinkProgram(prog);
        let mut linked: GLint = 0;
        glGetProgramiv(prog, GL_LINK_STATUS, &mut linked);
        if linked == 0 {
            return Err("flat program link failed".into());
        }
        glUseProgram(prog);
        let pos_loc = glGetAttribLocation(prog, c"pos".as_ptr());
        let color_loc = glGetUniformLocation(prog, c"u_color".as_ptr());
        let center_loc = glGetUniformLocation(prog, c"u_center".as_ptr());
        let half_loc = glGetUniformLocation(prog, c"u_half".as_ptr());
        let radius_loc = glGetUniformLocation(prog, c"u_radius".as_ptr());
        let border_loc = glGetUniformLocation(prog, c"u_border".as_ptr());
        if pos_loc < 0 {
            return Err("flat attribute location not found".into());
        }

        let mut vbo: GLuint = 0;
        glGenBuffers(1, &mut vbo);
        glBindBuffer(GL_ARRAY_BUFFER, vbo);
        glEnableVertexAttribArray(pos_loc as GLuint);
        glVertexAttribPointer(
            pos_loc as GLuint,
            2,
            GL_FLOAT,
            GL_FALSE,
            0,
            core::ptr::null(),
        );

        // Clear to the background, then draw each rect as two triangles.
        let bg = [
            background.r as f32 / 255.0,
            background.g as f32 / 255.0,
            background.b as f32 / 255.0,
            background.a as f32 / 255.0,
        ];
        glClearColor(bg[0], bg[1], bg[2], bg[3]);
        glClear(GL_COLOR_BUFFER_BIT);

        // logical (top-left origin) → GL NDC.
        let ndc = |x: f64, y: f64| (2.0 * x as f32 / wf - 1.0, 1.0 - 2.0 * y as f32 / hf);
        for (rect, color, radius, border) in rects {
            let (x0, y0) = ndc(rect.min_x(), rect.min_y());
            let (x1, y1) = ndc(rect.max_x(), rect.max_y());
            // SDF uniforms in framebuffer pixels (origin bottom-left, so flip y).
            let (rw, rh) = (rect.width() as f32, rect.height() as f32);
            let cx = rect.min_x() as f32 + rw / 2.0;
            let cy = hf - (rect.min_y() as f32 + rh / 2.0);
            let r = radius.clamp(0.0, rw.min(rh) / 2.0);
            glUniform2f(center_loc, cx, cy);
            glUniform2f(half_loc, rw / 2.0, rh / 2.0);
            glUniform1f(radius_loc, r);
            glUniform1f(border_loc, *border); // 0 = solid fill, >0 = stroke ring
            #[rustfmt::skip]
            let verts: [f32; 12] = [
                x0, y0,  x1, y0,  x0, y1,
                x0, y1,  x1, y0,  x1, y1,
            ];
            glBufferData(
                GL_ARRAY_BUFFER,
                std::mem::size_of_val(&verts) as isize,
                verts.as_ptr() as *const c_void,
                GL_STATIC_DRAW,
            );
            glVertexAttribPointer(
                pos_loc as GLuint,
                2,
                GL_FLOAT,
                GL_FALSE,
                0,
                core::ptr::null(),
            );
            glUniform4f(
                color_loc,
                color.r as f32 / 255.0,
                color.g as f32 / 255.0,
                color.b as f32 / 255.0,
                color.a as f32 / 255.0,
            );
            glDrawArrays(GL_TRIANGLES, 0, 6);
        }

        // --- text pass: alpha-blend glyph-coverage masks as colored quads ---
        if !texts.is_empty() {
            let tvs = compile(GL_VERTEX_SHADER, VERTEX_SRC)?; // pos + uv
            let tfs = compile(GL_FRAGMENT_SHADER, TEXT_FRAGMENT_SRC)?;
            let tprog = glCreateProgram();
            glAttachShader(tprog, tvs);
            glAttachShader(tprog, tfs);
            glLinkProgram(tprog);
            let mut tlinked: GLint = 0;
            glGetProgramiv(tprog, GL_LINK_STATUS, &mut tlinked);
            if tlinked == 0 {
                return Err("text program link failed".into());
            }
            glUseProgram(tprog);
            let tpos = glGetAttribLocation(tprog, c"pos".as_ptr());
            let tuv = glGetAttribLocation(tprog, c"uv".as_ptr());
            let tcolor = glGetUniformLocation(tprog, c"u_color".as_ptr());
            let tmask = glGetUniformLocation(tprog, c"u_mask".as_ptr());
            if tpos < 0 || tuv < 0 {
                return Err("text attribute location not found".into());
            }
            glEnable(GL_BLEND);
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);

            let mut tvbo: GLuint = 0;
            glGenBuffers(1, &mut tvbo);
            glBindBuffer(GL_ARRAY_BUFFER, tvbo);
            let mut mask_tex: GLuint = 0;
            glGenTextures(1, &mut mask_tex);
            glActiveTexture(GL_TEXTURE0);
            glUniform1i(tmask, 0);
            let stride = 4 * std::mem::size_of::<f32>() as GLsizei;

            for (mask, dst, color) in texts {
                let ms = mask.size();
                glBindTexture(GL_TEXTURE_2D, mask_tex);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
                glTexImage2D(
                    GL_TEXTURE_2D,
                    0,
                    GL_RGBA as GLint,
                    ms.width as GLsizei,
                    ms.height as GLsizei,
                    0,
                    GL_RGBA,
                    GL_UNSIGNED_BYTE,
                    mask.as_bytes().as_ptr() as *const c_void,
                );
                // Quad at `dst` with uv top→0 (matches the mask's top-first rows).
                let (x0, y0) = ndc(dst.min_x(), dst.min_y());
                let (x1, y1) = ndc(dst.max_x(), dst.max_y());
                #[rustfmt::skip]
                let verts: [f32; 24] = [
                    x0, y0, 0.0, 0.0,  x1, y0, 1.0, 0.0,  x0, y1, 0.0, 1.0,
                    x0, y1, 0.0, 1.0,  x1, y0, 1.0, 0.0,  x1, y1, 1.0, 1.0,
                ];
                glBufferData(
                    GL_ARRAY_BUFFER,
                    std::mem::size_of_val(&verts) as isize,
                    verts.as_ptr() as *const c_void,
                    GL_STATIC_DRAW,
                );
                glEnableVertexAttribArray(tpos as GLuint);
                glVertexAttribPointer(
                    tpos as GLuint,
                    2,
                    GL_FLOAT,
                    GL_FALSE,
                    stride,
                    core::ptr::null(),
                );
                glEnableVertexAttribArray(tuv as GLuint);
                glVertexAttribPointer(
                    tuv as GLuint,
                    2,
                    GL_FLOAT,
                    GL_FALSE,
                    stride,
                    (2 * std::mem::size_of::<f32>()) as *const c_void,
                );
                glUniform4f(
                    tcolor,
                    color.r as f32 / 255.0,
                    color.g as f32 / 255.0,
                    color.b as f32 / 255.0,
                    color.a as f32 / 255.0,
                );
                glDrawArrays(GL_TRIANGLES, 0, 6);
            }
        }

        // --- glyph-atlas pass: one shared coverage texture, per-glyph quads ---
        if let Some((atlas_pm, glyphs)) = atlas
            && !glyphs.is_empty()
        {
            let tvs = compile(GL_VERTEX_SHADER, VERTEX_SRC)?;
            let tfs = compile(GL_FRAGMENT_SHADER, TEXT_FRAGMENT_SRC)?;
            let tprog = glCreateProgram();
            glAttachShader(tprog, tvs);
            glAttachShader(tprog, tfs);
            glLinkProgram(tprog);
            let mut tlinked: GLint = 0;
            glGetProgramiv(tprog, GL_LINK_STATUS, &mut tlinked);
            if tlinked == 0 {
                return Err("atlas program link failed".into());
            }
            glUseProgram(tprog);
            let tpos = glGetAttribLocation(tprog, c"pos".as_ptr());
            let tuv = glGetAttribLocation(tprog, c"uv".as_ptr());
            let tcolor = glGetUniformLocation(tprog, c"u_color".as_ptr());
            let tmask = glGetUniformLocation(tprog, c"u_mask".as_ptr());
            glEnable(GL_BLEND);
            glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);

            // Upload the atlas once.
            let aw = atlas_pm.size().width as f32;
            let ah = atlas_pm.size().height as f32;
            let mut atex: GLuint = 0;
            glGenTextures(1, &mut atex);
            glActiveTexture(GL_TEXTURE0);
            glBindTexture(GL_TEXTURE_2D, atex);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            glTexImage2D(
                GL_TEXTURE_2D,
                0,
                GL_RGBA as GLint,
                aw as GLsizei,
                ah as GLsizei,
                0,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                atlas_pm.as_bytes().as_ptr() as *const c_void,
            );
            glUniform1i(tmask, 0);

            let mut avbo: GLuint = 0;
            glGenBuffers(1, &mut avbo);
            glBindBuffer(GL_ARRAY_BUFFER, avbo);
            let stride = 4 * std::mem::size_of::<f32>() as GLsizei;
            for (src, dst, color) in glyphs {
                let (u0, v0) = (src.min_x() as f32 / aw, src.min_y() as f32 / ah);
                let (u1, v1) = (src.max_x() as f32 / aw, src.max_y() as f32 / ah);
                let (x0, y0) = ndc(dst.min_x(), dst.min_y());
                let (x1, y1) = ndc(dst.max_x(), dst.max_y());
                #[rustfmt::skip]
                let verts: [f32; 24] = [
                    x0, y0, u0, v0,  x1, y0, u1, v0,  x0, y1, u0, v1,
                    x0, y1, u0, v1,  x1, y0, u1, v0,  x1, y1, u1, v1,
                ];
                glBufferData(
                    GL_ARRAY_BUFFER,
                    std::mem::size_of_val(&verts) as isize,
                    verts.as_ptr() as *const c_void,
                    GL_STATIC_DRAW,
                );
                glEnableVertexAttribArray(tpos as GLuint);
                glVertexAttribPointer(
                    tpos as GLuint,
                    2,
                    GL_FLOAT,
                    GL_FALSE,
                    stride,
                    core::ptr::null(),
                );
                glEnableVertexAttribArray(tuv as GLuint);
                glVertexAttribPointer(
                    tuv as GLuint,
                    2,
                    GL_FLOAT,
                    GL_FALSE,
                    stride,
                    (2 * std::mem::size_of::<f32>()) as *const c_void,
                );
                glUniform4f(
                    tcolor,
                    color.r as f32 / 255.0,
                    color.g as f32 / 255.0,
                    color.b as f32 / 255.0,
                    color.a as f32 / 255.0,
                );
                glDrawArrays(GL_TRIANGLES, 0, 6);
            }
        }

        glFinish();
        Ok(read_back(w, h))
    }
}

// ---- dma-buf import/export (the cross-process zero-copy surface seam) --------
//
// The browser content process renders the page into a GPU texture, *exports* it
// as a `dma-buf` (a kernel handle to GPU memory), and passes the fd to the UI
// (Stipple) process over a socket; Stipple *imports* it as a texture and composites
// it. Both ends use standard Mesa EGL extensions — no kernel `udmabuf` ioctls:
//   producer: EGL_MESA_image_dma_buf_export
//   consumer: EGL_EXT_image_dma_buf_import
// The self-test below does both ends in one process (the fd never leaves it) to
// prove the GPU mechanism; cross-process fd passing is the next step.

const EGL_EXTENSIONS: EGLint = 0x3055;
const EGL_GL_TEXTURE_2D: EGLenum = 0x30B1;
const EGL_IMAGE_PRESERVED_KHR: EGLint = 0x30D2;
const EGL_TRUE: EGLint = 1;
const EGL_LINUX_DMA_BUF_EXT: EGLenum = 0x3270;
const EGL_WIDTH: EGLint = 0x3057;
const EGL_HEIGHT: EGLint = 0x3056;
const EGL_LINUX_DRM_FOURCC_EXT: EGLint = 0x3271;
const EGL_DMA_BUF_PLANE0_FD_EXT: EGLint = 0x3272;
const EGL_DMA_BUF_PLANE0_OFFSET_EXT: EGLint = 0x3273;
const EGL_DMA_BUF_PLANE0_PITCH_EXT: EGLint = 0x3274;
const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: EGLint = 0x3443;
const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: EGLint = 0x3444;
// Sentinel: "no explicit modifier" (let the driver pick). Real tiled buffers
// report a concrete modifier that the importer must echo back.
const DRM_FORMAT_MOD_INVALID: u64 = u64::MAX;
// DRM fourcc for 8-bit RGBA in memory order [R,G,B,A] — matches `Pixmap`.
const DRM_FORMAT_ABGR8888: i32 = 0x3432_4241; // 'AB24'

type EglClientBuffer = *mut c_void;
type EglImage = *mut c_void;

type PfnCreateImage = unsafe extern "C" fn(
    EGLDisplay,
    EGLContext,
    EGLenum,
    EglClientBuffer,
    *const EGLint,
) -> EglImage;
type PfnDestroyImage = unsafe extern "C" fn(EGLDisplay, EglImage) -> EGLBoolean;
type PfnExportQuery =
    unsafe extern "C" fn(EGLDisplay, EglImage, *mut i32, *mut i32, *mut u64) -> EGLBoolean;
type PfnExport =
    unsafe extern "C" fn(EGLDisplay, EglImage, *mut i32, *mut EGLint, *mut EGLint) -> EGLBoolean;
type PfnTargetTexture = unsafe extern "C" fn(GLenum, EglImage);

/// Resolve an extension entry point by (NUL-terminated) name.
unsafe fn load_proc<T: Copy>(name: &[u8]) -> Result<T, String> {
    unsafe {
        debug_assert_eq!(name.last(), Some(&0), "proc name must be NUL-terminated");
        let p = eglGetProcAddress(name.as_ptr() as *const c_char);
        if p.is_null() {
            return Err(format!(
                "missing entry point {}",
                String::from_utf8_lossy(&name[..name.len() - 1])
            ));
        }
        Ok(core::mem::transmute_copy::<*const c_void, T>(&p))
    }
}

/// Bring up surfaceless EGL and return the device's EGL extension string — used
/// to check for `EGL_EXT_image_dma_buf_import` / `EGL_MESA_image_dma_buf_export`
/// before relying on them. Errors if EGL can't initialize.
pub fn dmabuf_extensions() -> Result<String, String> {
    unsafe {
        let dpy = egl_init()?;
        let s = eglQueryString(dpy, EGL_EXTENSIONS);
        if s.is_null() {
            return Err("eglQueryString(EGL_EXTENSIONS) returned null".into());
        }
        Ok(core::ffi::CStr::from_ptr(s).to_string_lossy().into_owned())
    }
}

/// Prove the cross-process surface mechanism end to end (in one process): upload
/// a known 2×2 pattern to a GL texture, export it as a `dma-buf`, re-import that
/// `dma-buf` as a second texture, read it back, and confirm the pixels survived.
/// Returns the imported pixels (top-first RGBA) on success.
///
/// Runs surfaceless, so it touches no window/X server and is safe to run
/// directly on a GPU box. Requires `EGL_MESA_image_dma_buf_export` +
/// `EGL_EXT_image_dma_buf_import` (real Mesa/GPU drivers; may be absent on
/// software-only Mesa).
pub fn dmabuf_export_import_self_test() -> Result<Vec<u8>, String> {
    unsafe { dmabuf_roundtrip(egl_init()?) }
}

/// The same export → import → sample round-trip as
/// [`dmabuf_export_import_self_test`], but on the specific GPU named by `drm_fd`
/// (e.g. the X server's device from `DRI3Open`) via a GBM device. Confirms that
/// device can export a dma-buf and re-import it — the prerequisite for handing
/// rendered frames to the X server through DRI3 + Present.
pub fn dmabuf_self_test_on_device(drm_fd: i32) -> Result<Vec<u8>, String> {
    unsafe { dmabuf_roundtrip(egl_init_gbm(drm_fd)?) }
}

/// Render a `w`×`h` solid frame on the GPU named by `drm_fd` (via GBM) and export
/// it as a single-plane dma-buf, returning the [`crate::gpu::DmabufExport`] descriptor
/// the X server needs to wrap it as a Pixmap. The fd is owned by the caller.
pub fn export_dmabuf_on_device(
    drm_fd: i32,
    w: u32,
    h: u32,
) -> Result<crate::gpu::DmabufExport, String> {
    unsafe { export_dmabuf_current(egl_init_gbm(drm_fd)?, w as i32, h as i32) }
}

/// Export the current context's freshly-rendered texture as a dma-buf and return
/// its descriptor. Mirrors the producer half of [`dmabuf_roundtrip`] (texture →
/// EGLImage → `eglExportDMABUFImageMESA`) but stops at export and hands back the
/// fd + layout instead of re-importing. Single-plane formats only.
unsafe fn export_dmabuf_current(
    dpy: EGLDisplay,
    w: i32,
    h: i32,
) -> Result<crate::gpu::DmabufExport, String> {
    unsafe {
        let ctx = eglGetCurrentContext();
        let create_image: PfnCreateImage = load_proc(b"eglCreateImageKHR\0")?;
        let destroy_image: PfnDestroyImage = load_proc(b"eglDestroyImageKHR\0")?;
        let export_query: PfnExportQuery = load_proc(b"eglExportDMABUFImageQueryMESA\0")?;
        let export: PfnExport = load_proc(b"eglExportDMABUFImageMESA\0")?;

        // A solid stipple-blue source texture of the requested size.
        let mut pixels = vec![0u8; (w as usize) * (h as usize) * 4];
        for px in pixels.as_chunks_mut::<4>().0 {
            *px = [0x60, 0x9c, 0xff, 0xff];
        }
        let mut tex: GLuint = 0;
        glGenTextures(1, &mut tex);
        glBindTexture(GL_TEXTURE_2D, tex);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glTexImage2D(
            GL_TEXTURE_2D,
            0,
            GL_RGBA as GLint,
            w,
            h,
            0,
            GL_RGBA,
            GL_UNSIGNED_BYTE,
            pixels.as_ptr() as *const c_void,
        );
        glFinish(); // ensure the upload lands before the image is exported

        let preserve = [EGL_IMAGE_PRESERVED_KHR, EGL_TRUE, EGL_NONE];
        let img = create_image(
            dpy,
            ctx,
            EGL_GL_TEXTURE_2D,
            tex as usize as EglClientBuffer,
            preserve.as_ptr(),
        );
        if img.is_null() {
            return Err(format!(
                "eglCreateImageKHR(GL_TEXTURE_2D) failed: {:#x}",
                eglGetError()
            ));
        }
        let (mut fourcc, mut planes, mut modifier) = (0i32, 0i32, 0u64);
        if export_query(dpy, img, &mut fourcc, &mut planes, &mut modifier) == 0 {
            destroy_image(dpy, img);
            return Err(format!(
                "eglExportDMABUFImageQueryMESA failed: {:#x}",
                eglGetError()
            ));
        }
        if planes != 1 {
            destroy_image(dpy, img);
            return Err(format!(
                "unsupported multi-plane dma-buf (planes={planes}); single-plane only"
            ));
        }
        let (mut fd, mut stride, mut offset) = (-1i32, 0i32, 0i32);
        if export(dpy, img, &mut fd, &mut stride, &mut offset) == 0 || fd < 0 {
            destroy_image(dpy, img);
            return Err(format!(
                "eglExportDMABUFImageMESA failed: {:#x}",
                eglGetError()
            ));
        }
        destroy_image(dpy, img);
        Ok(crate::gpu::DmabufExport {
            fd,
            width: w as u32,
            height: h as u32,
            stride: stride as u32,
            offset: offset as u32,
            modifier,
            fourcc: fourcc as u32,
            bpp: 32,
        })
    }
}

/// Export a GL texture as a dma-buf, re-import it, sample it, and verify the
/// pixels — on whatever EGL context is current (`dpy`).
unsafe fn dmabuf_roundtrip(dpy: EGLDisplay) -> Result<Vec<u8>, String> {
    unsafe {
        let ctx = eglGetCurrentContext();

        let create_image: PfnCreateImage = load_proc(b"eglCreateImageKHR\0")?;
        let destroy_image: PfnDestroyImage = load_proc(b"eglDestroyImageKHR\0")?;
        let export_query: PfnExportQuery = load_proc(b"eglExportDMABUFImageQueryMESA\0")?;
        let export: PfnExport = load_proc(b"eglExportDMABUFImageMESA\0")?;
        let target_texture: PfnTargetTexture = load_proc(b"glEGLImageTargetTexture2DOES\0")?;

        let (w, h) = (2i32, 2i32);
        // Source pattern: four distinct opaque colors, one per texel.
        let src: [u8; 16] = [
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 255, 255, // white
        ];

        // Producer: a normal GL texture holding the pattern.
        let mut tex_src: GLuint = 0;
        glGenTextures(1, &mut tex_src);
        glBindTexture(GL_TEXTURE_2D, tex_src);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glTexImage2D(
            GL_TEXTURE_2D,
            0,
            GL_RGBA as GLint,
            w,
            h,
            0,
            GL_RGBA,
            GL_UNSIGNED_BYTE,
            src.as_ptr() as *const c_void,
        );
        glFinish(); // ensure the upload lands before the image is exported

        // Wrap it in an EGLImage, then export that image as a dma-buf.
        let preserve = [EGL_IMAGE_PRESERVED_KHR, EGL_TRUE, EGL_NONE];
        let img_src = create_image(
            dpy,
            ctx,
            EGL_GL_TEXTURE_2D,
            tex_src as usize as EglClientBuffer,
            preserve.as_ptr(),
        );
        if img_src.is_null() {
            return Err(format!(
                "eglCreateImageKHR(GL_TEXTURE_2D) failed: {:#x}",
                eglGetError()
            ));
        }
        let (mut fourcc, mut planes, mut modifier) = (0i32, 0i32, 0u64);
        if export_query(dpy, img_src, &mut fourcc, &mut planes, &mut modifier) == 0 {
            return Err(format!(
                "eglExportDMABUFImageQueryMESA failed: {:#x}",
                eglGetError()
            ));
        }
        let (mut fd, mut stride, mut offset) = (-1i32, 0i32, 0i32);
        if export(dpy, img_src, &mut fd, &mut stride, &mut offset) == 0 || fd < 0 {
            return Err(format!(
                "eglExportDMABUFImageMESA failed: {:#x}",
                eglGetError()
            ));
        }

        // Consumer: import the dma-buf as a fresh texture. In the browser this fd
        // arrives over a socket from the content process.
        let mut import_attribs = vec![
            EGL_WIDTH,
            w,
            EGL_HEIGHT,
            h,
            EGL_LINUX_DRM_FOURCC_EXT,
            if fourcc != 0 {
                fourcc
            } else {
                DRM_FORMAT_ABGR8888
            },
            EGL_DMA_BUF_PLANE0_FD_EXT,
            fd,
            EGL_DMA_BUF_PLANE0_OFFSET_EXT,
            offset,
            EGL_DMA_BUF_PLANE0_PITCH_EXT,
            stride,
        ];
        // Echo back the export's format modifier — buffers are routinely tiled,
        // and importing such a dma-buf as LINEAR yields an invalid image (the
        // glEGLImageTargetTexture2DOES INVALID_OPERATION we hit otherwise).
        if modifier != DRM_FORMAT_MOD_INVALID {
            import_attribs.push(EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT);
            import_attribs.push((modifier & 0xFFFF_FFFF) as EGLint);
            import_attribs.push(EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT);
            import_attribs.push((modifier >> 32) as EGLint);
        }
        import_attribs.push(EGL_NONE);
        let img_dst = create_image(
            dpy,
            core::ptr::null_mut(),
            EGL_LINUX_DMA_BUF_EXT,
            core::ptr::null_mut(),
            import_attribs.as_ptr(),
        );
        if img_dst.is_null() {
            libc_close(fd);
            return Err(format!(
                "eglCreateImageKHR(LINUX_DMA_BUF) failed: {:#x}",
                eglGetError()
            ));
        }
        let mut tex_dst: GLuint = 0;
        glGenTextures(1, &mut tex_dst);
        glBindTexture(GL_TEXTURE_2D, tex_dst);
        // NEAREST so the 2x2 source texels map through cleanly (no blending).
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        target_texture(GL_TEXTURE_2D, img_dst);
        let gl_err = glGetError();
        if gl_err != 0 {
            libc_close(fd);
            return Err(format!(
                "glEGLImageTargetTexture2DOES failed (GL error {gl_err:#x}); \
                 export fourcc={fourcc:#x} planes={planes} modifier={modifier:#x} \
                 stride={stride} offset={offset}"
            ));
        }

        // Imported dma-buf textures are sample-only (not color-renderable), so
        // verify the way Stipple will actually use one: SAMPLE the imported texture
        // into a normal renderable target, then read THAT back.
        let mut tex_out: GLuint = 0;
        glGenTextures(1, &mut tex_out);
        glBindTexture(GL_TEXTURE_2D, tex_out);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
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
        let mut fbo: GLuint = 0;
        glGenFramebuffers(1, &mut fbo);
        glBindFramebuffer(GL_FRAMEBUFFER, fbo);
        glFramebufferTexture2D(
            GL_FRAMEBUFFER,
            GL_COLOR_ATTACHMENT0,
            GL_TEXTURE_2D,
            tex_out,
            0,
        );
        if glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE {
            libc_close(fd);
            return Err("FBO incomplete for output texture".into());
        }
        glViewport(0, 0, w, h);

        // Pass-through sampler program (same as the present path).
        let vs = compile(GL_VERTEX_SHADER, VERTEX_SRC)?;
        let fs = compile(GL_FRAGMENT_SHADER, FRAGMENT_SRC)?;
        let prog = glCreateProgram();
        glAttachShader(prog, vs);
        glAttachShader(prog, fs);
        glLinkProgram(prog);
        let mut linked: GLint = 0;
        glGetProgramiv(prog, GL_LINK_STATUS, &mut linked);
        if linked == 0 {
            libc_close(fd);
            return Err("self-test program link failed".into());
        }
        glUseProgram(prog);
        #[rustfmt::skip]
        let verts: [f32; 16] = [
            -1.0, -1.0, 0.0, 1.0,
             1.0, -1.0, 1.0, 1.0,
            -1.0,  1.0, 0.0, 0.0,
             1.0,  1.0, 1.0, 0.0,
        ];
        let mut vbo: GLuint = 0;
        glGenBuffers(1, &mut vbo);
        glBindBuffer(GL_ARRAY_BUFFER, vbo);
        glBufferData(
            GL_ARRAY_BUFFER,
            std::mem::size_of_val(&verts) as isize,
            verts.as_ptr() as *const c_void,
            GL_STATIC_DRAW,
        );
        let stride = 4 * std::mem::size_of::<f32>() as GLsizei;
        let pos_loc = glGetAttribLocation(prog, c"pos".as_ptr());
        let uv_loc = glGetAttribLocation(prog, c"uv".as_ptr());
        if pos_loc < 0 || uv_loc < 0 {
            libc_close(fd);
            return Err("self-test attribute location not found".into());
        }
        glEnableVertexAttribArray(pos_loc as GLuint);
        glVertexAttribPointer(
            pos_loc as GLuint,
            2,
            GL_FLOAT,
            GL_FALSE,
            stride,
            core::ptr::null(),
        );
        glEnableVertexAttribArray(uv_loc as GLuint);
        glVertexAttribPointer(
            uv_loc as GLuint,
            2,
            GL_FLOAT,
            GL_FALSE,
            stride,
            (2 * std::mem::size_of::<f32>()) as *const c_void,
        );
        // Sample the imported dma-buf texture into the output target.
        glActiveTexture(GL_TEXTURE0);
        glBindTexture(GL_TEXTURE_2D, tex_dst);
        let tex_loc = glGetUniformLocation(prog, c"tex".as_ptr());
        glUniform1i(tex_loc, 0);
        glClearColor(0.0, 0.0, 0.0, 1.0);
        glClear(GL_COLOR_BUFFER_BIT);
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
        glFinish();
        let out = read_back(w, h);

        destroy_image(dpy, img_dst);
        destroy_image(dpy, img_src);
        libc_close(fd);

        // The four source colors must all be present in the imported readback
        // (row order may flip between GL and the upload, so compare as a set).
        let pix = out.as_bytes();
        let want = [[255u8, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
        for w3 in want {
            let found = pix
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[0] == w3[0] && p[1] == w3[1] && p[2] == w3[2]);
            if !found {
                return Err(format!(
                    "imported pixels missing color {w3:?}; got {:?}",
                    pix
                ));
            }
        }
        Ok(pix.to_vec())
    }
}

unsafe extern "C" {
    /// `close(2)` for the exported dma-buf fd (std has no stable fd-close).
    #[link_name = "close"]
    fn libc_close(fd: i32) -> i32;
}
