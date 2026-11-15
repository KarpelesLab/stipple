//! Runtime permissions, for [`platform::permission`](crate::platform::permission):
//! `checkSelfPermission` for where one stands, and `requestPermissions` from
//! the headless [`Results`](crate::platform::dex::RESULTS) fragment to ask,
//! since a `NativeActivity` doesn't pass `onRequestPermissionsResult` on.
//!
//! Each request gets a request code of its own (from `0x1_0000`), and its
//! answer goes to the callback [`ask`] was given, on the main thread.
//! Notifications ([`notify`](super::notify)) ask through here too.
//!
//! Android doesn't tell "never asked" from "refused" (both are just not
//! granted), so the permissions asked for are written down in a file of the
//! app's (`stipple-permissions-asked`): not granted and never asked is
//! [`NotDetermined`](PermissionStatus::NotDetermined), not granted after
//! asking is [`Denied`](PermissionStatus::Denied). Before Android 6 every
//! permission is granted at install.

use std::collections::HashMap;
use std::sync::Mutex;

use super::jni::{self, Env, JObject, JValue, Jni};
use super::{classes, log, run_on_main, with_activity};
use crate::platform::PlatformError;
use crate::platform::permission::{Done, Permission, PermissionStatus};

/// What hears a request's answer: on the main thread, with the activity, and
/// whether the first permission was granted (`None` if the request was
/// interrupted).
pub(super) type Answer = Box<dyn FnOnce(&Jni, JObject, Option<bool>) + Send>;

/// The first request code; the document picker's are under `0x8000`.
const FIRST_CODE: i32 = 0x1_0000;

/// Requests out, by request code, and the next code.
static OUT: Mutex<Option<(i32, HashMap<i32, Answer>)>> = Mutex::new(None);

/// The file listing the permissions asked for, one per line.
const ASKED_FILE: &str = "stipple-permissions-asked";

/// The app's files directory, as the activity last said
/// (`internalDataPath`): read here rather than through
/// [`dirs`](crate::platform::dirs), which would take the activity's lock
/// this is called under.
static DIR: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

fn remember_dir(a: &super::ANativeActivity) {
    if a.internal_data_path.is_null() {
        return;
    }
    let dir = unsafe { std::ffi::CStr::from_ptr(a.internal_data_path) }
        .to_string_lossy()
        .into_owned();
    if !dir.is_empty() {
        *DIR.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.into());
    }
}

fn asked_path() -> Option<std::path::PathBuf> {
    let dir = DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    Some(dir.join(ASKED_FILE))
}

fn was_asked(name: &str) -> bool {
    asked_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|s| s.lines().any(|l| l == name))
}

