//! Where an item is in its set, the "3 of 12" a screen reader reads, worked
//! out once from the tree for every bridge (AT-SPI's `posinset` / `setsize`
//! attributes, UIA's `PositionInSet` / `SizeOfSet`, Android's
//! `CollectionItemInfo`, `aria-posinset` / `aria-setsize`).
//!
//! An item is a list item, a tree item, a tab, a radio button or a menu
//! item. Its widget may declare its place ([`A11yNode::position`]: a
//! virtualized list builds only the rows in view, so their siblings aren't
//! all there to count); otherwise its set is counted from the tree:
//!
//! - a radio button's: its group (the radio buttons side by side under one
//!   parent, as the member-of relation);
//! - a tree item's in a flat tree (items side by side, each with its
//!   [`level`](A11yNode::level)): the items of its level between its parent
//!   item and the next item a level up;
//! - otherwise the items of its role under the nearest container of its
//!   kind (a list's items, a tab list's tabs, a menu's items, a tree's or a
//!   tree item's child items), through the groups between, not inside a
//!   nested item or container.

// Not every bridge (nor every target) uses it; the tests do.
#![allow(dead_code)]

use super::{A11yNode, A11yRole};

/// An item's place in its set: its position, from 1, and the set's size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SetPosition {
    pub(crate) position: usize,
    pub(crate) size: usize,
}

/// The containers an item of `role` is counted in, when it is an item.
fn containers(role: A11yRole) -> Option<&'static [A11yRole]> {
    use A11yRole::*;
    Some(match role {
        ListItem => &[List],
        TreeItem => &[Tree, TreeItem],
        Tab => &[TabList],
        MenuItem => &[Menu, MenuBar],
        RadioButton => &[],
        _ => return None,
    })
}

impl A11yNode {
    /// The place of the item at `path` (under this root) in its set (see the
    /// module docs). `None` for a node that isn't an item, or isn't there.
    pub(crate) fn set_position(&self, path: &[usize]) -> Option<SetPosition> {
        let n = self.at_path(path)?;
        let kinds = containers(n.role)?;
        if let Some(p) = n.position {
            return Some(SetPosition {
                position: p.index + 1,
                size: p.count.max(p.index + 1),
            });
        }
        let set = if n.role == A11yRole::RadioButton {
            self.radio_group(path)
        } else if let (A11yRole::TreeItem, Some(level)) = (n.role, n.level) {
            self.flat_tree_level(path, level)
        } else {
            self.items_in_container(path, n.role, kinds)
        };
        let at = set.iter().position(|p| p == path)?;
        Some(SetPosition {
            position: at + 1,
            size: set.len(),
        })
    }

    /// The items of `level` beside the flat tree item at `path`: from the
    /// one after its parent item to the one before the next item a level up.
    fn flat_tree_level(&self, path: &[usize], level: usize) -> Vec<Vec<usize>> {
        let Some((&index, parent)) = path.split_last() else {
            return Vec::new();
        };
        let Some(p) = self.at_path(parent) else {
            return Vec::new();
        };
        let items: Vec<(usize, usize)> = p
            .children
            .iter()
            .enumerate()
            .filter(|(_, c)| c.role == A11yRole::TreeItem)
            .map(|(i, c)| (i, c.level.unwrap_or(1)))
            .collect();
        let Some(me) = items.iter().position(|&(i, _)| i == index) else {
            return Vec::new();
        };
        let start = items[..me]
            .iter()
            .rposition(|&(_, l)| l < level)
            .map_or(0, |k| k + 1);
        let end = items[me..]
            .iter()
            .position(|&(_, l)| l < level)
            .map_or(items.len(), |k| me + k);
        items[start..end]
            .iter()
            .filter(|&&(_, l)| l == level)
            .map(|&(i, _)| {
                let mut q = parent.to_vec();
                q.push(i);
                q
            })
            .collect()
    }

