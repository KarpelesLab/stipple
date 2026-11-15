//! Batching: the lowered operations, in paint order, merged into as few draw
//! calls as keep that order.
//!
//! Boxes and textured quads (images, image regions, sprites, glyphs) are all
//! drawn by one *quad* shader whose every input is a vertex attribute, so
//! any run of them that samples one texture (boxes sample none) and isn't
//! interrupted by a path, a clip or a layer is one draw call: [`Cmd::Quads`],
//! a range of the frame's quad vertices. With glyphs in the atlas
//! (`super::atlas`) a widget tree's boxes and text are a handful of draws.
//! Paths, clips and layers stay operations of their own ([`Cmd::Op`]).
//!
//! Each quad is two triangles, six vertices of [`QUAD_FLOATS`] floats, five
//! `vec4`s, every vertex of a quad holding the same values but the first two
//! `vec4`s:
//!
//! ```text
//!        box                              textured
//! a0     x y px py                         x y u v
//! a1     rect x0 y0 x1 y1                  edge distances l t r b
//! a2     color (premultiplied)             uv clamp u0 v0 u1 v1
//! a3     radius stroke 0 0                 tint × opacity (premultiplied)
//! a4     mode 0 0 0                        mode 0 0 0
//! ```
//!
//! `x y` is the device position (pixels, top-left origin), `px py` the same
//! for the box's coverage. A textured quad's `u v` are its texture
//! coordinates (unclamped: they run past the region over the antialiased
//! edge) and its edge distances are in device pixels, both linear over the
//! quad, so interpolating them is exact; an edge that isn't antialiased is
//! [`NO_EDGE`] away. `mode` is [`MODE_BOX`], [`MODE_TEX`] (straight-alpha
//! texels) or [`MODE_TEX_PREMUL`] (premultiplied texels: a layer).
//!
//! The quad fragment shader, in every executor:
//!
//! ```text
//! box:      cov = exact pixel coverage of the box or ring, or a signed
//!                 distance with rounded corners;              out = color * cov
//! textured: c = sample(clamp(uv)); if straight, c.rgb *= c.a; c *= tint;
//!           cov = clamp(min(l, r) + 0.5) * clamp(min(t, b) + 0.5); out = c * cov
//! ```
//!
//! A batch of boxes only, or one whose texture the executor couldn't make,
//! is drawn by the box-only variant of the shader, which leaves textured
//! quads out.

use super::atlas::AtlasPage;
use super::ops::{Op, Premul, Sample, Source};
use crate::render::Pixmap;
use oxideav_core::{Point, Transform2D};
use std::sync::Arc;

/// Floats per quad vertex.
pub(crate) const QUAD_FLOATS: usize = 20;
/// Vertices per quad.
pub(crate) const QUAD_VERTICES: usize = 6;
/// A box.
pub(crate) const MODE_BOX: f32 = 0.0;
/// Straight-alpha texels.
pub(crate) const MODE_TEX: f32 = 1.0;
/// Premultiplied texels (a layer composited).
pub(crate) const MODE_TEX_PREMUL: f32 = 2.0;
/// An edge distance that keeps full coverage (an edge not antialiased).
pub(crate) const NO_EDGE: f32 = 1.0e4;

/// The texture a batch samples.
#[derive(Clone, Debug)]
pub(crate) enum TexRef {
    /// An image's own texture, kept by its identity.
    Image(Arc<Pixmap>),
    /// The atlas page `plan.atlas[i]`.
    Atlas(usize),
}

impl TexRef {
    fn same(&self, other: &TexRef) -> bool {
        match (self, other) {
            (TexRef::Image(a), TexRef::Image(b)) => Arc::ptr_eq(a, b),
            (TexRef::Atlas(a), TexRef::Atlas(b)) => a == b,
            _ => false,
        }
    }
}

/// A run of quads drawn with one call.
#[derive(Clone, Debug)]
pub(crate) struct Quads {
    /// What the textured quads sample; `None` for boxes only.
    pub texture: Option<TexRef>,
    /// Sampled nearest-neighbor rather than bilinear.
    pub nearest: bool,
    /// The first vertex in `plan.quads` (in vertices) and how many.
    pub first: u32,
    pub count: u32,
}

