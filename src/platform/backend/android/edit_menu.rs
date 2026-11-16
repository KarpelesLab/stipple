//! The system's edit menu over a text field's selection: a floating
//! `ActionMode` (`View.startActionMode(callback, TYPE_FLOATING)`) started on
//! the host view, as `TextView` does, on Android 8 and later.
//!
//! Its callback is an `ActionMode.Callback2` of our own, assembled by
//! [`dex`](crate::platform::dex) and loaded from a dex of its own (with
//! `InMemoryDexClassLoader`, hence Android 8), every method native:
//! `onPrepareActionMode` fills the menu with the items the App's menu offers
//! (titled with the system's own `android.R.string.cut`, `copy`, `paste` and
//! `selectAll`, so they're translated), `onGetContentRect` places it by the
//! selection, and `onActionItemClicked` hands the pick to the App as
//! [`Event::EditAction`] and closes the menu. A new menu for a mode already
//! showing invalidates it (its items and its place). Everything here runs on
//! the main (Java) thread, asked from the UI thread with `run_on_main`.

use std::ffi::{CString, c_void};
use std::sync::{Mutex, OnceLock};

use super::jni::{self, Env, JObject, JValue, Jni};
use super::{Command, log, post_command, run_on_main};
use crate::platform::dex;
use crate::platform::{EditAction, EditMenu, Event};

/// `ActionMode.TYPE_FLOATING`.
const TYPE_FLOATING: i32 = 1;
/// `MenuItem.SHOW_AS_ACTION_ALWAYS`.
const SHOW_AS_ACTION_ALWAYS: i32 = 2;

const ACTION_MODE: &str = "Landroid/view/ActionMode;";
const MENU: &str = "Landroid/view/Menu;";

/// The floating action mode's callback.
const CALLBACK: dex::Class = dex::Class {
    name: "Lorg/stipple/android/EditMode;",
    superclass: "Landroid/view/ActionMode$Callback2;",
    interfaces: &[],
    ctor: &[],
    natives: &[
        dex::Native {
            name: "onCreateActionMode",
            params: &[ACTION_MODE, MENU],
            ret: "Z",
        },
        dex::Native {
            name: "onPrepareActionMode",
            params: &[ACTION_MODE, MENU],
            ret: "Z",
        },
        dex::Native {
            name: "onActionItemClicked",
            params: &[ACTION_MODE, "Landroid/view/MenuItem;"],
            ret: "Z",
        },
        dex::Native {
            name: "onDestroyActionMode",
            params: &[ACTION_MODE],
            ret: "V",
        },
        dex::Native {
            name: "onGetContentRect",
            params: &[
                ACTION_MODE,
                "Landroid/view/View;",
                "Landroid/graphics/Rect;",
            ],
            ret: "V",
        },
    ],
    load_library: None,
};

/// The loaded callback class, as the address of a global reference, once
/// per process (`0`: it couldn't be loaded).
static CLASS: OnceLock<usize> = OnceLock::new();

/// What shows: the menu (its rectangle in the view's pixels) and the
/// action mode showing it (a global reference's address, `0` for none).
#[derive(Clone, Copy, Debug)]
struct State {
    menu: Option<EditMenu>,
    /// The menu's rectangle in physical pixels: left, top, right, bottom.
    rect: [i32; 4],
    mode: usize,
}

