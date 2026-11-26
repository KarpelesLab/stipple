//! The UI Automation bridge's logic that needs no COM: which patterns a
//! node has, what its Name and Value read, where a text range's units start
//! and end, how far a scroll moves, and which events a tree swap raises. The
//! Windows-only provider (`platform::uia`) answers UIA's calls from these;
//! kept apart (and not gated on Windows) so they're unit-tested everywhere.

use crate::platform::access::{A11yChange, A11yNode, A11yRole, A11yScroll, A11yTextRun, changes};

// Property ids (UIAutomationClient.h) the events report.
pub(crate) const NAME: i32 = 30005;
pub(crate) const VALUE_VALUE: i32 = 30045;
pub(crate) const SCROLL_HORIZONTAL_PERCENT: i32 = 30053;
pub(crate) const SCROLL_VERTICAL_PERCENT: i32 = 30055;
pub(crate) const EXPAND_COLLAPSE_STATE: i32 = 30070;
pub(crate) const SELECTION_ITEM_IS_SELECTED: i32 = 30079;
pub(crate) const TOGGLE_STATE: i32 = 30086;
pub(crate) const RANGE_VALUE_VALUE: i32 = 30047;

// Events (UIAutomationClient.h).
pub(crate) const TEXT_SELECTION_CHANGED: i32 = 20014;
pub(crate) const TEXT_CHANGED: i32 = 20015;

// Text attributes.
pub(crate) const BACKGROUND_COLOR: i32 = 40001;
pub(crate) const CULTURE: i32 = 40004;
pub(crate) const FONT_NAME: i32 = 40005;
pub(crate) const FONT_SIZE: i32 = 40006;
pub(crate) const FONT_WEIGHT: i32 = 40007;
pub(crate) const FOREGROUND_COLOR: i32 = 40008;
pub(crate) const IS_ITALIC: i32 = 40014;
pub(crate) const IS_READ_ONLY: i32 = 40015;
pub(crate) const STRIKETHROUGH_STYLE: i32 = 40026;
pub(crate) const UNDERLINE_STYLE: i32 = 40030;

/// `UIA_ScrollPatternNoScroll`: the percent of an axis that doesn't scroll.
pub(crate) const NO_SCROLL: f64 = -1.0;

/// Which control patterns a node has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Patterns {
    pub(crate) invoke: bool,
    pub(crate) value: bool,
    pub(crate) toggle: bool,
    pub(crate) selection: bool,
    pub(crate) selection_item: bool,
    pub(crate) expand_collapse: bool,
    pub(crate) scroll: bool,
    pub(crate) text: bool,
    pub(crate) range_value: bool,
    /// Grid and Table, on a table.
    pub(crate) grid: bool,
    /// GridItem and TableItem, on a table's cell or column header.
    pub(crate) grid_item: bool,
}

/// The patterns of `n` (in a window: the actions all need one to land in).
pub(crate) fn patterns(n: &A11yNode) -> Patterns {
    use A11yRole::*;
    let toggle = matches!(n.role, CheckBox | Switch);
    let selection_item = matches!(n.role, ListItem | TreeItem | Row | Tab | RadioButton);
    let expand_collapse = n.expanded.is_some() || n.role == ComboBox;
    Patterns {
        // A checkbox toggles, an item is selected, a tree item expands: UIA
        // clients use those patterns for them, so Invoke stays on the plain
        // activatable controls (and a select, which a click opens).
        invoke: matches!(
            n.role,
            Button | MenuItem | ComboBox | CheckBox | RadioButton | Switch | Tab | ColumnHeader
        ),
        value: matches!(n.role, TextField | PasswordField | SpinButton),
        toggle,
        selection: matches!(n.role, List | Tree | Table | TabList),
        selection_item,
        expand_collapse,
        scroll: n.scroll.is_some(),
        text: matches!(n.role, TextField | PasswordField | SpinButton | Text),
        range_value: n.range.is_some(),
        grid: n.role == Table && n.table.is_some(),
        grid_item: matches!(n.role, Cell | ColumnHeader) && n.cell.is_some(),
    }
}

/// The RangeValue pattern's numbers: `(value, minimum, maximum, small
/// change, large change)`, a large change being a tenth of the range (or
/// the small one, when larger).
pub(crate) fn range_numbers(n: &A11yNode) -> Option<(f64, f64, f64, f64, f64)> {
    let r = n.range?;
    let small = r.increment();
    let span = r.max - r.min;
    let large = if span > 0.0 && span < 1e15 {
        (span / 10.0).max(small)
    } else {
        small * 10.0
    };
    Some((r.value, r.min, r.max, small, large))
}

/// The value of text attribute `attr` over all of `n`'s text (see
/// [`range_attribute`]), `None` when it isn't one the tree knows.
#[cfg(test)]
pub(crate) fn text_attribute(n: &A11yNode, attr: i32) -> Option<Val> {
    let len = text_of(n).len();
    match range_attribute(n, attr, (0, len), &|_| None) {
        Some(Attr::Val(v)) => Some(v),
        _ => None,
    }
}

/// A text attribute's value over a range: one value, or mixed (the range
/// spans runs where it differs: UIA's reserved mixed-attribute value).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Attr {
    Val(Val),
    Mixed,
}

/// `n`'s text runs, as character ranges of its text.
pub(crate) fn char_runs(n: &A11yNode) -> Vec<((usize, usize), A11yTextRun)> {
    n.text_runs()
        .into_iter()
        .map(|r| ((char_at(n, r.range.0), char_at(n, r.range.1)), r))
        .collect()
}

