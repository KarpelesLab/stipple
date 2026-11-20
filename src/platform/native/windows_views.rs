//! Win32: the live controls, real child windows of the app's window (see
//! [`views`](super::views)).
//!
//! Each [`NativeView`] is a child `HWND`: a `BUTTON` (`BS_PUSHBUTTON`, the
//! primary one `BS_DEFPUSHBUTTON`), a `msctls_trackbar32` or a
//! `msctls_progress32`, at the control's frame in physical pixels (the
//! frame times the window's DPI scale), in Segoe UI at the app's text size and that DPI,
//! and `DarkMode_Explorer`-themed in the dark appearance. They are made
//! under an activation context for Common Controls 6 (the manifest
//! `shell32.dll` carries), so they get the visual styles. The parent clips
//! them out of its own painting (`WS_CLIPCHILDREN`) and answers their
//! `WM_PRINTCLIENT` with its frame, the background behind a themed
//! button's rounded corners. A click (`BN_CLICKED`) or a slider's move
//! (`WM_HSCROLL`) comes to the parent's window procedure ([`command`],
//! [`scroll`]), which queues an
//! [`Event::NativeView`](crate::platform::Event::NativeView) and hands the
//! keyboard focus back to the parent, so keys stay the app's. Checkboxes
//! and radio buttons stay pictures: a Win32 one would show (and be named
//! by) a text of its own, where Stipple draws the label beside the box.
//!
//! The wheel over a slider scrolls what it's in (it goes to the parent);
//! a horizontal wheel (a tilt, a trackpad's sideways scroll) moves the
//! slider (`trackbar_proc`), as a drag across it would.
//!
//! A select is a `COMBOBOX` (`windows_select`). A control in a scroll area
//! is cut at the area's edge by a window region (`SetWindowRgn`, the part
//! of it the areas show): Windows draws, hit-tests and clips the parent's
//! painting (`WS_CLIPCHILDREN`) to it, so one half scrolled out stays live.
//! (A region does what a clipping container child window would, without
//! moving the controls into one and passing their notifications on.)

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use super::os_select as select;
use super::{NativeControl, NativeView, NativeViewEvent, ViewBackend, ViewSet};

/// The live text fields.
#[path = "windows_text.rs"]
mod text;
pub(crate) use text::{is_field_of, settle};

type Hwnd = *mut c_void;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct ActCtx {
    size: u32,
    flags: u32,
    source: *const u16,
    arch: u16,
    lang: u16,
    directory: *const u16,
    resource: *const u16,
    application: *const u16,
    module: *mut c_void,
}

#[repr(C)]
struct InitCommonControls {
    size: u32,
    classes: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: *mut c_void,
        instance: *mut c_void,
        param: *mut c_void,
    ) -> Hwnd;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
    fn SendMessageW(hwnd: Hwnd, msg: u32, wp: usize, lp: isize) -> isize;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
    fn GetClassNameW(hwnd: Hwnd, name: *mut u16, max: i32) -> i32;
    fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn MapWindowPoints(from: Hwnd, to: Hwnd, points: *mut Point, count: u32) -> i32;
    fn ChildWindowFromPointEx(parent: Hwnd, point: Point, flags: u32) -> Hwnd;
    fn SetFocus(hwnd: Hwnd) -> Hwnd;
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn EnableWindow(hwnd: Hwnd, enable: i32) -> i32;
    fn GetDpiForWindow(hwnd: Hwnd) -> u32;
    fn GetParent(hwnd: Hwnd) -> Hwnd;
    fn SetWindowRgn(hwnd: Hwnd, rgn: *mut c_void, redraw: i32) -> i32;
    fn GetWindowRgnBox(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn CallWindowProcW(prev: isize, hwnd: Hwnd, msg: u32, wp: usize, lp: isize) -> isize;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike: u32,
        charset: u32,
        precision: u32,
        clip: u32,
        quality: u32,
        pitch: u32,
        face: *const u16,
    ) -> *mut c_void;
    fn CreateRectRgn(left: i32, top: i32, right: i32, bottom: i32) -> *mut c_void;
}

#[link(name = "uxtheme")]
unsafe extern "system" {
    fn SetWindowTheme(hwnd: Hwnd, app: *const u16, list: *const u16) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateActCtxW(ctx: *const ActCtx) -> *mut c_void;
    fn ActivateActCtx(ctx: *mut c_void, cookie: *mut usize) -> i32;
    fn DeactivateActCtx(flags: u32, cookie: usize) -> i32;
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
}

