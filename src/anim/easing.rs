//! CSS-style cubic-bezier easing, named curves, and easing slopes.

use super::Easing;

/// A CSS `cubic-bezier(x1, y1, x2, y2)` timing curve: a cubic bezier from
/// `(0, 0)` to `(1, 1)` with control points `(x1, y1)` and `(x2, y2)`.
///
/// `x1` and `x2` are clamped to `[0, 1]` (so the curve is a function of time);
/// `y1` and `y2` may leave it, for curves that overshoot (see
/// [`Easing::BACK_OUT`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct CubicBezier {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

// Bitwise equality, so `Easing` can stay `Eq`: control points are plain
// numbers, never NaN in practice, and bitwise comparison is reflexive.
impl PartialEq for CubicBezier {
    fn eq(&self, o: &Self) -> bool {
        self.x1.to_bits() == o.x1.to_bits()
            && self.y1.to_bits() == o.y1.to_bits()
            && self.x2.to_bits() == o.x2.to_bits()
            && self.y2.to_bits() == o.y2.to_bits()
    }
}
impl Eq for CubicBezier {}

impl core::hash::Hash for CubicBezier {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        for v in [self.x1, self.y1, self.x2, self.y2] {
            v.to_bits().hash(h);
        }
    }
}

const fn clamp01(v: f64) -> f64 {
    if v < 0.0 {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    }
}

impl CubicBezier {
    /// A curve with control points `(x1, y1)` and `(x2, y2)`; `x1` and `x2`
    /// are clamped to `[0, 1]`.
    pub const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Self {
            x1: clamp01(x1),
            y1,
            x2: clamp01(x2),
            y2,
        }
    }

    fn coeffs(a: f64, b: f64) -> (f64, f64, f64) {
        // B(s) = ((ca*s + cb)*s + cc)*s for a bezier 0, a, b, 1.
        let cc = 3.0 * a;
        let cb = 3.0 * (b - a) - cc;
        let ca = 1.0 - cc - cb;
        (ca, cb, cc)
    }

    fn eval((a, b, c): (f64, f64, f64), s: f64) -> f64 {
        ((a * s + b) * s + c) * s
    }

    fn deriv((a, b, c): (f64, f64, f64), s: f64) -> f64 {
        (3.0 * a * s + 2.0 * b) * s + c
    }

    /// The curve parameter `s` at which the curve's x is `x`.
    fn solve_s(&self, x: f64) -> f64 {
        let cx = Self::coeffs(self.x1, self.x2);
        // Newton's method, which converges in a few steps for most curves...
        let mut s = x;
        for _ in 0..8 {
            let err = Self::eval(cx, s) - x;
            if err.abs() < 1e-9 {
                return s;
            }
            let d = Self::deriv(cx, s);
            if d.abs() < 1e-7 {
                break;
            }
            s -= err / d;
        }
        // ...and bisection where it doesn't (x is monotonic in s).
        let (mut lo, mut hi) = (0.0, 1.0);
        s = x;
        for _ in 0..64 {
            let v = Self::eval(cx, s);
            if (v - x).abs() < 1e-9 {
                break;
            }
            if v < x {
                lo = s;
            } else {
                hi = s;
            }
            s = (lo + hi) / 2.0;
        }
        s
    }

    /// Eased progress at normalized time `t` (clamped to `[0, 1]`).
    pub fn apply(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        if t == 0.0 || t == 1.0 {
            return t;
        }
        Self::eval(Self::coeffs(self.y1, self.y2), self.solve_s(t))
    }

    /// The curve's slope `d(progress)/dt` at normalized time `t`.
    pub fn slope(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        let s = self.solve_s(t);
        let dx = Self::deriv(Self::coeffs(self.x1, self.x2), s);
        let dy = Self::deriv(Self::coeffs(self.y1, self.y2), s);
        if dx.abs() > 1e-6 {
            return dy / dx;
        }
        // Vertical tangent in x (x1 or x2 at 0/1): fall back to a difference.
        let h = 1e-5;
        let (a, b) = ((t - h).max(0.0), (t + h).min(1.0));
        (self.apply(b) - self.apply(a)) / (b - a)
    }
}

