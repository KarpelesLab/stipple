//! Raw pointer and wheel events, and the pointer leaving the window, driven
//! through an [`App`].

use crate::App;
use crate::core::{Cx, Element, PointerEvent, PointerEventKind as Kind, ScrollId, WheelEvent};
use crate::geometry::{Point, Size, Transform};
use crate::layout::Axis;
use crate::platform::{
    ButtonState, Event, Modifiers, PointerButton, ScrollDelta, ScrollPhase, ScrollUnit,
};
use crate::render::Color;

const GRAY: Color = Color::rgb(128, 128, 128);
const RED: Color = Color::rgb(255, 0, 0);

#[derive(Default)]
struct Log {
    /// Which element ('a' or 'b') got which event.
    pointer: Vec<(char, PointerEvent)>,
    wheel: Vec<(char, WheelEvent)>,
}

impl Log {
    fn kinds(&self) -> Vec<(char, Kind, Option<PointerButton>)> {
        self.pointer
            .iter()
            .map(|(c, e)| (*c, e.kind, e.button))
            .collect()
    }
}

fn tile(cx: &mut Cx<Log>, name: char) -> Element {
    Element::stack(Axis::Vertical, vec![])
        .width(100.0)
        .height(50.0)
        .fill(GRAY)
        .on_pointer(cx, move |ev: &PointerEvent, log: &mut Log| {
            log.pointer.push((name, *ev))
        })
}

/// Two 100×50 tiles, `a` above `b`, in a 200×100 window.
fn app() -> App<Log> {
    App::new(Log::default(), |_: &Log, cx: &mut Cx<Log>| {
        let a = tile(cx, 'a');
        let b = tile(cx, 'b');
        Element::stack(Axis::Vertical, vec![a, b])
    })
    .logical_size(Size::new(200.0, 100.0))
}

fn press(button: PointerButton, x: f64, y: f64) -> Event {
    Event::PointerButton {
        button,
        state: ButtonState::Pressed,
        position: Point::new(x, y),
    }
}

fn release(button: PointerButton, x: f64, y: f64) -> Event {
    Event::PointerButton {
        button,
        state: ButtonState::Released,
        position: Point::new(x, y),
    }
}

fn moved(x: f64, y: f64) -> Event {
    Event::PointerMoved {
        position: Point::new(x, y),
    }
}

#[test]
fn middle_and_side_buttons_reach_the_element() {
    let mut app = app();
    let _ = app.render_frame();
    assert!(app.pointer_event(press(PointerButton::Middle, 10.0, 70.0)));
    assert!(app.pointer_event(release(PointerButton::Middle, 10.0, 70.0)));
    assert!(app.pointer_event(press(PointerButton::Back, 10.0, 70.0)));
    let log = &app.state().pointer;
    assert_eq!(
        app.state().kinds(),
        [
            ('b', Kind::Down, Some(PointerButton::Middle)),
            ('b', Kind::Up, Some(PointerButton::Middle)),
            ('b', Kind::Down, Some(PointerButton::Back)),
        ]
    );
    // In the element's coordinates, with its size.
    assert_eq!(log[0].1.position, Point::new(10.0, 20.0));
    assert_eq!(log[0].1.window_position, Point::new(10.0, 70.0));
    assert_eq!(log[0].1.size, Size::new(100.0, 50.0));
    assert!(log[0].1.buttons_held.contains(PointerButton::Middle));
    assert!(log[1].1.buttons_held.is_empty(), "released: no longer held");
    assert!(log[2].1.buttons_held.contains(PointerButton::Back));
}

#[test]
fn events_carry_their_modifiers() {
    let mut app = app();
    let _ = app.render_frame();
    crate::platform::set_pointer_modifiers(Modifiers {
        ctrl: true,
        ..Modifiers::default()
    });
    app.pointer_event(press(PointerButton::Left, 10.0, 10.0));
    crate::platform::set_pointer_modifiers(Modifiers::default());
    app.pointer_event(release(PointerButton::Left, 10.0, 10.0));
    let log = &app.state().pointer;
    assert!(log[0].1.modifiers.ctrl);
    assert!(!log[1].1.modifiers.ctrl);
}

