//! The frame clock: delivering [`Event::Wake`](crate::platform::Event::Wake) at
//! the display's refresh, but only while something asks for frames.
//!
//! A view that animates calls [`request`] (through `Cx::request_frame`) each time
//! it builds. The loop then wakes once, at the next refresh, and the app builds
//! again; when nothing asks, no frames come and an idle app uses no CPU.
//!
//! Where the frame comes from depends on the backend:
//!
//! - A *display source* ticks at the refresh: `wl_surface.frame` on Wayland, a
//!   `DwmFlush` thread on Windows, `CVDisplayLink` on macOS, `CADisplayLink` on
//!   iOS, `AChoreographer` on Android, `requestAnimationFrame` on the web. It
//!   calls [`tick`] from whatever thread it runs on.
//! - Otherwise a timer at the refresh [`interval`](set_interval) stands in (X11).
//!
//! A source that has been asked for a frame is trusted to deliver it, but if it
//! doesn't within two intervals (a hidden window, a compositor that skipped
//! us) the timer delivers it anyway, so a requested frame always arrives.
//!
//! There is one event loop per process, so this state is global, like
//! [`wake`](super::wake).

use crate::platform::time::Instant;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The refresh interval assumed until a backend reports the display's: 60 Hz.
pub const DEFAULT_INTERVAL: Duration = Duration::from_nanos(16_666_667);

/// How a display source is started and stopped. Both run on the UI thread.
#[derive(Clone)]
struct Source {
    /// Ask the source for a tick at the next refresh.
    arm: Arc<dyn Fn() + Send + Sync>,
    /// Stop a source that has been ticking with nothing to do.
    disarm: Option<Arc<dyn Fn() + Send + Sync>>,
}

struct Clock {
    /// When a frame was asked for, while one is outstanding.
    wanted: Option<Instant>,
    /// Whether a display source has promised to tick for the outstanding frame.
    claimed: bool,
    /// When the last frame was delivered, to pace the fallback timer.
    last: Option<Instant>,
    interval: Duration,
    /// A source reported ticking idle, so the UI thread should stop it.
    idle: bool,
}

impl Clock {
    const fn new() -> Self {
        Self {
            wanted: None,
            claimed: false,
            last: None,
            interval: DEFAULT_INTERVAL,
            idle: false,
        }
    }

    fn request(&mut self, now: Instant) {
        self.wanted.get_or_insert(now);
        self.idle = false;
    }

    /// When the fallback timer must deliver the outstanding frame.
    fn deadline(&self) -> Option<Instant> {
        let asked = self.wanted?;
        Some(if self.claimed {
            asked + self.interval * 2
        } else {
            match self.last {
                Some(last) => asked.max(last + self.interval),
                None => asked,
            }
        })
    }

    /// A display source ticked. Returns whether a frame was wanted.
    fn tick(&mut self, now: Instant) -> bool {
        if self.wanted.is_none() {
            self.claimed = false;
            return false;
        }
        self.deliver(now);
        true
    }

    fn deliver(&mut self, now: Instant) {
        self.wanted = None;
        self.claimed = false;
        self.last = Some(now);
    }

    /// Whether the fallback timer is due at `now`; if so, the frame is consumed.
    fn take_due(&mut self, now: Instant) -> bool {
        match self.deadline() {
            Some(at) if at <= now => {
                self.deliver(now);
                true
            }
            _ => false,
        }
    }
}

static CLOCK: Mutex<Clock> = Mutex::new(Clock::new());
static SOURCE: Mutex<Option<Source>> = Mutex::new(None);

fn clock<R>(f: impl FnOnce(&mut Clock) -> R) -> R {
    let mut c = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut c)
}

fn source() -> Option<Source> {
    SOURCE.lock().ok().and_then(|s| s.clone())
}

/// Ask for a frame at the display's next refresh. Call it on the UI thread,
/// once per frame, for as long as something animates; frames stop when calls
/// stop. Several calls before the frame arrives ask for one frame.
pub fn request() {
    let Some(now) = now() else {
        return; // no clock on this target, so nothing animates
    };
    let source = source();
    let deadline = clock(|c| {
        c.request(now);
        if source.is_some() {
            c.claimed = true;
        }
        c.deadline()
    });
    if let Some(source) = source {
        (source.arm)();
    }
    if let Some(at) = deadline {
        super::wake::schedule(at);
    }
}

/// Whether a frame has been asked for and not yet delivered.
pub fn wanted() -> bool {
    clock(|c| c.wanted.is_some())
}

/// For a display source: the display is ready for a frame. Safe from any
/// thread. Wakes the loop if a frame was wanted, and returns whether one was.
/// A source that keeps ticking with nothing wanted should call [`idle`].
pub(crate) fn tick() -> bool {
    let Some(now) = now() else {
        return false;
    };
    let wanted = clock(|c| c.tick(now));
    if wanted {
        super::wake::wake();
    }
    wanted
}

