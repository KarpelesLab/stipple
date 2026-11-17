//! Secrets kept by the OS: [`secure_store`] stores small byte values (a
//! token, a private key) where the platform keeps passwords, not in a plain
//! file.
//!
//! | | Where | Encrypted by | Backed up |
//! |---|---|---|---|
//! | iOS | the Keychain, generic passwords | the Secure Enclave's data protection, available after the first unlock | no (`ThisDeviceOnly`) |
//! | macOS | the data protection keychain (signed apps), else the login keychain | the keychain | no for the data protection keychain |
//! | Android | files in `getNoBackupFilesDir()`, AES-256-GCM with a key in the `AndroidKeyStore` (StrongBox when there is one) | the key, which never leaves the Keystore | no |
//! | Windows | Credential Manager, generic credentials, `CRED_PERSIST_LOCAL_MACHINE` | DPAPI, for the user | doesn't roam |
//! | Linux | the Secret Service (GNOME Keyring, KeePassXC, KWallet) over D-Bus | the keyring | the keyring's |
//! | Web | — ([`SecureStoreError::Unsupported`]) | | |
//!
//! Items are named by the app's [service](SecureStore::service) (its
//! [`app_info`](crate::platform::app_info) id by default) and a key. The
//! calls block, and on Linux a locked keyring may ask the user for its
//! password first: call them away from the UI thread when that matters.
//!
//! See `docs/secure-storage.md`.

use std::fmt;

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
mod apple;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

/// The Android backend's file format (the store itself is in the Android
/// backend, which holds the JNI).
#[cfg(any(target_os = "android", test))]
pub(crate) mod sealed;

/// Why a [`SecureStore`] call failed.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecureStoreError {
    /// The platform has no secure storage Stipple can use (the web).
    Unsupported,
    /// The platform has one, but it isn't there now: no Secret Service on
    /// the session bus (Linux), Android before 6.0 or before there is an
    /// application context, no user profile loaded (Windows services).
    /// Never answered with a plain file instead.
    Unavailable(String),
    /// The store is locked and wasn't unlocked: the device hasn't been
    /// unlocked since it started (iOS), or the keyring's unlock prompt was
    /// dismissed (Linux).
    Locked,
    /// The OS refused: the app lacks an entitlement (an access group not in
    /// `keychain-access-groups`), or the user denied the app access.
    Denied(String),
    /// The key is empty, longer than [`SecureStore::MAX_KEY_LEN`] bytes, or
    /// holds a control character.
    InvalidKey,
    /// The value is longer than [`SecureStore::MAX_VALUE_LEN`] bytes.
    TooLarge,
    /// A value is stored but can't be read back: its Android Keystore key
    /// is gone (the app's data was restored to another device, or the
    /// Keystore was reset) or the file was changed. Remove it and set it
    /// again.
    Corrupt,
    /// Any other failure, as the OS described it.
    Os(String),
}

impl fmt::Display for SecureStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "no secure storage on this platform"),
            Self::Unavailable(why) => write!(f, "secure storage unavailable: {why}"),
            Self::Locked => write!(f, "the secure store is locked"),
            Self::Denied(why) => write!(f, "access to the secure store denied: {why}"),
            Self::InvalidKey => write!(
                f,
                "invalid secure store key (empty, over {} bytes, or a control character)",
                SecureStore::MAX_KEY_LEN
            ),
            Self::TooLarge => write!(
                f,
                "secure store value over {} bytes",
                SecureStore::MAX_VALUE_LEN
            ),
            Self::Corrupt => write!(f, "the stored value can't be decrypted"),
            Self::Os(why) => write!(f, "secure store error: {why}"),
        }
    }
}

impl std::error::Error for SecureStoreError {}

/// The OS's store for the app's secrets: get, set and remove small byte
/// values by key. Made by [`secure_store`]; the setters choose where items
/// go, which matters only when something else must read them too (an app
/// extension).
///
/// ```no_run
/// use stipple::platform::secure_store;
/// let store = secure_store();
/// store.set("account-token", b"s3cr3t")?;
/// assert_eq!(store.get("account-token")?.as_deref(), Some(&b"s3cr3t"[..]));
/// store.remove("account-token")?;
/// # Ok::<(), stipple::platform::SecureStoreError>(())
/// ```
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecureStore {
    /// The namespace items are kept under: the Keychain's `kSecAttrService`,
    /// the Secret Service's `service` attribute, the start of a Windows
    /// credential's target name, the Android files' directory. `None` is the
    /// app's [`app_info`](crate::platform::app_info) id: the bundle id, the
    /// package name, or what [`set_app_info`](crate::platform::set_app_info)
    /// gave (the executable's name without either).
    pub service: Option<String>,
    /// The keychain access group (iOS, macOS): items shared with an app
    /// extension or another app of the same team. The group must be in the
    /// app's `keychain-access-groups` entitlement (`TEAMID.com.example.shared`),
    /// or be one of its app groups (`group.com.example`). Ignored elsewhere.
    pub access_group: Option<String>,
}

impl SecureStore {
    /// The longest key, in bytes of UTF-8.
    pub const MAX_KEY_LEN: usize = 120;
    /// The longest value, in bytes: Windows Credential Manager's limit for a
    /// credential's blob, the tightest of the backends, kept everywhere so
    /// that what works on one platform works on all.
    pub const MAX_VALUE_LEN: usize = 2560;

    /// The app's store, under its own id.
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep items under `service` instead of the app's id: an app
    /// extension, whose bundle id differs from the app's, gives the app's
    /// to share its items (with an [access group](Self::access_group)).
    pub fn service(mut self, service: impl Into<String>) -> Self {
        self.service = Some(service.into());
        self
    }

