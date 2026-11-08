//! A self-test of the native menus for CI, where System Events (and so
//! `osascript` clicking menus) needs an accessibility permission the runner
//! may not grant.
//!
//! With `STIPPLE_COCOA_MENU_SELFTEST` set, the backend prints the application's
//! menu bar as AppKit holds it, then runs the steps the variable lists,
//! separated by `;`, through real Objective-C dispatch and the real event loop:
//!
//! - `perform=Edit/Add Item` picks a menu bar item, as a click on it would
//!   (`-[NSMenu performActionForItemAtIndex:]`);
//! - `key=cmd+d` posts a key-down and key-up to the application's queue, as
//!   typing would (`cmd`, `shift`, `alt`, `ctrl`; a letter, or `tab`);
//! - `right=320,300` posts a right mouse press at that point of the content
//!   view; the context menu it opens is reported, its first item picked and
//!   the menu closed.
//!
//! Each step runs once the events before it were handled. Then it prints
//! `Cocoa menu selftest: done` and ends the event loop.

use std::cell::RefCell;
use std::ffi::c_void;

use super::{
    CTX, CgPoint, Id, Sel, class, msg_i64, msg_id, msg_id_u64, msg_rect, msg_u64, msg_void,
    nsstring, nsstring_to_string, objc_msgSend, sel,
};

/// The variable that turns the self-test on.
const VAR: &str = "STIPPLE_COCOA_MENU_SELFTEST";
/// `NSEventTypeApplicationDefined`, and the subtype marking a step (the wake
/// events use 0).
const APPLICATION_DEFINED: u64 = 15;
const STEP_SUBTYPE: i16 = 0x53;

#[derive(Clone, Debug)]
enum Step {
    Perform {
        menu: String,
        item: String,
    },
    Key {
        flags: u64,
        code: u16,
        chars: String,
    },
    Right {
        x: f64,
        y: f64,
    },
}

struct Check {
    window: Id,
    view: Id,
    steps: Vec<Step>,
    /// The step to post once the context menu being shown closes.
    after_popup: Option<usize>,
}

thread_local! {
    static CHECK: RefCell<Option<Check>> = const { RefCell::new(None) };
}

fn active() -> bool {
    CHECK.with(|c| c.borrow().is_some())
}

/// Print a line of the self-test's report, when it runs.
pub(super) fn note(args: std::fmt::Arguments<'_>) {
    if active() {
        println!("Cocoa menu: {args}");
    }
}

/// The macOS virtual key code of a letter or named key.
fn key_code(name: &str) -> Option<(u16, String)> {
    const LETTERS: &[(char, u16)] = &[
        ('a', 0),
        ('s', 1),
        ('d', 2),
        ('f', 3),
        ('h', 4),
        ('g', 5),
        ('z', 6),
        ('x', 7),
        ('c', 8),
        ('v', 9),
        ('b', 11),
        ('q', 12),
        ('w', 13),
        ('e', 14),
        ('r', 15),
        ('y', 16),
        ('t', 17),
        ('o', 31),
        ('u', 32),
        ('i', 34),
        ('p', 35),
        ('l', 37),
        ('j', 38),
        ('k', 40),
        ('n', 45),
        ('m', 46),
    ];
    match name {
        "tab" => Some((48, "\t".into())),
        "space" => Some((49, " ".into())),
        _ => {
            let mut chars = name.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let code = LETTERS.iter().find(|(l, _)| *l == c)?.1;
            Some((code, c.to_string()))
        }
    }
}

fn parse(spec: &str) -> Vec<Step> {
    spec.split(';')
        .filter_map(|step| {
            let (kind, arg) = step.trim().split_once('=')?;
            match kind {
                "perform" => {
                    let (menu, item) = arg.split_once('/')?;
                    Some(Step::Perform {
                        menu: menu.into(),
                        item: item.into(),
                    })
                }
                "key" => {
                    let mut flags = 0;
                    let mut key = None;
                    for part in arg.split('+') {
                        match part {
                            "cmd" => flags |= 1 << 20,
                            "alt" => flags |= 1 << 19,
                            "ctrl" => flags |= 1 << 18,
                            "shift" => flags |= 1 << 17,
                            other => key = key_code(other),
                        }
                    }
                    let (code, chars) = key?;
                    Some(Step::Key { flags, code, chars })
                }
                "right" => {
                    let (x, y) = arg.split_once(',')?;
                    Some(Step::Right {
                        x: x.trim().parse().ok()?,
                        y: y.trim().parse().ok()?,
                    })
                }
                _ => None,
            }
        })
        .collect()
}

