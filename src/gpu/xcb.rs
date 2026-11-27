//! A libxcb connection for presenting GPU frames to X11 windows.
//!
//! The X11 backend speaks the protocol itself over a socket, so it has no
//! `xcb_connection_t` to give `VK_KHR_xcb_surface` or EGL's XCB platform. A
//! window's id is the same on every connection to the server, though, so the
//! GPU opens a connection of its own to `$DISPLAY` and names the backend's
//! window on it: the driver's presentation (DRI3 and Present, or a plain
//! `PutImage` under a software driver) goes over this connection, everything
//! else stays on the backend's. libxcb is loaded at run time (`dlsym`) the
//! first time a GPU renderer presents to a window; without it (or without a
//! server), frames are read back and presented as before.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_void};

dlsym::library! {
    struct Xcb {
        unsafe extern "C" {
            fn xcb_connect(display: *const c_char, screen: *mut c_int) -> *mut c_void;
            fn xcb_connection_has_error(conn: *mut c_void) -> c_int;
        }
    }
}

/// The connection and its default screen number, opened once per process
/// and kept for its life (`xcb_connection_t` is thread-safe).
pub(crate) fn connection() -> Result<(*mut c_void, i32), String> {
    static CONN: std::sync::OnceLock<Result<(usize, i32), String>> = std::sync::OnceLock::new();
    CONN.get_or_init(open)
        .clone()
        .map(|(c, s)| (c as *mut c_void, s))
}

fn open() -> Result<(usize, i32), String> {
    // SAFETY: a system library, trusted like the rest of the platform.
    let lib = unsafe { <Xcb as dlsym::Bindings>::open("libxcb.so.1") }.map_err(|e| match e {
        dlsym::Error::SymbolNotFound { .. } => "libxcb has no xcb_connect".to_string(),
        _ => "libxcb.so.1 isn't installed".to_string(),
    })?;
    // The connection lives for the process, so its library does too.
    let lib: &'static Xcb = Box::leak(Box::new(lib));
    let mut screen: c_int = 0;
    // `$DISPLAY`, as the backend connects.
    // SAFETY: a null display name and a valid out-pointer.
    let conn = unsafe { lib.xcb_connect(std::ptr::null(), &mut screen) };
    if conn.is_null() {
        return Err("xcb_connect failed".into());
    }
    // SAFETY: a connection xcb_connect returned.
    let err = unsafe { lib.xcb_connection_has_error(conn) };
    if err != 0 {
        // A connection in error is still allocated; it's left as is
        // (never used) rather than freed through another symbol.
        return Err(format!("xcb_connect failed (error {err})"));
    }
    Ok((conn as usize, screen))
}
