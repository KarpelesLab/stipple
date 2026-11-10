//! The COM plumbing the TSF bridge shares: GUIDs, `HRESULT`s, an owning
//! interface pointer, and calls through the vtables of the TSF objects we use
//! (by slot number: each interface's methods in declaration order, after
//! `IUnknown`'s three).

use std::ffi::c_void;
use std::ptr::NonNull;

pub(super) type Hresult = i32;

pub(super) const S_OK: Hresult = 0;
pub(super) const E_NOTIMPL: Hresult = 0x8000_4001_u32 as i32;
pub(super) const E_NOINTERFACE: Hresult = 0x8000_4002_u32 as i32;
pub(super) const E_POINTER: Hresult = 0x8000_4003_u32 as i32;
pub(super) const E_FAIL: Hresult = 0x8000_4005_u32 as i32;
pub(super) const E_UNEXPECTED: Hresult = 0x8000_FFFF_u32 as i32;
pub(super) const E_INVALIDARG: Hresult = 0x8007_0057_u32 as i32;
pub(super) const CONNECT_E_NOCONNECTION: Hresult = 0x8004_0200_u32 as i32;
pub(super) const CONNECT_E_ADVISELIMIT: Hresult = 0x8004_0201_u32 as i32;
pub(super) const TS_E_INVALIDPOS: Hresult = 0x8004_0200_u32 as i32;
pub(super) const TS_E_NOLOCK: Hresult = 0x8004_0201_u32 as i32;
pub(super) const TS_E_SYNCHRONOUS: Hresult = 0x8004_0208_u32 as i32;
pub(super) const TS_S_ASYNC: Hresult = 0x0004_0300;
/// `RPC_E_CHANGED_MODE`: COM is already multi-threaded on this thread.
pub(super) const RPC_E_CHANGED_MODE: Hresult = 0x8001_0106_u32 as i32;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Guid(pub u32, pub u16, pub u16, pub [u8; 8]);

