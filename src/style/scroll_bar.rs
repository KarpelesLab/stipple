//! How scroll containers show their scroll bars. See [`ScrollBarLook`].

use super::Palette;
use crate::render::Color;

/// How the scroll bars of scroll containers ([`scroll`](crate::widgets::scroll),
/// [`scroll_horizontal`](crate::widgets::scroll_horizontal), the virtualized
/// [`virtual_list`](crate::widgets::virtual_list),
/// [`table`](crate::widgets::table) and [`tree`](crate::widgets::tree)) look
/// and behave: part of a [`Theme`](crate::style::Theme)
/// ([`Theme::scroll_bar`](crate::style::Theme::scroll_bar)).
///
/// A bar shows only while the content overflows its container: a track
/// along the container's edge (the right one, or the left in a right-to-left
/// layout; the bottom one for a sideways container) and a thumb in it, as
/// long as the share of the content in view and placed by the scroll offset.
/// Its colors come from the theme's [`Palette`] (its text color, faded), so
/// every theme's bars match it.
///
/// Two kinds, after the platforms':
///
/// - **Classic** ([`ScrollBarLook::CLASSIC`]): always shown, in a gutter the
///   container keeps for it beside the content (so the bar never covers a
///   row), with a track. The desktop convention: Windows, Linux and the web.
/// - **Overlay** ([`ScrollBarLook::OVERLAY`], [`ScrollBarLook::TOUCH`]):
///   drawn over the content's edge with no gutter, shown while the
///   container scrolls (and while the pointer is over the bar) and fading
///   out a moment after, with [`MotionTokens::scroll_bar`]. macOS's
///   default, and iOS's and Android's, where it is a thin indicator a
///   finger can't drag.
///
/// The presets pick the platform's kind ([`ScrollBarLook::platform`]);
/// [`Theme::native`](crate::style::Theme::native) on macOS follows *Show
/// scroll bars* in System Settings. Set another with
/// [`Theme::scroll_bar`](crate::style::Theme::scroll_bar):
///
/// ```
/// use stipple::style::{ScrollBarLook, Theme};
/// // Persistent bars everywhere, 16 px wide.
/// let theme = Theme::light().scroll_bar(ScrollBarLook::CLASSIC.width(16.0));
/// assert!(!theme.scroll_bar.overlay);
/// ```
///
/// [`MotionTokens::scroll_bar`]: crate::style::MotionTokens::scroll_bar
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct ScrollBarLook {
    /// Drawn over the content and shown only around scrolling (fading out
    /// after), instead of always shown in a gutter of its own.
    pub overlay: bool,
    /// How wide the bar's strip is across the container (logical pixels):
    /// the gutter a classic bar keeps, and the part of an overlay bar the
    /// pointer can grab.
    pub width: f64,
    /// How thick the thumb is at rest.
    pub thumb: f64,
    /// How thick the thumb grows under the pointer and while dragged.
    pub thumb_hover: f64,
    /// The shortest the thumb gets, however long the content.
    pub min_thumb: f64,
    /// Whether the pointer can drag the thumb and page by pressing the track
    /// (not on touch platforms, whose bars are only indicators).
    pub interactive: bool,
    /// Solid colors: the thumb in the text color, for high contrast (set by
    /// [`Theme::high_contrast`](crate::style::Theme::high_contrast)).
    pub strong: bool,
}

impl ScrollBarLook {
    /// A desktop bar: always shown in a 12 px gutter, a track and a 6 px
    /// thumb that grows to 8 px under the pointer.
    pub const CLASSIC: ScrollBarLook = ScrollBarLook {
        overlay: false,
        width: 12.0,
        thumb: 6.0,
        thumb_hover: 8.0,
        min_thumb: 24.0,
        interactive: true,
        strong: false,
    };

    /// macOS's overlay bar: a 7 px thumb over the content's edge, shown
    /// while scrolling and growing to 11 px (with a track) under the pointer.
    pub const OVERLAY: ScrollBarLook = ScrollBarLook {
        overlay: true,
        width: 15.0,
        thumb: 7.0,
        thumb_hover: 11.0,
        min_thumb: 24.0,
        interactive: true,
        strong: false,
    };

