//! A large read-only text: a log, a terminal's scrollback, a build's output.
//!
//! [`TextView`] holds the text as lines (appended to as it comes, up to an
//! optional cap that drops the oldest), a selection that can span any of
//! them, and a search with its matches. [`text_view`] shows it on the
//! virtualized list's engine: only the lines in (and just around) the
//! viewport are laid out, so a 10,000-line, megabyte log costs what a
//! screenful does, and appending a line doesn't lay the others out again.

use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::ops::Range;

mod regex;

use regex::Regex;
pub use regex::RegexError;

use super::virtual_list::{RowHeight, Rows, ScrollAlign, ScrollMemo, Viewport, Window};
use crate::core::{Cx, Element, KeyInput, Role};
use crate::geometry::Insets;
use crate::platform::CursorIcon;
use crate::platform::clipboard::{ClipboardContent, ClipboardKind};
use crate::render::{Color, Font, FontStyle, RichText, SpanStyle};

/// A place in a [`TextView`]: a line, and a byte offset into it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct LinePos {
    /// The line, from 0 at the top.
    pub line: usize,
    /// The byte offset in the line (at a character boundary).
    pub byte: usize,
}

impl LinePos {
    /// Byte `byte` of line `line`.
    pub fn new(line: usize, byte: usize) -> Self {
        Self { line, byte }
    }
}

/// One match of a [`TextView::find`]: its line and its bytes in the line.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LineMatch {
    /// The line it's on.
    pub line: usize,
    /// Its bytes in the line.
    pub range: Range<usize>,
}

/// How [`TextView::find`] matches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FindOptions {
    /// Match case exactly (by default, `Error` finds `error` and `ERROR`).
    pub case_sensitive: bool,
    /// Read the query as a regular expression (see
    /// [`regex`](Self::regex)).
    pub regex: bool,
}

impl FindOptions {
    /// Match case exactly, or not.
    pub fn case_sensitive(mut self, case_sensitive: bool) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    /// Read the query as a regular expression (`true`), matched within each
    /// line: literal characters, `.`, classes `[a-z]` and `[^...]`, `\d`
    /// `\w` `\s` and their negations `\D` `\W` `\S`, anchors `^` and `$`
    /// (the line's start and end), groups `(...)` with `|`, and `*`, `+`,
    /// `?`, `{m}`, `{m,}`, `{m,n}`, greedy or (followed by `?`) lazy. Case
    /// is ignored unless [`case_sensitive`](Self::case_sensitive). The
    /// engine runs in time linear in the text whatever the pattern (no
    /// backtracking). An invalid pattern finds nothing, and
    /// [`TextView::find_error`] says why (or use [`TextView::try_find`]).
    ///
    /// ```
    /// use stipple::widgets::{FindOptions, TextView};
    /// let mut log = TextView::from_text("took 12ms\nerror: took 340ms\n");
    /// let options = FindOptions::default().regex(true);
    /// assert_eq!(log.find(r"\d+ms", options), 2);
    /// assert_eq!(log.find("^error:.*?\\d", options), 1);
    /// assert_eq!(log.matches()[0].range, 0..13);
    /// assert!(log.try_find("(unclosed", options).is_err());
    /// ```
    pub fn regex(mut self, regex: bool) -> Self {
        self.regex = regex;
        self
    }
}

/// The text of a [`text_view`], kept in the app state: its lines, the
/// selection, and the search.
///
/// ```
/// use stipple::widgets::{FindOptions, LinePos, TextView};
/// let mut log = TextView::new().max_lines(10_000);
/// log.append_text("boot\nmount /data\n");
/// log.append_text("error: disk ");
/// log.append_text("full\n");
/// assert_eq!(log.len(), 3);
/// assert_eq!(log.line(2), Some("error: disk full"));
///
/// assert_eq!(log.find("DISK", FindOptions::default()), 1);
/// let m = log.find_next().unwrap();
/// assert_eq!((m.line, m.range.clone()), (2, 7..11));
///
/// log.select(LinePos::new(1, 6), LinePos::new(2, 5));
/// assert_eq!(log.selected_text().as_deref(), Some("/data\nerror"));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextView {
    lines: VecDeque<String>,
    /// Whether the last line is still open: the text appended so far didn't
    /// end it with a newline, so the next [`append_text`](Self::append_text)
    /// continues it.
    open: bool,
    max_lines: Option<usize>,
    /// How many lines the cap has ever dropped off the top.
    dropped: u64,
    /// The selection: where it started, and where it ends (the end a drag or
    /// Shift+arrows move).
    anchor: Option<LinePos>,
    caret: Option<LinePos>,
    query: Option<(String, FindOptions)>,
    /// The query compiled, in regex mode.
    pattern: Option<Regex>,
    /// Why the query isn't a valid regex.
    find_error: Option<RegexError>,
    matches: Vec<LineMatch>,
    current: Option<usize>,
    scroll: ScrollMemo,
    /// How many lines have each length, in characters: the widest sets how
    /// far the view scrolls sideways.
    widths: BTreeMap<usize, usize>,
    /// The sideways scrolling's bookkeeping.
    sideways: Sideways,
}

