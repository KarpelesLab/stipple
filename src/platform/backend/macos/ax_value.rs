//! Values, text ranges, scroll areas and change notifications through
//! NSAccessibility (what [`ax_plan`](super::ax_plan) works out, sent to
//! AppKit).
//!
//! Every element is an instance of a run-time subclass of
//! `NSAccessibilityElement`: `StippleAXElement`, `StippleAXPressElement`
//! (with `AXPress`, see [`ax_press`](super::ax_press)) or
//! `StippleAXScrollBar`. Each remembers the path of its node in the tree (an
//! associated object), and finds its window up its parents, as `AXPress`
//! does. On top of the properties `NSAccessibilityElement` stores, they:
//!
//! - forward a client's `setAccessibilityValue:` on a field to the app (its
//!   text replaced through the input-method path, after a click to focus
//!   it), and `setAccessibilitySelectedTextRange:` (the field selects the
//!   range); `isAccessibilitySelectorAllowed:` says only those are settable.
//!   The elements' own values are set through `NSAccessibilityElement`'s
//!   setters directly, so building them forwards nothing;
//! - add `AXScrollToVisible` to the actions of an element inside a scroll
//!   container (the legacy `accessibilityActionNames` /
//!   `accessibilityPerformAction:`, as the protocol has no method for it),
//!   which scrolls the container around it until it shows;
//! - as a scroll bar (an `AXScrollBar` child of a scroll area, a list, a
//!   tree or a table that scrolls, appended after its content and named as
//!   its `accessibilityVerticalScrollBar` or `…HorizontalScrollBar`), take an
//!   `AXValue` of `0..=1` (scrolling there) and `AXIncrement` /
//!   `AXDecrement` (a page on or back).
//!
//! What a client asks is queued for the window as events (a wheel, a click,
//! input-method edits; see `platform::access`) and delivered by the event
//! loop once AppKit returns.

use std::ffi::c_void;
use std::rc::Rc;

use super::ax_plan;
use super::{
    CTX, CgPoint, CgRect, CgSize, Class, Id, MacWindow, Sel, class, class_addMethod, msg_f64,
    msg_id, msg_void_id, nsstring, nsstring_to_string, objc_allocateClassPair, objc_getClass,
    objc_msgSend, objc_registerClassPair, post_wake_event, sel, window_for_view,
};
use crate::platform::access::{A11yChange, A11yNode};
use crate::platform::event::Event;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_setAssociatedObject(object: Id, key: *const c_void, value: Id, policy: usize);
    fn objc_getAssociatedObject(object: Id, key: *const c_void) -> Id;
    fn class_getMethodImplementation(cls: Class, name: Sel) -> *const c_void;
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSAccessibilityPostNotification(element: Id, notification: Id);
}

/// The key of the associated object holding an element's path.
static PATH_KEY: u8 = 0;
/// `OBJC_ASSOCIATION_RETAIN_NONATOMIC`.
const RETAIN_NONATOMIC: usize = 1;
/// `NSAccessibilityOrientationVertical` / `…Horizontal`.
const ORIENTATION_VERTICAL: i64 = 1;
const ORIENTATION_HORIZONTAL: i64 = 2;

/// `NSRange`.
#[repr(C)]
#[derive(Clone, Copy)]
struct NsRange {
    location: u64,
    length: u64,
}

/// What an element stands for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Node,
    Bar,
}

/// `NSAccessibilityElement`'s own implementation of `name`, to call on one
/// of our elements as `super` would.
fn base(name: &str) -> *const c_void {
    unsafe { class_getMethodImplementation(class("NSAccessibilityElement"), sel(name)) }
}

/// Set a property through `NSAccessibilityElement`'s setter (not ours,
/// which would forward it to the app).
unsafe fn store_id(el: Id, setter: &str, value: Id) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id) = std::mem::transmute(base(setter));
        f(el, sel(setter), value);
    }
}

