//! A monotonic clock that also works on the web.
//!
//! On every native target [`Instant`] is `std::time::Instant`. On
//! `wasm32-unknown-unknown`, where `std::time::Instant::now` panics, it is a
//! small stand-in with the same methods, read from the page's
//! `performance.now()` (imported by the web backend). Timers, the frame clock
//! and double-click timing all use this type, so they run on the web too.

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;

/// The crate's clock: what timers, the frame clock (and so animations and
/// transitions), touch velocity, long presses, double clicks and the like
/// read as "now". `Instant::now()` in a build; in the crate's own tests, the
/// calling thread's [`test_clock`], which moves only when a test advances it,
/// so no test depends on how fast the machine runs it.
#[inline]
pub(crate) fn now() -> Instant {
    #[cfg(test)]
    {
        test_clock::now()
    }
    #[cfg(not(test))]
    {
        Instant::now()
    }
}

/// The clock [`now`] reads in the crate's tests: one per thread, starting at a
/// fixed point ([`base`](test_clock::base), shared by every thread, where the
/// frame clock's timeline also starts) and moving only by
/// [`advance`](test_clock::advance). Two events a test feeds in a row are
/// simultaneous however slow the runner, and an animation is exactly as far
/// along as the test made it.
#[cfg(test)]
pub(crate) mod test_clock {
    use super::Instant;
    use std::cell::Cell;
    use std::sync::OnceLock;
    use std::time::Duration;

    thread_local! {
        /// How far this thread's clock is past [`base`].
        static OFFSET: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    }

    /// Where every thread's clock starts.
    pub(crate) fn base() -> Instant {
        static BASE: OnceLock<Instant> = OnceLock::new();
        *BASE.get_or_init(Instant::now)
    }

    /// This thread's time.
    pub(crate) fn now() -> Instant {
        base() + OFFSET.with(Cell::get)
    }

    /// Move this thread's clock forward by `by`.
    pub(crate) fn advance(by: Duration) {
        OFFSET.with(|o| o.set(o.get() + by));
    }

    #[test]
    fn the_clock_moves_only_when_advanced_and_per_thread() {
        let t0 = now();
        assert_eq!(now(), t0, "still");
        advance(Duration::from_millis(16));
        assert_eq!(now() - t0, Duration::from_millis(16));
        let other = std::thread::spawn(now).join().unwrap();
        assert_eq!(other, base(), "another thread starts at the base");
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::Instant;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::ops::{Add, AddAssign, Sub, SubAssign};
    use std::time::Duration;

    /// A point in time, as the time since the page loaded
    /// (`performance.now()`).
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Instant(Duration);

    impl Instant {
        /// The current time.
        pub fn now() -> Self {
            let ms = crate::platform::backend::web::now_ms();
            Self(Duration::from_secs_f64(ms.max(0.0) / 1000.0))
        }

        /// Time since `earlier`, or zero if `earlier` is later.
        pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
            self.0.saturating_sub(earlier.0)
        }

        /// Time since `earlier`, or zero if `earlier` is later (as std does
        /// since Rust 1.60).
        pub fn duration_since(&self, earlier: Instant) -> Duration {
            self.saturating_duration_since(earlier)
        }

        /// Time since `earlier`, or `None` if `earlier` is later.
        pub fn checked_duration_since(&self, earlier: Instant) -> Option<Duration> {
            self.0.checked_sub(earlier.0)
        }

        /// Time since this instant.
        pub fn elapsed(&self) -> Duration {
            Instant::now().saturating_duration_since(*self)
        }

        pub fn checked_add(&self, d: Duration) -> Option<Instant> {
            self.0.checked_add(d).map(Instant)
        }

        pub fn checked_sub(&self, d: Duration) -> Option<Instant> {
            self.0.checked_sub(d).map(Instant)
        }
    }

    impl Add<Duration> for Instant {
        type Output = Instant;
        fn add(self, d: Duration) -> Instant {
            Instant(self.0 + d)
        }
    }

    impl AddAssign<Duration> for Instant {
        fn add_assign(&mut self, d: Duration) {
            self.0 += d;
        }
    }

    impl Sub<Duration> for Instant {
        type Output = Instant;
        fn sub(self, d: Duration) -> Instant {
            Instant(self.0.saturating_sub(d))
        }
    }

    impl SubAssign<Duration> for Instant {
        fn sub_assign(&mut self, d: Duration) {
            self.0 = self.0.saturating_sub(d);
        }
    }

    impl Sub<Instant> for Instant {
        type Output = Duration;
        fn sub(self, other: Instant) -> Duration {
            self.saturating_duration_since(other)
        }
    }
}
