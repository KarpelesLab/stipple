//! A table / data grid on the virtualized list: column headers that sort on
//! click (with a ▲/▼ indicator), columns resized by dragging a header's right
//! edge, single or multiple row selection (Shift/Ctrl+click, Shift+arrows),
//! a cell cursor moved by Left/Right, and striped rows. Columns wider than
//! the view scroll sideways under a header that follows them, with leading
//! columns optionally frozen in place. Only the visible rows' cells, of the
//! visible columns, are built.

use std::ops::Range;
use std::rc::Rc;

use crate::core::{Align, Axis, BoxStyle, Cx, Element, KeyInput, Role, ScrollId, pointer};
use crate::geometry::Insets;

use super::virtual_list::{
    DEFAULT_OVERSCAN, ListState, RowHeight, Rows, SelectionMode, Window, frame, row_fill,
};

/// Width of the drag handle at a header's right edge, in logical pixels.
const HANDLE: f64 = 6.0;

/// A table column: its header title and starting width.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Column {
    /// The header's title.
    pub title: String,
    /// The starting width, in logical pixels (the user can resize it).
    pub width: f64,
    /// The narrowest a resize can make it.
    pub min_width: f64,
    /// Whether clicking the header sorts by this column.
    pub sortable: bool,
    /// Horizontal placement of the cells' content (`Start`, `Center`, `End`).
    pub align: Align,
}

impl Column {
    /// A column titled `title`, `width` wide, not sortable.
    pub fn new(title: impl Into<String>, width: f64) -> Self {
        Self {
            title: title.into(),
            width,
            min_width: 32.0,
            sortable: false,
            align: Align::Start,
        }
    }

    /// Let clicking the header sort by this column.
    pub fn sortable(mut self) -> Self {
        self.sortable = true;
        self
    }

    /// The narrowest a resize can make the column.
    pub fn min_width(mut self, width: f64) -> Self {
        self.min_width = width;
        self
    }

    /// Place the cells' content (e.g. `Align::End` for numbers).
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }
}

/// A sort direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortOrder {
    /// Smallest first.
    Ascending,
    /// Largest first.
    Descending,
}

impl SortOrder {
    /// The other direction.
    pub fn reversed(self) -> Self {
        match self {
            SortOrder::Ascending => SortOrder::Descending,
            SortOrder::Descending => SortOrder::Ascending,
        }
    }
}

/// A column resize in progress: which column, where the drag started, and the
/// column's width then.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Resize {
    column: usize,
    origin_x: f64,
    start_width: f64,
}

/// A table's state, kept in the app state: the row selection, the cell
/// cursor's column, the sort, and the column widths the user has set.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TableState {
    /// The row selection and keyboard cursor (rows by display index).
    pub rows: ListState,
    /// The cell cursor's column (Left/Right move it; `None` until they do).
    column: Option<usize>,
    sort: Option<(usize, SortOrder)>,
    /// Widths set by resizing, by column (`None` = the column's own width).
    widths: Vec<Option<f64>>,
    resize: Option<Resize>,
}

impl TableState {
    /// A table selecting rows in `mode`, unsorted.
    pub fn new(mode: SelectionMode) -> Self {
        Self {
            rows: ListState::new(mode),
            ..Self::default()
        }
    }

    /// The column and direction the table is sorted by, if any.
    pub fn sort(&self) -> Option<(usize, SortOrder)> {
        self.sort
    }

    /// Show the table as sorted by `column` (this only moves the indicator;
    /// the app orders its rows).
    pub fn set_sort(&mut self, sort: Option<(usize, SortOrder)>) {
        self.sort = sort;
    }

    /// The column of the cell cursor (the cell at the cursor row, which
    /// Left/Right move between columns), if the keyboard has put it in one.
    pub fn cursor_column(&self) -> Option<usize> {
        self.column
    }

    /// Put the cell cursor in `column` (or take it out of the cells). The
    /// table scrolls it into view sideways when its keys move it.
    pub fn set_cursor_column(&mut self, column: Option<usize>) {
        self.column = column;
    }

    /// The current width of `column`, defaulting to `default`.
    pub fn column_width(&self, column: usize, default: f64) -> f64 {
        self.widths
            .get(column)
            .copied()
            .flatten()
            .unwrap_or(default)
    }

