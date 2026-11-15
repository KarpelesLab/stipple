//! Notifications, for [`platform::notify`](crate::platform::notify):
//! `NotificationManager.notify` with a `Notification.Builder`, on a channel of
//! the app's (`stipple`, default importance, made on Android 8 and later).
//! Tapping one opens the app (its launch intent, as a `PendingIntent`).
//!
//! On Android 13 and later posting needs the `POST_NOTIFICATIONS` runtime
//! permission (which the app's manifest must declare). The first [`notify`]
//! without it asks (`requestPermissions` from the headless
//! [`Results`](crate::platform::dex::RESULTS) fragment, whose
//! `onRequestPermissionsResult` comes back here), and the notification waits
//! for the answer, as on iOS; once refused, `notify` fails, until the user
//! allows notifications in the settings. Before Android 13 there's nothing to
//! ask, but the user may have turned the app's notifications off, which
//! `notify` reports too.
//!
//! [`request_permission`] asks the same way without a notification waiting,
//! and [`post_scheduled`] posts one that [`alarm`](super::alarm) scheduled,
//! under a notification id made from the app's id, its tap opening the app
//! with that id in the launch intent ([`TAPPED_EXTRA`]).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use super::jni::{self, Env, JObject, JValue, Jni};
use super::{classes, log, run_on_main, with_activity};
use crate::platform::PlatformError;
use crate::platform::shell::Notification;

const PERMISSION: &str = "android.permission.POST_NOTIFICATIONS";
/// The request code of the permission request (above the document
/// picker's, which are under 0x8000).
const PERMISSION_CODE: i32 = 0x1_0000;
const CHANNEL: &str = "stipple";

/// The permission request: not made (or answered inconclusively), waiting
/// for the answer, allowed, refused.
const UNKNOWN: u8 = 0;
const ASKING: u8 = 1;
const ALLOWED: u8 = 2;
const REFUSED: u8 = 3;
static ANSWER: AtomicU8 = AtomicU8::new(UNKNOWN);

/// Notifications waiting for the permission request's answer.
static WAITING: Mutex<Vec<Notification>> = Mutex::new(Vec::new());

/// [`request_permission`]'s callers waiting for the answer.
type PermissionFn = Box<dyn FnOnce(bool) + Send>;
static ASKERS: Mutex<Vec<PermissionFn>> = Mutex::new(Vec::new());

/// The launch intent's extra naming the scheduled notification tapped, and
/// when it was posted (to tell one tap from another).
pub(super) const TAPPED_EXTRA: &str = "org.stipple.notification";
pub(super) const TAPPED_AT_EXTRA: &str = "org.stipple.notification.at";

/// Whether the channel has been made, in this process.
static CHANNEL_MADE: AtomicBool = AtomicBool::new(false);

fn os(msg: &str) -> PlatformError {
    PlatformError::Os(msg.into())
}

pub(crate) fn notify(n: &Notification) -> Result<(), PlatformError> {
    // Post now, if allowed.
    let posted = with_activity(|a| {
        let (vm, context, sdk) = (a.vm, a.clazz, a.sdk_version);
        Some(unsafe {
            jni::with_env(vm, |jni| {
                if sdk >= 33 && !granted(jni, context) {
                    return Some(None);
                }
                Some(Some(post(jni, context, sdk, n, None)))
            })
        })
    })
    .ok_or_else(|| os("no activity"))?
    .ok_or_else(|| os("JNI unavailable"))?;
    if let Some(result) = posted {
        return result;
    }
    // Ask for the permission, and post once it's answered.
    if ANSWER.load(Ordering::Acquire) == REFUSED {
        return Err(os("notifications aren't allowed"));
    }
    WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(n.clone());
    if !ask() {
        return Err(os("no activity"));
    }
    Ok(())
}

/// Ask for `POST_NOTIFICATIONS` (unless a request is out already), from the
/// results fragment. `false` if there's no activity to ask from.
fn ask() -> bool {
    if ANSWER.swap(ASKING, Ordering::AcqRel) != ASKING {
        let asking = run_on_main(|jni, activity| {
            let asked = (|| {
                let fragment = classes::results_fragment(jni, activity)?;
                let permissions = jni.string_array(&[PERMISSION])?;
                jni.call_void(
                    fragment,
                    c"requestPermissions",
                    c"([Ljava/lang/String;I)V",
                    &[JValue { l: permissions }, JValue { i: PERMISSION_CODE }],
                )
            })();
            if asked.is_some() {
                log("stipple: asking for the notification permission");
            } else {
                answered(jni, activity, None);
            }
        });
        if !asking {
            ANSWER.store(UNKNOWN, Ordering::Release);
            WAITING.lock().unwrap_or_else(|e| e.into_inner()).clear();
            for f in std::mem::take(&mut *ASKERS.lock().unwrap_or_else(|e| e.into_inner())) {
                f(false);
            }
            return false;
        }
    }
    true
}

