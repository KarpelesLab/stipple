//! The web executor: the lowered frame crosses into the page as a compact
//! binary op buffer, which `web/gpu.js` draws with WGSL shaders on WebGPU or
//! GLSL ES 3.0 on WebGL2, straight into the app's canvas.
//!
//! Everything that is geometry is done here, once, in Rust: each draw comes
//! with its vertices, already in device pixels (path fills as fan
//! triangles; boxes and image quads batched, `super::batch`, each batch one
//! draw), so the page only uploads them and replays the draws. Images are
//! textures the page keeps by id: an image is uploaded the first time it is
//! drawn (keyed by its `Arc<Pixmap>`'s identity, as the other executors key
//! theirs) and freed once it is dropped or hasn't been drawn for a while.
//! Each glyph atlas page is a texture too, updated with only what changed
//! in it (`gpu_texture_sub`).
//!
//! The buffer is a run of 32-bit words (`f32` bits where a value is a float):
//!
//! ```text
//! header   width height bg_r bg_g bg_b bg_a flags    (background premultiplied)
//! 2 path   even_odd color[4] bbox[4](i32) n verts[n*4]        (fan triangles)
//! 4 clip   rect[4](i32)                                       (a scissor)
//! 5 clip   n verts[n*4]                         (a convex polygon, as triangles)
//! 6 pop clip
//! 7 layer  opacity
//! 8 pop layer
//! 9 quads  id nearest n verts[n*20]     (a batch; id 0: boxes only, no texture)
//! ```
//!
//! A path or clip vertex is `x y aux_x aux_y`, a quad vertex the five
//! `vec4`s `super::batch` describes (see the shaders in `web/gpu.js`).
//!
//! `flags` bit 0 says the frame is partial ([`Plan::limit`]): it redraws only
//! what changed (its ops are clipped to that) over the last frame, which the
//! page keeps (WebGL2: a drawing buffer that is preserved; WebGPU: a texture
//! of its own, copied into the canvas's each frame). A page that can't keep
//! it (it has no last frame of that size) answers 4, and the frame is drawn
//! whole next time.

use super::atlas::{AtlasPage, Upload};
use super::batch::{Cmd, QUAD_FLOATS, TexRef};
use super::canvas::rekey;
use super::ops::{Clip, Op, Plan};
use super::renderer::{Api, Executor, GpuError, KeptFrame};
use crate::geometry::ScaleFactor;
use crate::platform::GpuLayer;
use crate::render::Pixmap;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

#[cfg(target_arch = "wasm32")]
mod js {
    #[link(wasm_import_module = "stipple")]
    unsafe extern "C" {
        /// Set up the GPU (`want`: 0 WebGPU then WebGL2, 1 WebGPU, 2 WebGL2).
        /// 0: still setting up (the page asks for a redraw once it's done),
        /// 1: WebGPU, 2: WebGL2, 3: none to be had.
        pub fn gpu_init(want: u32) -> u32;
        /// Upload a `w`×`h` straight-alpha RGBA texture as `id`; 0 if the
        /// device can't take it (too big).
        pub fn gpu_texture(id: u32, ptr: *const u8, w: u32, h: u32) -> u32;
        /// Replace the `w`×`h` pixels at `(x, y)` of texture `id` with the
        /// straight-alpha RGBA at `ptr`; 0 if it can't.
        pub fn gpu_texture_sub(id: u32, ptr: *const u8, x: u32, y: u32, w: u32, h: u32) -> u32;
        /// Free texture `id` (0: all of them).
        pub fn gpu_free(id: u32);
        /// Draw a frame: 0 shown, 1 the device was lost, 2 not now (the page
        /// asks for a redraw), 3 the GPU can't draw it, 4 it was partial and
        /// the page holds no last frame to draw it over.
        pub fn gpu_frame(ptr: *const u32, words: usize) -> u32;
        /// The app draws on the CPU again: the page gives the canvas back to
        /// the 2D context on the next CPU frame.
        pub fn gpu_release();
    }
}

