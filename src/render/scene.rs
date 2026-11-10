//! A retained list of draw primitives, lowered to an `oxideav-core`
//! [`VectorFrame`] for rasterization.
//!
//! `stipple::core` rebuilds a `Scene` from the widget tree each time the visible
//! state changes; `stipple::render` then rasterizes and presents it. The scene
//! works entirely in **logical pixels** — DPI scaling is applied at raster
//! time via the frame's view box (see [`Scene::into_vector_frame`]).

use crate::geometry::{Affine, Point, Rect, Size};
use crate::render::convert::{to_ox_point, to_transform2d};
use crate::render::{Color, Pixmap};
use oxideav_core::{
    Group, ImageRef, Node, Paint, Path, PathNode, Point as OxPoint, Rect as OxRect, Rgba, Stroke,
    Transform2D, VectorFrame, ViewBox,
};
use std::sync::Arc;

/// Control-point offset for approximating a quarter circle with a cubic
/// Bézier (the standard `4/3·(√2 − 1)` "kappa" constant).
const KAPPA: f64 = 0.552_284_749_830_793_4;

/// A structured record of a scene primitive, kept alongside the lowered oxideav
/// nodes so a GPU backend can consume the scene without re-deriving primitives
/// from vector paths (the CPU rasterizer uses the nodes; the GPU path uses
/// these). See [`Scene::commands`].
#[derive(Clone, Debug, PartialEq)]
pub enum DrawCmd {
    /// A box: `radius` rounds the corners (0 = sharp); `border` > 0 strokes the
    /// outline at that width instead of filling.
    Rect {
        rect: Rect,
        color: Color,
        radius: f64,
        border: f64,
    },
    /// A single line of text at `origin` (top-left).
    Text {
        text: String,
        origin: Point,
        size: f64,
        color: Color,
    },
    /// Begin clipping subsequent primitives to `rect` (nests until [`PopClip`]).
    /// GPU backends map this to a scissor rectangle.
    ///
    /// [`PopClip`]: DrawCmd::PopClip
    PushClip(Rect),
    /// End the innermost clip region opened by [`PushClip`](DrawCmd::PushClip).
    PopClip,
    /// Begin a layer: subsequent primitives (until the matching [`PopLayer`])
    /// are drawn through `transform` (logical pixels in, logical pixels out,
    /// composed with any enclosing layer's) and composited **as a group** at
    /// `opacity` — rendered together at full opacity, then blended once, so
    /// overlapping primitives inside don't show through each other.
    ///
    /// [`PopLayer`]: DrawCmd::PopLayer
    PushLayer { transform: Affine, opacity: f32 },
    /// End the innermost layer opened by [`PushLayer`](DrawCmd::PushLayer).
    PopLayer,
    /// A reserved area for **embedded content** (a browser page, video, or any
    /// externally-rendered surface), identified by `id`. The compositor draws
    /// the content here: the CPU path blits the registered content [`Pixmap`]
    /// over the placeholder fill after rasterizing; a GPU backend samples the
    /// imported (dma-buf / `IOSurface` / shared-handle) texture into this rect.
    /// See [`Scene::fill_viewport`].
    Viewport { rect: Rect, id: u64 },
    /// A bitmap drawn into `rect`: an OS-drawn control under the native theme.
    Image { rect: Rect, image: Arc<Pixmap> },
}

/// A clip region or layer under construction: the primitives emitted while it
/// is open, plus the clip path, transform and opacity applied to them as a
/// group when it closes. The scene keeps a stack of these so
/// [`Scene::push_clip`]/[`Scene::pop_clip`] and
/// [`Scene::push_layer`]/[`Scene::pop_layer`] can nest.
#[derive(Clone, Debug)]
struct ClipFrame {
    clip: Option<Path>,
    /// `true` for a [`Scene::push_layer`] frame, `false` for a clip (or the base).
    layer: bool,
    transform: Transform2D,
    opacity: f32,
    nodes: Vec<Node>,
}

