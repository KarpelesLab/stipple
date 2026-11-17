// gpu.js: Stipple's GPU renderer in the browser, for web/stipple.js.
//
// A module built with `--features stipple/gpu` lowers each frame's scene
// (src/gpu/ops.rs) and hands it over as a compact op buffer (the format is in
// src/gpu/web.rs): boxes, path fills, images, clips and layers, every draw
// with its vertices already in device pixels. This file draws it into the
// app's canvas with WebGPU (`canvas.getContext("webgpu")`, WGSL shaders) or,
// where the browser has no WebGPU, WebGL2 (GLSL ES 3.0), the same way the
// Metal executor does natively (src/gpu/metal_scene.rs): batches of analytic
// boxes and textured quads (images, glyphs from the atlas) each one draw
// (src/gpu/batch.rs), paths by stencil-and-cover at 4x4 supersampling in a
// scratch target, scissor and stencil clips, and
// layers in offscreen targets composited at their opacity (a resampled layer,
// a game's low-resolution frame, drawn in its own pixels and scaled into its
// rect), and external sources (a <video>, a VideoFrame, an ImageBitmap or a
// canvas the page registered, e.g. the camera's video) sampled straight from
// the source each frame: WebGPU imports a video or VideoFrame as a
// texture_external (importExternalTexture) and copies the others on the GPU
// (copyExternalImageToTexture); WebGL2 hands them to texImage2D. Everything is
// blended in premultiplied alpha on the encoded sRGB values, as the software
// renderer blends. No read-back: the canvas shows what was drawn.
//
// The frame is first *recorded* (shared by both APIs: the clip, layer and
// path logic) into a list of render-target switches and draws plus two
// arrays of vertices (paths and clips; batched quads) and one of uniforms,
// then *replayed* by the API's backend with one upload of each.
//
// A frame can be partial (the header's flags): it redraws only what changed
// (its ops are clipped to that) over the last frame, so the last frame is
// kept: in a preserved drawing buffer on WebGL2, in a texture of the
// renderer's own on WebGPU (the canvas's texture is new each frame), which is
// copied into the canvas's. Without a last frame to draw over, `frame`
// answers 4 and the module draws the next one whole.
//
//   const host = gpuHost({ memory, claim, current, redraw, log, source, onUpload });
//   imports.stipple = { ...host.imports, ... };
//
// `claim(fresh)` returns the canvas to take a GPU context on (stipple.js gives
// a fresh one if the 2D context has it, or `fresh` asks for one), `current()` the canvas the app is
// shown in now, `redraw()` asks the app for a whole frame (the GPU came up,
// asynchronously), `memory()` is the module's memory, `source(id)` the
// external source the page registered as `id` (stipple.js's
// `externalSource`), `onUpload()` hears each texture upload (stats).

// ---- The op buffer (src/gpu/web.rs) -----------------------------------------

const OP_PATH = 2, OP_CLIP_RECT = 4, OP_CLIP_POLY = 5,
  OP_POP_CLIP = 6, OP_LAYER = 7, OP_POP_LAYER = 8, OP_QUADS = 9,
  OP_RESAMPLED = 10, OP_QUADS_EXTERNAL = 11;

// What a recorded draw uses: its pipeline (shader, blending, stencil state).
// P_QUAD draws a batch of boxes and textured quads, P_QUADBOX one of boxes
// only (no texture); both read the quad vertices (QUAD_FLOATS each, the
// format of src/gpu/batch.rs), the others the 4-float ones. P_QUAD_EXT is
// P_QUAD sampling an external source (WebGPU: a texture_external).
const P_QUAD = 0, P_QUADBOX = 1, P_RESOLVE = 2, P_CLIP_INC = 3, P_CLIP_DEC = 4,
  P_WIND_NZ = 5, P_WIND_EO = 6, P_COVER_NZ = 7, P_COVER_EO = 8, P_QUAD_EXT = 9;
const TEXTURED = [true, false, true, false, false, false, false, false, false, true];
const QUAD = [true, true, false, false, false, false, false, false, false, true];
const QUAD_FLOATS = 20;
const NO_EDGE = 1e4;

// Render targets: the frame, a layer (0, 1, ...), the path scratch.
const T_FRAME = -1, T_SCRATCH = -2;
// A draw's texture: an image's id (1...), a layer, the scratch, or an
// external source.
const TEX_SCRATCH = -1000000;
const layerTex = (i) => -(i + 1);
const TEX_EXTERNAL = -2000000;
const extTex = (id) => TEX_EXTERNAL - id;
const extOf = (tex) => (tex <= TEX_EXTERNAL ? TEX_EXTERNAL - tex : 0);

// Recorded commands, CMD words each.
const C_OPEN = 1, C_DRAW = 2;
// How a C_OPEN starts its target: keeping it, cleared, or keeping its colors
// with the stencil cleared (a partial frame's first pass).
const KEEP = 0, CLEAR = 1, CLEAR_STENCIL = 2;
// The op buffer's header flags.
const FLAG_PARTIAL = 1;
const CMD = 12;

// A uniform slot: xf rect color uv clamp p0 p1 p2, one vec4 each (128 bytes),
// at a stride of 256 bytes (WebGPU's dynamic-offset alignment).
const SLOT_FLOATS = 32;
const SLOT_BYTES = 128;

// ---- Recording ----------------------------------------------------------------

class Recording {
  constructor() {
    this.cmds = new Int32Array(1024);
    this.ncmd = 0;
    this.verts = new Float32Array(16384);
    this.nvert = 0; // vertices (4 floats each)
    this.quads = new Float32Array(65536);
    this.nquad = 0; // quad vertices (QUAD_FLOATS each)
    this.unis = new Float32Array(4096);
    this.nslot = 0;
    this.stride = 64; // floats per slot
  }

  reset(w, h, bg, stride, partial) {
    this.w = w;
    this.h = h;
    this.bg = bg;
    this.partial = partial;
    this.stride = stride;
    this.ncmd = 0;
    this.nvert = 0;
    this.nquad = 0;
    this.nslot = 0;
    this.layers = 0;
    this.scratchW = 0;
    this.scratchH = 0;
    this.externals = new Set(); // the external sources drawn
  }

  cmd(...words) {
    if ((this.ncmd + 1) * CMD > this.cmds.length) {
      const c = new Int32Array(this.cmds.length * 2);
      c.set(this.cmds);
      this.cmds = c;
    }
    this.cmds.set(words, this.ncmd * CMD);
    this.ncmd++;
  }

  // Room for `n` more vertices; returns the first one's index.
  room(n) {
    const need = (this.nvert + n) * 4;
    if (need > this.verts.length) {
      const v = new Float32Array(Math.max(need, this.verts.length * 2));
      v.set(this.verts.subarray(0, this.nvert * 4));
      this.verts = v;
    }
    const first = this.nvert;
    this.nvert += n;
    return first;
  }

  // Copy `n` vertices from `f` at word `at`.
  copy(f, at, n) {
    const first = this.room(n);
    this.verts.set(f.subarray(at, at + n * 4), first * 4);
    return first;
  }

  // Room for `n` more quad vertices; returns the first one's index.
  roomQuads(n) {
    const need = (this.nquad + n) * QUAD_FLOATS;
    if (need > this.quads.length) {
      const v = new Float32Array(Math.max(need, this.quads.length * 2));
      v.set(this.quads.subarray(0, this.nquad * QUAD_FLOATS));
      this.quads = v;
    }
    const first = this.nquad;
    this.nquad += n;
    return first;
  }

  // Copy `n` quad vertices from `f` at word `at`.
  copyQuads(f, at, n) {
    const first = this.roomQuads(n);
    this.quads.set(f.subarray(at, at + n * QUAD_FLOATS), first * QUAD_FLOATS);
    return first;
  }

  // A `w` x `h` layer's premultiplied texture over the whole target at
  // `opacity`, as a quad (batch.rs's `tex_quad`, edges not antialiased).
  layerQuad(w, h, opacity) {
    const first = this.roomQuads(6);
    let o = first * QUAD_FLOATS;
    for (const [x, y] of [[0, 0], [w, 0], [0, h], [0, h], [w, 0], [w, h]]) {
      this.quads.set([x, y, x / w, y / h, NO_EDGE, NO_EDGE, NO_EDGE, NO_EDGE, 0, 0, 1, 1,
        opacity, opacity, opacity, opacity, 2, 0, 0, 0], o);
      o += QUAD_FLOATS;
    }
    return first;
  }

  // A resampled layer's top-left `r.w` x `r.h` pixels of a `w` x `h` target
  // over the device rect `r.dst` (batch.rs's `layer_quad`): kept half a
  // texel inside the part drawn, so bilinear doesn't reach past it.
  resampledQuad(w, h, r) {
    const first = this.roomQuads(6);
    let o = first * QUAD_FLOATS;
    const [x0, y0, x1, y1] = r.dst;
    const u1 = r.w / w, v1 = r.h / h;
    const cu0 = Math.min(0.5 / w, u1 / 2), cv0 = Math.min(0.5 / h, v1 / 2);
    const cu1 = Math.max(u1 - 0.5 / w, u1 / 2), cv1 = Math.max(v1 - 0.5 / h, v1 / 2);
    for (const [x, y, s, t] of [[x0, y0, 0, 0], [x1, y0, u1, 0], [x0, y1, 0, v1],
      [x0, y1, 0, v1], [x1, y0, u1, 0], [x1, y1, u1, v1]]) {
      this.quads.set([x, y, s, t, NO_EDGE, NO_EDGE, NO_EDGE, NO_EDGE, cu0, cv0, cu1, cv1,
        1, 1, 1, 1, 2, 0, 0, 0], o);
      o += QUAD_FLOATS;
    }
    return first;
  }

