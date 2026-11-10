//! [`Animated`]: a value that moves toward targets, interruptibly.

use std::sync::Arc;
use std::time::Duration;

use super::{
    Animatable, Animation, BoxedAnimation, Decay, Easing, Spring, SpringParams, Still, Vector,
};

/// How an [`Animated`] value moves to a new target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    /// Jump there.
    Instant,
    /// Ease there over `duration` seconds. Interrupting a moving value
    /// blends its current velocity in, so the motion doesn't jolt.
    Tween { duration: f64, easing: Easing },
    /// Spring there, starting with the current velocity.
    Spring(SpringParams),
}

impl Motion {
    /// A tween of `duration` seconds along `easing`.
    pub const fn tween(duration: f64, easing: Easing) -> Self {
        Motion::Tween { duration, easing }
    }
}

impl From<SpringParams> for Motion {
    fn from(p: SpringParams) -> Self {
        Motion::Spring(p)
    }
}

/// A tween that starts at a given velocity: `from → to` along `easing`, plus a
/// correction `c·h(u)` with `h(u) = u(1-u)²` (a Hermite basis function: zero
/// at both ends, slope 1 at the start, slope 0 at the end) sized so the
/// starting velocity is `v0`. The end is the plain tween's.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BlendTween<T: Animatable> {
    from: T,
    to: T,
    duration: f64,
    easing: Easing,
    correction: T::Vector,
}

impl<T: Animatable> BlendTween<T> {
    fn new(from: T, to: T, v0: T::Vector, duration: f64, easing: Easing) -> Self {
        let delta = to.to_vector().minus(from.to_vector());
        // Velocity at u = 0 is (delta·E'(0) + correction) / duration.
        let correction = v0.times(duration).minus(delta.times(easing.slope(0.0)));
        Self {
            from,
            to,
            duration,
            easing,
            correction,
        }
    }
}

impl<T: Animatable> Animation for BlendTween<T> {
    type Value = T;
    fn sample(&self, t: f64) -> T {
        if t >= self.duration {
            return self.to;
        }
        let u = (t / self.duration).max(0.0);
        let h = u * (1.0 - u) * (1.0 - u);
        let a = self.from.to_vector();
        let delta = self.to.to_vector().minus(a);
        T::from_vector(
            a.plus(delta.times(self.easing.apply(u)))
                .plus(self.correction.times(h)),
        )
    }
    fn velocity(&self, t: f64) -> T::Vector {
        if t >= self.duration {
            return T::Vector::ZERO;
        }
        let u = (t / self.duration).max(0.0);
        let dh = (1.0 - u) * (1.0 - 3.0 * u);
        let delta = self.to.to_vector().minus(self.from.to_vector());
        delta
            .times(self.easing.slope(u))
            .plus(self.correction.times(dh))
            .times(1.0 / self.duration)
    }
    fn duration(&self) -> Option<f64> {
        Some(self.duration)
    }
}

/// A finger (or pointer) driving the value directly.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Gesture<T: Animatable> {
    value: T,
    /// When `value` was last set, in seconds on the frame timeline.
    at: f64,
    /// Smoothed velocity of the recent moves.
    velocity: T::Vector,
}

/// Moves older than this before release don't count: the finger had stopped.
const GESTURE_IDLE: f64 = 0.1;
/// Time constant of the gesture velocity's smoothing.
const GESTURE_SMOOTHING: f64 = 0.03;

