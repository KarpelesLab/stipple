//! The Text pattern: `ITextProvider` on fields, spin buttons and labels, and
//! the `ITextRangeProvider`s it hands out. A range is a heap object of its
//! own (freed on its last `Release`) holding a character span of its node's
//! text and a reference on its provider, so the tree it reads stays alive
//! while a client holds it ([`UiaTree::in_use`]). Units, moves and boxes come
//! from `uia_logic`; the text is the node's (a single line, each character an
//! even share of the node's box, as AT-SPI's bridge reports them).

use super::patterns::{bstr, deliver, iface, iface_qi, simple_of};
use super::*;

const IID_ITextProvider: Guid = Guid(
    0x3589_c92c,
    0x63f3,
    0x4367,
    [0x99, 0xbb, 0xad, 0xa6, 0x53, 0xb7, 0x7c, 0xf2],
);
const IID_ITextRangeProvider: Guid = Guid(
    0x5347_ad7b,
    0xc355,
    0x46f8,
    [0xaf, 0xf5, 0x90, 0x90, 0x33, 0x58, 0x2f, 0x63],
);

#[link(name = "uiautomationcore")]
unsafe extern "system" {
    fn UiaGetReservedNotSupportedValue(value: *mut *mut c_void) -> Hresult;
}

/// `UiaPoint`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct UiaPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
pub(super) struct TextVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_selection: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    get_visible_ranges: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    range_from_child:
        unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> Hresult,
    range_from_point: unsafe extern "system" fn(*mut c_void, UiaPoint, *mut *mut c_void) -> Hresult,
    get_document_range: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    get_supported_text_selection: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
}

iface!(
    text_vtable,
    IID_ITextProvider,
    text_owner,
    text_qi,
    text_add_ref,
    text_release
);

/// The text `p` serves, as characters.
unsafe fn chars(p: *const Provider) -> Vec<char> {
    unsafe { node_of(p).map(logic::text_of).unwrap_or_default() }
}

/// A `SAFEARRAY` of the ranges `spans` of `p`'s text.
unsafe fn range_array(p: *mut Provider, spans: &[(usize, usize)]) -> *mut c_void {
    unsafe {
        let sa = SafeArrayCreateVector(VT_UNKNOWN, 0, spans.len() as u32);
        if sa.is_null() {
            return sa;
        }
        for (i, &(s, e)) in spans.iter().enumerate() {
            let r = new_range(p, s, e);
            let idx = i as i32;
            // The array takes its own reference; ours goes.
            SafeArrayPutElement(sa, &idx, r as *const c_void);
            range_release(r);
        }
        sa
    }
}

unsafe extern "system" fn get_selection(this: *mut c_void, ret: *mut *mut c_void) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let p = text_owner(this);
        let spans: Vec<(usize, usize)> = node_of(p)
            .and_then(logic::selection_of)
            .into_iter()
            .collect();
        *ret = range_array(p, &spans);
    }
    S_OK
}

unsafe extern "system" fn get_visible_ranges(this: *mut c_void, ret: *mut *mut c_void) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let p = text_owner(this);
        let len = chars(p).len();
        *ret = range_array(p, &[(0, len)]);
    }
    S_OK
}

/// `RangeFromChild`: the text has no embedded children.
unsafe extern "system" fn range_from_child(
    _this: *mut c_void,
    _child: *mut c_void,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        if !ret.is_null() {
            *ret = core::ptr::null_mut();
        }
    }
    E_INVALIDARG
}

/// `RangeFromPoint`: a degenerate range at the character boundary nearest
/// the point (screen pixels), along the node's line.
unsafe extern "system" fn range_from_point(
    this: *mut c_void,
    pt: UiaPoint,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let p = text_owner(this);
        let len = chars(p).len();
        let r = screen_rect(p);
        let at = if r.width > 0.0 {
            (((pt.x - r.left) / r.width) * len as f64)
                .round()
                .clamp(0.0, len as f64) as usize
        } else {
            0
        };
        *ret = new_range(p, at, at);
    }
    S_OK
}

unsafe extern "system" fn get_document_range(this: *mut c_void, ret: *mut *mut c_void) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let p = text_owner(this);
        *ret = new_range(p, 0, chars(p).len());
    }
    S_OK
}

