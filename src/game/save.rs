//! Saving: settings and save slots, written so that a crash, a full disk or
//! a pulled plug never leaves the player without a save.
//!
//! A [`SaveStore`] keeps named files in a directory (the app's data
//! directory), or in memory, or in the browser's `localStorage` on the web.
//! Each [`save`](SaveStore::save) goes through a temporary file in the same
//! directory, flushed to the disk, then renamed over the old file, whose
//! previous contents stay as `NAME.bak`. Every file ends with a checksum
//! line, so a damaged one is noticed, and [`load`](SaveStore::load) falls
//! back to the `.bak`. [`KeyValues`] is a small `key = value` text form for
//! settings, and [`SaveSlots`] numbered save slots with the time each was
//! written. See the "Saving" section of `docs/games.md`.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Where a [`SaveStore`] keeps its files.
#[derive(Clone, Debug)]
enum Backend {
    Dir(PathBuf),
    Memory(Arc<Mutex<BTreeMap<String, Vec<u8>>>>),
    #[cfg(target_arch = "wasm32")]
    Web(String),
}

/// A step of [`SaveStore::save`], where tests make it fail as if the
/// process died there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Halfway through writing the temporary file.
    WriteTmp,
    /// After writing it, before flushing it to the disk.
    SyncTmp,
    /// Before moving the current file to `.bak`.
    Backup,
    /// After moving it, before renaming the temporary file into place.
    Commit,
    /// After the rename, before flushing the directory.
    SyncDir,
}

/// Named files saved atomically, each with a `.bak` of its previous
/// contents.
///
/// ```no_run
/// use stipple::game::save::{KeyValues, SaveStore};
///
/// let store = SaveStore::app_data()?; // or SaveStore::new(dir)
/// let mut settings = KeyValues::new();
/// settings.set("music", 0.6);
/// settings.set("fullscreen", true);
/// store.save_text("settings", &settings)?;
///
/// let settings = store.load_text("settings")?.unwrap_or_default();
/// let music: f32 = settings.get_or("music", 0.8);
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// Names are a file's name without a directory: letters, digits, `-`, `_`
/// and `.`, not starting with a dot. `NAME.tmp` and `NAME.bak` are the
/// store's own. Clones share the same files.
#[derive(Clone, Debug)]
pub struct SaveStore {
    backend: Backend,
    #[cfg(test)]
    fail: std::cell::Cell<Option<Step>>,
}

/// What [`SaveStore::load`] read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    /// The contents, as saved.
    pub bytes: Vec<u8>,
    /// When it was saved.
    pub modified: SystemTime,
    /// Whether the file itself was missing or damaged and this is the
    /// `.bak`: the save before the last.
    pub from_backup: bool,
}

/// The first line of the footer every saved file ends with.
const MAGIC: &str = "#stipple-save 1";

impl SaveStore {
    /// Files in `dir`, which is created (with its parents) on the first
    /// save. Pass the app's data directory.
    pub fn new(dir: impl Into<PathBuf>) -> SaveStore {
        SaveStore::with(Backend::Dir(dir.into()))
    }

    /// Files in the app's data directory
    /// ([`platform::dirs`](crate::platform::dirs)`().data`, named after the
    /// app's id on the desktops: set it with
    /// [`App::app_info`](crate::App::app_info) before calling this). On the
    /// web, the browser's `localStorage` under the app's id.
    pub fn app_data() -> io::Result<SaveStore> {
        #[cfg(target_arch = "wasm32")]
        {
            Ok(SaveStore::local_storage(crate::platform::app_info().id))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::platform::dirs()
                .map(|d| SaveStore::new(d.data))
                .map_err(io::Error::other)
        }
    }

    /// Files kept in memory, gone when the last clone is dropped: for tests,
    /// and for a platform without a place to save.
    pub fn memory() -> SaveStore {
        SaveStore::with(Backend::Memory(Arc::default()))
    }

