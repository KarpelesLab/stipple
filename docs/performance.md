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
    let rows = s.messages.iter().enumerate().map(|(i, m)| {
        // Built again only when this message changes.
        cx.memo(("message", m.id), (m.revision, i == s.selected), |cx| {
            message_row(cx, m, i)
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

## Benchmarks

`examples/bench` times what a user feels, on the software renderer and on
the GPU one, and CI runs it on every push (the Perf workflow, see
[CI](ci.md#performance)). Each scenario runs a fixed number of iterations
after a warm-up, with fixed sizes and a fixed seed, and reports the median
and the 95th percentile:

| Scenario | What is timed |
|---|---|
| `first_frame_1x`, `first_frame_2x` | The whole first frame of a window of widgets (a toolbar, a text field, 32 rows of checkboxes, switches, sliders, buttons and progress bars, a 10,000-row `virtual_list`), 1024×768 at scale 1 and 2 |
| `hover` | The pointer moving onto another button, to the frame ready; and the pixels rasterized (only the damage is) |
| `scroll_list` | A wheel step (48 px) over the 10,000-row list, to the frame ready |
| `type_char` | A character typed into the focused text field, to the frame ready |
| `theme_switch` | A click on a button switching light and dark, to the frame ready |
| `sprites_2000` | 2000 32×32 sprites over a 1920×1080 background (the `spritedemo --bench` scene) |
| `layout_tree` | Laying out a tree of ~4,000 elements |
| `text_layout`, `text_layout_cold`, `text_layout_cjk` | Laying out a 400-word paragraph wrapped at 560 px: with shaping cached, with a font just loaded, and 1,200 Japanese characters through the system's CJK fallback (when there is one) |
| `startup` | Loading the system font, making the app and drawing its first frame |
| `calibration` | A fixed CPU loop: how fast the machine was during the run |

"To the frame ready" is the event and the frame that answers it, presented
as a window's frame is ([`App::present_frame`](https://docs.rs/stipple/latest/stipple/struct.App.html#method.present_frame)):
built, laid out, painted, and rasterized (only the damage, on the CPU) or
drawn on the GPU and read back. Layout and text don't draw, so they run
with the software renderer only.

```sh
cargo run --release --example bench                                   # the software renderer
cargo run --release --example bench --features mtl -- --renderer metal  # gl (--features gl), d3d11 (--features d3d)
cargo run --release --example bench -- --only hover,type_char --json out.json
```

`--quick` runs a third of the iterations, `--list` names the scenarios.
Asking for a GPU renderer the machine doesn't have measures the software
one instead, and the report says so (`gpu_unavailable`). The JSON holds
each scenario's median, p95, minimum and samples, and the run's
`calibration_ms`.

**In the browser**, the same scenarios run in the wasm module on the
software renderer (its GPU renderers draw into the page's canvas only), and
the page also runs the app live and times events dispatched to the canvas
(a hover, a wheel step, a key, a click on "Theme") to the next frame
presented after them: `web_hover`, `web_scroll_list`, `web_type_char`,
`web_theme_switch`, as `median_ms` (which includes waiting for the display's
next frame) and `work_ms` (the time in the event handler and the frame
callback), on the software renderer, WebGL2 and WebGPU:

```sh
cargo build --release --target wasm32-unknown-unknown --features gpu --example bench_web
python3 tools/bench/run_web.py --out target/bench                 # software, webgl2, webgpu
python3 tools/bench/run_web.py --renderer software --swiftshader  # as on CI: no GPU, SwiftShader
```

### Baselines and budgets

`tools/bench/compare.py` compares a run with its baseline,
`perf/baseline/<runner>-<renderer>.json` (`linux`, `macos`, `windows`,
`web` × `software`, `gl`, `metal`, `d3d11`, `webgl2`, `webgpu`). The
baseline's median is first scaled by how fast each machine was (the ratio
of the two runs' `calibration_ms`, clamped to 1/3–3), then a scenario fails
when it is more than its threshold times that (1.5× by default) *and* more
than `min_delta_ms` slower. With `--rerun`, the failing scenarios are
measured again and only one that fails both times fails the check. A
missing baseline records only; a `"provisional": true` one reports without
failing.

The budgets are hard limits, whatever the baseline: on the desktop runners,
`hover` and `type_char` must be ready within 16 ms (one frame at 60 Hz) on
the software renderer and Metal; `scroll_list` (16 ms) and `theme_switch`
(100 ms) warn for now, as do the browser's `web_hover` and `web_type_char`
work (16 ms). Thresholds and budgets live in `perf/budgets.json`.

To take new baselines, run the Perf workflow by hand with
**update_baseline** ticked (it commits the run's reports to
`perf/baseline/`), or download a run's reports and copy them in:

```sh
python3 tools/bench/update_baseline.py --run <run id>       # needs gh
python3 tools/bench/update_baseline.py --from target/bench --provisional --note "my machine"
```

Each workflow run's summary shows the tables (baseline, expected after
scaling, now, delta, budget), and its artifacts hold the JSON (`perf-linux`,
`perf-macos`, `perf-windows`, `perf-web`).
