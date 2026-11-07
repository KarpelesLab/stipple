//! Native file dialogs.
//!
//! A tiny platform-neutral surface — [`open_file`], [`save_file`],
//! [`pick_folder`] — backed by each OS's own dialog so apps get the system file
//! picker (with its bookmarks, permissions, and look) rather than a drawn-in-app
//! imitation:
//!
//! - **Linux**: `org.freedesktop.portal.FileChooser` over D-Bus
//!   (`xdg-desktop-portal`), driven by the hand-written client in [`crate::platform::a11y`].
//!   Works inside sandboxes and across desktops without linking GTK/Qt.
//! - **Windows**: the shell's `IFileOpenDialog` / `IFileSaveDialog` (COM, with
//!   hand-written vtables), `FOS_PICKFOLDERS` for folders. The dialog is modal
//!   to the thread's active window.
//! - **macOS**: `NSOpenPanel` / `NSSavePanel`, run with `runModal`. Call these
//!   from the main thread (the event handler runs there).
//! - **iOS, Android, web**: not implemented; these calls return `None`, as if
//!   the user cancelled.
//!
//! All three return `None` when the user cancels (or no backend is available).
//! These calls block until the user dismisses the dialog.

use std::path::PathBuf;

/// How a file dialog should be presented.
#[derive(Clone, Debug, Default)]
pub struct FileDialog {
    /// Window title for the dialog.
    pub title: String,
}

impl FileDialog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }
}

/// Prompt the user to choose an existing file to open.
pub fn open_file(opts: &FileDialog) -> Option<PathBuf> {
    let title = title_or(opts, "Open File");
    backend::file_chooser(Op::Open, &title)
}

/// Prompt the user to choose a destination to save a file.
pub fn save_file(opts: &FileDialog) -> Option<PathBuf> {
    let title = title_or(opts, "Save File");
    backend::file_chooser(Op::Save, &title)
}

/// Prompt the user to choose a directory.
pub fn pick_folder(opts: &FileDialog) -> Option<PathBuf> {
    let title = title_or(opts, "Select Folder");
    backend::file_chooser(Op::Folder, &title)
}

fn title_or(opts: &FileDialog, default: &str) -> String {
    if opts.title.is_empty() {
        default.to_string()
    } else {
        opts.title.clone()
    }
}

/// Which dialog flavour to present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Open,
    Save,
    Folder,
}

#[cfg(target_os = "linux")]
mod backend {
    use super::{Op, PathBuf};

    pub(super) fn file_chooser(op: Op, title: &str) -> Option<PathBuf> {
        let (member, directory) = match op {
            Op::Open => ("OpenFile", false),
            Op::Save => ("SaveFile", false),
            Op::Folder => ("OpenFile", true),
        };
        let mut bus = crate::platform::a11y::DBus::connect_session().ok()?;
        bus.portal_file_chooser(member, title, directory)
            .ok()
            .flatten()
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
#[path = "windows.rs"]
mod backend;

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[path = "macos.rs"]
mod backend;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
mod backend {
    use super::{Op, PathBuf};

    // iOS, Android and the web (`<input type=file>`) have no picker wired up
    // yet; report "cancelled".
    pub(super) fn file_chooser(_op: Op, _title: &str) -> Option<PathBuf> {
        None
    }
}
