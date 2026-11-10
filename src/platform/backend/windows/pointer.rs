//! Pointer odds and ends for the Win32 backend: `WM_MOUSELEAVE` through
//! `TrackMouseEvent`, and the user's double-click time and rectangle.

use std::cell::Cell;
use std::ffi::c_void;

pub(super) const WM_XBUTTONDOWN: u32 = 0x020B;
pub(super) const WM_XBUTTONUP: u32 = 0x020C;
pub(super) const WM_MOUSELEAVE: u32 = 0x02A3;

const TME_LEAVE: u32 = 0x0000_0002;
const SM_CXDOUBLECLK: i32 = 36;
const SM_CYDOUBLECLK: i32 = 37;

#[repr(C)]
struct TrackMouseEventInfo {
    size: u32,
    flags: u32,
    hwnd: *mut c_void,
    hover_time: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn TrackMouseEvent(info: *mut TrackMouseEventInfo) -> i32;
    fn GetDoubleClickTime() -> u32;
    fn GetSystemMetrics(index: i32) -> i32;
}

thread_local! {
    /// Whether a `WM_MOUSELEAVE` is asked for (one per entry into the window).
    static TRACKING: Cell<bool> = const { Cell::new(false) };
}

/// The pointer moved over `hwnd`: ask for a `WM_MOUSELEAVE` when it leaves,
/// if not already asked.
pub(super) fn track_leave(hwnd: *mut c_void) {
    if TRACKING.with(Cell::get) {
        return;
    }
    let mut info = TrackMouseEventInfo {
        size: std::mem::size_of::<TrackMouseEventInfo>() as u32,
        flags: TME_LEAVE,
        hwnd,
        hover_time: 0,
    };
    if unsafe { TrackMouseEvent(&mut info) } != 0 {
        TRACKING.with(|t| t.set(true));
    }
}

/// `WM_MOUSELEAVE` came: the request is spent.
pub(super) fn left() {
    TRACKING.with(|t| t.set(false));
}

/// Record the user's double-click time and distance: the rectangle
/// `SM_CXDOUBLECLK` × `SM_CYDOUBLECLK` (in pixels at `dpi`) is centered on the
/// first click.
pub(super) fn read_double_click(dpi: u32) {
    let ms = unsafe { GetDoubleClickTime() };
    let (w, h) = unsafe {
        (
            GetSystemMetrics(SM_CXDOUBLECLK),
            GetSystemMetrics(SM_CYDOUBLECLK),
        )
    };
    let scale = f64::from(dpi.max(1)) / 96.0;
    let distance = (w.max(h) > 0).then(|| f64::from(w.max(h)) / 2.0 / scale);
    crate::platform::set_double_click(
        (ms > 0).then(|| std::time::Duration::from_millis(u64::from(ms))),
        distance,
    );
}
