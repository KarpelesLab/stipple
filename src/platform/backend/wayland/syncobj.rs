//! Explicit synchronization for GPU frames: `wp_linux_drm_syncobj_v1`, by
//! hand, over DRM synchronization objects driven with their ioctls.
//!
//! Each buffer gets a timeline (a DRM syncobj, created on a render node and
//! shared with the compositor as an fd through `import_timeline`). A commit
//! names two points on it: the acquire point, which signals once the GPU
//! has drawn the buffer (the renderer's `sync_file` moved onto it, or, where
//! the renderer waited for the GPU itself, signalled at once), and the
//! release point, which the compositor signals once it no longer reads the
//! buffer. Points only grow: a buffer's acquire comes after its previous
//! release. While a surface has a `wp_linux_drm_syncobj_surface_v1`,
//! `wl_buffer.release` means nothing, so a buffer is free when its release
//! point has signalled; and a `wl_shm` buffer can't be attached to it (the
//! decorations, or frames read back): the object goes first ([`forget`]).
//!
//! Where the compositor has no `wp_linux_drm_syncobj_manager_v1`, no render
//! node does timelines (`DRM_CAP_SYNCOBJ_TIMELINE`), or
//! `STIPPLE_WAYLAND_SYNCOBJ=0`, frames go as before: the renderer waits for
//! the GPU and buffers come back with `wl_buffer.release`.

use std::collections::HashMap;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

use super::{WlConn, arg_u32, dbg};

/// The global's interface name (version 1).
pub(super) const MANAGER: &str = "wp_linux_drm_syncobj_manager_v1";

// wp_linux_drm_syncobj_manager_v1 requests.
const MANAGER_GET_SURFACE: u16 = 1;
const MANAGER_IMPORT_TIMELINE: u16 = 2;
// wp_linux_drm_syncobj_timeline_v1 / _surface_v1 requests.
const TIMELINE_DESTROY: u16 = 0;
const SURFACE_DESTROY: u16 = 0;
const SURFACE_SET_ACQUIRE_POINT: u16 = 1;
const SURFACE_SET_RELEASE_POINT: u16 = 2;

/// `_IOWR('d', nr, size)`.
const fn iowr(nr: u32, size: usize) -> u64 {
    (3u64 << 30) | ((size as u64) << 16) | (0x64 << 8) | nr as u64
}

#[repr(C)]
#[derive(Default)]
struct GetCap {
    capability: u64,
    value: u64,
}

#[repr(C)]
#[derive(Default)]
struct Create {
    handle: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Default)]
struct Handle {
    handle: u32,
    flags: u32,
    fd: i32,
    pad: u32,
}

#[repr(C)]
#[derive(Default)]
struct Transfer {
    src_handle: u32,
    dst_handle: u32,
    src_point: u64,
    dst_point: u64,
    flags: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Default)]
struct TimelineWait {
    handles: u64,
    points: u64,
    timeout_nsec: i64,
    count_handles: u32,
    flags: u32,
    first_signaled: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Default)]
struct TimelineArray {
    handles: u64,
    points: u64,
    count_handles: u32,
    flags: u32,
}

const IOCTL_GET_CAP: u64 = iowr(0x0c, size_of::<GetCap>());
const IOCTL_SYNCOBJ_CREATE: u64 = iowr(0xbf, size_of::<Create>());
const IOCTL_SYNCOBJ_DESTROY: u64 = iowr(0xc0, size_of::<Create>());
const IOCTL_SYNCOBJ_HANDLE_TO_FD: u64 = iowr(0xc1, size_of::<Handle>());
const IOCTL_SYNCOBJ_FD_TO_HANDLE: u64 = iowr(0xc2, size_of::<Handle>());
const IOCTL_SYNCOBJ_TIMELINE_WAIT: u64 = iowr(0xca, size_of::<TimelineWait>());
const IOCTL_SYNCOBJ_QUERY: u64 = iowr(0xcb, size_of::<TimelineArray>());
const IOCTL_SYNCOBJ_TRANSFER: u64 = iowr(0xcc, size_of::<Transfer>());
const IOCTL_SYNCOBJ_TIMELINE_SIGNAL: u64 = iowr(0xcd, size_of::<TimelineArray>());

const CAP_SYNCOBJ_TIMELINE: u64 = 0x14;
const FD_TO_HANDLE_IMPORT_SYNC_FILE: u32 = 1;
const WAIT_FOR_SUBMIT: u32 = 2;

#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

unsafe extern "C" {
    fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
    fn clock_gettime(clock: i32, ts: *mut Timespec) -> i32;
}

