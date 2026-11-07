use crate::geometry::{Insets, Point, Size, Vec2};

/// An axis-aligned rectangle in logical-pixel space, defined by its top-left
/// `origin` and its `size`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub const ZERO: Self = Self {
        origin: Point::ORIGIN,
        size: Size::ZERO,
    };

    #[inline]
    pub const fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    /// Construct from individual coordinates.
    #[inline]
    pub const fn from_xywh(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            origin: Point::new(x, y),
            size: Size::new(width, height),
        }
    }

    /// Construct from two corner points (in any order).
    #[inline]
    pub fn from_points(a: Point, b: Point) -> Self {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Self::from_xywh(x, y, (a.x - b.x).abs(), (a.y - b.y).abs())
    }

    #[inline]
    pub fn min_x(self) -> f64 {
        self.origin.x
    }
    #[inline]
    pub fn min_y(self) -> f64 {
        self.origin.y
    }
    #[inline]
    pub fn max_x(self) -> f64 {
        self.origin.x + self.size.width
    }
    #[inline]
    pub fn max_y(self) -> f64 {
        self.origin.y + self.size.height
    }

    #[inline]
    pub fn width(self) -> f64 {
        self.size.width
    }
    #[inline]
    pub fn height(self) -> f64 {
        self.size.height
    }

    #[inline]
    pub fn center(self) -> Point {
        Point::new(
            (self.min_x() + self.max_x()) * 0.5,
            (self.min_y() + self.max_y()) * 0.5,
        )
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.size.is_empty()
    }

    /// True if `p` lies within the rectangle (left/top inclusive,
    /// right/bottom exclusive — the standard half-open convention for
    /// pixel coverage and hit-testing).
    #[inline]
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.min_x() && p.x < self.max_x() && p.y >= self.min_y() && p.y < self.max_y()
    }

    /// Translate by a displacement.
    #[inline]
    pub fn translate(self, by: Vec2) -> Rect {
        Rect::new(self.origin + by, self.size)
    }

    /// Shrink inward by `insets` on each edge, clamping to a non-negative size.
    #[inline]
    pub fn inset(self, insets: Insets) -> Rect {
        Rect::new(
            Point::new(self.min_x() + insets.left, self.min_y() + insets.top),
            self.size.deflate(insets),
        )
    }

    /// The overlapping region of two rectangles, or `None` if disjoint.
    pub fn intersection(self, other: Rect) -> Option<Rect> {
        let x0 = self.min_x().max(other.min_x());
        let y0 = self.min_y().max(other.min_y());
        let x1 = self.max_x().min(other.max_x());
        let y1 = self.max_y().min(other.max_y());
        if x1 > x0 && y1 > y0 {
            Some(Rect::from_xywh(x0, y0, x1 - x0, y1 - y0))
        } else {
            None
        }
    }

    /// The smallest rectangle containing both inputs.
    pub fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x0 = self.min_x().min(other.min_x());
        let y0 = self.min_y().min(other.min_y());
        let x1 = self.max_x().max(other.max_x());
        let y1 = self.max_y().max(other.max_y());
        Rect::from_xywh(x0, y0, x1 - x0, y1 - y0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_is_half_open() {
        let r = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Point::new(0.0, 0.0)));
        assert!(r.contains(Point::new(9.999, 9.999)));
        assert!(!r.contains(Point::new(10.0, 5.0)));
    }

    #[test]
    fn intersection_and_union() {
        let a = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        let b = Rect::from_xywh(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersection(b), Some(Rect::from_xywh(5.0, 5.0, 5.0, 5.0)));
        assert_eq!(a.union(b), Rect::from_xywh(0.0, 0.0, 15.0, 15.0));

        let c = Rect::from_xywh(100.0, 100.0, 1.0, 1.0);
        assert_eq!(a.intersection(c), None);
    }

    #[test]
    fn inset_shrinks() {
        let r = Rect::from_xywh(0.0, 0.0, 20.0, 20.0).inset(Insets::uniform(5.0));
        assert_eq!(r, Rect::from_xywh(5.0, 5.0, 10.0, 10.0));
    }
}
