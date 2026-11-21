//! Presenting GPU frames to a window as `dma-buf`s: the seam between a GPU
//! renderer that draws into exportable images and a backend that hands them
//! to the compositor (Wayland's `zwp_linux_dmabuf_v1`, written by hand in
//! `backend/wayland/dmabuf.rs`).
//!
//! The backend speaks Wayland over its own socket, so a renderer can't make
//! a `VkSurfaceKHR` or a `wl_egl_window` for the window (both take
//! libwayland's proxies). Instead the renderer exports what it drew as
//! `dma-buf` file descriptors and gives them to the window's [`Sink`]: the
//! backend wraps each in a `wl_buffer` once, and attaches, damages and
//! commits it as a frame. Buffers come back free with `wl_buffer.release`,
//! or, with explicit synchronization (`wp_linux_drm_syncobj_v1`), once the
//! compositor signals the buffer's release point; the renderer then hands a
//! fence (a `sync_file`) with each commit instead of waiting for the GPU.
//!
//! Which GPU renders matters too: the compositor names its main device (a
//! `dev_t`), published here ([`set_main_device`]) before a renderer is made
//! so it can pick the same one (`VK_EXT_physical_device_drm`,
//! `EGL_EXT_device_drm`).

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU64, Ordering};

/// A DRM fourcc code (`drm_fourcc.h`), from its four characters.
pub(crate) const fn fourcc(code: &[u8; 4]) -> u32 {
    code[0] as u32 | (code[1] as u32) << 8 | (code[2] as u32) << 16 | (code[3] as u32) << 24
}

/// 32-bit pixels, one byte each of `[R, G, B, X]` in memory (`XB24`).
pub(crate) const XBGR8888: u32 = fourcc(b"XB24");
/// `[R, G, B, A]` in memory (`AB24`).
pub(crate) const ABGR8888: u32 = fourcc(b"AB24");
/// `[B, G, R, X]` in memory (`XR24`).
#[cfg_attr(not(feature = "vk"), allow(dead_code))]
pub(crate) const XRGB8888: u32 = fourcc(b"XR24");
/// `[B, G, R, A]` in memory (`AR24`).
#[cfg_attr(not(feature = "vk"), allow(dead_code))]
pub(crate) const ARGB8888: u32 = fourcc(b"AR24");

/// The linear layout: rows one after the other, `stride` bytes apart.
#[cfg_attr(not(feature = "vk"), allow(dead_code))]
pub(crate) const MOD_LINEAR: u64 = 0;
/// "No modifier given": the layout is whatever the driver implies.
pub(crate) const MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// A fourcc as its four characters, for logs.
pub(crate) fn fourcc_name(code: u32) -> String {
    code.to_le_bytes()
        .iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
        .collect()
}

/// One memory plane of a buffer: the `dma-buf` it is in (borrowed: the
/// renderer keeps it open), and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Plane {
    pub fd: i32,
    pub offset: u32,
    pub stride: u32,
}

/// A buffer the renderer drew into, described for the compositor.
#[derive(Clone, Debug)]
pub(crate) struct Buffer {
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub planes: Vec<Plane>,
}

/// The backend's id for a buffer it imported (its `wl_buffer`).
pub(crate) type BufferId = u32;

/// One tranche of the formats the compositor takes: the device it reads
/// them with (`target`, a `dev_t`), whether they can go straight to the
/// display (`scanout`), and the formats with their modifiers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tranche {
    pub target: Option<u64>,
    pub scanout: bool,
    pub formats: Vec<(u32, u64)>,
}

/// Every tranche's formats, in order, without repeats.
#[cfg(test)]
pub(crate) fn flatten(tranches: &[Tranche]) -> Vec<(u32, u64)> {
    let mut out: Vec<(u32, u64)> = Vec::new();
    for f in tranches.iter().flat_map(|t| &t.formats) {
        if !out.contains(f) {
            out.push(*f);
        }
    }
    out
}

/// The compositor's main device (`dev_t` + 1; 0 for unknown).
static MAIN_DEVICE: AtomicU64 = AtomicU64::new(0);

/// Publish the compositor's main device, for the renderer made next.
pub(crate) fn set_main_device(dev: u64) {
    MAIN_DEVICE.store(dev.wrapping_add(1), Ordering::Relaxed);
}