unsafe fn store_i64(el: Id, setter: &str, value: i64) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, i64) = std::mem::transmute(base(setter));
        f(el, sel(setter), value);
    }
}

unsafe fn store_range(el: Id, setter: &str, (location, length): (usize, usize)) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, NsRange) = std::mem::transmute(base(setter));
        let r = NsRange {
            location: location as u64,
            length: length as u64,
        };
        f(el, sel(setter), r);
    }
}

fn number_i64(v: i64) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, i64) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(class("NSNumber"), sel("numberWithInteger:"), v)
    }
}

fn number_f64(v: f64) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, f64) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(class("NSNumber"), sel("numberWithDouble:"), v)
    }
}

/// The methods every one of our element classes has (see the module docs).
///
/// # Safety
/// `cls` is a class pair still being built.
pub(super) unsafe fn add_common(cls: Class) {
    let methods: [(&str, *const c_void, &std::ffi::CStr); 5] = [
        (
            "setAccessibilityValue:",
            set_value as *const c_void,
            c"v@:@",
        ),
        (
            "setAccessibilitySelectedTextRange:",
            set_selected_range as *const c_void,
            c"v@:{_NSRange=QQ}",
        ),
        (
            "isAccessibilitySelectorAllowed:",
            selector_allowed as *const c_void,
            c"c@::",
        ),
        (
            "accessibilityActionNames",
            action_names as *const c_void,
            c"@@:",
        ),
        (
            "accessibilityPerformAction:",
            perform_action as *const c_void,
            c"v@:@",
        ),
    ];
    for (name, imp, types) in methods {
        unsafe { class_addMethod(cls, sel(name), imp, types.as_ptr()) };
    }
}

/// A run-time subclass of `NSAccessibilityElement` named `name`, with the
/// common methods and `extra`.
fn subclass(name: &std::ffi::CStr, extra: &[(&str, *const c_void, &std::ffi::CStr)]) -> Class {
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("NSAccessibilityElement"), name.as_ptr(), 0);
        if cls.is_null() {
            return class("NSAccessibilityElement");
        }
        add_common(cls);
        for &(sel_name, imp, types) in extra {
            class_addMethod(cls, sel(sel_name), imp, types.as_ptr());
        }
        objc_registerClassPair(cls);
        cls
    }
}

/// `StippleAXElement`: the element of a node nothing presses.
pub(super) fn element_class() -> Class {
    subclass(c"StippleAXElement", &[])
}

/// `StippleAXScrollBar`: a scroll container's scroll bar.
fn bar_class() -> Class {
    subclass(
        c"StippleAXScrollBar",
        &[
            (
                "accessibilityPerformIncrement",
                bar_increment as *const c_void,
                c"c@:",
            ),
            (
                "accessibilityPerformDecrement",
                bar_decrement as *const c_void,
                c"c@:",
            ),
        ],
    )
}

/// A new (autoreleased) element of class `cls`, as
/// `+accessibilityElementWithRole:frame:label:parent:` makes one.
///
/// # Safety
/// `role` and `label` are `NSString`s, `parent` an accessibility element.
pub(super) unsafe fn new_element(cls: Class, role: Id, frame: CgRect, label: Id, parent: Id) -> Id {
    unsafe {
        let el = msg_id(msg_id(cls, sel("alloc")), sel("init"));
        if el.is_null() {
            return el;
        }
        let set_rect: unsafe extern "C" fn(Id, Sel, CgRect) =
            std::mem::transmute(objc_msgSend as *const c_void);
        msg_void_id(el, sel("setAccessibilityRole:"), role);
        set_rect(el, sel("setAccessibilityFrame:"), frame);
        msg_void_id(el, sel("setAccessibilityLabel:"), label);
        msg_void_id(el, sel("setAccessibilityParent:"), parent);
        msg_id(el, sel("autorelease"))
    }
}

