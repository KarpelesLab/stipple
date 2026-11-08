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
// Options:
//   font      URL of the font to load (text doesn't render without one).
//   pickFile  async (accept) => File | null, in place of the <input type=file>
//             picker (tests use it).
//   saveFile  (name, blob) => void, in place of downloading the blob.
//
// Returns { exports, stats, canvas }: `stats` counts presents, animation
// frames and timer wakes, for tests.

const SHIFT = 1, CTRL = 2, ALT = 4, META = 8;

export async function start(canvas, wasmUrl, options = {}) {
  const ctx = canvas.getContext("2d");
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  const stats = { presents: 0, frames: 0, wakes: 0 };
  let e = null; // the module's exports
  let timer = null;
  let framePending = false;
  let pendingCopy = null;

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

  const imports = {
    stipple: {
      now: () => performance.now(),
      present(ptr, w, h, rects, n) {
        stats.presents++;
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
        if (framePending) return;
        framePending = true;
        requestAnimationFrame(() => {
          framePending = false;
          stats.frames++;
          e.stipple_frame();
        });
      },
      set_cursor(ptr, len) { canvas.style.cursor = str(ptr, len); },
      set_title(ptr, len) { document.title = str(ptr, len); },
      clipboard_write(ptr, len) {
        pendingCopy = str(ptr, len);
        // Needs a secure context and a user gesture; the copy event below
        // covers Ctrl/Cmd+C and X either way.
        try { navigator.clipboard?.writeText(pendingCopy)?.catch(() => {}); } catch (_) {}
      },
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
    },
  };

  const response = fetch(wasmUrl);
  let instance;
  try {
    ({ instance } = await WebAssembly.instantiateStreaming(response, imports));
  } catch (_) {
    // Served without `application/wasm`: compile from the bytes instead.
    const bytes = await (await fetch(wasmUrl)).arrayBuffer();
    ({ instance } = await WebAssembly.instantiate(bytes, imports));
  }
  e = instance.exports;

  if (options.font) {
    try {
      const r = await fetch(options.font);
      if (r.ok) {
        const [p, n] = pass(new Uint8Array(await r.arrayBuffer()));
        e.stipple_set_font(p, n);
      }
    } catch (_) { /* no font: shapes only */ }
  }

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

  const size = () => {
    const r = canvas.getBoundingClientRect();
    return [r.width, r.height, window.devicePixelRatio || 1];
  };
  e.stipple_start(...size());

  // Size and pixel-ratio changes (moving to another screen, zooming).
  const resize = () => e.stipple_resize(...size());
  new ResizeObserver(resize).observe(canvas);
  const watchRatio = () => {
    matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`)
      .addEventListener("change", () => { resize(); watchRatio(); }, { once: true });
  };
  watchRatio();

  // Pointer input, in CSS pixels relative to the canvas.
  const at = (ev) => {
    const r = canvas.getBoundingClientRect();
    return [ev.clientX - r.left, ev.clientY - r.top];
  };
  canvas.addEventListener("pointerdown", (ev) => {
    ev.preventDefault();
    canvas.focus();
    try { canvas.setPointerCapture(ev.pointerId); } catch (_) {}
    const [x, y] = at(ev);
    e.stipple_pointer(0, 0, x, y, mods(ev));
    e.stipple_pointer(1, ev.button, x, y, mods(ev));
  });
  canvas.addEventListener("pointermove", (ev) => {
    const [x, y] = at(ev);
    e.stipple_pointer(0, 0, x, y, mods(ev));
  });
  const release = (ev) => {
    const [x, y] = at(ev);
    e.stipple_pointer(2, ev.button < 0 ? 0 : ev.button, x, y, mods(ev));
  };
  canvas.addEventListener("pointerup", release);
  canvas.addEventListener("pointercancel", release);
  canvas.addEventListener("pointerleave", (ev) => {
    if (!canvas.hasPointerCapture?.(ev.pointerId)) e.stipple_pointer(3, 0, 0, 0, 0);
  });
  canvas.addEventListener("contextmenu", (ev) => ev.preventDefault());
  canvas.addEventListener("wheel", (ev) => {
    ev.preventDefault();
    const [x, y] = at(ev);
    e.stipple_pointer(0, 0, x, y, mods(ev));
    e.stipple_wheel(ev.deltaX, ev.deltaY, ev.deltaMode);
  }, { passive: false });

  // Keyboard input, while the canvas has focus.
  canvas.addEventListener("keydown", (ev) => {
    if (ev.isComposing) return;
    const [p, n] = passStr(ev.key);
    if (e.stipple_key(p, n, mods(ev))) ev.preventDefault();
  });
  canvas.addEventListener("focus", () => e.stipple_focus(1));
  canvas.addEventListener("blur", () => e.stipple_focus(0));

  // The clipboard: a page only gets to read it in a paste event, and writes it
  // most reliably in a copy or cut event.
  const focused = () => document.activeElement === canvas;
  document.addEventListener("paste", (ev) => {
    if (!focused()) return;
    const text = ev.clipboardData?.getData("text/plain");
    if (text == null) return;
    ev.preventDefault();
    const [p, n] = passStr(text);
    e.stipple_paste(p, n);
  });
  const copy = (ev) => {
    if (!focused() || pendingCopy === null) return;
    ev.clipboardData?.setData("text/plain", pendingCopy);
    pendingCopy = null;
    ev.preventDefault();
  };
  document.addEventListener("copy", copy);
  document.addEventListener("cut", copy);

  return { exports: e, stats, canvas };
}
