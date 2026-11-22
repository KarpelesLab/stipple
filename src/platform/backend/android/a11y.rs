//! Accessibility on Android: the host view's `AccessibilityNodeProvider`
//! (see [`classes`](super::classes)) gives TalkBack (and `uiautomator`) one
//! virtual view per node of the app's accessibility tree, with its role (as
//! the class name of the closest Android widget), text, state and screen
//! bounds, and acts on them: click, focus and select tap the node, scroll
//! forward and backward scroll it (any scroll container, plain or a list,
//! a page at a time, offered only where it can go), and set text replaces a
//! field's text. A scroll container that scrolled sends `TYPE_VIEW_SCROLLED`.
//! A slider, progress bar or number input has its `RangeInfo` (a slider's
//! and a number input's set with `ACTION_SET_PROGRESS`, or stepped with
//! scroll forward and backward, as a `SeekBar` is), and a moved one sends
//! `TYPE_VIEW_SELECTED`, as a `SeekBar` does; a table has its
//! `CollectionInfo` (all its rows and columns) and its cells and column
//! headers their `CollectionItemInfo`; a control a caption labels names it
//! with `setLabeledBy`.
//! Hover events from touch exploration (TalkBack's explore by touch) reach
//! the host view, which says which node is under the finger.
//!
//! The UI thread flattens each new tree into [`TREE`] (virtual view ids are
//! the nodes' indices in tree order; the root, the window, is the host view
//! itself) and asks the main thread to announce the change. The provider's
//! methods run on the main thread and read it from there. An alert (a toast)
//! appearing is announced too: a `TYPE_ANNOUNCEMENT` event from its node,
//! with its message, which TalkBack speaks.

use std::sync::Mutex;

use super::classes;
use super::jni::{self, Env, JObject, JValue, Jni};
use super::{ARect, Command, post_command};
use crate::geometry::Point;
use crate::platform::access::{A11yCell, A11yNode, A11yRange, A11yRole, A11yScroll, A11yTable};
use crate::platform::event::{ButtonState, Event, KeyCode, Modifiers, PointerButton, ScrollDelta};

/// `AccessibilityNodeProvider.HOST_VIEW_ID`.
const HOST_VIEW_ID: i32 = -1;

// AccessibilityNodeInfo actions.
const ACTION_FOCUS: i32 = 0x1;
const ACTION_CLEAR_FOCUS: i32 = 0x2;
const ACTION_SELECT: i32 = 0x4;
const ACTION_CLICK: i32 = 0x10;
const ACTION_ACCESSIBILITY_FOCUS: i32 = 0x40;
const ACTION_CLEAR_ACCESSIBILITY_FOCUS: i32 = 0x80;
const ACTION_SCROLL_FORWARD: i32 = 0x1000;
const ACTION_SCROLL_BACKWARD: i32 = 0x2000;
const ACTION_SET_TEXT: i32 = 0x20_0000;
/// `android.R.id.accessibilityActionSetProgress` (API 24).
const ACTION_SET_PROGRESS: i32 = 0x0102_003d;
const SET_PROGRESS_SDK: i32 = 24;

// AccessibilityEvent types.
const TYPE_VIEW_CLICKED: i32 = 0x1;
const TYPE_VIEW_SELECTED: i32 = 0x4;
const TYPE_VIEW_FOCUSED: i32 = 0x8;
const TYPE_VIEW_HOVER_ENTER: i32 = 0x80;
const TYPE_VIEW_HOVER_EXIT: i32 = 0x100;
const TYPE_VIEW_SCROLLED: i32 = 0x1000;
const TYPE_WINDOW_CONTENT_CHANGED: i32 = 0x800;
const TYPE_VIEW_ACCESSIBILITY_FOCUSED: i32 = 0x8000;
const TYPE_VIEW_ACCESSIBILITY_FOCUS_CLEARED: i32 = 0x1_0000;
const TYPE_ANNOUNCEMENT: i32 = 0x4000;

// AccessibilityNodeInfo.FOCUS_INPUT and FOCUS_ACCESSIBILITY.
const FOCUS_INPUT: i32 = 1;
const FOCUS_ACCESSIBILITY: i32 = 2;

// MotionEvent hover actions.
const ACTION_HOVER_EXIT: i32 = 10;

/// One node, flattened.
#[derive(Clone, Debug, PartialEq)]
struct Node {
    role: A11yRole,
    name: String,
    /// Bounds in window pixels.
    rect: ARect,
    /// Its center, in the app's logical pixels, where taps on it land.
    center: Point,
    /// Logical height, how far one scroll step goes.
    height: f64,
    /// The parent's index; `HOST_VIEW_ID` for the root's children.
    parent: i32,
    children: Vec<i32>,
    focused: bool,
    selected: bool,
    checked: Option<bool>,
    /// A field's placeholder, its hint text; empty for none.
    hint: String,
    /// Whether any of it is on screen (inside the content).
    visible: bool,
    /// For a scroll container, where it is scrolled.
    scroll: Option<A11yScroll>,
    /// For a scroll bar, the scrolling it shows (its range info).
    bar: Option<A11yScroll>,
    /// Its place in the tree (child indices from the root).
    path: Vec<usize>,
    /// Whether a scroll container that scrolls holds it (it can be scrolled
    /// on screen: `ACTION_SHOW_ON_SCREEN`).
    in_scroller: bool,
    /// Logical bounds, where a slider is pressed to set it.
    logical: (f64, f64, f64, f64),
    /// For a slider, a progress bar or a number input, its value.
    range: Option<A11yRange>,
    /// For a table, its size; for its cells, where they are.
    table: Option<A11yTable>,
    cell: Option<A11yCell>,
    /// The node that labels this one (its caption), or `HOST_VIEW_ID`.
    label: i32,
}

