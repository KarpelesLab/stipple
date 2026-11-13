//! Finer-grained reactivity, through an [`App`]: memoized subtrees and
//! derived values, animation frames that only move what animates, painting
//! only what shows, and what a frame of a large list costs.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::App;
use crate::anim::Easing;
use crate::core::{Cx, Damage, Element, LayoutNode, ScrollId, Transition};
use crate::geometry::{PhysicalSize, Point, Rect, Size, Transform};
use crate::platform::DropData;
use crate::prelude::*;
use crate::render::Surface;

/// A surface that keeps nothing.
#[derive(Default)]
struct Sink {
    size: PhysicalSize,
}

impl Surface for Sink {
    fn resize(&mut self, size: PhysicalSize) {
        self.size = size;
    }
    fn size(&self) -> PhysicalSize {
        self.size
    }
    fn present(&mut self, _: &Pixmap, _: &[Rect]) {}
}

/// Present the primary window's next frame as the run loop does on a frame
/// wake: an animation frame with nothing else changed may skip the build.
/// Returns its damage.
fn step<S: 'static>(app: &mut App<S>, surface: &mut Sink) -> Damage {
    let App {
        state,
        theme,
        font,
        panes,
        viewport_content,
        ..
    } = app;
    let pane = &mut panes[0];
    pane.anim_only = pane.animating && !pane.dirty;
    pane.dirty |= pane.animating && !pane.anim_only;
    let (scene, damage) = match pane.repaint(theme, font.as_ref(), false) {
        Some(frame) => frame,
        None => {
            let scene = pane.build_frame(state, theme, font.as_ref());
            (scene, pane.take_damage())
        }
    };
    let phys = pane.scale.to_physical(pane.attrs.logical_size);
    let shown = damage.clone();
    pane.present_scene(scene, damage, theme, viewport_content, surface, phys, false);
    shown
}

fn tree<S: 'static>(app: &App<S>) -> &LayoutNode {
    app.panes[0].tree.as_ref().expect("a frame was built")
}

/// Whether two laid-out trees draw the same.
fn same(a: &LayoutNode, b: &LayoutNode) -> bool {
    crate::core::diff_trees(a, b) == Damage::None
}

fn area(damage: &Damage) -> f64 {
    match damage {
        Damage::Regions(r) => r.iter().map(|r| r.width() * r.height()).sum(),
        Damage::Full => f64::INFINITY,
        Damage::None => 0.0,
    }
}

// --- Memoized subtrees -------------------------------------------------------

struct Rows {
    labels: Vec<&'static str>,
    tapped: Vec<usize>,
    /// Whether a button is shown above the rows (it shifts their ids).
    extra: bool,
    builds: Rc<Cell<usize>>,
}

const ROW_H: f64 = 20.0;

fn rows_app(labels: Vec<&'static str>) -> (App<Rows>, Rc<Cell<usize>>) {
    let builds = Rc::new(Cell::new(0));
    let state = Rows {
        labels,
        tapped: Vec::new(),
        extra: false,
        builds: builds.clone(),
    };
    let app = App::new(state, |s: &Rows, cx: &mut Cx<Rows>| {
        let mut children = Vec::new();
        if s.extra {
            children.push(
                Element::boxed(Default::default())
                    .width(200.0)
                    .height(ROW_H)
                    .on_tap(cx, |s: &mut Rows| s.tapped.push(99)),
            );
        }
        for (i, &text) in s.labels.iter().enumerate() {
            let builds = s.builds.clone();
            children.push(cx.memo(("row", i), text, move |cx| {
                builds.set(builds.get() + 1);
                Element::text(text, 12.0, Color::BLACK)
                    .width(200.0)
                    .height(ROW_H)
                    .on_tap(cx, move |s: &mut Rows| s.tapped.push(i))
            }));
        }
        Element::stack(Axis::Vertical, children)
    })
    .logical_size(Size::new(200.0, 200.0));
    (app, builds)
}

/// The tap ids in the laid-out tree, in order.
fn actions(node: &LayoutNode, out: &mut Vec<crate::core::ActionId>) {
    out.extend(node.action);
    for c in &node.children {
        actions(c, out);
    }
}

fn rebuild<S: 'static>(app: &mut App<S>) {
    app.panes[0].dirty = true;
    let _ = app.render_frame();
}

