//! The text store TSF talks to: one COM object per window that is the
//! context's `ITextStoreACP`, its `ITfContextOwnerCompositionSink` and an
//! `ITfTextEditSink`, each a vtable pointer in [`Store`]. The document, the
//! selection, the locks and the composition are kept by the platform-neutral
//! [`TextStore`]; this file answers COM with it and turns what it decides
//! into `Event::Ime` and sink notifications.
//!
//! Every call runs on the UI thread, inside the message loop (TSF calls in
//! from its hooks and from our own calls into it), and none of them waits:
//! locks are granted at once or queued until the holder releases its lock.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use super::com::*;
use crate::platform::event::Event;
use crate::platform::text_store::{
    ActiveEnd, AppText, LockGrant, Selection, StoreError, Sync, TextChange, TextStore,
};

use super::super::{ClientToScreen, GetClientRect, Hwnd, Point, Rect, push_event_to};

/// `TS_STATUS`.
#[repr(C)]
struct TsStatus {
    dynamic: u32,
    statics: u32,
}

/// `TS_SELECTION_ACP`.
#[repr(C)]
struct TsSelection {
    start: i32,
    end: i32,
    /// `TS_SELECTIONSTYLE`: the active end and the interim-character flag.
    active: i32,
    interim: i32,
}

/// `TS_TEXTCHANGE`.
#[repr(C)]
struct TsTextChange {
    start: i32,
    old_end: i32,
    new_end: i32,
}

/// `TS_RUNINFO`.
#[repr(C)]
struct TsRunInfo {
    count: u32,
    kind: i32,
}

const TS_SS_NOHIDDENTEXT: u32 = 0x8;
const TS_AS_TEXT_CHANGE: u32 = 0x1;
const TS_AS_SEL_CHANGE: u32 = 0x2;
const TS_AS_LAYOUT_CHANGE: u32 = 0x4;
const TS_LC_CHANGE: i32 = 1;
const TS_IAS_QUERYONLY: u32 = 0x2;
const TS_DEFAULT_SELECTION: u32 = u32::MAX;
const TS_AE_START: i32 = 1;
const TS_AE_END: i32 = 2;

type H = Hresult;
type P = *mut c_void;

#[repr(C)]
struct UnknownSlots {
    query_interface: unsafe extern "system" fn(P, *const Guid, *mut P) -> H,
    add_ref: unsafe extern "system" fn(P) -> u32,
    release: unsafe extern "system" fn(P) -> u32,
}

/// `ITextStoreACP`.
#[repr(C)]
struct TextStoreVtbl {
    unknown: UnknownSlots,
    advise_sink: unsafe extern "system" fn(P, *const Guid, P, u32) -> H,
    unadvise_sink: unsafe extern "system" fn(P, P) -> H,
    request_lock: unsafe extern "system" fn(P, u32, *mut H) -> H,
    get_status: unsafe extern "system" fn(P, *mut TsStatus) -> H,
    query_insert: unsafe extern "system" fn(P, i32, i32, u32, *mut i32, *mut i32) -> H,
    get_selection: unsafe extern "system" fn(P, u32, u32, *mut TsSelection, *mut u32) -> H,
    set_selection: unsafe extern "system" fn(P, u32, *const TsSelection) -> H,
    #[allow(clippy::type_complexity)]
    get_text: unsafe extern "system" fn(
        P,
        i32,
        i32,
        *mut u16,
        u32,
        *mut u32,
        *mut TsRunInfo,
        u32,
        *mut u32,
        *mut i32,
    ) -> H,
    set_text: unsafe extern "system" fn(P, u32, i32, i32, *const u16, u32, *mut TsTextChange) -> H,
    get_formatted_text: unsafe extern "system" fn(P, i32, i32, *mut P) -> H,
    get_embedded: unsafe extern "system" fn(P, i32, *const Guid, *const Guid, *mut P) -> H,
    query_insert_embedded: unsafe extern "system" fn(P, *const Guid, P, *mut i32) -> H,
    insert_embedded: unsafe extern "system" fn(P, u32, i32, i32, P, *mut TsTextChange) -> H,
    insert_text_at_selection: unsafe extern "system" fn(
        P,
        u32,
        *const u16,
        u32,
        *mut i32,
        *mut i32,
        *mut TsTextChange,
    ) -> H,
    insert_embedded_at_selection:
        unsafe extern "system" fn(P, u32, P, *mut i32, *mut i32, *mut TsTextChange) -> H,
    request_supported_attrs: unsafe extern "system" fn(P, u32, u32, *const Guid) -> H,
    request_attrs_at_position: unsafe extern "system" fn(P, i32, u32, *const Guid, u32) -> H,
    request_attrs_transitioning_at_position:
        unsafe extern "system" fn(P, i32, u32, *const Guid, u32) -> H,
    #[allow(clippy::type_complexity)]
    find_next_attr_transition: unsafe extern "system" fn(
        P,
        i32,
        i32,
        u32,
        *const Guid,
        u32,
        *mut i32,
        *mut i32,
        *mut i32,
    ) -> H,
    retrieve_requested_attrs: unsafe extern "system" fn(P, u32, P, *mut u32) -> H,
    get_end_acp: unsafe extern "system" fn(P, *mut i32) -> H,
    get_active_view: unsafe extern "system" fn(P, *mut u32) -> H,
    get_acp_from_point: unsafe extern "system" fn(P, u32, *const Point, u32, *mut i32) -> H,
    get_text_ext: unsafe extern "system" fn(P, u32, i32, i32, *mut Rect, *mut i32) -> H,
    get_screen_ext: unsafe extern "system" fn(P, u32, *mut Rect) -> H,
    get_wnd: unsafe extern "system" fn(P, u32, *mut Hwnd) -> H,
}

