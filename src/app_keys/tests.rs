//! The order keys go through the App: menus, then the focused element's raw
//! key handler, then the toolkit (Tab, editing keys, `on_key`).

use crate::App;
use crate::core::keys::Handled;
use crate::core::{Cx, Element, KeyInput, Menu, MenuBar, MenuItem, Shortcut};
use crate::geometry::{PhysicalSize, Rect, ScaleFactor, Size};
use crate::platform::{
    ButtonState, ImeState, KeyCode, KeyEvent, Modifiers, OptionAsMeta, PhysicalKey, Window,
    WindowId,
};
use crate::render::Surface;
use crate::widgets::column;
use std::cell::RefCell;

#[derive(Default)]
struct Term {
    /// What the raw key handler saw.
    raw: Vec<String>,
    /// What `on_key` got.
    typed: Vec<KeyInput>,
    /// Menu commands run.
    commands: Vec<&'static str>,
    /// Let Tab through the raw handler.
    pass_tab: bool,
    capture_tab: bool,
}

fn command() -> Modifiers {
    Modifiers {
        ctrl: !cfg!(target_os = "macos"),
        meta: cfg!(target_os = "macos"),
        ..Modifiers::default()
    }
}

fn ctrl() -> Modifiers {
    Modifiers {
        ctrl: true,
        ..Modifiers::default()
    }
}

fn key(state: ButtonState, logical: KeyCode, modifiers: Modifiers) -> KeyEvent {
    let mut ev = KeyEvent::new(state, logical, PhysicalKey::UNIDENTIFIED);
    ev.modifiers = modifiers;
    ev
}

fn press(logical: KeyCode, modifiers: Modifiers) -> KeyEvent {
    key(ButtonState::Pressed, logical, modifiers)
}

/// A terminal-like element that takes raw keys (all but `x`, and Tab unless
/// `pass_tab`), above a second focusable element; a menu bar with a Cmd+T
/// shortcut.
fn app() -> App<Term> {
    App::new(Term::default(), |s: &Term, cx: &mut Cx<Term>| {
        let pass_tab = s.pass_tab;
        let term = Element::boxed(Default::default())
            .width(200.0)
            .height(100.0)
            .on_raw_key(cx, move |s: &mut Term, ev: &KeyEvent| {
                s.raw.push(format!("{:?} {:?}", ev.state, ev.logical));
                match ev.logical {
                    KeyCode::Char('x') => Handled::No,
                    KeyCode::Tab if pass_tab => Handled::No,
                    _ => Handled::Yes,
                }
            })
            .on_key(cx, |s: &mut Term, input: &KeyInput| {
                s.typed.push(input.clone())
            })
            .captures_tab(s.capture_tab)
            .ime_cursor_area(Rect::from_xywh(8.0, 16.0, 1.0, 16.0))
            .option_as_meta(OptionAsMeta::Left);
        let other = Element::boxed(Default::default())
            .width(100.0)
            .height(20.0)
            .on_key(cx, |_: &mut Term, _: &KeyInput| {});
        column(vec![term, other])
    })
    .logical_size(Size::new(300.0, 200.0))
    .menu(|_: &Term| {
        MenuBar::new().menu(
            Menu::new("&Shell").item(
                MenuItem::new("New Tab", |s: &mut Term| s.commands.push("new tab"))
                    .shortcut(Shortcut::cmd('t')),
            ),
        )
    })
}

fn focused_app() -> App<Term> {
    let mut app = app();
    app.render_once();
    assert!(app.focus_next());
    app.render_once();
    app
}

#[test]
fn raw_keys_reach_the_element_before_the_toolkit() {
    let mut app = focused_app();
    let focused = app.panes[0].focused;
    // Keys the toolkit would drop or use: F5, Ctrl+D, Ctrl+C, Escape.
    for (code, mods) in [
        (KeyCode::F5, Modifiers::default()),
        (KeyCode::Char('d'), ctrl()),
        (KeyCode::Char('c'), ctrl()),
        (KeyCode::Escape, Modifiers::default()),
        (KeyCode::PageUp, Modifiers::default()),
    ] {
        let out = app.raw_key(0, &press(code, mods));
        assert!(out.consumed && out.ran, "{code:?}");
    }
    // Releases too.
    let release = key(ButtonState::Released, KeyCode::F5, Modifiers::default());
    assert!(app.raw_key(0, &release).consumed);
    // Tab, taken raw, doesn't move focus (the backend never sends the `Key`
    // that would).
    assert!(
        app.raw_key(0, &press(KeyCode::Tab, Modifiers::default()))
            .consumed
    );
    assert_eq!(app.panes[0].focused, focused);
    assert_eq!(
        app.state().raw,
        [
            "Pressed F5",
            "Pressed Char('d')",
            "Pressed Char('c')",
            "Pressed Escape",
            "Pressed PageUp",
            "Released F5",
            "Pressed Tab"
        ]
    );
    // Nothing reached `on_key`: Ctrl+C didn't become `Copy`.
    assert!(app.state().typed.is_empty());
}

#[test]
fn unhandled_raw_keys_fall_through_to_on_key() {
    let mut app = focused_app();
    let out = app.raw_key(0, &press(KeyCode::Char('x'), Modifiers::default()));
    assert!(out.ran && !out.consumed);
    // The backend then sends the text, which `on_key` gets.
    assert!(app.pane_type_text(0, "x"));
    assert_eq!(app.state().typed, [KeyInput::Text("x".into())]);
}

