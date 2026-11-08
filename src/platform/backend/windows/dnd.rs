//! Receiving drops on Windows: an OLE `IDropTarget` with a hand-written COM
//! vtable, registered with `RegisterDragDrop`, plus `WM_DROPFILES` for the
//! older shell protocol.
//!
//! OLE calls the target from inside our message loop's `DispatchMessageW`
//! (cross-process calls arrive as messages to the apartment), where the app's
//! handler can't run: each call queues its event like the window procedure
//! does, and answers the source with the app's last verdict (from
//! `Window::set_drop_accepted`), so the cursor's copy / no sign trails the
//! pointer by one move. A drag entering is first answered "copy" when it
//! carries files or text. The data is read on `Drop`: `CF_HDROP` (files, via
//! `DragQueryFileW`), else `CF_UNICODETEXT`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use super::{CTX, Hwnd, current_scale, push_event};
use crate::geometry::Point;
use crate::platform::event::{DragOffer, DropData, Event};

type Hresult = i32;

const S_OK: Hresult = 0;
const E_NOINTERFACE: Hresult = 0x8000_4002_u32 as i32;
const E_POINTER: Hresult = 0x8000_4003_u32 as i32;

const CF_UNICODETEXT: u16 = 13;
const CF_HDROP: u16 = 15;
const DVASPECT_CONTENT: u32 = 1;
const TYMED_HGLOBAL: u32 = 1;

const DROPEFFECT_NONE: u32 = 0;
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;
const DROPEFFECT_LINK: u32 = 4;

pub(super) const WM_DROPFILES: u32 = 0x0233;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid(u32, u16, u16, [u8; 8]);

