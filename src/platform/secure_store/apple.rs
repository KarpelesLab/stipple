//! iOS and macOS: generic passwords in the Keychain (`SecItemAdd`,
//! `SecItemCopyMatching`, `SecItemUpdate`, `SecItemDelete`), the queries
//! built as CoreFoundation dictionaries by hand.
//!
//! An item is `kSecClassGenericPassword` with `kSecAttrService` the service
//! and `kSecAttrAccount` the key, readable after the first unlock since boot
//! and never moved to another device
//! (`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`), in the access group
//! when one is given.
//!
//! On macOS the items go to the data protection keychain
//! (`kSecUseDataProtectionKeychain`, the iOS-style one), which takes an app
//! signed with an application identifier. An unsigned or ad hoc signed
//! binary (`cargo run`) has none (its code signature carries no
//! `com.apple.application-identifier` or `keychain-access-groups`
//! entitlement, read through `SecTaskCopyValueForEntitlement`), so the store
//! uses the user's login keychain for it, without an accessibility class
//! (it has none) or access group: from the first call, so that a process
//! that only reads finds what another one stored (the data protection
//! keychain refuses an unentitled write with `errSecMissingEntitlement`,
//! but answers a read with `errSecItemNotFound`). A refused write still
//! switches the process to the login keychain. Items don't move between the
//! two.

use super::{SecureStoreError, Target};
use std::ffi::c_void;

type CFTypeRef = *const c_void;
type OSStatus = i32;

const ERR_SEC_SUCCESS: OSStatus = 0;
const ERR_SEC_USER_CANCELED: OSStatus = -128;
const ERR_SEC_NOT_AVAILABLE: OSStatus = -25291;
const ERR_SEC_AUTH_FAILED: OSStatus = -25293;
const ERR_SEC_NO_SUCH_KEYCHAIN: OSStatus = -25294;
const ERR_SEC_DUPLICATE_ITEM: OSStatus = -25299;
const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;
const ERR_SEC_INTERACTION_NOT_ALLOWED: OSStatus = -25308;
const ERR_SEC_MISSING_ENTITLEMENT: OSStatus = -34018;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

/// Opaque stand-in for the `CFDictionary{Key,Value}CallBacks` structs: only
/// the real globals' addresses are taken.
#[repr(C)]
struct CFCallbacks {
    _opaque: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        num_values: isize,
        key_callbacks: *const CFCallbacks,
        value_callbacks: *const CFCallbacks,
    ) -> CFTypeRef;
    fn CFStringCreateWithBytes(
        allocator: CFTypeRef,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        is_external: u8,
    ) -> CFTypeRef;
    // As the macOS backend declares it (one signature per symbol).
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *mut c_void;
    fn CFDataGetLength(data: CFTypeRef) -> isize;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFRelease(cf: CFTypeRef);
    static kCFBooleanTrue: CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: CFCallbacks;
    static kCFTypeDictionaryValueCallBacks: CFCallbacks;
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecItemAdd(attributes: CFTypeRef, result: *mut CFTypeRef) -> OSStatus;
    fn SecItemCopyMatching(query: CFTypeRef, result: *mut CFTypeRef) -> OSStatus;
    fn SecItemUpdate(query: CFTypeRef, attributes: CFTypeRef) -> OSStatus;
    fn SecItemDelete(query: CFTypeRef) -> OSStatus;
    static kSecClass: CFTypeRef;
    static kSecClassGenericPassword: CFTypeRef;
    static kSecAttrService: CFTypeRef;
    static kSecAttrAccount: CFTypeRef;
    static kSecAttrAccessGroup: CFTypeRef;
    static kSecAttrAccessible: CFTypeRef;
    static kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly: CFTypeRef;
    static kSecValueData: CFTypeRef;
    static kSecReturnData: CFTypeRef;
    static kSecMatchLimit: CFTypeRef;
    static kSecMatchLimitOne: CFTypeRef;
    #[cfg(target_os = "macos")]
    static kSecUseDataProtectionKeychain: CFTypeRef;
    #[cfg(target_os = "macos")]
    fn SecTaskCreateFromSelf(allocator: CFTypeRef) -> CFTypeRef;
    #[cfg(target_os = "macos")]
    fn SecTaskCopyValueForEntitlement(
        task: CFTypeRef,
        entitlement: CFTypeRef,
        error: *mut CFTypeRef,
    ) -> CFTypeRef;
}

/// An owned CoreFoundation object, released on drop.
struct Cf(CFTypeRef);

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

fn cf_string(s: &str) -> Result<Cf, SecureStoreError> {
    let s = unsafe {
        CFStringCreateWithBytes(
            std::ptr::null(),
            s.as_ptr(),
            s.len() as isize,
            K_CF_STRING_ENCODING_UTF8,
            0,
        )
    };
    owned(s, "CFStringCreateWithBytes")
}

