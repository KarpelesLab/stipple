//! The game loop in a few lines: a square moved with the arrow keys, WASD,
//! a gamepad's left stick or an on-screen touch stick (all through the same
//! rebindable actions), stepped at a fixed 120 Hz and drawn interpolated in a
//! canvas at the display's rate, with an fps counter.
//!
//! Each frame only the canvas repaints; the view is rebuilt when the fps
//! shown changes or the game pauses. Shift, the pad's A or the touch "Go"
//! button held speeds the square up (on a phone: the left thumb steers while
//! the right holds Go). P or Start pauses (the loop stops and asks for no
//! frames); the Resume button resumes. Minimizing the window pauses too.
//!
//! The touch controls show once a finger touches the screen, and hide again
//! on a key or a gamepad.
//!
//! With `--selftest` it runs 120 frames, prints the loop's stats and quits.

use std::cell::Cell;
use std::rc::Rc;
use stipple::core::CanvasId;
use stipple::game::{
    Actions, AxisDirection, Binding, Corner, FixedStep, Frame, GameLoop, TouchButton,
    TouchControls, TouchOverlay, TouchStick,
};
use stipple::gamepad::{GamepadAxis, GamepadButton};
use stipple::geometry::Vec2;
use stipple::prelude::*;

const WORLD: CanvasId = CanvasId(1);
const TOUCH: CanvasId = CanvasId(2);
const FIELD: Size = Size::new(480.0, 320.0);
const SIDE: f64 = 32.0;
/// Pixels per second.
const SPEED: f64 = 240.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Act {
    MoveX,
    MoveY,
    Boost,
    Pause,
}

struct Game {
    actions: Actions<Act>,
    step: FixedStep,
    /// The square's top-left corner before and after the last step.
    prev: Point,
    pos: Point,
    /// Where the canvas draws it (interpolated between steps).
    drawn: Rc<Cell<Point>>,
    fps: u32,
    paused: bool,
    selftest: bool,
    touch: TouchOverlay,
}

fn actions() -> Actions<Act> {
    use PhysicalKey as K;
    let key = Binding::Key;
    let keys = |neg, pos| Binding::KeyAxis { neg, pos };
    // The stick's axes without a dead zone of their own: `stick` applies a
    // radial one to both together.
    let stick = |axis| Binding::GamepadAxis {
        axis,
        direction: AxisDirection::Both,
        dead_zone: 0.0,
    };
    Actions::new()
        .with(Act::MoveX, keys(K::ARROW_LEFT, K::ARROW_RIGHT))
        .with(Act::MoveX, keys(K::KEY_A, K::KEY_D))
        .with(Act::MoveX, stick(GamepadAxis::LeftX))
        .with(Act::MoveY, keys(K::ARROW_UP, K::ARROW_DOWN))
        .with(Act::MoveY, keys(K::KEY_W, K::KEY_S))
        .with(Act::MoveY, stick(GamepadAxis::LeftY))
        .with(Act::Boost, key(K::SHIFT_LEFT))
        .with(Act::Boost, Binding::GamepadButton(GamepadButton::South))
        .with(Act::Pause, key(K::KEY_P))
        .with(Act::Pause, Binding::GamepadButton(GamepadButton::Start))
}

/// A stick for the left thumb and two buttons for the right one; they are
/// the gamepad's left stick, A and Start.
fn touch_controls() -> TouchControls {
    TouchControls::new()
        .with_stick(TouchStick::left().at(Corner::BottomLeft, Vec2::new(90.0, 90.0)))
        .with_button(TouchButton::new(GamepadButton::South, "Go"))
        .with_button(
            TouchButton::new(GamepadButton::Start, "II")
                .at(Corner::TopRight, Vec2::new(36.0, 36.0))
                .radius(22.0),
        )
}

fn update(g: &mut Game, frame: &mut Frame) {
    g.actions.update(frame.input());
    if g.actions.pressed(Act::Pause) {
        g.paused = true;
        frame.rebuild();
        return;
    }
    // The touch controls' look follows their state (and showing, hiding).
    g.touch.sync(frame);
    let (dx, dy) = g.actions.stick(Act::MoveX, Act::MoveY, 0.2);
    let boost = if g.actions.held(Act::Boost) { 2.0 } else { 1.0 };
    for _ in 0..g.step.advance(frame.dt()) {
        g.prev = g.pos;
        let d = SPEED * boost * g.step.dt_secs();
        g.pos.x = (g.pos.x + f64::from(dx) * d).clamp(0.0, FIELD.width - SIDE);
        g.pos.y = (g.pos.y + f64::from(dy) * d).clamp(0.0, FIELD.height - SIDE);
    }
    let a = g.step.alpha();
    g.drawn.set(Point::new(
        g.prev.x + (g.pos.x - g.prev.x) * a,
        g.prev.y + (g.pos.y - g.prev.y) * a,
    ));
    frame.repaint(WORLD);
    // The label only changes when the fps shown does.
    let fps = frame.stats().fps.round() as u32;
    if fps != g.fps {
        g.fps = fps;
        frame.rebuild();
    }
    if g.selftest && frame.index() == 120 {
        let s = frame.stats();
        println!(
            "gameloop: {} frames, {:.1} fps, update {:?}, render {:?}",
            s.frames, s.fps, s.update_time, s.render_time
        );
        std::process::exit(0);
    }
}

fn view(g: &Game, cx: &mut Cx<Game>) -> Element {
    let theme = *cx.theme();
    let drawn = g.drawn.clone();
    let world = Element::canvas(WORLD, move |c: &mut Canvas| {
        let size = c.size();
        c.fill_rect(
            Rect::from_xywh(0.0, 0.0, size.width, size.height),
            Color::rgb(0x14, 0x18, 0x24),
        );
        let at = drawn.get();
        c.fill_rect(
            Rect::from_xywh(at.x, at.y, SIDE, SIDE),
            Color::rgb(0xf0, 0x80, 0x40),
        );
    })
    .width(FIELD.width)
    .height(FIELD.height);
    let status = if g.paused {
        row(vec![
            label(&theme, "Paused"),
            button_labeled(&theme, "Resume").on_tap(cx, |g: &mut Game| g.paused = false),
        ])
    } else {
        label(
            &theme,
            format!(
                "{} fps — arrows, WASD, a pad or touch to move; Shift or A to speed up; P to pause",
                g.fps
            ),
        )
    };
    // The touch controls over everything, covering the window.
    Element::layers(vec![
        column(vec![status, world])
            .padding(Insets::uniform(12.0))
            .gap(8.0),
        g.touch.element(&theme),
    ])
}

fn main() {
    let game = Game {
        actions: actions(),
        step: FixedStep::hz(120),
        prev: Point::new(0.0, 0.0),
        pos: Point::new(0.0, 0.0),
        drawn: Rc::new(Cell::new(Point::new(0.0, 0.0))),
        fps: 0,
        paused: false,
        selftest: std::env::args().any(|a| a == "--selftest"),
        touch: TouchOverlay::new(TOUCH).font(Font::system_default()),
    };
    App::new(game, view)
        .title("Game loop")
        .logical_size(Size::new(504.0, 384.0))
        .game_loop(
            GameLoop::new(update)
                .paused_when(|g: &Game| g.paused)
                .touch_controls(touch_controls()),
        )
        .run();
}