const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
const WS_CLIPCHILDREN: isize = 0x0200_0000;
const WS_EX_LAYOUTRTL: u32 = 0x0040_0000;
const WS_EX_NOPARENTNOTIFY: u32 = 0x0000_0004;
const GWL_STYLE: i32 = -16;
const BS_PUSHBUTTON: u32 = 0;
const BS_DEFPUSHBUTTON: u32 = 1;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_MOUSEHWHEEL: u32 = 0x020E;
const WM_NCDESTROY: u32 = 0x0082;
const GWLP_WNDPROC: i32 = -4;
const TBS_NOTICKS: u32 = 0x10;
const TBS_BOTH: u32 = 0x8;
const WM_SETFONT: u32 = 0x0030;
const WM_USER: u32 = 0x0400;
const TBM_GETPOS: u32 = WM_USER;
const TBM_SETPOS: u32 = WM_USER + 5;
const TBM_SETRANGEMIN: u32 = WM_USER + 7;
const TBM_SETRANGEMAX: u32 = WM_USER + 8;
const TBM_SETPAGESIZE: u32 = WM_USER + 21;
const PBM_SETPOS: u32 = WM_USER + 2;
const PBM_SETRANGE32: u32 = WM_USER + 6;
const BM_SETSTYLE: u32 = 0x00F4;
const BM_CLICK: u32 = 0x00F5;
const BN_CLICKED: usize = 0;
const TB_ENDTRACK: usize = 8;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOSIZE: u32 = 0x0001;
const CWP_SKIPINVISIBLE: u32 = 0x0001;
const ICC_BAR_CLASSES: u32 = 0x0004;
const ICC_PROGRESS_CLASS: u32 = 0x0020;
const ICC_STANDARD_CLASSES: u32 = 0x4000;
const ACTCTX_FLAG_ASSEMBLY_DIRECTORY_VALID: u32 = 0x004;
const ACTCTX_FLAG_RESOURCE_NAME_VALID: u32 = 0x008;
/// `HWND_TOP`.
const TOP: Hwnd = std::ptr::null_mut();

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn text_of(hwnd: Hwnd, class: bool) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe {
        if class {
            GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32)
        } else {
            GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32)
        }
    };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

thread_local! {
    /// Each window's controls, by its handle, with the DPI they were made
    /// for.
    static SETS: RefCell<HashMap<usize, (u32, ViewSet<usize>)>> = RefCell::new(HashMap::new());
    /// The Common Controls 6 activation context (0 until made, 1 if that
    /// failed), and the message font made per DPI.
    static CTX: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static FONTS: RefCell<HashMap<u32, usize>> = RefCell::new(HashMap::new());
    /// The region each control is cut to (in its window's coordinates),
    /// for the ones a scroll area cuts.
    static REGIONS: RefCell<HashMap<usize, (i32, i32, i32, i32)>> = RefCell::new(HashMap::new());
    /// Each slider's window procedure before `trackbar_proc`.
    static TRACKBAR_PROCS: RefCell<HashMap<usize, isize>> = RefCell::new(HashMap::new());
}

/// A slider's wheel: up and down scroll what it's in (the parent's, which
/// scrolls the area under the pointer), rather than move it; sideways (a
/// tilt, a trackpad's horizontal scroll) moves it, a step per notch as
/// the trackbar's own wheel does (right is further on).
unsafe extern "system" fn trackbar_proc(h: Hwnd, msg: u32, wp: usize, lp: isize) -> isize {
    let prev = TRACKBAR_PROCS.with(|p| p.borrow().get(&(h as usize)).copied());
    let Some(prev) = prev else {
        return 0;
    };
    unsafe {
        match msg {
            WM_MOUSEWHEEL => wheel_to_parent(h, msg, wp, lp),
            WM_MOUSEHWHEEL => {
                // The trackbar's wheel: a notch up is a step back.
                let delta = ((wp >> 16) & 0xffff) as u16 as i16;
                let back = (-(delta as i32)) as i16 as u16 as usize;
                let wp = (wp & 0xffff) | (back << 16);
                CallWindowProcW(prev, h, WM_MOUSEWHEEL, wp, lp);
                0
            }
            WM_NCDESTROY => {
                TRACKBAR_PROCS.with(|p| p.borrow_mut().remove(&(h as usize)));
                SetWindowLongPtrW(h, GWLP_WNDPROC, prev);
                CallWindowProcW(prev, h, msg, wp, lp)
            }
            _ => CallWindowProcW(prev, h, msg, wp, lp),
        }
    }
}

