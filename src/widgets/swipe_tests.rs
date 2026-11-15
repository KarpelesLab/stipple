//! Swipe actions: a drag that follows and settles open, shut, or (past the
//! full-swipe line) runs the primary action, at fixed frame times; a
//! vertical drag left to the list; a trackpad's sideways scroll; and in an
//! app, a mouse, a finger and the keyboard (the context menu) reaching the
//! actions of `virtual_list` rows and plain rows.

use std::rc::Rc;
use std::time::Duration;

use super::swipe::*;
use crate::App;
use crate::anim::{Easing, Motion};
use crate::core::{
    AccessNode, Cx, Element, PointerButtons, PointerEvent, PointerEventKind, Role, WheelEvent,
};
use crate::geometry::{Point, Size, Vec2};
use crate::platform::{
    ButtonState, Event, KeyCode, Modifiers, PointerButton, ScrollDelta, ScrollPhase, ScrollUnit,
    Touch, TouchPhase,
};

const MS: Duration = Duration::from_millis(1);
const T0: Duration = Duration::from_secs(10);
/// The rows' width in the handler tests.
const W: f64 = 360.0;

#[derive(Default)]
struct State {
    swipe: SwipeState,
    log: Vec<String>,
}

fn actions() -> SwipeActions<State> {
    SwipeActions::new()
        .leading(SwipeAction::new("Pin", |s: &mut State| {
            s.log.push("pin".into())
        }))
        .trailing(
            SwipeAction::new("Delete", |s: &mut State| s.log.push("delete".into())).destructive(),
        )
        .trailing(SwipeAction::new("Archive", |s: &mut State| {
            s.log.push("archive".into())
        }))
}

/// Row 1's handler, settling on a 100 ms linear tween.
fn swiper(actions: SwipeActions<State>) -> Swiper<State> {
    Swiper {
        access: Rc::new(|s: &mut State| &mut s.swipe),
        key: hash_key(1),
        actions,
        dir: 1.0,
        motion: Motion::tween(0.1, Easing::Linear),
    }
}

fn ev(kind: PointerEventKind, x: f64, y: f64) -> PointerEvent {
    PointerEvent {
        kind,
        button: matches!(kind, PointerEventKind::Down | PointerEventKind::Up)
            .then_some(PointerButton::Left),
        buttons_held: PointerButtons::NONE,
        position: Point::new(x, y - 100.0),
        window_position: Point::new(x, y),
        size: Size::new(W, 56.0),
        modifiers: Modifiers::default(),
        click_count: 1,
        movement: Vec2::ZERO,
    }
}

/// Press at `x0`, move through `xs` (one every `dt`), release at the last;
/// returns the release time.
fn drag(
    sw: &Swiper<State>,
    s: &mut State,
    at: Duration,
    x0: f64,
    xs: &[f64],
    dt: Duration,
) -> Duration {
    sw.pointer(&ev(PointerEventKind::Down, x0, 120.0), s, at);
    let mut t = at;
    for &x in xs {
        t += dt;
        sw.pointer(&ev(PointerEventKind::Move, x, 121.0), s, t);
    }
    sw.pointer(&ev(PointerEventKind::Up, *xs.last().unwrap(), 121.0), s, t);
    t
}

fn at(s: &State, t: Duration) -> f64 {
    s.swipe.offset_of(hash_key(1), t)
}

