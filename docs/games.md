# Games

A 2D game on Stipple draws its world into a [canvas](canvas.md) once per
display frame, reads the keyboard, mouse and gamepads, and builds its menus,
HUD and settings from Stipple's widgets. `stipple::game` holds the pieces
that are a game's own:

- **The loop** ([below](#the-loop)): an update per display frame that
  repaints the canvas without rebuilding the view, pausing on its own when
  the game can't be seen.
- **A fixed time step** ([below](#a-fixed-time-step)), so the simulation
  doesn't depend on the refresh rate.
- **Input** ([below](#input)): what is held, and what was pressed and
  released since the last update.
- **Actions** ([below](#actions)): "jump", "move left", bound to inputs the
  player can change and save.
- **Drawing** ([below](#drawing)): sprite sheets and animations, tilemaps, a
  camera, and pixel art scaled by whole numbers, on fast blit paths.

Pointer lock is in [pointer-events.md](pointer-events.md#pointer-lock) and
gamepads in [gamepads.md](gamepads.md). The rest of Phase 9 (see the
[roadmap](../ROADMAP.md#phase-9--2d-games)), touch controls, audio and game
UI, lands alongside.

The `gameloop` example puts the pieces together in about a hundred lines: a
square moved with the arrows or WASD, stepped at 120 Hz, with an fps label.

## The loop

`App::on_frame(update)` runs `update(&mut state, &mut Frame)` once per
display frame, on the UI thread, while the game runs. `App::game_loop` takes
a `GameLoop` for more control:

```rust
use stipple::prelude::*;
use stipple::game::{Frame, GameLoop};
use std::cell::RefCell;
use std::rc::Rc;

const WORLD: CanvasId = CanvasId(1);

#[derive(Default)]
struct World { x: f64 }

struct Game { world: Rc<RefCell<World>>, score: u32, paused: bool }

fn update(g: &mut Game, frame: &mut Frame) {
    g.world.borrow_mut().x += 60.0 * frame.dt_secs();
    frame.repaint(WORLD); // the canvas changed, the view didn't
    if frame.input().key_pressed(PhysicalKey::SPACE) {
        g.score += 1;
        frame.rebuild(); // the HUD shows the score
    }
}

fn view(g: &Game, cx: &mut Cx<Game>) -> Element {
    let theme = *cx.theme();
    let world = g.world.clone();
    column(vec![
        label(&theme, format!("Score: {}", g.score)),
        Element::canvas(WORLD, move |c: &mut Canvas| {
            let x = world.borrow().x % c.size().width;
            c.fill_rect(Rect::from_xywh(x, 10.0, 20.0, 20.0), Color::rgb(200, 40, 40));
        })
        .grow(1.0),
    ])
}

let game = Game { world: Default::default(), score: 0, paused: false };
let app = App::new(game, view)
    .game_loop(GameLoop::new(update).paused_when(|g: &Game| g.paused));
```

**Not rebuilding the view.** An update doesn't rebuild the view on its own:
it says what changed. `frame.repaint(id)` (or `repaint_rect(id, rect)`)
paints a canvas again, and only that area is rasterized and presented, as
with [`invalidate_canvas`](canvas.md#repainting); `frame.rebuild()` rebuilds
the view after the update, for when something the view shows changed (a
score, a pause menu). A frame that only repaints the canvas doesn't build or
lay out anything; the game's tests check it, and `STIPPLE_APP_DEBUG=1
gameloop --selftest` shows it (`present: repainted` each frame, `present:
built` only when the fps label changes). Since the canvas's paint callback
doesn't get the state, keep what it draws in an `Rc<RefCell<…>>` (or
`Rc<Cell<…>>`) shared by the state and the callback.

**The `Frame`.**

- `time()`: the frame time, on the same timeline as `Cx::frame_time` and
  animations.
- `dt()` / `dt_secs()`: the time since the last frame, at most
  `GameLoop::max_dt` (100 ms unless set), so after a hitch the game slows
  down instead of jumping. `raw_dt()` is the time before that cap. The first
  frame after starting or resuming gets one refresh interval.
- `index()`: how many frames ran before.
- `input()`: the [input state](#input).
- `stats()`: a `LoopStats` with `fps`, the mean `frame_time`, `update_time`
  (the update itself) and `render_time` (presenting a frame: building the
  view when it is rebuilt, painting, rasterizing, handing it over), averaged
  over half a second, and `frames` so far.

**Pausing.** The loop runs while the game can be seen and isn't paused. It
stops, and asks for no frames at all (no CPU), while:

- `paused_when(|s| …)` returns `true`: a pause flag in the state. It is
  asked after every event and update, so a key handler, a button in a pause
  menu or the update itself pauses and resumes the game. The update doesn't
  run while paused, so resume from the view (a button, an `on_raw_key`
  handler) or an `AppHandle::update`.
- every window of the app is minimized, or the app is in the background
  (mobile, see [window-lifecycle.md](window-lifecycle.md#going-to-the-background-mobile)).
- no window has keyboard focus, with `pause_when_unfocused(true)`.

On resuming, the first frame's `dt` is one refresh interval, not the time
spent paused, and keys and buttons pressed while paused (the one that
resumed) aren't reported as presses.

**How it runs.** The loop asks the [frame clock](animation.md) for a frame
after each update, the way an animating view does, and runs the update when
the frame arrives, before the window presents. Wakes that aren't frames
(an `AppHandle::update`, a timer) don't run it. With several windows it runs
once per frame, and each window repaints the canvases asked for.

To run an update in a test, without an app, make a `Frame` yourself:
`Frame::new(time, dt, &input)`, then look at `frame.repaints()` and
`frame.rebuilds()`.

## A fixed time step

Physics stepped by the frame's `dt` changes with the refresh rate. A
`FixedStep` spends each frame's `dt` in whole steps of a fixed size and
keeps the rest for the next frame:

```rust
use stipple::game::FixedStep;
# use std::time::Duration;
# struct Body { prev: f64, pos: f64, vel: f64 }
# let mut body = Body { prev: 0.0, pos: 0.0, vel: 100.0 };
# let dt = Duration::from_millis(16);
let mut step = FixedStep::hz(120); // or FixedStep::new(Duration)
for _ in 0..step.advance(dt) {
    body.prev = body.pos;
    body.pos += body.vel * step.dt_secs();
}
// Draw between the last two states.
let drawn = body.prev + (body.pos - body.prev) * step.alpha();
```

`alpha()` is how far the leftover time is into the next step (0 to 1).
`advance` runs at most `max_steps` steps per frame (8 unless set) and drops
the time beyond, rather than falling further behind each frame;
`dropped()` says how much. `reset()` forgets the leftover time (a new
level).

## Input

`frame.input()` is an `InputState`, kept up to date from the window's events
whatever has focus:

| | |
|---|---|
| `key_held(k)`, `key_pressed(k)`, `key_released(k)` | Keys by position (`PhysicalKey`: `KEY_W`, `ARROW_LEFT`, `SPACE`, … or `PhysicalKey::from_name("KeyW")`), the same on every layout. |
| `logical_held(c)`, `logical_pressed(c)`, `logical_released(c)` | Keys by what they gave under the layout when pressed (`KeyCode::Char('w')`, letters compared without case), for "press Q to quit" where the label matters more than the position. |
| `keys_held()`, `keys_pressed()`, `modifiers()` | All of them. |
| `button_held(b)`, `button_pressed(b)`, `button_released(b)` | Mouse buttons. |
| `pointer()` | Where the pointer is in the window, `None` outside it. |
| `motion()`, `wheel()` | How far it moved (or, locked, how far the mouse moved), and the wheel turned (in pixels; lines count 40 px), since the last update. |
| `gamepad_held(pad, b)`, `gamepad_pressed(pad, b)`, `gamepad_released(pad, b)` | Gamepad buttons, on `Some(id)` or on any pad with `None` (the touch controls included). |
| `gamepad_axis(pad, axis)` | A stick (−1…1, +Y down) or trigger (0…1), no dead zone; with `None`, the pad pushing it furthest. |
| `gamepads()`, `gamepad_buttons_pressed()` | The connected pads, and the buttons pressed on them. |
| `touches()`, `touches_started()`, `touches_ended()` | The fingers down (`id`, `start`, `position`), and those that landed and lifted since the last update. |
| `last_device()` | `Keyboard`, `Mouse`, `Gamepad` or `Touch`: what to show prompts for. |

*Pressed* and *released* are edges since the last update, and they are
latched: a key (or pad button) pressed and released between two frames
reads as both on the next frame, so a quick tap is never lost. A key repeat
is not a press. When the window loses focus, every held key, button and
finger is let go and reported released, and the pads' buttons and axes
too, so nothing released in another window stays held. A pad that
disconnects reports its held buttons released. The game loop starts gamepad
support, so pads that are already connected show up on their own.

The pointer events a touch screen's first finger also produces aren't taken
as the mouse: a tap is in `touches_started()`, not `button_pressed(Left)`.

The game sees keys whether or not a widget uses them too; while a text
field in a menu has focus, pause the game (or ignore its input).

**Mouse look.** Lock the pointer (`AppHandle::set_pointer_lock(true)`, from
a click; see [pointer lock](pointer-events.md#pointer-lock)) and read
`motion()`: while locked it adds up the mouse's relative motion
(`Event::PointerDelta`) since the last update, and the pointer's moves (it is
held in place) don't count; unlocked it is how far the pointer moved. Either
way it's in logical pixels, so one sensitivity works for both:

```rust
# let input = stipple::game::InputState::new();
# let (mut yaw, mut pitch) = (0.0f64, 0.0f64);
if input.pointer_locked() {
    let m = input.motion(); // frame.input()
    yaw += m.dx * 0.15;
    pitch = (pitch - m.dy * 0.15).clamp(-89.0, 89.0);
}
```

An `InputState` can also be fed by hand (`key`, `button`, `pointer_moved`,
`add_motion`, `wheel_by`, `gamepad`, `touch`, `event`, then `end_update`
once per update), for a test or for input the game routes itself.

**Touch controls.** For phones, `TouchControls` puts a virtual stick and
buttons over the game, and they act as one more pad: a `TouchStick` moves a
pad stick, a `TouchButton` holds a pad button, so the gamepad bindings below
play them too.

```rust
use stipple::core::CanvasId;
use stipple::game::{Corner, GameLoop, ShowTouch, TouchButton, TouchControls, TouchOverlay, TouchStick};
use stipple::gamepad::GamepadButton;
use stipple::prelude::*;

let controls = TouchControls::new()
    .with_stick(TouchStick::left()) // the left pad stick, floating over the left half
    .with_button(TouchButton::new(GamepadButton::South, "A")) // bottom right
    .with_button(TouchButton::new(GamepadButton::East, "B").at(Corner::BottomRight, Vec2::new(160.0, 60.0)))
    .show(ShowTouch::Auto);
// The loop: GameLoop::new(update).touch_controls(controls)
// The state keeps a TouchOverlay::new(CanvasId(2)).font(Font::system_default());
// the update calls overlay.sync(frame) (it repaints when they change), and
// the view puts overlay.element(&theme) last in an Element::layers covering
// the window.
```

- A finger belongs to the control it lands on until it lifts: a button (or
  just outside it), else a free stick whose area it's in, so the left thumb
  steers while the right one holds a button. A floating stick
  (`TouchStick::left()`'s default) centers where the finger lands anywhere
  on its half of the window; `floating(false)` needs the finger on it.
  Other fingers stay the game's (`touches()`).
- The stick reads its finger's distance from the center over `radius`,
  clamped to 1, with a radial `dead_zone`; a trigger button also pulls its
  axis all the way.
- Placement: each control's center is `inset` in from a `Corner` of the
  window; `radius` sizes it.
- `ShowTouch::Auto` (the default) shows them while touch is the last device
  used: they hide on a key, a click or a pad and come back on the next touch
  (before any input, they show on iOS and Android). `Always`, or `Never`
  (off: they take no touches).
- They're drawn in the theme's colors: the stick's ring in `surface` and
  `border`, the knob and the buttons in `primary` (`primary_active` while
  held), the labels in `on_primary` (with the overlay's font).
- `input.touch_controls()` gives their state (`visible()`, `stick_value(i)`,
  `button_held(i)`) to a game that draws them itself (`paint(canvas, theme,
  font)`).

## Actions

`Actions<A>` binds the game's actions (an `enum` of yours) to inputs, and
answers per update:

```rust
use stipple::game::{Actions, Binding};
use stipple::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Act { Left, Right, Up, Down, Jump, Steer }

let mut actions = Actions::new()
    .with(Act::Left, Binding::Key(PhysicalKey::ARROW_LEFT))
    .with(Act::Left, Binding::Key(PhysicalKey::KEY_A))
    .with(Act::Right, Binding::Key(PhysicalKey::ARROW_RIGHT))
    .with(Act::Jump, Binding::Key(PhysicalKey::SPACE))
    .with(Act::Jump, Binding::MouseButton(PointerButton::Left))
    .with(Act::Steer, Binding::KeyAxis { neg: PhysicalKey::KEY_Q, pos: PhysicalKey::KEY_E });

// In the update:
# let input = stipple::game::InputState::new();
actions.update(&input); // frame.input()
if actions.pressed(Act::Jump) { /* … */ }
let steer = actions.value(Act::Steer); // -1 to 1
let (dx, dy) = actions.axis2(Act::Left, Act::Right, Act::Up, Act::Down);
```

- `held(a)` while any of its bindings is held; `pressed(a)` on the update it
  starts, `released(a)` on the one it stops (both on one update for a tap).
- `value(a)` adds up its bindings' values (1 for a held key or button,
  -1/0/1 for a `KeyAxis`, analog for a gamepad axis), clamped to -1…1.
  `axis2` makes a direction from four actions, `y` down, no longer than 1,
  so diagonals aren't faster.
- `Binding` is `#[non_exhaustive]`, so match it with a `_` arm.

**Gamepads.** Gamepad bindings read any pad (the touch controls included):

| binding | text | value |
|---|---|---|
| `GamepadButton(b)` | `Pad:South` | 1 while held |
| `GamepadAxis { axis, direction: Both, dead_zone }` | `PadAxis:LeftX` | the axis (−1…1, a trigger 0…1) past the dead zone, rescaled to reach 1 |
| `GamepadAxis { axis, direction: Positive, .. }` | `PadAxis:LeftX+` | the positive half as 0…1: right, down, a trigger |
| `GamepadAxis { axis, direction: Negative, .. }` | `PadAxis:LeftX-` | the negative half as 0…1: left, up |

An axis binding is held while past its dead zone, and pressed or released
when it crosses it, so a half axis is a button: `PadAxis:LeftX+@0.5` is
"pushed right more than half way" (menu navigation, a D-pad stand-in).
`Binding::axis(axis, direction)` takes the default dead zone
(`Binding::DEFAULT_DEAD_ZONE`, 0.25), which the text form leaves out;
another is written `@0.35`.

For a stick, bind its two axes whole with no dead zone and read them
together with `stick`, which applies a radial dead zone (a per-axis one
snaps diagonals to the axes) and keeps the length within 1:

```rust
use stipple::game::{Actions, AxisDirection, Binding};
use stipple::gamepad::{GamepadAxis, GamepadButton};
use stipple::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Act { MoveX, MoveY, Jump, Right }

let raw = |axis| Binding::GamepadAxis { axis, direction: AxisDirection::Both, dead_zone: 0.0 };
let mut actions = Actions::new()
    .with(Act::MoveX, raw(GamepadAxis::LeftX))
    .with(Act::MoveX, Binding::KeyAxis { neg: PhysicalKey::KEY_A, pos: PhysicalKey::KEY_D })
    .with(Act::MoveY, raw(GamepadAxis::LeftY))
    .with(Act::Jump, Binding::GamepadButton(GamepadButton::South))
    .with(Act::Right, Binding::GamepadAxis {
        axis: GamepadAxis::LeftX,
        direction: AxisDirection::Positive,
        dead_zone: 0.5,
    });
# let input = stipple::game::InputState::new();
actions.update(&input);
let (dx, dy) = actions.stick(Act::MoveX, Act::MoveY, 0.2); // radial dead zone 0.2
```

**Rebinding.** `iter()` lists every action with its bindings, in the order
they were bound (the rows of a controls screen); `bindings(a)`,
`set_bindings(a, list)`, `rebind(a, slot, binding)` (a "primary" and
"secondary" column), `unbind(binding)` (take a key away from whatever had
it) and `bound_to(binding)` (conflicts). For a "press a key…" row:
`Binding::capture(frame.input())` gives the first key, mouse button or
gamepad button pressed since the last update, else a stick or trigger
pushed past half way (as the half axis it went along, `PadAxis:LeftY-`,
with the default dead zone); the touch controls aren't captured.
`Binding::from_key_event(ev)` does the same for keys from an `on_raw_key`
handler, for a settings screen shown while the game is paused.

**Saving them.** `to_text(name_of)` writes one line per action, and
`load_text(text, action_of)` reads it back over the defaults (actions not
in the text keep theirs; unknown names are skipped; a malformed line is an
error with its line number, and nothing changes):

```text
Left = Key:ArrowLeft, Key:KeyA, PadAxis:LeftX-@0.5
Jump = Key:Space, Mouse:Left, Pad:South
Steer = KeyAxis:KeyQ/KeyE, PadAxis:RightX
```

Keys are written by their W3C `code` name (`#` and the hex HID usage for a
key without one), mouse buttons as `Left`, `Right`, `Middle`, `Back`,
`Forward` or a number, pad buttons and axes by their `GamepadButton` and
`GamepadAxis` names. A single `Binding` prints and parses (`Display`,
`FromStr`) the same way.

## Drawing

A game draws its world into a canvas (`Element::canvas`, see
[canvas.md](canvas.md)) every frame. On top of the canvas's shapes, text and
bitmaps, a game gets:

- **Image regions**: `Scene::draw_image_region(src, dst, &image, &opts)` draws
  part of a bitmap: a frame of a sprite sheet, a tile of a tileset.
  `ImageOptions` flips it (`flip_x`, `flip_y`), rotates and scales it about a
  pivot (`rotate`, `scale`, `pivot`, as fractions of `dst`), multiplies it by a
  `tint`, fades it (`opacity`), and picks the sampling: `Filter::Linear`
  (smooth, the default) or `Filter::Nearest` (`.nearest()`: hard pixels).
  `draw_image_with(dst, &image, &opts)` draws a whole bitmap the same way.
  The pixels aren't copied: keep the `Arc<Pixmap>` and draw from it every
  frame.
- **`SpriteSheet`**: an image and its frames, cut as a grid
  (`SpriteSheet::grid(image, 16, 16)`, `grid_spaced` for margins and gaps) or
  added one by one, by name if you like (`add("coin", rect)`, `index("coin")`).
  `sheet.draw(scene, frame, dst, &opts)` and `draw_at(scene, frame, top_left,
  &opts)` draw a frame.
- **`SpriteAnimation`**: frames with durations (`SpriteAnimation::new(4..8,
  0.1)`, or `with_durations`), played `Loop`, `Once` or `PingPong`, sampled at
  a time: `walk.frame_at(t)` gives the sheet frame to draw.
- **`Tilemap`**: a grid of tile numbers (frames of a tileset sheet) with a
  tile size and an origin in the world. `map.draw(scene, &tileset, view,
  &opts)` draws only the cells that cross `view`, so a level of any size costs
  what's on screen; `cell_at`, `cell_rect` and `visible_cells` answer the
  usual questions.
- **`Camera2D`**: the world point at the screen's center (`position`), `zoom`,
  an optional `rotation`, and shake (`shake(amount)` adds trauma that fades as
  `update(dt)` runs). `follow(target, rate, dt)` trails a point smoothly,
  `clamp_to(bounds)` keeps the view inside the level. `transform()` is the
  world-to-screen `Affine`; `world_to_screen`, `screen_to_world` and
  `visible_rect` map between the two. Draw the world inside `cam.push(scene)`
  / `cam.pop(scene)` (a `Scene` layer) or `cam.draw(scene, |s| …)`. With
  `snap` (on by default) the translation is rounded to whole screen units, so
  pixel art doesn't shimmer while the camera glides.
- **`PixelViewport`**: a fixed virtual resolution for pixel art,
  `PixelViewport::new(320, 180)`. `vp.paint(canvas, |s| …)` gives the game a
  `Scene` of 320×180 to draw into, rasterizes it at that size, and blits it
  nearest-neighbor into the canvas at the largest whole-number scale that fits
  in device pixels, centered on whole pixels, with bars (`bar_color`) around
  it. `vp.pointer_to_virtual(e.position)` turns a pointer position on the
  canvas into virtual pixels (`None` on the bars); `layout(size, scale)` is
  the arithmetic on its own.

```rust
use std::sync::Arc;
use stipple::game::{Camera2D, PixelViewport, SpriteAnimation, SpriteSheet, Tilemap};
use stipple::prelude::*;
use stipple::render::ImageOptions;

struct World {
    sheet: SpriteSheet,
    map: Tilemap,
    walk: SpriteAnimation,
    cam: Camera2D,
    vp: PixelViewport,
    hero: Point,
    time: f64,
}

fn paint(w: &World, c: &mut Canvas) {
    let px = ImageOptions::new().nearest();
    w.vp.paint(c, |s| {
        w.cam.draw(s, |s| {
            w.map.draw(s, &w.sheet, w.cam.visible_rect(), &px);
            w.sheet.draw_at(s, w.walk.frame_at(w.time), w.hero, &px);
        });
    });
}
```

The `spritedemo` example is a complete scene: a sprite sheet painted pixel by
pixel at startup, a hero walking over a tilemap at 320×180 with the camera
following, coins spinning, clouds in parallax, a click to send the hero
somewhere (through `pointer_to_virtual` and `screen_to_world`), and the frame
rate printed every second. `spritedemo --png FILE` renders a frame
off-screen.

### How it draws, and what it costs

The software renderer draws image regions itself instead of handing them to
`oxideav-raster`, whose general path builds a canvas-sized coverage mask for
every primitive and samples images pixel by pixel through the inverse
transform. The scene is walked in paint order:

- an upright region drawn unscaled at whole pixels, or scaled
  nearest-neighbor (by whole numbers or not), is blitted through a table of
  source columns and rows: no transform per pixel, opaque runs copied whole,
  translucent pixels blended exactly as the rasterizer blends them;
- other upright regions (linear, scaled or at fractional positions) are
  sampled bilinear through per-column and per-row taps, clamped to the
  region so neighbors in an atlas don't bleed in, with antialiased edges;
- turned regions are mapped back pixel by pixel (nearest, or bilinear with
  antialiased edges);
- a solid rect on whole device pixels (a background, a health bar) is filled
  directly;
- everything else between them (paths, text) is rasterized by
  `oxideav-raster` for just the box it covers, and composited in order.

Regions inside a translucent layer, or a clip that isn't an upright rect,
are drawn by the rasterizer as plain images (nearest regions upscaled first
so they stay blocky). A canvas's paint callback now runs once per frame
rendered, not once per band.

The GPU path (`stipple::gpu`, see [gpu.md](gpu.md)) draws a region as a
textured quad from the whole bitmap's texture (cached by its `Arc`, uploaded
once): the region's texture coordinates (swapped for a flip), `GL_NEAREST`
for nearest sampling, the tint and opacity in the shader, and the samples
kept half a texel inside the region.

`spritedemo --bench` times the software renderer drawing a 1920×1080 frame:
a full-screen background image and 2000 32×32 sprites (discs with soft alpha
edges, so every sprite blends), single-threaded, release build, Apple M5 Max:

| What | ms per frame |
|---|---|
| Before: the same bitmaps through `draw_image` (the rasterizer's path) | 84 |
| Unscaled regions (`draw_image_region`) | 4.4 |
| 16×16 regions drawn 2× nearest, half of them flipped | 4.1 |
| Unscaled, tinted and at 80% opacity | 11 |
| Bilinear at fractional positions | 26 |
| Turned (bilinear) | 44 |

So a full 1080p screen of sprites fits a 60 Hz frame with room to spare when
they're drawn unscaled or scaled by whole numbers (what pixel art and most 2D
games draw); smooth subpixel motion and rotation cost more, and fit when
fewer sprites need them. A `PixelViewport` game rasterizes only its virtual
resolution (320×180 is 1/36 of 1080p) plus one nearest blit to the window.

Not yet: the frame isn't presented from the GPU without a read-back (Phase 6
work), and the software blits run on one thread.
