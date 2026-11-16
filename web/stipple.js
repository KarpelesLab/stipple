// stipple.js: the page side of a Stipple web app.
//
// A Stipple app built for wasm32-unknown-unknown with `stipple::web_app!`
// exports a small C ABI (`stipple_*`) and imports a few functions from the
// module "stipple". This file provides those imports and forwards DOM events
// to the exports. No wasm-bindgen, no dependencies. See docs/platforms/web.md.
//
//   import { start } from "./stipple.js";
//   const app = await start(document.querySelector("canvas"), "app.wasm", {
//     font: "font.ttf", // a TrueType/OpenType font to render text with
//   });
//
// Give the canvas a CSS size (say `width: 100vw; height: 100vh`); the app
// fills it at the device's pixel ratio and follows it as it changes. A canvas
// sized only by its width/height attributes keeps that size.
//
// Each call runs its own instance of the module, with its own memory, so any
// number of canvases on a page each run an app (the same module or different
// ones); a module is fetched and compiled once.
//
// A module built with `--features stipple/gpu` can draw on the GPU instead
// of the CPU: WebGPU, or WebGL2 where the browser has no WebGPU, through
// gpu.js (served next to this file). The canvas then holds a WebGPU or
// WebGL2 context rather than a 2D one; if the app falls back to the CPU (a
// GPU lost twice), the canvas is swapped for a fresh one, same attributes
// and listeners, for the 2D context.
//
// Next to the canvas, `start` adds an invisible overlay (`.stipple-overlay`)
// that mirrors the app's accessibility tree as ARIA elements for screen
// readers, and holds the hidden <textarea> that has keyboard focus while the
// app does, so input methods and dead keys work.
//
// Options:
//   font      URL of the font to load (the web has no system font; without
//             one, text isn't drawn and the console says so), or a list of
//             URLs: the first is the app's font, the others its fallbacks in
//             order (a CJK or emoji font, say). Any TrueType or OpenType
//             font, CFF (.otf) and collections (.ttc) included.
//   label     the accessible name of the app (default: the canvas's
//             aria-label, if any).
//   title     false to leave document.title alone (for a page that runs
//             several apps); by default the app's title becomes the page's.
//   pickFile  async (accept) => File | null, in place of the <input type=file>
//             picker (tests use it).
//   saveFile  (name, blob) => void, in place of downloading the blob.
//   openUrl   (url) => void, in place of window.open (stipple::platform::open_url).
//   notify    (title, body) => void, in place of the Notification API
//             (stipple::platform::notify).
//   share     async ({ title, text, url, files }) => void, in place of
//             navigator.share (stipple::platform::share): resolving means
//             shared, an AbortError cancelled, anything else unavailable.
//   stateKey  the sessionStorage key the app's saved state is kept under
//             (App::save_state / restore_state); by default one per module
//             URL and canvas order on the page. false keeps no state.
//   assets    the URL the app's packaged files are under
//             (stipple::platform::assets), relative to the page; default
//             "assets/". Each read fetches `assets + path`.
//   preload   the packaged files to fetch before the app starts, so its
//             blocking reads (assets().read, .image, ...) find them: an
//             array of paths, or the URL of a JSON array of them (a
//             manifest). Others load with Assets::load / AppHandle::load_asset.
//   renderer  what draws the frames, overriding the app's App::renderer as
//             STIPPLE_RENDERER does natively: "auto" (the default: the
//             app's choice, where Renderer::Auto is the GPU when the
//             browser has a hardware one, else the CPU), "software", "gpu" (WebGPU, else WebGL2, else
//             the CPU), "webgpu" or "webgl2". The page's `?renderer=` query
//             parameter overrides this option. The GPU ones need a module
//             built with the `gpu` feature.
//
// Returns { exports, stats, canvas, overlay, input, colorScheme, saveState,
// renderer, snapshot, gpu }: `stats` counts presents (CPU or GPU frames),
// animation frames, timer wakes and accessibility-tree updates, for tests;
// `canvas` is the canvas the app is in now, `renderer` what drew the last
// frame ("software", "webgpu" or "webgl2"), `snapshot()` a 2D canvas copy
// of the last frame (a GPU canvas can't be read back later), `gpu` the GPU
// host (gpu.js; `gpu.lose()` loses the device, for tests); `overlay` is the
// ARIA overlay, `input` the hidden
// textarea, `colorScheme()` hands the page's current color scheme to the
// app again (it already follows changes on its own), and `saveState()` saves
// the app's state now (it already does when the page hides or unloads).

import { gpuHost } from "./gpu.js";

const SHIFT = 1, CTRL = 2, ALT = 4, META = 8;

// Compiled modules by URL, so several canvases running the same app compile
// it once. Each still gets its own instance (and memory).
const modules = new Map();
let instances = 0;

// Whether the browser draws WebGL (and so WebGPU) on a hardware GPU rather
// than a software rasterizer (SwiftShader, llvmpipe), which is slower than
// the module's own: `renderer: "auto"` then draws on the GPU (in a module
// built with it). A context that would be slow isn't handed out at all with
// `failIfMajorPerformanceCaveat`; the renderer's name catches the rest.
// Asked once per page.
let hardwareGpu = null;
function hasHardwareGpu() {
  if (hardwareGpu !== null) return hardwareGpu;
  hardwareGpu = false;
  try {
    const gl = document.createElement("canvas")
      .getContext("webgl2", { failIfMajorPerformanceCaveat: true });
    if (gl) {
      const info = gl.getExtension("WEBGL_debug_renderer_info");
      const name = String(gl.getParameter(info ? info.UNMASKED_RENDERER_WEBGL : gl.RENDERER));
      hardwareGpu = !/swiftshader|llvmpipe|softpipe|lavapipe|software|basic render/i.test(name);
      gl.getExtension("WEBGL_lose_context")?.loseContext();
    }
  } catch (_) { /* no WebGL2: the CPU */ }
  return hardwareGpu;
}

// Bytes as base64 (Web Storage keeps strings).
function toBase64(bytes) {
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    bin += String.fromCharCode(...bytes.subarray(i, Math.min(i + 0x8000, bytes.length)));
  }
  return btoa(bin);
}

function compile(url) {
  let module = modules.get(url);
  if (!module) {
    module = (async () => {
      try {
        return await WebAssembly.compileStreaming(fetch(url));
      } catch (_) {
        // Served without `application/wasm`: compile from the bytes instead.
        return WebAssembly.compile(await (await fetch(url)).arrayBuffer());
      }
    })();
    module.catch(() => modules.delete(url));
    modules.set(url, module);
  }
  return module;
}

// Roles a screen reader can move focus to; focusing a text box focuses the
// app's field.
const FOCUSABLE = new Set([
  "button", "textbox", "checkbox", "radio", "switch", "combobox", "spinbutton", "option",
  "listbox", "grid", "row", "gridcell", "columnheader", "tree", "treeitem", "menuitem",
  "menubar", "menu", "separator",
]);
const TEXT_ROLES = new Set(["textbox", "spinbutton"]);

// ---- Audio (stipple::audio) ---------------------------------------------------
//
// The module has no threads, so the page pulls the mix: `audio_open` makes an
// AudioContext with a ScriptProcessorNode whose callback asks the module's
// `stipple_audio_render(frames)` for each buffer of interleaved stereo f32 and
// splits it into the two channels. Browsers keep a context suspended until the
// user interacts with the page (the autoplay policy), so the first click, tap
// or key press anywhere resumes it. Imported only by apps that use audio.
//
// `exports` returns the module's exports (they don't exist yet while the
// imports are being made).