/// Where `n`'s format runs start (in characters), with the text's length at
/// the end: what the Format unit moves by.
pub(crate) fn format_bounds(n: &A11yNode) -> Vec<usize> {
    let len = text_of(n).len();
    let mut out = vec![0];
    out.extend(char_runs(n).into_iter().map(|((_, e), _)| e.min(len)));
    out.push(len);
    out.dedup();
    out
}

/// A COLORREF (`0x00bbggrr`), as UIA's color attributes carry one.
fn colorref([r, g, b, _]: [u8; 4]) -> i32 {
    i32::from(r) | (i32::from(g) << 8) | (i32::from(b) << 16)
}

/// Attribute `attr` of run `r`; `None` when the run doesn't say (a color
/// plain text doesn't carry, a language it doesn't name) or it isn't one
/// the runs know. `lcid` maps a language tag to its LCID.
fn run_attribute(r: &A11yTextRun, attr: i32, lcid: &dyn Fn(&str) -> Option<i32>) -> Option<Val> {
    let f = &r.font;
    let line = |on: bool| Val::I4(i32::from(on));
    Some(match attr {
        FONT_NAME => Val::Str(f.family.clone()?),
        // Points: a logical pixel is a 96th of an inch.
        FONT_SIZE => Val::R8((f.size * 0.75 * 2.0).round() / 2.0),
        FONT_WEIGHT => Val::I4(i32::from(f.weight)),
        IS_ITALIC => Val::Bool(f.italic),
        // TextDecorationLineStyle: None 0, Single 1.
        UNDERLINE_STYLE => line(r.underline),
        STRIKETHROUGH_STYLE => line(r.strikethrough),
        FOREGROUND_COLOR => Val::I4(colorref(r.color?)),
        BACKGROUND_COLOR => Val::I4(colorref(r.background?)),
        CULTURE => Val::I4(lcid(r.language.as_deref()?)?),
        _ => return None,
    })
}

/// The value of text attribute `attr` over the characters `[start, end)`
/// of `n`'s text (an empty range: the run it's in): the runs' value, or
/// [`Attr::Mixed`] where the runs it spans differ. `None` when it isn't
/// one the tree knows (or no run says).
pub(crate) fn range_attribute(
    n: &A11yNode,
    attr: i32,
    (start, end): (usize, usize),
    lcid: &dyn Fn(&str) -> Option<i32>,
) -> Option<Attr> {
    if attr == IS_READ_ONLY {
        return Some(Attr::Val(Val::Bool(!crate::platform::access::editable(
            n.role,
        ))));
    }
    let runs = char_runs(n);
    let last = runs.len().checked_sub(1)?;
    let covered: Vec<&A11yTextRun> = if start >= end {
        let i = runs.iter().position(|(r, _)| start < r.1).unwrap_or(last);
        vec![&runs[i].1]
    } else {
        runs.iter()
            .filter(|(r, _)| r.0 < end && start < r.1)
            .map(|(_, run)| run)
            .collect()
    };
    let mut values = covered.iter().map(|r| run_attribute(r, attr, lcid));
    let first = values.next()?;
    if values.any(|v| v != first) {
        return Some(Attr::Mixed);
    }
    first.map(Attr::Val)
}

/// `FindAttribute`: the first (or last, `backward`) stretch of
/// `[start, end)` whose runs' attribute `attr` is one `want` accepts,
/// neighbouring runs that match joined.
pub(crate) fn find_attribute(
    n: &A11yNode,
    attr: i32,
    (start, end): (usize, usize),
    backward: bool,
    want: &dyn Fn(&Val) -> bool,
    lcid: &dyn Fn(&str) -> Option<i32>,
) -> Option<(usize, usize)> {
    if start >= end {
        return None;
    }
    let mut stretches: Vec<(usize, usize)> = Vec::new();
    let mut joined = false;
    for ((s, e), run) in char_runs(n) {
        let (s, e) = (s.max(start), e.min(end));
        if s >= e {
            continue;
        }
        let hit = run_attribute(&run, attr, lcid).is_some_and(|v| want(&v));
        match stretches.last_mut() {
            Some(last) if hit && joined && last.1 == s => last.1 = e,
            _ if hit => stretches.push((s, e)),
            _ => {}
        }
        joined = hit;
    }
    if backward {
        stretches.pop()
    } else {
        stretches.first().copied()
    }
}

/// Whether `n`'s name is its text (an editable field whose caret the tree
/// carries): then UIA reads the text as its Value and names it by its
/// caption, as a Win32 edit control is.
pub(crate) fn name_is_text(n: &A11yNode) -> bool {
    matches!(
        n.role,
        A11yRole::TextField | A11yRole::SpinButton | A11yRole::PasswordField
    ) && n.caret.is_some()
}

/// The node `id` names in `root`'s tree.
fn by_id<'a>(root: &'a A11yNode, id: &str) -> Option<&'a A11yNode> {
    if root.id.as_deref() == Some(id) {
        return Some(root);
    }
    root.children.iter().find_map(|c| by_id(c, id))
}

/// The Name UIA reports for `n` in `root`'s tree: its name, except for a
/// field whose name is its text, which is named by the node it's
/// [`labelled_by`](A11yNode::labelled_by) (nothing when none: its text is
/// its Value).
pub(crate) fn uia_name(root: &A11yNode, n: &A11yNode) -> String {
    if !name_is_text(n) {
        return n.name.clone();
    }
    n.labelled_by
        .as_deref()
        .and_then(|id| by_id(root, id))
        .map(|l| l.name.clone())
        .unwrap_or_default()
}

/// A field's Value: its text (a password field's reads as empty).
pub(crate) fn uia_value(n: &A11yNode) -> String {
    if n.role == A11yRole::PasswordField {
        String::new()
    } else {
        n.name.clone()
    }
}

