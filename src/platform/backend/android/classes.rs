//! The Java classes a `NativeActivity` lacks, defined at run time: a `View`
//! the backend adds to the activity (with an `InputConnection` for input
//! methods and an `AccessibilityNodeProvider` for screen readers),
//! `ComponentCallbacks2` for memory warnings, a headless `Fragment` that
//! hears activity and permission results (for [`pick`](super::pick) and
//! [`notify`](super::notify)), and `ActivityLifecycleCallbacks` that keep
//! that fragment out of the saved state.
//!
//! The classes are assembled into a dex file by [`dex`](crate::platform::dex)
//! (every method `native`, so there is no bytecode beyond the constructors),
//! loaded with `InMemoryDexClassLoader` on Android 8 and later (from a file,
//! with `DexClassLoader`, before), and their natives bound to the Rust
//! functions in [`ime`](super::ime), [`a11y`](super::a11y) and here with
//! `RegisterNatives`. Everything here runs on the main (Java) thread.

use std::ffi::{CString, c_void};
use std::sync::Mutex;
use std::sync::OnceLock;

use super::jni::{self, Env, JObject, JValue, Jni};
use super::{ANativeActivity, log};
use crate::platform::dex;

/// A global JNI reference, kept as an address so it can sit in a static.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Global(usize);

impl Global {
    pub(super) fn get(self) -> JObject {
        self.0 as JObject
    }
    fn is_null(self) -> bool {
        self.0 == 0
    }
}

/// The loaded classes, as global references, in [`dex::ANDROID_CLASSES`]'
/// order. Loaded once per process; `None` if loading failed.
static CLASSES: OnceLock<Option<Loaded>> = OnceLock::new();

/// The loaded classes, in [`dex::ANDROID_CLASSES`]' order.
type Loaded = [Global; dex::ANDROID_CLASSES.len()];

// Each class's index in [`dex::ANDROID_CLASSES`].
const HOST_VIEW: usize = 0;
const INPUT: usize = 1;
const NODES: usize = 2;
const CALLBACKS: usize = 3;
const RESULTS: usize = 4;
const LIFECYCLE: usize = 5;

/// Loaded class `index`, if loading worked.
fn class(index: usize) -> Option<JObject> {
    Some((*CLASSES.get()?)?[index].get())
}

/// The running activity's host view, its accessibility node provider and its
/// package name (all global references), while it has one.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Host {
    pub(super) view: Global,
    pub(super) provider: Global,
    pub(super) package: Global,
    pub(super) activity: Global,
}

static HOST: Mutex<Host> = Mutex::new(Host {
    view: Global(0),
    provider: Global(0),
    package: Global(0),
    activity: Global(0),
});

/// The current host view and friends (null when there is none).
pub(super) fn host() -> Host {
    *HOST.lock().unwrap_or_else(|e| e.into_inner())
}

