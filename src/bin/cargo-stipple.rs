//! `cargo stipple`: the store packaging tool (docs/packaging.md), installed
//! with `cargo install stipple`.
//!
//! The tool itself is the Python scripts in `tools/package` (standard library
//! only, Python 3.11 or later). This binary carries them, writes them to a
//! per-user cache directory, and runs them with the arguments it was given.
//! The scripts then prefer the copy in the app's own Stipple (the crate
//! Cargo downloaded for the app's dependency), so the packaging always
//! matches the library the app builds against. The binary is also the
//! scripts' host helper (icons, the Android dex) when the app depends on
//! this very release, which saves building one.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[path = "../../tools/package/helper.rs"]
mod helper;

/// A file of the repository (and of the published crate, through
/// Cargo.toml's `include`), at its path there.
macro_rules! file {
    ($path:literal) => {
        ($path, include_bytes!(concat!("../../", $path)))
    };
}

/// The scripts, at their paths under the cache directory.
const FILES: &[(&str, &[u8])] = &[
    file!("tools/package/cargo-stipple"),
    file!("tools/package/android.py"),
    file!("tools/package/android_code.py"),
    file!("tools/package/common.py"),
    file!("tools/package/helper.rs"),
    file!("tools/package/ios.py"),
    file!("tools/package/ios_extensions.py"),
    file!("tools/package/linux.py"),
    file!("tools/package/manifest.py"),
    file!("tools/android/billing.py"),
];

/// What the scripts' `common.Helper` passes before the helper's own
/// arguments when it runs this binary as the helper.
const HELPER_ARG: &str = "__stipple-helper";

fn main() -> ExitCode {
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == HELPER_ARG) {
        let rest: Vec<String> = args[1..]
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        return helper::run(&rest);
    }
    // `cargo stipple ...` runs `cargo-stipple stipple ...`.
    if args.first().is_some_and(|a| a == "stipple") {
        args.remove(0);
    }
    if args.first().is_some_and(|a| a == "--version" || a == "-V") {
        println!("cargo-stipple {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    match launch(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("cargo-stipple: error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn launch(args: Vec<OsString>) -> Result<ExitCode, String> {
    let dir = unpack()?;
    let script = dir.join("tools/package/cargo-stipple");
    let exe = std::env::current_exe().map_err(|e| format!("this executable's path: {e}"))?;
    let (python, python_args) = python();
    let mut cmd = Command::new(&python);
    cmd.args(python_args)
        .arg(&script)
        .args(args)
        .env("STIPPLE_PACKAGE_HELPER", &exe)
        .env("STIPPLE_PACKAGE_VERSION", env!("CARGO_PKG_VERSION"));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = cmd.exec();
        Err(not_found(&python, e))
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().map_err(|e| not_found(&python, e))?;
        Ok(ExitCode::from(
            status.code().unwrap_or(1).clamp(0, 255) as u8
        ))
    }
}

fn not_found(python: &OsString, e: std::io::Error) -> String {
    format!(
        "running {}: {e} (the tool needs Python 3.11 or later; set STIPPLE_PYTHON to it)",
        python.to_string_lossy()
    )
}

/// The Python to run: `STIPPLE_PYTHON`, else `python3` (`py -3` on Windows,
/// where `python3` is often the Store's placeholder).
fn python() -> (OsString, Vec<&'static str>) {
    if let Some(p) = std::env::var_os("STIPPLE_PYTHON").filter(|p| !p.is_empty()) {
        return (p, vec![]);
    }
    if cfg!(windows) {
        ("py".into(), vec!["-3"])
    } else {
        ("python3".into(), vec![])
    }
}

/// Write the scripts to `<cache>/cargo-stipple/<version>-<hash>/` (once:
/// files already there with the same contents are left alone), and return
/// that directory.
fn unpack() -> Result<PathBuf, String> {
    // FNV-1a over the files, so a build with other scripts gets its own
    // directory rather than overwriting one another run may be reading.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (name, data) in FILES {
        for b in name.bytes().chain(data.iter().copied()) {
            hash = (hash ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    let dir = cache_dir()?
        .join("cargo-stipple")
        .join(format!("{}-{hash:016x}", env!("CARGO_PKG_VERSION")));
    for (name, data) in FILES {
        let path = dir.join(name);
        if std::fs::read(&path).is_ok_and(|have| have == *data) {
            continue;
        }
        let parent = path.parent().expect("a file under the directory");
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        // Written beside, then renamed: a concurrent run never reads half a file.
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, data).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(dir)
}

/// The user's cache directory: `$XDG_CACHE_HOME` or `~/.cache` (Linux and
/// the BSDs), `~/Library/Caches` (macOS), `%LOCALAPPDATA%` (Windows).
fn cache_dir() -> Result<PathBuf, String> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let dir = if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Caches"))
    } else {
        var("XDG_CACHE_HOME")
            .filter(|p| p.is_absolute())
            .or_else(|| var("HOME").map(|h| h.join(".cache")))
    };
    dir.filter(|d| Path::new(d).is_absolute())
        .ok_or_else(|| "no cache directory (set HOME)".to_string())
}
