//! The bottom sheet: its detents, a drag that follows the pointer and
//! settles, springs back or closes (by position, and by a fling's velocity
//! at fixed frame times), a touch pull on its content handed over from the
//! scroll, and the scrim, Escape and Back closing it, before a page stack
//! under it pops.

use std::rc::Rc;
use std::time::Duration;

use super::sheet::*;
use crate::App;
use crate::anim::{Easing, Motion};
use crate::core::{
    AccessNode, Anchor, Cx, Element, Handled, KeyInput, PointerButtons, PointerEvent,
    PointerEventKind, Role,
};
use crate::geometry::{Point, Size, Vec2};
use crate::platform::{ButtonState, Event, KeyCode, Modifiers, PointerButton, Touch, TouchPhase};
use crate::style::{MotionTokens, Theme};

const MS: Duration = Duration::from_millis(1);
const T0: Duration = Duration::from_secs(10);

#[derive(Default)]
struct State {
    sheet: BottomSheet,
    log: Vec<String>,
    wide: bool,
}

const W: f64 = 360.0;
const H: f64 = 640.0;
/// The half detent's top edge.
const HALF_TOP: f64 = H / 2.0;

fn view(s: &State, cx: &mut Cx<State>) -> Element {
    let rows = (0..30)
        .map(|i| {
            super::button(cx, format!("Row {i}"))
                .height(50.0)
                .on_tap(cx, move |s| s.log.push(format!("row {i}")))
        })
        .collect();
    bottom_sheet(cx, &s.sheet, |s| &mut s.sheet)
        .title("Result")
        .detents(vec![SheetDetent::Half, SheetDetent::Full])
        .adaptive(s.wide)
        .children(rows)
        .on_dismiss(|s| s.log.push("dismissed".into()))
        .open(cx);
    super::button(cx, "Show")
        .on_tap(cx, |s| s.sheet.open())
        .height(40.0)
}

fn app() -> App<State> {
    crate::anim::set_reduced_motion(Some(true));
    let mut state = State::default();
    state.sheet.open();
    let mut app = App::new(state, view)
        .no_font()
        .logical_size(Size::new(W, H));
    app.render_once();
    app
}

fn button(app: &mut App<State>, pressed: bool, y: f64) {
    let position = Point::new(W / 2.0, y);
    app.pointer_event(Event::PointerMoved { position });
    app.pointer_event(Event::PointerButton {
        button: PointerButton::Left,
        state: if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        },
        position,
    });
}

/// Press at `from`, move to each of `to` and release at the last.
fn mouse_drag(app: &mut App<State>, from: f64, to: &[f64]) {
    button(app, true, from);
    for &y in to {
        app.pointer_event(Event::PointerMoved {
            position: Point::new(W / 2.0, y),
        });
        app.render_once();
    }
    button(app, false, *to.last().unwrap());
    app.render_once();
}

fn find(node: &AccessNode, role: Role) -> Option<AccessNode> {
    if node.role == role {
        return Some(node.clone());
    }
    node.children.iter().find_map(|c| find(c, role))
}

#[test]
fn it_rests_on_its_first_detent_as_a_dialog() {
    let app = app();
    let dialog = find(&app.accessibility_tree().unwrap(), Role::Dialog).expect("a dialog");
    assert_eq!(dialog.name, "Result");
    assert_eq!(dialog.bounds.min_y(), HALF_TOP);
    assert_eq!(dialog.bounds.height(), H / 2.0);
    assert!(app.wants_back());
    crate::anim::set_reduced_motion(None);
}

#[test]
fn a_drag_on_the_handle_follows_and_springs_back() {
    let mut app = app();
    button(&mut app, true, HALF_TOP + 20.0);
    app.pointer_event(Event::PointerMoved {
        position: Point::new(W / 2.0, HALF_TOP + 90.0),
    });
    assert!(app.state().sheet.is_dragging());
    app.render_once();
    button(&mut app, false, HALF_TOP + 90.0);
    app.render_once();
    assert!(app.state().sheet.is_open());
    assert_eq!(app.state().sheet.detent(), 0);
    let dialog = find(&app.accessibility_tree().unwrap(), Role::Dialog).unwrap();
    assert_eq!(dialog.bounds.min_y(), HALF_TOP);
    crate::anim::set_reduced_motion(None);
}

