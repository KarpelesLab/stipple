//! Text input: the [`TextInput`] state, and the single-line [`text_field`]
//! and multi-line [`text_area`] that edit it.

use std::cell::RefCell;
use std::rc::Rc;

use crate::core::{Align, Axis, Cx, Element, FocusId, KeyInput};
use crate::geometry::Insets;
use crate::style::Theme;

use super::{EditBuffer, EditKind, field_text};

/// The state of a [`text_field`] or [`text_area`]: an [`EditBuffer`] (the
/// text, caret, selection, undo history and input-method composition), and
/// how the field may be edited. Keep one in your app state per field.
///
/// ```
/// use stipple::core::KeyInput;
/// use stipple::widgets::TextInput;
/// let mut name = TextInput::new().max_length(5);
/// name.apply(&KeyInput::Text("Stipple".into()));
/// assert_eq!(name.text(), "Stipp"); // cut at five characters
/// assert_eq!(name.take(), "Stipp");
/// assert!(name.is_empty());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextInput {
    buffer: EditBuffer,
    max_length: Option<usize>,
    read_only: bool,
}

impl TextInput {
    /// An empty, editable input with no length limit.
    pub fn new() -> Self {
        Self::default()
    }

    /// An input holding `text`, the caret at its end.
    pub fn from_text(text: impl Into<String>) -> Self {
        Self {
            buffer: EditBuffer::from_text(text),
            ..Self::default()
        }
    }

    /// Accept at most `chars` characters: typing and pasting past it are cut
    /// short. Text already longer than that is left as it is.
    pub fn max_length(mut self, chars: usize) -> Self {
        self.max_length = Some(chars);
        self
    }

    /// Make the input read-only (`true`): its text can be selected and copied,
    /// but not changed by the user. [`set_text`](Self::set_text) still works.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Whether the user can't change the text.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Make the input read-only (`true`) or editable.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// The text.
    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// Whether the text is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Replace the text, the caret at its end, and forget the undo history.
    pub fn set_text(&mut self, text: impl Into<String>) {
        let direction = self.buffer.direction();
        self.buffer = EditBuffer::from_text(text);
        self.buffer.set_direction(direction);
    }

    /// Empty the input (forgetting the undo history).
    pub fn clear(&mut self) {
        self.set_text(String::new());
    }

    /// Take the text out, leaving the input empty: what a field's
    /// [`on_submit`](Element::on_submit) handler usually does.
    pub fn take(&mut self) -> String {
        let text = self.buffer.text().to_string();
        self.clear();
        text
    }

    /// The edit buffer: the caret, selection, direction and composition.
    pub fn buffer(&self) -> &EditBuffer {
        &self.buffer
    }

    /// The edit buffer, to move the caret, select, undo, or set the text
    /// direction. Edits made through it skip the length limit and read-only.
    pub fn buffer_mut(&mut self) -> &mut EditBuffer {
        &mut self.buffer
    }

    /// Apply a key as the field does while focused: like
    /// [`EditBuffer::apply`], but within the length limit, and only moving
    /// and copying while read-only. Returns whether the text changed.
    pub fn apply(&mut self, input: &KeyInput) -> bool {
        let revision = self.buffer.revision;
        if self.read_only {
            match input {
                KeyInput::Text(_)
                | KeyInput::Preedit { .. }
                | KeyInput::DeleteSurrounding { .. }
                | KeyInput::Backspace
                | KeyInput::Delete
                | KeyInput::DeleteWordBackward
                | KeyInput::DeleteWordForward
                | KeyInput::Undo
                | KeyInput::Redo
                | KeyInput::Paste
                | KeyInput::Enter => return false,
                // Cutting copies.
                KeyInput::Cut => self.buffer.apply(&KeyInput::Copy),
                other => self.buffer.apply(other),
            }
            return false;
        }
        match (input, self.max_length) {
            (KeyInput::Text(t), Some(_)) => {
                let t = self.fit(t);
                self.buffer.commit_text(&t);
            }
            (KeyInput::Paste, Some(_)) => {
                let t = self.fit(&crate::core::clipboard_text());
                if !t.is_empty() {
                    self.buffer.insert_as(&t, EditKind::Other);
                }
            }
            (KeyInput::Enter, Some(_)) => {
                if !self.fit("\n").is_empty() {
                    self.buffer.apply(input);
                }
            }
            _ => self.buffer.apply(input),
        }
        self.buffer.revision != revision
    }