/// What a [`text_view`]'s builds keep about its sideways scrolling, behind
/// a shared reference. Bookkeeping, as [`ScrollMemo`] is: any two compare
/// equal.
#[derive(Clone, Debug, Default)]
struct Sideways {
    /// A part of a line (its index, and a byte range in it) for the next
    /// build to scroll into view.
    reveal: Cell<Option<(usize, usize, usize)>>,
    /// Whether the last build found the lines wider than the view: the rows
    /// then keep room below them for the sideways bar's gutter.
    overflows: Cell<bool>,
}

impl PartialEq for Sideways {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Sideways {}

impl TextView {
    /// An empty view.
    pub fn new() -> Self {
        Self::default()
    }

    /// A view of `text`, a line per `\n`.
    pub fn from_text(text: &str) -> Self {
        let mut view = Self::new();
        view.append_text(text);
        view
    }

    /// Keep at most `lines` lines, dropping the oldest as new ones come
    /// (`None`: keep them all). What's on screen stays put as lines drop off
    /// the top.
    pub fn max_lines(mut self, lines: impl Into<Option<usize>>) -> Self {
        self.set_max_lines(lines);
        self
    }

    /// Set the cap [`max_lines`](Self::max_lines) sets, dropping lines now if
    /// there are more.
    pub fn set_max_lines(&mut self, lines: impl Into<Option<usize>>) {
        self.max_lines = lines.into().map(|n| n.max(1));
        self.trim();
    }

    /// How many lines there are.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Line `i`, without its newline.
    pub fn line(&self, i: usize) -> Option<&str> {
        self.lines.get(i).map(String::as_str)
    }

    /// The lines, from the top.
    pub fn lines(&self) -> impl Iterator<Item = &str> + '_ {
        self.lines.iter().map(String::as_str)
    }

