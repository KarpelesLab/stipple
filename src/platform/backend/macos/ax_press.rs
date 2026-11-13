//! Pressing a control through NSAccessibility: `AXPress`, which VoiceOver
//! sends for VO-Space and an assistive app with `AXUIElementPerformAction`.
//!
//! The elements of the controls a click activates (buttons, menu items,
//! checkboxes, switches, selects, tabs, column headers) are instances of
//! `StippleAXPressElement`, an `NSAccessibilityElement` subclass made at run
//! time whose `accessibilityPerformPress` clicks the middle of the element's
//! frame: a pointer move, press and release at that point of the view,
//! queued for the window and delivered by the event loop once AppKit returns
//! (as iOS's `accessibilityActivate` taps its node). AppKit lists `AXPress`
//! among an element's actions when it implements the method, so the other
//! elements, which have nothing to press, don't get the class.

use std::ffi::c_void;

use super::{
    CTX, CgPoint, CgRect, Class, Id, Sel, class, class_addMethod, msg_id, msg_rect,
    objc_allocateClassPair, objc_getClass, objc_msgSend, objc_registerClassPair, post_wake_event,
    sel, window_for_view,
};
use crate::geometry::Point;
use crate::platform::access::A11yRole;
use crate::platform::event::{ButtonState, Event, PointerButton};

/// Whether a click activates a node of `role`, so its element has `AXPress`.
pub(super) fn pressable(role: A11yRole) -> bool {
    use A11yRole::*;
    matches!(
        role,
        Button | MenuItem | CheckBox | Switch | ComboBox | Tab | ColumnHeader
    )
}

/// `StippleAXPressElement`, made on first use.
pub(super) fn press_class() -> Class {
    let name = c"StippleAXPressElement";
    unsafe {
        let existing = objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = objc_allocateClassPair(class("NSAccessibilityElement"), name.as_ptr(), 0);
        if cls.is_null() {
            return class("NSAccessibilityElement");
        }
        class_addMethod(
            cls,
            sel("accessibilityPerformPress"),
            perform_press as *const c_void,
            c"c@:".as_ptr(),
        );
        objc_registerClassPair(cls);
        cls
    }
}

/// A new (autoreleased) `StippleAXPressElement` with `role`, `frame` (screen
/// coordinates), `label` and `parent`, as
/// `+accessibilityElementWithRole:frame:label:parent:` makes one.
///
/// # Safety
/// `role` and `label` are `NSString`s, `parent` an accessibility element.
pub(super) unsafe fn new_element(role: Id, frame: CgRect, label: Id, parent: Id) -> Id {
    unsafe {
        let el = msg_id(msg_id(press_class(), sel("alloc")), sel("init"));
        if el.is_null() {
            return el;
        }
        let set_id: unsafe extern "C" fn(Id, Sel, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        let set_rect: unsafe extern "C" fn(Id, Sel, CgRect) =
            std::mem::transmute(objc_msgSend as *const c_void);
        set_id(el, sel("setAccessibilityRole:"), role);
        set_rect(el, sel("setAccessibilityFrame:"), frame);
        set_id(el, sel("setAccessibilityLabel:"), label);
        set_id(el, sel("setAccessibilityParent:"), parent);
        msg_id(el, sel("autorelease"))
    }
}

/// `accessibilityPerformPress`: click the middle of the element, in the
/// window whose view it is under. `NO` when it isn't under one any more.
extern "C" fn perform_press(this: Id, _cmd: Sel) -> bool {
    unsafe {
        // The view: up the parents (each element's parent is set when it's
        // made; the top one's is the view).
        let mut at = this;
        let mut win = None;
        for _ in 0..256 {
            at = msg_id(at, sel("accessibilityParent"));
            if at.is_null() {
                break;
            }
            if let Some(w) = window_for_view(at) {
                win = Some(w);
                break;
            }
        }
        let Some(win) = win else {
            println!("Cocoa a11y: AXPress on an element outside any window");
            return false;
        };
        let frame = msg_rect(this, sel("accessibilityFrame"));
        let mid = CgPoint {
            x: frame.origin.x + frame.size.width / 2.0,
            y: frame.origin.y + frame.size.height / 2.0,
        };
        // Screen to window coordinates (bottom-left origin), then to the
        // view's (top-left), as a click's `locationInWindow` is read.
        let from_screen: unsafe extern "C" fn(Id, Sel, CgPoint) -> CgPoint =
            std::mem::transmute(objc_msgSend as *const c_void);
        let p = from_screen(win.window, sel("convertPointFromScreen:"), mid);
        let view_h = msg_rect(win.view, sel("bounds")).size.height;
        let position = Point::new(p.x, view_h - p.y);
        let label = super::nsstring_to_string(msg_id(this, sel("accessibilityLabel")));
        println!(
            "Cocoa a11y: AXPress {label:?} at {:.0},{:.0}",
            position.x, position.y
        );
        let view = win.view;
        let queued = CTX.with(|c| {
            // (AppKit may ask from inside other work; never panic here.)
            let Ok(mut c) = c.try_borrow_mut() else {
                return false;
            };
            c.window_pending
                .push((view, Event::PointerMoved { position }));
            for state in [ButtonState::Pressed, ButtonState::Released] {
                c.window_pending.push((
                    view,
                    Event::PointerButton {
                        button: PointerButton::Left,
                        state,
                        position,
                    },
                ));
            }
            true
        });
        if !queued {
            println!("Cocoa a11y: AXPress {label:?} came while the event state was in use");
            return false;
        }
        // The loop may be waiting for an event: wake it to deliver these.
        post_wake_event();
        true
    }
}