    /// As much of `text` as fits in the length limit, in place of the
    /// selection.
    fn fit(&self, text: &str) -> String {
        let Some(max) = self.max_length else {
            return text.to_string();
        };
        let b = &self.buffer;
        let selected = b
            .selection()
            .map_or(0, |(s, e)| b.text()[s..e].chars().count());
        let room = max.saturating_sub(b.text().chars().count() - selected);
        text.chars().take(room).collect()
    }
}

/// What a field runs besides editing: [`Element::on_submit`] and
/// [`Element::on_change`] handlers, attached after the field is built.
pub(crate) struct FieldHooks<S> {
    submit: Handlers<S>,
    change: Handlers<S>,
}

/// A field's handlers of one kind.
type Handlers<S> = RefCell<Vec<Box<dyn FnMut(&mut S)>>>;

impl<S> FieldHooks<S> {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            submit: RefCell::new(Vec::new()),
            change: RefCell::new(Vec::new()),
        })
    }

    /// Run the submit handlers.
    pub(crate) fn submit(&self, state: &mut S) {
        for handler in self.submit.borrow_mut().iter_mut() {
            handler(state);
        }
    }

    /// Run the change handlers.
    pub(crate) fn changed(&self, state: &mut S) {
        for handler in self.change.borrow_mut().iter_mut() {
            handler(state);
        }
    }
}

impl Element {
    /// Run `handler` when Enter is pressed in this field: a [`text_field`],
    /// or a [`password_field`](super::password_field). A [`text_area`]
    /// takes Enter as a new line instead and never submits. On any other
    /// element this does nothing.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct Todo { draft: TextInput, items: Vec<String> }
    /// fn view(state: &Todo, cx: &mut Cx<Todo>) -> Element {
    ///     text_field(cx, &state.draft, |s| &mut s.draft)
    ///         .placeholder("What needs doing?")
    ///         .on_submit(cx, |s| {
    ///             let item = s.draft.take();
    ///             s.items.push(item);
    ///         })
    /// }
    /// # let theme = Theme::light();
    /// # let _ = view(&Todo { draft: TextInput::new(), items: vec![] }, &mut Cx::new(&theme));
    /// ```
    pub fn on_submit<S>(self, cx: &mut Cx<'_, S>, handler: impl FnMut(&mut S) + 'static) -> Self {
        if let Some(hooks) = self.focus.and_then(|id| cx.field_hooks(id)) {
            hooks.submit.borrow_mut().push(Box::new(handler));
        }
        self
    }

    /// Run `handler` after the user changes this field's text: a
    /// [`text_field`], [`text_area`] or
    /// [`password_field`](super::password_field). It runs after the edit, so
    /// the state already holds the new text. On any other element this does
    /// nothing.
    pub fn on_change<S>(self, cx: &mut Cx<'_, S>, handler: impl FnMut(&mut S) + 'static) -> Self {
        if let Some(hooks) = self.focus.and_then(|id| cx.field_hooks(id)) {
            hooks.change.borrow_mut().push(Box::new(handler));
        }
        self
    }
}

/// Register a field's key handler: `edit` applies a key to the field's state
/// and says whether the text changed; Enter submits instead when `submits`.
pub(crate) fn field_keys<S: 'static>(
    cx: &mut Cx<S>,
    el: Element,
    submits: bool,
    mut edit: impl FnMut(&mut S, &KeyInput) -> bool + 'static,
) -> (Element, FocusId) {
    let hooks = FieldHooks::new();
    let h = hooks.clone();
    let el = el.on_key(cx, move |s, k| {
        if submits && matches!(k, KeyInput::Enter) {
            h.submit(s);
        } else if edit(s, k) {
            h.changed(s);
        }
    });
    let id = el.focus.expect("on_key sets it");
    cx.set_field_hooks(id, hooks);
    (el, id)
}