/// Stand-ins off the web, for the tests: every texture uploads, there's
/// no GPU to draw with.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::missing_safety_doc)]
mod js {
    pub unsafe fn gpu_texture(_: u32, _: *const u8, _: u32, _: u32) -> u32 {
        1
    }
    pub unsafe fn gpu_texture_sub(_: u32, _: *const u8, _: u32, _: u32, _: u32, _: u32) -> u32 {
        1
    }
    pub unsafe fn gpu_free(_: u32) {}
    pub unsafe fn gpu_init(_: u32) -> u32 {
        3
    }
    pub unsafe fn gpu_frame(_: *const u32, _: usize) -> u32 {
        3
    }
    pub unsafe fn gpu_release() {}
}

/// Free a texture not drawn for this many frames.
const TEXTURE_KEEP_FRAMES: u64 = 120;

const OP_PATH: u32 = 2;
const OP_CLIP_RECT: u32 = 4;
const OP_CLIP_POLY: u32 = 5;
const OP_POP_CLIP: u32 = 6;
const OP_LAYER: u32 = 7;
const OP_POP_LAYER: u32 = 8;
const OP_QUADS: u32 = 9;

/// Header flag: a partial frame, drawn over the last one.
const FLAG_PARTIAL: u32 = 1;

struct Texture {
    image: Weak<Pixmap>,
    id: u32,
    used: u64,
    bytes: usize,
}

/// An atlas page's texture in the page, holding `version` of it.
struct AtlasTex {
    id: u32,
    side: u32,
    version: u64,
}

/// The web executor: the page's GPU, the textures it holds, and the op
/// buffer, kept from frame to frame.
pub(crate) struct WebExecutor {
    api: Api,
    textures: HashMap<usize, Texture>,
    /// The atlas pages' textures, by page id.
    atlas: HashMap<u64, AtlasTex>,
    next_id: u32,
    count: u64,
    uploads: usize,
    buf: Vec<u32>,
    /// Whether the page holds the last frame (the canvas is the one window).
    kept: KeptFrame,
}

impl WebExecutor {
    /// The page's GPU: `api` for one in particular, `None` for WebGPU and
    /// failing that WebGL2. [`GpuError::Pending`] while the page is still
    /// setting it up.
    pub(crate) fn new(api: Option<Api>) -> Result<Self, GpuError> {
        let want = match api {
            None => 0,
            Some(Api::WebGpu) => 1,
            Some(Api::WebGl2) => 2,
            Some(other) => {
                return Err(GpuError::Unavailable(format!(
                    "{} isn't available in a browser",
                    other.name()
                )));
            }
        };
        let api = match unsafe { js::gpu_init(want) } {
            0 => return Err(GpuError::Pending),
            1 => Api::WebGpu,
            2 => Api::WebGl2,
            _ => {
                return Err(GpuError::Unavailable(
                    "the browser has neither WebGPU nor WebGL2 for this canvas".into(),
                ));
            }
        };
        Ok(WebExecutor {
            api,
            textures: HashMap::new(),
            atlas: HashMap::new(),
            next_id: 1,
            count: 0,
            uploads: 0,
            buf: Vec::new(),
            kept: KeptFrame::default(),
        })
    }

    /// The page's id for `image`'s texture, uploading it unless it holds it.
    fn texture(&mut self, image: &Arc<Pixmap>) -> Option<u32> {
        let key = Arc::as_ptr(image) as usize;
        if let Some(t) = self.textures.get_mut(&key) {
            if t.image.upgrade().is_some_and(|i| Arc::ptr_eq(&i, image)) {
                t.used = self.count;
                return Some(t.id);
            }
            // The address was reused by another image.
            unsafe { js::gpu_free(t.id) };
            self.textures.remove(&key);
        }
        let size = image.size();
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let id = self.new_id();
        let ok =
            unsafe { js::gpu_texture(id, image.as_bytes().as_ptr(), size.width, size.height) != 0 };
        if !ok {
            return None;
        }
        self.uploads += 1;
        self.textures.insert(
            key,
            Texture {
                image: Arc::downgrade(image),
                id,
                used: self.count,
                bytes: image.as_bytes().len(),
            },
        );
        Some(id)
    }

