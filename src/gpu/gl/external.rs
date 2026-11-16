//! External images on GLES: a `dma-buf` (Linux) through
//! `EGL_EXT_image_dma_buf_import`, an `AHardwareBuffer` (Android) through
//! `EGL_ANDROID_get_native_client_buffer`, each plane an `EGLImage` bound
//! to a texture with `glEGLImageTargetTexture2DOES`. No pixel is copied.
//!
//! Imports are cached by the image's identity, like the image textures:
//! made the first frame an image is drawn, freed once it's dropped or
//! hasn't been drawn for a while. The images a frame drew are held until
//! the next frame begins, so a producer isn't handed its buffer back while
//! the frame may still be reading it. An image that can't be imported
//! (a format or modifier the driver refuses) is remembered as such and drawn
//! from its pixels read on the CPU.

use super::*;
use crate::render::{ExternalFormat, ExternalImage};
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::{Arc, Weak};

#[link(name = "GLESv2")]
unsafe extern "C" {
    fn glDeleteTextures(n: GLsizei, textures: *const GLuint);
}

type EglImage = *mut c_void;
type CreateImage =
    unsafe extern "C" fn(EGLDisplay, EGLContext, EGLenum, *mut c_void, *const EGLint) -> EglImage;
type DestroyImage = unsafe extern "C" fn(EGLDisplay, EglImage) -> EGLBoolean;
type TargetTexture = unsafe extern "C" fn(GLenum, EglImage);
#[cfg(target_os = "android")]
type NativeClientBuffer = unsafe extern "C" fn(*const c_void) -> *mut c_void;

#[cfg(target_os = "linux")]
const EGL_LINUX_DMA_BUF_EXT: EGLenum = 0x3270;
#[cfg(target_os = "android")]
const EGL_NATIVE_BUFFER_ANDROID: EGLenum = 0x3140;

/// Images of external images not drawn for this many frames are freed.
const KEEP_FRAMES: u64 = 120;

/// One external image's import: a texture per plane (none if it failed).
struct Import {
    image: Weak<ExternalImage>,
    egl: Vec<EglImage>,
    textures: Vec<GLuint>,
    used: u64,
}

/// The textures an external image is drawn with.
#[derive(Clone, Copy, Debug)]
pub(super) enum Planes {
    /// One RGBA texture.
    One(GLuint),
    /// Luma and chroma, converted by the YUV program.
    Yuv(GLuint, GLuint),
}

/// The context's imports, and the EGL entry points they need.
pub(super) struct Imports {
    create: Option<CreateImage>,
    destroy: Option<DestroyImage>,
    target: Option<TargetTexture>,
    #[cfg(target_os = "android")]
    client_buffer: Option<NativeClientBuffer>,
    dma_buf: bool,
    map: HashMap<usize, Import>,
    /// The images the last frame drew, held until the next one.
    held: Vec<Arc<ExternalImage>>,
}

unsafe fn proc_addr<T: Copy>(name: &CStr) -> Option<T> {
    unsafe {
        let p = eglGetProcAddress(name.as_ptr());
        (!p.is_null()).then(|| std::mem::transmute_copy::<*const c_void, T>(&p))
    }
}

impl Imports {
    /// What `dpy` (current, with a context) can import.
    pub(super) unsafe fn new(dpy: EGLDisplay) -> Self {
        unsafe {
            let ext = eglQueryString(dpy, EGL_EXTENSIONS);
            let ext = if ext.is_null() {
                String::new()
            } else {
                CStr::from_ptr(ext).to_string_lossy().into_owned()
            };
            let has = |name: &str| ext.split_ascii_whitespace().any(|e| e == name);
            Imports {
                create: proc_addr(c"eglCreateImageKHR"),
                destroy: proc_addr(c"eglDestroyImageKHR"),
                target: proc_addr(c"glEGLImageTargetTexture2DOES"),
                #[cfg(target_os = "android")]
                client_buffer: has("EGL_ANDROID_get_native_client_buffer")
                    .then(|| proc_addr(c"eglGetNativeClientBufferANDROID"))
                    .flatten(),
                dma_buf: cfg!(target_os = "linux") && has("EGL_EXT_image_dma_buf_import"),
                map: HashMap::new(),
                held: Vec::new(),
            }
        }
    }

