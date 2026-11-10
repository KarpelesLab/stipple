//! Lowering a [`Scene`]'s nodes for one area of it.
//!
//! Two things happen on the way to the rasterizer:
//!
//! - **Culling.** When only part of the scene is rasterized (an area repaint,
//!   or one band of a tiled full render), everything that can't touch that
//!   area is dropped first. The rasterizer works on canvas-sized masks for
//!   every primitive, so a primitive outside the area still costs a mask the
//!   size of the area; dropping it costs a bounding-box test.
//! - **Deferred content.** Bitmaps and canvases are recorded as placeholders
//!   and only turned into nodes here: a bitmap is copied (only the part that
//!   shows in the area), and a canvas's paint callback runs with the part of
//!   it that is being rasterized. So painting a scene copies no pixels, a
//!   cloned scene shares its bitmaps, and a canvas paints only what is
//!   needed.

use crate::geometry::{Rect, Vec2};
use crate::render::Pixmap;
use crate::render::canvas::{Canvas, CanvasPaint};
use crate::render::scene::Scene;
use oxideav_core::{
    Group, ImageRef, Node, Path, PathCommand, Rect as OxRect, Transform2D, VideoFrame, VideoPlane,
};
use std::sync::Arc;

/// Content recorded by a [`Scene`] as a placeholder, made into nodes when the
/// scene is lowered.
#[derive(Clone)]
pub(crate) enum Deferred {
    /// A straight-alpha RGBA bitmap.
    Image(Arc<Pixmap>),
    /// A canvas: `paint` draws `bounds` (in the coordinates the placeholder
    /// sits in) at `scale` physical pixels per logical pixel.
    Canvas {
        bounds: Rect,
        paint: CanvasPaint,
        scale: f64,
    },
}

impl std::fmt::Debug for Deferred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Deferred::Image(p) => f.debug_tuple("Image").field(&p.size()).finish(),
            Deferred::Canvas { bounds, .. } => {
                f.debug_struct("Canvas").field("bounds", bounds).finish()
            }
        }
    }
}

/// Placeholders are image nodes with no pixel planes whose timestamp carries
/// this tag plus the index into the scene's deferred table.
const TAG: i64 = 0x5354_4950_0000_0000;
const TAG_MASK: i64 = !0xFFFF_FFFF;

/// A placeholder node for deferred entry `index`, covering `bounds`.
pub(crate) fn placeholder(index: usize, bounds: Rect) -> Node {
    Node::Image(ImageRef {
        frame: Box::new(VideoFrame {
            pts: Some(TAG | index as i64),
            planes: Vec::new(),
        }),
        bounds: ox_rect(bounds),
        transform: Transform2D::identity(),
    })
}

fn deferred_index(img: &ImageRef) -> Option<usize> {
    let pts = img.frame.pts?;
    (img.frame.planes.is_empty() && pts & TAG_MASK == TAG).then_some((pts & !TAG_MASK) as usize)
}

fn ox_rect(r: Rect) -> OxRect {
    OxRect {
        x: r.min_x() as f32,
        y: r.min_y() as f32,
        width: r.width() as f32,
        height: r.height() as f32,
    }
}

/// An axis-aligned box as `[x0, y0, x1, y1]`.
type Bounds = [f32; 4];

