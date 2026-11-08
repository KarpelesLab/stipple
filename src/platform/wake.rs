//! Waking the event loop from timers and from other threads.
//!
//! The App keeps a deadline (its next timer) here, and any thread can call
//! [`wake`] to have the loop run soon. A backend's event loop waits for OS events
//! *or* either of those, then delivers [`Event::Wake`](crate::platform::Event::Wake).
//!
//! There's one event loop per process, so this state is global.

use crate::platform::time::Instant;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The wake state. The process has one ([`GLOBAL`]); tests make their own.
struct State {
    pending: AtomicBool,
    deadline: Mutex<Option<Instant>>,
    /// How the running backend interrupts its wait. `None` before one starts.
    hooks: Mutex<Option<Hooks>>,
}

static GLOBAL: State = State::new();

#[derive(Clone)]
struct Hooks {
    /// Interrupt the backend's wait, from any thread.
    signal: Arc<dyn Fn() + Send + Sync>,
    /// Arrange a wakeup at a deadline, for backends that can't wait with a timeout.
    schedule: Option<Arc<dyn Fn(Instant) + Send + Sync>>,
}

impl State {
    const fn new() -> Self {
        Self {
            pending: AtomicBool::new(false),
            deadline: Mutex::new(None),
            hooks: Mutex::new(None),
        }
    }

    fn hooks(&self) -> Option<Hooks> {
        self.hooks.lock().ok().and_then(|h| h.clone())
    }

    fn wake(&self) {
        self.pending.store(true, Ordering::Release);
        if let Some(hooks) = self.hooks() {
            (hooks.signal)();
        }
    }

    fn set_deadline(&self, deadline: Option<Instant>) {
        if let Ok(mut d) = self.deadline.lock() {
            *d = deadline;
        }
        if let Some(at) = deadline
            && let Some(schedule) = self.hooks().and_then(|h| h.schedule)
        {
            schedule(at);
        }
    }

    fn deadline(&self) -> Option<Instant> {
        self.deadline.lock().ok().and_then(|d| *d)
    }

    fn take(&self, now: Instant) -> bool {
        let woken = self.pending.swap(false, Ordering::AcqRel);
        let due = self
            .deadline
            .lock()
            .map(|mut d| match *d {
                Some(at) if at <= now => {
                    *d = None;
                    true
                }
                _ => false,
            })
            .unwrap_or(false);
        woken || due
    }

    fn timeout(&self, now: Instant) -> Option<std::time::Duration> {
        if self.pending.load(Ordering::Acquire) {
            return Some(std::time::Duration::ZERO);
        }
        self.deadline().map(|at| at.saturating_duration_since(now))
    }
}

/// Ask the event loop to deliver [`Event::Wake`](crate::platform::Event::Wake)
/// soon. Safe from any thread, and cheap to call repeatedly.
pub fn wake() {
    GLOBAL.wake();
}

/// Set (or clear) when the loop must next wake even with no events: the App's
/// next timer.
pub fn set_deadline(deadline: Option<Instant>) {
    GLOBAL.set_deadline(deadline);
}

/// The current deadline, if any.
pub fn deadline() -> Option<Instant> {
    GLOBAL.deadline()
}

/// Whether the loop should deliver a wake now: a pending [`wake`], or a deadline
/// that has passed. Consumes both.
pub fn take(now: Instant) -> bool {
    let woken = GLOBAL.take(now);
    // Both run: a frame that's due is consumed even when a wake came too.
    let frame = super::frame::take_due(now);
    woken || frame
}

