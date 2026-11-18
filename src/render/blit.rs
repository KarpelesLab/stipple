//! The software compositor: draws a lowered scene into a [`Pixmap`], blitting
//! image regions ([`Scene::draw_image_region`]) and pixel-aligned solid rects
//! itself, and handing everything else to `oxideav-raster` in runs.
//!
//! `oxideav-raster` draws every primitive through a coverage mask the size of
//! the canvas it renders, and samples images through a general per-pixel
//! path. That is the right tool for vector shapes and text, but a game draws
//! thousands of small bitmaps a frame. So the scene is walked in paint order:
//!
//! - an image region is blitted straight from its bitmap into the frame, with
//!   fast paths for upright regions drawn unscaled or scaled by whole numbers
//!   (a column/row index table, no per-pixel transform), a separable bilinear
//!   path for other upright ones, and a per-pixel inverse-mapped path for
//!   rotated ones;
//! - a solid rect whose edges fall on device pixels (a game's background, a
//!   health bar) is filled directly;
//! - everything else collects into a run, rasterized by `oxideav-raster` for
//!   just the box the run covers and composited over the frame before the
//!   next blit. The first run, when nothing is drawn yet, is rasterized over
//!   the whole frame with the background, as a scene without regions is.
//!
//! Groups (clips, transforms) holding regions are descended into when they
//! are upright rect clips or plain transforms at full opacity; any other
//! group is rasterized whole, its regions turned into plain image nodes.
//!
//! [`Scene::draw_image_region`]: crate::render::Scene::draw_image_region

use crate::geometry::Rect;
use crate::render::glyph_mask;
use crate::render::glyphs::GlyphDraw;
use crate::render::lower::{self, Blits, Bounds, glyphs_index, nearest_upscale, sprite_index};
use crate::render::sprite::{Filter, SpriteDraw};
use crate::render::{Color, Pixmap};
use oxideav_core::{Group, ImageRef, Node, Paint, PathNode, Transform2D};

/// Rasterizes `nodes` (in device pixels) for the device rect `[x0, y0, x1,
/// y1]` over `background`, into a pixmap of that rect's size.
pub(crate) type RasterRun<'a> = &'a dyn Fn(&[Node], [i32; 4], Color) -> Pixmap;

pub(crate) struct Compositor<'a> {
    out: Pixmap,
    blits: &'a Blits,
    background: Color,
    raster: RasterRun<'a>,
    /// Nothing is drawn yet: the frame holds only the background.
    fresh: bool,
    /// The run waiting to be rasterized, in device pixels.
    pending: Vec<Node>,
    /// The box the run covers.
    pending_bounds: Option<Bounds>,
    target: [i32; 4],
}

/// A group's clip, as the compositor takes it.
enum Inner {
    Clip([i32; 4]),
    Empty,
    Opaque,
}

fn upright(t: &Transform2D) -> bool {
    t.b == 0.0 && t.c == 0.0
}

/// Whether `nodes` hold a marker of something the compositor draws itself:
/// an image region, or a glyph run.
fn has_sprites(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| match n {
        Node::Image(img) => sprite_index(img).is_some() || glyphs_index(img).is_some(),
        Node::Group(g) => has_sprites(&g.children),
        _ => false,
    })
}

fn intersect(a: [i32; 4], b: [i32; 4]) -> [i32; 4] {
    [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ]
}

fn empty(r: [i32; 4]) -> bool {
    r[2] <= r[0] || r[3] <= r[1]
}

fn round_rect(b: Bounds) -> [i32; 4] {
    [
        b[0].round() as i32,
        b[1].round() as i32,
        b[2].round() as i32,
        b[3].round() as i32,
    ]
}

impl<'a> Compositor<'a> {
    pub fn new(
        width: u32,
        height: u32,
        background: Color,
        blits: &'a Blits,
        raster: RasterRun<'a>,
    ) -> Self {
        Self {
            // Cleared to the background when something is first blitted.
            out: Pixmap::new(crate::geometry::PhysicalSize::new(width, height)),
            blits,
            background,
            raster,
            fresh: true,
            pending: Vec::new(),
            pending_bounds: None,
            target: [0, 0, width as i32, height as i32],
        }
    }

