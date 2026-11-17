//! URLs that open the app on macOS, for
//! [`App::on_open_url`](crate::App::on_open_url): the custom schemes the
//! bundle's `Info.plist` claims (`CFBundleURLTypes`), which Launch Services
//! sends as a `kAEGetURL` Apple event. AppKit hands its URLs to the
//! application delegate's `application:openURLs:` (macOS 10.13), at launch
//! (once `finishLaunching` ran) and while the app runs. File URLs (a
//! document dropped on the Dock icon) are left out: they name files, not
//! links.

use std::ffi::c_void;

use super::{Class, Id, Sel, class_addMethod, msg_bool, msg_id, nsstring_to_string, sel};

/// `application:openURLs:`: an `NSArray` of `NSURL`s.
extern "C" fn open_urls(_this: Id, _cmd: Sel, _app: Id, urls: Id) {
    if urls.is_null() {
        return;
    }
    unsafe {
        let count: unsafe extern "C" fn(Id, Sel) -> usize =
            std::mem::transmute(super::objc_msgSend as *const c_void);
        let at: unsafe extern "C" fn(Id, Sel, usize) -> Id =
            std::mem::transmute(super::objc_msgSend as *const c_void);
        for i in 0..count(urls, sel("count")) {
            let url = at(urls, sel("objectAtIndex:"), i);
            if url.is_null() || msg_bool(url, sel("isFileURL")) {
                continue;
            }
            let url = nsstring_to_string(msg_id(url, sel("absoluteString")));
            crate::platform::incoming_url::deliver(url);
        }
    }
}

/// Give the application delegate `application:openURLs:`.
///
/// # Safety
/// `cls` must be a class pair still being built.
pub(super) unsafe fn add_app_delegate_methods(cls: Class) {
    unsafe {
        class_addMethod(
            cls,
            sel("application:openURLs:"),
            open_urls as *const c_void,
            c"v@:@@".as_ptr(),
        );
    }
}