    /// Files kept in the browser's `localStorage` (through `web/stipple.js`),
    /// under keys starting `prefix/`. They stay until the site's data is
    /// cleared; a browser may refuse to store more than a few megabytes in
    /// all, which `save` reports as an error.
    #[cfg(target_arch = "wasm32")]
    pub fn local_storage(prefix: impl Into<String>) -> SaveStore {
        SaveStore::with(Backend::Web(prefix.into()))
    }

    fn with(backend: Backend) -> SaveStore {
        SaveStore {
            backend,
            #[cfg(test)]
            fail: std::cell::Cell::new(None),
        }
    }

    /// The directory, for a store made with [`new`](Self::new).
    pub fn dir(&self) -> Option<&Path> {
        match &self.backend {
            Backend::Dir(dir) => Some(dir),
            _ => None,
        }
    }

    /// Make the next save fail at `step`, as if the process died there.
    #[cfg(test)]
    pub(crate) fn fail_at(&self, step: Step) {
        self.fail.set(Some(step));
    }

    /// Whether a test asked for a crash at `step` (once).
    fn fails(&self, step: Step) -> bool {
        #[cfg(test)]
        if self.fail.get() == Some(step) {
            self.fail.set(None);
            return true;
        }
        let _ = step;
        false
    }

    fn crash(&self, step: Step) -> io::Result<()> {
        if self.fails(step) {
            return Err(io::Error::other(format!("simulated crash at {step:?}")));
        }
        Ok(())
    }

    /// Save `bytes` as `name`: written to `name.tmp`, flushed to the disk,
    /// then renamed over `name`, whose last good contents become
    /// `name.bak`. Whenever it stops (an error, a crash, the power), `name`
    /// holds the old contents or the new ones, or is missing with the old
    /// ones in `name.bak`; [`load`](Self::load) reads whichever is good.
    pub fn save(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        check_name(name)?;
        let tmp = format!("{name}.tmp");
        let bak = format!("{name}.bak");
        let file = wrap(bytes, now());
        if self.fails(Step::WriteTmp) {
            self.backend.write(&tmp, &file[..file.len() / 2], false)?;
            return Err(io::Error::other("simulated crash at WriteTmp"));
        }
        let sync = !self.fails(Step::SyncTmp);
        self.backend.write(&tmp, &file, sync)?;
        if !sync {
            return Err(io::Error::other("simulated crash at SyncTmp"));
        }
        // Keep the current file as the backup, if it is good: a damaged one
        // mustn't push out a good backup.
        let current = self.backend.read(name)?;
        self.crash(Step::Backup)?;
        if current.as_deref().and_then(unwrap).is_some() {
            self.backend.rename(name, &bak)?;
        }
        self.crash(Step::Commit)?;
        self.backend.rename(&tmp, name)?;
        self.crash(Step::SyncDir)?;
        self.backend.sync_dir();
        Ok(())
    }

