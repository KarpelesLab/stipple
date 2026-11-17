//! Live OS controls (see `core::native_host`), driven through the App's
//! event handler with a pretend window that hosts them.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::App;
use crate::core::{Axis, Cx, Element};
use crate::geometry::{PhysicalSize, ScaleFactor, Size};
use crate::platform::backend::headless::HeadlessSurface;
use crate::platform::native::{
    NativeControl, NativeDeco, NativeState, NativeView, NativeViewEvent,
};
use crate::platform::{ControlFlow, Event, Window, WindowId};
use crate::render::{Color, Surface};

#[derive(Default)]
struct Seen {
    views: Vec<Vec<NativeView>>,
    a11y: Vec<crate::platform::A11yNode>,
}

struct HostWindow {
    seen: Rc<RefCell<Seen>>,
    hosts: bool,
}

impl Window for HostWindow {
    fn id(&self) -> WindowId {
        WindowId(1)
    }
    fn inner_size(&self) -> PhysicalSize {
        PhysicalSize::new(300, 200)
    }
    fn scale_factor(&self) -> ScaleFactor {
        ScaleFactor::IDENTITY
    }
    fn request_redraw(&self) {}
    fn set_title(&self, _title: &str) {}
    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(HeadlessSurface::new(self.inner_size()).0)
    }
    fn hosts_native_views(&self) -> bool {
        self.hosts
    }
    fn set_native_views(&self, views: &[NativeView]) {
        self.seen.borrow_mut().views.push(views.to_vec());
    }
    fn set_accessibility_tree(&self, root: &crate::platform::A11yNode) {
        self.seen.borrow_mut().a11y.push(root.clone());
    }
}

struct Panel {
    saves: Arc<AtomicU32>,
    /// The slider's position, in thousandths.
    level: Arc<AtomicU32>,
    overlay: bool,
}

fn deco(control: NativeControl) -> NativeDeco {
    NativeDeco {
        control,
        dark: false,
        state: NativeState::default(),
    }
}

/// A button and a slider as the native theme makes them, and a popup over
/// the button while `overlay` is set.
fn view(s: &Panel, cx: &mut Cx<Panel>) -> Element {
    let save = Element::stack(
        Axis::Horizontal,
        vec![Element::text("Save", 14.0, Color::BLACK)],
    )
    .native(deco(NativeControl::Button { primary: true }))
    .width(80.0)
    .height(30.0)
    .on_tap(cx, |s| {
        s.saves.fetch_add(1, Ordering::Relaxed);
    });
    let level = Element::boxed(Default::default())
        .native(deco(NativeControl::Slider {
            value: s.level.load(Ordering::Relaxed) as u16,
        }))
        .width(200.0)
        .height(30.0)
        .role(crate::core::Role::Slider)
        .on_drag(cx, |s, v| {
            s.level
                .store((v * 1000.0).round() as u32, Ordering::Relaxed);
        });
    if s.overlay {
        let popup = Element::boxed(Default::default())
            .fill(Color::WHITE)
            .width(40.0)
            .height(20.0);
        cx.overlay(crate::core::OverlaySpec::new(
            popup,
            crate::core::Anchor::At(crate::geometry::Point::new(10.0, 5.0)),
        ));
    }
    crate::widgets::column(vec![save, level])
}

type Handler = Box<dyn FnMut(Event, &dyn Window) -> ControlFlow>;

struct Started {
    handler: Handler,
    window: HostWindow,
    seen: Rc<RefCell<Seen>>,
    store: crate::core::AppHandle<Panel>,
    saves: Arc<AtomicU32>,
    level: Arc<AtomicU32>,
}

fn start(hosts: bool) -> Started {
    let (saves, level) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(500)));
    let app = App::new(
        Panel {
            saves: saves.clone(),
            level: level.clone(),
            overlay: false,
        },
        view,
    )
    .no_font()
    .logical_size(Size::new(300.0, 200.0));
    let store = app.handle();
    let (_, handler) = app.into_handler();
    let seen = Rc::new(RefCell::new(Seen::default()));
    let window = HostWindow {
        seen: seen.clone(),
        hosts,
    };
    Started {
        handler: Box::new(handler),
        window,
        seen,
        store,
        saves,
        level,
    }
}

#[test]
fn a_window_that_hosts_controls_gets_them_live_and_their_events_reach_the_handlers() {
    let Started {
        mut handler,
        window,
        seen,
        saves,
        level,
        ..
    } = start(true);
    handler(Event::RedrawRequested, &window);
    let views = seen.borrow().views.last().cloned().expect("views sent");
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].title, "Save");
    assert!(matches!(
        views[1].control,
        NativeControl::Slider { value: 500 }
    ));
    // The screen reader reads each control as the OS control.
    let a11y = seen.borrow().a11y.last().cloned().unwrap();
    fn natives(n: &crate::platform::A11yNode, out: &mut Vec<u64>) {
        out.extend(n.native_view);
        n.children.iter().for_each(|c| natives(c, out));
    }
    let mut ids = Vec::new();
    natives(&a11y, &mut ids);
    assert_eq!(ids, [views[0].id, views[1].id]);

    // A press on the OS button runs the button's handler.
    handler(
        Event::NativeView {
            id: views[0].id,
            event: NativeViewEvent::Activated,
        },
        &window,
    );
    assert_eq!(saves.load(Ordering::Relaxed), 1);
    // The slider moved: its drag handler sees the position.
    handler(
        Event::NativeView {
            id: views[1].id,
            event: NativeViewEvent::Value(0.25),
        },
        &window,
    );
    assert_eq!(level.load(Ordering::Relaxed), 250);
    let last = seen.borrow().views.last().cloned().unwrap();
    assert!(matches!(
        last[1].control,
        NativeControl::Slider { value: 250 }
    ));
    // An unknown control is ignored.
    handler(
        Event::NativeView {
            id: 12345,
            event: NativeViewEvent::Activated,
        },
        &window,
    );
    assert_eq!(saves.load(Ordering::Relaxed), 1);
}

#[test]
fn a_popup_over_a_control_turns_it_into_a_picture_until_it_goes() {
    let Started {
        mut handler,
        window,
        seen,
        store,
        ..
    } = start(true);
    handler(Event::RedrawRequested, &window);
    store.update(|s| s.overlay = true);
    handler(Event::Wake, &window);
    handler(Event::RedrawRequested, &window);
    let views = seen.borrow().views.last().cloned().unwrap();
    // The popup's text is over the button, not over the slider.
    let titles: Vec<&str> = views.iter().map(|v| v.title.as_str()).collect();
    assert_eq!(titles, [""], "{views:?}");
    store.update(|s| s.overlay = false);
    handler(Event::Wake, &window);
    handler(Event::RedrawRequested, &window);
    assert_eq!(seen.borrow().views.last().unwrap().len(), 2);
}

#[test]
fn a_window_that_hosts_none_keeps_every_control_a_picture() {
    let Started {
        mut handler,
        window,
        seen,
        ..
    } = start(false);
    handler(Event::RedrawRequested, &window);
    assert!(seen.borrow().views.iter().all(|v| v.is_empty()));
}
