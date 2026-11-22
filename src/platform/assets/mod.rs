//! Files the app was packaged with: the APK's assets, the bundle's
//! resources, a directory next to the executable, the web server's. See
//! [`Assets`] and `docs/os-integration.md`.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Mutex;

#[cfg(target_os = "android")]
#[allow(unsafe_code)]
mod android;

#[cfg(target_arch = "wasm32")]
pub(crate) mod web;

/// Why a packaged file couldn't be read.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetError {
    /// The path isn't a relative, `/`-separated path inside the assets
    /// (empty, absolute, with `..`, `.`, `\`, `:` or an empty component).
    InvalidPath(String),
    /// There's no such file among the assets.
    NotFound(String),
    /// On the web: a blocking read of a file the page didn't preload. Use
    /// [`Assets::load`], or list the file in `stipple.js`'s `preload`.
    NotPreloaded(String),
    /// The app has nowhere to read assets from: no assets directory on the
    /// desktop, no activity yet on Android. Says where it looked.
    Unavailable(String),
    /// [`Assets::open_fd`]: the asset is in the APK compressed, so it isn't
    /// a range of a file. Store it uncompressed (`cargo-stipple` stores
    /// images, sounds and video; `aapt -0 <ext>`, `noCompress` in Gradle),
    /// or read it with [`Assets::open`].
    Compressed(String),
    /// [`Assets::open_fd`] on the web, where assets aren't files.
    NoFile(String),
    /// The file is there but couldn't be read.
    Io {
        /// The asset's path.
        path: String,
        /// What went wrong.
        message: String,
    },
    /// The file was read, but isn't what it was asked for as (an image that
    /// doesn't decode, text that isn't UTF-8).
    Decode {
        /// The asset's path.
        path: String,
        /// What went wrong.
        message: String,
    },
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetError::InvalidPath(p) => write!(f, "{p:?} isn't a valid asset path"),
            AssetError::NotFound(p) => write!(f, "no asset {p:?}"),
            AssetError::NotPreloaded(p) => {
                write!(
                    f,
                    "asset {p:?} wasn't preloaded (on the web, load it first)"
                )
            }
            AssetError::Unavailable(why) => write!(f, "no assets: {why}"),
            AssetError::Compressed(p) => {
                write!(
                    f,
                    "asset {p:?} is compressed in the package: it isn't a file range"
                )
            }
            AssetError::NoFile(p) => write!(f, "asset {p:?} isn't in a file here"),
            AssetError::Io { path, message } => write!(f, "asset {path:?}: {message}"),
            AssetError::Decode { path, message } => {
                write!(f, "asset {path:?} doesn't decode: {message}")
            }
        }
    }
}

impl std::error::Error for AssetError {}

impl AssetError {
    fn decode(path: &str, e: impl std::fmt::Display) -> Self {
        AssetError::Decode {
            path: path.into(),
            message: e.to_string(),
        }
    }

    #[cfg_attr(any(target_os = "android", target_arch = "wasm32"), allow(dead_code))]
    fn io(path: &str, e: &std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::NotFound {
            return AssetError::NotFound(path.into());
        }
        AssetError::Io {
            path: path.into(),
            message: e.to_string(),
        }
    }
}

