//! The web executor: the lowered frame crosses into the page as a compact
//! binary op buffer, which `web/gpu.js` draws with WGSL shaders on WebGPU or
//! GLSL ES 3.0 on WebGL2, straight into the app's canvas.
//!
//! Everything that is geometry is done here, once, in Rust: each draw comes
//! with its vertices, already in device pixels (path fills as fan
//! triangles, image quads through their transform, boxes padded for their
//! antialiased edge), so the page only uploads them and replays the draws.
//! Images are textures the page keeps by id: an image is uploaded the first
//! time it is drawn (keyed by its `Arc<Pixmap>`'s identity, as the other
//! executors key theirs) and freed once it is dropped or hasn't been drawn
//! for a while. Glyph rasters are images like any other.
//!
//! The buffer is a run of 32-bit words (`f32` bits where a value is a float):
//!
//! ```text
//! header   width height bg_r bg_g bg_b bg_a          (background premultiplied)
//! 1 box    rect[4] radius stroke color[4] n verts[n*4]
//! 2 path   even_odd color[4] bbox[4](i32) n verts[n*4]        (fan triangles)
//! 3 image  id rect[4] px[2] opacity uv[4] clamp[4] tint[4] nearest n verts[n*4]
//! 4 clip   rect[4](i32)                                       (a scissor)
//! 5 clip   n verts[n*4]                         (a convex polygon, as triangles)
//! 6 pop clip
//! 7 layer  opacity
//! 8 pop layer
//! ```
//!
//! A vertex is `x y aux_x aux_y` (see the shaders in `web/gpu.js`).

use super::ops::{Clip, Op, Plan, Sample};
use super::renderer::{Api, Executor, GpuError};
use crate::geometry::ScaleFactor;
use crate::platform::GpuLayer;
use crate::render::Pixmap;
use oxideav_core::{Point, Transform2D};
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
        /// Free texture `id` (0: all of them).
        pub fn gpu_free(id: u32);
        /// Draw a frame: 0 shown, 1 the device was lost, 2 not now (the page
        /// asks for a redraw), 3 the GPU can't draw it.
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

const OP_BOX: u32 = 1;
const OP_PATH: u32 = 2;
const OP_IMAGE: u32 = 3;
const OP_CLIP_RECT: u32 = 4;
const OP_CLIP_POLY: u32 = 5;
const OP_POP_CLIP: u32 = 6;
const OP_LAYER: u32 = 7;
const OP_POP_LAYER: u32 = 8;

struct Texture {
    image: Weak<Pixmap>,
    id: u32,
    used: u64,
}

