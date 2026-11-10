//! The notification-area ("tray") icon and notifications, through
//! `Shell_NotifyIconW`.
//!
//! The tray icon (uID 1) reports clicks to the main window as `WM_APP_TRAY`:
//! a left click becomes [`Event::TrayActivated`], a right click opens its menu
//! with `TrackPopupMenu`, whose pick arrives as the `WM_COMMAND` the menu bar
//! uses. A notification is a balloon on the tray icon, or on a second icon
//! (uID 2) added for it and removed when the balloon goes; Windows 10 and 11
//! show balloons as toasts.

use super::{
    CTX, Handle, Hwnd, Lparam, Point, Rect, Wparam, create_icon, fill_menu, push_event, wide,
};
use crate::platform::PlatformError;
use crate::platform::event::Event;
use crate::platform::menu::NativeTray;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// The callback message of both icons: `wParam` is the uID, `lParam`'s low
/// word the mouse message or `NIN_*` notification.
pub(super) const WM_APP_TRAY: u32 = 0x8000 + 2;
const TRAY_UID: u32 = 1;
const NOTE_UID: u32 = 2;

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIF_MESSAGE: u32 = 0x1;
const NIF_ICON: u32 = 0x2;
const NIF_TIP: u32 = 0x4;
const NIF_INFO: u32 = 0x10;
const NIIF_INFO: u32 = 0x1;
const NIN_BALLOONHIDE: u32 = 0x403;
const NIN_BALLOONTIMEOUT: u32 = 0x404;
const NIN_BALLOONUSERCLICK: u32 = 0x405;
const WM_NULL: u32 = 0;
const WM_CONTEXTMENU: u32 = 0x007B;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONUP: u32 = 0x0205;
const TPM_RIGHTBUTTON: u32 = 0x2;
const TPM_BOTTOMALIGN: u32 = 0x20;
const IDI_APPLICATION: usize = 32512;

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
struct NotifyIconData {
    cb_size: u32,
    hwnd: Hwnd,
    uid: u32,
    flags: u32,
    callback_message: u32,
    icon: Handle,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    timeout_or_version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid_item: Guid,
    balloon_icon: Handle,
}

#[repr(C)]
struct NotifyIconIdentifier {
    cb_size: u32,
    hwnd: Hwnd,
    uid: u32,
    guid_item: Guid,
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn Shell_NotifyIconW(message: u32, data: *const NotifyIconData) -> i32;
    fn Shell_NotifyIconGetRect(id: *const NotifyIconIdentifier, rect: *mut Rect) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn TrackPopupMenu(
        menu: Handle,
        flags: u32,
        x: i32,
        y: i32,
        reserved: i32,
        hwnd: Hwnd,
        rect: *const c_void,
    ) -> i32;
    fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    fn GetCursorPos(point: *mut Point) -> i32;
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn LoadIconW(instance: Handle, name: usize) -> Handle;
}

/// The main window, which owns the icons (notifications come from any thread).
static MAIN_HWND: AtomicUsize = AtomicUsize::new(0);
/// The window's icon, for the notification icon. 0: the generic app icon.
static APP_ICON: AtomicUsize = AtomicUsize::new(0);
static TRAY_UP: AtomicBool = AtomicBool::new(false);
static NOTE_UP: AtomicBool = AtomicBool::new(false);
/// `TaskbarCreated`: Explorer (re)started, and icons must be added again.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// The tray's state on the UI thread.
#[derive(Default)]
pub(super) struct TrayState {
    tray: Option<NativeTray>,
    /// The popup `HMENU`.
    menu: usize,
    /// An `HICON` we made from the tray's pixmap (0 when borrowed).
    icon: usize,
}

/// Called once the main window exists.
pub(super) fn install(hwnd: Hwnd) {
    MAIN_HWND.store(hwnd as usize, Ordering::Release);
    let name = wide("TaskbarCreated");
    TASKBAR_CREATED.store(
        unsafe { RegisterWindowMessageW(name.as_ptr()) },
        Ordering::Release,
    );
}

/// The window that owns the icons, if it is still open.
pub(super) fn owner() -> Option<Hwnd> {
    match MAIN_HWND.load(Ordering::Acquire) {
        0 => None,
        hwnd => Some(hwnd as Hwnd),
    }
}

