//! Closing, quitting, focus and attention on macOS: the window delegate's
//! `windowShouldClose:`, `windowDidBecomeKey:` and `windowDidResignKey:`, an
//! application delegate answering `applicationShouldTerminate:`, and
//! `requestUserAttention:`, the Dock tile's badge and `NSBeep`.

use std::cell::Cell;
use std::ffi::c_void;

use super::{
    CTX, Class, Id, Sel, class, class_addMethod, msg_id, msg_void, msg_void_id, nsstring,
    objc_allocateClassPair, objc_getClass, objc_msgSend, objc_registerClassPair, sel,
};
use crate::platform::UserAttention;
use crate::platform::event::Event;

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSBeep();
}

/// `NSRequestUserAttentionType`.
const NS_CRITICAL_REQUEST: u64 = 0;
const NS_INFORMATIONAL_REQUEST: u64 = 10;
/// `NSApplicationTerminateReply`.
const NS_TERMINATE_CANCEL: u64 = 0;
const NS_TERMINATE_NOW: u64 = 1;

thread_local! {
    /// The attention request in progress (its id from `requestUserAttention:`).
    static ATTENTION: Cell<Option<i64>> = const { Cell::new(None) };
}

/// Queue `event` for the loop, which delivers it once AppKit returns.
fn post(event: Event) {
    CTX.with(|c| c.borrow_mut().pending.push(event));
}

/// `windowShouldClose:` — the close button or ⌘W. The window stays; the app
/// decides (and the loop ends if it closes).
extern "C" fn window_should_close(_this: Id, _cmd: Sel, _sender: Id) -> bool {
    post(Event::CloseRequested);
    false
}

/// `windowDidBecomeKey:` / `windowDidResignKey:`.
extern "C" fn window_did_become_key(_this: Id, _cmd: Sel, _notification: Id) {
    post(Event::FocusChanged(true));
}

extern "C" fn window_did_resign_key(_this: Id, _cmd: Sel, _notification: Id) {
    post(Event::FocusChanged(false));
}

/// Add the window delegate methods to the view class (the view is the
/// window's delegate).
///
/// # Safety
/// `cls` must be a class being built (allocated, not yet registered).
pub(super) unsafe fn add_window_methods(cls: Class) {
    unsafe {
        class_addMethod(
            cls,
            sel("windowShouldClose:"),
            window_should_close as *const c_void,
            c"c@:@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("windowDidBecomeKey:"),
            window_did_become_key as *const c_void,
            c"v@:@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("windowDidResignKey:"),
            window_did_resign_key as *const c_void,
            c"v@:@".as_ptr(),
        );
    }
}

/// `applicationShouldTerminate:` — the Dock's Quit, logging out, or anything
/// else sending `terminate:`. With nobody to ask, quit at once (AppKit then
/// exits the process, as without a delegate); otherwise cancel and let the
/// app decide ([`Event::QuitRequested`]). A logout cancelled that way stays
/// cancelled: the user logs out again once the app has quit.
extern "C" fn should_terminate(_this: Id, _cmd: Sel, _app: Id) -> u64 {
    if !crate::platform::app_confirms_quit() {
        return NS_TERMINATE_NOW;
    }
    post(Event::QuitRequested);
    NS_TERMINATE_CANCEL
}

/// Make an application delegate answering `applicationShouldTerminate:` the
/// delegate of `app`.
///
/// # Safety
/// `app` must be the shared `NSApplication`.
pub(super) unsafe fn install_app_delegate(app: Id) {
    let name = c"StippleAppDelegate";
    unsafe {
        let mut cls = objc_getClass(name.as_ptr());
        if cls.is_null() {
            cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
            class_addMethod(
                cls,
                sel("applicationShouldTerminate:"),
                should_terminate as *const c_void,
                c"Q@:@".as_ptr(),
            );
            objc_registerClassPair(cls);
        }
        // Kept for the process's life: `setDelegate:` doesn't retain.
        let delegate = msg_id(msg_id(cls, sel("alloc")), sel("init"));
        msg_void_id(app, sel("setDelegate:"), delegate);
    }
}

fn shared_app() -> Id {
    unsafe { msg_id(class("NSApplication"), sel("sharedApplication")) }
}

/// Bounce the Dock icon (once, or until the app is activated), or stop.
pub(super) fn request_attention(kind: Option<UserAttention>) {
    let app = shared_app();
    unsafe {
        if let Some(id) = ATTENTION.take() {
            let cancel: unsafe extern "C" fn(Id, Sel, i64) =
                std::mem::transmute(objc_msgSend as *const c_void);
            cancel(app, sel("cancelUserAttentionRequest:"), id);
        }
        let Some(kind) = kind else {
            return;
        };
        let ty = match kind {
            UserAttention::Informational => NS_INFORMATIONAL_REQUEST,
            UserAttention::Critical => NS_CRITICAL_REQUEST,
        };
        let request: unsafe extern "C" fn(Id, Sel, u64) -> i64 =
            std::mem::transmute(objc_msgSend as *const c_void);
        let id = request(app, sel("requestUserAttention:"), ty);
        // 0: the app is already active, so there's nothing to cancel.
        ATTENTION.set((id != 0).then_some(id));
    }
}

/// Set or clear the Dock tile's badge.
pub(super) fn set_badge(label: Option<&str>) {
    unsafe {
        let tile = msg_id(shared_app(), sel("dockTile"));
        let label = label.map_or(std::ptr::null_mut(), nsstring);
        msg_void_id(tile, sel("setBadgeLabel:"), label);
        // The tile redraws on `display`.
        msg_void(tile, sel("display"));
    }
}

/// Play the system alert sound.
pub(super) fn beep() {
    unsafe { NSBeep() };
}

/// The CI self-test (`STIPPLE_COCOA_LIFECYCLE`): close the window as its
/// close button would (`performClose:`) and quit as the Dock's Quit would
/// (`terminate:`), through real dispatch, so the delegates queue a close and
/// a quit request for the app; and set the Dock badge and read it back.
///
/// # Safety
/// `window` must be the live `NSWindow`, on the main thread.
pub(super) unsafe fn self_test(window: Id) {
    if std::env::var_os("STIPPLE_COCOA_LIFECYCLE").is_none() {
        return;
    }
    unsafe {
        msg_void_id(window, sel("performClose:"), std::ptr::null_mut());
        msg_void_id(shared_app(), sel("terminate:"), std::ptr::null_mut());
        set_badge(Some("3"));
        let tile = msg_id(shared_app(), sel("dockTile"));
        let label = super::nsstring_to_string(msg_id(tile, sel("badgeLabel")));
        set_badge(None);
        let pending = CTX.with(|c| c.borrow().pending.clone());
        println!("Cocoa lifecycle: badge={label:?} pending={pending:?}");
    }
}
