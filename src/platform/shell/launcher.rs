//! The launcher entry on Linux: a count, a progress bar and an urgent flag on
//! the app's icon in a dock or task manager, through the
//! `com.canonical.Unity.LauncherEntry` D-Bus signal (`Update`).
//!
//! The app broadcasts `Update(app_uri, properties)` on the session bus;
//! Ubuntu's Dock (Dash to Dock), KDE Plasma's task manager, Plank, Latte and
//! Unity listen for it and match `app_uri` (`application://<id>.desktop`) to
//! the `.desktop` file the app was launched from. Nothing answers, so a
//! desktop without such a launcher shows nothing. The connection stays open
//! on a thread of its own, which sends the whole state on every change.

use super::linux::{W, debug};
use crate::platform::a11y::DBus;
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};

/// What the launcher shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Entry {
    /// The badge's count, if the label is a number.
    pub count: Option<i64>,
    /// The progress, 0 to 1, while there is one.
    pub progress: Option<f64>,
    /// Whether the app asks for attention.
    pub urgent: bool,
}

/// The state last asked for, and the sending thread's end.
struct Launcher {
    entry: Entry,
    tx: Option<Sender<Entry>>,
}

static LAUNCHER: Mutex<Launcher> = Mutex::new(Launcher {
    entry: Entry {
        count: None,
        progress: None,
        urgent: false,
    },
    tx: None,
});

/// The count a badge label shows: a whole number of at least zero (a
/// launcher has no room for text, so other labels show none).
pub(crate) fn count_of(label: &str) -> Option<i64> {
    let label = label.trim();
    if label.is_empty() || !label.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(label.parse::<i64>().unwrap_or(i64::MAX))
}

/// Show `label`'s count on the launcher, or none.
pub(crate) fn set_badge(label: Option<&str>) {
    update(|e| e.count = label.and_then(count_of));
}

/// Show `progress` (clamped to 0..1) on the launcher, or none.
pub(crate) fn set_progress(progress: Option<f64>) {
    update(|e| e.progress = progress.filter(|p| !p.is_nan()).map(|p| p.clamp(0.0, 1.0)));
}

/// Mark the app urgent on the launcher, or not.
pub(crate) fn set_urgent(urgent: bool) {
    update(|e| e.urgent = urgent);
}

/// The app's window got the focus: what asked for attention has it.
pub(crate) fn focused() {
    let urgent = LAUNCHER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry
        .urgent;
    if urgent {
        set_urgent(false);
    }
}

fn update(f: impl FnOnce(&mut Entry)) {
    let mut l = LAUNCHER.lock().unwrap_or_else(|e| e.into_inner());
    let before = l.entry.clone();
    f(&mut l.entry);
    if l.entry == before {
        return;
    }
    let entry = l.entry.clone();
    if let Some(tx) = &l.tx
        && tx.send(entry.clone()).is_ok()
    {
        return;
    }
    let (tx, rx) = channel::<Entry>();
    let started = std::thread::Builder::new()
        .name("stipple-launcher".into())
        .spawn(move || {
            let uri = app_uri();
            let path = object_path(&uri);
            let mut bus = None;
            while let Ok(mut entry) = rx.recv() {
                // Only the latest state matters.
                while let Ok(next) = rx.try_recv() {
                    entry = next;
                }
                send(&mut bus, &uri, &path, &entry);
            }
        });
    if started.is_ok() && tx.send(entry).is_ok() {
        l.tx = Some(tx);
    }
}

/// Emit `Update` for `entry`, connecting first (and once more on a broken
/// connection).
fn send(bus: &mut Option<DBus>, uri: &str, path: &str, entry: &Entry) {
    let body = update_body(uri, entry);
    for _ in 0..2 {
        if bus.is_none() {
            match DBus::connect_session() {
                Ok(b) => *bus = Some(b),
                Err(e) => {
                    debug(format_args!("launcher entry: no session bus ({e})"));
                    return;
                }
            }
        }
        let Some(b) = bus.as_mut() else { return };
        match b.emit_signal(path, INTERFACE, "Update", "sa{sv}", &body) {
            Ok(()) => {
                debug(format_args!("launcher entry {uri}: {entry:?}"));
                return;
            }
            Err(e) => {
                debug(format_args!("launcher entry: {e}; reconnecting"));
                *bus = None;
            }
        }
    }
}

