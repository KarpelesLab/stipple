//! Text fields as live OS text controls (see `core::native_host::text`),
//! driven through the App's event handler with a pretend window that
//! hosts them: what the field hands its control, the control's edits,
//! focus and keys reaching the field, and the field staying live while it
//! is edited.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::App;
use crate::core::Cx;
use crate::geometry::{PhysicalSize, ScaleFactor, Size};
use crate::platform::backend::headless::HeadlessSurface;
use crate::platform::native::{
    Composition, NativeControl, NativeKey, NativeTextEdit, NativeView, NativeViewEvent,
};
use crate::platform::{ControlFlow, Event, ImeState, Window, WindowId};
use crate::render::Surface;
use crate::widgets::{NumberInput, TextInput};

#[derive(Default)]
struct Seen {
    views: Vec<Vec<NativeView>>,
    ime: Vec<Option<ImeState>>,
    typing: Vec<bool>,
    a11y: Vec<crate::platform::A11yNode>,
}

struct HostWindow {
    seen: Rc<RefCell<Seen>>,
}

impl Window for HostWindow {
    fn id(&self) -> WindowId {
        WindowId(1)
    }
    fn inner_size(&self) -> PhysicalSize {
        PhysicalSize::new(300, 300)
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
        true
    }
    fn set_native_views(&self, views: &[NativeView]) {
        self.seen.borrow_mut().views.push(views.to_vec());
    }
    fn set_ime(&self, state: Option<&ImeState>) {
        self.seen.borrow_mut().ime.push(state.cloned());
    }
    fn set_text_input_active(&self, active: bool) {
        self.seen.borrow_mut().typing.push(active);
    }
    fn set_accessibility_tree(&self, root: &crate::platform::A11yNode) {
        self.seen.borrow_mut().a11y.push(root.clone());
    }
}

struct Form {
    name: TextInput,
    count: NumberInput,
    log: Arc<Mutex<Vec<String>>>,
}

fn view(s: &Form, cx: &mut Cx<Form>) -> crate::core::Element {
    crate::widgets::column(vec![
        crate::widgets::text_field(cx, &s.name, |s| &mut s.name)
            .placeholder("Your name")
            .on_change(cx, |s| {
                let t = s.name.text().to_string();
                s.log.lock().unwrap().push(format!("change {t}"));
            })
            .on_submit(cx, |s| {
                let t = s.name.text().to_string();
                s.log.lock().unwrap().push(format!("submit {t}"));
            }),
        crate::widgets::number_input(cx, &s.count, |s| &mut s.count),
    ])
}

type Handler = Box<dyn FnMut(Event, &dyn Window) -> ControlFlow>;

struct Started {
    handler: Handler,
    window: HostWindow,
    seen: Rc<RefCell<Seen>>,
    log: Arc<Mutex<Vec<String>>>,
}

fn start() -> Started {
    crate::widgets::native_field_tests::FORCE.with(|f| f.set(true));
    let log = Arc::new(Mutex::new(Vec::new()));
    let app = App::new(
        Form {
            name: TextInput::from_text("Ann").max_length(6),
            count: NumberInput::new(5.0).range(0.0, 10.0),
            log: log.clone(),
        },
        view,
    )
    .no_font()
    .logical_size(Size::new(300.0, 300.0));
    let (_, handler) = app.into_handler();
    let seen = Rc::new(RefCell::new(Seen::default()));
    let window = HostWindow { seen: seen.clone() };
    Started {
        handler: Box::new(handler),
        window,
        seen,
        log,
    }
}

impl Started {
    fn frame(&mut self) {
        (self.handler)(Event::RedrawRequested, &self.window);
    }

    fn send(&mut self, id: u64, event: NativeViewEvent) {
        (self.handler)(Event::NativeView { id, event }, &self.window);
        self.frame();
    }

