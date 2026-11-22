//! Relations between nodes, and acting on a node scrolled out of view, as
//! assistive technologies see them through an [`App`]: an open select
//! controls its list, a tab bar's selected tab its panel, choice chips are
//! one radio group, a tree's items know their parents; and a screen
//! reader's activation of an item scrolled out of its list brings it into
//! view and presses that item (not what is drawn where it would be).

use crate::App;
use crate::core::{Cx, Element};
use crate::geometry::Size;
use crate::platform::Event;
use crate::platform::access::A11yNode;
use crate::widgets::{
    Select, Tab, TabBar, TabBarState, TreeModel, TreeSpec, TreeState, button, choice_chips, column,
    label, scroll, select, tab_bar, tree,
};

/// The platform tree, as a window's bridge gets it.
fn platform_tree<S: 'static>(app: &mut App<S>) -> A11yNode {
    app.render_once();
    crate::to_platform_a11y(&app.accessibility_tree().expect("a frame"))
}

/// The path to the first node `pred` picks.
fn path_to(n: &A11yNode, pred: &dyn Fn(&A11yNode) -> bool) -> Option<Vec<usize>> {
    if pred(n) {
        return Some(Vec::new());
    }
    n.children.iter().enumerate().find_map(|(i, c)| {
        path_to(c, pred).map(|mut p| {
            p.insert(0, i);
            p
        })
    })
}

fn named(tree: &A11yNode, role: crate::platform::A11yRole, name: &str) -> Vec<usize> {
    path_to(tree, &|n| n.role == role && n.name == name)
        .unwrap_or_else(|| panic!("no {role:?} {name:?} in {tree:#?}"))
}

fn deliver<S: 'static>(app: &mut App<S>, events: Vec<Event>) {
    for e in events {
        app.pointer_event(e);
    }
}

#[derive(Default)]
struct Form {
    size: Select,
    format: usize,
    tabs: TabBarState,
    tab: usize,
    folders: TreeState,
}

/// Two folders under a root, the first with a file.
struct Folders;

impl TreeModel for Folders {
    fn children(&self, parent: Option<u64>) -> Vec<u64> {
        match parent {
            None => vec![1],
            Some(1) => vec![2, 3],
            Some(2) => vec![4],
            _ => Vec::new(),
        }
    }
}

fn form(s: &Form, cx: &mut Cx<Form>) -> Element {
    const SIZES: [&str; 3] = ["Small", "Medium", "Large"];
    let bar = TabBar::new("docs", vec![Tab::new(1, "One"), Tab::new(2, "Two")])
        .selected(s.tab)
        .panel("doc-panel")
        .on_select(|s: &mut Form, i| s.tab = i);
    column(vec![
        select(cx, &SIZES, &s.size, |s| &mut s.size),
        choice_chips(cx, ["PNG", "JPEG", "WebP"], s.format, |s: &mut Form, i| {
            s.format = i;
        }),
        tab_bar(cx, bar, &s.tabs, |s| &mut s.tabs),
        label(cx, format!("Document {}", s.tab + 1)).a11y_id("doc-panel"),
        tree(
            cx,
            &s.folders,
            |s| &mut s.folders,
            TreeSpec::new(&Folders).height(200.0),
            |cx, row| label(cx, format!("Node {}", row.id)),
        ),
    ])
}

fn form_app() -> App<Form> {
    let mut state = Form::default();
    state.folders.set_expanded(1, true);
    state.folders.set_expanded(2, true);
    let mut app = App::new(state, form)
        .no_font()
        .logical_size(Size::new(600.0, 600.0));
    app.render_once();
    app
}

#[test]
fn an_open_select_controls_its_list() {
    use crate::platform::A11yRole;
    let mut app = form_app();
    let tree = platform_tree(&mut app);
    let combo = path_to(&tree, &|n| n.role == A11yRole::ComboBox).unwrap();
    assert!(tree.relations(&combo).controller_for.is_empty(), "closed");
    app.state.size.open();
    app.panes[0].dirty = true;
    let tree = platform_tree(&mut app);
    let combo = path_to(&tree, &|n| n.role == A11yRole::ComboBox).unwrap();
    let r = tree.relations(&combo);
    assert_eq!(r.controller_for.len(), 1, "{r:?}");
    let list = tree.at_path(&r.controller_for[0]).unwrap();
    assert_eq!(list.role, A11yRole::List);
    assert!(
        path_to(list, &|c| c.role == A11yRole::ListItem
            && c.name == "Medium")
        .is_some()
    );
    assert_eq!(
        tree.relations(&r.controller_for[0]).controlled_by,
        vec![combo]
    );
}

