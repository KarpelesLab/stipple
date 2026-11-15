//! macOS and iOS: the main bundle's `Info.plist` through `NSBundle`, the
//! directories through `NSFileManager`.
//!
//! A binary started outside an app bundle (`cargo run`) has a main bundle
//! too, the executable's directory, but no `Info.plist`: no identifier or
//! version, which then come from [`set_app_info`](super::set_app_info).

use super::{AppDirs, OsInfo};
use crate::platform::PlatformError;
use crate::platform::native::objc::{Id, class, msg, nsstring, rust_string, with_pool};
use std::path::PathBuf;

const NS_CACHES_DIRECTORY: usize = 13;
const NS_APPLICATION_SUPPORT_DIRECTORY: usize = 14;
const NS_USER_DOMAIN_MASK: usize = 1;

/// The main bundle's `Info.plist` string for `key`.
fn info_string(bundle: Id, key: &str) -> Option<String> {
    let k = nsstring(key);
    let v = unsafe { msg!(bundle, "objectForInfoDictionaryKey:", (k: Id) -> Id) };
    if v.is_null() {
        return None;
    }
    let ns_string = class("NSString");
    let is_string = unsafe { msg!(v, "isKindOfClass:", (ns_string: Id) -> bool) };
    if !is_string {
        return None;
    }
    rust_string(v).filter(|s| !s.is_empty())
}

pub(super) fn info() -> OsInfo {
    with_pool(|| {
        let bundle = unsafe { msg!(class("NSBundle"), "mainBundle", () -> Id) };
        if bundle.is_null() {
            return OsInfo::default();
        }
        let id = rust_string(unsafe { msg!(bundle, "bundleIdentifier", () -> Id) })
            .filter(|s| !s.is_empty());
        OsInfo {
            id,
            name: info_string(bundle, "CFBundleDisplayName")
                .or_else(|| info_string(bundle, "CFBundleName")),
            version: info_string(bundle, "CFBundleShortVersionString"),
            build: info_string(bundle, "CFBundleVersion"),
        }
    })
}

/// The path of `url` (an `NSURL`), if it is a file URL.
fn url_path(url: Id) -> Option<PathBuf> {
    if url.is_null() {
        return None;
    }
    rust_string(unsafe { msg!(url, "path", () -> Id) }).map(PathBuf::from)
}

/// The user's `directory` (an `NSSearchPathDirectory`).
fn user_dir(fm: Id, directory: usize) -> Option<PathBuf> {
    let mask = NS_USER_DOMAIN_MASK;
    let urls =
        unsafe { msg!(fm, "URLsForDirectory:inDomains:", (directory: usize, mask: usize) -> Id) };
    if urls.is_null() {
        return None;
    }
    url_path(unsafe { msg!(urls, "firstObject", () -> Id) })
}

pub(super) fn dirs() -> Result<AppDirs, PlatformError> {
    let (support, caches, temp) = with_pool(|| {
        let fm = unsafe { msg!(class("NSFileManager"), "defaultManager", () -> Id) };
        if fm.is_null() {
            return (None, None, None);
        }
        (
            user_dir(fm, NS_APPLICATION_SUPPORT_DIRECTORY),
            user_dir(fm, NS_CACHES_DIRECTORY),
            url_path(unsafe { msg!(fm, "temporaryDirectory", () -> Id) }),
        )
    });
    let missing = |what| PlatformError::Os(format!("NSFileManager has no {what} directory"));
    let support = support.ok_or_else(|| missing("Application Support"))?;
    let caches = caches.ok_or_else(|| missing("Caches"))?;
    let temp = temp.ok_or_else(|| missing("temporary"))?;
    // iOS: the app's own container. macOS: the user's, shared with every
    // app, so each gets a directory named after its bundle identifier.
    #[cfg(target_os = "macos")]
    let (support, caches, temp) = {
        let id = super::desktop_id();
        (support.join(&id), caches.join(&id), temp.join(&id))
    };
    Ok(AppDirs {
        config: support.clone(),
        data: support,
        cache: caches,
        temp,
    })
}