#[test]
fn a_memoized_subtree_is_built_again_only_when_its_deps_change() {
    let (mut app, builds) = rows_app(vec!["a", "b", "c"]);
    let _ = app.render_frame();
    assert_eq!(builds.get(), 3);
    rebuild(&mut app);
    assert_eq!(builds.get(), 3, "nothing changed: nothing built");
    app.state.labels[1] = "B";
    rebuild(&mut app);
    assert_eq!(builds.get(), 4, "only the changed row");
    // A new theme builds them all.
    app.base_theme = Theme::dark();
    rebuild(&mut app);
    assert_eq!(builds.get(), 7);
    // Rows that go away are forgotten, and built afresh when they're back.
    app.state.labels.pop();
    rebuild(&mut app);
    assert_eq!(builds.get(), 7);
    app.state.labels.push("c");
    rebuild(&mut app);
    assert_eq!(builds.get(), 8);
}

#[test]
fn a_reused_subtree_keeps_its_handlers_under_the_same_ids() {
    let (mut app, builds) = rows_app(vec!["a", "b", "c"]);
    let _ = app.render_frame();
    let mut before = Vec::new();
    actions(tree(&app), &mut before);
    rebuild(&mut app);
    assert_eq!(builds.get(), 3, "reused");
    let mut after = Vec::new();
    actions(tree(&app), &mut after);
    assert_eq!(before, after, "the same ids");
    // And they still reach the handlers that were built with the rows.
    assert!(app.click_at(Point::new(10.0, ROW_H * 1.5)));
    let _ = app.render_frame();
    assert!(app.click_at(Point::new(10.0, ROW_H * 2.5)));
    assert_eq!(app.state.tapped, vec![1, 2]);
    assert_eq!(builds.get(), 3, "clicks changed nothing the rows depend on");

    // A tap target above them moves their ids along: they're built again
    // (so ids their handlers captured stay right), then reused there.
    app.state.extra = true;
    rebuild(&mut app);
    assert_eq!(builds.get(), 6);
    rebuild(&mut app);
    assert_eq!(builds.get(), 6);
    app.state.tapped.clear();
    assert!(app.click_at(Point::new(10.0, ROW_H * 0.5)));
    let _ = app.render_frame();
    assert!(app.click_at(Point::new(10.0, ROW_H * 1.5)));
    let _ = app.render_frame();
    assert!(app.click_at(Point::new(10.0, ROW_H * 3.5)));
    assert_eq!(app.state.tapped, vec![99, 0, 2]);
}

struct Panel {
    dropped: Vec<String>,
    builds: Rc<Cell<usize>>,
}

#[test]
fn scroll_containers_and_drop_targets_keep_their_ids_too() {
    let builds = Rc::new(Cell::new(0));
    let state = Panel {
        dropped: Vec::new(),
        builds: builds.clone(),
    };
    let mut app = App::new(state, |s: &Panel, cx: &mut Cx<Panel>| {
        // A container before the memoized one takes the first id.
        let first = crate::widgets::scroll(cx, 50.0, Element::boxed(Default::default()));
        let builds = s.builds.clone();
        let panel = cx.memo("panel", (), move |cx| {
            builds.set(builds.get() + 1);
            let content = Element::stack(Axis::Vertical, vec![]).height(200.0);
            let list = crate::widgets::scroll(cx, 50.0, content);
            let id = list.scroll.expect("a scroll container");
            // A button that scrolls the list by the id it captured.
            let button = Element::boxed(Default::default())
                .height(20.0)
                .on_tap(cx, move |_: &mut Panel| {});
            let requests = cx.scroll_requests();
            let button = button.on_drop(cx, move |s: &mut Panel, data, _| {
                if let DropData::Text(t) = data {
                    s.dropped.push(t.clone());
                }
                requests.scroll_to(id, 30.0);
            });
            Element::stack(Axis::Vertical, vec![button, list])
        });
        Element::stack(Axis::Vertical, vec![first, panel])
    })
    .logical_size(Size::new(200.0, 200.0));
    let _ = app.render_frame();
    fn drop_of(n: &LayoutNode) -> Option<crate::core::DropId> {
        n.drop.or_else(|| n.children.iter().find_map(drop_of))
    }
    let find_drop = |app: &App<Panel>| drop_of(tree(app)).expect("the drop target");
    let (drop, list) = (find_drop(&app), ScrollId(1));
    rebuild(&mut app);
    rebuild(&mut app);
    assert_eq!(builds.get(), 1, "reused");
    assert_eq!(find_drop(&app), drop);
    let pane = &mut app.panes[0];
    let data = DropData::Text("hi".into());
    assert!(
        pane.handlers
            .dispatch_drop(drop, &data, Point::new(0.0, 0.0), &mut app.state)
    );
    assert_eq!(app.state.dropped, vec!["hi".to_string()]);
    rebuild(&mut app);
    assert_eq!(app.panes[0].scroll_offsets.get(&list), Some(&30.0));
}