/// One step of a batched frame.
#[derive(Clone, Debug)]
pub(crate) enum Cmd {
    Quads(Quads),
    /// `plan.ops[i]`: a path, a clip pushed or popped, a layer pushed or
    /// popped.
    Op(usize),
}

/// Batch `ops` (whose atlas regions are on `atlas`'s pages): the commands,
/// and the quad vertices they draw.
pub(crate) fn batch(ops: &[Op], atlas: &[AtlasPage]) -> (Vec<Cmd>, Vec<f32>) {
    let mut cmds: Vec<Cmd> = Vec::new();
    let mut verts: Vec<f32> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let before = verts.len();
        let (texture, nearest) = match op {
            Op::Box {
                rect,
                radius,
                stroke,
                color,
            } => {
                box_quad(&mut verts, *rect, *radius, *stroke, *color);
                (None, false)
            }
            Op::Image {
                image,
                transform,
                rect,
                opacity,
                uv,
                tint,
                nearest,
            } => {
                let (texture, sample) = match image {
                    Source::Image(image) => {
                        let size = image.size();
                        (
                            TexRef::Image(image.clone()),
                            Sample::region(*uv, size.width, size.height, *tint),
                        )
                    }
                    Source::Atlas { page, region } => {
                        let Some(p) = atlas.get(*page) else {
                            continue;
                        };
                        let side = p.side as f32;
                        let [x, y, w, h] = region.map(|v| v as f32);
                        let u = |t: f32| (x + t * w) / side;
                        let v = |t: f32| (y + t * h) / side;
                        let uv = [u(uv[0]), v(uv[1]), u(uv[2]), v(uv[3])];
                        (
                            TexRef::Atlas(*page),
                            Sample::region(uv, p.side, p.side, *tint),
                        )
                    }
                };
                tex_quad(&mut verts, transform, *rect, *opacity, false, true, &sample);
                (Some(texture), *nearest)
            }
            _ => {
                cmds.push(Cmd::Op(i));
                continue;
            }
        };
        let added = ((verts.len() - before) / QUAD_FLOATS) as u32;
        if added == 0 {
            continue;
        }
        if let Some(Cmd::Quads(q)) = cmds.last_mut() {
            let fits = match (&q.texture, &texture) {
                (_, None) => true,
                (None, Some(_)) => true,
                (Some(a), Some(b)) => a.same(b) && q.nearest == nearest,
            };
            if fits {
                if q.texture.is_none() && texture.is_some() {
                    q.texture = texture;
                    q.nearest = nearest;
                }
                q.count += added;
                continue;
            }
        }
        cmds.push(Cmd::Quads(Quads {
            texture,
            nearest,
            first: (before / QUAD_FLOATS) as u32,
            count: added,
        }));
    }
    (cmds, verts)
}

/// Push the six vertices of the quad over the corners `top-left, top-right,
/// bottom-left, bottom-right` (each its `a0` and `a1`), sharing `rest`
/// (`a2`, `a3`, `a4`).
fn push(out: &mut Vec<f32>, corners: [[f32; 8]; 4], rest: &[f32; 12]) {
    let [a, b, c, d] = corners;
    for v in [a, b, c, c, b, d] {
        out.extend_from_slice(&v);
        out.extend_from_slice(rest);
    }
}

/// A box (see [`Op::Box`]) as a quad, padded for its antialiased edge and
/// half its stroke.
pub(crate) fn box_quad(
    out: &mut Vec<f32>,
    rect: [f32; 4],
    radius: f32,
    stroke: f32,
    color: Premul,
) {
    let pad = 1.0 + stroke * 0.5;
    let (x0, y0, x1, y1) = (rect[0] - pad, rect[1] - pad, rect[2] + pad, rect[3] + pad);
    let v = |x: f32, y: f32| [x, y, x, y, rect[0], rect[1], rect[2], rect[3]];
    let mut rest = [0.0; 12];
    rest[..4].copy_from_slice(&color);
    rest[4] = radius;
    rest[5] = stroke;
    rest[8] = MODE_BOX;
    push(out, [v(x0, y0), v(x1, y0), v(x0, y1), v(x1, y1)], &rest);
}

