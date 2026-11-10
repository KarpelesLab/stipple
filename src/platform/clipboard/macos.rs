//! macOS clipboard: `NSPasteboard.generalPasteboard`, as plain UTF-8 text
//! (`public.utf8-plain-text`), HTML (`public.html`) and PNG (`public.png`).

use super::format::encode_png;
use super::{ClipboardContent, ClipboardFormats};
use crate::platform::native::objc::{Id, class, msg, nsstring, rust_string, with_pool};
use crate::render::Pixmap;

/// `NSPasteboardTypeString`'s value: the plain-text UTI.
const TYPE_STRING: &str = "public.utf8-plain-text";
/// `NSPasteboardTypeHTML`.
const TYPE_HTML: &str = "public.html";
/// `NSPasteboardTypePNG`.
const TYPE_PNG: &str = "public.png";

fn pasteboard() -> Id {
    unsafe { msg!(class("NSPasteboard"), "generalPasteboard", () -> Id) }
}

/// The pasteboard's string for `ty`.
fn string(pb: Id, ty: &str) -> Option<String> {
    let ty = nsstring(ty);
    rust_string(unsafe { msg!(pb, "stringForType:", (ty: Id) -> Id) })
}

/// The pasteboard's data for `ty`.
fn data(pb: Id, ty: &str) -> Option<Vec<u8>> {
    let ty = nsstring(ty);
    unsafe {
        let data = msg!(pb, "dataForType:", (ty: Id) -> Id);
        if data.is_null() {
            return None;
        }
        let len = msg!(data, "length", () -> usize);
        let bytes = msg!(data, "bytes", () -> *const u8);
        if bytes.is_null() {
            return None;
        }
        Some(std::slice::from_raw_parts(bytes, len).to_vec())
    }
}

pub fn text() -> Option<String> {
    with_pool(|| {
        let pb = pasteboard();
        if pb.is_null() {
            return None;
        }
        string(pb, TYPE_STRING)
    })
}

pub fn set_text(text: &str) -> bool {
    write(&ClipboardContent::text(text))
}

pub fn read(formats: ClipboardFormats) -> Option<ClipboardContent> {
    with_pool(|| {
        let pb = pasteboard();
        if pb.is_null() {
            return None;
        }
        let content = ClipboardContent {
            text: formats.text.then(|| string(pb, TYPE_STRING)).flatten(),
            html: formats.html.then(|| string(pb, TYPE_HTML)).flatten(),
            image: formats
                .image
                .then(|| data(pb, TYPE_PNG))
                .flatten()
                .and_then(|png| Pixmap::decode(&png).ok()),
        };
        (!content.is_empty()).then_some(content)
    })
}

pub fn write(content: &ClipboardContent) -> bool {
    let png = content.image.as_ref().and_then(encode_png);
    with_pool(|| {
        let pb = pasteboard();
        if pb.is_null() || content.is_empty() {
            return false;
        }
        unsafe {
            // `clearContents` returns the new change count (NSInteger); writing
            // without it first would be ignored by the pasteboard.
            let _ = msg!(pb, "clearContents", () -> isize);
        }
        let mut ok = true;
        for (text, ty) in [(&content.text, TYPE_STRING), (&content.html, TYPE_HTML)] {
            if let Some(text) = text {
                let (s, ty) = (nsstring(text), nsstring(ty));
                ok &= unsafe { msg!(pb, "setString:forType:", (s: Id, ty: Id) -> bool) };
            }
        }
        if let Some(png) = &png {
            let (bytes, len) = (png.as_ptr(), png.len());
            let ty = nsstring(TYPE_PNG);
            unsafe {
                let data = msg!(
                    class("NSData"),
                    "dataWithBytes:length:",
                    (bytes: *const u8, len: usize) -> Id
                );
                ok &= !data.is_null() && msg!(pb, "setData:forType:", (data: Id, ty: Id) -> bool);
            }
        }
        ok
    })
}
