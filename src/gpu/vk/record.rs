//! Recording a lowered [`Plan`] for the Vulkan executor: the operation list
//! becomes render passes and draws (which pipeline, what scissor, what stencil
//! reference, which vertices, what push constants, which texture), with no
//! Vulkan in it, so it is tested on every host.
//!
//! It is the Metal executor's `encode` (`metal_scene.rs`) without the API:
//! the same clip bookkeeping (scissor rectangles intersected, stencil polygons
//! counted up and down), layers in pooled offscreen targets cleared when
//! pushed and composited into their parent when popped, and paths by
//! stencil-and-cover into a supersampled scratch target, resolved into the
//! level. The executor then only turns each [`Cmd`] into Vulkan commands.
//!
//! The vertices of the whole frame go into one array (`[x, y, aux_x, aux_y]`
//! each, device pixels, top-left origin); each draw names a range of it.

use crate::gpu::ops::{Clip, Op, Plan, Premul, Sample};
use crate::render::Pixmap;
use oxideav_core::Transform2D;
use std::collections::HashMap;
use std::sync::Arc;

/// What a render pass draws into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Target {
    /// The frame (offscreen; copied into the swapchain when presenting).
    Frame,
    /// A layer of the pool, frame-sized.
    Layer(usize),
    /// The path scratch target (one channel).
    Scratch,
}

/// The pipelines, each a shader pair with its blend and stencil state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Pipe {
    /// Boxes, on the frame or a layer, inside the stencil clip.
    Box,
    /// Textured quads (images, glyphs, layers), inside the stencil clip.
    Tex,
    /// A path's coverage resolved into the frame or a layer.
    Resolve,
    /// A clip polygon counted into the stencil (no color).
    ClipInc,
    /// A clip polygon taken back out of the stencil.
    ClipDec,
    /// A path's winding into the scratch stencil, non-zero or even-odd.
    WindNonZero,
    WindEvenOdd,
    /// Full coverage in the scratch wherever the winding says inside.
    CoverNonZero,
    CoverEvenOdd,
}

impl Pipe {
    pub(crate) const ALL: [Pipe; 9] = [
        Pipe::Box,
        Pipe::Tex,
        Pipe::Resolve,
        Pipe::ClipInc,
        Pipe::ClipDec,
        Pipe::WindNonZero,
        Pipe::WindEvenOdd,
        Pipe::CoverNonZero,
        Pipe::CoverEvenOdd,
    ];

    /// Whether it draws into the scratch target (else the frame or a layer).
    pub(crate) fn on_scratch(self) -> bool {
        matches!(
            self,
            Pipe::WindNonZero | Pipe::WindEvenOdd | Pipe::CoverNonZero | Pipe::CoverEvenOdd
        )
    }
}

/// A texture a draw samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Texture {
    /// [`Recording::images`]`[index]`, sampled nearest or bilinear.
    Image { index: usize, nearest: bool },
    /// A layer, composited into its parent.
    Layer(usize),
    /// The scratch target, resolved.
    Scratch,
}

/// Push constants: `xf` (device pixels to clip space) for the vertex shader
/// in the first four floats, then the fragment shader's values at the
/// offsets the shaders in `src/gpu/shaders/` declare.
pub(crate) const PUSH_FLOATS: usize = 26;
/// The size of the push-constant range, in bytes.
pub(crate) const PUSH_BYTES: u32 = (PUSH_FLOATS * 4) as u32;

// Float indices into the push constants (byte offset / 4), per shader.
pub(crate) const XF: usize = 0;
pub(crate) const BOX_RECT: usize = 4;
pub(crate) const BOX_COLOR: usize = 8;
pub(crate) const BOX_RADIUS: usize = 12;
pub(crate) const BOX_STROKE: usize = 13;
pub(crate) const TEX_RECT: usize = 4;
pub(crate) const TEX_UV: usize = 8;
pub(crate) const TEX_CLAMP: usize = 12;
pub(crate) const TEX_TINT: usize = 16;
pub(crate) const TEX_PX: usize = 20;
pub(crate) const TEX_OPACITY: usize = 22;
pub(crate) const TEX_PREMUL: usize = 23;
pub(crate) const TEX_AA: usize = 24;
pub(crate) const FLAT_COLOR: usize = 4;
pub(crate) const RESOLVE_COLOR: usize = 4;
pub(crate) const RESOLVE_INV: usize = 8;
pub(crate) const RESOLVE_TAP: usize = 10;

