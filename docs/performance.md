# Performance: what a frame costs, and how to keep it small

A Stipple view is a function of the state: each time something changes, the
app builds the view again, lowers it (interaction states, transitions), lays
it out, paints it into a scene, works out what changed since the frame on
screen, and rasterizes only that. This page is about keeping those steps cheap
for large, animated interfaces: memoized subtrees and derived values in the
view, animation frames that skip the view altogether, and painting only what
shows.

## Memoized subtrees: `Cx::memo`

`cx.memo(key, deps, |cx| element)` keeps the element the closure built under
`key`, with a hash of `deps`. While `deps` hash the same, later builds return
the kept element without calling the closure:

```rust
fn view(s: &Inbox, cx: &mut Cx<Inbox>) -> Element {
    let theme = *cx.theme();
    let rows = s.messages.iter().enumerate().map(|(i, m)| {
        // Built again only when this message changes.
        cx.memo(("message", m.id), (m.revision, i == s.selected), |cx| {
            message_row(&theme, cx, m, i)
        })
    });
    column(rows.collect())
}
```

- **Handlers come along.** The closure gets the `Cx`, so the subtree can have
  tap, key, drag, drop, pointer and context handlers, focus and scroll
  containers. When it is reused, the handlers it registered last time are
  moved over from the previous frame's table under the same ids, so clicks on
  a reused row reach the handler built with it. They are the ones built with
  those `deps`, so give `deps` everything a handler captures that can change.
- **Ids stay right.** Handler ids are handed out in registration order, so a
  kept subtree is reused only where its ids come out the same: when everything
  built before it registered as many handlers of each kind (and scroll
  containers) as last time. If a button appears above it, it is built again
  once, then kept at its new place. An id a handler captured (a `ScrollId` to
  scroll from a button, a `FocusId`) is therefore never stale.
- **What it reads through `Cx` is an input.** The interaction states
  (`cx.interaction`), drop-target states (`cx.drop_state`), scroll offsets
  (`cx.scroll_offset`, which `virtual_list` reads) and window info
  (`cx.window_info`) it read are recorded and compared, as are the theme, the
  window's size and its focus. A reused subtree still counts as having read
  them: the app rebuilds when they change, and the subtree is built again.
- **Some subtrees are never kept**: one that asks for frames
  (`request_frame`, `animate`, `play`), declares overlays or context menus
  (`Element::context_menu`), or sets the window title. `frame_time` isn't
  tracked, so sample animations outside a memoized closure.
- **Layout comes along too.** A reused subtree that lowers to the same
  elements as last time (no state override or transition inside it changed
  anything) isn't measured or laid out again: its last layout is moved to
  where it now goes.
- Keys are per window and should be unique within a build (a second use of a
  key in the same build is built afresh, not kept). Nested memos are kept with
  the subtree around them. Whatever a build didn't use is dropped.

`deps` are hashed (with the standard library's SipHash), not compared, so
anything `Hash` works, `&str` and slices of the state included, without
cloning.

## Derived values: `Cx::derive`

`cx.derive(key, deps, || value)` caches a value the same way: computed again
only when `deps` change, cloned out otherwise (wrap a large one in an `Rc`).
For something costly derived from the state that the view needs on every
build: a sorted, filtered list, totals, a parsed document.

```rust
let visible = cx.derive("visible", (&s.items, &s.filter), || {
    Rc::new(s.items.iter().filter(|i| i.matches(&s.filter)).cloned().collect::<Vec<_>>())
});
```

## Animation frames without a build

Implicit [transitions](animation.md#transitions) of looks that don't
move the layout — fill, border, corner radius, text color, opacity and
transform — and the hover and press highlights fading don't need the view: when
the only thing that changed since the frame on screen is time, the app takes
the laid-out tree on screen, writes the in-flight values on the moving
elements, and paints. The view isn't built and nothing is laid out. The damage
is the moving elements' old and new extents (a recolored element: its own box;
a moved or faded one: everything it draws, where it was and where it is), so
only that is rasterized and presented.

Frames fall back to a full build when the view asked for them itself
(`Cx::request_frame` / `Cx::animate`: it samples the frame time, so only it
knows what changes), when padding or a fixed size is animating, during enter,
exit and layout animations, OS-drawn controls cross-fading between states, a
touch fling or a drop target's drag-over fade,
and when something moving changes what the pointer is over for an element
whose look (or the view) depends on that.

So prefer a transition over sampling an `Animated` in the view when the view
doesn't need the value: `.transform(t).transition(...)` animates without
builds, `cx.animate(&anim)` builds every frame.

## Painting only what shows

The scene for a frame leaves out what is drawn entirely outside the window or
outside a clip (a scroll container, a clipped panel): the rows of a long list
scrolled out of view aren't shaped or put in the scene, and raster work
follows what is visible. (`core::paint_visible` does it for a custom
pipeline; it assumes nothing draws more than `core::CULL_MARGIN`, 16 logical
pixels, past its box.) For lists of thousands of rows, also build only what is
visible: see [`virtual_list`](lists-and-tables.md).

## What it buys

`bench_a_large_list_with_one_moving_element` in `src/reactivity_tests.rs`
(`cargo test --release bench_ -- --nocapture`) times a frame of 500 rows in a
scroll container with one moving element, 800×600, on the software renderer.
On a Linux desktop (a busy one; medians over 60 frames):

| Frame | Build and damage | Raster and present | Damage |
|---|---|---|---|
| Before: transition moving one tile | 3.3 ms | 1.1 ms | 1345 px² |
| Now: the same, drawn without a build | 0.12 ms | 0.04 ms | 1344 px² |
| Before: the view turns a spinner itself | 3.6 ms | 1.4 ms | ~1600 px² |
| Now: the same | 0.88 ms | 0.05 ms | ~1250 px² |
| Now: the same, rows memoized | 0.73 ms | 0.05 ms | ~1600 px² |

The damage was already limited to what moved; what was spent was everything
around it (shaping and painting all 500 rows into the scene, and rasterizing
that scene for a 40×30 region).

What a built frame still costs grows with the tree: building or cloning it,
lowering it, comparing it with the last one and keeping a copy are walks over
every element. A memoized subtree saves building and laying out, not those
walks; a view with very cheap rows gains little from it, one with costly rows
(formatting, data work) much more.
