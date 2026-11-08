# Layout

Stipple lays out an element tree in two passes: `measure` works out each
element's natural size bottom-up, then `layout` hands each child its rectangle
top-down. There are two kinds of container, stacks and grids, and every element
can carry size constraints. The `layoutdemo` example shows all of it:

```sh
cargo run -p layoutdemo
```

## Stacks

`row(children)` lays children left to right, `column(children)` top to bottom.

```rust
row(vec![icon, label(&theme, "Title"), spacer(), close_button])
    .gap(8.0)                          // space between children
    .padding(Insets::uniform(12.0))    // space inside the edge
    .align(Align::Start, Align::Center) // (main axis, cross axis)
```

- A child's `.grow(weight)` gives it a share of the leftover main-axis space.
  `spacer()` is an empty element with `grow(1.0)`.
- A child's `.shrink(weight)` lets it give up space when its siblings don't fit,
  in proportion to `weight × its natural size`. Children don't shrink by
  default; without a shrinking child, an overflowing stack overflows.
- Main-axis alignment (`Start`, `Center`, `End`) places the children as a block
  when nothing grows. Cross-axis `Stretch` fills the stack's cross extent.

## Wrapping

`.wrap()` on a stack flows children that don't fit the main axis onto new lines:

```rust
row(tags).wrap().gap(6.0).align(Align::Start, Align::Center)
```

The gap applies between items and between lines. Each line is laid out like a
stack of its own: grow shares that line's leftover space, main-axis alignment
places that line, and cross-axis alignment works within the line's height (its
tallest child). Lines pack from the top. An item wider than the stack gets a
line to itself. (On a text element, `.wrap()` word-wraps the text instead.)

## Grids

`grid(columns, children)` places children into cells in order, row by row.

```rust
grid(
    vec![Track::Auto, Track::fr(1.0)],
    vec![
        heading(&theme, "Account").span(2, 1),
        label(&theme, "Name"), name_field,
        label(&theme, "Email"), email_field,
    ],
)
.gap(8.0)                              // .column_gap() / .row_gap() set one
.rows(vec![Track::px(40.0)])           // optional; unlisted rows are Auto
.align(Align::Stretch, Align::Center) // (horizontal, vertical) within each cell
```

Tracks come in three kinds:

| Track | Size |
|---|---|
| `Track::px(n)` | `n` logical pixels |
| `Track::Auto` | The largest child in the track |
| `Track::fr(w)` | A share, by weight, of the space left after the other tracks and gaps |

`fr` columns fill the width the grid is given, so a grid is as wide as its
container. `fr` rows fill the grid's height once it is laid out; while it is
measured they size to their content, keeping their weights proportional. An
`fr` track can shrink to nothing when the other tracks use up the space.

`.span(columns, rows)` makes a child cover several cells. Placement skips cells
that are already taken, and doesn't go back to fill earlier holes. A child
spanning `Auto` tracks that are too small for it grows them equally.

Cells stretch their child by default. A child with a fixed `.width()` or
`.height()` keeps it and sits at the start of the cell on that axis.

Children are measured at their column's width, so wrapped text in an `fr`
column wraps there and its row is tall enough.

## Size constraints

Any element can take a fixed size and bounds:

```rust
.width(240.0) .height(32.0)
.min_width(120.0) .max_width(480.0)
.min_height(24.0) .max_height(200.0)
```

Bounds apply wherever the size is decided: to the natural size, to growing (a
growing child stops at its maximum and the rest goes to its siblings), to
shrinking (it stops at its minimum), to cross-axis stretching, and to grid
cells. Content measures against the bounded space, so a paragraph with
`.max_width(260.0)` wraps at 260 even in a wider container. When a minimum is
larger than the maximum, the minimum wins, as in CSS.

## Right-to-left layouts

`Theme::with_layout_direction(LayoutDirection::Rtl)`, or
`.layout_direction(..)` on an element and its subtree, mirrors everything above
horizontally: rows run right to left, `Align::Start` on a horizontal axis is
the right edge, grid columns count from the right, and the `left` padding inset
lands on the right. Vertical layout doesn't change, and neither does the
source order of children (focus order follows it). See
[bidi.md](bidi.md#mirrored-layouts).

## Input, focus and accessibility

Every container produces ordinary `LayoutNode` children in source order, so
hit testing, keyboard focus order, scrolling and the accessibility tree work
the same inside grids and wrapped stacks as anywhere else. Focus moves in the
order the children were written, which for a grid is row by row.