    /// Draw `nodes` through `t` (their coordinates to device pixels).
    pub fn run(mut self, nodes: &[Node], t: &Transform2D) -> Pixmap {
        let clip = self.target;
        self.walk(nodes, t, clip);
        self.flush();
        self.start();
        self.out
    }

    /// Before drawing into the frame directly: clear it to the background
    /// if nothing has been drawn yet.
    fn start(&mut self) {
        if !self.fresh {
            return;
        }
        self.fresh = false;
        let b = self.background;
        if b.a != 0 {
            let px = [b.r, b.g, b.b, b.a];
            for d in self.out.as_bytes_mut().as_chunks_mut::<4>().0 {
                *d = px;
            }
        }
    }

    fn walk(&mut self, nodes: &[Node], t: &Transform2D, clip: [i32; 4]) {
        for node in nodes {
            match node {
                Node::Image(img) => {
                    if let Some(i) = sprite_index(img) {
                        self.flush();
                        if let Some(s) = self.blits.sprites.get(i) {
                            self.start();
                            blit_sprite(&mut self.out, s, img, t, clip);
                        }
                        continue;
                    }
                    if let Some(i) = glyphs_index(img) {
                        if let Some(run) = self.blits.glyphs.get(i) {
                            self.glyphs(run, &t.compose(&img.transform), clip);
                        }
                        continue;
                    }
                }
                Node::Group(g) if has_sprites(&g.children) => {
                    let local = t.compose(&g.transform);
                    let inner = if g.opacity < 1.0 || g.cache_key.is_some() {
                        Inner::Opaque
                    } else {
                        match &g.clip {
                            None => Inner::Clip(clip),
                            Some(c) if upright(&local) && lower::is_rect(c) => {
                                match lower::path_bounds(c) {
                                    Some(b) => {
                                        let r = intersect(
                                            clip,
                                            round_rect(lower::map_bounds(&local, b)),
                                        );
                                        if empty(r) {
                                            Inner::Empty
                                        } else {
                                            Inner::Clip(r)
                                        }
                                    }
                                    None => Inner::Empty,
                                }
                            }
                            Some(_) => Inner::Opaque,
                        }
                    };
                    match inner {
                        Inner::Clip(r) => self.walk(&g.children, &local, r),
                        Inner::Empty => {}
                        Inner::Opaque => {
                            let plain = self.unmark(node, t);
                            self.push(plain, t, clip);
                        }
                    }
                    continue;
                }
                // A pixel-aligned solid rect is filled directly.
                Node::Path(p) if self.fill(p, t, clip) => continue,
                _ => {}
            }
            self.push(node.clone(), t, clip);
        }
    }

    /// `node` with its region markers made into plain image nodes (`t` takes
    /// its coordinates to device pixels).
    fn unmark(&self, node: &Node, t: &Transform2D) -> Node {
        match node {
            Node::Image(img) => {
                if let Some(run) = glyphs_index(img).and_then(|i| self.blits.glyphs.get(i)) {
                    return Node::Group(Group {
                        transform: img.transform,
                        children: vec![run.node()],
                        ..Group::new()
                    });
                }
                match sprite_index(img).and_then(|i| self.blits.sprites.get(i)) {
                    Some(s) => {
                        let k = nearest_upscale(s, &img.bounds, &t.compose(&img.transform));
                        s.to_node(img.bounds, img.transform, k)
                    }
                    None => node.clone(),
                }
            }
            Node::Group(g) if has_sprites(&g.children) => {
                let local = t.compose(&g.transform);
                Node::Group(Group {
                    children: g.children.iter().map(|c| self.unmark(c, &local)).collect(),
                    ..g.clone()
                })
            }
            _ => node.clone(),
        }
    }

