//! The control patterns besides Invoke and Text, each an interface of the
//! provider with a vtable of its own (`IValueProvider`, `IToggleProvider`,
//! `ISelectionProvider`, `ISelectionItemProvider`, `IExpandCollapseProvider`,
//! `IScrollProvider`, `IScrollItemProvider`), and their properties. They read
//! their node's state from the tree the providers were built from, and act
//! through the events a click, typed text or a wheel would deliver (see
//! `platform::access::actions`), queued for the window.

use super::*;
use crate::platform::event::Event;

/// For the interface whose pointer is the provider's field `$field`: the
/// owner lookup and the `IUnknown` methods (QueryInterface answers
/// `IUnknown` and `$iid` with this interface, as the Invoke pattern's does).
macro_rules! iface {
    ($field:ident, $iid:expr, $owner:ident, $qi:ident, $add:ident, $rel:ident) => {
        pub(super) unsafe fn $owner(this: *mut c_void) -> *mut Provider {
            unsafe {
                (this as *mut u8).sub(core::mem::offset_of!(Provider, $field)) as *mut Provider
            }
        }
        unsafe extern "system" fn $qi(
            this: *mut c_void,
            iid: *const Guid,
            ppv: *mut *mut c_void,
        ) -> Hresult {
            unsafe { iface_qi(this, iid, ppv, &$iid, $add) }
        }
        unsafe extern "system" fn $add(this: *mut c_void) -> u32 {
            unsafe { add_ref($owner(this) as *mut c_void) }
        }
        unsafe extern "system" fn $rel(this: *mut c_void) -> u32 {
            unsafe { release($owner(this) as *mut c_void) }
        }
    };
}
pub(super) use iface;

/// `QueryInterface` on a pattern interface: itself for `IUnknown` and
/// `iid`, nothing else.
pub(super) unsafe fn iface_qi(
    this: *mut c_void,
    iid: *const Guid,
    ppv: *mut *mut c_void,
    want: &Guid,
    add: unsafe extern "system" fn(*mut c_void) -> u32,
) -> Hresult {
    unsafe {
        if ppv.is_null() {
            return E_POINTER;
        }
        let id = if iid.is_null() { None } else { Some(*iid) };
        if id != Some(IID_IUnknown) && id.as_ref() != Some(want) {
            *ppv = core::ptr::null_mut();
            return E_NOINTERFACE;
        }
        *ppv = this;
        add(this);
    }
    S_OK
}

const IID_IValueProvider: Guid = Guid(
    0xc793_5180,
    0x6fb3,
    0x4201,
    [0xb1, 0x74, 0x7d, 0xf7, 0x3a, 0xdb, 0xf6, 0x4a],
);
const IID_IToggleProvider: Guid = Guid(
    0x56d0_0bd0,
    0xc4f4,
    0x433c,
    [0xa8, 0x36, 0x1a, 0x52, 0xa5, 0x7e, 0x08, 0x92],
);
const IID_ISelectionProvider: Guid = Guid(
    0xfb8b_03af,
    0x3bdf,
    0x48d4,
    [0xbd, 0x36, 0x1a, 0x65, 0x79, 0x3b, 0xe1, 0x68],
);
const IID_ISelectionItemProvider: Guid = Guid(
    0x2aca_d808,
    0xb2d4,
    0x452d,
    [0xa4, 0x07, 0x91, 0xff, 0x1a, 0xd1, 0x67, 0xb2],
);
const IID_IExpandCollapseProvider: Guid = Guid(
    0xd847_d3a5,
    0xcab0,
    0x4a98,
    [0x8c, 0x32, 0xec, 0xb4, 0x5c, 0x59, 0xad, 0x24],
);
const IID_IScrollProvider: Guid = Guid(
    0xb38b_8077,
    0x1fc3,
    0x42a5,
    [0x8c, 0xae, 0xd4, 0x0c, 0x22, 0x15, 0x05, 0x5a],
);
const IID_IScrollItemProvider: Guid = Guid(
    0x2360_c714,
    0x4bf1,
    0x4b26,
    [0xba, 0x65, 0x9b, 0x21, 0x31, 0x61, 0x27, 0xeb],
);

// Pattern properties (UIAutomationClient.h).
const UIA_ValueValuePropertyId: i32 = logic::VALUE_VALUE;
const UIA_ScrollHorizontalScrollPercentPropertyId: i32 = logic::SCROLL_HORIZONTAL_PERCENT;
const UIA_ScrollVerticalScrollPercentPropertyId: i32 = logic::SCROLL_VERTICAL_PERCENT;
const UIA_ExpandCollapseExpandCollapseStatePropertyId: i32 = logic::EXPAND_COLLAPSE_STATE;
const UIA_SelectionItemIsSelectedPropertyId: i32 = logic::SELECTION_ITEM_IS_SELECTED;
const UIA_ToggleToggleStatePropertyId: i32 = logic::TOGGLE_STATE;

