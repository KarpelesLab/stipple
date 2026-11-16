//! Drops from other apps (split screen, freeform windows, a desktop):
//! `View.onDragEvent` on the [host view](super::classes), which fills the
//! activity's content.
//!
//! Android sends every window `ACTION_DRAG_STARTED` with the drag's
//! `ClipDescription`; the view asks for the rest of the drag if it carries
//! text or anything else the app can take as a file (an image, a PDF, a
//! `content:` URI of any type). `ACTION_DRAG_ENTERED` and the first
//! `ACTION_DRAG_LOCATION` become [`Event::DragEnter`], later locations
//! [`Event::DragOver`], `ACTION_DRAG_EXITED` (and `ACTION_DRAG_ENDED`
//! without a drop) [`Event::DragLeave`].
//!
//! On `ACTION_DROP` the view asks for the drag's URI permissions
//! (`Activity.requestDragAndDropPermissions`, Android 7 and later) and reads
//! the `ClipData`: each item's text, else its URI: a `file:` URI is its
//! path; a `content:` URI is opened through the `ContentResolver`
//! (`openFileDescriptor`) and copied, under its `OpenableColumns`
//! display name, into a directory of its own under the app's temporary
//! directory (on a thread of its own, as a provider may have to fetch it
//! first), and the drop carries the copy's path
//! ([`mobile_drop`](crate::platform::backend::mobile_drop)). Items with
//! neither (an `Intent`) are read with `coerceToText`. The drop reaches the
//! app's UI thread as [`Event::Drop`] once read.
//!
//! The callbacks run on the main (Java) thread, which can't wait for the
//! app: the answer to `ACTION_DROP` (the drag's result, for its source) is
//! the app's verdict from the last move ([`set_accepted`]), as on Windows
//! and macOS. Android shows no per-target cursor badge, so the verdict
//! changes nothing else.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::jni::{self, Env, JObject, JValue, Jni};
use super::{Command, log, post_command};
use crate::geometry::Point;
use crate::platform::backend::mobile_drop::{self, Item};
use crate::platform::event::{DragOffer, DropData, Event};

// DragEvent's actions.
const ACTION_DRAG_STARTED: i32 = 1;
const ACTION_DRAG_LOCATION: i32 = 2;
const ACTION_DROP: i32 = 3;
const ACTION_DRAG_ENDED: i32 = 4;
const ACTION_DRAG_ENTERED: i32 = 5;
const ACTION_DRAG_EXITED: i32 = 6;

/// A drag event for the UI thread, in window pixels.
#[derive(Clone, Debug)]
pub(crate) enum Drag {
    Enter { x: f32, y: f32, offer: DragOffer },
    Over { x: f32, y: f32 },
    Leave,
    Drop { x: f32, y: f32, data: DropData },
}

impl Drag {
    /// The app's event, its position made logical by `to_logical`.
    pub(super) fn event(self, to_logical: impl Fn(f32, f32) -> Point) -> Event {
        match self {
            Drag::Enter { x, y, offer } => Event::DragEnter {
                position: to_logical(x, y),
                offer,
            },
            Drag::Over { x, y } => Event::DragOver {
                position: to_logical(x, y),
            },
            Drag::Leave => Event::DragLeave,
            Drag::Drop { x, y, data } => Event::Drop {
                position: to_logical(x, y),
                data,
            },
        }
    }
}

