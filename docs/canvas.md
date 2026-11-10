# Custom painting

`Element::canvas` is a leaf whose pixels come from your code. It is laid out
and hit-tested like any other element, takes handlers, transforms and opacity,
and its content can change without rebuilding the view: you tell the app which
part of it changed, and only that part is painted again, rasterized and
presented. A terminal's grid, a chart, a waveform or a game board fits this
model.

```rust
use stipple::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

const GRID: CanvasId = CanvasId(1);

let rows = Rc::new(RefCell::new(vec![String::new(); 40]));
let font = Font::system_default();
let view_rows = rows.clone();
let app = App::new((), move |_: &(), _: &mut Cx<()>| {
    let (rows, font) = (view_rows.clone(), font.clone());
    Element::canvas(GRID, move |c: &mut Canvas| {
        let Some(font) = &font else { return };
        for (i, line) in rows.borrow().iter().enumerate() {
            let top = i as f64 * 17.0;
            let row = Rect::from_xywh(0.0, top, c.size().width, 17.0);
            if !c.needs_paint(row) {
                continue; // outside the area being painted
            }
            c.fill_rect(row, Color::BLACK);
            let mut run = GlyphRun::new(font, 13.0, Color::WHITE);
            run.push_cells(line, Point::new(0.0, top), 8.0);
            c.fill_glyph_run(&run);
        }
    })
    .grow(1.0)
});

// Later, from any thread, after changing row 3:
let handle = app.handle();
handle.invalidate_canvas_rect(GRID, Rect::from_xywh(0.0, 3.0 * 17.0, 960.0, 17.0));
```

## The paint callback

`Element::canvas(id, paint)` takes a `Fn(&mut Canvas)`. A `Canvas` is a
[`Scene`](https://docs.rs/stipple/latest/stipple/render/struct.Scene.html) in
the element's own coordinates: `(0, 0)` is its top-left corner, `c.size()` its
size, and what you draw is clipped to that box. Every `Scene` method works on
it.

The callback doesn't run while the view is built: it runs when the canvas is
**rasterized**, once for each area being rasterized, and `c.dirty()` is that
area. Draw at least the dirty area. Drawing more is harmless (anything outside
it is dropped before rasterizing), so testing rows with `c.needs_paint(rect)`
is only an optimization, but for a large grid it's the one that matters. The
callback may run several times per frame, each with a different area.

The callback isn't given the app state. It reads what it draws from data it
shares with the app: an `Rc<RefCell<…>>` you also keep in the state, or an
`Arc<Mutex<…>>` a background thread writes (a terminal's PTY reader).

## Repainting

- `AppHandle::invalidate_canvas(id)` repaints all of the canvas.
- `AppHandle::invalidate_canvas_rect(id, rect)` repaints `rect`, in the
  canvas's coordinates. A terminal passes the rows that changed.

Both are safe from any thread and don't rebuild the view: the app paints the
tree already on screen again, rasterizes only the invalidated areas (the
canvas callback asked for just those), and presents only them. Invalidations
that arrive before the app gets to them are merged. If a view rebuild happens
at the same time (a state update ran too), the invalidated areas are still
added to that frame's damage.

A rebuilt view doesn't repaint a canvas on its own: a new closure every build
would otherwise repaint it every build, and a hover change elsewhere would
repaint a whole terminal. Invalidate it, or give it a `revision`:

```rust
# use stipple::prelude::*;
# struct State { generation: u64 }
# fn view(s: &State, _: &mut Cx<State>) -> Element {
Element::canvas(CanvasId(2), |c: &mut Canvas| { /* … */ })
    .revision(s.generation) // a new value repaints all of it
# }
```

Moving or resizing the canvas repaints it, like any element. Through a
transform, invalidated areas are presented where the canvas is drawn.

## What to draw with

- Rects and rounded rects: `fill_rect`, `stroke_rect`, `fill_round_rect`,
  `stroke_round_rect`.
- Lines and paths: `draw_line(from, to, color, width)`, and `Path` (lines,
  quadratic and cubic Béziers) with `fill_path` and `stroke_path`;
  `StrokeStyle` sets the width, `LineCap`, `LineJoin` and a dash pattern.
- Bitmaps: `draw_image(rect, Arc<Pixmap>)` for straight-alpha RGBA. Drawing
  doesn't copy the pixels; when rasterizing, only the part of the bitmap inside
  the area being rasterized is copied. Keep the `Arc` and draw it every time
  (an inline sixel or iTerm2 image).
- Text at exact positions: `GlyphRun` holds clusters, each with its pen on the
  baseline, in one `Font` (family and style, so a bold run is a run with
  `font.bold()`), size and color, plus an optional `Decoration` (underlines in
  any style, strikethrough, overline) drawn along each stretch of adjacent
  clusters. `push_cells(text, top_left, advance)` places one character per cell;
  `push_cell(cluster, pen, width)` places a cluster spanning cells or carrying
  combining marks; `push(cluster, pen)` uses the cluster's own advance. Draw it
  with `fill_glyph_run`. `draw_decoration` draws decorations alone.
- Laid-out text: `fill_text` and `fill_layout`, as anywhere else.
- Anything else: `push_node` takes an `oxideav-core` node.

## Cost, and checking it

The software rasterizer works area by area: for an area repaint, everything that
can't touch the area is dropped first, so repainting one row of a 120×36 grid
costs about one row's worth (around 2 ms on a desktop CPU). A whole frame with
many primitives (a screenful of glyphs, or any canvas) is rasterized in bands
of 32 pixel rows the same way, with the same result.

`RasterStats::get()` / `RasterStats::take()` report how many pixels the
software rasterizer has filled, and in how many runs. The `canvasdemo` example
animates a 120×36 grid from a background thread at the display's refresh rate,
changing a few rows per tick; with `--selftest` it checks that the pixels
rasterized add up to the invalidated rows and not to whole frames. CI runs it on
X11, Wayland, Windows and macOS.

A custom renderer set with `App::render_with` gets the scene. The structured
`DrawCmd`s don't describe paths or canvases (`Scene::has_effects` returns `true`
for such a scene), but `stipple::gpu::render_scene_scaled` walks the scene's
nodes instead: it runs a canvas's paint callback for the part on screen and
draws what it painted on the GPU like the rest of the scene (see
[gpu.md](gpu.md)).

## Viewport content from another thread

For content a background thread renders into a whole bitmap (a video frame, a
sandboxed page), an embedded viewport (`Element::viewport`) fits better than a
canvas. `AppHandle::set_viewport_content(id, pixmap)` (and
`clear_viewport_content`) replace a viewport's content from any thread while
the app runs; only the viewport's area is presented, and the view isn't
rebuilt. Area repaints now work on windows with viewports too: the content is
put back over its placeholder after each area render.