#[test]
fn the_selected_tab_controls_its_panel() {
    use crate::platform::A11yRole;
    let mut app = form_app();
    let tree = platform_tree(&mut app);
    let one = named(&tree, A11yRole::Tab, "One");
    let panel = named(&tree, A11yRole::Text, "Document 1");
    assert_eq!(tree.relations(&one).controller_for, vec![panel.clone()]);
    assert_eq!(tree.relations(&panel).controlled_by, vec![one]);
    let two = named(&tree, A11yRole::Tab, "Two");
    assert!(tree.relations(&two).controller_for.is_empty());
}

#[test]
fn choice_chips_are_one_radio_group() {
    use crate::platform::A11yRole;
    let mut app = form_app();
    let tree = platform_tree(&mut app);
    let jpeg = named(&tree, A11yRole::RadioButton, "JPEG");
    let members: Vec<String> = tree
        .relations(&jpeg)
        .member_of
        .iter()
        .map(|p| tree.at_path(p).unwrap().name.clone())
        .collect();
    assert_eq!(members, ["PNG", "JPEG", "WebP"]);
}

#[test]
fn tree_items_know_their_level_and_parent() {
    use crate::platform::A11yRole;
    let mut app = form_app();
    let tree = platform_tree(&mut app);
    let item = |name: &str| named(&tree, A11yRole::TreeItem, name);
    let (root, a, file, b) = (
        item("Node 1"),
        item("Node 2"),
        item("Node 4"),
        item("Node 3"),
    );
    assert_eq!(tree.tree_level(&root), Some(1));
    assert_eq!(tree.tree_level(&file), Some(3));
    assert_eq!(tree.relations(&root).node_child_of, None);
    assert_eq!(
        tree.relations(&root).node_parent_of,
        vec![a.clone(), b.clone()]
    );
    assert_eq!(tree.relations(&file).node_child_of, Some(a.clone()));
    assert_eq!(tree.relations(&b).node_child_of, Some(root));
}

#[derive(Default)]
struct Feed {
    tapped: Option<usize>,
}

fn feed(_: &Feed, cx: &mut Cx<Feed>) -> Element {
    let rows = (0..30)
        .map(|i| {
            button(cx, format!("Item {i}"))
                .height(40.0)
                .on_tap(cx, move |s: &mut Feed| s.tapped = Some(i))
        })
        .collect();
    column(vec![
        scroll(cx).child(column(rows)).height(200.0),
        button(cx, "Below").on_tap(cx, |s: &mut Feed| s.tapped = Some(99)),
    ])
}

#[test]
fn activating_an_item_scrolled_out_of_view_presses_it() {
    use crate::platform::A11yRole;
    let mut app = App::new(Feed::default(), feed)
        .no_font()
        .logical_size(Size::new(300.0, 400.0));
    app.render_once();
    let tree = platform_tree(&mut app);
    // "Item 20" is laid out far below the list's 200 px (where "Below", or
    // nothing, is drawn): a click at its middle would miss it.
    let item = named(&tree, A11yRole::Button, "Item 20");
    let (_, y, _, _) = tree.at_path(&item).unwrap().bounds;
    assert!(y > 200.0, "out of view: {y}");
    let mut plain = App::new(Feed::default(), feed)
        .no_font()
        .logical_size(Size::new(300.0, 400.0));
    plain.render_once();
    deliver(&mut plain, tree.at_path(&item).unwrap().click_events());
    assert_ne!(plain.state().tapped, Some(20), "a bare click misses it");
    // One delivery, as a bridge hands it over: the scroll, then the click.
    deliver(&mut app, tree.activate_events(&item).unwrap());
    assert_eq!(app.state().tapped, Some(20));
    let tree = platform_tree(&mut app);
    let (_, y, _, h) = tree.at_path(&item).unwrap().bounds;
    assert!(y >= 0.0 && y + h <= 200.0, "in view now: {y}");
    // An item in view is pressed where it is, nothing scrolling.
    let shown = named(&tree, A11yRole::Button, "Item 19");
    let before = tree.clone();
    deliver(&mut app, tree.activate_events(&shown).unwrap());
    assert_eq!(app.state().tapped, Some(19));
    let after = platform_tree(&mut app);
    assert_eq!(
        before.at_path(&shown).map(|n| n.bounds),
        after.at_path(&shown).map(|n| n.bounds)
    );
    // Focusing (a screen reader's focus on an item) scrolls it in too.
    let first = named(&after, A11yRole::Button, "Item 0");
    deliver(&mut app, after.focus_events_at(&first).unwrap());
    assert_eq!(app.state().tapped, Some(0));
}