#[test]
fn a_slow_partial_swipe_opens_on_the_buttons_and_a_short_one_springs_back() {
    crate::anim::set_reduced_motion(Some(false));
    let sw = swiper(actions());
    let mut s = State::default();
    // 100 px left, slowly: past half the two trailing buttons (74 of 148).
    sw.pointer(&ev(PointerEventKind::Down, 300.0, 120.0), &mut s, T0);
    sw.pointer(
        &ev(PointerEventKind::Move, 290.0, 121.0),
        &mut s,
        T0 + 16 * MS,
    );
    assert!(s.swipe.is_swiping());
    // It follows from where the drag left the slop: no jump.
    assert_eq!(at(&s, T0 + 16 * MS), -2.0);
    sw.pointer(
        &ev(PointerEventKind::Move, 192.0, 121.0),
        &mut s,
        T0 + 400 * MS,
    );
    assert_eq!(at(&s, T0 + 400 * MS), -100.0);
    sw.pointer(
        &ev(PointerEventKind::Up, 192.0, 121.0),
        &mut s,
        T0 + 600 * MS,
    );
    let t = T0 + 600 * MS;
    assert!(!s.swipe.is_swiping());
    let mid = at(&s, t + 50 * MS);
    assert!(mid < -100.0 && mid > -148.0, "{mid}");
    assert_eq!(at(&s, t + 101 * MS), -2.0 * SWIPE_ACTION_WIDTH);
    assert!(s.swipe.is_open(1));
    assert!(s.log.is_empty());

    // From there, a short drag back right shuts it.
    let t = drag(&sw, &mut s, t + 200 * MS, 100.0, &[110.0, 200.0], 300 * MS);
    assert_eq!(at(&s, t + 101 * MS), 0.0);
    assert!(!s.swipe.is_open(1));

    // 40 px is short of half the buttons: back to rest.
    let t = drag(&sw, &mut s, t + 200 * MS, 300.0, &[290.0, 252.0], 300 * MS);
    assert_eq!(at(&s, t + 101 * MS), 0.0);
    assert!(s.log.is_empty());
}

#[test]
fn a_quick_flick_opens_it() {
    crate::anim::set_reduced_motion(Some(false));
    let sw = swiper(actions());
    let mut s = State::default();
    // 50 px in 32 ms: the velocity carries it past half the buttons.
    let t = drag(&sw, &mut s, T0, 300.0, &[290.0, 270.0, 242.0], 16 * MS);
    assert_eq!(at(&s, t + 101 * MS), -2.0 * SWIPE_ACTION_WIDTH);
}

#[test]
fn a_full_swipe_runs_the_primary_action() {
    crate::anim::set_reduced_motion(Some(false));
    let sw = swiper(actions());
    let mut s = State::default();
    // Across 70% of the row to the left: Delete (the trailing primary)
    // runs; destructive, its row is shut at once (it's leaving).
    let t = drag(&sw, &mut s, T0, 340.0, &[330.0, 200.0, 80.0], 200 * MS);
    assert_eq!(s.log, ["delete"]);
    assert_eq!(at(&s, t), 0.0);
    assert!(!s.swipe.is_open(1));

    // To the right: Pin runs and the row springs back.
    let t = drag(
        &sw,
        &mut s,
        t + 200 * MS,
        20.0,
        &[30.0, 150.0, 280.0],
        200 * MS,
    );
    assert_eq!(s.log, ["delete", "pin"]);
    assert!(at(&s, t + 50 * MS) > 0.0);
    assert_eq!(at(&s, t + 101 * MS), 0.0);
}

#[test]
fn without_a_full_swipe_it_resists_past_the_buttons() {
    crate::anim::set_reduced_motion(Some(false));
    let sw = swiper(actions().full_swipe(false));
    let mut s = State::default();
    sw.pointer(&ev(PointerEventKind::Down, 340.0, 120.0), &mut s, T0);
    sw.pointer(
        &ev(PointerEventKind::Move, 330.0, 120.0),
        &mut s,
        T0 + 16 * MS,
    );
    // 250 px: the 148 of the buttons, then a quarter of the other 102.
    sw.pointer(
        &ev(PointerEventKind::Move, 82.0, 120.0),
        &mut s,
        T0 + 300 * MS,
    );
    assert_eq!(at(&s, T0 + 300 * MS), -(148.0 + 102.0 * 0.25));
    sw.pointer(
        &ev(PointerEventKind::Up, 82.0, 120.0),
        &mut s,
        T0 + 500 * MS,
    );
    assert!(s.log.is_empty());
    assert_eq!(at(&s, T0 + 601 * MS), -148.0);
}

