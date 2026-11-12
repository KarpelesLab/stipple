//! Touch gestures through the `App`, fed synthetic touches as a touch-screen
//! backend delivers them: long presses opening context menus, sideways
//! scrolling, and pinches.

use std::time::Duration;

use crate::App;
use crate::core::{
    AccessNode, Axis, Cx, Element, Menu, MenuItem, PinchEvent, PinchPhase, Role, ScrollId,
};
use crate::geometry::{Point, Size, Vec2};
use crate::platform::{Touch, TouchPhase};
use crate::widgets::{button, column, label, row, scroll, scroll_horizontal};

#[derive(Default)]
struct Log {
    tapped: Vec<usize>,
    picked: Vec<String>,
    pinches: Vec<(PinchPhase, f64, Vec2, Point)>,
}

fn finger(id: u64, phase: TouchPhase, x: f64, y: f64) -> Touch {
    Touch {
        id,
        phase,
        position: Point::new(x, y),
    }
}

/// A 300×400 window: a strip of 10 chips 100 wide (1000 px, so it scrolls
/// up to 700 sideways) at the top, inside a list of 20 rows 50 tall, of which
/// the even ones have a context menu; and below the list, a 300×100 photo
/// that takes pinches (and taps, as row 99).
fn app() -> App<Log> {
    App::new(Log::default(), |_: &Log, cx: &mut Cx<Log>| {
        let theme = *cx.theme();
        let chips = (0..10)
            .map(|i| {
                button(&theme)
                    .width(100.0)
                    .height(40.0)
                    .on_tap(cx, move |s: &mut Log| s.tapped.push(100 + i))
            })
            .collect();
        let mut items = vec![scroll_horizontal(cx, 300.0, row(chips)).height(40.0)];
        for i in 0..20 {
            let mut item = button(&theme)
                .width(300.0)
                .height(50.0)
                .on_tap(cx, move |s: &mut Log| s.tapped.push(i));
            if i % 2 == 0 {
                item = item.context_menu(
                    cx,
                    Menu::new("").item(MenuItem::new(format!("Pin {i}"), move |s: &mut Log| {
                        s.picked.push(format!("pin {i}"))
                    })),
                );
            }
            items.push(item);
        }
        let photo = label(&theme, "Photo")
            .width(300.0)
            .height(100.0)
            .on_tap(cx, |s: &mut Log| s.tapped.push(99))
            .on_pinch(cx, |ev: &PinchEvent, s: &mut Log| {
                s.pinches
                    .push((ev.phase, ev.scale, ev.translation, ev.centroid))
            });
        column(vec![scroll(cx, 300.0, column(items)), photo])
    })
    .logical_size(Size::new(300.0, 400.0))
}

fn offset(app: &App<Log>, id: ScrollId) -> f64 {
    app.panes[0].scroll_offsets.get(&id).copied().unwrap_or(0.0)
}

/// The strip registers first (the list's content is built before it).
const LIST: ScrollId = ScrollId(1);
const STRIP: ScrollId = ScrollId(0);

fn menus(app: &App<Log>) -> Vec<AccessNode> {
    let tree = app.accessibility_tree().expect("a frame was built");
    tree.descendants()
        .into_iter()
        .filter(|n| n.role == Role::MenuItem)
        .cloned()
        .collect()
}

/// Let `secs` pass on the touch clock.
fn wait(app: &mut App<Log>, secs: f64) {
    app.panes[0].touch_epoch -= Duration::from_secs_f64(secs);
}

#[test]
fn a_long_press_opens_the_context_menu_and_doesnt_tap() {
    let mut app = app();
    app.render_once();
    // Row 2 is at y 140..190.
    app.touch(finger(1, TouchPhase::Started, 150.0, 160.0));
    assert!(
        app.panes[0].long_press_timer.is_some(),
        "a timer wakes the loop"
    );
    assert!(!app.advance_long_press(), "not yet");
    wait(&mut app, 0.6);
    assert!(app.advance_long_press());
    assert!(app.panes[0].long_press_timer.is_none());
    app.render_once();
    let items = menus(&app);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].name, "Pin 2");
    // At the finger.
    assert!(
        items[0].bounds.min_y() >= 160.0 - 1.0,
        "{:?}",
        items[0].bounds
    );
    // Lifting the finger doesn't tap the row (nor pick the item under it).
    app.touch(finger(1, TouchPhase::Moved, 152.0, 175.0));
    app.touch(finger(1, TouchPhase::Ended, 152.0, 175.0));
    app.render_once();
    assert!(app.state().tapped.is_empty(), "{:?}", app.state().tapped);
    assert!(app.state().picked.is_empty());
    assert_eq!(menus(&app).len(), 1, "the menu stays open");
    // A tap on the item picks it.
    let at = menus(&app)[0].bounds.center();
    app.touch(finger(2, TouchPhase::Started, at.x, at.y));
    app.touch(finger(2, TouchPhase::Ended, at.x, at.y));
    app.render_once();
    assert_eq!(app.state().picked, ["pin 2"]);
    assert!(menus(&app).is_empty());
    assert!(app.state().tapped.is_empty());
}

