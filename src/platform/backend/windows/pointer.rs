//! Pointer odds and ends for the Win32 backend: `WM_MOUSELEAVE` through
//! `TrackMouseEvent`, the pointer lock (raw input's `WM_INPUT` for the
//! motion, `ClipCursor` to hold the cursor still), and the user's
//! double-click time and rectangle.

use std::cell::Cell;
use std::ffi::c_void;

pub(super) const WM_XBUTTONDOWN: u32 = 0x020B;
pub(super) const WM_XBUTTONUP: u32 = 0x020C;
pub(super) const WM_MOUSELEAVE: u32 = 0x02A3;
pub(super) const WM_INPUT: u32 = 0x00FF;

const RID_INPUT: u32 = 0x1000_0003;
const RIDEV_REMOVE: u32 = 0x0000_0001;
/// The generic desktop page's mouse (HID usage page 1, usage 2).
const HID_USAGE_PAGE_GENERIC: u16 = 1;
const HID_USAGE_GENERIC_MOUSE: u16 = 2;

#[repr(C)]
struct RawInputDevice {
    usage_page: u16,
    usage: u16,
    flags: u32,
    target: *mut c_void,
}

#[repr(C)]
struct WinRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct WinPoint {
    x: i32,
    y: i32,
}

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
    fn RegisterRawInputDevices(devices: *const RawInputDevice, count: u32, size: u32) -> i32;
    fn GetRawInputData(
        input: *mut c_void,
        command: u32,
        data: *mut c_void,
        size: *mut u32,
        header_size: u32,
    ) -> u32;
    fn ClipCursor(rect: *const WinRect) -> i32;
    fn SetCursorPos(x: i32, y: i32) -> i32;
    fn GetClientRect(hwnd: *mut c_void, rect: *mut WinRect) -> i32;
    fn ClientToScreen(hwnd: *mut c_void, point: *mut WinPoint) -> i32;
    fn SetCursor(cursor: *mut c_void) -> *mut c_void;
}

thread_local! {
    /// Whether a `WM_MOUSELEAVE` is asked for (one per entry into the window).
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    /// The window the pointer is locked to (an `HWND`), or 0.
    static LOCKED: Cell<usize> = const { Cell::new(0) };
}

/// The window the pointer is locked to, if any.
pub(super) fn locked() -> Option<*mut c_void> {
    let h = LOCKED.with(Cell::get);
    (h != 0).then_some(h as *mut c_void)
}

/// Lock the pointer to `hwnd` (`true`): register for the mouse's raw input
/// (`WM_INPUT`, unaccelerated), hide the cursor and clip it to a pixel at
/// the client area's center. Or release it. Returns whether the pointer is
/// locked (or released) as asked.
pub(super) fn lock(hwnd: *mut c_void, on: bool) -> bool {
    let device = RawInputDevice {
        usage_page: HID_USAGE_PAGE_GENERIC,
        usage: HID_USAGE_GENERIC_MOUSE,
        flags: if on { 0 } else { RIDEV_REMOVE },
        target: if on { hwnd } else { std::ptr::null_mut() },
    };
    let size = std::mem::size_of::<RawInputDevice>() as u32;
    if !on {
        if locked().is_none() {
            return true;
        }
        LOCKED.with(|l| l.set(0));
        unsafe {
            RegisterRawInputDevices(&device, 1, size);
            ClipCursor(std::ptr::null());
        }
        return true;
    }
    if unsafe { RegisterRawInputDevices(&device, 1, size) } == 0 {
        return false;
    }
    LOCKED.with(|l| l.set(hwnd as usize));
    unsafe { SetCursor(std::ptr::null_mut()) };
    clip(hwnd);
    true
}

/// Hold the cursor at the center of `hwnd`'s client area (again: the window
/// moved or was resized, which `ClipCursor` doesn't follow).
pub(super) fn clip(hwnd: *mut c_void) {
    let mut r = WinRect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe { GetClientRect(hwnd, &mut r) };
    let mut c = WinPoint {
        x: (r.right - r.left) / 2,
        y: (r.bottom - r.top) / 2,
    };
    unsafe {
        ClientToScreen(hwnd, &mut c);
        SetCursorPos(c.x, c.y);
        let pixel = WinRect {
            left: c.x,
            top: c.y,
            right: c.x + 1,
            bottom: c.y + 1,
        };
        ClipCursor(&pixel);
    }
}

/// The motion in a `WM_INPUT`'s raw input (its `lParam`, an `HRAWINPUT`),
/// if it's the mouse moving.
pub(super) fn raw_motion(lp: isize) -> Option<(i32, i32)> {
    // RAWINPUTHEADER: the type, the size, then a handle and a WPARAM.
    let header = 8 + 2 * std::mem::size_of::<usize>();
    let mut buf = [0u64; 8];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let got = unsafe {
        GetRawInputData(
            lp as *mut c_void,
            RID_INPUT,
            buf.as_mut_ptr().cast(),
            &mut size,
            header as u32,
        )
    };
    if got == u32::MAX || got as usize > std::mem::size_of_val(&buf) {
        return None;
    }
    let bytes: Vec<u8> = buf.iter().flat_map(|w| w.to_ne_bytes()).collect();
    crate::platform::input::win32_raw_mouse_delta(&bytes[..got as usize], header)
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