#[test]
fn what_a_memoized_subtree_reads_through_cx_is_an_input() {
    let builds = Rc::new(Cell::new(0));
    let counter = builds.clone();
    let mut app = App::new((), move |_: &(), cx: &mut Cx<()>| {
        let counter = counter.clone();
        cx.memo("card", (), move |cx| {
            counter.set(counter.get() + 1);
            let fill = if cx.interaction("card").hovered() {
                Color::rgb(255, 0, 0)
            } else {
                Color::rgb(0, 0, 255)
            };
            Element::boxed(Default::default())
                .width(50.0)
                .height(50.0)
                .fill(fill)
                .key("card")
        })
    })
    .logical_size(Size::new(100.0, 100.0));
    let _ = app.render_frame();
    rebuild(&mut app);
    assert_eq!(builds.get(), 1);
    // Reused, it still counts as reading the card's states: hovering it
    // asks for a build, which builds it again.
    assert!(app.hover_at(Point::new(10.0, 10.0)));
    let _ = app.render_frame();
    assert_eq!(builds.get(), 2);
    assert_eq!(tree(&app).decoration.fill, Some(Color::rgb(255, 0, 0)));
}

#[test]
fn a_memoized_subtree_that_animates_is_built_every_time() {
    let builds = Rc::new(Cell::new(0));
    let counter = builds.clone();
    let mut app = App::new((), move |_: &(), cx: &mut Cx<()>| {
        let counter = counter.clone();
        cx.memo("spinner", (), move |cx| {
            counter.set(counter.get() + 1);
            cx.request_frame();
            Element::boxed(Default::default()).width(10.0).height(10.0)
        })
    });
    let _ = app.render_frame();
    let mut surface = Sink::default();
    let _ = step(&mut app, &mut surface);
    let _ = step(&mut app, &mut surface);
    assert_eq!(builds.get(), 3);
}

#[test]
fn nested_subtrees_are_kept_with_the_one_around_them() {
    let theme = Theme::light();
    let (outer, inner) = (Cell::new(0), Cell::new(0));
    let build = |cache, deps: u32| {
        let mut cx = Cx::<()>::new(&theme);
        cx.set_memo_cache(cache);
        let _ = cx.memo("outer", deps, |cx| {
            outer.set(outer.get() + 1);
            let child = cx.memo("inner", (), |_| {
                inner.set(inner.get() + 1);
                Element::text("inside", 12.0, Color::BLACK)
            });
            Element::stack(Axis::Vertical, vec![child])
        });
        cx.take_memo_cache()
    };
    let cache = build(Default::default(), 1);
    let cache = build(cache, 1);
    assert_eq!((outer.get(), inner.get()), (1, 1));
    assert_eq!(cache.len(), 2, "the inner one is kept with the outer one");
    let cache = build(cache, 2);
    assert_eq!((outer.get(), inner.get()), (2, 1), "only the outer one");
    let _ = build(cache, 2);
    assert_eq!((outer.get(), inner.get()), (2, 1));
}

#[test]
fn a_key_used_twice_in_a_build_is_built_both_times() {
    let theme = Theme::light();
    let builds = Cell::new(0);
    let mut cache = Default::default();
    for _ in 0..2 {
        let mut cx = Cx::<()>::new(&theme);
        cx.set_memo_cache(cache);
        for _ in 0..2 {
            let _ = cx.memo("same", (), |_| {
                builds.set(builds.get() + 1);
                Element::boxed(Default::default())
            });
        }
        cache = cx.take_memo_cache();
    }
    assert_eq!(builds.get(), 3, "the first use is kept, the second built");
}

#[test]
fn a_derived_value_is_computed_again_only_when_its_deps_change() {
    let theme = Theme::light();
    let runs = Cell::new(0);
    let mut cache = Default::default();
    for (prices, total) in [(vec![1, 2], 3), (vec![1, 2], 3), (vec![1, 5], 6)] {
        let mut cx = Cx::<()>::new(&theme);
        cx.set_memo_cache(cache);
        let sum = cx.derive("total", &prices, || {
            runs.set(runs.get() + 1);
            prices.iter().sum::<u32>()
        });
        assert_eq!(sum, total);
        cache = cx.take_memo_cache();
    }
    assert_eq!(runs.get(), 2);
}