/// The web executor: the page's GPU, the textures it holds, and the op
/// buffer, kept from frame to frame.
pub(crate) struct WebExecutor {
    api: Api,
    textures: HashMap<usize, Texture>,
    next_id: u32,
    count: u64,
    uploads: usize,
    buf: Vec<u32>,
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
            next_id: 1,
            count: 0,
            uploads: 0,
            buf: Vec::new(),
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
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
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
            },
        );
        Some(id)
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
        self.uploads = 0;
        for op in &plan.ops {
            match op {
                Op::Box {
                    rect,
                    radius,
                    stroke,
                    color,
                } => {
                    buf.push(OP_BOX);
                    push_f(&mut buf, rect);
                    push_f(&mut buf, &[*radius, *stroke]);
                    push_f(&mut buf, color);
                    let pad = 1.0 + stroke * 0.5;
                    let (x0, y0, x1, y1) =
                        (rect[0] - pad, rect[1] - pad, rect[2] + pad, rect[3] + pad);
                    let v = |x: f32, y: f32| [x, y, x, y];
                    push_quad(&mut buf, [v(x0, y0), v(x1, y0), v(x0, y1), v(x1, y1)]);
                }
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
                Op::Image {
                    image,
                    transform,
                    rect,
                    opacity,
                    uv,
                    tint,
                    nearest,
                } => {
                    let Some(id) = self.texture(image) else {
                        continue;
                    };
                    let size = image.size();
                    let sample = Sample::region(*uv, size.width, size.height, *tint, *nearest);
                    push_image(&mut buf, id, transform, *rect, *opacity, &sample);
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

/// Two triangles over the corners `top-left, top-right, bottom-left,
/// bottom-right`, each `[x, y, aux_x, aux_y]`, with their count first.
fn push_quad(buf: &mut Vec<u32>, [a, b, c, d]: [[f32; 4]; 4]) {
    buf.push(6);
    for v in [a, b, c, c, b, d] {
        push_f(buf, &v);
    }
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

/// A textured quad over the local `rect` drawn through `m`, its edges
/// padded a device pixel for their antialiasing (as `draw_texture` in the
/// Metal executor).
fn push_image(
    buf: &mut Vec<u32>,
    id: u32,
    m: &Transform2D,
    rect: [f32; 4],
    opacity: f32,
    sample: &Sample,
) {
    let px = (m.a.hypot(m.b).max(1e-6), m.c.hypot(m.d).max(1e-6));
    buf.push(OP_IMAGE);
    buf.push(id);
    push_f(buf, &rect);
    push_f(buf, &[px.0, px.1, opacity]);
    push_f(buf, &sample.uv);
    push_f(buf, &sample.clamp);
    push_f(buf, &sample.tint);
    buf.push(u32::from(sample.nearest));
    let (pu, pv) = (1.0 / px.0, 1.0 / px.1);
    let (x0, y0, x1, y1) = (rect[0] - pu, rect[1] - pv, rect[2] + pu, rect[3] + pv);
    let corner = |x: f32, y: f32| {
        let d = m.apply(Point { x, y });
        [d.x, d.y, x, y]
    };
    push_quad(
        buf,
        [
            corner(x0, y0),
            corner(x1, y0),
            corner(x0, y1),
            corner(x1, y1),
        ],
    );
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
        plan: &Plan,
        _layer: &GpuLayer,
        _scale: ScaleFactor,
    ) -> Result<(), GpuError> {
        self.encode(plan);
        let status = unsafe { js::gpu_frame(self.buf.as_ptr(), self.buf.len()) };
        self.sweep();
        match status {
            0 => Ok(()),
            1 => Err(GpuError::Lost(format!("the {} device", self.api.name()))),
            2 => Err(GpuError::Pending),
            _ => Err(GpuError::Unavailable(format!(
                "{} couldn't draw the frame",
                self.api.name()
            ))),
        }
    }

    fn uploads(&self) -> usize {
        self.uploads
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
            next_id: 1,
            count: 0,
            uploads: 0,
            buf: Vec::new(),
        }
    }

    /// Walk an op buffer as `web/gpu.js` does, returning the ops in it.
    fn walk(buf: &[u32]) -> Vec<u32> {
        let mut ops = Vec::new();
        let mut p = 6;
        while p < buf.len() {
            let op = buf[p];
            ops.push(op);
            p += 1;
            // Fixed words, then (for some) a vertex count and its vertices.
            let (fixed, verts) = match op {
                OP_BOX => (10, true),
                OP_PATH => (9, true),
                OP_IMAGE => (21, true),
                OP_CLIP_RECT => (4, false),
                OP_CLIP_POLY => (0, true),
                OP_POP_CLIP | OP_POP_LAYER => (0, false),
                OP_LAYER => (1, false),
                _ => panic!("unknown op {op} at {}", p - 1),
            };
            p += fixed;
            if verts {
                p += 1 + buf[p] as usize * 4;
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
            assert_eq!(ops.len(), plan.ops.len(), "one op each");
            let count = |op: u32| ops.iter().filter(|&&o| o == op).count();
            assert_eq!(count(OP_LAYER), count(OP_POP_LAYER));
            assert_eq!(
                count(OP_CLIP_RECT) + count(OP_CLIP_POLY),
                count(OP_POP_CLIP)
            );
        }
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
        drop(plan);
        drop(scene);
        ex.sweep();
        assert!(
            ex.textures.is_empty(),
            "the image is gone: so is its texture"
        );
    }
}
