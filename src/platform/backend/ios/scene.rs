//! The scene the app's window lives in (`UIWindowScene`), and what its
//! session means for the saved state.
//!
//! An app whose `Info.plist` has a `UIApplicationSceneManifest` runs under
//! UIKit's scene lifecycle: the app delegate names the scene delegate
//! (`application:configurationForConnectingSceneSession:options:`, so the
//! manifest needs no configurations of its own), whose
//! `scene:willConnectToSession:options:` builds the window on the scene. Without
//! the manifest, `application:didFinishLaunchingWithOptions:` builds it, as
//! before scenes.
//!
//! The session is what tells the two ways an app ends in the background
//! apart. When iOS ends it to free memory, the next launch reconnects the
//! same `UISceneSession` (its `persistentIdentifier` unchanged), and the
//! state saved under it comes back. When the user swipes it away in the app
//! switcher, UIKit destroys the session: it calls
//! `application:didDiscardSceneSessions:` (at the next launch, the app being
//! gone by then), which deletes the state saved under it, and the launch
//! connects a new session, which wouldn't take it anyway (see
//! [`saved_state`](super::saved_state)).
//!
//! UIKit may give that new session the old one's identifier, though: an
//! iPad simulator does, and only says the old one was discarded after the
//! scene connected, so a discard naming the connected session also takes
//! back the state restored on connecting, if the app hasn't had it yet. And
//! when the app asks UIKit to destroy its own session
//! (`requestSceneSessionDestruction:`), the state goes right then and no
//! more is saved under it: UIKit takes the request in its own time, the app
//! saves as the scene goes to the background, and an iPad simulator's next
//! launch connected the same identifier again without a word of a discard.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use super::{
    Class, Id, Sel, class, class_addMethod, class_addProtocol, msg_id, nsstring, objc_getProtocol,
    objc_msgSend, objc_registerClassPair, sel, set_objects, string_of,
};

thread_local! {
    /// The connected scene's session (retained), and its persistent
    /// identifier; none before a scene connects, or without scenes.
    static SESSION: RefCell<Option<(Id, String)>> = const { RefCell::new(None) };
    /// Whether the scene connected with saved state, handed to the app.
    static RESTORED: Cell<bool> = const { Cell::new(false) };
}

fn debug(args: std::fmt::Arguments<'_>) {
    crate::platform::window::debug_window(format_args!("saved state: {args}"));
}

/// Whether the app runs under the scene lifecycle: its `Info.plist` has a
/// `UIApplicationSceneManifest`.
pub(super) fn uses_scenes() -> bool {
    static USES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *USES.get_or_init(read_manifest)
}

fn read_manifest() -> bool {
    unsafe {
        let bundle = msg_id(class("NSBundle"), sel("mainBundle"));
        if bundle.is_null() {
            return false;
        }
        let f: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        !f(
            bundle,
            sel("objectForInfoDictionaryKey:"),
            nsstring("UIApplicationSceneManifest"),
        )
        .is_null()
    }
}

/// The session the state is saved under: the connected scene's persistent
/// identifier, or empty without scenes.
pub(super) fn current_session() -> String {
    SESSION.with(|s| {
        s.borrow()
            .as_ref()
            .map(|(_, id)| id.clone())
            .unwrap_or_default()
    })
}

/// Hand the state saved under `session` to the app's first event, deleting
/// the file whichever session saved it.
pub(super) fn restore_for_session(session: &str) {
    let Some(dir) = super::saved_state::dir() else {
        return;
    };
    let saved_by = super::saved_state::stored_session(&dir);
    let bytes = super::saved_state::take(&dir, session);
    match (&saved_by, &bytes) {
        (None, _) => debug(format_args!("none to restore in session {session:?}")),
        (Some(_), Some(b)) => debug(format_args!(
            "restoring {} bytes saved in session {session:?}",
            b.len()
        )),
        (Some(by), None) => debug(format_args!(
            "dropped the state saved in session {by:?}: this is session {session:?}"
        )),
    }
    RESTORED.with(|r| r.set(bytes.is_some()));
    crate::platform::saved_state::set_restored_state(bytes);
}

