//! iOS: `UIApplication openURL:options:completionHandler:`, and notifications
//! through `UNUserNotificationCenter`.
//!
//! The first [`notify`] asks the user's permission
//! (`requestAuthorizationWithOptions:completionHandler:`, alert, sound and
//! badge); later calls get the answer at once, without asking again. Once
//! allowed, each notification is an `UNNotificationRequest` with no trigger,
//! delivered immediately. The center's delegate (a class built at run time)
//! answers `userNotificationCenter:willPresentNotification:withCompletionHandler:`
//! with a banner, the list and the sound, so a notification also shows while
//! the app is on screen, which iOS otherwise doesn't do. The completion
//! handlers are blocks built by hand, as in the message boxes.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::platform::PlatformError;
use crate::platform::native::objc::{Id, Sel, class, msg, nsstring, sel, with_pool};
use crate::platform::shell::Notification;

pub fn open_url(url: &str) -> Result<(), PlatformError> {
    with_pool(|| {
        let s = nsstring(url);
        let nsurl = unsafe { msg!(class("NSURL"), "URLWithString:", (s: Id) -> Id) };
        if nsurl.is_null() {
            return Err(PlatformError::Os(format!("NSURL refused {url:?}")));
        }
        let app = unsafe { msg!(class("UIApplication"), "sharedApplication", () -> Id) };
        if app.is_null() {
            return Err(PlatformError::Os("no UIApplication".into()));
        }
        let options = unsafe { msg!(class("NSDictionary"), "dictionary", () -> Id) };
        let none: Id = std::ptr::null_mut();
        // Opening happens asynchronously; the (nil) completion handler would
        // say whether it worked.
        unsafe {
            msg!(app, "openURL:options:completionHandler:", (nsurl: Id, options: Id, none: Id) -> ())
        };
        Ok(())
    })
}

// The framework isn't loaded by UIKit; linking it makes its classes known.
#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_allocateClassPair(superclass: Id, name: *const i8, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn objc_getClass(name: *const i8) -> Id;
    fn objc_getProtocol(name: *const i8) -> *mut c_void;
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const i8) -> bool;
    fn class_addProtocol(cls: Id, protocol: *mut c_void) -> bool;
}

unsafe extern "C" {
    static _NSConcreteGlobalBlock: c_void;
}

const BLOCK_IS_GLOBAL: i32 = 1 << 28;

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

/// A block literal with no captures, calling `invoke`.
#[repr(C)]
struct Block<F> {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: F,
    descriptor: *const BlockDescriptor,
}

// SAFETY: the blocks are immutable statics with no captures.
unsafe impl<F> Sync for Block<F> {}

static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<Block<usize>>(),
};

type AuthorizedFn = unsafe extern "C" fn(*const c_void, bool, Id);

/// `^(BOOL granted, NSError *error)`: the permission request's answer.
static AUTHORIZED: Block<AuthorizedFn> = Block {
    isa: &raw const _NSConcreteGlobalBlock,
    flags: BLOCK_IS_GLOBAL,
    reserved: 0,
    invoke: authorized,
    descriptor: &DESCRIPTOR,
};

// `UNAuthorizationOptions`.
const AUTHORIZE_BADGE: usize = 1 << 0;
const AUTHORIZE_SOUND: usize = 1 << 1;
const AUTHORIZE_ALERT: usize = 1 << 2;
// `UNNotificationPresentationOptions` (iOS 14+).
const PRESENT_SOUND: usize = 1 << 1;
const PRESENT_LIST: usize = 1 << 3;
const PRESENT_BANNER: usize = 1 << 4;

/// What the user answered: not yet asked or still asking, allowed, refused.
const UNKNOWN: u8 = 0;
const ALLOWED: u8 = 1;
const REFUSED: u8 = 2;
static ANSWER: AtomicU8 = AtomicU8::new(UNKNOWN);

/// Notifications waiting for the permission request's answer.
static WAITING: Mutex<Vec<Notification>> = Mutex::new(Vec::new());

/// The answer to the permission request, on a queue of the framework's.
unsafe extern "C" fn authorized(_block: *const c_void, granted: bool, _error: Id) {
    ANSWER.store(if granted { ALLOWED } else { REFUSED }, Ordering::Release);
    let waiting = std::mem::take(&mut *WAITING.lock().unwrap_or_else(|e| e.into_inner()));
    for n in waiting {
        if granted {
            with_pool(|| post(&n));
        } else {
            eprintln!(
                "stipple: notification {:?} not shown: notifications aren't allowed",
                n.title
            );
        }
    }
}

/// `userNotificationCenter:willPresentNotification:withCompletionHandler:`:
/// show it while the app is on screen too.
extern "C" fn will_present(_this: Id, _cmd: Sel, _center: Id, _n: Id, handler: *mut c_void) {
    if handler.is_null() {
        return;
    }
    // A block's `invoke` follows its `isa`, flags and reserved fields.
    unsafe {
        let invoke =
            *(handler.cast::<u8>().add(16) as *const unsafe extern "C" fn(*mut c_void, usize));
        invoke(handler, PRESENT_BANNER | PRESENT_LIST | PRESENT_SOUND);
    }
}

