//! The single-instance transport on Linux and macOS: the owner holds an
//! `flock` on `<dir>/<id>.instance.lock` for its life and listens on the Unix
//! socket `<dir>/<id>.instance`, in a directory only the user can enter
//! (`$XDG_RUNTIME_DIR` on Linux, the per-user `$TMPDIR` on macOS, else a
//! `stipple-<uid>` directory of the user's own under the temp directory). The
//! lock goes with the process, so a crashed owner leaves only a stale socket,
//! which the next owner replaces.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Launch, file_name, serve};

unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
    fn getuid() -> u32;
}

const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;

/// The lock and the listening socket of the process that owns an id.
pub(super) struct Owner {
    lock: File,
    listener: UnixListener,
}

/// The directory the lock and the socket go in.
fn dir() -> io::Result<PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "fullrust"))]
    if let Some(run) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
        && run.is_absolute()
        && private(&run)
    {
        return Ok(run);
    }
    // macOS gives each user a private `$TMPDIR` (under `/var/folders`).
    #[cfg(target_os = "macos")]
    if let Some(tmp) = std::env::var_os("TMPDIR").map(PathBuf::from)
        && tmp.is_absolute()
        && private(&tmp)
    {
        return Ok(tmp);
    }
    let uid = unsafe { getuid() };
    let dir = std::env::temp_dir().join(format!("stipple-{uid}"));
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    if !private(&dir) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} isn't the user's own", dir.display()),
        ));
    }
    Ok(dir)
}

/// Whether `dir` is a directory (not a link) of the user's that nobody else
/// may enter.
fn private(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir)
        .is_ok_and(|m| m.is_dir() && m.uid() == unsafe { getuid() } && m.mode() & 0o077 == 0)
}

/// The lock file's and the socket's paths for `id`, in `dir`.
fn paths(dir: &Path, id: &str) -> (PathBuf, PathBuf) {
    let name = file_name(id);
    (
        dir.join(format!("{name}.instance.lock")),
        dir.join(format!("{name}.instance")),
    )
}

/// Take `id`: `Some` with the lock and a listening socket, `None` when
/// another process holds it.
pub(super) fn own(id: &str) -> io::Result<Option<Owner>> {
    own_in(&dir()?, id)
}

fn own_in(dir: &Path, id: &str) -> io::Result<Option<Owner>> {
    let (lock_path, sock) = paths(dir, id);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)?;
    if unsafe { flock(lock.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0 {
        let e = io::Error::last_os_error();
        return match e.kind() {
            io::ErrorKind::WouldBlock => Ok(None),
            _ => Err(e),
        };
    }
    // The last owner's socket, if it died without removing it.
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock)?;
    // The sticky bit keeps `$XDG_RUNTIME_DIR`'s periodic clean-up off them.
    let mode = if cfg!(any(target_os = "linux", target_os = "fullrust")) {
        0o1600
    } else {
        0o600
    };
    for path in [&sock, &lock_path] {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    Ok(Some(Owner { lock, listener }))
}

/// Serve launches on a thread for the process's life, handing each to `sink`.
pub(super) fn listen(owner: Owner, sink: Box<dyn Fn(Launch) + Send + Sync>) {
    let spawned = std::thread::Builder::new()
        .name("stipple-instance".into())
        .spawn(move || {
            // Held while the thread (the process) lives.
            let Owner { lock, listener } = owner;
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    continue;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                serve(stream, &*sink);
            }
            drop(lock);
        });
    if let Err(e) = spawned {
        crate::platform::window::debug_window(format_args!("single instance: {e}"));
    }
}

/// Connect to the owner of `id`.
pub(super) fn connect(id: &str) -> io::Result<UnixStream> {
    connect_in(&dir()?, id)
}

fn connect_in(dir: &Path, id: &str) -> io::Result<UnixStream> {
    let stream = UnixStream::connect(paths(dir, id).1)?;
    stream.set_read_timeout(Some(super::PATIENCE))?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::super::exchange;
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_second_launch_reaches_the_owner() {
        let dir = std::env::temp_dir().join(format!("si{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let id = "org.example.Instance";
        assert!(connect_in(&dir, id).is_err(), "nobody owns it yet");
        let owner = own_in(&dir, id).unwrap().expect("the first owns it");
        // Another claim (another open file, as another process has) is refused.
        assert!(own_in(&dir, id).unwrap().is_none());
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        listen(
            owner,
            Box::new(move |l| {
                let _ = tx.lock().unwrap().send(l);
            }),
        );
        let launch = Launch {
            args: vec!["--new-window".into(), "a b".into()],
            cwd: Some("/tmp".into()),
            env: vec![("K".into(), "v".into())],
            activation_token: Some("t".into()),
            ..Launch::default()
        };
        for _ in 0..2 {
            let stream = connect_in(&dir, id).unwrap();
            assert!(exchange(stream, &launch), "acknowledged");
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), launch);
        }
        let mode = std::fs::metadata(paths(&dir, id).1).unwrap().mode();
        assert_eq!(mode & 0o077, 0, "the socket is the user's alone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_socket_is_replaced() {
        let dir = std::env::temp_dir().join(format!("ss{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let id = "stale";
        // A socket file nobody listens on (its owner died).
        drop(UnixListener::bind(paths(&dir, id).1).unwrap());
        assert!(connect_in(&dir, id).is_err());
        let owner = own_in(&dir, id).unwrap().expect("no lock held");
        drop(owner);
        // The lock went with the owner.
        assert!(own_in(&dir, id).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_directory_is_private() {
        let d = dir().unwrap();
        assert!(private(&d), "{}", d.display());
    }
}
