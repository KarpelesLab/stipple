//! A tree view on the virtualized list: expandable nodes with indentation and
//! disclosure triangles. The expanded part of the tree is flattened to a list
//! of visible nodes (ids only) each frame, and only the rows in the viewport
//! are built.
//!
//! Keys: Up/Down/Home/End move the selection; Right expands a collapsed node
//! or moves to its first child; Left collapses an expanded node or moves to
//! its parent; Enter and Space toggle the selected node.

use std::collections::HashSet;
use std::rc::Rc;

use crate::core::{Align, Axis, Cx, Element, KeyInput, Role};
use crate::geometry::Insets;
use crate::style::Theme;

use super::virtual_list::{DEFAULT_OVERSCAN, RowHeight, Rows, Window, frame, row_fill};

/// Where a [`tree`] gets its nodes. Nodes are identified by a `u64` the model
/// chooses (an index, a database key, a hash of a path…), stable across
/// frames.
pub trait TreeModel {
    /// The children of `parent`, in order; the root nodes for `None`.
    fn children(&self, parent: Option<u64>) -> Vec<u64>;

    /// Whether `id` has children (and so shows a disclosure triangle).
    /// Override when that is cheaper than listing them.
    fn has_children(&self, id: u64) -> bool {
        !self.children(Some(id)).is_empty()
    }
}

/// A tree's state, kept in the app state: which nodes are expanded and which
/// one is selected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeState {
    expanded: HashSet<u64>,
    selected: Option<u64>,
}

impl TreeState {
    /// Everything collapsed, nothing selected.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_expanded(&self, id: u64) -> bool {
        self.expanded.contains(&id)
    }

    pub fn set_expanded(&mut self, id: u64, expanded: bool) {
        if expanded {
            self.expanded.insert(id);
        } else {
            self.expanded.remove(&id);
        }
    }

    /// The selected node.
    pub fn selected(&self) -> Option<u64> {
        self.selected
    }

    pub fn select(&mut self, id: Option<u64>) {
        self.selected = id;
    }
}

/// What a tree row builder is told about the node it builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub id: u64,
    /// Nesting depth (0 for a root).
    pub depth: usize,
    /// Position among the visible (flattened) rows.
    pub index: usize,
    pub has_children: bool,
    pub expanded: bool,
    pub selected: bool,
}

/// A visible node of the flattened tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Flat {
    id: u64,
    depth: usize,
    parent: Option<usize>,
    has_children: bool,
    expanded: bool,
}

/// The visible nodes, depth first: each root, and the children of every
/// expanded node.
fn flatten(model: &impl TreeModel, state: &TreeState) -> Vec<Flat> {
    let mut out = Vec::new();
    // (id, depth, parent index), pushed in reverse so they pop in order.
    let mut stack: Vec<(u64, usize, Option<usize>)> = model
        .children(None)
        .into_iter()
        .rev()
        .map(|id| (id, 0, None))
        .collect();
    while let Some((id, depth, parent)) = stack.pop() {
        let has_children = model.has_children(id);
        let expanded = has_children && state.is_expanded(id);
        let index = out.len();
        out.push(Flat {
            id,
            depth,
            parent,
            has_children,
            expanded,
        });
        if expanded {
            stack.extend(
                model
                    .children(Some(id))
                    .into_iter()
                    .rev()
                    .map(|c| (c, depth + 1, Some(index))),
            );
        }
    }
    out
}

/// Collapse the node at `pos`, moving the selection to it if it was on one of
/// its descendants (which are about to disappear).
fn collapse(state: &mut TreeState, flat: &[Flat], pos: usize) {
    let node = flat[pos];
    state.set_expanded(node.id, false);
    let hidden = flat[pos + 1..]
        .iter()
        .take_while(|f| f.depth > node.depth)
        .any(|f| Some(f.id) == state.selected);
    if hidden {
        state.selected = Some(node.id);
    }
}

/// Expand or collapse the node at `pos` (Enter / Space). Never moves the
/// selection to a new row, so returns `None`.
fn toggle(state: &mut TreeState, flat: &[Flat], pos: usize) -> Option<usize> {
    let node = flat[pos];
    if node.expanded {
        collapse(state, flat, pos);
    } else if node.has_children {
        state.set_expanded(node.id, true);
    }
    None
}

/// Apply a navigation key. Returns the index of a newly selected row (to
/// scroll into view), if the selection moved.
fn handle_key(state: &mut TreeState, flat: &[Flat], key: &KeyInput) -> Option<usize> {
    if flat.is_empty() {
        return None;
    }
    let last = flat.len() - 1;
    let pos = state
        .selected
        .and_then(|id| flat.iter().position(|f| f.id == id));
    let to = match (key, pos) {
        (KeyInput::Up, Some(p)) => p.saturating_sub(1),
        (KeyInput::Down, Some(p)) => (p + 1).min(last),
        (KeyInput::Up | KeyInput::Down | KeyInput::Home, None) | (KeyInput::Home, _) => 0,
        (KeyInput::End, _) => last,
        (KeyInput::Right, Some(p)) => {
            let node = flat[p];
            if node.has_children && !node.expanded {
                state.set_expanded(node.id, true);
                return None;
            }
            if !node.expanded {
                return None;
            }
            p + 1
        }
        (KeyInput::Left, Some(p)) => {
            let node = flat[p];
            if node.expanded {
                collapse(state, flat, p);
                return None;
            }
            node.parent?
        }
        (KeyInput::Enter, Some(p)) => return toggle(state, flat, p),
        (KeyInput::Text(t), Some(p)) if t == " " => return toggle(state, flat, p),
        _ => return None,
    };
    state.selected = Some(flat[to].id);
    Some(to)
}