/// For a display source that ticks on its own (a display link): it has ticked
/// a few times with no frame wanted, so it should be stopped. Safe from any
/// thread; the stop itself runs on the UI thread, on the next wake.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(crate) fn idle() {
    clock(|c| c.idle = true);
    super::wake::wake();
}

/// For a source that only ticks when asked as it presents (Wayland's frame
/// callback): it has been asked for the outstanding frame.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn claim() {
    clock(|c| {
        if c.wanted.is_some() {
            c.claimed = true;
        }
    });
}

/// Install the running backend's display source: `arm` asks it for a tick at
/// the next refresh, and `disarm`, if given, stops it after it has gone
/// [`idle`]. Both are called on the UI thread.
#[cfg_attr(
    not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_arch = "wasm32"
    )),
    allow(dead_code)
)]
pub(crate) fn install(
    arm: Arc<dyn Fn() + Send + Sync>,
    disarm: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    if let Ok(mut s) = SOURCE.lock() {
        *s = Some(Source { arm, disarm });
    }
}

/// Set the display's refresh interval, which paces the fallback timer.
/// Values outside 2–100 ms (500 Hz to 10 Hz) are ignored as implausible.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "ios")),
    allow(dead_code)
)]
pub(crate) fn set_interval(interval: Duration) {
    if (Duration::from_millis(2)..=Duration::from_millis(100)).contains(&interval) {
        clock(|c| c.interval = interval);
    }
}

/// The display's refresh interval, or [`DEFAULT_INTERVAL`] if not known.
pub fn interval() -> Duration {
    clock(|c| c.interval)
}

/// When the loop must wake to deliver the outstanding frame, if one is wanted.
pub(crate) fn deadline() -> Option<Instant> {
    clock(|c| c.deadline())
}

/// Called by [`wake::take`](super::wake::take) on the UI thread: whether the
/// fallback timer delivers a frame now. Also stops an idle display source.
pub(crate) fn take_due(now: Instant) -> bool {
    let (due, stop) = clock(|c| {
        let due = c.take_due(now);
        let stop = std::mem::take(&mut c.idle) && c.wanted.is_none();
        (due, stop)
    });
    if stop && let Some(disarm) = source().and_then(|s| s.disarm) {
        disarm();
    }
    due
}

/// The current time. Always `Some` now that the web has a clock
/// ([`time::Instant`](super::time::Instant)); kept an `Option` for targets
/// that may lack one.
pub(crate) fn now() -> Option<Instant> {
    Some(Instant::now())
}

/// Time since the frame clock started (the first call), the timeline animations
/// sample. `None` on targets without a clock.
pub fn elapsed() -> Option<Duration> {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let now = now()?;
    Some(now.saturating_duration_since(*EPOCH.get_or_init(|| now)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn timer_paces_frames_at_the_interval() {
        let mut c = Clock::new();
        let t0 = Instant::now();
        assert_eq!(c.deadline(), None, "idle: no frames");
        c.request(t0);
        assert_eq!(c.deadline(), Some(t0), "the first frame comes at once");
        assert!(c.take_due(t0));
        assert_eq!(c.deadline(), None, "delivered, and nothing asked again");

        c.request(t0 + 5 * MS);
        assert_eq!(
            c.deadline(),
            Some(t0 + c.interval),
            "paced from the last frame"
        );
        assert!(!c.take_due(t0 + 10 * MS));
        assert!(c.take_due(t0 + 17 * MS));
    }

    #[test]
    fn a_source_delivers_and_the_timer_backs_it_up() {
        let mut c = Clock::new();
        let t0 = Instant::now();
        c.request(t0);
        c.claimed = true;
        assert_eq!(c.deadline(), Some(t0 + c.interval * 2), "fallback only");
        assert!(c.tick(t0 + 16 * MS));
        assert_eq!(c.deadline(), None);
        assert!(!c.tick(t0 + 33 * MS), "no frame wanted: an idle tick");

        c.request(t0 + 40 * MS);
        c.claimed = true;
        assert!(!c.take_due(t0 + 50 * MS));
        assert!(
            c.take_due(t0 + 80 * MS),
            "the source never came: the timer did"
        );
    }

    #[test]
    fn repeated_requests_ask_for_one_frame() {
        let mut c = Clock::new();
        let t0 = Instant::now();
        c.request(t0);
        c.request(t0 + 3 * MS);
        assert_eq!(c.wanted, Some(t0), "the first request's time stands");
        assert!(c.take_due(t0 + 3 * MS));
        assert!(!c.take_due(t0 + 50 * MS), "one frame");
    }
}