impl Easing {
    /// CSS `ease`: a gentle start and a long, soft landing.
    pub const EASE: Easing = Easing::cubic_bezier(0.25, 0.1, 0.25, 1.0);
    /// CSS `ease-in`.
    pub const CSS_EASE_IN: Easing = Easing::cubic_bezier(0.42, 0.0, 1.0, 1.0);
    /// CSS `ease-out`.
    pub const CSS_EASE_OUT: Easing = Easing::cubic_bezier(0.0, 0.0, 0.58, 1.0);
    /// CSS `ease-in-out`.
    pub const CSS_EASE_IN_OUT: Easing = Easing::cubic_bezier(0.42, 0.0, 0.58, 1.0);
    /// Material 3 "standard": most on-screen movement.
    pub const STANDARD: Easing = Easing::cubic_bezier(0.2, 0.0, 0.0, 1.0);
    /// Material 3 "emphasized decelerate": things entering the screen.
    pub const EMPHASIZED_DECELERATE: Easing = Easing::cubic_bezier(0.05, 0.7, 0.1, 1.0);
    /// Material 3 "emphasized accelerate": things leaving the screen.
    pub const EMPHASIZED_ACCELERATE: Easing = Easing::cubic_bezier(0.3, 0.0, 0.8, 0.15);
    /// Overshoots the end by about 10% and settles back ("back out").
    pub const BACK_OUT: Easing = Easing::cubic_bezier(0.34, 1.56, 0.64, 1.0);

    /// A CSS `cubic-bezier(x1, y1, x2, y2)` curve; see [`CubicBezier`].
    pub const fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64) -> Easing {
        Easing::CubicBezier(CubicBezier::new(x1, y1, x2, y2))
    }

    /// The curve's slope, `d(apply(t))/dt`, at normalized time `t` (clamped
    /// to `[0, 1]`). Multiplied by `(to - from) / duration` it gives a tween's
    /// velocity, which is what lets a spring take over mid-tween smoothly.
    pub fn slope(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => 1.0,
            Easing::EaseIn => 2.0 * t,
            Easing::EaseOut => 2.0 * (1.0 - t),
            Easing::EaseInOut => {
                if t < 0.5 {
                    4.0 * t
                } else {
                    4.0 * (1.0 - t)
                }
            }
            Easing::CubicBezier(b) => b.slope(t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_matches_reference_points() {
        // Linear control points give the identity.
        let lin = CubicBezier::new(0.0, 0.0, 1.0, 1.0);
        for t in [0.1, 0.33, 0.5, 0.9] {
            assert!((lin.apply(t) - t).abs() < 1e-6);
        }
        // CSS `ease` at 50% of the time is ~80.2% of the way.
        assert!((Easing::EASE.apply(0.5) - 0.8024).abs() < 1e-3);
        // `ease-in-out` is symmetric.
        let e = Easing::CSS_EASE_IN_OUT;
        assert!((e.apply(0.5) - 0.5).abs() < 1e-6);
        assert!((e.apply(0.2) + e.apply(0.8) - 1.0).abs() < 1e-6);
        // Back-out overshoots.
        let peak = (1..100)
            .map(|i| Easing::BACK_OUT.apply(i as f64 / 100.0))
            .fold(0.0, f64::max);
        assert!(peak > 1.05, "{peak}");
        assert_eq!(Easing::BACK_OUT.apply(1.0), 1.0);
    }

    #[test]
    fn slopes_match_finite_differences() {
        let h = 1e-6;
        for e in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
            Easing::EASE,
            Easing::STANDARD,
            Easing::BACK_OUT,
            Easing::EMPHASIZED_ACCELERATE,
        ] {
            for t in [0.1, 0.3, 0.6, 0.9] {
                let fd = (e.apply(t + h) - e.apply(t - h)) / (2.0 * h);
                assert!((e.slope(t) - fd).abs() < 1e-3, "{e:?} at {t}");
            }
        }
        assert_eq!(Easing::EaseOut.slope(1.0), 0.0);
        assert!((Easing::STANDARD.slope(1.0)).abs() < 1e-3);
    }
}