/// Start the self-test if it's asked for: report the menu bar and queue the
/// first step. Called once the window is up and the app has installed its
/// menus.
///
/// # Safety
/// `window` and `view` must be the live window and its content view.
pub(super) unsafe fn start(window: Id, view: Id) {
    let Ok(spec) = std::env::var(VAR) else {
        return;
    };
    let steps = parse(&spec);
    println!("Cocoa menu selftest: {} steps {steps:?}", steps.len());
    CHECK.with(|c| {
        *c.borrow_mut() = Some(Check {
            window,
            view,
            steps,
            after_popup: None,
        })
    });
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        let main = msg_id(app, sel("mainMenu"));
        if main.is_null() {
            println!("Cocoa menu: no main menu");
        } else {
            for i in 0..msg_i64(main, sel("numberOfItems")) {
                let item = msg_id_u64(main, sel("itemAtIndex:"), i as u64);
                let sub = msg_id(item, sel("submenu"));
                let title = nsstring_to_string(msg_id(item, sel("title")));
                println!("Cocoa menu: {title}: {}", describe(sub));
            }
        }
        post_step(0);
    }
}

/// A menu's items, as `Title [⌘key] (off)` with `-` for separators and `>`
/// after a submenu's title. Validates the items first, as AppKit does before
/// showing a menu.
unsafe fn describe(menu: Id) -> String {
    if menu.is_null() {
        return String::new();
    }
    let mut out = Vec::new();
    unsafe {
        msg_void(menu, sel("update"));
        for i in 0..msg_i64(menu, sel("numberOfItems")) {
            let item = msg_id_u64(menu, sel("itemAtIndex:"), i as u64);
            if super::msg_bool(item, sel("isSeparatorItem")) {
                out.push("-".to_string());
                continue;
            }
            let mut s = nsstring_to_string(msg_id(item, sel("title")));
            let key = nsstring_to_string(msg_id(item, sel("keyEquivalent")));
            if !key.is_empty() {
                s.push_str(&format!(" [{key}]"));
            }
            if !msg_id(item, sel("submenu")).is_null() {
                s.push_str(" >");
            }
            if !super::msg_bool(item, sel("isEnabled")) {
                s.push_str(" (off)");
            }
            out.push(s);
        }
    }
    out.join(", ")
}

/// Queue step `index` behind the events already posted.
unsafe fn post_step(index: usize) {
    unsafe {
        let make: unsafe extern "C" fn(
            Id,
            Sel,
            u64,
            CgPoint,
            u64,
            f64,
            i64,
            Id,
            i16,
            i64,
            i64,
        ) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
        let event = make(
            class("NSEvent"),
            sel(
                "otherEventWithType:location:modifierFlags:timestamp:windowNumber:context:subtype:data1:data2:",
            ),
            APPLICATION_DEFINED,
            CgPoint { x: 0.0, y: 0.0 },
            0,
            0.0,
            0,
            std::ptr::null_mut(),
            STEP_SUBTYPE,
            index as i64,
            0,
        );
        post(event);
    }
}

unsafe fn post(event: Id) {
    if event.is_null() {
        println!("Cocoa menu: could not make an event");
        return;
    }
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        let post: unsafe extern "C" fn(Id, Sel, Id, bool) =
            std::mem::transmute(objc_msgSend as *const c_void);
        post(app, sel("postEvent:atStart:"), event, false);
    }
}

/// If `ev` is a self-test step, run it and return `true`.
///
/// # Safety
/// `ev` must be a live `NSEvent`.
pub(super) unsafe fn step_event(ev: Id) -> bool {
    if !active() || unsafe { msg_u64(ev, sel("type")) } != APPLICATION_DEFINED {
        return false;
    }
    let subtype = unsafe {
        let f: unsafe extern "C" fn(Id, Sel) -> i16 =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(ev, sel("subtype"))
    };
    if subtype != STEP_SUBTYPE {
        return false;
    }
    let index = unsafe { msg_i64(ev, sel("data1")) } as usize;
    unsafe { run_step(index) };
    true
}

unsafe fn run_step(index: usize) {
    let (window, view, step) = CHECK.with(|c| {
        let c = c.borrow();
        let c = c.as_ref().expect("checked by step_event");
        (c.window, c.view, c.steps.get(index).cloned())
    });
    let Some(step) = step else {
        println!("Cocoa menu selftest: done");
        CTX.with(|c| c.borrow_mut().should_close = true);
        return;
    };
    println!("Cocoa menu: step {index}: {step:?}");
    let number = unsafe { msg_i64(window, sel("windowNumber")) };
    match step {
        Step::Perform { menu, item } => {
            unsafe { perform(&menu, &item) };
            unsafe { post_step(index + 1) };
        }
        Step::Key { flags, code, chars } => unsafe {
            for ty in [super::NS_KEY_DOWN, super::NS_KEY_UP] {
                post(key_event(ty, flags, number, code, &chars));
            }
            post_step(index + 1);
        },
        Step::Right { x, y } => unsafe {
            // The window's coordinates start at the bottom left.
            let h = msg_rect(view, sel("bounds")).size.height;
            let make: unsafe extern "C" fn(
                Id,
                Sel,
                u64,
                CgPoint,
                u64,
                f64,
                i64,
                Id,
                i64,
                i64,
                f32,
            ) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
            let event = make(
                class("NSEvent"),
                sel(
                    "mouseEventWithType:location:modifierFlags:timestamp:windowNumber:context:eventNumber:clickCount:pressure:",
                ),
                super::NS_RIGHT_MOUSE_DOWN,
                CgPoint { x, y: h - y },
                0,
                0.0,
                number,
                std::ptr::null_mut(),
                0,
                1,
                1.0,
            );
            // The next step waits for the context menu to close.
            CHECK.with(|c| {
                if let Some(c) = c.borrow_mut().as_mut() {
                    c.after_popup = Some(index + 1);
                }
            });
            post(event);
        },
    }
}