  // Two triangles over [x0, y0, x1, y1], aux from `aux(x, y)`.
  quad(x0, y0, x1, y1, aux) {
    const first = this.room(6);
    let o = first * 4;
    for (const [x, y] of [[x0, y0], [x1, y0], [x0, y1], [x0, y1], [x1, y0], [x1, y1]]) {
      const [a, b] = aux(x, y);
      this.verts[o++] = x;
      this.verts[o++] = y;
      this.verts[o++] = a;
      this.verts[o++] = b;
    }
    return first;
  }

  // A new uniform slot holding `xf`, then the given vec4s in order.
  slot(xf, ...vecs) {
    const need = (this.nslot + 1) * this.stride;
    if (need > this.unis.length) {
      const u = new Float32Array(Math.max(need, this.unis.length * 2));
      u.set(this.unis.subarray(0, this.nslot * this.stride));
      this.unis = u;
    }
    const o = this.nslot * this.stride;
    this.unis.fill(0, o, o + SLOT_FLOATS);
    this.unis.set(xf, o);
    let at = o + 4;
    for (const v of vecs) {
      if (v) this.unis.set(v, at);
      at += 4;
    }
    return this.nslot++;
  }
}

const intersect = (a, b) => {
  const r = [Math.max(a[0], b[0]), Math.max(a[1], b[1]), Math.min(a[2], b[2]), Math.min(a[3], b[3])];
  r[2] = Math.max(r[2], r[0]);
  r[3] = Math.max(r[3], r[1]);
  return r;
};
const empty = (s) => s[2] <= s[0] || s[3] <= s[1];
const WHITE = [1, 1, 1, 1];

// Record the frame in the op buffer `u` into `rec`, for a device whose
// largest texture side is `max`.
function record(u, rec, max, stride) {
  const f = new Float32Array(u.buffer, u.byteOffset, u.length);
  const i32 = new Int32Array(u.buffer, u.byteOffset, u.length);
  const W = u[0], H = u[1];
  const partial = (u[6] & FLAG_PARTIAL) !== 0;
  rec.reset(W, H, [f[2], f[3], f[4], f[5]], stride, partial);
  const xfFrame = [2 / W, -2 / H, -1, 1];
  const levels = [{ layer: T_FRAME, opacity: 1, scissor: [0, 0, W, H], depth: 0, undo: [] }];
  let on = null;
  const open = (target, clear, vw, vh) => {
    rec.cmd(C_OPEN, target, clear === true ? CLEAR : clear === false ? KEEP : clear, vw, vh);
    on = target;
  };
  const draw = (pipe, slot, tex, first, count, s, ref, nearest = 0) =>
    rec.cmd(C_DRAW, pipe, slot, tex, first, count, s[0], s[1], s[2] - s[0], s[3] - s[1], ref, nearest);
  // Get ready to draw into `level`; false when its scissor is empty.
  const bind = (level) => {
    if (empty(level.scissor)) return false;
    if (on !== level.layer) open(level.layer, false, W, H);
    return true;
  };
  const stencilPolygon = (level, first, n, from, pipe) => {
    if (on !== level.layer) open(level.layer, false, W, H);
    if (n > 0) draw(pipe, rec.slot(xfFrame), 0, first, n, [0, 0, W, H], from);
  };
  const composite = (layer, parent) => {
    if (!bind(parent)) return;
    const r = layer.resample;
    if (r) {
      const first = rec.resampledQuad(W, H, r);
      const slot = rec.slot(xfFrame, null, null, null, null, null, [0, 0, 0, r.nearest]);
      draw(P_QUAD, slot, layerTex(layer.layer), first, 6, parent.scissor, parent.depth, r.nearest);
      return;
    }
    const first = rec.layerQuad(W, H, layer.opacity);
    draw(P_QUAD, rec.slot(xfFrame), layerTex(layer.layer), first, 6, parent.scissor, parent.depth);
  };

  open(T_FRAME, partial ? CLEAR_STENCIL : CLEAR, W, H);
  let p = 7;
  while (p < u.length) {
    const op = u[p++];
    const level = levels[levels.length - 1];
    switch (op) {
      case OP_QUADS: {
        const id = u[p], nearest = u[p + 1], n = u[p + 2];
        const at = p + 3;
        p = at + n * QUAD_FLOATS;
        if (n === 0 || !bind(level)) break;
        // p1.w: nearest (WebGPU's shader picks its sampler by it).
        const slot = rec.slot(xfFrame, null, null, null, null, null, [0, 0, 0, nearest]);
        draw(id ? P_QUAD : P_QUADBOX, slot, id, rec.copyQuads(f, at, n), n, level.scissor,
          level.depth, nearest);
        break;
      }
      case OP_QUADS_EXTERNAL: {
        const id = u[p], nearest = u[p + 1], n = u[p + 2];
        const at = p + 3;
        p = at + n * QUAD_FLOATS;
        if (n === 0 || !bind(level)) break;
        rec.externals.add(id);
        const slot = rec.slot(xfFrame, null, null, null, null, null, [0, 0, 0, nearest]);
        draw(P_QUAD_EXT, slot, extTex(id), rec.copyQuads(f, at, n), n, level.scissor,
          level.depth, nearest);
        break;
      }
      case OP_PATH: {
        const evenOdd = u[p] !== 0;
        const color = f.subarray(p + 1, p + 5);
        const bbox = [i32[p + 5], i32[p + 6], i32[p + 7], i32[p + 8]];
        const n = u[p + 9];
        const at = p + 10;
        p = at + n * 4;
        const b = intersect(bbox, level.scissor);
        if (empty(b)) break;
        const bw = b[2] - b[0], bh = b[3] - b[1];
        const ss = [4, 2, 1].find((s) => bw * s <= max && bh * s <= max) ?? 1;
        const sw = bw * ss, sh = bh * ss;
        rec.scratchW = Math.max(rec.scratchW, sw);
        rec.scratchH = Math.max(rec.scratchH, sh);
        // Stencil the winding into the scratch target (a viewport of sw x sh
        // pixels in it), cover it to get coverage, resolve that into the level.
        open(T_SCRATCH, true, sw, sh);
        const xf = [2 * ss / sw, -2 * ss / sh, -2 * ss * b[0] / sw - 1, 2 * ss * b[1] / sh + 1];
        const all = [0, 0, sw, sh];
        if (n > 0) {
          draw(evenOdd ? P_WIND_EO : P_WIND_NZ, rec.slot(xf, null, WHITE), 0, rec.copy(f, at, n), n, all, 0);
        }
        const [x0, y0, x1, y1] = b;
        const cover = rec.quad(x0, y0, x1, y1, () => [0, 0]);
        draw(evenOdd ? P_COVER_EO : P_COVER_NZ, rec.slot(xf, null, WHITE), 0, cover, 6, all, 0);
        if (!bind(level)) break;
        // p2: the scratch's texel size (filled in at replay) and the taps.
        const slot = rec.slot(xfFrame, null, color, null, null, null, null, [0, 0, ss > 1 ? ss * 0.25 : 0, 0]);
        const first = rec.quad(x0, y0, x1, y1, (x, y) => [(x - x0) * ss, (y - y0) * ss]);
        draw(P_RESOLVE, slot, TEX_SCRATCH, first, 6, level.scissor, level.depth);
        break;
      }
      case OP_CLIP_RECT: {
        const r = [i32[p], i32[p + 1], i32[p + 2], i32[p + 3]];
        p += 4;
        level.undo.push({ scissor: level.scissor });
        level.scissor = intersect(level.scissor, r);
        break;
      }
      case OP_CLIP_POLY: {
        const n = u[p];
        const at = p + 1;
        p = at + n * 4;
        const first = rec.copy(f, at, n);
        stencilPolygon(level, first, n, level.depth, P_CLIP_INC);
        level.depth++;
        level.undo.push({ first, n });
        break;
      }
      case OP_POP_CLIP: {
        const undo = level.undo.pop();
        if (!undo) break;
        if (undo.scissor) {
          level.scissor = undo.scissor;
        } else {
          stencilPolygon(level, undo.first, undo.n, level.depth, P_CLIP_DEC);
          level.depth--;
        }
        break;
      }
      case OP_LAYER: {
        const opacity = f[p++];
        const index = levels.length - 1;
        rec.layers = Math.max(rec.layers, index + 1);
        levels.push({ layer: index, opacity, scissor: level.scissor, depth: 0, undo: [] });
        open(index, true, W, H);
        break;
      }
      case OP_RESAMPLED: {
        // Drawn in its own pixels, w x h at the layer's top-left, unclipped
        // by what encloses it; composited into dst.
        const w = u[p], h = u[p + 1];
        const dst = [f[p + 2], f[p + 3], f[p + 4], f[p + 5]];
        const nearest = u[p + 6];
        p += 7;
        const index = levels.length - 1;
        rec.layers = Math.max(rec.layers, index + 1);
        levels.push({ layer: index, opacity: 1, scissor: [0, 0, Math.min(w, W), Math.min(h, H)],
          depth: 0, undo: [], resample: { w, h, dst, nearest } });
        open(index, true, W, H);
        break;
      }
      case OP_POP_LAYER: {
        if (levels.length < 2) break;
        const layer = levels.pop();
        composite(layer, levels[levels.length - 1]);
        break;
      }
      default:
        throw new Error(`stipple gpu: unknown op ${op} at word ${p - 1}`);
    }
  }
  while (levels.length > 1) {
    const layer = levels.pop();
    composite(layer, levels[levels.length - 1]);
  }
}

