//! A select (dropdown): a button showing the chosen option, which opens a list
//! of the options below it, with a field to search them by if it's
//! [searchable](Select::searchable).

use std::cell::Cell;

use crate::core::{Anchor, Axis, Cx, Element, KeyInput, OverlaySpec, Role};
use crate::geometry::Insets;
use crate::platform::native::NativeControl;

use super::motion::{self, Memo, Shown};
use super::native_size;
use super::popup_list::{ListScroll, PopupList, popup_list};
use super::text_input::{FieldHooks, FieldKey, TextInput, keyed_text_field, line_key};

/// The state of a [`select`]: which option is chosen, whether the list is open,
/// and which option the keyboard has highlighted in it (and, in a
/// [searchable](Self::searchable) one, the text searched for). Keep one in
/// your app state per select.
///
/// ```
/// use stipple::core::KeyInput;
/// use stipple::widgets::Select;
/// let options = ["Small", "Medium", "Large"];
/// let mut size = Select::new(Some(0));
/// size.handle_key(&KeyInput::Down, &options); // opens the list
/// size.handle_key(&KeyInput::Down, &options); // highlights "Medium"
/// size.handle_key(&KeyInput::Enter, &options); // chooses it
/// assert_eq!(size.selected(), Some(1));
/// assert!(!size.is_open());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Select {
    selected: Option<usize>,
    open: bool,
    highlighted: usize,
    /// When the list opened, for its entrance.
    shown: Memo<Shown>,
    /// A field at the top of the list filters it.
    searchable: bool,
    /// What that field holds.
    query: TextInput,
    /// The list just opened: it scrolls to the highlighted option.
    reveal: Cell<bool>,
}

impl Select {
    /// A closed select with `selected` chosen (`None` shows a dash).
    pub fn new(selected: Option<usize>) -> Self {
        Self {
            selected,
            highlighted: selected.unwrap_or(0),
            ..Self::default()
        }
    }

    /// Put a search field at the top of the list (`true`), for a long list
    /// (a font family out of hundreds). It takes the focus as the list
    /// opens, and gives it back to the select when the list closes; typing
    /// there (or on the select) shows only the options containing the text
    /// (ignoring case), and Up, Down and Enter pick among them.
    pub fn searchable(mut self, searchable: bool) -> Self {
        self.searchable = searchable;
        self
    }

    /// Whether the list has a search field.
    pub fn is_searchable(&self) -> bool {
        self.searchable
    }

    /// The text searched for (empty while the list is closed).
    pub fn query(&self) -> &str {
        self.query.text()
    }

    /// The chosen option's index.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Choose `index` (or nothing) without opening or closing the list.
    pub fn set_selected(&mut self, index: Option<usize>) {
        self.selected = index;
    }

    /// Whether the option list is showing.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The option the keyboard has highlighted in the open list.
    pub fn highlighted(&self) -> usize {
        self.highlighted
    }

    /// Show the list, highlighting the chosen option.
    pub fn open(&mut self) {
        self.open = true;
        self.highlighted = self.selected.unwrap_or(0);
        self.query.clear();
        self.reveal.set(true);
    }