/// `ITfContextOwnerCompositionSink`.
#[repr(C)]
struct OwnerVtbl {
    unknown: UnknownSlots,
    on_start_composition: unsafe extern "system" fn(P, P, *mut i32) -> H,
    on_update_composition: unsafe extern "system" fn(P, P, P) -> H,
    on_end_composition: unsafe extern "system" fn(P, P) -> H,
}

/// `ITfTextEditSink`.
#[repr(C)]
struct EditSinkVtbl {
    unknown: UnknownSlots,
    on_end_edit: unsafe extern "system" fn(P, P, u32, P) -> H,
}

/// The text store object. The interface pointers TSF holds point at its
/// three vtable fields.
#[repr(C)]
pub(super) struct Store {
    acp: &'static TextStoreVtbl,
    owner: &'static OwnerVtbl,
    edit: &'static EditSinkVtbl,
    refs: Cell<u32>,
    hwnd: Hwnd,
    core: RefCell<TextStore>,
    /// TSF's `ITextStoreACPSink`, and the changes it wants to hear of.
    sink: RefCell<Option<Com>>,
    mask: Cell<u32>,
    /// The composition in progress (`ITfCompositionView`).
    view: RefCell<Option<Com>>,
    /// The display attribute managers, when they could be created.
    attrs: Option<AttributeLookup>,
    /// The caret, in client-area physical pixels, for `GetTextExt`.
    caret: Cell<(i32, i32, i32, i32)>,
    /// The App's report that came while a lock was held, applied after.
    pending: RefCell<Option<AppText>>,
    /// While asking TSF to end a composition the App left.
    terminating: Cell<bool>,
    /// The context (`ITfContext`), to end compositions; set once created.
    context: RefCell<Option<Com>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("hwnd", &self.hwnd).finish()
    }
}

static ACP_VTBL: TextStoreVtbl = TextStoreVtbl {
    unknown: UnknownSlots {
        query_interface: acp_query_interface,
        add_ref: acp_add_ref,
        release: acp_release,
    },
    advise_sink,
    unadvise_sink,
    request_lock,
    get_status,
    query_insert,
    get_selection,
    set_selection,
    get_text,
    set_text,
    get_formatted_text,
    get_embedded,
    query_insert_embedded,
    insert_embedded,
    insert_text_at_selection,
    insert_embedded_at_selection,
    request_supported_attrs,
    request_attrs_at_position,
    request_attrs_transitioning_at_position: request_attrs_at_position,
    find_next_attr_transition,
    retrieve_requested_attrs,
    get_end_acp,
    get_active_view,
    get_acp_from_point,
    get_text_ext,
    get_screen_ext,
    get_wnd,
};

static OWNER_VTBL: OwnerVtbl = OwnerVtbl {
    unknown: UnknownSlots {
        query_interface: owner_query_interface,
        add_ref: owner_add_ref,
        release: owner_release,
    },
    on_start_composition,
    on_update_composition,
    on_end_composition,
};