/// A value that animates toward targets and can be interrupted at any time:
/// a new target, a new animation, or a finger taking hold continues from the
/// current value **and velocity**, so motion never jumps or jolts.
///
/// It's plain data sampled at a time (the frame time, as `Cx::frame_time()`
/// gives it): keep one in your state, change it from event handlers with the
/// frame time of the event's build, and read it in `view` with `Cx::animate`
/// (which also asks for the next frame while it moves).
///
/// ```
/// use std::time::Duration;
/// use stipple::anim::{Animated, SpringParams};
/// let ms = Duration::from_millis;
/// let mut x = Animated::new(0.0);
/// x.spring_to(100.0, SpringParams::SNAPPY, ms(0));
/// let mid = x.value(ms(100));
/// // Retarget mid-flight: it continues from where it is, at the same speed.
/// let speed = x.velocity(ms(100));
/// x.spring_to(-50.0, SpringParams::SNAPPY, ms(100));
/// assert_eq!(x.value(ms(100)), mid);
/// assert!((x.velocity(ms(100)) - speed).abs() < 1e-9);
/// ```
#[derive(Clone)]
pub struct Animated<T: Animatable> {
    anim: BoxedAnimation<T>,
    /// Start of `anim`, in seconds on the frame timeline.
    start: f64,
    target: T,
    gesture: Option<Gesture<T>>,
}

impl<T: Animatable + core::fmt::Debug> core::fmt::Debug for Animated<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Animated")
            .field("target", &self.target)
            .field("start", &self.start)
            .field("dragging", &self.gesture.is_some())
            .finish()
    }
}

impl<T: Animatable + Default> Default for Animated<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

fn secs(now: Duration) -> f64 {
    now.as_secs_f64()
}

impl<T: Animatable> Animated<T> {
    /// A value at rest.
    pub fn new(value: T) -> Self {
        Self {
            anim: Arc::new(Still(value)),
            start: 0.0,
            target: value,
            gesture: None,
        }
    }

    /// The value at `now`.
    pub fn value(&self, now: Duration) -> T {
        match &self.gesture {
            Some(g) => g.value,
            None => self.anim.sample(secs(now) - self.start),
        }
    }

    /// The velocity at `now`, in the value's vector units per second.
    pub fn velocity(&self, now: Duration) -> T::Vector {
        match &self.gesture {
            Some(g) if secs(now) - g.at <= GESTURE_IDLE => g.velocity,
            Some(_) => T::Vector::ZERO,
            None => self.anim.velocity(secs(now) - self.start),
        }
    }

    /// Where the value is heading: the last target, the end of the playing
    /// animation, or the dragged value.
    pub fn target(&self) -> T {
        match &self.gesture {
            Some(g) => g.value,
            None => self.target,
        }
    }

    /// Whether the value is still moving on its own at `now` (and so needs
    /// frames). A dragged value isn't: the gesture moves it.
    pub fn is_animating(&self, now: Duration) -> bool {
        self.gesture.is_none() && !self.anim.is_finished(secs(now) - self.start)
    }

    /// Whether a gesture holds the value (see [`drag_to`](Self::drag_to)).
    pub fn is_dragging(&self) -> bool {
        self.gesture.is_some()
    }

    /// Jump to `value` and stop.
    pub fn set(&mut self, value: T) {
        *self = Self::new(value);
    }

    /// Head for `target` with `motion`, unless already heading there (so a
    /// view can call it in every build with the target it wants). Returns
    /// whether a new animation started.
    pub fn animate_to(&mut self, target: T, motion: Motion, now: Duration) -> bool {
        if self.gesture.is_none() && target == self.target {
            return false;
        }
        self.retarget(target, motion, now);
        true
    }

    /// [`animate_to`](Self::animate_to) with a spring.
    pub fn spring_to(&mut self, target: T, params: SpringParams, now: Duration) -> bool {
        self.animate_to(target, Motion::Spring(params), now)
    }

    /// [`animate_to`](Self::animate_to) with a tween.
    pub fn tween_to(&mut self, target: T, duration: f64, easing: Easing, now: Duration) -> bool {
        self.animate_to(target, Motion::tween(duration, easing), now)
    }

    /// Head for `target` with `motion` from the current value and velocity,
    /// even if that's the current target (restarting a tween's clock). Ends
    /// any gesture, using its velocity.
    pub fn retarget(&mut self, target: T, motion: Motion, now: Duration) {
        let value = self.value(now);
        let velocity = self.velocity(now);
        let moving = self.gesture.is_some() || self.is_animating(now);
        self.start_from(value, velocity, moving, target, motion, now);
    }

