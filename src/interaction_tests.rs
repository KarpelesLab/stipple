//! Interaction states and implicit transitions, driven through an [`App`].

use std::cell::Cell;
use std::time::Duration;

use crate::App;
use crate::anim::Easing;
use crate::core::{Cx, Element, ElementKey, Interaction, LayoutNode, Transition};
use crate::geometry::{Point, Size};
use crate::layout::Axis;
use crate::platform::DragOffer;
use crate::render::Color;

const RED: Color = Color::rgb(255, 0, 0);
const GRAY: Color = Color::rgb(128, 128, 128);

fn tile() -> Element {
    Element::stack(Axis::Vertical, vec![])
        .width(100.0)
        .height(50.0)
        .fill(GRAY)
}

fn tree<S: 'static>(app: &App<S>) -> &LayoutNode {
    app.panes[0].tree.as_ref().expect("a frame was built")
}

/// The fill of the `i`th child of the root.
fn fill<S: 'static>(app: &App<S>, i: usize) -> Option<Color> {
    tree(app).children[i].decoration.fill
}

fn two_tiles(first: Element) -> App<()> {
    App::new((), move |_: &(), _: &mut Cx<()>| {
        Element::stack(Axis::Vertical, vec![first.clone(), tile()])
    })
    .logical_size(Size::new(200.0, 100.0))
}

#[test]
fn a_hover_override_restyles_and_only_its_states_rebuild() {
    let mut app = two_tiles(tile().hovered(|e| e.fill(RED)));
    let _ = app.render_frame();
    assert_eq!(fill(&app, 0), Some(GRAY));
    // Over the plain tile: nothing uses that, so nothing to rebuild.
    assert!(!app.hover_at(Point::new(10.0, 70.0)));
    // Over the styled one: rebuild, restyled.
    assert!(app.hover_at(Point::new(10.0, 10.0)));
    let _ = app.render_frame();
    assert_eq!(fill(&app, 0), Some(RED));
    // Moving within it changes nothing.
    assert!(!app.hover_at(Point::new(20.0, 20.0)));
    assert!(app.hover_at(Point::new(10.0, 70.0)));
    let _ = app.render_frame();
    assert_eq!(fill(&app, 0), Some(GRAY));
    assert!(!app.animating(), "no transition: nothing to animate");
}

#[test]
fn a_transition_animates_the_change_then_rests() {
    let mut app = two_tiles(
        tile()
            .hovered(|e| e.fill(RED))
            .transition(Transition::tween(Duration::from_secs(2), Easing::Linear)),
    );
    let _ = app.render_frame();
    let _ = app.render_frame();
    assert!(!app.animating(), "at rest: no frames");
    assert!(app.hover_at(Point::new(10.0, 10.0)));
    let _ = app.render_frame();
    assert!(app.animating(), "moving: frames");
    // (Frames come from the wall clock here; fixed frame times are tested in
    // `core::transition`.)
    std::thread::sleep(Duration::from_millis(50));
    let _ = app.render_frame();
    let mid = fill(&app, 0).expect("a fill");
    assert!(mid != GRAY && mid != RED, "between: {mid:?}");
}

#[test]
fn a_tappable_element_styling_its_own_hover_gets_no_highlight() {
    let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
        Element::stack(
            Axis::Vertical,
            vec![
                tile().hovered(|e| e.fill(RED)).on_tap(cx, |_| {}),
                tile().on_tap(cx, |_| {}),
            ],
        )
    })
    .logical_size(Size::new(200.0, 100.0));
    let _ = app.render_frame();
    assert!(app.hover_at(Point::new(10.0, 10.0)));
    let _ = app.render_frame();
    assert!(app.panes[0].highlight_alphas.is_empty(), "its own look");
    assert!(app.hover_at(Point::new(10.0, 70.0)));
    let _ = app.render_frame();
    assert!(!app.panes[0].highlight_alphas.is_empty(), "the theme's");
}