/// `SupportedTextSelection`: one selection (Single) in a field, none in a
/// label.
unsafe extern "system" fn get_supported_text_selection(
    this: *mut c_void,
    ret: *mut i32,
) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let p = text_owner(this);
        *ret =
            i32::from(node_of(p).is_some_and(|n| n.caret.is_some() || n.text_selection.is_some()));
    }
    S_OK
}

pub(super) static TEXT_VTBL: TextVtbl = TextVtbl {
    query_interface: text_qi,
    add_ref: text_add_ref,
    release: text_release,
    get_selection,
    get_visible_ranges,
    range_from_child,
    range_from_point,
    get_document_range,
    get_supported_text_selection,
};

// ---- Ranges ----------------------------------------------------------------

/// An `ITextRangeProvider`: `[start, end)` in characters of `provider`'s
/// text.
#[repr(C)]
struct Range {
    vtable: *const RangeVtbl,
    refcount: u32,
    provider: *mut Provider,
    start: usize,
    end: usize,
}

/// A new range (one reference, the caller's), holding a reference on `p`.
unsafe fn new_range(p: *mut Provider, start: usize, end: usize) -> *mut c_void {
    unsafe { add_ref(p as *mut c_void) };
    let (start, end) = (start.min(end), end.max(start));
    Box::into_raw(Box::new(Range {
        vtable: &RANGE_VTBL,
        refcount: 1,
        provider: p,
        start,
        end,
    })) as *mut c_void
}

/// The range behind `this`, when it is one of ours.
unsafe fn range<'a>(this: *mut c_void) -> Option<&'a mut Range> {
    if this.is_null() {
        return None;
    }
    let r = this as *mut Range;
    unsafe { core::ptr::eq((*r).vtable, &RANGE_VTBL).then(|| &mut *r) }
}

/// The range's span, clamped to its text, and the text.
unsafe fn span(r: &Range) -> (Vec<char>, (usize, usize)) {
    let text = unsafe { chars(r.provider) };
    let len = text.len();
    (text, (r.start.min(len), r.end.min(len)))
}

#[repr(C)]
struct RangeVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    clone: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    compare: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut i32) -> Hresult,
    compare_endpoints:
        unsafe extern "system" fn(*mut c_void, i32, *mut c_void, i32, *mut i32) -> Hresult,
    expand_to_enclosing_unit: unsafe extern "system" fn(*mut c_void, i32) -> Hresult,
    find_attribute:
        unsafe extern "system" fn(*mut c_void, i32, Variant, i32, *mut *mut c_void) -> Hresult,
    find_text:
        unsafe extern "system" fn(*mut c_void, *mut u16, i32, i32, *mut *mut c_void) -> Hresult,
    get_attribute_value: unsafe extern "system" fn(*mut c_void, i32, *mut Variant) -> Hresult,
    get_bounding_rectangles: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    get_enclosing_element: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    get_text: unsafe extern "system" fn(*mut c_void, i32, *mut *mut u16) -> Hresult,
    move_: unsafe extern "system" fn(*mut c_void, i32, i32, *mut i32) -> Hresult,
    move_endpoint_by_unit:
        unsafe extern "system" fn(*mut c_void, i32, i32, i32, *mut i32) -> Hresult,
    move_endpoint_by_range:
        unsafe extern "system" fn(*mut c_void, i32, *mut c_void, i32) -> Hresult,
    select: unsafe extern "system" fn(*mut c_void) -> Hresult,
    add_to_selection: unsafe extern "system" fn(*mut c_void) -> Hresult,
    remove_from_selection: unsafe extern "system" fn(*mut c_void) -> Hresult,
    scroll_into_view: unsafe extern "system" fn(*mut c_void, i32) -> Hresult,
    get_children: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
}

unsafe extern "system" fn range_qi(
    this: *mut c_void,
    iid: *const Guid,
    ppv: *mut *mut c_void,
) -> Hresult {
    unsafe { iface_qi(this, iid, ppv, &IID_ITextRangeProvider, range_add_ref) }
}

unsafe extern "system" fn range_add_ref(this: *mut c_void) -> u32 {
    unsafe {
        let r = this as *mut Range;
        (*r).refcount += 1;
        (*r).refcount
    }
}

