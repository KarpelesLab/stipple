//! Win32: a live select, a `COMBOBOX` (`CBS_DROPDOWNLIST`; see
//! [`NativeMenu`]).
//!
//! Its list holds the options, the chosen one selected (none with nothing
//! chosen); a click shows Windows' own drop-down list under it. A pick
//! (`CBN_SELENDOK`, or `CBN_SELCHANGE` from the keyboard while the list is
//! closed) becomes a [`NativeViewEvent::Picked`](super::NativeViewEvent::Picked)
//! at the parent's `WM_COMMAND` (`windows_views::command`), and the
//! keyboard focus goes back to the parent once the list closes
//! (`CBN_CLOSEUP`). The app's state then says what it shows: a pick it
//! refuses is set back. Its closed height is the select's (the selection
//! field's item height is fitted to it); the wheel over it scrolls what
//! it's in rather than changing the choice (`wheel_proc`). Screen readers
//! read it through the combo box's own UI Automation element, named with
//! the select's label through `IAccPropServices` (Dynamic Annotation) for
//! the clients that read MSAA's annotations.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use super::{NativeMenu, NativeView};

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
#[derive(Clone, Copy)]
struct Guid(u32, u16, u16, [u8; 8]);

#[link(name = "user32")]
unsafe extern "system" {
    fn SendMessageW(hwnd: Hwnd, msg: u32, wp: usize, lp: isize) -> isize;
    fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn GetParent(hwnd: Hwnd) -> Hwnd;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn CallWindowProcW(prev: isize, hwnd: Hwnd, msg: u32, wp: usize, lp: isize) -> isize;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: *mut c_void,
        context: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> i32;
}

/// The window class and style of a select.
pub(super) const CLASS: &str = "COMBOBOX";
/// `CBS_DROPDOWNLIST | WS_VSCROLL`.
pub(super) const STYLE: u32 = 0x0003 | 0x0020_0000;

const CB_ADDSTRING: u32 = 0x0143;
const CB_GETCURSEL: u32 = 0x0147;
const CB_GETCOUNT: u32 = 0x0146;
const CB_RESETCONTENT: u32 = 0x014B;
const CB_SETCURSEL: u32 = 0x014E;
const CB_GETDROPPEDSTATE: u32 = 0x0157;
const CB_SETITEMHEIGHT: u32 = 0x0153;
const CB_GETITEMHEIGHT: u32 = 0x0154;
const CB_SETMINVISIBLE: u32 = 0x1701;
const CBN_SELCHANGE: usize = 1;
const CBN_CLOSEUP: usize = 8;
const CBN_SELENDOK: usize = 9;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_MOUSEHWHEEL: u32 = 0x020E;
const WM_NCDESTROY: u32 = 0x0082;
const GWLP_WNDPROC: i32 = -4;
/// How many options the drop-down list shows before it scrolls.
const VISIBLE: usize = 8;