// The scratch target grows in powers of two, up to `max`.
const grow = (want, have, max) =>
  Math.min(Math.max(64, 2 ** Math.ceil(Math.log2(Math.max(want, have, 1)))), max);

// Put the scratch's texel size into every resolve's uniforms.
function fillResolve(rec, sw, sh) {
  const c = rec.cmds;
  for (let i = 0; i < rec.ncmd; i++) {
    const o = i * CMD;
    if (c[o] === C_DRAW && c[o + 1] === P_RESOLVE) {
      const u = c[o + 2] * rec.stride + 28;
      rec.unis[u] = 1 / sw;
      rec.unis[u + 1] = 1 / sh;
    }
  }
}

// ---- WebGPU -------------------------------------------------------------------

const WGSL = /* wgsl */ `
struct U { xf: vec4f, rect: vec4f, color: vec4f, uv: vec4f, clampr: vec4f, p0: vec4f, p1: vec4f, p2: vec4f };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var t: texture_2d<f32>;
@group(0) @binding(2) var lin: sampler;
@group(0) @binding(3) var near: sampler;

struct VOut { @builtin(position) pos: vec4f, @location(0) aux: vec2f };

// Every program shares one vertex shader: xy in device pixels mapped to clip
// space by xf (scale.xy, offset.zw), and a pass-through aux.
@vertex fn vs(@location(0) xy: vec2f, @location(1) aux: vec2f) -> VOut {
  var o: VOut;
  o.pos = vec4f(xy * u.xf.xy + u.xf.zw, 0.0, 1.0);
  o.aux = aux;
  return o;
}

// Batched boxes and textured quads (src/gpu/batch.rs): every input is a
// vertex attribute, so a run of them is one draw.
struct QOut {
  @builtin(position) pos: vec4f,
  @location(0) p: vec2f,
  @location(1) a1: vec4f,
  @location(2) a2: vec4f,
  @location(3) a3: vec4f,
  @location(4) mode: f32,
};

@vertex fn quad_vs(@location(0) a0: vec4f, @location(1) a1: vec4f, @location(2) a2: vec4f,
                   @location(3) a3: vec4f, @location(4) a4: vec4f) -> QOut {
  var o: QOut;
  o.pos = vec4f(a0.xy * u.xf.xy + u.xf.zw, 0.0, 1.0);
  o.p = a0.zw;
  o.a1 = a1;
  o.a2 = a2;
  o.a3 = a3;
  o.mode = a4.x;
  return o;
}

// A box: p is the device position, a1 the rect, a2 the color, a3 the radius
// and stroke. Coverage is the exact overlap of the pixel with a sharp box
// (or ring), or a signed distance for rounded corners.
fn span(p: f32, h: f32) -> f32 { return max(min(h, p + 0.5) - max(-h, p - 0.5), 0.0); }
fn boxcov(p: vec2f, h0: vec2f) -> f32 { let h = max(h0, vec2f(0.0)); return span(p.x, h.x) * span(p.y, h.y); }
fn sdf(p: vec2f, h: vec2f, r: f32) -> f32 {
  let d = abs(p) - (h - vec2f(r));
  return length(max(d, vec2f(0.0))) + min(max(d.x, d.y), 0.0) - r;
}
fn quad_box(i: QOut) -> vec4f {
  let c = (i.a1.xy + i.a1.zw) * 0.5;
  let h = (i.a1.zw - i.a1.xy) * 0.5;
  let p = i.p - c;
  let radius = i.a3.x;
  let stroke = i.a3.y;
  let w = stroke * 0.5;
  var cov: f32;
  if (radius <= 0.0) {
    if (stroke > 0.0) { cov = boxcov(p, h + vec2f(w)) - boxcov(p, h - vec2f(w)); } else { cov = boxcov(p, h); }
  } else {
    let r = min(radius, min(h.x, h.y));
    let d = sdf(p, h, r);
    if (stroke > 0.0) {
      cov = clamp(0.5 - (d - w), 0.0, 1.0) - clamp(0.5 - (d + w), 0.0, 1.0);
    } else {
      cov = clamp(0.5 - d, 0.0, 1.0);
    }
  }
  return i.a2 * cov;
}

// Boxes only (a batch without a texture): textured quads are left out.
@fragment fn quadbox_fs(i: QOut) -> @location(0) vec4f {
  if (i.mode < 0.5) { return quad_box(i); }
  return vec4f(0.0);
}

// Boxes and textured quads: p is the texture coordinate, kept within a2; a1
// the distances to the edges in device pixels (for their coverage); a3 the
// tint times the opacity (premultiplied); mode 2 for premultiplied texels.
// p1.w: nearest. (Sampled before branching: textureSample needs uniform
// control flow.)
@fragment fn quad_fs(i: QOut) -> @location(0) vec4f {
  let uv = clamp(i.p, i.a2.xy, i.a2.zw);
  let a = textureSample(t, lin, uv);
  let b = textureSample(t, near, uv);
  if (i.mode < 0.5) { return quad_box(i); }
  var c = select(a, b, u.p1.w > 0.5);
  if (i.mode < 1.5) { c = vec4f(c.rgb * c.a, c.a); }
  c = c * i.a3;
  let cov = clamp(min(i.a1.x, i.a1.z) + 0.5, 0.0, 1.0) * clamp(min(i.a1.y, i.a1.w) + 0.5, 0.0, 1.0);
  return c * cov;
}

// A flat color: the stencil and cover passes, and stencil clips.
@fragment fn flat_fs() -> @location(0) vec4f { return u.color; }

// Resolving a supersampled path coverage: aux is the position in scratch
// texels; four bilinear taps average the pixel's block. p2: texel size, tap.
@fragment fn resolve_fs(i: VOut) -> @location(0) vec4f {
  let inv = u.p2.xy;
  let k = u.p2.z;
  let c = textureSample(t, lin, (i.aux + vec2f(-k, -k)) * inv).r
        + textureSample(t, lin, (i.aux + vec2f(k, -k)) * inv).r
        + textureSample(t, lin, (i.aux + vec2f(-k, k)) * inv).r
        + textureSample(t, lin, (i.aux + vec2f(k, k)) * inv).r;
  return u.color * (c * 0.25);
}
`;

// The quads sampling an external source (a <video> or VideoFrame imported as
// a texture_external): quad_fs over textureSampleBaseClampToEdge, in a module
// of its own (binding 1 is a texture_2d in the other).
const WGSL_EXT = WGSL.slice(0, WGSL.indexOf("// Every program shares one vertex shader"))
  .replace("@group(0) @binding(1) var t: texture_2d<f32>;", "@group(0) @binding(1) var te: texture_external;")
  + WGSL.slice(WGSL.indexOf("// Batched boxes and textured quads"), WGSL.indexOf("// Boxes only (a batch without a texture)"))
  + /* wgsl */ `
@fragment fn quad_ext_fs(i: QOut) -> @location(0) vec4f {
  let uv = clamp(i.p, i.a2.xy, i.a2.zw);
  let a = textureSampleBaseClampToEdge(te, lin, uv);
  let b = textureSampleBaseClampToEdge(te, near, uv);
  if (i.mode < 0.5) { return quad_box(i); }
  let c = vec4f(select(a, b, u.p1.w > 0.5).rgb, 1.0) * i.a3;
  let cov = clamp(min(i.a1.x, i.a1.z) + 0.5, 0.0, 1.0) * clamp(min(i.a1.y, i.a1.w) + 0.5, 0.0, 1.0);
  return c * cov;
}
`;

