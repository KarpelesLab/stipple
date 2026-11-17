//! Scheduled notifications, for
//! [`platform::schedule_notification`](crate::platform::schedule_notification):
//! an `AlarmManager` alarm per id, whose `PendingIntent` broadcasts the
//! notification's id, title and body to a receiver that posts it
//! ([`notify::post_scheduled`]) when it goes off.
//!
//! Two receivers can hear it:
//!
//! - the [`Receiver`](crate::platform::dex::RECEIVER) class of the run-time
//!   dex, registered with the application context (not exported) the first
//!   time it's needed: it hears the alarms while the app's process lives,
//!   even with the activity gone;
//! - the [`Alarm`](crate::platform::dex::ALARM) class, when the app's
//!   manifest declares it and its APK carries the dex
//!   [`receiver_dex`](crate::platform::android_receiver_dex) writes: the
//!   system starts the app's process for it, its static initializer loads the
//!   app's library, and its `onReceive` is
//!   [`Java_org_stipple_android_Alarm_onReceive`], exported by the library.
//!   It also hears `BOOT_COMPLETED`, to set the alarms again after a reboot.
//!
//! When the manifest declares `Alarm`, the alarms name it (an explicit
//! intent, which the run-time receiver doesn't hear); otherwise they go to
//! the package, for the run-time receiver.
//!
//! `AlarmManager` can't list its alarms, so the backend keeps them in
//! `stipple-notifications.txt` in the app's files directory: for
//! [`pending_scheduled`], to set them again when the app starts (alarms don't
//! survive a reboot), and to drop a one-off once it's shown.
//!
//! The same receiver hears the share sheet's chooser picking a target (see
//! [`share`](super::share)).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use super::jni::{self, Env, JObject, JValue, Jni};
use super::{classes, log, notify, with_activity};
use crate::platform::PlatformError;
use crate::platform::event::Event;
use crate::platform::shell::ScheduledNotification;
use crate::platform::shell::schedule::{Entry, decode_all, encode_all, now_ms, upsert};

/// The alarms' broadcast action, and their data URIs' scheme (one URI per id,
/// so each id has a `PendingIntent` of its own).
const ALARM_ACTION: &str = "org.stipple.ALARM";
const SCHEME: &str = "stipple-notification";
/// The chooser's broadcast when the user picks a share target.
pub(super) const SHARED_ACTION: &str = "org.stipple.SHARED";
pub(super) const SHARE_EXTRA: &str = "org.stipple.share";
const ID_EXTRA: &str = "org.stipple.id";
const TITLE_EXTRA: &str = "org.stipple.title";
const BODY_EXTRA: &str = "org.stipple.body";
const ALARM_CLASS: &str = "org.stipple.android.Alarm";
pub(super) const TAPPED_CLASS: &str = "org.stipple.android.Tapped";
const STORE: &str = "stipple-notifications.txt";

/// `PendingIntent.FLAG_IMMUTABLE | FLAG_UPDATE_CURRENT`.
pub(super) const PENDING_FLAGS: i32 = 0x0400_0000 | 0x0800_0000;

/// The store, read and written under this lock.
static STORE_LOCK: Mutex<()> = Mutex::new(());

fn os(msg: &str) -> PlatformError {
    PlatformError::Os(msg.into())
}

/// `Build.VERSION.SDK_INT`.
pub(super) fn sdk(jni: &Jni) -> i32 {
    jni.static_int(c"android/os/Build$VERSION", c"SDK_INT")
        .unwrap_or(24)
}

/// Run `f` with a JNI env (attached here if need be) and the activity.
fn with_context<R>(f: impl FnOnce(&Jni, JObject, i32) -> Option<R>) -> Option<R> {
    with_activity(|a| {
        let (vm, context, sdk) = (a.vm, a.clazz, a.sdk_version);
        unsafe { jni::with_env(vm, |jni| f(jni, context, sdk)) }
    })
}

pub(crate) fn schedule(n: &ScheduledNotification) -> Result<(), PlatformError> {
    let entry = Entry::new(n, now_ms());
    with_context(|jni, context, sdk| {
        let mut entries = load(jni, context);
        upsert(&mut entries, entry.clone());
        save(jni, context, &entries);
        Some(arm(jni, context, sdk, &entry))
    })
    .ok_or_else(|| os("no activity"))?
    .ok_or_else(|| os("AlarmManager refused the alarm"))
}

