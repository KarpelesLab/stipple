//! `text_view`'s long lines through the `App`: scrolling sideways by wheel,
//! Shift+wheel, trackpad, keys and the bar; find and the selection
//! scrolling what they reach into view; the last line kept clear of the
//! sideways bar.

use crate::App;
use crate::core::{KeyInput, LayoutNode, NodeContent, scroll_range_for};
use crate::geometry::{Point, Rect, Size};
use crate::platform::{ButtonState, Event, Modifiers, PointerButton, ScrollDelta};
use crate::render::{Font, FontStyle};
use crate::style::{ScrollBarLook, Theme};
use crate::widgets::{FindOptions, LinePos, TextView, TextViewOptions, column, text_view};

const LH: f64 = 20.0;
const GUTTER: f64 = 12.0;

struct Log {
    view: TextView,
}

/// The long line, every tenth.
fn long(i: usize) -> String {
    format!("{i} {}END", "x".repeat(300))
}

/// A 400×300 window with the classic bars and a 200px-high view of 100
/// lines, every tenth of which (5, 15, ...) is over 300 characters long.
fn app(font: Font) -> App<Log> {
    let mut view = TextView::new();
    view.append((0..100).map(|i| {
        if i % 10 == 5 {
            long(i)
        } else {
            format!("line {i}")
        }
    }));
    let shown = font.clone();
    App::new(Log { view }, move |s: &Log, cx| {
        let options = TextViewOptions::default()
            .height(200.0)
            .line_height(LH)
            .text_size(14.0)
            .font(shown.clone());
        column(vec![text_view(cx, &s.view, |s| &mut s.view, options)])
    })
    .font(font)
    .theme(Theme::light().scroll_bar(ScrollBarLook::CLASSIC))
    .logical_size(Size::new(400.0, 300.0))
}

fn font() -> Option<Font> {
    let f = Font::from_family("monospace", FontStyle::default())
        .ok()
        .or_else(Font::system_default);
    if f.is_none() {
        eprintln!("skipping: no system font");
    }
    f
}

/// The view's scroll container: it scrolls down, and sideways.
fn view_node(app: &App<Log>) -> &LayoutNode {
    fn find(n: &LayoutNode) -> Option<&LayoutNode> {
        if n.scroll_x.is_some() {
            return Some(n);
        }
        n.children.iter().find_map(find)
    }
    find(app.panes[0].tree.as_ref().expect("built")).expect("the view")
}

fn offsets(app: &App<Log>) -> (f64, f64) {
    let node = view_node(app);
    let get = |id| app.panes[0].scroll_offsets.get(&id).copied().unwrap_or(0.0);
    (get(node.scroll.unwrap()), get(node.scroll_x.unwrap()))
}

/// How far the view scrolls sideways at most.
fn max_x(app: &App<Log>) -> f64 {
    let node = view_node(app);
    let id = node.scroll_x.unwrap();
    scroll_range_for(node, id, offsets(app).1)
}

/// The text leaves in the view, in order.
fn leaves(app: &App<Log>) -> Vec<&LayoutNode> {
    fn walk<'a>(n: &'a LayoutNode, out: &mut Vec<&'a LayoutNode>) {
        if matches!(n.content, NodeContent::Text { .. }) {
            out.push(n);
        }
        n.children.iter().for_each(|c| walk(c, out));
    }
    let mut out = Vec::new();
    walk(view_node(app), &mut out);
    out
}

fn wheel(app: &mut App<Log>, delta: ScrollDelta) -> bool {
    app.pointer_event(Event::Scroll { delta })
}

fn button(app: &mut App<Log>, state: ButtonState, at: Point) -> bool {
    app.pointer_event(Event::PointerButton {
        button: PointerButton::Left,
        state,
        position: at,
    })
}

fn center(r: Rect) -> Point {
    Point::new(r.min_x() + r.width() / 2.0, r.min_y() + r.height() / 2.0)
}

