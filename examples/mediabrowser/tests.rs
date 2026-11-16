//! App-level tests: the accessibility tree (roles, names and states of the
//! tree, the table, the grid, the buttons and the player's slider), the
//! keyboard, and the languages, on the in-memory sample.

use super::selftest::{row_names, settle};
use super::*;
use stipple::core::AccessNode;

fn app_in(language: &str) -> App<State> {
    let vfs = sample_vfs();
    let settings = Settings {
        language: Some(language.into()),
        width: 1100.0,
        height: 700.0,
        ..Settings::default()
    };
    // No worker threads: the tests pump the loader themselves.
    let loader = Loader::new(vfs.clone(), 1);
    let mut state = State::new(vfs, loader, settings);
    state.audio = Some(Audio::offline(48_000));
    let mut app = build_app(state).theme(Theme::light());
    settle(&mut app, Duration::from_secs(5));
    app
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

#[test]
fn the_access_tree_has_roles_names_and_states() {
    let mut app = app_in("en");
    // The panes, named.
    assert!(app.find(Role::Tree, "Folders").is_some());
    assert!(app.find(Role::Table, "Files").is_some());
    assert!(app.find(Role::Group, "Preview").is_some());
    assert!(app.find(Role::Toolbar, "Location").is_some());
    // The tree: the root expanded and selected, its folders as items.
    let items = nodes(&app, Role::TreeItem);
    let root = items
        .iter()
        .find(|n| n.name == "Samples")
        .expect("the root");
    assert!(root.selected);
    assert_eq!(root.expanded, Some(true));
    for name in ["Documents", "Pictures"] {
        let item = items.iter().find(|n| n.name == name).expect(name);
        assert_eq!(
            item.expanded,
            Some(false),
            "{name} has subfolders, collapsed"
        );
    }
    let music = items.iter().find(|n| n.name == "Music").expect("Music");
    assert_eq!(music.expanded, None, "Music has no subfolders");
    // The table: four sortable headers and a row per entry.
    let headers: Vec<String> = nodes(&app, Role::ColumnHeader)
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert_eq!(headers, ["Name", "Size", "Kind", "Modified"]);
    assert_eq!(
        row_names(&mut app),
        ["Documents", "Music", "Pictures", "README.md"]
    );
    // Buttons and the view switch.
    assert!(app.find(Role::Button, "Up").is_some());
    assert!(app.find(Role::Button, "Refresh").is_some());
    // The menu bar, where it's drawn.
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        assert!(app.find(Role::MenuItem, "View").is_some());
    }

    // Select a row: it reports selected, the preview follows.
    let cell = nodes(&app, Role::Cell)
        .into_iter()
        .find(|n| n.name == "README.md")
        .unwrap();
    assert!(app.click_at(cell.bounds.center()));
    settle(&mut app, Duration::from_secs(5));
    let selected: Vec<String> = nodes(&app, Role::Row)
        .into_iter()
        .filter(|n| n.selected)
        .map(|n| n.children[0].name.clone())
        .collect();
    assert_eq!(selected, ["README.md"]);
    assert!(app.find_text("Media browser sample").is_some());
    assert!(app.state().table.rows.cursor().is_some());
    // The table holds the keyboard focus.
    assert!(nodes(&app, Role::Table)[0].focused);
}

#[test]
fn the_player_is_reachable_and_its_slider_named() {
    let mut app = app_in("en");
    app.handle().update(|s: &mut State| {
        s.open_dir("Music");
        let i = s
            .entries()
            .iter()
            .position(|e| e.name == "chime.wav")
            .unwrap();
        s.table.rows.select(i);
    });
    settle(&mut app, Duration::from_secs(5));
    assert!(app.find(Role::Button, "Play").is_some());
    assert!(app.find(Role::Button, "Stop").is_some());
    let slider = nodes(&app, Role::Slider);
    assert_eq!(slider.len(), 1);
    assert_eq!(slider[0].name, "Position, 0:00 of 0:01");
    // Play from the keyboard: Tab to the Play button, Enter... or click.
    assert!(app.click(Role::Button, "Play"));
    settle(&mut app, Duration::from_secs(1));
    assert!(app.state().player.is_some());
    assert!(app.find(Role::Button, "Pause").is_some());
    // The slider takes focus and its keys seek.
    let slider = &nodes(&app, Role::Slider)[0];
    assert!(app.click_at(slider.bounds.center()));
    app.end_drag();
    assert!(app.press_key(KeyInput::End));
    settle(&mut app, Duration::from_secs(1));
    let at = app.state().player.as_ref().map(|p| p.voice.position());
    assert!(at.is_some_and(|d| d.as_secs_f64() > 1.4), "{at:?}");
}

