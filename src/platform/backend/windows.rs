//! A native Windows backend over raw Win32 FFI (`user32`/`gdi32`/`kernel32`).
//!
//! No `windows`/`winapi` crate — direct `extern "system"` declarations, so the
//! only "dependency" is the OS itself (the workspace policy in `ROADMAP.md`
//! §1). This module is the one place `unsafe` is permitted in the crate.
//!
//! Scope (v1): register a window class, create + show a top-level window, and
//! present the software [`Pixmap`] by blitting a top-down 32-bit DIB with
//! `StretchDIBits` on `WM_PAINT`. Input: mouse buttons, motion and the wheel
//! (`WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL`), keys with their modifiers (`GetKeyState`)
//! and typed text from `WM_CHAR`.
//!
//! **Verification:** build-checked on the `windows-latest` CI runner via the
//! workspace build matrix; runtime-screenshotted by the Visual workflow's
//! Windows job.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::geometry::{PhysicalSize, ScaleFactor, Size};
use crate::platform::ControlFlow;
use crate::platform::error::PlatformError;
use crate::platform::event::{ButtonState, Event, KeyCode, Modifiers, PointerButton, WindowId};
use crate::platform::input;
use crate::platform::menu::{MenuPopup, NativeMenu, NativeMenuBar, NativeMenuItem, NativeTray};
use crate::platform::window::{
    CursorIcon, Window, WindowAttributes, WindowInfo, debug_window, fit_icon,
};
use crate::render::{Pixmap, Surface};

/// Focus, session end, attention requests and the beep.
mod attention;
/// Receiving drops (OLE `IDropTarget`, `WM_DROPFILES`).
mod dnd;
/// Resize increments (`WM_SIZING`) and UTF-16 titles.
mod geometry;
/// Input methods through IMM32, the fallback for TSF.
mod ime;
/// Raw key events (`Event::RawKey`).
mod keys;
/// `WM_MOUSELEAVE` and the double-click settings.
mod pointer;
/// The notification-area icon and notification balloons.
mod tray;
/// Input methods through the Text Services Framework.
mod tsf;
/// Pacing animation frames by the compositor (`DwmFlush`).
mod vsync;

pub(crate) use tray::notify;

use geometry::{from_wide, wide};

// ---- Win32 type aliases -----------------------------------------------------

type Handle = *mut c_void;
type Hwnd = Handle;
type Wparam = usize;
type Lparam = isize;
type Lresult = isize;
type WndProc = unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult;

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
/// An undecorated window: `WS_POPUP`, with the sizing border (`WS_THICKFRAME`)
/// and taskbar minimize/maximize (`WS_MINIMIZEBOX | WS_MAXIMIZEBOX`).
const WS_POPUP_RESIZABLE: u32 = 0x8000_0000 | 0x0004_0000 | 0x0002_0000 | 0x0001_0000;
const CW_USEDEFAULT: i32 = -2147483648; // 0x80000000
const SW_SHOW: i32 = 5;
const SW_MAXIMIZE: i32 = 3;
const SW_RESTORE: i32 = 9;
const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;
const SW_MINIMIZE: i32 = 6;
/// The resizable border and the maximize button, dropped for fixed-size
/// windows.
const WS_THICKFRAME: u32 = 0x0004_0000;
const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
const WM_MOVE: u32 = 0x0003;
const WM_NCDESTROY: u32 = 0x0082;
/// The user is dragging a window edge (`lParam`: the window rectangle).
const WM_SIZING: u32 = 0x0214;
const WM_SETCURSOR: u32 = 0x0020;
const WM_GETMINMAXINFO: u32 = 0x0024;
const WM_SETICON: u32 = 0x0080;
/// `WM_SETCURSOR`'s hit-test code for the client area.
const HTCLIENT: u16 = 1;
const ICON_SMALL: usize = 0;
const ICON_BIG: usize = 1;
const GWL_STYLE: i32 = -16;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_FRAMECHANGED: u32 = 0x0020;
const SWP_NOOWNERZORDER: u32 = 0x0200;
const MONITOR_DEFAULTTONEAREST: u32 = 2;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_PAINT: u32 = 0x000F;
const WM_CLOSE: u32 = 0x0010;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
/// F10 and keys pressed with Alt arrive as "system" keys.
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_MOUSEHWHEEL: u32 = 0x020E;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_GETOBJECT: u32 = 0x003D;
const WM_SETTINGCHANGE: u32 = 0x001A;
const WM_THEMECHANGED: u32 = 0x031A;
const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;
const WM_DPICHANGED: u32 = 0x02E0;
const WM_QUIT: u32 = 0x0012;
/// A menu item was picked (`HIWORD(wParam)` 0, `lParam` 0 for menus).
const WM_COMMAND: u32 = 0x0111;
const MF_STRING: u32 = 0x0000;
const MF_GRAYED: u32 = 0x0001;
const MF_CHECKED: u32 = 0x0008;
const MF_POPUP: u32 = 0x0010;
const MF_SEPARATOR: u32 = 0x0800;
/// `TrackPopupMenuEx`: return the picked id (0 for none) instead of posting
/// `WM_COMMAND`, and send no `WM_INITMENU*`/`WM_MENUSELECT` notifications.
const TPM_RETURNCMD: u32 = 0x0100;
const TPM_NONOTIFY: u32 = 0x0080;
const WM_NULL: u32 = 0x0000;
/// Posted by [`crate::platform::wake`] to end the message wait.
const WM_APP_WAKE: u32 = 0x8000 + 1;
const PM_REMOVE: u32 = 0x0001;
const QS_ALLINPUT: u32 = 0x04FF;
/// Also return for input already in the queue but not yet looked at.
const MWMO_INPUTAVAILABLE: u32 = 0x0004;
const INFINITE: u32 = u32::MAX;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_NOSIZE: u32 = 0x0001;
/// `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`.
const PER_MONITOR_AWARE_V2: isize = -4;
/// `lParam` value identifying UIA's request for our root fragment provider.
const UIA_ROOT_OBJECT_ID: i32 = -25;

const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_LWIN: i32 = 0x5B;
const VK_RWIN: i32 = 0x5C;
const BI_RGB: u32 = 0;
const DIB_RGB_COLORS: u32 = 0;
const SRCCOPY: u32 = 0x00CC_0020;
const IDC_ARROW: usize = 32512;

/// The system cursor (`IDC_*` resource id) for each [`CursorIcon`]. Windows
/// has no grab cursors; the hand and the four-way arrow stand in.
fn cursor_id(icon: CursorIcon) -> usize {
    match icon {
        CursorIcon::Default => IDC_ARROW,
        CursorIcon::Text => 32513,                        // IDC_IBEAM
        CursorIcon::Crosshair => 32515,                   // IDC_CROSS
        CursorIcon::NwseResize => 32642,                  // IDC_SIZENWSE
        CursorIcon::NeswResize => 32643,                  // IDC_SIZENESW
        CursorIcon::EwResize => 32644,                    // IDC_SIZEWE
        CursorIcon::NsResize => 32645,                    // IDC_SIZENS
        CursorIcon::Move | CursorIcon::Grabbing => 32646, // IDC_SIZEALL
        CursorIcon::NotAllowed => 32648,                  // IDC_NO
        CursorIcon::Pointer | CursorIcon::Grab => 32649,  // IDC_HAND
        CursorIcon::Hidden => IDC_ARROW,                  // see `load_cursor`
    }
}

/// The `HCURSOR` to show for `icon`: none at all for
/// [`CursorIcon::Hidden`].
fn load_cursor(icon: CursorIcon) -> Handle {
    if icon == CursorIcon::Hidden {
        return std::ptr::null_mut();
    }
    unsafe { LoadCursorW(std::ptr::null_mut(), cursor_id(icon)) }
}

#[repr(C)]
struct WndClassW {
    style: u32,
    lpfn_wnd_proc: Option<WndProc>,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: Handle,
    h_icon: Handle,
    h_cursor: Handle,
    hbr_background: Handle,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    w_param: Wparam,
    l_param: Lparam,
    time: u32,
    pt: Point,
    private: u32,
}

