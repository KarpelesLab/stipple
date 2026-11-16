//! What the Vulkan and GLES executors share to present a Wayland window
//! through `dma-buf`s ([`crate::platform::dmabuf`]): picking a format the
//! compositor takes, and a small ring of exported buffers per window, each
//! drawn into only once the compositor has released it.
//!
//! Each frame is drawn into the executor's persistent frame target as for any
//! present (only the damage, over the window's last frame), then copied whole
//! into a free buffer of the ring, which is committed with the damage. The
//! executor waits for the GPU before the commit.

use crate::gpu::renderer::GpuError;
use crate::platform::GpuLayer;
use crate::platform::dmabuf::{
    ABGR8888, ARGB8888, BufferId, MOD_INVALID, Present, XBGR8888, XRGB8888,
};
use std::time::Duration;

/// Buffers per window, at most: two in the compositor's hands (one shown,
/// one queued) and one being drawn.
pub(crate) const MAX_BUFFERS: usize = 3;
/// How long a frame waits for the compositor to release a buffer.
const RELEASE_WAIT: Duration = Duration::from_millis(100);

/// The pixel layout a buffer is drawn in, as the executor knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "vk"), allow(dead_code))]
pub(crate) enum Layout {
    /// `[R, G, B, A]` bytes, the frame target's own: a straight copy.
    Rgba,
    /// `[B, G, R, A]` bytes: a swizzling copy (a blit).
    Bgra,
}

/// The fourccs the executors draw, best first: opaque before alpha (the
/// compositor needn't blend a window's background), the frame target's own
/// byte order before a swizzle.
#[cfg_attr(not(feature = "vk"), allow(dead_code))]
pub(crate) const FOURCCS: [(u32, Layout); 4] = [
    (XBGR8888, Layout::Rgba),
    (ABGR8888, Layout::Rgba),
    (XRGB8888, Layout::Bgra),
    (ARGB8888, Layout::Bgra),
];

/// The first of `FOURCCS` the compositor takes (`offered`) with a modifier
/// the driver can draw (`drawable(fourcc, layout)`, its modifiers for that
/// format, best first; LINEAR stands for "a linear image" where the driver
/// can't say): the fourcc, how it's drawn, and the modifiers both take, in
/// the driver's order. A compositor that gives no modifier for a format
/// (`MOD_INVALID`) is taken to mean LINEAR is fine only if it says so too.
#[cfg_attr(not(feature = "vk"), allow(dead_code))]
pub(crate) fn choose(
    offered: &[(u32, u64)],
    mut drawable: impl FnMut(u32, Layout) -> Vec<u64>,
) -> Option<(u32, Layout, Vec<u64>)> {
    FOURCCS.iter().find_map(|&(fourcc, layout)| {
        let theirs: Vec<u64> = offered
            .iter()
            .filter(|(f, m)| *f == fourcc && *m != MOD_INVALID)
            .map(|(_, m)| *m)
            .collect();
        if theirs.is_empty() {
            return None;
        }
        let both: Vec<u64> = drawable(fourcc, layout)
            .into_iter()
            .filter(|m| theirs.contains(m))
            .collect();
        (!both.is_empty()).then_some((fourcc, layout, both))
    })
}

/// The window's sink and key, for a Wayland layer.
pub(crate) fn sink_of(layer: &GpuLayer) -> Option<(u32, &dyn Present)> {
    match *layer {
        // The backend keeps the sink alive while the layer can be presented
        // to, and releases the layer before it goes.
        GpuLayer::Wayland { surface, sink } if !sink.is_null() => {
            Some((surface, unsafe { &*sink }.0.as_ref()))
        }
        _ => None,
    }
}

/// One buffer of a window's ring: what the executor drew into, and the
/// compositor's id for it.
pub(crate) struct Slot<T> {
    pub image: T,
    pub id: BufferId,
}

/// A window's buffers, all `size` pixels of one format.
pub(crate) struct Ring<T> {
    pub size: (u32, u32),
    pub slots: Vec<Slot<T>>,
    /// Frames committed (the first of a ring shows all of it).
    pub commits: u64,
}

impl<T> Ring<T> {
    pub(crate) fn new(size: (u32, u32)) -> Self {
        Ring {
            size,
            slots: Vec::new(),
            commits: 0,
        }
    }

    /// The index of a buffer free to draw into: one the compositor has
    /// released, else a new one (`make`, up to [`MAX_BUFFERS`]), else the
    /// first to be released within a short wait. `Skipped` when none is.
    pub(crate) fn acquire(
        &mut self,
        sink: &dyn Present,
        make: impl FnOnce() -> Result<Slot<T>, GpuError>,
    ) -> Result<usize, GpuError> {
        if let Some(i) = self.slots.iter().position(|s| sink.released(s.id)) {
            return Ok(i);
        }
        if self.slots.len() < MAX_BUFFERS {
            self.slots.push(make()?);
            return Ok(self.slots.len() - 1);
        }
        let ids: Vec<BufferId> = self.slots.iter().map(|s| s.id).collect();
        let id = sink
            .wait_release(&ids, RELEASE_WAIT)
            .ok_or(GpuError::Skipped)?;
        Ok(self.slots.iter().position(|s| s.id == id).unwrap())
    }

    /// Let go of every buffer: the compositor's ids, then (`free`) the
    /// executor's images.
    pub(crate) fn clear(&mut self, sink: Option<&dyn Present>, mut free: impl FnMut(T)) {
        for slot in self.slots.drain(..) {
            if let Some(sink) = sink {
                sink.destroy(slot.id);
            }
            free(slot.image);
        }
    }