    /// A touch screen's scroll indicator (iOS, Android): a 3 px line over
    /// the content's edge while it scrolls, which a finger doesn't drag.
    pub const TOUCH: ScrollBarLook = ScrollBarLook {
        overlay: true,
        width: 7.0,
        thumb: 3.0,
        thumb_hover: 3.0,
        min_thumb: 36.0,
        interactive: false,
        strong: false,
    };

    super::setters!(
        overlay: bool,
        width: f64,
        thumb: f64,
        thumb_hover: f64,
        min_thumb: f64,
        interactive: bool,
        strong: bool,
    );

    /// The platform's bar, as the built-in themes use it: [`OVERLAY`] on
    /// macOS, [`TOUCH`] on iOS and Android, [`CLASSIC`] elsewhere (Windows,
    /// Linux, the web).
    ///
    /// [`OVERLAY`]: Self::OVERLAY
    /// [`TOUCH`]: Self::TOUCH
    /// [`CLASSIC`]: Self::CLASSIC
    pub const fn platform() -> Self {
        #[cfg(target_os = "macos")]
        let look = Self::OVERLAY;
        #[cfg(any(target_os = "ios", target_os = "android"))]
        let look = Self::TOUCH;
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
        let look = Self::CLASSIC;
        look
    }

    /// The gutter a scroll container keeps beside its content for the bar
    /// when the content overflows: [`width`](Self::width) for a classic bar,
    /// none for an overlay one.
    pub fn gutter(&self) -> f64 {
        if self.overlay {
            0.0
        } else {
            self.width.max(0.0)
        }
    }

    /// The thumb's and the track's colors in `palette`, the thumb `hovered`
    /// or `pressed` (dragged), the pointer `over` the bar; the track is
    /// `None` where none is drawn (an overlay bar's, unless the pointer is
    /// over it).
    pub(crate) fn colors(
        &self,
        palette: &Palette,
        hovered: bool,
        pressed: bool,
        over: bool,
    ) -> (Color, Option<Color>) {
        let ink = palette.text;
        let alpha = |a: u8| Color::rgba(ink.r, ink.g, ink.b, a);
        let thumb = if self.strong {
            if pressed || hovered {
                palette.primary
            } else {
                ink
            }
        } else if pressed {
            alpha(0xa0)
        } else if hovered {
            alpha(0x80)
        } else {
            alpha(0x58)
        };
        let track = (!self.overlay || over || hovered || pressed).then(|| {
            if self.strong {
                alpha(0x30)
            } else {
                alpha(0x10)
            }
        });
        (thumb, track)
    }
}

impl Default for ScrollBarLook {
    fn default() -> Self {
        Self::platform()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_keeps_a_gutter_and_overlay_none() {
        assert_eq!(ScrollBarLook::CLASSIC.gutter(), 12.0);
        assert_eq!(ScrollBarLook::OVERLAY.gutter(), 0.0);
        assert_eq!(ScrollBarLook::TOUCH.gutter(), 0.0);
        const { assert!(!ScrollBarLook::TOUCH.interactive) };
    }

    #[test]
    fn colors_follow_the_palette_and_state() {
        let p = Palette::dark();
        let look = ScrollBarLook::CLASSIC;
        let (rest, track) = look.colors(&p, false, false, false);
        let (hover, _) = look.colors(&p, true, false, true);
        let (press, _) = look.colors(&p, false, true, true);
        assert_eq!((rest.r, rest.g, rest.b), (p.text.r, p.text.g, p.text.b));
        assert!(rest.a < hover.a && hover.a < press.a);
        assert!(track.is_some());
        // An overlay bar's track shows only under the pointer.
        assert!(
            ScrollBarLook::OVERLAY
                .colors(&p, false, false, false)
                .1
                .is_none()
        );
        assert!(
            ScrollBarLook::OVERLAY
                .colors(&p, false, false, true)
                .1
                .is_some()
        );
        // High contrast: a solid thumb.
        let strong = look.strong(true).colors(&p, false, false, false).0;
        assert_eq!(strong, p.text);
    }
}
