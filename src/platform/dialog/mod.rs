//! Native file dialogs, and message boxes (see [`message`] and
//! [`AppHandle::message`](crate::core::AppHandle::message)).
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
//! - **iOS, Android**: not implemented; these calls return `None`, as if the
//!   user cancelled.
//!
//! All three return `None` when the user cancels (or no backend is available).
//! These calls block until the user dismisses the dialog.
//!
//! ## Contents, and the web
//!
//! A browser can't block for a dialog, and gives a page a file's contents, not
//! its path. [`open_file_contents`] and [`save_file_contents`] work there and
//! everywhere else, so an app that wants to run on the web uses them:
//!
//! - [`open_file_contents`] reads the chosen file and passes its name and bytes
//!   to a callback. On the desktops the callback runs before the call returns.
//!   On the web the call returns at once; an `<input type=file>` opens, and the
//!   callback runs later, from the event loop, once the browser has read the
//!   file (or with `None` if the picker was cancelled). Change the state from
//!   the callback through an [`AppHandle`](crate::core::AppHandle), whose
//!   update then re-renders.
//! - [`save_file_contents`] asks where to save and writes the bytes. On the web
//!   it downloads them as [`FileDialog::file_name`] instead.
//!
//! On the web [`open_file`], [`save_file`] and [`pick_folder`] return `None`.
//!
//! [`open_file`]: crate::platform::dialog::open_file
//! [`save_file`]: crate::platform::dialog::save_file
//! [`pick_folder`]: crate::platform::dialog::pick_folder
//! [`open_file_contents`]: crate::platform::dialog::open_file_contents
//! [`save_file_contents`]: crate::platform::dialog::save_file_contents
//! [`FileDialog::file_name`]: crate::platform::dialog::FileDialog::file_name

use std::path::PathBuf;

mod message;
pub use message::{
    MessageBox, MessageDone, MessageInput, MessageKind, MessageResponse, message,
    press_native_button,
};
pub(crate) use message::{native_allowed, set_event_loop_running};

/// How a file dialog should be presented.
#[derive(Clone, Debug, Default)]
pub struct FileDialog {
    /// Window title for the dialog.
    pub title: String,
    /// The file name [`save_file_contents`] suggests: the download's name on
    /// the web. The desktop dialogs don't pre-fill it yet.
    pub file_name: String,
    /// Which files [`open_file_contents`] offers on the web, as an
    /// `<input accept>` list (`".txt,.md"`, `"image/*"`); empty for any.
    /// The desktop dialogs don't filter yet.
    pub accept: String,
}

impl FileDialog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn with_file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = name.into();
        self
    }

    pub fn with_accept(mut self, accept: impl Into<String>) -> Self {
        self.accept = accept.into();
        self
    }
}

/// A file's name (without its directory) and contents.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileContents {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Let the user choose a file, read it, and pass it to `done`, or `None` if
/// they cancelled (or it couldn't be read).
///
/// On the desktops `done` runs before this returns. On the web it runs later,
/// from the event loop (see the [module docs](self#contents-and-the-web)), so
/// call this from an input handler (a tap) or the browser won't open a picker.
pub fn open_file_contents(opts: &FileDialog, done: impl FnOnce(Option<FileContents>) + 'static) {
    #[cfg(target_arch = "wasm32")]
    {
        web::open(&opts.accept, Box::new(done));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let contents = open_file(opts).and_then(|path| {
            let bytes = std::fs::read(&path).ok()?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Some(FileContents { name, bytes })
        });
        done(contents);
    }
}

/// Ask where to save, then write `bytes` there. Returns whether they were
/// written. On the web this downloads `bytes` as
/// [`file_name`](FileDialog::file_name) (`"download"` if empty) and returns
/// `true`.
pub fn save_file_contents(opts: &FileDialog, bytes: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let name = if opts.file_name.is_empty() {
            "download"
        } else {
            &opts.file_name
        };
        crate::platform::backend::web::save_file(name, bytes);
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        save_file(opts).is_some_and(|path| std::fs::write(path, bytes).is_ok())
    }
}

/// The web's pending pickers, by request id.
#[cfg(target_arch = "wasm32")]
mod web {
    use super::FileContents;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    type Done = Box<dyn FnOnce(Option<FileContents>)>;

    thread_local! {
        static PENDING: RefCell<HashMap<u32, Done>> = RefCell::new(HashMap::new());
        static NEXT: Cell<u32> = const { Cell::new(1) };
    }

    pub(super) fn open(accept: &str, done: Done) {
        let id = NEXT.with(|n| n.replace(n.get().wrapping_add(1)));
        PENDING.with(|p| p.borrow_mut().insert(id, done));
        crate::platform::backend::web::open_file(id, accept);
    }

    pub(crate) fn picked(id: u32, file: Option<(String, Vec<u8>)>) {
        if let Some(done) = PENDING.with(|p| p.borrow_mut().remove(&id)) {
            done(file.map(|(name, bytes)| FileContents { name, bytes }));
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use web::picked as web_picked;

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

/// Let the user pick a color in the platform's color dialog, starting from
/// `initial`; `None` if they cancelled or there's no dialog. Blocks until the
/// dialog closes, like [`open_file`].
///
/// Only Windows has one (`ChooseColorW`, which keeps `initial`'s alpha);
/// elsewhere this returns `None` at once: use the drawn
/// [`color_picker`](crate::widgets::color_picker) or
/// [`color_button`](crate::widgets::color_button), which work everywhere.
/// (macOS's `NSColorPanel` is a floating panel rather than a dialog, and the
/// desktop portal only picks a color off the screen.)
pub fn pick_color(initial: crate::render::Color) -> Option<crate::render::Color> {
    #[cfg(target_os = "windows")]
    {
        backend::pick_color(initial)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = initial;
        None
    }
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

    // iOS and Android have no picker wired up yet, and the web's can't return
    // a path (see `open_file_contents`); report "cancelled".
    pub(super) fn file_chooser(_op: Op, _title: &str) -> Option<PathBuf> {
        None
    }
}
