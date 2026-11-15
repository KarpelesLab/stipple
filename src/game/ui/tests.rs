//! Game UI: spatial navigation's geometry and the focus moves it makes,
//! gamepads driving a menu, the HUD letting presses through, the "press a
//! key…" capture, typewriter timing and a bar's damage ghost at fixed frame
//! times.

use std::collections::HashMap;
use std::time::Duration;

use super::nav::{NavInput, PadNav, stick_direction};
use super::*;
use crate::App;
use crate::core::interact::Resolver;
use crate::core::tab_focus::{Direction, arrow_step, spatial_entry, spatial_pick, spatial_takes};
use crate::core::transition::Transitions;
use crate::core::{
    Cx, Element, ElementKey, FocusId, Handled, Interaction, KeyInput, LayoutNode, PointerEvent,
    layout, promote_tap_focus,
};
use crate::game::{Actions, Binding, InputState};
use crate::geometry::{Point, Rect, Size};
use crate::layout::Axis;
use crate::platform::gamepad::{GamepadAxis, GamepadButton, GamepadEvent, GamepadId};
use crate::platform::time::Instant;
use crate::platform::{ButtonState, Event, KeyCode, KeyEvent, PhysicalKey, PointerButton};
use crate::render::Color;
use crate::style::Theme;

const MS: Duration = Duration::from_millis(1);

fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::from_xywh(x, y, w, h)
}

// --- Spatial navigation's geometry ---

/// A 3 × 3 grid of 40 × 20 cells, 10 apart: ids 0..9, row by row.
fn grid() -> Vec<(FocusId, Rect)> {
    (0..9u32)
        .map(|i| {
            let (col, row) = (f64::from(i % 3), f64::from(i / 3));
            (FocusId(i), r(col * 50.0, row * 30.0, 40.0, 20.0))
        })
        .collect()
}

fn pick(cells: &[(FocusId, Rect)], from: usize, dir: Direction) -> Option<u32> {
    let others: Vec<_> = cells
        .iter()
        .copied()
        .filter(|(id, _)| id.0 != from as u32)
        .collect();
    spatial_pick(cells[from].1, &others, dir, true).map(|f| f.0)
}

#[test]
fn spatial_moves_to_the_neighbor_in_a_grid_and_wraps_in_line() {
    let g = grid();
    assert_eq!(pick(&g, 4, Direction::Right), Some(5));
    assert_eq!(pick(&g, 4, Direction::Left), Some(3));
    assert_eq!(pick(&g, 4, Direction::Up), Some(1));
    assert_eq!(pick(&g, 4, Direction::Down), Some(7));
    // Past the edge: round to the other end of the same row or column.
    assert_eq!(pick(&g, 5, Direction::Right), Some(3));
    assert_eq!(pick(&g, 1, Direction::Up), Some(7));
    assert_eq!(pick(&g, 6, Direction::Down), Some(0));
    // Without wrapping, nothing.
    let others: Vec<_> = g[..8].to_vec();
    assert_eq!(spatial_pick(g[8].1, &others, Direction::Right, false), None);
}

#[test]
fn spatial_prefers_what_is_in_line_unless_something_else_is_wholly_nearer() {
    // From a wide button: one far below in its column, one just below but
    // off to the side.
    let from = r(0.0, 0.0, 100.0, 20.0);
    let in_line = (FocusId(1), r(10.0, 200.0, 80.0, 20.0));
    let aside = (FocusId(2), r(150.0, 30.0, 80.0, 20.0));
    assert_eq!(
        spatial_pick(from, &[in_line, aside], Direction::Down, true),
        Some(FocusId(2)),
        "the one aside ends before the one in line starts"
    );
    // Once the one in line is near, it wins.
    let near = (FocusId(3), r(10.0, 30.0, 80.0, 20.0));
    assert_eq!(
        spatial_pick(from, &[in_line, aside, near], Direction::Down, true),
        Some(FocusId(3))
    );
    // Among the out-of-line ones, the better lined up.
    let a = (FocusId(4), r(120.0, 60.0, 40.0, 20.0));
    let b = (FocusId(5), r(400.0, 40.0, 40.0, 20.0));
    assert_eq!(
        spatial_pick(from, &[a, b], Direction::Down, false),
        Some(FocusId(4))
    );
    // Something level with it is not below it.
    let level = (FocusId(6), r(150.0, 0.0, 40.0, 20.0));
    assert_eq!(spatial_pick(from, &[level], Direction::Down, false), None);
}

