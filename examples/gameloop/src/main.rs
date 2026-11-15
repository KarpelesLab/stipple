//! The game loop in a few lines: a square moved with the arrow keys or WASD
//! (through rebindable actions), stepped at a fixed 120 Hz and drawn
//! interpolated in a canvas at the display's rate, with an fps counter.
//!
//! Each frame only the canvas repaints; the view is rebuilt when the fps
//! shown changes or the game pauses. P pauses (the loop stops and asks for no
//! frames); the Resume button resumes. Minimizing the window pauses too.
//!
//! With `--selftest` it runs 120 frames, prints the loop's stats and quits.

use std::cell::Cell;
use std::rc::Rc;
use stipple::core::CanvasId;
use stipple::game::{Actions, Binding, FixedStep, Frame, GameLoop};
use stipple::prelude::*;

const WORLD: CanvasId = CanvasId(1);
const FIELD: Size = Size::new(480.0, 320.0);
const SIDE: f64 = 32.0;
/// Pixels per second.
const SPEED: f64 = 240.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Act {
    Left,
    Right,
    Up,
    Down,
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
}

fn actions() -> Actions<Act> {
    use PhysicalKey as K;
    let key = Binding::Key;
    Actions::new()
        .with(Act::Left, key(K::ARROW_LEFT))
        .with(Act::Left, key(K::KEY_A))
        .with(Act::Right, key(K::ARROW_RIGHT))
        .with(Act::Right, key(K::KEY_D))
        .with(Act::Up, key(K::ARROW_UP))
        .with(Act::Up, key(K::KEY_W))
        .with(Act::Down, key(K::ARROW_DOWN))
        .with(Act::Down, key(K::KEY_S))
        .with(Act::Pause, key(K::KEY_P))
}

fn update(g: &mut Game, frame: &mut Frame) {
    g.actions.update(frame.input());
    if g.actions.pressed(Act::Pause) {
        g.paused = true;
        frame.rebuild();
        return;
    }
    let (dx, dy) = g.actions.axis2(Act::Left, Act::Right, Act::Up, Act::Down);
    for _ in 0..g.step.advance(frame.dt()) {
        g.prev = g.pos;
        let d = SPEED * g.step.dt_secs();
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
            format!("{} fps — arrows or WASD to move, P to pause", g.fps),
        )
    };
    column(vec![status, world])
        .padding(Insets::uniform(12.0))
        .gap(8.0)
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
    };
    App::new(game, view)
        .title("Game loop")
        .logical_size(Size::new(504.0, 384.0))
        .game_loop(GameLoop::new(update).paused_when(|g: &Game| g.paused))
        .run();
}
