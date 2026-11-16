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

impl Element {
    /// The kind of text this field takes, which picks the on-screen keyboard
    /// (a number pad, a phone pad, `@` for an e-mail address) and its
    /// defaults: only plain text is capitalized and corrected. On a
    /// [`text_field`], [`text_area`] or
    /// [`password_field`](super::password_field) (a number one is a PIN pad).
    /// iOS, Android and the web honor it; desktops, with no on-screen
    /// keyboard, ignore it, and the field takes any text everywhere (a hint,
    /// not a filter).
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct Scan { code: TextInput }
    /// fn view(state: &Scan, cx: &mut Cx<Scan>) -> Element {
    ///     text_field(cx, &state.code, |s| &mut s.code)
    ///         .placeholder("EAN-13")
    ///         .input_kind(InputKind::Number)
    ///         .enter_action(EnterAction::Done)
    ///         .on_submit(cx, |s| println!("{}", s.code.text()))
    /// }
    /// # let theme = Theme::light();
    /// # let _ = view(&Scan { code: TextInput::new() }, &mut Cx::new(&theme));
    /// ```
    pub fn input_kind(mut self, kind: crate::platform::InputKind) -> Self {
        self.input_hints().kind = kind;
        self
    }

    /// Which letters the on-screen keyboard capitalizes on its own. Default:
    /// sentences for [`InputKind::Text`](crate::platform::InputKind::Text),
    /// none for the other kinds and for passwords. iOS, Android and the web.
    pub fn autocapitalize(mut self, mode: crate::platform::Autocapitalize) -> Self {
        self.input_hints().autocapitalize = Some(mode);
        self
    }

    /// Whether the on-screen keyboard corrects words and offers suggestions
    /// (`false` keeps a code or a name exactly as typed). Default: on for
    /// plain text and search terms, off for the other kinds; never on a
    /// password. iOS, Android and the web.
    pub fn autocorrect(mut self, on: bool) -> Self {
        self.input_hints().autocorrect = Some(on);
        self
    }

    /// What the on-screen keyboard's action key says: Done, Go, Next, Search
    /// or Send. Each submits the field (runs its
    /// [`on_submit`](Element::on_submit) handlers), as Enter does;
    /// [`Next`](crate::platform::EnterAction::Next) then moves focus to the
    /// next field, on every platform (Enter on a desktop too). Default: the
    /// platform's own (Done on Android, Return on iOS). A [`text_area`]'s
    /// Enter stays a new line: there only `Next` takes effect, moving focus
    /// on without one.
    pub fn enter_action(mut self, action: crate::platform::EnterAction) -> Self {
        self.input_hints().action = Some(action);
        self
    }

    /// What the field holds, for the system's autofill to offer: a saved
    /// password, a one-time code from a text message, an address. iOS
    /// (`textContentType`) and the web (`autocomplete`).
    pub fn autofill(mut self, content: crate::platform::Autofill) -> Self {
        self.input_hints().autofill = Some(content);
        self
    }

    /// This element's keyboard hints, made on first use.
    pub(crate) fn input_hints(&mut self) -> &mut crate::platform::InputHints {
        self.semantics
            .get_or_insert_with(Default::default)
            .input
            .get_or_insert_with(Default::default)
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
    let (mut el, _) = field_keys(cx, el, false, move |s, k| field(s).apply(k));
    el.input_hints().multiline = true;
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

    #[derive(Default)]
    struct Form {
        first: TextInput,
        second: TextInput,
        notes: TextInput,
        submitted: u32,
    }

    #[test]
    fn hints_reach_the_focused_field_and_next_moves_on() {
        use crate::platform::{Autocapitalize, Autofill, EnterAction, InputKind};
        let mut app = App::new(Form::default(), |s: &Form, cx: &mut Cx<Form>| {
            crate::widgets::column(vec![
                text_field(cx, &s.first, |s| &mut s.first)
                    .input_kind(InputKind::Email)
                    .autocapitalize(Autocapitalize::Off)
                    .autocorrect(false)
                    .autofill(Autofill::Email)
                    .enter_action(EnterAction::Next)
                    .on_submit(cx, |s| s.submitted += 1),
                text_area(cx, &s.notes, |s| &mut s.notes).enter_action(EnterAction::Next),
                text_field(cx, &s.second, |s| &mut s.second),
            ])
        })
        .no_font()
        .logical_size(Size::new(300.0, 400.0));
        app.render_once();
        assert!(app.focus_next());
        let hints = |app: &App<Form>| {
            let pane = &app.panes[0];
            crate::core::input_hints(pane.tree.as_ref().unwrap(), pane.focused)
        };
        let h = hints(&app).expect("a text field has focus");
        assert_eq!(h.kind, InputKind::Email);
        assert_eq!(h.autocapitalize, Some(Autocapitalize::Off));
        assert_eq!(h.autocorrect, Some(false));
        assert_eq!(h.autofill, Some(Autofill::Email));
        assert!(!h.multiline && !h.secure);
        // Next: Enter submits, then focus moves to the text area.
        app.press_key(KeyInput::Enter);
        assert_eq!(app.state().submitted, 1);
        let h = hints(&app).expect("the text area has focus");
        assert!(h.multiline);
        // There, Next moves on without a new line.
        app.press_key(KeyInput::Enter);
        assert_eq!(app.state().notes.text(), "");
        let h = hints(&app).expect("the last field has focus");
        assert_eq!(h, crate::platform::InputHints::default());
    }

    #[test]
    fn the_keyboard_shrinks_the_layout_and_the_focused_field_scrolls_into_view() {
        let mut app = App::new(Form::default(), |s: &Form, cx: &mut Cx<Form>| {
            let fields = vec![
                crate::widgets::spacer().height(500.0),
                text_field(cx, &s.first, |s| &mut s.first),
                crate::widgets::spacer().height(40.0),
            ];
            crate::widgets::column(vec![crate::widgets::scroll(cx).children(fields)])
        })
        .no_font()
        .logical_size(Size::new(300.0, 400.0));
        app.render_once();
        fn field(n: &LayoutNode) -> Option<&LayoutNode> {
            if n.focus.is_some() {
                return Some(n);
            }
            n.children.iter().find_map(field)
        }
        let bounds = |app: &App<Form>| field(app.panes[0].tree.as_ref().unwrap()).unwrap().bounds;
        assert!(bounds(&app).min_y() > 400.0, "below the window to start");
        // Focusing it scrolls it into view.
        assert!(app.focus_next());
        app.render_once();
        let b = bounds(&app);
        assert!(b.max_y() <= 400.0 && b.min_y() >= 0.0, "{b:?}");
        // The keyboard covers the bottom 250: the view is laid out in the 150
        // above it, and the field scrolled up into that.
        app.set_keyboard_inset(250.0);
        app.render_once();
        let b = bounds(&app);
        assert!(b.max_y() <= 150.0 && b.min_y() >= 0.0, "{b:?}");
        let root = app.panes[0].tree.as_ref().unwrap();
        assert_eq!(root.bounds.height(), 150.0);
        // Scrolled away by the user, it stays away until the caret moves.
        let id = *app.panes[0].scroll_offsets.keys().next().unwrap();
        app.panes[0].scroll_offsets.insert(id, 0.0);
        app.panes[0].dirty = true;
        app.render_once();
        assert!(bounds(&app).min_y() > 150.0);
        app.type_text("x");
        app.render_once();
        let b = bounds(&app);
        assert!(b.max_y() <= 150.0, "typing brings it back: {b:?}");
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