#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// `MINMAXINFO`, filled in on `WM_GETMINMAXINFO`.
#[repr(C)]
struct MinMaxInfo {
    reserved: Point,
    max_size: Point,
    max_position: Point,
    min_track_size: Point,
    max_track_size: Point,
}

/// `MONITORINFO`.
#[repr(C)]
struct MonitorInfo {
    cb_size: u32,
    rc_monitor: Rect,
    rc_work: Rect,
    flags: u32,
}

#[repr(C)]
struct PaintStruct {
    hdc: Handle,
    f_erase: i32,
    rc_paint: Rect,
    f_restore: i32,
    f_inc_update: i32,
    rgb_reserved: [u8; 32],
}

#[repr(C)]
struct BitmapInfoHeader {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels_per_meter: i32,
    bi_y_pels_per_meter: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 1],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Handle;
}

#[link(name = "uiautomationcore")]
unsafe extern "system" {
    /// Package a UIA provider as the `WM_GETOBJECT` reply (handles the COM
    /// marshalling and `AddRef`), returning the `LRESULT` to pass back.
    fn UiaReturnRawElementProvider(hwnd: Hwnd, wp: Wparam, lp: Lparam, el: *mut c_void) -> Lresult;
}

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(hwnd: Hwnd, attr: u32, value: *const c_void, size: u32) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    fn LoadCursorW(instance: Handle, name: usize) -> Handle;
    #[allow(clippy::too_many_arguments)]
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Hwnd;
    fn SetProcessDpiAwarenessContext(context: isize) -> i32;
    fn GetDpiForSystem() -> u32;
    fn GetDpiForWindow(hwnd: Hwnd) -> u32;
    fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
    fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
    fn IsZoomed(hwnd: Hwnd) -> i32;
    fn IsIconic(hwnd: Hwnd) -> i32;
    fn UpdateWindow(hwnd: Hwnd) -> i32;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wp: Wparam, lp: Lparam) -> Lresult;
    fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
    fn PostMessageW(hwnd: Hwnd, msg: u32, wp: Wparam, lp: Lparam) -> i32;
    fn MsgWaitForMultipleObjectsEx(
        count: u32,
        handles: *const Handle,
        timeout_ms: u32,
        wake_mask: u32,
        flags: u32,
    ) -> u32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> Lresult;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: i32) -> i32;
    fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
    fn GetKeyState(vk: i32) -> i16;
    fn SetCapture(hwnd: Hwnd) -> Hwnd;
    fn ReleaseCapture() -> i32;
    fn SetCursor(cursor: Handle) -> Handle;
    fn SendMessageW(hwnd: Hwnd, msg: u32, wp: Wparam, lp: Lparam) -> Lresult;
    fn CreateIcon(
        instance: Handle,
        width: i32,
        height: i32,
        planes: u8,
        bits_per_pixel: u8,
        and_bits: *const u8,
        xor_bits: *const u8,
    ) -> Handle;
    fn DestroyIcon(icon: Handle) -> i32;
    fn GetWindowLongW(hwnd: Hwnd, index: i32) -> i32;
    fn SetWindowLongW(hwnd: Hwnd, index: i32, value: i32) -> i32;
    fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn CreateMenu() -> Handle;
    fn CreatePopupMenu() -> Handle;
    fn AppendMenuW(menu: Handle, flags: u32, id: usize, text: *const u16) -> i32;
    fn SetMenu(hwnd: Hwnd, menu: Handle) -> i32;
    fn DestroyMenu(menu: Handle) -> i32;
    fn DrawMenuBar(hwnd: Hwnd) -> i32;
    fn TrackPopupMenuEx(
        menu: Handle,
        flags: u32,
        x: i32,
        y: i32,
        hwnd: Hwnd,
        params: *const c_void,
    ) -> i32;
    fn ClientToScreen(hwnd: Hwnd, point: *mut Point) -> i32;
    fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    fn MonitorFromWindow(hwnd: Hwnd, flags: u32) -> Handle;
    fn GetMonitorInfoW(monitor: Handle, info: *mut MonitorInfo) -> i32;
    fn AdjustWindowRectExForDpi(
        rect: *mut Rect,
        style: u32,
        menu: i32,
        ex_style: u32,
        dpi: u32,
    ) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    #[allow(clippy::too_many_arguments)]
    fn StretchDIBits(
        hdc: Handle,
        x_dest: i32,
        y_dest: i32,
        w_dest: i32,
        h_dest: i32,
        x_src: i32,
        y_src: i32,
        w_src: i32,
        h_src: i32,
        bits: *const c_void,
        bmi: *const BitmapInfo,
        usage: u32,
        rop: u32,
    ) -> i32;
}

/// One window's state, shared between the message loop, the [`WndProc`]
/// (which blits on `WM_PAINT`), and the [`Surface`] (which updates the buffer).
#[derive(Default)]
struct PerWin {
    /// Top-down BGRA framebuffer for the current frame.
    fb: Vec<u8>,
    fb_w: i32,
    fb_h: i32,
    /// The current UIA provider tree, vended from `WM_GETOBJECT`. Rebuilt each
    /// time the App pushes a new accessibility tree.
    uia: Option<crate::platform::uia::UiaTree>,
    /// The window's DPI. 0 until the window exists, which reads as 96 (1x).
    dpi: u32,
    /// The client area, in physical pixels.
    size: PhysicalSize,
    /// Inner size limits in logical pixels, applied on `WM_GETMINMAXINFO`.
    min_size: Option<Size>,
    max_size: Option<Size>,
    /// Resizing snaps to whole steps of this (logical) size (`WM_SIZING`).
    increments: Option<Size>,
    /// While fullscreen: the style and window rectangle to restore.
    restore: Option<(i32, [i32; 4])>,
    /// The `HICON` set with `WM_SETICON`, destroyed when replaced.
    icon: usize,
    /// The window's menu bar (`HMENU`), destroyed when replaced.
    menu: usize,
    /// Where the window is and its state, as last reported (`WM_MOVE`,
    /// `WM_SIZE`).
    info: Option<WindowInfo>,
}

/// Per-thread state shared between the message loop, the [`WndProc`] and the
/// windows.
#[derive(Default)]
struct WinCtx {
    /// Every open window's state, by `HWND`.
    wins: HashMap<usize, PerWin>,
    /// The limits, increments and DPI of the window being created, for the
    /// messages it gets before `CreateWindowExW` returns its handle.
    pending: (Option<Size>, Option<Size>, Option<Size>, u32),
    /// The window whose message is being handled: events go to it.
    current: usize,
    /// Events pushed by the [`WndProc`] for a window, drained by the loop.
    queue: Vec<(usize, Event)>,
    /// Windows opened and closed from the handler, for the loop to adopt and
    /// destroy once it returns.
    opened: Vec<usize>,
    closed: Vec<usize>,
    /// The cursor shown over the client area (`WM_SETCURSOR`).
    cursor: CursorIcon,
    /// While a text field has focus: its caret, in client physical pixels,
    /// where the IME's windows go. `None` while the IME is off.
    ime_area: Option<Rect>,
    /// Pairs up `WM_CHAR`'s surrogates.
    utf16: input::Utf16Joiner,
    /// The app consumed the last key press as a raw key: the `WM_CHAR`s
    /// `TranslateMessage` posted for it are dropped. (Keys go to one window
    /// at a time, so this is shared.)
    drop_chars: bool,
    /// The tray icon's menu and icon.
    tray: tray::TrayState,
    /// Whether the app takes the drag over the window (its last
    /// `set_drop_accepted`), and where the drag last was.
    drop_accepted: bool,
    drag_last: Option<crate::geometry::Point>,
    /// A drag out of the app the app started with `Window::start_drag` from
    /// a window, for the message loop to run (see `dnd::drag_out`).
    drag_out: Option<(usize, crate::platform::DropData)>,
}

/// The window [`crate::platform::wake`] posts to (any open one).
static WAKE_TARGET: AtomicUsize = AtomicUsize::new(0);

