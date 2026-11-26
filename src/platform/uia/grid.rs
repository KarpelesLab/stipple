//! The RangeValue pattern (`IRangeValueProvider`) on sliders, progress bars
//! and spin buttons, and Grid / GridItem / Table / TableItem
//! (`IGridProvider`, `IGridItemProvider`, `ITableProvider`,
//! `ITableItemProvider`) on a table and its cells, each an interface with a
//! vtable of its own, as the other patterns (see `patterns`).
//!
//! A table's rows are counted as the tree says (`A11yNode::table`): all of
//! them, the header row (row 0 of a `widgets::table`) and the rows a
//! virtualized table hasn't built included; `GetItem` on a row that isn't
//! built fails with `E_INVALIDARG`, as for one out of range.

use super::patterns::{deliver, iface, iface_qi, provider_array, simple_of};
use super::*;

const IID_IRangeValueProvider: Guid = Guid(
    0x36dc_7aef,
    0x33e6,
    0x4691,
    [0xaf, 0xe1, 0x2b, 0xe7, 0x27, 0x4b, 0x3d, 0x33],
);
const IID_IGridProvider: Guid = Guid(
    0xb17d_6187,
    0x0907,
    0x464b,
    [0xa1, 0x68, 0x0e, 0xf1, 0x7a, 0x15, 0x72, 0xb1],
);
const IID_IGridItemProvider: Guid = Guid(
    0xd025_41f1,
    0xfb81,
    0x4d64,
    [0xae, 0x32, 0xf5, 0x20, 0xf8, 0xa6, 0xdb, 0xd1],
);
const IID_ITableProvider: Guid = Guid(
    0x9c86_0395,
    0x97b3,
    0x490a,
    [0xb5, 0x2a, 0x85, 0x8c, 0xc2, 0x2a, 0xf1, 0x66],
);
const IID_ITableItemProvider: Guid = Guid(
    0xb973_4fa6,
    0x771f,
    0x4d78,
    [0x9c, 0x90, 0x25, 0x17, 0x99, 0x93, 0x49, 0xcd],
);

pub(super) const UIA_RangeValuePatternId: i32 = 10003;
pub(super) const UIA_GridPatternId: i32 = 10006;
pub(super) const UIA_GridItemPatternId: i32 = 10007;
pub(super) const UIA_TablePatternId: i32 = 10012;
pub(super) const UIA_TableItemPatternId: i32 = 10013;

const UIA_LabeledByPropertyId: i32 = 30018;
const UIA_IsGridItemPatternAvailablePropertyId: i32 = 30029;
const UIA_IsGridPatternAvailablePropertyId: i32 = 30030;
const UIA_IsRangeValuePatternAvailablePropertyId: i32 = 30033;
const UIA_IsTablePatternAvailablePropertyId: i32 = 30038;
const UIA_IsTableItemPatternAvailablePropertyId: i32 = 30039;
const UIA_RangeValueValuePropertyId: i32 = logic::RANGE_VALUE_VALUE;
const UIA_RangeValueIsReadOnlyPropertyId: i32 = 30048;
const UIA_RangeValueMinimumPropertyId: i32 = 30049;
const UIA_RangeValueMaximumPropertyId: i32 = 30050;
const UIA_RangeValueLargeChangePropertyId: i32 = 30051;
const UIA_RangeValueSmallChangePropertyId: i32 = 30052;
const UIA_GridRowCountPropertyId: i32 = 30062;
const UIA_GridColumnCountPropertyId: i32 = 30063;
const UIA_GridItemRowPropertyId: i32 = 30064;
const UIA_GridItemColumnPropertyId: i32 = 30065;
const UIA_GridItemRowSpanPropertyId: i32 = 30066;
const UIA_GridItemColumnSpanPropertyId: i32 = 30067;
const UIA_TableRowOrColumnMajorPropertyId: i32 = 30083;
/// `RowOrColumnMajor_RowMajor`.
const ROW_MAJOR: i32 = 0;

