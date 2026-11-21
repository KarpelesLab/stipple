//! How a window's swapchain images are composited: opaque, or for a
//! transparent window (an X11 window with the ARGB visual, a translucent
//! Android window) with the frame's premultiplied alpha.

use super::super::sys::{
    COMPOSITE_INHERIT, COMPOSITE_OPAQUE, COMPOSITE_POST_MULTIPLIED, COMPOSITE_PRE_MULTIPLIED,
};

/// The `compositeAlpha` to make a swapchain with, of those the surface
/// supports (`supported`): an opaque window's best first; for a transparent
/// one (`alpha`), `PRE_MULTIPLIED` (the frames are premultiplied), else
/// `INHERIT`, where the window system takes the alpha as it does for the
/// window's own pixels (premultiplied, on X11 and Android). `None` when a
/// transparent window can't keep its alpha (only `OPAQUE`, or
/// `POST_MULTIPLIED`, which would darken the frames' colors twice).
pub(super) fn composite_alpha(supported: u32, alpha: bool) -> Option<u32> {
    let order: &[u32] = if alpha {
        &[COMPOSITE_PRE_MULTIPLIED, COMPOSITE_INHERIT]
    } else {
        &[
            COMPOSITE_OPAQUE,
            COMPOSITE_INHERIT,
            COMPOSITE_PRE_MULTIPLIED,
            COMPOSITE_POST_MULTIPLIED,
        ]
    };
    match order.iter().copied().find(|a| supported & a != 0) {
        Some(a) => Some(a),
        None if alpha => None,
        None => Some(COMPOSITE_OPAQUE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_windows_take_premultiplied_alpha() {
        // Mesa on an ARGB visual: INHERIT | PRE_MULTIPLIED.
        let argb = COMPOSITE_INHERIT | COMPOSITE_PRE_MULTIPLIED;
        assert_eq!(composite_alpha(argb, true), Some(COMPOSITE_PRE_MULTIPLIED));
        // Android: OPAQUE | INHERIT; the window's format keeps the alpha.
        let android = COMPOSITE_OPAQUE | COMPOSITE_INHERIT;
        assert_eq!(composite_alpha(android, true), Some(COMPOSITE_INHERIT));
        assert_eq!(composite_alpha(android, false), Some(COMPOSITE_OPAQUE));
        // A surface that is opaque, or only post-multiplies: no alpha.
        assert_eq!(composite_alpha(COMPOSITE_OPAQUE, true), None);
        assert_eq!(composite_alpha(COMPOSITE_POST_MULTIPLIED, true), None);
        // An opaque window takes what there is.
        assert_eq!(composite_alpha(argb, false), Some(COMPOSITE_INHERIT));
        assert_eq!(composite_alpha(0, false), Some(COMPOSITE_OPAQUE));
    }
}
