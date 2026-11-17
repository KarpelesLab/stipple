//! Editing text with a finger through the `App`, fed synthetic touches as a
//! touch-screen backend delivers them: a tap placing the caret, double taps
//! and long presses selecting a word, the selection handles, the edit menu
//! (drawn, and the system's through the window), and the focused field kept
//! above the on-screen keyboard.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crate::App;
use crate::app_text_touch::HandleKind;
use crate::core::{
    Cx, LayoutNode, NodeContent, Role, clipboard_text, leaf_layout, set_clipboard_text,
};
use crate::geometry::{PhysicalSize, Point, ScaleFactor, Size};
use crate::platform::{
    ButtonState, EditAction, EditMenu, Event, PointerButton, Touch, TouchPhase, Window, WindowId,
};
use crate::render::{Font, Surface};
use crate::widgets::{PasswordField, TextInput, column, password_field, text_area, text_field};

const LINE: &str = "hello brave new world";

struct Form {
    field: TextInput,
    notes: TextInput,
    pin: PasswordField,
}

impl Default for Form {
    fn default() -> Self {
        Form {
            field: TextInput::from_text(LINE),
            notes: TextInput::from_text("first line\nsecond line"),
            pin: PasswordField::default(),
        }
    }
}

fn view(s: &Form, cx: &mut Cx<Form>) -> crate::core::Element {
    column(vec![
        // Room for the edit menu above the field.
        crate::widgets::spacer().height(80.0),
        text_field(cx, &s.field, |s| &mut s.field).width(360.0),
        text_area(cx, &s.notes, |s| &mut s.notes),
        password_field(cx, &s.pin, |s| &mut s.pin),
    ])
}

/// A 400×500 window: a field holding [`LINE`] 80 pixels down, a text area
/// and a password field.
fn app(font: Font) -> App<Form> {
    App::new(Form::default(), view)
        .font(font)
        .logical_size(Size::new(400.0, 500.0))
}

fn font() -> Option<Font> {
    let f = Font::system_default();
    if f.is_none() {
        eprintln!("skipping: no system font");
    }
    f
}

fn finger(id: u64, phase: TouchPhase, at: Point) -> Touch {
    Touch {
        id,
        phase,
        position: at,
    }
}

fn tap(app: &mut App<Form>, at: Point) {
    app.touch(finger(1, TouchPhase::Started, at));
    app.touch(finger(1, TouchPhase::Ended, at));
    app.render_once();
}

/// Let the double-tap interval pass.
fn pause(app: &mut App<Form>) {
    app.panes[0].clicks.reset();
}

/// The text leaf showing `text`.
fn leaf<'a>(node: &'a LayoutNode, text: &str) -> Option<&'a LayoutNode> {
    if let NodeContent::Text { text: t, .. } = &node.content
        && t.as_str() == text
    {
        return Some(node);
    }
    node.children.iter().find_map(|c| leaf(c, text))
}

/// Where byte `i` of the leaf showing `text` is: the middle of its line.
fn point_of(app: &mut App<Form>, text: &str, i: usize) -> Point {
    app.render_once();
    let font = app.font.get();
    let tree = app.panes[0].tree.as_ref().unwrap();
    let leaf = leaf(tree, text).unwrap_or_else(|| panic!("no {text:?}"));
    let layout = leaf_layout(leaf, font).unwrap();
    let p = layout.caret_position(i);
    let b = leaf.text_rect();
    let h = layout.line_height(layout.line_of(i));
    Point::new(b.min_x() + p.x, b.min_y() + p.y + h / 2.0)
}

fn field(app: &App<Form>) -> &TextInput {
    &app.state().field
}

fn selected(input: &TextInput) -> &str {
    match input.buffer().selection() {
        Some((a, b)) => &input.text()[a..b],
        None => "",
    }
}

fn handles(app: &App<Form>) -> Vec<HandleKind> {
    app.panes[0]
        .text_touch
        .handles
        .iter()
        .map(|h| h.kind)
        .collect()
}

fn menu_items(app: &App<Form>) -> Vec<String> {
    let tree = app.accessibility_tree().expect("a frame was built");
    tree.descendants()
        .into_iter()
        .filter(|n| n.role == Role::MenuItem)
        .map(|n| n.name.clone())
        .collect()
}

/// The bounds of the first element with `role`.
fn focused_or<S: 'static>(app: &mut App<S>, role: Role) -> crate::geometry::Rect {
    app.render_once();
    let tree = app.accessibility_tree().unwrap();
    tree.descendants()
        .into_iter()
        .find(|n| n.role == role)
        .map(|n| n.bounds)
        .unwrap_or_else(|| panic!("no {role:?}"))
}