const INTERFACE: &str = "com.canonical.Unity.LauncherEntry";

/// The signal's body (`sa{sv}`): the app's URI, then every property, so a
/// launcher that missed earlier signals still shows the whole state.
pub(crate) fn update_body(uri: &str, e: &Entry) -> Vec<u8> {
    let mut w = W::default();
    w.str(uri);
    w.array(8, |w| {
        let entry = |w: &mut W, key: &str, sig: &str, f: &dyn Fn(&mut W)| {
            w.strukt(|w| {
                w.str(key);
                w.variant(sig, f);
            });
        };
        let count = e.count.unwrap_or(0);
        entry(w, "count", "x", &|w| {
            w.pad(8);
            w.buf.extend_from_slice(&count.to_le_bytes());
        });
        entry(w, "count-visible", "b", &|w| w.bool(e.count.is_some()));
        let progress = e.progress.unwrap_or(0.0);
        entry(w, "progress", "d", &|w| {
            w.pad(8);
            w.buf.extend_from_slice(&progress.to_le_bytes());
        });
        entry(w, "progress-visible", "b", &|w| {
            w.bool(e.progress.is_some())
        });
        entry(w, "urgent", "b", &|w| w.bool(e.urgent));
    });
    w.buf
}

/// The app's `.desktop` file id, which the launcher matches its entry to:
/// the Flatpak's id, else the id the app gave (`App::app_info`), else the
/// file a GLib launcher started this process from
/// (`$GIO_LAUNCHED_DESKTOP_FILE`, when its pid is ours), else the
/// executable's name.
fn desktop_id() -> String {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    desktop_id_from(
        var("FLATPAK_ID"),
        crate::platform::bundle::given_id(),
        var("GIO_LAUNCHED_DESKTOP_FILE").filter(|_| {
            var("GIO_LAUNCHED_DESKTOP_FILE_PID") == Some(std::process::id().to_string())
        }),
        super::linux::app_name(),
    )
}

pub(crate) fn desktop_id_from(
    flatpak: Option<String>,
    given: Option<String>,
    launched_from: Option<String>,
    exe: String,
) -> String {
    let from_file = launched_from.and_then(|path| {
        let name = std::path::Path::new(&path)
            .file_name()?
            .to_str()?
            .to_string();
        Some(name.strip_suffix(".desktop").unwrap_or(&name).to_string())
    });
    flatpak.or(given).or(from_file).unwrap_or(exe)
}

/// `application://<desktop id>.desktop`.
fn app_uri() -> String {
    format!("application://{}.desktop", desktop_id())
}

