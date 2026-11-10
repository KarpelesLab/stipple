//! Vector paths and strokes: straight and curved outlines, filled or stroked
//! (solid or dashed), for custom painting — a canvas's undercurl, box-drawing
//! lines, rounded selections.

use crate::geometry::{Point, Rect};
use crate::render::Color;
use crate::render::convert::to_ox_point;
use crate::render::scene::Scene;
use oxideav_core::{DashPattern, Paint, Path as OxPath, PathNode, Rgba, Stroke};

/// One step of a [`Path`].
#[derive(Clone, Copy, Debug, PartialEq)]
enum Segment {
    MoveTo(Point),
    LineTo(Point),
    QuadTo(Point, Point),
    CubicTo(Point, Point, Point),
    Close,
}

/// An outline made of straight lines and quadratic or cubic Bézier curves, in
/// logical pixels. Build it with [`move_to`](Path::move_to),
/// [`line_to`](Path::line_to), [`quad_to`](Path::quad_to),
/// [`cubic_to`](Path::cubic_to) and [`close`](Path::close), then fill it with
/// [`Scene::fill_path`] or stroke it with [`Scene::stroke_path`].
///
/// ```
/// use stipple::geometry::Point;
/// use stipple::render::Path;
///
/// // An undercurl: a wave under a cell, one bump per 4 px.
/// let mut wave = Path::new();
/// wave.move_to(Point::new(0.0, 16.0));
/// for i in 0..4 {
///     let x = i as f64 * 4.0;
///     wave.quad_to(Point::new(x + 2.0, if i % 2 == 0 { 13.0 } else { 19.0 }), Point::new(x + 4.0, 16.0));
/// }
/// assert!(!wave.is_empty());
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    segments: Vec<Segment>,
}

impl Path {
    /// An empty path.
    pub fn new() -> Self {
        Self::default()
    }

    /// A straight line from `from` to `to`.
    pub fn line(from: Point, to: Point) -> Self {
        let mut p = Self::new();
        p.move_to(from);
        p.line_to(to);
        p
    }

    /// The outline of `rect`, as a closed path.
    pub fn rect(rect: Rect) -> Self {
        let mut p = Self::new();
        p.move_to(Point::new(rect.min_x(), rect.min_y()));
        p.line_to(Point::new(rect.max_x(), rect.min_y()));
        p.line_to(Point::new(rect.max_x(), rect.max_y()));
        p.line_to(Point::new(rect.min_x(), rect.max_y()));
        p.close();
        p
    }

    /// Start a new sub-path at `to`.
    pub fn move_to(&mut self, to: Point) -> &mut Self {
        self.segments.push(Segment::MoveTo(to));
        self
    }

    /// A straight line from the current point to `to`.
    pub fn line_to(&mut self, to: Point) -> &mut Self {
        self.segments.push(Segment::LineTo(to));
        self
    }

    /// A quadratic Bézier curve to `to`, pulled towards `control`.
    pub fn quad_to(&mut self, control: Point, to: Point) -> &mut Self {
        self.segments.push(Segment::QuadTo(control, to));
        self
    }

    /// A cubic Bézier curve to `to`, leaving towards `c1` and arriving from
    /// `c2`.
    pub fn cubic_to(&mut self, c1: Point, c2: Point, to: Point) -> &mut Self {
        self.segments.push(Segment::CubicTo(c1, c2, to));
        self
    }

    /// Close the current sub-path with a line back to its start.
    pub fn close(&mut self) -> &mut Self {
        self.segments.push(Segment::Close);
        self
    }

    /// Whether the path has no segments.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The box around every point and control point of the path (which
    /// contains the curves), or `None` for an empty path.
    pub fn bounds(&self) -> Option<Rect> {
        let mut points = self.segments.iter().flat_map(|s| {
            let (a, b, c) = match *s {
                Segment::MoveTo(p) | Segment::LineTo(p) => (Some(p), None, None),
                Segment::QuadTo(c, p) => (Some(c), Some(p), None),
                Segment::CubicTo(c1, c2, p) => (Some(c1), Some(c2), Some(p)),
                Segment::Close => (None, None, None),
            };
            [a, b, c].into_iter().flatten()
        });
        let first = points.next()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for p in points {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        Some(Rect::from_xywh(x0, y0, x1 - x0, y1 - y0))
    }

    /// This path as an `oxideav` path.
    pub(crate) fn to_oxideav(&self) -> OxPath {
        let mut p = OxPath::new();
        for s in &self.segments {
            match *s {
                Segment::MoveTo(to) => p.move_to(to_ox_point(to)),
                Segment::LineTo(to) => p.line_to(to_ox_point(to)),
                Segment::QuadTo(c, to) => p.quad_to(to_ox_point(c), to_ox_point(to)),
                Segment::CubicTo(c1, c2, to) => {
                    p.cubic_to(to_ox_point(c1), to_ox_point(c2), to_ox_point(to))
                }
                Segment::Close => p.close(),
            };
        }
        p
    }
}

/// How the open ends of a stroked line are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineCap {
    /// Squared off at the end point.
    #[default]
    Butt,
    /// A half circle beyond the end point.
    Round,
    /// A half square beyond the end point.
    Square,
}

/// How the corners of a stroked path are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineJoin {
    /// A sharp corner.
    #[default]
    Miter,
    /// A rounded corner.
    Round,
    /// A corner cut off flat.
    Bevel,
}

/// How a path is stroked: its width, ends, corners and an optional dash
/// pattern.
///
/// ```
/// use stipple::render::{LineCap, StrokeStyle};
///
/// // A dotted underline: 1 px dots, 1 px apart.
/// let dotted = StrokeStyle::new(1.0).dashed(&[1.0, 1.0]);
/// let round = StrokeStyle::new(2.0).cap(LineCap::Round);
/// # let _ = (dotted, round);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct StrokeStyle {
    width: f64,
    cap: LineCap,
    join: LineJoin,
    dash: Vec<f64>,
    dash_offset: f64,
}

