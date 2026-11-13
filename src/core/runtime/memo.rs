//! Memoized subtrees and derived values: parts of a view that are built again
//! only when their inputs change.
//!
//! [`Cx::memo`] keeps the [`Element`] a closure built, by key, with a hash of
//! its inputs (its *deps*). While the deps hash the same, later builds skip
//! the closure and clone the kept element — and with it the handlers the
//! closure registered: they are moved over from the previous frame's table,
//! so the ids stamped on the kept elements still name them.
//!
//! Ids are handed out in registration order, so a kept subtree is only reused
//! when its handlers land where they were: when everything built before it
//! registered as many taps, keys, scroll containers… as last time. Otherwise
//! (a button appeared above it, say) it is built again, and kept at its new
//! place from then on. Ids a handler captured (a `ScrollId` to scroll from a
//! button) therefore stay right.
//!
//! What the closure read through the [`Cx`] counts as input too: the
//! interaction states ([`Cx::interaction`]), drop-target states
//! ([`Cx::drop_state`]), scroll offsets ([`Cx::scroll_offset`]) and window
//! info ([`Cx::window_info`]) it read are compared, as are the theme, the
//! window size and focus. A subtree that asks for frames (it animates), opens
//! overlays, registers context menus or sets the window title is never kept.
//!
//! [`Cx::derive`] caches a value the same way: computed again only when its
//! deps change.

use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use super::{Cx, DropTarget, Handlers, LayoutNode, ScrollId};
use crate::core::dnd::DropState;
use crate::core::element::Element;
use crate::core::interact::{ElementKey, Interaction};
use crate::core::{DropId, pointer_events::Slot};
use crate::geometry::{Rect, Size, Vec2};
use crate::layout::LayoutDirection;
use crate::platform::WindowInfo;
use crate::style::Theme;

/// What memoized subtrees and derived values a window kept from its last
/// build (see [`Cx::memo`] and [`Cx::derive`]). Opaque: the app hands it from
/// one build's [`Cx`] to the next.
#[derive(Default)]
pub struct MemoCache {
    subtrees: HashMap<u64, Entry, Hashed>,
    values: HashMap<u64, (u64, Box<dyn Any>), Hashed>,
}

impl MemoCache {
    /// How many subtrees are kept.
    pub fn len(&self) -> usize {
        self.subtrees.len()
    }

    /// Whether nothing is kept.
    pub fn is_empty(&self) -> bool {
        self.subtrees.is_empty() && self.values.is_empty()
    }
}

impl std::fmt::Debug for MemoCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoCache")
            .field("subtrees", &self.subtrees.len())
            .field("values", &self.values.len())
            .finish()
    }
}

/// One kept subtree.
struct Entry {
    deps: u64,
    theme: Theme,
    size: Size,
    focused: bool,
    element: Element,
    /// How many of each handler had been registered when it started and
    /// ended (its ids are the ones between).
    start: Counts,
    end: Counts,
    reads: Reads,
}

/// How many handlers of each kind a build has registered so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Counts {
    taps: u32,
    keys: u32,
    raw_keys: u32,
    drags: u32,
    text_pos: u32,
    contexts: u32,
    drops: u32,
    drag_sources: u32,
    pointers: u32,
    scrolls: u32,
}

/// What a memoized closure read through the [`Cx`], and what it did that
/// rules out keeping it.
#[derive(Clone, Debug, Default)]
struct Reads {
    interactions: Vec<(ElementKey, Interaction)>,
    drops: Vec<(DropId, DropState)>,
    scrolls: Vec<(ScrollId, f64)>,
    window_info: Option<WindowInfo>,
    /// Memoized subtrees and derived values inside it (kept with it).
    nested: Vec<u64>,
    values: Vec<u64>,
    volatile: bool,
}

impl Reads {
    fn absorb(&mut self, inner: &Reads) {
        self.interactions.extend_from_slice(&inner.interactions);
        self.drops.extend_from_slice(&inner.drops);
        self.scrolls.extend_from_slice(&inner.scrolls);
        if inner.window_info.is_some() {
            self.window_info = inner.window_info;
        }
        self.nested.extend_from_slice(&inner.nested);
        self.values.extend_from_slice(&inner.values);
        self.volatile |= inner.volatile;
    }
}

