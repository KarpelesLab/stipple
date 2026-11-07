/// Edge insets (padding/margin) in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl Insets {
    pub const ZERO: Self = Self {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: 0.0,
    };

    #[inline]
    pub const fn new(top: f64, right: f64, bottom: f64, left: f64) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// The same inset on all four edges.
    #[inline]
    pub const fn uniform(v: f64) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    /// Independent horizontal (left/right) and vertical (top/bottom) insets.
    #[inline]
    pub const fn symmetric(horizontal: f64, vertical: f64) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    /// Total inset along the horizontal axis (`left + right`).
    #[inline]
    pub fn horizontal(self) -> f64 {
        self.left + self.right
    }

    /// Total inset along the vertical axis (`top + bottom`).
    #[inline]
    pub fn vertical(self) -> f64 {
        self.top + self.bottom
    }
}
