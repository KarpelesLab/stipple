//! What iOS and Android drops carry, turned into [`DropData`]: the parts of
//! [`ios::drop`](super::ios) and [`android::drag`](super::android) that need
//! neither UIKit nor the JVM, so they're unit-tested on every host.
//!
//! Both systems hand over items by type (UTIs on iOS, MIME types on
//! Android). Text stays text; anything else (an image, a PDF, a file from
//! the Files app or a file manager) arrives as a stream or a temporary file
//! that is only readable while the drop is being handled, so it is copied
//! into a fresh directory under the app's temporary directory and the drop
//! carries that copy's path, as [`DropData::Files`]. A drop of several items
//! carries every file; one with no files carries its texts, one per line.

use std::path::{Path, PathBuf};

use crate::platform::event::{DragOffer, DropData};

/// One item of a drop, once read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    /// Plain text (or a web link, as its URL).
    Text(String),
    /// A file, copied to a path the app may read.
    File(PathBuf),
}

/// The drop's data from its items, or `None` if nothing could be read:
/// every file if there is one, else the texts joined by newlines.
pub(crate) fn merge(items: Vec<Item>) -> Option<DropData> {
    let mut files = Vec::new();
    let mut texts = Vec::new();
    for item in items {
        match item {
            Item::File(path) => files.push(path),
            Item::Text(text) => texts.push(text),
        }
    }
    if !files.is_empty() {
        Some(DropData::Files(files))
    } else if !texts.is_empty() {
        Some(DropData::Text(texts.join("\n")))
    } else {
        None
    }
}

/// How an item is read.
#[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Read {
    /// As text.
    Text,
    /// As a URL, kept as its text (a link dragged from a browser).
    Url,
    /// As a file of its own (copied).
    File,
}

/// Whether a MIME type (from Android's `ClipDescription`) is text the drop
/// can carry as [`DropData::Text`].
#[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
pub(crate) fn mime_is_text(mime: &str) -> bool {
    let mime = mime.trim().to_ascii_lowercase();
    mime == "text/plain"
        || mime.starts_with("text/plain;")
        || mime == "text/html"
        || mime == "text/uri-list"
        // ClipDescription.MIMETYPE_TEXT_INTENT and _URILIST stand-ins.
        || mime == "text/vnd.android.intent"
}

/// What a drag carrying items of these MIME types offers: text for the text
/// types, files for anything else (an image, a PDF, `*/*`: a `content:` URI
/// the app copies).
#[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
pub(crate) fn offer_from_mimes<'a>(mimes: impl IntoIterator<Item = &'a str>) -> DragOffer {
    let mut offer = DragOffer {
        files: false,
        text: false,
        internal: false,
    };
    for mime in mimes {
        if mime_is_text(mime) {
            offer.text = true;
        } else if !mime.trim().is_empty() {
            offer.files = true;
        }
    }
    offer
}

/// What an iOS item provider says about itself.
#[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct IosItem {
    /// It conforms to `public.image`.
    pub(crate) image: bool,
    /// To `public.plain-text`.
    pub(crate) text: bool,
    /// To `public.url`.
    pub(crate) url: bool,
    /// To `public.file-url`.
    pub(crate) file_url: bool,
    /// It has a `suggestedName` (a file's name, from the Files app).
    pub(crate) named: bool,
    /// It has any type at all.
    pub(crate) typed: bool,
}

/// How to read an iOS item provider. Images come as files (a picture from
/// Photos or Safari), then a web link as its URL, then anything named (a
/// file from the Files app, even a text file) as a file, then text; anything
/// else with a type is a file (`loadFileRepresentationForTypeIdentifier:`).
#[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
pub(crate) fn ios_read(item: IosItem) -> Option<Read> {
    if item.image || item.file_url {
        Some(Read::File)
    } else if item.url {
        Some(Read::Url)
    } else if item.named && item.typed {
        Some(Read::File)
    } else if item.text {
        Some(Read::Text)
    } else if item.typed {
        Some(Read::File)
    } else {
        None
    }
}

/// What a drop of items read as `reads` offers.
#[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
pub(crate) fn offer_from_reads(reads: impl IntoIterator<Item = Read>) -> DragOffer {
    let mut offer = DragOffer {
        files: false,
        text: false,
        internal: false,
    };
    for read in reads {
        match read {
            Read::Text | Read::Url => offer.text = true,
            Read::File => offer.files = true,
        }
    }
    offer
}

