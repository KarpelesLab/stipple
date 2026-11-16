//! Text flowing around shapes: floats, shapes placed anywhere, and images
//! whose transparent pixels the text may run over (CSS Shapes, floats and
//! `wrap-flow`, Word's "square / tight / through" wrapping).
//!
//! The engine lays a text (plain or [`RichText`](crate::render::RichText),
//! with its runs, inline images, bidi and line breaking) into a region of a
//! given width holding *exclusions*: each a [`FlowShape`] in a box, grown by
//! a margin, either floated to the left or right edge at the point of the
//! text it is anchored to, or placed anywhere ([`FlowPlacement`]).
//!
//! Line by line it takes the band the next line would fill (its top to its
//! bottom, as tall as the tallest run that ends up on it: a taller run grows
//! the band and the line is fitted again), subtracts what each exclusion
//! blocks in that band from the region's width, drops what's narrower than
//! [`FlowOptions::min_segment`], and fills the intervals that are left with
//! words, one display line per interval. A word that fits nowhere in the
//! band moves the band down, to where the room changes.
//!
//! Bidirectional text: each paragraph's levels are resolved once, for the
//! whole paragraph (rules P–I of UAX #9). The paragraph's text then fills a
//! band's intervals in its own direction (left to right in a left-to-right
//! paragraph, right to left in a right-to-left one), and each interval's
//! piece is reordered on its own as a display line (rules L1–L2): a piece
//! starts at its interval's start edge (its right edge, right to left).
//! Caret positions, hit testing and selection work per piece.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, Mutex, Weak};

use crate::geometry::{PhysicalSize, Point, Rect, Size};
use crate::layout::LayoutDirection;
use crate::render::bidi::{ParagraphLevels, TextDirection, has_strong};
use crate::render::text::{LayoutLine, Styling, TextLayout};
use crate::render::{Color, ImageFit, ImageSource, Pixmap, Svg};

/// The outline of an object text flows around, within its box: what the text
/// keeps out of (grown by the object's margin).
///
/// ```
/// use std::sync::Arc;
/// use stipple::geometry::{PhysicalSize, Point};
/// use stipple::render::{FlowShape, Pixmap};
/// let diamond = FlowShape::polygon(vec![
///     Point::new(50.0, 0.0),
///     Point::new(100.0, 50.0),
///     Point::new(50.0, 100.0),
///     Point::new(0.0, 50.0),
/// ]);
/// let star = Arc::new(Pixmap::new(PhysicalSize::new(64, 64)));
/// // Text runs over the pixels at most half opaque.
/// let tight = FlowShape::alpha(star, 0.5);
/// # let _ = (diamond, tight);
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum FlowShape {
    /// The whole box.
    #[default]
    Rect,
    /// The box with its corners rounded by this radius.
    RoundRect(f64),
    /// The ellipse (a circle in a square box) inscribed in the box.
    Ellipse,
    /// A polygon, its points in the box's coordinates (even-odd filled).
    Polygon(Arc<[Point]>),
    /// The pixels of an image, stretched over the box, whose alpha is above
    /// `threshold` (0 to 1): text runs over the more transparent ones, as
    /// CSS's `shape-image-threshold` says.
    Alpha {
        /// The image (an SVG is rasterized at the box's size).
        image: ImageSource,
        /// The alpha (0 to 1) a pixel must exceed to keep text out.
        threshold: f32,
    },
}

impl FlowShape {
    /// A polygon through `points`, in the box's coordinates.
    pub fn polygon(points: impl Into<Vec<Point>>) -> Self {
        Self::Polygon(points.into().into())
    }

    /// The opaque pixels of `image` (alpha above `threshold`, 0 to 1).
    pub fn alpha(image: impl Into<ImageSource>, threshold: f32) -> Self {
        Self::Alpha {
            image: image.into(),
            threshold,
        }
    }

    /// What the shape (in a box of `size`, grown by `margin`) blocks of the
    /// horizontal band `band` (from its top to its bottom, in box
    /// coordinates): sorted, disjoint intervals, in box coordinates.
    ///
    /// ```
    /// use stipple::geometry::Size;
    /// use stipple::render::FlowShape;
    /// let disc = FlowShape::Ellipse;
    /// let size = Size::new(100.0, 100.0);
    /// // Across the middle the whole disc, grown by the margin.
    /// let mid = disc.blocked(size, 49.0..51.0, 10.0);
    /// assert!((mid[0].0 + 10.0).abs() < 0.1 && (mid[0].1 - 110.0).abs() < 0.1);
    /// // Above it, nothing.
    /// assert!(disc.blocked(size, -40.0..-20.0, 10.0).is_empty());
    /// ```
    pub fn blocked(&self, size: Size, band: Range<f64>, margin: f64) -> Vec<(f64, f64)> {
        let (top, bottom) = (band.start, band.end);
        let m = margin.max(0.0);
        if size.width <= 0.0 || size.height <= 0.0 || bottom < -m || top > size.height + m {
            return Vec::new();
        }
        match self {
            FlowShape::Alpha { image, threshold } => {
                profile(image, size, *threshold, m).map_or_else(Vec::new, |p| p.band(top, bottom))
            }
            _ => self.sampled(size, top, bottom, m),
        }
    }

    /// [`blocked`](Self::blocked) for the geometric shapes: the shape's
    /// intervals on rows sampled through the band and the margin around it,
    /// each grown by as much of the margin as reaches the band from its row
    /// (a round margin, as CSS's `shape-margin`).
    fn sampled(&self, size: Size, top: f64, bottom: f64, m: f64) -> Vec<(f64, f64)> {
        let (w, h) = (size.width, size.height);
        let lo = (top - m).max(0.0);
        let hi = (bottom + m).min(h);
        if lo > hi {
            return Vec::new();
        }
        let mut ys = Vec::new();
        let mut y = lo;
        while y < hi {
            ys.push(y);
            y += 0.5;
        }
        ys.push(hi);
        // Where a shape is widest or turns: its middle, a polygon's corners,
        // and the band's own edges.
        let mut keys = vec![h / 2.0, top.clamp(0.0, h), bottom.clamp(0.0, h)];
        if let FlowShape::Polygon(points) = self {
            keys.extend(points.iter().map(|p| p.y));
        }
        ys.extend(keys.into_iter().filter(|y| (lo..=hi).contains(y)));
        let mut out = Vec::new();
        for y in ys {
            let d = if y < top {
                top - y
            } else if y > bottom {
                y - bottom
            } else {
                0.0
            };
            if d > m {
                continue;
            }
            let e = (m * m - d * d).max(0.0).sqrt();
            for (l, r) in self.row(w, h, y) {
                out.push((l - e, r + e));
            }
        }
        merge(out)
    }

