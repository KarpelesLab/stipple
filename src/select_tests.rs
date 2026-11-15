//! Selectable read-only text (`Element::selectable`) through the `App`:
//! dragging, double- and triple-clicking, keys, the clipboard, the context
//! menu, touch, accessibility, and rich text copied as plain text.

use std::sync::Arc;
use std::time::Duration;

use crate::App;
use crate::core::{AccessNode, Cx, KeyInput, Role, clipboard_text, set_clipboard_text};
use crate::geometry::{PhysicalSize, Point, Size};
use crate::platform::{ButtonState, Event, PointerButton, Touch, TouchPhase};
use crate::render::{Font, InlineIcon, Pixmap, RichText};
use crate::widgets::{TextInput, column, label, paragraph, text_field};

#[derive(Default)]
struct Doc {
    field: TextInput,
}

const CODE: &str = "4006381333931 EAN-13";

/// A 400×200 window: a selectable label (the barcode's text) at the top, a
/// plain label under it, a text field, and a selectable rich line with an
/// icon.
fn app(font: Font) -> App<Doc> {
    App::new(Doc::default(), |s: &Doc, cx: &mut Cx<Doc>| {
        let coin = Arc::new(Pixmap::new(PhysicalSize::new(8, 8)));
        column(vec![
            label(cx, CODE).selectable(),
            label(cx, "Not selectable"),
            text_field(cx, &s.field, |s| &mut s.field),
            paragraph(
                cx,
                RichText::new()
                    .push("Pay 120")
                    .icon(InlineIcon::new(coin).alt(" coins"))
                    .bold(" now"),
            )
            .selectable(),
        ])
    })
    .font(font)
    .logical_size(Size::new(400.0, 200.0))
}

fn font() -> Option<Font> {
    let f = Font::system_default();
    if f.is_none() {
        eprintln!("skipping: no system font");
    }
    f
}

fn button(state: ButtonState, button: PointerButton, at: Point) -> Event {
    Event::PointerButton {
        button,
        state,
        position: at,
    }
}

fn click(app: &mut App<Doc>, at: Point) {
    app.pointer_event(button(ButtonState::Pressed, PointerButton::Left, at));
    app.pointer_event(button(ButtonState::Released, PointerButton::Left, at));
}

fn drag(app: &mut App<Doc>, from: Point, to: Point) {
    app.pointer_event(button(ButtonState::Pressed, PointerButton::Left, from));
    app.pointer_event(Event::PointerMoved { position: to });
    app.pointer_event(button(ButtonState::Released, PointerButton::Left, to));
}

/// The text node named `name`, and the point `frac` of the way along it.
fn text(app: &mut App<Doc>, name: &str) -> AccessNode {
    app.render_once();
    app.find_text(name).unwrap_or_else(|| panic!("no {name:?}"))
}

fn along(node: &AccessNode, frac: f64) -> Point {
    let b = node.bounds;
    Point::new(b.min_x() + b.width() * frac, b.center().y)
}

fn copied(app: &mut App<Doc>) -> String {
    set_clipboard_text("");
    app.press_key(KeyInput::Copy);
    clipboard_text()
}

#[test]
fn dragging_selects_and_copy_copies_it() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    let code = text(&mut app, CODE);
    // From before the first digit into the text.
    drag(&mut app, along(&code, 0.0), along(&code, 0.1));
    app.render_once();
    let got = copied(&mut app);
    assert!(!got.is_empty() && CODE.starts_with(&got), "{got:?}");
    assert!(got.len() < CODE.len());
    // Screen readers see the selection.
    let node = app.find_text(CODE).unwrap();
    assert_eq!(node.text_selection, Some((0, got.len())));
    // Shift+End extends it to the end; Select All takes everything.
    app.press_key(KeyInput::SelectEnd);
    assert_eq!(copied(&mut app), CODE);
    // A click elsewhere clears it: nothing to copy.
    click(&mut app, Point::new(390.0, 190.0));
    app.render_once();
    assert_eq!(copied(&mut app), "");
    assert_eq!(app.find_text(CODE).unwrap().text_selection, None);
}

