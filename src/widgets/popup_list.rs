//! The list a [`select`](super::select) or a [`combo_box`](super::combo_box)
//! drops below itself: one row per entry, the highlighted one tinted (and,
//! in a select, the chosen one checked). A long list scrolls, builds only
//! the rows in view (a font picker's hundreds of families cost a screenful),
//! and follows the keyboard's highlight.

use crate::core::{Axis, Cx, Element, Role, ScrollId, ScrollRequests};
use crate::geometry::Insets;
use crate::style::Theme;

/// Lists longer than this scroll.
pub(crate) const MAX_VISIBLE: usize = 8;

/// The gap between rows.
const GAP: f64 = 2.0;

/// A popup list's scroll container, kept from one build to the next, and
/// what a key handler needs to scroll it to the highlighted row.
#[derive(Clone, Debug)]
pub(crate) struct ListScroll {
    id: ScrollId,
    /// Its offset as of this build, clamped to its rows.
    offset: f64,
    row: f64,
    viewport: f64,
    rows: usize,
    requests: ScrollRequests,
}

impl ListScroll {
    /// The accessibility id of the list this scrolls, which the control
    /// opening it [`controls`](Element::controls).
    pub(crate) fn list_id(&self) -> String {
        format!("stipple-popup-list-{}", self.id.0)
    }

    /// The scroll container of a list of `rows` rows. Register it on every
    /// build, open or closed, so the scroll containers declared after it
    /// keep their ids (and offsets).
    pub(crate) fn register<S>(cx: &mut Cx<S>, theme: &Theme, rows: usize) -> Self {
        let id = cx.register_scroll();
        let row = row_height(theme);
        let viewport = MAX_VISIBLE as f64 * (row + GAP);
        let content = rows as f64 * (row + GAP) - GAP;
        let offset = cx
            .scroll_offset(id)
            .clamp(0.0, (content - viewport).max(0.0));
        Self {
            id,
            offset,
            row,
            viewport,
            rows,
            requests: cx.scroll_requests(),
        }
    }

    /// Whether the list scrolls (it has more than [`MAX_VISIBLE`] rows).
    fn scrolls(&self) -> bool {
        self.rows > MAX_VISIBLE
    }

    fn pitch(&self) -> f64 {
        self.row + GAP
    }

    /// Scroll just enough for row `pos` to show whole.
    pub(crate) fn reveal(&self, pos: Option<usize>) {
        let Some(pos) = pos.filter(|_| self.scrolls()) else {
            return;
        };
        let top = pos as f64 * self.pitch();
        let bottom = top + self.row;
        if top < self.offset {
            self.requests.scroll_to(self.id, top);
        } else if bottom > self.offset + self.viewport {
            self.requests.scroll_to(self.id, bottom - self.viewport);
        }
    }

    /// Scroll to show row `pos` in the middle of the list as it opens (or
    /// the top, for none), whatever the list showed when it last closed, on
    /// `cx`'s next frame. A list that doesn't scroll has nothing to do.
    pub(crate) fn open_at<S>(&self, cx: &mut Cx<S>, pos: Option<usize>) {
        if !self.scrolls() {
            return;
        }
        let target = pos.map_or(0.0, |pos| {
            pos as f64 * self.pitch() - (self.viewport - self.row) / 2.0
        });
        self.requests.scroll_to(self.id, target.max(0.0));
        cx.request_frame();
    }
}

/// How tall a row is.
fn row_height(theme: &Theme) -> f64 {
    (theme.typography.body * 1.4 + 2.0 * theme.spacing.sm).round()
}

/// What the popup shows.
pub(crate) struct PopupList<'a> {
    /// The rows' labels, in order.
    pub labels: &'a [&'a str],
    /// The row the keyboard has highlighted.
    pub highlighted: Option<usize>,
    /// The row checked as chosen; `None` leaves no room for a check mark.
    pub checked: Option<Option<usize>>,
    /// What goes over the rows (a select's search field).
    pub header: Option<Element>,
}

/// The popup: `list`'s header and its rows in the container `scroll`, a
/// tap on row `i` running `pick(i)`.
pub(crate) fn popup_list<S, F>(
    cx: &mut Cx<S>,
    theme: &Theme,
    scroll: &ListScroll,
    list: PopupList<'_>,
    pick: impl Fn(usize) -> F,
) -> Element
where
    F: FnMut(&mut S) + 'static,
{
    let p = &theme.palette;
    let n = list.labels.len();
    let entry = |i: usize| {
        let text = list.labels[i];
        let mut cells = Vec::with_capacity(2);
        if let Some(checked) = list.checked {
            let mark = if checked == Some(i) { "\u{2713}" } else { "" };
            cells.push(Element::text(mark, theme.typography.body, p.primary).width(16.0));
        }
        cells.push(Element::text(text, theme.typography.body, p.text));
        let mut row = Element::stack(Axis::Horizontal, cells)
            .radius(theme.radius / 2.0)
            .padding(Insets::symmetric(theme.spacing.sm, 0.0))
            .gap(theme.spacing.xs)
            .height(scroll.row)
            .role(Role::ListItem)
            .a11y_label(text);
        if list.highlighted == Some(i) {
            row = row.fill(p.selection);
        }
        row
    };
    // Only the rows in view (and one either side) of a long list.
    let (first, last) = if scroll.scrolls() {
        let pitch = scroll.pitch();
        let first = (scroll.offset / pitch).floor() as usize;
        let last = ((scroll.offset + scroll.viewport) / pitch).ceil() as usize + 1;
        (first.saturating_sub(1), last.min(n))
    } else {
        (0, n)
    };
    let mut rows = Vec::with_capacity(last - first + 2);
    let spacer = |rows: usize| {
        super::spacer()
            .height((rows as f64 * scroll.pitch() - GAP).max(0.0))
            .grow(0.0)
    };
    if first > 0 {
        rows.push(spacer(first));
    }
    for i in first..last {
        rows.push(entry(i).on_tap(cx, pick(i)));
    }
    if last < n {
        rows.push(spacer(n - last));
    }
    let mut body = Element::stack(Axis::Vertical, rows).gap(GAP);
    if scroll.scrolls() {
        // As wide as its longest row, wherever it's scrolled: that row, laid
        // out flat and off stage.
        let longest = (0..n)
            .max_by_key(|&i| list.labels[i].chars().count())
            .unwrap_or(0);
        let strut = entry(longest).height(0.0).offstage(true);
        body = Element::layers(vec![strut, body]);
        body = Element::stack(Axis::Vertical, vec![body])
            .scrollable(scroll.id)
            .height(scroll.viewport);
    }
    let mut content = Vec::with_capacity(2);
    content.extend(list.header);
    content.push(body);
    Element::stack(Axis::Vertical, content)
        .gap(theme.spacing.xs)
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::uniform(theme.spacing.xs))
        .clip()
        .role(Role::List)
}
