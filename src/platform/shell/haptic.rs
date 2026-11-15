//! Haptic feedback: [`haptic`] plays one of a few named taps.
//!
//! | | How |
//! |---|---|
//! | iOS | `UISelectionFeedbackGenerator`, `UIImpactFeedbackGenerator` (light, medium, heavy), `UINotificationFeedbackGenerator` (success, warning, error) |
//! | Android | `View.performHapticFeedback` on the activity's view (no permission needed), following the user's touch-feedback setting |
//! | macOS | `NSHapticFeedbackManager` (felt on a Force Touch trackpad, while touching it) |
//! | Web | `navigator.vibrate`, where the browser has it (Android's Chrome and Firefox; not Safari) |
//! | Windows, Linux | nothing |

/// A kind of haptic tap, named for what it confirms.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Haptic {
    /// A value changed by one step: a picker's detent, a switch flipping.
    Selection,
    /// A light impact: something small snapped into place.
    Light,
    /// A medium impact.
    Medium,
    /// A heavy impact: a long press opening a menu.
    Heavy,
    /// Something succeeded: a code was scanned.
    Success,
    /// Something needs attention.
    Warning,
    /// Something failed.
    Error,
}

/// Play `kind` on the device's haptic engine. Does nothing where there is
/// none (desktops but a Mac's Force Touch trackpad, browsers without
/// `navigator.vibrate`), or when the user turned touch feedback off.
///
/// Call it from the UI thread; it returns at once.
///
/// ```no_run
/// use stipple::platform::{Haptic, haptic};
/// haptic(Haptic::Success);
/// ```
pub fn haptic(kind: Haptic) {
    os_haptic(kind);
}

impl Haptic {
    /// `navigator.vibrate`'s pattern for it: on, off, on… milliseconds.
    #[cfg_attr(not(any(target_arch = "wasm32", test)), allow(dead_code))]
    pub(crate) fn vibration(self) -> &'static [u32] {
        match self {
            Haptic::Selection => &[5],
            Haptic::Light => &[10],
            Haptic::Medium => &[20],
            Haptic::Heavy => &[35],
            Haptic::Success => &[10, 60, 10],
            Haptic::Warning => &[20, 80, 20],
            Haptic::Error => &[30, 60, 30, 60, 30],
        }
    }

    /// The `HapticFeedbackConstants` for it on Android API level `sdk`, the
    /// newer constants where they exist.
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    pub(crate) fn android_constant(self, sdk: i32) -> i32 {
        // LONG_PRESS 0, VIRTUAL_KEY 1, KEYBOARD_TAP 3, CLOCK_TICK 4,
        // CONFIRM 16 and REJECT 17 (API 30), GESTURE_START 12 (API 30).
        const LONG_PRESS: i32 = 0;
        const VIRTUAL_KEY: i32 = 1;
        const KEYBOARD_TAP: i32 = 3;
        const CLOCK_TICK: i32 = 4;
        const CONFIRM: i32 = 16;
        const REJECT: i32 = 17;
        match self {
            Haptic::Selection => CLOCK_TICK,
            Haptic::Light => KEYBOARD_TAP,
            Haptic::Medium => VIRTUAL_KEY,
            Haptic::Heavy => LONG_PRESS,
            Haptic::Success if sdk >= 30 => CONFIRM,
            Haptic::Success => VIRTUAL_KEY,
            Haptic::Warning => LONG_PRESS,
            Haptic::Error if sdk >= 30 => REJECT,
            Haptic::Error => LONG_PRESS,
        }
    }
}

#[cfg(target_os = "ios")]
fn os_haptic(kind: Haptic) {
    super::ios::haptic(kind);
}

#[cfg(target_os = "macos")]
fn os_haptic(kind: Haptic) {
    super::macos::haptic(kind);
}

#[cfg(target_os = "android")]
fn os_haptic(kind: Haptic) {
    crate::platform::backend::android::haptic(kind);
}

#[cfg(target_arch = "wasm32")]
fn os_haptic(kind: Haptic) {
    crate::platform::backend::web::vibrate(kind.vibration());
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_arch = "wasm32"
)))]
fn os_haptic(_kind: Haptic) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_android_constants_wait_for_their_api_level() {
        assert_eq!(Haptic::Success.android_constant(29), 1);
        assert_eq!(Haptic::Success.android_constant(30), 16);
        assert_eq!(Haptic::Error.android_constant(34), 17);
        assert_eq!(Haptic::Selection.android_constant(21), 4);
    }

    #[test]
    fn vibrations_are_short() {
        for k in [Haptic::Selection, Haptic::Heavy, Haptic::Error] {
            assert!(k.vibration().iter().sum::<u32>() < 300);
            assert_eq!(k.vibration().len() % 2, 1);
        }
    }
}
