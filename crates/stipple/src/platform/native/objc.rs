//! The small part of the Objective-C runtime the native renderers use.

use std::ffi::{CString, c_void};

pub type Id = *mut c_void;
pub type Sel = *mut c_void;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    pub fn objc_msgSend();
    fn objc_getClass(name: *const std::ffi::c_char) -> Id;
    fn sel_registerName(name: *const std::ffi::c_char) -> Sel;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CgSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct CgRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub fn class(name: &str) -> Id {
    let c = CString::new(name).expect("class name");
    unsafe { objc_getClass(c.as_ptr()) }
}

pub fn sel(name: &str) -> Sel {
    let c = CString::new(name).expect("selector");
    unsafe { sel_registerName(c.as_ptr()) }
}

/// Send a message with a fixed signature: `msg!(obj, "setX:", (x: f64) -> ())`.
macro_rules! msg {
    ($obj:expr, $sel:expr, ($($arg:ident : $ty:ty),*) -> $ret:ty) => {{
        let f: unsafe extern "C" fn($crate::platform::native::objc::Id,
            $crate::platform::native::objc::Sel $(, $ty)*) -> $ret =
            std::mem::transmute($crate::platform::native::objc::objc_msgSend as *const std::ffi::c_void);
        f($obj, $crate::platform::native::objc::sel($sel) $(, $arg)*)
    }};
}
pub(super) use msg;

/// `[NSString stringWithUTF8String:s]`.
#[cfg(target_os = "macos")]
pub fn nsstring(s: &str) -> Id {
    let c = CString::new(s).expect("string");
    let p = c.as_ptr();
    unsafe { msg!(class("NSString"), "stringWithUTF8String:", (p: *const std::ffi::c_char) -> Id) }
}

/// Run `f` inside an autorelease pool, for calls made outside the event loop.
pub fn with_pool<R>(f: impl FnOnce() -> R) -> R {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let r = f();
        objc_autoreleasePoolPop(pool);
        r
    }
}
