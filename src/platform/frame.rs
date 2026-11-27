//! The frame clock: delivering [`Event::Wake`](crate::platform::Event::Wake) at
//! the display's refresh, but only while something asks for frames.
//!
//! A view that animates calls `request` (through `Cx::request_frame`) each time
//! it builds. The loop then wakes once, at the next refresh, and the app builds
//! again; when nothing asks, no frames come and an idle app uses no CPU.
//!
//! Where the frame comes from depends on the backend:
//!
//! - A *display source* ticks at the refresh: `wl_surface.frame` on Wayland, a
//!   `DwmFlush` thread on Windows, `CVDisplayLink` on macOS, `CADisplayLink` on
//!   iOS, `AChoreographer` on Android, `requestAnimationFrame` on the web. It
//!   calls `tick` from whatever thread it runs on.
//! - Otherwise a timer at the refresh [`interval`] stands in (X11).
//!
//! A source that has been asked for a frame is trusted to deliver it, but if it
//! doesn't within two intervals (a hidden window, a compositor that skipped
//! us) the timer delivers it anyway, so a requested frame always arrives. A
//! source that misses [`MISS_LIMIT`] frames in a row (a display link that
//! doesn't tick, as on a virtual display) is no longer waited for: the timer
//! paces frames at the refresh interval until the source delivers one again.
//!
//! [`stats`] counts where frames came from and what presenting them cost, for
//! diagnostics (`asyncdemo --selftest` and `framepacing` print them). The
//! clock also measures how far apart the display source's ticks come while
//! something animates ([`FrameStats::measured_interval`]), which is the
//! refresh the app actually sees; with `STIPPLE_DEBUG_FRAMES` set it logs that,
//! the refresh a backend reports from its display or swap chain, and a line a
//! second about what was presented (an idle app logs nothing).
//!
//! There is one event loop per process, so this state is global, like
//! the loop's wake-up state.

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
#[non_exhaustive]
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
    /// Times the event loop woke to deliver [`Event::Wake`](crate::platform::Event::Wake):
    /// a frame, a timer, or another thread. An idle app adds none.
    pub wakeups: u64,
    /// How far apart the display source's ticks came while frames were
    /// asked for back to back: the refresh as observed, once enough ticks
    /// have been seen (see [`Meter`]).
    pub measured_interval: Option<Duration>,
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
    meter: Meter,
}

/// How many tick-to-tick gaps the [`Meter`] looks at.
const METER_WINDOW: usize = 16;

/// Measures the display source's refresh from the gaps between its ticks.
///
/// Only gaps between ticks that both delivered a frame, the second asked for
/// while the first was being drawn, count: those are what an animating app
/// sees. The measure is the median of the last [`METER_WINDOW`] gaps, so the
/// odd frame that took two refreshes to draw, or a tick that came late or
/// early, doesn't move it.
#[derive(Clone, Copy, Debug)]
struct Meter {
    /// The last display tick that delivered a frame.
    last: Option<Instant>,
    gaps: [Duration; METER_WINDOW],
    /// Gaps recorded so far (saturating at the window), and where the next goes.
    len: usize,
    next: usize,
}

impl Meter {
    const fn new() -> Self {
        Self {
            last: None,
            gaps: [Duration::ZERO; METER_WINDOW],
            len: 0,
            next: 0,
        }
    }

    /// The source ticked at `now` and delivered a frame asked for at `asked`.
    /// Returns the measured interval once the window is full.
    fn tick(&mut self, now: Instant, asked: Instant, interval: Duration) -> Option<Duration> {
        let last = self.last.replace(now);
        // Back to back: the frame was asked for after the last tick (while it
        // was drawn), not after a pause.
        let gap = last
            .filter(|&l| asked >= l)
            .map(|l| now.saturating_duration_since(l))?;
        // Ignore gaps no display has (over 500 Hz, under 10 Hz) and pauses.
        if gap < Duration::from_millis(2) || gap > interval.max(Duration::from_millis(25)) * 4 {
            return None;
        }
        self.gaps[self.next] = gap;
        self.next = (self.next + 1) % METER_WINDOW;
        self.len = (self.len + 1).min(METER_WINDOW);
        self.measured()
    }

    /// A tick or timer frame that wasn't back to back: start the next gap over.
    fn break_run(&mut self) {
        self.last = None;
    }