thread_local! {
    /// What the patterns of a tree outside any window did (the self-test
    /// reads it back).
    static RECORDED: core::cell::RefCell<Vec<Event>> =
        const { core::cell::RefCell::new(Vec::new()) };
}

/// Hand `events` to `p`'s window (queued, delivered after UIA's call
/// returns), or record them for a tree outside any window.
pub(super) unsafe fn deliver(p: *const Provider, events: Vec<Event>) {
    let hwnd = unsafe { (*p).hwnd };
    if hwnd.is_null() {
        RECORDED.with(|r| r.borrow_mut().extend(events));
    } else {
        crate::platform::backend::windows::queue_a11y_events(hwnd, events);
    }
}

/// The events recorded so far (see [`deliver`]), taken.
pub(super) fn take_recorded() -> Vec<Event> {
    RECORDED.with(|r| std::mem::take(&mut *r.borrow_mut()))
}

fn bool_var(v: &mut Variant, on: bool) {
    v.vt = VT_BOOL;
    v.val = if on { VARIANT_TRUE } else { 0 };
}

fn i4_var(v: &mut Variant, n: i32) {
    v.vt = VT_I4;
    v.val = n as u32 as u64;
}

fn r8_var(v: &mut Variant, x: f64) {
    v.vt = VT_R8;
    v.val = x.to_bits();
}

/// A new BSTR holding `s`.
pub(super) fn bstr(s: &str) -> *mut u16 {
    let wide: Vec<u16> = s.encode_utf16().filter(|&c| c != 0).chain([0]).collect();
    unsafe { SysAllocString(wide.as_ptr()) }
}

/// Answer the pattern property `id` of `p` into `v`; false when `id` isn't
/// one of them.
pub(super) unsafe fn property(p: *const Provider, id: i32, v: &mut Variant) -> bool {
    let has = unsafe { (*p).patterns };
    let Some(n) = (unsafe { node_of(p) }) else {
        return false;
    };
    match id {
        UIA_IsValuePatternAvailablePropertyId => bool_var(v, has.value),
        UIA_IsTogglePatternAvailablePropertyId => bool_var(v, has.toggle),
        UIA_IsSelectionPatternAvailablePropertyId => bool_var(v, has.selection),
        UIA_IsSelectionItemPatternAvailablePropertyId => bool_var(v, has.selection_item),
        UIA_IsExpandCollapsePatternAvailablePropertyId => bool_var(v, has.expand_collapse),
        UIA_IsScrollPatternAvailablePropertyId => bool_var(v, has.scroll),
        UIA_IsScrollItemPatternAvailablePropertyId => bool_var(v, unsafe { (*p).scroll_item }),
        UIA_IsTextPatternAvailablePropertyId => bool_var(v, has.text),
        UIA_ValueValuePropertyId if has.value => {
            v.vt = VT_BSTR;
            v.val = bstr(&logic::uia_value(n)) as u64;
        }
        UIA_ValueIsReadOnlyPropertyId if has.value => bool_var(v, false),
        UIA_ToggleToggleStatePropertyId if has.toggle => i4_var(v, logic::toggle_state(n)),
        UIA_ExpandCollapseExpandCollapseStatePropertyId if has.expand_collapse => {
            i4_var(v, logic::expand_state(n))
        }
        UIA_SelectionItemIsSelectedPropertyId if has.selection_item => {
            bool_var(v, n.selected || n.checked == Some(true))
        }
        UIA_SelectionCanSelectMultiplePropertyId | UIA_SelectionIsSelectionRequiredPropertyId
            if has.selection =>
        {
            bool_var(v, false)
        }
        UIA_ScrollHorizontalScrollPercentPropertyId
        | UIA_ScrollVerticalScrollPercentPropertyId
        | UIA_ScrollHorizontalViewSizePropertyId
        | UIA_ScrollVerticalViewSizePropertyId
        | UIA_ScrollHorizontallyScrollablePropertyId
        | UIA_ScrollVerticallyScrollablePropertyId => {
            let Some(s) = n.scroll else {
                return false;
            };
            let (hp, vp) = logic::scroll_percent(&s);
            let (hv, vv) = logic::view_size(&s);
            match id {
                UIA_ScrollHorizontalScrollPercentPropertyId => r8_var(v, hp),
                UIA_ScrollVerticalScrollPercentPropertyId => r8_var(v, vp),
                UIA_ScrollHorizontalViewSizePropertyId => r8_var(v, hv),
                UIA_ScrollVerticalViewSizePropertyId => r8_var(v, vv),
                UIA_ScrollHorizontallyScrollablePropertyId => {
                    bool_var(v, s.horizontal && s.scrolls())
                }
                _ => bool_var(v, !s.horizontal && s.scrolls()),
            }
        }
        _ => return false,
    }
    true
}

/// Write `on` to a `BOOL*` out-parameter.
unsafe fn put_bool(ret: *mut i32, on: bool) -> Hresult {
    if ret.is_null() {
        return E_POINTER;
    }
    unsafe { *ret = i32::from(on) };
    S_OK
}

