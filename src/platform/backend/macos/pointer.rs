//! Pointer odds and ends for the AppKit backend: the tracking area that
//! reports the pointer leaving the view (`NSMouseExited`), hiding the cursor
//! for [`CursorIcon::Hidden`](crate::platform::CursorIcon::Hidden) (AppKit
//! counts `hide`s and `unhide`s, so they're kept balanced), and the user's
//! double-click interval.

use std::cell::Cell;
use std::ffi::c_void;

use super::{
    CgPoint, CgRect, CgSize, Id, Sel, class, msg_f64, msg_id, msg_void, objc_msgSend, sel,
};

// NSEventType values for the other buttons, drags and the tracking area.
pub(super) const NS_RIGHT_MOUSE_DRAGGED: u64 = 7;
pub(super) const NS_MOUSE_EXITED: u64 = 9;
pub(super) const NS_OTHER_MOUSE_DOWN: u64 = 25;
pub(super) const NS_OTHER_MOUSE_UP: u64 = 26;
pub(super) const NS_OTHER_MOUSE_DRAGGED: u64 = 27;

// NSTrackingAreaOptions.
const TRACKING_MOUSE_ENTERED_AND_EXITED: u64 = 0x01;
const TRACKING_ACTIVE_ALWAYS: u64 = 0x80;
const TRACKING_IN_VISIBLE_RECT: u64 = 0x200;

thread_local! {
    /// Whether this backend has the cursor hidden (one `hide` outstanding).
    static HIDDEN: Cell<bool> = const { Cell::new(false) };
}

/// Have AppKit tell `view` when the pointer enters and leaves it, whichever
/// app is active, over whatever part of it is visible (so it follows
/// resizes).
pub(super) unsafe fn track_exits(view: Id) {
    let options =
        TRACKING_MOUSE_ENTERED_AND_EXITED | TRACKING_ACTIVE_ALWAYS | TRACKING_IN_VISIBLE_RECT;
    let zero = CgRect {
        origin: CgPoint { x: 0.0, y: 0.0 },
        size: CgSize {
            width: 0.0,
            height: 0.0,
        },
    };
    unsafe {
        let alloc = msg_id(class("NSTrackingArea"), sel("alloc"));
        let init: unsafe extern "C" fn(Id, Sel, CgRect, u64, Id, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let area = init(
            alloc,
            sel("initWithRect:options:owner:userInfo:"),
            zero,
            options,
            view,
            std::ptr::null_mut(),
        );
        if area.is_null() {
            return;
        }
        let add: unsafe extern "C" fn(Id, Sel, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        add(view, sel("addTrackingArea:"), area);
        // The view retains it.
        msg_void(area, sel("release"));
    }
}

/// Hide (`true`) or show the cursor, keeping AppKit's count balanced.
pub(super) fn set_hidden(hidden: bool) {
    if HIDDEN.with(|h| h.replace(hidden)) == hidden {
        return;
    }
    let s = if hidden { "hide" } else { "unhide" };
    unsafe { msg_void(class("NSCursor"), sel(s)) };
}

/// Record `NSEvent.doubleClickInterval`, the user's double-click time.
pub(super) fn read_double_click() {
    let secs = unsafe { msg_f64(class("NSEvent"), sel("doubleClickInterval")) };
    if secs.is_finite() && secs > 0.0 && secs < 10.0 {
        crate::platform::set_double_click(Some(std::time::Duration::from_secs_f64(secs)), None);
    }
}
