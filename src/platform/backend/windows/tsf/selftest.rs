//! A composition driven through TSF itself, for CI (`STIPPLE_TSF_SELFTEST=1`).
//!
//! No input method can be scripted on a CI runner without installing one, so
//! the app plays the input method's part: once a text field has focus it asks
//! the context for edit sessions (`ITfContext::RequestEditSession`, as a text
//! service does), starts a composition at the selection, writes a reading
//! into it and then its conversion (`ITfRange::SetText`), and ends it
//! (`ITfComposition::EndComposition`). Every step goes through the real TSF
//! manager, which locks our store, writes through `ITextStoreACP` and calls
//! the composition sink, so the field must show the preedit `にほん`, then
//! `日本`, then the committed `日本`. Steps are spaced by a timer so the App
//! draws each one, and the window closes a second after the last.
//!
//! Starting the composition is tried every way a client can (through
//! `ITfContextOwnerCompositionServices`, the owner's interface, and through
//! `ITfContextComposition`; over the selection's range and over the
//! insertion point `ITfInsertAtSelection` gives; with a composition sink and
//! without), printing each attempt's `HRESULT`. If none works, `日本` is
//! inserted with `ITfInsertAtSelection::InsertTextAtSelection` instead, which
//! still goes through the store's write path to a commit, and the log says
//! `composition unavailable`.

use std::cell::Cell;
use std::ffi::c_void;

use super::super::Hwnd;
use super::com::{
    Com, E_FAIL, E_NOINTERFACE, E_POINTER, Guid, Hresult, IID_ITFCOMPOSITIONSINK,
    IID_ITFCONTEXTCOMPOSITION, IID_ITFCONTEXTOWNERCOMPOSITIONSERVICES, IID_ITFEDITSESSION,
    IID_ITFINSERTATSELECTION, IID_IUNKNOWN, S_OK, out, out_hr, range_extent,
};
use super::debug;

type P = *mut c_void;
type Out = *mut *mut c_void;

const TF_ES_SYNC: u32 = 0x1;
const TF_ES_READWRITE: u32 = 0x6;
const TF_DEFAULT_SELECTION: u32 = u32::MAX;
/// `ITfInsertAtSelection` flags: only say where text would go.
const TF_IAS_QUERYONLY: u32 = 0x2;
const TIMER_ID: usize = 0x7353;

#[link(name = "user32")]
unsafe extern "system" {
    fn SetTimer(
        hwnd: Hwnd,
        id: usize,
        ms: u32,
        proc_: Option<unsafe extern "system" fn(Hwnd, u32, usize, u32)>,
    ) -> usize;
    fn KillTimer(hwnd: Hwnd, id: usize) -> i32;
}

/// Whether the self-test was asked for.
pub(super) fn enabled() -> bool {
    std::env::var_os("STIPPLE_TSF_SELFTEST").is_some_and(|v| v != "0")
}

/// What the TSF objects the steps use are.
#[derive(Clone, Copy)]
struct Target {
    context: P,
    client: u32,
    mgr: P,
    dim: P,
}

thread_local! {
    /// The next step (0: not started).
    static STEP: Cell<u32> = const { Cell::new(0) };
    static TARGET: Cell<Option<Target>> = const { Cell::new(None) };
    /// The composition in progress (`ITfComposition`, a reference we own).
    static COMPOSITION: Cell<P> = const { Cell::new(std::ptr::null_mut()) };
}

/// A text field took focus: start, once. `mgr` is the thread manager,
/// `dim` the window's document manager.
pub(super) fn on_focus(hwnd: Hwnd, context: &Com, client: u32, mgr: &Com, dim: &Com) {
    if !enabled() || STEP.with(|s| s.get()) != 0 {
        return;
    }
    STEP.with(|s| s.set(1));
    TARGET.with(|t| {
        t.set(Some(Target {
            context: context.ptr(),
            client,
            mgr: mgr.ptr(),
            dim: dim.ptr(),
        }))
    });
    println!("tsf selftest: started");
    unsafe { SetTimer(hwnd, TIMER_ID, 500, Some(tick)) };
}

