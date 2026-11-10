//! The OS's reduced-motion setting, and how animations scale with it.
//!
//! Every platform lets people ask for less motion: *Reduce motion* on macOS
//! and iOS, *Show animations in Windows* off, GNOME's *Animations* off (read
//! through the desktop portal), *Remove animations* on Android (its animator
//! duration scale at 0) and `prefers-reduced-motion: reduce` on the web. A
//! running [`App`](crate::App) reads the setting when it starts and again
//! when the OS reports a change.
//!
//! [`motion_scale`] turns it into a factor for durations: 0 when motion is
//! reduced (transitions snap), 1 otherwise, and on Android the system's
//! animator duration scale itself (0.5×, 2×, …). [`motion_duration`] applies
//! it. Anything that animates on its own calls one of them when it starts a
//! transition, so it follows the setting:
//!
//! ```
//! use std::time::Duration;
//! use stipple::anim::{motion_duration, reduced_motion};
//!
//! let fade = motion_duration(Duration::from_millis(150)); // zero: snap
//! // Movement (a slide, a zoom) can become a cross-fade instead.
//! let slide = !reduced_motion();
//! # let _ = (fade, slide);
//! ```
//!
//! [`set_reduced_motion`] overrides the system for the calling thread (the UI
//! thread, which runs the app): an app's own "reduce motion" option, or a
//! test that needs one or the other.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The largest scale taken from the system (Android's developer options go up
/// to 10×).
const MAX_SCALE: f64 = 10.0;

/// The system's scale as `f64` bits; 1.0 until an app reads the setting.
static SYSTEM: AtomicU64 = AtomicU64::new(0x3ff0_0000_0000_0000);

thread_local! {
    static OVERRIDE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Whether motion is reduced: the OS asks for it (or turned animations off),
/// or [`set_reduced_motion`] says so on this thread.
pub fn reduced_motion() -> bool {
    motion_scale() == 0.0
}

/// The factor animation durations are multiplied by: 0 under reduced motion,
/// so transitions snap; otherwise 1, or Android's animator duration scale.
pub fn motion_scale() -> f64 {
    match OVERRIDE.with(Cell::get) {
        Some(true) => 0.0,
        Some(false) => 1.0,
        None => system_motion_scale(),
    }
}

/// `duration` scaled by [`motion_scale`]: zero under reduced motion. Call it
/// when a transition starts.
pub fn motion_duration(duration: Duration) -> Duration {
    let scale = motion_scale();
    if scale == 1.0 {
        duration
    } else {
        duration.mul_f64(scale)
    }
}

/// Override the system's setting on the calling thread: `Some(true)` reduces
/// motion, `Some(false)` animates in full, `None` follows the system again.
/// Call it on the UI thread (before [`App::run`](crate::App::run), or from a
/// handler) for an in-app setting, or in a test.
pub fn set_reduced_motion(reduced: Option<bool>) {
    OVERRIDE.with(|o| o.set(reduced));
}

/// The system's own scale, ignoring [`set_reduced_motion`]: 0 when the OS
/// asks for reduced motion, else 1 (or Android's animator duration scale).
pub fn system_motion_scale() -> f64 {
    f64::from_bits(SYSTEM.load(Ordering::Relaxed))
}

/// Record the system's scale (clamped to 0–10; anything not finite counts as
/// 1). Returns whether it changed.
pub(crate) fn set_system_motion_scale(scale: f64) -> bool {
    let scale = if scale.is_finite() {
        scale.clamp(0.0, MAX_SCALE)
    } else {
        1.0
    };
    SYSTEM.swap(scale.to_bits(), Ordering::Relaxed) != scale.to_bits()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_snaps_or_restores_durations() {
        let d = Duration::from_millis(120);
        set_reduced_motion(Some(true));
        assert!(reduced_motion());
        assert_eq!(motion_scale(), 0.0);
        assert_eq!(motion_duration(d), Duration::ZERO);
        set_reduced_motion(Some(false));
        assert!(!reduced_motion());
        assert_eq!(motion_duration(d), d);
        set_reduced_motion(None);
    }

    #[test]
    fn the_override_is_per_thread() {
        set_reduced_motion(Some(true));
        let other = std::thread::spawn(|| OVERRIDE.with(Cell::get)).join();
        assert_eq!(other.unwrap(), None);
        set_reduced_motion(None);
    }
}