/// How long the loop may sleep before the deadline, or `None` to sleep until an
/// event. Zero when a wake is already pending.
pub fn timeout(now: Instant) -> Option<std::time::Duration> {
    let frame = super::frame::deadline().map(|at| at.saturating_duration_since(now));
    match (GLOBAL.timeout(now), frame) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Have a backend whose loop can't take a timeout wake at `at` (see
/// [`install`]). Backends that wait with [`timeout`] need nothing.
pub(crate) fn schedule(at: Instant) {
    if let Some(schedule) = GLOBAL.hooks().and_then(|h| h.schedule) {
        schedule(at);
    }
}

/// Install the running backend's way of interrupting its wait. `schedule` is for
/// backends whose loop can't take a timeout, and arranges a wakeup at a deadline.
// Android's loop doesn't wake on demand yet.
#[cfg_attr(target_os = "android", allow(dead_code))]
pub(crate) fn install(
    signal: Arc<dyn Fn() + Send + Sync>,
    schedule: Option<Arc<dyn Fn(Instant) + Send + Sync>>,
) {
    if let Ok(mut h) = GLOBAL.hooks.lock() {
        *h = Some(Hooks { signal, schedule });
    }
    // A wake that arrived before the backend started still needs delivering.
    if GLOBAL.pending.load(Ordering::Acquire) {
        wake();
    }
}

/// A self-pipe for Unix backends: [`wake`] writes a byte, and [`wait_readable`]
/// polls it alongside the display connection.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod unix {
    use std::sync::OnceLock;
    use std::time::Instant;

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    const POLLIN: i16 = 1;
    const O_NONBLOCK: i32 = 0o4000;
    const O_CLOEXEC: i32 = 0o2_000_000;
    const EINTR: i32 = 4;

    unsafe extern "C" {
        fn pipe2(fds: *mut i32, flags: i32) -> i32;
        fn poll(fds: *mut PollFd, n: u64, timeout_ms: i32) -> i32;
        fn read(fd: i32, buf: *mut std::ffi::c_void, n: usize) -> isize;
        fn write(fd: i32, buf: *const std::ffi::c_void, n: usize) -> isize;
        fn __errno_location() -> *mut i32;
    }

    /// `(read end, write end)`, created on first use.
    fn pipe() -> Option<(i32, i32)> {
        static PIPE: OnceLock<Option<(i32, i32)>> = OnceLock::new();
        *PIPE.get_or_init(|| {
            let mut fds = [0i32; 2];
            (unsafe { pipe2(fds.as_mut_ptr(), O_NONBLOCK | O_CLOEXEC) } == 0)
                .then_some((fds[0], fds[1]))
        })
    }

    /// Install the pipe as the wake signal. Call once when the loop starts.
    pub(crate) fn install() {
        let Some((_, write_end)) = pipe() else {
            return;
        };
        super::install(
            std::sync::Arc::new(move || {
                // A full pipe already means "wake", so a failed write is fine.
                let _ = unsafe { write(write_end, [1u8].as_ptr().cast(), 1) };
            }),
            None,
        );
    }

    /// Block until `fd` is readable (`true`), or until a wake or the deadline is
    /// due (`false`).
    pub(crate) fn wait_readable(fd: i32) -> bool {
        let pipe = pipe();
        loop {
            let now = Instant::now();
            let timeout = super::timeout(now);
            if timeout == Some(std::time::Duration::ZERO) {
                return false;
            }
            let ms = match timeout {
                // Round up, so we don't wake a hair early and spin.
                Some(t) => t.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32,
                None => -1,
            };
            let mut fds = [
                PollFd {
                    fd,
                    events: POLLIN,
                    revents: 0,
                },
                PollFd {
                    fd: pipe.map_or(-1, |p| p.0),
                    events: POLLIN,
                    revents: 0,
                },
            ];
            let n = unsafe { poll(fds.as_mut_ptr(), 2, ms) };
            if n < 0 {
                if unsafe { *__errno_location() } == EINTR {
                    continue;
                }
                return true; // let the caller's read report the error
            }
            if fds[1].revents != 0
                && let Some((read_end, _)) = pipe
            {
                let mut buf = [0u8; 64];
                while unsafe { read(read_end, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
            }
            if fds[0].revents != 0 {
                return true;
            }
            if fds[1].revents != 0 || super::timeout(Instant::now()) == Some(Default::default()) {
                return false;
            }
            if n == 0 {
                return false; // the deadline passed
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use unix::{install as install_unix, wait_readable};

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn wakes_and_deadlines() {
        let state = State::new();
        let now = Instant::now();
        assert!(!state.take(now));
        state.wake();
        assert_eq!(state.timeout(now), Some(Duration::ZERO));
        assert!(state.take(now));
        assert!(!state.take(now), "a wake is delivered once");

        state.set_deadline(Some(now + Duration::from_millis(50)));
        assert!(state.timeout(now).is_some_and(|t| t > Duration::ZERO));
        assert!(!state.take(now), "not due yet");
        assert!(state.take(now + Duration::from_millis(60)));
        assert_eq!(state.deadline(), None, "a due deadline is consumed");
    }
}