/// The app's packaged files ([`assets()`] returns it): images, fonts,
/// sounds, levels. They are read by a relative, `/`-separated path (`"art/sun.png"`) from wherever
/// the platform keeps them, so the same code finds them in an APK, an app
/// bundle, next to a desktop executable, on the web server, and in the
/// source tree during `cargo run`.
///
/// | | Where | How |
/// |---|---|---|
/// | Android | the APK's `assets/` | `AAssetManager_open`, `AAsset_read` |
/// | iOS | `<App>.app/assets/` | the main bundle's `resourcePath` |
/// | macOS | `<App>.app/Contents/Resources/assets/`, else `assets/` next to the executable | the main bundle's `resourcePath` |
/// | Windows, Linux | `assets/` next to the executable | the file system |
/// | Web | `assets/` next to the page (`stipple.js`'s `assets` option) | `fetch`, so asynchronous: see below |
///
/// Off Android and the web, `$STIPPLE_ASSETS` names the directory instead,
/// and when neither it nor the packaged directory exists, the one given to
/// [`set_assets_dir`] is used: an app passes its source tree's
/// (`concat!(env!("CARGO_MANIFEST_DIR"), "/assets")`) so `cargo run` and
/// `cargo test` find the files where they are.
///
/// On the web every read is a `fetch`, so it can't return at once:
/// [`Assets::load`] (and [`AppHandle::load_asset`](crate::core::AppHandle::load_asset),
/// which hands the result to the state) works everywhere, and the blocking
/// calls ([`Assets::read`], [`open`](Assets::open), [`image`](Assets::image)…)
/// work on the web only for files the page preloaded before the app started
/// (`stipple.js`'s `preload` option); others fail with
/// [`AssetError::NotPreloaded`]. See `docs/os-integration.md`.
///
/// ```no_run
/// use stipple::platform::assets;
/// let sun = assets().image("sun.png")?;
/// let intro = assets().text("story/intro.txt")?;
/// # Ok::<(), stipple::platform::AssetError>(())
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct Assets {
    _private: (),
}

/// The app's packaged files: see [`Assets`].
pub fn assets() -> Assets {
    Assets { _private: () }
}

/// The directory given by [`set_assets_dir`].
static GIVEN: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Where to read assets from when the app isn't packaged (no `assets`
/// directory in the bundle or next to the executable, and no
/// `$STIPPLE_ASSETS`): typically the source tree's, so `cargo run` and
/// `cargo test` work from a checkout. Packaged files win, so a shipped app
/// never reads from here. Ignored on Android and the web, whose assets are
/// always packaged.
///
/// ```no_run
/// stipple::platform::set_assets_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"));
/// ```
pub fn set_assets_dir(dir: impl Into<PathBuf>) {
    *GIVEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.into());
}

/// `path` checked as an asset path: relative, `/`-separated, inside the
/// assets. With `dir`, the empty path (the top) is allowed and a trailing
/// `/` dropped.
fn clean(path: &str, dir: bool) -> Result<&str, AssetError> {
    let invalid = || AssetError::InvalidPath(path.into());
    let p = if dir {
        path.strip_suffix('/').unwrap_or(path)
    } else {
        path
    };
    if p.is_empty() {
        return if dir { Ok(p) } else { Err(invalid()) };
    }
    if p.contains(['\\', ':', '\0']) {
        return Err(invalid());
    }
    if p.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
        return Err(invalid());
    }
    Ok(p)
}

/// The directories a desktop or iOS app reads assets from, in order:
/// `$STIPPLE_ASSETS`, the packaged directory, the one given.
#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(env) = std::env::var_os("STIPPLE_ASSETS").filter(|v| !v.is_empty()) {
        out.push(PathBuf::from(env));
    }
    if let Some(dir) = packaged() {
        out.push(dir);
    }
    if let Some(dir) = GIVEN.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        out.push(dir);
    }
    out
}

/// `assets` in the main bundle's resource directory: `Contents/Resources`
/// of a macOS bundle, the `.app` itself on iOS, the executable's directory
/// for a bare binary.
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn packaged() -> Option<PathBuf> {
    crate::platform::bundle::resource_dir().map(|d| d.join("assets"))
}

/// `assets` next to the executable.
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_arch = "wasm32"
)))]
fn packaged() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("assets"))
}

/// The first of the [`candidates`] that is a directory.
#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
fn root() -> Result<PathBuf, AssetError> {
    let all = candidates();
    if let Some(dir) = all.iter().find(|d| d.is_dir()) {
        return Ok(dir.clone());
    }
    let looked: Vec<String> = all.iter().map(|d| d.display().to_string()).collect();
    Err(AssetError::Unavailable(if looked.is_empty() {
        "no assets directory".into()
    } else {
        format!("no assets directory (looked in {})", looked.join(", "))
    }))
}

