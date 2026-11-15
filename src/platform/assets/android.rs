//! Android: the APK's `assets/` through the NDK's `AAssetManager`, which
//! the `ANativeActivity` carries. No JNI.
//!
//! An asset is opened while the activity is current (so its manager is
//! alive); an opened `AAsset` then reads on its own, from any thread.

use super::AssetError;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io::SeekFrom;

const AASSET_MODE_UNKNOWN: c_int = 0;
const AASSET_MODE_STREAMING: c_int = 2;
const SEEK_SET: c_int = 0;
const SEEK_CUR: c_int = 1;
const SEEK_END: c_int = 2;

#[link(name = "android")]
unsafe extern "C" {
    fn AAssetManager_open(mgr: *mut c_void, filename: *const c_char, mode: c_int) -> *mut c_void;
    fn AAssetManager_openDir(mgr: *mut c_void, dir: *const c_char) -> *mut c_void;
    fn AAssetDir_getNextFileName(dir: *mut c_void) -> *const c_char;
    fn AAssetDir_close(dir: *mut c_void);
    fn AAsset_read(asset: *mut c_void, buf: *mut c_void, count: usize) -> c_int;
    fn AAsset_seek64(asset: *mut c_void, offset: i64, whence: c_int) -> i64;
    fn AAsset_getLength64(asset: *mut c_void) -> i64;
    fn AAsset_close(asset: *mut c_void);
}

fn c_path(path: &str) -> Result<CString, AssetError> {
    CString::new(path).map_err(|_| AssetError::InvalidPath(path.into()))
}

fn no_activity() -> AssetError {
    AssetError::Unavailable("no activity yet (the APK's asset manager comes with it)".into())
}

/// An open `AAsset`, closed when dropped.
pub(super) struct Asset(*mut c_void);

// An AAsset isn't shared: one owner reads it, from whichever thread.
unsafe impl Send for Asset {}

impl Drop for Asset {
    fn drop(&mut self) {
        unsafe { AAsset_close(self.0) };
    }
}

impl Asset {
    pub(super) fn len(&self) -> u64 {
        u64::try_from(unsafe { AAsset_getLength64(self.0) }).unwrap_or(0)
    }

    pub(super) fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = unsafe { AAsset_read(self.0, buf.as_mut_ptr().cast(), buf.len()) };
        usize::try_from(n).map_err(|_| std::io::Error::other("AAsset_read failed"))
    }

    pub(super) fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(n) => (
                i64::try_from(n).map_err(|_| std::io::Error::other("offset too large"))?,
                SEEK_SET,
            ),
            SeekFrom::Current(n) => (n, SEEK_CUR),
            SeekFrom::End(n) => (n, SEEK_END),
        };
        let at = unsafe { AAsset_seek64(self.0, offset, whence) };
        u64::try_from(at).map_err(|_| std::io::Error::other("AAsset_seek64 failed"))
    }
}

/// Open `path` in `mode`, `None` if there's no such asset.
fn open_mode(path: &str, mode: c_int) -> Result<Option<Asset>, AssetError> {
    let c = c_path(path)?;
    crate::platform::backend::android::with_asset_manager(|mgr| {
        let a = unsafe { AAssetManager_open(mgr, c.as_ptr(), mode) };
        // `then`, not `then_some`: an `Asset` made of a null pointer would be
        // dropped, and `AAsset_close(NULL)` crashes (Android 15).
        Some((!a.is_null()).then(|| Asset(a)))
    })
    .ok_or_else(no_activity)
}

pub(super) fn open(path: &str) -> Result<Asset, AssetError> {
    open_mode(path, AASSET_MODE_STREAMING)?.ok_or_else(|| AssetError::NotFound(path.into()))
}

pub(super) fn read(path: &str) -> Result<Vec<u8>, AssetError> {
    let mut asset = open(path)?;
    let mut out = Vec::with_capacity(usize::try_from(asset.len()).unwrap_or(0));
    std::io::Read::read_to_end(&mut ReadAdapter(&mut asset), &mut out).map_err(|e| {
        AssetError::Io {
            path: path.into(),
            message: e.to_string(),
        }
    })?;
    Ok(out)
}

struct ReadAdapter<'a>(&'a mut Asset);

impl std::io::Read for ReadAdapter<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

pub(super) fn exists(path: &str) -> bool {
    open_mode(path, AASSET_MODE_UNKNOWN).is_ok_and(|a| a.is_some())
}

/// The files directly in `dir` (`AAssetDir` lists files, not directories).
pub(super) fn list(dir: &str) -> Result<Vec<String>, AssetError> {
    let c = c_path(dir)?;
    crate::platform::backend::android::with_asset_manager(|mgr| {
        let d = unsafe { AAssetManager_openDir(mgr, c.as_ptr()) };
        if d.is_null() {
            return Some(Vec::new());
        }
        let mut names = Vec::new();
        loop {
            let name = unsafe { AAssetDir_getNextFileName(d) };
            if name.is_null() {
                break;
            }
            names.push(
                unsafe { CStr::from_ptr(name) }
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        unsafe { AAssetDir_close(d) };
        Some(names)
    })
    .ok_or_else(no_activity)
}
