//! Values that can be animated: the [`Animatable`] trait and its [`Vector`]
//! space.
//!
//! An animation needs to blend two values and, to hand motion from one
//! animation to the next (a tween interrupted by a spring, a drag released
//! into a fling), to carry a *velocity* — a difference of values per second.
//! Not every value type can hold a difference (a [`Color`] is four `u8`s), so
//! each animatable type maps to a [`Vector`]: a plain `f64` space where
//! blending, velocities and springs are computed, and back.

use crate::geometry::{Insets, Point, Rect, Size, Vec2};
use crate::render::Color;

/// A real vector space an [`Animatable`] value is animated in: `f64`,
/// `[f64; N]`, and tuples of vectors.
///
/// Velocities are vectors too (units per second).
pub trait Vector: Copy + PartialEq + core::fmt::Debug + Send + Sync + 'static {
    /// The zero vector (no displacement, at rest).
    const ZERO: Self;
    /// `self + other`.
    fn plus(self, other: Self) -> Self;
    /// `self - other`.
    fn minus(self, other: Self) -> Self;
    /// `self * k`.
    fn times(self, k: f64) -> Self;
    /// Squared Euclidean length.
    fn norm_squared(self) -> f64;

    /// Euclidean length.
    fn norm(self) -> f64 {
        self.norm_squared().sqrt()
    }

    /// `self + (other - self) * t`, unclamped.
    fn lerp(self, other: Self, t: f64) -> Self {
        self.plus(other.minus(self).times(t))
    }
}

impl Vector for f64 {
    const ZERO: Self = 0.0;
    #[inline]
    fn plus(self, other: Self) -> Self {
        self + other
    }
    #[inline]
    fn minus(self, other: Self) -> Self {
        self - other
    }
    #[inline]
    fn times(self, k: f64) -> Self {
        self * k
    }
    #[inline]
    fn norm_squared(self) -> f64 {
        self * self
    }
}

impl<const N: usize> Vector for [f64; N] {
    const ZERO: Self = [0.0; N];
    #[inline]
    fn plus(self, other: Self) -> Self {
        core::array::from_fn(|i| self[i] + other[i])
    }
    #[inline]
    fn minus(self, other: Self) -> Self {
        core::array::from_fn(|i| self[i] - other[i])
    }
    #[inline]
    fn times(self, k: f64) -> Self {
        self.map(|c| c * k)
    }
    #[inline]
    fn norm_squared(self) -> f64 {
        self.iter().map(|c| c * c).sum()
    }
}

impl<A: Vector, B: Vector> Vector for (A, B) {
    const ZERO: Self = (A::ZERO, B::ZERO);
    fn plus(self, o: Self) -> Self {
        (self.0.plus(o.0), self.1.plus(o.1))
    }
    fn minus(self, o: Self) -> Self {
        (self.0.minus(o.0), self.1.minus(o.1))
    }
    fn times(self, k: f64) -> Self {
        (self.0.times(k), self.1.times(k))
    }
    fn norm_squared(self) -> f64 {
        self.0.norm_squared() + self.1.norm_squared()
    }
}

impl<A: Vector, B: Vector, C: Vector> Vector for (A, B, C) {
    const ZERO: Self = (A::ZERO, B::ZERO, C::ZERO);
    fn plus(self, o: Self) -> Self {
        (self.0.plus(o.0), self.1.plus(o.1), self.2.plus(o.2))
    }
    fn minus(self, o: Self) -> Self {
        (self.0.minus(o.0), self.1.minus(o.1), self.2.minus(o.2))
    }
    fn times(self, k: f64) -> Self {
        (self.0.times(k), self.1.times(k), self.2.times(k))
    }
    fn norm_squared(self) -> f64 {
        self.0.norm_squared() + self.1.norm_squared() + self.2.norm_squared()
    }
}

/// A value that can be animated: blended between two endpoints, driven by a
/// spring, and given a velocity.
///
/// Implement it for your own types by mapping them to a [`Vector`] (usually
/// `[f64; N]` of their fields) and back. `from_vector` receives values outside
/// the endpoints when a curve overshoots (a bouncy spring, a back-easing
/// bezier) and should clamp whatever can't go there (a color channel, a
/// non-negative size).
///
/// Implemented for `f64` (not `f32`, so float literals infer `f64`), `[f64; N]`, [`Point`], [`Vec2`], [`Size`],
/// [`Rect`], [`Insets`], [`Color`], and pairs and triples of animatable values
/// (to animate several properties as one).
pub trait Animatable: Copy + PartialEq + Send + Sync + 'static {
    /// The space the value is animated in.
    type Vector: Vector;
    /// Map the value into its vector space.
    fn to_vector(self) -> Self::Vector;
    /// Map a vector back to a value (clamping where the type requires).
    fn from_vector(v: Self::Vector) -> Self;

    /// Blend toward `to` by `t` (`0` = `self`, `1` = `to`), in the vector
    /// space. Unclamped: `t` outside `[0, 1]` extrapolates.
    fn interpolate(self, to: Self, t: f64) -> Self {
        Self::from_vector(self.to_vector().lerp(to.to_vector(), t))
    }
}

impl Animatable for f64 {
    type Vector = f64;
    #[inline]
    fn to_vector(self) -> f64 {
        self
    }
    #[inline]
    fn from_vector(v: f64) -> Self {
        v
    }
}

impl<const N: usize> Animatable for [f64; N] {
    type Vector = [f64; N];
    #[inline]
    fn to_vector(self) -> Self {
        self
    }
    #[inline]
    fn from_vector(v: Self) -> Self {
        v
    }
}