/// The drag in progress, as the main thread follows it.
#[derive(Clone, Copy, Debug)]
struct State {
    offer: DragOffer,
    /// Over the view (between `ENTERED` and `EXITED`).
    inside: bool,
    /// The app was told the drag entered (and not yet that it left).
    told: bool,
    /// Where it last was, in window pixels.
    at: (f32, f32),
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// The app's verdict on the drag, from its last move.
static ACCEPTED: AtomicBool = AtomicBool::new(false);
/// Drops so far, numbering their directories.
static DROPS: AtomicU64 = AtomicU64::new(1);

/// `Window::set_drop_accepted`: the app's verdict on the drag where it is.
pub(super) fn set_accepted(accepted: bool) {
    ACCEPTED.store(accepted, Ordering::Relaxed);
}

/// `HostView.onDragEvent`, on the main thread.
pub(super) unsafe extern "C" fn on_drag_event(env: Env, this: JObject, event: JObject) -> u8 {
    let answer = unsafe {
        jni::in_native(env, |jni| {
            let action = jni.call_int(event, c"getAction", c"()I", &[])?;
            Some(handle(jni, this, event, action))
        })
    };
    answer.unwrap_or(false) as u8
}

fn state() -> std::sync::MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn handle(jni: &Jni, view: JObject, event: JObject, action: i32) -> bool {
    match action {
        ACTION_DRAG_STARTED => {
            let offer = offer_of(jni, event);
            if offer.is_empty() {
                *state() = None;
                return false;
            }
            *state() = Some(State {
                offer,
                inside: false,
                told: false,
                at: (0.0, 0.0),
            });
            true
        }
        ACTION_DRAG_ENTERED => {
            if let Some(s) = state().as_mut() {
                s.inside = true;
            }
            true
        }
        ACTION_DRAG_LOCATION => {
            let at = position(jni, view, event);
            let mut guard = state();
            let Some(s) = guard.as_mut() else {
                return false;
            };
            s.inside = true;
            s.at = at;
            let (x, y) = at;
            if std::mem::replace(&mut s.told, true) {
                post_command(Command::Drag(Drag::Over { x, y }));
            } else {
                // Until the app answers, take anything that can be read.
                ACCEPTED.store(true, Ordering::Relaxed);
                post_command(Command::Drag(Drag::Enter {
                    x,
                    y,
                    offer: s.offer,
                }));
            }
            true
        }
        ACTION_DRAG_EXITED => {
            if let Some(s) = state().as_mut() {
                s.inside = false;
                if std::mem::take(&mut s.told) {
                    post_command(Command::Drag(Drag::Leave));
                }
            }
            true
        }
        ACTION_DROP => {
            let at = position(jni, view, event);
            let (offer, told) = match state().take() {
                Some(s) => (s.offer, s.told),
                None => (offer_of(jni, event), false),
            };
            let (x, y) = at;
            if !told {
                post_command(Command::Drag(Drag::Enter { x, y, offer }));
            }
            let accepted = ACCEPTED.load(Ordering::Relaxed);
            if !read_drop(jni, event, at) {
                post_command(Command::Drag(Drag::Leave));
                return false;
            }
            accepted
        }
        ACTION_DRAG_ENDED => {
            // Ended without a drop here (cancelled, or dropped elsewhere).
            if let Some(s) = state().take()
                && s.told
            {
                post_command(Command::Drag(Drag::Leave));
            }
            true
        }
        _ => false,
    }
}

/// What the drag carries, from its `ClipDescription`'s MIME types.
fn offer_of(jni: &Jni, event: JObject) -> DragOffer {
    let mimes = (|| {
        let desc = jni.call_object(
            event,
            c"getClipDescription",
            c"()Landroid/content/ClipDescription;",
            &[],
        )?;
        let n = jni.call_int(desc, c"getMimeTypeCount", c"()I", &[])?;
        Some(
            (0..n)
                .filter_map(|i| {
                    let s = jni.call_object(
                        desc,
                        c"getMimeType",
                        c"(I)Ljava/lang/String;",
                        &[JValue { i }],
                    )?;
                    jni.to_rust16(s)
                })
                .collect::<Vec<_>>(),
        )
    })()
    .unwrap_or_default();
    mobile_drop::offer_from_mimes(mimes.iter().map(String::as_str))
}

/// The event's position, in window pixels (it comes in the view's).
fn position(jni: &Jni, view: JObject, event: JObject) -> (f32, f32) {
    let x = jni.call_float(event, c"getX", c"()F", &[]).unwrap_or(0.0);
    let y = jni.call_float(event, c"getY", c"()F", &[]).unwrap_or(0.0);
    let origin = (|| {
        let xy = jni.int_array(2)?;
        jni.call_void(view, c"getLocationInWindow", c"([I)V", &[JValue { l: xy }])?;
        jni.ints::<2>(xy)
    })()
    .unwrap_or([0, 0]);
    (x + origin[0] as f32, y + origin[1] as f32)
}

/// Read the dropped `ClipData`: texts at once, URIs on a thread of their
/// own, which posts the drop when done. `false` if there's nothing to read.
fn read_drop(jni: &Jni, event: JObject, (x, y): (f32, f32)) -> bool {
    let host = super::classes::host();
    let activity = host.activity.get();
    let Some(clip) = jni.call_object(event, c"getClipData", c"()Landroid/content/ClipData;", &[])
    else {
        return false;
    };
    // The drag's URI permissions (Android 7 and later); held until the
    // activity goes, which is long enough to copy them.
    if !activity.is_null() {
        let _ = jni.call_object(
            activity,
            c"requestDragAndDropPermissions",
            c"(Landroid/view/DragEvent;)Landroid/view/DragAndDropPermissions;",
            &[JValue { l: event }],
        );
    }
    let n = jni
        .call_int(clip, c"getItemCount", c"()I", &[])
        .unwrap_or(0);
    let mut texts = Vec::new();
    let mut uris = Vec::new();
    for i in 0..n {
        let Some(item) = jni.call_object(
            clip,
            c"getItemAt",
            c"(I)Landroid/content/ClipData$Item;",
            &[JValue { i }],
        ) else {
            continue;
        };
        let text = jni
            .call_object(item, c"getText", c"()Ljava/lang/CharSequence;", &[])
            .and_then(|t| jni.char_sequence(t));
        if let Some(text) = text {
            texts.push(text);
            continue;
        }
        if let Some(uri) = jni.call_object(item, c"getUri", c"()Landroid/net/Uri;", &[]) {
            uris.push(jni.global(uri) as usize);
            continue;
        }
        // An Intent, or HTML only: as text.
        if !activity.is_null()
            && let Some(text) = jni
                .call_object(
                    item,
                    c"coerceToText",
                    c"(Landroid/content/Context;)Ljava/lang/CharSequence;",
                    &[JValue { l: activity }],
                )
                .and_then(|t| jni.char_sequence(t))
                .filter(|t| !t.is_empty())
        {
            texts.push(text);
        }
    }
    let mut items: Vec<Item> = texts.into_iter().map(Item::Text).collect();
    if uris.is_empty() {
        return match mobile_drop::merge(items) {
            Some(data) => {
                log_drop(&data);
                post_command(Command::Drag(Drag::Drop { x, y, data }));
                true
            }
            None => false,
        };
    }
    let resolver = (!activity.is_null())
        .then(|| {
            jni.call_object(
                activity,
                c"getContentResolver",
                c"()Landroid/content/ContentResolver;",
                &[],
            )
        })
        .flatten()
        .map(|r| jni.global(r) as usize);
    let vm = super::with_activity(|a| Some(a.vm as usize));
    let copy = move || {
        if let (Some(vm), Some(resolver)) = (vm, resolver) {
            unsafe {
                jni::with_env(vm as *mut std::ffi::c_void, |jni| {
                    let dir = super::app_dirs().map(|d| d.temp)?;
                    let _ = std::fs::create_dir_all(&dir);
                    let dir = mobile_drop::drop_dir(&dir, DROPS.fetch_add(1, Ordering::Relaxed))
                        .map_err(|e| log(&format!("stipple: no directory for the drop: {e}")))
                        .ok()?;
                    for &uri in &uris {
                        if let Some(path) = copy_uri(jni, resolver as JObject, uri as JObject, &dir)
                        {
                            items.push(Item::File(path));
                        }
                    }
                    for &g in uris.iter().chain([&resolver]) {
                        jni.delete_global(g as JObject);
                    }
                    Some(())
                });
            }
        }
        match mobile_drop::merge(items) {
            Some(data) => {
                log_drop(&data);
                post_command(Command::Drag(Drag::Drop { x, y, data }));
            }
            None => post_command(Command::Drag(Drag::Leave)),
        }
    };
    if std::thread::Builder::new()
        .name("stipple-drop".into())
        .spawn(copy)
        .is_err()
    {
        post_command(Command::Drag(Drag::Leave));
    }
    true
}

fn log_drop(data: &DropData) {
    let what = match data {
        DropData::Files(f) => format!("{} file(s)", f.len()),
        DropData::Text(t) => format!("text ({} bytes)", t.len()),
        DropData::Internal(_) => "internal".into(),
    };
    log(&format!("stipple: drop read: {what}"));
}

/// The file a dropped URI stands for: a `file:` URI's path, or a copy of a
/// `content:` URI's document in `dir`.
fn copy_uri(
    jni: &Jni,
    resolver: JObject,
    uri: JObject,
    dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let text = jni
        .call_object(uri, c"toString", c"()Ljava/lang/String;", &[])
        .and_then(|s| jni.to_rust16(s))?;
    if let Some(path) = mobile_drop::file_uri_path(&text) {
        return Some(path);
    }
    let mime = jni
        .call_object(
            resolver,
            c"getType",
            c"(Landroid/net/Uri;)Ljava/lang/String;",
            &[JValue { l: uri }],
        )
        .and_then(|s| jni.to_rust16(s));
    let shown = super::pick::display_name(jni, resolver, uri);
    let segment = jni
        .call_object(uri, c"getLastPathSegment", c"()Ljava/lang/String;", &[])
        .and_then(|s| jni.to_rust16(s))
        .unwrap_or_default();
    let name = mobile_drop::file_name(shown.as_deref(), &segment, mime.as_deref());
    let mut from = super::pick::open_fd(jni, resolver, uri, "r")?;
    let path = mobile_drop::unique_path(dir, &name);
    let copied = std::fs::File::create(&path).and_then(|mut to| std::io::copy(&mut from, &mut to));
    match copied {
        Ok(bytes) => {
            log(&format!("stipple: dropped {name:?} copied ({bytes} bytes)"));
            Some(path)
        }
        Err(e) => {
            log(&format!("stipple: can't copy the dropped {name:?}: {e}"));
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

/// `View.DRAG_FLAG_GLOBAL`: the drag may cross into other apps' windows.
const DRAG_FLAG_GLOBAL: i32 = 1 << 8;
/// `View.DRAG_FLAG_GLOBAL_URI_READ`: the target may read its URIs.
const DRAG_FLAG_GLOBAL_URI_READ: i32 = 1;

/// What a self-test drags.
pub(super) enum Dragged {
    Text(String),
    /// A file, put in `MediaStore`'s Downloads first (Android 10 and later)
    /// so the drag carries a `content:` URI, as a file manager's does.
    File(crate::platform::shell::ShareFile),
}

/// Start a system drag from the host view, on the main thread (see
/// [`drag_self_test`](super::drag_self_test)).
pub(super) fn start_drag(jni: &Jni, activity: JObject, what: &Dragged) {
    let view = super::classes::host().view.get();
    let started = (|| {
        if view.is_null() {
            return None;
        }
        let label = jni.string("stipple")?;
        let clip = match what {
            Dragged::Text(text) => {
                let text = jni.string16(text)?;
                jni.call_static_object(
                    c"android/content/ClipData",
                    c"newPlainText",
                    c"(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                    &[JValue { l: label }, JValue { l: text }],
                )?
            }
            Dragged::File(file) => {
                let uri = super::share::media_store(jni, activity, file)?;
                let resolver = jni.call_object(
                    activity,
                    c"getContentResolver",
                    c"()Landroid/content/ContentResolver;",
                    &[],
                )?;
                jni.call_static_object(
                    c"android/content/ClipData",
                    c"newUri",
                    c"(Landroid/content/ContentResolver;Ljava/lang/CharSequence;Landroid/net/Uri;)Landroid/content/ClipData;",
                    &[JValue { l: resolver }, JValue { l: label }, JValue { l: uri }],
                )?
            }
        };
        // The view's (empty) picture as the shadow: a zero-sized one is
        // refused.
        let shadow = jni.new_named(
            c"android/view/View$DragShadowBuilder",
            c"(Landroid/view/View;)V",
            &[JValue { l: view }],
        )?;
        jni.call_bool(
            view,
            c"startDragAndDrop",
            c"(Landroid/content/ClipData;Landroid/view/View$DragShadowBuilder;Ljava/lang/Object;I)Z",
            &[
                JValue { l: clip },
                JValue { l: shadow },
                JValue {
                    l: std::ptr::null_mut(),
                },
                JValue {
                    i: DRAG_FLAG_GLOBAL | DRAG_FLAG_GLOBAL_URI_READ,
                },
            ],
        )
    })();
    log(&format!("stipple: drag self-test started: {started:?}"));
}
