//! App-level tests of the tab bar: selecting, closing, adding, the keyboard,
//! reordering by drag, overflow and the context menu.

use super::tab_bar::*;
use crate::App;
use crate::core::{AccessNode, Align, Cx, Element, KeyInput, Menu, MenuItem, Role};
use crate::geometry::{Point, Rect, Size};
use crate::render::Color;

struct Doc {
    id: u64,
    name: String,
    color: Option<Color>,
    bell: bool,
}

struct State {
    docs: Vec<Doc>,
    selected: usize,
    bar: TabBarState,
    next: u64,
    log: Vec<String>,
}

impl State {
    fn new(n: usize) -> Self {
        Self {
            docs: (0..n)
                .map(|i| Doc {
                    id: i as u64,
                    name: format!("Doc {i}"),
                    color: (i == 1).then_some(Color::rgb(0xe5, 0x48, 0x4d)),
                    bell: i == 2,
                })
                .collect(),
            selected: 0,
            bar: TabBarState::new(),
            next: n as u64,
            log: Vec::new(),
        }
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let tabs = s
        .docs
        .iter()
        .map(|d| {
            let id = d.id;
            Tab::new(d.id, d.name.clone())
                .color(d.color)
                .indicator(d.bell.then_some(TabIndicator::Bell))
                .context_menu(Menu::new("").item(MenuItem::new(
                    "Close Others",
                    move |s: &mut State| {
                        s.docs.retain(|d| d.id == id);
                        s.selected = 0;
                    },
                )))
        })
        .collect();
    let bar = TabBar::new("docs", tabs)
        .selected(s.selected)
        .on_select(|s: &mut State, i| s.selected = i)
        .on_close(|s: &mut State, i| {
            s.log.push(format!("close {i}"));
            s.docs.remove(i);
            s.selected = s.selected.min(s.docs.len().saturating_sub(1));
        })
        .on_reorder(|s: &mut State, from, to| {
            let d = s.docs.remove(from);
            s.docs.insert(to, d);
            s.selected = to;
        })
        .on_rename(|s: &mut State, i| s.log.push(format!("rename {i}")))
        .on_new(|s: &mut State| {
            s.docs.push(Doc {
                id: s.next,
                name: format!("Doc {}", s.next),
                color: None,
                bell: false,
            });
            s.next += 1;
            s.selected = s.docs.len() - 1;
        });
    super::column(vec![tab_bar(cx, &theme, bar, &s.bar, |s: &mut State| {
        &mut s.bar
    })])
    .align(Align::Start, Align::Stretch)
    .grow(1.0)
}

fn app(n: usize, width: f64) -> App<State> {
    let mut app = App::new(State::new(n), view).logical_size(Size::new(width, 200.0));
    app.render_once();
    app
}

fn tabs_of(app: &App<State>) -> Vec<AccessNode> {
    let tree = app.accessibility_tree().unwrap();
    let list = tree
        .descendants()
        .into_iter()
        .find(|n| n.role == Role::TabList)
        .expect("a tab list")
        .clone();
    list.descendants()
        .into_iter()
        .filter(|n| n.role == Role::Tab)
        .cloned()
        .collect()
}

fn named(app: &App<State>, name: &str) -> Rect {
    app.accessibility_tree()
        .unwrap()
        .descendants()
        .into_iter()
        .find(|n| n.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
        .bounds
}

#[test]
fn tabs_select_close_and_add() {
    let mut app = app(3, 600.0);
    let tabs = tabs_of(&app);
    assert_eq!(tabs.len(), 3);
    assert!(tabs[0].selected && !tabs[1].selected);
    assert_eq!(tabs[1].name, "Doc 1");
    // Select the second tab.
    assert!(app.click_at(tabs[1].bounds.center()));
    assert_eq!(app.state().selected, 1);
    app.render_once();
    assert!(tabs_of(&app)[1].selected);
    // Its × (at the tab's right end) closes it, not selects it.
    let b = tabs_of(&app)[2].bounds;
    assert!(app.click_at(Point::new(b.max_x() - 17.0, b.center().y + 1.5)));
    assert_eq!(app.state().log, ["close 2"]);
    assert_eq!(app.state().docs.len(), 2);
    app.render_once();
    // "+" adds one, selected.
    let plus = named(&app, "New tab");
    assert!(app.click_at(plus.center()));
    assert_eq!(app.state().docs.len(), 3);
    assert_eq!(app.state().selected, 2);
}

#[test]
fn the_keyboard_moves_the_selection() {
    let mut app = app(3, 600.0);
    assert!(app.focus_next());
    app.press_key(KeyInput::Right);
    assert_eq!(app.state().selected, 1);
    app.render_once();
    app.press_key(KeyInput::End);
    assert_eq!(app.state().selected, 2);
    app.render_once();
    app.press_key(KeyInput::Right); // already the last
    assert_eq!(app.state().selected, 2);
    app.press_key(KeyInput::Home);
    assert_eq!(app.state().selected, 0);
    app.render_once();
    app.press_key(KeyInput::Delete);
    assert_eq!(app.state().log, ["close 0"]);
}

#[test]
fn dragging_a_tab_onto_another_reorders_and_slides() {
    let mut app = app(3, 600.0);
    let tabs = tabs_of(&app);
    let from = tabs[0].bounds.center();
    let to = tabs[2].bounds.center();
    app.pointer_press(from);
    app.pointer_drag_to(Point::new(from.x + 10.0, from.y));
    app.render_once();
    app.pointer_drag_to(to);
    app.render_once();
    app.pointer_release(to);
    let names: Vec<_> = app.state().docs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["Doc 1", "Doc 2", "Doc 0"]);
    assert_eq!(app.state().selected, 2);
    // The three tabs slide into their new places.
    assert_eq!(app.state().bar.slide_count(), 3);
}

#[test]
fn too_many_tabs_scroll_and_list() {
    let mut app = app(12, 520.0);
    let tabs = tabs_of(&app);
    let shown = tabs.len();
    assert!(shown < 12, "{shown}");
    assert_eq!(tabs[0].name, "Doc 0");
    // The left arrow is disabled at the start; the list picks any tab, and
    // the bar scrolls to show it.
    let list = named(&app, "All tabs");
    assert!(app.click_at(list.center()));
    assert!(app.state().bar.is_list_open());
    // Let the list finish growing in before aiming at an item.
    app.render_once();
    std::thread::sleep(std::time::Duration::from_millis(300));
    app.render_once();
    let last = named(&app, "Doc 11");
    assert!(app.click_at(last.center()));
    assert_eq!(app.state().selected, 11);
    assert!(!app.state().bar.is_list_open());
    app.render_once();
    let tabs = tabs_of(&app);
    assert_eq!(tabs.len(), shown);
    assert_eq!(tabs.last().unwrap().name, "Doc 11");
    assert!(tabs.last().unwrap().selected);
    // The left arrow scrolls by one, away from the selection.
    let first = app.state().bar.first();
    assert_eq!(first, 12 - shown);
    let left = named(&app, "Scroll tabs left");
    assert!(app.click_at(left.center()));
    assert_eq!(app.state().bar.first(), first - 1);
    app.render_once();
    assert_eq!(tabs_of(&app)[0].name, format!("Doc {}", first - 1));
    assert_eq!(app.state().selected, 11);
}

#[test]
fn a_right_click_opens_the_tab_menu() {
    let mut app = app(3, 600.0);
    let tab = tabs_of(&app)[1].bounds.center();
    assert!(app.right_click_at(tab));
    app.render_once();
    let item = named(&app, "Close Others");
    assert!(app.click_at(item.center()));
    assert_eq!(app.state().docs.len(), 1);
    assert_eq!(app.state().docs[0].name, "Doc 1");
}

#[test]
fn a_middle_click_closes_and_a_double_click_renames() {
    use crate::platform::{ButtonState, Event, PointerButton};
    let button = |button, state, at: Point| Event::PointerButton {
        button,
        state,
        position: at,
    };
    let mut app = app(3, 600.0);
    let tab = tabs_of(&app)[1].bounds.center();
    app.pointer_event(button(PointerButton::Middle, ButtonState::Pressed, tab));
    app.pointer_event(button(PointerButton::Middle, ButtonState::Released, tab));
    assert_eq!(app.state().log, ["close 1"]);
    app.render_once();
    let tab = tabs_of(&app)[0].bounds.center();
    for _ in 0..2 {
        app.pointer_event(button(PointerButton::Left, ButtonState::Pressed, tab));
        app.pointer_event(button(PointerButton::Left, ButtonState::Released, tab));
    }
    assert_eq!(app.state().log, ["close 1", "rename 0"]);
}