static EDIT_VTBL: EditSinkVtbl = EditSinkVtbl {
    unknown: UnknownSlots {
        query_interface: edit_query_interface,
        add_ref: edit_add_ref,
        release: edit_release,
    },
    on_end_edit,
};

impl Store {
    /// A new store for `hwnd`, with one reference (the caller's).
    pub(super) fn create(hwnd: Hwnd) -> *mut Store {
        Box::into_raw(Box::new(Store {
            acp: &ACP_VTBL,
            owner: &OWNER_VTBL,
            edit: &EDIT_VTBL,
            refs: Cell::new(1),
            hwnd,
            core: RefCell::new(TextStore::new()),
            sink: RefCell::new(None),
            mask: Cell::new(0),
            view: RefCell::new(None),
            attrs: AttributeLookup::new(),
            caret: Cell::new((0, 0, 0, 0)),
            pending: RefCell::new(None),
            terminating: Cell::new(false),
            context: RefCell::new(None),
        }))
    }

    /// The `ITextStoreACP` (and `IUnknown`) pointer.
    pub(super) fn unknown(this: *mut Store) -> P {
        this.cast()
    }

    /// The `ITfTextEditSink` pointer.
    pub(super) fn edit_sink(this: *mut Store) -> P {
        unsafe {
            (this as *mut u8)
                .add(std::mem::offset_of!(Store, edit))
                .cast()
        }
    }

    pub(super) fn set_context(&self, context: Option<Com>) {
        *self.context.borrow_mut() = context;
    }

    pub(super) fn composing(&self) -> bool {
        self.core.borrow().composing()
    }

    /// Where the caret is, in client physical pixels; TSF hears of a move.
    pub(super) fn set_caret(&self, rect: (i32, i32, i32, i32)) {
        if self.caret.replace(rect) != rect && !self.core.borrow().locked() {
            self.notify(TS_AS_LAYOUT_CHANGE, |sink| unsafe {
                // ITextStoreACPSink::OnLayoutChange.
                sink.call::<unsafe extern "system" fn(P, i32, u32) -> H>(5)(
                    sink.ptr(),
                    TS_LC_CHANGE,
                    0,
                )
            });
        }
    }

    /// The App's field, as it now is (`Window::set_ime`; empty when no
    /// field has focus).
    pub(super) fn report(&self, app: AppText) {
        if self.core.borrow().locked() {
            *self.pending.borrow_mut() = Some(app);
            return;
        }
        let sync = self.core.borrow_mut().sync(&app);
        match sync {
            Sync::InStep | Sync::Stale => {}
            Sync::Selection => self.selection_changed(),
            Sync::Changed { composing } => {
                if composing {
                    self.terminate();
                }
                self.replace_document(app);
            }
        }
    }

    /// No field has focus any more: end any composition and empty the
    /// document.
    pub(super) fn leave(&self) {
        if self.composing() {
            self.terminate();
        }
        self.report(AppText::default());
    }

    /// Make `app` the document, and tell TSF.
    fn replace_document(&self, app: AppText) {
        let ((old, new), clear) = self.core.borrow_mut().reset(app);
        if let Some(e) = clear {
            push_event_to(self.hwnd, Event::Ime(e));
        }
        self.text_changed(TextChange {
            start: 0,
            old_end: old,
            new_end: new,
        });
        self.selection_changed();
    }

    /// End the composition in progress without committing it: the App has
    /// already dropped its preedit.
    pub(super) fn terminate(&self) {
        self.core.borrow_mut().discard();
        let services = self
            .context
            .borrow()
            .as_ref()
            .and_then(|c| c.query(&IID_ITFCONTEXTOWNERCOMPOSITIONSERVICES));
        if let Some(s) = services {
            self.terminating.set(true);
            // ITfContextOwnerCompositionServices::TerminateComposition(NULL):
            // every composition. The input method may lock and edit meanwhile.
            unsafe {
                s.call::<unsafe extern "system" fn(P, P) -> H>(7)(s.ptr(), std::ptr::null_mut())
            };
            self.terminating.set(false);
        }
        self.view.borrow_mut().take();
        // What the input method left behind isn't the App's: back to the
        // App's text.
        let app = self.core.borrow().app().clone();
        self.replace_document(app);
    }

