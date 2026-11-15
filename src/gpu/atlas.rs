//! The glyph atlas: glyph rasters packed into a few large textures, so text
//! is drawn from one texture (and batched into one draw call) instead of a
//! texture per glyph.
//!
//! A page is a square straight-alpha RGBA bitmap, 512 pixels on a side at
//! first, doubled (up to 2048) when it fills up; past that a second page is
//! opened, and once both are full the least recently used rasters (never one
//! drawn in the frame being lowered) are evicted to make room. Rasters are
//! packed on shelves: rows of one height, filled left to right, whose freed
//! spans are reused, and whose height is given back once a whole shelf is
//! empty. A raster keeps the one-pixel transparent border its bounds were
//! padded with, and is sampled no closer than half a texel to its edges, so
//! bilinear filtering never reaches a neighbor.
//!
//! The pages live on the CPU, in the lowering's [`RasterCache`]
//! (`super::ops`); each frame's [`Plan`](super::ops::Plan) carries a
//! snapshot of every page ([`AtlasPage`]: its pixels, shared, and what
//! changed in it lately). An executor keeps one texture per page and brings
//! it up to date with [`AtlasPage::upload`]: the whole page when it hasn't
//! got it (or its size changed), else only the area written since the
//! version it has.
//!
//! [`RasterCache`]: super::ops::RasterCache

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A page's first side, in pixels.
const FIRST_SIDE: u32 = 512;
/// A page's largest side.
const MAX_SIDE: u32 = 2048;
/// Pages at most; rasters that don't fit once they are all full (and
/// nothing can be evicted) are drawn as images of their own.
const MAX_PAGES: usize = 2;
/// Shelf heights are rounded up to this.
const SHELF_STEP: u32 = 4;
/// Versions of changes a page remembers, for executors a few frames behind.
const HISTORY: usize = 64;
/// The largest raster put in the atlas, per side; bigger ones are images of
/// their own.
pub(crate) const MAX_RASTER: u32 = 256;

/// Unique page ids (an executor keeps a texture per id).
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Where a raster is: a page and a pixel rectangle `[x, y, w, h]` in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    pub page: usize,
    pub rect: [u32; 4],
}

/// A row of slots of one height.
#[derive(Debug)]
struct Shelf {
    y: u32,
    h: u32,
    /// Free spans `(x, w)`, sorted by `x`, never touching.
    free: Vec<(u32, u32)>,
}

impl Shelf {
    fn new(y: u32, h: u32, side: u32) -> Self {
        Shelf {
            y,
            h,
            free: vec![(0, side)],
        }
    }

    fn empty(&self, side: u32) -> bool {
        self.free.len() == 1 && self.free[0] == (0, side)
    }

    /// Take `w` pixels from the first free span wide enough.
    fn take(&mut self, w: u32) -> Option<u32> {
        let i = self.free.iter().position(|&(_, fw)| fw >= w)?;
        let (x, fw) = self.free[i];
        if fw == w {
            self.free.remove(i);
        } else {
            self.free[i] = (x + w, fw - w);
        }
        Some(x)
    }

    /// Give back `[x, x + w)`, merging it with its neighbors.
    fn give(&mut self, x: u32, w: u32) {
        let i = self.free.partition_point(|&(fx, _)| fx < x);
        self.free.insert(i, (x, w));
        if i + 1 < self.free.len() && x + w == self.free[i + 1].0 {
            self.free[i].1 += self.free[i + 1].1;
            self.free.remove(i + 1);
        }
        if i > 0 && self.free[i - 1].0 + self.free[i - 1].1 == x {
            self.free[i - 1].1 += self.free[i].1;
            self.free.remove(i);
        }
    }
}