thread_local! {
    /// Each select's window procedure before `wheel_proc`.
    static PROCS: RefCell<HashMap<usize, isize>> = RefCell::new(HashMap::new());
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Set up new select `h` for `v`: its options and choice, its list's
/// length, the wheel, its name.
pub(super) fn init(h: Hwnd, v: &NativeView) {
    unsafe {
        SendMessageW(h, CB_SETMINVISIBLE, VISIBLE, 0);
        let prev = SetWindowLongPtrW(h, GWLP_WNDPROC, wheel_proc as *const c_void as isize);
        PROCS.with(|p| p.borrow_mut().insert(h as usize, prev));
    }
    set_items(h, v.menu.as_ref());
    select(h, v.menu.as_ref());
    name(h, &v.label);
}

/// Bring select `h` from `old` to `new`: its options when they changed,
/// its choice also when the user picked one since (`old == new`).
pub(super) fn update(h: Hwnd, old: &NativeView, new: &NativeView) {
    let options = |v: &NativeView| v.menu.as_ref().map(|m| m.options.clone());
    if options(old) != options(new) {
        set_items(h, new.menu.as_ref());
    }
    select(h, new.menu.as_ref());
    if old.label != new.label {
        name(h, &new.label);
    }
}

fn set_items(h: Hwnd, menu: Option<&NativeMenu>) {
    unsafe {
        SendMessageW(h, CB_RESETCONTENT, 0, 0);
        for label in menu.map(|m| &m.options[..]).unwrap_or_default() {
            let text = wide(label);
            SendMessageW(h, CB_ADDSTRING, 0, text.as_ptr() as isize);
        }
    }
}

fn select(h: Hwnd, menu: Option<&NativeMenu>) {
    let want = menu.and_then(|m| m.selected).map_or(-1, |i| i as isize);
    unsafe {
        if SendMessageW(h, CB_GETCURSEL, 0, 0) != want {
            SendMessageW(h, CB_SETCURSEL, want as usize, 0);
        }
    }
}

/// The window height to give select `h` whose closed field is `field`
/// physical pixels high: the field and its drop-down list (which a combo
/// box's height includes).
pub(super) fn window_height(field: i32) -> i32 {
    field * (VISIBLE as i32 + 1) + 2
}

/// Fit select `h`'s closed field to `field` physical pixels high (the
/// selection field's item height sets it).
pub(super) fn fit(h: Hwnd, field: i32) {
    unsafe {
        let mut r = Rect::default();
        GetWindowRect(h, &mut r);
        let shown = r.bottom - r.top;
        if shown == field || field <= 0 {
            return;
        }
        let item = SendMessageW(h, CB_GETITEMHEIGHT, usize::MAX, 0) as i32;
        let to = (item + field - shown).max(1);
        SendMessageW(h, CB_SETITEMHEIGHT, usize::MAX, to as isize);
    }
}

/// What a `WM_COMMAND` notification `code` from select `h` means: a pick
/// (`Some(Some(index))`), the list closing (`Some(None)`: the focus goes
/// back to the parent), or nothing to tell the app.
pub(super) fn notified(h: Hwnd, code: usize) -> Option<Option<usize>> {
    unsafe {
        match code {
            CBN_SELENDOK => Some(usize::try_from(SendMessageW(h, CB_GETCURSEL, 0, 0)).ok()),
            // A keyboard move with the list closed is a pick too; one with
            // it open waits for the choice.
            CBN_SELCHANGE if SendMessageW(h, CB_GETDROPPEDSTATE, 0, 0) == 0 => {
                Some(usize::try_from(SendMessageW(h, CB_GETCURSEL, 0, 0)).ok())
            }
            CBN_CLOSEUP => Some(None),
            _ => None,
        }
    }
}

/// Pick option `index` of select `h` as the keyboard would with its list
/// closed: the combo box selects it, and its parent hears `CBN_SELCHANGE`
/// (which a selection the app makes doesn't send, so it's sent for it).
pub(super) fn pick(h: Hwnd, index: usize) -> bool {
    unsafe {
        if index as isize >= SendMessageW(h, CB_GETCOUNT, 0, 0) {
            return false;
        }
        SendMessageW(h, CB_SETCURSEL, index, 0);
        const WM_COMMAND: u32 = 0x0111;
        SendMessageW(GetParent(h), WM_COMMAND, CBN_SELCHANGE << 16, h as isize);
    }
    true
}

/// The wheel over a closed select scrolls what it's in (its parent's),
/// rather than changing the choice.
unsafe extern "system" fn wheel_proc(h: Hwnd, msg: u32, wp: usize, lp: isize) -> isize {
    let prev = PROCS.with(|p| p.borrow().get(&(h as usize)).copied());
    let Some(prev) = prev else {
        return 0;
    };
    unsafe {
        match msg {
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL if SendMessageW(h, CB_GETDROPPEDSTATE, 0, 0) == 0 => {
                super::os_views::wheel_to_parent(h, msg, wp, lp)
            }
            WM_NCDESTROY => {
                PROCS.with(|p| p.borrow_mut().remove(&(h as usize)));
                SetWindowLongPtrW(h, GWLP_WNDPROC, prev);
                CallWindowProcW(prev, h, msg, wp, lp)
            }
            _ => CallWindowProcW(prev, h, msg, wp, lp),
        }
    }
}

/// `CLSID_AccPropServices`.
const CLSID_ACC_PROP_SERVICES: Guid = Guid(
    0xB5F8_350B,
    0x0548,
    0x48B1,
    [0xA6, 0xEE, 0x88, 0xBD, 0x00, 0xB4, 0xA5, 0xE7],
);
/// `IID_IAccPropServices`.
const IID_ACC_PROP_SERVICES: Guid = Guid(
    0x6E26_E776,
    0x04F0,
    0x495D,
    [0x80, 0xE4, 0x33, 0x30, 0x35, 0x2E, 0x31, 0x69],
);
/// `PROPID_ACC_NAME`.
const PROPID_ACC_NAME: Guid = Guid(
    0x608D_3DF8,
    0x8128,
    0x4AA7,
    [0xA4, 0x28, 0xF5, 0x5E, 0x49, 0x26, 0x72, 0x91],
);

/// Name select `h` `label` for screen readers (none: Windows names it, by
/// its choice), through Dynamic Annotation: `IAccPropServices`'s
/// `SetHwndPropStr` (or `ClearHwndProps`) on its client object.
fn name(h: Hwnd, label: &str) {
    type SetStr = unsafe extern "system" fn(*mut c_void, Hwnd, u32, u32, Guid, *const u16) -> i32;
    type Clear = unsafe extern "system" fn(*mut c_void, Hwnd, u32, u32, *const Guid, i32) -> i32;
    type Release = unsafe extern "system" fn(*mut c_void) -> u32;
    const OBJID_CLIENT: u32 = 0xFFFF_FFFC;
    const CHILDID_SELF: u32 = 0;
    unsafe {
        // An apartment for COM, if this thread has none yet.
        CoInitializeEx(std::ptr::null_mut(), 0x2);
        let mut svc: *mut c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_ACC_PROP_SERVICES,
            std::ptr::null_mut(),
            1, // CLSCTX_INPROC_SERVER
            &IID_ACC_PROP_SERVICES,
            &mut svc,
        );
        if hr < 0 || svc.is_null() {
            return;
        }
        let vtbl = *(svc as *const *const usize);
        if label.is_empty() {
            // IUnknown (3), SetPropValue, SetPropServer, ClearProps,
            // SetHwndProp, SetHwndPropStr, SetHwndPropServer, ClearHwndProps.
            let clear: Clear = std::mem::transmute(*vtbl.add(9));
            clear(svc, h, OBJID_CLIENT, CHILDID_SELF, &PROPID_ACC_NAME, 1);
        } else {
            let set: SetStr = std::mem::transmute(*vtbl.add(7));
            let text = wide(label);
            set(
                svc,
                h,
                OBJID_CLIENT,
                CHILDID_SELF,
                PROPID_ACC_NAME,
                text.as_ptr(),
            );
        }
        let release: Release = std::mem::transmute(*vtbl.add(2));
        release(svc);
    }
}
