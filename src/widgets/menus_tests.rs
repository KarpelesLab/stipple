//! End-to-end tests of the menus Stipple draws: the in-window menu bar, its
//! keyboard navigation and shortcuts, and context menus, through `App`.

use crate::App;
use crate::core::{AccessNode, Cx, Menu, MenuBar, MenuItem, Role, Shortcut};
use crate::geometry::{Point, Size};
use crate::platform::{KeyCode, Modifiers};
use crate::widgets::{column, label};

#[derive(Default)]
struct Doc {
    log: Vec<&'static str>,
    dirty: bool,
    wrap: bool,
}

fn command() -> Modifiers {
    Modifiers {
        ctrl: !cfg!(target_os = "macos"),
        meta: cfg!(target_os = "macos"),
        ..Modifiers::default()
    }
}

fn app() -> App<Doc> {
    App::new(Doc::default(), |_: &Doc, cx: &mut Cx<Doc>| {
        let theme = *cx.theme();
        column(vec![
            label(&theme, "Body")
                .height(40.0)
                .width(200.0)
                .context_menu(
                    cx,
                    Menu::new("")
                        .item(MenuItem::new("Cut", |d: &mut Doc| d.log.push("cut")))
                        .item(
                            MenuItem::new("Paste", |d: &mut Doc| d.log.push("paste"))
                                .enabled(false),
                        )
                        .item(MenuItem::new("Copy", |d: &mut Doc| d.log.push("copy"))),
                ),
        ])
    })
    .logical_size(Size::new(400.0, 300.0))
    .menu(|doc: &Doc| {
        MenuBar::new()
            .menu(
                Menu::new("&File")
                    .item(MenuItem::new("&New", |d: &mut Doc| d.log.push("new")))
                    .item(
                        MenuItem::new("&Save", |d: &mut Doc| {
                            d.log.push("save");
                            d.dirty = false;
                        })
                        .shortcut(Shortcut::cmd('s'))
                        .enabled(doc.dirty),
                    )
                    .separator()
                    .submenu(
                        Menu::new("&Recent")
                            .item(MenuItem::new("a.txt", |d: &mut Doc| d.log.push("a")))
                            .item(MenuItem::new("b.txt", |d: &mut Doc| d.log.push("b"))),
                    )
                    .item(MenuItem::new("&Quit", |d: &mut Doc| d.log.push("quit"))),
            )
            .menu(
                Menu::new("&View").item(
                    MenuItem::new("&Wrap", |d: &mut Doc| d.wrap = !d.wrap)
                        .checked(doc.wrap)
                        .shortcut(Shortcut::key(KeyCode::F5)),
                ),
            )
            .menu(Menu::new("&Edit").item(
                MenuItem::new("&Dirty", |d: &mut Doc| d.dirty = true).shortcut(Shortcut::cmd('d')),
            ))
    })
}

fn nodes(app: &App<Doc>, role: Role) -> Vec<AccessNode> {
    let tree = app.accessibility_tree().expect("a frame was built");
    tree.descendants()
        .into_iter()
        .filter(|n| n.role == role)
        .cloned()
        .collect()
}

fn find(app: &App<Doc>, role: Role, name: &str) -> AccessNode {
    nodes(app, role)
        .into_iter()
        .find(|n| n.name == name)
        .unwrap_or_else(|| panic!("no {role:?} named {name:?}"))
}

fn click_on(app: &mut App<Doc>, role: Role, name: &str) {
    let node = find(app, role, name);
    let at = node.bounds.center();
    app.hover_at(at);
    app.click_at(at);
    app.render_once();
}

#[test]
fn the_bar_sits_above_the_content() {
    let mut app = app();
    app.render_once();
    assert_eq!(nodes(&app, Role::MenuBar).len(), 1);
    let file = find(&app, Role::MenuItem, "File");
    let view = find(&app, Role::MenuItem, "View");
    assert!(view.bounds.min_x() > file.bounds.min_x());
    let body = find(&app, Role::Text, "Body");
    assert!(
        body.bounds.min_y() >= file.bounds.max_y(),
        "content starts below the bar"
    );
    assert!(nodes(&app, Role::Menu).is_empty(), "no menu open");
}

