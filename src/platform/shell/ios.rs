//! iOS: `UIApplication openURL:options:completionHandler:`.

use crate::platform::PlatformError;
use crate::platform::native::objc::{Id, class, msg, nsstring, with_pool};

pub fn open_url(url: &str) -> Result<(), PlatformError> {
    with_pool(|| {
        let s = nsstring(url);
        let nsurl = unsafe { msg!(class("NSURL"), "URLWithString:", (s: Id) -> Id) };
        if nsurl.is_null() {
            return Err(PlatformError::Os(format!("NSURL refused {url:?}")));
        }
        let app = unsafe { msg!(class("UIApplication"), "sharedApplication", () -> Id) };
        if app.is_null() {
            return Err(PlatformError::Os("no UIApplication".into()));
        }
        let options = unsafe { msg!(class("NSDictionary"), "dictionary", () -> Id) };
        let none: Id = std::ptr::null_mut();
        // Opening happens asynchronously; the (nil) completion handler would
        // say whether it worked.
        unsafe {
            msg!(app, "openURL:options:completionHandler:", (nsurl: Id, options: Id, none: Id) -> ())
        };
        Ok(())
    })
}
