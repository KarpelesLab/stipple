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
//! us) the timer delivers it anyway, so a requested frame always arrives. A
//! source that misses [`MISS_LIMIT`] frames in a row (a display link that
//! doesn't tick, as on a virtual display) is no longer waited for: the timer
//! paces frames at the refresh interval until the source delivers one again.
//!
//! [`stats`] counts where frames came from and what presenting them cost, for
//! diagnostics (`asyncdemo --selftest` prints them).
//!
//! There is one event loop per process, so this state is global, like
//! [`wake`](super::wake).

use crate::platform::time::Instant;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The refresh interval assumed until a backend reports the display's: 60 Hz.
pub const DEFAULT_INTERVAL: Duration = Duration::from_nanos(16_666_667);

/// How many frames in a row a display source may leave to the timer before
/// the timer stops waiting for it.
pub const MISS_LIMIT: u32 = 3;

/// Where frames came from and what they cost, since the process started (see
/// [`stats`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// Frames the display source delivered.
    pub from_display: u64,
    /// Frames the fallback timer delivered (no source, or one that missed).
    pub from_timer: u64,
    /// Frames presented by an [`App`](crate::App), and the time spent
    /// building, rasterizing and handing them to the backend.
    pub presents: u64,
    pub present_time: Duration,
    /// The longest of those.
    pub present_max: Duration,
    /// Times a backend put a presented frame on screen itself, after the
    /// present (macOS's `drawRect:`), and the time that took.
    pub displays: u64,
    pub display_time: Duration,
    pub display_max: Duration,
}

impl FrameStats {
    /// The mean time per present.
    pub fn present_mean(&self) -> Duration {
        mean(self.present_time, self.presents)
    }

    /// The mean time per display.
    pub fn display_mean(&self) -> Duration {
        mean(self.display_time, self.displays)
    }
}

fn mean(total: Duration, n: u64) -> Duration {
    if n == 0 {
        Duration::ZERO
    } else {
        total / u32::try_from(n).unwrap_or(u32::MAX)
    }
}

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
    /// Frames in a row the source was trusted with and the timer delivered.
    misses: u32,
    /// The source missed [`MISS_LIMIT`] in a row: don't wait for it.
    distrusted: bool,
    stats: FrameStats,
}

impl Clock {
    const fn new() -> Self {
        Self {
            wanted: None,
            claimed: false,
            last: None,
            interval: DEFAULT_INTERVAL,
            idle: false,
            misses: 0,
            distrusted: false,
            stats: FrameStats {
                from_display: 0,
                from_timer: 0,
                presents: 0,
                present_time: Duration::ZERO,
                present_max: Duration::ZERO,
                displays: 0,
                display_time: Duration::ZERO,
                display_max: Duration::ZERO,
            },
        }
    }

    fn request(&mut self, now: Instant) {
        self.wanted.get_or_insert(now);
        self.idle = false;
    }

    /// A display source was asked for the outstanding frame: wait for it,
    /// unless it has stopped delivering.
    fn armed(&mut self) {
        if self.wanted.is_some() && !self.distrusted {
            self.claimed = true;
        }
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
        self.stats.from_display += 1;
        // It delivers: wait for it again.
        self.misses = 0;
        self.distrusted = false;
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
                if self.claimed {
                    self.misses += 1;
                    self.distrusted |= self.misses >= MISS_LIMIT;
                }
                self.deliver(now);
                self.stats.from_timer += 1;
                true
            }
            _ => false,
        }
    }
}

static CLOCK: Mutex<Clock> = Mutex::new(Clock::new());
static SOURCE: Mutex<Option<Source>> = Mutex::new(None);
/// Whether frames are off: the app is in the background.
static SUSPENDED: AtomicBool = AtomicBool::new(false);

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
    if suspended() {
        return; // in the background: nothing is on screen to animate
    }
    let source = source();
    let deadline = clock(|c| {
        c.request(now);
        if source.is_some() {
            c.armed();
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

/// Stop (`true`) or allow (`false`) frames, as the app goes to the background
/// and comes back. While suspended, [`request`] asks for nothing, an
/// outstanding frame is dropped and a display source is stopped; animations
/// ask again when the app builds on its return. Call on the UI thread.
pub(crate) fn set_suspended(suspended: bool) {
    if SUSPENDED.swap(suspended, Ordering::AcqRel) == suspended || !suspended {
        return;
    }
    clock(|c| {
        c.wanted = None;
        c.claimed = false;
        c.idle = false;
    });
    if let Some(disarm) = source().and_then(|s| s.disarm) {
        disarm();
    }
}

/// Held by tests that suspend frames, which is process-wide.
#[cfg(test)]
pub(crate) static SUSPEND_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Whether frames are suspended (the app is in the background).
pub fn suspended() -> bool {
    SUSPENDED.load(Ordering::Acquire)
}

/// Where frames came from and what presenting them cost, so far.
pub fn stats() -> FrameStats {
    clock(|c| c.stats)
}

/// Count a frame an app presented, which took `took`.
pub(crate) fn note_present(took: Duration) {
    clock(|c| {
        c.stats.presents += 1;
        c.stats.present_time += took;
        c.stats.present_max = c.stats.present_max.max(took);
    });
}

/// Count a backend putting a frame on screen after the present (macOS's
/// `drawRect:`), which took `took`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn note_display(took: Duration) {
    clock(|c| {
        c.stats.displays += 1;
        c.stats.display_time += took;
        c.stats.display_max = c.stats.display_max.max(took);
    });
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
    fn a_source_that_keeps_missing_is_no_longer_waited_for() {
        let mut c = Clock::new();
        let iv = c.interval;
        let mut t = Instant::now();
        // The source never ticks: the timer delivers after two intervals...
        for _ in 0..MISS_LIMIT {
            c.request(t);
            c.armed();
            assert_eq!(c.deadline(), Some(t + iv * 2), "waiting for the source");
            t += iv * 2;
            assert!(c.take_due(t));
        }
        // ...until it stops waiting: frames come at the refresh interval.
        c.request(t);
        c.armed();
        assert!(!c.claimed);
        assert_eq!(c.deadline(), Some(t + iv), "paced from the last frame");
        assert!(c.take_due(t + iv));
        assert_eq!(c.stats.from_timer, u64::from(MISS_LIMIT) + 1);
        // The source ticks again: it's trusted again.
        c.request(t + iv);
        assert!(c.tick(t + iv + MS));
        assert_eq!(c.stats.from_display, 1);
        c.request(t + iv * 2);
        c.armed();
        assert!(c.claimed, "waiting for the source again");
        // An occasional miss doesn't count against it.
        let mut c = Clock::new();
        for i in 0..10u32 {
            let at = t + iv * (4 * i);
            c.request(at);
            c.armed();
            if i % 2 == 0 {
                assert!(c.take_due(at + iv * 2), "missed");
            } else {
                assert!(c.tick(at + iv));
            }
        }
        assert!(!c.distrusted);
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
