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
cargo build --release -p webdemo --target wasm32-unknown-unknown
```

Serve these files from one directory:

- `examples/webdemo/index.html`, the page (and `multi.html`, two copies of
  the app side by side);
- `web/stipple.js`;
- `target/wasm32-unknown-unknown/release/webdemo.wasm`;
- `font.ttf`, the font the page loads. Without it, text doesn't render.

```sh
python3 -m http.server --directory site
```

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
| Rendering | `putImageData` of the frame, limited to the damaged rects |
| High-DPI | `devicePixelRatio` is the scale factor; the canvas's backing store is its CSS size times the ratio. A `ResizeObserver` and a `resolution` media query follow size and ratio changes (`Event::Resized`, `Event::ScaleFactorChanged`) |
| Pointer | pointer events, captured while a button is down; right-click opens context menus instead of the browser's |
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
`inputmode` between `text` and `none`, which shows or hides a touch keyboard.
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
  of its node, which runs the node's tap handler (or focuses a field).
- **Focus, app to page.** When the app's focus moves to a text field, the
  textarea takes keyboard focus and is labelled by the field's element
  (`aria-labelledby`); for any other focused node, its own element takes it.
  This only happens while focus is already in the app, so the page's focus is
  never stolen.
- **Focus, page to app.** Interactive elements are focusable (`tabindex=-1`);
  when a screen reader moves focus to a text field's element, the field is
  focused in the app.

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
| `stipple_ime(kind, ptr, len)` | composition: 0 the preedit, 1 committed text |
| `stipple_color_scheme(dark, accent)` | `prefers-color-scheme`, and the accent as `0xRRGGBB` (negative for none): before `stipple_start`, and on changes |
| `stipple_reduced_motion(reduce)` | `prefers-reduced-motion: reduce`: before `stipple_start`, and on changes |
| `stipple_focus(focused)` | focus entering and leaving the app |
| `stipple_wake()` | the timer |
| `stipple_frame()` | `requestAnimationFrame` |
| `stipple_file(id, …)`, `stipple_file_cancel(id)` | a picked file, or a cancelled picker |

It imports, from the module `"stipple"`: `now`, `present`, `schedule`,
`request_frame`, `set_cursor`, `set_title`, `clipboard_write`, `open_file`,
`save_file`, `open_url` (`window.open`, for `platform::open_url`), `notify`
(the `Notification` API, for `platform::notify`), `log` (panics are printed to
the console), `set_a11y` (the accessibility tree as JSON), `set_ime` (the caret
rect, or none) and `set_text_input`. See `src/platform/backend/web.rs`.

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