/// Hand the wheel message `msg` a live control `h` got to its parent, as
/// if the parent had it: first a `WM_MOUSEMOVE` at the pointer (the parent
/// hears of no moves while the pointer is over a child, and the app
/// scrolls what is under the last one), then the wheel.
pub(super) unsafe fn wheel_to_parent(h: Hwnd, msg: u32, wp: usize, lp: isize) -> isize {
    const WM_MOUSEMOVE: u32 = 0x0200;
    unsafe {
        let parent = GetParent(h);
        // The pointer, in screen coordinates.
        let mut p = [Point {
            x: (lp & 0xffff) as u16 as i16 as i32,
            y: ((lp >> 16) & 0xffff) as u16 as i16 as i32,
        }];
        MapWindowPoints(std::ptr::null_mut(), parent, p.as_mut_ptr(), 1);
        let at = ((p[0].y as u16 as isize) << 16) | (p[0].x as u16 as isize);
        SendMessageW(parent, WM_MOUSEMOVE, 0, at);
        SendMessageW(parent, msg, wp, lp)
    }
}

/// Run `f` under the Common Controls 6 activation context, made (and the
/// trackbar and progress classes registered) the first time.
fn with_styles<R>(f: impl FnOnce() -> R) -> R {
    let ctx = CTX.with(|c| {
        if c.get() == 0 {
            c.set(unsafe { make_context() }.map_or(1, |p| p as usize));
        }
        c.get()
    });
    let mut cookie = 0usize;
    let active = ctx > 1 && unsafe { ActivateActCtx(ctx as *mut c_void, &mut cookie) } != 0;
    let r = f();
    if active {
        unsafe { DeactivateActCtx(0, cookie) };
    }
    r
}

unsafe fn make_context() -> Option<*mut c_void> {
    unsafe {
        let mut dir = [0u16; 260];
        let n = GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) as usize;
        let dir_s = String::from_utf16_lossy(&dir[..n.min(dir.len())]);
        let source = wide(&format!("{dir_s}\\shell32.dll"));
        let directory = wide(&dir_s);
        let ctx = ActCtx {
            size: std::mem::size_of::<ActCtx>() as u32,
            flags: ACTCTX_FLAG_RESOURCE_NAME_VALID | ACTCTX_FLAG_ASSEMBLY_DIRECTORY_VALID,
            source: source.as_ptr(),
            arch: 0,
            lang: 0,
            directory: directory.as_ptr(),
            // The Common Controls 6 manifest shell32 carries, as resource 124.
            resource: 124 as *const u16,
            application: std::ptr::null(),
            module: std::ptr::null_mut(),
        };
        let handle = CreateActCtxW(&ctx);
        // INVALID_HANDLE_VALUE.
        let handle = (handle as isize != -1 && !handle.is_null()).then_some(handle);
        // Load Common Controls 6 and register its bar classes under it.
        let mut cookie = 0usize;
        let active = handle.is_some_and(|h| ActivateActCtx(h, &mut cookie) != 0);
        let lib = LoadLibraryW(wide("comctl32.dll").as_ptr());
        let init = GetProcAddress(lib, c"InitCommonControlsEx".as_ptr().cast());
        if !init.is_null() {
            let init: unsafe extern "system" fn(*const InitCommonControls) -> i32 =
                std::mem::transmute(init);
            let classes = InitCommonControls {
                size: std::mem::size_of::<InitCommonControls>() as u32,
                classes: ICC_BAR_CLASSES | ICC_PROGRESS_CLASS | ICC_STANDARD_CLASSES,
            };
            init(&classes);
        }
        if active {
            DeactivateActCtx(0, cookie);
        }
        handle
    }
}

/// Segoe UI, `px` physical pixels high (the em), made once per size.
fn font(px: u32) -> *mut c_void {
    FONTS.with(|f| {
        *f.borrow_mut().entry(px).or_insert_with(|| unsafe {
            let height = -(px as i32);
            CreateFontW(
                height,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                1,
                0,
                0,
                5,
                0,
                wide("Segoe UI").as_ptr(),
            ) as usize
        }) as *mut c_void
    })
}