#[test]
fn long_lines_scroll_sideways_by_wheel_trackpad_shift_and_keys() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    app.render_once();
    app.render_once();
    let node = view_node(&app);
    assert!(node.scroll_bar.is_some(), "a bar down the side");
    assert!(node.scroll_bar_x.is_some(), "and one along the bottom");
    let view = node.bounds;
    assert!(view.width() <= 400.0, "the view isn't as wide as its lines");
    let max = max_x(&app);
    assert!(max > 1_000.0, "300 characters scroll sideways: {max}");
    let left = leaves(&app)[0].bounds.min_x();

    // A sideways wheel.
    app.hover_at(Point::new(200.0, 100.0));
    assert!(wheel(&mut app, ScrollDelta::lines(2.0, 0.0)));
    app.render_once();
    let (y, x) = offsets(&app);
    assert_eq!(y, 0.0, "it didn't scroll down");
    assert!(x > 0.0);
    // The lines moved left by as much.
    assert!((leaves(&app)[0].bounds.min_x() - (left - x)).abs() < 0.5);

    // A trackpad's swipe, mostly sideways.
    assert!(wheel(&mut app, ScrollDelta::pixels(25.0, 3.0)));
    app.render_once();
    assert_eq!(offsets(&app), (0.0, x + 25.0));

    // Shift turns a vertical wheel sideways.
    crate::platform::set_pointer_modifiers(Modifiers::SHIFT);
    assert!(app.scroll_by(30.0));
    crate::platform::set_pointer_modifiers(Modifiers::default());
    app.render_once();
    assert_eq!(offsets(&app), (0.0, x + 55.0));
    // Without, it scrolls down.
    assert!(app.scroll_by(30.0));
    app.render_once();
    assert_eq!(offsets(&app), (30.0, x + 55.0));

    // Left and Right scroll sideways while the view has the focus.
    assert!(app.text_press_at(Point::new(50.0, 50.0)));
    app.end_text_select();
    app.render_once();
    assert!(app.press_key(KeyInput::Right));
    app.render_once();
    let after = offsets(&app).1;
    assert!(after > x + 55.0);
    assert!(app.press_key(KeyInput::Left));
    app.render_once();
    assert!((offsets(&app).1 - (x + 55.0)).abs() < 0.5);

    // Far past the end: stopped at the end, where the long lines' ends
    // show left of the vertical bar.
    assert!(wheel(&mut app, ScrollDelta::pixels(100_000.0, 0.0)));
    app.render_once();
    assert!(
        (offsets(&app).1 - max).abs() < 0.5,
        "{:?} {max}",
        offsets(&app)
    );
    let font = app.font.get().cloned().unwrap();
    let start = leaves(&app)
        .into_iter()
        .find(|l| matches!(&l.content, NodeContent::Text { text, .. } if text.ends_with("END")))
        .expect("a long line on screen")
        .text_rect()
        .min_x();
    let end = start + font.measure(&long(5), 14.0).width;
    assert!(end <= view.max_x() - GUTTER + 0.5, "{end} in {view:?}");
    assert!(end >= view.max_x() - GUTTER - 30.0, "{end} in {view:?}");
}

#[test]
fn find_and_the_selection_scroll_what_they_reach_into_view() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    app.render_once();
    app.render_once();
    let view = view_node(&app).bounds;
    // The match at a long line's end: scrolled down to and sideways.
    app.handle().update(|s: &mut Log| {
        assert_eq!(s.view.find("END", FindOptions::default()), 10);
        s.view.find_next();
    });
    app.run_pending();
    app.render_once();
    app.render_once();
    let (_, x) = offsets(&app);
    assert!(x > 1_000.0, "{x}");
    // On screen: the match's text is between the view's edges.
    let m = app.state().view.matches()[0].clone();
    let line = app.state().view.line(m.line).unwrap().to_string();
    let font = app.font.get().cloned().unwrap();
    let leaf = leaves(&app)
        .into_iter()
        .find(|l| match &l.content {
            NodeContent::Text { text, .. } => *text == line,
            _ => false,
        })
        .expect("the long line is built");
    let start = leaf.text_rect().min_x() + font.measure(&line[..m.range.start], 14.0).width;
    let end = leaf.text_rect().min_x() + font.measure(&line[..m.range.end], 14.0).width;
    assert!(
        start >= view.min_x() && end <= view.max_x() - GUTTER,
        "{start}..{end} in {view:?}"
    );

    // Back to a line's start: scrolled back.
    app.handle()
        .update(|s: &mut Log| s.view.scroll_to(LinePos::new(5, 0)));
    app.run_pending();
    app.render_once();
    assert_eq!(offsets(&app).1, 0.0);

    // Shift+Right along a long line follows the selection's end.
    assert!(app.text_press_at(Point::new(30.0, 30.0)));
    app.end_text_select();
    app.handle().update(|s: &mut Log| {
        s.view.select(LinePos::new(15, 0), LinePos::new(15, 280));
    });
    app.run_pending();
    assert!(app.press_key(KeyInput::SelectRight));
    app.render_once();
    app.render_once();
    assert!(offsets(&app).1 > 1_000.0, "{:?}", offsets(&app));
}

