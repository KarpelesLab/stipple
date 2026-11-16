//! GPU frames as `dma-buf`s: `zwp_linux_dmabuf_v1`, by hand.
//!
//! A GPU renderer draws a window's frame into an image it can export (a
//! Vulkan image with `dma-buf` memory, a GL texture through
//! `EGL_MESA_image_dma_buf_export`) and hands the file descriptors to the
//! window's [`Sink`](crate::platform::dmabuf::Sink). Here each buffer becomes
//! a `wl_buffer` once (`create_params`, an `add` per plane carrying its fd,
//! `create`, answered by `created` or `failed`), and every frame is that
//! buffer attached, its damage given with `wl_surface.damage_buffer`, a frame
//! callback asked for and the surface committed. The compositor sends
//! `wl_buffer.release` when it no longer reads a buffer; the renderer keeps a
//! few and draws into one that is free.
//!
//! Which formats and modifiers the compositor takes comes from the default
//! feedback object (version 4: a format table shared as an fd, then tranches
//! of indices into it, then `done`), or from the `modifier` events older
//! versions send after the bind (collected up to a `wl_display.sync`). Both
//! are asked for the first time a window wants a GPU layer, waiting for the
//! answer as a paste does: the other messages that come meanwhile are kept
//! for the event loop.
//!
//! The tranches keep the device the compositor reads them with and whether
//! they can be scanned out, and the main device is published for the
//! renderer to pick the same GPU: the feedback is asked for when a window
//! first wants a GPU layer, before the renderer is made.
//!
//! Where the compositor has `wp_linux_drm_syncobj_v1` (`syncobj.rs`), a
//! commit carries the frame's acquire and release points; elsewhere the
//! renderer waits for the GPU before handing a frame over, so the compositor
//! never reads a half-drawn buffer, with or without implicit synchronization
//! on the `dma-buf`. Under a title bar Stipple draws, frames go to a
//! subsurface (`content.rs`).

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use super::{
    MAP_PRIVATE, PROT_READ, Shared, WL_CALLBACK_DONE, WL_DISPLAY, WL_DISPLAY_SYNC,
    WL_SURFACE_ATTACH, WL_SURFACE_COMMIT, WL_SURFACE_SET_BUFFER_SCALE, WlConn, arg_u32, close, dbg,
    mmap, munmap, syncobj,
};
use crate::geometry::PhysicalSize;
use crate::platform::dmabuf::{Buffer, BufferId, MOD_INVALID, Present, Tranche};

/// The global's interface name, and the highest version spoken.
pub(super) const MANAGER: &str = "zwp_linux_dmabuf_v1";
pub(super) const VERSION: u32 = 4;

// zwp_linux_dmabuf_v1 requests, and its events (before version 4).
const CREATE_PARAMS: u16 = 1;
const GET_DEFAULT_FEEDBACK: u16 = 2;
const EV_FORMAT: u16 = 0;
const EV_MODIFIER: u16 = 1;
// zwp_linux_buffer_params_v1 requests and events.
const PARAMS_DESTROY: u16 = 0;
const PARAMS_ADD: u16 = 1;
const PARAMS_CREATE: u16 = 2;
const PARAMS_CREATED: u16 = 0;
const PARAMS_FAILED: u16 = 1;
// zwp_linux_dmabuf_feedback_v1 events.
const FB_DONE: u16 = 0;
const FB_FORMAT_TABLE: u16 = 1;
const FB_MAIN_DEVICE: u16 = 2;
const FB_TRANCHE_DONE: u16 = 3;
const FB_TRANCHE_TARGET_DEVICE: u16 = 4;
const FB_TRANCHE_FORMATS: u16 = 5;
const FB_TRANCHE_FLAGS: u16 = 6;
/// `tranche_flags`: the formats can be scanned out.
const TRANCHE_SCANOUT: u32 = 1;
// wl_buffer: the request to destroy it, and `release`.
const WL_BUFFER_DESTROY: u16 = 0;
const WL_BUFFER_RELEASE: u16 = 0;
/// `wl_surface.damage_buffer` (since `wl_surface` 4): damage in buffer
/// pixels, which a scaled buffer needs no conversion for.
const WL_SURFACE_DAMAGE_BUFFER: u16 = 9;