const AUDIO_FRAMES = 1024; // per callback: about 21 ms at 48 kHz

function audioHost(exports) {
  let ctx = null;
  let paused = false;
  const resume = () => {
    if (ctx && !paused && ctx.state !== "running") ctx.resume().catch(() => {});
  };
  const imports = {
    audio_open() {
      const AC = globalThis.AudioContext || globalThis.webkitAudioContext;
      if (!AC) return 0;
      try {
        ctx = new AC({ latencyHint: "interactive" });
      } catch (_) {
        return 0;
      }
      const node = ctx.createScriptProcessor(AUDIO_FRAMES, 0, 2);
      node.onaudioprocess = (ev) => {
        const out = ev.outputBuffer;
        const left = out.getChannelData(0);
        const right = out.getChannelData(1);
        const e = exports();
        const ptr = e?.stipple_audio_render?.(out.length) ?? 0;
        if (!ptr) {
          left.fill(0);
          right.fill(0);
          return;
        }
        const mix = new Float32Array(e.memory.buffer, ptr, out.length * 2);
        for (let i = 0; i < out.length; i++) {
          left[i] = mix[2 * i];
          right[i] = mix[2 * i + 1];
        }
      };
      node.connect(ctx.destination);
      // The autoplay policy: start on the first user gesture.
      for (const type of ["pointerdown", "keydown", "touchend"]) {
        globalThis.addEventListener?.(type, resume, { capture: true });
      }
      resume();
      return ctx.sampleRate;
    },
    audio_pause(p) {
      paused = p !== 0;
      if (!ctx) return;
      if (paused) ctx.suspend().catch(() => {});
      else resume();
    },
  };
  return { imports };
}

// ---- End of audio ------------------------------------------------------------

