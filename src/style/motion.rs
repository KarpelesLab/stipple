//! Motion tokens: how long the built-in widgets' animations take and along
//! which curves, per theme.
//!
//! Every widget that animates by default (buttons, switches, checkboxes,
//! radios, tabs, dropdowns and menus, toasts, the drawn message box and tree
//! disclosure triangles) reads its motion from [`Theme::motion`]
//! (a [`MotionTokens`]), so a theme sets the feel of the whole UI the way its
//! palette sets the colors. Stipple's own themes share
//! [`MotionTokens::standard`]; the Material 3 themes use
//! [`MotionTokens::material`]; [`Theme::native`] picks the platform's own
//! durations and curves (Fluent on Windows, AppKit on macOS, UIKit on iOS,
//! Material on Android, libadwaita elsewhere).
//!
//! Each token is an [`anim::Motion`](crate::anim::Motion), so every one of
//! them follows the OS's reduced-motion setting: tweens shorten to nothing
//! and springs jump (see `docs/animation.md`).
//!
//! [`Theme::motion`]: crate::style::Theme::motion
//! [`Theme::native`]: crate::style::Theme::native

use crate::anim::{Easing, Motion, SpringParams};

/// The motion of a theme's widgets: what each kind of change animates with.
///
/// Scales and offsets say where an entrance starts from: a popup opening at
/// `popup_scale` and `popup_offset` pixels short of its place grows and
/// slides to its full size and position while it fades in. A scale of 1 and
/// an offset of 0 make it a plain fade, and [`Motion::Instant`] makes it
/// appear at once.
///
/// ```
/// use stipple::anim::Motion;
/// use stipple::style::{MotionTokens, Theme};
/// // The default look, with popups that only fade.
/// let theme = Theme::light().with_motion(MotionTokens {
///     popup_scale: 1.0,
///     popup_offset: 0.0,
///     ..MotionTokens::standard()
/// });
/// # let _ = theme;
/// // Nothing animates (reduced motion does this too, system-wide).
/// let still = Theme::light().with_motion(MotionTokens::instant());
/// assert_eq!(still.motion.toggle, Motion::Instant);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionTokens {
    /// Color changes: hover and press looks, a switch's track, a checkbox's
    /// box, a segmented tab's fill and text.
    pub state: Motion,
    /// How a pressed button eases to [`press_scale`](Self::press_scale) and
    /// back.
    pub press: Motion,
    /// What a pressed button shrinks to (1 for not at all, as on platforms
    /// whose buttons don't).
    pub press_scale: f64,
    /// Toggles: a switch's thumb sliding over, a check mark or a radio dot
    /// appearing or going.
    pub toggle: Motion,
    /// A selection indicator sliding from one tab to another.
    pub indicator: Motion,
    /// Popups opening: a select's list, menus and submenus, a tab bar's list.
    pub popup: Motion,
    /// The scale a popup opens from.
    pub popup_scale: f64,
    /// How far (logical pixels) a popup slides in from, toward its anchor.
    pub popup_offset: f64,
    /// The drawn message box opening.
    pub dialog: Motion,
    /// The scale the message box opens from (above 1 shrinks into place, as
    /// UIKit's and WinUI's alerts do).
    pub dialog_scale: f64,
    /// A toast coming in.
    pub toast_enter: Motion,
    /// A toast going away.
    pub toast_exit: Motion,
    /// How far (logical pixels) a toast slides in from the window's edge.
    pub toast_offset: f64,
    /// The scale a toast comes in from and leaves to.
    pub toast_scale: f64,
    /// A tree's disclosure triangle turning as its node expands or collapses.
    pub disclosure: Motion,
}

impl Default for MotionTokens {
    fn default() -> Self {
        Self::standard()
    }
}

/// A tween of `ms` milliseconds along `easing`.
const fn ms(ms: u32, easing: Easing) -> Motion {
    Motion::tween(ms as f64 / 1000.0, easing)
}

