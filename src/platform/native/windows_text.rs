//! Win32: live text fields (see [`text`](super::super::text) for the
//! bookkeeping, and `docs/native-theme.md`, *Text fields*).
//!
//! A `text_field`, `number_input` or `password_field` is an `EDIT` child
//! window (`ES_AUTOHSCROLL`, `ES_PASSWORD` for a password, the hint as its
//! cue banner), a `text_area` a multi-line one (`ES_MULTILINE`,
//! `ES_WANTRETURN`, a vertical scroll bar), with the client edge Common
//! Controls 6 themes. Unlike the buttons it keeps the keyboard focus it
//! takes: the message loop hands its keys to it, so its caret, selection,
//! clipboard, undo, context menu and the input method's composition are
//! Windows' own.
//!
//! It is subclassed (its window procedure swapped for [`edit_proc`]): after
//! each message that can change what it holds (a key, a character, a click,
//! a paste, the input method's composition, read with
//! `ImmGetCompositionStringW`), what it holds is read back and the change
//! queued as an [`Event::NativeView`](crate::platform::Event::NativeView)
//! edit for the app, its taking the focus (a click into it) as the field's
//! focus. Return, Tab and Shift-Tab, Escape, and Up and Down in a one-line
//! field go to the app instead (submit, focus moves on, a number steps). Its
//! name for MSAA (as screen readers read an `EDIT`) is set with `IAccPropServices`
//! (`PROPID_ACC_NAME`): an `EDIT` has no label of its own.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;

use crate::platform::native::{
    Held, NativeKey, NativeText, NativeView, NativeViewEvent, TextMirror, TextPoke, from_crlf,
    to_crlf,
};

type Hwnd = *mut c_void;

#[link(name = "user32")]
unsafe extern "system" {
    fn SendMessageW(hwnd: Hwnd, msg: u32, wp: usize, lp: isize) -> isize;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
    fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn CallWindowProcW(proc: isize, hwnd: Hwnd, msg: u32, wp: usize, lp: isize) -> isize;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wp: usize, lp: isize) -> isize;
    fn SetFocus(hwnd: Hwnd) -> Hwnd;
    fn GetFocus() -> Hwnd;
    fn GetParent(hwnd: Hwnd) -> Hwnd;
    fn GetKeyState(vk: i32) -> i16;
    fn GetForegroundWindow() -> Hwnd;
}

#[link(name = "imm32")]
unsafe extern "system" {
    fn ImmGetContext(hwnd: Hwnd) -> *mut c_void;
    fn ImmReleaseContext(hwnd: Hwnd, himc: *mut c_void) -> i32;
    fn ImmGetCompositionStringW(himc: *mut c_void, index: u32, buf: *mut c_void, len: u32) -> i32;
    fn ImmNotifyIME(himc: *mut c_void, action: u32, index: u32, value: u32) -> i32;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid(u32, u16, u16, [u8; 8]);

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: *mut c_void,
        context: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> i32;
}

const ES_MULTILINE: u32 = 0x0004;
const ES_PASSWORD: u32 = 0x0020;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_WANTRETURN: u32 = 0x1000;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
pub(super) const WS_EX_CLIENTEDGE: u32 = 0x0200;
const GWLP_WNDPROC: i32 = -4;
const EM_GETSEL: u32 = 0x00B0;
const EM_SETSEL: u32 = 0x00B1;
const EM_SETREADONLY: u32 = 0x00CF;
const EM_SETCUEBANNER: u32 = 0x1501;
const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_IME_STARTCOMPOSITION: u32 = 0x010D;
const WM_IME_ENDCOMPOSITION: u32 = 0x010E;
const WM_IME_COMPOSITION: u32 = 0x010F;
const WM_IME_CHAR: u32 = 0x0286;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_CUT: u32 = 0x0300;
const WM_PASTE: u32 = 0x0302;
const WM_CLEAR: u32 = 0x0303;
const WM_UNDO: u32 = 0x0304;
const EM_UNDO: u32 = 0x00C7;
const EM_REPLACESEL: u32 = 0x00C2;
const WM_NCDESTROY: u32 = 0x0082;
const VK_TAB: usize = 0x09;
const VK_RETURN: usize = 0x0D;
const VK_ESCAPE: usize = 0x1B;
const VK_UP: usize = 0x26;
const VK_DOWN: usize = 0x28;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_F1: usize = 0x70;
const VK_F24: usize = 0x87;
const MK_LBUTTON: usize = 0x0001;
const GCS_COMPSTR: u32 = 0x0008;
const GCS_RESULTSTR: u32 = 0x0800;
const NI_COMPOSITIONSTR: u32 = 0x0015;
const CPS_CANCEL: u32 = 0x0004;