fn cf_data(bytes: &[u8]) -> Result<Cf, SecureStoreError> {
    let d = unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize) };
    owned(d.cast_const(), "CFDataCreate")
}

fn owned(cf: CFTypeRef, what: &str) -> Result<Cf, SecureStoreError> {
    if cf.is_null() {
        Err(SecureStoreError::Os(format!("{what} failed")))
    } else {
        Ok(Cf(cf))
    }
}

/// A `CFDictionary` of `pairs` (the dictionary retains them).
fn dict(pairs: &[(CFTypeRef, CFTypeRef)]) -> Result<Cf, SecureStoreError> {
    let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
    let values: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
    let d = unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            pairs.len() as isize,
            std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
            std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
        )
    };
    owned(d, "CFDictionaryCreate")
}

/// Which keychain this process uses on macOS (see the module's comment).
#[cfg(target_os = "macos")]
mod keychain {
    use super::{Cf, SecTaskCopyValueForEntitlement, SecTaskCreateFromSelf, cf_string};
    use std::sync::atomic::{AtomicU8, Ordering};

    const UNDECIDED: u8 = 0;
    const DATA_PROTECTION: u8 = 1;
    const LEGACY: u8 = 2;

    static MODE: AtomicU8 = AtomicU8::new(UNDECIDED);

    /// Whether the process's code signature carries an entitlement the data
    /// protection keychain takes (an application identifier, or keychain
    /// access groups).
    fn entitled() -> bool {
        let task = Cf(unsafe { SecTaskCreateFromSelf(std::ptr::null()) });
        if task.0.is_null() {
            // Can't tell: try the data protection keychain (a refused write
            // falls back).
            return true;
        }
        [
            "com.apple.application-identifier",
            "application-identifier",
            "keychain-access-groups",
        ]
        .iter()
        .any(|key| {
            let Ok(key) = cf_string(key) else {
                return false;
            };
            let value =
                Cf(unsafe { SecTaskCopyValueForEntitlement(task.0, key.0, std::ptr::null_mut()) });
            !value.0.is_null()
        })
    }

    pub(super) fn data_protection() -> bool {
        match MODE.load(Ordering::Relaxed) {
            DATA_PROTECTION => true,
            LEGACY => false,
            _ => {
                let mode = if entitled() { DATA_PROTECTION } else { LEGACY };
                match MODE.compare_exchange(UNDECIDED, mode, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => {
                        if mode == LEGACY {
                            super::super::debug(
                                "the data protection keychain needs a signed app (no \
                                 application identifier entitlement): using the login keychain",
                            );
                        }
                        mode == DATA_PROTECTION
                    }
                    // Another thread decided (or a write was refused) first.
                    Err(now) => now == DATA_PROTECTION,
                }
            }
        }
    }

    pub(super) fn fall_back() {
        if MODE.swap(LEGACY, Ordering::Relaxed) != LEGACY {
            super::super::debug(
                "the data protection keychain needs a signed app \
                 (errSecMissingEntitlement): using the login keychain",
            );
        }
    }
}

/// What an item's query says: class, service, account, and the access
/// group and keychain. `extra` comes after.
struct Query {
    service: Cf,
    account: Cf,
    group: Option<Cf>,
}

impl Query {
    fn new(target: &Target, key: &str) -> Result<Self, SecureStoreError> {
        Ok(Self {
            service: cf_string(&target.service)?,
            account: cf_string(key)?,
            group: target.access_group.as_deref().map(cf_string).transpose()?,
        })
    }

    /// The query's pairs, in the data protection keychain (`dp`) or not.
    fn pairs(&self, dp: bool) -> Vec<(CFTypeRef, CFTypeRef)> {
        let _ = dp;
        let mut pairs = unsafe {
            vec![
                (kSecClass, kSecClassGenericPassword),
                (kSecAttrService, self.service.0),
                (kSecAttrAccount, self.account.0),
            ]
        };
        #[cfg(target_os = "macos")]
        if dp {
            pairs.push(unsafe { (kSecUseDataProtectionKeychain, kCFBooleanTrue) });
        }
        // The login keychain has no access groups.
        if let Some(group) = &self.group
            && (dp || cfg!(target_os = "ios"))
        {
            pairs.push((unsafe { kSecAttrAccessGroup }, group.0));
        }
        pairs
    }
}

/// Run `op` with the data protection keychain, or (macOS) with the login
/// keychain when the process isn't allowed the first.
fn with_keychain(
    target: &Target,
    mut op: impl FnMut(bool) -> Result<OSStatus, SecureStoreError>,
) -> Result<OSStatus, SecureStoreError> {
    #[cfg(target_os = "macos")]
    {
        // With an access group, the data protection keychain is the only
        // one that has it.
        if target.access_group.is_some() || keychain::data_protection() {
            let status = op(true)?;
            // With an access group, the entitlement is what the app asked
            // for: no quiet fallback to a keychain without groups.
            if status != ERR_SEC_MISSING_ENTITLEMENT || target.access_group.is_some() {
                return Ok(status);
            }
            keychain::fall_back();
        }
        op(false)
    }
    #[cfg(target_os = "ios")]
    {
        let _ = target;
        op(true)
    }
}