pub(crate) fn cancel_scheduled(id: Option<&str>) {
    with_context(|jni, context, _sdk| {
        let mut entries = load(jni, context);
        let (gone, kept): (Vec<Entry>, Vec<Entry>) = entries
            .drain(..)
            .partition(|e| id.is_none_or(|id| e.id == id));
        save(jni, context, &kept);
        let mut ids: Vec<String> = gone.into_iter().map(|e| e.id).collect();
        if let Some(id) = id
            && !ids.iter().any(|x| x == id)
        {
            // Not in the store (scheduled before it was cleared): cancel it
            // all the same.
            ids.push(id.to_string());
        }
        for id in &ids {
            disarm(jni, context, id);
        }
        let manager = notify::manager(jni, context)?;
        match id {
            Some(id) => jni.call_void(
                manager,
                c"cancel",
                c"(I)V",
                &[JValue {
                    i: notify::scheduled_id(id),
                }],
            ),
            None => jni.call_void(manager, c"cancelAll", c"()V", &[]),
        }
    });
}

pub(crate) fn pending_scheduled(done: Box<dyn FnOnce(Vec<String>) + Send>) {
    let now = now_ms();
    let ids = with_context(|jni, context, _sdk| {
        Some(
            load(jni, context)
                .into_iter()
                .filter(|e| e.pending(now))
                .map(|e| e.id)
                .collect::<Vec<_>>(),
        )
    })
    .unwrap_or_default();
    done(ids);
}

// ---- The store ------------------------------------------------------------------

fn store_path(jni: &Jni, context: JObject) -> Option<String> {
    let dir = jni.call_object(context, c"getFilesDir", c"()Ljava/io/File;", &[])?;
    let path = jni.call_object(dir, c"getAbsolutePath", c"()Ljava/lang/String;", &[])?;
    Some(format!("{}/{STORE}", jni.to_rust16(path)?))
}

fn load(jni: &Jni, context: JObject) -> Vec<Entry> {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    store_path(jni, context)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| decode_all(&s))
        .unwrap_or_default()
}

fn save(jni: &Jni, context: JObject, entries: &[Entry]) {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(path) = store_path(jni, context) {
        let written = if entries.is_empty() {
            std::fs::remove_file(&path).or_else(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(e)
                }
            })
        } else {
            let tmp = format!("{path}.tmp");
            std::fs::write(&tmp, encode_all(entries)).and_then(|()| std::fs::rename(&tmp, &path))
        };
        if let Err(e) = written {
            log(&format!(
                "stipple: can't keep the scheduled notifications: {e}"
            ));
        }
    }
}

// ---- Alarms ---------------------------------------------------------------------

/// Whether the app's manifest declares the [`ALARM_CLASS`] receiver: 0 not
/// known yet, 1 no, 2 yes.
static DECLARED: AtomicU8 = AtomicU8::new(0);

fn alarm_declared(jni: &Jni, context: JObject) -> bool {
    declared(jni, context, &DECLARED, ALARM_CLASS, c"getReceiverInfo")
}

/// Whether the app's manifest declares the [`TAPPED_CLASS`] activity: 0 not
/// known yet, 1 no, 2 yes.
static TAPPED_DECLARED: AtomicU8 = AtomicU8::new(0);

/// Whether scheduled notifications' taps go to the
/// [`Tapped`](crate::platform::dex::TAPPED) activity, which the app's
/// manifest declares.
pub(super) fn tapped_declared(jni: &Jni, context: JObject) -> bool {
    declared(
        jni,
        context,
        &TAPPED_DECLARED,
        TAPPED_CLASS,
        c"getActivityInfo",
    )
}