/// One live text control.
struct Field {
    id: u64,
    parent: Hwnd,
    multiline: bool,
    secure: bool,
    /// Return moves focus on (the field's action is Next).
    next: bool,
    /// The app wants it to have the keyboard focus.
    wants_focus: bool,
    mirror: TextMirror,
    focused: bool,
    /// The input method's composition in progress.
    composing: String,
    /// What `update` last set besides the text (set again only when that
    /// changes).
    config: String,
}

thread_local! {
    static FIELDS: RefCell<HashMap<usize, Field>> = RefCell::new(HashMap::new());
    /// Each live text control's window procedure before [`edit_proc`]
    /// (kept apart from [`FIELDS`], which may be borrowed while a message
    /// comes in).
    static PROCS: RefCell<HashMap<usize, isize>> = RefCell::new(HashMap::new());
    /// The keys handed on to the window as shortcuts (their release goes
    /// too, and the character they make doesn't go in).
    static HANDED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static SETTING: Cell<bool> = const { Cell::new(false) };
    /// `IAccPropServices` (null until made; 1 if that failed).
    static PROPS: Cell<usize> = const { Cell::new(0) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn queue(parent: Hwnd, id: u64, event: NativeViewEvent) {
    crate::platform::backend::windows::queue_native_event(
        parent,
        crate::platform::Event::NativeView { id, event },
    );
}

fn setting<R>(f: impl FnOnce() -> R) -> R {
    SETTING.with(|s| s.set(true));
    let r = f();
    SETTING.with(|s| s.set(false));
    r
}

/// The window styles of an `EDIT` for `t`.
pub(super) fn style(t: &NativeText) -> u32 {
    let mut s = WS_TABSTOP;
    if t.multiline {
        s |= ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | WS_VSCROLL;
    } else {
        s |= ES_AUTOHSCROLL;
    }
    if t.secure {
        s |= ES_PASSWORD;
    }
    s
}

/// `hwnd`, an `EDIT` just made for `v` in `parent`: subclass it and set it
/// up.
pub(super) fn adopt(parent: Hwnd, hwnd: Hwnd, v: &NativeView) {
    let Some(t) = v.text.as_deref() else {
        return;
    };
    let proc = unsafe { GetWindowLongPtrW(hwnd, GWLP_WNDPROC) };
    PROCS.with(|p| p.borrow_mut().insert(hwnd as usize, proc));
    unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, edit_proc as *const () as usize as isize) };
    FIELDS.with(|f| {
        f.borrow_mut().insert(
            hwnd as usize,
            Field {
                id: v.id,
                parent,
                multiline: t.multiline,
                secure: t.secure,
                next: false,
                wants_focus: false,
                mirror: TextMirror::default(),
                focused: false,
                composing: String::new(),
                config: String::new(),
            },
        )
    });
    update(hwnd, v);
}

/// Whether `hwnd` is a live text control (one-line or several, a
/// password or not, as `t` asks): `false` means it must be made again.
pub(super) fn fits(hwnd: Hwnd, t: &NativeText) -> bool {
    FIELDS.with(|f| {
        f.borrow()
            .get(&(hwnd as usize))
            .is_some_and(|f| f.multiline == t.multiline && f.secure == t.secure)
    })
}