    /// All of the text, its lines joined by `\n`.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(l);
        }
        out
    }

    /// How many lines the [cap](Self::max_lines) has dropped off the top,
    /// ever: what an app keeping positions in the text subtracts from them.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Append `text` as it comes (a process's output, a socket's): it
    /// continues the last line unless that one was ended by a `\n`, and each
    /// `\n` in it ends a line. A `\r` before a `\n` is dropped.
    pub fn append_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut parts = text.split('\n').peekable();
        if let Some(first) = parts.next() {
            match self.lines.back_mut() {
                Some(last) if self.open => {
                    let before = char_count(last);
                    last.push_str(first);
                    self.resize_width(before, before + char_count(first));
                    let i = self.lines.len() - 1;
                    self.refind_line(i);
                }
                _ => self.push_raw(first.to_string()),
            }
        }
        // The text after its last `\n` (empty if it ends with one) is the
        // open line.
        while let Some(part) = parts.next() {
            let i = self.lines.len() - 1;
            if strip_cr(&mut self.lines[i]) {
                let after = char_count(&self.lines[i]);
                self.resize_width(after + 1, after);
            }
            self.refind_line(i);
            if parts.peek().is_none() && part.is_empty() {
                self.open = false;
                self.trim();
                return;
            }
            self.push_raw(part.to_string());
        }
        self.open = true;
        self.trim();
    }

    /// Append one line (a `\n` in it ends lines there too).
    pub fn push_line(&mut self, line: impl Into<String>) {
        let line = line.into();
        self.open = false;
        for part in line.split('\n') {
            let mut part = part.to_string();
            strip_cr(&mut part);
            self.push_raw(part);
        }
        self.trim();
    }

    /// Append each of `lines`.
    pub fn append<L: Into<String>>(&mut self, lines: impl IntoIterator<Item = L>) {
        for line in lines {
            self.push_line(line);
        }
    }

    /// Remove all the text, the selection and the matches (the search stays,
    /// and finds in what's appended next).
    pub fn clear(&mut self) {
        self.lines.clear();
        self.open = false;
        self.anchor = None;
        self.caret = None;
        self.matches.clear();
        self.current = None;
        self.widths.clear();
        self.scroll.reset();
    }

    fn push_raw(&mut self, line: String) {
        *self.widths.entry(char_count(&line)).or_default() += 1;
        self.lines.push_back(line);
        let i = self.lines.len() - 1;
        self.find_in_line(i);
    }

    /// A line went from `before` characters to `after`.
    fn resize_width(&mut self, before: usize, after: usize) {
        remove_width(&mut self.widths, before);
        *self.widths.entry(after).or_default() += 1;
    }

    /// The length of the longest line, in characters.
    pub(crate) fn widest(&self) -> usize {
        self.widths.keys().next_back().copied().unwrap_or(0)
    }

    /// Drop the oldest lines over the cap, moving what refers to lines.
    fn trim(&mut self) {
        let Some(max) = self.max_lines else { return };
        let n = self.lines.len().saturating_sub(max);
        if n == 0 {
            return;
        }
        for line in self.lines.drain(..n) {
            remove_width(&mut self.widths, char_count(&line));
        }
        self.dropped += n as u64;
        let shift = |p: Option<LinePos>| {
            p.map(|p| {
                if p.line < n {
                    LinePos::new(0, 0)
                } else {
                    LinePos::new(p.line - n, p.byte)
                }
            })
        };
        self.anchor = shift(self.anchor);
        self.caret = shift(self.caret);
        let gone = self.matches.partition_point(|m| m.line < n);
        self.matches.drain(..gone);
        for m in &mut self.matches {
            m.line -= n;
        }
        self.current = match self.current {
            Some(c) if c >= gone => Some(c - gone),
            _ => None,
        };
    }

    // --- Selection ---------------------------------------------------------

    /// The selection, from its start to its end (in text order), if
    /// something is selected.
    pub fn selection(&self) -> Option<(LinePos, LinePos)> {
        let (a, c) = (self.anchor?, self.caret?);
        (a != c).then(|| (a.min(c), a.max(c)))
    }

    /// Select from `from` to `to` (either order; clamped to the text).
    pub fn select(&mut self, from: LinePos, to: LinePos) {
        self.anchor = Some(self.clamp(from));
        self.caret = Some(self.clamp(to));
    }

    /// Select all the text.
    pub fn select_all(&mut self) {
        if let Some(last) = self.lines.len().checked_sub(1) {
            self.anchor = Some(LinePos::new(0, 0));
            self.caret = Some(LinePos::new(last, self.lines[last].len()));
        }
    }

    /// Select line `line`, whole.
    pub fn select_line(&mut self, line: usize) {
        if let Some(text) = self.lines.get(line) {
            let end = text.len();
            self.select(LinePos::new(line, 0), LinePos::new(line, end));
        }
    }

    /// Deselect.
    pub fn clear_selection(&mut self) {
        self.anchor = None;
        self.caret = None;
    }

    /// The selected text, its lines joined by `\n`.
    pub fn selected_text(&self) -> Option<String> {
        let (s, e) = self.selection()?;
        let mut out = String::new();
        let lines = self.lines.range(s.line..=e.line);
        for (line, text) in (s.line..).zip(lines) {
            let from = if line == s.line { s.byte } else { 0 };
            let to = if line == e.line { e.byte } else { text.len() };
            if line > s.line {
                out.push('\n');
            }
            out.push_str(&text[from..to]);
        }
        Some(out)
    }

    /// The part of line `line` that is selected, if any of it is.
    fn selected_in(&self, line: usize) -> Option<Range<usize>> {
        let (s, e) = self.selection()?;
        if line < s.line || line > e.line {
            return None;
        }
        let len = self.lines[line].len();
        let from = if line == s.line { s.byte } else { 0 };
        let to = if line == e.line { e.byte } else { len };
        Some(from..to)
    }

    /// `p`, on the text and at a character boundary.
    fn clamp(&self, p: LinePos) -> LinePos {
        let Some(last) = self.lines.len().checked_sub(1) else {
            return LinePos::default();
        };
        let line = p.line.min(last);
        LinePos::new(line, floor_boundary(&self.lines[line], p.byte))
    }

    /// A press (`extend` false) or drag (true) at `p`.
    fn pointer(&mut self, p: LinePos, extend: bool) {
        let p = self.clamp(p);
        if !extend || self.anchor.is_none() {
            self.anchor = Some(p);
        }
        self.caret = Some(p);
    }

    /// Select the word around the caret (a double-click).
    fn select_word(&mut self) {
        let Some(c) = self.caret else { return };
        let Some(text) = self.lines.get(c.line) else {
            return;
        };
        let word = |ch: char| ch.is_alphanumeric() || ch == '_';
        let start = text[..c.byte]
            .char_indices()
            .rev()
            .take_while(|&(_, ch)| word(ch))
            .last()
            .map_or(c.byte, |(i, _)| i);
        let end = text[c.byte..]
            .char_indices()
            .find(|&(_, ch)| !word(ch))
            .map_or(text.len(), |(i, _)| c.byte + i);
        self.select(LinePos::new(c.line, start), LinePos::new(c.line, end));
    }

    /// Move the selection's end by a key (Shift+arrows, Shift+Home/End).
    fn extend(&mut self, key: &KeyInput) {
        let Some(last) = self.lines.len().checked_sub(1) else {
            return;
        };
        let c = self.caret.unwrap_or_default();
        let text = &self.lines[c.line];
        let to = match key {
            KeyInput::SelectLeft => match text[..c.byte].chars().next_back() {
                Some(ch) => LinePos::new(c.line, c.byte - ch.len_utf8()),
                None if c.line > 0 => LinePos::new(c.line - 1, self.lines[c.line - 1].len()),
                None => c,
            },
            KeyInput::SelectRight => match text[c.byte..].chars().next() {
                Some(ch) => LinePos::new(c.line, c.byte + ch.len_utf8()),
                None if c.line < last => LinePos::new(c.line + 1, 0),
                None => c,
            },
            KeyInput::SelectUp => LinePos::new(c.line.saturating_sub(1), c.byte),
            KeyInput::SelectDown => LinePos::new((c.line + 1).min(last), c.byte),
            KeyInput::SelectHome => LinePos::new(0, 0),
            KeyInput::SelectEnd => LinePos::new(last, self.lines[last].len()),
            _ => return,
        };
        if self.anchor.is_none() {
            self.anchor = Some(c);
        }
        let to = self.clamp(to);
        self.caret = Some(to);
        self.scroll.scroll_to(to.line, ScrollAlign::Nearest);
        self.sideways.reveal.set(Some((to.line, to.byte, to.byte)));
    }

    // --- Find --------------------------------------------------------------

    /// Find `query` in every line (and in lines appended from now on, until
    /// [`clear_find`](Self::clear_find)), highlighting the matches; returns
    /// how many there are. Matches don't overlap, nor span lines. An empty
    /// query finds nothing. With [`FindOptions::regex`] the query is a
    /// regular expression; an invalid one finds nothing, and
    /// [`find_error`](Self::find_error) says why.
    pub fn find(&mut self, query: &str, options: FindOptions) -> usize {
        self.query = None;
        self.pattern = None;
        self.find_error = None;
        self.matches.clear();
        self.current = None;
        if query.is_empty() {
            return 0;
        }
        if options.regex {
            match Regex::new(query, options.case_sensitive) {
                Ok(pattern) => self.pattern = Some(pattern),
                Err(e) => {
                    self.find_error = Some(e);
                    return 0;
                }
            }
        }
        self.query = Some((query.to_string(), options));
        for i in 0..self.lines.len() {
            self.find_in_line(i);
        }
        self.matches.len()
    }

    /// [`find`](Self::find), with an invalid regular expression as an error
    /// (nothing is then found).
    pub fn try_find(&mut self, query: &str, options: FindOptions) -> Result<usize, RegexError> {
        let found = self.find(query, options);
        match &self.find_error {
            Some(e) => Err(e.clone()),
            None => Ok(found),
        }
    }

    /// Why the last [`find`](Self::find)'s regular expression isn't valid,
    /// if it isn't: what to show next to the search field.
    pub fn find_error(&self) -> Option<&RegexError> {
        self.find_error.as_ref()
    }

    /// Stop searching: no matches, none highlighted.
    pub fn clear_find(&mut self) {
        self.query = None;
        self.pattern = None;
        self.find_error = None;
        self.matches.clear();
        self.current = None;
    }

    /// The matches of the search, in text order.
    pub fn matches(&self) -> &[LineMatch] {
        &self.matches
    }

    /// The match [`find_next`](Self::find_next) or
    /// [`find_previous`](Self::find_previous) went to, and its index in
    /// [`matches`](Self::matches).
    pub fn current_match(&self) -> Option<(usize, &LineMatch)> {
        let i = self.current?;
        Some((i, self.matches.get(i)?))
    }

    /// Go to the next match (the first, after the last or before any),
    /// scrolling it into view; returns it.
    pub fn find_next(&mut self) -> Option<LineMatch> {
        let n = self.matches.len();
        let next = match self.current {
            _ if n == 0 => return None,
            Some(c) => (c + 1) % n,
            None => 0,
        };
        self.go_to_match(next)
    }

    /// Go to the previous match (the last, before the first or before any),
    /// scrolling it into view; returns it.
    pub fn find_previous(&mut self) -> Option<LineMatch> {
        let n = self.matches.len();
        let prev = match self.current {
            _ if n == 0 => return None,
            Some(c) => (c + n - 1) % n,
            None => n - 1,
        };
        self.go_to_match(prev)
    }

    fn go_to_match(&mut self, i: usize) -> Option<LineMatch> {
        let m = self.matches.get(i)?.clone();
        self.current = Some(i);
        self.scroll.scroll_to(m.line, ScrollAlign::Nearest);
        self.sideways
            .reveal
            .set(Some((m.line, m.range.start, m.range.end)));
        Some(m)
    }

    /// Add line `i`'s matches (it's the last line, or matches are rebuilt
    /// in order).
    fn find_in_line(&mut self, i: usize) {
        let Some((query, options)) = &self.query else {
            return;
        };
        let found = match &self.pattern {
            Some(pattern) => pattern.find_all(&self.lines[i]),
            None => find_in(&self.lines[i], query, options.case_sensitive),
        };
        for range in found {
            self.matches.push(LineMatch { line: i, range });
        }
    }

    /// Find again in the last line `i`, which grew.
    fn refind_line(&mut self, i: usize) {
        if self.query.is_none() {
            return;
        }
        let keep = self.matches.partition_point(|m| m.line < i);
        self.matches.truncate(keep);
        if self.current.is_some_and(|c| c >= keep) {
            self.current = None;
        }
        self.find_in_line(i);
    }

    // --- Scrolling ---------------------------------------------------------

    /// Scroll to line `line`, placed as `align` says (from any handler; the
    /// next build makes the scroll).
    pub fn scroll_to_line(&mut self, line: usize, align: ScrollAlign) {
        self.scroll.scroll_to(line, align);
    }

    /// Scroll so `pos` is in view, down and sideways, as the selection's
    /// end and the matches [`find_next`](Self::find_next) goes to are.
    pub fn scroll_to(&mut self, pos: LinePos) {
        let pos = self.clamp(pos);
        self.scroll.scroll_to(pos.line, ScrollAlign::Nearest);
        self.sideways
            .reveal
            .set(Some((pos.line, pos.byte, pos.byte)));
    }

    /// Scroll to the end (where a view that
    /// [sticks to its end](TextViewOptions::stick_to_end) then follows it
    /// again).
    pub fn scroll_to_end(&mut self) {
        let last = self.lines.len().saturating_sub(1);
        self.scroll.scroll_to(last, ScrollAlign::End);
    }
}