impl Node {
    /// The node as the shared actions take it (see `platform::access`).
    fn a11y(&self) -> A11yNode {
        A11yNode {
            focused: self.focused,
            range: self.range,
            ..A11yNode::new(self.role, self.name.clone(), self.logical)
        }
    }

    /// Whether a screen reader may set its range value: a slider's or a
    /// number input's.
    fn adjustable(&self) -> bool {
        self.a11y().range_settable()
    }
}

/// The tree the provider serves. `nodes[0]` is the root (the host view).
#[derive(Debug)]
struct Tree {
    nodes: Vec<Node>,
    /// The node TalkBack has its focus on, and the one under a hovering finger.
    a11y_focus: i32,
    hover: i32,
    /// The tree the nodes were flattened from: an action on a node scrolled
    /// out of view brings it into view first (`access::reveal`).
    source: Option<A11yNode>,
}

static TREE: Mutex<Tree> = Mutex::new(Tree {
    nodes: Vec::new(),
    a11y_focus: HOST_VIEW_ID,
    hover: HOST_VIEW_ID,
    source: None,
});

fn tree() -> std::sync::MutexGuard<'static, Tree> {
    TREE.lock().unwrap_or_else(|e| e.into_inner())
}

/// What changed in a new tree, for the main thread to announce.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Changes {
    /// The tree changed.
    pub(super) content: bool,
    /// Input focus moved to this node.
    pub(super) focused: Option<i32>,
    /// This alert (a toast) appeared (the last one, if several did).
    pub(super) alert: Option<i32>,
    /// This scroll container scrolled (the first one, if several did).
    pub(super) scrolled: Option<i32>,
    /// This slider's (progress bar's, number input's) value moved.
    pub(super) moved: Option<i32>,
}

impl Changes {
    pub(super) fn merge(&mut self, other: Changes) {
        self.content |= other.content;
        self.focused = other.focused.or(self.focused);
        self.alert = other.alert.or(self.alert);
        self.scrolled = other.scrolled.or(self.scrolled);
        self.moved = other.moved.or(self.moved);
    }
}

/// The last alert (a toast) in `new` that isn't in `old` (by name), as a
/// virtual view id; none for the first tree.
fn new_alert(old: &[Node], new: &[Node]) -> Option<i32> {
    if old.is_empty() {
        return None;
    }
    let is_alert = |n: &&Node| n.role == A11yRole::Alert && !n.name.is_empty();
    let mut before: Vec<&str> = old
        .iter()
        .filter(is_alert)
        .map(|n| n.name.as_str())
        .collect();
    let mut found = None;
    for (i, n) in new.iter().enumerate().filter(|(_, n)| is_alert(n)) {
        match before.iter().position(|b| *b == n.name) {
            Some(j) => {
                before.swap_remove(j);
            }
            None => found = Some(i as i32),
        }
    }
    found
}

/// Flatten `root` (logical pixels, laid out in `content`, window pixels, at
/// `scale`) into the tree the provider serves. On the UI thread. Returns what
/// changed.
pub(super) fn update(root: &A11yNode, content: ARect, scale: f64) -> Changes {
    struct At<'a> {
        content: ARect,
        scale: f64,
        path: &'a mut Vec<usize>,
        in_scroller: bool,
    }
    fn walk(n: &A11yNode, parent: i32, at: &mut At<'_>, out: &mut Vec<Node>) -> i32 {
        let (content, scale) = (at.content, at.scale);
        let id = out.len() as i32;
        let (x, y, w, h) = n.bounds;
        let px = |v: f64, origin: i32| (v * scale).round() as i32 + origin;
        let rect = ARect {
            left: px(x, content.left),
            top: px(y, content.top),
            right: px(x + w, content.left),
            bottom: px(y + h, content.top),
        };
        let visible = w > 0.0
            && h > 0.0
            && rect.right > content.left
            && rect.left < content.right
            && rect.bottom > content.top
            && rect.top < content.bottom;
        out.push(Node {
            role: n.role,
            name: n.name.clone(),
            rect,
            center: Point::new(x + w / 2.0, y + h / 2.0),
            height: h,
            parent: if parent == 0 { HOST_VIEW_ID } else { parent },
            children: Vec::new(),
            focused: n.focused,
            selected: n.selected,
            checked: n.checked,
            hint: n.hint().unwrap_or_default().to_string(),
            visible,
            scroll: n.scroll,
            bar: n.scroll_bar,
            path: at.path.clone(),
            in_scroller: at.in_scroller,
            logical: n.bounds,
            range: n.range,
            table: n.table,
            cell: n.cell,
            label: HOST_VIEW_ID,
        });
        let was = at.in_scroller;
        at.in_scroller |= n.scroll.is_some_and(|s| s.scrolls());
        let mut children = Vec::with_capacity(n.children.len());
        for (i, c) in n.children.iter().enumerate() {
            at.path.push(i);
            children.push(walk(c, id, at, out));
            at.path.pop();
        }
        at.in_scroller = was;
        out[id as usize].children = children;
        id
    }
    let mut nodes = Vec::new();
    let mut at = At {
        content,
        scale,
        path: &mut Vec::new(),
        in_scroller: false,
    };
    walk(root, 0, &mut at, &mut nodes);
    // Each control's caption (labelled-by, from the shared relations), by
    // its virtual view id.
    for i in 1..nodes.len() {
        if root
            .at_path(&nodes[i].path)
            .is_none_or(|n| n.labelled_by.is_none())
        {
            continue;
        }
        let rel = root.relations(&nodes[i].path);
        if let Some(caption) = rel.labelled_by.first()
            && let Some(j) = nodes.iter().position(|n| n.path == *caption)
            && j > 0
        {
            nodes[i].label = j as i32;
        }
    }
    let mut t = tree();
    if t.nodes == nodes {
        return Changes::default();
    }
    t.source = Some(root.clone());
    let focus_of = |nodes: &[Node]| {
        nodes
            .iter()
            .skip(1)
            .position(|n| n.focused)
            .map(|i| i as i32 + 1)
    };
    let was = focus_of(&t.nodes);
    let now = focus_of(&nodes);
    let alert = new_alert(&t.nodes, &nodes);
    let scrolled = scrolled(&t.nodes, &nodes);
    let moved = moved(&t.nodes, &nodes);
    t.nodes = nodes;
    if t.a11y_focus as usize >= t.nodes.len() {
        t.a11y_focus = HOST_VIEW_ID;
    }
    Changes {
        content: true,
        focused: now.filter(|_| now != was),
        alert,
        scrolled,
        moved,
    }
}