/// `ioctl`, again while interrupted.
fn drm_ioctl<T>(fd: i32, request: u64, arg: &mut T) -> std::io::Result<()> {
    loop {
        if unsafe { ioctl(fd, request as std::ffi::c_ulong, arg as *mut T) } == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if !matches!(e.raw_os_error(), Some(4 | 11)) {
            return Err(e);
        }
    }
}

/// A render node that does timeline syncobjs.
#[derive(Debug)]
pub(super) struct Drm {
    file: File,
}

impl Drm {
    /// The render node of `main` (the compositor's main device) if it does
    /// timelines, else the first that does.
    fn open(main: Option<u64>) -> Option<Drm> {
        let mut nodes: Vec<std::path::PathBuf> = std::fs::read_dir("/dev/dri")
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("renderD"))
            })
            .collect();
        nodes.sort();
        // The main device's own render node first (the main device may be
        // its primary node: the same device in sysfs).
        let device_of = |dev: u64| {
            let (major, minor) = (((dev >> 8) & 0xfff), (dev & 0xff) | ((dev >> 12) & !0xff));
            std::fs::canonicalize(format!("/sys/dev/char/{major}:{minor}/device")).ok()
        };
        if let Some(main) = main.and_then(device_of) {
            nodes.sort_by_key(|p| {
                crate::platform::dmabuf::dev_of(p).and_then(device_of) != Some(main.clone())
            });
        }
        nodes.into_iter().find_map(|path| {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .ok()?;
            let mut cap = GetCap {
                capability: CAP_SYNCOBJ_TIMELINE,
                value: 0,
            };
            drm_ioctl(file.as_raw_fd(), IOCTL_GET_CAP, &mut cap).ok()?;
            (cap.value != 0).then(|| {
                dbg(format_args!("syncobj timelines on {}", path.display()));
                Drm { file }
            })
        })
    }

    fn fd(&self) -> i32 {
        self.file.as_raw_fd()
    }

    fn create(&self) -> std::io::Result<u32> {
        let mut c = Create::default();
        drm_ioctl(self.fd(), IOCTL_SYNCOBJ_CREATE, &mut c)?;
        Ok(c.handle)
    }

    fn destroy(&self, handle: u32) {
        let mut c = Create { handle, flags: 0 };
        let _ = drm_ioctl(self.fd(), IOCTL_SYNCOBJ_DESTROY, &mut c);
    }

    /// The syncobj as an fd, for the compositor.
    fn export(&self, handle: u32) -> std::io::Result<OwnedFd> {
        let mut h = Handle {
            handle,
            ..Handle::default()
        };
        drm_ioctl(self.fd(), IOCTL_SYNCOBJ_HANDLE_TO_FD, &mut h)?;
        Ok(unsafe { OwnedFd::from_raw_fd(h.fd) })
    }

    /// Put `sync_file`'s fence on `timeline` at `point`: through a binary
    /// syncobj it's imported into, then transferred.
    fn import_fence(&self, timeline: u32, point: u64, sync_file: &OwnedFd) -> std::io::Result<()> {
        let temp = self.create()?;
        let mut h = Handle {
            handle: temp,
            flags: FD_TO_HANDLE_IMPORT_SYNC_FILE,
            fd: sync_file.as_raw_fd(),
            pad: 0,
        };
        let mut r = drm_ioctl(self.fd(), IOCTL_SYNCOBJ_FD_TO_HANDLE, &mut h);
        if r.is_ok() {
            let mut t = Transfer {
                src_handle: temp,
                dst_handle: timeline,
                dst_point: point,
                ..Transfer::default()
            };
            r = drm_ioctl(self.fd(), IOCTL_SYNCOBJ_TRANSFER, &mut t);
        }
        self.destroy(temp);
        r
    }

    /// Signal `point` on `timeline` now.
    fn signal(&self, timeline: u32, point: u64) -> std::io::Result<()> {
        let mut a = TimelineArray {
            handles: &timeline as *const u32 as u64,
            points: &point as *const u64 as u64,
            count_handles: 1,
            flags: 0,
        };
        drm_ioctl(self.fd(), IOCTL_SYNCOBJ_TIMELINE_SIGNAL, &mut a)
    }

    /// The last point signalled on `timeline`.
    fn query(&self, timeline: u32) -> u64 {
        let mut point = 0u64;
        let mut a = TimelineArray {
            handles: &timeline as *const u32 as u64,
            points: &mut point as *mut u64 as u64,
            count_handles: 1,
            flags: 0,
        };
        let _ = drm_ioctl(self.fd(), IOCTL_SYNCOBJ_QUERY, &mut a);
        point
    }

    /// Wait up to `timeout` for any of `points` on `timelines` (once a fence
    /// is there to wait for): the index of the first signalled.
    fn wait_any(&self, timelines: &[u32], points: &[u64], timeout: Duration) -> Option<usize> {
        let mut now = Timespec { sec: 0, nsec: 0 };
        unsafe {
            clock_gettime(1 /* CLOCK_MONOTONIC */, &mut now)
        };
        let deadline =
            now.sec as i128 * 1_000_000_000 + now.nsec as i128 + timeout.as_nanos() as i128;
        let mut w = TimelineWait {
            handles: timelines.as_ptr() as u64,
            points: points.as_ptr() as u64,
            timeout_nsec: deadline.min(i64::MAX as i128) as i64,
            count_handles: timelines.len() as u32,
            flags: WAIT_FOR_SUBMIT,
            ..TimelineWait::default()
        };
        drm_ioctl(self.fd(), IOCTL_SYNCOBJ_TIMELINE_WAIT, &mut w)
            .ok()
            .map(|()| w.first_signaled as usize)
    }
}