/// The object path the signal comes from: any will do (launchers match the
/// interface and the URI); libunity's is
/// `/com/canonical/unity/launcherentry/<hash of the URI>`.
pub(crate) fn object_path(uri: &str) -> String {
    // FNV-1a: stable, and digits only, as a path element must be.
    let hash = uri.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    format!("/com/canonical/unity/launcherentry/{hash}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::a11y::{read_string, read_u32};

    #[test]
    fn only_whole_numbers_are_counts() {
        assert_eq!(count_of("3"), Some(3));
        assert_eq!(count_of(" 120 "), Some(120));
        assert_eq!(count_of("99999999999999999999"), Some(i64::MAX));
        assert_eq!(count_of(""), None);
        assert_eq!(count_of("New"), None);
        assert_eq!(count_of("-1"), None);
        assert_eq!(count_of("1.5"), None);
    }

    /// Read a `{sv}` entry's key and variant signature, leaving `off` on the
    /// value.
    fn key(b: &[u8], off: &mut usize) -> (String, String) {
        *off = off.next_multiple_of(8);
        let k = read_string(b, off).unwrap();
        let n = b[*off] as usize;
        let sig = std::str::from_utf8(&b[*off + 1..*off + 1 + n])
            .unwrap()
            .to_string();
        *off += n + 2;
        (k, sig)
    }

    fn read_u64(b: &[u8], off: &mut usize) -> u64 {
        *off = off.next_multiple_of(8);
        let v = u64::from_le_bytes(b[*off..*off + 8].try_into().unwrap());
        *off += 8;
        v
    }

    #[test]
    fn marshals_an_update_body() {
        let e = Entry {
            count: Some(7),
            progress: Some(0.25),
            urgent: true,
        };
        let b = update_body("application://org.example.App.desktop", &e);
        let mut off = 0;
        assert_eq!(
            read_string(&b, &mut off).unwrap(),
            "application://org.example.App.desktop"
        );
        let len = read_u32(&b, &mut off).unwrap() as usize;
        off = off.next_multiple_of(8);
        let start = off;
        assert_eq!(key(&b, &mut off), ("count".into(), "x".into()));
        assert_eq!(read_u64(&b, &mut off), 7);
        assert_eq!(key(&b, &mut off), ("count-visible".into(), "b".into()));
        assert_eq!(read_u32(&b, &mut off).unwrap(), 1);
        assert_eq!(key(&b, &mut off), ("progress".into(), "d".into()));
        assert_eq!(f64::from_bits(read_u64(&b, &mut off)), 0.25);
        assert_eq!(key(&b, &mut off), ("progress-visible".into(), "b".into()));
        assert_eq!(read_u32(&b, &mut off).unwrap(), 1);
        assert_eq!(key(&b, &mut off), ("urgent".into(), "b".into()));
        assert_eq!(read_u32(&b, &mut off).unwrap(), 1);
        assert_eq!(off, b.len());
        assert_eq!(off - start, len);
    }

    /// Every `{sv}` entry of a body, with booleans and numbers as `u64`
    /// (doubles as their bits).
    fn entries(b: &[u8]) -> Vec<(String, u64)> {
        let mut off = 0;
        read_string(b, &mut off).unwrap();
        let len = read_u32(b, &mut off).unwrap() as usize;
        off = off.next_multiple_of(8);
        let end = off + len;
        let mut out = Vec::new();
        while off < end {
            let (k, sig) = key(b, &mut off);
            let v = match sig.as_str() {
                "b" => read_u32(b, &mut off).unwrap() as u64,
                _ => read_u64(b, &mut off),
            };
            out.push((k, v));
        }
        out
    }

    #[test]
    fn an_empty_entry_hides_everything() {
        let b = update_body("application://a.desktop", &Entry::default());
        let all = entries(&b);
        assert_eq!(all.len(), 5);
        assert!(all.iter().all(|(_, v)| *v == 0), "{all:?}");
    }

    #[test]
    fn the_desktop_id_comes_from_the_package_first() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(
            desktop_id_from(s("org.flat.App"), s("org.given"), None, "exe".into()),
            "org.flat.App"
        );
        assert_eq!(
            desktop_id_from(None, s("org.given"), None, "exe".into()),
            "org.given"
        );
        assert_eq!(
            desktop_id_from(
                None,
                None,
                s("/usr/share/applications/foo.desktop"),
                "exe".into()
            ),
            "foo"
        );
        assert_eq!(desktop_id_from(None, None, None, "exe".into()), "exe");
    }

    #[test]
    fn the_object_path_is_a_valid_path() {
        let p = object_path("application://org.example.App.desktop");
        assert!(p.starts_with("/com/canonical/unity/launcherentry/"));
        assert!(
            p.rsplit('/')
                .next()
                .unwrap()
                .bytes()
                .all(|b| b.is_ascii_digit())
        );
        assert_eq!(p, object_path("application://org.example.App.desktop"));
    }
}