    /// Fill `p` directly if it is a solid rect on device-pixel edges.
    fn fill(&mut self, p: &PathNode, t: &Transform2D, clip: [i32; 4]) -> bool {
        let (Some(Paint::Solid(c)), None) = (&p.fill, &p.stroke) else {
            return false;
        };
        if !upright(t) || !lower::is_rect(&p.path) {
            return false;
        }
        let Some(b) = lower::path_bounds(&p.path) else {
            return false;
        };
        let d = lower::map_bounds(t, b);
        let r = round_rect(d);
        if (0..4).any(|i| (d[i] - r[i] as f32).abs() > 1e-3) {
            return false;
        }
        self.flush();
        let r = intersect(r, clip);
        self.start();
        if empty(r) || c.a == 0 {
            return true;
        }
        let stride = self.out.stride();
        let px = [c.r, c.g, c.b, c.a];
        let buf = self.out.as_bytes_mut();
        for y in r[1]..r[3] {
            let row =
                &mut buf[y as usize * stride + r[0] as usize * 4..][..(r[2] - r[0]) as usize * 4];
            if c.a == 255 {
                for d in row.as_chunks_mut::<4>().0 {
                    d.copy_from_slice(&px);
                }
            } else {
                for d in row.as_chunks_mut::<4>().0 {
                    blend(d, c.r as u32, c.g as u32, c.b as u32, c.a as u32);
                }
            }
        }
        true
    }

    /// Draw glyph run `run`, through `t` and clipped to `clip`: each glyph
    /// blended from its coverage mask in the run's color, or, one that has
    /// none (a color emoji), filled by the rasterizer.
    fn glyphs(&mut self, run: &GlyphDraw, t: &Transform2D, clip: [i32; 4]) {
        let c = run.color();
        if c.a == 0 {
            return;
        }
        let (cr, cg, cb, ca) = (c.r as u32, c.g as u32, c.b as u32, c.a as u32);
        for g in run.glyphs() {
            let local = t.compose(&run.place(g));
            let outline = || run.outline(g, glyph_mask::MASK_COLOR);
            let Some(placed) = glyph_mask::place(g.key, &local, outline) else {
                if let Some(node) = run.glyph_node(g, false) {
                    self.push(node, t, clip);
                }
                continue;
            };
            let Some(mask) = placed.mask else {
                continue;
            };
            let (ox, oy) = (placed.origin.0 + mask.x, placed.origin.1 + mask.y);
            let r = intersect(clip, [ox, oy, ox + mask.w as i32, oy + mask.h as i32]);
            if empty(r) {
                continue;
            }
            // A run waiting under the glyph is drawn first; one beside it
            // can wait.
            if let Some(p) = self.pending_bounds
                && (p[0].floor() as i32) < r[2]
                && r[0] < p[2].ceil() as i32
                && (p[1].floor() as i32) < r[3]
                && r[1] < p[3].ceil() as i32
            {
                self.flush();
            }
            self.start();
            let stride = self.out.stride();
            let buf = self.out.as_bytes_mut();
            let (w, cols) = (mask.w as usize, (r[0] - ox) as usize..(r[2] - ox) as usize);
            for y in r[1]..r[3] {
                let m = &mask.coverage[(y - oy) as usize * w..][cols.clone()];
                let row = &mut buf[y as usize * stride + r[0] as usize * 4..][..m.len() * 4];
                for (d, &k) in row.as_chunks_mut::<4>().0.iter_mut().zip(m) {
                    if k != 0 {
                        blend(d, cr, cg, cb, (k as u32 * ca + 127) / 255);
                    }
                }
            }
        }
    }

