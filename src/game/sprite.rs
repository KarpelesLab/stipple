//! Sprite sheets (an image cut into frames) and frame animations.

use crate::geometry::{Point, Rect};
use crate::render::{ImageOptions, Pixmap, Scene};
use std::collections::HashMap;
use std::sync::Arc;

/// How a sprite sheet's image is cut into a grid of equal cells (see
/// [`SpriteSheet::from_grid`]): the cell size, the margin of pixels around
/// the grid and the spacing between cells, all in the image's pixels.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SpriteGrid {
    /// A cell's width.
    pub frame_width: u32,
    /// A cell's height.
    pub frame_height: u32,
    /// Pixels around the grid, on every side.
    pub margin: u32,
    /// Pixels between two cells.
    pub spacing: u32,
}

impl SpriteGrid {
    /// Cells of `frame_width`×`frame_height`, packed edge to edge.
    pub fn new(frame_width: u32, frame_height: u32) -> Self {
        Self {
            frame_width,
            frame_height,
            margin: 0,
            spacing: 0,
        }
    }

    /// With `margin` pixels around the grid.
    pub fn margin(mut self, margin: u32) -> Self {
        self.margin = margin;
        self
    }

    /// With `spacing` pixels between cells.
    pub fn spacing(mut self, spacing: u32) -> Self {
        self.spacing = spacing;
        self
    }
}

/// An image cut into frames: a grid of equal cells, or named regions of any
/// size (an atlas). Frames are numbered in the order they were added (a grid
/// row by row, left to right).
///
/// ```
/// use std::sync::Arc;
/// use stipple::game::SpriteSheet;
/// use stipple::geometry::{PhysicalSize, Rect};
/// use stipple::render::Pixmap;
///
/// let image = Arc::new(Pixmap::new(PhysicalSize::new(64, 32)));
/// let mut sheet = SpriteSheet::grid(image, 16, 16);
/// assert_eq!(sheet.len(), 8);
/// assert_eq!(sheet.frame(5), Some(Rect::from_xywh(16.0, 16.0, 16.0, 16.0)));
/// let coin = sheet.add("coin", Rect::from_xywh(0.0, 0.0, 8.0, 8.0));
/// assert_eq!(sheet.index("coin"), Some(coin));
/// ```
#[derive(Clone, Debug)]
pub struct SpriteSheet {
    image: Arc<Pixmap>,
    frames: Vec<Rect>,
    names: HashMap<String, usize>,
}

impl SpriteSheet {
    /// A sheet of `image` with no frames yet (add them with
    /// [`push`](Self::push) or [`add`](Self::add)).
    pub fn new(image: Arc<Pixmap>) -> Self {
        Self {
            image,
            frames: Vec::new(),
            names: HashMap::new(),
        }
    }

    /// `image` cut into a grid of `frame_width`×`frame_height` cells, row by
    /// row (a partial cell at the right or bottom edge is left out).
    pub fn grid(image: Arc<Pixmap>, frame_width: u32, frame_height: u32) -> Self {
        Self::from_grid(image, SpriteGrid::new(frame_width, frame_height))
    }