/// Report whether TSF's focus is on our document, and put it there if not
/// (the runner's window may not have the keyboard).
fn check_focus(t: &Target) {
    let Some(mgr) = (unsafe { Com::add_ref(t.mgr) }) else {
        return;
    };
    // ITfThreadMgr::GetFocus.
    let (hr, focus) = out_hr(|p| unsafe {
        mgr.call::<unsafe extern "system" fn(P, Out) -> Hresult>(7)(mgr.ptr(), p)
    });
    let ours = focus.as_ref().is_some_and(|f| f.ptr() == t.dim);
    let mut thread_focus = 0i32;
    // ITfThreadMgr::IsThreadFocus.
    unsafe {
        mgr.call::<unsafe extern "system" fn(P, *mut i32) -> Hresult>(10)(
            mgr.ptr(),
            &mut thread_focus,
        )
    };
    println!(
        "tsf selftest: focus: GetFocus={hr:#x} ours={ours} other={} thread focus={}",
        focus.is_some() && !ours,
        thread_focus != 0
    );
    if !ours {
        // ITfThreadMgr::SetFocus.
        let hr =
            unsafe { mgr.call::<unsafe extern "system" fn(P, P) -> Hresult>(8)(mgr.ptr(), t.dim) };
        println!("tsf selftest: SetFocus(document)={hr:#x}");
    }
}

unsafe extern "system" fn tick(hwnd: Hwnd, _msg: u32, id: usize, _time: u32) {
    unsafe { KillTimer(hwnd, id) };
    let step = STEP.with(|s| s.get());
    let Some(target) = TARGET.with(|t| t.get()) else {
        return;
    };
    if step > 3 {
        unsafe { super::super::PostMessageW(hwnd, super::super::WM_CLOSE, 0, 0) };
        return;
    }
    if step == 1 {
        check_focus(&target);
    }
    // Without a composition, steps 2 and 3 have nothing to do.
    if step == 1 || !COMPOSITION.with(|c| c.get()).is_null() {
        let session = Box::into_raw(Box::new(Session {
            vtbl: &VTBL,
            refs: Cell::new(1),
            step,
            context: target.context,
        }));
        let mut hr_session: Hresult = 0;
        // ITfContext::RequestEditSession.
        let hr = unsafe {
            super::com::slot::<unsafe extern "system" fn(P, u32, P, u32, *mut Hresult) -> Hresult>(
                target.context,
                3,
            )(
                target.context,
                target.client,
                session.cast(),
                TF_ES_SYNC | TF_ES_READWRITE,
                &mut hr_session,
            )
        };
        unsafe { release(session) };
        println!("tsf selftest: step {step}: hr={hr:#x} session={hr_session:#x}");
    } else {
        println!("tsf selftest: step {step}: skipped (no composition)");
    }
    match step {
        // The conversion stays up a while, for a screenshot.
        1 | 2 => {
            STEP.with(|s| s.set(step + 1));
            let wait = if step == 2 { 2000 } else { 500 };
            unsafe { SetTimer(hwnd, TIMER_ID, wait, Some(tick)) };
        }
        _ => {
            STEP.with(|s| s.set(4));
            println!("tsf selftest: done");
            unsafe { SetTimer(hwnd, TIMER_ID, 1000, Some(tick)) };
        }
    }
}

// ---- ITfEditSession ---------------------------------------------------------

#[repr(C)]
struct SessionVtbl {
    query_interface: unsafe extern "system" fn(*mut Session, *const Guid, *mut P) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut Session) -> u32,
    release: unsafe extern "system" fn(*mut Session) -> u32,
    do_edit_session: unsafe extern "system" fn(*mut Session, u32) -> Hresult,
}

#[repr(C)]
struct Session {
    vtbl: &'static SessionVtbl,
    refs: Cell<u32>,
    step: u32,
    context: P,
}

static VTBL: SessionVtbl = SessionVtbl {
    query_interface,
    add_ref,
    release,
    do_edit_session,
};

