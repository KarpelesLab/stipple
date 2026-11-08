//! A native X11 backend implemented directly against the wire protocol.
//!
//! No `xcb`/`x11` crate, no `libX11` FFI — just a `UnixStream` and the X11
//! byte protocol (the workspace policy in `ROADMAP.md` §1). It connects to
//! `$DISPLAY`, creates a top-level window, and presents the software [`Pixmap`].
//!
//! Scope: window creation, resize, close, pointer (move + buttons + wheel),
//! keyboard, and drops from other applications (XDND, see `dnd`) —
//! `GetKeyboardMapping` resolves keycodes to keysyms, which
//! become [`Event::Text`] (Latin-1 + Unicode) or editing [`Event::Key`]s.
//!
//! Presentation has two paths: when the server advertises the **MIT-SHM**
//! extension, frames are copied into a System V shared-memory segment the
//! server maps directly and blitted with `ShmPutImage` — limited to the
//! [`Surface::present`] damage rectangles, so a small change uploads no pixels
//! over the socket at all. Otherwise it falls back to plain `PutImage`. The shm
//! segment is the one place this otherwise-safe backend needs FFI (the SysV
//! `shm*` syscalls), hence the module-level `allow(unsafe_code)`.
//!
//! **Verification:** the pure codec (setup-reply / `$DISPLAY` / `.Xauthority`
//! parsing, RGBA→X11 conversion) is unit-tested below; the live socket path —
//! handshake, window mapping, present (both shm and `PutImage`), pointer +
//! keyboard input — is exercised by the `Visual` workflow's Xvfb jobs
//! (screenshot + `xdotool` click/type); Xvfb supports MIT-SHM, so CI covers the
//! shm path.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::io::{self, Read, Write};
use std::os::fd::RawFd;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::geometry::{PhysicalSize, Point, ScaleFactor, Size};
use crate::platform::ControlFlow;
use crate::platform::error::PlatformError;
use crate::platform::event::{
    ButtonState, Event, KeyCode, Modifiers, PointerButton, ScrollDelta, WindowId,
};
use crate::platform::window::{CursorIcon, Window, WindowAttributes, fit_icon};
use crate::render::{Pixmap, Surface};

/// The monitor refresh rate, which paces animation frames.
/// Receiving drops (XDND).
mod dnd;
mod refresh;

// ---- X11 protocol constants -------------------------------------------------

const OP_CREATE_WINDOW: u8 = 1;
const OP_CHANGE_WINDOW_ATTRIBUTES: u8 = 2;
const OP_DESTROY_WINDOW: u8 = 4;
const OP_MAP_WINDOW: u8 = 8;
const OP_INTERN_ATOM: u8 = 16;
const OP_GET_PROPERTY: u8 = 20;
const OP_CHANGE_PROPERTY: u8 = 18;
const OP_DELETE_PROPERTY: u8 = 19;
const OP_SET_SELECTION_OWNER: u8 = 22;
const OP_SEND_EVENT: u8 = 25;
const OP_GET_INPUT_FOCUS: u8 = 43;
const OP_QUERY_EXTENSION: u8 = 98;
const OP_SET_INPUT_FOCUS: u8 = 42;
const OP_OPEN_FONT: u8 = 45;
const OP_CREATE_GLYPH_CURSOR: u8 = 94;
const OP_CREATE_GC: u8 = 55;
const OP_PUT_IMAGE: u8 = 72;
const OP_GET_KEYBOARD_MAPPING: u8 = 101;

// MIT-SHM minor opcodes (within the extension's major opcode).
const SHM_ATTACH: u8 = 1;
const SHM_PUT_IMAGE: u8 = 3;

// System V IPC flags for the shared-memory segment.
const IPC_PRIVATE: i32 = 0;
const IPC_CREAT: i32 = 0o1000;
const IPC_RMID: i32 = 0;
const SHM_PERM: i32 = 0o600;

// Event masks.
const EV_EXPOSURE: u32 = 0x0000_8000;
const EV_KEY_PRESS: u32 = 0x0000_0001;
const EV_KEY_RELEASE: u32 = 0x0000_0002;
const EV_BUTTON_PRESS: u32 = 0x0000_0004;
const EV_BUTTON_RELEASE: u32 = 0x0000_0008;
const EV_POINTER_MOTION: u32 = 0x0000_0040;
const EV_STRUCTURE_NOTIFY: u32 = 0x0002_0000;

// Value-mask bits for CreateWindow / ChangeWindowAttributes.
const CW_BACK_PIXEL: u32 = 0x0000_0002;
const CW_EVENT_MASK: u32 = 0x0000_0800;
const CW_CURSOR: u32 = 0x0000_4000;

// The root-window event mask a window manager listens on for client requests
// such as `_NET_WM_STATE` changes (SubstructureNotify | SubstructureRedirect).
const EV_SUBSTRUCTURE: u32 = 0x0018_0000;

// Event codes (low 7 bits of the first byte).
const X_EXPOSE: u8 = 12;
const X_BUTTON_PRESS: u8 = 4;
const X_BUTTON_RELEASE: u8 = 5;
const X_MOTION_NOTIFY: u8 = 6;
const X_KEY_PRESS: u8 = 2;
const X_KEY_RELEASE: u8 = 3;
const X_CONFIGURE_NOTIFY: u8 = 22;
const X_SELECTION_CLEAR: u8 = 29;
const X_SELECTION_REQUEST: u8 = 30;
const X_SELECTION_NOTIFY: u8 = 31;
const X_CLIENT_MESSAGE: u8 = 33;
/// Sent to every client when the keyboard mapping changes (a layout switch,
/// `xmodmap`, or `xdotool` borrowing a spare keycode for a keysym).
const X_MAPPING_NOTIFY: u8 = 34;

// Predefined atoms (X11 protocol appendix): STRING type and the ATOM type.
const ATOM_STRING: u32 = 31;
const ATOM_ATOM: u32 = 4;
const ATOM_CARDINAL: u32 = 6;
const ATOM_WM_NORMAL_HINTS: u32 = 40;
const ATOM_WM_SIZE_HINTS: u32 = 41;

// WM_SIZE_HINTS flags (ICCCM 4.1.2.3).
const P_MIN_SIZE: u32 = 16;
const P_MAX_SIZE: u32 = 32;

/// `_NET_WM_ICON` is sent in one `ChangeProperty`, whose length (without
/// BIG-REQUESTS) is capped at 65535 words, so larger icons are scaled to this.
const MAX_ICON: u32 = 128;

/// The glyph in the core `cursor` font for each [`CursorIcon`] (from
/// `X11/cursorfont.h`; the mask is the next glyph).
/// These are the classic X shapes, not the desktop cursor theme.
fn cursor_glyph(icon: CursorIcon) -> u16 {
    match icon {
        CursorIcon::Default => 68,    // XC_left_ptr
        CursorIcon::Text => 152,      // XC_xterm
        CursorIcon::Pointer => 60,    // XC_hand2
        CursorIcon::Crosshair => 34,  // XC_crosshair
        CursorIcon::NotAllowed => 24, // XC_circle
        CursorIcon::Grab => 58,       // XC_hand1
        CursorIcon::Grabbing => 52,   // XC_fleur
        CursorIcon::Move => 52,       // XC_fleur
        CursorIcon::EwResize => 108,  // XC_sb_h_double_arrow
        CursorIcon::NsResize => 116,  // XC_sb_v_double_arrow
        CursorIcon::NwseResize => 14, // XC_bottom_right_corner
        CursorIcon::NeswResize => 12, // XC_bottom_left_corner
    }
}

/// Whether an X11 display is reachable (cheap check of `$DISPLAY`).
pub fn available() -> bool {
    std::env::var_os("DISPLAY").is_some()
}

/// Emit a diagnostic line to stderr when `STIPPLE_X11_DEBUG` is set. Used to
/// trace the live socket path (which CI exercises) without spamming normal use.
fn dbg(args: std::fmt::Arguments<'_>) {
    if std::env::var_os("STIPPLE_X11_DEBUG").is_some() {
        // Milliseconds since the first line, to line up with the app's.
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let ms = START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis();
        eprintln!("stipple x11: [{ms} ms] {args}");
    }
}

/// Parsed `$DISPLAY` (the local/unix-socket case).
#[derive(Debug, PartialEq, Eq)]
struct DisplayAddr {
    /// Display number (the `0` in `:0`).
    number: u32,
    /// Screen number (rarely non-zero); kept for completeness.
    screen: u32,
}

fn parse_display(display: &str) -> Option<DisplayAddr> {
    // Forms: ":0", ":0.1", "hostname:0". We support the local unix path, so we
    // only need the part after the last ':'.
    let after = display.rsplit(':').next()?;
    let mut parts = after.splitn(2, '.');
    let number: u32 = parts.next()?.parse().ok()?;
    let screen: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Some(DisplayAddr { number, screen })
}

/// The fields we need from the X11 connection-setup reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Setup {
    resource_id_base: u32,
    resource_id_mask: u32,
    root: u32,
    root_visual: u32,
    root_depth: u8,
    /// Server's max request length, in 4-byte units.
    max_request_len: u32,
    min_keycode: u8,
    max_keycode: u8,
}

/// Parse the additional-data portion of a successful setup reply (everything
/// after the first 8-byte header).
fn parse_setup(data: &[u8]) -> Option<Setup> {
    // Offsets within the additional data (see X11 protocol §Connection Setup).
    let resource_id_base = rd_u32(data, 4)?;
    let resource_id_mask = rd_u32(data, 8)?;
    let max_request_len = rd_u16(data, 18)? as u32;
    let vendor_len = rd_u16(data, 16)? as usize;
    let num_formats = *data.get(21)? as usize;

    // Skip the 32-byte fixed block, then vendor (padded to 4), then the
    // pixmap-formats list (8 bytes each), to reach the first SCREEN.
    let vendor_pad = (vendor_len + 3) & !3;
    let screens_off = 32 + vendor_pad + num_formats * 8;

    let min_keycode = *data.get(26)?;
    let max_keycode = *data.get(27)?;

    let root = rd_u32(data, screens_off)?;
    let root_visual = rd_u32(data, screens_off + 32)?;
    let root_depth = *data.get(screens_off + 38)?;

    Some(Setup {
        resource_id_base,
        resource_id_mask,
        root,
        root_visual,
        root_depth,
        max_request_len,
        min_keycode,
        max_keycode,
    })
}

fn rd_u16(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(off)?, *b.get(off + 1)?]))
}
fn rd_u32(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(off)?,
        *b.get(off + 1)?,
        *b.get(off + 2)?,
        *b.get(off + 3)?,
    ]))
}

/// Convert a straight-RGBA8 pixmap row into X11 ZPixmap bytes for a 24-depth
/// little-endian (LSBFirst) server: each pixel becomes `[B, G, R, 0]`.
fn rgba_to_bgrx(rgba: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(rgba.len());
    for px in rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], 0]);
    }
}

/// Swizzle a straight-RGBA8 row in place into a ZPixmap (`[B, G, R, 0]`) row of
/// equal pixel count. Used to fill the shared-memory image without allocating.
fn swizzle_row(src: &[u8], dst: &mut [u8]) {
    for (s, d) in src
        .as_chunks::<4>()
        .0
        .iter()
        .zip(dst.as_chunks_mut::<4>().0.iter_mut())
    {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = 0;
    }
}

// ---- MIT-SHM shared-memory presentation -------------------------------------

use core::ffi::c_void;

// System V shared-memory syscalls (libc, linked by default on Linux). Declared
// directly to keep the crate dependency-free; this is the only FFI in the X11
// backend.
unsafe extern "C" {
    fn shmget(key: i32, size: usize, shmflg: i32) -> i32;
    fn shmat(shmid: i32, shmaddr: *const c_void, shmflg: i32) -> *mut c_void;
    fn shmdt(shmaddr: *const c_void) -> i32;
    fn shmctl(shmid: i32, cmd: i32, buf: *mut c_void) -> i32;
}

/// A System V shared-memory segment attached to both this process and the X
/// server, used as the backing image for `ShmPutImage`.
#[derive(Debug)]
struct ShmState {
    /// MIT-SHM extension major opcode.
    major: u8,
    /// X resource id naming the attached segment (`ShmSeg`).
    seg: u32,
    /// Mapped address in this process.
    ptr: *mut u8,
    /// Bytes available at `ptr`.
    capacity: usize,
}

impl Drop for ShmState {
    fn drop(&mut self) {
        // Detach our mapping; the kernel frees the segment once the server
        // (which detaches on connection close) has also let go.
        unsafe {
            shmdt(self.ptr as *const c_void);
        }
    }
}