/// The shape of a [`tree`]: row height, viewport height and indentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeSpec {
    pub row_height: f64,
    /// The viewport height, in logical pixels.
    pub height: f64,
    /// Indentation per level, in logical pixels.
    pub indent: f64,
    /// Rows built beyond each edge of the viewport.
    pub overscan: usize,
}

impl Default for TreeSpec {
    fn default() -> Self {
        Self {
            row_height: 24.0,
            height: 240.0,
            indent: 16.0,
            overscan: DEFAULT_OVERSCAN,
        }
    }
}

impl TreeSpec {
    pub fn row_height(mut self, height: f64) -> Self {
        self.row_height = height;
        self
    }

    pub fn height(mut self, height: f64) -> Self {
        self.height = height;
        self
    }

    pub fn indent(mut self, indent: f64) -> Self {
        self.indent = indent;
        self
    }
}

/// A virtualized tree over `model`: each visible node is a row indented by its
/// depth, with a disclosure triangle (tap to expand or collapse) when it has
/// children, and the content `label` builds for it. Only the rows in the
/// viewport are built; the expanded part of the tree is walked (ids only) to
/// lay out the scroll extent.
///
/// ```
/// use stipple::prelude::*;
/// use stipple::widgets::{TreeModel, TreeSpec, TreeState, tree};
///
/// /// Ten roots of ten children each.
/// struct Numbers;
/// impl TreeModel for Numbers {
///     fn children(&self, parent: Option<u64>) -> Vec<u64> {
///         match parent {
///             None => (0..10).collect(),
///             Some(p) if p < 10 => (0..10).map(|c| 10 + p * 10 + c).collect(),
///             Some(_) => Vec::new(),
///         }
///     }
/// }
/// struct App { tree: TreeState }
/// let theme = Theme::light();
/// let mut cx = Cx::new(&theme);
/// let state = App { tree: TreeState::new() };
/// let view = tree(
///     &mut cx,
///     &theme,
///     &state.tree,
///     |s: &mut App| &mut s.tree,
///     &Numbers,
///     TreeSpec::default(),
///     |_cx, row| label(&theme, format!("Node {}", row.id)),
/// );
/// # let _ = view;
/// ```
pub fn tree<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    state: &TreeState,
    field: impl Fn(&mut S) -> &mut TreeState + Copy + 'static,
    model: &impl TreeModel,
    spec: TreeSpec,
    mut label: impl FnMut(&mut Cx<S>, &TreeRow) -> Element,
) -> Element {
    let p = &theme.palette;
    let flat: Rc<[Flat]> = flatten(model, state).into();
    let rows = Rows::new(flat.len(), RowHeight::Fixed(spec.row_height));
    let window = Window::new(cx, rows, spec.height, spec.overscan);
    let mut built = Vec::with_capacity(window.range.len());
    for index in window.range.clone() {
        let node = flat[index];
        let row = TreeRow {
            id: node.id,
            depth: node.depth,
            index,
            has_children: node.has_children,
            expanded: node.expanded,
            selected: state.selected == Some(node.id),
        };
        let mut disclosure = Element::stack(Axis::Horizontal, Vec::new())
            .width(16.0)
            .height(spec.row_height)
            .align(Align::Center, Align::Center);
        if node.has_children {
            let mark = if node.expanded { "▼" } else { "►" };
            disclosure = Element::stack(
                Axis::Horizontal,
                vec![Element::text(mark, theme.typography.caption, p.text_muted)],
            )
            .width(16.0)
            .height(spec.row_height)
            .align(Align::Center, Align::Center)
            .role(Role::Button);
            let flat = flat.clone();
            disclosure = disclosure.on_tap(cx, move |s: &mut S| {
                let st = field(s);
                if st.is_expanded(node.id) {
                    collapse(st, &flat, index);
                } else {
                    st.set_expanded(node.id, true);
                }
            });
        }
        let content = label(cx, &row);
        let indent = Element::stack(Axis::Horizontal, Vec::new())
            .width(node.depth as f64 * spec.indent)
            .height(1.0);
        let mut el = Element::stack(Axis::Horizontal, vec![indent, disclosure, content])
            .height(spec.row_height)
            .padding(Insets {
                left: theme.spacing.xs,
                right: theme.spacing.md,
                top: 0.0,
                bottom: 0.0,
            })
            .gap(theme.spacing.xs)
            .align(Align::Start, Align::Center)
            .role(Role::TreeItem)
            .selected(row.selected);
        if node.has_children {
            el = el.expanded(node.expanded);
        }
        if let Some(fill) = row_fill(theme, row.selected, false) {
            el = el.fill(fill);
        }
        let w = window.clone();
        built.push(el.on_tap(cx, move |s: &mut S| {
            field(s).select(Some(node.id));
            w.reveal(index);
        }));
    }
    let body = window.assemble(built);
    frame(theme, vec![body])
        .role(Role::Tree)
        .on_key(cx, move |s: &mut S, key| {
            if let Some(to) = handle_key(field(s), &flat, key) {
                window.reveal(to);
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::AccessNode;
    use crate::geometry::{Point, Size};

    /// 1,000 roots; root `r` has children `1000 + r*10 + c`; those are leaves.
    struct Big;
    impl TreeModel for Big {
        fn children(&self, parent: Option<u64>) -> Vec<u64> {
            match parent {
                None => (0..1_000).collect(),
                Some(p) if p < 1_000 => (0..10).map(|c| 1_000 + p * 10 + c).collect(),
                Some(_) => Vec::new(),
            }
        }
        fn has_children(&self, id: u64) -> bool {
            id < 1_000
        }
    }

    struct App {
        tree: TreeState,
    }

    fn app() -> crate::App<App> {
        crate::App::new(
            App {
                tree: TreeState::new(),
            },
            |state: &App, cx: &mut Cx<App>| {
                let theme = *cx.theme();
                let view = tree(
                    cx,
                    &theme,
                    &state.tree,
                    |s: &mut App| &mut s.tree,
                    &Big,
                    TreeSpec::default(),
                    |_cx, row| Element::text(format!("Node {}", row.id), 14.0, theme.palette.text),
                );
                // Under a container: the root is reported as the window.
                crate::widgets::column(vec![view.width(300.0)])
            },
        )
        .logical_size(Size::new(320.0, 300.0))
    }

    fn items(a: &AccessNode) -> Vec<&AccessNode> {
        a.descendants()
            .into_iter()
            .filter(|n| n.role == Role::TreeItem)
            .collect()
    }

    fn selected(app: &mut crate::App<App>) -> String {
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        items(&a)
            .into_iter()
            .find(|n| n.selected)
            .map(|n| n.name.clone())
            .unwrap_or_default()
    }

    #[test]
    fn flatten_follows_expansion() {
        let mut st = TreeState::new();
        assert_eq!(flatten(&Big, &st).len(), 1_000);
        st.set_expanded(3, true);
        st.set_expanded(1_030, true); // a leaf: nothing to show
        let flat = flatten(&Big, &st);
        assert_eq!(flat.len(), 1_010);
        assert_eq!(flat[4].id, 1_030);
        assert_eq!(flat[4].depth, 1);
        assert_eq!(flat[4].parent, Some(3));
        assert_eq!(flat[14].id, 4);
    }

    #[test]
    fn keyboard_expands_collapses_and_moves() {
        let mut app = app();
        app.render_once();
        // Click the first row to select it and focus the tree.
        assert!(app.click_at(Point::new(100.0, 1.0 + 12.0)));
        assert_eq!(selected(&mut app), "Node 0");
        let a = app.accessibility_tree().unwrap();
        assert!(
            a.descendants()
                .iter()
                .any(|n| n.role == Role::Tree && n.focused)
        );
        assert_eq!(items(&a)[0].expanded, Some(false));
        assert!(items(&a).len() < 20, "only the visible rows are built");

        app.press_key(KeyInput::Right); // expand
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        assert_eq!(items(&a)[0].expanded, Some(true));
        assert_eq!(items(&a)[1].name, "Node 1000");
        app.press_key(KeyInput::Right); // to the first child
        assert_eq!(selected(&mut app), "Node 1000");
        app.press_key(KeyInput::Down);
        assert_eq!(selected(&mut app), "Node 1001");
        app.press_key(KeyInput::Left); // a leaf: to its parent
        assert_eq!(selected(&mut app), "Node 0");
        app.press_key(KeyInput::Left); // collapse
        app.press_key(KeyInput::Down);
        assert_eq!(selected(&mut app), "Node 1");
        // End scrolls the last root into view.
        app.press_key(KeyInput::End);
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        let last = items(&a)
            .into_iter()
            .find(|n| n.name == "Node 999")
            .unwrap();
        assert!(last.selected && last.bounds.max_y() <= 241.5);
    }

    #[test]
    fn the_disclosure_triangle_toggles_and_collapsing_keeps_a_selection() {
        let mut app = app();
        app.render_once();
        // The triangle sits after the 4px left padding (depth 0, no indent).
        let triangle = Point::new(1.0 + 4.0 + 4.0 + 8.0, 1.0 + 24.0 + 12.0);
        assert!(app.click_at(triangle)); // expand node 1
        app.render_once();
        // Select its third child (row 4), then collapse node 1 from its triangle.
        assert!(app.click_at(Point::new(150.0, 1.0 + 24.0 * 4.0 + 12.0)));
        assert_eq!(selected(&mut app), "Node 1012");
        assert!(app.click_at(triangle));
        assert_eq!(selected(&mut app), "Node 1");
    }
}
