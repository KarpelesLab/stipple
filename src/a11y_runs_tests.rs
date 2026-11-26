//! Text attribute runs as assistive technologies see them, through an
//! [`App`]: rich text reports a run per stretch drawn differently (a bold
//! word, a link, another language, an image read as its alternative text),
//! a rich editor its marks and links, a text view its highlighted matches;
//! the platform's tree carries them, built only when a bridge asks.

use std::sync::Arc;

use crate::App;
use crate::core::{AccessNode, AccessTextRun, Cx, Element, Role};
use crate::geometry::{PhysicalSize, Size};
use crate::render::{Color, InlineIcon, Pixmap, RichText, SpanStyle};
use crate::widgets::{
    FindOptions, RichEditor, TextView, TextViewOptions, column, rich_editor, text_view,
};

const RED: Color = Color::rgb(0xd0, 0x30, 0x30);

struct Doc {
    editor: RichEditor,
    log: TextView,
}

fn rich_line() -> RichText {
    let coin = Arc::new(Pixmap::new(PhysicalSize::new(8, 8)));
    RichText::new()
        .push("Say ")
        .styled("bonjour", SpanStyle::new().language("fr").italic())
        .push(" to ")
        .icon(InlineIcon::new(coin).alt("a coin"))
        .push(" ")
        .styled(
            "docs",
            SpanStyle::new().color(RED).link("https://example.com"),
        )
        .styled(" now", SpanStyle::new().strikethrough().background(RED))
}

fn view(s: &Doc, cx: &mut Cx<Doc>) -> Element {
    column(vec![
        Element::rich_text(rich_line(), 14.0, Color::BLACK),
        Element::rich_text(RichText::new().bold("named"), 14.0, Color::BLACK).a11y_label("A label"),
        rich_editor(cx, &s.editor, |s| &mut s.editor),
        text_view(cx, &s.log, |s| &mut s.log, TextViewOptions::default()),
    ])
}

fn app() -> App<Doc> {
    let mut log = TextView::from_text("first\n\none error here");
    log.find("error", FindOptions::default());
    let doc = Doc {
        editor: RichEditor::from_markdown("Read **this** and [that](https://stipple.rs)."),
        log,
    };
    let mut app = App::new(doc, view)
        .no_font()
        .logical_size(Size::new(600.0, 500.0));
    app.render_once();
    app
}

fn texts(tree: &AccessNode) -> Vec<AccessNode> {
    tree.descendants()
        .into_iter()
        .filter(|n| !n.name.is_empty() && matches!(n.role, Role::Text | Role::TextField))
        .cloned()
        .collect()
}

/// The text of each run, with the run.
fn split(n: &AccessNode) -> Vec<(&str, AccessTextRun)> {
    n.text_runs()
        .into_iter()
        .map(|r| (&n.name[r.range.clone()], r))
        .collect()
}

#[test]
fn rich_text_reports_a_run_per_look() {
    let app = app();
    let tree = app.accessibility_tree().unwrap();
    let all = texts(&tree);
    let line = all.iter().find(|n| n.name.starts_with("Say")).unwrap();
    assert_eq!(line.name, "Say bonjour to a coin docs now");
    let runs = split(line);
    let words: Vec<&str> = runs.iter().map(|(t, _)| *t).collect();
    // The image's run reads as its alternative text; the plain stretches
    // around it look alike, so they join it.
    assert_eq!(words, ["Say ", "bonjour", " to a coin ", "docs", " now"]);
    let (_, fr) = &runs[1];
    assert_eq!(fr.language.as_deref(), Some("fr"));
    assert!(fr.font.italic);
    assert_eq!(fr.color, Some(Color::BLACK));
    let (_, link) = &runs[3];
    assert_eq!(link.link.as_deref(), Some("https://example.com"));
    assert_eq!(link.color, Some(RED));
    let (_, now) = &runs[4];
    assert!(now.strikethrough && !now.underline);
    assert_eq!(now.background, Some(RED));
    // A node named by its label has no runs of its text.
    let named = all.iter().find(|n| n.name == "A label").unwrap();
    assert_eq!(named.text_runs().len(), 1);
    assert_eq!(named.text_runs()[0].font.weight, 400);
}

#[test]
fn a_rich_editor_reports_its_marks_and_links() {
    let app = app();
    let tree = app.accessibility_tree().unwrap();
    let field = tree
        .descendants()
        .into_iter()
        .find(|n| n.role == Role::TextField && n.name.starts_with("Read"))
        .cloned()
        .expect("the editor");
    let runs = split(&field);
    let bold = runs.iter().find(|(t, _)| *t == "this").expect("a bold run");
    assert_eq!(bold.1.font.weight, 700);
    let link = runs.iter().find(|(t, _)| *t == "that").expect("a link run");
    assert_eq!(link.1.link.as_deref(), Some("https://stipple.rs"));
    assert!(link.1.underline);
}

#[test]
fn a_text_view_reports_its_highlighted_matches() {
    let app = app();
    let tree = app.accessibility_tree().unwrap();
    let line = texts(&tree)
        .into_iter()
        .find(|n| n.name == "first\n\none error here")
        .expect("the log's lines");
    let runs = split(&line);
    // Its lines on screen, the empty one read as empty, the match on the
    // third.
    let hit = runs.iter().find(|(t, _)| *t == "error").expect("the match");
    assert_eq!(hit.1.range, 11..16);
    assert!(hit.1.background.is_some());
    assert_eq!(runs[0].0, "first\n\none ");
    assert!(runs[0].1.background.is_none());
}

#[test]
fn the_platform_tree_builds_runs_when_asked() {
    let app = app();
    let tree = crate::to_platform_a11y(&app.accessibility_tree().unwrap());
    let paths = tree.find_paths(&|n| n.name.starts_with("Say"));
    let line = tree.at_path(&paths[0]).unwrap();
    assert!(line.runs.0.is_some(), "rich text carries its runs' source");
    let mut filled = line.clone();
    filled.fill_font_family("Inter");
    let runs = filled.text_runs();
    assert_eq!(runs.len(), 5);
    assert_eq!(runs[3].link.as_deref(), Some("https://example.com"));
    assert_eq!(runs[3].color, Some([0xd0, 0x30, 0x30, 0xff]));
    // The app's font fills in the family the runs don't name.
    assert!(
        runs.iter()
            .all(|r| r.font.family.as_deref() == Some("Inter"))
    );
    // The same frame again compares equal (its runs too), so the bridges
    // skip it.
    let again = crate::to_platform_a11y(&app.accessibility_tree().unwrap());
    assert_eq!(tree, again);
}
