//! AppKit: a live select, an `NSPopUpButton` (see [`NativeMenu`]).
//!
//! One menu item per option (added to its menu directly, so options with
//! the same text stay apart), the chosen one selected; with nothing chosen
//! none is. AppKit shows its menu over the button on a click, the chosen
//! item under the pointer, and a pick sends the button's action (the shared
//! target, `macos_views`), which reads `indexOfSelectedItem` into a
//! [`NativeViewEvent::Picked`](super::NativeViewEvent::Picked). The app's
//! state then says what it shows: a pick it refuses is set back.

use super::objc::{CgRect, Id, msg, nsstring};
use super::{NativeMenu, NativeView};

/// A new select at `f` for `v` (retained), its items set.
pub(super) unsafe fn create(f: CgRect, v: &NativeView) -> Id {
    unsafe {
        let alloc: Id = msg!(super::objc::class("NSPopUpButton"), "alloc", () -> Id);
        let no = false;
        let b: Id = msg!(alloc, "initWithFrame:pullsDown:", (f: CgRect, no: bool) -> Id);
        if !b.is_null() {
            let menu: Id = msg!(b, "menu", () -> Id);
            msg!(menu, "setAutoenablesItems:", (no: bool) -> ());
            set_items(b, v.menu.as_ref());
            select(b, v.menu.as_ref());
        }
        b
    }
}

/// Bring select `b` from `old` to `new`: its items when they changed, and
/// its choice, also when the user picked one since (`old == new`).
pub(super) unsafe fn update(b: Id, old: &NativeView, new: &NativeView) {
    let options = |v: &NativeView| v.menu.as_ref().map(|m| m.options.clone());
    unsafe {
        if options(old) != options(new) {
            set_items(b, new.menu.as_ref());
        }
        select(b, new.menu.as_ref());
    }
}

unsafe fn set_items(b: Id, menu: Option<&NativeMenu>) {
    unsafe {
        msg!(b, "removeAllItems", () -> ());
        let m: Id = msg!(b, "menu", () -> Id);
        let key = nsstring("");
        let none: super::objc::Sel = std::ptr::null_mut();
        for label in menu.map(|m| &m.options[..]).unwrap_or_default() {
            let title = nsstring(label);
            let item: Id = msg!(m, "addItemWithTitle:action:keyEquivalent:",
                (title: Id, none: super::objc::Sel, key: Id) -> Id);
            let yes = true;
            msg!(item, "setEnabled:", (yes: bool) -> ());
        }
    }
}

unsafe fn select(b: Id, menu: Option<&NativeMenu>) {
    let index = menu.and_then(|m| m.selected).map_or(-1, |i| i as isize);
    unsafe {
        let shown: isize = msg!(b, "indexOfSelectedItem", () -> isize);
        if shown != index {
            msg!(b, "selectItemAtIndex:", (index: isize) -> ());
        }
    }
}

/// The option the user picked in `b`.
pub(super) unsafe fn picked(b: Id) -> Option<usize> {
    let i: isize = unsafe { msg!(b, "indexOfSelectedItem", () -> isize) };
    usize::try_from(i).ok()
}

/// Pick option `index` of select `b` as a pick in its menu does: its menu
/// performs the item's action (`performActionForItemAtIndex:`), which the
/// pop-up button turns into choosing it and sending its own action.
/// Returns whether it did; if it didn't, the option is chosen and the
/// action sent for it (so the app still hears the pick) and `false` says
/// the menu's own path didn't work.
pub(super) unsafe fn pick(b: Id, index: usize) -> bool {
    unsafe {
        let m: Id = msg!(b, "menu", () -> Id);
        let n: isize = msg!(m, "numberOfItems", () -> isize);
        if index as isize >= n {
            return false;
        }
        let i = index as isize;
        msg!(m, "performActionForItemAtIndex:", (i: isize) -> ());
        if msg!(b, "indexOfSelectedItem", () -> isize) == i {
            return true;
        }
        msg!(b, "selectItemAtIndex:", (i: isize) -> ());
        let a: super::objc::Sel = msg!(b, "action", () -> super::objc::Sel);
        let t: Id = msg!(b, "target", () -> Id);
        msg!(b, "sendAction:to:", (a: super::objc::Sel, t: Id) -> bool);
        false
    }
}