pub(crate) type Push = [f32; PUSH_FLOATS];

/// One draw.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Draw {
    pub pipe: Pipe,
    /// `[x0, y0, x1, y1]` in the target's pixels, never empty.
    pub scissor: [i32; 4],
    /// The stencil reference (the clip depth, or 0).
    pub reference: u32,
    /// The first vertex in [`Recording::vertices`] and how many.
    pub first: u32,
    pub count: u32,
    pub push: Push,
    pub texture: Option<Texture>,
}

/// A render pass begun, or a draw in it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cmd {
    /// End the current pass (if any) and begin one on `target`, cleared to
    /// `clear` (premultiplied) and a zero stencil, or keeping what it holds.
    Pass {
        target: Target,
        clear: Option<Premul>,
    },
    Draw(Draw),
}

/// A recorded frame.
#[derive(Debug, Default)]
pub(crate) struct Recording {
    pub cmds: Vec<Cmd>,
    /// `[x, y, aux_x, aux_y]` per vertex.
    pub vertices: Vec<f32>,
    /// The images the frame draws, each once.
    pub images: Vec<Arc<Pixmap>>,
    /// Layers in use at once, at most.
    pub layers: usize,
    /// The scratch size the paths need; more than the scratch given means
    /// recording again on a bigger one.
    pub scratch: (i32, i32),
}

impl Recording {
    /// Whether it fits a scratch target of `size`.
    pub(crate) fn fits(&self, size: (i32, i32)) -> bool {
        self.scratch.0 <= size.0 && self.scratch.1 <= size.1
    }

    /// The draws that sample a texture, each texture once.
    pub(crate) fn textures(&self) -> Vec<Texture> {
        let mut seen = Vec::new();
        for c in &self.cmds {
            if let Cmd::Draw(Draw {
                texture: Some(t), ..
            }) = c
                && !seen.contains(t)
            {
                seen.push(*t);
            }
        }
        seen
    }
}

/// A scratch size, grown to cover `want` (powers of two, at least 64, at
/// most `max`).
pub(crate) fn grow(want: (i32, i32), have: (i32, i32), max: i32) -> (i32, i32) {
    let g = |want: i32, have: i32| {
        (want.max(have).max(1) as u32)
            .next_power_of_two()
            .clamp(64, max.max(64) as u32) as i32
    };
    (g(want.0, have.0), g(want.1, have.1))
}

/// A clip pushed on a target, and how to undo it.
enum Undo {
    Scissor([i32; 4]),
    Stencil(Vec<[f32; 2]>),
}

/// A target in use: the frame or a layer.
struct Level {
    layer: Option<usize>,
    opacity: f32,
    scissor: [i32; 4],
    depth: u32,
    undo: Vec<Undo>,
}

impl Level {
    fn target(&self) -> Target {
        self.layer.map_or(Target::Frame, Target::Layer)
    }
}

struct Recorder {
    out: Recording,
    on: Option<Target>,
    w: i32,
    h: i32,
    /// The scratch target's size, for its coordinates.
    scratch: (i32, i32),
    max: i32,
    images: HashMap<usize, usize>,
}

/// Device pixels of a `w`×`h` target to Vulkan's clip space (y down).
pub(crate) fn xf(w: i32, h: i32) -> [f32; 4] {
    [2.0 / w as f32, 2.0 / h as f32, -1.0, -1.0]
}

/// Record `plan` for a scratch target of `scratch` pixels, with targets at
/// most `max` pixels on a side.
pub(crate) fn record(plan: &Plan, scratch: (i32, i32), max: i32) -> Recording {
    let (w, h) = (plan.size.width as i32, plan.size.height as i32);
    let mut r = Recorder {
        out: Recording::default(),
        on: None,
        w,
        h,
        scratch,
        max,
        images: HashMap::new(),
    };
    r.frame(plan);
    r.out
}

impl Recorder {
    fn pass(&mut self, target: Target, clear: Option<Premul>) {
        self.out.cmds.push(Cmd::Pass { target, clear });
        self.on = Some(target);
    }

    /// Get ready to draw into `level`; `false` when its scissor is empty.
    fn bind(&mut self, level: &Level) -> bool {
        let s = level.scissor;
        if s[2] <= s[0] || s[3] <= s[1] {
            return false;
        }
        if self.on != Some(level.target()) {
            self.pass(level.target(), None);
        }
        true
    }