unsafe extern "system" fn query_interface(
    this: *mut Session,
    iid: *const Guid,
    out: *mut P,
) -> Hresult {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_ITFEDITSESSION {
        unsafe {
            add_ref(this);
            *out = this.cast();
        }
        S_OK
    } else {
        unsafe { *out = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(this: *mut Session) -> u32 {
    let s = unsafe { &*this };
    s.refs.set(s.refs.get() + 1);
    s.refs.get()
}

unsafe extern "system" fn release(this: *mut Session) -> u32 {
    let left = {
        let s = unsafe { &*this };
        s.refs.set(s.refs.get() - 1);
        s.refs.get()
    };
    if left == 0 {
        drop(unsafe { Box::from_raw(this) });
    }
    left
}

// ---- ITfCompositionSink (a static object: nothing to free) ------------------

#[repr(C)]
struct SinkVtbl {
    query_interface: unsafe extern "system" fn(*mut Sink, *const Guid, *mut P) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut Sink) -> u32,
    release: unsafe extern "system" fn(*mut Sink) -> u32,
    on_composition_terminated: unsafe extern "system" fn(*mut Sink, u32, P) -> Hresult,
}

#[repr(C)]
struct Sink {
    vtbl: &'static SinkVtbl,
}

static SINK_VTBL: SinkVtbl = SinkVtbl {
    query_interface: sink_query_interface,
    add_ref: sink_ref,
    release: sink_ref,
    on_composition_terminated,
};

/// `Sink` holds only a `&'static`, so one shared instance is enough.
struct SyncSink(Sink);
unsafe impl Sync for SyncSink {}
static SINK: SyncSink = SyncSink(Sink { vtbl: &SINK_VTBL });

unsafe extern "system" fn sink_query_interface(
    this: *mut Sink,
    iid: *const Guid,
    out: *mut P,
) -> Hresult {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_ITFCOMPOSITIONSINK {
        unsafe { *out = this.cast() };
        S_OK
    } else {
        unsafe { *out = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn sink_ref(_this: *mut Sink) -> u32 {
    1
}

unsafe extern "system" fn on_composition_terminated(_this: *mut Sink, _ec: u32, _c: P) -> Hresult {
    println!("tsf selftest: composition terminated by TSF");
    COMPOSITION.with(|c| drop(Com::from_raw(c.replace(std::ptr::null_mut()))));
    S_OK
}

fn sink_ptr() -> P {
    (&SINK.0 as *const Sink).cast_mut().cast()
}

// ---- The steps --------------------------------------------------------------

/// `TF_SELECTION`.
#[repr(C)]
struct TfSelection {
    range: P,
    active: i32,
    interim: i32,
}

/// Write `text` over the composition's range.
fn write(composition: P, ec: u32, text: &str) -> Hresult {
    let Some(comp) = (unsafe { Com::add_ref(composition) }) else {
        return E_FAIL;
    };
    // ITfComposition::GetRange.
    let Some(range) = out(|p| unsafe {
        comp.call::<unsafe extern "system" fn(P, Out) -> Hresult>(3)(comp.ptr(), p)
    }) else {
        return E_FAIL;
    };
    let units: Vec<u16> = text.encode_utf16().collect();
    // ITfRange::SetText.
    let hr = unsafe {
        range.call::<unsafe extern "system" fn(P, u32, u32, *const u16, i32) -> Hresult>(4)(
            range.ptr(),
            ec,
            0,
            units.as_ptr(),
            units.len() as i32,
        )
    };
    println!(
        "tsf selftest: SetText({text:?})={hr:#x}, composition now {:?}",
        range_extent(&range)
    );
    hr
}

/// The selection's range (`ITfContext::GetSelection`).
fn selection_range(context: &Com, ec: u32) -> Option<Com> {
    let mut sel = TfSelection {
        range: std::ptr::null_mut(),
        active: 0,
        interim: 0,
    };
    let mut fetched = 0u32;
    let hr = unsafe {
        context.call::<unsafe extern "system" fn(P, u32, u32, u32, *mut TfSelection, *mut u32) -> Hresult>(5)(
            context.ptr(),
            ec,
            TF_DEFAULT_SELECTION,
            1,
            &mut sel,
            &mut fetched,
        )
    };
    let range = Com::from_raw(sel.range);
    println!(
        "tsf selftest: GetSelection={hr:#x} fetched={fetched} range={:?}",
        range.as_ref().and_then(range_extent)
    );
    range.filter(|_| hr >= 0 && fetched == 1)
}

/// Where inserted text would go (`ITfInsertAtSelection`, query only), the
/// range a text service starts its composition over.
fn insertion_range(insert: Option<&Com>, ec: u32) -> Option<Com> {
    let insert = insert?;
    let (hr, range) = out_hr(|p| unsafe {
        insert.call::<unsafe extern "system" fn(P, u32, u32, *const u16, i32, Out) -> Hresult>(3)(
            insert.ptr(),
            ec,
            TF_IAS_QUERYONLY,
            std::ptr::null(),
            0,
            p,
        )
    });
    println!(
        "tsf selftest: InsertTextAtSelection(query)={hr:#x} range={:?}",
        range.as_ref().and_then(range_extent)
    );
    range
}

/// Start a composition, every way there is, until one works.
fn start_composition(context: &Com, ec: u32) -> Option<Com> {
    let insert = context.query(&IID_ITFINSERTATSELECTION);
    let ranges = [
        ("selection", selection_range(context, ec)),
        ("insertion point", insertion_range(insert.as_ref(), ec)),
    ];
    let services = [
        (
            "owner services",
            context.query(&IID_ITFCONTEXTOWNERCOMPOSITIONSERVICES),
        ),
        (
            "context composition",
            context.query(&IID_ITFCONTEXTCOMPOSITION),
        ),
    ];
    for (via, service) in &services {
        let Some(service) = service else {
            println!("tsf selftest: no {via} interface");
            continue;
        };
        for (over, range) in &ranges {
            let Some(range) = range else {
                continue;
            };
            for sink in [sink_ptr(), std::ptr::null_mut()] {
                // ITfContextComposition::StartComposition (the owner services
                // interface derives from it).
                let (hr, comp) = out_hr(|p| unsafe {
                    service.call::<unsafe extern "system" fn(P, u32, P, P, Out) -> Hresult>(3)(
                        service.ptr(),
                        ec,
                        range.ptr(),
                        sink,
                        p,
                    )
                });
                println!(
                    "tsf selftest: StartComposition via {via} over the {over} {} a sink: hr={hr:#x} composition={}",
                    if sink.is_null() { "without" } else { "with" },
                    comp.is_some()
                );
                if comp.is_some() {
                    return comp;
                }
            }
        }
    }
    // Nothing composes: insert the text plainly, through the same store.
    if let Some(insert) = &insert {
        let units: Vec<u16> = "日本".encode_utf16().collect();
        let (hr, _range) = out_hr(|p| unsafe {
            insert
                .call::<unsafe extern "system" fn(P, u32, u32, *const u16, i32, Out) -> Hresult>(3)(
                insert.ptr(),
                ec,
                0,
                units.as_ptr(),
                units.len() as i32,
                p,
            )
        });
        println!("tsf selftest: composition unavailable; InsertTextAtSelection(\"日本\")={hr:#x}");
    }
    None
}

unsafe extern "system" fn do_edit_session(this: *mut Session, ec: u32) -> Hresult {
    let s = unsafe { &*this };
    let Some(context) = (unsafe { Com::add_ref(s.context) }) else {
        return E_FAIL;
    };
    let hr = match s.step {
        1 => match start_composition(&context, ec) {
            Some(comp) => {
                let hr = write(comp.ptr(), ec, "にほん");
                COMPOSITION.with(|c| c.set(comp.into_raw()));
                hr
            }
            None => E_FAIL,
        },
        2 => write(COMPOSITION.with(|c| c.get()), ec, "日本"),
        _ => {
            let Some(comp) = Com::from_raw(COMPOSITION.with(|c| c.replace(std::ptr::null_mut())))
            else {
                return E_FAIL;
            };
            // ITfComposition::EndComposition.
            unsafe { comp.call::<unsafe extern "system" fn(P, u32) -> Hresult>(6)(comp.ptr(), ec) }
        }
    };
    debug(format_args!("selftest: edit session {}: {hr:#x}", s.step));
    hr
}