    fn new_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        id
    }

    /// Bring the page's atlas textures up to date with `pages`, and free
    /// those of pages gone.
    fn sync_atlas(&mut self, pages: &[AtlasPage]) {
        self.atlas.retain(|id, t| {
            let keep = pages.iter().any(|p| p.id == *id);
            if !keep {
                unsafe { js::gpu_free(t.id) };
            }
            keep
        });
        for p in pages {
            let have = self.atlas.get(&p.id).map(|t| (t.side, t.version));
            match p.upload(have) {
                Upload::Nothing => {}
                Upload::Whole => {
                    if let Some(old) = self.atlas.remove(&p.id) {
                        unsafe { js::gpu_free(old.id) };
                    }
                    let id = self.new_id();
                    let ok = unsafe { js::gpu_texture(id, p.pixels.as_ptr(), p.side, p.side) != 0 };
                    if ok {
                        self.uploads += 1;
                        let (side, version) = (p.side, p.version);
                        self.atlas.insert(p.id, AtlasTex { id, side, version });
                    }
                }
                Upload::Region(r) => {
                    let t = self.atlas.get_mut(&p.id).unwrap();
                    let bytes = p.region(r);
                    let (w, h) = (r[2] - r[0], r[3] - r[1]);
                    let ok =
                        unsafe { js::gpu_texture_sub(t.id, bytes.as_ptr(), r[0], r[1], w, h) != 0 };
                    if ok {
                        t.version = p.version;
                        self.uploads += 1;
                    }
                }
            }
        }
    }

    /// Update the textures `plan` changed in places (canvases kept as
    /// textures), a rectangle at a time.
    fn update_textures(&mut self, plan: &Plan) {
        for u in &plan.updates {
            let Some(t) = rekey(&mut self.textures, u, |t| &mut t.image) else {
                continue;
            };
            let id = t.id;
            for &r in &u.rects {
                let rows = u.rows(r);
                unsafe { js::gpu_texture_sub(id, rows.as_ptr(), r[0], r[1], r[2], r[3]) };
            }
        }
    }

    /// Free the textures whose image is gone or wasn't drawn lately.
    fn sweep(&mut self) {
        let count = self.count;
        self.textures.retain(|_, t| {
            let keep =
                t.image.strong_count() > 0 && count.saturating_sub(t.used) < TEXTURE_KEEP_FRAMES;
            if !keep {
                unsafe { js::gpu_free(t.id) };
            }
            keep
        });
        self.count += 1;
    }

    /// Encode `plan` into the op buffer, uploading what it draws.
    fn encode(&mut self, plan: &Plan) {
        let mut buf = std::mem::take(&mut self.buf);
        buf.clear();
        let bg = plan.background;
        let a = bg.a as f32 / 255.0;
        buf.extend([plan.size.width, plan.size.height]);
        push_f(
            &mut buf,
            &[
                bg.r as f32 / 255.0 * a,
                bg.g as f32 / 255.0 * a,
                bg.b as f32 / 255.0 * a,
                a,
            ],
        );
        buf.push(if plan.partial.is_some() {
            FLAG_PARTIAL
        } else {
            0
        });
        self.uploads = 0;
        self.sync_atlas(&plan.atlas);
        for cmd in &plan.cmds {
            let op = match cmd {
                Cmd::Quads(q) => {
                    let id = match &q.texture {
                        None => 0,
                        Some(TexRef::Image(image)) => self.texture(image).unwrap_or(0),
                        Some(TexRef::Atlas(i)) => plan
                            .atlas
                            .get(*i)
                            .and_then(|p| self.atlas.get(&p.id))
                            .map_or(0, |t| t.id),
                        // No platform buffer to import in a browser: what
                        // pixels it was given.
                        Some(TexRef::External(e)) => {
                            e.pixmap().and_then(|p| self.texture(&p)).unwrap_or(0)
                        }
                    };
                    buf.push(OP_QUADS);
                    buf.push(id);
                    buf.push(u32::from(q.nearest));
                    buf.push(q.count);
                    let n = QUAD_FLOATS;
                    let verts = &plan.quads[q.first as usize * n..(q.first + q.count) as usize * n];
                    push_f(&mut buf, verts);
                    continue;
                }
                Cmd::Op(i) => &plan.ops[*i],
            };
            match op {
                // Batched into `Cmd::Quads`.
                Op::Box { .. } | Op::Image { .. } => {}
                Op::Path {
                    contours,
                    even_odd,
                    color,
                    bbox,
                } => {
                    buf.push(OP_PATH);
                    buf.push(u32::from(*even_odd));
                    push_f(&mut buf, color);
                    buf.extend(bbox.iter().map(|&v| v as u32));
                    push_fans(&mut buf, contours.iter().map(|c| c.as_slice()));
                }
                Op::PushClip(Clip::Rect(r)) => {
                    buf.push(OP_CLIP_RECT);
                    buf.extend(r.iter().map(|&v| v as u32));
                }
                Op::PushClip(Clip::Polygon(pts)) => {
                    buf.push(OP_CLIP_POLY);
                    push_fans(&mut buf, std::iter::once(pts.as_slice()));
                }
                Op::PopClip => buf.push(OP_POP_CLIP),
                Op::PushLayer { opacity } => {
                    buf.push(OP_LAYER);
                    push_f(&mut buf, &[*opacity]);
                }
                // Never lowered for the web (`Walker::offscreen` draws the
                // software renderer's bitmap there): kept balanced anyway.
                Op::PushResampled(_) => {
                    buf.push(OP_LAYER);
                    push_f(&mut buf, &[1.0]);
                }
                Op::PopLayer => buf.push(OP_POP_LAYER),
            }
        }
        self.buf = buf;
    }
}

