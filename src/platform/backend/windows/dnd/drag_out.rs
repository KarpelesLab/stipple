//! Dragging out of the app on Windows: OLE `DoDragDrop` with a hand-written
//! `IDataObject` (`CF_HDROP` for files, `CF_UNICODETEXT` for text) and
//! `IDropSource`.
//!
//! When the app hands a drag to the window ([`Window::start_drag`]), the
//! message loop runs `DoDragDrop` once the handler returns: OLE's modal loop
//! then owns the mouse until the button comes up (drop) or Escape is pressed
//! (cancel), as `IDropSource::QueryContinueDrag` decides; the cursor is OLE's
//! own. A target reads the data through `GetData`, each time on a fresh
//! `HGLOBAL` it frees. While the drag runs our window's messages still
//! dispatch but the app's handler waits: a drag back over our own window
//! queues its events, delivered when `DoDragDrop` returns.
//!
//! [`Window::start_drag`]: crate::platform::Window::start_drag

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use super::{
    CF_HDROP, CF_UNICODETEXT, DROPEFFECT_COPY, DVASPECT_CONTENT, E_NOINTERFACE, E_POINTER,
    FormatEtc, Guid, Hresult, IID_IUNKNOWN, S_OK, StgMedium, TYMED_HGLOBAL, format,
};
use crate::platform::DropData;
use crate::platform::event::{hdrop_bytes, unicode_text_bytes};

const E_NOTIMPL: Hresult = 0x8000_4001_u32 as i32;
const E_OUTOFMEMORY: Hresult = 0x8007_000E_u32 as i32;
const DV_E_FORMATETC: Hresult = 0x8004_0064_u32 as i32;
const OLE_E_ADVISENOTSUPPORTED: Hresult = 0x8004_0003_u32 as i32;
const DRAGDROP_S_DROP: Hresult = 0x0004_0100;
const DRAGDROP_S_CANCEL: Hresult = 0x0004_0101;
const DRAGDROP_S_USEDEFAULTCURSORS: Hresult = 0x0004_0102;
const DATADIR_GET: u32 = 1;
const MK_LBUTTON: u32 = 1;
const GMEM_MOVEABLE: u32 = 0x0002;
const GMEM_ZEROINIT: u32 = 0x0040;

