//! Lowering a [`Scene`] for a GPU backend: the scene's node tree, walked in
//! paint order, becomes a flat list of device-space draw operations a backend
//! executes with a handful of shaders.
//!
//! The walk follows the CPU rasterizer (`oxideav-raster`) node for node, so the
//! two agree: the same transforms compose, the same strokes are built
//! (`stroke_to_fill_path`, at the same width), clips and group opacity nest the
//! same way. What becomes what:
//!
//! - **Boxes.** A solid rect or rounded rect (the paths [`Scene::fill_rect`],
//!   [`Scene::fill_round_rect`] and their strokes make) drawn upright becomes an
//!   [`Op::Box`], shaded analytically: exact pixel coverage for a sharp box, a
//!   signed distance for a rounded one.
//! - **Paths.** Every other solid fill or stroke is flattened (and stroked) on
//!   the CPU exactly as the rasterizer does, then filled on the GPU by
//!   stencil-and-cover ([`Op::Path`]).
//! - **Images.** A bitmap ([`Scene::draw_image`]: OS-drawn controls, decoded
//!   images, SVG rasters) becomes an [`Op::Image`] that keeps the scene's
//!   `Arc<Pixmap>`, so a backend can cache its texture by the image's
//!   identity: an image that doesn't change is uploaded once.
//! - **Glyphs.** Glyph groups (cache-keyed groups) are rasterized once per
//!   glyph, size, color and sub-pixel position on the CPU, kept in a
//!   [`RasterCache`] across frames, packed into its glyph atlas
//!   (`super::atlas`), and drawn as regions of the atlas page: so text is
//!   drawn from the scene's own glyph outlines, whatever its font, from one
//!   texture.
//! - **Layers.** A group with an opacity below 1 is an offscreen layer
//!   ([`Op::PushLayer`] / [`Op::PopLayer`]), composited once; transforms are
//!   applied to the geometry.
//! - **Clips.** An upright rect clip is a scissor rectangle, any other convex
//!   clip a stencil polygon ([`Op::PushClip`]).
//! - **Canvases** of a window's frame are kept as textures, updated where the
//!   frame is damaged (`gpu::canvas`); others run their paint callback into a
//!   scene that is walked in turn.
//! - Anything else (gradients, soft masks, raw `oxideav` image nodes, concave
//!   clips) is rasterized on the CPU for just its own bounds and drawn as an
//!   image: a local fallback, never the whole frame.
//!
//! The operations are then batched (`super::batch`): runs of boxes and
//! textured quads that share a texture become one draw call each.

use super::atlas::{self, Atlas, AtlasPage};
use super::batch::{self, Cmd};
pub(crate) use super::canvas::TextureUpdate;
use super::canvas::{CanvasCache, CanvasFrame};
use crate::core::CanvasId;
use crate::geometry::{PhysicalSize, Rect};
use crate::render::Filter;
use crate::render::glyph_mask::FastHasher;
use crate::render::glyphs::GlyphDraw;
use crate::render::lower::{self, Bounds, Deferred, Offscreen};
use crate::render::scene::{rect_path, round_rect_path};
use crate::render::sprite::ExternalDraw;
use crate::render::sprite::SpriteDraw;
use crate::render::{CanvasPaint, Color, ExternalImage, Pixmap, Scene};
use oxideav_core::{
    FillRule, Group, LineJoin, Node, Paint, Path, PathCommand, PathNode, Rgba, Stroke, Transform2D,
};
use oxideav_raster::{FlatContour, Renderer, flatten_path, stroke_to_fill_path};
use std::collections::HashMap;
use std::sync::Arc;

/// A premultiplied RGBA color, each channel `0.0..=1.0`.
pub(crate) type Premul = [f32; 4];

/// One device-space draw operation (pixels, top-left origin, y down).
#[derive(Clone, Debug)]
pub(crate) enum Op {
    /// An upright box `[x0, y0, x1, y1]`: filled, or with `stroke` > 0 an
    /// outline that wide centered on the edge; `radius` rounds the corners.
    Box {
        rect: [f32; 4],
        radius: f32,
        stroke: f32,
        color: Premul,
    },
    /// Closed polygons filled by `even_odd` or non-zero winding, within the
    /// pixel box `bbox` (`[x0, y0, x1, y1]`).
    Path {
        contours: Vec<Vec<[f32; 2]>>,
        even_odd: bool,
        color: Premul,
        bbox: [i32; 4],
    },
    /// A straight-alpha bitmap stretched over the local rect `rect`
    /// (`[x0, y0, x1, y1]`), drawn through `transform`, at `opacity`.
    /// `uv` is the part of the bitmap drawn, `[u0, v0, u1, v1]` in `0..=1`
    /// (swapped ends mirror it); `tint` (premultiplied) multiplies every
    /// texel; `nearest` samples nearest-neighbor instead of bilinear.
    Image {
        image: Source,
        transform: Transform2D,
        rect: [f32; 4],
        opacity: f32,
        uv: [f32; 4],
        tint: Premul,
        nearest: bool,
    },
    /// Clip what follows (until the matching [`Op::PopClip`]) to the
    /// intersection with this clip.
    PushClip(Clip),
    PopClip,
    /// Draw what follows (until the matching [`Op::PopLayer`]) into a layer,
    /// then composite it at `opacity`.
    PushLayer {
        opacity: f32,
    },
    /// Draw what follows (until the matching [`Op::PopLayer`]) into a layer
    /// in the layer's own pixels, `[0, 0, size]`, unclipped by what encloses
    /// it, then composite that part of it into `dst` ([`Scene::draw_offscreen`]:
    /// a game's frame at a low resolution, scaled up).
    PushResampled(Resample),
    PopLayer,
}

/// How an [`Op::PushResampled`] layer is drawn and composited.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Resample {
    /// The pixels drawn, at the layer's top-left (within the frame's size).
    pub size: [u32; 2],
    /// Where they go in the parent, device pixels `[x0, y0, x1, y1]`.
    pub dst: [f32; 4],
    /// Sampled nearest-neighbor rather than bilinear.
    pub nearest: bool,
}

impl Resample {
    /// The scissor a resampled layer starts with: all it draws.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn scissor(&self) -> [i32; 4] {
        [0, 0, self.size[0] as i32, self.size[1] as i32]
    }
}

/// The bitmap an [`Op::Image`] draws.
#[derive(Clone, Debug)]
pub(crate) enum Source {
    /// An image of its own, kept by its identity.
    Image(Arc<Pixmap>),
    /// A glyph raster: the pixel rectangle `region` (`[x, y, w, h]`) of the
    /// atlas page `plan.atlas[page]`.
    Atlas { page: usize, region: [u32; 4] },
    /// An image in another API's memory, imported (or, where it can't be,
    /// read on the CPU), kept by its identity.
    External(Arc<ExternalImage>),
}

/// All of a bitmap.
pub(crate) const FULL_UV: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
/// No tint.
pub(crate) const NO_TINT: Premul = [1.0, 1.0, 1.0, 1.0];

/// Which part of a texture a quad shows, and tinted how.
pub(crate) struct Sample {
    pub uv: [f32; 4],
    pub clamp: [f32; 4],
    pub tint: [f32; 4],
}

impl Sample {
    /// All of it, as it is.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) const WHOLE: Sample = Sample {
        uv: FULL_UV,
        clamp: [0.0, 0.0, 1.0, 1.0],
        tint: NO_TINT,
    };

    /// The part `uv` of a `w`×`h` texture, kept half a texel inside it so
    /// bilinear filtering doesn't reach its neighbors.
    pub(crate) fn region(uv: [f32; 4], w: u32, h: u32, tint: [f32; 4]) -> Sample {
        let inside = |a: f32, b: f32, n: u32| {
            let half = 0.5 / n.max(1) as f32;
            let (lo, hi) = (a.min(b) + half, a.max(b) - half);
            if lo <= hi {
                (lo, hi)
            } else {
                let mid = (a + b) / 2.0;
                (mid, mid)
            }
        };
        let (u0, u1) = inside(uv[0], uv[2], w);
        let (v0, v1) = inside(uv[1], uv[3], h);
        Sample {
            uv,
            clamp: [u0, v0, u1, v1],
            tint,
        }
    }
}