/// The text the Text pattern serves, as characters.
pub(crate) fn text_of(n: &A11yNode) -> Vec<char> {
    if n.role == A11yRole::PasswordField {
        Vec::new()
    } else {
        n.name.chars().collect()
    }
}

/// A byte index of `n`'s name as a character offset.
pub(crate) fn char_at(n: &A11yNode, byte: usize) -> usize {
    let b = byte.min(n.name.len());
    n.name.char_indices().take_while(|&(i, _)| i < b).count()
}

/// A character offset of `n`'s name as a byte index.
pub(crate) fn byte_at(n: &A11yNode, ch: usize) -> usize {
    n.name
        .char_indices()
        .nth(ch)
        .map_or(n.name.len(), |(i, _)| i)
}

/// The field's selection, else its caret, as a character range (UIA's
/// `GetSelection`: a degenerate range at the caret when nothing is
/// selected). `None` when the tree doesn't carry them.
pub(crate) fn selection_of(n: &A11yNode) -> Option<(usize, usize)> {
    if let Some((s, e)) = n.text_selection {
        return Some((char_at(n, s), char_at(n, e)));
    }
    let c = char_at(n, n.caret?);
    Some((c, c))
}

/// `ToggleState`: Off 0, On 1.
pub(crate) fn toggle_state(n: &A11yNode) -> i32 {
    i32::from(n.checked == Some(true))
}

/// `ExpandCollapseState`: Collapsed 0, Expanded 1, LeafNode 3 (a tree item
/// that can't expand).
pub(crate) fn expand_state(n: &A11yNode) -> i32 {
    match n.expanded {
        Some(true) => 1,
        Some(false) => 0,
        None if n.role == A11yRole::ComboBox => 0,
        None => 3,
    }
}

/// The Scroll pattern's percents `(horizontal, vertical)`: 0 to 100 on the
/// axis it scrolls on, [`NO_SCROLL`] on the other (and on both when its
/// content fits).
pub(crate) fn scroll_percent(s: &A11yScroll) -> (f64, f64) {
    let p = if s.scrolls() {
        s.fraction() * 100.0
    } else {
        NO_SCROLL
    };
    if s.horizontal {
        (p, NO_SCROLL)
    } else {
        (NO_SCROLL, p)
    }
}

/// The view sizes `(horizontal, vertical)`, in percent: the share in view on
/// its axis, all of it on the other.
pub(crate) fn view_size(s: &A11yScroll) -> (f64, f64) {
    let v = s.view_fraction() * 100.0;
    if s.horizontal { (v, 100.0) } else { (100.0, v) }
}

/// What `ScrollAmount` `amount` (LargeDecrement 0, SmallDecrement 1,
/// NoAmount 2, LargeIncrement 3, SmallIncrement 4) moves along the axis.
pub(crate) fn scroll_amount(s: &A11yScroll, amount: i32) -> f64 {
    let line = crate::platform::event::SCROLL_LINE_PX;
    match amount {
        0 => -s.page(),
        1 => -line,
        3 => s.page(),
        4 => line,
        _ => 0.0,
    }
}

/// How far `Scroll(horizontal, vertical)` moves a container along its axis
/// (the other axis' amount is ignored: it doesn't scroll).
pub(crate) fn scroll_request(s: &A11yScroll, horizontal: i32, vertical: i32) -> f64 {
    scroll_amount(s, if s.horizontal { horizontal } else { vertical })
}

/// The fraction `SetScrollPercent(horizontal, vertical)` asks for on the
/// container's axis; `None` for `NO_SCROLL` (leave it).
pub(crate) fn scroll_target(s: &A11yScroll, horizontal: f64, vertical: f64) -> Option<f64> {
    let p = if s.horizontal { horizontal } else { vertical };
    (p >= 0.0).then(|| (p / 100.0).clamp(0.0, 1.0))
}

// ---- Text units ------------------------------------------------------------

/// A `TextUnit`, as the ranges here move by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unit<'a> {
    Character,
    /// A format run: where they start, with the text's length at the end
    /// (see [`format_bounds`]).
    Format(&'a [usize]),
    Word,
    Line,
    Document,
}

/// `TextUnit` (Character 0, Format 1, Word 2, Line 3, Paragraph 4, Page 5,
/// Document 6) as the unit it's served by: a format run by the text's
/// runs, starting at `format` (see [`format_bounds`]), a paragraph a line,
/// a page the document.
pub(crate) fn unit(id: i32, format: &[usize]) -> Unit<'_> {
    match id {
        1 => Unit::Format(format),
        2 => Unit::Word,
        3 | 4 => Unit::Line,
        5 | 6 => Unit::Document,
        _ => Unit::Character,
    }
}

#[derive(PartialEq)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// Where `text`'s units start, in order, with `len` at the end: a word
/// starts where a run of letters or of punctuation begins (its spaces belong
/// to the word before), a line after each newline.
pub(crate) fn boundaries(text: &[char], unit: Unit) -> Vec<usize> {
    let len = text.len();
    let mut out = vec![0];
    match unit {
        Unit::Character => out.extend(1..=len),
        Unit::Document => out.push(len),
        Unit::Format(starts) => {
            out.extend(starts.iter().map(|&x| x.min(len)));
            out.push(len);
            out.sort_unstable();
        }
        Unit::Word => {
            for i in 1..len {
                let c = class(text[i]);
                if c != Class::Space && c != class(text[i - 1]) {
                    out.push(i);
                }
            }
            out.push(len);
        }
        Unit::Line => {
            for i in 1..len {
                if text[i - 1] == '\n' {
                    out.push(i);
                }
            }
            out.push(len);
        }
    }
    out.dedup();
    out
}