/// The provider of the node at `path` under `from` (child indices), or null.
pub(super) unsafe fn provider_under(from: *mut Provider, path: &[usize]) -> *mut Provider {
    unsafe {
        let mut at = from;
        for &i in path {
            let kids: &Vec<*mut Provider> = &(*at).children;
            match kids.get(i) {
                Some(&c) => at = c,
                None => return core::ptr::null_mut(),
            }
        }
        at
    }
}

/// The table provider above `p`, or null.
unsafe fn table_of(p: *mut Provider) -> *mut Provider {
    unsafe {
        let mut at = (*p).parent;
        while !at.is_null() && !(*at).patterns.grid {
            at = (*at).parent;
        }
        at
    }
}

/// The providers of the column headers of the table `t` (all of them, or
/// the one over `column`).
unsafe fn headers(t: *mut Provider, column: Option<usize>) -> Vec<*mut Provider> {
    unsafe {
        let Some(n) = node_of(t) else {
            return Vec::new();
        };
        n.column_headers()
            .into_iter()
            .filter(|(c, _)| column.is_none_or(|want| want == *c))
            .map(|(_, path)| provider_under(t, &path))
            .filter(|p| !p.is_null())
            .collect()
    }
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

/// Answer `p`'s property `id` that these patterns (and `LabeledBy`) own
/// into `v`; false when it isn't one of them.
pub(super) unsafe fn property(p: *mut Provider, id: i32, v: &mut Variant) -> bool {
    let has = unsafe { (*p).patterns };
    let Some(n) = (unsafe { node_of(p) }) else {
        return false;
    };
    match id {
        UIA_IsRangeValuePatternAvailablePropertyId => bool_var(v, has.range_value),
        UIA_IsGridPatternAvailablePropertyId | UIA_IsTablePatternAvailablePropertyId => {
            bool_var(v, has.grid)
        }
        UIA_IsGridItemPatternAvailablePropertyId | UIA_IsTableItemPatternAvailablePropertyId => {
            bool_var(v, has.grid_item)
        }
        UIA_RangeValueValuePropertyId
        | UIA_RangeValueIsReadOnlyPropertyId
        | UIA_RangeValueMinimumPropertyId
        | UIA_RangeValueMaximumPropertyId
        | UIA_RangeValueLargeChangePropertyId
        | UIA_RangeValueSmallChangePropertyId => {
            let Some((value, min, max, small, large)) = logic::range_numbers(n) else {
                return false;
            };
            match id {
                UIA_RangeValueValuePropertyId => r8_var(v, value),
                UIA_RangeValueIsReadOnlyPropertyId => bool_var(v, !n.range_settable()),
                UIA_RangeValueMinimumPropertyId => r8_var(v, min),
                UIA_RangeValueMaximumPropertyId => r8_var(v, max),
                UIA_RangeValueLargeChangePropertyId => r8_var(v, large),
                _ => r8_var(v, small),
            }
        }
        UIA_GridRowCountPropertyId | UIA_GridColumnCountPropertyId if has.grid => {
            let t = n.table.unwrap_or(A11yTable {
                rows: 0,
                columns: 0,
                frozen: 0,
            });
            i4_var(
                v,
                if id == UIA_GridRowCountPropertyId {
                    t.rows
                } else {
                    t.columns
                } as i32,
            );
        }
        UIA_TableRowOrColumnMajorPropertyId if has.grid => i4_var(v, ROW_MAJOR),
        UIA_GridItemRowPropertyId
        | UIA_GridItemColumnPropertyId
        | UIA_GridItemRowSpanPropertyId
        | UIA_GridItemColumnSpanPropertyId
            if has.grid_item =>
        {
            let c = n.cell.unwrap_or(A11yCell { row: 0, column: 0 });
            i4_var(
                v,
                match id {
                    UIA_GridItemRowPropertyId => c.row as i32,
                    UIA_GridItemColumnPropertyId => c.column as i32,
                    _ => 1,
                },
            );
        }
        // The caption a control names with `labelled_by` (the shared
        // labelled-by relation).
        UIA_LabeledByPropertyId if n.labelled_by.is_some() => unsafe {
            let root = (*p).root;
            let Some(tree) = (*root).source.as_deref() else {
                return false;
            };
            let Some(path) = tree.relations(&(*p).path).labelled_by.into_iter().next() else {
                return false;
            };
            let label = provider_under(root, &path);
            if label.is_null() || label == p {
                return false;
            }
            add_ref(label as *mut c_void);
            v.vt = VT_UNKNOWN;
            v.val = label as u64;
        },
        _ => return false,
    }
    true
}

/// Write `x` to a `double*` out-parameter.
unsafe fn put_f64(ret: *mut f64, x: f64) -> Hresult {
    if ret.is_null() {
        return E_POINTER;
    }
    unsafe { *ret = x };
    S_OK
}

/// Write `n` to an `int*` out-parameter.
unsafe fn put_i32(ret: *mut i32, n: i32) -> Hresult {
    if ret.is_null() {
        return E_POINTER;
    }
    unsafe { *ret = n };
    S_OK
}

// ---- RangeValue -------------------------------------------------------------

#[repr(C)]
pub(super) struct RangeValueVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub(super) set_value: unsafe extern "system" fn(*mut c_void, f64) -> Hresult,
    pub(super) get_value: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
    pub(super) get_is_read_only: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    pub(super) get_maximum: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
    pub(super) get_minimum: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
    pub(super) get_large_change: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
    pub(super) get_small_change: unsafe extern "system" fn(*mut c_void, *mut f64) -> Hresult,
}