/// The first node in `new` whose range value moved since `old` (the same
/// node: the same place in tree order and role).
fn moved(old: &[Node], new: &[Node]) -> Option<i32> {
    new.iter()
        .zip(old)
        .position(|(n, o)| match (n.range, o.range) {
            (Some(a), Some(b)) => n.role == o.role && a.value != b.value,
            _ => false,
        })
        .map(|i| i as i32)
}

/// The first scroll container in `new` whose offset moved since `old` (the
/// same node: the same place in tree order, still scrolling).
fn scrolled(old: &[Node], new: &[Node]) -> Option<i32> {
    new.iter()
        .zip(old)
        .position(|(n, o)| match (n.scroll, o.scroll) {
            (Some(a), Some(b)) => n.role == o.role && (a.offset - b.offset).abs() > 0.5,
            _ => false,
        })
        .map(|i| i as i32)
}

/// Announce `changes` to accessibility services. On the main thread.
pub(super) fn announce(jni: &Jni, changes: Changes) {
    let view = classes::host().view.get();
    if view.is_null() {
        return;
    }
    if changes.content {
        jni.call_void(
            view,
            c"sendAccessibilityEvent",
            c"(I)V",
            &[JValue {
                i: TYPE_WINDOW_CONTENT_CHANGED,
            }],
        );
    }
    if let Some(id) = changes.focused {
        send_event(jni, id, TYPE_VIEW_FOCUSED);
    }
    if let Some(id) = changes.scrolled.filter(|&id| id > 0) {
        send_event(jni, id, TYPE_VIEW_SCROLLED);
    }
    if let Some(id) = changes.moved.filter(|&id| id > 0) {
        send_event(jni, id, TYPE_VIEW_SELECTED);
    }
    // (Still an alert: the tree may have changed again since.)
    if let Some(id) = changes.alert.filter(|&id| {
        let t = tree();
        usize::try_from(id)
            .ok()
            .and_then(|i| t.nodes.get(i))
            .is_some_and(|n| n.role == A11yRole::Alert)
    }) {
        send_event(jni, id, TYPE_ANNOUNCEMENT);
    }
}

/// `AccessibilityNodeInfo.RangeInfo.RANGE_TYPE_FLOAT`.
const RANGE_TYPE_FLOAT: i32 = 1;

/// The Android widget class a role reads as.
fn class_name(role: A11yRole) -> &'static str {
    match role {
        A11yRole::Button | A11yRole::MenuItem | A11yRole::Tab => "android.widget.Button",
        A11yRole::TextField | A11yRole::SpinButton | A11yRole::PasswordField => {
            "android.widget.EditText"
        }
        A11yRole::Text => "android.widget.TextView",
        A11yRole::CheckBox => "android.widget.CheckBox",
        A11yRole::RadioButton => "android.widget.RadioButton",
        A11yRole::Switch => "android.widget.Switch",
        A11yRole::Slider => "android.widget.SeekBar",
        A11yRole::ProgressBar => "android.widget.ProgressBar",
        A11yRole::List | A11yRole::Tree | A11yRole::Table => "android.widget.ListView",
        A11yRole::ComboBox => "android.widget.Spinner",
        A11yRole::TabList => "android.widget.TabWidget",
        A11yRole::Toolbar => "android.widget.Toolbar",
        _ => "android.view.View",
    }
}