/// The window that owned the icons closed while others stay open: make `hwnd`
/// their owner, and show the tray icon on it again.
pub(super) fn rehome(hwnd: Hwnd) {
    if MAIN_HWND.load(Ordering::Acquire) != 0 {
        return;
    }
    MAIN_HWND.store(hwnd as usize, Ordering::Release);
    let tray = CTX.with(|c| c.borrow().tray.tray.clone());
    if let Some(tray) = tray {
        set_tray(hwnd, Some(&tray));
    }
}

/// Whether `icon` is the one notifications borrow (so a closing window must
/// not destroy it).
pub(super) fn is_app_icon(icon: usize) -> bool {
    icon != 0 && APP_ICON.load(Ordering::Acquire) == icon
}

/// The window's icon changed.
pub(super) fn set_app_icon(icon: Handle) {
    APP_ICON.store(icon as usize, Ordering::Release);
}

fn data(hwnd: Hwnd, uid: u32) -> NotifyIconData {
    let mut d: NotifyIconData = unsafe { std::mem::zeroed() };
    d.cb_size = std::mem::size_of::<NotifyIconData>() as u32;
    d.hwnd = hwnd;
    d.uid = uid;
    d
}

/// Copy `s` into a fixed UTF-16 buffer, truncated and NUL-terminated.
fn copy_into(dst: &mut [u16], s: &str) {
    let max = dst.len() - 1;
    let mut n = 0;
    for u in s.encode_utf16().take(max) {
        dst[n] = u;
        n += 1;
    }
    dst[n] = 0;
}

fn fallback_icon() -> Handle {
    match APP_ICON.load(Ordering::Acquire) {
        0 => unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) },
        icon => icon as Handle,
    }
}

/// Show, update or remove (`None`) the tray icon. Returns whether it's shown.
pub(super) fn set_tray(hwnd: Hwnd, tray: Option<&NativeTray>) -> bool {
    // The icons stay with the window that owns them.
    let hwnd = owner().unwrap_or(hwnd);
    let Some(tray) = tray else {
        if TRAY_UP.swap(false, Ordering::AcqRel) {
            unsafe { Shell_NotifyIconW(NIM_DELETE, &data(hwnd, TRAY_UID)) };
        }
        replace_state(TrayState::default());
        return false;
    };
    let own_icon = tray
        .icon
        .as_ref()
        .map(create_icon)
        .unwrap_or(std::ptr::null_mut());
    let menu = unsafe { super::CreatePopupMenu() };
    fill_menu(menu, &tray.menu);

    let mut d = data(hwnd, TRAY_UID);
    d.flags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    d.callback_message = WM_APP_TRAY;
    d.icon = if own_icon.is_null() {
        fallback_icon()
    } else {
        own_icon
    };
    copy_into(&mut d.tip, &tray.tooltip);
    let mut ok = false;
    if TRAY_UP.load(Ordering::Acquire) {
        ok = unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) } != 0;
    }
    if !ok {
        ok = unsafe { Shell_NotifyIconW(NIM_ADD, &d) } != 0;
    }
    TRAY_UP.store(ok, Ordering::Release);
    debug_rect(hwnd, ok);
    replace_state(TrayState {
        tray: Some(tray.clone()),
        menu: menu as usize,
        icon: own_icon as usize,
    });
    ok
}

/// With `STIPPLE_DEBUG_SHELL` set, say where the shell placed the icon.
fn debug_rect(hwnd: Hwnd, added: bool) {
    if std::env::var_os("STIPPLE_DEBUG_SHELL").is_none() {
        return;
    }
    let id = NotifyIconIdentifier {
        cb_size: std::mem::size_of::<NotifyIconIdentifier>() as u32,
        hwnd,
        uid: TRAY_UID,
        guid_item: Guid {
            data1: 0,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        },
    };
    let mut r = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let hr = unsafe { Shell_NotifyIconGetRect(&id, &mut r) };
    eprintln!(
        "stipple shell: Shell_NotifyIcon added={added}; GetRect hr={hr:#x} rect=({}, {}, {}, {})",
        r.left, r.top, r.right, r.bottom
    );
}

fn replace_state(new: TrayState) {
    let old = CTX.with(|c| std::mem::replace(&mut c.borrow_mut().tray, new));
    unsafe {
        if old.menu != 0 {
            super::DestroyMenu(old.menu as Handle);
        }
        if old.icon != 0 {
            super::DestroyIcon(old.icon as Handle);
        }
    }
}

