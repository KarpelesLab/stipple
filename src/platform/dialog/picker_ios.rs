//! iOS document pickers, for
//! [`open_file_contents`](super::open_file_contents) and
//! [`save_file_contents`](super::save_file_contents): a
//! `UIDocumentPickerViewController` presented from the key window's topmost
//! view controller.
//!
//! - **Open**: `initForOpeningContentTypes:asCopy:` with `asCopy` set, so the
//!   app gets a copy of the document in its own tmp directory, readable
//!   without security-scoped access. The content types are `UTType`s
//!   (`UniformTypeIdentifiers`) made from [`FileDialog::accept`](super::FileDialog::accept)
//!   (see [`mobile::ios_content_types`]). The copy is read, handed to the
//!   callback with its name, and deleted.
//! - **Save**: the bytes are written to a file of their own (named
//!   [`FileDialog::file_name`](super::FileDialog::file_name)) in the tmp
//!   directory, which `initForExportingURLs:asCopy:` lets the user copy
//!   wherever they choose (Files, iCloud Drive, another app's provider); the
//!   file goes once the picker closes.
//!
//! - **Folder**: `initForOpeningContentTypes:asCopy:` for `public.folder`
//!   without a copy: the picked URL is outside the sandbox, so its
//!   security-scoped access is started (`startAccessingSecurityScopedResource`)
//!   and held by a [`Scope`] inside the [`Folder`](super::Folder), which
//!   stops it once the last clone of the folder goes.
//!
//! The picker's delegate is a class built at run time
//! (`StippleDocumentPickerDelegate`, adopting `UIDocumentPickerDelegate`):
//! `documentPicker:didPickDocumentsAtURLs:` and `documentPickerWasCancelled:`
//! answer the request, on the main thread, which is the event loop's.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::PathBuf;

use super::mobile::{self, ContentType};
use crate::platform::native::objc::{Id, Sel, class, msg, nsstring, rust_string, with_pool};

// Not loaded by UIKit; linking it makes `UTType` known.
#[link(name = "UniformTypeIdentifiers", kind = "framework")]
unsafe extern "C" {}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_allocateClassPair(superclass: Id, name: *const i8, extra: usize) -> Id;
    fn objc_registerClassPair(cls: Id);
    fn objc_getClass(name: *const i8) -> Id;
    fn objc_getProtocol(name: *const i8) -> *mut c_void;
    fn class_addMethod(cls: Id, name: Sel, imp: *const c_void, types: *const i8) -> bool;
    fn class_addProtocol(cls: Id, protocol: *mut c_void) -> bool;
    fn object_getClassName(obj: Id) -> *const i8;
}

/// What a picker on screen was opened for.
enum Kind {
    Open,
    Folder,
    /// The directory holding the file being exported.
    Export(PathBuf),
}

struct Pending {
    id: u32,
    /// The picker, retained until it answers.
    picker: Id,
    kind: Kind,
}

thread_local! {
    static PENDING: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
}

/// Let the user pick a document of one of `types`, for dialog request `id`.
/// `false` if no picker could be shown.
pub(crate) fn open(id: u32, types: &[ContentType]) -> bool {
    with_pool(|| unsafe {
        let ut = class("UTType");
        if ut.is_null() {
            return false;
        }
        let array = msg!(class("NSMutableArray"), "array", () -> Id);
        for t in types {
            let ty = match t {
                ContentType::Identifier(s) => {
                    let s = nsstring(s);
                    msg!(ut, "typeWithIdentifier:", (s: Id) -> Id)
                }
                ContentType::Extension(e) => {
                    let e = nsstring(e);
                    msg!(ut, "typeWithFilenameExtension:", (e: Id) -> Id)
                }
                ContentType::Mime(m) => {
                    let m = nsstring(m);
                    msg!(ut, "typeWithMIMEType:", (m: Id) -> Id)
                }
            };
            if !ty.is_null() {
                msg!(array, "addObject:", (ty: Id) -> ());
            }
        }
        if msg!(array, "count", () -> usize) == 0 {
            let s = nsstring("public.data");
            let ty = msg!(ut, "typeWithIdentifier:", (s: Id) -> Id);
            msg!(array, "addObject:", (ty: Id) -> ());
        }
        let picker = msg!(class("UIDocumentPickerViewController"), "alloc", () -> Id);
        let copy = true;
        let picker = msg!(picker, "initForOpeningContentTypes:asCopy:",
            (array: Id, copy: bool) -> Id);
        let no = false;
        if !picker.is_null() {
            msg!(picker, "setAllowsMultipleSelection:", (no: bool) -> ());
        }
        present(id, picker, Kind::Open)
    })
}

