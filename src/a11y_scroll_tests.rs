//! Scrolling and toasts as assistive technologies see them, through an
//! [`App`]: a plain scroll container and a list report where they are
//! scrolled, and the events every platform bridge makes from a screen
//! reader's request (a page forward, a percentage, a child brought into
//! view) scroll the app's container; a toast appearing is one to announce.

use crate::App;
use crate::core::{Cx, Element, Role};
use crate::geometry::Size;
use crate::platform::Event;
use crate::platform::access::A11yNode;
use crate::widgets::{ToastKind, Toasts, button, column, scroll, toasts};

#[derive(Default)]
struct Feed {
    toasts: Toasts,
    tapped: usize,
}

fn view(s: &Feed, cx: &mut Cx<Feed>) -> Element {
    toasts(cx, &s.toasts, |s| &mut s.toasts);
    let rows = (0..30)
        .map(|i| {
            button(cx, format!("Item {i}"))
                .height(40.0)
                .on_tap(cx, move |s: &mut Feed| s.tapped = i)
        })
        .collect();
    column(vec![scroll(cx).child(column(rows)).height(200.0)])
}

fn app() -> App<Feed> {
    let mut app = App::new(Feed::default(), view)
        .no_font()
        .logical_size(Size::new(300.0, 300.0));
    app.render_once();
    app
}

/// The platform tree, as the window's bridge gets it.
fn platform_tree(app: &mut App<Feed>) -> A11yNode {
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

fn deliver(app: &mut App<Feed>, events: Vec<Event>) {
    for e in events {
        app.pointer_event(e);
    }
}

fn scroller(tree: &A11yNode) -> &A11yNode {
    let path = path_to(tree, &|n| n.scroll.is_some()).expect("a scroll container");
    tree.at_path(&path).unwrap()
}

#[test]
fn a_plain_scroll_container_scrolls_by_its_accessibility_actions() {
    let mut app = app();
    let core = app.accessibility_tree().unwrap();
    let group = core
        .descendants()
        .into_iter()
        .find(|n| n.scroll.is_some())
        .expect("the scroll container says it scrolls");
    assert_eq!(group.role, Role::Group);
    let s = group.scroll.unwrap();
    assert_eq!((s.offset, s.viewport), (0.0, 200.0));
    assert!(s.max > 900.0, "30 rows of 40 px in 200: {}", s.max);

    // A page forward (Android's ACTION_SCROLL_FORWARD, AT-SPI's action, a
    // scroll bar's increment): 80% of the viewport.
    let tree = platform_tree(&mut app);
    deliver(&mut app, scroller(&tree).scroll_page_events(true).unwrap());
    let tree = platform_tree(&mut app);
    assert_eq!(scroller(&tree).scroll.unwrap().offset, 160.0);

    // To the end (UIA's SetScrollPercent(100), a scroll bar's value of 1).
    deliver(
        &mut app,
        scroller(&tree).scroll_to_fraction_events(1.0).unwrap(),
    );
    let tree = platform_tree(&mut app);
    let s = scroller(&tree).scroll.unwrap();
    assert_eq!(s.offset, s.max);
    assert!(
        scroller(&tree).scroll_page_events(true).is_none(),
        "at the end"
    );

    // "Item 2", far above now, scrolled into view (AT-SPI's ScrollTo,
    // AXScrollToVisible, UIA's ScrollItem): the list's top shows it.
    let item = path_to(&tree, &|n| n.name == "Item 2").unwrap();
    deliver(&mut app, tree.scroll_into_view_events(&item).unwrap());
    let tree = platform_tree(&mut app);
    let (_, y, _, h) = tree.at_path(&item).unwrap().bounds;
    let (_, top, _, height) = scroller(&tree).bounds;
    assert!(
        y >= top && y + h <= top + height,
        "in view: {y} in {top}+{height}"
    );
    assert!(tree.scroll_into_view_events(&item).is_none());

    // A click on an item it shows reaches its handler.
    deliver(&mut app, tree.at_path(&item).unwrap().click_events());
    assert_eq!(app.state().tapped, 2);
}

#[test]
fn a_toast_appearing_is_the_one_to_announce() {
    let mut app = app();
    let before = platform_tree(&mut app);
    app.state.toasts.push("Saved", ToastKind::Success);
    app.panes[0].dirty = true;
    let after = platform_tree(&mut app);
    let new = after.alert_paths_new_since(Some(&before));
    assert_eq!(new.len(), 1);
    assert_eq!(new[0].1, "Saved");
    assert_eq!(
        after.at_path(&new[0].0).map(|n| n.name.as_str()),
        Some("Saved")
    );
    assert!(after.alert_paths_new_since(Some(&after)).is_empty());
}
