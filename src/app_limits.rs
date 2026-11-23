//! A window's size limits from its content's layout
//! ([`App::layout_limits`](crate::App::layout_limits)), and its natural size
//! to open at ([`App::natural_size`](crate::App::natural_size)).
//!
//! Each build measures the view's root for the smallest size it fits in and
//! the largest it has a use for ([`content_limits`]); the caller's own
//! `min_size`/`max_size` narrow them, and what comes out is given the window
//! when it changes, the window resized into it when it's outside.

use crate::core::{Element, content_limits, natural_size, same_measure};
use crate::geometry::Size;
use crate::platform::{Window, WindowAttributes};
use crate::render::Font;

/// Whether windows here can be resized at all: phones and the web give the
/// app the screen or the page's canvas, so there is nothing to limit.
pub(crate) const RESIZABLE_WINDOWS: bool = !cfg!(any(
    target_os = "android",
    target_os = "ios",
    target_arch = "wasm32"
));

/// The limits a pane's window has, as `(min, max)`: `None` for no limit,
/// an infinite side for none on that side.
pub(crate) type SizeLimits = (Option<Size>, Option<Size>);

/// What a pane keeps of its content's size.
#[derive(Default)]
pub(crate) struct Limits {
    /// The content's `(min, max)` as last built, when the window follows it.
    layout: Option<(Size, Size)>,
    /// The content's natural size, measured when the window is about to
    /// open at it.
    natural: Option<Size>,
    /// Whether the next build measures the natural size.
    want_natural: bool,
    /// The limits last given the window.
    sent: Option<SizeLimits>,
    /// The window's size when it was last asked into its limits.
    asked_at: Option<Size>,
    /// What [`layout`](Self::layout) was measured from: the view's root,
    /// the width, the font, and how many times fonts' chains had grown.
    /// A rebuild that measures alike (only colors or handlers changed, a
    /// hovered button) keeps the limits rather than measuring again.
    measured_from: Option<(Element, f64, Option<Font>, u64)>,
}

impl Limits {
    /// Measure the built view's `root`, laid out `width` wide, as `attrs`
    /// say: nothing when the window doesn't follow its content, or when the
    /// content follows the window (the view read its size: limits from such
    /// content would chase the window, which would chase them).
    ///
    /// The root is kept (it was built to be dropped after layout) for the
    /// next build to compare with.
    pub(crate) fn measure(
        &mut self,
        root: Element,
        attrs: &WindowAttributes,
        width: f64,
        follows_window: bool,
        font: Option<&Font>,
    ) {
        if !RESIZABLE_WINDOWS {
            return;
        }
        if self.want_natural {
            self.natural = Some(natural_size(&root, font));
        }
        if !attrs.layout_limits || follows_window {
            self.layout = None;
            self.measured_from = None;
            return;
        }
        let grown = crate::render::font::chains_grown();
        let same = self.layout.is_some()
            && self.measured_from.as_ref().is_some_and(|(r, w, f, g)| {
                *w == width && f.as_ref() == font && *g == grown && same_measure(r, &root)
            });
        if !same {
            self.layout = Some(content_limits(&root, width, font));
        }
        self.measured_from = Some((root, width, font.cloned(), grown));
    }

    /// Ask the next build for the content's natural size (or stop).
    pub(crate) fn want_natural(&mut self, on: bool) {
        self.want_natural = on;
    }

    /// The natural size measured, once.
    pub(crate) fn take_natural(&mut self) -> Option<Size> {
        self.natural.take()
    }

    /// The limits the window gets: the content's, narrowed by `attrs`'
    /// own.
    pub(crate) fn effective(&self, attrs: &WindowAttributes) -> SizeLimits {
        combine(attrs.min_size, attrs.max_size, self.layout)
    }

    /// The window's limits are `sent` already (it opened with them).
    pub(crate) fn opened_with(&mut self, sent: SizeLimits) {
        self.sent = Some(sent);
    }

    /// Whether a build measured the content's limits.
    pub(crate) fn measured(&self) -> bool {
        self.layout.is_some()
    }

    /// Forget what the window was told, so the next sync tells it again
    /// (the caller's own limits changed).
    pub(crate) fn resend(&mut self) {
        self.sent = None;
    }

