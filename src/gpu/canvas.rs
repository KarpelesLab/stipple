//! Canvases ([`Element::canvas`](crate::core::Element::canvas)) kept as
//! textures from frame to frame on the GPU renderers, re-rasterized and
//! re-uploaded only where they changed.
//!
//! A canvas drawn upright and wholly on the target is rasterized on the CPU
//! once, into a bitmap of exactly the pixels it covers, and drawn as that
//! texture. On later frames of the same window only the frame's damage over
//! it is rasterized again (what the software renderer would repaint: the
//! areas invalidated with `invalidate_canvas_rect`, whatever moved over it),
//! and only those rectangles are uploaded into the texture it already has
//! ([`TextureUpdate`]). A terminal that changes a row a tick pays for a row.
//!
//! A canvas that changes nearly all over most frames (a game drawn into it,
//! one that moves) is drawn straight on the GPU instead, its paint callback
//! walked into operations like the rest of the scene, until it settles again.
//! So is one drawn rotated or skewed, one partly off the target, and one whose
//! id two canvases of the same frame share.

use super::ops::{FULL_UV, NO_TINT, Op, Source};
use crate::core::CanvasId;
use crate::geometry::{Affine, PhysicalSize, Rect, Size};
use crate::render::{CanvasPaint, Pixmap, Scene, SoftwareRenderer};
use oxideav_core::Transform2D;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

/// After this many frames in a row repainted over half of a canvas, it is
/// drawn straight on the GPU.
const BUSY_FRAMES: u32 = 3;
/// After this many frames in a row repainting less than half of it, a canvas
/// drawn straight is kept as a texture again.
const CALM_FRAMES: u32 = 10;
/// A window whose frames stopped coming this many frames ago (of any window)
/// has its canvases dropped.
const OWNER_KEEP_FRAMES: u64 = 600;

/// A cached image changed in places: the executor keeps the texture it made
/// for `old` (an image now dropped), now `image`'s, and uploads `rects`.
#[derive(Debug)]
pub(crate) struct TextureUpdate {
    /// The identity (`Arc::as_ptr`) of the image the texture was made for.
    pub old: usize,
    /// The image the texture now shows.
    pub image: Arc<Pixmap>,
    /// The texel rectangles that changed, `[x, y, w, h]`.
    pub rects: Vec<[u32; 4]>,
}

impl TextureUpdate {
    /// `rect`'s pixels, row after row (for APIs that can't skip along a
    /// row, such as GLES2).
    #[cfg_attr(
        not(any(
            all(any(target_os = "linux", target_os = "android"), feature = "gl"),
            all(
                any(target_os = "linux", target_os = "android", target_os = "macos"),
                feature = "vk"
            ),
            all(any(target_os = "macos", target_os = "ios"), feature = "mtl"),
            all(target_arch = "wasm32", feature = "gpu"),
            test
        )),
        allow(dead_code)
    )]
    pub(crate) fn rows(&self, [x, y, w, h]: [u32; 4]) -> Vec<u8> {
        let stride = self.image.stride();
        let bytes = self.image.as_bytes();
        let row = w as usize * 4;
        let mut out = Vec::with_capacity(row * h as usize);
        for r in y as usize..(y + h) as usize {
            let at = r * stride + x as usize * 4;
            out.extend_from_slice(&bytes[at..at + row]);
        }
        out
    }

    /// The byte offset of `rect`'s first pixel in the image.
    #[cfg_attr(not(all(target_os = "windows", feature = "d3d")), allow(dead_code))]
    pub(crate) fn offset(&self, [x, y, _, _]: [u32; 4]) -> usize {
        y as usize * self.image.stride() + x as usize * 4
    }
}