    /// Set the width of `column`.
    pub fn set_column_width(&mut self, column: usize, width: f64) {
        if self.widths.len() <= column {
            self.widths.resize(column + 1, None);
        }
        self.widths[column] = Some(width);
    }
}

/// What a header click sorting the table calls (see [`TableSpec::on_sort`]).
type SortFn<S> = Rc<dyn Fn(&mut S, usize, SortOrder)>;
type KeyFn<S> = Rc<dyn Fn(&mut S, &KeyInput)>;

/// The shape of a [`table`]: its columns, row count and sizes, and what
/// sorting it calls, for an app state `S`.
#[non_exhaustive]
pub struct TableSpec<'a, S> {
    /// The columns, in order.
    pub columns: &'a [Column],
    /// How many rows the table has.
    pub rows: usize,
    /// The height of every row, and of the header.
    pub row_height: f64,
    /// The height of the scrolling body (below the header).
    pub height: f64,
    /// Tint every other row.
    pub striped: bool,
    /// Rows built beyond each edge of the viewport.
    pub overscan: usize,
    /// How many leading columns stay put while the others scroll sideways.
    pub frozen_columns: usize,
    on_sort: Option<SortFn<S>>,
    on_key: Option<KeyFn<S>>,
}

impl<S> Clone for TableSpec<'_, S> {
    fn clone(&self) -> Self {
        Self {
            on_sort: self.on_sort.clone(),
            on_key: self.on_key.clone(),
            ..*self
        }
    }
}

impl<S> std::fmt::Debug for TableSpec<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TableSpec")
            .field("columns", &self.columns)
            .field("rows", &self.rows)
            .field("row_height", &self.row_height)
            .field("height", &self.height)
            .field("striped", &self.striped)
            .field("overscan", &self.overscan)
            .field("frozen_columns", &self.frozen_columns)
            .field("on_sort", &self.on_sort.is_some())
            .field("on_key", &self.on_key.is_some())
            .finish()
    }
}

impl<'a, S> TableSpec<'a, S> {
    /// `rows` rows of `columns`, 28px rows, a 320px body, striped.
    pub fn new(columns: &'a [Column], rows: usize) -> Self {
        Self {
            columns,
            rows,
            row_height: 28.0,
            height: 320.0,
            striped: true,
            overscan: DEFAULT_OVERSCAN,
            frozen_columns: 0,
            on_sort: None,
            on_key: None,
        }
    }

    /// Set the height of every row, and of the header.
    pub fn row_height(mut self, height: f64) -> Self {
        self.row_height = height;
        self
    }

    /// Set the height of the scrolling body.
    pub fn height(mut self, height: f64) -> Self {
        self.height = height;
        self
    }

    /// Set whether every other row is tinted.
    pub fn striped(mut self, striped: bool) -> Self {
        self.striped = striped;
        self
    }

    /// Set how many rows are built beyond each edge of the viewport.
    pub fn overscan(mut self, rows: usize) -> Self {
        self.overscan = rows;
        self
    }

    /// Keep the first `columns` columns in place (with their headers) while
    /// the others scroll sideways under them: a name or id column that
    /// stays in view. They're on the right, right to left.
    pub fn frozen_columns(mut self, columns: usize) -> Self {
        self.frozen_columns = columns;
        self
    }

    /// Call `f` with the column and direction when a click on a sortable
    /// header sorts the table, for the app to reorder the rows it shows.
    pub fn on_sort(mut self, f: impl Fn(&mut S, usize, SortOrder) + 'static) -> Self {
        self.on_sort = Some(Rc::new(f));
        self
    }

    /// Give `f` the keys the table's selection doesn't use (Enter, typed
    /// text, Delete, F2…) while the table has focus. If it moves the cursor (`TableState::rows`), the table
    /// scrolls the new cursor row into view, as the arrow keys do: a file
    /// list's Enter to open and type-to-select go here.
    pub fn on_key(mut self, f: impl Fn(&mut S, &KeyInput) + 'static) -> Self {
        self.on_key = Some(Rc::new(f));
        self
    }
}

/// Where the columns are sideways this frame, and which are built: the
/// frozen ones and those in (and one beside each edge of) the view.
#[derive(Clone, Debug)]
struct Columns {
    /// Each column's start from the table's (leading) edge, then the total.
    starts: Rc<[f64]>,
    frozen: usize,
    /// The scrolling columns built.
    built: Range<usize>,
    /// The sideways scrolling: its id, offset (clamped) and the width the
    /// rows show in (the body's less the vertical bar's gutter).
    id: ScrollId,
    offset: f64,
    room: f64,
}

