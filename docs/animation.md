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
Most of the time you don't need any of this: style an element for its states
and give it a transition, or an enter, exit or layout animation, below.

## Interaction states

Every element can be styled for what the user is doing to it. Each state has
an override that restyles the element while it holds:

```rust
use stipple::prelude::*;

let card = column(vec![label(&theme, "Open")])
    .padding(Insets::uniform(12.0))
    .fill(theme.palette.surface)
    .radius(6.0)
    .hovered(|e| e.fill(theme.palette.surface.mix(theme.palette.primary, 0.1)))
    .pressed(|e| e.fill(theme.palette.surface.mix(theme.palette.primary, 0.2)))
    .focus_visible(|e| e.border(theme.palette.focus_ring, 2.0))
    .on_tap(cx, |s: &mut State| s.open());
```

| Override | While |
|---|---|
| `.hovered(f)` | the pointer is over the element (or something inside it) and nothing drawn above covers it |
| `.pressed(f)` | the primary button (or a finger) went down on it and is still down, over it |
| `.focused(f)` | it, or an element inside it, has keyboard focus |
| `.focus_visible(f)` | focused, and focus got there from the keyboard (Tab), not a click |
| `.dragged(f)` | it is being dragged: an in-app drag from its `drag_source`, or an `on_drag` slider-style drag |
| `.drag_over(f)` | a drag its drop target accepts is over it |
| `.drag_refused(f)` | a drag its drop target refuses is over it |
| `.when(Interaction::DISABLED, f)` | the view marked it `.disabled(true)` |
| `.when(Interaction::SELECTED, f)` | the view marked it `.selected(true)` |

`.when(states, f)` takes any set (`Interaction::HOVERED | Interaction::PRESSED`
waits for both). The closure gets the element without its children and returns
it restyled; what it changes among the fill, radius, border, padding, fixed
width and height, text color, cursor, opacity and transform is applied while
the state holds,
on top of the element's own style, in the order the overrides were given.
Overrides are resolved when the built tree is lowered for layout, so the
layout, the damage tracking and transitions see ordinary styles.

The app rebuilds after an event only when it changes a state some element has
an override for: moving the pointer over an element without a `.hovered` costs
nothing. A tappable element with its own `.hovered` or `.pressed` doesn't get
the theme's hover highlight, and a drop target with its own `.drag_over` or
`.drag_refused` doesn't get the app's drag-over highlight.

### Reading states in the view

To build from a state rather than restyle (different text, a different child),
give the element a key and ask the `Cx`:

```rust
let states = cx.interaction("save");
let text = if states.pressed() { "Saving…" } else { "Save" };
button_labeled(&theme, cx, text, |s: &mut State| s.save()).key("save")
```

`Cx::interaction(key)` returns an `Interaction` (`hovered()`, `pressed()`,
`focused()`, `focus_visible()`, `dragged()`, `drag_hover()` and
`drag_accepted()`). The view is rebuilt whenever one of them changes for a key
it read.

### Element identity

States and transitions belong to an element across builds, though the view
builds a new tree each time. An element keyed with `.key(k)` (any `Hash`
value: a string, an item id, a tuple) is that element wherever it is; keys are
window-wide, so give each its own. An element without a key is identified by
its position: the path of child indices from the root, or from its nearest
keyed ancestor. That suits elements that stay put; key the items of a list that
reorders, so their state follows them. `Cx::interaction` needs a key.

States come from where elements were in the frame on screen, worked out again
on each pointer, button, focus and drag event. On platforms that report the
pointer leaving the window (the web, iOS, Android), nothing is hovered after
it leaves; the desktop backends don't report that yet, so the last element
stays hovered.

## Transitions

Give an element a `Transition` and changes to its animatable properties
animate instead of jumping, whatever changed them: a state override, the app's
state, or both.

```rust
label(&theme, "Drop files here")
    .padding(Insets::uniform(16.0))
    .fill(theme.palette.surface)
    .drag_over(|e| e.padding(Insets::uniform(24.0)).fill(theme.palette.primary))
    // Grows with a little overshoot.
    .transition(Transition::spring(SpringParams::new(180.0, 0.45)))
```