    /// Give `window` its limits when they changed since it was last told,
    /// and resize it into them if it's outside: it is `size` now, and not
    /// resized while `pinned` (maximized or fullscreen). Returns the size
    /// asked for, if any.
    pub(crate) fn sync(
        &mut self,
        attrs: &WindowAttributes,
        window: &dyn Window,
        size: Size,
        pinned: bool,
    ) -> Option<Size> {
        // A window that can't be resized keeps the size it opened at; one
        // that doesn't follow its content got its own limits when opened.
        if !RESIZABLE_WINDOWS || !attrs.resizable || !attrs.layout_limits {
            return None;
        }
        // Nothing measured and nothing told yet: the window has the
        // caller's limits it opened with.
        if self.layout.is_none() && self.sent.is_none() {
            return None;
        }
        // Without the content's (it follows the window now), the caller's
        // own.
        let want = self.effective(attrs);
        if self.sent != Some(want) {
            self.sent = Some(want);
            window.set_min_size(want.0);
            window.set_max_size(want.1);
        }
        // The window is resized only into the content's.
        self.layout?;
        // Outside them (the limits changed, or no window manager holds the
        // window to them): asked into them, once at each size, so a window
        // manager that keeps it there (a tiling one) isn't asked again and
        // again.
        let fit = fit(size, want);
        let off =
            |a: Size, b: Size| (a.width - b.width).abs() > 0.5 || (a.height - b.height).abs() > 0.5;
        if !off(fit, size) || pinned || self.asked_at.is_some_and(|at| !off(at, size)) {
            return None;
        }
        self.asked_at = Some(size);
        window.set_inner_size(fit);
        Some(fit)
    }
}

/// The window's limits from the caller's `min` and `max` and the content's
/// `layout` ones: the larger minimum and the smaller maximum on each side,
/// the maximum never under the minimum.
pub(crate) fn combine(
    min: Option<Size>,
    max: Option<Size>,
    layout: Option<(Size, Size)>,
) -> SizeLimits {
    let Some((lo, hi)) = layout else {
        return (min, max);
    };
    let lo = match min {
        Some(m) => Size::new(m.width.max(lo.width), m.height.max(lo.height)),
        None => lo,
    };
    let hi = match max {
        Some(m) => Size::new(m.width.min(hi.width), m.height.min(hi.height)),
        None => hi,
    };
    let hi = Size::new(hi.width.max(lo.width), hi.height.max(lo.height));
    let min = (lo.width > 0.0 || lo.height > 0.0).then_some(lo);
    let max = (hi.width.is_finite() || hi.height.is_finite()).then_some(hi);
    (min, max)
}

/// `size` brought within `limits`.
pub(crate) fn fit(size: Size, (min, max): SizeLimits) -> Size {
    let mut size = size;
    if let Some(max) = max {
        size = Size::new(size.width.min(max.width), size.height.min(max.height));
    }
    if let Some(min) = min {
        size = Size::new(size.width.max(min.width), size.height.max(min.height));
    }
    size
}

#[cfg(test)]
mod tests {
    use super::*;

    const INF: f64 = f64::INFINITY;

    #[test]
    fn the_callers_minimum_is_a_floor_and_its_maximum_a_ceiling() {
        let layout = Some((Size::new(300.0, 100.0), Size::new(INF, 400.0)));
        // Larger per side.
        let (min, max) = combine(Some(Size::new(200.0, 150.0)), None, layout);
        assert_eq!(min, Some(Size::new(300.0, 150.0)));
        assert_eq!(max, Some(Size::new(INF, 400.0)));
        // Smaller per side, never under the minimum.
        let (min, max) = combine(None, Some(Size::new(250.0, 500.0)), layout);
        assert_eq!(min, Some(Size::new(300.0, 100.0)));
        assert_eq!(max, Some(Size::new(300.0, 400.0)));
        // Without the content's, the caller's alone.
        let own = (Some(Size::new(1.0, 2.0)), None);
        assert_eq!(combine(own.0, own.1, None), own);
        // Nothing to limit.
        assert_eq!(
            combine(None, None, Some((Size::ZERO, Size::new(INF, INF)))),
            (None, None)
        );
    }

    #[test]
    fn a_size_is_brought_within_the_limits() {
        let limits = (Some(Size::new(300.0, 100.0)), Some(Size::new(INF, 400.0)));
        assert_eq!(
            fit(Size::new(200.0, 500.0), limits),
            Size::new(300.0, 400.0)
        );
        assert_eq!(
            fit(Size::new(800.0, 200.0), limits),
            Size::new(800.0, 200.0)
        );
    }
}
