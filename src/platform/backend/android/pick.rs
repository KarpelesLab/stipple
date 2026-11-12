//! The Storage Access Framework's document picker, for
//! [`dialog::open_file_contents`](crate::platform::dialog::open_file_contents)
//! and [`dialog::save_file_contents`](crate::platform::dialog::save_file_contents):
//! `ACTION_OPEN_DOCUMENT` to pick a document to read, `ACTION_CREATE_DOCUMENT`
//! to make one to write.
//!
//! A `NativeActivity` doesn't pass on `onActivityResult`, so the picker is
//! started from the headless [`Results`](crate::platform::dex::RESULTS)
//! fragment (see [`classes::results_fragment`]), whose `onActivityResult`
//! comes here, on the main thread. The document's `content:` URI is then
//! opened through the `ContentResolver` on a thread of its own (a cloud
//! provider may download it first), as a file descriptor
//! (`openFileDescriptor`, then `detachFd`) that Rust reads or writes, and the
//! outcome goes to the UI thread as a [`Command::Picked`], where the app's
//! callback runs. The request code is the dialog's request id.

use std::ffi::c_void;
use std::sync::Mutex;

use super::jni::{self, Env, JObject, JValue, Jni};
use super::{Command, classes, log, post_command, run_on_main};
use crate::platform::dialog::{FileContents, mobile};

/// The picker's answer, for the UI thread.
#[derive(Clone, Debug)]
pub(crate) enum Outcome {
    Opened(Option<FileContents>),
    Saved(bool),
}

/// What a request waits to do with the document it gets.
enum Waiting {
    Open,
    Save(Vec<u8>),
}

impl Waiting {
    fn failed(&self) -> Outcome {
        match self {
            Waiting::Open => Outcome::Opened(None),
            Waiting::Save(_) => Outcome::Saved(false),
        }
    }
}

/// The requests started and not yet answered, by request code.
static WAITING: Mutex<Vec<(i32, Waiting)>> = Mutex::new(Vec::new());

/// `Activity.RESULT_OK`.
const RESULT_OK: i32 = -1;

/// Let the user pick a document to read, offering `mimes` (any if empty),
/// for dialog request `id`. `false` if there's no activity to ask from.
pub(crate) fn open_document(id: u32, mimes: Vec<String>) -> bool {
    run_on_main(move |jni, activity| {
        let started = (|| {
            let intent = intent(jni, "android.intent.action.OPEN_DOCUMENT")?;
            let ty = match mimes.as_slice() {
                [one] => one.as_str(),
                _ => "*/*",
            };
            set_type(jni, intent, ty)?;
            if mimes.len() > 1 {
                let list: Vec<&str> = mimes.iter().map(String::as_str).collect();
                let key = jni.string("android.intent.extra.MIME_TYPES")?;
                let array = jni.string_array(&list)?;
                jni.call_object(
                    intent,
                    c"putExtra",
                    c"(Ljava/lang/String;[Ljava/lang/String;)Landroid/content/Intent;",
                    &[JValue { l: key }, JValue { l: array }],
                )?;
            }
            start(jni, activity, intent, id, Waiting::Open)
        })();
        if started.is_none() {
            log("stipple: can't open the document picker");
            post_command(Command::Picked(id, Outcome::Opened(None)));
        }
    })
}

/// Let the user choose where to create a document called `name` of type
/// `mime`, and write `bytes` to it, for dialog request `id`. `false` if
/// there's no activity to ask from.
pub(crate) fn create_document(id: u32, name: String, mime: &'static str, bytes: Vec<u8>) -> bool {
    run_on_main(move |jni, activity| {
        let started = (|| {
            let intent = intent(jni, "android.intent.action.CREATE_DOCUMENT")?;
            set_type(jni, intent, mime)?;
            let key = jni.string("android.intent.extra.TITLE")?;
            let value = jni.string16(&name)?;
            jni.call_object(
                intent,
                c"putExtra",
                c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
                &[JValue { l: key }, JValue { l: value }],
            )?;
            start(jni, activity, intent, id, Waiting::Save(bytes))
        })();
        if started.is_none() {
            log("stipple: can't open the document picker");
            post_command(Command::Picked(id, Outcome::Saved(false)));
        }
    })
}

