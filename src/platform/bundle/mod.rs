//! Where the app keeps its files, and what it is: [`dirs`] gives the app's
//! data, config, cache and temp directories, [`app_info`] its identifier,
//! display name, version and build number.
//!
//! | | [`AppInfo`] from | [`AppDirs::data`] | [`AppDirs::cache`] | [`AppDirs::temp`] |
//! |---|---|---|---|---|
//! | macOS | the bundle's `Info.plist` (`NSBundle`) | `~/Library/Application Support/<id>` | `~/Library/Caches/<id>` | `$TMPDIR/<id>` |
//! | iOS | the bundle's `Info.plist` | `Library/Application Support` | `Library/Caches` | `tmp` |
//! | Android | `PackageManager.getPackageInfo` | `Context.getFilesDir()` | `Context.getCacheDir()` | `getCacheDir()/tmp` |
//! | Windows | the executable's version resource | `%APPDATA%\<id>` | `%LOCALAPPDATA%\<id>\Cache` | `%TEMP%\<id>` |
//! | Linux | — | `$XDG_DATA_HOME/<id>` | `$XDG_CACHE_HOME/<id>` | `$XDG_RUNTIME_DIR/<id>` |
//! | Web | — | — | — | — |
//!
//! What the OS doesn't say comes from [`set_app_info`] (or
//! [`App::app_info`](crate::App::app_info)), then from the executable's name.
//! See `docs/os-integration.md`.

use crate::platform::PlatformError;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
mod apple;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[allow(unsafe_code)]
mod unix {
    unsafe extern "C" {
        fn getuid() -> u32;
    }

    /// The real user id of the process.
    pub(super) fn uid() -> u32 {
        unsafe { getuid() }
    }
}

/// What the app is: its identifier, the name shown to the user, its version
/// and its build number.
///
/// [`app_info`] reads it from the OS where the app is packaged (an app
/// bundle's `Info.plist`, the Android package, a Windows version resource);
/// [`set_app_info`] gives the values to use where it isn't.
///
/// ```
/// use stipple::platform::AppInfo;
/// let info = AppInfo::new("org.example.Notes")
///     .name("Notes")
///     .version(env!("CARGO_PKG_VERSION"))
///     .build("42");
/// assert_eq!(info.id, "org.example.Notes");
/// assert_eq!(info.build.as_deref(), Some("42"));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AppInfo {
    /// A unique, stable identifier, in reverse-DNS form by convention
    /// (`org.example.Notes`): the bundle identifier on macOS and iOS, the
    /// package name on Android. Desktop directories are named after it.
    pub id: String,
    /// The name shown to the user.
    pub name: String,
    /// The version shown to the user (`CFBundleShortVersionString`,
    /// `versionName`), if known.
    pub version: Option<String>,
    /// The build number (`CFBundleVersion`, `versionCode`), if known.
    pub build: Option<String>,
}

impl AppInfo {
    /// An app identified by `id`, named after it, with no version.
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            name: id.clone(),
            id,
            version: None,
            build: None,
        }
    }

    /// Set the name shown to the user.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the version (`env!("CARGO_PKG_VERSION")`, say).
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Set the build number.
    pub fn build(mut self, build: impl Into<String>) -> Self {
        self.build = Some(build.into());
        self
    }
}

/// The app's own directories. Each one exists once [`dirs`] returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AppDirs {
    /// Persistent files the user would miss: a database, documents. Backed
    /// up where the OS backs up apps (iOS, Android, macOS Time Machine,
    /// Windows roaming profiles).
    pub data: PathBuf,
    /// Settings. The same directory as `data` everywhere but Linux
    /// (`$XDG_CONFIG_HOME/<id>`).
    pub config: PathBuf,
    /// Files that can be made again (downloads, thumbnails). Kept across
    /// launches, but not backed up, and the OS may delete them when storage
    /// runs low.
    pub cache: PathBuf,
    /// Scratch files, for the run that wrote them. May be cleared at any
    /// time the app isn't running, or at reboot.
    pub temp: PathBuf,
}

/// What the app said of itself through [`set_app_info`].
static GIVEN: Mutex<Option<AppInfo>> = Mutex::new(None);

/// Say what the app is, for where the OS can't tell: on Windows and Linux
/// without a packaged version, and on macOS for a binary that isn't in an
/// app bundle (a `cargo run`). Values the OS has (the bundle's, the Android
/// package's) win over these.
///
/// Call it before [`dirs`]: desktop directories are named after the id.
/// [`App::app_info`](crate::App::app_info) calls it.
pub fn set_app_info(info: AppInfo) {
    *GIVEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(info);
}

/// The identifier the app gave through [`set_app_info`], if any.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn given_id() -> Option<String> {
    GIVEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|i| i.id.clone())
        .filter(|id| !id.is_empty())
}