/// A texture over the local `rect` drawn through `m` at `opacity` as a
/// quad: `premul` if its texels are premultiplied, `aa` to antialias its
/// edges (padded a device pixel for them); `sample` says which part of it,
/// tinted how. Nothing for an empty `rect`.
pub(crate) fn tex_quad(
    out: &mut Vec<f32>,
    m: &Transform2D,
    rect: [f32; 4],
    opacity: f32,
    premul: bool,
    aa: bool,
    sample: &Sample,
) {
    let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
    if rw == 0.0 || rh == 0.0 {
        return;
    }
    // Device pixels per local unit along each local axis.
    let px = (m.a.hypot(m.b).max(1e-6), m.c.hypot(m.d).max(1e-6));
    let (pu, pv) = if aa {
        (1.0 / px.0, 1.0 / px.1)
    } else {
        (0.0, 0.0)
    };
    let uv = sample.uv;
    let corner = |x: f32, y: f32| {
        let d = m.apply(Point { x, y });
        let (fx, fy) = ((x - rect[0]) / rw, (y - rect[1]) / rh);
        let edges = if aa {
            [
                (x - rect[0]) * px.0,
                (y - rect[1]) * px.1,
                (rect[2] - x) * px.0,
                (rect[3] - y) * px.1,
            ]
        } else {
            [NO_EDGE; 4]
        };
        [
            d.x,
            d.y,
            uv[0] + (uv[2] - uv[0]) * fx,
            uv[1] + (uv[3] - uv[1]) * fy,
            edges[0],
            edges[1],
            edges[2],
            edges[3],
        ]
    };
    let (x0, y0, x1, y1) = (rect[0] - pu, rect[1] - pv, rect[2] + pu, rect[3] + pv);
    let mut rest = [0.0; 12];
    rest[..4].copy_from_slice(&sample.clamp);
    for (r, t) in rest[4..8].iter_mut().zip(sample.tint) {
        *r = t * opacity;
    }
    rest[8] = if premul { MODE_TEX_PREMUL } else { MODE_TEX };
    push(
        out,
        [
            corner(x0, y0),
            corner(x1, y0),
            corner(x0, y1),
            corner(x1, y1),
        ],
        &rest,
    );
}

#[cfg(test)]
mod tests {
    use super::super::ops::{self, RasterCache};
    use super::*;
    use crate::geometry::{PhysicalSize, Rect, Size};
    use crate::render::{Color, Scene};

    fn quads(cmds: &[Cmd]) -> Vec<&Quads> {
        cmds.iter()
            .filter_map(|c| match c {
                Cmd::Quads(q) => Some(q),
                Cmd::Op(_) => None,
            })
            .collect()
    }