    /// Read `name`, or its `.bak` when it is missing or damaged. `None` when
    /// neither exists; an [`InvalidData`](io::ErrorKind::InvalidData) error
    /// when neither is good.
    pub fn load(&self, name: &str) -> io::Result<Option<Loaded>> {
        check_name(name)?;
        let main = self.backend.read(name)?;
        if let Some((bytes, modified)) = main.as_deref().and_then(unwrap) {
            return Ok(Some(Loaded {
                bytes: bytes.to_vec(),
                modified,
                from_backup: false,
            }));
        }
        let bak = self.backend.read(&format!("{name}.bak"))?;
        if let Some((bytes, modified)) = bak.as_deref().and_then(unwrap) {
            return Ok(Some(Loaded {
                bytes: bytes.to_vec(),
                modified,
                from_backup: true,
            }));
        }
        if main.is_none() && bak.is_none() {
            return Ok(None);
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the save {name:?} and its backup are damaged"),
        ))
    }

    /// Save `values` as text (see [`KeyValues`]).
    pub fn save_text(&self, name: &str, values: &KeyValues) -> io::Result<()> {
        self.save(name, values.to_string().as_bytes())
    }

    /// Read text saved by [`save_text`](Self::save_text). Text that isn't
    /// UTF-8 or `key = value` lines is an
    /// [`InvalidData`](io::ErrorKind::InvalidData) error.
    pub fn load_text(&self, name: &str) -> io::Result<Option<KeyValues>> {
        let Some(loaded) = self.load(name)? else {
            return Ok(None);
        };
        let text = String::from_utf8(loaded.bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        KeyValues::parse(&text)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Whether `name` (or its backup) is there.
    pub fn exists(&self, name: &str) -> bool {
        check_name(name).is_ok()
            && (matches!(self.backend.read(name), Ok(Some(_)))
                || matches!(self.backend.read(&format!("{name}.bak")), Ok(Some(_))))
    }

    /// Delete `name`, its backup and any temporary file left by a save that
    /// didn't finish.
    pub fn delete(&self, name: &str) -> io::Result<()> {
        check_name(name)?;
        for file in [
            name.to_string(),
            format!("{name}.bak"),
            format!("{name}.tmp"),
        ] {
            self.backend.remove(&file)?;
        }
        self.backend.sync_dir();
        Ok(())
    }

    /// The names saved (with a file or a backup), sorted.
    pub fn names(&self) -> io::Result<Vec<String>> {
        let mut out: Vec<String> = self
            .backend
            .names()?
            .into_iter()
            .filter(|n| !n.ends_with(".tmp"))
            .map(|n| n.strip_suffix(".bak").map_or(n.clone(), str::to_string))
            .filter(|n| check_name(n).is_ok())
            .collect();
        out.sort();
        out.dedup();
        Ok(out)
    }
}

fn check_name(name: &str) -> io::Result<()> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && !name.ends_with(".tmp")
        && !name.ends_with(".bak")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name:?} isn't a save name (letters, digits, '-', '_', '.')"),
        ))
    }
}

/// Milliseconds since 1970.
fn now() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        web::now_ms()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }
}

/// The contents and a footer line: the magic, the CRC-32 and length of the
/// contents, and the time saved.
fn wrap(bytes: &[u8], time_ms: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 64);
    out.extend_from_slice(bytes);
    out.extend_from_slice(
        format!("\n{MAGIC} {:08x} {} {time_ms}\n", crc32(bytes), bytes.len()).as_bytes(),
    );
    out
}

/// The contents and time of a file made by [`wrap`], or `None` if it's
/// damaged.
fn unwrap(file: &[u8]) -> Option<(&[u8], SystemTime)> {
    let body = file.strip_suffix(b"\n")?;
    let at = body.iter().rposition(|&b| b == b'\n')?;
    let footer = std::str::from_utf8(&body[at + 1..]).ok()?;
    let mut parts = footer.strip_prefix(MAGIC)?.split_ascii_whitespace();
    let crc = u32::from_str_radix(parts.next()?, 16).ok()?;
    let len: usize = parts.next()?.parse().ok()?;
    let time: u64 = parts.next()?.parse().ok()?;
    let bytes = &body[..at];
    (parts.next().is_none() && bytes.len() == len && crc32(bytes) == crc)
        .then(|| (bytes, UNIX_EPOCH + Duration::from_millis(time)))
}

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// CRC-32 (IEEE), as zip and PNG use.
fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |c, &b| {
        CRC_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8)
    })
}

