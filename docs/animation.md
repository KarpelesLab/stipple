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

Or let `Cx::play(&anim, started)` do the sampling and the asking: it samples
any `Animation` (below) at the frame time minus `started`, and asks for the
next frame until it finishes. In
an event handler, which has no `Cx`, `stipple::anim::now()` reads the same
clock, so a handler can record *when* something happened and the view plays
from there.

## Animations are functions of time

Everything in `stipple::anim` implements the `Animation` trait: `sample(t)`
gives the value `t` seconds in, `velocity(t)` its rate of change,
`duration()` how long it runs (`None` for forever) and `is_finished(t)`
whether it has come to rest. Sampling has no side effects, so the same
animation can be checked at fixed times in a unit test and gives exactly what
the app will show at those frame times.

| Building block | What it does |
|---|---|
| `Tween<T>` | `from` to `to` over a duration, along an `Easing` |
| `Spring<T>` | a damped spring from a position and velocity to a target, in closed form (`SpringParams` presets: `GENTLE`, `SMOOTH`, `SNAPPY`, `BOUNCY`, `STIFF`, or `with_response(seconds, damping)`) |
| `Decay<T>` | a fling slowing by friction; `target()` is where it stops |
| `Keyframes<T>` | a start value and segments, each with its own duration and easing (`.to`, `.hold`, or CSS-style `Keyframes::percent`) |
| `Still<T>` | a value at rest |

`Easing` has the four classic curves, CSS `cubic-bezier()` curves
(`Easing::cubic_bezier(x1, y1, x2, y2)`) and named ones: `EASE`,
`CSS_EASE_IN` / `_OUT` / `_IN_OUT`, Material's `STANDARD`,
`EMPHASIZED_DECELERATE` and `EMPHASIZED_ACCELERATE`, and the overshooting
`BACK_OUT`. `Easing::slope(t)` is the curve's derivative, which is how a tween
knows its velocity.

They combine (`AnimationExt`):

- `.delay(seconds)` holds the start value, then plays.
- `.repeat(count)` (fractions allowed), `.repeat_forever()`, and
  `.alternate()` on either to play every other run backward.
- `.then(next)` builds a `Sequence`; a tuple `(a, b)` or `(a, b, c)` plays
  animations in parallel and produces a tuple of values, finishing when all
  have.
- `stagger(i, step)` is item `i`'s delay in a staggered group, and
  `stagger_within(i, count, step, max)` caps the whole group's spread.

Ready-made effects: `shake(amplitude, duration)` (an offset that swings and
dies away, for invalid input), `pulse(low, high, period)` (forever, eased at
both ends, for a drop target or an attention dot) and `entrance(i, step,
duration)` (a staggered 0-to-1 progress for list items).

```rust
use stipple::anim::{self, Animation};

// In the handler: remember when the input was refused.
s.invalid_at = Some(anim::now());

// In the view: shake the field from then on, until it settles.
let dx = match s.invalid_at {
    Some(at) => cx.play(&anim::shake(10.0, 0.4), at),
    None => 0.0,
};
```

## What can be animated

`Animatable` values: `f64`, `Color`, `Point`, `Vec2`, `Size`, `Rect`,
`Insets`, `[f64; N]`, and pairs and triples of them (to move several
properties as one). Each maps into a `Vector` space (`f64`, `[f64; N]` or
tuples) where blending, velocities and springs are computed; implement
`to_vector` / `from_vector` to animate your own type.

Colors animate in **premultiplied, gamma-encoded sRGB**: `[r·a, g·a, b·a, a]`.
Premultiplying keeps the hue when fading to or from transparent (red fades
out as red, not through grey); staying gamma-encoded matches `Color::mix`, the
hover highlights and CSS transitions for opaque colors. Overshoot (a bouncy
spring) is clamped per channel, and sizes stop at zero.

## Interruptible motion: `Animated<T>`

`Animated<T>` is a value that moves toward targets and can be interrupted at
any moment without a jump or a jolt: whatever replaces the running motion
starts from the current value **and velocity**.

- `animate_to(target, motion, now)` (or `spring_to`, `tween_to`) heads for a
  target with a `Motion` (`Instant`, `Tween { duration, easing }`, or
  `Spring(params)`). It does nothing if that's already the target, so a view
  may call it every build. A spring takes the current velocity as is; a tween
  interrupting a moving value adds a correction term that fades out over the
  tween, so it starts at the current velocity and ends exactly like the plain
  curve (from rest it *is* the plain curve). `retarget` restarts even toward
  the same target.
- `play(anim, now)` runs any animation (keyframes, a shake) in its place.
- `drag_to(value, now)` lets a gesture set the value directly; it tracks the
  gesture's velocity (smoothed over ~30 ms, and zero if the finger rested for
  100 ms). `release(target, motion, now)` hands off to a spring or tween with
  that velocity, `release_with_velocity` takes one the platform measured, and
  `fling(rate, now)` glides to a stop by friction (`Decay::NORMAL`,
  `Decay::FAST`) and returns where.
- `cx.animate(&animated)` reads it at the frame time and asks for a frame while
  it moves. A dragged value doesn't need frames: the gesture drives it.

```rust
use stipple::anim::{self, SpringParams};

// Handler: toggle; a second click mid-flight turns it round smoothly.
s.on = !s.on;
s.knob.spring_to(if s.on { 1.0 } else { 0.0 }, SpringParams::BOUNCY, anim::now());

// View:
let x = cx.animate(&s.knob);
```

`Spring::step(dt)` still advances a spring statefully, now with the exact
solution rather than an Euler step, so any frame interval is stable.

[`examples/motiondemo`](../examples/motiondemo) shows a field that shakes on
an invalid code, a pulsing drop target, a staggered list entrance and an
interruptible spring; `motiondemo --selftest` checks them at fixed times.

## What animates already

- `spinner(cx, &theme)` turns once a second for as long as it's shown. It asks
  for frames itself, so a view that shows it while loading and drops it when
  done goes back to idle on its own.
- Hover and press highlights fade in and out (120 ms, 60 ms in for a press)
  rather than switching. A pressed element shows the hover highlight twice over.
- A drop target's `DropState::level` eases in over 180 ms when a drag enters
  it and out when it leaves or drops, for a view to animate its drop zone
  with; see [drag-and-drop.md](drag-and-drop.md).

## Moving, scaling, turning and fading

`Element::transform` draws an element and its subtree translated, scaled and
rotated about an origin, and `Element::opacity` fades it as a group, without
changing layout; pointer input follows. Sample a `Tween` or `Spring` into them
each frame to animate. See [transforms.md](transforms.md).

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