impl Drop for WebExecutor {
    fn drop(&mut self) {
        unsafe { js::gpu_free(0) };
    }
}

fn push_f(buf: &mut Vec<u32>, values: &[f32]) {
    buf.extend(values.iter().map(|v| v.to_bits()));
}

/// Each polygon as a fan of triangles from its first point, with the vertex
/// count first.
fn push_fans<'a>(buf: &mut Vec<u32>, polys: impl Iterator<Item = &'a [[f32; 2]]> + Clone) {
    let n: usize = polys.clone().map(|p| p.len().saturating_sub(2) * 3).sum();
    buf.push(n as u32);
    for p in polys {
        for i in 1..p.len().saturating_sub(1) {
            for q in [p[0], p[i], p[i + 1]] {
                push_f(buf, &[q[0], q[1], 0.0, 0.0]);
            }
        }
    }
}

impl Executor for WebExecutor {
    fn api(&self) -> Api {
        self.api
    }

    fn read_back(&mut self, _plan: &Plan) -> Result<Pixmap, GpuError> {
        Err(GpuError::Unavailable(
            "in a browser the GPU draws into the canvas only".into(),
        ))
    }

    fn can_present(&self, layer: &GpuLayer) -> bool {
        matches!(layer, GpuLayer::Web)
    }

    fn present(
        &mut self,
        plan: &mut Plan,
        _layer: &GpuLayer,
        _scale: ScaleFactor,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), GpuError> {
        self.update_textures(plan);
        // Only what changed, over the last frame the page keeps.
        self.kept.limit(0, plan, dirty);
        self.encode(plan);
        let status = unsafe { js::gpu_frame(self.buf.as_ptr(), self.buf.len()) };
        self.sweep();
        if status == 0 {
            self.kept
                .presented(0, (plan.size.width, plan.size.height), dirty);
        } else {
            self.kept.forget();
        }
        match status {
            0 => Ok(()),
            1 => Err(GpuError::Lost(format!("the {} device", self.api.name()))),
            2 => Err(GpuError::Pending),
            4 => Err(GpuError::Skipped),
            _ => Err(GpuError::Unavailable(format!(
                "{} couldn't draw the frame",
                self.api.name()
            ))),
        }
    }

    fn uploads(&self) -> usize {
        self.uploads
    }

    fn texture_bytes(&self) -> usize {
        self.textures.values().map(|t| t.bytes).sum::<usize>()
            + self
                .atlas
                .values()
                .map(|a| a.side as usize * a.side as usize * 4)
                .sum::<usize>()
    }
}

