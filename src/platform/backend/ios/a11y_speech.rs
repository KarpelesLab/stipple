//! Rich text read in its languages: the elements' `accessibilityAttributedLabel`
//! (static text) and `accessibilityAttributedValue` (a field's text) carry
//! `UIAccessibilitySpeechAttributeLanguage` over the runs in another
//! language, so VoiceOver switches voice there. Built when VoiceOver asks,
//! from the runs the item's node carries (see `a11y_tree::speech_languages`);
//! anything else is `UIAccessibilityElement`'s own.

use std::ffi::c_void;

use super::a11y_tree::speech_languages;
use super::{
    Class, Id, ObjcSuper, Sel, class, class_addMethod, msg_id, nsstring, objc_msgSend,
    objc_msgSendSuper, sel,
};
use crate::platform::access::{A11yNode, A11yRole};

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static UIAccessibilitySpeechAttributeLanguage: Id;
}

/// `NSRange`.
#[repr(C)]
#[derive(Clone, Copy)]
struct NsRange {
    location: u64,
    length: u64,
}

/// Add the two getters to the element class `cls` (being built).
///
/// # Safety
/// `cls` is a class pair not yet registered.
pub(super) unsafe fn add_methods(cls: Class) {
    unsafe {
        class_addMethod(
            cls,
            sel("accessibilityAttributedLabel"),
            attributed_label as *const c_void,
            c"@@:".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("accessibilityAttributedValue"),
            attributed_value as *const c_void,
            c"@@:".as_ptr(),
        );
    }
}

/// `UIAccessibilityElement`'s own answer to `cmd`.
fn base(this: Id, cmd: Sel) -> Id {
    unsafe {
        let sup = ObjcSuper {
            receiver: this,
            super_class: class("UIAccessibilityElement"),
        };
        let f: unsafe extern "C" fn(*const ObjcSuper, Sel) -> Id =
            std::mem::transmute(objc_msgSendSuper as *const c_void);
        f(&sup, cmd)
    }
}

/// `node`'s text with its languages marked; `None` when it has none.
fn attributed(node: &A11yNode) -> Option<Id> {
    let langs = speech_languages(node);
    if langs.is_empty() {
        return None;
    }
    unsafe {
        let init: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let alloc = msg_id(class("NSMutableAttributedString"), sel("alloc"));
        let text = nsstring(&node.name.replace('\0', ""));
        let string = init(alloc, sel("initWithString:"), text);
        if string.is_null() {
            return None;
        }
        let add: unsafe extern "C" fn(Id, Sel, Id, Id, NsRange) =
            std::mem::transmute(objc_msgSend as *const c_void);
        for (location, length, tag) in langs {
            let range = NsRange {
                location: location as u64,
                length: length as u64,
            };
            add(
                string,
                sel("addAttribute:value:range:"),
                UIAccessibilitySpeechAttributeLanguage,
                nsstring(&tag.replace('\0', "")),
                range,
            );
        }
        Some(msg_id(string, sel("autorelease")))
    }
}

/// The attributed text of element `this` when its node is rich text for
/// which `wanted` holds; else the element's own.
fn answer(this: Id, cmd: Sel, wanted: fn(A11yRole) -> bool) -> Id {
    super::a11y::speech_of(this)
        .filter(|n| wanted(n.role))
        .and_then(|n| attributed(&n))
        .unwrap_or_else(|| base(this, cmd))
}

extern "C" fn attributed_label(this: Id, cmd: Sel) -> Id {
    answer(this, cmd, |r| r == A11yRole::Text)
}

extern "C" fn attributed_value(this: Id, cmd: Sel) -> Id {
    answer(this, cmd, |r| {
        matches!(r, A11yRole::TextField | A11yRole::SpinButton)
    })
}
