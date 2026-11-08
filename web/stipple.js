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
// Next to the canvas, `start` adds an invisible overlay (`.stipple-overlay`)
// that mirrors the app's accessibility tree as ARIA elements for screen
// readers, and holds the hidden <textarea> that has keyboard focus while the
// app does, so input methods and dead keys work.
//
// Options:
//   font      URL of the font to load (text doesn't render without one).
//   label     the accessible name of the app (default: the canvas's
//             aria-label, if any).
//   title     false to leave document.title alone (for a page that runs
//             several apps); by default the app's title becomes the page's.
//   pickFile  async (accept) => File | null, in place of the <input type=file>
//             picker (tests use it).
//   saveFile  (name, blob) => void, in place of downloading the blob.
//
// Returns { exports, stats, canvas, overlay, input, colorScheme }: `stats`
// counts presents, animation frames, timer wakes and accessibility-tree
// updates, for tests; `overlay` is the ARIA overlay, `input` the hidden
// textarea, and `colorScheme()` hands the page's current color scheme to the
// app again (it already follows changes on its own).

const SHIFT = 1, CTRL = 2, ALT = 4, META = 8;

// Compiled modules by URL, so several canvases running the same app compile
// it once. Each still gets its own instance (and memory).
const modules = new Map();
let instances = 0;

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
  "button", "textbox", "checkbox", "switch", "combobox", "spinbutton", "option", "listbox",
  "grid", "row", "gridcell", "columnheader", "tree", "treeitem", "menuitem", "menubar",
  "menu", "separator",
]);
const TEXT_ROLES = new Set(["textbox", "spinbutton"]);

export async function start(canvas, wasmUrl, options = {}) {
  const ctx = canvas.getContext("2d");
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  const stats = { presents: 0, frames: 0, wakes: 0, a11y: 0 };
  const prefix = `stipple${++instances}`;
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
  input.style.cssText =
    "position:absolute;left:0;top:0;width:1px;height:1px;margin:0;padding:0;border:0;" +
    "outline:none;resize:none;overflow:hidden;white-space:pre;font-size:16px;" +
    "color:transparent;background:transparent;caret-color:transparent;";
  overlay.append(root, input);
  canvas.after(overlay);
  // The overlay stands for the canvas's content.
  canvas.setAttribute("aria-hidden", "true");

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
      set_title(ptr, len) { if (options.title !== false) document.title = str(ptr, len); },
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
        // A touch keyboard only while a text field has focus.
        input.setAttribute("inputmode", active ? "text" : "none");
      },
    },
  };

  const module = await compile(wasmUrl);
  const instance = await WebAssembly.instantiate(module, imports);
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
  place();

  const size = () => {
    const r = canvas.getBoundingClientRect();
    return [r.width, r.height, window.devicePixelRatio || 1];
  };
  e.stipple_start(...size());

  // Size and pixel-ratio changes (moving to another screen, zooming).
  const resize = () => { place(); e.stipple_resize(...size()); };
  new ResizeObserver(resize).observe(canvas);
  const watchRatio = () => {
    matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`)
      .addEventListener("change", () => { resize(); watchRatio(); }, { once: true });
  };
  watchRatio();
  window.addEventListener("scroll", place, { capture: true, passive: true });

  // Pointer input, in CSS pixels relative to the canvas.
  const at = (ev) => {
    const r = canvas.getBoundingClientRect();
    return [ev.clientX - r.left, ev.clientY - r.top];
  };
  canvas.addEventListener("pointerdown", (ev) => {
    ev.preventDefault();
    focusQuietly(input);
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

  // Keyboard focus lives in the overlay (the textarea, or the element of the
  // app's focused node); the canvas hands it on when tabbed to.
  canvas.addEventListener("focus", () => focusQuietly(input));
  let appFocused = false;
  overlay.addEventListener("focusin", (ev) => {
    if (!appFocused) {
      appFocused = true;
      e.stipple_focus(1);
    }
    // A screen reader moved focus to a text field's element: focus the field.
    const el = ev.target;
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
  overlay.addEventListener("keydown", (ev) => {
    if (ev.isComposing || ev.keyCode === 229 || ev.key === "Dead" || ev.key === "Process"
      || ev.key === "Unidentified") return;
    const [p, n] = passStr(ev.key);
    if (e.stipple_key(p, n, mods(ev))) ev.preventDefault();
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

  return { exports: e, stats, canvas, overlay, input, colorScheme: scheme };
}
