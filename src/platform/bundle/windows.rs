//! Windows: the known folders (`SHGetKnownFolderPath`), and the name and
//! version from the executable's version resource (`VS_VERSIONINFO`, which
//! a build script adds with `winres`, `embed-resource` or an `.rc` file).
//! A Rust executable has none by default, so the app's
//! [`set_app_info`](super::set_app_info) answers then.

use super::{AppDirs, OsInfo};
use crate::platform::PlatformError;
use std::ffi::c_void;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// `FOLDERID_RoamingAppData`, `%APPDATA%`.
const ROAMING_APP_DATA: Guid = Guid {
    data1: 0x3EB6_85DB,
    data2: 0x65F9,
    data3: 0x4CF6,
    data4: [0xA0, 0x3A, 0xE3, 0xEF, 0x65, 0x72, 0x9F, 0x3D],
};

/// `FOLDERID_LocalAppData`, `%LOCALAPPDATA%`.
const LOCAL_APP_DATA: Guid = Guid {
    data1: 0xF1B3_2785,
    data2: 0x6FBA,
    data3: 0x4FCF,
    data4: [0x9D, 0x55, 0x7B, 0x8E, 0x7F, 0x15, 0x70, 0x91],
};

/// The `dwSignature` of a `VS_FIXEDFILEINFO`.
const FIXED_FILE_INFO_SIGNATURE: u32 = 0xFEEF_04BD;

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHGetKnownFolderPath(
        id: *const Guid,
        flags: u32,
        token: *mut c_void,
        path: *mut *mut u16,
    ) -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoTaskMemFree(p: *mut c_void);
}

#[link(name = "version")]
unsafe extern "system" {
    fn GetFileVersionInfoSizeW(file: *const u16, handle: *mut u32) -> u32;
    fn GetFileVersionInfoW(file: *const u16, handle: u32, len: u32, data: *mut c_void) -> i32;
    fn VerQueryValueW(
        block: *const c_void,
        sub_block: *const u16,
        buffer: *mut *mut c_void,
        len: *mut u32,
    ) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The known folder `id`, else the environment variable `var`.
fn known_folder(id: &Guid, var: &str) -> Option<PathBuf> {
    let mut p: *mut u16 = std::ptr::null_mut();
    let hr = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut p) };
    let path = if hr >= 0 && !p.is_null() {
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        let s = std::ffi::OsString::from_wide(unsafe { std::slice::from_raw_parts(p, len) });
        Some(PathBuf::from(s))
    } else {
        None
    };
    // Freed whether it succeeded or not, as the documentation says.
    unsafe { CoTaskMemFree(p as *mut c_void) };
    path.or_else(|| std::env::var_os(var).map(PathBuf::from))
        .filter(|p| p.is_absolute())
}

pub(super) fn dirs(id: &str) -> Result<AppDirs, PlatformError> {
    let roaming = known_folder(&ROAMING_APP_DATA, "APPDATA")
        .ok_or(PlatformError::NoBackend("app directories (no %APPDATA%)"))?;
    let local = known_folder(&LOCAL_APP_DATA, "LOCALAPPDATA").ok_or(PlatformError::NoBackend(
        "app directories (no %LOCALAPPDATA%)",
    ))?;
    // GetTempPath2W / GetTempPathW: the user's own `%TEMP%`.
    let temp = std::env::temp_dir().join(id);
    Ok(AppDirs {
        data: roaming.join(id),
        config: roaming.join(id),
        cache: local.join(id).join("Cache"),
        temp,
    })
}

/// The executable's version resource.
struct VersionInfo(Vec<u8>);

impl VersionInfo {
    fn of(exe: &Path) -> Option<Self> {
        let name: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut handle = 0u32;
        let size = unsafe { GetFileVersionInfoSizeW(name.as_ptr(), &mut handle) };
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        let ok = unsafe { GetFileVersionInfoW(name.as_ptr(), 0, size, data.as_mut_ptr().cast()) };
        (ok != 0).then_some(Self(data))
    }

    /// The value at `path`, and its length (`VerQueryValueW`'s unit).
    fn query(&self, path: &str) -> Option<(*const u8, usize)> {
        let sub = wide(path);
        let mut p: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let ok = unsafe { VerQueryValueW(self.0.as_ptr().cast(), sub.as_ptr(), &mut p, &mut len) };
        (ok != 0 && !p.is_null() && len > 0).then_some((p as *const u8, len as usize))
    }

    /// The `(language, code page)` pairs the strings are given in.
    fn translations(&self) -> Vec<(u16, u16)> {
        let Some((p, len)) = self.query("\\VarFileInfo\\Translation") else {
            return Vec::new();
        };
        let bytes = unsafe { std::slice::from_raw_parts(p, len) };
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                (
                    u16::from_le_bytes([c[0], c[1]]),
                    u16::from_le_bytes([c[2], c[3]]),
                )
            })
            .collect()
    }

    /// The string `name` (`ProductName`, `ProductVersion`…) in the first
    /// translation that has it.
    fn string(&self, name: &str) -> Option<String> {
        let mut tables: Vec<String> = self
            .translations()
            .into_iter()
            .map(|(lang, cp)| format!("{lang:04x}{cp:04x}"))
            .collect();
        // US English in Unicode, which most resources use, if no table is
        // listed.
        tables.push("040904b0".into());
        tables.into_iter().find_map(|t| {
            let (p, len) = self.query(&format!("\\StringFileInfo\\{t}\\{name}"))?;
            // `len` counts UTF-16 units here, with the terminating NUL.
            let units = unsafe { std::slice::from_raw_parts(p as *const u16, len) };
            let units = units.split(|&u| u == 0).next().unwrap_or(&[]);
            let s = String::from_utf16_lossy(units).trim().to_string();
            (!s.is_empty()).then_some(s)
        })
    }

    /// The fixed product version, `a.b.c.d`, unless it is all zeros.
    fn fixed_version(&self) -> Option<String> {
        let (p, len) = self.query("\\")?;
        if len < 52 {
            return None;
        }
        let words = unsafe { std::slice::from_raw_parts(p as *const u32, 6) };
        if words[0] != FIXED_FILE_INFO_SIGNATURE || (words[4] | words[5]) == 0 {
            return None;
        }
        let (ms, ls) = (words[4], words[5]);
        Some(format!(
            "{}.{}.{}.{}",
            ms >> 16,
            ms & 0xFFFF,
            ls >> 16,
            ls & 0xFFFF
        ))
    }
}

pub(super) fn info() -> OsInfo {
    let Some(v) = std::env::current_exe()
        .ok()
        .and_then(|exe| VersionInfo::of(&exe))
    else {
        return OsInfo::default();
    };
    OsInfo {
        id: None,
        name: v
            .string("ProductName")
            .or_else(|| v.string("FileDescription")),
        version: v.string("ProductVersion").or_else(|| v.fixed_version()),
        build: None,
    }
}
