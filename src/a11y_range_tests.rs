//! Range values, tables, labels and text attributes as assistive
//! technologies see them, through an [`App`]: a slider, a progress bar and
//! a number input report their ranges, and the events every platform bridge
//! makes from a screen reader's "set the value" or "increment" set them; a
//! virtualized table numbers its rows and cells among all of its rows; a
//! field names its caption; text says how it is drawn.

use crate::App;
use crate::core::{AccessRange, AccessTable, Cx, Element, KeyInput, Role};
use crate::geometry::Size;
use crate::layout::LayoutDirection;
use crate::platform::Event;
use crate::platform::access::{A11yNode, A11yRole};
use crate::platform::{ButtonState, Ime};
use crate::widgets::{
    Column, NumberInput, SelectionMode, TableSpec, TableState, column, label, number_input,
    progress_bar, slider, table,
};

struct Form {
    volume: f64,
    level: NumberInput,
    table: TableState,
    rtl: bool,
}

fn view(s: &Form, cx: &mut Cx<Form>) -> Element {
    let cols = [Column::new("Name", 100.0), Column::new("Value", 80.0)];
    column(vec![
        slider(cx, s.volume, |s: &mut Form, v| s.volume = v)
            .a11y_label("Volume")
            .width(200.0),
        progress_bar(cx, s.volume),
        label(cx, "Level").a11y_id("level"),
        number_input(cx, &s.level, |s| &mut s.level).labelled_by("level"),
        table(
            cx,
            &s.table,
            |s| &mut s.table,
            TableSpec::new(&cols, 50).row_height(20.0).height(100.0),
            |cx, row, col| label(cx, format!("r{row}c{col}")),
        ),
    ])
    .gap(4.0)
    .layout_direction(if s.rtl {
        LayoutDirection::Rtl
    } else {
        LayoutDirection::Ltr
    })
}

fn app(rtl: bool) -> App<Form> {
    let form = Form {
        volume: 0.5,
        level: NumberInput::new(3.0).range(0.0, 10.0),
        table: TableState::new(SelectionMode::Single),
        rtl,
    };
    let mut app = App::new(form, view)
        .no_font()
        .logical_size(Size::new(400.0, 400.0));
    app.render_once();
    app
}

/// The platform tree, as the window's bridge gets it.
fn platform_tree(app: &mut App<Form>) -> A11yNode {
    app.render_once();
    crate::to_platform_a11y(&app.accessibility_tree().expect("a frame"))
}

fn find(n: &A11yNode, role: A11yRole) -> &A11yNode {
    fn go(n: &A11yNode, role: A11yRole) -> Option<&A11yNode> {
        if n.role == role {
            return Some(n);
        }
        n.children.iter().find_map(|c| go(c, role))
    }
    go(n, role).unwrap_or_else(|| panic!("no {role:?}"))
}

/// Hand a bridge's events to the app, as a window's event loop does.
fn deliver(app: &mut App<Form>, events: Vec<Event>) {
    for e in events {
        match e {
            Event::Ime(Ime::Commit(text)) => {
                app.press_key(KeyInput::Text(text));
            }
            Event::Ime(Ime::DeleteSurrounding { before, after }) => {
                app.press_key(KeyInput::DeleteSurrounding { before, after });
            }
            Event::Key {
                code,
                state: ButtonState::Pressed,
                modifiers,
            } => {
                app.press_key_code(code, modifiers);
            }
            Event::Key { .. } => {}
            e => {
                app.pointer_event(e);
            }
        }
    }
}

#[test]
fn sliders_progress_bars_and_number_inputs_report_their_ranges() {
    let app = app(false);
    let tree = app.accessibility_tree().unwrap();
    let all = tree.descendants();
    let slider = all.iter().find(|n| n.role == Role::Slider).unwrap();
    assert_eq!(slider.name, "Volume");
    assert_eq!(
        slider.range,
        Some(AccessRange::new(0.5, 0.0, 1.0).step(0.05))
    );
    let bar = all.iter().find(|n| n.role == Role::ProgressBar).unwrap();
    assert_eq!(bar.range.map(|r| r.value), Some(0.5));
    let spin = all.iter().find(|n| n.role == Role::SpinButton).unwrap();
    assert_eq!(spin.range, Some(AccessRange::new(3.0, 0.0, 10.0).step(1.0)));
    assert_eq!(spin.labelled_by.as_deref(), Some("level"));
}

#[test]
fn a_screen_reader_sets_a_slider_where_the_value_lies() {
    for rtl in [false, true] {
        let mut app = app(rtl);
        let tree = platform_tree(&mut app);
        let s = find(&tree, A11yRole::Slider);
        assert!(s.range_settable());
        assert_eq!(s.range.unwrap().rtl, rtl);
        deliver(&mut app, s.set_range_events(0.75).unwrap());
        assert!(
            (app.state().volume - 0.75).abs() < 1e-9,
            "rtl {rtl}: {}",
            app.state().volume
        );
        // To the very end, exactly.
        let tree = platform_tree(&mut app);
        deliver(
            &mut app,
            find(&tree, A11yRole::Slider).set_range_events(1.0).unwrap(),
        );
        assert_eq!(app.state().volume, 1.0, "rtl {rtl}");
        let tree = platform_tree(&mut app);
        let s = find(&tree, A11yRole::Slider);
        assert!(s.step_range_events(true).is_none(), "at the end");
        deliver(&mut app, s.step_range_events(false).unwrap());
        assert!((app.state().volume - 0.95).abs() < 1e-9, "a step down");
    }
}

