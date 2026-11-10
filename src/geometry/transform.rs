use crate::geometry::{Affine, Point, Rect, Vec2};

/// A 2D transform of an element and its subtree: translate, rotate and scale
/// about an origin given as a fraction of the element's box.
///
/// The parts are kept apart (rather than as a matrix) so they can be animated
/// one by one — [`Transform::lerp`] interpolates each — and so the origin can
/// follow the element's laid-out box, which isn't known when the view builds.
///
/// Whatever order the builder methods are called in, the parts apply in a
/// fixed order, like CSS's individual `translate`, `rotate` and `scale`
/// properties: a point is first scaled, then rotated, both about the origin,
/// then translated:
///
/// ```text
/// p' = O + translate + R(rotate) · S(scale) · (p − O)
/// ```
///
/// where `O` is the origin resolved against the box. Rotation is in radians,
/// clockwise on screen (y grows downward). Translation is in logical pixels.
///
/// ```
/// use stipple::geometry::Transform;
/// // Grow by 10% and tilt 5° about the center, nudged 4 px down.
/// let t = Transform::IDENTITY.scale(1.1).rotate_degrees(5.0).translate(0.0, 4.0);
/// assert!(!t.is_identity());
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// Offset in logical pixels, applied last.
    pub translate: Vec2,
    /// Horizontal and vertical scale factors about the origin.
    pub scale: Vec2,
    /// Clockwise rotation about the origin, in radians.
    pub rotate: f64,
    /// The fixed point of scale and rotation, as a fraction of the box:
    /// `(0, 0)` is the top-left corner, `(0.5, 0.5)` (the default) the center,
    /// `(1, 1)` the bottom-right corner.
    pub origin: Point,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    /// No transform: the element draws where it was laid out.
    pub const IDENTITY: Self = Self {
        translate: Vec2::new(0.0, 0.0),
        scale: Vec2::new(1.0, 1.0),
        rotate: 0.0,
        origin: Point::new(0.5, 0.5),
    };

    /// Add an offset of `(dx, dy)` logical pixels.
    pub fn translate(mut self, dx: f64, dy: f64) -> Self {
        self.translate = Vec2::new(self.translate.dx + dx, self.translate.dy + dy);
        self
    }

    /// Scale both axes by `s` (multiplying any scale already set).
    pub fn scale(self, s: f64) -> Self {
        self.scale_xy(s, s)
    }

    /// Scale the axes by `sx` and `sy` (multiplying any scale already set).
    pub fn scale_xy(mut self, sx: f64, sy: f64) -> Self {
        self.scale = Vec2::new(self.scale.dx * sx, self.scale.dy * sy);
        self
    }

    /// Rotate clockwise by `radians` (added to any rotation already set).
    pub fn rotate(mut self, radians: f64) -> Self {
        self.rotate += radians;
        self
    }

    /// Rotate clockwise by `degrees` (added to any rotation already set).
    pub fn rotate_degrees(self, degrees: f64) -> Self {
        self.rotate(degrees.to_radians())
    }

    /// Set the origin of scale and rotation, as fractions of the box's width
    /// and height (`(0, 0)` top-left, `(1, 1)` bottom-right).
    pub fn origin(mut self, x: f64, y: f64) -> Self {
        self.origin = Point::new(x, y);
        self
    }

    /// Whether this transform leaves everything where it is (the origin
    /// doesn't matter then).
    pub fn is_identity(&self) -> bool {
        self.translate == Vec2::new(0.0, 0.0)
            && self.scale == Vec2::new(1.0, 1.0)
            && self.rotate == 0.0
    }

    /// The transform at `t` (0 = `a`, 1 = `b`) of the way from `a` to `b`,
    /// interpolating each part on its own — so a rotation from 0 to 2π turns a
    /// full circle instead of standing still.
    pub fn lerp(a: Transform, b: Transform, t: f64) -> Transform {
        let mix = |x: f64, y: f64| x + (y - x) * t;
        Transform {
            translate: Vec2::new(
                mix(a.translate.dx, b.translate.dx),
                mix(a.translate.dy, b.translate.dy),
            ),
            scale: Vec2::new(mix(a.scale.dx, b.scale.dx), mix(a.scale.dy, b.scale.dy)),
            rotate: mix(a.rotate, b.rotate),
            origin: Point::new(mix(a.origin.x, b.origin.x), mix(a.origin.y, b.origin.y)),
        }
    }

    /// The point `origin` names within `bounds`, in the same space as `bounds`.
    pub fn origin_in(&self, bounds: Rect) -> Point {
        Point::new(
            bounds.min_x() + self.origin.x * bounds.width(),
            bounds.min_y() + self.origin.y * bounds.height(),
        )
    }

    /// The matrix this transform is for an element laid out at `bounds`: it
    /// maps the element's (and its subtree's) layout coordinates to where they
    /// are drawn, in the parent's coordinates.
    pub fn to_affine(&self, bounds: Rect) -> Affine {
        let o = self.origin_in(bounds);
        Affine::translate(Vec2::new(-o.x, -o.y))
            .then(Affine::scale(self.scale.dx, self.scale.dy))
            .then(Affine::rotate(self.rotate))
            .then(Affine::translate(Vec2::new(
                o.x + self.translate.dx,
                o.y + self.translate.dy,
            )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
    }

    #[test]
    fn identity_maps_points_to_themselves() {
        let a = Transform::IDENTITY.to_affine(Rect::from_xywh(10.0, 20.0, 30.0, 40.0));
        assert_eq!(a, Affine::IDENTITY);
        assert!(Transform::default().is_identity());
    }

    #[test]
    fn scale_is_about_the_origin() {
        let b = Rect::from_xywh(0.0, 0.0, 100.0, 50.0);
        let a = Transform::IDENTITY.scale(2.0).to_affine(b);
        // The center stays put; corners move away from it.
        assert!(close(
            a.apply(Point::new(50.0, 25.0)),
            Point::new(50.0, 25.0)
        ));
        assert!(close(
            a.apply(Point::new(0.0, 0.0)),
            Point::new(-50.0, -25.0)
        ));
        // About the top-left corner, it stays put instead.
        let a = Transform::IDENTITY.scale(2.0).origin(0.0, 0.0).to_affine(b);
        assert!(close(a.apply(Point::new(0.0, 0.0)), Point::ORIGIN));
        assert!(close(
            a.apply(Point::new(10.0, 5.0)),
            Point::new(20.0, 10.0)
        ));
    }

    #[test]
    fn rotation_is_clockwise_on_screen() {
        let b = Rect::from_xywh(0.0, 0.0, 20.0, 20.0);
        let a = Transform::IDENTITY.rotate_degrees(90.0).to_affine(b);
        // The top-middle point swings to the right-middle.
        assert!(close(
            a.apply(Point::new(10.0, 0.0)),
            Point::new(20.0, 10.0)
        ));
    }

    #[test]
    fn parts_apply_in_a_fixed_order() {
        let b = Rect::from_xywh(0.0, 0.0, 20.0, 20.0);
        let x = Transform::IDENTITY
            .translate(5.0, 0.0)
            .rotate_degrees(90.0)
            .scale(2.0);
        let y = Transform::IDENTITY
            .scale(2.0)
            .rotate_degrees(90.0)
            .translate(5.0, 0.0);
        assert_eq!(x, y);
        // Scale, then rotate, then translate: (20, 10) → (30, 10) → (10, 30) → (15, 30).
        let p = x.to_affine(b).apply(Point::new(20.0, 10.0));
        assert!(close(p, Point::new(15.0, 30.0)), "{p:?}");
    }

    #[test]
    fn lerp_interpolates_each_part() {
        let a = Transform::IDENTITY;
        let b = Transform::IDENTITY
            .translate(10.0, 0.0)
            .scale(3.0)
            .rotate(2.0);
        let m = Transform::lerp(a, b, 0.5);
        assert_eq!(m.translate, Vec2::new(5.0, 0.0));
        assert_eq!(m.scale, Vec2::new(2.0, 2.0));
        assert_eq!(m.rotate, 1.0);
        assert_eq!(Transform::lerp(a, b, 1.0), b);
    }
}