impl ClipFrame {
    fn base() -> Self {
        Self {
            clip: None,
            layer: false,
            transform: Transform2D::identity(),
            opacity: 1.0,
            nodes: Vec::new(),
        }
    }
}

/// A builder of vector draw primitives in logical-pixel space.
#[derive(Clone, Debug)]
pub struct Scene {
    logical_size: Size,
    // A stack of clip frames; the base (index 0) is the unclipped root. Every
    // primitive is emitted into the top frame; `pop_clip` wraps a frame's nodes
    // in a clipped `Group` and folds it into the frame below.
    stack: Vec<ClipFrame>,
    commands: Vec<DrawCmd>,
    /// Physical pixels per logical pixel of the frame this scene is for, so
    /// bitmaps can be produced at the resolution they'll be shown at.
    scale: f64,
}

impl Scene {
    /// Create an empty scene covering `logical_size`.
    pub fn new(logical_size: Size) -> Self {
        Self {
            logical_size,
            stack: vec![ClipFrame::base()],
            commands: Vec::new(),
            scale: 1.0,
        }
    }

    /// Set the physical-per-logical pixel scale this scene will be shown at.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
    }

    /// The physical-per-logical pixel scale this scene will be shown at.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// Draw `image` (straight-alpha RGBA) into `rect`.
    pub fn draw_image(&mut self, rect: Rect, image: Arc<Pixmap>) {
        self.emit(Node::Image(ImageRef {
            frame: Box::new(image.to_video_frame()),
            bounds: OxRect {
                x: rect.min_x() as f32,
                y: rect.min_y() as f32,
                width: rect.width() as f32,
                height: rect.height() as f32,
            },
            transform: Transform2D::identity(),
        }));
        self.commands.push(DrawCmd::Image { rect, image });
    }

    /// Emit a node into the current (innermost) clip frame.
    fn emit(&mut self, node: Node) {
        // The stack always has at least the base frame.
        self.stack.last_mut().unwrap().nodes.push(node);
    }

    /// Begin clipping subsequently-emitted primitives to `rect`; nests until the
    /// matching [`pop_clip`](Scene::pop_clip). Used by scroll containers to mask
    /// overflowing content to the viewport.
    pub fn push_clip(&mut self, rect: Rect) {
        self.stack.push(ClipFrame {
            clip: Some(rect_path(rect)),
            ..ClipFrame::base()
        });
        self.commands.push(DrawCmd::PushClip(rect));
    }

    /// End the innermost clip region opened by [`push_clip`](Scene::push_clip),
    /// folding its primitives into a clipped group. A no-op if none is open
    /// (or the innermost open region is a layer).
    pub fn pop_clip(&mut self) {
        if self.stack.len() <= 1 || self.stack.last().is_some_and(|f| f.layer) {
            return; // unbalanced pop — ignore rather than drop the base frame
        }
        self.close_frame();
        self.commands.push(DrawCmd::PopClip);
    }

    /// Begin a layer: primitives emitted until the matching
    /// [`pop_layer`](Scene::pop_layer) are drawn through `transform` (mapping
    /// logical pixels to logical pixels, inside any enclosing layer's) and
    /// composited as a group at `opacity` (`0.0..=1.0`): the layer's content
    /// is rendered at full opacity on its own, then blended once, so
    /// overlapping primitives inside it don't show through each other. Layers
    /// and clips nest freely; a clip opened inside a layer is transformed with
    /// it.
    pub fn push_layer(&mut self, transform: Affine, opacity: f32) {
        let opacity = if opacity.is_nan() {
            1.0
        } else {
            opacity.clamp(0.0, 1.0)
        };
        self.stack.push(ClipFrame {
            layer: true,
            transform: to_transform2d(transform),
            opacity,
            ..ClipFrame::base()
        });
        self.commands
            .push(DrawCmd::PushLayer { transform, opacity });
    }

    /// End the innermost layer opened by [`push_layer`](Scene::push_layer). A
    /// no-op if none is open (or the innermost open region is a clip).
    pub fn pop_layer(&mut self) {
        if self.stack.len() <= 1 || self.stack.last().is_some_and(|f| !f.layer) {
            return;
        }
        self.close_frame();
        self.commands.push(DrawCmd::PopLayer);
    }

    /// Whether any layer pushed so far moves things or makes them translucent
    /// (a [`DrawCmd::PushLayer`] with a non-identity transform or an opacity
    /// below 1). Backends that can't draw layers natively fall back on this.
    pub fn has_effects(&self) -> bool {
        self.commands.iter().any(|c| {
            matches!(c, DrawCmd::PushLayer { transform, opacity }
                if !transform.is_identity() || *opacity < 1.0)
        })
    }

    /// Pop the innermost frame and fold its nodes, as one group carrying its
    /// clip, transform and opacity, into the frame below.
    fn close_frame(&mut self) {
        let frame = self.stack.pop().unwrap();
        let group = Group {
            children: frame.nodes,
            clip: frame.clip,
            transform: frame.transform,
            opacity: frame.opacity,
            ..Group::new()
        };
        self.emit(Node::Group(group));
    }

    /// The structured draw commands recorded by the typed helpers (for a GPU
    /// backend; the CPU rasterizer uses the lowered nodes instead).
    pub fn commands(&self) -> &[DrawCmd] {
        &self.commands
    }

    /// The scene's extent in logical pixels.
    #[inline]
    pub fn logical_size(&self) -> Size {
        self.logical_size
    }

    /// Number of top-level primitives queued (at the root clip level).
    #[inline]
    pub fn len(&self) -> usize {
        self.stack[0].nodes.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.stack.iter().all(|f| f.nodes.is_empty())
    }

    /// Stroke the outline of a rectangle with rounded corners.
    pub fn stroke_round_rect(&mut self, rect: Rect, radius: f64, color: Color, width: f64) {
        let path = round_rect_path(rect, radius);
        let stroke = Stroke::solid(width as f32, Rgba::from(color));
        self.emit(Node::Path(PathNode::new(path).with_stroke(stroke)));
        self.commands.push(DrawCmd::Rect {
            rect,
            color,
            radius,
            border: width,
        });
    }

    /// Fill an axis-aligned rectangle with a solid color.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        let path = rect_path(rect);
        self.emit(Node::Path(
            PathNode::new(path).with_fill(Paint::Solid(color.into())),
        ));
        self.commands.push(DrawCmd::Rect {
            rect,
            color,
            radius: 0.0,
            border: 0.0,
        });
    }

    /// Fill a rectangle with rounded corners (corner `radius` in logical
    /// pixels, clamped to half the shorter side).
    pub fn fill_round_rect(&mut self, rect: Rect, radius: f64, color: Color) {
        let path = round_rect_path(rect, radius);
        self.emit(Node::Path(
            PathNode::new(path).with_fill(Paint::Solid(color.into())),
        ));
        self.commands.push(DrawCmd::Rect {
            rect,
            color,
            radius,
            border: 0.0,
        });
    }

    /// Stroke the outline of a rectangle with the given line `width`.
    pub fn stroke_rect(&mut self, rect: Rect, color: Color, width: f64) {
        let path = rect_path(rect);
        let stroke = Stroke::solid(width as f32, Rgba::from(color));
        self.emit(Node::Path(PathNode::new(path).with_stroke(stroke)));
        self.commands.push(DrawCmd::Rect {
            rect,
            color,
            radius: 0.0,
            border: width,
        });
    }

    /// Reserve an embedded-content area `rect` for the viewport `id`, painting a
    /// solid `placeholder` so the region is visible before any content is
    /// composited in. Records a [`DrawCmd::Viewport`] so a GPU backend can
    /// texture the rect from an imported content surface instead, and so the app
    /// can locate the rect to blit registered CPU content over the placeholder.
    pub fn fill_viewport(&mut self, rect: Rect, id: u64, placeholder: Color) {
        let path = rect_path(rect);
        self.emit(Node::Path(
            PathNode::new(path).with_fill(Paint::Solid(placeholder.into())),
        ));
        self.commands.push(DrawCmd::Viewport { rect, id });
    }

    /// Record a text draw command (the glyph nodes are pushed separately by
    /// [`Scene::fill_text`](crate::render::Scene::fill_text)).
    pub(crate) fn record_text(&mut self, text: &str, origin: Point, size: f64, color: Color) {
        self.commands.push(DrawCmd::Text {
            text: text.to_string(),
            origin,
            size,
            color,
        });
    }

    /// Escape hatch: push a pre-built oxideav scene-graph node. Lets callers
    /// emit text runs, images, gradients, or clips that the typed helpers
    /// don't yet cover.
    pub fn push_node(&mut self, node: Node) {
        self.emit(node);
    }

    /// Lower the scene to an `oxideav-core` [`VectorFrame`].
    ///
    /// The frame carries a view box equal to the logical size, so the
    /// rasterizer maps logical pixels onto whatever physical canvas size the
    /// renderer was constructed with — that mapping *is* the DPI scale.
    pub fn into_vector_frame(self) -> VectorFrame {
        let size = self.logical_size;
        self.into_vector_frame_view(Rect::from_xywh(0.0, 0.0, size.width, size.height))
    }

    /// Lower to a frame whose view box is `view` — a logical-pixel sub-rect of
    /// the scene. A renderer sized to that region's physical pixels then
    /// rasterizes only the region; content outside it is clipped by the canvas
    /// bounds. Used for area-limited repaints (see `SoftwareRenderer::render_region`).
    pub fn into_vector_frame_region(self, view: Rect) -> VectorFrame {
        self.into_vector_frame_view(view)
    }

    /// Shared lowering: close any open clips, wrap the root nodes in a group,
    /// and attach `view` as the frame's view box.
    fn into_vector_frame_view(mut self, view: Rect) -> VectorFrame {
        // Defensively close any clip regions or layers the caller left open.
        while self.stack.len() > 1 {
            if self.stack.last().is_some_and(|f| f.layer) {
                self.pop_layer();
            } else {
                self.pop_clip();
            }
        }
        let (w, h) = (view.width() as f32, view.height() as f32);
        let root = Group {
            children: self.stack.pop().unwrap().nodes,
            ..Group::new()
        };
        VectorFrame::new(w, h)
            .with_view_box(ViewBox {
                min_x: view.min_x() as f32,
                min_y: view.min_y() as f32,
                width: w,
                height: h,
            })
            .with_root(root)
    }
}

