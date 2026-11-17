//! Gamepad, touch and locked-pointer input driven through the App's event
//! handler, as a backend delivers it, into a game's actions and its touch
//! overlay.

use super::{
    Actions, AxisDirection, Binding, Corner, Frame, GameLoop, TouchButton, TouchControls,
    TouchOverlay, TouchStick,
};
use crate::App;
use crate::core::{CanvasId, Cx, Element};
use crate::gamepad::{GamepadAxis, GamepadButton, GamepadEvent, GamepadId, GamepadInfo};
use crate::geometry::{PhysicalSize, Point, ScaleFactor, Size, Vec2};
use crate::platform::backend::headless::HeadlessSurface;
use crate::platform::{
    ButtonState, Event, KeyCode, KeyEvent, PhysicalKey, PointerButton, Touch, TouchPhase, Window,
    WindowId, WindowInfo,
};
use crate::render::Surface;
use std::cell::RefCell;
use std::rc::Rc;

const TOUCH: CanvasId = CanvasId(31);

struct FakeWindow;

impl Window for FakeWindow {
    fn id(&self) -> WindowId {
        WindowId(100)
    }
    fn inner_size(&self) -> PhysicalSize {
        PhysicalSize::new(200, 100)
    }
    fn scale_factor(&self) -> ScaleFactor {
        ScaleFactor::IDENTITY
    }
    fn request_redraw(&self) {}
    fn set_title(&self, _: &str) {}
    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(HeadlessSurface::new(self.inner_size()).0)
    }
    fn info(&self) -> WindowInfo {
        WindowInfo::from_size(self.inner_size(), self.scale_factor())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Act {
    Jump,
    MoveX,
    MoveY,
}

/// What each update saw.
#[derive(Default)]
struct Log {
    jumps: u32,
    jump_held: Vec<bool>,
    stick: Vec<(f32, f32)>,
    motion: Vec<Vec2>,
    overlay_repaints: u32,
}

struct Game {
    actions: Actions<Act>,
    touch: TouchOverlay,
    log: Rc<RefCell<Log>>,
}

fn view(g: &Game, cx: &mut Cx<Game>) -> Element {
    let theme = *cx.theme();
    Element::layers(vec![
        Element::canvas(CanvasId(30), |_| {}),
        g.touch.element(&theme),
    ])
}

fn update(g: &mut Game, frame: &mut Frame) {
    g.actions.update(frame.input());
    let mut log = g.log.borrow_mut();
    log.jumps += u32::from(g.actions.pressed(Act::Jump));
    log.jump_held.push(g.actions.held(Act::Jump));
    log.stick.push(g.actions.stick(Act::MoveX, Act::MoveY, 0.1));
    log.motion.push(frame.input().motion());
    if g.touch.sync(frame) {
        log.overlay_repaints += 1;
    }
}

/// The frame clock delivers the frame asked for (see `app_tests`).
fn deliver() -> bool {
    crate::platform::frame::deliver_in_test()
}

fn finger(id: u64, phase: TouchPhase, x: f64, y: f64) -> Event {
    Event::Touch(Touch {
        id,
        phase,
        position: Point::new(x, y),
    })
}

#[test]
fn pads_touch_controls_and_the_locked_pointer_reach_the_actions() {
    let _lock = crate::platform::frame::SUSPEND_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    crate::platform::frame::set_suspended(false);
    let log = Rc::new(RefCell::new(Log::default()));
    let full = |axis| Binding::GamepadAxis {
        axis,
        direction: AxisDirection::Both,
        dead_zone: 0.0,
    };
    let game = Game {
        actions: Actions::new()
            .with(Act::Jump, Binding::GamepadButton(GamepadButton::South))
            .with(Act::Jump, Binding::Key(PhysicalKey::SPACE))
            .with(Act::MoveX, full(GamepadAxis::LeftX))
            .with(Act::MoveY, full(GamepadAxis::LeftY)),
        touch: TouchOverlay::new(TOUCH),
        log: log.clone(),
    };
    // A 200×100 window: the stick floats over the left half, A sits at
    // (160, 60).
    let controls = TouchControls::new()
        .stick(TouchStick::left().radius(40.0).dead_zone(0.0))
        .button(
            TouchButton::new(GamepadButton::South, "A")
                .at(Corner::BottomRight, Vec2::new(40.0, 40.0))
                .radius(20.0),
        );
    let mut app = App::new(game, view)
        .no_font()
        .logical_size(Size::new(200.0, 100.0))
        .game_loop(GameLoop::new(update).touch_controls(controls));
    // Locked from the start, as a platform granting it would.
    assert!(app.set_pointer_locked(true, true));
    let (_, mut handler) = app.into_handler();
    crate::platform::dialog::set_event_loop_running(false);
    let window = FakeWindow;
    let mut send = |event: Event| handler(event, &window);
    // Deliver a frame and run its update.
    macro_rules! frame {
        () => {
            assert!(deliver(), "a frame was asked for");
            send(Event::Wake);
        };
    }
    let pad = GamepadId(41);
    let button = |button, pressed| {
        Event::Gamepad(GamepadEvent::Button {
            id: pad,
            button,
            pressed,
        })
    };
    let axis = |axis, value| {
        Event::Gamepad(GamepadEvent::Axis {
            id: pad,
            axis,
            value,
        })
    };
    let key = |state| Event::RawKey(KeyEvent::new(state, KeyCode::Char(' '), PhysicalKey::SPACE));

    send(Event::RedrawRequested);
    send(Event::Gamepad(GamepadEvent::Connected {
        id: pad,
        info: GamepadInfo::new("pad"),
    }));
    // A tap of A between two frames jumps once.
    send(button(GamepadButton::South, true));
    send(button(GamepadButton::South, false));
    frame!();
    assert_eq!(log.borrow().jumps, 1);
    assert_eq!(log.borrow().jump_held.last(), Some(&false));
    // The stick, through the actions' radial dead zone (0.1).
    send(axis(GamepadAxis::LeftX, 0.6));
    frame!();
    let (x, y) = *log.borrow().stick.last().unwrap();
    assert!((x - 5.0 / 9.0).abs() < 1e-5 && y == 0.0, "{x}");
    send(axis(GamepadAxis::LeftX, 0.0));

    // The locked pointer: the warp to the center isn't motion, deltas are.
    send(Event::PointerMoved {
        position: Point::new(100.0, 50.0),
    });
    send(Event::PointerDelta { dx: 4.0, dy: -1.0 });
    send(Event::PointerDelta { dx: 1.0, dy: 0.0 });
    frame!();
    assert_eq!(log.borrow().motion.last(), Some(&Vec2::new(5.0, -1.0)));

    // Touch: hidden until a finger lands, then the left thumb steers while
    // the right one holds A. The first finger's pointer events (as backends
    // send them) are no mouse click.
    assert_eq!(log.borrow().overlay_repaints, 1, "synced once, hidden");
    send(finger(1, TouchPhase::Started, 50.0, 50.0));
    send(Event::PointerButton {
        button: PointerButton::Left,
        state: ButtonState::Pressed,
        position: Point::new(50.0, 50.0),
    });
    send(finger(1, TouchPhase::Moved, 30.0, 50.0));
    send(finger(2, TouchPhase::Started, 165.0, 62.0));
    frame!();
    {
        let log = log.borrow();
        assert_eq!(log.jumps, 2, "the touch button is Pad:South");
        assert_eq!(log.jump_held.last(), Some(&true));
        // -20 of 40 is -0.5, then the radial dead zone.
        let (x, y) = *log.stick.last().unwrap();
        assert!((x + 4.0 / 9.0).abs() < 1e-5 && y == 0.0, "{x}");
        assert_eq!(log.overlay_repaints, 2, "shown, so repainted");
    }
    frame!();
    assert_eq!(log.borrow().overlay_repaints, 2, "nothing changed");
    // Both lift.
    send(finger(2, TouchPhase::Ended, 165.0, 62.0));
    send(finger(1, TouchPhase::Ended, 30.0, 50.0));
    send(Event::PointerButton {
        button: PointerButton::Left,
        state: ButtonState::Released,
        position: Point::new(30.0, 50.0),
    });
    frame!();
    {
        let log = log.borrow();
        assert_eq!(log.jump_held.last(), Some(&false));
        assert_eq!(*log.stick.last().unwrap(), (0.0, 0.0));
        assert_eq!(log.overlay_repaints, 3);
    }
    // A key hides the controls again.
    send(key(ButtonState::Pressed));
    frame!();
    assert_eq!(log.borrow().jumps, 3);
    assert_eq!(log.borrow().overlay_repaints, 4, "hidden");
    // The pad going away while held: released.
    send(button(GamepadButton::South, true));
    send(key(ButtonState::Released));
    frame!();
    assert_eq!(log.borrow().jump_held.last(), Some(&true));
    send(Event::Gamepad(GamepadEvent::Disconnected { id: pad }));
    frame!();
    assert_eq!(log.borrow().jump_held.last(), Some(&false));
}
