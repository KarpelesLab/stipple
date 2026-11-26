//! What the macOS elements say of a node beyond its role and label, and the
//! notifications a tree swap posts, kept free of AppKit so it is unit-tested
//! on every host (`ax_value` sends it to AppKit).
//!
//! - A field's `AXValue` is its text (when the tree carries it: the field's
//!   name is its text), and its label is then its caption (the node its
//!   `labelled_by` names) or nothing, so VoiceOver doesn't read the text
//!   twice. A password field has no value (`AXSecureTextField`).
//! - A field's text range attributes count UTF-16 units, as `NSString` does.
//! - A checkbox, switch or radio button's `AXValue` is 1 when checked, else 0
//!   (a tab's: 1 when it's the selected one).
//! - A slider's or a progress bar's `AXValue` is its number, with its
//!   `AXMinValue`, `AXMaxValue` and `AXValueDescription` ("50%"); a
//!   slider's can be set, and stepped with `AXIncrement` / `AXDecrement`.
//! - A table has its `AXRowCount` and `AXColumnCount` (all its rows), a
//!   row its `AXIndex`, a cell its `AXRowIndexRange` and
//!   `AXColumnIndexRange`.
//! - A plain group that scrolls is an `AXScrollArea`; it (and a list, tree
//!   or table that scrolls) has a scroll bar along its trailing edge, whose
//!   `AXValue` is how far it is scrolled, `0..=1`.

// Not every helper is used on every target; the tests use them all.
#![allow(dead_code)]

use crate::platform::access::{A11yChange, A11yNode, A11yRole};

/// How wide the scroll bar element is reported, in points.
pub(crate) const BAR_WIDTH: f64 = 12.0;

/// `s`'s length in UTF-16 units.
pub(crate) fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The UTF-16 offset of byte `b` of `s` (clamped to its end, snapped back
/// to a character).
pub(crate) fn utf16_at(s: &str, b: usize) -> usize {
    let mut b = b.min(s.len());
    while !s.is_char_boundary(b) {
        b -= 1;
    }
    utf16_len(&s[..b])
}

/// The byte offset of UTF-16 offset `u` in `s` (clamped to its end; inside
/// a surrogate pair, the character's start).
pub(crate) fn byte_at(s: &str, u: usize) -> usize {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        let next = units + c.len_utf16();
        if next > u {
            return i;
        }
        units = next;
    }
    s.len()
}

/// Whether `node` is a field whose name is its text (the tree carries its
/// caret), which its `AXValue` then holds.
pub(crate) fn text_value(node: &A11yNode) -> Option<&str> {
    let field = matches!(node.role, A11yRole::TextField | A11yRole::SpinButton);
    (field && node.caret.is_some()).then_some(node.name.as_str())
}

/// Whether a client may set the node's `AXValue` (its text).
pub(crate) fn settable_text(node: &A11yNode) -> bool {
    matches!(
        node.role,
        A11yRole::TextField | A11yRole::SpinButton | A11yRole::PasswordField
    )
}

/// A field's selection as `(location, length)` in UTF-16 units: its
/// selection, else its caret (empty). `None` when the tree carries neither.
pub(crate) fn selected_range(node: &A11yNode) -> Option<(usize, usize)> {
    let text = text_value(node)?;
    if let Some((s, e)) = node.text_selection {
        let (s, e) = (utf16_at(text, s), utf16_at(text, e));
        return Some((s.min(e), s.abs_diff(e)));
    }
    node.caret.map(|c| (utf16_at(text, c), 0))
}

/// The bytes `anchor..caret` of a field's text a range of UTF-16 units
/// `(location, length)` selects.
pub(crate) fn range_bytes(node: &A11yNode, location: usize, length: usize) -> (usize, usize) {
    let t = &node.name;
    (
        byte_at(t, location),
        byte_at(t, location.saturating_add(length)),
    )
}

/// A checkable node's `AXValue`: 1 when checked (a tab: selected), else 0.
pub(crate) fn number_value(node: &A11yNode) -> Option<i64> {
    use A11yRole::*;
    match node.role {
        CheckBox | Switch | RadioButton => node.checked.map(i64::from),
        Tab => Some(i64::from(node.selected)),
        _ => None,
    }
}

/// A slider's or a progress bar's `(value, min, max)` (a number input's
/// value is its text, above).
pub(crate) fn range_value(node: &A11yNode) -> Option<(f64, f64, f64)> {
    let r = node
        .range
        .filter(|_| text_value(node).is_none() && !settable_text(node))?;
    Some((r.value, r.min, r.max))
}