struct Win32 {
    parent: Hwnd,
    scale: f64,
    dpi: u32,
}

impl Win32 {
    fn place(&self, h: Hwnd, v: &NativeView) {
        let s = self.scale;
        let r = v.frame;
        let (x, y) = (
            (r.min_x() * s).round() as i32,
            (r.min_y() * s).round() as i32,
        );
        let (w, hh) = (
            (r.max_x() * s).round() as i32 - x,
            (r.max_y() * s).round() as i32 - y,
        );
        // A combo box's height takes its drop-down list in too.
        let height = if v.menu.is_some() {
            select::window_height(hh)
        } else {
            hh
        };
        unsafe { SetWindowPos(h, TOP, x, y, w, height, SWP_NOZORDER | SWP_NOACTIVATE) };
        if v.menu.is_some() {
            select::fit(h, hh);
        }
        // Cut to the part its scroll areas show (in its own coordinates).
        let cut = v.visible().filter(|c| *c != v.frame).map(|c| {
            let (l, r) = (
                (c.min_x() * s).round() as i32 - x,
                (c.max_x() * s).round() as i32 - x,
            );
            // A mirrored (right-to-left) window counts from its right.
            let (l, r) = if v.rtl { (w - r, w - l) } else { (l, r) };
            (
                l,
                (c.min_y() * s).round() as i32 - y,
                r,
                (c.max_y() * s).round() as i32 - y,
            )
        });
        let had = REGIONS.with(|r| r.borrow().get(&(h as usize)).copied());
        if had != cut {
            unsafe {
                let rgn = cut.map_or(std::ptr::null_mut(), |(l, t, r, b)| {
                    CreateRectRgn(l, t, r, b)
                });
                // The window owns the region from here on.
                SetWindowRgn(h, rgn, 1);
            }
            REGIONS.with(|r| match cut {
                Some(c) => r.borrow_mut().insert(h as usize, c),
                None => r.borrow_mut().remove(&(h as usize)),
            });
        }
    }

    fn set_common(&self, h: Hwnd, v: &NativeView) {
        self.place(h, v);
        unsafe {
            // The app's text size (the message font's, 9 points, else).
            let size = if v.font_size > 0.0 { v.font_size } else { 12.0 };
            let px = (size * self.dpi as f64 / 96.0).round().max(1.0) as u32;
            SendMessageW(h, WM_SETFONT, font(px) as usize, 1);
            EnableWindow(h, v.enabled as i32);
            // A combo box and an edit control are dark in the common dialogs'
            // theme.
            let theme = wide(match (v.dark, v.menu.is_some() || v.text.is_some()) {
                (true, true) => "DarkMode_CFD",
                (true, false) => "DarkMode_Explorer",
                _ => "Explorer",
            });
            SetWindowTheme(h, theme.as_ptr(), std::ptr::null());
            // A button's text is its title; the others' (which they don't
            // show) its name for a screen reader. A combo box's is its
            // choice (`windows_select` names it), a text field's its text
            // (`text::update`).
            let text = match v.control {
                _ if v.menu.is_some() => return,
                NativeControl::Button { .. } => &v.title,
                NativeControl::TextField => return,
                _ => &v.label,
            };
            SetWindowTextW(h, wide(text).as_ptr());
        }
    }
}

impl ViewBackend for Win32 {
    type Handle = usize;

