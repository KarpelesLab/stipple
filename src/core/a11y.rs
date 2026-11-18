//! Accessibility tree: a semantic view of the UI for assistive technologies.
//!
//! [`accessibility_tree`] walks the retained [`LayoutNode`] tree and produces a
//! pruned [`AccessNode`] tree — the platform-neutral abstraction a future
//! AT-SPI (Linux), UI Automation (Windows), or `NSAccessibility` (macOS) backend
//! would expose to screen readers. Each node carries a [`Role`], an accessible
//! `name`, its `bounds`, and whether it currently holds focus.
//!
//! Roles are inferred from how an element behaves: a tap handler makes it a
//! [`Role::Button`], a keyboard/focus handle a [`Role::TextField`], bare text a
//! [`Role::Text`]; the rest are [`Role::Group`] containers (with the tree root
//! reported as the [`Role::Window`]). Interactive nodes absorb their text
//! descendants as their `name`, and purely decorative containers (no name, no
//! children) are dropped, so the tree stays small and meaningful.
//!
//! Widgets whose semantics can't be inferred (lists, tables, trees, a select,
//! a toolbar) declare them with [`Element::role`](crate::core::Element::role),
//! [`a11y_label`](crate::core::Element::a11y_label) (a name for an element
//! with no text, such as an icon button),
//! [`selected`](crate::core::Element::selected),
//! [`expanded`](crate::core::Element::expanded) and
//! [`checked`](crate::core::Element::checked); a declared role wins over the
//! inferred one. A control names the element that labels it with
//! [`labelled_by`](crate::core::Element::labelled_by) and the label's
//! [`a11y_id`](crate::core::Element::a11y_id). A text field's node carries
//! its caret and selection.

use std::collections::HashMap;

use crate::core::runtime::{FocusId, LayoutNode, NodeContent, ScrollId, first_text};
use crate::geometry::Rect;

/// Where a scroll container is scrolled, as assistive technologies read it
/// (UIA's Scroll pattern, `AXScrollArea`, Android's scrollable nodes): its
/// offset and range along the axis it scrolls on, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct AccessScroll {
    /// How far it is scrolled from its start (the top, or the leading edge
    /// sideways), `0..=max`.
    pub offset: f64,
    /// The largest offset: the content's length less the viewport's (0 when
    /// it all fits).
    pub max: f64,
    /// The viewport's length along the axis (what a page scrolls).
    pub viewport: f64,
    /// Whether it scrolls sideways rather than up and down.
    pub horizontal: bool,
    /// A sideways strip laid out right to left (its start is on the right).
    pub rtl: bool,
}

/// The semantic kind of an [`AccessNode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Role {
    /// The top-level window (tree root).
    Window,
    /// A non-interactive container.
    Group,
    /// An activatable control (has a tap handler).
    Button,
    /// An editable text field (focusable / keyboard target).
    TextField,
    /// A run of static text.
    Text,
    /// A list of selectable items (also the open list of a
    /// [`ComboBox`](Role::ComboBox)).
    List,
    /// One item of a [`Role::List`].
    ListItem,
    /// A table / data grid: rows of cells under column headers.
    Table,
    /// One row of a [`Role::Table`], holding [`Role::Cell`]s (or headers).
    Row,
    /// One cell of a table row.
    Cell,
    /// A column header of a table (often clickable to sort).
    ColumnHeader,
    /// A tree of expandable items.
    Tree,
    /// One item of a [`Role::Tree`].
    TreeItem,
    /// A button that opens a [`List`](Role::List) to pick one value from (a
    /// select). Named by its current value.
    ComboBox,
    /// A numeric field with increment and decrement steps.
    SpinButton,
    /// A box that is checked or not (see [`Element::checked`](crate::core::Element::checked)).
    CheckBox,
    /// One option of a group of which exactly one is picked (a
    /// [`radio`](crate::widgets::radio) or a
    /// [`choice_chips`](crate::widgets::choice_chips) chip); the picked one is
    /// [`checked`](crate::core::Element::checked).
    RadioButton,
    /// An on/off switch (see [`Element::checked`](crate::core::Element::checked)).
    Switch,
    /// A draggable divider between two resizable panes.
    Splitter,
    /// A control that sets a value along a track (a
    /// [`slider`](crate::widgets::slider)): a volume, a playback position.
    /// Named by its [`a11y_label`](crate::core::Element::a11y_label).
    Slider,
    /// A row of command buttons.
    Toolbar,
    /// A visual divider between groups (in a toolbar or menu).
    Separator,
    /// A transient notification (a toast): screen readers announce it.
    Alert,
    /// A menu bar: a row of menu titles.
    MenuBar,
    /// An open menu (a dropdown, submenu or context menu) of
    /// [`MenuItem`](Role::MenuItem)s.
    Menu,
    /// One command of a [`Menu`](Role::Menu), or a title in a
    /// [`MenuBar`](Role::MenuBar).
    MenuItem,
    /// A row of document tabs (a [`tab_bar`](crate::widgets::tab_bar)) holding
    /// [`Tab`](Role::Tab)s.
    TabList,
    /// One tab of a [`TabList`](Role::TabList); the selected one is
    /// [`selected`](crate::core::Element::selected).
    Tab,
    /// A text field whose text is hidden (a
    /// [`password_field`](crate::widgets::password_field)). Its name never
    /// carries the text.
    PasswordField,
    /// A modal dialog asking the user something (a drawn message box).
    Dialog,
}