#[test]
fn a_drag_past_the_right_edge_selects_on_and_scrolls() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    app.render_once();
    app.render_once();
    // Line 5, from near its start, dragged to the view's right edge and
    // past it.
    let y = 1.0 + 5.0 * LH + LH / 2.0;
    assert!(app.text_press_at(Point::new(20.0, y)));
    let mut last = 0.0;
    for k in 0..6 {
        assert!(app.text_drag_at(Point::new(399.0 + k as f64, y)));
        app.render_once();
        let x = offsets(&app).1;
        assert!(x >= last, "{x} after {last}");
        last = x;
    }
    app.end_text_select();
    assert!(last > 0.0, "the drag scrolled sideways");
    let (from, to) = app.state().view.selection().unwrap();
    assert_eq!((from.line, to.line), (5, 5));
    assert!(to.byte > 40, "{to:?}");
}

#[test]
fn the_sideways_thumb_drags_and_the_last_line_clears_its_bar() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    app.render_once();
    app.render_once();
    let node = view_node(&app);
    let view = node.bounds;
    let thumb = node.scroll_bar_x.as_ref().unwrap().thumb;
    // Along the bottom, short of the vertical bar's corner.
    assert!(thumb.max_y() <= view.max_y() + 0.5);
    assert!(thumb.min_y() >= view.max_y() - GUTTER - 0.5);
    let track = node.scroll_bar_x.as_ref().unwrap().track.unwrap().0;
    assert!(track.max_x() <= view.max_x() - GUTTER + 0.5);
    let vtrack = node.scroll_bar.as_ref().unwrap().track.unwrap().0;
    assert!(vtrack.max_y() <= view.max_y() - GUTTER + 0.5);

    let grab = center(thumb);
    assert!(button(&mut app, ButtonState::Pressed, grab));
    app.pointer_event(Event::PointerMoved {
        position: Point::new(grab.x + 100.0, grab.y),
    });
    assert!(button(
        &mut app,
        ButtonState::Released,
        Point::new(grab.x + 100.0, grab.y)
    ));
    app.render_once();
    let (y, x) = offsets(&app);
    assert_eq!(y, 0.0);
    // The thumb moved 100 px of its travel: as much of the content.
    let max = max_x(&app);
    let travel = track.width() - 2.0 * 3.0 - thumb.width();
    assert!((x - max * 100.0 / travel).abs() < 1.0, "{x} of {max}");
    assert_eq!(
        app.state().view.selection(),
        None,
        "the press was the bar's"
    );

    // At the end, the last line shows above the sideways bar.
    app.handle().update(|s: &mut Log| s.view.scroll_to_end());
    app.run_pending();
    app.render_once();
    app.render_once();
    let last = leaves(&app)
        .into_iter()
        .max_by(|a, b| a.bounds.max_y().total_cmp(&b.bounds.max_y()))
        .unwrap()
        .bounds;
    let bar = view_node(&app).scroll_bar_x.as_ref().unwrap().thumb;
    assert!(last.max_y() <= bar.min_y() + 3.0, "{last:?} under {bar:?}");
}

#[test]
fn short_lines_have_no_sideways_bar() {
    let Some(font) = font() else { return };
    let mut app = App::new(
        Log {
            view: TextView::from_text("one\ntwo\nthree"),
        },
        |s: &Log, cx| {
            let options = TextViewOptions::default().height(200.0);
            column(vec![text_view(cx, &s.view, |s| &mut s.view, options)])
        },
    )
    .font(font)
    .theme(Theme::light().scroll_bar(ScrollBarLook::CLASSIC))
    .logical_size(Size::new(400.0, 300.0));
    app.render_once();
    app.render_once();
    assert!(view_node(&app).scroll_bar_x.is_none());
    assert_eq!(max_x(&app), 0.0);
    app.hover_at(Point::new(100.0, 50.0));
    wheel(&mut app, ScrollDelta::pixels(50.0, 0.0));
    app.render_once();
    assert_eq!(offsets(&app).1, 0.0);
}

#[test]
fn a_sideways_finger_drag_scrolls_sideways() {
    use crate::platform::{Touch, TouchPhase};
    let Some(font) = font() else { return };
    let mut app = app(font);
    app.render_once();
    app.render_once();
    let finger = |phase, x| Touch {
        id: 1,
        phase,
        position: Point::new(x, 100.0),
    };
    app.touch(finger(TouchPhase::Started, 300.0));
    for x in [280.0, 200.0, 100.0] {
        app.touch(finger(TouchPhase::Moved, x));
    }
    app.touch(finger(TouchPhase::Ended, 100.0));
    let (y, x) = offsets(&app);
    assert_eq!(y, 0.0);
    assert_eq!(x, 200.0 - crate::core::touch::SLOP);
}