/// Ask for the permission to post (Android 13 and later), and tell `done`
/// whether notifications may be shown. Before 13, whether the user left
/// them on.
pub(crate) fn request_permission(done: PermissionFn) {
    let now = with_activity(|a| {
        let (vm, context, sdk) = (a.vm, a.clazz, a.sdk_version);
        unsafe {
            jni::with_env(vm, |jni| {
                if sdk >= 33 && !granted(jni, context) {
                    return Some(None);
                }
                Some(Some(enabled(jni, context)))
            })
        }
    });
    match now {
        Some(Some(enabled)) => done(enabled),
        None => done(false),
        Some(None) => {
            ASKERS.lock().unwrap_or_else(|e| e.into_inner()).push(done);
            ask();
        }
    }
}

/// Whether the user left the app's notifications on.
fn enabled(jni: &Jni, context: JObject) -> bool {
    manager(jni, context)
        .and_then(|m| jni.call_bool(m, c"areNotificationsEnabled", c"()Z", &[]))
        .unwrap_or(true)
}

/// The `NotificationManager`.
pub(super) fn manager(jni: &Jni, context: JObject) -> Option<JObject> {
    let name = jni.string("notification")?;
    jni.call_object(
        context,
        c"getSystemService",
        c"(Ljava/lang/String;)Ljava/lang/Object;",
        &[JValue { l: name }],
    )
}

/// Post `n`, scheduled under the app's `id`, now (when its alarm went off):
/// from a broadcast receiver, so nothing is asked. On the main thread.
pub(super) fn post_scheduled(
    jni: &Jni,
    context: JObject,
    sdk: i32,
    id: &str,
    n: &Notification,
) -> Result<(), PlatformError> {
    if sdk >= 33 && !granted(jni, context) {
        return Err(os("notifications aren't allowed"));
    }
    post(jni, context, sdk, n, Some(id))
}

/// Whether the app holds `POST_NOTIFICATIONS`.
fn granted(jni: &Jni, context: JObject) -> bool {
    let Some(permission) = jni.string(PERMISSION) else {
        return false;
    };
    jni.call_int(
        context,
        c"checkSelfPermission",
        c"(Ljava/lang/String;)I",
        &[JValue { l: permission }],
    ) == Some(0)
}

/// The permission request's answer (`None` when it didn't conclude): post
/// what waited, or drop it. On the main thread.
fn answered(jni: &Jni, context: JObject, granted: Option<bool>) {
    ANSWER.store(
        match granted {
            Some(true) => ALLOWED,
            Some(false) => REFUSED,
            None => UNKNOWN,
        },
        Ordering::Release,
    );
    log(&format!("stipple: notification permission {granted:?}"));
    for f in std::mem::take(&mut *ASKERS.lock().unwrap_or_else(|e| e.into_inner())) {
        f(granted == Some(true));
    }
    let waiting = std::mem::take(&mut *WAITING.lock().unwrap_or_else(|e| e.into_inner()));
    let sdk = with_activity(|a| Some(a.sdk_version)).unwrap_or(33);
    for n in waiting {
        let result = if granted == Some(true) {
            post(jni, context, sdk, &n, None)
        } else {
            Err(os("notifications aren't allowed"))
        };
        if let Err(e) = result {
            eprintln!("stipple: notification {:?} not shown: {e}", n.title);
        }
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
    if code != PERMISSION_CODE {
        return;
    }
    unsafe {
        jni::in_native(env, |jni| {
            // Empty when the request was interrupted.
            let granted = jni.int_vec(results).first().map(|&r| r == 0);
            let activity =
                jni.call_object(this, c"getActivity", c"()Landroid/app/Activity;", &[])?;
            answered(jni, activity, granted);
            Some(())
        });
    }
}

/// Post `n` through the `NotificationManager`; `tag` is the app's id for a
/// scheduled one.
fn post(
    jni: &Jni,
    context: JObject,
    sdk: i32,
    n: &Notification,
    tag: Option<&str>,
) -> Result<(), PlatformError> {
    let failed = || os("NotificationManager refused the notification");
    let manager = manager(jni, context).ok_or_else(|| os("no NotificationManager"))?;
    if jni.call_bool(manager, c"areNotificationsEnabled", c"()Z", &[]) == Some(false) {
        return Err(os("notifications are turned off for the app"));
    }
    if sdk >= 26 && !CHANNEL_MADE.load(Ordering::Acquire) {
        make_channel(jni, manager).ok_or_else(|| os("can't make the notification channel"))?;
        CHANNEL_MADE.store(true, Ordering::Release);
    }
    let builder = if sdk >= 26 {
        let channel = jni.string(CHANNEL).ok_or_else(failed)?;
        jni.new_named(
            c"android/app/Notification$Builder",
            c"(Landroid/content/Context;Ljava/lang/String;)V",
            &[JValue { l: context }, JValue { l: channel }],
        )
    } else {
        jni.new_named(
            c"android/app/Notification$Builder",
            c"(Landroid/content/Context;)V",
            &[JValue { l: context }],
        )
    }
    .ok_or_else(failed)?;
    const SET: &std::ffi::CStr = c"(Ljava/lang/CharSequence;)Landroid/app/Notification$Builder;";
    let icon = small_icon(jni, context);
    jni.call_object(
        builder,
        c"setSmallIcon",
        c"(I)Landroid/app/Notification$Builder;",
        &[JValue { i: icon }],
    )
    .ok_or_else(failed)?;
    let title = jni.string16(&n.title).ok_or_else(failed)?;
    jni.call_object(builder, c"setContentTitle", SET, &[JValue { l: title }])
        .ok_or_else(failed)?;
    if !n.body.is_empty() {
        let body = jni.string16(&n.body).ok_or_else(failed)?;
        jni.call_object(builder, c"setContentText", SET, &[JValue { l: body }])
            .ok_or_else(failed)?;
    }
    jni.call_object(
        builder,
        c"setAutoCancel",
        c"(Z)Landroid/app/Notification$Builder;",
        &[JValue { z: 1 }],
    );
    if let Some(open) = launch_intent(jni, context, tag) {
        jni.call_object(
            builder,
            c"setContentIntent",
            c"(Landroid/app/PendingIntent;)Landroid/app/Notification$Builder;",
            &[JValue { l: open }],
        );
    }
    let notification = jni
        .call_object(builder, c"build", c"()Landroid/app/Notification;", &[])
        .ok_or_else(failed)?;
    static NEXT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);
    let id = match tag {
        Some(tag) => scheduled_id(tag),
        None => NEXT.fetch_add(1, Ordering::Relaxed),
    };
    jni.call_void(
        manager,
        c"notify",
        c"(ILandroid/app/Notification;)V",
        &[JValue { i: id }, JValue { l: notification }],
    )
    .ok_or_else(failed)?;
    log(&format!("stipple: notification {id} posted"));
    Ok(())
}

