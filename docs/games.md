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

The rest of Phase 9 (see the [roadmap](../ROADMAP.md#phase-9--2d-games)),
pointer lock, gamepads, touch controls, sprites and tilemaps, audio and game
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
| `motion()`, `wheel()` | How far it moved, and the wheel turned (in pixels; lines count 40 px), since the last update. |

*Pressed* and *released* are edges since the last update, and they are
latched: a key pressed and released between two frames reads as both on the
next frame, so a quick tap is never lost. A key repeat is not a press. When
the window loses focus, every held key and button is let go and reported
released, so a key released in another window doesn't stay held.

The game sees keys whether or not a widget uses them too; while a text
field in a menu has focus, pause the game (or ignore its input).

An `InputState` can also be fed by hand (`key`, `button`, `pointer_moved`,
`wheel_by`, `event`, then `end_update` once per update), for a test or for
input the game routes itself.

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
  -1/0/1 for a `KeyAxis`), clamped to -1…1. `axis2` makes a direction from
  four actions, `y` down, no longer than 1, so diagonals aren't faster.
- `Binding` is `#[non_exhaustive]`: gamepad buttons and axes join it.

**Rebinding.** `iter()` lists every action with its bindings, in the order
they were bound (the rows of a controls screen); `bindings(a)`,
`set_bindings(a, list)`, `rebind(a, slot, binding)` (a "primary" and
"secondary" column), `unbind(binding)` (take a key away from whatever had
it) and `bound_to(binding)` (conflicts). For a "press a key…" row:
`Binding::capture(frame.input())` gives the first key or mouse button
pressed since the last update, and `Binding::from_key_event(ev)` does the
same from an `on_raw_key` handler, for a settings screen shown while the
game is paused.

**Saving them.** `to_text(name_of)` writes one line per action, and
`load_text(text, action_of)` reads it back over the defaults (actions not
in the text keep theirs; unknown names are skipped; a malformed line is an
error with its line number, and nothing changes):

```text
Left = Key:ArrowLeft, Key:KeyA
Jump = Key:Space, Mouse:Left
Steer = KeyAxis:KeyQ/KeyE
```

Keys are written by their W3C `code` name (`#` and the hex HID usage for a
key without one), mouse buttons as `Left`, `Right`, `Middle`, `Back`,
`Forward` or a number. A single `Binding` prints and parses (`Display`,
`FromStr`) the same way.