#[test]
fn a_progress_bar_is_read_only_and_a_number_input_takes_the_number() {
    let mut app = app(false);
    let tree = platform_tree(&mut app);
    let bar = find(&tree, A11yRole::ProgressBar);
    assert!(!bar.range_settable());
    assert!(bar.set_range_events(0.2).is_none());

    let spin = find(&tree, A11yRole::SpinButton);
    deliver(&mut app, spin.set_range_events(7.0).unwrap());
    assert_eq!(app.state().level.value(), 7.0);
    assert_eq!(app.state().level.text(), "7");
    let tree = platform_tree(&mut app);
    let spin = find(&tree, A11yRole::SpinButton);
    assert_eq!(spin.range.map(|r| r.value), Some(7.0));
    deliver(&mut app, spin.step_range_events(true).unwrap());
    assert_eq!(app.state().level.value(), 8.0);
    // Past the range: clamped to it.
    let tree = platform_tree(&mut app);
    deliver(
        &mut app,
        find(&tree, A11yRole::SpinButton)
            .set_range_events(42.0)
            .unwrap(),
    );
    assert_eq!(app.state().level.value(), 10.0);
}

#[test]
fn a_virtualized_table_numbers_its_rows_among_all_of_them() {
    let mut app = app(false);
    let tree = platform_tree(&mut app);
    let t = find(&tree, A11yRole::Table);
    assert_eq!(t.table.map(|t| (t.rows, t.columns)), Some((51, 2)));
    let headers = t.column_headers();
    assert_eq!(headers.iter().map(|(c, _)| *c).collect::<Vec<_>>(), [0, 1]);
    let (_, name) = t.table_item(0, Some(1)).unwrap();
    assert_eq!(name.name, "Value");
    let (_, first) = t.table_item(1, Some(0)).unwrap();
    assert_eq!(first.name, "r0c0");
    assert!(t.table_item(40, Some(0)).is_none(), "not built");

    // Scrolled down, the rows built are numbered where they are.
    let events = t.scroll_by_events(600.0).unwrap();
    deliver(&mut app, events);
    let tree = platform_tree(&mut app);
    let t = find(&tree, A11yRole::Table);
    let (path, cell) = t.table_item(31, Some(1)).expect("row 31 built");
    assert_eq!(cell.name, "r30c1");
    assert_eq!(cell.cell.map(|c| (c.row, c.column)), Some((31, 1)));
    // Its table is above it.
    let at = crate::a11y_range_tests::path_of(&tree, t);
    let mut full = at.clone();
    full.extend(path);
    assert_eq!(tree.table_above(&full), Some(at));
}

/// The path from `root` to `node` (by address).
fn path_of(root: &A11yNode, node: &A11yNode) -> Vec<usize> {
    fn go(n: &A11yNode, want: *const A11yNode, path: &mut Vec<usize>) -> bool {
        if std::ptr::eq(n, want) {
            return true;
        }
        for (i, c) in n.children.iter().enumerate() {
            path.push(i);
            if go(c, want, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = Vec::new();
    assert!(go(root, node, &mut path));
    path
}

#[test]
fn a_table_built_by_hand_counts_its_rows_and_cells() {
    let row = |cells: &[&str]| {
        Element::stack(
            crate::core::Axis::Horizontal,
            cells
                .iter()
                .map(|c| {
                    Element::text(*c, 14.0, crate::render::Color::BLACK)
                        .width(40.0)
                        .role(Role::Cell)
                })
                .collect(),
        )
        .role(Role::Row)
    };
    let app = App::new((), move |_: &(), _: &mut Cx<()>| {
        column(vec![
            column(vec![row(&["a", "b", "c"]), row(&["d", "e", "f"])]).role(Role::Table),
            column(vec![row(&["x"])])
                .role(Role::Table)
                .a11y_table(AccessTable::new(9, 4)),
        ])
    })
    .no_font()
    .logical_size(Size::new(300.0, 200.0));
    let mut app = app;
    app.render_once();
    let tree = app.accessibility_tree().unwrap();
    let tables: Vec<_> = tree
        .descendants()
        .into_iter()
        .filter(|n| n.role == Role::Table)
        .collect();
    assert_eq!(tables[0].table, Some(AccessTable::new(2, 3)));
    assert_eq!(tables[1].table, Some(AccessTable::new(9, 4)), "declared");
    let e = tables[0]
        .descendants()
        .into_iter()
        .find(|n| n.name == "e")
        .unwrap();
    assert_eq!(e.cell.map(|c| (c.row, c.column)), Some((1, 1)));
}

#[test]
fn a_field_names_its_caption_and_text_says_how_it_is_drawn() {
    let mut app = app(false);
    let tree = platform_tree(&mut app);
    let spin = find(&tree, A11yRole::SpinButton);
    let at = tree
        .find_paths(&|n| n.role == A11yRole::SpinButton)
        .remove(0);
    let caption = tree.relations(&at).labelled_by.remove(0);
    assert!(spin.labelled_by.is_some());
    let label = tree.at_path(&caption).unwrap();
    assert_eq!(label.name, "Level");
    let font = label.font.as_ref().expect("a label's font");
    assert_eq!((font.weight, font.italic), (400, false));
    assert!(font.size > 0.0);
    // The app's font fills in a family the text doesn't name.
    let mut t = tree.clone();
    t.fill_font_family("Inter");
    let l = t.at_path(&caption).unwrap();
    assert_eq!(l.font.as_ref().unwrap().family.as_deref(), Some("Inter"));
}