#[test]
fn a_side_without_actions_doesnt_move() {
    let sw = swiper(SwipeActions::new().trailing(SwipeAction::new("Delete", |_: &mut State| {})));
    let mut s = State::default();
    sw.pointer(&ev(PointerEventKind::Down, 100.0, 120.0), &mut s, T0);
    sw.pointer(
        &ev(PointerEventKind::Move, 200.0, 120.0),
        &mut s,
        T0 + 16 * MS,
    );
    assert_eq!(at(&s, T0 + 16 * MS), 0.0);
}

#[test]
fn a_vertical_drag_is_left_to_the_list() {
    let sw = swiper(actions());
    let mut s = State::default();
    sw.pointer(&ev(PointerEventKind::Down, 300.0, 120.0), &mut s, T0);
    // Mostly down: a scroll. Moving sideways afterwards changes nothing.
    sw.pointer(
        &ev(PointerEventKind::Move, 296.0, 140.0),
        &mut s,
        T0 + 16 * MS,
    );
    sw.pointer(
        &ev(PointerEventKind::Move, 150.0, 150.0),
        &mut s,
        T0 + 32 * MS,
    );
    assert!(!s.swipe.is_swiping());
    assert_eq!(at(&s, T0 + 32 * MS), 0.0);
    sw.pointer(
        &ev(PointerEventKind::Up, 150.0, 150.0),
        &mut s,
        T0 + 48 * MS,
    );

    // A sideways move the touch recognizer took for a scroll (a sideways
    // strip around the row) doesn't swipe either.
    crate::core::touch::set_gesture(false, true);
    sw.pointer(&ev(PointerEventKind::Down, 300.0, 120.0), &mut s, T0);
    sw.pointer(
        &ev(PointerEventKind::Move, 250.0, 120.0),
        &mut s,
        T0 + 16 * MS,
    );
    crate::core::touch::set_gesture(false, false);
    assert!(!s.swipe.is_swiping());
}

#[test]
fn pressing_another_row_shuts_the_open_one() {
    crate::anim::set_reduced_motion(Some(false));
    let sw = swiper(actions());
    let other = Swiper {
        key: hash_key(2),
        ..swiper(actions())
    };
    let mut s = State::default();
    let t = drag(&sw, &mut s, T0, 300.0, &[290.0, 192.0], 300 * MS) + 200 * MS;
    assert!(s.swipe.is_open(1));
    other.pointer(&ev(PointerEventKind::Down, 50.0, 200.0), &mut s, t);
    assert!(!s.swipe.is_open(1));
    // Row 1 springs shut while row 2 is pressed.
    let mid = at(&s, t + 50 * MS);
    assert!(mid < 0.0 && mid > -148.0, "{mid}");
    assert_eq!(at(&s, t + 101 * MS), 0.0);
}

fn wheel(dx: f64, unit: ScrollUnit, phase: ScrollPhase) -> WheelEvent {
    WheelEvent {
        dx,
        dy: 0.0,
        unit,
        phase,
        position: Point::new(10.0, 10.0),
        window_position: Point::new(10.0, 110.0),
        modifiers: Modifiers::default(),
    }
}