unsafe fn key_event(ty: u64, flags: u64, window: i64, code: u16, chars: &str) -> Id {
    unsafe {
        let make: unsafe extern "C" fn(
            Id,
            Sel,
            u64,
            CgPoint,
            u64,
            f64,
            i64,
            Id,
            Id,
            Id,
            bool,
            u16,
        ) -> Id = std::mem::transmute(objc_msgSend as *const c_void);
        make(
            class("NSEvent"),
            sel(
                "keyEventWithType:location:modifierFlags:timestamp:windowNumber:context:characters:charactersIgnoringModifiers:isARepeat:keyCode:",
            ),
            ty,
            CgPoint { x: 0.0, y: 0.0 },
            flags,
            0.0,
            window,
            std::ptr::null_mut(),
            nsstring(chars),
            nsstring(chars),
            false,
            code,
        )
    }
}

/// Pick `item` of the menu bar's `menu` the way a click on it does.
unsafe fn perform(menu: &str, item: &str) {
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        let main = msg_id(app, sel("mainMenu"));
        let Some(sub) = find(main, menu).map(|i| msg_id(i, sel("submenu"))) else {
            println!("Cocoa menu: no menu {menu:?}");
            return;
        };
        let index: i64 = {
            let f: unsafe extern "C" fn(Id, Sel, Id) -> i64 =
                std::mem::transmute(objc_msgSend as *const c_void);
            f(sub, sel("indexOfItemWithTitle:"), nsstring(item))
        };
        if index < 0 {
            println!("Cocoa menu: no item {item:?} in {menu:?}");
            return;
        }
        msg_void(sub, sel("update"));
        let f: unsafe extern "C" fn(Id, Sel, i64) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(sub, sel("performActionForItemAtIndex:"), index);
        println!("Cocoa menu: performed {menu} > {item}");
    }
}

/// The item titled `title` in `menu`.
unsafe fn find(menu: Id, title: &str) -> Option<Id> {
    if menu.is_null() {
        return None;
    }
    let item = unsafe { super::msg_id_id(menu, sel("itemWithTitle:"), nsstring(title)) };
    (!item.is_null()).then_some(item)
}

/// Have the self-test see the context menu `menu` open (its delegate is the
/// menu target, whose `menuWillOpen:` is [`menu_will_open`]).
///
/// # Safety
/// `menu` must be a live `NSMenu` and `target` the menu target.
pub(super) unsafe fn watch(menu: Id, target: Id) {
    if active() {
        unsafe { super::msg_void_id(menu, sel("setDelegate:"), target) };
    }
}

/// `menuWillOpen:` — the self-test's context menu opened: report it, pick
/// its first item and close it.
pub(super) extern "C" fn menu_will_open(_this: Id, _cmd: Sel, menu: Id) {
    if !active() {
        return;
    }
    unsafe {
        println!("Cocoa context menu: open: {}", describe(menu));
        let f: unsafe extern "C" fn(Id, Sel, i64) =
            std::mem::transmute(objc_msgSend as *const c_void);
        f(menu, sel("performActionForItemAtIndex:"), 0);
        // Close it once tracking is under way (a run loop turn later).
        let modes = super::msg_id_id(
            class("NSArray"),
            sel("arrayWithObject:"),
            nsstring("kCFRunLoopCommonModes"),
        );
        let later: unsafe extern "C" fn(Id, Sel, Sel, Id, f64, Id) =
            std::mem::transmute(objc_msgSend as *const c_void);
        later(
            menu,
            sel("performSelector:withObject:afterDelay:inModes:"),
            sel("cancelTracking"),
            std::ptr::null_mut(),
            0.3,
            modes,
        );
    }
}

/// The context menu closed with `picked` (an item's tag): report it and go
/// on with the self-test.
pub(super) fn popup_closed(picked: Option<i64>) {
    let next = CHECK.with(|c| c.borrow_mut().as_mut().and_then(|c| c.after_popup.take()));
    if active() {
        println!("Cocoa context menu: closed, picked {picked:?}");
    }
    if let Some(next) = next {
        unsafe { post_step(next) };
    }
}