/// The path an element remembers, as `"n:0/2"` (a node) or `"b:0/2"` (its
/// scroll bar).
fn tag(kind: Kind, path: &[usize]) -> String {
    let p: Vec<String> = path.iter().map(usize::to_string).collect();
    format!(
        "{}:{}",
        if kind == Kind::Bar { "b" } else { "n" },
        p.join("/")
    )
}

fn untag(s: &str) -> Option<(Kind, Vec<usize>)> {
    let (k, p) = s.split_once(':')?;
    let kind = match k {
        "n" => Kind::Node,
        "b" => Kind::Bar,
        _ => return None,
    };
    let path = if p.is_empty() {
        Vec::new()
    } else {
        p.split('/')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?
    };
    Some((kind, path))
}

unsafe fn remember(el: Id, kind: Kind, path: &[usize]) {
    unsafe {
        objc_setAssociatedObject(
            el,
            (&raw const PATH_KEY).cast(),
            nsstring(&tag(kind, path)),
            RETAIN_NONATOMIC,
        );
    }
}

/// The window an element is in (up its parents to the view), what it stands
/// for, and its node's path.
fn owner(el: Id) -> Option<(Rc<MacWindow>, Kind, Vec<usize>)> {
    unsafe {
        let s = objc_getAssociatedObject(el, (&raw const PATH_KEY).cast());
        if s.is_null() {
            return None;
        }
        let (kind, path) = untag(&nsstring_to_string(s))?;
        let mut at = el;
        for _ in 0..256 {
            at = msg_id(at, sel("accessibilityParent"));
            if at.is_null() {
                return None;
            }
            if let Some(w) = window_for_view(at) {
                return Some((w, kind, path));
            }
        }
        None
    }
}

/// Run `f` with the element's window, kind and node (its window's tree and
/// the node's path in it); `None` when it isn't in a window's tree any more.
fn with_node<R>(el: Id, f: impl FnOnce(&MacWindow, Kind, &A11yNode, &[usize]) -> R) -> Option<R> {
    let (win, kind, path) = owner(el)?;
    let tree = win.a11y_tree.try_borrow().ok()?;
    let root = tree.as_ref()?;
    Some(f(&win, kind, root, &path))
}

/// Queue `events` for `view`'s window and wake the loop to deliver them.
fn queue(view: Id, events: Vec<Event>) -> bool {
    if events.is_empty() {
        return true;
    }
    let queued = CTX.with(|c| {
        // (AppKit may ask from inside other work; never panic here.)
        let Ok(mut c) = c.try_borrow_mut() else {
            return false;
        };
        c.window_pending
            .extend(events.into_iter().map(|e| (view, e)));
        true
    });
    if queued {
        post_wake_event();
    }
    queued
}

fn debug() -> bool {
    std::env::var_os("STIPPLE_COCOA_A11Y").is_some()
}

/// `setAccessibilityValue:`: a field's new text, or where its scroll bar
/// scrolls to; stored as `NSAccessibilityElement` does for anything else.
extern "C" fn set_value(this: Id, cmd: Sel, value: Id) {
    let done = with_node(this, |win, kind, root, path| {
        let node = root.at_path(path)?;
        let events = match kind {
            Kind::Node if ax_plan::settable_text(node) => {
                let text = if value.is_null() {
                    String::new()
                } else {
                    nsstring_to_string(unsafe { msg_id(value, sel("description")) })
                };
                if debug() {
                    println!("Cocoa a11y: AXValue set to {text:?}");
                }
                node.set_text_events(&text)
            }
            Kind::Bar if !value.is_null() => {
                let fraction = unsafe { msg_f64(value, sel("doubleValue")) };
                if debug() {
                    println!("Cocoa a11y: scroll bar set to {fraction:.2}");
                }
                node.scroll_to_fraction_events(fraction).unwrap_or_default()
            }
            _ => return None,
        };
        Some(queue(win.view, events))
    })
    .flatten();
    if done.is_none() {
        unsafe {
            let f: unsafe extern "C" fn(Id, Sel, Id) =
                std::mem::transmute(base("setAccessibilityValue:"));
            f(this, cmd, value);
        }
    }
}

