//! Predictive back on Android 13 and later: an `OnBackInvokedCallback`
//! registered on the activity's `OnBackInvokedDispatcher` while the app would
//! go back ([`set_back_enabled`](crate::platform::set_back_enabled)), and
//! unregistered while it wouldn't, so the system can preview its own back
//! animation (to the home screen) and go back without asking.
//!
//! With `android:enableOnBackInvokedCallback="true"` in the manifest, the
//! Back key and the back gesture no longer reach the native input queue on
//! Android 13 and later: they go to the topmost callback, ours while it's
//! registered (the soft keyboard's own sits above it while it's up). Without
//! the flag, or before Android 13, Back stays a key event and this changes
//! nothing that matters (Android asks the topmost callback, ours, which does
//! what the key would).
//!
//! The callback is a class of its own, assembled by
//! [`dex`](crate::platform::dex) and loaded separately from the backend's
//! other classes, because its interface only exists on Android 13 (and
//! `OnBackAnimationCallback`, with the gesture's progress, on Android 14):
//! [`dex::BACK_CALLBACK`] on API 33, [`dex::BACK_ANIMATION`] from API 34.
//! Everything here runs on the main (Java) thread.

use std::ffi::{CString, c_void};
use std::sync::{Mutex, OnceLock};

use super::jni::{self, Env, JObject, JValue, Jni};
use super::{ANativeActivity, Command, log, post_command};
use crate::platform::dex;
use crate::platform::{BackEdge, BackGesture, Event};

/// `OnBackInvokedDispatcher.PRIORITY_DEFAULT`.
const PRIORITY_DEFAULT: i32 = 0;
/// `WindowInsets.Type.ime()`.
const INSETS_TYPE_IME: i32 = 1 << 3;

/// The loaded callback class, as the address of a global reference, once
/// per process (`0`: it couldn't be loaded).
static CLASS: OnceLock<usize> = OnceLock::new();

/// The current activity's callback: a global reference (as an address,
/// `0` until made) and whether it's registered.
static CALLBACK: Mutex<(usize, bool)> = Mutex::new((0, false));

/// The class for this release: with the gesture's progress from API 34.
fn class_for(sdk: i32) -> dex::Class {
    if sdk >= 34 {
        dex::BACK_ANIMATION
    } else {
        dex::BACK_CALLBACK
    }
}

/// The Rust function behind each of the callback's natives.
fn function(name: &str) -> Option<*const c_void> {
    Some(match name {
        "onBackInvoked" => on_back_invoked as *const c_void,
        "onBackStarted" => on_back_started as *const c_void,
        "onBackProgressed" => on_back_progressed as *const c_void,
        "onBackCancelled" => on_back_cancelled as *const c_void,
        _ => return None,
    })
}

/// Load the callback class from a dex of its own (API 33+, so always with
/// `InMemoryDexClassLoader`), and bind its natives.
fn load(jni: &Jni, activity: &ANativeActivity) -> Option<JObject> {
    let class = class_for(activity.sdk_version);
    let parent = jni.call_object(
        activity.clazz,
        c"getClassLoader",
        c"()Ljava/lang/ClassLoader;",
        &[],
    )?;
    let bytes: &'static [u8] = Box::leak(dex::build(&[class]).into_boxed_slice());
    let buffer = jni.direct_buffer(bytes)?;
    let Some(loader) = jni.new_named(
        c"dalvik/system/InMemoryDexClassLoader",
        c"(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V",
        &[JValue { l: buffer }, JValue { l: parent }],
    ) else {
        jni.failed("creating the back callback's class loader");
        return None;
    };
    let name = jni.string(&dex::binary_name(class.name))?;
    let Some(loaded) = jni.call_object(
        loader,
        c"loadClass",
        c"(Ljava/lang/String;)Ljava/lang/Class;",
        &[JValue { l: name }],
    ) else {
        log(&format!("stipple: can't load {}", class.name));
        return None;
    };
    let mut methods = Vec::new();
    for n in class.natives {
        methods.push((
            CString::new(n.name).ok()?,
            CString::new(n.signature()).ok()?,
            function(n.name)?,
        ));
    }
    if !jni.register_natives(loaded, &methods) {
        log(&format!("stipple: can't bind {}'s natives", class.name));
        return None;
    }
    Some(jni.global(loaded))
}