iface!(
    range_value_vtable,
    IID_IRangeValueProvider,
    range_owner,
    range_qi,
    range_add_ref,
    range_release
);

/// `SetValue`: a slider pressed at the value's place, a spin button's
/// number typed (see `A11yNode::set_range_events`); a progress bar is
/// read-only, a value past the range invalid.
unsafe extern "system" fn range_set_value(this: *mut c_void, value: f64) -> Hresult {
    unsafe {
        let p = range_owner(this);
        let Some(n) = node_of(p) else {
            return E_FAIL;
        };
        let Some(r) = n.range else {
            return E_FAIL;
        };
        if !n.range_settable() {
            return UIA_E_INVALIDOPERATION;
        }
        if !value.is_finite() || value < r.min || value > r.max {
            return E_INVALIDARG;
        }
        match n.set_range_events(value) {
            Some(events) => {
                deliver(p, events);
                S_OK
            }
            None => UIA_E_INVALIDOPERATION,
        }
    }
}

/// One of the range's numbers (see `uia_logic::range_numbers`).
unsafe fn range_number(
    this: *mut c_void,
    ret: *mut f64,
    pick: fn((f64, f64, f64, f64, f64)) -> f64,
) -> Hresult {
    unsafe {
        match node_of(range_owner(this)).and_then(logic::range_numbers) {
            Some(nums) => put_f64(ret, pick(nums)),
            None => E_FAIL,
        }
    }
}

unsafe extern "system" fn range_get_value(this: *mut c_void, ret: *mut f64) -> Hresult {
    unsafe { range_number(this, ret, |n| n.0) }
}

unsafe extern "system" fn range_get_minimum(this: *mut c_void, ret: *mut f64) -> Hresult {
    unsafe { range_number(this, ret, |n| n.1) }
}

unsafe extern "system" fn range_get_maximum(this: *mut c_void, ret: *mut f64) -> Hresult {
    unsafe { range_number(this, ret, |n| n.2) }
}

unsafe extern "system" fn range_get_small_change(this: *mut c_void, ret: *mut f64) -> Hresult {
    unsafe { range_number(this, ret, |n| n.3) }
}

unsafe extern "system" fn range_get_large_change(this: *mut c_void, ret: *mut f64) -> Hresult {
    unsafe { range_number(this, ret, |n| n.4) }
}