/// Double-tap byte `i` of the field.
fn double_tap(app: &mut App<Form>, i: usize) {
    let at = point_of(app, LINE, i);
    tap(app, at);
    tap(app, at);
}

#[test]
fn a_tap_places_the_caret_when_the_finger_lifts() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    // Between "brave" and " new".
    let at = point_of(&mut app, LINE, 11);
    app.touch(finger(1, TouchPhase::Started, at));
    app.render_once();
    assert_eq!(app.focused(), None, "not on the press");
    app.touch(finger(1, TouchPhase::Ended, at));
    app.render_once();
    assert!(app.focused().is_some());
    assert_eq!(field(&app).buffer().caret(), 11);
    assert_eq!(handles(&app), [HandleKind::Caret]);
    assert_eq!(app.edit_menu(), None);
    // Typing hides the handle.
    app.type_text("!");
    app.render_once();
    assert_eq!(field(&app).text(), "hello brave! new world");
    assert!(handles(&app).is_empty());
}

#[test]
fn a_tap_on_the_caret_shows_and_hides_the_edit_menu() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    let at = point_of(&mut app, LINE, 5);
    tap(&mut app, at);
    pause(&mut app);
    tap(&mut app, at);
    let menu = app.edit_menu().expect("the menu shows");
    assert!(!menu.cut && !menu.copy && menu.paste && menu.select_all);
    assert_eq!(menu_items(&app), ["Paste", "Select All"]);
    // Above the caret.
    let bar = app.panes[0].text_touch.bar_rect.unwrap();
    assert!(bar.max_y() <= menu.rect.min_y(), "{bar:?} over {menu:?}");
    pause(&mut app);
    tap(&mut app, at);
    assert_eq!(app.edit_menu(), None);
    assert!(menu_items(&app).is_empty());
}

#[test]
fn a_double_tap_selects_the_word_with_handles_and_the_menu() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    double_tap(&mut app, 8);
    app.render_once();
    assert_eq!(selected(field(&app)), "brave");
    assert_eq!(handles(&app), [HandleKind::Start, HandleKind::End]);
    let menu = app.edit_menu().expect("the menu shows");
    assert!(menu.cut && menu.copy && menu.paste && menu.select_all);
    assert_eq!(menu_items(&app), ["Cut", "Copy", "Paste", "Select All"]);
    // The handles are drawn, in the theme's primary color.
    let pixmap = app.render_once();
    let knob = app.panes[0].text_touch.handles[0];
    let c = Point::new(knob.tip.x, knob.tip.y + 8.0);
    let px = pixmap.pixel(c.x as u32, c.y as u32).unwrap();
    let primary = crate::style::Theme::light().palette.primary;
    assert_eq!([px[0], px[1], px[2]], [primary.r, primary.g, primary.b]);
}

#[test]
fn a_long_press_selects_the_word_instead_of_a_context_menu_and_extends() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    let at = point_of(&mut app, LINE, 8);
    app.touch(finger(1, TouchPhase::Started, at));
    assert!(
        app.panes[0].long_press_timer.is_some(),
        "a field long-presses"
    );
    crate::platform::time::test_clock::advance(Duration::from_secs_f64(0.6));
    assert!(app.advance_long_press());
    app.render_once();
    assert!(app.focused().is_some());
    assert_eq!(selected(field(&app)), "brave");
    assert!(app.edit_menu().is_some());
    // The finger moves on, over "new": the selection grows a word's end at
    // a time from "brave", the menu hiding meanwhile.
    let to = point_of(&mut app, LINE, 15);
    app.touch(finger(
        1,
        TouchPhase::Moved,
        Point::new((at.x + to.x) / 2.0, at.y),
    ));
    app.touch(finger(1, TouchPhase::Moved, to));
    app.render_once();
    assert_eq!(selected(field(&app)), "brave new");
    assert_eq!(app.edit_menu(), None);
    // Back inside the word: it stays selected.
    let back = point_of(&mut app, LINE, 9);
    app.touch(finger(1, TouchPhase::Moved, back));
    app.render_once();
    assert_eq!(selected(field(&app)), "brave");
    // Lifting shows the menu again, and picks nothing.
    app.touch(finger(1, TouchPhase::Moved, to));
    app.touch(finger(1, TouchPhase::Ended, to));
    app.render_once();
    assert_eq!(selected(field(&app)), "brave new");
    assert_eq!(menu_items(&app), ["Cut", "Copy", "Paste", "Select All"]);
}