    /// The live text controls, as the window was last told.
    fn fields(&self) -> Vec<NativeView> {
        self.seen
            .borrow()
            .views
            .last()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|v| v.control == NativeControl::TextField)
            .collect()
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

fn edit(seq: u64, replaced: (usize, usize), inserted: &str, caret: usize) -> NativeTextEdit {
    NativeTextEdit {
        seq,
        replaced,
        inserted: inserted.into(),
        anchor: caret,
        caret,
        composing: None,
    }
}

#[test]
fn a_text_field_is_a_live_text_control_with_its_state() {
    let mut t = start();
    t.frame();
    let fields = t.fields();
    assert_eq!(fields.len(), 2, "{fields:?}");
    let name = fields[0].text.as_deref().unwrap();
    assert_eq!(name.text, "Ann");
    assert_eq!((name.anchor, name.caret), (3, 3));
    assert_eq!(name.placeholder, "Your name");
    assert!(!name.focused && !name.multiline && !name.secure);
    // Read by its hint, its value being the control's own.
    assert_eq!(fields[0].label, "Your name");
    assert!(fields[0].font_size > 0.0);
    let count = fields[1].text.as_deref().unwrap();
    assert_eq!(count.text, "5");
    // A screen reader reads each field once: as its OS control.
    let a11y = t.seen.borrow().a11y.last().cloned().unwrap();
    fn natives(n: &crate::platform::A11yNode, out: &mut Vec<u64>) {
        out.extend(n.native_view);
        n.children.iter().for_each(|c| natives(c, out));
    }
    let mut ids = Vec::new();
    natives(&a11y, &mut ids);
    assert_eq!(ids, [fields[0].id, fields[1].id]);
}

#[test]
fn typing_in_the_control_edits_the_field_and_its_handlers_run() {
    let mut t = start();
    t.frame();
    let id = t.fields()[0].id;
    t.send(id, NativeViewEvent::Focus(true));
    assert!(t.fields()[0].text.as_ref().unwrap().focused);
    t.send(id, NativeViewEvent::Edited(edit(41, (3, 3), "a", 4)));
    let name = t.fields()[0].text.clone().unwrap();
    assert_eq!(name.text, "Anna");
    assert_eq!(name.caret, 4);
    assert_eq!(name.seen, 41, "the control learns the app took the edit in");
    assert_eq!(t.log(), ["change Anna"]);
    // Past the length limit: the app keeps what fits, and the control is
    // told so.
    t.send(id, NativeViewEvent::Edited(edit(42, (4, 4), "bella", 9)));
    let name = t.fields()[0].text.clone().unwrap();
    assert_eq!(name.text, "Annabe");
    assert_eq!((name.anchor, name.caret), (6, 6));
    // Return submits.
    t.send(id, NativeViewEvent::Key(NativeKey::Enter));
    assert_eq!(t.log().last().unwrap(), "submit Annabe");
    // Deleting a selection.
    t.send(id, NativeViewEvent::Edited(edit(43, (0, 4), "", 0)));
    assert_eq!(t.fields()[0].text.as_ref().unwrap().text, "be");
    // The window's own input method stayed off throughout.
    let seen = t.seen.borrow();
    assert!(seen.ime.iter().all(|s| s.is_none()), "{:?}", seen.ime);
    assert!(seen.typing.iter().all(|on| !on));
}

#[test]
fn a_composition_in_the_control_is_the_fields_preedit() {
    let mut t = start();
    t.frame();
    let id = t.fields()[0].id;
    let mut e = edit(7, (3, 3), "", 3);
    e.composing = Some(Composition {
        text: "にほ".into(),
        cursor: Some((6, 6)),
    });
    t.send(id, NativeViewEvent::Edited(e));
    let name = t.fields()[0].text.clone().unwrap();
    assert_eq!(name.text, "Ann");
    let c = name.composing.unwrap();
    assert_eq!((c.text.as_str(), c.cursor), ("にほ", Some((6, 6))));
    // Committed: the text has it, the composition is gone.
    t.send(id, NativeViewEvent::Edited(edit(8, (3, 3), "日", 6)));
    let name = t.fields()[0].text.clone().unwrap();
    assert_eq!(name.text, "Ann日");
    assert!(name.composing.is_none());
}

#[test]
fn a_field_being_edited_stays_live_with_its_focus_ring_showing() {
    let mut t = start();
    t.frame();
    // Tab to the field: the ring shows, and the field is still the OS's.
    (t.handler)(
        Event::Key {
            code: crate::platform::KeyCode::Tab,
            state: crate::platform::ButtonState::Pressed,
            modifiers: crate::platform::Modifiers::NONE,
        },
        &t.window,
    );
    t.frame();
    let fields = t.fields();
    assert_eq!(fields.len(), 2);
    assert!(fields[0].text.as_ref().unwrap().focused);
    // Tab from inside the control moves on to the number.
    t.send(fields[0].id, NativeViewEvent::Key(NativeKey::Tab));
    let fields = t.fields();
    assert!(!fields[0].text.as_ref().unwrap().focused);
    assert!(fields[1].text.as_ref().unwrap().focused);
}

#[test]
fn an_invalid_number_stays_live_while_edited_and_shows_its_own_frame_after() {
    let mut t = start();
    t.frame();
    let id = t.fields()[1].id;
    t.send(id, NativeViewEvent::Focus(true));
    // "5" becomes "-" on the way to "-3": not a number in range.
    t.send(id, NativeViewEvent::Edited(edit(3, (0, 1), "-", 1)));
    let fields = t.fields();
    assert_eq!(fields.len(), 2, "still live while focused");
    let count = fields[1].text.as_ref().unwrap();
    assert_eq!(count.text, "-");
    assert!(count.invalid);
    // Up steps it, as in Stipple's own field.
    t.send(id, NativeViewEvent::Key(NativeKey::Up));
    assert_eq!(t.fields()[1].text.as_ref().unwrap().text, "6");
    // Invalid again and focus elsewhere: Stipple's red frame, a picture.
    t.send(id, NativeViewEvent::Edited(edit(4, (0, 1), "x-", 2)));
    let name = t.fields()[0].id;
    t.send(name, NativeViewEvent::Focus(true));
    let fields = t.fields();
    assert!(fields.iter().all(|f| f.id != id), "{fields:?}");
}

#[test]
fn the_control_leaving_focus_outside_the_app_unfocuses_the_field() {
    let mut t = start();
    t.frame();
    let id = t.fields()[0].id;
    t.send(id, NativeViewEvent::Focus(true));
    t.send(id, NativeViewEvent::Focus(false));
    assert!(!t.fields()[0].text.as_ref().unwrap().focused);
    // A stale blur for a field that no longer has focus changes nothing.
    let other = t.fields()[1].id;
    t.send(other, NativeViewEvent::Focus(true));
    t.send(id, NativeViewEvent::Focus(false));
    assert!(t.fields()[1].text.as_ref().unwrap().focused);
}

#[test]
fn edits_amount_to_the_keys_a_field_takes() {
    use crate::core::KeyInput;
    let keys = crate::app_native_views::text::keys(&edit(1, (1, 3), "", 1));
    assert_eq!(
        keys[1..],
        [
            KeyInput::SetSelection {
                anchor: 1,
                caret: 3
            },
            KeyInput::Backspace,
            KeyInput::SetSelection {
                anchor: 1,
                caret: 1
            },
        ]
    );
    // A move of the caret alone.
    let keys = crate::app_native_views::text::keys(&edit(2, (0, 0), "", 2));
    assert_eq!(keys.len(), 2);
}