/// A clip region, in device pixels.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Clip {
    /// A pixel rectangle `[x0, y0, x1, y1]`.
    Rect([i32; 4]),
    /// A convex polygon.
    Polygon(Vec<[f32; 2]>),
}

/// A lowered frame.
#[derive(Debug)]
pub(crate) struct Plan {
    pub size: PhysicalSize,
    pub background: Color,
    /// The operations in paint order.
    pub ops: Vec<Op>,
    /// `Some` when the frame redraws only this pixel rectangle
    /// (`[x0, y0, x1, y1]`, see [`Plan::limit`]): the target keeps the
    /// previous frame's pixels (loaded, not cleared) and only the stencil
    /// starts cleared.
    pub partial: Option<[i32; 4]>,
    /// The glyph atlas's pages, as this frame left them.
    pub atlas: Vec<AtlasPage>,
    /// The operations batched: what an executor draws.
    pub cmds: Vec<Cmd>,
    /// The vertices of every [`Cmd::Quads`] (see `super::batch`).
    pub quads: Vec<f32>,
    /// Cached textures whose images changed in places, to update before
    /// drawing (the canvases kept as textures, `gpu::canvas`).
    pub updates: Vec<TextureUpdate>,
}

impl Plan {
    /// Redraw only the pixel rectangle `r` of the frame, over the previous
    /// frame the target holds: the ops are clipped to `r` (a scissor) over
    /// the background filled in it, and the ops wholly outside it are
    /// dropped. The background has to be opaque, so that filling `r` with it
    /// replaces what was there exactly as a clear would.
    pub(crate) fn limit(&mut self, r: [i32; 4]) {
        debug_assert_eq!(
            self.background.a, 255,
            "a partial frame needs an opaque background"
        );
        let keep = [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32];
        let bg = self.background;
        let fill = Op::Box {
            rect: keep,
            radius: 0.0,
            stroke: 0.0,
            color: [
                bg.r as f32 / 255.0,
                bg.g as f32 / 255.0,
                bg.b as f32 / 255.0,
                1.0,
            ],
        };
        let old = std::mem::take(&mut self.ops);
        let mut ops = Vec::with_capacity(old.len() + 3);
        ops.push(Op::PushClip(Clip::Rect(r)));
        ops.push(fill);
        // What a resampled layer draws is in its own pixels, not the
        // frame's: kept whole (its composite is what the clip limits).
        let mut layers: Vec<bool> = Vec::new();
        ops.extend(old.into_iter().filter(|op| match op {
            Op::PushLayer { .. } => {
                layers.push(false);
                true
            }
            Op::PushResampled(_) => {
                layers.push(true);
                true
            }
            Op::PopLayer => {
                layers.pop();
                true
            }
            _ => layers.contains(&true) || op_bounds(op).is_none_or(|b| intersects(&b, &keep)),
        }));
        ops.push(Op::PopClip);
        // Batched anew: what's left of the frame.
        let (cmds, quads) = batch::batch(&ops, &self.atlas);
        self.ops = ops;
        self.cmds = cmds;
        self.quads = quads;
        self.partial = Some(r);
    }
}

/// The device-space box a drawing op can touch (padded a pixel for its
/// antialiased edge and bilinear filtering), or `None` for the ops that
/// draw nothing themselves (clips, layers).
fn op_bounds(op: &Op) -> Option<Bounds> {
    match op {
        Op::Box { rect, stroke, .. } => {
            let pad = 1.0 + stroke.max(0.0) * 0.5;
            Some([rect[0] - pad, rect[1] - pad, rect[2] + pad, rect[3] + pad])
        }
        Op::Path { bbox, .. } => Some([
            bbox[0] as f32 - 1.0,
            bbox[1] as f32 - 1.0,
            bbox[2] as f32 + 1.0,
            bbox[3] as f32 + 1.0,
        ]),
        Op::Image {
            transform, rect, ..
        } => {
            let corners = [
                (rect[0], rect[1]),
                (rect[2], rect[1]),
                (rect[0], rect[3]),
                (rect[2], rect[3]),
            ]
            .map(|(x, y)| {
                let p = transform.apply(oxideav_core::Point { x, y });
                (p.x, p.y)
            });
            let (mut b, ok) = (
                [
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                ],
                corners.iter().all(|(x, y)| x.is_finite() && y.is_finite()),
            );
            if !ok {
                return None;
            }
            for (x, y) in corners {
                b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
            }
            Some([b[0] - 1.0, b[1] - 1.0, b[2] + 1.0, b[3] + 1.0])
        }
        Op::PushClip(_)
        | Op::PopClip
        | Op::PushLayer { .. }
        | Op::PushResampled(_)
        | Op::PopLayer => None,
    }
}

/// Glyph rasters kept across frames, keyed by glyph, size, color, transform
/// and sub-pixel position: packed into the atlas, or (too big for it, or
/// when it is full of this frame's glyphs) each an `Arc<Pixmap>` whose
/// identity stays the same while it's cached, so a backend's texture cache
/// (keyed by identity) keeps its texture too.
#[derive(Default)]
pub(crate) struct RasterCache {
    entries: HashMap<u64, Cached>,
    /// Rasters in the atlas, each with its offset (see [`Cached::offset`]).
    atlas: Atlas<(i32, i32)>,
    frame: u64,
}

struct Cached {
    image: Arc<Pixmap>,
    /// The raster's top-left relative to the whole-pixel part of the glyph's
    /// position.
    offset: (i32, i32),
    used: u64,
}

/// Rasters unused for this many frames are dropped.
const KEEP_FRAMES: u64 = 120;
/// Past this many rasters, the least recently used are dropped first.
const MAX_ENTRIES: usize = 4096;
/// Sub-pixel positions are kept to this fraction of a pixel.
const SUBPIXEL: f32 = 16.0;

impl RasterCache {
    /// Rasters currently held.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len() + self.atlas.len()
    }

    /// End a frame: forget rasters that weren't used lately.
    fn sweep(&mut self) {
        let frame = self.frame;
        self.entries
            .retain(|_, c| frame.saturating_sub(c.used) < KEEP_FRAMES);
        if self.entries.len() > MAX_ENTRIES {
            let mut ages: Vec<u64> = self.entries.values().map(|c| c.used).collect();
            ages.sort_unstable();
            let cut = ages[self.entries.len() - MAX_ENTRIES];
            self.entries.retain(|_, c| c.used > cut);
        }
        self.atlas.sweep(KEEP_FRAMES, MAX_ENTRIES);
        self.frame += 1;
    }
}

/// Lower `scene` for a `physical`-pixel target cleared to `background`.
/// The scene's logical extent is fitted to the target the way the software
/// renderer fits its view box (uniform scale, centered).
#[allow(dead_code)] // the tests and the web's comparison page use it
pub(crate) fn plan(
    scene: &Scene,
    background: Color,
    physical: PhysicalSize,
    cache: &mut RasterCache,
) -> Plan {
    plan_with(scene, background, physical, cache, None)
}

/// A window's frame for [`plan_with`]: its canvases are kept as textures in
/// `cache` under `owner`, and `damage` (logical rects, `None` for all of it)
/// is what changed since its last frame.
pub(crate) struct CanvasArgs<'a> {
    pub cache: &'a mut CanvasCache,
    pub owner: u64,
    pub damage: Option<Vec<Rect>>,
    /// Canvas scrolls before the damage.
    pub scrolls: Vec<crate::core::CanvasScroll>,
}

/// [`plan`], keeping canvases as textures when `canvases` says whose frame
/// this is.
pub(crate) fn plan_with(
    scene: &Scene,
    background: Color,
    physical: PhysicalSize,
    cache: &mut RasterCache,
    canvases: Option<CanvasArgs<'_>>,
) -> Plan {
    let mut updates = Vec::new();
    let (w, h) = (physical.width as f32, physical.height as f32);
    let logical = scene.logical_size();
    let (lw, lh) = (logical.width as f32, logical.height as f32);
    let mut ops = Vec::new();
    if lw > 0.0 && lh > 0.0 && w > 0.0 && h > 0.0 {
        // xMidYMid meet, as the rasterizer maps the scene's view box.
        let s = (w / lw).min(h / lh);
        let root = Transform2D {
            a: s,
            b: 0.0,
            c: 0.0,
            d: s,
            e: (w - lw * s) / 2.0,
            f: (h - lh * s) / 2.0,
        };
        let target = [0.0, 0.0, w, h];
        let mut frame = canvases.map(|c| {
            (
                c.cache.begin(c.owner, c.damage, c.scrolls, &root, target),
                c.cache,
            )
        });
        let mut walker = Walker {
            ops: &mut ops,
            cache,
            canvases: frame.as_mut().map(|(f, _)| f),
            target,
        };
        scene.with_root(|nodes, deferred| walker.nodes(deferred, nodes, &root));
        if let Some((f, c)) = frame {
            updates = c.end(f);
        }
    }
    let atlas = cache.atlas.snapshot();
    cache.sweep();
    let (cmds, quads) = batch::batch(&ops, &atlas);
    Plan {
        size: physical,
        background,
        ops,
        partial: None,
        atlas,
        cmds,
        quads,
        updates,
    }
}

