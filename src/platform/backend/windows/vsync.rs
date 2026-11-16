//! The frame clock's display source on Windows: a thread that waits for the
//! next frame, then delivers a tick.
//!
//! What it waits on depends on how the window is drawn. A window drawn on the
//! GPU has a DXGI swap chain with a frame-latency waitable object (see
//! `gpu/d3d/scene.rs`), signalled when the swap chain can take another frame:
//! after a present, that is when DWM has picked the frame up at the display's
//! refresh. The renderer registers the object here ([`pace_by`]); after a
//! present the thread waits on it, ticks, and leaves the wait it consumed to
//! the renderer ([`wait_for_slot`] skips its own wait), so a frame is drawn
//! exactly when the swap chain has room for it. Otherwise (the CPU path, or
//! nothing presented since the last tick) it waits with `DwmFlush`, which
//! blocks until DWM's next composition pass.
//!
//! The thread sleeps on a condition variable until a frame is asked for, so an
//! idle app has it parked: no waits, no presents.

use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[link(name = "dwmapi")]
unsafe extern "system" {
    /// Block until DWM's next composition pass. Returns an `HRESULT`.
    fn DwmFlush() -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn WaitForSingleObject(handle: *mut c_void, ms: u32) -> u32;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn DuplicateHandle(
        source_process: *mut c_void,
        source: *mut c_void,
        target_process: *mut c_void,
        target: *mut *mut c_void,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
}

const WAIT_OBJECT_0: u32 = 0;
const DUPLICATE_SAME_ACCESS: u32 = 2;

/// Whether a frame has been asked for and the thread hasn't picked it up yet.
static ARMED: Mutex<bool> = Mutex::new(false);
static WAKE: Condvar = Condvar::new();

/// A handle of our own to a swap chain's waitable object, closed when the
/// last user lets go (the thread may be waiting on it as the chain goes).
struct Handle(usize);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0 as *mut c_void) };
    }
}

/// The swap chain whose waitable object paces frames: the renderer's handle
/// (to recognize it), and our duplicate.
struct Pacer {
    original: usize,
    handle: Arc<Handle>,
}

static PACER: Mutex<Option<Pacer>> = Mutex::new(None);
/// The thread consumed the waitable object's signal for the next frame: the
/// renderer mustn't wait for it again.
static PREPAID: AtomicBool = AtomicBool::new(false);

/// Start the vsync thread and install it as the frame clock's source.
pub(super) fn install() {
    let started = std::thread::Builder::new()
        .name("stipple-vsync".into())
        .spawn(run)
        .is_ok();
    if !started {
        return; // the frame clock's timer paces frames instead
    }
    crate::platform::frame::name_source("DwmFlush");
    crate::platform::frame::install(
        Arc::new(|| {
            *ARMED.lock().unwrap_or_else(|e| e.into_inner()) = true;
            WAKE.notify_one();
        }),
        None,
    );
}

/// Pace frames by `waitable`, a swap chain's frame-latency waitable object
/// (a window's, as its swap chain is made). The latest one made wins.
#[cfg_attr(not(feature = "d3d"), allow(dead_code))]
pub(crate) fn pace_by(waitable: *mut c_void) {
    if waitable.is_null() {
        return;
    }
    let mut dup = std::ptr::null_mut();
    let ok = unsafe {
        let me = GetCurrentProcess();
        DuplicateHandle(me, waitable, me, &mut dup, 0, 0, DUPLICATE_SAME_ACCESS) != 0
    };
    if !ok || dup.is_null() {
        return;
    }
    let mut pacer = PACER.lock().unwrap_or_else(|e| e.into_inner());
    *pacer = Some(Pacer {
        original: waitable as usize,
        handle: Arc::new(Handle(dup as usize)),
    });
    PREPAID.store(false, Ordering::Release);
}

/// The swap chain owning `waitable` is going: stop pacing by it.
#[cfg_attr(not(feature = "d3d"), allow(dead_code))]
pub(crate) fn stop_pacing(waitable: *mut c_void) {
    let mut pacer = PACER.lock().unwrap_or_else(|e| e.into_inner());
    if pacer
        .as_ref()
        .is_some_and(|p| p.original == waitable as usize)
    {
        *pacer = None;
        PREPAID.store(false, Ordering::Release);
    }
}

/// For the renderer, before drawing into the swap chain owning `waitable`:
/// wait until it can take a frame, at most `timeout`. Returns `false` if it
/// couldn't in time. When the vsync thread already consumed the signal (the
/// tick that asked for this frame), returns at once.
#[cfg_attr(not(feature = "d3d"), allow(dead_code))]
pub(crate) fn wait_for_slot(waitable: *mut c_void, timeout: Duration) -> bool {
    let paced = PACER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|p| p.original == waitable as usize);
    if !paced {
        return unsafe { WaitForSingleObject(waitable, millis(timeout)) } == WAIT_OBJECT_0;
    }
    // The thread may be waiting on the same object: wait in short slices,
    // so a signal it takes is seen as prepaid within one.
    let until = Instant::now() + timeout;
    loop {
        if PREPAID.swap(false, Ordering::AcqRel) {
            return true;
        }
        if unsafe { WaitForSingleObject(waitable, 2) } == WAIT_OBJECT_0 {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
    }
}

fn millis(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX).max(1)
}

fn run() {
    let mut last = Instant::now();
    let mut by_chain = false;
    loop {
        {
            let mut armed = ARMED.lock().unwrap_or_else(|e| e.into_inner());
            while !*armed {
                armed = WAKE.wait(armed).unwrap_or_else(|e| e.into_inner());
            }
            *armed = false;
        }
        let interval = crate::platform::frame::interval();
        // The swap chain, when a frame was presented to it since the last
        // signal this thread took (else nothing will signal it).
        let chain = PACER
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|p| p.handle.clone())
            .filter(|_| !PREPAID.load(Ordering::Acquire));
        let signalled = chain.is_some_and(|h| {
            let ms = millis(interval * 2).min(100);
            unsafe { WaitForSingleObject(h.0 as *mut c_void, ms) == WAIT_OBJECT_0 }
        });
        if signalled {
            PREPAID.store(true, Ordering::Release);
        } else {
            let ok = unsafe { DwmFlush() } >= 0;
            // Without composition DwmFlush fails at once, and it can return
            // early: never tick faster than twice the refresh rate, or at all
            // without waiting out an interval when it failed.
            let spacing = if ok { interval / 2 } else { interval };
            let since = last.elapsed();
            if since < spacing {
                std::thread::sleep(spacing - since);
            }
        }
        if signalled != by_chain {
            by_chain = signalled;
            crate::platform::frame::name_source(if signalled {
                "DXGI frame-latency waitable"
            } else {
                "DwmFlush"
            });
        }
        last = Instant::now();
        crate::platform::frame::tick();
    }
}