/// Bring `hwnd` to `v`: its settings, its focus, and the field's text where
/// the app's state differs from what it holds.
pub(super) fn update(hwnd: Hwnd, v: &NativeView) {
    let Some(t) = v.text.as_deref() else {
        return;
    };
    let Some(mut f) = FIELDS.with(|fs| fs.borrow_mut().remove(&(hwnd as usize))) else {
        return;
    };
    f.next = t.hints.enter() == Some(crate::platform::EnterAction::Next);
    f.wants_focus = t.focused;
    let config = format!("{:?}", (t.read_only, &t.placeholder, &v.label));
    unsafe {
        if config != f.config {
            SendMessageW(hwnd, EM_SETREADONLY, t.read_only as usize, 0);
            if !t.multiline {
                let hint = wide(&t.placeholder);
                SendMessageW(hwnd, EM_SETCUEBANNER, 0, hint.as_ptr() as isize);
            }
            name(hwnd, &v.label);
            f.config = config;
        }
        let focused = GetFocus() == hwnd;
        if t.focused && !focused && GetForegroundWindow() == f.parent {
            setting(|| SetFocus(hwnd));
            f.focused = GetFocus() == hwnd;
            if t.seen >= f.mirror.sent() {
                f.mirror.forget();
            }
        } else if !t.focused && focused {
            setting(|| SetFocus(f.parent));
            f.focused = false;
        }
        if let Some(want) = f.mirror.wanted(t) {
            write(hwnd, &mut f, &want);
        }
    }
    FIELDS.with(|fs| fs.borrow_mut().insert(hwnd as usize, f));
}

/// Window `parent` took the keyboard focus back from elsewhere (it was
/// activated): the field the app has focused takes it, as it had it.
pub(crate) fn settle(parent: Hwnd) {
    let wants = FIELDS.with(|f| {
        f.borrow()
            .iter()
            .find(|(_, f)| f.parent == parent && f.wants_focus)
            .map(|(h, _)| *h as Hwnd)
    });
    if let Some(h) = wants {
        setting(|| unsafe { SetFocus(h) });
    }
}

/// Whether `other` is one of `parent`'s live text controls (focus moving
/// between the two isn't the window's focus coming or going).
pub(crate) fn is_field_of(parent: Hwnd, other: Hwnd) -> bool {
    // By its window procedure, which stays while the field is being
    // updated (and out of `FIELDS`).
    !other.is_null()
        && PROCS.with(|p| {
            p.try_borrow()
                .is_ok_and(|p| p.contains_key(&(other as usize)))
        })
        && unsafe { GetParent(other) } == parent
}

/// `hwnd` is being destroyed: forget it, and give the keyboard back to the
/// window if it had it.
pub(super) fn destroy(hwnd: Hwnd) {
    let f = FIELDS.with(|fs| fs.borrow_mut().remove(&(hwnd as usize)));
    if let Some(f) = f {
        unsafe {
            if GetFocus() == hwnd {
                setting(|| SetFocus(f.parent));
            }
            if let Some(proc) = PROCS.with(|p| p.borrow_mut().remove(&(hwnd as usize))) {
                SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc);
            }
        }
    }
}

/// Put `want` in `hwnd`, ending a composition in progress.
unsafe fn write(hwnd: Hwnd, f: &mut Field, want: &Held) {
    setting(|| unsafe {
        if !f.composing.is_empty() {
            let himc = ImmGetContext(hwnd);
            if !himc.is_null() {
                ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_CANCEL, 0);
                ImmReleaseContext(hwnd, himc);
            }
            f.composing.clear();
        }
        let (mut units, sel) = to_crlf(&want.text, want.sel_utf16());
        units.push(0);
        SetWindowTextW(hwnd, units.as_ptr());
        SendMessageW(hwnd, EM_SETSEL, sel.0, sel.1 as isize);
    })
}