unsafe extern "system" fn range_release(this: *mut c_void) -> u32 {
    unsafe {
        let r = this as *mut Range;
        (*r).refcount = (*r).refcount.saturating_sub(1);
        let left = (*r).refcount;
        if left == 0 {
            let b = Box::from_raw(r);
            release(b.provider as *mut c_void);
        }
        left
    }
}

unsafe extern "system" fn clone(this: *mut c_void, ret: *mut *mut c_void) -> Hresult {
    unsafe {
        let (Some(r), false) = (range(this), ret.is_null()) else {
            return E_POINTER;
        };
        *ret = new_range(r.provider, r.start, r.end);
    }
    S_OK
}

unsafe extern "system" fn compare(this: *mut c_void, other: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe {
        let (Some(a), Some(b)) = (range(this), range(other)) else {
            return E_INVALIDARG;
        };
        if ret.is_null() {
            return E_POINTER;
        }
        *ret = i32::from(a.provider == b.provider && a.start == b.start && a.end == b.end);
    }
    S_OK
}

/// An endpoint (Start 0, End 1) of `r`.
fn endpoint(r: &Range, which: i32) -> usize {
    if which == 0 { r.start } else { r.end }
}

unsafe extern "system" fn compare_endpoints(
    this: *mut c_void,
    mine: i32,
    other: *mut c_void,
    theirs: i32,
    ret: *mut i32,
) -> Hresult {
    unsafe {
        let (Some(a), Some(b)) = (range(this), range(other)) else {
            return E_INVALIDARG;
        };
        if ret.is_null() {
            return E_POINTER;
        }
        *ret =
            (endpoint(a, mine) as i64 - endpoint(b, theirs) as i64).clamp(-1 << 30, 1 << 30) as i32;
    }
    S_OK
}

unsafe extern "system" fn expand_to_enclosing_unit(this: *mut c_void, unit: i32) -> Hresult {
    unsafe {
        let Some(r) = range(this) else {
            return E_INVALIDARG;
        };
        let (text, (s, _)) = span(r);
        (r.start, r.end) = logic::expand(&text, s, logic::unit(unit));
    }
    S_OK
}

/// The attribute `attr` over the range's text (one run: all of a node's
/// text is drawn alike), when the tree knows it.
unsafe fn attribute(r: &Range, attr: i32) -> Option<logic::Val> {
    unsafe { node_of(r.provider).and_then(|n| logic::text_attribute(n, attr)) }
}

/// Whether the VARIANT `v` holds `want`.
fn variant_is(v: &Variant, want: &logic::Val) -> bool {
    use logic::Val;
    match want {
        Val::Bool(b) => v.vt == VT_BOOL && (v.val as u16 != 0) == *b,
        Val::I4(n) => v.vt == VT_I4 && v.val as u32 as i32 == *n,
        Val::R8(x) => v.vt == VT_R8 && f64::from_bits(v.val) == *x,
        Val::Str(s) => v.vt == VT_BSTR && unsafe { bstr_to_string(v.val as *const u16) } == *s,
    }
}

/// `FindAttribute`: the text is one run, so the range itself when its
/// attribute is `value` (and it isn't empty), else nothing.
unsafe extern "system" fn find_attribute(
    this: *mut c_void,
    attr: i32,
    value: Variant,
    _backward: i32,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        *ret = core::ptr::null_mut();
        let Some(r) = range(this) else {
            return E_INVALIDARG;
        };
        let (_, (s, e)) = span(r);
        if e > s && attribute(r, attr).is_some_and(|a| variant_is(&value, &a)) {
            *ret = new_range(r.provider, s, e);
        }
    }
    S_OK
}

unsafe extern "system" fn find_text(
    this: *mut c_void,
    needle: *mut u16,
    backward: i32,
    ignore_case: i32,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        let Some(r) = range(this) else {
            return E_INVALIDARG;
        };
        if ret.is_null() {
            return E_POINTER;
        }
        let (text, sp) = span(r);
        let needle = bstr_to_string(needle);
        *ret = match logic::find_text(&text, sp, &needle, backward != 0, ignore_case != 0) {
            Some((s, e)) => new_range(r.provider, s, e),
            None => core::ptr::null_mut(),
        };
    }
    S_OK
}