/// The text leaf of a field for `buffer`: its text with the caret,
/// selection and composition, filling the field (so right-to-left lines can
/// sit at its right edge) and shrinking to it, so text (or a placeholder)
/// longer than the field doesn't widen it; the frame clips what overflows.
fn field_leaf(buffer: &EditBuffer, theme: &Theme) -> Element {
    let shown = buffer.display();
    field_text(shown.text, theme, theme.palette.text)
        .direction(buffer.direction())
        .grow(1.0)
        .shrink(1.0)
        .caret(shown.caret)
        .selection(shown.selection)
        .preedit(shown.preedit)
}

/// The frame around a field's text, clipping it.
fn field_frame(theme: &Theme, leaf: Element) -> Element {
    Element::stack(Axis::Horizontal, vec![leaf])
        .clip()
        .fill(theme.palette.surface)
        .radius(theme.radius)
        .border(theme.palette.border, 1.0)
        .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
}

/// Wire a click to place the caret (and a drag or shift-click to select) in
/// the field `field` returns.
fn field_pointer<S>(
    cx: &mut Cx<S>,
    el: Element,
    field: impl Fn(&mut S) -> &mut TextInput + Copy + 'static,
) -> Element {
    el.on_text_pos(cx, move |s, index, extend| {
        let b = &mut field(s).buffer;
        let index = b.display_to_text(index);
        if extend {
            b.extend_to(index);
        } else {
            b.place_caret(index);
        }
    })
}

/// A single-line text field editing `input`, the [`TextInput`] that `field`
/// returns from the app state: typing, the caret and selection (by keyboard
/// and pointer), undo and redo, the clipboard, input methods and
/// right-to-left text. Enter runs its [`on_submit`](Element::on_submit)
/// handlers, and an edit its [`on_change`](Element::on_change) ones; give it
/// a hint with [`placeholder`](Element::placeholder). Default width 200
/// logical px; override with `.width(..)`.
///
/// `field` is an accessor, `|s| &mut s.name`; the state type comes from `cx`,
/// so it needs no annotation.
///
/// ```
/// use stipple::prelude::*;
/// struct App { name: TextInput }
/// fn view(state: &App, cx: &mut Cx<App>) -> Element {
///     column(vec![
///         label(cx, "Name"),
///         text_field(cx, &state.name, |s| &mut s.name).placeholder("Your name"),
///     ])
/// }
/// # let theme = Theme::light();
/// # let _ = view(&App { name: TextInput::new() }, &mut Cx::new(&theme));
/// ```
pub fn text_field<S: 'static>(
    cx: &mut Cx<S>,
    input: &TextInput,
    field: impl Fn(&mut S) -> &mut TextInput + Copy + 'static,
) -> Element {
    let theme = *cx.theme();
    let el = field_frame(&theme, field_leaf(&input.buffer, &theme))
        .default_width(200.0)
        .native_text_frame(&theme);
    let (el, _) = field_keys(cx, el, true, move |s, k| match k {
        // Single-line: a triple-click selects the whole field.
        KeyInput::SelectLine => {
            field(s).buffer.select_all();
            false
        }
        _ => field(s).apply(k),
    });
    field_pointer(cx, el, field)
}

