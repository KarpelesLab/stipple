//! What the mobile document pickers share: the callbacks waiting for a
//! picker's answer, and turning [`FileDialog::accept`](super::FileDialog::accept)
//! (the web's `<input accept>` list) into the MIME types Android's Storage
//! Access Framework filters by and the uniform type identifiers iOS's
//! `UIDocumentPickerViewController` wants.
//!
//! The pickers answer later, from the event loop: on iOS the delegate's
//! methods run on the main thread, which is the loop's; on Android the
//! activity result reaches a fragment on the Java main thread, and the backend
//! hands it to the UI thread as a command. Either way [`finish_open`] and
//! [`finish_save`] run on the thread that called [`register`].

/// One item of an `accept` list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Accept {
    /// A file name extension, lowercased, without the dot: `txt`.
    Extension(String),
    /// A MIME type, lowercased; the subtype may be `*`: `image/*`.
    Mime(String),
}

/// Split an `accept` list (`".txt, .md,image/*"`) into its items. A bare
/// word (`txt`) is taken for an extension.
pub(crate) fn parse_accept(accept: &str) -> Vec<Accept> {
    accept
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != ".")
        .map(|s| {
            let s = s.to_ascii_lowercase();
            if let Some(ext) = s.strip_prefix('.') {
                Accept::Extension(ext.to_string())
            } else if s.contains('/') {
                Accept::Mime(s)
            } else {
                Accept::Extension(s)
            }
        })
        .collect()
}

/// The MIME types of common extensions, for Android, whose picker filters by
/// MIME type only.
const EXTENSIONS: &[(&str, &str)] = &[
    ("txt", "text/plain"),
    ("text", "text/plain"),
    ("log", "text/plain"),
    ("md", "text/markdown"),
    ("markdown", "text/markdown"),
    ("csv", "text/csv"),
    ("tsv", "text/tab-separated-values"),
    ("html", "text/html"),
    ("htm", "text/html"),
    ("css", "text/css"),
    ("js", "text/javascript"),
    ("xml", "text/xml"),
    ("json", "application/json"),
    ("pdf", "application/pdf"),
    ("zip", "application/zip"),
    ("gz", "application/gzip"),
    ("rtf", "application/rtf"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("svg", "image/svg+xml"),
    ("ico", "image/x-icon"),
    ("heic", "image/heic"),
    ("mp3", "audio/mpeg"),
    ("m4a", "audio/mp4"),
    ("ogg", "audio/ogg"),
    ("wav", "audio/x-wav"),
    ("flac", "audio/flac"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("mov", "video/quicktime"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
];

/// The MIME type of file name extension `ext` (lowercase, no dot), if it's a
/// common one.
fn mime_of_extension(ext: &str) -> Option<&'static str> {
    EXTENSIONS.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
}

/// The MIME types an Android `ACTION_OPEN_DOCUMENT` should offer for
/// `accept`, without repeats; empty for any file. An extension without a
/// known type stands for `application/octet-stream`, the type document
/// providers give files they don't recognize.
pub(crate) fn android_mime_types(accept: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in parse_accept(accept) {
        let mime = match item {
            Accept::Mime(m) if m == "*/*" || m == "*" => return Vec::new(),
            Accept::Mime(m) => m,
            Accept::Extension(e) => mime_of_extension(&e)
                .unwrap_or("application/octet-stream")
                .to_string(),
        };
        if !out.contains(&mime) {
            out.push(mime);
        }
    }
    out
}

/// The MIME type of a document called `name`, for Android's
/// `ACTION_CREATE_DOCUMENT`: from its extension, else
/// `application/octet-stream`.
pub(crate) fn mime_for_name(name: &str) -> &'static str {
    name.rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .and_then(|(_, ext)| mime_of_extension(&ext.to_ascii_lowercase()))
        .unwrap_or("application/octet-stream")
}

/// How to name one content type to iOS's `UTType`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ContentType {
    /// `[UTType typeWithIdentifier:]`: `public.image`.
    Identifier(String),
    /// `[UTType typeWithFilenameExtension:]`: `txt`.
    Extension(String),
    /// `[UTType typeWithMIMEType:]`: `application/pdf`.
    Mime(String),
}

