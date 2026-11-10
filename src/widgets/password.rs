//! A password field: a single-line text field that shows a bullet per
//! character, keeps its text off the clipboard and out of the input method,
//! and can be revealed with an eye button.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::core::{Align, Axis, Cx, Element, KeyInput, Role};
use crate::geometry::Insets;
use crate::render::{Color, Svg, TextDirection};
use crate::style::Theme;

use super::{EditBuffer, placeholder};

/// What a hidden character shows as.
const BULLET: char = '•';

/// The state of a [`password_field`]: the text, and whether it is shown.
/// Keep one in your app state per field.
///
/// The field's edits keep no undo history (which would hold copies of the
/// password), and [`clear`](Self::clear) drops the text.
///
/// ```
/// use stipple::core::KeyInput;
/// use stipple::widgets::PasswordField;
/// let mut pw = PasswordField::new();
/// pw.apply(&KeyInput::Text("hunter2".into()));
/// assert_eq!(pw.text(), "hunter2");
/// assert_eq!(pw.shown(), "•••••••");
/// pw.apply(&KeyInput::SelectAll);
/// pw.apply(&KeyInput::Cut); // refused: the text stays
/// assert_eq!(pw.text(), "hunter2");
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PasswordField {
    buffer: EditBuffer,
    revealed: bool,
    reveal_button: bool,
    label: String,
}

impl PasswordField {
    /// An empty, hidden field with no reveal button, named "Password" to
    /// assistive technologies.
    pub fn new() -> Self {
        Self {
            label: "Password".into(),
            ..Self::default()
        }
    }

    /// Show an eye button at the end of the field that reveals the text
    /// while it's on.
    pub fn with_reveal_button(mut self, on: bool) -> Self {
        self.reveal_button = on;
        self
    }

    /// The name assistive technologies read for the field (they never read
    /// its text).
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// The password.
    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// Whether the field is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Replace the text (caret at the end).
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.buffer = EditBuffer::from_text(text);
        self.buffer.clear_history();
    }

    /// Empty the field.
    pub fn clear(&mut self) {
        self.set_text(String::new());
    }

    /// Whether the text is shown in clear.
    pub fn is_revealed(&self) -> bool {
        self.revealed
    }

    /// Show the text in clear (`true`) or as bullets.
    pub fn set_revealed(&mut self, revealed: bool) {
        self.revealed = revealed;
    }

    /// Flip [`is_revealed`](Self::is_revealed).
    pub fn toggle_revealed(&mut self) {
        self.revealed = !self.revealed;
    }

    /// The text as the field draws it: a bullet per character, or the text
    /// itself while revealed.
    pub fn shown(&self) -> String {
        if self.revealed {
            self.buffer.text().to_string()
        } else {
            mask(self.buffer.text())
        }
    }

    /// Apply a key to the field, as it does while focused. Copying and
    /// cutting do nothing (the text never reaches the clipboard), nor do undo
    /// and redo; word motions go to the ends, since words would tell where
    /// the password has spaces; composition (preedit) is ignored, as the
    /// field takes no input method.
    pub fn apply(&mut self, input: &KeyInput) {
        let b = &mut self.buffer;
        match input {
            KeyInput::Copy
            | KeyInput::Cut
            | KeyInput::Undo
            | KeyInput::Redo
            | KeyInput::Preedit { .. }
            | KeyInput::Enter
            | KeyInput::Escape => {}
            KeyInput::WordLeft | KeyInput::Up => b.home(),
            KeyInput::WordRight | KeyInput::Down => b.end(),
            KeyInput::SelectWordLeft | KeyInput::SelectUp => b.select_home(),
            KeyInput::SelectWordRight | KeyInput::SelectDown => b.select_end(),
            KeyInput::SelectWord | KeyInput::SelectLine => b.select_all(),
            KeyInput::DeleteWordBackward => {
                if b.selection().is_none() {
                    b.select_home();
                }
                b.backspace();
            }
            KeyInput::DeleteWordForward => {
                if b.selection().is_none() {
                    b.select_end();
                }
                b.delete();
            }
            other => b.apply(other),
        }
        b.clear_history();
    }

    /// Put the caret at `index` of the shown text (or extend the selection
    /// there), as a click or drag does.
    pub fn place(&mut self, shown_index: usize, extend: bool) {
        let index = if self.revealed {
            shown_index.min(self.buffer.text().len())
        } else {
            byte_of_char(self.buffer.text(), shown_index / BULLET.len_utf8())
        };
        if extend {
            self.buffer.extend_to(index);
        } else {
            self.buffer.place_caret(index);
        }
    }

    /// The caret and selection in the shown text.
    fn shown_marks(&self) -> (usize, Option<(usize, usize)>) {
        let b = &self.buffer;
        if self.revealed {
            return (b.caret(), b.selection());
        }
        let at = |i: usize| b.text()[..i].chars().count() * BULLET.len_utf8();
        (at(b.caret()), b.selection().map(|(s, e)| (at(s), at(e))))
    }
}

