//! The app's side of the Dock and the menu bar's system menus on macOS: the
//! application delegate's `applicationShouldHandleReopen:hasVisibleWindows:`
//! (a click on the Dock icon, the app launched again) and
//! `applicationDockMenu:` (the Dock icon's menu, from
//! [`App::dock_menu`](crate::App::dock_menu)); the Window, Services and
//! Help menus' roles (`NSApp.windowsMenu`, `servicesMenu`, `helpMenu`); and
//! the self-test of all of it (`STIPPLE_COCOA_APP_DELEGATE`). Files opened
//! with the app come through `application:openURLs:` (see `open_url`).

use std::cell::Cell;
use std::ffi::c_void;

use super::{
    CTX, Class, Id, Sel, add_item, class, class_addMethod, fill_menu, menu_item, menu_target,
    msg_id, msg_u64, msg_void, msg_void_id, ns_menu, nsstring, nsstring_to_string, objc_msgSend,
    sel,
};
use crate::platform::event::Event;
use crate::platform::menu::{DOCK_ID_BASE, MenuRole, NativeMenu};

thread_local! {
    /// The Dock menu (retained), or null.
    static DOCK_MENU: Cell<Id> = const { Cell::new(std::ptr::null_mut()) };
}

/// `applicationShouldHandleReopen:hasVisibleWindows:`: report it, and let
/// AppKit go on as it would (bringing a minimized window back).
extern "C" fn should_handle_reopen(_this: Id, _cmd: Sel, _app: Id, visible: bool) -> bool {
    crate::platform::window::debug_window(format_args!("reopen, visible windows {visible}"));
    crate::platform::shell::post(Event::Reopen(visible));
    true
}

/// `applicationDockMenu:`: the app's menu, or none.
extern "C" fn dock_menu(_this: Id, _cmd: Sel, _app: Id) -> Id {
    DOCK_MENU.get()
}

/// Give the application delegate `applicationShouldHandleReopen:…` and
/// `applicationDockMenu:`.
///
/// # Safety
/// `cls` must be a class pair still being built.
pub(super) unsafe fn add_app_delegate_methods(cls: Class) {
    unsafe {
        class_addMethod(
            cls,
            sel("applicationShouldHandleReopen:hasVisibleWindows:"),
            should_handle_reopen as *const c_void,
            c"c@:@c".as_ptr(),
        );
        class_addMethod(
            cls,
            sel("applicationDockMenu:"),
            dock_menu as *const c_void,
            c"@@:@".as_ptr(),
        );
    }
}

/// Keep `menu` as the Dock menu (`None` removes it). Its commands report
/// their ids (from [`DOCK_ID_BASE`]) as menu commands.
///
/// # Safety
/// On the main thread.
pub(super) unsafe fn set_dock_menu(menu: Option<&NativeMenu>) {
    CTX.with(|c| {
        c.borrow_mut()
            .menu_states
            .retain(|&tag, _| tag < i64::from(DOCK_ID_BASE))
    });
    unsafe {
        let old = DOCK_MENU.replace(std::ptr::null_mut());
        if !old.is_null() {
            msg_void(old, sel("release"));
        }
        let Some(menu) = menu.filter(|m| !m.items.is_empty()) else {
            return;
        };
        let m = ns_menu("");
        fill_menu(m, &menu.items, menu_target(), false);
        DOCK_MENU.set(m);
    }
}

/// `NSEventModifierFlagCommand`.
const CMD: u64 = 1 << 20;

/// Make `menu` (just filled with the app's items) play `role`: put the
/// Window menu's standard items around the app's, and tell AppKit which menu
/// it is (after `setMainMenu:`, see [`apply_roles`]).
///
/// # Safety
/// `menu` must be a live `NSMenu`.
pub(super) unsafe fn prepare(menu: Id, role: Option<MenuRole>) {
    if role != Some(MenuRole::Window) {
        return;
    }
    unsafe {
        let insert = |item: Id, at: i64| {
            let f: unsafe extern "C" fn(Id, Sel, Id, i64) =
                std::mem::transmute(objc_msgSend as *const c_void);
            f(menu, sel("insertItem:atIndex:"), item, at);
            msg_void(item, sel("release"));
        };
        let separator = || {
            msg_id(
                msg_id(class("NSMenuItem"), sel("separatorItem")),
                sel("retain"),
            )
        };
        let own = msg_u64(menu, sel("numberOfItems"));
        insert(
            menu_item("Minimize", Some(sel("performMiniaturize:")), "m", CMD),
            0,
        );
        insert(menu_item("Zoom", Some(sel("performZoom:")), "", 0), 1);
        if own > 0 {
            insert(separator(), 2);
        }
        msg_void_id(
            menu,
            sel("addItem:"),
            msg_id(class("NSMenuItem"), sel("separatorItem")),
        );
        add_item(
            menu,
            menu_item("Bring All to Front", Some(sel("arrangeInFront:")), "", 0),
        );
    }
}

