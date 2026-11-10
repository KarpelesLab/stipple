# The keyboard: raw keys, Tab, repeat and Option as Meta

Most widgets take the keyboard through `Element::on_key`: they get
`KeyInput`s, which the App has already mapped from keys (`Backspace`,
`SelectLeft`, `Copy`, typed `Text`). Keys with no mapping are dropped, Tab
moves focus, and Escape and the menu keys are the toolkit's.

An element that needs **every** key, such as a terminal, a game or a key-binding
editor, adds `Element::on_raw_key`.

```rust
let terminal = column(rows)
    .on_raw_key(cx, |term: &mut Term, ev: &KeyEvent| {
        if !ev.is_press() {
            return Handled::No;
        }
        match term.encode(ev) {
            Some(bytes) => {
                term.pty.write(&bytes);
                Handled::Yes
            }
            None => Handled::No, // let the toolkit have it
        }
    })
    // IME commits and dead-key text arrive here.
    .on_key(cx, |term: &mut Term, input: &KeyInput| term.input(input))
    .captures_tab(true)
    .ime_cursor_area(cursor_cell)
    .option_as_meta(OptionAsMeta::Left);
```

## `KeyEvent`

The handler gets each press, including repeats, and each release as a
`KeyEvent`. It follows the W3C split between `code` and `key`:

