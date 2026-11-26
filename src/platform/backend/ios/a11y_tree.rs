//! The accessibility tree as iOS's VoiceOver gets it, kept free of UIKit so it
//! is unit-tested on every host: one item per node of the tree (the root, the
//! view itself, aside), in reading order, each with what its
//! `UIAccessibilityElement` says, and which notification an update needs.
//!
//! A table's element is a data table (`UIAccessibilityContainerDataTable`):
//! it vends the elements of the nodes inside it (the view vends the rest),
//! says its size and finds a cell by row and column; each cell says its row
//! and column. A control labelled by a caption reads the caption in its
//! label.

use crate::platform::access::{A11yCell, A11yNode, A11yRole, A11yTable};

/// `UIAccessibilityTraitButton`, `…StaticText`, `…Header` and `…Selected`,
/// as bits the backend maps to UIKit's constants.
pub(crate) const BUTTON: u8 = 1;
pub(crate) const STATIC_TEXT: u8 = 2;
pub(crate) const HEADER: u8 = 4;
pub(crate) const SELECTED: u8 = 8;
/// A text field: UIKit has no public trait for it, so the backend borrows
/// the one a `UITextField` reports (the text-entry trait VoiceOver reads as
/// "text field", and XCUITest as `.textField`).
pub(crate) const TEXT_ENTRY: u8 = 16;
/// `UIAccessibilityTraitAdjustable`: a slider, which VoiceOver's swipe up
/// and down step (`accessibilityIncrement` / `…Decrement`).
pub(crate) const ADJUSTABLE: u8 = 32;

/// One node, as its accessibility element shows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item {
    pub(crate) role: A11yRole,
    /// Whether VoiceOver stops on it: containers (a group, a list, a
    /// toolbar…) and separators don't, their children do.
    pub(crate) is_element: bool,
    pub(crate) label: String,
    pub(crate) value: String,
    /// `accessibilityHint`: a field's placeholder, which VoiceOver reads
    /// after a pause, past the (empty) value and the role.
    pub(crate) hint: String,
    pub(crate) traits: u8,
    /// `(x, y, width, height)` in the view's coordinates (logical pixels).
    pub(crate) frame: (f64, f64, f64, f64),
    /// The live OS control that stands for the node, by id: the container
    /// vends that `UIControl` (which speaks for itself) in its place.
    pub(crate) native: Option<u64>,
    /// For an adjustable item (a slider), its node without its children:
    /// what an increment or decrement steps (see `platform::access`).
    pub(crate) adjust: Option<A11yNode>,
    /// The table item (by index) whose element vends this one; `None` for
    /// the view's.
    pub(crate) container: Option<usize>,
    /// For a table, its size (all its rows, those not built too).
    pub(crate) table: Option<A11yTable>,
    /// For a table's row, cell or column header, where it is.
    pub(crate) cell: Option<A11yCell>,
    /// Its place in the tree (child indices from the root), where an
    /// activation brings it into view (`access::reveal`).
    pub(crate) path: Vec<usize>,
}

/// The roles whose children VoiceOver reads instead of them (as
/// `core::a11y`'s containers).
fn is_container(role: A11yRole) -> bool {
    use A11yRole::*;
    matches!(
        role,
        Window
            | Group
            | List
            | Table
            | Row
            | Tree
            | Toolbar
            | Alert
            | MenuBar
            | Menu
            | TabList
            | Dialog
    )
}