/// For an executor's texture cache (keyed by `Arc::as_ptr` of the image,
/// holding a [`Weak`] to it): move the texture made for `u.old` under
/// `u.image`'s key and return it, to upload `u.rects` into. `None` when the
/// cache has no such texture (it was swept, or never made): `u.image` is
/// then uploaded whole when drawn, like any new image.
#[cfg_attr(
    not(any(
        all(any(target_os = "linux", target_os = "android"), feature = "gl"),
        all(
            any(target_os = "linux", target_os = "android", target_os = "macos"),
            feature = "vk"
        ),
        all(any(target_os = "macos", target_os = "ios"), feature = "mtl"),
        all(target_os = "windows", feature = "d3d"),
        all(target_arch = "wasm32", feature = "gpu"),
        test
    )),
    allow(dead_code)
)]
pub(crate) fn rekey<'m, T>(
    textures: &'m mut HashMap<usize, T>,
    u: &TextureUpdate,
    weak: fn(&mut T) -> &mut Weak<Pixmap>,
) -> Option<&'m mut T> {
    let new = Arc::as_ptr(&u.image) as usize;
    // The old image is gone (its texture is ours to take), and nothing is
    // cached under the new one's address (a live `Weak` keeps an address
    // from being reused, so there can't be).
    if textures.contains_key(&new) || weak(textures.get_mut(&u.old)?).strong_count() != 0 {
        return None;
    }
    let mut t = textures.remove(&u.old)?;
    *weak(&mut t) = Arc::downgrade(&u.image);
    Some(textures.entry(new).or_insert(t))
}

/// Where a canvas lands on the target, and at what transform: a texture
/// rasterized for one is good for another only if they're the same.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Key {
    /// The texture's size in pixels.
    size: (u32, u32),
    /// The canvas's scale on each axis, and where its corner falls within
    /// the texture's first pixel.
    at: [f32; 4],
    /// The canvas's own size, and the scale its bitmaps are drawn for.
    canvas: (f64, f64, f64),
}

struct Entry {
    /// The canvas's pixels while it's kept as a texture.
    image: Option<Arc<Pixmap>>,
    key: Option<Key>,
    /// Frames in a row that repainted most of it.
    busy: u32,
    /// Drawn straight on the GPU, and frames in a row since that repainted
    /// little of it.
    direct: bool,
    calm: u32,
    /// Two canvases of a frame have this id: always drawn straight.
    shared: bool,
    /// Drawn in the frame being lowered.
    seen: bool,
}

impl Entry {
    fn new() -> Self {
        Entry {
            image: None,
            key: None,
            busy: 0,
            direct: false,
            calm: 0,
            shared: false,
            seen: false,
        }
    }
}

/// One window's canvases.
#[derive(Default)]
struct Owner {
    canvases: HashMap<CanvasId, Entry>,
    /// The frame it last drew.
    last: u64,
}

/// The canvases each window keeps as textures. Windows are told apart by an
/// owner number their frames come with.
#[derive(Default)]
pub(crate) struct CanvasCache {
    owners: HashMap<u64, Owner>,
    frame: u64,
    /// Bytes of canvas pixels the last frame rasterized and sent to the GPU.
    uploaded: usize,
}

impl CanvasCache {
    /// Start a frame of `owner`'s, whose damage since its last frame is
    /// `damage` (logical rects; `None`: all of it), drawn through `root`
    /// onto a `target` (`[x0, y0, x1, y1]` pixels).
    pub(crate) fn begin(
        &mut self,
        owner: u64,
        damage: Option<Vec<Rect>>,
        root: &Transform2D,
        target: [f32; 4],
    ) -> CanvasFrame {
        self.frame += 1;
        let frame = self.frame;
        self.owners
            .retain(|_, o| frame.saturating_sub(o.last) < OWNER_KEEP_FRAMES);
        let mut canvases = self.owners.remove(&owner).unwrap_or_default();
        canvases.last = frame;
        let damage = damage.map(|rects| {
            rects
                .iter()
                .filter_map(|r| {
                    let x0 = (r.min_x() as f32 * root.a + root.e).floor().max(target[0]);
                    let y0 = (r.min_y() as f32 * root.d + root.f).floor().max(target[1]);
                    let x1 = (r.max_x() as f32 * root.a + root.e).ceil().min(target[2]);
                    let y1 = (r.max_y() as f32 * root.d + root.f).ceil().min(target[3]);
                    (x1 > x0 && y1 > y0).then_some([x0 as i32, y0 as i32, x1 as i32, y1 as i32])
                })
                .collect()
        });
        CanvasFrame {
            owner,
            canvases,
            damage,
            target,
            updates: Vec::new(),
            uploaded: 0,
        }
    }