// `claim()` hands over the canvas, at the last moment: a canvas that gave a
// WebGPU context can't give a WebGL2 one. `fallback`: whether a fallback
// adapter (a software one, SwiftShader) will do.
async function createWebGpu(claim, log, fallback) {
  if (!navigator.gpu) return null;
  const adapter = await navigator.gpu.requestAdapter({ powerPreference: "high-performance" });
  if (!adapter) return null;
  // A fallback adapter runs on the CPU, slower than WebGL2 on the same
  // rasterizer, and where the browser composites in software (Chrome
  // without a GPU) its canvas can't show frames: asking for its texture
  // fails in the GPU process, which then drops every GPU context of the
  // page's process, WebGL2 and 2D canvases alike, other frames' too. Only
  // when WebGPU was asked for by name.
  if (!fallback && (adapter.info?.isFallbackAdapter ?? adapter.isFallbackAdapter)) {
    log("stipple: WebGPU has only a fallback (software) adapter: WebGL2 instead");
    return null;
  }
  const device = await adapter.requestDevice();
  const format = navigator.gpu.getPreferredCanvasFormat();
  device.pushErrorScope("validation");
  const module = device.createShaderModule({ code: WGSL });
  const U = { binding: 0, visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT,
    buffer: { type: "uniform", hasDynamicOffset: true, minBindingSize: SLOT_BYTES } };
  const layoutU = device.createBindGroupLayout({ entries: [U] });
  const layoutT = device.createBindGroupLayout({ entries: [U,
    { binding: 1, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: "float" } },
    { binding: 2, visibility: GPUShaderStage.FRAGMENT, sampler: { type: "filtering" } },
    { binding: 3, visibility: GPUShaderStage.FRAGMENT, sampler: { type: "filtering" } }] });
  const layoutE = device.createBindGroupLayout({ entries: [U,
    { binding: 1, visibility: GPUShaderStage.FRAGMENT, externalTexture: {} },
    { binding: 2, visibility: GPUShaderStage.FRAGMENT, sampler: { type: "filtering" } },
    { binding: 3, visibility: GPUShaderStage.FRAGMENT, sampler: { type: "filtering" } }] });
  const plU = device.createPipelineLayout({ bindGroupLayouts: [layoutU] });
  const plT = device.createPipelineLayout({ bindGroupLayouts: [layoutT] });
  const plE = device.createPipelineLayout({ bindGroupLayouts: [layoutE] });
  const moduleExt = device.createShaderModule({ code: WGSL_EXT });
  const blend = { color: { srcFactor: "one", dstFactor: "one-minus-src-alpha" },
    alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha" } };
  const face = (compare, passOp) => ({ compare, passOp, failOp: "keep", depthFailOp: "keep" });
  const plain = { entryPoint: "vs", buffers: [{ arrayStride: 16, attributes: [
    { shaderLocation: 0, offset: 0, format: "float32x2" },
    { shaderLocation: 1, offset: 8, format: "float32x2" }] }] };
  const quads = { entryPoint: "quad_vs", buffers: [{ arrayStride: QUAD_FLOATS * 4,
    attributes: [0, 1, 2, 3, 4].map((i) => ({ shaderLocation: i, offset: i * 16, format: "float32x4" })) }] };
  const pipeline = (fs, layout, target, { blended = false, write = true, front, back = front, read = 0xff, writeMask = 0, vertex = plain, mod = module }) =>
    device.createRenderPipeline({
      layout,
      vertex: { module: mod, ...vertex },
      fragment: { module: mod, entryPoint: fs, targets: [{ format: target,
        blend: blended ? blend : undefined, writeMask: write ? GPUColorWrite.ALL : 0 }] },
      primitive: { topology: "triangle-list", cullMode: "none" },
      depthStencil: { format: "stencil8", depthWriteEnabled: false, depthCompare: "always",
        stencilFront: front, stencilBack: back, stencilReadMask: read, stencilWriteMask: writeMask },
    });
  const test = face("equal", "keep");
  const pipes = [];
  pipes[P_QUAD] = pipeline("quad_fs", plT, format, { blended: true, front: test, vertex: quads });
  pipes[P_QUADBOX] = pipeline("quadbox_fs", plU, format, { blended: true, front: test, vertex: quads });
  pipes[P_RESOLVE] = pipeline("resolve_fs", plT, format, { blended: true, front: test });
  pipes[P_CLIP_INC] = pipeline("flat_fs", plU, format, { write: false, front: face("equal", "increment-clamp"), writeMask: 0xff });
  pipes[P_CLIP_DEC] = pipeline("flat_fs", plU, format, { write: false, front: face("equal", "decrement-clamp"), writeMask: 0xff });
  pipes[P_WIND_NZ] = pipeline("flat_fs", plU, "r8unorm", { write: false,
    front: face("always", "increment-wrap"), back: face("always", "decrement-wrap"), writeMask: 0xff });
  pipes[P_WIND_EO] = pipeline("flat_fs", plU, "r8unorm", { write: false, front: face("always", "invert"), writeMask: 0xff });
  pipes[P_COVER_NZ] = pipeline("flat_fs", plU, "r8unorm", { front: face("not-equal", "keep") });
  pipes[P_COVER_EO] = pipeline("flat_fs", plU, "r8unorm", { front: face("not-equal", "keep"), read: 1 });
  pipes[P_QUAD_EXT] = pipeline("quad_ext_fs", plE, format, { blended: true, front: test, vertex: quads, mod: moduleExt });
  const error = await device.popErrorScope();
  if (error) {
    log(`stipple: WebGPU couldn't build its pipelines: ${error.message}`);
    device.destroy();
    return null;
  }
  const canvas = claim();
  const ctx = canvas.getContext("webgpu");
  if (!ctx) {
    device.destroy();
    return null;
  }
  // The frame is drawn into a texture of the renderer's own and copied in.
  const usage = GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_DST;
  ctx.configure({ device, format, alphaMode: "premultiplied", usage });
  // A device doesn't mean the canvas can show its frames (a browser without
  // GPU compositing can have WebGPU on SwiftShader and still fail to hand out
  // the canvas's texture): clear it once and see.
  device.pushErrorScope("validation");
  const probe = device.createCommandEncoder();
  probe.beginRenderPass({ colorAttachments: [{ view: ctx.getCurrentTexture().createView(),
    loadOp: "clear", storeOp: "store", clearValue: { r: 0, g: 0, b: 0, a: 0 } }] }).end();
  device.queue.submit([probe.finish()]);
  const shown = await device.popErrorScope();
  if (shown) {
    log(`stipple: WebGPU can't draw into this canvas: ${shown.message}`);
    ctx.unconfigure();
    device.destroy();
    return null;
  }
  return new WebGpuRenderer(canvas, device, ctx, format, pipes, layoutU, layoutT, layoutE, log);
}

// Whether `src` is a source WebGPU imports as a texture_external: a video
// with a frame, or a VideoFrame.
const importable = (src) =>
  (typeof HTMLVideoElement !== "undefined" && src instanceof HTMLVideoElement && src.readyState >= 2 && src.videoWidth > 0)
  || (typeof VideoFrame !== "undefined" && src instanceof VideoFrame);

// An external source's size, or null if it has nothing to draw yet.
const sourceSize = (src) => {
  if (!src) return null;
  if (typeof HTMLVideoElement !== "undefined" && src instanceof HTMLVideoElement) {
    return src.readyState >= 2 && src.videoWidth > 0 ? [src.videoWidth, src.videoHeight] : null;
  }
  if (typeof VideoFrame !== "undefined" && src instanceof VideoFrame) {
    return [src.displayWidth, src.displayHeight];
  }
  const w = src.naturalWidth ?? src.width, h = src.naturalHeight ?? src.height;
  return w > 0 && h > 0 ? [w, h] : null;
};

class WebGpuRenderer {
  constructor(canvas, device, ctx, format, pipes, layoutU, layoutT, layoutE, log) {
    this.api = "webgpu";
    this.canvas = canvas;
    this.device = device;
    this.ctx = ctx;
    this.format = format;
    this.pipes = pipes;
    this.layoutU = layoutU;
    this.layoutT = layoutT;
    this.layoutE = layoutE;
    this.source = () => null; // set by gpuHost
    this.copies = new Map(); // external source id -> { tex, view, w, h, bg, gen } (copied ones)
    this.max = device.limits.maxTextureDimension2D;
    this.stride = Math.max(256, device.limits.minUniformBufferOffsetAlignment) / 4;
    this.textures = new Map(); // id -> { tex, view, bg, gen }
    this.layers = []; // { tex, view, stencil, bg, gen }
    this.scratch = null;
    this.frameStencil = null;
    // The frame target, which keeps the last frame (see `frame`).
    this.frameTex = null;
    this.vbuf = null;
    this.qbuf = null;
    this.ubuf = null;
    this.gen = 0; // bumped when the uniform buffer is replaced (bind groups follow)
    this.lin = device.createSampler({ magFilter: "linear", minFilter: "linear" });
    this.near = device.createSampler({ magFilter: "nearest", minFilter: "nearest" });
    this.rec = new Recording();
    this.lost = false;
    this.broken = false;
    device.lost.then((info) => {
      if (!this.destroyed) log(`stipple: the WebGPU device was lost (${info.reason || "unknown"}: ${info.message})`);
      this.lost = true;
      // Ask for a frame, which sets the renderer up again: an app with
      // nothing animating wouldn't draw until its next input.
      if (!this.destroyed) this.onBroken?.();
    });
    device.addEventListener?.("uncapturederror", (ev) => {
      if (!this.broken) log(`stipple: WebGPU: ${ev.error?.message ?? ev.error}`);
      this.broken = true;
      // The frame was lost: ask for another, which falls back to the CPU.
      this.onBroken?.();
    });
  }