/// `setAccessibilitySelectedTextRange:`: the field selects the range.
extern "C" fn set_selected_range(this: Id, cmd: Sel, range: NsRange) {
    let done = with_node(this, |win, kind, root, path| {
        let node = root.at_path(path).filter(|_| kind == Kind::Node)?;
        ax_plan::text_value(node)?;
        let (anchor, caret) =
            ax_plan::range_bytes(node, range.location as usize, range.length as usize);
        if debug() {
            println!(
                "Cocoa a11y: AXSelectedTextRange set to {},{}",
                range.location, range.length
            );
        }
        Some(queue(win.view, node.select_events(anchor, caret)?))
    })
    .flatten();
    if done.is_none() {
        unsafe {
            let f: unsafe extern "C" fn(Id, Sel, NsRange) =
                std::mem::transmute(base("setAccessibilitySelectedTextRange:"));
            f(this, cmd, range);
        }
    }
}

/// `isAccessibilitySelectorAllowed:`: only a field's value and selection,
/// and a scroll bar's value, are a client's to set.
extern "C" fn selector_allowed(this: Id, cmd: Sel, selector: Sel) -> bool {
    let value = selector == sel("setAccessibilityValue:");
    let range = selector == sel("setAccessibilitySelectedTextRange:");
    if value || range {
        return with_node(this, |_, kind, root, path| {
            let Some(node) = root.at_path(path) else {
                return false;
            };
            match kind {
                Kind::Bar => value,
                Kind::Node if value => ax_plan::settable_text(node),
                Kind::Node => ax_plan::text_value(node).is_some(),
            }
        })
        .unwrap_or(false);
    }
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Sel) -> i8 =
            std::mem::transmute(base("isAccessibilitySelectorAllowed:"));
        f(this, cmd, selector) != 0
    }
}

/// The protocol's action methods and the action each stands for.
const PROTOCOL_ACTIONS: [(&str, &str); 8] = [
    ("accessibilityPerformPress", "AXPress"),
    ("accessibilityPerformIncrement", "AXIncrement"),
    ("accessibilityPerformDecrement", "AXDecrement"),
    ("accessibilityPerformShowMenu", "AXShowMenu"),
    ("accessibilityPerformCancel", "AXCancel"),
    ("accessibilityPerformConfirm", "AXConfirm"),
    ("accessibilityPerformPick", "AXPick"),
    ("accessibilityPerformRaise", "AXRaise"),
];

/// The legacy `accessibilityActionNames`: the actions of the protocol
/// methods the element implements (and allows), plus `AXScrollToVisible`
/// inside a scroll container. Overriding the legacy method means AppKit no
/// longer derives the list from the protocol for us (its own legacy
/// implementation returns nothing for an `NSAccessibilityElement`), so the
/// protocol's actions are listed here.
extern "C" fn action_names(this: Id, cmd: Sel) -> Id {
    let base_names = unsafe {
        let f: unsafe extern "C" fn(Id, Sel) -> Id =
            std::mem::transmute(base("accessibilityActionNames"));
        f(this, cmd)
    };
    let inside = with_node(this, |_, kind, root, path| {
        kind == Kind::Node && ax_plan::in_scroll(root, path)
    })
    .unwrap_or(false);
    unsafe {
        let list = if base_names.is_null() {
            msg_id(class("NSMutableArray"), sel("array"))
        } else {
            msg_id(msg_id(base_names, sel("mutableCopy")), sel("autorelease"))
        };
        let responds: unsafe extern "C" fn(Id, Sel, Sel) -> i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        let contains: unsafe extern "C" fn(Id, Sel, Id) -> i8 =
            std::mem::transmute(objc_msgSend as *const c_void);
        for (method, action) in PROTOCOL_ACTIONS {
            let m = sel(method);
            if responds(this, sel("respondsToSelector:"), m) == 0
                || !selector_allowed(this, sel("isAccessibilitySelectorAllowed:"), m)
            {
                continue;
            }
            let name = nsstring(action);
            if contains(list, sel("containsObject:"), name) == 0 {
                msg_void_id(list, sel("addObject:"), name);
            }
        }
        if inside {
            msg_void_id(list, sel("addObject:"), nsstring("AXScrollToVisible"));
        }
        list
    }
}