impl Role {
    /// Containers expose their children; the other roles take their name from
    /// their text and absorb their descendants.
    fn is_container(self) -> bool {
        matches!(
            self,
            Role::Window
                | Role::Group
                | Role::List
                | Role::Table
                | Role::Row
                | Role::Tree
                | Role::Toolbar
                | Role::Alert
                | Role::MenuBar
                | Role::Menu
                | Role::TabList
                | Role::Dialog
        )
    }
}

/// Semantics an element declares for assistive technologies: a role overriding
/// the inferred one, a name overriding its text, and item state. Set via
/// [`Element::role`](crate::core::Element::role),
/// [`a11y_label`](crate::core::Element::a11y_label),
/// [`selected`](crate::core::Element::selected),
/// [`expanded`](crate::core::Element::expanded) and
/// [`checked`](crate::core::Element::checked).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Semantics {
    /// The role to report, instead of the inferred one.
    pub role: Option<Role>,
    /// The name to report, instead of the element's text.
    pub label: Option<String>,
    /// Whether this item is selected (list items, rows, tree items).
    pub selected: bool,
    /// For an expandable item, whether it is expanded; `None` if it can't
    /// expand.
    pub expanded: Option<bool>,
    /// For a checkbox or switch, whether it is checked (on); `None` if it
    /// can't be checked.
    pub checked: Option<bool>,
    /// An id other elements' relations name this element by (see
    /// [`labelled_by`](Semantics::labelled_by)).
    pub id: Option<String>,
    /// The [`id`](Semantics::id) of the element that labels this one (a
    /// field's caption).
    pub labelled_by: Option<String>,
    /// A text field's hint, read while it's empty (see
    /// [`Element::placeholder`](crate::core::Element::placeholder)).
    pub placeholder: Option<String>,
    /// A text field's hints for the on-screen keyboard (see
    /// [`Element::input_kind`](crate::core::Element::input_kind)).
    pub input: Option<crate::platform::InputHints>,
    /// A press on this element leaves keyboard focus where it is (see
    /// [`Element::keeps_focus`](crate::core::Element::keeps_focus)).
    pub keep_focus: bool,
    /// The element pastes HTML and images too: a paste reads every format
    /// from the system clipboard, not just text (a rich text editor).
    pub rich_paste: bool,
    /// A select's options, for the OS's pop-up button that shows it live
    /// under the native theme (see `core::native_host`).
    pub choices: Option<std::sync::Arc<Choices>>,
    /// A text field's state, for a live OS text control to edit (under
    /// the native theme; see `core::native_host`).
    pub native_text: Option<Box<crate::platform::native::NativeText>>,
}

/// A select's options as its live OS control shows them: their labels, the
/// chosen one, and the tap handler a pick of each runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Choices {
    pub labels: Vec<String>,
    pub selected: Option<usize>,
    /// One per label: what picking it does (the app may refuse it).
    pub picks: Vec<crate::core::runtime::ActionId>,
}