/// How long to wait for the compositor's answers (formats, a buffer).
const ANSWER: Duration = Duration::from_secs(2);

/// The protocol state, kept on the connection.
#[derive(Debug, Default)]
pub(super) struct Dmabuf {
    /// The bound global and its version, when the compositor has one.
    manager: Option<(u32, u32)>,
    /// The default feedback (version 4), once asked for.
    feedback: Option<u32>,
    /// The `wl_display.sync` that ends the format events (before 4).
    sync: Option<u32>,
    /// The feedback's format table: `(fourcc, modifier)` per index.
    table: Vec<(u32, u64)>,
    /// What the feedback or the events said so far (the tranche being
    /// described, and those done), and the whole list once it is complete
    /// (`asked` and then `known`).
    tranche: Tranche,
    pending: Vec<Tranche>,
    tranches: Vec<Tranche>,
    asked: bool,
    known: bool,
    main_device: Option<u64>,
    /// `create` requests waiting for `created` / `failed`: the answer, once
    /// it came (`Some(id)`, `None` for failed).
    params: HashMap<u32, Option<Option<u32>>>,
    /// The `wl_buffer`s made here, and whether the compositor holds each.
    buffers: HashMap<u32, bool>,
}

/// Keep the bound global.
pub(super) fn setup(conn: &mut WlConn, manager: u32, version: u32) {
    conn.dmabuf.manager = Some((manager, version));
}

/// Whether presenting GPU frames as `dma-buf`s is allowed: on, unless
/// `STIPPLE_WAYLAND_DMABUF=0` (then GPU frames are read back, as before).
pub(super) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("STIPPLE_WAYLAND_DMABUF").map_or(true, |v| v.trim() != "0"))
}

/// Whether the compositor has `zwp_linux_dmabuf_v1`.
pub(super) fn available(conn: &WlConn) -> bool {
    conn.dmabuf.manager.is_some()
}