#[test]
fn dragging_the_handles_moves_the_selections_ends() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    double_tap(&mut app, 8);
    app.render_once();
    let drag = |app: &mut App<Form>, kind: HandleKind, to_index: usize| {
        let h = *app.panes[0]
            .text_touch
            .handles
            .iter()
            .find(|h| h.kind == kind)
            .unwrap();
        // The finger takes the knob below the line, a little off its center.
        let grab = Point::new(h.tip.x + 3.0, h.tip.y + 10.0);
        let target = point_of(app, LINE, to_index);
        let to = Point::new(target.x + 3.0, grab.y + (target.y - h.aim().y));
        app.touch(finger(2, TouchPhase::Started, grab));
        assert_eq!(app.edit_menu(), None, "hidden while dragging");
        app.touch(finger(
            2,
            TouchPhase::Moved,
            Point::new(grab.x + 12.0, grab.y),
        ));
        app.touch(finger(2, TouchPhase::Moved, to));
        app.touch(finger(2, TouchPhase::Ended, to));
        app.render_once();
    };
    drag(&mut app, HandleKind::End, 15);
    assert_eq!(selected(field(&app)), "brave new");
    assert!(app.focused().is_some());
    drag(&mut app, HandleKind::Start, 0);
    assert_eq!(selected(field(&app)), "hello brave new");
    // The menu comes back once the handle is let go.
    assert!(app.edit_menu().is_some_and(|m| m.copy));
    // Nothing scrolled or tapped on the way.
    assert_eq!(field(&app).text(), LINE);
}

#[test]
fn the_caret_handle_drags_the_caret() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    let at = point_of(&mut app, LINE, 5);
    tap(&mut app, at);
    let h = app.panes[0].text_touch.handles[0];
    assert_eq!(h.kind, HandleKind::Caret);
    let grab = Point::new(h.tip.x, h.tip.y + 8.0);
    let target = point_of(&mut app, LINE, 16);
    let to = Point::new(target.x, grab.y);
    app.touch(finger(2, TouchPhase::Started, grab));
    app.touch(finger(2, TouchPhase::Moved, to));
    app.touch(finger(2, TouchPhase::Ended, to));
    app.render_once();
    assert_eq!(field(&app).buffer().caret(), 16);
    assert_eq!(field(&app).buffer().selection(), None);
    // A tap on the handle shows the menu.
    let h = app.panes[0].text_touch.handles[0];
    let knob = Point::new(h.tip.x, h.tip.y + 8.0);
    tap(&mut app, knob);
    assert!(app.edit_menu().is_some_and(|m| m.paste && !m.copy));
}

#[test]
fn the_drawn_menus_items_edit_the_field() {
    let Some(font) = font() else { return };
    set_clipboard_text("");
    let mut app = app(font);
    double_tap(&mut app, 8);
    app.render_once();
    // Copy, with a tap on the drawn item.
    let copy = app.find(Role::MenuItem, "Copy").unwrap().bounds.center();
    tap(&mut app, copy);
    assert_eq!(clipboard_text(), "brave");
    assert_eq!(app.edit_menu(), None, "the menu closes");
    assert_eq!(selected(field(&app)), "brave", "the selection stays");
    // Cut.
    app.panes[0].text_touch.menu = true;
    app.render_once();
    app.render_once();
    let cut = app.find(Role::MenuItem, "Cut").unwrap().bounds.center();
    tap(&mut app, cut);
    assert_eq!(field(&app).text(), "hello  new world");
    // Paste, through the App's own entry point.
    set_clipboard_text("bold");
    assert!(app.edit_action(EditAction::Paste));
    assert_eq!(field(&app).text(), "hello bold new world");
    // Select All keeps the menu up, now without Select All.
    assert!(app.edit_action(EditAction::SelectAll));
    app.render_once();
    app.render_once();
    assert_eq!(selected(field(&app)), "hello bold new world");
    assert_eq!(menu_items(&app), ["Cut", "Copy", "Paste"]);
}

#[test]
fn a_password_field_offers_neither_cut_nor_copy() {
    let Some(font) = font() else { return };
    let mut form = Form::default();
    form.pin.set_text("1234");
    let mut app = App::new(form, view)
        .font(font)
        .logical_size(Size::new(400.0, 500.0));
    let at = focused_or(&mut app, Role::PasswordField).center();
    app.touch(finger(1, TouchPhase::Started, at));
    crate::platform::time::test_clock::advance(Duration::from_secs_f64(0.6));
    assert!(app.advance_long_press());
    app.touch(finger(1, TouchPhase::Ended, at));
    app.render_once();
    let menu = app.edit_menu().expect("the menu shows");
    assert!(!menu.cut && !menu.copy && menu.paste);
}