fn clickable(role: A11yRole) -> bool {
    matches!(
        role,
        A11yRole::Button
            | A11yRole::MenuItem
            | A11yRole::Tab
            | A11yRole::TextField
            | A11yRole::SpinButton
            | A11yRole::PasswordField
            | A11yRole::CheckBox
            | A11yRole::RadioButton
            | A11yRole::Switch
            | A11yRole::ComboBox
            | A11yRole::ListItem
            | A11yRole::TreeItem
            | A11yRole::Row
            | A11yRole::Cell
    )
}

fn editable(role: A11yRole) -> bool {
    matches!(
        role,
        A11yRole::TextField | A11yRole::SpinButton | A11yRole::PasswordField
    )
}

/// Whether node `n` scrolls: a scroll container (a plain one, or a list,
/// tree or table around one) whose content overflows; a list, tree or table
/// that says nothing of its scrolling is taken to.
fn scrollable(n: &Node) -> bool {
    match n.scroll {
        Some(s) => s.scrolls(),
        None => matches!(n.role, A11yRole::List | A11yRole::Tree | A11yRole::Table),
    }
}

/// Which ways node `n` can scroll: `(back, forward)`.
fn scroll_ways(n: &Node) -> (bool, bool) {
    match n.scroll {
        Some(s) => (s.offset > 0.5, s.offset < s.max - 0.5),
        None => (true, true),
    }
}

/// The wheel `(dx, dy)` that scrolls node `n` a page forward or back.
fn scroll_wheel(n: &Node, forward: bool) -> (f64, f64) {
    match n.scroll {
        Some(s) => {
            let page = if forward { s.page() } else { -s.page() };
            s.wheel(s.to(s.offset + page))
        }
        None => {
            let step = n.height * 0.8;
            (0.0, if forward { step } else { -step })
        }
    }
}

/// Where the window is on screen, in pixels (the decor view's position).
fn window_origin(jni: &Jni) -> Option<(i32, i32)> {
    let activity = classes::host().activity.get();
    let window = jni.call_object(activity, c"getWindow", c"()Landroid/view/Window;", &[])?;
    let decor = jni.call_object(window, c"getDecorView", c"()Landroid/view/View;", &[])?;
    let at = jni.int_array(2)?;
    jni.call_void(decor, c"getLocationOnScreen", c"([I)V", &[JValue { l: at }])?;
    let [x, y] = jni.ints::<2>(at)?;
    Some((x, y))
}

/// `AccessibilityNodeInfo.setHintText` came in API 26 (Android 8.0).
const HINT_TEXT_SDK: i32 = 26;

/// `Build.VERSION.SDK_INT`, read once (0 if it can't be).
fn sdk_int(jni: &Jni) -> i32 {
    static SDK: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *SDK.get_or_init(|| {
        jni.static_int(c"android/os/Build$VERSION", c"SDK_INT")
            .unwrap_or(0)
    })
}

fn set_bool(jni: &Jni, info: JObject, setter: &std::ffi::CStr, value: bool) {
    jni.call_void(info, setter, c"(Z)V", &[JValue { z: u8::from(value) }]);
}

fn add_child(jni: &Jni, info: JObject, view: JObject, id: i32) {
    jni.call_void(
        info,
        c"addChild",
        c"(Landroid/view/View;I)V",
        &[JValue { l: view }, JValue { i: id }],
    );
}