impl ShmState {
    /// The mapped segment as a mutable byte slice.
    fn buffer(&mut self) -> &mut [u8] {
        // Safe: we own this mapping for `capacity` bytes; access is single-
        // threaded (present runs on the event-loop thread).
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.capacity) }
    }

    /// Copy the damaged regions of `pixmap` into the segment and blit them with
    /// `ShmPutImage`. Returns `false` if the frame doesn't fit (caller should
    /// fall back to `PutImage`); `true` on a successful (queued) present.
    fn present(
        &mut self,
        conn: &mut Conn,
        window: u32,
        gc: u32,
        pixmap: &Pixmap,
        damage: &[crate::geometry::Rect],
    ) -> bool {
        let size = pixmap.size();
        let (w, h) = (size.width as usize, size.height as usize);
        let stride = w * 4;
        if stride * h > self.capacity {
            return false; // grew past our segment; let PutImage handle it
        }
        let (major, seg) = (self.major, self.seg);
        let src = pixmap.as_bytes();
        let dst = self.buffer();

        // Build the integer regions to upload: explicit damage, or the whole
        // frame when none is given (first frame / expose / resize).
        let mut regions: Vec<(usize, usize, usize, usize)> = Vec::new();
        if damage.is_empty() {
            regions.push((0, 0, w, h));
        } else {
            for r in damage {
                let x0 = (r.min_x().floor().max(0.0) as usize).min(w);
                let y0 = (r.min_y().floor().max(0.0) as usize).min(h);
                let x1 = (r.max_x().ceil().max(0.0) as usize).min(w);
                let y1 = (r.max_y().ceil().max(0.0) as usize).min(h);
                if x1 > x0 && y1 > y0 {
                    regions.push((x0, y0, x1 - x0, y1 - y0));
                }
            }
        }

        for &(x, y, rw, rh) in &regions {
            // Copy each row's sub-span into the shared image (matching strides).
            for row in y..y + rh {
                let off = row * stride + x * 4;
                let len = rw * 4;
                swizzle_row(&src[off..off + len], &mut dst[off..off + len]);
            }
            if conn
                .send(&finish(shm_put_image(
                    major, window, gc, seg, w, h, x, y, rw, rh,
                )))
                .is_err()
            {
                return false;
            }
        }
        true
    }
}

/// Encode a `ShmPutImage` request blitting the sub-rectangle `(x, y, rw, rh)` of
/// a `total_w × total_h` ZPixmap image (depth 24) from the attached segment to
/// the drawable at the same position.
#[allow(clippy::too_many_arguments)]
fn shm_put_image(
    major: u8,
    drawable: u32,
    gc: u32,
    seg: u32,
    total_w: usize,
    total_h: usize,
    x: usize,
    y: usize,
    rw: usize,
    rh: usize,
) -> Vec<u8> {
    let mut req = vec![major, SHM_PUT_IMAGE, 0, 0];
    req.extend_from_slice(&drawable.to_le_bytes());
    req.extend_from_slice(&gc.to_le_bytes());
    req.extend_from_slice(&(total_w as u16).to_le_bytes());
    req.extend_from_slice(&(total_h as u16).to_le_bytes());
    req.extend_from_slice(&(x as u16).to_le_bytes()); // src-x
    req.extend_from_slice(&(y as u16).to_le_bytes()); // src-y
    req.extend_from_slice(&(rw as u16).to_le_bytes()); // src-width
    req.extend_from_slice(&(rh as u16).to_le_bytes()); // src-height
    req.extend_from_slice(&(x as i16).to_le_bytes()); // dst-x
    req.extend_from_slice(&(y as i16).to_le_bytes()); // dst-y
    req.push(24); // depth
    req.push(2); // format: ZPixmap
    req.push(0); // send-event: false
    req.push(0); // unused
    req.extend_from_slice(&seg.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // offset into segment
    req
}

/// Query whether the server supports an extension, returning its major opcode.
/// Must be called before the window is mapped, so the reply doesn't interleave
/// with events.
fn query_extension(conn: &mut Conn, name: &[u8]) -> io::Result<Option<u8>> {
    let mut req = vec![OP_QUERY_EXTENSION, 0, 0, 0];
    req.extend_from_slice(&(name.len() as u16).to_le_bytes());
    req.extend_from_slice(&[0u8; 2]); // unused
    req.extend_from_slice(name);
    pad4(&mut req);
    conn.send(&finish(req))?;
    let mut pkt = [0u8; 32];
    conn.stream.read_exact(&mut pkt)?;
    // Reply: byte 8 = present (bool), byte 9 = major-opcode.
    if pkt[0] != 1 || pkt[8] == 0 {
        return Ok(None);
    }
    Ok(Some(pkt[9]))
}

// ---- DRI3: GPU buffer sharing with the X server -----------------------------
//
// DRI3 + Present is how a GPU frame reaches the window with no readback (the
// chosen Phase-B path): render to a dma-buf, wrap it as an X Pixmap via
// DRI3 `PixmapFromBuffers` (the fd sent over this socket with SCM_RIGHTS), then
// flip it to the window via the Present extension. `dri3_open` is the first step
// — it returns the server's DRM device fd, so our GPU context can render on the
// same device the server can import from. The fd arrives as ancillary data on
// the reply, received via `crate::platform::scm`.

const DRI3_QUERY_VERSION: u8 = 0;
const DRI3_OPEN: u8 = 1;

/// Build a `DRI3Open` request (minor opcode 1): drawable + provider (0 = pick).
fn dri3_open_request(major: u8, drawable: u32) -> Vec<u8> {
    let mut req = vec![major, DRI3_OPEN, 0, 0];
    req.extend_from_slice(&drawable.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // provider
    finish(req)
}

/// Negotiate DRI3 and open the server's DRM device, returning its fd. Returns
/// `Ok(None)` if the server has no DRI3 extension. The fd is owned by the caller.
fn dri3_open(conn: &mut Conn, drawable: u32) -> io::Result<Option<RawFd>> {
    let Some(major) = query_extension(conn, b"DRI3")? else {
        return Ok(None);
    };

    // DRI3QueryVersion — negotiate (we want >= 1.2 for PixmapFromBuffers later).
    let mut req = vec![major, DRI3_QUERY_VERSION, 0, 0];
    req.extend_from_slice(&1u32.to_le_bytes()); // client major
    req.extend_from_slice(&2u32.to_le_bytes()); // client minor
    conn.send(&finish(req))?;
    let mut reply = [0u8; 32];
    conn.stream.read_exact(&mut reply)?;
    if reply[0] != 1 {
        return Ok(None); // error reply
    }

    // DRI3Open — reply carries the DRM fd as ancillary data (nfd in byte 1).
    conn.send(&dri3_open_request(major, drawable))?;
    let mut reply = [0u8; 32];
    let mut fds = Vec::new();
    let n = crate::platform::scm::recv_with_fds(&conn.stream, &mut reply, &mut fds, 1)?;
    // The 32-byte reply may arrive split from the ancillary chunk; read the rest.
    if n < 32 {
        conn.stream.read_exact(&mut reply[n..])?;
    }
    if reply[0] != 1 {
        for fd in fds {
            unsafe { libc_close_fd(fd) };
        }
        return Ok(None);
    }
    Ok(fds.first().copied())
}

/// Connect to `$DISPLAY`, negotiate DRI3, and open the server's DRM device,
/// returning its fd (owned by the caller). `None` if the server lacks DRI3.
/// Maps no window (DRI3Open targets the root drawable), so it's a benign query.
/// The fd can be handed to `stipple::gpu` to bind EGL/GBM to the server's GPU.
pub fn dri3_open_drm_fd() -> Result<Option<RawFd>, PlatformError> {
    let mut conn = Conn::connect()?;
    let root = conn.setup.root;
    dri3_open(&mut conn, root).map_err(os)
}

/// As [`dri3_open_drm_fd`] but closes the fd and returns a human-readable result.
pub fn dri3_open_probe() -> Result<String, PlatformError> {
    match dri3_open_drm_fd()? {
        Some(fd) => {
            let msg = format!("DRI3Open ok: DRM device fd = {fd}");
            unsafe { libc_close_fd(fd) };
            Ok(msg)
        }
        None => Ok("DRI3 unavailable on this server".to_string()),
    }
}

unsafe extern "C" {
    #[link_name = "close"]
    fn libc_close_fd(fd: RawFd) -> i32;
}

// ---- DRI3 PixmapFromBuffers + Present: flip a GPU frame with no readback -----
//
// The last hop of the zero-copy path: wrap a rendered dma-buf as an X Pixmap via
// DRI3 `PixmapFromBuffers` (the plane fds sent over this socket with SCM_RIGHTS,
// echoing the format modifier so tiled buffers import correctly), then flip that
// Pixmap to the window with the Present extension's `PresentPixmap`. The
// `PixmapFromBuffers` step needs a real GPU (the server imports the dma-buf), so
// it's hardware-gated; the Present extension itself needs no GPU (it works under
// Xvfb), so its negotiation is CI-verifiable. The wire encoders below are
// unit-tested regardless.

const DRI3_PIXMAP_FROM_BUFFERS: u8 = 7;
const DRI3_FENCE_FROM_FD: u8 = 4;
const PRESENT_QUERY_VERSION: u8 = 0;
const PRESENT_PIXMAP: u8 = 1;

/// A rendered dma-buf to wrap as an X Pixmap via DRI3 `PixmapFromBuffers`. Holds
/// the geometry + format the server needs to interpret the (1..=4) planes; the
/// plane fds are passed separately as SCM_RIGHTS ancillary data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmabufImage {
    pub width: u16,
    pub height: u16,
    /// Pixmap depth (e.g. 24 for XRGB8888, 32 for ARGB8888).
    pub depth: u8,
    /// Bits per pixel (e.g. 32).
    pub bpp: u8,
    /// DRM format modifier — must echo the export's modifier (e.g. from
    /// `EGL_DMA_BUF_PLANE0_MODIFIER_*`) or a tiled buffer imports as garbage.
    pub modifier: u64,
    /// Per-plane `(stride, offset)` in bytes; 1..=4 planes, parallel to the fds.
    pub planes: Vec<(u32, u32)>,
}

/// Build a DRI3 `PixmapFromBuffers` request (minor 7): wrap the dma-buf planes
/// of `img` (whose fds are sent alongside as ancillary data via
/// [`crate::platform::scm::send_with_fds`]) as the X resource `pixmap`, in `window`'s
/// screen. Absent planes (of the 4 wire slots) are zero-filled.
pub fn pixmap_from_buffers_request(
    major: u8,
    pixmap: u32,
    window: u32,
    img: &DmabufImage,
) -> Vec<u8> {
    let mut req = vec![major, DRI3_PIXMAP_FROM_BUFFERS, 0, 0];
    req.extend_from_slice(&pixmap.to_le_bytes());
    req.extend_from_slice(&window.to_le_bytes());
    req.push(img.planes.len() as u8);
    req.extend_from_slice(&[0u8; 3]); // pad
    req.extend_from_slice(&img.width.to_le_bytes());
    req.extend_from_slice(&img.height.to_le_bytes());
    // stride0,offset0 .. stride3,offset3 (zero-filled for absent planes).
    for i in 0..4 {
        let (stride, offset) = img.planes.get(i).copied().unwrap_or((0, 0));
        req.extend_from_slice(&stride.to_le_bytes());
        req.extend_from_slice(&offset.to_le_bytes());
    }
    req.push(img.depth);
    req.push(img.bpp);
    req.extend_from_slice(&[0u8; 2]); // pad
    req.extend_from_slice(&img.modifier.to_le_bytes());
    finish(req)
}

/// Build a Present `PresentPixmap` request (minor 1): present `pixmap` to
/// `window`. `wait_fence` is an `XSyncFence` the server waits on before reading
/// the pixmap (`0` = none) — pass a fence the producer triggers when its GPU
/// render finishes (see [`dri3_fence_from_fd_request`]) for **frame sync**, so
/// the compositor never samples a half-rendered buffer. Regions, target CRTC,
/// the idle fence, and target MSC/divisor/remainder are left as 0 — a plain
/// immediate present; `serial` and `options` pass through. No notify list.
pub fn present_pixmap_request(
    major: u8,
    window: u32,
    pixmap: u32,
    wait_fence: u32,
    serial: u32,
    options: u32,
) -> Vec<u8> {
    let mut req = vec![major, PRESENT_PIXMAP, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&pixmap.to_le_bytes());
    req.extend_from_slice(&serial.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // valid region (None)
    req.extend_from_slice(&0u32.to_le_bytes()); // update region (None)
    req.extend_from_slice(&0i16.to_le_bytes()); // x-off
    req.extend_from_slice(&0i16.to_le_bytes()); // y-off
    req.extend_from_slice(&0u32.to_le_bytes()); // target CRTC (None)
    req.extend_from_slice(&wait_fence.to_le_bytes()); // wait fence (frame sync)
    req.extend_from_slice(&0u32.to_le_bytes()); // idle fence (None)
    req.extend_from_slice(&options.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // unused
    req.extend_from_slice(&0u64.to_le_bytes()); // target MSC
    req.extend_from_slice(&0u64.to_le_bytes()); // divisor
    req.extend_from_slice(&0u64.to_le_bytes()); // remainder
    finish(req)
}

/// Build a DRI3 `FenceFromFD` request (minor 4): wrap a kernel sync-file `fd`
/// (passed alongside as `SCM_RIGHTS` ancillary data) as the `XSyncFence`
/// resource `fence` on `drawable`'s screen. `initially_triggered` seeds the
/// fence state. The producer exports its render-completion fence as a sync-file
/// and the server waits on the resulting `XSyncFence` in
/// [`present_pixmap_request`] before sampling the pixmap — that is GPU frame
/// sync, no CPU stall.
pub fn dri3_fence_from_fd_request(
    major: u8,
    drawable: u32,
    fence: u32,
    initially_triggered: bool,
) -> Vec<u8> {
    let mut req = vec![major, DRI3_FENCE_FROM_FD, 0, 0];
    req.extend_from_slice(&drawable.to_le_bytes());
    req.extend_from_slice(&fence.to_le_bytes());
    req.push(initially_triggered as u8);
    req.extend_from_slice(&[0u8; 3]); // pad
    finish(req)
}

/// Negotiate the Present extension, returning the server's `(major, minor)`
/// version. `None` if the server lacks Present. Works without a GPU.
fn present_query_version(conn: &mut Conn) -> io::Result<Option<(u32, u32)>> {
    let Some(major) = query_extension(conn, b"Present")? else {
        return Ok(None);
    };
    let mut req = vec![major, PRESENT_QUERY_VERSION, 0, 0];
    req.extend_from_slice(&1u32.to_le_bytes()); // client major
    req.extend_from_slice(&2u32.to_le_bytes()); // client minor
    conn.send(&finish(req))?;
    let mut reply = [0u8; 32];
    conn.stream.read_exact(&mut reply)?;
    if reply[0] != 1 {
        return Ok(None);
    }
    let smajor = u32::from_le_bytes([reply[8], reply[9], reply[10], reply[11]]);
    let sminor = u32::from_le_bytes([reply[12], reply[13], reply[14], reply[15]]);
    Ok(Some((smajor, sminor)))
}

// The runtime composition that flips a GPU frame to a window with no readback —
// query DRI3 + Present, `conn.new_id()` for the pixmap,
// `scm::send_with_fds(pixmap_from_buffers_request(..), plane_fds)`, then
// `present_pixmap_request(..)` — lives in the windowed compositor once
// `stipple::gpu` exports a scene's dma-buf descriptor (fds + per-plane
// stride/offset + modifier). The wire encoders above are that path's tested
// building blocks; both DRI3 import and Present flip need real GPU hardware.

/// Connect and negotiate the Present extension, returning a one-line status.
/// Present needs no GPU (Xvfb supports it), so this is CI-verifiable — it proves
/// the half of the zero-copy present path that doesn't require DRM hardware.
pub fn present_probe() -> Result<String, PlatformError> {
    let mut conn = Conn::connect()?;
    match present_query_version(&mut conn).map_err(os)? {
        Some((maj, min)) => Ok(format!("Present {maj}.{min} available")),
        None => Ok("Present unavailable on this server".to_string()),
    }
}

/// Present a rendered dma-buf to a freshly created window with **no readback**,
/// composing the full zero-copy path end to end: connect, create + map a window
/// the size of `img`, then DRI3 `PixmapFromBuffers` (the plane `fds` passed over
/// the socket as `SCM_RIGHTS`) → Present `PresentPixmap` to flip it. A
/// `GetInputFocus` round-trip then surfaces any protocol error from those
/// requests. Returns a one-line status.
///
/// **Hardware-gated**: the server imports the dma-buf, so this needs a real GPU +
/// a DRM-capable X server — under Xvfb DRI3 is absent and it reports unavailable
/// (without touching `fds`). `fds.len()` must equal `img.planes.len()`; the fds
/// come from `stipple::gpu::export_dmabuf_on_device` bound to *this* server's DRM
/// device (its fd from [`dri3_open_drm_fd`]), so the server can import them.
pub fn dri3_present_dmabuf_self_test(
    img: &DmabufImage,
    fds: &[RawFd],
) -> Result<String, PlatformError> {
    let mut conn = Conn::connect()?;
    let Some(dri3_major) = query_extension(&mut conn, b"DRI3").map_err(os)? else {
        return Ok("DRI3 unavailable on this server (no GPU/DRM; e.g. Xvfb)".to_string());
    };
    let Some(present_major) = query_extension(&mut conn, b"Present").map_err(os)? else {
        return Ok("Present unavailable on this server".to_string());
    };

    // Create + map a window the size of the image (minimal: no GC/SHM needed —
    // the dma-buf pixmap is presented directly).
    let window = conn.new_id();
    let setup = conn.setup;
    let (w, h) = (img.width.max(1), img.height.max(1));
    let mut req = vec![OP_CREATE_WINDOW, setup.root_depth, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&setup.root.to_le_bytes());
    req.extend_from_slice(&0i16.to_le_bytes()); // x
    req.extend_from_slice(&0i16.to_le_bytes()); // y
    req.extend_from_slice(&w.to_le_bytes());
    req.extend_from_slice(&h.to_le_bytes());
    req.extend_from_slice(&0u16.to_le_bytes()); // border width
    req.extend_from_slice(&1u16.to_le_bytes()); // class: InputOutput
    req.extend_from_slice(&setup.root_visual.to_le_bytes());
    req.extend_from_slice(&CW_BACK_PIXEL.to_le_bytes()); // value-mask
    req.extend_from_slice(&0u32.to_le_bytes()); // back-pixel: black
    conn.send(&finish(req)).map_err(os)?;
    let mut req = vec![OP_MAP_WINDOW, 0, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    conn.send(&finish(req)).map_err(os)?;

    // Wrap the dma-buf as an X pixmap (its plane fds ride as ancillary data) and
    // flip that pixmap to the window — the zero-copy present.
    let pixmap = conn.new_id();
    let req = pixmap_from_buffers_request(dri3_major, pixmap, window, img);
    crate::platform::scm::send_with_fds(&conn.stream, &req, fds).map_err(os)?;
    // wait_fence 0: an immediate present. A real producer passes a fence wrapped
    // from its render-completion sync-file via `dri3_fence_from_fd_request`.
    conn.send(&present_pixmap_request(
        present_major,
        window,
        pixmap,
        0,
        0,
        0,
    ))
    .map_err(os)?;

    // Round-trip on GetInputFocus: if any request above errored, the error packet
    // arrives in place of the reply (first byte 0).
    conn.send(&finish(vec![OP_GET_INPUT_FOCUS, 0, 0, 0]))
        .map_err(os)?;
    let mut pkt = [0u8; 32];
    conn.stream.read_exact(&mut pkt).map_err(os)?;
    if pkt[0] == 0 {
        return Err(PlatformError::Os(format!(
            "X11 error during DRI3/Present (code {})",
            pkt[1]
        )));
    }
    Ok(format!(
        "presented {w}x{h} dma-buf pixmap to window {window:#x} (DRI3 {dri3_major} + Present {present_major})"
    ))
}

/// Set up MIT-SHM: create a shared segment of `capacity` bytes, attach it to the
/// server, and confirm the attach succeeded. Returns `None` (with everything
/// cleaned up) if the extension is absent or any step fails — the caller then
/// presents via `PutImage`. Must run before the window is mapped.
fn setup_shm(conn: &mut Conn, capacity: usize) -> Option<ShmState> {
    let major = query_extension(conn, b"MIT-SHM").ok()??;

    let shmid = unsafe { shmget(IPC_PRIVATE, capacity, IPC_CREAT | SHM_PERM) };
    if shmid < 0 {
        return None;
    }
    let addr = unsafe { shmat(shmid, core::ptr::null(), 0) };
    if addr as isize == -1 {
        unsafe {
            shmctl(shmid, IPC_RMID, core::ptr::null_mut());
        }
        return None;
    }
    let ptr = addr as *mut u8;

    // ShmAttach: hand the segment to the server.
    let seg = conn.new_id();
    let mut req = vec![major, SHM_ATTACH, 0, 0];
    req.extend_from_slice(&seg.to_le_bytes());
    req.extend_from_slice(&(shmid as u32).to_le_bytes());
    req.push(0); // read-only: false
    req.extend_from_slice(&[0u8; 3]);
    let cleanup = |ptr: *mut u8, shmid: i32| unsafe {
        shmdt(ptr as *const c_void);
        shmctl(shmid, IPC_RMID, core::ptr::null_mut());
    };
    if conn.send(&finish(req)).is_err() {
        cleanup(ptr, shmid);
        return None;
    }

    // Synchronize on GetInputFocus (which has a reply): if ShmAttach errored, the
    // error packet arrives first. No window is mapped yet, so nothing else can
    // interleave with the reply.
    if conn
        .send(&finish(vec![OP_GET_INPUT_FOCUS, 0, 0, 0]))
        .is_err()
    {
        cleanup(ptr, shmid);
        return None;
    }
    let mut pkt = [0u8; 32];
    if conn.stream.read_exact(&mut pkt).is_err() {
        cleanup(ptr, shmid);
        return None;
    }
    if pkt[0] == 0 {
        // ShmAttach failed; drain the GetInputFocus reply and bail.
        let mut reply = [0u8; 32];
        let _ = conn.stream.read_exact(&mut reply);
        cleanup(ptr, shmid);
        return None;
    }

    // Attach confirmed: mark the segment for deletion now (the server holds a
    // reference), so it's freed automatically once everyone detaches.
    unsafe {
        shmctl(shmid, IPC_RMID, core::ptr::null_mut());
    }
    Some(ShmState {
        major,
        seg,
        ptr,
        capacity,
    })
}

/// Read the first MIT-MAGIC-COOKIE-1 entry's cookie from `.Xauthority`, if any.
/// Best-effort: returns `None` when the file is absent or has no cookie (the
/// server may still accept an unauthenticated local connection).
fn read_auth_cookie() -> Option<Vec<u8>> {
    let path = std::env::var_os("XAUTHORITY")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".Xauthority"))
        })?;
    let data = std::fs::read(path).ok()?;
    parse_xauth_cookie(&data)
}