/// `application:configurationForConnectingSceneSession:options:`: every
/// scene is a window scene with our delegate.
extern "C" fn configuration_for_connecting(
    _this: Id,
    _cmd: Sel,
    _app: Id,
    session: Id,
    _options: Id,
) -> Id {
    unsafe {
        let role = msg_id(session, sel("role"));
        let init: unsafe extern "C" fn(Id, Sel, Id, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let config = init(
            msg_id(class("UISceneConfiguration"), sel("alloc")),
            sel("initWithName:sessionRole:"),
            nsstring("Default Configuration"),
            role,
        );
        let set: unsafe extern "C" fn(Id, Sel, Class) =
            std::mem::transmute(objc_msgSend as *const c_void);
        set(
            config,
            sel("setDelegateClass:"),
            class("StippleSceneDelegate"),
        );
        msg_id(config, sel("autorelease"))
    }
}

/// `application:didDiscardSceneSessions:`: the user closed these sessions
/// (swiped the app away); their saved state goes. UIKit may name the
/// identifier the scene just connected with, reused for a new session
/// after the old one was destroyed: what was restored from the old one goes
/// too, if the app hasn't taken it yet.
extern "C" fn did_discard_sessions(_this: Id, _cmd: Sel, _app: Id, sessions: Id) {
    let ids: Vec<String> = unsafe {
        set_objects(sessions)
            .into_iter()
            .map(|s| string_of(msg_id(s, sel("persistentIdentifier"))))
            .collect()
    };
    let dropped =
        super::saved_state::dir().is_some_and(|dir| super::saved_state::discard(&dir, &ids));
    let connected = current_session();
    let named = super::saved_state::names_connected(&ids, &connected);
    let withdrawn = named && crate::platform::saved_state::take_restored_state().is_some();
    let restored = RESTORED.with(Cell::get);
    debug(format_args!(
        "sessions {ids:?} discarded{}{}",
        if dropped { ", their state deleted" } else { "" },
        if !named {
            String::new()
        } else {
            format!(
                "; the connected session {connected:?} has their identifier: {}",
                match (restored, withdrawn) {
                    (_, true) => "the state restored from it withdrawn",
                    (true, false) => "the state restored from it already went to the app",
                    (false, false) => "nothing was restored from it",
                }
            )
        }
    ));
}

/// `scene:willConnectToSession:options:`: build the window on the scene,
/// with the state its session saved, if any.
extern "C" fn will_connect(_this: Id, _cmd: Sel, scene: Id, session: Id, _options: Id) {
    unsafe {
        let id = string_of(msg_id(session, sel("persistentIdentifier")));
        debug(format_args!("scene connected, session {id:?}"));
        // Connected (again): a live session, whose state is saved, even
        // after the app asked to destroy one of this identifier.
        super::saved_state::reopen(&id);
        let session = msg_id(session, sel("retain"));
        let old = SESSION.with(|s| s.borrow_mut().replace((session, id.clone())));
        if let Some((old, _)) = old {
            msg_id(old, sel("release"));
        }
        let window = super::CTX.with(|c| c.borrow().window);
        if !window.is_null() {
            // A scene of the running app reconnected: the window moves to
            // it, with the app as it is.
            super::msg_void_id(window, sel("setWindowScene:"), scene);
            super::msg_void(window, sel("makeKeyAndVisible"));
            super::Window::request_redraw(&super::current_window());
            return;
        }
        restore_for_session(&id);
        let init: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let window = init(
            msg_id(class("UIWindow"), sel("alloc")),
            sel("initWithWindowScene:"),
            scene,
        );
        super::show_window(window);
    }
}

/// Give the app delegate the scene methods, when the app uses scenes (an
/// app delegate answering them might be taken for one that does).
///
/// # Safety
/// `cls` must be a class pair still being built.
pub(super) unsafe fn add_app_delegate_methods(cls: Class) {
    if !uses_scenes() {
        return;
    }
    unsafe {
        class_addMethod(
            cls,
            sel("application:configurationForConnectingSceneSession:options:"),
            configuration_for_connecting as *const c_void,
            c"@@:@@@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("application:didDiscardSceneSessions:"),
            did_discard_sessions as *const c_void,
            c"v@:@@".as_ptr(),
        );
    }
}

/// Register `StippleSceneDelegate`, the scene delegate the configuration
/// names.
pub(super) fn register_scene_delegate() -> Class {
    let name = c"StippleSceneDelegate";
    unsafe {
        let existing = super::objc_getClass(name.as_ptr());
        if !existing.is_null() {
            return existing;
        }
        let cls = super::objc_allocateClassPair(class("UIResponder"), name.as_ptr(), 0);
        class_addMethod(
            cls,
            sel("scene:willConnectToSession:options:"),
            will_connect as *const c_void,
            c"v@:@@@".as_ptr(),
        );
        let protocol = objc_getProtocol(c"UIWindowSceneDelegate".as_ptr());
        if !protocol.is_null() {
            class_addProtocol(cls, protocol);
        }
        objc_registerClassPair(cls);
        cls
    }
}

/// For self-tests: ask UIKit to destroy the connected scene's session, as
/// the user swiping the app away in the app switcher does. The state saved
/// under it goes now, and no more is saved (see
/// [`close`](super::saved_state::close)): UIKit may never say it's done.
/// `failed` hears UIKit's error, if it refuses (the session then lives on,
/// and saves again). Returns the session's identifier, or `None` without a
/// scene.
pub(super) fn request_destruction(failed: fn(String)) -> Option<String> {
    let (session, id) = SESSION.with(|s| s.borrow().clone())?;
    *FAILED.lock().unwrap_or_else(|e| e.into_inner()) = Some((failed, id.clone()));
    unsafe {
        let app = msg_id(class("UIApplication"), sel("sharedApplication"));
        // UIKit only destroys a session where the app may have several
        // (an iPad, with `UIApplicationSupportsMultipleScenes`); elsewhere
        // it refuses, and the state stays.
        let multiple = super::msg_bool(app, sel("supportsMultipleScenes"));
        debug(format_args!("supportsMultipleScenes={multiple}"));
        if multiple {
            let dir = super::saved_state::dir();
            let deleted = super::saved_state::close(dir.as_deref(), &id);
            debug(format_args!(
                "closing session {id:?}: {}; saving nothing more for it",
                if deleted {
                    "deleted the state saved in it"
                } else {
                    "no state was saved in it"
                }
            ));
        }
        let block: *mut ErrorBlock = Box::leak(Box::new(ErrorBlock {
            isa: &raw const _NSConcreteGlobalBlock,
            flags: BLOCK_IS_GLOBAL,
            reserved: 0,
            invoke: destruction_failed,
            descriptor: &DESCRIPTOR,
        }));
        let f: unsafe extern "C" fn(Id, Sel, Id, Id, *mut ErrorBlock) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(
            app,
            sel("requestSceneSessionDestruction:options:errorHandler:"),
            session,
            std::ptr::null_mut(),
            block,
        );
    }
    debug(format_args!("asked to destroy session {id:?}"));
    Some(id)
}

/// Who hears that UIKit refused to destroy the session, and which session.
static FAILED: std::sync::Mutex<Option<Failed>> = std::sync::Mutex::new(None);

/// The callback for UIKit's refusal, and the session it was asked to destroy.
type Failed = (fn(String), String);

unsafe extern "C" {
    static _NSConcreteGlobalBlock: c_void;
}

const BLOCK_IS_GLOBAL: i32 = 1 << 28;

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

/// A block literal taking an `NSError *`.
#[repr(C)]
struct ErrorBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut ErrorBlock, Id),
    descriptor: *const BlockDescriptor,
}

static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<ErrorBlock>(),
};

unsafe extern "C" fn destruction_failed(_block: *mut ErrorBlock, error: Id) {
    let what = unsafe { string_of(msg_id(error, sel("description"))) };
    debug(format_args!("destroying the session failed: {what}"));
    // UIKit may call it on another thread: the callback is a plain `fn`.
    let failed = FAILED.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some((failed, id)) = failed {
        // The session lives on: its state is saved again.
        super::saved_state::reopen(&id);
        failed(what);
    }
}
