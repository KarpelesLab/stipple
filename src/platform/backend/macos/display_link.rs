//! The frame clock's display source on macOS: a `CVDisplayLink`, which calls
//! back on its own thread at each refresh of the active displays.
//!
//! The link runs only while frames are wanted: a request starts it, and after a
//! few callbacks with nothing to do it reports [`idle`](crate::platform::frame::idle)
//! and the UI thread stops it.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

type DisplayLink = *mut c_void;

/// `CVTime`: a duration as `value / scale` seconds.
#[repr(C)]
#[derive(Clone, Copy)]
struct CvTime {
    value: i64,
    scale: i32,
    flags: i32,
}
/// `kCVTimeIsIndefinite`.
const TIME_INDEFINITE: i32 = 1;

/// `CVDisplayLinkOutputCallback`. The timestamps are `const CVTimeStamp *`,
/// unused here.
type OutputCallback = extern "C" fn(
    link: DisplayLink,
    now: *const c_void,
    output_time: *const c_void,
    flags_in: u64,
    flags_out: *mut u64,
    context: *mut c_void,
) -> i32;

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVDisplayLinkCreateWithActiveCGDisplays(link: *mut DisplayLink) -> i32;
    fn CVDisplayLinkSetOutputCallback(
        link: DisplayLink,
        callback: OutputCallback,
        context: *mut c_void,
    ) -> i32;
    fn CVDisplayLinkStart(link: DisplayLink) -> i32;
    fn CVDisplayLinkStop(link: DisplayLink) -> i32;
    fn CVDisplayLinkGetNominalOutputVideoRefreshPeriod(link: DisplayLink) -> CvTime;
}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

/// The link, created once.
static LINK: AtomicUsize = AtomicUsize::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);
/// Callbacks in a row that found no frame wanted.
static IDLE_TICKS: AtomicU32 = AtomicU32::new(0);
/// After this many idle callbacks, the link is stopped.
const IDLE_LIMIT: u32 = 3;

extern "C" fn on_refresh(
    _link: DisplayLink,
    _now: *const c_void,
    _output_time: *const c_void,
    _flags_in: u64,
    _flags_out: *mut u64,
    _context: *mut c_void,
) -> i32 {
    // Waking the loop posts an autoreleased NSEvent from this thread, which has
    // no pool of its own.
    let pool = unsafe { objc_autoreleasePoolPush() };
    if crate::platform::frame::tick() {
        IDLE_TICKS.store(0, Ordering::Relaxed);
    } else if IDLE_TICKS.fetch_add(1, Ordering::Relaxed) + 1 == IDLE_LIMIT {
        crate::platform::frame::idle();
    }
    unsafe { objc_autoreleasePoolPop(pool) };
    0 // kCVReturnSuccess
}

/// Create the display link and install it as the frame clock's source. Without
/// one (no display), the frame clock's timer paces frames.
pub(super) fn install() {
    let mut link: DisplayLink = std::ptr::null_mut();
    unsafe {
        if CVDisplayLinkCreateWithActiveCGDisplays(&mut link) != 0 || link.is_null() {
            return;
        }
        if CVDisplayLinkSetOutputCallback(link, on_refresh, std::ptr::null_mut()) != 0 {
            return;
        }
        let period = CVDisplayLinkGetNominalOutputVideoRefreshPeriod(link);
        if period.flags & TIME_INDEFINITE == 0 && period.value > 0 && period.scale > 0 {
            let secs = period.value as f64 / period.scale as f64;
            crate::platform::frame::set_interval(Duration::from_secs_f64(secs));
        }
    }
    LINK.store(link as usize, Ordering::Release);
    crate::platform::frame::install(Arc::new(start), Some(Arc::new(stop)));
}

fn link() -> DisplayLink {
    LINK.load(Ordering::Acquire) as DisplayLink
}

fn start() {
    if !RUNNING.swap(true, Ordering::AcqRel) {
        IDLE_TICKS.store(0, Ordering::Relaxed);
        if unsafe { CVDisplayLinkStart(link()) } != 0 {
            RUNNING.store(false, Ordering::Release);
        }
    }
}

fn stop() {
    if RUNNING.swap(false, Ordering::AcqRel) {
        unsafe { CVDisplayLinkStop(link()) };
    }
}