unsafe extern "system" fn range_get_is_read_only(this: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let ro = node_of(range_owner(this)).is_none_or(|n| !n.range_settable());
        *ret = i32::from(ro);
    }
    S_OK
}

pub(super) static RANGE_VALUE_VTBL: RangeValueVtbl = RangeValueVtbl {
    query_interface: range_qi,
    add_ref: range_add_ref,
    release: range_release,
    set_value: range_set_value,
    get_value: range_get_value,
    get_is_read_only: range_get_is_read_only,
    get_maximum: range_get_maximum,
    get_minimum: range_get_minimum,
    get_large_change: range_get_large_change,
    get_small_change: range_get_small_change,
};

// ---- Grid -------------------------------------------------------------------

#[repr(C)]
pub(super) struct GridVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub(super) get_item:
        unsafe extern "system" fn(*mut c_void, i32, i32, *mut *mut c_void) -> Hresult,
    pub(super) get_row_count: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    pub(super) get_column_count: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
}

iface!(
    grid_vtable,
    IID_IGridProvider,
    grid_owner,
    grid_qi,
    grid_add_ref,
    grid_release
);

/// `GetItem`: the cell at `row` and `column`, when it is built.
unsafe extern "system" fn grid_get_item(
    this: *mut c_void,
    row: i32,
    column: i32,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        *ret = core::ptr::null_mut();
        let p = grid_owner(this);
        let Some(n) = node_of(p) else {
            return E_FAIL;
        };
        let (Ok(row), Ok(column)) = (usize::try_from(row), usize::try_from(column)) else {
            return E_INVALIDARG;
        };
        let Some((path, _)) = n.table_item(row, Some(column)) else {
            return E_INVALIDARG;
        };
        let cell = provider_under(p, &path);
        if cell.is_null() {
            return E_INVALIDARG;
        }
        yield_provider(cell, simple_of, ret);
    }
    S_OK
}

/// The table's `(rows, columns)`.
unsafe fn table_size(p: *mut Provider) -> (i32, i32) {
    unsafe {
        node_of(p)
            .and_then(|n| n.table)
            .map_or((0, 0), |t| (t.rows as i32, t.columns as i32))
    }
}

unsafe extern "system" fn grid_get_row_count(this: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe { put_i32(ret, table_size(grid_owner(this)).0) }
}

unsafe extern "system" fn grid_get_column_count(this: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe { put_i32(ret, table_size(grid_owner(this)).1) }
}

pub(super) static GRID_VTBL: GridVtbl = GridVtbl {
    query_interface: grid_qi,
    add_ref: grid_add_ref,
    release: grid_release,
    get_item: grid_get_item,
    get_row_count: grid_get_row_count,
    get_column_count: grid_get_column_count,
};

// ---- GridItem ---------------------------------------------------------------

#[repr(C)]
pub(super) struct GridItemVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub(super) get_row: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    pub(super) get_column: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    pub(super) get_row_span: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    pub(super) get_column_span: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    pub(super) get_containing_grid:
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
}

iface!(
    grid_item_vtable,
    IID_IGridItemProvider,
    grid_item_owner,
    grid_item_qi,
    grid_item_add_ref,
    grid_item_release
);

/// The cell's `(row, column)`.
unsafe fn cell_at(p: *mut Provider) -> Option<(i32, i32)> {
    unsafe {
        node_of(p)
            .and_then(|n| n.cell)
            .map(|c| (c.row as i32, c.column as i32))
    }
}

unsafe extern "system" fn item_get_row(this: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe {
        match cell_at(grid_item_owner(this)) {
            Some((r, _)) => put_i32(ret, r),
            None => E_FAIL,
        }
    }
}

unsafe extern "system" fn item_get_column(this: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe {
        match cell_at(grid_item_owner(this)) {
            Some((_, c)) => put_i32(ret, c),
            None => E_FAIL,
        }
    }
}