    /// End `frame`: forget the canvases it didn't draw, and return the
    /// textures to update.
    pub(crate) fn end(&mut self, frame: CanvasFrame) -> Vec<TextureUpdate> {
        let CanvasFrame {
            owner,
            mut canvases,
            updates,
            uploaded,
            ..
        } = frame;
        canvases.canvases.retain(|_, e| std::mem::take(&mut e.seen));
        self.owners.insert(owner, canvases);
        self.uploaded = uploaded;
        updates
    }

    /// The frame `owner` drew last didn't reach the screen: its textures
    /// may not match the pixels kept here, so start over.
    pub(crate) fn forget(&mut self, owner: u64) {
        self.owners.remove(&owner);
    }

    /// Bytes of canvas pixels the last frame rasterized and sent to the GPU.
    pub(crate) fn uploaded(&self) -> usize {
        self.uploaded
    }

    /// Bytes of canvas pixels kept (each one also held as a texture).
    pub(crate) fn bytes(&self) -> usize {
        self.owners
            .values()
            .flat_map(|o| o.canvases.values())
            .filter_map(|e| e.image.as_ref())
            .map(|i| i.as_bytes().len())
            .sum()
    }
}

/// One frame's canvases, while the frame is lowered.
pub(crate) struct CanvasFrame {
    owner: u64,
    canvases: Owner,
    /// What changed since the window's last frame, in target pixels
    /// (`None`: everything).
    damage: Option<Vec<[i32; 4]>>,
    target: [f32; 4],
    updates: Vec<TextureUpdate>,
    uploaded: usize,
}

impl CanvasFrame {
    /// Canvas `id`, `bounds` in the coordinates `local` maps to the target,
    /// as a texture: the operation drawing it, or `None` to draw it straight.
    pub(crate) fn draw(
        &mut self,
        id: CanvasId,
        bounds: Rect,
        paint: &CanvasPaint,
        scale: f64,
        local: &Transform2D,
    ) -> Option<Op> {
        let entry = self.canvases.canvases.entry(id).or_insert_with(Entry::new);
        if std::mem::replace(&mut entry.seen, true) {
            entry.shared = true;
        }
        if entry.shared {
            entry.image = None;
            return None;
        }
        let shift = Transform2D::translate(bounds.origin.x as f32, bounds.origin.y as f32);
        let Some((x0, y0, key)) = place(&local.compose(&shift), bounds.size, scale, self.target)
        else {
            entry.image = None;
            return None;
        };
        let (tw, th) = key.size;
        let whole = [0, 0, tw, th];
        let fresh = entry.image.is_none() || entry.key != Some(key);
        // What the frame's damage covers of it.
        let damaged: Vec<[u32; 4]> = match &self.damage {
            Some(damage) => damage
                .iter()
                .filter_map(|d| {
                    let x = (d[0] - x0).max(0);
                    let y = (d[1] - y0).max(0);
                    let r = (d[2] - x0).min(tw as i32);
                    let b = (d[3] - y0).min(th as i32);
                    (r > x && b > y).then_some([x as u32, y as u32, (r - x) as u32, (b - y) as u32])
                })
                .collect(),
            None => vec![whole],
        };
        let area: u64 = damaged.iter().map(|r| r[2] as u64 * r[3] as u64).sum();
        let mostly = area * 2 >= tw as u64 * th as u64;
        if entry.direct {
            entry.calm = if mostly { 0 } else { entry.calm + 1 };
            if entry.calm < CALM_FRAMES {
                return None;
            }
            entry.direct = false;
            entry.busy = 0;
        } else {
            // A new texture is all painted, but only damage counts as busy.
            entry.busy = if mostly { entry.busy + 1 } else { 0 };
            if entry.busy >= BUSY_FRAMES {
                entry.direct = true;
                entry.calm = 0;
                entry.image = None;
                return None;
            }
        }
        let rects = if fresh { vec![whole] } else { damaged };
        if !rects.is_empty() {
            // The pixels kept, to change in place (their texture with them),
            // or new ones.
            let (old, mut pixels) = match entry.image.take() {
                Some(img) if !fresh || img.size() == PhysicalSize::new(tw, th) => {
                    let old = Arc::as_ptr(&img) as usize;
                    (
                        Some(old),
                        Arc::try_unwrap(img).unwrap_or_else(|img| (*img).clone()),
                    )
                }
                _ => (None, Pixmap::new(PhysicalSize::new(tw, th))),
            };
            let scene = canvas_scene(&key, bounds.size, paint);
            let renderer = SoftwareRenderer::new();
            for &[x, y, w, h] in &rects {
                let view = Rect::from_xywh(x as f64, y as f64, w as f64, h as f64);
                let part = renderer.render_area(&scene, view, PhysicalSize::new(w, h));
                pixels.blit(&part, x, y);
                self.uploaded += w as usize * h as usize * 4;
            }
            let image = Arc::new(pixels);
            if let Some(old) = old {
                self.updates.push(TextureUpdate {
                    old,
                    image: image.clone(),
                    rects,
                });
            }
            entry.image = Some(image);
            entry.key = Some(key);
        }
        let image = entry.image.clone()?;
        Some(Op::Image {
            image: Source::Image(image),
            transform: Transform2D::translate(x0 as f32, y0 as f32),
            rect: [0.0, 0.0, tw as f32, th as f32],
            opacity: 1.0,
            uv: FULL_UV,
            tint: NO_TINT,
            nearest: true,
        })
    }
}