/// Whether the app's manifest declares component `class`, asking the
/// `PackageManager`'s `getter` (`getReceiverInfo`, `getActivityInfo`) once
/// per process and keeping the answer in `cache`.
fn declared(
    jni: &Jni,
    context: JObject,
    cache: &AtomicU8,
    class: &str,
    getter: &std::ffi::CStr,
) -> bool {
    match cache.load(Ordering::Relaxed) {
        1 => return false,
        2 => return true,
        _ => {}
    }
    let declared = (|| {
        let pm = jni.call_object(
            context,
            c"getPackageManager",
            c"()Landroid/content/pm/PackageManager;",
            &[],
        )?;
        let name = jni.string(class)?;
        let component = jni.new_named(
            c"android/content/ComponentName",
            c"(Landroid/content/Context;Ljava/lang/String;)V",
            &[JValue { l: context }, JValue { l: name }],
        )?;
        // Throws NameNotFoundException (cleared, so `None`) when it isn't.
        jni.call_object(
            pm,
            getter,
            c"(Landroid/content/ComponentName;I)Landroid/content/pm/ActivityInfo;",
            &[JValue { l: component }, JValue { i: 0 }],
        )
    })()
    .is_some();
    cache.store(if declared { 2 } else { 1 }, Ordering::Relaxed);
    let what = match (class, declared) {
        (ALARM_CLASS, true) => "declared: scheduled notifications show without the app running",
        (ALARM_CLASS, false) => {
            "not declared: scheduled notifications show while the process lives"
        }
        (_, true) => "declared: a tap is heard while the activity runs",
        (_, false) => "not declared: a tap is heard only when it starts the activity",
    };
    log(&format!("stipple: {class} {what}"));
    declared
}

/// The `PendingIntent` broadcasting `id`'s alarm (with its contents, when
/// `entry` is given; cancelling only needs one that matches).
fn alarm_intent(jni: &Jni, context: JObject, id: &str, entry: Option<&Entry>) -> Option<JObject> {
    let action = jni.string(ALARM_ACTION)?;
    let intent = jni.new_named(
        c"android/content/Intent",
        c"(Ljava/lang/String;)V",
        &[JValue { l: action }],
    )?;
    if alarm_declared(jni, context) {
        let name = jni.string(ALARM_CLASS)?;
        jni.call_object(
            intent,
            c"setClassName",
            c"(Landroid/content/Context;Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue { l: context }, JValue { l: name }],
        )?;
    } else {
        ensure_receiver(jni, context);
        let package = jni.call_object(context, c"getPackageName", c"()Ljava/lang/String;", &[])?;
        jni.call_object(
            intent,
            c"setPackage",
            c"(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue { l: package }],
        )?;
    }
    let uri = jni.string16(&format!("{SCHEME}:{}", percent_encode(id)))?;
    let uri = jni.call_static_object(
        c"android/net/Uri",
        c"parse",
        c"(Ljava/lang/String;)Landroid/net/Uri;",
        &[JValue { l: uri }],
    )?;
    jni.call_object(
        intent,
        c"setData",
        c"(Landroid/net/Uri;)Landroid/content/Intent;",
        &[JValue { l: uri }],
    )?;
    if let Some(e) = entry {
        for (key, value) in [
            (ID_EXTRA, &e.id),
            (TITLE_EXTRA, &e.title),
            (BODY_EXTRA, &e.body),
        ] {
            let (key, value) = (jni.string(key)?, jni.string16(value)?);
            jni.call_object(
                intent,
                c"putExtra",
                c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
                &[JValue { l: key }, JValue { l: value }],
            )?;
        }
    }
    jni.call_static_object(
        c"android/app/PendingIntent",
        c"getBroadcast",
        c"(Landroid/content/Context;ILandroid/content/Intent;I)Landroid/app/PendingIntent;",
        &[
            JValue { l: context },
            JValue {
                i: crate::platform::shell::schedule::int_id(id),
            },
            JValue { l: intent },
            JValue { i: PENDING_FLAGS },
        ],
    )
}

fn alarm_manager(jni: &Jni, context: JObject) -> Option<JObject> {
    let name = jni.string("alarm")?;
    jni.call_object(
        context,
        c"getSystemService",
        c"(Ljava/lang/String;)Ljava/lang/Object;",
        &[JValue { l: name }],
    )
}