/// The unit around `pos`: from the last start at or before it to the next
/// one (at the end of the text, the last unit).
pub(crate) fn enclosing(text: &[char], pos: usize, unit: Unit) -> (usize, usize) {
    let b = boundaries(text, unit);
    let len = text.len();
    let pos = pos.min(len);
    let i = match b.iter().rposition(|&x| x <= pos) {
        Some(i) if b[i] == len && i > 0 => i - 1,
        Some(i) => i,
        None => 0,
    };
    (b[i], b.get(i + 1).copied().unwrap_or(len))
}

/// `ExpandToEnclosingUnit` on `[start, end)`: the unit `start` is in (the
/// range stays at least that unit; a range longer than one unit is cut to
/// the first, as Win32's edit controls do).
pub(crate) fn expand(text: &[char], start: usize, unit: Unit) -> (usize, usize) {
    enclosing(text, start, unit)
}

/// `Move(unit, count)` on `[start, end)`: the range moved by `count` units
/// (back for a negative count), as the unit there, or degenerate when it
/// was. Returns the new range and how many units it moved.
pub(crate) fn move_range(
    text: &[char],
    (start, end): (usize, usize),
    unit: Unit,
    count: i32,
) -> ((usize, usize), i32) {
    let b = boundaries(text, unit);
    let len = text.len();
    let degenerate = start == end;
    let (s, _) = enclosing(text, start, unit);
    let mut i = b.iter().position(|&x| x == s).unwrap_or(0);
    // The last start a range can move to: the end of the text for a
    // degenerate one, else the last unit's start.
    let last = if degenerate || len == 0 {
        b.len() - 1
    } else {
        b.len().saturating_sub(2)
    };
    let mut moved = 0;
    while moved < count && i < last {
        i += 1;
        moved += 1;
    }
    while moved > count && i > 0 {
        i -= 1;
        moved -= 1;
    }
    let at = b[i];
    if degenerate {
        ((at, at), moved)
    } else {
        ((at, b.get(i + 1).copied().unwrap_or(len)), moved)
    }
}

/// Move the position `pos` across `count` unit boundaries (back for a
/// negative count). Returns where it lands and how many it crossed.
pub(crate) fn move_point(text: &[char], pos: usize, unit: Unit, count: i32) -> (usize, i32) {
    let b = boundaries(text, unit);
    let mut pos = pos.min(text.len());
    let mut moved = 0;
    while moved < count {
        match b.iter().find(|&&x| x > pos) {
            Some(&x) => pos = x,
            None => break,
        }
        moved += 1;
    }
    while moved > count {
        match b.iter().rev().find(|&&x| x < pos) {
            Some(&x) => pos = x,
            None => break,
        }
        moved -= 1;
    }
    (pos, moved)
}

/// `MoveEndpointByUnit`: move the start (`endpoint` 0) or the end (1) of
/// `[start, end)`; an endpoint moved past the other drags it along.
pub(crate) fn move_endpoint(
    text: &[char],
    (start, end): (usize, usize),
    endpoint: i32,
    unit: Unit,
    count: i32,
) -> ((usize, usize), i32) {
    let at = if endpoint == 0 { start } else { end };
    let (to, moved) = move_point(text, at, unit, count);
    (set_endpoint((start, end), endpoint, to), moved)
}

/// The range with its `endpoint` at `to`, the other end following when
/// they'd cross.
pub(crate) fn set_endpoint(
    (start, end): (usize, usize),
    endpoint: i32,
    to: usize,
) -> (usize, usize) {
    if endpoint == 0 {
        (to, end.max(to))
    } else {
        (start.min(to), to)
    }
}

/// The boxes `[x, y, w, h]` of `[start, end)` in a single line of `len`
/// characters filling `bounds` (each character an even share of its width;
/// the glyphs aren't known here). None for an empty range.
pub(crate) fn range_boxes(
    bounds: (f64, f64, f64, f64),
    len: usize,
    (start, end): (usize, usize),
) -> Option<[f64; 4]> {
    if end <= start || len == 0 {
        return None;
    }
    let (x, y, w, h) = bounds;
    let per = w / len as f64;
    Some([x + per * start as f64, y, per * (end - start) as f64, h])
}

/// `FindText`: where `needle` is in `hay[start..end]`, first (or last, going
/// `backward`), optionally ignoring case.
pub(crate) fn find_text(
    hay: &[char],
    (start, end): (usize, usize),
    needle: &str,
    backward: bool,
    ignore_case: bool,
) -> Option<(usize, usize)> {
    let fold = |c: char| {
        if ignore_case {
            c.to_lowercase().next().unwrap_or(c)
        } else {
            c
        }
    };
    let n: Vec<char> = needle.chars().map(fold).collect();
    let end = end.min(hay.len());
    if n.is_empty() || end < start || end - start < n.len() {
        return None;
    }
    let at = |i: usize| (0..n.len()).all(|k| fold(hay[i + k]) == n[k]);
    let mut starts = start..=end - n.len();
    let found = if backward {
        starts.rev().find(|&i| at(i))
    } else {
        starts.find(|&i| at(i))
    };
    found.map(|i| (i, i + n.len()))
}

// ---- Events ------------------------------------------------------------

/// A property's value, as a VARIANT carries it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Val {
    Bool(bool),
    I4(i32),
    R8(f64),
    Str(String),
}

/// One UIA event a tree swap raises, on the node at `path` of the new tree.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UiaEvent {
    /// `UiaRaiseAutomationPropertyChangedEvent(property, old, new)`.
    Property {
        path: Vec<usize>,
        property: i32,
        old: Val,
        new: Val,
    },
    /// `UiaRaiseStructureChangedEvent(ChildrenInvalidated)` on the parent.
    ChildrenInvalidated { path: Vec<usize> },
    /// `SelectionItem_ElementSelectedEvent`: the item became selected.
    ElementSelected { path: Vec<usize> },
    /// `UiaRaiseNotificationEvent`: an alert (a toast) to read out.
    Notification { path: Vec<usize>, text: String },
    /// `UiaRaiseAutomationEvent(id)`: a text pattern's selection or text
    /// changed.
    Automation { path: Vec<usize>, id: i32 },
}

