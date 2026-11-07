//! Bridges between Stipple's logical-pixel geometry (`f64`) and
//! `oxideav-core`'s scene-graph geometry (`f32`).
//!
//! Keeping these conversions in one place means the rest of Stipple never
//! depends on oxideav's coordinate types directly — only `stipple::render` does.

use crate::geometry::{Affine, Point};
use oxideav_core::{Point as OxPoint, Transform2D};

/// Convert a Stipple point to an oxideav scene point (`f64` → `f32`).
#[inline]
pub fn to_ox_point(p: Point) -> OxPoint {
    OxPoint {
        x: p.x as f32,
        y: p.y as f32,
    }
}

/// Convert a Stipple affine transform to an oxideav `Transform2D`.
///
/// Both use the same `[a, b, c, d, e, f]` column convention, so this is a
/// component-wise `f64` → `f32` narrowing.
#[inline]
pub fn to_transform2d(a: Affine) -> Transform2D {
    let [aa, b, c, d, e, f] = a.as_array();
    Transform2D {
        a: aa as f32,
        b: b as f32,
        c: c as f32,
        d: d as f32,
        e: e as f32,
        f: f as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Vec2;

    #[test]
    fn transform_components_map_across() {
        let t = to_transform2d(Affine::translate(Vec2::new(3.0, 4.0)));
        assert_eq!((t.e, t.f), (3.0, 4.0));
        assert_eq!((t.a, t.d), (1.0, 1.0));
    }

    #[test]
    fn point_narrows() {
        let p = to_ox_point(Point::new(1.5, 2.5));
        assert_eq!((p.x, p.y), (1.5, 2.5));
    }
}