/// `GetAttributeValue`: the node's font name, size (points), weight,
/// italic and read-only state (see `uia_logic::text_attribute`); UIA's
/// reserved "not supported" value for the others.
unsafe extern "system" fn get_attribute_value(
    this: *mut c_void,
    attr: i32,
    ret: *mut Variant,
) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        if let Some(a) = range(this).and_then(|r| attribute(r, attr)) {
            *ret = super::events::variant(&a);
            return S_OK;
        }
        let mut v = Variant::empty();
        let mut unk: *mut c_void = core::ptr::null_mut();
        if UiaGetReservedNotSupportedValue(&mut unk) == S_OK {
            v.vt = VT_UNKNOWN;
            v.val = unk as u64;
        }
        *ret = v;
    }
    S_OK
}

/// `GetBoundingRectangles`: the span's box on its (single) line, in screen
/// pixels, as `[left, top, width, height]` doubles; none for an empty span.
unsafe extern "system" fn get_bounding_rectangles(
    this: *mut c_void,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        let Some(r) = range(this) else {
            return E_INVALIDARG;
        };
        if ret.is_null() {
            return E_POINTER;
        }
        let (text, sp) = span(r);
        let s = screen_rect(r.provider);
        let rect = logic::range_boxes((s.left, s.top, s.width, s.height), text.len(), sp);
        let values: Vec<f64> = rect.map(|b| b.to_vec()).unwrap_or_default();
        let sa = SafeArrayCreateVector(VT_R8, 0, values.len() as u32);
        if !sa.is_null() {
            for (i, x) in values.iter().enumerate() {
                let idx = i as i32;
                SafeArrayPutElement(sa, &idx, x as *const f64 as *const c_void);
            }
        }
        *ret = sa;
    }
    S_OK
}

unsafe extern "system" fn get_enclosing_element(
    this: *mut c_void,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        let Some(r) = range(this) else {
            return E_INVALIDARG;
        };
        if ret.is_null() {
            return E_POINTER;
        }
        yield_provider(r.provider, simple_of, ret);
    }
    S_OK
}

unsafe extern "system" fn get_text(this: *mut c_void, max: i32, ret: *mut *mut u16) -> Hresult {
    unsafe {
        let Some(r) = range(this) else {
            return E_INVALIDARG;
        };
        if ret.is_null() {
            return E_POINTER;
        }
        let (text, (s, e)) = span(r);
        let mut out: String = text[s..e].iter().collect();
        // `maxLength` counts UTF-16 units; -1 for all of it.
        if max >= 0 {
            let mut units = 0;
            let cut = out
                .char_indices()
                .find(|&(_, c)| {
                    units += c.len_utf16();
                    units > max as usize
                })
                .map(|(i, _)| i);
            if let Some(i) = cut {
                out.truncate(i);
            }
        }
        *ret = bstr(&out);
    }
    S_OK
}

unsafe extern "system" fn move_(
    this: *mut c_void,
    unit: i32,
    count: i32,
    ret: *mut i32,
) -> Hresult {
    unsafe {
        let Some(r) = range(this) else {
            return E_INVALIDARG;
        };
        let (text, sp) = span(r);
        let ((s, e), moved) = logic::move_range(&text, sp, logic::unit(unit), count);
        (r.start, r.end) = (s, e);
        if !ret.is_null() {
            *ret = moved;
        }
    }
    S_OK
}

unsafe extern "system" fn move_endpoint_by_unit(
    this: *mut c_void,
    which: i32,
    unit: i32,
    count: i32,
    ret: *mut i32,
) -> Hresult {
    unsafe {
        let Some(r) = range(this) else {
            return E_INVALIDARG;
        };
        let (text, sp) = span(r);
        let ((s, e), moved) = logic::move_endpoint(&text, sp, which, logic::unit(unit), count);
        (r.start, r.end) = (s, e);
        if !ret.is_null() {
            *ret = moved;
        }
    }
    S_OK
}

unsafe extern "system" fn move_endpoint_by_range(
    this: *mut c_void,
    which: i32,
    other: *mut c_void,
    theirs: i32,
) -> Hresult {
    unsafe {
        let Some(b) = range(other).map(|b| endpoint(b, theirs)) else {
            return E_INVALIDARG;
        };
        let Some(r) = range(this) else {
            return E_INVALIDARG;
        };
        (r.start, r.end) = logic::set_endpoint((r.start, r.end), which, b);
    }
    S_OK
}