    /// The formats imported without a copy.
    pub(super) fn formats(&self) -> &'static [ExternalFormat] {
        if self.create.is_none() || self.target.is_none() {
            return &[];
        }
        #[cfg(target_os = "android")]
        if self.client_buffer.is_some() {
            return &[ExternalFormat::Rgba8, ExternalFormat::Rgbx8];
        }
        if self.dma_buf {
            &[
                ExternalFormat::Rgba8,
                ExternalFormat::Bgra8,
                ExternalFormat::Rgbx8,
                ExternalFormat::Bgrx8,
                ExternalFormat::Nv12,
            ]
        } else {
            &[]
        }
    }

    /// A frame begins: let go of the images the last one drew, and free the
    /// imports of images gone or not drawn lately.
    pub(super) unsafe fn begin(&mut self, dpy: EGLDisplay, frame: u64) {
        self.held.clear();
        let destroy = self.destroy;
        self.map.retain(|_, i| {
            let keep = i.image.strong_count() > 0 && frame.saturating_sub(i.used) < KEEP_FRAMES;
            if !keep {
                unsafe { free(dpy, destroy, i) };
            }
            keep
        });
        crate::platform::external::note_native(self.formats());
    }

    /// The textures for `image`, imported unless they're cached; `None` if
    /// it can't be imported (draw its pixels instead).
    pub(super) unsafe fn get(
        &mut self,
        dpy: EGLDisplay,
        image: &Arc<ExternalImage>,
        frame: u64,
    ) -> Option<Planes> {
        unsafe {
            let key = Arc::as_ptr(image) as usize;
            if let Some(i) = self.map.get_mut(&key)
                && !i.image.upgrade().is_some_and(|i| Arc::ptr_eq(&i, image))
            {
                // The address was reused by another image.
                let mut old = self.map.remove(&key).unwrap();
                free(dpy, self.destroy, &mut old);
            }
            if !self.map.contains_key(&key) {
                let import = self.import(dpy, image);
                self.map.insert(key, import);
            }
            let i = self.map.get_mut(&key)?;
            i.used = frame;
            if !self.held.iter().any(|h| Arc::ptr_eq(h, image)) {
                self.held.push(image.clone());
            }
            match i.textures[..] {
                [t] => Some(Planes::One(t)),
                [y, uv] => Some(Planes::Yuv(y, uv)),
                _ => None,
            }
        }
    }

    /// Import `image`: an `EGLImage` and a texture per plane, or nothing.
    unsafe fn import(&self, dpy: EGLDisplay, image: &Arc<ExternalImage>) -> Import {
        let mut import = Import {
            image: Arc::downgrade(image),
            egl: Vec::new(),
            textures: Vec::new(),
            used: 0,
        };
        let (Some(create), Some(target)) = (self.create, self.target) else {
            return import;
        };
        image.wait_acquire();
        unsafe {
            for (buffer, target_kind, attribs) in self.sources(image) {
                let egl = create(
                    dpy,
                    std::ptr::null_mut(),
                    target_kind,
                    buffer,
                    attribs.as_ptr(),
                );
                if egl.is_null() {
                    break;
                }
                import.egl.push(egl);
                let mut tex = 0;
                glGenTextures(1, &mut tex);
                glBindTexture(GL_TEXTURE_2D, tex);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
                while glGetError() != 0 {}
                target(GL_TEXTURE_2D, egl);
                import.textures.push(tex);
                if glGetError() != 0 {
                    break;
                }
            }
            if import.textures.len() != image.format().planes()
                || import.egl.len() != import.textures.len()
            {
                free(dpy, self.destroy, &mut import);
            }
        }
        import
    }

    /// Each plane's `eglCreateImageKHR` arguments: the client buffer, the
    /// target and the attributes.
    #[allow(unused_variables)]
    fn sources(&self, image: &ExternalImage) -> Vec<(*mut c_void, EGLenum, Vec<EGLint>)> {
        use crate::platform::external::Handle;
        match image.handle() {
            #[cfg(target_os = "linux")]
            Handle::DmaBuf(_) if self.dma_buf => {
                let fds = image.dma_buf_planes().unwrap_or_default();
                crate::platform::external::plane_imports(image)
                    .into_iter()
                    .zip(fds)
                    .map(|((w, h, code, offset, stride), (fd, _, _))| {
                        let a = crate::platform::external::egl_dma_buf_attribs(
                            w,
                            h,
                            code,
                            fd,
                            offset,
                            stride,
                            image.modifier(),
                        );
                        (std::ptr::null_mut(), EGL_LINUX_DMA_BUF_EXT, a)
                    })
                    .collect()
            }
            #[cfg(target_os = "android")]
            Handle::HardwareBuffer(b) => match self.client_buffer {
                Some(get) => {
                    let buffer = unsafe { get(*b) };
                    if buffer.is_null() {
                        return Vec::new();
                    }
                    const EGL_IMAGE_PRESERVED_KHR: EGLint = 0x30D2;
                    vec![(
                        buffer,
                        EGL_NATIVE_BUFFER_ANDROID,
                        vec![EGL_IMAGE_PRESERVED_KHR, 1, EGL_NONE],
                    )]
                }
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Free every import (the context is going away).
    pub(super) unsafe fn clear(&mut self, dpy: EGLDisplay) {
        self.held.clear();
        for (_, mut i) in self.map.drain() {
            unsafe { free(dpy, self.destroy, &mut i) };
        }
    }
}

unsafe fn free(dpy: EGLDisplay, destroy: Option<DestroyImage>, i: &mut Import) {
    unsafe {
        for t in i.textures.drain(..) {
            glDeleteTextures(1, &t);
        }
        for e in i.egl.drain(..) {
            if let Some(d) = destroy {
                d(dpy, e);
            }
        }
    }
}

/// The YUV program's body: NV12 from a luma (`.r`) and a chroma (`.rg`:
/// Cb, Cr) texture, full range BT.601 as the CPU converts it.
pub(super) const QUAD_YUV_MAIN: &str = "uniform sampler2D u_tex;\n\
uniform sampler2D u_tex2;\n\
void main() {\n\
  if (v_mode < 0.5) {\n\
    gl_FragColor = quadBox();\n\
  } else {\n\
    vec2 uv = clamp(v_p, v_a2.xy, v_a2.zw);\n\
    float y = texture2D(u_tex, uv).r;\n\
    vec2 c = texture2D(u_tex2, uv).rg - vec2(128.0 / 255.0);\n\
    vec4 t = vec4(clamp(vec3(y + 1.402 * c.y, y - 0.344136 * c.x - 0.714136 * c.y,\n\
                             y + 1.772 * c.x), 0.0, 1.0), 1.0);\n\
    t *= v_a3;\n\
    float cov = clamp(min(v_a1.x, v_a1.z) + 0.5, 0.0, 1.0)\n\
              * clamp(min(v_a1.y, v_a1.w) + 0.5, 0.0, 1.0);\n\
    gl_FragColor = t * cov;\n\
  }\n\
}\n";