fn parse_xauth_cookie(data: &[u8]) -> Option<Vec<u8>> {
    // Each entry: family(2) addr_len(2) addr name_len(2) name data_len(2) data,
    // all big-endian lengths.
    let be16 = |b: &[u8], o: usize| -> Option<usize> {
        Some(u16::from_be_bytes([*b.get(o)?, *b.get(o + 1)?]) as usize)
    };
    let mut o = 0;
    while o + 2 <= data.len() {
        o += 2; // family
        let addr_len = be16(data, o)?;
        o += 2 + addr_len;
        let num_len = be16(data, o)?;
        o += 2 + num_len;
        let name_len = be16(data, o)?;
        let name = data.get(o + 2..o + 2 + name_len)?;
        o += 2 + name_len;
        let data_len = be16(data, o)?;
        let cookie = data.get(o + 2..o + 2 + data_len)?;
        o += 2 + data_len;
        if name == b"MIT-MAGIC-COOKIE-1" {
            return Some(cookie.to_vec());
        }
    }
    None
}

// ---- Connection -------------------------------------------------------------

struct Conn {
    stream: UnixStream,
    setup: Setup,
    /// Physical pixels per logical pixel, from `Xft.dpi`.
    scale: f64,
    next_id: u32,
    id_step: u32,
    /// The text we currently own on the `CLIPBOARD` selection. Served to other
    /// clients in response to `SelectionRequest`; updated by `set_clipboard`.
    clipboard_text: String,
    /// Atoms interned once at startup for the clipboard protocol (0 until set).
    atom_clipboard: u32,
    atom_utf8: u32,
    atom_targets: u32,
    /// WM_PROTOCOLS / WM_DELETE_WINDOW, interned once so every window (including
    /// ones opened later) registers the close protocol the same way.
    atom_wm_protocols: u32,
    atom_wm_delete: u32,
    /// EWMH atoms for window state and the icon (0 until set).
    atom_net_wm_state: u32,
    atom_max_vert: u32,
    atom_max_horz: u32,
    atom_fullscreen: u32,
    atom_net_wm_icon: u32,
    /// The XDND atoms (0 until set).
    xdnd: dnd::Atoms,
    /// The core `cursor` font, opened on first use (0 until then).
    cursor_font: u32,
    /// Cursors created so far, by glyph.
    cursors: Vec<(u16, u32)>,
}