/// Let the user pick a folder, for dialog request `id`. `false` if no
/// picker could be shown.
pub(crate) fn open_folder(id: u32) -> bool {
    with_pool(|| unsafe {
        let ut = class("UTType");
        if ut.is_null() {
            return false;
        }
        let s = nsstring("public.folder");
        let folder = msg!(ut, "typeWithIdentifier:", (s: Id) -> Id);
        if folder.is_null() {
            return false;
        }
        let array = msg!(class("NSArray"), "arrayWithObject:", (folder: Id) -> Id);
        let picker = msg!(class("UIDocumentPickerViewController"), "alloc", () -> Id);
        let copy = false;
        let picker = msg!(picker, "initForOpeningContentTypes:asCopy:",
            (array: Id, copy: bool) -> Id);
        let no = false;
        if !picker.is_null() {
            msg!(picker, "setAllowsMultipleSelection:", (no: bool) -> ());
        }
        present(id, picker, Kind::Folder)
    })
}

/// A picked folder's URL, retained, with its security-scoped access (when
/// it has one: a folder in the app's own container needs none) stopped and
/// the URL released when dropped.
pub(crate) struct Scope {
    url: Id,
    accessing: bool,
}

// NSURL is immutable and its scoped-access calls are thread-safe.
unsafe impl Send for Scope {}
unsafe impl Sync for Scope {}

impl Drop for Scope {
    fn drop(&mut self) {
        unsafe {
            if self.accessing {
                msg!(self.url, "stopAccessingSecurityScopedResource", () -> ());
            }
            msg!(self.url, "release", () -> ());
        }
    }
}

/// The folder at `url`, its security-scoped access started.
unsafe fn scoped_folder(url: Id) -> Option<super::Folder> {
    unsafe {
        let path = PathBuf::from(rust_string(msg!(url, "path", () -> Id))?);
        let accessing = msg!(url, "startAccessingSecurityScopedResource", () -> bool);
        msg!(url, "retain", () -> Id);
        debug(format_args!(
            "picker: folder {} (scoped access {accessing})",
            path.display()
        ));
        Some(super::Folder::scoped(path, Scope { url, accessing }))
    }
}

/// Let the user choose where to save `bytes` as `name`, for dialog request
/// `id`. `false` if no picker could be shown.
pub(crate) fn export(id: u32, name: &str, bytes: &[u8]) -> bool {
    let dir = std::env::temp_dir().join(format!("stipple-export-{}-{id}", std::process::id()));
    let path = dir.join(name);
    if std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&path, bytes))
        .is_err()
    {
        let _ = std::fs::remove_dir_all(&dir);
        return false;
    }
    let shown = with_pool(|| unsafe {
        let s = nsstring(&path.to_string_lossy());
        let url = msg!(class("NSURL"), "fileURLWithPath:", (s: Id) -> Id);
        let urls = msg!(class("NSArray"), "arrayWithObject:", (url: Id) -> Id);
        let picker = msg!(class("UIDocumentPickerViewController"), "alloc", () -> Id);
        let copy = true;
        let picker = msg!(picker, "initForExportingURLs:asCopy:", (urls: Id, copy: bool) -> Id);
        present(id, picker, Kind::Export(dir.clone()))
    });
    if !shown {
        let _ = std::fs::remove_dir_all(&dir);
    }
    shown
}