    /// The shape's intervals on the row at `y`, in a `w` × `h` box.
    fn row(&self, w: f64, h: f64, y: f64) -> Vec<(f64, f64)> {
        if !(0.0..=h).contains(&y) {
            return Vec::new();
        }
        match self {
            FlowShape::Rect | FlowShape::Alpha { .. } => vec![(0.0, w)],
            FlowShape::RoundRect(r) => {
                let r = r.clamp(0.0, w.min(h) / 2.0);
                let dy = if y < r {
                    r - y
                } else if y > h - r {
                    y - (h - r)
                } else {
                    0.0
                };
                let inset = r - (r * r - dy * dy).max(0.0).sqrt();
                vec![(inset, w - inset)]
            }
            FlowShape::Ellipse => {
                let (rx, ry) = (w / 2.0, h / 2.0);
                let dy = (y - ry) / ry;
                if dy.abs() > 1.0 {
                    return Vec::new();
                }
                let half = rx * (1.0 - dy * dy).max(0.0).sqrt();
                vec![(rx - half, rx + half)]
            }
            FlowShape::Polygon(points) => polygon_row(points, y),
        }
    }
}

/// The intervals of the even-odd polygon `points` on the row at `y`.
fn polygon_row(points: &[Point], y: f64) -> Vec<(f64, f64)> {
    let n = points.len();
    let mut xs = Vec::new();
    let mut flat = Vec::new();
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        if a.y == b.y {
            if a.y == y {
                flat.push((a.x.min(b.x), a.x.max(b.x)));
            }
            continue;
        }
        if (a.y <= y && y < b.y) || (b.y <= y && y < a.y) {
            xs.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
        }
    }
    xs.sort_by(f64::total_cmp);
    let mut out: Vec<(f64, f64)> = (0..xs.len() / 2)
        .map(|i| (xs[2 * i], xs[2 * i + 1]))
        .collect();
    out.extend(flat);
    merge(out)
}

/// Sort `intervals` and join the overlapping and touching ones.
pub(crate) fn merge(mut intervals: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    intervals.retain(|(a, b)| b >= a);
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(intervals.len());
    for (a, b) in intervals {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// `free` (sorted, disjoint) without `blocked` (sorted, disjoint).
pub(crate) fn subtract(free: &[(f64, f64)], blocked: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(mut a, b) in free {
        for &(ba, bb) in blocked {
            if bb <= a || ba >= b {
                continue;
            }
            if ba > a {
                out.push((a, ba));
            }
            a = a.max(bb);
            if a >= b {
                break;
            }
        }
        if a < b {
            out.push((a, b));
        }
    }
    out
}

/// Which sides of an object text may run on, in a line beside it (Word's
/// wrap text "both sides / largest only / left only / right only", CSS's
/// `wrap-flow`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WrapSide {
    /// Both sides (where there's room): an object in the middle of a column
    /// has text left and right of it.
    #[default]
    Both,
    /// Only the side of the region with more room beside the object's box.
    Largest,
    /// Only its left.
    Left,
    /// Only its right.
    Right,
    /// Neither: no text beside it, only above and below.
    Clear,
}

/// Where an object text flows around goes.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum FlowPlacement {
    /// Against the region's left edge, at the top of the line holding its
    /// anchor (below the floats already there when it doesn't fit beside
    /// them); text runs on its right.
    FloatLeft,
    /// Against the right edge, likewise; text runs on its left.
    FloatRight,
    /// Centered between the region's edges, at the top of the line holding
    /// its anchor (below the floats already there when it overlaps them);
    /// text runs on both sides of it.
    FloatCenter,
    /// With its box's top-left here, in the text's coordinates; text runs
    /// on both sides of it.
    At(Point),
    /// With its box's center `x` times the region's width across, and `y`
    /// times the region's width below the top of the paragraph holding its
    /// anchor (fractions of the width, so it keeps its place as the region
    /// widens or narrows), kept within the region's left and right edges
    /// and below its top. The paragraph's top is where it would be without
    /// positioned objects, so the object stays put while the text it pushes
    /// aside moves; all the text, the paragraphs above it too, flows around
    /// it.
    Positioned(Point),
}

/// How an object sits in flowing text: its [placement](FlowPlacement), its
/// [shape](FlowShape), how far text keeps from the shape (`margin`, like
/// CSS's `shape-margin`), on which sides text runs ([`WrapSide`]), and for a
/// float, the byte of the text it's anchored to.
///
/// ```
/// use stipple::geometry::Point;
/// use stipple::render::{FlowItem, FlowShape, WrapSide};
/// let photo = FlowItem::float_right().shape(FlowShape::Ellipse).margin(8.0);
/// let badge = FlowItem::at(Point::new(120.0, 40.0)).wrap(WrapSide::Largest);
/// # let _ = (photo, badge);
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct FlowItem {
    /// Where it goes.
    pub placement: FlowPlacement,
    /// Its outline in its box.
    pub shape: FlowShape,
    /// How far text keeps from the shape, in logical pixels.
    pub margin: f64,
    /// Which sides text runs on; `None` is the placement's: right of a left
    /// float, left of a right one, both sides of a placed object.
    pub wrap: Option<WrapSide>,
    /// Let text into the shape's holes and hollows ("through" wrapping),
    /// where they're wide enough, instead of keeping out of everything
    /// between its left and right edges in a line. Only with
    /// [`WrapSide::Both`].
    pub through: bool,
    /// For a float: the byte of the text it's anchored to (it floats at the
    /// top of the line holding that byte).
    pub anchor: usize,
}

impl FlowItem {
    fn new(placement: FlowPlacement) -> Self {
        Self {
            placement,
            shape: FlowShape::Rect,
            margin: 0.0,
            wrap: None,
            through: false,
            anchor: 0,
        }
    }

    /// Floated against the left edge.
    pub fn float_left() -> Self {
        Self::new(FlowPlacement::FloatLeft)
    }

    /// Floated against the right edge.
    pub fn float_right() -> Self {
        Self::new(FlowPlacement::FloatRight)
    }

    /// Centered between the region's edges, at the line holding its anchor.
    pub fn float_center() -> Self {
        Self::new(FlowPlacement::FloatCenter)
    }

    /// Placed with its top-left at `at`, in the text's coordinates.
    pub fn at(at: Point) -> Self {
        Self::new(FlowPlacement::At(at))
    }

    /// Placed with its center at `center`, in fractions of the region's
    /// width, from the top of its anchor's paragraph
    /// ([`FlowPlacement::Positioned`]).
    pub fn positioned(center: Point) -> Self {
        Self::new(FlowPlacement::Positioned(center))
    }

    /// Outlined by `shape` instead of its box.
    pub fn shape(mut self, shape: FlowShape) -> Self {
        self.shape = shape;
        self
    }

    /// Text keeps `margin` logical pixels from the shape.
    pub fn margin(mut self, margin: f64) -> Self {
        self.margin = margin.max(0.0);
        self
    }