/// The app draws on the CPU again (see [`js::gpu_release`]).
pub(crate) fn release() {
    unsafe { js::gpu_release() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fans_cover_each_polygon() {
        let mut buf = Vec::new();
        let square = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        let line = [[0.0, 0.0], [1.0, 1.0]];
        push_fans(&mut buf, [&square[..], &line[..]].into_iter());
        assert_eq!(buf[0], 6, "two triangles, the line adds none");
        assert_eq!(buf.len(), 1 + 6 * 4);
        let f = |i: usize| f32::from_bits(buf[1 + i]);
        assert_eq!(
            (f(4), f(5)),
            (4.0, 0.0),
            "the first triangle's second point"
        );
    }

    fn executor() -> WebExecutor {
        WebExecutor {
            api: Api::WebGpu,
            textures: HashMap::new(),
            atlas: HashMap::new(),
            next_id: 1,
            count: 0,
            uploads: 0,
            buf: Vec::new(),
            kept: KeptFrame::default(),
        }
    }

    /// Walk an op buffer as `web/gpu.js` does, returning the ops in it.
    fn walk(buf: &[u32]) -> Vec<u32> {
        let mut ops = Vec::new();
        let mut p = 7;
        while p < buf.len() {
            let op = buf[p];
            ops.push(op);
            p += 1;
            // Fixed words, then (for some) a vertex count and its vertices
            // of so many words.
            let (fixed, verts) = match op {
                OP_PATH => (9, 4),
                OP_CLIP_RECT => (4, 0),
                OP_CLIP_POLY => (0, 4),
                OP_POP_CLIP | OP_POP_LAYER => (0, 0),
                OP_LAYER => (1, 0),
                OP_QUADS => (2, QUAD_FLOATS),
                _ => panic!("unknown op {op} at {}", p - 1),
            };
            p += fixed;
            if verts > 0 {
                p += 1 + buf[p] as usize * verts;
            }
        }
        assert_eq!(p, buf.len(), "the last op ends the buffer");
        ops
    }

    #[test]
    fn the_op_buffer_walks_as_the_page_reads_it() {
        let mut ex = executor();
        let mut rasters = super::super::ops::RasterCache::default();
        let bg = crate::render::Color::rgb(1, 2, 3);
        for scene in [
            super::super::corpus::boxes(),
            super::super::corpus::paths(),
            super::super::corpus::layers(),
            super::super::corpus::clipped(),
            super::super::corpus::regions(),
        ] {
            let size = crate::geometry::PhysicalSize::new(320, 240);
            let plan = super::super::ops::plan(&scene, bg, size, &mut rasters);
            ex.encode(&plan);
            assert_eq!(&ex.buf[..2], &[320, 240]);
            let ops = walk(&ex.buf);
            assert_eq!(ops.len(), plan.cmds.len(), "one op per command");
            let count = |op: u32| ops.iter().filter(|&&o| o == op).count();
            assert_eq!(count(OP_LAYER), count(OP_POP_LAYER));
            assert_eq!(
                count(OP_CLIP_RECT) + count(OP_CLIP_POLY),
                count(OP_POP_CLIP)
            );
        }
        // A partial frame says so in its header, and is clipped to its region.
        let mut plan = super::super::ops::plan(
            &super::super::corpus::boxes(),
            bg,
            crate::geometry::PhysicalSize::new(320, 240),
            &mut rasters,
        );
        ex.encode(&plan);
        assert_eq!(ex.buf[6], 0, "a whole frame");
        plan.limit([10, 10, 50, 40]);
        ex.encode(&plan);
        assert_eq!(ex.buf[6], FLAG_PARTIAL);
        let ops = walk(&ex.buf);
        assert_eq!(ops[0], OP_CLIP_RECT);
        assert_eq!(&ex.buf[8..12], &[10, 10, 50, 40]);
        assert_eq!(*ops.last().unwrap(), OP_POP_CLIP);
        // An image is uploaded once while it lives, and freed after.
        let scene = super::super::corpus::images();
        let plan = super::super::ops::plan(
            &scene,
            bg,
            crate::geometry::PhysicalSize::new(160, 120),
            &mut rasters,
        );
        ex.encode(&plan);
        assert_eq!(ex.uploads(), 1);
        ex.sweep();
        ex.encode(&plan);
        assert_eq!(ex.uploads(), 0, "the same image, the same texture");
        // A batch of quads: its texture's id, then its vertices.
        let at = 7;
        assert_eq!(ex.buf[at], OP_QUADS, "{:?}", &ex.buf[..12]);
        assert!(ex.buf[at + 1] > 0);
        drop(plan);
        drop(scene);
        ex.sweep();
        assert!(
            ex.textures.is_empty(),
            "the image is gone: so is its texture"
        );
    }
}