pub(super) const IID_IUNKNOWN: Guid = Guid(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub(super) const CLSID_TF_THREADMGR: Guid = Guid(
    0x529A_9E6B,
    0x6587,
    0x4F23,
    [0xAB, 0x9E, 0x9C, 0x7D, 0x68, 0x3E, 0x3C, 0x50],
);
pub(super) const IID_ITFTHREADMGR: Guid = Guid(
    0xAA80_E801,
    0x2021,
    0x11D2,
    [0x93, 0xE0, 0x00, 0x60, 0xB0, 0x67, 0xB8, 0x6E],
);
pub(super) const CLSID_TF_CATEGORYMGR: Guid = Guid(
    0xA4B5_44A1,
    0x438D,
    0x4B41,
    [0x93, 0x25, 0x86, 0x95, 0x23, 0xE2, 0xD6, 0xC7],
);
pub(super) const IID_ITFCATEGORYMGR: Guid = Guid(
    0xC3AC_EFB5,
    0xF69D,
    0x4905,
    [0x93, 0x8F, 0xFC, 0xAD, 0xCF, 0x4B, 0xE8, 0x30],
);
pub(super) const CLSID_TF_DISPLAYATTRIBUTEMGR: Guid = Guid(
    0x3CE7_4DE4,
    0x53D3,
    0x4D74,
    [0x8B, 0x83, 0x43, 0x1B, 0x38, 0x28, 0xBA, 0x53],
);
pub(super) const IID_ITFDISPLAYATTRIBUTEMGR: Guid = Guid(
    0x8DED_7393,
    0x5DB1,
    0x475C,
    [0x9E, 0x71, 0xA3, 0x91, 0x11, 0xB0, 0xFF, 0x67],
);
pub(super) const IID_ITEXTSTOREACP: Guid = Guid(
    0x2888_8FE3,
    0xC2A0,
    0x483A,
    [0xA3, 0xEA, 0x8C, 0xB1, 0xCE, 0x51, 0xFF, 0x3D],
);
pub(super) const IID_ITEXTSTOREACPSINK: Guid = Guid(
    0x22D4_4C94,
    0xA419,
    0x4542,
    [0xA2, 0x72, 0xAE, 0x26, 0x09, 0x3E, 0xCE, 0xCF],
);
pub(super) const IID_ITFCONTEXTOWNERCOMPOSITIONSINK: Guid = Guid(
    0x5F20_AA40,
    0xB57A,
    0x4F34,
    [0x96, 0xAB, 0x35, 0x76, 0xF3, 0x77, 0xCC, 0x79],
);
pub(super) const IID_ITFTEXTEDITSINK: Guid = Guid(
    0x8127_D409,
    0xCCD3,
    0x4683,
    [0x96, 0x7A, 0xB4, 0x3D, 0x5B, 0x48, 0x2B, 0xF7],
);
pub(super) const IID_ITFSOURCE: Guid = Guid(
    0x4EA4_8A35,
    0x60AE,
    0x446F,
    [0x8F, 0xD6, 0xE6, 0xA8, 0xD8, 0x24, 0x59, 0xF7],
);
pub(super) const IID_ITFRANGEACP: Guid = Guid(
    0x057A_6296,
    0x029B,
    0x4154,
    [0xB7, 0x9A, 0x0D, 0x46, 0x1D, 0x4E, 0xA9, 0x4C],
);
pub(super) const IID_ITFCONTEXTOWNERCOMPOSITIONSERVICES: Guid = Guid(
    0x8646_2810,
    0x593B,
    0x4916,
    [0x97, 0x64, 0x19, 0xC0, 0x8E, 0x9C, 0xE1, 0x10],
);
pub(super) const IID_ITFCONTEXTCOMPOSITION: Guid = Guid(
    0xD40C_8AAE,
    0xAC92,
    0x4FC7,
    [0x9A, 0x11, 0x0E, 0xE0, 0xE2, 0x3A, 0xA3, 0x9B],
);
pub(super) const IID_ITFEDITSESSION: Guid = Guid(
    0xAA80_E803,
    0x2021,
    0x11D2,
    [0x93, 0xE0, 0x00, 0x60, 0xB0, 0x67, 0xB8, 0x6E],
);
/// The display attribute property (`GUID_PROP_ATTRIBUTE`).
pub(super) const GUID_PROP_ATTRIBUTE: Guid = Guid(
    0x34B4_5670,
    0x7526,
    0x11D2,
    [0xA1, 0x47, 0x00, 0x10, 0x5A, 0x27, 0x99, 0xB5],
);

const CLSCTX_INPROC_SERVER: u32 = 0x1;
const COINIT_APARTMENTTHREADED: u32 = 0x2;
const VT_I4: u16 = 3;

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> Hresult;
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: *mut c_void,
        context: u32,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> Hresult;
}

#[link(name = "oleaut32")]
unsafe extern "system" {
    fn VariantClear(v: *mut Variant) -> Hresult;
}

/// A `VARIANT`: the type, three reserved words, and a value union two
/// pointers wide.
#[repr(C)]
pub(super) struct Variant {
    vt: u16,
    reserved: [u16; 3],
    val: [usize; 2],
}

/// `TF_DA_COLOR`.
#[repr(C)]
#[derive(Default)]
struct DaColor {
    kind: i32,
    value: u32,
}

/// `TF_DISPLAYATTRIBUTE`.
#[repr(C)]
#[derive(Default)]
struct DisplayAttribute {
    text: DaColor,
    background: DaColor,
    line_style: i32,
    bold_line: i32,
    line: DaColor,
    attr: i32,
}

/// Make this thread a single-threaded apartment, as TSF needs. Fails if it
/// is already multi-threaded.
pub(super) fn init_apartment() -> bool {
    let hr = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
    hr >= 0 && hr != RPC_E_CHANGED_MODE
}

/// An interface pointer we hold a reference on, released on drop.
#[derive(Debug)]
pub(super) struct Com(NonNull<c_void>);