#[test]
fn a_tap_or_a_drag_doesnt_long_press() {
    let mut app = app();
    app.render_once();
    // A tap on row 2 taps it, and drops the timer.
    app.touch(finger(1, TouchPhase::Started, 150.0, 160.0));
    app.touch(finger(1, TouchPhase::Ended, 150.0, 160.0));
    assert_eq!(app.state().tapped, [2]);
    assert!(app.panes[0].long_press_timer.is_none());
    wait(&mut app, 0.6);
    assert!(!app.advance_long_press());
    // A drag scrolls instead.
    app.touch(finger(1, TouchPhase::Started, 150.0, 260.0));
    app.touch(finger(1, TouchPhase::Moved, 150.0, 200.0));
    assert!(app.panes[0].long_press_timer.is_none());
    wait(&mut app, 0.6);
    assert!(!app.advance_long_press());
    app.touch(finger(1, TouchPhase::Ended, 150.0, 200.0));
    app.render_once();
    assert!(menus(&app).is_empty());
    assert_eq!(offset(&app, LIST), 60.0 - crate::core::touch::SLOP);
}

#[test]
fn holding_a_row_without_a_menu_still_taps_it() {
    let mut app = app();
    app.render_once();
    // Row 1 (y 90..140) has no context menu: no timer, and a long hold taps.
    app.touch(finger(1, TouchPhase::Started, 150.0, 110.0));
    assert!(app.panes[0].long_press_timer.is_none());
    wait(&mut app, 1.0);
    assert!(!app.advance_long_press());
    app.touch(finger(1, TouchPhase::Ended, 150.0, 110.0));
    assert_eq!(app.state().tapped, [1]);
}

#[test]
fn a_sideways_drag_scrolls_the_strip_and_a_vertical_one_the_list() {
    let mut app = app();
    app.render_once();
    // Left 200 px across the strip (y 0..40).
    app.touch(finger(1, TouchPhase::Started, 250.0, 20.0));
    for x in [230.0, 150.0, 50.0] {
        app.touch(finger(1, TouchPhase::Moved, x, 22.0));
    }
    app.touch(finger(1, TouchPhase::Ended, 50.0, 22.0));
    assert!(app.state().tapped.is_empty(), "no chip tapped");
    assert_eq!(offset(&app, STRIP), 200.0 - crate::core::touch::SLOP);
    assert_eq!(offset(&app, LIST), 0.0);
    // The chips moved with it: x 20 is now over chip 2 (200..300 before).
    app.render_once();
    app.click_at(Point::new(20.0, 20.0));
    assert_eq!(app.state().tapped, [102]);
    // Up over the strip scrolls the list.
    app.touch(finger(1, TouchPhase::Started, 150.0, 30.0));
    app.touch(finger(1, TouchPhase::Moved, 152.0, 20.0));
    app.touch(finger(1, TouchPhase::Ended, 152.0, 20.0));
    assert_eq!(offset(&app, LIST), 10.0 - crate::core::touch::SLOP);
    // Sideways past the end stops at the end.
    app.panes[0].scroll_offsets.insert(LIST, 0.0);
    app.panes[0].dirty = true;
    app.render_once();
    for _ in 0..3 {
        app.touch(finger(1, TouchPhase::Started, 290.0, 20.0));
        app.touch(finger(1, TouchPhase::Moved, 280.0, 20.0));
        app.touch(finger(1, TouchPhase::Moved, 0.0, 20.0));
        app.touch(finger(1, TouchPhase::Ended, 0.0, 20.0));
        app.render_once();
    }
    assert_eq!(offset(&app, STRIP), 700.0);
}

