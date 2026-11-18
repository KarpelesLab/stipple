//! Glyph coverage masks for the software compositor (`render::blit`).
//!
//! Filling a glyph's outline costs the rasterizer a mask the size of the area
//! it draws on, and a terminal grid ([glyph runs](crate::render::GlyphRun))
//! draws ten thousand glyphs a frame, the same hundred over and over. So the
//! compositor rasterizes each glyph once, as coverage, per glyph (face,
//! glyph, size and synthesis), device transform and position within a pixel
//! (snapped to a sixteenth, as the GPU renderers' glyph atlas does), keeps
//! it, and blends it tinted with the run's color wherever it's drawn.

use crate::render::Color;
use oxideav_core::{Node, Paint, Transform2D};
use oxideav_raster::Renderer;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Positions within a pixel a glyph is rasterized for, per axis.
pub(crate) const SUBPIXEL: f32 = 16.0;
/// A glyph larger than this on either side is drawn as an outline.
const MAX_SIDE: u32 = 256;
/// Past this many bytes of masks the cache starts over.
const MAX_BYTES: usize = 16 << 20;

/// The opaque white a glyph's outline is rasterized in for its mask.
pub(crate) const MASK_COLOR: Color = Color::WHITE;

/// A glyph's coverage, `w`×`h` bytes, its top-left at `(x, y)` from the
/// whole pixel its origin was snapped into.
pub(crate) struct Mask {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub coverage: Vec<u8>,
}

/// What a glyph is drawn from.
#[derive(Clone)]
enum Entry {
    /// It covers nothing.
    Empty,
    Mask(Rc<Mask>),
    /// Not all solid paint (a color emoji), or too large: drawn as an
    /// outline.
    Outline,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    glyph: u64,
    /// The linear part of the transform, in ten-thousandths.
    linear: [i32; 4],
    phase: (u8, u8),
}

/// A hasher for small keys of integers that only this process sees
/// (FxHash's multiply-rotate): the glyph caches look up a key per glyph
/// drawn, and SipHash would be most of that.
#[derive(Clone, Copy, Default)]
pub(crate) struct FastHasher(u64);

impl std::hash::Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(b));
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn write_u32(&mut self, v: u32) {
        self.write_u64(v as u64);
    }

    fn write_u16(&mut self, v: u16) {
        self.write_u64(v as u64);
    }

    fn write_u8(&mut self, v: u8) {
        self.write_u64(v as u64);
    }

    fn write_i32(&mut self, v: i32) {
        self.write_u64(v as u32 as u64);
    }

    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
}

/// Builds [`FastHasher`]s.
pub(crate) type FastHash = std::hash::BuildHasherDefault<FastHasher>;

