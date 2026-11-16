//! Presenting to a Wayland window without reading back: the frame is copied
//! from the frame target into a texture exported as a `dma-buf`
//! (`EGL_MESA_image_dma_buf_export`) and handed to the window's sink, which
//! attaches it through `zwp_linux_dmabuf_v1` (see
//! [`crate::gpu::dmabuf_present`]).
//!
//! The context stays the offscreen one (Mesa's surfaceless platform). A GL
//! texture's layout is the driver's choice, so the compositor has to take
//! the modifier the export reports: whether it does is probed once, on a
//! small texture, and checked again for each buffer; where it doesn't, the
//! window's frames are read back.

use super::*;
use crate::gpu::dmabuf_present::{self as dp, Ring, Slot};
use crate::platform::dmabuf::{ABGR8888, Buffer, Plane, Present, XBGR8888};

const EGL_GL_TEXTURE_2D: EGLenum = 0x30B1;

type EglImage = *mut c_void;
type CreateImage =
    unsafe extern "C" fn(EGLDisplay, EGLContext, EGLenum, *mut c_void, *const EGLint) -> EglImage;
type DestroyImage = unsafe extern "C" fn(EGLDisplay, EglImage) -> EGLBoolean;
type ExportQuery =
    unsafe extern "C" fn(EGLDisplay, EglImage, *mut i32, *mut i32, *mut u64) -> EGLBoolean;
type Export =
    unsafe extern "C" fn(EGLDisplay, EglImage, *mut i32, *mut EGLint, *mut EGLint) -> EGLBoolean;

#[link(name = "GLESv2")]
unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn glCopyTexSubImage2D(
        target: GLenum,
        level: GLint,
        xoffset: GLint,
        yoffset: GLint,
        x: GLint,
        y: GLint,
        width: GLsizei,
        height: GLsizei,
    );
}

unsafe extern "C" {
    fn close(fd: i32) -> i32;
}

/// The export entry points of a display that has them.
#[derive(Clone, Copy)]
struct Fns {
    create_image: CreateImage,
    destroy_image: DestroyImage,
    export_query: ExportQuery,
    export: Export,
}

impl Fns {
    /// `dpy`'s, if it has `EGL_MESA_image_dma_buf_export` and texture images.
    unsafe fn load(dpy: EGLDisplay) -> Result<Fns, String> {
        unsafe {
            let exts = eglQueryString(dpy, EGL_EXTENSIONS);
            if exts.is_null() {
                return Err("no EGL extensions".into());
            }
            let exts = CStr::from_ptr(exts).to_string_lossy();
            for want in [
                "EGL_MESA_image_dma_buf_export",
                "EGL_KHR_gl_texture_2D_image",
            ] {
                if !exts.split(' ').any(|e| e == want) {
                    return Err(format!("no {want}"));
                }
            }
            let get = |name: &CStr| {
                let p = eglGetProcAddress(name.as_ptr());
                if p.is_null() {
                    Err(format!("no {}", name.to_string_lossy()))
                } else {
                    Ok(p)
                }
            };
            Ok(Fns {
                create_image: std::mem::transmute::<*const c_void, CreateImage>(get(
                    c"eglCreateImageKHR",
                )?),
                destroy_image: std::mem::transmute::<*const c_void, DestroyImage>(get(
                    c"eglDestroyImageKHR",
                )?),
                export_query: std::mem::transmute::<*const c_void, ExportQuery>(get(
                    c"eglExportDMABUFImageQueryMESA",
                )?),
                export: std::mem::transmute::<*const c_void, Export>(get(
                    c"eglExportDMABUFImageMESA",
                )?),
            })
        }
    }
}

/// An exported texture: the texture and its planes' fds (ours to close).
pub(super) struct Exported {
    tex: GLuint,
    fds: Vec<i32>,
}

impl Exported {
    unsafe fn delete(self) {
        unsafe {
            glDeleteTextures(1, &self.tex);
            for fd in self.fds {
                close(fd);
            }
        }
    }
}

/// The Wayland windows' rings, and what the display can export.
#[derive(Default)]
pub(super) struct Wayland {
    /// The export entry points (or why there are none), looked up once.
    fns: Option<Result<Fns, String>>,
    /// Whether the compositor takes what the export makes (probed once).
    taken: Option<bool>,
    rings: HashMap<u32, Ring<Exported>>,
}

impl Wayland {
    /// Let go of every ring (the context is going away: its textures with it).
    pub(super) unsafe fn close(&mut self) {
        for (_, mut ring) in self.rings.drain() {
            ring.clear(None, |e| unsafe {
                for fd in e.fds {
                    close(fd);
                }
            });
        }
    }
}

