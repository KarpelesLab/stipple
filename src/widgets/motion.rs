//! How the built-in widgets animate by default, from the theme's
//! [`MotionTokens`]: the bookkeeping that remembers when something appeared,
//! and the looks of things coming in and going out.
//!
//! Most widgets animate through the element transitions of
//! [`Element::transition_for`] (a hover look, a switch's thumb). Entrances
//! need a start time, and a widget's state is what lives across builds, so
//! the stateful widgets keep a [`Memo`] in it: the view only reads the state,
//! and the memo is the part it may write. Something already showing in the
//! widget's first build counts as having always been there, so an app that
//! starts with a list open shows it at rest.

use std::cell::RefCell;
use std::time::Duration;

use crate::anim::{Animated, Motion};
use crate::core::{Cx, Effect, Element, Transition};
use crate::geometry::Transform;
use crate::render::Color;
use crate::style::MotionTokens;

/// Scratch state a widget's view keeps in its (otherwise read-only) state
/// across builds. Never part of the state's value: any two compare equal.
#[derive(Clone, Debug, Default)]
pub(crate) struct Memo<T>(RefCell<T>);

impl<T> Memo<T> {
    pub(crate) fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }
}

impl<T> PartialEq for Memo<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<T> Eq for Memo<T> {}

/// Whether something a widget shows (a popup, say) was showing, and since
/// when.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Shown {
    /// The widget hasn't been built yet.
    #[default]
    Unbuilt,
    /// Built, and not showing.
    Hidden,
    /// Showing since then (`None`: since the widget's first build).
    Since(Option<Duration>),
}

impl Shown {
    /// Note whether the thing is `shown` in the build at `now`, and return
    /// when it appeared: `None` if it isn't showing or was there from the
    /// start (so it doesn't animate in).
    pub(crate) fn track(&mut self, shown: bool, now: Duration) -> Option<Duration> {
        if !shown {
            *self = Shown::Hidden;
            return None;
        }
        match *self {
            Shown::Since(at) => at,
            Shown::Unbuilt => {
                *self = Shown::Since(None);
                None
            }
            Shown::Hidden => {
                let at = start_of(now);
                *self = Shown::Since(Some(at));
                Some(at)
            }
        }
    }
}

/// When a change seen by the build at `now` starts: one frame before, so the
/// frame that shows it is already under way (as element transitions count).
pub(crate) fn start_of(now: Duration) -> Duration {
    now.saturating_sub(crate::platform::frame::interval())
}

/// How far (0 to 1, exactly 1 once over) a motion from 0 to 1 that started
/// at `start` is at `now`, and whether it's still moving. A spring may pass 1
/// on the way.
pub(crate) fn progress_at(motion: Motion, start: Duration, now: Duration) -> (f64, bool) {
    let mut a = Animated::new(0.0);
    a.animate_to(1.0, motion, start);
    if a.is_animating(now) {
        (a.value(now), true)
    } else {
        (1.0, false)
    }
}