/// Tell AppKit which of the main menu's menus play a role.
///
/// # Safety
/// On the main thread, once the main menu holds the menus.
pub(super) unsafe fn apply_roles(roles: &[(Id, MenuRole)]) {
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        for &(menu, role) in roles {
            let setter = match role {
                MenuRole::Window => "setWindowsMenu:",
                MenuRole::Services => "setServicesMenu:",
                MenuRole::Help => "setHelpMenu:",
            };
            msg_void_id(app, sel(setter), menu);
        }
    }
}

/// `[menu itemAtIndex:i]`.
unsafe fn item_at(menu: Id, i: u64) -> Id {
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel, i64) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(menu, sel("itemAtIndex:"), i as i64)
    }
}

/// The titles of `menu`'s items.
unsafe fn titles(menu: Id) -> Vec<String> {
    if menu.is_null() {
        return Vec::new();
    }
    unsafe {
        let count = msg_u64(menu, sel("numberOfItems"));
        (0..count)
            .map(|i| nsstring_to_string(msg_id(item_at(menu, i), sel("title"))))
            .collect()
    }
}

/// The CI self-test (`STIPPLE_COCOA_APP_DELEGATE`): send the application
/// delegate a reopen, an `application:openURLs:` with a folder and a custom
/// URL, and ask it for the Dock menu, through real dispatch, and read back
/// which menus AppKit takes for the Window, Services and Help menus and the
/// application menu's title. The reopen and the URLs reach the app's hooks
/// once the loop runs.
///
/// # Safety
/// On the main thread, once the app's first frame installed its menus.
pub(super) unsafe fn self_test() {
    if std::env::var_os("STIPPLE_COCOA_APP_DELEGATE").is_none() {
        return;
    }
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        let delegate = msg_id(app, sel("delegate"));
        let reopen: unsafe extern "C" fn(Id, Sel, Id, bool) -> bool =
            std::mem::transmute(objc_msgSend as *const c_void);
        let handled = reopen(
            delegate,
            sel("applicationShouldHandleReopen:hasVisibleWindows:"),
            app,
            false,
        );
        let folder = msg_id_id_class("NSURL", "fileURLWithPath:", nsstring("/tmp"));
        let link = msg_id_id_class(
            "NSURL",
            "URLWithString:",
            nsstring("stipple-selftest://open?x=1"),
        );
        let objects = [folder, link];
        let array: unsafe extern "C" fn(Id, Sel, *const Id, u64) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let urls = array(
            class("NSArray"),
            sel("arrayWithObjects:count:"),
            objects.as_ptr(),
            objects.len() as u64,
        );
        let open: unsafe extern "C" fn(Id, Sel, Id, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        open(delegate, sel("application:openURLs:"), app, urls);
        let ask: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const c_void);
        let dock_menu = ask(delegate, sel("applicationDockMenu:"), app);
        let dock = titles(dock_menu);
        // Pick the Dock menu's first item, as the Dock would.
        if !dock.is_empty() {
            let pick: unsafe extern "C" fn(Id, Sel, i64) =
                std::mem::transmute(objc_msgSend as *const c_void);
            pick(dock_menu, sel("performActionForItemAtIndex:"), 0);
        }
        let title = |getter: &str| {
            let menu = msg_id(app, sel(getter));
            (!menu.is_null()).then(|| nsstring_to_string(msg_id(menu, sel("title"))))
        };
        let main = msg_id(app, sel("mainMenu"));
        let app_menu = msg_id(item_at(main, 0), sel("submenu"));
        let app_title = nsstring_to_string(msg_id(app_menu, sel("title")));
        let app_menu = titles(app_menu);
        println!(
            "Cocoa app delegate: reopen={handled} dock={dock:?} windows={:?} services={:?} help={:?}",
            title("windowsMenu"),
            title("servicesMenu"),
            title("helpMenu"),
        );
        println!(
            "Cocoa app delegate: windows menu={:?} app menu {app_title:?}={app_menu:?}",
            titles(msg_id(app, sel("windowsMenu")))
        );
    }
}

/// `[NSClass sel:arg]`.
unsafe fn msg_id_id_class(cls: &str, s: &str, arg: Id) -> Id {
    unsafe { super::msg_id_id(class(cls), sel(s), arg) }
}
