//! Pull to refresh: a pull handed over by the touch recognizer follows the
//! finger (less and less), refreshes past the threshold and stays until the
//! app finishes, at fixed frame times; in an app, a finger pulling a list at
//! its top (over swipeable rows too), a list scrolled down scrolling back
//! instead, a mouse not pulling, and the keyboard's Refresh.

use std::rc::Rc;
use std::time::Duration;

use super::refresh::*;
use crate::App;
use crate::anim::{Easing, Motion};
use crate::core::{AccessNode, Cx, Element, PointerButtons, PointerEvent, PointerEventKind, Role};
use crate::geometry::{Point, Size, Vec2};
use crate::platform::{ButtonState, Event, KeyCode, Modifiers, PointerButton, Touch, TouchPhase};

const MS: Duration = Duration::from_millis(1);
const T0: Duration = Duration::from_secs(10);

#[derive(Default)]
struct State {
    refresh: RefreshState,
    refreshed: u32,
}

fn puller() -> Puller<State> {
    Puller {
        access: Rc::new(|s: &mut State| &mut s.refresh),
        on_refresh: Rc::new(|s: &mut State| s.refreshed += 1),
        motion: Motion::tween(0.1, Easing::Linear),
    }
}

fn ev(kind: PointerEventKind, y: f64) -> PointerEvent {
    PointerEvent {
        kind,
        button: matches!(kind, PointerEventKind::Down | PointerEventKind::Up)
            .then_some(PointerButton::Left),
        buttons_held: PointerButtons::NONE,
        position: Point::new(100.0, y),
        window_position: Point::new(100.0, y),
        size: Size::new(360.0, 400.0),
        modifiers: Modifiers::default(),
        click_count: 1,
        movement: Vec2::ZERO,
    }
}

/// How far down the indicator is, as `pull_to_refresh` builds it at `t`
/// (the Material look: the disc's top is 48 px above it).
fn pulled(s: &State, t: Duration) -> f64 {
    let theme = crate::style::Theme::light();
    let mut cx = Cx::<State>::new(&theme);
    cx.set_frame_time(t);
    let el = pull_to_refresh(&mut cx, &s.refresh, |s| &mut s.refresh, |_| {})
        .child(Element::boxed(Default::default()).height(400.0));
    let crate::core::ElementKind::Layers { children, .. } = &el.kind else {
        panic!("layers");
    };
    match children.get(1) {
        Some(over) => over.transform.map_or(0.0, |t| t.translate.dy) + 48.0,
        None => 0.0,
    }
}

#[test]
fn a_pull_past_the_threshold_refreshes_until_the_app_finishes() {
    crate::anim::set_reduced_motion(Some(false));
    let p = puller();
    let mut s = State::default();
    crate::core::touch::set_gesture(true, false);
    p.pointer(&ev(PointerEventKind::Down, 100.0), &mut s, T0);
    p.pointer(&ev(PointerEventKind::Move, 110.0), &mut s, T0 + 16 * MS);
    assert!(s.refresh.is_pulling());
    // 50 px down: it follows, a little less than all of it.
    p.pointer(&ev(PointerEventKind::Move, 160.0), &mut s, T0 + 100 * MS);
    let at50 = pulled(&s, T0 + 100 * MS);
    assert!(at50 > 40.0 && at50 < 50.0, "{at50}");
    // 120 px: past the threshold, but well short of 120.
    p.pointer(&ev(PointerEventKind::Move, 230.0), &mut s, T0 + 300 * MS);
    let at120 = pulled(&s, T0 + 300 * MS);
    assert!(at120 > REFRESH_THRESHOLD && at120 < 100.0, "{at120}");
    p.pointer(&ev(PointerEventKind::Up, 230.0), &mut s, T0 + 400 * MS);
    crate::core::touch::set_gesture(false, false);
    assert!(s.refresh.is_refreshing());
    assert_eq!(s.refreshed, 1);
    // It settles where it spins, and stays there.
    let t = T0 + 400 * MS;
    assert_eq!(pulled(&s, t + 101 * MS), REFRESH_THRESHOLD);
    assert_eq!(pulled(&s, t + Duration::from_secs(5)), REFRESH_THRESHOLD);
    s.refresh.finish();
    assert!(!s.refresh.is_refreshing());
}

