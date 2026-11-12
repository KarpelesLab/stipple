//! Virtualized lists: a scrolling list that builds only the rows in (and just
//! around) its viewport.
//!
//! [`virtual_list`] takes a row count, row heights (one fixed height, or one
//! per row) and a row-builder closure, and builds a scroll container whose
//! content is a spacer for the rows above the viewport, the visible rows, and
//! a spacer for the rows below — so the scroll extent is that of the full
//! content while a 10,000-row list builds about twenty rows. The scroll offset
//! comes from [`Cx::scroll_offset`]; wheel scrolling rebuilds the frame, which
//! builds the newly visible rows.
//!
//! Selection lives in a [`ListState`] the app keeps in its state. Clicking a
//! row selects it (Shift+click extends, Ctrl/Cmd+click toggles, in
//! [`SelectionMode::Multi`]); the arrow keys, Home and End move the cursor
//! (with Shift they extend the selection), Ctrl/Cmd+A selects all and Space
//! toggles the cursor row. Moving the cursor scrolls it into view through
//! [`Cx::scroll_requests`].
//!
//! [`table`](crate::widgets::table) and [`tree`](crate::widgets::tree) are
//! built on the same engine.

use std::collections::BTreeSet;
use std::ops::Range;
use std::rc::Rc;

use crate::core::{
    Align, Axis, BoxStyle, Cx, Element, KeyInput, Role, ScrollId, ScrollRequests, pointer,
};
use crate::geometry::Insets;
use crate::platform::Modifiers;
use crate::render::Color;
use crate::style::Theme;

/// Rows built beyond each edge of the viewport by default, so a small scroll
/// shows already-built rows on its first frame.
pub const DEFAULT_OVERSCAN: usize = 2;

/// How the rows of a virtualized list are sized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowHeight<'a> {
    /// Every row is this tall, in logical pixels.
    Fixed(f64),
    /// Row `i` is `heights[i]` tall; the list has `heights.len()` rows.
    Variable(&'a [f64]),
}

/// Row geometry: where each row starts. Cheap to clone (handlers capture it).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Rows {
    count: usize,
    fixed: f64,
    /// For variable heights, the top of each row plus the total (`count + 1`
    /// entries).
    tops: Option<Rc<[f64]>>,
}

impl Rows {
    pub(crate) fn new(count: usize, height: RowHeight<'_>) -> Self {
        match height {
            RowHeight::Fixed(h) => Rows {
                count,
                fixed: h.max(1.0),
                tops: None,
            },
            RowHeight::Variable(heights) => {
                let mut tops = Vec::with_capacity(heights.len() + 1);
                let mut y = 0.0;
                tops.push(y);
                for h in heights {
                    y += h.max(0.0);
                    tops.push(y);
                }
                Rows {
                    count: heights.len(),
                    fixed: 1.0,
                    tops: Some(tops.into()),
                }
            }
        }
    }

    pub(crate) fn count(&self) -> usize {
        self.count
    }

    /// The top of row `i` (or the total height for `i == count`).
    pub(crate) fn top(&self, i: usize) -> f64 {
        let i = i.min(self.count);
        match &self.tops {
            Some(tops) => tops[i],
            None => i as f64 * self.fixed,
        }
    }

    pub(crate) fn height(&self, i: usize) -> f64 {
        self.top(i + 1) - self.top(i)
    }

    pub(crate) fn total(&self) -> f64 {
        self.top(self.count)
    }

    /// The row containing `y` (clamped to the last row). `count` must be > 0.
    fn index_at(&self, y: f64) -> usize {
        let last = self.count - 1;
        match &self.tops {
            Some(tops) => tops[1..=self.count]
                .partition_point(|&bottom| bottom <= y)
                .min(last),
            None => ((y.max(0.0) / self.fixed) as usize).min(last),
        }
    }

    /// The rows to build for a viewport of `height` at `offset`, plus
    /// `overscan` rows each side.
    pub(crate) fn window(&self, offset: f64, height: f64, overscan: usize) -> Range<usize> {
        if self.count == 0 {
            return 0..0;
        }
        let first = self.index_at(offset).saturating_sub(overscan);
        let last = (self.index_at(offset + height.max(0.0)) + 1 + overscan).min(self.count);
        first..last
    }

    /// The largest valid offset for a viewport of `height`.
    pub(crate) fn max_offset(&self, height: f64) -> f64 {
        (self.total() - height).max(0.0)
    }

