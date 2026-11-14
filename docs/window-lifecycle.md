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
| Android | `onPause` / `onResume` (after a pause) | `onLowMemory`, and `onTrimMemory` at the running-low, critical, moderate and complete levels |
| Desktop, web | not sent | not sent |

On Android, `onPause` waits until the app has handled `Background`, so a
draft saved in the hook is written before the system can stop the process.

## Going back (Android)

Android's Back button (and the back gesture) asks the app to go back first:
close what is on top, a menu, a dialog, then a screen. An open menu closes on
its own; `App::on_back` decides the rest, answering `Handled::Yes` when it went
back and `Handled::No` when there is nothing left to close:

```rust,no_run
use stipple::prelude::*;

struct State {
    screens: Vec<&'static str>,
    dialog: bool,
}
# fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }

App::new(State { screens: vec!["home"], dialog: false }, view)
    .on_back(|s| {
        if s.dialog {
            s.dialog = false;
        } else if s.screens.len() > 1 {
            s.screens.pop();
        } else {
            return Handled::No; // the system takes over
        }
        Handled::Yes
    })
    .run();
```

Unhandled, Back does what it does in any Android app (`onBackPressed`): the
activity finishes, or, on Android 12 and later, a task's root activity moves
to the back. While the soft keyboard is up, the first Back only closes it.
A raw backend handler gets `Event::BackRequested` and calls
`stipple::platform::handle_back()` when it went back.

### Predictive back (Android 13 and later)

Android 13 and later can preview where the back gesture leads (the home
screen, the previous app) while the finger is still down, so they ask *before*
the gesture whether the app will take it. `on_back` only answers after, so say
it beforehand with `App::back_enabled`, from the same state:

```rust,no_run
use stipple::prelude::*;

struct State {
    screens: Vec<&'static str>,
    peek: f32,
}
# fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }

App::new(State { screens: vec!["home"], peek: 0.0 }, view)
    // Something to go back from: Back is the app's.
    .back_enabled(|s| s.screens.len() > 1)
    .on_back(|s| {
        s.peek = 0.0;
        s.screens.pop();
        Handled::Yes
    })
    // The gesture's progress (Android 14 and later), to animate the
    // screen underneath into view.
    .on_back_progress(|s, gesture| match gesture {
        BackGesture::Progressed { progress } => s.peek = progress,
        BackGesture::Cancelled => s.peek = 0.0,
        BackGesture::Started { .. } => {}
    })
    .run();
```

The App asks the hook after each event (an open menu counts as something to
go back from) and tells the system when the answer changes: the backend
registers an `OnBackInvokedCallback` on the activity's
`OnBackInvokedDispatcher` while it's `true`, and takes it away while it's
`false`. Then:

- while it's registered, Back (the key or the gesture) reaches the callback,
  and from it `on_back`. On Android 14 and later it is an
  `OnBackAnimationCallback`, whose gesture arrives as `BackGesture::Started`
  (with the edge), `Progressed` (0 to 1) and either `Cancelled` or the
  `on_back` call, through `App::on_back_progress` (`Event::BackProgress` for a
  raw handler). A Back key press also starts and completes a "gesture";
- while it isn't, the system goes back without asking the app, with its own
  preview animation;
- should `back_enabled` say `true` and `on_back` then answer `Handled::No`,
  the system's default still runs (`onBackPressed`), without its preview.

Without `back_enabled`, an app with an `on_back` hook takes every Back (no
system preview, as before predictive back) and one without takes none. A raw
handler says it with `stipple::platform::set_back_enabled`; until it does, it
gets every Back.

The app opts in with `android:enableOnBackInvokedCallback="true"` on the
manifest's `<application>` (as `androiddemo` does). Without the flag, and
before Android 13, Back stays the key event it was and every Back reaches
`on_back`, whatever `back_enabled` says. While the soft keyboard is up, the
first Back still only closes it (the keyboard's callback sits above the app's;
if the app's was registered after the keyboard came up, it closes the keyboard
itself).

### The back swipe (iOS)

iOS has no system Back. Its equivalent is the interactive swipe from the
screen's leading edge of a `UINavigationController`, which belongs to UIKit's
navigation stack, which Stipple doesn't use; so the backend recognizes the
swipe itself and feeds the same hooks: while `back_enabled` says yes, a finger
that starts at the left edge (the right edge when the theme's layout is
right-to-left) and moves inward is the back gesture.

