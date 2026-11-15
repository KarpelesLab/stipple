//! A 2D camera: the transform from world to screen.

use crate::geometry::{Affine, Point, Rect, Size, Vec2};
use crate::render::Scene;

/// A camera over a 2D world: which world point is at the center of the
/// screen ([`position`](Self::position)), how far it's zoomed in, an optional
/// rotation, and shake. Its [`transform`](Self::transform) maps world
/// coordinates to the screen (a canvas's coordinates); draw the world inside
/// [`push`](Self::push) / [`pop`](Self::pop), or through
/// [`draw`](Self::draw).
///
/// ```
/// use stipple::game::Camera2D;
/// use stipple::geometry::{Point, Size};
///
/// let cam = Camera2D::new(Size::new(320.0, 180.0))
///     .at(Point::new(1000.0, 500.0))
///     .with_zoom(2.0);
/// // The followed point is at the screen's center…
/// assert_eq!(cam.world_to_screen(Point::new(1000.0, 500.0)), Point::new(160.0, 90.0));
/// // …and the screen's corner is half a screen away at 2× zoom.
/// assert_eq!(cam.screen_to_world(Point::new(0.0, 0.0)), Point::new(920.0, 455.0));
/// ```
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct Camera2D {
    /// The world point at the center of the screen.
    pub position: Point,
    /// Screen units per world unit (2 shows everything twice as big).
    pub zoom: f64,
    /// The camera's turn in radians (the world turns the other way on
    /// screen).
    pub rotation: f64,
    /// The size of the screen area the camera fills (a canvas's size).
    pub viewport: Size,
    /// Round the final translation to whole screen units, so pixel art
    /// doesn't shimmer as the camera moves by fractions (on by default).
    pub snap: bool,
    /// The largest shake offset, in screen units, at full trauma.
    pub shake_strength: f64,
    /// Trauma lost per second.
    pub shake_decay: f64,
    trauma: f64,
    time: f64,
}

impl Camera2D {
    /// A camera filling `viewport`, looking at the world origin, unzoomed.
    pub fn new(viewport: Size) -> Self {
        Self {
            position: Point::ORIGIN,
            zoom: 1.0,
            rotation: 0.0,
            viewport,
            snap: true,
            shake_strength: 8.0,
            shake_decay: 1.5,
            trauma: 0.0,
            time: 0.0,
        }
    }

    /// Looking at `position` (see [`position`](Self::position)).
    pub fn at(mut self, position: Point) -> Self {
        self.position = position;
        self
    }

    /// Zoomed to `zoom` screen units per world unit.
    pub fn with_zoom(mut self, zoom: f64) -> Self {
        self.zoom = zoom;
        self
    }

    /// Turned by `rotation` radians.
    pub fn with_rotation(mut self, rotation: f64) -> Self {
        self.rotation = rotation;
        self
    }

    /// Snapping to whole screen units, or not (see [`snap`](Self::snap)).
    pub fn with_snap(mut self, snap: bool) -> Self {
        self.snap = snap;
        self
    }

    /// Shaking by up to `strength` screen units at full trauma, losing
    /// `decay` trauma a second.
    pub fn with_shake(mut self, strength: f64, decay: f64) -> Self {
        self.shake_strength = strength;
        self.shake_decay = decay;
        self
    }

    /// Shake the camera: add `amount` (`0..=1`) of trauma. The shake's
    /// offset grows with the square of the trauma, which fades at
    /// [`shake_decay`](Self::shake_decay) per second as
    /// [`update`](Self::update) runs.
    pub fn shake(&mut self, amount: f64) {
        if amount.is_finite() {
            self.trauma = (self.trauma + amount).clamp(0.0, 1.0);
        }
    }

    /// The trauma left (`0..=1`).
    pub fn trauma(&self) -> f64 {
        self.trauma
    }