/// The Rust function behind native method `name` of class `class`.
fn function(class: &str, name: &str) -> Option<*const c_void> {
    use super::{a11y, ime};
    Some(match (class, name) {
        ("HostView", "onCreateInputConnection") => ime::on_create_input_connection as *const c_void,
        ("HostView", "onCheckIsTextEditor") => ime::on_check_is_text_editor as *const c_void,
        ("HostView", "getAccessibilityNodeProvider") => a11y::node_provider as *const c_void,
        ("HostView", "dispatchHoverEvent") => a11y::dispatch_hover_event as *const c_void,
        ("Input", "commitText") => ime::commit_text as *const c_void,
        ("Input", "setComposingText") => ime::set_composing_text as *const c_void,
        ("Input", "finishComposingText") => ime::finish_composing_text as *const c_void,
        ("Input", "deleteSurroundingText") => ime::delete_surrounding_text as *const c_void,
        ("Input", "deleteSurroundingTextInCodePoints") => {
            ime::delete_surrounding_text_in_code_points as *const c_void
        }
        ("Input", "setComposingRegion") => ime::set_composing_region as *const c_void,
        ("Input", "setSelection") => ime::set_selection as *const c_void,
        ("Input", "getTextBeforeCursor") => ime::get_text_before_cursor as *const c_void,
        ("Input", "getTextAfterCursor") => ime::get_text_after_cursor as *const c_void,
        ("Input", "getSelectedText") => ime::get_selected_text as *const c_void,
        ("Input", "getCursorCapsMode") => ime::get_cursor_caps_mode as *const c_void,
        ("Input", "performEditorAction") => ime::perform_editor_action as *const c_void,
        ("Input", "performContextMenuAction") => ime::perform_context_menu_action as *const c_void,
        ("Nodes", "createAccessibilityNodeInfo") => a11y::create_node_info as *const c_void,
        ("Nodes", "performAction") => a11y::perform_action as *const c_void,
        ("Nodes", "findFocus") => a11y::find_focus as *const c_void,
        ("Callbacks", "onTrimMemory") => on_trim_memory as *const c_void,
        ("Callbacks", "onLowMemory") => on_low_memory as *const c_void,
        ("Callbacks", "onConfigurationChanged") => on_configuration_changed as *const c_void,
        ("Results", "onActivityResult") => super::pick::on_activity_result as *const c_void,
        ("Results", "onRequestPermissionsResult") => {
            super::notify::on_request_permissions_result as *const c_void
        }
        ("Lifecycle", "onActivitySaveInstanceState") => on_save_instance_state as *const c_void,
        ("Lifecycle", "onActivityCreated") => on_activity_bundle as *const c_void,
        ("Lifecycle", _) => on_activity as *const c_void,
        _ => return None,
    })
}

/// Load the classes into the process, once. On the main thread.
fn load(jni: &Jni, activity: &ANativeActivity) -> Option<Loaded> {
    let parent = jni.call_object(
        activity.clazz,
        c"getClassLoader",
        c"()Ljava/lang/ClassLoader;",
        &[],
    )?;
    let bytes: &'static [u8] = Box::leak(dex::build(&dex::ANDROID_CLASSES).into_boxed_slice());
    let loader = if activity.sdk_version >= 26 {
        let buffer = jni.direct_buffer(bytes)?;
        jni.new_named(
            c"dalvik/system/InMemoryDexClassLoader",
            c"(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V",
            &[JValue { l: buffer }, JValue { l: parent }],
        )
    } else {
        // Before Android 8, from a file in the app's private storage.
        let dir = unsafe { std::ffi::CStr::from_ptr(activity.internal_data_path) }
            .to_str()
            .ok()?;
        let path = format!("{dir}/stipple-classes.dex");
        std::fs::write(&path, bytes).ok()?;
        let (jpath, jdir) = (jni.string(&path)?, jni.string(dir)?);
        jni.new_named(
            c"dalvik/system/DexClassLoader",
            c"(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/ClassLoader;)V",
            &[
                JValue { l: jpath },
                JValue { l: jdir },
                JValue {
                    l: std::ptr::null_mut(),
                },
                JValue { l: parent },
            ],
        )
    };
    let Some(loader) = loader else {
        jni.failed("creating the class loader");
        return None;
    };
    let mut out = [Global::default(); dex::ANDROID_CLASSES.len()];
    for (slot, class) in out.iter_mut().zip(dex::ANDROID_CLASSES) {
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
        let short = class
            .name
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .trim_end_matches(';');
        let mut methods = Vec::new();
        for n in class.natives {
            let f = function(short, n.name)?;
            methods.push((
                CString::new(n.name).ok()?,
                CString::new(n.signature()).ok()?,
                f,
            ));
        }
        if !jni.register_natives(loaded, &methods) {
            log(&format!("stipple: can't bind {}'s natives", class.name));
            return None;
        }
        *slot = Global(jni.global(loaded) as usize);
    }
    Some(out)
}