/// [`progress_at`] the frame being built, asking for another frame while it
/// moves. `None` (no start) is over.
pub(crate) fn progress<S>(cx: &mut Cx<S>, motion: Motion, start: Option<Duration>) -> f64 {
    let Some(start) = start else {
        return 1.0;
    };
    let (p, moving) = progress_at(motion, start, cx.frame_time());
    if moving {
        cx.request_frame();
    }
    p
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Fade `el` in and grow it from `from_scale`, sliding it `offset` (logical
/// pixels, a vector toward its place) at progress `p`, scaling about
/// `origin` (fractions of its box). At 1 it's untouched.
pub(crate) fn enter(
    el: Element,
    p: f64,
    from_scale: f64,
    offset: (f64, f64),
    origin: (f64, f64),
) -> Element {
    if p >= 1.0 {
        return el;
    }
    let rest = 1.0 - p;
    let t = Transform::IDENTITY
        .scale(lerp(from_scale, 1.0, p))
        .translate(-offset.0 * rest, -offset.1 * rest)
        .origin(origin.0, origin.1);
    let el = if t.is_identity() { el } else { el.transform(t) };
    el.opacity(p.clamp(0.0, 1.0) as f32)
}

/// A popup (a dropdown list, a menu) opening at progress `p`, from the top
/// (`origin` is where it grows from, as fractions of its box; it slides down
/// toward its place).
pub(crate) fn popup(el: Element, m: &MotionTokens, p: f64, origin: (f64, f64)) -> Element {
    enter(el, p, m.popup_scale, (0.0, m.popup_offset), origin)
}

/// The drawn message box opening at progress `p`.
pub(crate) fn dialog(el: Element, m: &MotionTokens, p: f64) -> Element {
    enter(el, p, m.dialog_scale, (0.0, 0.0), (0.5, 0.5))
}

/// A popup's entrance as an [`Effect`], for the popups that keep no state
/// (the core's enter animations know when they appeared): fading in as it
/// grows from the theme's popup scale about `origin` and slides into place.
pub(crate) fn popup_effect(m: &MotionTokens, origin: (f64, f64)) -> Effect {
    let t = Transform::IDENTITY
        .scale(m.popup_scale)
        .translate(0.0, -m.popup_offset)
        .origin(origin.0, origin.1);
    Effect::custom(0.0, t).transition(Transition(m.popup))
}

/// The dialog's entrance as an [`Effect`].
pub(crate) fn dialog_effect(m: &MotionTokens) -> Effect {
    Effect::custom(0.0, Transform::IDENTITY.scale(m.dialog_scale)).transition(Transition(m.dialog))
}

/// A popup closing, as an [`Effect`] for the core's exit animations (which
/// keep it drawn, out of reach of the pointer, the keyboard and screen
/// readers, until it's gone): fading out as it shrinks back about `origin`
/// toward where it opened from.
pub(crate) fn popup_exit(m: &MotionTokens, origin: (f64, f64)) -> Effect {
    popup_effect(m, origin).transition(Transition(m.popup_exit))
}

/// The dialog (and the drawn message box) closing, as an [`Effect`].
pub(crate) fn dialog_exit(m: &MotionTokens) -> Effect {
    dialog_effect(m).transition(Transition(m.dialog_exit))
}

/// `overlay` composited `times` over `base` (`None`: nothing under it, so
/// the overlay itself, `times` as opaque).
pub(crate) fn lit(base: Option<Color>, overlay: Color, times: u8) -> Color {
    let a = overlay.a.saturating_mul(times);
    match base {
        Some(base) => base.mix(overlay.with_alpha(255), f64::from(a) / 255.0),
        None => overlay.with_alpha(a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::Easing;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn shown_counts_from_the_first_build() {
        let mut s = Shown::default();
        let t0 = Duration::from_secs(1);
        assert_eq!(s.track(true, t0), None, "there from the start: at rest");
        assert_eq!(s.track(true, t0 + 100 * MS), None);
        assert_eq!(s.track(false, t0 + 200 * MS), None);
        let t1 = t0 + 300 * MS;
        let at = s.track(true, t1).expect("appeared: animates in");
        assert_eq!(at, start_of(t1));
        assert_eq!(s.track(true, t1 + 50 * MS), Some(at), "same start after");
        // Hidden first, then shown: animates.
        let mut s = Shown::default();
        s.track(false, t0);
        assert!(s.track(true, t1).is_some());
    }

    #[test]
    fn progress_runs_to_exactly_one() {
        let m = Motion::tween(0.2, Easing::Linear);
        let t0 = Duration::from_secs(2);
        assert_eq!(progress_at(m, t0, t0), (0.0, true));
        let (mid, moving) = progress_at(m, t0, t0 + 100 * MS);
        assert!(moving && (mid - 0.5).abs() < 1e-9, "{mid}");
        assert_eq!(progress_at(m, t0, t0 + 200 * MS), (1.0, false));
        assert_eq!(progress_at(Motion::Instant, t0, t0), (1.0, false));
    }

    #[test]
    fn reduced_motion_skips_entrances() {
        crate::anim::set_reduced_motion(Some(true));
        let m = Motion::tween(0.2, Easing::Linear);
        let t0 = Duration::from_secs(2);
        assert_eq!(progress_at(m, t0, t0), (1.0, false));
        crate::anim::set_reduced_motion(None);
    }

    #[test]
    fn an_entrance_leaves_nothing_behind_at_rest() {
        let el = Element::stack(crate::layout::Axis::Vertical, vec![]);
        let done = enter(el.clone(), 1.0, 0.5, (0.0, 10.0), (0.5, 0.0));
        assert_eq!(done.transform, None);
        assert_eq!(done.opacity, 1.0);
        let mid = enter(el, 0.5, 0.5, (0.0, 10.0), (0.5, 0.0));
        let t = mid.transform.expect("moving");
        assert!((t.scale.dx - 0.75).abs() < 1e-9);
        assert!(
            (t.translate.dy + 5.0).abs() < 1e-9,
            "slides down into place"
        );
        assert_eq!(mid.opacity, 0.5);
    }

    #[test]
    fn lit_composites_the_overlay() {
        let ov = Color::rgba(0, 0, 0, 51); // 20%
        let white = Some(Color::WHITE);
        assert_eq!(lit(white, ov, 1), Color::rgb(204, 204, 204));
        assert_eq!(lit(white, ov, 2), Color::rgb(153, 153, 153));
        assert_eq!(lit(None, ov, 2), Color::rgba(0, 0, 0, 102));
    }
}