#[test]
fn a_finger_scrolling_over_a_field_neither_focuses_it_nor_selects() {
    let Some(font) = font() else { return };
    #[derive(Default)]
    struct Long {
        fields: Vec<TextInput>,
    }
    let mut app = App::new(
        Long {
            fields: (0..12)
                .map(|i| TextInput::from_text(format!("field {i}")))
                .collect(),
        },
        |s: &Long, cx: &mut Cx<Long>| {
            let fields = (0..s.fields.len())
                .map(|i| text_field(cx, &s.fields[i], move |s| &mut s.fields[i]).height(60.0))
                .collect();
            crate::widgets::scroll(cx).children(fields)
        },
    )
    .font(font)
    .logical_size(Size::new(300.0, 300.0));
    app.render_once();
    let at = Point::new(100.0, 200.0);
    app.touch(finger(1, TouchPhase::Started, at));
    for dy in [5.0, 20.0, 60.0, 100.0] {
        app.touch(finger(1, TouchPhase::Moved, Point::new(at.x, at.y - dy)));
    }
    app.touch(finger(1, TouchPhase::Ended, Point::new(at.x, at.y - 100.0)));
    app.render_once();
    assert_eq!(app.focused(), None);
    assert!(app.panes[0].text_touch.handles.is_empty());
    // A tap does focus it.
    app.touch(finger(2, TouchPhase::Started, at));
    app.touch(finger(2, TouchPhase::Ended, at));
    assert!(app.focused().is_some());
}

#[test]
fn the_tapped_field_is_kept_above_the_on_screen_keyboard() {
    let Some(font) = font() else { return };
    #[derive(Default)]
    struct Long {
        fields: Vec<TextInput>,
    }
    let mut app = App::new(
        Long {
            fields: (0..8)
                .map(|i| TextInput::from_text(format!("field {i}")))
                .collect(),
        },
        |s: &Long, cx: &mut Cx<Long>| {
            let fields = (0..s.fields.len())
                .map(|i| text_field(cx, &s.fields[i], move |s| &mut s.fields[i]).height(60.0))
                .collect();
            crate::widgets::scroll(cx).children(fields)
        },
    )
    .font(font)
    .logical_size(Size::new(300.0, 400.0));
    app.render_once();
    // The sixth field, at 300..360.
    let at = Point::new(100.0, 330.0);
    app.touch(finger(1, TouchPhase::Started, at));
    app.touch(finger(1, TouchPhase::Ended, at));
    app.render_once();
    assert!(app.focused().is_some());
    // The keyboard comes up over the bottom 200 pixels.
    app.set_keyboard_inset(200.0);
    app.render_once();
    let tree = app.accessibility_tree().unwrap();
    let field = tree
        .descendants()
        .into_iter()
        .find(|n| n.focused)
        .map(|n| n.bounds)
        .expect("the focused field");
    assert!(field.max_y() <= 200.0, "{field:?} is above the keyboard");
    // Its handle came along.
    let h = app.panes[0].text_touch.handles[0];
    assert!(h.tip.y <= 200.0, "{h:?}");
}

/// A touch screen's window whose system shows the edit menu: it records
/// what the App asks for.
struct PhoneWindow(Rc<RefCell<Vec<Option<EditMenu>>>>);

impl Window for PhoneWindow {
    fn id(&self) -> WindowId {
        WindowId(1)
    }
    fn inner_size(&self) -> PhysicalSize {
        PhysicalSize::new(400, 500)
    }
    fn scale_factor(&self) -> ScaleFactor {
        ScaleFactor::IDENTITY
    }
    fn request_redraw(&self) {}
    fn set_title(&self, _title: &str) {}
    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(crate::platform::backend::headless::HeadlessSurface::new(self.inner_size()).0)
    }
    fn has_edit_menu(&self) -> bool {
        true
    }
    fn show_edit_menu(&self, menu: Option<&EditMenu>) {
        self.0.borrow_mut().push(menu.copied());
    }
}