    /// Add `node`, drawn through `t` and clipped to `clip`, to the run.
    fn push(&mut self, node: Node, t: &Transform2D, clip: [i32; 4]) {
        let c = [
            clip[0] as f32,
            clip[1] as f32,
            clip[2] as f32,
            clip[3] as f32,
        ];
        let b = match lower::subtree_bounds(std::slice::from_ref(&node)) {
            Some(b) => {
                let b = lower::map_bounds(t, b);
                // Room for antialiasing.
                [
                    (b[0] - 2.0).max(c[0]),
                    (b[1] - 2.0).max(c[1]),
                    (b[2] + 2.0).min(c[2]),
                    (b[3] + 2.0).min(c[3]),
                ]
            }
            None => c,
        };
        if b[2] <= b[0] || b[3] <= b[1] {
            return;
        }
        self.pending_bounds = Some(match self.pending_bounds {
            Some(p) => [
                p[0].min(b[0]),
                p[1].min(b[1]),
                p[2].max(b[2]),
                p[3].max(b[3]),
            ],
            None => b,
        });
        let mut wrapped = Node::Group(Group {
            transform: *t,
            children: vec![node],
            ..Group::new()
        });
        if clip != self.target {
            let r = Rect::from_xywh(
                clip[0] as f64,
                clip[1] as f64,
                (clip[2] - clip[0]) as f64,
                (clip[3] - clip[1]) as f64,
            );
            wrapped = Node::Group(Group {
                clip: Some(crate::render::scene::rect_path(r)),
                children: vec![wrapped],
                ..Group::new()
            });
        }
        self.pending.push(wrapped);
    }

    /// Rasterize the run and composite it over the frame.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let nodes = std::mem::take(&mut self.pending);
        let Some(b) = self.pending_bounds.take() else {
            return;
        };
        if self.fresh {
            self.fresh = false;
            self.out = (self.raster)(&nodes, self.target, self.background);
            return;
        }
        let r = intersect(
            [
                b[0].floor() as i32,
                b[1].floor() as i32,
                b[2].ceil() as i32,
                b[3].ceil() as i32,
            ],
            self.target,
        );
        if empty(r) {
            return;
        }
        let run = (self.raster)(&nodes, r, Color::TRANSPARENT);
        over(&mut self.out, &run, r[0] as usize, r[1] as usize);
    }
}

/// Source-over a straight-alpha pixel `(r, g, b)` at alpha `sa` onto the
/// straight-alpha pixel `d`, rounding as `oxideav-raster` does.
#[inline(always)]
pub(crate) fn blend(d: &mut [u8; 4], r: u32, g: u32, b: u32, sa: u32) {
    if sa == 0 {
        return;
    }
    if sa >= 255 {
        d[0] = r as u8;
        d[1] = g as u8;
        d[2] = b as u8;
        d[3] = 255;
        return;
    }
    blend_premul(
        d,
        (r * sa + 127) / 255,
        (g * sa + 127) / 255,
        (b * sa + 127) / 255,
        sa,
    );
}

/// Source-over a premultiplied pixel onto the straight-alpha pixel `d`.
#[inline(always)]
fn blend_premul(d: &mut [u8; 4], sr: u32, sg: u32, sb: u32, sa: u32) {
    if sa == 0 {
        return;
    }
    let inv = 255 - sa.min(255);
    let da0 = d[3] as u32;
    if da0 == 255 {
        d[0] = (sr + (d[0] as u32 * inv + 127) / 255).min(255) as u8;
        d[1] = (sg + (d[1] as u32 * inv + 127) / 255).min(255) as u8;
        d[2] = (sb + (d[2] as u32 * inv + 127) / 255).min(255) as u8;
        return;
    }
    let dr = (d[0] as u32 * da0 + 127) / 255;
    let dg = (d[1] as u32 * da0 + 127) / 255;
    let db = (d[2] as u32 * da0 + 127) / 255;
    let or = sr + (dr * inv + 127) / 255;
    let og = sg + (dg * inv + 127) / 255;
    let ob = sb + (db * inv + 127) / 255;
    let oa = sa + (da0 * inv + 127) / 255;
    if oa == 0 {
        return;
    }
    d[0] = ((or * 255 + oa / 2) / oa).min(255) as u8;
    d[1] = ((og * 255 + oa / 2) / oa).min(255) as u8;
    d[2] = ((ob * 255 + oa / 2) / oa).min(255) as u8;
    d[3] = oa.min(255) as u8;
}

