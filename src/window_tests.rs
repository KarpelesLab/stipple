//! Runtime windows, titles and geometry, driven through the App's event
//! handler with a pretend multi-window backend.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::App;
use crate::core::{Cx, Element};
use crate::geometry::{PhysicalSize, Point, ScaleFactor, Size};
use crate::platform::backend::headless::HeadlessSurface;
use crate::platform::{
    ButtonState, ControlFlow, Event, PointerButton, Window, WindowAttributes, WindowId, WindowInfo,
    WindowRequest,
};
use crate::render::Surface;

/// The pretend desktop: which windows are open, what they were told, and
/// what the "system" reports of each.
#[derive(Default)]
struct Desk {
    next: Cell<u64>,
    open: RefCell<Vec<u64>>,
    log: RefCell<Vec<String>>,
    info: RefCell<HashMap<u64, WindowInfo>>,
}

impl Desk {
    fn new() -> Rc<Desk> {
        let desk = Rc::new(Desk::default());
        desk.next.set(100);
        desk.open.borrow_mut().push(100);
        desk
    }

    fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut self.log.borrow_mut())
    }
}

struct FakeWindow {
    id: u64,
    desk: Rc<Desk>,
}

impl Window for FakeWindow {
    fn id(&self) -> WindowId {
        WindowId(self.id)
    }
    fn inner_size(&self) -> PhysicalSize {
        PhysicalSize::new(200, 100)
    }
    fn scale_factor(&self) -> ScaleFactor {
        ScaleFactor::IDENTITY
    }
    fn request_redraw(&self) {}
    fn set_title(&self, title: &str) {
        self.desk
            .log
            .borrow_mut()
            .push(format!("title {} {title}", self.id));
    }
    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(HeadlessSurface::new(self.inner_size()).0)
    }
    fn open_window(&self, attrs: WindowAttributes) -> Option<WindowId> {
        let id = self.desk.next.get() + 1;
        self.desk.next.set(id);
        self.desk.open.borrow_mut().push(id);
        self.desk
            .log
            .borrow_mut()
            .push(format!("open {id} {} focus={}", attrs.title, attrs.focus));
        Some(WindowId(id))
    }
    fn close_window(&self) {
        self.desk.open.borrow_mut().retain(|w| *w != self.id);
        self.desk
            .log
            .borrow_mut()
            .push(format!("close {}", self.id));
    }
    fn set_visible(&self, visible: bool) {
        self.desk
            .log
            .borrow_mut()
            .push(format!("visible {} {visible}", self.id));
    }
    fn info(&self) -> WindowInfo {
        self.desk
            .info
            .borrow()
            .get(&self.id)
            .copied()
            .unwrap_or_else(|| WindowInfo::from_size(self.inner_size(), self.scale_factor()))
    }
    fn set_position(&self, at: Point) {
        self.desk
            .log
            .borrow_mut()
            .push(format!("position {} {},{}", self.id, at.x, at.y));
    }
    fn set_inner_size(&self, size: Size) {
        self.desk
            .log
            .borrow_mut()
            .push(format!("size {} {}x{}", self.id, size.width, size.height));
    }
    fn set_resize_increments(&self, step: Option<Size>) {
        self.desk
            .log
            .borrow_mut()
            .push(format!("increments {} {step:?}", self.id));
    }
}

/// Run the app's handler as a multi-window backend would.
struct Run<H> {
    desk: Rc<Desk>,
    handler: H,
}

impl<H: FnMut(Event, &dyn Window) -> ControlFlow> Run<H> {
    fn send(&mut self, id: u64, event: Event) -> ControlFlow {
        let window = FakeWindow {
            id,
            desk: self.desk.clone(),
        };
        (self.handler)(event, &window)
    }

    /// A wake, delivered to every open window in turn.
    fn wake(&mut self) -> ControlFlow {
        let open = self.desk.open.borrow().clone();
        for id in open {
            if self.send(id, Event::Wake) == ControlFlow::Exit {
                return ControlFlow::Exit;
            }
        }
        ControlFlow::Wait
    }
}