    fn measured(&self) -> Option<Duration> {
        if self.len < METER_WINDOW {
            return None;
        }
        let mut gaps = self.gaps;
        gaps.sort_unstable();
        Some(gaps[METER_WINDOW / 2])
    }
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
                wakeups: 0,
                measured_interval: None,
            },
            meter: Meter::new(),
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
        let Some(asked) = self.wanted else {
            self.claimed = false;
            self.meter.break_run();
            return false;
        };
        if let Some(measured) = self.meter.tick(now, asked, self.interval) {
            self.stats.measured_interval = Some(measured);
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
                self.meter.break_run();
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
pub(crate) fn request() {
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

/// For tests (holding [`SUSPEND_TEST_LOCK`]), which have no display: the
/// fallback timer delivers the frame asked for, if one was. Each call is a
/// second later than the last, past the clock's pacing, and far past any
/// frame another test thread asked for on its own
/// [test clock](crate::platform::time::test_clock) (they all share this one
/// frame clock).
#[cfg(test)]
pub(crate) fn deliver_in_test() -> bool {
    use std::sync::atomic::AtomicU64;
    static LATER: AtomicU64 = AtomicU64::new(0);
    let later = Duration::from_secs(1_000_000 + LATER.fetch_add(1, Ordering::Relaxed));
    take_due(crate::platform::time::test_clock::base() + later)
}

/// Whether frames are suspended (the app is in the background).
pub(crate) fn suspended() -> bool {
    SUSPENDED.load(Ordering::Acquire)
}

/// Where frames came from and what presenting them cost, so far.
pub fn stats() -> FrameStats {
    clock(|c| c.stats)
}

/// Count a frame an app presented, which took `took`.
pub(crate) fn note_present(took: Duration) {
    let stats = clock(|c| {
        c.stats.presents += 1;
        c.stats.present_time += took;
        c.stats.present_max = c.stats.present_max.max(took);
        c.stats
    });
    if debug_on()
        && let Some(now) = now()
    {
        log_summary(now, stats);
    }
}

/// Count the event loop waking to deliver a wake (from
/// [`wake::take`](super::wake::take)).
pub(crate) fn note_wake() {
    clock(|c| c.stats.wakeups += 1);
}

/// What the display source is, for the log: `"timer"` until a backend names
/// its source.
static SOURCE_NAME: Mutex<&str> = Mutex::new("timer");

/// Name the running backend's display source (`"wl_surface.frame"`,
/// `"Present"`, `"DXGI frame-latency waitable"`…), for the log.
pub(crate) fn name_source(name: &'static str) {
    if let Ok(mut n) = SOURCE_NAME.lock() {
        *n = name;
    }
}

/// The display source's name (see [`name_source`]).
pub fn source_name() -> &'static str {
    SOURCE_NAME.lock().map(|n| *n).unwrap_or("timer")
}

/// Whether `STIPPLE_DEBUG_FRAMES` asks for the frame log.
fn debug_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("STIPPLE_DEBUG_FRAMES").is_some_and(|v| v != "0"))
}

/// A line of the frame log, where the platform shows diagnostics.
fn debug_line(msg: &str) {
    #[cfg(target_os = "android")]
    super::backend::android::log(msg);
    #[cfg(target_arch = "wasm32")]
    super::backend::web::log(msg, false);
    #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
    eprintln!("{msg}");
}

fn hz(interval: Duration) -> f64 {
    1.0 / interval.as_secs_f64().max(1e-9)
}

/// Whether `a` and `b` differ by more than 10%: a change worth logging (a
/// timer-driven vblank jitters by several percent; a real change of
/// refresh is 60 to 120 Hz, 144 to 120, or 60 to 48).
fn differs(a: Duration, b: Duration) -> bool {
    let (a, b) = (a.as_secs_f64(), b.as_secs_f64());
    (a - b).abs() > 0.1 * a.max(b)
}

/// Log the measured refresh when it first settles and when it moves.
fn log_measured(measured: Duration) {
    static LOGGED: Mutex<Option<Duration>> = Mutex::new(None);
    if !debug_on() {
        return;
    }
    let Ok(mut logged) = LOGGED.lock() else {
        return;
    };
    if logged.is_some_and(|l| !differs(l, measured)) {
        return;
    }
    *logged = Some(measured);
    debug_line(&format!(
        "stipple: frame pacing: {} ticks every {:.2} ms ({:.1} Hz), measured",
        source_name(),
        measured.as_secs_f64() * 1000.0,
        hz(measured)
    ));
}