/// `new Intent(action)`, for documents that can be opened as a stream
/// (`CATEGORY_OPENABLE`).
fn intent(jni: &Jni, action: &str) -> Option<JObject> {
    let action = jni.string(action)?;
    let intent = jni.new_named(
        c"android/content/Intent",
        c"(Ljava/lang/String;)V",
        &[JValue { l: action }],
    )?;
    let openable = jni.string("android.intent.category.OPENABLE")?;
    jni.call_object(
        intent,
        c"addCategory",
        c"(Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue { l: openable }],
    )?;
    Some(intent)
}

fn set_type(jni: &Jni, intent: JObject, mime: &str) -> Option<()> {
    let mime = jni.string(mime)?;
    jni.call_object(
        intent,
        c"setType",
        c"(Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue { l: mime }],
    )
    .map(|_| ())
}

/// Start the picker from the results fragment, with request code `id`.
fn start(jni: &Jni, activity: JObject, intent: JObject, id: u32, waiting: Waiting) -> Option<()> {
    let fragment = classes::results_fragment(jni, activity)?;
    let code = id as i32;
    lock().push((code, waiting));
    let started = jni.call_void(
        fragment,
        c"startActivityForResult",
        c"(Landroid/content/Intent;I)V",
        &[JValue { l: intent }, JValue { i: code }],
    );
    if started.is_none() {
        // No activity handles it (ActivityNotFoundException).
        take(code);
        return None;
    }
    log(&format!("stipple: document picker started (request {id})"));
    Some(())
}

fn lock() -> std::sync::MutexGuard<'static, Vec<(i32, Waiting)>> {
    WAITING.lock().unwrap_or_else(|e| e.into_inner())
}

fn take(code: i32) -> Option<Waiting> {
    let mut waiting = lock();
    let i = waiting.iter().position(|(c, _)| *c == code)?;
    Some(waiting.remove(i).1)
}

/// `Results.onActivityResult`: the picker closed. On the main thread.
pub(super) unsafe extern "C" fn on_activity_result(
    env: Env,
    this: JObject,
    code: i32,
    result: i32,
    data: JObject,
) {
    let Some(waiting) = take(code) else {
        return;
    };
    let id = code as u32;
    let failed = waiting.failed();
    if result != RESULT_OK || data.is_null() {
        log(&format!(
            "stipple: document picker cancelled (request {id})"
        ));
        post_command(Command::Picked(id, failed));
        return;
    }
    // The URI and the resolver, held for the thread that opens the document.
    let held = unsafe {
        jni::in_native(env, |jni| {
            let uri = jni.call_object(data, c"getData", c"()Landroid/net/Uri;", &[])?;
            let activity =
                jni.call_object(this, c"getActivity", c"()Landroid/app/Activity;", &[])?;
            let resolver = jni.call_object(
                activity,
                c"getContentResolver",
                c"()Landroid/content/ContentResolver;",
                &[],
            )?;
            Some((jni.global(uri) as usize, jni.global(resolver) as usize))
        })
    };
    let vm = super::with_activity(|a| Some(a.vm as usize));
    let (Some((uri, resolver)), Some(vm)) = (held, vm) else {
        post_command(Command::Picked(id, failed));
        return;
    };
    let if_unspawned = failed.clone();
    let spawned = std::thread::Builder::new()
        .name("stipple-document".into())
        .spawn(move || {
            let outcome = unsafe {
                jni::with_env(vm as *mut c_void, |jni| {
                    let (uri, resolver) = (uri as JObject, resolver as JObject);
                    let outcome = transfer(jni, resolver, uri, waiting);
                    jni.delete_global(uri);
                    jni.delete_global(resolver);
                    Some(outcome)
                })
            };
            post_command(Command::Picked(id, outcome.unwrap_or(failed)));
        });
    if spawned.is_err() {
        post_command(Command::Picked(id, if_unspawned));
    }
}

