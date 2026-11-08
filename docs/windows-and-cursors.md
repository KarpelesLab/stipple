# Window control and cursors

A window can have a minimum and maximum size, open maximized or fullscreen, and
carry an icon. The mouse cursor changes as the pointer moves over elements.

## Window control

Set these before the app runs, on the `App`:

```rust,no_run
use stipple::prelude::*;
# struct State;
# fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
# let icon = Pixmap::new(PhysicalSize::new(64, 64));
App::new(State, view)
    .logical_size(Size::new(640.0, 480.0))
    .min_size(Size::new(400.0, 320.0))
    .max_size(Size::new(1600.0, 1200.0))
    .maximized(false)
    .fullscreen(false)
    .icon(icon) // a straight-alpha RGBA Pixmap, e.g. from Pixmap::decode
    .run();
```

or on the `WindowAttributes` of a window opened with `App::open_window`
(`with_min_size`, `with_max_size`, `with_maximized`, `with_fullscreen`,
`with_icon`). Sizes are in logical pixels, like `logical_size`.

To change the window while the app runs, queue a `WindowRequest` through the
app's handle. It applies to the primary window between events:

```rust,no_run
# use stipple::prelude::*;
# struct State { full: bool }
# fn view(state: &State, cx: &mut Cx<State>) -> Element {
let handle = cx.handle();
button_labeled(cx.theme(), "Fullscreen").on_tap(cx, move |s: &mut State| {
    s.full = !s.full;
    handle.window(WindowRequest::Fullscreen(s.full));
})
# }
```

The requests are `Maximized(bool)`, `Fullscreen(bool)`, `MinSize(Option<Size>)`,
`MaxSize(Option<Size>)` and `Icon(Option<Pixmap>)`. Code that drives a backend
directly calls the same methods on the `Window` trait (`set_min_size`,
`set_max_size`, `set_maximized`, `set_fullscreen`, `set_icon`); backends that
lack a feature ignore it.

How each platform does it:

| | Size limits | Maximize | Fullscreen | Icon |
|---|---|---|---|---|
| X11 | `WM_NORMAL_HINTS` | `_NET_WM_STATE_MAXIMIZED_*` | `_NET_WM_STATE_FULLSCREEN` | `_NET_WM_ICON` (scaled to 128 px at most) |
| Wayland | `xdg_toplevel.set_min_size` / `set_max_size` | `set_maximized` | `set_fullscreen` | `xdg-toplevel-icon-v1`, when the compositor has it |
| Windows | `WM_GETMINMAXINFO` | `ShowWindow(SW_MAXIMIZE)` | borderless, covering the monitor | `WM_SETICON` (big and small) |
| macOS | `setContentMinSize:` / `setContentMaxSize:` | `zoom:` | `toggleFullScreen:` (its own Space) | the application's Dock icon |

On X11 and Wayland the window manager or compositor has the last word: without
one (a bare Xvfb), maximize and fullscreen do nothing. A window that isn't
`resizable` gets its size as both limits on X11 and Wayland. macOS has no
per-window icons, so the icon replaces the app's Dock icon while it runs. iOS,
Android and the web ignore these settings (the web shows cursor shapes, though,
through the canvas's `style.cursor`).

## Cursors

`CursorIcon` names the cursor shapes: `Default`, `Text`, `Pointer`,
`Crosshair`, `NotAllowed`, `Grab`, `Grabbing`, `Move`, and the resize arrows
`EwResize`, `NsResize`, `NwseResize` and `NeswResize`.

Elements pick a cursor from what they do:

- editable text (`text_editor`, `text_area`, `text_field`) shows the I-beam;
- anything with `on_tap` or `on_drag` (buttons, checkboxes, sliders, tabs…)
  shows the pointing hand;
- everything else shows the cursor of the element it sits in, and the arrow at
  the top.

Choose one yourself with `.cursor(...)`, which also covers the element's
children unless they pick their own:

```rust,no_run
# use stipple::prelude::*;
# let theme = Theme::light();
let splitter = Element::boxed(BoxStyle::default())
    .width(4.0)
    .cursor(CursorIcon::EwResize);
let disabled = button_labeled(&theme, "Save").cursor(CursorIcon::NotAllowed);
```

The app sets the cursor when the pointer moves onto an element that wants a
different one. During a drag or a text selection the cursor stays as it was
when the drag started. The scrim behind a modal dialog shows the arrow.

Each backend uses the system's own cursors:

| | How | Notes |
|---|---|---|
| X11 | the core cursor font (`CreateGlyphCursor`) | the classic X shapes, not the desktop's cursor theme (that needs Xcursor theme files, which Stipple doesn't read) |
| Wayland | `wp_cursor_shape_v1` | without it the compositor's default cursor stays |
| Windows | `LoadCursorW` + `WM_SETCURSOR` | no grab cursors: grab is the hand, grabbing and move the four-way arrow |
| macOS | `NSCursor` | diagonal resize and move use AppKit's private cursors when present, else the crosshair and the open hand |

## Trying it

`cargo run -p window -- --controls` adds buttons that maximize the window and
make it fullscreen, a text field, and a strip of boxes, each with another
cursor. `--maximized` and `--fullscreen` open the window that way.

## What CI checks

On X11 (Xvfb), CI reads the window's `WM_NORMAL_HINTS` and `_NET_WM_ICON` back
with `xprop`, and moves the pointer over a button and off it to check that the
cursor changes. Xvfb has no window manager, so maximize and fullscreen aren't
checked there; they were checked by hand under xfwm4. Wayland, Windows and
macOS are build-checked only.