/// What the OS knows of the app; empty fields where it knows nothing.
#[derive(Default)]
#[cfg_attr(
    not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "android",
        target_os = "windows"
    )),
    allow(dead_code)
)]
pub(crate) struct OsInfo {
    pub(crate) id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) build: Option<String>,
}

/// The app's identifier, display name, version and build number: the OS's
/// (the bundle, the package, the version resource), else what
/// [`set_app_info`] gave, else the executable's name (and no version).
///
/// ```no_run
/// let info = stipple::platform::app_info();
/// println!("{} {}", info.name, info.version.as_deref().unwrap_or("?"));
/// ```
pub fn app_info() -> AppInfo {
    let given = GIVEN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    merge(os_info(), given, exe_name())
}

/// The OS's values, then the app's, then `fallback` for the id and name.
fn merge(os: OsInfo, given: Option<AppInfo>, fallback: Option<String>) -> AppInfo {
    let given = given.unwrap_or_default();
    let some = |s: Option<String>| s.filter(|s| !s.is_empty());
    let id = some(os.id)
        .or_else(|| some(Some(given.id.clone())))
        .or_else(|| fallback.clone())
        .unwrap_or_else(|| "app".into());
    let name = some(os.name)
        .or_else(|| some(Some(given.name)))
        .or(fallback)
        .unwrap_or_else(|| id.clone());
    AppInfo {
        id,
        name,
        version: some(os.version).or_else(|| some(given.version)),
        build: some(os.build).or_else(|| some(given.build)),
    }
}

/// The executable's file name without its extension.
fn exe_name() -> Option<String> {
    std::env::current_exe()
        .ok()?
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
}

/// The app's data, config, cache and temp directories, created if they don't
/// exist yet (with their parents; on Linux and macOS, the temp directory
/// readable by the user only).
///
/// On the desktop they are named after [`app_info`]'s id, so give it one
/// first ([`set_app_info`]) unless the app is bundled; on iOS and Android
/// they are the app's own container's. Fails on the web, which has no file
/// system, and when the OS has no answer (no `$HOME` on Linux, Android
/// before the activity starts).
///
/// ```no_run
/// let dirs = stipple::platform::dirs()?;
/// std::fs::write(dirs.data.join("notes.db"), b"")?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn dirs() -> Result<AppDirs, PlatformError> {
    let dirs = os_dirs()?;
    for dir in [&dirs.data, &dirs.config, &dirs.cache] {
        create(dir)?;
    }
    create_private(&dirs.temp)?;
    Ok(dirs)
}

fn create(dir: &Path) -> Result<(), PlatformError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| PlatformError::Os(format!("can't create {}: {e}", dir.display())))
}

/// Create `dir` for the user alone. Under a shared parent (`/tmp`), it must
/// be a directory of the user's, not a link someone else left there.
fn create_private(dir: &Path) -> Result<(), PlatformError> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        if let Some(parent) = dir.parent() {
            create(parent)?;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(PlatformError::Os(format!(
                    "can't create {}: {e}",
                    dir.display()
                )));
            }
        }
        let meta = std::fs::symlink_metadata(dir)
            .map_err(|e| PlatformError::Os(format!("{}: {e}", dir.display())))?;
        // `symlink_metadata`: a link isn't a directory.
        if !meta.is_dir() || meta.uid() != unix::uid() || meta.permissions().mode() & 0o077 != 0 {
            return Err(PlatformError::Os(format!(
                "{} isn't a private directory of the user's",
                dir.display()
            )));
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    create(dir)
}

/// `id` as one safe path component: no separators, no `..`, no controls.
fn dir_name(id: &str) -> String {
    let s: String = id
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if s.is_empty() { "app".into() } else { s.into() }
}

/// The app's directory name on the desktop.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", target_os = "linux")),
    allow(dead_code)
)]
fn desktop_id() -> String {
    dir_name(&app_info().id)
}

/// The XDG base directories for `id`, from the environment `var` gives
/// (`$XDG_DATA_HOME`, `$XDG_CONFIG_HOME`, `$XDG_CACHE_HOME`,
/// `$XDG_RUNTIME_DIR`, `$HOME`, `$TMPDIR`), with the spec's defaults. A
/// relative path in an `XDG_*` variable is ignored, as the spec says.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn xdg_dirs(
    id: &str,
    uid: u32,
    var: impl Fn(&str) -> Option<String>,
) -> Result<AppDirs, PlatformError> {
    let abs = |name: &str| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let home = abs("HOME").ok_or(PlatformError::NoBackend("app directories ($HOME is unset)"))?;
    let base = |name: &str, default: &str| abs(name).unwrap_or_else(|| home.join(default));
    let temp = match abs("XDG_RUNTIME_DIR") {
        Some(run) => run.join(id),
        None => abs("TMPDIR")
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(format!("{id}-{uid}")),
    };
    Ok(AppDirs {
        data: base("XDG_DATA_HOME", ".local/share").join(id),
        config: base("XDG_CONFIG_HOME", ".config").join(id),
        cache: base("XDG_CACHE_HOME", ".cache").join(id),
        temp,
    })
}