impl Com {
    /// Take ownership of a reference already counted (an out-parameter).
    pub(super) fn from_raw(p: *mut c_void) -> Option<Com> {
        NonNull::new(p).map(Com)
    }

    /// Add a reference to `p` and own it.
    pub(super) unsafe fn add_ref(p: *mut c_void) -> Option<Com> {
        let c = Com::from_raw(p)?;
        unsafe { slot::<unsafe extern "system" fn(*mut c_void) -> u32>(p, 1)(p) };
        Some(c)
    }

    pub(super) fn ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    /// Give the reference away.
    pub(super) fn into_raw(self) -> *mut c_void {
        let p = self.ptr();
        std::mem::forget(self);
        p
    }

    /// `CoCreateInstance` (in process).
    pub(super) fn create(clsid: &Guid, iid: &Guid) -> Result<Com, Hresult> {
        let mut out = std::ptr::null_mut();
        let hr = unsafe {
            CoCreateInstance(
                clsid,
                std::ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                iid,
                &mut out,
            )
        };
        if hr < 0 {
            return Err(hr);
        }
        Com::from_raw(out).ok_or(E_POINTER)
    }

    /// `QueryInterface`.
    pub(super) fn query(&self, iid: &Guid) -> Option<Com> {
        let mut out = std::ptr::null_mut();
        let hr = unsafe {
            self.call::<unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult>(0)(
                self.ptr(),
                iid,
                &mut out,
            )
        };
        if hr < 0 {
            return None;
        }
        Com::from_raw(out)
    }

    /// The method in vtable slot `n`, as `F` (an `extern "system" fn` whose
    /// first parameter is the interface pointer).
    pub(super) unsafe fn call<F: Copy>(&self, n: usize) -> F {
        unsafe { slot(self.ptr(), n) }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        let p = self.ptr();
        unsafe { slot::<unsafe extern "system" fn(*mut c_void) -> u32>(p, 2)(p) };
    }
}

/// Slot `n` of `obj`'s vtable, as the function pointer type `F`.
pub(super) unsafe fn slot<F: Copy>(obj: *mut c_void, n: usize) -> F {
    debug_assert_eq!(size_of::<F>(), size_of::<usize>());
    unsafe {
        let vtbl = *(obj as *const *const usize);
        std::mem::transmute_copy::<usize, F>(&*vtbl.add(n))
    }
}

type Out = *mut *mut c_void;

/// Read an interface out-parameter: `f` fills it and returns an `HRESULT`.
pub(super) fn out(f: impl FnOnce(Out) -> Hresult) -> Option<Com> {
    let mut p = std::ptr::null_mut();
    if f(&mut p) < 0 {
        if !p.is_null() {
            drop(Com::from_raw(p));
        }
        return None;
    }
    Com::from_raw(p)
}

/// `ITfRangeACP::GetExtent` of an `ITfRange`: its start and end.
pub(super) fn range_extent(range: &Com) -> Option<(usize, usize)> {
    let acp = range.query(&IID_ITFRANGEACP)?;
    let (mut start, mut len) = (0i32, 0i32);
    // ITfRange has 22 methods (slots 3..=24); GetExtent follows.
    let hr = unsafe {
        acp.call::<unsafe extern "system" fn(*mut c_void, *mut i32, *mut i32) -> Hresult>(25)(
            acp.ptr(),
            &mut start,
            &mut len,
        )
    };
    if hr < 0 || start < 0 || len < 0 {
        return None;
    }
    Some((start as usize, start as usize + len as usize))
}

/// `ITfCompositionView::GetRange`.
pub(super) fn view_range(view: &Com) -> Option<Com> {
    out(|p| unsafe {
        view.call::<unsafe extern "system" fn(*mut c_void, Out) -> Hresult>(4)(view.ptr(), p)
    })
}

/// The managers that turn a display attribute atom into its
/// `TF_DA_ATTR_INFO`.
#[derive(Debug)]
pub(super) struct AttributeLookup {
    pub(super) category: Com,
    pub(super) display: Com,
}