/// Core Animation's default timing function
/// (`CAMediaTimingFunction(name: .default)`): the same curve as CSS `ease`.
const CA_DEFAULT: Easing = Easing::EASE;
/// WinUI's `ControlFastOutSlowInKeySpline` (`0,0 0,1`).
const FLUENT_DECELERATE: Easing = Easing::cubic_bezier(0.0, 0.0, 0.0, 1.0);
/// The accelerating counterpart, for things leaving.
const FLUENT_ACCELERATE: Easing = Easing::cubic_bezier(1.0, 0.0, 1.0, 1.0);
/// Adwaita's `$ease-out-quad`.
const ADW_EASE_OUT_QUAD: Easing = Easing::cubic_bezier(0.25, 0.46, 0.45, 0.94);
/// libadwaita's `ease-out-cubic`.
const ADW_EASE_OUT_CUBIC: Easing = Easing::cubic_bezier(0.33, 1.0, 0.68, 1.0);
/// Jetpack Compose's `FastOutSlowInEasing`.
const FAST_OUT_SLOW_IN: Easing = Easing::cubic_bezier(0.4, 0.0, 0.2, 1.0);
/// Jetpack Compose's `LinearOutSlowInEasing`.
const LINEAR_OUT_SLOW_IN: Easing = Easing::cubic_bezier(0.0, 0.0, 0.2, 1.0);

impl MotionTokens {
    /// Stipple's own motion, shared by [`Theme::light`](super::Theme::light),
    /// [`Theme::dark`](super::Theme::dark) and their variants: quick
    /// ease-out color changes (150 ms, like a CSS transition), a slight press
    /// scale, springy toggles and tab indicators, popups that grow from 96%
    /// as they fade in, and toasts that slide in on a spring.
    pub const fn standard() -> Self {
        Self {
            state: ms(150, Easing::EaseOut),
            press: ms(100, Easing::EaseOut),
            press_scale: 0.97,
            toggle: Motion::Spring(SpringParams::with_response(0.3, 0.8)),
            indicator: Motion::Spring(SpringParams::with_response(0.35, 0.9)),
            popup: ms(150, Easing::CSS_EASE_OUT),
            popup_scale: 0.96,
            popup_offset: 4.0,
            dialog: ms(200, Easing::CSS_EASE_OUT),
            dialog_scale: 0.95,
            toast_enter: Motion::Spring(SpringParams::with_response(0.4, 0.85)),
            toast_exit: ms(150, Easing::EaseIn),
            toast_offset: 24.0,
            toast_scale: 1.0,
            disclosure: ms(150, Easing::EaseOut),
        }
    }

    /// Nothing animates: every change is immediate. (The OS's reduced-motion
    /// setting does this for every theme on its own.)
    pub const fn instant() -> Self {
        Self {
            state: Motion::Instant,
            press: Motion::Instant,
            press_scale: 1.0,
            toggle: Motion::Instant,
            indicator: Motion::Instant,
            popup: Motion::Instant,
            popup_scale: 1.0,
            popup_offset: 0.0,
            dialog: Motion::Instant,
            dialog_scale: 1.0,
            toast_enter: Motion::Instant,
            toast_exit: Motion::Instant,
            toast_offset: 0.0,
            toast_scale: 1.0,
            disclosure: Motion::Instant,
        }
    }