/// The compositor's main device (a `dev_t`), if one said.
#[cfg_attr(not(any(feature = "vk", feature = "gl")), allow(dead_code))]
pub(crate) fn main_device() -> Option<u64> {
    match MAIN_DEVICE.load(Ordering::Relaxed) {
        0 => None,
        d => Some(d - 1),
    }
}

/// The device number of the device file at `path` (`st_rdev`).
#[cfg_attr(not(any(feature = "vk", feature = "gl")), allow(dead_code))]
pub(crate) fn dev_of(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.rdev())
}

/// A `dev_t` from its major and minor numbers (glibc's `makedev`).
#[cfg_attr(not(feature = "vk"), allow(dead_code))]
pub(crate) fn makedev(major: u64, minor: u64) -> u64 {
    ((major & 0xffff_f000) << 32)
        | ((major & 0xfff) << 8)
        | ((minor & 0xffff_ff00) << 12)
        | (minor & 0xff)
}

/// What a window's backend does with the buffers a renderer gives it.
pub(crate) trait Present {
    /// The formats and modifiers the compositor takes, by tranche, its
    /// preferred ones first. Empty when it takes none (or said nothing yet
    /// and won't).
    fn tranches(&self) -> Vec<Tranche>;
    /// Whether commits take a fence the compositor waits for (explicit
    /// synchronization): then the renderer needn't wait for the GPU first.
    fn explicit_sync(&self) -> bool {
        false
    }
    /// Whether the window can show a buffer yet (Wayland: after its first
    /// configure).
    fn ready(&self) -> bool;
    /// Whether the window is transparent: its buffers must keep the frames'
    /// (premultiplied) alpha, in a format with alpha.
    fn alpha(&self) -> bool {
        false
    }
    /// Wrap `buffer` for the compositor (once per buffer; the fds may be
    /// closed after).
    fn import(&self, buffer: &Buffer) -> Result<BufferId, String>;
    /// Whether the compositor is done with `id` (it can be drawn into).
    fn released(&self, id: BufferId) -> bool;
    /// Wait until one of `ids` is released, up to `timeout`: the first one
    /// that is, or `None`.
    fn wait_release(&self, ids: &[BufferId], timeout: std::time::Duration) -> Option<BufferId>;
    /// Show `id` as the window's next frame, `size` pixels, with `damage`
    /// (`[x0, y0, x1, y1]` in buffer pixels; `None` for all of it) changed
    /// since the last one shown. `fence`: a `sync_file` that signals once
    /// the GPU has drawn it (with [`explicit_sync`](Present::explicit_sync);
    /// `None` when the renderer waited for the GPU instead).
    fn commit(
        &self,
        id: BufferId,
        size: (u32, u32),
        damage: Option<[i32; 4]>,
        fence: Option<OwnedFd>,
    ) -> Result<(), String>;
    /// Let go of `id` (the renderer freed its memory or will).
    fn destroy(&self, id: BufferId);
}

/// A window's [`Present`], at an address that stays put while the window
/// lives: [`GpuLayer::Wayland`](crate::platform::GpuLayer::Wayland) points at
/// it. The backend releases the layer through the renderer before the window
/// (and this) go away.
pub(crate) struct Sink(pub(crate) Box<dyn Present>);

impl std::fmt::Debug for Sink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sink")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourccs_match_drm_fourcc_h() {
        // From drm_fourcc.h: fourcc_code('X', 'B', '2', '4') and friends.
        assert_eq!(XBGR8888, 0x3432_4258);
        assert_eq!(ABGR8888, 0x3432_4241);
        assert_eq!(XRGB8888, 0x3432_5258);
        assert_eq!(ARGB8888, 0x3432_5241);
        assert_eq!(fourcc_name(ABGR8888), "AB24");
        assert_eq!(MOD_INVALID, (1u64 << 56) - 1);
    }

    #[test]
    fn tranches_flatten_in_order_and_devices_compose() {
        let t = |formats: Vec<(u32, u64)>| Tranche {
            formats,
            ..Tranche::default()
        };
        let all = flatten(&[t(vec![(1, 0), (2, 0)]), t(vec![(2, 0), (1, 5)])]);
        assert_eq!(all, vec![(1, 0), (2, 0), (1, 5)]);
        // /dev/dri/renderD128 is 226:128; card0 226:0.
        assert_eq!(makedev(226, 128), 0xe280);
        assert_eq!(makedev(226, 0), 0xe200);
        assert_eq!(makedev(259, 0x1_2345), 0x1231_0345);
    }
}