/// Run `f` on `p`'s node, or fail when the tree no longer has it.
unsafe fn with_node(p: *mut Provider, f: impl FnOnce(&A11yNode) -> Hresult) -> Hresult {
    match unsafe { node_of(p) } {
        Some(n) => f(n),
        None => E_FAIL,
    }
}

/// Click `p`'s node.
unsafe fn click(p: *mut Provider) -> Hresult {
    unsafe {
        with_node(p, |n| {
            // Brought into view first (see `access::reveal`).
            let events = super::relations::activate_events(p).unwrap_or_else(|| n.click_events());
            deliver(p, events);
            S_OK
        })
    }
}

// ---- Value ------------------------------------------------------------------

#[repr(C)]
pub(super) struct ValueVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    set_value: unsafe extern "system" fn(*mut c_void, *const u16) -> Hresult,
    get_value: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult,
    get_is_read_only: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
}

iface!(
    value_vtable,
    IID_IValueProvider,
    value_owner,
    value_qi,
    value_add_ref,
    value_release
);

/// `SetValue`: replace the field's text (focused by a click first), through
/// the input-method path typed text takes.
unsafe extern "system" fn set_value(this: *mut c_void, value: *const u16) -> Hresult {
    unsafe {
        let p = value_owner(this);
        let text = bstr_to_string(value);
        with_node(p, |n| {
            deliver(p, n.set_text_events(&text));
            S_OK
        })
    }
}

unsafe extern "system" fn get_value(this: *mut c_void, ret: *mut *mut u16) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let p = value_owner(this);
        with_node(p, |n| {
            *ret = bstr(&logic::uia_value(n));
            S_OK
        })
    }
}

unsafe extern "system" fn get_is_read_only(_this: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe { put_bool(ret, false) }
}

pub(super) static VALUE_VTBL: ValueVtbl = ValueVtbl {
    query_interface: value_qi,
    add_ref: value_add_ref,
    release: value_release,
    set_value,
    get_value,
    get_is_read_only,
};

// ---- Toggle -----------------------------------------------------------------

#[repr(C)]
pub(super) struct ToggleVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    toggle: unsafe extern "system" fn(*mut c_void) -> Hresult,
    get_toggle_state: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
}

iface!(
    toggle_vtable,
    IID_IToggleProvider,
    toggle_owner,
    toggle_qi,
    toggle_add_ref,
    toggle_release
);

/// `Toggle`: a click, as on the box.
unsafe extern "system" fn toggle(this: *mut c_void) -> Hresult {
    unsafe { click(toggle_owner(this)) }
}

unsafe extern "system" fn get_toggle_state(this: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let p = toggle_owner(this);
        with_node(p, |n| {
            *ret = logic::toggle_state(n);
            S_OK
        })
    }
}

pub(super) static TOGGLE_VTBL: ToggleVtbl = ToggleVtbl {
    query_interface: toggle_qi,
    add_ref: toggle_add_ref,
    release: toggle_release,
    toggle,
    get_toggle_state,
};

// ---- Selection --------------------------------------------------------------

#[repr(C)]
pub(super) struct SelectionVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_selection: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    get_can_select_multiple: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    get_is_selection_required: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
}

iface!(
    selection_vtable,
    IID_ISelectionProvider,
    selection_owner,
    selection_qi,
    selection_add_ref,
    selection_release
);

/// The selected items under the container `p` (not those of a container
/// inside it), in tree order.
unsafe fn selected_items(p: *mut Provider, out: &mut Vec<*mut Provider>) {
    unsafe {
        let children: &[*mut Provider] = &(*p).children;
        for &c in children {
            if (*c).patterns.selection {
                continue;
            }
            if (*c).patterns.selection_item
                && node_of(c).is_some_and(|n| n.selected || n.checked == Some(true))
            {
                out.push(c);
            }
            selected_items(c, out);
        }
    }
}

/// A `SAFEARRAY` of `IRawElementProviderSimple*` for `items` (`AddRef`ed by
/// the array).
pub(super) unsafe fn provider_array(items: &[*mut Provider]) -> *mut c_void {
    unsafe {
        let sa = SafeArrayCreateVector(VT_UNKNOWN, 0, items.len() as u32);
        if sa.is_null() {
            return sa;
        }
        for (i, &it) in items.iter().enumerate() {
            let idx = i as i32;
            let unk = it as *mut c_void;
            SafeArrayPutElement(sa, &idx, unk as *const c_void);
        }
        sa
    }
}

unsafe extern "system" fn get_selection(this: *mut c_void, ret: *mut *mut c_void) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let mut items = Vec::new();
        selected_items(selection_owner(this), &mut items);
        *ret = provider_array(&items);
    }
    S_OK
}

unsafe extern "system" fn get_false(_this: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe { put_bool(ret, false) }
}

