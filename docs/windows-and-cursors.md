# Windows and cursors

An app can have several windows, open and close them while it runs, retitle,
move and resize them, and read where they are and what state they're in. A
window can have a minimum and maximum size, open maximized or fullscreen, and
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

The requests are `Maximized(bool)`, `Fullscreen(bool)`, `Minimized(bool)`,
`MinSize(Option<Size>)`, `MaxSize(Option<Size>)`, `Icon(Option<Pixmap>)`,
`Title(String)`, `Position(Point)` (the outer top-left corner),
`InnerSize(Size)` and `ResizeIncrements(Option<Size>)`. `handle.window(..)`
changes the primary window; `handle.window_to(id, ..)` any window. Code that
drives a backend directly calls the same methods on the `Window` trait
(`set_min_size`, `set_max_size`, `set_maximized`, `set_fullscreen`,
`set_icon`, `set_title`, `set_position`, `set_inner_size`,
`set_resize_increments`, `set_minimized`); backends that lack a feature
ignore it.

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

## Several windows

`App::open_window` registers windows before the app runs, and
`AppHandle::open_window` opens one while it runs, from any thread. Both return
the window's `WindowId`; the first window is `WindowId::PRIMARY`. Every window
has its own view onto the one shared state, and `cx.window_id()` tells a view
which window it is building, so one view function can serve many windows:

```rust,no_run
# use stipple::prelude::*;
# struct State { docs: Vec<(WindowId, String)> }
fn editor(s: &State, cx: &mut Cx<State>) -> Element {
    let id = cx.window_id();
    let name = s.docs.iter().find(|(w, _)| *w == id).map_or("", |(_, n)| n);
    cx.window_title(format!("{name} — Editor"));
    label(cx.theme(), name)
}
# fn f(handle: AppHandle<State>) {
let id = handle.open_window(
    WindowAttributes::new().with_title("Untitled").with_focus(false),
    editor,
);
// ...
handle.close_window(id);
# }
```

A view passed to `AppHandle::open_window` must be `Send`, as the handle can be
used from other threads. `WindowAttributes::with_focus(false)` asks for a new
window not to take focus (Wayland leaves that to the compositor).

Closing the last window ends the app, unless
`App::exit_on_last_window_closed(false)`: then the last window is hidden
instead, the app keeps running (timers, the tray icon), new windows can open,
and `AppHandle::exit` ends it. The user closing a window closes just that one.

An event that changes the state in one window re-renders the others too, and
a wake reaches every window (see [async.md](async.md)). Window requests for a
window apply on its next event, which the request itself causes.

| | Several windows | Opening at run time |
|---|---|---|
| X11 | top-level windows on one connection | `CreateWindow` |
| Wayland | one `xdg_toplevel` per window | another `xdg_toplevel` |
| Windows | one `HWND` per window, the same window class | `CreateWindowExW` |
| macOS | one `NSWindow` per window | another `NSWindow` |
| iOS, Android, web | one window: other windows are ignored | — |

## Titles

The title starts as `WindowAttributes::title` (`App::title` for the primary
window). A view can derive it from the state with `cx.window_title(..)`; the
window is retitled whenever the title a build gives changes. A
`WindowRequest::Title` sets it directly.

| | Title |
|---|---|
| X11 | `_NET_WM_NAME` (`UTF8_STRING`) and `WM_NAME` (Latin-1 `STRING` when the title fits, else `UTF8_STRING`) |
| Wayland | `xdg_toplevel.set_title` |
| Windows | `SetWindowTextW` |
| macOS | `setTitle:` |

## Position, size and state

`WindowInfo` says where a window is and what state it's in: its outer top-left
`position`, `inner_size` and `outer_size` (with the frame), `scale`,
`maximized`, `minimized`, `fullscreen` and the `work_area` of its monitor, all
in logical pixels. Read it:

- in a view, with `cx.window_info()` (the view is built again when it
  changes);
- anywhere, with `handle.window_info(id)` (as of the last event the app
  handled for that window), and `handle.windows()` lists the open ones;
- as it changes, with `App::on_window_change(|state, id, info| ..)`.

Backends report moves and state changes as `Event::WindowChanged` and resizes
as `Event::Resized`.

Resize increments make the user's resizing snap to whole steps (a terminal's
character cell) counted from the minimum size.

