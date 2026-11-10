//! Input methods through the Text Services Framework (TSF), with IMM32
//! ([`super::ime`]) as the fallback.
//!
//! Modern input methods are TSF text services: the Microsoft Japanese,
//! Chinese and Korean IMEs, handwriting, voice typing and the touch
//! keyboard's suggestions. IMM32 reaches them only through its compatibility
//! layer; a TSF client talks to them directly. All of it is hand-written COM,
//! as for UI Automation and OLE drag and drop:
//!
//! - *The thread.* The first time a window reports its IME state, the thread
//!   becomes a single-threaded apartment, a thread manager
//!   (`CLSID_TF_ThreadMgr`) is created and activated (`ITfThreadMgr::Activate`
//!   gives our client id), and an empty document manager is made for windows
//!   with no text field focused. If any of that fails, or `STIPPLE_TSF=0` is
//!   set, IMM32 is used instead, as before.
//! - *Each window* gets a document manager with one context, created over our
//!   text store ([`store::Store`]: `ITextStoreACP`,
//!   `ITfContextOwnerCompositionSink`, and an `ITfTextEditSink` advised on the
//!   context to read display attributes). While a text field (or an
//!   `ime_input` element) has focus, `ITfThreadMgr::AssociateFocus` ties the
//!   window to that document manager, so TSF focuses it whenever the window
//!   has the keyboard; otherwise the window is tied to the empty one, which
//!   leaves input methods nothing to compose into.
//! - *The document* is the field's text and selection from `Window::set_ime`,
//!   and the caret rectangle answers `GetTextExt`, so candidate windows go by
//!   the caret. What the input method writes becomes `Event::Ime`: see
//!   [`crate::platform::text_store`].
//!
//! Keys reach text services through the hooks TSF installs on activation, so
//! the message loop is unchanged. `STIPPLE_DEBUG_TSF=1` logs what happens.

mod com;
mod selftest;
mod store;

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;

use com::{Com, Hresult, IID_ITFSOURCE, IID_ITFTEXTEDITSINK, IID_ITFTHREADMGR, out};
use store::Store;

use super::Hwnd;
use crate::platform::ImeState;
use crate::platform::text_store::AppText;

type P = *mut c_void;
type Out = *mut *mut c_void;

/// `TF_POPF_ALL`: pop every context.
const TF_POPF_ALL: u32 = 0x1;

/// The activated thread manager.
#[derive(Debug)]
struct Thread {
    mgr: Com,
    client: u32,
    /// A document manager with no context, for windows without a text field.
    empty: Com,
}

/// One window's document manager, context and store.
#[derive(Debug)]
struct Doc {
    dim: Com,
    context: Com,
    /// Our reference on the store (its `ITextStoreACP` pointer).
    store: Com,
    /// The text edit sink's cookie (`ITfSource::AdviseSink`).
    edit_cookie: Option<u32>,
    /// Whether the window is tied to this document manager (a field has
    /// focus) rather than the empty one.
    focused: Cell<bool>,
}

impl Doc {
    fn store(&self) -> &Store {
        unsafe { &*(self.store.ptr() as *const Store) }
    }
}

thread_local! {
    static THREAD: OnceCell<Option<Rc<Thread>>> = const { OnceCell::new() };
    static DOCS: RefCell<HashMap<usize, Rc<Doc>>> = RefCell::new(HashMap::new());
}

/// Log with `STIPPLE_DEBUG_TSF` (or during the self-test).
pub(super) fn debug(args: std::fmt::Arguments<'_>) {
    thread_local! {
        static ON: bool = std::env::var_os("STIPPLE_DEBUG_TSF").is_some_and(|v| v != "0")
            || selftest::enabled();
    }
    if ON.with(|on| *on) {
        eprintln!("stipple-tsf: {args}");
    }
}

thread_local! {
    /// TSF went wrong on this thread: it is switched off at the next
    /// `set_state` (not from inside a TSF callback), and IMM32 takes over.
    static BROKEN: Cell<bool> = const { Cell::new(false) };
    /// It has been switched off.
    static OFF: Cell<bool> = const { Cell::new(false) };
}

/// Something in the TSF path failed in a way that could lose typing: give
/// up on TSF for the rest of the run.
pub(super) fn fail(why: &str) {
    if !BROKEN.with(|b| b.replace(true)) {
        eprintln!("stipple-tsf: {why}; switching to IMM32");
    }
}

