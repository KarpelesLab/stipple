//! A libxcb connection for presenting GPU frames to X11 windows.
//!
//! The X11 backend speaks the protocol itself over a socket, so it has no
//! `xcb_connection_t` to give `VK_KHR_xcb_surface` or EGL's XCB platform. A
//! window's id is the same on every connection to the server, though, so the
//! GPU opens a connection of its own to `$DISPLAY` and names the backend's
//! window on it: the driver's presentation (DRI3 and Present, or a plain
//! `PutImage` under a software driver) goes over this connection, everything
//! else stays on the backend's. libxcb is loaded with `dlopen` the first time
//! a GPU renderer presents to a window; without it (or without a server),
//! frames are read back and presented as before.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_void};

unsafe extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}
const RTLD_NOW: c_int = 2;

type XcbConnect = unsafe extern "C" fn(*const c_char, *mut c_int) -> *mut c_void;
type XcbHasError = unsafe extern "C" fn(*mut c_void) -> c_int;

/// The connection and its default screen number, opened once per process
/// and kept for its life (`xcb_connection_t` is thread-safe).
pub(crate) fn connection() -> Result<(*mut c_void, i32), String> {
    static CONN: std::sync::OnceLock<Result<(usize, i32), String>> = std::sync::OnceLock::new();
    CONN.get_or_init(|| unsafe { open() })
        .clone()
        .map(|(c, s)| (c as *mut c_void, s))
}

unsafe fn open() -> Result<(usize, i32), String> {
    unsafe {
        let lib = dlopen(c"libxcb.so.1".as_ptr(), RTLD_NOW);
        if lib.is_null() {
            return Err("libxcb.so.1 isn't installed".into());
        }
        let connect = dlsym(lib, c"xcb_connect".as_ptr());
        let has_error = dlsym(lib, c"xcb_connection_has_error".as_ptr());
        if connect.is_null() || has_error.is_null() {
            return Err("libxcb has no xcb_connect".into());
        }
        let connect: XcbConnect = std::mem::transmute(connect);
        let has_error: XcbHasError = std::mem::transmute(has_error);
        let mut screen: c_int = 0;
        // `$DISPLAY`, as the backend connects.
        let conn = connect(std::ptr::null(), &mut screen);
        if conn.is_null() {
            return Err("xcb_connect failed".into());
        }
        let err = has_error(conn);
        if err != 0 {
            // A connection in error is still allocated; it's left as is
            // (never used) rather than freed through another symbol.
            return Err(format!("xcb_connect failed (error {err})"));
        }
        Ok((conn as usize, screen))
    }
}