/// The legacy `accessibilityPerformAction:`: `AXScrollToVisible` scrolls the
/// element into view; the rest is AppKit's.
extern "C" fn perform_action(this: Id, cmd: Sel, action: Id) {
    if nsstring_to_string(action) == "AXScrollToVisible" {
        with_node(this, |win, _, root, path| {
            if debug() {
                println!("Cocoa a11y: AXScrollToVisible {path:?}");
            }
            if let Some(events) = root.scroll_into_view_events(path) {
                queue(win.view, events);
            }
        });
        return;
    }
    // The protocol's actions, which the list above names: call the method
    // (AppKit's legacy implementation doesn't, once it is overridden).
    let name = nsstring_to_string(action);
    if let Some((method, _)) = PROTOCOL_ACTIONS.iter().find(|(_, a)| *a == name) {
        unsafe {
            let responds: unsafe extern "C" fn(Id, Sel, Sel) -> i8 =
                std::mem::transmute(objc_msgSend as *const c_void);
            if responds(this, sel("respondsToSelector:"), sel(method)) != 0 {
                let f: unsafe extern "C" fn(Id, Sel) -> i8 =
                    std::mem::transmute(objc_msgSend as *const c_void);
                f(this, sel(method));
                return;
            }
        }
    }
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, Id) =
            std::mem::transmute(base("accessibilityPerformAction:"));
        f(this, cmd, action);
    }
}

/// A page on or back, from a scroll bar.
fn bar_page(this: Id, forward: bool) -> bool {
    with_node(this, |win, _, root, path| {
        let events = root.at_path(path)?.scroll_page_events(forward)?;
        Some(queue(win.view, events))
    })
    .flatten()
    .unwrap_or(false)
}

extern "C" fn bar_increment(this: Id, _cmd: Sel) -> bool {
    bar_page(this, true)
}

extern "C" fn bar_decrement(this: Id, _cmd: Sel) -> bool {
    bar_page(this, false)
}

/// Give a freshly made element `el` for `node` (at `path` under `root`) what
/// it says beyond its role and label: its path, subrole, value, text
/// ranges, and a scroll area's role.
///
/// # Safety
/// `el` is one of our elements (or null).
pub(super) unsafe fn decorate(el: Id, node: &A11yNode, path: &[usize], root: Option<&A11yNode>) {
    if el.is_null() {
        return;
    }
    unsafe {
        remember(el, Kind::Node, path);
        if let Some(role) = ax_plan::role_override(node) {
            msg_void_id(el, sel("setAccessibilityRole:"), nsstring(role));
        }
        if let Some(sub) = ax_plan::subrole(node) {
            msg_void_id(el, sel("setAccessibilitySubrole:"), nsstring(sub));
        }
        if let Some(label) = ax_plan::field_label(node, root) {
            msg_void_id(el, sel("setAccessibilityLabel:"), nsstring(&label));
        }
        if let Some(text) = ax_plan::text_value(node) {
            store_id(el, "setAccessibilityValue:", nsstring(text));
            let len = ax_plan::utf16_len(text);
            store_i64(el, "setAccessibilityNumberOfCharacters:", len as i64);
            store_i64(el, "setAccessibilityInsertionPointLineNumber:", 0);
            store_range(el, "setAccessibilityVisibleCharacterRange:", (0, len));
            if let Some((at, n)) = ax_plan::selected_range(node) {
                store_range(el, "setAccessibilitySelectedTextRange:", (at, n));
                let t: Vec<u16> = text.encode_utf16().skip(at).take(n).collect();
                let selected = String::from_utf16_lossy(&t);
                store_id(el, "setAccessibilitySelectedText:", nsstring(&selected));
            }
        } else if let Some(n) = ax_plan::number_value(node) {
            store_id(el, "setAccessibilityValue:", number_i64(n));
        }
    }
}