/// One page: its pixels, its shelves, and what changed in it.
#[derive(Debug)]
struct Page {
    id: u64,
    side: u32,
    /// Straight-alpha RGBA, `side`×`side`.
    pixels: Arc<Vec<u8>>,
    /// Top to bottom, covering `0..top`.
    shelves: Vec<Shelf>,
    top: u32,
    version: u64,
    /// `(version, rect)` for the last versions, oldest first; complete
    /// after `base`.
    history: VecDeque<(u64, [u32; 4])>,
    base: u64,
    /// Written since the last snapshot.
    pending: Option<[u32; 4]>,
}

impl Page {
    fn new(side: u32) -> Self {
        Page {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            side,
            pixels: Arc::new(vec![0; (side * side * 4) as usize]),
            shelves: Vec::new(),
            top: 0,
            version: 0,
            history: VecDeque::new(),
            base: 0,
            pending: None,
        }
    }

    /// Room for a `w`×`h` raster, or `None`.
    fn alloc(&mut self, w: u32, h: u32) -> Option<[u32; 4]> {
        if w > self.side || h > self.side {
            return None;
        }
        let side = self.side;
        // The snuggest shelf in use that's tall enough without wasting
        // much, then an empty one (cut to height), then a new one on top.
        let best = self
            .shelves
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                !s.empty(side)
                    && s.h >= h
                    && s.h <= h + h / 4 + SHELF_STEP
                    && s.free.iter().any(|&(_, fw)| fw >= w)
            })
            .min_by_key(|(_, s)| s.h)
            .map(|(i, _)| i);
        if let Some(i) = best {
            let s = &mut self.shelves[i];
            let x = s.take(w)?;
            return Some([x, s.y, w, h]);
        }
        let want = h.next_multiple_of(SHELF_STEP).min(side);
        if let Some(i) = self
            .shelves
            .iter()
            .enumerate()
            .filter(|(_, s)| s.empty(side) && s.h >= want)
            .min_by_key(|(_, s)| s.h)
            .map(|(i, _)| i)
        {
            let s = &mut self.shelves[i];
            if s.h > want {
                let rest = Shelf::new(s.y + want, s.h - want, side);
                s.h = want;
                self.shelves.insert(i + 1, rest);
            }
            let s = &mut self.shelves[i];
            let x = s.take(w)?;
            return Some([x, s.y, w, h]);
        }
        if self.top + want <= side {
            let mut s = Shelf::new(self.top, want, side);
            self.top += want;
            let x = s.take(w)?;
            let y = s.y;
            self.shelves.push(s);
            return Some([x, y, w, h]);
        }
        None
    }

    /// Give back the slot at `rect`.
    fn free(&mut self, rect: [u32; 4]) {
        let side = self.side;
        let Some(i) = self.shelves.iter().position(|s| s.y == rect[1]) else {
            return;
        };
        self.shelves[i].give(rect[0], rect[2]);
        if !self.shelves[i].empty(side) {
            return;
        }
        // Merge with empty neighbors; an empty shelf on top goes back to
        // the open space.
        if i + 1 < self.shelves.len() && self.shelves[i + 1].empty(side) {
            let next = self.shelves.remove(i + 1);
            self.shelves[i].h += next.h;
        }
        let i = if i > 0 && self.shelves[i - 1].empty(side) {
            let this = self.shelves.remove(i);
            self.shelves[i - 1].h += this.h;
            i - 1
        } else {
            i
        };
        if i + 1 == self.shelves.len() {
            let last = self.shelves.pop().unwrap();
            self.top = last.y;
        }
    }

    /// Double the side (up to `max`), keeping every raster where it is.
    fn grow(&mut self, max: u32) {
        if self.side >= max {
            return;
        }
        let (old, side) = (self.side, (self.side * 2).min(max));
        let mut pixels = vec![0; (side * side * 4) as usize];
        let (row, new_row) = (old as usize * 4, side as usize * 4);
        for y in 0..old as usize {
            pixels[y * new_row..y * new_row + row]
                .copy_from_slice(&self.pixels[y * row..(y + 1) * row]);
        }
        self.pixels = Arc::new(pixels);
        for s in &mut self.shelves {
            s.give(old, side - old);
        }
        self.side = side;
        // Its size changed: executors upload it whole.
        self.version += 1;
        self.history.clear();
        self.base = self.version;
        self.pending = None;
    }

    /// Write the `w`×`h` straight-alpha RGBA `data` at `rect`.
    fn write(&mut self, rect: [u32; 4], data: &[u8]) {
        let [x, y, w, h] = rect.map(|v| v as usize);
        let side = self.side as usize;
        let pixels = Arc::make_mut(&mut self.pixels);
        for r in 0..h {
            let at = ((y + r) * side + x) * 4;
            pixels[at..at + w * 4].copy_from_slice(&data[r * w * 4..(r + 1) * w * 4]);
        }
        let r = [rect[0], rect[1], rect[0] + rect[2], rect[1] + rect[3]];
        self.pending = Some(match self.pending {
            Some(p) => [
                p[0].min(r[0]),
                p[1].min(r[1]),
                p[2].max(r[2]),
                p[3].max(r[3]),
            ],
            None => r,
        });
    }

    /// Close this frame's changes into a version, and snapshot the page.
    fn snapshot(&mut self) -> AtlasPage {
        if let Some(r) = self.pending.take() {
            self.version += 1;
            self.history.push_back((self.version, r));
            if self.history.len() > HISTORY {
                let (v, _) = self.history.pop_front().unwrap();
                self.base = v;
            }
        }
        AtlasPage {
            id: self.id,
            side: self.side,
            pixels: self.pixels.clone(),
            version: self.version,
            base: self.base,
            history: self.history.iter().copied().collect(),
        }
    }
}