/// Switch TSF off after [`fail`]: untie every window from its document and
/// deactivate the thread manager, so input methods reach the IMM32 path
/// through Windows' compatibility layer.
fn shut_down() {
    if OFF.with(|o| o.replace(true)) {
        return;
    }
    let docs: Vec<(usize, Rc<Doc>)> = DOCS.with(|d| d.borrow_mut().drain().collect());
    let t = THREAD.with(|t| t.get().cloned().flatten());
    for (hwnd, doc) in docs {
        if let Some(t) = &t {
            associate(t, hwnd as Hwnd, None);
        }
        release_doc(doc);
    }
    if let Some(t) = t {
        // ITfThreadMgr::Deactivate.
        unsafe { t.mgr.call::<unsafe extern "system" fn(P) -> Hresult>(4)(t.mgr.ptr()) };
    }
    debug(format_args!("off: using IMM32"));
}

/// The thread manager, activated on first use; `None` means IMM32.
fn thread() -> Option<Rc<Thread>> {
    if OFF.with(|o| o.get()) {
        return None;
    }
    THREAD.with(|t| t.get_or_init(activate).clone())
}

fn activate() -> Option<Rc<Thread>> {
    if std::env::var_os("STIPPLE_TSF").is_some_and(|v| v == "0") {
        debug(format_args!("off (STIPPLE_TSF=0): using IMM32"));
        return None;
    }
    if !com::init_apartment() {
        debug(format_args!("off (the thread is not an STA): using IMM32"));
        return None;
    }
    let mgr = match Com::create(&com::CLSID_TF_THREADMGR, &IID_ITFTHREADMGR) {
        Ok(m) => m,
        Err(hr) => {
            debug(format_args!(
                "off (no thread manager: {hr:#x}): using IMM32"
            ));
            return None;
        }
    };
    let mut client = 0u32;
    // ITfThreadMgr::Activate.
    let hr = unsafe {
        mgr.call::<unsafe extern "system" fn(P, *mut u32) -> Hresult>(3)(mgr.ptr(), &mut client)
    };
    if hr < 0 {
        debug(format_args!("off (Activate: {hr:#x}): using IMM32"));
        return None;
    }
    // ITfThreadMgr::CreateDocumentMgr.
    let Some(empty) = out(|p| unsafe {
        mgr.call::<unsafe extern "system" fn(P, Out) -> Hresult>(5)(mgr.ptr(), p)
    }) else {
        unsafe { mgr.call::<unsafe extern "system" fn(P) -> Hresult>(4)(mgr.ptr()) };
        debug(format_args!("off (CreateDocumentMgr failed): using IMM32"));
        return None;
    };
    debug(format_args!("active (client id {client})"));
    Some(Rc::new(Thread { mgr, client, empty }))
}

/// Tie `hwnd` to `dim` (`ITfThreadMgr::AssociateFocus`).
fn associate(t: &Thread, hwnd: Hwnd, dim: Option<&Com>) {
    let mut prev: P = std::ptr::null_mut();
    let hr = unsafe {
        t.mgr
            .call::<unsafe extern "system" fn(P, Hwnd, P, Out) -> Hresult>(9)(
            t.mgr.ptr(),
            hwnd,
            dim.map_or(std::ptr::null_mut(), Com::ptr),
            &mut prev,
        )
    };
    drop(Com::from_raw(prev));
    if hr < 0 && dim.is_some() {
        debug(format_args!("AssociateFocus: {hr:#x}"));
        fail("AssociateFocus failed");
    }
}