const IID_IUNKNOWN: Guid = Guid(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IDROPTARGET: Guid = Guid(0x0000_0122, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

#[repr(C)]
#[derive(Clone, Copy)]
struct PointL {
    x: i32,
    y: i32,
}

#[repr(C)]
struct FormatEtc {
    cf_format: u16,
    ptd: *mut c_void,
    aspect: u32,
    lindex: i32,
    tymed: u32,
}

#[repr(C)]
struct StgMedium {
    tymed: u32,
    /// The union's `hGlobal` (every member is pointer-sized).
    handle: *mut c_void,
    unk_for_release: *mut c_void,
}

/// The slots of `IDataObject` we call.
#[repr(C)]
struct DataObjectVtbl {
    query_interface: usize,
    add_ref: usize,
    release: usize,
    get_data: unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut StgMedium) -> Hresult,
    get_data_here: usize,
    query_get_data: unsafe extern "system" fn(*mut c_void, *const FormatEtc) -> Hresult,
}

#[repr(C)]
struct DropTargetVtbl {
    query_interface:
        unsafe extern "system" fn(*mut DropTarget, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut DropTarget) -> u32,
    release: unsafe extern "system" fn(*mut DropTarget) -> u32,
    drag_enter:
        unsafe extern "system" fn(*mut DropTarget, *mut c_void, u32, PointL, *mut u32) -> Hresult,
    drag_over: unsafe extern "system" fn(*mut DropTarget, u32, PointL, *mut u32) -> Hresult,
    drag_leave: unsafe extern "system" fn(*mut DropTarget) -> Hresult,
    drop: unsafe extern "system" fn(*mut DropTarget, *mut c_void, u32, PointL, *mut u32) -> Hresult,
}

/// Our `IDropTarget`: the vtable pointer first, as COM expects.
#[repr(C)]
struct DropTarget {
    vtbl: &'static DropTargetVtbl,
    refs: AtomicU32,
    hwnd: Hwnd,
}

static VTBL: DropTargetVtbl = DropTargetVtbl {
    query_interface,
    add_ref,
    release,
    drag_enter,
    drag_over,
    drag_leave,
    drop,
};

#[link(name = "ole32")]
unsafe extern "system" {
    fn OleInitialize(reserved: *mut c_void) -> Hresult;
    fn RegisterDragDrop(hwnd: Hwnd, target: *mut c_void) -> Hresult;
    fn ReleaseStgMedium(medium: *mut StgMedium);
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn DragAcceptFiles(hwnd: Hwnd, accept: i32);
    fn DragQueryFileW(hdrop: *mut c_void, index: u32, file: *mut u16, cch: u32) -> u32;
    fn DragQueryPoint(hdrop: *mut c_void, point: *mut PointL) -> i32;
    fn DragFinish(hdrop: *mut c_void);
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalLock(mem: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(mem: *mut c_void) -> i32;
    fn GlobalSize(mem: *mut c_void) -> usize;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn ScreenToClient(hwnd: Hwnd, point: *mut PointL) -> i32;
}

/// Make `hwnd` a drop target: initialize OLE on this thread and register an
/// `IDropTarget`, and accept `WM_DROPFILES` too. Returns whether OLE
/// registration worked (`WM_DROPFILES` works regardless).
pub(super) fn register(hwnd: Hwnd) -> bool {
    unsafe { DragAcceptFiles(hwnd, 1) };
    // S_FALSE: already initialized on this thread, which is fine.
    let hr = unsafe { OleInitialize(std::ptr::null_mut()) };
    if hr < 0 {
        return false;
    }
    let target = Box::into_raw(Box::new(DropTarget {
        vtbl: &VTBL,
        refs: AtomicU32::new(1),
        hwnd,
    }));
    // OLE holds its own reference; ours lives as long as the process.
    unsafe { RegisterDragDrop(hwnd, target.cast()) >= 0 }
}

unsafe extern "system" fn query_interface(
    this: *mut DropTarget,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> Hresult {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_IDROPTARGET {
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

unsafe extern "system" fn add_ref(this: *mut DropTarget) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn release(this: *mut DropTarget) -> u32 {
    let left = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1 };
    if left == 0 {
        std::mem::drop(unsafe { Box::from_raw(this) });
    }
    left
}

/// A screen point as a logical position in `hwnd`'s client area.
fn client_point(hwnd: Hwnd, pt: PointL) -> Point {
    let mut p = pt;
    unsafe { ScreenToClient(hwnd, &mut p) };
    let scale = current_scale();
    Point::new(p.x as f64 / scale, p.y as f64 / scale)
}

fn format(cf: u16) -> FormatEtc {
    FormatEtc {
        cf_format: cf,
        ptd: std::ptr::null_mut(),
        aspect: DVASPECT_CONTENT,
        lindex: -1,
        tymed: TYMED_HGLOBAL,
    }
}

/// Call `IDataObject::QueryGetData` for `cf`.
fn has_format(data: *mut c_void, cf: u16) -> bool {
    if data.is_null() {
        return false;
    }
    let f = format(cf);
    unsafe {
        let vtbl = *(data as *const *const DataObjectVtbl);
        ((*vtbl).query_get_data)(data, &f) == S_OK
    }
}

/// What a data object offers.
fn offer_of(data: *mut c_void) -> DragOffer {
    DragOffer {
        files: has_format(data, CF_HDROP),
        text: has_format(data, CF_UNICODETEXT),
        internal: false,
    }
}

/// The effect to answer: the app's verdict, as the first effect the source
/// allows of copy, move and link.
fn effect(allowed: u32) -> u32 {
    let accepted = CTX.with(|c| c.borrow().drop_accepted);
    if !accepted {
        return DROPEFFECT_NONE;
    }
    [DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_LINK]
        .into_iter()
        .find(|e| allowed & e != 0)
        .unwrap_or(DROPEFFECT_NONE)
}

unsafe extern "system" fn drag_enter(
    this: *mut DropTarget,
    data: *mut c_void,
    _keys: u32,
    pt: PointL,
    effect_out: *mut u32,
) -> Hresult {
    let hwnd = unsafe { (*this).hwnd };
    let offer = offer_of(data);
    // Until the app answers, take anything we can read.
    CTX.with(|c| c.borrow_mut().drop_accepted = !offer.is_empty());
    push_event(Event::DragEnter {
        position: client_point(hwnd, pt),
        offer,
    });
    if !effect_out.is_null() {
        unsafe { *effect_out = effect(*effect_out) };
    }
    S_OK
}

unsafe extern "system" fn drag_over(
    this: *mut DropTarget,
    _keys: u32,
    pt: PointL,
    effect_out: *mut u32,
) -> Hresult {
    let hwnd = unsafe { (*this).hwnd };
    let position = client_point(hwnd, pt);
    // OLE repeats DragOver while the pointer rests; only moves are news.
    let moved = CTX.with(|c| {
        let mut c = c.borrow_mut();
        let moved = c.drag_last != Some(position);
        c.drag_last = Some(position);
        moved
    });
    if moved {
        push_event(Event::DragOver { position });
    }
    if !effect_out.is_null() {
        unsafe { *effect_out = effect(*effect_out) };
    }
    S_OK
}

unsafe extern "system" fn drag_leave(_this: *mut DropTarget) -> Hresult {
    CTX.with(|c| c.borrow_mut().drag_last = None);
    push_event(Event::DragLeave);
    S_OK
}

unsafe extern "system" fn drop(
    this: *mut DropTarget,
    data: *mut c_void,
    _keys: u32,
    pt: PointL,
    effect_out: *mut u32,
) -> Hresult {
    let hwnd = unsafe { (*this).hwnd };
    CTX.with(|c| c.borrow_mut().drag_last = None);
    let position = client_point(hwnd, pt);
    let dropped = read_files(data).or_else(|| read_text(data));
    let answer = match &dropped {
        Some(_) if !effect_out.is_null() => effect(unsafe { *effect_out }),
        _ => DROPEFFECT_NONE,
    };
    if !effect_out.is_null() {
        unsafe { *effect_out = answer };
    }
    push_event(match dropped {
        Some(data) => Event::Drop { position, data },
        None => Event::DragLeave,
    });
    S_OK
}

/// Get `cf` from the data object as an `HGLOBAL` medium, run `read` on it and
/// release it.
fn with_medium<T>(data: *mut c_void, cf: u16, read: impl FnOnce(*mut c_void) -> T) -> Option<T> {
    if !has_format(data, cf) {
        return None;
    }
    let f = format(cf);
    let mut medium = StgMedium {
        tymed: 0,
        handle: std::ptr::null_mut(),
        unk_for_release: std::ptr::null_mut(),
    };
    unsafe {
        let vtbl = *(data as *const *const DataObjectVtbl);
        if ((*vtbl).get_data)(data, &f, &mut medium) != S_OK {
            return None;
        }
        let out = (medium.tymed == TYMED_HGLOBAL && !medium.handle.is_null())
            .then(|| read(medium.handle));
        ReleaseStgMedium(&mut medium);
        out
    }
}

fn read_files(data: *mut c_void) -> Option<DropData> {
    with_medium(data, CF_HDROP, hdrop_paths)
        .filter(|files| !files.is_empty())
        .map(DropData::Files)
}

fn read_text(data: *mut c_void) -> Option<DropData> {
    with_medium(data, CF_UNICODETEXT, |mem| unsafe {
        let ptr = GlobalLock(mem) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let max = GlobalSize(mem) / 2;
        let units = std::slice::from_raw_parts(ptr, max);
        let len = units.iter().position(|&u| u == 0).unwrap_or(max);
        let text = String::from_utf16_lossy(&units[..len]);
        GlobalUnlock(mem);
        Some(text)
    })
    .flatten()
    .map(DropData::Text)
}

/// The paths in an `HDROP`.
fn hdrop_paths(hdrop: *mut c_void) -> Vec<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, std::ptr::null_mut(), 0) };
    (0..count)
        .filter_map(|i| {
            let len = unsafe { DragQueryFileW(hdrop, i, std::ptr::null_mut(), 0) } as usize;
            let mut buf = vec![0u16; len + 1];
            let got = unsafe { DragQueryFileW(hdrop, i, buf.as_mut_ptr(), buf.len() as u32) };
            (got > 0).then(|| std::ffi::OsString::from_wide(&buf[..got as usize]).into())
        })
        .collect()
}

/// Handle `WM_DROPFILES`: the files in the `HDROP` in `wparam`, dropped at the
/// point it carries.
pub(super) fn drop_files(hdrop: *mut c_void) {
    let mut pt = PointL { x: 0, y: 0 };
    unsafe { DragQueryPoint(hdrop, &mut pt) };
    let files = hdrop_paths(hdrop);
    unsafe { DragFinish(hdrop) };
    if files.is_empty() {
        return;
    }
    let scale = current_scale();
    push_event(Event::Drop {
        position: Point::new(pt.x as f64 / scale, pt.y as f64 / scale),
        data: DropData::Files(files),
    });
}
