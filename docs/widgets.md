# Widgets

<!-- live: widgetsdemo -->

Widgets are plain functions that take the view's `cx` first and return an
`Element`: `label(cx, "Hi")`, `button(cx, "Save")`, `checkbox(cx, "Wi-Fi",
on, on_toggle)` and so on (see the
[API docs](https://docs.rs/stipple/latest/stipple/widgets/)). They read their
colors and metrics from the theme `cx` carries, and the handlers that follow
`cx` get their state type from it, so closures need no annotations:

```rust
struct Todo { draft: TextInput, items: Vec<String>, done: Vec<bool> }

fn view(state: &Todo, cx: &mut Cx<Todo>) -> Element {
    let mut rows = vec![heading(cx, "Todo")];
    for (i, item) in state.items.iter().enumerate() {
        rows.push(checkbox(cx, item, state.done[i], move |s| {
            s.done[i] = !s.done[i]
        }));
    }
    rows.push(row(vec![
        text_field(cx, &state.draft, |s| &mut s.draft)
            .placeholder("What needs doing?")
            .on_submit(cx, Todo::add),
        button(cx, "Add").on_tap(cx, Todo::add),
    ]));
    column(rows).gap(8.0).padding(Insets::uniform(16.0))
}
```

- **Buttons.** `button(cx, "Save")` is a primary button. For another variant
  or for custom content, build a `Button` and finish it with `view(cx)`:
  `Button::new("Delete").variant(Variant::Danger).view(cx)`,
  `Button::from_content(icon).variant(Variant::Ghost).view(cx)`. Make any of
  them do something with `.on_tap(cx, |s| …)`.
- **Checkboxes, switches and radio buttons** take their label:
  `checkbox(cx, "Show hidden files", checked, |s| …)`,
  `switch(cx, "Sync", on, |s| …)`, `radio(cx, "Large", selected, |s| …)`.
  The label is drawn next to the control (after a checkbox or radio button,
  before a switch, whose knob then sits at the end of a stretched row), a
  click on it toggles the control, and it is the control's accessible name.
  When something else shows what the control is for (a checkbox in each
  table row, a settings row's caption), `Toggle` hides the label and keeps
  the name: `Toggle::checkbox("Select row 3", picked).label_visible(false)
  .view(cx).on_tap(cx, |s| …)`.
- **Text.** `label` is a line of body text, `heading` a larger one, and
  `paragraph` body text that wraps.
- **Text input.** `text_field` (one line) and `text_area` (several) edit a
  `TextInput` kept in the app state, reached through an accessor,
  `|s| &mut s.draft`. It has undo and redo, selection, the clipboard, input
  methods and right-to-left text (its `EditBuffer`, `buffer()` /
  `buffer_mut()`), and `TextInput::new().max_length(n).read_only(b)` limit
  it. On the field: `.placeholder(..)`, `.on_submit(cx, ..)` (Enter, single
  line only) and `.on_change(cx, ..)` (after each edit). `take()` empties it
  and returns the text.
- **Containers.** Plain layout takes its children directly and no `cx`:
  `row(vec![…])`, `column(vec![…])`, `grid(tracks, vec![…])`, `spacer()`.
  Containers that need `cx` (`panel`, `toolbar`, `scroll`,
  `scroll_horizontal`, and the game UI's `menu_list` and `setting_row`)
  return their frame, and you add the children after it:
  `panel(cx).children(vec![…])`, `scroll(cx).child(list)`. The frame
  is built before the children, so they can use `cx` too. `split_pane`
  takes its two `.panes(first, second)`, and `dialog` its content and
  `.actions(…)` before `.open(cx)`.
- **Another theme for part of a view**, for previews and theme galleries:
  `cx.themed(Theme::dark(), |cx| panel(cx).children(…))`.

Accessors infer like handlers do, as long as the closure is written in the
call, after `cx`. A closure kept in a variable first has no `cx` to learn the
state type from, so it needs one: `let name = |s: &mut App| &mut s.name;`.

The rest of this page covers the widgets that keep state between frames. Each
has a state type that you store in your app state, and the widget takes an
accessor closure to reach it, the same way `text_field` takes its
`TextInput`. The `widgetsdemo` example shows them all:

```sh
cargo run --example widgetsdemo            # or: dark, material, material-dark, contrast, native
```

`widgetsdemo` switches between its pages with a navigation bar. The tab bar,
password field and color picker are in `sessionsdemo`, a
terminal-like app's chrome (with message boxes, see
[os-integration.md](os-integration.md#message-boxes)):

```sh
cargo run --example sessionsdemo           # the same theme names
```

## Select

A button showing the chosen option. Clicking it opens the list of options
below it, or above it when there's no room below.

```rust
struct App { size: Select }
// in the view:
select(cx, &["Small", "Medium", "Large"], &state.size, |s| &mut s.size)
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
number_input(cx, &state.width, |s| &mut s.width)
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
split_pane(cx, &state.sidebar, |s| &mut s.sidebar).panes(nav(cx), content(cx))
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
toolbar(cx).children(vec![
    tool_button(cx, "Save").on_tap(cx, |s| s.save()),
    tool_separator(cx),
    ToolButton::new("B").selected(state.bold).view(cx).on_tap(cx, |s| s.bold = !s.bold),
    ToolButton::from_icon(&state.trash_icon, "Delete").view(cx).on_tap(cx, |s| s.delete()),
    spacer(),                      // push the rest to the end
    tool_button(cx, "Help").on_tap(cx, |s| s.help()),
])
```

`tool_button` is a text button; a `ToolButton` can show an icon instead, and
stays tinted while `selected` (a toggle that is on). Icons are drawn at 18×18
and can be a `Pixmap` or an `Svg`. Keep the `Arc` in your state, because
images compare by identity. An icon button's name is what screen readers read
out. Any element can go in a toolbar, a `select` for
example. Items that don't fit overflow the toolbar; it doesn't move them to an
overflow menu yet.

## Chips

Small pill-shaped toggles: one alone, a group that picks one option, or a
group of filters each on or off. A selected chip is filled with the accent
color and led by a check mark, which slides and fades in on the theme's
`toggle` motion (a chip's icon gives way to it).

```rust
struct App { symbology: usize, filters: [bool; 3], flash: bool }
// in the view:
choice_chips(cx, ["2D", "Stacked", "Linear", "Postal"], state.symbology,
    |s, i| s.symbology = i)
filter_chips(cx, ["QR", "EAN", "Code 128"], &state.filters,
    |s, i| s.filters[i] = !s.filters[i])
chip(cx, Chip::new("Flash").icon(&state.flash_icon), state.flash,
    |s| s.flash = !s.flash)
```

The app keeps the selection; the chips report taps. `choice_chips` takes the
selected index, or `None` for none. Labels can be `&str`s, `String`s or
`Chip`s (a `Chip` adds a leading icon, a `Pixmap` or an `Svg`).

- A group lays its chips out in a row that wraps onto more lines when they
  don't fit the width its container gives it (stretch it across, as a panel
  does). Inside `scroll_horizontal` it stays on one line and scrolls
  sideways.
- `choice_chips` are radio buttons: Tab stops at the selected chip only, and
  the arrow keys move to the previous or next chip and pick it, wrapping;
  Home and End go to the first and the last.
- `filter_chips` are checkboxes: each is a Tab stop, the arrow keys move focus
  between them without toggling, and Enter or Space toggles the focused one.
- In a right-to-left layout the row is mirrored and Left and Right follow it.

The arrow keys come from `Element::focus_group`, which any container of
buttons can use: `FocusGroup::Select` for a radio group (put `radio`s in it),
`FocusGroup::Move` for a row of checkboxes or tool buttons.

## Toasts

Short notifications above everything else: stacked in the window's
bottom-right corner on a desktop, or one at a time as a bar across the bottom
(a snackbar) on a phone.

```rust
struct App { toasts: Toasts, notes: Vec<String> }
// in the view, once per frame (it adds an overlay rather than returning an element):
toasts(cx, &state.toasts, |s| &mut s.toasts);

// A toast that stays until it's dismissed:
state.toasts.push("Connected", ToastKind::Info);

// A toast that dismisses itself after 4 seconds, using a timer on the app's handle:
let handle = cx.handle();
button(cx, "Save").on_tap(cx, move |s| {
    let id = s.toasts.push("Saved", ToastKind::Success);
    s.toasts.dismiss_after(id, Duration::from_secs(4), &handle, |s| &mut s.toasts);
})

// A toast with an action: its button dismisses the toast and runs the closure.
let id = s.toasts.push("Deleted", ToastKind::Info);
s.toasts.dismiss_after(id, Duration::from_secs(6), &handle, |s| &mut s.toasts);
s.toasts.action(id, "Undo", move |s: &mut App| s.notes.push(note.clone()));

// Where they go, and room for a bottom navigation bar (56 px tall):
let toasts = Toasts::new()
    .placement(ToastPlacement::Auto)   // the default
    .bottom_offset(56.0);
```

`ToastKind` (`Info`, `Success`, `Warning`, `Error`) sets the color of the accent
bar. Every toast has a × that dismisses it, and can be swiped away: dragged
sideways with a finger or the mouse it follows along, fading, and let go past
about a third of its width it slides on out; short of that it springs back
(with the theme's toast motion). A swipe that starts on the action button
doesn't press it. Clicks outside the toasts reach the UI underneath, the strip
kept clear by the bottom offset included.

`ToastPlacement` picks the layout:

| Placement | Layout |
|---|---|
| `Auto` (default) | `Bottom` in a window narrower than 600 logical pixels (`NARROW_WINDOW`), else `Corner` |
| `Corner` | up to four stacked in the bottom trailing corner (bottom-left right to left), 320 wide, newest at the bottom |
| `Bottom` | a snackbar: the newest toast only, centered, the window's width less the theme's `lg` margin each side, at most 560 wide; older ones wait behind it, their timers running |

`bottom_offset` / `set_bottom_offset` raise either layout by that many
logical pixels, above a bottom navigation bar or tab bar. The window's
[safe area](layout.md#the-safe-area-phones) is kept clear of by itself.

`action` takes a closure on the state type the `toasts` view is built for
(`Fn(&mut S) + Send + Sync`); it runs once, after the toast is dismissed, so it
may push another toast. A toast keeps at most one action; giving it another
replaces it.

Each toast is a `Role::Alert` named by its message, with its action and ×
buttons inside. A toast appearing (not those already up in the first frame) is
announced:

| Platform | How |
|---|---|
| macOS | `AXAnnouncementRequested` on the window, high priority |
| iOS | `UIAccessibilityAnnouncementNotification` (the layout-changed notification as for any new element, not a screen change) |
| Android | a `TYPE_ANNOUNCEMENT` accessibility event from the toast's node, with its message |
| Web | the node gets ARIA role `alert`, which browsers' screen readers generally announce (the bridge reuses its elements, so this is not guaranteed) |
| Linux (AT-SPI) | the role is `notification`; no announcement event is raised yet |
| Windows (UIA) | a group; no notification event (`UiaRaiseNotificationEvent`) yet |

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
    .on_rename(|s: &mut App, i| s.start_renaming(i))
    .on_new(|s: &mut App| s.docs.push(Doc::new()));
tab_bar(cx, bar, &state.bar, |s| &mut s.bar)
```

- Clicking a tab selects it; its × or a middle click closes it, and a double
  click runs `on_rename` (the app asks for the name: `sessionsdemo` uses a
  message box with a text field).
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

## Navigation bar

The bar along the bottom of a phone app that switches between its top-level
destinations: an icon over a label each, the current one highlighted. The app
keeps which one is selected.

```rust
const SCAN: &str = include_str!("scan.svg"); // drawn in currentColor

let items = vec![
    NavItem::new("Scan", NavIcon::template(SCAN)),
    NavItem::new("History", &state.history_icon)      // an Arc<Svg> or Arc<Pixmap>
        .selected_icon(&state.history_filled)         // shown while selected
        .badge(Some(NavBadge::Count(state.unread))),  // or NavBadge::Dot
    NavItem::new("Create", NavIcon::template(CREATE)),
    NavItem::new("Settings", NavIcon::template(SETTINGS)),
];
let pages = page_stack(cx, 4, state.tab, |cx, i, visible| match i {
    0 => scan_page(cx, state, visible), // pause the camera when !visible
    1 => history_page(cx, state),
    2 => create_page(cx, state),
    _ => settings_page(cx, state),
});
let bar = navigation_bar(cx, items, state.tab, |s, i| s.tab = i);
column(vec![pages, bar]).grow(1.0)
```

- Tapping an item runs `on_select` with its index. Focused (Tab), ←/→ move
  the selection (mirrored right to left) and Home/End go to the ends.
- A `NavIcon::template` is SVG source using `currentColor`, tinted with the
  item's ink so one icon serves both states and follows the theme; an image
  (`Arc<Svg>`, `Arc<Pixmap>`, `ImageSource`) draws in its own colors.
- The look follows the theme (`NavLook::of`): Material 3's bar (80 px, a pill
  that grows in behind the selected icon, the label below) everywhere it
  draws itself, and `UITabBar`'s proportions (49 px over a hairline, no pill,
  the selected icon and small label in the accent) under `Theme::native()`
  on iOS. `NavigationBar::new(items, selected, on_select).look(NavLook::Ios)`
  picks one; `.view(cx)` builds it.
- In an app drawn [edge to edge](layout.md#the-safe-area-phones), the bar's
  background runs on under the home indicator (or Android's navigation
  bar) and to the screen's sides, its items kept inside the window's safe
  area.
- Assistive technologies see a `TabList` of `Tab`s, the selected one
  selected, each named by its label and badge ("History, 3 new").
- `navigation_rail` lays the same items down the side (Material 3's rail,
  80 px wide, ↑/↓ to move), for wide windows: pick by `cx.window_size()`.

**Keeping pages alive.** `page_stack` builds every page each frame, in
order, but only the selected one is on stage. The others are laid out where
it is and kept off stage (`Element::offstage`): not drawn, hit, focused or
read by assistive technologies. So their scroll containers keep their
offsets (and every scroll container, tap and field keeps the same id from
frame to frame), and a page left for another comes back as it was. Focus in
a page that goes off stage is dropped. Each page is told whether it shows.
Since every page is built every frame, keep pages cheap to build, or memoize
them with `cx.memo`.

## Password field

A single-line field that shows a bullet per character.

```rust
struct App { password: PasswordField }
let state = App { password: PasswordField::new().reveal_button(true) };
// in the view:
password_field(cx, &state.password, |s| &mut s.password)
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
- `reveal_button(true)` adds an eye button that shows the text while on.

## Color picker

`color_picker(cx, &state.picker, field, on_change)` is an inline
picker: a saturation/value area for the current hue, a hue strip, an alpha
strip, a hex field and a row of swatches. `color_button(...)` is a swatch
button that opens it in a popup below.

```rust
struct App { tint: ColorPicker, accent: Color }
let state = App { tint: ColorPicker::new(Color::rgb(0, 0x90, 0xff)), ... };
color_button(cx, &state.tint, |s| &mut s.tint,
    |s, c| s.accent = c)
```

The picker keeps the hue, saturation and value apart, so dragging to gray and
back keeps the hue. The hex field takes `#rgb`, `#rrggbb` and `#rrggbbaa`; the
color follows as soon as what's typed parses. `ColorPicker::alpha_strip(false)`
drops the alpha strip and `swatches` replaces the swatches.

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

<!-- live: themegallery -->

## Motion

The built-in widgets animate by default, under every theme, with the
timings and curves in the theme's motion tokens (`theme.motion`, a
`MotionTokens`):

| Widget | What moves | Token |
|---|---|---|
| `button` / `Button` | the hover and press looks fade in and out (the hover overlay over the button's color, twice over while pressed); a pressed button shrinks a little | `state`, `press`, `press_scale` |
| `switch` | the thumb slides over; the track's color fades | `toggle`, `state` |
| `checkbox`, `radio` | the check mark or dot pops in (and out) as it grows and fades; the box's color fades | `toggle`, `state` |
| `chip`, `choice_chips`, `filter_chips` | the check mark pops in as it grows and fades, its slot opening before the label (or the icon fading out under it); the fill, border and label color fade; hover and press as a button | `toggle`, `state`, `press`, `press_scale` |
| `tabs` | the accent fill slides (and stretches) from the old tab to the new one under the labels; the tabs' own fills and text colors fade | `indicator`, `state` |
| `tab_bar` | the accent strip slides out of the tab selected before and into the new one; the list of every tab grows in, and fades and shrinks back out when it closes | `indicator`, `popup`, `popup_exit` |
| `select` | the list grows and slides in from the button as it fades in; closing, it fades as it shrinks back toward the button | `popup`, `popup_scale`, `popup_offset`, `popup_exit` |
| Drawn menus (the in-window menu bar, submenus and context menus), `open_menu` | each menu grows in from its corner, and shrinks back and fades out as it closes | `popup`, `popup_exit` |
| `tooltip` | it fades in, and out | `popup`, `popup_exit` |
| `toasts` | a new toast slides in from the window's edge; a dismissed one slides back out and fades before it goes; the others glide to their new places | `toast_enter`, `toast_exit`, `toast_offset`, `toast_scale` |
| The drawn message box (`AppHandle::message`), `dialog` | it fades in as it scales into place, its scrim fading in with it; closing, it scales back as it fades out with its scrim | `dialog`, `dialog_scale`, `dialog_exit` |
| `tree` | a disclosure triangle turns as its node expands or collapses | `disclosure` |
| Drop targets (`on_drop`) | the app's "drop here" highlight fades in and out with the target's drag-over level | `DROP_FADE` |
| Controls the OS draws under `Theme::native()` | the OS's picture of the old state fades into the new one: hover, press and focus; a checkbox, radio or switch flipping | `state`, `toggle` |

What's already showing when a widget is first built doesn't animate in: an
app that starts with a select's list open, or with toasts up, shows them at
rest. Nothing animates at rest either, and every widget is laid out and drawn
exactly as before: the motion only shows while something changes, and an idle
app asks for no frames.

Each theme has its own motion. `Theme::light()`, `dark()` and their variants
use `MotionTokens::standard()`: quick ease-out color changes, a 0.97 press
scale, springy toggles, popups that grow from 96%. The Material 3 themes use
`MotionTokens::material()`. `Theme::native()` takes the platform's:

| Platform | Preset | Where the numbers come from |
|---|---|---|
| Windows | `fluent()` | WinUI's control animation durations (83, 167 and 250 ms) along its fast-out-slow-in spline (`0,0 0,1`); `ContentDialog` opens from 105% |
| macOS | `macos()` | `NSAnimationContext`'s default 0.25 s along Core Animation's default timing function; menus appear at once and fade out, as `NSMenu`s do |
| iOS | `ios()` | SwiftUI's `.snappy` and `.smooth` springs; alerts shrink into place from 115%, as `UIAlertController`'s do |
| Android | `material()` | the Material 3 motion tokens and Jetpack Compose's components: 120 ms menus from 80% scale, a 250 ms tab indicator, snackbars that fade in over 150 ms and out over 75 ms |
| Linux | `adwaita()` | Adwaita's `200ms $ease-out-quad` transitions; GTK 4's popovers appear and go at once |

None of them scale buttons but the standard one, as none of those platforms'
buttons do. Change the motion with `Theme::motion`, starting from a
preset:

```rust
// No shrink when pressed.
let theme = Theme::light().motion(MotionTokens::standard().press_scale(1.0));
```

`MotionTokens::instant()` turns the widgets' motion off. The OS's
reduced-motion setting does too, for every theme (see
[animation.md](animation.md#reduced-motion)): the tokens are `anim::Motion`s,
so tweens shorten to nothing and springs jump.

A button's hover and press looks are worked out from its variant's color when
it's built; restyle one with `.fill(…)` and give it its own `.hovered` and
`.pressed` too. While a button is shrunk under the pointer, a release right on
its edge can land just outside it, as with CSS's `:active` scale. Set
`press_scale` to 1 if that matters.

The stateful widgets remember when something opened in their state; the
stateless `open_menu`, `dialog`, `tooltip` and the drawn message box come
in through the core's enter animations (`Element::enter`, see
[animation.md](animation.md#enter-exit-and-layout-animations)). Everything
that closes goes out through the core's exit animations (`Element::exit` on
the overlay's content): when the view stops declaring the overlay, the app
keeps drawing it where it was while it leaves, and meanwhile it takes no
clicks, focus or hover and screen readers don't see it, so a click where a
closing list still shows reaches what's under it. A modal's scrim fades in
and out with its dialog, on the dialog's own motion (and stops blocking at
once when it closes). Each message box is its own overlay, so the next one
waiting comes in as the last one leaves. Reopened while it's leaving, a
popup turns round from where it is. Switching between menu bar menus, or
between submenus, swaps them without an exit, as native menus do.

The segmented `tabs` slide their indicator through the core's layout
animations: the accent fill is one keyed element (`Element::animate_layout`)
in a layer between the tabs' own fills and their labels (`Element::layers`),
so it moves from wherever the old tab was. It's keyed by the strip's first
tap, which is unique in the window; if the view changes how many handlers
come before the strip, that change snaps instead of sliding. The indicator
is measured from the window (or the enclosing scroll content), so it also
glides when something above moves the whole strip in the same frame.

Controls the OS draws under `Theme::native()` (see
[native-theme.md](native-theme.md)) are still pictures, one per state, so
they cross-fade: for a moment after a change the old state's picture is drawn
over the new one, fading out, on the theme's `state` motion (hover, press,
focus) or `toggle` motion (a checkbox, radio or switch flipping). A slider's
and a progress bar's value, and a switch between light and dark, change at
once (the first follows the pointer; the second cross-fades the whole
window). A control is followed by its tap or focus handle. Seen only through
unit tests: CI's Windows and macOS runs don't hover native controls.

## Accessibility

| Widget | Role | Name |
|---|---|---|
| Select | `ComboBox`; its open list is a `List` of `ListItem`s | the chosen option |
| Number input | `SpinButton`, with `Increase` and `Decrease` buttons | the text |
| Split pane divider | `Splitter` | — |
| Toolbar | `Toolbar`, with buttons and `Separator`s | — |
| Choice chips | `RadioButton`s, the selected one checked | the chip's label |
| Filter chips, a chip alone | `CheckBox`es, checked while on | the chip's label |
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
`tablist`/`tab`. Radio buttons map everywhere: UI Automation's `RadioButton`,
`AXRadioButton`, AT-SPI's `radio button`, ARIA's `radio`, Android's
`RadioButton`, and on iOS a button that is selected while checked.