/// The scroll bar of `el`, the element of scroll container `node` (at
/// `path`) whose frame is `frame` (screen coordinates): made, named as its
/// vertical or horizontal scroll bar, and returned to be appended to its
/// children. `None` when its content fits.
///
/// # Safety
/// `el` is one of our elements.
pub(super) unsafe fn scroll_bar(
    el: Id,
    node: &A11yNode,
    path: &[usize],
    frame: CgRect,
) -> Option<Id> {
    let s = node.scroll.filter(|s| s.scrolls())?;
    let f = (
        frame.origin.x,
        frame.origin.y,
        frame.size.width,
        frame.size.height,
    );
    let [x, y, w, h] = ax_plan::bar_frame(f, s.horizontal, s.rtl);
    let bar_frame = CgRect {
        origin: CgPoint { x, y },
        size: CgSize {
            width: w,
            height: h,
        },
    };
    unsafe {
        let bar = new_element(
            bar_class(),
            nsstring("AXScrollBar"),
            bar_frame,
            nsstring(""),
            el,
        );
        if bar.is_null() {
            return None;
        }
        remember(bar, Kind::Bar, path);
        let orientation = if s.horizontal {
            ORIENTATION_HORIZONTAL
        } else {
            ORIENTATION_VERTICAL
        };
        store_i64(bar, "setAccessibilityOrientation:", orientation);
        store_id(bar, "setAccessibilityValue:", number_f64(s.fraction()));
        store_id(bar, "setAccessibilityMinValue:", number_f64(0.0));
        store_id(bar, "setAccessibilityMaxValue:", number_f64(1.0));
        let which = if s.horizontal {
            "setAccessibilityHorizontalScrollBar:"
        } else {
            "setAccessibilityVerticalScrollBar:"
        };
        msg_void_id(el, sel(which), bar);
        Some(bar)
    }
}

/// What an element says of its value, for the in-process dump: `value=…`
/// (and `subrole=…`), or nothing.
pub(super) fn describe(el: Id) -> String {
    let mut out = String::new();
    unsafe {
        let v = msg_id(el, sel("accessibilityValue"));
        if !v.is_null() {
            let s = nsstring_to_string(msg_id(v, sel("description")));
            out.push_str(&format!(" value={s}"));
        }
        let sub = nsstring_to_string(msg_id(el, sel("accessibilitySubrole")));
        if !sub.is_empty() {
            out.push_str(&format!(" subrole={sub}"));
        }
    }
    out
}

/// The tree went from `old` to `new` under `view` (its elements made again):
/// post the notifications for what changed on the new elements.
pub(super) fn announce(view: Id, old: Option<&A11yNode>, new: &A11yNode) {
    let Some(old) = old else {
        return;
    };
    let changes: Vec<A11yChange> = crate::platform::access::changes(old, new);
    for post in ax_plan::plan(new, &changes) {
        let el = if post.path.is_empty() {
            Some(view)
        } else {
            super::ax_focus::element_at(view, &post.path)
        };
        let Some(mut el) = el else {
            continue;
        };
        if post.bar {
            let s = new.at_path(&post.path).and_then(|n| n.scroll);
            let which = if s.is_some_and(|s| s.horizontal) {
                "accessibilityHorizontalScrollBar"
            } else {
                "accessibilityVerticalScrollBar"
            };
            el = unsafe { msg_id(el, sel(which)) };
            if el.is_null() {
                continue;
            }
        }
        unsafe { NSAccessibilityPostNotification(el, nsstring(post.name)) };
        if debug() {
            println!("Cocoa a11y: posted {} at {:?}", post.name, post.path);
        }
    }
}