/// Where a canvas of `size`, drawn through `t` (its corner at the origin),
/// lands on `target`: the top-left pixel of the texture for it, and its key.
/// `None` unless it's upright, unflipped and wholly on the target.
fn place(t: &Transform2D, size: Size, scale: f64, target: [f32; 4]) -> Option<(i32, i32, Key)> {
    let upright = t.b == 0.0 && t.c == 0.0 && t.a > 0.0 && t.d > 0.0;
    if !upright {
        return None;
    }
    let (w, h) = (size.width as f32, size.height as f32);
    let (x0, y0) = (t.e.floor(), t.f.floor());
    let (x1, y1) = ((t.e + t.a * w).ceil(), (t.f + t.d * h).ceil());
    let inside = x0 >= target[0] && y0 >= target[1] && x1 <= target[2] && y1 <= target[3];
    if !inside || x1 <= x0 || y1 <= y0 {
        return None;
    }
    let key = Key {
        size: ((x1 - x0) as u32, (y1 - y0) as u32),
        at: [t.a, t.d, t.e - x0, t.f - y0],
        canvas: (size.width, size.height, scale),
    };
    Some((x0 as i32, y0 as i32, key))
}

/// A scene of the texture's pixels holding just the canvas, placed as `key`
/// says: rasterizing an area of it runs the paint callback for that area.
fn canvas_scene(key: &Key, size: Size, paint: &CanvasPaint) -> Scene {
    let (w, h) = key.size;
    let [a, d, e, f] = key.at;
    let mut scene = Scene::new(Size::new(w as f64, h as f64));
    scene.set_scale(key.canvas.2);
    scene.push_layer(
        Affine::new(a as f64, 0.0, 0.0, d as f64, e as f64, f as f64),
        1.0,
    );
    scene.draw_canvas(Rect::from_xywh(0.0, 0.0, size.width, size.height), paint);
    scene.pop_layer();
    scene
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{Canvas, Color};
    use std::cell::Cell;
    use std::rc::Rc;

    const TARGET: [f32; 4] = [0.0, 0.0, 200.0, 100.0];

    /// A canvas that fills a rect per call, counting the pixels of the areas
    /// it was asked to paint.
    fn counting() -> (CanvasPaint, Rc<Cell<f64>>) {
        let painted = Rc::new(Cell::new(0.0));
        let p = painted.clone();
        let paint: CanvasPaint = Rc::new(move |c: &mut Canvas| {
            let d = c.dirty();
            p.set(p.get() + d.width() * d.height());
            c.fill_rect(
                Rect::from_xywh(0.0, 0.0, 100.0, 50.0),
                Color::rgb(200, 30, 30),
            );
        });
        (paint, painted)
    }

    fn frame(cache: &mut CanvasCache, damage: Option<Vec<Rect>>) -> CanvasFrame {
        cache.begin(1, damage, &Transform2D::identity(), TARGET)
    }

    fn bounds() -> Rect {
        Rect::from_xywh(10.0, 20.0, 100.0, 50.0)
    }

    #[test]
    fn a_canvas_is_rasterized_once_then_only_where_damaged() {
        let mut cache = CanvasCache::default();
        let (paint, painted) = counting();
        let t = Transform2D::identity();

        let mut f = frame(&mut cache, None);
        let op = f
            .draw(CanvasId(1), bounds(), &paint, 1.0, &t)
            .expect("a texture");
        let Op::Image {
            image: Source::Image(image),
            transform,
            ..
        } = &op
        else {
            panic!("{op:?}");
        };
        assert_eq!(image.size(), PhysicalSize::new(100, 50));
        assert_eq!((transform.e, transform.f), (10.0, 20.0));
        assert!(cache.end(f).is_empty(), "a new texture is uploaded whole");
        assert_eq!(painted.get(), 5000.0);
        let first = Arc::as_ptr(image) as usize;

        // Nothing changed: nothing painted, the same image.
        painted.set(0.0);
        let mut f = frame(&mut cache, Some(Vec::new()));
        let op = f.draw(CanvasId(1), bounds(), &paint, 1.0, &t).unwrap();
        assert!(
            matches!(&op, Op::Image { image: Source::Image(image), .. } if Arc::as_ptr(image) as usize == first)
        );
        assert!(cache.end(f).is_empty());
        assert_eq!(painted.get(), 0.0);
        assert_eq!(cache.uploaded(), 0);

        // One row damaged (partly off the canvas): just that row, in the
        // canvas's texels, as an update of the texture it had.
        let mut f = frame(
            &mut cache,
            Some(vec![Rect::from_xywh(0.0, 30.0, 300.0, 4.0)]),
        );
        let op = f.draw(CanvasId(1), bounds(), &paint, 1.0, &t).unwrap();
        let updates = cache.end(f);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].old, first);
        assert_eq!(updates[0].rects, vec![[0, 10, 100, 4]]);
        assert!(
            matches!(&op, Op::Image { image: Source::Image(image), .. } if Arc::ptr_eq(image, &updates[0].image))
        );
        assert_eq!(painted.get(), 400.0);
        assert_eq!(cache.uploaded(), 1600);
        let px = updates[0].rows([0, 0, 1, 1]);
        assert_eq!(px, vec![200, 30, 30, 255]);
    }

    #[test]
    fn an_update_moves_the_texture_to_the_new_image() {
        let mut cache = CanvasCache::default();
        let (paint, _) = counting();
        let t = Transform2D::identity();
        let mut textures: HashMap<usize, (Weak<Pixmap>, u32)> = HashMap::new();
        let mut f = frame(&mut cache, None);
        let Some(Op::Image {
            image: Source::Image(image),
            ..
        }) = f.draw(CanvasId(1), bounds(), &paint, 1.0, &t)
        else {
            panic!();
        };
        cache.end(f);
        textures.insert(Arc::as_ptr(&image) as usize, (Arc::downgrade(&image), 7));
        drop(image);
        let mut f = frame(
            &mut cache,
            Some(vec![Rect::from_xywh(10.0, 20.0, 5.0, 5.0)]),
        );
        f.draw(CanvasId(1), bounds(), &paint, 1.0, &t).unwrap();
        let updates = cache.end(f);
        let moved = rekey(&mut textures, &updates[0], |t| &mut t.0).expect("moved");
        assert_eq!(moved.1, 7);
        let key = Arc::as_ptr(&updates[0].image) as usize;
        assert!(textures[&key].0.upgrade().is_some());
        assert_eq!(textures.len(), 1);
        // An image still alive isn't taken.
        let other = Arc::new(Pixmap::new(PhysicalSize::new(1, 1)));
        textures.insert(Arc::as_ptr(&other) as usize, (Arc::downgrade(&other), 8));
        let u = TextureUpdate {
            old: Arc::as_ptr(&other) as usize,
            image: Arc::new(Pixmap::new(PhysicalSize::new(1, 1))),
            rects: vec![[0, 0, 1, 1]],
        };
        assert!(rekey(&mut textures, &u, |t| &mut t.0).is_none());
    }

    #[test]
    fn a_canvas_repainted_whole_every_frame_is_drawn_straight_until_it_settles() {
        let mut cache = CanvasCache::default();
        let (paint, _) = counting();
        let t = Transform2D::identity();
        let mut draw = |damage: Option<Vec<Rect>>| {
            let mut f = frame(&mut cache, damage);
            let op = f.draw(CanvasId(1), bounds(), &paint, 1.0, &t);
            cache.end(f);
            op.is_some()
        };
        assert!(draw(None));
        assert!(draw(None));
        assert!(
            !draw(None),
            "the third whole repaint in a row goes straight"
        );
        assert!(!draw(None));
        for _ in 0..CALM_FRAMES - 1 {
            assert!(!draw(Some(Vec::new())));
        }
        assert!(draw(Some(Vec::new())), "settled: a texture again");
    }

    #[test]
    fn canvases_that_cannot_be_textures_are_drawn_straight() {
        let mut cache = CanvasCache::default();
        let (paint, _) = counting();
        let mut f = frame(&mut cache, None);
        // Rotated.
        let r = Transform2D {
            a: 0.0,
            b: 1.0,
            c: -1.0,
            d: 0.0,
            e: 100.0,
            f: 0.0,
        };
        assert!(f.draw(CanvasId(1), bounds(), &paint, 1.0, &r).is_none());
        // Partly off the target.
        let off = Transform2D::translate(150.0, 0.0);
        assert!(f.draw(CanvasId(2), bounds(), &paint, 1.0, &off).is_none());
        // Two canvases with one id.
        let t = Transform2D::identity();
        assert!(f.draw(CanvasId(3), bounds(), &paint, 1.0, &t).is_some());
        assert!(f.draw(CanvasId(3), bounds(), &paint, 1.0, &t).is_none());
        cache.end(f);
        let mut f = frame(&mut cache, None);
        assert!(f.draw(CanvasId(3), bounds(), &paint, 1.0, &t).is_none());
        cache.end(f);
    }

    #[test]
    fn moving_by_a_fraction_or_resizing_rasterizes_anew_and_gone_canvases_are_dropped() {
        let mut cache = CanvasCache::default();
        let (paint, painted) = counting();
        let t = Transform2D::identity();
        let mut f = frame(&mut cache, None);
        f.draw(CanvasId(1), bounds(), &paint, 1.0, &t).unwrap();
        cache.end(f);
        painted.set(0.0);
        let mut f = frame(&mut cache, Some(Vec::new()));
        f.draw(
            CanvasId(1),
            bounds(),
            &paint,
            1.0,
            &Transform2D::translate(0.5, 0.0),
        )
        .unwrap();
        assert!(cache.end(f).is_empty(), "a new size: a new texture");
        assert_eq!(painted.get(), 5000.0, "all of it, painted anew");
        assert!(cache.bytes() > 0);
        let f = frame(&mut cache, Some(Vec::new()));
        cache.end(f);
        assert_eq!(cache.bytes(), 0, "not drawn: dropped");
    }
}