- It arrives as `BackGesture::Started { edge }`, then `Progressed` as the
  finger moves (the distance over the view's width, 0 to 1), as on Android
  14, through `on_back_progress`.
- Lifting past half way, or flicking inward, goes back: `on_back` runs (a raw
  handler gets `Event::BackRequested`). Taking it back, or lifting early and
  slowly, is `BackGesture::Cancelled`.
- Until it is recognized (24 points from the edge, then 10 inward and mostly
  sideways), the finger is the app's as usual: a tap at the edge taps, and a
  drag up or down scrolls. Once it is, the app's touch is cancelled, as
  UIKit's own edge-pan recognizer does.
- While `back_enabled` says no (and with neither hook) the edge is the app's
  alone. Without `back_enabled`, an app with an `on_back` hook takes the edge
  always, as it takes every Back on Android: give it `back_enabled` so a list
  or a strip at the edge keeps its sideways drags while there is nothing to go
  back from. A raw handler turns it on with
  `stipple::platform::set_back_enabled(true)`; until it does, it's off.

The swipe is recognized from the view's touches rather than by a
`UIScreenEdgePanGestureRecognizer`: the decision is made on the same path as
every touch, so it works the same for the synthetic touches a self-test sends
and is unit-tested on every host. The edge is the view's, which is the safe
area's: in landscape on a phone with a notch, a finger starting in the inset
beside the view isn't seen.

## Keeping the state across process death

A backgrounded app may be ended by the system to free memory, and started
again later as if it never left: the user expects the screen they were on.
`App::save_state` turns that state into bytes when the system asks (Android's
`onSaveInstanceState`, as the app goes to the background), and
`App::restore_state` gets them back when the system starts the app again from
them, before the first frame:

```rust,no_run
use stipple::prelude::*;

struct State {
    screen: u8,
    draft: String,
}
# fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }

App::new(State { screen: 0, draft: String::new() }, view)
    .save_state(|s| {
        let mut bytes = vec![s.screen];
        bytes.extend_from_slice(s.draft.as_bytes());
        bytes
    })
    .restore_state(|s, bytes| {
        if let Some((&screen, draft)) = bytes.split_first() {
            s.screen = screen;
            s.draft = String::from_utf8_lossy(draft).into_owned();
        }
    })
    .run();
```

The format is the app's: there is no serde dependency, and a few bytes by
hand (or a line of text) is usually enough. It's for the session's state (the
screen, a scroll position, an unsent draft), not the user's data, which
belongs in files saved as it changes (or on `Lifecycle::Background`): the
system only restores the state when it ended the app itself, not when the user
closed it. Keep it to a few kilobytes. `restore_state` isn't called on a fresh
start; ignore bytes that don't parse. A raw handler gets `Event::SaveState`
and answers with `stipple::platform::save_state(bytes)`, and reads them back
with `stipple::platform::take_restored_state()`.

| | Saved in | Restored |
|---|---|---|
| Android | `onSaveInstanceState`: `NativeActivity` keeps the bytes in the activity's saved `Bundle` (as a byte array) | `ANativeActivity_onCreate`'s `savedState`, when the activity is recreated: after the process was ended in the background, or in the same process on a configuration change the manifest doesn't declare |
| iOS | a file in the app's `Library/Caches`, under the scene's session, written on `UIApplicationDidEnterBackgroundNotification` (and `WillTerminateNotification`); coming back to the foreground deletes it, and so does UIKit discarding the session (the user swiped the app away) | at the next launch, when the scene reconnects the session that saved it (after a system kill), which reads the file and deletes it |
| Web | the tab's `sessionStorage` (base64), on `pagehide` and when the page is hidden (`visibilitychange`) | when the page loads again in the same tab: a reload, a discarded tab brought back, going back to it in the tab's history |
| Desktop | never asked: desktops don't end apps on their own | |

On iOS the system suspends a backgrounded app and later ends it without a
word, so the state is saved on the way out, every time the app goes to the
background. It follows the platform's rule (Android's too): the state comes
back after the system ended the app, not after the user closed it. The scene
session tells the two apart. The file names the
`UISceneSession.persistentIdentifier` it was saved in; after a system kill
UIKit reconnects that session and the state comes back, while swiping the app
away in the app switcher destroys it: UIKit calls
`application:didDiscardSceneSessions:` (at the next launch, the app being gone
by then), which deletes the file, and the launch connects a new session, which
wouldn't take it anyway (an app can ask for the same thing,
`requestSceneSessionDestruction:`, but UIKit only grants it on a device that
supports multiple scenes, an iPad with `UIApplicationSupportsMultipleScenes`;
an iPhone answers "The current device does not support multiple scenes").
UIKit's identifiers don't always follow that: an iPad simulator gives the
next launch's session the destroyed one's identifier, and reports the discard
after the scene connected, so a discard naming the connected session also
takes back what was restored on connecting (if the app hasn't had it yet).
And when the app asks to destroy its own session, the backend deletes the
file there and then and saves nothing more under that session (unless UIKit
refuses, or the app comes back to the foreground): UIKit takes the request
in its own time and may never report the discard.
This needs the app to use scenes: a
`UIApplicationSceneManifest` in its `Info.plist` (see
[platforms/ios.md](platforms/ios.md#packaging)); without one, iOS doesn't tell
the app about a swipe-away, and the state comes back then too. UIKit's own
state restoration (the scene's `stateRestorationActivity`, or restoration
identifiers on view controllers) wasn't used for the bytes: UIKit writes it
when it chooses, where the file is written before iOS may end the app, and a
self-test can check it. A crash on screen doesn't bring back an old state (the
file goes when the app comes back to the foreground).

On the web, `web/stipple.js` keeps the bytes under a key made of the module's
URL and the canvas's order on the page (`start`'s `stateKey` option names
another, or `false` keeps none); a new tab starts afresh. Closing the tab is
the user closing the app, and `sessionStorage` goes with the tab, so the web
already follows the rule above; a browser that brings a tab back (reopening a
closed tab, restoring its session after a restart) brings its
`sessionStorage` back with it, as it does the page's history.


## Trying it

`cargo run -p lifecycle` opens a window that asks before closing while "work
is running": close it once and it cancels and asks you to close again; close
it again and it checks for a moment, then closes itself. It shows whether it
has the focus, and a second after it loses the focus it rings the bell, asks
for attention and counts the bells in the Dock badge.

## What CI checks

- **Android** (API 34, 33 and 30 emulators, `visual-android`; 33 and 30 run
  the release-dependent checks only): Back reaches
  `androiddemo`, which handles one (resetting its tap count) and leaves the
  next to the system; Home sends it to the background and `am start` brings it
  back; `am send-trim-memory RUNNING_LOW` is a memory warning. Each is checked
  in the app's log. With predictive back on: the callback is registered only
  while the tap count isn't 0, `input keyevent BACK` goes through it (not the
  key event), an edge swipe brings `Started` and the progress and goes back,
  a swipe taken back is `Cancelled` and doesn't, and a Back while the app
  wouldn't take it goes to the system (the app goes to the background without
  being asked). On API 33 the Back key and the edge swipe go through the
  callback without progress; on API 30 (no predictive back) they arrive as
  the Back key, and the declined one reaches the app, which says no, and
  `onBackPressed` sends the activity away. The emulators get a navigation
  bar (`hw.mainKeys=no`): with the default profile's hardware keys
  SystemUI's edge handler is off and no swipe goes back. Process death
  (API 34): with two taps counted, Home saves the state,
  `am kill` ends the process, and starting it again restores the count (in
  the log and in the accessibility tree's "Tapped 2 times").
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
  warning posted in-process arrives as `LowMemory`. The back swipe, with
  synthetic touches through the backend's touch path: from the left edge
  over a strip while the app has nothing to go back from, the strip scrolls;
  with a "screen" open the same swipe brings `Started` and the progress, then
  `on_back`, and the strip doesn't move; a short swipe taken back is
  `Cancelled`. Process death: after the self-test, CI sends the app to the
  background, ends it with `xcrun simctl terminate` and launches it again,
  and the field's text comes back through `restore_state`, in the same scene
  session (this passed in CI before the app used scenes; the session check
  is new). The user closing the app runs on an iPad simulator, booted in the
  same job after the iPhone is shut down, since UIKit only destroys a session
  where the app may have several scenes (the iPhone refuses: "The current
  device does not support multiple scenes"); the bundle is a copy whose
  `Info.plist` adds the iPad to `UIDeviceFamily` and sets
  `UIApplicationSupportsMultipleScenes`. There the kill is checked again
  (`TOUCHDEMO_TYPE` types "For later", which comes back in the same
  session), then the app types something, goes to the background and
  destroys its own scene session (`requestSceneSessionDestruction:`, the
  path a swipe-away in the app switcher takes): the save as it goes is
  declined, the file is gone while it still runs, and the next launch
  restores nothing (reported, not required until it passes). The first run
  failed: UIKit took the request without error but never reported a
  discard, the next launch connected the same session identifier and
  restored the saved state, so the backend now drops the state on the
  request itself, and the step no longer requires a new identifier or a
  discard (both are logged). A real swipe in the app switcher
  isn't scripted: the simulator has no way to drive it short of XCUITest. The recognizer and the saved state's file
  are unit-tested on every host, as is the swipe in front of an `App`.
- **Web** (headless Chrome): `webdemo`'s `index.html#restore` clicks its
  counter three times and reloads the tab; the reloaded app starts with the
  count, from `sessionStorage`.

Not checked anywhere: the system beep (nothing listens on a CI runner), the
attention requests on Windows and macOS, and focus on Windows and macOS.