- A `Transition` wraps an `anim::Motion`: `Transition::ease(duration)`
  (ease-out), `Transition::tween(duration, easing)` or
  `Transition::spring(SpringParams::BOUNCY)` (any `SpringParams`);
  `Transition::default()` is a 150 ms ease-out and `Transition::instant()`
  switches. `From<Motion>` and `From<SpringParams>` convert.
- The animatable properties are `AnimProp::Fill`, `Border` (color and width),
  `Radius`, `Padding`, `Width` and `Height` (between two fixed sizes; to or from
  "sized by content" they switch), `TextColor`, `Opacity` and `Transform`
  (part by part, as `Transform::lerp`; no transform is the identity). A fill
  or border that appears or goes away fades in from, or out to, transparent
  (colors blend in premultiplied sRGB, so the hue holds).
- `.transition(t)` animates them all; `.transition_for(AnimProp::Fill, t)`
  gives one property its own (later calls win).
- Each property is an `Animated` value, so a change mid-flight heads for the
  new value from wherever it is now, keeping its velocity: reversing a hover
  is smooth. Reduced motion applies as it does to every `Animated`.
- The first build an element appears in shows it as it is; only later changes
  animate.

The app keeps the in-flight values by element identity (above), asks for a
frame while any of them moves and stops when all are at rest, so a UI full of
transitions uses no CPU when nothing changes. It counts each change from one
frame before the build that saw it, so the change already shows on that frame.

## Enter, exit and layout animations

Elements can also animate as they come and go, and as the layout moves them:

```rust
use stipple::prelude::*;

let rows = state.items.iter().map(|item| {
    label(&theme, &item.name)
        .key(item.id)
        // Fades in, sliding from the left, when added.
        .enter(Effect::fade().and(Effect::slide(-40.0, 0.0)))
        // Fades out, shrinking a little, when removed.
        .exit(Effect::fade().and(Effect::scale(0.8)))
        // Glides to its new place when the list reorders.
        .animate_position(Transition::spring(SpringParams::SMOOTH))
});
column(rows.collect())
```

- An `Effect` is how the element looks while it isn't there: an opacity and a
  transform, and the `Transition` to move between that and its own look.
  `Effect::fade()`, `Effect::slide(dx, dy)` and `Effect::scale(s)` are the
  presets (200 ms, easing out), `.and(other)` combines two (opacities
  multiply, offsets add, scales multiply), `Effect::custom(opacity,
  transform)` makes your own (a rotation, an origin of your choosing) and
  `.transition(t)` sets the motion, a tween or a spring.
- `.enter(effect)` plays when the element appears: it wasn't in the last
  frame. The window's first frame shows everything as it is.
- `.exit(effect)` plays when it goes away: the view stops building it. The app
  keeps drawing it, as it was last drawn, under its old parent at its old
  place (a *ghost*), until the effect ends. A ghost has no handlers, takes no
  hover, clicks or focus, isn't in the accessibility tree, and takes no room
  in the layout: its siblings close up at once, or glide up if they animate
  their layout. If the view builds the element again before it's gone, it
  turns round and comes back from where it is.