/// The largest character boundary of `s` at or before `i`.
fn floor_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Drop a `\r` ending `line`, saying whether there was one.
fn strip_cr(line: &mut String) -> bool {
    let cr = line.ends_with('\r');
    if cr {
        line.pop();
    }
    cr
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

/// Count one line of `chars` characters fewer in `widths`.
fn remove_width(widths: &mut BTreeMap<usize, usize>, chars: usize) {
    if let Some(n) = widths.get_mut(&chars) {
        *n -= 1;
        if *n == 0 {
            widths.remove(&chars);
        }
    }
}

/// The non-overlapping matches of `query` in `line`.
fn find_in(line: &str, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    if case_sensitive {
        return line
            .match_indices(query)
            .map(|(i, m)| i..i + m.len())
            .collect();
    }
    let mut out = Vec::new();
    if query.is_ascii() {
        let (hay, q) = (line.as_bytes(), query.as_bytes());
        let mut i = 0;
        while i + q.len() <= hay.len() {
            if hay[i..i + q.len()].eq_ignore_ascii_case(q) {
                out.push(i..i + q.len());
                i += q.len();
            } else {
                i += 1;
            }
        }
        return out;
    }
    // Compared lowercased, a character at a time.
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let mut from = 0;
    while from < line.len() {
        let mut qi = 0;
        let mut end = None;
        'chars: for (j, ch) in line[from..].char_indices() {
            for lc in ch.to_lowercase() {
                if q.get(qi) != Some(&lc) {
                    break 'chars;
                }
                qi += 1;
            }
            if qi == q.len() {
                end = Some(from + j + ch.len_utf8());
                break;
            }
        }
        match end {
            Some(e) => {
                out.push(from..e);
                from = e;
            }
            None => from += line[from..].chars().next().map_or(1, char::len_utf8),
        }
    }
    out
}