#[test]
fn spatial_crosses_between_columns_of_different_heights() {
    // A settings screen: tall rows on the left, a column of short ones on
    // the right.
    let left = [
        (FocusId(0), r(0.0, 0.0, 200.0, 50.0)),
        (FocusId(1), r(0.0, 60.0, 200.0, 50.0)),
    ];
    let right = [
        (FocusId(2), r(300.0, 0.0, 200.0, 30.0)),
        (FocusId(3), r(300.0, 40.0, 200.0, 30.0)),
        (FocusId(4), r(300.0, 80.0, 200.0, 30.0)),
    ];
    let all: Vec<_> = left.iter().chain(&right).copied().collect();
    let go = |from: usize, dir| {
        let others: Vec<_> = all
            .iter()
            .copied()
            .filter(|c| c.0.0 != from as u32)
            .collect();
        spatial_pick(all[from].1, &others, dir, true).map(|f| f.0)
    };
    assert_eq!(go(1, Direction::Right), Some(4), "the nearest in line");
    assert_eq!(go(0, Direction::Right), Some(2));
    assert_eq!(go(3, Direction::Left), Some(0));
    assert_eq!(go(4, Direction::Left), Some(1));
}

// --- Spatial focus groups in a laid-out tree ---

fn tile() -> Element {
    Element::stack(Axis::Vertical, vec![])
        .width(60.0)
        .height(20.0)
}

fn laid(build: impl FnOnce(&mut Cx<()>) -> Element) -> LayoutNode {
    let theme = Theme::light();
    let mut cx = Cx::new(&theme);
    let el = build(&mut cx);
    let mut tree = layout(&el, Rect::from_xywh(0.0, 0.0, 400.0, 300.0), None);
    promote_tap_focus(&mut tree, &[]);
    tree
}

#[test]
fn arrows_in_a_spatial_group_go_by_position_not_tree_order() {
    // Two columns, built column by column: tree order runs down the left
    // one first.
    let tree = laid(|cx| {
        let col = |cx: &mut Cx<()>| {
            Element::stack(
                Axis::Vertical,
                vec![tile().on_tap(cx, |_| {}), tile().on_tap(cx, |_| {})],
            )
            .gap(10.0)
        };
        Element::stack(Axis::Horizontal, vec![col(cx), col(cx)])
            .gap(20.0)
            .spatial_nav()
    });
    let id = |c: usize, r: usize| tree.children[c].children[r].focus.unwrap();
    let step = |from, k: KeyInput| arrow_step(&tree, from, &k).map(|s| s.0);
    assert_eq!(step(id(0, 0), KeyInput::Right), Some(id(1, 0)));
    assert_eq!(step(id(1, 0), KeyInput::Down), Some(id(1, 1)));
    assert_eq!(step(id(1, 1), KeyInput::Left), Some(id(0, 1)));
    assert_eq!(step(id(0, 1), KeyInput::Down), Some(id(0, 0)), "wraps");
    assert_eq!(step(id(0, 0), KeyInput::Home), None);
    // The group's first control is where focus starts.
    assert_eq!(spatial_entry(&tree, None, None), Some(id(0, 0)));
    assert_eq!(spatial_entry(&tree, None, Some(id(1, 1))), None);
}

