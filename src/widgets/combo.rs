//! An editable combo box: a text field that suggests entries matching what
//! is typed, in a list dropped below it.

use std::cell::Cell;

use crate::core::{Anchor, Cx, Element, KeyInput, OverlaySpec};

use super::motion::{self, Memo, Shown};
use super::popup_list::{ListScroll, PopupList, popup_list};
use super::text_input::{FieldHooks, FieldKey, TextInput, keyed_text_field, line_key};

/// The state of a [`combo_box`]: the text (a [`TextInput`]), whether the
/// suggestions show, and which one the keyboard has highlighted. Keep one in
/// your app state per combo box.
///
/// ```
/// use stipple::core::KeyInput;
/// use stipple::widgets::ComboBox;
/// let hosts = ["example.com", "example.org", "files.local"];
/// let mut host = ComboBox::new();
/// host.handle_key(&KeyInput::Text("ex".into()), &hosts); // suggests two
/// assert!(host.is_open());
/// host.handle_key(&KeyInput::Down, &hosts); // highlights "example.com"
/// host.handle_key(&KeyInput::Down, &hosts); // then "example.org"
/// host.handle_key(&KeyInput::Enter, &hosts); // takes it
/// assert_eq!(host.text(), "example.org");
/// assert!(!host.is_open());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComboBox {
    input: TextInput,
    open: bool,
    /// The suggestion highlighted, by index; `None` keeps the typed text.
    highlighted: Option<usize>,
    /// The list shows every suggestion, not only those matching the text
    /// (opened from its button, or by Down on text matching none).
    browsing: bool,
    /// When the list opened, for its entrance.
    shown: Memo<Shown>,
    /// The list just opened: it scrolls to the highlighted suggestion.
    reveal: Cell<bool>,
}

impl ComboBox {
    /// An empty combo box, its suggestions hidden.
    pub fn new() -> Self {
        Self::default()
    }

    /// A combo box holding `text`.
    pub fn from_text(text: impl Into<String>) -> Self {
        Self {
            input: TextInput::from_text(text),
            ..Self::default()
        }
    }

    /// The text: typed, or a suggestion taken.
    pub fn text(&self) -> &str {
        self.input.text()
    }