/// `Select`: have the field select the span (focusing it first).
unsafe extern "system" fn select(this: *mut c_void) -> Hresult {
    unsafe {
        let Some(r) = range(this) else {
            return E_INVALIDARG;
        };
        let Some(n) = node_of(r.provider) else {
            return E_FAIL;
        };
        let (_, (s, e)) = span(r);
        match n.select_events(logic::byte_at(n, s), logic::byte_at(n, e)) {
            Some(events) => {
                deliver(r.provider, events);
                S_OK
            }
            None => UIA_E_INVALIDOPERATION,
        }
    }
}

/// A field has one selection: there is none to add or take away.
unsafe extern "system" fn no_multiple_selection(_this: *mut c_void) -> Hresult {
    UIA_E_INVALIDOPERATION
}

/// `ScrollIntoView`: the line is always in its field's view.
unsafe extern "system" fn range_scroll_into_view(_this: *mut c_void, _top: i32) -> Hresult {
    S_OK
}

unsafe extern "system" fn get_children(_this: *mut c_void, ret: *mut *mut c_void) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        *ret = SafeArrayCreateVector(VT_UNKNOWN, 0, 0);
    }
    S_OK
}

static RANGE_VTBL: RangeVtbl = RangeVtbl {
    query_interface: range_qi,
    add_ref: range_add_ref,
    release: range_release,
    clone,
    compare,
    compare_endpoints,
    expand_to_enclosing_unit,
    find_attribute,
    find_text,
    get_attribute_value,
    get_bounding_rectangles,
    get_enclosing_element,
    get_text,
    move_,
    move_endpoint_by_unit,
    move_endpoint_by_range,
    select,
    add_to_selection: no_multiple_selection,
    remove_from_selection: no_multiple_selection,
    scroll_into_view: range_scroll_into_view,
    get_children,
};

// ---- Self-test --------------------------------------------------------------

/// The text of range `r`, through its vtable.
unsafe fn text_through(r: *mut c_void) -> String {
    unsafe {
        let vt = super::patterns::vt::<RangeVtbl>(r);
        let mut b: *mut u16 = core::ptr::null_mut();
        (vt.get_text)(r, -1, &mut b);
        let s = bstr_to_string(b);
        SysFreeString(b);
        s
    }
}