#[test]
fn a_control_with_its_own_keys_keeps_left_and_right_in_a_spatial_group() {
    let tree = laid(|cx| {
        Element::stack(
            Axis::Vertical,
            vec![tile().on_key(cx, |_, _| {}), tile().on_tap(cx, |_| {})],
        )
        .spatial_nav()
    });
    let slider = tree.children[0].focus.unwrap();
    let button = tree.children[1].focus.unwrap();
    assert!(!spatial_takes(&tree, slider, &KeyInput::Left));
    assert!(!spatial_takes(&tree, slider, &KeyInput::Right));
    assert!(spatial_takes(&tree, slider, &KeyInput::Down));
    assert!(spatial_takes(&tree, button, &KeyInput::Left));
    assert!(!spatial_takes(&tree, button, &KeyInput::Enter));
    assert_eq!(
        arrow_step(&tree, slider, &KeyInput::Down).map(|s| s.0),
        Some(button)
    );
}

// --- A menu in an App: keys, gamepads, back ---

#[derive(Default)]
struct Menu {
    pressed: Vec<&'static str>,
    backs: u32,
    nav_off: bool,
}

fn menu_app() -> App<Menu> {
    menu_app_with(Menu::default())
}

fn menu_app_with(menu: Menu) -> App<Menu> {
    App::new(menu, |_: &Menu, cx: &mut Cx<Menu>| {
        let theme = *cx.theme();
        let b = |cx: &mut Cx<Menu>, name: &'static str| {
            menu_button(cx, &theme, name, move |m: &mut Menu| m.pressed.push(name))
        };
        let items = vec![b(cx, "One"), b(cx, "Two"), b(cx, "Three")];
        menu_list(&theme, items)
    })
    .logical_size(Size::new(400.0, 300.0))
    .gamepad_navigation_when(|m: &Menu| !m.nav_off)
    .on_back(|m: &mut Menu| {
        m.backs += 1;
        Handled::Yes
    })
}

fn pad() -> GamepadId {
    GamepadId(7)
}

fn button(button: GamepadButton, pressed: bool) -> GamepadEvent {
    GamepadEvent::Button {
        id: pad(),
        button,
        pressed,
    }
}

fn axis(axis: GamepadAxis, value: f32) -> GamepadEvent {
    GamepadEvent::Axis {
        id: pad(),
        axis,
        value,
    }
}

#[test]
fn the_first_menu_button_takes_focus_and_the_keys_move_it() {
    let mut app = menu_app();
    app.render_once();
    assert!(app.focused().is_some(), "the menu's first button");
    app.render_once();
    assert!(app.press_key(KeyInput::Down));
    assert!(app.press_key(KeyInput::Enter));
    assert!(app.press_key(KeyInput::Up));
    assert!(app.press_key(KeyInput::Up), "wraps to the last");
    assert!(app.press_key(KeyInput::Text(" ".into())));
    assert_eq!(app.state().pressed, ["Two", "Three"]);
    // Escape goes back through on_back.
    assert!(app.press_key_code(KeyCode::Escape, Default::default()));
    assert_eq!(app.state().backs, 1);
}

#[test]
fn a_gamepad_moves_presses_and_goes_back() {
    let mut app = menu_app();
    app.render_once();
    let t0 = Instant::now();
    assert!(app.gamepad_event(&button(GamepadButton::DPadDown, true), t0));
    app.gamepad_event(&button(GamepadButton::DPadDown, false), t0);
    assert!(app.gamepad_event(&button(GamepadButton::South, true), t0));
    app.gamepad_event(&button(GamepadButton::South, false), t0);
    assert_eq!(app.state().pressed, ["Two"]);
    // The stick, past its threshold, moves once; it has to come back
    // before it moves again.
    assert!(!app.gamepad_event(&axis(GamepadAxis::LeftY, 0.3), t0));
    assert!(app.gamepad_event(&axis(GamepadAxis::LeftY, 0.8), t0));
    assert!(!app.gamepad_event(&axis(GamepadAxis::LeftY, 0.9), t0));
    app.gamepad_event(&axis(GamepadAxis::LeftY, 0.0), t0);
    app.gamepad_event(&button(GamepadButton::South, true), t0);
    assert_eq!(app.state().pressed, ["Two", "Three"]);
    // East goes back.
    assert!(app.gamepad_event(&button(GamepadButton::East, true), t0));
    assert_eq!(app.state().backs, 1);
    // Focus stayed where it was.
    app.gamepad_event(&button(GamepadButton::East, false), t0);
    app.press_key(KeyInput::Enter);
    assert_eq!(app.state().pressed, ["Two", "Three", "Three"]);
}

