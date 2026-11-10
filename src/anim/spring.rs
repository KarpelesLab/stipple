//! Spring parameters and the closed-form damped-spring solution behind
//! [`Spring`].

use super::{Animatable, Animation, Spring, Vector};

/// Below this "amplitude" (displacement from the target, plus velocity over
/// the natural frequency, in the value's vector units) a spring counts as
/// settled. A thousandth of a pixel, or a quarter of a color level.
pub const REST_EPSILON: f64 = 1e-3;

/// How a spring moves: its stiffness and damping ratio.
///
/// Use a preset or [`SpringParams::with_response`], which takes the more
/// intuitive "how long one oscillation takes" in place of a stiffness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringParams {
    /// `k` in `a = -k·x - c·v`: the natural angular frequency squared.
    /// Higher is snappier.
    pub stiffness: f64,
    /// Damping ratio `ζ`: `1.0` is critically damped (fastest without
    /// overshoot), below bounces, above creeps.
    pub damping: f64,
}

impl Default for SpringParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl SpringParams {
    /// What [`Spring::new`] uses: stiffness 120, critically damped.
    pub const DEFAULT: Self = Self::new(120.0, 1.0);
    /// Slow and soft, no overshoot (0.8 s response).
    pub const GENTLE: Self = Self::with_response(0.8, 1.0);
    /// A smooth, unhurried settle, no overshoot (0.5 s response).
    pub const SMOOTH: Self = Self::with_response(0.5, 1.0);
    /// Quick, with a hint of overshoot (0.3 s response, ζ 0.85).
    pub const SNAPPY: Self = Self::with_response(0.3, 0.85);
    /// Visibly bounces before settling (0.5 s response, ζ 0.5).
    pub const BOUNCY: Self = Self::with_response(0.5, 0.5);
    /// Very fast and firm, no overshoot (0.15 s response).
    pub const STIFF: Self = Self::with_response(0.15, 1.0);

    /// A spring with stiffness `k` and damping ratio `ζ`.
    pub const fn new(stiffness: f64, damping: f64) -> Self {
        Self { stiffness, damping }
    }

    /// A spring whose undamped oscillation takes `response` seconds, with
    /// damping ratio `damping` (SwiftUI's and Core Animation's parameters).
    pub const fn with_response(response: f64, damping: f64) -> Self {
        let w = 2.0 * core::f64::consts::PI / response;
        Self::new(w * w, damping)
    }
}

/// Coefficients of the closed-form solution: with displacement `x0` and
/// velocity `v0` at time 0, `x(t) = a·x0 + b·v0` and `v(t) = av·x0 + bv·v0`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Coeffs {
    pub a: f64,
    pub b: f64,
    pub av: f64,
    pub bv: f64,
}

pub(crate) fn coeffs(params: SpringParams, t: f64) -> Coeffs {
    let k = params.stiffness;
    if k.is_nan() || k <= 0.0 || !t.is_finite() {
        // No spring: snap to the target.
        return Coeffs {
            a: 0.0,
            b: 0.0,
            av: 0.0,
            bv: 0.0,
        };
    }
    let w = k.sqrt();
    let z = params.damping.max(0.0);
    if (z - 1.0).abs() < 1e-6 {
        // Critically damped.
        let e = (-w * t).exp();
        Coeffs {
            a: e * (1.0 + w * t),
            b: e * t,
            av: -e * w * w * t,
            bv: e * (1.0 - w * t),
        }
    } else if z < 1.0 {
        // Underdamped: a decaying oscillation.
        let wd = w * (1.0 - z * z).sqrt();
        let e = (-z * w * t).exp();
        let (s, c) = (wd * t).sin_cos();
        Coeffs {
            a: e * (c + z * w / wd * s),
            b: e * s / wd,
            av: -e * w * w / wd * s,
            bv: e * (c - z * w / wd * s),
        }
    } else {
        // Overdamped: two decaying exponentials.
        let r = (z * z - 1.0).sqrt();
        let (r1, r2) = (-w * (z - r), -w * (z + r));
        let (e1, e2) = ((r1 * t).exp(), (r2 * t).exp());
        let d = r1 - r2;
        Coeffs {
            a: (r1 * e2 - r2 * e1) / d,
            b: (e1 - e2) / d,
            av: r1 * r2 * (e2 - e1) / d,
            bv: (r1 * e1 - r2 * e2) / d,
        }
    }
}

impl<T: Animatable> Spring<T> {
    /// A spring at `position` moving at `velocity`, toward `target`.
    pub fn with_velocity(
        position: T,
        velocity: T::Vector,
        target: T,
        params: SpringParams,
    ) -> Self {
        Self {
            position,
            velocity,
            target,
            stiffness: params.stiffness,
            damping: params.damping,
        }
    }