impl Backend {
    fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        match self {
            Backend::Dir(dir) => match std::fs::read(dir.join(name)) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            },
            Backend::Memory(files) => Ok(lock(files).get(name).cloned()),
            #[cfg(target_arch = "wasm32")]
            Backend::Web(prefix) => Ok(web::get(&format!("{prefix}/{name}"))),
        }
    }

    /// Write a whole file, flushed to the disk if `sync`.
    fn write(&self, name: &str, bytes: &[u8], sync: bool) -> io::Result<()> {
        match self {
            Backend::Dir(dir) => {
                use std::io::Write;
                std::fs::create_dir_all(dir)?;
                let mut file = std::fs::File::create(dir.join(name))?;
                file.write_all(bytes)?;
                if sync {
                    file.sync_all()?;
                }
                Ok(())
            }
            Backend::Memory(files) => {
                lock(files).insert(name.to_string(), bytes.to_vec());
                Ok(())
            }
            #[cfg(target_arch = "wasm32")]
            Backend::Web(prefix) => web::set(&format!("{prefix}/{name}"), bytes),
        }
    }

    /// Rename `from` to `to`, replacing it.
    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        match self {
            Backend::Dir(dir) => std::fs::rename(dir.join(from), dir.join(to)),
            Backend::Memory(files) => {
                let mut files = lock(files);
                let bytes = files
                    .remove(from)
                    .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
                files.insert(to.to_string(), bytes);
                Ok(())
            }
            #[cfg(target_arch = "wasm32")]
            Backend::Web(prefix) => {
                let from = format!("{prefix}/{from}");
                let bytes =
                    web::get(&from).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
                web::set(&format!("{prefix}/{to}"), &bytes)?;
                web::remove(&from);
                Ok(())
            }
        }
    }

    fn remove(&self, name: &str) -> io::Result<()> {
        match self {
            Backend::Dir(dir) => match std::fs::remove_file(dir.join(name)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
            Backend::Memory(files) => {
                lock(files).remove(name);
                Ok(())
            }
            #[cfg(target_arch = "wasm32")]
            Backend::Web(prefix) => {
                web::remove(&format!("{prefix}/{name}"));
                Ok(())
            }
        }
    }

    /// Flush the directory, so a rename survives a power cut (Unix; elsewhere
    /// the rename is enough).
    fn sync_dir(&self) {
        #[cfg(unix)]
        if let Backend::Dir(dir) = self
            && let Ok(d) = std::fs::File::open(dir)
        {
            // Some file systems refuse; the data itself is already flushed.
            let _ = d.sync_all();
        }
    }

    fn names(&self) -> io::Result<Vec<String>> {
        match self {
            Backend::Dir(dir) => {
                let entries = match std::fs::read_dir(dir) {
                    Ok(e) => e,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
                    Err(e) => return Err(e),
                };
                Ok(entries
                    .filter_map(|e| e.ok()?.file_name().into_string().ok())
                    .collect())
            }
            Backend::Memory(files) => Ok(lock(files).keys().cloned().collect()),
            #[cfg(target_arch = "wasm32")]
            Backend::Web(prefix) => Ok(web::keys(&format!("{prefix}/"))),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `localStorage`, through `web/stipple.js` (values are kept base64).
#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code)] // the imports from `web/stipple.js`
mod web {
    use std::io;

    #[link(wasm_import_module = "stipple")]
    unsafe extern "C" {
        /// `Date.now()`.
        fn date_now() -> f64;
        /// Store a value; 0 if the browser refused (full, or storage off).
        fn storage_set(key: *const u8, key_len: usize, ptr: *const u8, len: usize) -> u32;
        /// A value's length, kept for `storage_read`; -1 if there's none.
        fn storage_get(key: *const u8, key_len: usize) -> i32;
        /// The keys starting with a prefix, one per line, kept for
        /// `storage_read`; their length.
        fn storage_keys(prefix: *const u8, prefix_len: usize) -> i32;
        /// Copy what `storage_get` or `storage_keys` kept to `ptr`.
        fn storage_read(ptr: *mut u8);
        fn storage_remove(key: *const u8, key_len: usize);
    }

    /// `Date.now()`: milliseconds since 1970.
    pub(super) fn now_ms() -> u64 {
        (unsafe { date_now() }).max(0.0) as u64
    }

    fn take(len: i32) -> Option<Vec<u8>> {
        let len = usize::try_from(len).ok()?;
        let mut buf = vec![0u8; len];
        unsafe { storage_read(buf.as_mut_ptr()) };
        Some(buf)
    }

    pub(super) fn get(key: &str) -> Option<Vec<u8>> {
        take(unsafe { storage_get(key.as_ptr(), key.len()) })
    }

    pub(super) fn set(key: &str, bytes: &[u8]) -> io::Result<()> {
        if unsafe { storage_set(key.as_ptr(), key.len(), bytes.as_ptr(), bytes.len()) } == 0 {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "localStorage refused the save",
            ));
        }
        Ok(())
    }

    pub(super) fn remove(key: &str) {
        unsafe { storage_remove(key.as_ptr(), key.len()) }
    }

    pub(super) fn keys(prefix: &str) -> Vec<String> {
        let bytes =
            take(unsafe { storage_keys(prefix.as_ptr(), prefix.len()) }).unwrap_or_default();
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|k| k.strip_prefix(prefix))
            .map(str::to_string)
            .collect()
    }
}