#[test]
fn a_held_direction_repeats_after_a_delay() {
    let mut app = menu_app();
    app.render_once();
    let t0 = Instant::now();
    app.gamepad_event(&button(GamepadButton::DPadDown, true), t0); // Two
    let at = |d: Duration| t0.checked_add(d).unwrap();
    assert!(!app.gamepad_repeat(at(REPEAT_DELAY - MS)), "not yet");
    assert!(app.gamepad_repeat(at(REPEAT_DELAY)), "Three");
    assert!(!app.gamepad_repeat(at(REPEAT_DELAY + REPEAT_EVERY - MS)));
    assert!(app.gamepad_repeat(at(REPEAT_DELAY + REPEAT_EVERY)), "One");
    app.gamepad_event(
        &button(GamepadButton::DPadDown, false),
        at(REPEAT_DELAY * 2),
    );
    assert!(!app.gamepad_repeat(at(REPEAT_DELAY * 3)), "let go");
    app.gamepad_event(&button(GamepadButton::South, true), at(REPEAT_DELAY * 3));
    assert_eq!(app.state().pressed, ["One"]);
}

#[test]
fn the_pad_translator_follows_the_stick_with_hysteresis() {
    assert_eq!(stick_direction(0.4, 0.0, None), None);
    assert_eq!(stick_direction(0.6, 0.1, None), Some(Direction::Right));
    assert_eq!(stick_direction(-0.1, -0.7, None), Some(Direction::Up));
    // Held, it stays held until it comes back within the release.
    assert_eq!(
        stick_direction(0.35, 0.0, Some(Direction::Right)),
        Some(Direction::Right)
    );
    assert_eq!(stick_direction(0.25, 0.0, Some(Direction::Right)), None);
    // Leaning further another way takes that way.
    assert_eq!(
        stick_direction(0.4, 0.8, Some(Direction::Right)),
        Some(Direction::Down)
    );
    let mut nav = PadNav::default();
    let t0 = Instant::now();
    assert_eq!(
        nav.event(&axis(GamepadAxis::LeftX, -0.9), t0),
        Some(NavInput::Move(Direction::Left))
    );
    assert!(nav.deadline().is_some());
    nav.event(&GamepadEvent::Disconnected { id: pad() }, t0);
    assert_eq!(nav.deadline(), None, "a pad that goes lets go");
    assert_eq!(
        nav.event(&button(GamepadButton::West, true), t0),
        None,
        "West does nothing"
    );
}

#[test]
fn gamepad_navigation_can_be_off() {
    // Off from the state (a capture in progress, say).
    let mut app = menu_app_with(Menu {
        nav_off: true,
        ..Menu::default()
    });
    app.render_once();
    let t0 = Instant::now();
    app.press_key(KeyInput::Enter); // "One", the keyboard still works
    assert!(!app.gamepad_event(&button(GamepadButton::South, true), t0));
    assert!(!app.press_key_code(KeyCode::Escape, Default::default()));
    assert_eq!(app.state().pressed, ["One"]);
    assert_eq!(app.state().backs, 0);
}

// --- The HUD lets presses through ---

#[derive(Default)]
struct Hud {
    canvas: Vec<Point>,
    paused: u32,
}