    /// Replace the text (the suggestions stay as they are).
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.input.set_text(text);
    }

    /// The text being edited, with its caret and selection.
    pub fn input(&self) -> &TextInput {
        &self.input
    }

    /// The text being edited, to change it, its length limit or the
    /// caret.
    pub fn input_mut(&mut self) -> &mut TextInput {
        &mut self.input
    }

    /// Whether the suggestions show.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The suggestion the keyboard has highlighted, by index into the
    /// suggestions; `None` while the typed text stands.
    pub fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    /// Show every suggestion (as the field's button does), none
    /// highlighted.
    pub fn open(&mut self) {
        self.open = true;
        self.browsing = true;
        self.highlighted = None;
        self.reveal.set(true);
    }

    /// Hide the suggestions, keeping the text.
    pub fn close(&mut self) {
        self.open = false;
        self.browsing = false;
        self.highlighted = None;
    }

    /// Show the suggestions if they're hidden, hide them if they show.
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    /// The suggestions the list shows, by index: those containing the text
    /// (ignoring case; the ones starting with it first), or all of them
    /// while the text is empty or the list was opened to browse.
    pub fn matches<T: AsRef<str>>(&self, suggestions: &[T]) -> Vec<usize> {
        let typed = self.input.text().to_lowercase();
        if self.browsing || typed.is_empty() {
            return (0..suggestions.len()).collect();
        }
        let lower: Vec<String> = suggestions
            .iter()
            .map(|s| s.as_ref().to_lowercase())
            .collect();
        let starts = (0..lower.len()).filter(|&i| lower[i].starts_with(&typed));
        let inside = (0..lower.len())
            .filter(|&i| !lower[i].starts_with(&typed) && lower[i].contains(&typed));
        starts.chain(inside).collect()
    }

    /// Take suggestion `index`: it becomes the text, and the list closes.
    pub fn accept<T: AsRef<str>>(&mut self, index: usize, suggestions: &[T]) {
        if let Some(s) = suggestions.get(index) {
            self.input.set_text(s.as_ref());
        }
        self.close();
    }

    /// Handle a key pressed while the field has focus, over its
    /// `suggestions`. Returns `true` if the text changed.
    ///
    /// Down opens the list (on the suggestions matching the text, or all of
    /// them) and moves the highlight down it; Up moves it up, and off the
    /// top back to the typed text. Enter (and Tab, while the list shows)
    /// takes the highlighted suggestion; with none highlighted, Enter
    /// closes the list and submits the field. Escape closes the list,
    /// keeping the text. Other keys edit the text, as in a
    /// [`text_field`](super::text_field), and show the suggestions matching
    /// it (if any).
    pub fn handle_key<T: AsRef<str>>(&mut self, key: &KeyInput, suggestions: &[T]) -> bool {
        self.key(key, suggestions) == FieldKey::Changed
    }

    /// [`handle_key`](Self::handle_key), saying whether the field submits.
    fn key<T: AsRef<str>>(&mut self, key: &KeyInput, suggestions: &[T]) -> FieldKey {
        let matches = self.matches(suggestions);
        let at = self
            .highlighted
            .and_then(|h| matches.iter().position(|&i| i == h));
        match key {
            KeyInput::Down if !self.open => {
                if matches.is_empty() {
                    self.open();
                } else {
                    self.open = true;
                    self.reveal.set(true);
                }
                let matches = self.matches(suggestions);
                self.highlighted = matches.first().copied();
                FieldKey::Kept
            }
            KeyInput::Down => {
                let next = at.map_or(0, |k| (k + 1).min(matches.len().saturating_sub(1)));
                self.highlighted = matches.get(next).copied().or(self.highlighted);
                FieldKey::Kept
            }
            KeyInput::Up if self.open => {
                self.highlighted = match at {
                    Some(0) | None => None,
                    Some(k) => matches.get(k - 1).copied(),
                };
                FieldKey::Kept
            }
            KeyInput::Enter if self.open => match self.highlighted {
                Some(i) if at.is_some() => {
                    self.accept(i, suggestions);
                    FieldKey::Changed
                }
                _ => {
                    self.close();
                    FieldKey::Submit
                }
            },
            KeyInput::Text(t) if t == "\t" && self.open => match self.highlighted {
                Some(i) if at.is_some() => {
                    self.accept(i, suggestions);
                    FieldKey::Changed
                }
                _ => {
                    self.close();
                    FieldKey::Kept
                }
            },
            KeyInput::Escape if self.open => {
                self.close();
                FieldKey::Kept
            }
            _ => {
                let edited = line_key(&mut self.input, key);
                if edited == FieldKey::Changed {
                    // Typing shows what matches it; the typed text stands.
                    self.browsing = false;
                    self.highlighted = None;
                    self.open = !self.input.is_empty() && !self.matches(suggestions).is_empty();
                }
                edited
            }
        }
    }
}