/// A raster in the atlas.
#[derive(Debug)]
struct Entry<T> {
    slot: Slot,
    used: u64,
    data: T,
}

/// Rasters packed into pages, keyed by `u64`, each with some `T` beside it.
#[derive(Debug)]
pub(crate) struct Atlas<T> {
    pages: Vec<Page>,
    entries: HashMap<u64, Entry<T>>,
    frame: u64,
    max_side: u32,
    max_pages: usize,
}

impl<T> Default for Atlas<T> {
    fn default() -> Self {
        Self::with_limits(MAX_SIDE, MAX_PAGES)
    }
}

impl<T> Atlas<T> {
    /// An atlas of at most `max_pages` pages of at most `max_side` pixels.
    pub(crate) fn with_limits(max_side: u32, max_pages: usize) -> Self {
        Atlas {
            pages: Vec::new(),
            entries: HashMap::new(),
            frame: 0,
            max_side,
            max_pages,
        }
    }

    /// Rasters held.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// The raster for `key`, marked used this frame.
    pub(crate) fn get(&mut self, key: u64) -> Option<(Slot, &T)> {
        let frame = self.frame;
        let e = self.entries.get_mut(&key)?;
        e.used = frame;
        Some((e.slot, &e.data))
    }

    /// Put the `w`×`h` straight-alpha RGBA raster `pixels` in for `key`:
    /// in free space, else in a page grown or opened for it, else in space
    /// freed by evicting the least recently used rasters. `None` when it
    /// doesn't fit (too big, or every page is full of this frame's rasters).
    pub(crate) fn insert(
        &mut self,
        key: u64,
        w: u32,
        h: u32,
        pixels: &[u8],
        data: T,
    ) -> Option<(Slot, &T)> {
        if w == 0 || h == 0 || w > MAX_RASTER || h > MAX_RASTER || w > self.max_side {
            return None;
        }
        debug_assert_eq!(pixels.len(), (w * h * 4) as usize);
        if let Some(old) = self.entries.remove(&key) {
            self.pages[old.slot.page].free(old.slot.rect);
        }
        let slot = self.place(w, h)?;
        self.pages[slot.page].write(slot.rect, pixels);
        let frame = self.frame;
        let e = self.entries.entry(key).or_insert(Entry {
            slot,
            used: frame,
            data,
        });
        Some((e.slot, &e.data))
    }