#[test]
fn a_hud_lets_presses_through_to_the_game_except_on_its_controls() {
    let mut app = App::new(Hud::default(), |_: &Hud, cx: &mut Cx<Hud>| {
        let theme = *cx.theme();
        let game = Element::stack(Axis::Vertical, vec![]).grow(1.0).on_pointer(
            cx,
            |ev: &PointerEvent, s: &mut Hud| {
                if ev.kind == crate::core::PointerEventKind::Down {
                    s.canvas.push(ev.position);
                }
            },
        );
        let pause = Element::stack(Axis::Vertical, vec![])
            .width(40.0)
            .height(40.0)
            .on_tap(cx, |s: &mut Hud| s.paused += 1);
        let bar = stat_bar(&theme, &Bar::new(50.0, 100.0, theme.palette.danger));
        let overlay = Element::stack(Axis::Horizontal, vec![bar, crate::widgets::spacer(), pause])
            .align(crate::core::Align::Start, crate::core::Align::Start);
        hud(game, overlay)
    })
    .logical_size(Size::new(300.0, 200.0));
    app.render_once();
    let press = |x: f64, y: f64| Event::PointerButton {
        button: PointerButton::Left,
        state: ButtonState::Pressed,
        position: Point::new(x, y),
    };
    // The empty HUD and the health bar: the game.
    app.pointer_event(press(150.0, 150.0));
    app.pointer_event(press(20.0, 5.0));
    assert_eq!(app.state().canvas.len(), 2);
    // The pause button: the button.
    assert!(app.click_at(Point::new(280.0, 20.0)));
    assert_eq!(app.state().paused, 1);
}

#[test]
fn a_hit_through_overlay_takes_only_its_controls_presses() {
    #[derive(Default)]
    struct S {
        below: u32,
        on_top: u32,
    }
    let mut app = App::new(S::default(), |_: &S, cx: &mut Cx<S>| {
        let below = Element::stack(Axis::Vertical, vec![])
            .width(300.0)
            .height(200.0)
            .on_tap(cx, |s: &mut S| s.below += 1);
        let button = Element::stack(Axis::Vertical, vec![])
            .width(50.0)
            .height(50.0)
            .on_tap(cx, |s: &mut S| s.on_top += 1);
        let layer = Element::stack(Axis::Vertical, vec![button])
            .width(300.0)
            .height(200.0)
            .hit_through();
        cx.overlay(crate::core::OverlaySpec {
            content: layer,
            anchor: crate::core::Anchor::At(Point::new(0.0, 0.0)),
            modal: false,
            dismiss: None,
        });
        Element::stack(Axis::Vertical, vec![below]).grow(1.0)
    })
    .logical_size(Size::new(300.0, 200.0));
    app.render_once();
    assert!(app.click_at(Point::new(200.0, 150.0)));
    assert!(app.click_at(Point::new(20.0, 20.0)));
    assert_eq!((app.state().below, app.state().on_top), (1, 1));
}

// --- "Press a key…" ---

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Act {
    Jump,
    Fire,
}

fn key(k: PhysicalKey, code: KeyCode) -> KeyEvent {
    KeyEvent::new(ButtonState::Pressed, code, k)
}

#[test]
fn a_capture_binds_the_next_input_and_flags_conflicts() {
    let mut actions = Actions::new()
        .with(Act::Jump, Binding::Key(PhysicalKey::SPACE))
        .with(Act::Fire, Binding::Key(PhysicalKey::KEY_J));
    let mut input = InputState::new();
    let mut cap = RebindCapture::new(Act::Jump, 0);
    let t0 = Duration::from_secs(5);
    // The Enter that started it is let go.
    input.key(&key(PhysicalKey::ENTER, KeyCode::Enter));
    assert_eq!(cap.poll(&input, t0), Capture::Waiting);
    input.end_update();
    assert_eq!(cap.poll(&input, t0 + 16 * MS), Capture::Waiting);
    input.end_update();
    // J, which Fire has too.
    input.key(&key(PhysicalKey::KEY_J, KeyCode::Char('j')));
    let got = cap.poll(&input, t0 + 32 * MS);
    assert_eq!(got, Capture::Bound(Binding::Key(PhysicalKey::KEY_J)));
    let Capture::Bound(b) = got else {
        unreachable!()
    };
    assert_eq!(cap.apply(&mut actions, b), [Act::Fire]);
    assert_eq!(
        actions.bindings(Act::Jump),
        [Binding::Key(PhysicalKey::KEY_J)]
    );
    assert_eq!(conflicts(&actions, Act::Fire), [Act::Jump]);
    // A mouse button binds too, in a slot of its own.
    let mut cap = RebindCapture::new(Act::Jump, 1);
    input.end_update();
    cap.poll(&input, t0);
    input.button(PointerButton::Right, ButtonState::Pressed);
    let Capture::Bound(b) = cap.poll(&input, t0 + 16 * MS) else {
        panic!("a click binds");
    };
    cap.apply(&mut actions, b);
    assert_eq!(actions.bindings(Act::Jump).len(), 2);
    assert_eq!(binding_name(&actions.bindings(Act::Jump)[1]), "Right click");
}