struct Walker<'a> {
    ops: &'a mut Vec<Op>,
    cache: &'a mut RasterCache,
    /// The window's canvases kept as textures, when it has them.
    canvases: Option<&'a mut CanvasFrame>,
    /// The target's extent, for culling.
    target: Bounds,
}

/// What [`Walker::clip`] did with a group's clip.
enum ClipOutcome {
    Pushed,
    /// Nothing inside it can show.
    Empty,
    /// Not a shape the backend clips to.
    Unsupported,
}

fn upright(t: &Transform2D) -> bool {
    t.b == 0.0 && t.c == 0.0
}

fn premul(c: Rgba) -> Premul {
    let a = c.a as f32 / 255.0;
    [
        c.r as f32 / 255.0 * a,
        c.g as f32 / 255.0 * a,
        c.b as f32 / 255.0 * a,
        a,
    ]
}

fn solid(paint: &Paint) -> Option<Rgba> {
    match paint {
        Paint::Solid(c) => Some(*c),
        _ => None,
    }
}

fn intersects(a: &Bounds, b: &Bounds) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// The upright rectangle `path` is, as `[x0, y0, x1, y1]`.
pub(crate) fn as_rect(path: &Path) -> Option<[f32; 4]> {
    if !lower::is_rect(path) {
        return None;
    }
    lower::path_bounds(path)
}