/// The view controller to present from: the app's window's root (the key
/// window's, without the app's view), or what it presents, all the way up.
/// The app's own window, as the key window may be none (or another of the
/// app's) for a moment, as one presentation leaves and the next comes.
pub(crate) unsafe fn presenter() -> Id {
    unsafe {
        let view = crate::platform::backend::ios::app_view();
        let mut window = if view.is_null() {
            std::ptr::null_mut()
        } else {
            msg!(view, "window", () -> Id)
        };
        if window.is_null() {
            let app = msg!(class("UIApplication"), "sharedApplication", () -> Id);
            if !app.is_null() {
                window = msg!(app, "keyWindow", () -> Id);
            }
        }
        if window.is_null() {
            return std::ptr::null_mut();
        }
        let mut top = msg!(window, "rootViewController", () -> Id);
        while !top.is_null() {
            let next = msg!(top, "presentedViewController", () -> Id);
            if next.is_null() {
                break;
            }
            top = next;
        }
        top
    }
}

/// Present `picker` (owned: released here if it can't be shown) with our
/// delegate, and remember it.
unsafe fn present(id: u32, picker: Id, kind: Kind) -> bool {
    if picker.is_null() {
        return false;
    }
    unsafe {
        if presenter().is_null() {
            msg!(picker, "release", () -> ());
            return false;
        }
        let delegate = delegate();
        msg!(picker, "setDelegate:", (delegate: Id) -> ());
    }
    PENDING.with(|p| p.borrow_mut().push(Pending { id, picker, kind }));
    debug(format_args!("picker: request {id} asked"));
    show(picker, 0);
    true
}

/// Present `picker` from the top controller once nothing is mid-transition:
/// UIKit drops a presentation asked for while another is still being
/// presented or dismissed, with only a log line. Tries again every 50 ms,
/// for up to 5 s, then presents all the same (or, with nothing to present
/// from, answers the request as cancelled). (A document picker's own
/// presentation may then be delayed by UIKit until its remote view service
/// is up; see [`asked`].)
fn show(picker: Id, tries: u32) {
    if !PENDING.with(|p| p.borrow().iter().any(|x| x.picker == picker)) {
        return;
    }
    let last = tries >= 100;
    /// What came of a try.
    enum Try {
        Again,
        Presented,
        Nowhere,
    }
    let tried = with_pool(|| unsafe {
        let root = presenter();
        if root.is_null() {
            debug(format_args!(
                "picker: nothing to present from (try {tries})"
            ));
            return if last { Try::Nowhere } else { Try::Again };
        }
        let coordinator = !msg!(root, "transitionCoordinator", () -> Id).is_null();
        let presenting = msg!(root, "isBeingPresented", () -> bool);
        let dismissing = msg!(root, "isBeingDismissed", () -> bool);
        if (coordinator || presenting || dismissing) && !last {
            if tries.is_multiple_of(10) {
                debug(format_args!(
                    "picker: waiting on {} (try {tries}: transition={coordinator} \
                     presented={presenting} dismissed={dismissing})",
                    class_name(root)
                ));
            }
            return Try::Again;
        }
        debug(format_args!(
            "picker: presenting from {} (try {tries})",
            class_name(root)
        ));
        let animated = true;
        let none: Id = std::ptr::null_mut();
        msg!(root, "presentViewController:animated:completion:",
            (picker: Id, animated: bool, none: Id) -> ());
        ASKED.with(|a| a.borrow_mut().push(picker));
        Try::Presented
    });
    match tried {
        Try::Again => after_ms(50, move || show(picker, tries + 1)),
        Try::Presented => {}
        // Never shown: the request is answered, not left waiting forever.
        Try::Nowhere => give_up(picker),
    }
}

/// A picker that couldn't be presented: its request answered as cancelled.
fn give_up(picker: Id) {
    let Some(pending) = take(picker) else {
        return;
    };
    debug(format_args!("picker: request {} not shown", pending.id));
    unsafe { msg!(picker, "release", () -> ()) };
    match pending.kind {
        Kind::Open => mobile::finish_open(pending.id, None),
        Kind::Folder => mobile::finish_folder(pending.id, None),
        Kind::Export(dir) => {
            let _ = std::fs::remove_dir_all(dir);
            mobile::finish_save(pending.id, false);
        }
    }
}