#[test]
fn escape_or_waiting_too_long_cancels_a_capture() {
    let mut input = InputState::new();
    let t0 = Duration::from_secs(1);
    let mut cap = RebindCapture::new(Act::Fire, 0).timeout(Duration::from_secs(2));
    cap.poll(&input, t0);
    input.key(&key(PhysicalKey::ESCAPE, KeyCode::Escape));
    assert_eq!(cap.poll(&input, t0 + 16 * MS), Capture::Cancelled);
    let mut cap = RebindCapture::new(Act::Fire, 0).timeout(Duration::from_secs(2));
    let quiet = InputState::new();
    cap.poll(&quiet, t0);
    assert_eq!(cap.poll(&quiet, t0 + 1999 * MS), Capture::Waiting);
    assert_eq!(cap.poll(&quiet, t0 + 2000 * MS), Capture::Cancelled);
    // From a raw key handler, too.
    assert_eq!(
        cap.key_event(&key(PhysicalKey::ESCAPE, KeyCode::Escape)),
        Capture::Cancelled
    );
    assert_eq!(
        cap.key_event(&key(PhysicalKey::KEY_W, KeyCode::Char('w'))),
        Capture::Bound(Binding::Key(PhysicalKey::KEY_W))
    );
}

#[test]
fn a_capture_by_device_replaces_the_binding_of_the_same_kind() {
    let mut actions = Actions::new()
        .with(Act::Jump, Binding::Key(PhysicalKey::SPACE))
        .with(Act::Jump, Binding::GamepadButton(GamepadButton::South));
    let cap = RebindCapture::by_device(Act::Jump);
    cap.apply(&mut actions, Binding::GamepadButton(GamepadButton::North));
    cap.apply(&mut actions, Binding::Key(PhysicalKey::KEY_J));
    assert_eq!(
        actions.bindings(Act::Jump),
        [
            Binding::Key(PhysicalKey::KEY_J),
            Binding::GamepadButton(GamepadButton::North)
        ]
    );
    // None of that kind yet: added.
    let cap = RebindCapture::by_device(Act::Fire);
    cap.apply(&mut actions, Binding::MouseButton(PointerButton::Left));
    assert_eq!(
        actions.bindings(Act::Fire),
        [Binding::MouseButton(PointerButton::Left)]
    );
}

#[test]
fn bindings_have_short_names() {
    let k = |k| binding_name(&Binding::Key(k));
    assert_eq!(k(PhysicalKey::KEY_W), "W");
    assert_eq!(k(PhysicalKey::DIGIT_1), "1");
    assert_eq!(k(PhysicalKey::ARROW_LEFT), "\u{2190}");
    assert_eq!(k(PhysicalKey::SPACE), "Space");
    assert_eq!(k(PhysicalKey::SHIFT_LEFT), "L-Shift");
    assert_eq!(k(PhysicalKey::CONTROL_RIGHT), "R-Ctrl");
    assert_eq!(k(PhysicalKey::ESCAPE), "Esc");
    assert_eq!(
        binding_name(&Binding::KeyAxis {
            neg: PhysicalKey::KEY_Q,
            pos: PhysicalKey::KEY_E
        }),
        "Q / E"
    );
    assert_eq!(
        binding_name(&Binding::MouseButton(PointerButton::Left)),
        "Left click"
    );
    use crate::game::AxisDirection;
    use crate::platform::gamepad::{GamepadAxis, GamepadButton};
    assert_eq!(
        binding_name(&Binding::GamepadButton(GamepadButton::South)),
        "Pad A"
    );
    assert_eq!(
        binding_name(&Binding::GamepadButton(GamepadButton::DPadLeft)),
        "Pad \u{2190}"
    );
    assert_eq!(
        binding_name(&Binding::axis(GamepadAxis::LeftX, AxisDirection::Negative)),
        "L-Stick \u{2190}"
    );
    assert_eq!(
        binding_name(&Binding::axis(GamepadAxis::RightY, AxisDirection::Both)),
        "R-Stick \u{2195}"
    );
    assert_eq!(
        binding_name(&Binding::axis(
            GamepadAxis::LeftTrigger,
            AxisDirection::Positive
        )),
        "Pad LT"
    );
}