/// The file name extension for a MIME type or UTI, for names that lack one.
pub(crate) fn extension_for(ty: &str) -> Option<&'static str> {
    let ty = ty.trim().to_ascii_lowercase();
    let ty = ty.split(';').next().unwrap_or_default();
    Some(match ty {
        "image/png" | "public.png" => "png",
        "image/jpeg" | "image/jpg" | "public.jpeg" => "jpg",
        "image/gif" | "com.compuserve.gif" => "gif",
        "image/webp" | "org.webmproject.webp" => "webp",
        "image/heic" | "public.heic" => "heic",
        "image/heif" | "public.heif" => "heif",
        "image/tiff" | "public.tiff" => "tiff",
        "image/bmp" | "com.microsoft.bmp" => "bmp",
        "image/svg+xml" | "public.svg-image" => "svg",
        "application/pdf" | "com.adobe.pdf" => "pdf",
        "text/plain" | "public.plain-text" | "public.utf8-plain-text" => "txt",
        "text/html" | "public.html" => "html",
        "application/json" | "public.json" => "json",
        "application/zip" | "public.zip-archive" => "zip",
        "audio/mpeg" | "public.mp3" => "mp3",
        "video/mp4" | "public.mpeg-4" => "mp4",
        "video/quicktime" | "com.apple.quicktime-movie" => "mov",
        _ => return None,
    })
}