    /// `image` cut as `grid` says: like [`grid`](Self::grid), for a sheet
    /// with a margin around the cells or spacing between them.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use stipple::game::{SpriteGrid, SpriteSheet};
    /// use stipple::geometry::{PhysicalSize, Rect};
    /// use stipple::render::Pixmap;
    ///
    /// let image = Arc::new(Pixmap::new(PhysicalSize::new(37, 20)));
    /// // 10 px cells, 1 px around them and 2 px between: x at 1, 13 and 25.
    /// let sheet = SpriteSheet::from_grid(image, SpriteGrid::new(10, 10).margin(1).spacing(2));
    /// assert_eq!(sheet.frame(2), Some(Rect::from_xywh(25.0, 1.0, 10.0, 10.0)));
    /// ```
    pub fn from_grid(image: Arc<Pixmap>, grid: SpriteGrid) -> Self {
        let SpriteGrid {
            frame_width,
            frame_height,
            margin,
            spacing,
        } = grid;
        let mut sheet = Self::new(image);
        let size = sheet.image.size();
        if frame_width == 0 || frame_height == 0 {
            return sheet;
        }
        let mut y = margin;
        while y + frame_height <= size.height.saturating_sub(margin) {
            let mut x = margin;
            while x + frame_width <= size.width.saturating_sub(margin) {
                sheet.frames.push(Rect::from_xywh(
                    x as f64,
                    y as f64,
                    frame_width as f64,
                    frame_height as f64,
                ));
                x += frame_width + spacing;
            }
            y += frame_height + spacing;
        }
        sheet
    }

    /// The sheet's image.
    pub fn image(&self) -> &Arc<Pixmap> {
        &self.image
    }

    /// Add a frame (a rect in the image's pixels), returning its number.
    pub fn push(&mut self, rect: Rect) -> usize {
        self.frames.push(rect);
        self.frames.len() - 1
    }

    /// Add a frame under `name`, returning its number.
    pub fn add(&mut self, name: impl Into<String>, rect: Rect) -> usize {
        let i = self.push(rect);
        self.names.insert(name.into(), i);
        i
    }

    /// The number of the frame named `name`.
    pub fn index(&self, name: &str) -> Option<usize> {
        self.names.get(name).copied()
    }

    /// Frame `index`'s rect in the image's pixels.
    pub fn frame(&self, index: usize) -> Option<Rect> {
        self.frames.get(index).copied()
    }

    /// How many frames the sheet has.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the sheet has no frames.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Draw frame `index` into `dst` with `options` (nothing if there's no
    /// such frame).
    pub fn draw(&self, scene: &mut Scene, index: usize, dst: Rect, options: &ImageOptions) {
        if let Some(src) = self.frame(index) {
            scene.draw_image_region(src, dst, &self.image, options);
        }
    }

    /// Draw frame `index` at its own size with its top-left at `at`.
    pub fn draw_at(&self, scene: &mut Scene, index: usize, at: Point, options: &ImageOptions) {
        if let Some(src) = self.frame(index) {
            let dst = Rect::from_xywh(at.x, at.y, src.width(), src.height());
            scene.draw_image_region(src, dst, &self.image, options);
        }
    }
}

/// What a [`SpriteAnimation`] does after its last frame.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Playback {
    /// Starts over.
    #[default]
    Loop,
    /// Stays on the last frame.
    Once,
    /// Plays backwards to the first frame, then forwards again.
    PingPong,
}

/// A sequence of sprite-sheet frames, each shown for its own duration,
/// sampled at a time: a walk cycle, an explosion, a blinking coin.
///
/// ```
/// use stipple::game::{Playback, SpriteAnimation};
///
/// // Frames 4, 5, 6 and 7 of a sheet at 10 frames a second.
/// let walk = SpriteAnimation::new(4..8, 0.1);
/// assert_eq!(walk.frame_at(0.0), 4);
/// assert_eq!(walk.frame_at(0.25), 6);
/// assert_eq!(walk.frame_at(0.45), 4); // looped
///
/// let boom = SpriteAnimation::new([0, 1, 2], 0.05).playback(Playback::Once);
/// assert_eq!(boom.frame_at(10.0), 2);
/// assert!(boom.finished(10.0));
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteAnimation {
    /// Each step's frame number and duration in seconds.
    steps: Vec<(usize, f64)>,
    playback: Playback,
    total: f64,
}

impl SpriteAnimation {
    /// `frames`, each shown for `frame_duration` seconds, looping.
    pub fn new(frames: impl IntoIterator<Item = usize>, frame_duration: f64) -> Self {
        Self::with_durations(frames.into_iter().map(|f| (f, frame_duration)))
    }

