//! A folder the user chose ([`pick_folder_then`](super::pick_folder_then)),
//! read and written by name: a directory on the desktops and iOS (there
//! through a security-scoped URL, accessed while the [`Folder`] lives), a
//! Storage Access Framework document tree on Android, where there's no path.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A folder the user picked ([`pick_folder_then`](super::pick_folder_then)),
/// or one at a known path ([`Folder::new`]): its files listed, read and
/// written by name, and its subfolders opened, the same way on every
/// platform.
///
/// On the desktops and iOS it's a directory, with a [`path`](Folder::path)
/// (on iOS one outside the app's sandbox, reachable while this value or a
/// clone or subfolder of it lives: the picked URL's security-scoped access
/// is started when it's picked and stopped when the last goes). On Android
/// it's a document tree (`ACTION_OPEN_DOCUMENT_TREE`) with a `content:`
/// [`uri`](Folder::uri) and no path, read and written through the
/// `ContentResolver` (`DocumentsContract`). The access lasts while the app
/// runs; it isn't kept for the next launch yet.
///
/// The calls block (a cloud provider may download first): call them off
/// the UI thread for large folders. A `Folder` is `Send` and `Sync`.
///
/// ```no_run
/// use stipple::platform::dialog::{self, FileDialog};
/// dialog::pick_folder_then(&FileDialog::new(), |folder| {
///     let Some(folder) = folder else { return };
///     for entry in folder.entries().unwrap_or_default() {
///         println!("{} {}", entry.name, if entry.is_dir { "(folder)" } else { "" });
///     }
///     folder.write("notes.txt", b"hello").ok();
/// });
/// ```
#[derive(Clone)]
pub struct Folder {
    inner: Arc<Inner>,
}

struct Inner {
    name: String,
    at: At,
    /// The folder this one was opened from: kept alive, as it may hold the
    /// access (iOS's security scope, Android's tree).
    _parent: Option<Folder>,
    /// iOS: the picked URL, its security-scoped access stopped when dropped.
    #[cfg(target_os = "ios")]
    _scope: Option<super::picker_ios::Scope>,
}

enum At {
    Path(PathBuf),
    #[cfg(target_os = "android")]
    Document {
        tree: Arc<crate::platform::backend::android::DocumentTree>,
        id: String,
    },
}

/// One thing in a [`Folder`]: a file or a subfolder.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderEntry {
    /// Its name in the folder.
    pub name: String,
    /// Whether it's a folder ([`Folder::folder`] opens it).
    pub is_dir: bool,
    /// A file's size in bytes, when known.
    pub size: Option<u64>,
}

impl std::fmt::Debug for Folder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("Folder");
        d.field("name", &self.inner.name);
        if let Some(p) = self.path() {
            d.field("path", &p);
        }
        if let Some(u) = self.uri() {
            d.field("uri", &u);
        }
        d.finish()
    }
}

/// A name for something directly in a folder: not empty, no separator, not
/// `.` or `..`.
pub(crate) fn check_name(name: &str) -> io::Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name:?} isn't a name in a folder"),
        ));
    }
    Ok(())
}

fn not_found(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("no {name:?} in the folder"),
    )
}

impl Folder {
    /// The directory at `path` as a `Folder` (on Android too, for one the
    /// app can reach by path, such as its own data directory).
    pub fn new(path: impl Into<PathBuf>) -> Folder {
        let path = path.into();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        Folder::make(name, At::Path(path), None)
    }

    fn make(name: String, at: At, parent: Option<Folder>) -> Folder {
        Folder {
            inner: Arc::new(Inner {
                name,
                at,
                _parent: parent,
                #[cfg(target_os = "ios")]
                _scope: None,
            }),
        }
    }

    /// iOS: the picked directory, with its URL's security-scoped access.
    #[cfg(target_os = "ios")]
    pub(crate) fn scoped(path: PathBuf, scope: super::picker_ios::Scope) -> Folder {
        let mut f = Folder::new(path);
        if let Some(inner) = Arc::get_mut(&mut f.inner) {
            inner._scope = Some(scope);
        }
        f
    }

    /// Android: the root of a picked document tree.
    #[cfg(target_os = "android")]
    pub(crate) fn tree(
        name: String,
        tree: Arc<crate::platform::backend::android::DocumentTree>,
        id: String,
    ) -> Folder {
        Folder::make(name, At::Document { tree, id }, None)
    }

    /// The folder's name, as the user saw it in the picker.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Where it is, on the desktops and iOS; `None` on Android, where a
    /// picked folder has no path (see [`uri`](Folder::uri)).
    pub fn path(&self) -> Option<&Path> {
        match &self.inner.at {
            At::Path(p) => Some(p),
            #[cfg(target_os = "android")]
            At::Document { .. } => None,
        }
    }

    /// Android: the folder's `content:` document URI (for the platform's
    /// own APIs, through [`platform::android`](crate::platform::android));
    /// `None` elsewhere.
    pub fn uri(&self) -> Option<String> {
        match &self.inner.at {
            At::Path(_) => None,
            #[cfg(target_os = "android")]
            At::Document { tree, id } => tree.uri(id),
        }
    }

