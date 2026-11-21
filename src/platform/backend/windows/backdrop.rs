//! Transparent windows (`WindowAttributes::transparent`) and the blur behind
//! them (`WindowAttributes::blur`).
//!
//! The desktop window manager draws the window's frame material into the
//! whole client area (`DwmExtendFrameIntoClientArea` with margins of -1, "a
//! sheet of glass"), and then uses the alpha of what the window paints there:
//! the frame is blitted as premultiplied `B, G, R, A`, and where its alpha is
//! below 255 the desktop shows through. On Windows 11 (22H2 on) the window can
//! ask DWM for a system backdrop under that: the acrylic one
//! (`DWMSBT_TRANSIENTWINDOW`) blurs what is behind the window; without blur
//! it asks for none, so what shows through is plain. Earlier Windows have no
//! documented blur and show the desktop through unblurred.
//!
//! Under a GPU renderer the frames go to a premultiplied swap chain that
//! DirectComposition shows over the window (`gpu::d3d`'s `dcomp`); the
//! window's own surface under it is cleared to transparent, so only the
//! swap chain's frames show.

use core::ffi::c_void;

/// `DWMWA_SYSTEMBACKDROP_TYPE` (Windows 11 22H2 and later).
const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
/// `DWMSBT_NONE`: no backdrop material.
const DWMSBT_NONE: i32 = 1;
/// `DWMSBT_TRANSIENTWINDOW`: acrylic, a blur of what is behind the window.
const DWMSBT_TRANSIENTWINDOW: i32 = 3;

/// `MARGINS`: how far the frame reaches into the client area, from each edge;
/// -1 everywhere for all of it.
#[repr(C)]
struct Margins {
    left: i32,
    right: i32,
    top: i32,
    bottom: i32,
}

/// `BLACK_BRUSH`: black is fully transparent over DWM's frame.
const BLACK_BRUSH: i32 = 4;

#[link(name = "user32")]
unsafe extern "system" {
    fn FillRect(hdc: *mut c_void, rect: *const super::Rect, brush: *mut c_void) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn GetStockObject(which: i32) -> *mut c_void;
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmExtendFrameIntoClientArea(hwnd: *mut c_void, margins: *const Margins) -> i32;
    fn DwmSetWindowAttribute(hwnd: *mut c_void, attr: u32, value: *const c_void, size: u32) -> i32;
}

/// Let what is behind `hwnd` show through where its frames are translucent,
/// blurred with `blur` where the system can.
pub(super) fn make_transparent(hwnd: *mut c_void, blur: bool) {
    let all = Margins {
        left: -1,
        right: -1,
        top: -1,
        bottom: -1,
    };
    let backdrop = if blur {
        DWMSBT_TRANSIENTWINDOW
    } else {
        DWMSBT_NONE
    };
    unsafe {
        DwmExtendFrameIntoClientArea(hwnd, &all);
        // Fails before Windows 11 22H2, which has no backdrops: the desktop
        // shows through unblurred.
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const i32 as *const c_void,
            4,
        );
    }
}

/// Clear `rect` of a transparent window's surface (`hdc`, being painted) to
/// transparent: under GPU frames, which DirectComposition shows over it, so
/// a CPU frame painted before doesn't show through them.
///
/// # Safety
/// `hdc` is the device context `BeginPaint` gave.
pub(super) unsafe fn clear(hdc: *mut c_void, rect: &super::Rect) {
    unsafe { FillRect(hdc, rect, GetStockObject(BLACK_BRUSH)) };
}