    /// Text runs on `side`.
    pub fn wrap(mut self, side: WrapSide) -> Self {
        self.wrap = Some(side);
        self
    }

    /// Text runs into the shape's holes and hollows too.
    pub fn through(mut self) -> Self {
        self.through = true;
        self
    }

    /// Floats at the line holding byte `anchor` of the text.
    pub fn anchor(mut self, anchor: usize) -> Self {
        self.anchor = anchor;
        self
    }

    fn side(&self) -> WrapSide {
        self.wrap.unwrap_or(match self.placement {
            FlowPlacement::FloatLeft => WrapSide::Right,
            FlowPlacement::FloatRight => WrapSide::Left,
            FlowPlacement::FloatCenter | FlowPlacement::At(_) | FlowPlacement::Positioned(_) => {
                WrapSide::Both
            }
        })
    }

    /// Whether it floats at the line of its anchor, rather than being
    /// placed.
    fn floats(&self) -> bool {
        matches!(
            self.placement,
            FlowPlacement::FloatLeft | FlowPlacement::FloatRight | FlowPlacement::FloatCenter
        )
    }
}

/// How text flows: the narrowest room beside an object that still takes
/// text, and the space between paragraphs.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct FlowOptions {
    /// Room narrower than this (logical pixels) beside or between objects
    /// is left empty. Default 16.
    pub min_segment: f64,
    /// Space added between paragraphs (`\n`), in logical pixels. Default 0.
    pub paragraph_spacing: f64,
}

impl Default for FlowOptions {
    fn default() -> Self {
        Self {
            min_segment: 16.0,
            paragraph_spacing: 0.0,
        }
    }
}

impl FlowOptions {
    /// Leave room narrower than `width` empty.
    pub fn min_segment(mut self, width: f64) -> Self {
        self.min_segment = width.max(0.0);
        self
    }

    /// Add `space` between paragraphs.
    pub fn paragraph_spacing(mut self, space: f64) -> Self {
        self.paragraph_spacing = space.max(0.0);
        self
    }
}

/// A region a text flows into: its width, the paragraphs' direction, the
/// objects in it (each of a known size, placed as a [`FlowItem`] says), and
/// the [`FlowOptions`]. Lay a text out in it with
/// [`Font::layout_flow`](crate::render::Font::layout_flow) or
/// [`RichText::layout_flow`](crate::render::RichText::layout_flow); the
/// layout says where each object went
/// ([`TextLayout::flow_item_rect`]).
///
/// ```no_run
/// use stipple::geometry::{Point, Size};
/// use stipple::render::{Color, FlowItem, FlowShape, Font, Scene, TextFlow};
/// let font = Font::system_default().unwrap();
/// let flow = TextFlow::new(320.0)
///     .item(FlowItem::float_right().shape(FlowShape::Ellipse).margin(6.0), Size::new(120.0, 120.0));
/// let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit.";
/// let layout = font.layout_flow(text, 16.0, &flow);
/// let mut scene = Scene::new(Size::new(320.0, 400.0));
/// scene.fill_layout(&layout, Point::new(0.0, 0.0), Color::BLACK);
/// let disc = layout.flow_item_rect(0).unwrap();
/// scene.fill_round_rect(disc, disc.width() / 2.0, Color::rgb(200, 80, 80));
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct TextFlow {
    pub(crate) width: f64,
    pub(crate) direction: TextDirection,
    pub(crate) items: Vec<(FlowItem, Size)>,
    pub(crate) options: FlowOptions,
}

impl TextFlow {
    /// A region `width` logical pixels wide (unbounded: as wide as the
    /// longest paragraph and the floats beside it), with no objects, its
    /// paragraphs' direction detected, default options.
    pub fn new(width: f64) -> Self {
        Self {
            width,
            direction: TextDirection::Auto,
            items: Vec::new(),
            options: FlowOptions::default(),
        }
    }

    /// Paragraphs in `direction` (by default each paragraph's first strong
    /// character decides).
    pub fn direction(mut self, direction: TextDirection) -> Self {
        self.direction = direction;
        self
    }

    /// And an object `size` big, placed as `item` says.
    pub fn item(mut self, item: FlowItem, size: Size) -> Self {
        self.items.push((item, size));
        self
    }

    /// With `options`.
    pub fn options(mut self, options: FlowOptions) -> Self {
        self.options = options;
        self
    }

    /// Leave room narrower than `width` empty.
    pub fn min_segment(mut self, width: f64) -> Self {
        self.options = self.options.min_segment(width);
        self
    }

    /// Add `space` between paragraphs.
    pub fn paragraph_spacing(mut self, space: f64) -> Self {
        self.options = self.options.paragraph_spacing(space);
        self
    }
}

/// A width relative to the text an image is in: `fraction` of the width the
/// text is laid out in, at least `min` and at most `max` logical pixels
/// (`max` winning over `min`). An editor caps an image at its natural size
/// with `max`, and keeps it within the text's edges with a `fraction` of 1.
///
/// ```
/// use stipple::render::RelativeWidth;
/// let w = RelativeWidth::new(0.5).max(300.0);
/// assert_eq!(w.resolve(Some(400.0), 0.0), 200.0);
/// assert_eq!(w.resolve(Some(1000.0), 0.0), 300.0); // capped
/// assert_eq!(w.resolve(None, 120.0), 300.0); // unbounded: its cap
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct RelativeWidth {
    /// The fraction of the text's width (1 is all of it).
    pub fraction: f64,
    /// The least width, in logical pixels.
    pub min: f64,
    /// The most, in logical pixels (infinite: no limit).
    pub max: f64,
}

impl RelativeWidth {
    /// `fraction` of the text's width, with no bounds.
    pub fn new(fraction: f64) -> Self {
        Self {
            fraction: fraction.max(0.0),
            min: 0.0,
            max: f64::INFINITY,
        }
    }

    /// At least `px` logical pixels wide.
    pub fn min(mut self, px: f64) -> Self {
        self.min = px.max(0.0);
        self
    }

    /// At most `px` logical pixels wide.
    pub fn max(mut self, px: f64) -> Self {
        self.max = px.max(0.0);
        self
    }

    /// The width in a text `width` wide; in an unbounded one (`None`), its
    /// `max` if it has one, else `fallback` (a natural width), bounded.
    pub fn resolve(&self, width: Option<f64>, fallback: f64) -> f64 {
        let w = match width {
            Some(w) if w.is_finite() => self.fraction * w.max(0.0),
            _ if self.max.is_finite() => self.max,
            _ => fallback,
        };
        w.max(self.min).min(self.max).max(0.0)
    }
}