// ---- Per-platform dispatch ---------------------------------------------------

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) use apple::resource_dir;

#[cfg(any(target_os = "macos", target_os = "ios"))]
use apple::{dirs as os_dirs, info as os_info};

#[cfg(target_os = "windows")]
fn os_info() -> OsInfo {
    windows::info()
}

#[cfg(target_os = "windows")]
fn os_dirs() -> Result<AppDirs, PlatformError> {
    windows::dirs(&desktop_id())
}

#[cfg(target_os = "android")]
fn os_info() -> OsInfo {
    crate::platform::backend::android::app_info().unwrap_or_default()
}

#[cfg(target_os = "android")]
fn os_dirs() -> Result<AppDirs, PlatformError> {
    crate::platform::backend::android::app_dirs().ok_or(PlatformError::NoBackend(
        "app directories (no activity yet)",
    ))
}

#[cfg(target_os = "linux")]
fn os_info() -> OsInfo {
    OsInfo::default()
}

#[cfg(target_os = "linux")]
fn os_dirs() -> Result<AppDirs, PlatformError> {
    xdg_dirs(&desktop_id(), unix::uid(), |v| std::env::var(v).ok())
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android",
    target_os = "linux"
)))]
fn os_info() -> OsInfo {
    OsInfo::default()
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    target_os = "android",
    target_os = "linux"
)))]
fn os_dirs() -> Result<AppDirs, PlatformError> {
    Err(PlatformError::NoBackend("app directories"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_os_wins_then_the_app_then_the_executable() {
        let os = OsInfo {
            id: Some("com.example.os".into()),
            version: Some("2.0".into()),
            ..OsInfo::default()
        };
        let given = AppInfo::new("org.example.app").name("App").build("7");
        let info = merge(os, Some(given.clone()), Some("exe".into()));
        assert_eq!(info.id, "com.example.os");
        assert_eq!(info.name, "App");
        assert_eq!(info.version.as_deref(), Some("2.0"));
        assert_eq!(info.build.as_deref(), Some("7"));

        let info = merge(OsInfo::default(), Some(given), Some("exe".into()));
        assert_eq!(info.id, "org.example.app");
        assert_eq!(info.version, None);

        let info = merge(OsInfo::default(), None, Some("exe".into()));
        assert_eq!((info.id.as_str(), info.name.as_str()), ("exe", "exe"));
        assert_eq!(info.version, None);
    }

    #[test]
    fn ids_become_one_safe_path_component() {
        assert_eq!(dir_name("org.example.Notes"), "org.example.Notes");
        assert_eq!(dir_name("../etc"), "_etc");
        assert_eq!(dir_name(".."), "app");
        assert_eq!(dir_name("a/b\\c:d"), "a_b_c_d");
        assert_eq!(dir_name("  "), "app");
        assert_eq!(dir_name("My App"), "My App");
    }

    #[test]
    fn xdg_defaults_and_overrides() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let d = xdg_dirs("org.x", 1000, env(&[("HOME", "/home/u")])).unwrap();
        assert_eq!(d.data, Path::new("/home/u/.local/share/org.x"));
        assert_eq!(d.config, Path::new("/home/u/.config/org.x"));
        assert_eq!(d.cache, Path::new("/home/u/.cache/org.x"));
        assert_eq!(d.temp, Path::new("/tmp/org.x-1000"));

        let d = xdg_dirs(
            "org.x",
            1000,
            env(&[
                ("HOME", "/home/u"),
                ("XDG_DATA_HOME", "/d"),
                ("XDG_CONFIG_HOME", "relative/ignored"),
                ("XDG_CACHE_HOME", "/c"),
                ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ]),
        )
        .unwrap();
        assert_eq!(d.data, Path::new("/d/org.x"));
        assert_eq!(d.config, Path::new("/home/u/.config/org.x"));
        assert_eq!(d.cache, Path::new("/c/org.x"));
        assert_eq!(d.temp, Path::new("/run/user/1000/org.x"));

        assert!(xdg_dirs("org.x", 0, env(&[])).is_err());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_temp_directory_is_private() {
        let base = std::env::temp_dir().join(format!("stipple-dirs-test-{}", std::process::id()));
        let dir = base.join("app");
        create_private(&dir).unwrap();
        create_private(&dir).unwrap(); // again: already there
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        // Someone else's link in its place is refused.
        let link = base.join("link");
        std::os::unix::fs::symlink(&base, &link).unwrap();
        assert!(create_private(&link).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
