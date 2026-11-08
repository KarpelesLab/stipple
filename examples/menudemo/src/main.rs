//! Menus: a menu bar with shortcuts, a submenu, checkable and disabled items,
//! and a context menu on the canvas.
//!
//! On macOS the menu bar is the system one (with the standard application
//! menu in front), on Windows the window's native menu bar, and on Linux a bar
//! Stipple draws across the top of the window: click a title, or press Alt or
//! F10 and use the arrow keys, Enter and Escape, or Alt plus an underlined
//! letter. Shortcuts (Ctrl+D adds an item, Ctrl+S saves…) work everywhere.
//! Right-click the canvas, or press the Menu key or Shift+F10 over it, for its
//! context menu.
//!
//! Pass `dark` for the dark theme. CI (X11/Xvfb) opens the File menu from the
//! keyboard and with a click, adds items with the shortcut, opens the context
//! menu with a right-click and picks from it, screenshotting each step.

use stipple::prelude::*;

const W: f64 = 640.0;
const H: f64 = 420.0;

const COLORS: [(&str, Color); 3] = [
    ("&Blue", Color::rgb(0x3b, 0x6e, 0xd8)),
    ("&Green", Color::rgb(0x2f, 0x9e, 0x5b)),
    ("&Orange", Color::rgb(0xe0, 0x7b, 0x26)),
];
const ZOOMS: [u32; 3] = [100, 125, 150];

struct State {
    items: Vec<String>,
    /// How many items were there at the last save.
    saved: usize,
    next: usize,
    color: usize,
    zoom: usize,
    grid: bool,
    status: String,
}

impl State {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            saved: 0,
            next: 1,
            color: 0,
            zoom: 0,
            grid: true,
            status: "Ready".into(),
        }
    }

    fn dirty(&self) -> bool {
        self.items.len() != self.saved
    }

    fn add(&mut self) {
        self.items.push(format!("Item {}", self.next));
        self.next += 1;
        self.status = format!("Added item {}", self.next - 1);
    }

    fn clear(&mut self) {
        self.items.clear();
        self.status = "Cleared".into();
    }
}

/// The colors, as a submenu with the current one checked.
fn color_menu(state: &State) -> Menu<State> {
    COLORS
        .iter()
        .enumerate()
        .fold(Menu::new("&Color"), |menu, (i, (name, _))| {
            menu.item(
                MenuItem::new(*name, move |s: &mut State| {
                    s.color = i;
                    s.status = format!("Color: {}", COLORS[i].0.trim_start_matches('&'));
                })
                .checked(state.color == i),
            )
        })
}

fn menu_bar(state: &State, handle: &AppHandle<State>) -> MenuBar<State> {
    let quit = handle.clone();
    let zoom = ZOOMS
        .iter()
        .enumerate()
        .fold(Menu::new("&Zoom"), |menu, (i, pct)| {
            menu.item(
                MenuItem::new(format!("{pct}%"), move |s: &mut State| {
                    s.zoom = i;
                    s.status = format!("Zoom {}%", ZOOMS[i]);
                })
                .checked(state.zoom == i),
            )
        });
    MenuBar::new()
        .menu(
            Menu::new("&File")
                .item(
                    MenuItem::new("&New", |s: &mut State| {
                        s.clear();
                        s.saved = 0;
                        s.status = "New document".into();
                    })
                    .shortcut(Shortcut::cmd('n')),
                )
                .item(
                    MenuItem::new("&Save", |s: &mut State| {
                        s.saved = s.items.len();
                        s.status = format!("Saved {} items", s.saved);
                    })
                    .shortcut(Shortcut::cmd('s'))
                    .enabled(state.dirty()),
                )
                .submenu(
                    Menu::new("Open &Recent")
                        .item(MenuItem::new("notes.txt", |s: &mut State| {
                            s.status = "Opened notes.txt".into()
                        }))
                        .item(MenuItem::new("todo.txt", |s: &mut State| {
                            s.status = "Opened todo.txt".into()
                        })),
                )
                .separator()
                .item(
                    MenuItem::new("&Quit", move |_: &mut State| quit.exit())
                        .shortcut(Shortcut::cmd('q')),
                ),
        )
        .menu(
            Menu::new("&Edit")
                .item(MenuItem::new("&Add Item", State::add).shortcut(Shortcut::cmd('d')))
                .item(
                    MenuItem::new("&Remove Last", |s: &mut State| {
                        s.items.pop();
                        s.status = "Removed the last item".into();
                    })
                    .shortcut(Shortcut::key(KeyCode::Delete))
                    .enabled(!state.items.is_empty()),
                )
                .item(
                    MenuItem::new("C&lear", State::clear)
                        .shortcut(Shortcut::cmd_key(KeyCode::Delete))
                        .enabled(!state.items.is_empty()),
                ),
        )
        .menu(
            Menu::new("&View")
                .item(
                    MenuItem::new("Show &Grid", |s: &mut State| {
                        s.grid = !s.grid;
                        s.status = format!("Grid {}", if s.grid { "on" } else { "off" });
                    })
                    .shortcut(Shortcut::cmd('g'))
                    .checked(state.grid),
                )
                .separator()
                .submenu(zoom)
                .submenu(color_menu(state)),
        )
        .menu(
            Menu::new("&Help").item(MenuItem::new("&About Menus", |s: &mut State| {
                s.status = "Stipple menus demo".into()
            })),
        )
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let p = theme.palette;
    let scale = ZOOMS[state.zoom] as f64 / 100.0;
    let chips: Vec<Element> = state
        .items
        .iter()
        .map(|item| {
            Element::stack(
                Axis::Horizontal,
                vec![Element::text(
                    item.clone(),
                    theme.typography.body * scale,
                    Color::WHITE,
                )],
            )
            .fill(Color::rgba(0, 0, 0, 0x40))
            .radius(theme.radius)
            .padding(Insets::symmetric(theme.spacing.sm, theme.spacing.xs))
        })
        .collect();
    let mut canvas = Element::stack(Axis::Horizontal, chips)
        .wrap()
        .gap(theme.spacing.sm)
        .padding(Insets::uniform(theme.spacing.md))
        .fill(COLORS[state.color].1)
        .radius(theme.radius)
        .grow(1.0);
    if state.grid {
        canvas = canvas.border(Color::rgba(255, 255, 255, 0x90), 2.0);
    }
    let canvas = canvas.context_menu(
        cx,
        Menu::new("Canvas")
            .item(MenuItem::new("&Add Item", State::add).shortcut(Shortcut::cmd('d')))
            .item(MenuItem::new("C&lear", State::clear).enabled(!state.items.is_empty()))
            .separator()
            .submenu(color_menu(state)),
    );
    let saved = if state.dirty() { "unsaved" } else { "saved" };
    column(vec![
        heading(&theme, "Menus"),
        Element::text(
            format!(
                "{} \u{2014} {} items, {saved}",
                state.status,
                state.items.len()
            ),
            theme.typography.body,
            p.text_muted,
        ),
        paragraph(
            &theme,
            "Use the menu bar or its shortcuts, or right-click the canvas.",
        ),
        canvas,
    ])
    .gap(theme.spacing.md)
    .padding(Insets::uniform(theme.spacing.lg))
    .align(Align::Start, Align::Stretch)
    .fill(p.background)
}

