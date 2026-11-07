//! A table / data grid on the virtualized list: column headers that sort on
//! click (with a ▲/▼ indicator), columns resized by dragging a header's right
//! edge, single or multiple row selection (Shift/Ctrl+click, Shift+arrows),
//! and striped rows. Only the visible rows' cells are built.

use crate::core::{Align, Axis, BoxStyle, Cx, Element, Role, pointer};
use crate::geometry::Insets;
use crate::style::Theme;

use super::virtual_list::{
    DEFAULT_OVERSCAN, ListState, RowHeight, Rows, SelectionMode, Window, frame, row_fill,
};

/// Width of the drag handle at a header's right edge, in logical pixels.
const HANDLE: f64 = 6.0;

/// A table column: its header title and starting width.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
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
    Ascending,
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

/// A table's state, kept in the app state: the row selection, the sort, and
/// the column widths the user has set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableState {
    /// The row selection and keyboard cursor (rows by display index).
    pub rows: ListState,
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

/// The shape of a [`table`]: its columns, row count and sizes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableSpec<'a> {
    pub columns: &'a [Column],
    pub rows: usize,
    /// The height of every row, and of the header.
    pub row_height: f64,
    /// The height of the scrolling body (below the header).
    pub height: f64,
    /// Tint every other row.
    pub striped: bool,
    /// Rows built beyond each edge of the viewport.
    pub overscan: usize,
}

impl<'a> TableSpec<'a> {
    /// `rows` rows of `columns`, 28px rows, a 320px body, striped.
    pub fn new(columns: &'a [Column], rows: usize) -> Self {
        Self {
            columns,
            rows,
            row_height: 28.0,
            height: 320.0,
            striped: true,
            overscan: DEFAULT_OVERSCAN,
        }
    }

    pub fn row_height(mut self, height: f64) -> Self {
        self.row_height = height;
        self
    }

    pub fn height(mut self, height: f64) -> Self {
        self.height = height;
        self
    }

    pub fn striped(mut self, striped: bool) -> Self {
        self.striped = striped;
        self
    }
}

/// A virtualized table of `spec.rows` rows: a header row, then a scrolling
/// body that builds only the visible rows' cells with `cell(cx, row, column)`.
///
/// Clicking a sortable header sorts by it (again to reverse): the table moves
/// its indicator, clears the selection, scrolls to the top and calls `on_sort` with the column and
/// direction, and the app reorders the rows it shows (the row index passed to
/// `cell` is the display position). Dragging a header's right edge resizes the
/// column. Rows select per the [`TableState`]'s [`SelectionMode`].
///
/// ```
/// use stipple::prelude::*;
/// use stipple::widgets::{Column, SelectionMode, TableSpec, TableState, table};
///
/// struct App { table: TableState }
/// let theme = Theme::light();
/// let mut cx = Cx::new(&theme);
/// let state = App { table: TableState::new(SelectionMode::Multi) };
/// let columns = [Column::new("Name", 160.0).sortable(), Column::new("Size", 80.0)];
/// let grid = table(
///     &mut cx,
///     &theme,
///     &state.table,
///     |s: &mut App| &mut s.table,
///     TableSpec::new(&columns, 10_000),
///     |_s: &mut App, _column, _order| { /* reorder the rows */ },
///     |_cx, row, column| label(&theme, format!("{row}:{column}")),
/// );
/// # let _ = grid;
/// ```
pub fn table<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    state: &TableState,
    field: impl Fn(&mut S) -> &mut TableState + Copy + 'static,
    spec: TableSpec<'_>,
    on_sort: impl Fn(&mut S, usize, SortOrder) + Clone + 'static,
    mut cell: impl FnMut(&mut Cx<S>, usize, usize) -> Element,
) -> Element {
    let p = &theme.palette;
    let widths: Vec<f64> = spec
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| state.column_width(i, c.width).max(c.min_width))
        .collect();

    // The body's viewport: only its visible rows get built.
    let rows = Rows::new(spec.rows, RowHeight::Fixed(spec.row_height));
    let window = Window::new(cx, rows, spec.height, spec.overscan);

    // The header: a title (tap to sort) and a resize handle per column.
    let mut headers = Vec::with_capacity(spec.columns.len());
    for (col, column) in spec.columns.iter().enumerate() {
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
            .align(Align::Start, Align::Center)
            .clip()
            .role(Role::ColumnHeader);
        if column.sortable {
            let on_sort = on_sort.clone();
            let (requests, id) = (window.requests.clone(), window.id);
            head = head.on_tap(cx, move |s: &mut S| {
                let t = field(s);
                let order = match t.sort {
                    Some((c, o)) if c == col => o.reversed(),
                    _ => SortOrder::Ascending,
                };
                t.sort = Some((col, order));
                t.rows.clear();
                t.rows.set_cursor(None);
                requests.scroll_to(id, 0.0);
                on_sort(s, col, order);
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
            .on_drag(cx, move |s: &mut S, _| {
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
        headers.push(Element::stack(Axis::Horizontal, vec![head, handle]));
    }
    let header = Element::stack(Axis::Horizontal, headers)
        .fill(p.background)
        .role(Role::Row);
    let rule = Element::boxed(BoxStyle {
        fill: Some(p.border),
        radius: 0.0,
        border: None,
    })
    .height(1.0);

    let multi = state.rows.mode() != SelectionMode::Single;
    let mut built = Vec::with_capacity(window.range.len());
    for index in window.range.clone() {
        let selected = state.rows.is_selected(index);
        let cursor = state.rows.cursor() == Some(index);
        let mut cells = Vec::with_capacity(spec.columns.len());
        for (col, column) in spec.columns.iter().enumerate() {
            let content = cell(cx, index, col);
            cells.push(
                Element::stack(Axis::Horizontal, vec![content])
                    .width(widths[col])
                    .height(spec.row_height)
                    .padding(Insets::symmetric(theme.spacing.md, 0.0))
                    .align(column.align, Align::Center)
                    .clip()
                    .role(Role::Cell),
            );
        }
        let mut el = Element::stack(Axis::Horizontal, cells).height(spec.row_height);
        if let Some(fill) = row_fill(theme, selected, spec.striped && index % 2 == 1) {
            el = el.fill(fill);
        }
        if multi && cursor {
            el = el.border(p.focus_ring, 1.0);
        }
        let w = window.clone();
        built.push(
            el.role(Role::Row)
                .selected(selected)
                .on_tap(cx, move |s: &mut S| {
                    field(s).rows.click(index, pointer().modifiers);
                    w.reveal(index);
                }),
        );
    }
    let count = spec.rows;
    let body = window.assemble(built);
    frame(theme, vec![header, rule, body])
        .role(Role::Table)
        .on_key(cx, move |s: &mut S, key| {
            let st = &mut field(s).rows;
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
                    &theme,
                    &state.table,
                    |s: &mut App| &mut s.table,
                    TableSpec::new(&cols, state.order.len())
                        .row_height(24.0)
                        .height(240.0),
                    |s: &mut App, col, order| {
                        s.sorted.push((col, order));
                        if order == SortOrder::Descending {
                            s.order.reverse();
                        }
                    },
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