    /// Hide the list, keeping the choice.
    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
    }

    /// Open the list if it's closed, close it if it's open.
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    /// Choose `index` and close the list.
    pub fn choose(&mut self, index: usize) {
        self.selected = Some(index);
        self.highlighted = index;
        self.close();
    }

    /// The options the list shows, by index: those containing the search
    /// text (all of them, with none).
    fn shown<T: AsRef<str>>(&self, options: &[T]) -> Vec<usize> {
        let query = self.query.text().to_lowercase();
        if !self.searchable || query.is_empty() {
            return (0..options.len()).collect();
        }
        (0..options.len())
            .filter(|&i| options[i].as_ref().to_lowercase().contains(&query))
            .collect()
    }

    /// The search text changed: highlight the first option still shown,
    /// unless the highlighted one is.
    fn refilter<T: AsRef<str>>(&mut self, options: &[T]) {
        let shown = self.shown(options);
        if !shown.contains(&self.highlighted)
            && let Some(&first) = shown.first()
        {
            self.highlighted = first;
        }
    }

    /// Where the highlighted option is in the list shown, if it shows.
    fn highlighted_row<T: AsRef<str>>(&self, options: &[T]) -> Option<usize> {
        self.shown(options)
            .iter()
            .position(|&i| i == self.highlighted)
    }

    /// Handle a key pressed while the select has focus, over its `options`.
    /// Returns `true` if the chosen option changed.
    ///
    /// Closed: Down, Up, Enter and Space open the list; Home and End choose
    /// the first and last option; typing a letter chooses the next option
    /// starting with it (in a [searchable](Self::searchable) select, it
    /// opens the list searching for it). Open: Up and Down (and Home and
    /// End) move the highlight, Enter or Space chooses it, Escape closes the
    /// list, and a letter highlights the next option starting with it; in a
    /// searchable select, typing edits the search instead (Space too).
    pub fn handle_key<T: AsRef<str>>(&mut self, key: &KeyInput, options: &[T]) -> bool {
        let n = options.len();
        if n == 0 {
            self.close();
            return false;
        }
        let before = self.selected;
        self.highlighted = self.highlighted.min(n - 1);
        if self.open {
            let shown = self.shown(options);
            let at = shown.iter().position(|&i| i == self.highlighted);
            let step = |to: Option<usize>| to.and_then(|k| shown.get(k).copied());
            match key {
                KeyInput::Down => {
                    let next = at.map_or(0, |k| (k + 1).min(shown.len().saturating_sub(1)));
                    self.highlighted = step(Some(next)).unwrap_or(self.highlighted);
                }
                KeyInput::Up => {
                    let next = at.map_or(0, |k| k.saturating_sub(1));
                    self.highlighted = step(Some(next)).unwrap_or(self.highlighted);
                }
                KeyInput::Home if !self.searchable => self.highlighted = 0,
                KeyInput::End if !self.searchable => self.highlighted = n - 1,
                KeyInput::Enter => {
                    if at.is_some() {
                        self.choose(self.highlighted);
                    }
                }
                KeyInput::Escape => self.close(),
                KeyInput::Text(t) if t == " " && !self.searchable => self.choose(self.highlighted),
                KeyInput::Text(t) if !self.searchable => {
                    if let Some(i) = next_match(options, self.highlighted, t) {
                        self.highlighted = i;
                    }
                }
                _ if self.searchable => {
                    self.query.apply(key);
                    self.refilter(options);
                }
                _ => {}
            }
        } else {
            match key {
                KeyInput::Down | KeyInput::Up | KeyInput::Enter => self.open(),
                KeyInput::Text(t) if t == " " => self.open(),
                KeyInput::Home => self.choose(0),
                KeyInput::End => self.choose(n - 1),
                KeyInput::Text(t) if self.searchable && !t.trim().is_empty() => {
                    self.open();
                    self.query.apply(key);
                    self.refilter(options);
                }
                KeyInput::Text(t) => {
                    let from = self.selected.unwrap_or(n - 1);
                    if let Some(i) = next_match(options, from, t) {
                        self.choose(i);
                    }
                }
                _ => {}
            }
        }
        self.selected != before
    }
}

/// The first option after `from` (wrapping) whose label starts with `typed`,
/// ignoring case.
fn next_match<T: AsRef<str>>(options: &[T], from: usize, typed: &str) -> Option<usize> {
    let typed = typed.to_lowercase();
    if typed.trim().is_empty() {
        return None;
    }
    let n = options.len();
    (1..=n)
        .map(|k| (from + k) % n)
        .find(|&i| options[i].as_ref().to_lowercase().starts_with(&typed))
}