#[test]
fn clicking_opens_a_menu_and_picking_runs_the_action() {
    let mut app = app();
    app.render_once();
    click_on(&mut app, Role::MenuItem, "File");
    let menu = find(&app, Role::Menu, "File");
    let file = find(&app, Role::MenuItem, "File");
    assert!(
        menu.bounds.min_y() >= file.bounds.max_y(),
        "drops below the title"
    );
    // Save is disabled (nothing to save): clicking it does nothing.
    click_on(&mut app, Role::MenuItem, "Save");
    assert!(app.state().log.is_empty());
    assert_eq!(nodes(&app, Role::Menu).len(), 1, "still open");
    click_on(&mut app, Role::MenuItem, "New");
    assert_eq!(app.state().log, vec!["new"]);
    assert!(nodes(&app, Role::Menu).is_empty(), "closed after the pick");
}

#[test]
fn hovering_switches_menus_and_opens_submenus() {
    let mut app = app();
    app.render_once();
    click_on(&mut app, Role::MenuItem, "File");
    let recent = find(&app, Role::MenuItem, "Recent");
    app.hover_at(recent.bounds.center());
    app.render_once();
    assert_eq!(nodes(&app, Role::Menu).len(), 2, "the submenu opened");
    let sub = find(&app, Role::Menu, "Recent");
    assert!(
        sub.bounds.min_x() >= recent.bounds.max_x() - 1.0,
        "beside its item"
    );
    click_on(&mut app, Role::MenuItem, "b.txt");
    assert_eq!(app.state().log, vec!["b"]);
    assert!(nodes(&app, Role::Menu).is_empty());

    click_on(&mut app, Role::MenuItem, "File");
    app.hover_at(find(&app, Role::MenuItem, "View").bounds.center());
    app.render_once();
    find(&app, Role::Menu, "View");
    // A click on its title closes it; so does a press outside.
    click_on(&mut app, Role::MenuItem, "View");
    assert!(nodes(&app, Role::Menu).is_empty());
    click_on(&mut app, Role::MenuItem, "View");
    find(&app, Role::Menu, "View");
    app.click_at(Point::new(390.0, 290.0));
    app.render_once();
    assert!(nodes(&app, Role::Menu).is_empty());
}

#[test]
fn shortcuts_run_enabled_commands_only() {
    let mut app = app();
    app.render_once();
    assert!(
        !app.press_key_code(KeyCode::Char('s'), command()),
        "Save is disabled"
    );
    assert!(app.press_key_code(KeyCode::Char('d'), command()));
    assert!(app.state().dirty);
    // Save follows the state: now enabled, and the shortcut takes either case.
    assert!(app.press_key_code(KeyCode::Char('S'), command()));
    assert_eq!(app.state().log, vec!["save"]);
    assert!(!app.state().dirty);
    assert!(app.press_key_code(KeyCode::F5, Modifiers::default()));
    assert!(app.state().wrap);
    // Native ids reach the same commands: 5 is Quit.
    assert!(app.menu_command(5));
    assert_eq!(app.state().log, vec!["save", "quit"]);
    assert!(!app.menu_command(2), "Save is disabled again");
}

