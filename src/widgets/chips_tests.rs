//! App-level tests of the chips: roles and names, picking and toggling by
//! tap and by keyboard, arrow-key focus in a group (right to left too), and
//! wrapping.

use super::chips::*;
use crate::App;
use crate::core::{AccessNode, Align, Cx, Element, KeyInput, LayoutDirection, Role};
use crate::geometry::{PhysicalSize, Size};
use crate::render::Pixmap;
use crate::style::Theme;
use std::sync::Arc;

const SYMBOLOGIES: [&str; 4] = ["2D", "Stacked", "Linear", "Postal"];
const FILTERS: [&str; 3] = ["QR", "EAN", "Code 128"];

struct State {
    choice: Option<usize>,
    filters: Vec<bool>,
    alone: bool,
    radio: bool,
    horizontal: bool,
}

impl State {
    fn new() -> Self {
        Self {
            choice: Some(1),
            filters: vec![false, true, false],
            alone: false,
            radio: false,
            horizontal: false,
        }
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let icon = Arc::new(Pixmap::new(PhysicalSize::new(4, 4)));
    let choices = choice_chips(cx, &theme, SYMBOLOGIES, s.choice, |s: &mut State, i| {
        s.choice = Some(i)
    });
    let filters = filter_chips(cx, &theme, FILTERS, &s.filters, |s: &mut State, i| {
        s.filters[i] = !s.filters[i]
    });
    let filters = if s.horizontal {
        super::scroll_horizontal(cx, 60.0, filters)
    } else {
        filters
    };
    super::column(vec![
        choices,
        filters,
        super::row(vec![
            chip(cx, &theme, Chip::new("Flash").icon(&icon), s.alone, |s| {
                s.alone = !s.alone
            }),
            super::radio(cx, &theme, s.radio, |s: &mut State| s.radio = true),
        ]),
    ])
    .gap(8.0)
    // Stretched across, the groups wrap at the window's width.
    .align(Align::Start, Align::Stretch)
}

fn app_with(theme: Theme, width: f64, state: State) -> App<State> {
    let mut app = App::new(state, view)
        .theme(theme)
        .logical_size(Size::new(width, 400.0));
    app.render_once();
    app
}

fn app(width: f64) -> App<State> {
    app_with(Theme::light(), width, State::new())
}

fn nodes(app: &App<State>, role: Role) -> Vec<AccessNode> {
    app.accessibility_tree()
        .unwrap()
        .descendants()
        .into_iter()
        .filter(|n| n.role == role)
        .cloned()
        .collect()
}

fn named(app: &App<State>, name: &str) -> AccessNode {
    app.accessibility_tree()
        .unwrap()
        .descendants()
        .into_iter()
        .find(|n| n.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
        .clone()
}

fn focused_name(app: &App<State>) -> Option<String> {
    app.accessibility_tree()
        .unwrap()
        .descendants()
        .into_iter()
        .find(|n| n.focused)
        .map(|n| n.name.clone())
}

#[test]
fn choice_chips_are_radio_buttons_and_filters_checkboxes() {
    let app = app(600.0);
    let radios = nodes(&app, Role::RadioButton);
    // The four choices and the radio widget.
    assert_eq!(radios.len(), 5);
    let names: Vec<&str> = radios[..4].iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, SYMBOLOGIES, "named by the label, not the check mark");
    let checked: Vec<Option<bool>> = radios.iter().map(|n| n.checked).collect();
    assert_eq!(
        checked,
        [
            Some(false),
            Some(true),
            Some(false),
            Some(false),
            Some(false)
        ]
    );
    let boxes = nodes(&app, Role::CheckBox);
    let names: Vec<&str> = boxes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["QR", "EAN", "Code 128", "Flash"]);
    assert_eq!(boxes[1].checked, Some(true));
    assert_eq!(boxes[3].checked, Some(false));
}

#[test]
fn taps_pick_and_toggle() {
    let mut app = app(600.0);
    assert!(app.click_at(named(&app, "Postal").bounds.center()));
    assert_eq!(app.state().choice, Some(3));
    app.render_once();
    assert_eq!(named(&app, "Postal").checked, Some(true));
    assert_eq!(named(&app, "Stacked").checked, Some(false));
    assert!(app.click_at(named(&app, "QR").bounds.center()));
    assert!(app.click_at(named(&app, "EAN").bounds.center()));
    assert_eq!(app.state().filters, [true, false, false]);
    assert!(app.click_at(named(&app, "Flash").bounds.center()));
    assert!(app.state().alone);
    app.render_once();
    assert_eq!(named(&app, "Flash").checked, Some(true));
}