/// The events going from `old` to `new` (the node pairs `changes` matches by
/// place), then a notification for each alert (toast) that appeared.
pub(crate) fn events(old: &A11yNode, new: &A11yNode) -> Vec<UiaEvent> {
    let mut out = Vec::new();
    let node = |t: &A11yNode, p: &[usize]| t.at_path(p).cloned();
    for change in changes(old, new) {
        let prop = |path: &Vec<usize>, property, f: &dyn Fn(&A11yNode, &A11yNode) -> Val| {
            let (a, b) = (node(old, path)?, node(new, path)?);
            Some(UiaEvent::Property {
                path: path.clone(),
                property,
                old: f(old, &a),
                new: f(new, &b),
            })
        };
        match &change {
            A11yChange::Name(p) => {
                // A field whose name is its text: its Value changed, below.
                if new.at_path(p).is_some_and(|n| !name_is_text(n)) {
                    out.extend(prop(p, NAME, &|t, n| Val::Str(uia_name(t, n))));
                }
                // The text a Text pattern serves changed.
                if new.at_path(p).is_some_and(|n| patterns(n).text) {
                    out.push(UiaEvent::Automation {
                        path: p.clone(),
                        id: TEXT_CHANGED,
                    });
                }
            }
            A11yChange::Value(p) => {
                out.extend(prop(p, VALUE_VALUE, &|_, n| Val::Str(uia_value(n))));
            }
            A11yChange::Checked(p) => {
                out.extend(prop(p, TOGGLE_STATE, &|_, n| Val::I4(toggle_state(n))));
            }
            A11yChange::Expanded(p) => {
                out.extend(prop(p, EXPAND_COLLAPSE_STATE, &|_, n| {
                    Val::I4(expand_state(n))
                }));
            }
            A11yChange::Selected(p) => {
                out.extend(prop(p, SELECTION_ITEM_IS_SELECTED, &|_, n| {
                    Val::Bool(n.selected)
                }));
                if new.at_path(p).is_some_and(|n| n.selected) {
                    out.push(UiaEvent::ElementSelected { path: p.clone() });
                }
            }
            A11yChange::Scrolled(p) => {
                let percent = |n: &A11yNode, horizontal: bool| {
                    let (h, v) = n
                        .scroll
                        .as_ref()
                        .map_or((NO_SCROLL, NO_SCROLL), scroll_percent);
                    Val::R8(if horizontal { h } else { v })
                };
                let horizontal = new
                    .at_path(p)
                    .and_then(|n| n.scroll)
                    .is_some_and(|s| s.horizontal);
                let property = if horizontal {
                    SCROLL_HORIZONTAL_PERCENT
                } else {
                    SCROLL_VERTICAL_PERCENT
                };
                out.extend(prop(p, property, &move |_, n| percent(n, horizontal)));
            }
            A11yChange::Children(p) => {
                out.push(UiaEvent::ChildrenInvalidated { path: p.clone() });
            }
            A11yChange::TextSelection(p) => {
                if new.at_path(p).is_some_and(|n| patterns(n).text) {
                    out.push(UiaEvent::Automation {
                        path: p.clone(),
                        id: TEXT_SELECTION_CHANGED,
                    });
                }
            }
            A11yChange::Range(p) => {
                out.extend(prop(p, RANGE_VALUE_VALUE, &|_, n| {
                    Val::R8(n.range.map_or(0.0, |r| r.value))
                }));
            }
        }
    }
    for (path, text) in new_alerts(old, new) {
        out.push(UiaEvent::Notification { path, text });
    }
    out
}

