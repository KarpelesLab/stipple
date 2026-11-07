use core::ops::{Add, Div, Mul, Neg, Sub};

/// A 2D vector (displacement) in logical pixels.
///
/// Distinct from [`Point`](crate::geometry::Point): a `Vec2` is a difference between
/// points, not a position. Subtracting two points yields a `Vec2`; adding a
/// `Vec2` to a point yields a point.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub dx: f64,
    pub dy: f64,
}

impl Vec2 {
    pub const ZERO: Self = Self { dx: 0.0, dy: 0.0 };

    #[inline]
    pub const fn new(dx: f64, dy: f64) -> Self {
        Self { dx, dy }
    }

    /// Uniform vector with both components set to `v`.
    #[inline]
    pub const fn splat(v: f64) -> Self {
        Self { dx: v, dy: v }
    }

    #[inline]
    pub fn length(self) -> f64 {
        self.length_squared().sqrt()
    }

    #[inline]
    pub fn length_squared(self) -> f64 {
        self.dx * self.dx + self.dy * self.dy
    }

    #[inline]
    pub fn dot(self, other: Self) -> f64 {
        self.dx * other.dx + self.dy * other.dy
    }

    /// Returns the unit vector in the same direction, or [`Vec2::ZERO`] if the
    /// length is zero.
    #[inline]
    pub fn normalized(self) -> Self {
        let len = self.length();
        if len == 0.0 { Self::ZERO } else { self / len }
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.dx + rhs.dx, self.dy + rhs.dy)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.dx - rhs.dx, self.dy - rhs.dy)
    }
}

impl Neg for Vec2 {
    type Output = Vec2;
    #[inline]
    fn neg(self) -> Vec2 {
        Vec2::new(-self.dx, -self.dy)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn mul(self, rhs: f64) -> Vec2 {
        Vec2::new(self.dx * rhs, self.dy * rhs)
    }
}

impl Div<f64> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn div(self, rhs: f64) -> Vec2 {
        Vec2::new(self.dx / rhs, self.dy / rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_and_normalize() {
        let v = Vec2::new(3.0, 4.0);
        assert_eq!(v.length(), 5.0);
        let n = v.normalized();
        assert!((n.length() - 1.0).abs() < 1e-12);
        assert_eq!(Vec2::ZERO.normalized(), Vec2::ZERO);
    }

    #[test]
    fn arithmetic() {
        assert_eq!(
            Vec2::new(1.0, 2.0) + Vec2::new(3.0, 4.0),
            Vec2::new(4.0, 6.0)
        );
        assert_eq!(Vec2::splat(2.0) * 3.0, Vec2::new(6.0, 6.0));
        assert_eq!(-Vec2::new(1.0, -2.0), Vec2::new(-1.0, 2.0));
    }
}