#[test]
fn tab_stops_at_the_picked_choice_and_the_arrows_pick() {
    let mut app = app(600.0);
    assert!(app.focus_next());
    app.render_once();
    assert_eq!(focused_name(&app).as_deref(), Some("Stacked"));
    // The next stop is past the group: the first filter.
    assert!(app.focus_next());
    app.render_once();
    assert_eq!(focused_name(&app).as_deref(), Some("QR"));
    assert!(app.focus_prev());
    app.render_once();
    assert_eq!(focused_name(&app).as_deref(), Some("Stacked"));

    assert!(app.press_key(KeyInput::Right));
    assert_eq!(app.state().choice, Some(2));
    app.render_once();
    assert_eq!(focused_name(&app).as_deref(), Some("Linear"));
    assert!(app.press_key(KeyInput::Down));
    app.render_once();
    assert_eq!(app.state().choice, Some(3));
    // Past the last, back to the first.
    assert!(app.press_key(KeyInput::Right));
    app.render_once();
    assert_eq!(app.state().choice, Some(0));
    assert_eq!(focused_name(&app).as_deref(), Some("2D"));
    assert!(app.press_key(KeyInput::Up));
    app.render_once();
    assert_eq!(app.state().choice, Some(3));
    assert!(app.press_key(KeyInput::Home));
    app.render_once();
    assert_eq!(app.state().choice, Some(0));
    assert!(app.press_key(KeyInput::End));
    app.render_once();
    assert_eq!(app.state().choice, Some(3));
    assert!(!app.press_key(KeyInput::Escape), "other keys do nothing");
}

#[test]
fn with_nothing_picked_tab_stops_at_the_first_choice() {
    let state = State {
        choice: None,
        ..State::new()
    };
    let mut app = app_with(Theme::light(), 600.0, state);
    assert!(app.focus_next());
    app.render_once();
    assert_eq!(focused_name(&app).as_deref(), Some("2D"));
    // Space picks it.
    assert!(app.type_text(" "));
    assert_eq!(app.state().choice, Some(0));
}

#[test]
fn arrows_move_among_filters_without_toggling_them() {
    let mut app = app(600.0);
    app.focus_next();
    app.focus_next();
    app.render_once();
    assert_eq!(focused_name(&app).as_deref(), Some("QR"));
    // Each filter is a Tab stop.
    assert!(app.focus_next());
    app.render_once();
    assert_eq!(focused_name(&app).as_deref(), Some("EAN"));
    assert!(app.press_key(KeyInput::Right));
    app.render_once();
    assert_eq!(focused_name(&app).as_deref(), Some("Code 128"));
    assert_eq!(app.state().filters, [false, true, false]);
    assert!(app.press_key(KeyInput::Left));
    assert!(app.press_key(KeyInput::Left));
    app.render_once();
    assert_eq!(focused_name(&app).as_deref(), Some("QR"));
    assert!(app.type_text(" "));
    assert_eq!(app.state().filters, [true, true, false]);
    // A chip alone isn't in a group: the arrows leave it be.
    app.focus_next();
    app.focus_next();
    app.focus_next();
    app.render_once();
    assert_eq!(focused_name(&app).as_deref(), Some("Flash"));
    assert!(!app.press_key(KeyInput::Right));
}

#[test]
fn right_to_left_the_arrows_follow_the_mirrored_row() {
    let theme = Theme::light().with_layout_direction(LayoutDirection::Rtl);
    let mut app = app_with(theme, 600.0, State::new());
    let (stacked, linear) = (named(&app, "Stacked"), named(&app, "Linear"));
    assert!(linear.bounds.max_x() < stacked.bounds.min_x(), "mirrored");
    app.focus_next();
    app.render_once();
    assert!(app.press_key(KeyInput::Left));
    assert_eq!(app.state().choice, Some(2), "left goes on to Linear");
    app.render_once();
    assert!(app.press_key(KeyInput::Right));
    assert_eq!(app.state().choice, Some(1));
}

#[test]
fn groups_wrap_but_scroll_sideways_on_one_line() {
    // (Without a font, text measures nothing: a chip is its padding, and
    // the selected one its check mark too.)
    let app = app(70.0);
    let ys: Vec<f64> = SYMBOLOGIES
        .iter()
        .map(|n| named(&app, n).bounds.min_y())
        .collect();
    assert!(ys[3] > ys[0], "wrapped onto another line: {ys:?}");
    for n in SYMBOLOGIES {
        assert!(named(&app, n).bounds.max_x() <= 70.0 + 0.5, "{n} fits");
    }
    let state = State {
        horizontal: true,
        ..State::new()
    };
    let app = app_with(Theme::light(), 70.0, state);
    let ys: Vec<f64> = FILTERS
        .iter()
        .map(|n| named(&app, n).bounds.min_y())
        .collect();
    assert!(
        ys.iter().all(|y| (y - ys[0]).abs() < 0.5),
        "one line: {ys:?}"
    );
    assert!(
        named(&app, "Code 128").bounds.max_x() > 60.0,
        "past the viewport"
    );
}

#[test]
fn the_radio_widget_reads_as_a_radio_button() {
    let mut app = app(600.0);
    let radio = nodes(&app, Role::RadioButton).pop().unwrap();
    assert_eq!(radio.checked, Some(false));
    assert!(app.click_at(radio.bounds.center()));
    app.render_once();
    assert_eq!(nodes(&app, Role::RadioButton)[4].checked, Some(true));
}