#[test]
fn a_reused_subtree_is_laid_out_from_its_last_layout_until_its_look_changes() {
    let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
        let card = cx.memo("card", (), |_| {
            Element::stack(
                Axis::Vertical,
                vec![Element::boxed(Default::default()).width(40.0).height(20.0)],
            )
            .padding(Insets::uniform(4.0))
            .key("card")
            .hovered(|e| e.padding(Insets::uniform(10.0)))
        });
        Element::stack(Axis::Vertical, vec![card])
    })
    .logical_size(Size::new(200.0, 200.0));
    let _ = app.render_frame();
    let first = tree(&app).clone();
    rebuild(&mut app);
    assert!(same(tree(&app), &first), "the same layout");
    // Hovered, it lowers to a different padding: laid out again.
    assert!(app.hover_at(Point::new(5.0, 5.0)));
    let _ = app.render_frame();
    let card = &tree(&app).children[0];
    assert_eq!(card.children[0].bounds.origin, Point::new(10.0, 10.0));
    assert!(app.hover_at(Point::new(150.0, 150.0)));
    let _ = app.render_frame();
    assert!(same(tree(&app), &first), "back as it was");
}

// --- Animation frames that skip the build -----------------------------------

struct Mover {
    right: bool,
    builds: Rc<Cell<usize>>,
}

const MOVE: Duration = Duration::from_secs(30);

/// A tile that moves right (or turns from black to white, if `fill`) over
/// `length` when told to, above 50 rows.
fn mover_app(fill: bool, length: Duration) -> (App<Mover>, Rc<Cell<usize>>) {
    let builds = Rc::new(Cell::new(0));
    let state = Mover {
        right: false,
        builds: builds.clone(),
    };
    let app = App::new(state, move |s: &Mover, _: &mut Cx<Mover>| {
        s.builds.set(s.builds.get() + 1);
        let x = if s.right { 100.0 } else { 0.0 };
        let mut tile = Element::boxed(Default::default())
            .width(40.0)
            .height(30.0)
            .key("tile")
            .transition(Transition::tween(length, Easing::Linear));
        tile = if fill {
            tile.fill(if s.right { Color::WHITE } else { Color::BLACK })
        } else {
            tile.fill(Color::BLACK)
                .transform(Transform::IDENTITY.translate(x, 0.0))
        };
        let rows = (0..50)
            .map(|i| {
                Element::text(format!("row {i}"), 12.0, Color::BLACK)
                    .height(ROW_H)
                    .fill(Color::rgb(240, 240, 240))
            })
            .collect();
        Element::stack(
            Axis::Vertical,
            vec![tile, Element::stack(Axis::Vertical, rows)],
        )
    })
    .logical_size(Size::new(300.0, 300.0));
    (app, builds)
}

#[test]
fn a_moving_element_is_drawn_again_without_a_build_and_damages_only_its_path() {
    let (mut app, builds) = mover_app(false, MOVE);
    let mut surface = Sink::default();
    let _ = step(&mut app, &mut surface);
    app.state.right = true;
    app.panes[0].dirty = true;
    let _ = step(&mut app, &mut surface);
    assert!(app.animating());
    let built = builds.get();
    let mut x = tree(&app).children[0]
        .transform
        .map_or(0.0, |t| t.translate.dx);
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(5));
        let damage = step(&mut app, &mut surface);
        let tile = &tree(&app).children[0];
        let now = tile.transform.expect("moving").translate.dx;
        assert!(now > x && now < 100.0, "on its way: {x} -> {now}");
        // Where it was and where it is now (a pixel wider each way), merged.
        let expect = Rect::from_xywh(x - 1.0, -1.0, now - x + 42.0, 32.0);
        let Damage::Regions(r) = &damage else {
            panic!("localized: {damage:?}")
        };
        assert_eq!(r.len(), 1);
        assert!(
            (r[0].min_x() - expect.min_x()).abs() < 1e-6,
            "{r:?} vs {expect:?}"
        );
        assert!(
            (r[0].width() - expect.width()).abs() < 1e-6,
            "{r:?} vs {expect:?}"
        );
        assert_eq!(r[0].height(), expect.height());
        let presented = app.panes[0].presented.as_ref().expect("presented");
        assert!(same(presented, tree(&app)), "the baseline follows");
        x = now;
    }
    assert_eq!(builds.get(), built, "the view wasn't built");
    // A real change builds again.
    app.state.right = false;
    app.panes[0].dirty = true;
    let _ = step(&mut app, &mut surface);
    assert_eq!(builds.get(), built + 1);
}