/// Set `entry`'s alarm: exact where the app may (Android 6 to 11, or with
/// the exact-alarm permission), else inexact; repeating with
/// `setRepeating` (inexact since Android 4.4).
fn arm(jni: &Jni, context: JObject, sdk: i32, entry: &Entry) -> Option<()> {
    let manager = alarm_manager(jni, context)?;
    let pi = alarm_intent(jni, context, &entry.id, Some(entry))?;
    // AlarmManager.RTC_WAKEUP.
    let rtc_wakeup = JValue { i: 0 };
    let due = JValue {
        j: entry.due_ms as i64,
    };
    const SET: &std::ffi::CStr = c"(IJLandroid/app/PendingIntent;)V";
    let armed = if entry.repeat_ms > 0 {
        jni.call_void(
            manager,
            c"setRepeating",
            c"(IJJLandroid/app/PendingIntent;)V",
            &[
                rtc_wakeup,
                due,
                JValue {
                    j: entry.repeat_ms as i64,
                },
                JValue { l: pi },
            ],
        )
    } else {
        let exact = sdk < 31
            || jni.call_bool(manager, c"canScheduleExactAlarms", c"()Z", &[]) == Some(true);
        let method = match (exact, sdk >= 23) {
            (true, true) => c"setExactAndAllowWhileIdle",
            (true, false) => c"setExact",
            (false, true) => c"setAndAllowWhileIdle",
            (false, false) => c"set",
        };
        jni.call_void(manager, method, SET, &[rtc_wakeup, due, JValue { l: pi }])
    };
    if armed.is_some() {
        log(&format!(
            "stipple: notification {:?} scheduled in {} ms",
            entry.id,
            entry.due_ms as i64 - now_ms() as i64
        ));
    }
    armed
}

fn disarm(jni: &Jni, context: JObject, id: &str) {
    let (Some(manager), Some(pi)) = (
        alarm_manager(jni, context),
        alarm_intent(jni, context, id, None),
    ) else {
        return;
    };
    jni.call_void(
        manager,
        c"cancel",
        c"(Landroid/app/PendingIntent;)V",
        &[JValue { l: pi }],
    );
    jni.call_void(pi, c"cancel", c"()V", &[]);
}

/// Set again the alarms of the notifications still to show, dropping the
/// ones whose time passed: when the app starts (alarms don't survive a
/// reboot) and at boot.
fn rearm(jni: &Jni, context: JObject, sdk: i32) {
    let now = now_ms();
    let entries = load(jni, context);
    let (keep, gone): (Vec<Entry>, Vec<Entry>) = entries.into_iter().partition(|e| e.pending(now));
    if !gone.is_empty() {
        save(jni, context, &keep);
    }
    for mut e in keep {
        // A repeating one whose first time passed starts again from now.
        while e.repeat_ms > 0 && e.due_ms <= now {
            e.due_ms += e.repeat_ms;
        }
        arm(jni, context, sdk, &e);
    }
}

/// Register the run-time [`Receiver`](crate::platform::dex::RECEIVER) with
/// the application context, once per process: one for the alarms (whose
/// intents carry a `stipple-notification:` URI) and one for the chooser.
pub(super) fn ensure_receiver(jni: &Jni, context: JObject) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.load(Ordering::Acquire) {
        return;
    }
    let registered = (|| {
        let class = classes::receiver_class()?;
        let app = jni.call_object(
            context,
            c"getApplicationContext",
            c"()Landroid/content/Context;",
            &[],
        )?;
        let sdk = sdk(jni);
        for (action, scheme) in [(ALARM_ACTION, Some(SCHEME)), (SHARED_ACTION, None)] {
            let receiver = jni.new_object(class, c"()V", &[])?;
            let a = jni.string(action)?;
            let filter = jni.new_named(
                c"android/content/IntentFilter",
                c"(Ljava/lang/String;)V",
                &[JValue { l: a }],
            )?;
            if let Some(scheme) = scheme {
                let s = jni.string(scheme)?;
                jni.call_void(
                    filter,
                    c"addDataScheme",
                    c"(Ljava/lang/String;)V",
                    &[JValue { l: s }],
                )?;
            }
            // Kept for the process: the registration holds the receiver.
            if sdk >= 33 {
                // Context.RECEIVER_NOT_EXPORTED.
                jni.call_object(
                    app,
                    c"registerReceiver",
                    c"(Landroid/content/BroadcastReceiver;Landroid/content/IntentFilter;I)Landroid/content/Intent;",
                    &[JValue { l: receiver }, JValue { l: filter }, JValue { i: 4 }],
                );
            } else {
                jni.call_object(
                    app,
                    c"registerReceiver",
                    c"(Landroid/content/BroadcastReceiver;Landroid/content/IntentFilter;)Landroid/content/Intent;",
                    &[JValue { l: receiver }, JValue { l: filter }],
                );
            }
        }
        Some(())
    })();
    if registered.is_some() {
        DONE.store(true, Ordering::Release);
        log("stipple: broadcast receiver registered");
    } else {
        log("stipple: can't register the broadcast receiver");
    }
}

