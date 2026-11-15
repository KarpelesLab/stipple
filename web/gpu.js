// gpu.js: Stipple's GPU renderer in the browser, for web/stipple.js.
//
// A module built with `--features stipple/gpu` lowers each frame's scene
// (src/gpu/ops.rs) and hands it over as a compact op buffer (the format is in
// src/gpu/web.rs): boxes, path fills, images, clips and layers, every draw
// with its vertices already in device pixels. This file draws it into the
// app's canvas with WebGPU (`canvas.getContext("webgpu")`, WGSL shaders) or,
// where the browser has no WebGPU, WebGL2 (GLSL ES 3.0), the same way the
// Metal executor does natively (src/gpu/metal_scene.rs): analytic boxes,
// paths by stencil-and-cover at 4x4 supersampling in a scratch target,
// textured quads for images and glyphs, scissor and stencil clips, and
// layers in offscreen targets composited at their opacity. Everything is
// blended in premultiplied alpha on the encoded sRGB values, as the software
// renderer blends. No read-back: the canvas shows what was drawn.
//
// The frame is first *recorded* (shared by both APIs: the clip, layer and
// path logic) into a list of render-target switches and draws plus one array
// of vertices and one of uniforms, then *replayed* by the API's backend with
// one upload of each.
//
// A frame can be partial (the header's flags): it redraws only what changed
// (its ops are clipped to that) over the last frame, so the last frame is
// kept: in a preserved drawing buffer on WebGL2, in a texture of the
// renderer's own on WebGPU (the canvas's texture is new each frame), which is
// copied into the canvas's. Without a last frame to draw over, `frame`
// answers 4 and the module draws the next one whole.
//
//   const host = gpuHost({ memory, claim, current, redraw, log });
//   imports.stipple = { ...host.imports, ... };
//
// `claim(fresh)` returns the canvas to take a GPU context on (stipple.js gives
// a fresh one if the 2D context has it, or `fresh` asks for one), `current()` the canvas the app is
// shown in now, `redraw()` asks the app for a whole frame (the GPU came up,
// asynchronously), `memory()` is the module's memory.

// ---- The op buffer (src/gpu/web.rs) -----------------------------------------

const OP_BOX = 1, OP_PATH = 2, OP_IMAGE = 3, OP_CLIP_RECT = 4, OP_CLIP_POLY = 5,
  OP_POP_CLIP = 6, OP_LAYER = 7, OP_POP_LAYER = 8;

// What a recorded draw uses: its pipeline (shader, blending, stencil state).
const P_BOX = 0, P_TEX = 1, P_RESOLVE = 2, P_CLIP_INC = 3, P_CLIP_DEC = 4,
  P_WIND_NZ = 5, P_WIND_EO = 6, P_COVER_NZ = 7, P_COVER_EO = 8;
const TEXTURED = [false, true, true, false, false, false, false, false, false];