#[test]
fn dragged_down_far_it_closes() {
    let mut app = app();
    mouse_drag(&mut app, HALF_TOP + 20.0, &[HALF_TOP + 60.0, H - 20.0]);
    assert!(!app.state().sheet.is_open());
    assert_eq!(app.state().log, ["dismissed"]);
    assert!(find(&app.accessibility_tree().unwrap(), Role::Dialog).is_none());
    crate::anim::set_reduced_motion(None);
}

#[test]
fn dragged_up_it_rests_on_the_next_detent() {
    let mut app = app();
    mouse_drag(&mut app, HALF_TOP + 20.0, &[HALF_TOP, 60.0]);
    assert_eq!(app.state().sheet.detent(), 1);
    let dialog = find(&app.accessibility_tree().unwrap(), Role::Dialog).unwrap();
    assert_eq!(
        dialog.bounds.min_y(),
        48.0,
        "the full detent leaves a strip"
    );
    crate::anim::set_reduced_motion(None);
}

#[test]
fn a_mouse_on_the_content_doesnt_drag_it() {
    let mut app = app();
    mouse_drag(&mut app, HALF_TOP + 150.0, &[HALF_TOP + 160.0, H - 10.0]);
    assert!(app.state().sheet.is_open());
    assert_eq!(app.state().sheet.detent(), 0);
    assert!(app.state().log.is_empty(), "{:?}", app.state().log);
    crate::anim::set_reduced_motion(None);
}

fn finger(app: &mut App<State>, phase: TouchPhase, y: f64) {
    app.touch(Touch::new(1, phase, Point::new(W / 2.0, y)));
    app.render_once();
}

#[test]
fn a_finger_pulling_the_content_down_at_its_top_drags_the_sheet() {
    let mut app = app();
    let y = HALF_TOP + 80.0;
    finger(&mut app, TouchPhase::Started, y);
    finger(&mut app, TouchPhase::Moved, y + 20.0);
    assert!(app.state().sheet.is_dragging());
    finger(&mut app, TouchPhase::Moved, H - 10.0);
    finger(&mut app, TouchPhase::Ended, H - 10.0);
    assert!(!app.state().sheet.is_open());
    // The row it landed on didn't tap.
    assert_eq!(app.state().log, ["dismissed"]);
    crate::anim::set_reduced_motion(None);
}

#[test]
fn a_finger_pushing_the_content_up_scrolls_it() {
    let mut app = app();
    let y = HALF_TOP + 250.0;
    finger(&mut app, TouchPhase::Started, y);
    finger(&mut app, TouchPhase::Moved, y - 30.0);
    finger(&mut app, TouchPhase::Moved, y - 120.0);
    assert!(!app.state().sheet.is_dragging());
    finger(&mut app, TouchPhase::Ended, y - 120.0);
    assert!(app.state().sheet.is_open());
    let scrolled = app.panes[0].scroll_offsets.values().any(|&o| o > 50.0);
    assert!(scrolled, "{:?}", app.panes[0].scroll_offsets);
    // Scrolled down, a pull scrolls back rather than dragging the sheet.
    finger(&mut app, TouchPhase::Started, y - 100.0);
    finger(&mut app, TouchPhase::Moved, y - 60.0);
    assert!(!app.state().sheet.is_dragging());
    finger(&mut app, TouchPhase::Ended, y - 60.0);
    assert!(app.state().log.is_empty(), "{:?}", app.state().log);
    crate::anim::set_reduced_motion(None);
}

#[test]
fn the_scrim_escape_and_back_close_it() {
    let mut app = app();
    assert!(app.click_at(Point::new(W / 2.0, 100.0)));
    assert!(!app.state().sheet.is_open());
    app.render_once();

    for close in 0..2 {
        app.handle().update(|s: &mut State| s.sheet.open());
        app.run_pending();
        app.render_once();
        assert!(app.state().sheet.is_open());
        if close == 0 {
            assert!(app.press_key_code(KeyCode::Escape, Modifiers::default()));
        } else {
            assert!(app.request_back());
        }
        assert!(!app.state().sheet.is_open());
        app.render_once();
    }
    assert_eq!(app.state().log, ["dismissed"; 3]);
    let _ = KeyInput::Escape;
    crate::anim::set_reduced_motion(None);
}