#[test]
fn a_horizontal_wheel_scrolls_the_strip() {
    use crate::platform::{Event, ScrollDelta};
    let mut app = app();
    app.render_once();
    app.hover_at(Point::new(150.0, 20.0));
    let wheel = |dx, dy| Event::Scroll {
        delta: ScrollDelta::pixels(dx, dy),
    };
    assert!(app.pointer_event(wheel(80.0, 0.0)));
    assert_eq!(offset(&app, STRIP), 80.0);
    // A vertical wheel over it scrolls the list it is in.
    assert!(app.pointer_event(wheel(0.0, 30.0)));
    assert_eq!(offset(&app, LIST), 30.0);
    assert_eq!(offset(&app, STRIP), 80.0);
}

#[test]
fn two_fingers_pinch_the_photo_without_tapping_it() {
    let mut app = app();
    app.render_once();
    // The photo is at y 300..400.
    app.touch(finger(1, TouchPhase::Started, 100.0, 350.0));
    app.touch(finger(2, TouchPhase::Started, 200.0, 350.0));
    app.touch(finger(1, TouchPhase::Moved, 50.0, 360.0));
    app.touch(finger(2, TouchPhase::Moved, 250.0, 360.0));
    app.touch(finger(2, TouchPhase::Ended, 250.0, 360.0));
    app.touch(finger(1, TouchPhase::Ended, 50.0, 360.0));
    assert!(app.state().tapped.is_empty(), "{:?}", app.state().tapped);
    let pinches = &app.state().pinches;
    let phases: Vec<_> = pinches.iter().map(|p| p.0).collect();
    assert_eq!(
        phases,
        [
            PinchPhase::Began,
            PinchPhase::Changed,
            PinchPhase::Changed,
            PinchPhase::Ended
        ]
    );
    let (_, scale, translation, centroid) = pinches[3];
    assert!((scale - 2.0).abs() < 1e-9, "scale {scale}");
    assert_eq!(translation, Vec2::new(0.0, 10.0));
    // In the photo's coordinates.
    assert_eq!(centroid, Point::new(150.0, 60.0));
    // One finger still taps it.
    app.touch(finger(3, TouchPhase::Started, 150.0, 350.0));
    app.touch(finger(3, TouchPhase::Ended, 150.0, 350.0));
    assert_eq!(app.state().tapped, [99]);
}

#[test]
fn a_second_finger_over_the_list_is_ignored() {
    let mut app = app();
    app.render_once();
    app.touch(finger(1, TouchPhase::Started, 150.0, 110.0));
    app.touch(finger(2, TouchPhase::Started, 250.0, 110.0));
    app.touch(finger(2, TouchPhase::Ended, 250.0, 110.0));
    // The first finger's tap still lands, once.
    app.touch(finger(1, TouchPhase::Ended, 150.0, 110.0));
    assert_eq!(app.state().tapped, [1]);
    assert!(app.state().pinches.is_empty());
}

#[test]
fn a_right_to_left_strip_scrolls_towards_the_left() {
    use crate::layout::LayoutDirection;
    let mut app = App::new((), |_: &(), cx: &mut Cx<()>| {
        let theme = *cx.theme();
        let chips = (0..10)
            .map(|_| button(&theme).width(100.0).height(40.0))
            .collect();
        let strip = Element::stack(Axis::Horizontal, vec![row(chips)])
            .width(300.0)
            .height(40.0)
            .scrollable(cx.register_scroll());
        column(vec![strip]).layout_direction(LayoutDirection::Rtl)
    })
    .logical_size(Size::new(300.0, 100.0));
    app.render_once();
    let id = ScrollId(0);
    let first_chip = |app: &App<()>| {
        let tree = app.panes[0].tree.as_ref().unwrap();
        let strip = crate::core::find_scroll(tree, id).unwrap();
        strip.children[0].bounds
    };
    // Laid out from the right edge, overflowing to the left.
    assert_eq!(first_chip(&app).max_x(), 300.0);
    // A finger moving right reveals what is further left.
    app.touch(finger(1, TouchPhase::Started, 50.0, 20.0));
    app.touch(finger(1, TouchPhase::Moved, 60.0, 20.0));
    app.touch(finger(1, TouchPhase::Moved, 150.0, 20.0));
    app.touch(finger(1, TouchPhase::Ended, 150.0, 20.0));
    app.render_once();
    let off = app.panes[0].scroll_offsets[&id];
    assert_eq!(off, 100.0 - crate::core::touch::SLOP);
    assert_eq!(first_chip(&app).max_x(), 300.0 + off);
}