| | Position | Size | Resize increments | State | Work area |
|---|---|---|---|---|---|
| X11 | `ConfigureNotify` (synthetic ones inside a frame), `_NET_FRAME_EXTENTS`; `ConfigureWindow` | `ConfigureWindow` | `PResizeInc` in `WM_NORMAL_HINTS` | `_NET_WM_STATE`; minimize with `WM_CHANGE_STATE` | `_NET_WORKAREA` (the whole desktop's) |
| Wayland | — (none: Wayland hides window positions from apps) | the app picks its size unless the compositor imposes one (tiled, maximized, fullscreen) | — | `xdg_toplevel.configure` states; `set_minimized` (no restore) | — |
| Windows | `GetWindowRect`, `SetWindowPos` | `AdjustWindowRectEx` | `WM_SIZING` | `IsZoomed`, `IsIconic`; `ShowWindow` | `GetMonitorInfoW` |
| macOS | `frame` (flipped to top-left), `setFrameTopLeftPoint:` | `setContentSize:` | `setContentResizeIncrements:` | `isZoomed`, `isMiniaturized`; `miniaturize:` | `visibleFrame` |

## Cursors

`CursorIcon` names the cursor shapes: `Default`, `Text`, `Pointer`,
`Crosshair`, `NotAllowed`, `Grab`, `Grabbing`, `Move`, the resize arrows
`EwResize`, `NsResize`, `NwseResize` and `NeswResize`, and `Hidden`, which
hides the pointer over the element (a terminal hiding it while the user types,
a playing video).

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
different one, and when a new frame asks for another one under a pointer at
rest. During a drag or a text selection the cursor stays as it was when the
drag started. The scrim behind a modal dialog shows the arrow.

To hide the cursor while the user types, as terminals and editors do, keep a
flag in the state: set it from the key handler, clear it on the next pointer
move (an [`on_pointer`](pointer-events.md) `Move`), and pick
`CursorIcon::Hidden` while it's set. `cargo run -p pointerdemo` does this.

Each backend uses the system's own cursors:

| | How | Notes |
|---|---|---|
| X11 | the core cursor font (`CreateGlyphCursor`) | the classic X shapes, not the desktop's cursor theme (that needs Xcursor theme files, which Stipple doesn't read); `Hidden` is a blank cursor (`CreateCursor` from an empty bitmap) |
| Wayland | `wp_cursor_shape_v1` | without it the compositor's default cursor stays, and `Hidden` doesn't hide it (`wl_pointer.set_cursor` with no surface hides it, but without cursor shapes nothing could bring it back) |
| Windows | `LoadCursorW` + `WM_SETCURSOR` | no grab cursors: grab is the hand, grabbing and move the four-way arrow; `Hidden` sets no cursor |
| macOS | `NSCursor` | diagonal resize and move use AppKit's private cursors when present, else the crosshair and the open hand; `Hidden` is `[NSCursor hide]`, undone over the title bar and outside the window |
| Web | the canvas's `style.cursor` | `Hidden` is `none` |

## Trying it

`cargo run -p window -- --controls` adds buttons that maximize the window and
make it fullscreen, a text field, and a strip of boxes, each with another
cursor. `--maximized` and `--fullscreen` open the window that way.

## What CI checks

On X11 (Xvfb), CI reads the window's `WM_NORMAL_HINTS` and `_NET_WM_ICON` back
with `xprop`, and moves the pointer over a button and off it to check that the
cursor changes. Xvfb has no window manager, so maximize and fullscreen aren't
checked there; they were checked by hand under xfwm4. It also types into
`pointerdemo` and checks the app asks for the hidden cursor and back (the
blank cursor itself isn't looked at). Wayland, Windows and macOS are
build-checked only.

`multiwindow --selftest` changes its windows while it runs: the first
window's title follows the state, the second is retitled, moved to 360,260
and resized to 300x200, the first gets 8x16 resize increments, and a third
window opens and then closes (see `.github/scripts/runtime-windows.sh`). CI
runs it on X11 (titles read back with `xprop`, increments from
`WM_NORMAL_HINTS`, geometry with `xdotool`, the third window found and gone),
on Wayland under sway (titles and the third window in `swaymsg -t get_tree`;
sway tiles, so geometry isn't checked), and on Windows and macOS, where the
backend prints what the OS reads back (`STIPPLE_DEBUG_WINDOWS=1`). The X11
geometry with a reparenting window manager (frame extents, synthetic
`ConfigureNotify`) was checked by hand under xfwm4. App-level behavior
(which window gets what, closing the last window, re-rendering the others) is
unit-tested with a pretend multi-window backend.