/// A `w`×`h` RGBA texture exported as a `dma-buf`: the texture, and the
/// buffer as the export describes it (its fourcc, modifier and planes).
unsafe fn export(state: &State, fns: Fns, w: i32, h: i32) -> Result<(Exported, Buffer), String> {
    unsafe {
        let mut tex = 0;
        glGenTextures(1, &mut tex);
        glBindTexture(GL_TEXTURE_2D, tex);
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
        let fail = |what: &str| {
            let e = format!("{what} failed: {:#x}", eglGetError());
            glDeleteTextures(1, &tex);
            e
        };
        let none = [EGL_NONE];
        let image = (fns.create_image)(
            state.dpy,
            state.ctx,
            EGL_GL_TEXTURE_2D,
            tex as usize as *mut c_void,
            none.as_ptr(),
        );
        if image.is_null() {
            return Err(fail("eglCreateImageKHR"));
        }
        let (mut fourcc, mut planes, mut modifier) = (0i32, 0i32, 0u64);
        if (fns.export_query)(state.dpy, image, &mut fourcc, &mut planes, &mut modifier) == 0
            || !(1..=4).contains(&planes)
        {
            (fns.destroy_image)(state.dpy, image);
            return Err(fail("eglExportDMABUFImageQueryMESA"));
        }
        let (mut fds, mut strides, mut offsets) = ([-1i32; 4], [0i32; 4], [0i32; 4]);
        let ok = (fns.export)(
            state.dpy,
            image,
            fds.as_mut_ptr(),
            strides.as_mut_ptr(),
            offsets.as_mut_ptr(),
        );
        (fns.destroy_image)(state.dpy, image);
        let n = planes as usize;
        if ok == 0 || fds[0] < 0 {
            return Err(fail("eglExportDMABUFImageMESA"));
        }
        // Planes may share the first plane's fd (or have none of their own).
        let plane_fds: Vec<i32> = (0..n)
            .map(|i| if fds[i] >= 0 { fds[i] } else { fds[0] })
            .collect();
        let mut own: Vec<i32> = fds[..n].iter().copied().filter(|&fd| fd >= 0).collect();
        own.dedup();
        let buffer = Buffer {
            width: w as u32,
            height: h as u32,
            fourcc: fourcc as u32,
            modifier,
            planes: (0..n)
                .map(|i| Plane {
                    fd: plane_fds[i],
                    offset: offsets[i] as u32,
                    stride: strides[i] as u32,
                })
                .collect(),
        };
        Ok((Exported { tex, fds: own }, buffer))
    }
}

/// The fourcc to give the compositor for `buffer` (exported as `[R, G, B,
/// A]` bytes), if it takes it with the buffer's modifier: XBGR8888 (the same
/// bytes, alpha ignored) first, as a window is opaque.
fn taken_as(offered: &[(u32, u64)], buffer: &Buffer) -> Option<u32> {
    if buffer.fourcc != ABGR8888 && buffer.fourcc != XBGR8888 {
        return offered
            .contains(&(buffer.fourcc, buffer.modifier))
            .then_some(buffer.fourcc);
    }
    [XBGR8888, ABGR8888]
        .into_iter()
        .find(|&f| offered.contains(&(f, buffer.modifier)))
}

impl State {
    /// The export entry points, looked up once.
    unsafe fn export_fns(&mut self) -> Result<Fns, String> {
        let dpy = self.dpy;
        self.wayland
            .fns
            .get_or_insert_with(|| unsafe { Fns::load(dpy) })
            .clone()
    }

    /// Whether frames can go to the Wayland window `sink` is for: the
    /// display exports, and the compositor takes what it exports (tried on
    /// a small texture, imported once, the first time).
    pub(super) unsafe fn can_present_dmabuf(&mut self, sink: &dyn Present) -> bool {
        unsafe {
            if let Some(taken) = self.wayland.taken {
                return taken;
            }
            let fns = match self.export_fns() {
                Ok(f) => f,
                Err(e) => {
                    dp::say_reading_back("OpenGL ES", &e);
                    self.wayland.taken = Some(false);
                    return false;
                }
            };
            let offered = sink.formats();
            let taken = match export(self, fns, 64, 64) {
                Ok((probe, buffer)) => {
                    let as_ = taken_as(&offered, &buffer);
                    // And that it imports what it says it takes.
                    let imported = match as_ {
                        None => Err(format!(
                            "the compositor doesn't take {} with modifier {:#x}",
                            crate::platform::dmabuf::fourcc_name(buffer.fourcc),
                            buffer.modifier
                        )),
                        Some(fourcc) => sink.import(&Buffer { fourcc, ..buffer }),
                    };
                    probe.delete();
                    match imported {
                        Ok(id) => {
                            sink.destroy(id);
                            true
                        }
                        Err(e) => {
                            dp::say_reading_back("OpenGL ES", &e);
                            false
                        }
                    }
                }
                Err(e) => {
                    dp::say_reading_back("OpenGL ES", &e);
                    false
                }
            };
            self.wayland.taken = Some(taken);
            taken
        }
    }