/// Once a second while frames are presented: how many, and from where.
fn log_summary(now: Instant, stats: FrameStats) {
    static WINDOW: Mutex<Option<(Instant, FrameStats)>> = Mutex::new(None);
    let Ok(mut window) = WINDOW.lock() else {
        return;
    };
    let Some((start, before)) = *window else {
        *window = Some((now, stats));
        return;
    };
    let secs = now.saturating_duration_since(start).as_secs_f64();
    if secs > 3.0 {
        // The last present was long ago (the app idled): start a new window.
        *window = Some((now, stats));
        return;
    }
    if secs < 1.0 {
        return;
    }
    *window = Some((now, stats));
    let presents = stats.presents - before.presents;
    debug_line(&format!(
        "stipple: frames: {presents} presents in {secs:.2} s ({:.1}/s): {} from the display \
         ({}), {} from the timer, {} wakeups",
        presents as f64 / secs,
        stats.from_display - before.from_display,
        source_name(),
        stats.from_timer - before.from_timer,
        stats.wakeups - before.wakeups,
    ));
}

/// A backend learned the display's refresh from its display or swap chain
/// (`from`: RandR, Present's UST/MSC, `VK_GOOGLE_display_timing`, a display
/// link): pace the fallback timer by it, and log it when it changes.
#[cfg_attr(
    not(any(
        target_os = "linux",
        target_os = "fullrust",
        target_os = "macos",
        target_os = "ios",
        all(target_os = "android", feature = "vk")
    )),
    allow(dead_code)
)]
pub(crate) fn report_refresh(from: &'static str, interval: Duration) {
    static LOGGED: Mutex<Option<(&str, Duration)>> = Mutex::new(None);
    set_interval(interval);
    if !debug_on() {
        return;
    }
    let Ok(mut logged) = LOGGED.lock() else {
        return;
    };
    if logged.is_some_and(|(f, l)| f == from && !differs(l, interval)) {
        return;
    }
    *logged = Some((from, interval));
    debug_line(&format!(
        "stipple: frame pacing: {from} reports a {:.2} ms refresh ({:.1} Hz)",
        interval.as_secs_f64() * 1000.0,
        hz(interval)
    ));
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
#[cfg_attr(
    not(any(test, target_os = "linux", target_os = "fullrust")),
    allow(dead_code)
)]
pub(crate) fn wanted() -> bool {
    clock(|c| c.wanted.is_some())
}

/// For a display source: the display is ready for a frame. Safe from any
/// thread. Wakes the loop if a frame was wanted, and returns whether one was.
/// A source that keeps ticking with nothing wanted should call [`idle`].
pub(crate) fn tick() -> bool {
    let Some(now) = now() else {
        return false;
    };
    let (wanted, measured) = clock(|c| (c.tick(now), c.stats.measured_interval));
    if wanted {
        super::wake::wake();
        if let Some(measured) = measured {
            log_measured(measured);
        }
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
#[cfg_attr(
    not(any(target_os = "linux", target_os = "fullrust")),
    allow(dead_code)
)]
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

/// Take the display source away (it can no longer tick: the window it
/// watched is gone): the timer paces frames from now on.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "fullrust")),
    allow(dead_code)
)]
pub(crate) fn uninstall() {
    if let Ok(mut s) = SOURCE.lock() {
        *s = None;
    }
    clock(|c| c.claimed = false);
    name_source("timer");
}

/// Set the display's refresh interval, which paces the fallback timer.
/// Values outside 2–100 ms (500 Hz to 10 Hz) are ignored as implausible.
#[cfg_attr(
    not(any(
        target_os = "linux",
        target_os = "fullrust",
        target_os = "macos",
        target_os = "ios"
    )),
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
#[cfg_attr(target_os = "ios", allow(dead_code))] // UIKit runs its own loop
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
    Some(crate::platform::time::now())
}