impl Columns {
    /// The frozen columns' width.
    fn frozen_width(&self) -> f64 {
        self.starts[self.frozen]
    }

    /// The offset that shows column `c` whole beside the frozen ones,
    /// moving as little as possible (its start when it's wider than the
    /// room), or `None` when it shows already (or is frozen).
    fn reveal(&self, c: usize) -> Option<f64> {
        if c < self.frozen || c + 1 >= self.starts.len() {
            return None;
        }
        let (start, end) = (self.starts[c], self.starts[c + 1]);
        let first = self.offset + self.frozen_width();
        let to = if start < first - 0.5 {
            start - self.frozen_width()
        } else if end > self.offset + self.room + 0.5 {
            (end - self.room).min(start - self.frozen_width())
        } else {
            return None;
        };
        Some(to.max(0.0))
    }

    /// The row (or header) of `cells`: the frozen ones, then a strip of the
    /// built ones (clipped clear of the frozen ones) that follows the
    /// sideways scrolling. (The body's rows are pinned back against it,
    /// which their content would follow too.)
    fn row(&self, frozen: Vec<Element>, cells: Vec<Element>) -> Element {
        let gap = self.starts[self.built.start] - self.frozen_width();
        let mut strip = Vec::with_capacity(cells.len() + 1);
        strip.push(Element::boxed(BoxStyle::default()).width(gap.max(0.0)));
        strip.extend(cells);
        let strip = Element::stack(Axis::Horizontal, strip)
            .width(0.0)
            .grow(1.0)
            .clip()
            .scroll_follow(self.id, false);
        let mut children = frozen;
        children.push(strip);
        Element::stack(Axis::Horizontal, children)
    }
}

/// The columns' places and which to build: all of them until the body
/// overflows sideways, else the frozen ones and those the view shows.
fn place_columns<S>(
    cx: &mut Cx<S>,
    window: &Window,
    widths: &[f64],
    frozen: usize,
    gutter: f64,
) -> Columns {
    let mut starts = Vec::with_capacity(widths.len() + 1);
    let mut x = 0.0;
    starts.push(x);
    for w in widths {
        x += w;
        starts.push(x);
    }
    let total = x;
    let id = cx.register_scroll();
    // The body's width (the last frame's, or a guess the app builds again
    // for when the layout gives it another), less the vertical bar's
    // gutter while the rows overflow, which with the sideways bar's below
    // them they may only then.
    let view = cx.fit_scroll_width(window.id);
    let rows = window.rows.total();
    let room = |down: bool| (view - if down { gutter } else { 0.0 }).max(0.0);
    let mut down = rows > window.height + 0.5;
    let mut across = total > room(down) + 0.5;
    if across && !down {
        down = rows + gutter > window.height + 0.5;
        across = total > room(down) + 0.5;
    }
    let room = room(down);
    let max = if across { total - room } else { 0.0 };
    let offset = cx.scroll_offset(id).clamp(0.0, max);
    let n = widths.len();
    let frozen = frozen.min(n);
    let built = if across {
        let from = offset + starts[frozen];
        let to = offset + room;
        let first = (frozen..n).find(|&c| starts[c + 1] > from).unwrap_or(n);
        let last = (first..n)
            .find(|&c| starts[c + 1] >= to)
            .unwrap_or(n.max(1) - 1);
        // One more each side, so a small scroll shows built cells.
        first.saturating_sub(1).max(frozen)..(last + 2).min(n)
    } else {
        frozen..n
    };
    Columns {
        starts: starts.into(),
        frozen,
        built,
        id,
        offset,
        room,
    }
}