/// A bullet per character of `text`.
fn mask(text: &str) -> String {
    std::iter::repeat_n(BULLET, text.chars().count()).collect()
}

/// The byte offset of character `n` of `text` (its length past the end).
fn byte_of_char(text: &str, n: usize) -> usize {
    text.char_indices().nth(n).map_or(text.len(), |(i, _)| i)
}

thread_local! {
    /// The eye icons, by stroke color and whether they're crossed out.
    static EYES: RefCell<HashMap<(Color, bool), Arc<Svg>>> = RefCell::new(HashMap::new());
}

/// An eye icon in `color`, crossed out when `crossed`.
fn eye(color: Color, crossed: bool) -> Option<Arc<Svg>> {
    EYES.with(|eyes| {
        if let Some(svg) = eyes.borrow().get(&(color, crossed)) {
            return Some(svg.clone());
        }
        let stroke = format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b);
        let slash = if crossed {
            format!(r#"<path d="M3 3l18 18" stroke="{stroke}" stroke-width="2" stroke-linecap="round"/>"#)
        } else {
            String::new()
        };
        let doc = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24"><path d="M1.5 12s4-7 10.5-7 10.5 7 10.5 7-4 7-10.5 7S1.5 12 1.5 12z" fill="none" stroke="{stroke}" stroke-width="2" stroke-linejoin="round"/><circle cx="12" cy="12" r="3" fill="none" stroke="{stroke}" stroke-width="2"/>{slash}</svg>"#
        );
        let svg = Arc::new(Svg::parse(doc.as_bytes()).ok()?);
        eyes.borrow_mut().insert((color, crossed), svg.clone());
        Some(svg)
    })
}

/// A single-line password field showing `state`'s text as bullets, with the
/// caret and selection, wired to keyboard and pointer input like
/// [`text_editor`](super::text_editor). `field` is an accessor returning the
/// [`PasswordField`] inside the app state.
///
/// What it does differently from a text field:
///
/// - The text shows as a bullet per character, unless revealed (by the eye
///   button, with [`PasswordField::with_reveal_button`], or
///   [`PasswordField::set_revealed`]).
/// - Copy and cut do nothing, even revealed; paste works.
/// - It takes no input method: typing goes straight in (an IME would see the
///   password, and might remember it). Assistive technologies get a
///   [`Role::PasswordField`] named by the field's label, never its text.
/// - On macOS, secure keyboard entry (`EnableSecureEventInput`) is on while it
///   has focus, so other processes can't read the keystrokes.
///
/// Default width 200 logical px; override with `.width(..)`.
pub fn password_field<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    state: &PasswordField,
    field: impl Fn(&mut S) -> &mut PasswordField + Copy + 'static,
) -> Element {
    password_field_with(cx, theme, state, Rc::new(move |s, f| f(field(s))), None)
}

/// Runs a change on a [`PasswordField`] wherever it lives.
pub(crate) type EditFn<S> = Rc<dyn Fn(&mut S, &mut dyn FnMut(&mut PasswordField))>;
/// Sees each key first; `true` means it handled the key.
pub(crate) type KeyFilter<S> = Rc<dyn Fn(&mut S, &KeyInput) -> bool>;