// --- Typewriter text ---

#[test]
fn a_typewriter_reveals_characters_over_frame_time_and_skips() {
    let mut tw = Typewriter::new("Héllo there").speed(20.0);
    assert_eq!(tw.len(), 11);
    let t0 = Duration::from_secs(3);
    assert_eq!(tw.shown_at(t0), 0, "the first frame starts the clock");
    assert_eq!(tw.visible_at(t0 + 49 * MS), "");
    assert_eq!(tw.visible_at(t0 + 50 * MS), "H");
    assert_eq!(tw.visible_at(t0 + 100 * MS), "Hé", "by character");
    assert_eq!(tw.visible_at(t0 + 250 * MS), "Héllo");
    assert!(!tw.is_done());
    // "Next" while it types shows the rest; again, it moves on.
    assert!(!tw.advance());
    assert_eq!(tw.visible_at(t0 + 260 * MS), "Héllo there");
    assert!(tw.advance());
    // Typed out on its own, it is done.
    let tw = Typewriter::new("Hi").speed(10.0);
    tw.shown_at(t0);
    assert_eq!(tw.visible_at(t0 + 200 * MS), "Hi");
    assert!(tw.is_done());
}

#[test]
fn a_dialogue_box_asks_for_frames_while_it_types() {
    let theme = Theme::light();
    let tw = Typewriter::new("One two three");
    let mut cx = Cx::<()>::new(&theme);
    cx.set_frame_time(Duration::from_secs(1));
    let _ = DialogueBox::new(&tw, |_| {})
        .speaker("Ana")
        .view(&mut cx, &theme);
    assert!(cx.frame_requested());
    let mut cx = Cx::<()>::new(&theme);
    cx.set_frame_time(Duration::from_secs(9));
    let _ = DialogueBox::new(&tw, |_| {})
        .speaker("Ana")
        .view(&mut cx, &theme);
    assert!(!cx.frame_requested(), "typed out: no more frames");
}

// --- The bar's ghost, at fixed frame times ---

/// Builds a view at chosen frame times and lowers it as the app does.
struct Rig {
    transitions: Transitions,
    states: HashMap<ElementKey, Interaction>,
}

impl Rig {
    fn new() -> Self {
        crate::anim::set_reduced_motion(Some(false));
        Self {
            transitions: Transitions::default(),
            states: HashMap::new(),
        }
    }

    fn build(&mut self, now: Duration, view: impl FnOnce(&mut Cx<()>) -> Element) -> Element {
        let theme = Theme::light();
        let mut cx = Cx::<()>::new(&theme);
        cx.set_frame_time(now);
        let mut el = view(&mut cx);
        self.transitions.begin(Some(now));
        let mut r = Resolver {
            states: &self.states,
            transitions: &mut self.transitions,
            watch: Default::default(),
            styled_actions: Vec::new(),
            styled_drops: Vec::new(),
            presence: HashMap::new(),
            anchors: Default::default(),
        };
        r.resolve(&mut el, ElementKey::root(0));
        self.transitions.end();
        el
    }
}

