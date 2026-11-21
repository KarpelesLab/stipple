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
//! window's frames are read back. A compositor that takes only `[B, G, R]`
//! orders (XRGB8888, ARGB8888) gets the frame drawn into the texture with
//! red and blue swapped (a quad sampling `.bgra`) instead of copied.
//!
//! With explicit sync the copy is followed by a native fence
//! (`EGL_ANDROID_native_fence_sync`) whose `sync_file` the commit carries;
//! without, `glFinish` before the commit. Where the compositor names its
//! main device and the surfaceless display is on another GPU, the context is
//! made on the main device's EGL device instead (`EGL_EXT_device_drm`,
//! `EGL_EXT_platform_device`); formats from tranches another device reads
//! are taken LINEAR only.

use super::*;
use crate::gpu::dmabuf_present::{self as dp, Ring, Slot};
use crate::platform::dmabuf::{ABGR8888, ARGB8888, Buffer, Plane, Present, XBGR8888, XRGB8888};

const EGL_GL_TEXTURE_2D: EGLenum = 0x30B1;
const EGL_SYNC_NATIVE_FENCE_ANDROID: EGLenum = 0x3144;
const EGL_SYNC_NATIVE_FENCE_FD_ANDROID: EGLint = 0x3145;
const EGL_NO_NATIVE_FENCE_FD_ANDROID: EGLint = -1;
const EGL_DEVICE_EXT: EGLint = 0x322C;
const EGL_DRM_DEVICE_FILE_EXT: EGLint = 0x3233;
const EGL_DRM_RENDER_NODE_FILE_EXT: EGLint = 0x3377;
const EGL_PLATFORM_DEVICE_EXT: EGLenum = 0x313F;

type EglSync = *mut c_void;
type CreateSync = unsafe extern "C" fn(EGLDisplay, EGLenum, *const EGLint) -> EglSync;
type DestroySync = unsafe extern "C" fn(EGLDisplay, EglSync) -> EGLBoolean;
type DupNativeFence = unsafe extern "C" fn(EGLDisplay, EglSync) -> EGLint;
type QueryDisplayAttrib = unsafe extern "C" fn(EGLDisplay, EGLint, *mut EGLAttrib) -> EGLBoolean;
type QueryDeviceString = unsafe extern "C" fn(*mut c_void, EGLint) -> *const c_char;
type QueryDevices = unsafe extern "C" fn(EGLint, *mut *mut c_void, *mut EGLint) -> EGLBoolean;