/// A virtualized table of `spec.rows` rows: a header row, then a scrolling
/// body that builds only the visible rows' cells with `cell(cx, row, column)`.
///
/// Clicking a sortable header sorts by it (again to reverse): the table moves
/// its indicator, clears the selection, scrolls to the top and calls the
/// spec's [`on_sort`](TableSpec::on_sort) with the column and direction, and
/// the app reorders the rows it shows (the row index passed to `cell` is the
/// display position). Dragging a header's right edge resizes the
/// column. Rows select per the [`TableState`]'s [`SelectionMode`].
///
/// Columns wider than the table's width scroll sideways (a horizontal wheel
/// or trackpad swipe, Shift+wheel, the bar along the bottom, or the cell
/// cursor), the header following them; only the columns in view are built
/// (each row's [`frozen_columns`](TableSpec::frozen_columns) too, which stay
/// put). Right to left, the columns start at the right.
///
/// ```
/// use stipple::prelude::*;
/// use stipple::widgets::{Column, SelectionMode, TableSpec, TableState, table};
///
/// struct App { table: TableState }
/// let theme = Theme::light();
/// let mut cx = Cx::<App>::new(&theme);
/// let state = App { table: TableState::new(SelectionMode::Multi) };
/// let columns = [Column::new("Name", 160.0).sortable(), Column::new("Size", 80.0)];
/// let grid = table(
///     &mut cx,
///     &state.table,
///     |s| &mut s.table,
///     TableSpec::new(&columns, 10_000)
///         .on_sort(|_s: &mut App, _column, _order| { /* reorder the rows */ }),
///     |cx, row, column| label(cx, format!("{row}:{column}")),
/// );
/// # let _ = grid;
/// ```
pub fn table<S: 'static>(
    cx: &mut Cx<S>,
    state: &TableState,
    field: impl Fn(&mut S) -> &mut TableState + Copy + 'static,
    spec: TableSpec<'_, S>,
    mut cell: impl FnMut(&mut Cx<S>, usize, usize) -> Element,
) -> Element {
    let theme = &super::theme_of(cx);
    let p = &theme.palette;
    let widths: Vec<f64> = spec
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| state.column_width(i, c.width).max(c.min_width))
        .collect();

    // The body's viewport: only its visible rows get built, and of them only
    // the columns in view.
    let rows = Rows::new(spec.rows, RowHeight::Fixed(spec.row_height));
    let mut window = Window::new(cx, rows, spec.height, spec.overscan);
    let gutter = theme.scroll_bar.gutter();
    let cols = place_columns(cx, &window, &widths, spec.frozen_columns, gutter);
    // While the columns overflow, the sideways bar covers the body's bottom
    // (the layout scrolls the rows a gutter further for it).
    if cols.starts[widths.len()] > cols.room + 0.5 {
        window.cover_bottom(gutter);
    }
    let shown = |c: usize| c < cols.frozen || cols.built.contains(&c);

    // The rows before the header: their handlers' ids don't move as the
    // header's columns come and go sideways (what's under the pointer stays
    // hovered).
    let multi = state.rows.mode() != SelectionMode::Single;
    let cursor_column = state.column.filter(|&c| c < widths.len());
    let mut built = Vec::with_capacity(window.range.len());
    for index in window.range.clone() {
        let selected = state.rows.is_selected(index);
        let cursor = state.rows.cursor() == Some(index);
        let mut frozen = Vec::with_capacity(cols.frozen);
        let mut cells = Vec::with_capacity(cols.built.len());
        for (col, column) in spec.columns.iter().enumerate() {
            if !shown(col) {
                continue;
            }
            let content = cell(cx, index, col);
            let mut el = Element::stack(Axis::Horizontal, vec![content])
                .width(widths[col])
                .height(spec.row_height)
                .padding(Insets::symmetric(theme.spacing.md, 0.0))
                .align(column.align, Align::Center)
                .clip()
                .role(Role::Cell)
                .a11y_column(col);
            if cursor && cursor_column == Some(col) {
                el = el.border(p.focus_ring, 1.0);
            }
            if col < cols.frozen {
                frozen.push(el);
            } else {
                cells.push(el);
            }
        }
        // No wider than the view (however wide the columns): its fill, the
        // hover and its bounds stay in it.
        let mut el = cols
            .row(frozen, cells)
            .max_width(cols.room)
            .height(spec.row_height);
        if let Some(fill) = row_fill(theme, selected, spec.striped && index % 2 == 1) {
            el = el.fill(fill);
        }
        if multi && cursor && cursor_column.is_none() {
            el = el.border(p.focus_ring, 1.0);
        }
        let w = window.clone();
        // Row 0 is the header row.
        let el = el.role(Role::Row).a11y_row(index + 1);
        built.push(el.selected(selected).on_tap(cx, move |s| {
            field(s).rows.click(index, pointer().modifiers);
            w.reveal(index);
        }));
    }
    // The header: a title (tap to sort) and a resize handle per column.
    let mut frozen_heads = Vec::with_capacity(cols.frozen);
    let mut heads = Vec::with_capacity(cols.built.len());
    for (col, column) in spec.columns.iter().enumerate() {
        if !shown(col) {
            continue;
        }
        let mut title = vec![
            Element::text(column.title.clone(), theme.typography.body, p.text),
            Element::stack(Axis::Horizontal, Vec::new()).grow(1.0),
        ];
        if let Some((sorted, order)) = state.sort()
            && sorted == col
        {
            let mark = match order {
                SortOrder::Ascending => "▲",
                SortOrder::Descending => "▼",
            };
            title.push(Element::text(mark, theme.typography.caption, p.text_muted));
        }
        let mut head = Element::stack(Axis::Horizontal, title)
            .width((widths[col] - HANDLE).max(0.0))
            .height(spec.row_height)
            .padding(Insets::symmetric(theme.spacing.md, 0.0))
            .gap(theme.spacing.xs)
            .clip()
            .role(Role::ColumnHeader)
            .a11y_column(col);
        if column.sortable {
            let on_sort = spec.on_sort.clone();
            let (requests, id) = (window.requests.clone(), window.id);
            head = head.on_tap(cx, move |s| {
                let t = field(s);
                let order = match t.sort {
                    Some((c, o)) if c == col => o.reversed(),
                    _ => SortOrder::Ascending,
                };
                t.sort = Some((col, order));
                t.rows.clear();
                t.rows.set_cursor(None);
                requests.scroll_to(id, 0.0);
                if let Some(on_sort) = &on_sort {
                    on_sort(s, col, order);
                }
            });
        }
        let (min, current) = (column.min_width, widths[col]);
        let line = Element::boxed(BoxStyle {
            fill: Some(p.border),
            radius: 0.0,
            border: None,
        })
        .width(1.0)
        .height((spec.row_height - 10.0).max(0.0));
        let handle = Element::stack(Axis::Horizontal, vec![line])
            .width(HANDLE)
            .height(spec.row_height)
            .align(Align::Center, Align::Center)
            .on_drag(cx, move |s, _| {
                let at = pointer();
                let Some(origin) = at.drag_origin else { return };
                let t = field(s);
                // A new drag (or a different column) starts from the width now.
                let start = match t.resize {
                    Some(r) if r.column == col && r.origin_x == origin.x => r.start_width,
                    _ => {
                        let start_width = current;
                        t.resize = Some(Resize {
                            column: col,
                            origin_x: origin.x,
                            start_width,
                        });
                        start_width
                    }
                };
                t.set_column_width(col, (start + at.position.x - origin.x).max(min));
            });
        let both = Element::stack(Axis::Horizontal, vec![head, handle]);
        if col < cols.frozen {
            frozen_heads.push(both);
        } else {
            heads.push(both);
        }
    }
    // Its columns end where the rows' do, short of the vertical bar.
    let header = Element::stack(
        Axis::Horizontal,
        vec![cols.row(frozen_heads, heads).grow(1.0).max_width(cols.room)],
    )
    .height(spec.row_height)
    .fill(p.background)
    .role(Role::Row);
    let rule = Element::boxed(BoxStyle {
        fill: Some(p.border),
        radius: 0.0,
        border: None,
    })
    .height(1.0);

    let count = spec.rows;
    let total = cols.starts[widths.len()];
    // The content is as wide as the columns, which the body scrolls
    // sideways; its rows stay put against that (each one's strip follows).
    let content = window
        .content(built)
        .min_width(total)
        .scroll_follow(cols.id, true);
    let body = window.scroller(content).scrollable_both(cols.id);
    let on_key = spec.on_key.clone();
    let columns = spec.columns.len();
    let rtl = theme.layout_direction.is_rtl();
    let requests = window.requests.clone();
    frame(theme, vec![header, rule, body])
        .role(Role::Table)
        .a11y_table(crate::core::AccessTable::new(count + 1, columns).frozen_columns(cols.frozen))
        .on_key(cx, move |s, key| {
            let t = field(s);
            let before = (t.rows.cursor(), t.column);
            // Left and Right move the cell cursor across the columns (the
            // other way right to left), Ctrl/Cmd with them to the first or
            // last.
            let last = columns.saturating_sub(1);
            let at = t.column.unwrap_or(0).min(last);
            let column = match (key, rtl) {
                (KeyInput::Left, false) | (KeyInput::Right, true) => Some(at.saturating_sub(1)),
                (KeyInput::Right, false) | (KeyInput::Left, true) => Some((at + 1).min(last)),
                (KeyInput::WordLeft, false) | (KeyInput::WordRight, true) => Some(0),
                (KeyInput::WordRight, false) | (KeyInput::WordLeft, true) => Some(last),
                _ => None,
            };
            if columns > 0
                && let Some(c) = column
            {
                t.column = Some(c);
            } else if !t.rows.handle_key(key, count)
                && let Some(f) = &on_key
            {
                f(s, key);
            }
            let t = field(s);
            let after = (t.rows.cursor(), t.column);
            if after != before {
                if let Some(c) = after.0 {
                    window.reveal(c);
                }
                // The cursor's cell in view sideways too.
                if let Some(to) = after.1.and_then(|c| cols.reveal(c)) {
                    requests.scroll_to(cols.id, to);
                }
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::AccessNode;
    use crate::geometry::{Point, Size};

    struct App {
        table: TableState,
        order: Vec<usize>,
        sorted: Vec<(usize, SortOrder)>,
    }

    fn columns() -> Vec<Column> {
        vec![
            Column::new("Name", 120.0).sortable(),
            Column::new("Value", 80.0).align(Align::End),
        ]
    }

    fn app() -> crate::App<App> {
        crate::App::new(
            App {
                table: TableState::new(SelectionMode::Multi),
                order: (0..10_000).collect(),
                sorted: Vec::new(),
            },
            |state: &App, cx: &mut Cx<App>| {
                let theme = *cx.theme();
                let cols = columns();
                let grid = table(
                    cx,
                    &state.table,
                    |s| &mut s.table,
                    TableSpec::new(&cols, state.order.len())
                        .row_height(24.0)
                        .height(240.0)
                        .on_sort(|s: &mut App, col, order| {
                            s.sorted.push((col, order));
                            if order == SortOrder::Descending {
                                s.order.reverse();
                            }
                        }),
                    |_cx, row, col| {
                        let item = state.order[row];
                        let text = if col == 0 {
                            format!("Item {item}")
                        } else {
                            format!("{}", item * 3)
                        };
                        Element::text(text, 14.0, theme.palette.text)
                    },
                );
                // Under a container: the root is reported as the window.
                crate::widgets::column(vec![grid])
            },
        )
        .no_font()
        .logical_size(Size::new(400.0, 320.0))
    }

    fn rows(a: &AccessNode) -> Vec<&AccessNode> {
        // Body rows are the selectable ones (the header row has headers).
        a.descendants()
            .into_iter()
            .filter(|n| n.role == Role::Row && n.children.iter().all(|c| c.role == Role::Cell))
            .collect()
    }

    #[test]
    fn roles_virtualization_and_selection() {
        let mut app = app();
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        let table = a.descendants().into_iter().find(|n| n.role == Role::Table);
        assert!(table.is_some());
        let headers: Vec<_> = a
            .descendants()
            .into_iter()
            .filter(|n| n.role == Role::ColumnHeader)
            .map(|n| n.name.clone())
            .collect();
        assert_eq!(headers, vec!["Name", "Value"]);
        let body = rows(&a);
        assert!(body.len() < 20, "{} rows built", body.len());
        assert_eq!(body[0].children[0].name, "Item 0");
        assert_eq!(body[0].children[1].name, "0");

        // Rows start below the 1px frame, the 24px header and the 1px rule.
        let y = |i: f64| 1.0 + 24.0 + 1.0 + 24.0 * i + 12.0;
        assert!(app.click_at(Point::new(40.0, y(2.0))));
        crate::platform::set_pointer_modifiers(crate::platform::Modifiers {
            ctrl: true,
            ..Default::default()
        });
        assert!(app.click_at(Point::new(40.0, y(5.0))));
        crate::platform::set_pointer_modifiers(Default::default());
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        let selected: Vec<_> = rows(&a)
            .into_iter()
            .filter(|r| r.selected)
            .map(|r| r.children[0].name.clone())
            .collect();
        assert_eq!(selected, vec!["Item 2", "Item 5"]);
        // Shift+Down extends from the cursor.
        assert!(app.press_key(crate::core::KeyInput::SelectDown));
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        let n = rows(&a).into_iter().filter(|r| r.selected).count();
        assert_eq!(n, 2, "a Shift range replaces: rows 5..=6");
    }

    #[test]
    fn header_click_sorts_and_toggles() {
        let mut app = app();
        app.render_once();
        // Scrolled, the overscan rows above the body lie under the header,
        // clipped: the header still gets the click.
        app.hover_at(Point::new(30.0, 100.0));
        assert!(app.scroll_by(24.0 * 30.0));
        app.render_once();
        let header = Point::new(30.0, 13.0);
        assert!(app.click_at(header));
        app.render_once();
        assert!(app.click_at(header));
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        // Descending reversed the order; the indicator follows.
        assert_eq!(rows(&a)[0].children[0].name, "Item 9999");
        // The value column isn't sortable: clicking it does nothing.
        assert!(!app.click_at(Point::new(160.0, 13.0)));
    }

    #[test]
    fn keys_the_selection_skips_reach_on_key_and_a_moved_cursor_is_revealed() {
        // Typing a number jumps to "Item <n>"; Enter records the cursor row.
        struct Typed {
            table: TableState,
            opened: Vec<usize>,
        }
        let mut app = crate::App::new(
            Typed {
                table: TableState::new(SelectionMode::Multi),
                opened: Vec::new(),
            },
            |state: &Typed, cx: &mut Cx<Typed>| {
                let theme = *cx.theme();
                let cols = columns();
                let grid = table(
                    cx,
                    &state.table,
                    |s| &mut s.table,
                    TableSpec::new(&cols, 10_000)
                        .row_height(24.0)
                        .height(240.0)
                        .on_key(|s: &mut Typed, key| match key {
                            KeyInput::Text(t) => {
                                if let Ok(n) = t.parse::<usize>() {
                                    s.table.rows.select(n * 1000);
                                }
                            }
                            KeyInput::Enter => {
                                s.opened.extend(s.table.rows.cursor());
                            }
                            _ => {}
                        }),
                    |_cx, row, _col| Element::text(format!("Item {row}"), 14.0, theme.palette.text),
                );
                crate::widgets::column(vec![grid])
            },
        )
        .no_font()
        .logical_size(Size::new(400.0, 320.0));
        app.render_once();
        let y = 1.0 + 24.0 + 1.0 + 12.0;
        assert!(app.click_at(Point::new(40.0, y)));
        // The arrows are still the selection's.
        assert!(app.press_key(KeyInput::Down));
        assert!(app.press_key(KeyInput::Text("7".into())));
        assert!(app.press_key(KeyInput::Enter));
        assert_eq!(app.state().opened, vec![7000]);
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        let row = rows(&a)
            .into_iter()
            .find(|r| r.children[0].name == "Item 7000")
            .expect("row 7000 scrolled into view");
        assert!(row.selected);
    }

    #[test]
    fn dragging_a_header_edge_resizes_the_column() {
        let mut app = app();
        app.render_once();
        // The handle sits at the right edge of the 120px "Name" column.
        let edge = 1.0 + 120.0 - HANDLE / 2.0;
        assert!(app.drag_at_point(Point::new(edge, 13.0)));
        assert!(app.drag_at_point(Point::new(edge + 40.0, 13.0)));
        assert!(app.drag_at_point(Point::new(edge + 60.0, 13.0)));
        app.end_drag();
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        let first = &rows(&a)[0].children[0];
        assert!(
            (first.bounds.width() - 180.0).abs() < 0.5,
            "{:?}",
            first.bounds
        );
        // A second drag starts from the new width; the minimum holds.
        let edge = 1.0 + 180.0 - HANDLE / 2.0;
        assert!(app.drag_at_point(Point::new(edge, 13.0)));
        assert!(app.drag_at_point(Point::new(edge - 500.0, 13.0)));
        app.end_drag();
        app.render_once();
        let a = app.accessibility_tree().unwrap();
        let first = &rows(&a)[0].children[0];
        assert!(
            (first.bounds.width() - 32.0).abs() < 0.5,
            "{:?}",
            first.bounds
        );
    }
}