/// Load the classes (once per process) and give `activity` a host view:
/// focusable, filling the content, on top of the `NativeActivity`'s own. It
/// draws nothing (the native window shows through) and takes no input of its
/// own (the input queue gets it first); it's there for the input method and
/// accessibility services. Also hears memory warnings. Called from
/// `ANativeActivity_onCreate`, on the main thread. Returns whether it worked;
/// if not, the app runs as before (keys only, no accessibility).
pub(super) fn install(activity: &ANativeActivity) -> bool {
    let ok = unsafe {
        jni::in_native(activity.env, |jni| {
            let classes = (*CLASSES.get_or_init(|| load(jni, activity)))?;
            let (view_class, callbacks_class) = (classes[HOST_VIEW], classes[CALLBACKS]);
            let view = jni.new_object(
                view_class.get(),
                c"(Landroid/content/Context;)V",
                &[JValue { l: activity.clazz }],
            )?;
            let yes = [JValue { z: 1 }];
            jni.call_void(view, c"setFocusable", c"(Z)V", &yes)?;
            jni.call_void(view, c"setFocusableInTouchMode", c"(Z)V", &yes)?;
            // IMPORTANT_FOR_ACCESSIBILITY_YES.
            jni.call_void(
                view,
                c"setImportantForAccessibility",
                c"(I)V",
                &[JValue { i: 1 }],
            )?;
            // MATCH_PARENT both ways.
            let params = jni.new_named(
                c"android/view/ViewGroup$LayoutParams",
                c"(II)V",
                &[JValue { i: -1 }, JValue { i: -1 }],
            )?;
            jni.call_void(
                activity.clazz,
                c"addContentView",
                c"(Landroid/view/View;Landroid/view/ViewGroup$LayoutParams;)V",
                &[JValue { l: view }, JValue { l: params }],
            )?;
            jni.call_bool(view, c"requestFocus", c"()Z", &[])?;
            let package = jni.call_object(
                activity.clazz,
                c"getPackageName",
                c"()Ljava/lang/String;",
                &[],
            )?;
            let callbacks = jni.new_object(callbacks_class.get(), c"()V", &[])?;
            jni.call_void(
                activity.clazz,
                c"registerComponentCallbacks",
                c"(Landroid/content/ComponentCallbacks;)V",
                &[JValue { l: callbacks }],
            )?;
            register_lifecycle(jni, activity.clazz);
            let mut host = HOST.lock().unwrap_or_else(|e| e.into_inner());
            release(jni, &mut host);
            *host = Host {
                view: Global(jni.global(view) as usize),
                provider: Global::default(),
                package: Global(jni.global(package) as usize),
                activity: Global(jni.global(activity.clazz) as usize),
            };
            Some(())
        })
    }
    .is_some();
    if ok {
        log("stipple: host view installed (input connection, accessibility)");
    } else {
        log("stipple: no host view; the soft keyboard types through key events only");
    }
    ok
}

fn release(jni: &Jni, host: &mut Host) {
    for g in [host.view, host.provider, host.package, host.activity] {
        jni.delete_global(g.get());
    }
    *host = Host::default();
    let mut fragment = FRAGMENT.lock().unwrap_or_else(|e| e.into_inner());
    jni.delete_global(fragment.get());
    *fragment = Global::default();
}

/// The activity's [`dex::RESULTS`] fragment, once added.
static FRAGMENT: Mutex<Global> = Mutex::new(Global(0));

/// The headless fragment that hears activity and permission results for the
/// current activity, added the first time it's asked for. On the main thread
/// (`activity` is the `NativeActivity`).
pub(super) fn results_fragment(jni: &Jni, activity: JObject) -> Option<JObject> {
    let have = *FRAGMENT.lock().unwrap_or_else(|e| e.into_inner());
    if !have.is_null() {
        return Some(have.get());
    }
    let fragment = jni.new_object(class(RESULTS)?, c"()V", &[])?;
    let manager = jni.call_object(
        activity,
        c"getFragmentManager",
        c"()Landroid/app/FragmentManager;",
        &[],
    )?;
    let transaction = jni.call_object(
        manager,
        c"beginTransaction",
        c"()Landroid/app/FragmentTransaction;",
        &[],
    )?;
    let tag = jni.string("stipple-results")?;
    jni.call_object(
        transaction,
        c"add",
        c"(Landroid/app/Fragment;Ljava/lang/String;)Landroid/app/FragmentTransaction;",
        &[JValue { l: fragment }, JValue { l: tag }],
    )?;
    // The state may already be saved (the activity is stopping); the
    // fragment has no state to lose.
    jni.call_int(transaction, c"commitAllowingStateLoss", c"()I", &[])?;
    // Attached now, so it can start activities.
    jni.call_bool(manager, c"executePendingTransactions", c"()Z", &[])?;
    let global = Global(jni.global(fragment) as usize);
    *FRAGMENT.lock().unwrap_or_else(|e| e.into_inner()) = global;
    log("stipple: results fragment added");
    Some(global.get())
}

