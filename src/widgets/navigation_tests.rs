//! App-level tests of the navigation bar, the rail and the page stack:
//! selecting by tap and keyboard, roles and badges, the looks' proportions,
//! and pages kept alive (scrolled, focused) while another shows.

use std::cell::RefCell;

use super::navigation::*;
use crate::App;
use crate::core::{AccessNode, Align, Axis, Cx, Element, KeyInput, Role, ScrollId};
use crate::geometry::{Point, Rect, Size};
use crate::platform::{Event, ScrollDelta};
use crate::style::Theme;

const HOME: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M12 3l9 8h-3v9h-5v-6h-2v6H6v-9H3z" fill="currentColor"/></svg>"#;

#[derive(Default)]
struct State {
    tab: usize,
    log: Vec<String>,
    look: Option<NavLook>,
    inset: f64,
    rail: bool,
    /// What each page was told as it was built: (page, visible).
    built: RefCell<Vec<(usize, bool)>>,
}

fn items() -> Vec<NavItem> {
    vec![
        NavItem::new("Scan", NavIcon::template(HOME)),
        NavItem::new("History", NavIcon::template(HOME)).badge(Some(NavBadge::Count(3))),
        NavItem::new("Create", NavIcon::template(HOME)),
        NavItem::new("Settings", NavIcon::template(HOME)).badge(Some(NavBadge::Dot)),
    ]
}

