//! Animation primitives for Stipple.
//!
//! Small, allocation-free building blocks the runtime drives from the platform
//! frame clock: [`Easing`] curves, scalar [`lerp`]/[`Tween`] interpolation, and
//! a critically-tunable [`Spring`]. Per-frame integration and binding these to
//! widget state lands with the reactive runtime (`stipple::core`).

#![forbid(unsafe_code)]

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
        }
    }
}

/// A time-driven interpolation between two scalar values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween {
    pub from: f64,
    pub to: f64,
    /// Total duration in seconds.
    pub duration: f64,
    pub easing: Easing,
}

impl Tween {
    pub fn new(from: f64, to: f64, duration: f64, easing: Easing) -> Self {
        Self {
            from,
            to,
            duration,
            easing,
        }
    }

    /// Sample the tween at `elapsed` seconds.
    pub fn sample(&self, elapsed: f64) -> f64 {
        if self.duration <= 0.0 {
            return self.to;
        }
        let t = (elapsed / self.duration).clamp(0.0, 1.0);
        lerp(self.from, self.to, self.easing.apply(t))
    }

    /// Whether the tween has reached its end at `elapsed` seconds.
    pub fn is_finished(&self, elapsed: f64) -> bool {
        elapsed >= self.duration
    }
}

/// A damped spring toward a target, integrated with a fixed semi-implicit
/// Euler step. Good for natural-feeling motion that responds to interruption.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub position: f64,
    pub velocity: f64,
    pub target: f64,
    /// Angular frequency (stiffness). Higher = snappier.
    pub stiffness: f64,
    /// Damping ratio. `1.0` is critically damped (no overshoot).
    pub damping: f64,
}

impl Spring {
    pub fn new(position: f64, target: f64) -> Self {
        Self {
            position,
            velocity: 0.0,
            target,
            stiffness: 120.0,
            damping: 1.0,
        }
    }

    /// Advance the spring by `dt` seconds.
    pub fn step(&mut self, dt: f64) {
        let k = self.stiffness;
        let c = 2.0 * self.damping * k.sqrt();
        let force = -k * (self.position - self.target) - c * self.velocity;
        self.velocity += force * dt;
        self.position += self.velocity * dt;
    }

    /// Whether the spring has effectively settled at its target.
    pub fn is_at_rest(&self) -> bool {
        (self.position - self.target).abs() < 1e-3 && self.velocity.abs() < 1e-3
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
