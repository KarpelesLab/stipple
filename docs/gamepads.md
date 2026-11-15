# Gamepads

`stipple::gamepad` reads game controllers on every platform, mapped to one
standard layout: the one printed on an Xbox or PlayStation pad.

## The API

```rust
use stipple::gamepad::{self, GamepadAxis, GamepadButton, GamepadEvent};

// Events, through the App: connects, disconnects, buttons, axes.
App::new(State::default(), view)
    .on_gamepad(|s, event| match event {
        GamepadEvent::Connected { id, info } => s.pads.push((*id, info.clone())),
        GamepadEvent::Disconnected { id } => s.pads.retain(|(p, _)| p != id),
        GamepadEvent::Button { button: GamepadButton::South, pressed: true, .. } => s.jump(),
        _ => {}
    })
    .run();

// Polling, from a per-frame update.
for (id, info) in gamepad::gamepads() {
    let pad = gamepad::state(id).unwrap_or_default();
    let (x, y) = pad.left_stick(); // −1..1, +Y down
    let fire = pad.pressed(GamepadButton::RightTrigger);
    let throttle = pad.axis(GamepadAxis::RightTrigger); // 0..1
}

// Rumble: low-frequency (strong) and high-frequency (weak) motors, 0..1.
gamepad::set_rumble(id, 0.8, 0.3, Duration::from_millis(250));
```

| item | what it is |
|---|---|
| `GamepadId` | a connected pad; never reused, so a pad that comes back gets a new one |
| `GamepadInfo` | `name`, and `vendor_id` / `product_id` where the platform gives them |
| `GamepadButton` | `South`, `East`, `West`, `North`, `LeftShoulder`, `RightShoulder`, `LeftTrigger`, `RightTrigger`, `Select`, `Start`, `Guide`, `LeftStick`, `RightStick`, `DPadUp` / `Down` / `Left` / `Right` |
| `GamepadAxis` | `LeftX`, `LeftY`, `RightX`, `RightY`, `LeftTrigger`, `RightTrigger` |
| `GamepadState` | `pressed(button)`, `axis(axis)`, `left_stick()`, `right_stick()`, `pressed_buttons()` |
| `GamepadEvent` | `Connected { id, info }`, `Disconnected { id }`, `Button { id, button, pressed }`, `Axis { id, axis, value }` |
| `gamepads()` | the connected pads, oldest first |
| `state(id)` | a pad's buttons and axes now |
| `set_rumble(id, low, high, duration)` | rumble; `false` where the pad or platform can't |
| `App::on_gamepad(hook)` | every event, with the state; the windows re-render after it |
| `Event::Gamepad` | the same events, for code on the raw `platform::run` loop |

Gamepad support starts the first time it's used (`on_gamepad`, `gamepads`,
`state`, `gamepad::start()`, or a game loop): devices are opened and, on
Windows and Linux, a thread starts. An app that never asks pays nothing.
Pads already connected then arrive as `Connected` events.

## With Actions

A game on [the game loop](games.md) needn't handle events or poll: the
frame's `InputState` follows every pad (`gamepad_held(None, GamepadButton::South)`,
`gamepad_axis(Some(id), GamepadAxis::LeftX)`, with latched presses and
releases, and everything let go on disconnect and focus loss), and
`Actions` take pad bindings next to keys, rebindable and saved the same way:

```rust
use stipple::game::{Actions, AxisDirection, Binding};
use stipple::gamepad::{GamepadAxis, GamepadButton};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Act { Jump, MoveX, MoveY, MenuDown }

let raw = |axis| Binding::GamepadAxis { axis, direction: AxisDirection::Both, dead_zone: 0.0 };
let actions = Actions::new()
    .with(Act::Jump, Binding::GamepadButton(GamepadButton::South)) // Pad:South
    .with(Act::MoveX, raw(GamepadAxis::LeftX))                     // PadAxis:LeftX@0
    .with(Act::MoveY, raw(GamepadAxis::LeftY))
    .with(Act::MenuDown, Binding::GamepadAxis {                    // PadAxis:LeftY+@0.5
        axis: GamepadAxis::LeftY,
        direction: AxisDirection::Positive,
        dead_zone: 0.5,
    });
// Per update: actions.update(frame.input());
// actions.stick(Act::MoveX, Act::MoveY, 0.2) is the left stick with a radial dead zone.
```

