//! Dragging files out of the app on iOS and Android: the parts that don't
//! need the system, so they're unit-tested on every host.
//!
//! Android hands other apps a file as a `content:` URI of the app's own
//! provider (`org.stipple.android.Files`, declared in the manifest with the
//! authority `<package>.stipple.files`, not exported, granting URI
//! permissions): `content://<authority>/root/<the absolute path,
//! percent-encoded>`. The provider is stateless (the URI names the file), so
//! a target may read it after the app's process was restarted; only the URIs
//! a drag grants are readable at all. Its type comes from the file's
//! extension ([`mime_for_name`]).

use std::path::{Path, PathBuf};

/// What the app's provider authority is called after its package.
pub(crate) const AUTHORITY_SUFFIX: &str = ".stipple.files";

/// The first path segment of the provider's URIs.
const ROOT: &str = "/root";

/// The provider's authority for the app's package `package`.
pub(crate) fn authority(package: &str) -> String {
    format!("{package}{AUTHORITY_SUFFIX}")
}

/// The `content:` URI under `authority` for the absolute path `path`.
/// `None` for a relative path.
pub(crate) fn content_uri(authority: &str, path: &Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    let file = crate::platform::path_to_file_uri(path);
    let encoded = file.strip_prefix("file://")?;
    Some(format!("content://{authority}{ROOT}{encoded}"))
}

/// The file a provider URI's decoded path (`Uri.getPath()`) names, if it
/// is one of [`content_uri`]'s: under the root, absolute, and without `..`
/// or `.` segments.
pub(crate) fn uri_path(decoded: &str) -> Option<PathBuf> {
    let path = decoded.strip_prefix(ROOT)?;
    if !path.starts_with('/') || path.len() < 2 {
        return None;
    }
    if path.split('/').any(|s| s == ".." || s == ".") {
        return None;
    }
    Some(PathBuf::from(path))
}

/// The MIME type a file named `name` is offered as, from its extension.
pub(crate) fn mime_for_name(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "txt" | "text" | "log" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

/// A file's display name: its last path component.
pub(crate) fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_name_the_file_and_come_back() {
        let auth = authority("com.example.app");
        assert_eq!(auth, "com.example.app.stipple.files");
        let path = Path::new("/data/user/0/com.example.app/cache/a b%.txt");
        let uri = content_uri(&auth, path).unwrap();
        assert_eq!(
            uri,
            "content://com.example.app.stipple.files/root/data/user/0/com.example.app/cache/a%20b%25.txt"
        );
        // Android decodes the path (`Uri.getPath()`) before the provider reads it.
        assert_eq!(
            uri_path("/root/data/user/0/com.example.app/cache/a b%.txt").as_deref(),
            Some(path)
        );
        assert_eq!(content_uri(&auth, Path::new("relative.txt")), None);
    }

    #[test]
    fn only_the_providers_own_paths_resolve() {
        assert_eq!(uri_path("/data/x"), None, "not under the root");
        assert_eq!(uri_path("/root"), None);
        assert_eq!(uri_path("/root/"), None);
        assert_eq!(uri_path("/rootx/a"), None);
        assert_eq!(uri_path("/root/data/../etc/passwd"), None);
        assert_eq!(uri_path("/root/./a"), None);
        assert_eq!(uri_path("/root/a/b").as_deref(), Some(Path::new("/a/b")));
    }

    #[test]
    fn types_come_from_extensions() {
        assert_eq!(mime_for_name("notes.TXT"), "text/plain");
        assert_eq!(mime_for_name("photo.jpeg"), "image/jpeg");
        assert_eq!(mime_for_name("doc.pdf"), "application/pdf");
        assert_eq!(mime_for_name("archive"), "application/octet-stream");
        assert_eq!(mime_for_name("x.unknownext"), "application/octet-stream");
        assert_eq!(display_name(Path::new("/a/b/c.txt")), "c.txt");
    }
}
