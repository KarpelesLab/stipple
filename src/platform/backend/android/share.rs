//! The share sheet, for [`platform::share`](crate::platform::share): an
//! `ACTION_SEND` intent (`ACTION_SEND_MULTIPLE` for several files) through
//! `Intent.createChooser`, started from the headless
//! [`Results`](crate::platform::dex::RESULTS) fragment so its closing comes
//! back (`onActivityResult`, request code [`SHARE_CODE`] + the request's id).
//!
//! The chooser says which target the user picked through an `IntentSender`
//! (Android 5.1+): a broadcast to the run-time receiver (see
//! [`alarm::ensure_receiver`](super::alarm)), which answers
//! [`ShareOutcome::Shared`]. The chooser closing without one answers
//! [`ShareOutcome::Cancelled`], a moment later, as the broadcast may arrive
//! after the result.
//!
//! Files must reach the target as `content:` URIs it may read. Without a
//! `FileProvider` (a Java class an APK with `hasCode="false"` can't declare)
//! they go into `MediaStore`'s Downloads (Android 10+; no permission needed
//! for the app's own files), and the URIs are granted to the target
//! (`FLAG_GRANT_READ_URI_PERMISSION`). They stay in the user's Downloads.
//! Before Android 10 files can't be shared, and a share of files only is
//! [`ShareOutcome::Unavailable`].

use super::jni::{JObject, JValue, Jni};
use super::{alarm, classes, log, run_on_main};
use crate::platform::shell::sharing::answer;
use crate::platform::shell::{Share, ShareFile, ShareOutcome};

/// The request codes of shares: this bit, plus the request's id (under
/// 0x10000). Above the document picker's and the permission request's.
pub(super) const SHARE_CODE: i32 = 0x2_0000;

pub(crate) fn share(id: u32, share: Share) -> bool {
    run_on_main(move |jni, activity| {
        if start(jni, activity, id, &share).is_none() {
            log("stipple: can't open the share sheet");
            answer(id, ShareOutcome::Unavailable.code());
        }
    })
}