impl StrokeStyle {
    /// A solid stroke `width` logical pixels wide, with butt ends and mitered
    /// corners.
    pub fn new(width: f64) -> Self {
        Self {
            width,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            dash: Vec::new(),
            dash_offset: 0.0,
        }
    }

    /// Dash the stroke: alternating on and off lengths in logical pixels,
    /// starting with on. An empty pattern is solid.
    pub fn dashed(mut self, pattern: &[f64]) -> Self {
        self.dash = pattern.to_vec();
        self
    }

    /// Start the dash pattern this far along the path.
    pub fn dash_offset(mut self, offset: f64) -> Self {
        self.dash_offset = offset;
        self
    }

    /// Set how open ends are drawn.
    pub fn cap(mut self, cap: LineCap) -> Self {
        self.cap = cap;
        self
    }

    /// Set how corners are drawn.
    pub fn join(mut self, join: LineJoin) -> Self {
        self.join = join;
        self
    }

    /// The stroke width in logical pixels.
    pub fn width(&self) -> f64 {
        self.width
    }

    fn to_oxideav(&self, color: Color) -> Stroke {
        let mut s = Stroke::solid(self.width as f32, Rgba::from(color));
        s.cap = match self.cap {
            LineCap::Butt => oxideav_core::LineCap::Butt,
            LineCap::Round => oxideav_core::LineCap::Round,
            LineCap::Square => oxideav_core::LineCap::Square,
        };
        s.join = match self.join {
            LineJoin::Miter => oxideav_core::LineJoin::Miter,
            LineJoin::Round => oxideav_core::LineJoin::Round,
            LineJoin::Bevel => oxideav_core::LineJoin::Bevel,
        };
        if self.dash.iter().any(|d| *d > 0.0) {
            s.dash = Some(
                DashPattern::new(self.dash.iter().map(|d| d.max(0.0) as f32).collect())
                    .with_offset(self.dash_offset as f32),
            );
        }
        s
    }
}

impl Scene {
    /// Fill the inside of `path` (non-zero winding) with `color`.
    pub fn fill_path(&mut self, path: &Path, color: Color) {
        if path.is_empty() {
            return;
        }
        self.emit_uncommanded(oxideav_core::Node::Path(
            PathNode::new(path.to_oxideav()).with_fill(Paint::Solid(color.into())),
        ));
    }

    /// Stroke `path` in `color` with `style` (width, ends, corners, dashes).
    pub fn stroke_path(&mut self, path: &Path, color: Color, style: &StrokeStyle) {
        if path.is_empty() || style.width <= 0.0 {
            return;
        }
        self.emit_uncommanded(oxideav_core::Node::Path(
            PathNode::new(path.to_oxideav()).with_stroke(style.to_oxideav(color)),
        ));
    }

    /// A straight line from `from` to `to`, `width` logical pixels wide, with
    /// butt ends.
    pub fn draw_line(&mut self, from: Point, to: Point, color: Color, width: f64) {
        self.stroke_path(&Path::line(from, to), color, &StrokeStyle::new(width));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{ScaleFactor, Size};
    use crate::render::SoftwareRenderer;

    fn raster(scene: Scene) -> crate::render::Pixmap {
        SoftwareRenderer::new()
            .with_background(Color::BLACK)
            .render(scene, ScaleFactor::IDENTITY)
    }

    #[test]
    fn a_line_covers_its_pixels_only() {
        let mut scene = Scene::new(Size::new(20.0, 20.0));
        scene.draw_line(
            Point::new(0.0, 10.0),
            Point::new(20.0, 10.0),
            Color::WHITE,
            2.0,
        );
        assert!(scene.has_effects(), "the GPU path can't draw paths yet");
        let out = raster(scene);
        assert_eq!(out.pixel(5, 9), Some([255, 255, 255, 255]));
        assert_eq!(out.pixel(5, 10), Some([255, 255, 255, 255]));
        assert_eq!(out.pixel(5, 12), Some([0, 0, 0, 255]));
    }

    #[test]
    fn a_dashed_line_has_gaps() {
        let mut scene = Scene::new(Size::new(20.0, 4.0));
        let path = Path::line(Point::new(0.0, 2.0), Point::new(20.0, 2.0));
        scene.stroke_path(
            &path,
            Color::WHITE,
            &StrokeStyle::new(2.0).dashed(&[4.0, 4.0]),
        );
        let out = raster(scene);
        assert_eq!(out.pixel(1, 1), Some([255, 255, 255, 255]), "first dash");
        assert_eq!(out.pixel(5, 1), Some([0, 0, 0, 255]), "first gap");
        assert_eq!(out.pixel(9, 1), Some([255, 255, 255, 255]), "second dash");
    }

    #[test]
    fn a_filled_curve_is_inked_inside() {
        let mut p = Path::new();
        p.move_to(Point::new(0.0, 20.0))
            .cubic_to(
                Point::new(0.0, 0.0),
                Point::new(20.0, 0.0),
                Point::new(20.0, 20.0),
            )
            .close();
        assert_eq!(p.bounds(), Some(Rect::from_xywh(0.0, 0.0, 20.0, 20.0)));
        let mut scene = Scene::new(Size::new(20.0, 20.0));
        scene.fill_path(&p, Color::WHITE);
        let out = raster(scene);
        assert_eq!(out.pixel(10, 15), Some([255, 255, 255, 255]));
        assert_eq!(out.pixel(1, 2), Some([0, 0, 0, 255]), "outside the curve");
    }
}
