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

## Themes and the native theme

These widgets read all their colors and metrics from the theme, so they follow
every built-in theme. Under `Theme::native()` on Windows, macOS and iOS, the OS draws
the select's button bezel (as a regular push button) and the number input's
field frame. The other parts are drawn by Stipple in the system's colors,
because the platforms have no matching control that Stipple can draw yet:
the stepper buttons, the split divider, toolbars and toasts.

## Accessibility

| Widget | Role | Name |
|---|---|---|
| Select | `ComboBox`; its open list is a `List` of `ListItem`s | the chosen option |
| Number input | `SpinButton`, with `Increase` and `Decrease` buttons | the text |
| Split pane divider | `Splitter` | — |
| Toolbar | `Toolbar`, with buttons and `Separator`s | — |
| Toast | `Alert`, with a `Dismiss` button | the message |

Set a role or name on your own elements with `.role(Role::…)` and
`.a11y_label("…")`. The OS bridges (AT-SPI, UI Automation, NSAccessibility)
don't have these roles yet, so they report the closest role they do have:
a button, a text field, or a group.
