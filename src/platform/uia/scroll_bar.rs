//! Scroll bars under UI Automation: a node of
//! [`A11yRole::ScrollBar`](crate::platform::A11yRole::ScrollBar) is a
//! `ScrollBar` control with its `Orientation`, and the `RangeValue` pattern
//! (`IRangeValueProvider`): its value is the container's offset (logical
//! pixels from its start), from 0 to the largest; setting it scrolls the
//! container there, through the wheel the other scroll actions use (see
//! `platform::access::actions`).

use super::patterns::{deliver, iface, iface_qi};
use super::*;
use crate::platform::access::A11yScroll;

pub(super) const UIA_ScrollBarControlTypeId: i32 = 50014;
pub(super) const UIA_RangeValuePatternId: i32 = 10003;
const UIA_OrientationPropertyId: i32 = 30023;
const UIA_IsRangeValuePatternAvailablePropertyId: i32 = 30033;
const UIA_RangeValueValuePropertyId: i32 = 30047;
const UIA_RangeValueIsReadOnlyPropertyId: i32 = 30048;
const UIA_RangeValueMinimumPropertyId: i32 = 30049;
const UIA_RangeValueMaximumPropertyId: i32 = 30050;
const UIA_RangeValueLargeChangePropertyId: i32 = 30051;
const UIA_RangeValueSmallChangePropertyId: i32 = 30052;
/// `OrientationType_Horizontal` / `…Vertical`.
const ORIENTATION_HORIZONTAL: i32 = 1;
const ORIENTATION_VERTICAL: i32 = 2;

const IID_IRangeValueProvider: Guid = Guid(
    0x36dc_7aef,
    0x33e6,
    0x4691,
    [0xaf, 0xe1, 0x2b, 0xe7, 0x27, 0x4b, 0x3d, 0x33],
);

/// The scroll bar state of `p`'s node, if it is a scroll bar.
unsafe fn bar_of(p: *const Provider) -> Option<(&'static A11yNode, A11yScroll)> {
    let n: &'static A11yNode = unsafe { node_of(p) }?;
    Some((n, n.scroll_bar?))
}

/// What a scroll bar's range values are: `(value, minimum, maximum, large
/// change, small change)`.
pub(crate) fn range(s: &A11yScroll) -> (f64, f64, f64, f64, f64) {
    (
        s.offset,
        0.0,
        s.max.max(0.0),
        s.page(),
        crate::platform::event::SCROLL_LINE_PX,
    )
}

/// Answer scroll bar property `id` of `p` into `v`; false when `id` isn't
/// one of them (or `p` isn't a scroll bar).
pub(super) unsafe fn property(p: *const Provider, id: i32, v: &mut Variant) -> bool {
    let Some((_, s)) = (unsafe { bar_of(p) }) else {
        return false;
    };
    let (value, min, max, large, small) = range(&s);
    let r8 = |v: &mut Variant, x: f64| {
        v.vt = VT_R8;
        v.val = x.to_bits();
    };
    match id {
        UIA_OrientationPropertyId => {
            v.vt = VT_I4;
            let o = if s.horizontal {
                ORIENTATION_HORIZONTAL
            } else {
                ORIENTATION_VERTICAL
            };
            v.val = o as u32 as u64;
        }
        UIA_IsRangeValuePatternAvailablePropertyId => {
            v.vt = VT_BOOL;
            v.val = VARIANT_TRUE;
        }
        UIA_RangeValueIsReadOnlyPropertyId => {
            v.vt = VT_BOOL;
            v.val = 0;
        }
        UIA_RangeValueValuePropertyId => r8(v, value),
        UIA_RangeValueMinimumPropertyId => r8(v, min),
        UIA_RangeValueMaximumPropertyId => r8(v, max),
        UIA_RangeValueLargeChangePropertyId => r8(v, large),
        UIA_RangeValueSmallChangePropertyId => r8(v, small),
        _ => return false,
    }
    true
}

#[repr(C)]
pub(super) struct RangeVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    set_value: unsafe extern "system" fn(*mut c_void, f64) -> Hresult,
    get_value: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
    get_is_read_only: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    get_maximum: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
    get_minimum: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
    get_large_change: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
    get_small_change: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
}

iface!(
    range_vtable,
    IID_IRangeValueProvider,
    range_owner,
    range_qi,
    range_add_ref,
    range_release
);

/// `SetValue`: scroll the bar's container to that offset.
unsafe extern "system" fn set_value(this: *mut c_void, value: f64) -> Hresult {
    unsafe {
        let p = range_owner(this);
        let Some((n, s)) = bar_of(p) else {
            return E_FAIL;
        };
        if !value.is_finite() {
            return E_INVALIDARG;
        }
        if let Some(events) = n.scroll_by_events(s.to(value)) {
            deliver(p, events);
        }
        S_OK
    }
}

/// One of the pattern's `double` properties.
unsafe fn double(
    this: *mut c_void,
    ret: *mut f64,
    pick: fn((f64, f64, f64, f64, f64)) -> f64,
) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let Some((_, s)) = bar_of(range_owner(this)) else {
            return E_FAIL;
        };
        *ret = pick(range(&s));
        S_OK
    }
}

unsafe extern "system" fn get_value(this: *mut c_void, r: *mut f64) -> Hresult {
    unsafe { double(this, r, |r| r.0) }
}
unsafe extern "system" fn get_minimum(this: *mut c_void, r: *mut f64) -> Hresult {
    unsafe { double(this, r, |r| r.1) }
}
unsafe extern "system" fn get_maximum(this: *mut c_void, r: *mut f64) -> Hresult {
    unsafe { double(this, r, |r| r.2) }
}
unsafe extern "system" fn get_large_change(this: *mut c_void, r: *mut f64) -> Hresult {
    unsafe { double(this, r, |r| r.3) }
}
unsafe extern "system" fn get_small_change(this: *mut c_void, r: *mut f64) -> Hresult {
    unsafe { double(this, r, |r| r.4) }
}
unsafe extern "system" fn get_is_read_only(_this: *mut c_void, r: *mut i32) -> Hresult {
    if r.is_null() {
        return E_POINTER;
    }
    unsafe { *r = 0 };
    S_OK
}

pub(super) static RANGE_VTBL: RangeVtbl = RangeVtbl {
    query_interface: range_qi,
    add_ref: range_add_ref,
    release: range_release,
    set_value,
    get_value,
    get_is_read_only,
    get_maximum,
    get_minimum,
    get_large_change,
    get_small_change,
};