    fn alloc(&mut self, w: u32, h: u32) -> Option<Slot> {
        self.pages
            .iter_mut()
            .enumerate()
            .find_map(|(page, p)| p.alloc(w, h).map(|rect| Slot { page, rect }))
    }

    fn place(&mut self, w: u32, h: u32) -> Option<Slot> {
        if let Some(s) = self.alloc(w, h) {
            return Some(s);
        }
        // Grow a page, the smallest first.
        loop {
            let max = self.max_side;
            let Some(p) = self
                .pages
                .iter_mut()
                .filter(|p| p.side < max)
                .min_by_key(|p| p.side)
            else {
                break;
            };
            p.grow(max);
            if let Some(s) = self.alloc(w, h) {
                return Some(s);
            }
        }
        if self.pages.len() < self.max_pages {
            self.pages.push(Page::new(FIRST_SIDE.min(self.max_side)));
            return self.place(w, h);
        }
        // Evict the least recently used, a quarter of them at a time,
        // never what this frame drew.
        let frame = self.frame;
        let mut old: Vec<(u64, u64)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.used < frame)
            .map(|(&k, e)| (e.used, k))
            .collect();
        old.sort_unstable();
        let mut old = old.into_iter().peekable();
        while old.peek().is_some() {
            let n = (self.entries.len() / 4).max(1);
            for (_, k) in old.by_ref().take(n) {
                if let Some(e) = self.entries.remove(&k) {
                    self.pages[e.slot.page].free(e.slot.rect);
                }
            }
            if let Some(s) = self.alloc(w, h) {
                return Some(s);
            }
        }
        None
    }

    /// Drop rasters unused for `keep` frames, and past `max` rasters the
    /// least recently used.
    pub(crate) fn sweep(&mut self, keep: u64, max: usize) {
        let frame = self.frame;
        let mut gone: Vec<u64> = self
            .entries
            .iter()
            .filter(|(_, e)| frame.saturating_sub(e.used) >= keep)
            .map(|(&k, _)| k)
            .collect();
        if self.entries.len() - gone.len() > max {
            let mut ages: Vec<(u64, u64)> = self
                .entries
                .iter()
                .filter(|(_, e)| frame.saturating_sub(e.used) < keep)
                .map(|(&k, e)| (e.used, k))
                .collect();
            ages.sort_unstable();
            let extra = ages.len() - max;
            gone.extend(ages[..extra].iter().map(|&(_, k)| k));
        }
        for k in gone {
            if let Some(e) = self.entries.remove(&k) {
                self.pages[e.slot.page].free(e.slot.rect);
            }
        }
        self.frame += 1;
    }

    /// Close this frame's changes and snapshot every page, for the plan.
    pub(crate) fn snapshot(&mut self) -> Vec<AtlasPage> {
        self.pages.iter_mut().map(Page::snapshot).collect()
    }
}

/// A page as a frame sees it: its pixels and what changed in it lately.
#[derive(Clone, Debug)]
pub(crate) struct AtlasPage {
    /// Unique across every atlas: an executor's key for its texture.
    pub id: u64,
    /// Pixels on a side (it is square).
    pub side: u32,
    /// Straight-alpha RGBA, `side`×`side`.
    pub pixels: Arc<Vec<u8>>,
    /// Goes up with every frame that wrote to it.
    pub version: u64,
    base: u64,
    history: Vec<(u64, [u32; 4])>,
}

/// What an executor uploads to bring its texture of a page up to date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Upload {
    Nothing,
    /// All of it (into a new texture of [`AtlasPage::side`]).
    Whole,
    /// The pixels in `[x0, y0, x1, y1]` (see [`AtlasPage::region`]).
    Region([u32; 4]),
}