fn start(jni: &Jni, activity: JObject, id: u32, share: &Share) -> Option<()> {
    let sdk = alarm::sdk(jni);
    let uris: Vec<(JObject, String)> = if sdk >= 29 {
        share
            .files
            .iter()
            .filter_map(|f| Some((media_store(jni, activity, f)?, f.mime_type())))
            .collect()
    } else {
        if !share.files.is_empty() {
            log("stipple: sharing files needs Android 10 (MediaStore)");
        }
        Vec::new()
    };
    let text = share.combined_text();
    if uris.is_empty() && text.is_empty() {
        return None;
    }
    let action = if uris.len() > 1 {
        "android.intent.action.SEND_MULTIPLE"
    } else {
        "android.intent.action.SEND"
    };
    let action = jni.string(action)?;
    let intent = jni.new_named(
        c"android/content/Intent",
        c"(Ljava/lang/String;)V",
        &[JValue { l: action }],
    )?;
    let mime = match uris.as_slice() {
        [] => "text/plain".to_string(),
        [(_, m), rest @ ..] if rest.iter().all(|(_, x)| x == m) => m.clone(),
        [(_, m), rest @ ..] => {
            let family = m.split('/').next().unwrap_or("*");
            if rest
                .iter()
                .all(|(_, x)| x.split('/').next() == Some(family))
            {
                format!("{family}/*")
            } else {
                "*/*".into()
            }
        }
    };
    let mime = jni.string(&mime)?;
    jni.call_object(
        intent,
        c"setType",
        c"(Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue { l: mime }],
    )?;
    const PUT_STRING: &std::ffi::CStr =
        c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;";
    for (key, value) in [
        ("android.intent.extra.TEXT", &text),
        ("android.intent.extra.SUBJECT", &share.subject),
        ("android.intent.extra.TITLE", &share.subject),
    ] {
        if value.is_empty() {
            continue;
        }
        let (k, v) = (jni.string(key)?, jni.string16(value)?);
        jni.call_object(
            intent,
            c"putExtra",
            PUT_STRING,
            &[JValue { l: k }, JValue { l: v }],
        )?;
    }
    let stream = jni.string("android.intent.extra.STREAM")?;
    match uris.as_slice() {
        [] => {}
        [(uri, _)] => {
            jni.call_object(
                intent,
                c"putExtra",
                c"(Ljava/lang/String;Landroid/os/Parcelable;)Landroid/content/Intent;",
                &[JValue { l: stream }, JValue { l: *uri }],
            )?;
        }
        many => {
            let list = jni.new_named(c"java/util/ArrayList", c"()V", &[])?;
            for (uri, _) in many {
                jni.call_bool(
                    list,
                    c"add",
                    c"(Ljava/lang/Object;)Z",
                    &[JValue { l: *uri }],
                )?;
            }
            jni.call_object(
                intent,
                c"putParcelableArrayListExtra",
                c"(Ljava/lang/String;Ljava/util/ArrayList;)Landroid/content/Intent;",
                &[JValue { l: stream }, JValue { l: list }],
            )?;
        }
    }
    if !uris.is_empty() {
        // FLAG_GRANT_READ_URI_PERMISSION.
        jni.call_object(
            intent,
            c"addFlags",
            c"(I)Landroid/content/Intent;",
            &[JValue { i: 1 }],
        )?;
    }
    let title = if share.subject.is_empty() {
        std::ptr::null_mut()
    } else {
        jni.string16(&share.subject)?
    };
    let chooser = match chosen_sender(jni, activity, id) {
        Some(sender) if sdk >= 22 => jni.call_static_object(
            c"android/content/Intent",
            c"createChooser",
            c"(Landroid/content/Intent;Ljava/lang/CharSequence;Landroid/content/IntentSender;)Landroid/content/Intent;",
            &[JValue { l: intent }, JValue { l: title }, JValue { l: sender }],
        ),
        _ => jni.call_static_object(
            c"android/content/Intent",
            c"createChooser",
            c"(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;",
            &[JValue { l: intent }, JValue { l: title }],
        ),
    }?;
    let fragment = classes::results_fragment(jni, activity)?;
    jni.call_void(
        fragment,
        c"startActivityForResult",
        c"(Landroid/content/Intent;I)V",
        &[
            JValue { l: chooser },
            JValue {
                i: SHARE_CODE | (id & 0xffff) as i32,
            },
        ],
    )?;
    log(&format!(
        "stipple: share chooser started (request {id}, {} files)",
        uris.len()
    ));
    Some(())
}

/// The `IntentSender` the chooser fires when the user picks a target: a
/// broadcast of [`alarm::SHARED_ACTION`] to the package, carrying `id`.
fn chosen_sender(jni: &Jni, context: JObject, id: u32) -> Option<JObject> {
    alarm::ensure_receiver(jni, context);
    let action = jni.string(alarm::SHARED_ACTION)?;
    let intent = jni.new_named(
        c"android/content/Intent",
        c"(Ljava/lang/String;)V",
        &[JValue { l: action }],
    )?;
    let package = jni.call_object(context, c"getPackageName", c"()Ljava/lang/String;", &[])?;
    jni.call_object(
        intent,
        c"setPackage",
        c"(Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue { l: package }],
    )?;
    let key = jni.string(alarm::SHARE_EXTRA)?;
    jni.call_object(
        intent,
        c"putExtra",
        c"(Ljava/lang/String;I)Landroid/content/Intent;",
        &[JValue { l: key }, JValue { i: id as i32 }],
    )?;
    let pi = jni.call_static_object(
        c"android/app/PendingIntent",
        c"getBroadcast",
        c"(Landroid/content/Context;ILandroid/content/Intent;I)Landroid/app/PendingIntent;",
        &[
            JValue { l: context },
            JValue {
                i: SHARE_CODE | (id & 0xffff) as i32,
            },
            JValue { l: intent },
            JValue {
                i: alarm::PENDING_FLAGS,
            },
        ],
    )?;
    jni.call_object(
        pi,
        c"getIntentSender",
        c"()Landroid/content/IntentSender;",
        &[],
    )
}