    /// Add `verts`, returning `(first, count)`.
    fn vertices(&mut self, verts: &[f32]) -> (u32, u32) {
        let first = (self.out.vertices.len() / 4) as u32;
        self.out.vertices.extend_from_slice(verts);
        (first, (verts.len() / 4) as u32)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        pipe: Pipe,
        scissor: [i32; 4],
        reference: u32,
        verts: &[f32],
        push: Push,
        texture: Option<Texture>,
    ) {
        if verts.is_empty() {
            return;
        }
        let (first, count) = self.vertices(verts);
        self.out.cmds.push(Cmd::Draw(Draw {
            pipe,
            scissor,
            reference,
            first,
            count,
            push,
            texture,
        }));
    }

    fn push(xf: [f32; 4]) -> Push {
        let mut p = [0.0; PUSH_FLOATS];
        p[XF..XF + 4].copy_from_slice(&xf);
        p
    }

    fn image(&mut self, image: &Arc<Pixmap>) -> Option<usize> {
        let size = image.size();
        if size.width == 0
            || size.height == 0
            || size.width > self.max as u32
            || size.height > self.max as u32
        {
            return None;
        }
        let key = Arc::as_ptr(image) as usize;
        if let Some(&i) = self.images.get(&key) {
            return Some(i);
        }
        let i = self.out.images.len();
        self.out.images.push(image.clone());
        self.images.insert(key, i);
        Some(i)
    }

    fn draw_box(&mut self, level: &Level, rect: [f32; 4], radius: f32, stroke: f32, color: Premul) {
        if !self.bind(level) {
            return;
        }
        let pad = 1.0 + stroke * 0.5;
        let (x0, y0, x1, y1) = (rect[0] - pad, rect[1] - pad, rect[2] + pad, rect[3] + pad);
        #[rustfmt::skip]
        let v = [
            x0, y0, x0, y0,  x1, y0, x1, y0,  x0, y1, x0, y1,
            x0, y1, x0, y1,  x1, y0, x1, y0,  x1, y1, x1, y1,
        ];
        let mut p = Self::push(xf(self.w, self.h));
        p[BOX_RECT..BOX_RECT + 4].copy_from_slice(&rect);
        p[BOX_COLOR..BOX_COLOR + 4].copy_from_slice(&color);
        p[BOX_RADIUS] = radius;
        p[BOX_STROKE] = stroke;
        self.draw(Pipe::Box, level.scissor, level.depth, &v, p, None);
    }

    /// Draw `texture` over the local `rect` through `m`: `premul` if its
    /// pixels are premultiplied, `aa` to antialias its edges.
    #[allow(clippy::too_many_arguments)]
    fn draw_texture(
        &mut self,
        level: &Level,
        texture: Texture,
        m: &Transform2D,
        rect: [f32; 4],
        opacity: f32,
        premul: bool,
        aa: bool,
        sample: &Sample,
    ) {
        if !self.bind(level) {
            return;
        }
        let px = (m.a.hypot(m.b).max(1e-6), m.c.hypot(m.d).max(1e-6));
        let (pu, pv) = if aa {
            (1.0 / px.0, 1.0 / px.1)
        } else {
            (0.0, 0.0)
        };
        let (x0, y0, x1, y1) = (rect[0] - pu, rect[1] - pv, rect[2] + pu, rect[3] + pv);
        let corner = |x: f32, y: f32| {
            let d = m.apply(oxideav_core::Point { x, y });
            [d.x, d.y, x, y]
        };
        let (a, b, c, d) = (
            corner(x0, y0),
            corner(x1, y0),
            corner(x0, y1),
            corner(x1, y1),
        );
        let v: Vec<f32> = [a, b, c, c, b, d].concat();
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        let mut p = Self::push(xf(self.w, self.h));
        p[TEX_RECT..TEX_RECT + 4].copy_from_slice(&rect);
        p[TEX_UV..TEX_UV + 4].copy_from_slice(&sample.uv);
        p[TEX_CLAMP..TEX_CLAMP + 4].copy_from_slice(&sample.clamp);
        p[TEX_TINT..TEX_TINT + 4].copy_from_slice(&sample.tint);
        p[TEX_PX] = px.0;
        p[TEX_PX + 1] = px.1;
        p[TEX_OPACITY] = opacity;
        p[TEX_PREMUL] = flag(premul);
        p[TEX_AA] = flag(aa);
        self.draw(Pipe::Tex, level.scissor, level.depth, &v, p, Some(texture));
    }

