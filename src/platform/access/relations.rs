//! The relations between nodes a screen reader follows, worked out once from
//! the tree for every bridge (AT-SPI's relation sets, UIA's `ControllerFor`
//! and `Level`, `AXTitleUIElement` / `AXLinkedUIElements` /
//! `AXDisclosureLevel`, `aria-controls` / `aria-level`):
//!
//! - labelled-by / label-for: a node's [`labelled_by`](A11yNode::labelled_by)
//!   names its caption's [`id`](A11yNode::id);
//! - controller-for / controlled-by: a node's
//!   [`controls`](A11yNode::controls) names the node it controls (a combo
//!   box's open list, a tab's panel);
//! - member-of: the radio buttons side by side under one parent are one
//!   group, each a member of it (itself included, as GTK reports them);
//! - node-child-of / node-parent-of: a tree item's parent item. In a flat
//!   tree (items side by side, each with its [`level`](A11yNode::level), as
//!   `widgets::tree` builds them), the nearest item before it one level up;
//!   in a nested one, the tree item around it. A top-level item has none.
//!
//! Every relation is a list of paths (child indices from the root) to the
//! nodes it names, in tree order.

// Not every bridge (nor every target) uses every helper; the tests do.
#![allow(dead_code)]

use super::{A11yNode, A11yRole};

/// A node's relations, as paths under the root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Relations {
    pub(crate) labelled_by: Vec<Vec<usize>>,
    pub(crate) label_for: Vec<Vec<usize>>,
    pub(crate) controller_for: Vec<Vec<usize>>,
    pub(crate) controlled_by: Vec<Vec<usize>>,
    pub(crate) member_of: Vec<Vec<usize>>,
    pub(crate) node_child_of: Option<Vec<usize>>,
    pub(crate) node_parent_of: Vec<Vec<usize>>,
}

