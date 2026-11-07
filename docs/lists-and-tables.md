# Lists, tables and trees

`virtual_list`, `table` and `tree` show large data sets by building only the
rows on screen. A 10,000-row table builds about twenty rows a frame; the scroll
extent still covers all 10,000. The `tabledemo` example has a 10,000-row table
and a tree:

```sh
cargo run -p tabledemo               # --theme dark | contrast | native
```

## Virtualized lists

```rust
use stipple::prelude::*;

struct App { list: ListState }

fn view(state: &App, cx: &mut Cx<App>) -> Element {
    let theme = *cx.theme();
    virtual_list(
        cx,
        &theme,
        &state.list,
        |s: &mut App| &mut s.list,              // where the ListState lives
        VirtualList::fixed(10_000, 24.0).height(320.0),
        |_cx, row| label(&theme, format!("Row {}", row.index)),
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

## Tables

```rust
let columns = [
    Column::new("Name", 180.0).sortable(),
    Column::new("Size", 100.0).sortable().align(Align::End),
];
table(
    cx,
    &theme,
    &state.table,                         // a TableState
    |s: &mut App| &mut s.table,
    TableSpec::new(&columns, state.order.len()).row_height(26.0).height(400.0),
    |s: &mut App, column, order| s.sort(column, order),
    |_cx, row, column| label(&theme, cell_text(state, row, column)),
)
```

- **Sorting**: clicking a sortable header sorts by it, and clicking again
  reverses it. The table moves its ▲/▼ indicator, clears the selection, scrolls
  to the top and calls your `on_sort`. You reorder the data: the `row` passed to
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
    &theme,
    &state.tree,                            // a TreeState
    |s: &mut App| &mut s.tree,
    &state.folders,                         // impl TreeModel
    TreeSpec::default().row_height(26.0).height(400.0),
    |_cx, row| label(&theme, name_of(row.id)),
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
