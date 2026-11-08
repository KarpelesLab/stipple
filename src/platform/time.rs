//! A monotonic clock that also works on the web.
//!
//! On every native target [`Instant`] is `std::time::Instant`. On
//! `wasm32-unknown-unknown`, where `std::time::Instant::now` panics, it is a
//! small stand-in with the same methods, read from the page's
//! `performance.now()` (imported by the web backend). Timers, the frame clock
//! and double-click timing all use this type, so they run on the web too.

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;

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