/// [`password_field`] for a field reached through `edit` (a drawn message
/// box keeps its own), with `keys` seeing each key first.
pub(crate) fn password_field_with<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    state: &PasswordField,
    edit: EditFn<S>,
    keys: Option<KeyFilter<S>>,
) -> Element {
    let (caret, selection) = state.shown_marks();
    let e = edit.clone();
    let text = Element::text(
        placeholder(state.shown()),
        theme.font_size,
        theme.palette.text,
    )
    .direction(TextDirection::Ltr)
    .grow(1.0)
    .shrink(1.0)
    .caret(caret)
    .selection(selection)
    .clip()
    .on_text_pos(cx, move |s, index, extend| {
        e(s, &mut |p| p.place(index, extend))
    });
    let mut children = vec![text];
    if state.reveal_button
        && let Some(icon) = eye(theme.palette.text_muted, state.revealed)
    {
        let hover = theme.palette.hover_overlay;
        let e = edit.clone();
        let toggle = Element::stack(
            Axis::Horizontal,
            vec![Element::image(icon).width(18.0).height(18.0)],
        )
        .padding(Insets::uniform(2.0))
        .radius(theme.radius / 2.0)
        .align(Align::Center, Align::Center)
        .hovered(move |e| e.fill(hover))
        .on_tap(cx, move |s: &mut S| e(s, &mut |p| p.toggle_revealed()))
        .a11y_label(if state.revealed {
            "Hide password"
        } else {
            "Show password"
        });
        children.push(toggle);
    }
    Element::stack(Axis::Horizontal, children)
        .gap(theme.spacing.xs)
        .fill(theme.palette.surface)
        .radius(theme.radius)
        .border(theme.palette.border, 1.0)
        .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
        .align(Align::Start, Align::Center)
        .width(200.0)
        .native_text_frame(theme)
        .cursor(crate::platform::CursorIcon::Text)
        .on_key(cx, move |s, k| {
            if keys.as_ref().is_some_and(|f| f(s, k)) {
                return;
            }
            edit(s, &mut |p| p.apply(k));
        })
        .role(Role::PasswordField)
        .a11y_label(state.label.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::geometry::Size;

    #[test]
    fn masks_and_maps_the_caret() {
        let mut pw = PasswordField::new();
        pw.apply(&KeyInput::Text("héllo".into()));
        assert_eq!(pw.shown(), "•••••");
        assert_eq!(pw.shown_marks().0, 5 * 3);
        // A click between the second and third bullets.
        pw.place(2 * 3, false);
        assert_eq!(pw.buffer.caret(), "hé".len());
        pw.place(4 * 3 + 1, true);
        assert_eq!(pw.shown_marks().1, Some((6, 12)));
        pw.set_revealed(true);
        assert_eq!(pw.shown(), "héllo");
        assert_eq!(pw.shown_marks().1, Some(("hé".len(), "héll".len())));
    }

    #[test]
    fn keeps_the_text_off_the_clipboard_and_out_of_history() {
        crate::core::set_clipboard_text("before");
        let mut pw = PasswordField::new();
        pw.apply(&KeyInput::Text("secret".into()));
        pw.apply(&KeyInput::SelectAll);
        pw.apply(&KeyInput::Copy);
        pw.apply(&KeyInput::Cut);
        assert_eq!(crate::core::clipboard_text(), "before");
        assert_eq!(pw.text(), "secret");
        pw.apply(&KeyInput::Backspace);
        assert!(pw.is_empty());
        pw.apply(&KeyInput::Undo);
        assert!(pw.is_empty(), "undo would bring the password back");
        assert!(!pw.buffer.can_undo());
        // Paste works.
        pw.apply(&KeyInput::Paste);
        assert_eq!(pw.text(), "before");
        // Word deletion takes everything before the caret.
        pw.apply(&KeyInput::DeleteWordBackward);
        assert!(pw.is_empty());
    }

    struct State {
        pw: PasswordField,
    }

    fn view(s: &State, cx: &mut Cx<State>) -> Element {
        let theme = *cx.theme();
        super::super::column(vec![password_field(cx, &theme, &s.pw, |s: &mut State| {
            &mut s.pw
        })])
        .padding(Insets::uniform(10.0))
    }

    #[test]
    fn typing_into_the_field_goes_without_an_ime_and_shows_bullets() {
        let state = State {
            pw: PasswordField::new().with_reveal_button(true),
        };
        let mut app = App::new(state, view).logical_size(Size::new(300.0, 80.0));
        app.render_once();
        assert!(app.focus_next());
        app.render_once();
        assert!(app.type_text("pa55"));
        assert_eq!(app.state().pw.text(), "pa55");
        app.render_once();
        let tree = app.accessibility_tree().unwrap();
        let node = tree
            .descendants()
            .into_iter()
            .find(|n| n.role == Role::PasswordField)
            .expect("a password field");
        assert_eq!(node.name, "Password");
        assert!(node.focused);
        // The eye, at the field's end, reveals it.
        let b = node.bounds;
        let eye = crate::geometry::Point::new(b.max_x() - 22.0, b.center().y);
        assert!(app.click_at(eye));
        assert!(app.state().pw.is_revealed());
        // Typing still lands in the field.
        assert!(app.type_text("!"));
        assert_eq!(app.state().pw.text(), "pa55!");
    }
}
