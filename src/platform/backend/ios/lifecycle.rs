//! The app's life on iOS: going to the background and coming back, memory
//! warnings, and the system's appearance changing while it runs.
//!
//! The view observes `UIApplicationDidEnterBackgroundNotification`,
//! `UIApplicationWillEnterForegroundNotification` and
//! `UIApplicationDidReceiveMemoryWarningNotification`, which UIKit posts
//! whether the app uses scenes or not (with scenes, once the last one goes to
//! the background), and delivers them as [`Event::Lifecycle`]. Going to the
//! background (and `UIApplicationWillTerminateNotification`) also saves the
//! app's state, under the scene's session (see
//! [`saved_state`](super::saved_state)).
//!
//! `traitCollectionDidChange:` on the view follows light and dark mode: when
//! the screen's `userInterfaceStyle` changes, the next wake carries
//! [`Event::ThemeChanged`](crate::platform::Event::ThemeChanged). The screen's
//! traits, not the view's, decide, as UIKit flips a backgrounded app's traits
//! to snapshot it in both looks.

use std::cell::Cell;
use std::ffi::c_void;

use super::{
    Class, Id, ObjcSuper, Sel, call_handler, class, class_addMethod, current_window, msg_id,
    objc_msgSend, objc_msgSendSuper, sel,
};
use crate::platform::event::{Event, Lifecycle};

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static UIApplicationDidEnterBackgroundNotification: Id;
    static UIApplicationWillEnterForegroundNotification: Id;
    static UIApplicationDidReceiveMemoryWarningNotification: Id;
    static UIApplicationWillTerminateNotification: Id;
}

thread_local! {
    /// The screen's `userInterfaceStyle` last seen (0 until read).
    static STYLE: Cell<i64> = const { Cell::new(0) };
}

/// `UIScreen.mainScreen.traitCollection.userInterfaceStyle`: 1 light, 2 dark.
fn screen_style() -> i64 {
    unsafe {
        let screen = msg_id(class("UIScreen"), sel("mainScreen"));
        let traits = msg_id(screen, sel("traitCollection"));
        if traits.is_null() {
            return 0;
        }
        let f: unsafe extern "C" fn(Id, Sel) -> i64 =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(traits, sel("userInterfaceStyle"))
    }
}

/// Report an appearance change, if the screen's style changed since last
/// seen.
fn check_appearance() {
    let style = screen_style();
    if style != 0 && STYLE.with(|s| s.replace(style)) != style {
        crate::platform::native::note_changed();
    }
}

extern "C" fn trait_collection_did_change(this: Id, cmd: Sel, previous: Id) {
    unsafe {
        let sup = ObjcSuper {
            receiver: this,
            super_class: class("UIView"),
        };
        let f: unsafe extern "C" fn(*const ObjcSuper, Sel, Id) =
            std::mem::transmute(objc_msgSendSuper as *const c_void);
        f(&sup, cmd, previous);
    }
    check_appearance();
}

fn deliver(change: Lifecycle) {
    crate::platform::window::debug_window(format_args!("lifecycle {change:?}"));
    call_handler(Event::Lifecycle(change), &current_window());
}

/// Ask the app for the state to keep, and keep it (see
/// [`saved_state`](super::saved_state)): iOS may end the app from here on
/// without telling it.
fn save_state() {
    let bytes = crate::platform::saved_state::deliver_save(|event| {
        call_handler(event, &current_window());
    });
    if let Some(dir) = super::saved_state::dir() {
        // Under the scene's session: a new session (after the user swiped
        // the app away) won't take it.
        let session = super::scene::current_session();
        crate::platform::window::debug_window(format_args!(
            "saved state: {} bytes saved in session {session:?}",
            bytes.as_ref().map_or(0, Vec::len)
        ));
        super::saved_state::store(&dir, &session, bytes.as_deref());
    }
}

extern "C" fn did_enter_background(_this: Id, _cmd: Sel, _notification: Id) {
    deliver(Lifecycle::Background);
    save_state();
}

extern "C" fn will_terminate(_this: Id, _cmd: Sel, _notification: Id) {
    save_state();
}

extern "C" fn will_enter_foreground(_this: Id, _cmd: Sel, _notification: Id) {
    // Back on screen: the state saved on the way out is stale from now on.
    if let Some(dir) = super::saved_state::dir() {
        super::saved_state::store(&dir, "", None);
    }
    deliver(Lifecycle::Foreground);
    // The look may have changed while the app was away.
    check_appearance();
}

extern "C" fn did_receive_memory_warning(_this: Id, _cmd: Sel, _notification: Id) {
    deliver(Lifecycle::LowMemory);
}

const OBSERVERS: [(&str, extern "C" fn(Id, Sel, Id)); 4] = [
    ("stippleDidEnterBackground:", did_enter_background),
    ("stippleWillEnterForeground:", will_enter_foreground),
    (
        "stippleDidReceiveMemoryWarning:",
        did_receive_memory_warning,
    ),
    ("stippleWillTerminate:", will_terminate),
];

/// Add the observer methods and `traitCollectionDidChange:` to the view class.
///
/// # Safety
/// `cls` must be a class pair still being built (before
/// `objc_registerClassPair`).
pub(super) unsafe fn add_methods(cls: Class) {
    for (name, imp) in OBSERVERS {
        unsafe { class_addMethod(cls, sel(name), imp as *const c_void, c"v@:@".as_ptr()) };
    }
    unsafe {
        class_addMethod(
            cls,
            sel("traitCollectionDidChange:"),
            trait_collection_did_change as *const c_void,
            c"v@:@".as_ptr(),
        )
    };
}

/// Have `view` hear the app's lifecycle notifications, and note the current
/// appearance.
///
/// # Safety
/// `view` must be a live `StippleUIView`, which stays alive for the process.
pub(super) unsafe fn observe(view: Id) {
    STYLE.with(|s| s.set(screen_style()));
    let add: unsafe extern "C" fn(Id, Sel, Id, Sel, Id, Id) =
        unsafe { std::mem::transmute(objc_msgSend as *const c_void) };
    unsafe {
        let center = msg_id(class("NSNotificationCenter"), sel("defaultCenter"));
        let names = [
            UIApplicationDidEnterBackgroundNotification,
            UIApplicationWillEnterForegroundNotification,
            UIApplicationDidReceiveMemoryWarningNotification,
            UIApplicationWillTerminateNotification,
        ];
        for ((selector, _), name) in OBSERVERS.iter().zip(names) {
            add(
                center,
                sel("addObserver:selector:name:object:"),
                view,
                sel(selector),
                name,
                std::ptr::null_mut(),
            );
        }
    }
}

/// For self-tests: post `UIApplicationDidReceiveMemoryWarningNotification`,
/// as the system does when memory runs short.
pub(super) fn post_memory_warning() {
    unsafe {
        let center = msg_id(class("NSNotificationCenter"), sel("defaultCenter"));
        let app = msg_id(class("UIApplication"), sel("sharedApplication"));
        let post: unsafe extern "C" fn(Id, Sel, Id, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        post(
            center,
            sel("postNotificationName:object:"),
            UIApplicationDidReceiveMemoryWarningNotification,
            app,
        );
    }
}
