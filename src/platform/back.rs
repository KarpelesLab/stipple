//! Predictive back: telling the system ahead of time whether the app would
//! go back, and the back gesture's progress.
//!
//! Android 13 and later can preview where the back gesture leads (the home
//! screen, the previous activity) while the finger is still down, so they ask
//! *before* the gesture whether the app will take it: an app that has an
//! `OnBackInvokedCallback` registered gets it, one without gets the system's
//! animation and default. [`set_back_enabled`] is that answer; the
//! [`App`](crate::App) gives it from
//! [`App::back_enabled`](crate::App::back_enabled) (and its open menus) after
//! each event. On Android 14 and later the app also hears the gesture's
//! progress, as [`Event::BackProgress`](crate::platform::Event::BackProgress),
//! to animate its own preview. See `docs/window-lifecycle.md`.

use std::sync::atomic::{AtomicU8, Ordering};

/// The back gesture's progress, as Android 14 and later report it
/// (`OnBackAnimationCallback`), for an app that animates what going back
/// will show. [`Event::BackRequested`](crate::platform::Event::BackRequested)
/// follows a gesture that completes; a [`Cancelled`](Self::Cancelled) one
/// goes nowhere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BackGesture {
    /// The gesture began, from `edge` of the screen.
    Started { edge: BackEdge },
    /// The finger moved: `progress` runs from 0 (just started) to 1 (far
    /// enough to go back when it lifts).
    Progressed { progress: f32 },
    /// The user took the gesture back: nothing happens.
    Cancelled,
}

/// The screen edge a back gesture starts from (`BackEvent.getSwipeEdge`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BackEdge {
    #[default]
    Left,
    Right,
    /// Not a swipe: the Back key or button.
    None,
}

impl BackEdge {
    /// `BackEvent.EDGE_LEFT` (0), `EDGE_RIGHT` (1) and `EDGE_NONE` (2).
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn from_android(edge: i32) -> BackEdge {
        match edge {
            0 => BackEdge::Left,
            1 => BackEdge::Right,
            _ => BackEdge::None,
        }
    }
}

/// Not said yet: the backend acts as if the app takes Back, as before
/// predictive back, so raw handlers that never call [`set_back_enabled`]
/// keep getting [`Event::BackRequested`](crate::platform::Event::BackRequested).
const UNKNOWN: u8 = 0;
const NO: u8 = 1;
const YES: u8 = 2;

static ENABLED: AtomicU8 = AtomicU8::new(UNKNOWN);

/// Tell the system whether the app would go back if the user asked now: it
/// has a dialog, a screen or a menu to close. With `false`, Android 13 and
/// later (when the manifest sets `android:enableOnBackInvokedCallback`)
/// leave the back gesture to the system, which can preview its own
/// animation (to the home screen, or the previous activity), and the app
/// doesn't hear it; with `true` it arrives as
/// [`Event::BackRequested`](crate::platform::Event::BackRequested). Before
/// Android 13, and without the manifest flag, every Back reaches the app
/// whatever this says. Cheap to call often: only changes reach the system.
///
/// The [`App`](crate::App) calls this itself (see
/// [`App::back_enabled`](crate::App::back_enabled)); raw backend users that
/// never call it get every Back.
pub fn set_back_enabled(enabled: bool) {
    let new = if enabled { YES } else { NO };
    if ENABLED.swap(new, Ordering::Relaxed) != new {
        #[cfg(target_os = "android")]
        crate::platform::backend::android::back_enabled_changed(enabled);
    }
}

/// Whether the app said it would go back ([`set_back_enabled`]); `true` until
/// it says.
pub fn back_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) != NO
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_map_from_android() {
        assert_eq!(BackEdge::from_android(0), BackEdge::Left);
        assert_eq!(BackEdge::from_android(1), BackEdge::Right);
        assert_eq!(BackEdge::from_android(2), BackEdge::None);
    }
}