fn run<S: 'static>(app: App<S>) -> Run<impl FnMut(Event, &dyn Window) -> ControlFlow> {
    let (_, handler) = app.into_handler();
    let mut run = Run {
        desk: Desk::new(),
        handler,
    };
    run.send(100, Event::RedrawRequested);
    run
}

struct Doc {
    name: String,
    clicks: u32,
}

fn doc() -> Doc {
    Doc {
        name: "a.txt".into(),
        clicks: 0,
    }
}

/// A view titled after the document, that counts clicks anywhere in it.
fn titled(s: &Doc, cx: &mut Cx<Doc>) -> Element {
    cx.window_title(format!("{} ({})", s.name, s.clicks));
    crate::widgets::spacer()
        .grow(1.0)
        .on_tap(cx, |s: &mut Doc| s.clicks += 1)
}

#[test]
fn the_view_titles_its_window_when_the_title_changes() {
    let app = App::new(doc(), titled);
    let handle = app.handle();
    let mut run = run(app);
    assert_eq!(run.desk.take_log(), ["title 100 a.txt (0)"]);
    // The same title isn't sent again.
    run.send(100, Event::RedrawRequested);
    assert!(run.desk.take_log().is_empty());
    handle.update(|s| s.name = "b.txt".into());
    run.wake();
    assert_eq!(run.desk.take_log(), ["title 100 b.txt (0)"]);
    // A request retitles it directly.
    handle.window(WindowRequest::Title("Renamed".into()));
    run.wake();
    assert_eq!(run.desk.take_log(), ["title 100 Renamed"]);
}

#[test]
fn windows_open_and_close_at_run_time() {
    let seen: Arc<Mutex<Vec<WindowId>>> = Default::default();
    let app = App::new(doc(), titled);
    let handle = app.handle();
    let mut run = run(app);
    run.desk.take_log();

    // One view for any number of windows, told which it builds.
    let saw = seen.clone();
    let id = handle.open_window(
        WindowAttributes::new()
            .with_title("Second")
            .with_focus(false),
        move |s: &Doc, cx: &mut Cx<Doc>| {
            saw.lock().unwrap().push(cx.window_id());
            cx.window_title(format!("second: {}", s.clicks));
            crate::widgets::spacer()
        },
    );
    assert_ne!(id, WindowId::PRIMARY);
    run.wake();
    assert_eq!(run.desk.take_log(), ["open 101 Second focus=false"]);
    assert_eq!(handle.windows(), [WindowId::PRIMARY, id]);
    assert!(handle.window_info(id).is_some());
    run.send(101, Event::RedrawRequested);
    assert_eq!(run.desk.take_log(), ["title 101 second: 0"]);
    assert_eq!(*seen.lock().unwrap(), [id]);

    // Requests go to their own window.
    handle.window_to(id, WindowRequest::Position(Point::new(10.0, 20.0)));
    handle.window_to(id, WindowRequest::InnerSize(Size::new(300.0, 200.0)));
    handle.window(WindowRequest::ResizeIncrements(Some(Size::new(8.0, 16.0))));
    run.wake();
    let log = run.desk.take_log();
    assert!(log.contains(&"position 101 10,20".to_string()), "{log:?}");
    assert!(log.contains(&"size 101 300x200".to_string()), "{log:?}");
    assert!(
        log.iter()
            .any(|l| l.starts_with("increments 100 Some(Size")),
        "{log:?}"
    );

    // Closing it at run time leaves the app running with the first.
    handle.close_window(id);
    assert_eq!(run.wake(), ControlFlow::Wait);
    assert_eq!(run.desk.take_log(), ["close 101"]);
    assert_eq!(handle.windows(), [WindowId::PRIMARY]);
    assert_eq!(handle.window_info(id), None);
    // Requests for it are dropped; closing it again does nothing.
    handle.window_to(id, WindowRequest::Title("gone".into()));
    handle.close_window(id);
    assert_eq!(run.wake(), ControlFlow::Wait);
    assert!(run.desk.take_log().is_empty());

    // Closing the last window ends the app.
    assert_eq!(run.send(100, Event::CloseRequested), ControlFlow::Exit);
}

