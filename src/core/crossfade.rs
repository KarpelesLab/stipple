//! A whole-window cross-fade from the frame that was on screen to the new one,
//! for theme changes (the OS switching to dark mode, or the app changing its
//! theme).
//!
//! The app keeps the last frame it presented. When the theme changes, that
//! frame becomes the fade's starting picture, and for [`THEME_FADE`] each
//! presented frame is the new frame blended over it, more of the new one each
//! time. Frames are asked for only while the fade runs; under reduced motion
//! (see [`anim::motion_duration`](crate::anim::motion_duration)) there is no
//! fade and the new theme shows at once.
//!
//! It works on the finished pixels, so it covers the software renderer and a
//! GPU renderer set with [`App::render_with`](crate::App::render_with) alike.

use std::time::Duration;

use crate::anim::Easing;
use crate::render::Pixmap;

/// How long a theme change takes to cross-fade.
pub const THEME_FADE: Duration = Duration::from_millis(200);

/// One window's cross-fade.
#[derive(Debug, Default)]
pub(crate) struct CrossFade {
    /// The picture faded from, while fading.
    from: Option<Pixmap>,
    start: Duration,
    length: Duration,
    /// How much of the new frame shows, as of the last [`advance`](Self::advance).
    mix: f64,
    /// The frame on screen is a blend, so the next one must replace it whole.
    shown: bool,
}

impl CrossFade {
    /// Fade from `from` (the frame on screen) over `length`, starting at
    /// `now`. With no clock or no length (reduced motion), there's no fade.
    /// A fade already running starts again from the blend now on screen.
    pub(crate) fn start(&mut self, from: Pixmap, now: Option<Duration>, length: Duration) {
        let Some(now) = now.filter(|_| !length.is_zero()) else {
            self.from = None;
            return;
        };
        let from = match self.from.take() {
            Some(old) if old.size() == from.size() => blend(&old, &from, self.mix),
            _ => from,
        };
        *self = CrossFade {
            from: Some(from),
            start: now,
            length,
            mix: 0.0,
            shown: self.shown,
        };
        self.advance(Some(now));
    }

    /// Move the fade to `now`, the time of the frame being built. It counts
    /// from one frame before the change, so the first frame already shows
    /// some of the new one; once it's through, the old picture is let go.
    pub(crate) fn advance(&mut self, now: Option<Duration>) {
        if self.from.is_none() {
            return;
        }
        let Some(now) = now else {
            self.from = None;
            return;
        };
        let t = now
            .saturating_sub(self.start)
            .saturating_add(crate::platform::frame::interval());
        let p = t.as_secs_f64() / self.length.as_secs_f64();
        if p >= 1.0 {
            self.from = None;
            self.mix = 1.0;
        } else {
            self.mix = Easing::EaseOut.apply(p);
        }
    }

    /// Whether the fade still runs (and wants frames).
    pub(crate) fn active(&self) -> bool {
        self.from.is_some()
    }

    /// Whether the next present must be a whole frame: the fade runs, or its
    /// last blend is still on screen.
    pub(crate) fn needs_full(&self) -> bool {
        self.from.is_some() || self.shown
    }

    /// What to show for `frame`: the blend while fading, or `None` to show the
    /// frame itself. A frame of another size (the window was resized) ends the
    /// fade.
    pub(crate) fn compose(&mut self, frame: &Pixmap) -> Option<Pixmap> {
        let blended = match &self.from {
            Some(from) if from.size() == frame.size() => Some(blend(from, frame, self.mix)),
            _ => None,
        };
        if blended.is_none() {
            self.from = None;
        }
        self.shown = blended.is_some();
        blended
    }
}

/// `from` and `to` mixed: `mix` 0 is `from`, 1 is `to`. Same sizes.
pub(crate) fn blend(from: &Pixmap, to: &Pixmap, mix: f64) -> Pixmap {
    let w = (mix.clamp(0.0, 1.0) * 256.0).round() as u32;
    let mut out = to.clone();
    for (o, &a) in out.as_bytes_mut().iter_mut().zip(from.as_bytes()) {
        let b = u32::from(*o);
        *o = ((u32::from(a) * (256 - w) + b * w + 128) >> 8) as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PhysicalSize;

    const MS: Duration = Duration::from_millis(1);

    fn solid(rgba: [u8; 4]) -> Pixmap {
        let size = PhysicalSize::new(4, 2);
        Pixmap::from_rgba8(size, rgba.repeat(8))
    }

    #[test]
    fn mid_fade_frames_blend_the_old_and_new_pixels() {
        let (white, black) = (solid([255, 255, 255, 255]), solid([0, 0, 0, 255]));
        let mut fade = CrossFade::default();
        let t0 = Duration::from_secs(2);
        fade.start(white, Some(t0), THEME_FADE);
        assert!(fade.active() && fade.needs_full());

        // Halfway (counting the one-frame lead), EaseOut has 3/4 of the new.
        let lead = crate::platform::frame::interval();
        fade.advance(Some(t0 + THEME_FADE / 2 - lead));
        let mid = fade.compose(&black).expect("fading");
        let expected = 64; // a quarter of white's 255 left
        assert_eq!(mid.pixel(1, 1), Some([expected, expected, expected, 255]));

        // Earlier frames show less of the new one.
        fade.advance(Some(t0 + 20 * MS));
        let early = fade.compose(&black).unwrap().pixel(0, 0).unwrap()[0];
        assert!(early > expected && early < 255, "{early}");

        // Done: the old frame is dropped, and one more whole frame replaces
        // the last blend.
        fade.advance(Some(t0 + THEME_FADE));
        assert!(!fade.active());
        assert!(fade.needs_full(), "a blend is still on screen");
        assert_eq!(fade.compose(&black), None);
        assert!(!fade.needs_full());
    }

    #[test]
    fn no_length_or_no_clock_snaps() {
        let mut fade = CrossFade::default();
        fade.start(solid([255; 4]), Some(Duration::ZERO), Duration::ZERO);
        assert!(!fade.active());
        fade.start(solid([255; 4]), None, THEME_FADE);
        assert!(!fade.active());
        assert_eq!(fade.compose(&solid([0; 4])), None);
    }

    #[test]
    fn a_resize_ends_the_fade() {
        let mut fade = CrossFade::default();
        fade.start(solid([255; 4]), Some(Duration::ZERO), THEME_FADE);
        let bigger = Pixmap::new(PhysicalSize::new(8, 8));
        assert_eq!(fade.compose(&bigger), None);
        assert!(!fade.active());
    }

    #[test]
    fn a_second_change_fades_on_from_the_blend_on_screen() {
        let mut fade = CrossFade::default();
        let t0 = Duration::from_secs(1);
        fade.start(solid([255, 255, 255, 255]), Some(t0), THEME_FADE);
        let lead = crate::platform::frame::interval();
        fade.advance(Some(t0 + THEME_FADE / 2 - lead));
        let shown = fade.compose(&solid([0, 0, 0, 255])).unwrap();
        // The new theme changes again: the fade restarts from what's shown.
        let t1 = t0 + THEME_FADE / 2 - lead;
        fade.start(solid([0, 0, 0, 255]), Some(t1), THEME_FADE);
        assert_eq!(fade.from.as_ref(), Some(&shown));
    }
}