fn mark_asked(names: &[&str]) {
    let Some(path) = asked_path() else {
        return;
    };
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    for name in names {
        if !text.lines().any(|l| l == *name) {
            text.push_str(name);
            text.push('\n');
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

/// Ask for `names` (from the results fragment, on the main thread), and
/// tell `answer`. If there's no activity to ask from, `answer` isn't called
/// and this returns `false`.
pub(super) fn ask(names: &[&str], answer: Answer) -> bool {
    let code = {
        let mut out = OUT.lock().unwrap_or_else(|e| e.into_inner());
        let (next, map) = out.get_or_insert_with(|| (FIRST_CODE, HashMap::new()));
        let code = *next;
        *next = if *next >= 0x1_FFFF {
            FIRST_CODE
        } else {
            *next + 1
        };
        map.insert(code, answer);
        code
    };
    let owned: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    let queued = run_on_main(move |jni, activity| {
        let names: Vec<&str> = owned.iter().map(String::as_str).collect();
        let asked = (|| {
            let fragment = classes::results_fragment(jni, activity)?;
            let permissions = jni.string_array(&names)?;
            jni.call_void(
                fragment,
                c"requestPermissions",
                c"([Ljava/lang/String;I)V",
                &[JValue { l: permissions }, JValue { i: code }],
            )
        })();
        if asked.is_some() {
            mark_asked(&names);
            log(&format!("stipple: asking for {names:?}"));
        } else {
            answered(jni, activity, code, None);
        }
    });
    if !queued {
        take(code);
    }
    queued
}

fn take(code: i32) -> Option<Answer> {
    OUT.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|(_, map)| map.remove(&code))
}

fn answered(jni: &Jni, activity: JObject, code: i32, granted: Option<bool>) {
    if let Some(f) = take(code) {
        f(jni, activity, granted);
    }
}

/// `Results.onRequestPermissionsResult`. On the main thread.
pub(super) unsafe extern "C" fn on_request_permissions_result(
    env: Env,
    this: JObject,
    code: i32,
    _permissions: JObject,
    results: JObject,
) {
    unsafe {
        jni::in_native(env, |jni| {
            // Empty when the request was interrupted. Granted if any was
            // (fine or coarse location).
            let results = jni.int_vec(results);
            let granted = (!results.is_empty()).then(|| results.contains(&0));
            let activity =
                jni.call_object(this, c"getActivity", c"()Landroid/app/Activity;", &[])?;
            answered(jni, activity, code, granted);
            Some(())
        });
    }
}

/// The Android permissions behind `permission` on API level `sdk`; empty for
/// none to ask.
fn names(permission: Permission, sdk: i32) -> &'static [&'static str] {
    match permission {
        Permission::Camera => &["android.permission.CAMERA"],
        Permission::Microphone => &["android.permission.RECORD_AUDIO"],
        Permission::Notifications if sdk >= 33 => &["android.permission.POST_NOTIFICATIONS"],
        Permission::Notifications => &[],
        Permission::Photos if sdk >= 33 => &["android.permission.READ_MEDIA_IMAGES"],
        Permission::Photos => &["android.permission.READ_EXTERNAL_STORAGE"],
        Permission::Location => &[
            "android.permission.ACCESS_FINE_LOCATION",
            "android.permission.ACCESS_COARSE_LOCATION",
        ],
    }
}

/// Whether the app holds `name`.
pub(super) fn granted(jni: &Jni, context: JObject, name: &str) -> bool {
    let Some(permission) = jni.string(name) else {
        return false;
    };
    jni.call_int(
        context,
        c"checkSelfPermission",
        c"(Ljava/lang/String;)I",
        &[JValue { l: permission }],
    ) == Some(0)
}

/// Where `permission` stands, with the env and the activity.
fn status_with(jni: &Jni, context: JObject, sdk: i32, permission: Permission) -> PermissionStatus {
    if permission == Permission::Notifications && !super::notify::enabled(jni, context) {
        return PermissionStatus::Denied;
    }
    let names = names(permission, sdk);
    if sdk < 23 || names.is_empty() || names.iter().any(|n| granted(jni, context, n)) {
        return PermissionStatus::Granted;
    }
    if names.iter().any(|n| was_asked(n)) {
        PermissionStatus::Denied
    } else {
        PermissionStatus::NotDetermined
    }
}

pub(crate) fn status(permission: Permission) -> PermissionStatus {
    with_activity(|a| {
        remember_dir(a);
        let (vm, context, sdk) = (a.vm, a.clazz, a.sdk_version);
        unsafe { jni::with_env(vm, |jni| Some(status_with(jni, context, sdk, permission))) }
    })
    .unwrap_or(PermissionStatus::NotDetermined)
}

pub(crate) fn request(permission: Permission, done: Done) {
    let now = status(permission);
    if now != PermissionStatus::NotDetermined {
        done(now);
        return;
    }
    let sdk = with_activity(|a| Some(a.sdk_version)).unwrap_or(0);
    if permission == Permission::Notifications {
        super::notify::ask(Box::new(move |_| done(status(Permission::Notifications))));
        return;
    }
    let names = names(permission, sdk);
    let done = std::sync::Arc::new(Mutex::new(Some(done)));
    let later = done.clone();
    let asked = ask(
        names,
        Box::new(move |jni, activity, _granted| {
            let sdk = with_activity(|a| Some(a.sdk_version)).unwrap_or(sdk);
            let status = status_with(jni, activity, sdk, permission);
            if let Some(done) = later.lock().unwrap_or_else(|e| e.into_inner()).take() {
                done(status);
            }
        }),
    );
    if !asked && let Some(done) = done.lock().unwrap_or_else(|e| e.into_inner()).take() {
        done(PermissionStatus::NotDetermined);
    }
}

pub(crate) fn open_settings() -> Result<(), PlatformError> {
    let opened = with_activity(|a| {
        let (vm, activity) = (a.vm, a.clazz);
        let package = unsafe {
            jni::with_env(vm, |jni| {
                let p =
                    jni.call_object(activity, c"getPackageName", c"()Ljava/lang/String;", &[])?;
                jni.to_rust(p)
            })
        }?;
        let uri = format!("package:{package}");
        unsafe {
            jni::start_intent(
                vm,
                activity,
                "android.settings.APPLICATION_DETAILS_SETTINGS",
                &uri,
            )
        }
        .then_some(())
    });
    opened.ok_or_else(|| PlatformError::Os("can't open the app's settings".into()))
}