    /// Material 3 (the Material themes, and [`Theme::native`](super::Theme::native)
    /// on Android). From the M3 motion tokens (durations `short1`–`long4`,
    /// `standard` and `emphasized` easing) and Jetpack Compose's Material
    /// components: the dropdown menu opens over 120 ms from 80% scale along
    /// `LinearOutSlowInEasing`; the tab row's indicator slides over 250 ms
    /// along `FastOutSlowInEasing`; snackbars fade in over 150 ms from 80%
    /// scale and out over 75 ms. Buttons don't scale (their state layer
    /// changes).
    pub const fn material() -> Self {
        Self {
            state: ms(100, Easing::STANDARD), // short2
            press: ms(100, Easing::STANDARD), // short2
            press_scale: 1.0,
            toggle: ms(200, Easing::STANDARD),    // short4
            indicator: ms(250, FAST_OUT_SLOW_IN), // Compose TabRow
            popup: ms(120, LINEAR_OUT_SLOW_IN),   // Compose DropdownMenu
            popup_scale: 0.8,
            popup_offset: 0.0,
            dialog: ms(200, Easing::EMPHASIZED_DECELERATE),
            dialog_scale: 0.9,
            toast_enter: ms(150, FAST_OUT_SLOW_IN), // Compose Snackbar
            toast_exit: ms(75, Easing::Linear),
            toast_offset: 0.0,
            toast_scale: 0.8,
            disclosure: ms(200, Easing::STANDARD), // short4
        }
    }

    /// Fluent, as WinUI animates its controls ([`Theme::native`](super::Theme::native)
    /// on Windows). WinUI's shared control durations are
    /// `ControlFasterAnimationDuration` (83 ms, the brush transitions of a
    /// button's hover and press), `ControlFastAnimationDuration` (167 ms) and
    /// `ControlNormalAnimationDuration` (250 ms), along
    /// `ControlFastOutSlowInKeySpline` (`0,0 0,1`). A `ContentDialog` opens
    /// from 105% scale as it fades in over 250 ms; flyouts and menus slide in
    /// a little as they fade. Buttons don't scale.
    pub const fn fluent() -> Self {
        Self {
            state: ms(83, FLUENT_DECELERATE),
            press: ms(83, FLUENT_DECELERATE),
            press_scale: 1.0,
            toggle: ms(167, FLUENT_DECELERATE),
            indicator: ms(250, FLUENT_DECELERATE),
            popup: ms(250, FLUENT_DECELERATE),
            popup_scale: 1.0,
            popup_offset: 8.0,
            dialog: ms(250, FLUENT_DECELERATE),
            dialog_scale: 1.05,
            toast_enter: ms(250, FLUENT_DECELERATE),
            toast_exit: ms(167, FLUENT_ACCELERATE),
            toast_offset: 32.0,
            toast_scale: 1.0,
            disclosure: ms(167, FLUENT_DECELERATE),
        }
    }

    /// AppKit's feel ([`Theme::native`](super::Theme::native) on macOS):
    /// `NSAnimationContext`'s default 0.25 s along Core Animation's default
    /// timing function for toggles, the segmented control and disclosure
    /// triangles; menus appear at once, as `NSMenu`s do; the message box pops
    /// in from a little smaller, roughly like an alert panel's
    /// (`NSWindow.AnimationBehavior.alertPanel`). AppKit's buttons change
    /// their look at once and don't scale; a 100 ms fade keeps the drawn ones
    /// from flashing.
    pub const fn macos() -> Self {
        Self {
            state: ms(100, Easing::EaseOut),
            press: Motion::Instant,
            press_scale: 1.0,
            toggle: ms(250, CA_DEFAULT),
            indicator: ms(250, CA_DEFAULT),
            popup: Motion::Instant,
            popup_scale: 1.0,
            popup_offset: 0.0,
            dialog: Motion::Spring(SpringParams::with_response(0.3, 0.75)),
            dialog_scale: 0.9,
            toast_enter: Motion::Spring(SpringParams::with_response(0.45, 0.9)),
            toast_exit: ms(250, CA_DEFAULT),
            toast_offset: 32.0,
            toast_scale: 1.0,
            disclosure: ms(250, CA_DEFAULT),
        }
    }