fn page(s: &State, cx: &mut Cx<State>, i: usize, visible: bool) -> Element {
    s.built.borrow_mut().push((i, visible));
    let theme = *cx.theme();
    match i {
        0 => {
            let rows = (0..30)
                .map(|r| {
                    super::label(&theme, format!("Row {r}"))
                        .height(40.0)
                        .on_tap(cx, move |s: &mut State| s.log.push(format!("row {r}")))
                })
                .collect();
            let id = cx.register_scroll();
            Element::stack(Axis::Vertical, rows)
                .align(Align::Start, Align::Stretch)
                .scrollable(id)
                .grow(1.0)
        }
        1 => Element::text("Field", 14.0, theme.palette.text)
            .on_key(cx, |s: &mut State, _k: &KeyInput| {
                s.log.push("typed".into())
            })
            .a11y_label("Field"),
        n => super::label(&theme, format!("Page {n}")),
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let pages = page_stack(cx, 4, s.tab, |cx, i, visible| page(s, cx, i, visible));
    let on = |s: &mut State, i| {
        s.log.push(format!("select {i}"));
        s.tab = i;
    };
    if s.rail {
        let rail = navigation_rail(cx, &theme, items(), s.tab, on);
        return super::row(vec![rail, pages])
            .align(Align::Start, Align::Stretch)
            .grow(1.0);
    }
    let mut bar = NavigationBar::new(items(), s.tab, on).bottom_inset(s.inset);
    if let Some(look) = s.look {
        bar = bar.look(look);
    }
    super::column(vec![pages, bar.view(cx, &theme)])
        .align(Align::Start, Align::Stretch)
        .grow(1.0)
}

fn app_with(state: State) -> App<State> {
    let mut app = App::new(state, view).logical_size(Size::new(400.0, 600.0));
    app.render_once();
    app
}

fn app() -> App<State> {
    app_with(State::default())
}

fn find(app: &App<State>, role: Role) -> AccessNode {
    app.accessibility_tree()
        .unwrap()
        .descendants()
        .into_iter()
        .find(|n| n.role == role)
        .unwrap_or_else(|| panic!("no {role:?}"))
        .clone()
}

fn tabs(app: &App<State>) -> Vec<AccessNode> {
    find(app, Role::TabList)
        .descendants()
        .into_iter()
        .filter(|n| n.role == Role::Tab)
        .cloned()
        .collect()
}

fn named(app: &App<State>, name: &str) -> Option<Rect> {
    app.accessibility_tree()
        .unwrap()
        .descendants()
        .into_iter()
        .find(|n| n.name == name)
        .map(|n| n.bounds)
}

#[test]
fn the_bar_is_a_tab_list_of_its_destinations() {
    let app = app();
    let tabs = tabs(&app);
    let names: Vec<_> = tabs.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Scan", "History, 3 new", "Create", "Settings, new"]);
    assert!(tabs[0].selected && !tabs[1].selected);
    // The items share the width, along the bottom.
    let w: Vec<f64> = tabs.iter().map(|t| t.bounds.width()).collect();
    assert!(w.iter().all(|x| (x - 100.0).abs() < 0.5), "{w:?}");
    let bar = find(&app, Role::TabList).bounds;
    assert_eq!(bar.max_y(), 600.0);
    assert_eq!(bar.height(), 80.0, "Material's bar is 80 tall");
}

#[test]
fn a_tap_or_the_keyboard_selects() {
    let mut app = app();
    let history = tabs(&app)[1].bounds.center();
    assert!(app.click_at(history));
    assert_eq!(app.state().tab, 1);
    app.render_once();
    assert!(tabs(&app)[1].selected);
    // The tap focused the bar: the arrows move along it.
    app.press_key(KeyInput::Right);
    assert_eq!(app.state().tab, 2);
    app.render_once();
    app.press_key(KeyInput::End);
    assert_eq!(app.state().tab, 3);
    app.render_once();
    app.press_key(KeyInput::Right); // already the last: nothing
    app.press_key(KeyInput::Home);
    assert_eq!(app.state().tab, 0);
    assert_eq!(
        app.state().log,
        ["select 1", "select 2", "select 3", "select 0"]
    );
}

#[test]
fn the_ios_look_is_a_tab_bars_height_and_the_inset_extends_it() {
    let app = app_with(State {
        look: Some(NavLook::Ios),
        inset: 34.0,
        ..State::default()
    });
    let bar = find(&app, Role::TabList).bounds;
    // A hairline, 49 for the items, 34 of safe area under them.
    assert_eq!(bar.height(), 0.5 + 49.0 + 34.0);
    let item = tabs(&app)[0].bounds;
    assert_eq!(item.height(), 49.0);
    assert_eq!(item.max_y(), 600.0 - 34.0, "the items stay above the inset");
    // Material, with the same inset.
    let app = app_with(State {
        inset: 34.0,
        ..State::default()
    });
    assert_eq!(find(&app, Role::TabList).bounds.height(), 80.0 + 34.0);
    assert_eq!(NavLook::of(&Theme::light()), NavLook::Material);
}

#[test]
fn only_the_shown_page_is_drawn_hit_or_read() {
    let mut app = app();
    assert!(named(&app, "Row 0").is_some());
    assert!(named(&app, "Field").is_none(), "page 1 is off stage");
    assert!(named(&app, "Page 2").is_none());
    // Every page is built, each told whether it shows.
    let built = app.state().built.borrow().clone();
    assert_eq!(
        &built[built.len() - 4..],
        [(0, true), (1, false), (2, false), (3, false)]
    );
    let row = named(&app, "Row 1").unwrap().center();
    assert!(app.click_at(row));
    assert_eq!(app.state().log, ["row 1"]);
    // Shown page 2: the rows under it are gone for the pointer.
    let b = tabs(&app)[2].bounds.center();
    app.click_at(b);
    app.render_once();
    assert!(named(&app, "Page 2").is_some());
    assert!(named(&app, "Row 0").is_none());
    app.click_at(row); // nothing to tap where the rows were
    assert_eq!(app.state().log, ["row 1", "select 2"]);
    let built = app.state().built.borrow().clone();
    assert_eq!(
        &built[built.len() - 4..],
        [(0, false), (1, false), (2, true), (3, false)]
    );
}

#[test]
fn a_page_left_comes_back_scrolled_where_it_was() {
    let mut app = app();
    let offset = |app: &App<State>| app.panes[0].scroll_offsets.get(&ScrollId(0)).copied();
    app.pointer_event(Event::PointerMoved {
        position: Point::new(50.0, 100.0),
    });
    assert!(app.pointer_event(Event::Scroll {
        delta: ScrollDelta::lines(0.0, 3.0),
    }));
    app.render_once();
    let scrolled = offset(&app).unwrap();
    assert!(scrolled > 0.0);
    let top = named(&app, "Row 5").unwrap();
    // Away to page 3 and back: the list kept its offset, and draws there.
    let to = |app: &mut App<State>, i: usize| {
        let at = tabs(app)[i].bounds.center();
        assert!(app.click_at(at));
        app.render_once();
    };
    to(&mut app, 3);
    // The wheel over page 3 doesn't reach the list behind it.
    app.pointer_event(Event::PointerMoved {
        position: Point::new(50.0, 100.0),
    });
    app.pointer_event(Event::Scroll {
        delta: ScrollDelta::lines(0.0, 3.0),
    });
    app.render_once();
    assert_eq!(offset(&app), Some(scrolled));
    to(&mut app, 0);
    assert_eq!(offset(&app), Some(scrolled));
    assert_eq!(named(&app, "Row 5"), Some(top));
}

#[test]
fn focus_in_a_page_that_leaves_goes() {
    let mut app = app();
    let at = tabs(&app)[1].bounds.center();
    assert!(app.click_at(at));
    app.render_once();
    // Focus the field (Tab reaches it before or after the bar).
    let mut typed = false;
    for _ in 0..4 {
        app.focus_next();
        app.press_key(KeyInput::Text("a".into()));
        if app.state().log.last().is_some_and(|l| l == "typed") {
            typed = true;
            break;
        }
    }
    assert!(typed, "{:?}", app.state().log);
    assert!(app.focused().is_some());
    // The app moves to another destination (not by a tap on the bar, which
    // would take the focus anyway): the field goes off stage, and its focus
    // with it.
    app.handle().update(|s: &mut State| s.tab = 0);
    app.run_pending();
    app.render_once();
    assert_eq!(app.focused(), None);
    app.press_key(KeyInput::Text("b".into()));
    assert_eq!(app.state().log.last().unwrap(), "typed");
    assert_eq!(app.state().log.iter().filter(|l| *l == "typed").count(), 1);
}

#[test]
fn the_rail_runs_down_the_side() {
    let mut app = app_with(State {
        rail: true,
        ..State::default()
    });
    let tabs = tabs(&app);
    assert_eq!(tabs.len(), 4);
    let rail = find(&app, Role::TabList).bounds;
    assert_eq!(
        (rail.min_x(), rail.width(), rail.height()),
        (0.0, 80.0, 600.0)
    );
    assert!(tabs[1].bounds.min_y() > tabs[0].bounds.max_y());
    assert!(app.click_at(tabs[2].bounds.center()));
    assert_eq!(app.state().tab, 2);
    app.render_once();
    app.press_key(KeyInput::Up);
    assert_eq!(app.state().tab, 1);
    app.render_once();
    // The pages sit beside it.
    assert!(named(&app, "Field").unwrap().min_x() >= 80.0);
}