/// A multi-line text area editing `input`, the [`TextInput`] that `field`
/// returns: the same editing as [`text_field`], but top-aligned so wrapped
/// and `\n`-separated lines stack from the top. Enter inserts a new line, and
/// Up/Down move between lines. Default size 280×120 logical px; override with
/// `.width(..)`/`.height(..)`.
pub fn text_area<S: 'static>(
    cx: &mut Cx<S>,
    input: &TextInput,
    field: impl Fn(&mut S) -> &mut TextInput + Copy + 'static,
) -> Element {
    let theme = *cx.theme();
    let el = field_frame(&theme, field_leaf(&input.buffer, &theme))
        .align(Align::Start, Align::Start)
        .default_width(280.0)
        .height(120.0)
        .native_text_frame(&theme);
    let (el, _) = field_keys(cx, el, false, move |s, k| field(s).apply(k));
    field_pointer(cx, el, field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::core::LayoutNode;
    use crate::geometry::Size;

    #[derive(Default)]
    struct Todo {
        draft: TextInput,
        items: Vec<String>,
        edits: u32,
    }

    fn app() -> App<Todo> {
        App::new(Todo::default(), |s: &Todo, cx: &mut Cx<Todo>| {
            crate::widgets::column(vec![
                text_field(cx, &s.draft, |s| &mut s.draft)
                    .on_submit(cx, |s| {
                        let item = s.draft.take();
                        s.items.push(item);
                    })
                    .on_change(cx, |s| s.edits += 1),
            ])
        })
        .no_font()
        .logical_size(Size::new(300.0, 100.0))
    }

    #[test]
    fn enter_submits_and_edits_report_changes() {
        let mut app = app();
        app.render_once();
        assert!(app.focus_next());
        app.type_text("milk");
        assert_eq!(app.state().draft.text(), "milk");
        let edits = app.state().edits;
        assert!(edits >= 1, "typing changed the text");
        // A caret move changes nothing.
        app.press_key(KeyInput::Left);
        assert_eq!(app.state().edits, edits);
        app.press_key(KeyInput::Enter);
        assert_eq!(app.state().items, ["milk"]);
        assert_eq!(
            app.state().draft.text(),
            "",
            "a single-line field takes no newline"
        );
    }

    #[test]
    fn max_length_cuts_typing_and_pasting() {
        let mut t = TextInput::new().max_length(4);
        t.apply(&KeyInput::Text("ab".into()));
        t.apply(&KeyInput::Text("cdef".into()));
        assert_eq!(t.text(), "abcd");
        assert!(
            !t.apply(&KeyInput::Text("x".into())),
            "full: nothing changes"
        );
        // Typing over a selection makes room.
        t.apply(&KeyInput::SelectAll);
        t.apply(&KeyInput::Text("éèêëz".into()));
        assert_eq!(t.text(), "éèêë", "counts characters, not bytes");
        crate::core::set_clipboard_text("123456");
        t.apply(&KeyInput::SelectAll);
        t.apply(&KeyInput::Paste);
        assert_eq!(t.text(), "1234");
    }

    #[test]
    fn read_only_selects_and_copies_but_never_edits() {
        let mut t = TextInput::from_text("fixed").read_only(true);
        for k in [
            KeyInput::Text("x".into()),
            KeyInput::Backspace,
            KeyInput::Paste,
            KeyInput::Enter,
        ] {
            assert!(!t.apply(&k));
        }
        t.apply(&KeyInput::SelectAll);
        t.apply(&KeyInput::Cut);
        assert_eq!(t.text(), "fixed", "cutting only copies");
        assert_eq!(crate::core::clipboard_text(), "fixed");
        assert_eq!(t.buffer().selection(), Some((0, 5)));
    }

    #[test]
    fn a_long_placeholder_is_clipped_to_the_field() {
        let mut app = App::new(Todo::default(), |s: &Todo, cx: &mut Cx<Todo>| {
            crate::widgets::column(vec![
                text_field(cx, &s.draft, |s| &mut s.draft)
                    .placeholder("A hint far longer than the field could ever show in one line"),
            ])
            .align(Align::Start, Align::Start)
        })
        .logical_size(Size::new(600.0, 100.0));
        if let Some(font) = crate::render::Font::system_default() {
            app = app.font(font);
        }
        app.render_once();
        fn field(n: &LayoutNode) -> Option<&LayoutNode> {
            if n.focus.is_some() {
                return Some(n);
            }
            n.children.iter().find_map(field)
        }
        let tree = app.panes[0].tree.as_ref().unwrap();
        let f = field(tree).unwrap();
        assert_eq!(f.bounds.size.width, 200.0, "keeps its width");
        assert!(f.clip, "and clips the hint");
    }

    #[test]
    fn themed_scopes_swap_the_theme_widgets_read() {
        let light = Theme::light();
        let mut cx = Cx::<()>::new(&light);
        let dark = Theme::dark();
        let fill = |el: &Element| el.decoration.fill;
        let inner = cx.themed(dark, |cx| crate::widgets::divider(cx));
        let outer = crate::widgets::divider(&cx);
        assert_eq!(fill(&inner), Some(dark.palette.border));
        assert_eq!(fill(&outer), Some(light.palette.border));
        assert_eq!(*cx.theme(), light, "back after the scope");
    }
}