/// The rounded rectangle `path` is (what [`round_rect_path`] makes), as its
/// box and corner radius.
pub(crate) fn as_round_rect(path: &Path) -> Option<([f32; 4], f32)> {
    let [
        PathCommand::MoveTo(start),
        PathCommand::LineTo(_),
        PathCommand::CubicCurveTo { end: corner, .. },
        ..,
    ] = path.commands.as_slice()
    else {
        return None;
    };
    if path.commands.len() != 10 {
        return None;
    }
    // Start sits at (x0 + r, y0); the first corner ends at (x1, y0 + r).
    let r = corner.y - start.y;
    let (x0, y0, x1) = (start.x - r, start.y, corner.x);
    let PathCommand::LineTo(bottom) = path.commands[3] else {
        return None;
    };
    let y1 = bottom.y + r;
    if r <= 0.0 || x1 <= x0 || y1 <= y0 {
        return None;
    }
    let rect = Rect::from_xywh(x0 as f64, y0 as f64, (x1 - x0) as f64, (y1 - y0) as f64);
    let again = round_rect_path(rect, r as f64);
    let close = |a: &oxideav_core::Point, b: &oxideav_core::Point| {
        (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3
    };
    let same = again.commands.len() == path.commands.len()
        && again
            .commands
            .iter()
            .zip(&path.commands)
            .all(|pair| match pair {
                (PathCommand::MoveTo(a), PathCommand::MoveTo(b))
                | (PathCommand::LineTo(a), PathCommand::LineTo(b)) => close(a, b),
                (
                    PathCommand::CubicCurveTo {
                        c1: a1,
                        c2: a2,
                        end: a3,
                    },
                    PathCommand::CubicCurveTo {
                        c1: b1,
                        c2: b2,
                        end: b3,
                    },
                ) => close(a1, b1) && close(a2, b2) && close(a3, b3),
                (PathCommand::Close, PathCommand::Close) => true,
                _ => false,
            });
    same.then_some(([x0, y0, x1, y1], r))
}

/// `r` mapped through the upright transform `t`.
fn map_rect(t: &Transform2D, r: [f32; 4]) -> [f32; 4] {
    lower::map_bounds(t, r)
}

/// The stroke outline the rasterizer fills for `stroke` along `path` under
/// `t` (as `oxideav-raster` builds it: the width scaled by the mean of the
/// transform's diagonal).
///
/// As oxideav-raster's own renderer does it (so the two agree): the stroke
/// (width, dashes, joins, caps) is in user space, so the outline is built
/// in user space scaled by `s`, the transform's largest singular value, and
/// then mapped to the device by `t · scale(1/s)`, which never stretches a
/// length, so the flattening tolerances still bound the device error.
fn stroke_contours(path: &Path, t: &Transform2D, stroke: &Stroke) -> Vec<FlatContour> {
    let (a, b, c, d) = (t.a as f64, t.b as f64, t.c as f64, t.d as f64);
    let det = a * d - b * c;
    let e2 = a * a + b * b + c * c + d * d;
    let s = ((e2 + (e2 * e2 - 4.0 * det * det).max(0.0).sqrt()) * 0.5).sqrt();
    if !(s.is_finite() && s > 1e-12) || det.abs() <= 1e-18 * e2.max(1e-300) {
        return Vec::new(); // a singular transform: no area
    }
    let mut st = stroke.clone();
    if let Some(dash) = &mut st.dash {
        for v in &mut dash.array {
            *v = (*v as f64 * s) as f32;
        }
        dash.offset = (dash.offset as f64 * s) as f32;
    }
    let width = (stroke.width as f64 * s) as f32;
    let pre = Transform2D::scale(s as f32, s as f32);
    let (fa, fb, fc, fd) = (a / s, b / s, c / s, d / s);
    let (fe, ff) = (t.e as f64, t.f as f64);
    flatten_path(&path.commands, &pre)
        .iter()
        .flat_map(|c| stroke_to_fill_path(c, &st, width))
        .map(|mut c| {
            for p in &mut c.points {
                let (x, y) = (p.0 as f64, p.1 as f64);
                *p = ((fa * x + fc * y + fe) as f32, (fb * x + fd * y + ff) as f32);
            }
            c
        })
        .collect()
}

/// Whether the closed polygon `pts` is convex (and not degenerate).
fn convex(pts: &[[f32; 2]]) -> bool {
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut sign = 0.0f32;
    for i in 0..n {
        let (a, b, c) = (pts[i], pts[(i + 1) % n], pts[(i + 2) % n]);
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cross.abs() < 1e-4 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    sign != 0.0
}

/// A hash of every paint under `nodes`: glyph rasters bake their color in.
fn paint_hash(nodes: &[Node], h: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    for n in nodes {
        match n {
            Node::Path(p) => {
                for paint in [p.fill.as_ref(), p.stroke.as_ref().map(|s| &s.paint)]
                    .into_iter()
                    .flatten()
                {
                    match paint {
                        Paint::Solid(c) => (0u8, c.r, c.g, c.b, c.a).hash(h),
                        // Not expected in a glyph; keyed apart by its address.
                        other => (1u8, other as *const Paint as usize).hash(h),
                    }
                }
                p.stroke.as_ref().map(|s| s.width.to_bits()).hash(h);
            }
            Node::Group(g) => {
                g.opacity.to_bits().hash(h);
                paint_hash(&g.children, h);
            }
            _ => 2u8.hash(h),
        }
    }
}

/// Rasterize `node`, drawn through `t`, into a `w`×`h` bitmap whose top-left
/// is the device pixel `(x0, y0)`.
fn raster(node: &Node, t: &Transform2D, x0: i32, y0: i32, w: u32, h: u32) -> Pixmap {
    Pixmap::from_rgba8(PhysicalSize::new(w, h), raster_bytes(node, t, x0, y0, w, h))
}

/// [`raster`]'s pixels, straight-alpha RGBA rows.
fn raster_bytes(node: &Node, t: &Transform2D, x0: i32, y0: i32, w: u32, h: u32) -> Vec<u8> {
    let shift = Transform2D::translate(-x0 as f32, -y0 as f32).compose(t);
    // A one-off render: no need for the bitmap cache.
    let frame = Renderer::with_cache_capacity(w, h, 1).render_node(node, shift);
    let plane = &frame.planes[0];
    let row = w as usize * 4;
    let mut data = Vec::with_capacity(row * h as usize);
    for y in 0..h as usize {
        data.extend_from_slice(&plane.data[y * plane.stride..y * plane.stride + row]);
    }
    data
}

impl Walker<'_> {
    fn nodes(&mut self, deferred: &[Deferred], nodes: &[Node], t: &Transform2D) {
        for n in nodes {
            self.node(deferred, n, t);
        }
    }

    fn visible(&self, local: Option<Bounds>, t: &Transform2D) -> bool {
        local.is_none_or(|b| intersects(&lower::map_bounds(t, b), &self.target))
    }

    fn node(&mut self, deferred: &[Deferred], node: &Node, t: &Transform2D) {
        match node {
            Node::Path(p) => self.path(p, t),
            Node::Group(g) => self.group(deferred, g, t),
            Node::Image(img) => match lower::deferred_index(img) {
                Some(i) => {
                    let local = t.compose(&img.transform);
                    let r = img.bounds;
                    let rect = [r.x, r.y, r.x + r.width, r.y + r.height];
                    if !self.visible(Some(rect), &local) {
                        return;
                    }
                    match deferred.get(i) {
                        Some(Deferred::Image(image)) => self.ops.push(Op::Image {
                            image: Source::Image(image.clone()),
                            transform: local,
                            rect,
                            opacity: 1.0,
                            uv: FULL_UV,
                            tint: NO_TINT,
                            nearest: false,
                        }),
                        Some(Deferred::Sprite(sprite)) => self.sprite(sprite, img, &local),
                        Some(Deferred::External(e)) => self.external(e, img, &local),
                        Some(Deferred::Canvas {
                            bounds,
                            paint,
                            scale,
                            id,
                        }) => self.canvas(*bounds, paint, *scale, &local, *id),
                        Some(Deferred::Offscreen(o)) => self.offscreen(o, rect, &local),
                        Some(Deferred::Glyphs(g)) => self.glyph_run(deferred, g, &local),
                        None => {}
                    }
                }
                None => self.fallback(node, t),
            },
            _ => self.fallback(node, t),
        }
    }

    fn path(&mut self, p: &PathNode, t: &Transform2D) {
        let pad = p
            .stroke
            .as_ref()
            .map_or(0.0, |s| s.width * s.miter_limit.max(1.5) / 2.0);
        let b =
            lower::path_bounds(&p.path).map(|b| [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad]);
        if !self.visible(b, t) {
            return;
        }
        let fill = p.fill.as_ref().map(solid);
        let stroke = p.stroke.as_ref().map(|s| solid(&s.paint));
        if matches!(fill, Some(None)) || matches!(stroke, Some(None)) {
            // A gradient: the rasterizer paints it.
            return self.fallback(&Node::Path(p.clone()), t);
        }
        if let Some(Some(color)) = fill {
            self.fill(p, t, color);
        }
        if let (Some(s), Some(Some(color))) = (&p.stroke, stroke) {
            self.stroke(&p.path, t, s, color);
        }
    }

    fn fill(&mut self, p: &PathNode, t: &Transform2D, color: Rgba) {
        let color = premul(color);
        if upright(t) {
            if let Some(r) = as_rect(&p.path) {
                return self.ops.push(Op::Box {
                    rect: map_rect(t, r),
                    radius: 0.0,
                    stroke: 0.0,
                    color,
                });
            }
            if let Some((r, radius)) = as_round_rect(&p.path)
                && (t.a.abs() - t.d.abs()).abs() < 1e-4
            {
                return self.ops.push(Op::Box {
                    rect: map_rect(t, r),
                    radius: radius * t.a.abs(),
                    stroke: 0.0,
                    color,
                });
            }
        }
        let contours = flatten_path(&p.path.commands, t);
        self.polygons(contours, p.fill_rule == FillRule::EvenOdd, color);
    }

    fn stroke(&mut self, path: &Path, t: &Transform2D, s: &Stroke, color: Rgba) {
        let color = premul(color);
        if upright(t) && s.dash.is_none() {
            let width = s.width * 0.5 * (t.a.abs() + t.d.abs());
            // A sharp box's corners are miters (√2 is inside any miter limit
            // of at least that).
            if let Some(r) = as_rect(path)
                && s.join == LineJoin::Miter
                && s.miter_limit >= std::f32::consts::SQRT_2
            {
                return self.ops.push(Op::Box {
                    rect: map_rect(t, r),
                    radius: 0.0,
                    stroke: width,
                    color,
                });
            }
            // A rounded box has no corners to join.
            if let Some((r, radius)) = as_round_rect(path)
                && (t.a.abs() - t.d.abs()).abs() < 1e-4
            {
                return self.ops.push(Op::Box {
                    rect: map_rect(t, r),
                    radius: radius * t.a.abs(),
                    stroke: width,
                    color,
                });
            }
        }
        let contours = stroke_contours(path, t, s);
        self.polygons(contours, false, color);
    }

    /// Fill flattened `contours` by stencil-and-cover.
    fn polygons(&mut self, contours: Vec<FlatContour>, even_odd: bool, color: Premul) {
        let mut b = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        let contours: Vec<Vec<[f32; 2]>> = contours
            .into_iter()
            .filter(|c| c.points.len() >= 3)
            .map(|c| {
                c.points
                    .iter()
                    .map(|&(x, y)| {
                        b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
                        [x, y]
                    })
                    .collect()
            })
            .collect();
        if contours.is_empty() {
            return;
        }
        let bbox = [
            (b[0].floor() as i32).max(0),
            (b[1].floor() as i32).max(0),
            (b[2].ceil() as i32).min(self.target[2] as i32),
            (b[3].ceil() as i32).min(self.target[3] as i32),
        ];
        if bbox[2] <= bbox[0] || bbox[3] <= bbox[1] {
            return;
        }
        self.ops.push(Op::Path {
            contours,
            even_odd,
            color,
            bbox,
        });
    }

    fn group(&mut self, deferred: &[Deferred], g: &Group, t: &Transform2D) {
        let local = t.compose(&g.transform);
        if g.opacity <= 0.0 {
            return;
        }
        if let Some(key) = g.cache_key {
            return self.cached(g, key, &local);
        }
        if let Some(clip) = &g.clip
            && !self.visible(lower::path_bounds(clip), &local)
        {
            return;
        }
        let layer = g.opacity < 1.0;
        if layer {
            self.ops.push(Op::PushLayer { opacity: g.opacity });
        }
        let clipped = match &g.clip {
            None => false,
            Some(clip) => match self.clip(clip, &local) {
                ClipOutcome::Pushed => true,
                ClipOutcome::Empty => {
                    if layer {
                        self.ops.pop();
                    }
                    return;
                }
                ClipOutcome::Unsupported => {
                    if layer {
                        self.ops.pop();
                    }
                    return self.fallback(&Node::Group(g.clone()), t);
                }
            },
        };
        self.nodes(deferred, &g.children, &local);
        if clipped {
            self.ops.push(Op::PopClip);
        }
        if layer {
            self.ops.push(Op::PopLayer);
        }
    }

    fn clip(&mut self, clip: &Path, t: &Transform2D) -> ClipOutcome {
        if upright(t)
            && let Some(r) = as_rect(clip)
        {
            let d = map_rect(t, r);
            let px = [
                d[0].round() as i32,
                d[1].round() as i32,
                d[2].round() as i32,
                d[3].round() as i32,
            ];
            if px[2] <= px[0] || px[3] <= px[1] {
                return ClipOutcome::Empty;
            }
            self.ops.push(Op::PushClip(Clip::Rect(px)));
            return ClipOutcome::Pushed;
        }
        let contours = flatten_path(&clip.commands, t);
        let [contour] = contours.as_slice() else {
            return if contours.is_empty() {
                ClipOutcome::Empty
            } else {
                ClipOutcome::Unsupported
            };
        };
        let mut pts: Vec<[f32; 2]> = contour.points.iter().map(|&(x, y)| [x, y]).collect();
        if pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
        if !convex(&pts) {
            return ClipOutcome::Unsupported;
        }
        self.ops.push(Op::PushClip(Clip::Polygon(pts)));
        ClipOutcome::Pushed
    }

    /// A cache-keyed group (a glyph): rasterized once per key, color,
    /// transform and sub-pixel position, then drawn as an image.
    fn cached(&mut self, g: &Group, key: u64, local: &Transform2D) {
        use std::hash::{Hash, Hasher};
        let Some(local_bounds) = lower::subtree_bounds(&g.children) else {
            return self.fallback_group(g, local);
        };
        // Snap the position to a sub-pixel grid, then split it into whole
        // pixels and the fraction the raster is made for.
        let q = |v: f32| (v * SUBPIXEL).round() / SUBPIXEL;
        let (ex, fy) = (q(local.e), q(local.f));
        let (bx, by) = (ex.floor(), fy.floor());
        let snapped = Transform2D {
            e: ex,
            f: fy,
            ..*local
        };
        let dev = lower::map_bounds(&snapped, local_bounds);
        if !intersects(&dev, &self.target) {
            return;
        }
        let (x0, y0) = (
            (dev[0] - bx).floor() as i32 - 1,
            (dev[1] - by).floor() as i32 - 1,
        );
        let (x1, y1) = (
            (dev[2] - bx).ceil() as i32 + 1,
            (dev[3] - by).ceil() as i32 + 1,
        );
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        if w == 0 || h == 0 || w > 2048 || h > 2048 {
            return self.fallback_group(g, local);
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        for v in [local.a, local.b, local.c, local.d] {
            ((v * 1e4).round() as i64).hash(&mut hasher);
        }
        (((ex - bx) * SUBPIXEL) as u8, ((fy - by) * SUBPIXEL) as u8).hash(&mut hasher);
        g.clip.is_some().hash(&mut hasher);
        paint_hash(&g.children, &mut hasher);
        let k = hasher.finish();
        let frame = self.cache.frame;
        let push = |ops: &mut Vec<Op>, image: Source, offset: (i32, i32), w: u32, h: u32| {
            let (ox, oy) = (bx + offset.0 as f32, by + offset.1 as f32);
            ops.push(Op::Image {
                image,
                transform: Transform2D::translate(ox, oy),
                rect: [0.0, 0.0, w as f32, h as f32],
                opacity: g.opacity,
                uv: FULL_UV,
                tint: NO_TINT,
                nearest: false,
            });
        };
        let atlased = |slot: atlas::Slot| Source::Atlas {
            page: slot.page,
            region: slot.rect,
        };
        if let Some((slot, &offset)) = self.cache.atlas.get(k) {
            return push(self.ops, atlased(slot), offset, slot.rect[2], slot.rect[3]);
        }
        if let Some(entry) = self.cache.entries.get_mut(&k) {
            entry.used = frame;
            let (iw, ih) = (entry.image.size().width, entry.image.size().height);
            let image = Source::Image(entry.image.clone());
            return push(self.ops, image, entry.offset, iw, ih);
        }
        let node = Node::Group(Group {
            children: g.children.clone(),
            clip: g.clip.clone(),
            ..Group::new()
        });
        // The raster's top-left in device pixels, for this position.
        let at = Transform2D::translate(-bx, -by).compose(&snapped);
        let pixels = raster_bytes(&node, &at, x0, y0, w, h);
        if let Some((slot, _)) = self.cache.atlas.insert(k, w, h, &pixels, (x0, y0)) {
            return push(self.ops, atlased(slot), (x0, y0), w, h);
        }
        let image = Arc::new(Pixmap::from_rgba8(PhysicalSize::new(w, h), pixels));
        self.cache.entries.insert(
            k,
            Cached {
                image: image.clone(),
                offset: (x0, y0),
                used: frame,
            },
        );
        push(self.ops, Source::Image(image), (x0, y0), w, h);
    }

    /// A glyph run (`Scene::fill_glyph_run`): each glyph from the glyph
    /// atlas, rasterized once per glyph, color, transform and sub-pixel
    /// position (as [`cached`](Self::cached) draws a laid-out text's, keyed
    /// without walking its outline). One the atlas can't take goes that way.
    fn glyph_run(&mut self, deferred: &[Deferred], run: &GlyphDraw, t: &Transform2D) {
        use std::hash::{Hash, Hasher};
        let c = run.color();
        let q = |v: f32| (v * SUBPIXEL).round() / SUBPIXEL;
        let k4 = |v: f32| (v * 1e4).round() as i32;
        for g in run.glyphs() {
            let local = t.compose(&run.place(g));
            let (ex, fy) = (q(local.e), q(local.f));
            let (bx, by) = (ex.floor(), fy.floor());
            let mut h = FastHasher::default();
            (b"run", g.key).hash(&mut h);
            [k4(local.a), k4(local.b), k4(local.c), k4(local.d)].hash(&mut h);
            (((ex - bx) * SUBPIXEL) as u8, ((fy - by) * SUBPIXEL) as u8).hash(&mut h);
            (c.r, c.g, c.b, c.a).hash(&mut h);
            let k = h.finish();
            let hit = match self.cache.atlas.get(k) {
                Some((slot, &offset)) => Some((slot, offset)),
                None => {
                    // Rasterized now, into the atlas.
                    let Some(outline) = run.outline(g, c) else {
                        continue;
                    };
                    let Some(b) = lower::subtree_bounds(std::slice::from_ref(&outline)) else {
                        continue;
                    };
                    let snapped = Transform2D {
                        e: ex,
                        f: fy,
                        ..local
                    };
                    let dev = lower::map_bounds(&snapped, b);
                    let (x0, y0) = (
                        (dev[0] - bx).floor() as i32 - 1,
                        (dev[1] - by).floor() as i32 - 1,
                    );
                    let (x1, y1) = (
                        (dev[2] - bx).ceil() as i32 + 1,
                        (dev[3] - by).ceil() as i32 + 1,
                    );
                    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
                    let at = Transform2D::translate(-bx, -by).compose(&snapped);
                    let pixels = (w > 0 && h > 0 && w <= 2048 && h <= 2048)
                        .then(|| raster_bytes(&outline, &at, x0, y0, w, h));
                    pixels.and_then(|px| {
                        let (slot, _) = self.cache.atlas.insert(k, w, h, &px, (x0, y0))?;
                        Some((slot, (x0, y0)))
                    })
                }
            };
            let Some((slot, offset)) = hit else {
                if let Some(node) = run.glyph_node(g, true) {
                    self.node(deferred, &node, t);
                }
                continue;
            };
            let (ox, oy) = (bx + offset.0 as f32, by + offset.1 as f32);
            let (w, h) = (slot.rect[2] as f32, slot.rect[3] as f32);
            if !intersects(&[ox, oy, ox + w, oy + h], &self.target) {
                continue;
            }
            self.ops.push(Op::Image {
                image: Source::Atlas {
                    page: slot.page,
                    region: slot.rect,
                },
                transform: Transform2D::translate(ox, oy),
                rect: [0.0, 0.0, w, h],
                opacity: 1.0,
                uv: FULL_UV,
                tint: NO_TINT,
                nearest: false,
            });
        }
    }

    /// [`fallback`](Self::fallback) for a group already composed into `local`.
    fn fallback_group(&mut self, g: &Group, local: &Transform2D) {
        let node = Node::Group(Group {
            transform: Transform2D::identity(),
            ..g.clone()
        });
        self.fallback(&node, local);
    }

    /// Rasterize `node`, drawn through `t`, on the CPU for the part of the
    /// target it covers, and draw that as an image.
    fn fallback(&mut self, node: &Node, t: &Transform2D) {
        let b = lower::subtree_bounds(std::slice::from_ref(node))
            .map(|b| lower::map_bounds(t, b))
            .unwrap_or(self.target);
        // Room for antialiasing, miters and caps beyond the bounds.
        let x0 = ((b[0] - 2.0).floor()).max(0.0) as i32;
        let y0 = ((b[1] - 2.0).floor()).max(0.0) as i32;
        let x1 = ((b[2] + 2.0).ceil()).min(self.target[2]) as i32;
        let y1 = ((b[3] + 2.0).ceil()).min(self.target[3]) as i32;
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let image = Arc::new(raster(node, t, x0, y0, w, h));
        self.ops.push(Op::Image {
            image: Source::Image(image),
            transform: Transform2D::translate(x0 as f32, y0 as f32),
            rect: [0.0, 0.0, w as f32, h as f32],
            opacity: 1.0,
            uv: FULL_UV,
            tint: NO_TINT,
            nearest: false,
        });
    }

    /// A region of a bitmap ([`Scene::draw_image_region`]): the bitmap's
    /// own texture, sampled over the region (mirrored by swapping its ends),
    /// tinted and faded in the shader.
    fn sprite(&mut self, s: &SpriteDraw, img: &oxideav_core::ImageRef, local: &Transform2D) {
        let size = s.image.size();
        let (w, h) = (size.width as f32, size.height as f32);
        let [x0, y0, x1, y1] = s.src.map(|v| v as f32);
        let (mut u0, mut u1) = (x0 / w, x1 / w);
        let (mut v0, mut v1) = (y0 / h, y1 / h);
        if s.flip_x {
            std::mem::swap(&mut u0, &mut u1);
        }
        if s.flip_y {
            std::mem::swap(&mut v0, &mut v1);
        }
        let t = s.tint;
        let r = img.bounds;
        self.ops.push(Op::Image {
            image: Source::Image(s.image.clone()),
            transform: *local,
            rect: [r.x, r.y, r.x + r.width, r.y + r.height],
            opacity: s.opacity,
            uv: [u0, v0, u1, v1],
            tint: premul(Rgba::new(t[0], t[1], t[2], t[3])),
            nearest: s.filter == Filter::Nearest,
        });
    }

    /// A scene drawn at a resolution of its own ([`Scene::draw_offscreen`])
    /// into the local rect `rect`: into a resampled layer of its pixels,
    /// composited into `rect`, when it's drawn upright and fits the frame's
    /// layers; else the software renderer's bitmap of it, as an image.
    fn offscreen(&mut self, o: &Offscreen, rect: [f32; 4], local: &Transform2D) {
        let (pw, ph) = (o.pixels.width, o.pixels.height);
        let nearest = o.filter == Filter::Nearest;
        let logical = o.content.logical_size();
        let (lw, lh) = (logical.width as f32, logical.height as f32);
        let resampled = upright(local)
            && local.a > 0.0
            && local.d > 0.0
            && lw > 0.0
            && lh > 0.0
            && pw as f32 <= self.target[2] - self.target[0]
            && ph as f32 <= self.target[3] - self.target[1];
        if !resampled {
            self.ops.push(Op::Image {
                image: Source::Image(o.rendered()),
                transform: *local,
                rect,
                opacity: 1.0,
                uv: FULL_UV,
                tint: NO_TINT,
                nearest,
            });
            return;
        }
        self.ops.push(Op::PushResampled(Resample {
            size: [pw, ph],
            dst: map_rect(local, rect),
            nearest,
        }));
        // The content's extent fitted to its pixels, as the software
        // renderer maps a scene's view box (xMidYMid meet).
        let (w, h) = (pw as f32, ph as f32);
        let s = (w / lw).min(h / lh);
        let root = Transform2D {
            a: s,
            b: 0.0,
            c: 0.0,
            d: s,
            e: (w - lw * s) / 2.0,
            f: (h - lh * s) / 2.0,
        };
        // Culled to the layer's pixels; canvases in it (not the window's
        // own) are painted straight.
        let target = std::mem::replace(&mut self.target, [0.0, 0.0, w, h]);
        let canvases = self.canvases.take();
        o.content
            .with_root(|nodes, deferred| self.nodes(deferred, nodes, &root));
        self.target = target;
        self.canvases = canvases;
        self.ops.push(Op::PopLayer);
    }

    /// A region of an external image: its texture, imported by the
    /// executor, sampled over the region as a sprite's.
    fn external(&mut self, e: &ExternalDraw, img: &oxideav_core::ImageRef, local: &Transform2D) {
        let size = e.image.size();
        let (w, h) = (size.width as f32, size.height as f32);
        let [x0, y0, x1, y1] = e.src.map(|v| v as f32);
        let (mut u0, mut u1) = (x0 / w, x1 / w);
        let (mut v0, mut v1) = (y0 / h, y1 / h);
        if e.flip_x {
            std::mem::swap(&mut u0, &mut u1);
        }
        if e.flip_y {
            std::mem::swap(&mut v0, &mut v1);
        }
        let t = e.tint;
        let r = img.bounds;
        self.ops.push(Op::Image {
            image: Source::External(e.image.clone()),
            transform: *local,
            rect: [r.x, r.y, r.x + r.width, r.y + r.height],
            opacity: e.opacity,
            uv: [u0, v0, u1, v1],
            tint: premul(Rgba::new(t[0], t[1], t[2], t[3])),
            nearest: e.filter == Filter::Nearest,
        });
    }

    /// A canvas: kept as a texture when it can be (`gpu::canvas`), else its
    /// paint callback runs for the part of `bounds` on the target, and what
    /// it drew is walked, clipped to `bounds`.
    fn canvas(
        &mut self,
        bounds: Rect,
        paint: &CanvasPaint,
        scale: f64,
        local: &Transform2D,
        id: Option<CanvasId>,
    ) {
        if let (Some(id), Some(frame)) = (id, self.canvases.as_deref_mut())
            && let Some(op) = frame.draw(id, bounds, paint, scale, local)
        {
            self.ops.push(op);
            return;
        }
        let dirty = match lower::invert(local) {
            Some(inv) => {
                let v = lower::map_bounds(&inv, self.target);
                let v = Rect::from_xywh(
                    v[0] as f64,
                    v[1] as f64,
                    (v[2] - v[0]) as f64,
                    (v[3] - v[1]) as f64,
                );
                match v.intersection(bounds) {
                    Some(d) => d,
                    None => return,
                }
            }
            None => return,
        };
        let scene = lower::paint_canvas(bounds, paint, scale, dirty);
        let clip = rect_path(bounds);
        let clipped = match self.clip(&clip, local) {
            ClipOutcome::Pushed => true,
            ClipOutcome::Empty => return,
            ClipOutcome::Unsupported => false,
        };
        let shift = Transform2D::translate(bounds.origin.x as f32, bounds.origin.y as f32);
        let inner = local.compose(&shift);
        scene.with_root(|nodes, deferred| self.nodes(deferred, nodes, &inner));
        if clipped {
            self.ops.push(Op::PopClip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Affine, Point, Size, Vec2};

    fn rotate_about(radians: f64, c: Point) -> Affine {
        Affine::translate(Vec2::new(-c.x, -c.y))
            .then(Affine::rotate(radians))
            .then(Affine::translate(Vec2::new(c.x, c.y)))
    }

    fn lower(scene: &Scene, cache: &mut RasterCache) -> Vec<Op> {
        let size = PhysicalSize::new(
            scene.logical_size().width as u32,
            scene.logical_size().height as u32,
        );
        plan(scene, Color::BLACK, size, cache).ops
    }

    #[test]
    fn rects_and_round_rects_become_boxes() {
        let mut s = Scene::new(Size::new(100.0, 100.0));
        s.fill_rect(Rect::from_xywh(1.0, 2.0, 10.0, 20.0), Color::WHITE);
        s.fill_round_rect(Rect::from_xywh(10.0, 10.0, 40.0, 30.0), 6.0, Color::WHITE);
        s.stroke_round_rect(
            Rect::from_xywh(10.0, 10.0, 40.0, 30.0),
            6.0,
            Color::WHITE,
            2.0,
        );
        s.stroke_rect(Rect::from_xywh(5.0, 5.0, 10.0, 10.0), Color::WHITE, 1.0);
        let ops = lower(&s, &mut RasterCache::default());
        let boxes: Vec<_> = ops
            .iter()
            .map(|op| match op {
                Op::Box {
                    rect,
                    radius,
                    stroke,
                    ..
                } => (*rect, *radius, *stroke),
                other => panic!("not a box: {other:?}"),
            })
            .collect();
        assert_eq!(
            boxes,
            vec![
                ([1.0, 2.0, 11.0, 22.0], 0.0, 0.0),
                ([10.0, 10.0, 50.0, 40.0], 6.0, 0.0),
                ([10.0, 10.0, 50.0, 40.0], 6.0, 2.0),
                ([5.0, 5.0, 15.0, 15.0], 0.0, 1.0),
            ]
        );
    }

    #[test]
    fn a_round_rect_is_recognized_only_when_it_is_one() {
        let rect = Rect::from_xywh(3.0, 4.0, 30.0, 12.0);
        let (r, radius) = as_round_rect(&round_rect_path(rect, 4.0)).unwrap();
        assert_eq!(r, [3.0, 4.0, 33.0, 16.0]);
        assert!((radius - 4.0).abs() < 1e-5);
        // A radius past half the short side is clamped, and still found.
        let (_, radius) = as_round_rect(&round_rect_path(rect, 100.0)).unwrap();
        assert!((radius - 6.0).abs() < 1e-5);
        assert!(as_round_rect(&rect_path(rect)).is_none());
        let mut bent = round_rect_path(rect, 4.0);
        if let PathCommand::CubicCurveTo { c1, .. } = &mut bent.commands[2] {
            c1.x += 1.0;
        }
        assert!(as_round_rect(&bent).is_none());
    }

    #[test]
    fn a_rotated_rect_is_a_path_and_a_translucent_layer_is_a_layer() {
        let mut s = Scene::new(Size::new(100.0, 100.0));
        s.push_layer(rotate_about(0.3, Point::new(50.0, 50.0)), 0.5);
        s.fill_rect(Rect::from_xywh(30.0, 30.0, 40.0, 40.0), Color::WHITE);
        s.pop_layer();
        let ops = lower(&s, &mut RasterCache::default());
        assert!(matches!(ops[0], Op::PushLayer { opacity } if opacity == 0.5));
        let Op::Path { contours, .. } = &ops[1] else {
            panic!("{:?}", ops[1]);
        };
        assert_eq!(contours.len(), 1);
        assert!(matches!(ops[2], Op::PopLayer));
        assert_eq!(ops.len(), 3);
    }

    #[test]
    fn an_opaque_layer_only_moves_things() {
        let mut s = Scene::new(Size::new(100.0, 100.0));
        s.push_layer(Affine::translate(Vec2::new(5.0, 7.0)), 1.0);
        s.fill_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), Color::WHITE);
        s.pop_layer();
        let ops = lower(&s, &mut RasterCache::default());
        assert!(
            matches!(&ops[..], [Op::Box { rect, .. }] if *rect == [5.0, 7.0, 15.0, 17.0]),
            "{ops:?}"
        );
    }

    #[test]
    fn an_offscreen_scene_is_a_resampled_layer_in_its_own_pixels() {
        use crate::render::Filter;
        let mut low = Scene::new(Size::new(20.0, 10.0));
        low.fill_rect(Rect::from_xywh(2.0, 3.0, 4.0, 5.0), Color::WHITE);
        let mut s = Scene::new(Size::new(100.0, 50.0));
        let dst = Rect::from_xywh(10.0, 5.0, 60.0, 30.0);
        s.draw_offscreen(dst, PhysicalSize::new(20, 10), low, Filter::Nearest);
        let mut cache = RasterCache::default();
        let ops = lower(&s, &mut cache);
        assert_eq!(ops.len(), 3, "{ops:?}");
        let Op::PushResampled(r) = &ops[0] else {
            panic!("{:?}", ops[0]);
        };
        assert_eq!(r.size, [20, 10]);
        assert_eq!(r.dst, [10.0, 5.0, 70.0, 35.0]);
        assert!(r.nearest);
        // The fill is in the layer's pixels, not the frame's.
        assert!(matches!(
            ops[1],
            Op::Box {
                rect: [2.0, 3.0, 6.0, 8.0],
                ..
            }
        ));
        assert!(matches!(ops[2], Op::PopLayer));
    }

    #[test]
    fn an_offscreen_scene_bigger_than_the_frame_is_its_bitmap() {
        use crate::render::Filter;
        let mut low = Scene::new(Size::new(200.0, 10.0));
        low.fill_rect(Rect::from_xywh(2.0, 3.0, 4.0, 5.0), Color::WHITE);
        let mut s = Scene::new(Size::new(100.0, 50.0));
        let dst = Rect::from_xywh(0.0, 0.0, 100.0, 5.0);
        s.draw_offscreen(dst, PhysicalSize::new(200, 10), low, Filter::Linear);
        let ops = lower(&s, &mut RasterCache::default());
        assert_eq!(ops.len(), 1);
        let Op::Image {
            image: Source::Image(image),
            nearest: false,
            ..
        } = &ops[0]
        else {
            panic!("{:?}", ops[0]);
        };
        assert_eq!(image.size(), PhysicalSize::new(200, 10));
    }

    #[test]
    fn a_limited_plan_keeps_what_a_resampled_layer_draws() {
        let boxed = |x: f32| Op::Box {
            rect: [x, 10.0, x + 10.0, 20.0],
            radius: 0.0,
            stroke: 0.0,
            color: [1.0; 4],
        };
        let mut plan = Plan {
            updates: Vec::new(),
            size: PhysicalSize::new(200, 100),
            background: Color::rgb(255, 0, 0),
            ops: vec![
                Op::PushResampled(Resample {
                    size: [40, 30],
                    dst: [100.0, 0.0, 180.0, 60.0],
                    nearest: true,
                }),
                boxed(0.0),
                Op::PopLayer,
                boxed(0.0),
            ],
            partial: None,
            atlas: Vec::new(),
            cmds: Vec::new(),
            quads: Vec::new(),
        };
        plan.limit([95, 0, 120, 50]);
        let ops = &plan.ops;
        // The clip, the fill, the layer with its box (in its own pixels,
        // outside the region in the frame's), its pop and the clip's: the
        // box after it, outside the region, is gone.
        assert_eq!(ops.len(), 6, "{ops:?}");
        assert!(matches!(ops[2], Op::PushResampled(_)));
        assert!(matches!(
            ops[3],
            Op::Box {
                rect: [0.0, ..],
                ..
            }
        ));
        assert!(matches!(ops[4], Op::PopLayer));
    }

    #[test]
    fn a_limited_plan_fills_its_region_and_drops_what_lies_outside() {
        let boxed = |x: f32| Op::Box {
            rect: [x, 10.0, x + 10.0, 20.0],
            radius: 0.0,
            stroke: 0.0,
            color: [1.0; 4],
        };
        let mut plan = Plan {
            updates: Vec::new(),
            size: PhysicalSize::new(200, 100),
            background: Color::rgb(255, 0, 0),
            ops: vec![
                boxed(0.0),
                Op::PushLayer { opacity: 0.5 },
                boxed(100.0),
                boxed(150.0),
                Op::PopLayer,
            ],
            partial: None,
            atlas: Vec::new(),
            cmds: Vec::new(),
            quads: Vec::new(),
        };
        plan.limit([95, 0, 120, 50]);
        assert_eq!(plan.partial, Some([95, 0, 120, 50]));
        let ops = &plan.ops;
        assert!(matches!(ops[0], Op::PushClip(Clip::Rect([95, 0, 120, 50]))));
        assert!(matches!(
            ops[1],
            Op::Box {
                rect: [95.0, 0.0, 120.0, 50.0],
                color: [1.0, 0.0, 0.0, 1.0],
                ..
            }
        ));
        // The box at 0 and the one at 150 are gone; the layer stays.
        assert!(matches!(ops[2], Op::PushLayer { .. }));
        assert!(matches!(
            ops[3],
            Op::Box {
                rect: [100.0, ..],
                ..
            }
        ));
        assert!(matches!(ops[4], Op::PopLayer));
        assert!(matches!(ops[5], Op::PopClip));
        assert_eq!(ops.len(), 6);
        // Batched anew: the clip, the fill and the box before the layer in
        // one batch, the layer's box in another.
        let quads: Vec<u32> = plan
            .cmds
            .iter()
            .filter_map(|c| match c {
                Cmd::Quads(q) => Some(q.count),
                Cmd::Op(_) => None,
            })
            .collect();
        assert_eq!(quads, vec![6, 6]);
        assert_eq!(plan.quads.len(), 12 * batch::QUAD_FLOATS);
    }

    #[test]
    fn clips_nest_as_scissors_and_skip_what_is_empty() {
        let mut s = Scene::new(Size::new(100.0, 100.0));
        s.push_clip(Rect::from_xywh(10.0, 10.0, 50.0, 50.0));
        s.fill_rect(Rect::from_xywh(0.0, 0.0, 100.0, 100.0), Color::WHITE);
        s.pop_clip();
        s.push_clip(Rect::from_xywh(10.0, 10.0, 0.0, 50.0));
        s.fill_rect(Rect::from_xywh(0.0, 0.0, 100.0, 100.0), Color::WHITE);
        s.pop_clip();
        let ops = lower(&s, &mut RasterCache::default());
        assert!(matches!(
            &ops[0],
            Op::PushClip(Clip::Rect([10, 10, 60, 60]))
        ));
        assert!(matches!(ops[1], Op::Box { .. }));
        assert!(matches!(ops[2], Op::PopClip));
        assert_eq!(ops.len(), 3, "{ops:?}");
    }

    #[test]
    fn a_rotated_clip_is_a_polygon() {
        let mut s = Scene::new(Size::new(100.0, 100.0));
        s.push_layer(rotate_about(0.5, Point::new(50.0, 50.0)), 1.0);
        s.push_clip(Rect::from_xywh(20.0, 20.0, 60.0, 60.0));
        s.fill_rect(Rect::from_xywh(0.0, 0.0, 100.0, 100.0), Color::WHITE);
        s.pop_clip();
        s.pop_layer();
        let ops = lower(&s, &mut RasterCache::default());
        assert!(
            matches!(&ops[0], Op::PushClip(Clip::Polygon(p)) if p.len() == 4),
            "{ops:?}"
        );
    }

    #[test]
    fn images_keep_their_identity() {
        let image = Arc::new(Pixmap::new(PhysicalSize::new(4, 4)));
        let mut s = Scene::new(Size::new(100.0, 100.0));
        s.draw_image(Rect::from_xywh(10.0, 10.0, 20.0, 20.0), image.clone());
        // Off the target: dropped.
        s.draw_image(Rect::from_xywh(200.0, 10.0, 20.0, 20.0), image.clone());
        let ops = lower(&s, &mut RasterCache::default());
        let [
            Op::Image {
                image: drawn, rect, ..
            },
        ] = &ops[..]
        else {
            panic!("{ops:?}");
        };
        assert!(matches!(drawn, Source::Image(d) if Arc::ptr_eq(d, &image)));
        assert_eq!(*rect, [10.0, 10.0, 30.0, 30.0]);
    }

    #[test]
    fn a_region_samples_its_part_of_the_image() {
        use crate::render::ImageOptions;
        let image = Arc::new(Pixmap::new(PhysicalSize::new(8, 4)));
        let mut s = Scene::new(Size::new(100.0, 100.0));
        let opts = ImageOptions::new()
            .nearest()
            .flip_x(true)
            .tint(Color::rgba(255, 0, 0, 255))
            .opacity(0.5);
        let src = Rect::from_xywh(2.0, 0.0, 4.0, 2.0);
        s.draw_image_region(src, Rect::from_xywh(10.0, 10.0, 40.0, 20.0), &image, &opts);
        let ops = lower(&s, &mut RasterCache::default());
        let [
            Op::Image {
                image: drawn,
                rect,
                uv,
                tint,
                nearest,
                opacity,
                ..
            },
        ] = &ops[..]
        else {
            panic!("{ops:?}");
        };
        assert!(
            matches!(drawn, Source::Image(d) if Arc::ptr_eq(d, &image)),
            "the whole image's texture"
        );
        assert_eq!(*rect, [10.0, 10.0, 50.0, 30.0]);
        assert_eq!(*uv, [0.75, 0.0, 0.25, 0.5], "mirrored across");
        assert_eq!(*tint, [1.0, 0.0, 0.0, 1.0]);
        assert!(*nearest);
        assert_eq!(*opacity, 0.5);
    }

    #[test]
    fn glyphs_are_rasterized_once_and_reused() {
        let Some(font) = crate::render::Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        let mut s = Scene::new(Size::new(200.0, 40.0));
        s.fill_text(
            "lll",
            Point::new(4.0, 4.0),
            &crate::render::TextPaint::new(&font, 16.0, Color::WHITE),
        );
        let mut cache = RasterCache::default();
        let first = lower(&s, &mut cache);
        let images = |ops: &[Op]| -> Vec<(usize, [u32; 4])> {
            ops.iter()
                .filter_map(|op| match op {
                    Op::Image {
                        image: Source::Atlas { page, region },
                        ..
                    } => Some((*page, *region)),
                    _ => None,
                })
                .collect()
        };
        let a = images(&first);
        assert_eq!(a.len(), 3, "each glyph a region of the atlas: {first:?}");
        // Three 'l's at most at three sub-pixel phases; never more rasters
        // than glyphs.
        assert!(cache.len() <= 3);
        let second = images(&lower(&s, &mut cache));
        assert_eq!(a, second, "a repeated frame reuses its rasters");
        // A different color is a different raster.
        let mut red = Scene::new(Size::new(200.0, 40.0));
        red.fill_text(
            "l",
            Point::new(4.0, 4.0),
            &crate::render::TextPaint::new(&font, 16.0, Color::rgb(255, 0, 0)),
        );
        let r = images(&lower(&red, &mut cache));
        assert_ne!(r[0], a[0]);
    }

    #[test]
    fn outlined_shadowed_rich_text_lowers_to_glyph_rasters_and_images() {
        use crate::geometry::Vec2;
        use crate::render::{InlineIcon, RichText, TextOutline, TextPaint, TextShadow};
        let Some(font) = crate::render::Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        let icon = Arc::new(Pixmap::new(crate::geometry::PhysicalSize::new(4, 4)));
        let text = RichText::new()
            .push("l")
            .icon(InlineIcon::new(icon.clone()));
        let paint = TextPaint::new(&font, 16.0, Color::WHITE)
            .outline(TextOutline::new(2.0, Color::BLACK))
            .shadow(TextShadow::new(
                Vec2::new(1.0, 1.0),
                Color::rgba(0, 0, 80, 128),
            ));
        let mut s = Scene::new(Size::new(200.0, 40.0));
        s.fill_rich_text(&text, Point::new(4.0, 4.0), &paint);
        let mut cache = RasterCache::default();
        let ops = lower(&s, &mut cache);
        let images: Vec<&Source> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Image { image, .. } => Some(image),
                _ => None,
            })
            .collect();
        let region = |s: &Source| match s {
            Source::Atlas { region, .. } => *region,
            Source::Image(_) | Source::External(_) => panic!("not in the atlas"),
        };
        // The shadow, the outline and the fill are each a raster of the
        // glyph (three different ones here; a shadow in the outline's color
        // would share its raster), then the icon is its own image.
        assert_eq!(images.len(), 4, "{ops:?}");
        assert_ne!(region(images[0]), region(images[1]));
        assert_ne!(region(images[1]), region(images[2]));
        assert!(matches!(images[3], Source::Image(i) if Arc::ptr_eq(i, &icon)));
        // The outline raster is wider than the fill's.
        assert!(region(images[1])[2] > region(images[2])[2]);
        // The translucent shadow is composited as a layer.
        let shadow_layer =
            matches!(ops[0], Op::PushLayer { opacity } if (opacity - 128.0 / 255.0).abs() < 0.01);
        assert!(shadow_layer, "{ops:?}");
    }

    #[test]
    fn a_dashed_line_and_a_curve_are_paths() {
        use crate::render::{Path as SPath, StrokeStyle};
        let mut s = Scene::new(Size::new(100.0, 100.0));
        s.stroke_path(
            &SPath::line(Point::new(5.0, 50.0), Point::new(95.0, 50.0)),
            Color::WHITE,
            &StrokeStyle::new(3.0).dashed(&[6.0, 4.0]),
        );
        let mut curve = SPath::new();
        curve
            .move_to(Point::new(10.0, 10.0))
            .quad_to(Point::new(50.0, 90.0), Point::new(90.0, 10.0))
            .close();
        s.fill_path(&curve, Color::WHITE);
        let ops = lower(&s, &mut RasterCache::default());
        assert_eq!(ops.len(), 2);
        let Op::Path { contours, .. } = &ops[0] else {
            panic!()
        };
        assert!(contours.len() > 5, "one outline per dash");
        assert!(matches!(
            ops[1],
            Op::Path {
                even_odd: false,
                ..
            }
        ));
    }

    #[test]
    fn convexity() {
        assert!(convex(&[
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0]
        ]));
        assert!(!convex(&[
            [0.0, 0.0],
            [10.0, 0.0],
            [5.0, 2.0],
            [10.0, 10.0],
            [0.0, 10.0]
        ]));
        assert!(!convex(&[[0.0, 0.0], [1.0, 1.0]]));
    }
}
