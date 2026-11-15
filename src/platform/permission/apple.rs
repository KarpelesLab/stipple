//! Permissions on iOS and macOS: `AVCaptureDevice` for the camera and the
//! microphone (both), and on iOS `UNUserNotificationCenter`, `PHPhotoLibrary`
//! and `CLLocationManager`. The frameworks are loaded when first asked
//! (`dlopen`), so an app that never asks doesn't link them. The completion
//! handlers are blocks built by hand ([`once_block`]).

use super::{Done, Permission, PermissionStatus};
use crate::platform::PlatformError;
use crate::platform::native::objc::{
    Id, class, load_framework, msg, nsstring, once_block, with_pool,
};

/// `AVMediaTypeVideo` / `AVMediaTypeAudio`: the constants' values, which the
/// framework compares as strings.
const VIDEO: &str = "vide";
const AUDIO: &str = "soun";

/// `AVCaptureDevice`, loading AVFoundation first; null if missing.
pub(crate) fn capture_device_class() -> Id {
    load_framework("AVFoundation");
    class("AVCaptureDevice")
}

/// An `AVAuthorizationStatus` (or `PHAuthorizationStatus`, numbered alike:
/// 0 not determined, 1 restricted, 2 denied, 3 authorized, 4 limited).
fn av_status(status: isize) -> PermissionStatus {
    match status {
        0 => PermissionStatus::NotDetermined,
        1 => PermissionStatus::Restricted,
        2 => PermissionStatus::Denied,
        _ => PermissionStatus::Granted,
    }
}

fn media_status(media: &str) -> PermissionStatus {
    with_pool(|| {
        let cls = capture_device_class();
        if cls.is_null() {
            return PermissionStatus::Restricted;
        }
        let media = nsstring(media);
        av_status(unsafe { msg!(cls, "authorizationStatusForMediaType:", (media: Id) -> isize) })
    })
}

fn media_request(media: &'static str, done: Done) {
    if media_status(media) != PermissionStatus::NotDetermined {
        done(media_status(media));
        return;
    }
    with_pool(|| {
        let cls = capture_device_class();
        let block = once_block(move |_granted: u8| done(media_status(media)));
        let media = nsstring(media);
        unsafe {
            msg!(cls, "requestAccessForMediaType:completionHandler:", (media: Id, block: Id) -> ())
        };
    });
}

pub(crate) fn status(permission: Permission) -> PermissionStatus {
    match permission {
        Permission::Camera => media_status(VIDEO),
        Permission::Microphone => media_status(AUDIO),
        #[cfg(target_os = "ios")]
        Permission::Notifications => ios::notifications(),
        #[cfg(target_os = "ios")]
        Permission::Photos => ios::photos(),
        #[cfg(target_os = "ios")]
        Permission::Location => ios::location(),
        #[cfg(not(target_os = "ios"))]
        _ => PermissionStatus::Granted,
    }
}

pub(crate) fn request(permission: Permission, done: Done) {
    match permission {
        Permission::Camera => media_request(VIDEO, done),
        Permission::Microphone => media_request(AUDIO, done),
        #[cfg(target_os = "ios")]
        Permission::Notifications => {
            crate::platform::shell::ios::request_permission(Box::new(move |_| {
                done(ios::notifications())
            }))
        }
        #[cfg(target_os = "ios")]
        Permission::Photos => ios::request_photos(done),
        #[cfg(target_os = "ios")]
        Permission::Location => ios::request_location(done),
        #[cfg(not(target_os = "ios"))]
        _ => done(PermissionStatus::Granted),
    }
}

#[cfg(target_os = "ios")]
pub(crate) fn open_settings() -> Result<(), PlatformError> {
    // The value of `UIApplicationOpenSettingsURLString`.
    crate::platform::open_url("app-settings:")
}

#[cfg(target_os = "macos")]
pub(crate) fn open_settings() -> Result<(), PlatformError> {
    crate::platform::open_url("x-apple.systempreferences:com.apple.preference.security?Privacy")
}

#[cfg(target_os = "ios")]
mod ios {
    use super::{Done, PermissionStatus, av_status};
    use crate::platform::native::objc::{
        Id, Sel, class, load_framework, msg, once_block, sel, with_pool,
    };
    use std::ffi::c_void;
    use std::sync::Mutex;

    /// `UNAuthorizationStatus`: 0 not determined, 1 denied, 2 authorized,
    /// 3 provisional, 4 ephemeral.
    pub(super) fn notifications() -> PermissionStatus {
        match crate::platform::shell::ios::notification_authorization() {
            Some(0) | None => PermissionStatus::NotDetermined,
            Some(1) => PermissionStatus::Denied,
            Some(_) => PermissionStatus::Granted,
        }
    }