    fn text_changed(&self, change: TextChange) {
        let c = TsTextChange {
            start: change.start as i32,
            old_end: change.old_end as i32,
            new_end: change.new_end as i32,
        };
        self.notify(TS_AS_TEXT_CHANGE, |sink| unsafe {
            // ITextStoreACPSink::OnTextChange.
            sink.call::<unsafe extern "system" fn(P, u32, *const TsTextChange) -> H>(3)(
                sink.ptr(),
                0,
                &c,
            )
        });
    }

    fn selection_changed(&self) {
        self.notify(TS_AS_SEL_CHANGE, |sink| unsafe {
            // ITextStoreACPSink::OnSelectionChange.
            sink.call::<unsafe extern "system" fn(P) -> H>(4)(sink.ptr())
        });
    }

    /// Tell the sink, if it asked for `what`, without holding any borrow (it
    /// may call straight back in).
    fn notify(&self, what: u32, f: impl FnOnce(&Com) -> H) {
        if self.mask.get() & what == 0 {
            return;
        }
        let sink = self
            .sink
            .borrow()
            .as_ref()
            .and_then(|s| unsafe { Com::add_ref(s.ptr()) });
        if let Some(sink) = sink {
            f(&sink);
        }
    }

    /// After a lock: send the App what changed, then take the report that
    /// waited for the lock.
    fn flush(&self) {
        let events = self.core.borrow_mut().flush();
        for e in events {
            super::debug(format_args!("{e:?}"));
            push_event_to(self.hwnd, Event::Ime(e));
        }
        let pending = self.pending.borrow_mut().take();
        if let Some(app) = pending {
            self.report(app);
        }
    }

    /// The caret rectangle in screen coordinates.
    fn caret_on_screen(&self) -> Rect {
        let (l, t, r, b) = self.caret.get();
        let mut p = Point { x: l, y: t };
        unsafe { ClientToScreen(self.hwnd, &mut p) };
        Rect {
            left: p.x,
            top: p.y,
            right: p.x + (r - l),
            bottom: p.y + (b - t),
        }
    }

    /// Read the composition's range back from TSF, and its display
    /// attributes with the read cookie `ec`.
    fn read_composition(&self, context: Option<&Com>, ec: u32) {
        let range = self.view.borrow().as_ref().and_then(view_range);
        let Some(range) = range else {
            return;
        };
        if let Some(r) = range_extent(&range) {
            self.core.borrow_mut().update_composition(r);
        }
        if let (Some(lookup), Some(context)) = (&self.attrs, context) {
            let found = lookup.read(context, ec, &range);
            self.core.borrow_mut().set_attributes(found);
        }
    }
}

fn hr_of(e: StoreError) -> H {
    match e {
        StoreError::NoLock => TS_E_NOLOCK,
        StoreError::InvalidPos => TS_E_INVALIDPOS,
    }
}

fn pos(v: i32) -> Result<usize, H> {
    usize::try_from(v).map_err(|_| TS_E_INVALIDPOS)
}

/// The text TSF passes in.
unsafe fn units<'a>(p: *const u16, n: u32) -> &'a [u16] {
    if p.is_null() || n == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(p, n as usize) }
    }
}

fn put_change(out: *mut TsTextChange, c: TextChange) {
    if !out.is_null() {
        unsafe {
            *out = TsTextChange {
                start: c.start as i32,
                old_end: c.old_end as i32,
                new_end: c.new_end as i32,
            }
        };
    }
}

// ---- IUnknown, for each interface -------------------------------------------

unsafe fn base_of(this: P, offset: usize) -> *mut Store {
    unsafe { (this as *mut u8).sub(offset).cast() }
}

unsafe fn query(store: *mut Store, iid: *const Guid, out: *mut P) -> H {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    let iid = unsafe { *iid };
    let base = store as *mut u8;
    let at = if iid == IID_IUNKNOWN || iid == IID_ITEXTSTOREACP {
        Some(0)
    } else if iid == IID_ITFCONTEXTOWNERCOMPOSITIONSINK {
        Some(std::mem::offset_of!(Store, owner))
    } else if iid == IID_ITFTEXTEDITSINK {
        Some(std::mem::offset_of!(Store, edit))
    } else {
        None
    };
    match at {
        Some(offset) => unsafe {
            add_ref(store);
            *out = base.add(offset).cast();
            S_OK
        },
        None => unsafe {
            *out = std::ptr::null_mut();
            E_NOINTERFACE
        },
    }
}

