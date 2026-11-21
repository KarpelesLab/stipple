//! Apple's `morx` glyph metamorphosis table (AAT), read by hand: the
//! substitutions Apple Color Emoji forms its skin tones, flags, keycaps and
//! ZWJ sequences with, and that other AAT-only fonts rely on.
//!
//! A `morx` table is a list of chains, each a list of subtables applied in
//! turn to the whole glyph run when their feature flags are among the
//! chain's defaults. Four kinds are read: rearrangement (0), contextual (1),
//! ligature (2) and noncontextual (4). The state machine drivers follow
//! Apple's TrueType Reference Manual and HarfBuzz's behaviour where the
//! manual is vague (end of text, a mark never set, a ligature stack running
//! out). Insertion subtables (5) are skipped: no system emoji font uses
//! them. Only the chain defaults are applied (no feature selection), for
//! horizontal text in logical order.

/// Glyph id marking a glyph a ligature consumed; removed at the end.
const DELETED: u16 = 0xFFFF;

/// Classes every extended state table reserves.
const END_OF_TEXT: u16 = 0;
const OUT_OF_BOUNDS: u16 = 1;
const DELETED_GLYPH: u16 = 2;

/// Coverage bits of a subtable.
const VERTICAL: u32 = 0x8000_0000;
const BACKWARDS: u32 = 0x4000_0000;
const ALL_DIRECTIONS: u32 = 0x2000_0000;

/// Entry flags shared by the state machines.
const DONT_ADVANCE: u16 = 0x4000;

/// How many glyphs a ligature subtable remembers as components.
const STACK: usize = 64;

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        b.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// The `tag` table of the face whose table directory starts at `header` in
/// `bytes` (0 for a lone font; a collection's per-face offset otherwise).
pub(crate) fn find_table<'a>(bytes: &'a [u8], header: usize, tag: &[u8; 4]) -> Option<&'a [u8]> {
    let n = be16(bytes, header.checked_add(4)?)? as usize;
    for i in 0..n {
        let rec = header + 12 + i * 16;
        if bytes.get(rec..rec + 4)? == tag {
            let off = be32(bytes, rec + 8)? as usize;
            let len = be32(bytes, rec + 12)? as usize;
            return bytes.get(off..off.checked_add(len)?);
        }
    }
    None
}

/// Where face `index` of a collection starts (0 for a lone font).
pub(crate) fn face_header(bytes: &[u8], index: Option<u32>) -> usize {
    match index {
        Some(i) if bytes.get(0..4) == Some(b"ttcf") => {
            be32(bytes, 12 + i as usize * 4).unwrap_or(0) as usize
        }
        _ => 0,
    }
}

/// An AAT lookup table (formats 0, 2, 4, 6, 8 and 10) at the start of `t`:
/// the value for `glyph`, if the table has one.
pub(crate) fn lookup(t: &[u8], glyph: u16, num_glyphs: u16) -> Option<u16> {
    let format = be16(t, 0)?;
    let g = glyph as usize;
    match format {
        // Simple array, one value per glyph.
        0 => {
            if glyph >= num_glyphs {
                return None;
            }
            be16(t, 2 + g * 2)
        }
        // Segments mapping a range to one value (2), or to an array of
        // values (4).
        2 | 4 => {
            let unit = be16(t, 2)? as usize;
            let n = be16(t, 4)? as usize;
            if unit < 6 {
                return None;
            }
            let (mut lo, mut hi) = (0usize, n);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let rec = 12 + mid * unit;
                let last = be16(t, rec)?;
                let first = be16(t, rec + 2)?;
                if glyph > last {
                    lo = mid + 1;
                } else if glyph < first {
                    hi = mid;
                } else {
                    if last == 0xFFFF && first == 0xFFFF {
                        return None; // the terminating segment
                    }
                    let v = be16(t, rec + 4)?;
                    return if format == 2 {
                        Some(v)
                    } else {
                        be16(t, v as usize + (glyph - first) as usize * 2)
                    };
                }
            }
            None
        }
        // Single glyphs, each with its value.
        6 => {
            let unit = be16(t, 2)? as usize;
            let n = be16(t, 4)? as usize;
            if unit < 4 {
                return None;
            }
            let (mut lo, mut hi) = (0usize, n);
            while lo < hi {
                let mid = (lo + hi) / 2;
                let rec = 12 + mid * unit;
                let at = be16(t, rec)?;
                match glyph.cmp(&at) {
                    std::cmp::Ordering::Greater => lo = mid + 1,
                    std::cmp::Ordering::Less => hi = mid,
                    std::cmp::Ordering::Equal => {
                        return if at == 0xFFFF { None } else { be16(t, rec + 2) };
                    }
                }
            }
            None
        }
        // A trimmed array: values for a run of glyphs.
        8 => {
            let first = be16(t, 2)?;
            let count = be16(t, 4)?;
            let i = glyph.checked_sub(first)?;
            if i >= count {
                return None;
            }
            be16(t, 6 + i as usize * 2)
        }
        // The same, with values of 1, 2, 4 or 8 bytes (only the low 16
        // bits are wanted here).
        10 => {
            let size = be16(t, 2)? as usize;
            let first = be16(t, 4)?;
            let count = be16(t, 6)?;
            let i = glyph.checked_sub(first)?;
            if i >= count || !matches!(size, 1 | 2 | 4 | 8) {
                return None;
            }
            let at = 8 + i as usize * size;
            let bytes = t.get(at..at + size)?;
            let mut v: u64 = 0;
            for &b in bytes {
                v = (v << 8) | b as u64;
            }
            Some(v as u16)
        }
        _ => None,
    }
}