    /// Fill `contours` by stencil-and-cover: the winding into the scratch
    /// stencil at `ss`×`ss` samples per pixel, full coverage where it says
    /// inside, then that coverage resolved into `level` in `color`.
    fn draw_path(
        &mut self,
        level: &Level,
        contours: &[Vec<[f32; 2]>],
        even_odd: bool,
        color: Premul,
        bbox: [i32; 4],
    ) {
        let s = level.scissor;
        let b = [
            bbox[0].max(s[0]),
            bbox[1].max(s[1]),
            bbox[2].min(s[2]),
            bbox[3].min(s[3]),
        ];
        let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
        if bw <= 0 || bh <= 0 {
            return;
        }
        let max = self.max;
        let ss = [4, 2, 1]
            .into_iter()
            .find(|ss| bw * ss <= max && bh * ss <= max)
            .unwrap_or(1);
        let (sw, sh) = ((bw * ss).min(max), (bh * ss).min(max));
        self.out.scratch = (self.out.scratch.0.max(sw), self.out.scratch.1.max(sh));
        let (vw, vh) = (self.scratch.0.max(1) as f32, self.scratch.1.max(1) as f32);
        self.pass(Target::Scratch, Some([0.0; 4]));
        let fs = ss as f32;
        // Device pixels to scratch clip space.
        let sxf = [
            2.0 * fs / vw,
            2.0 * fs / vh,
            -2.0 * fs * b[0] as f32 / vw - 1.0,
            -2.0 * fs * b[1] as f32 / vh - 1.0,
        ];
        let all = [0, 0, sw, sh];
        let mut white = Self::push(sxf);
        white[FLAT_COLOR..FLAT_COLOR + 4].copy_from_slice(&[1.0; 4]);

        let fans = fan_triangles(contours.iter().map(|c| c.as_slice()));
        let wind = if even_odd {
            Pipe::WindEvenOdd
        } else {
            Pipe::WindNonZero
        };
        self.draw(wind, all, 0, &fans, white, None);

        let cover = if even_odd {
            Pipe::CoverEvenOdd
        } else {
            Pipe::CoverNonZero
        };
        let (x0, y0, x1, y1) = (b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32);
        #[rustfmt::skip]
        let quad = [
            x0, y0, 0.0, 0.0,  x1, y0, 0.0, 0.0,  x0, y1, 0.0, 0.0,
            x0, y1, 0.0, 0.0,  x1, y0, 0.0, 0.0,  x1, y1, 0.0, 0.0,
        ];
        self.draw(cover, all, 0, &quad, white, None);

        // Resolve into the level.
        if !self.bind(level) {
            return;
        }
        let st = |x: f32, y: f32| [x, y, (x - x0) * fs, (y - y0) * fs];
        let v: Vec<f32> = [
            st(x0, y0),
            st(x1, y0),
            st(x0, y1),
            st(x0, y1),
            st(x1, y0),
            st(x1, y1),
        ]
        .concat();
        let mut p = Self::push(xf(self.w, self.h));
        p[RESOLVE_COLOR..RESOLVE_COLOR + 4].copy_from_slice(&color);
        p[RESOLVE_INV] = 1.0 / vw;
        p[RESOLVE_INV + 1] = 1.0 / vh;
        p[RESOLVE_TAP] = if ss > 1 { fs * 0.25 } else { 0.0 };
        self.draw(
            Pipe::Resolve,
            level.scissor,
            level.depth,
            &v,
            p,
            Some(Texture::Scratch),
        );
    }

    /// Count the polygon `pts` into `level`'s stencil (or take it back out),
    /// where it is `from`.
    fn stencil_polygon(&mut self, level: &Level, pts: &[[f32; 2]], from: u32, pipe: Pipe) {
        let full = Level {
            layer: level.layer,
            opacity: 1.0,
            scissor: [0, 0, self.w, self.h],
            depth: 0,
            undo: Vec::new(),
        };
        if !self.bind(&full) {
            return;
        }
        let fans = fan_triangles(std::iter::once(pts));
        let p = Self::push(xf(self.w, self.h));
        self.draw(pipe, full.scissor, from, &fans, p, None);
    }

