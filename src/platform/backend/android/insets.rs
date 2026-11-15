//! The geometry of the system bars' insets: where the app draws, and what
//! of the bars overlaps it (Android-free, tested on hosts).

/// `ARect`, as `onContentRectChanged` reports it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ARect {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

/// The system bars' insets (with the display cutout's), and the keyboard's
/// height, in window pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WindowInsets {
    pub(crate) bars: ARect,
    pub(crate) ime: i32,
}

/// The content rectangle: the window (or, unless edge to edge, the content
/// rectangle `NativeActivity` reports, which shrinks for the keyboard on
/// older Androids) less the system bars unless edge to edge. The keyboard
/// doesn't shrink it: the App lays the view out above
/// [`keyboard`]. The whole window if that leaves nothing.
pub(crate) fn content_rect(
    window: (i32, i32),
    native: Option<ARect>,
    insets: WindowInsets,
    edge: bool,
) -> ARect {
    let (w, h) = window;
    let full = ARect {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    let mut r = match native {
        Some(r) if !edge => ARect {
            left: r.left.clamp(0, w),
            top: r.top.clamp(0, h),
            right: r.right.clamp(0, w),
            bottom: r.bottom.clamp(0, h),
        },
        _ => full,
    };
    let b = insets.bars;
    if !edge {
        r.left = r.left.max(b.left);
        r.top = r.top.max(b.top);
        r.right = r.right.min(w - b.right);
        r.bottom = r.bottom.min(h - b.bottom);
    }
    if r.right > r.left && r.bottom > r.top {
        r
    } else {
        full
    }
}

/// How far up from `content`'s bottom the keyboard reaches, in window
/// pixels (zero when the content already ends above it).
pub(crate) fn keyboard(window: (i32, i32), content: ARect, insets: WindowInsets) -> i32 {
    if insets.ime <= 0 {
        return 0;
    }
    (content.bottom - (window.1 - insets.ime)).clamp(0, content.bottom - content.top)
}

/// What of the system bars still overlaps `content`, in window pixels from
/// each of its edges: the safe area (zero unless edge to edge).
pub(crate) fn safe_area(window: (i32, i32), content: ARect, insets: WindowInsets) -> ARect {
    let (w, h) = window;
    let b = insets.bars;
    ARect {
        left: (b.left - content.left).max(0),
        top: (b.top - content.top).max(0),
        right: (b.right - (w - content.right)).max(0),
        bottom: (b.bottom - (h - content.bottom)).max(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: (i32, i32) = (1080, 2400);
    const BARS: ARect = ARect {
        left: 0,
        top: 120,
        right: 0,
        bottom: 63,
    };

    #[test]
    fn the_content_keeps_clear_of_the_bars_unless_edge_to_edge() {
        let insets = WindowInsets { bars: BARS, ime: 0 };
        // Android 15 lays every app out edge to edge: the content rectangle
        // is the whole window, so the bars come off it.
        let full = Some(ARect {
            left: 0,
            top: 0,
            right: 1080,
            bottom: 2400,
        });
        let r = content_rect(PHONE, full, insets, false);
        assert_eq!((r.top, r.bottom), (120, 2400 - 63));
        assert_eq!(safe_area(PHONE, r, insets), ARect::default());
        // Edge to edge: all of it, the bars as the safe area.
        let r = content_rect(PHONE, full, insets, true);
        assert_eq!((r.top, r.bottom), (0, 2400));
        assert_eq!(safe_area(PHONE, r, insets), BARS);
        // The keyboard up: the content stays, and the keyboard covers its
        // bottom 900 pixels (the App lays the view out above them; the
        // navigation bar under it is covered too).
        let typing = WindowInsets {
            bars: BARS,
            ime: 900,
        };
        let r = content_rect(PHONE, full, typing, true);
        assert_eq!(r.bottom, 2400);
        assert_eq!(keyboard(PHONE, r, typing), 900);
        assert_eq!(safe_area(PHONE, r, typing).top, 120);
        // Not edge to edge, the content ends above the navigation bar, which
        // the keyboard's height includes.
        let r = content_rect(PHONE, full, typing, false);
        assert_eq!(keyboard(PHONE, r, typing), 900 - 63);
        // An older Android shrinking the content for the keyboard itself.
        let shrunk = Some(ARect {
            left: 0,
            top: 0,
            right: 1080,
            bottom: 1500,
        });
        let r = content_rect(PHONE, shrunk, typing, false);
        assert_eq!(keyboard(PHONE, r, typing), 0);
        assert_eq!(keyboard(PHONE, r, insets), 0);
    }
}
