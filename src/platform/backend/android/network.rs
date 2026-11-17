//! The default network on Android: a `ConnectivityManager.NetworkCallback`
//! registered with `registerDefaultNetworkCallback` (API 24), and the
//! default network's `NetworkCapabilities`.
//!
//! The callback is a class of its own, assembled by
//! [`dex`](crate::platform::dex) and loaded apart from the backend's other
//! classes (it's only needed by an app that watches the network): a
//! subclass of the abstract `NetworkCallback` whose `onAvailable`, `onLost`,
//! `onCapabilitiesChanged` and `onBlockedStatusChanged` are native. They run
//! on the system's connectivity thread, which reads the capabilities into a
//! [`Reading`](crate::platform::network) and reports it: the transport, the
//! Internet capability for online, `NOT_METERED` (or `TEMPORARILY_NOT_METERED`)
//! against metered, `getRestrictBackgroundStatus()` for Data Saver, and the
//! network's handle for its identity.
//!
//! Registering needs `android.permission.ACCESS_NETWORK_STATE` in the
//! manifest (`network = true` in `cargo-stipple`'s `[android]` table).

use std::ffi::{CString, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::jni::{self, Env, JObject, JValue, Jni};
use super::{ANativeActivity, log, with_activity};
use crate::platform::dex;
use crate::platform::network::android_caps::{self as caps, Caps};

const NETWORK: &str = "Landroid/net/Network;";

/// The callback: every method but the constructor native.
const CALLBACK: dex::Class = dex::Class {
    name: "Lorg/stipple/android/NetworkWatch;",
    superclass: "Landroid/net/ConnectivityManager$NetworkCallback;",
    interfaces: &[],
    ctor: &[],
    natives: &[
        dex::Native {
            name: "onAvailable",
            params: &[NETWORK],
            ret: "V",
        },
        dex::Native {
            name: "onLost",
            params: &[NETWORK],
            ret: "V",
        },
        dex::Native {
            name: "onCapabilitiesChanged",
            params: &[NETWORK, "Landroid/net/NetworkCapabilities;"],
            ret: "V",
        },
        // API 29; never called before.
        dex::Native {
            name: "onBlockedStatusChanged",
            params: &[NETWORK, "Z"],
            ret: "V",
        },
    ],
    load_library: None,
};

/// Hears Data Saver switched on or off
/// (`ACTION_RESTRICT_BACKGROUND_CHANGED`), which no network callback does.
const POLICY: dex::Class = dex::Class {
    name: "Lorg/stipple/android/NetworkPolicy;",
    superclass: "Landroid/content/BroadcastReceiver;",
    interfaces: &[],
    ctor: &[],
    natives: &[dex::Native {
        name: "onReceive",
        params: &["Landroid/content/Context;", "Landroid/content/Intent;"],
        ret: "V",
    }],
    load_library: None,
};

/// `Context.RECEIVER_NOT_EXPORTED` (API 33): system broadcasts still come.
const RECEIVER_NOT_EXPORTED: i32 = 4;

/// The `ConnectivityManager`, a global reference (as an address; `0` until
/// registered).
static MANAGER: Mutex<usize> = Mutex::new(0);

/// Counts what the callback heard, so a loss can tell whether a new
/// network came since.
static HEARD: AtomicU64 = AtomicU64::new(0);

/// How long a lost default network waits for its replacement before the
/// loss is reported: Android reports Wi-Fi lost a moment before cellular
/// takes over, which isn't worth an "offline" flash.
const LOSS_GRACE: Duration = Duration::from_millis(1000);

/// The Rust function behind each native.
fn function(name: &str) -> Option<*const c_void> {
    Some(match name {
        "onAvailable" => on_available as *const c_void,
        "onLost" => on_lost as *const c_void,
        "onCapabilitiesChanged" => on_capabilities_changed as *const c_void,
        "onBlockedStatusChanged" => on_blocked_status_changed as *const c_void,
        "onReceive" => on_policy_changed as *const c_void,
        _ => return None,
    })
}

/// Load the callback and receiver classes and bind their natives.
fn load(jni: &Jni, activity: &ANativeActivity) -> Option<[JObject; 2]> {
    let parent = jni.call_object(
        activity.clazz,
        c"getClassLoader",
        c"()Ljava/lang/ClassLoader;",
        &[],
    )?;
    let bytes: &'static [u8] = Box::leak(dex::build(&[CALLBACK, POLICY]).into_boxed_slice());
    let loader = if activity.sdk_version >= 26 {
        let buffer = jni.direct_buffer(bytes)?;
        jni.new_named(
            c"dalvik/system/InMemoryDexClassLoader",
            c"(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V",
            &[JValue { l: buffer }, JValue { l: parent }],
        )
    } else {
        // Android 7, from a file in the app's private storage.
        let dir = unsafe { std::ffi::CStr::from_ptr(activity.internal_data_path) }
            .to_str()
            .ok()?;
        let path = format!("{dir}/stipple-network.dex");
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
        jni.failed("creating the network callback's class loader");
        return None;
    };
    let mut out = [std::ptr::null_mut(); 2];
    for (slot, class) in out.iter_mut().zip([CALLBACK, POLICY]) {
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
        *slot = loaded;
    }
    Some(out)
}

/// What `caps` (a `NetworkCapabilities`) has.
fn read_caps(jni: &Jni, caps: JObject) -> Option<Caps> {
    if caps.is_null() {
        return None;
    }
    let transport = |t| {
        jni.call_bool(caps, c"hasTransport", c"(I)Z", &[JValue { i: t }])
            .unwrap_or(false)
    };
    let capability = |c| {
        jni.call_bool(caps, c"hasCapability", c"(I)Z", &[JValue { i: c }])
            .unwrap_or(false)
    };
    Some(Caps {
        wifi: transport(caps::TRANSPORT_WIFI),
        cellular: transport(caps::TRANSPORT_CELLULAR),
        ethernet: transport(caps::TRANSPORT_ETHERNET),
        internet: capability(caps::CAPABILITY_INTERNET),
        not_metered: capability(caps::CAPABILITY_NOT_METERED),
        temporarily_not_metered: capability(caps::CAPABILITY_TEMPORARILY_NOT_METERED),
    })
}

/// Report `network` (null: none) with `caps` (null: ask the manager).
///
/// A network whose capabilities can't be read any more, or that has no
/// Internet (callbacks for the old default network can come after it's
/// gone), is skipped: a default network that's really lost comes as
/// `onLost`.
fn report(jni: &Jni, manager: JObject, network: JObject, caps: JObject) {
    use crate::platform::network::Reading;
    if network.is_null() {
        HEARD.fetch_add(1, Ordering::AcqRel);
        crate::platform::network::report(Reading::OFFLINE);
        return;
    }
    let caps = if caps.is_null() {
        jni.call_object(
            manager,
            c"getNetworkCapabilities",
            c"(Landroid/net/Network;)Landroid/net/NetworkCapabilities;",
            &[JValue { l: network }],
        )
    } else {
        Some(caps)
    };
    let Some(caps) = caps.and_then(|c| read_caps(jni, c)).filter(|c| c.internet) else {
        return;
    };
    let handle = jni
        .call_long(network, c"getNetworkHandle", c"()J", &[])
        .unwrap_or(0);
    let restrict = jni
        .call_int(manager, c"getRestrictBackgroundStatus", c"()I", &[])
        .unwrap_or(0);
    HEARD.fetch_add(1, Ordering::AcqRel);
    crate::platform::network::report(caps::reading(Some(caps), handle, restrict));
}

fn manager() -> JObject {
    *MANAGER.lock().unwrap_or_else(|e| e.into_inner()) as JObject
}

unsafe extern "C" fn on_available(env: Env, _this: JObject, network: JObject) {
    unsafe {
        jni::in_native(env, |jni| {
            report(jni, manager(), network, std::ptr::null_mut());
            Some(())
        });
    }
}

unsafe extern "C" fn on_lost(_env: Env, _this: JObject, _network: JObject) {
    // The default network is gone: offline, unless another one takes over
    // (`onAvailable`) within the grace.
    let seen = HEARD.fetch_add(1, Ordering::AcqRel) + 1;
    let spawned = std::thread::Builder::new()
        .name("stipple-network-lost".into())
        .spawn(move || {
            std::thread::sleep(LOSS_GRACE);
            if HEARD.load(Ordering::Acquire) == seen {
                crate::platform::network::report(crate::platform::network::Reading::OFFLINE);
            }
        });
    if spawned.is_err() {
        crate::platform::network::report(crate::platform::network::Reading::OFFLINE);
    }
}

unsafe extern "C" fn on_capabilities_changed(
    env: Env,
    _this: JObject,
    network: JObject,
    caps: JObject,
) {
    unsafe {
        jni::in_native(env, |jni| {
            report(jni, manager(), network, caps);
            Some(())
        });
    }
}

unsafe extern "C" fn on_policy_changed(
    env: Env,
    _this: JObject,
    _context: JObject,
    _intent: JObject,
) {
    // Data Saver turned on or off: read the default network again.
    unsafe {
        jni::in_native(env, |jni| {
            let manager = manager();
            let active = jni
                .call_object(
                    manager,
                    c"getActiveNetwork",
                    c"()Landroid/net/Network;",
                    &[],
                )
                .unwrap_or(std::ptr::null_mut());
            report(jni, manager, active, std::ptr::null_mut());
            Some(())
        });
    }
}

unsafe extern "C" fn on_blocked_status_changed(
    env: Env,
    _this: JObject,
    network: JObject,
    _blocked: u8,
) {
    // Data Saver turned on or off, among others: read it all again.
    unsafe {
        jni::in_native(env, |jni| {
            report(jni, manager(), network, std::ptr::null_mut());
            Some(())
        });
    }
}

/// Register the callback, once per process. `false` while there's no
/// activity yet (the App tries again as its loop starts).
pub(crate) fn start() -> bool {
    let started = with_activity(|activity| {
        if activity.sdk_version < 24 {
            log("stipple: the network needs Android 7 (API 24): not watched");
            return Some(());
        }
        unsafe {
            jni::with_env(activity.vm, |jni| {
                let context = jni
                    .call_object(
                        activity.clazz,
                        c"getApplicationContext",
                        c"()Landroid/content/Context;",
                        &[],
                    )
                    .unwrap_or(activity.clazz);
                let service = jni.string("connectivity")?;
                let manager = jni.call_object(
                    context,
                    c"getSystemService",
                    c"(Ljava/lang/String;)Ljava/lang/Object;",
                    &[JValue { l: service }],
                )?;
                let [class, policy] = load(jni, activity)?;
                let callback = jni.new_object(class, c"()V", &[])?;
                let manager = jni.global(manager);
                let callback = jni.global(callback);
                *MANAGER.lock().unwrap_or_else(|e| e.into_inner()) = manager as usize;
                // The status now, before the callback's first answer.
                let active = jni
                    .call_object(
                        manager,
                        c"getActiveNetwork",
                        c"()Landroid/net/Network;",
                        &[],
                    )
                    .unwrap_or(std::ptr::null_mut());
                report(jni, manager, active, std::ptr::null_mut());
                if jni
                    .call_void(
                        manager,
                        c"registerDefaultNetworkCallback",
                        c"(Landroid/net/ConnectivityManager$NetworkCallback;)V",
                        &[JValue { l: callback }],
                    )
                    .is_none()
                {
                    log("stipple: can't watch the network: does the manifest have \
                         android.permission.ACCESS_NETWORK_STATE? (network = true in \
                         cargo-stipple's [android] table)");
                } else {
                    log("stipple: network callback registered");
                }
                watch_policy(jni, activity, context, policy);
                Some(())
            })
        };
        Some(())
    });
    started.is_some()
}

/// Register the Data Saver receiver on `context`.
fn watch_policy(jni: &Jni, activity: &ANativeActivity, context: JObject, class: JObject) {
    let done = (|| {
        let receiver = jni.global(jni.new_object(class, c"()V", &[])?);
        let action = jni.string("android.net.conn.RESTRICT_BACKGROUND_CHANGED")?;
        let filter = jni.new_named(
            c"android/content/IntentFilter",
            c"(Ljava/lang/String;)V",
            &[JValue { l: action }],
        )?;
        if activity.sdk_version >= 33 {
            jni.call_object(
                context,
                c"registerReceiver",
                c"(Landroid/content/BroadcastReceiver;Landroid/content/IntentFilter;I)Landroid/content/Intent;",
                &[
                    JValue { l: receiver },
                    JValue { l: filter },
                    JValue {
                        i: RECEIVER_NOT_EXPORTED,
                    },
                ],
            );
        } else {
            jni.call_object(
                context,
                c"registerReceiver",
                c"(Landroid/content/BroadcastReceiver;Landroid/content/IntentFilter;)Landroid/content/Intent;",
                &[JValue { l: receiver }, JValue { l: filter }],
            );
        }
        // (`registerReceiver` answers null, there being no sticky intent,
        // so its answer says nothing.)
        Some(())
    })();
    if done.is_none() {
        log("stipple: can't watch Data Saver");
    }
}