#[derive(Default)]
struct Cache {
    masks: HashMap<Key, Entry, FastHash>,
    bytes: usize,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

/// A glyph drawn through a transform, snapped: the whole device pixel its
/// origin falls in, and its mask (`None`: it covers nothing).
pub(crate) struct Placed {
    pub origin: (i32, i32),
    pub mask: Option<Rc<Mask>>,
}

/// Whether every paint under `nodes` is solid (an outline glyph; not a
/// color emoji's gradients or bitmap).
fn solid(nodes: &[Node]) -> bool {
    nodes.iter().all(|n| match n {
        Node::Path(p) => [p.fill.as_ref(), p.stroke.as_ref().map(|s| &s.paint)]
            .into_iter()
            .flatten()
            .all(|paint| matches!(paint, Paint::Solid(_))),
        Node::Group(g) => g.opacity >= 1.0 && g.clip.is_none() && solid(&g.children),
        _ => false,
    })
}

/// Glyph `key` drawn through `t` (to device pixels): its mask, rasterized
/// the first time from `outline` (the glyph at its origin, painted
/// [`MASK_COLOR`]). `None` when it is to be drawn as an outline instead.
pub(crate) fn place(
    key: u64,
    t: &Transform2D,
    outline: impl FnOnce() -> Option<Node>,
) -> Option<Placed> {
    let q = |v: f32| (v * SUBPIXEL).round() / SUBPIXEL;
    let (ex, fy) = (q(t.e), q(t.f));
    if !ex.is_finite() || !fy.is_finite() {
        return None;
    }
    let (bx, by) = (ex.floor(), fy.floor());
    let (px, py) = (ex - bx, fy - by);
    let k = |v: f32| (v * 1e4).round() as i32;
    let key = Key {
        glyph: key,
        linear: [k(t.a), k(t.b), k(t.c), k(t.d)],
        phase: ((px * SUBPIXEL) as u8, (py * SUBPIXEL) as u8),
    };
    let origin = (bx as i32, by as i32);
    let entry = match CACHE.with(|c| c.borrow().masks.get(&key).cloned()) {
        Some(e) => e,
        None => {
            let local = Transform2D { e: px, f: py, ..*t };
            let entry = match outline() {
                None => Entry::Empty,
                Some(node) if !solid(std::slice::from_ref(&node)) => Entry::Outline,
                Some(node) => rasterize(&node, &local),
            };
            CACHE.with(|c| {
                let mut c = c.borrow_mut();
                let bytes = match &entry {
                    Entry::Mask(m) => m.coverage.len() + 64,
                    _ => 64,
                };
                if c.bytes + bytes > MAX_BYTES {
                    c.masks.clear();
                    c.bytes = 0;
                }
                c.bytes += bytes;
                c.masks.insert(key, entry.clone());
            });
            entry
        }
    };
    match entry {
        Entry::Empty => Some(Placed { origin, mask: None }),
        Entry::Mask(m) => Some(Placed {
            origin,
            mask: Some(m),
        }),
        Entry::Outline => None,
    }
}

/// `node` (painted white) drawn through `t` into a mask cropped to what it
/// covers.
fn rasterize(node: &Node, t: &Transform2D) -> Entry {
    let Some(b) = crate::render::lower::subtree_bounds(std::slice::from_ref(node)) else {
        return Entry::Empty;
    };
    let d = crate::render::lower::map_bounds(t, b);
    let (x0, y0) = (d[0].floor() as i32 - 1, d[1].floor() as i32 - 1);
    let (x1, y1) = (d[2].ceil() as i32 + 1, d[3].ceil() as i32 + 1);
    if x1 <= x0 || y1 <= y0 {
        return Entry::Empty;
    }
    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    if w > MAX_SIDE || h > MAX_SIDE {
        return Entry::Outline;
    }
    let shift = Transform2D::translate(-x0 as f32, -y0 as f32).compose(t);
    let frame = Renderer::with_cache_capacity(w, h, 1).render_node(node, shift);
    let plane = &frame.planes[0];
    let alpha = |x: u32, y: u32| plane.data[y as usize * plane.stride + x as usize * 4 + 3];
    // The rows and columns it covers.
    let (mut c0, mut r0, mut c1, mut r1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if alpha(x, y) != 0 {
                c0 = c0.min(x);
                c1 = c1.max(x + 1);
                r0 = r0.min(y);
                r1 = r1.max(y + 1);
            }
        }
    }
    if c1 <= c0 {
        return Entry::Empty;
    }
    let mut coverage = Vec::with_capacity(((c1 - c0) * (r1 - r0)) as usize);
    for y in r0..r1 {
        coverage.extend((c0..c1).map(|x| alpha(x, y)));
    }
    Entry::Mask(Rc::new(Mask {
        x: x0 + c0 as i32,
        y: y0 + r0 as i32,
        w: c1 - c0,
        h: r1 - r0,
        coverage,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::{Path, PathNode, Point};

    fn square() -> Node {
        let mut p = Path::new();
        p.move_to(Point { x: 0.0, y: 0.0 });
        p.line_to(Point { x: 4.0, y: 0.0 });
        p.line_to(Point { x: 4.0, y: 4.0 });
        p.line_to(Point { x: 0.0, y: 4.0 });
        p.close();
        Node::Path(PathNode::new(p).with_fill(Paint::Solid(MASK_COLOR.to_oxideav())))
    }

    #[test]
    fn a_glyph_is_rasterized_once_per_position_within_a_pixel() {
        let made = std::cell::Cell::new(0);
        let outline = || {
            made.set(made.get() + 1);
            Some(square())
        };
        let a = place(0xfeed, &Transform2D::translate(10.0, 20.0), outline).unwrap();
        let b = place(0xfeed, &Transform2D::translate(30.0, 5.0), outline).unwrap();
        assert_eq!((a.origin, b.origin), ((10, 20), (30, 5)));
        let (ma, mb) = (a.mask.unwrap(), b.mask.unwrap());
        assert!(Rc::ptr_eq(&ma, &mb), "one mask for both");
        // Cropped to the square, wholly covered.
        assert_eq!((ma.x, ma.y, ma.w, ma.h), (0, 0, 4, 4));
        assert!(ma.coverage.iter().all(|&c| c == 255));
        let c = place(0xfeed, &Transform2D::translate(10.5, 20.0), outline).unwrap();
        let mc = c.mask.unwrap();
        assert!(!Rc::ptr_eq(&mc, &ma), "half a pixel on: another");
        assert_eq!((mc.x, mc.w), (0, 5));
        assert_eq!(made.get(), 2);
    }

    #[test]
    fn a_glyph_that_is_not_solid_paint_is_drawn_as_an_outline() {
        let mut p = Path::new();
        p.move_to(Point { x: 0.0, y: 0.0 });
        p.line_to(Point { x: 4.0, y: 4.0 });
        p.line_to(Point { x: 0.0, y: 4.0 });
        p.close();
        let image = Node::Group(oxideav_core::Group::new().with_child(Node::Path(
            PathNode::new(p).with_fill(Paint::Solid(MASK_COLOR.to_oxideav())),
        )));
        let mut faded = image.clone();
        if let Node::Group(g) = &mut faded {
            g.opacity = 0.5;
        }
        assert!(place(0xbeef, &Transform2D::identity(), || Some(faded)).is_none());
        assert!(place(0xbee0, &Transform2D::identity(), || Some(image)).is_some());
        let empty = place(0xcafe, &Transform2D::identity(), || None).unwrap();
        assert!(empty.mask.is_none());
    }
}