#[test]
fn a_trackpad_swipe_opens_and_a_wheel_notch_steps() {
    crate::anim::set_reduced_motion(Some(false));
    let sw = swiper(actions());
    let mut s = State::default();
    // Two fingers right, 90 px: the content follows left, toward the
    // trailing buttons; the fingers lift and it opens on them.
    sw.wheel(
        &wheel(30.0, ScrollUnit::Pixels, ScrollPhase::Started),
        &mut s,
        T0,
    );
    sw.wheel(
        &wheel(30.0, ScrollUnit::Pixels, ScrollPhase::Moved),
        &mut s,
        T0 + 16 * MS,
    );
    sw.wheel(
        &wheel(30.0, ScrollUnit::Pixels, ScrollPhase::Moved),
        &mut s,
        T0 + 32 * MS,
    );
    assert!(s.swipe.is_swiping());
    assert_eq!(at(&s, T0 + 32 * MS), -90.0);
    sw.wheel(
        &wheel(0.0, ScrollUnit::Pixels, ScrollPhase::Ended),
        &mut s,
        T0 + 300 * MS,
    );
    assert_eq!(at(&s, T0 + 401 * MS), -148.0);
    // Its momentum doesn't move it.
    sw.wheel(
        &wheel(20.0, ScrollUnit::Pixels, ScrollPhase::Momentum),
        &mut s,
        T0 + 420 * MS,
    );
    assert_eq!(at(&s, T0 + 420 * MS), -148.0);

    // A tilt wheel notch back shuts it; the next one, too soon after, is
    // the same flick; later it opens the leading side.
    let t = T0 + Duration::from_secs(1);
    sw.wheel(
        &wheel(-1.0, ScrollUnit::Lines, ScrollPhase::None),
        &mut s,
        t,
    );
    assert_eq!(at(&s, t + 101 * MS), 0.0);
    sw.wheel(
        &wheel(-1.0, ScrollUnit::Lines, ScrollPhase::None),
        &mut s,
        t + 150 * MS,
    );
    assert_eq!(at(&s, t + 260 * MS), 0.0);
    sw.wheel(
        &wheel(-1.0, ScrollUnit::Lines, ScrollPhase::None),
        &mut s,
        t + 500 * MS,
    );
    assert_eq!(at(&s, t + 601 * MS), SWIPE_ACTION_WIDTH);
    assert!(s.log.is_empty());
}

// --- In an app ------------------------------------------------------------

struct Notes {
    items: Vec<u32>,
    list: super::ListState,
    swipe: SwipeState,
    log: Vec<String>,
}

const ROW: f64 = 50.0;
const AW: f64 = 360.0;

fn notes_view(s: &Notes, cx: &mut Cx<Notes>) -> Element {
    let list = super::virtual_list(
        cx,
        &s.list,
        |s| &mut s.list,
        super::VirtualList::fixed(s.items.len(), ROW).height(400.0),
        |cx, row| {
            let id = s.items[row.index];
            let actions = SwipeActions::new()
                .trailing(
                    SwipeAction::new("Delete", move |s: &mut Notes| {
                        s.items.retain(|&i| i != id);
                        s.log.push(format!("delete {id}"));
                    })
                    .destructive(),
                )
                .leading(SwipeAction::new("Pin", move |s: &mut Notes| {
                    s.log.push(format!("pin {id}"))
                }));
            swipe_row(cx, &s.swipe, |s| &mut s.swipe, id, actions)
                .child(super::label(cx, format!("Note {id}")))
        },
    );
    super::column(vec![list.width(AW)])
}

fn notes() -> App<Notes> {
    crate::anim::set_reduced_motion(Some(true));
    let mut app = App::new(
        Notes {
            items: (0..30).collect(),
            list: super::ListState::new(super::SelectionMode::Single),
            swipe: SwipeState::new(),
            log: Vec::new(),
        },
        notes_view,
    )
    .no_font()
    .logical_size(Size::new(AW, 420.0));
    app.render_once();
    app
}

/// The middle of row `i`'s height (window y).
fn row_y(i: usize) -> f64 {
    1.0 + ROW * i as f64 + ROW / 2.0
}

fn find(app: &App<Notes>, role: Role, name: &str) -> Option<AccessNode> {
    let a = app.accessibility_tree().unwrap();
    a.descendants()
        .into_iter()
        .find(|n| n.role == role && n.name == name)
        .cloned()
}