/// Settings as text: `key = value` lines, in the order they were set.
///
/// ```text
/// # comments and blank lines are skipped
/// music = 0.6
/// fullscreen = true
/// name = Ada\nLovelace
/// ```
///
/// Keys and values are trimmed. In a value, a backslash, a line break and a
/// carriage return are written `\\`, `\n` and `\r`, so any text fits on a
/// line. A key can't hold `=`, a line break or start with `#`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyValues {
    entries: Vec<(String, String)>,
}

/// A line of [`KeyValues`] text that isn't `key = value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseKeyValuesError {
    /// The line, from 1.
    pub line: usize,
}

impl fmt::Display for ParseKeyValuesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: expected `key = value`", self.line)
    }
}

impl std::error::Error for ParseKeyValuesError {}

impl KeyValues {
    pub fn new() -> KeyValues {
        KeyValues::default()
    }

    /// Read `key = value` lines. Blank lines and lines starting with `#`
    /// are skipped; another line without `=` is an error.
    pub fn parse(text: &str) -> Result<KeyValues, ParseKeyValuesError> {
        let mut out = KeyValues::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or(ParseKeyValuesError { line: i + 1 })?;
            let key = key.trim();
            if key.is_empty() {
                return Err(ParseKeyValuesError { line: i + 1 });
            }
            out.set(key, unescape(value.trim()));
        }
        Ok(out)
    }

    /// A key's value.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// A key's value parsed (`f32`, `bool`, `u32`, …); `None` if it's missing
    /// or doesn't parse.
    pub fn parse_value<T: FromStr>(&self, key: &str) -> Option<T> {
        self.get(key)?.parse().ok()
    }

    /// A key's value parsed, or `default`.
    pub fn get_or<T: FromStr>(&self, key: &str, default: T) -> T {
        self.parse_value(key).unwrap_or(default)
    }

    /// Set a key (where it was, or at the end), to anything `Display`.
    ///
    /// # Panics
    ///
    /// If the key is empty, holds `=` or a line break, or starts with `#`.
    pub fn set(&mut self, key: &str, value: impl fmt::Display) {
        let key = key.trim();
        assert!(
            !key.is_empty() && !key.starts_with('#') && !key.contains(['=', '\n', '\r']),
            "{key:?} can't be a key"
        );
        let value = value.to_string();
        match self.entries.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => self.entries.push((key.to_string(), value)),
        }
    }

    /// Remove a key; its value, if it was there.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(i).1)
    }

    /// The keys and values, in order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The text form, one `key = value` line each.