/// Source-over a row of straight-alpha pixels onto `dst`, copying opaque
/// runs whole.
fn blend_row(dst: &mut [[u8; 4]], src: &[[u8; 4]]) {
    let n = dst.len().min(src.len());
    let mut i = 0;
    while i < n {
        if src[i][3] == 255 {
            let mut j = i + 1;
            while j < n && src[j][3] == 255 {
                j += 1;
            }
            dst[i..j].copy_from_slice(&src[i..j]);
            i = j;
        } else {
            let p = src[i];
            blend(
                &mut dst[i],
                p[0] as u32,
                p[1] as u32,
                p[2] as u32,
                p[3] as u32,
            );
            i += 1;
        }
    }
}

/// Composite the straight-alpha `src` over `dst` with its top-left at `(x,
/// y)` (inside `dst`).
fn over(dst: &mut Pixmap, src: &Pixmap, x: usize, y: usize) {
    let (sw, sh) = (src.size().width as usize, src.size().height as usize);
    let (dw, dh) = (dst.size().width as usize, dst.size().height as usize);
    let w = sw.min(dw.saturating_sub(x));
    let h = sh.min(dh.saturating_sub(y));
    let (ss, ds) = (src.stride(), dst.stride());
    let s = src.as_bytes();
    let d = dst.as_bytes_mut();
    for row in 0..h {
        let srow = s[row * ss..][..w * 4].as_chunks::<4>().0;
        let drow = &mut d[(y + row) * ds + x * 4..][..w * 4];
        blend_row(drow.as_chunks_mut::<4>().0, srow);
    }
}

/// No source pixel for this column or row.
const NONE: u32 = u32::MAX;

/// The tint and opacity of a region as multipliers (`0..=255`).
#[derive(Clone, Copy)]
struct Modulate {
    r: u32,
    g: u32,
    b: u32,
    a: u32,
}

impl Modulate {
    fn of(s: &SpriteDraw) -> Self {
        let [r, g, b, a] = s.tint;
        Self {
            r: r as u32,
            g: g as u32,
            b: b as u32,
            a: (a as f32 * s.opacity).round() as u32,
        }
    }

    #[inline(always)]
    fn put(self, d: &mut [u8; 4], p: &[u8; 4]) {
        let a = (p[3] as u32 * self.a + 127) / 255;
        blend(
            d,
            (p[0] as u32 * self.r + 127) / 255,
            (p[1] as u32 * self.g + 127) / 255,
            (p[2] as u32 * self.b + 127) / 255,
            a,
        );
    }
}

/// Blit region `s`, whose placeholder is `img`, drawn through `t` (to device
/// pixels) and clipped to `clip`, into `out`.
pub(crate) fn blit_sprite(
    out: &mut Pixmap,
    s: &SpriteDraw,
    img: &ImageRef,
    t: &Transform2D,
    clip: [i32; 4],
) {
    let l = t.compose(&img.transform);
    let b = img.bounds;
    let (sw, sh) = (s.width() as f64, s.height() as f64);
    let (bx, by, bw, bh) = (b.x as f64, b.y as f64, b.width as f64, b.height as f64);
    // Source pixels to the placeholder's coordinates, flips included.
    let (kx, ox) = if s.flip_x {
        (-bw / sw, bx + bw)
    } else {
        (bw / sw, bx)
    };
    let (ky, oy) = if s.flip_y {
        (-bh / sh, by + bh)
    } else {
        (bh / sh, by)
    };
    let (la, lb, lc, ld, le, lf) = (
        l.a as f64, l.b as f64, l.c as f64, l.d as f64, l.e as f64, l.f as f64,
    );
    // Source pixels to device pixels.
    let m = [
        la * kx,
        lb * kx,
        lc * ky,
        ld * ky,
        la * ox + lc * oy + le,
        lb * ox + ld * oy + lf,
    ];
    let corner = |u: f64, v: f64| (m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5]);
    let pts = [
        corner(0.0, 0.0),
        corner(sw, 0.0),
        corner(0.0, sh),
        corner(sw, sh),
    ];
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
        return;
    }
    let area = [
        (x0.floor() as i32).max(clip[0]),
        (y0.floor() as i32).max(clip[1]),
        (x1.ceil() as i32).min(clip[2]),
        (y1.ceil() as i32).min(clip[3]),
    ];
    if empty(area) {
        return;
    }
    let md = Modulate::of(s);
    if md.a == 0 {
        return;
    }
    let axis = m[1].abs() < 1e-9 && m[2].abs() < 1e-9;
    if axis {
        let whole = |v: f64| (v - v.round()).abs() < 1e-6;
        let unscaled = (m[0].abs() - 1.0).abs() < 1e-9
            && (m[3].abs() - 1.0).abs() < 1e-9
            && whole(m[4])
            && whole(m[5]);
        if s.filter == Filter::Nearest || unscaled {
            blit_table(out, s, &m, area, md);
        } else {
            blit_bilinear_axis(out, s, &m, area, md);
        }
    } else {
        blit_general(out, s, &m, area, md);
    }
}