impl Conn {
    fn connect() -> Result<Self, PlatformError> {
        let display =
            std::env::var("DISPLAY").map_err(|_| PlatformError::NoBackend("DISPLAY unset"))?;
        let addr =
            parse_display(&display).ok_or(PlatformError::NoBackend("could not parse DISPLAY"))?;
        let path = format!("/tmp/.X11-unix/X{}", addr.number);
        let mut stream = UnixStream::connect(&path)
            .map_err(|e| PlatformError::Os(format!("connect {path}: {e}")))?;

        // --- Setup request ---
        let cookie = read_auth_cookie().unwrap_or_default();
        let (auth_name, auth_data): (&[u8], &[u8]) = if cookie.is_empty() {
            (b"", b"")
        } else {
            (b"MIT-MAGIC-COOKIE-1", &cookie)
        };
        let mut req = Vec::new();
        req.push(b'l'); // little-endian
        req.push(0);
        req.extend_from_slice(&11u16.to_le_bytes()); // protocol major
        req.extend_from_slice(&0u16.to_le_bytes()); // protocol minor
        req.extend_from_slice(&(auth_name.len() as u16).to_le_bytes());
        req.extend_from_slice(&(auth_data.len() as u16).to_le_bytes());
        req.extend_from_slice(&0u16.to_le_bytes()); // pad
        req.extend_from_slice(auth_name);
        pad4(&mut req);
        req.extend_from_slice(auth_data);
        pad4(&mut req);
        stream.write_all(&req).map_err(os)?;
        stream.flush().map_err(os)?;

        // --- Setup reply ---
        let mut header = [0u8; 8];
        stream.read_exact(&mut header).map_err(os)?;
        if header[0] != 1 {
            // 0 = failed, 2 = authenticate.
            return Err(PlatformError::Os(format!(
                "X11 setup refused (status {})",
                header[0]
            )));
        }
        let extra_words = u16::from_le_bytes([header[6], header[7]]) as usize;
        let mut rest = vec![0u8; extra_words * 4];
        stream.read_exact(&mut rest).map_err(os)?;
        let setup = parse_setup(&rest).ok_or(PlatformError::Os("malformed setup reply".into()))?;

        dbg(format_args!(
            "connected: root={:#x} visual={:#x} depth={} max_req={}",
            setup.root, setup.root_visual, setup.root_depth, setup.max_request_len
        ));
        let id_step = setup.resource_id_mask & setup.resource_id_mask.wrapping_neg();
        Ok(Self {
            stream,
            setup,
            scale: 1.0,
            next_id: 0,
            id_step: id_step.max(1),
            clipboard_text: String::new(),
            atom_clipboard: 0,
            atom_utf8: 0,
            atom_targets: 0,
            atom_wm_protocols: 0,
            atom_wm_delete: 0,
            atom_net_wm_state: 0,
            atom_max_vert: 0,
            atom_max_horz: 0,
            atom_fullscreen: 0,
            atom_net_wm_icon: 0,
            xdnd: dnd::Atoms::default(),
            cursor_font: 0,
            cursors: Vec::new(),
        })
    }

    /// Allocate a fresh XID.
    fn new_id(&mut self) -> u32 {
        let id = self.setup.resource_id_base | (self.next_id * self.id_step);
        self.next_id += 1;
        id
    }

    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.stream.write_all(bytes)?;
        self.stream.flush()
    }
}

fn os(e: io::Error) -> PlatformError {
    PlatformError::Os(e.to_string())
}

fn pad4(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

/// Set the request-length field (offset 2, in 4-byte units) from the final
/// buffer length, then return the buffer.
fn finish(mut req: Vec<u8>) -> Vec<u8> {
    let words = (req.len() / 4) as u16;
    req[2..4].copy_from_slice(&words.to_le_bytes());
    req
}

// ---- Window + Surface -------------------------------------------------------

/// Shared mutable connection so the window's surface and the event loop can
/// both talk to the server.
type SharedConn = Arc<Mutex<Conn>>;

/// Windows opened (or closed) from inside the event loop — e.g. by
/// [`Window::open_window`]. The loop drains these after each handler call and
/// merges them into its live set. Single-threaded (`Rc`/`RefCell`): every X11
/// window shares one connection and one loop, so there is no cross-thread use.
#[derive(Debug, Default)]
struct WindowReg {
    /// Newly created windows awaiting adoption into the loop's live set.
    opened: Vec<X11Window>,
    /// XIDs of windows the app asked to close.
    closed: Vec<u32>,
}

type Registry = Rc<RefCell<WindowReg>>;

#[derive(Debug)]
struct X11Window {
    conn: SharedConn,
    window: u32,
    gc: u32,
    size: PhysicalSize,
    // Set up once before mapping; moved into the surface on `create_surface`.
    shm: RefCell<Option<ShmState>>,
    // Shared with the event loop so opening/closing siblings reaches it. Held by
    // every window but normally points at empty queues (drained each iteration),
    // so there is no retained reference cycle through the loop's live set.
    reg: Registry,
    /// The minimum and maximum inner size, in logical pixels, last given to
    /// the window manager in `WM_NORMAL_HINTS`.
    limits: Cell<(Option<Size>, Option<Size>)>,
    /// Whether the app takes the drag over the window, as last told through
    /// `set_drop_accepted` (XDND answers the source with it).
    drop_accepted: Cell<bool>,
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conn")
            .field("setup", &self.setup)
            .finish_non_exhaustive()
    }
}

impl Window for X11Window {
    fn id(&self) -> WindowId {
        WindowId(self.window as u64)
    }
    fn set_tray(&self, tray: Option<&crate::platform::NativeTray>) -> bool {
        crate::platform::shell::sni::set_tray(tray)
    }
    fn inner_size(&self) -> PhysicalSize {
        self.size
    }
    fn scale_factor(&self) -> ScaleFactor {
        ScaleFactor::new(self.conn.lock().unwrap().scale)
    }
    fn request_redraw(&self) {
        // Driven by Expose; an explicit invalidate would need a SendEvent.
    }
    fn set_title(&self, _title: &str) {}
    fn set_clipboard(&self, text: &str) {
        // Become the owner of the CLIPBOARD selection and remember the text;
        // the event loop serves it to requestors via SelectionRequest.
        let mut conn = self.conn.lock().unwrap();
        conn.clipboard_text = text.to_string();
        let selection = conn.atom_clipboard;
        if selection == 0 {
            return;
        }
        let mut req = vec![OP_SET_SELECTION_OWNER, 0, 0, 0];
        req.extend_from_slice(&self.window.to_le_bytes()); // owner
        req.extend_from_slice(&selection.to_le_bytes()); // CLIPBOARD
        req.extend_from_slice(&0u32.to_le_bytes()); // time = CurrentTime
        let _ = conn.send(&finish(req));
    }
    fn create_surface(&self) -> Box<dyn Surface> {
        Box::new(X11Surface {
            conn: self.conn.clone(),
            window: self.window,
            gc: self.gc,
            size: self.size,
            shm: self.shm.borrow_mut().take(),
        })
    }
    fn open_window(&self, attrs: WindowAttributes) -> Option<WindowId> {
        // Create + map a sibling window on the same connection, then hand it to
        // the event loop (which shares `reg`) to adopt into its live set. Its
        // events arrive through the same handler, keyed by `id()`.
        let win = {
            let mut conn = self.conn.lock().unwrap();
            create_window(
                &mut conn,
                &attrs,
                self.conn.clone(),
                self.reg.clone(),
                false,
            )
            .ok()?
        };
        let id = win.id();
        self.reg.borrow_mut().opened.push(win);
        Some(id)
    }
    fn close_window(&self) {
        self.reg.borrow_mut().closed.push(self.window);
    }
    fn set_cursor(&self, cursor: CursorIcon) {
        let mut conn = self.conn.lock().unwrap();
        let _ = set_cursor(&mut conn, self.window, cursor);
    }
    fn set_min_size(&self, size: Option<Size>) {
        let (_, max) = self.limits.get();
        self.limits.set((size, max));
        let mut conn = self.conn.lock().unwrap();
        let _ = set_size_hints(&mut conn, self.window, size, max);
    }
    fn set_max_size(&self, size: Option<Size>) {
        let (min, _) = self.limits.get();
        self.limits.set((min, size));
        let mut conn = self.conn.lock().unwrap();
        let _ = set_size_hints(&mut conn, self.window, min, size);
    }
    fn set_maximized(&self, maximized: bool) {
        let mut conn = self.conn.lock().unwrap();
        let (vert, horz) = (conn.atom_max_vert, conn.atom_max_horz);
        let _ = send_wm_state(&mut conn, self.window, maximized, vert, horz);
    }
    fn set_fullscreen(&self, fullscreen: bool) {
        let mut conn = self.conn.lock().unwrap();
        let full = conn.atom_fullscreen;
        let _ = send_wm_state(&mut conn, self.window, fullscreen, full, 0);
    }
    fn set_drop_accepted(&self, accepted: bool) {
        self.drop_accepted.set(accepted);
    }
    fn set_icon(&self, icon: Option<&Pixmap>) {
        let mut conn = self.conn.lock().unwrap();
        let _ = set_icon(&mut conn, self.window, icon);
    }
}

/// Tell the window manager the inner size limits (`WM_NORMAL_HINTS`), given in
/// logical pixels and sent in physical ones.
fn set_size_hints(
    conn: &mut Conn,
    window: u32,
    min: Option<Size>,
    max: Option<Size>,
) -> io::Result<()> {
    let hints = size_hints(ScaleFactor::new(conn.scale), min, max);
    set_property32(
        conn,
        window,
        ATOM_WM_NORMAL_HINTS,
        ATOM_WM_SIZE_HINTS,
        &hints,
    )
}

/// The 18 `WM_SIZE_HINTS` words for these limits: flags, x, y, width, height
/// (obsolete), min w/h, max w/h, resize increments, aspect ratios, base size and
/// gravity.
fn size_hints(scale: ScaleFactor, min: Option<Size>, max: Option<Size>) -> [u32; 18] {
    let mut hints = [0u32; 18];
    let px = |s: Size| {
        let p = scale.to_physical(s);
        (p.width.max(1), p.height.max(1))
    };
    if let Some(min) = min {
        hints[0] |= P_MIN_SIZE;
        (hints[5], hints[6]) = px(min);
    }
    if let Some(max) = max {
        hints[0] |= P_MAX_SIZE;
        (hints[7], hints[8]) = px(max);
    }
    hints
}

/// Ask the window manager to add or remove up to two `_NET_WM_STATE` atoms,
/// with the EWMH client message to the root window.
fn send_wm_state(conn: &mut Conn, window: u32, add: bool, a: u32, b: u32) -> io::Result<()> {
    let (root, state) = (conn.setup.root, conn.atom_net_wm_state);
    if state == 0 || a == 0 {
        return Ok(());
    }
    let req = wm_state_message(root, window, state, add, a, b);
    conn.send(&req)
}

/// A `SendEvent` of a `_NET_WM_STATE` `ClientMessage` (EWMH: data is action,
/// first property, second property, source indication = 1 for applications).
fn wm_state_message(root: u32, window: u32, state: u32, add: bool, a: u32, b: u32) -> Vec<u8> {
    let mut req = vec![OP_SEND_EVENT, 0 /* propagate */, 0, 0];
    req.extend_from_slice(&root.to_le_bytes());
    req.extend_from_slice(&EV_SUBSTRUCTURE.to_le_bytes());
    // The 32-byte event.
    req.extend_from_slice(&[X_CLIENT_MESSAGE, 32, 0, 0]);
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&state.to_le_bytes());
    for word in [add as u32, a, b, 1, 0] {
        req.extend_from_slice(&word.to_le_bytes());
    }
    finish(req)
}

/// Set (or, with `None`, remove) `_NET_WM_ICON`: width, height, then one
/// `0xAARRGGBB` word per pixel, rows top first.
fn set_icon(conn: &mut Conn, window: u32, icon: Option<&Pixmap>) -> io::Result<()> {
    let prop = conn.atom_net_wm_icon;
    if prop == 0 {
        return Ok(());
    }
    match icon {
        Some(icon) => set_property32(conn, window, prop, ATOM_CARDINAL, &icon_words(icon)),
        None => {
            let mut req = vec![OP_DELETE_PROPERTY, 0, 0, 0];
            req.extend_from_slice(&window.to_le_bytes());
            req.extend_from_slice(&prop.to_le_bytes());
            conn.send(&finish(req))
        }
    }
}

/// The `_NET_WM_ICON` words for `icon`, scaled down to [`MAX_ICON`] if needed.
fn icon_words(icon: &Pixmap) -> Vec<u32> {
    let icon = fit_icon(icon, MAX_ICON);
    let size = icon.size();
    let mut words = Vec::with_capacity(2 + size.pixel_count() as usize);
    words.push(size.width);
    words.push(size.height);
    words.extend(
        icon.as_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| u32::from_be_bytes([p[3], p[0], p[1], p[2]])),
    );
    words
}

