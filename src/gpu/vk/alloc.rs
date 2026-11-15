//! Sub-allocating a block of device memory: textures (one per image and per
//! glyph) share a few large allocations instead of each taking one of the
//! device's limited allocations (`maxMemoryAllocationCount` is 4096 on many
//! drivers).
//!
//! [`Ranges`] is the bookkeeping only: first-fit, aligned, coalescing on free.

/// The free parts of a block of `size` bytes.
#[derive(Debug)]
pub(crate) struct Ranges {
    size: u64,
    /// `(offset, len)`, sorted by offset, never touching.
    free: Vec<(u64, u64)>,
}

impl Ranges {
    pub(crate) fn new(size: u64) -> Self {
        Ranges {
            size,
            free: vec![(0, size)],
        }
    }

    /// Take `size` bytes at an offset that is a multiple of `align` (a power
    /// of two), or `None` when nothing free is big enough.
    pub(crate) fn alloc(&mut self, size: u64, align: u64) -> Option<u64> {
        let align = align.max(1);
        for i in 0..self.free.len() {
            let (o, l) = self.free[i];
            let start = o.next_multiple_of(align);
            let end = start.checked_add(size)?;
            if end > o + l {
                continue;
            }
            let mut pieces = Vec::with_capacity(2);
            if start > o {
                pieces.push((o, start - o));
            }
            if end < o + l {
                pieces.push((end, o + l - end));
            }
            self.free.splice(i..=i, pieces);
            return Some(start);
        }
        None
    }

    /// Give back `size` bytes at `offset` (from [`alloc`](Self::alloc)).
    pub(crate) fn free(&mut self, offset: u64, size: u64) {
        let i = self.free.partition_point(|&(o, _)| o < offset);
        self.free.insert(i, (offset, size));
        // Merge with the next, then with the previous.
        if i + 1 < self.free.len() && self.free[i].0 + self.free[i].1 == self.free[i + 1].0 {
            self.free[i].1 += self.free[i + 1].1;
            self.free.remove(i + 1);
        }
        if i > 0 && self.free[i - 1].0 + self.free[i - 1].1 == self.free[i].0 {
            self.free[i - 1].1 += self.free[i].1;
            self.free.remove(i);
        }
    }

    /// Whether nothing is taken.
    pub(crate) fn is_empty(&self) -> bool {
        self.free == [(0, self.size)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocations_are_aligned_and_dont_overlap() {
        let mut r = Ranges::new(1024);
        let a = r.alloc(100, 1).unwrap();
        let b = r.alloc(100, 256).unwrap();
        let c = r.alloc(10, 4).unwrap();
        assert_eq!((a, b, c), (0, 256, 100));
        assert_eq!(b % 256, 0);
        assert!(r.alloc(1000, 1).is_none());
        assert!(!r.is_empty());
    }

    #[test]
    fn freed_ranges_coalesce_and_are_reused() {
        let mut r = Ranges::new(300);
        let a = r.alloc(100, 1).unwrap();
        let b = r.alloc(100, 1).unwrap();
        let c = r.alloc(100, 1).unwrap();
        assert!(r.alloc(1, 1).is_none());
        r.free(b, 100);
        assert_eq!(r.alloc(100, 1), Some(100));
        r.free(100, 100);
        r.free(a, 100);
        r.free(c, 100);
        assert!(r.is_empty());
        assert_eq!(r.alloc(300, 1), Some(0));
    }

    #[test]
    fn a_too_big_request_fails_without_overflow() {
        let mut r = Ranges::new(64);
        assert!(r.alloc(u64::MAX, 1).is_none());
        assert!(r.alloc(65, 1).is_none());
        assert_eq!(r.alloc(64, 64), Some(0));
    }
}