/// The `AccessibilityNodeInfo` for virtual view `id` (or the host view).
fn node_info(jni: &Jni, id: i32) -> Option<JObject> {
    let host = classes::host();
    let view = host.view.get();
    if view.is_null() {
        return None;
    }
    let info_class = c"android/view/accessibility/AccessibilityNodeInfo";
    if id == HOST_VIEW_ID {
        let info = jni.call_static_object(
            info_class,
            c"obtain",
            c"(Landroid/view/View;)Landroid/view/accessibility/AccessibilityNodeInfo;",
            &[JValue { l: view }],
        )?;
        jni.call_void(
            view,
            c"onInitializeAccessibilityNodeInfo",
            c"(Landroid/view/accessibility/AccessibilityNodeInfo;)V",
            &[JValue { l: info }],
        )?;
        // The view takes input focus (for keys and the IME), but to
        // TalkBack a focusable node is one item that reads out every
        // descendant that can't act: the window would be one stop, and its
        // labels would never get a stop of their own. The window is a
        // container: not focusable, no focus actions.
        set_bool(jni, info, c"setFocusable", false);
        for a in [ACTION_FOCUS, ACTION_CLEAR_FOCUS] {
            jni.call_void(info, c"removeAction", c"(I)V", &[JValue { i: a }]);
        }
        let t = tree();
        let children = t
            .nodes
            .first()
            .map(|n| n.children.clone())
            .unwrap_or_default();
        drop(t);
        for c in children {
            add_child(jni, info, view, c);
        }
        return Some(info);
    }
    let t = tree();
    let node = t
        .nodes
        .get(usize::try_from(id).ok()?)
        .filter(|_| id > 0)?
        .clone();
    let a11y_focused = t.a11y_focus == id;
    drop(t);
    let info = jni.call_static_object(
        info_class,
        c"obtain",
        c"(Landroid/view/View;I)Landroid/view/accessibility/AccessibilityNodeInfo;",
        &[JValue { l: view }, JValue { i: id }],
    )?;
    let seq = c"(Ljava/lang/CharSequence;)V";
    jni.call_void(
        info,
        c"setPackageName",
        seq,
        &[JValue {
            l: host.package.get(),
        }],
    )?;
    let class = jni.string(class_name(node.role))?;
    jni.call_void(info, c"setClassName", seq, &[JValue { l: class }])?;
    if node.parent == HOST_VIEW_ID {
        jni.call_void(
            info,
            c"setParent",
            c"(Landroid/view/View;)V",
            &[JValue { l: view }],
        )?;
    } else {
        jni.call_void(
            info,
            c"setParent",
            c"(Landroid/view/View;I)V",
            &[JValue { l: view }, JValue { i: node.parent }],
        )?;
    }
    for &c in &node.children {
        add_child(jni, info, view, c);
    }
    let (ox, oy) = window_origin(jni).unwrap_or((0, 0));
    let r = node.rect;
    let rect = jni.new_named(
        c"android/graphics/Rect",
        c"(IIII)V",
        &[
            JValue { i: r.left + ox },
            JValue { i: r.top + oy },
            JValue { i: r.right + ox },
            JValue { i: r.bottom + oy },
        ],
    )?;
    jni.call_void(
        info,
        c"setBoundsInScreen",
        c"(Landroid/graphics/Rect;)V",
        &[JValue { l: rect }],
    )?;
    if !node.name.is_empty() {
        let name = jni.string16(&node.name)?;
        // Text and fields read their text; containers are described.
        let setter = match node.role {
            A11yRole::Group | A11yRole::Window | A11yRole::Dialog | A11yRole::List => {
                c"setContentDescription"
            }
            _ => c"setText",
        };
        jni.call_void(info, setter, seq, &[JValue { l: name }])?;
    }
    let role = node.role;
    set_bool(jni, info, c"setEnabled", true);
    set_bool(jni, info, c"setVisibleToUser", node.visible);
    set_bool(jni, info, c"setFocusable", clickable(role));
    set_bool(jni, info, c"setFocused", node.focused);
    set_bool(jni, info, c"setClickable", clickable(role));
    set_bool(jni, info, c"setSelected", node.selected);
    set_bool(jni, info, c"setAccessibilityFocused", a11y_focused);
    if let Some(checked) = node.checked {
        set_bool(jni, info, c"setCheckable", true);
        set_bool(jni, info, c"setChecked", checked);
    }
    if editable(role) {
        set_bool(jni, info, c"setEditable", true);
    }
    if role == A11yRole::PasswordField {
        set_bool(jni, info, c"setPassword", true);
    }
    // A field's placeholder is its hint text, which TalkBack reads after an
    // empty field's role (`setHintText`, API 26; older systems go without).
    // A hint that can't be set leaves the rest of the node as it is.
    if !node.hint.is_empty()
        && sdk_int(jni) >= HINT_TEXT_SDK
        && let Some(hint) = jni.string16(&node.hint)
    {
        let _ = jni.call_void(info, c"setHintText", seq, &[JValue { l: hint }]);
    }
    if scrollable(&node) {
        set_bool(jni, info, c"setScrollable", true);
    }
    // A scroll bar's value: the offset in its range (`RangeInfo`, as a
    // seek bar's), which TalkBack reads as a percentage.
    if let Some(bar) = node.bar
        && let Some(range) = jni.call_static_object(
            c"android/view/accessibility/AccessibilityNodeInfo$RangeInfo",
            c"obtain",
            c"(IFFF)Landroid/view/accessibility/AccessibilityNodeInfo$RangeInfo;",
            &[
                JValue {
                    i: RANGE_TYPE_FLOAT,
                },
                JValue { f: 0.0 },
                JValue {
                    f: bar.max.max(0.0) as f32,
                },
                JValue {
                    f: bar.offset as f32,
                },
            ],
        )
    {
        let _ = jni.call_void(
            info,
            c"setRangeInfo",
            c"(Landroid/view/accessibility/AccessibilityNodeInfo$RangeInfo;)V",
            &[JValue { l: range }],
        );
    }
    // What it says of its range, table and caption leaves the rest of the
    // node as it is when a call fails.
    let _ = collection_info(jni, info, &node);
    if node.label > 0 {
        let _ = jni.call_void(
            info,
            c"setLabeledBy",
            c"(Landroid/view/View;I)V",
            &[JValue { l: view }, JValue { i: node.label }],
        );
    }
    let mut actions = vec![if a11y_focused {
        ACTION_CLEAR_ACCESSIBILITY_FOCUS
    } else {
        ACTION_ACCESSIBILITY_FOCUS
    }];
    if clickable(role) {
        actions.extend([ACTION_CLICK, ACTION_FOCUS]);
    }
    if matches!(
        role,
        A11yRole::ListItem | A11yRole::TreeItem | A11yRole::Row | A11yRole::Tab
    ) {
        actions.push(ACTION_SELECT);
    }
    if scrollable(&node) {
        let (back, forward) = scroll_ways(&node);
        if forward {
            actions.push(ACTION_SCROLL_FORWARD);
        }
        if back {
            actions.push(ACTION_SCROLL_BACKWARD);
        }
    }
    if editable(role) {
        actions.push(ACTION_SET_TEXT);
    }
    // A slider steps with scroll forward and backward, as a `SeekBar`.
    if node.adjustable() && !scrollable(&node) {
        let a = node.a11y();
        if a.step_range_events(true).is_some() {
            actions.push(ACTION_SCROLL_FORWARD);
        }
        if a.step_range_events(false).is_some() {
            actions.push(ACTION_SCROLL_BACKWARD);
        }
    }
    for a in actions {
        jni.call_void(info, c"addAction", c"(I)V", &[JValue { i: a }])?;
    }
    // Inside a list that scrolls, it can be brought on screen (API 23).
    if node.in_scroller
        && let Some(show) = show_on_screen(jni)
        && let Some(action) = jni.new_named(
            c"android/view/accessibility/AccessibilityNodeInfo$AccessibilityAction",
            c"(ILjava/lang/CharSequence;)V",
            &[
                JValue { i: show },
                JValue {
                    l: std::ptr::null_mut(),
                },
            ],
        )
    {
        let _ = jni.call_void(
            info,
            c"addAction",
            c"(Landroid/view/accessibility/AccessibilityNodeInfo$AccessibilityAction;)V",
            &[JValue { l: action }],
        );
    }
    if node.adjustable() && sdk_int(jni) >= SET_PROGRESS_SDK {
        let _ = add_standard_action(jni, info, ACTION_SET_PROGRESS);
    }
    Some(info)
}