fn intersects(a: &Bounds, b: &Bounds) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// The box around `b` mapped by `t`.
fn map_bounds(t: &Transform2D, b: Bounds) -> Bounds {
    if t.b == 0.0 && t.c == 0.0 {
        let (x0, x1) = (t.a * b[0] + t.e, t.a * b[2] + t.e);
        let (y0, y1) = (t.d * b[1] + t.f, t.d * b[3] + t.f);
        return [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
    }
    let corners = [(b[0], b[1]), (b[2], b[1]), (b[0], b[3]), (b[2], b[3])];
    let mut out = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for (x, y) in corners {
        let p = t.apply(oxideav_core::Point { x, y });
        out[0] = out[0].min(p.x);
        out[1] = out[1].min(p.y);
        out[2] = out[2].max(p.x);
        out[3] = out[3].max(p.y);
    }
    out
}

/// The box around every point and control point of `path` (curves stay
/// inside their control points; an arc is padded by its radius).
fn path_bounds(path: &Path) -> Option<Bounds> {
    let mut b = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    let mut add = |p: &oxideav_core::Point, pad: f32| {
        b[0] = b[0].min(p.x - pad);
        b[1] = b[1].min(p.y - pad);
        b[2] = b[2].max(p.x + pad);
        b[3] = b[3].max(p.y + pad);
    };
    for c in &path.commands {
        match c {
            PathCommand::MoveTo(p) | PathCommand::LineTo(p) => add(p, 0.0),
            PathCommand::QuadCurveTo { control, end } => {
                add(control, 0.0);
                add(end, 0.0);
            }
            PathCommand::CubicCurveTo { c1, c2, end } => {
                add(c1, 0.0);
                add(c2, 0.0);
                add(end, 0.0);
            }
            PathCommand::ArcTo { rx, ry, end, .. } => add(end, rx.abs().max(ry.abs()) * 2.0),
            PathCommand::Close => {}
            // A future command: no bound.
            #[allow(unreachable_patterns)]
            _ => return None,
        }
    }
    (b[0] <= b[2]).then_some(b)
}

/// Whether `path` is an upright rectangle: a move, three lines along the
/// axes and a close (what [`Scene::push_clip`] makes).
fn is_rect(path: &Path) -> bool {
    let [
        PathCommand::MoveTo(a),
        PathCommand::LineTo(b),
        PathCommand::LineTo(c),
        PathCommand::LineTo(d),
        PathCommand::Close,
    ] = path.commands.as_slice()
    else {
        return false;
    };
    [a, b, c, d, a]
        .windows(2)
        .all(|w| w[0].x == w[1].x || w[0].y == w[1].y)
}

/// The inverse of `t`, if it has one.
fn invert(t: &Transform2D) -> Option<Transform2D> {
    let det = t.a * t.d - t.b * t.c;
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let (a, b, c, d) = (t.d / det, -t.b / det, -t.c / det, t.a / det);
    Some(Transform2D {
        a,
        b,
        c,
        d,
        e: -(a * t.e + c * t.f),
        f: -(b * t.e + d * t.f),
    })
}

/// Slack for rounding when testing a primitive's box against the view, in
/// logical pixels. (Coverage comes only from geometry inside a pixel, so a box
/// that ends where the view starts adds nothing to it.)
const MARGIN: f32 = 0.01;

/// Lowers nodes for an area of a scene. `view` is the area in the scene's
/// root (logical-pixel) coordinates, or `None` to keep everything.
pub(crate) struct Lower<'a> {
    pub deferred: &'a [Deferred],
    pub view: Option<Bounds>,
}