#[test]
fn back_closes_the_sheet_before_the_stack_under_it_pops() {
    struct Both {
        nav: super::NavStack<u32>,
        sheet: BottomSheet,
    }
    crate::anim::set_reduced_motion(Some(true));
    let mut nav = super::NavStack::new(0);
    nav.push(1);
    let mut sheet = BottomSheet::new();
    sheet.open();
    let mut app = App::new(Both { nav, sheet }, |s: &Both, cx: &mut Cx<Both>| {
        super::nav_stack(
            cx,
            &s.nav,
            |s| &mut s.nav,
            |cx, p| {
                // The page opens the sheet: declared after the stack's Back.
                bottom_sheet(cx, &s.sheet, |s| &mut s.sheet)
                    .child(super::label(cx, "In the sheet"))
                    .open(cx);
                super::label(cx, format!("Page {p}"))
            },
        )
    })
    .no_font()
    .on_back(|_| Handled::No)
    .logical_size(Size::new(W, H));
    app.render_once();
    assert!(app.request_back());
    assert!(!app.state().sheet.is_open());
    assert_eq!(app.state().nav.depth(), 2);
    app.render_once();
    assert!(app.request_back());
    assert_eq!(app.state().nav.depth(), 1);
    crate::anim::set_reduced_motion(None);
}

#[test]
fn on_a_wide_window_it_is_a_centered_dialog() {
    let theme = Theme::light();
    let mut cx = Cx::<State>::new(&theme);
    cx.set_window_size(Size::new(900.0, 700.0));
    let mut state = State {
        wide: true,
        ..State::default()
    };
    state.sheet.open();
    view(&state, &mut cx);
    let overlays = cx.take_overlays();
    assert_eq!(overlays.len(), 1);
    assert_eq!(overlays[0].anchor, Anchor::Center);
    assert!(overlays[0].modal);
    // Narrow, a sheet along the bottom; closed, nothing.
    cx.set_window_size(Size::new(W, H));
    view(&state, &mut cx);
    assert!(matches!(
        cx.take_overlays()[0].anchor,
        Anchor::Aligned { .. }
    ));
    state.sheet.dismiss();
    view(&state, &mut cx);
    assert!(cx.take_overlays().is_empty());
}

/// A sheet on a half and a full detent in a 600 px window (the full one
/// 552 tall), its settling a 100 ms linear tween.
fn dragger() -> Dragger<State> {
    Dragger {
        access: Rc::new(|s: &mut State| &mut s.sheet),
        close: Rc::new(|s: &mut State| {
            s.sheet.dismiss();
            s.log.push("closed".into());
        }),
        detents: vec![SheetDetent::Half, SheetDetent::Full],
        window: 600.0,
        max: 552.0,
        header: 48.0,
        motion: Motion::tween(0.1, Easing::Linear),
    }
}

/// A pointer event at window `y` on a sheet `height` tall whose top is at
/// `top`.
fn ev(kind: PointerEventKind, y: f64, top: f64, height: f64) -> PointerEvent {
    PointerEvent {
        kind,
        button: (kind != PointerEventKind::Move).then_some(PointerButton::Left),
        buttons_held: PointerButtons::NONE,
        position: Point::new(10.0, y - top),
        window_position: Point::new(10.0, y),
        size: Size::new(W, height),
        modifiers: Modifiers::default(),
        click_count: 1,
        movement: Vec2::ZERO,
    }
}

/// The sheet's height as `view` builds it at `now`.
fn built_height(s: &State, now: Duration) -> f64 {
    let theme = Theme::light().motion(MotionTokens::standard());
    let mut cx = Cx::<State>::new(&theme);
    cx.set_frame_time(now);
    cx.set_window_size(Size::new(W, 600.0));
    bottom_sheet(&mut cx, &s.sheet, |s| &mut s.sheet)
        .detents(vec![SheetDetent::Half, SheetDetent::Full])
        .open(&mut cx);
    let o = cx.take_overlays();
    // Below its lowest detent it keeps that height and slides down.
    let dy = o[0].content.transform.map_or(0.0, |t| t.translate.dy);
    crate::core::measure(&o[0].content, Size::new(W, 600.0), None).height - dy
}