#[test]
fn a_short_pull_goes_back_up() {
    crate::anim::set_reduced_motion(Some(false));
    let p = puller();
    let mut s = State::default();
    crate::core::touch::set_gesture(true, false);
    p.pointer(&ev(PointerEventKind::Down, 100.0), &mut s, T0);
    p.pointer(&ev(PointerEventKind::Move, 110.0), &mut s, T0 + 16 * MS);
    p.pointer(&ev(PointerEventKind::Move, 150.0), &mut s, T0 + 200 * MS);
    p.pointer(&ev(PointerEventKind::Up, 150.0), &mut s, T0 + 300 * MS);
    crate::core::touch::set_gesture(false, false);
    assert!(!s.refresh.is_refreshing());
    assert_eq!(s.refreshed, 0);
    let mid = pulled(&s, T0 + 350 * MS);
    assert!(mid > 0.0 && mid < 40.0, "{mid}");
    assert_eq!(pulled(&s, T0 + 401 * MS), 0.0);
}

#[test]
fn a_mouse_drag_doesnt_pull() {
    let p = puller();
    let mut s = State::default();
    p.pointer(&ev(PointerEventKind::Down, 100.0), &mut s, T0);
    p.pointer(&ev(PointerEventKind::Move, 300.0), &mut s, T0 + 16 * MS);
    assert!(!s.refresh.is_pulling());
    p.pointer(&ev(PointerEventKind::Up, 300.0), &mut s, T0 + 32 * MS);
    assert_eq!(s.refreshed, 0);
}

#[test]
fn begin_shows_it_spinning_and_finish_takes_it_away() {
    crate::anim::set_reduced_motion(Some(true));
    let mut s = State::default();
    assert_eq!(pulled(&s, T0), 0.0);
    s.refresh.begin();
    assert!(s.refresh.is_refreshing());
    assert_eq!(pulled(&s, T0), REFRESH_THRESHOLD);
    s.refresh.finish();
    assert_eq!(pulled(&s, T0), 0.0);
    crate::anim::set_reduced_motion(None);
}

#[test]
fn every_indicator_frame_draws() {
    // Each look's glyph parses at every pull and turn step (the spinner's
    // documents are built from strings, like these).
    let theme = crate::style::Theme::light();
    let mut cx = Cx::<State>::new(&theme);
    for refreshing in [false, true] {
        for i in 0..=80 {
            cx.set_frame_time(i * 17 * MS);
            let mut s = State::default();
            if refreshing {
                s.refresh.begin();
            }
            let el = pull_to_refresh(&mut cx, &s.refresh, |s| &mut s.refresh, |_| {})
                .child(Element::boxed(Default::default()));
            assert!(matches!(el.kind, crate::core::ElementKind::Layers { .. }));
        }
    }
    assert!(super::refresh::glyphs_parse());
}

// --- In an app ------------------------------------------------------------

struct Feed {
    refresh: RefreshState,
    swipe: super::SwipeState,
    list: super::ListState,
    items: Vec<u32>,
    log: Vec<String>,
}

const ROW: f64 = 50.0;
const W: f64 = 360.0;

fn feed_view(s: &Feed, cx: &mut Cx<Feed>) -> Element {
    let list = super::pull_to_refresh(
        cx,
        &s.refresh,
        |s| &mut s.refresh,
        |s| s.log.push("refresh".into()),
    )
    .child(super::virtual_list(
        cx,
        &s.list,
        |s| &mut s.list,
        super::VirtualList::fixed(s.items.len(), ROW).height(400.0),
        |cx, row| {
            let id = s.items[row.index];
            let actions = super::SwipeActions::new()
                .trailing(super::SwipeAction::new("Archive", move |s: &mut Feed| {
                    s.log.push(format!("archive {id}"))
                }));
            super::swipe_row(cx, &s.swipe, |s| &mut s.swipe, id, actions)
                .child(super::label(cx, format!("Item {id}")))
        },
    ));
    super::column(vec![list.width(W)])
}

fn feed() -> App<Feed> {
    crate::anim::set_reduced_motion(Some(true));
    let mut app = App::new(
        Feed {
            refresh: RefreshState::new(),
            swipe: super::SwipeState::new(),
            list: super::ListState::new(super::SelectionMode::Single),
            items: (0..40).collect(),
            log: Vec::new(),
        },
        feed_view,
    )
    .no_font()
    .logical_size(Size::new(W, 420.0));
    app.render_once();
    app
}

fn finger(app: &mut App<Feed>, phase: TouchPhase, y: f64) {
    app.touch(Touch::new(1, phase, Point::new(150.0, y)));
    app.render_once();
}

fn find(app: &App<Feed>, role: Role, name: &str) -> Option<AccessNode> {
    let a = app.accessibility_tree().unwrap();
    a.descendants()
        .into_iter()
        .find(|n| n.role == role && n.name == name)
        .cloned()
}

