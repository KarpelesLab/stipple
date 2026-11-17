//! The app's `FileProvider`, for files dragged out of it
//! ([`drag`](super::drag)): the [`Files`](crate::platform::dex::FILES)
//! `ContentProvider`, which the app's manifest declares (authority
//! `<package>.stipple.files`, not exported, granting URI permissions) and
//! whose class is in the APK's `classes.dex`
//! ([`receiver_dex`](crate::platform::android_receiver_dex)). The system
//! creates it when the app's process starts; its static initializer loads
//! the app's library, and its methods are the `Java_org_stipple_android_Files_*`
//! functions here, exported by the library.
//!
//! It serves only reads: `openFile` opens the file the URI names
//! ([`mobile_drag::uri_path`]) read-only and hands the descriptor over
//! (`ParcelFileDescriptor.adoptFd`); `query` answers `OpenableColumns`
//! (the display name and size), `getType` the type from the extension.
//! Other apps can read only the URIs a drag granted them
//! (`DRAG_FLAG_GLOBAL_URI_READ`).

use std::os::fd::IntoRawFd;
use std::path::PathBuf;

use super::jni::{self, Env, JObject, JValue, Jni};
use super::log;
use crate::platform::backend::mobile_drag;

/// `OpenableColumns.DISPLAY_NAME` and `SIZE`.
const DISPLAY_NAME: &str = "_display_name";
const SIZE: &str = "_size";

/// The file `uri` (a `content:` URI of the provider) names.
fn path_of(jni: &Jni, uri: JObject) -> Option<PathBuf> {
    if uri.is_null() {
        return None;
    }
    let decoded = jni
        .call_object(uri, c"getPath", c"()Ljava/lang/String;", &[])
        .and_then(|s| jni.to_rust16(s))?;
    mobile_drag::uri_path(&decoded)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_org_stipple_android_Files_onCreate(env: Env, _this: JObject) -> u8 {
    // At the process's start, on the main thread: for platform::android.
    super::java::entered(env);
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_org_stipple_android_Files_getType(
    env: Env,
    _this: JObject,
    uri: JObject,
) -> JObject {
    unsafe {
        jni::in_native_object(env, |jni| {
            let path = path_of(jni, uri)?;
            jni.string(mobile_drag::mime_for_name(&mobile_drag::display_name(
                &path,
            )))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_org_stipple_android_Files_openFile(
    env: Env,
    _this: JObject,
    uri: JObject,
    mode: JObject,
) -> JObject {
    unsafe {
        jni::in_native_object(env, |jni| {
            let path = path_of(jni, uri)?;
            let mode = jni.to_rust16(mode).unwrap_or_default();
            if mode != "r" {
                log(&format!(
                    "stipple: the file provider is read-only (asked {mode:?})"
                ));
                return None;
            }
            let file = std::fs::File::open(&path)
                .map_err(|e| log(&format!("stipple: can't open {}: {e}", path.display())))
                .ok()?;
            log(&format!(
                "stipple: file provider opened {:?}",
                mobile_drag::display_name(&path)
            ));
            jni.call_static_object(
                c"android/os/ParcelFileDescriptor",
                c"adoptFd",
                c"(I)Landroid/os/ParcelFileDescriptor;",
                &[JValue {
                    i: file.into_raw_fd(),
                }],
            )
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_org_stipple_android_Files_query(
    env: Env,
    _this: JObject,
    uri: JObject,
    projection: JObject,
    _selection: JObject,
    _args: JObject,
    _order: JObject,
) -> JObject {
    unsafe {
        jni::in_native_object(env, |jni| {
            let path = path_of(jni, uri)?;
            let size = std::fs::metadata(&path).ok()?.len();
            let name = mobile_drag::display_name(&path);
            // The columns asked for that it knows, or both.
            let asked: Vec<String> = jni
                .object_vec(projection)
                .into_iter()
                .filter_map(|s| jni.to_rust16(s))
                .filter(|c| c == DISPLAY_NAME || c == SIZE)
                .collect();
            let columns: Vec<&str> = if asked.is_empty() {
                vec![DISPLAY_NAME, SIZE]
            } else {
                asked.iter().map(String::as_str).collect()
            };
            let names = jni.string_array(&columns)?;
            let cursor = jni.new_named(
                c"android/database/MatrixCursor",
                c"([Ljava/lang/String;I)V",
                &[JValue { l: names }, JValue { i: 1 }],
            )?;
            let mut row = Vec::new();
            for column in &columns {
                row.push(if *column == DISPLAY_NAME {
                    jni.string16(&name)?
                } else {
                    jni.call_static_object(
                        c"java/lang/Long",
                        c"valueOf",
                        c"(J)Ljava/lang/Long;",
                        &[JValue { j: size as i64 }],
                    )?
                });
            }
            let object = jni.find_class(c"java/lang/Object")?;
            let row = jni.object_array(object, &row)?;
            jni.call_void(
                cursor,
                c"addRow",
                c"([Ljava/lang/Object;)V",
                &[JValue { l: row }],
            )?;
            Some(cursor)
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_org_stipple_android_Files_insert(
    _env: Env,
    _this: JObject,
    _uri: JObject,
    _values: JObject,
) -> JObject {
    std::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_org_stipple_android_Files_delete(
    _env: Env,
    _this: JObject,
    _uri: JObject,
    _selection: JObject,
    _args: JObject,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn Java_org_stipple_android_Files_update(
    _env: Env,
    _this: JObject,
    _uri: JObject,
    _values: JObject,
    _selection: JObject,
    _args: JObject,
) -> i32 {
    0
}

/// The `content:` URI (a `android.net.Uri`) other apps read `path` through,
/// under the app's package `package`.
pub(super) fn uri_for(jni: &Jni, package: &str, path: &std::path::Path) -> Option<JObject> {
    let text = mobile_drag::content_uri(&mobile_drag::authority(package), path)?;
    let text = jni.string(&text)?;
    jni.call_static_object(
        c"android/net/Uri",
        c"parse",
        c"(Ljava/lang/String;)Landroid/net/Uri;",
        &[JValue { l: text }],
    )
}