pub(super) static SELECTION_VTBL: SelectionVtbl = SelectionVtbl {
    query_interface: selection_qi,
    add_ref: selection_add_ref,
    release: selection_release,
    get_selection,
    get_can_select_multiple: get_false,
    get_is_selection_required: get_false,
};

// ---- SelectionItem ----------------------------------------------------------

#[repr(C)]
pub(super) struct SelectionItemVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    select: unsafe extern "system" fn(*mut c_void) -> Hresult,
    add_to_selection: unsafe extern "system" fn(*mut c_void) -> Hresult,
    remove_from_selection: unsafe extern "system" fn(*mut c_void) -> Hresult,
    get_is_selected: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    get_selection_container: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
}

iface!(
    selection_item_vtable,
    IID_ISelectionItemProvider,
    item_owner,
    item_qi,
    item_add_ref,
    item_release
);

/// `Select` and `AddToSelection`: a click on the item (a list or a tab bar
/// selects the item clicked); already selected, nothing to do.
unsafe extern "system" fn select_item(this: *mut c_void) -> Hresult {
    unsafe {
        let p = item_owner(this);
        if node_of(p).is_some_and(|n| n.selected || n.checked == Some(true)) {
            return S_OK;
        }
        click(p)
    }
}

/// `RemoveFromSelection`: an item stays selected until another one is.
unsafe extern "system" fn remove_from_selection(this: *mut c_void) -> Hresult {
    unsafe {
        let p = item_owner(this);
        if node_of(p).is_some_and(|n| n.selected || n.checked == Some(true)) {
            UIA_E_INVALIDOPERATION
        } else {
            S_OK
        }
    }
}

unsafe extern "system" fn get_is_selected(this: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe {
        let p = item_owner(this);
        let on = node_of(p).is_some_and(|n| n.selected || n.checked == Some(true));
        put_bool(ret, on)
    }
}

unsafe extern "system" fn get_selection_container(
    this: *mut c_void,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let mut at = (*item_owner(this)).parent;
        while !at.is_null() && !(*at).patterns.selection {
            at = (*at).parent;
        }
        yield_provider(at, simple_of, ret);
    }
    S_OK
}

/// `p`'s `IRawElementProviderSimple` interface (the provider itself).
pub(super) unsafe fn simple_of(p: *mut Provider) -> *mut c_void {
    p as *mut c_void
}

pub(super) static SELECTION_ITEM_VTBL: SelectionItemVtbl = SelectionItemVtbl {
    query_interface: item_qi,
    add_ref: item_add_ref,
    release: item_release,
    select: select_item,
    add_to_selection: select_item,
    remove_from_selection,
    get_is_selected,
    get_selection_container,
};

// ---- ExpandCollapse ---------------------------------------------------------

#[repr(C)]
pub(super) struct ExpandVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    expand: unsafe extern "system" fn(*mut c_void) -> Hresult,
    collapse: unsafe extern "system" fn(*mut c_void) -> Hresult,
    get_expand_collapse_state: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
}

iface!(
    expand_vtable,
    IID_IExpandCollapseProvider,
    expand_owner,
    expand_qi,
    expand_add_ref,
    expand_release
);

/// Expand (`open`) or collapse the node, unless it is so already: a tree
/// item is selected with a click and opened with the arrow key its tree
/// takes; a select, a menu or a disclosure opens and closes with a click.
unsafe fn set_expanded(this: *mut c_void, open: bool) -> Hresult {
    use crate::platform::event::{ButtonState, KeyCode, Modifiers};
    unsafe {
        let p = expand_owner(this);
        with_node(p, |n| {
            let state = logic::expand_state(n);
            if state == 3 {
                return UIA_E_INVALIDOPERATION;
            }
            if (state == 1) == open {
                return S_OK;
            }
            let mut events =
                super::relations::activate_events(p).unwrap_or_else(|| n.click_events());
            if n.role == A11yRole::TreeItem {
                let code = if open {
                    KeyCode::ArrowRight
                } else {
                    KeyCode::ArrowLeft
                };
                for state in [ButtonState::Pressed, ButtonState::Released] {
                    events.push(Event::Key {
                        code,
                        state,
                        modifiers: Modifiers::default(),
                    });
                }
            }
            deliver(p, events);
            S_OK
        })
    }
}

unsafe extern "system" fn expand(this: *mut c_void) -> Hresult {
    unsafe { set_expanded(this, true) }
}

unsafe extern "system" fn collapse(this: *mut c_void) -> Hresult {
    unsafe { set_expanded(this, false) }
}

unsafe extern "system" fn get_expand_collapse_state(this: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let p = expand_owner(this);
        with_node(p, |n| {
            *ret = logic::expand_state(n);
            S_OK
        })
    }
}

pub(super) static EXPAND_VTBL: ExpandVtbl = ExpandVtbl {
    query_interface: expand_qi,
    add_ref: expand_add_ref,
    release: expand_release,
    expand,
    collapse,
    get_expand_collapse_state,
};

// ---- Scroll -----------------------------------------------------------------