#[test]
fn a_slow_release_settles_back_and_a_fling_closes() {
    crate::anim::set_reduced_motion(Some(false));
    let d = dragger();
    let mut s = State::default();
    s.sheet.open();
    // Dragged 90 px down slowly: it heads back up to the half detent.
    d.pointer(&ev(PointerEventKind::Down, 310.0, 300.0, 300.0), &mut s, T0);
    d.pointer(
        &ev(PointerEventKind::Move, 320.0, 300.0, 300.0),
        &mut s,
        T0 + 16 * MS,
    );
    d.pointer(
        &ev(PointerEventKind::Move, 410.0, 300.0, 300.0),
        &mut s,
        T0 + 500 * MS,
    );
    d.pointer(
        &ev(PointerEventKind::Up, 410.0, 300.0, 300.0),
        &mut s,
        T0 + 700 * MS,
    );
    assert!(s.sheet.is_open());
    assert_eq!(s.sheet.detent(), 0);
    let t1 = T0 + 700 * MS;
    assert_eq!(built_height(&s, t1), 210.0);
    let mid = built_height(&s, t1 + 50 * MS);
    assert!(mid > 210.0 && mid < 300.0, "{mid}");
    assert_eq!(built_height(&s, t1 + 101 * MS), 300.0);

    // The same 90 px flung: its velocity carries it under half the lowest
    // detent, so it closes.
    let t2 = t1 + 200 * MS;
    d.pointer(&ev(PointerEventKind::Down, 310.0, 300.0, 300.0), &mut s, t2);
    d.pointer(
        &ev(PointerEventKind::Move, 320.0, 300.0, 300.0),
        &mut s,
        t2 + 16 * MS,
    );
    d.pointer(
        &ev(PointerEventKind::Move, 410.0, 300.0, 300.0),
        &mut s,
        t2 + 48 * MS,
    );
    d.pointer(
        &ev(PointerEventKind::Up, 410.0, 300.0, 300.0),
        &mut s,
        t2 + 48 * MS,
    );
    assert!(!s.sheet.is_open());
    assert_eq!(s.log, ["closed"]);
}

#[test]
fn a_fling_up_goes_to_the_full_detent_and_past_it_resists() {
    crate::anim::set_reduced_motion(Some(false));
    let d = dragger();
    let mut s = State::default();
    s.sheet.open();
    d.pointer(&ev(PointerEventKind::Down, 310.0, 300.0, 300.0), &mut s, T0);
    d.pointer(
        &ev(PointerEventKind::Move, 300.0, 300.0, 300.0),
        &mut s,
        T0 + 16 * MS,
    );
    d.pointer(
        &ev(PointerEventKind::Move, 240.0, 300.0, 300.0),
        &mut s,
        T0 + 48 * MS,
    );
    d.pointer(
        &ev(PointerEventKind::Up, 240.0, 300.0, 300.0),
        &mut s,
        T0 + 48 * MS,
    );
    assert_eq!(s.sheet.detent(), 1);
    assert_eq!(built_height(&s, T0 + 200 * MS), 552.0);
    // Dragged 100 px past the tallest, it goes 20.
    let t1 = T0 + 300 * MS;
    d.pointer(&ev(PointerEventKind::Down, 60.0, 48.0, 552.0), &mut s, t1);
    d.pointer(
        &ev(PointerEventKind::Move, 50.0, 48.0, 552.0),
        &mut s,
        t1 + 16 * MS,
    );
    d.pointer(
        &ev(PointerEventKind::Move, -50.0, 48.0, 552.0),
        &mut s,
        t1 + 300 * MS,
    );
    assert_eq!(built_height(&s, t1 + 300 * MS), 572.0);
}

#[test]
fn a_press_on_the_content_with_a_mouse_doesnt_drag() {
    crate::anim::set_reduced_motion(Some(false));
    let d = dragger();
    let mut s = State::default();
    s.sheet.open();
    d.pointer(&ev(PointerEventKind::Down, 450.0, 300.0, 300.0), &mut s, T0);
    d.pointer(
        &ev(PointerEventKind::Move, 560.0, 300.0, 300.0),
        &mut s,
        T0 + 16 * MS,
    );
    assert!(!s.sheet.is_dragging());
    d.pointer(
        &ev(PointerEventKind::Up, 560.0, 300.0, 300.0),
        &mut s,
        T0 + 32 * MS,
    );
    assert!(s.sheet.is_open());
}
