//! Keyboard focus through NSAccessibility: the focused node's element says
//! `AXFocused`, the view answers `accessibilityFocusedUIElement` with it (what
//! an assistive app reads as the application's `AXFocusedUIElement`), and a
//! focus move posts `AXFocusedUIElementChanged` on the newly focused element,
//! which VoiceOver follows (as UIA's focus-changed event on Windows, AT-SPI's
//! `focus:` on Linux). The focus is whatever the App's tree says: a text field
//! clicked or tabbed to, or a button reached with Tab.

use std::ffi::c_void;

use super::{Id, Sel, msg_id, msg_id_u64, msg_u64, nsstring, objc_msgSend, sel, window_for_view};
use crate::platform::access::A11yNode;

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSAccessibilityPostNotification(element: Id, notification: Id);
}

/// Mark a freshly made element focused (`setAccessibilityFocused:`), when its
/// node is.
///
/// # Safety
/// `el` is an `NSAccessibilityElement` (or null).
pub(super) unsafe fn mark(el: Id, node: &A11yNode) {
    if el.is_null() || !node.focused {
        return;
    }
    unsafe {
        let set: unsafe extern "C" fn(Id, Sel, bool) =
            std::mem::transmute(objc_msgSend as *const c_void);
        set(el, sel("setAccessibilityFocused:"), true);
    }
}

/// The element of the node at `path` (child indices from the tree's root)
/// under `view`, from the elements the view hands out.
fn element_at(view: Id, path: &[usize]) -> Option<Id> {
    let mut el = view;
    for &i in path {
        let children = unsafe { msg_id(el, sel("accessibilityChildren")) };
        if children.is_null() || i as u64 >= unsafe { msg_u64(children, sel("count")) } {
            return None;
        }
        el = unsafe { msg_id_u64(children, sel("objectAtIndex:"), i as u64) };
    }
    (!path.is_empty() && !el.is_null()).then_some(el)
}

/// `accessibilityFocusedUIElement` on the view: the focused node's element,
/// or the view itself when nothing in it has focus.
pub(super) extern "C" fn focused_ui_element(this: Id, _cmd: Sel) -> Id {
    let path = window_for_view(this).and_then(|w| {
        w.a11y_tree
            .borrow()
            .as_ref()
            .and_then(A11yNode::focused_path)
    });
    path.and_then(|p| element_at(this, &p)).unwrap_or(this)
}

/// Focus moved to the node at `path`: post `AXFocusedUIElementChanged` on
/// its element (built now, from the tree the view holds).
pub(super) fn announce(view: Id, path: &[usize]) {
    let Some(el) = element_at(view, path) else {
        return;
    };
    unsafe { NSAccessibilityPostNotification(el, nsstring("AXFocusedUIElementChanged")) };
    if std::env::var_os("STIPPLE_COCOA_A11Y").is_some() {
        let label = super::nsstring_to_string(unsafe { msg_id(el, sel("accessibilityLabel")) });
        println!("Cocoa a11y: focus moved to {label:?}");
    }
}