/// An image floating in a [`RichText`](crate::render::RichText): text flows
/// around it, keeping out of its [shape](FlowShape) (by default its box; see
/// [`alpha`](Self::alpha) for its opaque pixels). It is anchored where it is
/// in the text, as one U+FFFC that takes no room in the line, and floats at
/// the top of that line. Without a width it's as wide as its natural size
/// (or its height and aspect ratio make it).
///
/// ```
/// use std::sync::Arc;
/// use stipple::geometry::PhysicalSize;
/// use stipple::render::{FloatImage, Pixmap, RichText};
/// let moon = Arc::new(Pixmap::new(PhysicalSize::new(64, 64)));
/// let text = RichText::new()
///     .float(FloatImage::new(moon).right().width(96.0).alpha(0.5).margin(8.0).alt("the moon"))
///     .push("Text flows around the moon's lit part …");
/// assert_eq!(text.plain_text(), "the moonText flows around the moon's lit part …");
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct FloatImage {
    /// The image.
    pub source: ImageSource,
    /// Its width in logical pixels (`None`: from its height, or natural).
    pub width: Option<f64>,
    /// Its height in logical pixels (`None`: from its width, or natural).
    pub height: Option<f64>,
    /// How it floats and what text keeps out of.
    pub item: FlowItem,
    /// What copying the text and screen readers get in its place.
    pub alt: String,
    /// Its width relative to the text's (its height following, keeping its
    /// aspect ratio), resolved when the text is laid out: in place of
    /// `width` and `height`.
    pub relative_width: Option<RelativeWidth>,
    /// A selection frame drawn around it, with resize handles at its
    /// corners, in this color: an editor's selected image.
    pub frame: Option<Color>,
}

impl FloatImage {
    /// `source`, floated right, at its natural size, keeping text out of its
    /// box.
    pub fn new(source: impl Into<ImageSource>) -> Self {
        Self {
            source: source.into(),
            width: None,
            height: None,
            item: FlowItem::float_right(),
            alt: String::new(),
            relative_width: None,
            frame: None,
        }
    }

    /// Floated against the left edge.
    pub fn left(mut self) -> Self {
        self.item.placement = FlowPlacement::FloatLeft;
        self
    }

    /// Floated against the right edge.
    pub fn right(mut self) -> Self {
        self.item.placement = FlowPlacement::FloatRight;
        self
    }

    /// Centered in the text's width at its line, text on both sides.
    pub fn center(mut self) -> Self {
        self.item.placement = FlowPlacement::FloatCenter;
        self
    }

    /// Placed with its top-left at `at` in the text's box, text on both
    /// sides.
    pub fn at(mut self, at: Point) -> Self {
        self.item.placement = FlowPlacement::At(at);
        self
    }

    /// Placed with its center at `center`, in fractions of the text's width,
    /// from the top of its paragraph ([`FlowPlacement::Positioned`]).
    pub fn positioned(mut self, center: Point) -> Self {
        self.item.placement = FlowPlacement::Positioned(center);
        self
    }

    /// As wide as `width` says of the width the text is laid out in, its
    /// height keeping its aspect ratio.
    pub fn relative_width(mut self, width: RelativeWidth) -> Self {
        self.relative_width = Some(width);
        self
    }

    /// Drawn with a selection frame and corner handles in `color`.
    pub fn frame(mut self, color: Color) -> Self {
        self.frame = Some(color);
        self
    }

    /// `width` logical pixels wide.
    pub fn width(mut self, width: f64) -> Self {
        self.width = Some(width.max(0.0));
        self
    }

    /// `height` logical pixels tall.
    pub fn height(mut self, height: f64) -> Self {
        self.height = Some(height.max(0.0));
        self
    }

    /// Outlined by `shape`.
    pub fn shape(mut self, shape: FlowShape) -> Self {
        self.item.shape = shape;
        self
    }

    /// Outlined by its own pixels whose alpha is above `threshold` (0 to 1):
    /// text runs over the transparent ones.
    pub fn alpha(mut self, threshold: f32) -> Self {
        self.item.shape = FlowShape::alpha(self.source.clone(), threshold);
        self
    }

    /// Text keeps `margin` logical pixels from it.
    pub fn margin(mut self, margin: f64) -> Self {
        self.item.margin = margin.max(0.0);
        self
    }

    /// Text runs on `side`.
    pub fn wrap(mut self, side: WrapSide) -> Self {
        self.item.wrap = Some(side);
        self
    }

    /// Text runs into its holes and hollows too.
    pub fn through(mut self) -> Self {
        self.item.through = true;
        self
    }

    /// Copied and read aloud as `alt`.
    pub fn alt(mut self, alt: impl Into<String>) -> Self {
        self.alt = alt.into();
        self
    }

    /// The size it's drawn at in a text `region` wide (`None`: unbounded),
    /// its [`relative_width`](Self::relative_width) resolved there.
    pub fn size_in(&self, region: Option<f64>) -> Size {
        let Some(rel) = self.relative_width else {
            return self.size();
        };
        let natural = self.source.size();
        let w = rel.resolve(region, natural.width);
        let h = if natural.width > 0.0 {
            w * natural.height / natural.width
        } else {
            w
        };
        Size::new(w, h)
    }

    /// The size it's drawn at, without a
    /// [`relative_width`](Self::relative_width) (with one, see
    /// [`size_in`](Self::size_in)).
    pub fn size(&self) -> Size {
        let natural = self.source.size();
        let aspect = |a: f64, b: f64| if b > 0.0 { a / b } else { 1.0 };
        match (self.width, self.height) {
            (Some(w), Some(h)) => Size::new(w, h),
            (Some(w), None) => Size::new(w, w * aspect(natural.height, natural.width)),
            (None, Some(h)) => Size::new(h * aspect(natural.width, natural.height), h),
            (None, None) => natural,
        }
    }
}

/// Where a flowed layout's objects went.
#[derive(Clone, Debug, Default)]
pub(crate) struct FlowGeometry {
    /// The layout's size: the region's width, down to the lowest line or
    /// object.
    pub size: Size,
    /// Each [`TextFlow`] item's box, in order.
    pub items: Vec<Rect>,
    /// The rich text's floating images, in text order, with their boxes.
    pub images: Vec<PlacedImage>,
}

/// A rich text's floating image where the flow engine put it.
#[derive(Clone, Debug)]
pub(crate) struct PlacedImage {
    pub rect: Rect,
    pub source: ImageSource,
    /// The byte of the text it is anchored at.
    pub anchor: usize,
    /// Its selection frame's color, if it has one.
    pub frame: Option<Color>,
}

// --- Alpha shapes ---

/// The rows of an alpha shape at one size and margin: per logical-pixel row
/// of the box (from row `first`, which the margin can make negative), the
/// blocked intervals, grown by the margin.
#[derive(Debug)]
struct Profile {
    first: i64,
    rows: Vec<Vec<(f64, f64)>>,
}

impl Profile {
    fn band(&self, top: f64, bottom: f64) -> Vec<(f64, f64)> {
        let a = top.floor() as i64;
        let b = (bottom.ceil() as i64).max(a + 1);
        let mut out = Vec::new();
        for j in a..b {
            let k = j - self.first;
            if k >= 0
                && let Some(row) = self.rows.get(k as usize)
            {
                out.extend_from_slice(row);
            }
        }
        merge(out)
    }