/// Add the standard action `id` (one of `android.R.id.accessibilityAction…`)
/// as an `AccessibilityAction` object, the only way to add one past the
/// old bit mask.
fn add_standard_action(jni: &Jni, info: JObject, id: i32) -> Option<()> {
    let action = jni.new_named(
        c"android/view/accessibility/AccessibilityNodeInfo$AccessibilityAction",
        c"(ILjava/lang/CharSequence;)V",
        &[
            JValue { i: id },
            JValue {
                l: std::ptr::null_mut(),
            },
        ],
    )?;
    jni.call_void(
        info,
        c"addAction",
        c"(Landroid/view/accessibility/AccessibilityNodeInfo$AccessibilityAction;)V",
        &[JValue { l: action }],
    )
}

/// The node's `RangeInfo`, `CollectionInfo` or `CollectionItemInfo`, where
/// it has one.
fn collection_info(jni: &Jni, info: JObject, node: &Node) -> Option<()> {
    if let Some(r) = node.range {
        // As floats (an unbounded number input's ends clamped to them).
        let f = |v: f64| JValue {
            f: v.clamp(f32::MIN as f64, f32::MAX as f64) as f32,
        };
        let range = jni.call_static_object(
            c"android/view/accessibility/AccessibilityNodeInfo$RangeInfo",
            c"obtain",
            c"(IFFF)Landroid/view/accessibility/AccessibilityNodeInfo$RangeInfo;",
            &[
                JValue {
                    i: RANGE_TYPE_FLOAT,
                },
                f(r.min),
                f(r.max),
                f(r.value),
            ],
        )?;
        jni.call_void(
            info,
            c"setRangeInfo",
            c"(Landroid/view/accessibility/AccessibilityNodeInfo$RangeInfo;)V",
            &[JValue { l: range }],
        )?;
    }
    if let Some(t) = node.table {
        let collection = jni.call_static_object(
            c"android/view/accessibility/AccessibilityNodeInfo$CollectionInfo",
            c"obtain",
            c"(IIZ)Landroid/view/accessibility/AccessibilityNodeInfo$CollectionInfo;",
            &[
                JValue { i: t.rows as i32 },
                JValue {
                    i: t.columns as i32,
                },
                JValue { z: 0 },
            ],
        )?;
        jni.call_void(
            info,
            c"setCollectionInfo",
            c"(Landroid/view/accessibility/AccessibilityNodeInfo$CollectionInfo;)V",
            &[JValue { l: collection }],
        )?;
    }
    if let Some(c) = node.cell
        && matches!(node.role, A11yRole::Cell | A11yRole::ColumnHeader)
    {
        let item = jni.call_static_object(
            c"android/view/accessibility/AccessibilityNodeInfo$CollectionItemInfo",
            c"obtain",
            c"(IIIIZ)Landroid/view/accessibility/AccessibilityNodeInfo$CollectionItemInfo;",
            &[
                JValue { i: c.row as i32 },
                JValue { i: 1 },
                JValue { i: c.column as i32 },
                JValue { i: 1 },
                JValue {
                    z: u8::from(node.role == A11yRole::ColumnHeader),
                },
            ],
        )?;
        jni.call_void(
            info,
            c"setCollectionItemInfo",
            c"(Landroid/view/accessibility/AccessibilityNodeInfo$CollectionItemInfo;)V",
            &[JValue { l: item }],
        )?;
    }
    Some(())
}

/// The value `ACTION_SET_PROGRESS`'s arguments ask for.
fn progress_value(jni: &Jni, args: JObject) -> Option<f64> {
    if args.is_null() {
        return None;
    }
    let key = jni.string("android.view.accessibility.action.ARGUMENT_PROGRESS_VALUE")?;
    jni.call_float(
        args,
        c"getFloat",
        c"(Ljava/lang/String;)F",
        &[JValue { l: key }],
    )
    .map(f64::from)
}