#[repr(C)]
pub(super) struct ScrollVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    scroll: unsafe extern "system" fn(*mut c_void, i32, i32) -> Hresult,
    set_scroll_percent: unsafe extern "system" fn(*mut c_void, f64, f64) -> Hresult,
    get_horizontal_scroll_percent: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
    get_vertical_scroll_percent: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
    get_horizontal_view_size: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
    get_vertical_view_size: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
    get_horizontally_scrollable: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    get_vertically_scrollable: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
}

iface!(
    scroll_vtable,
    IID_IScrollProvider,
    scroll_owner,
    scroll_qi,
    scroll_add_ref,
    scroll_release
);

/// Run `f` on `p`'s scroll state and node.
unsafe fn with_scroll(
    p: *mut Provider,
    f: impl FnOnce(&A11yNode, &crate::platform::access::A11yScroll) -> Hresult,
) -> Hresult {
    unsafe {
        with_node(p, |n| match &n.scroll {
            Some(s) => f(n, s),
            None => E_FAIL,
        })
    }
}

/// `Scroll(horizontal, vertical)`: a page or a line along the container's
/// axis, as a wheel moves it.
unsafe extern "system" fn scroll(this: *mut c_void, horizontal: i32, vertical: i32) -> Hresult {
    unsafe {
        let p = scroll_owner(this);
        with_scroll(p, |n, s| {
            if let Some(events) = n.scroll_by_events(logic::scroll_request(s, horizontal, vertical))
            {
                deliver(p, events);
            }
            S_OK
        })
    }
}

/// `SetScrollPercent`: scroll to that share of the range on its axis.
unsafe extern "system" fn set_scroll_percent(
    this: *mut c_void,
    horizontal: f64,
    vertical: f64,
) -> Hresult {
    unsafe {
        let p = scroll_owner(this);
        with_scroll(p, |n, s| {
            if let Some(events) = logic::scroll_target(s, horizontal, vertical)
                .and_then(|f| n.scroll_to_fraction_events(f))
            {
                deliver(p, events);
            }
            S_OK
        })
    }
}

/// One of the scroll pattern's `double` properties.
unsafe fn scroll_double(
    this: *mut c_void,
    ret: *mut f64,
    pick: fn(&crate::platform::access::A11yScroll) -> f64,
) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        with_scroll(scroll_owner(this), |_, s| {
            *ret = pick(s);
            S_OK
        })
    }
}

unsafe extern "system" fn get_horizontal_scroll_percent(this: *mut c_void, r: *mut f64) -> Hresult {
    unsafe { scroll_double(this, r, |s| logic::scroll_percent(s).0) }
}
unsafe extern "system" fn get_vertical_scroll_percent(this: *mut c_void, r: *mut f64) -> Hresult {
    unsafe { scroll_double(this, r, |s| logic::scroll_percent(s).1) }
}
unsafe extern "system" fn get_horizontal_view_size(this: *mut c_void, r: *mut f64) -> Hresult {
    unsafe { scroll_double(this, r, |s| logic::view_size(s).0) }
}
unsafe extern "system" fn get_vertical_view_size(this: *mut c_void, r: *mut f64) -> Hresult {
    unsafe { scroll_double(this, r, |s| logic::view_size(s).1) }
}
unsafe extern "system" fn get_horizontally_scrollable(this: *mut c_void, r: *mut i32) -> Hresult {
    unsafe {
        with_scroll(scroll_owner(this), |_, s| {
            put_bool(r, s.horizontal && s.scrolls())
        })
    }
}
unsafe extern "system" fn get_vertically_scrollable(this: *mut c_void, r: *mut i32) -> Hresult {
    unsafe {
        with_scroll(scroll_owner(this), |_, s| {
            put_bool(r, !s.horizontal && s.scrolls())
        })
    }
}

pub(super) static SCROLL_VTBL: ScrollVtbl = ScrollVtbl {
    query_interface: scroll_qi,
    add_ref: scroll_add_ref,
    release: scroll_release,
    scroll,
    set_scroll_percent,
    get_horizontal_scroll_percent,
    get_vertical_scroll_percent,
    get_horizontal_view_size,
    get_vertical_view_size,
    get_horizontally_scrollable,
    get_vertically_scrollable,
};

// ---- ScrollItem -------------------------------------------------------------

#[repr(C)]
pub(super) struct ScrollItemVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    scroll_into_view: unsafe extern "system" fn(*mut c_void) -> Hresult,
}

iface!(
    scroll_item_vtable,
    IID_IScrollItemProvider,
    scroll_item_owner,
    scroll_item_qi,
    scroll_item_add_ref,
    scroll_item_release
);

/// `ScrollIntoView`: scroll the nearest container around the node until it
/// shows (nothing when it does already).
unsafe extern "system" fn scroll_into_view(this: *mut c_void) -> Hresult {
    unsafe {
        let p = scroll_item_owner(this);
        let Some(root) = (*(*p).root).source.as_deref() else {
            return E_FAIL;
        };
        if let Some(events) = root.scroll_into_view_events(&(*p).path) {
            deliver(p, events);
        }
    }
    S_OK
}