    /// The offset that brings row `i` fully into a viewport of `height`
    /// currently at `offset`, moving as little as possible.
    pub(crate) fn reveal(&self, i: usize, offset: f64, height: f64) -> f64 {
        let (top, bottom) = (self.top(i), self.top(i + 1));
        if top < offset {
            top
        } else if bottom > offset + height {
            (bottom - height).max(0.0)
        } else {
            offset
        }
    }
}

/// A scroll viewport over [`Rows`] for this frame: its container id, the
/// clamped offset, and which rows to build.
#[derive(Clone, Debug)]
pub(crate) struct Window {
    pub(crate) id: ScrollId,
    pub(crate) offset: f64,
    pub(crate) height: f64,
    pub(crate) rows: Rows,
    pub(crate) range: Range<usize>,
    pub(crate) requests: ScrollRequests,
}

impl Window {
    /// Register the scroll container and work out which rows are visible.
    pub(crate) fn new<S>(cx: &mut Cx<S>, rows: Rows, height: f64, overscan: usize) -> Self {
        let id = cx.register_scroll();
        let offset = cx.scroll_offset(id).min(rows.max_offset(height));
        let range = rows.window(offset, height, overscan);
        Window {
            id,
            offset,
            height,
            range,
            rows,
            requests: cx.scroll_requests(),
        }
    }

    /// Scroll so row `i` is fully visible (a no-op if it already is).
    pub(crate) fn reveal(&self, i: usize) {
        if i >= self.rows.count() {
            return;
        }
        let to = self.rows.reveal(i, self.offset, self.height);
        if (to - self.offset).abs() > f64::EPSILON {
            self.requests.scroll_to(self.id, to);
        }
    }

    /// The scroll container holding the built `rows` (in `range` order) between
    /// spacers that stand in for the rows that weren't built.
    pub(crate) fn assemble(&self, rows: Vec<Element>) -> Element {
        let above = self.rows.top(self.range.start);
        let below = self.rows.total() - self.rows.top(self.range.end);
        let mut children = Vec::with_capacity(rows.len() + 2);
        children.push(Element::boxed(BoxStyle::default()).height(above));
        children.extend(rows);
        children.push(Element::boxed(BoxStyle::default()).height(below));
        let content = Element::stack(Axis::Vertical, children).align(Align::Start, Align::Stretch);
        Element::stack(Axis::Vertical, vec![content])
            .height(self.height)
            .scrollable(self.id)
            .align(Align::Start, Align::Stretch)
    }
}

/// How many rows a [`ListState`] lets the user select.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionMode {
    /// No selection; the keyboard cursor still moves.
    None,
    /// At most one row.
    #[default]
    Single,
    /// Any set of rows (Shift extends a range, Ctrl/Cmd toggles a row).
    Multi,
}

/// The selection and keyboard cursor of a list or table, kept in the app
/// state. Rows are addressed by index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListState {
    mode: SelectionMode,
    cursor: Option<usize>,
    /// Where a Shift-extended range starts.
    anchor: Option<usize>,
    selected: BTreeSet<usize>,
}