#[test]
fn quick_presses_count_double_and_triple_clicks() {
    let mut app = app();
    let _ = app.render_frame();
    for _ in 0..4 {
        app.pointer_event(press(PointerButton::Left, 10.0, 10.0));
        app.pointer_event(release(PointerButton::Left, 11.0, 10.0));
    }
    // Another button starts over, and so does a press far away.
    app.pointer_event(press(PointerButton::Right, 11.0, 10.0));
    app.pointer_event(press(PointerButton::Right, 60.0, 30.0));
    let counts: Vec<u32> = app
        .state()
        .pointer
        .iter()
        .map(|(_, e)| e.click_count)
        .collect();
    assert_eq!(counts, [1, 1, 2, 2, 3, 3, 1, 1, 1, 1]);
}

#[test]
fn a_press_captures_moves_and_the_release() {
    let mut app = app();
    let _ = app.render_frame();
    app.pointer_event(moved(10.0, 10.0));
    app.pointer_event(press(PointerButton::Right, 10.0, 10.0));
    // Over `b`, then out of the window: still `a`'s, no leave.
    app.pointer_event(moved(10.0, 70.0));
    app.pointer_event(moved(-30.0, 120.0));
    let last = app.state().pointer.last().unwrap().1;
    assert_eq!(
        (last.kind, last.position),
        (Kind::Move, Point::new(-30.0, 120.0))
    );
    assert!(!last.is_inside());
    // The leave the window reports while the button is down waits.
    app.pointer_event(Event::PointerLeft);
    app.pointer_event(release(PointerButton::Right, -30.0, 120.0));
    assert_eq!(
        app.state().kinds(),
        [
            ('a', Kind::Move, None),
            ('a', Kind::Down, Some(PointerButton::Right)),
            ('a', Kind::Move, None),
            ('a', Kind::Move, None),
            ('a', Kind::Up, Some(PointerButton::Right)),
            // Released outside the window: that's the leave.
            ('a', Kind::Leave, None),
        ]
    );
}

#[test]
fn moving_between_elements_leaves_the_first() {
    let mut app = app();
    let _ = app.render_frame();
    app.pointer_event(moved(10.0, 10.0));
    app.pointer_event(moved(10.0, 60.0));
    app.pointer_event(moved(150.0, 60.0)); // off both
    app.pointer_event(press(PointerButton::Left, 10.0, 10.0));
    // Released over `b`: `a` hears it, then hears it lost the pointer.
    app.pointer_event(release(PointerButton::Left, 10.0, 60.0));
    assert_eq!(
        app.state().kinds(),
        [
            ('a', Kind::Move, None),
            ('a', Kind::Leave, None),
            ('b', Kind::Move, None),
            ('b', Kind::Leave, None),
            ('a', Kind::Down, Some(PointerButton::Left)),
            ('a', Kind::Up, Some(PointerButton::Left)),
            ('a', Kind::Leave, None),
            ('b', Kind::Move, None),
        ]
    );
}

#[test]
fn positions_are_read_through_transforms() {
    let mut app = App::new(Log::default(), |_: &Log, cx: &mut Cx<Log>| {
        // Drawn twice as big about its top-left corner.
        let a = tile(cx, 'a').transform(Transform::IDENTITY.scale(2.0).origin(0.0, 0.0));
        Element::stack(Axis::Vertical, vec![a])
    })
    .logical_size(Size::new(300.0, 200.0));
    let _ = app.render_frame();
    app.pointer_event(press(PointerButton::Left, 150.0, 80.0));
    let ev = app.state().pointer[0].1;
    assert_eq!(ev.position, Point::new(75.0, 40.0));
    assert!(ev.is_inside());
}

#[test]
fn leaving_the_window_clears_every_hover() {
    let mut app = App::new(Log::default(), |_: &Log, cx: &mut Cx<Log>| {
        let a = tile(cx, 'a');
        // A button: the theme's hover highlight, and a `.hovered` look.
        let b = Element::stack(Axis::Vertical, vec![])
            .width(100.0)
            .height(50.0)
            .fill(GRAY)
            .hovered(|e| e.fill(RED))
            .on_tap(cx, |_: &mut Log| {});
        Element::stack(Axis::Vertical, vec![a, b])
    })
    .logical_size(Size::new(200.0, 100.0));
    let _ = app.render_frame();
    app.pointer_event(moved(10.0, 10.0));
    app.pointer_event(moved(10.0, 60.0));
    let _ = app.render_frame();
    assert!(app.panes[0].hovered.is_some());
    assert_eq!(
        app.panes[0].tree.as_ref().unwrap().children[1]
            .decoration
            .fill,
        Some(RED)
    );
    assert!(app.pointer_event(Event::PointerLeft));
    let _ = app.render_frame();
    assert!(
        app.panes[0].hovered.is_none(),
        "the hover highlight is gone"
    );
    assert_eq!(
        app.panes[0].tree.as_ref().unwrap().children[1]
            .decoration
            .fill,
        Some(GRAY),
        "and the hovered look"
    );
    // Over `a` then out: `a` hears it leave.
    app.pointer_event(moved(10.0, 10.0));
    app.pointer_event(Event::PointerLeft);
    assert_eq!(
        app.state().kinds(),
        [
            ('a', Kind::Move, None),
            ('a', Kind::Leave, None),
            ('a', Kind::Move, None),
            ('a', Kind::Leave, None),
        ]
    );
}