/// A safe file name for a dropped item: `suggested` (the item's display
/// name) stripped of directories and characters no file system takes, else
/// `fallback`; given the extension for `ty` if it has none.
pub(crate) fn file_name(suggested: Option<&str>, fallback: &str, ty: Option<&str>) -> String {
    let clean = |s: &str| -> String {
        let last = s.rsplit(['/', '\\']).next().unwrap_or_default();
        let s: String = last
            .chars()
            .map(|c| {
                if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let s = s.trim().trim_start_matches('.').to_string();
        // Keep under the usual 255-byte limit, on a character boundary.
        let mut end = s.len().min(200);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s[..end].to_string()
    };
    let mut name = suggested.map(clean).filter(|s| !s.is_empty());
    if name.is_none() {
        name = Some(clean(fallback)).filter(|s| !s.is_empty());
    }
    let mut name = name.unwrap_or_else(|| "dropped".into());
    let has_extension = Path::new(&name).extension().is_some_and(|e| !e.is_empty());
    if !has_extension && let Some(ext) = ty.and_then(extension_for) {
        name.push('.');
        name.push_str(ext);
    }
    name
}

/// A new, empty directory for one drop's files: `<base>/stipple-drops/<n>`,
/// the first `n` from `seq` not taken. Each drop gets its own, so two items
/// of the same name don't collide and a later drop doesn't overwrite an
/// earlier one's files.
pub(crate) fn drop_dir(base: &Path, seq: u64) -> std::io::Result<PathBuf> {
    let root = base.join("stipple-drops");
    std::fs::create_dir_all(&root)?;
    for n in seq.. {
        let dir = root.join(n.to_string());
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!("u64 ran out of drop directories")
}

/// `name` in `dir`, numbered (`name (2).ext`, …) if a file of that name is
/// there already.
pub(crate) fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    if !path.exists() {
        return path;
    }
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = p.extension().and_then(|s| s.to_str());
    (2..)
        .map(|i| match ext {
            Some(ext) => dir.join(format!("{stem} ({i}).{ext}")),
            None => dir.join(format!("{stem} ({i})")),
        })
        .find(|p| !p.exists())
        .expect("a free name")
}

/// The local path a `file:` URI stands for, if it is one (Android items can
/// carry them; other schemes go through the `ContentResolver`).
#[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
pub(crate) fn file_uri_path(uri: &str) -> Option<PathBuf> {
    crate::platform::parse_uri_list(uri).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_win_over_texts_and_texts_join() {
        assert_eq!(merge(vec![]), None);
        assert_eq!(
            merge(vec![Item::Text("a".into()), Item::Text("b".into())]),
            Some(DropData::Text("a\nb".into()))
        );
        assert_eq!(
            merge(vec![
                Item::Text("caption".into()),
                Item::File("/tmp/x.png".into()),
                Item::File("/tmp/y.pdf".into()),
            ]),
            Some(DropData::Files(vec![
                "/tmp/x.png".into(),
                "/tmp/y.pdf".into()
            ]))
        );
    }

    #[test]
    fn mime_types_offer_text_or_files() {
        assert_eq!(offer_from_mimes(["text/plain"]), DragOffer::TEXT);
        assert_eq!(
            offer_from_mimes(["text/html", "text/uri-list"]),
            DragOffer::TEXT
        );
        assert_eq!(offer_from_mimes(["image/png"]), DragOffer::FILES);
        assert_eq!(offer_from_mimes(["application/pdf"]), DragOffer::FILES);
        let both = offer_from_mimes(["text/plain", "image/jpeg"]);
        assert!(both.text && both.files && !both.internal);
        assert!(offer_from_mimes(Vec::<&str>::new()).is_empty());
        assert!(mime_is_text("TEXT/PLAIN; charset=utf-8"));
        assert!(!mime_is_text("image/*"));
    }

    #[test]
    fn ios_items_read_by_type() {
        let typed = IosItem {
            typed: true,
            ..IosItem::default()
        };
        // A photo (also offering its URL, and named) is a file.
        let photo = IosItem {
            image: true,
            url: true,
            named: true,
            ..typed
        };
        assert_eq!(ios_read(photo), Some(Read::File));
        // A file URL.
        let file_url = IosItem {
            url: true,
            file_url: true,
            ..typed
        };
        assert_eq!(ios_read(file_url), Some(Read::File));
        // A text file from the Files app: named.
        let text_file = IosItem {
            text: true,
            named: true,
            ..typed
        };
        assert_eq!(ios_read(text_file), Some(Read::File));
        // Selected text.
        let text = IosItem {
            text: true,
            ..typed
        };
        assert_eq!(ios_read(text), Some(Read::Text));
        // A link from Safari (with its title, and its text).
        let link = IosItem {
            url: true,
            text: true,
            named: true,
            ..typed
        };
        assert_eq!(ios_read(link), Some(Read::Url));
        // A PDF or anything else with a type.
        assert_eq!(ios_read(typed), Some(Read::File));
        assert_eq!(ios_read(IosItem::default()), None);
        let offer = offer_from_reads([Read::Url, Read::File]);
        assert!(offer.text && offer.files);
        assert_eq!(offer_from_reads([Read::Text]), DragOffer::TEXT);
    }

    #[test]
    fn file_names_are_cleaned_and_get_extensions() {
        assert_eq!(
            file_name(Some("photo.jpg"), "x", Some("image/png")),
            "photo.jpg"
        );
        assert_eq!(
            file_name(Some("IMG_0001"), "x", Some("public.jpeg")),
            "IMG_0001.jpg"
        );
        assert_eq!(file_name(Some("../../etc/passwd"), "x", None), "passwd");
        assert_eq!(file_name(Some("a:b*c?.txt"), "x", None), "a_b_c_.txt");
        assert_eq!(
            file_name(Some("..."), "item", Some("image/png")),
            "item.png"
        );
        assert_eq!(file_name(None, "", Some("application/pdf")), "dropped.pdf");
        assert_eq!(file_name(Some(".hidden"), "x", None), "hidden");
        let long = "é".repeat(300);
        let name = file_name(Some(&long), "x", None);
        assert!(name.len() <= 200 && name.chars().all(|c| c == 'é'));
        assert_eq!(extension_for("image/jpeg; q=1"), Some("jpg"));
        assert_eq!(extension_for("application/x-unknown"), None);
    }

    #[test]
    fn drops_get_their_own_directories_and_names_dont_collide() {
        let base = std::env::temp_dir().join(format!("stipple-drop-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let a = drop_dir(&base, 1).unwrap();
        let b = drop_dir(&base, 1).unwrap();
        assert_ne!(a, b);
        assert!(a.is_dir() && b.is_dir());
        let first = unique_path(&a, "x.png");
        std::fs::write(&first, b"1").unwrap();
        assert_eq!(unique_path(&a, "x.png"), a.join("x (2).png"));
        std::fs::write(a.join("noext"), b"1").unwrap();
        assert_eq!(unique_path(&a, "noext"), a.join("noext (2)"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn file_uris_become_paths() {
        assert_eq!(
            file_uri_path("file:///sdcard/Download/a%20b.txt"),
            Some(PathBuf::from("/sdcard/Download/a b.txt"))
        );
        assert_eq!(file_uri_path("content://media/external/images/1"), None);
    }
}