/// The previous frame's handlers, to move kept subtrees' handlers from.
struct Previous<S> {
    taps: Vec<Option<super::TapFn<S>>>,
    keys: Vec<Option<super::KeyFn<S>>>,
    raw_keys: Vec<Option<super::RawKeyFn<S>>>,
    drags: Vec<Option<super::DragFn<S>>>,
    text_pos: Vec<Option<super::TextPosFn<S>>>,
    contexts: Vec<Option<super::ContextFn<S>>>,
    drops: Vec<Option<DropTarget<S>>>,
    drag_sources: Vec<Option<super::DragSourceFn<S>>>,
    pointers: Vec<Option<Slot<S>>>,
}

fn open<T>(v: Vec<T>) -> Vec<Option<T>> {
    v.into_iter().map(Some).collect()
}

impl<S> From<Handlers<S>> for Previous<S> {
    fn from(h: Handlers<S>) -> Self {
        Self {
            taps: open(h.taps),
            keys: open(h.keys),
            raw_keys: open(h.raw_keys),
            drags: open(h.drags),
            text_pos: open(h.text_pos),
            contexts: open(h.contexts),
            drops: open(h.drops),
            drag_sources: open(h.drag_sources),
            pointers: open(h.pointers.into_slots()),
        }
    }
}

/// Whether `prev[from..to]` are all still there.
fn present<T>(prev: &[Option<T>], from: u32, to: u32) -> bool {
    prev.get(from as usize..to as usize)
        .is_some_and(|r| r.iter().all(Option::is_some))
}

/// Move `prev[from..to]` onto the end of `into`.
fn take<T>(prev: &mut [Option<T>], from: u32, to: u32, into: &mut Vec<T>) {
    into.extend(
        prev[from as usize..to as usize]
            .iter_mut()
            .filter_map(Option::take),
    );
}

/// One build's memo bookkeeping, in its [`Cx`].
pub(super) struct MemoFrame<S> {
    cache: MemoCache,
    used: HashSet<u64, Hashed>,
    values_used: HashSet<u64, Hashed>,
    /// The previous frame's handlers, as handed over and once opened.
    prev: Option<Handlers<S>>,
    opened: Option<Previous<S>>,
    /// What each memoized closure being built read, innermost last.
    stack: RefCell<Vec<Reads>>,
}

impl<S> Default for MemoFrame<S> {
    fn default() -> Self {
        Self {
            cache: MemoCache::default(),
            used: HashSet::default(),
            values_used: HashSet::default(),
            prev: None,
            opened: None,
            stack: RefCell::new(Vec::new()),
        }
    }
}

impl<S> MemoFrame<S> {
    fn record(&self, f: impl FnOnce(&mut Reads)) {
        if let Some(top) = self.stack.borrow_mut().last_mut() {
            f(top);
        }
    }

    /// Note that a memoized closure being built read the states of `key`.
    pub(super) fn read_interaction(&self, key: ElementKey, states: Interaction) {
        self.record(|r| r.interactions.push((key, states)));
    }

    pub(super) fn read_drop(&self, id: DropId, state: DropState) {
        self.record(|r| r.drops.push((id, state)));
    }

    pub(super) fn read_scroll(&self, id: ScrollId, offset: f64) {
        self.record(|r| r.scrolls.push((id, offset)));
    }

    pub(super) fn read_window_info(&self, info: WindowInfo) {
        self.record(|r| r.window_info = Some(info));
    }
}

/// Keys here are hashes already: maps of them use them as they are.
#[derive(Clone, Copy, Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ b as u64;
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
}

type Hashed = std::hash::BuildHasherDefault<KeyHasher>;

/// A key or deps value, hashed.
fn hash_of(salt: u32, value: impl Hash) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    salt.hash(&mut h);
    value.hash(&mut h);
    h.finish()
}