fn rect_path(rect: Rect) -> Path {
    let (x0, y0, x1, y1) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let mut p = Path::new();
    p.move_to(OxPoint {
        x: x0 as f32,
        y: y0 as f32,
    });
    p.line_to(OxPoint {
        x: x1 as f32,
        y: y0 as f32,
    });
    p.line_to(OxPoint {
        x: x1 as f32,
        y: y1 as f32,
    });
    p.line_to(OxPoint {
        x: x0 as f32,
        y: y1 as f32,
    });
    p.close();
    p
}

fn round_rect_path(rect: Rect, radius: f64) -> Path {
    let r = radius.max(0.0).min(rect.width().min(rect.height()) / 2.0);
    if r <= 0.0 {
        return rect_path(rect);
    }
    let (x0, y0, x1, y1) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let k = r * KAPPA;
    use crate::geometry::Point as P;
    let mut p = Path::new();
    // Start at the top edge just right of the top-left corner, go clockwise.
    p.move_to(to_ox_point(P::new(x0 + r, y0)));
    p.line_to(to_ox_point(P::new(x1 - r, y0)));
    p.cubic_to(
        to_ox_point(P::new(x1 - r + k, y0)),
        to_ox_point(P::new(x1, y0 + r - k)),
        to_ox_point(P::new(x1, y0 + r)),
    );
    p.line_to(to_ox_point(P::new(x1, y1 - r)));
    p.cubic_to(
        to_ox_point(P::new(x1, y1 - r + k)),
        to_ox_point(P::new(x1 - r + k, y1)),
        to_ox_point(P::new(x1 - r, y1)),
    );
    p.line_to(to_ox_point(P::new(x0 + r, y1)));
    p.cubic_to(
        to_ox_point(P::new(x0 + r - k, y1)),
        to_ox_point(P::new(x0, y1 - r + k)),
        to_ox_point(P::new(x0, y1 - r)),
    );
    p.line_to(to_ox_point(P::new(x0, y0 + r)));
    p.cubic_to(
        to_ox_point(P::new(x0, y0 + r - k)),
        to_ox_point(P::new(x0 + r - k, y0)),
        to_ox_point(P::new(x0 + r, y0)),
    );
    p.close();
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Point;

    /// An image lands where it's placed, keeps its colors, and blends its alpha
    /// over what's underneath (the path OS-drawn controls take).
    #[test]
    fn draw_image_composites_straight_alpha() {
        use crate::geometry::{PhysicalSize, ScaleFactor};
        use crate::render::software::SoftwareRenderer;

        let size = PhysicalSize::new(4, 4);
        let mut data = Vec::new();
        for _ in 0..8 {
            data.extend_from_slice(&[200, 100, 50, 255]); // top half: opaque
        }
        for _ in 0..8 {
            data.extend_from_slice(&[200, 100, 50, 0]); // bottom half: clear
        }
        let image = Arc::new(Pixmap::from_rgba8(size, data));
        let mut scene = Scene::new(Size::new(8.0, 8.0));
        scene.fill_rect(Rect::from_xywh(0.0, 0.0, 8.0, 8.0), Color::rgb(0, 0, 255));
        scene.draw_image(Rect::from_xywh(2.0, 2.0, 4.0, 4.0), image);
        assert!(matches!(
            scene.commands().last(),
            Some(DrawCmd::Image { .. })
        ));

        let out = SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);
        assert_eq!(out.pixel(0, 0), Some([0, 0, 255, 255]), "outside the image");
        assert_eq!(out.pixel(3, 2), Some([200, 100, 50, 255]), "opaque part");
        assert_eq!(out.pixel(3, 5), Some([0, 0, 255, 255]), "clear part");
    }

    #[test]
    fn scene_lowers_to_frame_with_viewbox() {
        let mut scene = Scene::new(Size::new(200.0, 100.0));
        scene.fill_rect(Rect::from_xywh(10.0, 10.0, 50.0, 50.0), Color::WHITE);
        assert_eq!(scene.len(), 1);
        let frame = scene.into_vector_frame();
        assert_eq!((frame.width, frame.height), (200.0, 100.0));
        let vb = frame.view_box.expect("view box set");
        assert_eq!((vb.width, vb.height), (200.0, 100.0));
        assert_eq!(frame.root.children.len(), 1);
    }

    #[test]
    fn records_structured_draw_commands() {
        let mut scene = Scene::new(Size::new(100.0, 100.0));
        scene.fill_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), Color::WHITE);
        scene.fill_round_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), 4.0, Color::BLACK);
        scene.stroke_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), Color::WHITE, 2.0);
        let cmds = scene.commands();
        assert_eq!(cmds.len(), 3);
        assert!(
            matches!(cmds[0], DrawCmd::Rect { radius, border, .. } if radius == 0.0 && border == 0.0)
        );
        assert!(matches!(cmds[1], DrawCmd::Rect { radius, .. } if radius == 4.0));
        assert!(matches!(cmds[2], DrawCmd::Rect { border, .. } if border == 2.0));
    }

    #[test]
    fn push_pop_clip_nests_a_clipped_group() {
        let mut scene = Scene::new(Size::new(200.0, 200.0));
        scene.fill_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), Color::WHITE); // root level
        scene.push_clip(Rect::from_xywh(20.0, 20.0, 50.0, 50.0));
        scene.fill_rect(Rect::from_xywh(25.0, 25.0, 100.0, 100.0), Color::BLACK); // clipped
        scene.fill_rect(Rect::from_xywh(30.0, 30.0, 5.0, 5.0), Color::BLACK); // clipped
        scene.pop_clip();
        // Root has the first rect plus one clipped group.
        assert_eq!(scene.len(), 2);
        // Commands record the clip bracket around the two inner rects.
        let cmds = scene.commands();
        assert!(matches!(cmds[1], DrawCmd::PushClip(_)));
        assert!(matches!(cmds[4], DrawCmd::PopClip));
        let frame = scene.into_vector_frame();
        // Root group: rect + clipped group.
        assert_eq!(frame.root.children.len(), 2);
        let clipped = match &frame.root.children[1] {
            Node::Group(g) => g,
            _ => panic!("expected a clipped group as the second child"),
        };
        assert!(clipped.clip.is_some(), "nested group carries the clip path");
        assert_eq!(clipped.children.len(), 2, "two clipped rects");
    }

    /// Render `scene` at 1x over an opaque black background.
    fn raster(scene: Scene) -> crate::render::Pixmap {
        use crate::geometry::ScaleFactor;
        use crate::render::software::SoftwareRenderer;
        SoftwareRenderer::new()
            .with_background(Color::BLACK)
            .render(scene, ScaleFactor::IDENTITY)
    }

    const RED: Color = Color::rgb(255, 0, 0);

    #[test]
    fn a_layer_translates_what_it_holds() {
        use crate::geometry::Vec2;
        let mut scene = Scene::new(Size::new(40.0, 40.0));
        scene.push_layer(Affine::translate(Vec2::new(20.0, 20.0)), 1.0);
        scene.fill_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), RED);
        scene.pop_layer();
        assert!(scene.has_effects());
        let out = raster(scene);
        assert_eq!(out.pixel(25, 25), Some([255, 0, 0, 255]));
        assert_eq!(out.pixel(5, 5), Some([0, 0, 0, 255]));
    }

    #[test]
    fn a_layer_rotates_about_a_point() {
        use crate::geometry::Vec2;
        // A horizontal bar through the middle, turned 90° about the center,
        // stands upright.
        let about = Affine::translate(Vec2::new(-20.0, -20.0))
            .then(Affine::rotate(std::f64::consts::FRAC_PI_2))
            .then(Affine::translate(Vec2::new(20.0, 20.0)));
        let mut scene = Scene::new(Size::new(40.0, 40.0));
        scene.push_layer(about, 1.0);
        scene.fill_rect(Rect::from_xywh(10.0, 18.0, 20.0, 4.0), RED);
        scene.pop_layer();
        let out = raster(scene);
        assert_eq!(out.pixel(20, 12), Some([255, 0, 0, 255]), "now vertical");
        assert_eq!(out.pixel(20, 27), Some([255, 0, 0, 255]), "now vertical");
        assert_eq!(
            out.pixel(12, 20),
            Some([0, 0, 0, 255]),
            "no longer horizontal"
        );
    }

    #[test]
    fn nested_layers_compose_inner_first() {
        use crate::geometry::Vec2;
        let mut scene = Scene::new(Size::new(40.0, 40.0));
        scene.push_layer(Affine::translate(Vec2::new(20.0, 0.0)), 1.0);
        scene.push_layer(Affine::scale(2.0, 2.0), 1.0);
        // (1, 1, 2, 2) → scaled to (2, 2, 4, 4) → moved to (22, 2, 4, 4).
        scene.fill_rect(Rect::from_xywh(1.0, 1.0, 2.0, 2.0), RED);
        scene.pop_layer();
        scene.pop_layer();
        let out = raster(scene);
        assert_eq!(out.pixel(23, 3), Some([255, 0, 0, 255]));
        assert_eq!(out.pixel(25, 5), Some([255, 0, 0, 255]));
        assert_eq!(out.pixel(1, 1), Some([0, 0, 0, 255]));
        assert_eq!(out.pixel(27, 7), Some([0, 0, 0, 255]));
    }

    /// Two overlapping opaque children at 50% group opacity read as one
    /// half-transparent shape: the overlap is no darker than the rest.
    #[test]
    fn layer_opacity_composites_as_a_group() {
        let mut scene = Scene::new(Size::new(40.0, 20.0));
        scene.push_layer(Affine::IDENTITY, 0.5);
        scene.fill_rect(Rect::from_xywh(0.0, 0.0, 25.0, 20.0), RED);
        scene.fill_rect(Rect::from_xywh(15.0, 0.0, 25.0, 20.0), RED);
        scene.pop_layer();
        assert!(scene.has_effects());
        let out = raster(scene);
        let alone = out.pixel(5, 10).unwrap();
        let overlap = out.pixel(20, 10).unwrap();
        assert_eq!(alone, overlap, "no seam where the children overlap");
        assert!(
            (120..=136).contains(&alone[0]),
            "half red over black: {alone:?}"
        );
    }

    #[test]
    fn a_clip_inside_a_layer_moves_with_it() {
        use crate::geometry::Vec2;
        let mut scene = Scene::new(Size::new(40.0, 40.0));
        scene.push_layer(Affine::translate(Vec2::new(20.0, 0.0)), 1.0);
        scene.push_clip(Rect::from_xywh(0.0, 0.0, 10.0, 10.0));
        scene.fill_rect(Rect::from_xywh(0.0, 0.0, 40.0, 40.0), RED);
        scene.pop_clip();
        scene.pop_layer();
        let out = raster(scene);
        assert_eq!(
            out.pixel(25, 5),
            Some([255, 0, 0, 255]),
            "inside the moved clip"
        );
        assert_eq!(out.pixel(5, 5), Some([0, 0, 0, 255]), "where the clip was");
        assert_eq!(out.pixel(25, 15), Some([0, 0, 0, 255]), "below the clip");
    }

    #[test]
    fn mismatched_pops_are_ignored_and_open_layers_close() {
        let mut scene = Scene::new(Size::new(10.0, 10.0));
        scene.push_layer(Affine::IDENTITY, 0.5);
        scene.pop_clip(); // not a clip: ignored
        scene.fill_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), RED);
        assert_eq!(scene.commands().len(), 2, "the stray pop records nothing");
        let frame = scene.into_vector_frame();
        let Node::Group(g) = &frame.root.children[0] else {
            panic!("the open layer closes into a group");
        };
        assert_eq!(g.opacity, 0.5);
        assert_eq!(g.children.len(), 1);
    }

    #[test]
    fn fill_viewport_paints_placeholder_and_records_command() {
        let mut scene = Scene::new(Size::new(200.0, 100.0));
        let r = Rect::from_xywh(10.0, 10.0, 80.0, 60.0);
        scene.fill_viewport(r, 7, Color::rgb(0x20, 0x24, 0x2c));
        // A placeholder fill node is emitted so the CPU raster shows the area.
        assert_eq!(scene.len(), 1);
        // The structured command carries the rect + id for the compositor.
        let cmds = scene.commands();
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], DrawCmd::Viewport { id, rect } if id == 7 && rect == r));
    }

    #[test]
    fn zero_radius_round_rect_is_plain_rect() {
        let path = round_rect_path(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), 0.0);
        // Plain rect: move + 3 lines + close == 5 commands.
        assert_eq!(path.commands.len(), 5);
    }

    #[test]
    fn round_rect_has_corner_curves() {
        let path = round_rect_path(Rect::from_xywh(0.0, 0.0, 20.0, 20.0), 4.0);
        let cubics = path
            .commands
            .iter()
            .filter(|c| matches!(c, oxideav_core::PathCommand::CubicCurveTo { .. }))
            .count();
        assert_eq!(cubics, 4);
        let _ = Point::ORIGIN;
    }
}