    /// The nodes of `role` under the nearest node above `path` whose role
    /// is one of `kinds` (its parent when there is none), through the nodes
    /// between: not inside another node of `role` or of `kinds`.
    fn items_in_container(
        &self,
        path: &[usize],
        role: A11yRole,
        kinds: &[A11yRole],
    ) -> Vec<Vec<usize>> {
        let Some((_, parent)) = path.split_last() else {
            return Vec::new();
        };
        let above = (0..path.len())
            .rev()
            .map(|k| &path[..k])
            .find(|p| self.at_path(p).is_some_and(|a| kinds.contains(&a.role)))
            .unwrap_or(parent);
        fn go(
            n: &A11yNode,
            role: A11yRole,
            kinds: &[A11yRole],
            path: &mut Vec<usize>,
            out: &mut Vec<Vec<usize>>,
        ) {
            for (i, c) in n.children.iter().enumerate() {
                path.push(i);
                if c.role == role {
                    out.push(path.clone());
                } else if !kinds.contains(&c.role) {
                    go(c, role, kinds, path, out);
                }
                path.pop();
            }
        }
        let mut out = Vec::new();
        if let Some(c) = self.at_path(above) {
            go(c, role, kinds, &mut above.to_vec(), &mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::A11yPosition;
    use super::*;

    fn leaf(role: A11yRole, name: &str) -> A11yNode {
        A11yNode::new(role, name, (0.0, 0.0, 10.0, 10.0))
    }

    fn group(role: A11yRole, children: Vec<A11yNode>) -> A11yNode {
        A11yNode {
            children,
            ..leaf(role, "")
        }
    }

    fn at(position: usize, size: usize) -> Option<SetPosition> {
        Some(SetPosition { position, size })
    }

    #[test]
    fn a_lists_items_are_counted_through_their_wrappers() {
        let root = group(
            A11yRole::Window,
            vec![group(
                A11yRole::List,
                vec![
                    group(
                        A11yRole::Group,
                        vec![leaf(A11yRole::ListItem, "a"), leaf(A11yRole::ListItem, "b")],
                    ),
                    A11yNode {
                        // An item holding a list of its own: not counted in.
                        children: vec![group(A11yRole::List, vec![leaf(A11yRole::ListItem, "x")])],
                        ..leaf(A11yRole::ListItem, "c")
                    },
                    leaf(A11yRole::ScrollBar, ""),
                ],
            )],
        );
        assert_eq!(root.set_position(&[0, 0, 1]), at(2, 3));
        assert_eq!(root.set_position(&[0, 1]), at(3, 3));
        assert_eq!(root.set_position(&[0, 1, 0, 0]), at(1, 1));
        assert_eq!(root.set_position(&[0, 2]), None, "not an item");
        assert_eq!(root.set_position(&[0]), None);
        assert_eq!(root.set_position(&[9]), None);
    }

    #[test]
    fn a_declared_place_wins() {
        let row = |index| A11yNode {
            position: Some(A11yPosition { index, count: 500 }),
            ..leaf(A11yRole::ListItem, "")
        };
        let root = group(
            A11yRole::Window,
            vec![group(A11yRole::List, vec![row(41), row(42)])],
        );
        assert_eq!(root.set_position(&[0, 1]), at(43, 500));
    }

    #[test]
    fn a_flat_trees_items_are_counted_by_level() {
        let item = |name: &str, level| A11yNode {
            level: Some(level),
            ..leaf(A11yRole::TreeItem, name)
        };
        let root = group(
            A11yRole::Window,
            vec![group(
                A11yRole::Tree,
                vec![
                    item("root", 1),
                    item("Music", 2),
                    item("a.wav", 3),
                    item("b.wav", 3),
                    item("Photos", 2),
                    item("Videos", 2),
                    item("other", 1),
                    item("Docs", 2),
                ],
            )],
        );
        assert_eq!(root.set_position(&[0, 0]), at(1, 2));
        assert_eq!(root.set_position(&[0, 6]), at(2, 2));
        assert_eq!(root.set_position(&[0, 1]), at(1, 3));
        assert_eq!(root.set_position(&[0, 5]), at(3, 3));
        assert_eq!(root.set_position(&[0, 3]), at(2, 2));
        assert_eq!(root.set_position(&[0, 7]), at(1, 1));
    }

    #[test]
    fn a_nested_trees_tabs_menus_and_radio_buttons() {
        let root = group(
            A11yRole::Window,
            vec![
                group(
                    A11yRole::Tree,
                    vec![
                        A11yNode {
                            children: vec![group(
                                A11yRole::Group,
                                vec![leaf(A11yRole::TreeItem, "b"), leaf(A11yRole::TreeItem, "c")],
                            )],
                            ..leaf(A11yRole::TreeItem, "a")
                        },
                        leaf(A11yRole::TreeItem, "d"),
                    ],
                ),
                group(
                    A11yRole::TabList,
                    vec![leaf(A11yRole::Tab, "One"), leaf(A11yRole::Tab, "Two")],
                ),
                group(
                    A11yRole::Menu,
                    vec![
                        leaf(A11yRole::MenuItem, "Cut"),
                        leaf(A11yRole::Separator, ""),
                        leaf(A11yRole::MenuItem, "Paste"),
                    ],
                ),
                group(
                    A11yRole::Group,
                    vec![
                        leaf(A11yRole::RadioButton, "PNG"),
                        leaf(A11yRole::Text, "or"),
                        leaf(A11yRole::RadioButton, "JPEG"),
                    ],
                ),
            ],
        );
        assert_eq!(root.set_position(&[0, 0]), at(1, 2));
        assert_eq!(root.set_position(&[0, 1]), at(2, 2));
        assert_eq!(root.set_position(&[0, 0, 0, 1]), at(2, 2));
        assert_eq!(root.set_position(&[1, 1]), at(2, 2));
        assert_eq!(root.set_position(&[2, 2]), at(2, 2));
        assert_eq!(root.set_position(&[3, 2]), at(2, 2));
        assert_eq!(root.set_position(&[3, 1]), None);
    }
}
