# Widgets

Most widgets are plain functions that take the theme and return an `Element`:
`label`, `button_labeled`, `checkbox`, `slider`, `text_editor` and so on (see
the [API docs](https://docs.rs/stipple/latest/stipple/widgets/)). This page
covers the ones that keep state between frames. Each has a state type that you
store in your app state, and the widget takes an accessor closure to reach it,
the same way `text_editor` takes its `EditBuffer`. The `widgetsdemo` example
shows them all:

```sh
cargo run -p widgetsdemo            # or: dark, material, material-dark, contrast, native
```

The tab bar, password field and color picker are in `sessionsdemo`, a
terminal-like app's chrome (with message boxes, see
[os-integration.md](os-integration.md#message-boxes)):

```sh
cargo run -p sessionsdemo           # the same theme names
```

## Select

A button showing the chosen option. Clicking it opens the list of options
below it, or above it when there's no room below.

```rust
struct App { size: Select }
// in the view:
select(cx, &theme, &["Small", "Medium", "Large"], &state.size, |s: &mut App| &mut s.size)
```

Read the choice with `state.size.selected()`. The list closes when an option
is chosen, on Escape, or on a press outside it. While the select has focus,
Down, Up, Enter and Space open the list. In the open list the arrow keys, Home
and End move the highlight, and Enter or Space chooses. Typing a letter jumps to
the next option that starts with it. Lists longer than eight options scroll, but
moving the highlight with the keyboard doesn't scroll them yet.

## Number input

An editable field with ▴ and ▾ stepper buttons.

```rust
struct App { width: NumberInput }
let width = NumberInput::new(640.0).range(100.0, 4000.0).step(10.0);
// in the view:
number_input(cx, &theme, &state.width, |s: &mut App| &mut s.width)
```

`value()` is always a finite number within the range. Typing only accepts
characters that can appear in a number. Each edit that parses updates the
value, clamped to the range. Text that isn't a number in range gets a red
border, and the value keeps its last good number. Enter rewrites the text from
the value, and Escape reverts the text. Up and Down add or subtract one step.
The text shows as many decimals as the step has, or as many as you set with
`.precision(n)`.

## Split pane

Two panes with a divider between them.

```rust
struct App { sidebar: SplitPane }
let sidebar = SplitPane::horizontal(0.3).min_sizes(200.0, 300.0); // or ::vertical
// in the view:
split_pane(cx, &theme, &state.sidebar, |s: &mut App| &mut s.sidebar, nav, content)
```

A horizontal split puts the panes side by side, and a vertical split stacks
them. The divider stays under the point where you grabbed it while you drag. It
stops where a pane would get smaller than its minimum size. With the divider
focused (Tab), the arrow keys move it by 5%, and Home and End move it as far as
it can go. Each pane is clipped to its share. The split grows to fill its
container, so a vertical split needs a height from its container.

Splits nest: put a vertical split in one pane of a horizontal one.

## Toolbar

A row of compact buttons on a surface strip.

```rust
toolbar(&theme, vec![
    tool_button(cx, &theme, "Save", |s: &mut App| s.save()),
    tool_separator(&theme),
    tool_toggle(cx, &theme, "B", state.bold, |s: &mut App| s.bold = !s.bold),
    tool_icon_button(cx, &theme, &state.trash_icon, "Delete", |s: &mut App| s.delete()),
    spacer(),                      // push the rest to the end
    tool_button(cx, &theme, "Help", |s: &mut App| s.help()),
])
```

Icons are drawn at 18×18 and can be a `Pixmap` or an `Svg`. Keep the `Arc` in
your state, because images compare by identity. An icon button's `name` is
what screen readers read out. Any element can go in a toolbar, a `select` for
example. Items that don't fit overflow the toolbar; it doesn't move them to an
overflow menu yet.

## Toasts

Short notifications, stacked in the window's bottom-right corner above
everything else.

```rust
struct App { toasts: Toasts }
// in the view, once per frame (it adds an overlay rather than returning an element):
toasts(cx, &theme, &state.toasts, |s: &mut App| &mut s.toasts);

// A toast that stays until its × is clicked:
state.toasts.push("Connected", ToastKind::Info);

// A toast that dismisses itself after 4 seconds, using a timer on the app's handle:
let handle = cx.handle();
button_labeled(&theme, "Save").on_tap(cx, move |s: &mut App| {
    s.toasts.push_for(&handle, Duration::from_secs(4), |s: &mut App| &mut s.toasts,
        "Saved", ToastKind::Success);
})
```

`ToastKind` (`Info`, `Success`, `Warning`, `Error`) sets the color of the accent
bar. At most four toasts show at once, the newest ones. Clicks outside the
toasts reach the UI underneath.

## Tab bar

A document tab bar, as in a browser, an editor or a terminal. The app keeps
the tabs; the bar reports what the user does to them.

```rust
struct App { docs: Vec<Doc>, selected: usize, bar: TabBarState }

let tabs = state.docs.iter().map(|d| {
    Tab::new(d.id, d.name.clone())          // a stable id per tab
        .color(d.color)                      // a strip along the top and a tint
        .indicator(d.bell.then_some(TabIndicator::Bell)) // or Dot, Count(n)
        .context_menu(Menu::new("").item(MenuItem::new("Close Others", ...)))
}).collect();
let bar = TabBar::new("docs", tabs)
    .selected(state.selected)
    .on_select(|s: &mut App, i| s.selected = i)
    .on_close(|s: &mut App, i| { s.docs.remove(i); })
    .on_reorder(|s: &mut App, from, to| { let d = s.docs.remove(from); s.docs.insert(to, d); })
    .on_new(|s: &mut App| s.docs.push(Doc::new()));
tab_bar(cx, &theme, bar, &state.bar, |s: &mut App| &mut s.bar)
```

- Clicking a tab selects it; its × closes it. Middle-click to close and
  double-click to rename wait on per-button pointer events.
- With `on_reorder`, a tab drags (an in-app drag, `DropData::Internal`) onto
  another to move there; the tabs it passes slide into their new places on a
  spring.
- The tabs share the bar's width, between `TAB_MIN_WIDTH` (110) and
  `TAB_MAX_WIDTH` (220). When they don't fit, the bar shows as many as do,
  with ‹ › arrows to scroll and a ▾ button listing every tab; a newly selected
  tab is scrolled into view. The bar measures against the window's width
  (`Cx::window_size`) unless given `.width(..)`.
- Focused (Tab), ←/→ select the previous or next tab, Home/End the first or
  last, and Delete closes the selected one.
- Tabs fade their hover look in and out (`.hovered` with a `.transition`), as
  do the × and the bar's buttons, and a dragged tab dims (`.dragged`).

Dragging a tab out into a window of its own, and macOS's native window tabs,
aren't done.

## Password field

A single-line field that shows a bullet per character.

```rust
struct App { password: PasswordField }
let state = App { password: PasswordField::new().with_reveal_button(true) };
// in the view:
password_field(cx, &theme, &state.password, |s: &mut App| &mut s.password)
// read it with state.password.text(); clear() empties it
```

- Copy and cut do nothing (even revealed); paste works. Its edits keep no undo
  history, and word motions go to the ends.
- It takes no input method: the IME is turned off while it has focus (it would
  see the password, and may learn it), so typing goes straight in.
- On macOS, secure keyboard entry (`EnableSecureEventInput`) is on while it has
  focus, so other processes can't read the keystrokes through event taps
  (`platform::secure_input()` tells). The other platforms have no equivalent
  an app turns on.
- `with_reveal_button(true)` adds an eye button that shows the text while on.

## Color picker

`color_picker(cx, &theme, &state.picker, field, on_change)` is an inline
picker: a saturation/value area for the current hue, a hue strip, an alpha
strip, a hex field and a row of swatches. `color_button(...)` is a swatch
button that opens it in a popup below.

```rust
struct App { tint: ColorPicker, accent: Color }
let state = App { tint: ColorPicker::new(Color::rgb(0, 0x90, 0xff)), ... };
color_button(cx, &theme, &state.tint, |s: &mut App| &mut s.tint,
    |s: &mut App, c| s.accent = c)
```

The picker keeps the hue, saturation and value apart, so dragging to gray and
back keeps the hue. The hex field takes `#rgb`, `#rrggbb` and `#rrggbbaa`; the
color follows as soon as what's typed parses. `ColorPicker::with_alpha(false)`
drops the alpha strip and `with_swatches` replaces the swatches.

`platform::dialog::pick_color(initial)` opens the platform's color dialog
instead, where it has one: `ChooseColorW` on Windows (it blocks, like the file
dialogs). Elsewhere it returns `None`: macOS's `NSColorPanel` is a floating
panel rather than a dialog, and the desktop portal only picks a color off the
screen.

## Themes and the native theme

These widgets read all their colors and metrics from the theme, so they follow
every built-in theme. Under `Theme::native()` on Windows, macOS and iOS, the OS draws
the select's button bezel (as a regular push button) and the number input's
field frame. The other parts are drawn by Stipple in the system's colors,
because the platforms have no matching control that Stipple can draw yet:
the stepper buttons, the split divider, toolbars, toasts, the tab bar and
the color picker. The password field's frame is the OS's text field frame
there.

## Accessibility

| Widget | Role | Name |
|---|---|---|
| Select | `ComboBox`; its open list is a `List` of `ListItem`s | the chosen option |
| Number input | `SpinButton`, with `Increase` and `Decrease` buttons | the text |
| Split pane divider | `Splitter` | — |
| Toolbar | `Toolbar`, with buttons and `Separator`s | — |
| Toast | `Alert`, with a `Dismiss` button | the message |
| Tab bar | `TabList` of `Tab`s (the current one selected), with buttons | the tab's label |
| Password field | `PasswordField` | its label ("Password"), never the text |
| Color picker | `Group`; the button is a `Button`, expanded while open | "Color #rrggbb" |
| Drawn message box | `Dialog` | its title, or else its message |

Set a role or name on your own elements with `.role(Role::…)` and
`.a11y_label("…")`. The OS bridges (AT-SPI, UI Automation, NSAccessibility)
don't have these roles yet, so they report the closest role they do have:
a button, a text field, or a group. Menus (see [menus.md](menus.md)) report
`MenuBar`, `Menu` and `MenuItem`, which the bridges do map, and tabs map to
UI Automation's `Tab`/`TabItem`, NSAccessibility's `AXTabGroup`, and ARIA's
`tablist`/`tab`.
