//! A native Windows backend over raw Win32 FFI (`user32`/`gdi32`/`kernel32`).
//!
//! No `windows`/`winapi` crate — direct `extern "system"` declarations, so the
//! only "dependency" is the OS itself (the workspace policy in `ROADMAP.md`
//! §1). This module is the one place `unsafe` is permitted in the crate.
//!
//! Scope (v1): register a window class, create + show a top-level window, and
//! present the software [`Pixmap`] by blitting a top-down 32-bit DIB with
//! `StretchDIBits` on `WM_PAINT`. Input (mouse/keyboard) and live resize are
//! follow-ups; the first milestone is a CI-screenshotable window.
//!
//! **Verification:** build-checked on the `windows-latest` CI runner via the
//! workspace build matrix; runtime-screenshotted by the Visual workflow's
//! Windows job.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::c_void;

use crate::geometry::{PhysicalSize, ScaleFactor};
use crate::platform::ControlFlow;
use crate::platform::error::PlatformError;
use crate::platform::event::{ButtonState, Event, KeyCode, PointerButton, WindowId};
use crate::platform::window::{Window, WindowAttributes};
use crate::render::{Pixmap, Surface};

// ---- Win32 type aliases -----------------------------------------------------

type Handle = *mut c_void;
type Hwnd = Handle;
type Wparam = usize;
type Lparam = isize;
type Lresult = isize;
type WndProc = unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult;

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const CW_USEDEFAULT: i32 = -2147483648; // 0x80000000
const SW_SHOW: i32 = 5;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_PAINT: u32 = 0x000F;
const WM_CLOSE: u32 = 0x0010;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_GETOBJECT: u32 = 0x003D;
const WM_DPICHANGED: u32 = 0x02E0;
const WM_QUIT: u32 = 0x0012;
/// Posted by [`crate::platform::wake`] to end the message wait.
const WM_APP_WAKE: u32 = 0x8000 + 1;
const PM_REMOVE: u32 = 0x0001;
const QS_ALLINPUT: u32 = 0x04FF;
/// Also return for input already in the queue but not yet looked at.
const MWMO_INPUTAVAILABLE: u32 = 0x0004;
const INFINITE: u32 = u32::MAX;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
/// `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`.
const PER_MONITOR_AWARE_V2: isize = -4;
/// `lParam` value identifying UIA's request for our root fragment provider.
const UIA_ROOT_OBJECT_ID: i32 = -25;

const VK_BACK: usize = 0x08;
const VK_TAB: usize = 0x09;
const VK_RETURN: usize = 0x0D;
const VK_ESCAPE: usize = 0x1B;
const VK_LEFT: usize = 0x25;
const VK_UP: usize = 0x26;
const VK_RIGHT: usize = 0x27;
const VK_DOWN: usize = 0x28;
const BI_RGB: u32 = 0;
const DIB_RGB_COLORS: u32 = 0;
const SRCCOPY: u32 = 0x00CC_0020;
const IDC_ARROW: usize = 32512;

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
    fn PostQuitMessage(code: i32);
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: i32) -> i32;
    fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
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

/// Per-thread window state shared between the message loop, the [`WndProc`]
/// (which blits on `WM_PAINT`), and the [`Surface`] (which updates the buffer).
#[derive(Default)]
struct WinCtx {
    /// Top-down BGRA framebuffer for the current frame.
    fb: Vec<u8>,
    fb_w: i32,
    fb_h: i32,
    /// Events pushed by the [`WndProc`], drained by the message loop.
    queue: Vec<Event>,
    /// The current UIA provider tree, vended from `WM_GETOBJECT`. Rebuilt each
    /// time the App pushes a new accessibility tree.
    uia: Option<crate::platform::uia::UiaTree>,
    /// The window's DPI. 0 until the window exists, which reads as 96 (1x).
    dpi: u32,
}

/// Physical pixels per logical pixel for the window's current display.
fn current_scale() -> f64 {
    CTX.with(|c| match c.borrow().dpi {
        0 => 1.0,
        dpi => dpi as f64 / 96.0,
    })
}