fn error(status: OSStatus, what: &str) -> SecureStoreError {
    match status {
        ERR_SEC_INTERACTION_NOT_ALLOWED => SecureStoreError::Locked,
        ERR_SEC_MISSING_ENTITLEMENT => SecureStoreError::Denied(format!(
            "{what}: errSecMissingEntitlement (the access group must be in the app's \
             keychain-access-groups entitlement, and the app signed)"
        )),
        ERR_SEC_USER_CANCELED => SecureStoreError::Denied(format!("{what}: the user cancelled")),
        ERR_SEC_AUTH_FAILED => SecureStoreError::Denied(format!("{what}: errSecAuthFailed")),
        ERR_SEC_NOT_AVAILABLE | ERR_SEC_NO_SUCH_KEYCHAIN => {
            SecureStoreError::Unavailable(format!("{what}: no keychain (OSStatus {status})"))
        }
        _ => SecureStoreError::Os(format!("{what}: OSStatus {status}")),
    }
}

pub(super) fn get(target: &Target, key: &str) -> Result<Option<Vec<u8>>, SecureStoreError> {
    let q = Query::new(target, key)?;
    let mut found: Option<Vec<u8>> = None;
    let status = with_keychain(target, |dp| {
        let mut pairs = q.pairs(dp);
        unsafe {
            pairs.push((kSecReturnData, kCFBooleanTrue));
            pairs.push((kSecMatchLimit, kSecMatchLimitOne));
        }
        let query = dict(&pairs)?;
        let mut result: CFTypeRef = std::ptr::null();
        let status = unsafe { SecItemCopyMatching(query.0, &mut result) };
        let result = Cf(result);
        if status == ERR_SEC_SUCCESS {
            if result.0.is_null() || unsafe { CFGetTypeID(result.0) != CFDataGetTypeID() } {
                return Err(SecureStoreError::Os(
                    "SecItemCopyMatching returned no data".into(),
                ));
            }
            let bytes = unsafe {
                let len = CFDataGetLength(result.0).max(0) as usize;
                let ptr = CFDataGetBytePtr(result.0);
                if len == 0 || ptr.is_null() {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(ptr, len).to_vec()
                }
            };
            found = Some(bytes);
        }
        Ok(status)
    })?;
    match status {
        ERR_SEC_SUCCESS => Ok(found),
        ERR_SEC_ITEM_NOT_FOUND => Ok(None),
        s => Err(error(s, "SecItemCopyMatching")),
    }
}

pub(super) fn set(target: &Target, key: &str, value: &[u8]) -> Result<(), SecureStoreError> {
    let q = Query::new(target, key)?;
    let data = cf_data(value)?;
    let mut what = "SecItemAdd";
    let status = with_keychain(target, |dp| {
        let protection = |pairs: &mut Vec<(CFTypeRef, CFTypeRef)>| {
            // The login keychain has no accessibility classes.
            if dp || cfg!(target_os = "ios") {
                pairs.push(unsafe {
                    (
                        kSecAttrAccessible,
                        kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
                    )
                });
            }
        };
        let mut attrs = q.pairs(dp);
        attrs.push((unsafe { kSecValueData }, data.0));
        protection(&mut attrs);
        let attrs = dict(&attrs)?;
        what = "SecItemAdd";
        let status = unsafe { SecItemAdd(attrs.0, std::ptr::null_mut()) };
        if status != ERR_SEC_DUPLICATE_ITEM {
            return Ok(status);
        }
        // There already: replace its data (and its accessibility, which an
        // older item may not have had).
        let query = dict(&q.pairs(dp))?;
        let mut update = vec![(unsafe { kSecValueData }, data.0)];
        protection(&mut update);
        let update = dict(&update)?;
        what = "SecItemUpdate";
        Ok(unsafe { SecItemUpdate(query.0, update.0) })
    })?;
    match status {
        ERR_SEC_SUCCESS => Ok(()),
        s => Err(error(s, what)),
    }
}

pub(super) fn remove(target: &Target, key: &str) -> Result<(), SecureStoreError> {
    let q = Query::new(target, key)?;
    let status = with_keychain(target, |dp| {
        let query = dict(&q.pairs(dp))?;
        Ok(unsafe { SecItemDelete(query.0) })
    })?;
    match status {
        ERR_SEC_SUCCESS | ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        s => Err(error(s, "SecItemDelete")),
    }
}