/// Handle `WM_APP_TRAY`.
pub(super) fn callback(hwnd: Hwnd, wp: Wparam, lp: Lparam) {
    let event = (lp & 0xffff) as u32;
    match (wp as u32, event) {
        (TRAY_UID, WM_LBUTTONUP) => push_event(Event::TrayActivated),
        (TRAY_UID, WM_RBUTTONUP | WM_CONTEXTMENU) => show_menu(hwnd),
        (NOTE_UID, NIN_BALLOONHIDE | NIN_BALLOONTIMEOUT | NIN_BALLOONUSERCLICK)
            if NOTE_UP.swap(false, Ordering::AcqRel) =>
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &data(hwnd, NOTE_UID));
        },
        _ => {}
    }
}

/// The tray menu at the pointer. The pick comes back as `WM_COMMAND`.
fn show_menu(hwnd: Hwnd) {
    let menu = CTX.with(|c| c.borrow().tray.menu);
    if menu == 0 {
        return;
    }
    let mut pt = Point { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut pt);
        // Without this the menu doesn't close when clicking elsewhere.
        SetForegroundWindow(hwnd);
        TrackPopupMenu(
            menu as Handle,
            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            hwnd,
            std::ptr::null(),
        );
        super::PostMessageW(hwnd, WM_NULL, 0, 0);
    }
}

/// Whether `msg` is `TaskbarCreated`; if so, put the tray icon back.
pub(super) fn taskbar_created(hwnd: Hwnd, msg: u32) -> bool {
    let id = TASKBAR_CREATED.load(Ordering::Acquire);
    if id == 0 || msg != id {
        return false;
    }
    TRAY_UP.store(false, Ordering::Release);
    NOTE_UP.store(false, Ordering::Release);
    let tray = CTX.with(|c| c.borrow().tray.tray.clone());
    if let Some(tray) = tray {
        set_tray(hwnd, Some(&tray));
    }
    true
}

/// Remove both icons (the window is going away).
pub(super) fn remove_all(hwnd: Hwnd) {
    if TRAY_UP.swap(false, Ordering::AcqRel) {
        unsafe { Shell_NotifyIconW(NIM_DELETE, &data(hwnd, TRAY_UID)) };
    }
    if NOTE_UP.swap(false, Ordering::AcqRel) {
        unsafe { Shell_NotifyIconW(NIM_DELETE, &data(hwnd, NOTE_UID)) };
    }
    MAIN_HWND.store(0, Ordering::Release);
}

/// Show a notification balloon. Safe from any thread once the window exists.
pub(crate) fn notify(title: &str, body: &str) -> Result<(), PlatformError> {
    let hwnd = MAIN_HWND.load(Ordering::Acquire) as Hwnd;
    if hwnd.is_null() {
        return Err(PlatformError::Os(
            "notifications need the app's window to exist".into(),
        ));
    }
    let on_tray = TRAY_UP.load(Ordering::Acquire);
    let mut d = data(hwnd, if on_tray { TRAY_UID } else { NOTE_UID });
    d.flags = NIF_INFO;
    copy_into(&mut d.info_title, title);
    // An empty text removes the balloon, so give it at least a space.
    copy_into(&mut d.info, if body.is_empty() { " " } else { body });
    d.info_flags = NIIF_INFO;
    let ok = if on_tray {
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) != 0 }
    } else {
        d.flags |= NIF_MESSAGE | NIF_ICON | NIF_TIP;
        d.callback_message = WM_APP_TRAY;
        d.icon = fallback_icon();
        copy_into(&mut d.tip, title);
        let mut ok =
            NOTE_UP.load(Ordering::Acquire) && unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) } != 0;
        if !ok {
            ok = unsafe { Shell_NotifyIconW(NIM_ADD, &d) } != 0;
        }
        NOTE_UP.store(ok, Ordering::Release);
        ok
    };
    if std::env::var_os("STIPPLE_DEBUG_SHELL").is_some() {
        eprintln!(
            "stipple shell: notification balloon on uID {} shown={ok}",
            d.uid
        );
    }
    if ok {
        Ok(())
    } else {
        Err(PlatformError::Os(
            "Shell_NotifyIconW refused the balloon".into(),
        ))
    }
}