/// Time since the frame clock started (the first call), the timeline animations
/// sample. `None` on targets without a clock. In the crate's tests, time since
/// the [test clock](crate::platform::time::test_clock)'s base, so the same on
/// every thread.
pub(crate) fn elapsed() -> Option<Duration> {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let now = now()?;
    #[cfg(test)]
    let first = crate::platform::time::test_clock::base();
    #[cfg(not(test))]
    let first = now;
    Some(now.saturating_duration_since(*EPOCH.get_or_init(|| first)))
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

    #[test]
    fn a_requested_frame_comes_from_the_source_tick() {
        let mut c = Clock::new();
        let t0 = Instant::now();
        // Requested, and the source armed for it: the timer only backs it up.
        c.request(t0);
        c.armed();
        assert!(c.claimed);
        assert!(!c.take_due(t0 + 10 * MS), "the timer waits for the source");
        // The callback comes: that's the tick that delivers the frame.
        assert!(c.tick(t0 + 16 * MS));
        assert_eq!(c.wanted, None);
        assert!(!c.claimed);
        assert_eq!((c.stats.from_display, c.stats.from_timer), (1, 0));
        assert!(!c.take_due(t0 + 100 * MS), "delivered once");
    }

    #[test]
    fn an_idle_clock_asks_for_nothing_and_counts_nothing() {
        let mut c = Clock::new();
        let t0 = Instant::now();
        // Nothing requested: no deadline, so the loop sleeps; arming without a
        // request claims nothing.
        c.armed();
        assert!(!c.claimed);
        assert_eq!(c.deadline(), None);
        for i in 0..100 {
            assert!(!c.take_due(t0 + i * 16 * MS));
        }
        // A source that ticks anyway (a display link winding down) delivers
        // nothing.
        assert!(!c.tick(t0 + 5 * MS));
        assert_eq!(c.stats, FrameStats::default());
    }

    #[test]
    fn a_missed_callback_times_out_to_the_timer() {
        let mut c = Clock::new();
        let iv = c.interval;
        let t0 = Instant::now();
        c.request(t0);
        c.armed();
        // The compositor never calls back (the window was hidden): the timer
        // delivers two intervals on, and counts a miss.
        assert!(!c.take_due(t0 + iv * 2 - MS));
        assert!(c.take_due(t0 + iv * 2));
        assert_eq!((c.stats.from_display, c.stats.from_timer), (0, 1));
        assert_eq!(c.misses, 1);
        // The late callback, after the timer delivered, is an idle tick.
        assert!(!c.tick(t0 + iv * 2 + MS));
    }

    #[test]
    fn the_meter_measures_back_to_back_ticks() {
        let iv = Duration::from_micros(16_667);
        let mut m = Meter::new();
        let t0 = Instant::now();
        let mut measured = None;
        for i in 0..=METER_WINDOW as u32 {
            let at = t0 + iv * i;
            // Asked for while the previous frame was drawn.
            let asked = if i == 0 { at } else { at - iv / 2 };
            measured = m.tick(at, asked, iv);
        }
        assert_eq!(measured, Some(iv));
    }

    #[test]
    fn the_meter_ignores_slow_frames_and_pauses() {
        let iv = Duration::from_micros(8_333); // 120 Hz
        let mut m = Meter::new();
        let mut t = Instant::now();
        let mut measured = None;
        for i in 0..40u32 {
            // Every third frame takes two refreshes to draw.
            let gap = if i % 3 == 0 { iv * 2 } else { iv };
            let asked = t + MS;
            t += gap;
            measured = m.tick(t, asked, iv).or(measured);
        }
        assert_eq!(measured, Some(iv), "the median: the refresh");

        // A frame asked for after a pause doesn't count the pause.
        let mut m = Meter::new();
        let t0 = Instant::now();
        assert_eq!(m.tick(t0, t0, iv), None);
        assert_eq!(m.tick(t0 + MS * 500, t0 + MS * 490, iv), None);
        assert_eq!(m.len, 0, "the pause is not a gap");
        // Nor does one asked for before the last tick (a request that
        // waited out a whole frame isn't back to back).
        m.break_run();
        assert_eq!(m.tick(t0 + MS * 600, t0 + MS * 600, iv), None);
        assert_eq!(m.tick(t0 + MS * 610, t0 + MS * 590, iv), None);
        assert_eq!(m.len, 0);
    }

    #[test]
    fn ticks_feed_the_meter_and_timer_frames_break_its_run() {
        let mut c = Clock::new();
        let iv = c.interval;
        let mut t = Instant::now();
        for _ in 0..=METER_WINDOW {
            c.request(t);
            c.armed();
            t += iv;
            assert!(c.tick(t));
        }
        let measured = c.stats.measured_interval.expect("measured");
        assert!(!differs(measured, iv));
        // A timer frame in between breaks the run: the next tick starts over.
        c.request(t);
        assert!(c.take_due(t + iv * 2));
        assert_eq!(c.meter.last, None);
    }

    #[test]
    fn differences_under_ten_percent_are_the_same_refresh() {
        let a = Duration::from_micros(16_667);
        assert!(!differs(a, Duration::from_micros(16_800)));
        assert!(!differs(a, Duration::from_micros(17_450)));
        assert!(
            differs(Duration::from_micros(6_944), Duration::from_micros(8_333)),
            "144 to 120 Hz"
        );
        assert!(differs(a, Duration::from_micros(20_833)), "48 Hz");
        assert!(differs(a, Duration::from_micros(8_333)));
    }
}
