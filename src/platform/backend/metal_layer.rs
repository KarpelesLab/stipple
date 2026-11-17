//! The GPU presentation seam on macOS and iOS: a `CAMetalLayer` laid over a
//! view's own layer, which a GPU renderer presents frames into.
//!
//! The Metal layer is a sublayer covering the view, so taking it away again
//! (the renderer fell back to the CPU) uncovers the view's own drawing; the
//! view's `drawRect:` keeps working underneath. The renderer sets the layer's
//! device, pixel format, color space and drawable size; this side only keeps
//! its frame (points) and contents scale matching the view.
#![allow(unsafe_code)]
#![cfg_attr(not(feature = "gpu"), allow(dead_code))]

use std::ffi::{CStr, c_void};

type Id = *mut c_void;
type Sel = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const std::ffi::c_char) -> Id;
    fn sel_registerName(name: *const std::ffi::c_char) -> Sel;
    fn objc_msgSend();
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

fn sel(name: &CStr) -> Sel {
    unsafe { sel_registerName(name.as_ptr()) }
}

unsafe fn send(obj: Id, s: &CStr) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, sel(s))
    }
}

unsafe fn send_id(obj: Id, s: &CStr, a: Id) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, sel(s), a)
    }
}

unsafe fn send_bool(obj: Id, s: &CStr, a: bool) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, bool) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, sel(s), a)
    }
}

unsafe fn send_f64(obj: Id, s: &CStr, a: f64) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, f64) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(obj, sel(s), a)
    }
}

/// A view's Metal layer, and the geometry last given to it.
#[derive(Debug)]
pub(crate) struct MetalLayer {
    layer: Id,
    frame: Rect,
    scale: f64,
    /// Frames present with Core Animation's transaction.
    with_transaction: bool,
}

impl MetalLayer {
    /// Lay a new `CAMetalLayer` over `parent` (a view's layer), or `None`
    /// when Core Animation has none.
    ///
    /// # Safety
    /// Main thread; `parent` is a live `CALayer`.
    pub(crate) unsafe fn attach(parent: Id) -> Option<Self> {
        unsafe {
            let cls = objc_getClass(c"CAMetalLayer".as_ptr());
            if cls.is_null() || parent.is_null() {
                return None;
            }
            let layer = send(send(cls, c"alloc"), c"init");
            if layer.is_null() {
                return None;
            }
            send_bool(layer, c"setOpaque:", true);
            // Presents wait for the display's refresh (the default, said
            // anyway: frames are paced by the display link and the layer
            // together, and an unsynced layer would tear). macOS only: iOS
            // layers always sync.
            #[cfg(target_os = "macos")]
            send_bool(layer, c"setDisplaySyncEnabled:", true);
            send_id(parent, c"addSublayer:", layer);
            Some(Self {
                layer,
                frame: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 0.0,
                },
                scale: 0.0,
                with_transaction: false,
            })
        }
    }

    /// The `CAMetalLayer`.
    pub(crate) fn layer(&self) -> Id {
        self.layer
    }

    /// Cover `width`×`height` points at `scale` pixels per point.
    ///
    /// # Safety
    /// Main thread.
    pub(crate) unsafe fn fit(&mut self, width: f64, height: f64, scale: f64) {
        let frame = Rect {
            x: 0.0,
            y: 0.0,
            w: width,
            h: height,
        };
        if frame == self.frame && scale == self.scale {
            return;
        }
        unsafe {
            // Without an implicit animation.
            let tx = objc_getClass(c"CATransaction".as_ptr());
            send(tx, c"begin");
            send_bool(tx, c"setDisableActions:", true);
            let set: unsafe extern "C" fn(Id, Sel, Rect) =
                std::mem::transmute(objc_msgSend as *const c_void);
            set(self.layer, sel(c"setFrame:"), frame);
            send_f64(self.layer, c"setContentsScale:", scale);
            send(tx, c"commit");
        }
        self.frame = frame;
        self.scale = scale;
    }

    /// Present frames with Core Animation's transaction while `on`: when
    /// the view shows live OS controls (`native::os_views`), which move in
    /// the same pass as the frame is drawn, so a scrolled frame and the
    /// controls' new places show together rather than a frame apart.
    ///
    /// # Safety
    /// Main thread.
    pub(crate) unsafe fn present_with_transaction(&mut self, on: bool) {
        if on != self.with_transaction {
            unsafe { send_bool(self.layer, c"setPresentsWithTransaction:", on) };
            self.with_transaction = on;
        }
    }

    /// Take the layer off its view and release it.
    ///
    /// # Safety
    /// Main thread.
    pub(crate) unsafe fn detach(self) {
        unsafe {
            send(self.layer, c"removeFromSuperlayer");
            send(self.layer, c"release");
        }
    }
}