fn mouse(app: &mut App<Notes>, pressed: Option<bool>, x: f64, y: f64) {
    let position = Point::new(x, y);
    app.pointer_event(Event::PointerMoved { position });
    if let Some(pressed) = pressed {
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
    app.render_once();
}

#[test]
fn a_mouse_drag_reveals_the_buttons_and_a_tap_on_one_runs_it() {
    let mut app = notes();
    let y = row_y(2);
    mouse(&mut app, Some(true), 300.0, y);
    mouse(&mut app, None, 280.0, y);
    mouse(&mut app, None, 180.0, y);
    assert!(app.state().swipe.is_swiping());
    mouse(&mut app, Some(false), 180.0, y);
    // Open on its button; the drag selected nothing.
    assert!(app.state().swipe.is_open(2));
    assert_eq!(app.state().list.selected_count(), 0);
    // The button is at the trailing edge, as wide as the row moved.
    assert!(app.click_at(Point::new(AW - 1.0 - SWIPE_ACTION_WIDTH / 2.0, y)));
    app.render_once();
    assert_eq!(app.state().log, ["delete 2"]);
    assert!(app.find_text("Note 2").is_none());
    // The row below took its place.
    let next = app.find_text("Note 3").unwrap();
    assert!(
        (next.bounds.center().y - y).abs() < 1.0,
        "{:?}",
        next.bounds
    );
    crate::anim::set_reduced_motion(None);
}

#[test]
fn a_tap_on_an_open_row_shuts_it_instead_of_selecting() {
    let mut app = notes();
    let y = row_y(1);
    mouse(&mut app, Some(true), 300.0, y);
    mouse(&mut app, None, 280.0, y);
    mouse(&mut app, None, 180.0, y);
    mouse(&mut app, Some(false), 180.0, y);
    assert!(app.state().swipe.is_open(1));
    assert!(app.click_at(Point::new(60.0, y)));
    app.render_once();
    assert!(!app.state().swipe.is_open(1));
    assert_eq!(app.state().list.selected_count(), 0);
    // Shut, a tap selects as usual.
    assert!(app.click_at(Point::new(60.0, y)));
    app.render_once();
    assert!(app.state().list.is_selected(1));
    crate::anim::set_reduced_motion(None);
}

fn finger(app: &mut App<Notes>, phase: TouchPhase, x: f64, y: f64) {
    app.touch(Touch::new(1, phase, Point::new(x, y)));
    app.render_once();
}

#[test]
fn a_finger_swipes_sideways_and_scrolls_up_and_down() {
    let mut app = notes();
    let y = row_y(3);
    // Sideways past the full-swipe line: Delete runs.
    finger(&mut app, TouchPhase::Started, 340.0, y);
    finger(&mut app, TouchPhase::Moved, 320.0, y + 2.0);
    finger(&mut app, TouchPhase::Moved, 200.0, y + 4.0);
    finger(&mut app, TouchPhase::Moved, 60.0, y + 4.0);
    finger(&mut app, TouchPhase::Ended, 60.0, y + 4.0);
    assert_eq!(app.state().log, ["delete 3"]);
    assert_eq!(app.state().list.selected_count(), 0, "the swipe didn't tap");

    // Up the list: it scrolls, and nothing swipes.
    finger(&mut app, TouchPhase::Started, 200.0, row_y(6));
    finger(&mut app, TouchPhase::Moved, 196.0, row_y(6) - 30.0);
    finger(&mut app, TouchPhase::Moved, 150.0, row_y(6) - 120.0);
    assert!(!app.state().swipe.is_swiping());
    finger(&mut app, TouchPhase::Ended, 150.0, row_y(6) - 120.0);
    let scrolled = app.panes[0].scroll_offsets.values().any(|&o| o > 50.0);
    assert!(scrolled, "{:?}", app.panes[0].scroll_offsets);
    assert_eq!(app.state().log, ["delete 3"]);
    crate::anim::set_reduced_motion(None);
}

#[test]
fn a_trackpad_swipe_reaches_the_row_and_a_vertical_scroll_the_list() {
    let mut app = notes();
    mouse(&mut app, None, 200.0, row_y(1));
    let swipe = ScrollDelta::pixels(-40.0, 1.0).phase(ScrollPhase::Started);
    assert!(app.pointer_event(Event::Scroll { delta: swipe }));
    app.render_once();
    assert!(app.state().swipe.is_swiping());
    let end = ScrollDelta::pixels(0.0, 0.0).phase(ScrollPhase::Ended);
    app.pointer_event(Event::Scroll { delta: end });
    app.render_once();
    // A scroll toward the start (fingers moving right) brings the leading
    // Pin out.
    assert!(app.state().swipe.is_open(1));
    assert!(app.click_at(Point::new(1.0 + SWIPE_ACTION_WIDTH / 2.0, row_y(1))));
    app.render_once();
    assert_eq!(app.state().log, ["pin 1"]);
    assert!(!app.state().swipe.is_open(1));
    // A vertical scroll isn't the row's: the list takes it.
    let down = ScrollDelta::pixels(2.0, 60.0).phase(ScrollPhase::Started);
    app.pointer_event(Event::Scroll { delta: down });
    app.render_once();
    assert!(!app.state().swipe.is_swiping());
    let scrolled = app.panes[0].scroll_offsets.values().any(|&o| o > 50.0);
    assert!(scrolled, "{:?}", app.panes[0].scroll_offsets);
    crate::anim::set_reduced_motion(None);
}

#[test]
fn the_actions_are_in_the_context_menu_for_the_keyboard() {
    let mut app = notes();
    // Focus the list on row 4, then move the pointer off the rows: the Menu
    // key opens the cursor row's actions.
    assert!(app.click_at(Point::new(60.0, row_y(4))));
    app.render_once();
    app.hover_at(Point::new(10.0, 415.0));
    assert!(app.press_key_code(KeyCode::ContextMenu, Modifiers::default()));
    app.render_once();
    let names: Vec<String> = app
        .accessibility_tree()
        .unwrap()
        .descendants()
        .into_iter()
        .filter(|n| n.role == Role::MenuItem)
        .map(|n| n.name.clone())
        .collect();
    assert_eq!(names, ["Pin", "Delete"]);
    let delete = find(&app, Role::MenuItem, "Delete").unwrap();
    assert!(app.click_at(delete.bounds.center()));
    app.render_once();
    assert_eq!(app.state().log, ["delete 4"]);
    // A right-click on a row opens its own.
    assert!(app.right_click_at(Point::new(60.0, row_y(0))));
    app.render_once();
    assert!(find(&app, Role::MenuItem, "Pin").is_some());
    crate::anim::set_reduced_motion(None);
}

/// Plain rows in a `scroll`, each a button: the Menu key on a focused row's
/// button opens the row's actions (the nearest context menu around it).
#[test]
fn plain_rows_reach_their_actions_from_a_focused_button() {
    crate::anim::set_reduced_motion(Some(true));
    let mut app = App::new(
        Notes {
            items: (0..5).collect(),
            list: super::ListState::new(super::SelectionMode::Single),
            swipe: SwipeState::new(),
            log: Vec::new(),
        },
        |s: &Notes, cx: &mut Cx<Notes>| {
            let rows = s
                .items
                .iter()
                .map(|&id| {
                    let actions = SwipeActions::new()
                        .trailing(SwipeAction::new("Archive", move |s: &mut Notes| {
                            s.log.push(format!("archive {id}"))
                        }));
                    swipe_row(cx, &s.swipe, |s| &mut s.swipe, id, actions).child(
                        super::button(cx, format!("Open {id}"))
                            .on_tap(cx, move |s| s.log.push(format!("open {id}"))),
                    )
                })
                .collect();
            super::scroll(cx).children(rows)
        },
    )
    .no_font()
    .logical_size(Size::new(AW, 420.0));
    app.render_once();
    assert!(app.focus_next());
    assert!(app.focus_next());
    app.render_once();
    app.hover_at(Point::new(-50.0, -50.0));
    assert!(app.press_key_code(KeyCode::ContextMenu, Modifiers::default()));
    app.render_once();
    let item = find(&app, Role::MenuItem, "Archive").expect("the row's action");
    assert!(app.click_at(item.bounds.center()));
    assert_eq!(app.state().log, ["archive 1"]);
    crate::anim::set_reduced_motion(None);
}