The bindings read any pad, which suits a single-player game; the on-screen
[touch controls](games.md#input) are one more pad to them. See
[games.md](games.md#actions) for dead zones, half axes and capturing a
binding from the pad.

## The layout

Face buttons are named by where they sit, not by their labels:

| | Xbox | PlayStation | Nintendo |
|---|---|---|---|
| `South` | A | ✕ | B |
| `East` | B | ○ | A |
| `West` | X | □ | Y |
| `North` | Y | △ | X |
| `Select` | View / Back | Share / Create | − |
| `Start` | Menu / Start | Options | + |
| `Guide` | Xbox | PS | Home |

**Axes.** Sticks run from −1 to 1, **+X right and +Y down** (screen
coordinates; XInput and GameController report up as positive and are
flipped). Triggers run from 0 to 1. Triggers are also buttons: where a pad has
no digital trigger, the button is pressed past 30/255 of the travel (XInput's
threshold). No dead zone is applied except the one Linux reports for each
stick (`flat`), so apply your own (0.15 to 0.25 is common).

## Per platform

| platform | source | hot-plug | ids | rumble |
|---|---|---|---|---|
| Windows | XInput: `xinput1_4.dll`, else `xinput9_1_0.dll`, loaded at run time; `XInputGetStateEx` (for Guide) where present. Up to 4 Xbox-compatible pads. Polled every 8 ms on a thread while a pad is connected (empty slots once a second), and again by `state()` | ✅ | — | ✅ `XInputSetState` |
| macOS, iOS | `GameController.framework` (loaded at run time): `GCController.controllers`' extended gamepads, polled 60 times a second on the main run loop while one is connected (twice a second otherwise), and by `state()` on the main thread. Input only reaches the frontmost app | ✅ | — | ❌ |
| Linux | evdev: `/dev/input/event*` with `BTN_SOUTH` and absolute axes, read on a thread; inotify on `/dev/input` for hot-plug | ✅ | ✅ | ✅ `FF_RUMBLE` |
| Android | the activity's key events from `AINPUT_SOURCE_GAMEPAD` (`AKEYCODE_BUTTON_*`, D-pad) and motion events from `AINPUT_SOURCE_JOYSTICK`; named through `InputDevice` | on first input; no disconnects | ✅ | ❌ |
| Web | `navigator.getGamepads()` polled each animation frame by `web/stipple.js` while a pad is connected; the standard mapping | ✅ (browsers show a pad after its first button press) | ✅ when the browser's `id` carries them | ✅ `vibrationActuator` where offered |

Notes:

- **Linux mapping** follows the kernel's
  [gamepad spec](https://docs.kernel.org/input/gamepad.html), with the usual
  departures: triggers on `ABS_Z` / `ABS_RZ` (xpad, hid-playstation) or
  `ABS_BRAKE` / `ABS_GAS` as well as `ABS_HAT2Y` / `ABS_HAT2X`; a right
  stick on `ABS_Z` / `ABS_RZ` for pads without `ABS_RX` / `ABS_RY`; the D-pad
  as `BTN_DPAD_*` or `ABS_HAT0X` / `ABS_HAT0Y`. xpad (and Microsoft's pads)
  report the Xbox X and Y buttons as `BTN_X` / `BTN_Y`, which share their codes
  with `BTN_NORTH` / `BTN_WEST`; those are swapped back. After `SYN_DROPPED`
  the whole state is read again.
- **Android**: D-pad keys from a pad also go on as arrow keys, so the UI can
  be driven with the pad; the other buttons go only to the gamepad.
- **Web**: pads the browser doesn't map to the standard layout are read as if
  it did.

## Linux permissions

evdev nodes belong to `root:input`. Desktop sessions usually grant the
logged-in user access to game controllers through udev's `uaccess` tag
(systemd's `70-uaccess.rules` tags joysticks; Steam's udev rules tag many
pads). Otherwise, either add the user to the `input` group
(`sudo usermod -aG input $USER`, then log in again; this also exposes
keyboards, so prefer the next option), or add a rule such as:

```
# /etc/udev/rules.d/70-gamepads.rules
SUBSYSTEM=="input", ENV{ID_INPUT_JOYSTICK}=="1", TAG+="uaccess"
```

A device that can't be opened is skipped: no pads, no error. Rumble needs the
device opened read-write; read-only access still gives input.

## The example

`examples/gamepaddemo` shows each connected pad's buttons and axes live, logs
every event (also printed to stdout), and rumbles a pad when South is
pressed. `gamepaddemo --list` skips the window: it prints the pads it finds
and exits.