unsafe extern "system" fn item_get_span(_this: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe { put_i32(ret, 1) }
}

unsafe extern "system" fn item_get_containing_grid(
    this: *mut c_void,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        yield_provider(table_of(grid_item_owner(this)), simple_of, ret);
    }
    S_OK
}

pub(super) static GRID_ITEM_VTBL: GridItemVtbl = GridItemVtbl {
    query_interface: grid_item_qi,
    add_ref: grid_item_add_ref,
    release: grid_item_release,
    get_row: item_get_row,
    get_column: item_get_column,
    get_row_span: item_get_span,
    get_column_span: item_get_span,
    get_containing_grid: item_get_containing_grid,
};

// ---- Table ------------------------------------------------------------------

#[repr(C)]
pub(super) struct TableVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub(super) get_row_headers: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    pub(super) get_column_headers:
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    pub(super) get_row_or_column_major: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
}

iface!(
    table_vtable,
    IID_ITableProvider,
    table_owner,
    table_qi,
    table_add_ref,
    table_release
);

/// No row headers: an empty array.
unsafe extern "system" fn no_headers(_this: *mut c_void, ret: *mut *mut c_void) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        *ret = provider_array(&[]);
    }
    S_OK
}

unsafe extern "system" fn table_get_column_headers(
    this: *mut c_void,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        *ret = provider_array(&headers(table_owner(this), None));
    }
    S_OK
}

unsafe extern "system" fn table_get_major(_this: *mut c_void, ret: *mut i32) -> Hresult {
    unsafe { put_i32(ret, ROW_MAJOR) }
}

pub(super) static TABLE_VTBL: TableVtbl = TableVtbl {
    query_interface: table_qi,
    add_ref: table_add_ref,
    release: table_release,
    get_row_headers: no_headers,
    get_column_headers: table_get_column_headers,
    get_row_or_column_major: table_get_major,
};

// ---- TableItem --------------------------------------------------------------

#[repr(C)]
pub(super) struct TableItemVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    pub(super) get_row_header_items:
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
    pub(super) get_column_header_items:
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult,
}

iface!(
    table_item_vtable,
    IID_ITableItemProvider,
    table_item_owner,
    table_item_qi,
    table_item_add_ref,
    table_item_release
);

/// The header over a cell's column (none for a header itself).
unsafe extern "system" fn item_get_column_headers(
    this: *mut c_void,
    ret: *mut *mut c_void,
) -> Hresult {
    unsafe {
        if ret.is_null() {
            return E_POINTER;
        }
        let p = table_item_owner(this);
        let items = match node_of(p) {
            Some(n) if n.role == A11yRole::Cell => {
                let column = n.cell.map(|c| c.column);
                let t = table_of(p);
                if t.is_null() {
                    Vec::new()
                } else {
                    headers(t, column)
                }
            }
            _ => Vec::new(),
        };
        *ret = provider_array(&items);
    }
    S_OK
}

pub(super) static TABLE_ITEM_VTBL: TableItemVtbl = TableItemVtbl {
    query_interface: table_item_qi,
    add_ref: table_item_add_ref,
    release: table_item_release,
    get_row_header_items: no_headers,
    get_column_header_items: item_get_column_headers,
};

// ---- Self-test --------------------------------------------------------------

