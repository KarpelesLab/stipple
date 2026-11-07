//! A virtualized 10,000-row table and a tree, side by side.
//!
//! - **Table**: click a header to sort (again to reverse), drag a header's
//!   right edge to resize its column, click / Shift+click / Ctrl+click rows,
//!   or use the arrows (Shift extends), Home/End and Ctrl+A.
//! - **Tree**: click a triangle to expand; Left/Right collapse/expand or move
//!   to the parent / first child; Up/Down move. The `logs` folder holds 5,000
//!   entries.
//!
//! Only the rows on screen are built. Pass `--theme dark|contrast|native` to
//! try another built-in theme. CI (X11/Xvfb) screenshots it.

use stipple::prelude::*;

const W: f64 = 900.0;
const H: f64 = 560.0;
const ROWS: usize = 10_000;

const KINDS: [&str; 5] = ["Text", "Image", "Audio", "Video", "Archive"];
const EXT: [&str; 5] = ["txt", "png", "ogg", "mkv", "zip"];

/// One row of the table's data.
struct File {
    name: String,
    kind: usize,
    size: u64,
}

/// A deterministic pseudo-random sequence (so the CI screenshot is stable).
fn lcg(seed: u64) -> u64 {
    seed.wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
}

fn files() -> Vec<File> {
    let mut x = 42u64;
    (0..ROWS)
        .map(|i| {
            x = lcg(x);
            let kind = (x >> 33) as usize % KINDS.len();
            x = lcg(x);
            let size = ((x >> 20) % 50_000_000) >> ((x >> 8) % 24);
            File {
                name: format!("file-{:05}.{}", (i * 7919) % 100_000, EXT[kind]),
                kind,
                size,
            }
        })
        .collect()
}

fn human(size: u64) -> String {
    match size {
        s if s >= 1 << 20 => format!("{:.1} MB", s as f64 / (1u64 << 20) as f64),
        s if s >= 1 << 10 => format!("{:.1} KB", s as f64 / 1024.0),
        s => format!("{s} B"),
    }
}

/// A folder tree in an arena; ids index into it.
struct Folders {
    names: Vec<String>,
    children: Vec<Vec<u64>>,
    roots: Vec<u64>,
}

impl Folders {
    fn add(&mut self, parent: Option<u64>, name: &str) -> u64 {
        let id = self.names.len() as u64;
        self.names.push(name.to_string());
        self.children.push(Vec::new());
        match parent {
            Some(p) => self.children[p as usize].push(id),
            None => self.roots.push(id),
        }
        id
    }

    fn new() -> Self {
        let mut f = Folders {
            names: Vec::new(),
            children: Vec::new(),
            roots: Vec::new(),
        };
        let src = f.add(None, "src");
        let widgets = f.add(Some(src), "widgets");
        for name in ["mod.rs", "table.rs", "tree.rs", "virtual_list.rs"] {
            f.add(Some(widgets), name);
        }
        let core = f.add(Some(src), "core");
        for name in ["a11y.rs", "element.rs", "runtime.rs"] {
            f.add(Some(core), name);
        }
        f.add(Some(src), "lib.rs");
        let docs = f.add(None, "docs");
        f.add(Some(docs), "layout.md");
        f.add(Some(docs), "lists-and-tables.md");
        let logs = f.add(None, "logs");
        for i in 0..5_000 {
            f.add(Some(logs), &format!("run-{i:04}.log"));
        }
        f.add(None, "Cargo.toml");
        f.add(None, "README.md");
        f
    }
}

impl TreeModel for Folders {
    fn children(&self, parent: Option<u64>) -> Vec<u64> {
        match parent {
            None => self.roots.clone(),
            Some(id) => self.children[id as usize].clone(),
        }
    }

    fn has_children(&self, id: u64) -> bool {
        !self.children[id as usize].is_empty()
    }
}

struct Demo {
    files: Vec<File>,
    /// Display order: `order[row]` is the file shown at `row`.
    order: Vec<usize>,
    table: TableState,
    folders: Folders,
    tree: TreeState,
}

impl Demo {
    fn new() -> Self {
        let mut tree = TreeState::new();
        tree.set_expanded(0, true); // src
        tree.set_expanded(1, true); // src/widgets
        tree.select(Some(4)); // tree.rs
        let mut table = TableState::new(SelectionMode::Multi);
        table.rows.select(2);
        table.rows.extend_to(4);
        Demo {
            files: files(),
            order: (0..ROWS).collect(),
            table,
            folders: Folders::new(),
            tree,
        }
    }