    /// Keep items in the keychain access group `group` (iOS, macOS).
    pub fn access_group(mut self, group: impl Into<String>) -> Self {
        self.access_group = Some(group.into());
        self
    }

    /// The value stored under `key`, or `None` if there is none.
    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStoreError> {
        check_key(key)?;
        os::get(&self.target(), key)
    }

    /// Store `value` under `key`, replacing what was there.
    pub fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStoreError> {
        check_key(key)?;
        if value.len() > Self::MAX_VALUE_LEN {
            return Err(SecureStoreError::TooLarge);
        }
        os::set(&self.target(), key, value)
    }

    /// Remove the value under `key`. Removing a key that holds nothing
    /// succeeds.
    pub fn remove(&self, key: &str) -> Result<(), SecureStoreError> {
        check_key(key)?;
        os::remove(&self.target(), key)
    }

    /// Where items go, the service resolved.
    fn target(&self) -> Target {
        let service = self
            .service
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| crate::platform::app_info().id);
        Target {
            service,
            access_group: self.access_group.clone().filter(|g| !g.is_empty()),
        }
    }
}

/// The app's secure store: [`SecureStore::new`].
///
/// ```no_run
/// let token = stipple::platform::secure_store().get("account-token")?;
/// # Ok::<(), stipple::platform::SecureStoreError>(())
/// ```
pub fn secure_store() -> SecureStore {
    SecureStore::new()
}

/// A [`SecureStore`]'s resolved place.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
pub(crate) struct Target {
    pub(crate) service: String,
    pub(crate) access_group: Option<String>,
}

/// A key is 1 to [`SecureStore::MAX_KEY_LEN`] bytes with no control
/// characters (it names files and shows in the OS's password managers).
fn check_key(key: &str) -> Result<(), SecureStoreError> {
    if key.is_empty() || key.len() > SecureStore::MAX_KEY_LEN || key.chars().any(char::is_control) {
        return Err(SecureStoreError::InvalidKey);
    }
    Ok(())
}

/// A Windows credential's target name: `<service>/<key>`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn credential_name(service: &str, key: &str) -> String {
    format!("{service}/{key}")
}

/// A line to stderr under `STIPPLE_DEBUG_SECURE_STORE=1`: which store a
/// backend chose, and why.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
fn debug(msg: &str) {
    if std::env::var_os("STIPPLE_DEBUG_SECURE_STORE").is_some_and(|v| v == "1") {
        eprintln!("stipple: secure store: {msg}");
    }
}

// ---- Per-platform dispatch ---------------------------------------------------

#[cfg(any(target_os = "macos", target_os = "ios"))]
use apple as os;

#[cfg(target_os = "windows")]
use windows as os;

#[cfg(target_os = "linux")]
use linux as os;

#[cfg(target_os = "android")]
mod os {
    pub(super) use crate::platform::backend::android::secure_store::{get, remove, set};
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "linux",
    target_os = "android"
)))]
mod os {
    use super::{SecureStoreError, Target};

    pub(super) fn get(_: &Target, _: &str) -> Result<Option<Vec<u8>>, SecureStoreError> {
        Err(SecureStoreError::Unsupported)
    }

    pub(super) fn set(_: &Target, _: &str, _: &[u8]) -> Result<(), SecureStoreError> {
        Err(SecureStoreError::Unsupported)
    }

    pub(super) fn remove(_: &Target, _: &str) -> Result<(), SecureStoreError> {
        Err(SecureStoreError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_checked_before_the_os_is_asked() {
        let store = secure_store().service("org.stipple.test");
        for bad in [
            "",
            "a\nb",
            "tab\there",
            &"k".repeat(SecureStore::MAX_KEY_LEN + 1),
        ] {
            assert_eq!(store.get(bad), Err(SecureStoreError::InvalidKey), "{bad:?}");
            assert_eq!(store.set(bad, b"x"), Err(SecureStoreError::InvalidKey));
            assert_eq!(store.remove(bad), Err(SecureStoreError::InvalidKey));
        }
        assert!(check_key(&"k".repeat(SecureStore::MAX_KEY_LEN)).is_ok());
        assert!(check_key("wireguard/private-key é").is_ok());
        let big = vec![0u8; SecureStore::MAX_VALUE_LEN + 1];
        assert_eq!(store.set("k", &big), Err(SecureStoreError::TooLarge));
    }

    #[test]
    fn the_service_defaults_to_the_app_id() {
        let t = SecureStore::new().target();
        assert_eq!(t.service, crate::platform::app_info().id);
        assert_eq!(t.access_group, None);
        let t = SecureStore::new()
            .service("org.example.App")
            .access_group("TEAMID.org.example.shared")
            .target();
        assert_eq!(t.service, "org.example.App");
        assert_eq!(t.access_group.as_deref(), Some("TEAMID.org.example.shared"));
        // Empty strings mean "the default".
        let t = SecureStore::new().service("").access_group("").target();
        assert_eq!(t.service, crate::platform::app_info().id);
        assert_eq!(t.access_group, None);
    }

    #[test]
    fn windows_credentials_are_named_service_slash_key() {
        assert_eq!(
            credential_name("org.example.Vpn", "wireguard-key"),
            "org.example.Vpn/wireguard-key"
        );
    }

    #[test]
    fn errors_say_what_failed() {
        assert!(SecureStoreError::TooLarge.to_string().contains("2560"));
        assert!(
            SecureStoreError::Unavailable("no Secret Service".into())
                .to_string()
                .contains("no Secret Service")
        );
    }
}