/// A node in the accessibility tree.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct AccessNode {
    pub role: Role,
    /// The accessible name (a control's label, or the text itself).
    pub name: String,
    pub bounds: Rect,
    /// Whether this node currently holds keyboard focus.
    pub focused: bool,
    /// Whether this item is selected (see [`Element::selected`](crate::core::Element::selected)).
    pub selected: bool,
    /// Whether this item is expanded, if it can expand.
    pub expanded: Option<bool>,
    /// Whether this checkbox or switch is checked, if it can be.
    pub checked: Option<bool>,
    /// For an editable text field, the caret as a byte index into `name`
    /// (`None` when the name isn't the field's text: a password field, a
    /// field with an [`a11y_label`](crate::core::Element::a11y_label)).
    pub caret: Option<usize>,
    /// For an editable text field, the selected byte range `[start, end)` of
    /// `name`, if any (see [`caret`](AccessNode::caret)); for
    /// [selectable](crate::core::Element::selectable) text, its selection.
    pub text_selection: Option<(usize, usize)>,
    /// The id relations name this node by
    /// ([`Element::a11y_id`](crate::core::Element::a11y_id)).
    pub id: Option<String>,
    /// The id of the node that labels this one
    /// ([`Element::labelled_by`](crate::core::Element::labelled_by)).
    pub labelled_by: Option<String>,
    /// A text field's hint
    /// ([`Element::placeholder`](crate::core::Element::placeholder)), which
    /// screen readers read while it's empty.
    pub placeholder: Option<String>,
    /// For a scroll container (an [`Element::scrollable`](crate::core::Element::scrollable)
    /// group, or the list, tree or table around one), where it is scrolled.
    pub scroll: Option<AccessScroll>,
    /// When a live OS control stands for this node (under
    /// [`Theme::native`](crate::style::Theme::native), in a window that
    /// hosts them), its id: the platform exposes that control here.
    pub native_view: Option<u64>,
    pub children: Vec<AccessNode>,
}

/// The accessible text at or under `node` (a control's label / a field's value).
fn text_of(node: &LayoutNode) -> String {
    let found = if node.semantics.is_some() {
        own_text(node)
    } else {
        first_text(node)
    };
    match found.map(|n| &n.content) {
        // Text cut short with an ellipsis reads in full.
        Some(NodeContent::Text { text, style, .. }) => match style.full() {
            Some(full) => full.to_string(),
            None => style.plain(text, 0..text.len()),
        },
        _ => String::new(),
    }
}

/// The name a screen reader reads for `node`: its label, or its text.
pub(crate) fn name_of(node: &LayoutNode) -> String {
    node.semantics
        .as_ref()
        .and_then(|s| s.label.clone())
        .unwrap_or_else(|| text_of(node))
}

/// The first text under `node`, skipping descendants that are controls of their
/// own (a tree item's disclosure toggle, a checkbox in a row), so an item is
/// named by its label rather than by a glyph.
fn own_text(node: &LayoutNode) -> Option<&LayoutNode> {
    if matches!(node.content, NodeContent::Text { .. }) {
        return Some(node);
    }
    node.children
        .iter()
        .filter(|c| c.action.is_none() && !c.ghost)
        .find_map(own_text)
}

/// Infer a node's role from its interaction handles and content.
fn role_of(node: &LayoutNode) -> Role {
    if let Some(role) = node.semantics.as_ref().and_then(|s| s.role) {
        role
    } else if node.action.is_some() {
        Role::Button
    } else if node.focus.is_some() {
        Role::TextField
    } else if matches!(node.content, NodeContent::Text { .. }) {
        Role::Text
    } else {
        Role::Group
    }
}

/// Scroll container `node`'s [`AccessScroll`], at the offsets the app keeps.
fn scroll_of(node: &LayoutNode, offsets: &HashMap<ScrollId, f64>) -> Option<AccessScroll> {
    let id = node.scroll?;
    let offset = offsets.get(&id).copied().unwrap_or(0.0).max(0.0);
    let max = crate::core::scroll_range(node, offset).max(0.0);
    let b = node.bounds;
    Some(AccessScroll {
        offset: offset.min(max),
        max,
        viewport: if node.scroll_horizontal {
            b.width()
        } else {
            b.height()
        },
        horizontal: node.scroll_horizontal,
        rtl: node.scroll_horizontal && node.rtl,
    })
}