fn debug(args: std::fmt::Arguments<'_>) {
    crate::platform::window::debug_window(args);
}

/// `obj`'s class name, for diagnostics.
unsafe fn class_name(obj: Id) -> String {
    if obj.is_null() {
        return "nil".into();
    }
    let name = unsafe { object_getClassName(obj) };
    if name.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

thread_local! {
    /// Pickers handed to `presentViewController:`, newest last.
    static ASKED: RefCell<Vec<Id>> = const { RefCell::new(Vec::new()) };
}

/// The newest picker of ours that UIKit was asked to present but isn't on
/// top: its presentation is pending (UIKit holds a document picker back
/// until its remote view service has loaded, which on a simulator can take
/// a long while, or not happen, right after another picker was dismissed).
fn asked() -> Option<Id> {
    ASKED.with(|a| {
        a.borrow_mut()
            .retain(|&x| PENDING.with(|p| p.borrow().iter().any(|q| q.picker == x)));
        a.borrow().last().copied()
    })
}

/// Run `f` on the main queue in `ms` milliseconds.
fn after_ms(ms: i64, f: impl FnOnce() + 'static) {
    extern "C" fn trampoline(context: *mut c_void) {
        let f = unsafe { Box::from_raw(context.cast::<Box<dyn FnOnce()>>()) };
        f();
    }
    // libdispatch, part of libSystem; the main queue is a macro over the
    // symbol.
    unsafe extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_time(when: u64, delta_ns: i64) -> u64;
        fn dispatch_after_f(
            when: u64,
            queue: *const c_void,
            context: *mut c_void,
            work: extern "C" fn(*mut c_void),
        );
    }
    let f: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    unsafe {
        dispatch_after_f(
            dispatch_time(0, ms * 1_000_000),
            (&raw const _dispatch_main_q).cast(),
            Box::into_raw(f).cast(),
            trampoline,
        )
    };
}

/// The request `picker` was shown for, no longer pending.
fn take(picker: Id) -> Option<Pending> {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let i = p.iter().position(|x| x.picker == picker)?;
        Some(p.remove(i))
    })
}

/// `documentPicker:didPickDocumentsAtURLs:`.
extern "C" fn did_pick(_this: Id, _cmd: Sel, picker: Id, urls: Id) {
    let Some(pending) = take(picker) else {
        return;
    };
    let url = if urls.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe { msg!(urls, "firstObject", () -> Id) }
    };
    let path = (!url.is_null())
        .then(|| rust_string(unsafe { msg!(url, "path", () -> Id) }))
        .flatten()
        .map(PathBuf::from);
    let folder = match pending.kind {
        Kind::Folder if !url.is_null() => unsafe { scoped_folder(url) },
        _ => None,
    };
    unsafe { msg!(picker, "release", () -> ()) };
    match pending.kind {
        Kind::Folder => mobile::finish_folder(pending.id, folder),
        Kind::Open => {
            let file = path.and_then(|path| {
                let bytes = std::fs::read(&path).ok();
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // Our copy (asCopy), in the app's tmp directory.
                let _ = std::fs::remove_file(&path);
                Some(super::FileContents {
                    name,
                    bytes: bytes?,
                })
            });
            mobile::finish_open(pending.id, file);
        }
        Kind::Export(dir) => {
            let _ = std::fs::remove_dir_all(dir);
            mobile::finish_save(pending.id, true);
        }
    }
}

/// `documentPickerWasCancelled:`.
extern "C" fn cancelled(_this: Id, _cmd: Sel, picker: Id) {
    let Some(pending) = take(picker) else {
        return;
    };
    unsafe { msg!(picker, "release", () -> ()) };
    match pending.kind {
        Kind::Open => mobile::finish_open(pending.id, None),
        Kind::Folder => mobile::finish_folder(pending.id, None),
        Kind::Export(dir) => {
            let _ = std::fs::remove_dir_all(dir);
            mobile::finish_save(pending.id, false);
        }
    }
}