/// The nodes `selftest_grid` reads: a slider "Volume" at half, a progress bar
/// "Loading" at 30%, and a table of 11 rows (its header row, then rows 5
/// and 6 built) and two columns.
pub(super) fn selftest_nodes() -> Vec<A11yNode> {
    use crate::platform::access::A11yRange;
    let range = |value, step| A11yRange {
        value,
        min: 0.0,
        max: 1.0,
        step,
        rtl: false,
    };
    let at = |row, column| Some(A11yCell { row, column });
    let cell = |role, name: &str, row, column| A11yNode {
        cell: at(row, column),
        ..A11yNode::new(role, name, (0.0, 0.0, 50.0, 20.0))
    };
    let row = |index: usize| A11yNode {
        cell: at(index, 0),
        children: vec![
            cell(A11yRole::Cell, &format!("a{index}"), index, 0),
            cell(A11yRole::Cell, &format!("b{index}"), index, 1),
        ],
        ..A11yNode::new(A11yRole::Row, "", (0.0, 0.0, 100.0, 20.0))
    };
    vec![
        A11yNode {
            range: Some(range(0.5, 0.05)),
            ..A11yNode::new(A11yRole::Slider, "Volume", (0.0, 270.0, 200.0, 20.0))
        },
        A11yNode {
            range: Some(range(0.3, 0.0)),
            ..A11yNode::new(A11yRole::ProgressBar, "Loading", (0.0, 290.0, 200.0, 8.0))
        },
        A11yNode {
            table: Some(A11yTable {
                rows: 11,
                columns: 2,
                frozen: 0,
            }),
            children: vec![
                A11yNode {
                    cell: at(0, 0),
                    children: vec![
                        cell(A11yRole::ColumnHeader, "Name", 0, 0),
                        cell(A11yRole::ColumnHeader, "Value", 0, 1),
                    ],
                    ..A11yNode::new(A11yRole::Row, "", (0.0, 0.0, 100.0, 20.0))
                },
                A11yNode {
                    children: vec![row(5), row(6)],
                    ..A11yNode::new(A11yRole::Group, "", (0.0, 0.0, 100.0, 40.0))
                },
            ],
            ..A11yNode::new(A11yRole::Table, "Data", (0.0, 0.0, 100.0, 60.0))
        },
    ]
}

