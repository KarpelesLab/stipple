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
//! commits it as a frame. Buffers come back free with `wl_buffer.release`.

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

/// What a window's backend does with the buffers a renderer gives it.
pub(crate) trait Present {
    /// The formats and modifiers the compositor takes, its preferred ones
    /// first. Empty when it takes none (or said nothing yet and won't).
    fn formats(&self) -> Vec<(u32, u64)>;
    /// Whether the window can show a buffer yet (Wayland: after its first
    /// configure).
    fn ready(&self) -> bool;
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
    /// since the last one shown.
    fn commit(
        &self,
        id: BufferId,
        size: (u32, u32),
        damage: Option<[i32; 4]>,
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
}