impl Lower<'_> {
    /// The area `view` as a [`Lower`] view, padded for rounding.
    pub(crate) fn view_of(view: Rect) -> Bounds {
        [
            view.min_x() as f32 - MARGIN,
            view.min_y() as f32 - MARGIN,
            view.max_x() as f32 + MARGIN,
            view.max_y() as f32 + MARGIN,
        ]
    }

    /// Lower `nodes`, drawn through `t` (their coordinates to the root's).
    pub(crate) fn nodes(&self, nodes: &[Node], t: &Transform2D) -> Vec<Node> {
        nodes.iter().filter_map(|n| self.node(n, t)).collect()
    }

    fn visible(&self, local: Option<Bounds>, t: &Transform2D) -> bool {
        match (&self.view, local) {
            (Some(view), Some(b)) => intersects(&map_bounds(t, b), view),
            _ => true,
        }
    }

    fn node(&self, node: &Node, t: &Transform2D) -> Option<Node> {
        match node {
            Node::Path(p) => {
                if self.view.is_some() {
                    let pad = p.stroke.as_ref().map_or(0.0, |s| {
                        // Miter corners can reach `miter_limit / 2` widths
                        // out; square caps √2 / 2.
                        s.width * s.miter_limit.max(1.5) / 2.0
                    });
                    let b = path_bounds(&p.path)
                        .map(|b| [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad]);
                    if !self.visible(b, t) {
                        return None;
                    }
                }
                Some(node.clone())
            }
            Node::Image(img) => match deferred_index(img) {
                Some(i) => self.materialize(i, img, t),
                None => {
                    let local = t.compose(&img.transform);
                    let r = img.bounds;
                    self.visible(Some([r.x, r.y, r.x + r.width, r.y + r.height]), &local)
                        .then(|| node.clone())
                }
            },
            Node::Group(g) => {
                let local = t.compose(&g.transform);
                if self.view.is_some()
                    && let Some(clip) = &g.clip
                    && !self.visible(path_bounds(clip), &local)
                {
                    return None;
                }
                if g.cache_key.is_some() {
                    // A cached bitmap (a glyph): keep or drop it whole.
                    if self.view.is_some() && !self.visible(subtree_bounds(&g.children), &local) {
                        return None;
                    }
                    return Some(node.clone());
                }
                let children = self.nodes(&g.children, &local);
                if children.is_empty() && !g.children.is_empty() {
                    return None;
                }
                // A rect clip around the whole view clips nothing the view
                // doesn't, and costs a mask the size of the area for every
                // primitive under it.
                let clip = g.clip.clone().filter(|c| !self.covers_view(c, &local));
                Some(Node::Group(Group {
                    transform: g.transform,
                    opacity: g.opacity,
                    clip,
                    children,
                    cache_key: None,
                }))
            }
            // Soft masks and future node kinds: kept as they are.
            _ => Some(node.clone()),
        }
    }

    /// Whether `clip`, drawn through `t`, is an upright rectangle holding the
    /// whole view.
    fn covers_view(&self, clip: &Path, t: &Transform2D) -> bool {
        let Some(view) = self.view else {
            return false;
        };
        if t.b != 0.0 || t.c != 0.0 || !is_rect(clip) {
            return false;
        }
        let Some(b) = path_bounds(clip) else {
            return false;
        };
        let r = map_bounds(t, b);
        r[0] <= view[0] && r[1] <= view[1] && r[2] >= view[2] && r[3] >= view[3]
    }

    /// Turn placeholder `img` for deferred entry `index` into nodes.
    fn materialize(&self, index: usize, img: &ImageRef, t: &Transform2D) -> Option<Node> {
        let r = img.bounds;
        let local = t.compose(&img.transform);
        if !self.visible(Some([r.x, r.y, r.x + r.width, r.y + r.height]), &local) {
            return None;
        }
        match self.deferred.get(index)? {
            Deferred::Image(pixmap) => Some(self.image(pixmap, img, &local)),
            Deferred::Canvas {
                bounds,
                paint,
                scale,
            } => self.canvas(*bounds, paint, *scale, &local),
        }
    }

    /// `pixmap` as an image node in place of `img`: only the part that shows
    /// in the view (plus a margin for filtering) when it's drawn upright.
    fn image(&self, pixmap: &Pixmap, img: &ImageRef, local: &Transform2D) -> Node {
        let (pw, ph) = (pixmap.size().width, pixmap.size().height);
        let b = img.bounds;
        let full = || {
            Node::Image(ImageRef {
                frame: Box::new(pixmap.to_video_frame()),
                bounds: b,
                transform: img.transform,
            })
        };
        let Some(view) = self.view else {
            return full();
        };
        if pw == 0 || ph == 0 || b.width <= 0.0 || b.height <= 0.0 {
            return full();
        }
        if local.b != 0.0 || local.c != 0.0 {
            return full();
        }
        let Some(inv) = invert(local) else {
            return full();
        };
        // The view in the image's own coordinates, then in source pixels.
        let v = map_bounds(&inv, view);
        let (sx, sy) = (pw as f32 / b.width, ph as f32 / b.height);
        const PAD: f32 = 2.0;
        let col0 = (((v[0] - b.x) * sx).floor() - PAD).clamp(0.0, pw as f32) as u32;
        let col1 = (((v[2] - b.x) * sx).ceil() + PAD).clamp(0.0, pw as f32) as u32;
        let row0 = (((v[1] - b.y) * sy).floor() - PAD).clamp(0.0, ph as f32) as u32;
        let row1 = (((v[3] - b.y) * sy).ceil() + PAD).clamp(0.0, ph as f32) as u32;
        if (col0, row0, col1, row1) == (0, 0, pw, ph) || col1 <= col0 || row1 <= row0 {
            return full();
        }
        let (cw, stride) = ((col1 - col0) as usize * 4, pixmap.stride());
        let src = pixmap.as_bytes();
        let mut data = Vec::with_capacity(cw * (row1 - row0) as usize);
        for y in row0 as usize..row1 as usize {
            let at = y * stride + col0 as usize * 4;
            data.extend_from_slice(&src[at..at + cw]);
        }
        Node::Image(ImageRef {
            frame: Box::new(VideoFrame {
                pts: None,
                planes: vec![VideoPlane { stride: cw, data }],
            }),
            bounds: OxRect {
                x: b.x + col0 as f32 / sx,
                y: b.y + row0 as f32 / sy,
                width: (col1 - col0) as f32 / sx,
                height: (row1 - row0) as f32 / sy,
            },
            transform: img.transform,
        })
    }

    /// Run a canvas's paint callback for the part of `bounds` in the view,
    /// and lower what it drew, clipped to `bounds`.
    fn canvas(
        &self,
        bounds: Rect,
        paint: &CanvasPaint,
        scale: f64,
        local: &Transform2D,
    ) -> Option<Node> {
        // The part to paint, in the canvas's coordinates.
        let dirty = match (&self.view, invert(local)) {
            (Some(view), Some(inv)) => {
                let [x0, y0, x1, y1] = *view;
                let exact = [x0 + MARGIN, y0 + MARGIN, x1 - MARGIN, y1 - MARGIN];
                let v = map_bounds(&inv, exact);
                let v = Rect::from_xywh(
                    v[0] as f64,
                    v[1] as f64,
                    (v[2] - v[0]) as f64,
                    (v[3] - v[1]) as f64,
                );
                v.intersection(bounds)?
            }
            _ => bounds,
        };
        let origin = bounds.origin;
        let mut scene = Scene::new(bounds.size);
        scene.set_scale(scale);
        {
            let dirty = dirty.translate(Vec2::new(-origin.x, -origin.y));
            let mut canvas = Canvas::new(&mut scene, dirty);
            paint(&mut canvas);
        }
        let shift = Transform2D::translate(origin.x as f32, origin.y as f32);
        let inner = scene.lower_root(self.view, &local.compose(&shift));
        if inner.is_empty() {
            return None;
        }
        let clip = crate::render::scene::rect_path(bounds);
        let clip = (!self.covers_view(&clip, local)).then_some(clip);
        Some(Node::Group(Group {
            clip,
            children: vec![Node::Group(Group {
                transform: shift,
                children: inner,
                ..Group::new()
            })],
            ..Group::new()
        }))
    }
}