/// A document manager with a context over a new store for `hwnd`.
fn create_doc(t: &Thread, hwnd: Hwnd) -> Option<Doc> {
    let mgr = &t.mgr;
    let dim = out(|p| unsafe {
        mgr.call::<unsafe extern "system" fn(P, Out) -> Hresult>(5)(mgr.ptr(), p)
    })?;
    let store = Store::create(hwnd);
    let store_ref = Com::from_raw(Store::unknown(store))?;
    let mut ec = 0u32;
    // ITfDocumentMgr::CreateContext.
    let context = out(|p| unsafe {
        dim.call::<unsafe extern "system" fn(P, u32, u32, P, Out, *mut u32) -> Hresult>(3)(
            dim.ptr(),
            t.client,
            0,
            store_ref.ptr(),
            p,
            &mut ec,
        )
    })?;
    // ITfDocumentMgr::Push.
    let hr = unsafe {
        dim.call::<unsafe extern "system" fn(P, P) -> Hresult>(4)(dim.ptr(), context.ptr())
    };
    if hr < 0 {
        debug(format_args!("Push failed: {hr:#x}"));
        return None;
    }
    unsafe { (*store).set_context(Com::add_ref(context.ptr())) };
    // The text edit sink, for display attributes.
    let edit_cookie =
        context.query(&IID_ITFSOURCE).and_then(|source| {
            let mut cookie = 0u32;
            // ITfSource::AdviseSink.
            let hr = unsafe {
                source
                    .call::<unsafe extern "system" fn(P, *const com::Guid, P, *mut u32) -> Hresult>(
                        3,
                    )(
                    source.ptr(),
                    &IID_ITFTEXTEDITSINK,
                    Store::edit_sink(store),
                    &mut cookie,
                )
            };
            (hr >= 0).then_some(cookie)
        });
    debug(format_args!(
        "document for {:#x} (edit sink: {})",
        hwnd as usize,
        edit_cookie.is_some()
    ));
    Some(Doc {
        dim,
        context,
        store: store_ref,
        edit_cookie,
        focused: Cell::new(false),
    })
}

/// `hwnd`'s document, created on first use.
fn doc(t: &Thread, hwnd: Hwnd) -> Option<Rc<Doc>> {
    if let Some(d) = DOCS.with(|d| d.borrow().get(&(hwnd as usize)).cloned()) {
        return Some(d);
    }
    let d = Rc::new(create_doc(t, hwnd)?);
    DOCS.with(|docs| docs.borrow_mut().insert(hwnd as usize, d.clone()));
    Some(d)
}

/// Show input methods the focused field, or none (`Window::set_ime`).
/// Returns `false` when TSF isn't in use, for IMM32 to handle it.
pub(super) fn set_state(hwnd: Hwnd, state: Option<&ImeState>) -> bool {
    if BROKEN.with(|b| b.get()) {
        shut_down();
        return false;
    }
    let handled =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| try_set_state(hwnd, state)))
            .unwrap_or_else(|_| {
                fail("set_ime panicked");
                false
            });
    // If anything failed on the way, this report goes to IMM32 too.
    if BROKEN.with(|b| b.get()) {
        shut_down();
        return false;
    }
    handled
}

fn try_set_state(hwnd: Hwnd, state: Option<&ImeState>) -> bool {
    let Some(t) = thread() else {
        return false;
    };
    let Some(doc) = doc(&t, hwnd) else {
        return false;
    };
    match state {
        Some(state) => {
            let r = super::ime::caret_rect(state);
            let store = doc.store();
            store.set_caret((r.left, r.top, r.right, r.bottom));
            store.report(AppText::from_state(
                &state.surrounding,
                state.cursor,
                state.anchor,
            ));
            if !doc.focused.replace(true) {
                associate(&t, hwnd, Some(&doc.dim));
                debug(format_args!("focus {:#x}", hwnd as usize));
            }
            selftest::on_focus(hwnd, &doc.context, t.client, &t.mgr, &doc.dim);
        }
        None => {
            if doc.focused.replace(false) {
                // A composition left mid-way is dropped, as the App dropped
                // its preedit.
                doc.store().leave();
                associate(&t, hwnd, Some(&t.empty));
                debug(format_args!("unfocus {:#x}", hwnd as usize));
            }
        }
    }
    true
}

/// `hwnd` is being destroyed: let go of its document.
pub(super) fn forget(hwnd: Hwnd) {
    let Some(doc) = DOCS.with(|d| d.borrow_mut().remove(&(hwnd as usize))) else {
        return;
    };
    if let Some(t) = thread() {
        associate(&t, hwnd, None);
    }
    release_doc(doc);
}

/// Unadvise and pop a document, and let go of it.
fn release_doc(doc: Rc<Doc>) {
    if let (Some(cookie), Some(source)) = (doc.edit_cookie, doc.context.query(&IID_ITFSOURCE)) {
        // ITfSource::UnadviseSink.
        unsafe {
            source.call::<unsafe extern "system" fn(P, u32) -> Hresult>(4)(source.ptr(), cookie)
        };
    }
    // ITfDocumentMgr::Pop.
    unsafe {
        doc.dim
            .call::<unsafe extern "system" fn(P, u32) -> Hresult>(5)(
            doc.dim.ptr(), TF_POPF_ALL
        )
    };
    // The store holds the context: break the cycle.
    doc.store().set_context(None);
    drop(doc);
}