pub(super) static SCROLL_ITEM_VTBL: ScrollItemVtbl = ScrollItemVtbl {
    query_interface: scroll_item_qi,
    add_ref: scroll_item_add_ref,
    release: scroll_item_release,
    scroll_into_view,
};

// ---- Self-test --------------------------------------------------------------

/// The pattern interface `pattern` of the provider behind interface `p`,
/// through `GetPatternProvider` on its `…Simple` interface, as a client
/// gets it. Null when it has none.
pub(super) unsafe fn pattern_through(p: *mut c_void, pattern: i32) -> *mut c_void {
    unsafe {
        let simple = query_through(p, &IID_IRawElementProviderSimple);
        if simple.is_null() {
            return simple;
        }
        let vt = &**(simple as *mut *const ProviderVtbl);
        let mut out: *mut c_void = core::ptr::null_mut();
        (vt.get_pattern_provider)(simple, pattern, &mut out);
        release_through(simple);
        out
    }
}

/// The vtable behind interface pointer `p`, as `T`.
pub(super) unsafe fn vt<'a, T>(p: *mut c_void) -> &'a T {
    unsafe { &**(p as *mut *const T) }
}

/// The first provider (depth first, as `Navigate` walks) whose node has
/// `role` and `name`, as its `…Fragment` interface (`AddRef`ed).
pub(super) unsafe fn find_through(frag: *mut c_void, role_type: i32, name: &str) -> *mut c_void {
    unsafe {
        if int_through(frag, UIA_ControlTypePropertyId) == role_type
            && name_through(frag, UIA_NamePropertyId) == name
        {
            let vt = vt::<FragmentVtbl>(frag);
            (vt.add_ref)(frag);
            return frag;
        }
        let mut child = navigate_through(frag, NavigateDirection_FirstChild);
        while !child.is_null() {
            let found = find_through(child, role_type, name);
            if !found.is_null() {
                release_through(child);
                return found;
            }
            let next = navigate_through(child, NavigateDirection_NextSibling);
            release_through(child);
            child = next;
        }
        core::ptr::null_mut()
    }
}