/// A list, tree or table scrolls through the scroll container inside it (its
/// body): the first unnamed group with a scroll under `node`, reached through
/// unnamed groups only, gives its scroll to `node` (and is left a plain
/// group, so the one container isn't reported twice).
fn hoist_scroll(node: &mut AccessNode) {
    fn take(children: &mut [AccessNode]) -> Option<AccessScroll> {
        children
            .iter_mut()
            .filter(|c| c.role == Role::Group && c.name.is_empty())
            .find_map(|c| c.scroll.take().or_else(|| take(&mut c.children)))
    }
    if node.scroll.is_none() && matches!(node.role, Role::List | Role::Tree | Role::Table) {
        node.scroll = take(&mut node.children);
    }
}

/// What [`build`] carries down the tree.
struct Ctx<'a> {
    focused: Option<FocusId>,
    offsets: &'a HashMap<ScrollId, f64>,
}

fn build(node: &LayoutNode, cx: &Ctx<'_>, is_root: bool) -> AccessNode {
    let focused = cx.focused;
    let role = if is_root { Role::Window } else { role_of(node) };
    // Interactive/text roles take their name from their text and absorb their
    // descendants; containers recurse and expose their meaningful children.
    let label = node.semantics.as_ref().and_then(|s| s.label.clone());
    let (mut caret, mut text_selection) = (None, None);
    let (name, children) = if role.is_container() {
        (
            label.unwrap_or_default(),
            node.children
                .iter()
                .filter(|c| !c.ghost)
                .map(|c| build(c, cx, false))
                .filter(|a| !a.is_prunable())
                .collect(),
        )
    } else {
        let labelled = label.is_some();
        let mut name = label.unwrap_or_else(|| text_of(node));
        // An empty field lays out a single space to keep its line's height
        // (`widgets::text_field`), or its placeholder; a screen reader should
        // read it as empty, not as "space" or as if the hint were typed.
        if role == Role::TextField && (name == " " || shows_placeholder(node)) {
            name.clear();
        }
        if !labelled && matches!(role, Role::TextField | Role::SpinButton) {
            (caret, text_selection) = caret_of(node, &name);
        } else if !labelled && role == Role::Text {
            text_selection = read_selection_of(node);
        }
        (name, Vec::new())
    };
    let sem = node.semantics.as_ref();
    let mut out = AccessNode {
        role,
        name,
        bounds: node.bounds,
        focused: node.focus.is_some() && node.focus == focused,
        selected: node.semantics.as_ref().is_some_and(|s| s.selected),
        expanded: node.semantics.as_ref().and_then(|s| s.expanded),
        checked: node.semantics.as_ref().and_then(|s| s.checked),
        caret,
        text_selection,
        id: sem.and_then(|s| s.id.clone()),
        labelled_by: sem.and_then(|s| s.labelled_by.clone()),
        placeholder: sem.and_then(|s| s.placeholder.clone()),
        scroll: scroll_of(node, cx.offsets),
        native_view: node.native_a11y.or(node.native_live),
        children,
    };
    // A live OS control speaks for all of it (a button's label too).
    if out.native_view.is_some() {
        out.children.clear();
    }
    hoist_scroll(&mut out);
    out
}

/// Whether the field `node` shows its placeholder in place of a value.
fn shows_placeholder(node: &LayoutNode) -> bool {
    matches!(
        own_text(node).map(|n| &n.content),
        Some(NodeContent::Text { style, .. }) if style.placeholder
    )
}