/// How a [`text_view`] looks and behaves.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TextViewOptions {
    /// The viewport height; `None` (the default) fills the parent as
    /// [`scroll`](crate::widgets::scroll) does.
    pub height: Option<f64>,
    /// The text size; `None` is the theme's body size.
    pub text_size: Option<f64>,
    /// The height of a line; `None` is 1.4 times the text size, rounded up.
    pub line_height: Option<f64>,
    /// The font; `None` is the system's monospaced font (the app's font
    /// where there's none).
    pub font: Option<Font>,
    /// Open at the end, and stay there while scrolled there as lines are
    /// appended.
    pub stick_to_end: bool,
    /// The scroll container's key (see [`Cx::register_scroll_keyed`]).
    pub scroll_key: Option<u64>,
}

impl TextViewOptions {
    /// Fix the viewport height.
    pub fn height(mut self, height: f64) -> Self {
        self.height = Some(height);
        self
    }

    /// Set the text size.
    pub fn text_size(mut self, size: f64) -> Self {
        self.text_size = Some(size);
        self
    }

    /// Set the height of a line.
    pub fn line_height(mut self, height: f64) -> Self {
        self.line_height = Some(height);
        self
    }

    /// Show the text in `font`.
    pub fn font(mut self, font: Font) -> Self {
        self.font = Some(font);
        self
    }

    /// Follow the end: open there, and stay there while scrolled there as
    /// lines are appended (scrolling up stops following; back at the end it
    /// resumes).
    pub fn stick_to_end(mut self, stick: bool) -> Self {
        self.stick_to_end = stick;
        self
    }

    /// Keep the scroll position under `key` (see
    /// [`Cx::register_scroll_keyed`]).
    pub fn scroll_key(mut self, key: impl std::hash::Hash) -> Self {
        use std::hash::Hasher;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut h);
        self.scroll_key = Some(h.finish());
        self
    }
}

thread_local! {
    /// The monospaced system font, looked up on first need.
    static MONO: std::cell::RefCell<Option<Option<Font>>> = const { std::cell::RefCell::new(None) };
}

fn mono_font() -> Option<Font> {
    MONO.with(|m| {
        m.borrow_mut()
            .get_or_insert_with(|| Font::from_family("monospace", FontStyle::default()).ok())
            .clone()
    })
}