    fn create(&mut self, v: &NativeView) -> Option<usize> {
        let mut ex = 0;
        let (class, style) = match v.control {
            NativeControl::Button { .. } if v.menu.is_some() => (select::CLASS, select::STYLE),
            NativeControl::Button { primary } => (
                "BUTTON",
                if primary {
                    BS_DEFPUSHBUTTON
                } else {
                    BS_PUSHBUTTON
                },
            ),
            NativeControl::Slider { .. } => ("msctls_trackbar32", TBS_NOTICKS | TBS_BOTH),
            NativeControl::Progress { .. } => ("msctls_progress32", 0),
            NativeControl::TextField => {
                ex = text::WS_EX_CLIENTEDGE;
                ("EDIT", text::style(v.text.as_deref()?))
            }
            _ => return None,
        };
        let ex = ex | WS_EX_NOPARENTNOTIFY | if v.rtl { WS_EX_LAYOUTRTL } else { 0 };
        let h = with_styles(|| unsafe {
            CreateWindowExW(
                ex,
                wide(class).as_ptr(),
                wide(if v.text.is_some() { "" } else { &v.title }).as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | style,
                0,
                0,
                1,
                1,
                self.parent,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null_mut(),
            )
        });
        if h.is_null() {
            return None;
        }
        unsafe {
            // The parent leaves its children's areas alone when it paints.
            let style = GetWindowLongPtrW(self.parent, GWL_STYLE);
            if style & WS_CLIPCHILDREN == 0 {
                SetWindowLongPtrW(self.parent, GWL_STYLE, style | WS_CLIPCHILDREN);
            }
            match v.control {
                NativeControl::Slider { value } => {
                    SendMessageW(h, TBM_SETRANGEMIN, 0, 0);
                    SendMessageW(h, TBM_SETRANGEMAX, 0, 1000);
                    SendMessageW(h, TBM_SETPAGESIZE, 0, 100);
                    SendMessageW(h, TBM_SETPOS, 1, value as isize);
                    let prev =
                        SetWindowLongPtrW(h, GWLP_WNDPROC, trackbar_proc as *const c_void as isize);
                    TRACKBAR_PROCS.with(|p| p.borrow_mut().insert(h as usize, prev));
                }
                NativeControl::Progress { value } => {
                    SendMessageW(h, PBM_SETRANGE32, 0, 1000);
                    SendMessageW(h, PBM_SETPOS, value as usize, 0);
                }
                _ => {}
            }
        }
        self.set_common(h, v);
        if v.menu.is_some() {
            select::init(h, v);
        }
        if matches!(v.control, NativeControl::TextField) {
            text::adopt(self.parent, h, v);
        }
        Some(h as usize)
    }

    fn update(&mut self, h: &mut usize, old: &NativeView, new: &NativeView) {
        // A field that changes between one line, a password and several
        // lines is made again (an `EDIT` keeps its styles).
        if let Some(t) = new.text.as_deref()
            && !text::fits(*h as Hwnd, t)
            && let Some(fresh) = self.create(new)
        {
            self.destroy(std::mem::replace(h, fresh));
            return;
        }
        let h = *h as Hwnd;
        // Typing: only the text changed.
        if matches!(new.control, NativeControl::TextField) && old.same_but_text(new) {
            text::update(h, new);
            return;
        }
        self.set_common(h, new);
        if new.menu.is_some() {
            select::update(h, old, new);
            return;
        }
        if matches!(new.control, NativeControl::TextField) {
            text::update(h, new);
        }
        unsafe {
            match (old.control, new.control) {
                (NativeControl::Button { primary: a }, NativeControl::Button { primary: b })
                    if a != b =>
                {
                    let style = if b { BS_DEFPUSHBUTTON } else { BS_PUSHBUTTON };
                    SendMessageW(h, BM_SETSTYLE, style as usize, 1);
                }
                (_, NativeControl::Slider { value }) => {
                    let shown = SendMessageW(h, TBM_GETPOS, 0, 0);
                    if shown != value as isize {
                        SendMessageW(h, TBM_SETPOS, 1, value as isize);
                    }
                }
                (_, NativeControl::Progress { value }) => {
                    SendMessageW(h, PBM_SETPOS, value as usize, 0);
                }
                _ => {}
            }
        }
    }

    fn destroy(&mut self, h: usize) {
        REGIONS.with(|r| r.borrow_mut().remove(&h));
        text::destroy(h as Hwnd);
        unsafe { DestroyWindow(h as Hwnd) };
    }
}

/// The controls that are live here: Win32's checkboxes and radio buttons
/// show a text of their own (see the top of this file).
pub(crate) fn live_kind(control: NativeControl) -> bool {
    matches!(
        control,
        NativeControl::Button { .. }
            | NativeControl::Slider { .. }
            | NativeControl::Progress { .. }
            | NativeControl::TextField
    )
}