/// Register the callback with `activity`'s dispatcher (`enabled`), or take
/// it away. Before Android 13, nothing to do. On the main thread.
pub(super) fn set(jni: &Jni, activity: &ANativeActivity, enabled: bool) {
    if activity.sdk_version < 33 {
        return;
    }
    let class = *CLASS.get_or_init(|| load(jni, activity).map_or(0, |c| c as usize));
    if class == 0 {
        return;
    }
    let mut callback = CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    if callback.1 == enabled {
        return;
    }
    let done = (|| {
        if callback.0 == 0 {
            let object = jni.new_object(class as JObject, c"()V", &[])?;
            callback.0 = jni.global(object) as usize;
        }
        let dispatcher = jni.call_object(
            activity.clazz,
            c"getOnBackInvokedDispatcher",
            c"()Landroid/window/OnBackInvokedDispatcher;",
            &[],
        )?;
        let object = JValue {
            l: callback.0 as JObject,
        };
        if enabled {
            jni.call_void(
                dispatcher,
                c"registerOnBackInvokedCallback",
                c"(ILandroid/window/OnBackInvokedCallback;)V",
                &[
                    JValue {
                        i: PRIORITY_DEFAULT,
                    },
                    object,
                ],
            )
        } else {
            jni.call_void(
                dispatcher,
                c"unregisterOnBackInvokedCallback",
                c"(Landroid/window/OnBackInvokedCallback;)V",
                &[object],
            )
        }
    })()
    .is_some();
    if done {
        callback.1 = enabled;
        log(if enabled {
            "stipple: back callback registered"
        } else {
            "stipple: back callback unregistered"
        });
    } else {
        log("stipple: can't change the back callback");
    }
}

/// The activity is going away (its dispatcher with it): let go of the
/// callback. On the main thread.
pub(super) fn uninstall(env: Env) {
    unsafe {
        jni::in_native(env, |jni| {
            let mut callback = CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
            if callback.0 != 0 {
                jni.delete_global(callback.0 as JObject);
            }
            *callback = (0, false);
            Some(())
        });
    }
}

/// Whether the soft keyboard is up (`WindowInsets.isVisible(ime())`, API 30).
fn keyboard_up(jni: &Jni) -> bool {
    let view = super::classes::host().view.get();
    if view.is_null() {
        return false;
    }
    (|| {
        let insets = jni.call_object(
            view,
            c"getRootWindowInsets",
            c"()Landroid/view/WindowInsets;",
            &[],
        )?;
        jni.call_bool(
            insets,
            c"isVisible",
            c"(I)Z",
            &[JValue { i: INSETS_TYPE_IME }],
        )
    })()
    .unwrap_or(false)
}

unsafe extern "C" fn on_back_invoked(env: Env, _this: JObject) {
    // Registered after the keyboard came up, the callback sits above the
    // keyboard's own: Back closes the keyboard first, as it does otherwise.
    let closed = unsafe {
        jni::in_native(env, |jni| {
            (keyboard_up(jni) && super::ime::soft_input(jni, false)).then_some(())
        })
    }
    .is_some();
    if closed {
        log("stipple: back closed the soft keyboard");
        return;
    }
    log("stipple: back from OnBackInvokedCallback");
    post_command(Command::Back);
}

/// A `BackEvent`'s edge and progress.
fn back_event(env: Env, event: JObject) -> Option<(BackEdge, f32)> {
    unsafe {
        jni::in_native(env, |jni| {
            let edge = jni.call_int(event, c"getSwipeEdge", c"()I", &[])?;
            let progress = jni.call_float(event, c"getProgress", c"()F", &[])?;
            Some((BackEdge::from_android(edge), progress))
        })
    }
}

unsafe extern "C" fn on_back_started(env: Env, _this: JObject, event: JObject) {
    let edge = back_event(env, event).map_or(BackEdge::None, |(edge, _)| edge);
    log(&format!("stipple: back gesture started ({edge:?})"));
    post_command(Command::Deliver(Event::BackProgress(
        BackGesture::Started { edge },
    )));
}

unsafe extern "C" fn on_back_progressed(env: Env, _this: JObject, event: JObject) {
    if let Some((_, progress)) = back_event(env, event) {
        post_command(Command::Deliver(Event::BackProgress(
            BackGesture::Progressed {
                progress: progress.clamp(0.0, 1.0),
            },
        )));
    }
}

unsafe extern "C" fn on_back_cancelled(_env: Env, _this: JObject) {
    log("stipple: back gesture cancelled");
    post_command(Command::Deliver(Event::BackProgress(
        BackGesture::Cancelled,
    )));
}