/// A buffer's timeline: the DRM syncobj, the compositor's object for it, and
/// the last point used (its release point once committed; 0 before).
#[derive(Debug)]
struct Timeline {
    handle: u32,
    object: u32,
    point: u64,
}

/// The protocol state, kept on the connection.
#[derive(Debug, Default)]
pub(super) struct Syncobj {
    manager: Option<u32>,
    /// The render node, once opened (`Some(None)`: none does timelines).
    drm: Option<Option<Drm>>,
    /// Each surface's `wp_linux_drm_syncobj_surface_v1`.
    surfaces: HashMap<u32, u32>,
    /// Each buffer's timeline.
    timelines: HashMap<u32, Timeline>,
}

/// Keep the bound global.
pub(super) fn setup(conn: &mut WlConn, manager: u32) {
    conn.syncobj.manager = Some(manager);
}

/// Whether explicit sync is allowed: on, unless `STIPPLE_WAYLAND_SYNCOBJ=0`.
fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("STIPPLE_WAYLAND_SYNCOBJ").map_or(true, |v| v.trim() != "0"))
}

/// Whether frames go with explicit sync: the compositor has the protocol and
/// a render node does timelines (opened the first time).
pub(super) fn available(conn: &mut WlConn) -> bool {
    if conn.syncobj.manager.is_none() || !enabled() {
        return false;
    }
    let main = conn.dmabuf.main_device();
    conn.syncobj
        .drm
        .get_or_insert_with(|| {
            let drm = Drm::open(main);
            if drm.is_none() {
                dbg(format_args!(
                    "{MANAGER} offered, but no render node does timeline syncobjs"
                ));
            }
            drm
        })
        .is_some()
}

/// Whether buffer `id` is free: its release point has signalled (or it was
/// never committed with one). `None` when it has no timeline.
pub(super) fn released(conn: &WlConn, id: u32) -> Option<bool> {
    let t = conn.syncobj.timelines.get(&id)?;
    let drm = conn.syncobj.drm.as_ref()?.as_ref()?;
    Some(t.point == 0 || drm.query(t.handle) >= t.point)
}

/// Wait up to `timeout` for the release point of one of `ids`: the first
/// that is free.
pub(super) fn wait_release(conn: &WlConn, ids: &[u32], timeout: Duration) -> Option<u32> {
    let drm = conn.syncobj.drm.as_ref()?.as_ref()?;
    let (mut handles, mut points, mut which) = (Vec::new(), Vec::new(), Vec::new());
    for &id in ids {
        let t = conn.syncobj.timelines.get(&id)?;
        if t.point == 0 {
            return Some(id);
        }
        handles.push(t.handle);
        points.push(t.point);
        which.push(id);
    }
    drm.wait_any(&handles, &points, timeout)
        .and_then(|i| which.get(i).copied())
}