/// Show `icon` over `window`, creating its cursor from the core cursor font
/// the first time it's used.
fn set_cursor(conn: &mut Conn, window: u32, icon: CursorIcon) -> io::Result<()> {
    dbg(format_args!("cursor {icon:?} on window={window:#x}"));
    let glyph = cursor_glyph(icon);
    let cursor = match conn.cursors.iter().find(|(g, _)| *g == glyph) {
        Some(&(_, id)) => id,
        None => {
            if conn.cursor_font == 0 {
                let font = conn.new_id();
                let name = b"cursor";
                let mut req = vec![OP_OPEN_FONT, 0, 0, 0];
                req.extend_from_slice(&font.to_le_bytes());
                req.extend_from_slice(&(name.len() as u16).to_le_bytes());
                req.extend_from_slice(&0u16.to_le_bytes());
                req.extend_from_slice(name);
                pad4(&mut req);
                conn.send(&finish(req))?;
                conn.cursor_font = font;
            }
            let id = conn.new_id();
            let font = conn.cursor_font;
            conn.send(&glyph_cursor_request(id, font, glyph))?;
            conn.cursors.push((glyph, id));
            id
        }
    };
    let mut req = vec![OP_CHANGE_WINDOW_ATTRIBUTES, 0, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&CW_CURSOR.to_le_bytes());
    req.extend_from_slice(&cursor.to_le_bytes());
    conn.send(&finish(req))
}

/// `CreateGlyphCursor` for `glyph` of the cursor font (mask = `glyph + 1`),
/// black on white.
fn glyph_cursor_request(id: u32, font: u32, glyph: u16) -> Vec<u8> {
    let mut req = vec![OP_CREATE_GLYPH_CURSOR, 0, 0, 0];
    req.extend_from_slice(&id.to_le_bytes());
    req.extend_from_slice(&font.to_le_bytes()); // source font
    req.extend_from_slice(&font.to_le_bytes()); // mask font
    req.extend_from_slice(&glyph.to_le_bytes());
    req.extend_from_slice(&(glyph + 1).to_le_bytes());
    for c in [0u16, 0, 0, 0xffff, 0xffff, 0xffff] {
        req.extend_from_slice(&c.to_le_bytes());
    }
    finish(req)
}

struct X11Surface {
    conn: SharedConn,
    window: u32,
    gc: u32,
    size: PhysicalSize,
    /// Present via MIT-SHM when available; `None` falls back to `PutImage`.
    shm: Option<ShmState>,
}

impl std::fmt::Debug for X11Surface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11Surface")
            .field("window", &self.window)
            .field("size", &self.size)
            .finish()
    }
}

impl Surface for X11Surface {
    fn resize(&mut self, size: PhysicalSize) {
        self.size = size;
    }
    fn size(&self) -> PhysicalSize {
        self.size
    }
    fn present(&mut self, pixmap: &Pixmap, damage: &[crate::geometry::Rect]) {
        let size = pixmap.size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        dbg(format_args!(
            "present {}x{} to window={:#x} (shm={})",
            size.width,
            size.height,
            self.window,
            self.shm.is_some()
        ));
        let mut conn = self.conn.lock().unwrap();

        // Fast path: copy only the damaged regions into shared memory and blit
        // them. On failure (frame grew past the segment), drop shm and fall
        // through to PutImage for this and subsequent frames.
        if let Some(shm) = self.shm.as_mut() {
            if shm.present(&mut conn, self.window, self.gc, pixmap, damage) {
                return;
            }
            self.shm = None;
        }
        // PutImage may exceed the server's max request length; send in row
        // bands that each fit. Header is 24 bytes; budget the rest for pixels.
        let max_bytes = (conn.setup.max_request_len as usize)
            .saturating_mul(4)
            .max(256 * 1024);
        let row_bytes = size.width as usize * 4;
        let rows_per = (max_bytes.saturating_sub(24) / row_bytes.max(1)).max(1);

        let src = pixmap.as_bytes();
        let mut bgrx = Vec::new();
        let mut y = 0u32;
        while y < size.height {
            let band = rows_per.min((size.height - y) as usize);
            let start = y as usize * row_bytes;
            let end = start + band * row_bytes;
            rgba_to_bgrx(&src[start..end], &mut bgrx);

            let mut req = vec![OP_PUT_IMAGE, 2 /* ZPixmap */, 0, 0];
            req.extend_from_slice(&self.window.to_le_bytes());
            req.extend_from_slice(&self.gc.to_le_bytes());
            req.extend_from_slice(&(size.width as u16).to_le_bytes());
            req.extend_from_slice(&(band as u16).to_le_bytes());
            req.extend_from_slice(&0i16.to_le_bytes()); // dst-x
            req.extend_from_slice(&(y as i16).to_le_bytes()); // dst-y
            req.push(0); // left-pad
            req.push(24); // depth
            req.extend_from_slice(&0u16.to_le_bytes()); // pad
            req.extend_from_slice(&bgrx);
            pad4(&mut req);
            if conn.send(&finish(req)).is_err() {
                break;
            }
            y += band as u32;
        }
    }
}

// ---- Run loop ---------------------------------------------------------------