impl Assets {
    /// The directory assets are read from, where they are files: `None` on
    /// Android (they're in the APK) and the web, and when there's none.
    pub fn dir(&self) -> Option<PathBuf> {
        #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
        {
            root().ok()
        }
        #[cfg(any(target_os = "android", target_arch = "wasm32"))]
        {
            None
        }
    }

    /// The whole of the asset at `path`. On the web, only a preloaded one.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, AssetError> {
        let p = clean(path, false)?;
        #[cfg(target_os = "android")]
        {
            android::read(p)
        }
        #[cfg(target_arch = "wasm32")]
        {
            web::read(p)
        }
        #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
        {
            std::fs::read(root()?.join(p)).map_err(|e| AssetError::io(p, &e))
        }
    }

    /// The asset at `path` as a reader, which reads (and seeks) as it goes
    /// rather than all at once: for a large file read in pieces. On the web,
    /// only a preloaded one, read from memory.
    pub fn open(&self, path: &str) -> Result<AssetReader, AssetError> {
        let p = clean(path, false)?;
        #[cfg(target_os = "android")]
        {
            android::open(p).map(|a| AssetReader {
                inner: Inner::Android(a),
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            web::read(p).map(|bytes| AssetReader {
                inner: Inner::Memory(std::io::Cursor::new(bytes)),
            })
        }
        #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
        {
            let file = std::fs::File::open(root()?.join(p)).map_err(|e| AssetError::io(p, &e))?;
            if file.metadata().is_ok_and(|m| m.is_dir()) {
                return Err(AssetError::NotFound(p.into()));
            }
            Ok(AssetReader {
                inner: Inner::File(file),
            })
        }
    }

    /// The asset at `path` as a range of an open file, for code that wants a
    /// file descriptor rather than bytes: a media player
    /// (`MediaPlayer.setDataSource(fd, offset, length)`, `AMediaExtractor_setDataSourceFd`),
    /// `mmap`, a decoder in another library.
    ///
    /// On Android the file is the APK itself and the asset a range of it
    /// (`AAsset_openFileDescriptor64`), which only an asset stored
    /// uncompressed has ([`AssetError::Compressed`] otherwise; `cargo-stipple`
    /// stores images, sounds and video uncompressed). Elsewhere the asset is
    /// a file of its own: offset 0, its whole length, and its
    /// [`path`](AssetFd::path). On the web there's no file
    /// ([`AssetError::NoFile`]).
    pub fn open_fd(&self, path: &str) -> Result<AssetFd, AssetError> {
        let p = clean(path, false)?;
        #[cfg(target_os = "android")]
        {
            android::open_fd(p).map(|(file, offset, len)| AssetFd {
                file,
                offset,
                len,
                path: None,
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            Err(AssetError::NoFile(p.into()))
        }
        #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
        {
            let full = root()?.join(p);
            let file = std::fs::File::open(&full).map_err(|e| AssetError::io(p, &e))?;
            let meta = file.metadata().map_err(|e| AssetError::io(p, &e))?;
            if meta.is_dir() {
                return Err(AssetError::NotFound(p.into()));
            }
            Ok(AssetFd {
                file,
                offset: 0,
                len: meta.len(),
                path: Some(full),
            })
        }
    }

    /// Whether there's an asset (a file, not a directory) at `path`. On the
    /// web, whether one was preloaded there.
    pub fn exists(&self, path: &str) -> bool {
        let Ok(p) = clean(path, false) else {
            return false;
        };
        #[cfg(target_os = "android")]
        {
            android::exists(p)
        }
        #[cfg(target_arch = "wasm32")]
        {
            web::exists(p)
        }
        #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
        {
            root().is_ok_and(|r| r.join(p).is_file())
        }
    }

    /// The names of the files directly in the directory `dir` (`""` for
    /// the top), sorted; not its subdirectories, which Android can't list.
    /// On the web, the preloaded ones. A directory that isn't there lists
    /// nothing.
    pub fn list(&self, dir: &str) -> Result<Vec<String>, AssetError> {
        let d = clean(dir, true)?;
        #[cfg(target_os = "android")]
        let mut names = android::list(d)?;
        #[cfg(target_arch = "wasm32")]
        let mut names = web::list(d);
        #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
        let mut names = {
            let path = root()?.join(d);
            match std::fs::read_dir(&path) {
                Ok(entries) => entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect::<Vec<_>>(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(AssetError::io(d, &e)),
            }
        };
        names.sort();
        Ok(names)
    }

    /// The asset at `path` as UTF-8 text.
    pub fn text(&self, path: &str) -> Result<String, AssetError> {
        String::from_utf8(self.read(path)?).map_err(|e| AssetError::decode(path, e))
    }

    /// The image at `path`, decoded ([`Pixmap::decode`](crate::render::Pixmap::decode)).
    pub fn image(&self, path: &str) -> Result<crate::render::Pixmap, AssetError> {
        crate::render::Pixmap::decode(&self.read(path)?).map_err(|e| AssetError::decode(path, e))
    }

    /// The SVG at `path`, parsed ([`Svg::parse`](crate::render::Svg::parse)).
    pub fn svg(&self, path: &str) -> Result<crate::render::Svg, AssetError> {
        crate::render::Svg::parse(&self.read(path)?).map_err(|e| AssetError::decode(path, e))
    }

    /// The font at `path` ([`Font::from_bytes`](crate::render::Font::from_bytes)).
    pub fn font(&self, path: &str) -> Result<crate::render::Font, AssetError> {
        crate::render::Font::from_bytes(self.read(path)?).map_err(|e| AssetError::decode(path, e))
    }

    /// The sound at `path`, decoded whole
    /// ([`Sound::decode`](crate::audio::Sound::decode)): for effects. Music
    /// streams from its encoded bytes instead:
    /// `audio.play_music(assets().read("theme.ogg")?, params)`.
    pub fn sound(&self, path: &str) -> Result<crate::audio::Sound, AssetError> {
        crate::audio::Sound::decode(&self.read(path)?).map_err(|e| AssetError::decode(path, e))
    }

    /// Read the asset at `path` without blocking, and hand its bytes to
    /// `done`: on a worker thread natively, on the page's thread on the web
    /// (once its `fetch` is done). It works for every asset on every
    /// platform; a preloaded one on the web is handed over before this
    /// returns. To decode on the worker too and put the result in the
    /// state, use [`AppHandle::load_asset`](crate::core::AppHandle::load_asset).
    pub fn load(
        &self,
        path: &str,
        done: impl FnOnce(Result<Vec<u8>, AssetError>) + Send + 'static,
    ) {
        let p = match clean(path, false) {
            Ok(p) => p.to_owned(),
            Err(e) => return done(Err(e)),
        };
        #[cfg(target_arch = "wasm32")]
        {
            web::load(p, Box::new(done));
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            worker::run(Box::new(move || done(assets().read(&p))));
        }
    }
}

/// The threads [`Assets::load`] reads (and decodes) on: a few, shared, made
/// as they're first needed.
#[cfg(not(target_arch = "wasm32"))]
mod worker {
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex, OnceLock};

    type Job = Box<dyn FnOnce() + Send>;

    pub(super) fn run(job: Job) {
        static QUEUE: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
        let queue = QUEUE.get_or_init(|| {
            let (tx, rx) = channel::<Job>();
            let rx = Arc::new(Mutex::new(rx));
            let n = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(1, 4));
            for i in 0..n {
                let rx = rx.clone();
                let _ = std::thread::Builder::new()
                    .name(format!("stipple-assets-{i}"))
                    .spawn(move || work(&rx));
            }
            Mutex::new(tx)
        });
        let sent = queue.lock().unwrap_or_else(|e| e.into_inner()).send(job);
        if let Err(e) = sent {
            // No worker (threads couldn't start): do it here.
            (e.0)();
        }
    }

    fn work(rx: &Mutex<Receiver<Job>>) {
        loop {
            let job = match rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
                Ok(job) => job,
                Err(_) => return,
            };
            job();
        }
    }
}

/// An asset as a range of an open file ([`Assets::open_fd`]): the bytes
/// from [`offset`](AssetFd::offset), [`len`](AssetFd::len) of them. On
/// Android the file is the whole APK, so read only that range (`pread`, or
/// hand the descriptor, offset and length on); elsewhere it's the asset's
/// own file, from 0.
#[derive(Debug)]
pub struct AssetFd {
    file: std::fs::File,
    offset: u64,
    len: u64,
    path: Option<PathBuf>,
}

impl AssetFd {
    /// The open file (on Unix, `as_raw_fd` gives its descriptor).
    pub fn file(&self) -> &std::fs::File {
        &self.file
    }

    /// The file, to own (it closes when dropped).
    pub fn into_file(self) -> std::fs::File {
        self.file
    }

    /// Where the asset starts in the file.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// The asset's length, in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether the asset is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The file's path, where the asset is a file of its own (not on
    /// Android, where it's inside the APK): for an API that takes a path
    /// or URL (`AVPlayer`, a desktop media library).
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// The asset's bytes, read from its range of the file (moves the file's
    /// position).
    pub fn read_all(&self) -> std::io::Result<Vec<u8>> {
        let mut f = &self.file;
        f.seek(SeekFrom::Start(self.offset))?;
        let mut out = Vec::with_capacity(usize::try_from(self.len).unwrap_or(0));
        f.take(self.len).read_to_end(&mut out)?;
        Ok(out)
    }
}

/// An open asset, read as it goes ([`Assets::open`]). It implements
/// [`Read`] and [`Seek`].
pub struct AssetReader {
    inner: Inner,
}

enum Inner {
    #[cfg_attr(any(target_os = "android", target_arch = "wasm32"), allow(dead_code))]
    File(std::fs::File),
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Memory(std::io::Cursor<Vec<u8>>),
    #[cfg(target_os = "android")]
    Android(android::Asset),
}

impl AssetReader {
    /// The asset's size, in bytes.
    pub fn len(&self) -> u64 {
        match &self.inner {
            Inner::File(f) => f.metadata().map_or(0, |m| m.len()),
            Inner::Memory(c) => c.get_ref().len() as u64,
            #[cfg(target_os = "android")]
            Inner::Android(a) => a.len(),
        }
    }

    /// Whether the asset is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for AssetReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssetReader")
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

impl Read for AssetReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            Inner::File(f) => f.read(buf),
            Inner::Memory(c) => c.read(buf),
            #[cfg(target_os = "android")]
            Inner::Android(a) => a.read(buf),
        }
    }
}