/// The ghost's and the fill's widths in a built [`stat_bar`].
fn widths(el: &mut Element) -> (f64, f64) {
    let layers = el.children_mut().unwrap();
    let w = |strip: &mut Element| strip.children_mut().unwrap()[0].layout.size.width.unwrap();
    let ghost = w(&mut layers[0]);
    let fill = w(&mut layers[1]);
    (ghost, fill)
}

#[test]
fn a_bar_going_down_leaves_a_ghost_that_lingers_then_drains() {
    let mut rig = Rig::new();
    let theme = Theme::light();
    let bar = |v: f32| move |_: &mut Cx<()>| stat_bar(&theme, &Bar::new(v, 100.0, Color::WHITE));
    let frame = crate::platform::frame::interval();
    let t0 = Duration::from_secs(10);
    let mut el = rig.build(t0, bar(100.0));
    assert_eq!(widths(&mut el), (196.0, 196.0));
    // Down to half: the fill gets there in 120 ms, the ghost takes 700.
    let head = t0 + 500 * MS - frame;
    rig.build(head + frame, bar(50.0));
    let (ghost, _) = widths(&mut rig.build(head + 60 * MS, bar(50.0)));
    assert!(ghost > 190.0, "it lingers first: {ghost}");
    let (ghost, fill) = widths(&mut rig.build(head + 130 * MS, bar(50.0)));
    assert_eq!(fill, 98.0, "the fill is there");
    assert!(ghost > 150.0, "{ghost}");
    let (ghost, _) = widths(&mut rig.build(head + 450 * MS, bar(50.0)));
    assert!(ghost > 98.0 && ghost < 140.0, "then it drains: {ghost}");
    let (ghost, fill) = widths(&mut rig.build(head + 701 * MS, bar(50.0)));
    assert_eq!((ghost, fill), (98.0, 98.0));
    assert!(!rig.transitions.animating());
    // Back up: the fill leads; the ghost, behind it, never shows.
    let head = head + 2000 * MS;
    rig.build(head + frame, bar(80.0));
    let (ghost, fill) = widths(&mut rig.build(head + 60 * MS, bar(80.0)));
    assert!(fill > ghost, "{fill} over {ghost}");
}

#[test]
fn floating_texts_go_after_their_life_and_counters_pop_on_change() {
    let mut texts = FloatingTexts::new();
    let t0 = Duration::from_secs(2);
    texts.spawn("+10", Color::WHITE, Point::new(10.0, 20.0), t0);
    texts.spawn("-5", Color::WHITE, Point::new(30.0, 20.0), t0 + 300 * MS);
    assert!(!texts.update(t0 + FLOAT_LIFE - MS));
    assert!(texts.update(t0 + FLOAT_LIFE), "the first goes");
    assert_eq!(texts.len(), 1);
    // Each is keyed, and leaves with an exit effect.
    let theme = Theme::light();
    let mut el = floating_texts(&theme, &texts);
    let item = &mut el.children_mut().unwrap()[0];
    let reactive = item.reactive.as_ref().unwrap();
    assert!(reactive.key.is_some());
    assert!(reactive.presence.as_ref().is_some_and(|p| p.exit.is_some()));
    // A counter's number is keyed by its value, so a new value enters.
    let key_of = |v: i64| {
        let mut c = counter(&theme, "score", "SCORE", v);
        c.children_mut().unwrap()[1].reactive.as_ref().unwrap().key
    };
    assert_ne!(key_of(1), key_of(2));
    assert_eq!(key_of(3), key_of(3));
}

#[test]
fn a_game_style_makes_a_theme_with_its_colors_and_focus_look() {
    let style = GameStyle::ember().font_size(20.0);
    let theme = Theme::game(&style);
    assert_eq!(theme.palette.primary, style.accent);
    assert_eq!(theme.palette.background, style.background);
    assert_eq!(theme.typography.body, 20.0);
    assert_eq!(theme.typography.heading, 40.0);
    assert_eq!(theme.focus, style.focus);
    assert!(theme.is_dark());
    assert!(GameStyle::neon().focus.glow > 0.0);
    assert!(
        GameStyle::neon()
            .font_bytes(b"not a font".to_vec())
            .is_err()
    );
}
