//! `org.a11y.atspi.Text`'s text attributes: a node's runs drawn alike
//! ([`A11yNode::text_runs`], built when a screen reader asks) as the
//! attribute sets `GetAttributeRun`, `GetAttributes`, `GetAttributeValue`
//! and `GetDefaultAttributes` return, under the names and in the formats
//! GTK and ATK use (`family-name`, `size` in points, `weight`, `style`,
//! `underline`, `strikethrough`, `fg-color` / `bg-color` as `"r,g,b"` in
//! 16 bits a channel, `language`). Offsets are in characters.

use super::text::{byte_offset, char_offset};
use crate::platform::access::{A11yNode, A11yTextRun};

/// An attribute set: `(name, value)` pairs, in a fixed order.
pub(super) type Attrs = Vec<(&'static str, String)>;

/// A color as ATK writes it: `"r,g,b"`, each channel 0 to 65535.
fn color(c: [u8; 4]) -> String {
    let w = |v: u8| u32::from(v) * 257;
    format!("{},{},{}", w(c[0]), w(c[1]), w(c[2]))
}

/// Logical pixels as points (a pixel is a 96th of an inch), to the half
/// point: `"12"`, `"10.5"`.
fn points(px: f64) -> String {
    let pt = (px * 0.75 * 2.0).round() / 2.0;
    if pt.fract() == 0.0 {
        format!("{}", pt as i64)
    } else {
        format!("{pt}")
    }
}

/// Every attribute of `run`.
fn all_of(run: &A11yTextRun) -> Attrs {
    let f = &run.font;
    let mut out: Attrs = Vec::with_capacity(9);
    if let Some(family) = &f.family {
        out.push(("family-name", family.clone()));
    }
    out.push(("size", points(f.size)));
    out.push(("weight", f.weight.to_string()));
    out.push((
        "style",
        if f.italic { "italic" } else { "normal" }.to_string(),
    ));
    out.push((
        "underline",
        if run.underline { "single" } else { "none" }.to_string(),
    ));
    out.push(("strikethrough", run.strikethrough.to_string()));
    if let Some(c) = run.color {
        out.push(("fg-color", color(c)));
    }
    if let Some(c) = run.background {
        out.push(("bg-color", color(c)));
    }
    if let Some(lang) = &run.language {
        out.push(("language", lang.clone()));
    }
    out
}

/// The node's default attributes: its font, no lines, and the color most
/// of its text is drawn in (none when no run says).
pub(super) fn defaults(n: &A11yNode) -> Attrs {
    let runs = n.text_runs();
    let Some(font) = n
        .font
        .clone()
        .or_else(|| runs.first().map(|r| r.font.clone()))
    else {
        return Vec::new();
    };
    // The color covering the most bytes.
    let mut colors: Vec<([u8; 4], usize)> = Vec::new();
    for r in &runs {
        if let Some(c) = r.color {
            let len = r.range.1 - r.range.0;
            match colors.iter_mut().find(|(k, _)| *k == c) {
                Some((_, n)) => *n += len,
                None => colors.push((c, len)),
            }
        }
    }
    let fg = colors.iter().max_by_key(|(_, n)| *n).map(|(c, _)| *c);
    let mut base = A11yTextRun::plain((0, 0), font);
    base.color = fg;
    all_of(&base)
}

/// The run at character `offset` of `n`'s text: its attributes (only
/// those that differ from the defaults, unless `with_defaults`) and its
/// character range. An empty set over the whole text when it has none.
pub(super) fn run_at(n: &A11yNode, offset: i32, with_defaults: bool) -> (Attrs, i32, i32) {
    let t = n.name.as_str();
    let count = super::text::char_count(t);
    let Some(run) = n.text_run_at(byte_offset(t, offset)) else {
        return (Vec::new(), 0, count);
    };
    let (s, e) = (char_offset(t, run.range.0), char_offset(t, run.range.1));
    let all = all_of(&run);
    if with_defaults {
        // Its own, then the defaults it doesn't set.
        let mut out = all;
        for d in defaults(n) {
            if !out.iter().any(|(k, _)| *k == d.0) {
                out.push(d);
            }
        }
        return (out, s, e);
    }
    let base = defaults(n);
    let differs = all.into_iter().filter(|a| !base.contains(a)).collect();
    (differs, s, e)
}

/// `GetAttributeValue`: attribute `name` at character `offset` (the
/// default where the run doesn't set it), empty when there is none.
pub(super) fn value_at(n: &A11yNode, offset: i32, name: &str) -> String {
    run_at(n, offset, true)
        .0
        .into_iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
        .unwrap_or_default()
}

/// `attrs` as the `(&str, &str)` pairs the marshaller writes.
pub(super) fn pairs(attrs: &Attrs) -> Vec<(&str, &str)> {
    attrs.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::access::{A11yFont, A11yRole, A11yRuns};

    fn font(weight: u16, italic: bool) -> A11yFont {
        A11yFont {
            family: None,
            size: 16.0,
            weight,
            italic,
        }
    }

    const BLACK: [u8; 4] = [0, 0, 0, 255];

    /// "héllo bold link" with "bold" bold and "link" underlined, blue, in
    /// French, on yellow.
    fn node() -> A11yNode {
        let mut n = A11yNode::new(A11yRole::Text, "héllo bold link", (0.0, 0.0, 150.0, 20.0));
        n.font = Some(A11yFont {
            family: Some("Inter".into()),
            ..font(400, false)
        });
        let plain = |r, f| A11yTextRun {
            color: Some(BLACK),
            ..A11yTextRun::plain(r, f)
        };
        let link = A11yTextRun {
            underline: true,
            color: Some([0, 0, 255, 255]),
            background: Some([255, 255, 0, 255]),
            language: Some("fr".into()),
            link: Some("https://stipple.rs".into()),
            ..A11yTextRun::plain((12, 16), font(400, false))
        };
        n.runs = A11yRuns::fixed(vec![
            plain((0, 7), font(400, false)),
            plain((7, 11), font(700, false)),
            plain((11, 12), font(400, false)),
            link,
        ]);
        n
    }

    fn get<'a>(a: &'a Attrs, k: &str) -> Option<&'a str> {
        a.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn defaults_are_the_font_and_the_most_used_color() {
        let d = defaults(&node());
        assert_eq!(get(&d, "family-name"), Some("Inter"));
        assert_eq!(get(&d, "size"), Some("12"));
        assert_eq!(get(&d, "weight"), Some("400"));
        assert_eq!(get(&d, "style"), Some("normal"));
        assert_eq!(get(&d, "underline"), Some("none"));
        assert_eq!(get(&d, "strikethrough"), Some("false"));
        assert_eq!(get(&d, "fg-color"), Some("0,0,0"));
        assert_eq!(get(&d, "bg-color"), None);
    }

    #[test]
    fn runs_in_characters_with_what_differs() {
        let n = node();
        // "héllo " is 6 characters (7 bytes): the bold run is 6..10.
        let (a, s, e) = run_at(&n, 7, false);
        assert_eq!((s, e), (6, 10));
        assert_eq!(a, vec![("weight", "700".to_string())]);
        let (a, s, e) = run_at(&n, 12, false);
        assert_eq!((s, e), (11, 15));
        assert_eq!(get(&a, "underline"), Some("single"));
        assert_eq!(get(&a, "fg-color"), Some("0,0,65535"));
        assert_eq!(get(&a, "bg-color"), Some("65535,65535,0"));
        assert_eq!(get(&a, "language"), Some("fr"));
        assert_eq!(get(&a, "weight"), None, "the default's");
        // With the defaults, all of them.
        let (a, _, _) = run_at(&n, 12, true);
        assert_eq!(get(&a, "weight"), Some("400"));
        assert_eq!(get(&a, "family-name"), Some("Inter"));
        // Plain text: nothing differs, over its run.
        let (a, s, e) = run_at(&n, 0, false);
        assert!(a.is_empty());
        assert_eq!((s, e), (0, 6));
        // Past the end: the last run.
        assert_eq!(run_at(&n, 99, false).1, 11);
        assert_eq!(value_at(&n, 8, "weight"), "700");
        assert_eq!(value_at(&n, 0, "style"), "normal");
        assert_eq!(value_at(&n, 0, "invalid"), "");
    }

    #[test]
    fn text_with_no_font_has_no_attributes() {
        let n = A11yNode::new(A11yRole::Text, "plain", (0.0, 0.0, 10.0, 10.0));
        assert_eq!(run_at(&n, 2, true), (Vec::new(), 0, 5));
        assert!(defaults(&n).is_empty());
        assert_eq!(points(14.0), "10.5");
    }
}