    /// UIKit's feel ([`Theme::native`](super::Theme::native) on iOS): springs
    /// with SwiftUI's documented presets (`.snappy`: 0.5 s, bounce 0.15, for
    /// toggles, menus and toasts; `.smooth`: 0.5 s, no bounce, for the
    /// segmented control's thumb), a message box that shrinks into place from
    /// above full size as `UIAlertController`'s alerts do, and highlights that
    /// fade back over 0.2 s. Buttons don't scale.
    pub const fn ios() -> Self {
        let snappy = Motion::Spring(SpringParams::with_response(0.5, 0.85));
        Self {
            state: ms(200, Easing::EaseOut),
            press: Motion::Instant,
            press_scale: 1.0,
            toggle: snappy,
            indicator: Motion::Spring(SpringParams::with_response(0.5, 1.0)),
            popup: snappy,
            popup_scale: 0.9,
            popup_offset: 0.0,
            dialog: Motion::Spring(SpringParams::with_response(0.4, 1.0)),
            dialog_scale: 1.15,
            toast_enter: snappy,
            toast_exit: ms(250, CA_DEFAULT),
            toast_offset: 32.0,
            toast_scale: 1.0,
            disclosure: ms(250, CA_DEFAULT),
        }
    }

    /// GNOME's libadwaita ([`Theme::native`](super::Theme::native) on Linux):
    /// Adwaita's stylesheet gives buttons and switches
    /// `transition: all 200ms $ease-out-quad`; GTK 4's popovers appear at
    /// once; dialogs open on a critically damped spring and toasts slide in
    /// over 300 ms along `ease-out-cubic` (approximations of libadwaita's
    /// `AdwDialog` and `AdwToastOverlay` animations). Buttons don't scale.
    pub const fn adwaita() -> Self {
        Self {
            state: ms(200, ADW_EASE_OUT_QUAD),
            press: ms(200, ADW_EASE_OUT_QUAD),
            press_scale: 1.0,
            toggle: ms(200, ADW_EASE_OUT_QUAD),
            indicator: ms(200, ADW_EASE_OUT_QUAD),
            popup: Motion::Instant,
            popup_scale: 1.0,
            popup_offset: 0.0,
            dialog: Motion::Spring(SpringParams::with_response(0.25, 1.0)),
            dialog_scale: 0.9,
            toast_enter: ms(300, ADW_EASE_OUT_CUBIC),
            toast_exit: ms(200, ADW_EASE_OUT_CUBIC),
            toast_offset: 24.0,
            toast_scale: 1.0,
            disclosure: ms(200, ADW_EASE_OUT_QUAD),
        }
    }

    /// The platform's own motion, as [`Theme::native`](super::Theme::native)
    /// uses it.
    pub const fn native() -> Self {
        #[cfg(target_os = "windows")]
        let m = Self::fluent();
        #[cfg(target_os = "macos")]
        let m = Self::macos();
        #[cfg(target_os = "ios")]
        let m = Self::ios();
        #[cfg(target_os = "android")]
        let m = Self::material();
        #[cfg(not(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "ios",
            target_os = "android"
        )))]
        let m = Self::adwaita();
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every preset's motions end, scales are positive and offsets aren't
    /// negative.
    #[test]
    fn presets_are_sane() {
        for m in [
            MotionTokens::standard(),
            MotionTokens::instant(),
            MotionTokens::material(),
            MotionTokens::fluent(),
            MotionTokens::macos(),
            MotionTokens::ios(),
            MotionTokens::adwaita(),
        ] {
            for motion in [
                m.state,
                m.press,
                m.toggle,
                m.indicator,
                m.popup,
                m.dialog,
                m.toast_enter,
                m.toast_exit,
                m.disclosure,
            ] {
                match motion {
                    Motion::Instant => {}
                    Motion::Tween { duration, .. } => assert!((0.0..=0.6).contains(&duration)),
                    Motion::Spring(p) => assert!(p.stiffness > 0.0 && p.damping > 0.5),
                }
            }
            for scale in [m.press_scale, m.popup_scale, m.dialog_scale, m.toast_scale] {
                assert!(scale > 0.5 && scale < 1.3, "{scale}");
            }
            assert!(m.popup_offset >= 0.0 && m.toast_offset >= 0.0);
        }
    }
}
