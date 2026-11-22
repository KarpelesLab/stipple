//! What the macOS elements say of each other and of static text, set once
//! the window's elements are all made (an element can only name another
//! that exists): from `platform::access::relations`,
//!
//! - a control labelled by a caption (`Element::labelled_by`) has the
//!   caption's element as its `AXTitleUIElement`, as an `NSTextField` with
//!   a label beside it does in AppKit (and the caption the control among
//!   its `AXServesAsTitleForUIElements`);
//! - a control that controls another (`Element::controls`: a select's open
//!   list, a tab's panel) has it among its `AXLinkedUIElements`, which
//!   VoiceOver offers to jump to (VO-Command-J);
//! - a tree item (an outline row) has its `AXDisclosureLevel` (0 at the
//!   top), its parent row as `AXDisclosedByRow` and its child rows as
//!   `AXDisclosedRows`;
//!
//! and static text has its text as its `AXValue` (as AppKit's labels do;
//! the label stays too), with its `AXNumberOfCharacters` and, when part of
//! selectable text is selected, its `AXSelectedTextRange` and
//! `AXSelectedText`.

use std::ffi::c_void;

use super::{Class, Id, Sel, class, msg_id, msg_id_u64, msg_u64, nsstring, objc_msgSend, sel};
use crate::platform::access::{A11yNode, A11yRole};

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn class_getMethodImplementation(cls: Class, name: Sel) -> *const c_void;
}

/// `NSRange`.
#[repr(C)]
struct NsRange {
    location: u64,
    length: u64,
}

/// Set a property through `NSAccessibilityElement`'s own setter (not one
/// of ours, which forward a client's value to the app).
unsafe fn store<T>(el: Id, setter: &str, value: T) {
    unsafe {
        let imp = class_getMethodImplementation(class("NSAccessibilityElement"), sel(setter));
        let f: unsafe extern "C" fn(Id, Sel, T) = std::mem::transmute(imp);
        f(el, sel(setter), value);
    }
}

/// Send setter `setter` to `el` (one of ours or a live OS control).
unsafe fn send<T>(el: Id, setter: &str, value: T) {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, T) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(el, sel(setter), value);
    }
}

/// The element of the node at `path`, under the window root's `top`
/// elements (an `NSArray`, in its children's order; each element's own
/// children after it in the same order).
fn element_at(top: Id, path: &[usize]) -> Option<Id> {
    let (&first, rest) = path.split_first()?;
    let mut list = top;
    let mut el;
    let mut i = first;
    let mut rest = rest.iter();
    loop {
        if list.is_null() || i as u64 >= unsafe { msg_u64(list, sel("count")) } {
            return None;
        }
        el = unsafe { msg_id_u64(list, sel("objectAtIndex:"), i as u64) };
        match rest.next() {
            Some(&next) => {
                list = unsafe { msg_id(el, sel("accessibilityChildren")) };
                i = next;
            }
            None => return (!el.is_null()).then_some(el),
        }
    }
}

/// An autoreleased `NSArray` of the elements at `paths` (those found).
fn elements(top: Id, paths: &[Vec<usize>]) -> Option<Id> {
    let found: Vec<Id> = paths.iter().filter_map(|p| element_at(top, p)).collect();
    if found.is_empty() {
        return None;
    }
    unsafe {
        let arr = msg_id(class("NSMutableArray"), sel("array"));
        for el in found {
            let add: unsafe extern "C" fn(Id, Sel, Id) =
                std::mem::transmute(objc_msgSend as *const c_void);
            add(arr, sel("addObject:"), el);
        }
        Some(arr)
    }
}

/// Set the links and static text values on the elements made for `root`'s
/// children (`top`, in order).
///
/// # Safety
/// `top` is the `NSArray` of the root's children's elements.
pub(super) unsafe fn link(top: Id, root: &A11yNode) {
    fn walk(n: &A11yNode, path: &mut Vec<usize>, f: &mut dyn FnMut(&A11yNode, &[usize])) {
        if !path.is_empty() {
            f(n, path);
        }
        for (i, c) in n.children.iter().enumerate() {
            path.push(i);
            walk(c, path, f);
            path.pop();
        }
    }
    walk(root, &mut Vec::new(), &mut |n, path| {
        // (A live OS control is its own element: it takes the links too,
        // through its own setters.)
        let Some(el) = element_at(top, path) else {
            return;
        };
        let r = root.relations(path);
        unsafe {
            if let Some(title) = r.labelled_by.first().and_then(|p| element_at(top, p)) {
                send(el, "setAccessibilityTitleUIElement:", title);
            }
            if let Some(fields) = elements(top, &r.label_for) {
                send(el, "setAccessibilityServesAsTitleForUIElements:", fields);
            }
            if let Some(linked) = elements(top, &r.controller_for) {
                send(el, "setAccessibilityLinkedUIElements:", linked);
            }
            if let Some(level) = root.tree_level(path) {
                send(el, "setAccessibilityDisclosureLevel:", level as i64 - 1);
                if let Some(parent) = r.node_child_of.as_deref().and_then(|p| element_at(top, p)) {
                    send(el, "setAccessibilityDisclosedByRow:", parent);
                }
                if let Some(rows) = elements(top, &r.node_parent_of) {
                    send(el, "setAccessibilityDisclosedRows:", rows);
                }
            }
            if n.role == A11yRole::Text && !n.name.is_empty() && n.native_view.is_none() {
                static_text(el, n);
            }
        }
    });
}

/// A static text's `AXValue`, length and selection.
unsafe fn static_text(el: Id, n: &A11yNode) {
    use super::ax_plan::{utf16_at, utf16_len};
    let text = n.name.as_str();
    unsafe {
        store(el, "setAccessibilityValue:", nsstring(text));
        store(
            el,
            "setAccessibilityNumberOfCharacters:",
            utf16_len(text) as i64,
        );
        if let Some((s, e)) = n.text_selection {
            let (s, e) = (utf16_at(text, s), utf16_at(text, e));
            let (at, len) = (s.min(e), s.abs_diff(e));
            store(
                el,
                "setAccessibilitySelectedTextRange:",
                NsRange {
                    location: at as u64,
                    length: len as u64,
                },
            );
            let t: Vec<u16> = text.encode_utf16().skip(at).take(len).collect();
            store(
                el,
                "setAccessibilitySelectedText:",
                nsstring(&String::from_utf16_lossy(&t)),
            );
        }
    }
}
