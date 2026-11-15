//! Runtime permissions: whether the user lets the app use the camera, the
//! microphone, notifications, the photo library or the location, asking
//! them, and sending them to the app's page in the system settings once
//! they've said no.
//!
//! ```no_run
//! use stipple::platform::permission::{self, Permission, PermissionStatus};
//!
//! match permission::status(Permission::Camera) {
//!     PermissionStatus::Granted => { /* open the camera */ }
//!     PermissionStatus::NotDetermined => permission::request(Permission::Camera, |status| {
//!         println!("the user said {status:?}");
//!     }),
//!     // Asking again does nothing: only the settings can change it now.
//!     _ => { let _ = permission::open_settings(); }
//! }
//! ```
//!
//! From a view, [`AppHandle::request_permission`](crate::core::AppHandle::request_permission)
//! asks and applies the answer to the state on the UI thread.
//!
//! | | Camera, microphone | Notifications | Photos | Location | Settings page |
//! |---|---|---|---|---|---|
//! | iOS | `AVCaptureDevice` `authorizationStatusForMediaType:` / `requestAccessForMediaType:` | `UNUserNotificationCenter` | `PHPhotoLibrary` (read and write) | `CLLocationManager`, while in use | `UIApplicationOpenSettingsURLString` |
//! | Android | `checkSelfPermission` / `requestPermissions` (`CAMERA`, `RECORD_AUDIO`) | `POST_NOTIFICATIONS` (13+; before, whether they're on) | `READ_MEDIA_IMAGES` (13+), else `READ_EXTERNAL_STORAGE` | `ACCESS_FINE_LOCATION` or `ACCESS_COARSE_LOCATION` | `ACTION_APPLICATION_DETAILS_SETTINGS` |
//! | Web | the Permissions API; asking opens and closes a `getUserMedia` stream | `Notification` | always granted (files come through the picker) | the Permissions API; asking reads the position once | none |
//! | macOS | `AVCaptureDevice`, as on iOS | granted | granted | granted | System Settings' Privacy pane |
//! | Windows, Linux | granted | granted | granted | granted | Windows' privacy settings; none on Linux |
//!
//! Where the platform gates nothing, the answer is
//! [`Granted`](crate::platform::permission::PermissionStatus::Granted): whether there's a camera at all is
//! [`Camera`](crate::camera::Camera)'s business, not the permission's.
//!
//! Each platform also needs the app to say why it asks, or the request fails
//! (or, on iOS and macOS, the process is killed): `NSCameraUsageDescription`,
//! `NSMicrophoneUsageDescription`, `NSPhotoLibraryUsageDescription` and
//! `NSLocationWhenInUseUsageDescription` in the `Info.plist`, and a
//! `<uses-permission>` in the Android manifest. See `docs/os-integration.md`.

use crate::platform::PlatformError;

#[cfg(any(target_os = "ios", target_os = "macos"))]
#[allow(unsafe_code)]
pub(crate) mod apple;

/// Something the app needs the user's permission for.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Permission {
    /// The camera ([`stipple::camera`](crate::camera)).
    Camera,
    /// The microphone.
    Microphone,
    /// Showing notifications ([`notify`](crate::platform::notify),
    /// [`schedule_notification`](crate::platform::schedule_notification)).
    Notifications,
    /// Reading (and adding to) the photo library.
    Photos,
    /// The device's location, while the app is in use.
    Location,
}

/// Where a [`Permission`] stands.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PermissionStatus {
    /// The app may use it (also where the platform asks nothing).
    Granted,
    /// The user said no. Asking again shows nothing on iOS, and on Android
    /// after the second refusal: send them to the settings with
    /// [`open_settings`].
    Denied,
    /// Not asked yet: [`request`] shows the system's prompt.
    NotDetermined,
    /// Not the user's to give: parental controls, a device policy. Neither
    /// asking nor the settings can change it.
    Restricted,
}

impl PermissionStatus {
    /// Whether the app may use it.
    pub fn is_granted(self) -> bool {
        self == Self::Granted
    }
}

/// Where `permission` stands now, without asking. Cheap; call it from the UI
/// thread.
///
/// On the web, where the Permissions API answers asynchronously, this is
/// what the page last heard (it asks once at start and follows changes), so
/// right after the page loads it may still be
/// [`NotDetermined`](PermissionStatus::NotDetermined).
pub fn status(permission: Permission) -> PermissionStatus {
    os::status(permission)
}

/// Ask the user for `permission` (the system's prompt), and tell `done`
/// where it stands then. If it was already decided, nothing is shown and
/// `done` hears the answer at once.
///
/// Call it from the UI thread. `done` runs on whichever thread the system
/// answers on (sometimes before this returns); change the state from it
/// with [`AppHandle::update`](crate::core::AppHandle::update), or use
/// [`AppHandle::request_permission`](crate::core::AppHandle::request_permission),
/// which does.
pub fn request(permission: Permission, done: impl FnOnce(PermissionStatus) + Send + 'static) {
    os::request(permission, Box::new(done));
}

/// Open the app's page in the system settings (iOS, Android), or the
/// privacy settings (macOS, Windows), where the user can change what they
/// refused. `Err` where there's no such page (Linux, the web).
pub fn open_settings() -> Result<(), PlatformError> {
    os::open_settings()
}

/// What [`request`] hands the platform.
pub(crate) type Done = Box<dyn FnOnce(PermissionStatus) + Send>;

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod os {
    pub(super) use super::apple::{open_settings, request, status};
}

#[cfg(target_os = "android")]
mod os {
    pub(super) use crate::platform::backend::android::permission::{
        open_settings, request, status,
    };
}

#[cfg(target_arch = "wasm32")]
mod os {
    use super::{Done, Permission, PermissionStatus};
    use crate::platform::PlatformError;
    use crate::platform::backend::web;

    pub(super) fn status(permission: Permission) -> PermissionStatus {
        match web::permission_kind(permission) {
            Some(kind) => web::permission_status(kind),
            None => PermissionStatus::Granted,
        }
    }

    pub(super) fn request(permission: Permission, done: Done) {
        match web::permission_kind(permission) {
            Some(kind) => web::request_permission(kind, done),
            None => done(PermissionStatus::Granted),
        }
    }

    pub(super) fn open_settings() -> Result<(), PlatformError> {
        Err(PlatformError::NoBackend("a settings page"))
    }
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "android",
    target_arch = "wasm32"
)))]
mod os {
    use super::{Done, Permission, PermissionStatus};
    use crate::platform::PlatformError;

    pub(super) fn status(_permission: Permission) -> PermissionStatus {
        PermissionStatus::Granted
    }

    pub(super) fn request(permission: Permission, done: Done) {
        done(status(permission));
    }

    #[cfg(target_os = "windows")]
    pub(super) fn open_settings() -> Result<(), PlatformError> {
        crate::platform::open_url("ms-settings:privacy")
    }

    #[cfg(not(target_os = "windows"))]
    pub(super) fn open_settings() -> Result<(), PlatformError> {
        Err(PlatformError::NoBackend("a settings page"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktops_gate_nothing_but_the_camera_on_macos() {
        for p in [
            Permission::Notifications,
            Permission::Photos,
            Permission::Location,
        ] {
            assert_eq!(status(p), PermissionStatus::Granted, "{p:?}");
            let (tx, rx) = std::sync::mpsc::channel();
            request(p, move |s| tx.send(s).unwrap());
            assert_eq!(rx.recv().unwrap(), PermissionStatus::Granted);
        }
    }
}