    /// What's directly in the folder, sorted by name.
    pub fn entries(&self) -> io::Result<Vec<FolderEntry>> {
        let mut out = match &self.inner.at {
            At::Path(p) => {
                let mut out = Vec::new();
                for e in std::fs::read_dir(p)? {
                    let e = e?;
                    let Ok(name) = e.file_name().into_string() else {
                        continue;
                    };
                    // Through symlinks, as opening it does.
                    let meta = std::fs::metadata(e.path()).or_else(|_| e.metadata())?;
                    out.push(FolderEntry {
                        name,
                        is_dir: meta.is_dir(),
                        size: (!meta.is_dir()).then_some(meta.len()),
                    });
                }
                out
            }
            #[cfg(target_os = "android")]
            At::Document { tree, id } => tree
                .children(id)?
                .into_iter()
                .map(|c| FolderEntry {
                    name: c.name,
                    is_dir: c.is_dir,
                    size: c.size,
                })
                .collect(),
        };
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// The whole of the file `name` in the folder.
    pub fn read(&self, name: &str) -> io::Result<Vec<u8>> {
        check_name(name)?;
        match &self.inner.at {
            At::Path(p) => std::fs::read(p.join(name)),
            #[cfg(target_os = "android")]
            At::Document { tree, id } => {
                let child = tree
                    .children(id)?
                    .into_iter()
                    .find(|c| c.name == name && !c.is_dir)
                    .ok_or_else(|| not_found(name))?;
                tree.read(&child.id)
            }
        }
    }

    /// Write `bytes` to the file `name` in the folder, making it or
    /// replacing what it held. (On Android a provider may give a new file
    /// another name when the type it guesses from the extension wants
    /// another one.)
    pub fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        check_name(name)?;
        match &self.inner.at {
            At::Path(p) => std::fs::write(p.join(name), bytes),
            #[cfg(target_os = "android")]
            At::Document { tree, id } => {
                let existing = tree.children(id)?.into_iter().find(|c| c.name == name);
                match existing {
                    Some(c) if c.is_dir => Err(io::Error::new(
                        io::ErrorKind::IsADirectory,
                        format!("{name:?} is a folder"),
                    )),
                    Some(c) => tree.write(&c.id, bytes),
                    None => {
                        let mime = super::mobile::mime_for_name(name);
                        let child = tree.create(id, name, mime)?;
                        tree.write(&child, bytes)
                    }
                }
            }
        }
    }

    /// The subfolder `name`, which must be there.
    pub fn folder(&self, name: &str) -> io::Result<Folder> {
        check_name(name)?;
        let at = match &self.inner.at {
            At::Path(p) => {
                let path = p.join(name);
                if !std::fs::metadata(&path)?.is_dir() {
                    return Err(not_found(name));
                }
                At::Path(path)
            }
            #[cfg(target_os = "android")]
            At::Document { tree, id } => {
                let child = tree
                    .children(id)?
                    .into_iter()
                    .find(|c| c.name == name && c.is_dir)
                    .ok_or_else(|| not_found(name))?;
                At::Document {
                    tree: tree.clone(),
                    id: child.id,
                }
            }
        };
        Ok(Folder::make(name.into(), at, Some(self.clone())))
    }

    /// Make the subfolder `name` (or open it, if it's already there).
    pub fn create_folder(&self, name: &str) -> io::Result<Folder> {
        check_name(name)?;
        match &self.inner.at {
            At::Path(p) => match std::fs::create_dir(p.join(name)) {
                Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
                _ => {}
            },
            #[cfg(target_os = "android")]
            At::Document { tree, id } => {
                if !tree
                    .children(id)?
                    .iter()
                    .any(|c| c.name == name && c.is_dir)
                {
                    let made = tree.create(id, name, "vnd.android.document/directory")?;
                    return Ok(Folder::make(
                        name.into(),
                        At::Document {
                            tree: tree.clone(),
                            id: made,
                        },
                        Some(self.clone()),
                    ));
                }
            }
        }
        self.folder(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_in_the_folder() {
        for bad in ["", ".", "..", "a/b", "a\\b", "a\0"] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        for good in ["a", "notes.txt", ".hidden", "a b", "…"] {
            assert!(check_name(good).is_ok(), "{good:?}");
        }
    }

    #[test]
    fn a_directory_is_listed_read_and_written() {
        let dir = std::env::temp_dir().join(format!("stipple-folder-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("b.txt"), "bee").unwrap();
        let f = Folder::new(&dir);
        assert_eq!(f.path(), Some(dir.as_path()));
        assert!(f.uri().is_none());
        assert!(f.name().starts_with("stipple-folder-test-"));
        f.write("a.txt", b"hello").unwrap();
        assert_eq!(f.read("a.txt").unwrap(), b"hello");
        f.write("a.txt", b"hi").unwrap();
        assert_eq!(f.read("a.txt").unwrap(), b"hi");
        assert_eq!(
            f.entries().unwrap(),
            [
                FolderEntry {
                    name: "a.txt".into(),
                    is_dir: false,
                    size: Some(2)
                },
                FolderEntry {
                    name: "b.txt".into(),
                    is_dir: false,
                    size: Some(3)
                },
                FolderEntry {
                    name: "sub".into(),
                    is_dir: true,
                    size: None
                },
            ]
        );
        assert!(f.read("../x").is_err());
        assert_eq!(f.read("nope").unwrap_err().kind(), io::ErrorKind::NotFound);
        let sub = f.folder("sub").unwrap();
        assert_eq!(sub.name(), "sub");
        sub.write("c", b"see").unwrap();
        assert_eq!(std::fs::read(dir.join("sub/c")).unwrap(), b"see");
        assert!(f.folder("b.txt").is_err());
        let made = f.create_folder("made").unwrap();
        assert!(made.entries().unwrap().is_empty());
        assert!(f.create_folder("made").is_ok(), "already there");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
