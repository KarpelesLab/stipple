//! UIKit: a live select, a `UIButton` whose `UIMenu` is its primary action
//! (see [`NativeMenu`]).
//!
//! The button has the regular button configuration (iOS 26's glass where
//! UIKit has it, as the other buttons) with the pop-up indicator, its title
//! the chosen option. Its menu holds one `UIAction` per option, the chosen
//! one checked (`UIMenuElementStateOn`), and with
//! `changesSelectionAsPrimaryAction` UIKit shows the menu on a tap, from
//! the button, as a pop-up button's. Every action runs one shared handler
//! block, which finds the control and the option by the action's
//! identifier (`stipple.<id>.<index>`) and sends
//! [`NativeViewEvent::Picked`]; the app's state then says what the button
//! shows: a pick it refuses is set back.

use std::ffi::c_void;

use super::objc::{Id, Sel, class, msg, nsstring, sel};
use super::{NativeMenu, NativeView, NativeViewEvent};

unsafe extern "C" {
    static _NSConcreteGlobalBlock: c_void;
}

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

/// The handler block every option's `UIAction` runs (`^(UIAction *)`).
#[repr(C)]
struct PickBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut PickBlock, Id),
    descriptor: *const BlockDescriptor,
}

static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<PickBlock>(),
};

/// The one handler block, made once and never freed (it's global: copying
/// it hands back the same pointer).
fn handler() -> Id {
    thread_local! {
        static BLOCK: std::cell::Cell<Id> = const { std::cell::Cell::new(std::ptr::null_mut()) };
    }
    BLOCK.with(|b| {
        if b.get().is_null() {
            const BLOCK_IS_GLOBAL: i32 = 1 << 28;
            let block = Box::leak(Box::new(PickBlock {
                isa: &raw const _NSConcreteGlobalBlock,
                flags: BLOCK_IS_GLOBAL,
                reserved: 0,
                invoke: picked,
                descriptor: &DESCRIPTOR,
            }));
            b.set((block as *mut PickBlock).cast());
        }
        b.get()
    })
}

/// The identifier of control `id`'s action for option `index`.
fn identifier(id: u64, index: usize) -> String {
    format!("stipple.{id}.{index}")
}

/// The control and option an action's identifier names.
fn parse(identifier: &str) -> Option<(u64, usize)> {
    let rest = identifier.strip_prefix("stipple.")?;
    let (id, index) = rest.split_once('.')?;
    Some((id.parse().ok()?, index.parse().ok()?))
}

/// The handler block's body: the user picked `action`'s option.
unsafe extern "C" fn picked(_block: *mut PickBlock, action: Id) {
    let ident = unsafe { msg!(action, "identifier", () -> Id) };
    let Some((id, index)) = super::objc::rust_string(ident).as_deref().and_then(parse) else {
        return;
    };
    super::os_views::touched(id);
    crate::platform::backend::ios::send_event(crate::platform::Event::NativeView {
        id,
        event: NativeViewEvent::Picked(index),
    });
}

/// A new select for `v` (retained), its title and menu set.
pub(super) unsafe fn create(v: &NativeView) -> Id {
    unsafe {
        let kind = 1_i64; // UIButtonTypeSystem
        let b: Id = msg!(class("UIButton"), "buttonWithType:", (kind: i64) -> Id);
        if b.is_null() {
            return b;
        }
        let b: Id = msg!(b, "retain", () -> Id);
        let config = super::os_views::button_config(false);
        // UIButtonConfigurationIndicatorPopup.
        let s = sel("setIndicator:");
        if !config.is_null() && msg!(config, "respondsToSelector:", (s: Sel) -> bool) {
            let popup = 2_i64;
            msg!(config, "setIndicator:", (popup: i64) -> ());
        }
        msg!(b, "setConfiguration:", (config: Id) -> ());
        let yes = true;
        msg!(b, "setShowsMenuAsPrimaryAction:", (yes: bool) -> ());
        set(b, v);
        b
    }
}

/// Bring select `b` from `old` to `new`: its menu and title, when they
/// changed, or when the user picked from it since (`old == new`: the app's
/// state decides what it shows).
pub(super) unsafe fn update(b: Id, old: &NativeView, new: &NativeView) {
    if old == new || old.menu != new.menu || old.title != new.title {
        unsafe { set(b, new) };
    }
}

/// Set `b`'s menu (the chosen option checked) and title to `v`'s.
unsafe fn set(b: Id, v: &NativeView) {
    let empty = NativeMenu::default();
    let menu = v.menu.as_ref().unwrap_or(&empty);
    unsafe {
        let actions: Id = msg!(class("NSMutableArray"), "array", () -> Id);
        let block = handler();
        let nil: Id = std::ptr::null_mut();
        for (i, label) in menu.options.iter().enumerate() {
            let title = nsstring(label);
            let ident = nsstring(&identifier(v.id, i));
            let action: Id = msg!(class("UIAction"), "actionWithTitle:image:identifier:handler:",
                (title: Id, nil: Id, ident: Id, block: Id) -> Id);
            if menu.selected == Some(i) {
                let on = 1_i64; // UIMenuElementStateOn
                msg!(action, "setState:", (on: i64) -> ());
            }
            msg!(actions, "addObject:", (action: Id) -> ());
        }
        let title = nsstring("");
        let m: Id =
            msg!(class("UIMenu"), "menuWithTitle:children:", (title: Id, actions: Id) -> Id);
        // Off while it's being replaced: a menu with nothing chosen would
        // otherwise have UIKit choose its first item.
        let s = sel("setChangesSelectionAsPrimaryAction:");
        let selects = msg!(b, "respondsToSelector:", (s: Sel) -> bool);
        if selects {
            let no = false;
            msg!(b, "setChangesSelectionAsPrimaryAction:", (no: bool) -> ());
        }
        msg!(b, "setMenu:", (m: Id) -> ());
        if selects && menu.selected.is_some() {
            let yes = true;
            msg!(b, "setChangesSelectionAsPrimaryAction:", (yes: bool) -> ());
        }
        super::os_views::set_title(b, &v.title);
        // Read as its label (when it has one) and the chosen option.
        let value = if v.label.is_empty() {
            std::ptr::null_mut()
        } else {
            nsstring(&v.title)
        };
        msg!(b, "setAccessibilityValue:", (value: Id) -> ());
    }
}

/// Pick option `index` of select `b` as a user's pick in its menu would:
/// UIKit runs the option's action (`sendAction:`), whose handler is the
/// one a pick runs. Returns whether there was such an option.
pub(super) unsafe fn pick(b: Id, index: usize) -> bool {
    unsafe {
        let m: Id = msg!(b, "menu", () -> Id);
        if m.is_null() {
            return false;
        }
        let items: Id = msg!(m, "children", () -> Id);
        let n: usize = msg!(items, "count", () -> usize);
        if index >= n {
            return false;
        }
        let action: Id = msg!(items, "objectAtIndex:", (index: usize) -> Id);
        msg!(b, "sendAction:", (action: Id) -> ());
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn identifiers_name_the_control_and_the_option() {
        let s = super::identifier(u64::MAX - 3, 12);
        assert_eq!(super::parse(&s), Some((u64::MAX - 3, 12)));
        assert_eq!(super::parse("other.1.2"), None);
    }
}
