//! The frame clock's display source on iOS: a `CADisplayLink` on the main run
//! loop, calling back at each screen refresh.
//!
//! The link is paused while no frames are wanted: a request unpauses it, and
//! after a few callbacks with nothing to do it reports
//! [`idle`](crate::platform::frame::idle) and is paused again.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use super::{
    Id, Sel, class, class_addMethod, msg_id, msg_void_bool, nsstring, objc_allocateClassPair,
    objc_getClass, objc_msgSend, objc_registerClassPair, sel,
};

// CADisplayLink lives in QuartzCore (UIKit pulls it in too).
#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

/// The display link, created once and kept for the app's life.
static LINK: AtomicUsize = AtomicUsize::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);
/// Callbacks in a row that found no frame wanted.
static IDLE_TICKS: AtomicU32 = AtomicU32::new(0);
/// After this many idle callbacks, the link is paused.
const IDLE_LIMIT: u32 = 3;

/// `-[StippleFrameTarget frame:]`, called by the display link on the main thread.
extern "C" fn on_refresh(_this: Id, _cmd: Sel, _link: Id) {
    if crate::platform::frame::tick() {
        IDLE_TICKS.store(0, Ordering::Relaxed);
    } else if IDLE_TICKS.fetch_add(1, Ordering::Relaxed) + 1 == IDLE_LIMIT {
        crate::platform::frame::idle();
    }
}

/// A plain object whose `frame:` method the display link targets.
fn target_class() -> *mut c_void {
    let name = c"StippleFrameTarget";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
        if cls.is_null() {
            return cls;
        }
        class_addMethod(
            cls,
            sel("frame:"),
            on_refresh as *const c_void,
            c"v@:@".as_ptr(),
        );
        objc_registerClassPair(cls);
        cls
    }
}

/// Create the display link, paused, on the main run loop, and install it as the
/// frame clock's source. Call on the main thread.
pub(super) fn install() {
    unsafe {
        let cls = target_class();
        let link_cls = class("CADisplayLink");
        if cls.is_null() || link_cls.is_null() {
            return; // the frame clock's timer paces frames instead
        }
        let target = msg_id(msg_id(cls, sel("alloc")), sel("init"));
        let make: unsafe extern "C" fn(Id, Sel, Id, Sel) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let link = make(
            link_cls,
            sel("displayLinkWithTarget:selector:"),
            target,
            sel("frame:"),
        );
        if link.is_null() {
            return;
        }
        let link = msg_id(link, sel("retain"));
        msg_void_bool(link, sel("setPaused:"), true);
        let add: unsafe extern "C" fn(Id, Sel, Id, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        let run_loop = msg_id(class("NSRunLoop"), sel("mainRunLoop"));
        // NSRunLoopCommonModes, so frames keep coming while scrolling/tracking.
        add(
            link,
            sel("addToRunLoop:forMode:"),
            run_loop,
            nsstring("kCFRunLoopCommonModes"),
        );
        LINK.store(link as usize, Ordering::Release);

        // UIScreen.maximumFramesPerSecond (iOS 10.3+): 60, or 120 on ProMotion.
        let screen = msg_id(class("UIScreen"), sel("mainScreen"));
        let fps: unsafe extern "C" fn(Id, Sel) -> isize =
            std::mem::transmute(objc_msgSend as *const c_void);
        let hz = fps(screen, sel("maximumFramesPerSecond"));
        if hz > 0 {
            crate::platform::frame::set_interval(Duration::from_secs_f64(1.0 / hz as f64));
        }
    }
    crate::platform::frame::install(Arc::new(start), Some(Arc::new(stop)));
}

fn set_paused(paused: bool) {
    let link = LINK.load(Ordering::Acquire) as Id;
    if !link.is_null() {
        unsafe { msg_void_bool(link, sel("setPaused:"), paused) };
    }
}

fn start() {
    if !RUNNING.swap(true, Ordering::AcqRel) {
        IDLE_TICKS.store(0, Ordering::Relaxed);
        set_paused(false);
    }
}

fn stop() {
    if RUNNING.swap(false, Ordering::AcqRel) {
        set_paused(true);
    }
}