| Field | What it is |
|---|---|
| `state` | `Pressed` or `Released`. |
| `logical` | The key under the current layout. It is either a named key (`Enter`, `F5`, `PageUp`, `Shift`, `Control`, `Alt`, `Meta`, `AltGraph`, `CapsLock`…) or `Char(c)`. `c` is the character with Shift applied as held, but without Ctrl, Alt or AltGr: `Char('a')`, `Char('A')` with Shift, and `Char('1')` for keypad 1 with Num Lock on. This is the identity `Event::Key` and `Shortcut`s use. |
| `physical` | The key's position, whatever the layout. It is a USB HID usage, and `.name()` gives the W3C `code` (`"KeyA"`, `"Numpad1"`, `"ShiftRight"`). On AZERTY, the key with `physical.name() == Some("KeyA")` types `q`. |
| `location` | `Standard`, `Left` or `Right` (modifiers), or `Numpad`. Keypad keys can be told apart from the main block, which DECKPAM application keypad mode needs. |
| `modifiers` | Shift, Ctrl, Alt and the logo key, as elsewhere. |
| `sides` | Which physical modifier keys are held, left and right apart (`sides.left_alt()`, `sides.right_ctrl()`…). |
| `repeat` | `true` for a press the OS (or, on Wayland, Stipple) generated because the key is held. |
| `text` | What the key types with the modifiers held, as a text field would get it: Shift, AltGr and, on macOS, Option applied, but not Ctrl or Cmd. It is `None` for named keys and releases, and never a control character. A terminal maps Ctrl itself from `logical`. |
| `text_ignoring_modifiers` | What the key types with only Shift applied (AppKit's `charactersIgnoringModifiers`). On macOS, Option+B gives `text` `"∫"` and this `"b"`: "Option as Meta" sends `ESC b`. |
| `scancode` | The platform's own code, for logging: the X11 or xkb keycode, the Windows scan code (`0xe0`-prefixed when extended), the macOS virtual key code, the Android key code. |

## Who gets a key first

For a key press in a window, in this order:

1. **The OS.** It may keep keys for itself: Alt+Tab, the Windows key, and
   global shortcuts. On macOS, the native menu bar matches its key
   equivalents.
2. **The input method and dead keys.** When the focused element takes IME
   input (a text field, or an element with `ime_input`), the IME sees the key
   first and may keep it, for example to compose `にほん`. Stipple's Compose
   state on X11 and Wayland also keeps the keys of a dead-key or Compose
   sequence. Text that comes out of either arrives through `on_key` as
   `KeyInput::Preedit` and `KeyInput::Text`, not as raw keys.
3. **Menus.** An open menu, or a menu bar focused from the keyboard, takes
   every key. A key that matches a menu bar `Shortcut` runs its command. So
   Cmd+T can still open a tab in a terminal.
4. **An in-app drag.** Escape cancels it.
5. **The focused element's `on_raw_key`.** Returning `Handled::Yes` ends the
   key here: no `Event::Key` or `Event::Text` follows for it, so it is never
   mapped, never moves focus and never types.
6. **The toolkit**, for keys the raw handler returned `Handled::No` for, or
   when there is no raw handler:
   - Tab moves focus, unless the element captures Tab.
   - F10, Alt+letter and the context-menu key open the menus.
   - Editing keys go to `on_key` as `KeyInput`s.
   - Typed characters go to `on_key` as `KeyInput::Text`.

Releases go to the raw handler too, after an open menu. Alt pressed and
released on its own focuses the in-window menu bar, unless the raw handler
used the press.

A viewport that holds input focus (see `Element::viewport`) gets keys through
its `ViewportEvent::Key` sink instead. Raw key handlers don't run then.

## Keeping Tab

`Element::captures_tab(true)` keeps Tab and Shift+Tab with the element.
Focus doesn't move. The raw handler gets the keys first. If it doesn't
handle Tab, `on_key` gets `KeyInput::Text("\t")`; an unhandled Shift+Tab does
nothing.

A raw handler that returns `Handled::Yes` for Tab keeps it even without
`captures_tab`. `captures_tab` makes sure focus stays even when the handler
passes Tab on, and it gives Tab to elements that only have `on_key`.

## Input methods for custom elements

Text fields switch the IME on when they take focus. For any other element,
`Element::ime_input(true)` does the same. `Element::ime_cursor_area(rect)`
also places the IME's candidate window: `rect` is relative to the element's
top-left, so a terminal passes its cursor cell. Without it, the window goes
by the element's whole box.

The composition arrives at the element's `on_key` handler. It comes as
`KeyInput::Preedit { text, cursor }`, which the element draws at its cursor,
and committed text comes as `KeyInput::Text`. If focus moves away
mid-composition, the element gets an empty preedit. This works on every
platform with an IME: `text-input-v3` on Wayland, XIM on X11, IMM32 on
Windows, `NSTextInputClient` on macOS, and the hidden textarea on the web. A
viewport element (`Element::viewport(id).ime_input(true)`) gets the IME while
it holds input focus. See [ime.md](ime.md).

With the IME on, keys it doesn't use still arrive raw: Ctrl combinations,
arrows, and on X11 every key an XIM server sends back. Keys that type text
arrive as commits instead.

## Option as Meta (macOS)

`Element::option_as_meta(OptionAsMeta::Left)` makes the left Option key a Meta
key while the element has focus. `Right` and `Both` work the same way. With
such an Option key held:

- the keyboard layout doesn't compose, so `KeyEvent::text` is `"b"` for
  Option+B, not `"∫"`;
- the key skips the input method.

The other Option key still composes, which suits users who type `@` or `€`
with right Option. Elsewhere Alt doesn't compose, so this setting has no
effect there. `KeyEvent::text_ignoring_modifiers` and `KeyEvent::sides` let
an app make the same choice itself.

## Key repeat

| Platform | Repeat comes from | `repeat` flag |
|---|---|---|
| X11 | The X server. Stipple turns on XKB's *detectable* auto-repeat (`XkbPerClientFlags`), so a held key sends presses with no fake release between them. | A press of a key already down. Without XKB, a press with the timestamp of the release just before it. |
| Wayland | Stipple. The compositor sends `wl_keyboard.repeat_info` (rate and delay), and the event loop repeats the last key pressed while it is held. The repeat runs on the same deadline as the loop's timers, so it doesn't busy-wait. A rate of 0 turns repeat off. Releasing the key, pressing another one or focus leaving the window (`wl_keyboard.leave`) stops it. Modifiers don't repeat and don't stop it. | Set on the repeats. Each repeat also sends the key's `Key`/`Text`, so a held Backspace or letter repeats in a text field. |
| Windows | The OS: `WM_KEYDOWN`. | `lParam` bit 30. |
| macOS | The OS: `keyDown:`. | `isARepeat`. |
| Web | The browser. | `KeyboardEvent.repeat`. |
| Android | The OS. | `getRepeatCount() > 0`. |
| iOS | UIKit, through `insertText:`. | Always `false`; `pressesBegan:` doesn't say. |

## Per platform

- **X11.** The physical key is the keycode minus 8 (the evdev code, under
  every XKB keymap). The text comes from the core keyboard mapping, at the
  level Shift, Caps Lock, Num Lock and AltGr select. Sides are tracked from
  the modifier keys' own events. A Stipple client that initializes XKB also
  selects `XkbNewKeyboardNotify` and `XkbMapNotify` events to keep refreshing
  its keymap. `STIPPLE_X11_NO_XKB_REPEAT=1` leaves XKB off.
- **Wayland.** The physical key is the evdev code, when the compositor's
  keymap uses xkb's evdev keycodes, as a real keyboard's does. A virtual
  keyboard such as `wtype`'s numbers its own keycodes in the order it first
  types the keys, so there the physical key is unidentified
  (`PhysicalKey::UNIDENTIFIED`) rather than wrong. The logical key and the
  text come from the keymap's symbols (Shift and Caps Lock levels). Sides are
  tracked from the modifier keys' own events.
- **Windows.** The physical key comes from the scan code and the extended-key
  flag. Sides come from `GetKeyState(VK_LSHIFT…)`. The text comes from
  `ToUnicode` with flag 4, which leaves a pending dead key alone (Windows 10
  1607 and later). Keys the IME takes (`VK_PROCESSKEY`) aren't raw. When the
  app uses a key, the `WM_CHAR`s posted for it are dropped.
- **macOS.** The physical key comes from `keyCode`. Sides come from the
  device-dependent bits of `modifierFlags`. The text is `characters`, and
  `text_ignoring_modifiers` is `charactersIgnoringModifiers`. Modifier keys on
  their own (`flagsChanged:`) are raw keys too. A key the app uses doesn't go
  on to AppKit.
- **Web.** The physical key is `KeyboardEvent.code`. The text is
  `KeyboardEvent.key`. `text_ignoring_modifiers` is worked out from `code`
  for letters, digits and Space, and is `key` otherwise. A key the app uses
  gets `preventDefault`.
- **Android.** The physical key comes from the scan code (evdev). Sides come
  from the meta state. The text comes from `KeyCharacterMap` over JNI.
- **iOS.** The physical key is `UIKey.keyCode` (a HID usage). A press the app
  uses doesn't go on to UIKit's text system.

## Below the App

Apps that drive a backend directly see `Event::RawKey(KeyEvent)` before each
key's `Event::Key` or `Event::Text`. Calling
`stipple::platform::consume_key()` while handling it stops the backend from
translating that key any further.

## Testing

Unit tests cover:

- the HID tables (evdev, Windows scan codes, macOS key codes, W3C names);
- keysym levels and text;
- side tracking and the repeat timer;
- the X11 XKB requests and repeat detection;
- the Wayland key path, including repeats;
- at App level, the order of precedence: raw keys before Tab, editing keys
  and Ctrl+C; menu shortcuts and open menus before raw keys; fall-through to
  `on_key`; `captures_tab`; the IME and Option-as-Meta following the focused
  custom element.

In CI, on X11, `xdotool` types F-keys, Ctrl+letters, Tab, Shift+Tab, Escape
and Page Up into `examples/rawkeys`, and holds a key. The job checks what
arrives: each key's logical and physical identity, modifiers and sides, that
Ctrl+T runs the menu command instead of reaching the element, and one press,
then repeats, then one release for the held key. Under sway, `wtype` does the
same with F5, Ctrl+D and Tab, and holds a key past the repeat delay. The job
checks the logical keys and modifiers (wtype's keycodes aren't positions, so
the physical keys are unidentified there), that `repeat_info` arrived, and one
press, then repeats, then one release for the held key. Windows, macOS, the
web, iOS and Android are build-checked, with their pure mapping code
unit-tested on Linux.

## Not yet

- On Windows, `DefWindowProc` still sees an Alt+key system message the app
  used, because the window procedure runs before the app answers.
- On macOS, with Option as Meta set but the raw handler passing the key on,
  the usual path still types the composed character.
- Stipple reads only the first two levels (plain and Shift) of the Wayland
  xkb keymap, so AltGr characters aren't typed there yet, raw or not.
- On X11 and Wayland, Stipple's dead keys and Compose sequences take their
  keys before the raw handler, as an IME would.
- Modifiers pressed while another window had focus are only known by side
  once they are pressed again (X11, Wayland, the web).