    /// Advance the shake by `dt` seconds.
    pub fn update(&mut self, dt: f64) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        self.time += dt;
        self.trauma = (self.trauma - self.shake_decay * dt).max(0.0);
    }

    /// Move toward `target` by the fraction of the way a critically smooth
    /// follow covers in `dt` seconds at `rate` (higher is snappier; about 5
    /// trails a walking character nicely).
    pub fn follow(&mut self, target: Point, rate: f64, dt: f64) {
        let k = 1.0 - (-rate.max(0.0) * dt.max(0.0)).exp();
        self.position = Point::new(
            self.position.x + (target.x - self.position.x) * k,
            self.position.y + (target.y - self.position.y) * k,
        );
    }

    /// Keep the view inside `bounds` (world space), centering it on an axis
    /// where the bounds are smaller than the view. Ignores rotation.
    pub fn clamp_to(&mut self, bounds: Rect) {
        let z = if self.zoom > 0.0 { self.zoom } else { 1.0 };
        let (hw, hh) = (
            self.viewport.width / z / 2.0,
            self.viewport.height / z / 2.0,
        );
        let axis = |p: f64, lo: f64, hi: f64, half: f64| {
            if hi - lo <= half * 2.0 {
                (lo + hi) / 2.0
            } else {
                p.clamp(lo + half, hi - half)
            }
        };
        self.position = Point::new(
            axis(self.position.x, bounds.min_x(), bounds.max_x(), hw),
            axis(self.position.y, bounds.min_y(), bounds.max_y(), hh),
        );
    }

    /// The shake's offset right now, in screen units.
    pub fn shake_offset(&self) -> Vec2 {
        if self.trauma <= 0.0 {
            return Vec2::new(0.0, 0.0);
        }
        let s = self.shake_strength * self.trauma * self.trauma;
        let t = self.time;
        // Smooth pseudo-noise: incommensurate sines per axis.
        let n = |a: f64, b: f64, c: f64| {
            ((t * a).sin() * 0.5 + (t * b + 1.3).sin() * 0.3 + (t * c + 2.1).sin() * 0.2)
                .clamp(-1.0, 1.0)
        };
        Vec2::new(s * n(37.0, 61.0, 89.0), s * n(43.0, 71.0, 97.0))
    }

    /// The world-to-screen transform.
    pub fn transform(&self) -> Affine {
        let shake = self.shake_offset();
        let m = Affine::translate(Vec2::new(-self.position.x, -self.position.y))
            .then(Affine::scale(self.zoom, self.zoom))
            .then(Affine::rotate(-self.rotation))
            .then(Affine::translate(Vec2::new(
                self.viewport.width / 2.0 + shake.dx,
                self.viewport.height / 2.0 + shake.dy,
            )));
        if self.snap && self.rotation == 0.0 {
            let [a, b, c, d, e, f] = m.as_array();
            Affine::new(a, b, c, d, e.round(), f.round())
        } else {
            m
        }
    }

    /// Where the world point `p` is on screen.
    pub fn world_to_screen(&self, p: Point) -> Point {
        self.transform().apply(p)
    }

    /// The world point under the screen point `p`.
    pub fn screen_to_world(&self, p: Point) -> Point {
        self.transform().inverse().map_or(p, |inv| inv.apply(p))
    }

    /// The world-space box around everything on screen: what to draw (see
    /// [`Tilemap::draw`](crate::game::Tilemap::draw)).
    pub fn visible_rect(&self) -> Rect {
        let Some(inv) = self.transform().inverse() else {
            return Rect::from_xywh(self.position.x, self.position.y, 0.0, 0.0);
        };
        inv.map_rect(Rect::new(Point::ORIGIN, self.viewport))
    }

    /// Start drawing in world coordinates: what's drawn until
    /// [`pop`](Self::pop) goes through the camera's transform.
    pub fn push(&self, scene: &mut Scene) {
        scene.push_layer(self.transform(), 1.0);
    }

    /// End drawing in world coordinates.
    pub fn pop(&self, scene: &mut Scene) {
        scene.pop_layer();
    }

    /// Run `draw` with the scene in world coordinates.
    pub fn draw(&self, scene: &mut Scene, draw: impl FnOnce(&mut Scene)) {
        self.push(scene);
        draw(scene);
        self.pop(scene);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_visible_rect_is_the_screen_in_world_space() {
        let mut cam = Camera2D::new(Size::new(200.0, 100.0));
        cam.position = Point::new(50.0, 50.0);
        cam.zoom = 0.5;
        assert_eq!(
            cam.visible_rect(),
            Rect::from_xywh(-150.0, -50.0, 400.0, 200.0)
        );
        let p = Point::new(12.0, -7.0);
        let back = cam.screen_to_world(cam.world_to_screen(p));
        assert!((back.x - p.x).abs() < 1e-9 && (back.y - p.y).abs() < 1e-9);
    }

    #[test]
    fn shake_fades_and_follow_and_clamp_settle() {
        let mut cam = Camera2D::new(Size::new(100.0, 100.0));
        cam.shake(1.0);
        cam.update(0.1);
        let o = cam.shake_offset();
        assert!(o.dx.abs() <= 8.0 && o.dy.abs() <= 8.0);
        assert!(o.dx != 0.0 || o.dy != 0.0);
        cam.update(2.0);
        assert_eq!(cam.trauma(), 0.0);
        assert_eq!(cam.shake_offset(), Vec2::new(0.0, 0.0));

        for _ in 0..200 {
            cam.follow(Point::new(500.0, 20.0), 5.0, 1.0 / 60.0);
        }
        assert!((cam.position.x - 500.0).abs() < 1.0);
        cam.clamp_to(Rect::from_xywh(0.0, 0.0, 400.0, 300.0));
        assert_eq!(cam.position, Point::new(350.0, 50.0));
    }

    #[test]
    fn a_turned_camera_turns_the_world_the_other_way() {
        let mut cam = Camera2D::new(Size::new(100.0, 100.0));
        cam.rotation = std::f64::consts::FRAC_PI_2;
        // A point right of the camera shows above the center.
        let p = cam.world_to_screen(Point::new(10.0, 0.0));
        assert!(
            (p.x - 50.0).abs() < 1e-9 && (p.y - 40.0).abs() < 1e-9,
            "{p:?}"
        );
    }
}
