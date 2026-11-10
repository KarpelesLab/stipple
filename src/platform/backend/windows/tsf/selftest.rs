//! A composition driven through TSF itself, for CI (`STIPPLE_TSF_SELFTEST=1`).
//!
//! No input method can be scripted on a CI runner without installing one, so
//! the app plays the input method's part: once a text field has focus it asks
//! the context for edit sessions (`ITfContext::RequestEditSession`, as a text
//! service does), starts a composition at the selection
//! (`ITfContextComposition::StartComposition`), writes a reading into it and
//! then its conversion (`ITfRange::SetText`), and ends it
//! (`ITfComposition::EndComposition`). Every step goes through the real TSF
//! manager, which locks our store, writes through `ITextStoreACP` and calls
//! the composition sink, so the field must show the preedit `にほん`, then
//! `日本`, then the committed `日本`. Steps are spaced by a timer so the App
//! draws each one, and the window closes a second after the last.

use std::cell::Cell;
use std::ffi::c_void;

use super::super::Hwnd;
use super::com::{
    Com, Guid, Hresult, IID_ITFCONTEXTCOMPOSITION, IID_ITFEDITSESSION, IID_IUNKNOWN, out,
};
use super::debug;

type P = *mut c_void;
type Out = *mut *mut c_void;

const TF_ES_SYNC: u32 = 0x1;
const TF_ES_READWRITE: u32 = 0x6;
const TF_DEFAULT_SELECTION: u32 = u32::MAX;
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

thread_local! {
    /// The next step (0: not started).
    static STEP: Cell<u32> = const { Cell::new(0) };
    /// The context and client id the steps run against.
    static TARGET: Cell<(P, u32)> = const { Cell::new((std::ptr::null_mut(), 0)) };
    /// The composition in progress (`ITfComposition`, a reference we own).
    static COMPOSITION: Cell<P> = const { Cell::new(std::ptr::null_mut()) };
}

/// A text field took focus: start, once.
pub(super) fn on_focus(hwnd: Hwnd, context: &Com, client: u32) {
    if !enabled() || STEP.with(|s| s.get()) != 0 {
        return;
    }
    STEP.with(|s| s.set(1));
    TARGET.with(|t| t.set((context.ptr(), client)));
    println!("tsf selftest: started");
    unsafe { SetTimer(hwnd, TIMER_ID, 500, Some(tick)) };
}

unsafe extern "system" fn tick(hwnd: Hwnd, _msg: u32, id: usize, _time: u32) {
    unsafe { KillTimer(hwnd, id) };
    let step = STEP.with(|s| s.get());
    let (context, client) = TARGET.with(|t| t.get());
    if context.is_null() || step > 3 {
        if step > 3 {
            unsafe { super::super::PostMessageW(hwnd, super::super::WM_CLOSE, 0, 0) };
        }
        return;
    }
    let session = Box::into_raw(Box::new(Session {
        vtbl: &VTBL,
        refs: Cell::new(1),
        step,
        context,
    }));
    let mut hr_session: Hresult = 0;
    // ITfContext::RequestEditSession.
    let hr = unsafe {
        super::com::slot::<unsafe extern "system" fn(P, u32, P, u32, *mut Hresult) -> Hresult>(
            context, 3,
        )(
            context,
            client,
            session.cast(),
            TF_ES_SYNC | TF_ES_READWRITE,
            &mut hr_session,
        )
    };
    unsafe { release(session) };
    println!("tsf selftest: step {step}: hr={hr:#x} session={hr_session:#x}");
    match step {
        // The conversion stays up a while, for a screenshot.
        1 | 2 => {
            STEP.with(|s| s.set(step + 1));
            let wait = if step == 2 { 2000 } else { 500 };
            unsafe { SetTimer(hwnd, TIMER_ID, wait, Some(tick)) };
        }
        3 => {
            STEP.with(|s| s.set(4));
            println!("tsf selftest: done");
            unsafe { SetTimer(hwnd, TIMER_ID, 1000, Some(tick)) };
        }
        _ => {}
    }
}

/// `ITfEditSession`.
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
        return super::com::E_POINTER;
    }
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_ITFEDITSESSION {
        unsafe {
            add_ref(this);
            *out = this.cast();
        }
        super::com::S_OK
    } else {
        unsafe { *out = std::ptr::null_mut() };
        super::com::E_NOINTERFACE
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
        return super::com::E_FAIL;
    };
    // ITfComposition::GetRange.
    let Some(range) = out(|p| unsafe {
        comp.call::<unsafe extern "system" fn(P, Out) -> Hresult>(3)(comp.ptr(), p)
    }) else {
        return super::com::E_FAIL;
    };
    let units: Vec<u16> = text.encode_utf16().collect();
    // ITfRange::SetText.
    unsafe {
        range.call::<unsafe extern "system" fn(P, u32, u32, *const u16, i32) -> Hresult>(4)(
            range.ptr(),
            ec,
            0,
            units.as_ptr(),
            units.len() as i32,
        )
    }
}

unsafe extern "system" fn do_edit_session(this: *mut Session, ec: u32) -> Hresult {
    let s = unsafe { &*this };
    let Some(context) = (unsafe { Com::add_ref(s.context) }) else {
        return super::com::E_FAIL;
    };
    let hr = match s.step {
        1 => {
            // A composition over the selection, with the reading in it.
            let mut sel = TfSelection {
                range: std::ptr::null_mut(),
                active: 0,
                interim: 0,
            };
            let mut fetched = 0u32;
            // ITfContext::GetSelection.
            let hr = unsafe {
                context.call::<unsafe extern "system" fn(
                    P,
                    u32,
                    u32,
                    u32,
                    *mut TfSelection,
                    *mut u32,
                ) -> Hresult>(5)(
                    context.ptr(),
                    ec,
                    TF_DEFAULT_SELECTION,
                    1,
                    &mut sel,
                    &mut fetched,
                )
            };
            let Some(range) = Com::from_raw(sel.range).filter(|_| hr >= 0 && fetched == 1) else {
                debug(format_args!("selftest: GetSelection: {hr:#x}"));
                return super::com::E_FAIL;
            };
            let Some(cc) = context.query(&IID_ITFCONTEXTCOMPOSITION) else {
                return super::com::E_NOINTERFACE;
            };
            // ITfContextComposition::StartComposition.
            let comp = out(|p| unsafe {
                cc.call::<unsafe extern "system" fn(P, u32, P, P, Out) -> Hresult>(3)(
                    cc.ptr(),
                    ec,
                    range.ptr(),
                    std::ptr::null_mut(),
                    p,
                )
            });
            let Some(comp) = comp else {
                debug(format_args!("selftest: StartComposition failed"));
                return super::com::E_FAIL;
            };
            let hr = write(comp.ptr(), ec, "にほん");
            COMPOSITION.with(|c| c.set(comp.into_raw()));
            hr
        }
        2 => write(COMPOSITION.with(|c| c.get()), ec, "日本"),
        _ => {
            let Some(comp) = Com::from_raw(COMPOSITION.with(|c| c.replace(std::ptr::null_mut())))
            else {
                return super::com::E_FAIL;
            };
            // ITfComposition::EndComposition.
            unsafe { comp.call::<unsafe extern "system" fn(P, u32) -> Hresult>(6)(comp.ptr(), ec) }
        }
    };
    debug(format_args!("selftest: edit session {}: {hr:#x}", s.step));
    hr
}
