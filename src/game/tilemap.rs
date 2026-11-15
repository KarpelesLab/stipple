//! A grid of tiles drawn from a tileset.

use crate::game::SpriteSheet;
use crate::geometry::{Point, Rect, Size};
use crate::render::{ImageOptions, Scene};
use std::ops::Range;

/// No tile in a cell.
const EMPTY: u32 = u32::MAX;

/// A grid of cells, each empty or holding a tile: a frame number of the
/// tileset [`SpriteSheet`] it's drawn with. The grid sits in world space with
/// its top-left corner at [`origin`](Self::origin), each cell
/// [`tile_size`](Self::tile_size) big.
///
/// [`draw`](Self::draw) draws only the cells that cross the visible rect
/// (a [`Camera2D`](crate::game::Camera2D)'s
/// [`visible_rect`](crate::game::Camera2D::visible_rect)), so a level of any
/// size costs only what's on screen.
///
/// ```
/// use stipple::game::Tilemap;
/// use stipple::geometry::{Point, Rect, Size};
///
/// let mut map = Tilemap::new(100, 50, Size::new(16.0, 16.0));
/// map.set(3, 2, Some(7));
/// assert_eq!(map.get(3, 2), Some(7));
/// assert_eq!(map.cell_at(Point::new(50.0, 40.0)), Some((3, 2)));
/// // A 320×180 view at the top-left sees 20×12 cells (the last row in part).
/// let (cols, rows) = map.visible_cells(Rect::from_xywh(0.0, 0.0, 320.0, 180.0));
/// assert_eq!((cols, rows), (0..20, 0..12));
/// ```
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct Tilemap {
    cols: u32,
    rows: u32,
    tile_size: Size,
    tiles: Vec<u32>,
    /// Where the grid's top-left corner is in world space.
    pub origin: Point,
}

impl Tilemap {
    /// An empty `cols`×`rows` grid of `tile_size` cells at the origin.
    pub fn new(cols: u32, rows: u32, tile_size: Size) -> Self {
        Self {
            cols,
            rows,
            tile_size,
            tiles: vec![EMPTY; cols as usize * rows as usize],
            origin: Point::ORIGIN,
        }
    }

    /// A grid from its cells, row by row (`None` for an empty cell); cells
    /// past the end of `tiles` are empty.
    pub fn from_tiles(
        cols: u32,
        rows: u32,
        tile_size: Size,
        tiles: impl IntoIterator<Item = Option<usize>>,
    ) -> Self {
        let mut map = Self::new(cols, rows, tile_size);
        for (cell, tile) in map.tiles.iter_mut().zip(tiles) {
            *cell = tile.map_or(EMPTY, |t| t.min(EMPTY as usize - 1) as u32);
        }
        map
    }

    /// With its top-left corner at `origin` in world space.
    pub fn at(mut self, origin: Point) -> Self {
        self.origin = origin;
        self
    }

    /// The number of columns.
    pub fn cols(&self) -> u32 {
        self.cols
    }

    /// The number of rows.
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// A cell's size in world units.
    pub fn tile_size(&self) -> Size {
        self.tile_size
    }

    /// The tile in cell `(col, row)`, if any.
    pub fn get(&self, col: u32, row: u32) -> Option<usize> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        let t = self.tiles[(row * self.cols + col) as usize];
        (t != EMPTY).then_some(t as usize)
    }

    /// Put `tile` in cell `(col, row)` (`None` empties it). Out of range: no
    /// effect.
    pub fn set(&mut self, col: u32, row: u32, tile: Option<usize>) {
        if col < self.cols && row < self.rows {
            self.tiles[(row * self.cols + col) as usize] =
                tile.map_or(EMPTY, |t| t.min(EMPTY as usize - 1) as u32);
        }
    }

    /// Put `tile` in every cell.
    pub fn fill(&mut self, tile: Option<usize>) {
        let t = tile.map_or(EMPTY, |t| t.min(EMPTY as usize - 1) as u32);
        self.tiles.fill(t);
    }

    /// The whole grid in world space.
    pub fn bounds(&self) -> Rect {
        Rect::from_xywh(
            self.origin.x,
            self.origin.y,
            self.cols as f64 * self.tile_size.width,
            self.rows as f64 * self.tile_size.height,
        )
    }

    /// Cell `(col, row)` in world space.
    pub fn cell_rect(&self, col: u32, row: u32) -> Rect {
        Rect::from_xywh(
            self.origin.x + col as f64 * self.tile_size.width,
            self.origin.y + row as f64 * self.tile_size.height,
            self.tile_size.width,
            self.tile_size.height,
        )
    }

    /// The cell under the world point `p`, if it's on the grid.
    pub fn cell_at(&self, p: Point) -> Option<(u32, u32)> {
        let (tw, th) = (self.tile_size.width, self.tile_size.height);
        if tw <= 0.0 || th <= 0.0 {
            return None;
        }
        let c = ((p.x - self.origin.x) / tw).floor();
        let r = ((p.y - self.origin.y) / th).floor();
        (c >= 0.0 && r >= 0.0 && c < self.cols as f64 && r < self.rows as f64)
            .then_some((c as u32, r as u32))
    }

    /// The columns and rows of the cells that cross `view` (world space).
    pub fn visible_cells(&self, view: Rect) -> (Range<u32>, Range<u32>) {
        let (tw, th) = (self.tile_size.width, self.tile_size.height);
        if tw <= 0.0 || th <= 0.0 || view.width() <= 0.0 || view.height() <= 0.0 {
            return (0..0, 0..0);
        }
        let span = |lo: f64, hi: f64, origin: f64, size: f64, n: u32| {
            let a = ((lo - origin) / size).floor().clamp(0.0, n as f64) as u32;
            let b = ((hi - origin) / size).ceil().clamp(0.0, n as f64) as u32;
            a..b.max(a)
        };
        (
            span(view.min_x(), view.max_x(), self.origin.x, tw, self.cols),
            span(view.min_y(), view.max_y(), self.origin.y, th, self.rows),
        )
    }

    /// Draw the cells that cross `view` (world space) with `tileset`'s
    /// frames and `options` (pass `ImageOptions::new().nearest()` for pixel
    /// art), returning how many tiles were drawn. Draw it inside the
    /// camera's transform (see [`Camera2D::push`](crate::game::Camera2D::push)).
    pub fn draw(
        &self,
        scene: &mut Scene,
        tileset: &SpriteSheet,
        view: Rect,
        options: &ImageOptions,
    ) -> usize {
        let (cols, rows) = self.visible_cells(view);
        let mut drawn = 0;
        for row in rows {
            let line = &self.tiles[(row * self.cols) as usize..][..self.cols as usize];
            for col in cols.clone() {
                let t = line[col as usize];
                if t == EMPTY {
                    continue;
                }
                if let Some(src) = tileset.frame(t as usize) {
                    let dst = self.cell_rect(col, row);
                    scene.draw_image_region(src, dst, tileset.image(), options);
                    drawn += 1;
                }
            }
        }
        drawn
    }
}