/// Read the picked document, or write the bytes to the created one.
fn transfer(jni: &Jni, resolver: JObject, uri: JObject, waiting: Waiting) -> Outcome {
    use std::io::{Read, Write};
    match waiting {
        Waiting::Open => {
            let name = display_name(jni, resolver, uri)
                .or_else(|| {
                    let s =
                        jni.call_object(uri, c"getLastPathSegment", c"()Ljava/lang/String;", &[])?;
                    jni.to_rust16(s)
                })
                .unwrap_or_else(|| "document".into());
            let Some(mut file) = open_fd(jni, resolver, uri, "r") else {
                return Outcome::Opened(None);
            };
            let mut bytes = Vec::new();
            if let Err(e) = file.read_to_end(&mut bytes) {
                log(&format!("stipple: can't read {name:?}: {e}"));
                return Outcome::Opened(None);
            }
            log(&format!(
                "stipple: document picked: {name:?} ({} bytes)",
                bytes.len()
            ));
            Outcome::Opened(Some(FileContents { name, bytes }))
        }
        Waiting::Save(bytes) => {
            let Some(mut file) = open_fd(jni, resolver, uri, "w") else {
                return Outcome::Saved(false);
            };
            let saved = file.write_all(&bytes).and_then(|()| file.flush()).is_ok();
            drop(file);
            log(&format!(
                "stipple: document written: {saved} ({} bytes)",
                bytes.len()
            ));
            Outcome::Saved(saved)
        }
    }
}

/// The document's `OpenableColumns.DISPLAY_NAME`.
fn display_name(jni: &Jni, resolver: JObject, uri: JObject) -> Option<String> {
    let columns = jni.string_array(&["_display_name"])?;
    let none = std::ptr::null_mut();
    let cursor = jni.call_object(
        resolver,
        c"query",
        c"(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
        &[
            JValue { l: uri },
            JValue { l: columns },
            JValue { l: none },
            JValue { l: none },
            JValue { l: none },
        ],
    )?;
    let name = jni
        .call_bool(cursor, c"moveToFirst", c"()Z", &[])
        .filter(|&first| first)
        .and_then(|_| {
            jni.call_object(
                cursor,
                c"getString",
                c"(I)Ljava/lang/String;",
                &[JValue { i: 0 }],
            )
        })
        .and_then(|s| jni.to_rust16(s));
    jni.call_void(cursor, c"close", c"()V", &[]);
    name.filter(|n| !n.is_empty())
}

/// The document as a file Rust owns: `openFileDescriptor(uri, mode)`, then
/// `detachFd`.
fn open_fd(jni: &Jni, resolver: JObject, uri: JObject, mode: &str) -> Option<std::fs::File> {
    use std::os::fd::FromRawFd;
    let mode = jni.string(mode)?;
    let pfd = jni.call_object(
        resolver,
        c"openFileDescriptor",
        c"(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;",
        &[JValue { l: uri }, JValue { l: mode }],
    );
    let Some(pfd) = pfd else {
        log("stipple: can't open the document");
        return None;
    };
    let fd = jni.call_int(pfd, c"detachFd", c"()I", &[])?;
    // SAFETY: detachFd hands the descriptor over; the File closes it.
    (fd >= 0).then(|| unsafe { std::fs::File::from_raw_fd(fd) })
}

/// On the UI thread: hand the outcome to the app's callback.
pub(super) fn deliver(id: u32, outcome: Outcome) {
    match outcome {
        Outcome::Opened(file) => mobile::finish_open(id, file),
        Outcome::Saved(saved) => mobile::finish_save(id, saved),
    }
}