#[test]
fn a_release_outside_the_window_ends_the_hover() {
    let mut app = App::new(Log::default(), |_: &Log, cx: &mut Cx<Log>| {
        let b = Element::stack(Axis::Vertical, vec![])
            .width(100.0)
            .height(50.0)
            .hovered(|e| e.fill(RED))
            .on_tap(cx, |_: &mut Log| {});
        Element::stack(Axis::Vertical, vec![b])
    })
    .logical_size(Size::new(200.0, 100.0));
    let _ = app.render_frame();
    app.pointer_event(moved(10.0, 10.0));
    app.pointer_event(press(PointerButton::Left, 10.0, 10.0));
    app.pointer_event(moved(10.0, 300.0));
    // The window says the pointer left while the button is down: too soon.
    app.pointer_event(Event::PointerLeft);
    app.pointer_event(release(PointerButton::Left, 10.0, 300.0));
    let _ = app.render_frame();
    assert!(app.panes[0].hovered.is_none());
    assert_eq!(
        app.panes[0].tree.as_ref().unwrap().children[0]
            .decoration
            .fill,
        None
    );
}

#[test]
fn the_wheel_goes_to_the_innermost_taker() {
    let mut app = App::new(Log::default(), |_: &Log, cx: &mut Cx<Log>| {
        let id = cx.register_scroll();
        let wheel = Element::stack(Axis::Vertical, vec![])
            .width(100.0)
            .height(50.0)
            .on_wheel(cx, |ev: &WheelEvent, log: &mut Log| {
                log.wheel.push(('w', *ev))
            });
        let filler = Element::stack(Axis::Vertical, vec![])
            .width(100.0)
            .height(400.0);
        let list = Element::stack(Axis::Vertical, vec![wheel, filler])
            .height(100.0)
            .scrollable(id);
        Element::stack(Axis::Vertical, vec![list])
    })
    .logical_size(Size::new(200.0, 100.0));
    let _ = app.render_frame();
    let scroll = |app: &App<Log>| app.panes[0].scroll_offsets.get(&ScrollId(0)).copied();
    // Over the wheel element: it takes the scroll (lines, horizontal too).
    app.pointer_event(moved(10.0, 10.0));
    assert!(app.pointer_event(Event::Scroll {
        delta: ScrollDelta::lines(1.0, 0.0),
    }));
    let trackpad = ScrollDelta::pixels(0.0, 3.5).with_phase(ScrollPhase::Started);
    assert!(app.pointer_event(Event::Scroll { delta: trackpad }));
    assert_eq!(scroll(&app).unwrap_or(0.0), 0.0, "the list didn't move");
    let wheel = &app.state().wheel;
    assert_eq!(wheel.len(), 2);
    assert_eq!((wheel[0].1.dx, wheel[0].1.unit), (1.0, ScrollUnit::Lines));
    assert_eq!(
        (wheel[1].1.dy, wheel[1].1.unit, wheel[1].1.phase),
        (3.5, ScrollUnit::Pixels, ScrollPhase::Started)
    );
    assert_eq!(wheel[1].1.position, Point::new(10.0, 10.0));
    // Over the rest of the list: the list scrolls, a line at 40 px.
    app.pointer_event(moved(10.0, 80.0));
    assert!(app.pointer_event(Event::Scroll {
        delta: ScrollDelta::lines(0.0, 1.0),
    }));
    assert_eq!(scroll(&app), Some(40.0));
    assert_eq!(app.state().wheel.len(), 2);
}