impl Animatable for Point {
    type Vector = [f64; 2];
    fn to_vector(self) -> [f64; 2] {
        [self.x, self.y]
    }
    fn from_vector([x, y]: [f64; 2]) -> Self {
        Point::new(x, y)
    }
}

impl Animatable for Vec2 {
    type Vector = [f64; 2];
    fn to_vector(self) -> [f64; 2] {
        [self.dx, self.dy]
    }
    fn from_vector([x, y]: [f64; 2]) -> Self {
        Vec2::new(x, y)
    }
}

/// Sizes don't go negative: an overshooting curve stops at zero.
impl Animatable for Size {
    type Vector = [f64; 2];
    fn to_vector(self) -> [f64; 2] {
        [self.width, self.height]
    }
    fn from_vector([w, h]: [f64; 2]) -> Self {
        Size::new(w.max(0.0), h.max(0.0))
    }
}

/// Origin and size, as `[x, y, width, height]`; the size doesn't go negative.
impl Animatable for Rect {
    type Vector = [f64; 4];
    fn to_vector(self) -> [f64; 4] {
        [
            self.origin.x,
            self.origin.y,
            self.size.width,
            self.size.height,
        ]
    }
    fn from_vector([x, y, w, h]: [f64; 4]) -> Self {
        Rect::from_xywh(x, y, w.max(0.0), h.max(0.0))
    }
}

impl Animatable for Insets {
    type Vector = [f64; 4];
    fn to_vector(self) -> [f64; 4] {
        [self.top, self.right, self.bottom, self.left]
    }
    fn from_vector([top, right, bottom, left]: [f64; 4]) -> Self {
        Insets {
            top,
            right,
            bottom,
            left,
        }
    }
}

/// Colors animate as **premultiplied sRGB**: `[r·a, g·a, b·a, a]` with each
/// channel in `0..=1`, gamma-encoded as stored.
///
/// Premultiplying makes fades to and from transparent keep their hue (red to
/// transparent fades red out, rather than through grey or black); staying in
/// gamma-encoded sRGB matches [`Color::mix`] (which the hover highlights use)
/// for opaque colors, and CSS transitions. Channels are clamped to their range
/// and rounded on the way back, so an overshooting spring can't wrap a channel.
impl Animatable for Color {
    type Vector = [f64; 4];
    fn to_vector(self) -> [f64; 4] {
        let a = self.a as f64 / 255.0;
        let c = |v: u8| v as f64 / 255.0 * a;
        [c(self.r), c(self.g), c(self.b), a]
    }
    fn from_vector([r, g, b, a]: [f64; 4]) -> Self {
        let a = a.clamp(0.0, 1.0);
        let alpha = (a * 255.0).round() as u8;
        if alpha == 0 {
            return Color::TRANSPARENT;
        }
        let c = |v: f64| ((v / a).clamp(0.0, 1.0) * 255.0).round() as u8;
        Color::rgba(c(r), c(g), c(b), alpha)
    }
}

impl<A: Animatable, B: Animatable> Animatable for (A, B) {
    type Vector = (A::Vector, B::Vector);
    fn to_vector(self) -> Self::Vector {
        (self.0.to_vector(), self.1.to_vector())
    }
    fn from_vector(v: Self::Vector) -> Self {
        (A::from_vector(v.0), B::from_vector(v.1))
    }
}

impl<A: Animatable, B: Animatable, C: Animatable> Animatable for (A, B, C) {
    type Vector = (A::Vector, B::Vector, C::Vector);
    fn to_vector(self) -> Self::Vector {
        (self.0.to_vector(), self.1.to_vector(), self.2.to_vector())
    }
    fn from_vector(v: Self::Vector) -> Self {
        (
            A::from_vector(v.0),
            B::from_vector(v.1),
            C::from_vector(v.2),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_round_trip_and_blend_premultiplied() {
        for c in [
            Color::rgb(12, 200, 99),
            Color::rgba(255, 0, 0, 128),
            Color::rgba(1, 2, 3, 1),
            Color::WHITE,
        ] {
            assert_eq!(Color::from_vector(c.to_vector()), c);
        }
        // Opaque colors blend like `Color::mix`.
        let (a, b) = (Color::rgb(0, 100, 200), Color::rgb(255, 50, 0));
        for t in [0.0, 0.25, 0.5, 0.8, 1.0] {
            assert_eq!(a.interpolate(b, t), a.mix(b, t), "t={t}");
        }
        // Fading red out keeps it red.
        let mid = Color::rgb(255, 0, 0).interpolate(Color::TRANSPARENT, 0.5);
        assert_eq!((mid.r, mid.g, mid.b), (255, 0, 0));
        assert!((mid.a as i32 - 128).abs() <= 1);
        // Overshoot clamps rather than wrapping.
        let over = Color::BLACK.interpolate(Color::WHITE, 1.5);
        assert_eq!(over, Color::WHITE);
    }

    #[test]
    fn geometry_interpolates_componentwise() {
        let r = Rect::from_xywh(0.0, 0.0, 10.0, 20.0)
            .interpolate(Rect::from_xywh(10.0, 20.0, 30.0, 40.0), 0.5);
        assert_eq!(r, Rect::from_xywh(5.0, 10.0, 20.0, 30.0));
        let s = Size::new(10.0, 10.0).interpolate(Size::ZERO, 2.0);
        assert_eq!(s, Size::ZERO, "sizes clamp at zero");
        let p = (Point::new(0.0, 0.0), 1.0).interpolate((Point::new(2.0, 4.0), 3.0), 0.5);
        assert_eq!(p, (Point::new(1.0, 2.0), 2.0));
        let i = Insets::uniform(4.0).interpolate(Insets::uniform(8.0), 0.25);
        assert_eq!(i, Insets::uniform(5.0));
    }
}