/// The content types an iOS document picker should offer for `accept`: a
/// MIME type with a `*` subtype is its family's identifier (`image/*` is
/// `public.image`); nothing (or `*/*`) is `public.data`, any file.
pub(crate) fn ios_content_types(accept: &str) -> Vec<ContentType> {
    let any = || vec![ContentType::Identifier("public.data".into())];
    let mut out: Vec<ContentType> = Vec::new();
    for item in parse_accept(accept) {
        let t = match item {
            Accept::Extension(e) => ContentType::Extension(e),
            Accept::Mime(m) => match m.split_once('/') {
                Some(("*", _)) | None => return any(),
                Some((family, "*")) => ContentType::Identifier(
                    match family {
                        "image" => "public.image",
                        "text" => "public.text",
                        "audio" => "public.audio",
                        "video" => "public.movie",
                        "font" => "public.font",
                        _ => "public.data",
                    }
                    .into(),
                ),
                Some(_) => ContentType::Mime(m),
            },
        };
        if !out.contains(&t) {
            out.push(t);
        }
    }
    if out.is_empty() { any() } else { out }
}

#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) use pending::{Done, finish_open, finish_save, register};

/// The callbacks waiting for a picker, by request id, on the UI thread.
#[cfg(any(target_os = "ios", target_os = "android"))]
mod pending {
    use super::super::FileContents;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    /// What to call with a picker's answer.
    pub(crate) enum Done {
        Open(Box<dyn FnOnce(Option<FileContents>)>),
        Save(Box<dyn FnOnce(bool)>),
    }

    thread_local! {
        static PENDING: RefCell<HashMap<u32, Done>> = RefCell::new(HashMap::new());
        static NEXT: Cell<u32> = const { Cell::new(1) };
    }

    /// Keep `done` until the picker answers; returns the request's id, in
    /// `1..0x8000` (Android passes it as the activity's request code).
    pub(crate) fn register(done: Done) -> u32 {
        let id = NEXT.with(|n| {
            let id = n.get();
            n.set(if id >= 0x7fff { 1 } else { id + 1 });
            id
        });
        PENDING.with(|p| p.borrow_mut().insert(id, done));
        id
    }

    fn take(id: u32) -> Option<Done> {
        PENDING.with(|p| p.borrow_mut().remove(&id))
    }

    /// Request `id` (an open) is answered: the file, or `None`.
    pub(crate) fn finish_open(id: u32, file: Option<FileContents>) {
        match take(id) {
            Some(Done::Open(done)) => done(file),
            Some(Done::Save(done)) => done(false),
            None => {}
        }
    }

    /// Request `id` (a save) is answered: whether the bytes were written.
    pub(crate) fn finish_save(id: u32, saved: bool) {
        match take(id) {
            Some(Done::Save(done)) => done(saved),
            Some(Done::Open(done)) => done(None),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_lists_split_into_extensions_and_types() {
        assert_eq!(
            parse_accept(" .TXT, md ,image/*,, ."),
            vec![
                Accept::Extension("txt".into()),
                Accept::Extension("md".into()),
                Accept::Mime("image/*".into()),
            ]
        );
        assert!(parse_accept("").is_empty());
    }

    #[test]
    fn android_filters_by_mime_type() {
        assert!(android_mime_types("").is_empty());
        assert_eq!(
            android_mime_types(".txt,.text,text/*,.PNG"),
            ["text/plain", "text/*", "image/png"]
        );
        assert_eq!(android_mime_types(".stipple"), ["application/octet-stream"]);
        assert!(android_mime_types(".txt,*/*").is_empty());
        assert_eq!(mime_for_name("notes.TXT"), "text/plain");
        assert_eq!(mime_for_name("archive.tar.gz"), "application/gzip");
        assert_eq!(mime_for_name(".profile"), "application/octet-stream");
        assert_eq!(mime_for_name("README"), "application/octet-stream");
    }

    #[test]
    fn ios_names_uniform_types() {
        let id = |s: &str| ContentType::Identifier(s.into());
        assert_eq!(ios_content_types(""), [id("public.data")]);
        assert_eq!(ios_content_types("*/*,.txt"), [id("public.data")]);
        assert_eq!(
            ios_content_types(".txt,image/*,video/*,application/pdf,.txt"),
            [
                ContentType::Extension("txt".into()),
                id("public.image"),
                id("public.movie"),
                ContentType::Mime("application/pdf".into()),
            ]
        );
    }
}