/// An editable combo box: a text field editing the text of `state` (the
/// [`ComboBox`] that `field` returns), with a list of the `suggestions`
/// containing the text dropped below it as the user types (the ones
/// starting with it first), and a button at its end showing them all (a
/// history of hosts, a font family out of [`Font::families`]). Up and Down
/// move through the list, Enter or Tab takes the highlighted suggestion, a
/// tap takes one, and Escape (or a press outside) closes the list; the
/// typed text stays unless a suggestion is taken (see
/// [`ComboBox::handle_key`]). A long list builds only the rows in view and
/// keeps the highlighted one in sight.
///
/// It edits as a [`text_field`](super::text_field) does, and sizes as one
/// (200 logical px, stretched by a column); [`on_change`](Element::on_change)
/// runs after the text changes, by typing or by taking a suggestion, and
/// [`on_submit`](Element::on_submit) on Enter with no suggestion
/// highlighted. Assistive technologies see a text field, and the list as a
/// [`Role::List`](crate::core::Role::List) while it shows. Under the native
/// theme, where the OS's own text field edits the text, the button isn't
/// shown (Down shows every suggestion).
///
/// [`Font::families`]: crate::render::Font::families
///
/// ```
/// use stipple::prelude::*;
/// struct Connect { host: ComboBox, history: Vec<String> }
/// fn view(state: &Connect, cx: &mut Cx<Connect>) -> Element {
///     column(vec![
///         label(cx, "Host"),
///         combo_box(cx, &state.host, &state.history, |s| &mut s.host)
///             .placeholder("user@host")
///             .on_submit(cx, |s| {
///                 let host = s.host.text().to_string();
///                 s.history.retain(|h| *h != host);
///                 s.history.insert(0, host);
///             }),
///     ])
/// }
/// # let theme = Theme::light();
/// # let state = Connect { host: ComboBox::new(), history: vec!["example.com".into()] };
/// # let _ = view(&state, &mut Cx::new(&theme));
/// ```
pub fn combo_box<S: 'static, T: AsRef<str>>(
    cx: &mut Cx<S>,
    state: &ComboBox,
    suggestions: &[T],
    field: impl Fn(&mut S) -> &mut ComboBox + Copy + 'static,
) -> Element {
    let theme = super::theme_of(cx);
    let p = &theme.palette;
    let labels: Vec<String> = suggestions.iter().map(|o| o.as_ref().to_string()).collect();
    // The field's change handlers, which taking a suggestion runs too.
    let hooks = FieldHooks::new();
    let matches = if state.open {
        state.matches(&labels)
    } else {
        Vec::new()
    };
    let open = state.open && !matches.is_empty();
    let scroll = ListScroll::register(cx, &theme, matches.len());
    let row_of = |h: Option<usize>| h.and_then(|h| matches.iter().position(|&i| i == h));
    let opened = state.shown.with(|s| s.track(open, cx.frame_time()));
    let list = if open {
        if state.reveal.take() {
            scroll.open_at(cx, row_of(state.highlighted));
        }
        let t = motion::progress(cx, theme.motion.popup, opened);
        let shown: Vec<&str> = matches.iter().map(|&i| labels[i].as_str()).collect();
        let list = PopupList {
            labels: &shown,
            highlighted: row_of(state.highlighted),
            checked: None,
            header: None,
        };
        let list = popup_list(cx, &theme, &scroll, list, |row| {
            let (i, labels, hooks) = (matches[row], labels.clone(), hooks.clone());
            move |s: &mut S| {
                field(s).accept(i, &labels);
                hooks.changed(s);
            }
        });
        let list = motion::popup(list, &theme.motion, t, (0.5, 0.0))
            .exit(motion::popup_exit(&theme.motion, (0.5, 0.0)));
        let dismiss = cx.register(move |s| field(s).close());
        Some((list, dismiss))
    } else {
        None
    };

    let keys = {
        let (labels, scroll) = (labels.clone(), scroll.clone());
        move |s: &mut S, k: &KeyInput| {
            let combo = field(s);
            let did = combo.key(k, &labels);
            let matches = combo.matches(&labels);
            scroll.reveal(
                combo
                    .highlighted
                    .and_then(|h| matches.iter().position(|&i| i == h)),
            );
            did
        }
    };
    let mut el = keyed_text_field(cx, &state.input, move |s| &mut field(s).input, hooks, keys)
        .autocorrect(false)
        // Tab takes the highlighted suggestion while the list shows.
        .captures_tab(open);
    // The button at its end: every suggestion, whatever the text.
    if super::field_deco(&theme).is_none()
        && !labels.is_empty()
        && let Some(frame) = el.children_mut()
    {
        let chevron = Element::text("\u{25BE}", theme.typography.body, p.text_muted)
            .width((theme.typography.body * 1.25).round())
            .min_height(theme.typography.body)
            .align_self(crate::core::Align::Stretch)
            .on_tap(cx, move |s| field(s).toggle());
        frame.push(chevron);
    }
    if let (Some((content, dismiss)), Some(id)) = (list, el.focus) {
        cx.overlay(OverlaySpec {
            content,
            anchor: Anchor::BelowFocus(id),
            modal: false,
            dismiss: Some(dismiss),
        });
    }
    el
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::core::Role;
    use crate::geometry::{Point, Size};
    use crate::widgets::column;

    const HOSTS: [&str; 5] = [
        "example.com",
        "files.example.org",
        "example.org",
        "build-01",
        "build-02",
    ];

    #[derive(Default)]
    struct Connect {
        host: ComboBox,
        changes: u32,
        submitted: Vec<String>,
    }

    fn app() -> App<Connect> {
        App::new(Connect::default(), |s: &Connect, cx: &mut Cx<Connect>| {
            column(vec![
                combo_box(cx, &s.host, &HOSTS, |s| &mut s.host)
                    .on_change(cx, |s| s.changes += 1)
                    .on_submit(cx, |s| {
                        let host = s.host.text().to_string();
                        s.submitted.push(host);
                    }),
            ])
        })
        .no_font()
        .logical_size(Size::new(300.0, 400.0))
    }

    fn shown(app: &mut App<Connect>) -> Vec<String> {
        app.render_once();
        app.accessibility_tree()
            .unwrap()
            .descendants()
            .into_iter()
            .filter(|n| n.role == Role::ListItem)
            .map(|n| n.name.clone())
            .collect()
    }

    fn focus_field(app: &mut App<Connect>) {
        let field = app.find(Role::TextField, "").expect("the field");
        app.click_at(field.bounds.center());
    }

    #[test]
    fn typing_suggests_matches_starting_with_it_first() {
        let mut app = app();
        focus_field(&mut app);
        assert!(shown(&mut app).is_empty(), "nothing typed, nothing shown");
        app.type_text("example.");
        assert!(app.state().host.is_open());
        assert_eq!(
            shown(&mut app),
            vec!["example.com", "example.org", "files.example.org"]
        );
        // The list drops below the field, at least as wide.
        let field = app.find(Role::TextField, "example.").unwrap();
        let list = app.find(Role::List, "").unwrap();
        assert!(list.bounds.min_y() >= field.bounds.max_y());
        assert!(list.bounds.width() >= field.bounds.width());
        // Text matching nothing closes it, and the text stays.
        app.type_text("zzz");
        assert!(shown(&mut app).is_empty());
        assert_eq!(app.state().host.text(), "example.zzz");
    }

    #[test]
    fn arrows_and_enter_take_a_suggestion_and_up_returns_to_the_text() {
        let mut app = app();
        focus_field(&mut app);
        app.type_text("build");
        let typed = app.state().changes;
        app.press_key(KeyInput::Down);
        app.press_key(KeyInput::Down);
        assert_eq!(app.state().host.highlighted(), Some(4));
        app.press_key(KeyInput::Up);
        app.press_key(KeyInput::Up);
        assert_eq!(
            app.state().host.highlighted(),
            None,
            "back to the typed text"
        );
        app.press_key(KeyInput::Down);
        assert!(app.press_key(KeyInput::Enter));
        assert_eq!(app.state().host.text(), "build-01");
        assert!(!app.state().host.is_open());
        assert_eq!(app.state().changes, typed + 1, "taking it is a change");
        assert!(app.state().submitted.is_empty(), "and not a submit");
        // Enter with nothing highlighted submits the text.
        app.press_key(KeyInput::Enter);
        assert_eq!(app.state().submitted, vec!["build-01"]);
    }

    #[test]
    fn escape_closes_and_keeps_the_free_text() {
        let mut app = app();
        focus_field(&mut app);
        app.type_text("ex");
        app.press_key(KeyInput::Down);
        app.press_key(KeyInput::Escape);
        assert!(!app.state().host.is_open());
        assert_eq!(app.state().host.text(), "ex");
        // Tab takes the highlighted one while the list shows.
        app.press_key(KeyInput::Down);
        app.press_key(KeyInput::Down);
        app.render_once();
        app.press_key_code(crate::platform::KeyCode::Tab, Default::default());
        assert_eq!(app.state().host.text(), "example.org");
    }

    #[test]
    fn a_tap_takes_a_suggestion_and_the_button_shows_them_all() {
        let mut app = app();
        focus_field(&mut app);
        app.type_text("files");
        assert!(app.click(Role::ListItem, "files.example.org"));
        assert_eq!(app.state().host.text(), "files.example.org");
        assert_eq!(app.state().changes, 2, "the typing, then the pick");
        // The button at the field's end lists every host.
        let field = app.find(Role::TextField, "files.example.org").unwrap();
        let chevron = Point::new(field.bounds.max_x() - 20.0, field.bounds.center().y);
        assert!(app.click_at(chevron));
        assert_eq!(shown(&mut app).len(), HOSTS.len());
        // Down when closed on text matching nothing browses them all too.
        app.click_at(chevron);
        assert!(!app.state().host.is_open());
        app.type_text("!");
        app.press_key(KeyInput::Down);
        assert_eq!(shown(&mut app).len(), HOSTS.len());
        assert_eq!(app.state().host.highlighted(), Some(0));
    }
}