// Render targets: the frame, a layer (0, 1, ...), the path scratch.
const T_FRAME = -1, T_SCRATCH = -2;
// A draw's texture: an image's id (1...), a layer, or the scratch.
const TEX_SCRATCH = -1000000;
const layerTex = (i) => -(i + 1);

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
    this.nslot = 0;
    this.layers = 0;
    this.scratchW = 0;
    this.scratchH = 0;
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
  const draw = (pipe, slot, tex, first, count, s, ref) =>
    rec.cmd(C_DRAW, pipe, slot, tex, first, count, s[0], s[1], s[2] - s[0], s[3] - s[1], ref);
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
    const slot = rec.slot(xfFrame, [0, 0, W, H], WHITE, [0, 0, 1, 1], [0, 0, 1, 1],
      [0, 0, layer.opacity, 1], [1, 1, 0, 0]);
    const first = rec.quad(0, 0, W, H, (x, y) => [x, y]);
    draw(P_TEX, slot, layerTex(layer.layer), first, 6, parent.scissor, parent.depth);
  };

  open(T_FRAME, partial ? CLEAR_STENCIL : CLEAR, W, H);
  let p = 7;
  while (p < u.length) {
    const op = u[p++];
    const level = levels[levels.length - 1];
    switch (op) {
      case OP_BOX: {
        const rect = f.subarray(p, p + 4);
        const radius = f[p + 4], stroke = f[p + 5];
        const color = f.subarray(p + 6, p + 10);
        const n = u[p + 10];
        const at = p + 11;
        p = at + n * 4;
        if (!bind(level)) break;
        const slot = rec.slot(xfFrame, rect, color, null, null, [radius, stroke, 0, 0]);
        draw(P_BOX, slot, 0, rec.copy(f, at, n), n, level.scissor, level.depth);
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
      case OP_IMAGE: {
        const id = u[p];
        const rect = f.subarray(p + 1, p + 5);
        const px = [f[p + 5], f[p + 6]];
        const opacity = f[p + 7];
        const uv = f.subarray(p + 8, p + 12);
        const clamp = f.subarray(p + 12, p + 16);
        const tint = f.subarray(p + 16, p + 20);
        const nearest = u[p + 20];
        const n = u[p + 21];
        const at = p + 22;
        p = at + n * 4;
        if (!bind(level)) break;
        const slot = rec.slot(xfFrame, rect, tint, uv, clamp, [0, 0, opacity, 0], [px[0], px[1], 1, nearest]);
        draw(P_TEX, slot, id, rec.copy(f, at, n), n, level.scissor, level.depth);
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

// A box: aux is the device position. Coverage is the exact overlap of the
// pixel with a sharp box (or ring), or a signed distance for rounded corners.
// p0: radius, stroke.
fn span(p: f32, h: f32) -> f32 { return max(min(h, p + 0.5) - max(-h, p - 0.5), 0.0); }
fn boxcov(p: vec2f, h0: vec2f) -> f32 { let h = max(h0, vec2f(0.0)); return span(p.x, h.x) * span(p.y, h.y); }
fn sdf(p: vec2f, h: vec2f, r: f32) -> f32 {
  let d = abs(p) - (h - vec2f(r));
  return length(max(d, vec2f(0.0))) + min(max(d.x, d.y), 0.0) - r;
}
@fragment fn box_fs(i: VOut) -> @location(0) vec4f {
  let c = (u.rect.xy + u.rect.zw) * 0.5;
  let h = (u.rect.zw - u.rect.xy) * 0.5;
  let p = i.aux - c;
  let radius = u.p0.x;
  let stroke = u.p0.y;
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
  return u.color * cov;
}

// A textured quad: aux is the local position inside rect; uv the part of the
// texture spread over it, kept within clampr; color the (premultiplied) tint.
// p0: -, -, opacity, premultiplied texels; p1: device pixels per local unit
// along x and y (for edge coverage), antialiased edges, nearest.
@fragment fn tex_fs(i: VOut) -> @location(0) vec4f {
  let f = (i.aux - u.rect.xy) / (u.rect.zw - u.rect.xy);
  let uv = clamp(mix(u.uv.xy, u.uv.zw, f), u.clampr.xy, u.clampr.zw);
  let a = textureSample(t, lin, uv);
  let b = textureSample(t, near, uv);
  var c = select(a, b, u.p1.w > 0.5);
  if (u.p0.w < 0.5) { c = vec4f(c.rgb * c.a, c.a); }
  c = c * u.color;
  var cov = 1.0;
  if (u.p1.z > 0.5) {
    let d = min(i.aux - u.rect.xy, u.rect.zw - i.aux) * u.p1.xy;
    cov = clamp(d.x + 0.5, 0.0, 1.0) * clamp(d.y + 0.5, 0.0, 1.0);
  }
  return c * (u.p0.z * cov);
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

// `claim()` hands over the canvas, at the last moment: a canvas that gave a
// WebGPU context can't give a WebGL2 one.
async function createWebGpu(claim, log) {
  if (!navigator.gpu) return null;
  const adapter = await navigator.gpu.requestAdapter({ powerPreference: "high-performance" });
  if (!adapter) return null;
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
  const plU = device.createPipelineLayout({ bindGroupLayouts: [layoutU] });
  const plT = device.createPipelineLayout({ bindGroupLayouts: [layoutT] });
  const blend = { color: { srcFactor: "one", dstFactor: "one-minus-src-alpha" },
    alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha" } };
  const face = (compare, passOp) => ({ compare, passOp, failOp: "keep", depthFailOp: "keep" });
  const pipeline = (fs, layout, target, { blended = false, write = true, front, back = front, read = 0xff, writeMask = 0 }) =>
    device.createRenderPipeline({
      layout,
      vertex: { module, entryPoint: "vs", buffers: [{ arrayStride: 16, attributes: [
        { shaderLocation: 0, offset: 0, format: "float32x2" },
        { shaderLocation: 1, offset: 8, format: "float32x2" }] }] },
      fragment: { module, entryPoint: fs, targets: [{ format: target,
        blend: blended ? blend : undefined, writeMask: write ? GPUColorWrite.ALL : 0 }] },
      primitive: { topology: "triangle-list", cullMode: "none" },
      depthStencil: { format: "stencil8", depthWriteEnabled: false, depthCompare: "always",
        stencilFront: front, stencilBack: back, stencilReadMask: read, stencilWriteMask: writeMask },
    });
  const test = face("equal", "keep");
  const pipes = [];
  pipes[P_BOX] = pipeline("box_fs", plU, format, { blended: true, front: test });
  pipes[P_TEX] = pipeline("tex_fs", plT, format, { blended: true, front: test });
  pipes[P_RESOLVE] = pipeline("resolve_fs", plT, format, { blended: true, front: test });
  pipes[P_CLIP_INC] = pipeline("flat_fs", plU, format, { write: false, front: face("equal", "increment-clamp"), writeMask: 0xff });
  pipes[P_CLIP_DEC] = pipeline("flat_fs", plU, format, { write: false, front: face("equal", "decrement-clamp"), writeMask: 0xff });
  pipes[P_WIND_NZ] = pipeline("flat_fs", plU, "r8unorm", { write: false,
    front: face("always", "increment-wrap"), back: face("always", "decrement-wrap"), writeMask: 0xff });
  pipes[P_WIND_EO] = pipeline("flat_fs", plU, "r8unorm", { write: false, front: face("always", "invert"), writeMask: 0xff });
  pipes[P_COVER_NZ] = pipeline("flat_fs", plU, "r8unorm", { front: face("not-equal", "keep") });
  pipes[P_COVER_EO] = pipeline("flat_fs", plU, "r8unorm", { front: face("not-equal", "keep"), read: 1 });
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
  return new WebGpuRenderer(canvas, device, ctx, format, pipes, layoutU, layoutT, log);
}

class WebGpuRenderer {
  constructor(canvas, device, ctx, format, pipes, layoutU, layoutT, log) {
    this.api = "webgpu";
    this.canvas = canvas;
    this.device = device;
    this.ctx = ctx;
    this.format = format;
    this.pipes = pipes;
    this.layoutU = layoutU;
    this.layoutT = layoutT;
    this.max = device.limits.maxTextureDimension2D;
    this.stride = Math.max(256, device.limits.minUniformBufferOffsetAlignment) / 4;
    this.textures = new Map(); // id -> { tex, view, bg, gen }
    this.layers = []; // { tex, view, stencil, bg, gen }
    this.scratch = null;
    this.frameStencil = null;
    // The frame target, which keeps the last frame (see `frame`).
    this.frameTex = null;
    this.vbuf = null;
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
    dev.queue.writeBuffer(this.ubuf, 0, rec.unis.buffer, 0, Math.max(rec.nslot, 1) * rec.stride * 4);
    const frameView = this.frameTex.view;
    const enc = dev.createCommandEncoder();
    let pass = null;
    let pipe = -1;
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
        pipe = -1;
        continue;
      }
      // C_DRAW
      const p = c[o + 1], slot = c[o + 2], tex = c[o + 3];
      let group = this.bgU;
      if (TEXTURED[p]) {
        const t = tex === TEX_SCRATCH ? this.scratch : tex < 0 ? this.layers[-tex - 1] : this.textures.get(tex);
        if (!t) continue;
        group = this.bindGroup(t);
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

// The fragment shaders: the WGSL ones above, in GLSL ES 3.0.
const GLSL_FS_HEAD = `#version 300 es
precision highp float;
layout(std140) uniform U { vec4 xf; vec4 rect; vec4 color; vec4 uv; vec4 clampr; vec4 p0; vec4 p1; vec4 p2; };
uniform sampler2D u_tex;
in vec2 v_aux;
out vec4 o;
`;

const GLSL_FS = {
  box: `
float span(float p, float h) { return max(min(h, p + 0.5) - max(-h, p - 0.5), 0.0); }
float boxcov(vec2 p, vec2 h) { h = max(h, vec2(0.0)); return span(p.x, h.x) * span(p.y, h.y); }
float sdf(vec2 p, vec2 h, float r) {
  vec2 d = abs(p) - (h - vec2(r));
  return length(max(d, vec2(0.0))) + min(max(d.x, d.y), 0.0) - r;
}
void main() {
  vec2 c = (rect.xy + rect.zw) * 0.5;
  vec2 h = (rect.zw - rect.xy) * 0.5;
  vec2 p = v_aux - c;
  float radius = p0.x, stroke = p0.y, w = stroke * 0.5;
  float cov;
  if (radius <= 0.0) {
    cov = stroke > 0.0 ? boxcov(p, h + vec2(w)) - boxcov(p, h - vec2(w)) : boxcov(p, h);
  } else {
    float r = min(radius, min(h.x, h.y));
    float d = sdf(p, h, r);
    cov = stroke > 0.0 ? clamp(0.5 - (d - w), 0.0, 1.0) - clamp(0.5 - (d + w), 0.0, 1.0)
                       : clamp(0.5 - d, 0.0, 1.0);
  }
  o = color * cov;
}`,
  tex: `
void main() {
  vec2 f = (v_aux - rect.xy) / (rect.zw - rect.xy);
  vec2 st = clamp(mix(uv.xy, uv.zw, f), clampr.xy, clampr.zw);
  vec4 c = texture(u_tex, st);
  if (p0.w < 0.5) { c.rgb *= c.a; }
  c *= color;
  float cov = 1.0;
  if (p1.z > 0.5) {
    vec2 d = min(v_aux - rect.xy, rect.zw - v_aux) * p1.xy;
    cov = clamp(d.x + 0.5, 0.0, 1.0) * clamp(d.y + 0.5, 0.0, 1.0);
  }
  o = c * (p0.z * cov);
}`,
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
    for (const [name, body] of Object.entries(GLSL_FS)) {
      const p = gl.createProgram();
      gl.attachShader(p, vs);
      gl.attachShader(p, shader(gl.FRAGMENT_SHADER, GLSL_FS_HEAD + body));
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
      case P_BOX: case P_TEX: case P_RESOLVE:
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
    gl.bindVertexArray(this.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.vbo);
    gl.bufferData(gl.ARRAY_BUFFER, rec.verts.subarray(0, rec.nvert * 4), gl.STREAM_DRAW);
    gl.bindBuffer(gl.UNIFORM_BUFFER, this.ubo);
    gl.bufferData(gl.UNIFORM_BUFFER, rec.unis.subarray(0, Math.max(rec.nslot, 1) * rec.stride), gl.STREAM_DRAW);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.enable(gl.STENCIL_TEST);
    gl.enable(gl.SCISSOR_TEST);
    gl.disable(gl.CULL_FACE);
    gl.disable(gl.DEPTH_TEST);
    gl.activeTexture(gl.TEXTURE0);
    const names = ["box", "tex", "resolve", "flat", "flat", "flat", "flat", "flat", "flat"];
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
      const p = c[o + 1], slot = c[o + 2], tex = c[o + 3];
      if (TEXTURED[p]) {
        const t = tex === TEX_SCRATCH ? this.scratch?.tex : tex < 0 ? this.layers[-tex - 1]?.tex : this.textures.get(tex);
        if (!t) continue;
        gl.bindTexture(gl.TEXTURE_2D, t);
        const nearest = p === P_TEX && rec.unis[slot * rec.stride + 27] > 0.5;
        gl.bindSampler(0, nearest ? this.near : this.lin);
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
      }, log);
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
export function gpuHost({ memory, claim, current, redraw, log = console.log, onFrame, onRelease }) {
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
      if (r) r.onBroken = redraw;
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
      return gpu.texture(id, new Uint8Array(memory().buffer, ptr, w * h * 4), w, h) ? 1 : 0;
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