  texture(id, data, w, h) {
    if (w > this.max || h > this.max) return false;
    const tex = this.device.createTexture({ size: [w, h], format: "rgba8unorm",
      usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
    this.device.queue.writeTexture({ texture: tex }, data, { bytesPerRow: w * 4 }, [w, h]);
    this.free(id);
    this.textures.set(id, { tex, view: tex.createView(), bg: null, gen: -1 });
    return true;
  }

  // Replace the w x h pixels at (x, y) of texture `id`.
  textureSub(id, data, x, y, w, h) {
    const t = this.textures.get(id);
    if (!t) return false;
    this.device.queue.writeTexture({ texture: t.tex, origin: [x, y] }, data, { bytesPerRow: w * 4 }, [w, h]);
    return true;
  }

  // The page let go of external source `id`.
  freeExternal(id) {
    const c = this.copies.get(id);
    if (c) {
      c.tex.destroy();
      this.copies.delete(id);
    }
  }

  // This frame's external sources, ready to draw: by id, the bind group and
  // the pipeline (a texture_external for a video or VideoFrame, else a copy
  // made on the GPU drawn as an image).
  externals(rec) {
    const out = new Map();
    for (const id of rec.externals) {
      const src = this.source(id);
      const size = sourceSize(src);
      if (!size) continue;
      if (importable(src)) {
        const ext = this.device.importExternalTexture({ source: src });
        out.set(id, { pipe: P_QUAD_EXT, group: this.device.createBindGroup({ layout: this.layoutE, entries: [
          { binding: 0, resource: { buffer: this.ubuf, size: SLOT_BYTES } },
          { binding: 1, resource: ext },
          { binding: 2, resource: this.lin },
          { binding: 3, resource: this.near }] }) });
        continue;
      }
      let c = this.copies.get(id);
      if (!c || c.w !== size[0] || c.h !== size[1]) {
        c?.tex.destroy();
        const tex = this.device.createTexture({ size, format: "rgba8unorm", usage: GPUTextureUsage.TEXTURE_BINDING
          | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT });
        c = { tex, view: tex.createView(), w: size[0], h: size[1], bg: null, gen: -1 };
        this.copies.set(id, c);
      }
      this.device.queue.copyExternalImageToTexture({ source: src }, { texture: c.tex }, size);
      out.set(id, { pipe: P_QUAD, group: this.bindGroup(c) });
    }
    return out;
  }

  free(id) {
    if (id === 0) {
      for (const t of this.textures.values()) t.tex.destroy();
      this.textures.clear();
      return;
    }
    const t = this.textures.get(id);
    if (t) {
      // A texture an encoded but unsubmitted frame uses can't be destroyed
      // yet; frames are submitted at once, so this one is done with.
      t.tex.destroy();
      this.textures.delete(id);
    }
  }

  target(w, h, format) {
    const tex = this.device.createTexture({ size: [w, h], format,
      usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING });
    const stencil = this.device.createTexture({ size: [w, h], format: "stencil8",
      usage: GPUTextureUsage.RENDER_ATTACHMENT });
    return { tex, view: tex.createView(), stencil, sview: stencil.createView(), w, h, bg: null, gen: -1 };
  }

  bindGroup(t) {
    if (t.gen !== this.gen) {
      t.bg = this.device.createBindGroup({ layout: this.layoutT, entries: [
        { binding: 0, resource: { buffer: this.ubuf, size: SLOT_BYTES } },
        { binding: 1, resource: t.view },
        { binding: 2, resource: this.lin },
        { binding: 3, resource: this.near }] });
      t.gen = this.gen;
    }
    return t.bg;
  }

  buffers(rec) {
    const vbytes = Math.max(rec.nvert * 16, 16);
    if (!this.vbuf || this.vbuf.size < vbytes) {
      this.vbuf?.destroy();
      this.vbuf = this.device.createBuffer({ size: Math.max(vbytes * 2, 1 << 18),
        usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST });
    }
    const qbytes = Math.max(rec.nquad * QUAD_FLOATS * 4, 16);
    if (!this.qbuf || this.qbuf.size < qbytes) {
      this.qbuf?.destroy();
      this.qbuf = this.device.createBuffer({ size: Math.max(qbytes * 2, 1 << 20),
        usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST });
    }
    const ubytes = Math.max(rec.nslot, 1) * rec.stride * 4;
    if (!this.ubuf || this.ubuf.size < ubytes) {
      this.ubuf?.destroy();
      this.ubuf = this.device.createBuffer({ size: Math.max(ubytes * 2, 1 << 16),
        usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
      this.gen++;
      this.bgU = this.device.createBindGroup({ layout: this.layoutU,
        entries: [{ binding: 0, resource: { buffer: this.ubuf, size: SLOT_BYTES } }] });
    }
  }

  // Draw the op buffer `u` into the canvas: 0 shown, 1 lost, 3 can't.
  frame(u) {
    if (this.lost) return 1;
    if (this.broken) return 3;
    const W = u[0], H = u[1];
    if (W > this.max || H > this.max) return 3;
    const rec = this.rec;
    record(u, rec, this.max, this.stride);
    const dev = this.device;
    const ft = this.frameTex;
    if (rec.partial && !(ft && ft.w === W && ft.h === H)) return 4;
    if (this.canvas.width !== W || this.canvas.height !== H) {
      this.canvas.width = W;
      this.canvas.height = H;
    }
    if (!ft || ft.w !== W || ft.h !== H) {
      ft?.tex.destroy();
      const tex = dev.createTexture({ size: [W, H], format: this.format,
        usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC });
      this.frameTex = { tex, view: tex.createView(), w: W, h: H };
    }
    if (this.layers[0] && (this.layers[0].w !== W || this.layers[0].h !== H)) {
      for (const l of this.layers) { l.tex.destroy(); l.stencil.destroy(); }
      this.layers = [];
    }
    while (this.layers.length < rec.layers) this.layers.push(this.target(W, H, this.format));
    if (rec.scratchW > 0 && (!this.scratch || this.scratch.w < rec.scratchW || this.scratch.h < rec.scratchH)) {
      const sw = grow(rec.scratchW, this.scratch?.w ?? 0, this.max);
      const sh = grow(rec.scratchH, this.scratch?.h ?? 0, this.max);
      if (this.scratch) { this.scratch.tex.destroy(); this.scratch.stencil.destroy(); }
      this.scratch = this.target(sw, sh, "r8unorm");
    }
    if (this.scratch) fillResolve(rec, this.scratch.w, this.scratch.h);
    if (!this.frameStencil || this.frameStencil.w !== W || this.frameStencil.h !== H) {
      this.frameStencil?.tex.destroy();
      const tex = dev.createTexture({ size: [W, H], format: "stencil8", usage: GPUTextureUsage.RENDER_ATTACHMENT });
      this.frameStencil = { tex, view: tex.createView(), w: W, h: H };
    }
    this.buffers(rec);
    dev.queue.writeBuffer(this.vbuf, 0, rec.verts.buffer, 0, rec.nvert * 16);
    if (rec.nquad > 0) dev.queue.writeBuffer(this.qbuf, 0, rec.quads.buffer, 0, rec.nquad * QUAD_FLOATS * 4);
    dev.queue.writeBuffer(this.ubuf, 0, rec.unis.buffer, 0, Math.max(rec.nslot, 1) * rec.stride * 4);
    const exts = rec.externals.size ? this.externals(rec) : null;
    const frameView = this.frameTex.view;
    const enc = dev.createCommandEncoder();
    let pass = null;
    let pipe = -1;
    let quadsBound = false;
    const c = rec.cmds;
    const bg = rec.bg;
    for (let i = 0; i < rec.ncmd; i++) {
      const o = i * CMD;
      if (c[o] === C_OPEN) {
        pass?.end();
        const target = c[o + 1], mode = c[o + 2];
        let view, sview;
        if (target === T_FRAME) { view = frameView; sview = this.frameStencil.view; }
        else if (target === T_SCRATCH) { view = this.scratch.view; sview = this.scratch.sview; }
        else { view = this.layers[target].view; sview = this.layers[target].sview; }
        const cc = target === T_FRAME ? { r: bg[0], g: bg[1], b: bg[2], a: bg[3] } : { r: 0, g: 0, b: 0, a: 0 };
        pass = enc.beginRenderPass({
          colorAttachments: [{ view, loadOp: mode === CLEAR ? "clear" : "load", storeOp: "store", clearValue: cc }],
          depthStencilAttachment: { view: sview, stencilLoadOp: mode === KEEP ? "load" : "clear",
            stencilStoreOp: "store", stencilClearValue: 0 },
        });
        if (target === T_SCRATCH) pass.setViewport(0, 0, c[o + 3], c[o + 4], 0, 1);
        pass.setVertexBuffer(0, this.vbuf);
        quadsBound = false;
        pipe = -1;
        continue;
      }
      // C_DRAW
      let p = c[o + 1];
      const slot = c[o + 2], tex = c[o + 3];
      let group = this.bgU;
      if (p === P_QUAD_EXT) {
        const e = exts?.get(extOf(tex));
        if (e) { group = e.group; p = e.pipe; }
        else p = P_QUADBOX; // nothing to draw from yet: its boxes still draw
      } else if (TEXTURED[p]) {
        const t = tex === TEX_SCRATCH ? this.scratch : tex < 0 ? this.layers[-tex - 1] : this.textures.get(tex);
        if (t) group = this.bindGroup(t);
        else if (p === P_QUAD) p = P_QUADBOX; // its boxes still draw
        else continue;
      }
      if (QUAD[p] !== quadsBound) {
        quadsBound = QUAD[p];
        pass.setVertexBuffer(0, quadsBound ? this.qbuf : this.vbuf);
      }
      if (p !== pipe) { pass.setPipeline(this.pipes[p]); pipe = p; }
      pass.setBindGroup(0, group, [slot * rec.stride * 4]);
      pass.setScissorRect(c[o + 6], c[o + 7], c[o + 8], c[o + 9]);
      pass.setStencilReference(c[o + 10]);
      pass.draw(c[o + 5], 1, c[o + 4], 0);
    }
    pass?.end();
    enc.copyTextureToTexture({ texture: this.frameTex.tex }, { texture: this.ctx.getCurrentTexture() }, [W, H]);
    dev.queue.submit([enc.finish()]);
    return 0;
  }

  // Wait for the GPU (timing).
  finish() {
    return this.device.queue.onSubmittedWorkDone();
  }

  // Lose the device on purpose (tests).
  lose() {
    this.device.destroy();
  }

  destroy() {
    this.destroyed = true;
    this.device.destroy();
  }
}

// ---- WebGL2 -------------------------------------------------------------------

const GLSL_VS = `#version 300 es
precision highp float;
layout(std140) uniform U { vec4 xf; vec4 rect; vec4 color; vec4 uv; vec4 clampr; vec4 p0; vec4 p1; vec4 p2; };
// The frame is drawn y-down into the canvas; offscreen targets (layers, the
// scratch) are stored top row first, so their texture coordinates match.
uniform float u_flip;
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_aux;
out vec2 v_aux;
void main() {
  v_aux = a_aux;
  gl_Position = vec4(a_pos * xf.xy + xf.zw, 0.0, 1.0);
  gl_Position.y *= u_flip;
}
`;

// The batched quads' vertex shader (WGSL's quad_vs).
const GLSL_QUAD_VS = `#version 300 es
precision highp float;
layout(std140) uniform U { vec4 xf; vec4 rect; vec4 color; vec4 uv; vec4 clampr; vec4 p0; vec4 p1; vec4 p2; };
uniform float u_flip;
layout(location = 0) in vec4 a0;
layout(location = 1) in vec4 a1;
layout(location = 2) in vec4 a2;
layout(location = 3) in vec4 a3;
layout(location = 4) in vec4 a4;
out vec2 v_p;
out vec4 v_a1;
out vec4 v_a2;
out vec4 v_a3;
out float v_mode;
void main() {
  v_p = a0.zw;
  v_a1 = a1;
  v_a2 = a2;
  v_a3 = a3;
  v_mode = a4.x;
  gl_Position = vec4(a0.xy * xf.xy + xf.zw, 0.0, 1.0);
  gl_Position.y *= u_flip;
}
`;

const GLSL_QUAD_HEAD = `#version 300 es
precision highp float;
uniform sampler2D u_tex;
in vec2 v_p;
in vec4 v_a1;
in vec4 v_a2;
in vec4 v_a3;
in float v_mode;
out vec4 o;
float span(float p, float h) { return max(min(h, p + 0.5) - max(-h, p - 0.5), 0.0); }
float boxcov(vec2 p, vec2 h) { h = max(h, vec2(0.0)); return span(p.x, h.x) * span(p.y, h.y); }
float sdf(vec2 p, vec2 h, float r) {
  vec2 d = abs(p) - (h - vec2(r));
  return length(max(d, vec2(0.0))) + min(max(d.x, d.y), 0.0) - r;
}
vec4 quadBox() {
  vec2 c = (v_a1.xy + v_a1.zw) * 0.5;
  vec2 h = (v_a1.zw - v_a1.xy) * 0.5;
  vec2 p = v_p - c;
  float radius = v_a3.x, stroke = v_a3.y, w = stroke * 0.5;
  float cov;
  if (radius <= 0.0) {
    cov = stroke > 0.0 ? boxcov(p, h + vec2(w)) - boxcov(p, h - vec2(w)) : boxcov(p, h);
  } else {
    float r = min(radius, min(h.x, h.y));
    float d = sdf(p, h, r);
    cov = stroke > 0.0 ? clamp(0.5 - (d - w), 0.0, 1.0) - clamp(0.5 - (d + w), 0.0, 1.0)
                       : clamp(0.5 - d, 0.0, 1.0);
  }
  return v_a2 * cov;
}
`;

const GLSL_QUAD_FS = {
  quad: `
void main() {
  if (v_mode < 0.5) { o = quadBox(); return; }
  vec4 c = texture(u_tex, clamp(v_p, v_a2.xy, v_a2.zw));
  if (v_mode < 1.5) { c.rgb *= c.a; }
  c *= v_a3;
  float cov = clamp(min(v_a1.x, v_a1.z) + 0.5, 0.0, 1.0) * clamp(min(v_a1.y, v_a1.w) + 0.5, 0.0, 1.0);
  o = c * cov;
}`,
  quadbox: `
void main() { o = v_mode < 0.5 ? quadBox() : vec4(0.0); }`,
};

// The fragment shaders: the WGSL ones above, in GLSL ES 3.0.
const GLSL_FS_HEAD = `#version 300 es
precision highp float;
layout(std140) uniform U { vec4 xf; vec4 rect; vec4 color; vec4 uv; vec4 clampr; vec4 p0; vec4 p1; vec4 p2; };
uniform sampler2D u_tex;
in vec2 v_aux;
out vec4 o;
`;

const GLSL_FS = {
  flat: `
void main() { o = color; }`,
  resolve: `
void main() {
  vec2 inv = p2.xy;
  float k = p2.z;
  float c = texture(u_tex, (v_aux + vec2(-k, -k)) * inv).r
          + texture(u_tex, (v_aux + vec2(k, -k)) * inv).r
          + texture(u_tex, (v_aux + vec2(-k, k)) * inv).r
          + texture(u_tex, (v_aux + vec2(k, k)) * inv).r;
  o = color * (c * 0.25);
}`,
};

function createWebGl2(canvas, log) {
  const gl = canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: true, antialias: false,
    depth: false, stencil: true, preserveDrawingBuffer: true, powerPreference: "high-performance" });
  if (!gl) return null;
  if (gl.isContextLost()) return null;
  if (!gl.getContextAttributes()?.stencil) {
    log("stipple: WebGL2 gave the canvas no stencil buffer");
    return null;
  }
  const shader = (type, src) => {
    const s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS) && !gl.isContextLost()) {
      throw new Error(gl.getShaderInfoLog(s));
    }
    return s;
  };
  const programs = {};
  try {
    const vs = shader(gl.VERTEX_SHADER, GLSL_VS);
    const qvs = shader(gl.VERTEX_SHADER, GLSL_QUAD_VS);
    const all = [
      ...Object.entries(GLSL_FS).map(([name, body]) => [name, vs, GLSL_FS_HEAD + body]),
      ...Object.entries(GLSL_QUAD_FS).map(([name, body]) => [name, qvs, GLSL_QUAD_HEAD + body]),
    ];
    for (const [name, v, fs] of all) {
      const p = gl.createProgram();
      gl.attachShader(p, v);
      gl.attachShader(p, shader(gl.FRAGMENT_SHADER, fs));
      gl.linkProgram(p);
      if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p));
      gl.uniformBlockBinding(p, gl.getUniformBlockIndex(p, "U"), 0);
      gl.useProgram(p);
      const tex = gl.getUniformLocation(p, "u_tex");
      if (tex) gl.uniform1i(tex, 0);
      programs[name] = { p, flip: gl.getUniformLocation(p, "u_flip"), flipped: 0 };
    }
  } catch (err) {
    log(`stipple: WebGL2 couldn't build its shaders: ${err.message ?? err}`);
    return null;
  }
  return new WebGl2Renderer(canvas, gl, programs, log);
}

