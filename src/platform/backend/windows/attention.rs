//! Focus, session end, attention requests and the beep on Windows:
//! `WM_SETFOCUS`/`WM_KILLFOCUS`, `WM_QUERYENDSESSION`, `FlashWindowEx` and
//! `MessageBeep`.

use super::{Hwnd, Lresult, PostMessageW, WM_NULL, push_event_to};
use crate::platform::UserAttention;
use crate::platform::event::Event;

pub(super) const WM_SETFOCUS: u32 = 0x0007;
pub(super) const WM_KILLFOCUS: u32 = 0x0008;
pub(super) const WM_QUERYENDSESSION: u32 = 0x0011;

/// `FLASHWINFO` (`dwFlags`): stop, flash the caption, flash the taskbar
/// button, flash until the window comes to the foreground.
const FLASHW_STOP: u32 = 0;
const FLASHW_CAPTION: u32 = 1;
const FLASHW_TRAY: u32 = 2;
const FLASHW_TIMERNOFG: u32 = 0xC;
/// `MessageBeep`'s default sound.
const MB_OK: u32 = 0;

#[repr(C)]
struct FlashWInfo {
    cb_size: u32,
    hwnd: Hwnd,
    flags: u32,
    count: u32,
    timeout: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn FlashWindowEx(info: *const FlashWInfo) -> i32;
    fn MessageBeep(kind: u32) -> i32;
}

/// Queue `event` for the app, and post a message so the loop delivers it
/// even when it came with a message sent from another thread (which the loop
/// doesn't see).
fn post(hwnd: Hwnd, event: Event) {
    push_event_to(hwnd, event);
    unsafe { PostMessageW(hwnd, WM_NULL, 0, 0) };
}

/// Turn a focus message into an event for the app.
pub(super) fn focus_message(hwnd: Hwnd, msg: u32) {
    post(hwnd, Event::FocusChanged(msg == WM_SETFOCUS));
}

/// Answer `WM_QUERYENDSESSION` (the user is logging off or shutting down):
/// with no app confirming quits, let the session end; otherwise refuse for
/// now and ask the app ([`Event::QuitRequested`]). Windows then shows that
/// the app is blocking shutdown, and the user can go on anyway.
pub(super) fn query_end_session(hwnd: Hwnd) -> Lresult {
    if !crate::platform::app_confirms_quit() {
        return 1;
    }
    post(hwnd, Event::QuitRequested);
    0
}

/// The `FLASHWINFO` flags and count for an attention request: a few flashes
/// for information, flashing until the window is activated for something
/// critical, or stopping.
fn flash_params(kind: Option<UserAttention>) -> (u32, u32) {
    match kind {
        None => (FLASHW_STOP, 0),
        Some(UserAttention::Informational) => (FLASHW_TRAY | FLASHW_CAPTION, 3),
        Some(UserAttention::Critical) => (FLASHW_TRAY | FLASHW_CAPTION | FLASHW_TIMERNOFG, 0),
    }
}

/// Flash the window's taskbar button and caption, or stop.
pub(super) fn flash(hwnd: Hwnd, kind: Option<UserAttention>) {
    let (flags, count) = flash_params(kind);
    let info = FlashWInfo {
        cb_size: std::mem::size_of::<FlashWInfo>() as u32,
        hwnd,
        flags,
        count,
        timeout: 0, // the cursor blink rate
    };
    unsafe { FlashWindowEx(&info) };
}

/// Play the default system sound.
pub(super) fn beep() {
    unsafe { MessageBeep(MB_OK) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_flags() {
        assert_eq!(flash_params(None), (FLASHW_STOP, 0));
        assert_eq!(flash_params(Some(UserAttention::Informational)).1, 3);
        assert_eq!(
            flash_params(Some(UserAttention::Critical)).0 & FLASHW_TIMERNOFG,
            FLASHW_TIMERNOFG
        );
    }
}