fn build() -> App<State> {
    let mut app = App::new(State::new(), view)
        .title("Stipple Menus")
        .logical_size(Size::new(W, H));
    let handle = app.handle();
    app = app.menu(move |s: &State| menu_bar(s, &handle));
    if std::env::args().nth(1).as_deref() == Some("dark") {
        app = app.theme(Theme::dark());
    }
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app
}

fn main() {
    build().run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use stipple::core::Role;

    /// Where CI (visual.yml) clicks: the File title and the canvas.
    const FILE_AT: Point = Point::new(24.0, 14.0);
    const CANVAS_AT: Point = Point::new(320.0, 300.0);

    fn find(app: &App<State>, role: Role, name: &str) -> Option<Rect> {
        let tree = app.accessibility_tree()?;
        tree.descendants()
            .into_iter()
            .find(|n| n.role == role && n.name == name)
            .map(|n| n.bounds)
    }

    #[test]
    fn the_targets_sit_where_ci_clicks() {
        let mut app = build();
        if Font::system_default().is_none() {
            return; // positions depend on text metrics
        }
        app.render_once();
        let file = find(&app, Role::MenuItem, "File").expect("the drawn menu bar");
        assert!(file.contains(FILE_AT), "{file:?}");
        assert!(app.click_at(FILE_AT));
        app.render_once();
        assert!(find(&app, Role::Menu, "File").is_some());
        app.press_key_code(KeyCode::Escape, Modifiers::default());
        app.press_key_code(KeyCode::Escape, Modifiers::default());

        assert!(app.right_click_at(CANVAS_AT));
        app.render_once();
        assert!(find(&app, Role::Menu, "Canvas").is_some());
        // CI picks "Clear" with Down, Down, Enter after adding two items.
        app.press_key_code(KeyCode::Escape, Modifiers::default());
        let cmd = Modifiers {
            ctrl: !cfg!(target_os = "macos"),
            meta: cfg!(target_os = "macos"),
            ..Modifiers::default()
        };
        app.press_key_code(KeyCode::Char('d'), cmd);
        app.press_key_code(KeyCode::Char('d'), cmd);
        assert_eq!(app.state().items.len(), 2);
        app.render_once();
        app.right_click_at(CANVAS_AT);
        app.render_once();
        app.press_key_code(KeyCode::ArrowDown, Modifiers::default());
        app.press_key_code(KeyCode::ArrowDown, Modifiers::default());
        app.press_key_code(KeyCode::Enter, Modifiers::default());
        assert!(app.state().items.is_empty());
        assert_eq!(app.state().status, "Cleared");
    }

    #[test]
    fn save_follows_the_state() {
        let mut app = build();
        app.render_once();
        let cmd = Modifiers {
            ctrl: !cfg!(target_os = "macos"),
            meta: cfg!(target_os = "macos"),
            ..Modifiers::default()
        };
        assert!(
            !app.press_key_code(KeyCode::Char('s'), cmd),
            "nothing to save"
        );
        app.press_key_code(KeyCode::Char('d'), cmd);
        assert!(app.press_key_code(KeyCode::Char('s'), cmd));
        assert_eq!(app.state().status, "Saved 1 items");
        assert!(app.press_key_code(KeyCode::Char('g'), cmd));
        assert!(!app.state().grid);
    }
}
