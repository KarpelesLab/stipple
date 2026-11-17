//! URLs that open the app on Android, for
//! [`App::on_open_url`](crate::App::on_open_url): custom schemes and App Links
//! arrive as `ACTION_VIEW` intents carrying the URL as their data.
//!
//! A `NativeActivity` hears its launch intent, but not the intents that reach
//! it while it runs (it doesn't pass `onNewIntent` on), and an intent filter
//! on it would start a second instance of it in the browser's task. So the
//! filters go on Stipple's [`Link`](crate::platform::dex::LINK) activity
//! instead (declared by cargo-stipple for `url-schemes` and
//! `associated-domains`, with the dex from
//! [`receiver_dex`](crate::platform::android_receiver_dex) in the APK), the
//! same windowless trampoline as the notification taps'
//! [`Tapped`](crate::platform::dex::TAPPED): it hands the URL to the app,
//! whatever runs, brings the app's own activity forward (or starts it, its
//! event loop then finding the URL queued), and finishes. See
//! [`forward`](super::alarm::forward).
//!
//! An app with a hand-written manifest that puts `VIEW` filters on its
//! `NativeActivity` still has the launch URL: [`activity_created`] reads the
//! activity's own intent.

use std::sync::Mutex;

use super::jni::{Env, JObject, JValue, Jni};
use super::log;

const ACTION_VIEW: &str = "android.intent.action.VIEW";
/// `Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY`: brought back from Recents,
/// with the intent it was first started with.
const LAUNCHED_FROM_HISTORY: i32 = 0x0010_0000;

/// The `System.identityHashCode` of the app activity's intents already
/// reported, so a recreated activity (same launch intent) doesn't report its
/// URL again.
static REPORTED: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// The URL `activity`'s intent carries, if it's an `ACTION_VIEW` with data
/// not brought back from Recents; with the intent.
fn view_url(jni: &Jni, activity: JObject) -> Option<(JObject, String)> {
    let intent = jni.call_object(activity, c"getIntent", c"()Landroid/content/Intent;", &[])?;
    if jni.call_int(intent, c"getFlags", c"()I", &[])? & LAUNCHED_FROM_HISTORY != 0 {
        return None;
    }
    let action = jni.call_object(intent, c"getAction", c"()Ljava/lang/String;", &[])?;
    if jni.to_rust16(action)? != ACTION_VIEW {
        return None;
    }
    let data = jni.call_object(intent, c"getDataString", c"()Ljava/lang/String;", &[])?;
    Some((intent, jni.to_rust16(data)?))
}

/// The `Link` activity's report: its intent's URL, to the app.
fn report_link(jni: &Jni, activity: JObject) {
    match view_url(jni, activity) {
        Some((_, url)) => {
            log(&format!("stipple: opened with {url:?}"));
            crate::platform::incoming_url::deliver(url);
        }
        None => log("stipple: the link activity started without a URL"),
    }
}

/// The app's activity was created: if a `VIEW` intent started it (a manifest
/// with the filters on the `NativeActivity` itself), hand its URL to the app,
/// once. From `onCreate`, on the main thread.
pub(super) fn activity_created(jni: &Jni, activity: JObject) {
    let Some((intent, url)) = view_url(jni, activity) else {
        return;
    };
    let id = jni
        .call_static_int(
            c"java/lang/System",
            c"identityHashCode",
            c"(Ljava/lang/Object;)I",
            &[JValue { l: intent }],
        )
        .unwrap_or(0);
    let mut reported = REPORTED.lock().unwrap_or_else(|e| e.into_inner());
    if reported.contains(&id) {
        return;
    }
    reported.push(id);
    drop(reported);
    log(&format!("stipple: launched with {url:?}"));
    crate::platform::incoming_url::deliver(url);
}

/// `org.stipple.android.Link.onCreate`, the windowless activity whose
/// `VIEW` intent filters claim the app's URL schemes and App Link domains.
///
/// # Safety
/// Called by the VM, with a valid `env` and references.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_org_stipple_android_Link_onCreate(
    env: Env,
    this: JObject,
    saved: JObject,
) {
    unsafe { super::alarm::forward(env, this, saved, report_link, "the link") }
}