class WebGl2Renderer {
  constructor(canvas, gl, programs, log) {
    this.api = "webgl2";
    this.canvas = canvas;
    this.gl = gl;
    this.programs = programs;
    this.max = gl.getParameter(gl.MAX_TEXTURE_SIZE);
    const align = gl.getParameter(gl.UNIFORM_BUFFER_OFFSET_ALIGNMENT);
    this.stride = Math.max(256, align) / 4;
    this.textures = new Map(); // id -> WebGLTexture
    this.source = () => null; // set by gpuHost
    this.extTextures = new Map(); // external source id -> WebGLTexture
    this.layers = []; // { fbo, tex, rb, w, h }
    this.scratch = null;
    this.rec = new Recording();
    this.wasLost = false;
    // The drawing buffer (preserved) holds the last frame, of this size.
    this.kept = null;
    this.vao = gl.createVertexArray();
    this.vbo = gl.createBuffer();
    this.ubo = gl.createBuffer();
    gl.bindVertexArray(this.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.vbo);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 16, 0);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 2, gl.FLOAT, false, 16, 8);
    this.qvao = gl.createVertexArray();
    this.qvbo = gl.createBuffer();
    gl.bindVertexArray(this.qvao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.qvbo);
    for (let i = 0; i < 5; i++) {
      gl.enableVertexAttribArray(i);
      gl.vertexAttribPointer(i, 4, gl.FLOAT, false, QUAD_FLOATS * 4, i * 16);
    }
    gl.bindVertexArray(null);
    const sampler = (filter) => {
      const s = gl.createSampler();
      gl.samplerParameteri(s, gl.TEXTURE_MIN_FILTER, filter);
      gl.samplerParameteri(s, gl.TEXTURE_MAG_FILTER, filter);
      gl.samplerParameteri(s, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
      gl.samplerParameteri(s, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
      return s;
    };
    this.lin = sampler(gl.LINEAR);
    this.near = sampler(gl.NEAREST);
    this.onLost = (ev) => {
      ev.preventDefault(); // so the context can come back
      if (this.destroyed || this.wasLost) return;
      log("stipple: the WebGL2 context was lost");
      this.wasLost = true;
      // Ask for a frame, which waits for the context to come back and draws
      // again: an app with nothing animating wouldn't until its next input.
      this.onBroken?.();
    };
    canvas.addEventListener("webglcontextlost", this.onLost);
  }

  // Lost: the event said so, or the context says so before its event came.
  get lost() {
    return this.wasLost || this.gl.isContextLost();
  }

  set lost(v) {
    this.wasLost = v;
  }

  newTexture(format, w, h) {
    const gl = this.gl;
    const tex = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texStorage2D(gl.TEXTURE_2D, 1, format, w, h);
    return tex;
  }

  texture(id, data, w, h) {
    if (w > this.max || h > this.max || this.lost) return false;
    const gl = this.gl;
    this.free(id);
    const tex = this.newTexture(gl.RGBA8, w, h);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_COLORSPACE_CONVERSION_WEBGL, gl.NONE);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, data);
    this.textures.set(id, tex);
    return true;
  }

  // Replace the w x h pixels at (x, y) of texture `id`.
  textureSub(id, data, x, y, w, h) {
    const tex = this.textures.get(id);
    if (!tex || this.lost) return false;
    const gl = this.gl;
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
    gl.pixelStorei(gl.UNPACK_COLORSPACE_CONVERSION_WEBGL, gl.NONE);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, x, y, w, h, gl.RGBA, gl.UNSIGNED_BYTE, data);
    return true;
  }

  // The page let go of external source `id`.
  freeExternal(id) {
    const t = this.extTextures.get(id);
    if (t && !this.lost) this.gl.deleteTexture(t);
    this.extTextures.delete(id);
  }

  // This frame's external sources, each handed to texImage2D (a copy the
  // browser makes on the GPU for a video): by id, the texture.
  externals(rec) {
    const gl = this.gl;
    const out = new Map();
    for (const id of rec.externals) {
      const src = this.source(id);
      if (!sourceSize(src)) continue;
      let tex = this.extTextures.get(id);
      if (!tex) {
        tex = gl.createTexture();
        gl.bindTexture(gl.TEXTURE_2D, tex);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
        this.extTextures.set(id, tex);
      }
      gl.bindTexture(gl.TEXTURE_2D, tex);
      gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
      gl.pixelStorei(gl.UNPACK_PREMULTIPLY_ALPHA_WEBGL, false);
      try {
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, src);
        out.set(id, tex);
      } catch {
        // A source it can't take (a closed VideoFrame): nothing drawn.
      }
    }
    return out;
  }

  free(id) {
    // A lost context took its textures with it.
    const del = (t) => { if (!this.lost) this.gl.deleteTexture(t); };
    if (id === 0) {
      for (const t of this.textures.values()) del(t);
      this.textures.clear();
      return;
    }
    const t = this.textures.get(id);
    if (t) {
      del(t);
      this.textures.delete(id);
    }
  }

  target(w, h, format) {
    const gl = this.gl;
    const tex = this.newTexture(format, w, h);
    const rb = gl.createRenderbuffer();
    gl.bindRenderbuffer(gl.RENDERBUFFER, rb);
    gl.renderbufferStorage(gl.RENDERBUFFER, gl.DEPTH24_STENCIL8, w, h);
    const fbo = gl.createFramebuffer();
    gl.bindFramebuffer(gl.FRAMEBUFFER, fbo);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
    gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.DEPTH_STENCIL_ATTACHMENT, gl.RENDERBUFFER, rb);
    return { fbo, tex, rb, w, h };
  }

  dropTarget(t) {
    const gl = this.gl;
    gl.deleteFramebuffer(t.fbo);
    gl.deleteTexture(t.tex);
    gl.deleteRenderbuffer(t.rb);
  }

  // Blending, color writes and the stencil test for pipeline `p`, at
  // stencil reference `ref`.
  state(p, ref) {
    const gl = this.gl;
    switch (p) {
      case P_QUAD: case P_QUADBOX: case P_RESOLVE: case P_QUAD_EXT:
        gl.enable(gl.BLEND);
        gl.colorMask(true, true, true, true);
        gl.stencilFunc(gl.EQUAL, ref, 0xff);
        gl.stencilOp(gl.KEEP, gl.KEEP, gl.KEEP);
        gl.stencilMask(0);
        break;
      case P_CLIP_INC: case P_CLIP_DEC:
        gl.disable(gl.BLEND);
        gl.colorMask(false, false, false, false);
        gl.stencilFunc(gl.EQUAL, ref, 0xff);
        gl.stencilOp(gl.KEEP, gl.KEEP, p === P_CLIP_INC ? gl.INCR : gl.DECR);
        gl.stencilMask(0xff);
        break;
      case P_WIND_NZ: case P_WIND_EO:
        gl.disable(gl.BLEND);
        gl.colorMask(false, false, false, false);
        gl.stencilFunc(gl.ALWAYS, 0, 0xff);
        if (p === P_WIND_EO) {
          gl.stencilOp(gl.KEEP, gl.KEEP, gl.INVERT);
        } else {
          gl.stencilOpSeparate(gl.FRONT, gl.KEEP, gl.KEEP, gl.INCR_WRAP);
          gl.stencilOpSeparate(gl.BACK, gl.KEEP, gl.KEEP, gl.DECR_WRAP);
        }
        gl.stencilMask(0xff);
        break;
      default: // the covers
        gl.disable(gl.BLEND);
        gl.colorMask(true, true, true, true);
        gl.stencilFunc(gl.NOTEQUAL, 0, p === P_COVER_EO ? 1 : 0xff);
        gl.stencilOp(gl.KEEP, gl.KEEP, gl.KEEP);
        gl.stencilMask(0);
    }
  }

  frame(u) {
    const gl = this.gl;
    if (this.lost) {
      this.wasLost = true; // even once it's restored: these objects are gone
      return 1;
    }
    const W = u[0], H = u[1];
    if (W > this.max || H > this.max) return 3;
    const rec = this.rec;
    record(u, rec, this.max, this.stride);
    if (rec.partial && !(this.kept && this.kept[0] === W && this.kept[1] === H &&
      this.canvas.width === W && this.canvas.height === H)) {
      return 4;
    }
    if (this.canvas.width !== W || this.canvas.height !== H) {
      this.canvas.width = W;
      this.canvas.height = H;
    }
    this.kept = [W, H];
    if (this.layers[0] && (this.layers[0].w !== W || this.layers[0].h !== H)) {
      for (const l of this.layers) this.dropTarget(l);
      this.layers = [];
    }
    while (this.layers.length < rec.layers) this.layers.push(this.target(W, H, gl.RGBA8));
    if (rec.scratchW > 0 && (!this.scratch || this.scratch.w < rec.scratchW || this.scratch.h < rec.scratchH)) {
      const sw = grow(rec.scratchW, this.scratch?.w ?? 0, this.max);
      const sh = grow(rec.scratchH, this.scratch?.h ?? 0, this.max);
      if (this.scratch) this.dropTarget(this.scratch);
      this.scratch = this.target(sw, sh, gl.R8);
    }
    if (this.scratch) fillResolve(rec, this.scratch.w, this.scratch.h);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.qvbo);
    gl.bufferData(gl.ARRAY_BUFFER, rec.quads.subarray(0, rec.nquad * QUAD_FLOATS), gl.STREAM_DRAW);
    gl.bindVertexArray(this.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.vbo);
    gl.bufferData(gl.ARRAY_BUFFER, rec.verts.subarray(0, rec.nvert * 4), gl.STREAM_DRAW);
    let quadsBound = false;
    gl.bindBuffer(gl.UNIFORM_BUFFER, this.ubo);
    gl.bufferData(gl.UNIFORM_BUFFER, rec.unis.subarray(0, Math.max(rec.nslot, 1) * rec.stride), gl.STREAM_DRAW);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.enable(gl.STENCIL_TEST);
    gl.enable(gl.SCISSOR_TEST);
    gl.disable(gl.CULL_FACE);
    gl.disable(gl.DEPTH_TEST);
    gl.activeTexture(gl.TEXTURE0);
    const exts = rec.externals.size ? this.externals(rec) : null;
    const names = ["quad", "quadbox", "resolve", "flat", "flat", "flat", "flat", "flat", "flat", "quad"];
    let flip = 1, th = H, prog = null;
    const c = rec.cmds;
    const bg = rec.bg;
    for (let i = 0; i < rec.ncmd; i++) {
      const o = i * CMD;
      if (c[o] === C_OPEN) {
        const target = c[o + 1], mode = c[o + 2], vw = c[o + 3], vh = c[o + 4];
        const t = target === T_FRAME ? null : target === T_SCRATCH ? this.scratch : this.layers[target];
        gl.bindFramebuffer(gl.FRAMEBUFFER, t ? t.fbo : null);
        gl.viewport(0, 0, vw, vh);
        flip = t ? -1 : 1;
        th = H;
        if (mode !== KEEP) {
          const cc = t ? [0, 0, 0, 0] : bg;
          // The scratch: only the part this path uses.
          if (target === T_SCRATCH) gl.scissor(0, 0, vw, vh);
          else gl.disable(gl.SCISSOR_TEST);
          gl.colorMask(true, true, true, true);
          gl.stencilMask(0xff);
          gl.clearColor(cc[0], cc[1], cc[2], cc[3]);
          gl.clearStencil(0);
          gl.clear(mode === CLEAR ? gl.COLOR_BUFFER_BIT | gl.STENCIL_BUFFER_BIT : gl.STENCIL_BUFFER_BIT);
          gl.enable(gl.SCISSOR_TEST);
        }
        continue;
      }
      let p = c[o + 1];
      const slot = c[o + 2], tex = c[o + 3];
      if (TEXTURED[p]) {
        const t = p === P_QUAD_EXT ? exts?.get(extOf(tex))
          : tex === TEX_SCRATCH ? this.scratch?.tex : tex < 0 ? this.layers[-tex - 1]?.tex : this.textures.get(tex);
        if (t) {
          gl.bindTexture(gl.TEXTURE_2D, t);
          gl.bindSampler(0, (p === P_QUAD || p === P_QUAD_EXT) && c[o + 11] ? this.near : this.lin);
        } else if (p === P_QUAD || p === P_QUAD_EXT) {
          p = P_QUADBOX; // its boxes still draw
        } else {
          continue;
        }
      }
      if (QUAD[p] !== quadsBound) {
        quadsBound = QUAD[p];
        gl.bindVertexArray(quadsBound ? this.qvao : this.vao);
      }
      const pr = this.programs[names[p]];
      if (pr !== prog) {
        gl.useProgram(pr.p);
        prog = pr;
      }
      if (pr.flipped !== flip) {
        gl.uniform1f(pr.flip, flip);
        pr.flipped = flip;
      }
      this.state(p, c[o + 10]);
      const sx = c[o + 6], sy = c[o + 7], sw = c[o + 8], sh = c[o + 9];
      gl.scissor(sx, flip > 0 ? th - (sy + sh) : sy, sw, sh);
      gl.bindBufferRange(gl.UNIFORM_BUFFER, 0, this.ubo, slot * rec.stride * 4, SLOT_BYTES);
      gl.drawArrays(gl.TRIANGLES, c[o + 4], c[o + 5]);
    }
    gl.bindSampler(0, null);
    gl.bindVertexArray(null);
    return 0;
  }

  finish() {
    // Reading a pixel waits for the GPU (finish() alone may not).
    const gl = this.gl;
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(4));
    return Promise.resolve();
  }

  // Lose the context, which the browser then restores (as it does after a
  // real loss, the default prevented).
  lose() {
    const ext = this.gl.getExtension("WEBGL_lose_context");
    ext?.loseContext();
    setTimeout(() => ext?.restoreContext(), 50);
  }

  destroy() {
    this.destroyed = true;
    this.canvas.removeEventListener("webglcontextlost", this.onLost);
  }
}

