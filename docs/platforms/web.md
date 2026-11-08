# Web

A Stipple app compiles to a `wasm32-unknown-unknown` module that renders into an
RGBA framebuffer, which a small hand-written script blits onto a `<canvas>`. No
`wasm-bindgen` and no JavaScript framework are involved.

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
the crate still builds and tests natively. The page side is
[`web/stipple.js`](../../web/stipple.js), one file with no dependencies:

```html
<canvas id="app" style="width: 100vw; height: 100vh"></canvas>
<script type="module">
  import { start } from "./stipple.js";
  await start(document.getElementById("app"), "app.wasm", { font: "font.ttf" });
</script>
```

Give the canvas a CSS size; the app fills it. A font is needed for text: the
page fetches it and hands it to the app, unless the app set its own with
`App::font`.

## Building the demo

```sh
rustup target add wasm32-unknown-unknown
cargo build --release -p webdemo --target wasm32-unknown-unknown
```

Serve these four files from one directory:

- `examples/webdemo/index.html`, the page;
- `web/stipple.js`;
- `target/wasm32-unknown-unknown/release/webdemo.wasm`;
- `font.ttf`, the font the page loads. Without it, text doesn't render.

```sh
python3 -m http.server --directory site
```

[`examples/webdemo`](../../examples/webdemo) has a click counter, a spinner
while a timer "loads", a repeating timer, a text editor with copy and paste, and
buttons that open and save a file. `index.html#selftest` drives all of that with
scripted DOM events and prints a report; CI runs it in headless Chrome.

## What works

| | How |
|---|---|
| Rendering | `putImageData` of the frame, limited to the damaged rects |
| High-DPI | `devicePixelRatio` is the scale factor; the canvas's backing store is its CSS size times the ratio. A `ResizeObserver` and a `resolution` media query follow size and ratio changes (`Event::Resized`, `Event::ScaleFactorChanged`) |
| Pointer | pointer events, captured while a button is down; right-click opens context menus instead of the browser's |
| Keyboard | `keydown`'s `key` and modifiers: text, editing and navigation keys, F1–F12 and Ctrl/Cmd shortcuts. The browser keeps its own shortcuts (F5, Ctrl+R and so on) |
| Scrolling | `wheel` events, in pixels, lines or pages |
| Cursor shapes | the canvas's `style.cursor`, from `CursorIcon` |
| Timers and `AppHandle::update` | one `setTimeout` at the earliest deadline, which calls back into the module |
| Animation | `requestAnimationFrame` paces the frame clock, and stops when nothing animates |
| Clipboard | see below |
| File dialogs | see below |

### Clipboard

A page can only read the clipboard during a `paste` event. Ctrl/Cmd+V therefore
reaches the app through that event, carrying the text, rather than as a key.
Copy and cut write with `navigator.clipboard.writeText` and also fill the `copy`
/ `cut` event that follows the key, which works without a secure context.
`stipple::platform::clipboard::text()` returns what the last paste carried, and
`set_text` writes as copying does.

### File dialogs

A browser can't block for a dialog and doesn't reveal paths, so `open_file`,
`save_file` and `pick_folder` return `None` on the web. Use the contents-based
calls, which work on every platform:

```rust
use stipple::platform::dialog::{self, FileDialog};

// In a tap handler:
let handle = cx.handle();
dialog::open_file_contents(&FileDialog::new().with_accept(".txt"), move |file| {
    if let Some(file) = file {
        handle.update(move |s: &mut State| s.text = String::from_utf8_lossy(&file.bytes).into());
    }
});

dialog::save_file_contents(&FileDialog::new().with_file_name("notes.txt"), bytes);
```

- `open_file_contents` opens an `<input type=file>` and returns at once; the
  callback runs once the browser has read the file, and changes the state
  through the `AppHandle`, which re-renders. On the desktops the same call shows
  the native dialog and runs the callback before returning. Call it from an
  input handler: browsers only open a picker in response to user input.
- `save_file_contents` downloads the bytes as `file_name` (`download` if
  empty). On the desktops it shows the save dialog and writes the file.
- `FileDialog::accept` filters the web's picker; the desktop dialogs ignore it,
  and don't pre-fill `file_name` yet.

## How it fits together

`App::run` on the web keeps the event handler and returns; from then on the
page calls into the module for each DOM event, and the module calls a few
functions it imports from the page. Both sides are implementation details
shared by `web_app!` and `stipple.js`, listed here for reference.

The module exports:

| Export | Called for |
|---|---|
| `stipple_alloc(len)` | memory for the page to pass bytes in (taken back by the export that receives them) |
| `stipple_set_font(ptr, len)` | the font's bytes |
| `stipple_start(w, h, ratio)` | building and running the app in a canvas of `w` × `h` CSS pixels |
| `stipple_resize(w, h, ratio)` | size or pixel-ratio changes |
| `stipple_pointer(kind, button, x, y, mods)` | moves, presses, releases and leaving the canvas |
| `stipple_wheel(dx, dy, mode)` | `wheel` |
| `stipple_key(ptr, len, mods)` | `keydown`; returns whether to `preventDefault` |
| `stipple_paste(ptr, len)` | `paste` |
| `stipple_focus(focused)` | focus and blur |
| `stipple_wake()` | the timer |
| `stipple_frame()` | `requestAnimationFrame` |
| `stipple_file(id, …)`, `stipple_file_cancel(id)` | a picked file, or a cancelled picker |

It imports, from the module `"stipple"`: `now`, `present`, `schedule`,
`request_frame`, `set_cursor`, `set_title`, `clipboard_write`, `open_file`,
`save_file` and `log` (panics are printed to the console). See
`src/platform/backend/web.rs`.

`std::time::Instant::now` panics on `wasm32-unknown-unknown`, so Stipple's
timers and frame clock use `stipple::platform::time::Instant`, which is std's
natively and reads `performance.now()` on the web.

## Known gaps

- No input methods (IME) or dead keys: `keydown` delivers what it has, and
  composition is skipped.
- No accessibility tree for screen readers.
- No OS-drawn controls; `Theme::native()` falls back to the self-drawn look,
  and doesn't follow `prefers-color-scheme` yet.
- Window controls (size limits, maximize, fullscreen, icon) do nothing.
- One canvas per page: the backend's state is per module.