impl fmt::Display for KeyValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (k, v) in &self.entries {
            writeln!(f, "{k} = {}", escape(v))?;
        }
        Ok(())
    }
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Numbered save slots in a [`SaveStore`]: `PREFIX-0`, `PREFIX-1`, …,
/// listed with when each was saved for a "load game" screen.
///
/// ```no_run
/// use stipple::game::save::{SaveSlots, SaveStore, SlotState};
///
/// let slots = SaveSlots::new(SaveStore::new("/path/to/the/app/data"), "slot", 3);
/// slots.save(1, b"level=4\ncoins=37\n")?;
/// for slot in slots.list() {
///     match slot.state {
///         SlotState::Empty => println!("{}: empty", slot.index),
///         SlotState::Saved(info) => println!("{}: saved {:?}", slot.index, info.modified),
///         SlotState::Damaged => println!("{}: damaged", slot.index),
///     }
/// }
/// let bytes = slots.load(1)?; // Option<Vec<u8>>
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct SaveSlots {
    store: SaveStore,
    prefix: String,
    count: usize,
}

/// A slot, as [`SaveSlots::list`] sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotInfo {
    pub index: usize,
    pub state: SlotState,
}

/// What is in a slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlotState {
    Empty,
    Saved(SaveInfo),
    /// There's a save, but it and its backup are damaged.
    Damaged,
}

/// A saved slot's details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveInfo {
    /// When it was saved.
    pub modified: SystemTime,
    /// Its size in bytes.
    pub size: usize,
    /// Whether it's the backup: the last save was damaged.
    pub from_backup: bool,
}

impl SaveSlots {
    /// `count` slots named `prefix-0` to `prefix-{count - 1}`.
    pub fn new(store: SaveStore, prefix: impl Into<String>, count: usize) -> SaveSlots {
        SaveSlots {
            store,
            prefix: prefix.into(),
            count,
        }
    }

    /// How many slots there are.
    pub fn count(&self) -> usize {
        self.count
    }

    /// The store the slots are in.
    pub fn store(&self) -> &SaveStore {
        &self.store
    }

    /// The file name of slot `index`.
    pub fn name(&self, index: usize) -> String {
        format!("{}-{index}", self.prefix)
    }