/// What `hwnd` holds (the composition at the caret included).
unsafe fn read(hwnd: Hwnd, composing: &str) -> Held {
    unsafe {
        let n = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        buf.truncate(got);
        let (mut s, mut e) = (0u32, 0u32);
        SendMessageW(
            hwnd,
            EM_GETSEL,
            &mut s as *mut u32 as usize,
            &mut e as *mut u32 as isize,
        );
        let (mut text, (s, e)) = from_crlf(&buf, (s as usize, e as usize));
        let mut held = Held::from_utf16(text.clone(), (s, e), None);
        if !composing.is_empty() {
            // The input method composes at the caret, in a window of its
            // own: as marked text there, for the app.
            let at = held.sel.1;
            text.insert_str(at, composing);
            let end = at + composing.len();
            held = Held {
                text,
                sel: (end, end),
                marked: Some((at, end)),
            };
        }
        held
    }
}

/// The composition `hwnd`'s input method has in progress.
unsafe fn composition(hwnd: Hwnd) -> String {
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_null() {
            return String::new();
        }
        let bytes = ImmGetCompositionStringW(himc, GCS_COMPSTR, std::ptr::null_mut(), 0);
        let mut text = String::new();
        if bytes > 0 {
            let mut buf = vec![0u16; bytes as usize / 2];
            ImmGetCompositionStringW(himc, GCS_COMPSTR, buf.as_mut_ptr().cast(), bytes as u32);
            text = String::from_utf16_lossy(&buf);
        }
        ImmReleaseContext(hwnd, himc);
        text
    }
}

/// Read `hwnd` back and queue what the user changed for the app.
fn poll(hwnd: Hwnd) {
    if SETTING.with(|s| s.get()) {
        return;
    }
    // Read with the fields not borrowed: reading sends the control
    // messages, which come back through `edit_proc`.
    let Some(composing) = FIELDS.with(|f| {
        f.try_borrow()
            .ok()?
            .get(&(hwnd as usize))
            .map(|f| f.composing.clone())
    }) else {
        return;
    };
    let held = unsafe { read(hwnd, &composing) };
    let focused = unsafe { GetFocus() } == hwnd;
    let found = FIELDS.with(|f| {
        let mut f = f.try_borrow_mut().ok()?;
        let field = f.get_mut(&(hwnd as usize))?;
        let mut events = Vec::new();
        if focused && !field.focused {
            events.push(NativeViewEvent::Focus(true));
        }
        field.focused = focused;
        if let Some(edit) = field.mirror.observe(held) {
            events.push(NativeViewEvent::Edited(edit));
        }
        Some((field.parent, field.id, events))
    });
    if let Some((parent, id, events)) = found {
        for e in events {
            queue(parent, id, e);
        }
    }
}

/// The key the app takes from the field instead, for a key-down of
/// virtual key `vk`.
fn app_key(vk: usize, multiline: bool, next: bool) -> Option<NativeKey> {
    let shift = unsafe { GetKeyState(VK_SHIFT) } < 0;
    match vk {
        VK_RETURN if !multiline || next => Some(NativeKey::Enter),
        VK_TAB if shift => Some(NativeKey::BackTab),
        VK_TAB => Some(NativeKey::Tab),
        VK_ESCAPE => Some(NativeKey::Escape),
        VK_UP if !multiline => Some(NativeKey::Up),
        VK_DOWN if !multiline => Some(NativeKey::Down),
        _ => None,
    }
}

/// Whether a key-down of virtual key `vk` is a shortcut for the app (its
/// menus' Ctrl+S, F5...) rather than the field's: a function key, or a
/// letter or digit with Ctrl, other than the edit control's own (select
/// all, copy, cut, paste, undo).
fn shortcut(vk: usize) -> bool {
    let down = |k| unsafe { GetKeyState(k) } < 0;
    if (VK_F1..=VK_F24).contains(&vk) {
        return true;
    }
    let ctrl = down(VK_CONTROL) && !down(VK_MENU);
    let key = vk as u8;
    ctrl && vk < 0x80
        && (key.is_ascii_uppercase() || key.is_ascii_digit())
        && !matches!(key, b'A' | b'C' | b'V' | b'X' | b'Y' | b'Z')
}