impl AtlasPage {
    /// What to upload to a texture holding version `have` of this page at
    /// `side` pixels (`None`: no texture yet).
    pub(crate) fn upload(&self, have: Option<(u32, u64)>) -> Upload {
        let Some((side, version)) = have else {
            return Upload::Whole;
        };
        if side != self.side || version > self.version || version < self.base {
            return Upload::Whole;
        }
        if version == self.version {
            return Upload::Nothing;
        }
        let mut r: Option<[u32; 4]> = None;
        for &(v, c) in &self.history {
            if v > version {
                r = Some(match r {
                    Some(p) => [
                        p[0].min(c[0]),
                        p[1].min(c[1]),
                        p[2].max(c[2]),
                        p[3].max(c[3]),
                    ],
                    None => c,
                });
            }
        }
        r.map_or(Upload::Nothing, Upload::Region)
    }

    /// The pixels of `[x0, y0, x1, y1]`, row after row with no padding.
    pub(crate) fn region(&self, r: [u32; 4]) -> Vec<u8> {
        let side = self.side as usize;
        let [x0, y0, x1, y1] = r.map(|v| v as usize);
        let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
        for y in y0..y1 {
            out.extend_from_slice(&self.pixels[(y * side + x0) * 4..(y * side + x1) * 4]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(w: u32, h: u32, v: u8) -> Vec<u8> {
        vec![v; (w * h * 4) as usize]
    }

    fn overlap(a: [u32; 4], b: [u32; 4]) -> bool {
        a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
    }

    #[test]
    fn rasters_pack_without_overlapping_and_keep_their_pixels() {
        let mut atlas: Atlas<()> = Atlas::default();
        let mut slots = Vec::new();
        for i in 0..300u64 {
            let (w, h) = (6 + (i * 7 % 23) as u32, 10 + (i * 5 % 17) as u32);
            let (s, _) = atlas.insert(i, w, h, &raster(w, h, i as u8), ()).unwrap();
            assert_eq!((s.rect[2], s.rect[3]), (w, h));
            slots.push(s);
        }
        for (i, a) in slots.iter().enumerate() {
            for b in &slots[i + 1..] {
                assert!(a.page != b.page || !overlap(a.rect, b.rect), "{a:?} {b:?}");
            }
        }
        let pages = atlas.snapshot();
        assert_eq!(pages.len(), 1, "300 small rasters fit one page");
        assert_eq!(pages[0].side, FIRST_SIDE);
        // Each raster's pixels are where its slot says.
        let p = &pages[0];
        for (i, s) in slots.iter().enumerate() {
            let at = ((s.rect[1] * p.side + s.rect[0]) * 4) as usize;
            assert_eq!(p.pixels[at], i as u8);
        }
        // Shelves are reused: a shelf holds many rasters.
        assert!(atlas.pages[0].shelves.len() < 40);
    }

    #[test]
    fn a_full_page_grows_then_a_second_one_opens() {
        let mut atlas: Atlas<()> = Atlas::default();
        let mut i = 0u64;
        // 128×128 rasters: 16 fill a 512 page.
        while atlas.pages.first().is_none_or(|p| p.side == FIRST_SIDE) {
            atlas.insert(i, 128, 128, &raster(128, 128, 1), ()).unwrap();
            i += 1;
        }
        assert_eq!(i, 17, "the 17th raster grew the page");
        assert_eq!(atlas.pages[0].side, FIRST_SIDE * 2);
        // Everything placed before the growth is still there.
        for k in 0..i {
            assert!(atlas.get(k).is_some());
        }
        while atlas.pages.len() < 2 {
            atlas.insert(i, 128, 128, &raster(128, 128, 1), ()).unwrap();
            i += 1;
        }
        assert_eq!(atlas.pages[0].side, MAX_SIDE);
        assert_eq!(i, (MAX_SIDE / 128).pow(2) as u64 + 1);
    }

    #[test]
    fn a_full_atlas_evicts_the_least_recently_used() {
        let mut atlas: Atlas<u32> = Atlas::with_limits(256, 1);
        // Four 128×128 rasters fill the one page, over four frames.
        for k in 0..4u64 {
            atlas
                .insert(k, 128, 128, &raster(128, 128, 1), k as u32)
                .unwrap();
            atlas.sweep(1000, 1000);
        }
        // Use 0, 2 and 3 again: 1 is the least recently used.
        for k in [0, 2, 3] {
            assert!(atlas.get(k).is_some());
        }
        atlas.sweep(1000, 1000);
        for k in [0, 2, 3] {
            atlas.get(k);
        }
        let (slot, data) = atlas.insert(9, 128, 128, &raster(128, 128, 2), 9).unwrap();
        assert_eq!(*data, 9);
        assert!(atlas.get(1).is_none(), "1 was evicted");
        assert!(atlas.get(0).is_some() && atlas.get(2).is_some() && atlas.get(3).is_some());
        assert_eq!(slot.page, 0);
        // Everything is drawn this frame: nothing more can go.
        assert!(
            atlas
                .insert(10, 128, 128, &raster(128, 128, 3), 10)
                .is_none()
        );
    }

    #[test]
    fn freed_shelves_are_merged_and_reused_at_any_height() {
        let mut atlas: Atlas<()> = Atlas::with_limits(64, 1);
        for k in 0..4u64 {
            atlas.insert(k, 64, 16, &raster(64, 16, 1), ()).unwrap();
        }
        assert!(atlas.insert(9, 64, 32, &raster(64, 32, 1), ()).is_none());
        atlas.sweep(1000, 1000);
        // Evicting two 16-high shelves makes room for a 32-high raster.
        let (s, _) = atlas.insert(9, 64, 32, &raster(64, 32, 1), ()).unwrap();
        assert_eq!(s.rect[3], 32);
        assert_eq!(atlas.len(), 3);
    }

    #[test]
    fn an_executor_uploads_only_what_changed_since_its_version() {
        let mut atlas: Atlas<()> = Atlas::default();
        atlas.insert(1, 4, 4, &raster(4, 4, 7), ()).unwrap();
        let p = atlas.snapshot().remove(0);
        assert_eq!(p.upload(None), Upload::Whole);
        let have = Some((p.side, p.version));
        assert_eq!(p.upload(have), Upload::Nothing);
        atlas.sweep(1000, 1000);
        let (s, _) = atlas.insert(2, 3, 5, &raster(3, 5, 9), ()).unwrap();
        let q = atlas.snapshot().remove(0);
        let r = s.rect;
        let region = [r[0], r[1], r[0] + 3, r[1] + 5];
        assert_eq!(q.upload(have), Upload::Region(region));
        assert_eq!(q.region(region), raster(3, 5, 9));
        // A frame that wrote nothing has nothing new.
        let again = atlas.snapshot().remove(0);
        assert_eq!(again.version, q.version);
        // Growing the page uploads it whole.
        let mut big: Atlas<()> = Atlas::default();
        big.insert(1, 4, 4, &raster(4, 4, 1), ()).unwrap();
        let small = big.snapshot().remove(0);
        let mut k = 2;
        while big.pages[0].side == FIRST_SIDE {
            big.insert(k, 200, 200, &raster(200, 200, 1), ()).unwrap();
            k += 1;
        }
        let grown = big.snapshot().remove(0);
        assert_eq!(
            grown.upload(Some((small.side, small.version))),
            Upload::Whole
        );
    }

    #[test]
    fn unused_rasters_are_swept() {
        let mut atlas: Atlas<()> = Atlas::default();
        atlas.insert(1, 8, 8, &raster(8, 8, 1), ()).unwrap();
        atlas.insert(2, 8, 8, &raster(8, 8, 1), ()).unwrap();
        for _ in 0..4 {
            atlas.get(2);
            atlas.sweep(3, 1000);
        }
        assert!(atlas.get(1).is_none());
        assert!(atlas.get(2).is_some());
        // Past `max`, the oldest go.
        atlas.insert(3, 8, 8, &raster(8, 8, 1), ()).unwrap();
        atlas.sweep(3, 1);
        assert_eq!(atlas.len(), 1);
    }
}
