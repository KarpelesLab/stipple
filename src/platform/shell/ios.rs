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
//!
//! Scheduled notifications ([`schedule`]) are requests under the app's id with
//! a `UNTimeIntervalNotificationTrigger`, or a `UNCalendarNotificationTrigger`
//! for a daily or weekly one; the delegate's
//! `userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:`
//! reports a tap as [`Event::NotificationTapped`]. The delegate is set while
//! the app finishes launching ([`install_delegate`]), so the tap that launched
//! it is heard too.
//!
//! The share sheet ([`share`]) is a `UIActivityViewController`, and haptics
//! ([`haptic`]) the three `UIFeedbackGenerator`s.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::platform::PlatformError;
use crate::platform::event::Event;
use crate::platform::native::objc::{
    CgRect, Id, Sel, class, msg, nsstring, objc_msgSend, on_main_thread, rust_string, sel,
    with_pool,
};
use crate::platform::shell::{Haptic, Notification, ScheduledNotification, Share, ShareOutcome};

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

/// [`request_permission`]'s callers waiting for the answer.
type PermissionFn = Box<dyn FnOnce(bool) + Send>;
static ASKERS: Mutex<Vec<PermissionFn>> = Mutex::new(Vec::new());

/// The answer to the permission request, on a queue of the framework's.
unsafe extern "C" fn authorized(_block: *const c_void, granted: bool, _error: Id) {
    ANSWER.store(if granted { ALLOWED } else { REFUSED }, Ordering::Release);
    let askers = std::mem::take(&mut *ASKERS.lock().unwrap_or_else(|e| e.into_inner()));
    for f in askers {
        f(granted);
    }
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

/// The prefix of the identifiers [`notify`] gives its requests, which aren't
/// the app's to hear about.
const NOTIFY_PREFIX: &str = "stipple-notify-";

/// `userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:`:
/// the user tapped a notification (or acted on it); tell the app which.
extern "C" fn did_receive(_this: Id, _cmd: Sel, _center: Id, response: Id, handler: *mut c_void) {
    let id = (!response.is_null())
        .then(|| unsafe {
            let n = msg!(response, "notification", () -> Id);
            let request = msg!(n, "request", () -> Id);
            rust_string(msg!(request, "identifier", () -> Id))
        })
        .flatten();
    if let Some(id) = id.filter(|id| !id.starts_with(NOTIFY_PREFIX)) {
        eprintln!("stipple: notification {id:?} tapped");
        super::post(Event::NotificationTapped(id));
    }
    if !handler.is_null() {
        unsafe {
            let invoke =
                *(handler.cast::<u8>().add(16) as *const unsafe extern "C" fn(*mut c_void));
            invoke(handler);
        }
    }
}

/// Make the center's delegate ours now: called while the app finishes
/// launching, so the response to a tap that launched it reaches us.
pub(crate) fn install_delegate() {
    with_pool(|| {
        let _ = center();
    });
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
            class_addMethod(
                cls,
                sel("userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:"),
                did_receive as *const c_void,
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

/// A new (retained) `UNMutableNotificationContent` showing `n`, with the
/// default sound.
unsafe fn content(n: &Notification) -> Id {
    unsafe {
        let content =
            msg!(msg!(class("UNMutableNotificationContent"), "alloc", () -> Id), "init", () -> Id);
        let title = nsstring(&n.title);
        msg!(content, "setTitle:", (title: Id) -> ());
        let body = nsstring(&n.body);
        msg!(content, "setBody:", (body: Id) -> ());
        let sound = msg!(class("UNNotificationSound"), "defaultSound", () -> Id);
        msg!(content, "setSound:", (sound: Id) -> ());
        content
    }
}

/// Hand `n` to the notification center, to show now.
fn post(n: &Notification) {
    let Some(center) = center() else {
        return;
    };
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    unsafe {
        let content = content(n);
        let id = nsstring(&format!(
            "{NOTIFY_PREFIX}{}-{}",
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

// ---- Scheduled notifications ---------------------------------------------------

type ErrorFn = unsafe extern "C" fn(*const c_void, Id);

/// `^(NSError *error)`: a scheduled request was added, or refused.
static ADDED: Block<ErrorFn> = Block {
    isa: &raw const _NSConcreteGlobalBlock,
    flags: BLOCK_IS_GLOBAL,
    reserved: 0,
    invoke: added,
    descriptor: &DESCRIPTOR,
};

unsafe extern "C" fn added(_block: *const c_void, error: Id) {
    if error.is_null() {
        eprintln!("stipple: scheduled notification request accepted");
    } else {
        let what = rust_string(unsafe { msg!(error, "localizedDescription", () -> Id) });
        eprintln!(
            "stipple: scheduled notification refused: {}",
            what.unwrap_or_default()
        );
    }
}

const DAY_MS: u64 = 86_400_000;
const WEEK_MS: u64 = 7 * DAY_MS;

/// Add a request under `n.id` with a trigger for its time (replacing the one
/// pending under that id, as the center does).
pub(crate) fn schedule(n: &ScheduledNotification) -> Result<(), PlatformError> {
    let now = super::schedule::now_ms();
    let due = n.due_ms(now);
    let repeat = n.repeat_ms();
    with_pool(|| unsafe {
        let center = center().ok_or_else(|| PlatformError::Os("no notification center".into()))?;
        let trigger = if repeat == DAY_MS || repeat == WEEK_MS {
            // At the first time's time of day (and weekday), every day (week).
            let secs = due as f64 / 1000.0;
            let date = msg!(class("NSDate"), "dateWithTimeIntervalSince1970:", (secs: f64) -> Id);
            let calendar = msg!(class("NSCalendar"), "currentCalendar", () -> Id);
            // NSCalendarUnitHour | Minute | Second, and Weekday.
            let mut units: usize = 32 | 64 | 128;
            if repeat == WEEK_MS {
                units |= 512;
            }
            let parts = msg!(calendar, "components:fromDate:", (units: usize, date: Id) -> Id);
            let yes = true;
            msg!(class("UNCalendarNotificationTrigger"), "triggerWithDateComponents:repeats:",
                (parts: Id, yes: bool) -> Id)
        } else if repeat > 0 {
            // iOS repeats an interval of a minute or more, first after one.
            let secs = (repeat as f64 / 1000.0).max(60.0);
            let yes = true;
            msg!(class("UNTimeIntervalNotificationTrigger"), "triggerWithTimeInterval:repeats:",
                (secs: f64, yes: bool) -> Id)
        } else {
            // The interval must be positive.
            let secs = (due.saturating_sub(now) as f64 / 1000.0).max(0.1);
            let no = false;
            msg!(class("UNTimeIntervalNotificationTrigger"), "triggerWithTimeInterval:repeats:",
                (secs: f64, no: bool) -> Id)
        };
        if trigger.is_null() {
            return Err(PlatformError::Os("no trigger for the notification".into()));
        }
        let content = content(&n.notification);
        let id = nsstring(&n.id);
        let request = msg!(class("UNNotificationRequest"), "requestWithIdentifier:content:trigger:",
            (id: Id, content: Id, trigger: Id) -> Id);
        msg!(content, "release", () -> ());
        let block: *const Block<ErrorFn> = &ADDED;
        msg!(center, "addNotificationRequest:withCompletionHandler:",
            (request: Id, block: *const Block<ErrorFn>) -> ());
        Ok(())
    })
}

/// Remove the pending and delivered notifications under `id`, or all.
pub(crate) fn cancel_scheduled(id: Option<&str>) {
    with_pool(|| unsafe {
        let Some(center) = center() else {
            return;
        };
        match id {
            Some(id) => {
                let s = nsstring(id);
                let ids = msg!(class("NSArray"), "arrayWithObject:", (s: Id) -> Id);
                msg!(center, "removePendingNotificationRequestsWithIdentifiers:", (ids: Id) -> ());
                msg!(center, "removeDeliveredNotificationsWithIdentifiers:", (ids: Id) -> ());
            }
            None => {
                msg!(center, "removeAllPendingNotificationRequests", () -> ());
                msg!(center, "removeAllDeliveredNotifications", () -> ());
            }
        }
    })
}

type PendingFn = Box<dyn FnOnce(Vec<String>) + Send>;

/// The pending requests' ids (`getPendingNotificationRequestsWithCompletionHandler:`),
/// for `done`, on a queue of the framework's.
pub(crate) fn pending_scheduled(done: PendingFn) {
    type ListFn = unsafe extern "C" fn(*mut ListBlock, Id);
    #[repr(C)]
    struct ListBlock {
        isa: *const c_void,
        flags: i32,
        reserved: i32,
        invoke: ListFn,
        descriptor: *const BlockDescriptor,
        done: Option<PendingFn>,
    }
    static LIST_DESCRIPTOR: BlockDescriptor = BlockDescriptor {
        reserved: 0,
        size: std::mem::size_of::<ListBlock>(),
    };
    unsafe extern "C" fn got(block: *mut ListBlock, requests: Id) {
        let mut ids = Vec::new();
        if !requests.is_null() {
            let n = unsafe { msg!(requests, "count", () -> usize) };
            for i in 0..n {
                let id = unsafe {
                    let r = msg!(requests, "objectAtIndex:", (i: usize) -> Id);
                    rust_string(msg!(r, "identifier", () -> Id))
                };
                ids.extend(id.filter(|id| !id.starts_with(NOTIFY_PREFIX)));
            }
        }
        // Answered once; the block itself stays (leaked), as it may be copied.
        if let Some(done) = unsafe { (*block).done.take() } {
            done(ids);
        }
    }
    let Some(center) = with_pool(center) else {
        done(Vec::new());
        return;
    };
    let block: *mut ListBlock = Box::leak(Box::new(ListBlock {
        isa: &raw const _NSConcreteGlobalBlock,
        flags: BLOCK_IS_GLOBAL,
        reserved: 0,
        invoke: got,
        descriptor: &LIST_DESCRIPTOR,
        done: Some(done),
    }));
    unsafe {
        msg!(center, "getPendingNotificationRequestsWithCompletionHandler:", (block: *mut ListBlock) -> ())
    };
}

/// The app's `UNAuthorizationStatus`, from the center's settings. The center
/// answers on a queue of its own; this waits for it (a moment, at most two
/// seconds). `None` if there's no center or no answer.
pub(crate) fn notification_authorization() -> Option<isize> {
    use std::sync::{Arc, Condvar};
    let center = with_pool(center)?;
    let answer: Arc<(Mutex<Option<isize>>, Condvar)> = Arc::default();
    let got = answer.clone();
    let block = crate::platform::native::objc::once_block(move |settings: Id| {
        let status = if settings.is_null() {
            0
        } else {
            unsafe { msg!(settings, "authorizationStatus", () -> isize) }
        };
        *got.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(status);
        got.1.notify_all();
    });
    with_pool(|| unsafe {
        msg!(center, "getNotificationSettingsWithCompletionHandler:", (block: Id) -> ())
    });
    let (lock, cond) = &*answer;
    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let (guard, _) = cond
        .wait_timeout_while(guard, std::time::Duration::from_secs(2), |a| a.is_none())
        .unwrap_or_else(|e| e.into_inner());
    *guard
}

/// Ask for permission (alert, sound, badge); the system prompts the first
/// time and answers at once after.
pub(crate) fn request_permission(done: PermissionFn) {
    let Some(center) = with_pool(center) else {
        done(false);
        return;
    };
    ASKERS.lock().unwrap_or_else(|e| e.into_inner()).push(done);
    let options = AUTHORIZE_ALERT | AUTHORIZE_SOUND | AUTHORIZE_BADGE;
    let block: *const Block<AuthorizedFn> = &AUTHORIZED;
    with_pool(|| unsafe {
        msg!(center, "requestAuthorizationWithOptions:completionHandler:",
            (options: usize, block: *const Block<AuthorizedFn>) -> ())
    });
}

// ---- The share sheet -----------------------------------------------------------

/// Present a `UIActivityViewController` with `share`'s items for share
/// request `id`. `false` if there's nothing to present it from.
pub(crate) fn share(id: u32, share: &Share) -> bool {
    if !on_main_thread() {
        return false;
    }
    type DoneFn = unsafe extern "C" fn(*mut ShareBlock, Id, bool, Id, Id);
    #[repr(C)]
    struct ShareBlock {
        isa: *const c_void,
        flags: i32,
        reserved: i32,
        invoke: DoneFn,
        descriptor: *const BlockDescriptor,
        id: u32,
    }
    static SHARE_DESCRIPTOR: BlockDescriptor = BlockDescriptor {
        reserved: 0,
        size: std::mem::size_of::<ShareBlock>(),
    };
    /// `^(UIActivityType type, BOOL completed, NSArray *items, NSError *error)`.
    unsafe extern "C" fn done(
        block: *mut ShareBlock,
        _ty: Id,
        completed: bool,
        _items: Id,
        _e: Id,
    ) {
        let outcome = if completed {
            ShareOutcome::Shared
        } else {
            ShareOutcome::Cancelled
        };
        super::sharing::answer(unsafe { (*block).id }, outcome.code());
    }
    with_pool(|| unsafe {
        let top = crate::platform::dialog::picker_ios::presenter();
        if top.is_null() {
            return false;
        }
        let items = msg!(class("NSMutableArray"), "array", () -> Id);
        if !share.text.is_empty() {
            let s = nsstring(&share.text);
            msg!(items, "addObject:", (s: Id) -> ());
        }
        if !share.url.is_empty() {
            let s = nsstring(&share.url);
            let url = msg!(class("NSURL"), "URLWithString:", (s: Id) -> Id);
            let item = if url.is_null() { s } else { url };
            msg!(items, "addObject:", (item: Id) -> ());
        }
        for path in super::sharing::write_files(id, &share.files) {
            let s = nsstring(&path.to_string_lossy());
            let url = msg!(class("NSURL"), "fileURLWithPath:", (s: Id) -> Id);
            msg!(items, "addObject:", (url: Id) -> ());
        }
        let none: Id = std::ptr::null_mut();
        let avc = msg!(class("UIActivityViewController"), "alloc", () -> Id);
        let avc = msg!(avc, "initWithActivityItems:applicationActivities:",
            (items: Id, none: Id) -> Id);
        if avc.is_null() {
            return false;
        }
        if !share.subject.is_empty() {
            // Mail's subject line; key-value coding, as there's no setter.
            let subject = nsstring(&share.subject);
            let key = nsstring("subject");
            msg!(avc, "setValue:forKey:", (subject: Id, key: Id) -> ());
        }
        // Leaked: one small block per share (UIKit copies it, which for a
        // global block is the block itself).
        let block: *mut ShareBlock = Box::leak(Box::new(ShareBlock {
            isa: &raw const _NSConcreteGlobalBlock,
            flags: BLOCK_IS_GLOBAL,
            reserved: 0,
            invoke: done,
            descriptor: &SHARE_DESCRIPTOR,
            id,
        }));
        msg!(avc, "setCompletionWithItemsHandler:", (block: *mut ShareBlock) -> ());
        // On an iPad the sheet is a popover, which must point somewhere.
        let popover = msg!(avc, "popoverPresentationController", () -> Id);
        if !popover.is_null() {
            let mut view = crate::platform::backend::ios::app_view();
            if view.is_null() {
                view = msg!(top, "view", () -> Id);
            }
            msg!(popover, "setSourceView:", (view: Id) -> ());
            let rect = match share.anchor {
                Some(r) => CgRect {
                    x: r.origin.x,
                    y: r.origin.y,
                    width: r.size.width.max(1.0),
                    height: r.size.height.max(1.0),
                },
                None => {
                    let get: unsafe extern "C" fn(Id, Sel) -> CgRect =
                        std::mem::transmute(objc_msgSend as *const c_void);
                    let b = get(view, sel("bounds"));
                    CgRect {
                        x: b.x + b.width / 2.0,
                        y: b.y + b.height / 2.0,
                        width: 1.0,
                        height: 1.0,
                    }
                }
            };
            let set: unsafe extern "C" fn(Id, Sel, CgRect) =
                std::mem::transmute(objc_msgSend as *const c_void);
            set(popover, sel("setSourceRect:"), rect);
        }
        let animated = true;
        msg!(top, "presentViewController:animated:completion:",
            (avc: Id, animated: bool, none: Id) -> ());
        msg!(avc, "release", () -> ());
        eprintln!("stipple: share sheet presented");
        true
    })
}

// ---- Haptics -------------------------------------------------------------------

/// Play `kind` with a feedback generator, kept for the next time (made on
/// first use, on the main thread).
pub(crate) fn haptic(kind: Haptic) {
    if !on_main_thread() {
        return;
    }
    thread_local! {
        static GENERATORS: std::cell::RefCell<std::collections::HashMap<u8, usize>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    // (cache key, class, impact style), and how to fire it.
    let (key, cls, style): (u8, &str, isize) = match kind {
        Haptic::Selection => (0, "UISelectionFeedbackGenerator", -1),
        Haptic::Light => (1, "UIImpactFeedbackGenerator", 0),
        Haptic::Medium => (2, "UIImpactFeedbackGenerator", 1),
        Haptic::Heavy => (3, "UIImpactFeedbackGenerator", 2),
        _ => (4, "UINotificationFeedbackGenerator", -1),
    };
    with_pool(|| unsafe {
        let generator = GENERATORS.with(|g| {
            *g.borrow_mut().entry(key).or_insert_with(|| {
                let c = class(cls);
                if c.is_null() {
                    return 0;
                }
                let obj = msg!(c, "alloc", () -> Id);
                let obj = if style >= 0 {
                    msg!(obj, "initWithStyle:", (style: isize) -> Id)
                } else {
                    msg!(obj, "init", () -> Id)
                };
                obj as usize
            })
        }) as Id;
        if generator.is_null() {
            return;
        }
        match kind {
            Haptic::Selection => msg!(generator, "selectionChanged", () -> ()),
            Haptic::Light | Haptic::Medium | Haptic::Heavy => {
                msg!(generator, "impactOccurred", () -> ())
            }
            _ => {
                // UINotificationFeedbackType: success 0, warning 1, error 2.
                let ty: isize = match kind {
                    Haptic::Warning => 1,
                    Haptic::Error => 2,
                    _ => 0,
                };
                msg!(generator, "notificationOccurred:", (ty: isize) -> ())
            }
        }
    })
}
