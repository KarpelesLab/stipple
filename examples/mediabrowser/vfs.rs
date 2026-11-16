//! Where the browser's files come from: a folder on disk, or a sample tree
//! held in memory (the web build, where there is no file system, and tests).
//! Paths are relative to the root, `/`-separated, `""` for the root itself.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// What a file is, from its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Folder,
    Image,
    Audio,
    Markdown,
    Text,
    Other,
}

impl Kind {
    pub fn of(name: &str) -> Kind {
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => Kind::Image,
            "wav" | "ogg" | "oga" | "mp3" | "flac" | "opus" => Kind::Audio,
            "md" | "markdown" => Kind::Markdown,
            "txt" | "text" | "log" | "csv" | "json" | "toml" | "rs" | "ini" | "xml" => Kind::Text,
            _ => Kind::Other,
        }
    }

    /// The catalog key of the kind's name.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Folder => "kind.folder",
            Kind::Image => "kind.image",
            Kind::Audio => "kind.audio",
            Kind::Markdown => "kind.markdown",
            Kind::Text => "kind.text",
            Kind::Other => "kind.other",
        }
    }
}

/// One entry of a folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    /// From the root, `/`-separated.
    pub path: String,
    pub kind: Kind,
    /// Bytes (0 for folders).
    pub size: u64,
    /// Seconds since 1970, UTC.
    pub modified: i64,
}

/// A file in the in-memory tree.
#[derive(Debug)]
struct MemFile {
    bytes: Arc<[u8]>,
    modified: i64,
}

/// The in-memory tree: files by path; folders are implied by the paths
/// (and kept when empty).
#[derive(Debug, Default)]
pub struct MemFs {
    files: Mutex<BTreeMap<String, MemFile>>,
    folders: Mutex<Vec<String>>,
}

impl MemFs {
    pub fn new(files: Vec<(String, Vec<u8>, i64)>) -> Self {
        let fs = MemFs::default();
        for (path, bytes, modified) in files {
            fs.insert(&path, bytes, modified);
        }
        fs
    }

    fn insert(&self, path: &str, bytes: Vec<u8>, modified: i64) {
        let mut folders = self.folders.lock().unwrap();
        let mut at = path;
        while let Some((parent, _)) = at.rsplit_once('/') {
            if !folders.iter().any(|f| f == parent) {
                folders.push(parent.to_string());
            }
            at = parent;
        }
        drop(folders);
        self.files.lock().unwrap().insert(
            path.to_string(),
            MemFile {
                bytes: bytes.into(),
                modified,
            },
        );
    }
}

/// The file system the browser shows. Cheap to clone; the loader's workers
/// hold clones.
#[derive(Clone, Debug)]
pub enum Vfs {
    Disk(PathBuf),
    Memory(Arc<MemFs>),
}

/// `parent/name`, or `name` at the root.
pub fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// The folder holding `path` (`""` for the root's entries); `None` for the
/// root.
pub fn parent(path: &str) -> Option<&str> {
    if path.is_empty() {
        None
    } else {
        Some(path.rsplit_once('/').map_or("", |(p, _)| p))
    }
}

/// The last component of `path`.
pub fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, n)| n)
}