#[test]
fn every_control_is_reachable_with_tab() {
    let mut app = app_in("en");
    let mut seen = Vec::new();
    for _ in 0..40 {
        if !app.focus_next() {
            break;
        }
        let _ = app.render_frame();
        let focused: Vec<(Role, String)> = app
            .accessibility_tree()
            .unwrap()
            .descendants()
            .into_iter()
            .filter(|n| n.focused)
            .map(|n| (n.role, n.name.clone()))
            .collect();
        seen.extend(focused);
    }
    for (role, name) in [
        (Role::Button, "Up"),
        (Role::Tree, "Folders"),
        (Role::Table, "Files"),
    ] {
        assert!(
            seen.iter().any(|(r, n)| *r == role && n == name),
            "{role:?} {name:?} not reached by Tab: {seen:?}"
        );
    }
}

#[test]
fn keyboard_navigation_type_to_select_and_enter() {
    let mut app = app_in("en");
    let cell = nodes(&app, Role::Cell)
        .into_iter()
        .find(|n| n.name == "Documents")
        .unwrap();
    assert!(app.click_at(cell.bounds.center()));
    // Typing "p" jumps to Pictures; Enter opens it.
    assert!(app.press_key(KeyInput::Text("p".into())));
    assert_eq!(app.state().current().unwrap().name, "Pictures");
    assert!(app.press_key(KeyInput::Enter));
    settle(&mut app, Duration::from_secs(5));
    assert_eq!(app.state().cwd(), "Pictures");
    assert_eq!(
        row_names(&mut app),
        ["Holidays", "Tiles", "checker.bmp", "sunset.png"]
    );
    // Backspace goes back up, with Pictures selected.
    let cell = nodes(&app, Role::Cell)
        .into_iter()
        .find(|n| n.name == "sunset.png")
        .unwrap();
    assert!(app.click_at(cell.bounds.center()));
    assert!(app.press_key(KeyInput::Backspace));
    settle(&mut app, Duration::from_secs(1));
    assert_eq!(app.state().cwd(), "");
    assert_eq!(app.state().current().unwrap().name, "Pictures");
}

#[test]
fn grid_tiles_are_list_items_and_thumbnails_load() {
    let mut app = app_in("en");
    app.handle().update(|s: &mut State| {
        s.set_mode(Mode::Grid);
        s.open_dir("Pictures/Tiles");
    });
    settle(&mut app, Duration::from_secs(10));
    assert!(app.find(Role::List, "Files").is_some());
    let tiles = nodes(&app, Role::ListItem);
    assert!(
        tiles.len() >= 8 && tiles.len() < 48,
        "{} tiles",
        tiles.len()
    );
    assert_eq!(tiles[0].name, "tile-00.png");
    let stats = app.state().loader.stats();
    assert!(stats.decoded >= 8, "{stats:?}");
    // Arrow keys move through the grid.
    assert!(app.click_at(tiles[0].bounds.center()));
    assert!(app.press_key(KeyInput::Right));
    assert_eq!(app.state().current().unwrap().name, "tile-01.png");
    assert!(app.press_key(KeyInput::SelectDown));
    assert!(app.state().chosen().len() > 2);
}

#[test]
fn french_and_arabic_translate_and_arabic_mirrors() {
    let mut fr = app_in("fr");
    assert!(fr.find(Role::Tree, "Dossiers").is_some());
    let headers: Vec<String> = nodes(&fr, Role::ColumnHeader)
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert_eq!(headers, ["Nom", "Taille", "Type", "Modifié"]);
    assert!(fr.find_text("4 éléments · 0 sélectionné").is_some());

    let ar = app_in("ar");
    let tree = &nodes(&ar, Role::Tree)[0];
    let table = &nodes(&ar, Role::Table)[0];
    assert_eq!(tree.name, "المجلدات");
    assert!(
        tree.bounds.min_x() > table.bounds.min_x(),
        "the tree is on the right"
    );
    let headers: Vec<String> = nodes(&ar, Role::ColumnHeader)
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert_eq!(headers, ["الاسم", "الحجم", "النوع", "آخر تعديل"]);
    // The first column is at the right edge.
    let h = nodes(&ar, Role::ColumnHeader);
    assert!(h[0].bounds.min_x() > h[1].bounds.min_x());
}