/// A select: a button showing the chosen option (from `options`) with a
/// chevron, which opens the list of options below it. The list closes on a
/// choice, on Escape, or on a press outside it; while the select has focus the
/// keyboard drives it (see [`Select::handle_key`]). A long list scrolls,
/// building only the rows in view, and keeps the highlighted option in
/// sight; a [searchable](Select::searchable) one has a field at its top that
/// filters it. Its width is 200 logical px unless a column stretches it, as a
/// [`text_field`](super::text_field)'s is; `.width(..)` sets another (the
/// list is at least as wide as the button).
///
/// `field` is an accessor returning the [`Select`] inside the app state. Read
/// the choice back with [`Select::selected`]; [`on_change`](Element::on_change)
/// runs a handler after the user chooses another option.
///
/// Under [`Theme::native`](crate::style::Theme::native) the OS draws the button's bezel; in a window on
/// iOS, macOS and Windows the select is the OS's own pop-up button (a
/// `UIButton` with a `UIMenu`, an `NSPopUpButton`, a `COMBOBOX`), whose
/// menu or list picks the option (see `docs/native-theme.md`), unless it is
/// searchable, which those don't do. Reported to assistive technologies as a
/// [`Role::ComboBox`] named by the chosen option, with a [`Role::List`] of
/// [`Role::ListItem`]s while open.
///
/// ```
/// use stipple::prelude::*;
/// struct App { size: Select, preview: String }
/// fn view(state: &App, cx: &mut Cx<App>) -> Element {
///     const SIZES: [&str; 3] = ["Small", "Medium", "Large"];
///     select(cx, &SIZES, &state.size, |s| &mut s.size).on_change(cx, |s| {
///         let i = s.size.selected().unwrap_or(0);
///         s.preview = format!("{} preview", SIZES[i]);
///     })
/// }
/// # let theme = Theme::light();
/// # let mut cx = Cx::new(&theme);
/// # let el = view(&App { size: Select::new(None), preview: String::new() }, &mut cx);
/// # drop(el);
/// # assert!(cx.into_handlers().len() >= 2);
/// ```
pub fn select<S: 'static, T: AsRef<str>>(
    cx: &mut Cx<S>,
    options: &[T],
    state: &Select,
    field: impl Fn(&mut S) -> &mut Select + Copy + 'static,
) -> Element {
    let theme = &super::theme_of(cx);
    let p = &theme.palette;
    let labels: Vec<String> = options.iter().map(|o| o.as_ref().to_string()).collect();
    // The app's change handlers, which a choice by any route runs.
    let hooks = FieldHooks::new();
    let tap = cx.register(move |s| field(s).toggle());

    let open = state.open && !labels.is_empty();
    let shown = if open {
        state.shown(&labels)
    } else {
        Vec::new()
    };
    let scroll = ListScroll::register(cx, theme, shown.len());
    // The select's keys, registered ahead of its list's (whose search field
    // takes keys too), so it keeps its focus id whether the list shows.
    let keys = {
        let (labels, scroll, h) = (labels.clone(), scroll.clone(), hooks.clone());
        cx.register_key(move |s, k| {
            if field(s).handle_key(k, &labels) {
                h.changed(s);
            }
            let sel = field(s);
            scroll.reveal(sel.highlighted_row(&labels));
        })
    };
    let opened = state.shown.with(|s| s.track(open, cx.frame_time()));
    if open {
        if state.reveal.take() {
            scroll.open_at(cx, state.highlighted_row(&labels));
        }
        let t = motion::progress(cx, theme.motion.popup, opened);
        let header = state.searchable.then(|| {
            let (labels, scroll, hooks) = (labels.clone(), scroll.clone(), hooks.clone());
            search_field(cx, state, field, move |s, k| {
                let sel = field(s);
                let k = match k {
                    // The list's keys, from the field.
                    KeyInput::Up | KeyInput::Down | KeyInput::Enter | KeyInput::Escape => {
                        if sel.handle_key(k, &labels) {
                            hooks.changed(s);
                        }
                        return FieldKey::Kept;
                    }
                    k => k,
                };
                let edited = line_key(&mut sel.query, k);
                sel.refilter(&labels);
                scroll.reveal(sel.highlighted_row(&labels));
                match edited {
                    FieldKey::Submit => FieldKey::Kept,
                    other => other,
                }
            })
        });
        let shown_labels: Vec<&str> = shown.iter().map(|&i| labels[i].as_str()).collect();
        let list = PopupList {
            labels: &shown_labels,
            highlighted: shown.iter().position(|&i| i == state.highlighted),
            checked: Some(
                state
                    .selected
                    .and_then(|c| shown.iter().position(|&i| i == c)),
            ),
            header,
        };
        let list = popup_list(cx, theme, &scroll, list, |row| {
            let (i, hooks) = (shown[row], hooks.clone());
            move |s: &mut S| {
                let before = field(s).selected();
                field(s).choose(i);
                if before != Some(i) {
                    hooks.changed(s);
                }
            }
        });
        // It grows in from its button, and fades back out when it closes.
        let list = motion::popup(list, &theme.motion, t, (0.5, 0.0))
            .exit(motion::popup_exit(&theme.motion, (0.5, 0.0)));
        let dismiss = cx.register(move |s| field(s).close());
        cx.overlay(OverlaySpec {
            content: list,
            anchor: Anchor::Below(tap),
            modal: false,
            dismiss: Some(dismiss),
        });
    }

    let (text, ink) = match state.selected.and_then(|i| labels.get(i)) {
        Some(label) => (label.clone(), p.text),
        None => ("\u{2014}".to_string(), p.text_muted),
    };
    let body = vec![
        Element::text(text, theme.typography.body, ink),
        super::spacer(),
        Element::text("\u{25BE}", theme.typography.body, p.text_muted),
    ];
    let control = NativeControl::Button { primary: false };
    let mut el = Element::stack(Axis::Horizontal, body)
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
        .gap(theme.spacing.sm)
        .default_width(200.0);
    // The OS's pop-up buttons have no search field.
    if let Some(deco) = theme.native_deco(control).filter(|_| !state.searchable) {
        el = native_size(el.native(deco), control).default_width(200.0);
        // Where the window shows it live, it's the OS's pop-up button, whose
        // own menu picks an option: each pick runs one of these.
        let picks = (0..labels.len())
            .map(|i| {
                let hooks = hooks.clone();
                cx.register(move |s| {
                    let before = field(s).selected();
                    field(s).choose(i);
                    if before != Some(i) {
                        hooks.changed(s);
                    }
                })
            })
            .collect();
        el = el.native_choices(crate::core::a11y::Choices {
            labels: labels.clone(),
            selected: state.selected.filter(|&i| i < labels.len()),
            picks,
        });
    }
    el.action = Some(tap);
    el.focus = Some(keys);
    cx.set_field_hooks(keys, hooks);
    // Known again when its search field gives the focus back.
    el.key(("stipple-select", tap)).role(Role::ComboBox)
}