/// Run `f` on `hwnd`'s state, creating it (from the pending window's limits)
/// if it's new.
fn with_win<R>(hwnd: Hwnd, f: impl FnOnce(&mut PerWin) -> R) -> R {
    CTX.with(|c| {
        let mut c = c.borrow_mut();
        let c = &mut *c;
        let (min_size, max_size, increments, dpi) = c.pending;
        let w = c.wins.entry(hwnd as usize).or_insert_with(|| PerWin {
            min_size,
            max_size,
            increments,
            dpi,
            ..Default::default()
        });
        f(w)
    })
}

/// Read `hwnd`'s state, if it has any.
fn read_win<R>(hwnd: Hwnd, f: impl FnOnce(&PerWin) -> R) -> Option<R> {
    CTX.with(|c| c.borrow().wins.get(&(hwnd as usize)).map(f))
}

/// Physical pixels per logical pixel for `hwnd`'s current display.
fn scale_of(hwnd: Hwnd) -> f64 {
    match read_win(hwnd, |w| w.dpi).unwrap_or(0) {
        0 => 1.0,
        dpi => dpi as f64 / 96.0,
    }
}

/// Physical pixels per logical pixel for the display of the window whose
/// message is being handled.
fn current_scale() -> f64 {
    let hwnd = CTX.with(|c| c.borrow().current);
    scale_of(hwnd as Hwnd)
}

/// Run `f` with `hwnd` as the window events go to.
fn with_current<R>(hwnd: Hwnd, f: impl FnOnce() -> R) -> R {
    let prev = CTX.with(|c| std::mem::replace(&mut c.borrow_mut().current, hwnd as usize));
    let r = f();
    CTX.with(|c| c.borrow_mut().current = prev);
    r
}

/// Queue `ev` for the window whose message is being handled.
fn push_event(ev: Event) {
    CTX.with(|c| {
        let mut c = c.borrow_mut();
        let hwnd = c.current;
        c.queue.push((hwnd, ev));
    });
}

/// Queue `ev` for `hwnd`.
fn push_event_to(hwnd: Hwnd, ev: Event) {
    CTX.with(|c| c.borrow_mut().queue.push((hwnd as usize, ev)));
}

fn zero_rect() -> Rect {
    Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    }
}

/// The window's rectangle on the desktop (frame included) and its client
/// area's size, in physical pixels.
fn window_rects(hwnd: Hwnd) -> (Rect, Rect) {
    let (mut outer, mut client) = (zero_rect(), zero_rect());
    unsafe {
        GetWindowRect(hwnd, &mut outer);
        GetClientRect(hwnd, &mut client);
    }
    (outer, client)
}

/// The work area of `hwnd`'s monitor, in physical desktop pixels.
fn work_area(hwnd: Hwnd) -> Option<Rect> {
    let mut info = MonitorInfo {
        cb_size: core::mem::size_of::<MonitorInfo>() as u32,
        rc_monitor: zero_rect(),
        rc_work: zero_rect(),
        flags: 0,
    };
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0).then_some(info.rc_work)
}

/// Where `hwnd` is and its state, read from the system. While minimized, the
/// position and sizes are the ones it had before (`prev`).
fn read_info(hwnd: Hwnd, prev: Option<WindowInfo>) -> WindowInfo {
    let scale = scale_of(hwnd);
    let logical = |v: i32| v as f64 / scale;
    let (outer, client) = window_rects(hwnd);
    let minimized = unsafe { IsIconic(hwnd) } != 0;
    let maximized = unsafe { IsZoomed(hwnd) } != 0;
    let fullscreen = read_win(hwnd, |w| w.restore.is_some()).unwrap_or(false);
    let work_area = work_area(hwnd).map(|r| {
        crate::geometry::Rect::from_xywh(
            logical(r.left),
            logical(r.top),
            logical(r.right - r.left),
            logical(r.bottom - r.top),
        )
    });
    let mut info = WindowInfo {
        position: Some(crate::geometry::Point::new(
            logical(outer.left),
            logical(outer.top),
        )),
        inner_size: Size::new(
            logical(client.right - client.left),
            logical(client.bottom - client.top),
        ),
        outer_size: Size::new(
            logical(outer.right - outer.left),
            logical(outer.bottom - outer.top),
        ),
        scale: ScaleFactor::new(scale),
        maximized,
        minimized,
        fullscreen,
        work_area,
    };
    if minimized && let Some(prev) = prev {
        info.position = prev.position;
        info.inner_size = prev.inner_size;
        info.outer_size = prev.outer_size;
    }
    info
}

/// Read `hwnd`'s position and state again after a move or resize, and tell
/// the app with [`Event::WindowChanged`] when anything but its size changed.
fn refresh_info(hwnd: Hwnd) {
    let prev = read_win(hwnd, |w| w.info).flatten();
    let now = read_info(hwnd, prev);
    with_win(hwnd, |w| w.info = Some(now));
    let Some(prev) = prev else {
        return;
    };
    if prev.position != now.position
        || prev.maximized != now.maximized
        || prev.minimized != now.minimized
        || prev.fullscreen != now.fullscreen
        || prev.work_area != now.work_area
    {
        push_event_to(hwnd, Event::WindowChanged);
        debug_window(format_args!(
            "state {:#x} maximized={} minimized={} fullscreen={}",
            hwnd as u64, now.maximized, now.minimized, now.fullscreen
        ));
    }
}

/// The window's title, read back from the system.
fn window_text(hwnd: Hwnd) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) }.max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    from_wide(&buf[..n.max(0) as usize])
}

/// With `STIPPLE_DEBUG_WINDOWS`, report the window's position and client
/// size as the system has them.
fn debug_frame(hwnd: Hwnd) {
    let (outer, client) = window_rects(hwnd);
    let scale = scale_of(hwnd);
    let l = |v: i32| (v as f64 / scale).round() as i64;
    debug_window(format_args!(
        "frame {:#x} pos={},{} inner={}x{}",
        hwnd as u64,
        l(outer.left),
        l(outer.top),
        l(client.right - client.left),
        l(client.bottom - client.top)
    ));
}

/// Extract the signed `(x, y)` from a mouse-message `lParam` (LOWORD/HIWORD).
/// Returns a Stipple logical-pixel point (distinct from the local Win32 `Point`).
fn mouse_xy(lp: Lparam) -> crate::geometry::Point {
    let x = (lp & 0xffff) as u16 as i16 as f64;
    let y = ((lp >> 16) & 0xffff) as u16 as i16 as f64;
    // Windows reports physical pixels under per-monitor DPI awareness.
    let scale = current_scale();
    crate::geometry::Point::new(x / scale, y / scale)
}

/// The modifier keys held when the message being handled was posted
/// (`GetKeyState` follows the message queue, not the live keyboard).
fn modifiers() -> Modifiers {
    // The high bit (a negative SHORT) means the key is down.
    let down = |vk| unsafe { GetKeyState(vk) } < 0;
    Modifiers {
        shift: down(VK_SHIFT),
        ctrl: down(VK_CONTROL),
        alt: down(VK_MENU),
        meta: down(VK_LWIN) || down(VK_RWIN),
    }
}

/// The key event for a `WM_(SYS)KEYDOWN`/`UP`: a named key, or — with Ctrl,
/// Alt or the Windows key held — a letter/digit shortcut such as Ctrl+C, whose
/// `WM_CHAR` is only a control code (Alt+F arrives as `WM_SYSCHAR`). Ctrl+Alt
/// is AltGr, which types text.
fn key_event(vk: usize, state: ButtonState) -> Option<Event> {
    let modifiers = modifiers();
    let shortcut = modifiers.ctrl != modifiers.alt || modifiers.meta;
    let code = input::win32_vk(vk).or_else(|| {
        shortcut
            .then(|| input::win32_shortcut_char(vk, modifiers.shift))
            .flatten()
            .map(KeyCode::Char)
    })?;
    Some(Event::Key {
        code,
        state,
        modifiers,
    })
}