    /// The opaque runs of `pm` (alpha above `threshold`) stretched over a
    /// `size` box, then grown by `margin` in every direction (a disc).
    fn build(pm: &Pixmap, size: Size, threshold: f32, margin: f64) -> Self {
        let (pw, ph) = (pm.size().width as usize, pm.size().height as usize);
        let k = size.height.ceil().max(0.0) as usize;
        let mut base: Vec<Vec<(f64, f64)>> = vec![Vec::new(); k];
        if pw > 0 && ph > 0 && k > 0 {
            let (sx, sy) = (size.width / pw as f64, size.height / ph as f64);
            let thr = (threshold.clamp(0.0, 1.0) * 255.0).floor() as u8;
            let data = pm.as_bytes();
            for py in 0..ph {
                let row = &data[py * pw * 4..(py + 1) * pw * 4];
                let mut runs = Vec::new();
                let mut x = 0;
                while x < pw {
                    if row[x * 4 + 3] > thr {
                        let s = x;
                        while x < pw && row[x * 4 + 3] > thr {
                            x += 1;
                        }
                        runs.push((s as f64 * sx, x as f64 * sx));
                    } else {
                        x += 1;
                    }
                }
                if runs.is_empty() {
                    continue;
                }
                let (y0, y1) = (py as f64 * sy, (py + 1) as f64 * sy);
                let r0 = y0.floor() as usize;
                let r1 = (y1.ceil() as usize).max(r0 + 1).min(k);
                for r in base.iter_mut().take(r1).skip(r0) {
                    r.extend_from_slice(&runs);
                }
            }
        }
        let base: Vec<Vec<(f64, f64)>> = base.into_iter().map(merge).collect();
        let m = margin.max(0.0);
        if m == 0.0 {
            return Self {
                first: 0,
                rows: base,
            };
        }
        let mi = m.ceil() as i64;
        let ki = k as i64;
        let rows = (-mi..ki + mi)
            .map(|j| {
                let mut out = Vec::new();
                for kk in (j - mi).max(0)..(j + mi + 1).min(ki) {
                    let gap = ((j - kk).abs() - 1).max(0) as f64;
                    if gap > m {
                        continue;
                    }
                    let e = (m * m - gap * gap).sqrt();
                    out.extend(base[kk as usize].iter().map(|&(a, b)| (a - e, b + e)));
                }
                merge(out)
            })
            .collect();
        Self { first: -mi, rows }
    }
}

/// Keeps an image's allocation (so its address) alive while a profile of it
/// is cached under that address.
#[derive(Debug)]
enum Pin {
    Bitmap(#[allow(dead_code)] Weak<Pixmap>),
    Svg(#[allow(dead_code)] Weak<Svg>),
}

type ProfileKey = (usize, i64, i64, u16, i64);

/// Alpha profiles by image, box size, threshold and margin (each in 1/16 px
/// or 1/1000): a float's shape is built once, however often its text is
/// laid out again.
static PROFILES: Mutex<Vec<(ProfileKey, Pin, Arc<Profile>)>> = Mutex::new(Vec::new());

/// How many profiles stay cached.
const PROFILES_KEPT: usize = 32;

fn profile(image: &ImageSource, size: Size, threshold: f32, margin: f64) -> Option<Arc<Profile>> {
    let q = |v: f64| (v * 16.0).round() as i64;
    let ptr = match image {
        ImageSource::Bitmap(pm) => Arc::as_ptr(pm) as usize,
        ImageSource::Svg(svg) => Arc::as_ptr(svg) as usize,
    };
    let key = (
        ptr,
        q(size.width),
        q(size.height),
        (threshold.clamp(0.0, 1.0) * 1000.0) as u16,
        q(margin),
    );
    let mut cache = PROFILES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = cache.iter().position(|(k, ..)| *k == key) {
        // Most recently used last.
        let entry = cache.remove(i);
        let p = entry.2.clone();
        cache.push(entry);
        return Some(p);
    }
    let (pm, pin) = match image {
        ImageSource::Bitmap(pm) => (pm.clone(), Pin::Bitmap(Arc::downgrade(pm))),
        ImageSource::Svg(svg) => {
            let px = PhysicalSize::new(
                size.width.ceil().max(1.0) as u32,
                size.height.ceil().max(1.0) as u32,
            );
            (
                svg.rasterize_cached(px, ImageFit::Fill),
                Pin::Svg(Arc::downgrade(svg)),
            )
        }
    };
    let p = Arc::new(Profile::build(&pm, size, threshold, margin));
    if cache.len() >= PROFILES_KEPT {
        cache.remove(0);
    }
    cache.push((key, pin, p.clone()));
    Some(p)
}

#[cfg(test)]
pub(crate) fn profiles_cached() -> usize {
    PROFILES.lock().map_or(0, |c| c.len())
}

// --- The engine ---

/// An object placed in the region.
#[derive(Debug)]
struct Placed {
    rect: Rect,
    shape: FlowShape,
    margin: f64,
    side: WrapSide,
    through: bool,
    /// For a float, where it floats.
    float: Option<Side>,
}

/// Which edge a float is against, or the middle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
    Center,
}

/// An object of the region, before it's placed.
struct Obj {
    item: FlowItem,
    size: Size,
    /// For a rich text's floating image, the image.
    image: Option<ImageSource>,
    frame: Option<Color>,
}

/// What a pass of the engine laid out: the lines, each object's box (`None`:
/// not placed), the text's bottom, and each paragraph's top.
struct Pass {
    lines: Vec<LayoutLine>,
    rects: Vec<Option<Rect>>,
    bottom: f64,
    para_tops: Vec<f64>,
}

impl Placed {
    /// Add what it blocks of the band `top..bottom` (region coordinates) to
    /// `out`.
    fn block(&self, top: f64, bottom: f64, width: f64, out: &mut Vec<(f64, f64)>) {
        let (r, m) = (self.rect, self.margin);
        if bottom <= r.min_y() - m || top >= r.max_y() + m {
            return;
        }
        let local = self
            .shape
            .blocked(r.size, top - r.min_y()..bottom - r.min_y(), m);
        let (Some(first), Some(last)) = (local.first(), local.last()) else {
            return;
        };
        let (a, b) = (first.0 + r.min_x(), last.1 + r.min_x());
        let side = match self.side {
            WrapSide::Largest => {
                if r.min_x() - m >= width - (r.max_x() + m) {
                    WrapSide::Left
                } else {
                    WrapSide::Right
                }
            }
            s => s,
        };
        match side {
            WrapSide::Both if self.through => {
                out.extend(local.iter().map(|&(l, h)| (l + r.min_x(), h + r.min_x())))
            }
            WrapSide::Both | WrapSide::Largest => out.push((a, b)),
            WrapSide::Left => out.push((a, f64::INFINITY)),
            WrapSide::Right => out.push((f64::NEG_INFINITY, b)),
            WrapSide::Clear => out.push((f64::NEG_INFINITY, f64::INFINITY)),
        }
    }
}

/// The room left in the band `top..bottom` of a region `width` wide holding
/// `placed`, without what's narrower than `min`; `None` when nothing blocks
/// any of it.
fn available(
    placed: &[Placed],
    top: f64,
    bottom: f64,
    width: f64,
    min: f64,
) -> Option<Vec<(f64, f64)>> {
    let mut blocked = Vec::new();
    for p in placed {
        p.block(top, bottom, width, &mut blocked);
    }
    let blocked = merge(blocked);
    if subtract(&[(0.0, width)], &blocked) == [(0.0, width)] {
        return None;
    }
    let mut free = subtract(&[(0.0, width)], &blocked);
    free.retain(|(a, b)| b - a >= min);
    Some(free)
}

/// Where to try a band after `y` when nothing fit in the one at `y`, `h`
/// tall: the next bottom of an object, or a quarter line further down
/// (shapes narrow and widen gradually), whichever comes first.
fn next_y(placed: &[Placed], y: f64, h: f64) -> f64 {
    let step = y + (h / 4.0).max(1.0);
    placed
        .iter()
        .map(|p| p.rect.max_y() + p.margin)
        .filter(|&b| b > y + 1e-6)
        .fold(step, f64::min)
}

/// One band's lines, before they're placed: each piece's line (its runs
/// shaped) and interval, and where the text goes on after the band.
#[derive(Clone, Debug)]
struct Band {
    lines: Vec<LayoutLine>,
    ascent: f64,
    height: f64,
    /// Where the paragraph's next band starts (relative to the paragraph).
    next: usize,
}

/// A paragraph being laid out.
struct Para<'p> {
    text: &'p str,
    /// Its first byte in the whole text.
    start: usize,
    levels: ParagraphLevels,
    offsets: Vec<usize>,
}