    fn start_from(
        &mut self,
        value: T,
        velocity: T::Vector,
        moving: bool,
        target: T,
        motion: Motion,
        now: Duration,
    ) {
        self.gesture = None;
        self.target = target;
        self.start = secs(now);
        // Reduced motion (see `motion_scale`): tweens scale (to nothing when
        // reduced), and springs jump.
        let scale = super::motion_scale();
        let motion = match motion {
            Motion::Tween { duration, easing } => Motion::Tween {
                duration: duration * scale,
                easing,
            },
            Motion::Spring(_) if scale == 0.0 => Motion::Instant,
            other => other,
        };
        self.anim = match motion {
            Motion::Instant => Arc::new(Still(target)),
            Motion::Spring(p) => Arc::new(Spring::with_velocity(value, velocity, target, p)),
            // From rest a tween plays its plain curve; in flight it carries
            // the current velocity in.
            Motion::Tween { duration, easing } if duration > 0.0 => {
                let v0 = if moving {
                    velocity
                } else {
                    target
                        .to_vector()
                        .minus(value.to_vector())
                        .times(easing.slope(0.0) / duration)
                };
                Arc::new(BlendTween::new(value, target, v0, duration, easing))
            }
            Motion::Tween { .. } => Arc::new(Still(target)),
        };
    }

    /// Play `anim` from `now` (its local time 0 is `now`), replacing whatever
    /// was running — a [`shake`](super::shake), a [`pulse`](super::pulse),
    /// keyframes. It starts from its own first value; the target becomes its
    /// end value (its start value if it never ends). Under
    /// [reduced motion](super::reduced_motion) it jumps straight there.
    pub fn play(&mut self, anim: impl Animation<Value = T> + Send + Sync + 'static, now: Duration) {
        self.gesture = None;
        self.target = anim.sample(anim.duration().unwrap_or(0.0));
        self.start = secs(now);
        self.anim = if super::reduced_motion() {
            Arc::new(Still(self.target))
        } else {
            Arc::new(anim)
        };
    }

    /// Let a gesture set the value directly (a drag, a touch scroll): the
    /// value follows exactly, and the gesture's velocity is tracked for the
    /// hand-off on [`release`](Self::release). Grabbing a moving value takes
    /// its current velocity as the starting estimate.
    pub fn drag_to(&mut self, value: T, now: Duration) {
        let t = secs(now);
        let gesture = match self.gesture {
            None => Gesture {
                value,
                at: t,
                velocity: if self.is_animating(now) {
                    self.velocity(now)
                } else {
                    T::Vector::ZERO
                },
            },
            Some(g) => {
                let dt = t - g.at;
                if dt <= 0.0 {
                    Gesture { value, ..g }
                } else {
                    let moved = value.to_vector().minus(g.value.to_vector());
                    let instant = moved.times(1.0 / dt);
                    let previous = if dt > GESTURE_IDLE {
                        T::Vector::ZERO
                    } else {
                        g.velocity
                    };
                    // Exponential smoothing by elapsed time, so the estimate
                    // doesn't depend on how often moves arrive.
                    let w = 1.0 - (-dt / GESTURE_SMOOTHING).exp();
                    Gesture {
                        value,
                        at: t,
                        velocity: previous.lerp(instant, w),
                    }
                }
            }
        };
        self.target = value;
        self.gesture = Some(gesture);
    }

    /// End a gesture and hand off to `motion` toward `target`, carrying the
    /// gesture's velocity (zero if the finger had stopped for 100 ms).
    pub fn release(&mut self, target: T, motion: Motion, now: Duration) {
        self.retarget(target, motion, now);
    }

    /// End a gesture with a velocity the platform measured (a touch fling's),
    /// handing off to `motion` toward `target`.
    pub fn release_with_velocity(
        &mut self,
        target: T,
        velocity: T::Vector,
        motion: Motion,
        now: Duration,
    ) {
        let value = self.value(now);
        self.start_from(value, velocity, true, target, motion, now);
    }