/// What VoiceOver reads for a range value (`AXValueDescription`).
pub(crate) fn value_description(node: &A11yNode) -> Option<String> {
    range_value(node)?;
    node.range.map(|r| r.text())
}

/// Whether a client may set (and step) the node's range value: a slider's.
pub(crate) fn range_settable(node: &A11yNode) -> bool {
    range_value(node).is_some() && node.range_settable()
}

/// A table's `(rows, columns)`.
pub(crate) fn table_size(node: &A11yNode) -> Option<(usize, usize)> {
    node.table
        .filter(|_| node.role == A11yRole::Table)
        .map(|t| (t.rows, t.columns))
}

/// A row's `AXIndex`.
pub(crate) fn row_index(node: &A11yNode) -> Option<usize> {
    (node.role == A11yRole::Row).then_some(node.cell?.row)
}

/// A cell's (or column header's) `(row, column)`, each a range of one.
pub(crate) fn cell_index(node: &A11yNode) -> Option<(usize, usize)> {
    matches!(node.role, A11yRole::Cell | A11yRole::ColumnHeader)
        .then_some(node.cell.map(|c| (c.row, c.column))?)
}

/// The paths (from a table) of its rows, in order.
pub(crate) fn table_rows(table: &A11yNode) -> Vec<Vec<usize>> {
    fn go(n: &A11yNode, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        for (i, c) in n.children.iter().enumerate() {
            path.push(i);
            match c.role {
                A11yRole::Row => out.push(path.clone()),
                A11yRole::Table => {}
                _ => go(c, path, out),
            }
            path.pop();
        }
    }
    let mut out = Vec::new();
    go(table, &mut Vec::new(), &mut out);
    out
}

/// The node's subrole, where AppKit's own control would have one.
pub(crate) fn subrole(node: &A11yNode) -> Option<&'static str> {
    match node.role {
        A11yRole::Switch => Some("AXSwitch"),
        A11yRole::PasswordField => Some("AXSecureTextField"),
        _ => None,
    }
}

/// Whether the node is a scroll container whose content overflows.
pub(crate) fn scrolls(node: &A11yNode) -> bool {
    node.scroll.is_some_and(|s| s.scrolls())
}

/// The role that replaces the role's own for this node: a plain group that
/// scrolls is a scroll area.
pub(crate) fn role_override(node: &A11yNode) -> Option<&'static str> {
    (node.role == A11yRole::Group && scrolls(node)).then_some("AXScrollArea")
}

/// The label of a field whose value is its text: the name of its caption
/// (the node `labelled_by` names, under `root`), else none. `None` for
/// any other node, which keeps its name as its label.
pub(crate) fn field_label(node: &A11yNode, root: Option<&A11yNode>) -> Option<String> {
    text_value(node)?;
    fn find<'a>(n: &'a A11yNode, id: &str) -> Option<&'a A11yNode> {
        if n.id.as_deref() == Some(id) {
            return Some(n);
        }
        n.children.iter().find_map(|c| find(c, id))
    }
    let caption = node
        .labelled_by
        .as_deref()
        .and_then(|id| find(root?, id))
        .map(|c| c.name.clone());
    Some(caption.unwrap_or_default())
}

/// The scroll bar's frame for a container at `frame` (screen coordinates,
/// bottom-left origin, as `(x, y, width, height)`): a strip along its right
/// edge, or its bottom one when it scrolls sideways (the left one, right to
/// left).
pub(crate) fn bar_frame(frame: (f64, f64, f64, f64), horizontal: bool, rtl: bool) -> [f64; 4] {
    let (x, y, w, h) = frame;
    let t = BAR_WIDTH.min(w).min(h);
    match (horizontal, rtl) {
        (true, _) => [x, y, w, t],
        (false, false) => [x + w - t, y, t, h],
        (false, true) => [x, y, t, h],
    }
}

/// One notification to post: `name` on the element of the node at `path`
/// (the view for the root), or on its scroll bar when `bar`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Post {
    pub(crate) path: Vec<usize>,
    pub(crate) name: &'static str,
    pub(crate) bar: bool,
}

/// At most this many notifications for one swap; past them only the layout
/// change, which tells a client to read the whole thing again.
const MAX_POSTS: usize = 32;