fn push_event(ev: Event) {
    CTX.with(|c| c.borrow_mut().queue.push(ev));
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

fn map_vk(vk: usize) -> Option<KeyCode> {
    Some(match vk {
        VK_BACK => KeyCode::Backspace,
        VK_TAB => KeyCode::Tab,
        VK_RETURN => KeyCode::Enter,
        VK_ESCAPE => KeyCode::Escape,
        VK_LEFT => KeyCode::ArrowLeft,
        VK_RIGHT => KeyCode::ArrowRight,
        VK_UP => KeyCode::ArrowUp,
        VK_DOWN => KeyCode::ArrowDown,
        _ => return None,
    })
}

thread_local! {
    static CTX: RefCell<WinCtx> = RefCell::new(WinCtx::default());
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wnd_proc(hwnd: Hwnd, msg: u32, wp: Wparam, lp: Lparam) -> Lresult {
    match msg {
        WM_PAINT => {
            let mut ps: PaintStruct = unsafe { std::mem::zeroed() };
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            CTX.with(|c| {
                let c = c.borrow();
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
                push_event(Event::Resized(PhysicalSize::new(w, h)));
            }
            0
        }
        WM_DPICHANGED => {
            // wParam's low word is the new DPI; lParam is the suggested window
            // rectangle. Moving there sends the WM_SIZE that re-renders.
            let dpi = (wp & 0xffff) as u32;
            CTX.with(|c| c.borrow_mut().dpi = dpi);
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
            push_event(Event::PointerMoved {
                position: mouse_xy(lp),
            });
            0
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP
        | WM_MBUTTONUP => {
            let button = match msg {
                WM_LBUTTONDOWN | WM_LBUTTONUP => PointerButton::Left,
                WM_RBUTTONDOWN | WM_RBUTTONUP => PointerButton::Right,
                _ => PointerButton::Middle,
            };
            let state = match msg {
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => ButtonState::Pressed,
                _ => ButtonState::Released,
            };
            push_event(Event::PointerButton {
                button,
                state,
                position: mouse_xy(lp),
            });
            0
        }
        WM_KEYDOWN => {
            if let Some(code) = map_vk(wp) {
                push_event(Event::Key {
                    code,
                    state: ButtonState::Pressed,
                    modifiers: Default::default(),
                });
            }
            0
        }
        WM_CHAR => {
            // wParam is a UTF-16 code unit; emit printable characters as text.
            if wp >= 0x20
                && let Some(ch) = char::from_u32(wp as u32)
            {
                push_event(Event::Text(ch.to_string()));
            }
            0
        }
        WM_GETOBJECT => {
            // UIA asks for our root provider with lParam == UiaRootObjectId. Hand
            // back the live provider tree; other object ids fall through to the
            // default (e.g. legacy MSAA, which we don't serve).
            if lp as i32 == UIA_ROOT_OBJECT_ID {
                let root = CTX.with(|c| {
                    c.borrow()
                        .uia
                        .as_ref()
                        .map(|t| t.root_provider())
                        .unwrap_or(std::ptr::null_mut())
                });
                if !root.is_null() {
                    return unsafe { UiaReturnRawElementProvider(hwnd, wp, lp, root) };
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_CLOSE => {
            push_event(Event::CloseRequested);
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[derive(Debug)]
struct WinWindow {
    hwnd: Hwnd,
    size: PhysicalSize,
}

impl Window for WinWindow {
    fn id(&self) -> WindowId {
        WindowId(self.hwnd as u64)
    }
    fn inner_size(&self) -> PhysicalSize {
        self.size
    }
    fn scale_factor(&self) -> ScaleFactor {
        ScaleFactor::new(current_scale())
    }
    fn request_redraw(&self) {
        unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
    }
    fn set_title(&self, _title: &str) {}
    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(WinSurface {
            hwnd: self.hwnd,
            size: self.size,
        })
    }
    fn set_accessibility_tree(&self, root: &crate::platform::access::A11yNode) {
        // Rebuild the UIA provider tree; `WM_GETOBJECT` hands its root to UIA.
        let tree = crate::platform::uia::UiaTree::build(root);
        CTX.with(|c| c.borrow_mut().uia = Some(tree));
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
        CTX.with(|c| {
            let mut c = c.borrow_mut();
            c.fb = fb;
            c.fb_w = size.width as i32;
            c.fb_h = size.height as i32;
        });
        unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
    }
}

/// Create a window, render the initial frame, and pump the Win32 message loop
/// until `WM_QUIT`. `handler` is invoked for [`Event::RedrawRequested`] (to
/// produce the frame) and [`Event::CloseRequested`].
pub fn run<H>(attrs: WindowAttributes, mut handler: H) -> Result<(), PlatformError>
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    let class_name = wide("StippleWindowClass");
    let title = wide(&attrs.title);
    // Without this, Windows renders us at 96 DPI and stretches the blurry result.
    // It fails harmlessly if the app's manifest already set an awareness.
    unsafe { SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2) };
    let system_dpi = unsafe { GetDpiForSystem() };
    let size = ScaleFactor::new(system_dpi as f64 / 96.0).to_physical(attrs.logical_size);

    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let cursor = unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) };

    let class = WndClassW {
        style: 0,
        lpfn_wnd_proc: Some(wnd_proc),
        cb_cls_extra: 0,
        cb_wnd_extra: 0,
        h_instance: hinstance,
        h_icon: std::ptr::null_mut(),
        h_cursor: cursor,
        hbr_background: std::ptr::null_mut(),
        lpsz_menu_name: std::ptr::null(),
        lpsz_class_name: class_name.as_ptr(),
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        return Err(PlatformError::Os("RegisterClassW failed".into()));
    }

    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            size.width as i32,
            size.height as i32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null_mut(),
        )
    };
    if hwnd.is_null() {
        return Err(PlatformError::Os("CreateWindowExW failed".into()));
    }
    // The window may have opened on a display with a different DPI to the system's.
    CTX.with(|c| c.borrow_mut().dpi = unsafe { GetDpiForWindow(hwnd) });
    if let Some(dark) = attrs.dark {
        // Ask the desktop window manager for a title bar matching the content.
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

    // Use the actual client area for the framebuffer size.
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe { GetClientRect(hwnd, &mut rect) };
    let client = PhysicalSize::new(
        (rect.right - rect.left).max(1) as u32,
        (rect.bottom - rect.top).max(1) as u32,
    );
    let mut win = WinWindow { hwnd, size: client };

    // Render the initial frame into the thread-local framebuffer.
    handler(Event::RedrawRequested, &win);

    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
    }

    // Message loop. The WndProc translates native messages into Stipple events
    // and queues them (it can't see the generic `handler`); we drain and
    // dispatch the queue after each message.
    //
    // Between messages, wait for input or a wakeup (timers, other threads). A
    // wake from another thread posts a message, which ends the wait.
    let target = hwnd as usize;
    crate::platform::wake::install(
        std::sync::Arc::new(move || unsafe {
            PostMessageW(target as Hwnd, WM_APP_WAKE, 0, 0);
        }),
        None,
    );
    let mut msg: Msg = unsafe { std::mem::zeroed() };
    'outer: loop {
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
            let events: Vec<Event> = CTX.with(|c| std::mem::take(&mut c.borrow_mut().queue));
            for event in events {
                if let Event::Resized(size) = event {
                    win.size = size;
                }
                if handler(event, &win) == ControlFlow::Exit {
                    unsafe { DestroyWindow(hwnd) };
                }
            }
        }
        if crate::platform::wake::take(std::time::Instant::now())
            && handler(Event::Wake, &win) == ControlFlow::Exit
        {
            unsafe { DestroyWindow(hwnd) };
        }
    }
    Ok(())
}