/// An extended state table's header (`STXHeader`): `t` starts at it.
struct StateTable<'a> {
    t: &'a [u8],
    classes: u32,
    class_table: usize,
    states: usize,
    entries: usize,
    num_glyphs: u16,
}

impl<'a> StateTable<'a> {
    fn new(t: &'a [u8], num_glyphs: u16) -> Option<Self> {
        Some(Self {
            t,
            classes: be32(t, 0)?,
            class_table: be32(t, 4)? as usize,
            states: be32(t, 8)? as usize,
            entries: be32(t, 12)? as usize,
            num_glyphs,
        })
    }

    fn class(&self, glyph: u16) -> u16 {
        if glyph == DELETED {
            return DELETED_GLYPH;
        }
        self.t
            .get(self.class_table..)
            .and_then(|t| lookup(t, glyph, self.num_glyphs))
            .unwrap_or(OUT_OF_BOUNDS)
    }

    /// The entry index for `state` and `class`.
    fn entry(&self, state: u16, class: u16) -> Option<u16> {
        let class = if (class as u32) < self.classes {
            class
        } else {
            OUT_OF_BOUNDS
        };
        let at = self.states + (state as usize * self.classes as usize + class as usize) * 2;
        be16(self.t, at)
    }

    /// Entry `i`'s bytes, entries being `size` bytes each.
    fn entry_data(&self, i: u16, size: usize) -> Option<&'a [u8]> {
        let at = self.entries + i as usize * size;
        self.t.get(at..at + size)
    }
}

/// A state machine's actions: `transition` sees each glyph (and the end of
/// text, at `i == glyphs.len()`) with the entry its state and class chose.
trait Machine {
    /// Bytes per entry, `newState` and `flags` first.
    const ENTRY: usize;
    fn transition(&mut self, glyphs: &mut [u16], i: usize, flags: u16, entry: &[u8]);
}

/// Run `machine` over `glyphs` through `table`.
fn drive<M: Machine>(table: &StateTable<'_>, glyphs: &mut [u16], machine: &mut M) {
    let len = glyphs.len();
    let mut state: u16 = 0;
    let mut i = 0usize;
    // A guard against tables that never advance.
    let mut budget = len * 16 + 64;
    loop {
        let class = if i < len {
            table.class(glyphs[i])
        } else {
            END_OF_TEXT
        };
        let Some(e) = table.entry(state, class) else {
            return;
        };
        let Some(entry) = table.entry_data(e, M::ENTRY) else {
            return;
        };
        let new_state = u16::from_be_bytes([entry[0], entry[1]]);
        let flags = u16::from_be_bytes([entry[2], entry[3]]);
        machine.transition(glyphs, i, flags, entry);
        state = new_state;
        if i >= len {
            return;
        }
        if flags & DONT_ADVANCE == 0 || budget == 0 {
            i += 1;
        } else {
            budget -= 1;
        }
    }
}

/// Type 0: swap glyphs at the ends of a marked range.
struct Rearrangement {
    start: usize,
    end: usize,
}

