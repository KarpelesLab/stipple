# Web

A Stipple app compiles to a `wasm32-unknown-unknown` module that renders into an
RGBA framebuffer, which a small hand-written script blits onto a `<canvas>`. No
`wasm-bindgen` and no JavaScript framework are involved.

<!-- live: webdemo -->

## Making an app a web app

The app is an ordinary `App`. Build it in a function and hand that to
`stipple::web_app!` in a `cdylib` crate:

```rust
use stipple::prelude::*;

pub fn app() -> App<State> {
    App::new(State::default(), view).theme(Theme::dark())
}

stipple::web_app!(app);
```

```toml
[lib]
crate-type = ["cdylib", "rlib"]
```

The macro generates the module's exports on `wasm32` and nothing elsewhere, so
the crate still builds and tests natively.

A program that builds its app in `main` and calls `run` can be a web app as it
is: on the web, `App::run` hands the app to the page and returns, so
`stipple::web_app!(main)` works too. The app fills the canvas, whatever
`logical_size` it asked for, and `Font::system_default()` is the font the page
loaded. That's how the examples on the [site](#the-site) run: each ends its
`main.rs` with the macro, and a second target, a `cdylib`, includes it (in
Stipple's repository, an `[[example]]` named `NAME_web`). In a crate of your
own, that target is the library:

```rust
// src/main.rs, at the bottom
stipple::web_app!(main);
```

```rust
// src/web.rs
#[path = "main.rs"]
mod example;
```

```toml
[lib]
name = "myapp_web"
path = "src/web.rs"
crate-type = ["cdylib"]
```

`cargo build --lib --release --target wasm32-unknown-unknown` then builds
`myapp_web.wasm`, and `cargo run` still opens a window. What doesn't carry
over is what the browser can't do: threads (`std::thread::spawn` panics),
`std::time::Instant` (use `stipple::platform::time::Instant`, which reads
`performance.now()`), the file system, and exiting the process.

The page side is
[`web/stipple.js`](../../web/stipple.js), one file with no dependencies:

```html
<canvas id="app" style="width: 100vw; height: 100vh"></canvas>
<script type="module">
  import { start } from "./stipple.js";
  await start(document.getElementById("app"), "app.wasm", { font: "font.ttf" });
</script>
```

Give the canvas a CSS size; the app fills it. A font is needed for text, and
the web has no system fonts to fall back on: the page fetches it and hands it
to the app, unless the app set its own with `App::font`. Without either, the
browser console says so and text isn't drawn. `font` can also be a list: the
first is the app's font and the rest its fallbacks, in order, for characters
it lacks (`{ font: ["Inter.ttf", "NotoSansJP-Regular.otf"] }` draws Japanese
from the second).

`start` also adds an invisible element next to the canvas, `.stipple-overlay`,
fixed over it: the [ARIA mirror](#accessibility) of the app's accessibility
tree, and the hidden `<textarea>` that holds keyboard focus for
[input methods](#input-methods-and-dead-keys). It's positioned from the
canvas's `getBoundingClientRect()`, so it lines up unless an ancestor has a
`transform` (which makes `position: fixed` relative to that ancestor).

Other options: `label`, the app's accessible name (default: the canvas's
`aria-label`), and `title: false`, which keeps the app from setting
`document.title`.

## Building the demo

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --example webdemo --target wasm32-unknown-unknown
```

Serve these files from one directory:

- `examples/webdemo/index.html`, the page (and `multi.html`, two copies of
  the app side by side);
- `web/stipple.js` and `web/gpu.js` (which `stipple.js` imports);
- `target/wasm32-unknown-unknown/release/examples/webdemo.wasm`;
- `font.ttf`, the font the page loads. Without it, text doesn't render.

```sh
python3 -m http.server --directory site
```

## The site

The `Pages` workflow publishes <https://karpeleslab.github.io/stipple/> on
each push to `master`: the README, these guides, and every example that runs
in a browser, built by [`tools/site`](../../tools/site/build.py) (Python, no
dependencies). [`tools/site/examples.toml`](../../tools/site/examples.toml)
lists the examples, which run on the web and which need a desktop, and the one
cargo command that builds them all. A guide shows an example running where it
has a line

```html
<!-- live: todo -->
```

which GitHub doesn't show. To build the site locally:

```sh
python3 tools/site/build.py wasm        # every web example, release, wasm32, SIMD, LTO
python3 tools/site/build.py site --font /path/to/DejaVuSans.ttf  # and wasm-opt, if installed
python3 tools/site/check.py target/site # each example in headless Chrome, and the links
python3 -m http.server --directory target/site
```

`check.py` opens every example and fails if one doesn't start, draw or
panics; it also saves each one's first frames as the gallery's thumbnail. CI
runs it on every push. The demo's own pages keep their URLs: `webdemo.html`
(this page's `index.html`), `multi.html`, `webgpu.html` and `game.html`.

[`examples/webdemo`](../../examples/webdemo) has a click counter, a spinner
while a timer "loads", a repeating timer, a text editor with copy and paste,
buttons that open and save a file, and a checkbox, in `Theme::native()`.
`index.html#selftest` drives all of that with scripted DOM events and reports
what the app saw: also composition and dead keys, the ARIA overlay (and a click
on its elements), the theme following the color scheme, and a second app
started on the same page. CI runs it in headless Chrome, once with a dark and
once with a light color scheme.

## What works

| | How |
|---|---|
| Rendering | `putImageData` of the frame, limited to the damaged rects; or on the GPU, WebGPU or WebGL2 straight into the canvas (below) |
| High-DPI | `devicePixelRatio` is the scale factor; the canvas's backing store is its CSS size times the ratio. A `ResizeObserver` and a `resolution` media query follow size and ratio changes (`Event::Resized`, `Event::ScaleFactorChanged`) |
| Pointer | pointer events, captured while a button is down; right-click opens context menus instead of the browser's |
| Touch | touch events (each of `changedTouches`, by `identifier`) go through the app's gestures as on iOS and Android: touch scrolling and flings on both axes, a long press for the context menu, pinches; the first finger is also the pointer. The canvas has `touch-action: none` and no long-press callout, so the browser neither scrolls nor zooms the page under it. See [pointer-events.md](../pointer-events.md#touch-gestures) |
| Keyboard | `keydown`'s `key` and modifiers: text, editing and navigation keys, F1–F12 and Ctrl/Cmd shortcuts. The browser keeps its own shortcuts (F5, Ctrl+R and so on) |
| Input methods and dead keys | see below |
| Accessibility | see below |
| Dark mode and accent | see below |
| Several apps on a page | see below |
| Scrolling | `wheel` events, in pixels, lines or pages |
| Cursor shapes | the canvas's `style.cursor`, from `CursorIcon` |
| Timers and `AppHandle::update` | one `setTimeout` at the earliest deadline, which calls back into the module |
| Animation | `requestAnimationFrame` paces the frame clock, and stops when nothing animates |
| Clipboard | see below |
| File dialogs | see below |
| Safe area | `env(safe-area-inset-*)`, measured on a hidden probe and taken as far as it reaches over the canvas, as `cx.safe_area()` (`Event::WindowChanged` when it changes). Zero unless the page's viewport meta tag has `viewport-fit=cover` and the phone has a notch or rounded corners. See [layout.md](../layout.md#the-safe-area-phones) |
| On-screen keyboard | the focused field's [hints](../text-editing.md#keyboard-hints) become the hidden textarea's `inputmode`, `enterkeyhint`, `autocapitalize`, `autocorrect`, `spellcheck` and `autocomplete`; how far the keyboard reaches over the canvas comes from `visualViewport` (the canvas's bottom less the visual viewport's) as `cx.keyboard_inset()`, and the view is laid out above it |
| Keeping the screen awake | `AppHandle::set_keep_awake` takes a Screen Wake Lock (`navigator.wakeLock`), asked again each time the page becomes visible (the browser lets it go when the page hides); nothing where the browser has no wake lock |
| Share sheet | `navigator.share` (with `File`s where `navigator.canShare` takes them), from a click handler; browsers without it answer `ShareOutcome::Unavailable` |
| Haptics | `navigator.vibrate` (Chrome and Firefox on Android; not Safari or desktops) |
| Permissions | `platform::permission`: the Permissions API for the camera, the microphone and the location (what it last said; it's asked once at start and followed), `Notification.permission`; asking opens a `getUserMedia` stream and stops it, or reads the position once. No settings page to open |
| Camera | `stipple::camera`: `getUserMedia` into a hidden `<video>`, each new frame (`requestVideoFrameCallback`) drawn into a canvas and handed over as RGBA on the page's thread; torch, zoom and focus point through `applyConstraints` where the browser offers them. Needs HTTPS (or `localhost`). See [camera.md](../camera.md) |
| Scheduled notifications | a `setTimeout` (then `setInterval`) in the page, so only while it's open; clicking one focuses the page and reports its id (`App::on_notification_tapped`). No service worker, so nothing shows once the tab is closed |

### Clipboard

A page can only read the clipboard during a `paste` event. Ctrl/Cmd+V therefore
reaches the app through that event, carrying the text, rather than as a key.
Copy and cut write with `navigator.clipboard.writeText` and also fill the `copy`
/ `cut` event that follows the key, which works without a secure context.
`stipple::platform::clipboard::text()` returns what the last paste carried, and
`set_text` writes as copying does. HTML goes along: `clipboard::write` with HTML
uses `navigator.clipboard.write` (a `ClipboardItem` with `text/plain` and
`text/html`) and fills the `copy` event's `text/html`, and `clipboard::read`
returns the HTML the last paste carried (passed in through `stipple_paste_html`).
Images aren't supported.

### File dialogs

A browser can't block for a dialog and doesn't reveal paths, so `open_file`,
`save_file` and `pick_folder` return `None` on the web, and
`pick_folder_then` answers `None` (Chromium's `showDirectoryPicker` isn't
used). Use the contents-based
calls, which work on every platform:

```rust
use stipple::platform::dialog::{self, FileDialog};

// In a tap handler:
let handle = cx.handle();
dialog::open_file_contents(&FileDialog::new().accept(".txt"), move |file| {
    if let Some(file) = file {
        handle.update(move |s| s.text = String::from_utf8_lossy(&file.bytes).into());
    }
});

dialog::save_file_contents(&FileDialog::new().file_name("notes.txt"), bytes);
```

- `open_file_contents` opens an `<input type=file>` and returns at once; the
  callback runs once the browser has read the file, and changes the state
  through the `AppHandle`, which re-renders. iOS and Android answer the same
  way (see their pages). On the desktops the same call shows
  the native dialog and runs the callback before returning. Call it from an
  input handler: browsers only open a picker in response to user input.
- `save_file_contents` downloads the bytes as `file_name` (`download` if
  empty). On the desktops it shows the save dialog and writes the file.
- `FileDialog::accept` filters the web's picker; the desktop dialogs ignore it,
  and don't pre-fill `file_name` yet.

### Packaged assets

[`platform::assets`](../os-integration.md#packaged-assets) fetches files from
`start`'s `assets` URL (default `assets/`, next to the page). A fetch can't
block, so `AppHandle::load_asset` and `assets().load` work for any file and
answer once it arrives, while the blocking calls (`read`, `image`, `text`…)
see only the files listed in `start`'s `preload` (an array, or the URL of a
JSON array), which `stipple.js` fetches before it starts the app. The site
does this for each example with an `assets/` directory: it serves it next to
the example's page with an `assets.json` listing every file, which the page
preloads (`imagedemo` reads its PNG and SVG that way).

### Input methods and dead keys

Keyboard focus, while the app has it, is on a hidden `<textarea>` in the
overlay (clicking the canvas, or tabbing to it, puts it there). `keydown`
still delivers named keys, shortcuts and plain characters, and those it
handles are `preventDefault`ed, so they never type into the textarea as well.
Keys an input method or a dead key takes are left alone (`isComposing`, key
code 229, `Process`, `Dead`), and the textarea's events carry their text
instead:

- `compositionupdate` is `Ime::Preedit` (the field shows it underlined at the
  caret) and `compositionend` is `Ime::Commit`;
- an `input` event that inserted text outside a composition (a character
  composed with a dead key where the browser doesn't use composition, or one
  from an on-screen keyboard) is committed the same way.

`Window::set_ime` moves the textarea to the caret, so the IME's candidate
window opens next to the text, and `set_text_input_active` switches its
`inputmode` between the focused field's (`text`, `numeric`, `decimal`,
`tel`, `email`, `url`, `search`, from `set_input_hints`) and `none`, which
shows or hides a touch keyboard.
The textarea is emptied after each commit; the field draws the text.

### Accessibility

The app's accessibility tree (the one the desktops give AT-SPI, UI Automation
and NSAccessibility) is sent to the page as JSON after each frame in which it
changed, and `stipple.js` mirrors it in the overlay: one element per node,
nested like the tree, each placed over its node's bounds, so a screen reader
reads them in order and can point at them. The overlay is `opacity: 0` and
lets the pointer through to the canvas; the canvas itself is `aria-hidden`.
Elements are reused from one update to the next and only what changed is
touched.

| Stipple role | ARIA |
|---|---|
| button, text field, combo box, spin button, checkbox, switch | `button`, `textbox`, `combobox`, `spinbutton`, `checkbox`, `switch` |
| list, list item | `listbox`, `option` |
| table, row, cell, column header | `grid`, `row`, `gridcell`, `columnheader` |
| tree, tree item | `tree`, `treeitem` |
| menu bar, menu, menu item | `menubar`, `menu`, `menuitem` |
| toolbar, alert, splitter and separator | `toolbar`, `alert`, `separator` |
| text | no role, its text as content |
| group | `group` when it has a name, otherwise no role |

A leaf's name is its content (a button's label, a field's text); a
container's is its `aria-label`. `selected`, `expanded` and `checked` become
`aria-selected`, `aria-expanded` and `aria-checked` (`Element::checked` marks
a checkbox or switch; the built-in ones do).

- **Activation.** A screen reader activates an element by clicking it; the
  overlay turns a click on an element into a press and release at the middle
  of its node, which runs the node's tap handler (or focuses a field). A node
  scrolled out of its container's view is scrolled into it first, and the
  click lands where it then is.
- **Relations.** A control that controls another (`Element::controls`: a
  select's open list, a tab's panel) has `aria-controls` naming that node's
  element (`ct` and `i` in the JSON); a tree item has its `aria-level`
  (`lv`), and an item its `aria-posinset` / `aria-setsize` (`ps`, see
  [positions in a set](../accessibility.md#positions-in-a-set)).
- **Focus, app to page.** When the app's focus moves to a text field, the
  textarea takes keyboard focus and is labelled by the field's element
  (`aria-labelledby`); for any other focused node, its own element takes it.
  This only happens while focus is already in the app, so the page's focus is
  never stolen.
- **Focus, page to app.** Interactive elements are focusable (`tabindex=-1`);
  when a screen reader moves focus to a text field's element, the field is
  focused in the app.
- **Scrolling.** A scroll container's element carries where it is scrolled
  (`sc` in the JSON). When focus moves to an element scrolled out of its
  container's view, the overlay scrolls the app's container to show it (a
  wheel at the container's middle, the least movement that shows it). ARIA
  has no scroll role or action to say more.

### Theme

`Theme::native()` follows the page's `prefers-color-scheme`: `stipple.js` reads
it before the app is built and listens for the media query's `change` event,
which reaches a running app as `Event::ThemeChanged`. The accent color is the
CSS `AccentColor` system color, where the browser supports it (Firefox and
Safari; Chrome reports its own default blue rather than the OS's accent).
Otherwise the theme keeps Stipple's. The colors and metrics are the ones
`Theme::native()` uses on Linux; there are no OS-drawn controls on the web.

`prefers-reduced-motion` reaches the app the same way (see
[reduced motion](../animation.md#reduced-motion)): while it's `reduce`, fades
and theme changes snap.

### Several apps on a page

Each `start` call instantiates the module afresh, with its own memory, so the
backend's state (it lives in the module) is per canvas and any number of apps
run side by side, the same module or different ones. The module is fetched and
compiled once per URL. Each app's overlay, timers, frames and clipboard
handling are its own; the `paste`, `copy` and `cut` listeners act only for the
app that has focus. Pass `title: false` so the apps don't fight over
`document.title`. See `examples/webdemo/multi.html`.

### Keeping the state across a reload

`App::save_state` and `App::restore_state` work as on a phone, with the tab
standing in for the process: when the page hides (`visibilitychange`; a
phone's browser may discard a hidden tab without another word) or goes away
(`pagehide`, a reload or navigating elsewhere), `stipple.js` asks the app for
its state and keeps it, base64, in the tab's `sessionStorage`. When the page
loads again in the same tab (a reload, a discarded tab brought back, going
back to it in the history), it hands the bytes to the app before its first
frame. A new tab, or another browser window, starts afresh; an app that saves
nothing removes what was kept.

The key is the module's URL and the canvas's order on the page, so each of
several apps keeps its own; `start`'s `stateKey` option names another, and
`stateKey: false` keeps no state. `start` also returns `saveState()`, to save
at another moment. `webdemo` keeps its click count and checkbox this way;
`index.html#restore` checks it in CI. See
[window-lifecycle.md](../window-lifecycle.md#keeping-the-state-across-process-death).

What should outlive the tab (settings, saved games) goes in `localStorage`
through `stipple::game::save::SaveStore` (`SaveStore::app_data()` or
`local_storage(prefix)`); see [games.md](../games.md#saving).

### Rendering on the GPU

A module built with the `gpu` feature (`--features stipple/gpu`; the site
builds its examples with it) can draw its frames on the GPU instead of the
CPU: [WebGPU](https://www.w3.org/TR/webgpu/) through the canvas's `"webgpu"`
context, or WebGL2 where the browser has no WebGPU, presenting straight into
the canvas with no read-back. The scene is lowered in the module as on every
GPU platform (`src/gpu/ops.rs`) and handed to `web/gpu.js` as a compact op
buffer each frame (boxes, path fills with their triangles, images by texture
id, clips, layers); images and glyph rasters are uploaded once and kept by
identity. `gpu.js` draws it with WGSL shaders on WebGPU and GLSL ES 3.0 on
WebGL2, the same four the Metal executor uses. See [gpu.md](../gpu.md#the-web).

What draws is the app's `App::renderer`, which the page can override as
`STIPPLE_RENDERER` does natively, with `start`'s `renderer` option or the
page's `?renderer=` query parameter:

| `renderer` | Draws with |
|---|---|
| `auto` (the default) | the app's choice; `Renderer::Auto` is the GPU when the browser runs WebGPU / WebGL2 on a hardware one (and the module has the `gpu` feature), else the CPU |
| `software` | the CPU, `putImageData` |
| `gpu` | WebGPU, else WebGL2, else the CPU |
| `webgpu`, `webgl2` | that API, else the CPU |

```js
const app = await start(canvas, "app.wasm", { font: "font.ttf", renderer: "gpu" });
console.log(app.renderer); // "webgpu", "webgl2" or "software", once it has drawn
```

The GPU comes up asynchronously: the app's first frame waits for it (a few
milliseconds), then is drawn whole. A lost device (or context) is set up
again once; when that fails, or the device is lost a second time, the app
draws on the CPU from then on. A canvas keeps the first kind of context it
hands out, so `stipple.js` then swaps in a fresh canvas element (the same
attributes, style and listeners) for the 2D context; `app.canvas` is always
the current one. A GPU canvas can't be read back after the frame is shown:
`app.snapshot()` gives a 2D copy of the last frame.

### Speed

`auto` puts an app on the GPU where the browser has a real one; out of view
(scrolled away, in an iframe scrolled off its page, a hidden tab) an app
draws no frames at all; the site builds its modules with SIMD, LTO and
`wasm-opt`. Numbers, and what to do for your own app, are in
[performance.md](../performance.md#on-the-web).

## How it fits together

`App::run` on the web keeps the event handler and returns; from then on the
page calls into the module for each DOM event, and the module calls a few
functions it imports from the page. Both sides are implementation details
shared by `web_app!` and `stipple.js`, listed here for reference.

The module exports:

| Export | Called for |
|---|---|
| `stipple_alloc(len)` | memory for the page to pass bytes in (taken back by the export that receives them) |
| `stipple_set_font(ptr, len)` | a font's bytes: the first call's is the app's font, each later one a fallback |
| `stipple_start(w, h, ratio)` | building and running the app in a canvas of `w` × `h` CSS pixels |
| `stipple_resize(w, h, ratio)` | size or pixel-ratio changes |
| `stipple_pointer(kind, button, x, y, mods)` | moves, presses, releases and leaving the canvas; while the pointer is locked, its motion (`movementX`/`movementY`) and the lock ending or being refused |
| `stipple_touch(phase, id, x, y)` | each finger of `touchstart` (0), `touchmove` (1), `touchend` (2), `touchcancel` (3); a page whose module lacks it takes fingers as the mouse |
| `stipple_wheel(dx, dy, mode)` | `wheel` |
| `stipple_key(ptr, len, mods)` | `keydown`; returns whether to `preventDefault` |
| `stipple_paste(ptr, len)` | `paste` |
| `stipple_ime(kind, ptr, len)` | composition: 0 the preedit, 1 committed text |
| `stipple_color_scheme(dark, accent)` | `prefers-color-scheme`, and the accent as `0xRRGGBB` (negative for none): before `stipple_start`, and on changes |
| `stipple_reduced_motion(reduce)` | `prefers-reduced-motion: reduce`: before `stipple_start`, and on changes |
| `stipple_focus(focused)` | focus entering and leaving the app |
| `stipple_wake()` | the timer |
| `stipple_frame()` | `requestAnimationFrame` |
| `stipple_file(id, …)`, `stipple_file_cancel(id)` | a picked file, or a cancelled picker |
| `stipple_asset(id, path…, data…, status)` | a packaged file fetched for `fetch_asset` request `id` (0: a preload, before `stipple_start`), with its HTTP status (0: the fetch failed) |
| `stipple_restore_state(ptr, len)` | the state saved in `sessionStorage`, before `stipple_start` |
| `stipple_save_state()` | `pagehide` and `visibilitychange` to hidden: the app saves its state, through the `save_state` import |
| `stipple_renderer(ptr, len)` | the page's `renderer` option, before `stipple_start` |
| `stipple_gpu_hardware(on)` | whether WebGL2 runs on a hardware GPU (not SwiftShader), before `stipple_start`, for `renderer: "auto"` |
| `stipple_visible(on)` | the canvas scrolled out of view or the tab hidden (0), or back (1): frames stop meanwhile |
| `stipple_redraw()` | the GPU came up: draw a whole frame |

It imports, from the module `"stipple"`: `now`, `present`, `schedule`,
`request_frame`, `set_cursor`, `pointer_lock` (`requestPointerLock`, which
browsers only grant during a user gesture), `set_title`, `clipboard_write`,
`clipboard_write_html` (the HTML for the next `clipboard_write`), `open_file`,
`save_file`, `open_url` (`window.open`, for `platform::open_url`), `notify`
(the `Notification` API, for `platform::notify`), `log` (panics are printed to
the console), `set_a11y` (the accessibility tree as JSON), `set_ime` (the caret
rect, or none), `set_text_input`, `set_input_hints` (the focused field's
keyboard attributes), `save_state` (the bytes to keep in
`sessionStorage`; none removes them) and `fetch_asset` (a packaged file
under the `assets` URL, answered through `stipple_asset`), and for `stipple::game::save`'s
`SaveStore::local_storage`: `date_now`, `storage_set`, `storage_get`,
`storage_keys`, `storage_read` and `storage_remove` (`localStorage`, values
kept base64). See `src/platform/backend/web.rs`. With the `gpu` feature, from
`gpu.js`: `gpu_init(want)` (set up WebGPU and/or WebGL2; "pending" until it
has), `gpu_texture(id, ptr, w, h)` and `gpu_free(id)` (image textures),
`gpu_frame(ptr, words)` (draw the op buffer: shown, lost, pending or failed)
and `gpu_release()` (the app draws on the CPU again); see `src/gpu/web.rs`.

`std::time::Instant::now` panics on `wasm32-unknown-unknown`, so Stipple's
timers and frame clock use `stipple::platform::time::Instant`, which is std's
natively and reads `performance.now()` on the web.

## Known gaps

- The IME and ARIA paths are checked with synthetic DOM events in headless
  Chrome, not with a real input method or screen reader yet.
- The textarea isn't told the field's surrounding text, so IME features that
  read it (reconversion, some predictive keyboards) don't see it.
- Screen readers get no live announcements of value changes beyond the tree
  updates (no `aria-live` regions; an `alert` node relies on its role).
- No OS-drawn controls: `Theme::native()` uses the self-drawn look.
- Window controls (size limits, maximize, fullscreen, icon) do nothing.
- `App::on_open_url` hears nothing: a page is opened by its own URL, which
  the app reads at start (`location`, through the page's JavaScript).