    fn checked(&self, index: usize) -> io::Result<String> {
        if index < self.count {
            Ok(self.name(index))
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("there's no save slot {index} (of {})", self.count),
            ))
        }
    }

    /// Every slot, in order, with what's in it.
    pub fn list(&self) -> Vec<SlotInfo> {
        (0..self.count)
            .map(|index| SlotInfo {
                index,
                state: match self.store.load(&self.name(index)) {
                    Ok(None) => SlotState::Empty,
                    Ok(Some(l)) => SlotState::Saved(SaveInfo {
                        modified: l.modified,
                        size: l.bytes.len(),
                        from_backup: l.from_backup,
                    }),
                    Err(_) => SlotState::Damaged,
                },
            })
            .collect()
    }

    /// The slot saved last, for "Continue".
    pub fn latest(&self) -> Option<usize> {
        self.list()
            .into_iter()
            .filter_map(|s| match s.state {
                SlotState::Saved(info) => Some((info.modified, s.index)),
                _ => None,
            })
            .max()
            .map(|(_, i)| i)
    }

    /// Save into slot `index` (atomically, see [`SaveStore::save`]).
    pub fn save(&self, index: usize, bytes: &[u8]) -> io::Result<()> {
        self.store.save(&self.checked(index)?, bytes)
    }

    /// What slot `index` holds (or its backup).
    pub fn load(&self, index: usize) -> io::Result<Option<Vec<u8>>> {
        Ok(self.store.load(&self.checked(index)?)?.map(|l| l.bytes))
    }

    /// Empty slot `index`.
    pub fn delete(&self, index: usize) -> io::Result<()> {
        self.store.delete(&self.checked(index)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system's temp directory.
    fn temp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("stipple-save-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn stores(tag: &str) -> Vec<(SaveStore, Option<PathBuf>)> {
        let dir = temp_dir(tag);
        vec![
            (SaveStore::new(&dir), Some(dir)),
            (SaveStore::memory(), None),
        ]
    }

    fn cleanup(dir: Option<PathBuf>) {
        if let Some(dir) = dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn saves_round_trip_with_a_time() {
        for (store, dir) in stores("roundtrip") {
            assert_eq!(store.load("game").unwrap(), None);
            let before = SystemTime::now() - Duration::from_secs(1);
            store.save("game", b"hello\nworld").unwrap();
            let l = store.load("game").unwrap().unwrap();
            assert_eq!(l.bytes, b"hello\nworld");
            assert!(!l.from_backup);
            assert!(l.modified >= before && l.modified <= SystemTime::now());
            // Empty contents too.
            store.save("empty", b"").unwrap();
            assert_eq!(store.load("empty").unwrap().unwrap().bytes, b"");
            assert_eq!(store.names().unwrap(), vec!["empty", "game"]);
            assert!(store.exists("game"));
            store.delete("game").unwrap();
            assert!(!store.exists("game"));
            assert_eq!(store.load("game").unwrap(), None);
            cleanup(dir);
        }
    }

    #[test]
    fn the_previous_save_is_kept_as_bak() {
        let dir = temp_dir("bak");
        let store = SaveStore::new(&dir);
        store.save("s", b"one").unwrap();
        assert!(!dir.join("s.bak").exists());
        store.save("s", b"two").unwrap();
        let bak = std::fs::read(dir.join("s.bak")).unwrap();
        assert_eq!(unwrap(&bak).unwrap().0, b"one");
        assert!(!dir.join("s.tmp").exists());
        cleanup(Some(dir));
    }

    #[test]
    fn a_damaged_file_falls_back_to_the_bak() {
        let dir = temp_dir("damaged");
        let store = SaveStore::new(&dir);
        store.save("s", b"first").unwrap();
        store.save("s", b"second").unwrap();
        // Flip a byte, then cut it short: both are noticed.
        let path = dir.join("s");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[2] ^= 0x20;
        std::fs::write(&path, &bytes).unwrap();
        let l = store.load("s").unwrap().unwrap();
        assert_eq!((l.bytes.as_slice(), l.from_backup), (&b"first"[..], true));
        std::fs::write(&path, &bytes[..4]).unwrap();
        assert!(store.load("s").unwrap().unwrap().from_backup);
        // A damaged file doesn't push out the good backup on the next save.
        store.save("s", b"third").unwrap();
        let bak = std::fs::read(dir.join("s.bak")).unwrap();
        assert_eq!(unwrap(&bak).unwrap().0, b"first");
        assert_eq!(store.load("s").unwrap().unwrap().bytes, b"third");
        // Both damaged: an error, not someone else's data.
        std::fs::write(&path, b"junk").unwrap();
        std::fs::write(dir.join("s.bak"), b"junk").unwrap();
        let err = store.load("s").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        cleanup(Some(dir));
    }

    #[test]
    fn a_crash_at_any_step_leaves_the_old_or_the_new_save() {
        let steps = [
            Step::WriteTmp,
            Step::SyncTmp,
            Step::Backup,
            Step::Commit,
            Step::SyncDir,
        ];
        for step in steps {
            for (store, dir) in stores("crash") {
                store.save("s", b"old").unwrap();
                store.fail_at(step);
                assert!(store.save("s", b"new").is_err(), "{step:?}");
                let got = store.load("s").unwrap().unwrap().bytes;
                // Up to the rename it's the old save; after, the new one.
                let want: &[u8] = if matches!(step, Step::SyncDir) {
                    b"new"
                } else {
                    b"old"
                };
                assert_eq!(got, want, "{step:?}");
                // The next save goes through, whatever was left behind.
                store.save("s", b"newer").unwrap();
                let l = store.load("s").unwrap().unwrap();
                assert_eq!((l.bytes.as_slice(), l.from_backup), (&b"newer"[..], false));
                assert_eq!(store.names().unwrap(), vec!["s"], "{step:?}");
                cleanup(dir);
            }
        }
    }

    #[test]
    fn a_crash_on_the_first_save_leaves_nothing() {
        for (store, dir) in stores("first") {
            store.fail_at(Step::WriteTmp);
            assert!(store.save("s", b"new").is_err());
            assert_eq!(store.load("s").unwrap(), None);
            store.fail_at(Step::Commit);
            assert!(store.save("s", b"new").is_err());
            assert_eq!(store.load("s").unwrap(), None);
            cleanup(dir);
        }
    }

    #[test]
    fn names_are_checked() {
        let store = SaveStore::memory();
        for bad in ["", ".hidden", "a/b", "..", "x.bak", "x.tmp", "a b", "é"] {
            assert_eq!(
                store.save(bad, b"").unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{bad:?}"
            );
        }
        store.save("settings.txt", b"").unwrap();
        store.save("slot-1_a", b"").unwrap();
    }

    #[test]
    fn key_values_round_trip() {
        let mut kv = KeyValues::new();
        kv.set("music", 0.6);
        kv.set("fullscreen", true);
        kv.set("name", "Ada\nLovelace \\o/");
        kv.set("music", 0.5); // replaced in place
        let text = kv.to_string();
        assert_eq!(
            text,
            "music = 0.5\nfullscreen = true\nname = Ada\\nLovelace \\\\o/\n"
        );
        let back = KeyValues::parse(&text).unwrap();
        assert_eq!(back, kv);
        assert_eq!(back.get_or("music", 1.0f32), 0.5);
        assert!(back.get_or("fullscreen", false));
        assert_eq!(back.get_or("missing", 7u32), 7);
        assert_eq!(back.parse_value::<u32>("name"), None);
        assert_eq!(back.get("name"), Some("Ada\nLovelace \\o/"));
        let parsed = KeyValues::parse("# c\n\n  a =1 \nb= x = y\n").unwrap();
        assert_eq!(
            parsed.iter().collect::<Vec<_>>(),
            [("a", "1"), ("b", "x = y")]
        );
        assert_eq!(
            KeyValues::parse("a = 1\noops\n").unwrap_err(),
            ParseKeyValuesError { line: 2 }
        );
        let mut kv = parsed;
        assert_eq!(kv.remove("a").as_deref(), Some("1"));
        assert_eq!(kv.len(), 1);
    }

    #[test]
    fn text_saves_round_trip() {
        let store = SaveStore::memory();
        assert_eq!(store.load_text("settings").unwrap(), None);
        let mut kv = KeyValues::new();
        kv.set("volume", 0.25);
        store.save_text("settings", &kv).unwrap();
        assert_eq!(store.load_text("settings").unwrap(), Some(kv));
        store.save("settings", b"not = a\nkey value file").unwrap();
        assert_eq!(
            store.load_text("settings").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn slots_list_and_find_the_latest() {
        for (store, dir) in stores("slots") {
            let slots = SaveSlots::new(store.clone(), "slot", 3);
            assert!(slots.list().iter().all(|s| s.state == SlotState::Empty));
            assert_eq!(slots.latest(), None);
            slots.save(2, b"two").unwrap();
            std::thread::sleep(Duration::from_millis(5));
            slots.save(0, b"zero!").unwrap();
            let list = slots.list();
            assert_eq!(list.len(), 3);
            assert_eq!(list[1].state, SlotState::Empty);
            let SlotState::Saved(info) = list[0].state else {
                panic!("{:?}", list[0]);
            };
            assert_eq!((info.size, info.from_backup), (5, false));
            assert_eq!(slots.latest(), Some(0));
            assert_eq!(slots.load(2).unwrap().as_deref(), Some(&b"two"[..]));
            assert_eq!(slots.load(1).unwrap(), None);
            assert!(slots.save(3, b"").is_err());
            // Damaged with no backup.
            store.save("slot-1", b"x").unwrap();
            if let Some(dir) = &dir {
                std::fs::write(dir.join("slot-1"), b"junk").unwrap();
                assert_eq!(slots.list()[1].state, SlotState::Damaged);
            }
            slots.delete(2).unwrap();
            assert_eq!(slots.list()[2].state, SlotState::Empty);
            cleanup(dir);
        }
    }
}