/// The caret and selection of an editable field whose name is `name`: from
/// its editable text leaf (the one carrying a caret), as byte indices into
/// `name`.
fn caret_of(node: &LayoutNode, name: &str) -> (Option<usize>, Option<(usize, usize)>) {
    let Some(leaf) = first_text(node).filter(|l| l.caret.is_some()) else {
        return (None, None);
    };
    let NodeContent::Text { text, .. } = &leaf.content else {
        return (None, None);
    };
    let hint = matches!(&leaf.content, NodeContent::Text { style, .. } if style.placeholder);
    if text != name && !(name.is_empty() && (text == " " || hint)) {
        return (None, None);
    }
    let snap = |i: usize| {
        let mut i = i.min(name.len());
        while !name.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let caret = leaf.caret.map(snap);
    let selection = leaf
        .selection
        .map(|(a, b)| (snap(a.min(b)), snap(a.max(b))))
        .filter(|(a, b)| b > a);
    (caret, selection)
}

/// The selection in read-only, [selectable](crate::core::Element::selectable)
/// text, as byte indices into its accessible name (where rich text's images
/// are their alternative text).
fn read_selection_of(node: &LayoutNode) -> Option<(usize, usize)> {
    let leaf = own_text(node)?;
    let NodeContent::Text { text, style, .. } = &leaf.content else {
        return None;
    };
    let (s, e) = leaf.selection.filter(|_| style.selectable)?;
    let at = |i: usize| {
        let mut i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        style.plain(text, 0..i).len()
    };
    let (s, e) = (at(s.min(e)), at(s.max(e)));
    (e > s).then_some((s, e))
}

impl AccessNode {
    /// A container with no name and no children carries no semantics, so it can
    /// be dropped from the tree.
    fn is_prunable(&self) -> bool {
        self.role == Role::Group
            && self.name.is_empty()
            && self.children.is_empty()
            && self.native_view.is_none()
    }

    /// Total node count (including this one), for tests/inspection.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(AccessNode::count).sum::<usize>()
    }

    /// Depth-first iterator visiting this node and all descendants.
    pub fn descendants(&self) -> Vec<&AccessNode> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.descendants());
        }
        out
    }
}

/// Build the accessibility tree for a laid-out UI, marking the node that holds
/// `focused` (if any). The root is reported as a [`Role::Window`]. Scroll
/// containers read as scrolled to their start here (the offsets live in the
/// app: [`App::accessibility_tree`](crate::App::accessibility_tree) has them).
pub fn accessibility_tree(root: &LayoutNode, focused: Option<FocusId>) -> AccessNode {
    accessibility_tree_at(root, focused, &HashMap::new())
}