    /// The damage to commit for a frame whose change was `dirty`: all of a
    /// ring's first frame.
    pub(crate) fn damage(&mut self, dirty: Option<[i32; 4]>) -> Option<[i32; 4]> {
        let first = self.commits == 0;
        self.commits += 1;
        dirty.filter(|_| !first)
    }
}

/// Say once how a Wayland window's frames are presented (CI looks for it).
pub(crate) fn say_presenting(api: &str, fourcc: u32, modifier: u64, planes: usize) {
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| {
        eprintln!(
            "stipple: presenting with {api} to the Wayland window through linux-dmabuf ({}, modifier {modifier:#x}, {planes} plane{})",
            crate::platform::dmabuf::fourcc_name(fourcc),
            if planes == 1 { "" } else { "s" }
        )
    });
}

/// Say once why a Wayland window's GPU frames are read back instead.
pub(crate) fn say_reading_back(api: &str, why: &str) {
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| {
        eprintln!("stipple: {api} can't present to the Wayland window ({why}); reading frames back")
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::dmabuf::MOD_LINEAR;

    /// LINEAR, the one modifier a linear image has.
    fn linear() -> Vec<u64> {
        vec![MOD_LINEAR]
    }
    use crate::platform::dmabuf::Buffer;
    use std::cell::RefCell;

    #[test]
    fn choose_takes_the_first_format_both_sides_have() {
        let offered = [
            (ARGB8888, MOD_LINEAR),
            (XBGR8888, 7),
            (XBGR8888, MOD_LINEAR),
            (ABGR8888, MOD_INVALID),
        ];
        // The driver draws XBGR as 7 or 9, preferring 9: only 7 is taken.
        let pick = choose(
            &offered,
            |f, _| {
                if f == XBGR8888 { vec![9, 7] } else { linear() }
            },
        );
        assert_eq!(pick, Some((XBGR8888, Layout::Rgba, vec![7])));
        // Linear only: XBGR8888 linear.
        let pick = choose(&offered, |_, _| linear());
        assert_eq!(pick, Some((XBGR8888, Layout::Rgba, vec![MOD_LINEAR])));
        // ABGR without a modifier doesn't count; ARGB linear does (a blit).
        let offered = [(ABGR8888, MOD_INVALID), (ARGB8888, MOD_LINEAR)];
        let pick = choose(&offered, |_, _| linear());
        assert_eq!(pick, Some((ARGB8888, Layout::Bgra, vec![MOD_LINEAR])));
        // Nothing in common.
        assert_eq!(choose(&offered, |_, _| vec![5]), None);
        assert_eq!(choose(&[], |_, _| linear()), None);
    }

    /// A compositor that holds every buffer committed until told otherwise.
    #[derive(Default)]
    struct Fake {
        held: RefCell<Vec<BufferId>>,
        next: RefCell<BufferId>,
        destroyed: RefCell<Vec<BufferId>>,
        release_on_wait: RefCell<Option<BufferId>>,
    }

    impl Present for Fake {
        fn formats(&self) -> Vec<(u32, u64)> {
            vec![(XBGR8888, MOD_LINEAR)]
        }
        fn ready(&self) -> bool {
            true
        }
        fn import(&self, _: &Buffer) -> Result<BufferId, String> {
            *self.next.borrow_mut() += 1;
            Ok(*self.next.borrow())
        }
        fn released(&self, id: BufferId) -> bool {
            !self.held.borrow().contains(&id)
        }
        fn wait_release(&self, ids: &[BufferId], _: Duration) -> Option<BufferId> {
            let id = self.release_on_wait.borrow_mut().take()?;
            self.held.borrow_mut().retain(|&h| h != id);
            ids.contains(&id).then_some(id)
        }
        fn commit(&self, id: BufferId, _: (u32, u32), _: Option<[i32; 4]>) -> Result<(), String> {
            self.held.borrow_mut().push(id);
            Ok(())
        }
        fn destroy(&self, id: BufferId) {
            self.destroyed.borrow_mut().push(id);
        }
    }

    #[test]
    fn the_ring_grows_to_three_then_waits_for_a_release() {
        let sink = Fake::default();
        let mut ring: Ring<u32> = Ring::new((4, 4));
        let mut made = 0;
        let frame = |ring: &mut Ring<u32>, made: &mut u32| -> Result<usize, GpuError> {
            let i = ring.acquire(&sink, || {
                *made += 1;
                let id = sink
                    .import(&Buffer {
                        width: 4,
                        height: 4,
                        fourcc: XBGR8888,
                        modifier: MOD_LINEAR,
                        planes: Vec::new(),
                    })
                    .map_err(GpuError::Lost)?;
                Ok(Slot { image: *made, id })
            })?;
            sink.commit(ring.slots[i].id, (4, 4), None).unwrap();
            Ok(i)
        };
        // Three frames, three buffers, all held.
        for want in 0..3 {
            assert_eq!(frame(&mut ring, &mut made).unwrap(), want);
        }
        assert_eq!(made, 3);
        // A fourth: none free and none released in time.
        assert!(matches!(
            frame(&mut ring, &mut made),
            Err(GpuError::Skipped)
        ));
        // The compositor lets go of the second one while we wait.
        *sink.release_on_wait.borrow_mut() = Some(2);
        assert_eq!(frame(&mut ring, &mut made).unwrap(), 1);
        assert_eq!(made, 3);
        // A ring's first commit damages all of it.
        assert_eq!(ring.damage(Some([0, 0, 1, 1])), None);
        assert_eq!(ring.damage(Some([0, 0, 1, 1])), Some([0, 0, 1, 1]));
        let mut freed = Vec::new();
        ring.clear(Some(&sink), |img| freed.push(img));
        assert_eq!(freed, vec![1, 2, 3]);
        assert_eq!(*sink.destroyed.borrow(), vec![1, 2, 3]);
    }
}