/// The search field at the top of a searchable select's list, editing its
/// query; `keys` takes its keys.
fn search_field<S: 'static>(
    cx: &mut Cx<S>,
    state: &Select,
    field: impl Fn(&mut S) -> &mut Select + Copy + 'static,
    keys: impl FnMut(&mut S, &KeyInput) -> FieldKey + 'static,
) -> Element {
    let hooks = FieldHooks::new();
    keyed_text_field(cx, &state.query, move |s| &mut field(s).query, hooks, keys)
        .input_kind(crate::platform::InputKind::Search)
        .autocorrect(false)
        // Typing goes on there (and an on-screen keyboard shows); when the list
        // closes, focus goes back to the select.
        .autofocus()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::core::Role;
    use crate::geometry::{Point, Size};

    const OPTIONS: [&str; 4] = ["Apple", "Banana", "Blueberry", "Cherry"];

    #[test]
    fn keyboard_opens_moves_and_chooses() {
        let mut s = Select::new(Some(1));
        assert!(!s.handle_key(&KeyInput::Down, &OPTIONS));
        assert!(s.is_open());
        assert_eq!(s.highlighted(), 1, "opens on the chosen option");
        s.handle_key(&KeyInput::Down, &OPTIONS);
        s.handle_key(&KeyInput::Down, &OPTIONS);
        s.handle_key(&KeyInput::Down, &OPTIONS); // clamps at the end
        assert_eq!(s.highlighted(), 3);
        s.handle_key(&KeyInput::Home, &OPTIONS);
        assert_eq!(s.highlighted(), 0);
        s.handle_key(&KeyInput::Up, &OPTIONS); // clamps at the start
        assert_eq!(s.highlighted(), 0);
        assert!(s.handle_key(&KeyInput::Enter, &OPTIONS));
        assert_eq!((s.selected(), s.is_open()), (Some(0), false));
    }

    #[test]
    fn escape_closes_without_choosing() {
        let mut s = Select::new(Some(2));
        s.handle_key(&KeyInput::Text(" ".into()), &OPTIONS);
        assert!(s.is_open());
        s.handle_key(&KeyInput::End, &OPTIONS);
        assert!(!s.handle_key(&KeyInput::Escape, &OPTIONS));
        assert_eq!((s.selected(), s.is_open()), (Some(2), false));
    }

    #[test]
    fn typing_jumps_to_the_next_match() {
        let mut s = Select::new(None);
        assert!(s.handle_key(&KeyInput::Text("b".into()), &OPTIONS));
        assert_eq!(s.selected(), Some(1));
        s.handle_key(&KeyInput::Text("B".into()), &OPTIONS); // next "b", any case
        assert_eq!(s.selected(), Some(2));
        s.handle_key(&KeyInput::Text("b".into()), &OPTIONS); // wraps around
        assert_eq!(s.selected(), Some(1));
        assert!(!s.handle_key(&KeyInput::Text("z".into()), &OPTIONS));
        s.handle_key(&KeyInput::End, &OPTIONS);
        assert_eq!(s.selected(), Some(3));
    }

    #[test]
    fn empty_options_do_nothing() {
        let mut s = Select::new(None);
        let none: [&str; 0] = [];
        assert!(!s.handle_key(&KeyInput::Down, &none));
        assert!(!s.is_open());
    }

    struct State {
        fruit: Select,
    }

    fn app() -> App<State> {
        App::new(
            State {
                fruit: Select::new(Some(0)),
            },
            |s: &State, cx: &mut Cx<State>| {
                super::super::column(vec![select(cx, &OPTIONS, &s.fruit, |s| &mut s.fruit)])
                    .padding(Insets::uniform(10.0))
            },
        )
        .no_font()
        .logical_size(Size::new(300.0, 300.0))
    }

    #[test]
    fn clicks_open_the_list_below_and_choose() {
        let mut app = app();
        app.render_once();
        // The button spans x 10..210 from y 10.
        assert!(app.click_at(Point::new(100.0, 20.0)));
        assert!(app.state().fruit.is_open());
        app.render_once();
        let tree = app.accessibility_tree().unwrap();
        let nodes = tree.descendants();
        let list = nodes
            .iter()
            .find(|n| n.role == Role::List)
            .expect("the open list is in the accessibility tree");
        let items: Vec<_> = list
            .descendants()
            .into_iter()
            .filter(|n| n.role == Role::ListItem)
            .collect();
        assert_eq!(items.len(), OPTIONS.len());
        assert_eq!(items[2].name, "Blueberry");
        assert!(list.bounds.min_y() > 20.0, "dropped below the button");
        assert!(
            list.bounds.width() >= 200.0,
            "at least as wide as the button"
        );
        let target = items[2].bounds.center();
        // Click the third option.
        assert!(app.click_at(target));
        assert_eq!(app.state().fruit.selected(), Some(2));
        assert!(!app.state().fruit.is_open());
        // The select kept keyboard focus through the choice.
        assert!(app.focused().is_some());
        assert!(app.press_key(KeyInput::Down));
        assert!(app.state().fruit.is_open());
    }

    #[test]
    fn an_outside_click_closes_the_list() {
        let mut app = app();
        app.render_once();
        app.click_at(Point::new(100.0, 20.0));
        app.render_once();
        assert!(app.click_at(Point::new(280.0, 280.0)));
        assert!(!app.state().fruit.is_open());
        assert_eq!(app.state().fruit.selected(), Some(0));
    }

    #[test]
    fn reports_a_combo_box_named_by_its_value() {
        let mut app = app();
        app.render_once();
        let tree = app.accessibility_tree().unwrap();
        let combo = tree
            .descendants()
            .into_iter()
            .find(|n| n.role == Role::ComboBox)
            .unwrap()
            .clone();
        assert_eq!(combo.name, "Apple");
    }

    #[test]
    fn it_stretches_in_a_column_and_keeps_a_set_width() {
        let mut app = app();
        let combo = app.find(Role::ComboBox, "Apple").unwrap();
        // The column is 280 wide inside its padding.
        assert_eq!(combo.bounds.width(), 280.0);
        let mut app = App::new(
            State {
                fruit: Select::new(Some(0)),
            },
            |s: &State, cx: &mut Cx<State>| {
                super::super::column(vec![
                    select(cx, &OPTIONS, &s.fruit, |s| &mut s.fruit).width(120.0),
                ])
            },
        )
        .no_font()
        .logical_size(Size::new(300.0, 300.0));
        let combo = app.find(Role::ComboBox, "Apple").unwrap();
        assert_eq!(combo.bounds.width(), 120.0);
    }

    struct Picked {
        fruit: Select,
        changes: Vec<Option<usize>>,
    }

    fn picked(options: &'static [&'static str], fruit: Select) -> App<Picked> {
        App::new(
            Picked {
                fruit,
                changes: Vec::new(),
            },
            move |s: &Picked, cx: &mut Cx<Picked>| {
                super::super::column(vec![
                    select(cx, options, &s.fruit, |s| &mut s.fruit).on_change(cx, |s| {
                        let now = s.fruit.selected();
                        s.changes.push(now);
                    }),
                ])
            },
        )
        .no_font()
        .logical_size(Size::new(300.0, 400.0))
    }

    #[test]
    fn on_change_runs_after_a_click_or_a_key_changes_the_choice() {
        let mut app = picked(&OPTIONS, Select::new(Some(0)));
        assert!(app.click(Role::ComboBox, "Apple"));
        assert!(app.click(Role::ListItem, "Cherry"));
        assert_eq!(app.state().changes, vec![Some(3)]);
        // Choosing the chosen one again changes nothing.
        app.click(Role::ComboBox, "Cherry");
        app.click(Role::ListItem, "Cherry");
        assert_eq!(app.state().changes, vec![Some(3)]);
        // The keyboard: Home chooses the first.
        assert!(app.press_key(KeyInput::Home));
        assert_eq!(app.state().changes, vec![Some(3), Some(0)]);
        // Opening and closing isn't a change.
        app.press_key(KeyInput::Down);
        app.press_key(KeyInput::Escape);
        assert_eq!(app.state().changes.len(), 2);
    }

    const MANY: [&str; 40] = [
        "Agate",
        "Amber",
        "Beryl",
        "Coral",
        "Diamond",
        "Emerald",
        "Garnet",
        "Granite",
        "Jade",
        "Jasper",
        "Jet",
        "Lapis",
        "Malachite",
        "Marble",
        "Moonstone",
        "Obsidian",
        "Onyx",
        "Opal",
        "Pearl",
        "Peridot",
        "Pumice",
        "Quartz",
        "Ruby",
        "Sapphire",
        "Shale",
        "Slate",
        "Spinel",
        "Sunstone",
        "Tanzanite",
        "Topaz",
        "Tourmaline",
        "Turquoise",
        "Zircon",
        "Basalt",
        "Chalk",
        "Flint",
        "Gneiss",
        "Mica",
        "Schist",
        "Talc",
    ];

    fn items(app: &mut App<Picked>) -> Vec<String> {
        app.render_once();
        app.accessibility_tree()
            .unwrap()
            .descendants()
            .into_iter()
            .filter(|n| n.role == Role::ListItem)
            .map(|n| n.name.clone())
            .collect()
    }

    #[test]
    fn a_long_list_builds_the_rows_in_view_and_follows_the_highlight() {
        let mut app = picked(&MANY, Select::new(Some(0)));
        app.click(Role::ComboBox, "Agate");
        let first = items(&mut app);
        assert!(first.len() < MANY.len(), "only the rows in view: {first:?}");
        assert_eq!(first[0], "Agate");
        // Down twenty times: the list scrolls along, keeping it in view.
        for _ in 0..20 {
            app.press_key(KeyInput::Down);
            app.render_once();
        }
        assert_eq!(app.state().fruit.highlighted(), 20);
        let list = app.find(Role::List, "").expect("the list");
        let shown = app.find(Role::ListItem, MANY[20]).expect("in view");
        assert!(shown.bounds.min_y() >= list.bounds.min_y() - 0.5);
        assert!(shown.bounds.max_y() <= list.bounds.max_y() + 0.5);
        assert!(!items(&mut app).contains(&"Agate".to_string()));
        app.press_key(KeyInput::Enter);
        assert_eq!(app.state().changes, vec![Some(20)]);
        // Opened again, it shows the chosen one.
        app.press_key(KeyInput::Down);
        app.render_once();
        app.render_once();
        assert!(app.find(Role::ListItem, MANY[20]).is_some());
    }

    #[test]
    fn a_searchable_select_filters_as_you_type() {
        let mut app = picked(&MANY, Select::new(None).searchable(true));
        app.render_once();
        // Typing on the focused select opens it, searching.
        app.focus_next();
        assert!(app.type_text("s"));
        assert!(app.state().fruit.is_open());
        app.type_text("t");
        assert_eq!(app.state().fruit.query(), "st");
        let shown = items(&mut app);
        assert_eq!(shown, vec!["Moonstone", "Sunstone", "Schist"]);
        // Down from the first match, then Enter.
        app.press_key(KeyInput::Down);
        app.press_key(KeyInput::Enter);
        assert_eq!(app.state().fruit.selected(), Some(27));
        assert_eq!(app.state().changes, vec![Some(27)]);
        assert_eq!(app.state().fruit.query(), "", "closing forgets the search");
        app.render_once();
        let combo = app.find(Role::ComboBox, "Sunstone").unwrap();
        assert!(combo.focused, "focus went back to the select");
        // Opened again, its search field takes the focus, and the typing.
        app.press_key(KeyInput::Down);
        app.render_once();
        let search = app.find(Role::TextField, "").expect("a search field");
        app.click_at(search.bounds.center());
        app.type_text("CH");
        assert_eq!(items(&mut app), vec!["Malachite", "Chalk", "Schist"]);
        app.press_key(KeyInput::Enter);
        assert_eq!(app.state().fruit.selected(), Some(12));
        assert!(!app.state().fruit.is_open());
    }
}