/// The pickers' delegate, made once (a picker holds its delegate weakly).
fn delegate() -> Id {
    static DELEGATE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *DELEGATE.get_or_init(|| unsafe {
        let name = c"StippleDocumentPickerDelegate";
        let mut cls = objc_getClass(name.as_ptr());
        if cls.is_null() {
            cls = objc_allocateClassPair(class("NSObject"), name.as_ptr(), 0);
            class_addMethod(
                cls,
                crate::platform::native::objc::sel("documentPicker:didPickDocumentsAtURLs:"),
                did_pick as *const c_void,
                c"v@:@@".as_ptr(),
            );
            class_addMethod(
                cls,
                crate::platform::native::objc::sel("documentPickerWasCancelled:"),
                cancelled as *const c_void,
                c"v@:@".as_ptr(),
            );
            let protocol = objc_getProtocol(c"UIDocumentPickerDelegate".as_ptr());
            if !protocol.is_null() {
                class_addProtocol(cls, protocol);
            }
            objc_registerClassPair(cls);
        }
        msg!(msg!(cls, "alloc", () -> Id), "init", () -> Id) as usize
    }) as Id
}

/// The class name of the view controller on top (presented last), or `""`;
/// for self-tests.
pub(crate) fn presented_class() -> String {
    with_pool(|| unsafe {
        let top = presenter();
        if top.is_null() {
            return String::new();
        }
        let ours = PENDING.with(|p| p.borrow().iter().any(|x| x.picker == top));
        if !ours && asked().is_some() {
            return "UIDocumentPickerViewController (presentation delayed)".into();
        }
        let name = object_getClassName(top);
        if name.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(name)
            .to_string_lossy()
            .into_owned()
    })
}

/// Close the document picker on top as its Cancel button does: dismiss it
/// and tell its delegate (`documentPickerWasCancelled:`). `false` if no
/// picker of ours is on top. For self-tests, which can't tap the picker
/// (its contents run in another process).
pub(crate) fn cancel_presented() -> bool {
    answer_presented(None)
}

/// Close the document picker on top as choosing `path` in it does: dismiss
/// it and hand its delegate the file's URL
/// (`documentPicker:didPickDocumentsAtURLs:`), as UIKit does once the user
/// picked a document (for an open, a copy of it in the app's tmp directory,
/// which the delegate reads and deletes) or a place to export to. `false` if
/// no picker of ours is on top. For self-tests.
pub(crate) fn pick_presented(path: &str) -> bool {
    answer_presented(Some(path))
}

/// Dismiss our picker on top and tell its delegate it picked `path`, or was
/// cancelled.
fn answer_presented(path: Option<&str>) -> bool {
    with_pool(|| unsafe {
        let mut top = presenter();
        let ours = !top.is_null() && PENDING.with(|p| p.borrow().iter().any(|x| x.picker == top));
        let waiting = asked();
        debug(format_args!(
            "picker: answering {path:?}: on top {} (ours {ours}), asked {}, pending {}",
            class_name(top),
            waiting.is_some(),
            PENDING.with(|p| p.borrow().len())
        ));
        if !ours {
            // A picker whose presentation UIKit still holds back: answer it
            // all the same (dismissing it cancels the pending presentation).
            match waiting {
                Some(picker) => top = picker,
                None => return false,
            }
        }
        msg!(top, "retain", () -> Id);
        let animated = false;
        let none: Id = std::ptr::null_mut();
        msg!(top, "dismissViewControllerAnimated:completion:", (animated: bool, none: Id) -> ());
        let delegate = msg!(top, "delegate", () -> Id);
        if !delegate.is_null() {
            match path {
                Some(path) => {
                    let s = nsstring(path);
                    let url = msg!(class("NSURL"), "fileURLWithPath:", (s: Id) -> Id);
                    let urls = msg!(class("NSArray"), "arrayWithObject:", (url: Id) -> Id);
                    msg!(delegate, "documentPicker:didPickDocumentsAtURLs:",
                        (top: Id, urls: Id) -> ());
                }
                None => msg!(delegate, "documentPickerWasCancelled:", (top: Id) -> ()),
            }
        }
        msg!(top, "release", () -> ());
        true
    })
}