// ---- The receivers ------------------------------------------------------------

/// `Receiver.onReceive`, the run-time receiver. On the main thread.
pub(super) unsafe extern "C" fn on_receive(
    env: Env,
    _this: JObject,
    context: JObject,
    intent: JObject,
) {
    unsafe {
        jni::in_native(env, |jni| {
            received(jni, context, intent);
            Some(())
        });
    }
}

/// `org.stipple.android.Alarm.onReceive`, the receiver an app's manifest
/// declares (with the dex [`receiver_dex`](crate::platform::android_receiver_dex)
/// writes in its APK): JNI finds it by this name once the class's static
/// initializer loaded the library. It may run in a process the activity
/// never started in.
///
/// # Safety
/// Called by the VM, with a valid `env` and references.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_org_stipple_android_Alarm_onReceive(
    env: Env,
    _this: JObject,
    context: JObject,
    intent: JObject,
) {
    super::java::entered(env);
    unsafe {
        jni::in_native(env, |jni| {
            received(jni, context, intent);
            Some(())
        });
    }
}

fn received(jni: &Jni, context: JObject, intent: JObject) -> Option<()> {
    let action = jni.call_object(intent, c"getAction", c"()Ljava/lang/String;", &[]);
    let action = action.and_then(|a| jni.to_rust16(a)).unwrap_or_default();
    let extra = |key: &str| -> Option<String> {
        let k = jni.string(key)?;
        let v = jni.call_object(
            intent,
            c"getStringExtra",
            c"(Ljava/lang/String;)Ljava/lang/String;",
            &[JValue { l: k }],
        )?;
        jni.to_rust16(v)
    };
    match action.as_str() {
        ALARM_ACTION => {
            let id = extra(ID_EXTRA)?;
            let n = crate::platform::Notification::new(extra(TITLE_EXTRA).unwrap_or_default())
                .body(extra(BODY_EXTRA).unwrap_or_default());
            let sdk = sdk(jni);
            match notify::post_scheduled(jni, context, sdk, &id, &n) {
                Ok(()) => log(&format!("stipple: scheduled notification {id:?} posted")),
                Err(e) => log(&format!("stipple: scheduled notification {id:?}: {e}")),
            }
            // A one-off is done; a repeating one stays.
            let now = now_ms();
            let mut entries = load(jni, context);
            let before = entries.len();
            entries.retain(|e| e.id != id || e.repeat_ms > 0 || e.due_ms > now + 60_000);
            if entries.len() != before {
                save(jni, context, &entries);
            }
        }
        SHARED_ACTION => {
            let k = jni.string(SHARE_EXTRA)?;
            let share = jni.call_int(
                intent,
                c"getIntExtra",
                c"(Ljava/lang/String;I)I",
                &[JValue { l: k }, JValue { i: 0 }],
            )?;
            super::share::chosen(share as u32);
        }
        "android.intent.action.BOOT_COMPLETED"
        | "android.intent.action.LOCKED_BOOT_COMPLETED"
        | "android.intent.action.MY_PACKAGE_REPLACED" => {
            log("stipple: setting the scheduled notifications again");
            rearm(jni, context, sdk(jni));
        }
        _ => {}
    }
    Some(())
}

// ---- The activity starting ----------------------------------------------------

/// The `at` extras of the taps already reported, so a recreated activity
/// (same launch intent) doesn't report its tap again.
static REPORTED: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// The activity was created: set the alarms again, and if a notification's
/// tap started it, tell the app which. From `onCreate`, on the main thread.
pub(super) fn activity_created(jni: &Jni, activity: JObject, sdk: i32) {
    rearm(jni, activity, sdk);
    report_tap(jni, activity);
}