static STATE: Mutex<State> = Mutex::new(State {
    menu: None,
    rect: [0; 4],
    mode: 0,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the system shows the edit menu on Android `sdk`: a floating
/// action mode, with the callback loaded from memory (Android 8).
pub(super) fn available(sdk: i32) -> bool {
    sdk >= 26
}

/// The Rust function behind each of the callback's natives.
fn function(name: &str) -> Option<*const c_void> {
    Some(match name {
        "onCreateActionMode" => on_create as *const c_void,
        "onPrepareActionMode" => on_prepare as *const c_void,
        "onActionItemClicked" => on_clicked as *const c_void,
        "onDestroyActionMode" => on_destroy as *const c_void,
        "onGetContentRect" => on_get_content_rect as *const c_void,
        _ => return None,
    })
}

/// Load the callback class, through `activity`'s class loader, and bind its
/// natives.
fn load(jni: &Jni, activity: JObject) -> Option<JObject> {
    let parent = jni.call_object(
        activity,
        c"getClassLoader",
        c"()Ljava/lang/ClassLoader;",
        &[],
    )?;
    let bytes: &'static [u8] = Box::leak(dex::build(&[CALLBACK]).into_boxed_slice());
    let buffer = jni.direct_buffer(bytes)?;
    let Some(loader) = jni.new_named(
        c"dalvik/system/InMemoryDexClassLoader",
        c"(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V",
        &[JValue { l: buffer }, JValue { l: parent }],
    ) else {
        jni.failed("creating the edit menu's class loader");
        return None;
    };
    let name = jni.string(&dex::binary_name(CALLBACK.name))?;
    let Some(loaded) = jni.call_object(
        loader,
        c"loadClass",
        c"(Ljava/lang/String;)Ljava/lang/Class;",
        &[JValue { l: name }],
    ) else {
        log("stipple: can't load the edit menu's callback");
        return None;
    };
    let mut methods = Vec::new();
    for n in CALLBACK.natives {
        methods.push((
            CString::new(n.name).ok()?,
            CString::new(n.signature()).ok()?,
            function(n.name)?,
        ));
    }
    if !jni.register_natives(loaded, &methods) {
        log("stipple: can't bind the edit menu's natives");
        return None;
    }
    Some(jni.global(loaded))
}

/// Show `menu` (its rectangle in logical pixels, `scale` physical pixels
/// each) as a floating action mode on the host view, or close it (`None`).
/// From the UI thread.
pub(super) fn show(menu: Option<&EditMenu>, scale: f64) {
    let menu = menu.copied();
    let rect = menu.map_or([0; 4], |m| {
        let r = m.rect;
        [
            (r.min_x() * scale).floor() as i32,
            (r.min_y() * scale).floor() as i32,
            (r.max_x() * scale).ceil() as i32,
            (r.max_y() * scale).ceil() as i32,
        ]
    });
    run_on_main(move |jni, activity| {
        let mode = {
            let mut s = state();
            s.menu = menu;
            s.rect = rect;
            s.mode
        };
        if menu.is_none() {
            if mode != 0 {
                jni.call_void(mode as JObject, c"finish", c"()V", &[]);
            }
            return;
        }
        if mode != 0 {
            // Showing already: its items and its place change.
            jni.call_void(mode as JObject, c"invalidate", c"()V", &[]);
            jni.call_void(mode as JObject, c"invalidateContentRect", c"()V", &[]);
            log("stipple: edit menu updated");
            return;
        }
        let class = *CLASS.get_or_init(|| load(jni, activity).map_or(0, |c| c as usize));
        let view = super::classes::host().view.get();
        if class == 0 || view.is_null() {
            log("stipple: no edit menu (no callback or host view)");
            return;
        }
        let started = (|| {
            let callback = jni.new_object(class as JObject, c"()V", &[])?;
            let mode = jni.call_object(
                view,
                c"startActionMode",
                c"(Landroid/view/ActionMode$Callback;I)Landroid/view/ActionMode;",
                &[JValue { l: callback }, JValue { i: TYPE_FLOATING }],
            )?;
            (!mode.is_null()).then(|| jni.global(mode))
        })();
        match started {
            Some(mode) => {
                state().mode = mode as usize;
                log(&format!(
                    "stipple: edit menu shown: {}",
                    names(menu.as_ref())
                ));
            }
            None => log("stipple: the edit menu didn't start"),
        }
    });
}

/// The menu's items, for the log: `Cut,Copy,Paste,Select All`.
fn names(menu: Option<&EditMenu>) -> String {
    menu.map(|m| {
        m.actions()
            .iter()
            .map(|a| a.label())
            .collect::<Vec<_>>()
            .join(",")
    })
    .unwrap_or_default()
}

/// The system's string for `action`'s item (`android.R.string.*`).
fn title_res(action: EditAction) -> i32 {
    match action {
        EditAction::Copy => 0x0104_0001,
        EditAction::Cut => 0x0104_0003,
        EditAction::Paste => 0x0104_000b,
        EditAction::SelectAll => 0x0104_000d,
    }
}

unsafe extern "C" fn on_create(_env: Env, _this: JObject, _mode: JObject, _menu: JObject) -> u8 {
    1
}

/// Fill `menu` with the items the App's menu offers.
unsafe extern "C" fn on_prepare(env: Env, _this: JObject, _mode: JObject, menu: JObject) -> u8 {
    let wanted = state().menu;
    let Some(wanted) = wanted else {
        return 0;
    };
    unsafe {
        jni::in_native(env, |jni| {
            jni.call_void(menu, c"clear", c"()V", &[])?;
            for (order, action) in wanted.actions().into_iter().enumerate() {
                let item = jni.call_object(
                    menu,
                    c"add",
                    c"(IIII)Landroid/view/MenuItem;",
                    &[
                        JValue { i: 0 },
                        JValue { i: action.id() },
                        JValue { i: order as i32 },
                        JValue {
                            i: title_res(action),
                        },
                    ],
                )?;
                jni.call_void(
                    item,
                    c"setShowAsAction",
                    c"(I)V",
                    &[JValue {
                        i: SHOW_AS_ACTION_ALWAYS,
                    }],
                )?;
            }
            Some(())
        });
    }
    1
}

/// An item was tapped: the App applies it, and the menu closes.
unsafe extern "C" fn on_clicked(env: Env, _this: JObject, mode: JObject, item: JObject) -> u8 {
    let id = unsafe { jni::in_native(env, |jni| jni.call_int(item, c"getItemId", c"()I", &[])) };
    let Some(action) = id.and_then(EditAction::from_id) else {
        return 0;
    };
    log(&format!("stipple: edit menu picked {}", action.label()));
    post_command(Command::Deliver(Event::EditAction(action)));
    state().menu = None;
    unsafe {
        jni::in_native(env, |jni| jni.call_void(mode, c"finish", c"()V", &[]));
    }
    1
}

/// The mode ended (finished, or the system closed it): forget it.
unsafe extern "C" fn on_destroy(env: Env, _this: JObject, mode: JObject) {
    let ours = std::mem::take(&mut state().mode);
    if ours != 0 {
        unsafe {
            jni::in_native(env, |jni| {
                if !jni.same(ours as JObject, mode) {
                    // Another mode's end: keep ours.
                    state().mode = ours;
                    return None;
                }
                jni.delete_global(ours as JObject);
                Some(())
            });
        }
    }
    log("stipple: edit menu closed");
}

/// Where the menu points: the selection, in the view's pixels.
unsafe extern "C" fn on_get_content_rect(
    env: Env,
    _this: JObject,
    _mode: JObject,
    _view: JObject,
    rect: JObject,
) {
    let [l, t, r, b] = state().rect;
    unsafe {
        jni::in_native(env, |jni| {
            jni.call_void(
                rect,
                c"set",
                c"(IIII)V",
                &[
                    JValue { i: l },
                    JValue { i: t },
                    JValue { i: r },
                    JValue { i: b.max(t + 1) },
                ],
            )
        });
    }
}