/// Queue the raw key event for a key message, ahead of its [`Event::Key`].
/// A new key press also ends the dropping of the previous one's characters.
fn push_raw_key(vk: usize, lp: Lparam, state: ButtonState) {
    if state == ButtonState::Pressed {
        CTX.with(|c| c.borrow_mut().drop_chars = false);
    }
    if let Some(ev) = keys::raw_event(vk, lp, state, modifiers()) {
        push_event(ev);
    }
}

thread_local! {
    static CTX: RefCell<WinCtx> = RefCell::new(WinCtx::default());
}

/// Append `items` to the menu `menu`: `&` mnemonics pass through, and a
/// shortcut shows right-aligned after a tab.
fn fill_menu(menu: Handle, items: &[NativeMenuItem]) {
    for item in items {
        match item {
            NativeMenuItem::Action {
                id,
                label,
                shortcut,
                enabled,
                checked,
            } => {
                let mut text = label.clone();
                if let Some(s) = shortcut {
                    text.push('\t');
                    text.push_str(&s.label_for(false));
                }
                let mut flags = MF_STRING;
                if !enabled {
                    flags |= MF_GRAYED;
                }
                if *checked == Some(true) {
                    flags |= MF_CHECKED;
                }
                let text = wide(&text);
                unsafe { AppendMenuW(menu, flags, *id as usize, text.as_ptr()) };
            }
            NativeMenuItem::Separator => unsafe {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            },
            NativeMenuItem::Submenu(sub) => {
                let popup = unsafe { CreatePopupMenu() };
                fill_menu(popup, &sub.items);
                let text = wide(&sub.label);
                unsafe { AppendMenuW(menu, MF_POPUP, popup as usize, text.as_ptr()) };
            }
        }
    }
}

/// Show `menu` as a context menu at `at` (logical client coordinates) and
/// wait for it to close. `TrackPopupMenuEx` runs its own modal loop; the
/// window keeps painting meanwhile, and input it gets is queued as usual.
fn popup_menu(hwnd: Hwnd, menu: &NativeMenu, at: crate::geometry::Point) -> MenuPopup {
    let popup = unsafe { CreatePopupMenu() };
    if popup.is_null() {
        return MenuPopup::Unsupported;
    }
    fill_menu(popup, &menu.items);
    let scale = current_scale();
    let mut p = Point {
        x: (at.x * scale).round() as i32,
        y: (at.y * scale).round() as i32,
    };
    let picked = unsafe {
        ClientToScreen(hwnd, &mut p);
        // The menu only closes on a click elsewhere if its owner is the
        // foreground window, and needs a message after it to settle (see
        // `TrackPopupMenu`'s remarks).
        SetForegroundWindow(hwnd);
        let id = TrackPopupMenuEx(
            popup,
            TPM_RETURNCMD | TPM_NONOTIFY,
            p.x,
            p.y,
            hwnd,
            std::ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(popup);
        id
    };
    if picked > 0 {
        MenuPopup::Picked(picked as u32)
    } else {
        MenuPopup::Dismissed
    }
}

/// Give `hwnd` a menu bar for `bar`, replacing the previous one. The first one
/// grows the window by the bar's height, so the client area keeps its size.
fn set_menu_bar(hwnd: Hwnd, bar: &NativeMenuBar) {
    let menu = unsafe { CreateMenu() };
    for m in &bar.menus {
        let popup = unsafe { CreatePopupMenu() };
        fill_menu(popup, &m.items);
        let text = wide(&m.label);
        unsafe { AppendMenuW(menu, MF_POPUP, popup as usize, text.as_ptr()) };
    }
    let client = |hwnd| {
        let mut r = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe { GetClientRect(hwnd, &mut r) };
        r.bottom - r.top
    };
    let before = client(hwnd);
    unsafe { SetMenu(hwnd, menu) };
    let old = with_win(hwnd, |w| std::mem::replace(&mut w.menu, menu as usize));
    if old != 0 {
        // Destroys its submenus too.
        unsafe { DestroyMenu(old as Handle) };
    } else {
        let lost = before - client(hwnd);
        let mut w = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if lost > 0 && unsafe { GetWindowRect(hwnd, &mut w) } != 0 {
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    w.right - w.left,
                    w.bottom - w.top + lost,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
        }
    }
    unsafe { DrawMenuBar(hwnd) };
}

/// Whether a `WM_SETTINGCHANGE` `lParam` (a NUL-terminated UTF-16 string, or
/// null) is `name`.
///
/// # Safety
/// `lp` must be null or point to a NUL-terminated UTF-16 string.
unsafe fn names_setting(lp: Lparam, name: &str) -> bool {
    let p = lp as *const u16;
    if p.is_null() {
        return false;
    }
    let mut want = name.encode_utf16();
    let mut i = 0;
    loop {
        let c = unsafe { *p.add(i) };
        match want.next() {
            Some(w) if w == c => i += 1,
            Some(_) => return false,
            None => return c == 0,
        }
    }
}

/// Ask the desktop window manager for a dark or light title bar.
fn set_dark_title(hwnd: Hwnd, dark: bool) {
    let value = dark as i32;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &value as *const i32 as *const c_void,
            4,
        );
    }
}

/// The window size (frame included) for an inner size in logical pixels, at
/// the window's DPI and with its current style.
fn outer_size(hwnd: Hwnd, size: Size) -> Point {
    let dpi = match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96,
        dpi => dpi,
    };
    let inner = ScaleFactor::new(dpi as f64 / 96.0).to_physical(size);
    let mut r = Rect {
        left: 0,
        top: 0,
        right: inner.width as i32,
        bottom: inner.height as i32,
    };
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    let menu = read_win(hwnd, |w| w.menu != 0).unwrap_or(false) as i32;
    unsafe { AdjustWindowRectExForDpi(&mut r, style, menu, 0, dpi) };
    Point {
        x: r.right - r.left,
        y: r.bottom - r.top,
    }
}

/// Re-apply the size limits by "resizing" to the current size: Windows clamps
/// the new size through `WM_GETMINMAXINFO`.
fn reapply_limits(hwnd: Hwnd) {
    let mut r = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        GetWindowRect(hwnd, &mut r);
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Make the window borderless and cover its monitor, or put back the style and
/// rectangle it had before.
fn set_fullscreen(hwnd: Hwnd, fullscreen: bool) {
    let saved = read_win(hwnd, |w| w.restore).flatten();
    match (fullscreen, saved) {
        (true, None) => {
            let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
            let mut r = Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let mut info = MonitorInfo {
                cb_size: core::mem::size_of::<MonitorInfo>() as u32,
                rc_monitor: Rect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
                rc_work: Rect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
                flags: 0,
            };
            let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
            if unsafe { GetWindowRect(hwnd, &mut r) } == 0
                || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0
            {
                return;
            }
            with_win(hwnd, |w| {
                w.restore = Some((style, [r.left, r.top, r.right, r.bottom]));
            });
            let m = info.rc_monitor;
            unsafe {
                SetWindowLongW(hwnd, GWL_STYLE, style & !(WS_OVERLAPPEDWINDOW as i32));
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(), // HWND_TOP
                    m.left,
                    m.top,
                    m.right - m.left,
                    m.bottom - m.top,
                    SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
                );
            }
        }
        (false, Some((style, [left, top, right, bottom]))) => {
            with_win(hwnd, |w| w.restore = None);
            unsafe {
                SetWindowLongW(hwnd, GWL_STYLE, style);
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    left,
                    top,
                    right - left,
                    bottom - top,
                    SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
                );
            }
        }
        _ => {}
    }
}

/// An `HICON` from a straight-alpha RGBA image: 32-bit BGRA color bits, rows
/// top first, plus a 1-bit AND mask (set where fully transparent) with rows
/// padded to 16 bits.
fn create_icon(icon: &crate::render::Pixmap) -> Handle {
    let icon = fit_icon(icon, 256);
    let size = icon.size();
    let (w, h) = (size.width as usize, size.height as usize);
    let (bgra, mask) = icon_bits(icon.as_bytes(), w, h);
    unsafe {
        CreateIcon(
            GetModuleHandleW(std::ptr::null()),
            w as i32,
            h as i32,
            1,
            32,
            mask.as_ptr(),
            bgra.as_ptr(),
        )
    }
}