/// For each device column (row) in `lo..hi`, the source column (row) whose
/// pixel covers its center: `scale` and `offset` map source to device along
/// the axis; `first` is the source rect's first column (row), `n` its size.
fn axis_table(lo: i32, hi: i32, scale: f64, offset: f64, first: u32, n: u32) -> Vec<u32> {
    (lo..hi)
        .map(|x| {
            let u = (x as f64 + 0.5 - offset) / scale;
            if u >= 0.0 && u < n as f64 {
                first + (u.floor() as u32).min(n - 1)
            } else {
                NONE
            }
        })
        .collect()
}

/// An upright region drawn nearest-neighbor, or unscaled at whole pixels:
/// every device pixel takes the source pixel under its center, found through
/// a per-column and a per-row table.
fn blit_table(out: &mut Pixmap, s: &SpriteDraw, m: &[f64; 6], area: [i32; 4], md: Modulate) {
    let cols = axis_table(area[0], area[2], m[0], m[4], s.src[0], s.width());
    let rows = axis_table(area[1], area[3], m[3], m[5], s.src[1], s.height());
    // Trim the columns no source pixel lands on.
    let Some(first) = cols.iter().position(|&c| c != NONE) else {
        return;
    };
    let last = cols.iter().rposition(|&c| c != NONE).unwrap_or(first);
    let cols = &cols[first..=last];
    let x0 = area[0] as usize + first;
    // A run of consecutive source columns copies a row slice at a time.
    let contiguous = cols.windows(2).all(|w| w[1] == w[0] + 1);
    let plain = s.plain();
    let (ss, ds) = (s.image.stride(), out.stride());
    let src = s.image.as_bytes();
    let dst = out.as_bytes_mut();
    for (i, &sy) in rows.iter().enumerate() {
        if sy == NONE {
            continue;
        }
        let y = area[1] as usize + i;
        let drow = &mut dst[y * ds + x0 * 4..][..cols.len() * 4];
        let srow = src[sy as usize * ss..][..ss].as_chunks::<4>().0;
        if contiguous {
            let srow = &srow[cols[0] as usize..][..cols.len()];
            if plain {
                blend_row(drow.as_chunks_mut::<4>().0, srow);
            } else {
                for (d, p) in drow.as_chunks_mut::<4>().0.iter_mut().zip(srow) {
                    md.put(d, p);
                }
            }
        } else if plain {
            for (d, &sx) in drow.as_chunks_mut::<4>().0.iter_mut().zip(cols) {
                if sx != NONE {
                    let p = &srow[sx as usize];
                    blend(d, p[0] as u32, p[1] as u32, p[2] as u32, p[3] as u32);
                }
            }
        } else {
            for (d, &sx) in drow.as_chunks_mut::<4>().0.iter_mut().zip(cols) {
                if sx != NONE {
                    md.put(d, &srow[sx as usize]);
                }
            }
        }
    }
}