impl ListState {
    /// An empty selection in `mode`.
    pub fn new(mode: SelectionMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    /// The keyboard cursor: the row the arrow keys move from.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// Move the cursor without changing the selection.
    pub fn set_cursor(&mut self, row: Option<usize>) {
        self.cursor = row;
    }

    pub fn is_selected(&self, row: usize) -> bool {
        self.selected.contains(&row)
    }

    /// The selected rows, in ascending order.
    pub fn selected(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected.iter().copied()
    }

    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    /// Deselect everything (the cursor stays).
    pub fn clear(&mut self) {
        self.selected.clear();
    }

    /// Select just `row`, moving the cursor and range anchor there.
    pub fn select(&mut self, row: usize) {
        self.cursor = Some(row);
        self.anchor = Some(row);
        self.selected.clear();
        if self.mode != SelectionMode::None {
            self.selected.insert(row);
        }
    }

    /// Flip `row`'s selection (Ctrl/Cmd+click), moving the cursor and anchor
    /// there. Outside [`SelectionMode::Multi`] it selects `row`.
    pub fn toggle(&mut self, row: usize) {
        if self.mode != SelectionMode::Multi {
            return self.select(row);
        }
        self.cursor = Some(row);
        self.anchor = Some(row);
        if !self.selected.remove(&row) {
            self.selected.insert(row);
        }
    }

    /// Select the range from the anchor to `row` (Shift+click), moving the
    /// cursor there. Outside [`SelectionMode::Multi`] it selects `row`.
    pub fn extend_to(&mut self, row: usize) {
        if self.mode != SelectionMode::Multi {
            return self.select(row);
        }
        let anchor = self.anchor.unwrap_or(row);
        self.anchor = Some(anchor);
        self.cursor = Some(row);
        self.selected = (anchor.min(row)..=anchor.max(row)).collect();
    }

    /// Select all `count` rows (multi-selection only).
    pub fn select_all(&mut self, count: usize) {
        if self.mode == SelectionMode::Multi {
            self.selected = (0..count).collect();
        }
    }

    /// Forget rows at or past `count` (after the list shrank).
    pub fn truncate(&mut self, count: usize) {
        self.selected.retain(|&r| r < count);
        let clamp = |r: Option<usize>| r.filter(|&r| r < count);
        self.cursor = clamp(self.cursor);
        self.anchor = clamp(self.anchor);
    }

    /// Apply a click on `row` with the `modifiers` held: Shift extends,
    /// Ctrl/Cmd toggles, otherwise select just that row.
    pub fn click(&mut self, row: usize, modifiers: Modifiers) {
        if modifiers.shift {
            self.extend_to(row);
        } else if modifiers.ctrl || modifiers.meta {
            self.toggle(row);
        } else {
            self.select(row);
        }
    }

    /// Apply a navigation key for a list of `count` rows. Returns `true` if
    /// the key was handled.
    pub fn handle_key(&mut self, key: &KeyInput, count: usize) -> bool {
        if count == 0 {
            return false;
        }
        let last = count - 1;
        let at = self.cursor.map(|c| c.min(last));
        let target = |up: bool| match at {
            None => 0,
            Some(c) if up => c.saturating_sub(1),
            Some(c) => (c + 1).min(last),
        };
        match key {
            KeyInput::Up => self.select(target(true)),
            KeyInput::Down => self.select(target(false)),
            KeyInput::Home => self.select(0),
            KeyInput::End => self.select(last),
            KeyInput::SelectUp => self.extend_to(target(true)),
            KeyInput::SelectDown => self.extend_to(target(false)),
            KeyInput::SelectHome => self.extend_to(0),
            KeyInput::SelectEnd => self.extend_to(last),
            KeyInput::SelectAll => self.select_all(count),
            KeyInput::Text(t) if t == " " => match at {
                Some(c) => self.toggle(c),
                None => self.select(0),
            },
            KeyInput::Escape if self.mode == SelectionMode::Multi => self.clear(),
            _ => return false,
        }
        true
    }
}

/// What a row builder is told about the row it builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowInfo {
    /// The row's index.
    pub index: usize,
    pub selected: bool,
    /// Whether the keyboard cursor is on this row.
    pub cursor: bool,
}

/// The shape of a [`virtual_list`]: how many rows, how tall, and how tall the
/// viewport is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualList<'a> {
    pub rows: usize,
    pub row_height: RowHeight<'a>,
    /// The viewport height, in logical pixels.
    pub height: f64,
    /// Rows built beyond each edge of the viewport.
    pub overscan: usize,
}

impl<'a> VirtualList<'a> {
    /// `rows` rows of `row_height` each, in a 240px viewport.
    pub fn fixed(rows: usize, row_height: f64) -> Self {
        Self {
            rows,
            row_height: RowHeight::Fixed(row_height),
            height: 240.0,
            overscan: DEFAULT_OVERSCAN,
        }
    }

    /// One row per entry of `heights`, in a 240px viewport.
    pub fn variable(heights: &'a [f64]) -> Self {
        Self {
            rows: heights.len(),
            row_height: RowHeight::Variable(heights),
            height: 240.0,
            overscan: DEFAULT_OVERSCAN,
        }
    }

    /// Set the viewport height.
    pub fn height(mut self, height: f64) -> Self {
        self.height = height;
        self
    }

    /// Set how many rows are built beyond each edge of the viewport.
    pub fn overscan(mut self, rows: usize) -> Self {
        self.overscan = rows;
        self
    }

    pub(crate) fn geometry(&self) -> Rows {
        match self.row_height {
            RowHeight::Fixed(h) => Rows::new(self.rows, RowHeight::Fixed(h)),
            RowHeight::Variable(hs) => Rows::new(hs.len(), RowHeight::Variable(hs)),
        }
    }
}