/// Show exactly `views` in window `parent` (`Window::set_native_views`).
pub(crate) fn sync(parent: Hwnd, views: &[NativeView]) {
    let dpi = match unsafe { GetDpiForWindow(parent) } {
        0 => 96,
        d => d,
    };
    let mut backend = Win32 {
        parent,
        scale: dpi as f64 / 96.0,
        dpi,
    };
    let (made_for, mut set) = SETS.with(|s| {
        s.borrow_mut()
            .remove(&(parent as usize))
            .unwrap_or((dpi, ViewSet::default()))
    });
    // A new DPI: every control again, at its size and in its font there.
    if made_for != dpi {
        set.sync(&mut backend, &[]);
    }
    if set.sync(&mut backend, views) {
        for (_, h) in set.in_order() {
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
            unsafe { SetWindowPos(*h as Hwnd, TOP, 0, 0, 0, 0, flags) };
        }
    }
    SETS.with(|s| s.borrow_mut().insert(parent as usize, (dpi, set)));
}

/// Forget window `parent`'s controls (it's being destroyed, and its
/// children with it).
pub(crate) fn forget(parent: Hwnd) {
    SETS.with(|s| s.borrow_mut().remove(&(parent as usize)));
}

/// The live control `child` of `parent`, if it is one: its id and kind,
/// and whether it's a select.
fn find(parent: Hwnd, child: Hwnd) -> Option<(u64, NativeControl, bool)> {
    SETS.with(|s| {
        let s = s.try_borrow().ok()?;
        let (_, set) = s.get(&(parent as usize))?;
        let id = set.id_of(&(child as usize))?;
        let (v, _) = set.get(id)?;
        Some((id, v.control, v.menu.is_some()))
    })
}

/// The user acted on control `id` of `parent`: the next list sets it to
/// what the app's state says, changed or not.
fn touched(parent: Hwnd, id: u64) {
    SETS.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut()
            && let Some((_, set)) = s.get_mut(&(parent as usize))
        {
            set.touch(id);
        }
    });
}

/// `WM_COMMAND` from child `child` of `parent`: the event a button's click
/// or a select's pick makes, if it is a live control's.
pub(crate) fn command(parent: Hwnd, wp: usize, child: Hwnd) -> Option<crate::platform::Event> {
    let (id, control, menu) = find(parent, child)?;
    if menu {
        let index = select::notified(child, (wp >> 16) & 0xffff)?;
        let Some(index) = index else {
            // The list closed: the keyboard goes back to the app.
            unsafe { SetFocus(parent) };
            return None;
        };
        touched(parent, id);
        return Some(crate::platform::Event::NativeView {
            id,
            event: NativeViewEvent::Picked(index),
        });
    }
    if (wp >> 16) & 0xffff != BN_CLICKED || matches!(control, NativeControl::TextField) {
        return None;
    }
    touched(parent, id);
    // The keyboard stays the app's.
    unsafe { SetFocus(parent) };
    Some(crate::platform::Event::NativeView {
        id,
        event: NativeViewEvent::Activated,
    })
}

/// `WM_HSCROLL` from child `child` of `parent`: the event a slider's move
/// makes, if it is a live control's.
pub(crate) fn scroll(parent: Hwnd, wp: usize, child: Hwnd) -> Option<crate::platform::Event> {
    let (id, control, _) = find(parent, child)?;
    if !matches!(control, NativeControl::Slider { .. }) {
        return None;
    }
    touched(parent, id);
    if wp & 0xffff == TB_ENDTRACK {
        unsafe { SetFocus(parent) };
    }
    let pos = unsafe { SendMessageW(child, TBM_GETPOS, 0, 0) };
    Some(crate::platform::Event::NativeView {
        id,
        event: NativeViewEvent::Value(pos as f64 / 1000.0),
    })
}

/// The child window `id` is shown in, for the accessibility bridge.
pub(crate) fn handle_of(parent: Hwnd, id: u64) -> Option<Hwnd> {
    SETS.with(|s| {
        let s = s.try_borrow().ok()?;
        s.get(&(parent as usize))?
            .1
            .get(id)
            .map(|(_, h)| *h as Hwnd)
    })
}