#[test]
fn a_finger_pulling_the_list_at_its_top_refreshes_it() {
    let mut app = feed();
    // The finger lands on a swipeable row: the row lets go of it when the
    // pull takes it.
    finger(&mut app, TouchPhase::Started, 80.0);
    finger(&mut app, TouchPhase::Moved, 100.0);
    assert!(app.state().refresh.is_pulling());
    finger(&mut app, TouchPhase::Moved, 140.0);
    finger(&mut app, TouchPhase::Moved, 220.0);
    // The indicator shows, for screen readers too.
    assert!(find(&app, Role::Text, "Refresh").is_some());
    finger(&mut app, TouchPhase::Ended, 220.0);
    assert!(app.state().refresh.is_refreshing());
    assert_eq!(app.state().log, ["refresh"]);
    assert_eq!(app.state().list.selected_count(), 0, "the row didn't tap");
    assert!(find(&app, Role::Text, "Refresh…").is_some());
    // Still refreshing, frames keep coming (the indicator spins).
    assert!(app.animating());

    // The app finishes: it goes away.
    app.handle().update(|s: &mut Feed| s.refresh.finish());
    app.run_pending();
    app.render_once();
    assert!(find(&app, Role::Text, "Refresh…").is_none());
    assert!(find(&app, Role::Text, "Refresh").is_none());
    crate::anim::set_reduced_motion(None);
}

#[test]
fn scrolled_down_a_pull_scrolls_back_and_a_short_one_doesnt_refresh() {
    let mut app = feed();
    finger(&mut app, TouchPhase::Started, 300.0);
    finger(&mut app, TouchPhase::Moved, 270.0);
    finger(&mut app, TouchPhase::Moved, 100.0);
    finger(&mut app, TouchPhase::Ended, 100.0);
    let scrolled = |app: &App<Feed>| {
        app.panes[0]
            .scroll_offsets
            .values()
            .copied()
            .fold(0.0, f64::max)
    };
    assert!(scrolled(&app) > 100.0);
    // Down again from there: it scrolls back, no pull.
    finger(&mut app, TouchPhase::Started, 100.0);
    finger(&mut app, TouchPhase::Moved, 130.0);
    assert!(!app.state().refresh.is_pulling());
    finger(&mut app, TouchPhase::Moved, 160.0);
    finger(&mut app, TouchPhase::Ended, 160.0);
    assert!(app.state().log.is_empty());

    // Back at the top, a pull short of the threshold doesn't refresh.
    app.panes[0].scroll_offsets.clear();
    app.render_once();
    finger(&mut app, TouchPhase::Started, 100.0);
    finger(&mut app, TouchPhase::Moved, 120.0);
    finger(&mut app, TouchPhase::Moved, 150.0);
    assert!(app.state().refresh.is_pulling());
    finger(&mut app, TouchPhase::Ended, 150.0);
    assert!(!app.state().refresh.is_refreshing());
    assert!(app.state().log.is_empty());
    crate::anim::set_reduced_motion(None);
}

#[test]
fn a_mouse_doesnt_pull_and_the_keyboard_refreshes_from_the_menu() {
    let mut app = feed();
    let at = |y| Point::new(150.0, y);
    // A mouse dragging down from the top of the list: no pull.
    let button = |app: &mut App<Feed>, state, y| {
        app.pointer_event(Event::PointerButton {
            button: PointerButton::Left,
            state,
            position: at(y),
        });
        app.render_once();
    };
    app.pointer_event(Event::PointerMoved { position: at(80.0) });
    button(&mut app, ButtonState::Pressed, 80.0);
    app.pointer_event(Event::PointerMoved {
        position: at(250.0),
    });
    app.render_once();
    assert!(!app.state().refresh.is_pulling());
    button(&mut app, ButtonState::Released, 250.0);
    assert!(!app.state().refresh.is_refreshing());
    // A click puts the list's cursor on row 2.
    assert!(app.click_at(at(1.0 + ROW * 2.5)));
    app.render_once();

    // Focus on the list, the pointer elsewhere: the Menu key's menu has
    // the cursor row's action and Refresh.
    app.hover_at(Point::new(10.0, 415.0));
    assert!(app.focused().is_some());
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
    assert_eq!(names, ["Archive", "Refresh"]);
    let refresh = find(&app, Role::MenuItem, "Refresh").unwrap();
    assert!(app.click_at(refresh.bounds.center()));
    app.render_once();
    assert!(app.state().refresh.is_refreshing());
    assert_eq!(app.state().log, ["refresh"]);
    // A right-click on a row offers Refresh too (after its own action);
    // while refreshing it doesn't run again.
    assert!(app.right_click_at(at(130.0)));
    app.render_once();
    let refresh = find(&app, Role::MenuItem, "Refresh").unwrap();
    assert!(app.click_at(refresh.bounds.center()));
    assert_eq!(app.state().log, ["refresh"]);
    crate::anim::set_reduced_motion(None);
}