impl Seek for AssetReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match &mut self.inner {
            Inner::File(f) => f.seek(pos),
            Inner::Memory(c) => c.seek(pos),
            #[cfg(target_os = "android")]
            Inner::Android(a) => a.seek(pos),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_assets() {
        assert_eq!(clean("sun.png", false), Ok("sun.png"));
        assert_eq!(clean("art/sun.png", false), Ok("art/sun.png"));
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a/./b",
            "./a",
            "a//b",
            "a/",
            "a\\b",
            "C:x",
            "a\0b",
            "..",
        ] {
            assert_eq!(
                clean(bad, false),
                Err(AssetError::InvalidPath(bad.into())),
                "{bad:?}"
            );
        }
        assert_eq!(clean("", true), Ok(""));
        assert_eq!(clean("art/", true), Ok("art"));
        assert_eq!(clean("/", true), Ok(""));
        assert!(clean("../", true).is_err());
    }

    /// The directory, reads, readers, listing and loading, against a
    /// temporary directory given by `set_assets_dir` (one test, since the
    /// setting is the process's). The test binary has no packaged
    /// directory next to it.
    #[test]
    fn reads_from_the_assets_directory() {
        let dir = std::env::temp_dir().join(format!("stipple-assets-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("art/deep")).unwrap();
        std::fs::write(dir.join("hello.txt"), "hello, assets\n").unwrap();
        std::fs::write(dir.join("art/a.bin"), [1u8, 2, 3, 4, 5]).unwrap();
        std::fs::write(dir.join("art/b.bin"), [9u8]).unwrap();
        std::fs::write(dir.join("not-utf8.txt"), [0xffu8, 0xfe]).unwrap();
        set_assets_dir(&dir);
        let a = assets();
        assert_eq!(a.dir().as_deref(), Some(dir.as_path()));
        assert_eq!(a.text("hello.txt").unwrap(), "hello, assets\n");
        assert_eq!(a.read("art/a.bin").unwrap(), [1, 2, 3, 4, 5]);
        assert_eq!(
            a.read("nope.txt"),
            Err(AssetError::NotFound("nope.txt".into()))
        );
        assert!(matches!(
            a.read("../hello.txt"),
            Err(AssetError::InvalidPath(_))
        ));
        assert!(matches!(
            a.text("not-utf8.txt"),
            Err(AssetError::Decode { .. })
        ));
        assert!(matches!(
            a.image("hello.txt"),
            Err(AssetError::Decode { .. })
        ));
        assert!(a.exists("art/b.bin"));
        assert!(!a.exists("art"), "a directory isn't an asset");
        assert!(!a.exists("../hello.txt"));
        assert_eq!(a.list("").unwrap(), ["hello.txt", "not-utf8.txt"]);
        assert_eq!(a.list("art/").unwrap(), ["a.bin", "b.bin"]);
        assert!(a.list("missing").unwrap().is_empty());
        assert!(matches!(a.open("art"), Err(AssetError::NotFound(_))));

        let fd = a.open_fd("art/a.bin").unwrap();
        assert_eq!((fd.offset(), fd.len()), (0, 5));
        assert_eq!(fd.path(), Some(dir.join("art/a.bin").as_path()));
        assert_eq!(fd.read_all().unwrap(), [1, 2, 3, 4, 5]);
        assert!(matches!(a.open_fd("art"), Err(AssetError::NotFound(_))));
        assert!(matches!(a.open_fd("nope"), Err(AssetError::NotFound(_))));

        let mut r = a.open("art/a.bin").unwrap();
        assert_eq!(r.len(), 5);
        r.seek(SeekFrom::Start(2)).unwrap();
        let mut rest = Vec::new();
        r.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, [3, 4, 5]);

        let (tx, rx) = std::sync::mpsc::channel();
        let tx2 = tx.clone();
        a.load("hello.txt", move |r| tx.send(r).unwrap());
        a.load("../x", move |r| tx2.send(r).unwrap());
        let mut got: Vec<_> = (0..2)
            .map(|_| rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap())
            .collect();
        got.sort_by_key(|r| r.is_ok());
        assert!(matches!(got[0], Err(AssetError::InvalidPath(_))));
        assert_eq!(got[1].as_deref(), Ok(&b"hello, assets\n"[..]));

        // Through the app's handle: decoded on a worker, applied on pump.
        let h = crate::core::AppHandle::<Vec<String>>::new();
        let utf8 = |b: &[u8]| std::str::from_utf8(b).map(str::to_owned);
        h.load_asset("hello.txt", utf8, |s, r| s.push(r.unwrap()));
        h.load_asset("not-utf8.txt", utf8, |s, r| {
            assert!(matches!(r, Err(AssetError::Decode { .. })));
            s.push("decode error".into());
        });
        let mut state = Vec::new();
        let start = std::time::Instant::now();
        while state.len() < 2 && start.elapsed() < std::time::Duration::from_secs(5) {
            h.pump(&mut state, crate::platform::time::Instant::now());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        state.sort();
        assert_eq!(state, ["decode error", "hello, assets\n"]);

        *GIVEN.lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