/// Bands already filled, by text and looks (see [`content_key`]), then by
/// where the band starts and the room it had: moving an object only fills
/// (and shapes) again the lines whose room it changed.
type FillMemo = HashMap<(usize, Vec<(i64, i64)>, bool), Arc<Band>>;

static FILLS: Mutex<Vec<(u64, FillMemo)>> = Mutex::new(Vec::new());

/// How many texts' bands, and how many bands per text, stay memoized.
const FILL_TEXTS: usize = 16;
const FILLS_PER_TEXT: usize = 4096;

impl Styling {
    /// A hash of what the bands of `text` depend on: the text, the
    /// direction, and each look's font, size and image size.
    fn content_key(&self, text: &str, direction: TextDirection, start_right: bool) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        direction.hash(&mut h);
        start_right.hash(&mut h);
        for f in &self.faces {
            f.font.hash(&mut h);
            f.font.chain().len().hash(&mut h);
            f.size.to_bits().hash(&mut h);
            if let Some((_, s)) = &f.icon {
                s.width.to_bits().hash(&mut h);
                s.height.to_bits().hash(&mut h);
            }
            f.float.is_some().hash(&mut h);
        }
        self.spans.hash(&mut h);
        h.finish()
    }

    /// Lay `text` out in these looks into a region `max_width` wide (or as
    /// wide as its longest paragraph and its floats), flowing around
    /// `flow`'s objects and the floating images among the looks.
    pub(crate) fn layout_flow<'a>(
        mut self,
        text: &'a str,
        direction: TextDirection,
        max_width: Option<f64>,
        ui: LayoutDirection,
        flow: Option<&TextFlow>,
    ) -> TextLayout<'a> {
        let bounded = max_width.filter(|w| w.is_finite()).map(|w| w.max(0.0));
        self.resolve_icons(bounded);
        let start_right = ui.is_rtl() && direction == TextDirection::Auto;
        let options = flow.map(|f| f.options).unwrap_or_default();
        // Every object: the flow's items, then the floating images.
        let mut objs: Vec<Obj> = flow
            .map(|f| {
                f.items
                    .iter()
                    .map(|(item, size)| Obj {
                        item: item.clone(),
                        size: *size,
                        image: None,
                        frame: None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let n_items = objs.len();
        for &(at, fi) in &self.spans {
            if let Some(fl) = &self.faces[fi].float {
                objs.push(Obj {
                    item: fl.item.clone().anchor(at),
                    size: fl.size_in(bounded),
                    image: Some(fl.source.clone()),
                    frame: fl.frame,
                });
            }
        }
        let width = match bounded {
            Some(w) => w,
            None => {
                let mut start = 0;
                let mut widest: f64 = 0.0;
                for para in text.split('\n') {
                    widest = widest.max(self.width(text, start..start + para.len()));
                    start += para.len() + 1;
                }
                let beside: f64 = objs
                    .iter()
                    .filter(|o| o.item.floats())
                    .map(|o| o.size.width + o.item.margin)
                    .sum();
                let placed = objs
                    .iter()
                    .filter_map(|o| match o.item.placement {
                        FlowPlacement::At(p) => Some(p.x + o.size.width),
                        FlowPlacement::Positioned(_) => Some(o.size.width),
                        _ => None,
                    })
                    .fold(0.0, f64::max);
                (widest + beside).max(placed).ceil()
            }
        };

        let key = self.content_key(text, direction, start_right);
        let mut memo = take_memo(key);
        // Objects positioned from their paragraph's top: where the
        // paragraphs are without them, then laid out at those places.
        if objs
            .iter()
            .any(|o| matches!(o.item.placement, FlowPlacement::Positioned(_)))
        {
            let tops = self
                .flow_pass(
                    text,
                    direction,
                    start_right,
                    width,
                    options,
                    &objs,
                    &mut memo,
                )
                .para_tops;
            for o in &mut objs {
                if let FlowPlacement::Positioned(c) = o.item.placement {
                    let anchor = o.item.anchor.min(text.len());
                    let para = text.as_bytes()[..anchor]
                        .iter()
                        .filter(|b| **b == b'\n')
                        .count();
                    let top = tops.get(para).copied().unwrap_or(0.0);
                    let (w, h) = (o.size.width, o.size.height);
                    let x = (c.x * width - w / 2.0).min(width - w).max(0.0);
                    let y = (top + c.y * width - h / 2.0).max(0.0);
                    o.item.placement = FlowPlacement::At(Point::new(x, y));
                }
            }
        }
        let pass = self.flow_pass(
            text,
            direction,
            start_right,
            width,
            options,
            &objs,
            &mut memo,
        );
        put_memo(key, memo);
        let rects = pass.rects;
        let bottom = rects
            .iter()
            .flatten()
            .map(|r| r.max_y())
            .fold(pass.bottom, f64::max);
        let geometry = FlowGeometry {
            size: Size::new(width, bottom),
            items: rects[..n_items]
                .iter()
                .map(|r| r.unwrap_or_default())
                .collect(),
            images: objs[n_items..]
                .iter()
                .zip(&rects[n_items..])
                .filter_map(|(o, r)| {
                    Some(PlacedImage {
                        rect: (*r)?,
                        source: o.image.clone()?,
                        anchor: o.item.anchor,
                        frame: o.frame,
                    })
                })
                .collect(),
        };
        TextLayout {
            text,
            styling: self,
            lines: pass.lines,
            width,
            align_width: width,
            start_right,
            flow: Some(Box::new(geometry)),
        }
    }

    /// One pass of the engine: `text` laid out in a region `width` wide
    /// around `objs`, filling bands through `memo`. Objects still
    /// [`Positioned`](FlowPlacement::Positioned) are left out: the first
    /// pass finds their paragraphs' tops.
    #[allow(clippy::too_many_arguments)]
    fn flow_pass(
        &self,
        text: &str,
        direction: TextDirection,
        start_right: bool,
        width: f64,
        options: FlowOptions,
        objs: &[Obj],
        memo: &mut FillMemo,
    ) -> Pass {
        let mut rects: Vec<Option<Rect>> = vec![None; objs.len()];
        let mut placed: Vec<Placed> = Vec::new();
        let place = |placed: &mut Vec<Placed>, obj: &Obj, rect: Rect, float| {
            placed.push(Placed {
                rect,
                shape: obj.item.shape.clone(),
                margin: obj.item.margin,
                side: obj.item.side(),
                through: obj.item.through,
                float,
            });
        };
        for (i, o) in objs.iter().enumerate() {
            if let FlowPlacement::At(p) = o.item.placement {
                let rect = Rect::new(p, o.size);
                rects[i] = Some(rect);
                place(&mut placed, o, rect, None);
            }
        }
        let mut pending: Vec<usize> = (0..objs.len()).filter(|&i| objs[i].item.floats()).collect();
        pending.sort_by_key(|&i| objs[i].item.anchor);
        // Float `i` at the top of the band at `y` (or lower, below floats it
        // doesn't fit beside).
        let float = |placed: &mut Vec<Placed>, rects: &mut Vec<Option<Rect>>, i: usize, y: f64| {
            let o = &objs[i];
            let side = match o.item.placement {
                FlowPlacement::FloatLeft => Side::Left,
                FlowPlacement::FloatRight => Side::Right,
                _ => Side::Center,
            };
            let (w, h, m) = (o.size.width, o.size.height, o.item.margin);
            // Not above the floats before it.
            let mut top = placed
                .iter()
                .filter(|p| p.float.is_some())
                .map(|p| p.rect.min_y())
                .fold(y, f64::max);
            let x = loop {
                let beside: Vec<&Placed> = placed
                    .iter()
                    .filter(|p| {
                        p.float.is_some() && p.rect.min_y() < top + h && p.rect.max_y() > top
                    })
                    .collect();
                if side == Side::Center {
                    let x = ((width - w) / 2.0).max(0.0);
                    let clear = beside.iter().all(|p| {
                        let gap = p.margin.max(m);
                        p.rect.max_x() + gap <= x || p.rect.min_x() - gap >= x + w
                    });
                    if clear {
                        break x;
                    }
                } else {
                    // A centered float bounds a left one on the right, a
                    // right one on the left.
                    let lo = beside
                        .iter()
                        .filter(|p| {
                            p.float == Some(Side::Left)
                                || (side == Side::Right && p.float == Some(Side::Center))
                        })
                        .map(|p| p.rect.max_x() + p.margin.max(m))
                        .fold(0.0, f64::max);
                    let hi = beside
                        .iter()
                        .filter(|p| {
                            p.float == Some(Side::Right)
                                || (side == Side::Left && p.float == Some(Side::Center))
                        })
                        .map(|p| p.rect.min_x() - p.margin.max(m))
                        .fold(width, f64::min);
                    if beside.is_empty() || hi - lo >= w {
                        break if side == Side::Left {
                            lo
                        } else {
                            (hi - w).max(0.0)
                        };
                    }
                }
                top = beside
                    .iter()
                    .map(|p| p.rect.max_y())
                    .fold(f64::INFINITY, f64::min);
            };
            let rect = Rect::from_xywh(x, top, w, h);
            rects[i] = Some(rect);
            place(placed, o, rect, Some(side));
        };

        let mut lines: Vec<LayoutLine> = Vec::new();
        let mut para_tops: Vec<f64> = Vec::new();
        let mut y = 0.0;
        let mut start = 0usize;
        let paras: Vec<&str> = text.split('\n').collect();
        for (pi, para) in paras.iter().enumerate() {
            para_tops.push(y);
            let para_dir = if start_right && !has_strong(para) {
                TextDirection::Rtl
            } else {
                direction
            };
            let p = Para {
                text: para,
                start,
                levels: ParagraphLevels::new(para, para_dir),
                offsets: para
                    .char_indices()
                    .map(|(i, _)| i)
                    .chain(std::iter::once(para.len()))
                    .collect(),
            };
            let rtl = p.levels.base == 1;
            let mut pos = 0usize;
            loop {
                // A float anchored in text already laid out floats here.
                while let Some(&i) = pending.first() {
                    if objs[i].item.anchor >= start + pos {
                        break;
                    }
                    pending.remove(0);
                    float(&mut placed, &mut rects, i, y);
                }
                let first_h = {
                    let f = self.face_at(start + pos);
                    f.ascent() + f.descent()
                };
                let mut h = first_h;
                let mut tries = 0;
                let band = loop {
                    let room = available(&placed, y, y + h, width, options.min_segment);
                    let force = room.is_none()
                        || !placed
                            .iter()
                            .any(|p| p.rect.max_y() + p.margin > y + h + 1e-6);
                    let room = room.unwrap_or_else(|| vec![(0.0, width)]);
                    let band = if room.is_empty() {
                        None
                    } else {
                        let mkey = (
                            start + pos,
                            room.iter()
                                .map(|&(a, b)| {
                                    ((a * 64.0).round() as i64, (b * 64.0).round() as i64)
                                })
                                .collect(),
                            force,
                        );
                        match memo.get(&mkey) {
                            Some(b) => Some(b.clone()),
                            None => {
                                let b = self
                                    .fill_band(text, &p, pos, &room, rtl, force)
                                    .map(Arc::new);
                                if let Some(b) = &b {
                                    if memo.len() >= FILLS_PER_TEXT {
                                        memo.clear();
                                    }
                                    memo.insert(mkey, b.clone());
                                }
                                b
                            }
                        }
                    };
                    let Some(band) = band else {
                        // Nothing fits here: try lower.
                        y = next_y(&placed, y, h);
                        h = first_h;
                        continue;
                    };
                    // Floats anchored in this band's text float at its top,
                    // and the band is filled again around them.
                    let end = if band.next >= para.len() {
                        start + para.len() + 1
                    } else {
                        start + band.next
                    };
                    let mut floated = false;
                    while let Some(&i) = pending.first() {
                        if objs[i].item.anchor >= end {
                            break;
                        }
                        pending.remove(0);
                        float(&mut placed, &mut rects, i, y);
                        floated = true;
                    }
                    if floated {
                        h = first_h;
                        continue;
                    }
                    // A taller run than the band was measured for: measure
                    // the room again for its height.
                    if band.height > h + 1e-6 && tries < 8 {
                        h = band.height;
                        tries += 1;
                        continue;
                    }
                    break band;
                };
                for line in &band.lines {
                    let mut line = line.clone();
                    line.top = y;
                    line.ascent = band.ascent;
                    line.height = band.height;
                    lines.push(line);
                }
                y += band.height;
                pos = band.next;
                if pos >= para.len() {
                    break;
                }
            }
            start += para.len() + 1;
            if pi + 1 < paras.len() {
                y += options.paragraph_spacing;
            }
        }
        // Floats anchored past the text float below it.
        for i in std::mem::take(&mut pending) {
            float(&mut placed, &mut rects, i, y);
        }
        Pass {
            lines,
            rects,
            bottom: y,
            para_tops,
        }
    }

    /// Fill a band with the paragraph's text from byte `pos`: the room's
    /// intervals in the paragraph's direction, each with as many words as
    /// fit. An interval whose first word doesn't fit is skipped; with no
    /// word fitting anywhere, `None`, unless `force`d: then the widest
    /// interval takes the word anyway.
    fn fill_band(
        &self,
        text: &str,
        p: &Para<'_>,
        pos: usize,
        room: &[(f64, f64)],
        rtl: bool,
        force: bool,
    ) -> Option<Band> {
        let mut order = room.to_vec();
        if rtl {
            order.reverse();
        }
        let mut pieces: Vec<(Range<usize>, (f64, f64))> = Vec::new();
        let mut at = pos;
        if at >= p.text.len() {
            pieces.push((at..at, order[0]));
        } else {
            for iv in &order {
                if at >= p.text.len() {
                    break;
                }
                if let Some((r, next)) = self.fill_piece(text, p, at, iv.1 - iv.0, false) {
                    pieces.push((r, *iv));
                    at = next;
                }
            }
        }
        if pieces.is_empty() {
            if !force {
                return None;
            }
            let iv = *order
                .iter()
                .max_by(|a, b| (a.1 - a.0).total_cmp(&(b.1 - b.0)))?;
            let (r, next) = self.fill_piece(text, p, at, iv.1 - iv.0, true)?;
            pieces.push((r, iv));
            at = next;
        }
        let char_at = |b: usize| p.offsets.partition_point(|&o| o < b);
        let mut lines = Vec::with_capacity(pieces.len());
        let (mut ascent, mut descent) = (0.0_f64, 0.0_f64);
        for (r, iv) in pieces {
            let chars = char_at(r.start)..char_at(r.end);
            let levels = p.levels.line_levels(chars.clone());
            let mut runs = self.shape_line(p.text, p.start, &p.offsets, chars, &levels);
            for run in &mut runs {
                run.range = p.start + run.range.start..p.start + run.range.end;
            }
            let (a, d) = if runs.is_empty() {
                let f = self.face_at(p.start + r.start);
                (f.ascent(), f.descent())
            } else {
                runs.iter().fold((0.0_f64, 0.0_f64), |(a, d), run| {
                    let f = &self.faces[run.face];
                    (a.max(f.ascent()), d.max(f.descent()))
                })
            };
            ascent = ascent.max(a);
            descent = descent.max(d);
            lines.push(LayoutLine {
                range: p.start + r.start..p.start + r.end,
                rtl: p.levels.base == 1,
                width: runs.iter().map(|r| r.width).sum(),
                runs,
                top: 0.0,
                ascent: a,
                height: a + d,
                left: iv.0,
                slot: Some(iv.1 - iv.0),
            });
        }
        Some(Band {
            lines,
            ascent,
            height: ascent + descent,
            next: at,
        })
    }

    /// The words of the paragraph from byte `pos` that fit in `max` logical
    /// pixels: the piece's range and where the text goes on (past the space
    /// it broke at), both relative to the paragraph. `None` when even the
    /// first word doesn't fit, unless `force`d.
    fn fill_piece(
        &self,
        text: &str,
        p: &Para<'_>,
        pos: usize,
        max: f64,
        force: bool,
    ) -> Option<(Range<usize>, usize)> {
        let s = p.start;
        let w = |a: usize, b: usize| self.width(text, s + a..s + b);
        let word_end = |at: usize| p.text[at..].find(' ').map_or(p.text.len(), |i| at + i);
        let first = word_end(pos);
        let mut width = w(pos, first);
        if width > max + 1e-6 && !force {
            return None;
        }
        let mut end = first;
        while end < p.text.len() {
            let next = word_end(end + 1);
            let more = w(end, end + 1) + w(end + 1, next);
            if width + more > max + 1e-6 {
                return Some((pos..end, end + 1));
            }
            width += more;
            end = next;
        }
        Some((pos..end, end))
    }
}

fn take_memo(key: u64) -> FillMemo {
    let mut all = FILLS.lock().unwrap_or_else(|e| e.into_inner());
    all.iter()
        .position(|(k, _)| *k == key)
        .map(|i| all.remove(i).1)
        .unwrap_or_default()
}

fn put_memo(key: u64, memo: FillMemo) {
    let mut all = FILLS.lock().unwrap_or_else(|e| e.into_inner());
    if all.len() >= FILL_TEXTS {
        all.remove(0);
    }
    all.push((key, memo));
}

impl TextLayout<'_> {
    /// Where item `i` of the [`TextFlow`] the text was laid out with went,
    /// relative to the text box; `None` without one.
    pub fn flow_item_rect(&self, i: usize) -> Option<Rect> {
        self.flow.as_ref()?.items.get(i).copied()
    }

    /// Where the rich text's floating images ([`FloatImage`]) went, in text
    /// order, relative to the text box.
    pub fn float_rects(&self) -> Vec<Rect> {
        self.flow
            .as_ref()
            .map(|f| f.images.iter().map(|p| p.rect).collect())
            .unwrap_or_default()
    }

    /// The box display line `line` is drawn in, relative to the text box:
    /// its left edge, top, advance width and height.
    pub fn line_rect(&self, line: usize) -> Option<Rect> {
        let l = self.lines.get(line)?;
        Some(Rect::from_xywh(self.line_x(l), l.top, l.width, l.height))
    }
}

#[cfg(test)]
#[path = "flow_tests.rs"]
mod tests;