/// The color (BGRA) and AND-mask bytes for [`create_icon`].
fn icon_bits(rgba: &[u8], w: usize, h: usize) -> (Vec<u8>, Vec<u8>) {
    let stride = w.div_ceil(16) * 2;
    let mut mask = vec![0u8; stride * h];
    let mut bgra = Vec::with_capacity(rgba.len());
    for (i, p) in rgba.as_chunks::<4>().0.iter().enumerate() {
        bgra.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        if p[3] == 0 {
            let (x, y) = (i % w, i / w);
            mask[y * stride + x / 8] |= 0x80 >> (x % 8);
        }
    }
    (bgra, mask)
}

unsafe extern "system" fn wnd_proc(hwnd: Hwnd, msg: u32, wp: Wparam, lp: Lparam) -> Lresult {
    // Events the message produces go to this window.
    with_current(hwnd, || unsafe { handle_message(hwnd, msg, wp, lp) })
}

/// Snap a `WM_SIZING` rectangle to the window's resize increments.
fn snap_sizing(hwnd: Hwnd, edge: u32, rect: &mut Rect) {
    let Some((step, min)) = read_win(hwnd, |w| w.increments.map(|s| (s, w.min_size))).flatten()
    else {
        return;
    };
    let scale = scale_of(hwnd);
    let px = |v: f64| (v * scale).round() as i32;
    let (outer, client) = window_rects(hwnd);
    let frame = (
        (outer.right - outer.left) - (client.right - client.left),
        (outer.bottom - outer.top) - (client.bottom - client.top),
    );
    let base = min.map_or((0, 0), |m| (px(m.width), px(m.height)));
    let [left, top, right, bottom] = geometry::snap_sizing(
        edge,
        [rect.left, rect.top, rect.right, rect.bottom],
        frame,
        base,
        (px(step.width), px(step.height)),
    );
    *rect = Rect {
        left,
        top,
        right,
        bottom,
    };
}

/// Clean up after a destroyed window: its menu, icon and state.
fn forget_window(hwnd: Hwnd) {
    let gone = CTX.with(|c| c.borrow_mut().wins.remove(&(hwnd as usize)));
    if let Some(w) = gone {
        unsafe {
            if w.menu != 0 {
                DestroyMenu(w.menu as Handle);
            }
            // The notifications may still borrow the icon.
            if w.icon != 0 && !tray::is_app_icon(w.icon) {
                DestroyIcon(w.icon as Handle);
            }
        }
    }
}

