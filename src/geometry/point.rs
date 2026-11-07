use crate::geometry::Vec2;
use core::ops::{Add, Sub};

/// A position in logical-pixel space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const ORIGIN: Self = Self { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Euclidean distance to `other`.
    #[inline]
    pub fn distance_to(self, other: Point) -> f64 {
        (other - self).length()
    }

    /// Linear interpolation toward `other` by `t` (0.0 = self, 1.0 = other).
    #[inline]
    pub fn lerp(self, other: Point, t: f64) -> Point {
        Point::new(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
        )
    }
}

impl Add<Vec2> for Point {
    type Output = Point;
    #[inline]
    fn add(self, rhs: Vec2) -> Point {
        Point::new(self.x + rhs.dx, self.y + rhs.dy)
    }
}

impl Sub<Vec2> for Point {
    type Output = Point;
    #[inline]
    fn sub(self, rhs: Vec2) -> Point {
        Point::new(self.x - rhs.dx, self.y - rhs.dy)
    }
}

/// Subtracting two points yields the displacement between them.
impl Sub for Point {
    type Output = Vec2;
    #[inline]
    fn sub(self, rhs: Point) -> Vec2 {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_vec_algebra() {
        let a = Point::new(1.0, 1.0);
        let b = Point::new(4.0, 5.0);
        assert_eq!(b - a, Vec2::new(3.0, 4.0));
        assert_eq!(a.distance_to(b), 5.0);
        assert_eq!(a + Vec2::new(3.0, 4.0), b);
        assert_eq!(a.lerp(b, 0.5), Point::new(2.5, 3.0));
    }
}