impl Vfs {
    /// The root's name, for the tree's first node and the window title.
    pub fn root_name(&self) -> String {
        match self {
            Vfs::Disk(root) => root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string()),
            Vfs::Memory(_) => "Samples".to_string(),
        }
    }

    /// Where `path` is on disk, if it's on disk.
    pub fn real_path(&self, path: &str) -> Option<PathBuf> {
        match self {
            Vfs::Disk(root) => Some(if path.is_empty() {
                root.clone()
            } else {
                path.split('/').fold(root.clone(), |p, c| p.join(c))
            }),
            Vfs::Memory(_) => None,
        }
    }

    /// What to show for "Copy path": the real path, or the path in the tree.
    pub fn display_path(&self, path: &str) -> String {
        match self.real_path(path) {
            Some(p) => p.display().to_string(),
            None => format!("/{path}"),
        }
    }

    /// The entries of folder `dir`, unsorted. Hidden (dot) files are left out.
    pub fn list(&self, dir: &str) -> Result<Vec<Entry>, String> {
        match self {
            Vfs::Disk(_) => {
                let real = self.real_path(dir).ok_or("no path")?;
                let mut out = Vec::new();
                for item in std::fs::read_dir(&real).map_err(|e| e.to_string())? {
                    let Ok(item) = item else { continue };
                    let name = item.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        continue;
                    }
                    let Ok(meta) = item.metadata() else { continue };
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs() as i64);
                    let kind = if meta.is_dir() {
                        Kind::Folder
                    } else {
                        Kind::of(&name)
                    };
                    out.push(Entry {
                        path: join(dir, &name),
                        size: if meta.is_dir() { 0 } else { meta.len() },
                        name,
                        kind,
                        modified,
                    });
                }
                Ok(out)
            }
            Vfs::Memory(fs) => {
                let prefix = if dir.is_empty() {
                    String::new()
                } else {
                    format!("{dir}/")
                };
                let direct = |p: &str| {
                    p.strip_prefix(&prefix)
                        .filter(|rest| !rest.is_empty() && !rest.contains('/'))
                        .map(str::to_string)
                };
                let mut out = Vec::new();
                for folder in fs.folders.lock().unwrap().iter() {
                    if let Some(name) = direct(folder) {
                        out.push(Entry {
                            path: folder.clone(),
                            name,
                            kind: Kind::Folder,
                            size: 0,
                            modified: 1_794_873_600,
                        });
                    }
                }
                for (path, file) in fs.files.lock().unwrap().iter() {
                    if let Some(name) = direct(path) {
                        out.push(Entry {
                            path: path.clone(),
                            kind: Kind::of(&name),
                            name,
                            size: file.bytes.len() as u64,
                            modified: file.modified,
                        });
                    }
                }
                Ok(out)
            }
        }
    }

    /// The bytes of file `path`.
    pub fn read(&self, path: &str) -> Result<Arc<[u8]>, String> {
        match self {
            Vfs::Disk(_) => {
                let real = self.real_path(path).ok_or("no path")?;
                std::fs::read(real)
                    .map(Into::into)
                    .map_err(|e| e.to_string())
            }
            Vfs::Memory(fs) => fs
                .files
                .lock()
                .unwrap()
                .get(path)
                .map(|f| f.bytes.clone())
                .ok_or_else(|| format!("{path}: not found")),
        }
    }

    /// Copy files dropped on the window into folder `dir`; returns how many
    /// were copied. Folders are skipped, and a name already taken gets a
    /// number.
    pub fn copy_in(&self, dir: &str, files: &[PathBuf]) -> usize {
        let mut copied = 0;
        for src in files {
            let Ok(bytes) = std::fs::read(src) else {
                continue;
            };
            let Some(name) = src.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            let taken = self.list(dir).unwrap_or_default();
            let mut target = name.clone();
            let mut n = 2;
            while taken.iter().any(|e| e.name == target) {
                target = match name.rsplit_once('.') {
                    Some((stem, ext)) => format!("{stem} ({n}).{ext}"),
                    None => format!("{name} ({n})"),
                };
                n += 1;
            }
            let ok = match self {
                Vfs::Disk(_) => self
                    .real_path(&join(dir, &target))
                    .is_some_and(|p| std::fs::write(p, &bytes).is_ok()),
                Vfs::Memory(fs) => {
                    fs.insert(&join(dir, &target), bytes, 1_794_873_600);
                    true
                }
            };
            copied += usize::from(ok);
        }
        copied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_folders_list_their_direct_entries() {
        let fs = Vfs::Memory(Arc::new(MemFs::new(vec![
            ("a/b/c.txt".into(), b"hi".to_vec(), 0),
            ("a/d.png".into(), vec![0; 10], 5),
            ("e.md".into(), vec![], 0),
        ])));
        let mut root: Vec<_> = fs.list("").unwrap().into_iter().map(|e| e.name).collect();
        root.sort();
        assert_eq!(root, vec!["a", "e.md"]);
        let a = fs.list("a").unwrap();
        assert_eq!(a.len(), 2);
        let png = a.iter().find(|e| e.name == "d.png").unwrap();
        assert_eq!((png.kind, png.size, png.modified), (Kind::Image, 10, 5));
        assert_eq!(&*fs.read("a/b/c.txt").unwrap(), b"hi");
        assert_eq!(parent("a/b/c.txt"), Some("a/b"));
        assert_eq!(parent("e.md"), Some(""));
        assert_eq!(parent(""), None);
    }
}