/// Exercise the control patterns of a provider tree through their COM
/// interfaces (each pattern's interface from `GetPatternProvider`, each
/// method through its vtable), printing what they answer: Value, Toggle,
/// Selection and SelectionItem, ExpandCollapse, Scroll and ScrollItem, Text
/// and its ranges, and the events a tree swap plans. The actions (a tree
/// outside a window has nowhere to send them) are recorded and checked.
pub fn selftest_patterns() -> Result<(), String> {
    use crate::platform::access::A11yScroll;
    use crate::platform::event::{Ime, ScrollDelta};
    fn node(role: A11yRole, name: &str, bounds: (f64, f64, f64, f64)) -> A11yNode {
        A11yNode::new(role, name, bounds)
    }
    let item = |i: usize, selected| A11yNode {
        selected,
        ..node(
            A11yRole::ListItem,
            &format!("Item {i}"),
            (0.0, 100.0 + 30.0 * i as f64 - 40.0, 200.0, 30.0),
        )
    };
    let root = A11yNode {
        children: vec![
            A11yNode {
                id: Some("cap".into()),
                ..node(A11yRole::Text, "Caption", (0.0, 0.0, 60.0, 20.0))
            },
            A11yNode {
                caret: Some(3),
                text_selection: None,
                labelled_by: Some("cap".into()),
                font: Some(crate::platform::access::A11yFont {
                    family: Some("Segoe UI".into()),
                    size: 16.0,
                    weight: 400,
                    italic: false,
                }),
                ..node(A11yRole::TextField, "hello world", (60.0, 0.0, 110.0, 20.0))
            },
            A11yNode {
                checked: Some(false),
                ..node(A11yRole::CheckBox, "Remember", (0.0, 30.0, 100.0, 20.0))
            },
            A11yNode {
                scroll: Some(A11yScroll {
                    offset: 40.0,
                    max: 160.0,
                    viewport: 100.0,
                    horizontal: false,
                    rtl: false,
                }),
                children: (0..5).map(|i| item(i, i == 1)).collect(),
                ..node(A11yRole::List, "", (0.0, 100.0, 200.0, 100.0))
            },
            A11yNode {
                children: vec![A11yNode {
                    expanded: Some(false),
                    ..node(A11yRole::TreeItem, "src", (0.0, 220.0, 200.0, 20.0))
                }],
                ..node(A11yRole::Tree, "", (0.0, 220.0, 200.0, 40.0))
            },
            text::selftest_rich_node(),
        ]
        .into_iter()
        .chain(super::grid::selftest_nodes())
        .collect(),
        ..node(A11yRole::Window, "Patterns", (0.0, 0.0, 400.0, 300.0))
    };
    let tree = UiaTree::build(&root);
    let check = |ok: bool, what: &str| -> Result<(), String> {
        if ok {
            println!("UIA patterns: ok: {what}");
            Ok(())
        } else {
            Err(format!("UIA patterns: {what}"))
        }
    };
    take_recorded();
    unsafe {
        let frag = query_through(tree.root_provider(), &IID_IRawElementProviderFragment);
        let find = |ty: i32, name: &str| {
            let f = find_through(frag, ty, name);
            if f.is_null() {
                Err(format!("no element {ty} {name:?}"))
            } else {
                Ok(f)
            }
        };

        // A field with a caption is named by it; its text is its Value.
        let field = find(UIA_EditControlTypeId, "Caption")?;
        let value = pattern_through(field, UIA_ValuePatternId);
        check(!value.is_null(), "the field has the Value pattern")?;
        let vvt = vt::<ValueVtbl>(value);
        let mut b: *mut u16 = core::ptr::null_mut();
        (vvt.get_value)(value, &mut b);
        let text = bstr_to_string(b);
        SysFreeString(b);
        check(text == "hello world", &format!("Value reads {text:?}"))?;
        let mut ro = 1;
        (vvt.get_is_read_only)(value, &mut ro);
        check(ro == 0, "the field isn't read-only")?;
        let hi: Vec<u16> = "hi".encode_utf16().chain([0]).collect();
        (vvt.set_value)(value, hi.as_ptr());
        let got = take_recorded();
        check(
            got.len() == 5 && got[4] == Event::Ime(Ime::Commit("hi".into())),
            &format!("SetValue focuses the field and types the text: {got:?}"),
        )?;
        release_through(value);
        text::selftest_text(field)?;
        let rich = find(UIA_TextControlTypeId, "Rich bold link bonjour")?;
        // Reported, not required yet (CI's non-blocking step greps for it).
        match text::selftest_runs(rich) {
            Ok(()) => println!("UIA patterns: text runs passed"),
            Err(e) => println!("UIA patterns: text runs FAILED: {e}"),
        }
        release_through(rich);
        // LabeledBy: the caption's element.
        let simple = query_through(field, &IID_IRawElementProviderSimple);
        let mut lv = Variant::empty();
        (vt::<ProviderVtbl>(simple).get_property_value)(simple, 30018, &mut lv);
        release_through(simple);
        let label = if lv.vt == VT_UNKNOWN && lv.val != 0 {
            let l = lv.val as *mut c_void;
            let name = name_through(l, UIA_NamePropertyId);
            release_through(l);
            Some(name)
        } else {
            None
        };
        check(
            label.as_deref() == Some("Caption"),
            &format!("LabeledBy is the caption: {label:?}"),
        )?;
        release_through(field);

        // A checkbox toggles with a click.
        let box_ = find(UIA_CheckBoxControlTypeId, "Remember")?;
        let tog = pattern_through(box_, UIA_TogglePatternId);
        check(!tog.is_null(), "the checkbox has the Toggle pattern")?;
        let tvt = vt::<ToggleVtbl>(tog);
        let mut state = -1;
        (tvt.get_toggle_state)(tog, &mut state);
        check(state == 0, &format!("ToggleState is Off ({state})"))?;
        (tvt.toggle)(tog);
        let got = take_recorded();
        check(got.len() == 3, &format!("Toggle clicks it: {got:?}"))?;
        release_through(tog);
        release_through(box_);

        // The list: its selection, its scroll, an item's selection state.
        let list = find(UIA_ListControlTypeId, "")?;
        let sel = pattern_through(list, UIA_SelectionPatternId);
        check(!sel.is_null(), "the list has the Selection pattern")?;
        let svt = vt::<SelectionVtbl>(sel);
        let mut arr: *mut c_void = core::ptr::null_mut();
        (svt.get_selection)(sel, &mut arr);
        let picked = provider_names(arr);
        check(picked == ["Item 1"], &format!("GetSelection is {picked:?}"))?;
        release_through(sel);
        let sc = pattern_through(list, UIA_ScrollPatternId);
        check(!sc.is_null(), "the list has the Scroll pattern")?;
        let scvt = vt::<ScrollVtbl>(sc);
        let (mut hp, mut vp, mut vv) = (0.0, 0.0, 0.0);
        (scvt.get_horizontal_scroll_percent)(sc, &mut hp);
        (scvt.get_vertical_scroll_percent)(sc, &mut vp);
        (scvt.get_vertical_view_size)(sc, &mut vv);
        let mut vs = 0;
        (scvt.get_vertically_scrollable)(sc, &mut vs);
        check(
            hp == logic::NO_SCROLL && vp == 25.0 && (vv - 100.0 / 2.6).abs() < 1e-9 && vs == 1,
            &format!("scroll percents h={hp} v={vp}, view {vv:.1}%, scrollable {vs}"),
        )?;
        (scvt.set_scroll_percent)(sc, logic::NO_SCROLL, 100.0);
        let got = take_recorded();
        check(
            got.last()
                == Some(&Event::Scroll {
                    delta: ScrollDelta::pixels(0.0, 120.0),
                }),
            &format!("SetScrollPercent(-1, 100) wheels to the end: {got:?}"),
        )?;
        (scvt.scroll)(sc, 2, 0);
        let got = take_recorded();
        check(
            got.last()
                == Some(&Event::Scroll {
                    delta: ScrollDelta::pixels(0.0, -40.0),
                }),
            &format!("Scroll(NoAmount, LargeDecrement) pages back to the start: {got:?}"),
        )?;
        release_through(sc);
        release_through(list);

        let first = find(UIA_ListItemControlTypeId, "Item 0")?;
        let it = pattern_through(first, UIA_SelectionItemPatternId);
        check(!it.is_null(), "an item has the SelectionItem pattern")?;
        let ivt = vt::<SelectionItemVtbl>(it);
        let mut on = 1;
        (ivt.get_is_selected)(it, &mut on);
        let mut container: *mut c_void = core::ptr::null_mut();
        (ivt.get_selection_container)(it, &mut container);
        let cname = int_through(container, UIA_ControlTypePropertyId);
        release_through(container);
        check(
            on == 0 && cname == UIA_ListControlTypeId,
            "Item 0 isn't selected, and its container is the list",
        )?;
        (ivt.select)(it);
        // Item 0 is above the list's viewport (scrolled by 40): it is
        // scrolled into view first (a wheel), then clicked.
        let got = take_recorded();
        check(
            got.len() == 5
                && matches!(got[1], Event::Scroll { .. })
                && matches!(
                    got[4],
                    Event::PointerButton {
                        state: crate::platform::event::ButtonState::Released,
                        ..
                    }
                ),
            &format!("Select scrolls the item into view, then clicks it: {got:?}"),
        )?;
        release_through(it);
        // Above the viewport: ScrollIntoView scrolls back to it.
        let si = pattern_through(first, UIA_ScrollItemPatternId);
        check(
            !si.is_null(),
            "an item in the list has the ScrollItem pattern",
        )?;
        (vt::<ScrollItemVtbl>(si).scroll_into_view)(si);
        let got = take_recorded();
        check(
            got.last()
                == Some(&Event::Scroll {
                    delta: ScrollDelta::pixels(0.0, -40.0),
                }),
            &format!("ScrollIntoView scrolls up to Item 0: {got:?}"),
        )?;
        release_through(si);
        release_through(first);

        // A tree item expands with a click and the Right arrow.
        let ti = find(UIA_TreeItemControlTypeId, "src")?;
        let ex = pattern_through(ti, UIA_ExpandCollapsePatternId);
        check(!ex.is_null(), "a tree item has the ExpandCollapse pattern")?;
        let evt = vt::<ExpandVtbl>(ex);
        let mut st = -1;
        (evt.get_expand_collapse_state)(ex, &mut st);
        check(st == 0, &format!("ExpandCollapseState is Collapsed ({st})"))?;
        (evt.collapse)(ex);
        check(
            take_recorded().is_empty(),
            "Collapse on a collapsed item does nothing",
        )?;
        (evt.expand)(ex);
        let got = take_recorded();
        check(
            got.len() == 5
                && matches!(
                    got[3],
                    Event::Key {
                        code: crate::platform::event::KeyCode::ArrowRight,
                        ..
                    }
                ),
            &format!("Expand clicks it and presses Right: {got:?}"),
        )?;
        release_through(ex);
        release_through(ti);
        super::grid::selftest_grid(frag)?;
        release_through(frag);
    }
    // What a swap would raise: the checkbox checked, the list scrolled.
    let mut after = root.clone();
    after.children[2].checked = Some(true);
    if let Some(s) = after.children[3].scroll.as_mut() {
        s.offset = 160.0;
    }
    let planned = logic::events(&root, &after);
    check(
        planned.len() == 2,
        &format!("a swap plans a ToggleState and a scroll change: {planned:?}"),
    )?;
    if tree.in_use() {
        return Err("UIA patterns: a pattern or range kept a reference".into());
    }
    println!("UIA patterns: all checks passed");
    Ok(())
}

/// The names of the providers in a `SAFEARRAY` of `IRawElementProviderSimple*`,
/// destroying the array.
pub(super) unsafe fn provider_names(sa: *mut c_void) -> Vec<String> {
    unsafe {
        let mut out = Vec::new();
        if sa.is_null() {
            return out;
        }
        let mut upper = -1;
        SafeArrayGetUBound(sa, 1, &mut upper);
        for i in 0..=upper {
            let mut el: *mut c_void = core::ptr::null_mut();
            SafeArrayGetElement(sa, &i, &mut el as *mut *mut c_void as *mut c_void);
            if !el.is_null() {
                out.push(name_through(el, UIA_NamePropertyId));
                release_through(el);
            }
        }
        SafeArrayDestroy(sa);
        out
    }
}