unsafe fn add_ref(store: *mut Store) -> u32 {
    let s = unsafe { &*store };
    s.refs.set(s.refs.get() + 1);
    s.refs.get()
}

unsafe fn release(store: *mut Store) -> u32 {
    let left = {
        let s = unsafe { &*store };
        s.refs.set(s.refs.get() - 1);
        s.refs.get()
    };
    if left == 0 {
        drop(unsafe { Box::from_raw(store) });
    }
    left
}

macro_rules! unknown_for {
    ($qi:ident, $add:ident, $rel:ident, $field:ident) => {
        unsafe extern "system" fn $qi(this: P, iid: *const Guid, out: *mut P) -> H {
            unsafe { query(base_of(this, std::mem::offset_of!(Store, $field)), iid, out) }
        }
        unsafe extern "system" fn $add(this: P) -> u32 {
            unsafe { add_ref(base_of(this, std::mem::offset_of!(Store, $field))) }
        }
        unsafe extern "system" fn $rel(this: P) -> u32 {
            unsafe { release(base_of(this, std::mem::offset_of!(Store, $field))) }
        }
    };
}

unknown_for!(acp_query_interface, acp_add_ref, acp_release, acp);
unknown_for!(owner_query_interface, owner_add_ref, owner_release, owner);
unknown_for!(edit_query_interface, edit_add_ref, edit_release, edit);

/// The store behind an `ITextStoreACP` pointer.
unsafe fn store<'a>(this: P) -> &'a Store {
    unsafe { &*(this as *const Store) }
}

// ---- ITextStoreACP ----------------------------------------------------------

unsafe extern "system" fn advise_sink(this: P, iid: *const Guid, punk: P, mask: u32) -> H {
    let s = unsafe { store(this) };
    if iid.is_null() || punk.is_null() {
        return E_INVALIDARG;
    }
    if unsafe { *iid } != IID_ITEXTSTOREACPSINK {
        return E_INVALIDARG;
    }
    let Some(given) = (unsafe { Com::add_ref(punk) }) else {
        return E_INVALIDARG;
    };
    let identity = |c: &Com| c.query(&IID_IUNKNOWN).map(|u| u.ptr());
    let mut sink = s.sink.borrow_mut();
    if let Some(old) = sink.as_ref() {
        // The same sink again only changes what it wants to hear of.
        if identity(old) == identity(&given) {
            s.mask.set(mask);
            return S_OK;
        }
        return CONNECT_E_ADVISELIMIT;
    }
    let Some(acp_sink) = given.query(&IID_ITEXTSTOREACPSINK) else {
        return E_NOINTERFACE;
    };
    *sink = Some(acp_sink);
    s.mask.set(mask);
    S_OK
}

unsafe extern "system" fn unadvise_sink(this: P, _punk: P) -> H {
    let s = unsafe { store(this) };
    let old = s.sink.borrow_mut().take();
    if old.is_none() {
        return CONNECT_E_NOCONNECTION;
    }
    s.mask.set(0);
    S_OK
}

unsafe extern "system" fn request_lock(this: P, flags: u32, session: *mut H) -> H {
    let s = unsafe { store(this) };
    if session.is_null() {
        return E_INVALIDARG;
    }
    let sink = s
        .sink
        .borrow()
        .as_ref()
        .and_then(|c| unsafe { Com::add_ref(c.ptr()) });
    let Some(sink) = sink else {
        return E_UNEXPECTED;
    };
    let grant = s.core.borrow_mut().request_lock(flags);
    let mut now = match grant {
        LockGrant::Refused => {
            unsafe { *session = TS_E_SYNCHRONOUS };
            return S_OK;
        }
        LockGrant::Queued => {
            unsafe { *session = TS_S_ASYNC };
            return S_OK;
        }
        LockGrant::Now(f) => f,
    };
    // The store stays alive through the callbacks even if TSF lets go.
    let this_store = this as *mut Store;
    unsafe { add_ref(this_store) };
    let mut first = true;
    loop {
        // ITextStoreACPSink::OnLockGranted: TSF runs the edit sessions now.
        let hr = unsafe { sink.call::<unsafe extern "system" fn(P, u32) -> H>(8)(sink.ptr(), now) };
        if first {
            unsafe { *session = hr };
            first = false;
        }
        // Then the lock queued meanwhile, if any.
        match s.core.borrow_mut().release_lock() {
            Some(f) => now = f,
            None => break,
        }
    }
    s.flush();
    unsafe { release(this_store) };
    S_OK
}