#[test]
fn the_systems_edit_menu_shows_instead_and_its_picks_apply() {
    let Some(font) = font() else { return };
    set_clipboard_text("");
    let seen = Rc::new(RefCell::new(Vec::new()));
    let text = Rc::new(RefCell::new(String::new()));
    let app = {
        let text = text.clone();
        App::new(Form::default(), move |s: &Form, cx: &mut Cx<Form>| {
            *text.borrow_mut() = s.field.text().to_string();
            view(s, cx)
        })
        .font(font)
        .logical_size(Size::new(400.0, 500.0))
    };
    // Where "brave" is, from a copy laid out the same.
    let at = {
        let mut probe = App::new(Form::default(), view)
            .font(Font::system_default().unwrap())
            .logical_size(Size::new(400.0, 500.0));
        point_of(&mut probe, LINE, 8)
    };
    let (_, mut handler) = app.into_handler();
    let window = PhoneWindow(seen.clone());
    let mut send = |e: Event| {
        handler(e, &window);
    };
    send(Event::RedrawRequested);
    let press = |state| Event::PointerButton {
        button: PointerButton::Left,
        state,
        position: at,
    };
    for _ in 0..2 {
        send(Event::Touch(finger(1, TouchPhase::Started, at)));
        send(press(ButtonState::Pressed));
        send(Event::Touch(finger(1, TouchPhase::Ended, at)));
        send(press(ButtonState::Released));
        send(Event::RedrawRequested);
    }
    let shown = seen.borrow().last().copied().flatten();
    let menu = shown.expect("the system's menu was asked for");
    assert!(menu.cut && menu.copy && menu.paste);
    assert!(menu.rect.contains(at), "{menu:?} at {at:?}");
    // The system's Cut comes back as an event.
    send(Event::EditAction(EditAction::Cut));
    send(Event::RedrawRequested);
    assert_eq!(*text.borrow(), "hello  new world");
    assert_eq!(clipboard_text(), "brave");
    assert_eq!(seen.borrow().last().copied(), Some(None), "closed");
}

#[test]
fn the_rich_editor_selects_and_copies_with_a_finger_too() {
    use crate::widgets::{RichEditor, rich_editor};
    let Some(font) = font() else { return };
    set_clipboard_text("");
    struct Doc {
        editor: RichEditor,
    }
    let mut app = App::new(
        Doc {
            editor: RichEditor::from_markdown("hello **brave** new world"),
        },
        |s: &Doc, cx: &mut Cx<Doc>| {
            column(vec![
                crate::widgets::spacer().height(80.0),
                rich_editor(cx, &s.editor, |s| &mut s.editor),
            ])
        },
    )
    .font(font)
    .logical_size(Size::new(500.0, 400.0));
    app.render_once();
    fn containing<'a>(node: &'a LayoutNode, part: &str) -> Option<&'a LayoutNode> {
        if let NodeContent::Text { text, .. } = &node.content
            && text.contains(part)
        {
            return Some(node);
        }
        node.children.iter().find_map(|c| containing(c, part))
    }
    let at = {
        let font = app.font.get();
        let tree = app.panes[0].tree.as_ref().unwrap();
        let leaf = containing(tree, "brave").expect("the editor's text");
        let NodeContent::Text { text, .. } = &leaf.content else {
            unreachable!()
        };
        let i = text.find("brave").unwrap() + 2;
        let layout = leaf_layout(leaf, font).unwrap();
        let p = layout.caret_position(i);
        let b = leaf.text_rect();
        Point::new(b.min_x() + p.x, b.min_y() + p.y + 6.0)
    };
    app.touch(finger(1, TouchPhase::Started, at));
    crate::platform::time::test_clock::advance(Duration::from_secs_f64(0.6));
    assert!(app.advance_long_press());
    app.touch(finger(1, TouchPhase::Ended, at));
    app.render_once();
    let menu = app.edit_menu().expect("the menu shows");
    assert!(menu.copy && menu.cut);
    assert_eq!(app.panes[0].text_touch.handles.len(), 2);
    assert!(app.edit_action(EditAction::Copy));
    assert_eq!(clipboard_text(), "brave");
}

#[test]
fn a_tap_on_an_empty_fields_caret_shows_the_edit_menu() {
    let Some(font) = font() else { return };
    let mut app = app(font);
    let start = point_of(&mut app, LINE, 0);
    let inside = point_of(&mut app, LINE, 5);
    tap(&mut app, inside);
    pause(&mut app);
    assert!(app.edit_action(EditAction::SelectAll));
    app.render_once();
    assert!(app.edit_action(EditAction::Cut));
    app.render_once();
    assert_eq!(field(&app).text(), "");
    // The empty field lays out a lone space; a tap just inside it is on the
    // caret all the same: the menu, with Paste only.
    let at = Point::new(start.x + 2.0, start.y);
    tap(&mut app, at);
    let menu = app.edit_menu().expect("the menu shows");
    assert!(!menu.cut && !menu.copy && menu.paste && !menu.select_all);
    pause(&mut app);
    tap(&mut app, at);
    assert_eq!(app.edit_menu(), None);
}