/// Send accessibility event `kind` for virtual view `id`, through the host
/// view's parent as `ExploreByTouchHelper` does.
fn send_event(jni: &Jni, id: i32, kind: i32) -> Option<()> {
    let host = classes::host();
    let view = host.view.get();
    if view.is_null() {
        return None;
    }
    let (name, class) = {
        let t = tree();
        let node = t.nodes.get(usize::try_from(id).ok()?)?;
        (node.name.clone(), class_name(node.role))
    };
    let event = jni.call_static_object(
        c"android/view/accessibility/AccessibilityEvent",
        c"obtain",
        c"(I)Landroid/view/accessibility/AccessibilityEvent;",
        &[JValue { i: kind }],
    )?;
    let seq = c"(Ljava/lang/CharSequence;)V";
    jni.call_void(
        event,
        c"setPackageName",
        seq,
        &[JValue {
            l: host.package.get(),
        }],
    )?;
    let class = jni.string(class)?;
    jni.call_void(event, c"setClassName", seq, &[JValue { l: class }])?;
    jni.call_void(
        event,
        c"setSource",
        c"(Landroid/view/View;I)V",
        &[JValue { l: view }, JValue { i: id }],
    )?;
    if !name.is_empty() {
        let list = jni.call_object(event, c"getText", c"()Ljava/util/List;", &[])?;
        let name = jni.string16(&name)?;
        jni.call_bool(
            list,
            c"add",
            c"(Ljava/lang/Object;)Z",
            &[JValue { l: name }],
        )?;
    }
    let parent = jni.call_object(view, c"getParent", c"()Landroid/view/ViewParent;", &[])?;
    jni.call_bool(
        parent,
        c"requestSendAccessibilityEvent",
        c"(Landroid/view/View;Landroid/view/accessibility/AccessibilityEvent;)Z",
        &[JValue { l: view }, JValue { l: event }],
    )?;
    Some(())
}

/// `android.R.id.accessibilityActionShowOnScreen` (API 23; `None` before).
fn show_on_screen(jni: &Jni) -> Option<i32> {
    static ID: std::sync::OnceLock<Option<i32>> = std::sync::OnceLock::new();
    *ID.get_or_init(|| jni.static_int(c"android/R$id", c"accessibilityActionShowOnScreen"))
}

/// The events that bring node `path` into view, then tap it there (`tap`:
/// with the tap), from the tree the nodes came from.
fn revealed(path: &[usize], tap: bool) -> Option<Vec<Event>> {
    let t = tree();
    let root = t.source.as_ref()?;
    if tap {
        root.activate_events(path)
    } else {
        root.scroll_into_view_events(path)
    }
}

/// Tap `node`, brought into view first when a scroll container hides it.
fn tap_node(node: &Node) {
    match revealed(&node.path, true) {
        Some(events) => {
            for event in events.into_iter().chain([Event::PointerLeft]) {
                post_command(Command::Deliver(event));
            }
        }
        None => tap(node.center),
    }
}

/// Tap `at` (logical pixels), as a finger would.
fn tap(at: Point) {
    for event in [
        Event::PointerMoved { position: at },
        Event::PointerButton {
            button: PointerButton::Left,
            state: ButtonState::Pressed,
            position: at,
        },
        Event::PointerButton {
            button: PointerButton::Left,
            state: ButtonState::Released,
            position: at,
        },
        Event::PointerLeft,
    ] {
        post_command(Command::Deliver(event));
    }
}

/// Act on node `id`. Returns whether the action applies.
fn act(jni: &Jni, id: i32, action: i32, args: JObject) -> bool {
    let Some(node) = tree()
        .nodes
        .get(id.max(0) as usize)
        .filter(|_| id > 0)
        .cloned()
    else {
        return false;
    };
    match action {
        ACTION_ACCESSIBILITY_FOCUS => {
            let old = std::mem::replace(&mut tree().a11y_focus, id);
            if old != id {
                if old != HOST_VIEW_ID {
                    send_event(jni, old, TYPE_VIEW_ACCESSIBILITY_FOCUS_CLEARED);
                }
                send_event(jni, id, TYPE_VIEW_ACCESSIBILITY_FOCUSED);
            }
            true
        }
        ACTION_CLEAR_ACCESSIBILITY_FOCUS => {
            let mut t = tree();
            if t.a11y_focus == id {
                t.a11y_focus = HOST_VIEW_ID;
                drop(t);
                send_event(jni, id, TYPE_VIEW_ACCESSIBILITY_FOCUS_CLEARED);
            }
            true
        }
        ACTION_CLICK | ACTION_FOCUS | ACTION_SELECT => {
            tap_node(&node);
            if action == ACTION_CLICK {
                send_event(jni, id, TYPE_VIEW_CLICKED);
            }
            true
        }
        // A slider (a number input) steps.
        ACTION_SCROLL_FORWARD | ACTION_SCROLL_BACKWARD
            if node.adjustable() && !scrollable(&node) =>
        {
            match node
                .a11y()
                .step_range_events(action == ACTION_SCROLL_FORWARD)
            {
                Some(events) => {
                    for e in events {
                        post_command(Command::Deliver(e));
                    }
                    true
                }
                None => false,
            }
        }
        ACTION_SET_PROGRESS if node.adjustable() => {
            let Some(events) =
                progress_value(jni, args).and_then(|v| node.a11y().set_range_events(v))
            else {
                return false;
            };
            for e in events {
                post_command(Command::Deliver(e));
            }
            true
        }
        ACTION_SCROLL_FORWARD | ACTION_SCROLL_BACKWARD if scrollable(&node) => {
            let forward = action == ACTION_SCROLL_FORWARD;
            let (back, ahead) = scroll_ways(&node);
            if !(if forward { ahead } else { back }) {
                return false;
            }
            let (dx, dy) = scroll_wheel(&node, forward);
            post_command(Command::Deliver(Event::PointerMoved {
                position: node.center,
            }));
            post_command(Command::Deliver(Event::Scroll {
                delta: ScrollDelta::pixels(dx, dy),
            }));
            true
        }
        _ if Some(action) == show_on_screen(jni) && node.in_scroller => {
            // Scrolled until it shows (nothing when it does already).
            for event in revealed(&node.path, false).unwrap_or_default() {
                post_command(Command::Deliver(event));
            }
            true
        }
        ACTION_SET_TEXT if editable(node.role) => {
            let text = (!args.is_null())
                .then(|| {
                    let key = jni.string("ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE")?;
                    let cs = jni.call_object(
                        args,
                        c"getCharSequence",
                        c"(Ljava/lang/String;)Ljava/lang/CharSequence;",
                        &[JValue { l: key }],
                    )?;
                    jni.char_sequence(cs)
                })
                .flatten()
                .unwrap_or_default();
            tap(node.center);
            let ctrl = Modifiers {
                ctrl: true,
                ..Modifiers::default()
            };
            let press = |code, modifiers| {
                for state in [ButtonState::Pressed, ButtonState::Released] {
                    post_command(Command::Deliver(Event::Key {
                        code,
                        state,
                        modifiers,
                    }));
                }
            };
            press(KeyCode::Char('a'), ctrl);
            if text.is_empty() {
                press(KeyCode::Backspace, Modifiers::default());
            } else {
                post_command(Command::Deliver(Event::Text(text)));
            }
            true
        }
        _ => false,
    }
}