unsafe extern "system" fn get_status(_this: P, status: *mut TsStatus) -> H {
    if status.is_null() {
        return E_INVALIDARG;
    }
    unsafe {
        *status = TsStatus {
            dynamic: 0,
            statics: TS_SS_NOHIDDENTEXT,
        }
    };
    S_OK
}

unsafe extern "system" fn query_insert(
    this: P,
    start: i32,
    end: i32,
    _len: u32,
    out_start: *mut i32,
    out_end: *mut i32,
) -> H {
    let s = unsafe { store(this) };
    if out_start.is_null() || out_end.is_null() {
        return E_INVALIDARG;
    }
    let (Ok(a), Ok(b)) = (pos(start), pos(end)) else {
        return E_INVALIDARG;
    };
    match s.core.borrow().query_insert(a, b) {
        Ok((a, b)) => {
            unsafe {
                *out_start = a as i32;
                *out_end = b as i32;
            }
            S_OK
        }
        Err(_) => E_INVALIDARG,
    }
}

unsafe extern "system" fn get_selection(
    this: P,
    index: u32,
    count: u32,
    out: *mut TsSelection,
    fetched: *mut u32,
) -> H {
    let s = unsafe { store(this) };
    if out.is_null() || fetched.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *fetched = 0 };
    if index != 0 && index != TS_DEFAULT_SELECTION {
        return E_INVALIDARG;
    }
    let sel = match s.core.borrow().selection() {
        Ok(sel) => sel,
        Err(e) => return hr_of(e),
    };
    if count == 0 {
        return S_OK;
    }
    let Selection { start, end, active } = sel;
    unsafe {
        *out = TsSelection {
            start: start as i32,
            end: end as i32,
            active: match active {
                ActiveEnd::Start => TS_AE_START,
                ActiveEnd::End => TS_AE_END,
                ActiveEnd::None => 0,
            },
            interim: 0,
        };
        *fetched = 1;
    }
    S_OK
}

