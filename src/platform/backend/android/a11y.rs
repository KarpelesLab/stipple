//! Accessibility on Android: the host view's `AccessibilityNodeProvider`
//! (see [`classes`](super::classes)) gives TalkBack (and `uiautomator`) one
//! virtual view per node of the app's accessibility tree, with its role (as
//! the class name of the closest Android widget), text, state and screen
//! bounds, and acts on them: click, focus and select tap the node, scroll
//! forward and backward scroll it, and set text replaces a field's text.
//! Hover events from touch exploration (TalkBack's explore by touch) reach
//! the host view, which says which node is under the finger.
//!
//! The UI thread flattens each new tree into [`TREE`] (virtual view ids are
//! the nodes' indices in tree order; the root, the window, is the host view
//! itself) and asks the main thread to announce the change. The provider's
//! methods run on the main thread and read it from there.

use std::sync::Mutex;

use super::classes;
use super::jni::{self, Env, JObject, JValue, Jni};
use super::{ARect, Command, post_command};
use crate::geometry::Point;
use crate::platform::access::{A11yNode, A11yRole};
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

// AccessibilityEvent types.
const TYPE_VIEW_CLICKED: i32 = 0x1;
const TYPE_VIEW_FOCUSED: i32 = 0x8;
const TYPE_VIEW_HOVER_ENTER: i32 = 0x80;
const TYPE_VIEW_HOVER_EXIT: i32 = 0x100;
const TYPE_WINDOW_CONTENT_CHANGED: i32 = 0x800;
const TYPE_VIEW_ACCESSIBILITY_FOCUSED: i32 = 0x8000;
const TYPE_VIEW_ACCESSIBILITY_FOCUS_CLEARED: i32 = 0x1_0000;

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
    /// Whether any of it is on screen (inside the content).
    visible: bool,
}

/// The tree the provider serves. `nodes[0]` is the root (the host view).
#[derive(Debug)]
struct Tree {
    nodes: Vec<Node>,
    /// The node TalkBack has its focus on, and the one under a hovering finger.
    a11y_focus: i32,
    hover: i32,
}

static TREE: Mutex<Tree> = Mutex::new(Tree {
    nodes: Vec::new(),
    a11y_focus: HOST_VIEW_ID,
    hover: HOST_VIEW_ID,
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
}

impl Changes {
    pub(super) fn merge(&mut self, other: Changes) {
        self.content |= other.content;
        self.focused = other.focused.or(self.focused);
    }
}

/// Flatten `root` (logical pixels, laid out in `content`, window pixels, at
/// `scale`) into the tree the provider serves. On the UI thread. Returns what
/// changed.
pub(super) fn update(root: &A11yNode, content: ARect, scale: f64) -> Changes {
    fn walk(n: &A11yNode, parent: i32, content: ARect, scale: f64, out: &mut Vec<Node>) -> i32 {
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
            visible,
        });
        let children: Vec<i32> = n
            .children
            .iter()
            .map(|c| walk(c, id, content, scale, out))
            .collect();
        out[id as usize].children = children;
        id
    }
    let mut nodes = Vec::new();
    walk(root, 0, content, scale, &mut nodes);
    let mut t = tree();
    if t.nodes == nodes {
        return Changes::default();
    }
    let focus_of = |nodes: &[Node]| {
        nodes
            .iter()
            .skip(1)
            .position(|n| n.focused)
            .map(|i| i as i32 + 1)
    };
    let was = focus_of(&t.nodes);
    let now = focus_of(&nodes);
    t.nodes = nodes;
    if t.a11y_focus as usize >= t.nodes.len() {
        t.a11y_focus = HOST_VIEW_ID;
    }
    Changes {
        content: true,
        focused: now.filter(|_| now != was),
    }
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
}

/// The Android widget class a role reads as.
fn class_name(role: A11yRole) -> &'static str {
    match role {
        A11yRole::Button | A11yRole::MenuItem | A11yRole::Tab => "android.widget.Button",
        A11yRole::TextField | A11yRole::SpinButton | A11yRole::PasswordField => {
            "android.widget.EditText"
        }
        A11yRole::Text => "android.widget.TextView",
        A11yRole::CheckBox => "android.widget.CheckBox",
        A11yRole::Switch => "android.widget.Switch",
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

fn scrollable(role: A11yRole) -> bool {
    matches!(role, A11yRole::List | A11yRole::Tree | A11yRole::Table)
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
    if scrollable(role) {
        set_bool(jni, info, c"setScrollable", true);
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
    if scrollable(role) {
        actions.extend([ACTION_SCROLL_FORWARD, ACTION_SCROLL_BACKWARD]);
    }
    if editable(role) {
        actions.push(ACTION_SET_TEXT);
    }
    for a in actions {
        jni.call_void(info, c"addAction", c"(I)V", &[JValue { i: a }])?;
    }
    Some(info)
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
            tap(node.center);
            if action == ACTION_CLICK {
                send_event(jni, id, TYPE_VIEW_CLICKED);
            }
            true
        }
        ACTION_SCROLL_FORWARD | ACTION_SCROLL_BACKWARD => {
            let step = node.height * 0.8;
            let dy = if action == ACTION_SCROLL_FORWARD {
                step
            } else {
                -step
            };
            post_command(Command::Deliver(Event::PointerMoved {
                position: node.center,
            }));
            post_command(Command::Deliver(Event::Scroll {
                delta: ScrollDelta::pixels(0.0, dy),
            }));
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
