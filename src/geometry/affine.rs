use crate::geometry::{Point, Vec2};

/// A 2D affine transform stored as six coefficients `[a, b, c, d, e, f]`,
/// row-major, mapping a point `(x, y)` to:
///
/// ```text
/// x' = a·x + c·y + e
/// y' = b·x + d·y + f
/// ```
///
/// This matches the column convention used by SVG/PostScript `matrix(...)`
/// and by `oxideav-core`'s `Transform2D`; `stipple::render` converts between the
/// two at the render boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine([f64; 6]);

impl Default for Affine {
    #[inline]
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    #[inline]
    pub const fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Self([a, b, c, d, e, f])
    }

    #[inline]
    pub const fn translate(v: Vec2) -> Self {
        Self([1.0, 0.0, 0.0, 1.0, v.dx, v.dy])
    }

    #[inline]
    pub const fn scale(sx: f64, sy: f64) -> Self {
        Self([sx, 0.0, 0.0, sy, 0.0, 0.0])
    }

    #[inline]
    pub fn rotate(radians: f64) -> Self {
        let (s, c) = radians.sin_cos();
        Self([c, s, -s, c, 0.0, 0.0])
    }

    #[inline]
    pub const fn as_array(self) -> [f64; 6] {
        self.0
    }

    /// Compose so that `self` is applied **after** `inner`
    /// (`result(p) = self(inner(p))`).
    pub fn then(self, outer: Affine) -> Affine {
        let [a1, b1, c1, d1, e1, f1] = self.0;
        let [a2, b2, c2, d2, e2, f2] = outer.0;
        Affine([
            a1 * a2 + b1 * c2,
            a1 * b2 + b1 * d2,
            c1 * a2 + d1 * c2,
            c1 * b2 + d1 * d2,
            e1 * a2 + f1 * c2 + e2,
            e1 * b2 + f1 * d2 + f2,
        ])
    }

    /// Apply the transform to a point.
    #[inline]
    pub fn apply(self, p: Point) -> Point {
        let [a, b, c, d, e, f] = self.0;
        Point::new(a * p.x + c * p.y + e, b * p.x + d * p.y + f)
    }

    /// The determinant of the linear (2×2) part.
    #[inline]
    pub fn determinant(self) -> f64 {
        let [a, b, c, d, ..] = self.0;
        a * d - b * c
    }

    /// Matrix inverse, or `None` if the transform is singular.
    pub fn inverse(self) -> Option<Affine> {
        let det = self.determinant();
        if det.abs() < f64::EPSILON {
            return None;
        }
        let [a, b, c, d, e, f] = self.0;
        let inv = 1.0 / det;
        Some(Affine([
            d * inv,
            -b * inv,
            -c * inv,
            a * inv,
            (c * f - d * e) * inv,
            (b * e - a * f) * inv,
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_then_scale_order() {
        // Translate by (10, 0), then scale 2x: point (1,0) -> (22, 0).
        let t = Affine::translate(Vec2::new(10.0, 0.0)).then(Affine::scale(2.0, 2.0));
        assert_eq!(t.apply(Point::new(1.0, 0.0)), Point::new(22.0, 0.0));
    }

    #[test]
    fn inverse_roundtrip() {
        let t = Affine::translate(Vec2::new(5.0, -3.0)).then(Affine::scale(2.0, 4.0));
        let p = Point::new(7.0, 9.0);
        let back = t.inverse().unwrap().apply(t.apply(p));
        assert!((back.x - p.x).abs() < 1e-9 && (back.y - p.y).abs() < 1e-9);
    }

    #[test]
    fn singular_has_no_inverse() {
        assert!(Affine::scale(0.0, 1.0).inverse().is_none());
    }
}