impl<S> Cx<'_, S> {
    /// A part of the view built again only when `deps` change: the element
    /// `build` returns is kept under `key`, and while `deps` hash the same as
    /// when it was built, later builds return it without calling `build`.
    ///
    /// `build` gets this [`Cx`], so the subtree can have handlers, focus,
    /// scroll containers and drop targets: when it is reused, the handlers it
    /// registered are carried over with it, under the same ids. Give `deps`
    /// everything `build` uses that can change (the closure's captures, the
    /// state it reads); the theme, the window's size and focus, and what it
    /// reads through the `Cx` ([`interaction`](Cx::interaction),
    /// [`drop_state`](Cx::drop_state), [`scroll_offset`](Cx::scroll_offset),
    /// [`window_info`](Cx::window_info)) are tracked for it. Its handlers too
    /// are the ones built with those deps, so they must not capture anything
    /// else that changes.
    ///
    /// A kept subtree is reused only where its ids come out the same: when
    /// everything built before it registered as many handlers of each kind as
    /// last time (else it is built again). A subtree that asks for frames
    /// ([`request_frame`](Cx::request_frame), [`animate`](Cx::animate)),
    /// declares overlays or context menus, or sets the title is built every
    /// time; [`frame_time`](Cx::frame_time) isn't tracked. Keys are per
    /// window and must be unique within a build (a second use of a key in
    /// the same build is built afresh). Kept subtrees not used in a build are
    /// dropped. See `docs/performance.md`.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct Todo { items: Vec<String>, done: usize }
    /// fn view(s: &Todo, cx: &mut Cx<Todo>) -> Element {
    ///     let theme = *cx.theme();
    ///     let rows = s.items.iter().enumerate().map(|(i, item)| {
    ///         // Built again only when this row's text changes.
    ///         cx.memo(("row", i), item, |cx| {
    ///             label(&theme, item.as_str()).on_tap(cx, move |s: &mut Todo| s.done = i)
    ///         })
    ///     });
    ///     column(rows.collect())
    /// }
    /// # let theme = Theme::light();
    /// # let mut cx = Cx::new(&theme);
    /// # let _ = view(&Todo { items: vec!["a".into()], done: 0 }, &mut cx);
    /// ```
    pub fn memo(
        &mut self,
        key: impl Hash,
        deps: impl Hash,
        build: impl FnOnce(&mut Self) -> Element,
    ) -> Element {
        let key = hash_of(0x6d65_6d6f, key);
        let deps = hash_of(0x6465_7073, deps);
        // A second use of the key this build: built afresh, not kept.
        let first = self.memo.used.insert(key);
        let start = self.counts();
        self.memo.record(|r| r.nested.push(key));
        if first && let Some(element) = self.reuse(key, deps, start) {
            return element;
        }
        let before = (
            self.frame.requests,
            self.overlays.len(),
            self.context_menus.len(),
            self.window_title.is_some(),
        );
        self.memo.stack.borrow_mut().push(Reads::default());
        let mut element = build(self);
        let mut reads = self.memo.stack.borrow_mut().pop().unwrap_or_default();
        let after = (
            self.frame.requests,
            self.overlays.len(),
            self.context_menus.len(),
            self.window_title.is_some(),
        );
        reads.volatile |= before != after;
        self.memo.record(|r| r.absorb(&reads));
        if first {
            if reads.volatile {
                self.memo.cache.subtrees.remove(&key);
            } else {
                MemoMark::stamp(&mut element);
                let entry = Entry {
                    deps,
                    theme: *self.theme,
                    size: self.window_size,
                    focused: self.window_focused,
                    element: element.clone(),
                    start,
                    end: self.counts(),
                    reads,
                };
                self.memo.cache.subtrees.insert(key, entry);
            }
        }
        element
    }

    /// A value computed again only when `deps` change: `compute`'s result is
    /// kept under `key`, and returned (cloned) while `deps` hash the same.
    /// For something costly derived from the state — a sorted, filtered
    /// list, totals, a parsed document — that the view needs on every build.
    /// Wrap a large value in an `Rc` to make the clone cheap. Values not
    /// asked for in a build are dropped.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct Shop { cents: Vec<u64> }
    /// fn view(s: &Shop, cx: &mut Cx<Shop>) -> Element {
    ///     let theme = *cx.theme();
    ///     let total = cx.derive("total", &s.cents, || s.cents.iter().sum::<u64>());
    ///     label(&theme, format!("Total: {}.{:02}", total / 100, total % 100))
    /// }
    /// # let theme = Theme::light();
    /// # let mut cx = Cx::new(&theme);
    /// # let _ = view(&Shop { cents: vec![100, 250] }, &mut cx);
    /// ```
    pub fn derive<T: Clone + 'static>(
        &mut self,
        key: impl Hash,
        deps: impl Hash,
        compute: impl FnOnce() -> T,
    ) -> T {
        let key = hash_of(0x7661_6c75, key);
        let deps = hash_of(0x6465_7073, deps);
        self.memo.values_used.insert(key);
        self.memo.record(|r| r.values.push(key));
        if let Some((d, value)) = self.memo.cache.values.get(&key)
            && *d == deps
            && let Some(value) = value.downcast_ref::<T>()
        {
            return value.clone();
        }
        let value = compute();
        self.memo
            .cache
            .values
            .insert(key, (deps, Box::new(value.clone())));
        value
    }

    /// How many handlers of each kind have been registered so far.
    fn counts(&self) -> Counts {
        Counts {
            taps: self.taps.len() as u32,
            keys: self.keys.len() as u32,
            raw_keys: self.raw_keys.len() as u32,
            drags: self.drags.len() as u32,
            text_pos: self.text_pos.len() as u32,
            contexts: self.contexts.len() as u32,
            drops: self.drops.len() as u32,
            drag_sources: self.drag_sources.len() as u32,
            pointers: self.pointers.len() as u32,
            scrolls: self.next_scroll,
        }
    }

    /// The subtree kept under `key`, with its handlers moved over, if it was
    /// built with `deps` from the same inputs and its ids start at `start`.
    fn reuse(&mut self, key: u64, deps: u64, start: Counts) -> Option<Element> {
        let entry = self.memo.cache.subtrees.get(&key)?;
        let same = entry.deps == deps
            && entry.start == start
            && entry.theme == *self.theme
            && entry.size == self.window_size
            && entry.focused == self.window_focused
            && entry
                .reads
                .interactions
                .iter()
                .all(|&(k, v)| self.interactions.get(&k).copied().unwrap_or_default() == v)
            && entry
                .reads
                .drops
                .iter()
                .all(|&(id, v)| self.drop_states.get(&id).copied().unwrap_or_default() == v)
            && entry
                .reads
                .scrolls
                .iter()
                .all(|&(id, v)| self.raw_scroll_offset(id) == v)
            && entry
                .reads
                .window_info
                .is_none_or(|info| info == self.window_info);
        if !same {
            return None;
        }
        // Its handlers, from the previous frame.
        if self.memo.opened.is_none() {
            let prev = self.memo.prev.take().unwrap_or_default();
            self.memo.opened = Some(prev.into());
        }
        let prev = self.memo.opened.as_mut()?;
        let (a, b) = (entry.start, entry.end);
        let there = present(&prev.taps, a.taps, b.taps)
            && present(&prev.keys, a.keys, b.keys)
            && present(&prev.raw_keys, a.raw_keys, b.raw_keys)
            && present(&prev.drags, a.drags, b.drags)
            && present(&prev.text_pos, a.text_pos, b.text_pos)
            && present(&prev.contexts, a.contexts, b.contexts)
            && present(&prev.drops, a.drops, b.drops)
            && present(&prev.drag_sources, a.drag_sources, b.drag_sources)
            && present(&prev.pointers, a.pointers, b.pointers);
        if !there {
            return None;
        }
        take(&mut prev.taps, a.taps, b.taps, &mut self.taps);
        take(&mut prev.keys, a.keys, b.keys, &mut self.keys);
        take(
            &mut prev.raw_keys,
            a.raw_keys,
            b.raw_keys,
            &mut self.raw_keys,
        );
        take(&mut prev.drags, a.drags, b.drags, &mut self.drags);
        take(
            &mut prev.text_pos,
            a.text_pos,
            b.text_pos,
            &mut self.text_pos,
        );
        take(
            &mut prev.contexts,
            a.contexts,
            b.contexts,
            &mut self.contexts,
        );
        take(&mut prev.drops, a.drops, b.drops, &mut self.drops);
        let sources = &mut self.drag_sources;
        take(
            &mut prev.drag_sources,
            a.drag_sources,
            b.drag_sources,
            sources,
        );
        let mut slots = Vec::new();
        take(&mut prev.pointers, a.pointers, b.pointers, &mut slots);
        for slot in slots {
            self.pointers.push_slot(slot);
        }
        self.next_scroll = b.scrolls;
        // What it read still counts as read: the app rebuilds when it changes.
        for &(k, _) in &entry.reads.interactions {
            if !self.interaction_queried.contains(&k) {
                self.interaction_queried.push(k);
            }
        }
        for &(id, _) in &entry.reads.drops {
            if !self.drop_queried.contains(&id) {
                self.drop_queried.push(id);
            }
        }
        if entry.reads.window_info.is_some() {
            self.window_info_read = true;
        }
        // What is kept inside it stays kept.
        self.memo.used.extend(entry.reads.nested.iter().copied());
        self.memo
            .values_used
            .extend(entry.reads.values.iter().copied());
        let reads = entry.reads.clone();
        let element = entry.element.clone();
        self.memo.record(|r| r.absorb(&reads));
        Some(element)
    }

    /// Seed the memo cache from the previous build (see [`Cx::memo`]).
    pub fn set_memo_cache(&mut self, cache: MemoCache) {
        self.memo.cache = cache;
        self.memo.used.clear();
        self.memo.values_used.clear();
    }

    /// Hand over the previous build's handlers, for reused memoized subtrees
    /// to take theirs from (the app does this before each build). The ones
    /// not taken are dropped with this `Cx`.
    pub fn set_previous_handlers(&mut self, handlers: Handlers<S>) {
        self.memo.prev = Some(handlers);
        self.memo.opened = None;
    }

    /// Take the memo cache back, dropping what this build didn't use.
    pub fn take_memo_cache(&mut self) -> MemoCache {
        let mut cache = std::mem::take(&mut self.memo.cache);
        let used = std::mem::take(&mut self.memo.used);
        let values = std::mem::take(&mut self.memo.values_used);
        cache.subtrees.retain(|k, _| used.contains(k));
        cache.values.retain(|k, _| values.contains(k));
        cache
    }
}