    /// `PHAccessLevelReadWrite`.
    const READ_WRITE: isize = 2;

    fn photo_library() -> Id {
        load_framework("Photos");
        class("PHPhotoLibrary")
    }

    pub(super) fn photos() -> PermissionStatus {
        with_pool(|| {
            let cls = photo_library();
            if cls.is_null() {
                return PermissionStatus::Restricted;
            }
            av_status(unsafe {
                msg!(cls, "authorizationStatusForAccessLevel:", (READ_WRITE: isize) -> isize)
            })
        })
    }

    pub(super) fn request_photos(done: Done) {
        if photos() != PermissionStatus::NotDetermined {
            done(photos());
            return;
        }
        with_pool(|| {
            let cls = photo_library();
            let block = once_block(move |status: isize| done(av_status(status)));
            unsafe {
                msg!(cls, "requestAuthorizationForAccessLevel:handler:",
                    (READ_WRITE: isize, block: Id) -> ())
            };
        });
    }

    fn location_manager_class() -> Id {
        load_framework("CoreLocation");
        class("CLLocationManager")
    }

    /// A `CLAuthorizationStatus`: 0 not determined, 1 restricted, 2 denied,
    /// 3 always, 4 when in use.
    fn location_status(status: i32) -> PermissionStatus {
        match status {
            0 => PermissionStatus::NotDetermined,
            1 => PermissionStatus::Restricted,
            2 => PermissionStatus::Denied,
            _ => PermissionStatus::Granted,
        }
    }

    pub(super) fn location() -> PermissionStatus {
        with_pool(|| {
            let cls = location_manager_class();
            if cls.is_null() {
                return PermissionStatus::Restricted;
            }
            location_status(unsafe { msg!(cls, "authorizationStatus", () -> i32) })
        })
    }

    /// Those waiting for the location prompt's answer.
    static WAITING: Mutex<Vec<Done>> = Mutex::new(Vec::new());
    /// The manager asking (kept, as its delegate hears the answer), as an
    /// address.
    static MANAGER: Mutex<usize> = Mutex::new(0);

    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_allocateClassPair(superclass: Id, name: *const i8, extra: usize) -> Id;
        fn objc_registerClassPair(cls: Id);
        fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const i8) -> bool;
    }

    /// `locationManagerDidChangeAuthorization:` (iOS 14), also called when
    /// the manager is made: once decided, answer whoever waits.
    extern "C" fn changed(_this: Id, _cmd: Sel, manager: Id) {
        let status = location_status(unsafe { msg!(manager, "authorizationStatus", () -> i32) });
        if status == PermissionStatus::NotDetermined {
            return;
        }
        let waiting = std::mem::take(&mut *WAITING.lock().unwrap_or_else(|e| e.into_inner()));
        for done in waiting {
            done(status);
        }
    }

    /// `locationManager:didChangeAuthorizationStatus:`, before iOS 14.
    extern "C" fn changed_status(this: Id, cmd: Sel, manager: Id, _status: i32) {
        changed(this, cmd, manager);
    }

    fn delegate() -> Id {
        static DELEGATE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *DELEGATE.get_or_init(|| unsafe {
            let cls =
                objc_allocateClassPair(class("NSObject"), c"StippleLocationDelegate".as_ptr(), 0);
            if cls.is_null() {
                return 0;
            }
            class_addMethod(
                cls,
                sel("locationManagerDidChangeAuthorization:"),
                changed as *const c_void,
                c"v@:@".as_ptr(),
            );
            class_addMethod(
                cls,
                sel("locationManager:didChangeAuthorizationStatus:"),
                changed_status as *const c_void,
                c"v@:@i".as_ptr(),
            );
            objc_registerClassPair(cls);
            msg!(msg!(cls, "alloc", () -> Id), "init", () -> Id) as usize
        }) as Id
    }

    /// Ask for the location while in use. From the main thread, whose run
    /// loop delivers the delegate's calls.
    pub(super) fn request_location(done: Done) {
        if location() != PermissionStatus::NotDetermined {
            done(location());
            return;
        }
        WAITING.lock().unwrap_or_else(|e| e.into_inner()).push(done);
        with_pool(|| unsafe {
            let mut kept = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
            if *kept == 0 {
                let cls = location_manager_class();
                let manager = msg!(msg!(cls, "alloc", () -> Id), "init", () -> Id);
                let delegate = delegate();
                msg!(manager, "setDelegate:", (delegate: Id) -> ());
                *kept = manager as usize;
            }
            let manager = *kept as Id;
            msg!(manager, "requestWhenInUseAuthorization", () -> ());
        });
    }
}
