//! A select (dropdown): a button showing the chosen option, which opens a list
//! of the options below it.

use crate::core::{Align, Anchor, Axis, Cx, Element, KeyInput, OverlaySpec, Role};
use crate::geometry::Insets;
use crate::platform::native::NativeControl;
use crate::style::Theme;

use super::motion::{self, Memo, Shown};
use super::{native_size, scroll};

/// Lists longer than this scroll.
const MAX_VISIBLE: usize = 8;

/// The state of a [`select`]: which option is chosen, whether the list is open,
/// and which option the keyboard has highlighted in it. Keep one in your app
/// state per select.
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
}

impl Select {
    /// A closed select with `selected` chosen (`None` shows a dash).
    pub fn new(selected: Option<usize>) -> Self {
        Self {
            selected,
            open: false,
            highlighted: selected.unwrap_or(0),
            shown: Memo::default(),
        }
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
    }

    /// Hide the list, keeping the choice.
    pub fn close(&mut self) {
        self.open = false;
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
        self.open = false;
    }

    /// Handle a key pressed while the select has focus, over its `options`.
    /// Returns `true` if the chosen option changed.
    ///
    /// Closed: Down, Up, Enter and Space open the list; Home and End choose
    /// the first and last option; typing a letter chooses the next option
    /// starting with it. Open: Up and Down (and Home and End) move the
    /// highlight, Enter or Space chooses it, Escape closes the list, and a
    /// letter highlights the next option starting with it.
    pub fn handle_key<T: AsRef<str>>(&mut self, key: &KeyInput, options: &[T]) -> bool {
        let n = options.len();
        if n == 0 {
            self.close();
            return false;
        }
        let before = self.selected;
        self.highlighted = self.highlighted.min(n - 1);
        if self.open {
            match key {
                KeyInput::Down => self.highlighted = (self.highlighted + 1).min(n - 1),
                KeyInput::Up => self.highlighted = self.highlighted.saturating_sub(1),
                KeyInput::Home => self.highlighted = 0,
                KeyInput::End => self.highlighted = n - 1,
                KeyInput::Enter => self.choose(self.highlighted),
                KeyInput::Text(t) if t == " " => self.choose(self.highlighted),
                KeyInput::Escape => self.close(),
                KeyInput::Text(t) => {
                    if let Some(i) = next_match(options, self.highlighted, t) {
                        self.highlighted = i;
                    }
                }
                _ => {}
            }
        } else {
            match key {
                KeyInput::Down | KeyInput::Up | KeyInput::Enter => self.open(),
                KeyInput::Text(t) if t == " " => self.open(),
                KeyInput::Home => self.choose(0),
                KeyInput::End => self.choose(n - 1),
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
/// keyboard drives it (see [`Select::handle_key`]). Default width 200 logical
/// px; override with `.width(..)` (the list is at least as wide as the button).
///
/// `field` is an accessor returning the [`Select`] inside the app state. Read
/// the choice back with [`Select::selected`].
///
/// Under [`Theme::native`] the OS draws the button's bezel. Reported to
/// assistive technologies as a [`Role::ComboBox`] named by the chosen option,
/// with a [`Role::List`] of [`Role::ListItem`]s while open.
///
/// ```
/// use stipple::prelude::*;
/// struct App { size: Select }
/// fn view(state: &App, cx: &mut Cx<App>) -> Element {
///     let theme = *cx.theme();
///     select(cx, &theme, &["Small", "Medium", "Large"], &state.size, |s: &mut App| &mut s.size)
/// }
/// # let theme = Theme::light();
/// # let mut cx = Cx::new(&theme);
/// # let el = view(&App { size: Select::new(None) }, &mut cx);
/// # assert!(el.action.is_some() && el.focus.is_some());
/// ```
pub fn select<S, T: AsRef<str>>(
    cx: &mut Cx<S>,
    theme: &Theme,
    options: &[T],
    state: &Select,
    field: impl Fn(&mut S) -> &mut Select + Copy + 'static,
) -> Element {
    let p = &theme.palette;
    let labels: Vec<String> = options.iter().map(|o| o.as_ref().to_string()).collect();
    let tap = cx.register(move |s: &mut S| field(s).toggle());

    let open = state.open && !labels.is_empty();
    let opened = state.shown.with(|s| s.track(open, cx.frame_time()));
    if open {
        let t = motion::progress(cx, theme.motion.popup, opened);
        let list = option_list(cx, theme, &labels, state, field);
        // It grows in from its button, and fades back out when it closes.
        let list = motion::popup(list, &theme.motion, t, (0.5, 0.0))
            .exit(motion::popup_exit(&theme.motion, (0.5, 0.0)));
        let dismiss = cx.register(move |s: &mut S| field(s).close());
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
        .align(Align::Start, Align::Center)
        .width(200.0);
    if let Some(deco) = theme.native_deco(control) {
        el = native_size(el.native(deco), control).width(200.0);
    }
    el.action = Some(tap);
    el.role(Role::ComboBox).on_key(cx, move |s: &mut S, k| {
        field(s).handle_key(k, &labels);
    })
}

/// The open list: one row per option, the highlighted one tinted and the
/// chosen one checked.
fn option_list<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    labels: &[String],
    state: &Select,
    field: impl Fn(&mut S) -> &mut Select + Copy + 'static,
) -> Element {
    let p = &theme.palette;
    let row_height = (theme.typography.body * 1.4 + 2.0 * theme.spacing.sm).round();
    let mut rows = Vec::with_capacity(labels.len());
    for (i, text) in labels.iter().enumerate() {
        let mark = if state.selected == Some(i) {
            "\u{2713}"
        } else {
            ""
        };
        let mut row = Element::stack(
            Axis::Horizontal,
            vec![
                Element::text(mark, theme.typography.body, p.primary).width(16.0),
                Element::text(text.clone(), theme.typography.body, p.text),
            ],
        )
        .radius(theme.radius / 2.0)
        .padding(Insets::symmetric(theme.spacing.sm, 0.0))
        .gap(theme.spacing.xs)
        .align(Align::Start, Align::Center)
        .height(row_height)
        .role(Role::ListItem)
        .a11y_label(text.clone());
        if i == state.highlighted {
            row = row.fill(p.selection);
        }
        rows.push(row.on_tap(cx, move |s: &mut S| field(s).choose(i)));
    }
    let many = rows.len() > MAX_VISIBLE;
    let mut list = Element::stack(Axis::Vertical, rows)
        .gap(2.0)
        .align(Align::Start, Align::Stretch);
    if many {
        let height = MAX_VISIBLE as f64 * (row_height + 2.0);
        list = scroll(cx, height, list);
    }
    Element::stack(Axis::Vertical, vec![list])
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::uniform(theme.spacing.xs))
        .align(Align::Start, Align::Stretch)
        .clip()
        .role(Role::List)
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
                let theme = *cx.theme();
                super::super::column(vec![select(
                    cx,
                    &theme,
                    &OPTIONS,
                    &s.fruit,
                    |s: &mut State| &mut s.fruit,
                )])
                .padding(Insets::uniform(10.0))
            },
        )
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
}
