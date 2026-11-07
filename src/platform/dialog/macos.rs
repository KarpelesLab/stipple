//! macOS file dialogs: `NSOpenPanel` / `NSSavePanel`, run modally.

use super::{Op, PathBuf};
use crate::platform::native::objc::{Id, class, msg, nsstring, with_pool};
use std::ffi::{CStr, OsStr, c_char};
use std::os::unix::ffi::OsStrExt;

/// `NSModalResponseOK`.
const MODAL_RESPONSE_OK: isize = 1;

/// An `NSURL`'s path, from its `fileSystemRepresentation` (the exact bytes the
/// file system uses, rather than a display string).
unsafe fn url_path(url: Id) -> Option<PathBuf> {
    if url.is_null() {
        return None;
    }
    let p = unsafe { msg!(url, "fileSystemRepresentation", () -> *const c_char) };
    if p.is_null() {
        return None;
    }
    let bytes = unsafe { CStr::from_ptr(p) }.to_bytes();
    Some(PathBuf::from(OsStr::from_bytes(bytes)))
}

pub(super) fn file_chooser(op: Op, title: &str) -> Option<PathBuf> {
    with_pool(|| unsafe {
        // Panels need the shared application; this is a no-op once it exists.
        let _ = msg!(class("NSApplication"), "sharedApplication", () -> Id);
        let panel = match op {
            Op::Save => msg!(class("NSSavePanel"), "savePanel", () -> Id),
            Op::Open | Op::Folder => msg!(class("NSOpenPanel"), "openPanel", () -> Id),
        };
        if panel.is_null() {
            return None;
        }

        // Modern macOS doesn't draw panel titles, so show it as the message too.
        let text = nsstring(title);
        msg!(panel, "setTitle:", (text: Id) -> ());
        msg!(panel, "setMessage:", (text: Id) -> ());
        let yes = true;
        msg!(panel, "setCanCreateDirectories:", (yes: bool) -> ());
        if op != Op::Save {
            let files = op == Op::Open;
            let dirs = op == Op::Folder;
            let multiple = false;
            msg!(panel, "setCanChooseFiles:", (files: bool) -> ());
            msg!(panel, "setCanChooseDirectories:", (dirs: bool) -> ());
            msg!(panel, "setAllowsMultipleSelection:", (multiple: bool) -> ());
        }

        if msg!(panel, "runModal", () -> isize) != MODAL_RESPONSE_OK {
            return None;
        }
        let url = match op {
            // An open panel reports its selection through `URLs`.
            Op::Open | Op::Folder => {
                let urls = msg!(panel, "URLs", () -> Id);
                if urls.is_null() {
                    return None;
                }
                msg!(urls, "firstObject", () -> Id)
            }
            Op::Save => msg!(panel, "URL", () -> Id),
        };
        url_path(url)
    })
}