/// If `activity`'s intent is a scheduled notification's tap not reported
/// yet, tell the app which.
fn report_tap(jni: &Jni, activity: JObject) {
    let tapped = (|| {
        let intent = jni.call_object(activity, c"getIntent", c"()Landroid/content/Intent;", &[])?;
        // FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY: brought back from Recents,
        // with the intent it was first started with.
        let flags = jni.call_int(intent, c"getFlags", c"()I", &[])?;
        if flags & 0x0010_0000 != 0 {
            return None;
        }
        let key = jni.string(notify::TAPPED_EXTRA)?;
        let id = jni.call_object(
            intent,
            c"getStringExtra",
            c"(Ljava/lang/String;)Ljava/lang/String;",
            &[JValue { l: key }],
        )?;
        let id = jni.to_rust16(id)?;
        let key = jni.string(notify::TAPPED_AT_EXTRA)?;
        let at = jni.call_long(
            intent,
            c"getLongExtra",
            c"(Ljava/lang/String;J)J",
            &[JValue { l: key }, JValue { j: 0 }],
        )?;
        let mut reported = REPORTED.lock().unwrap_or_else(|e| e.into_inner());
        if reported.contains(&at) {
            return None;
        }
        reported.push(at);
        Some(id)
    })();
    if let Some(id) = tapped {
        log(&format!("stipple: notification {id:?} tapped"));
        // Queued for the event loop: the running one, or the one the app's
        // activity starts.
        crate::platform::shell::post(Event::NotificationTapped(id));
    }
}

/// `org.stipple.android.Tapped.onCreate`, the windowless activity a
/// scheduled notification's tap opens when the app's manifest declares it
/// (with the dex [`receiver_dex`](crate::platform::android_receiver_dex)
/// writes in its APK). A `NativeActivity` doesn't pass `onNewIntent` on, so
/// a tap that only brings the running activity forward would go unheard;
/// this activity reports the tap whatever runs, brings the app's activity
/// forward with its launch intent (or starts it, the app's event loop then
/// finding the tap queued), and finishes.
///
/// # Safety
/// Called by the VM, with a valid `env` and references.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_org_stipple_android_Tapped_onCreate(
    env: Env,
    this: JObject,
    saved: JObject,
) {
    super::java::entered(env);
    unsafe {
        jni::in_native(env, |jni| {
            // No window, whatever the manifest says: it finishes at once.
            if let Some(theme) = jni.static_int(c"android/R$style", c"Theme_NoDisplay") {
                jni.call_void(this, c"setTheme", c"(I)V", &[JValue { i: theme }]);
            }
            jni.call_super_void(
                this,
                c"android/app/Activity",
                c"onCreate",
                c"(Landroid/os/Bundle;)V",
                &[JValue { l: saved }],
            );
            report_tap(jni, this);
            let opened = (|| {
                let pm = jni.call_object(
                    this,
                    c"getPackageManager",
                    c"()Landroid/content/pm/PackageManager;",
                    &[],
                )?;
                let package =
                    jni.call_object(this, c"getPackageName", c"()Ljava/lang/String;", &[])?;
                // With FLAG_ACTIVITY_NEW_TASK, as the launcher starts it: it
                // brings the app's task forward, or starts it. Plus
                // FLAG_ACTIVITY_REORDER_TO_FRONT: a running activity comes
                // forward, even when its task was started some other way (no
                // second activity on top).
                let intent = jni.call_object(
                    pm,
                    c"getLaunchIntentForPackage",
                    c"(Ljava/lang/String;)Landroid/content/Intent;",
                    &[JValue { l: package }],
                )?;
                jni.call_object(
                    intent,
                    c"addFlags",
                    c"(I)Landroid/content/Intent;",
                    &[JValue { i: 0x0002_0000 }],
                )?;
                jni.call_void(
                    this,
                    c"startActivity",
                    c"(Landroid/content/Intent;)V",
                    &[JValue { l: intent }],
                )
            })();
            if opened.is_none() {
                log("stipple: can't open the app from the notification");
            }
            jni.call_void(this, c"finish", c"()V", &[]);
            // No animation of its own on the way out.
            jni.call_void(
                this,
                c"overridePendingTransition",
                c"(II)V",
                &[JValue { i: 0 }, JValue { i: 0 }],
            );
            Some(())
        });
    }
}

/// `id` with everything but unreserved characters percent-encoded, for a URI.
fn percent_encode(id: &str) -> String {
    let mut out = String::new();
    for b in id.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