/// The Text pattern of the field `field` ("hello world", caret after
/// "hel"), through `ITextProvider` and its ranges' vtables.
pub(super) unsafe fn selftest_text(field: *mut c_void) -> Result<(), String> {
    use super::patterns::{pattern_through, take_recorded, vt};
    use crate::platform::event::{Event, Ime};
    let check = |ok: bool, what: &str| -> Result<(), String> {
        if ok {
            println!("UIA patterns: ok: {what}");
            Ok(())
        } else {
            Err(format!("UIA patterns: {what}"))
        }
    };
    unsafe {
        let tp = pattern_through(field, UIA_TextPatternId);
        check(!tp.is_null(), "the field has the Text pattern")?;
        let tvt = vt::<TextVtbl>(tp);
        let mut doc: *mut c_void = core::ptr::null_mut();
        (tvt.get_document_range)(tp, &mut doc);
        let all = text_through(doc);
        check(
            all == "hello world",
            &format!("DocumentRange reads {all:?}"),
        )?;
        let rvt = vt::<RangeVtbl>(doc);
        // A copy, collapsed to the start, expanded to the first word, then
        // moved on to the next.
        let mut word: *mut c_void = core::ptr::null_mut();
        (rvt.clone)(doc, &mut word);
        (rvt.move_endpoint_by_range)(word, 1, doc, 0);
        (rvt.expand_to_enclosing_unit)(word, 2);
        let first = text_through(word);
        let mut moved = 0;
        (rvt.move_)(word, 2, 1, &mut moved);
        let second = text_through(word);
        check(
            first == "hello " && second == "world" && moved == 1,
            &format!("word units: {first:?}, then {second:?} (moved {moved})"),
        )?;
        let mut cmp = 0;
        (rvt.compare_endpoints)(word, 1, doc, 1, &mut cmp);
        check(cmp == 0, "the last word ends where the document does")?;
        (rvt.move_endpoint_by_unit)(word, 0, 0, 2, &mut moved);
        let tail = text_through(word);
        check(
            tail == "rld",
            &format!("MoveEndpointByUnit(Start, Character, 2): {tail:?}"),
        )?;
        // Its box: the last three of eleven characters' even shares.
        let mut sa: *mut c_void = core::ptr::null_mut();
        (rvt.get_bounding_rectangles)(word, &mut sa);
        let mut upper = -1;
        SafeArrayGetUBound(sa, 1, &mut upper);
        let mut left = 0.0f64;
        SafeArrayGetElement(sa, &0, &mut left as *mut f64 as *mut c_void);
        SafeArrayDestroy(sa);
        check(
            upper == 3 && (left - (60.0 + 80.0)).abs() < 1e-9,
            &format!("GetBoundingRectangles: {} doubles, left {left}", upper + 1),
        )?;
        let mut found: *mut c_void = core::ptr::null_mut();
        let needle: Vec<u16> = "WORLD".encode_utf16().chain([0]).collect();
        (rvt.find_text)(doc, needle.as_ptr() as *mut u16, 0, 1, &mut found);
        check(
            !found.is_null() && text_through(found) == "world",
            "FindText ignoring case",
        )?;
        range_release(found);
        // Selecting a range hands the field its byte span.
        (rvt.select)(word);
        let got = take_recorded();
        check(
            got.last()
                == Some(&Event::Ime(Ime::SetSelection {
                    anchor: 8,
                    caret: 11,
                })),
            &format!("Select selects it in the field: {got:?}"),
        )?;
        // GetSelection: a degenerate range at the caret.
        let mut sel: *mut c_void = core::ptr::null_mut();
        (tvt.get_selection)(tp, &mut sel);
        let mut caret: *mut c_void = core::ptr::null_mut();
        SafeArrayGetElement(sel, &0, &mut caret as *mut *mut c_void as *mut c_void);
        SafeArrayDestroy(sel);
        let r = range(caret).map(|r| (r.start, r.end));
        range_release(caret);
        check(
            r == Some((3, 3)),
            &format!("GetSelection is the caret: {r:?}"),
        )?;
        // Its text attributes: the field's font (Segoe UI, 16 px: 12 pt).
        let mut v = Variant::empty();
        (rvt.get_attribute_value)(doc, logic::FONT_NAME, &mut v);
        let name = (v.vt == VT_BSTR).then(|| bstr_to_string(v.val as *const u16));
        super::events::free(&v);
        let mut size = Variant::empty();
        (rvt.get_attribute_value)(doc, logic::FONT_SIZE, &mut size);
        let mut weight = Variant::empty();
        (rvt.get_attribute_value)(doc, logic::FONT_WEIGHT, &mut weight);
        check(
            name.as_deref() == Some("Segoe UI")
                && size.vt == VT_R8
                && f64::from_bits(size.val) == 12.0
                && weight.vt == VT_I4
                && weight.val as u32 == 400,
            &format!(
                "text attributes: FontName {name:?}, FontSize {}, FontWeight {}",
                f64::from_bits(size.val),
                weight.val as u32
            ),
        )?;
        let mut other = Variant::empty();
        (rvt.get_attribute_value)(doc, 40008, &mut other);
        check(
            other.vt == VT_UNKNOWN,
            "an attribute the tree doesn't know is not supported",
        )?;
        let mut found: *mut c_void = core::ptr::null_mut();
        (rvt.find_attribute)(doc, logic::FONT_WEIGHT, weight, 0, &mut found);
        check(
            !found.is_null() && text_through(found) == "hello world",
            "FindAttribute(FontWeight 400) finds the text",
        )?;
        range_release(found);
        let mut bold = Variant::empty();
        bold.vt = VT_I4;
        bold.val = 700;
        let mut none: *mut c_void = core::ptr::null_mut();
        (rvt.find_attribute)(doc, logic::FONT_WEIGHT, bold, 0, &mut none);
        check(
            none.is_null(),
            "FindAttribute(FontWeight 700) finds nothing",
        )?;
        range_release(word);
        range_release(doc);
        release_through(tp);
    }
    Ok(())
}