#[test]
fn closing_the_primary_window_keeps_the_others() {
    let mut app = App::new(doc(), titled);
    let other = app.open_window(WindowAttributes::new().with_title("Other"), titled);
    let handle = app.handle();
    let mut run = run(app);
    assert!(
        run.desk
            .take_log()
            .contains(&"open 101 Other focus=true".to_string())
    );
    assert_eq!(handle.windows(), [WindowId::PRIMARY, other]);
    assert_eq!(run.send(100, Event::CloseRequested), ControlFlow::Wait);
    assert_eq!(run.desk.take_log(), ["close 100"]);
    assert_eq!(handle.windows(), [other]);
    assert_eq!(run.send(101, Event::CloseRequested), ControlFlow::Exit);
}

#[test]
fn the_app_can_keep_running_with_no_window() {
    let app = App::new(doc(), titled).exit_on_last_window_closed(false);
    let handle = app.handle();
    let mut run = run(app);
    run.desk.take_log();
    // The last window hides instead of closing.
    assert_eq!(run.send(100, Event::CloseRequested), ControlFlow::Wait);
    assert_eq!(run.desk.take_log(), ["visible 100 false"]);
    assert!(handle.windows().is_empty());
    // Timers still run, and new windows open from the hidden one.
    handle.update(|s| s.clicks = 5);
    let id = handle.open_window(WindowAttributes::new().with_title("Again"), titled);
    run.wake();
    assert_eq!(run.desk.take_log(), ["open 101 Again focus=true"]);
    run.send(101, Event::RedrawRequested);
    assert_eq!(run.desk.take_log(), ["title 101 a.txt (5)"]);
    // Closing that one really closes it: the hidden window keeps the loop.
    handle.close_window(id);
    assert_eq!(run.wake(), ControlFlow::Wait);
    assert_eq!(run.desk.take_log(), ["close 101"]);
    handle.exit();
    assert_eq!(run.wake(), ControlFlow::Exit);
}

#[test]
fn a_change_in_one_window_reaches_the_others() {
    let mut app = App::new(doc(), titled).logical_size(Size::new(200.0, 100.0));
    app.open_window(WindowAttributes::new().with_title("Other"), titled);
    let mut run = run(app);
    run.send(101, Event::RedrawRequested);
    run.desk.take_log();
    // A click in the first window changes the state...
    let at = Point::new(50.0, 50.0);
    for state in [ButtonState::Pressed, ButtonState::Released] {
        run.send(
            100,
            Event::PointerButton {
                button: PointerButton::Left,
                state,
                position: at,
            },
        );
    }
    assert_eq!(run.desk.take_log(), ["title 100 a.txt (1)"]);
    // ...and the other window follows on the wake that requests.
    run.wake();
    assert_eq!(run.desk.take_log(), ["title 101 a.txt (1)"]);
}

#[test]
fn window_changes_reach_the_state_and_views_that_read_them() {
    let changes: Rc<RefCell<Vec<(WindowId, bool)>>> = Default::default();
    let log = changes.clone();
    let app = App::new(doc(), |_: &Doc, cx: &mut Cx<Doc>| {
        let info = cx.window_info();
        cx.window_title(if info.maximized { "max" } else { "normal" });
        crate::widgets::spacer()
    })
    .on_window_change(move |_, id, info| log.borrow_mut().push((id, info.maximized)));
    let handle = app.handle();
    let mut run = run(app);
    assert_eq!(run.desk.take_log(), ["title 100 normal"]);
    changes.borrow_mut().clear();

    let mut info = WindowInfo::from_size(PhysicalSize::new(200, 100), ScaleFactor::IDENTITY);
    info.maximized = true;
    info.position = Some(Point::new(5.0, 6.0));
    run.desk.info.borrow_mut().insert(100, info);
    run.send(100, Event::WindowChanged);
    assert_eq!(*changes.borrow(), [(WindowId::PRIMARY, true)]);
    assert_eq!(run.desk.take_log(), ["title 100 max"]);
    assert_eq!(handle.window_info(WindowId::PRIMARY), Some(info));
    // Nothing changed: no call.
    run.send(100, Event::WindowChanged);
    assert_eq!(changes.borrow().len(), 1);
}