const IID_IDATAOBJECT: Guid = Guid(0x0000_010E, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IDROPSOURCE: Guid = Guid(0x0000_0121, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

#[link(name = "ole32")]
unsafe extern "system" {
    fn DoDragDrop(data: *mut c_void, source: *mut c_void, ok: u32, effect: *mut u32) -> Hresult;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHCreateStdEnumFmtEtc(n: u32, formats: *const FormatEtc, out: *mut *mut c_void) -> Hresult;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
    fn GlobalLock(mem: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(mem: *mut c_void) -> i32;
    fn GlobalFree(mem: *mut c_void) -> *mut c_void;
}

type Unknown<T> = unsafe extern "system" fn(*mut T) -> u32;
type QueryInterface<T> =
    unsafe extern "system" fn(*mut T, *const Guid, *mut *mut c_void) -> Hresult;

#[repr(C)]
struct DataObjectVtbl {
    query_interface: QueryInterface<DataObject>,
    add_ref: Unknown<DataObject>,
    release: Unknown<DataObject>,
    get_data:
        unsafe extern "system" fn(*mut DataObject, *const FormatEtc, *mut StgMedium) -> Hresult,
    get_data_here:
        unsafe extern "system" fn(*mut DataObject, *const FormatEtc, *mut StgMedium) -> Hresult,
    query_get_data: unsafe extern "system" fn(*mut DataObject, *const FormatEtc) -> Hresult,
    get_canonical_format_etc:
        unsafe extern "system" fn(*mut DataObject, *const FormatEtc, *mut FormatEtc) -> Hresult,
    set_data: unsafe extern "system" fn(
        *mut DataObject,
        *const FormatEtc,
        *mut StgMedium,
        i32,
    ) -> Hresult,
    enum_format_etc: unsafe extern "system" fn(*mut DataObject, u32, *mut *mut c_void) -> Hresult,
    d_advise: unsafe extern "system" fn(
        *mut DataObject,
        *const FormatEtc,
        u32,
        *mut c_void,
        *mut u32,
    ) -> Hresult,
    d_unadvise: unsafe extern "system" fn(*mut DataObject, u32) -> Hresult,
    enum_d_advise: unsafe extern "system" fn(*mut DataObject, *mut *mut c_void) -> Hresult,
}

/// Our `IDataObject`: one format, its bytes served on demand.
#[repr(C)]
struct DataObject {
    vtbl: &'static DataObjectVtbl,
    refs: AtomicU32,
    cf: u16,
    bytes: Vec<u8>,
}

#[repr(C)]
struct DropSourceVtbl {
    query_interface: QueryInterface<DropSource>,
    add_ref: Unknown<DropSource>,
    release: Unknown<DropSource>,
    query_continue_drag: unsafe extern "system" fn(*mut DropSource, i32, u32) -> Hresult,
    give_feedback: unsafe extern "system" fn(*mut DropSource, u32) -> Hresult,
}

#[repr(C)]
struct DropSource {
    vtbl: &'static DropSourceVtbl,
    refs: AtomicU32,
}

static DATA_VTBL: DataObjectVtbl = DataObjectVtbl {
    query_interface: data_query_interface,
    add_ref: data_add_ref,
    release: data_release,
    get_data,
    get_data_here,
    query_get_data,
    get_canonical_format_etc,
    set_data,
    enum_format_etc,
    d_advise,
    d_unadvise,
    enum_d_advise,
};

static SOURCE_VTBL: DropSourceVtbl = DropSourceVtbl {
    query_interface: source_query_interface,
    add_ref: source_add_ref,
    release: source_release,
    query_continue_drag,
    give_feedback,
};

/// The format and bytes `data` is dragged as.
fn encode(data: &DropData) -> Option<(u16, Vec<u8>)> {
    use std::os::windows::ffi::OsStrExt;
    match data {
        DropData::Files(paths) if !paths.is_empty() => {
            let wide: Vec<Vec<u16>> = paths
                .iter()
                .map(|p| p.as_os_str().encode_wide().collect())
                .collect();
            Some((CF_HDROP, hdrop_bytes(&wide)))
        }
        DropData::Text(text) => Some((CF_UNICODETEXT, unicode_text_bytes(text))),
        _ => None,
    }
}

/// Run a drag of `data` out of the app with `DoDragDrop`, until it drops or
/// is cancelled. Returns whether a target took it.
pub(in super::super) fn run(data: &DropData) -> bool {
    let Some((cf, bytes)) = encode(data) else {
        return false;
    };
    let object = Box::into_raw(Box::new(DataObject {
        vtbl: &DATA_VTBL,
        refs: AtomicU32::new(1),
        cf,
        bytes,
    }));
    let source = Box::into_raw(Box::new(DropSource {
        vtbl: &SOURCE_VTBL,
        refs: AtomicU32::new(1),
    }));
    let mut effect = 0u32;
    let hr = unsafe { DoDragDrop(object.cast(), source.cast(), DROPEFFECT_COPY, &mut effect) };
    // Targets that kept a reference keep the object alive past this.
    unsafe {
        data_release(object);
        source_release(source);
    }
    hr == DRAGDROP_S_DROP && effect != 0
}

fn matches(object: &DataObject, f: &FormatEtc) -> bool {
    f.cf_format == object.cf && f.tymed & TYMED_HGLOBAL != 0 && f.aspect == DVASPECT_CONTENT
}

unsafe extern "system" fn data_query_interface(
    this: *mut DataObject,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> Hresult {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_IDATAOBJECT {
        unsafe {
            data_add_ref(this);
            *out = this.cast();
        }
        S_OK
    } else {
        unsafe { *out = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn data_add_ref(this: *mut DataObject) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn data_release(this: *mut DataObject) -> u32 {
    let left = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1 };
    if left == 0 {
        drop(unsafe { Box::from_raw(this) });
    }
    left
}

unsafe extern "system" fn get_data(
    this: *mut DataObject,
    f: *const FormatEtc,
    medium: *mut StgMedium,
) -> Hresult {
    if f.is_null() || medium.is_null() {
        return E_POINTER;
    }
    let object = unsafe { &*this };
    if !matches(object, unsafe { &*f }) {
        return DV_E_FORMATETC;
    }
    // A fresh copy each time: the receiver frees it (`ReleaseStgMedium`).
    let mem = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, object.bytes.len()) };
    if mem.is_null() {
        return E_OUTOFMEMORY;
    }
    unsafe {
        let ptr = GlobalLock(mem) as *mut u8;
        if ptr.is_null() {
            GlobalFree(mem);
            return E_OUTOFMEMORY;
        }
        std::ptr::copy_nonoverlapping(object.bytes.as_ptr(), ptr, object.bytes.len());
        GlobalUnlock(mem);
        *medium = StgMedium {
            tymed: TYMED_HGLOBAL,
            handle: mem,
            unk_for_release: std::ptr::null_mut(),
        };
    }
    S_OK
}

unsafe extern "system" fn get_data_here(
    _this: *mut DataObject,
    _f: *const FormatEtc,
    _medium: *mut StgMedium,
) -> Hresult {
    E_NOTIMPL
}

unsafe extern "system" fn query_get_data(this: *mut DataObject, f: *const FormatEtc) -> Hresult {
    if f.is_null() {
        return E_POINTER;
    }
    if matches(unsafe { &*this }, unsafe { &*f }) {
        S_OK
    } else {
        DV_E_FORMATETC
    }
}

unsafe extern "system" fn get_canonical_format_etc(
    _this: *mut DataObject,
    _f: *const FormatEtc,
    out: *mut FormatEtc,
) -> Hresult {
    if !out.is_null() {
        unsafe { (*out).ptd = std::ptr::null_mut() };
    }
    E_NOTIMPL
}

unsafe extern "system" fn set_data(
    _this: *mut DataObject,
    _f: *const FormatEtc,
    _medium: *mut StgMedium,
    _release: i32,
) -> Hresult {
    // The shell offers extras (drag images, drop descriptions); we take none.
    E_NOTIMPL
}

unsafe extern "system" fn enum_format_etc(
    this: *mut DataObject,
    direction: u32,
    out: *mut *mut c_void,
) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    if direction != DATADIR_GET {
        unsafe { *out = std::ptr::null_mut() };
        return E_NOTIMPL;
    }
    let f = format(unsafe { (*this).cf });
    unsafe { SHCreateStdEnumFmtEtc(1, &f, out) }
}

unsafe extern "system" fn d_advise(
    _this: *mut DataObject,
    _f: *const FormatEtc,
    _flags: u32,
    _sink: *mut c_void,
    _conn: *mut u32,
) -> Hresult {
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn d_unadvise(_this: *mut DataObject, _conn: u32) -> Hresult {
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn enum_d_advise(_this: *mut DataObject, _out: *mut *mut c_void) -> Hresult {
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn source_query_interface(
    this: *mut DropSource,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> Hresult {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    let iid = unsafe { *iid };
    if iid == IID_IUNKNOWN || iid == IID_IDROPSOURCE {
        unsafe {
            source_add_ref(this);
            *out = this.cast();
        }
        S_OK
    } else {
        unsafe { *out = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn source_add_ref(this: *mut DropSource) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn source_release(this: *mut DropSource) -> u32 {
    let left = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1 };
    if left == 0 {
        drop(unsafe { Box::from_raw(this) });
    }
    left
}

/// Escape cancels; the left button coming up drops.
unsafe extern "system" fn query_continue_drag(
    _this: *mut DropSource,
    escape: i32,
    keys: u32,
) -> Hresult {
    if escape != 0 {
        DRAGDROP_S_CANCEL
    } else if keys & MK_LBUTTON == 0 {
        DRAGDROP_S_DROP
    } else {
        S_OK
    }
}

unsafe extern "system" fn give_feedback(_this: *mut DropSource, _effect: u32) -> Hresult {
    DRAGDROP_S_USEDEFAULTCURSORS
}