unsafe extern "system" fn set_selection(this: P, count: u32, sel: *const TsSelection) -> H {
    let s = unsafe { store(this) };
    if count != 1 || sel.is_null() {
        return E_INVALIDARG;
    }
    let sel = unsafe { &*sel };
    let (Ok(a), Ok(b)) = (pos(sel.start), pos(sel.end)) else {
        return TS_E_INVALIDPOS;
    };
    let active = match sel.active {
        TS_AE_START => ActiveEnd::Start,
        TS_AE_END => ActiveEnd::End,
        _ => ActiveEnd::None,
    };
    match s.core.borrow_mut().set_selection(a, b, active) {
        Ok(()) => S_OK,
        Err(e) => hr_of(e),
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn get_text(
    this: P,
    start: i32,
    end: i32,
    plain: *mut u16,
    plain_len: u32,
    plain_got: *mut u32,
    runs: *mut TsRunInfo,
    runs_len: u32,
    runs_got: *mut u32,
    next: *mut i32,
) -> H {
    let s = unsafe { store(this) };
    if plain_got.is_null() || runs_got.is_null() || next.is_null() {
        return E_INVALIDARG;
    }
    let Ok(start) = pos(start) else {
        return TS_E_INVALIDPOS;
    };
    let end = if end == -1 {
        None
    } else {
        match pos(end) {
            Ok(e) => Some(e),
            Err(e) => return e,
        }
    };
    let max = if plain.is_null() {
        usize::MAX
    } else {
        plain_len as usize
    };
    let core = s.core.borrow();
    let text = match core.text(start, end, max) {
        Ok(t) => t,
        Err(e) => return hr_of(e),
    };
    unsafe {
        if !plain.is_null() {
            std::ptr::copy_nonoverlapping(text.as_ptr(), plain, text.len());
            *plain_got = text.len() as u32;
        } else {
            *plain_got = 0;
        }
        if !runs.is_null() && runs_len > 0 && !text.is_empty() {
            *runs = TsRunInfo {
                count: text.len() as u32,
                kind: 0, // TS_RT_PLAIN
            };
            *runs_got = 1;
        } else {
            *runs_got = 0;
        }
        *next = (start + text.len()) as i32;
    }
    S_OK
}

unsafe extern "system" fn set_text(
    this: P,
    _flags: u32,
    start: i32,
    end: i32,
    text: *const u16,
    len: u32,
    change: *mut TsTextChange,
) -> H {
    let s = unsafe { store(this) };
    let (Ok(a), Ok(b)) = (pos(start), pos(end)) else {
        return TS_E_INVALIDPOS;
    };
    let with = unsafe { units(text, len) };
    let done = s.core.borrow_mut().set_text(a, b, with);
    match done {
        Ok(c) => {
            put_change(change, c);
            S_OK
        }
        Err(e) => hr_of(e),
    }
}

unsafe extern "system" fn get_formatted_text(_this: P, _s: i32, _e: i32, out: *mut P) -> H {
    if !out.is_null() {
        unsafe { *out = std::ptr::null_mut() };
    }
    E_NOTIMPL
}

unsafe extern "system" fn get_embedded(
    _this: P,
    _pos: i32,
    _service: *const Guid,
    _iid: *const Guid,
    out: *mut P,
) -> H {
    if !out.is_null() {
        unsafe { *out = std::ptr::null_mut() };
    }
    E_NOTIMPL
}

unsafe extern "system" fn query_insert_embedded(
    _this: P,
    _service: *const Guid,
    _format: P,
    insertable: *mut i32,
) -> H {
    if insertable.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *insertable = 0 };
    S_OK
}

unsafe extern "system" fn insert_embedded(
    _this: P,
    _flags: u32,
    _s: i32,
    _e: i32,
    _data: P,
    _change: *mut TsTextChange,
) -> H {
    E_NOTIMPL
}

unsafe extern "system" fn insert_text_at_selection(
    this: P,
    flags: u32,
    text: *const u16,
    len: u32,
    out_start: *mut i32,
    out_end: *mut i32,
    change: *mut TsTextChange,
) -> H {
    let s = unsafe { store(this) };
    let query_only = flags & TS_IAS_QUERYONLY != 0;
    if query_only && (out_start.is_null() || out_end.is_null()) {
        return E_INVALIDARG;
    }
    let with = unsafe { units(text, len) };
    let done = s.core.borrow_mut().insert_at_selection(with, query_only);
    match done {
        Ok((a, b, c)) => {
            unsafe {
                if !out_start.is_null() {
                    *out_start = a as i32;
                }
                if !out_end.is_null() {
                    *out_end = b as i32;
                }
            }
            if let Some(c) = c {
                put_change(change, c);
            }
            S_OK
        }
        Err(e) => hr_of(e),
    }
}

unsafe extern "system" fn insert_embedded_at_selection(
    _this: P,
    _flags: u32,
    _data: P,
    _s: *mut i32,
    _e: *mut i32,
    _change: *mut TsTextChange,
) -> H {
    E_NOTIMPL
}

/// No attributes (fonts, reading direction, input scope) are offered: the
/// requests succeed and retrieve nothing.
unsafe extern "system" fn request_supported_attrs(
    _this: P,
    _flags: u32,
    _n: u32,
    _ids: *const Guid,
) -> H {
    S_OK
}

unsafe extern "system" fn request_attrs_at_position(
    _this: P,
    _pos: i32,
    _n: u32,
    _ids: *const Guid,
    _flags: u32,
) -> H {
    S_OK
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn find_next_attr_transition(
    _this: P,
    _start: i32,
    halt: i32,
    _n: u32,
    _ids: *const Guid,
    _flags: u32,
    next: *mut i32,
    found: *mut i32,
    offset: *mut i32,
) -> H {
    if next.is_null() || found.is_null() || offset.is_null() {
        return E_INVALIDARG;
    }
    unsafe {
        *next = halt;
        *found = 0;
        *offset = 0;
    }
    S_OK
}

unsafe extern "system" fn retrieve_requested_attrs(
    _this: P,
    _n: u32,
    _vals: P,
    fetched: *mut u32,
) -> H {
    if fetched.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *fetched = 0 };
    S_OK
}

unsafe extern "system" fn get_end_acp(this: P, out: *mut i32) -> H {
    let s = unsafe { store(this) };
    if out.is_null() {
        return E_INVALIDARG;
    }
    match s.core.borrow().len() {
        Ok(n) => {
            unsafe { *out = n as i32 };
            S_OK
        }
        Err(e) => hr_of(e),
    }
}

unsafe extern "system" fn get_active_view(_this: P, view: *mut u32) -> H {
    if view.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *view = 0 };
    S_OK
}