/// The fill for a row: the selection tint, a faint stripe on odd rows (if
/// `striped`), or nothing.
pub(crate) fn row_fill(theme: &Theme, selected: bool, striped_odd: bool) -> Option<Color> {
    if selected {
        Some(theme.palette.selection)
    } else if striped_odd {
        let o = theme.palette.hover_overlay;
        Some(Color::rgba(o.r, o.g, o.b, o.a / 2))
    } else {
        None
    }
}

/// The frame around a list, table or tree: the surface, a hairline border,
/// and a 1px inset so rows don't paint over the border.
pub(crate) fn frame(theme: &Theme, children: Vec<Element>) -> Element {
    Element::stack(Axis::Vertical, children)
        .fill(theme.palette.surface)
        .border(theme.palette.border, 1.0)
        .radius(theme.radius / 2.0)
        .padding(Insets::uniform(1.0))
        .align(Align::Start, Align::Stretch)
        .clip()
}

/// A virtualized, selectable list of `spec.rows` rows: only the rows in (and
/// [`overscan`](VirtualList::overscan) rows around) the viewport are built,
/// by `row`, while the scroll extent covers them all.
///
/// `state` is the list's [`ListState`] and `field` the accessor that reaches it
/// inside the app state, as with [`text_editor`](crate::widgets::text_editor).
/// The row builder gets the [`Cx`] (rows can hold their own controls) and a
/// [`RowInfo`]; the list adds the selection tint, the click handling and the
/// row's height. The list takes keyboard focus as a whole.
///
/// ```
/// use stipple::prelude::*;
/// use stipple::widgets::{ListState, SelectionMode, VirtualList, virtual_list};
///
/// struct App { list: ListState }
/// let theme = Theme::light();
/// let mut cx = Cx::new(&theme);
/// let state = App { list: ListState::new(SelectionMode::Multi) };
/// let list = virtual_list(
///     &mut cx,
///     &theme,
///     &state.list,
///     |s: &mut App| &mut s.list,
///     VirtualList::fixed(10_000, 24.0).height(240.0),
///     |_cx, row| label(&theme, format!("Row {}", row.index)),
/// );
/// # let _ = list;
/// ```
pub fn virtual_list<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    state: &ListState,
    field: impl Fn(&mut S) -> &mut ListState + Copy + 'static,
    spec: VirtualList<'_>,
    mut row: impl FnMut(&mut Cx<S>, RowInfo) -> Element,
) -> Element {
    let window = Window::new(cx, spec.geometry(), spec.height, spec.overscan);
    let multi = state.mode() != SelectionMode::Single;
    let mut rows = Vec::with_capacity(window.range.len());
    for index in window.range.clone() {
        let info = RowInfo {
            index,
            selected: state.is_selected(index),
            cursor: state.cursor() == Some(index),
        };
        let content = row(cx, info);
        let mut el = Element::stack(Axis::Horizontal, vec![content])
            .height(window.rows.height(index))
            .padding(Insets::symmetric(theme.spacing.md, 0.0))
            .align(Align::Start, Align::Center);
        if let Some(fill) = row_fill(theme, info.selected, false) {
            el = el.fill(fill);
        }
        // In multi-selection the cursor can sit on an unselected row; mark it.
        if multi && info.cursor {
            el = el.border(theme.palette.focus_ring, 1.0);
        }
        let w = window.clone();
        rows.push(
            el.role(Role::ListItem)
                .selected(info.selected)
                .on_tap(cx, move |s: &mut S| {
                    field(s).click(index, pointer().modifiers);
                    w.reveal(index);
                }),
        );
    }
    let count = window.rows.count();
    let body = window.assemble(rows);
    frame(theme, vec![body])
        .role(Role::List)
        .on_key(cx, move |s: &mut S, key| {
            let st = field(s);
            let before = st.cursor();
            if st.handle_key(key, count)
                && let Some(c) = st.cursor()
                && Some(c) != before
            {
                window.reveal(c);
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{AccessNode, layout};
    use crate::geometry::Rect;
    use std::cell::Cell;
    use std::collections::HashMap;

    struct App {
        list: ListState,
    }

    fn count_elements(el: &Element) -> usize {
        use crate::core::ElementKind;
        1 + match &el.kind {
            ElementKind::Stack { children, .. }
            | ElementKind::Grid { children, .. }
            | ElementKind::Layers { children, .. } => children.iter().map(count_elements).sum(),
            _ => 0,
        }
    }

    /// Build a 10,000-row list scrolled to `offset`, returning the element,
    /// how many rows were built, and the lowest/highest index built.
    fn build_at(offset: f64) -> (Element, usize, usize, usize) {
        let theme = Theme::light();
        let mut cx = Cx::<App>::new(&theme);
        let mut offsets = HashMap::new();
        offsets.insert(ScrollId(0), offset);
        cx.set_scroll_state(offsets, ScrollRequests::default());
        let state = App {
            list: ListState::new(SelectionMode::Multi),
        };
        let built = Cell::new(0usize);
        let lo = Cell::new(usize::MAX);
        let hi = Cell::new(0usize);
        let el = virtual_list(
            &mut cx,
            &theme,
            &state.list,
            |s: &mut App| &mut s.list,
            VirtualList::fixed(10_000, 24.0).height(240.0),
            |_cx, row| {
                built.set(built.get() + 1);
                lo.set(lo.get().min(row.index));
                hi.set(hi.get().max(row.index));
                Element::text(format!("Row {}", row.index), 14.0, Color::BLACK)
            },
        );
        (el, built.get(), lo.get(), hi.get())
    }

    #[test]
    fn ten_thousand_rows_build_only_the_visible_ones() {
        // 240px / 24px = 10 rows visible, plus the overscan each side.
        let (el, built, lo, hi) = build_at(0.0);
        assert_eq!((lo, hi), (0, 10 + DEFAULT_OVERSCAN));
        assert!(built <= 10 + 2 * DEFAULT_OVERSCAN + 1, "built {built} rows");
        // The whole tree stays O(visible): a handful of elements per row.
        let total = count_elements(&el);
        assert!(total < 100, "{total} elements for 10,000 rows");

        // Scrolled to the middle: the rows around row 5,000, still a handful.
        let (el, built, lo, hi) = build_at(5_000.0 * 24.0);
        assert_eq!(lo, 5_000 - DEFAULT_OVERSCAN);
        assert_eq!(hi, 5_000 + 10 + DEFAULT_OVERSCAN);
        assert!(built <= 10 + 2 * DEFAULT_OVERSCAN + 1);
        assert!(count_elements(&el) < 100);

        // Past the end clamps to the last page.
        let (_, _, lo, hi) = build_at(1e9);
        assert_eq!(hi, 9_999);
        assert_eq!(lo, 10_000 - 10 - DEFAULT_OVERSCAN);
    }

    #[test]
    fn the_scroll_extent_covers_every_row() {
        let (el, ..) = build_at(1_000.0);
        let tree = layout(&el, Rect::from_xywh(0.0, 0.0, 300.0, 400.0), None);
        let scroller = crate::core::find_scroll(&tree, ScrollId(0)).unwrap();
        assert_eq!(scroller.bounds.height(), 240.0);
        let content = &scroller.children[0];
        assert_eq!(content.bounds.height(), 10_000.0 * 24.0);
        // The first built row sits where row (1000/24 - overscan) belongs.
        let first = (1_000.0 / 24.0) as usize - DEFAULT_OVERSCAN;
        assert_eq!(
            content.children[1].bounds.min_y() - content.bounds.min_y(),
            first as f64 * 24.0
        );
    }

    #[test]
    fn variable_heights_window_and_reveal() {
        let heights: Vec<f64> = (0..100)
            .map(|i| if i % 2 == 0 { 20.0 } else { 40.0 })
            .collect();
        let rows = Rows::new(0, RowHeight::Variable(&heights));
        assert_eq!(rows.count(), 100);
        assert_eq!(rows.total(), 50.0 * 60.0);
        assert_eq!(rows.top(3), 20.0 + 40.0 + 20.0);
        // y = 70 falls in row 2 (60..80); 0 overscan.
        assert_eq!(rows.window(70.0, 0.0, 0), 2..3);
        assert_eq!(rows.window(0.0, 100.0, 0), 0..4);
        // Revealing row 10 (top 300) from offset 0 in a 100px viewport.
        assert_eq!(rows.reveal(10, 0.0, 100.0), 320.0 - 100.0);
        assert_eq!(rows.reveal(0, 200.0, 100.0), 0.0);
    }

    #[test]
    fn selection_keys_and_modifier_clicks() {
        let mut s = ListState::new(SelectionMode::Multi);
        s.click(3, Modifiers::default());
        s.click(
            6,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(s.selected().collect::<Vec<_>>(), vec![3, 4, 5, 6]);
        s.click(
            4,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(s.selected().collect::<Vec<_>>(), vec![3, 5, 6]);
        assert!(s.handle_key(&KeyInput::Down, 10));
        assert_eq!(s.selected().collect::<Vec<_>>(), vec![5]);
        assert!(s.handle_key(&KeyInput::SelectDown, 10));
        assert!(s.handle_key(&KeyInput::SelectDown, 10));
        assert_eq!(s.selected().collect::<Vec<_>>(), vec![5, 6, 7]);
        assert!(s.handle_key(&KeyInput::End, 10));
        assert_eq!(s.cursor(), Some(9));
        assert!(s.handle_key(&KeyInput::Down, 10));
        assert_eq!(s.cursor(), Some(9), "stays on the last row");
        assert!(s.handle_key(&KeyInput::SelectAll, 10));
        assert_eq!(s.selected_count(), 10);
        assert!(!s.handle_key(&KeyInput::Left, 10));

        let mut single = ListState::new(SelectionMode::Single);
        single.click(
            2,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        );
        single.click(
            5,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(single.selected().collect::<Vec<_>>(), vec![5]);
    }

    fn list_app() -> crate::App<App> {
        crate::App::new(
            App {
                list: ListState::new(SelectionMode::Multi),
            },
            |state: &App, cx: &mut Cx<App>| {
                let theme = *cx.theme();
                let list = virtual_list(
                    cx,
                    &theme,
                    &state.list,
                    |s: &mut App| &mut s.list,
                    VirtualList::fixed(10_000, 24.0).height(240.0),
                    |_cx, row| Element::text(format!("Row {}", row.index), 14.0, Color::BLACK),
                );
                // Under a container: the root is reported as the window.
                crate::widgets::column(vec![list.width(300.0)])
            },
        )
        .logical_size(crate::geometry::Size::new(320.0, 300.0))
    }

    fn items(a: &AccessNode) -> Vec<&AccessNode> {
        a.descendants()
            .into_iter()
            .filter(|n| n.role == Role::ListItem)
            .collect()
    }

    #[test]
    fn keyboard_scrolls_the_cursor_into_view_and_wheel_builds_new_rows() {
        let mut app = list_app();
        app.render_once();
        // Click the second row: it is selected and the list takes focus.
        assert!(app.click_at(crate::geometry::Point::new(50.0, 1.0 + 24.0 + 12.0)));
        assert!(app.focused().is_some());
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        let list = a.descendants().into_iter().find(|n| n.role == Role::List);
        assert!(list.is_some_and(|l| l.focused));
        assert!(items(&a).iter().any(|n| n.name == "Row 1" && n.selected));

        // End jumps to the last row and scrolls it into view.
        assert!(app.press_key(KeyInput::End));
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        let last = items(&a)
            .into_iter()
            .find(|n| n.name == "Row 9999")
            .unwrap();
        assert!(last.selected);
        assert!(
            last.bounds.max_y() <= 1.0 + 240.0 + 0.5,
            "{:?}",
            last.bounds
        );
        assert!(items(&a).len() < 20);

        // Home goes back; the wheel then scrolls into rows never built before.
        assert!(app.press_key(KeyInput::Home));
        app.render_once();
        app.hover_at(crate::geometry::Point::new(50.0, 100.0));
        assert!(app.scroll_by(24.0 * 500.0));
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        assert!(items(&a).iter().any(|n| n.name == "Row 505"));
        assert!(!items(&a).iter().any(|n| n.name == "Row 0"));
    }

    #[test]
    fn shift_click_extends_through_the_pointer_modifiers() {
        let mut app = list_app();
        app.render_once();
        let row_y = |i: f64| 1.0 + 24.0 * i + 12.0;
        assert!(app.click_at(crate::geometry::Point::new(50.0, row_y(1.0))));
        app.render_once();
        crate::platform::set_pointer_modifiers(Modifiers {
            shift: true,
            ..Modifiers::default()
        });
        assert!(app.click_at(crate::geometry::Point::new(50.0, row_y(4.0))));
        crate::platform::set_pointer_modifiers(Modifiers::default());
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        let selected: Vec<_> = items(&a)
            .into_iter()
            .filter(|n| n.selected)
            .map(|n| n.name.clone())
            .collect();
        assert_eq!(selected, vec!["Row 1", "Row 2", "Row 3", "Row 4"]);
    }
}