fn item(node: &A11yNode) -> Item {
    use A11yRole::*;
    let role = node.role;
    // A text field's name is its text: that's the element's value.
    let field = matches!(role, TextField | SpinButton | PasswordField);
    let (label, value) = if field {
        (String::new(), node.name.clone())
    } else {
        // In the user's language (VoiceOver reads them as they are).
        use crate::locale::{Builtin, builtin};
        let pick = |yes: bool, a: Builtin, b: Builtin| builtin(if yes { a } else { b }).to_string();
        let value = match (role, node.checked, node.expanded) {
            // A slider's or a progress bar's value, as words ("50%").
            _ if node.range.is_some() => node.range.map(|r| r.text()).unwrap_or_default(),
            (Switch, Some(on), _) => pick(on, Builtin::On, Builtin::Off),
            (_, Some(checked), _) => pick(checked, Builtin::Checked, Builtin::Unchecked),
            (_, None, Some(open)) => pick(open, Builtin::Expanded, Builtin::Collapsed),
            _ => String::new(),
        };
        (node.name.clone(), value)
    };
    let mut traits = match role {
        Button | MenuItem | ComboBox | CheckBox | RadioButton | Switch | Tab => BUTTON,
        Text => STATIC_TEXT,
        ColumnHeader => HEADER,
        TextField | SpinButton | PasswordField => TEXT_ENTRY,
        _ => 0,
    };
    let adjust = (node.range_settable() && role == Slider).then(|| A11yNode {
        children: Vec::new(),
        ..node.clone()
    });
    if adjust.is_some() {
        traits |= ADJUSTABLE;
    }
    if node.selected && matches!(role, ListItem | Row | Cell | TreeItem | Tab) {
        traits |= SELECTED;
    }
    // UIKit has no radio trait: the picked option of a group reads as selected.
    if role == RadioButton && node.checked == Some(true) {
        traits |= SELECTED;
    }
    let is_element = match role {
        // UIKit's scroll views have no scroll bar elements: VoiceOver
        // scrolls with three fingers (`accessibilityScroll:`).
        Separator | ScrollBar => false,
        _ if is_container(role) => node.children.is_empty() && !node.name.is_empty(),
        Text => !node.name.is_empty(),
        _ => true,
    };
    Item {
        role,
        is_element,
        label,
        value,
        hint: node.hint().unwrap_or_default().to_string(),
        traits,
        frame: node.bounds,
        native: node.native_view,
        adjust,
        container: None,
        table: node.table.filter(|_| role == Table),
        cell: node.cell,
        path: Vec::new(),
    }
}

/// The text of the caption `node` is labelled by, in its label: a field's
/// (whose label is empty: its name is its text) or an unnamed control's is
/// the caption; a named one's starts with it, unless its name says it.
fn add_caption(root: &A11yNode, node: &A11yNode, it: &mut Item) {
    let Some(caption) = node
        .labelled_by
        .as_deref()
        .and_then(|id| root.path_of_id(id))
        .and_then(|p| root.at_path(&p))
        .map(|c| c.name.trim())
        .filter(|c| !c.is_empty())
    else {
        return;
    };
    if it.label.is_empty() {
        it.label = caption.to_string();
    } else if !it.label.contains(caption) {
        it.label = format!("{caption}, {}", it.label);
    }
}