/// A virtualized, read-only view of a [`TextView`]'s lines: only the lines in
/// (and just around) the viewport are built and laid out, in a monospaced
/// font, one per row and unwrapped: the view scrolls sideways as far as the
/// longest line reaches (a horizontal wheel or trackpad, Shift+wheel,
/// Left/Right, a sideways finger drag, or the bar along its bottom).
///
/// Drag to select (across lines, including ones scrolled in as the pointer
/// passes an edge), double-click a word, triple-click a line, Shift+arrows
/// to extend; Ctrl/Cmd+C or the context menu copies, Ctrl/Cmd+A selects all,
/// Escape deselects. Up/Down scroll a line, Home/End to the top and end.
/// [`TextView::find`]'s matches are highlighted, the current one more; the
/// match gone to and the selection's moving end are scrolled into view,
/// down and sideways.
///
/// Screen readers read it as text, named by the lines on screen.
///
/// ```
/// use stipple::prelude::*;
/// use stipple::widgets::{TextView, TextViewOptions, text_view};
///
/// struct App { log: TextView }
/// fn view(state: &App, cx: &mut Cx<App>) -> Element {
///     column(vec![
///         heading(cx, "Log"),
///         text_view(cx, &state.log, |s| &mut s.log, TextViewOptions::default().stick_to_end(true)),
///     ])
/// }
/// # let theme = Theme::light();
/// # let app = App { log: TextView::from_text("one\ntwo") };
/// # let _ = view(&app, &mut Cx::new(&theme));
/// ```
pub fn text_view<S: 'static>(
    cx: &mut Cx<S>,
    state: &TextView,
    field: impl Fn(&mut S) -> &mut TextView + Copy + 'static,
    options: TextViewOptions,
) -> Element {
    let theme = &super::theme_of(cx);
    let size = options.text_size.unwrap_or(theme.typography.body);
    let lh = options.line_height.unwrap_or((size * 1.4).ceil()).max(1.0);
    let font = options.font.clone().or_else(mono_font);
    // The classic bars' gutter: the sideways bar's, below the rows, when the
    // lines overflow sideways.
    let gutter = theme.scroll_bar.gutter();
    let view = Viewport {
        height: options.height,
        overscan: super::virtual_list::DEFAULT_OVERSCAN,
        stick_to_end: options.stick_to_end,
        key: options.scroll_key,
        dropped: state.dropped,
        tail: if state.sideways.overflows.get() {
            gutter
        } else {
            0.0
        },
    };
    let rows = Rows::new(state.len(), RowHeight::Fixed(lh));
    let window = Window::open(cx, rows, &view, &state.scroll);
    // Lines aren't wrapped: the view scrolls sideways too.
    let x_id = match options.scroll_key {
        Some(key) => cx.register_scroll_keyed((key, "sideways")),
        None => cx.register_scroll(),
    };
    let x_off = cx.scroll_offset(x_id);
    let pad_x = theme.spacing.sm;
    let pad_y = ((lh - size * 1.25) / 2.0).max(0.0);
    let found = Color::rgba(
        theme.palette.warning.r,
        theme.palette.warning.g,
        theme.palette.warning.b,
        0x60,
    );
    let current = theme.palette.warning.with_alpha(0xc0);
    let current_match = state.current_match().map(|(i, _)| i);
    let mut first_match = state
        .matches
        .partition_point(|m| m.line < window.range.start);
    let mut lines = Vec::with_capacity(window.range.len());
    let mut visible = String::new();
    let (top, bottom) = (window.offset, window.offset + window.height);
    for i in window.range.clone() {
        let text = &state.lines[i];
        let row_top = i as f64 * lh;
        if row_top + lh > top + 0.5 && row_top < bottom - 0.5 {
            if !visible.is_empty() {
                visible.push('\n');
            }
            visible.push_str(text);
        }
        // What's highlighted: the selection over the current match over the
        // other matches.
        let mut marks: Vec<(Range<usize>, Color)> = Vec::new();
        while let Some(m) = state.matches.get(first_match).filter(|m| m.line == i) {
            let color = if current_match == Some(first_match) {
                current
            } else {
                found
            };
            marks.push((m.range.clone(), color));
            first_match += 1;
        }
        let selected = state.selected_in(i);
        // An empty line is a space, so it has a height to hit and a
        // selection shows on it.
        let shown = if text.is_empty() { " " } else { text.as_str() };
        let mut el = if marks.is_empty() && selected.is_none() {
            Element::text(shown, size, theme.palette.text)
        } else {
            let sel = match selected {
                // An empty line inside the selection shows its space selected.
                Some(_) if text.is_empty() => state
                    .selection()
                    .filter(|(_, end)| end.line != i)
                    .map(|_| 0..1),
                other => other.filter(|r| !r.is_empty()),
            };
            Element::rich_text(
                highlighted(shown, sel, theme.palette.selection, &marks),
                size,
                theme.palette.text,
            )
        };
        if let Some(f) = &font {
            el = el.font(f.clone());
        }
        lines.push(el.height(lh).padding(Insets::new(pad_y, pad_x, 0.0, pad_x)));
    }
    // How wide the lines are: the longest's characters at the font's
    // advance (exact in a monospaced font), or the lines built, measured,
    // when wider (wide characters, another font).
    let advance = match &font {
        Some(f) => (f.measure("0000000000", size).width / 10.0).max(0.1),
        None => size * 0.6,
    };
    let measure = |text: &str| match &font {
        Some(f) => f.measure(text, size).width,
        None => char_count(text) as f64 * advance,
    };
    let mut wide = state.widest() as f64 * advance;
    if font.is_some() {
        for i in window.range.clone() {
            wide = wide.max(measure(&state.lines[i]));
        }
    }
    // The vertical bar's gutter, when the rows overflow: the lines end left
    // of it.
    let beside = if window.rows.total() + view.tail > window.height + 0.5 {
        gutter
    } else {
        0.0
    };
    let content_width = (wide + 2.0 * pad_x).ceil() + beside;
    // The room the lines get: the viewport's width (last frame's) less its
    // hairline frame and the vertical bar.
    let room = cx
        .last_scroll_viewport(window.id)
        .map(|v| v.width - 2.0 - beside);
    if let Some(room) = room {
        state
            .sideways
            .overflows
            .set(content_width - beside > room + 0.5);
        // A match gone to, or the selection's end: scrolled into view
        // sideways, with a little room around it.
        if let Some((line, from, to)) = state.sideways.reveal.take()
            && let Some(text) = state.lines.get(line)
        {
            let (from, to) = (floor_boundary(text, from), floor_boundary(text, to));
            let start = pad_x + measure(&text[..from]);
            let end = pad_x + measure(&text[..to]);
            let margin = (advance * 4.0).min(room / 4.0);
            let to_x = if start - margin < x_off {
                Some(start - margin)
            } else if end + margin > x_off + room {
                // The end in view, unless that hides the start.
                Some((end + margin - room).min(start - margin))
            } else {
                None
            };
            if let Some(x) = to_x {
                window.requests.scroll_to(x_id, x.max(0.0));
            }
        }
    }
    let first = window.range.start;
    let count = state.len();
    let content =
        window
            .content(lines)
            .min_width(content_width)
            .on_text_hit(cx, move |s, hit, extend| {
                let st = field(s);
                let mut line = first + hit.leaf;
                let mut byte = hit.index;
                // Above or below the built lines (dragging out of the view):
                // whole lines from there, scrolled into view.
                if hit.point.y < 0.0 {
                    line = line.saturating_sub((-hit.point.y / lh).ceil() as usize);
                    byte = 0;
                } else if hit.point.y > lh {
                    line += (hit.point.y / lh) as usize;
                    byte = usize::MAX;
                }
                line = line.min(count.saturating_sub(1));
                st.pointer(LinePos::new(line, byte), extend);
                if extend {
                    // Dragged past an edge: what it reaches scrolls in.
                    st.scroll.scroll_to(line, ScrollAlign::Nearest);
                    if let Some(c) = st.caret {
                        st.sideways.reveal.set(Some((c.line, c.byte, c.byte)));
                    }
                }
            });
    let w = window.clone();
    let step = (advance * 4.0).ceil();
    let body = window
        .scroller(content)
        .scrollable_x(x_id)
        .fill(theme.palette.surface)
        .border(theme.palette.border, 1.0)
        .radius(theme.radius / 2.0)
        .padding(Insets::uniform(1.0))
        .cursor(CursorIcon::Text)
        .role(Role::Text)
        .a11y_label(visible);
    let menu = crate::core::menu::Menu::new("")
        .item(
            crate::core::menu::MenuItem::new("Copy", move |s: &mut S| copy(field(s)))
                .enabled(state.selection().is_some()),
        )
        .item(crate::core::menu::MenuItem::new(
            "Select All",
            move |s: &mut S| field(s).select_all(),
        ));
    body.context_menu(cx, menu).on_key(cx, move |s, key| {
        let st = field(s);
        match key {
            KeyInput::Copy | KeyInput::Cut => copy(st),
            KeyInput::SelectAll => st.select_all(),
            KeyInput::Escape => st.clear_selection(),
            KeyInput::SelectWord => st.select_word(),
            KeyInput::SelectLine => {
                if let Some(c) = st.caret {
                    st.select_line(c.line);
                }
            }
            KeyInput::Up => w.scroll_by(-lh),
            KeyInput::Down => w.scroll_by(lh),
            KeyInput::Left => w.requests.scroll_to(x_id, (x_off - step).max(0.0)),
            KeyInput::Right => w.requests.scroll_to(x_id, x_off + step),
            KeyInput::Home => st.scroll_to_line(0, ScrollAlign::Start),
            KeyInput::End => st.scroll_to_end(),
            KeyInput::SelectLeft
            | KeyInput::SelectRight
            | KeyInput::SelectUp
            | KeyInput::SelectDown
            | KeyInput::SelectHome
            | KeyInput::SelectEnd => st.extend(key),
            _ => {}
        }
    })
}

