//! App extensions and app groups (iOS, and macOS where it applies).
//!
//! An app extension (a widget, a share extension, a notification service, a
//! Network Extension provider) is a separate executable in the app's
//! `PlugIns/<Name>.appex`, started by the system, not the user, and handed
//! to Foundation's `NSExtensionMain`, which looks up the class the bundle's
//! `Info.plist` names (`NSExtension.NSExtensionPrincipalClass`) and serves
//! the host's requests. [`extension_main`] is that call, for a Rust `main`
//! that first registers its principal class with the Objective-C runtime.
//! `cargo-stipple ios` builds and signs the bundle (see
//! `docs/packaging.md#app-extensions`).
//!
//! The app and its extensions share files through an app group's container,
//! [`shared_dir`]. Inside an extension, [`dirs`](super::dirs) are the
//! extension's own container, not the app's.

use crate::platform::PlatformError;
use std::path::PathBuf;

/// The shared container of the app group `group` (`group.com.example.app`):
/// a directory the app and its extensions (and the team's other apps in the
/// group) all read and write. `None` when the process isn't entitled to the
/// group (`com.apple.security.application-groups`; `cargo-stipple` adds it
/// from `ios.app-groups`), and on platforms without app groups.
///
/// On iOS it is
/// `NSFileManager.containerURLForSecurityApplicationGroupIdentifier:`, which
/// the system creates when the app is installed. On macOS the same call
/// gives `~/Library/Group Containers/<group>`; a sandboxed app needs the
/// entitlement, and the directory is created here if it isn't there yet.
///
/// ```no_run
/// if let Some(dir) = stipple::platform::shared_dir("group.com.example.vpn") {
///     std::fs::write(dir.join("config.json"), b"{}")?;
/// }
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn shared_dir(group: &str) -> Option<PathBuf> {
    if group.is_empty() || group.contains('\0') {
        return None;
    }
    os::shared_dir(group)
}

/// Hand the process to Foundation's `NSExtensionMain`, as an app extension's
/// `main` does once it has registered its principal class (the class named
/// by `NSExtensionPrincipalClass`, made at run time with
/// `objc_allocateClassPair`; see `examples/extensiondemo/extension.rs`).
/// It serves the host until the system ends the process, so it only returns
/// an error: on platforms without app extensions, or when Foundation has no
/// `NSExtensionMain`.
///
/// An extension needs no [`App`](crate::App): the non-UI parts of Stipple
/// ([`dirs`](super::dirs), [`app_info`](super::app_info) (the extension's
/// own bundle), [`shared_dir`], [`preferred_languages`](super::preferred_languages)
/// and [`locale`](super::locale), files and threads) work without
/// `App::run`. `cargo-stipple` can also link the executable with
/// `NSExtensionMain` as its entry point instead (`entry = "NSExtensionMain"`),
/// for an extension whose principal class comes from another language.
///
/// ```no_run
/// // In the extension's `main`, after registering its principal class:
/// let Err(e) = stipple::platform::extension_main();
/// eprintln!("{e}");
/// std::process::exit(1);
/// ```
pub fn extension_main() -> Result<std::convert::Infallible, PlatformError> {
    os::extension_main()
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
#[allow(unsafe_code)]
mod os {
    use crate::platform::PlatformError;
    use crate::platform::native::objc::{
        Id, class, load_framework, msg, nsstring, rust_string, symbol, with_pool,
    };
    use std::ffi::{c_char, c_int};
    use std::path::PathBuf;

    unsafe extern "C" {
        /// libSystem: the process's arguments, as `main` received them.
        fn _NSGetArgc() -> *mut c_int;
        fn _NSGetArgv() -> *mut *mut *mut c_char;
    }

    pub(super) fn shared_dir(group: &str) -> Option<PathBuf> {
        let dir = with_pool(|| unsafe {
            let fm = msg!(class("NSFileManager"), "defaultManager", () -> Id);
            if fm.is_null() {
                return None;
            }
            let g = nsstring(group);
            let url = msg!(
                fm,
                "containerURLForSecurityApplicationGroupIdentifier:",
                (g: Id) -> Id
            );
            if url.is_null() {
                return None;
            }
            rust_string(msg!(url, "path", () -> Id))
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
        })?;
        // iOS made it at install time; macOS may hand back a path to make.
        #[cfg(target_os = "macos")]
        if !dir.is_dir() {
            std::fs::create_dir_all(&dir).ok()?;
        }
        Some(dir)
    }

    pub(super) fn extension_main() -> Result<std::convert::Infallible, PlatformError> {
        let foundation = load_framework("Foundation");
        let f = symbol(foundation, c"NSExtensionMain");
        if f.is_null() {
            return Err(PlatformError::NoBackend("NSExtensionMain (Foundation)"));
        }
        let f: unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int =
            unsafe { std::mem::transmute(f) };
        let code = unsafe { f(*_NSGetArgc(), *_NSGetArgv()) };
        // It doesn't return in an extension the system started; started any
        // other way, it may.
        std::process::exit(code)
    }
}

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
mod os {
    use crate::platform::PlatformError;
    use std::path::PathBuf;

    pub(super) fn shared_dir(_group: &str) -> Option<PathBuf> {
        None
    }

    pub(super) fn extension_main() -> Result<std::convert::Infallible, PlatformError> {
        Err(PlatformError::NoBackend(
            "app extensions (iOS and macOS only)",
        ))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn no_group_no_directory() {
        assert_eq!(super::shared_dir(""), None);
        assert_eq!(super::shared_dir("group.a\0b"), None);
    }

    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    #[test]
    fn elsewhere_there_are_no_extensions() {
        assert_eq!(super::shared_dir("group.com.example"), None);
        assert!(super::extension_main().is_err());
    }
}
