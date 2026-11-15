//! The game loop driven through the App's event handler, as a backend
//! drives it: an update per delivered frame, canvas-only frames without a
//! rebuild, pausing, and input fed from the window's events.

use super::{Frame, GameLoop};
use crate::App;
use crate::core::{CanvasId, Cx, Element};
use crate::geometry::{PhysicalSize, Rect, ScaleFactor, Size};
use crate::platform::backend::headless::HeadlessSurface;
use crate::platform::{
    ButtonState, ControlFlow, Event, KeyCode, KeyEvent, PhysicalKey, Window, WindowId, WindowInfo,
};
use crate::render::{Canvas, Color, Surface};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const WORLD: CanvasId = CanvasId(9);

struct FakeWindow {
    minimized: Cell<bool>,
}

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
        let mut info = WindowInfo::from_size(self.inner_size(), self.scale_factor());
        info.minimized = self.minimized.get();
        info
    }
}

/// What the view and the update saw.
#[derive(Default)]
struct Log {
    builds: u32,
    paints: u32,
    frames: u32,
    dts: Vec<Duration>,
    spaces: u32,
    left_released: u32,
}

struct Game {
    log: Rc<RefCell<Log>>,
    paused: bool,
}

fn view(g: &Game, _: &mut Cx<Game>) -> Element {
    g.log.borrow_mut().builds += 1;
    let log = g.log.clone();
    Element::canvas(WORLD, move |c: &mut Canvas| {
        log.borrow_mut().paints += 1;
        c.fill_rect(Rect::from_xywh(0.0, 0.0, 10.0, 10.0), Color::BLACK);
    })
    .width(100.0)
    .height(100.0)
}

fn update(g: &mut Game, frame: &mut Frame) {
    let mut log = g.log.borrow_mut();
    log.frames += 1;
    log.dts.push(frame.dt());
    if frame.input().key_pressed(PhysicalKey::SPACE) {
        log.spaces += 1;
        frame.rebuild(); // the view shows something that changed
    }
    if frame.input().key_released(PhysicalKey::ARROW_LEFT) {
        log.left_released += 1;
    }
    frame.repaint(WORLD);
}

fn key(state: ButtonState, physical: PhysicalKey) -> Event {
    Event::RawKey(KeyEvent::new(state, KeyCode::Unidentified(0), physical))
}

/// The frame clock delivers the frame asked for. Nothing else delivers
/// frames in tests. (Each delivery is a second later than the last, past
/// the clock's pacing.)
fn deliver() -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LATER: AtomicU64 = AtomicU64::new(10);
    let later = Duration::from_secs(LATER.fetch_add(1, Ordering::Relaxed));
    crate::platform::frame::take_due(crate::platform::time::Instant::now() + later)
}

#[test]
fn the_loop_updates_per_frame_repaints_without_builds_and_pauses() {
    let _lock = crate::platform::frame::SUSPEND_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    crate::platform::frame::set_suspended(false);
    let log = Rc::new(RefCell::new(Log::default()));
    let app = App::new(
        Game {
            log: log.clone(),
            paused: false,
        },
        view,
    )
    .logical_size(Size::new(200.0, 100.0))
    .game_loop(GameLoop::new(update).paused_when(|g: &Game| g.paused));
    let requests = app.game.as_ref().unwrap().requests.clone();
    let handle = app.handle();
    let (_, mut handler) = app.into_handler();
    // No real event loop runs: keep native alerts off for the tests that
    // check the drawn ones.
    crate::platform::dialog::set_event_loop_running(false);
    let window = FakeWindow {
        minimized: Cell::new(false),
    };
    let mut send = |event: Event| handler(event, &window);
    let frames = |log: &Rc<RefCell<Log>>| log.borrow().frames;

    send(Event::RedrawRequested);
    assert_eq!(log.borrow().builds, 1);
    assert!(requests.get() > 0, "a running loop asks for a frame");
    // A wake that isn't a frame (an update, a timer) runs no update.
    handle.update(|_| {});
    send(Event::Wake);
    assert_eq!(frames(&log), 0);
    assert_eq!(log.borrow().builds, 2, "the update rebuilt the view");

    // Each frame runs the update once; only the canvas repaints.
    for n in 1..=3 {
        let painted = log.borrow().paints;
        assert!(deliver(), "frame {n} was asked for");
        send(Event::Wake);
        send(Event::Wake); // a second wake in the same frame: nothing
        assert_eq!(frames(&log), n);
        assert!(log.borrow().paints > painted, "the canvas repainted");
    }
    assert_eq!(
        log.borrow().builds,
        2,
        "canvas-only frames don't rebuild the view"
    );
    assert_eq!(log.borrow().dts[0], crate::platform::frame::interval());

    // Input reaches the update; it asks for a rebuild.
    send(key(ButtonState::Pressed, PhysicalKey::SPACE));
    send(key(ButtonState::Released, PhysicalKey::SPACE));
    deliver();
    send(Event::Wake);
    assert_eq!(log.borrow().spaces, 1, "a tap between frames is seen");
    let builds = log.borrow().builds;
    assert!(builds > 2, "the update asked for a rebuild");
    deliver();
    send(Event::Wake);
    assert_eq!(log.borrow().spaces, 1, "once");
    assert_eq!(log.borrow().builds, builds);

    // Paused from the state: no frames asked for, no updates.
    handle.update(|g| g.paused = true);
    send(Event::Wake);
    let (asked, ran) = (requests.get(), frames(&log));
    for _ in 0..3 {
        deliver();
        send(Event::Wake);
        send(key(ButtonState::Pressed, PhysicalKey::SPACE));
        send(key(ButtonState::Released, PhysicalKey::SPACE));
    }
    assert_eq!(requests.get(), asked, "paused: no frames");
    assert_eq!(frames(&log), ran);

    // Resumed: a normal dt, and the presses made while paused are gone.
    handle.update(|g| g.paused = false);
    send(Event::Wake);
    assert!(requests.get() > asked);
    assert!(deliver());
    send(Event::Wake);
    assert_eq!(frames(&log), ran + 1);
    assert_eq!(
        *log.borrow().dts.last().unwrap(),
        crate::platform::frame::interval()
    );
    assert_eq!(log.borrow().spaces, 1);

    // Minimized: paused until restored.
    window.minimized.set(true);
    send(Event::WindowChanged);
    let asked = requests.get();
    deliver();
    send(Event::Wake);
    assert_eq!(requests.get(), asked);
    assert_eq!(frames(&log), ran + 1);
    window.minimized.set(false);
    send(Event::WindowChanged);
    assert!(deliver());
    send(Event::Wake);
    assert_eq!(frames(&log), ran + 2);

    // Focus loss lets go of a held key: the update sees it released.
    send(key(ButtonState::Pressed, PhysicalKey::ARROW_LEFT));
    deliver();
    send(Event::Wake);
    send(Event::FocusChanged(false));
    deliver();
    send(Event::Wake);
    assert_eq!(log.borrow().left_released, 1);
    assert_eq!(send(Event::Wake), ControlFlow::Wait);
}

#[test]
fn an_update_can_run_outside_the_loop() {
    let input = super::InputState::new();
    let mut frame = Frame::new(Duration::from_secs(1), Duration::from_millis(16), &input);
    let mut g = Game {
        log: Default::default(),
        paused: false,
    };
    update(&mut g, &mut frame);
    assert_eq!(frame.repaints().collect::<Vec<_>>(), [(WORLD, None)]);
    assert!(!frame.rebuilds());
}