impl Relations {
    /// Whether it relates the node to nothing.
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl A11yNode {
    /// The paths of the nodes under this one (itself included) that `pred`
    /// picks, in tree order.
    pub(crate) fn find_paths(&self, pred: &dyn Fn(&A11yNode) -> bool) -> Vec<Vec<usize>> {
        fn go(
            n: &A11yNode,
            pred: &dyn Fn(&A11yNode) -> bool,
            path: &mut Vec<usize>,
            out: &mut Vec<Vec<usize>>,
        ) {
            if pred(n) {
                out.push(path.clone());
            }
            for (i, c) in n.children.iter().enumerate() {
                path.push(i);
                go(c, pred, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        go(self, pred, &mut Vec::new(), &mut out);
        out
    }

    /// The path of the first node under this one whose id is `id`.
    pub(crate) fn path_of_id(&self, id: &str) -> Option<Vec<usize>> {
        self.find_paths(&|n| n.id.as_deref() == Some(id))
            .into_iter()
            .next()
    }

    /// The relations of the node at `path` under this root (none when there
    /// is no node there).
    pub(crate) fn relations(&self, path: &[usize]) -> Relations {
        let Some(n) = self.at_path(path) else {
            return Relations::default();
        };
        let ids = |id: &Option<String>| match id {
            Some(id) => self.find_paths(&|m| m.id.as_ref() == Some(id)),
            None => Vec::new(),
        };
        let naming = |field: fn(&A11yNode) -> Option<&String>| match &n.id {
            Some(id) => self.find_paths(&|m| field(m) == Some(id)),
            None => Vec::new(),
        };
        let (child_of, parent_of) = self.tree_links(path);
        Relations {
            labelled_by: ids(&n.labelled_by),
            label_for: naming(|m| m.labelled_by.as_ref()),
            controller_for: ids(&n.controls),
            controlled_by: naming(|m| m.controls.as_ref()),
            member_of: self.radio_group(path),
            node_child_of: child_of,
            node_parent_of: parent_of,
        }
    }

    /// The radio buttons in the group of the one at `path` (itself
    /// included): its parent's radio button children. Empty for anything
    /// that isn't a radio button.
    pub(crate) fn radio_group(&self, path: &[usize]) -> Vec<Vec<usize>> {
        let Some((_, parent)) = path.split_last() else {
            return Vec::new();
        };
        if self.at_path(path).map(|n| n.role) != Some(A11yRole::RadioButton) {
            return Vec::new();
        }
        let Some(p) = self.at_path(parent) else {
            return Vec::new();
        };
        p.children
            .iter()
            .enumerate()
            .filter(|(_, c)| c.role == A11yRole::RadioButton)
            .map(|(i, _)| {
                let mut q = parent.to_vec();
                q.push(i);
                q
            })
            .collect()
    }

    /// A tree item's depth, 1 at the top: its [`level`](Self::level), else
    /// one more than the tree items around it. `None` for anything that
    /// isn't a tree item.
    pub(crate) fn tree_level(&self, path: &[usize]) -> Option<usize> {
        let n = self.at_path(path)?;
        if n.role != A11yRole::TreeItem {
            return None;
        }
        if let Some(level) = n.level {
            return Some(level.max(1));
        }
        let mut at = self;
        let mut level = 1;
        for &i in &path[..path.len() - 1] {
            at = at.children.get(i)?;
            if at.role == A11yRole::TreeItem {
                level += 1;
            }
        }
        Some(level)
    }

    /// A tree item's parent item and its child items (see the module docs).
    fn tree_links(&self, path: &[usize]) -> (Option<Vec<usize>>, Vec<Vec<usize>>) {
        let Some(n) = self.at_path(path).filter(|n| n.role == A11yRole::TreeItem) else {
            return (None, Vec::new());
        };
        let Some((&index, parent)) = path.split_last() else {
            return (None, Vec::new());
        };
        let sibling = |i: usize| {
            let mut q = parent.to_vec();
            q.push(i);
            q
        };
        if let Some(level) = n.level {
            // A flat tree: the items beside it say how deep they are.
            let Some(p) = self.at_path(parent) else {
                return (None, Vec::new());
            };
            let items = |range: Box<dyn Iterator<Item = usize>>| {
                range
                    .filter_map(|i| {
                        let c = &p.children[i];
                        (c.role == A11yRole::TreeItem).then_some((i, c.level.unwrap_or(1)))
                    })
                    .collect::<Vec<_>>()
            };
            let before = items(Box::new((0..index).rev()));
            let child_of = before
                .iter()
                .find(|&&(_, l)| l < level)
                .filter(|&&(_, l)| l + 1 == level)
                .map(|&(i, _)| sibling(i));
            let after = items(Box::new(index + 1..p.children.len()));
            let parent_of = after
                .iter()
                .take_while(|&&(_, l)| l > level)
                .filter(|&&(_, l)| l == level + 1)
                .map(|&(i, _)| sibling(i))
                .collect();
            return (child_of, parent_of);
        }
        // A nested tree: the tree item around it, and those inside it.
        let child_of = (1..path.len())
            .rev()
            .map(|k| &path[..k])
            .find(|p| {
                self.at_path(p)
                    .is_some_and(|a| a.role == A11yRole::TreeItem)
            })
            .map(<[usize]>::to_vec);
        fn inner(n: &A11yNode, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
            for (i, c) in n.children.iter().enumerate() {
                path.push(i);
                if c.role == A11yRole::TreeItem {
                    out.push(path.clone());
                } else {
                    inner(c, path, out);
                }
                path.pop();
            }
        }
        let mut parent_of = Vec::new();
        inner(n, &mut path.to_vec(), &mut parent_of);
        (child_of, parent_of)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(role: A11yRole, name: &str) -> A11yNode {
        A11yNode::new(role, name, (0.0, 0.0, 10.0, 10.0))
    }

    fn with_id(mut n: A11yNode, id: &str) -> A11yNode {
        n.id = Some(id.into());
        n
    }

    fn group(role: A11yRole, children: Vec<A11yNode>) -> A11yNode {
        A11yNode {
            children,
            ..leaf(role, "")
        }
    }

    #[test]
    fn controls_and_labels_link_both_ways() {
        let combo = A11yNode {
            controls: Some("list".into()),
            labelled_by: Some("cap".into()),
            ..leaf(A11yRole::ComboBox, "Apple")
        };
        let root = group(
            A11yRole::Window,
            vec![
                with_id(leaf(A11yRole::Text, "Fruit"), "cap"),
                combo,
                with_id(
                    group(A11yRole::List, vec![leaf(A11yRole::ListItem, "Apple")]),
                    "list",
                ),
            ],
        );
        let r = root.relations(&[1]);
        assert_eq!(r.labelled_by, vec![vec![0]]);
        assert_eq!(r.controller_for, vec![vec![2]]);
        assert!(r.controlled_by.is_empty() && r.member_of.is_empty());
        assert_eq!(root.relations(&[2]).controlled_by, vec![vec![1]]);
        assert_eq!(root.relations(&[0]).label_for, vec![vec![1]]);
        assert!(root.relations(&[2, 0]).is_empty());
        assert!(root.relations(&[7]).is_empty());
    }

    #[test]
    fn radio_buttons_side_by_side_are_one_group() {
        let root = group(
            A11yRole::Window,
            vec![
                group(
                    A11yRole::Group,
                    vec![
                        leaf(A11yRole::RadioButton, "PNG"),
                        leaf(A11yRole::Text, "or"),
                        leaf(A11yRole::RadioButton, "JPEG"),
                    ],
                ),
                group(A11yRole::Group, vec![leaf(A11yRole::RadioButton, "Alone")]),
            ],
        );
        assert_eq!(
            root.relations(&[0, 2]).member_of,
            vec![vec![0, 0], vec![0, 2]]
        );
        assert_eq!(root.relations(&[1, 0]).member_of, vec![vec![1, 0]]);
        assert!(root.relations(&[0, 1]).member_of.is_empty());
    }

    #[test]
    fn a_flat_tree_links_items_by_level() {
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
                    item("Photos", 2),
                    item("other", 1),
                ],
            )],
        );
        let r = root.relations(&[0, 0]);
        assert_eq!(r.node_child_of, None);
        assert_eq!(r.node_parent_of, vec![vec![0, 1], vec![0, 3]]);
        let music = root.relations(&[0, 1]);
        assert_eq!(music.node_child_of, Some(vec![0, 0]));
        assert_eq!(music.node_parent_of, vec![vec![0, 2]]);
        assert_eq!(root.relations(&[0, 2]).node_child_of, Some(vec![0, 1]));
        assert_eq!(root.relations(&[0, 3]).node_child_of, Some(vec![0, 0]));
        assert!(root.relations(&[0, 4]).node_parent_of.is_empty());
        assert_eq!(root.tree_level(&[0, 2]), Some(3));
        assert_eq!(root.tree_level(&[0]), None);
    }

    #[test]
    fn a_nested_tree_links_items_by_place() {
        let root = group(
            A11yRole::Window,
            vec![group(
                A11yRole::Tree,
                vec![A11yNode {
                    children: vec![group(
                        A11yRole::Group,
                        vec![leaf(A11yRole::TreeItem, "b"), leaf(A11yRole::TreeItem, "c")],
                    )],
                    ..leaf(A11yRole::TreeItem, "a")
                }],
            )],
        );
        let a = root.relations(&[0, 0]);
        assert_eq!(a.node_child_of, None);
        assert_eq!(a.node_parent_of, vec![vec![0, 0, 0, 0], vec![0, 0, 0, 1]]);
        assert_eq!(
            root.relations(&[0, 0, 0, 1]).node_child_of,
            Some(vec![0, 0])
        );
        assert_eq!(root.tree_level(&[0, 0, 0, 1]), Some(2));
    }
}