    /// Frames each with its own duration in seconds, looping.
    pub fn with_durations(steps: impl IntoIterator<Item = (usize, f64)>) -> Self {
        let steps: Vec<(usize, f64)> = steps
            .into_iter()
            .map(|(f, d)| (f, if d.is_finite() { d.max(0.0) } else { 0.0 }))
            .collect();
        let total = steps.iter().map(|s| s.1).sum();
        Self {
            steps,
            playback: Playback::Loop,
            total,
        }
    }

    /// Set what happens after the last frame.
    pub fn playback(mut self, playback: Playback) -> Self {
        self.playback = playback;
        self
    }

    /// One pass through the frames, in seconds.
    pub fn duration(&self) -> f64 {
        self.total
    }

    /// The number of frames in a pass.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether there are no frames.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Whether a [`Once`](Playback::Once) animation is over at `time`
    /// (never, for the others).
    pub fn finished(&self, time: f64) -> bool {
        self.playback == Playback::Once && time >= self.total
    }

    /// Which step (`0..len`) shows at `time` seconds from the start.
    pub fn step_at(&self, time: f64) -> usize {
        let n = self.steps.len();
        if n <= 1 || self.total <= 0.0 || !time.is_finite() || time <= 0.0 {
            return 0;
        }
        let find = |t: f64| {
            let mut acc = 0.0;
            for (i, s) in self.steps.iter().enumerate() {
                acc += s.1;
                if t < acc {
                    return i;
                }
            }
            n - 1
        };
        match self.playback {
            Playback::Once => {
                if time >= self.total {
                    n - 1
                } else {
                    find(time)
                }
            }
            Playback::Loop => find(time % self.total),
            Playback::PingPong => {
                // Forwards through every step, then back through the inner
                // ones (the ends aren't shown twice).
                let back: f64 = self.steps[1..n - 1].iter().map(|s| s.1).sum();
                let cycle = self.total + back;
                let t = time % cycle;
                if t < self.total {
                    find(t)
                } else {
                    let mut acc = self.total;
                    for i in (1..n - 1).rev() {
                        acc += self.steps[i].1;
                        if t < acc {
                            return i;
                        }
                    }
                    1
                }
            }
        }
    }

    /// The sheet frame number that shows at `time` seconds from the start.
    pub fn frame_at(&self, time: f64) -> usize {
        self.steps.get(self.step_at(time)).map_or(0, |s| s.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PhysicalSize;

    #[test]
    fn a_spaced_grid_skips_margins_and_gaps() {
        let image = Arc::new(Pixmap::new(PhysicalSize::new(37, 20)));
        // 1 px margin, 2 px spacing, 10 px cells: x at 1, 13, 25; one row.
        let sheet = SpriteSheet::from_grid(image, SpriteGrid::new(10, 10).margin(1).spacing(2));
        assert_eq!(sheet.len(), 3);
        assert_eq!(sheet.frame(2), Some(Rect::from_xywh(25.0, 1.0, 10.0, 10.0)));
    }

    #[test]
    fn frames_with_their_own_durations() {
        let a = SpriteAnimation::with_durations([(3, 0.5), (7, 0.1), (9, 0.4)]);
        assert_eq!(a.duration(), 1.0);
        assert_eq!(a.frame_at(0.49), 3);
        assert_eq!(a.frame_at(0.55), 7);
        assert_eq!(a.frame_at(0.7), 9);
        assert_eq!(a.frame_at(1.2), 3);
        assert!(!a.finished(5.0));
    }

    #[test]
    fn ping_pong_turns_at_the_ends() {
        let a = SpriteAnimation::new(0..4, 1.0).playback(Playback::PingPong);
        let steps: Vec<usize> = (0..8).map(|t| a.frame_at(t as f64 + 0.5)).collect();
        assert_eq!(steps, vec![0, 1, 2, 3, 2, 1, 0, 1]);
    }
}
