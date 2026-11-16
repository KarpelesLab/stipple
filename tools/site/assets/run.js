// Runs one example: the page's <canvas> gets the example's wasm module
// through stipple.js. `?embed` shows only the canvas (guides embed the page
// in an iframe); `?check` is for tools/site/check.py: after the app has drawn,
// it posts what it saw (and the canvas as a PNG, the gallery's thumbnail) to
// the checker's server. `?renderer=gpu` (or webgpu, webgl2, software) is
// stipple.js's: what draws the frames.
import { start } from "../stipple.js";

const script = document.currentScript ?? document.querySelector("script[data-wasm]");
const { name, wasm, font, assets, preload } = script.dataset;
const params = new URLSearchParams(location.search);
const embed = params.has("embed");
const check = params.has("check");
if (embed) document.body.classList.add("embed");

const canvas = document.getElementById("app");
const status = document.querySelector(".status");
const errors = [];
const origError = console.error;
console.error = (...args) => {
  errors.push(args.map(String).join(" "));
  origError.apply(console, args);
};
window.addEventListener("error", (e) => errors.push(String(e.message)));
window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));

function fail(message) {
  status.textContent = message;
  status.classList.add("error");
  status.hidden = false;
}

let app = null;
try {
  // title: false keeps the page's title; stateKey: false starts each visit
  // afresh (the gallery's thumbnails, and readers, see the example as it
  // starts).
  // assets/preload: the example's packaged files (examples/NAME/assets/),
  // preloaded so its blocking reads find them.
  app = await start(canvas, wasm, { font, title: false, stateKey: false, assets, preload });
  status.hidden = true;
  if (!embed) canvas.focus();
} catch (e) {
  fail(`This example didn't start: ${e?.message ?? e}`);
}
// A panic reaches the console; say so on the page too.
const watch = setInterval(() => {
  const panic = errors.find((e) => e.startsWith("stipple: panicked"));
  if (panic) {
    fail(`The example stopped: ${panic.replace(/^stipple: /, "")}`);
    clearInterval(watch);
  }
}, 500);

if (check) {
  // Wait for the first frames, then let animations settle a little.
  const t0 = performance.now();
  while ((app?.stats?.presents ?? 0) === 0 && performance.now() - t0 < 8000) {
    await new Promise((r) => setTimeout(r, 100));
  }
  // Half the settling time to warm up, then the texture uploads over the
  // other half: an animation on the GPU that uploads nothing a frame (a
  // game drawn as a resampled layer) shows none.
  const settle = Number(params.get("settle") ?? 1500);
  await new Promise((r) => setTimeout(r, settle / 2));
  const u0 = app?.stats?.uploads ?? 0, f0 = app?.stats?.presents ?? 0;
  await new Promise((r) => setTimeout(r, settle / 2));
  const uploads = (app?.stats?.uploads ?? 0) - u0, uploadFrames = (app?.stats?.presents ?? 0) - f0;
  // How many distinct colors a sample of the canvas has: a blank one has 1.
  let colors = 0;
  let png = null;
  try {
    // The last frame as a 2D canvas: on the GPU (?renderer=gpu) the canvas
    // holds a WebGPU or WebGL2 context, which can't be read back later.
    // A GPU canvas whose context was lost draws again once it's back.
    let shot = app?.snapshot?.();
    for (let i = 0; !shot && app?.renderer && app.renderer !== "software" && i < 50; i++) {
      await new Promise((r) => setTimeout(r, 100));
      shot = app.snapshot();
    }
    if (!shot && app?.renderer && app.renderer !== "software") {
      throw new Error(`no snapshot of the ${app.renderer} canvas (its context lost?)`);
    }
    shot ??= canvas;
    const ctx = shot.getContext("2d");
    const { width, height } = shot;
    const data = ctx.getImageData(0, 0, width, height).data;
    const seen = new Set();
    const step = Math.max(1, Math.floor((width * height) / 40000));
    for (let i = 0; i < width * height; i += step) {
      const o = i * 4;
      seen.add((data[o] << 16) | (data[o + 1] << 8) | data[o + 2]);
      if (seen.size > 1000) break;
    }
    colors = seen.size;
    png = shot.toDataURL("image/png");
  } catch (e) {
    errors.push(`reading the canvas: ${e}`);
  }
  // Run again with ?renderer=, the report is NAME@RENDERER.
  const asked = params.get("renderer");
  const report = {
    name: asked ? `${name}@${asked}` : name,
    started: !!app,
    presents: app?.stats?.presents ?? 0,
    renderer: app?.renderer ?? null,
    colors,
    uploads,
    uploadFrames,
    errors,
    png,
  };
  await fetch("/__report", { method: "POST", body: JSON.stringify(report) }).catch(() => {});
}