    #[test]
    fn boxes_and_images_of_one_texture_are_one_batch() {
        let image = Arc::new(Pixmap::new(PhysicalSize::new(4, 4)));
        let other = Arc::new(Pixmap::new(PhysicalSize::new(4, 4)));
        let mut s = Scene::new(Size::new(100.0, 100.0));
        s.fill_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), Color::WHITE);
        s.draw_image(Rect::from_xywh(10.0, 10.0, 20.0, 20.0), image.clone());
        s.fill_round_rect(Rect::from_xywh(5.0, 5.0, 10.0, 10.0), 3.0, Color::WHITE);
        s.draw_image(Rect::from_xywh(30.0, 10.0, 20.0, 20.0), image.clone());
        // Another texture: a new batch.
        s.draw_image(Rect::from_xywh(50.0, 10.0, 20.0, 20.0), other.clone());
        s.fill_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), Color::WHITE);
        // A clip ends the batch.
        s.push_clip(Rect::from_xywh(0.0, 0.0, 50.0, 50.0));
        s.fill_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), Color::WHITE);
        s.pop_clip();
        let plan = ops::plan(
            &s,
            Color::BLACK,
            PhysicalSize::new(100, 100),
            &mut RasterCache::default(),
        );
        let q = quads(&plan.cmds);
        assert_eq!(plan.cmds.len(), 5, "{:?}", plan.cmds);
        assert_eq!(q.len(), 3);
        assert_eq!(q[0].count as usize, 4 * QUAD_VERTICES);
        assert!(matches!(&q[0].texture, Some(TexRef::Image(i)) if Arc::ptr_eq(i, &image)));
        assert_eq!(q[1].count as usize, 2 * QUAD_VERTICES);
        assert!(matches!(&q[1].texture, Some(TexRef::Image(i)) if Arc::ptr_eq(i, &other)));
        assert!(q[2].texture.is_none());
        assert_eq!(q[2].first, q[1].first + q[1].count);
        assert_eq!(
            plan.quads.len(),
            7 * QUAD_VERTICES * QUAD_FLOATS,
            "every quad's vertices, once"
        );
    }

    #[test]
    fn nearest_and_bilinear_regions_are_separate_batches() {
        use crate::render::ImageOptions;
        let sheet = Arc::new(Pixmap::new(PhysicalSize::new(8, 8)));
        let mut s = Scene::new(Size::new(100.0, 100.0));
        let src = Rect::from_xywh(0.0, 0.0, 4.0, 4.0);
        for i in 0..10 {
            let dst = Rect::from_xywh(i as f64 * 8.0, 0.0, 8.0, 8.0);
            s.draw_image_region(src, dst, &sheet, &ImageOptions::new().nearest());
        }
        let dst = Rect::from_xywh(0.0, 50.0, 8.0, 8.0);
        s.draw_image_region(src, dst, &sheet, &ImageOptions::new());
        let plan = ops::plan(
            &s,
            Color::BLACK,
            PhysicalSize::new(100, 100),
            &mut RasterCache::default(),
        );
        let q = quads(&plan.cmds);
        assert_eq!(q.len(), 2);
        assert!(q[0].nearest && !q[1].nearest);
        assert_eq!(q[0].count as usize, 10 * QUAD_VERTICES);
    }

    #[test]
    fn a_textured_quad_interpolates_its_texture_coordinates_and_edges() {
        let mut out = Vec::new();
        let m = Transform2D::translate(10.0, 20.0);
        let sample = Sample::region([0.0, 0.0, 1.0, 1.0], 4, 4, [1.0; 4]);
        tex_quad(
            &mut out,
            &m,
            [0.0, 0.0, 4.0, 8.0],
            0.5,
            false,
            true,
            &sample,
        );
        assert_eq!(out.len(), QUAD_VERTICES * QUAD_FLOATS);
        let v = |i: usize| &out[i * QUAD_FLOATS..(i + 1) * QUAD_FLOATS];
        // The top-left corner, a pixel out for the antialiased edge.
        assert_eq!(
            &v(0)[..8],
            &[9.0, 19.0, -0.25, -0.125, -1.0, -1.0, 5.0, 9.0]
        );
        // The bottom-right one.
        assert_eq!(&v(5)[..8], &[15.0, 29.0, 1.25, 1.125, 5.0, 9.0, -1.0, -1.0]);
        assert_eq!(&v(0)[8..12], &sample.clamp);
        assert_eq!(&v(0)[12..16], &[0.5; 4], "the tint times the opacity");
        assert_eq!(v(0)[16], MODE_TEX);
    }

    /// The widget gallery's frame: how many draws batching saves.
    #[test]
    fn a_widget_window_batches_into_few_draws() {
        let Some(font) = crate::render::Font::system_default() else {
            eprintln!("skipping: no system font found");
            return;
        };
        let scene = super::super::corpus::widgets(font);
        let mut cache = RasterCache::default();
        let plan = ops::plan(
            &scene,
            Color::BLACK,
            PhysicalSize::new(2560, 1600),
            &mut cache,
        );
        let unbatched = plan
            .ops
            .iter()
            .filter(|o| matches!(o, Op::Box { .. } | Op::Image { .. }))
            .count();
        let batches = quads(&plan.cmds).len();
        eprintln!(
            "widgets: {} operations, {unbatched} boxes and quads drawn in {batches} batches, {} atlas page(s)",
            plan.ops.len(),
            plan.atlas.len()
        );
        assert!(unbatched > 500, "{unbatched}");
        assert!(batches * 20 < unbatched, "{batches} batches");
        assert_eq!(plan.atlas.len(), 1, "every glyph in one page");
    }
}