/// Take `(object, opcode, args)` if it's for one of our objects. `now`:
/// the message was just read, so a file descriptor it carries is the last
/// one queued (the event loop's messages take theirs from the front).
pub(super) fn handle(conn: &mut WlConn, object: u32, opcode: u16, args: &[u8], now: bool) -> bool {
    let word = |i: usize| -> u32 {
        args.get(i..i + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let d = &mut conn.dmabuf;
    if let Some(busy) = d.buffers.get_mut(&object) {
        if opcode == WL_BUFFER_RELEASE {
            *busy = false;
        }
        return true;
    }
    if let Some(answer) = d.params.get_mut(&object) {
        match opcode {
            PARAMS_CREATED => *answer = Some(Some(word(0))),
            PARAMS_FAILED => *answer = Some(None),
            _ => {}
        }
        return true;
    }
    if d.manager.is_some_and(|(m, _)| m == object) {
        match opcode {
            EV_FORMAT if d.feedback.is_none() => d.tranche.formats.push((word(0), MOD_INVALID)),
            EV_MODIFIER if d.feedback.is_none() => {
                let modifier = (u64::from(word(4)) << 32) | u64::from(word(8));
                d.tranche.formats.push((word(0), modifier));
            }
            _ => {}
        }
        return true;
    }
    if d.sync == Some(object) && opcode == WL_CALLBACK_DONE {
        d.sync = None;
        let tranche = std::mem::take(&mut d.tranche);
        d.tranches = finish(vec![tranche]);
        d.known = true;
        return true;
    }
    if d.feedback == Some(object) {
        match opcode {
            FB_FORMAT_TABLE => {
                let fd = if now {
                    conn.fds.pop_back()
                } else {
                    conn.fds.pop_front()
                };
                if let Some(fd) = fd {
                    conn.dmabuf.table = read_table(fd, word(0) as usize);
                }
            }
            FB_MAIN_DEVICE => conn.dmabuf.main_device = dev_t(args),
            FB_TRANCHE_TARGET_DEVICE => conn.dmabuf.tranche.target = dev_t(args),
            FB_TRANCHE_FLAGS => conn.dmabuf.tranche.scanout = word(0) & TRANCHE_SCANOUT != 0,
            FB_TRANCHE_FORMATS => {
                let d = &mut conn.dmabuf;
                let formats = tranche_formats(args, &d.table);
                d.tranche.formats.extend(formats);
            }
            FB_TRANCHE_DONE => {
                let d = &mut conn.dmabuf;
                let tranche = std::mem::take(&mut d.tranche);
                d.pending.push(tranche);
            }
            FB_DONE => {
                let d = &mut conn.dmabuf;
                d.tranches = finish(std::mem::take(&mut d.pending));
                d.known = true;
            }
            _ => {}
        }
        return true;
    }
    false
}

/// The tranches collected, each without repeats or formats an earlier one
/// gave, in the order they came; empty ones dropped.
fn finish(pending: Vec<Tranche>) -> Vec<Tranche> {
    let mut seen: Vec<(u32, u64)> = Vec::new();
    let mut out = Vec::with_capacity(pending.len());
    for mut t in pending {
        t.formats.retain(|f| {
            let new = !seen.contains(f);
            if new {
                seen.push(*f);
            }
            new
        });
        if !t.formats.is_empty() {
            out.push(t);
        }
    }
    out
}

impl Dmabuf {
    /// The compositor's main device, once its feedback said.
    pub(super) fn main_device(&self) -> Option<u64> {
        self.main_device
    }
}

/// A feedback format table: 16 bytes an entry, a 32-bit fourcc, 4 bytes of
/// padding and a 64-bit modifier (native endian, as it's shared memory).
fn parse_table(bytes: &[u8]) -> Vec<(u32, u64)> {
    bytes
        .as_chunks::<16>()
        .0
        .iter()
        .map(|e| {
            let format = u32::from_ne_bytes([e[0], e[1], e[2], e[3]]);
            let modifier = u64::from_ne_bytes(e[8..16].try_into().unwrap());
            (format, modifier)
        })
        .collect()
}

/// Map and read the format table `fd` (`size` bytes); closes the fd.
fn read_table(fd: i32, size: usize) -> Vec<(u32, u64)> {
    if size == 0 {
        unsafe { close(fd) };
        return Vec::new();
    }
    let addr = unsafe { mmap(std::ptr::null_mut(), size, PROT_READ, MAP_PRIVATE, fd, 0) };
    unsafe { close(fd) };
    if addr as isize == -1 {
        return Vec::new();
    }
    let table = parse_table(unsafe { std::slice::from_raw_parts(addr as *const u8, size) });
    unsafe { munmap(addr, size) };
    table
}

/// A `tranche_formats` event: an array (its byte length, then 16-bit indices
/// into the table).
fn tranche_formats(args: &[u8], table: &[(u32, u64)]) -> Vec<(u32, u64)> {
    let len = args
        .get(0..4)
        .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    args.get(4..4 + len)
        .unwrap_or(&[])
        .as_chunks::<2>()
        .0
        .iter()
        .filter_map(|i| {
            table
                .get(u16::from_ne_bytes([i[0], i[1]]) as usize)
                .copied()
        })
        .collect()
}

/// A `main_device` event: an array holding a `dev_t`.
fn dev_t(args: &[u8]) -> Option<u64> {
    let len = u32::from_le_bytes(args.get(0..4)?.try_into().ok()?) as usize;
    let b = args.get(4..4 + len)?;
    match len {
        8 => Some(u64::from_ne_bytes(b.try_into().ok()?)),
        4 => Some(u64::from(u32::from_ne_bytes(b.try_into().ok()?))),
        _ => None,
    }
}

/// `zwp_linux_buffer_params_v1.add`'s arguments (its fd goes alongside):
/// the plane, where it starts, its stride, the modifier's two halves.
fn add_args(plane: u32, offset: u32, stride: u32, modifier: u64) -> Vec<u8> {
    let mut a = Vec::new();
    arg_u32(&mut a, plane);
    arg_u32(&mut a, offset);
    arg_u32(&mut a, stride);
    arg_u32(&mut a, (modifier >> 32) as u32);
    arg_u32(&mut a, modifier as u32);
    a
}

/// `create`'s arguments: size, fourcc, no flags.
fn create_args(width: u32, height: u32, fourcc: u32) -> Vec<u8> {
    let mut a = Vec::new();
    for v in [width, height, fourcc, 0] {
        arg_u32(&mut a, v);
    }
    a
}

/// `damage_buffer`'s arguments for `damage` (`[x0, y0, x1, y1]`) on a
/// `size` buffer: x, y, width, height, clamped to it; all of it for `None`.
fn damage_args(damage: Option<[i32; 4]>, size: (u32, u32)) -> Vec<u8> {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let r = damage.map_or([0, 0, w, h], |r| {
        [
            r[0].clamp(0, w),
            r[1].clamp(0, h),
            r[2].clamp(0, w),
            r[3].clamp(0, h),
        ]
    });
    let mut a = Vec::new();
    for v in [r[0], r[1], (r[2] - r[0]).max(0), (r[3] - r[1]).max(0)] {
        arg_u32(&mut a, v as u32);
    }
    a
}

unsafe extern "C" {
    fn poll(fds: *mut PollFd, n: u64, timeout: i32) -> i32;
}
#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}
const POLLIN: i16 = 1;

/// Read messages until `done` says so or `deadline` passes, taking ours
/// and keeping the others for the event loop. Returns whether `done` did.
fn wait(conn: &mut WlConn, deadline: Instant, done: impl Fn(&WlConn) -> bool) -> bool {
    let _ = conn.stream.flush();
    while !done(conn) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        let mut p = PollFd {
            fd: conn.stream.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        };
        let ms = left.as_millis().clamp(1, i32::MAX as u128) as i32;
        if unsafe { poll(&mut p, 1, ms) } <= 0 {
            continue;
        }
        let Ok((object, opcode, args)) = conn.recv() else {
            return false;
        };
        if !handle(conn, object, opcode, &args, true) {
            conn.queue.push_back((object, opcode, args));
        }
    }
    true
}

/// The formats the compositor takes, by tranche, asking for them the first
/// time (and publishing its main device for the renderer).
pub(super) fn tranches(conn: &mut WlConn) -> Vec<Tranche> {
    let Some((manager, version)) = conn.dmabuf.manager else {
        return Vec::new();
    };
    if !conn.dmabuf.asked {
        conn.dmabuf.asked = true;
        if version >= 4 {
            let feedback = conn.new_id();
            let _ = conn.send(manager, GET_DEFAULT_FEEDBACK, &feedback.to_le_bytes());
            conn.dmabuf.feedback = Some(feedback);
        } else {
            // The format events came after the bind: a round trip ends them.
            let sync = conn.new_id();
            let _ = conn.send(WL_DISPLAY, WL_DISPLAY_SYNC, &sync.to_le_bytes());
            conn.dmabuf.sync = Some(sync);
        }
        let known = wait(conn, Instant::now() + ANSWER, |c| c.dmabuf.known);
        let d = &conn.dmabuf;
        dbg(format_args!(
            "linux-dmabuf v{version}: {} formats in {} tranches{}, main device {:?}",
            d.tranches.iter().map(|t| t.formats.len()).sum::<usize>(),
            d.tranches.len(),
            if known { "" } else { " (no answer)" },
            d.main_device
        ));
        if let Some(dev) = d.main_device {
            crate::platform::dmabuf::set_main_device(dev);
        }
    }
    conn.dmabuf.tranches.clone()
}

/// Make a `wl_buffer` of `buffer`, waiting for the compositor's answer.
fn import(conn: &mut WlConn, buffer: &Buffer) -> Result<u32, String> {
    let (manager, _) = conn.dmabuf.manager.ok_or("no zwp_linux_dmabuf_v1")?;
    let params = conn.new_id();
    let io = |e: std::io::Error| e.to_string();
    conn.send(manager, CREATE_PARAMS, &params.to_le_bytes())
        .map_err(io)?;
    for (i, p) in buffer.planes.iter().enumerate() {
        let a = add_args(i as u32, p.offset, p.stride, buffer.modifier);
        conn.send_with_fd(params, PARAMS_ADD, &a, p.fd)
            .map_err(io)?;
    }
    let a = create_args(buffer.width, buffer.height, buffer.fourcc);
    conn.send(params, PARAMS_CREATE, &a).map_err(io)?;
    conn.dmabuf.params.insert(params, None);
    let answered = wait(conn, Instant::now() + ANSWER, |c| {
        c.dmabuf.params.get(&params).is_some_and(Option::is_some)
    });
    let answer = conn.dmabuf.params.remove(&params).flatten();
    let _ = conn.send(params, PARAMS_DESTROY, &[]);
    let _ = conn.stream.flush();
    match answer {
        Some(Some(id)) => {
            conn.dmabuf.buffers.insert(id, false);
            Ok(id)
        }
        Some(None) => Err("the compositor couldn't import the buffer".into()),
        None if answered => Err("no buffer".into()),
        None => Err("the compositor didn't answer".into()),
    }
}

/// A window's dma-buf presentation.
pub(super) struct WindowSink {
    pub conn: Shared,
    pub toplevel: u32,
    pub scale: std::rc::Rc<std::cell::Cell<u32>>,
    pub configured: std::rc::Rc<std::cell::Cell<bool>>,
    /// Where frames go: the window's surface, or the subsurface under the
    /// decorations Stipple draws.
    pub content: std::rc::Rc<std::cell::RefCell<super::content::Content>>,
    /// The buffers made for it, destroyed with it.
    pub made: std::cell::RefCell<Vec<u32>>,
    /// Whether frames go with explicit sync, decided the first time asked.
    pub explicit: std::cell::Cell<Option<bool>>,
}

impl WindowSink {
    fn explicit(&self) -> bool {
        if let Some(e) = self.explicit.get() {
            return e;
        }
        let e = syncobj::available(&mut self.conn.lock().unwrap());
        dbg(format_args!("explicit sync: {e}"));
        self.explicit.set(Some(e));
        e
    }
}

impl Present for WindowSink {
    fn tranches(&self) -> Vec<Tranche> {
        tranches(&mut self.conn.lock().unwrap())
    }

    fn explicit_sync(&self) -> bool {
        self.explicit()
    }

    fn ready(&self) -> bool {
        self.configured.get()
    }

    fn import(&self, buffer: &Buffer) -> Result<BufferId, String> {
        let id = import(&mut self.conn.lock().unwrap(), buffer)?;
        self.made.borrow_mut().push(id);
        Ok(id)
    }

    fn released(&self, id: BufferId) -> bool {
        let conn = self.conn.lock().unwrap();
        // With explicit sync, when its release point has signalled.
        syncobj::released(&conn, id).unwrap_or(conn.dmabuf.buffers.get(&id) == Some(&false))
    }

    fn wait_release(&self, ids: &[BufferId], timeout: Duration) -> Option<BufferId> {
        if self.explicit() {
            let conn = self.conn.lock().unwrap();
            return syncobj::wait_release(&conn, ids, timeout);
        }
        let mut conn = self.conn.lock().unwrap();
        let free = |c: &WlConn| {
            ids.iter()
                .copied()
                .find(|id| c.dmabuf.buffers.get(id) == Some(&false))
        };
        wait(&mut conn, Instant::now() + timeout, |c| free(c).is_some());
        free(&conn)
    }

    fn commit(
        &self,
        id: BufferId,
        size: (u32, u32),
        damage: Option<[i32; 4]>,
        fence: Option<std::os::fd::OwnedFd>,
    ) -> Result<(), String> {
        let explicit = self.explicit();
        let mut conn = self.conn.lock().unwrap();
        let scale = self.scale.get().max(1);
        let logical = PhysicalSize::new(size.0 / scale, size.1 / scale);
        super::toplevel::use_decor(&mut conn, self.toplevel);
        let io = |e: std::io::Error| e.to_string();
        // Under a title bar Stipple draws, the subsurface; else the window's
        // surface, the buffer all of the window's geometry.
        let mut content = self.content.borrow_mut();
        let surface = content.target(&mut conn).map_err(io)?;
        let main = surface == content.main();
        if main {
            super::decor::send_geometry(&mut conn, logical);
        }
        conn.send(surface, WL_SURFACE_SET_BUFFER_SCALE, &scale.to_le_bytes())
            .map_err(io)?;
        if explicit {
            syncobj::set_points(&mut conn, surface, id, fence)?;
        }
        let mut a = Vec::new();
        for v in [id, 0, 0] {
            arg_u32(&mut a, v);
        }
        conn.send(surface, WL_SURFACE_ATTACH, &a).map_err(io)?;
        conn.send(
            surface,
            WL_SURFACE_DAMAGE_BUFFER,
            &damage_args(damage, size),
        )
        .map_err(io)?;
        super::request_frame_callback(&mut conn, surface);
        conn.send(surface, WL_SURFACE_COMMIT, &[]).map_err(io)?;
        if !main {
            content.finish(&mut conn, logical, scale).map_err(io)?;
        }
        conn.stream.flush().map_err(io)?;
        if let Some(busy) = conn.dmabuf.buffers.get_mut(&id) {
            *busy = true;
        }
        dbg(format_args!(
            "present dma-buf {id} {}x{} damage {damage:?}",
            size.0, size.1
        ));
        Ok(())
    }

    fn destroy(&self, id: BufferId) {
        self.made.borrow_mut().retain(|&b| b != id);
        let mut conn = self.conn.lock().unwrap();
        destroy_buffer(&mut conn, id);
        let _ = conn.stream.flush();
    }
}

fn destroy_buffer(conn: &mut WlConn, id: u32) {
    syncobj::destroy_buffer(conn, id);
    if conn.dmabuf.buffers.remove(&id).is_some() {
        let _ = conn.send(id, WL_BUFFER_DESTROY, &[]);
    }
}

impl Drop for WindowSink {
    fn drop(&mut self) {
        let made = std::mem::take(&mut *self.made.borrow_mut());
        if made.is_empty() {
            return;
        }
        if let Ok(mut conn) = self.conn.lock() {
            for id in made {
                destroy_buffer(&mut conn, id);
            }
            let _ = conn.stream.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::dmabuf::{ABGR8888, MOD_LINEAR, XBGR8888};

    #[test]
    fn format_table_entries_are_sixteen_bytes() {
        let mut bytes = Vec::new();
        for (f, m) in [(XBGR8888, MOD_LINEAR), (ABGR8888, 0x0100_0000_0000_0001u64)] {
            bytes.extend_from_slice(&f.to_ne_bytes());
            bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&m.to_ne_bytes());
        }
        let table = parse_table(&bytes);
        assert_eq!(
            table,
            vec![(XBGR8888, MOD_LINEAR), (ABGR8888, 0x0100_0000_0000_0001)]
        );
        // A tranche: an array of 16-bit indices (here 1, 0 and one past the
        // end, which is skipped).
        let mut args = Vec::new();
        args.extend_from_slice(&6u32.to_le_bytes());
        for i in [1u16, 0, 7] {
            args.extend_from_slice(&i.to_ne_bytes());
        }
        args.extend_from_slice(&[0; 2]); // padding to 4
        assert_eq!(tranche_formats(&args, &table), vec![table[1], table[0]]);
    }

    #[test]
    fn formats_are_kept_in_order_without_repeats() {
        let t = |scanout, formats: Vec<(u32, u64)>| Tranche {
            target: Some(0xe280),
            scanout,
            formats,
        };
        let pending = vec![
            t(true, vec![(1, 0), (2, 0), (1, 0)]),
            t(false, vec![(1, 0), (1, 5)]),
            t(false, vec![(2, 0)]),
        ];
        assert_eq!(
            finish(pending),
            vec![t(true, vec![(1, 0), (2, 0)]), t(false, vec![(1, 5)])]
        );
    }

    #[test]
    fn feedback_tranches_carry_their_device_and_flags() {
        let mut conn = test_conn(Dmabuf {
            manager: Some((9, 4)),
            feedback: Some(40),
            table: vec![(XBGR8888, MOD_LINEAR), (ABGR8888, 7)],
            ..Dmabuf::default()
        });
        let dev = |d: u64| {
            let mut a = 8u32.to_le_bytes().to_vec();
            a.extend_from_slice(&d.to_ne_bytes());
            a
        };
        let indices = |i: &[u16]| {
            let mut a = ((i.len() * 2) as u32).to_le_bytes().to_vec();
            for i in i {
                a.extend_from_slice(&i.to_ne_bytes());
            }
            while !a.len().is_multiple_of(4) {
                a.push(0);
            }
            a
        };
        for (opcode, args) in [
            (FB_MAIN_DEVICE, dev(0xe280)),
            (FB_TRANCHE_TARGET_DEVICE, dev(0xe280)),
            (FB_TRANCHE_FLAGS, 1u32.to_le_bytes().to_vec()),
            (FB_TRANCHE_FORMATS, indices(&[1])),
            (FB_TRANCHE_DONE, Vec::new()),
            (FB_TRANCHE_TARGET_DEVICE, dev(0xe280)),
            (FB_TRANCHE_FLAGS, 0u32.to_le_bytes().to_vec()),
            (FB_TRANCHE_FORMATS, indices(&[0, 1])),
            (FB_TRANCHE_DONE, Vec::new()),
            (FB_DONE, Vec::new()),
        ] {
            assert!(handle(&mut conn, 40, opcode, &args, true));
        }
        let d = &conn.dmabuf;
        assert!(d.known);
        assert_eq!(d.main_device(), Some(0xe280));
        assert_eq!(
            d.tranches,
            vec![
                Tranche {
                    target: Some(0xe280),
                    scanout: true,
                    formats: vec![(ABGR8888, 7)],
                },
                Tranche {
                    target: Some(0xe280),
                    scanout: false,
                    formats: vec![(XBGR8888, MOD_LINEAR)],
                },
            ]
        );
    }

    #[test]
    fn main_device_is_a_dev_t_array() {
        let mut args = 8u32.to_le_bytes().to_vec();
        args.extend_from_slice(&0xe280u64.to_ne_bytes());
        assert_eq!(dev_t(&args), Some(0xe280));
        assert_eq!(dev_t(&[0, 0, 0, 0]), None);
    }

    #[test]
    fn requests_encode_as_the_protocol_says() {
        let words = |a: &[u8]| -> Vec<u32> {
            a.as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes(*b))
                .collect()
        };
        // add: plane, offset, stride, modifier_hi, modifier_lo.
        assert_eq!(
            words(&add_args(1, 64, 4096, 0x0300_0000_0000_0e08)),
            vec![1, 64, 4096, 0x0300_0000, 0x0e08]
        );
        // create: width, height, format, flags.
        assert_eq!(
            words(&create_args(640, 480, XBGR8888)),
            vec![640, 480, XBGR8888, 0]
        );
        // damage_buffer: x, y, width, height, clamped to the buffer.
        assert_eq!(
            words(&damage_args(Some([10, 20, 30, 50]), (100, 40))),
            vec![10, 20, 20, 20]
        );
        assert_eq!(words(&damage_args(None, (100, 40))), vec![0, 0, 100, 40]);
    }

    #[test]
    fn events_for_our_objects_are_taken() {
        let mut d = Dmabuf {
            manager: Some((9, 3)),
            ..Dmabuf::default()
        };
        d.buffers.insert(0xff00_0001, true);
        d.params.insert(20, None);
        let mut conn = test_conn(d);
        // wl_buffer.release frees it.
        assert!(handle(&mut conn, 0xff00_0001, WL_BUFFER_RELEASE, &[], true));
        assert!(!conn.dmabuf.buffers[&0xff00_0001]);
        // created(new_id) answers the params.
        assert!(handle(
            &mut conn,
            20,
            PARAMS_CREATED,
            &0xff00_0002u32.to_le_bytes(),
            true
        ));
        assert_eq!(conn.dmabuf.params[&20], Some(Some(0xff00_0002)));
        // A modifier event (version 3): format, modifier hi, lo.
        let mut a = Vec::new();
        for v in [XBGR8888, 0, 0] {
            arg_u32(&mut a, v);
        }
        assert!(handle(&mut conn, 9, EV_MODIFIER, &a, true));
        conn.dmabuf.sync = Some(30);
        assert!(handle(&mut conn, 30, WL_CALLBACK_DONE, &[0; 4], true));
        assert!(conn.dmabuf.known);
        assert_eq!(
            crate::platform::dmabuf::flatten(&conn.dmabuf.tranches),
            vec![(XBGR8888, MOD_LINEAR)]
        );
        // Someone else's.
        assert!(!handle(&mut conn, 31, 0, &[], true));
    }

    /// A connection over a socket pair, for the handlers.
    fn test_conn(dmabuf: Dmabuf) -> WlConn {
        let (mut conn, _) = super::super::clipboard::tests::conn_pair();
        conn.dmabuf = dmabuf;
        conn
    }
}
