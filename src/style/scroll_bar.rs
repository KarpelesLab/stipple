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
    /// Arrow buttons at the bar's ends, each a square as wide as the bar:
    /// a press steps [`arrow_step`](Self::arrow_step) back or on, and keeps
    /// stepping while held. On by default only where the platform's own bars
    /// have them (Windows); an overlay bar never shows them.
    pub arrows: bool,
    /// How far an arrow press scrolls (logical pixels).
    pub arrow_step: f64,
    /// How long a press on the track or an arrow is held before it repeats
    /// (seconds): the platform's delay.
    pub repeat_delay: f64,
    /// How often a held press repeats after that (seconds), paging until the
    /// thumb reaches the pointer (stepping until the end, for an arrow).
    pub repeat_interval: f64,
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
        arrows: false,
        arrow_step: 40.0,
        repeat_delay: 0.25,
        repeat_interval: 0.05,
    };

    /// Windows' bar: [`CLASSIC`](Self::CLASSIC) with arrow buttons, and
    /// Win32's repeat timing (0.2 s, then every 0.05 s).
    pub const WINDOWS: ScrollBarLook = ScrollBarLook {
        arrows: true,
        repeat_delay: 0.2,
        ..Self::CLASSIC
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
        arrows: false,
        arrow_step: 40.0,
        repeat_delay: 0.5,
        repeat_interval: 0.05,
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
        arrows: false,
        arrow_step: 40.0,
        repeat_delay: 0.5,
        repeat_interval: 0.05,
    };

    super::setters!(
        overlay: bool,
        width: f64,
        thumb: f64,
        thumb_hover: f64,
        min_thumb: f64,
        interactive: bool,
        strong: bool,
        arrows: bool,
        arrow_step: f64,
        repeat_delay: f64,
        repeat_interval: f64,
    );

    /// The platform's bar, as the built-in themes use it: [`OVERLAY`] on
    /// macOS, [`TOUCH`] on iOS and Android, [`WINDOWS`] (arrow buttons) on
    /// Windows, [`CLASSIC`] elsewhere (Linux, the web).
    ///
    /// [`OVERLAY`]: Self::OVERLAY
    /// [`TOUCH`]: Self::TOUCH
    /// [`WINDOWS`]: Self::WINDOWS
    /// [`CLASSIC`]: Self::CLASSIC
    pub const fn platform() -> Self {
        #[cfg(target_os = "macos")]
        let look = Self::OVERLAY;
        #[cfg(any(target_os = "ios", target_os = "android"))]
        let look = Self::TOUCH;
        #[cfg(target_os = "windows")]
        let look = Self::WINDOWS;
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "android",
            target_os = "windows"
        )))]
        let look = Self::CLASSIC;
        look
    }

    /// The legacy (always shown) bar of macOS, as `Theme::native` takes it
    /// when *Show scroll bars* is *Always*: [`CLASSIC`](Self::CLASSIC) with
    /// AppKit's repeat timing (0.5 s, then every 0.05 s).
    pub const MAC_LEGACY: ScrollBarLook = ScrollBarLook {
        repeat_delay: 0.5,
        ..Self::CLASSIC
    };

    /// Whether arrow buttons show at the bar's ends: [`arrows`](Self::arrows)
    /// on a bar that the pointer uses and that isn't an overlay.
    pub fn shows_arrows(&self) -> bool {
        self.arrows && !self.overlay && self.interactive
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

    /// An arrow button's colors in `palette`: the arrow's, and the button's
    /// fill (none at rest), the pointer over it (`hovered`), held down
    /// (`pressed`), or at the end it scrolls towards (`spent`: faded).
    pub(crate) fn arrow_colors(
        &self,
        palette: &Palette,
        hovered: bool,
        pressed: bool,
        spent: bool,
    ) -> (Color, Option<Color>) {
        let ink = palette.text;
        let alpha = |a: u8| Color::rgba(ink.r, ink.g, ink.b, a);
        let glyph = if spent {
            alpha(0x40)
        } else if self.strong || pressed {
            ink
        } else if hovered {
            alpha(0xc0)
        } else {
            alpha(0x90)
        };
        let fill = if pressed {
            Some(alpha(0x30))
        } else if hovered {
            Some(alpha(0x18))
        } else {
            None
        };
        (glyph, fill)
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