impl Machine for Rearrangement {
    const ENTRY: usize = 4;
    fn transition(&mut self, glyphs: &mut [u16], i: usize, flags: u16, _: &[u8]) {
        const MARK_FIRST: u16 = 0x8000;
        const MARK_LAST: u16 = 0x2000;
        // Per verb: how many glyphs move from the start (high nibble) and
        // the end (low), 3 meaning two that also swap.
        const MAP: [u8; 16] = [
            0x00, 0x10, 0x01, 0x11, 0x20, 0x30, 0x02, 0x03, 0x12, 0x13, 0x21, 0x31, 0x22, 0x32,
            0x23, 0x33,
        ];
        let len = glyphs.len();
        if flags & MARK_FIRST != 0 {
            self.start = i;
        }
        if flags & MARK_LAST != 0 {
            self.end = (i + 1).min(len);
        }
        let verb = (flags & 0xF) as usize;
        if verb == 0 || self.start >= self.end || self.end > len {
            return;
        }
        let m = MAP[verb];
        let l = 2.min(m >> 4) as usize;
        let r = 2.min(m & 0xF) as usize;
        let (start, end) = (self.start, self.end);
        if end - start < l + r {
            return;
        }
        let left: Vec<u16> = glyphs[start..start + l].to_vec();
        let right: Vec<u16> = glyphs[end - r..end].to_vec();
        let middle: Vec<u16> = glyphs[start + l..end - r].to_vec();
        let mut out = right;
        out.extend(middle);
        out.extend(left);
        glyphs[start..end].copy_from_slice(&out);
        if m >> 4 == 3 {
            glyphs.swap(end - 1, end - 2);
        }
        if m & 0xF == 3 {
            glyphs.swap(start, start + 1);
        }
    }
}

/// Type 1: substitute the current glyph and a marked earlier one through
/// lookup tables the entry names.
struct Contextual<'a> {
    /// The subtable's list of lookup table offsets.
    subs: &'a [u8],
    num_glyphs: u16,
    mark: usize,
    mark_set: bool,
}

impl Contextual<'_> {
    fn substitute(&self, index: u16, glyph: u16) -> Option<u16> {
        if index == 0xFFFF || glyph == DELETED {
            return None;
        }
        let off = be32(self.subs, index as usize * 4)? as usize;
        lookup(self.subs.get(off..)?, glyph, self.num_glyphs)
    }
}

impl Machine for Contextual<'_> {
    const ENTRY: usize = 8;
    fn transition(&mut self, glyphs: &mut [u16], i: usize, flags: u16, entry: &[u8]) {
        const SET_MARK: u16 = 0x8000;
        let len = glyphs.len();
        if len == 0 || (i >= len && !self.mark_set) {
            return;
        }
        let mark_index = u16::from_be_bytes([entry[4], entry[5]]);
        let current_index = u16::from_be_bytes([entry[6], entry[7]]);
        if self.mark < len
            && let Some(g) = self.substitute(mark_index, glyphs[self.mark])
        {
            glyphs[self.mark] = g;
        }
        let at = i.min(len - 1);
        if let Some(g) = self.substitute(current_index, glyphs[at]) {
            glyphs[at] = g;
        }
        if flags & SET_MARK != 0 {
            self.mark_set = true;
            self.mark = i;
        }
    }
}

/// Type 2: glyphs pushed as components, joined into a ligature by actions
/// that sum each component's value into an index of the ligature list.
struct Ligature<'a> {
    actions: &'a [u8],
    components: &'a [u8],
    ligatures: &'a [u8],
    stack: [usize; STACK],
    len: usize,
}

impl Machine for Ligature<'_> {
    const ENTRY: usize = 6;
    fn transition(&mut self, glyphs: &mut [u16], i: usize, flags: u16, entry: &[u8]) {
        const SET_COMPONENT: u16 = 0x8000;
        const PERFORM_ACTION: u16 = 0x2000;
        const LAST: u32 = 0x8000_0000;
        const STORE: u32 = 0x4000_0000;
        if flags & SET_COMPONENT != 0 {
            // The same glyph twice (after a "don't advance") counts once.
            if self.len > 0 && self.stack[(self.len - 1) % STACK] == i {
                self.len -= 1;
            }
            self.stack[self.len % STACK] = i;
            self.len += 1;
        }
        if flags & PERFORM_ACTION == 0 || self.len == 0 || i >= glyphs.len() {
            return;
        }
        let mut action_at = u16::from_be_bytes([entry[4], entry[5]]) as usize * 4;
        let mut cursor = self.len;
        let mut ligature_idx: u32 = 0;
        loop {
            if cursor == 0 {
                // The stack ran out: forget it.
                self.len = 0;
                return;
            }
            cursor -= 1;
            let pos = self.stack[cursor % STACK];
            let Some(action) = be32(self.actions, action_at) else {
                return;
            };
            let Some(&glyph) = glyphs.get(pos) else {
                return;
            };
            // A 30-bit signed offset added to the glyph id.
            let mut offset = action & 0x3FFF_FFFF;
            if offset & 0x2000_0000 != 0 {
                offset |= 0xC000_0000;
            }
            let component = (glyph as u32).wrapping_add(offset) as usize;
            let Some(value) = be16(self.components, component.wrapping_mul(2)) else {
                return;
            };
            ligature_idx = ligature_idx.wrapping_add(value as u32);
            if action & (STORE | LAST) != 0 {
                let Some(lig) = be16(self.ligatures, ligature_idx as usize * 2) else {
                    return;
                };
                glyphs[pos] = lig;
                // The components after this one are part of it.
                while self.len - 1 > cursor {
                    self.len -= 1;
                    let p = self.stack[self.len % STACK];
                    if let Some(g) = glyphs.get_mut(p) {
                        *g = DELETED;
                    }
                }
            }
            action_at += 4;
            if action & LAST != 0 {
                return;
            }
        }
    }
}

