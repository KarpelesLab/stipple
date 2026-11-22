//! Range values and tables through NSAccessibility (what
//! [`ax_plan`](super::ax_plan) works out, sent to AppKit):
//!
//! - a slider's and a progress bar's `AXValue` (an `NSNumber`), `AXMinValue`,
//!   `AXMaxValue` and `AXValueDescription`; a slider's element is a
//!   `StippleAXRangeElement`, whose `accessibilityPerformIncrement` /
//!   `…Decrement` step it (VoiceOver's VO-Up / VO-Down), and whose value a
//!   client sets (see `ax_value::set_value`): a press at the value's place,
//!   as the other bridges do (`platform::access`);
//! - a table's `AXRowCount`, `AXColumnCount`, `AXRows` and
//!   `AXColumnHeaderUIElements`, a row's `AXIndex`, a cell's
//!   `AXRowIndexRange` and `AXColumnIndexRange`.

use std::ffi::c_void;

use super::{Class, Id, Sel, class, msg_void_id, nsstring, objc_msgSend, sel};
use super::{ax_plan, ax_value};
use crate::platform::access::A11yNode;

/// `StippleAXRangeElement`: a slider's element.
pub(super) fn range_class() -> Class {
    ax_value::subclass(
        c"StippleAXRangeElement",
        &[
            (
                "accessibilityPerformIncrement",
                increment as *const c_void,
                c"c@:",
            ),
            (
                "accessibilityPerformDecrement",
                decrement as *const c_void,
                c"c@:",
            ),
        ],
    )
}

/// Step the slider one increment up or down.
fn step(this: Id, up: bool) -> bool {
    ax_value::act_on_node(this, |root, path| root.at_path(path)?.step_range_events(up))
        .unwrap_or(false)
}

extern "C" fn increment(this: Id, _cmd: Sel) -> bool {
    step(this, true)
}

extern "C" fn decrement(this: Id, _cmd: Sel) -> bool {
    step(this, false)
}

fn number_f64(v: f64) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, f64) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(class("NSNumber"), sel("numberWithDouble:"), v)
    }
}

/// Give the element `el` of `node` its range value and table position.
///
/// # Safety
/// `el` is one of our elements (or null).
pub(super) unsafe fn decorate(el: Id, node: &A11yNode) {
    if el.is_null() {
        return;
    }
    unsafe {
        if let Some((value, min, max)) = ax_plan::range_value(node) {
            ax_value::store_id(el, "setAccessibilityValue:", number_f64(value));
            ax_value::store_id(el, "setAccessibilityMinValue:", number_f64(min));
            ax_value::store_id(el, "setAccessibilityMaxValue:", number_f64(max));
            if let Some(text) = ax_plan::value_description(node) {
                msg_void_id(
                    el,
                    sel("setAccessibilityValueDescription:"),
                    nsstring(&text),
                );
            }
        }
        if let Some((rows, columns)) = ax_plan::table_size(node) {
            ax_value::store_i64(el, "setAccessibilityRowCount:", rows as i64);
            ax_value::store_i64(el, "setAccessibilityColumnCount:", columns as i64);
        }
        if let Some(index) = ax_plan::row_index(node) {
            ax_value::store_i64(el, "setAccessibilityIndex:", index as i64);
        }
        if let Some((row, column)) = ax_plan::cell_index(node) {
            ax_value::store_range(el, "setAccessibilityRowIndexRange:", (row, 1));
            ax_value::store_range(el, "setAccessibilityColumnIndexRange:", (column, 1));
        }
    }
}

/// Once a table's element `el` has its children: its `AXRows` (the rows'
/// elements) and `AXColumnHeaderUIElements` (its headers').
///
/// # Safety
/// `el` is the element of `node`, with its children set.
pub(super) unsafe fn link_table(el: Id, node: &A11yNode) {
    if el.is_null() || ax_plan::table_size(node).is_none() {
        return;
    }
    let array = |paths: Vec<Vec<usize>>| unsafe {
        let arr = super::msg_id(class("NSMutableArray"), sel("array"));
        for p in paths {
            if let Some(child) = super::ax_focus::element_at(el, &p) {
                msg_void_id(arr, sel("addObject:"), child);
            }
        }
        arr
    };
    unsafe {
        msg_void_id(
            el,
            sel("setAccessibilityRows:"),
            array(ax_plan::table_rows(node)),
        );
        let headers = node.column_headers().into_iter().map(|(_, p)| p).collect();
        msg_void_id(
            el,
            sel("setAccessibilityColumnHeaderUIElements:"),
            array(headers),
        );
    }
}