export async function start(canvas, wasmUrl, options = {}) {
  // The canvas's context: the 2D one for CPU frames, taken at the first one;
  // or WebGPU / WebGL2 (gpu.js). A canvas keeps the first context it gives,
  // so going from one to the other swaps in a fresh canvas (swapCanvas).
  let ctx = null;
  let kind = null; // "2d" | "gpu"
  let blank = false; // the 2D canvas came back blank: the next frame goes whole
  let renderer = "software";
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  const stats = { presents: 0, frames: 0, wakes: 0, a11y: 0, gpuFrames: 0 };
  const prefix = `stipple${++instances}`;
  let e = null; // the module's exports
  let timer = null;
  // The app asked for a frame (request_frame), and whether a
  // requestAnimationFrame is out for it. Frames wait while the canvas is out
  // of view (scrolled away, or in an iframe scrolled off its page): nothing
  // would show them, and an animation there would cost as much as one in
  // view. The module hears of it too (stipple_visible), so its frame clock
  // stops rather than falling back to a timer; coming back into view, it
  // draws a whole frame and animations go on.
  let framePending = false;
  let rafQueued = false;
  let inView = true;
  let shown = true; // what the module was last told
  const runFrame = () => {
    rafQueued = false;
    if (!framePending) return;
    framePending = false;
    stats.frames++;
    e.stipple_frame();
  };
  const queueFrame = () => {
    if (rafQueued || !inView) return;
    rafQueued = true;
    requestAnimationFrame(runFrame);
  };
  const syncShown = () => {
    const now = inView && document.visibilityState !== "hidden";
    if (now === shown || !e?.stipple_visible) return;
    shown = now;
    e.stipple_visible(now ? 1 : 0);
  };
  const visibility = typeof IntersectionObserver === "function"
    ? new IntersectionObserver((entries) => {
      const last = entries[entries.length - 1];
      inView = last.isIntersecting || last.intersectionRatio > 0;
      syncShown();
      if (inView && framePending) queueFrame();
    })
    : null;
  // The screen wake lock the app wants (keep_awake), and the one held.
  let wantAwake = false;
  let wakeLock = null;
  const syncWake = () => {
    if (wantAwake && !wakeLock && document.visibilityState === "visible") {
      wakeLock = navigator.wakeLock?.request("screen").then((lock) => {
        lock.addEventListener("release", () => { wakeLock = null; });
        if (!wantAwake) lock.release();
        return lock;
      }).catch(() => { wakeLock = null; }) ?? null;
    } else if (!wantAwake && wakeLock) {
      wakeLock.then((lock) => lock?.release());
      wakeLock = null;
    }
  };
  // Permissions (permission_status): what the Permissions API last said,
  // by kind: 0 camera, 1 microphone, 2 notifications, 3 location. Codes: 0
  // granted, 1 denied, 2 not asked.
  const permCode = (state) => (state === "granted" ? 0 : state === "denied" ? 1 : 2);
  const permState = [2, 2, 2, 2];
  ["camera", "microphone", "notifications", "geolocation"].forEach((name, kind) => {
    try {
      navigator.permissions?.query({ name }).then((st) => {
        permState[kind] = permCode(st.state);
        st.addEventListener?.("change", () => { permState[kind] = permCode(st.state); });
      }, () => {});
    } catch (_) { /* not a name this browser knows */ }
  });
  // The cameras (camera_open): video inputs found, and those open by id.
  let cameraCount = 0xffffffff;
  navigator.mediaDevices?.enumerateDevices?.().then(
    (ds) => { cameraCount = ds.filter((d) => d.kind === "videoinput").length; },
    () => {},
  );
  const cameras = new Map();
  // Hand each new frame of an open camera to the module, as RGBA.
  const pumpCamera = (id, cam) => {
    const next = () => {
      if (cam.closed) return;
      if (cam.video.requestVideoFrameCallback) cam.video.requestVideoFrameCallback(grab);
      else requestAnimationFrame((now) => grab(now, null));
    };
    const grab = (now, meta) => {
      if (cam.closed) return;
      const v = cam.video;
      const w = v.videoWidth, h = v.videoHeight;
      if (!cam.paused && w && h && v.readyState >= 2 && e) {
        if (!cam.canvas || cam.canvas.width !== w || cam.canvas.height !== h) {
          cam.canvas = typeof OffscreenCanvas !== "undefined"
            ? new OffscreenCanvas(w, h)
            : Object.assign(document.createElement("canvas"), { width: w, height: h });
          cam.ctx = cam.canvas.getContext("2d", { willReadFrequently: true });
        }
        cam.ctx.drawImage(v, 0, 0, w, h);
        const [ptr, len] = pass(cam.ctx.getImageData(0, 0, w, h).data);
        e.stipple_camera_frame?.(id, ptr, len, w, h, meta?.mediaTime != null ? meta.mediaTime * 1000 : now);
      }
      next();
    };
    next();
  };
  let pendingCopy = null;
  let pendingHtml = null;
  let storageOut = new Uint8Array(0); // see storage_get

  const str = (ptr, len) => dec.decode(new Uint8Array(e.memory.buffer, ptr, len));
  // Copy bytes into memory the module allocates; the export that receives
  // them takes ownership.
  const pass = (bytes) => {
    const ptr = e.stipple_alloc(bytes.length);
    new Uint8Array(e.memory.buffer, ptr, bytes.length).set(bytes);
    return [ptr, bytes.length];
  };
  const passStr = (s) => pass(enc.encode(s));
  const mods = (ev) =>
    (ev.shiftKey ? SHIFT : 0) | (ev.ctrlKey ? CTRL : 0) | (ev.altKey ? ALT : 0) | (ev.metaKey ? META : 0);

  const pickFile = options.pickFile ?? ((accept) => new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    if (accept) input.accept = accept;
    input.addEventListener("change", () => resolve(input.files[0] ?? null));
    input.addEventListener("cancel", () => resolve(null));
    input.click();
  }));
  const saveFile = options.saveFile ?? ((name, blob) => {
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = name;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 10000);
  });

  // ---- The overlay: ARIA mirror of the accessibility tree, and the textarea.
  //
  // Fixed over the canvas and invisible (opacity 0, no pointer events), so
  // the canvas gets the mouse and the screen reader gets the elements, each
  // at its element's place.
  const overlay = document.createElement("div");
  overlay.className = "stipple-overlay";
  overlay.style.cssText =
    "position:fixed;overflow:hidden;opacity:0;pointer-events:none;margin:0;padding:0;border:0;";
  const label = options.label ?? canvas.getAttribute("aria-label");
  if (label) {
    overlay.setAttribute("role", "group");
    overlay.setAttribute("aria-label", label);
  }
  const root = document.createElement("div");
  root.style.cssText = "position:absolute;left:0;top:0;";
  const input = document.createElement("textarea");
  for (const [k, v] of [["autocomplete", "off"], ["autocorrect", "off"], ["autocapitalize", "off"],
    ["spellcheck", "false"], ["inputmode", "none"]]) input.setAttribute(k, v);
  input.tabIndex = -1;
  // The keyboard the focused field asks for, and whether one has focus.
  let inputMode = "text";
  let textInput = false;
  input.style.cssText =
    "position:absolute;left:0;top:0;width:1px;height:1px;margin:0;padding:0;border:0;" +
    "outline:none;resize:none;overflow:hidden;white-space:pre;font-size:16px;" +
    "color:transparent;background:transparent;caret-color:transparent;";
  overlay.append(root, input);
  canvas.after(overlay);
  // The overlay stands for the canvas's content.
  canvas.setAttribute("aria-hidden", "true");

  // The canvas's event listeners, kept to move them to a fresh canvas.
  const listeners = [];
  const listen = (type, fn, opts) => {
    listeners.push([type, fn, opts]);
    canvas.addEventListener(type, fn, opts);
  };
  // Put a fresh canvas in this one's place (same attributes, style and
  // listeners), for the other kind of context.
  let resizes = null;
  const swapCanvas = () => {
    const old = canvas;
    const fresh = old.cloneNode(false);
    for (const [type, fn, opts] of listeners) fresh.addEventListener(type, fn, opts);
    const focused = document.activeElement === old;
    old.replaceWith(fresh);
    canvas = fresh;
    ctx = null;
    kind = null;
    if (resizes) {
      resizes.unobserve(old);
      resizes.observe(fresh);
    }
    visibility?.unobserve(old);
    visibility?.observe(fresh);
    if (focused) fresh.focus({ preventScroll: true });
  };

  const place = () => {
    const r = canvas.getBoundingClientRect();
    const s = overlay.style;
    s.left = `${r.left}px`;
    s.top = `${r.top}px`;
    s.width = `${r.width}px`;
    s.height = `${r.height}px`;
  };

  // Focus moves the page makes itself, which shouldn't echo back to the app.
  let quiet = false;
  const focusQuietly = (el) => {
    if (document.activeElement === el) return;
    quiet = true;
    try { el.focus({ preventScroll: true }); } finally { quiet = false; }
  };
  const hasFocus = () => overlay.contains(document.activeElement);

  // Scroll the app's scroll container around `el` (the nearest overlay
  // element that is one) so that `el`'s node shows: the least movement, as
  // a wheel at the container's middle. A screen reader that moves focus to a
  // control scrolled out of view brings it back this way.
  const revealNode = (el) => {
    const b = el._b;
    let c = el.parentElement;
    while (c && c !== overlay && !c._sc) c = c.parentElement;
    if (!b || !c || !c._sc || !c._b) return;
    const across = c._sc[2];
    const [cx, cy, cw, ch] = c._b;
    const [v0, v1, t0, t1] = across
      ? [cx, cx + cw, b[0], b[0] + b[2]]
      : [cy, cy + ch, b[1], b[1] + b[3]];
    // How far the content moves: a wheel's pixels, the way it reveals
    // (right and down positive, a right-to-left strip too).
    const by = t0 < v0 ? t0 - v0 : t1 > v1 ? Math.min(t1 - v1, t0 - v0) : 0;
    if (Math.abs(by) < 0.5) return;
    e.stipple_pointer(0, 0, cx + cw / 2, cy + ch / 2, 0);
    if (across) e.stipple_wheel(by, 0, 0);
    else e.stipple_wheel(0, by, 0);
  };

  // A click at the middle of `el`'s node: how a screen reader's "activate"
  // reaches the element (its tap handler, or focus for a text field).
  const clickNode = (el) => {
    const b = el._b;
    if (!b) return;
    const x = b[0] + b[2] / 2, y = b[1] + b[3] / 2;
    e.stipple_pointer(0, 0, x, y, 0);
    e.stipple_pointer(1, 0, x, y, 0);
    e.stipple_pointer(2, 0, x, y, 0);
  };

  let ids = 0;
  const idOf = (el) => el.id || (el.id = `${prefix}-n${++ids}`);
  const attr = (el, name, value) => {
    if (value === null || value === undefined) {
      if (el.hasAttribute(name)) el.removeAttribute(name);
    } else if (el.getAttribute(name) !== String(value)) {
      el.setAttribute(name, value);
    }
  };

  // Bring `el` in line with `node`, relative to its parent's origin (px, py),
  // touching only what changed. Returns the focused node's element, if any.
  const sync = (el, node, px, py) => {
    const [x, y, w, h] = node.b;
    el._b = node.b;
    const s = el.style;
    const pos = `${x - px},${y - py},${w},${h}`;
    if (el._pos !== pos) {
      el._pos = pos;
      s.cssText = `position:absolute;overflow:hidden;white-space:pre;margin:0;padding:0;` +
        `left:${x - px}px;top:${y - py}px;width:${w}px;height:${h}px;`;
    }
    attr(el, "role", node.r || null);
    attr(el, "aria-selected", node.s === undefined ? null : node.s ? "true" : "false");
    attr(el, "aria-expanded", node.e === undefined ? null : node.e ? "true" : "false");
    attr(el, "aria-checked", node.c === undefined ? null : node.c ? "true" : "false");
    attr(el, "aria-placeholder", node.p ?? null);
    el._sc = node.sc ?? null;
    if (FOCUSABLE.has(node.r)) {
      if (el.tabIndex !== -1 || !el.hasAttribute("tabindex")) el.tabIndex = -1;
    } else if (el.hasAttribute("tabindex")) {
      el.removeAttribute("tabindex");
    }
    const kids = node.k ?? [];
    let focused = node.f ? el : null;
    if (kids.length === 0) {
      // A leaf says its name as its content (a button's label, a text, a
      // field's value).
      attr(el, "aria-label", null);
      if (el.childElementCount > 0 || el.textContent !== node.n) el.textContent = node.n;
    } else {
      attr(el, "aria-label", node.n || null);
      if (el.firstChild && el.firstChild.nodeType === Node.TEXT_NODE) el.textContent = "";
      for (let i = 0; i < kids.length; i++) {
        let child = el.children[i];
        if (!child) {
          child = document.createElement("div");
          child.dataset.stipple = "";
          el.append(child);
        }
        focused = sync(child, kids[i], x, y) ?? focused;
      }
      while (el.children.length > kids.length) el.lastElementChild.remove();
    }
    return focused;
  };

  const updateA11y = (json) => {
    stats.a11y++;
    place();
    const tree = JSON.parse(json);
    const focused = sync(root, tree, 0, 0);
    // Keyboard focus follows the app's: a text field's is the textarea
    // (named by the field), anything else's is its own element.
    if (focused && TEXT_ROLES.has(focused.getAttribute("role"))) {
      attr(input, "role", focused.getAttribute("role") === "spinbutton" ? "spinbutton" : null);
      attr(input, "aria-labelledby", idOf(focused));
      if (hasFocus()) focusQuietly(input);
    } else {
      attr(input, "role", null);
      attr(input, "aria-labelledby", null);
      if (focused && hasFocus()) focusQuietly(focused);
    }
  };

  const openUrl = options.openUrl ?? ((url) => window.open(url, "_blank", "noopener"));
  const notify = options.notify ?? ((title, body) => {
    if (typeof Notification === "undefined") return console.warn("no Notification API");
    const show = () => new Notification(title, { body });
    if (Notification.permission === "granted") return show();
    if (Notification.permission !== "denied") {
      Notification.requestPermission().then((p) => { if (p === "granted") show(); });
    }
  });

  // stipple::platform::share: navigator.share, with the files handed over
  // just before (share_file).
  const share = options.share ?? ((data) => {
    if (!navigator.share) return Promise.reject(new Error("no navigator.share"));
    if (data.files?.length && navigator.canShare && !navigator.canShare(data)) {
      return Promise.reject(new Error("these files can't be shared"));
    }
    return navigator.share(data);
  });
  let shareFiles = [];
  // Scheduled notifications (stipple::platform::schedule_notification):
  // timers while the page is open, by id.
  const scheduled = new Map();
  const showScheduled = (id, title, body) => {
    if (typeof Notification === "undefined" || Notification.permission !== "granted") {
      return console.warn(`stipple: notification ${id} not shown: no permission`);
    }
    const n = new Notification(title, { body, tag: id });
    n.onclick = () => {
      window.focus();
      n.close();
      const [p, l] = passStr(id);
      e.stipple_notification?.(p, l);
    };
  };
  const unschedule = (id) => {
    const t = scheduled.get(id);
    if (t) { clearTimeout(t.timeout); clearInterval(t.interval); }
    scheduled.delete(id);
  };

  // The app's saved state (App::save_state) lives in the tab's
  // sessionStorage, which a reload, a discarded tab brought back and the
  // history's back and forward keep, but a new tab doesn't.
  const stateKey = options.stateKey === false ? null
    : options.stateKey ?? `stipple-state:${new URL(wasmUrl, location.href).pathname}:${instances}`;

  const audio = audioHost(() => e); // see "Audio" above
  // The GPU renderer (gpu.js), for a module built with it.
  const gpu = gpuHost({
    memory: () => e.memory,
    claim: (fresh) => {
      if (kind === "2d" || (fresh && kind === "gpu")) swapCanvas();
      kind = "gpu";
      return canvas;
    },
    current: () => canvas,
    redraw: () => e?.stipple_redraw?.(),
    log: (msg) => console.log(msg),
    onFrame: (api) => {
      stats.presents++;
      stats.gpuFrames++;
      renderer = api;
    },
  });

  const imports = {
    stipple: {
      ...audio.imports,
      ...gpu.imports,
      now: () => performance.now(),
      present(ptr, w, h, rects, n) {
        stats.presents++;
        renderer = "software";
        if (kind === "gpu") {
          swapCanvas();
          n = 0;
        }
        if (!ctx) {
          ctx = canvas.getContext("2d");
          kind = "2d";
          // Chrome drops the 2D context of a canvas out of sight (a hidden
          // tab, an iframe scrolled away) and gives it back blank: draw the
          // whole frame again then.
          const c = canvas;
          c.addEventListener("contextrestored", () => {
            if (c !== canvas) return;
            blank = true;
            e?.stipple_redraw?.();
          });
        }
        if (blank) {
          blank = false;
          n = 0;
        }
        if (canvas.width !== w || canvas.height !== h) {
          canvas.width = w; // clears the canvas, so draw it all
          canvas.height = h;
          n = 0;
        }
        const img = new ImageData(new Uint8ClampedArray(e.memory.buffer, ptr, w * h * 4), w, h);
        if (n === 0) {
          ctx.putImageData(img, 0, 0);
        } else {
          const r = new Uint32Array(e.memory.buffer, rects, n * 4);
          for (let i = 0; i < n; i++) {
            ctx.putImageData(img, 0, 0, r[4 * i], r[4 * i + 1], r[4 * i + 2], r[4 * i + 3]);
          }
        }
      },
      schedule(ms) {
        if (timer !== null) clearTimeout(timer);
        timer = ms < 0 ? null : setTimeout(() => {
          timer = null;
          stats.wakes++;
          e.stipple_wake();
        }, ms);
      },
      request_frame() {
        framePending = true;
        queueFrame();
      },
      set_cursor(ptr, len) { canvas.style.cursor = str(ptr, len); },
      keep_awake(on) { wantAwake = on !== 0; syncWake(); },
      pointer_lock(on) {
        // Browsers only lock during (or a few seconds after) a user
        // gesture; a refusal or the lock ending reports kind 5.
        wantLock = on !== 0;
        if (!wantLock) {
          if (document.pointerLockElement === canvas) document.exitPointerLock();
          return;
        }
        if (document.pointerLockElement === canvas) return;
        try {
          // Unaccelerated where the browser can; where it can't, plain.
          requestLock({ unadjustedMovement: true })?.catch?.((err) => {
            if (err?.name !== "NotSupportedError" || !wantLock) return lockLost();
            try { requestLock()?.catch?.(lockLost); } catch (_) { lockLost(); }
          });
        } catch (_) { lockLost(); }
      },
      set_title(ptr, len) { if (options.title !== false) document.title = str(ptr, len); },
      clipboard_write(ptr, len) {
        pendingCopy = str(ptr, len);
        // Needs a secure context and a user gesture; the copy event below
        // covers Ctrl/Cmd+C and X either way.
        try {
          if (pendingHtml !== null && typeof ClipboardItem !== "undefined") {
            const blob = (s, type) => new Blob([s], { type });
            navigator.clipboard?.write([new ClipboardItem({
              "text/plain": blob(pendingCopy, "text/plain"),
              "text/html": blob(pendingHtml, "text/html"),
            })])?.catch(() => {});
          } else {
            navigator.clipboard?.writeText(pendingCopy)?.catch(() => {});
          }
        } catch (_) {}
      },
      clipboard_write_html(ptr, len) { pendingHtml = len ? str(ptr, len) : null; },
      open_file(id, ptr, len) {
        Promise.resolve(pickFile(str(ptr, len))).then(async (file) => {
          if (!file) return e.stipple_file_cancel(id);
          const data = new Uint8Array(await file.arrayBuffer());
          const [np, nl] = passStr(file.name);
          const [dp, dl] = pass(data);
          e.stipple_file(id, np, nl, dp, dl);
        }).catch((err) => { console.error(err); e.stipple_file_cancel(id); });
      },
      save_file(np, nl, dp, dl) {
        const name = str(np, nl);
        saveFile(name, new Blob([new Uint8Array(e.memory.buffer, dp, dl).slice()]));
      },
      log(level, ptr, len) { (level ? console.error : console.log)(str(ptr, len)); },
      set_a11y(ptr, len) { updateA11y(str(ptr, len)); },
      set_ime(active, x, y, w, h) {
        // The textarea sits at the caret, so the IME's candidate window
        // opens next to it.
        const s = input.style;
        s.left = `${active ? x : 0}px`;
        s.top = `${active ? y : 0}px`;
        s.height = `${active ? Math.max(h, 1) : 1}px`;
      },
      set_text_input(active) {
        // A touch keyboard only while a text field has focus: the one its
        // hints ask for.
        textInput = active !== 0;
        input.setAttribute("inputmode", textInput ? inputMode : "none");
      },
      set_input_hints(ptr, len) {
        // The focused field's keyboard: `name=value;...` attributes.
        for (const pair of str(ptr, len).split(";")) {
          const at = pair.indexOf("=");
          if (at < 0) continue;
          const [k, v] = [pair.slice(0, at), pair.slice(at + 1)];
          if (k === "inputmode") inputMode = v || "text";
          else if (v) input.setAttribute(k, v);
          else input.removeAttribute(k);
        }
        input.setAttribute("inputmode", textInput ? inputMode : "none");
      },
      open_url(ptr, len) { openUrl(str(ptr, len)); },
      notify(tp, tl, bp, bl) { notify(str(tp, tl), str(bp, bl)); },
      share_file(np, nl, mp, ml, dp, dl) {
        const bytes = new Uint8Array(e.memory.buffer, dp, dl).slice();
        shareFiles.push(new File([bytes], str(np, nl), { type: str(mp, ml) }));
      },
      share(id, tp, tl, xp, xl, up, ul) {
        const data = {};
        if (tl) data.title = str(tp, tl);
        if (xl) data.text = str(xp, xl);
        if (ul) data.url = str(up, ul);
        if (shareFiles.length) data.files = shareFiles;
        shareFiles = [];
        Promise.resolve().then(() => share(data)).then(
          () => e.stipple_answer?.(id, 0),
          (err) => e.stipple_answer?.(id, err?.name === "AbortError" ? 1 : 2),
        );
      },
      vibrate(ptr, n) {
        try { navigator.vibrate?.(Array.from(new Uint32Array(e.memory.buffer, ptr, n))); } catch (_) {}
      },
      schedule_notification(ip, il, tp, tl, bp, bl, delay, repeat) {
        const id = str(ip, il), title = str(tp, tl), body = str(bp, bl);
        unschedule(id);
        const t = { timeout: null, interval: null };
        t.timeout = setTimeout(() => {
          showScheduled(id, title, body);
          if (repeat > 0) t.interval = setInterval(() => showScheduled(id, title, body), repeat);
          else scheduled.delete(id);
        }, Math.min(delay, 0x7fffffff));
        scheduled.set(id, t);
      },
      cancel_notification(ip, il) {
        if (il === 0) { for (const id of [...scheduled.keys()]) unschedule(id); return; }
        unschedule(str(ip, il));
      },
      permission_status(kind) {
        if (kind === 2) {
          if (typeof Notification === "undefined") return 1;
          return permCode(Notification.permission === "default" ? "prompt" : Notification.permission);
        }
        return permState[kind] ?? 2;
      },
      permission_request(id, kind) {
        const answer = (code) => {
          if (code === 0 || code === 1) permState[kind] = code;
          e.stipple_answer?.(id, code);
        };
        if (kind === 0 || kind === 1) {
          const md = navigator.mediaDevices;
          if (!md?.getUserMedia) return answer(3);
          md.getUserMedia(kind === 0 ? { video: true } : { audio: true }).then(
            (stream) => { stream.getTracks().forEach((t) => t.stop()); answer(0); },
            (err) => answer(err?.name === "NotAllowedError" || err?.name === "SecurityError" ? 1 : 3),
          );
        } else if (kind === 2) {
          if (typeof Notification === "undefined") return answer(1);
          if (Notification.permission !== "default") {
            return answer(Notification.permission === "granted" ? 0 : 1);
          }
          Promise.resolve(Notification.requestPermission()).then(
            (p) => answer(permCode(p === "default" ? "prompt" : p)),
            () => answer(1),
          );
        } else if (kind === 3) {
          if (!navigator.geolocation) return answer(1);
          navigator.geolocation.getCurrentPosition(
            () => answer(0),
            (err) => answer(err?.code === 1 ? 1 : 2),
          );
        } else {
          answer(0);
        }
      },
      camera_count: () => cameraCount,
      camera_open(id, facing, width, height) {
        const md = navigator.mediaDevices;
        if (!md?.getUserMedia) return e.stipple_camera_failed?.(id, 2);
        const video = document.createElement("video");
        video.muted = true;
        video.playsInline = true;
        video.setAttribute("aria-hidden", "true");
        // In the document (some browsers only advance a video that is), but
        // not seen.
        Object.assign(video.style, {
          position: "fixed", left: "0", top: "0", width: "1px", height: "1px",
          opacity: "0", pointerEvents: "none",
        });
        document.body.appendChild(video);
        const cam = { video, stream: null, track: null, canvas: null, ctx: null, paused: false, closed: false };
        cameras.set(id, cam);
        md.getUserMedia({
          audio: false,
          video: {
            facingMode: facing === 1 ? "user" : "environment",
            width: { ideal: width }, height: { ideal: height },
          },
        }).then((stream) => {
          if (cam.closed) { stream.getTracks().forEach((t) => t.stop()); return; }
          permState[0] = 0;
          cam.stream = stream;
          cam.track = stream.getVideoTracks()[0] ?? null;
          video.srcObject = stream;
          video.play().catch(() => {});
          const caps = cam.track?.getCapabilities?.() ?? {};
          const settings = cam.track?.getSettings?.() ?? {};
          const user = settings.facingMode ? settings.facingMode === "user" : facing === 1;
          const flags = (caps.torch ? 1 : 0)
            | (caps.pointsOfInterest || caps.focusMode?.includes?.("single-shot") ? 2 : 0)
            | (user ? 4 : 0);
          e.stipple_camera_caps?.(id, flags, caps.zoom?.min ?? 1, caps.zoom?.max ?? 1);
          pumpCamera(id, cam);
        }, (err) => {
          const name = err?.name;
          if (name === "NotAllowedError") permState[0] = 1;
          e.stipple_camera_failed?.(id,
            name === "NotAllowedError" || name === "SecurityError" ? 1
              : name === "NotFoundError" || name === "OverconstrainedError" ? 2 : 3);
        });
      },
      camera_pause(id, paused) {
        const cam = cameras.get(id);
        if (!cam) return;
        cam.paused = paused !== 0;
        if (cam.track) cam.track.enabled = !cam.paused;
      },
      camera_close(id) {
        const cam = cameras.get(id);
        if (!cam) return;
        cameras.delete(id);
        cam.closed = true;
        cam.stream?.getTracks().forEach((t) => t.stop());
        cam.video.srcObject = null;
        cam.video.remove();
      },
      camera_torch(id, on) {
        cameras.get(id)?.track?.applyConstraints({ advanced: [{ torch: on !== 0 }] }).catch(() => {});
      },
      camera_zoom(id, factor) {
        cameras.get(id)?.track?.applyConstraints({ advanced: [{ zoom: factor }] }).catch(() => {});
      },
      camera_focus(id, x, y) {
        cameras.get(id)?.track?.applyConstraints({
          advanced: [{ pointsOfInterest: [{ x, y }], focusMode: "single-shot" }],
        }).catch(() => {});
      },
      save_state(ptr, len) {
        if (!stateKey) return;
        try {
          if (len === 0) return sessionStorage.removeItem(stateKey);
          sessionStorage.setItem(stateKey, toBase64(new Uint8Array(e.memory.buffer, ptr, len)));
        } catch (err) { console.warn("stipple: the state couldn't be saved", err); }
      },
      // stipple::game::save on the web: localStorage, values kept base64.
      // A read leaves its bytes in `storageOut` for storage_read to copy
      // into a buffer the module allocated for them.
      date_now: () => Date.now(),
      storage_set(kp, kl, vp, vl) {
        try {
          localStorage.setItem(str(kp, kl), toBase64(new Uint8Array(e.memory.buffer, vp, vl)));
          return 1;
        } catch (_) { return 0; }
      },
      storage_get(kp, kl) {
        let value = null;
        try { value = localStorage.getItem(str(kp, kl)); } catch (_) {}
        if (value === null) return -1;
        try { storageOut = Uint8Array.from(atob(value), (c) => c.charCodeAt(0)); } catch (_) { return -1; }
        return storageOut.length;
      },
      storage_keys(pp, pl) {
        const prefix = str(pp, pl), keys = [];
        try {
          for (let i = 0; i < localStorage.length; i++) {
            const k = localStorage.key(i);
            if (k !== null && k.startsWith(prefix)) keys.push(k);
          }
        } catch (_) {}
        storageOut = new TextEncoder().encode(keys.join("\n"));
        return storageOut.length;
      },
      storage_read(ptr) {
        new Uint8Array(e.memory.buffer, ptr, storageOut.length).set(storageOut);
        storageOut = new Uint8Array(0);
      },
      storage_remove(kp, kl) { try { localStorage.removeItem(str(kp, kl)); } catch (_) {} },
      gamepad_rumble(index, low, high, ms) {
        const pad = navigator.getGamepads?.()[index];
        const act = pad?.vibrationActuator;
        if (!act?.playEffect) return 0;
        act.playEffect("dual-rumble", {
          duration: ms, strongMagnitude: low, weakMagnitude: high,
        }).catch(() => {});
        return 1;
      },
      fetch_asset(id, ptr, len) {
        const path = str(ptr, len);
        getAsset(path).then(([data, status]) => deliverAsset(id, path, data, status));
      },
      message_box(confirm, ptr, len) {
        const text = str(ptr, len);
        if (confirm) return window.confirm(text) ? 1 : 0;
        window.alert(text);
        return 1;
      },
    },
  };

  // Packaged files (stipple::platform::assets): fetched under `assets`,
  // answered through stipple_asset (request 0: a preload).
  const assetBase = new URL((options.assets ?? "assets/").replace(/\/*$/, "/"), location.href);
  const getAsset = async (path) => {
    try {
      const r = await fetch(new URL(path, assetBase));
      if (!r.ok) return [new Uint8Array(0), r.status];
      return [new Uint8Array(await r.arrayBuffer()), 200];
    } catch (_) { return [new Uint8Array(0), 0]; }
  };
  const deliverAsset = (id, path, data, status) => {
    const [pp, pl] = passStr(path);
    const [dp, dl] = pass(data);
    e.stipple_asset?.(id, pp, pl, dp, dl, status);
  };

  const module = await compile(wasmUrl);
  const instance = await WebAssembly.instantiate(module, imports);
  e = instance.exports;

  // The first font is the app's; each later one is its fallback, in order
  // (a CJK or emoji font, say). Fetched together, handed over in order.
  const fonts = [options.font ?? []].flat();
  const loaded = await Promise.all(fonts.map(async (url) => {
    try {
      const r = await fetch(url);
      return r.ok ? new Uint8Array(await r.arrayBuffer()) : null;
    } catch (_) { return null; /* no font: shapes only */ }
  }));
  for (const bytes of loaded) {
    if (!bytes) continue;
    const [p, n] = pass(bytes);
    e.stipple_set_font(p, n);
  }

  if (options.preload) {
    let list = options.preload;
    if (typeof list === "string") {
      try {
        const r = await fetch(new URL(list, location.href));
        list = r.ok ? await r.json() : [];
      } catch (_) { list = []; }
    }
    const got = await Promise.all(list.map(async (path) => [path, ...await getAsset(path)]));
    for (const [path, data, status] of got) deliverAsset(0, path, data, status);
  }

  // prefers-color-scheme, read before the app is built (Theme::native() reads
  // it) and followed as it changes (Event::ThemeChanged). The accent comes
  // from the CSS AccentColor system color where the browser has one.
  const dark = window.matchMedia?.("(prefers-color-scheme: dark)");
  const accent = () => {
    if (!window.CSS?.supports?.("color", "AccentColor")) return -1;
    const probe = document.createElement("div");
    probe.style.cssText = "display:none;color:AccentColor";
    document.body.append(probe);
    const m = getComputedStyle(probe).color.match(/(\d+)\D+(\d+)\D+(\d+)/);
    probe.remove();
    return m ? (+m[1] << 16) | (+m[2] << 8) | +m[3] : -1;
  };
  const scheme = () => e.stipple_color_scheme?.(dark?.matches ? 1 : 0, accent());
  scheme();
  dark?.addEventListener?.("change", scheme);
  // prefers-reduced-motion, likewise: animations snap while it's `reduce`.
  const still = window.matchMedia?.("(prefers-reduced-motion: reduce)");
  const motion = () => e.stipple_reduced_motion?.(still?.matches ? 1 : 0);
  motion();
  still?.addEventListener?.("change", motion);
  // The languages (navigator.languages), the clock and the first day of the
  // week (Intl), and the time zone, read before the app is built
  // (platform::locale) and followed on languagechange (Event::LocaleChanged).
  const localeInfo = () => {
    const langs = navigator.languages?.length ? [...navigator.languages] : [navigator.language || "en-US"];
    let hour12 = "";
    try {
      const o = new Intl.DateTimeFormat(langs[0], { hour: "numeric" }).resolvedOptions();
      if (o.hourCycle) hour12 = o.hourCycle === "h11" || o.hourCycle === "h12" ? "1" : "0";
      else if (o.hour12 !== undefined) hour12 = o.hour12 ? "1" : "0";
    } catch (_) {}
    let firstDay = "";
    try {
      const l = new Intl.Locale(langs[0]);
      const w = l.getWeekInfo?.() ?? l.weekInfo;
      if (w?.firstDay) firstDay = String(w.firstDay);
    } catch (_) {}
    const offset = -new Date().getTimezoneOffset();
    return `languages=${langs.join(",")}\nhour12=${hour12}\nfirst_day=${firstDay}\nutc_offset=${offset}\n`;
  };
  const locale = () => {
    if (!e.stipple_locale) return;
    const [p, n] = passStr(localeInfo());
    e.stipple_locale(p, n);
  };
  locale();
  window.addEventListener?.("languagechange", locale);

  // A canvas that CSS doesn't size would grow with its backing store at a
  // pixel ratio above 1, so pin it at its current size.
  const r0 = canvas.getBoundingClientRect();
  const w0 = canvas.width;
  canvas.width = w0 + 1;
  if (canvas.getBoundingClientRect().width !== r0.width) {
    canvas.style.width = `${r0.width}px`;
    canvas.style.height = `${r0.height}px`;
  }
  canvas.width = w0;
  if (canvas.tabIndex < 0) canvas.tabIndex = 0;
  canvas.style.outline = "none";
  canvas.style.touchAction = "none";
  // A long press is the app's (its context menu), not the browser's callout
  // or text selection.
  canvas.style.webkitTouchCallout = "none";
  canvas.style.webkitUserSelect = "none";
  canvas.style.userSelect = "none";
  place();

  const size = () => {
    const r = canvas.getBoundingClientRect();
    return [r.width, r.height, window.devicePixelRatio || 1];
  };
  // A reload: hand the app the state it saved, before its first frame.
  if (stateKey && e.stipple_restore_state) {
    let saved = null;
    try { saved = sessionStorage.getItem(stateKey); } catch (_) {}
    if (saved) {
      try {
        const bin = atob(saved);
        const [p, n] = pass(Uint8Array.from(bin, (c) => c.charCodeAt(0)));
        e.stipple_restore_state(p, n);
      } catch (_) { /* not ours: start afresh */ }
    }
  }
  // What draws: the page's ?renderer=, else the option, else the app's choice.
  const rendererOption = new URLSearchParams(location.search).get("renderer") ?? options.renderer;
  if (rendererOption && e.stipple_renderer) {
    const [p, n] = passStr(String(rendererOption));
    e.stipple_renderer(p, n);
  }
  // `auto` (the app's Renderer::Auto) is the GPU on a hardware one, when
  // the module has the GPU renderer (it imports gpu.js's functions).
  const auto = !rendererOption || String(rendererOption).toLowerCase() === "auto";
  if (auto && e.stipple_gpu_hardware
    && WebAssembly.Module.imports(module).some((i) => i.name === "gpu_frame")) {
    e.stipple_gpu_hardware(hasHardwareGpu() ? 1 : 0);
  }
  e.stipple_start(...size());
  syncShown(); // a page opened in a background tab
  // Save it whenever the page may go: hidden (a phone's browser can discard
  // a backgrounded tab without another event) and unloading.
  const saveState = () => e.stipple_save_state?.();
  window.addEventListener("pagehide", saveState);
  document.addEventListener("visibilitychange", () => {
    syncShown();
    if (document.visibilityState === "hidden") saveState();
    // The browser lets a wake lock go when the page hides: ask again.
    else syncWake();
  });

  // The safe area (a phone's notch, status bar and home indicator, with
  // `viewport-fit=cover`), measured through a probe padded by it, as far as
  // it reaches over the canvas.
  const safeProbe = document.createElement("div");
  safeProbe.style.cssText = "position:fixed;left:0;top:0;width:0;height:0;" +
    "visibility:hidden;pointer-events:none;padding:env(safe-area-inset-top,0px) " +
    "env(safe-area-inset-right,0px) env(safe-area-inset-bottom,0px) env(safe-area-inset-left,0px)";
  document.body.append(safeProbe);
  const safeArea = () => {
    if (!e.stipple_safe_area) return;
    const s = getComputedStyle(safeProbe);
    const r = canvas.getBoundingClientRect();
    const vw = document.documentElement.clientWidth || window.innerWidth;
    const vh = document.documentElement.clientHeight || window.innerHeight;
    const px = (v) => parseFloat(v) || 0;
    e.stipple_safe_area(
      Math.max(0, px(s.paddingTop) - r.top),
      Math.max(0, px(s.paddingRight) - (vw - r.right)),
      Math.max(0, px(s.paddingBottom) - (vh - r.bottom)),
      Math.max(0, px(s.paddingLeft) - r.left));
  };
  safeArea();
  window.addEventListener("resize", safeArea);
  // The on-screen keyboard: the visual viewport shrinks above it while the
  // layout viewport (and the canvas) stays, so what of the canvas lies below
  // the visual viewport's bottom is under the keyboard.
  const keyboardInset = () => {
    const vv = window.visualViewport;
    if (!vv || !e.stipple_keyboard_inset) return;
    const r = canvas.getBoundingClientRect();
    const bottom = vv.offsetTop + vv.height;
    e.stipple_keyboard_inset(Math.max(0, Math.min(r.height, r.bottom - bottom)));
  };
  window.visualViewport?.addEventListener("resize", keyboardInset);
  window.visualViewport?.addEventListener("scroll", keyboardInset);
  // Size and pixel-ratio changes (moving to another screen, zooming).
  const resize = () => { place(); e.stipple_resize(...size()); safeArea(); keyboardInset(); };
  resizes = new ResizeObserver(resize);
  resizes.observe(canvas);
  visibility?.observe(canvas);
  const watchRatio = () => {
    matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`)
      .addEventListener("change", () => { resize(); watchRatio(); }, { once: true });
  };
  watchRatio();
  window.addEventListener("scroll", place, { capture: true, passive: true });

  // The pointer lock: whether the app wants it, and telling it when it ends
  // (Escape, the page losing focus) or was refused. With the promise the
  // newer `requestPointerLock` returns, a refusal rejects it; older browsers
  // fire `pointerlockerror` instead.
  let wantLock = false;
  let lockPromised = false;
  const lockLost = () => {
    if (!wantLock) return;
    wantLock = false;
    e.stipple_pointer(5, 0, 0, 0, 0);
  };
  const requestLock = (options) => {
    const p = canvas.requestPointerLock(options);
    lockPromised = typeof p?.then === "function";
    return p;
  };
  document.addEventListener("pointerlockchange", () => {
    if (document.pointerLockElement !== canvas) lockLost();
  });
  document.addEventListener("pointerlockerror", () => { if (!lockPromised) lockLost(); });

  // Pointer input, in CSS pixels relative to the canvas.
  const at = (ev) => {
    const r = canvas.getBoundingClientRect();
    return [ev.clientX - r.left, ev.clientY - r.top];
  };
  // Fingers come as touch events (below), each as a touch of its own; their
  // pointer events are left out. A module without `stipple_touch` (an older
  // build) takes touches as the mouse.
  const fingers = !!e.stipple_touch;
  const finger = (ev) => fingers && ev.pointerType === "touch";
  listen("pointerdown", (ev) => {
    if (finger(ev)) return;
    ev.preventDefault();
    focusQuietly(input);
    try { canvas.setPointerCapture(ev.pointerId); } catch (_) {}
    const [x, y] = at(ev);
    e.stipple_pointer(0, 0, x, y, mods(ev));
    e.stipple_pointer(1, ev.button, x, y, mods(ev));
  });
  listen("pointermove", (ev) => {
    if (finger(ev)) return;
    // The locked pointer stays put: only its motion counts.
    if (document.pointerLockElement === canvas) {
      if (ev.movementX || ev.movementY) e.stipple_pointer(4, 0, ev.movementX, ev.movementY, mods(ev));
      return;
    }
    const [x, y] = at(ev);
    e.stipple_pointer(0, 0, x, y, mods(ev));
    // A button pressed or released while another is down (a "chord")
    // arrives as a move whose `button` is set.
    if (ev.button >= 0 && ev.button <= 4) {
      const bit = [1, 4, 2, 8, 16][ev.button];
      e.stipple_pointer(ev.buttons & bit ? 1 : 2, ev.button, x, y, mods(ev));
    }
  });
  // No middle-click autoscroll or paste over the canvas: the app gets it.
  listen("mousedown", (ev) => {
    if (ev.button === 1) ev.preventDefault();
  });
  const release = (ev) => {
    if (finger(ev)) return;
    const [x, y] = at(ev);
    e.stipple_pointer(2, ev.button < 0 ? 0 : ev.button, x, y, mods(ev));
  };
  listen("pointerup", release);
  listen("pointercancel", release);
  listen("pointerleave", (ev) => {
    if (finger(ev)) return;
    if (!canvas.hasPointerCapture?.(ev.pointerId)) e.stipple_pointer(3, 0, 0, 0, 0);
  });
  // Touch events: every finger that changed, in CSS pixels relative to the
  // canvas. The app recognizes the gestures (scrolling and flinging, long
  // press for the context menu, pinch), the same way on every platform.
  // Taking them (preventDefault) keeps the browser from scrolling or zooming
  // the page and from sending the mouse events it emulates.
  const touch = (phase) => (ev) => {
    if (ev.cancelable) ev.preventDefault();
    if (phase === 0) focusQuietly(input);
    const r = canvas.getBoundingClientRect();
    for (const t of ev.changedTouches) {
      e.stipple_touch(phase, t.identifier >>> 0, t.clientX - r.left, t.clientY - r.top);
    }
  };
  if (fingers) {
    listen("touchstart", touch(0), { passive: false });
    listen("touchmove", touch(1), { passive: false });
    listen("touchend", touch(2), { passive: false });
    listen("touchcancel", touch(3), { passive: false });
  }
  listen("contextmenu", (ev) => ev.preventDefault());
  listen("wheel", (ev) => {
    ev.preventDefault();
    const [x, y] = at(ev);
    e.stipple_pointer(0, 0, x, y, mods(ev));
    e.stipple_wheel(ev.deltaX, ev.deltaY, ev.deltaMode);
  }, { passive: false });

  // Keyboard focus lives in the overlay (the textarea, or the element of the
  // app's focused node); the canvas hands it on when tabbed to.
  listen("focus", () => focusQuietly(input));
  let appFocused = false;
  overlay.addEventListener("focusin", (ev) => {
    if (!appFocused) {
      appFocused = true;
      e.stipple_focus(1);
    }
    // A screen reader moved focus to a text field's element: focus the field.
    // Any control it moved to that is scrolled out of view comes into view.
    const el = ev.target;
    if (!quiet && el !== input && el._b) revealNode(el);
    if (!quiet && el !== input && TEXT_ROLES.has(el.getAttribute?.("role"))) clickNode(el);
  });
  overlay.addEventListener("focusout", () => {
    // Focus may only be moving within the overlay; look once it settled.
    queueMicrotask(() => {
      if (appFocused && !hasFocus()) {
        appFocused = false;
        e.stipple_focus(0);
      }
    });
  });
  // A screen reader's "activate" clicks the element: click the node.
  overlay.addEventListener("click", (ev) => {
    const el = ev.target.closest?.("[data-stipple]");
    if (!el || !el.getAttribute("role")) return;
    ev.preventDefault();
    clickNode(el);
  });

  // Keys, while the app has focus. Keys an input method or a dead key takes
  // (`isComposing`, `Process`, `Dead`) are left to the textarea, whose
  // composition and `input` events below carry the text instead.
  // Every key goes first to `stipple_key_raw` (an element taking raw keys
  // may use it, which ends it here).
  const rawKey = (ev, release) => {
    if (!e.stipple_key_raw) return false;
    const [kp, kn] = passStr(ev.key);
    const [cp, cn] = passStr(ev.code ?? "");
    const flags = (release ? 1 : 0) | (ev.repeat ? 2 : 0);
    return e.stipple_key_raw(kp, kn, cp, cn, mods(ev), flags) !== 0;
  };
  overlay.addEventListener("keydown", (ev) => {
    if (ev.isComposing || ev.keyCode === 229 || ev.key === "Dead" || ev.key === "Process"
      || ev.key === "Unidentified") return;
    if (rawKey(ev, false)) {
      ev.preventDefault();
      return;
    }
    const [p, n] = passStr(ev.key);
    if (e.stipple_key(p, n, mods(ev))) ev.preventDefault();
  });
  overlay.addEventListener("keyup", (ev) => {
    if (ev.isComposing || ev.keyCode === 229 || ev.key === "Dead" || ev.key === "Process") return;
    if (rawKey(ev, true)) ev.preventDefault();
  });
  const ime = (kind, text) => {
    const [p, n] = passStr(text ?? "");
    e.stipple_ime(kind, p, n);
  };
  let composing = false;
  input.addEventListener("compositionstart", () => { composing = true; });
  input.addEventListener("compositionupdate", (ev) => ime(0, ev.data));
  input.addEventListener("compositionend", (ev) => {
    composing = false;
    if (ev.data) ime(1, ev.data);
    else ime(0, "");
    input.value = "";
  });
  input.addEventListener("input", (ev) => {
    if (composing || ev.isComposing || ev.inputType === "insertCompositionText") return;
    // Text that no keydown carried: a character composed with a dead key
    // outside a composition, or one picked from an on-screen keyboard.
    if ((ev.inputType === "insertText" || ev.inputType === "insertReplacementText") && ev.data) {
      ime(1, ev.data);
    }
    input.value = "";
  });

  // The clipboard: a page only gets to read it in a paste event, and writes it
  // most reliably in a copy or cut event.
  const focused = () => hasFocus() || document.activeElement === canvas;
  document.addEventListener("paste", (ev) => {
    if (!focused()) return;
    const text = ev.clipboardData?.getData("text/plain");
    if (text == null) return;
    ev.preventDefault();
    if (e.stipple_paste_html) {
      const [hp, hn] = passStr(ev.clipboardData?.getData("text/html") ?? "");
      e.stipple_paste_html(hp, hn);
    }
    const [p, n] = passStr(text);
    e.stipple_paste(p, n);
  });
  const copy = (ev) => {
    if (!focused() || pendingCopy === null) return;
    ev.clipboardData?.setData("text/plain", pendingCopy);
    if (pendingHtml !== null) ev.clipboardData?.setData("text/html", pendingHtml);
    pendingCopy = null;
    pendingHtml = null;
    ev.preventDefault();
  };
  document.addEventListener("copy", copy);
  document.addEventListener("cut", copy);

  // Gamepads: the Gamepad API has no input events, so poll
  // `navigator.getGamepads()` each animation frame while a pad is connected,
  // and hand the module each button and axis (standard-mapping indexes; it
  // keeps only changes). A module without the exports skips it.
  if (e.stipple_gamepad_connect && navigator.getGamepads) {
    const known = new Map(); // index -> Gamepad.id
    let polling = false;
    const pollPads = () => {
      polling = false;
      let pads = [];
      try { pads = navigator.getGamepads() || []; } catch (_) {}
      const seen = new Set();
      for (const pad of pads) {
        if (!pad || !pad.connected) continue;
        seen.add(pad.index);
        if (known.get(pad.index) !== pad.id) {
          known.set(pad.index, pad.id);
          e.stipple_gamepad_connect(pad.index, ...passStr(pad.id));
        }
        pad.buttons.forEach((b, i) => e.stipple_gamepad_button(pad.index, i, b.value, b.pressed ? 1 : 0));
        pad.axes.forEach((v, i) => e.stipple_gamepad_axis(pad.index, i, v));
      }
      for (const index of [...known.keys()]) {
        if (!seen.has(index)) {
          known.delete(index);
          e.stipple_gamepad_disconnect(index);
        }
      }
      if (known.size) schedulePads();
    };
    const schedulePads = () => {
      if (polling) return;
      polling = true;
      requestAnimationFrame(pollPads);
    };
    window.addEventListener("gamepadconnected", schedulePads);
    window.addEventListener("gamepaddisconnected", schedulePads);
    schedulePads();
  }

  return {
    exports: e, stats, overlay, input, colorScheme: scheme, saveState, gpu,
    get canvas() { return canvas; },
    get renderer() { return renderer; },
    // A 2D canvas with the last frame: the canvas itself on the CPU; on the
    // GPU, a copy made right after drawing the frame again.
    snapshot() { return kind === "gpu" ? gpu.snapshot() : canvas; },
  };
}