/// Every node under `root` (not `root`, which the view stands for), depth
/// first in reading order.
pub(crate) fn flatten(root: &A11yNode) -> Vec<Item> {
    fn walk(
        root: &A11yNode,
        node: &A11yNode,
        path: &mut Vec<usize>,
        table: Option<usize>,
        out: &mut Vec<Item>,
    ) {
        for (i, child) in node.children.iter().enumerate() {
            path.push(i);
            let index = out.len();
            let mut it = item(child);
            it.container = table;
            it.path = path.clone();
            add_caption(root, child, &mut it);
            out.push(it);
            let inner = if child.role == A11yRole::Table {
                Some(index)
            } else {
                table
            };
            walk(root, child, path, inner, out);
            path.pop();
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut Vec::new(), None, &mut out);
    out
}

/// The items `container`'s element vends (the view's for `None`), by index,
/// in reading order.
pub(crate) fn vended_by(items: &[Item], container: Option<usize>) -> Vec<usize> {
    (0..items.len())
        .filter(|&i| items[i].container == container)
        .collect()
}

/// The cell (or column header) of table item `table` at `row` and
/// `column`, by index: `accessibilityDataTableCellElementForRow:column:`.
/// `None` for a row the table hasn't built.
pub(crate) fn cell_at(items: &[Item], table: usize, row: usize, column: usize) -> Option<usize> {
    (0..items.len()).find(|&i| {
        let it = &items[i];
        it.container == Some(table)
            && matches!(it.role, A11yRole::Cell | A11yRole::ColumnHeader)
            && it.cell.is_some_and(|c| c.row == row && c.column == column)
    })
}

/// The column headers of table item `table` over `column`, by index:
/// `accessibilityHeaderElementsForColumn:`.
pub(crate) fn column_headers(items: &[Item], table: usize, column: usize) -> Vec<usize> {
    (0..items.len())
        .filter(|&i| {
            let it = &items[i];
            it.container == Some(table)
                && it.role == A11yRole::ColumnHeader
                && it.cell.is_some_and(|c| c.column == column)
        })
        .collect()
}

/// A scroll container an item sits in (or is), as VoiceOver's
/// three-finger swipe scrolls it: the container's node without its
/// children (its bounds and [`scroll`](A11yNode::scroll)).
pub(crate) type Scroller = A11yNode;

/// For each item [`flatten`] gives, in the same order, the scroll containers
/// around it, innermost first (the node itself first when it is one), that
/// scroll at all.
pub(crate) fn scrollers(root: &A11yNode) -> Vec<Vec<Scroller>> {
    fn walk(node: &A11yNode, around: &mut Vec<Scroller>, out: &mut Vec<Vec<Scroller>>) {
        for child in &node.children {
            let own = child.scroll.filter(|s| s.scrolls()).map(|s| A11yNode {
                scroll: Some(s),
                ..A11yNode::new(child.role, "", child.bounds)
            });
            let pushed = own.is_some();
            if let Some(own) = own {
                around.push(own);
            }
            out.push(around.iter().rev().cloned().collect());
            walk(child, around, out);
            if pushed {
                around.pop();
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut Vec::new(), &mut out);
    out
}

/// `UIAccessibilityScrollDirection`: which way VoiceOver's three fingers
/// swiped.
pub(crate) const SCROLL_RIGHT: i64 = 1;
pub(crate) const SCROLL_LEFT: i64 = 2;
pub(crate) const SCROLL_UP: i64 = 3;
pub(crate) const SCROLL_DOWN: i64 = 4;
pub(crate) const SCROLL_NEXT: i64 = 5;
pub(crate) const SCROLL_PREVIOUS: i64 = 6;

/// What a swipe in `direction` scrolls, from the containers around an item
/// (innermost first): the first one on the swipe's axis that can move that
/// way, and whether forward (towards its end). Fingers swiping up bring the
/// next page up (forward), left the next page in from the right (forward,
/// except in a right-to-left strip, whose next page is on the left).
pub(crate) fn scroll_target(around: &[Scroller], direction: i64) -> Option<(&Scroller, bool)> {
    around.iter().find_map(|n| {
        let s = n.scroll?;
        let forward = match (direction, s.horizontal) {
            (SCROLL_UP, false) => true,
            (SCROLL_DOWN, false) => false,
            (SCROLL_LEFT, true) => !s.rtl,
            (SCROLL_RIGHT, true) => s.rtl,
            (SCROLL_NEXT, _) => true,
            (SCROLL_PREVIOUS, _) => false,
            _ => return None,
        };
        let room = if forward {
            s.offset < s.max - 0.5
        } else {
            s.offset > 0.5
        };
        room.then_some((n, forward))
    })
}

/// What VoiceOver says after a scroll (`UIAccessibilityPageScrolledNotification`):
/// "Page N of M", the page the offset will be on after scrolling `forward`.
pub(crate) fn page_status(n: &Scroller, forward: bool) -> String {
    let Some(s) = n.scroll else {
        return String::new();
    };
    let step = s.page();
    let offset = (s.offset + if forward { step } else { -step }).clamp(0.0, s.max);
    // Pages of a viewport each: the last one ends at the content's end.
    let view = s.viewport.max(1.0);
    let pages = ((s.max + view) / view).ceil().max(1.0) as usize;
    let at = if offset >= s.max - 0.5 {
        pages
    } else {
        ((offset / view).round() as usize + 1).min(pages)
    };
    format!("Page {at} of {pages}")
}

/// What VoiceOver must hear of an update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// Nothing, or only frames moved (scrolling): the elements carry the new
    /// frames when asked.
    None,
    /// Elements came, went or changed: `UIAccessibilityLayoutChangedNotification`.
    Layout,
    /// A different screen: the first tree, or a dialog opened or closed.
    /// `UIAccessibilityScreenChangedNotification`.
    Screen,
}

/// The alerts (toasts) in `new` that weren't in `old`, by label, in reading
/// order: what VoiceOver announces (`UIAccessibilityAnnouncementNotification`)
/// rather than moving its cursor to them. None for the first tree.
pub(crate) fn new_alerts(old: &[Item], new: &[Item]) -> Vec<String> {
    if old.is_empty() {
        return Vec::new();
    }
    let alerts = |items: &[Item]| -> Vec<String> {
        items
            .iter()
            .filter(|i| i.role == A11yRole::Alert && !i.label.is_empty())
            .map(|i| i.label.clone())
            .collect()
    };
    let mut before = alerts(old);
    alerts(new)
        .into_iter()
        .filter(|label| match before.iter().position(|b| b == label) {
            Some(i) => {
                before.swap_remove(i);
                false
            }
            None => true,
        })
        .collect()
}

pub(crate) fn change(old: &[Item], new: &[Item]) -> Change {
    let dialogs = |items: &[Item]| items.iter().filter(|i| i.role == A11yRole::Dialog).count();
    if old.is_empty() && !new.is_empty() || dialogs(old) != dialogs(new) {
        return Change::Screen;
    }
    let same = old.len() == new.len()
        && old.iter().zip(new).all(|(a, b)| {
            a.role == b.role
                && a.is_element == b.is_element
                && a.label == b.label
                && a.value == b.value
                && a.hint == b.hint
                && a.traits == b.traits
        });
    if same { Change::None } else { Change::Layout }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: A11yRole, name: &str) -> A11yNode {
        A11yNode::new(role, name, (1.0, 2.0, 30.0, 10.0))
    }

    fn window(children: Vec<A11yNode>) -> A11yNode {
        A11yNode {
            children,
            ..node(A11yRole::Window, "App")
        }
    }

    #[test]
    fn one_item_per_node_with_label_value_and_traits() {
        let tree = window(vec![
            node(A11yRole::TextField, "hello"),
            A11yNode {
                children: vec![
                    A11yNode {
                        selected: true,
                        ..node(A11yRole::ListItem, "One")
                    },
                    node(A11yRole::Button, "Two"),
                ],
                ..node(A11yRole::List, "")
            },
            A11yNode {
                checked: Some(true),
                ..node(A11yRole::Switch, "Wi-Fi")
            },
            node(A11yRole::Separator, ""),
        ]);
        let items = flatten(&tree);
        let summary: Vec<_> = items
            .iter()
            .map(|i| (i.is_element, i.label.as_str(), i.value.as_str(), i.traits))
            .collect();
        assert_eq!(
            summary,
            [
                (true, "", "hello", TEXT_ENTRY),
                (false, "", "", 0),
                (true, "One", "", SELECTED),
                (true, "Two", "", BUTTON),
                (true, "Wi-Fi", "on", BUTTON),
                (false, "", "", 0),
            ]
        );
        assert_eq!(items[0].frame, (1.0, 2.0, 30.0, 10.0));
        assert!(items.iter().all(|i| i.hint.is_empty()));
        assert!(items.iter().all(|i| i.native.is_none()));
    }

    #[test]
    fn a_slider_is_adjustable_and_reads_its_value() {
        use crate::platform::access::A11yRange;
        let range = |value| {
            Some(A11yRange {
                value,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                rtl: false,
            })
        };
        let tree = window(vec![
            A11yNode {
                range: range(0.5),
                ..node(A11yRole::Slider, "Volume")
            },
            A11yNode {
                range: range(0.25),
                ..node(A11yRole::ProgressBar, "Loading")
            },
        ]);
        let items = flatten(&tree);
        assert_eq!(
            (items[0].label.as_str(), items[0].value.as_str()),
            ("Volume", "50%")
        );
        assert_eq!(items[0].traits & ADJUSTABLE, ADJUSTABLE);
        let up = items[0].adjust.as_ref().unwrap().step_range_events(true);
        assert_eq!(up.map(|e| e.len()), Some(4), "a press at 55%");
        assert_eq!(items[1].value, "25%");
        assert!(items[1].adjust.is_none(), "a progress bar is read-only");
        assert_eq!(items[1].traits & ADJUSTABLE, 0);
    }

    #[test]
    fn a_node_a_live_control_stands_for_names_it() {
        let tree = window(vec![A11yNode {
            native_view: Some(42),
            checked: Some(false),
            ..node(A11yRole::Switch, "Sync")
        }]);
        let items = flatten(&tree);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].native, Some(42));
        assert_eq!(items[0].label, "Sync");
    }

    #[test]
    fn three_finger_swipes_scroll_the_container_on_their_axis() {
        use crate::platform::access::A11yScroll;
        let scroll = |offset, horizontal, rtl| {
            Some(A11yScroll {
                offset,
                max: 300.0,
                viewport: 100.0,
                horizontal,
                rtl,
            })
        };
        let strip = A11yNode {
            scroll: scroll(0.0, true, false),
            children: vec![node(A11yRole::Button, "Chip 0")],
            ..node(A11yRole::Group, "")
        };
        let list = A11yNode {
            scroll: scroll(120.0, false, false),
            children: vec![node(A11yRole::Button, "Row 0"), strip],
            ..node(A11yRole::Group, "")
        };
        let tree = window(vec![node(A11yRole::Text, "Title"), list]);
        let around = scrollers(&tree);
        assert_eq!(around.len(), flatten(&tree).len());
        assert!(around[0].is_empty(), "the title is in no container");
        // The list itself, its row, the strip, the chip.
        let lens: Vec<_> = around.iter().map(Vec::len).collect();
        assert_eq!(lens, [0, 1, 1, 2, 2]);
        let chip = &around[4];
        // Up and down pass the sideways strip by, to the list.
        let (n, forward) = scroll_target(chip, SCROLL_UP).unwrap();
        assert!(!n.scroll.unwrap().horizontal && forward);
        let (n, forward) = scroll_target(chip, SCROLL_LEFT).unwrap();
        assert!(n.scroll.unwrap().horizontal && forward);
        // The strip is at its start: nothing scrolls it back.
        assert!(scroll_target(chip, SCROLL_RIGHT).is_none());
        assert_eq!(page_status(&around[1][0], true), "Page 3 of 4");
        assert_eq!(page_status(&around[1][0], false), "Page 1 of 4");
    }

    #[test]
    fn a_table_vends_its_cells_and_finds_them_by_row_and_column() {
        let cell = |role, name: &str, row, column| A11yNode {
            cell: Some(A11yCell { row, column }),
            ..node(role, name)
        };
        let row = |index, cells: Vec<A11yNode>| A11yNode {
            cell: Some(A11yCell {
                row: index,
                column: 0,
            }),
            children: cells,
            ..node(A11yRole::Row, "")
        };
        let tree = window(vec![
            node(A11yRole::Text, "Title"),
            A11yNode {
                table: Some(A11yTable {
                    rows: 21,
                    columns: 2,
                    frozen: 0,
                }),
                children: vec![
                    row(
                        0,
                        vec![
                            cell(A11yRole::ColumnHeader, "Name", 0, 0),
                            cell(A11yRole::ColumnHeader, "Value", 0, 1),
                        ],
                    ),
                    // A virtualized table's rows 5 and 6, in a group.
                    A11yNode {
                        children: vec![
                            row(5, vec![cell(A11yRole::Cell, "Item 5", 5, 0)]),
                            row(6, vec![cell(A11yRole::Cell, "Item 6", 6, 0)]),
                        ],
                        ..node(A11yRole::Group, "")
                    },
                ],
                ..node(A11yRole::Table, "")
            },
            node(A11yRole::Button, "After"),
        ]);
        let items = flatten(&tree);
        let named = |name: &str| items.iter().position(|i| i.label == name).unwrap();
        let table = items
            .iter()
            .position(|i| i.role == A11yRole::Table)
            .unwrap();
        assert_eq!(
            items[table].table,
            Some(A11yTable {
                rows: 21,
                columns: 2,
                frozen: 0,
            })
        );
        // The view vends the title, the table and the button; the table the
        // rest.
        let view: Vec<_> = vended_by(&items, None);
        assert_eq!(view, [named("Title"), table, named("After")]);
        let inside = vended_by(&items, Some(table));
        assert!(inside.contains(&named("Item 6")) && inside.contains(&named("Name")));
        assert_eq!(inside.len(), items.len() - 3);
        assert_eq!(cell_at(&items, table, 6, 0), Some(named("Item 6")));
        assert_eq!(cell_at(&items, table, 0, 1), Some(named("Value")));
        assert_eq!(cell_at(&items, table, 9, 0), None, "a row not built");
        assert_eq!(column_headers(&items, table, 1), [named("Value")]);
        assert_eq!(
            items[named("Item 5")].cell,
            Some(A11yCell { row: 5, column: 0 })
        );
        assert_eq!(items[named("Item 5")].path, [1, 1, 0, 0]);
        assert_eq!(items[named("Name")].traits & HEADER, HEADER);
    }

    #[test]
    fn a_caption_is_in_the_controls_label() {
        let caption = |id: &str, name: &str| A11yNode {
            id: Some(id.into()),
            ..node(A11yRole::Text, name)
        };
        let by = |id: &str, n: A11yNode| A11yNode {
            labelled_by: Some(id.into()),
            ..n
        };
        let tree = window(vec![
            caption("level", "Level"),
            by("level", node(A11yRole::SpinButton, "3")),
            caption("vol", "Volume"),
            by("vol", node(A11yRole::Slider, "Volume")),
            caption("fruit", "Fruit"),
            by("fruit", node(A11yRole::ComboBox, "Apple")),
            by("nowhere", node(A11yRole::Button, "OK")),
        ]);
        let items = flatten(&tree);
        let labels: Vec<_> = items
            .iter()
            .map(|i| (i.label.as_str(), i.value.as_str()))
            .collect();
        assert_eq!(
            labels,
            [
                ("Level", ""),
                ("Level", "3"),
                ("Volume", ""),
                ("Volume", ""),
                ("Fruit", ""),
                ("Fruit, Apple", ""),
                ("OK", ""),
            ]
        );
    }

    #[test]
    fn a_placeholder_is_the_fields_hint() {
        let field = |text: &str, placeholder: &str| A11yNode {
            placeholder: Some(placeholder.into()),
            ..node(A11yRole::TextField, text)
        };
        let empty = flatten(&window(vec![field("", "Your name")]));
        assert_eq!(
            (empty[0].value.as_str(), empty[0].hint.as_str()),
            ("", "Your name")
        );
        // Typing keeps the hint; another hint is a layout change.
        let typed = flatten(&window(vec![field("Ada", "Your name")]));
        assert_eq!(typed[0].hint, "Your name");
        let other = flatten(&window(vec![field("", "Search")]));
        assert_eq!(change(&empty, &other), Change::Layout);
    }

    #[test]
    fn updates_post_the_notification_they_need() {
        let first = flatten(&window(vec![node(A11yRole::Button, "OK")]));
        assert_eq!(change(&[], &first), Change::Screen);
        assert_eq!(change(&first, &first), Change::None);
        // Scrolling moves frames only.
        let moved = flatten(&window(vec![A11yNode::new(
            A11yRole::Button,
            "OK",
            (5.0, 50.0, 30.0, 10.0),
        )]));
        assert_eq!(change(&first, &moved), Change::None);
        let renamed = flatten(&window(vec![node(A11yRole::Button, "Done")]));
        assert_eq!(change(&first, &renamed), Change::Layout);
        let dialog = flatten(&window(vec![
            node(A11yRole::Button, "OK"),
            A11yNode {
                children: vec![node(A11yRole::Button, "Yes")],
                ..node(A11yRole::Dialog, "Sure?")
            },
        ]));
        assert_eq!(change(&first, &dialog), Change::Screen);
        // A toast is announced, without changing the screen.
        let toast = flatten(&window(vec![
            node(A11yRole::Button, "OK"),
            A11yNode {
                children: vec![node(A11yRole::Button, "Undo")],
                ..node(A11yRole::Alert, "Deleted")
            },
        ]));
        assert_eq!(change(&first, &toast), Change::Layout);
        assert_eq!(new_alerts(&first, &toast), ["Deleted"]);
        assert!(new_alerts(&toast, &toast).is_empty());
        assert!(new_alerts(&[], &toast).is_empty(), "the first tree");
        assert!(new_alerts(&toast, &first).is_empty(), "gone");
    }
}