#[test]
fn the_view_reads_keyed_states_through_cx() {
    thread_local!(static SEEN: Cell<Interaction> = const { Cell::new(Interaction::NONE) });
    let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
        SEEN.with(|s| s.set(cx.interaction("card")));
        Element::stack(
            Axis::Vertical,
            vec![
                tile().key("card").on_key(cx, |_, _| {}),
                tile().on_key(cx, |_, _| {}),
            ],
        )
    })
    .logical_size(Size::new(200.0, 100.0));
    let _ = app.render_frame();
    assert!(app.hover_at(Point::new(10.0, 10.0)), "the view reads it");
    let _ = app.render_frame();
    assert!(SEEN.with(Cell::get).hovered());
    // Tab focuses it from the keyboard: focus-visible.
    assert!(app.focus_next());
    let _ = app.render_frame();
    let s = SEEN.with(Cell::get);
    assert!(s.focused() && s.focus_visible(), "{s:?}");
    // A click on the other moves focus away.
    assert!(app.click_at(Point::new(10.0, 70.0)));
    let _ = app.render_frame();
    assert!(!SEEN.with(Cell::get).focused());
    // Pressed while held over it.
    app.pointer_press(Point::new(10.0, 10.0));
    let _ = app.render_frame();
    assert!(SEEN.with(Cell::get).pressed());
    app.pointer_release(Point::new(10.0, 10.0));
    let _ = app.render_frame();
    let s = SEEN.with(Cell::get);
    assert!(!s.pressed() && s.focused() && !s.focus_visible(), "{s:?}");
}

#[test]
fn a_drop_target_styles_its_drag_over_look() {
    let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
        Element::stack(
            Axis::Vertical,
            vec![
                tile()
                    .drag_over(|e| e.fill(RED))
                    .drag_refused(|e| e.fill(Color::rgb(0, 0, 0)))
                    .on_drop_of(cx, DragOffer::FILES, |_, _, _| {}),
            ],
        )
    })
    .logical_size(Size::new(200.0, 100.0));
    let _ = app.render_frame();
    assert!(app.drag_enter(Point::new(10.0, 10.0), DragOffer::FILES));
    let _ = app.render_frame();
    assert_eq!(fill(&app, 0), Some(RED));
    assert_eq!(
        app.panes[0].dnd.highlight, None,
        "its own look, not the app's"
    );
    app.drag_leave();
    assert!(!app.drag_enter(Point::new(10.0, 10.0), DragOffer::TEXT));
    let _ = app.render_frame();
    assert_eq!(fill(&app, 0), Some(Color::rgb(0, 0, 0)), "refused");
    app.drag_leave();
    let _ = app.render_frame();
    assert_eq!(fill(&app, 0), Some(GRAY));
}

#[test]
fn a_dragged_source_knows_it() {
    let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
        Element::stack(
            Axis::Vertical,
            vec![
                tile()
                    .dragged(|e| e.radius(9.0))
                    .drag_source(cx, |_| Some(crate::platform::DropData::Text("x".into()))),
            ],
        )
    })
    .logical_size(Size::new(200.0, 100.0));
    let _ = app.render_frame();
    app.pointer_press(Point::new(10.0, 10.0));
    app.pointer_drag_to(Point::new(10.0, 30.0));
    let _ = app.render_frame();
    assert_eq!(tree(&app).children[0].decoration.radius, 9.0);
    app.pointer_release(Point::new(10.0, 30.0));
    let _ = app.render_frame();
    assert_eq!(tree(&app).children[0].decoration.radius, 0.0);
}

#[test]
fn keys_are_on_the_laid_out_tree() {
    let mut app = two_tiles(tile().key("k").transition(Transition::default()));
    let _ = app.render_frame();
    assert_eq!(tree(&app).children[0].key, Some(ElementKey::new("k")));
    assert_eq!(
        tree(&app).children[1].key,
        None,
        "plain elements carry none"
    );
}