#[test]
fn double_click_selects_a_word_and_triple_click_the_line() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    let code = text(&mut app, CODE);
    let at = along(&code, 0.2);
    click(&mut app, at);
    click(&mut app, at);
    app.render_once();
    assert_eq!(copied(&mut app), "4006381333931");
    click(&mut app, at);
    app.render_once();
    assert_eq!(copied(&mut app), CODE);
}

#[test]
fn plain_text_and_fields_dont_take_the_selection() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    let plain = text(&mut app, "Not selectable");
    drag(&mut app, along(&plain, 0.0), along(&plain, 1.0));
    app.render_once();
    assert_eq!(copied(&mut app), "");
    // A selection, then a click into the field: Ctrl+C goes to the field.
    let code = text(&mut app, CODE);
    drag(&mut app, along(&code, 0.0), along(&code, 1.0));
    app.render_once();
    assert_eq!(copied(&mut app), CODE);
    let field = app
        .find(Role::TextField, "")
        .expect("the field")
        .bounds
        .center();
    click(&mut app, field);
    app.render_once();
    app.type_text("typed");
    app.press_key(KeyInput::SelectAll);
    assert_eq!(copied(&mut app), "typed");
}

#[test]
fn rich_text_copies_its_images_alt_text() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    let rich = text(&mut app, "Pay 120 coins now");
    click(&mut app, along(&rich, 0.5));
    app.press_key(KeyInput::SelectAll);
    app.render_once();
    assert_eq!(copied(&mut app), "Pay 120 coins now");
    let node = app.find_text("Pay 120 coins now").unwrap();
    assert_eq!(node.text_selection, Some((0, "Pay 120 coins now".len())));
}

fn menu_items(app: &App<Doc>) -> Vec<AccessNode> {
    app.accessibility_tree()
        .expect("a frame")
        .descendants()
        .into_iter()
        .filter(|n| n.role == Role::MenuItem)
        .cloned()
        .collect()
}

#[test]
fn right_click_selects_the_word_and_offers_copy() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    let code = text(&mut app, CODE);
    assert!(app.right_click_at(along(&code, 0.2)));
    app.render_once();
    let names: Vec<String> = menu_items(&app).into_iter().map(|n| n.name).collect();
    assert_eq!(names, ["Copy", "Select All"]);
    set_clipboard_text("");
    assert!(app.click(Role::MenuItem, "Copy"));
    app.render_once();
    assert_eq!(clipboard_text(), "4006381333931");
}

#[test]
fn a_tap_doesnt_select_but_a_long_press_selects_a_word() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    let code = text(&mut app, CODE);
    let at = along(&code, 0.2);
    let finger = |phase| Touch {
        id: 1,
        phase,
        position: at,
    };
    app.touch(finger(TouchPhase::Started));
    app.touch(finger(TouchPhase::Ended));
    app.render_once();
    assert_eq!(copied(&mut app), "");
    app.touch(finger(TouchPhase::Started));
    app.panes[0].touch_epoch -= Duration::from_secs_f64(0.6);
    assert!(app.advance_long_press());
    app.render_once();
    assert_eq!(copied(&mut app), "4006381333931");
    let names: Vec<String> = menu_items(&app).into_iter().map(|n| n.name).collect();
    assert_eq!(names, ["Copy", "Select All"]);
    app.touch(finger(TouchPhase::Ended));
    // Select All from the menu.
    assert!(app.click(Role::MenuItem, "Select All"));
    app.render_once();
    assert_eq!(copied(&mut app), CODE);
}

#[test]
fn the_selection_is_highlighted_under_the_text() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    let code = text(&mut app, CODE);
    let before = app.render_once();
    let at = along(&code, 0.2);
    click(&mut app, at);
    click(&mut app, at);
    let after = app.render_once();
    // Between two digits, above the baseline: the background before, the
    // selection's tint after.
    let (x, y) = (
        code.bounds.min_x() as u32 + 2,
        code.bounds.min_y() as u32 + 1,
    );
    assert_ne!(before.pixel(x, y), after.pixel(x, y));
    let theme = crate::style::Theme::light();
    let bg = before.pixel(x, y).unwrap();
    let px = after.pixel(x, y).unwrap();
    let tint = theme.palette.selection;
    // Toward the selection color's hue (blue in the light theme).
    assert!(
        (px[2] as i32 - px[0] as i32) > (bg[2] as i32 - bg[0] as i32),
        "{bg:?} -> {px:?} ({tint:?})"
    );
}