impl AttributeLookup {
    pub(super) fn new() -> Option<AttributeLookup> {
        Some(AttributeLookup {
            category: Com::create(&CLSID_TF_CATEGORYMGR, &IID_ITFCATEGORYMGR).ok()?,
            display: Com::create(&CLSID_TF_DISPLAYATTRIBUTEMGR, &IID_ITFDISPLAYATTRIBUTEMGR)
                .ok()?,
        })
    }

    /// The `TF_DA_ATTR_INFO` of a `GUID_PROP_ATTRIBUTE` value (a `TfGuidAtom`).
    fn attr_of(&self, atom: u32) -> Option<i32> {
        let mut guid = Guid(0, 0, 0, [0; 8]);
        // ITfCategoryMgr::GetGUID.
        let hr = unsafe {
            self.category
                .call::<unsafe extern "system" fn(*mut c_void, u32, *mut Guid) -> Hresult>(15)(
                self.category.ptr(),
                atom,
                &mut guid,
            )
        };
        if hr < 0 {
            return None;
        }
        // ITfDisplayAttributeMgr::GetDisplayAttributeInfo.
        let info = out(|p| unsafe {
            self.display.call::<unsafe extern "system" fn(
                *mut c_void,
                *const Guid,
                Out,
                *mut Guid,
            ) -> Hresult>(5)(self.display.ptr(), &guid, p, std::ptr::null_mut())
        })?;
        let mut da = DisplayAttribute::default();
        // ITfDisplayAttributeInfo::GetAttributeInfo.
        let hr = unsafe {
            info.call::<unsafe extern "system" fn(*mut c_void, *mut DisplayAttribute) -> Hresult>(5)(
                info.ptr(),
                &mut da,
            )
        };
        (hr >= 0).then_some(da.attr)
    }

    /// The display attributes over `range` of `context` (an `ITfContext`),
    /// read with the edit cookie `ec`: each run's extent and its
    /// `TF_DA_ATTR_INFO`.
    pub(super) fn read(&self, context: &Com, ec: u32, range: &Com) -> Vec<(usize, usize, i32)> {
        let mut found = Vec::new();
        // ITfContext::GetProperty.
        let Some(prop) = out(|p| unsafe {
            context.call::<unsafe extern "system" fn(*mut c_void, *const Guid, Out) -> Hresult>(12)(
                context.ptr(),
                &GUID_PROP_ATTRIBUTE,
                p,
            )
        }) else {
            return found;
        };
        // ITfReadOnlyProperty::EnumRanges.
        let Some(runs) = out(|p| unsafe {
            prop.call::<unsafe extern "system" fn(*mut c_void, u32, Out, *mut c_void) -> Hresult>(4)(
                prop.ptr(),
                ec,
                p,
                range.ptr(),
            )
        }) else {
            return found;
        };
        for _ in 0..256 {
            let mut fetched = 0u32;
            // IEnumTfRanges::Next.
            let Some(run) = out(|p| unsafe {
                runs.call::<unsafe extern "system" fn(*mut c_void, u32, Out, *mut u32) -> Hresult>(
                    4,
                )(runs.ptr(), 1, p, &mut fetched)
            }) else {
                break;
            };
            if fetched != 1 {
                break;
            }
            let mut v = Variant {
                vt: 0,
                reserved: [0; 3],
                val: [0; 2],
            };
            // ITfReadOnlyProperty::GetValue.
            let hr = unsafe {
                prop.call::<unsafe extern "system" fn(
                    *mut c_void,
                    u32,
                    *mut c_void,
                    *mut Variant,
                ) -> Hresult>(5)(prop.ptr(), ec, run.ptr(), &mut v)
            };
            let atom = (hr >= 0 && v.vt == VT_I4).then_some(v.val[0] as u32);
            unsafe { VariantClear(&mut v) };
            if let (Some(attr), Some((s, e))) =
                (atom.and_then(|a| self.attr_of(a)), range_extent(&run))
            {
                found.push((s, e, attr));
            }
        }
        found
    }
}