/// Put `file` in `MediaStore`'s Downloads and return its `content:` URI
/// (Android 10+).
pub(super) fn media_store(jni: &Jni, context: JObject, file: &ShareFile) -> Option<JObject> {
    use std::io::Write;
    use std::os::fd::FromRawFd;
    let resolver = jni.call_object(
        context,
        c"getContentResolver",
        c"()Landroid/content/ContentResolver;",
        &[],
    )?;
    let external = jni.string("external")?;
    let collection = jni.call_static_object(
        c"android/provider/MediaStore$Downloads",
        c"getContentUri",
        c"(Ljava/lang/String;)Landroid/net/Uri;",
        &[JValue { l: external }],
    )?;
    let values = jni.new_named(c"android/content/ContentValues", c"()V", &[])?;
    const PUT: &std::ffi::CStr = c"(Ljava/lang/String;Ljava/lang/String;)V";
    for (key, value) in [
        ("_display_name", file.safe_name()),
        ("mime_type", file.mime_type()),
    ] {
        let (k, v) = (jni.string(key)?, jni.string16(&value)?);
        jni.call_void(values, c"put", PUT, &[JValue { l: k }, JValue { l: v }])?;
    }
    let pending = |on: i32| -> Option<JObject> {
        let v = jni.new_named(c"android/content/ContentValues", c"()V", &[])?;
        let k = jni.string("is_pending")?;
        let boxed = jni.call_static_object(
            c"java/lang/Integer",
            c"valueOf",
            c"(I)Ljava/lang/Integer;",
            &[JValue { i: on }],
        )?;
        jni.call_void(
            v,
            c"put",
            c"(Ljava/lang/String;Ljava/lang/Integer;)V",
            &[JValue { l: k }, JValue { l: boxed }],
        )?;
        Some(v)
    };
    let first = pending(1)?;
    jni.call_void(
        values,
        c"putAll",
        c"(Landroid/content/ContentValues;)V",
        &[JValue { l: first }],
    )?;
    let uri = jni.call_object(
        resolver,
        c"insert",
        c"(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;",
        &[JValue { l: collection }, JValue { l: values }],
    )?;
    let mode = jni.string("w")?;
    let pfd = jni.call_object(
        resolver,
        c"openFileDescriptor",
        c"(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;",
        &[JValue { l: uri }, JValue { l: mode }],
    )?;
    let fd = jni.call_int(pfd, c"detachFd", c"()I", &[])?;
    if fd < 0 {
        return None;
    }
    // SAFETY: detachFd hands the descriptor over; the File closes it.
    let mut out = unsafe { std::fs::File::from_raw_fd(fd) };
    let written = out.write_all(&file.bytes).and_then(|()| out.flush());
    drop(out);
    if let Err(e) = written {
        log(&format!(
            "stipple: can't write {:?} to share: {e}",
            file.name
        ));
        return None;
    }
    let done = pending(0)?;
    let none = std::ptr::null_mut();
    jni.call_int(
        resolver,
        c"update",
        c"(Landroid/net/Uri;Landroid/content/ContentValues;Ljava/lang/String;[Ljava/lang/String;)I",
        &[
            JValue { l: uri },
            JValue { l: done },
            JValue { l: none },
            JValue { l: none },
        ],
    )?;
    Some(uri)
}

/// The chooser's `IntentSender` fired for share `id`: the user picked a
/// target. On the main thread.
pub(super) fn chosen(id: u32) {
    log(&format!("stipple: share target chosen (request {id})"));
    answer(id, ShareOutcome::Shared.code());
}

/// The chooser for request code `code` closed. If no target was picked
/// (its broadcast may still be on its way), the share was cancelled.
pub(super) fn closed(code: i32) {
    let id = (code & 0xffff) as u32;
    log(&format!("stipple: share chooser closed (request {id})"));
    let _ = std::thread::Builder::new()
        .name("stipple-share".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            answer(id, ShareOutcome::Cancelled.code());
        });
}