/// One axis of a bilinear tap: two source indices, the weight of the second
/// (`0..=256`) and the pixel's coverage by the region (`0..=256`).
#[derive(Clone, Copy)]
struct Tap {
    i0: u32,
    i1: u32,
    f: u32,
    cov: u32,
}

/// Bilinear taps for device columns (rows) `lo..hi`, clamped to the region
/// so no neighbor in the atlas bleeds in.
fn bilinear_axis(lo: i32, hi: i32, scale: f64, offset: f64, first: u32, n: u32) -> Vec<Tap> {
    let e0 = offset.min(offset + scale * n as f64);
    let e1 = offset.max(offset + scale * n as f64);
    (lo..hi)
        .map(|x| {
            let xf = x as f64;
            let cov = ((xf + 1.0).min(e1) - xf.max(e0)).clamp(0.0, 1.0);
            let u = (xf + 0.5 - offset) / scale - 0.5;
            let i = u.floor();
            let f = ((u - i) * 256.0).round() as u32;
            let clamp = |k: f64| first + (k.max(0.0) as u32).min(n - 1);
            Tap {
                i0: clamp(i),
                i1: clamp(i + 1.0),
                f,
                cov: (cov * 256.0).round() as u32,
            }
        })
        .collect()
}

/// Premultiply a pixel.
#[inline(always)]
fn premul(p: &[u8; 4]) -> [u32; 4] {
    let a = p[3] as u32;
    [
        (p[0] as u32 * a + 127) / 255,
        (p[1] as u32 * a + 127) / 255,
        (p[2] as u32 * a + 127) / 255,
        a,
    ]
}

/// Write a premultiplied sample `c` at coverage `cov` (`0..=65536`) with
/// the region's tint and opacity.
#[inline(always)]
fn put_premul(d: &mut [u8; 4], c: [u32; 4], cov: u32, md: Modulate) {
    // Coverage and opacity as one factor, 0..=255.
    let k = ((cov as u64 * md.a as u64 + 32768) >> 16) as u32;
    if k == 0 || c[3] == 0 {
        return;
    }
    let sa = (c[3] * k + 127) / 255;
    let ch = |v: u32, t: u32| ((v * t + 127) / 255 * k + 127) / 255;
    blend_premul(d, ch(c[0], md.r), ch(c[1], md.g), ch(c[2], md.b), sa);
}

/// An upright region drawn bilinear at any scale and position.
fn blit_bilinear_axis(
    out: &mut Pixmap,
    s: &SpriteDraw,
    m: &[f64; 6],
    area: [i32; 4],
    md: Modulate,
) {
    let cols = bilinear_axis(area[0], area[2], m[0], m[4], s.src[0], s.width());
    let rows = bilinear_axis(area[1], area[3], m[3], m[5], s.src[1], s.height());
    let (ss, ds) = (s.image.stride(), out.stride());
    let src = s.image.as_bytes();
    let dst = out.as_bytes_mut();
    let x0 = area[0] as usize;
    for (i, ty) in rows.iter().enumerate() {
        if ty.cov == 0 {
            continue;
        }
        let y = area[1] as usize + i;
        let r0 = src[ty.i0 as usize * ss..][..ss].as_chunks::<4>().0;
        let r1 = src[ty.i1 as usize * ss..][..ss].as_chunks::<4>().0;
        let drow = &mut dst[y * ds + x0 * 4..][..cols.len() * 4];
        for (d, tx) in drow.as_chunks_mut::<4>().0.iter_mut().zip(&cols) {
            let cov = tx.cov * ty.cov;
            if cov == 0 {
                continue;
            }
            let p00 = premul(&r0[tx.i0 as usize]);
            let p01 = premul(&r0[tx.i1 as usize]);
            let p10 = premul(&r1[tx.i0 as usize]);
            let p11 = premul(&r1[tx.i1 as usize]);
            let (fx, fy) = (tx.f, ty.f);
            let w00 = (256 - fx) * (256 - fy);
            let w01 = fx * (256 - fy);
            let w10 = (256 - fx) * fy;
            let w11 = fx * fy;
            let mut c = [0u32; 4];
            for k in 0..4 {
                c[k] = (p00[k] * w00 + p01[k] * w01 + p10[k] * w10 + p11[k] * w11 + 32768) >> 16;
            }
            put_premul(d, c, cov, md);
        }
    }
}

