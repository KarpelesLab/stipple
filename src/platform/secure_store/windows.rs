//! Windows: generic credentials in Credential Manager (`CredWriteW`,
//! `CredReadW`, `CredDeleteW`), one per key, named `<service>/<key>`, kept
//! on this machine (`CRED_PERSIST_LOCAL_MACHINE`: they don't roam with a
//! roaming profile).
//!
//! Credential Manager rather than DPAPI over files of Stipple's own: the
//! credentials are encrypted with DPAPI for the user anyway, a write is
//! atomic without a file format or locking of Stipple's, and the user sees
//! (and can remove) them in Control Panel's Credential Manager. Its limit
//! of 2560 bytes a credential is [`SecureStore::MAX_VALUE_LEN`](super::SecureStore::MAX_VALUE_LEN).

use super::{SecureStoreError, Target};
use std::ffi::c_void;

const CRED_TYPE_GENERIC: u32 = 1;
const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;
const ERROR_NOT_FOUND: u32 = 1168;
const ERROR_NO_SUCH_LOGON_SESSION: u32 = 1312;
const ERROR_ACCESS_DENIED: u32 = 5;

#[repr(C)]
struct FileTime {
    low: u32,
    high: u32,
}

/// `CREDENTIALW`.
#[repr(C)]
struct Credential {
    flags: u32,
    kind: u32,
    target_name: *mut u16,
    comment: *mut u16,
    last_written: FileTime,
    blob_size: u32,
    blob: *mut u8,
    persist: u32,
    attribute_count: u32,
    attributes: *mut c_void,
    target_alias: *mut u16,
    user_name: *mut u16,
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn CredWriteW(credential: *const Credential, flags: u32) -> i32;
    fn CredReadW(
        target: *const u16,
        kind: u32,
        flags: u32,
        credential: *mut *mut Credential,
    ) -> i32;
    fn CredDeleteW(target: *const u16, kind: u32, flags: u32) -> i32;
    fn CredFree(buffer: *mut c_void);
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn error(code: u32, what: &str) -> SecureStoreError {
    match code {
        ERROR_NO_SUCH_LOGON_SESSION => SecureStoreError::Unavailable(format!(
            "{what}: no logon session with a user profile (ERROR_NO_SUCH_LOGON_SESSION)"
        )),
        ERROR_ACCESS_DENIED => SecureStoreError::Denied(format!("{what}: ERROR_ACCESS_DENIED")),
        _ => SecureStoreError::Os(format!("{what}: error {code}")),
    }
}

pub(super) fn get(target: &Target, key: &str) -> Result<Option<Vec<u8>>, SecureStoreError> {
    let name = wide(&super::credential_name(&target.service, key));
    let mut cred: *mut Credential = std::ptr::null_mut();
    if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) } == 0 {
        return match unsafe { GetLastError() } {
            ERROR_NOT_FOUND => Ok(None),
            code => Err(error(code, "CredReadW")),
        };
    }
    let value = unsafe {
        let c = &*cred;
        if c.blob.is_null() || c.blob_size == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(c.blob, c.blob_size as usize).to_vec()
        }
    };
    unsafe { CredFree(cred.cast()) };
    Ok(Some(value))
}

pub(super) fn set(target: &Target, key: &str, value: &[u8]) -> Result<(), SecureStoreError> {
    let mut name = wide(&super::credential_name(&target.service, key));
    let mut user = wide(key);
    let mut blob = value.to_vec();
    let cred = Credential {
        flags: 0,
        kind: CRED_TYPE_GENERIC,
        target_name: name.as_mut_ptr(),
        comment: std::ptr::null_mut(),
        last_written: FileTime { low: 0, high: 0 },
        blob_size: blob.len() as u32,
        blob: if blob.is_empty() {
            std::ptr::null_mut()
        } else {
            blob.as_mut_ptr()
        },
        persist: CRED_PERSIST_LOCAL_MACHINE,
        attribute_count: 0,
        attributes: std::ptr::null_mut(),
        target_alias: std::ptr::null_mut(),
        user_name: user.as_mut_ptr(),
    };
    let ok = unsafe { CredWriteW(&cred, 0) } != 0;
    // The copy of the secret this side made.
    blob.iter_mut().for_each(|b| *b = 0);
    if ok {
        Ok(())
    } else {
        Err(error(unsafe { GetLastError() }, "CredWriteW"))
    }
}

pub(super) fn remove(target: &Target, key: &str) -> Result<(), SecureStoreError> {
    let name = wide(&super::credential_name(&target.service, key));
    if unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) } != 0 {
        return Ok(());
    }
    match unsafe { GetLastError() } {
        ERROR_NOT_FOUND => Ok(()),
        code => Err(error(code, "CredDeleteW")),
    }
}
