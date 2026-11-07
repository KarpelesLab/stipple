//! The frame clock's display source on Windows: a thread that waits for the
//! desktop compositor's next frame with `DwmFlush`, then delivers a tick.
//!
//! The thread sleeps on a condition variable until a frame is asked for, so an
//! idle app has it parked. `DwmFlush` blocks until DWM's next composition pass,
//! which runs at the display's refresh.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

#[link(name = "dwmapi")]
unsafe extern "system" {
    /// Block until DWM's next composition pass. Returns an `HRESULT`.
    fn DwmFlush() -> i32;
}

/// Whether a frame has been asked for and the thread hasn't picked it up yet.
static ARMED: Mutex<bool> = Mutex::new(false);
static WAKE: Condvar = Condvar::new();

/// Start the vsync thread and install it as the frame clock's source.
pub(super) fn install() {
    let started = std::thread::Builder::new()
        .name("stipple-vsync".into())
        .spawn(run)
        .is_ok();
    if !started {
        return; // the frame clock's timer paces frames instead
    }
    crate::platform::frame::install(
        Arc::new(|| {
            *ARMED.lock().unwrap_or_else(|e| e.into_inner()) = true;
            WAKE.notify_one();
        }),
        None,
    );
}

fn run() {
    let mut last = Instant::now();
    loop {
        {
            let mut armed = ARMED.lock().unwrap_or_else(|e| e.into_inner());
            while !*armed {
                armed = WAKE.wait(armed).unwrap_or_else(|e| e.into_inner());
            }
            *armed = false;
        }
        let ok = unsafe { DwmFlush() } >= 0;
        // Without composition DwmFlush fails at once, and it can return early:
        // never tick faster than twice the refresh rate, or at all without
        // waiting out an interval when it failed.
        let interval = crate::platform::frame::interval();
        let spacing = if ok { interval / 2 } else { interval };
        let since = last.elapsed();
        if since < spacing {
            std::thread::sleep(spacing - since);
        }
        last = Instant::now();
        crate::platform::frame::tick();
    }
}
