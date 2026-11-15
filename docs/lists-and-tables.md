# Lists, tables and trees

`virtual_list`, `table` and `tree` show large data sets by building only the
rows on screen. A 10,000-row table builds about twenty rows a frame; the scroll
extent still covers all 10,000. The `tabledemo` example has a 10,000-row table
and a tree:

```sh
cargo run --example tabledemo               # --theme dark | contrast | native
```

<!-- live: tabledemo -->

## Virtualized lists

```rust
use stipple::prelude::*;

struct App { list: ListState }

fn view(state: &App, cx: &mut Cx<App>) -> Element {
    virtual_list(
        cx,
        &state.list,
        |s| &mut s.list,              // where the ListState lives
        VirtualList::fixed(10_000, 24.0).height(320.0),
        |cx, row| label(cx, format!("Row {}", row.index)),
    )
}
```

- **Rows**: `VirtualList::fixed(count, height)` for one row height, or
  `VirtualList::variable(&heights)` for one height per row. `.overscan(n)` sets
  how many rows are built beyond each edge of the viewport (default 2).
- **The row builder** gets the `Cx` (a row can hold its own buttons or
  checkboxes) and a `RowInfo` with the row's `index`, and whether it is
  `selected` and holds the keyboard `cursor`. The list adds the row's height,
  the selection tint and the click handling.
- **How it works**: the list registers a scroll container and reads its offset
  with `Cx::scroll_offset`. Its content is a spacer for the rows above the
  viewport, the visible rows, and a spacer for the rows below. A wheel scroll
  rebuilds the frame, which builds the rows that scrolled in.

### Selection and keys

`ListState::new(SelectionMode::None | Single | Multi)` holds the selection and
the keyboard cursor, by row index.

| Input | Single | Multi |
|---|---|---|
| Click | Select the row | Select only the row |
| Shift+click | Select the row | Select the range from the anchor |
| Ctrl/Cmd+click | Select the row | Toggle the row |
| Up / Down, Home / End | Move the selection | Move the selection |
| Shift+arrows, Shift+Home/End | Move the selection | Extend the range |
| Ctrl/Cmd+A | — | Select all |
| Space | Select the cursor row | Toggle the cursor row |
| Escape | — | Clear the selection |

Moving the cursor scrolls it into view. In multi-selection the cursor row has a
focus-colored outline, since it can sit on an unselected row. Read the selection
with `selected()`, `is_selected(i)` and `cursor()`; change it with `select`,
`toggle`, `extend_to`, `select_all` and `clear`. After the data shrinks, call
`truncate(count)`.

## Swipe actions and pull to refresh

Phone apps act on a list's rows by swiping them, and check for new items by
pulling the list down. The `listgestures` example is an inbox with both:

```sh
cargo run --example listgestures              # --selftest drives it off-screen
```

<!-- live: listgestures -->

### Swipe actions

`swipe_row` puts actions behind a row. Its state is a `SwipeState` in the app
state, one per list (one row is open at a time), and each row has a key that
stays with its item (an id, not the index):

```rust
use stipple::prelude::*;

struct App { mail: Vec<Mail>, list: ListState, swipe: SwipeState }

// In the row builder of a virtual_list (or for each row of a `scroll`):
|cx, row| {
    let id = state.mail[row.index].id;
    let actions = SwipeActions::new()
        .trailing(SwipeAction::new("Delete", move |s: &mut App| s.delete(id)).destructive())
        .trailing(SwipeAction::new("Archive", move |s: &mut App| s.archive(id)))
        .leading(SwipeAction::new("Unread", move |s: &mut App| s.toggle_read(id)).icon(icon));
    swipe_row(cx, &state.swipe, |s| &mut s.swipe, id, actions)
        .child(label(cx, &state.mail[row.index].subject))
}
```

- **Leading and trailing.** Dragging the row toward the trailing edge (right,
  left to right) shows the leading actions; toward the leading edge, the
  trailing ones. Right-to-left layouts mirror it. Each side's first action is
  its primary one, at the outer edge. A side with no actions doesn't move.
- **The drag.** The row follows the finger (or the mouse), the buttons
  widening behind it with their colors, labels and icons. Let go past half
  the buttons (or flicked that way) and it stays open on them; a tap on one
  runs it, and a tap on the row, or pressing another row, shuts it. Short of
  that, it springs back. Past 60% of the row's width the primary action
  fills the side, and letting go there runs it (a *full swipe*;
  `.full_swipe(false)` turns it off, and the row then resists past its
  buttons). Settling uses the theme's `swipe` motion.
- **Destructive actions** (`.destructive()`) are red by default, and when one
  runs, its row is shut at once: the app removes the item, the row slides
  out and fades (its exit), and the rows below glide up into the gap. A
  side's first action is the theme's primary color by default, the others
  gray; `.color(c)` sets one.
- **Scrolling still works.** A drag only swipes once it is mostly sideways,
  decided where it leaves the 8-pixel touch slop, on the same move as the
  touch recognizer: a vertical drag scrolls the list, and a row inside a
  sideways strip scrolls the strip. The swipe cancels the press, so the row
  isn't tapped or selected.
- **Mouse and trackpad.** A sideways mouse drag swipes as a finger does. A
  two-finger sideways trackpad scroll moves the row and settles it when the
  fingers lift (without a full swipe); a tilt wheel or Shift+wheel notch
  opens and shuts it a step at a time. Vertical scrolls still go to the
  list.
- **Keyboard and assistive technologies.** Every action is also in the row's
  context menu: a right-click, a long press, or the Menu key or Shift+F10
  with the pointer over it. With a `virtual_list` focused, the Menu key opens
  the cursor row's actions; in a plain list, the actions of the row around
  the focused control. Screen reader users reach them the same way.

