//! macOS clipboard: `NSPasteboard.generalPasteboard`, plain UTF-8 text.

use crate::platform::native::objc::{Id, class, msg, nsstring, rust_string, with_pool};

/// `NSPasteboardTypeString`'s value: the plain-text UTI.
const TYPE_STRING: &str = "public.utf8-plain-text";

fn pasteboard() -> Id {
    unsafe { msg!(class("NSPasteboard"), "generalPasteboard", () -> Id) }
}

pub fn text() -> Option<String> {
    with_pool(|| {
        let pb = pasteboard();
        if pb.is_null() {
            return None;
        }
        let ty = nsstring(TYPE_STRING);
        let s = unsafe { msg!(pb, "stringForType:", (ty: Id) -> Id) };
        rust_string(s)
    })
}

pub fn set_text(text: &str) -> bool {
    with_pool(|| {
        let pb = pasteboard();
        if pb.is_null() {
            return false;
        }
        let s = nsstring(text);
        let ty = nsstring(TYPE_STRING);
        unsafe {
            // `clearContents` returns the new change count (NSInteger); writing
            // without it first would be ignored by the pasteboard.
            let _ = msg!(pb, "clearContents", () -> isize);
            msg!(pb, "setString:forType:", (s: Id, ty: Id) -> bool)
        }
    })
}
