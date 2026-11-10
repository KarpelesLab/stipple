//! Animation primitives for Stipple.
//!
//! Everything here is a pure function of time: an [`Animation`] is sampled at
//! a local time in seconds (`sample(t)`, `velocity(t)`, `is_finished(t)`), so
//! views drive it from `Cx::frame_time()` and tests check it at fixed times.
//! See `docs/animation.md`.
//!
//! - Values: anything [`Animatable`] (`f64`, [`Color`](crate::render::Color),
//!   points, sizes, rects, insets, tuples, or your own type).
//! - Curves: [`Easing`] (including CSS [`CubicBezier`]s), [`Tween`],
//!   [`Spring`] (closed-form, with [`SpringParams`] presets), [`Decay`]
//!   (flings), and [`Keyframes`].
//! - Composition: [`AnimationExt`] (`delay`, `repeat`, `alternate`, `then`),
//!   [`Sequence`], tuples as parallel groups, [`stagger`].
//! - Effects: [`shake`], [`pulse`], [`entrance`].
//! - Interruption: [`Animated`] holds a value that retargets, takes gestures,
//!   and hands off between motions keeping its value and velocity.

#![forbid(unsafe_code)]

mod animated;
mod easing;
mod keyframes;
mod spring;
mod timeline;
mod value;

pub use animated::{Animated, Motion};
pub use easing::CubicBezier;
pub use keyframes::{Keyframes, entrance, pulse, shake};
pub use spring::{REST_EPSILON, SpringParams};
pub use timeline::{
    Animation, AnimationExt, BoxedAnimation, Decay, Delay, Repeat, Sequence, Still, stagger,
    stagger_within,
};
pub use value::{Animatable, Vector};

/// The current time on the animation timeline that `Cx::frame_time()`
/// reports, for starting or retargeting animations from an event handler
/// (which has no `Cx`). Zero on targets without a clock.
pub fn now() -> std::time::Duration {
    crate::platform::frame::elapsed().unwrap_or_default()
}

/// Linearly interpolate from `a` to `b` by `t` (unclamped).
#[inline]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Standard easing curves, mapping normalized time `t ∈ [0, 1]` to an eased
/// progress value. Inputs are clamped to `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Easing {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// A CSS `cubic-bezier()` curve; see [`Easing::cubic_bezier`] and the
    /// named curves ([`Easing::EASE`], [`Easing::STANDARD`], ...).
    CubicBezier(CubicBezier),
}

impl Easing {
    /// Apply the curve to a normalized time value.
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t,
            Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
            Easing::CubicBezier(b) => b.apply(t),
        }
    }
}

/// A time-driven interpolation between two values (`f64` unless another
/// [`Animatable`] is given).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween<T = f64> {
    pub from: T,
    pub to: T,
    /// Total duration in seconds.
    pub duration: f64,
    pub easing: Easing,
}

impl<T: Animatable> Tween<T> {
    pub fn new(from: T, to: T, duration: f64, easing: Easing) -> Self {
        Self {
            from,
            to,
            duration,
            easing,
        }
    }

    /// Sample the tween at `elapsed` seconds.
    pub fn sample(&self, elapsed: f64) -> T {
        if self.duration <= 0.0 || elapsed >= self.duration {
            return self.to;
        }
        let t = (elapsed / self.duration).clamp(0.0, 1.0);
        self.from.interpolate(self.to, self.easing.apply(t))
    }

    /// Whether the tween has reached its end at `elapsed` seconds.
    pub fn is_finished(&self, elapsed: f64) -> bool {
        elapsed >= self.duration
    }
}

/// A damped spring toward a target. Good for natural-feeling motion that
/// responds to interruption.
///
/// Use it statefully ([`step`](Spring::step) each frame) or, as an
/// [`Animation`], purely: `sample(t)` is the closed-form position `t` seconds
/// after the state in its fields. Generic over any [`Animatable`] (`f64` by
/// default); the velocity is in the value's [`Vector`] space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring<T: Animatable = f64> {
    pub position: T,
    pub velocity: T::Vector,
    pub target: T,
    /// Stiffness `k`, the natural angular frequency squared. Higher = snappier.
    pub stiffness: f64,
    /// Damping ratio. `1.0` is critically damped (no overshoot).
    pub damping: f64,
}

impl<T: Animatable> Spring<T> {
    pub fn new(position: T, target: T) -> Self {
        Self {
            position,
            velocity: T::Vector::ZERO,
            target,
            stiffness: 120.0,
            damping: 1.0,
        }
    }

    /// Advance the spring by `dt` seconds (exactly, with the closed-form
    /// solution, so any frame interval is stable).
    pub fn step(&mut self, dt: f64) {
        let velocity = Animation::velocity(self, dt);
        self.position = Animation::sample(self, dt);
        self.velocity = velocity;
    }

    /// Whether the spring has effectively settled at its target.
    pub fn is_at_rest(&self) -> bool {
        self.position
            .to_vector()
            .minus(self.target.to_vector())
            .norm()
            < 1e-3
            && self.velocity.norm() < 1e-3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_endpoints_are_fixed() {
        for e in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
        ] {
            assert!((e.apply(0.0) - 0.0).abs() < 1e-9, "{e:?} at 0");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-9, "{e:?} at 1");
        }
        assert!(Easing::EaseIn.apply(0.5) < 0.5); // accelerates from rest
    }

    #[test]
    fn tween_clamps_and_finishes() {
        let tw = Tween::new(0.0, 10.0, 2.0, Easing::Linear);
        assert_eq!(tw.sample(0.0), 0.0);
        assert_eq!(tw.sample(1.0), 5.0);
        assert_eq!(tw.sample(5.0), 10.0);
        assert!(tw.is_finished(2.0));
    }

    #[test]
    fn spring_settles_at_target() {
        let mut s = Spring::new(0.0, 1.0);
        for _ in 0..2000 {
            s.step(1.0 / 240.0);
        }
        assert!(s.is_at_rest(), "spring should settle: pos={}", s.position);
        assert!((s.position - 1.0).abs() < 1e-2);
    }
}