/// What marks the root of a memoized subtree. Shared by every reuse of one
/// build of it, it remembers how the subtree was measured and laid out, so a
/// reused subtree that lowers to the same elements as last time (no state
/// override or transition inside it changed anything) isn't laid out again.
#[derive(Clone, Default)]
pub(crate) struct MemoMark(Rc<Laid>);

#[derive(Default)]
struct Laid {
    /// The subtree as last lowered (without this mark), and what was
    /// measured and laid out from it.
    lowered: RefCell<Option<Element>>,
    measures: RefCell<Vec<(Size, Size)>>,
    layout: RefCell<Option<(LayoutDirection, LayoutNode)>>,
}

impl PartialEq for MemoMark {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for MemoMark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MemoMark")
    }
}

impl MemoMark {
    /// The mark on `el`, if it is the root of a memoized subtree.
    pub(crate) fn of(el: &Element) -> Option<&MemoMark> {
        el.reactive.as_ref().and_then(|r| r.memo.as_ref())
    }

    /// Mark `el` as the root of a new build of a memoized subtree.
    fn stamp(el: &mut Element) {
        el.reactive.get_or_insert_with(Default::default).memo = Some(MemoMark::default());
    }

    /// `el` (marked with this) was just lowered: keep what was measured and
    /// laid out from it only if it lowered to the same elements as last time.
    pub(crate) fn lowered(&self, el: &mut Element) {
        let mark = el.reactive.as_mut().and_then(|r| r.memo.take());
        let mut lowered = self.0.lowered.borrow_mut();
        if lowered.as_ref() != Some(el) {
            *lowered = Some(el.clone());
            self.0.measures.borrow_mut().clear();
            *self.0.layout.borrow_mut() = None;
        }
        if let Some(r) = el.reactive.as_mut() {
            r.memo = mark;
        }
    }