// ---- The natives ---------------------------------------------------------------------

pub(super) unsafe extern "C" fn node_provider(env: Env, _this: JObject) -> JObject {
    // A global reference, which a native method may return.
    unsafe { jni::in_native(env, classes::provider) }.unwrap_or(std::ptr::null_mut())
}

pub(super) unsafe extern "C" fn create_node_info(env: Env, _this: JObject, id: i32) -> JObject {
    unsafe { jni::in_native_object(env, |jni| node_info(jni, id)) }
}

pub(super) unsafe extern "C" fn perform_action(
    env: Env,
    _this: JObject,
    id: i32,
    action: i32,
    args: JObject,
) -> u8 {
    let done = unsafe {
        jni::in_native(env, |jni| {
            if id == HOST_VIEW_ID {
                let view = classes::host().view.get();
                return jni.call_bool(
                    view,
                    c"performAccessibilityAction",
                    c"(ILandroid/os/Bundle;)Z",
                    &[JValue { i: action }, JValue { l: args }],
                );
            }
            Some(act(jni, id, action, args))
        })
    };
    u8::from(done == Some(true))
}

pub(super) unsafe extern "C" fn find_focus(env: Env, _this: JObject, kind: i32) -> JObject {
    let id = {
        let t = tree();
        match kind {
            FOCUS_INPUT => t
                .nodes
                .iter()
                .skip(1)
                .position(|n| n.focused)
                .map(|i| i as i32 + 1),
            FOCUS_ACCESSIBILITY => (t.a11y_focus != HOST_VIEW_ID).then_some(t.a11y_focus),
            _ => None,
        }
    };
    match id {
        Some(id) => unsafe { jni::in_native_object(env, |jni| node_info(jni, id)) },
        None => std::ptr::null_mut(),
    }
}

/// A hover from touch exploration (the native input queue leaves a finger's
/// hover to the view): announce the node under it, as TalkBack expects.
pub(super) unsafe extern "C" fn dispatch_hover_event(
    env: Env,
    _this: JObject,
    event: JObject,
) -> u8 {
    let handled = unsafe {
        jni::in_native(env, |jni| {
            let action = jni.call_int(event, c"getActionMasked", c"()I", &[])?;
            let x = jni.call_float(event, c"getRawX", c"()F", &[])?;
            let y = jni.call_float(event, c"getRawY", c"()F", &[])?;
            let (ox, oy) = window_origin(jni).unwrap_or((0, 0));
            let (x, y) = (x as i32 - ox, y as i32 - oy);
            let hit = if action == ACTION_HOVER_EXIT {
                HOST_VIEW_ID
            } else {
                let t = tree();
                t.nodes
                    .iter()
                    .enumerate()
                    .skip(1)
                    .rev()
                    .find(|(_, n)| {
                        n.visible
                            && (n.rect.left..n.rect.right).contains(&x)
                            && (n.rect.top..n.rect.bottom).contains(&y)
                    })
                    .map_or(HOST_VIEW_ID, |(i, _)| i as i32)
            };
            let old = std::mem::replace(&mut tree().hover, hit);
            if old != hit {
                if hit != HOST_VIEW_ID {
                    send_event(jni, hit, TYPE_VIEW_HOVER_ENTER);
                }
                if old != HOST_VIEW_ID {
                    send_event(jni, old, TYPE_VIEW_HOVER_EXIT);
                }
            }
            Some(true)
        })
    };
    u8::from(handled == Some(true))
}