/// The alerts in `new` that `old` didn't have (see
/// [`A11yNode::alerts_new_since`]), with the path to each one.
fn new_alerts(old: &A11yNode, new: &A11yNode) -> Vec<(Vec<usize>, String)> {
    fn alerts(n: &A11yNode, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, String)>) {
        if n.role == A11yRole::Alert && !n.name.is_empty() {
            out.push((path.clone(), n.name.clone()));
        }
        for (i, c) in n.children.iter().enumerate() {
            path.push(i);
            alerts(c, path, out);
            path.pop();
        }
    }
    let mut all = Vec::new();
    alerts(new, &mut Vec::new(), &mut all);
    let mut wanted = new.alerts_new_since(Some(old));
    // The last of each name is the new one (an alert there before keeps
    // its earlier place).
    let mut out = Vec::new();
    for (path, name) in all.into_iter().rev() {
        if let Some(i) = wanted.iter().rposition(|w| *w == name) {
            wanted.remove(i);
            out.push((path, name));
        }
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn leaf(role: A11yRole, name: &str) -> A11yNode {
        A11yNode::new(role, name, (0.0, 0.0, 100.0, 20.0))
    }

    fn scroll(offset: f64, horizontal: bool) -> A11yScroll {
        A11yScroll {
            offset,
            max: 200.0,
            viewport: 100.0,
            horizontal,
            rtl: false,
        }
    }

    #[test]
    fn patterns_follow_roles_and_state() {
        let p = patterns(&leaf(A11yRole::CheckBox, "Bold"));
        assert!(p.toggle && p.invoke && !p.value);
        let p = patterns(&leaf(A11yRole::TextField, "x"));
        assert!(p.value && p.text && !p.invoke);
        let item = A11yNode {
            expanded: Some(false),
            ..leaf(A11yRole::TreeItem, "src")
        };
        let p = patterns(&item);
        assert!(p.selection_item && p.expand_collapse);
        assert!(patterns(&leaf(A11yRole::ComboBox, "a")).expand_collapse);
        assert!(patterns(&leaf(A11yRole::List, "")).selection);
        let pane = A11yNode {
            scroll: Some(scroll(0.0, false)),
            ..leaf(A11yRole::Group, "")
        };
        assert!(patterns(&pane).scroll);
        assert_eq!(patterns(&leaf(A11yRole::Group, "")), Patterns::default());
        assert_eq!(expand_state(&leaf(A11yRole::TreeItem, "leaf")), 3);
        assert_eq!(expand_state(&item), 0);
    }

    #[test]
    fn a_field_is_named_by_its_caption_and_valued_by_its_text() {
        let field = A11yNode {
            caret: Some(2),
            labelled_by: Some("cap".into()),
            ..leaf(A11yRole::TextField, "hi")
        };
        let root = A11yNode {
            children: vec![
                A11yNode {
                    id: Some("cap".into()),
                    ..leaf(A11yRole::Text, "Note")
                },
                field.clone(),
            ],
            ..leaf(A11yRole::Window, "")
        };
        assert_eq!(uia_name(&root, &field), "Note");
        assert_eq!(uia_value(&field), "hi");
        // No caption: no name; its text is its value.
        let bare = A11yNode {
            labelled_by: None,
            ..field.clone()
        };
        assert_eq!(uia_name(&root, &bare), "");
        // A field the tree has no caret for keeps its name.
        let labelled = leaf(A11yRole::TextField, "Name");
        assert_eq!(uia_name(&root, &labelled), "Name");
        let password = A11yNode {
            caret: Some(0),
            ..leaf(A11yRole::PasswordField, "")
        };
        assert_eq!(uia_value(&password), "");
        assert!(text_of(&password).is_empty());
    }

    #[test]
    fn selection_in_characters() {
        let field = A11yNode {
            caret: Some(6),
            text_selection: Some((1, 6)),
            ..leaf(A11yRole::TextField, "héllo")
        };
        assert_eq!(selection_of(&field), Some((1, 5)));
        let caret = A11yNode {
            text_selection: None,
            caret: Some(3),
            ..field.clone()
        };
        assert_eq!(selection_of(&caret), Some((2, 2)));
        assert_eq!(byte_at(&field, 2), 3);
        assert_eq!(byte_at(&field, 9), 6);
        assert_eq!(selection_of(&leaf(A11yRole::TextField, "x")), None);
    }

    #[test]
    fn scroll_percents_and_amounts() {
        let v = scroll(50.0, false);
        assert_eq!(scroll_percent(&v), (NO_SCROLL, 25.0));
        let (vh, vv) = view_size(&v);
        assert_eq!(vh, 100.0);
        assert!((vv - 100.0 / 3.0).abs() < 1e-9);
        let h = scroll(200.0, true);
        assert_eq!(scroll_percent(&h), (100.0, NO_SCROLL));
        let fits = A11yScroll { max: 0.0, ..v };
        assert_eq!(scroll_percent(&fits), (NO_SCROLL, NO_SCROLL));
        assert_eq!(scroll_request(&v, 2, 3), 80.0);
        assert_eq!(scroll_request(&v, 3, 1), -40.0);
        assert_eq!(scroll_request(&h, 0, 2), -80.0);
        assert_eq!(scroll_target(&v, NO_SCROLL, 50.0), Some(0.5));
        assert_eq!(scroll_target(&v, 30.0, NO_SCROLL), None);
        assert_eq!(scroll_target(&h, 150.0, NO_SCROLL), Some(1.0));
    }

    #[test]
    fn word_and_line_boundaries() {
        let t = chars("Hello, big world");
        assert_eq!(boundaries(&t, Unit::Word), vec![0, 5, 7, 11, 16]);
        assert_eq!(enclosing(&t, 8, Unit::Word), (7, 11));
        assert_eq!(
            enclosing(&t, 16, Unit::Word),
            (11, 16),
            "the end: the last word"
        );
        assert_eq!(enclosing(&t, 3, Unit::Character), (3, 4));
        assert_eq!(enclosing(&t, 3, Unit::Line), (0, 16));
        assert_eq!(expand(&t, 2, Unit::Document), (0, 16));
        let lines = chars("ab\ncd\n");
        assert_eq!(boundaries(&lines, Unit::Line), vec![0, 3, 6]);
        assert_eq!(enclosing(&lines, 4, Unit::Line), (3, 6));
        assert_eq!(unit(0, &[]), Unit::Character);
        assert_eq!(unit(1, &[0, 3]), Unit::Format(&[0, 3]));
        assert_eq!(unit(4, &[]), Unit::Line);
        assert_eq!(unit(5, &[]), Unit::Document);
        let empty: Vec<char> = Vec::new();
        assert_eq!(enclosing(&empty, 0, Unit::Word), (0, 0));
    }

    #[test]
    fn moving_ranges_and_endpoints() {
        let t = chars("one two three");
        // A word range moved on by one word, and back past the start.
        assert_eq!(move_range(&t, (0, 4), Unit::Word, 1), ((4, 8), 1));
        assert_eq!(move_range(&t, (4, 8), Unit::Word, -5), ((0, 4), -1));
        // Not past the last word; a degenerate range reaches the end.
        assert_eq!(move_range(&t, (0, 4), Unit::Word, 9), ((8, 13), 2));
        assert_eq!(move_range(&t, (0, 0), Unit::Word, 9), ((13, 13), 3));
        assert_eq!(move_range(&t, (5, 5), Unit::Character, 2), ((7, 7), 2));
        // Endpoints cross boundaries; one moved past the other drags it.
        assert_eq!(
            move_endpoint(&t, (0, 0), 1, Unit::Character, 3),
            ((0, 3), 3)
        );
        assert_eq!(move_endpoint(&t, (0, 3), 0, Unit::Word, 2), ((8, 8), 2));
        assert_eq!(
            move_endpoint(&t, (4, 8), 0, Unit::Character, -9),
            ((0, 8), -4)
        );
        assert_eq!(
            move_endpoint(&t, (0, 13), 1, Unit::Document, 1),
            ((0, 13), 0)
        );
        assert_eq!(set_endpoint((4, 8), 1, 2), (2, 2));
    }

    #[test]
    fn boxes_and_find() {
        assert_eq!(
            range_boxes((10.0, 5.0, 100.0, 20.0), 10, (2, 5)),
            Some([30.0, 5.0, 30.0, 20.0])
        );
        assert_eq!(range_boxes((0.0, 0.0, 1.0, 1.0), 10, (3, 3)), None);
        let t = chars("Abc abc");
        assert_eq!(find_text(&t, (0, 7), "abc", false, false), Some((4, 7)));
        assert_eq!(find_text(&t, (0, 7), "abc", false, true), Some((0, 3)));
        assert_eq!(find_text(&t, (0, 7), "ABC", true, true), Some((4, 7)));
        assert_eq!(find_text(&t, (1, 6), "abc", false, true), None);
    }

    #[test]
    fn a_swap_raises_property_structure_and_notification_events() {
        let tree = |text: &str, on: bool, offset: f64, alert: bool| {
            let mut children = vec![
                A11yNode {
                    caret: Some(text.len()),
                    ..leaf(A11yRole::TextField, text)
                },
                A11yNode {
                    checked: Some(on),
                    ..leaf(A11yRole::CheckBox, "Remember")
                },
                A11yNode {
                    scroll: Some(scroll(offset, false)),
                    children: vec![A11yNode {
                        selected: on,
                        ..leaf(A11yRole::ListItem, "Row 0")
                    }],
                    ..leaf(A11yRole::Group, "")
                },
            ];
            if alert {
                children.push(leaf(A11yRole::Alert, "Saved"));
            }
            A11yNode {
                children,
                ..leaf(A11yRole::Window, "")
            }
        };
        let old = tree("a", false, 0.0, false);
        assert!(events(&old, &old).is_empty());
        let new = tree("ab", true, 100.0, true);
        assert_eq!(
            events(&old, &new),
            vec![
                UiaEvent::ChildrenInvalidated { path: vec![] },
                UiaEvent::Automation {
                    path: vec![0],
                    id: TEXT_CHANGED
                },
                UiaEvent::Property {
                    path: vec![0],
                    property: VALUE_VALUE,
                    old: Val::Str("a".into()),
                    new: Val::Str("ab".into())
                },
                UiaEvent::Automation {
                    path: vec![0],
                    id: TEXT_SELECTION_CHANGED
                },
                UiaEvent::Property {
                    path: vec![1],
                    property: TOGGLE_STATE,
                    old: Val::I4(0),
                    new: Val::I4(1)
                },
                UiaEvent::Property {
                    path: vec![2],
                    property: SCROLL_VERTICAL_PERCENT,
                    old: Val::R8(0.0),
                    new: Val::R8(50.0)
                },
                UiaEvent::Property {
                    path: vec![2, 0],
                    property: SELECTION_ITEM_IS_SELECTED,
                    old: Val::Bool(false),
                    new: Val::Bool(true)
                },
                UiaEvent::ElementSelected { path: vec![2, 0] },
                UiaEvent::Notification {
                    path: vec![3],
                    text: "Saved".into()
                },
            ]
        );
        // A label renamed is a Name change.
        let a = A11yNode {
            children: vec![leaf(A11yRole::Text, "Rows tapped: 0")],
            ..leaf(A11yRole::Window, "")
        };
        let b = A11yNode {
            children: vec![leaf(A11yRole::Text, "Rows tapped: 1")],
            ..leaf(A11yRole::Window, "")
        };
        assert_eq!(
            events(&a, &b),
            vec![
                UiaEvent::Property {
                    path: vec![0],
                    property: NAME,
                    old: Val::Str("Rows tapped: 0".into()),
                    new: Val::Str("Rows tapped: 1".into())
                },
                UiaEvent::Automation {
                    path: vec![0],
                    id: TEXT_CHANGED
                },
            ]
        );
    }

    #[test]
    fn ranges_tables_and_text_attributes() {
        use crate::platform::access::{A11yCell, A11yFont, A11yRange, A11yTable};
        let slider = |value| A11yNode {
            range: Some(A11yRange {
                value,
                min: 0.0,
                max: 1.0,
                step: 0.05,
                rtl: false,
            }),
            ..leaf(A11yRole::Slider, "Volume")
        };
        let p = patterns(&slider(0.5));
        assert!(p.range_value && !p.value && !p.grid);
        assert_eq!(
            range_numbers(&slider(0.5)),
            Some((0.5, 0.0, 1.0, 0.05, 0.1))
        );
        let table = A11yNode {
            table: Some(A11yTable {
                rows: 3,
                columns: 2,
                frozen: 0,
            }),
            ..leaf(A11yRole::Table, "")
        };
        assert!(patterns(&table).grid);
        let cell = A11yNode {
            cell: Some(A11yCell { row: 1, column: 0 }),
            ..leaf(A11yRole::Cell, "x")
        };
        assert!(patterns(&cell).grid_item);
        assert!(!patterns(&leaf(A11yRole::Cell, "x")).grid_item);

        // A slider moved: its RangeValue.Value changed.
        let window = |n| A11yNode {
            children: vec![n],
            ..leaf(A11yRole::Window, "")
        };
        assert_eq!(
            events(&window(slider(0.5)), &window(slider(0.75))),
            vec![UiaEvent::Property {
                path: vec![0],
                property: RANGE_VALUE_VALUE,
                old: Val::R8(0.5),
                new: Val::R8(0.75)
            }]
        );

        let text = A11yNode {
            font: Some(A11yFont {
                family: Some("Segoe UI".into()),
                size: 14.0,
                weight: 700,
                italic: true,
            }),
            ..leaf(A11yRole::Text, "Bold")
        };
        assert_eq!(
            text_attribute(&text, FONT_NAME),
            Some(Val::Str("Segoe UI".into()))
        );
        assert_eq!(text_attribute(&text, FONT_SIZE), Some(Val::R8(10.5)));
        assert_eq!(text_attribute(&text, FONT_WEIGHT), Some(Val::I4(700)));
        assert_eq!(text_attribute(&text, IS_ITALIC), Some(Val::Bool(true)));
        assert_eq!(text_attribute(&text, IS_READ_ONLY), Some(Val::Bool(true)));
        assert_eq!(text_attribute(&text, 40008), None, "not one it knows");
        assert_eq!(text_attribute(&leaf(A11yRole::Text, "x"), FONT_NAME), None);
    }

    /// "Rich bold link bonjour": plain, bold, plain, a blue underlined
    /// link, plain, italic French.
    fn rich() -> A11yNode {
        use crate::platform::access::{A11yFont, A11yRuns};
        let font = |weight, italic| A11yFont {
            family: Some("Segoe UI".into()),
            size: 16.0,
            weight,
            italic,
        };
        let plain = |s, e| A11yTextRun::plain((s, e), font(400, false));
        let runs = vec![
            plain(0, 5),
            A11yTextRun::plain((5, 9), font(700, false)),
            plain(9, 10),
            A11yTextRun {
                underline: true,
                color: Some([0x1a, 0x5f, 0xd0, 0xff]),
                link: Some("https://stipple.rs".into()),
                ..plain(10, 14)
            },
            plain(14, 15),
            A11yTextRun {
                language: Some("fr".into()),
                ..A11yTextRun::plain((15, 22), font(400, true))
            },
        ];
        A11yNode {
            font: Some(font(400, false)),
            runs: A11yRuns::fixed(runs),
            ..leaf(A11yRole::Text, "Rich bold link bonjour")
        }
    }

    #[test]
    fn text_attributes_by_run() {
        let n = rich();
        let lcid = |tag: &str| (tag == "fr").then_some(0x040c);
        let at = |attr, range| range_attribute(&n, attr, range, &lcid);
        let v = |v| Some(Attr::Val(v));
        assert_eq!(at(FONT_WEIGHT, (5, 9)), v(Val::I4(700)));
        assert_eq!(at(FONT_WEIGHT, (0, 5)), v(Val::I4(400)));
        assert_eq!(at(FONT_WEIGHT, (3, 7)), Some(Attr::Mixed));
        assert_eq!(at(FONT_WEIGHT, (6, 6)), v(Val::I4(700)), "the caret's run");
        assert_eq!(at(FONT_NAME, (0, 22)), v(Val::Str("Segoe UI".into())));
        assert_eq!(at(UNDERLINE_STYLE, (10, 14)), v(Val::I4(1)));
        assert_eq!(at(UNDERLINE_STYLE, (9, 14)), Some(Attr::Mixed));
        assert_eq!(at(STRIKETHROUGH_STYLE, (0, 22)), v(Val::I4(0)));
        assert_eq!(at(FOREGROUND_COLOR, (10, 14)), v(Val::I4(0x00d0_5f1a)));
        assert_eq!(at(FOREGROUND_COLOR, (0, 5)), None, "plain text says none");
        assert_eq!(at(BACKGROUND_COLOR, (0, 22)), None);
        assert_eq!(at(CULTURE, (15, 22)), v(Val::I4(0x040c)));
        assert_eq!(at(CULTURE, (10, 22)), Some(Attr::Mixed));
        assert_eq!(at(IS_ITALIC, (16, 18)), v(Val::Bool(true)));
        assert_eq!(at(IS_READ_ONLY, (0, 1)), v(Val::Bool(true)));
        assert_eq!(at(40013, (0, 1)), None, "not one it knows");

        let bold = |v: &Val| *v == Val::I4(700);
        let plain = |v: &Val| *v == Val::I4(400);
        let find = |range, backward, want: &dyn Fn(&Val) -> bool| {
            find_attribute(&n, FONT_WEIGHT, range, backward, want, &lcid)
        };
        assert_eq!(find((0, 22), false, &bold), Some((5, 9)));
        assert_eq!(find((6, 22), false, &bold), Some((6, 9)), "clipped");
        assert_eq!(find((9, 22), false, &bold), None);
        // Neighbouring runs that match join: weight 400 from 9 to the end.
        assert_eq!(find((0, 22), true, &plain), Some((9, 22)));
        assert_eq!(find((0, 22), false, &plain), Some((0, 5)));

        let b = format_bounds(&n);
        assert_eq!(b, vec![0, 5, 9, 10, 14, 15, 22]);
        let t = chars(&n.name);
        let f = unit(1, &b);
        assert_eq!(expand(&t, 6, f), (5, 9));
        assert_eq!(move_range(&t, (5, 9), f, 2), ((10, 14), 2));
        assert_eq!(move_endpoint(&t, (0, 0), 1, f, 1), ((0, 5), 1));
        // Text drawn alike is one run.
        let one = A11yNode {
            font: rich().font,
            ..leaf(A11yRole::Text, "abc")
        };
        assert_eq!(format_bounds(&one), vec![0, 3]);
    }
}