    /// Its size when measured in `avail` before, if it was.
    pub(crate) fn measured(&self, avail: Size) -> Option<Size> {
        let measures = self.0.measures.borrow();
        measures.iter().find(|(a, _)| *a == avail).map(|(_, s)| *s)
    }

    /// Remember it measures `size` in `avail`.
    pub(crate) fn remember_measure(&self, avail: Size, size: Size) {
        if self.0.lowered.borrow().is_some() {
            self.0.measures.borrow_mut().push((avail, size));
        }
    }

    /// The node it was laid out to before, moved to `bounds`, if it was laid
    /// out at that size in `direction`.
    pub(crate) fn laid_out(&self, bounds: Rect, direction: LayoutDirection) -> Option<LayoutNode> {
        let layout = self.0.layout.borrow();
        let (dir, node) = layout.as_ref()?;
        if *dir != direction || node.bounds.size != bounds.size {
            return None;
        }
        let mut node = node.clone();
        let by = bounds.origin - node.bounds.origin;
        if by != Vec2::ZERO {
            crate::core::render::translate(&mut node, by);
        }
        Some(node)
    }

    /// Remember it was laid out to `node` in `direction`.
    pub(crate) fn remember_layout(&self, direction: LayoutDirection, node: &LayoutNode) {
        if self.0.lowered.borrow().is_some() {
            *self.0.layout.borrow_mut() = Some((direction, node.clone()));
        }
    }
}