/// The live text controls' window procedure (see the top of this file).
unsafe extern "system" fn edit_proc(hwnd: Hwnd, msg: u32, wp: usize, lp: isize) -> isize {
    let Some(proc) = PROCS.with(|p| p.try_borrow().ok()?.get(&(hwnd as usize)).copied()) else {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    };
    let call = || unsafe { CallWindowProcW(proc, hwnd, msg, wp, lp) };
    // Being set up, or changed by the backend: the edit control's own.
    let Some((parent, id, multiline, next)) = FIELDS.with(|f| {
        f.try_borrow().ok().and_then(|f| {
            f.get(&(hwnd as usize))
                .map(|f| (f.parent, f.id, f.multiline, f.next))
        })
    }) else {
        if msg == WM_NCDESTROY {
            PROCS.with(|p| p.borrow_mut().remove(&(hwnd as usize)));
        }
        return call();
    };
    match msg {
        WM_KEYDOWN => {
            if let Some(key) = app_key(wp, multiline, next) {
                // What was typed first, then the key.
                poll(hwnd);
                queue(parent, id, NativeViewEvent::Key(key));
                return 0;
            }
            // The app's shortcuts reach it as from its own window.
            if shortcut(wp) {
                HANDED.with(|h| h.borrow_mut().push(wp));
                unsafe { SendMessageW(parent, msg, wp, lp) };
                return 0;
            }
            let r = call();
            poll(hwnd);
            r
        }
        WM_KEYUP if HANDED.with(|h| h.borrow().contains(&wp)) => {
            HANDED.with(|h| h.borrow_mut().retain(|&k| k != wp));
            unsafe { SendMessageW(parent, msg, wp, lp) }
        }
        // The characters those keys make (a beep, a tab, a new line, a
        // shortcut's control character).
        WM_CHAR
            if matches!(wp, 0x09 | 0x1B)
                || (wp == 0x0D && (!multiline || next))
                || (wp == 0x0A && !multiline)
                || (wp < 0x20 && wp != 0x08 && HANDED.with(|h| !h.borrow().is_empty())) =>
        {
            0
        }
        WM_IME_STARTCOMPOSITION | WM_IME_COMPOSITION | WM_IME_ENDCOMPOSITION => {
            let r = call();
            let comp = if msg == WM_IME_ENDCOMPOSITION
                || (msg == WM_IME_COMPOSITION && lp as u32 & GCS_RESULTSTR != 0)
            {
                String::new()
            } else if msg == WM_IME_COMPOSITION && lp as u32 & GCS_COMPSTR != 0 {
                unsafe { composition(hwnd) }
            } else {
                FIELDS.with(|f| {
                    f.borrow()
                        .get(&(hwnd as usize))
                        .map(|f| f.composing.clone())
                        .unwrap_or_default()
                })
            };
            FIELDS.with(|f| {
                if let Some(f) = f.borrow_mut().get_mut(&(hwnd as usize)) {
                    f.composing = comp;
                }
            });
            poll(hwnd);
            r
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let r = call();
            // The window's focus comes or goes with its field's, when the
            // other end is outside it.
            let other = wp as Hwnd;
            if other != parent && !is_field_of(parent, other) {
                crate::platform::backend::windows::queue_native_event(
                    parent,
                    crate::platform::Event::FocusChanged(msg == WM_SETFOCUS),
                );
            }
            if msg == WM_SETFOCUS {
                poll(hwnd);
            } else {
                FIELDS.with(|f| {
                    if let Some(f) = f.borrow_mut().get_mut(&(hwnd as usize)) {
                        f.focused = false;
                    }
                });
            }
            r
        }
        WM_MOUSEMOVE if wp & MK_LBUTTON == 0 => call(),
        WM_KEYUP | WM_CHAR | WM_IME_CHAR | WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP
        | WM_LBUTTONDBLCLK | WM_CUT | WM_PASTE | WM_CLEAR | WM_UNDO | EM_UNDO | EM_REPLACESEL => {
            let r = call();
            poll(hwnd);
            r
        }
        WM_NCDESTROY => {
            let r = call();
            FIELDS.with(|f| f.borrow_mut().remove(&(hwnd as usize)));
            PROCS.with(|p| p.borrow_mut().remove(&(hwnd as usize)));
            r
        }
        _ => call(),
    }
}