unsafe fn handle_message(hwnd: Hwnd, msg: u32, wp: Wparam, lp: Lparam) -> Lresult {
    if tray::taskbar_created(hwnd, msg) {
        return 0;
    }
    match msg {
        WM_SETCURSOR if (lp & 0xffff) as u16 == HTCLIENT => {
            let cursor = CTX.with(|c| c.borrow().cursor);
            unsafe { SetCursor(load_cursor(cursor)) };
            1
        }
        WM_GETMINMAXINFO => {
            let (min, max, fullscreen) =
                with_win(hwnd, |w| (w.min_size, w.max_size, w.restore.is_some()));
            let info = unsafe { &mut *(lp as *mut MinMaxInfo) };
            if let Some(min) = min {
                info.min_track_size = outer_size(hwnd, min);
            }
            // A fullscreen window covers the monitor whatever its maximum.
            if let Some(max) = max.filter(|_| !fullscreen) {
                info.max_track_size = outer_size(hwnd, max);
                info.max_size = outer_size(hwnd, max);
            }
            0
        }
        WM_PAINT => {
            let mut ps: PaintStruct = unsafe { std::mem::zeroed() };
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            CTX.with(|c| {
                let c = c.borrow();
                let Some(c) = c.wins.get(&(hwnd as usize)) else {
                    return;
                };
                if !c.fb.is_empty() {
                    let bmi = BitmapInfo {
                        header: BitmapInfoHeader {
                            bi_size: core::mem::size_of::<BitmapInfoHeader>() as u32,
                            bi_width: c.fb_w,
                            // Negative height => top-down DIB (row 0 is the top).
                            bi_height: -c.fb_h,
                            bi_planes: 1,
                            bi_bit_count: 32,
                            bi_compression: BI_RGB,
                            bi_size_image: 0,
                            bi_x_pels_per_meter: 0,
                            bi_y_pels_per_meter: 0,
                            bi_clr_used: 0,
                            bi_clr_important: 0,
                        },
                        colors: [0],
                    };
                    unsafe {
                        StretchDIBits(
                            hdc,
                            0,
                            0,
                            c.fb_w,
                            c.fb_h,
                            0,
                            0,
                            c.fb_w,
                            c.fb_h,
                            c.fb.as_ptr() as *const c_void,
                            &bmi,
                            DIB_RGB_COLORS,
                            SRCCOPY,
                        );
                    }
                }
            });
            unsafe { EndPaint(hwnd, &ps) };
            0
        }
        WM_SIZE => {
            // lParam LOWORD/HIWORD = new client width/height.
            let w = (lp & 0xffff) as u32;
            let h = ((lp >> 16) & 0xffff) as u32;
            if w > 0 && h > 0 {
                let size = PhysicalSize::new(w, h);
                with_win(hwnd, |win| win.size = size);
                push_event(Event::Resized(size));
            }
            refresh_info(hwnd);
            0
        }
        WM_MOVE => {
            refresh_info(hwnd);
            0
        }
        WM_SIZING => {
            snap_sizing(hwnd, wp as u32, unsafe { &mut *(lp as *mut Rect) });
            1
        }
        WM_DPICHANGED => {
            // wParam's low word is the new DPI; lParam is the suggested window
            // rectangle. Moving there sends the WM_SIZE that re-renders.
            let dpi = (wp & 0xffff) as u32;
            with_win(hwnd, |w| w.dpi = dpi);
            let r = unsafe { &*(lp as *const Rect) };
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            0
        }
        WM_MOUSEMOVE => {
            // Ask to hear when the pointer leaves the window.
            pointer::track_leave(hwnd);
            crate::platform::set_pointer_modifiers(modifiers());
            push_event(Event::PointerMoved {
                position: mouse_xy(lp),
            });
            0
        }
        pointer::WM_MOUSELEAVE => {
            pointer::left();
            push_event(Event::PointerLeft);
            0
        }
        WM_LBUTTONDOWN
        | WM_RBUTTONDOWN
        | WM_MBUTTONDOWN
        | pointer::WM_XBUTTONDOWN
        | WM_LBUTTONUP
        | WM_RBUTTONUP
        | WM_MBUTTONUP
        | pointer::WM_XBUTTONUP => {
            let button = match msg {
                WM_LBUTTONDOWN | WM_LBUTTONUP => PointerButton::Left,
                WM_RBUTTONDOWN | WM_RBUTTONUP => PointerButton::Right,
                WM_MBUTTONDOWN | WM_MBUTTONUP => PointerButton::Middle,
                _ => input::win32_xbutton(wp),
            };
            let state = match msg {
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | pointer::WM_XBUTTONDOWN => {
                    ButtonState::Pressed
                }
                _ => ButtonState::Released,
            };
            crate::platform::set_pointer_modifiers(modifiers());
            // Keep the moves coming while any button is down, even outside
            // the window: drags (and drags out of the app) need them, and the
            // release.
            if state == ButtonState::Pressed {
                unsafe { SetCapture(hwnd) };
            } else if !input::win32_any_button_down(wp) {
                unsafe { ReleaseCapture() };
            }
            push_event(Event::PointerButton {
                button,
                state,
                position: mouse_xy(lp),
            });
            // The side buttons' messages want TRUE back.
            isize::from(matches!(
                msg,
                pointer::WM_XBUTTONDOWN | pointer::WM_XBUTTONUP
            ))
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            // The pointer position (in screen coordinates here) isn't needed:
            // the app scrolls whatever is under the last `WM_MOUSEMOVE`.
            crate::platform::set_pointer_modifiers(modifiers());
            push_event(Event::Scroll {
                delta: input::win32_wheel(wp, msg == WM_MOUSEHWHEEL),
            });
            0
        }
        WM_KEYDOWN | WM_KEYUP => {
            let state = if msg == WM_KEYDOWN {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            };
            push_raw_key(wp, lp, state);
            if let Some(ev) = key_event(wp, state) {
                push_event(ev);
            }
            0
        }
        WM_SYSKEYDOWN | WM_SYSKEYUP => {
            let state = if msg == WM_SYSKEYDOWN {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            };
            push_raw_key(wp, lp, state);
            if let Some(ev) = key_event(wp, state) {
                push_event(ev);
            }
            // Let Windows handle Alt+F4, Alt+Space and friends as well.
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_CHAR => {
            // wParam is a UTF-16 code unit (a character beyond the BMP comes
            // as two); emit printable characters as text, unless the key went
            // out as a shortcut from WM_KEYDOWN. A dead key followed by a
            // letter arrives here already composed, from TranslateMessage.
            let m = modifiers();
            let shortcut = (m.ctrl && !m.alt) || m.meta;
            let ch = CTX.with(|c| c.borrow_mut().utf16.push(wp as u16));
            // The key that typed it was consumed as a raw key.
            if CTX.with(|c| c.borrow().drop_chars) {
                return 0;
            }
            if let Some(ch) = ch
                && !ch.is_control()
                && !shortcut
            {
                push_event(Event::Text(ch.to_string()));
            }
            0
        }
        // Input methods: we draw the composition inline, so the IME's own
        // composition window stays hidden (see `ime`).
        ime::WM_IME_SETCONTEXT => unsafe {
            DefWindowProcW(hwnd, msg, wp, lp & !ime::ISC_SHOWUICOMPOSITIONWINDOW)
        },
        ime::WM_IME_STARTCOMPOSITION => {
            ime::place_windows(hwnd);
            0
        }
        ime::WM_IME_COMPOSITION => {
            ime::composition(hwnd, lp);
            0
        }
        ime::WM_IME_ENDCOMPOSITION => {
            ime::end_composition();
            0
        }
        WM_GETOBJECT => {
            // UIA asks for our root provider with lParam == UiaRootObjectId. Hand
            // back the live provider tree; other object ids fall through to the
            // default (e.g. legacy MSAA, which we don't serve).
            if lp as i32 == UIA_ROOT_OBJECT_ID {
                let root = read_win(hwnd, |w| w.uia.as_ref().map(|t| t.root_provider()))
                    .flatten()
                    .unwrap_or(std::ptr::null_mut());
                if !root.is_null() {
                    return unsafe { UiaReturnRawElementProvider(hwnd, wp, lp, root) };
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        // Dark mode and the accent color: `ImmersiveColorSet` names the setting
        // that changed; the colorization message follows accent changes. The
        // animation setting (reduced motion) comes with its SPI code.
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_DWMCOLORIZATIONCOLORCHANGED => {
            // SPI_SETCLIENTAREAANIMATION: the animation effects setting.
            if msg != WM_SETTINGCHANGE
                || wp == 0x1043
                || unsafe { names_setting(lp, "ImmersiveColorSet") }
            {
                crate::platform::native::note_changed();
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        // A pick from the menu bar. (Accelerator tables aren't used: the App
        // matches shortcuts from the key events, as on every platform.)
        WM_COMMAND if (wp >> 16) & 0xffff == 0 && lp == 0 => {
            push_event(Event::MenuCommand((wp & 0xffff) as u32));
            0
        }
        tray::WM_APP_TRAY => {
            tray::callback(hwnd, wp, lp);
            0
        }
        dnd::WM_DROPFILES => {
            dnd::drop_files(wp as *mut c_void);
            0
        }
        WM_CLOSE => {
            push_event(Event::CloseRequested);
            0
        }
        attention::WM_SETFOCUS | attention::WM_KILLFOCUS => {
            attention::focus_message(hwnd, msg);
            0
        }
        attention::WM_QUERYENDSESSION => attention::query_end_session(hwnd),
        WM_DESTROY => {
            tsf::forget(hwnd);
            // The loop ends when the last window is gone; if others stay,
            // the tray icon moves to one of them.
            if tray::owner() == Some(hwnd) {
                tray::remove_all(hwnd);
            }
            0
        }
        WM_NCDESTROY => {
            forget_window(hwnd);
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[derive(Debug)]
struct WinWindow {
    hwnd: Hwnd,
}

/// Give `hwnd` the icon `icon` (or none), replacing the one it had.
fn set_window_icon(hwnd: Hwnd, icon: Option<&crate::render::Pixmap>) {
    let new = icon.map_or(std::ptr::null_mut(), create_icon);
    unsafe {
        SendMessageW(hwnd, WM_SETICON, ICON_BIG, new as Lparam);
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL, new as Lparam);
    }
    tray::set_app_icon(new);
    let old = with_win(hwnd, |w| std::mem::replace(&mut w.icon, new as usize));
    if old != 0 {
        unsafe { DestroyIcon(old as Handle) };
    }
}

impl Window for WinWindow {
    fn id(&self) -> WindowId {
        WindowId(self.hwnd as u64)
    }
    fn inner_size(&self) -> PhysicalSize {
        read_win(self.hwnd, |w| w.size).unwrap_or_default()
    }
    fn scale_factor(&self) -> ScaleFactor {
        ScaleFactor::new(scale_of(self.hwnd))
    }
    fn request_redraw(&self) {
        unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
    }
    fn set_title(&self, title: &str) {
        let text = wide(title);
        unsafe { SetWindowTextW(self.hwnd, text.as_ptr()) };
        debug_window(format_args!(
            "title {:#x} {:?}",
            self.hwnd as u64,
            window_text(self.hwnd)
        ));
    }
    fn open_window(&self, attrs: WindowAttributes) -> Option<WindowId> {
        let hwnd = create_window(&attrs).ok()?;
        show_window(hwnd, &attrs, attrs.focus);
        // The loop adopts it once the handler returns, then draws it.
        CTX.with(|c| c.borrow_mut().opened.push(hwnd as usize));
        push_event_to(hwnd, Event::RedrawRequested);
        Some(WindowId(hwnd as u64))
    }
    fn close_window(&self) {
        CTX.with(|c| c.borrow_mut().closed.push(self.hwnd as usize));
    }
    fn set_visible(&self, visible: bool) {
        let cmd = if visible { SW_SHOW } else { SW_HIDE };
        unsafe { ShowWindow(self.hwnd, cmd) };
    }
    fn info(&self) -> WindowInfo {
        match read_win(self.hwnd, |w| w.info).flatten() {
            Some(info) => info,
            None => read_info(self.hwnd, None),
        }
    }
    fn set_position(&self, position: crate::geometry::Point) {
        let scale = scale_of(self.hwnd);
        unsafe {
            SetWindowPos(
                self.hwnd,
                std::ptr::null_mut(),
                (position.x * scale).round() as i32,
                (position.y * scale).round() as i32,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        debug_frame(self.hwnd);
    }
    fn set_inner_size(&self, size: Size) {
        let outer = outer_size(self.hwnd, size);
        unsafe {
            SetWindowPos(
                self.hwnd,
                std::ptr::null_mut(),
                0,
                0,
                outer.x,
                outer.y,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        debug_frame(self.hwnd);
    }
    fn set_resize_increments(&self, step: Option<Size>) {
        let step = step.filter(|s| s.width > 0.0 && s.height > 0.0);
        with_win(self.hwnd, |w| w.increments = step);
        match step {
            Some(s) => debug_window(format_args!(
                "increments {:#x} {}x{}",
                self.hwnd as u64,
                s.width.round() as i64,
                s.height.round() as i64
            )),
            None => debug_window(format_args!("increments {:#x} none", self.hwnd as u64)),
        }
    }
    fn set_minimized(&self, minimized: bool) {
        let cmd = if minimized { SW_MINIMIZE } else { SW_RESTORE };
        unsafe { ShowWindow(self.hwnd, cmd) };
    }
    fn clipboard(&self) -> Option<String> {
        crate::platform::clipboard::text()
    }
    fn set_clipboard(&self, text: &str) {
        crate::platform::clipboard::set_text(text);
    }
    fn read_clipboard(
        &self,
        kind: crate::platform::clipboard::ClipboardKind,
        formats: crate::platform::clipboard::ClipboardFormats,
    ) -> crate::platform::clipboard::ClipboardRead {
        // Text, HTML and images; no primary selection here.
        use crate::platform::clipboard::{ClipboardKind, ClipboardRead, read};
        ClipboardRead::Ready(
            (kind == ClipboardKind::Clipboard)
                .then(|| read(formats))
                .flatten(),
        )
    }
    fn write_clipboard(
        &self,
        kind: crate::platform::clipboard::ClipboardKind,
        content: &crate::platform::clipboard::ClipboardContent,
    ) {
        if kind == crate::platform::clipboard::ClipboardKind::Clipboard {
            crate::platform::clipboard::write(content);
        }
    }
    fn set_dark(&self, dark: bool) {
        set_dark_title(self.hwnd, dark);
        // Have the frame repaint now rather than at the next activation.
        unsafe {
            SetWindowPos(
                self.hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
        };
    }
    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(WinSurface {
            hwnd: self.hwnd,
            size: self.inner_size(),
        })
    }
    fn set_accessibility_tree(&self, root: &crate::platform::access::A11yNode) {
        // Rebuild the UIA provider tree; `WM_GETOBJECT` hands its root to UIA.
        let tree = crate::platform::uia::UiaTree::build(root);
        with_win(self.hwnd, |w| w.uia = Some(tree));
    }
    fn set_ime(&self, state: Option<&crate::platform::ImeState>) {
        with_current(self.hwnd, || {
            if !tsf::set_state(self.hwnd, state) {
                ime::set_state(self.hwnd, state);
            }
        });
    }
    fn set_drop_accepted(&self, accepted: bool) {
        CTX.with(|c| c.borrow_mut().drop_accepted = accepted);
    }
    fn start_drag(&self, data: &crate::platform::DropData) -> bool {
        // `DoDragDrop` runs its own modal loop, so the message loop starts it
        // once the handler returns.
        if !data.can_leave_app() {
            return false;
        }
        CTX.with(|c| c.borrow_mut().drag_out = Some((self.hwnd as usize, data.clone())));
        true
    }
    fn set_cursor(&self, cursor: CursorIcon) {
        CTX.with(|c| c.borrow_mut().cursor = cursor);
        // The pointer is over the client area (we got here from a mouse move),
        // so show it now rather than on the next `WM_SETCURSOR`.
        unsafe { SetCursor(load_cursor(cursor)) };
    }
    fn set_min_size(&self, size: Option<Size>) {
        with_win(self.hwnd, |w| w.min_size = size);
        reapply_limits(self.hwnd);
    }
    fn set_max_size(&self, size: Option<Size>) {
        with_win(self.hwnd, |w| w.max_size = size);
        reapply_limits(self.hwnd);
    }
    fn set_maximized(&self, maximized: bool) {
        let cmd = if maximized { SW_MAXIMIZE } else { SW_RESTORE };
        unsafe { ShowWindow(self.hwnd, cmd) };
    }
    fn set_fullscreen(&self, fullscreen: bool) {
        set_fullscreen(self.hwnd, fullscreen);
    }
    fn set_menu_bar(&self, menu: &NativeMenuBar) -> bool {
        set_menu_bar(self.hwnd, menu);
        true
    }
    fn popup_menu(&self, menu: &NativeMenu, at: crate::geometry::Point) -> MenuPopup {
        with_current(self.hwnd, || popup_menu(self.hwnd, menu, at))
    }
    fn set_tray(&self, tray: Option<&NativeTray>) -> bool {
        tray::set_tray(self.hwnd, tray)
    }
    fn request_attention(&self, kind: Option<crate::platform::UserAttention>) {
        attention::flash(self.hwnd, kind);
    }
    fn beep(&self) {
        attention::beep();
    }
    fn set_icon(&self, icon: Option<&crate::render::Pixmap>) {
        set_window_icon(self.hwnd, icon);
    }
}

#[derive(Debug)]
struct WinSurface {
    hwnd: Hwnd,
    size: PhysicalSize,
}

impl Surface for WinSurface {
    fn resize(&mut self, size: PhysicalSize) {
        self.size = size;
    }
    fn size(&self) -> PhysicalSize {
        self.size
    }
    fn present(&mut self, pixmap: &Pixmap, _damage: &[crate::geometry::Rect]) {
        let size = pixmap.size();
        let src = pixmap.as_bytes();
        // RGBA -> BGRA (Win32 DIBs are BGRA, top-down via negative height).
        let mut fb = vec![0u8; src.len()];
        for (d, s) in fb
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(src.as_chunks::<4>().0)
        {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
        with_win(self.hwnd, |w| {
            w.fb = fb;
            w.fb_w = size.width as i32;
            w.fb_h = size.height as i32;
        });
        unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
    }
}

const CLASS_NAME: &str = "StippleWindowClass";

/// Register the window class every Stipple window uses.
fn register_class() -> Result<(), PlatformError> {
    let class_name = wide(CLASS_NAME);
    let class = WndClassW {
        style: 0,
        lpfn_wnd_proc: Some(wnd_proc),
        cb_cls_extra: 0,
        cb_wnd_extra: 0,
        h_instance: unsafe { GetModuleHandleW(std::ptr::null()) },
        h_icon: std::ptr::null_mut(),
        h_cursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
        hbr_background: std::ptr::null_mut(),
        lpsz_menu_name: std::ptr::null(),
        lpsz_class_name: class_name.as_ptr(),
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(PlatformError::Os("RegisterClassW failed".into()));
    }
    Ok(())
}

/// Create (but don't show) a top-level window for `attrs`: its title, inner
/// size, position, size limits, resize increments, title-bar color and icon.
fn create_window(attrs: &WindowAttributes) -> Result<Hwnd, PlatformError> {
    let class_name = wide(CLASS_NAME);
    let title = wide(&attrs.title);
    let system_dpi = unsafe { GetDpiForSystem() };
    let scale = ScaleFactor::new(system_dpi as f64 / 96.0);
    let size = scale.to_physical(attrs.logical_size);
    let mut style = if attrs.decorations {
        WS_OVERLAPPEDWINDOW
    } else {
        WS_POPUP_RESIZABLE
    };
    let (mut min, mut max) = (attrs.min_size, attrs.max_size);
    if !attrs.resizable {
        // No resizing border or maximize button, and the size as both limits.
        style &= !(WS_THICKFRAME | WS_MAXIMIZEBOX);
        (min, max) = (Some(attrs.logical_size), Some(attrs.logical_size));
    }
    let (x, y) = match attrs.position {
        Some((x, y)) => (
            (x as f64 * scale.get()).round() as i32,
            (y as f64 * scale.get()).round() as i32,
        ),
        None => (CW_USEDEFAULT, CW_USEDEFAULT),
    };
    // Size limits, read on `WM_GETMINMAXINFO` (first sent while creating).
    let increments = attrs.resize_increments;
    CTX.with(|c| c.borrow_mut().pending = (min, max, increments, system_dpi));
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            style,
            x,
            y,
            size.width as i32,
            size.height as i32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null_mut(),
        )
    };
    CTX.with(|c| c.borrow_mut().pending = (None, None, None, 0));
    if hwnd.is_null() {
        return Err(PlatformError::Os("CreateWindowExW failed".into()));
    }
    // The window may have opened on a display with a different DPI to the
    // system's.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    with_win(hwnd, |w| {
        w.dpi = dpi;
        w.min_size = min;
        w.max_size = max;
        w.increments = increments;
    });
    if let Some(dark) = attrs.dark {
        // A title bar matching the content.
        set_dark_title(hwnd, dark);
    }
    // `CreateWindowExW` sizes the whole window: make the client area the
    // size asked for.
    let outer = outer_size(hwnd, attrs.logical_size);
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            outer.x,
            outer.y,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    let (_, client) = window_rects(hwnd);
    let client = PhysicalSize::new(
        (client.right - client.left).max(1) as u32,
        (client.bottom - client.top).max(1) as u32,
    );
    with_win(hwnd, |w| w.size = client);
    // Take drops from other applications.
    if !dnd::register(hwnd) {
        eprintln!("stipple: RegisterDragDrop failed; only WM_DROPFILES drops work");
    }
    if let Some(icon) = attrs.icon.as_ref() {
        set_window_icon(hwnd, Some(icon));
    }
    debug_window(format_args!(
        "opened {:#x} {:?}",
        hwnd as u64,
        window_text(hwnd)
    ));
    Ok(hwnd)
}

/// Show a window made by [`create_window`], maximized or fullscreen if
/// `attrs` say so, and activate it if `focus`.
fn show_window(hwnd: Hwnd, attrs: &WindowAttributes, focus: bool) {
    let show = if attrs.maximized {
        SW_MAXIMIZE
    } else if focus {
        SW_SHOW
    } else {
        SW_SHOWNOACTIVATE
    };
    unsafe {
        ShowWindow(hwnd, show);
        UpdateWindow(hwnd);
    }
    if attrs.fullscreen {
        set_fullscreen(hwnd, true);
    }
    if focus {
        unsafe { SetForegroundWindow(hwnd) };
    }
    refresh_info(hwnd);
}

/// Adopt the windows the handler opened and destroy the ones it closed.
fn settle(windows: &mut Vec<WinWindow>) {
    let (opened, closed) = CTX.with(|c| {
        let mut c = c.borrow_mut();
        (std::mem::take(&mut c.opened), std::mem::take(&mut c.closed))
    });
    for hwnd in opened {
        if !windows.iter().any(|w| w.hwnd as usize == hwnd) {
            windows.push(WinWindow { hwnd: hwnd as Hwnd });
        }
    }
    for hwnd in closed {
        if let Some(pos) = windows.iter().position(|w| w.hwnd as usize == hwnd) {
            windows.remove(pos);
            unsafe { DestroyWindow(hwnd as Hwnd) };
            debug_window(format_args!("closed {:#x}", hwnd as u64));
        }
    }
    if let Some(first) = windows.first() {
        // Wakes and the tray icon need a window that is still open.
        let target = WAKE_TARGET.load(Ordering::Acquire);
        if !windows.iter().any(|w| w.hwnd as usize == target) {
            WAKE_TARGET.store(first.hwnd as usize, Ordering::Release);
        }
        if tray::owner().is_none() {
            tray::rehome(first.hwnd);
        }
    }
}

/// Deliver the queued events, each to its window, until the queue is empty
/// (handling them may queue more). Returns `Exit` if the handler did.
fn deliver_queued<H>(windows: &mut Vec<WinWindow>, handler: &mut H) -> ControlFlow
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    // Bounded, in case handling events keeps making more; what's left waits
    // for the next message, which we post.
    for _ in 0..8 {
        settle(windows);
        let events: Vec<(usize, Event)> = CTX.with(|c| std::mem::take(&mut c.borrow_mut().queue));
        if events.is_empty() {
            return ControlFlow::Wait;
        }
        // After a raw key the handler consumed, the events the same key
        // produced are skipped.
        let mut skip = false;
        for (hwnd, event) in events {
            settle(windows);
            // Events from outside any window's messages go to the first.
            let target = match windows.iter().find(|w| w.hwnd as usize == hwnd) {
                Some(w) => Some(w),
                None if hwnd == 0 => windows.first(),
                None => None, // closed meanwhile
            };
            let Some(w) = target else {
                continue;
            };
            let flow = match event {
                Event::RawKey(key) => {
                    let pressed = key.is_press();
                    let (used, flow) = crate::platform::key::deliver_raw(key, |e| handler(e, w));
                    skip = used;
                    if pressed && used {
                        CTX.with(|c| c.borrow_mut().drop_chars = true);
                    }
                    flow
                }
                Event::Key { .. } | Event::Text(_) if skip => ControlFlow::Wait,
                event => {
                    skip = false;
                    handler(event, w)
                }
            };
            if flow == ControlFlow::Exit {
                return ControlFlow::Exit;
            }
        }
    }
    settle(windows);
    let pending = CTX.with(|c| !c.borrow().queue.is_empty());
    if pending && let Some(w) = windows.first() {
        unsafe { PostMessageW(w.hwnd, WM_NULL, 0, 0) };
    }
    ControlFlow::Wait
}

/// Create a window, render the initial frame, and pump the Win32 message loop
/// until the last window closes or the handler returns `Exit`. More windows
/// open with [`Window::open_window`]; each one's events reach `handler` with
/// that window, and [`Event::Wake`] goes to every window in turn.
pub fn run<H>(attrs: WindowAttributes, mut handler: H) -> Result<(), PlatformError>
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    // Without this, Windows renders us at 96 DPI and stretches the blurry result.
    // It fails harmlessly if the app's manifest already set an awareness.
    unsafe { SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2) };
    pointer::read_double_click(unsafe { GetDpiForSystem() });
    register_class()?;
    let hwnd = create_window(&attrs)?;
    tray::install(hwnd);
    let mut windows = vec![WinWindow { hwnd }];

    // Render the initial frame into the window's framebuffer.
    handler(Event::RedrawRequested, &windows[0]);
    show_window(hwnd, &attrs, attrs.focus);

    // Message loop. The WndProc translates native messages into Stipple events
    // and queues them for their window (it can't see the generic `handler`);
    // we drain and dispatch the queue after each message.
    //
    // Between messages, wait for input or a wakeup (timers, other threads). A
    // wake from another thread posts a message to an open window, which ends
    // the wait.
    WAKE_TARGET.store(hwnd as usize, Ordering::Release);
    crate::platform::wake::install(
        std::sync::Arc::new(|| unsafe {
            PostMessageW(
                WAKE_TARGET.load(Ordering::Acquire) as Hwnd,
                WM_APP_WAKE,
                0,
                0,
            );
        }),
        None,
    );
    // Animation frames are paced by DWM's composition, on a thread of their own.
    vsync::install();
    let mut msg: Msg = unsafe { std::mem::zeroed() };
    'outer: while !windows.is_empty() {
        let timeout = match crate::platform::wake::timeout(std::time::Instant::now()) {
            // Round up, so we don't wake a hair early and spin.
            Some(t) => t.as_micros().div_ceil(1000).min(INFINITE as u128 - 1) as u32,
            None => INFINITE,
        };
        unsafe {
            MsgWaitForMultipleObjectsEx(
                0,
                std::ptr::null(),
                timeout,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if msg.message == WM_QUIT {
                break 'outer;
            }
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if deliver_queued(&mut windows, &mut handler) == ControlFlow::Exit {
                break 'outer;
            }
            // A drag the app handed out runs here, outside the handler. What
            // it did to our own window waits in the queue; a message gets
            // the loop to deliver it.
            if let Some((owner, data)) = CTX.with(|c| c.borrow_mut().drag_out.take()) {
                unsafe { ReleaseCapture() };
                let _dropped = dnd::drag_out::run(&data);
                unsafe { PostMessageW(owner as Hwnd, WM_NULL, 0, 0) };
            }
            if windows.is_empty() {
                break 'outer;
            }
        }
        if crate::platform::wake::take(std::time::Instant::now()) {
            // Every window hears the wake, so each can catch up.
            let ids: Vec<usize> = windows.iter().map(|w| w.hwnd as usize).collect();
            for id in ids {
                settle(&mut windows);
                if let Some(w) = windows.iter().find(|w| w.hwnd as usize == id)
                    && handler(Event::Wake, w) == ControlFlow::Exit
                {
                    break 'outer;
                }
            }
            if deliver_queued(&mut windows, &mut handler) == ControlFlow::Exit {
                break 'outer;
            }
        }
    }
    // The loop ended: close whatever is still open.
    for w in windows.drain(..) {
        unsafe { DestroyWindow(w.hwnd) };
        debug_window(format_args!("closed {:#x}", w.hwnd as u64));
    }
    Ok(())
}