/// The center's delegate, made once (the center holds it weakly).
fn delegate() -> Id {
    static DELEGATE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *DELEGATE.get_or_init(|| unsafe {
        let name = c"StippleNotificationDelegate";
        let mut cls = objc_getClass(name.as_ptr());
        if cls.is_null() {
            cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
            class_addMethod(
                cls,
                sel("userNotificationCenter:willPresentNotification:withCompletionHandler:"),
                will_present as *const c_void,
                c"v@:@@@?".as_ptr(),
            );
            let protocol = objc_getProtocol(c"UNUserNotificationCenterDelegate".as_ptr());
            if !protocol.is_null() {
                class_addProtocol(cls, protocol);
            }
            objc_registerClassPair(cls);
        }
        msg!(msg!(cls, "alloc", () -> Id), "init", () -> Id) as usize
    }) as Id
}

/// `[UNUserNotificationCenter currentNotificationCenter]`, with our delegate.
fn center() -> Option<Id> {
    let cls = class("UNUserNotificationCenter");
    if cls.is_null() {
        return None;
    }
    let center = unsafe { msg!(cls, "currentNotificationCenter", () -> Id) };
    if center.is_null() {
        return None;
    }
    let delegate = delegate();
    unsafe {
        if msg!(center, "delegate", () -> Id) != delegate {
            msg!(center, "setDelegate:", (delegate: Id) -> ());
        }
    }
    Some(center)
}

/// Hand `n` to the notification center, to show now.
fn post(n: &Notification) {
    let Some(center) = center() else {
        return;
    };
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    unsafe {
        let content =
            msg!(msg!(class("UNMutableNotificationContent"), "alloc", () -> Id), "init", () -> Id);
        let title = nsstring(&n.title);
        msg!(content, "setTitle:", (title: Id) -> ());
        let body = nsstring(&n.body);
        msg!(content, "setBody:", (body: Id) -> ());
        let sound = msg!(class("UNNotificationSound"), "defaultSound", () -> Id);
        msg!(content, "setSound:", (sound: Id) -> ());
        let id = nsstring(&format!(
            "stipple-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let none: Id = std::ptr::null_mut();
        let request = msg!(class("UNNotificationRequest"), "requestWithIdentifier:content:trigger:",
            (id: Id, content: Id, none: Id) -> Id);
        msg!(content, "release", () -> ());
        msg!(center, "addNotificationRequest:withCompletionHandler:", (request: Id, none: Id) -> ());
    }
}

pub fn notify(n: &Notification) -> Result<(), PlatformError> {
    match ANSWER.load(Ordering::Acquire) {
        ALLOWED => {
            with_pool(|| post(n));
            return Ok(());
        }
        REFUSED => return Err(PlatformError::Os("notifications aren't allowed".into())),
        _ => {}
    }
    with_pool(|| {
        let center = center().ok_or_else(|| PlatformError::Os("no notification center".into()))?;
        WAITING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(n.clone());
        // Asks the first time (the system's prompt); answers at once after.
        let options = AUTHORIZE_ALERT | AUTHORIZE_SOUND | AUTHORIZE_BADGE;
        let block: *const Block<AuthorizedFn> = &AUTHORIZED;
        unsafe {
            msg!(center, "requestAuthorizationWithOptions:completionHandler:",
                (options: usize, block: *const Block<AuthorizedFn>) -> ())
        };
        Ok(())
    })
}

/// The app's notification permission as the center reports it
/// (`getNotificationSettingsWithCompletionHandler:`), for self-tests:
/// `not_determined`, `denied`, `authorized`, `provisional` or `ephemeral`.
/// Called back on a queue of the framework's.
pub(crate) fn authorization_status(done: fn(&'static str)) {
    type SettingsFn = unsafe extern "C" fn(*mut SettingsBlock, Id);
    #[repr(C)]
    struct SettingsBlock {
        isa: *const c_void,
        flags: i32,
        reserved: i32,
        invoke: SettingsFn,
        descriptor: *const BlockDescriptor,
        done: fn(&'static str),
    }
    static SETTINGS_DESCRIPTOR: BlockDescriptor = BlockDescriptor {
        reserved: 0,
        size: std::mem::size_of::<SettingsBlock>(),
    };
    unsafe extern "C" fn got(block: *mut SettingsBlock, settings: Id) {
        let status = if settings.is_null() {
            -1
        } else {
            unsafe { msg!(settings, "authorizationStatus", () -> isize) }
        };
        let name = match status {
            0 => "not_determined",
            1 => "denied",
            2 => "authorized",
            3 => "provisional",
            4 => "ephemeral",
            _ => "unknown",
        };
        unsafe { ((*block).done)(name) };
    }
    let Some(center) = with_pool(center) else {
        done("unavailable");
        return;
    };
    // Leaked: one per call, from self-tests.
    let block: *mut SettingsBlock = Box::leak(Box::new(SettingsBlock {
        isa: &raw const _NSConcreteGlobalBlock,
        flags: BLOCK_IS_GLOBAL,
        reserved: 0,
        invoke: got,
        descriptor: &SETTINGS_DESCRIPTOR,
        done,
    }));
    unsafe {
        msg!(center, "getNotificationSettingsWithCompletionHandler:", (block: *mut SettingsBlock) -> ())
    };
}