// ---- Choosing and setting up -------------------------------------------------

// `want`: 0 WebGPU, then WebGL2; 1 WebGPU; 2 WebGL2.
// `claim(fresh)` hands over the canvas; `fresh` asks for a new one (a WebGPU
// attempt took the old one's context).
async function createRenderer(claim, want, log) {
  let taken = false;
  if (want !== 2) {
    try {
      const r = await createWebGpu(() => {
        taken = true;
        return claim(false);
      }, log, want === 1);
      if (r) return r;
    } catch (err) {
      log(`stipple: WebGPU couldn't be set up: ${err.message ?? err}`);
    }
    if (want === 1) return null;
  }
  try {
    return createWebGl2(claim(taken), log);
  } catch (err) {
    log(`stipple: WebGL2 couldn't be set up: ${err.message ?? err}`);
    return null;
  }
}

const API_CODE = { webgpu: 1, webgl2: 2 };

// The module's gpu_* imports over a canvas (see the top of this file).
export function gpuHost({ memory, claim, current, redraw, log = console.log, onFrame, onRelease,
  source = () => null, onUpload }) {
  let gpu = null;
  let state = "idle"; // idle | pending | ready | failed
  let failedWant = -1;
  // Where the last frame's op buffer is in the module's memory (the module
  // keeps it until the next frame), for a snapshot.
  let last = null;
  const lastWords = () => last && new Uint32Array(memory().buffer, last[0], last[1]);

  const start = (want) => {
    state = "pending";
    const prev = gpu;
    if (gpu) {
      gpu.destroy();
      gpu = null;
    }
    (async () => {
      // A lost WebGL2 context comes back on the same canvas, if at all:
      // give the browser a moment to restore it.
      if (prev?.api === "webgl2" && prev.gl.isContextLost() && prev.canvas === current()) {
        await new Promise((resolve) => {
          const t = setTimeout(resolve, 3000);
          prev.canvas.addEventListener("webglcontextrestored", () => {
            clearTimeout(t);
            resolve();
          }, { once: true });
        });
      }
      let r = null;
      try {
        r = await createRenderer(claim, want, log);
      } catch (err) {
        log(`stipple: no GPU renderer: ${err.message ?? err}`);
      }
      gpu = r;
      if (r) {
        r.onBroken = redraw;
        r.source = source;
      }
      state = r ? "ready" : "failed";
      failedWant = r ? -1 : want;
      redraw();
    })();
  };

  const imports = {
    gpu_init(want) {
      if (state === "pending") return 0;
      if (state === "ready" && gpu && !gpu.lost && gpu.canvas === current() &&
        (want === 0 || API_CODE[gpu.api] === want)) {
        return API_CODE[gpu.api];
      }
      if (state === "failed" && failedWant === want) return 3;
      start(want);
      return 0;
    },
    gpu_texture(id, ptr, w, h) {
      if (!gpu) return 0;
      onUpload?.();
      return gpu.texture(id, new Uint8Array(memory().buffer, ptr, w * h * 4), w, h) ? 1 : 0;
    },
    gpu_texture_sub(id, ptr, x, y, w, h) {
      if (!gpu) return 0;
      onUpload?.();
      return gpu.textureSub(id, new Uint8Array(memory().buffer, ptr, w * h * 4), x, y, w, h) ? 1 : 0;
    },
    gpu_free(id) {
      gpu?.free(id);
    },
    gpu_frame(ptr, words) {
      if (state === "pending") return 2;
      if (!gpu) return 3;
      if (gpu.lost) return 1;
      if (gpu.canvas !== current()) {
        // The canvas went back to the 2D context: set up anew.
        gpu.lost = true;
        return 1;
      }
      last = [ptr, words];
      let status;
      try {
        status = gpu.frame(lastWords());
      } catch (err) {
        log(`stipple: the GPU frame failed: ${err.message ?? err}`);
        status = 3;
      }
      if (status === 0) onFrame?.(gpu.api);
      return status;
    },
    gpu_release() {
      onRelease?.();
    },
  };

  return {
    imports,
    // What draws: "webgpu", "webgl2", or null.
    get api() {
      return state === "ready" && gpu && !gpu.lost ? gpu.api : null;
    },
    // A 2D canvas holding the last frame drawn (drawn again, in this task,
    // so the GPU canvas's contents are still there to copy), or null.
    snapshot() {
      if (!gpu || gpu.lost || !last || gpu.frame(lastWords()) !== 0) return null;
      const out = document.createElement("canvas");
      out.width = gpu.canvas.width;
      out.height = gpu.canvas.height;
      out.getContext("2d").drawImage(gpu.canvas, 0, 0);
      return out;
    },
    // The page let go of external source `id`: free what was made of it.
    releaseExternal(id) {
      gpu?.freeExternal(id);
    },
    // Resolves once the GPU has done its work (timing).
    finish() {
      return gpu ? gpu.finish() : Promise.resolve();
    },
    // Lose the device (tests): the app falls back as it would on a real loss.
    lose() {
      gpu?.lose();
    },
    // Set up for `want` now (0 either, 1 WebGPU, 2 WebGL2); resolves with the
    // API, or null (the comparison page).
    async init(want) {
      if (imports.gpu_init(want) !== 0) return this.api;
      while (state === "pending") await new Promise((r) => setTimeout(r, 10));
      return this.api;
    },
  };
}
