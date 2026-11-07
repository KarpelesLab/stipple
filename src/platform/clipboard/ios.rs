//! iOS clipboard: `UIPasteboard.generalPasteboard`'s `string` property.

use crate::platform::native::objc::{Id, class, msg, nsstring, rust_string, with_pool};

fn pasteboard() -> Id {
    unsafe { msg!(class("UIPasteboard"), "generalPasteboard", () -> Id) }
}

pub fn text() -> Option<String> {
    with_pool(|| {
        let pb = pasteboard();
        if pb.is_null() {
            return None;
        }
        rust_string(unsafe { msg!(pb, "string", () -> Id) })
    })
}

pub fn set_text(text: &str) -> bool {
    with_pool(|| {
        let pb = pasteboard();
        if pb.is_null() {
            return false;
        }
        let s = nsstring(text);
        unsafe { msg!(pb, "setString:", (s: Id) -> ()) };
        true
    })
}