In a `virtual_list` the row's padding and selection tint go on the sliding
content, so the actions behind span the whole row. In a plain `scroll` the
row fills the width its column gives it.

### Pull to refresh

`pull_to_refresh` wraps a `scroll` or a `virtual_list`, with a
`RefreshState` in the app state and the callback that starts a refresh:

```rust
struct App { items: Vec<Item>, refresh: RefreshState, /* … */ }

pull_to_refresh(cx, &state.refresh, |s| &mut s.refresh, move |s| {
    // Start loading; the indicator spins until `finish`.
    handle.spawn(fetch(), |s, items| { s.items = items; s.refresh.finish(); });
})
.child(scroll(cx).children(rows))
```

- On a touch screen, a finger dragging the list *down* while it is scrolled
  to its top pulls the indicator out after it, following less and less the
  further it goes. Let go past `REFRESH_THRESHOLD` (64 px) and the state
  starts refreshing and the callback runs; the indicator stays, spinning,
  until the app calls `refresh.finish()`, then goes back up. Short of the
  threshold it goes back up at once. The touch recognizer hands the finger
  over as it does for a bottom sheet, so a drag anywhere else (or a list
  scrolled down) scrolls as usual, and the row the finger landed on isn't
  tapped.
- **The look** follows the platform: under the iOS look (`Theme::native()` on
  iOS) the list slides down and UIKit's spokes turn in the gap above it,
  appearing one by one as it's pulled; elsewhere a Material disc drops from
  the top over the list, its arc growing with the pull and turning while it
  refreshes.
- `refresh.begin()` shows the indicator spinning without a pull (a refresh
  the app started itself); `is_refreshing()` says whether one is under way;
  the callback doesn't run again while it is.
- **Mouse**: there is no pull. A desktop app puts a Refresh button or menu
  item in its window (the example has one in its toolbar).
- **Keyboard and assistive technologies**: "Refresh" is in the context menu
  of the list and of the rows inside it, after the rows' own actions, and
  runs the callback the same way. `RefreshState::new().label("Actualiser")`
  names it in the user's language. While it refreshes, screen readers see
  the indicator as "Refresh…".

The tests drive both at fixed frame times with simulated fingers
(`src/widgets/swipe_tests.rs`, `src/widgets/refresh_tests.rs`), and
`listgestures --selftest` drives the example the same way.

## Tables

```rust
let columns = [
    Column::new("Name", 180.0).sortable(),
    Column::new("Size", 100.0).sortable().align(Align::End),
];
table(
    cx,
    &state.table,                         // a TableState
    |s| &mut s.table,
    TableSpec::new(&columns, state.order.len())
        .row_height(26.0)
        .height(400.0)
        .on_sort(|s: &mut App, column, order| s.sort(column, order)),
    |cx, row, column| label(cx, cell_text(state, row, column)),
)
```

- **Sorting**: clicking a sortable header sorts by it, and clicking again
  reverses it. The table moves its ▲/▼ indicator, clears the selection, scrolls
  to the top and calls the spec's `on_sort`. You reorder the data: the `row` passed to
  the cell builder is the display position, so keep an order (`Vec<usize>`) and
  map through it.
- **Resizing**: drag a header's right edge. Widths live in the `TableState`
  (`column_width`, `set_column_width`) and stop at `Column::min_width`.
- **Rows**: `TableState::rows` is a `ListState`, so selection works as above.
  Rows are striped unless `.striped(false)`.
- Columns have fixed widths, so a table wider than its container is clipped;
  there is no horizontal scrolling yet.

## Trees

The data comes from a `TreeModel`: `children(None)` lists the roots, and
`children(Some(id))` lists a node's children. Nodes are `u64` ids that you
choose and that stay the same across frames. Override `has_children` if
listing the children is expensive.

```rust
tree(
    cx,
    &state.tree,                            // a TreeState
    |s| &mut s.tree,
    TreeSpec::new(&state.folders)           // a TreeModel
        .row_height(26.0)
        .height(400.0),
    |cx, row| label(cx, name_of(row.id)),
)
```

Each frame the tree walks its expanded nodes (ids only) into a flat list of
visible rows, and builds the rows in the viewport. Each row is indented by its
depth, and a node with children shows a disclosure triangle that expands or
collapses it.

| Key | Effect |
|---|---|
| Up / Down, Home / End | Move the selection |
| Right | Expand; if already expanded, go to the first child |
| Left | Collapse; if already collapsed, go to the parent |
| Enter / Space | Toggle the selected node |

Collapsing a node that contains the selection moves the selection to that node.
`TreeState` has `is_expanded`, `set_expanded`, `selected` and `select`.

## Accessibility

Each widget reports the right roles: the list is a `List` of `ListItem`s, the
table a `Table` of `Row`s holding `ColumnHeader`s and `Cell`s, and the tree a
`Tree` of `TreeItem`s. Items report whether they are selected, and tree items
whether they are expanded. Your own widgets can declare semantics with
`Element::role`, `.selected(bool)` and `.expanded(bool)`. UI Automation and
NSAccessibility get the matching control types. Selected and expanded states
don't reach the platform bridges yet.

Each list, table or tree takes keyboard focus as one control and draws one
focus ring. Tab moves past it, not through its rows.

## Pointer modifiers and drags

Tap handlers only receive the app state. A handler that needs more reads
`stipple::core::pointer()`, which returns the pointer position, where the
current drag started, and the modifier keys held at the last button press.
The X11, Wayland, Windows and macOS backends record those modifiers. Tests
simulate them with `stipple::platform::set_pointer_modifiers`.