#[test]
fn a_color_change_repaints_just_the_element() {
    let (mut app, builds) = mover_app(true, Duration::from_secs(2));
    let mut surface = Sink::default();
    let _ = step(&mut app, &mut surface);
    app.state.right = true;
    app.panes[0].dirty = true;
    let _ = step(&mut app, &mut surface);
    let built = builds.get();
    std::thread::sleep(Duration::from_millis(100));
    let damage = step(&mut app, &mut surface);
    assert_eq!(builds.get(), built);
    assert_eq!(
        damage,
        Damage::Regions(vec![Rect::from_xywh(0.0, 0.0, 40.0, 30.0)])
    );
    let fill = tree(&app).children[0].decoration.fill.expect("a fill");
    assert!(
        fill != Color::BLACK && fill != Color::WHITE,
        "between: {fill:?}"
    );
}

#[test]
fn frames_the_view_asks_for_or_that_move_the_layout_are_built() {
    // A view that animates itself is built for every frame.
    let builds = Rc::new(Cell::new(0));
    let counter = builds.clone();
    let mut app = App::new((), move |_: &(), cx: &mut Cx<()>| {
        counter.set(counter.get() + 1);
        cx.request_frame();
        Element::boxed(Default::default()).width(10.0).height(10.0)
    });
    let mut surface = Sink::default();
    for _ in 0..3 {
        let _ = step(&mut app, &mut surface);
    }
    assert_eq!(builds.get(), 3);

    // A size in motion moves the layout: built for every frame.
    let builds = Rc::new(Cell::new(0));
    let counter = builds.clone();
    let wide = Rc::new(Cell::new(false));
    let w = wide.clone();
    let mut app = App::new((), move |_: &(), _: &mut Cx<()>| {
        counter.set(counter.get() + 1);
        Element::boxed(Default::default())
            .width(if w.get() { 100.0 } else { 10.0 })
            .height(10.0)
            .key("bar")
            .transition(Transition::tween(MOVE, Easing::Linear))
    });
    let _ = step(&mut app, &mut surface);
    wide.set(true);
    app.panes[0].dirty = true;
    let _ = step(&mut app, &mut surface);
    std::thread::sleep(Duration::from_millis(2));
    let _ = step(&mut app, &mut surface);
    assert_eq!(builds.get(), 3);
}

// --- Painting what shows ------------------------------------------------------

#[test]
fn rows_scrolled_out_of_view_are_not_painted() {
    let rows = (0..200)
        .map(|i| {
            Element::boxed(Default::default())
                .height(ROW_H)
                .fill(Color::rgb(i as u8, 0, 0))
        })
        .collect();
    let theme = Theme::light();
    let mut cx = Cx::<()>::new(&theme);
    let list = crate::widgets::scroll(&mut cx, 100.0, Element::stack(Axis::Vertical, rows));
    let window = Size::new(100.0, 300.0);
    let mut laid = crate::core::layout(&list, Rect::from_xywh(0.0, 0.0, 100.0, 300.0), None);
    let mut offsets = std::collections::HashMap::new();
    offsets.insert(ScrollId(0), 1000.0);
    crate::core::apply_scroll(&mut laid, &mut offsets);
    let mut all = crate::render::Scene::new(window);
    crate::core::paint(&laid, &mut all, None);
    let mut shown = crate::render::Scene::new(window);
    let visible = Rect::from_xywh(0.0, 0.0, window.width, window.height);
    crate::core::paint_visible(&laid, &mut shown, None, visible);
    let (shown_n, all_n) = (shown.primitive_count(), all.primitive_count());
    assert!(shown_n < 20, "only the rows in view: {shown_n}");
    assert!(all_n > 200, "{all_n}");
    let render =
        |scene| crate::render::SoftwareRenderer::new().render(scene, ScaleFactor::IDENTITY);
    assert!(render(all) == render(shown), "the same pixels");
}

// --- What a frame costs --------------------------------------------------------

struct Bench {
    moved: bool,
}

fn list_row(theme: &Theme, cx: &mut Cx<Bench>, i: usize) -> Element {
    row(vec![
        label(theme, format!("Row {i}")),
        spacer(),
        label(theme, format!("{} items", i * 7 % 13)),
    ])
    .padding(Insets::uniform(4.0))
    .fill(if i.is_multiple_of(2) {
        theme.palette.surface
    } else {
        theme.palette.background
    })
    .on_tap(cx, move |_: &mut Bench| {})
}