unsafe extern "system" fn get_acp_from_point(
    _this: P,
    _view: u32,
    _pt: *const Point,
    _flags: u32,
    _out: *mut i32,
) -> H {
    E_NOTIMPL
}

/// Every range is placed at the caret (the composition's start while
/// composing), the one place Stipple's fields report.
unsafe extern "system" fn get_text_ext(
    this: P,
    _view: u32,
    _start: i32,
    _end: i32,
    rect: *mut Rect,
    clipped: *mut i32,
) -> H {
    let s = unsafe { store(this) };
    if rect.is_null() || clipped.is_null() {
        return E_INVALIDARG;
    }
    if s.core.borrow().len().is_err() {
        return TS_E_NOLOCK;
    }
    unsafe {
        *rect = s.caret_on_screen();
        *clipped = 0;
    }
    S_OK
}

unsafe extern "system" fn get_screen_ext(this: P, _view: u32, rect: *mut Rect) -> H {
    let s = unsafe { store(this) };
    if rect.is_null() {
        return E_INVALIDARG;
    }
    let mut r = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe { GetClientRect(s.hwnd, &mut r) };
    let mut p = Point { x: 0, y: 0 };
    unsafe { ClientToScreen(s.hwnd, &mut p) };
    unsafe {
        *rect = Rect {
            left: p.x,
            top: p.y,
            right: p.x + r.right,
            bottom: p.y + r.bottom,
        }
    };
    S_OK
}

unsafe extern "system" fn get_wnd(this: P, _view: u32, out: *mut Hwnd) -> H {
    let s = unsafe { store(this) };
    if out.is_null() {
        return E_INVALIDARG;
    }
    unsafe { *out = s.hwnd };
    S_OK
}

// ---- ITfContextOwnerCompositionSink -----------------------------------------

unsafe fn owner_store<'a>(this: P) -> &'a Store {
    unsafe { &*base_of(this, std::mem::offset_of!(Store, owner)) }
}

unsafe extern "system" fn on_start_composition(this: P, view: P, ok: *mut i32) -> H {
    let s = unsafe { owner_store(this) };
    if ok.is_null() {
        return E_INVALIDARG;
    }
    // A composition starting while we end one the App left is refused.
    if s.terminating.get() {
        unsafe { *ok = 0 };
        return S_OK;
    }
    unsafe { *ok = 1 };
    let view = unsafe { Com::add_ref(view) };
    let range = view
        .as_ref()
        .and_then(view_range)
        .and_then(|r| range_extent(&r));
    *s.view.borrow_mut() = view;
    s.core.borrow_mut().start_composition(range);
    super::debug(format_args!("composition started at {range:?}"));
    S_OK
}

unsafe extern "system" fn on_update_composition(this: P, view: P, range: P) -> H {
    let s = unsafe { owner_store(this) };
    let new_range = unsafe { Com::add_ref(range) }
        .or_else(|| unsafe { Com::add_ref(view) }.as_ref().and_then(view_range));
    if let Some(r) = new_range.as_ref().and_then(range_extent) {
        s.core.borrow_mut().update_composition(r);
    }
    S_OK
}

unsafe extern "system" fn on_end_composition(this: P, _view: P) -> H {
    let s = unsafe { owner_store(this) };
    s.view.borrow_mut().take();
    s.core.borrow_mut().end_composition();
    super::debug(format_args!("composition ended"));
    // Ended outside a lock (rarely): commit now.
    if !s.core.borrow().locked() {
        s.flush();
    }
    S_OK
}

// ---- ITfTextEditSink --------------------------------------------------------

/// At the end of each edit session, still under its lock: read where the
/// composition is and its display attributes (which clause is converting),
/// for the flush that follows.
unsafe extern "system" fn on_end_edit(this: P, context: P, ec: u32, _record: P) -> H {
    let s = unsafe { &*base_of(this, std::mem::offset_of!(Store, edit)) };
    if s.composing() {
        let context = unsafe { Com::add_ref(context) };
        s.read_composition(context.as_ref(), ec);
    }
    S_OK
}