/// Drive the RangeValue, Grid, GridItem, Table and TableItem patterns of
/// [`selftest_nodes`] (under the fragment `frag`) through their vtables.
pub(super) unsafe fn selftest_grid(frag: *mut c_void) -> Result<(), String> {
    use super::patterns::{find_through, pattern_through, take_recorded, vt};
    use crate::platform::event::Event;
    let check = |ok: bool, what: &str| -> Result<(), String> {
        if ok {
            println!("UIA patterns: ok: {what}");
            Ok(())
        } else {
            Err(format!("UIA patterns: {what}"))
        }
    };
    let find = |ty: i32, name: &str| {
        let f = unsafe { find_through(frag, ty, name) };
        if f.is_null() {
            Err(format!("no element {ty} {name:?}"))
        } else {
            Ok(f)
        }
    };
    unsafe {
        // A slider's range, set by a press at the value's place.
        let slider = find(UIA_SliderControlTypeId, "Volume")?;
        let rv = pattern_through(slider, UIA_RangeValuePatternId);
        check(!rv.is_null(), "the slider has the RangeValue pattern")?;
        let rvt = vt::<RangeValueVtbl>(rv);
        let (mut value, mut min, mut max, mut small, mut large) = (0.0, -1.0, 0.0, 0.0, 0.0);
        let mut ro = 1;
        (rvt.get_value)(rv, &mut value);
        (rvt.get_minimum)(rv, &mut min);
        (rvt.get_maximum)(rv, &mut max);
        (rvt.get_small_change)(rv, &mut small);
        (rvt.get_large_change)(rv, &mut large);
        (rvt.get_is_read_only)(rv, &mut ro);
        check(
            value == 0.5 && min == 0.0 && max == 1.0 && small == 0.05 && large == 0.1 && ro == 0,
            &format!("RangeValue {value} in {min}..{max}, steps {small}/{large}, read-only {ro}"),
        )?;
        let hr = (rvt.set_value)(rv, 0.75);
        let got = take_recorded();
        check(
            hr == S_OK
                && got.len() == 4
                && matches!(got[3], Event::PointerButton { position, .. }
                    if position.x == 150.0 && position.y == 280.0),
            &format!("SetValue(0.75) presses the slider at 3/4 of it: {got:?}"),
        )?;
        let hr = (rvt.set_value)(rv, 2.0);
        check(
            hr == E_INVALIDARG && take_recorded().is_empty(),
            "SetValue past the maximum is refused",
        )?;
        release_through(rv);
        release_through(slider);

        // A progress bar's is read-only.
        let bar = find(UIA_ProgressBarControlTypeId, "Loading")?;
        let rv = pattern_through(bar, UIA_RangeValuePatternId);
        check(!rv.is_null(), "the progress bar has the RangeValue pattern")?;
        let rvt = vt::<RangeValueVtbl>(rv);
        let mut value = 0.0;
        (rvt.get_value)(rv, &mut value);
        (rvt.get_is_read_only)(rv, &mut ro);
        let hr = (rvt.set_value)(rv, 0.5);
        check(
            value == 0.3 && ro == 1 && hr == UIA_E_INVALIDOPERATION,
            &format!("the progress bar reads {value}, read-only {ro}, refuses SetValue"),
        )?;
        release_through(rv);
        release_through(bar);

        // The table: its size, a cell by row and column, its headers.
        let table = find(UIA_DataGridControlTypeId, "Data")?;
        let grid = pattern_through(table, UIA_GridPatternId);
        check(!grid.is_null(), "the table has the Grid pattern")?;
        let gvt = vt::<GridVtbl>(grid);
        let (mut rows, mut columns) = (0, 0);
        (gvt.get_row_count)(grid, &mut rows);
        (gvt.get_column_count)(grid, &mut columns);
        check(
            rows == 11 && columns == 2,
            &format!("Grid has {rows} rows and {columns} columns"),
        )?;
        let mut item: *mut c_void = core::ptr::null_mut();
        let hr = (gvt.get_item)(grid, 5, 1, &mut item);
        check(
            hr == S_OK && !item.is_null() && name_through(item, UIA_NamePropertyId) == "b5",
            "GetItem(5, 1) is b5",
        )?;
        let mut none: *mut c_void = core::ptr::null_mut();
        let hr = (gvt.get_item)(grid, 2, 0, &mut none);
        check(
            hr == E_INVALIDARG && none.is_null(),
            "GetItem on a row not built fails",
        )?;
        release_through(grid);
        let tp = pattern_through(table, UIA_TablePatternId);
        check(!tp.is_null(), "the table has the Table pattern")?;
        let mut sa: *mut c_void = core::ptr::null_mut();
        (vt::<TableVtbl>(tp).get_column_headers)(tp, &mut sa);
        let names = super::patterns::provider_names(sa);
        check(
            names == ["Name", "Value"],
            &format!("its column headers are {names:?}"),
        )?;
        release_through(tp);

        let gi = pattern_through(item, UIA_GridItemPatternId);
        check(!gi.is_null(), "a cell has the GridItem pattern")?;
        let ivt = vt::<GridItemVtbl>(gi);
        let (mut r, mut c, mut span) = (-1, -1, 0);
        (ivt.get_row)(gi, &mut r);
        (ivt.get_column)(gi, &mut c);
        (ivt.get_row_span)(gi, &mut span);
        let mut container: *mut c_void = core::ptr::null_mut();
        (ivt.get_containing_grid)(gi, &mut container);
        let kind = int_through(container, UIA_ControlTypePropertyId);
        release_through(container);
        check(
            r == 5 && c == 1 && span == 1 && kind == UIA_DataGridControlTypeId,
            &format!("GridItem at ({r}, {c}), span {span}, in the table"),
        )?;
        release_through(gi);
        let ti = pattern_through(item, UIA_TableItemPatternId);
        check(!ti.is_null(), "a cell has the TableItem pattern")?;
        let mut sa: *mut c_void = core::ptr::null_mut();
        (vt::<TableItemVtbl>(ti).get_column_header_items)(ti, &mut sa);
        let names = super::patterns::provider_names(sa);
        check(
            names == ["Value"],
            &format!("its column header is {names:?}"),
        )?;
        release_through(ti);
        release_through(item);
        release_through(table);
    }
    Ok(())
}