/// [`accessibility_tree`] with the scroll containers at `offsets`.
pub(crate) fn accessibility_tree_at(
    root: &LayoutNode,
    focused: Option<FocusId>,
    offsets: &HashMap<ScrollId, f64>,
) -> AccessNode {
    build(root, &Ctx { focused, offsets }, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Axis, BoxStyle, Cx, Element, layout};
    use crate::render::Color;
    use crate::style::Theme;

    /// Lay a small interactive view out and return its accessibility tree.
    fn tree(focused: Option<FocusId>) -> AccessNode {
        let theme = Theme::light();
        let mut cx = Cx::<()>::new(&theme);
        let field = Element::stack(
            Axis::Horizontal,
            vec![Element::text("hello", 14.0, Color::BLACK)],
        )
        .on_key(&mut cx, |_: &mut (), _| {});
        let button = Element::stack(
            Axis::Horizontal,
            vec![Element::text("OK", 14.0, Color::BLACK)],
        )
        .on_tap(&mut cx, |_: &mut ()| {});
        let root = Element::stack(
            Axis::Vertical,
            vec![Element::text("Title", 18.0, Color::BLACK), field, button],
        )
        .fill(Color::WHITE)
        .padding(crate::geometry::Insets::uniform(4.0));
        let laid = layout(&root, Rect::from_xywh(0.0, 0.0, 200.0, 120.0), None);
        accessibility_tree(&laid, focused)
    }

    #[test]
    fn derives_roles_and_names() {
        let a = tree(None);
        assert_eq!(a.role, Role::Window);
        // The root exposes: a Title (Text), the field (TextField), the button.
        let roles: Vec<Role> = a.children.iter().map(|c| c.role).collect();
        assert_eq!(roles, vec![Role::Text, Role::TextField, Role::Button]);
        assert_eq!(a.children[0].name, "Title");
        assert_eq!(a.children[1].name, "hello"); // field value
        assert_eq!(a.children[2].name, "OK"); // button label
        // Interactive nodes absorbed their text children.
        assert!(a.children[1].children.is_empty());
        assert!(a.children[2].children.is_empty());
    }

    #[test]
    fn marks_the_focused_field() {
        // Focus the text field (the only focusable, id 0).
        let a = tree(Some(FocusId(0)));
        let field = a
            .descendants()
            .into_iter()
            .find(|n| n.role == Role::TextField)
            .unwrap();
        assert!(field.focused);
        // Nothing else is focused.
        assert_eq!(a.descendants().iter().filter(|n| n.focused).count(), 1);
    }

    #[test]
    fn an_empty_text_field_reads_as_empty() {
        let theme = Theme::light();
        let mut cx = Cx::<crate::widgets::TextInput>::new(&theme);
        let empty = crate::widgets::TextInput::default();
        let field = crate::widgets::text_field(&mut cx, &empty, |s| s);
        let root = Element::stack(Axis::Vertical, vec![field]);
        let laid = layout(&root, Rect::from_xywh(0.0, 0.0, 200.0, 40.0), None);
        let a = accessibility_tree(&laid, None);
        let field = a
            .descendants()
            .into_iter()
            .find(|n| n.role == Role::TextField)
            .unwrap();
        assert_eq!(field.name, "");
    }

    #[test]
    fn scroll_containers_say_where_they_are_scrolled() {
        let theme = Theme::light();
        let mut cx = Cx::<()>::new(&theme);
        let rows = |cx: &mut Cx<()>| {
            (0..10)
                .map(|i| {
                    Element::text(format!("Row {i}"), 14.0, Color::BLACK)
                        .height(20.0)
                        .on_tap(cx, |_: &mut ()| {})
                })
                .collect::<Vec<_>>()
        };
        let plain = cx.register_scroll();
        let in_list = cx.register_scroll();
        let scroller = Element::stack(Axis::Vertical, rows(&mut cx))
            .height(100.0)
            .scrollable(plain);
        let list = Element::stack(
            Axis::Vertical,
            vec![
                Element::stack(Axis::Vertical, rows(&mut cx))
                    .height(50.0)
                    .scrollable(in_list),
            ],
        )
        .role(Role::List);
        let root = Element::stack(Axis::Vertical, vec![scroller, list]);
        let mut laid = layout(&root, Rect::from_xywh(0.0, 0.0, 200.0, 400.0), None);
        let mut offsets = HashMap::from([(plain, 30.0), (in_list, 500.0)]);
        crate::core::apply_scroll(&mut laid, &mut offsets);
        let a = accessibility_tree_at(&laid, None, &offsets);
        let group = &a.children[0];
        assert_eq!(group.role, Role::Group);
        let s = group.scroll.expect("a plain scroll container scrolls");
        assert_eq!((s.offset, s.max, s.viewport), (30.0, 100.0, 100.0));
        assert!(!s.horizontal);
        // The list takes its body's scroll (clamped to its end), and the
        // body's group doesn't report it again.
        let list = &a.children[1];
        assert_eq!(list.role, Role::List);
        let s = list.scroll.expect("the list scrolls");
        assert_eq!((s.offset, s.max), (150.0, 150.0));
        assert!(
            list.descendants()
                .iter()
                .skip(1)
                .all(|n| n.scroll.is_none())
        );
        // Without the app's offsets, it reads as at its start.
        let fresh = accessibility_tree(&laid, None);
        assert_eq!(fresh.children[0].scroll.map(|s| s.offset), Some(0.0));
    }

    #[test]
    fn prunes_decorative_containers() {
        // A panel wrapping an empty decorative box plus a label.
        let root = Element::stack(
            Axis::Vertical,
            vec![
                Element::boxed(BoxStyle::default()).width(10.0).height(10.0), // decorative
                Element::text("Label", 14.0, Color::BLACK),
            ],
        );
        let laid = layout(&root, Rect::from_xywh(0.0, 0.0, 100.0, 100.0), None);
        let a = accessibility_tree(&laid, None);
        // Only the text survives; the empty box is pruned.
        assert_eq!(a.children.len(), 1);
        assert_eq!(a.children[0].role, Role::Text);
    }
}