/// Register the [`dex::LIFECYCLE`] callbacks with the application, once per
/// process.
fn register_lifecycle(jni: &Jni, activity: JObject) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let registered = (|| {
        let app = jni.call_object(
            activity,
            c"getApplication",
            c"()Landroid/app/Application;",
            &[],
        )?;
        let callbacks = jni.new_object(class(LIFECYCLE)?, c"()V", &[])?;
        jni.call_void(
            app,
            c"registerActivityLifecycleCallbacks",
            c"(Landroid/app/Application$ActivityLifecycleCallbacks;)V",
            &[JValue { l: callbacks }],
        )
    })();
    if registered.is_some() {
        DONE.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// `onActivitySaveInstanceState`, after the activity wrote its fragments into
/// `state`: drop them again. The framework would restore the
/// [`dex::RESULTS`] fragment through the app's class loader, which doesn't
/// have the class, and crash; and whoever waited for its results is gone with
/// the process anyway.
unsafe extern "C" fn on_save_instance_state(
    env: Env,
    _this: JObject,
    activity: JObject,
    state: JObject,
) {
    let ours = host().activity;
    if ours.is_null() || state.is_null() {
        return;
    }
    unsafe {
        jni::in_native(env, |jni| {
            if !jni.same(activity, ours.get()) {
                return None;
            }
            let key = jni.string("android:fragments")?;
            jni.call_void(
                state,
                c"remove",
                c"(Ljava/lang/String;)V",
                &[JValue { l: key }],
            )
        });
    }
}

unsafe extern "C" fn on_activity_bundle(_env: Env, _this: JObject, _a: JObject, _b: JObject) {}

unsafe extern "C" fn on_activity(_env: Env, _this: JObject, _a: JObject) {}

/// The activity is going away: let go of its view. On the main thread.
pub(super) fn uninstall(env: Env) {
    unsafe {
        jni::in_native(env, |jni| {
            release(jni, &mut HOST.lock().unwrap_or_else(|e| e.into_inner()));
            Some(())
        });
    }
}

/// The node provider for the host view, made the first time it's asked for.
pub(super) fn provider(jni: &Jni) -> Option<JObject> {
    let mut host = HOST.lock().unwrap_or_else(|e| e.into_inner());
    if host.view.is_null() {
        return None;
    }
    if host.provider.is_null() {
        let nodes = class(NODES)?;
        let provider = jni.new_object(nodes, c"()V", &[])?;
        host.provider = Global(jni.global(provider) as usize);
    }
    Some(host.provider.get())
}

/// The `Input` class, for a new connection.
pub(super) fn input_class() -> Option<JObject> {
    class(INPUT)
}

/// `ComponentCallbacks2.TRIM_MEMORY_*` levels that mean memory is short:
/// running low or critical while in the foreground, or moderate and complete
/// in the background (the process is next in line to be killed).
fn memory_is_short(level: i32) -> bool {
    matches!(level, 10 | 15 | 60 | 80)
}

unsafe extern "C" fn on_trim_memory(_env: Env, _this: JObject, level: i32) {
    if memory_is_short(level) {
        log(&format!("stipple: trim memory, level {level}"));
        super::post_command(super::Command::LowMemory);
    }
}

unsafe extern "C" fn on_low_memory(_env: Env, _this: JObject) {
    // NativeActivity reports this one itself, through onLowMemory.
}

unsafe extern "C" fn on_configuration_changed(_env: Env, _this: JObject, _config: JObject) {
    // NativeActivity reports this one itself, through onConfigurationChanged.
}