- The content of an overlay (`cx.overlay`) can have an exit too: when the
  view stops declaring the overlay, the app keeps drawing it where it was,
  over the window, while it leaves. A modal overlay's scrim fades in and out
  with it, on its content's enter and exit motion. Like any ghost, a leaving overlay takes no input: a
  click on it reaches what's beneath (the scrim stops blocking at once), the
  keyboard can't focus into it, and screen readers no longer see it. An
  overlay is the same one across frames by its content's key, or else by its
  place among the overlays of its kind of `Anchor` (the first `Below`, the
  second `At`, ...), so a dropdown closing doesn't make a toast or a tooltip
  look like it left and came back. This is how the built-in popups, menus,
  tooltips and dialogs close (see [widgets.md](widgets.md#motion)).
- `.appear(effect)` is both.
- `.animate_layout(t)` animates the element from where it was drawn to where
  the layout puts it now, whenever that changes: a list reorders, a row is
  inserted above, a panel opens. The element stays laid out at its new place
  and is drawn moved back (and, for a size change, scaled), and that moves to
  nothing (FLIP). `.animate_position(t)` moves it without scaling: a new size
  applies at once, which suits rows with text.

Key the elements that use them (`.key(id)`): an element is "the same one"
across frames by its key, so a keyed row that moves in the list is recognized
as moved, not as one removed and another added. (Unkeyed elements are
identified by their position in the tree, which a reorder changes.)

Positions are measured from the nearest ancestor that animates its own layout
(whose motion then carries its children: they don't move twice), else from the
content of the nearest scroll container, else from the window. So scrolling
doesn't animate anything, a parent and its child each animate only their own
change, and resizing the window snaps instead of animating.

Everything runs on the laid-out tree, after layout and scrolling, as a
transform and a group opacity on top of the element's own ones (its
`.transform` and `.opacity`, and their transitions): hit-testing follows where
the element is drawn, as for any transform. Each animation is an `Animated`
value, so a change mid-flight (a second reorder, a removal undone) carries on
from the current place and speed; under reduced motion they all snap, and a
removed element disappears at once. The app asks for frames only while
something enters, leaves or moves.

A few things to know:

- A ghost is drawn at its old place in the tree's order (over the siblings
  before it, under those after it), and an element sliding past others is
  drawn at its new place in that order, which may be beneath them.
- A size change scales the element and its content (text included) from the
  old size; when that looks wrong, use `animate_position`.
- When an element with its own rotation is resized by its layout animation
  unevenly, the scale is applied as if before the rotation.

[`examples/listmotion`](../examples/listmotion) shows a list whose rows enter,
leave and glide when it's shuffled, sorted or pushed down by a panel;
`listmotion --selftest` drives it off-screen, and CI screenshots
`listmotion --demo` on X11 mid-flight.

## What animates already

- `spinner(cx, &theme)` turns once a second for as long as it's shown. It asks
  for frames itself, so a view that shows it while loading and drops it when
  done goes back to idle on its own.
- Hover and press highlights fade in and out (120 ms, 60 ms in for a press)
  rather than switching. A pressed element shows the hover highlight twice over.
- A drop target's `DropState::level` eases in over 180 ms when a drag enters
  it and out when it leaves or drops, for a view to animate its drop zone
  with by hand; `.drag_over` with a transition usually does it for you. A
  target that styles itself neither way gets the app's "drop here"
  highlight, which fades in and out with that level. See
  [drag-and-drop.md](drag-and-drop.md).
- The built-in widgets: buttons' hover and press looks (and a slight press
  scale), switches' thumbs and tracks, check marks and radio dots, tab
  fills and the tab bar's sliding indicator, select lists and drawn menus
  opening, toasts coming and going, the drawn message box opening and tree
  disclosure triangles turning. They're built from the pieces on this page
  (state overrides with transitions, `Animated` values, transforms and
  opacity), with durations and curves from the theme's motion tokens,
  below.
- A theme change cross-fades the window from the old frame to the new one over
  200 ms (`stipple::core::THEME_FADE`): the OS switching to dark mode under
  `Theme::native()`, or the app switching themes through `App::theme_with`.
- A touch fling glides to a stop after the finger lifts.

## Motion tokens

A theme's `motion` field (`MotionTokens`) says how its widgets move: each
token is an `anim::Motion` (a tween with its easing, or a spring), plus the
scales and offsets entrances start from:

| Token | Used for |
|---|---|
| `state` | color changes: hover and press looks, a switch's track, a checkbox's box, segmented tabs |
| `press`, `press_scale` | a pressed button shrinking (`press_scale` 1: it doesn't) |
| `toggle` | a switch's thumb, a check mark, a radio dot |
| `indicator` | the tab bar's accent strip and the segmented tabs' fill sliding to the selected tab |
| `popup`, `popup_scale`, `popup_offset` | select lists, drawn menus, `open_menu` and the tab bar's list opening, tooltips showing |
| `popup_exit` | those closing: fading out as they shrink back toward `popup_scale` and `popup_offset` |
| `dialog`, `dialog_scale` | the drawn message box and `open_dialog` opening |
| `dialog_exit` | those closing, their scrim fading with them |
| `toast_enter`, `toast_exit`, `toast_offset`, `toast_scale` | toasts coming and going |
| `disclosure` | tree disclosure triangles turning |

`MotionTokens::standard()` is Stipple's own (the light and dark themes),
`material()` Material 3's (the Material themes, and Android), and
`Theme::native()` picks the platform's: `fluent()` on Windows, `macos()`,
`ios()`, and `adwaita()` on Linux. [widgets.md](widgets.md#motion) lists
what each widget does and where each preset's numbers come from;
`Theme::with_motion` sets your own, and `MotionTokens::instant()` turns the
widgets' motion off.

Your own elements can follow the theme too:

```rust
let m = theme.motion;
card.hovered(|e| e.fill(highlight))
    .transition(Transition(m.state))
```

## Theme changes

`App::theme_with` derives the theme from the state, for an app with its own
dark-mode switch; it gets the state and the theme set with `App::theme` (which
follows the system under `Theme::native()`):

```rust
App::new(State::default(), view)
    .theme(Theme::native())
    .theme_with(|state: &State, system: &Theme| match state.dark {
        Some(true) => Theme::dark(),
        Some(false) => Theme::light(),
        None => *system,
    })
```

Whenever the theme views see changes, the App keeps the frame that was on
screen and, for the next 200 ms, presents the new frame blended over it, asking
for frames only while the fade runs. It blends finished pixels, so it works the
same with the software renderer and a GPU one set with `App::render_with`. A
frame whose view didn't change underneath is only re-blended, not rasterized
again; resizing the window mid-fade ends it.

## Reduced motion

Every platform has a setting for less motion, and Stipple reads it when the app
starts and again when the OS reports a change:

| Platform | Setting | Change heard from |
|---|---|---|
| Windows | *Animation effects* off (`SPI_GETCLIENTAREAANIMATION`) | `WM_SETTINGCHANGE` with `SPI_SETCLIENTAREAANIMATION` |
| macOS | *Reduce motion* (`NSWorkspace.accessibilityDisplayShouldReduceMotion`) | `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification` |
| iOS | *Reduce Motion* (`UIAccessibilityIsReduceMotionEnabled`) | `UIAccessibilityReduceMotionStatusDidChangeNotification` |
| Android | *Remove animations* (`Settings.Global` `animator_duration_scale` at 0, read over JNI) | read again whenever the activity resumes |
| Linux | GNOME's `enable-animations` or KDE's `AnimationDurationFactor`, through the desktop portal; without a portal, `gtk-enable-animations` in GTK's `settings.ini` | the portal's `SettingChanged` signal |
| Web | `prefers-reduced-motion: reduce` | the media query's `change` event |

`stipple::anim::motion_scale()` turns it into a factor for durations: 0 under
reduced motion, otherwise 1 (on Android, the animator duration scale itself,
so 0.5× or 2× in the developer options apply too). `anim::motion_duration(d)`
applies it, and `anim::reduced_motion()` (or `Cx::reduced_motion()`) asks the
question directly. Anything that starts a transition should scale its length
with `motion_duration`, so it snaps when motion is reduced, and swap movement
(a slide, a zoom) for a cross-fade or a jump:

```rust
use std::time::Duration;
use stipple::anim::{motion_duration, Easing, Tween};

let length = motion_duration(Duration::from_millis(250)).as_secs_f64();
let fade = Tween::new(0.0, 1.0, length, Easing::EaseOut); // ends at once when 0
```

What the toolkit does under reduced motion:

- `Animated<T>` (and so `Cx::animate`) applies the scale itself: tweens
  shorten (to nothing), springs jump to their target, `fling` stops where the
  value is, and `play` (a shake, a pulse, keyframes) jumps to the end. A plain
  `Tween` or `Spring` you sample yourself doesn't know about it; use
  `motion_duration` as above.
- Hover and press highlights and drop targets' `DropState::level` switch at once.
- Element transitions (`Element::transition`) run on `Animated`, so they
  shorten or jump the same way.
- The built-in widgets' motion (it runs on `Animated` too) snaps: switches
  flip, lists and toasts appear and go at once, under every theme's tokens.
- Theme changes switch without a cross-fade.
- A touch scroll stops where the finger lifts, without a fling.
- The spinner keeps turning, since it's what shows that something is happening,
  but at half the speed (as Bootstrap's spinners do).

`anim::set_reduced_motion(Some(true))` (or `Some(false)`) overrides the system
for the calling thread, which is the UI thread: an app's own "reduce motion"
option, or a test that needs one or the other. `None` follows the system again.

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

- There's no smooth (animated) scrolling to snap; wheel scrolling jumps.
- With several windows, frames re-render the window the wake arrived for, so
  animations in other windows advance on their next event.
