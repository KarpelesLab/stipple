//! The text attribute runs of the ARIA overlay's rich text: `web/stipple.js`
//! writes a static text whose runs differ (a link, a word in another
//! language, a highlighted match, a bold word) as markup, so a screen reader
//! reads its links as links (`<a href>`), its languages in their voices
//! (`lang`), its highlights as marked (`<mark>`), and its emphasis.
//!
//! [`aria_json`](crate::platform::access::aria_json)'s tree is sent as it
//! is; the runs follow it as `"ru":[[index, runs], …]`, `index` a node's
//! place in the tree in preorder (the tree's root 0), each run
//! `{"t": text, "b":1 bold, "i":1 italic, "u":1 underlined, "s":1 struck
//! through, "m":1 highlighted, "l": language, "a": link}` (flags only when
//! set). A page's older script ignores the key. Only static text whose
//! node carries runs (rich text) is looked at, and the runs are built only
//! when the tree changed (the bridge sends nothing otherwise).

use std::fmt::Write;

use crate::platform::access::{A11yNode, A11yRole, A11yTextRun};

/// `json`, [`aria_json`](crate::platform::access::aria_json)'s tree for
/// `root`, with the runs of its rich text after it.
pub(crate) fn with_runs(mut json: String, root: &A11yNode) -> String {
    let mut runs = String::new();
    let mut index = 0;
    collect(root, &mut index, &mut runs);
    if !runs.is_empty() && json.ends_with('}') {
        json.pop();
        json.push_str(",\"ru\":[");
        json.push_str(&runs);
        json.push_str("]}");
    }
    json
}

/// Append the runs of `n` and its descendants (from preorder `index` on)
/// to `out`, comma-separated.
fn collect(n: &A11yNode, index: &mut usize, out: &mut String) {
    let at = *index;
    *index += 1;
    if n.role == A11yRole::Text && n.runs.0.is_some() && n.children.is_empty() {
        let runs = n.text_runs();
        if worth_markup(&runs) {
            if !out.is_empty() {
                out.push(',');
            }
            let _ = write!(out, "[{at},[");
            let mut first = true;
            for r in &runs {
                let Some(text) = n.name.get(r.range.0..r.range.1) else {
                    continue;
                };
                if !first {
                    out.push(',');
                }
                first = false;
                run_json(out, text, r);
            }
            out.push_str("]]");
        }
    }
    for c in &n.children {
        collect(c, index, out);
    }
}

/// Whether `runs` read differently from plain text: more than one, or one
/// that links, has a language or is highlighted.
fn worth_markup(runs: &[A11yTextRun]) -> bool {
    runs.len() > 1
        || runs
            .iter()
            .any(|r| r.link.is_some() || r.language.is_some() || r.background.is_some())
}

/// Run `r` over `text`, as the page reads it.
fn run_json(out: &mut String, text: &str, r: &A11yTextRun) {
    out.push_str("{\"t\":");
    string(out, text);
    for (key, on) in [
        ("b", r.font.weight >= 600),
        ("i", r.font.italic),
        ("u", r.underline),
        ("s", r.strikethrough),
        ("m", r.background.is_some_and(|c| c[3] > 0)),
    ] {
        if on {
            let _ = write!(out, ",\"{key}\":1");
        }
    }
    if let Some(l) = &r.language {
        out.push_str(",\"l\":");
        string(out, l);
    }
    if let Some(a) = &r.link {
        out.push_str(",\"a\":");
        string(out, a);
    }
    out.push('}');
}

/// `s` as a JSON string.
fn string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::access::{A11yFont, A11yRuns, aria_json};

    fn font(weight: u16, italic: bool) -> A11yFont {
        A11yFont {
            family: None,
            size: 14.0,
            weight,
            italic,
        }
    }

    fn rich(name: &str, runs: Vec<A11yTextRun>) -> A11yNode {
        let mut n = A11yNode::new(A11yRole::Text, name, (0.0, 0.0, 10.0, 10.0));
        n.font = Some(font(400, false));
        n.runs = A11yRuns::fixed(runs);
        n
    }

    fn window(children: Vec<A11yNode>) -> A11yNode {
        let mut root = A11yNode::new(A11yRole::Window, "", (0.0, 0.0, 0.0, 0.0));
        root.children = children;
        root
    }

    #[test]
    fn rich_text_sends_its_runs_after_the_tree() {
        let bold = A11yTextRun::plain((0, 4), font(700, false));
        let link = A11yTextRun {
            link: Some("https://x.test/?a=\"b\"".into()),
            underline: true,
            ..A11yTextRun::plain((5, 9), font(400, false))
        };
        let fr = A11yTextRun {
            language: Some("fr".into()),
            background: Some([255, 224, 102, 255]),
            ..A11yTextRun::plain((10, 17), font(400, true))
        };
        let root = window(vec![
            A11yNode::new(A11yRole::Button, "OK", (0.0, 0.0, 0.0, 0.0)),
            rich("Bold link bonjour", vec![bold, link, fr]),
            // Rich text drawn all alike stays text.
            rich("plain", Vec::new()),
        ]);
        let tree = aria_json(&root);
        let json = with_runs(tree.clone(), &root);
        assert!(json.starts_with(&tree[..tree.len() - 1]));
        assert!(
            json.ends_with(concat!(
                r#","ru":[[2,[{"t":"Bold","b":1},{"t":" "},"#,
                r#"{"t":"link","u":1,"a":"https://x.test/?a=\"b\""},{"t":" "},"#,
                r#"{"t":"bonjour","i":1,"m":1,"l":"fr"}]]]}"#
            )),
            "{json}"
        );
    }

    /// An app's rich text, end to end: the index is its node's place in
    /// the tree the page parses.
    #[test]
    fn an_apps_rich_text_is_indexed_in_preorder() {
        use crate::App;
        use crate::render::{Color, RichText, SpanStyle};
        use crate::widgets::{button, column, label};
        let view = |_: &(), cx: &mut crate::core::Cx<()>| {
            let text = RichText::new()
                .bold("Notes")
                .push(" ")
                .styled("type", SpanStyle::new().link("https://stipple.rs"));
            column(vec![
                button(cx, "OK"),
                label(cx, "plain"),
                crate::core::Element::rich_text(text, 14.0, Color::BLACK),
            ])
        };
        let mut app = App::new((), view)
            .no_font()
            .logical_size(crate::geometry::Size::new(300.0, 200.0));
        app.render_once();
        let root = crate::to_platform_a11y(&app.accessibility_tree().unwrap());
        let json = with_runs(aria_json(&root), &root);
        // Preorder: the window 0, its children from 1.
        let at = root
            .children
            .iter()
            .position(|c| c.name == "Notes type")
            .map(|i| i + 1);
        assert_eq!(at, Some(3), "{json}");
        assert!(
            json.ends_with(concat!(
                r#","ru":[[3,[{"t":"Notes","b":1},{"t":" "},"#,
                r#"{"t":"type","a":"https://stipple.rs"}]]]}"#
            )),
            "{json}"
        );
    }

    #[test]
    fn a_tree_without_rich_text_is_sent_as_it_is() {
        let root = window(vec![
            rich("plain", Vec::new()),
            A11yNode::new(A11yRole::Text, "text", (0.0, 0.0, 0.0, 0.0)),
        ]);
        let tree = aria_json(&root);
        assert_eq!(with_runs(tree.clone(), &root), tree);
    }
}
