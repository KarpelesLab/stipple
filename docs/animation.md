# Animation

Stipple redraws when something changes. To animate, a view asks for another
frame each time it builds; the loop then wakes at the display's next refresh,
the view builds again with a later time, and so on. When builds stop asking,
frames stop, and an idle app uses no CPU.

## Asking for frames

`Cx::frame_time()` is the time of the frame being built, on a timeline that
starts with the app's first frame. `Cx::request_frame()` asks for the next one.
Sample your animation at the frame time, and keep asking while it moves:

```rust
use stipple::anim::{Easing, Tween};

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    // `opened_at` is a `Duration` from `cx.frame_time()` when the panel opened.
    let t = cx.frame_time().saturating_sub(state.opened_at).as_secs_f64();
    let fade = Tween::new(0.0, 1.0, 0.25, Easing::EaseOut);
    if !fade.is_finished(t) {
        cx.request_frame(); // still moving: build again next refresh
    }
    progress_bar(&theme, fade.sample(t))
}
```

Take start times from `cx.frame_time()` too, so they're on the same timeline. A
`Spring` works the same
way: step it by the time since the last frame and keep asking until
`is_at_rest()`.

`App::animating()` tells whether the last frame asked for another, which is handy
in tests.

## What animates already

- `spinner(cx, &theme)` turns once a second for as long as it's shown. It asks
  for frames itself, so a view that shows it while loading and drops it when
  done goes back to idle on its own.
- Hover and press highlights fade in and out (120 ms, 60 ms in for a press)
  rather than switching. A pressed element shows the hover highlight twice over.

## Where frames come from

| Platform | Frames paced by |
|---|---|
| Wayland | `wl_surface.frame` callbacks, requested with each commit |
| X11 | a timer at the monitor's refresh rate, read from RandR (60 Hz if unknown) |
| Windows | a thread waiting on `DwmFlush`, DWM's composition pass |
| macOS | a `CVDisplayLink`, stopped while nothing animates |
| iOS | a `CADisplayLink`, paused while nothing animates |
| Web | `requestAnimationFrame`, requested only while something animates |
| Android | not yet: nothing animates there |

A frame that's asked for always arrives: if the display source doesn't deliver
within two refresh intervals (a hidden window, say), a timer does.
`stipple::platform::frame` holds the clock.

## Not yet

- Theme changes (light to dark) switch at once rather than cross-fading.
- With several windows, frames re-render the window the wake arrived for, so
  animations in other windows advance on their next event.
