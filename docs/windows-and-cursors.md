# Windows and cursors

An app can have several windows, open and close them while it runs, retitle,
move and resize them, and read where they are and what state they're in. A
window can have a minimum and maximum size, open maximized or fullscreen, and
carry an icon. The mouse cursor changes as the pointer moves over elements.

<!-- live: window -->

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
(`min_size`, `max_size`, `maximized`, `fullscreen`,
`icon`). Sizes are in logical pixels, like `logical_size`.

A window is also no smaller than its content fits in, and no larger than
its content has a use for where that is finite: `min_size` is a floor
under that and `max_size` a ceiling over it. `layout_limits(false)` turns
that off, and `natural_size(true)` opens the window at its content's
natural size instead of `logical_size`; see
[layout.md](layout.md#window-size-limits).

To change the window while the app runs, queue a `WindowRequest` through the
app's handle. It applies to the primary window between events:

```rust,no_run
# use stipple::prelude::*;
# struct State { full: bool }
# fn view(state: &State, cx: &mut Cx<State>) -> Element {
let handle = cx.handle();
button(cx, "Fullscreen").on_tap(cx, move |s| {
    s.full = !s.full;
    handle.window(WindowRequest::Fullscreen(s.full));
})
# }
```

The requests are `Maximized(bool)`, `Fullscreen(bool)`, `Minimized(bool)`,
`MinSize(Option<Size>)`, `MaxSize(Option<Size>)` (the window's own limits,
which the content's combine with), `Icon(Option<Pixmap>)`,
`Title(String)`, `Position(Point)` (the outer top-left corner),
`InnerSize(Size)`, `ResizeIncrements(Option<Size>)` and `Activate(..)` (see
[below](#bringing-a-window-to-the-front)), and for the app as
a whole (whichever window they're sent to) `KeepAwake(bool)` and
`Orientations(..)`, which `handle.set_keep_awake(..)` and
`handle.set_orientations(..)` send (see
[below](#phones-orientation-and-keeping-awake)). `handle.window(..)`
changes the primary window; `handle.window_to(id, ..)` any window. Code that
drives a backend directly calls the same methods on the `Window` trait
(`set_min_size`, `set_max_size`, `set_maximized`, `set_fullscreen`,
`set_icon`, `set_title`, `set_position`, `set_inner_size`,
`set_resize_increments`, `set_minimized`, `activate`); backends that lack a
feature ignore it.

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
    cx.set_window_title(format!("{name} — Editor"));
    label(cx, name)
}
# fn f(handle: AppHandle<State>) {
let id = handle.open_window(
    WindowAttributes::new().title("Untitled").focus(false),
    editor,
);
// ...
handle.close_window(id);
# }
```

A view passed to `AppHandle::open_window` must be `Send`, as the handle can be
used from other threads. `WindowAttributes::focus(false)` asks for a new
window not to take focus (Wayland leaves that to the compositor).

Closing the last window ends the app, unless
`App::exit_on_last_window_closed(false)`: then the last window is hidden
instead, the app keeps running (timers, the tray icon), new windows can open,
and `AppHandle::exit` ends it. The user closing a window closes just that one.

An event that changes the state in one window re-renders the others too, and
a wake reaches every window (see [async.md](async.md)). Window requests for a
window apply on its next event, which the request itself causes.

An in-app drag (a tab, a list item) goes on over the app's other windows, and
one let go outside them all can open a window of its own (see
[drag-and-drop.md](drag-and-drop.md#across-windows-and-out-of-them) and the
[tab bar](widgets.md#tabs-between-windows)).

A window can belong to another, as a terminal's Preferences or a document's
dialogs do: `WindowAttributes::parent(id)` keeps it above that window, opens
it centered on it and closes it with it. `.modal(true)` also blocks the
parent while it's open: the app drops the parent's pointer, key and close
input, and the platform knows too. `.sheet(true)` makes it a sheet on macOS
(a modal window elsewhere). The `ownedwindows` example opens one of each.

```rust,no_run
# use stipple::prelude::*;
# use stipple::platform::WindowAttributes;
# struct State;
# fn prefs(_: &State, _: &mut Cx<State>) -> Element { spacer() }
# fn f(handle: AppHandle<State>, terminal: WindowId) {
handle.open_window(
    WindowAttributes::new()
        .title("Preferences")
        .parent(terminal)
        .modal(true)
        .sheet(true),
    prefs,
);
# }
```

| | Belonging to a window | Modal |
|---|---|---|
| X11 | `WM_TRANSIENT_FOR` and `_NET_WM_WINDOW_TYPE_DIALOG`: the window manager keeps it above and usually centers it | `_NET_WM_STATE_MODAL` |
| Wayland | `xdg_toplevel.set_parent`, and an `xdg_dialog_v1` where the compositor has `xdg-dialog-v1` (Mutter 46+, KWin 6.1+) | `xdg_dialog_v1.set_modal` (the compositor may dim and attach it); the app drops the parent's input either way |
| Windows | the owner window (`CreateWindowExW`'s parent): above it, minimized with it, no taskbar button; centered on it | the owner disabled (`EnableWindow`) until it closes |
| macOS | a child window (`addChildWindow:ordered:`), centered on it | `.sheet(true)`: `beginSheet:completionHandler:`; else the app only |

| | Several windows | Opening at run time |
|---|---|---|
| X11 | top-level windows on one connection | `CreateWindow` |
| Wayland | one `xdg_toplevel` per window | another `xdg_toplevel` |
| Windows | one `HWND` per window, the same window class | `CreateWindowExW` |
| macOS | one `NSWindow` per window | another `NSWindow` |
| iOS, Android, web | one window: other windows are ignored | — |

### Bringing a window to the front

`handle.window_to(id, WindowRequest::Activate(None))` raises a window and
gives it keyboard focus, restoring it first if it was minimized: the window
with a bell in it, a document that is already open. Desktops guard against
apps stealing focus, so it works best right after the user did something in
the app (the request quotes that input); where the system refuses, the user
is told the window wants attention instead. For a launch the running
instance heard (see [os-integration.md](os-integration.md)), pass the
launch's activation token instead of `None`: the launcher's token is what
lets another app's launch take the focus.

| | How | When the system refuses |
|---|---|---|
| X11 | `_NET_ACTIVE_WINDOW` from the application, with the token's `_TIME` stamp, else the time of the user's last key or button press (or of the app's launch); without a window manager, a raise and `SetInputFocus` | the window manager's own (usually: urgent) |
| Wayland | `xdg_activation_v1` with the token, else with one quoting the last input event's serial and the surface with the keyboard | the compositor marks it urgent |
| Windows | `ShowWindow` (`SW_RESTORE` if minimized), `BringWindowToTop`, `SetForegroundWindow`; only the foreground app (or one it allowed with `AllowSetForegroundWindow`) may take the foreground | the taskbar button flashes (`FlashWindowEx`) |
| macOS | `deminiaturize:`, `activateIgnoringOtherApps:`, `makeKeyAndOrderFront:` | — |

### Launching and who the window belongs to

On Linux a launcher (the dock, the app grid, `gtk-launch`) hands the app a
startup id (`DESKTOP_STARTUP_ID`, X11) or an activation token
(`XDG_ACTIVATION_TOKEN`, Wayland). Stipple takes both out of the environment
as it starts, so the processes the app starts (a terminal's shells) don't
inherit a stale one (`Launch::current()` still reports them, so a launch
forwarded to a running instance carries its token), and uses them for its
first window: on X11 it sets
`_NET_STARTUP_ID` on it and sends the startup-notification `remove` message
once it maps (the launcher's busy cursor stops), on Wayland it activates the
window with the token once it shows. The window manager then focuses the new
window as the launch's, past its focus-stealing prevention.

Docks and taskbars match windows to the app's `.desktop` file. On X11 every
window has `WM_CLASS`: the program's file name (or `RESOURCE_NAME`) as the
instance, and as the class the id given to `App::app_info` (the same id
Wayland's `xdg_toplevel.set_app_id` carries; set `StartupWMClass` to it), or
else the capitalized program name. `_NET_WM_PID` and `WM_CLIENT_MACHINE` say
which process the window is.

## Titles

The title starts as `WindowAttributes::title` (`App::title` for the primary
window). A view can derive it from the state with `cx.set_window_title(..)`; the
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

`restored_position` and `restored_size` are where the window goes back to
when it leaves maximized or fullscreen (its normal bounds; the current ones
while it is neither). An app that saves its windows and brings them back (a
seamless upgrade, a restored session) opens each with them as
`WindowAttributes::position` and `logical_size`, plus `maximized(true)` or
`fullscreen(true)`: a window opened maximized restores to the position and
size it was opened with.

| | Restored bounds |
|---|---|
| X11 | the last geometry in the normal state (when the window manager resized before saying it maximized, the one before it); opened maximized, `_NET_WM_STATE` set before the window maps at its normal size |
| Wayland | the last size in the normal state (no position); the window goes back to it when the compositor leaves the size to it |
| Windows | `GetWindowPlacement`'s `rcNormalPosition` (from workspace to screen coordinates), or the rectangle saved going fullscreen |
| macOS | the frame before `zoom:` or fullscreen (`windowShouldZoom:toFrame:`, `windowWillEnterFullScreen:`) |

Resize increments make the user's resizing snap to whole steps (a terminal's
character cell) counted from the minimum size.

| | Position | Size | Resize increments | State | Work area |
|---|---|---|---|---|---|
| X11 | `ConfigureNotify` (synthetic ones inside a frame), `_NET_FRAME_EXTENTS`; `ConfigureWindow` | `ConfigureWindow` | `PResizeInc` in `WM_NORMAL_HINTS` | `_NET_WM_STATE`; minimize with `WM_CHANGE_STATE` | `_NET_WORKAREA` (the whole desktop's) |
| Wayland | — (none: Wayland hides window positions from apps) | the app picks its size unless the compositor imposes one (tiled, maximized, fullscreen) | — | `xdg_toplevel.configure` states; `set_minimized` (no restore) | — |
| Windows | `GetWindowRect`, `SetWindowPos` | `AdjustWindowRectEx` | `WM_SIZING` | `IsZoomed`, `IsIconic`; `ShowWindow` | `GetMonitorInfoW` |
| macOS | `frame` (flipped to top-left), `setFrameTopLeftPoint:` | `setContentSize:` | `setContentResizeIncrements:` | `isZoomed`, `isMiniaturized`; `miniaturize:` | `visibleFrame` |

## Phones: orientation and keeping awake

**Orientation.** A phone or tablet turns the app as the user turns the
device, as far as the bundle allows (`UISupportedInterfaceOrientations` in
the `Info.plist`, `android:screenOrientation` in the manifest): the window
gets a new size (`Event::Resized`), and on iOS a new safe area, laid out and
drawn inside the rotation's animation. A view picks its portrait or
landscape layout from `cx.window_info().orientation()` (`Portrait` or
`Landscape`: whether it is wider than tall), or from the size itself.

`App::orientations(Orientations::PORTRAIT)` lets it turn only those ways,
and `handle.set_orientations(..)` changes that while the app runs (a game
locking a minigame sideways with `Orientations::LANDSCAPE`, then letting it
turn again with `Orientations::ALL`); turned a way that isn't allowed, the
app turns to one that is. `LANDSCAPE_LEFT` has the top of the device on the
left (turned a quarter counterclockwise). iOS answers the root view
controller's `supportedInterfaceOrientations` with it, asks again with
`setNeedsUpdateOfSupportedInterfaceOrientations` (iOS 16; before, the
system's `attemptRotationToDeviceOrientation`) and turns the scene with
`requestGeometryUpdateWithPreferences:`; the `Info.plist` must list the
orientations too, and an iPad app that allows multitasking (no
`UIRequiresFullScreen`) turns every way regardless. Android calls
`Activity.setRequestedOrientation`, overriding the manifest; keep
`orientation|screenSize` in its `configChanges` so a turn resizes the app
instead of recreating the activity. Desktops and the web ignore it.

**Keeping the screen awake.** `handle.set_keep_awake(true)` keeps the
screen from dimming and locking while the app shows (a game the player
watches, a recipe, a presentation); `false` lets it sleep again. A phone
keeps it only while the app is in the foreground.

| | How |
|---|---|
| iOS | `UIApplication.idleTimerDisabled` |
| Android | `FLAG_KEEP_SCREEN_ON` on the window (`ANativeActivity_setWindowFlags`), no permission needed |
| Web | the Screen Wake Lock API (`navigator.wakeLock.request("screen")`), asked again whenever the page becomes visible; where the browser lacks it, nothing |
| macOS | an IOKit power assertion, `PreventUserIdleDisplaySleep` |
| Windows | `SetThreadExecutionState(ES_CONTINUOUS \| ES_DISPLAY_REQUIRED \| ES_SYSTEM_REQUIRED)`, on the UI thread |
| Linux | the desktop portal's `Inhibit` (idle), else `org.freedesktop.ScreenSaver.Inhibit`, over D-Bus; the inhibition lasts while the connection stays open |

`examples/screendemo` shows all three: its backdrop under the bars, the safe
area outlined, buttons locking it upright or sideways, and a switch keeping
the screen awake.

## Translucent windows

A window can let what is behind it show through: `App::transparent(true)`
(or `WindowAttributes::transparent` for a window opened later) keeps the
frame's alpha all the way to the screen, and wherever the app paints with
less than full alpha the desktop shows. The usual way is a theme whose
background is translucent; the panels, text and controls drawn over it keep
their own colors:

```rust,no_run
# use stipple::prelude::*;
# struct State;
# fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
App::new(State, view)
    .transparent(true)
    .blur(true) // blur what shows through, where the system can
    .theme_with(|_: &State, system: &Theme| {
        let mut theme = *system;
        // 85% opaque.
        theme.palette.background = theme.palette.background.with_alpha(217);
        theme
    })
    .run();
```

`blur(true)` also blurs what shows through, and implies `transparent`. How
each platform does it:

| Platform | Transparency | Blur |
|---|---|---|
| macOS | `NSWindow` not opaque, clear background; the Metal layer not opaque | an `NSVisualEffectView` (behind the window, HUD material) under the content |
| Windows | DWM's frame extended over the whole client area, the frame blitted premultiplied | the acrylic system backdrop (`DWMSBT_TRANSIENTWINDOW`), Windows 11 22H2 and later; none before |
| X11 | a 32-bit ARGB visual and its colormap, when a compositing manager runs (`_NET_WM_CM_S0`); opaque otherwise | `_KDE_NET_WM_BLUR_BEHIND_REGION` (KWin, picom) |
| Wayland | `ARGB8888` buffers with no opaque region | `ext-background-effect-v1` where the compositor can blur, else KWin's `org_kde_kwin_blur` |
| Android | the activity's window in a translucent format (`PixelFormat.TRANSLUCENT`), its buffers `RGBA_8888`, over the wallpaper (`FLAG_SHOW_WALLPAPER`; under a translucent theme, `android:windowIsTranslucent`, the app below shows instead) | `Window.setBackgroundBlurRadius`, Android 12 and later, where the device blurs across windows |

The GPU renderers keep the alpha too, the frame premultiplied:

| Renderer | How |
|---|---|
| Metal (macOS) | a `CAMetalLayer` that isn't opaque |
| Direct3D 11 (Windows) | a premultiplied swap chain made for composition (`CreateSwapChainForComposition`, `DXGI_ALPHA_MODE_PREMULTIPLIED`) and shown over the window through DirectComposition (a flip-model swap chain on a window is always opaque); the window's own surface under it is cleared to transparent |
| Vulkan (X11, Android) | a swapchain with `compositeAlpha` `PRE_MULTIPLIED`, else `INHERIT` (the window's own: premultiplied on X11's ARGB visual and on Android) |
| OpenGL ES (X11, Android) | an EGL window surface whose config has an alpha channel (on X11, that of the window's ARGB visual) |
| Vulkan, OpenGL ES (Wayland) | `dma-buf`s of a format with alpha (`ABGR8888`, `ARGB8888`) instead of `XBGR8888` |

Where the surface can't keep the alpha (a Vulkan driver offering only an
opaque `compositeAlpha`, as NVIDIA's on an X server without its driver
does; no EGL config with alpha; a compositor taking no format with alpha),
that window's frames go through its bitmap instead, which keeps it (read
back from the GPU, or drawn on the CPU), and the app says so once on
stderr; other windows still present straight. iOS and the web ignore both
settings.

`cargo run --example glass` opens a translucent window whose slider sets the
background's opacity (`--blur` blurs what is behind, `--opacity N` starts at
N%).

## Cursors

`CursorIcon` names the cursor shapes: `Default`, `Text`, `Pointer`,
`Crosshair`, `NotAllowed`, `Grab`, `Grabbing`, `Move`, the resize arrows
`EwResize`, `NsResize`, `NwseResize` and `NeswResize`, and `Hidden`, which
hides the pointer over the element (a terminal hiding it while the user types,
a playing video).

Elements pick a cursor from what they do:

- editable text (`text_field`, `text_area`) shows the I-beam;
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
let disabled = button(cx, "Save").cursor(CursorIcon::NotAllowed);
```

The app sets the cursor when the pointer moves onto an element that wants a
different one, and when a new frame asks for another one under a pointer at
rest. During a drag or a text selection the cursor stays as it was when the
drag started. The scrim behind a modal dialog shows the arrow.

To hide the cursor while the user types, as terminals and editors do, keep a
flag in the state: set it from the key handler, clear it on the next pointer
move (an [`on_pointer`](pointer-events.md) `Move`), and pick
`CursorIcon::Hidden` while it's set. `cargo run --example pointerdemo` does this.

A game that hides the cursor to steer with the mouse locks the pointer
instead, which also holds it in place and reports the mouse's motion: see
[Pointer lock](pointer-events.md#pointer-lock).

Each backend uses the system's own cursors:

| | How | Notes |
|---|---|---|
| X11 | the core cursor font (`CreateGlyphCursor`) | the classic X shapes, not the desktop's cursor theme (that needs Xcursor theme files, which Stipple doesn't read); `Hidden` is a blank cursor (`CreateCursor` from an empty bitmap) |
| Wayland | `wp_cursor_shape_v1` | without it the compositor's default cursor stays, and `Hidden` doesn't hide it (`wl_pointer.set_cursor` with no surface hides it, but without cursor shapes nothing could bring it back) |
| Windows | `LoadCursorW` + `WM_SETCURSOR` | no grab cursors: grab is the hand, grabbing and move the four-way arrow; `Hidden` sets no cursor |
| macOS | `NSCursor` | diagonal resize and move use AppKit's private cursors when present, else the crosshair and the open hand; `Hidden` is `[NSCursor hide]`, undone over the title bar and outside the window |
| Web | the canvas's `style.cursor` | `Hidden` is `none` |

## Trying it

`cargo run --example window -- --controls` adds buttons that maximize the window and
make it fullscreen, a text field, and a strip of boxes, each with another
cursor. `--maximized` and `--fullscreen` open the window that way.

## What CI checks

On the iOS simulator, `screendemo`'s self-test reads the safe area, locks
itself sideways (the simulator's interface turns), checks the view
followed, turns back upright and checks the idle timer is off after
`set_keep_awake`. On the Android emulator, `androiddemo` (edge to edge,
upright only, kept awake) reports a non-zero safe area, and `dumpsys`
shows the requested orientation and `FLAG_KEEP_SCREEN_ON`. The iOS one
is required; the Android one is non-blocking for now. Keeping the screen awake on the desktops is
build-checked only.

On X11 (Xvfb), CI reads the window's `WM_NORMAL_HINTS` and `_NET_WM_ICON` back
with `xprop`, and moves the pointer over a button and off it to check that the
cursor changes. Xvfb has no window manager, so maximize and fullscreen aren't
checked there; they were checked by hand under xfwm4. It also types into
`pointerdemo` and checks the app asks for the hidden cursor and back (the
blank cursor itself isn't looked at). Wayland, Windows and macOS are
build-checked only.

Translucent windows: CI runs `glass` at 0% and 100% background opacity
under sway, under Xvfb with `xcompmgr` as the compositing manager, and on
Windows, and checks the background differs (required on Windows and under
`xcompmgr`, a warning under sway for now); it was
looked at by hand on macOS, with and without blur. Blur is checked only by
hand, on macOS.

`multiwindow --selftest` changes its windows while it runs: the first
window's title follows the state, the second is retitled, moved to 360,260
and resized to 400x300, the first gets 8x16 resize increments, and a third
window opens and then closes; then the second is maximized (its restored
bounds must be 360,260 and 400x300, checked on Windows and macOS) and the
first brought to the front (on X11, with no window manager, `xdotool
getwindowfocus` must name it). It is started with a startup id and an
activation token, which must be gone from its environment on Linux, and on
X11 `xprop` reads `WM_CLASS`, `_NET_WM_PID` and `_NET_STARTUP_ID` back (see
`.github/scripts/runtime-windows.sh`). CI
runs it on X11 (titles read back with `xprop`, increments from
`WM_NORMAL_HINTS`, geometry with `xdotool`, the third window found and gone),
on Wayland under sway (titles and the third window in `swaymsg -t get_tree`;
sway tiles, so geometry isn't checked), and on Windows and macOS, where the
backend prints what the OS reads back (`STIPPLE_DEBUG_WINDOWS=1`). The X11
geometry with a reparenting window manager (frame extents, synthetic
`ConfigureNotify`) was checked by hand under xfwm4. App-level behavior
(which window gets what, closing the last window, re-rendering the others) is
unit-tested with a pretend multi-window backend.
