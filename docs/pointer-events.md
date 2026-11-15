# Pointer events

Taps, drags and text selection cover most widgets. A custom element that
draws its own content, such as a terminal (mouse reporting, selection,
middle-click paste), a canvas or a map, needs the raw pointer: every button,
every move, the modifier keys, click counts and the wheel as the device
reports it. `Element::on_pointer` and `Element::on_wheel` give it that, and
`Element::on_pinch` gives it two-finger gestures on touch screens (see
[Touch gestures](#touch-gestures)). A game or a 3D view can lock the pointer
and read the mouse's motion instead (see [Pointer lock](#pointer-lock)).

<!-- live: pointerdemo -->

```rust
use stipple::prelude::*;

#[derive(Default)]
struct Term {
    selecting: Option<u32>, // the click count of the selection under way
    scrolled: f64,          // lines, accumulated
}

fn view(_: &Term, cx: &mut Cx<Term>) -> Element {
    Element::boxed(BoxStyle::default())
        .on_pointer(cx, |ev: &PointerEvent, t: &mut Term| {
            match (ev.kind, ev.button) {
                // 1: by character, 2: by word, 3: by line.
                (PointerEventKind::Down, Some(PointerButton::Left)) => {
                    t.selecting = Some(ev.click_count);
                }
                (PointerEventKind::Down, Some(PointerButton::Middle)) => {
                    // Paste the PRIMARY selection here.
                }
                (PointerEventKind::Up, Some(PointerButton::Left)) => t.selecting = None,
                _ => {}
            }
        })
        .on_wheel(cx, |ev: &WheelEvent, t: &mut Term| {
            t.scrolled += match ev.unit {
                stipple::platform::ScrollUnit::Lines => ev.dy,
                stipple::platform::ScrollUnit::Pixels => ev.dy / 16.0, // a cell's height
            };
        })
}
```

## Pointer events

The handler gets a `PointerEvent` and the app state:

| Field | |
|---|---|
| `kind` | `Down`, `Up`, `Move` or `Leave`, and `Motion` while the [pointer is locked](#pointer-lock) |
| `button` | the button that went down or up: `Left`, `Right`, `Middle`, `Back`, `Forward` or `Other(n)`; `None` for moves and leaves |
| `buttons_held` | the buttons down after this event (`contains`, `is_empty`) |
| `position` | the pointer in the element's own coordinates: logical pixels from its top-left corner, through any [transform](transforms.md) on the way to it |
| `window_position` | the pointer in the window |
| `size` | the element's laid-out size |
| `modifiers` | Shift, Ctrl, Alt and the logo key, held at this event |
| `click_count` | for a press, 1, 2 or 3 for a single, double or triple click (then 1 again); a release has its press's count; moves and leaves 0 |
| `movement` | for `Motion`, how far the mouse moved; zero otherwise |

Which element gets them:

- **Moves** go to the top-most element with a pointer handler under the
  pointer. When the pointer moves onto another element, or off it, the one it
  was over gets a `Leave` first.
- **A press captures the pointer.** The element it went down on gets every
  move and release until the last button is up, even outside it and outside
  the window (where `position` is outside `size`; `is_inside()` tells). It
  gets no `Leave` while it holds the capture: if the pointer is elsewhere when
  the last button comes up, the `Leave` follows the `Up`.
- **The pointer leaving the window** sends the hovered element its `Leave`.

Presses count as a double or triple click when they are of the same button,
close in time and place: the platform's double-click time and distance where
Stipple can read them (`GetDoubleClickTime` and `SM_CXDOUBLECLK` on Windows,
`NSEvent.doubleClickInterval` on macOS, the X settings daemon's
`Net/DoubleClickTime` and `Net/DoubleClickDistance` on X11), else 500 ms and
4 px. `stipple::platform::double_click()` reads them.

Pointer events come in addition to the app's own handling, and before it: a
press on a focusable element still focuses it (so a terminal with `on_key`
gets the keyboard when clicked), a tap handler on it or around it still runs,
and a right-click still opens a context menu. Give the element only
`on_pointer` (and `on_key`) to have the pointer to itself.

## Wheel events

The `on_wheel` handler gets a `WheelEvent`: `dx` and `dy` (positive towards
the end of the content: down, right) in `unit`s, the `phase`, `position` in the
element's coordinates, `window_position` and `modifiers`.

- `ScrollUnit::Lines`: a wheel's notches, one per click (fractions from
  high-resolution wheels, and from precision touchpads on Windows). Scroll that
  many lines.
- `ScrollUnit::Pixels`: a trackpad or another precise device, in logical
  pixels. Accumulate them and scroll a line each time they add up to one.
- `phase`: `Started`, `Moved`, `Ended` and `Momentum` for trackpad gestures on
  macOS (and `Started`/`Moved`/`Ended` for finger scrolling on Wayland); `None`
  for wheels and where the platform doesn't say. A gesture's `Ended` usually
  carries no delta.

Horizontal scrolling arrives as `dx`: a tilting wheel, a trackpad, Shift+wheel
on some platforms, and buttons 6 and 7 on X11.

An element with `on_wheel` takes the wheel over itself instead of the scroll
container it is in; a scroll container inside it still takes the wheel over
itself. (A `swipe_row` takes only sideways scrolls, and a gesture's
deltaless end; vertical ones go on to its list.) Scroll containers scroll a line as 40 logical pixels
(`stipple::platform::SCROLL_LINE_PX`).

## Touch gestures

On a touch screen (iOS, Android, and phones' and tablets' browsers) every
finger arrives as an `Event::Touch`, and the first one down is also the
pointer: a left-button press, its moves and a release, which `on_pointer`
sees like a mouse's. The App runs the touches through one gesture recognizer
in the shared core (`core::touch`), so every platform behaves the same:

- **Scrolling.** A finger dragged in a scroll container scrolls it, and
  flings it on, slowing down, if it lifts while moving (no fling under reduced
  motion). A press only becomes a scroll once the finger has moved 8 logical
  pixels, so taps still tap; then the pressed widget lets go. The direction
  the finger moved picks the innermost container along that axis: sideways
  strips (`widgets::scroll_horizontal`, or any horizontal stack made
  `scrollable`) scroll sideways, and inside a vertical list a vertical drag
  still scrolls the list. With no container along the drag, the widget under
  the finger keeps it (a slider drags). A finger landing on a fling stops it
  without tapping.
- **Pulling a widget.** A finger dragged *down* in a vertical container
  that is scrolled to its top, inside a bottom sheet or a
  `pull_to_refresh`, pulls that widget instead (the sheet down, the refresh
  indicator out). The widget takes the pointer from there: if an element
  inside it had the press (a swipeable row), that element gets a release
  first, then the widget a press where the finger is, and the moves.
- **List rows swiped sideways.** A `swipe_row` watches its press's moves
  too, and decides on the same move as the recognizer: a mostly sideways
  move with no sideways container to scroll swipes the row; a vertical
  one (or one the recognizer took for a scroll) leaves it alone. See
  [lists-and-tables.md](lists-and-tables.md#swipe-actions).
- **Long press.** A finger held within those 8 pixels for half a second over
  an element with a `context_menu` (or an `on_context` handler) opens the
  menu where the finger is, as a right-click does with a mouse. The press is
  cancelled, so lifting the finger taps nothing (nor the menu item under it);
  a tap then picks from the menu. Moving first, or lifting before the delay,
  is a scroll or a tap as usual. Elsewhere, holding a button and lifting
  still taps it, as on iOS and Android.
- **Pinch, two-finger pan and rotation.** A second finger landing while the
  first is still pressing (or scrolling) starts a pinch when the element
  under the midpoint has `on_pinch`; its handler gets a `PinchEvent` for the
  start, each move and the end:

| Field | Meaning |
|---|---|
| `phase` | `Began` (scale 1), `Changed`, `Ended` (a finger lifted), `Cancelled` (the system took the touches) |
| `scale` | the distance between the fingers over the distance at `Began` |
| `rotation` | how far the line between them turned since `Began`, in radians, clockwise on screen |
| `translation` | how far their midpoint moved since `Began` (a two-finger pan) |
| `centroid`, `window_centroid` | the midpoint, in the element's coordinates and the window's |
| `size` | the element's size |

  Each value is measured from where the gesture began, so a handler keeps
  what it had at `Began` and applies them to that:

```rust
use stipple::core::{PinchEvent, PinchPhase};
Element::boxed(BoxStyle::default()).on_pinch(cx, |ev: &PinchEvent, s: &mut Photo| {
    if ev.phase == PinchPhase::Began {
        s.start = (s.zoom, s.pan);
    }
    s.zoom = (s.start.0 * ev.scale).clamp(0.5, 8.0);
    s.pan = s.start.1 + ev.translation;
})
```

  The first finger's press is cancelled when the pinch begins (no tap), and
  when either finger lifts the pinch ends; the other is ignored until it
  lifts too. Without an `on_pinch` element under them, a second finger is
  ignored and the first goes on scrolling or pressing. Further fingers are
  ignored.

| | Fingers | Long press opens the menu | Pinch |
|---|---|---|---|
| iOS | `touchesBegan:` and friends (`multipleTouchEnabled`) | ✅ | ✅ |
| Android | `AMotionEvent` pointers (`POINTER_DOWN` / `UP`) | ✅ | ✅ |
| Web | `touchstart` / `touchmove` / `touchend` / `touchcancel` (the canvas has `touch-action: none`) | ✅ | ✅ |
| Desktops | — (a touch screen there drives the mouse) | — | — |

`App::touch` feeds the primary window a synthetic `Touch` as a touch-screen
backend does (the touch, then the first finger's pointer events), and
`App::advance_fling` / `App::advance_long_press` stand in for the loop's
frames and the long-press timer; `src/touch_tests.rs` and the recognizer's
own tests drive long presses, sideways strips and pinches through them.
On devices, CI pinches `androiddemo`'s button on the Android emulator with
two fingers: `adb shell input` has only one, so the script makes a virtual
touch screen with the shell's `uinput` command (Android 13 and 14 have it,
11 doesn't), whose events go through the kernel and the system's input
pipeline as a real screen's do; the app logs the pinch's start, scale and
end, and no tap. On iOS, `touchdemo`'s simulator self-test feeds the
backend's touch path two synthetic fingers.

## The pointer leaving the window

When the pointer leaves the window, nothing is hovered any more: the theme's
hover highlight goes, `.hovered(...)` looks and `Cx::interaction` drop their
hover state, and the element with `on_pointer` under it gets its `Leave`.
While a button is down the window keeps the pointer; the hover ends when the
button comes up outside.

| | Leaving | Capture for every button |
|---|---|---|
| X11 | `LeaveNotify` | the server's implicit grab |
| Wayland | `wl_pointer.leave` | the compositor's implicit grab |
| Windows | `TrackMouseEvent` / `WM_MOUSELEAVE` | `SetCapture` on any press, released with the last button |
| macOS | a tracking area's `mouseExited` | AppKit sends the drags (`mouseDragged`, `rightMouseDragged`, `otherMouseDragged`) to the view |
| Web | `pointerleave` | `setPointerCapture` |
| Android | hover exit, and after a finger lifts | the system's |
| iOS | after a finger lifts | the system's |

## Buttons per platform

| | Middle | Back / forward | Others |
|---|---|---|---|
| X11 | button 2 | buttons 8 and 9 | `Other(n)`, the X button number (4–7 are the wheel) |
| Wayland | `BTN_MIDDLE` | `BTN_SIDE`/`BTN_BACK`, `BTN_EXTRA`/`BTN_FORWARD` | `Other(code)`, the evdev code |
| Windows | `WM_MBUTTON*` | `WM_XBUTTON*` | — |
| macOS | `otherMouseDown` button 2 | buttons 3 and 4 | `Other(n)`, the button number |
| Web | `button` 1 | 3 and 4 | `Other(n)` |
| Android | the mouse's tertiary button | its back and forward buttons | — |
| iOS | — (touch: the first finger is the left button) | — | — |

Middle-click paste is up to the app: `on_pointer` sees the middle button, and
the clipboard API reads what to paste.

## Pointer lock

A game steering a camera with the mouse, or a 3D viewer orbiting, wants the
mouse's motion rather than the pointer's position: the pointer would hit
the screen's edge. Locking the pointer hides the cursor and holds it in
place; mouse motion then reaches `on_pointer` as `PointerEventKind::Motion`
events, with how far the mouse moved in `movement` (positive right and
down, roughly logical pixels, unaccelerated where the platform offers it).

```rust
use stipple::core::{PointerEvent, PointerEventKind};
use stipple::prelude::*;

#[derive(Default)]
struct Game {
    yaw: f64,
    pitch: f64,
}

fn view(game: &Game, cx: &mut Cx<Game>) -> Element {
    let handle = cx.handle();
    let paused = !cx.pointer_locked();
    Element::boxed(BoxStyle::default())
        .on_pointer(cx, move |ev: &PointerEvent, g: &mut Game| match ev.kind {
            PointerEventKind::Motion => {
                g.yaw += ev.movement.dx * 0.2;
                g.pitch = (g.pitch - ev.movement.dy * 0.2).clamp(-89.0, 89.0);
            }
            // "Click to resume": lock from the click itself (browsers insist).
            PointerEventKind::Down if paused => handle.set_pointer_lock(true),
            _ => {}
        })
}

App::new(Game::default(), view)
    .on_pointer_lock(|_, _, locked| {
        // The view shows "click to resume" while it's off.
        println!("pointer locked: {locked}");
    })
    .run();
```

- `AppHandle::set_pointer_lock(true)` locks the primary window's pointer,
  `false` releases it (another window: `window_to` with
  `WindowRequest::PointerLock`). A locked pointer shows the cursor again
  where it was held when released.
- `App::on_pointer_lock(|state, window, locked| ...)` hears the lock start
  (`true`) and end (`false`): released by the app, lost (the window lost
  focus, Escape in a browser, the compositor took it back), or refused (the
  platform has no pointer lock, the browser saw no user gesture).
  `Cx::pointer_locked()` tells the view, which is rebuilt when it changes.
- The lock ends when the window loses keyboard focus, and the app hears so;
  it doesn't come back by itself. Escape releases it only on the web (the
  browser's rule); elsewhere an app that wants that handles Escape itself.
- While locked, `Motion` goes where a move would: the element holding the
  capture, else the one under the pointer when it was locked. Moves stop
  (the pointer stays put); buttons and the wheel still come, at the
  position the pointer is held at (the window's center on desktops).
- The platform event is `Event::PointerDelta { dx, dy }`, and a lock the
  system ended or refused is `Event::PointerLockLost`.

| | Holds the pointer | Motion | Notes |
|---|---|---|---|
| macOS | `CGAssociateMouseAndMouseCursorPosition(false)` after `CGWarpMouseCursorPosition` to the view's center, `[NSCursor hide]` | mouse events' `deltaX` / `deltaY` (points; with the system's acceleration) | |
| Windows | `ClipCursor` to one pixel at the client area's center, no cursor on `WM_SETCURSOR` | raw input (`RegisterRawInputDevices`, usage page 1 / usage 2), `WM_INPUT`'s `lLastX` / `lLastY` (device counts, unaccelerated) | absolute devices (tablets, remote desktop) send none |
| X11 | `GrabPointer` (confined to the window, a blank cursor) and `WarpPointer` back to the center after each move | each move's distance from the last, the warp's own move left out (physical pixels over the scale) | fails while another client holds a grab |
| Wayland | `zwp_pointer_constraints_v1.lock_pointer` (one-shot), the cursor hidden through `wl_pointer.set_cursor` | `zwp_relative_pointer_v1.relative_motion`'s unaccelerated `dx` / `dy` | refused without both protocols; the cursor hides only with `wp_cursor_shape_v1` |
| Web | `canvas.requestPointerLock({ unadjustedMovement: true })`, plain where that's unsupported | `pointermove`'s `movementX` / `movementY` (CSS pixels) | only during or shortly after a user gesture: lock from a click or key handler; Escape releases it |
| iOS, Android | — (refused: `on_pointer_lock` hears `false`) | — | |

`cargo run --example pointerdemo`: `L` locks the pointer and the pad prints each
motion and the total; `L` or Escape releases it. CI runs it on X11 under
Xvfb with `--lock-test` (lock on the first click), moves the mouse with
`xdotool mousemove_relative` and checks the motion and that the pointer
stayed at the window's center (non-blocking for now). `App::set_pointer_locked`
locks as a platform would without a window, and `App::pointer_event` with
`Event::PointerDelta` feeds motion, for unit tests.

## Testing

`App::pointer_event` feeds the primary window a platform `Event` (a
`PointerButton`, `PointerMoved`, `PointerLeft` or `Scroll`) as the run loop
would, without a window: unit tests drive a view through it and check the
state. `stipple::platform::set_pointer_modifiers` sets the modifiers the next
events carry.

`cargo run --example pointerdemo` prints every pointer and wheel event its pad gets,
whether it's hovered, and hides the cursor while you type. CI runs it on X11
under Xvfb and checks, with `xdotool`, a middle click, a double click,
Shift+right-click, the horizontal wheel (buttons 6 and 7), the hidden cursor
while typing, and the pointer leaving the window (the `Leave`, and the hover
look ending). The other backends are build-checked only.
