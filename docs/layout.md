# Layout

Stipple lays out an element tree in two passes: `measure` works out each
element's natural size bottom-up, then `layout` hands each child its rectangle
top-down. There are two kinds of container, stacks and grids, and every element
can carry size constraints. The `layoutdemo` example shows all of it:

```sh
cargo run --example layoutdemo
```

<!-- live: layoutdemo -->

## Stacks

`row(children)` lays children left to right, `column(children)` top to bottom.

```rust
row(vec![icon, label(cx, "Title"), spacer(), close_button])
    .gap(8.0)                          // space between children
    .padding(Insets::uniform(12.0))    // space inside the edge
    .align(Align::Start, Align::End)   // (main axis, cross axis)
```

By default, as in CSS flexbox, a row centers its children across (a checkbox,
a label and a button line up on their middles) and a column stretches them
to its width (a row inside it spans the column, so a `spacer()` in the row
pushes). A child with a fixed size across keeps it, and `.align_self(align)`
on a child overrides the stack's cross-axis alignment for that child alone:

```rust
column(vec![
    text_field(cx, &state.note, |s| &mut s.note), // full width
    button(cx, "Save").align_self(Align::End),               // its own width, at the right
])
```

A text field's default width (200) is only a default: a column stretches it
like any other unsized child, while `.width(w)` fixes it.

- A child's `.grow(weight)` gives it a share of the leftover main-axis space.
  `spacer()` is an empty element with `grow(1.0)`.
- A child's `.shrink(weight)` lets it give up space when its siblings don't fit,
  in proportion to `weight × its natural size`. Children don't shrink by
  default; without a shrinking child, an overflowing stack overflows.
- Main-axis alignment (`Start`, `Center`, `End`) places the children as a block
  when nothing grows. Cross-axis `Stretch` fills the stack's cross extent,
  except for a child sized on that axis.

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
        heading(cx, "Account").span(2, 1),
        label(cx, "Name"), name_field,
        label(cx, "Email"), email_field,
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

As in CSS grid, a child can align itself in its cell over the grid's
`.align(..)`: `.justify_self(align)` horizontally and `.align_self(align)`
vertically. (`Stretch` there still leaves a fixed size alone.)

```rust
grid(vec![Track::Auto, Track::fr(1.0)], vec![
    label(cx, "Total"),
    label(cx, "42.00").justify_self(Align::End), // at the right of its stretched column
])
```

Children are measured at their column's width, so wrapped text in an `fr`
column wraps there and its row is tall enough.

## Layers

`Element::layers(children)` stacks its children over one another in the
same box, each later one drawn over (and hit before) the ones before it: a
fill behind content, a badge over an icon. Each child fills the box unless
it's sized, then sits per `.align(horizontal, vertical)`; the box is as big as
its biggest child. A child's `.align_self()` and `.justify_self()` have no
effect here: the layers' `.align()` places every child (wrap one in a stack or
a one-cell grid to place it on its own).

```rust
let card = Element::layers(vec![
    Element::boxed(BoxStyle::filled(theme.palette.primary)),
    label(cx, "Over the fill").padding(Insets::uniform(8.0)),
]);
```

The segmented `tabs` widget uses it to slide its selection indicator under
its labels.

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

### Aspect ratio

`.aspect_ratio(w / h)` keeps an element's width over its height, as CSS's
`aspect-ratio` does: "as wide as there is, and square" (a QR code, a camera's
scan window), 16:9 for a video, 4:3 thumbnails, with no window size in the
view, so it holds inside a dialog or one pane of a split view too.

```rust
column(vec![
    Element::canvas(CanvasId(1), draw_code).aspect_ratio(1.0), // the column's width, square
    grid(vec![Track::fr(1.0); 3], thumbnails),                 // each .aspect_ratio(4.0 / 3.0)
])
```

The height follows the width once that is decided: a fixed `.width(..)`, or
the width it is given (a column's, a grid cell's, or, in a row, a share of
the free width, as if it grew; rows of squares share the row evenly, and the
row is as tall as they are). With only `.height(..)` fixed, the width follows
the height instead; with both fixed the ratio does nothing. Bounds still apply
to each side, so `.aspect_ratio(16.0 / 9.0).max_width(320.0)` is at most
320 × 180.

## Scroll containers

A `scroll(cx)` list or a `scroll_horizontal(cx)` strip lays its content out
at its natural length and clips it to the viewport. The viewport's length
along the scroll direction is `.height(h)` (`.width(w)` sideways) when one
is set; otherwise it follows the parent:

- **In a stack along the same axis** (a list in a column, a strip in a row)
  it grows into the free space, as with `.grow(1.0)`, up to its
  `max_height`, and when its siblings don't fit, it shrinks first (down to
  100 pixels, or its content's length if less; `min_height` sets another
  floor). A list under a toolbar takes the rest of the window.
- **Anywhere else** (an auto-sized dialog or popover, a row holding a
  vertical list) it is as long as its content, but no longer than the space
  its parent was given: a short list shows whole, a long one scrolls in the
  room there is. It never collapses to nothing for lack of a size, and never
  grows past its parent for a long list.

While its content overflows, a scroll container shows a scroll bar. On
Linux, Windows and the web it keeps a gutter for it: the content is laid
out that much narrower (shorter, sideways), and a container sized by its
content grows by that much instead. See
[Scroll bars](widgets.md#scroll-bars).

## The safe area (phones)

On a phone the system draws over parts of the screen: the status bar, a
notch or Dynamic Island, the home indicator or Android's navigation bar,
rounded corners. By default a Stipple app gets the rest, the safe area, and
the theme's background fills around it, so nothing else needs doing.

A camera's preview, a photo or a map should run under the bars instead,
with only the controls kept clear of them. `App::edge_to_edge(true)` gives
the app the whole screen (on iOS and Android; desktops and the web always
give it the whole window), and `cx.safe_area()` says how far in from each
edge the system's things reach, in logical pixels, as `Insets`. A view that
reads it is built again when it changes: the phone turned, the bars shown or
hidden, iPad split view. `safe_area(cx)` is a column padded by it:

```rust,no_run
use stipple::prelude::*;
# struct State;
fn view(_: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    Element::layers(vec![
        // Under the bars: the whole screen.
        Element::boxed(BoxStyle::filled(theme.palette.primary)),
        // The controls: inside the safe area.
        safe_area(cx).children(vec![heading(cx, "Viewfinder")]),
    ])
    .grow(1.0)
}
# fn main() {
App::new(State, view).edge_to_edge(true).run();
# }
```

`navigation_bar` reads it too: its background runs on under the home
indicator, its items above it; `toasts` stay clear of it. Edge to edge, the
status bar's icons turn dark over a light top edge and light over a dark one
(from the color the frame has in its top-left corner; Android's navigation
bar from the bottom-left one). The on-screen keyboard isn't in the safe
area: while it is up (iOS, Android, a phone's browser) the App lays the view
out above it, the safe area's bottom shrinks by it, and `cx.keyboard_inset()`
says how tall it is (see
[text-editing.md](text-editing.md#the-on-screen-keyboard)). In a browser,
a page whose viewport meta tag has `viewport-fit=cover` reports a phone's
`env(safe-area-inset-*)`, as far as they reach over the canvas.

It is also in `WindowInfo::safe_area`, with the window's orientation
(`WindowInfo::orientation()`, landscape when wider than tall); see
[windows-and-cursors.md](windows-and-cursors.md#phones-orientation-and-keeping-awake).

## Right-to-left layouts

`Theme::layout_direction(LayoutDirection::Rtl)`, or
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