/// The notifications for `changes` (from the tree before to `new`), in
/// order, with one `AXLayoutChanged` on the view at the end when children
/// came, went or scrolled.
pub(crate) fn plan(new: &A11yNode, changes: &[A11yChange]) -> Vec<Post> {
    let post = |path: &[usize], name, bar| Post {
        path: path.to_vec(),
        name,
        bar,
    };
    let mut out = Vec::new();
    let mut layout = false;
    for change in changes {
        match change {
            A11yChange::Name(p) => out.push(post(p, "AXTitleChanged", false)),
            A11yChange::Value(p) | A11yChange::Checked(p) => {
                out.push(post(p, "AXValueChanged", false));
            }
            // (A number input's value is its text: its Value change posts.)
            A11yChange::Range(p) => {
                if new.at_path(p).is_some_and(|n| range_value(n).is_some()) {
                    out.push(post(p, "AXValueChanged", false));
                }
            }
            A11yChange::TextSelection(p) => out.push(post(p, "AXSelectedTextChanged", false)),
            A11yChange::Selected(p) => {
                let parent = &p[..p.len().saturating_sub(1)];
                out.push(post(parent, "AXSelectedChildrenChanged", false));
                let rows = new.at_path(parent).is_some_and(|n| {
                    matches!(n.role, A11yRole::List | A11yRole::Table | A11yRole::Tree)
                });
                if rows {
                    out.push(post(parent, "AXSelectedRowsChanged", false));
                }
            }
            A11yChange::Expanded(p) => {
                let open = new.at_path(p).and_then(|n| n.expanded) == Some(true);
                let name = if open {
                    "AXRowExpanded"
                } else {
                    "AXRowCollapsed"
                };
                out.push(post(p, name, false));
            }
            A11yChange::Scrolled(p) => {
                if new.at_path(p).is_some_and(scrolls) {
                    out.push(post(p, "AXValueChanged", true));
                }
                layout = true;
            }
            A11yChange::Children(_) => layout = true,
        }
    }
    if out.len() > MAX_POSTS {
        out.clear();
        layout = true;
    }
    if layout {
        out.push(post(&[], "AXLayoutChanged", false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::access::{A11yScroll, changes};

    fn node(role: A11yRole, name: &str) -> A11yNode {
        A11yNode::new(role, name, (0.0, 0.0, 100.0, 20.0))
    }

    #[test]
    fn text_ranges_count_utf16_units() {
        let s = "a😀é";
        assert_eq!(utf16_len(s), 4);
        assert_eq!(utf16_at(s, 1), 1);
        assert_eq!(utf16_at(s, 5), 3);
        assert_eq!(utf16_at(s, 3), 1, "inside the emoji: its start");
        assert_eq!(byte_at(s, 3), 5);
        assert_eq!(byte_at(s, 2), 1, "inside the pair: the emoji's start");
        assert_eq!(byte_at(s, 99), s.len());
        let field = A11yNode {
            caret: Some(5),
            text_selection: Some((1, 5)),
            ..node(A11yRole::TextField, s)
        };
        assert_eq!(selected_range(&field), Some((1, 2)));
        assert_eq!(range_bytes(&field, 1, 2), (1, 5));
        let caret = A11yNode {
            text_selection: None,
            ..field.clone()
        };
        assert_eq!(selected_range(&caret), Some((3, 0)));
        // A labelled field's name isn't its text: no value, no range.
        let labelled = node(A11yRole::TextField, "Search");
        assert_eq!(text_value(&labelled), None);
        assert_eq!(selected_range(&labelled), None);
    }

    #[test]
    fn values_labels_and_roles() {
        let check = |role, on| A11yNode {
            checked: Some(on),
            ..node(role, "x")
        };
        assert_eq!(number_value(&check(A11yRole::CheckBox, true)), Some(1));
        assert_eq!(number_value(&check(A11yRole::Switch, false)), Some(0));
        assert_eq!(number_value(&node(A11yRole::Button, "OK")), None);
        assert_eq!(subrole(&check(A11yRole::Switch, true)), Some("AXSwitch"));
        assert_eq!(
            subrole(&node(A11yRole::PasswordField, "")),
            Some("AXSecureTextField")
        );
        let root = A11yNode {
            children: vec![
                A11yNode {
                    id: Some("cap".into()),
                    ..node(A11yRole::Text, "Note")
                },
                A11yNode {
                    caret: Some(0),
                    labelled_by: Some("cap".into()),
                    ..node(A11yRole::TextField, "hi")
                },
            ],
            ..node(A11yRole::Window, "")
        };
        let field = &root.children[1];
        assert_eq!(field_label(field, Some(&root)).as_deref(), Some("Note"));
        let bare = A11yNode {
            labelled_by: None,
            ..field.clone()
        };
        assert_eq!(field_label(&bare, Some(&root)).as_deref(), Some(""));
        assert_eq!(field_label(&root.children[0], Some(&root)), None);
        let scroller = A11yNode {
            scroll: Some(A11yScroll {
                offset: 0.0,
                max: 50.0,
                viewport: 20.0,
                horizontal: false,
                rtl: false,
            }),
            ..node(A11yRole::Group, "")
        };
        assert_eq!(role_override(&scroller), Some("AXScrollArea"));
        assert_eq!(role_override(&node(A11yRole::Group, "")), None);
        assert_eq!(
            bar_frame((10.0, 20.0, 100.0, 50.0), false, false),
            [98.0, 20.0, 12.0, 50.0]
        );
        assert_eq!(
            bar_frame((10.0, 20.0, 100.0, 50.0), true, false),
            [10.0, 20.0, 100.0, 12.0]
        );
    }

    #[test]
    fn a_swap_posts_what_changed() {
        let scroll = |offset| {
            Some(A11yScroll {
                offset,
                max: 300.0,
                viewport: 100.0,
                horizontal: false,
                rtl: false,
            })
        };
        let tree = |text: &str, on, offset, selected| A11yNode {
            children: vec![
                A11yNode {
                    caret: Some(text.len()),
                    ..node(A11yRole::TextField, text)
                },
                A11yNode {
                    checked: Some(on),
                    ..node(A11yRole::CheckBox, "Remember")
                },
                A11yNode {
                    scroll: scroll(offset),
                    children: vec![A11yNode {
                        selected,
                        ..node(A11yRole::ListItem, "Row 0")
                    }],
                    ..node(A11yRole::List, "")
                },
            ],
            ..node(A11yRole::Window, "")
        };
        let old = tree("a", false, 0.0, false);
        let new = tree("ab", true, 40.0, true);
        let posts: Vec<_> = plan(&new, &changes(&old, &new))
            .into_iter()
            .map(|p| (p.path, p.name, p.bar))
            .collect();
        assert_eq!(
            posts,
            [
                (vec![0], "AXTitleChanged", false),
                (vec![0], "AXValueChanged", false),
                (vec![0], "AXSelectedTextChanged", false),
                (vec![1], "AXValueChanged", false),
                (vec![2], "AXValueChanged", true),
                (vec![2], "AXSelectedChildrenChanged", false),
                (vec![2], "AXSelectedRowsChanged", false),
                (vec![], "AXLayoutChanged", false),
            ]
        );
    }

    #[test]
    fn sliders_progress_bars_and_tables() {
        use crate::platform::access::{A11yCell, A11yRange, A11yTable};
        let range = |value| {
            Some(A11yRange {
                value,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                rtl: false,
            })
        };
        let slider = A11yNode {
            range: range(0.5),
            ..node(A11yRole::Slider, "Volume")
        };
        assert_eq!(range_value(&slider), Some((0.5, 0.0, 1.0)));
        assert_eq!(value_description(&slider).as_deref(), Some("50%"));
        assert!(range_settable(&slider));
        let bar = A11yNode {
            range: range(0.3),
            ..node(A11yRole::ProgressBar, "")
        };
        assert!(range_value(&bar).is_some() && !range_settable(&bar));
        // A number input's value is its text.
        let spin = A11yNode {
            caret: Some(1),
            range: range(1.0),
            ..node(A11yRole::SpinButton, "1")
        };
        assert_eq!(range_value(&spin), None);
        let table = A11yNode {
            table: Some(A11yTable {
                rows: 5,
                columns: 2,
                frozen: 0,
            }),
            children: vec![A11yNode {
                children: vec![A11yNode {
                    cell: Some(A11yCell { row: 3, column: 0 }),
                    children: vec![A11yNode {
                        cell: Some(A11yCell { row: 3, column: 1 }),
                        ..node(A11yRole::Cell, "b")
                    }],
                    ..node(A11yRole::Row, "")
                }],
                ..node(A11yRole::Group, "")
            }],
            ..node(A11yRole::Table, "")
        };
        assert_eq!(table_size(&table), Some((5, 2)));
        assert_eq!(table_rows(&table), [vec![0, 0]]);
        let row = &table.children[0].children[0];
        assert_eq!(row_index(row), Some(3));
        assert_eq!(cell_index(&row.children[0]), Some((3, 1)));
        // A slider moved posts a value change; a number input's text does.
        let window = |n: A11yNode| A11yNode {
            children: vec![n],
            ..node(A11yRole::Window, "")
        };
        let moved = A11yNode {
            range: range(0.75),
            ..slider.clone()
        };
        let (old, new) = (window(slider), window(moved));
        assert_eq!(
            plan(&new, &changes(&old, &new)),
            [Post {
                path: vec![0],
                name: "AXValueChanged",
                bar: false
            }]
        );
    }
}