    fn sort(&mut self, column: usize, order: SortOrder) {
        let files = &self.files;
        self.order.sort_by(|&a, &b| {
            let (fa, fb) = (&files[a], &files[b]);
            let by = match column {
                0 => a.cmp(&b),
                1 => fa.name.cmp(&fb.name),
                2 => fa.kind.cmp(&fb.kind).then(a.cmp(&b)),
                _ => fa.size.cmp(&fb.size),
            };
            match order {
                SortOrder::Ascending => by,
                SortOrder::Descending => by.reverse(),
            }
        });
    }
}

fn columns() -> Vec<Column> {
    vec![
        Column::new("#", 70.0).sortable().align(Align::End),
        Column::new("Name", 180.0).sortable().min_width(80.0),
        Column::new("Kind", 100.0).sortable(),
        Column::new("Size", 110.0).sortable().align(Align::End),
    ]
}

fn view(state: &Demo, cx: &mut Cx<Demo>) -> Element {
    let theme = *cx.theme();
    let sp = theme.spacing;
    let cols = columns();

    let grid = table(
        cx,
        &theme,
        &state.table,
        |s: &mut Demo| &mut s.table,
        TableSpec::new(&cols, state.order.len())
            .row_height(26.0)
            .height(390.0),
        |s: &mut Demo, column, order| s.sort(column, order),
        |_cx, row, column| {
            let i = state.order[row];
            let f = &state.files[i];
            let text = match column {
                0 => (i + 1).to_string(),
                1 => f.name.clone(),
                2 => KINDS[f.kind].to_string(),
                _ => human(f.size),
            };
            label(&theme, text)
        },
    );

    let folders = tree(
        cx,
        &theme,
        &state.tree,
        |s: &mut Demo| &mut s.tree,
        &state.folders,
        TreeSpec::default().row_height(26.0).height(417.0),
        |_cx, row| label(&theme, state.folders.names[row.id as usize].clone()),
    )
    .width(280.0);

    let sort = match state.table.sort() {
        Some((c, o)) => format!(
            " · sorted by {} {}",
            cols[c].title,
            if o == SortOrder::Ascending {
                "ascending"
            } else {
                "descending"
            }
        ),
        None => String::new(),
    };
    let status = Element::text(
        format!(
            "{} rows · {} selected{sort}",
            state.order.len(),
            state.table.rows.selected_count()
        ),
        theme.typography.caption,
        theme.palette.text_muted,
    );

    column(vec![
        heading(&theme, "Tables and trees"),
        status,
        row(vec![grid, folders])
            .gap(sp.lg)
            .align(Align::Start, Align::Start),
    ])
    .gap(sp.sm)
    .padding(Insets::uniform(sp.lg))
    .grow(1.0)
}

fn theme_from_args() -> Theme {
    let args: Vec<String> = std::env::args().collect();
    let name = args
        .iter()
        .position(|a| a == "--theme")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);
    match name {
        Some("dark") => Theme::dark(),
        Some("contrast") => Theme::light().high_contrast(),
        Some("native") => Theme::native(),
        _ => Theme::light(),
    }
}

fn demo_app(theme: Theme) -> App<Demo> {
    App::new(Demo::new(), view)
        .title("Stipple — tables and trees")
        .theme(theme)
        .logical_size(Size::new(W, H))
}

fn main() {
    let mut app = demo_app(theme_from_args());
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use stipple::core::Role;

    #[test]
    fn renders_in_every_theme_with_the_expected_structure() {
        for theme in [
            Theme::light(),
            Theme::dark(),
            Theme::light().high_contrast(),
            Theme::native(),
        ] {
            let mut app = demo_app(theme);
            app.render_once();
            let a = app.accessibility_tree().unwrap();
            let count = |role| a.descendants().iter().filter(|n| n.role == role).count();
            assert_eq!(count(Role::Table), 1);
            assert_eq!(count(Role::Tree), 1);
            assert_eq!(count(Role::ColumnHeader), 4);
            // 10,000 rows, but only a screenful (plus overscan) are built.
            assert!(count(Role::Row) < 25, "{} rows", count(Role::Row));
            assert!(count(Role::TreeItem) < 25);
            // The tree shows src/widgets expanded, with tree.rs selected.
            let selected: Vec<_> = a
                .descendants()
                .into_iter()
                .filter(|n| n.role == Role::TreeItem && n.selected)
                .map(|n| n.name.clone())
                .collect();
            assert_eq!(selected, vec!["tree.rs"]);
        }
    }

    #[test]
    fn sorting_by_size_orders_the_rows() {
        let mut demo = Demo::new();
        demo.sort(3, SortOrder::Descending);
        let sizes: Vec<u64> = demo.order.iter().map(|&i| demo.files[i].size).collect();
        assert!(sizes.windows(2).all(|w| w[0] >= w[1]));
    }
}