pub(crate) fn live_controls() -> Vec<super::LiveControl> {
    let shown: Vec<(Hwnd, u64, Hwnd)> = SETS.with(|s| {
        s.borrow()
            .iter()
            .flat_map(|(parent, (_, set))| {
                set.in_order()
                    .map(|(v, h)| (*parent as Hwnd, v.id, *h as Hwnd))
                    .collect::<Vec<_>>()
            })
            .collect()
    });
    shown
        .into_iter()
        .map(|(parent, id, h)| unsafe {
            let mut r = Rect::default();
            GetWindowRect(h, &mut r);
            let mut pts = [
                Point {
                    x: r.left,
                    y: r.top,
                },
                Point {
                    x: r.right,
                    y: r.bottom,
                },
            ];
            MapWindowPoints(std::ptr::null_mut(), parent, pts.as_mut_ptr(), 2);
            let mid = Point {
                x: (pts[0].x + pts[1].x) / 2,
                y: (pts[0].y + pts[1].y) / 2,
            };
            let hit = ChildWindowFromPointEx(parent, mid, CWP_SKIPINVISIBLE) == h;
            let dpi = match GetDpiForWindow(parent) {
                0 => 96,
                d => d,
            };
            let s = dpi as f64 / 96.0;
            let (x0, x1) = (pts[0].x.min(pts[1].x), pts[0].x.max(pts[1].x));
            let frame = crate::geometry::Rect::from_xywh(
                x0 as f64 / s,
                pts[0].y as f64 / s,
                (x1 - x0) as f64 / s,
                (pts[1].y - pts[0].y) as f64 / s,
            );
            // The window region it's cut to (in its own coordinates, left
            // to right), if any: its middle is where a click lands on it.
            let mut b = Rect::default();
            const SIMPLEREGION: i32 = 2;
            let cut = GetWindowRgnBox(h, &mut b) == SIMPLEREGION;
            let (visible, hit) = if cut {
                let visible = crate::geometry::Rect::from_xywh(
                    (x0 + b.left) as f64 / s,
                    (pts[0].y + b.top) as f64 / s,
                    (b.right - b.left) as f64 / s,
                    (b.bottom - b.top) as f64 / s,
                );
                let mid = Point {
                    x: x0 + (b.left + b.right) / 2,
                    y: pts[0].y + (b.top + b.bottom) / 2,
                };
                (
                    visible,
                    ChildWindowFromPointEx(parent, mid, CWP_SKIPINVISIBLE) == h,
                )
            } else {
                (frame, hit)
            };
            let class = text_of(h, true);
            let (value, focused) = text::value_of(h).unwrap_or_default();
            super::LiveControl {
                id,
                // A button's title, a combo box's choice.
                title: if class.eq_ignore_ascii_case("button")
                    || class.eq_ignore_ascii_case("combobox")
                {
                    text_of(h, false)
                } else {
                    String::new()
                },
                class,
                frame,
                visible,
                value,
                focused,
                hit,
                container: if cut { "HRGN".into() } else { String::new() },
                app_touches: false,
            }
        })
        .collect()
}

/// The CI script wheels over the controls with real input instead.
pub(crate) fn post_scroll(_at: crate::geometry::Point, _dx: f64, _dy: f64) -> bool {
    false
}

/// A `COMBOBOX`'s list is no menu the app opens.
pub(crate) fn menu_shown(_dismiss: bool) -> bool {
    false
}

pub(crate) fn poke_text(id: u64, poke: super::TextPoke) -> bool {
    let found = SETS.with(|s| {
        s.borrow()
            .values()
            .find_map(|(_, set)| set.get(id).map(|(_, h)| *h as Hwnd))
    });
    found.is_some_and(|h| text::poke(h, poke))
}

pub(crate) fn poke(id: u64, value: Option<f64>) -> bool {
    let found = SETS.with(|s| {
        s.borrow().values().find_map(|(_, set)| {
            set.get(id)
                .map(|(v, h)| (v.control, v.menu.is_some(), *h as Hwnd))
        })
    });
    let Some((control, menu, h)) = found else {
        return false;
    };
    if menu {
        return select::pick(h, value.unwrap_or(0.0) as usize);
    }
    unsafe {
        match control {
            // A click, as the user's: the button tells its parent.
            NativeControl::Button { .. } => {
                SendMessageW(h, BM_CLICK, 0, 0);
            }
            // The trackbar tells its parent only of the user's moves: say it
            // for it, as a move to the new position would.
            NativeControl::Slider { .. } => {
                let pos = (value.unwrap_or(0.5) * 1000.0).round() as isize;
                SendMessageW(h, TBM_SETPOS, 1, pos);
                const WM_HSCROLL: u32 = 0x0114;
                const TB_THUMBPOSITION: usize = 4;
                SendMessageW(
                    GetParent(h),
                    WM_HSCROLL,
                    TB_THUMBPOSITION | ((pos as usize) << 16),
                    h as isize,
                );
            }
            _ => {}
        }
    }
    true
}