/// Name `hwnd` for MSAA (`PROPID_ACC_NAME`).
unsafe fn name(hwnd: Hwnd, label: &str) {
    const CLSID: Guid = Guid(
        0xb5f8_350b,
        0x0548,
        0x48b1,
        [0xa6, 0xee, 0x88, 0xbd, 0x00, 0xb4, 0xa5, 0xe7],
    );
    const IID: Guid = Guid(
        0x6e26_e776,
        0x04f0,
        0x495d,
        [0x80, 0xe4, 0x33, 0x30, 0x35, 0x2e, 0x31, 0x69],
    );
    const PROPID_ACC_NAME: Guid = Guid(
        0x608d_3df8,
        0x8128,
        0x4aa7,
        [0xa4, 0x28, 0xf5, 0x5e, 0x49, 0x26, 0x72, 0x91],
    );
    const CLSCTX_INPROC_SERVER: u32 = 1;
    const OBJID_CLIENT: u32 = 0xFFFF_FFFC;
    let props = PROPS.with(|p| {
        if p.get() == 0 {
            let mut out: *mut c_void = std::ptr::null_mut();
            let hr = unsafe {
                CoCreateInstance(
                    &CLSID,
                    std::ptr::null_mut(),
                    CLSCTX_INPROC_SERVER,
                    &IID,
                    &mut out,
                )
            };
            p.set(if hr >= 0 && !out.is_null() {
                out as usize
            } else {
                1
            });
        }
        p.get()
    });
    if props <= 1 || label.is_empty() {
        return;
    }
    type SetHwndPropStr =
        unsafe extern "system" fn(*mut c_void, Hwnd, u32, u32, Guid, *const u16) -> i32;
    unsafe {
        let this = props as *mut c_void;
        let vtbl = *(this as *const *const usize);
        // IUnknown's three, then SetPropValue, SetPropServer, ClearProps,
        // SetHwndProp, SetHwndPropStr.
        let set: SetHwndPropStr = std::mem::transmute(*vtbl.add(7));
        let text = wide(label);
        set(this, hwnd, OBJID_CLIENT, 0, PROPID_ACC_NAME, text.as_ptr());
    }
}

/// What `hwnd` holds and whether it has the focus, if it is a live text
/// control (for `live_controls`).
pub(super) fn value_of(hwnd: Hwnd) -> Option<(String, bool)> {
    let composing = FIELDS.with(|f| {
        f.try_borrow()
            .ok()?
            .get(&(hwnd as usize))
            .map(|f| f.composing.clone())
    })?;
    let held = unsafe { read(hwnd, &composing) };
    Some((held.text, unsafe { GetFocus() } == hwnd))
}

/// The self-test's `poke_text` on `hwnd`.
pub(super) fn poke(hwnd: Hwnd, poke: TextPoke) -> bool {
    let known = FIELDS.with(|f| f.borrow().contains_key(&(hwnd as usize)));
    if !known {
        return false;
    }
    unsafe {
        if GetFocus() != hwnd {
            SetFocus(hwnd);
        }
        if GetFocus() != hwnd {
            return false;
        }
        match poke {
            TextPoke::Focus => {}
            // Characters as the keyboard's `TranslateMessage` makes them.
            TextPoke::Type(text) => {
                for u in text.encode_utf16() {
                    SendMessageW(hwnd, WM_CHAR, u as usize, 1);
                }
            }
            // Driving an input method takes one: not done here.
            TextPoke::Compose(_) => return false,
            TextPoke::Enter => {
                SendMessageW(hwnd, WM_KEYDOWN, VK_RETURN, 1);
            }
        }
    }
    poll(hwnd);
    true
}
