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
    /// Whether windows show a native menu bar (as on Windows) or the app
    /// draws one (as on Linux).
    native_menus: Cell<bool>,
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
        if let Some(parent) = attrs.parent {
            self.desk.log.borrow_mut().push(format!(
                "parent {id} {} modal={}",
                parent.0,
                attrs.blocks_parent()
            ));
        }
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
    fn set_keep_awake(&self, on: bool) {
        self.desk
            .log
            .borrow_mut()
            .push(format!("awake {} {on}", self.id));
    }
    fn set_orientations(&self, set: crate::platform::Orientations) {
        self.desk
            .log
            .borrow_mut()
            .push(format!("orientations {} {set:?}", self.id));
    }
    fn activate(&self, token: Option<&str>) {
        let line = match token {
            Some(token) => format!("activate {} {token}", self.id),
            None => format!("activate {}", self.id),
        };
        self.desk.log.borrow_mut().push(line);
    }
    fn set_menu_bar(&self, menu: &crate::platform::NativeMenuBar) -> bool {
        if self.desk.native_menus.get() {
            self.desk
                .log
                .borrow_mut()
                .push(format!("menu {} {}", self.id, menu.menus.len()));
        }
        self.desk.native_menus.get()
    }
    fn set_input_hints(&self, hints: &crate::platform::InputHints) {
        self.desk.log.borrow_mut().push(format!(
            "hints {} {:?} {:?} secure={}",
            self.id,
            hints.kind,
            hints.enter(),
            hints.secure
        ));
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
    cx.set_window_title(format!("{} ({})", s.name, s.clicks));
    crate::widgets::spacer()
        .grow(1.0)
        .on_tap(cx, |s| s.clicks += 1)
}