/// Put the selection on the clipboard.
fn copy(view: &TextView) {
    if let Some(text) = view.selected_text() {
        crate::core::write_clipboard(ClipboardKind::Clipboard, ClipboardContent::from_text(text));
    }
}

/// `text` with `selected` on a `selection` background over the `marks`
/// (ranges in text order, each on its color).
fn highlighted(
    text: &str,
    selected: Option<Range<usize>>,
    selection: Color,
    marks: &[(Range<usize>, Color)],
) -> RichText {
    let mut cuts = vec![0, text.len()];
    if let Some(r) = &selected {
        cuts.extend([r.start, r.end]);
    }
    for (r, _) in marks {
        cuts.extend([r.start, r.end]);
    }
    cuts.retain(|&c| c <= text.len());
    cuts.sort_unstable();
    cuts.dedup();
    let mut rich = RichText::new();
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let color = if selected
            .as_ref()
            .is_some_and(|r| r.start <= a && b <= r.end)
        {
            Some(selection)
        } else {
            marks
                .iter()
                .find(|(r, _)| r.start <= a && b <= r.end)
                .map(|(_, c)| *c)
        };
        rich = match color {
            Some(c) => rich.styled(&text[a..b], SpanStyle::new().background(c)),
            None => rich.push(&text[a..b]),
        };
    }
    rich
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appended_text_continues_the_open_line() {
        let mut v = TextView::new();
        v.append_text("a");
        v.append_text("b\nc");
        v.append_text("d\r\n");
        v.append_text("e\n\n");
        assert_eq!(v.lines().collect::<Vec<_>>(), vec!["ab", "cd", "e", ""]);
        v.push_line("f\ng");
        v.append_text("h");
        assert_eq!(
            v.lines().collect::<Vec<_>>(),
            vec!["ab", "cd", "e", "", "f", "g", "h"]
        );
        assert_eq!(v.text(), "ab\ncd\ne\n\nf\ng\nh");
    }

    #[test]
    fn the_cap_drops_the_oldest_and_moves_the_selection_and_matches() {
        let mut v = TextView::new().max_lines(3);
        v.find("x", FindOptions::default());
        v.append(["x1", "a", "x2"]);
        v.select(LinePos::new(1, 0), LinePos::new(2, 1));
        assert_eq!(v.find_next().unwrap().line, 0);
        assert_eq!(v.find_next().unwrap().line, 2);
        v.push_line("x3");
        assert_eq!(v.dropped(), 1);
        assert_eq!(v.lines().collect::<Vec<_>>(), vec!["a", "x2", "x3"]);
        assert_eq!(v.selected_text().as_deref(), Some("a\nx"));
        let lines: Vec<_> = v.matches().iter().map(|m| m.line).collect();
        assert_eq!(lines, vec![1, 2]);
        // The current match ("x2") moved with its line.
        assert_eq!(v.current_match().map(|(i, m)| (i, m.line)), Some((0, 1)));
    }

    #[test]
    fn find_matches_case_and_unicode_and_follows_appends() {
        let mut v = TextView::from_text("Error one\nno ERROR here\nerrorerror\n");
        assert_eq!(v.find("error", FindOptions::default()), 4);
        assert_eq!(
            v.find("Error", FindOptions::default().case_sensitive(true)),
            1
        );
        assert_eq!(v.find("ÉTÉ", FindOptions::default()), 0);
        v.append_text("l'été, ÉTÉ\n");
        assert_eq!(v.matches().len(), 2);
        assert_eq!(v.matches()[0].range, 2..7);
        // An open line found in again as it grows.
        v.find("abc", FindOptions::default());
        v.append_text("xa");
        assert_eq!(v.matches().len(), 0);
        v.append_text("bc");
        assert_eq!(v.matches().len(), 1);
        assert_eq!(v.matches()[0].range, 1..4);
        // Next and previous wrap around.
        let mut v = TextView::from_text("a\nb a\na");
        v.find("a", FindOptions::default());
        assert_eq!(v.find_previous().unwrap().line, 2);
        assert_eq!(v.find_next().unwrap().line, 0);
        assert_eq!(v.find_previous().unwrap().line, 2);
    }

    #[test]
    fn selections_span_lines_and_words() {
        let mut v = TextView::from_text("alpha beta\ngamma\ndelta");
        v.select(LinePos::new(2, 3), LinePos::new(0, 6));
        assert_eq!(v.selected_text().as_deref(), Some("beta\ngamma\ndel"));
        v.select_all();
        assert_eq!(v.selected_text().unwrap(), v.text());
        v.pointer(LinePos::new(0, 8), false);
        v.select_word();
        assert_eq!(v.selected_text().as_deref(), Some("beta"));
        v.extend(&KeyInput::SelectDown);
        assert_eq!(v.selected_text().as_deref(), Some("beta\ngamma"));
        v.clear_selection();
        assert_eq!(v.selection(), None);
    }

    #[test]
    fn highlights_split_at_their_edges() {
        let sel = Color::rgb(0, 0, 255);
        let mark = Color::rgb(255, 255, 0);
        let rich = highlighted("hello world", Some(3..7), sel, &[(6..11, mark)]);
        assert_eq!(rich.text(), "hello world");
        assert_eq!(rich.plain_text(), "hello world");
    }
}