/// 500 rows in a scroll container, each memoized if `memo`.
fn list(theme: &Theme, cx: &mut Cx<Bench>, memo: bool) -> Element {
    let rows = (0..500)
        .map(|i| match memo {
            true => cx.memo(("row", i), i, |cx| list_row(theme, cx, i)),
            false => list_row(theme, cx, i),
        })
        .collect();
    crate::widgets::scroll(cx, 560.0, column(rows))
}

/// The median time of `frames` frames' build (or animation frame) and their
/// presenting, and their mean damaged area.
fn measure(app: &mut App<Bench>, frames: usize) -> (Duration, Duration, f64) {
    let mut surface = Sink::default();
    let (mut builds, mut presents, mut damaged) = (Vec::new(), Vec::new(), 0.0);
    for _ in 0..frames {
        std::thread::sleep(Duration::from_millis(1));
        let App {
            state,
            theme,
            font,
            panes,
            viewport_content,
            ..
        } = app;
        let pane = &mut panes[0];
        pane.anim_only = pane.animating && !pane.dirty;
        pane.dirty |= pane.animating && !pane.anim_only;
        let start = Instant::now();
        let (scene, damage) = match pane.repaint(theme, font.as_ref(), false) {
            Some(frame) => frame,
            None => {
                let scene = pane.build_frame(state, theme, font.as_ref());
                (scene, pane.take_damage())
            }
        };
        builds.push(start.elapsed());
        damaged += match &damage {
            Damage::Full => 800.0 * 600.0,
            d => area(d),
        };
        let phys = pane.scale.to_physical(pane.attrs.logical_size);
        let start = Instant::now();
        pane.present_scene(
            scene,
            damage,
            theme,
            viewport_content,
            &mut surface,
            phys,
            false,
        );
        presents.push(start.elapsed());
    }
    builds.sort();
    presents.sort();
    let mid = frames / 2;
    (builds[mid], presents[mid], damaged / frames as f64)
}

/// A frame of 500 rows and one moving element: through an implicit
/// transition (drawn without a build) and through a view that animates it
/// itself (built every frame), with the rows memoized or not. Prints what a
/// frame costs (`cargo test --release bench_ -- --nocapture`); checks the
/// damage stays where the element moves.
#[test]
fn bench_a_large_list_with_one_moving_element() {
    let Some(font) = Font::system_default() else {
        eprintln!("skipping: no system font found");
        return;
    };
    const FRAMES: usize = 60;
    let mut app = App::new(Bench { moved: false }, |s: &Bench, cx: &mut Cx<Bench>| {
        let theme = *cx.theme();
        let x = if s.moved { 300.0 } else { 0.0 };
        let mover = Element::boxed(Default::default())
            .width(40.0)
            .height(30.0)
            .fill(theme.palette.primary)
            .key("mover")
            .transform(Transform::IDENTITY.translate(x, 0.0))
            .transition(Transition::tween(Duration::from_secs(60), Easing::Linear));
        column(vec![mover, list(&theme, cx, false)])
    })
    .logical_size(Size::new(800.0, 600.0))
    .font(font.clone());
    let _ = measure(&mut app, 3);
    app.state.moved = true;
    app.panes[0].dirty = true;
    let _ = measure(&mut app, 1);
    let (build, present, damaged) = measure(&mut app, FRAMES);
    eprintln!("transition: build {build:?}, present {present:?}, damage {damaged:.0} px²");
    assert!(damaged < 2000.0, "damage where it moves: {damaged}");

    for memo in [false, true] {
        let mut app = App::new(
            Bench { moved: false },
            move |_: &Bench, cx: &mut Cx<Bench>| {
                let theme = *cx.theme();
                cx.request_frame();
                let turn = cx.frame_time().as_secs_f64() * 3.0;
                let spinner = Element::boxed(Default::default())
                    .width(30.0)
                    .height(30.0)
                    .fill(theme.palette.primary)
                    .transform(Transform::IDENTITY.rotate(turn));
                column(vec![spinner, list(&theme, cx, memo)])
            },
        )
        .logical_size(Size::new(800.0, 600.0))
        .font(font.clone());
        let _ = measure(&mut app, 3);
        let (build, present, damaged) = measure(&mut app, FRAMES);
        eprintln!(
            "view-driven (memo {memo}): build {build:?}, present {present:?}, damage {damaged:.0} px²"
        );
        assert!(damaged < 2000.0, "damage where it turns: {damaged}");
    }
}