    fn composite(&mut self, layer: &Level, parent: &Level) {
        let full = [0.0, 0.0, self.w as f32, self.h as f32];
        self.draw_texture(
            parent,
            Texture::Layer(layer.layer.unwrap()),
            &Transform2D::identity(),
            full,
            layer.opacity,
            true,
            false,
            &Sample::WHOLE,
        );
    }

    fn frame(&mut self, plan: &Plan) {
        let (w, h) = (self.w, self.h);
        let mut levels = vec![Level {
            layer: None,
            opacity: 1.0,
            scissor: [0, 0, w, h],
            depth: 0,
            undo: Vec::new(),
        }];
        let bg = plan.background;
        let a = bg.a as f32 / 255.0;
        let clear = [
            bg.r as f32 / 255.0 * a,
            bg.g as f32 / 255.0 * a,
            bg.b as f32 / 255.0 * a,
            a,
        ];
        // A partial frame keeps what the target holds (only the stencil
        // starts cleared, see the executor).
        self.pass(Target::Frame, plan.partial.is_none().then_some(clear));

        for op in &plan.ops {
            match op {
                Op::Box {
                    rect,
                    radius,
                    stroke,
                    color,
                } => {
                    let level = levels.last().unwrap();
                    self.draw_box(level, *rect, *radius, *stroke, *color);
                }
                Op::Path {
                    contours,
                    even_odd,
                    color,
                    bbox,
                } => {
                    let level = levels.last().unwrap();
                    self.draw_path(level, contours, *even_odd, *color, *bbox);
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
                    let Some(index) = self.image(image) else {
                        continue;
                    };
                    let size = image.size();
                    let sample = Sample::region(*uv, size.width, size.height, *tint, *nearest);
                    let texture = Texture::Image {
                        index,
                        nearest: sample.nearest,
                    };
                    let level = levels.last().unwrap();
                    self.draw_texture(
                        level, texture, transform, *rect, *opacity, false, true, &sample,
                    );
                }
                Op::PushClip(Clip::Rect(r)) => {
                    let level = levels.last_mut().unwrap();
                    let s = level.scissor;
                    level.undo.push(Undo::Scissor(s));
                    let mut n = [
                        s[0].max(r[0]),
                        s[1].max(r[1]),
                        s[2].min(r[2]),
                        s[3].min(r[3]),
                    ];
                    n[2] = n[2].max(n[0]);
                    n[3] = n[3].max(n[1]);
                    level.scissor = n;
                }
                Op::PushClip(Clip::Polygon(pts)) => {
                    let mut level = levels.pop().unwrap();
                    self.stencil_polygon(&level, pts, level.depth, Pipe::ClipInc);
                    level.depth += 1;
                    level.undo.push(Undo::Stencil(pts.clone()));
                    levels.push(level);
                }
                Op::PopClip => {
                    let mut level = levels.pop().unwrap();
                    match level.undo.pop() {
                        Some(Undo::Scissor(s)) => level.scissor = s,
                        Some(Undo::Stencil(pts)) => {
                            self.stencil_polygon(&level, &pts, level.depth, Pipe::ClipDec);
                            level.depth -= 1;
                        }
                        None => {}
                    }
                    levels.push(level);
                }
                Op::PushLayer { opacity } => {
                    let index = levels.iter().filter(|l| l.layer.is_some()).count();
                    self.out.layers = self.out.layers.max(index + 1);
                    let scissor = levels.last().unwrap().scissor;
                    levels.push(Level {
                        layer: Some(index),
                        opacity: *opacity,
                        scissor,
                        depth: 0,
                        undo: Vec::new(),
                    });
                    self.pass(Target::Layer(index), Some([0.0; 4]));
                }
                Op::PopLayer => {
                    if levels.len() < 2 {
                        continue;
                    }
                    let layer = levels.pop().unwrap();
                    self.composite(&layer, levels.last().unwrap());
                }
            }
        }
        // Close any layers left open.
        while levels.len() > 1 {
            let layer = levels.pop().unwrap();
            self.composite(&layer, levels.last().unwrap());
        }
    }
}