/// The notification id of the one scheduled under `tag`: the last posted
/// under it is replaced. Negative, so it never meets the others, numbered
/// from 1.
pub(super) fn scheduled_id(tag: &str) -> i32 {
    !crate::platform::shell::schedule::int_id(tag)
}

/// The app's channel: `new NotificationChannel("stipple", "Notifications",
/// IMPORTANCE_DEFAULT)`. Making it again is harmless.
fn make_channel(jni: &Jni, manager: JObject) -> Option<()> {
    let id = jni.string(CHANNEL)?;
    let name = jni.string("Notifications")?;
    // NotificationManager.IMPORTANCE_DEFAULT.
    let importance = 3;
    let channel = jni.new_named(
        c"android/app/NotificationChannel",
        c"(Ljava/lang/String;Ljava/lang/CharSequence;I)V",
        &[
            JValue { l: id },
            JValue { l: name },
            JValue { i: importance },
        ],
    )?;
    jni.call_void(
        manager,
        c"createNotificationChannel",
        c"(Landroid/app/NotificationChannel;)V",
        &[JValue { l: channel }],
    )
}

/// The notification's icon: the app's, else the system's info icon (a
/// notification without one is refused).
fn small_icon(jni: &Jni, context: JObject) -> i32 {
    jni.call_object(
        context,
        c"getApplicationInfo",
        c"()Landroid/content/pm/ApplicationInfo;",
        &[],
    )
    .and_then(|info| jni.int_field(info, c"icon"))
    .filter(|&icon| icon != 0)
    .or_else(|| jni.static_int(c"android/R$drawable", c"ic_dialog_info"))
    .unwrap_or(0)
}

/// A `PendingIntent` that opens the app, saying it was for scheduled
/// notification `tag`.
fn launch_intent(jni: &Jni, context: JObject, tag: Option<&str>) -> Option<JObject> {
    let pm = jni.call_object(
        context,
        c"getPackageManager",
        c"()Landroid/content/pm/PackageManager;",
        &[],
    )?;
    let package = jni.call_object(context, c"getPackageName", c"()Ljava/lang/String;", &[])?;
    let intent = jni.call_object(
        pm,
        c"getLaunchIntentForPackage",
        c"(Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue { l: package }],
    )?;
    let mut code = 0;
    if let Some(tag) = tag {
        code = crate::platform::shell::schedule::int_id(tag);
        let (key, value) = (jni.string(TAPPED_EXTRA)?, jni.string16(tag)?);
        jni.call_object(
            intent,
            c"putExtra",
            c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue { l: key }, JValue { l: value }],
        )?;
        let key = jni.string(TAPPED_AT_EXTRA)?;
        let at = crate::platform::shell::schedule::now_ms() as i64;
        jni.call_object(
            intent,
            c"putExtra",
            c"(Ljava/lang/String;J)Landroid/content/Intent;",
            &[JValue { l: key }, JValue { j: at }],
        )?;
    }
    // FLAG_IMMUTABLE | FLAG_UPDATE_CURRENT.
    let flags = 0x0400_0000 | 0x0800_0000;
    jni.call_static_object(
        c"android/app/PendingIntent",
        c"getActivity",
        c"(Landroid/content/Context;ILandroid/content/Intent;I)Landroid/app/PendingIntent;",
        &[
            JValue { l: context },
            JValue { i: code },
            JValue { l: intent },
            JValue { i: flags },
        ],
    )
}