/// Create, configure, and map one top-level window on `conn`, returning a live
/// [`X11Window`]. Used both for the initial window and for siblings opened later
/// via [`Window::open_window`]. Requires the WM/clipboard atoms to be interned
/// on `conn` already (see [`run`]).
fn create_window(
    conn: &mut Conn,
    attrs: &WindowAttributes,
    shared: SharedConn,
    reg: Registry,
    use_shm: bool,
) -> Result<X11Window, PlatformError> {
    let size = ScaleFactor::new(conn.scale).to_physical(attrs.logical_size);
    let (w, h) = (size.width.max(1) as u16, size.height.max(1) as u16);
    let window = conn.new_id();
    let gc = conn.new_id();
    let setup = conn.setup;

    // CreateWindow.
    let (x, y) = attrs.position.unwrap_or((0, 0));
    let mut req = vec![OP_CREATE_WINDOW, setup.root_depth, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&setup.root.to_le_bytes());
    req.extend_from_slice(&(x as i16).to_le_bytes()); // x
    req.extend_from_slice(&(y as i16).to_le_bytes()); // y
    req.extend_from_slice(&w.to_le_bytes());
    req.extend_from_slice(&h.to_le_bytes());
    req.extend_from_slice(&0u16.to_le_bytes()); // border width
    req.extend_from_slice(&1u16.to_le_bytes()); // class: InputOutput
    req.extend_from_slice(&setup.root_visual.to_le_bytes());
    req.extend_from_slice(&(CW_BACK_PIXEL | CW_EVENT_MASK).to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // back-pixel: black
    let mask = EV_EXPOSURE
        | EV_KEY_PRESS
        | EV_KEY_RELEASE
        | EV_BUTTON_PRESS
        | EV_BUTTON_RELEASE
        | EV_POINTER_MOTION
        | EV_STRUCTURE_NOTIFY;
    req.extend_from_slice(&mask.to_le_bytes());
    conn.send(&finish(req)).map_err(os)?;

    // CreateGC.
    let mut req = vec![OP_CREATE_GC, 0, 0, 0];
    req.extend_from_slice(&gc.to_le_bytes());
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // value-mask: none
    conn.send(&finish(req)).map_err(os)?;

    // Title via WM_NAME (atom 39 = WM_NAME, predefined; type STRING = 31).
    set_property_str(conn, window, 39, 31, attrs.title.as_bytes()).map_err(os)?;

    // WM_PROTOCOLS / WM_DELETE_WINDOW so the close button delivers an event.
    let (wm_protocols, wm_delete) = (conn.atom_wm_protocols, conn.atom_wm_delete);
    set_property_atoms(conn, window, wm_protocols, &[wm_delete]).map_err(os)?;

    // Size limits, initial state and icon, all read by the window manager when
    // the window maps. A fixed-size window has its size as both limits.
    let limits = if attrs.resizable {
        (attrs.min_size, attrs.max_size)
    } else {
        (Some(attrs.logical_size), Some(attrs.logical_size))
    };
    if limits != (None, None) {
        set_size_hints(conn, window, limits.0, limits.1).map_err(os)?;
    }
    let mut states = Vec::new();
    if attrs.maximized {
        states.extend([conn.atom_max_vert, conn.atom_max_horz]);
    }
    if attrs.fullscreen {
        states.push(conn.atom_fullscreen);
    }
    if !states.is_empty() {
        let prop = conn.atom_net_wm_state;
        set_property_atoms(conn, window, prop, &states).map_err(os)?;
    }
    if attrs.icon.is_some() {
        set_icon(conn, window, attrs.icon.as_ref()).map_err(os)?;
    }
    // Take drops from other applications.
    if conn.xdnd.aware != 0 {
        let aware = conn.xdnd.aware;
        set_property_atoms(conn, window, aware, &[dnd::VERSION]).map_err(os)?;
    }

    // Set up MIT-SHM presentation before mapping (its handshake needs replies
    // that must not interleave with window events). Falls back to PutImage when
    // unavailable. Siblings opened mid-loop pass `use_shm = false`: the first
    // window is already emitting events, so reply-reads here would race the
    // event stream — those windows present via PutImage instead.
    let shm = if use_shm {
        setup_shm(conn, (w as usize) * (h as usize) * 4)
    } else {
        None
    };
    dbg(format_args!("shm present path: {}", shm.is_some()));

    // MapWindow.
    let mut req = vec![OP_MAP_WINDOW, 0, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    conn.send(&finish(req)).map_err(os)?;

    // Grab keyboard focus so key events arrive even with no window manager
    // (e.g. under Xvfb in CI). revert-to = PointerRoot (1), time = 0
    // (CurrentTime). Pointer events are delivered regardless of focus.
    let mut req = vec![OP_SET_INPUT_FOCUS, 1, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes());
    conn.send(&finish(req)).map_err(os)?;

    dbg(format_args!(
        "mapped window={window:#x} gc={gc:#x} size={}x{}",
        w, h
    ));

    Ok(X11Window {
        conn: shared,
        window,
        gc,
        size,
        shm: RefCell::new(shm),
        reg,
        limits: Cell::new(limits),
        drop_accepted: Cell::new(false),
    })
}

/// Connect to the X server, create a window, and drive `handler` over the event
/// stream until the handler returns [`ControlFlow::Exit`] or every window has
/// closed. Additional top-level windows opened via [`Window::open_window`] share
/// this one connection and loop; their events are dispatched to the same handler
/// against the [`Window`] they belong to.
pub fn run<H>(attrs: WindowAttributes, mut handler: H) -> Result<(), PlatformError>
where
    H: FnMut(Event, &dyn Window) -> ControlFlow,
{
    let mut conn = Conn::connect()?;

    // Fetch the keyboard mapping up front (no window yet, so the reply is the
    // next message and won't interleave with events).
    let mut keymap = fetch_keymap(&mut conn).map_err(os)?;
    // Dead keys and compose sequences, from the locale's Compose file.
    let mut compose = crate::platform::compose::ComposeState::default();
    dbg(format_args!(
        "keymap: {} keysyms, {}/keycode",
        keymap.syms.len(),
        keymap.per
    ));

    // Intern the WM + clipboard atoms once, up front, so `create_window` (used
    // for the first window and every later sibling) and the event loop can rely
    // on them.
    conn.atom_wm_protocols = intern_atom(&mut conn, b"WM_PROTOCOLS").map_err(os)?;
    conn.atom_wm_delete = intern_atom(&mut conn, b"WM_DELETE_WINDOW").map_err(os)?;
    conn.atom_clipboard = intern_atom(&mut conn, b"CLIPBOARD").map_err(os)?;
    conn.atom_utf8 = intern_atom(&mut conn, b"UTF8_STRING").map_err(os)?;
    conn.atom_targets = intern_atom(&mut conn, b"TARGETS").map_err(os)?;
    conn.atom_net_wm_state = intern_atom(&mut conn, b"_NET_WM_STATE").map_err(os)?;
    conn.atom_max_vert = intern_atom(&mut conn, b"_NET_WM_STATE_MAXIMIZED_VERT").map_err(os)?;
    conn.atom_max_horz = intern_atom(&mut conn, b"_NET_WM_STATE_MAXIMIZED_HORZ").map_err(os)?;
    conn.atom_fullscreen = intern_atom(&mut conn, b"_NET_WM_STATE_FULLSCREEN").map_err(os)?;
    conn.atom_net_wm_icon = intern_atom(&mut conn, b"_NET_WM_ICON").map_err(os)?;
    conn.xdnd = dnd::Atoms::intern(&mut conn).map_err(os)?;
    let mut xdnd = dnd::Xdnd {
        atoms: conn.xdnd,
        ..Default::default()
    };
    conn.scale = read_xft_scale(&mut conn).map_err(os)?;
    // Animation frames come from a timer at the monitor's refresh rate.
    if let Some(interval) = refresh::interval(&mut conn) {
        dbg(format_args!("refresh interval {interval:?}"));
        crate::platform::frame::set_interval(interval);
    }
    let wm_delete = conn.atom_wm_delete;

    // Wrap the connection for sharing, then create the first window against it
    // exactly as `open_window` creates later siblings.
    let reg: Registry = Rc::new(RefCell::new(WindowReg::default()));
    let shared: SharedConn = Arc::new(Mutex::new(conn));
    let initial = {
        let mut conn = shared.lock().unwrap();
        create_window(&mut conn, &attrs, shared.clone(), reg.clone(), true)?
    };

    // The live set of windows, keyed by XID via linear scan (tiny N).
    let mut windows: Vec<X11Window> = vec![initial];

    // Event loop. Events are 32 bytes each. Between events, wait on the
    // connection and on wakeups (timers, other threads) together.
    crate::platform::wake::install_unix();
    let fd = {
        use std::os::fd::AsRawFd;
        shared.lock().unwrap().stream.as_raw_fd()
    };
    let mut buf = [0u8; 32];
    // Events read while waiting for a reply (the keymap after a mapping
    // change), handled before reading more.
    let mut backlog: std::collections::VecDeque<[u8; 32]> = std::collections::VecDeque::new();
    loop {
        if let Some(event) = backlog.pop_front() {
            buf = event;
        } else {
            if !crate::platform::wake::wait_readable(fd) {
                if crate::platform::wake::take(std::time::Instant::now())
                    && handler(Event::Wake, &windows[0]) == ControlFlow::Exit
                {
                    break;
                }
                continue;
            }
            let mut conn = shared.lock().unwrap();
            conn.stream.read_exact(&mut buf).map_err(os)?;
        }
        // The keyboard mapping changed: fetch it again. The reply may come
        // after other events, which wait in the backlog.
        if buf[0] & 0x7f == X_MAPPING_NOTIFY && buf[4] == 1 {
            let mut conn = shared.lock().unwrap();
            match refetch_keymap(&mut conn, &mut backlog) {
                Ok(map) => {
                    dbg(format_args!("keymap changed: {} keysyms", map.syms.len()));
                    keymap = map;
                }
                Err(e) => dbg(format_args!("keymap refetch failed: {e}")),
            }
            continue;
        }
        // An error reply (first byte 0) is not an event; log and skip it.
        if buf[0] == 0 {
            dbg(format_args!(
                "X error code={} major={} minor={} bad_resource={:#x}",
                buf[1],
                buf[10],
                u16::from_le_bytes([buf[8], buf[9]]),
                rd_u32(&buf, 4).unwrap_or(0)
            ));
            continue;
        }
        // Route the event to the window it names (falling back to the first);
        // a drop's data comes back to the window it was dropped on.
        let xid = match buf[0] & 0x7f {
            X_SELECTION_NOTIFY => xdnd.converting_window().unwrap_or(0),
            _ => event_window(&buf),
        };
        let idx = windows.iter().position(|w| w.window == xid).unwrap_or(0);
        let win = &windows[idx];
        let flow = match buf[0] & 0x7f {
            X_EXPOSE => {
                dbg(format_args!("expose"));
                handler(Event::RedrawRequested, win)
            }
            X_CONFIGURE_NOTIFY => {
                let nw = u16::from_le_bytes([buf[20], buf[21]]) as u32;
                let nh = u16::from_le_bytes([buf[22], buf[23]]) as u32;
                handler(Event::Resized(PhysicalSize::new(nw, nh)), win)
            }
            X_MOTION_NOTIFY => {
                let (x, y) = event_xy(&buf);
                let scale = win.scale_factor().get();
                handler(
                    Event::PointerMoved {
                        position: Point::new(x / scale, y / scale),
                    },
                    win,
                )
            }
            code @ (X_BUTTON_PRESS | X_BUTTON_RELEASE) => {
                let detail = buf[1];
                let (x, y) = event_xy(&buf);
                let state = if code == X_BUTTON_PRESS {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                dbg(format_args!("button {detail} {state:?} at {x},{y}"));
                // Buttons 4/5 are the vertical wheel, 6/7 the horizontal one.
                if let Some(delta) = wheel_button_delta(detail) {
                    if code == X_BUTTON_PRESS {
                        handler(Event::Scroll { delta }, win)
                    } else {
                        ControlFlow::Wait
                    }
                } else {
                    let button = match detail {
                        1 => PointerButton::Left,
                        2 => PointerButton::Middle,
                        3 => PointerButton::Right,
                        n => PointerButton::Other(n as u16),
                    };
                    let scale = win.scale_factor().get();
                    // The button event's modifier mask (same layout as a key
                    // event's), for Shift/Ctrl+click.
                    let mask = rd_u16(&buf, 28).unwrap_or(0);
                    crate::platform::set_pointer_modifiers(Modifiers {
                        shift: mask & 0x0001 != 0,
                        ctrl: mask & 0x0004 != 0,
                        alt: mask & 0x0008 != 0,
                        meta: mask & 0x0040 != 0,
                    });
                    handler(
                        Event::PointerButton {
                            button,
                            state,
                            position: Point::new(x / scale, y / scale),
                        },
                        win,
                    )
                }
            }
            code @ (X_KEY_PRESS | X_KEY_RELEASE) => {
                let state = if code == X_KEY_PRESS {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                // buf[1] = keycode; buf[28..30] = modifier mask
                // (bit 0 = Shift, bit 2 = Control).
                let keycode = buf[1];
                let mask = rd_u16(&buf, 28).unwrap_or(0);
                // X core modifier mask: Shift=1, Control=4, Mod1/Alt=8,
                // Mod4/Super=0x40 (the common logo-key binding).
                let modifiers = Modifiers {
                    shift: mask & 0x0001 != 0,
                    ctrl: mask & 0x0004 != 0,
                    alt: mask & 0x0008 != 0,
                    meta: mask & 0x0040 != 0,
                };
                let ks = keymap.keysym(keycode, modifiers.shift);
                dbg(format_args!("key {keycode} {state:?} keysym {ks:#x}"));
                let shortcut = modifiers.ctrl || modifiers.meta;
                let composed = (state == ButtonState::Pressed)
                    .then(|| crate::platform::compose::feed_event(&mut compose, ks, shortcut))
                    .flatten();
                match composed {
                    Some(ime) => {
                        dbg(format_args!("compose: keysym {ks:#x} -> {ime:?}"));
                        handler(Event::Ime(ime), win)
                    }
                    None => match keysym_to_event(ks, state, modifiers) {
                        Some(ev) => handler(ev, win),
                        None => ControlFlow::Wait,
                    },
                }
            }
            X_CLIENT_MESSAGE if xdnd.atoms.is_xdnd(rd_u32(&buf, 8).unwrap_or(0)) => {
                xdnd.client_message(&shared, &mut backlog, &buf, win, &mut handler)
            }
            X_SELECTION_NOTIFY => {
                xdnd.selection_notify(&shared, &mut backlog, &buf, win, &mut handler)
            }
            X_CLIENT_MESSAGE => {
                // data starts at byte 12; first 32-bit word is the protocol atom.
                let atom = rd_u32(&buf, 12).unwrap_or(0);
                if atom == wm_delete {
                    handler(Event::CloseRequested, win)
                } else {
                    ControlFlow::Wait
                }
            }
            X_SELECTION_REQUEST => {
                // Another client wants our CLIPBOARD text. Serve it, then notify.
                let mut conn = shared.lock().unwrap();
                let _ = serve_selection_request(&mut conn, &buf);
                ControlFlow::Wait
            }
            X_SELECTION_CLEAR => {
                // We lost ownership of the selection; drop our copy.
                let mut conn = shared.lock().unwrap();
                conn.clipboard_text.clear();
                ControlFlow::Wait
            }
            _ => ControlFlow::Wait, // errors, replies, unhandled events
        };
        if flow == ControlFlow::Exit {
            break;
        }
        // Adopt windows opened during this iteration, and destroy closed ones.
        {
            let mut r = reg.borrow_mut();
            windows.append(&mut r.opened);
            for xid in r.closed.drain(..) {
                if let Some(pos) = windows.iter().position(|w| w.window == xid) {
                    windows.remove(pos);
                    let mut conn = shared.lock().unwrap();
                    let mut req = vec![OP_DESTROY_WINDOW, 0, 0, 0];
                    req.extend_from_slice(&xid.to_le_bytes());
                    let _ = conn.send(&finish(req));
                }
            }
        }
        if windows.is_empty() {
            break; // every window closed
        }
    }
    Ok(())
}

/// The keyboard mapping (keycode → keysyms) fetched via `GetKeyboardMapping`,
/// used to turn key events into text + editing keys.
struct Keymap {
    syms: Vec<u32>,
    per: usize,
    min: u8,
}

impl Keymap {
    fn keysym(&self, keycode: u8, shift: bool) -> u32 {
        if keycode < self.min || self.per == 0 {
            return 0;
        }
        let base = (keycode - self.min) as usize * self.per;
        let i = base + if shift && self.per > 1 { 1 } else { 0 };
        let ks = self.syms.get(i).copied().unwrap_or(0);
        // Fall back to the unshifted keysym when the shifted slot is empty.
        if ks == 0 {
            self.syms.get(base).copied().unwrap_or(0)
        } else {
            ks
        }
    }
}

/// Send `GetKeyboardMapping` for every keycode.
fn request_keymap(conn: &mut Conn) -> io::Result<()> {
    let min = conn.setup.min_keycode;
    let max = conn.setup.max_keycode;
    let count = max.saturating_sub(min).saturating_add(1);
    conn.send(&finish(vec![
        OP_GET_KEYBOARD_MAPPING,
        0,
        0,
        0,
        min,
        count,
        0,
        0,
    ]))
}

fn fetch_keymap(conn: &mut Conn) -> io::Result<Keymap> {
    request_keymap(conn)?;
    let mut reply = [0u8; 32];
    conn.stream.read_exact(&mut reply)?;
    read_keymap_body(conn, &reply)
}

/// Fetch the keymap again from inside the event loop, where events may arrive
/// before the reply: those go to `backlog`.
fn refetch_keymap(
    conn: &mut Conn,
    backlog: &mut std::collections::VecDeque<[u8; 32]>,
) -> io::Result<Keymap> {
    request_keymap(conn)?;
    loop {
        let mut packet = [0u8; 32];
        conn.stream.read_exact(&mut packet)?;
        match packet[0] {
            1 => return read_keymap_body(conn, &packet),
            0 => return Err(io::Error::other("GetKeyboardMapping failed")),
            _ => backlog.push_back(packet),
        }
    }
}

/// Read the keysyms that follow a `GetKeyboardMapping` reply header.
fn read_keymap_body(conn: &mut Conn, reply: &[u8; 32]) -> io::Result<Keymap> {
    let min = conn.setup.min_keycode;
    let per = reply[1] as usize;
    let len_words = rd_u32(reply, 4).unwrap_or(0) as usize; // = count * per
    let mut body = vec![0u8; len_words * 4];
    conn.stream.read_exact(&mut body)?;
    let syms = body
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    Ok(Keymap { syms, per, min })
}

/// F1 … F12, indexed from F1.
const FUNCTION_KEYS: [KeyCode; 12] = [
    KeyCode::F1,
    KeyCode::F2,
    KeyCode::F3,
    KeyCode::F4,
    KeyCode::F5,
    KeyCode::F6,
    KeyCode::F7,
    KeyCode::F8,
    KeyCode::F9,
    KeyCode::F10,
    KeyCode::F11,
    KeyCode::F12,
];

/// The scroll delta for a core-protocol wheel "button": 4/5 scroll up/down, 6/7
/// left/right, one line per click. `None` for real buttons.
fn wheel_button_delta(detail: u8) -> Option<ScrollDelta> {
    let line = crate::platform::input::LINE_PX;
    let (dx, dy) = match detail {
        4 => (0.0, -line),
        5 => (0.0, line),
        6 => (-line, 0.0),
        7 => (line, 0.0),
        _ => return None,
    };
    Some(ScrollDelta { dx, dy })
}

/// Translate an X11 keysym into a Stipple event: editing/navigation keys become
/// [`Event::Key`] (carrying `modifiers`); printable Latin-1 / Unicode keysyms
/// become [`Event::Text`] (on press only).
fn keysym_to_event(ks: u32, state: ButtonState, modifiers: Modifiers) -> Option<Event> {
    let code = match ks {
        0xff08 => Some(KeyCode::Backspace),
        0xff09 => Some(KeyCode::Tab),
        0xff0d => Some(KeyCode::Enter),
        0xff1b => Some(KeyCode::Escape),
        0xff51 => Some(KeyCode::ArrowLeft),
        0xff52 => Some(KeyCode::ArrowUp),
        0xff53 => Some(KeyCode::ArrowRight),
        0xff54 => Some(KeyCode::ArrowDown),
        0xff50 => Some(KeyCode::Home),
        0xff57 => Some(KeyCode::End),
        0xff55 => Some(KeyCode::PageUp),   // XK_Prior
        0xff56 => Some(KeyCode::PageDown), // XK_Next
        0xff63 => Some(KeyCode::Insert),
        0xffff => Some(KeyCode::Delete),
        // XK_F1 … XK_F12 are consecutive.
        0xffbe..=0xffc9 => Some(FUNCTION_KEYS[(ks - 0xffbe) as usize]),
        // XK_Alt_L / XK_Alt_R, and XK_Meta_L / XK_Meta_R that Shift+Alt gives.
        0xffe7..=0xffea => Some(KeyCode::Alt),
        0xff67 => Some(KeyCode::ContextMenu), // XK_Menu
        _ => None,
    };
    if let Some(code) = code {
        return Some(Event::Key {
            code,
            state,
            modifiers,
        });
    }
    if state != ButtonState::Pressed {
        return None;
    }
    // Latin-1 keysyms map directly to codepoints, the Unicode plane is
    // 0x0100_0000 | codepoint, and the legacy keysyms (Latin-2, Greek,
    // Cyrillic, …) go through the keysym table.
    let c = crate::platform::compose::keysym_to_char(ks)?;
    // With Ctrl/Meta/Alt held, a printable key is a shortcut (Ctrl+C, Alt+F,
    // …), not text input — deliver it as a `Key` so the app can map it. AltGr
    // is a different modifier (Mod5), so it still types.
    if modifiers.ctrl || modifiers.meta || modifiers.alt {
        return Some(Event::Key {
            code: KeyCode::Char(c),
            state,
            modifiers,
        });
    }
    Some(Event::Text(c.to_string()))
}

/// The XID of the window an event belongs to, read from the per-event-type
/// offset (Expose/ClientMessage at 4, ConfigureNotify's window at 8, input
/// events' event-window at 12). 0 for events that don't name a window.
fn event_window(buf: &[u8; 32]) -> u32 {
    match buf[0] & 0x7f {
        X_EXPOSE | X_CLIENT_MESSAGE => rd_u32(buf, 4).unwrap_or(0),
        X_CONFIGURE_NOTIFY => rd_u32(buf, 8).unwrap_or(0),
        X_MOTION_NOTIFY | X_BUTTON_PRESS | X_BUTTON_RELEASE | X_KEY_PRESS | X_KEY_RELEASE => {
            rd_u32(buf, 12).unwrap_or(0)
        }
        _ => 0,
    }
}

fn event_xy(buf: &[u8; 32]) -> (f64, f64) {
    // event-x / event-y are i16 at offsets 24 / 26 for pointer events.
    let x = i16::from_le_bytes([buf[24], buf[25]]) as f64;
    let y = i16::from_le_bytes([buf[26], buf[27]]) as f64;
    (x, y)
}

/// The display's scale, from `Xft.dpi` in the root window's `RESOURCE_MANAGER`
/// property: the setting desktops and toolkits use for it on X11. 96 DPI is 1x.
fn read_xft_scale(conn: &mut Conn) -> io::Result<f64> {
    const RESOURCE_MANAGER: u32 = 23; // a predefined atom
    const MAX_WORDS: u32 = 64 * 1024;
    let mut req = vec![OP_GET_PROPERTY, 0 /* don't delete */, 0, 0];
    req.extend_from_slice(&conn.setup.root.to_le_bytes());
    req.extend_from_slice(&RESOURCE_MANAGER.to_le_bytes());
    req.extend_from_slice(&0u32.to_le_bytes()); // any type
    req.extend_from_slice(&0u32.to_le_bytes()); // offset
    req.extend_from_slice(&MAX_WORDS.to_le_bytes());
    conn.send(&finish(req))?;
    let mut reply = [0u8; 32];
    conn.stream.read_exact(&mut reply)?;
    let words = rd_u32(&reply, 4).unwrap_or(0) as usize;
    let mut value = vec![0u8; words * 4];
    conn.stream.read_exact(&mut value)?;
    if reply[0] != 1 {
        return Ok(1.0); // an error packet, not a reply
    }
    Ok(parse_xft_scale(&String::from_utf8_lossy(&value)).unwrap_or(1.0))
}

fn parse_xft_scale(resources: &str) -> Option<f64> {
    let dpi: f64 = resources
        .lines()
        .find_map(|line| line.strip_prefix("Xft.dpi:"))?
        .trim_matches(|c: char| c.is_whitespace() || c == '\0')
        .parse()
        .ok()?;
    (dpi > 0.0).then_some(dpi / 96.0)
}

fn intern_atom(conn: &mut Conn, name: &[u8]) -> io::Result<u32> {
    let mut req = vec![OP_INTERN_ATOM, 0 /* only-if-exists = false */, 0, 0];
    req.extend_from_slice(&(name.len() as u16).to_le_bytes());
    req.extend_from_slice(&0u16.to_le_bytes()); // pad
    req.extend_from_slice(name);
    pad4(&mut req);
    conn.send(&finish(req))?;
    // InternAtom reply: 32-byte header; atom at offset 8.
    let mut reply = [0u8; 32];
    conn.stream.read_exact(&mut reply)?;
    Ok(rd_u32(&reply, 8).unwrap_or(0))
}

fn set_property_str(
    conn: &mut Conn,
    window: u32,
    prop: u32,
    ty: u32,
    val: &[u8],
) -> io::Result<()> {
    let mut req = vec![OP_CHANGE_PROPERTY, 0 /* Replace */, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&prop.to_le_bytes());
    req.extend_from_slice(&ty.to_le_bytes());
    req.push(8); // format
    req.extend_from_slice(&[0, 0, 0]); // pad
    req.extend_from_slice(&(val.len() as u32).to_le_bytes());
    req.extend_from_slice(val);
    pad4(&mut req);
    conn.send(&finish(req))
}

fn set_property_atoms(conn: &mut Conn, window: u32, prop: u32, atoms: &[u32]) -> io::Result<()> {
    set_property32(conn, window, prop, ATOM_ATOM, atoms)
}

/// Replace `prop` on `window` with a list of 32-bit `values` of type `ty`.
fn set_property32(
    conn: &mut Conn,
    window: u32,
    prop: u32,
    ty: u32,
    values: &[u32],
) -> io::Result<()> {
    conn.send(&finish(property32_request(window, prop, ty, values)))
}

/// A format-32 `ChangeProperty` (mode Replace), before [`finish`].
fn property32_request(window: u32, prop: u32, ty: u32, values: &[u32]) -> Vec<u8> {
    let mut req = vec![OP_CHANGE_PROPERTY, 0, 0, 0];
    req.extend_from_slice(&window.to_le_bytes());
    req.extend_from_slice(&prop.to_le_bytes());
    req.extend_from_slice(&ty.to_le_bytes());
    req.push(32); // format
    req.extend_from_slice(&[0, 0, 0]);
    req.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for v in values {
        req.extend_from_slice(&v.to_le_bytes());
    }
    req
}

/// Answer a `SelectionRequest`: fill the requestor's property with our clipboard
/// text (or the supported target list) and send back a `SelectionNotify`.
fn serve_selection_request(conn: &mut Conn, ev: &[u8; 32]) -> io::Result<()> {
    // SelectionRequest event fields: time@4, owner@8, requestor@12,
    // selection@16, target@20, property@24.
    let time = rd_u32(ev, 4).unwrap_or(0);
    let requestor = rd_u32(ev, 12).unwrap_or(0);
    let selection = rd_u32(ev, 16).unwrap_or(0);
    let target = rd_u32(ev, 20).unwrap_or(0);
    // An obsolete requestor may send property=None; reply into the target atom.
    let mut property = rd_u32(ev, 24).unwrap_or(0);
    if property == 0 {
        property = target;
    }

    // Only the CLIPBOARD selection is served. Decide whether we can satisfy the
    // requested target; on success the reply carries `property`, else `None`.
    let utf8 = conn.atom_utf8;
    let targets = conn.atom_targets;
    let satisfied = if selection != conn.atom_clipboard {
        false
    } else if target == targets {
        // Advertise the targets we support, as a list of ATOMs.
        let list = [targets, utf8, ATOM_STRING];
        set_property_atoms(conn, requestor, property, &list).is_ok()
    } else if target == utf8 || target == ATOM_STRING {
        let text = conn.clipboard_text.clone();
        set_property_str(conn, requestor, property, target, text.as_bytes()).is_ok()
    } else {
        false
    };
    let reply_prop = if satisfied { property } else { 0 };

    // Build a SelectionNotify event (type 31) and SendEvent it to the requestor.
    let mut notify = [0u8; 32];
    notify[0] = 31; // SelectionNotify
    notify[4..8].copy_from_slice(&time.to_le_bytes());
    notify[8..12].copy_from_slice(&requestor.to_le_bytes());
    notify[12..16].copy_from_slice(&selection.to_le_bytes());
    notify[16..20].copy_from_slice(&target.to_le_bytes());
    notify[20..24].copy_from_slice(&reply_prop.to_le_bytes());

    let mut req = vec![OP_SEND_EVENT, 0 /* propagate = false */, 0, 0];
    req.extend_from_slice(&requestor.to_le_bytes()); // destination
    req.extend_from_slice(&0u32.to_le_bytes()); // event-mask: deliver to creator
    req.extend_from_slice(&notify);
    conn.send(&finish(req))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xft_dpi_from_resources() {
        let res = "Xft.antialias:\t1\nXft.dpi:\t192\nXft.hinting:\t1\n";
        assert_eq!(parse_xft_scale(res), Some(2.0));
        assert_eq!(parse_xft_scale("Xft.dpi: 144\n"), Some(1.5));
        assert_eq!(parse_xft_scale("Xft.antialias:\t1\n"), None);
        assert_eq!(parse_xft_scale("Xft.dpi:\tbogus\n"), None);
    }

    #[test]
    fn dri3_open_request_is_well_formed() {
        // DRI3Open: major opcode, minor 1, length 3 words, drawable, provider 0.
        let req = dri3_open_request(0x90, 0x0012_3456);
        assert_eq!(req[0], 0x90);
        assert_eq!(req[1], DRI3_OPEN);
        assert_eq!(u16::from_le_bytes([req[2], req[3]]), 3);
        assert_eq!(rd_u32(&req, 4).unwrap(), 0x0012_3456);
        assert_eq!(rd_u32(&req, 8).unwrap(), 0);
        assert_eq!(req.len(), 12);
    }

    #[test]
    fn pixmap_from_buffers_request_is_well_formed() {
        // A single-plane ARGB8888 4096×2160 buffer with a tiled modifier.
        let img = DmabufImage {
            width: 4096,
            height: 2160,
            depth: 24,
            bpp: 32,
            modifier: 0x0100_0000_0000_0001, // an arbitrary 64-bit DRM modifier
            planes: vec![(16384, 0)],
        };
        let req = pixmap_from_buffers_request(0x90, 0x0040_0000, 0x0020_0001, &img);
        assert_eq!(req[0], 0x90);
        assert_eq!(req[1], DRI3_PIXMAP_FROM_BUFFERS);
        // Fixed wire size: header(4) + pixmap(4) + window(4) + num/pad/w/h(8)
        // + 8 plane words(32) + depth/bpp/pad(4) + modifier(8) = 64 bytes / 16 words.
        assert_eq!(u16::from_le_bytes([req[2], req[3]]), 16);
        assert_eq!(req.len(), 64);
        assert_eq!(rd_u32(&req, 4).unwrap(), 0x0040_0000); // pixmap
        assert_eq!(rd_u32(&req, 8).unwrap(), 0x0020_0001); // window
        assert_eq!(req[12], 1); // num_buffers
        assert_eq!(u16::from_le_bytes([req[16], req[17]]), 4096); // width
        assert_eq!(u16::from_le_bytes([req[18], req[19]]), 2160); // height
        assert_eq!(rd_u32(&req, 20).unwrap(), 16384); // stride0
        assert_eq!(rd_u32(&req, 24).unwrap(), 0); // offset0
        // Planes 1..3 are zero-filled (stride1@28 .. offset3@48).
        assert_eq!(rd_u32(&req, 28).unwrap(), 0);
        assert_eq!(rd_u32(&req, 48).unwrap(), 0);
        assert_eq!(req[52], 24); // depth
        assert_eq!(req[53], 32); // bpp
        // Modifier is the last 8 bytes (little-endian u64).
        let modifier = u64::from_le_bytes(req[56..64].try_into().unwrap());
        assert_eq!(modifier, 0x0100_0000_0000_0001);
    }

    #[test]
    fn present_pixmap_request_is_well_formed() {
        // wait_fence 0x0060_0002 exercises the frame-sync slot.
        let req = present_pixmap_request(0x91, 0x0020_0001, 0x0040_0000, 0x0060_0002, 7, 0);
        assert_eq!(req[0], 0x91);
        assert_eq!(req[1], PRESENT_PIXMAP);
        // 4-byte header + window+pixmap+serial+valid+update (5 words) + off word
        // + crtc+wait+idle+options+unused (5 words) + msc/divisor/remainder
        // (6 words) = 18 words.
        assert_eq!(u16::from_le_bytes([req[2], req[3]]), 18);
        assert_eq!(req.len(), 72);
        assert_eq!(rd_u32(&req, 4).unwrap(), 0x0020_0001); // window
        assert_eq!(rd_u32(&req, 8).unwrap(), 0x0040_0000); // pixmap
        assert_eq!(rd_u32(&req, 12).unwrap(), 7); // serial
        assert_eq!(rd_u32(&req, 16).unwrap(), 0); // valid region (None)
        assert_eq!(rd_u32(&req, 20).unwrap(), 0); // update region (None)
        // crtc@28, wait-fence@32 (the frame-sync fence), idle-fence@36.
        assert_eq!(rd_u32(&req, 28).unwrap(), 0); // target CRTC (None)
        assert_eq!(rd_u32(&req, 32).unwrap(), 0x0060_0002); // wait fence
        assert_eq!(rd_u32(&req, 36).unwrap(), 0); // idle fence (None)
        // target-msc/divisor/remainder are the final three zero u64s.
        assert_eq!(u64::from_le_bytes(req[48..56].try_into().unwrap()), 0);
        assert_eq!(u64::from_le_bytes(req[64..72].try_into().unwrap()), 0);
    }

    #[test]
    fn dri3_fence_from_fd_request_is_well_formed() {
        let req = dri3_fence_from_fd_request(0x90, 0x0020_0001, 0x0060_0002, true);
        assert_eq!(req[0], 0x90);
        assert_eq!(req[1], DRI3_FENCE_FROM_FD);
        // header(4) + drawable(4) + fence(4) + triggered/pad(4) = 16 bytes / 4 words.
        assert_eq!(u16::from_le_bytes([req[2], req[3]]), 4);
        assert_eq!(req.len(), 16);
        assert_eq!(rd_u32(&req, 4).unwrap(), 0x0020_0001); // drawable
        assert_eq!(rd_u32(&req, 8).unwrap(), 0x0060_0002); // fence
        assert_eq!(req[12], 1); // initially_triggered
    }

    #[test]
    fn selection_notify_send_event_is_well_formed() {
        // A SendEvent carrying a SelectionNotify is 44 bytes (11 words): 12-byte
        // header (opcode, propagate, length, destination, mask) + 32-byte event.
        let mut conn_ev = [0u8; 32];
        conn_ev[0] = X_SELECTION_REQUEST;
        conn_ev[12..16].copy_from_slice(&0xCAFEu32.to_le_bytes()); // requestor@12
        // (No live socket here; just assert the encoder shape via a hand build.)
        let requestor = rd_u32(&conn_ev, 12).unwrap();
        let mut notify = [0u8; 32];
        notify[0] = 31;
        let mut req = vec![OP_SEND_EVENT, 0, 0, 0];
        req.extend_from_slice(&requestor.to_le_bytes());
        req.extend_from_slice(&0u32.to_le_bytes());
        req.extend_from_slice(&notify);
        let req = finish(req);
        assert_eq!(req[0], OP_SEND_EVENT);
        assert_eq!(req.len(), 44);
        assert_eq!(u16::from_le_bytes([req[2], req[3]]), 11); // length in words
        assert_eq!(rd_u32(&req, 4).unwrap(), 0xCAFE); // destination
    }

    #[test]
    fn parses_display_forms() {
        assert_eq!(
            parse_display(":0"),
            Some(DisplayAddr {
                number: 0,
                screen: 0
            })
        );
        assert_eq!(
            parse_display(":1.2"),
            Some(DisplayAddr {
                number: 1,
                screen: 2
            })
        );
        assert_eq!(
            parse_display("host:3"),
            Some(DisplayAddr {
                number: 3,
                screen: 0
            })
        );
        assert_eq!(parse_display("garbage"), None);
    }

    #[test]
    fn rgba_to_bgrx_swizzles() {
        let mut out = Vec::new();
        rgba_to_bgrx(&[10, 20, 30, 255, 1, 2, 3, 128], &mut out);
        assert_eq!(out, vec![30, 20, 10, 0, 3, 2, 1, 0]);
    }

    #[test]
    fn parse_setup_reads_screen_fields() {
        // Build a minimal additional-data buffer: 32-byte fixed block, no
        // vendor, one pixmap format (8 bytes), then a screen — matching the
        // X11 connection-setup reply layout.
        let mut d = vec![0u8; 32];
        d[4..8].copy_from_slice(&0x0040_0000u32.to_le_bytes()); // resource-id-base
        d[8..12].copy_from_slice(&0x001f_ffffu32.to_le_bytes()); // resource-id-mask
        d[16..18].copy_from_slice(&0u16.to_le_bytes()); // vendor-length
        d[18..20].copy_from_slice(&65535u16.to_le_bytes()); // max-request-length
        d[21] = 1; // number-of-formats
        d.extend_from_slice(&[0u8; 8]); // one pixmap format
        // Screen: root at +0, root-visual at +32, root-depth at +38.
        let mut screen = vec![0u8; 40];
        screen[0..4].copy_from_slice(&0x0000_0123u32.to_le_bytes()); // root
        screen[32..36].copy_from_slice(&0x0000_0021u32.to_le_bytes()); // root-visual
        screen[38] = 24; // root-depth
        d.extend_from_slice(&screen);

        let s = parse_setup(&d).expect("parse");
        assert_eq!(s.resource_id_base, 0x0040_0000);
        assert_eq!(s.resource_id_mask, 0x001f_ffff);
        assert_eq!(s.root, 0x0000_0123);
        assert_eq!(s.root_visual, 0x0000_0021);
        assert_eq!(s.root_depth, 24);
        assert_eq!(s.max_request_len, 65535);
    }

    #[test]
    fn parse_xauth_finds_cookie() {
        // family(2) addrlen(2)=1 addr("a") numlen(2)=1 num("0")
        // namelen(2)=18 name("MIT-MAGIC-COOKIE-1") datalen(2)=4 data(DE AD BE EF)
        let mut e = Vec::new();
        e.extend_from_slice(&0u16.to_be_bytes());
        e.extend_from_slice(&1u16.to_be_bytes());
        e.push(b'a');
        e.extend_from_slice(&1u16.to_be_bytes());
        e.push(b'0');
        e.extend_from_slice(&18u16.to_be_bytes());
        e.extend_from_slice(b"MIT-MAGIC-COOKIE-1");
        e.extend_from_slice(&4u16.to_be_bytes());
        e.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(parse_xauth_cookie(&e), Some(vec![0xDE, 0xAD, 0xBE, 0xEF]));
        assert_eq!(parse_xauth_cookie(&[]), None);
    }

    #[test]
    fn finish_sets_length_in_words() {
        let req = finish(vec![1, 0, 0, 0, 0, 0, 0, 0]); // 8 bytes = 2 words
        assert_eq!(u16::from_le_bytes([req[2], req[3]]), 2);
    }

    #[test]
    fn swizzle_row_reorders_to_bgrx() {
        let mut dst = [0u8; 8];
        swizzle_row(&[10, 20, 30, 255, 1, 2, 3, 128], &mut dst);
        assert_eq!(dst, [30, 20, 10, 0, 3, 2, 1, 0]);
    }

    #[test]
    fn shm_put_image_encodes_fields() {
        // major=130, draw=0xaa, gc=0xbb, seg=0xcc, total 200x100, rect (8,16,4,2).
        let req = finish(shm_put_image(130, 0xaa, 0xbb, 0xcc, 200, 100, 8, 16, 4, 2));
        assert_eq!(req[0], 130); // major opcode
        assert_eq!(req[1], SHM_PUT_IMAGE); // minor opcode
        assert_eq!(u16::from_le_bytes([req[2], req[3]]), 10); // length in words
        assert_eq!(rd_u32(&req, 4), Some(0xaa)); // drawable
        assert_eq!(rd_u32(&req, 8), Some(0xbb)); // gc
        assert_eq!(u16::from_le_bytes([req[12], req[13]]), 200); // total-width
        assert_eq!(u16::from_le_bytes([req[14], req[15]]), 100); // total-height
        assert_eq!(u16::from_le_bytes([req[16], req[17]]), 8); // src-x
        assert_eq!(u16::from_le_bytes([req[18], req[19]]), 16); // src-y
        assert_eq!(u16::from_le_bytes([req[20], req[21]]), 4); // src-width
        assert_eq!(u16::from_le_bytes([req[22], req[23]]), 2); // src-height
        assert_eq!(i16::from_le_bytes([req[24], req[25]]), 8); // dst-x
        assert_eq!(i16::from_le_bytes([req[26], req[27]]), 16); // dst-y
        assert_eq!(req[28], 24); // depth
        assert_eq!(req[29], 2); // ZPixmap
        assert_eq!(req[30], 0); // send-event
        assert_eq!(rd_u32(&req, 32), Some(0xcc)); // shmseg
        assert_eq!(rd_u32(&req, 36), Some(0)); // offset
    }

    #[test]
    fn keysyms_map_navigation_and_function_keys() {
        let key = |ks| match keysym_to_event(ks, ButtonState::Pressed, Modifiers::default()) {
            Some(Event::Key { code, .. }) => Some(code),
            _ => None,
        };
        assert_eq!(key(0xff51), Some(KeyCode::ArrowLeft)); // unchanged
        assert_eq!(key(0xff50), Some(KeyCode::Home));
        assert_eq!(key(0xff55), Some(KeyCode::PageUp));
        assert_eq!(key(0xff56), Some(KeyCode::PageDown));
        assert_eq!(key(0xff63), Some(KeyCode::Insert));
        assert_eq!(key(0xffbe), Some(KeyCode::F1));
        assert_eq!(key(0xffc3), Some(KeyCode::F6));
        assert_eq!(key(0xffc9), Some(KeyCode::F12));
        assert_eq!(key(0xffca), None); // F13
        // Ctrl turns a printable key into a shortcut; without it, it's text.
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            keysym_to_event(0x63, ButtonState::Pressed, ctrl),
            Some(Event::Key {
                code: KeyCode::Char('c'),
                state: ButtonState::Pressed,
                modifiers: ctrl,
            })
        );
        assert_eq!(
            keysym_to_event(0x63, ButtonState::Pressed, Modifiers::default()),
            Some(Event::Text("c".into()))
        );
    }

    #[test]
    fn size_hints_carry_min_and_max_in_physical_pixels() {
        let hints = size_hints(
            ScaleFactor::new(2.0),
            Some(Size::new(100.0, 50.0)),
            Some(Size::new(400.0, 300.0)),
        );
        assert_eq!(hints[0], P_MIN_SIZE | P_MAX_SIZE);
        assert_eq!(&hints[5..9], &[200, 100, 800, 600]);
        assert_eq!(size_hints(ScaleFactor::IDENTITY, None, None), [0; 18]);
    }

    #[test]
    fn icon_words_are_argb_after_the_size() {
        let mut icon = Pixmap::new(PhysicalSize::new(2, 1));
        icon.as_bytes_mut()
            .copy_from_slice(&[0x11, 0x22, 0x33, 0x44, 0xaa, 0xbb, 0xcc, 0xff]);
        assert_eq!(icon_words(&icon), vec![2, 1, 0x4411_2233, 0xffaa_bbcc]);
        // Large icons are scaled to fit one request.
        let big = icon_words(&Pixmap::new(PhysicalSize::new(512, 256)));
        assert_eq!(&big[..2], &[MAX_ICON, MAX_ICON / 2]);
    }

    #[test]
    fn wm_state_message_is_a_root_client_message() {
        let req = wm_state_message(0x100, 0x200, 7, true, 8, 9);
        assert_eq!(req.len(), 44); // 12-byte header + 32-byte event
        assert_eq!(u16::from_le_bytes([req[2], req[3]]), 11);
        assert_eq!(u32::from_le_bytes(req[4..8].try_into().unwrap()), 0x100);
        assert_eq!(
            u32::from_le_bytes(req[8..12].try_into().unwrap()),
            EV_SUBSTRUCTURE
        );
        assert_eq!(&req[12..14], &[X_CLIENT_MESSAGE, 32]);
        let word = |i: usize| u32::from_le_bytes(req[i..i + 4].try_into().unwrap());
        assert_eq!((word(16), word(20)), (0x200, 7));
        assert_eq!((word(24), word(28), word(32), word(36)), (1, 8, 9, 1));
    }

    #[test]
    fn glyph_cursor_request_is_eight_words() {
        let req = glyph_cursor_request(5, 6, 152);
        assert_eq!(req.len(), 32);
        assert_eq!(u16::from_le_bytes([req[2], req[3]]), 8);
        assert_eq!(u16::from_le_bytes([req[16], req[17]]), 152);
        assert_eq!(u16::from_le_bytes([req[18], req[19]]), 153);
        let prop = property32_request(1, 2, 3, &[4, 5]);
        assert_eq!(prop.len(), 24 + 8);
    }

    #[test]
    fn wheel_buttons_scroll_a_line() {
        assert_eq!(
            wheel_button_delta(4),
            Some(ScrollDelta { dx: 0.0, dy: -40.0 })
        );
        assert_eq!(
            wheel_button_delta(5),
            Some(ScrollDelta { dx: 0.0, dy: 40.0 })
        );
        assert_eq!(
            wheel_button_delta(6),
            Some(ScrollDelta { dx: -40.0, dy: 0.0 })
        );
        assert_eq!(
            wheel_button_delta(7),
            Some(ScrollDelta { dx: 40.0, dy: 0.0 })
        );
        assert_eq!(wheel_button_delta(1), None);
    }
}