/// Apply `morx` table `morx` to `glyphs` (of a font with `num_glyphs`
/// glyphs). Each output glyph comes with how many input glyphs it stands
/// for (a ligature its components', a deleted glyph's count going to the
/// glyph before it).
pub(crate) fn apply(morx: &[u8], num_glyphs: u16, glyphs: &[u16]) -> (Vec<u16>, Vec<u16>) {
    let mut g = glyphs.to_vec();
    let version = be16(morx, 0).unwrap_or(0);
    if (2..=3).contains(&version)
        && let Some(chains) = be32(morx, 4)
    {
        let mut at = 8usize;
        for _ in 0..chains {
            let Some(len) = apply_chain(morx, at, num_glyphs, &mut g) else {
                break;
            };
            at += len;
        }
    }
    // Drop deleted glyphs, folding their counts into the glyph before (or,
    // leading ones, the glyph after).
    let mut out = Vec::with_capacity(g.len());
    let mut counts: Vec<u16> = Vec::with_capacity(g.len());
    let mut pending: u16 = 0;
    for glyph in g {
        if glyph != DELETED {
            out.push(glyph);
            counts.push(1 + std::mem::take(&mut pending));
        } else if let Some(c) = counts.last_mut() {
            *c = c.saturating_add(1);
        } else {
            pending = pending.saturating_add(1);
        }
    }
    (out, counts)
}

/// One chain at `at`: its subtables applied to `glyphs`. The chain's length.
fn apply_chain(morx: &[u8], at: usize, num_glyphs: u16, glyphs: &mut [u16]) -> Option<usize> {
    let flags = be32(morx, at)?;
    let length = be32(morx, at + 4)? as usize;
    let features = be32(morx, at + 8)? as usize;
    let subtables = be32(morx, at + 12)?;
    let chain = morx.get(at..at.checked_add(length)?)?;
    let mut sub = 16 + features.checked_mul(12)?;
    for _ in 0..subtables {
        let len = be32(chain, sub)? as usize;
        let coverage = be32(chain, sub + 4)?;
        let sub_flags = be32(chain, sub + 8)?;
        if len < 12 {
            break;
        }
        let body = chain.get(sub + 12..sub + len)?;
        let horizontal = coverage & ALL_DIRECTIONS != 0 || coverage & VERTICAL == 0;
        if sub_flags & flags != 0 && horizontal {
            let backwards = coverage & BACKWARDS != 0;
            if backwards {
                glyphs.reverse();
            }
            apply_subtable(coverage & 0xFF, body, num_glyphs, glyphs);
            if backwards {
                glyphs.reverse();
            }
        }
        sub += len;
    }
    Some(length)
}

fn apply_subtable(kind: u32, body: &[u8], num_glyphs: u16, glyphs: &mut [u16]) {
    match kind {
        0 => {
            if let Some(table) = StateTable::new(body, num_glyphs) {
                drive(&table, glyphs, &mut Rearrangement { start: 0, end: 0 });
            }
        }
        1 => {
            let Some(table) = StateTable::new(body, num_glyphs) else {
                return;
            };
            let Some(subs) = be32(body, 16).and_then(|o| body.get(o as usize..)) else {
                return;
            };
            let mut m = Contextual {
                subs,
                num_glyphs,
                mark: 0,
                mark_set: false,
            };
            drive(&table, glyphs, &mut m);
        }
        2 => {
            let Some(table) = StateTable::new(body, num_glyphs) else {
                return;
            };
            let part = |at: usize| be32(body, at).and_then(|o| body.get(o as usize..));
            let (Some(actions), Some(components), Some(ligatures)) = (part(16), part(20), part(24))
            else {
                return;
            };
            let mut m = Ligature {
                actions,
                components,
                ligatures,
                stack: [0; STACK],
                len: 0,
            };
            drive(&table, glyphs, &mut m);
        }
        4 => {
            for g in glyphs.iter_mut() {
                if *g != DELETED
                    && let Some(n) = lookup(body, *g, num_glyphs)
                {
                    *g = n;
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "morx_tests.rs"]
mod tests;