/// Swaps red and blue: the frame target drawn into a buffer whose bytes are
/// `[B, G, R, A]` (an RGBA texture the compositor reads as XRGB8888).
const SWIZZLE_FS: &str = "varying vec2 v_aux;\n\
uniform sampler2D u_tex;\n\
void main() { gl_FragColor = texture2D(u_tex, v_aux).bgra; }\n";

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
    fn glFlush();
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
    /// Native fences, where the display has `EGL_ANDROID_native_fence_sync`.
    fence: Option<(CreateSync, DestroySync, DupNativeFence)>,
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
            let has = |name: &str| exts.split(' ').any(|e| e == name);
            let fence = (has("EGL_ANDROID_native_fence_sync") && has("EGL_KHR_fence_sync"))
                .then(|| {
                    Some((
                        std::mem::transmute::<*const c_void, CreateSync>(
                            get(c"eglCreateSyncKHR").ok()?,
                        ),
                        std::mem::transmute::<*const c_void, DestroySync>(
                            get(c"eglDestroySyncKHR").ok()?,
                        ),
                        std::mem::transmute::<*const c_void, DupNativeFence>(
                            get(c"eglDupNativeFenceFDANDROID").ok()?,
                        ),
                    ))
                })
                .flatten();
            Ok(Fns {
                fence,
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

/// An exported texture: the texture and its planes' fds (ours to close);
/// whether the compositor reads it with red and blue swapped, and the
/// framebuffer it is drawn into through then (made the first time).
pub(super) struct Exported {
    tex: GLuint,
    fds: Vec<i32>,
    swizzled: bool,
    fbo: GLuint,
}

impl Exported {
    unsafe fn delete(self) {
        unsafe {
            if self.fbo != 0 {
                glDeleteFramebuffers(1, &self.fbo);
            }
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
    /// Whether the compositor takes what the export makes (probed once):
    /// for opaque windows, and for transparent ones (in a format with
    /// alpha).
    taken: [Option<bool>; 2],
    rings: HashMap<u32, Ring<Exported>>,
    /// The display's `dev_t`s (primary, render node), where it says.
    devices: Option<Vec<u64>>,
    /// The program drawing a frame with red and blue swapped, once needed.
    swizzle: Option<Program>,
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
        Ok((
            Exported {
                tex,
                fds: own,
                swizzled: false,
                fbo: 0,
            },
            buffer,
        ))
    }
}

/// The fourcc to give the compositor for `buffer` (exported as `[R, G, B,
/// A]` bytes), if it takes it with the buffer's modifier, and whether the
/// frame is drawn into it with red and blue swapped: XBGR8888 (the same
/// bytes, alpha ignored) first, as a window is opaque, then ABGR8888, then
/// the `[B, G, R]` orders (XRGB8888, ARGB8888: the same layout, drawn
/// swapped). A transparent window's (`alpha`) only with alpha.
fn taken_as(offered: &[(u32, u64)], buffer: &Buffer, alpha: bool) -> Option<(u32, bool)> {
    let keeps = |f: u32| !alpha || dp::has_alpha(f);
    if buffer.fourcc != ABGR8888 && buffer.fourcc != XBGR8888 {
        return (offered.contains(&(buffer.fourcc, buffer.modifier)) && keeps(buffer.fourcc))
            .then_some((buffer.fourcc, false));
    }
    [
        (XBGR8888, false),
        (ABGR8888, false),
        (XRGB8888, true),
        (ARGB8888, true),
    ]
    .into_iter()
    .find(|&(f, _)| keeps(f) && offered.contains(&(f, buffer.modifier)))
}

/// [`taken_as`] in the first of the tranches (see `dp::offered`) that takes
/// `buffer`.
fn taken_in(offered: &[Vec<(u32, u64)>], buffer: &Buffer, alpha: bool) -> Option<(u32, bool)> {
    offered.iter().find_map(|t| taken_as(t, buffer, alpha))
}

/// The function `name` of an extension, if EGL has it.
unsafe fn proc_of<T: Copy>(name: &CStr) -> Option<T> {
    unsafe {
        let p = eglGetProcAddress(name.as_ptr());
        (!p.is_null()).then(|| std::mem::transmute_copy::<*const c_void, T>(&p))
    }
}

/// An EGL device's `dev_t`s (its primary node, its render node).
unsafe fn device_devs(device: *mut c_void) -> Vec<u64> {
    unsafe {
        let Some(query) = proc_of::<QueryDeviceString>(c"eglQueryDeviceStringEXT") else {
            return Vec::new();
        };
        [EGL_DRM_DEVICE_FILE_EXT, EGL_DRM_RENDER_NODE_FILE_EXT]
            .into_iter()
            .filter_map(|what| {
                let s = query(device, what);
                if s.is_null() {
                    // Not a DRM device (or no render node): a query error,
                    // not one to keep.
                    eglGetError();
                    return None;
                }
                let path = CStr::from_ptr(s).to_str().ok()?;
                crate::platform::dmabuf::dev_of(std::path::Path::new(path))
            })
            .collect()
    }
}

/// The `dev_t`s of the GPU `dpy` is on, where EGL says (`EGL_EXT_device_query`
/// and `EGL_EXT_device_drm`); empty for a software one.
pub(super) unsafe fn display_devices(dpy: EGLDisplay) -> Vec<u64> {
    unsafe {
        let Some(query) = proc_of::<QueryDisplayAttrib>(c"eglQueryDisplayAttribEXT") else {
            return Vec::new();
        };
        let mut device: EGLAttrib = 0;
        if query(dpy, EGL_DEVICE_EXT, &mut device) == 0 || device == 0 {
            eglGetError();
            return Vec::new();
        }
        device_devs(device as *mut c_void)
    }
}

/// The offscreen display, moved to the Wayland compositor's main device
/// where it named one and `dpy` (current, from [`egl_init`]) is on another
/// GPU: a context on that device's EGL device (`EGL_EXT_platform_device`),
/// made current, `dpy`'s let go. `dpy` itself otherwise, or if that fails;
/// and `dpy` on a software renderer (its GPU unknown) stays.
pub(super) unsafe fn on_main_device(dpy: EGLDisplay) -> EGLDisplay {
    unsafe {
        let Some(main) = crate::platform::dmabuf::main_device() else {
            return dpy;
        };
        let ours = display_devices(dpy);
        if ours.is_empty() || ours.contains(&main) {
            return dpy;
        }
        let Some(query) = proc_of::<QueryDevices>(c"eglQueryDevicesEXT") else {
            return dpy;
        };
        let mut n: EGLint = 0;
        if query(0, std::ptr::null_mut(), &mut n) == 0 || n <= 0 {
            return dpy;
        }
        let mut devices = vec![std::ptr::null_mut(); n as usize];
        if query(n, devices.as_mut_ptr(), &mut n) == 0 {
            return dpy;
        }
        devices.truncate(n.max(0) as usize);
        let Some(&device) = devices.iter().find(|&&d| device_devs(d).contains(&main)) else {
            return dpy;
        };
        let old = eglGetCurrentContext();
        let attribs = [EGL_NONE as EGLAttrib];
        let other = eglGetPlatformDisplay(EGL_PLATFORM_DEVICE_EXT, device, attribs.as_ptr());
        if other.is_null() {
            return dpy;
        }
        match egl_finish(other, EGL_PBUFFER_BIT) {
            Ok(other) => {
                eglDestroyContext(dpy, old);
                other
            }
            Err(e) => {
                crate::gpu::dmabuf_present::say_reading_back(
                    "OpenGL ES",
                    &format!("can't render on the compositor's device ({e})"),
                );
                eglMakeCurrent(dpy, core::ptr::null_mut(), core::ptr::null_mut(), old);
                dpy
            }
        }
    }
}

/// A native fence after what was just drawn, as a `sync_file` (flushed).
unsafe fn native_fence(dpy: EGLDisplay, fns: Fns) -> Option<std::os::fd::OwnedFd> {
    unsafe {
        use std::os::fd::FromRawFd;
        let (create, destroy, dup) = fns.fence?;
        let attribs = [
            EGL_SYNC_NATIVE_FENCE_FD_ANDROID,
            EGL_NO_NATIVE_FENCE_FD_ANDROID,
            EGL_NONE,
        ];
        let sync = create(dpy, EGL_SYNC_NATIVE_FENCE_ANDROID, attribs.as_ptr());
        if sync.is_null() {
            eglGetError();
            return None;
        }
        // The fence exists once the commands before it are flushed.
        glFlush();
        let fd = dup(dpy, sync);
        destroy(dpy, sync);
        (fd >= 0).then(|| std::os::fd::OwnedFd::from_raw_fd(fd))
    }
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

    /// The formats the compositor takes as the display can use them:
    /// scanout tranches first, another device's LINEAR only.
    unsafe fn offered(&mut self, sink: &dyn Present) -> Vec<Vec<(u32, u64)>> {
        let dpy = self.dpy;
        let ours = self
            .wayland
            .devices
            .get_or_insert_with(|| unsafe { display_devices(dpy) })
            .clone();
        if let Some(main) = crate::platform::dmabuf::main_device() {
            dp::say_device("OpenGL ES", &ours, main);
        }
        dp::offered(&sink.tranches(), &ours)
    }

    /// Whether frames can go to the Wayland window `sink` is for: the
    /// display exports, and the compositor takes what it exports (tried on
    /// a small texture, imported once, the first time).
    pub(super) unsafe fn can_present_dmabuf(&mut self, sink: &dyn Present) -> bool {
        unsafe {
            let alpha = sink.alpha() as usize;
            if let Some(taken) = self.wayland.taken[alpha] {
                return taken;
            }
            let fns = match self.export_fns() {
                Ok(f) => f,
                Err(e) => {
                    dp::say_reading_back("OpenGL ES", &e);
                    self.wayland.taken[alpha] = Some(false);
                    return false;
                }
            };
            let offered = self.offered(sink);
            let taken = match export(self, fns, 64, 64) {
                Ok((probe, buffer)) => {
                    let as_ = taken_in(&offered, &buffer, alpha == 1);
                    // And that it imports what it says it takes.
                    let imported = match as_ {
                        None => Err(format!(
                            "the compositor doesn't take {} with modifier {:#x}",
                            crate::platform::dmabuf::fourcc_name(buffer.fourcc),
                            buffer.modifier
                        )),
                        Some((fourcc, _)) => sink.import(&Buffer { fourcc, ..buffer }),
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
            self.wayland.taken[alpha] = Some(taken);
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
            let offered = self.offered(sink);
            let i = ring.acquire(sink, || {
                let (mut image, buffer) = export(self, fns, w, h).map_err(GpuError::Lost)?;
                let Some((fourcc, swizzled)) = taken_in(&offered, &buffer, sink.alpha()) else {
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
                    self.wayland.taken[sink.alpha() as usize] = Some(false);
                    return Err(GpuError::Skipped);
                };
                let buffer = Buffer { fourcc, ..buffer };
                image.swizzled = swizzled;
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
                        self.wayland.taken[sink.alpha() as usize] = Some(false);
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
            if ring.slots[i].image.swizzled {
                self.draw_swizzled(&mut ring.slots[i].image, w, h)?;
            } else {
                // The frame target's rows are the window's, top first, as
                // the buffer's are: a straight copy.
                glBindFramebuffer(GL_FRAMEBUFFER, self.frame_target.as_ref().unwrap().fbo);
                glDisable(GL_SCISSOR_TEST);
                glBindTexture(GL_TEXTURE_2D, ring.slots[i].image.tex);
                glCopyTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, 0, 0, w, h);
            }
            let err = glGetError();
            if err != GL_NO_ERROR {
                return Err(GpuError::Lost(format!(
                    "GL error {err:#x} copying the frame"
                )));
            }
            // With explicit sync, a fence the compositor waits for; else
            // the compositor reads it once the GPU is done with it.
            let fence = if sink.explicit_sync() {
                let fence = native_fence(self.dpy, fns);
                dp::say_sync("OpenGL ES", fence.is_some());
                fence
            } else {
                None
            };
            if fence.is_none() {
                glFinish();
            }
            self.kept.presented(key, (w as u32, h as u32), dirty);
            let damage = ring.damage(dirty);
            sink.commit(ring.slots[i].id, (w as u32, h as u32), damage, fence)
                .map_err(GpuError::Lost)
        }
    }

    /// Draw the frame target into `image` with red and blue swapped, row
    /// for row (the buffer's rows are the window's, top first, as the
    /// target's are).
    unsafe fn draw_swizzled(
        &mut self,
        image: &mut Exported,
        w: i32,
        h: i32,
    ) -> Result<(), GpuError> {
        unsafe {
            if self.wayland.swizzle.is_none() {
                let p = Program::new(SWIZZLE_FS, &["u_tex"]).map_err(GpuError::Lost)?;
                self.wayland.swizzle = Some(p);
            }
            if image.fbo == 0 {
                glGenFramebuffers(1, &mut image.fbo);
                glBindFramebuffer(GL_FRAMEBUFFER, image.fbo);
                glFramebufferTexture2D(
                    GL_FRAMEBUFFER,
                    GL_COLOR_ATTACHMENT0,
                    GL_TEXTURE_2D,
                    image.tex,
                    0,
                );
                if glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE {
                    return Err(GpuError::Lost(
                        "the exported texture can't be drawn into".into(),
                    ));
                }
            }
            glBindFramebuffer(GL_FRAMEBUFFER, image.fbo);
            glViewport(0, 0, w, h);
            glDisable(GL_SCISSOR_TEST);
            glDisable(GL_STENCIL_TEST);
            glDisable(GL_BLEND);
            glColorMask(1, 1, 1, 1);
            let p = self.wayland.swizzle.as_ref().unwrap();
            glUseProgram(p.id);
            // Device pixels to clip space, y = 0 at texel row 0.
            glUniform4f(p.loc("u_xf"), 2.0 / w as f32, 2.0 / h as f32, -1.0, -1.0);
            glActiveTexture(GL_TEXTURE0);
            glBindTexture(GL_TEXTURE_2D, self.frame_target.as_ref().unwrap().tex);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
            glUniform1i(p.loc("u_tex"), 0);
            let (fw, fh) = (w as f32, h as f32);
            let mut v = Vec::with_capacity(24);
            for (x, y) in [
                (0.0, 0.0),
                (fw, 0.0),
                (0.0, fh),
                (0.0, fh),
                (fw, 0.0),
                (fw, fh),
            ] {
                v.extend_from_slice(&[x, y, x / fw, y / fh]);
            }
            self.vertices(p, &v);
            glDrawArrays(GL_TRIANGLES, 0, 6);
            Ok(())
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
    use crate::platform::dmabuf::MOD_LINEAR;

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
        assert_eq!(
            taken_as(&offered, &buffer(ABGR8888, 5), false),
            Some((XBGR8888, false))
        );
        assert_eq!(
            taken_as(&offered, &buffer(ABGR8888, MOD_LINEAR), false),
            Some((ABGR8888, false))
        );
        assert_eq!(taken_as(&offered, &buffer(ABGR8888, 6), false), None);
        assert_eq!(taken_as(&offered, &buffer(ARGB8888, 5), false), None);
        // Only [B, G, R] orders: drawn swapped, opaque first.
        let offered = [(ARGB8888, 5), (XRGB8888, 5)];
        assert_eq!(
            taken_as(&offered, &buffer(ABGR8888, 5), false),
            Some((XRGB8888, true))
        );
        assert_eq!(
            taken_as(&[(ARGB8888, 5)], &buffer(XBGR8888, 5), false),
            Some((ARGB8888, true))
        );
        // A transparent window's: with alpha only.
        let offered = [(XBGR8888, 5), (ABGR8888, 5), (XRGB8888, 5)];
        assert_eq!(
            taken_as(&offered, &buffer(ABGR8888, 5), true),
            Some((ABGR8888, false))
        );
        assert_eq!(taken_as(&[(XBGR8888, 5)], &buffer(ABGR8888, 5), true), None);
        assert_eq!(taken_as(&[(XRGB8888, 5)], &buffer(XRGB8888, 5), true), None);
    }
}
