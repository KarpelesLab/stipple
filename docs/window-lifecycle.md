# Closing, quitting, focus and attention

An app can ask before a window closes or the app quits, follow which window
has the keyboard focus, and ask for the user's attention (flash the taskbar,
bounce the Dock icon, sound the bell). On Wayland, Stipple also draws the
window's title bar when the compositor won't.

## Asking before a window closes

Closing a window (its close button, Alt+F4, ⌘W, the window manager) asks
`App::on_close_requested` first. The hook gets the state and the window's id,
and answers `CloseResponse::Close` or `CloseResponse::Cancel`:

```rust,no_run
use stipple::platform::WindowId;
use stipple::prelude::*;

struct State {
    running: usize,
    confirm: Option<WindowId>,
}
# fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }

App::new(State { running: 2, confirm: None }, view)
    .on_close_requested(|s, id| {
        if s.running == 0 {
            CloseResponse::Close
        } else {
            s.confirm = Some(id); // the view shows "Close anyway?"
            CloseResponse::Cancel
        }
    })
    .run();
```

The hook may change the state (the view is rebuilt), so cancelling usually
shows a confirmation. When the user confirms, or a check made in the
background comes back, close the window yourself with
`AppHandle::close_window(id)`, which doesn't ask again. Closing the last
window ends the app, as when the user closes it (unless
`App::exit_on_last_window_closed(false)`). The id is the app's own for the
window: `WindowId::PRIMARY` for the first, what `App::open_window` /
`AppHandle::open_window` returned for the others, and what the view reads
with `Cx::window_id()` (see [windows-and-cursors.md](windows-and-cursors.md#several-windows)).

## Asking before the app quits

Quitting the app as a whole (⌘Q, the Dock's Quit and logging out on macOS,
ending the session on Windows) asks `App::on_quit_requested`, which answers
the same way. Without a quit hook, a quit asks the close hook for each open
window and goes ahead only if every one of them closes. To quit later, call
`AppHandle::exit()`.

| | Close | Quit |
|---|---|---|
| X11 | `WM_DELETE_WINDOW` | — |
| Wayland | `xdg_toplevel.close`, and the close button Stipple draws | — |
| Windows | `WM_CLOSE` | `WM_QUERYENDSESSION` |
| macOS | `windowShouldClose:` | the Quit menu item (⌘Q), and `applicationShouldTerminate:` (the Dock, logging out) |

The operating system wants its answer to a logout on the spot, before the app
has had a chance to ask the user. So while the app has a hook, Windows and
macOS refuse at once and hand the request to the app; the user logs out again
once the app has quit (Windows shows that the app is holding up the logout,
with a button to go on anyway). Without a hook the session ends as usual.

## Window focus

`Cx::window_focused()` tells the view whether its window has the keyboard
focus (it is the active window), and the view is rebuilt when that changes:
draw a hollow caret while it hasn't, as a terminal does. `App::on_window_focus`
hears each change with the window's id, to send a terminal's focus reports or
to clear a bell once the user looks:

```rust,no_run
# use stipple::prelude::*;
# struct State { focused: bool, bells: u32 }
# fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
App::new(State { focused: true, bells: 0 }, view)
    .on_window_focus(|s, _id, focused| {
        s.focused = focused;
        if focused {
            s.bells = 0;
        }
    })
    .run();
```

| | Focus comes from |
|---|---|
| X11 | `FocusIn` / `FocusOut` (not those from keyboard grabs, such as a window manager's Alt+Tab) |
| Wayland | the toplevel's `activated` state |
| Windows | `WM_SETFOCUS` / `WM_KILLFOCUS` |
| macOS | `windowDidBecomeKey:` / `windowDidResignKey:` |
| Android, web | the activity's and the page's focus |

A window counts as focused until the platform says otherwise; X11 and Wayland
start every window unfocused until the server or compositor focuses it. iOS
doesn't report focus.

## Attention, the bell and the badge

Three calls on the app's handle (or `WindowRequest`s for `AppHandle::window`)
go to the primary window:

- `request_attention(UserAttention::Informational | Critical)` asks for the
  user's attention without taking the focus. The request ends when the window
  is activated.
- `beep()` sounds the system's alert sound, as an audible bell.
- `set_badge(Some("3"))` shows a short label on the application's icon, and
  `set_badge(None)` removes it.

| | Attention | Beep | Badge |
|---|---|---|---|
| X11 | the ICCCM urgency hint (`WM_HINTS`) and `_NET_WM_STATE_DEMANDS_ATTENTION`, both cleared on focus | the core `Bell` request | — |
| Wayland | an `xdg_activation_v1` activation, which compositors show as urgent for a window that isn't focused | `xdg_system_bell_v1`, where the compositor has it; else nothing (Wayland has no other bell) | — |
| Windows | `FlashWindowEx`: the taskbar button flashes three times, or until the window comes to the front for `Critical` | `MessageBeep` | — |
| macOS | `requestUserAttention:`: the Dock icon bounces once, or until the app is activated | `NSBeep` | the Dock tile's `badgeLabel` |

The Windows taskbar's overlay icon (`ITaskbarList3::SetOverlayIcon`) and the
Unity `LauncherEntry` badge on Linux aren't wired up.

## Window decorations on Wayland

On X11, Windows and macOS the system draws the title bar. On Wayland it's
negotiated: where the compositor offers `zxdg_decoration_manager_v1` (KDE,
sway and the other wlroots compositors), Stipple asks it to draw them
(`server_side`). Where it doesn't (GNOME's Mutter, Weston), or it answers
`client_side`, Stipple draws them itself:

- a title bar above the content, with the window's title and minimize,
  maximize and close buttons, light or dark like the window's title bar;
- dragging the title bar moves the window (`xdg_toplevel.move`), a
  double-click maximizes or restores it, and a right-click opens the
  compositor's window menu (`show_window_menu`);
- an invisible 8-pixel margin around the window resizes it from its edges and
  corners (`xdg_toplevel.resize`), with the matching cursor;
- maximized or tiled, the margin goes; fullscreen, the title bar goes too.

The title bar and margin live in the window's buffer, around the content
(`set_window_geometry` tells the compositor where the window proper is). The
app still sees only its content: its size, and pointer positions within it.

`WindowAttributes::with_decorations(false)` turns the decorations off: no title
bar at all, on Wayland (Stipple asks for none and draws none), X11
(`_MOTIF_WM_HINTS`) and Windows (a popup window with a sizing border).

## Going to the background (mobile)

On a phone the user leaves the app by going home or switching apps, and comes
back later, or not: the system may end a backgrounded app to free memory.
`App::on_lifecycle` hears each change as a `Lifecycle` (also delivered to the
handler as `Event::Lifecycle`):

```rust,no_run
use stipple::prelude::*;

struct State {
    draft: String,
    thumbnails: Vec<Pixmap>,
}
# fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }

App::new(State { draft: String::new(), thumbnails: Vec::new() }, view)
    .on_lifecycle(|s, change| match change {
        // Save now: the process may not come back.
        Lifecycle::Background => {
            let _ = std::fs::write("draft.txt", &s.draft);
        }
        // Drop what can be rebuilt.
        Lifecycle::LowMemory => s.thumbnails.clear(),
        _ => {}
    })
    .run();
```

- `Background`: the app left the screen. From then on the App asks for no
  animation frames (the display link stops) and doesn't build or repaint, even
  when timers or `AppHandle` updates change the state; they still run for as
  long as the OS lets the process run.
- `Foreground`: back on screen. Every window rebuilds and repaints, and
  animations ask for frames again.
- `LowMemory`: the system is short of memory. The App drops its cached
  OS-drawn controls; drop your own caches.

The hook may change the state; the view is rebuilt (when on screen).

| | Background / Foreground | Low memory |
|---|---|---|
| iOS | `UIApplicationDidEnterBackgroundNotification` / `WillEnterForegroundNotification` | `UIApplicationDidReceiveMemoryWarningNotification` |
| Android | not sent yet | not sent yet |
| Desktop, web | not sent | not sent |

## Trying it

`cargo run -p lifecycle` opens a window that asks before closing while "work
is running": close it once and it cancels and asks you to close again; close
it again and it checks for a moment, then closes itself. It shows whether it
has the focus, and a second after it loses the focus it rings the bell, asks
for attention and counts the bells in the Dock badge.

## What CI checks

- **X11** (Xvfb, with a script standing in for the window manager): a
  `WM_DELETE_WINDOW` the app cancels, then one it confirms, after which the app
  closes the window itself; focus moving to another window and back; the
  urgency hint set by the bell (`xprop WM_HINTS`) and cleared on focus.
- **Wayland** (headless sway): the window asks for and gets server-side
  decorations; focus moves to another window and back (`activated`);
  `swaymsg kill` (`xdg_toplevel.close`) is cancelled, then confirmed. Whether
  sway marks the window urgent after the activation request is reported, not
  required.
- **Wayland client-side decorations** (headless Weston, which has no
  decoration manager): the app presents a decorated buffer of the expected
  size, and a screenshot is kept. The buttons, moving and resizing aren't
  driven (the headless compositor has no pointer); they are unit-tested
  against a scripted compositor, as are the decoration negotiation and the
  activation request.
- **Windows**: `WM_QUERYENDSESSION` is refused and reaches the app, then
  `WM_CLOSE` is confirmed and the app closes its window.
- **macOS**: an in-process self-test sends `performClose:` and `terminate:`
  through AppKit, which reach the app through the delegates, and sets the Dock
  badge and reads it back.

- **iOS** (the simulator): `touchdemo`'s self-test is sent to the background
  (CI launches another app) and brought back (launched again), and checks it
  heard `Background` and `Foreground` and built no view in between; a memory
  warning posted in-process arrives as `LowMemory`.

Not checked anywhere: the system beep (nothing listens on a CI runner), the
attention requests on Windows and macOS, and focus on Windows and macOS.