    /// Replace the stiffness and damping.
    pub fn with_params(mut self, params: SpringParams) -> Self {
        self.stiffness = params.stiffness;
        self.damping = params.damping;
        self
    }

    /// The stiffness and damping.
    pub fn params(&self) -> SpringParams {
        SpringParams::new(self.stiffness, self.damping)
    }

    /// Displacement from the target and velocity, `t` seconds on from the
    /// spring's current state (exact, not integrated).
    fn state_at(&self, t: f64) -> (T::Vector, T::Vector) {
        let x0 = self.position.to_vector().minus(self.target.to_vector());
        let c = coeffs(self.params(), t.max(0.0));
        (
            x0.times(c.a).plus(self.velocity.times(c.b)),
            x0.times(c.av).plus(self.velocity.times(c.bv)),
        )
    }

    fn amplitude(&self, x: T::Vector, v: T::Vector) -> f64 {
        // sqrt(x² + v²/k) never grows for a damped spring (it's the energy),
        // so "settled" doesn't flicker on and off.
        (x.norm_squared() + v.norm_squared() / self.stiffness.max(1e-12)).sqrt()
    }

    /// The time from now at which the spring settles (within
    /// [`REST_EPSILON`]), or infinity for an undamped spring.
    pub fn settle_time(&self) -> f64 {
        let settled = |t: f64| {
            let (x, v) = self.state_at(t);
            self.amplitude(x, v) < REST_EPSILON || self.stiffness.is_nan() || self.stiffness <= 0.0
        };
        if settled(0.0) {
            return 0.0;
        }
        if self.damping <= 0.0 {
            return f64::INFINITY;
        }
        let mut hi = 1.0 / self.stiffness.sqrt();
        while !settled(hi) {
            hi *= 2.0;
            if hi > 1e6 {
                return f64::INFINITY;
            }
        }
        let mut lo = 0.0;
        for _ in 0..60 {
            let mid = (lo + hi) / 2.0;
            if settled(mid) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        hi
    }
}

/// A spring sampled as a pure function of time: `t` seconds after the state
/// in its fields. Settled samples return the target exactly.
impl<T: Animatable> Animation for Spring<T> {
    type Value = T;

    fn sample(&self, t: f64) -> T {
        if t <= 0.0 {
            return self.position;
        }
        let (x, v) = self.state_at(t);
        if self.amplitude(x, v) < REST_EPSILON {
            return self.target;
        }
        T::from_vector(self.target.to_vector().plus(x))
    }

    fn velocity(&self, t: f64) -> T::Vector {
        let (x, v) = self.state_at(t);
        if self.amplitude(x, v) < REST_EPSILON {
            return T::Vector::ZERO;
        }
        v
    }

    fn duration(&self) -> Option<f64> {
        Some(self.settle_time()).filter(|d| d.is_finite())
    }

    fn is_finished(&self, t: f64) -> bool {
        let (x, v) = self.state_at(t);
        self.amplitude(x, v) < REST_EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_velocity(p: SpringParams) {
        let s = Spring::with_velocity(3.0, -20.0, 10.0, p);
        let h = 1e-6;
        for t in [0.01, 0.05, 0.2, 0.5] {
            let fd = (s.state_at(t + h).0 - s.state_at(t - h).0) / (2.0 * h);
            assert!((s.state_at(t).1 - fd).abs() < 1e-3, "{p:?} at {t}");
        }
        // Initial conditions hold.
        assert!((s.state_at(0.0).0 - -7.0).abs() < 1e-12);
        assert!((s.state_at(0.0).1 - -20.0).abs() < 1e-12);
    }

    #[test]
    fn closed_form_is_consistent_for_every_damping() {
        for p in [
            SpringParams::DEFAULT,
            SpringParams::BOUNCY,
            SpringParams::SNAPPY,
            SpringParams::new(200.0, 2.0),
        ] {
            check_velocity(p);
            let s = Spring::with_velocity(0.0, 0.0, 1.0, p);
            let d = s.settle_time();
            assert!(d.is_finite() && d > 0.0);
            assert!(s.is_finished(d) && !s.is_finished(d * 0.9));
            assert_eq!(s.sample(d), 1.0);
        }
    }

    #[test]
    fn bouncy_overshoots_and_critical_does_not() {
        let max = |p| {
            let s = Spring::with_velocity(0.0, 0.0, 1.0, p);
            (0..400)
                .map(|i| s.sample(i as f64 / 200.0))
                .fold(0.0, f64::max)
        };
        assert!(max(SpringParams::BOUNCY) > 1.1);
        assert!(max(SpringParams::SMOOTH) <= 1.0 + 1e-9);
    }

    #[test]
    fn response_sets_the_period() {
        let p = SpringParams::with_response(0.5, 1.0);
        let w = p.stiffness.sqrt();
        assert!((2.0 * core::f64::consts::PI / w - 0.5).abs() < 1e-12);
    }
}