    /// End a gesture (or interrupt the animation) with a friction glide that
    /// keeps the current velocity and decays at `rate` per second (see
    /// [`Decay::NORMAL`]). Returns where it will come to rest. Under
    /// [reduced motion](super::reduced_motion) there's no glide: it stops
    /// where it is.
    pub fn fling(&mut self, rate: f64, now: Duration) -> T {
        if super::reduced_motion() {
            let here = self.value(now);
            self.set(here);
            return here;
        }
        let decay = Decay::new(self.value(now), self.velocity(now), rate);
        let rest = decay.target();
        self.play(decay, now);
        self.target = rest;
        rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Point;
    use crate::render::Color;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    /// Value and velocity are continuous across a retarget at `at`.
    fn assert_continuous<T: Animatable + core::fmt::Debug>(
        before: (T, T::Vector),
        a: &Animated<T>,
        at: Duration,
    ) {
        assert_eq!(a.value(at), before.0);
        let dv = a.velocity(at).minus(before.1).norm();
        assert!(dv < 1e-6, "velocity jumped by {dv}");
    }

    #[test]
    fn reduced_motion_snaps_tweens_springs_flings_and_effects() {
        crate::anim::set_reduced_motion(Some(true));
        let mut x = Animated::new(0.0);
        x.tween_to(10.0, 0.3, Easing::EaseOut, ms(0));
        assert_eq!(x.value(ms(0)), 10.0);
        assert!(!x.is_animating(ms(0)));
        x.spring_to(-5.0, SpringParams::SNAPPY, ms(10));
        assert_eq!(x.value(ms(10)), -5.0);
        assert!(!x.is_animating(ms(10)));
        x.drag_to(0.0, ms(20));
        x.drag_to(50.0, ms(40));
        assert_eq!(
            x.fling(Decay::<f64>::NORMAL, ms(40)),
            50.0,
            "stops where it is"
        );
        assert!(!x.is_animating(ms(40)));
        x.play(crate::anim::shake(8.0, 0.4), ms(50));
        assert!(!x.is_animating(ms(50)));
        crate::anim::set_reduced_motion(None);
    }

    #[test]
    fn spring_retarget_keeps_value_and_velocity() {
        let mut x = Animated::new(0.0);
        assert!(x.spring_to(100.0, SpringParams::SNAPPY, ms(0)));
        assert!(
            !x.spring_to(100.0, SpringParams::SNAPPY, ms(50)),
            "idempotent"
        );
        let before = (x.value(ms(80)), x.velocity(ms(80)));
        assert!(before.1 > 100.0, "moving fast");
        x.spring_to(-100.0, SpringParams::SNAPPY, ms(80));
        assert_continuous(before, &x, ms(80));
        // It still carries forward briefly before turning round.
        assert!(x.value(ms(90)) > before.0);
        assert!(x.is_animating(ms(500)));
        assert!(!x.is_animating(ms(5000)));
        assert_eq!(x.value(ms(5000)), -100.0);
    }

    #[test]
    fn tween_to_spring_handoff_is_smooth() {
        let mut x = Animated::new(Point::ORIGIN);
        x.tween_to(Point::new(200.0, 100.0), 0.5, Easing::EaseInOut, ms(0));
        let before = (x.value(ms(250)), x.velocity(ms(250)));
        x.spring_to(Point::new(0.0, 300.0), SpringParams::SMOOTH, ms(250));
        let after = (x.value(ms(250)), x.velocity(ms(250)));
        assert!((after.0 - before.0).length() < 1e-9);
        assert!(after.1.minus(before.1).norm() < 1e-9);
    }

    #[test]
    fn tween_retarget_in_flight_keeps_velocity() {
        for easing in [Easing::EaseOut, Easing::Linear, Easing::STANDARD] {
            let mut x = Animated::new(0.0);
            x.tween_to(100.0, 1.0, easing, ms(0));
            // From rest it's the plain curve.
            assert!((x.value(ms(300)) - 100.0 * easing.apply(0.3)).abs() < 1e-9);
            let before = (x.value(ms(300)), x.velocity(ms(300)));
            x.tween_to(0.0, 0.5, easing, ms(300));
            assert_continuous(before, &x, ms(300));
            assert_eq!(x.value(ms(800)), 0.0);
            assert!(!x.is_animating(ms(800)));
        }
    }

    #[test]
    fn colors_retarget_smoothly() {
        let mut c = Animated::new(Color::rgb(0, 0, 255));
        c.spring_to(Color::rgb(255, 0, 0), SpringParams::DEFAULT, ms(0));
        let mid = c.value(ms(150));
        c.spring_to(Color::rgb(0, 255, 0), SpringParams::DEFAULT, ms(150));
        assert_eq!(c.value(ms(150)), mid);
        assert_eq!(c.value(ms(10_000)), Color::rgb(0, 255, 0));
    }

    #[test]
    fn drag_then_release_into_spring() {
        let mut x = Animated::new(0.0);
        // A finger moving at 1000 px/s, sampled every 10 ms.
        for i in 0..=10 {
            x.drag_to(i as f64 * 10.0, ms(i * 10));
        }
        assert!(x.is_dragging() && !x.is_animating(ms(100)));
        assert_eq!(x.value(ms(100)), 100.0);
        assert!((x.velocity(ms(100)) - 1000.0).abs() < 50.0);
        x.release(0.0, Motion::Spring(SpringParams::SMOOTH), ms(100));
        assert!(!x.is_dragging());
        assert_eq!(x.value(ms(100)), 100.0);
        assert!(
            (x.velocity(ms(100)) - 1000.0).abs() < 50.0,
            "keeps the fling"
        );
        assert!(x.value(ms(110)) > 100.0, "carries on before springing back");
        assert_eq!(x.value(ms(10_000)), 0.0);
    }

    #[test]
    fn a_resting_finger_releases_without_velocity() {
        let mut x = Animated::new(0.0);
        x.drag_to(0.0, ms(0));
        x.drag_to(50.0, ms(10));
        // Held still for 200 ms, then released.
        x.release(0.0, Motion::Spring(SpringParams::SMOOTH), ms(210));
        assert_eq!(x.velocity(ms(210)), 0.0);
    }

    #[test]
    fn grabbing_mid_flight_takes_its_velocity() {
        let mut x = Animated::new(0.0);
        x.spring_to(100.0, SpringParams::SMOOTH, ms(0));
        let v = x.velocity(ms(100));
        x.drag_to(x.value(ms(100)), ms(100));
        assert_eq!(x.velocity(ms(100)), v);
    }

    #[test]
    fn release_with_velocity_and_fling() {
        let mut x = Animated::new(0.0);
        x.drag_to(10.0, ms(0));
        x.release_with_velocity(0.0, -300.0, Motion::Spring(SpringParams::DEFAULT), ms(0));
        assert_eq!(x.velocity(ms(0)), -300.0);

        let mut y = Animated::new(0.0);
        y.drag_to(0.0, ms(0));
        y.drag_to(20.0, ms(10));
        y.drag_to(40.0, ms(20));
        let rest = y.fling(Decay::<f64>::FAST, ms(20));
        assert!(rest > 40.0);
        assert_eq!(y.target(), rest);
        assert_eq!(y.value(ms(20)), 40.0);
        assert_eq!(y.value(ms(10_000)), rest);
    }

    #[test]
    fn play_and_instant() {
        let mut x = Animated::new(0.0);
        x.play(super::super::shake(10.0, 0.7), ms(1000));
        assert_eq!(x.target(), 0.0);
        assert!((x.value(ms(1100)) - 10.0).abs() < 1e-9);
        assert!(x.is_animating(ms(1600)) && !x.is_animating(ms(1700)));
        x.animate_to(5.0, Motion::Instant, ms(2000));
        assert_eq!(x.value(ms(2000)), 5.0);
        assert!(!x.is_animating(ms(2000)));
    }
}