/// Set the acquire and release points of buffer `id`'s next commit on
/// `surface` (made its sync object if it has none): the acquire point
/// carries `fence`, or is signalled at once without one (the GPU was waited
/// for). Makes the buffer's timeline the first time.
pub(super) fn set_points(
    conn: &mut WlConn,
    surface: u32,
    id: u32,
    fence: Option<OwnedFd>,
) -> Result<(), String> {
    let manager = conn.syncobj.manager.ok_or("no syncobj manager")?;
    let io = |e: std::io::Error| e.to_string();
    if !conn.syncobj.timelines.contains_key(&id) {
        let drm = conn
            .syncobj
            .drm
            .as_ref()
            .and_then(Option::as_ref)
            .ok_or("no render node")?;
        let handle = drm.create().map_err(io)?;
        let fd = match drm.export(handle) {
            Ok(fd) => fd,
            Err(e) => {
                drm.destroy(handle);
                return Err(e.to_string());
            }
        };
        let object = conn.new_id();
        conn.send_with_fd(
            manager,
            MANAGER_IMPORT_TIMELINE,
            &object.to_le_bytes(),
            fd.as_raw_fd(),
        )
        .map_err(io)?;
        conn.syncobj.timelines.insert(
            id,
            Timeline {
                handle,
                object,
                point: 0,
            },
        );
    }
    let object = match conn.syncobj.surfaces.get(&surface) {
        Some(&o) => o,
        None => {
            let o = conn.new_id();
            let mut a = Vec::new();
            arg_u32(&mut a, o);
            arg_u32(&mut a, surface);
            conn.send(manager, MANAGER_GET_SURFACE, &a).map_err(io)?;
            conn.syncobj.surfaces.insert(surface, o);
            o
        }
    };
    let s = &mut conn.syncobj;
    let drm = s
        .drm
        .as_ref()
        .and_then(Option::as_ref)
        .ok_or("no render node")?;
    let t = s.timelines.get_mut(&id).ok_or("no timeline")?;
    let (acquire, release) = (t.point + 1, t.point + 2);
    match fence {
        Some(fence) => drm.import_fence(t.handle, acquire, &fence).map_err(io)?,
        None => drm.signal(t.handle, acquire).map_err(io)?,
    }
    t.point = release;
    let timeline = t.object;
    for (opcode, point) in [
        (SURFACE_SET_ACQUIRE_POINT, acquire),
        (SURFACE_SET_RELEASE_POINT, release),
    ] {
        conn.send(object, opcode, &point_args(timeline, point))
            .map_err(io)?;
    }
    Ok(())
}

/// `set_acquire_point` / `set_release_point`'s arguments.
fn point_args(timeline: u32, point: u64) -> Vec<u8> {
    let mut a = Vec::new();
    arg_u32(&mut a, timeline);
    arg_u32(&mut a, (point >> 32) as u32);
    arg_u32(&mut a, point as u32);
    a
}

/// Drop `surface`'s sync object, if it has one: before a `wl_shm` buffer is
/// attached to it, or it is destroyed.
pub(super) fn forget(conn: &mut WlConn, surface: u32) {
    if let Some(object) = conn.syncobj.surfaces.remove(&surface) {
        let _ = conn.send(object, SURFACE_DESTROY, &[]);
    }
}

/// Drop buffer `id`'s timeline, if it has one.
pub(super) fn destroy_buffer(conn: &mut WlConn, id: u32) {
    if let Some(t) = conn.syncobj.timelines.remove(&id) {
        let _ = conn.send(t.object, TIMELINE_DESTROY, &[]);
        if let Some(Some(drm)) = &conn.syncobj.drm {
            drm.destroy(t.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_numbers_match_drm_h() {
        // From the kernel's drm.h, as compiled for x86-64.
        assert_eq!(IOCTL_GET_CAP, 0xc010_640c);
        assert_eq!(IOCTL_SYNCOBJ_CREATE, 0xc008_64bf);
        assert_eq!(IOCTL_SYNCOBJ_DESTROY, 0xc008_64c0);
        assert_eq!(IOCTL_SYNCOBJ_HANDLE_TO_FD, 0xc010_64c1);
        assert_eq!(IOCTL_SYNCOBJ_FD_TO_HANDLE, 0xc010_64c2);
        assert_eq!(IOCTL_SYNCOBJ_TIMELINE_WAIT, 0xc028_64ca);
        assert_eq!(IOCTL_SYNCOBJ_QUERY, 0xc018_64cb);
        assert_eq!(IOCTL_SYNCOBJ_TRANSFER, 0xc020_64cc);
        assert_eq!(IOCTL_SYNCOBJ_TIMELINE_SIGNAL, 0xc018_64cd);
    }

    #[test]
    fn points_are_split_into_halves() {
        let a = point_args(7, 0x1_0000_0002);
        let words: Vec<u32> = a
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        assert_eq!(words, vec![7, 1, 2]);
    }

    /// The ioctls on a real render node, where there is one that does
    /// timelines (skipped elsewhere).
    #[test]
    fn a_timeline_signals_and_waits() {
        let Some(drm) = Drm::open(None) else {
            eprintln!("no render node with timeline syncobjs: skipped");
            return;
        };
        let t = drm.create().unwrap();
        assert_eq!(drm.query(t), 0);
        // Nothing at point 2 yet: the wait times out.
        assert_eq!(drm.wait_any(&[t], &[2], Duration::from_millis(10)), None);
        drm.signal(t, 2).unwrap();
        assert_eq!(drm.query(t), 2);
        assert_eq!(drm.wait_any(&[t], &[2], Duration::from_millis(10)), Some(0));
        let fd = drm.export(t).unwrap();
        assert!(fd.as_raw_fd() >= 0);
        drm.destroy(t);
    }
}