#[test]
fn keyboard_drives_the_bar() {
    let mut app = app();
    app.render_once();
    let none = Modifiers::default();
    // F10 puts the keyboard on the bar; Down opens File on its first item.
    assert!(app.press_key_code(KeyCode::F10, none));
    app.render_once();
    assert!(app.press_key_code(KeyCode::ArrowDown, none));
    app.render_once();
    find(&app, Role::Menu, "File");
    // Down skips the disabled Save and the separator, to Recent; Right opens it.
    app.press_key_code(KeyCode::ArrowDown, none);
    app.press_key_code(KeyCode::ArrowRight, none);
    app.render_once();
    find(&app, Role::Menu, "Recent");
    app.press_key_code(KeyCode::ArrowDown, none);
    app.press_key_code(KeyCode::Enter, none);
    assert_eq!(app.state().log, vec!["b"]);
    app.render_once();
    assert!(nodes(&app, Role::Menu).is_empty());

    // Alt on its own focuses the bar; Right then Enter opens View.
    app.press_key_code(KeyCode::Alt, none);
    assert!(app.release_key_code(KeyCode::Alt));
    app.render_once();
    app.press_key_code(KeyCode::ArrowRight, none);
    app.press_key_code(KeyCode::Enter, none);
    app.render_once();
    find(&app, Role::Menu, "View");
    // Left moves to File; typing a mnemonic picks.
    app.press_key_code(KeyCode::ArrowLeft, none);
    app.render_once();
    find(&app, Role::Menu, "File");
    app.press_key_code(KeyCode::Char('q'), none);
    assert_eq!(app.state().log, vec!["b", "quit"]);

    // Alt+F opens File; Escape steps back to the bar, then leaves it.
    let alt = Modifiers { alt: true, ..none };
    app.render_once();
    app.press_key_code(KeyCode::Alt, none);
    assert!(app.press_key_code(KeyCode::Char('f'), alt));
    assert!(
        !app.release_key_code(KeyCode::Alt),
        "Alt was used for Alt+F"
    );
    app.render_once();
    find(&app, Role::Menu, "File");
    app.press_key_code(KeyCode::Escape, none);
    app.render_once();
    assert!(nodes(&app, Role::Menu).is_empty());
    assert!(
        app.press_key_code(KeyCode::Escape, none),
        "still on the bar"
    );
    assert!(!app.press_key_code(KeyCode::Escape, none), "left the bar");
}

#[test]
fn a_right_click_opens_the_context_menu_at_the_pointer() {
    let mut app = app();
    app.render_once();
    let body = find(&app, Role::Text, "Body");
    let at = Point::new(body.bounds.min_x() + 30.0, body.bounds.center().y);
    assert!(app.right_click_at(at));
    app.render_once();
    let menu = &nodes(&app, Role::Menu)[0];
    assert_eq!((menu.bounds.min_x(), menu.bounds.min_y()), (at.x, at.y));
    click_on(&mut app, Role::MenuItem, "Copy");
    assert_eq!(app.state().log, vec!["copy"]);
    assert!(nodes(&app, Role::Menu).is_empty());

    // From the keyboard, at the pointer: Down skips the disabled Paste.
    app.hover_at(at);
    assert!(app.press_key_code(KeyCode::ContextMenu, Modifiers::default()));
    app.render_once();
    app.press_key_code(KeyCode::ArrowDown, Modifiers::default());
    app.press_key_code(KeyCode::Enter, Modifiers::default());
    assert_eq!(app.state().log, vec!["copy", "copy"]);

    // Shift+F10 too; Escape closes.
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    assert!(app.press_key_code(KeyCode::F10, shift));
    app.render_once();
    assert_eq!(nodes(&app, Role::Menu).len(), 1);
    app.press_key_code(KeyCode::Escape, Modifiers::default());
    app.render_once();
    assert!(nodes(&app, Role::Menu).is_empty());
}

#[test]
fn a_context_menu_near_the_edge_stays_inside_the_window() {
    let mut app = app().logical_size(Size::new(240.0, 300.0));
    app.render_once();
    let body = find(&app, Role::Text, "Body");
    let at = Point::new(body.bounds.max_x() - 2.0, body.bounds.max_y() - 2.0);
    app.right_click_at(at);
    app.render_once();
    let menu = &nodes(&app, Role::Menu)[0];
    assert!(menu.bounds.max_x() <= 240.0 + 0.5, "{:?}", menu.bounds);
    assert!(menu.bounds.min_x() >= 0.0);
}