/// The box around everything in `nodes` (in their coordinates), or `None` if
/// some of it has no known bound.
fn subtree_bounds(nodes: &[Node]) -> Option<Bounds> {
    let mut out = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for n in nodes {
        let b = match n {
            Node::Path(p) => {
                let pad = p
                    .stroke
                    .as_ref()
                    .map_or(0.0, |s| s.width * s.miter_limit.max(1.5) / 2.0);
                let b = path_bounds(&p.path)?;
                [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad]
            }
            Node::Group(g) => map_bounds(&g.transform, subtree_bounds(&g.children)?),
            Node::Image(i) => {
                let r = i.bounds;
                map_bounds(&i.transform, [r.x, r.y, r.x + r.width, r.y + r.height])
            }
            _ => return None,
        };
        out = [
            out[0].min(b[0]),
            out[1].min(b[1]),
            out[2].max(b[2]),
            out[3].max(b[3]),
        ];
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{PhysicalSize, Point, ScaleFactor, Size};
    use crate::render::{Color, SoftwareRenderer};

    #[test]
    fn placeholders_round_trip_their_index() {
        let Node::Image(img) = placeholder(42, Rect::from_xywh(0.0, 0.0, 1.0, 1.0)) else {
            unreachable!()
        };
        assert_eq!(deferred_index(&img), Some(42));
    }

    #[test]
    fn inverse_undoes_a_transform() {
        let t = Transform2D::translate(5.0, -3.0).compose(&Transform2D::scale(2.0, 4.0));
        let inv = invert(&t).unwrap();
        let p = inv.apply(t.apply(oxideav_core::Point { x: 7.0, y: 9.0 }));
        assert!((p.x - 7.0).abs() < 1e-4 && (p.y - 9.0).abs() < 1e-4);
    }

    /// An area render keeps only what touches the area, and the image it
    /// draws is cropped to it — and still matches the full render there.
    #[test]
    fn an_area_render_culls_and_crops_and_matches() {
        let mut data = Vec::new();
        for y in 0..64u32 {
            for x in 0..64u32 {
                data.extend_from_slice(&[(x * 4) as u8, (y * 4) as u8, 128, 255]);
            }
        }
        let image = Arc::new(Pixmap::from_rgba8(PhysicalSize::new(64, 64), data));
        let make = || {
            let mut s = Scene::new(Size::new(100.0, 100.0));
            s.draw_image(Rect::from_xywh(10.0, 10.0, 64.0, 64.0), image.clone());
            s.fill_rect(Rect::from_xywh(80.0, 80.0, 10.0, 10.0), Color::WHITE);
            s
        };
        let view = Rect::from_xywh(20.0, 30.0, 10.0, 5.0);
        let scene = make();
        let root = scene.lower_root(Some(Lower::view_of(view)), &Transform2D::identity());
        assert_eq!(root.len(), 1, "the far rect is culled");
        let Node::Image(img) = &root[0] else {
            panic!("the image stays")
        };
        assert!(img.frame.planes[0].data.len() < 64 * 64 * 4 / 4, "cropped");

        let r = SoftwareRenderer::new().with_background(Color::BLACK);
        let full = r.render(make(), ScaleFactor::IDENTITY);
        let area = r.render_area(&make(), view, PhysicalSize::new(10, 5));
        for y in 0..5 {
            for x in 0..10 {
                assert_eq!(area.pixel(x, y), full.pixel(20 + x, 30 + y), "at {x},{y}");
            }
        }
        let _ = Point::ORIGIN;
    }
}