/// A rotated (or sheared) region: every device pixel in its box is mapped
/// back into the source.
fn blit_general(out: &mut Pixmap, s: &SpriteDraw, m: &[f64; 6], area: [i32; 4], md: Modulate) {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return;
    }
    // Device to source pixels.
    let ia = m[3] / det;
    let ib = -m[1] / det;
    let ic = -m[2] / det;
    let id = m[0] / det;
    let ie = -(ia * m[4] + ic * m[5]);
    let if_ = -(ib * m[4] + id * m[5]);
    let (sw, sh) = (s.width() as f64, s.height() as f64);
    // Device pixels per source pixel along each source axis, for the
    // antialiased edge.
    let (pu, pv) = (m[0].hypot(m[1]), m[2].hypot(m[3]));
    let nearest = s.filter == Filter::Nearest;
    let (ss, ds) = (s.image.stride(), out.stride());
    let src = s.image.as_bytes();
    let dst = out.as_bytes_mut();
    let (fx0, fy0) = (s.src[0] as usize, s.src[1] as usize);
    let (wmax, hmax) = (s.width() as i64 - 1, s.height() as i64 - 1);
    let texel = |x: i64, y: i64| -> &[u8; 4] {
        let (x, y) = (x.clamp(0, wmax) as usize, y.clamp(0, hmax) as usize);
        src[(fy0 + y) * ss + (fx0 + x) * 4..][..4]
            .try_into()
            .expect("four bytes")
    };
    for y in area[1]..area[3] {
        let cy = y as f64 + 0.5;
        let drow = dst[y as usize * ds..][..ds].as_chunks_mut::<4>().0;
        for x in area[0]..area[2] {
            let cx = x as f64 + 0.5;
            let u = ia * cx + ic * cy + ie;
            let v = ib * cx + id * cy + if_;
            let d = &mut drow[x as usize];
            if nearest {
                if u < 0.0 || v < 0.0 || u >= sw || v >= sh {
                    continue;
                }
                let p = texel(u as i64, v as i64);
                if s.plain() {
                    blend(d, p[0] as u32, p[1] as u32, p[2] as u32, p[3] as u32);
                } else {
                    md.put(d, p);
                }
                continue;
            }
            let cu = ((u.min(sw - u)) * pu + 0.5).clamp(0.0, 1.0);
            let cv = ((v.min(sh - v)) * pv + 0.5).clamp(0.0, 1.0);
            let cov = (cu * cv * 65536.0) as u32;
            if cov == 0 {
                continue;
            }
            let (su, sv) = (u - 0.5, v - 0.5);
            let (iu, iv) = (su.floor(), sv.floor());
            let fx = ((su - iu) * 256.0) as u32;
            let fy = ((sv - iv) * 256.0) as u32;
            let (iu, iv) = (iu as i64, iv as i64);
            let p00 = premul(texel(iu, iv));
            let p01 = premul(texel(iu + 1, iv));
            let p10 = premul(texel(iu, iv + 1));
            let p11 = premul(texel(iu + 1, iv + 1));
            let w00 = (256 - fx) * (256 - fy);
            let w01 = fx * (256 - fy);
            let w10 = (256 - fx) * fy;
            let w11 = fx * fy;
            let mut c = [0u32; 4];
            for k in 0..4 {
                c[k] = (p00[k] * w00 + p01[k] * w01 + p10[k] * w10 + p11[k] * w11 + 32768) >> 16;
            }
            put_premul(d, c, cov, md);
        }
    }
}