/// Each polygon as a triangle fan from its first point, flattened to a
/// triangle list of `[x, y, 0, 0]` vertices.
pub(crate) fn fan_triangles<'a>(polys: impl Iterator<Item = &'a [[f32; 2]]>) -> Vec<f32> {
    let mut v = Vec::new();
    for p in polys {
        if p.len() < 3 {
            continue;
        }
        for i in 1..p.len() - 1 {
            for q in [p[0], p[i], p[i + 1]] {
                v.extend_from_slice(&[q[0], q[1], 0.0, 0.0]);
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PhysicalSize;
    use crate::gpu::ops::{FULL_UV, NO_TINT};
    use crate::render::Color;

    fn plan(ops: Vec<Op>) -> Plan {
        Plan {
            size: PhysicalSize::new(100, 50),
            background: Color::rgb(255, 0, 0),
            ops,
            partial: None,
        }
    }

    fn passes(r: &Recording) -> Vec<(Target, bool)> {
        r.cmds
            .iter()
            .filter_map(|c| match c {
                Cmd::Pass { target, clear } => Some((*target, clear.is_some())),
                _ => None,
            })
            .collect()
    }

    fn draws(r: &Recording) -> Vec<&Draw> {
        r.cmds
            .iter()
            .filter_map(|c| match c {
                Cmd::Draw(d) => Some(d),
                _ => None,
            })
            .collect()
    }

    fn a_box() -> Op {
        Op::Box {
            rect: [10.0, 10.0, 30.0, 20.0],
            radius: 0.0,
            stroke: 0.0,
            color: [0.5, 0.0, 0.0, 0.5],
        }
    }

    fn image(w: u32, h: u32) -> Arc<Pixmap> {
        Arc::new(Pixmap::new(PhysicalSize::new(w, h)))
    }

    fn draw_image(img: &Arc<Pixmap>, nearest: bool) -> Op {
        Op::Image {
            image: img.clone(),
            transform: Transform2D::identity(),
            rect: [0.0, 0.0, 16.0, 16.0],
            opacity: 1.0,
            uv: FULL_UV,
            tint: NO_TINT,
            nearest,
        }
    }

    #[test]
    fn a_frame_starts_cleared_to_the_premultiplied_background() {
        let r = record(&plan(vec![]), (0, 0), 4096);
        assert_eq!(
            r.cmds,
            vec![Cmd::Pass {
                target: Target::Frame,
                clear: Some([1.0, 0.0, 0.0, 1.0])
            }]
        );
    }

    #[test]
    fn a_partial_frame_keeps_the_target_and_scissors_its_draws() {
        let mut p = plan(vec![a_box()]);
        p.limit([10, 5, 30, 25]);
        let r = record(&p, (0, 0), 4096);
        assert_eq!(
            r.cmds[0],
            Cmd::Pass {
                target: Target::Frame,
                clear: None
            }
        );
        let d = draws(&r);
        assert!(!d.is_empty());
        assert!(d.iter().all(|d| d.scissor == [10, 5, 30, 25]));
    }

    #[test]
    fn a_box_is_one_quad_with_its_uniforms() {
        let r = record(&plan(vec![a_box()]), (0, 0), 4096);
        let d = draws(&r);
        assert_eq!(d.len(), 1);
        let d = d[0];
        assert_eq!(d.pipe, Pipe::Box);
        assert_eq!((d.first, d.count), (0, 6));
        assert_eq!(d.scissor, [0, 0, 100, 50]);
        assert_eq!(d.reference, 0);
        assert_eq!(&d.push[XF..XF + 4], &[0.02, 0.04, -1.0, -1.0]);
        assert_eq!(&d.push[BOX_RECT..BOX_RECT + 4], &[10.0, 10.0, 30.0, 20.0]);
        assert_eq!(&d.push[BOX_COLOR..BOX_COLOR + 4], &[0.5, 0.0, 0.0, 0.5]);
        // Padded by a pixel for the antialiased edge.
        assert_eq!(&r.vertices[..4], &[9.0, 9.0, 9.0, 9.0]);
        assert_eq!(r.vertices.len(), 24);
    }

    #[test]
    fn device_corners_map_to_vulkan_clip_corners() {
        let [sx, sy, ox, oy] = xf(100, 50);
        assert_eq!((0.0 * sx + ox, 0.0 * sy + oy), (-1.0, -1.0));
        assert_eq!((100.0 * sx + ox, 50.0 * sy + oy), (1.0, 1.0));
    }

    #[test]
    fn a_path_stencils_covers_and_resolves_through_the_scratch() {
        let path = Op::Path {
            contours: vec![vec![[10.0, 10.0], [40.0, 10.0], [40.0, 30.0], [10.0, 30.0]]],
            even_odd: false,
            color: [0.0, 0.0, 1.0, 1.0],
            bbox: [10, 10, 40, 30],
        };
        let r = record(&plan(vec![path]), (256, 128), 4096);
        assert_eq!(
            passes(&r),
            vec![
                (Target::Frame, true),
                (Target::Scratch, true),
                (Target::Frame, false)
            ]
        );
        let d = draws(&r);
        assert_eq!(
            d.iter().map(|d| d.pipe).collect::<Vec<_>>(),
            vec![Pipe::WindNonZero, Pipe::CoverNonZero, Pipe::Resolve]
        );
        // A quad is two fan triangles.
        assert_eq!(d[0].count, 6);
        // 4×4 samples per pixel over the 30×20 box.
        assert_eq!(r.scratch, (120, 80));
        assert!(r.fits((256, 128)));
        assert_eq!(d[0].scissor, [0, 0, 120, 80]);
        assert_eq!(d[2].texture, Some(Texture::Scratch));
        assert_eq!(d[2].push[RESOLVE_TAP], 1.0);
        assert_eq!(d[2].push[RESOLVE_INV], 1.0 / 256.0);
        // The box's corner lands on the scratch's top-left corner.
        let s = &d[0].push;
        assert_eq!(10.0 * s[0] + s[2], -1.0);
        assert_eq!(10.0 * s[1] + s[3], -1.0);
        // The resolve reads scratch texels from the box's corner.
        let v = d[2].first as usize * 4;
        assert_eq!(&r.vertices[v..v + 4], &[10.0, 10.0, 0.0, 0.0]);
    }

    #[test]
    fn even_odd_paths_use_their_own_pipelines() {
        let path = Op::Path {
            contours: vec![vec![[0.0, 0.0], [8.0, 0.0], [8.0, 8.0]]],
            even_odd: true,
            color: [1.0; 4],
            bbox: [0, 0, 8, 8],
        };
        let r = record(&plan(vec![path]), (64, 64), 4096);
        let pipes: Vec<_> = draws(&r).iter().map(|d| d.pipe).collect();
        assert_eq!(
            pipes,
            vec![Pipe::WindEvenOdd, Pipe::CoverEvenOdd, Pipe::Resolve]
        );
    }

    #[test]
    fn a_scratch_too_small_is_reported_and_big_paths_drop_samples() {
        let path = |w: i32| Op::Path {
            contours: vec![vec![[0.0, 0.0], [w as f32, 0.0], [w as f32, 10.0]]],
            even_odd: false,
            color: [1.0; 4],
            bbox: [0, 0, w, 10],
        };
        let r = record(&plan(vec![path(50)]), (64, 64), 4096);
        assert_eq!(r.scratch, (200, 40));
        assert!(!r.fits((64, 64)));
        assert_eq!(grow(r.scratch, (64, 64), 4096), (256, 64));
        // Past the size limit at 4×, 2× is used.
        let big = Plan {
            size: PhysicalSize::new(3000, 50),
            ..plan(vec![path(3000)])
        };
        let r = record(&big, (64, 64), 8192);
        assert_eq!(r.scratch, (6000, 20));
    }

    #[test]
    fn grow_is_a_power_of_two_within_limits() {
        assert_eq!(grow((1, 1), (0, 0), 4096), (64, 64));
        assert_eq!(grow((65, 300), (128, 0), 4096), (128, 512));
        assert_eq!(grow((9000, 10), (0, 0), 4096), (4096, 64));
    }

    #[test]
    fn a_layer_is_drawn_apart_and_composited_into_its_parent() {
        let r = record(
            &plan(vec![
                Op::PushLayer { opacity: 0.5 },
                a_box(),
                Op::PushLayer { opacity: 0.25 },
                a_box(),
                Op::PopLayer,
                Op::PopLayer,
            ]),
            (0, 0),
            4096,
        );
        assert_eq!(r.layers, 2);
        assert_eq!(
            passes(&r),
            vec![
                (Target::Frame, true),
                (Target::Layer(0), true),
                (Target::Layer(1), true),
                (Target::Layer(0), false),
                (Target::Frame, false),
            ]
        );
        let d = draws(&r);
        assert_eq!(d[2].texture, Some(Texture::Layer(1)));
        assert_eq!(d[2].push[TEX_OPACITY], 0.25);
        assert_eq!(d[2].push[TEX_PREMUL], 1.0);
        assert_eq!(d[2].push[TEX_AA], 0.0);
        assert_eq!(d[3].texture, Some(Texture::Layer(0)));
        assert_eq!(d[3].push[TEX_OPACITY], 0.5);
        assert_eq!(r.textures(), vec![Texture::Layer(1), Texture::Layer(0)]);
    }

    #[test]
    fn a_layer_left_open_is_still_composited() {
        let r = record(
            &plan(vec![Op::PushLayer { opacity: 0.5 }, a_box()]),
            (0, 0),
            4096,
        );
        let d = draws(&r);
        assert_eq!(d.last().unwrap().texture, Some(Texture::Layer(0)));
    }

    #[test]
    fn rect_clips_intersect_the_scissor_and_empty_ones_draw_nothing() {
        let r = record(
            &plan(vec![
                Op::PushClip(Clip::Rect([5, 5, 60, 40])),
                Op::PushClip(Clip::Rect([20, -10, 200, 30])),
                a_box(),
                Op::PopClip,
                a_box(),
                Op::PushClip(Clip::Rect([70, 0, 80, 10])),
                a_box(),
                Op::PopClip,
                Op::PopClip,
                a_box(),
            ]),
            (0, 0),
            4096,
        );
        let s: Vec<_> = draws(&r).iter().map(|d| d.scissor).collect();
        // [5,5,60,40] ∩ [70,0,80,10] is empty: that box isn't drawn.
        assert_eq!(s, vec![[20, 5, 60, 30], [5, 5, 60, 40], [0, 0, 100, 50]]);
    }

    #[test]
    fn polygon_clips_count_the_stencil_up_and_down() {
        let tri = vec![[0.0, 0.0], [50.0, 0.0], [0.0, 50.0]];
        let r = record(
            &plan(vec![
                Op::PushClip(Clip::Polygon(tri.clone())),
                Op::PushClip(Clip::Polygon(tri)),
                a_box(),
                Op::PopClip,
                a_box(),
                Op::PopClip,
                a_box(),
            ]),
            (0, 0),
            4096,
        );
        let d: Vec<_> = draws(&r).iter().map(|d| (d.pipe, d.reference)).collect();
        assert_eq!(
            d,
            vec![
                (Pipe::ClipInc, 0),
                (Pipe::ClipInc, 1),
                (Pipe::Box, 2),
                (Pipe::ClipDec, 2),
                (Pipe::Box, 1),
                (Pipe::ClipDec, 1),
                (Pipe::Box, 0),
            ]
        );
    }

    #[test]
    fn images_are_listed_once_and_sampled_as_asked() {
        let a = image(16, 16);
        let b = image(8, 4);
        let r = record(
            &plan(vec![
                draw_image(&a, false),
                draw_image(&b, true),
                draw_image(&a, true),
            ]),
            (0, 0),
            4096,
        );
        assert_eq!(r.images.len(), 2);
        assert!(Arc::ptr_eq(&r.images[0], &a));
        let t: Vec<_> = draws(&r).iter().map(|d| d.texture).collect();
        assert_eq!(
            t,
            vec![
                Some(Texture::Image {
                    index: 0,
                    nearest: false
                }),
                Some(Texture::Image {
                    index: 1,
                    nearest: true
                }),
                Some(Texture::Image {
                    index: 0,
                    nearest: true
                }),
            ]
        );
        // Kept half a texel inside the image.
        let d = &draws(&r)[1];
        assert_eq!(
            &d.push[TEX_CLAMP..TEX_CLAMP + 4],
            &[1.0 / 16.0, 1.0 / 8.0, 1.0 - 1.0 / 16.0, 1.0 - 1.0 / 8.0]
        );
        assert_eq!(d.push[TEX_PREMUL], 0.0);
        assert_eq!(d.push[TEX_AA], 1.0);
    }

    #[test]
    fn empty_and_oversized_images_are_skipped() {
        let r = record(
            &plan(vec![
                draw_image(&image(0, 4), false),
                draw_image(&image(5000, 4), false),
            ]),
            (0, 0),
            4096,
        );
        assert!(r.images.is_empty());
        assert!(draws(&r).is_empty());
    }

    #[test]
    fn push_constant_offsets_fit_the_range() {
        for i in [BOX_STROKE, TEX_AA, RESOLVE_TAP, FLAT_COLOR + 3] {
            assert!(i < PUSH_FLOATS);
        }
        // Vulkan guarantees 128 bytes of push constants.
        const { assert!(PUSH_BYTES <= 128 && PUSH_BYTES.is_multiple_of(4)) };
    }
}