#[test]
fn the_view_titles_its_window_when_the_title_changes() {
    let app = App::new(doc(), titled).no_font();
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
    let app = App::new(doc(), titled).no_font();
    let handle = app.handle();
    let mut run = run(app);
    run.desk.take_log();

    // One view for any number of windows, told which it builds.
    let saw = seen.clone();
    let id = handle.open_window(
        WindowAttributes::new().title("Second").focus(false),
        move |s: &Doc, cx: &mut Cx<Doc>| {
            saw.lock().unwrap().push(cx.window_id());
            cx.set_window_title(format!("second: {}", s.clicks));
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
    let mut app = App::new(doc(), titled).no_font();
    let other = app.open_window(WindowAttributes::new().title("Other"), titled);
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
    let app = App::new(doc(), titled)
        .no_font()
        .exit_on_last_window_closed(false);
    let handle = app.handle();
    let mut run = run(app);
    run.desk.take_log();
    // The last window hides instead of closing.
    assert_eq!(run.send(100, Event::CloseRequested), ControlFlow::Wait);
    assert_eq!(run.desk.take_log(), ["visible 100 false"]);
    assert!(handle.windows().is_empty());
    // Timers still run, and new windows open from the hidden one.
    handle.update(|s| s.clicks = 5);
    let id = handle.open_window(WindowAttributes::new().title("Again"), titled);
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
    let mut app = App::new(doc(), titled)
        .no_font()
        .logical_size(Size::new(200.0, 100.0));
    app.open_window(WindowAttributes::new().title("Other"), titled);
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
fn a_view_reading_the_safe_area_follows_it_but_not_the_window_moving() {
    let app = App::new(doc(), |_: &Doc, cx: &mut Cx<Doc>| {
        let safe = cx.safe_area();
        cx.set_window_title(format!("top {}", safe.top));
        crate::widgets::spacer()
    })
    .no_font();
    let mut run = run(app);
    assert_eq!(run.desk.take_log(), ["title 100 top 0"]);
    // Moved: nothing it reads changed, so no rebuild (and no retitle).
    let mut info = WindowInfo::from_size(PhysicalSize::new(200, 100), ScaleFactor::IDENTITY);
    info.position = Some(Point::new(5.0, 6.0));
    run.desk.info.borrow_mut().insert(100, info);
    run.send(100, Event::WindowChanged);
    run.send(100, Event::RedrawRequested);
    assert!(run.desk.take_log().is_empty());
    // Turned: the safe area changed, and the view is built again.
    info.safe_area = crate::geometry::Insets::new(47.0, 0.0, 34.0, 0.0);
    run.desk.info.borrow_mut().insert(100, info);
    run.send(100, Event::WindowChanged);
    assert_eq!(run.desk.take_log(), ["title 100 top 47"]);
    assert_eq!(
        info.orientation(),
        crate::platform::Orientation::Landscape,
        "200x100 is wider than tall"
    );
}

#[test]
fn keep_awake_and_orientations_reach_the_window() {
    let app = App::new(doc(), titled).no_font();
    let handle = app.handle();
    let mut run = run(app);
    run.desk.take_log();
    handle.set_keep_awake(true);
    handle.set_orientations(crate::platform::Orientations::LANDSCAPE);
    run.wake();
    assert_eq!(
        run.desk.take_log(),
        [
            "awake 100 true",
            "orientations 100 Orientations(LANDSCAPE_LEFT | LANDSCAPE_RIGHT)"
        ]
    );
}

#[test]
fn window_changes_reach_the_state_and_views_that_read_them() {
    let changes: Rc<RefCell<Vec<(WindowId, bool)>>> = Default::default();
    let log = changes.clone();
    let app = App::new(doc(), |_: &Doc, cx: &mut Cx<Doc>| {
        let info = cx.window_info();
        cx.set_window_title(if info.maximized { "max" } else { "normal" });
        crate::widgets::spacer()
    })
    .no_font()
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

#[test]
fn a_backgrounded_app_repaints_only_when_it_comes_back() {
    let _lock = crate::platform::frame::SUSPEND_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    use crate::platform::Lifecycle;
    let seen: Rc<RefCell<Vec<Lifecycle>>> = Default::default();
    let record = seen.clone();
    let app = App::new(doc(), titled)
        .no_font()
        .on_lifecycle(move |_, change| {
            record.borrow_mut().push(change);
        });
    let handle = app.handle();
    let mut run = run(app);
    run.desk.take_log();
    run.send(100, Event::Lifecycle(Lifecycle::Background));
    assert!(crate::platform::frame::suspended());
    // Updates run, but nothing is built or shown while away...
    handle.update(|s| s.name = "b.txt".into());
    run.wake();
    assert!(
        run.desk.take_log().is_empty(),
        "no repaint in the background"
    );
    // ...until the app is back on screen.
    run.send(100, Event::Lifecycle(Lifecycle::Foreground));
    assert!(!crate::platform::frame::suspended());
    assert_eq!(run.desk.take_log(), ["title 100 b.txt (0)"]);
    assert_eq!(
        *seen.borrow(),
        [Lifecycle::Background, Lifecycle::Foreground]
    );
}

#[test]
fn the_focused_fields_keyboard_hints_reach_the_window_and_the_view_sees_the_keyboard() {
    use crate::platform::{EnterAction, InputKind, KeyCode, Modifiers};
    use crate::widgets::TextInput;
    struct Form {
        code: TextInput,
        pin: crate::widgets::PasswordField,
    }
    let app = App::new(
        Form {
            code: TextInput::new(),
            pin: Default::default(),
        },
        |s: &Form, cx: &mut Cx<Form>| {
            let keyboard = cx.keyboard_inset();
            let bottom = cx.safe_area().bottom;
            cx.set_window_title(format!("keyboard {keyboard} bottom {bottom}"));
            crate::widgets::column(vec![
                crate::widgets::text_field(cx, &s.code, |s| &mut s.code)
                    .input_kind(InputKind::Number)
                    .enter_action(EnterAction::Done),
                crate::widgets::password_field(cx, &s.pin, |s| &mut s.pin)
                    .input_kind(InputKind::Number),
            ])
        },
    )
    .no_font();
    let mut run = run(app);
    assert_eq!(run.desk.take_log(), ["title 100 keyboard 0 bottom 0"]);
    let tab = |state| Event::Key {
        code: KeyCode::Tab,
        state,
        modifiers: Modifiers::default(),
    };
    run.send(100, tab(ButtonState::Pressed));
    run.send(100, tab(ButtonState::Released));
    run.send(100, Event::RedrawRequested);
    let log = run.desk.take_log();
    assert!(
        log.contains(&"hints 100 Number Some(Done) secure=false".to_string()),
        "{log:?}"
    );
    // A PIN field: a number pad, masked.
    run.send(100, tab(ButtonState::Pressed));
    run.send(100, Event::RedrawRequested);
    let log = run.desk.take_log();
    assert!(
        log.contains(&"hints 100 Number None secure=true".to_string()),
        "{log:?}"
    );
    // The keyboard comes up over the home indicator: the view is built
    // again, its safe area ending at the keyboard.
    let mut info = WindowInfo::from_size(PhysicalSize::new(200, 100), ScaleFactor::IDENTITY);
    info.safe_area = crate::geometry::Insets::new(0.0, 0.0, 20.0, 0.0);
    run.desk.info.borrow_mut().insert(100, info);
    run.send(100, Event::WindowChanged);
    run.desk.take_log();
    info.keyboard_inset = 60.0;
    run.desk.info.borrow_mut().insert(100, info);
    run.send(100, Event::WindowChanged);
    run.send(100, Event::RedrawRequested);
    assert!(
        run.desk
            .take_log()
            .contains(&"title 100 keyboard 60 bottom 0".to_string())
    );
}

#[test]
fn a_modal_window_belongs_to_its_parent_blocks_it_and_closes_with_it() {
    let app = App::new(doc(), titled)
        .no_font()
        .logical_size(Size::new(200.0, 100.0));
    let handle = app.handle();
    let mut run = run(app);
    run.desk.take_log();
    // The backend hears the parent by its own id (100), not the app's (0).
    let child = handle.open_window(
        WindowAttributes::new()
            .title("Prefs")
            .parent(WindowId::PRIMARY)
            .modal(true),
        |_: &Doc, _: &mut Cx<Doc>| crate::widgets::spacer(),
    );
    run.wake();
    assert_eq!(
        run.desk.take_log(),
        ["open 101 Prefs focus=true", "parent 101 100 modal=true"]
    );
    run.send(101, Event::RedrawRequested);
    run.desk.take_log();

    // Clicks in the parent don't reach it while the modal window is open,
    // and neither does closing it.
    let click = |run: &mut Run<_>| {
        for state in [ButtonState::Pressed, ButtonState::Released] {
            run.send(
                100,
                Event::PointerButton {
                    button: PointerButton::Left,
                    state,
                    position: Point::new(50.0, 50.0),
                },
            );
        }
    };
    click(&mut run);
    assert!(run.desk.take_log().is_empty());
    assert_eq!(run.send(100, Event::CloseRequested), ControlFlow::Wait);
    assert_eq!(handle.windows(), [WindowId::PRIMARY, child]);

    // Once it closes, the parent takes input again.
    handle.close_window(child);
    run.wake();
    assert_eq!(run.desk.take_log(), ["close 101"]);
    click(&mut run);
    assert_eq!(run.desk.take_log(), ["title 100 a.txt (1)"]);

    // A window that belongs to another closes with it.
    let other = handle.open_window(
        WindowAttributes::new()
            .title("Owned")
            .parent(WindowId::PRIMARY),
        |_: &Doc, _: &mut Cx<Doc>| crate::widgets::spacer(),
    );
    let second = handle.open_window(WindowAttributes::new().title("Second"), titled);
    run.wake();
    let log = run.desk.take_log();
    assert!(
        log.contains(&"parent 102 100 modal=false".to_string()),
        "{log:?}"
    );
    assert_eq!(handle.windows(), [WindowId::PRIMARY, other, second]);
    assert_eq!(run.send(100, Event::CloseRequested), ControlFlow::Wait);
    run.wake();
    assert_eq!(handle.windows(), [second]);
}

#[test]
fn a_drawn_message_box_shows_in_the_window_it_is_for() {
    let app = App::new(doc(), titled)
        .no_font()
        .logical_size(Size::new(400.0, 300.0));
    let handle = app.handle();
    let mut run = run(app);
    let other = handle.open_window(
        WindowAttributes::new().title("Other"),
        |_: &Doc, _: &mut Cx<Doc>| crate::widgets::spacer(),
    );
    run.wake();
    handle.message(
        crate::platform::dialog::MessageBox::new("Host key changed")
            .drawn(true)
            .window(other),
        |_: &mut Doc, _| {},
    );
    // Drawn in the other window only.
    let seen = handle.alert_view(other, false).map(|v| v.mb.text);
    assert_eq!(seen, None, "not asked yet: the pump starts it");
    run.wake();
    assert!(handle.alert_view(other, false).is_some());
    assert!(handle.alert_view(WindowId::PRIMARY, true).is_none());
    // A box for no window shows in the primary one.
    handle.message(
        crate::platform::dialog::MessageBox::new("Saved").drawn(true),
        |_: &mut Doc, _| {},
    );
    run.wake();
    assert_eq!(
        handle
            .alert_view(WindowId::PRIMARY, true)
            .map(|v| v.mb.text),
        Some("Saved".to_string())
    );
}

/// The state of an app whose menu command acts on a window: the windows it
/// was picked from, shared with the test.
struct Tabs {
    opened_in: Rc<RefCell<Vec<WindowId>>>,
}

fn tabs_app() -> (App<Tabs>, Rc<RefCell<Vec<WindowId>>>) {
    use crate::core::{Menu, MenuBar, MenuItem, Shortcut};
    let opened: Rc<RefCell<Vec<WindowId>>> = Default::default();
    let state = Tabs {
        opened_in: opened.clone(),
    };
    let app = App::new(state, |_: &Tabs, _: &mut Cx<Tabs>| crate::widgets::spacer())
        .no_font()
        .menu(|_: &Tabs| {
            MenuBar::new().menu(
                Menu::new("&File").item(
                    MenuItem::for_window("New &Tab", |s: &mut Tabs, window| {
                        s.opened_in.borrow_mut().push(window)
                    })
                    .shortcut(Shortcut::cmd('t')),
                ),
            )
        });
    (app, opened)
}

fn blank(_: &Tabs, _: &mut Cx<Tabs>) -> Element {
    crate::widgets::spacer()
}

/// Press and release `code` in window `id`.
fn tap_key<H: FnMut(Event, &dyn Window) -> ControlFlow>(
    run: &mut Run<H>,
    id: u64,
    code: crate::platform::KeyCode,
    modifiers: crate::platform::Modifiers,
) {
    for state in [ButtonState::Pressed, ButtonState::Released] {
        run.send(
            id,
            Event::Key {
                code,
                state,
                modifiers,
            },
        );
    }
}

/// Open the drawn menu bar's first menu from the keyboard and pick its first
/// command, in window `id`.
fn pick_from_drawn_bar<H: FnMut(Event, &dyn Window) -> ControlFlow>(run: &mut Run<H>, id: u64) {
    use crate::platform::{KeyCode, Modifiers};
    let none = Modifiers::default();
    tap_key(run, id, KeyCode::F10, none);
    run.send(id, Event::RedrawRequested);
    tap_key(run, id, KeyCode::ArrowDown, none);
    run.send(id, Event::RedrawRequested);
    tap_key(run, id, KeyCode::Enter, none);
}

#[test]
fn every_window_draws_the_menu_bar_and_its_commands_know_the_window() {
    let (mut app, opened) = tabs_app();
    let second = app.open_window(WindowAttributes::new().title("Second"), blank);
    let handle = app.handle();
    let mut run = run(app);
    run.send(101, Event::RedrawRequested);
    // The second window's drawn bar runs the command for it, the first's for
    // the first, and a shortcut for the window it's pressed in.
    pick_from_drawn_bar(&mut run, 101);
    pick_from_drawn_bar(&mut run, 100);
    let cmd = crate::platform::Modifiers {
        ctrl: !cfg!(target_os = "macos"),
        meta: cfg!(target_os = "macos"),
        ..Default::default()
    };
    tap_key(&mut run, 101, crate::platform::KeyCode::Char('t'), cmd);
    // Closing the first window leaves the other with its bar.
    assert_eq!(run.send(100, Event::CloseRequested), ControlFlow::Wait);
    assert_eq!(handle.windows(), [second]);
    run.send(101, Event::RedrawRequested);
    pick_from_drawn_bar(&mut run, 101);
    assert_eq!(
        *opened.borrow(),
        [second, WindowId::PRIMARY, second, second],
        "the window each pick came from"
    );
    let log = run.desk.take_log();
    assert!(!log.iter().any(|l| l.starts_with("menu ")), "{log:?}");
}

#[test]
fn every_window_gets_the_native_menu_bar_and_picks_come_from_their_window() {
    let (mut app, opened) = tabs_app();
    let second = app.open_window(WindowAttributes::new().title("Second"), blank);
    let handle = app.handle();
    let (_, handler) = app.into_handler();
    let desk = Desk::new();
    desk.native_menus.set(true);
    let mut run = Run { desk, handler };
    run.send(100, Event::RedrawRequested);
    run.send(101, Event::RedrawRequested);
    let log = run.desk.take_log();
    assert!(log.contains(&"menu 100 1".to_string()), "{log:?}");
    assert!(log.contains(&"menu 101 1".to_string()), "{log:?}");
    // A pick from the second window's bar is for the second window.
    run.send(101, Event::MenuCommand(1));
    run.send(100, Event::MenuCommand(1));
    // The first closing doesn't take the other's bar; a window opened later
    // gets one too.
    assert_eq!(run.send(100, Event::CloseRequested), ControlFlow::Wait);
    let third = handle.open_window(WindowAttributes::new().title("Third"), blank);
    run.wake();
    run.send(102, Event::RedrawRequested);
    let log = run.desk.take_log();
    assert!(log.contains(&"menu 102 1".to_string()), "{log:?}");
    assert!(
        !log.contains(&"menu 101 1".to_string()),
        "unchanged: {log:?}"
    );
    run.send(102, Event::MenuCommand(1));
    assert_eq!(*opened.borrow(), [second, WindowId::PRIMARY, third]);
}

#[test]
fn a_focus_request_activates_its_window() {
    let mut app = App::new(doc(), titled).no_font();
    let other = app.open_window(WindowAttributes::new().title("Other"), titled);
    let handle = app.handle();
    let mut run = run(app);
    run.desk.take_log();
    handle.window_to(other, WindowRequest::Activate(None));
    run.wake();
    let log = run.desk.take_log();
    assert!(log.contains(&"activate 101".to_string()), "{log:?}");
    assert!(!log.contains(&"activate 100".to_string()), "{log:?}");
}

#[test]
fn a_window_opened_maximized_restores_to_the_bounds_it_was_opened_with() {
    let app = App::new(doc(), titled).no_font();
    let handle = app.handle();
    let mut run = run(app);
    let id = handle.open_window(
        WindowAttributes::new()
            .title("Saved")
            .position(30, 40)
            .logical_size(Size::new(640.0, 480.0))
            .maximized(true),
        titled,
    );
    run.wake();
    let info = handle.window_info(id).unwrap();
    assert!(info.maximized);
    assert_eq!(info.restored_position, Some(Point::new(30.0, 40.0)));
    assert_eq!(info.restored_size, Size::new(640.0, 480.0));
}