    /// Draw `plan` into the frame target, copy it into a free buffer of the
    /// window's ring, wait for the GPU and commit it.
    pub(super) unsafe fn present_dmabuf(
        &mut self,
        plan: &mut Plan,
        surface: u32,
        sink: &dyn Present,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        unsafe {
            if !sink.ready() {
                return Err(GpuError::Skipped);
            }
            // (Checked by `can_present_dmabuf` first: read back otherwise.)
            let fns = self.export_fns().map_err(|_| GpuError::Skipped)?;
            let size = (plan.size.width, plan.size.height);
            if self
                .wayland
                .rings
                .get(&surface)
                .is_some_and(|r| r.size != size)
            {
                let mut old = self.wayland.rings.remove(&surface).unwrap();
                old.clear(Some(sink), |e| e.delete());
            }
            let mut ring = self
                .wayland
                .rings
                .remove(&surface)
                .unwrap_or_else(|| Ring::new(size));
            let r = self.frame_into(&mut ring, fns, plan, surface, sink, dirty);
            if r.is_err() {
                self.kept.forget();
            }
            self.wayland.rings.insert(surface, ring);
            r
        }
    }

    unsafe fn frame_into(
        &mut self,
        ring: &mut Ring<Exported>,
        fns: Fns,
        plan: &mut Plan,
        surface: u32,
        sink: &dyn Present,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        unsafe {
            let (w, h) = (plan.size.width as i32, plan.size.height as i32);
            let offered = sink.formats();
            let i = ring.acquire(sink, || {
                let (image, buffer) = export(self, fns, w, h).map_err(GpuError::Lost)?;
                let Some(fourcc) = taken_as(&offered, &buffer) else {
                    image.delete();
                    // Read back from the next frame on.
                    dp::say_reading_back(
                        "OpenGL ES",
                        &format!(
                            "the compositor doesn't take {} with modifier {:#x}",
                            crate::platform::dmabuf::fourcc_name(buffer.fourcc),
                            buffer.modifier
                        ),
                    );
                    self.wayland.taken = Some(false);
                    return Err(GpuError::Skipped);
                };
                let buffer = Buffer { fourcc, ..buffer };
                match sink.import(&buffer) {
                    Ok(id) => {
                        dp::say_presenting(
                            "OpenGL ES",
                            fourcc,
                            buffer.modifier,
                            buffer.planes.len(),
                        );
                        Ok(Slot { image, id })
                    }
                    Err(e) => {
                        image.delete();
                        dp::say_reading_back("OpenGL ES", &e);
                        self.wayland.taken = Some(false);
                        Err(GpuError::Skipped)
                    }
                }
            })?;
            // Only what changed, over the window's last frame, when the
            // target holds it.
            let key = surface as usize;
            if self
                .frame_target
                .as_ref()
                .is_some_and(|t| (t.w, t.h) == (w, h))
            {
                self.kept.limit(key, plan, dirty);
            }
            self.draw(plan).map_err(GpuError::Lost)?;
            // The frame target's rows are the window's, top first, as the
            // buffer's are: a straight copy.
            glBindFramebuffer(GL_FRAMEBUFFER, self.frame_target.as_ref().unwrap().fbo);
            glDisable(GL_SCISSOR_TEST);
            glBindTexture(GL_TEXTURE_2D, ring.slots[i].image.tex);
            glCopyTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, 0, 0, w, h);
            let err = glGetError();
            if err != GL_NO_ERROR {
                return Err(GpuError::Lost(format!(
                    "GL error {err:#x} copying the frame"
                )));
            }
            // The compositor reads it once the GPU is done with it.
            glFinish();
            self.kept.presented(key, (w as u32, h as u32), dirty);
            let damage = ring.damage(dirty);
            sink.commit(ring.slots[i].id, (w as u32, h as u32), damage)
                .map_err(GpuError::Lost)
        }
    }

    /// Let go of the Wayland window `surface`'s ring.
    pub(super) unsafe fn release_dmabuf(&mut self, surface: u32, sink: &dyn Present) {
        if let Some(mut ring) = self.wayland.rings.remove(&surface) {
            ring.clear(Some(sink), |e| unsafe { e.delete() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::dmabuf::{ARGB8888, MOD_LINEAR};

    #[test]
    fn rgba_exports_are_offered_as_opaque_first() {
        let buffer = |fourcc, modifier| Buffer {
            width: 1,
            height: 1,
            fourcc,
            modifier,
            planes: Vec::new(),
        };
        let offered = [(ABGR8888, 5), (XBGR8888, 5), (ABGR8888, MOD_LINEAR)];
        assert_eq!(taken_as(&offered, &buffer(ABGR8888, 5)), Some(XBGR8888));
        assert_eq!(
            taken_as(&offered, &buffer(ABGR8888, MOD_LINEAR)),
            Some(ABGR8888)
        );
        assert_eq!(taken_as(&offered, &buffer(ABGR8888, 6)), None);
        assert_eq!(taken_as(&offered, &buffer(ARGB8888, 5)), None);
    }
}