#[test]
fn menu_shortcuts_beat_the_raw_handler() {
    let mut app = focused_app();
    // Cmd+T is the menu's: the element doesn't see it, and the `Key` the
    // backend sends next runs the command.
    let out = app.raw_key(0, &press(KeyCode::Char('t'), command()));
    assert!(!out.consumed && !out.ran);
    assert!(app.state().raw.is_empty());
    assert!(app.menu_key(0, KeyCode::Char('t'), command()));
    assert_eq!(app.state().commands, ["new tab"]);
    // An unclaimed combination reaches the element.
    let other = if cfg!(target_os = "macos") {
        KeyCode::Char('t') // Ctrl+T, when Cmd is the command key
    } else {
        KeyCode::Char('r')
    };
    assert!(app.raw_key(0, &press(other, ctrl())).consumed);
    assert_eq!(app.state().raw.len(), 1);
}

#[test]
fn an_open_menu_takes_keys_first() {
    let mut app = focused_app();
    let active = app.panes[0].menu_ui.borrow_mut().focus_bar();
    if !active {
        return; // no menu bar drawn in the window here
    }
    let down = press(KeyCode::ArrowDown, Modifiers::default());
    assert!(!app.raw_key(0, &down).ran);
    assert!(app.state().raw.is_empty());
}

#[test]
fn nothing_raw_without_a_handler_in_focus() {
    let mut app = app();
    app.render_once();
    let f1 = press(KeyCode::F1, Modifiers::default());
    // Nothing focused.
    assert!(!app.raw_key(0, &f1).ran);
    // The second element has no raw handler.
    app.focus_next();
    app.focus_next();
    app.render_once();
    assert!(!app.raw_key(0, &f1).ran);
    assert!(app.state().raw.is_empty());
}

#[test]
fn captured_tab_stays_with_the_element() {
    let tab = press(KeyCode::Tab, Modifiers::default());
    // Without capture, an unhandled Tab moves focus on.
    let mut app = focused_app();
    app.state.pass_tab = true;
    app.panes[0].dirty = true;
    app.render_once();
    let first = app.panes[0].focused;
    assert!(!app.raw_key(0, &tab).consumed);
    assert!(app.tab_key(0, Modifiers::default()));
    assert_ne!(app.panes[0].focused, first);

    // With it, Tab reaches `on_key` as a tab character and focus stays;
    // Shift+Tab does nothing.
    let mut app = focused_app();
    app.state.pass_tab = true;
    app.state.capture_tab = true;
    app.panes[0].dirty = true;
    app.render_once();
    let first = app.panes[0].focused;
    assert!(!app.raw_key(0, &tab).consumed);
    assert!(app.tab_key(0, Modifiers::default()));
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    assert!(!app.tab_key(0, shift));
    assert_eq!(app.panes[0].focused, first);
    assert_eq!(app.state().typed, [KeyInput::Text("\t".into())]);
}

/// A window that records what the App tells it about keys.
#[derive(Default)]
struct KeyWindow {
    ime: RefCell<Vec<Option<ImeState>>>,
    option: RefCell<Vec<OptionAsMeta>>,
}

impl Window for KeyWindow {
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
        unimplemented!("not presented in this test")
    }
    fn set_ime(&self, state: Option<&ImeState>) {
        self.ime.borrow_mut().push(state.cloned());
    }
    fn set_option_as_meta(&self, option: OptionAsMeta) {
        self.option.borrow_mut().push(option);
    }
}

#[test]
fn custom_elements_get_the_ime_and_option_as_meta() {
    let mut app = app();
    let window = KeyWindow::default();
    app.render_once();
    app.panes[0].sync_ime(&window, None);
    app.panes[0].sync_option_as_meta(&window);
    assert!(window.ime.borrow().is_empty());
    assert!(window.option.borrow().is_empty());

    // Focusing the terminal turns the IME on at its cursor cell.
    assert!(app.focus_next());
    app.render_once();
    app.panes[0].sync_ime(&window, None);
    app.panes[0].sync_option_as_meta(&window);
    let ime = window.ime.borrow()[0].clone().expect("the IME is on");
    let tree = app.panes[0].tree.as_ref().unwrap();
    let b = crate::core::find_focus(tree, app.panes[0].focused.unwrap())
        .unwrap()
        .bounds;
    assert_eq!(
        ime.cursor_area,
        Rect::from_xywh(b.min_x() + 8.0, b.min_y() + 16.0, 1.0, 16.0)
    );
    assert_eq!(*window.option.borrow(), [OptionAsMeta::Left]);

    // Composition goes to `on_key`, as for a text field.
    let preedit = KeyInput::Preedit {
        text: "にほん".into(),
        cursor: Some((0, 9)),
    };
    assert!(app.press_key(preedit.clone()));
    assert_eq!(app.state().typed, [preedit]);

    // Moving on switches both off again.
    assert!(app.focus_next());
    app.render_once();
    app.panes[0].sync_ime(&window, None);
    app.panes[0].sync_option_as_meta(&window);
    assert_eq!(window.ime.borrow().len(), 2);
    assert!(window.ime.borrow()[1].is_none());
    assert_eq!(
        *window.option.borrow(),
        [OptionAsMeta::Left, OptionAsMeta::None]
    );
}
