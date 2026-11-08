//! Stipple's self-drawn widget library.
//!
//! Every widget is composed from the `stipple::core` [`Element`] IR and drawn by
//! `oxideav-raster` — there are no native controls. The builders here read
//! their colors and metrics from a [`Theme`], so a single theme swap re-skins
//! the whole UI.
//!
//! Available widgets: structure (`column`, `row`, `grid`, `spacer`, `panel`), content
//! (`label`, `heading`, `swatch`, `divider`), and interactive controls —
//! `button_variant` (Primary/Secondary/Ghost/Danger) / `button_labeled`
//! ([`Element::on_tap`]), `text_field` / `text_editor` ([`Element::on_key`]; the
//! latter is caret-aware via [`EditBuffer`]), `checkbox`, `switch`, and `slider`
//! ([`Element::on_drag`]). All colors and metrics come from the [`Theme`], so
//! swapping or customizing the theme reskins everything.

#![forbid(unsafe_code)]

pub(crate) mod menus;
#[cfg(test)]
mod menus_tests;
mod number;
mod preedit;
mod select;
mod split;
mod toast;
mod toolbar;

pub use number::{NumberInput, number_input};
pub use preedit::DisplayText;
pub use select::{Select, select};
pub use split::{SplitPane, split_pane};
pub use toast::{Toast, ToastId, ToastKind, Toasts, toasts};
pub use toolbar::{tool_button, tool_icon_button, tool_separator, tool_toggle, toolbar};

use crate::core::{
    Align, Anchor, Axis, BoxStyle, Cx, Element, KeyInput, OverlaySpec, Role, Track, ViewportId,
};
use crate::geometry::{Insets, Point};
use crate::platform::native::{self, NativeControl};
use crate::render::{Color, ImageSource, Svg, TextDirection};
use crate::style::Theme;
use std::sync::Arc;

mod table;
mod tree;
mod virtual_list;

pub use table::{Column, SortOrder, TableSpec, TableState, table};
pub use tree::{TreeModel, TreeRow, TreeSpec, TreeState, tree};
pub use virtual_list::{
    DEFAULT_OVERSCAN, ListState, RowHeight, RowInfo, SelectionMode, VirtualList, virtual_list,
};

/// A vertical container.
/// Apply the size the OS gives `control`.
fn native_size(mut el: Element, control: NativeControl) -> Element {
    let (width, height) = native::size_hint(control);
    if let Some(width) = width {
        el = el.width(width);
    }
    if let Some(height) = height {
        el = el.height(height);
    }
    el
}

impl Element {
    /// Under the native theme, have the OS draw this text field's frame.
    fn native_text_frame(self, theme: &Theme) -> Self {
        match theme.native_deco(NativeControl::TextField) {
            Some(deco) => self.native(deco),
            None => self,
        }
    }
}

pub fn column(children: Vec<Element>) -> Element {
    Element::stack(Axis::Vertical, children)
}

/// A horizontal container.
pub fn row(children: Vec<Element>) -> Element {
    Element::stack(Axis::Horizontal, children)
}

/// A grid placing `children` in order, row by row, across `columns` tracks:
/// [`Track::px`] fixed, [`Track::fr`] fractional, or [`Track::Auto`]
/// (content-sized). Rows size to their content unless set with
/// [`Element::rows`]; cells stretch to fill by default.
///
/// ```
/// use stipple::prelude::*;
/// let form = grid(
///     vec![Track::Auto, Track::fr(1.0)],
///     vec![
///         Element::boxed(BoxStyle::default()).width(60.0).height(20.0),
///         Element::boxed(BoxStyle::default()).height(20.0),
///     ],
/// )
/// .gap(8.0);
/// # let _ = form;
/// ```
pub fn grid(columns: Vec<Track>, children: Vec<Element>) -> Element {
    Element::grid(columns, children)
}

/// `key` as a widget laid out left to right should read it: in a mirrored UI
/// (`rtl`) Left and Right swap, so they still point towards the end and the
/// start of the layout: Left expands a node in a right-to-left tree, and a
/// split divider still moves the way the arrow points.
pub(crate) fn logical_key(rtl: bool, key: &KeyInput) -> KeyInput {
    match key {
        KeyInput::Left if rtl => KeyInput::Right,
        KeyInput::Right if rtl => KeyInput::Left,
        _ => key.clone(),
    }
}

/// A flexible gap that pushes siblings apart (grow = 1).
pub fn spacer() -> Element {
    Element::stack(Axis::Horizontal, Vec::new()).grow(1.0)
}

/// A fixed-height vertical scroll viewport around `content`. Content taller than
/// `height` overflows and is clipped to the viewport; wheel events over it
/// scroll the app's offset (re-clamped each frame). Register the container once
/// per frame via `cx` so its scroll position is stable across rebuilds.
pub fn scroll<S>(cx: &mut Cx<S>, height: f64, content: Element) -> Element {
    let id = cx.register_scroll();
    Element::stack(Axis::Vertical, vec![content])
        .height(height)
        .scrollable(id)
        .align(Align::Start, Align::Stretch)
}

/// An embedded-content viewport of `width`×`height` logical pixels for the
/// given [`ViewportId`], framed with a hairline themed border. The app fills the
/// reserved rect with externally-rendered pixels (a browser page, video, or a
/// sandboxed content process's GPU surface) by locating it via
/// [`collect_viewports`](crate::core::collect_viewports); until then a neutral
/// placeholder shows. The `id` is caller-chosen so it stays stable across frames
/// and correlates with the content source.
pub fn viewport(theme: &Theme, id: ViewportId, width: f64, height: f64) -> Element {
    Element::viewport(id)
        .width(width)
        .height(height)
        .border(theme.palette.border, 1.0)
}

/// An image: a decoded bitmap (see [`Pixmap::decode`]) or an [`Svg`], shared
/// by `Arc`. Sized to the image's natural size in logical pixels; override with
/// [`width`](Element::width)/[`height`](Element::height) (one alone keeps the
/// aspect ratio) and choose placement with [`fit`](Element::fit) (default
/// [`ImageFit::Contain`]).
///
/// Keep the `Arc` in your state or a `static` rather than decoding in the view:
/// images compare by identity, so a fresh `Arc` each frame repaints each frame.
///
/// [`Pixmap::decode`]: crate::render::Pixmap::decode
/// [`ImageFit::Contain`]: crate::render::ImageFit::Contain
pub fn image(source: impl Into<ImageSource>) -> Element {
    Element::image(source)
}

/// An SVG document, rasterized at the physical size it's shown at so it stays
/// sharp at any scale. Sized like [`image`].
pub fn svg(doc: &Arc<Svg>) -> Element {
    Element::image(doc)
}

/// A raised surface card: themed fill, hairline border, rounded corners, and
/// comfortable padding. Lays its `children` vertically.
pub fn panel(theme: &Theme, children: Vec<Element>) -> Element {
    Element::stack(Axis::Vertical, children)
        .fill(theme.palette.surface)
        .radius(theme.radius)
        .border(theme.palette.border, 1.0)
        .padding(Insets::uniform(theme.spacing.lg))
        .gap(theme.spacing.md)
        // Stretch children across the card width so rows and dividers fill it.
        .align(Align::Start, Align::Stretch)
}

/// A primary action surface (the visual basis of an icon-only button). Fixed
/// size; for a text button use [`button_labeled`].
pub fn button(theme: &Theme) -> Element {
    Element::boxed(BoxStyle {
        fill: Some(theme.palette.primary),
        radius: theme.radius,
        border: None,
    })
    .width(96.0)
    .height(36.0)
}

/// A single line of text in the theme's default text color and body size.
pub fn label(theme: &Theme, text: impl Into<String>) -> Element {
    Element::text(text, theme.typography.body, theme.palette.text)
}

/// A larger heading in the theme's heading size.
pub fn heading(theme: &Theme, text: impl Into<String>) -> Element {
    Element::text(text, theme.typography.heading, theme.palette.text)
}

/// A block of body text that word-wraps to its container width across as many
/// lines as needed. Place it in a cross-stretch container (like [`panel`]) so it
/// takes the full width.
pub fn paragraph(theme: &Theme, text: impl Into<String>) -> Element {
    Element::text(text, theme.typography.body, theme.palette.text).wrap()
}

/// Visual emphasis level for a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Filled accent — the primary action.
    Primary,
    /// Outlined surface — a secondary action.
    Secondary,
    /// Text-only, no fill — a low-emphasis action.
    Ghost,
    /// Filled danger color — a destructive action.
    Danger,
}

/// A text button styled per [`Variant`], reading colors from the theme.
pub fn button_variant(theme: &Theme, label: impl Into<String>, variant: Variant) -> Element {
    let p = &theme.palette;
    // The OS's primary and regular buttons; it has no equivalent of the others.
    let control = match variant {
        Variant::Primary => Some(NativeControl::Button { primary: true }),
        Variant::Secondary => Some(NativeControl::Button { primary: false }),
        Variant::Ghost | Variant::Danger => None,
    };
    if let Some(control) = control
        && let Some(deco) = theme.native_deco(control)
    {
        let ink = if matches!(variant, Variant::Primary) && native::primary_label_is_on_accent() {
            p.on_primary
        } else {
            p.text
        };
        let el = Element::stack(
            Axis::Horizontal,
            vec![Element::text(label, theme.typography.body, ink)],
        )
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::symmetric(theme.spacing.lg, 0.0))
        .align(Align::Center, Align::Center)
        .native(deco);
        return native_size(el, control);
    }
    let (fill, ink, border): (Option<Color>, Color, Option<Color>) = match variant {
        Variant::Primary => (Some(p.primary), p.on_primary, None),
        Variant::Secondary => (Some(p.surface), p.text, Some(p.border)),
        Variant::Ghost => (None, p.primary, None),
        Variant::Danger => (Some(p.danger), p.danger.on_color(), None),
    };
    let mut el = Element::stack(
        Axis::Horizontal,
        vec![Element::text(label, theme.typography.body, ink)],
    )
    .radius(theme.radius)
    .padding(Insets::symmetric(theme.spacing.lg, theme.spacing.sm))
    .align(Align::Center, Align::Center);
    if let Some(fill) = fill {
        el = el.fill(fill);
    }
    if let Some(border) = border {
        el = el.border(border, 1.0);
    }
    el
}

/// A primary button with a centered text `label` (shorthand for
/// [`button_variant`] with [`Variant::Primary`]). Sizes to the label plus
/// padding (intrinsic sizing via the active font's measurement).
pub fn button_labeled(theme: &Theme, label: impl Into<String>) -> Element {
    button_variant(theme, label, Variant::Primary)
}

/// Apply a [`KeyInput`] to an editable string: append committed text, or
/// remove the last character on backspace. Navigation keys are ignored (no
/// caret model). For caret-aware editing use an [`EditBuffer`] with
/// [`text_editor`] instead.
pub fn edit_string(value: &mut String, input: &KeyInput) {
    match input {
        KeyInput::Text(t) => value.push_str(t),
        KeyInput::Backspace => {
            value.pop();
        }
        _ => {}
    }
}

/// A single-line text buffer with a caret, for in-place editing.
///
/// The caret is a byte index into the text, always kept on a UTF-8 char
/// boundary. Text is inserted and deleted at the caret; the arrow / Home / End
/// keys move it. Drive it by feeding [`KeyInput`]s to [`EditBuffer::apply`], and
/// render it (with a positioned caret) via [`text_editor`].
///
/// Edits are recorded for [`undo`](Self::undo) / [`redo`](Self::redo).
/// Consecutive typing (and consecutive Backspace or Delete presses) coalesce
/// into one undo step; a caret move, a selection change, or a different kind of
/// edit ends the step. Equality compares the text, caret and selection only,
/// not the undo history.
#[derive(Clone, Debug, Default)]
pub struct EditBuffer {
    text: String,
    caret: usize,
    /// The fixed end of an active selection (the caret is the moving end);
    /// `None` when nothing is selected.
    anchor: Option<usize>,
    history: History,
    /// The paragraph direction, for drawing and for which way Left and Right
    /// move the caret.
    direction: TextDirection,
    /// Input-method composition shown at the caret (not part of `text`).
    preedit: Option<preedit::Preedit>,
}

impl PartialEq for EditBuffer {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && self.caret == other.caret
            && self.anchor == other.anchor
            && self.preedit == other.preedit
    }
}

impl Eq for EditBuffer {}

/// The most undo steps an [`EditBuffer`] keeps; older steps are dropped.
const UNDO_LIMIT: usize = 1000;

/// What kind of edit an undo step records, deciding whether the next edit may
/// join it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    /// Typed text: consecutive typing coalesces.
    Typing,
    /// Backspace: consecutive presses coalesce.
    Backspace,
    /// Delete: consecutive presses coalesce.
    Delete,
    /// Anything else (paste, cut, newline, word delete, typing over a
    /// selection): always its own step.
    Other,
}

/// One primitive replacement: at byte `at`, `removed` was replaced by
/// `inserted`.
#[derive(Clone, Debug)]
struct Edit {
    at: usize,
    removed: String,
    inserted: String,
}

/// One undo step: a run of primitive edits plus the caret/anchor before and
/// after, so undo and redo restore the selection too.
#[derive(Clone, Debug)]
struct UndoStep {
    kind: EditKind,
    edits: Vec<Edit>,
    before: (usize, Option<usize>),
    after: (usize, Option<usize>),
}

/// The undo and redo stacks of an [`EditBuffer`].
#[derive(Clone, Debug, Default)]
struct History {
    undo: Vec<UndoStep>,
    redo: Vec<UndoStep>,
    /// Whether the top undo step may still absorb the next edit of its kind.
    /// Cleared by caret moves, selection changes, and undo/redo.
    open: bool,
}

/// A character class for word-wise movement and double-click selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharClass {
    /// Line break: never part of a double-click selection.
    Newline,
    /// Other whitespace.
    Space,
    /// Letters and digits (any script) and `_`.
    Word,
    /// Everything else (punctuation, symbols).
    Punct,
}

fn char_class(c: char) -> CharClass {
    if c == '\n' {
        CharClass::Newline
    } else if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

fn is_blank(class: CharClass) -> bool {
    matches!(class, CharClass::Newline | CharClass::Space)
}

impl EditBuffer {
    /// An empty buffer with the caret at the start.
    pub fn new() -> Self {
        Self::default()
    }

    /// A buffer holding `text`, caret placed at the end.
    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let caret = text.len();
        Self {
            text,
            caret,
            anchor: None,
            history: History::default(),
            direction: TextDirection::Auto,
            preedit: None,
        }
    }

    /// This buffer with its paragraph direction set (see
    /// [`set_direction`](Self::set_direction)).
    pub fn with_direction(mut self, direction: TextDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Set the paragraph direction: [`TextDirection::Auto`] (the default) takes
    /// each line's direction from its first strong character. [`text_editor`]
    /// and [`text_area`] draw the text with it, and Left and Right move the
    /// caret visually along the line it gives.
    pub fn set_direction(&mut self, direction: TextDirection) {
        self.direction = direction;
    }

    /// The paragraph direction.
    pub fn direction(&self) -> TextDirection {
        self.direction
    }

    /// The current text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The caret position as a byte index into [`text`](Self::text).
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// `true` if there is no text.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The selected byte range `[start, end)`, or `None` when nothing is
    /// selected (the anchor is unset or collapsed onto the caret).
    pub fn selection(&self) -> Option<(usize, usize)> {
        match self.anchor {
            Some(a) if a != self.caret => Some((a.min(self.caret), a.max(self.caret))),
            _ => None,
        }
    }

    /// Replace `range` with `s`, leave the caret after the insertion with no
    /// selection, and record the change for undo as an edit of `kind`.
    fn replace(&mut self, range: std::ops::Range<usize>, s: &str, kind: EditKind) {
        if range.is_empty() && s.is_empty() {
            return;
        }
        let before = (self.caret, self.anchor);
        let removed = self.text[range.clone()].to_string();
        self.text.replace_range(range.clone(), s);
        self.caret = range.start + s.len();
        self.anchor = None;
        let edit = Edit {
            at: range.start,
            removed,
            inserted: s.to_string(),
        };
        let after = (self.caret, None);
        let h = &mut self.history;
        h.redo.clear();
        match h.undo.last_mut() {
            Some(step) if h.open && kind != EditKind::Other && step.kind == kind => {
                step.edits.push(edit);
                step.after = after;
            }
            _ => {
                h.undo.push(UndoStep {
                    kind,
                    edits: vec![edit],
                    before,
                    after,
                });
                if h.undo.len() > UNDO_LIMIT {
                    h.undo.remove(0);
                }
            }
        }
        h.open = kind != EditKind::Other;
    }

    /// End the current undo step, so the next edit starts a new one. Called on
    /// every caret move and selection change.
    fn seal(&mut self) {
        self.history.open = false;
    }

    /// Delete the active selection (if any) as its own undo step, leaving the
    /// caret at its start. Returns `true` if something was removed.
    fn delete_selection(&mut self) -> bool {
        if let Some((s, e)) = self.selection() {
            self.replace(s..e, "", EditKind::Other);
            true
        } else {
            self.anchor = None;
            false
        }
    }

    /// Insert `s` as typed text, replacing the selection first, and advance the
    /// caret past it. Consecutive typing coalesces into one undo step.
    pub fn insert(&mut self, s: &str) {
        self.insert_as(s, EditKind::Typing);
    }

    /// Insert `s` at the caret (replacing any selection) as an edit of `kind`.
    /// Typing over a selection is always its own undo step.
    fn insert_as(&mut self, s: &str, kind: EditKind) {
        let (range, kind) = match self.selection() {
            Some((a, b)) => (a..b, EditKind::Other),
            None => (self.caret..self.caret, kind),
        };
        self.replace(range, s, kind);
    }

    /// Delete the selection, or the character before the caret (Backspace).
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if let Some(prev) = self.prev_boundary(self.caret) {
            self.replace(prev..self.caret, "", EditKind::Backspace);
        }
    }

    /// Delete the selection, or the character at the caret (Delete).
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if let Some(next) = self.next_boundary(self.caret) {
            self.replace(self.caret..next, "", EditKind::Delete);
        }
    }

    /// Delete the selection, or from the previous word start to the caret
    /// (Ctrl+Backspace; Alt+Backspace on macOS).
    pub fn delete_word_backward(&mut self) {
        if self.delete_selection() {
            return;
        }
        let start = self.word_left_of(self.caret);
        self.replace(start..self.caret, "", EditKind::Other);
    }

    /// Delete the selection, or from the caret to the next word end
    /// (Ctrl+Delete; Alt+Delete on macOS).
    pub fn delete_word_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        let end = self.word_right_of(self.caret);
        self.replace(self.caret..end, "", EditKind::Other);
    }

    /// `true` if there is an edit to [`undo`](Self::undo).
    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    /// `true` if there is an undone edit to [`redo`](Self::redo).
    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    /// Revert the last undo step, restoring the text, caret and selection as
    /// they were before it. Returns `false` if there was nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(step) = self.history.undo.pop() else {
            return false;
        };
        for e in step.edits.iter().rev() {
            self.text
                .replace_range(e.at..e.at + e.inserted.len(), &e.removed);
        }
        (self.caret, self.anchor) = step.before;
        self.history.redo.push(step);
        self.seal();
        true
    }

    /// Re-apply the last undone step. Returns `false` if there was nothing to
    /// redo. Any new edit clears the redo stack.
    pub fn redo(&mut self) -> bool {
        let Some(step) = self.history.redo.pop() else {
            return false;
        };
        for e in &step.edits {
            self.text
                .replace_range(e.at..e.at + e.removed.len(), &e.inserted);
        }
        (self.caret, self.anchor) = step.after;
        self.history.undo.push(step);
        self.seal();
        true
    }

    /// Forget all undo and redo steps (e.g. after loading a new document).
    pub fn clear_history(&mut self) {
        self.history = History::default();
    }

    /// The class of the char starting at byte `i`, or `None` at the end.
    fn class_after(&self, i: usize) -> Option<CharClass> {
        self.text[i..].chars().next().map(char_class)
    }

    /// The class of the char ending at byte `i`, or `None` at the start.
    fn class_before(&self, i: usize) -> Option<CharClass> {
        self.text[..i].chars().next_back().map(char_class)
    }

    /// The start of the word at or before `i`: skip whitespace backwards, then
    /// the run of same-class chars (a word, or a run of punctuation).
    fn word_left_of(&self, mut i: usize) -> usize {
        while self.class_before(i).is_some_and(is_blank) {
            i = self.prev_boundary(i).unwrap_or(0);
        }
        if let Some(class) = self.class_before(i) {
            while self.class_before(i) == Some(class) {
                i = self.prev_boundary(i).unwrap_or(0);
            }
        }
        i
    }

    /// The end of the word at or after `i`: skip whitespace forwards, then the
    /// run of same-class chars.
    fn word_right_of(&self, mut i: usize) -> usize {
        while self.class_after(i).is_some_and(is_blank) {
            i = self.next_boundary(i).unwrap_or(self.text.len());
        }
        if let Some(class) = self.class_after(i) {
            while self.class_after(i) == Some(class) {
                i = self.next_boundary(i).unwrap_or(self.text.len());
            }
        }
        i
    }

    /// The byte range `[start, end)` of the word (or run of punctuation, or of
    /// spaces) at byte `i`. A word touching `i` on either side wins over
    /// punctuation, and punctuation over spaces, so a click just past a word's
    /// last letter still picks the word. Never spans a line break; empty when
    /// `i` sits between two line breaks.
    pub fn word_range_at(&self, i: usize) -> (usize, usize) {
        let i = self.snap(i);
        let sides = [self.class_after(i), self.class_before(i)];
        let Some(class) = [CharClass::Word, CharClass::Punct, CharClass::Space]
            .into_iter()
            .find(|c| sides.contains(&Some(*c)))
        else {
            return (i, i);
        };
        let mut start = i;
        while self.class_before(start) == Some(class) {
            start = self.prev_boundary(start).unwrap_or(0);
        }
        let mut end = i;
        while self.class_after(end) == Some(class) {
            end = self.next_boundary(end).unwrap_or(self.text.len());
        }
        (start, end)
    }

    /// Move the caret to the previous word start, clearing the selection.
    pub fn word_left(&mut self) {
        self.caret = self.word_left_of(self.caret);
        self.anchor = None;
        self.seal();
    }

    /// Move the caret to the next word end, clearing the selection.
    pub fn word_right(&mut self) {
        self.caret = self.word_right_of(self.caret);
        self.anchor = None;
        self.seal();
    }

    /// Extend the selection to the previous word start.
    pub fn select_word_left(&mut self) {
        self.extend(|b| b.caret = b.word_left_of(b.caret));
    }

    /// Extend the selection to the next word end.
    pub fn select_word_right(&mut self) {
        self.extend(|b| b.caret = b.word_right_of(b.caret));
    }

    /// Select the word at the caret (see [`word_range_at`](Self::word_range_at)),
    /// as a double-click does.
    pub fn select_word(&mut self) {
        let (s, e) = self.word_range_at(self.caret);
        self.anchor = Some(s);
        self.caret = e;
        self.seal();
    }

    /// Select the line containing the caret (without its line break), as a
    /// triple-click does.
    pub fn select_line(&mut self) {
        self.anchor = Some(self.line_start(self.caret));
        self.caret = self.line_end(self.caret);
        self.seal();
    }

    /// Move the caret one character left on screen, collapsing any selection
    /// to its start (its end on a right-to-left line).
    ///
    /// The movement is visual: inside right-to-left text, Left moves forward
    /// through the text. See [`visual_neighbor`](Self::visual_neighbor).
    pub fn move_left(&mut self) {
        self.move_visually(false);
    }

    /// Move the caret one character right on screen, collapsing any selection
    /// to its end (its start on a right-to-left line).
    pub fn move_right(&mut self) {
        self.move_visually(true);
    }

    fn move_visually(&mut self, right: bool) {
        if let Some((s, e)) = self.selection() {
            let rtl = self.line_is_rtl(self.caret);
            self.caret = if right != rtl { e } else { s };
        } else if let Some(next) = self.visual_neighbor(self.caret, right) {
            self.caret = next;
        }
        self.anchor = None;
        self.seal();
    }

    /// Whether the line holding byte `i` runs right to left.
    fn line_is_rtl(&self, i: usize) -> bool {
        let line = &self.text[self.line_start(i)..self.line_end(i)];
        self.direction.is_rtl_for(line)
    }

    /// The caret position one character left (`right == false`) or right of
    /// byte `i` on screen, or `None` when there's nowhere to go.
    ///
    /// Each line is run through the Unicode Bidirectional Algorithm and the
    /// caret steps between the visual edges of its characters, so it moves
    /// left to right across mixed text even where the logical order jumps.
    /// At the start or end of a line it crosses to the previous or next line.
    pub fn visual_neighbor(&self, i: usize, right: bool) -> Option<usize> {
        let (ls, le) = (self.line_start(i), self.line_end(i));
        let line = &self.text[ls..le];
        if let Some(b) = crate::render::bidi::visual_step_in(line, i - ls, self.direction, right) {
            return Some(ls + b);
        }
        // At the line's visual edge: Right goes forward in a left-to-right
        // line and back in a right-to-left one, onto the neighboring line.
        let forward = right != self.direction.is_rtl_for(line);
        if forward && i == le {
            self.next_boundary(i)
        } else if !forward && i == ls {
            self.prev_boundary(i)
        } else {
            None
        }
    }

    /// Move the caret to the start of the current line, clearing the selection.
    pub fn home(&mut self) {
        self.caret = self.line_start(self.caret);
        self.anchor = None;
        self.seal();
    }

    /// Move the caret to the end of the current line, clearing the selection.
    pub fn end(&mut self) {
        self.caret = self.line_end(self.caret);
        self.anchor = None;
        self.seal();
    }

    /// Move the caret up one line (same byte column), clearing the selection.
    pub fn up(&mut self) {
        self.caret = self.caret_up(self.caret);
        self.anchor = None;
        self.seal();
    }

    /// Move the caret down one line (same byte column), clearing the selection.
    pub fn down(&mut self) {
        self.caret = self.caret_down(self.caret);
        self.anchor = None;
        self.seal();
    }

    /// Byte index of the start of the line containing `byte` (after the
    /// preceding `\n`, or 0).
    fn line_start(&self, byte: usize) -> usize {
        self.text[..byte].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    /// Byte index of the end of the line containing `byte` (before the next
    /// `\n`, or the text end).
    fn line_end(&self, byte: usize) -> usize {
        self.text[byte..]
            .find('\n')
            .map(|i| byte + i)
            .unwrap_or(self.text.len())
    }

    /// The caret position one line above `byte`, keeping the byte column.
    fn caret_up(&self, byte: usize) -> usize {
        let start = self.line_start(byte);
        if start == 0 {
            return 0; // already on the first line
        }
        let col = byte - start;
        let prev_start = self.line_start(start - 1);
        let prev_end = start - 1; // the '\n' ending the previous line
        self.snap((prev_start + col).min(prev_end))
    }

    /// The caret position one line below `byte`, keeping the byte column.
    fn caret_down(&self, byte: usize) -> usize {
        let end = self.line_end(byte);
        if end == self.text.len() {
            return self.text.len(); // already on the last line
        }
        let col = byte - self.line_start(byte);
        let next_start = end + 1;
        let next_end = self.line_end(next_start);
        self.snap((next_start + col).min(next_end))
    }

    /// Begin or extend a selection: anchor at the current caret if not already
    /// selecting, then run `motion` to move the caret (the moving end).
    fn extend(&mut self, motion: impl FnOnce(&mut Self)) {
        if self.anchor.is_none() {
            self.anchor = Some(self.caret);
        }
        let anchor = self.anchor;
        motion(self);
        self.anchor = anchor; // motion's `move_*` clears it; restore.
        self.seal();
    }

    /// Extend the selection one character left on screen.
    pub fn select_left(&mut self) {
        self.extend(|b| {
            if let Some(prev) = b.visual_neighbor(b.caret, false) {
                b.caret = prev;
            }
        });
    }

    /// Extend the selection one character right on screen.
    pub fn select_right(&mut self) {
        self.extend(|b| {
            if let Some(next) = b.visual_neighbor(b.caret, true) {
                b.caret = next;
            }
        });
    }

    /// Extend the selection to the start of the current line.
    pub fn select_home(&mut self) {
        self.extend(|b| b.caret = b.line_start(b.caret));
    }

    /// Extend the selection to the end of the current line.
    pub fn select_end(&mut self) {
        self.extend(|b| b.caret = b.line_end(b.caret));
    }

    /// Select the entire text.
    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.text.len();
        self.seal();
    }

    /// Place the caret at byte index `i` (snapped to a char boundary) and clear
    /// any selection. For click-to-position from a pointer hit-test.
    pub fn place_caret(&mut self, i: usize) {
        self.caret = self.snap(i);
        self.anchor = None;
        self.seal();
    }

    /// Move the caret (the selection's moving end) to byte index `i`, anchoring
    /// the selection at the prior caret if not already selecting. For
    /// drag-to-select from a pointer hit-test.
    pub fn extend_to(&mut self, i: usize) {
        if self.anchor.is_none() {
            self.anchor = Some(self.caret);
        }
        self.caret = self.snap(i);
        self.seal();
    }

    /// Snap a byte index to the nearest char boundary at or below it, clamped to
    /// the text length.
    fn snap(&self, i: usize) -> usize {
        let mut i = i.min(self.text.len());
        while !self.text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    /// Apply a [`KeyInput`]: insert/replace text, delete, or move/extend the
    /// caret. Enter/Escape are ignored (single-line).
    pub fn apply(&mut self, input: &KeyInput) {
        match input {
            KeyInput::Text(t) => self.commit_text(t),
            KeyInput::Preedit { text, cursor } => self.set_preedit(text, *cursor),
            KeyInput::DeleteSurrounding { before, after } => {
                self.delete_surrounding(*before, *after)
            }
            KeyInput::Backspace => self.backspace(),
            KeyInput::Delete => self.delete(),
            KeyInput::Left => self.move_left(),
            KeyInput::Right => self.move_right(),
            KeyInput::Up => self.up(),
            KeyInput::Down => self.down(),
            KeyInput::Home => self.home(),
            KeyInput::End => self.end(),
            KeyInput::SelectLeft => self.select_left(),
            KeyInput::SelectRight => self.select_right(),
            KeyInput::SelectUp => self.extend(|b| b.caret = b.caret_up(b.caret)),
            KeyInput::SelectDown => self.extend(|b| b.caret = b.caret_down(b.caret)),
            KeyInput::SelectHome => self.select_home(),
            KeyInput::SelectEnd => self.select_end(),
            KeyInput::SelectAll => self.select_all(),
            KeyInput::WordLeft => self.word_left(),
            KeyInput::WordRight => self.word_right(),
            KeyInput::SelectWordLeft => self.select_word_left(),
            KeyInput::SelectWordRight => self.select_word_right(),
            KeyInput::DeleteWordBackward => self.delete_word_backward(),
            KeyInput::DeleteWordForward => self.delete_word_forward(),
            KeyInput::SelectWord => self.select_word(),
            KeyInput::SelectLine => self.select_line(),
            KeyInput::Undo => {
                self.undo();
            }
            KeyInput::Redo => {
                self.redo();
            }
            // Clipboard: copy/cut write the selection to the process clipboard
            // mirror (the app syncs it to the OS); paste inserts the mirror text.
            KeyInput::Copy => {
                if let Some(sel) = self.selected_text() {
                    crate::core::set_clipboard_text(&sel);
                }
            }
            KeyInput::Cut => {
                if let Some(sel) = self.selected_text() {
                    crate::core::set_clipboard_text(&sel);
                    self.delete_selection();
                }
            }
            KeyInput::Paste => {
                let text = crate::core::clipboard_text();
                if !text.is_empty() {
                    self.insert_as(&text, EditKind::Other);
                }
            }
            // Enter inserts a newline (the buffer is multi-line capable), as
            // its own undo step.
            KeyInput::Enter => self.insert_as("\n", EditKind::Other),
            KeyInput::Escape => {}
        }
    }

    /// The currently selected text, or `None` if there is no selection.
    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|(s, e)| self.text[s..e].to_string())
    }

    /// The char boundary strictly before `i`, or `None` at the start.
    fn prev_boundary(&self, i: usize) -> Option<usize> {
        if i == 0 {
            return None;
        }
        let mut p = i - 1;
        while !self.text.is_char_boundary(p) {
            p -= 1;
        }
        Some(p)
    }

    /// The char boundary strictly after `i`, or `None` at the end.
    fn next_boundary(&self, i: usize) -> Option<usize> {
        if i >= self.text.len() {
            return None;
        }
        let mut n = i + 1;
        while n < self.text.len() && !self.text.is_char_boundary(n) {
            n += 1;
        }
        Some(n)
    }
}

/// A single-line editable text field showing `value` and routing keyboard
/// input to `on_key` while focused (pair with [`edit_string`]). Default width
/// 200 logical px; override with `.width(..)` on the returned element.
pub fn text_field<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    value: &str,
    on_key: impl FnMut(&mut S, &KeyInput) + 'static,
) -> Element {
    // A leading space keeps an empty field from collapsing to zero height.
    let shown = if value.is_empty() {
        String::from(" ")
    } else {
        value.to_string()
    };
    Element::stack(
        Axis::Horizontal,
        vec![Element::text(shown, theme.font_size, theme.palette.text)],
    )
    .fill(theme.palette.surface)
    .radius(theme.radius)
    .border(theme.palette.border, 1.0)
    .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
    .align(Align::Start, Align::Center)
    .width(200.0)
    .native_text_frame(theme)
    .cursor(crate::platform::CursorIcon::Text)
    .on_key(cx, on_key)
}

/// A single-line editable field backed by an [`EditBuffer`], rendering the text
/// with the caret and selection at the buffer's positions (shown while focused).
///
/// `field` is an accessor that returns the `&mut EditBuffer` inside the app
/// state; the field wires both keyboard input (`EditBuffer::apply`) and pointer
/// interaction (click to place the caret, drag to select) to it. Default width
/// 200 logical px; override with `.width(..)` on the returned element.
///
/// ```
/// # use stipple::widgets::{text_editor, EditBuffer};
/// # use stipple::core::runtime::Cx;
/// # use stipple::style::Theme;
/// struct App { name: EditBuffer }
/// let theme = Theme::light();
/// let mut cx = Cx::new(&theme);
/// let state = App { name: EditBuffer::from_text("hi") };
/// let field = text_editor(&mut cx, &theme, &state.name, |s: &mut App| &mut s.name);
/// assert!(field.focus.is_some() && field.text_pos.is_some());
/// ```
pub fn text_editor<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    buffer: &EditBuffer,
    field: impl Fn(&mut S) -> &mut EditBuffer + Copy + 'static,
) -> Element {
    let shown = buffer.display();
    // The text fills the field so right-to-left lines can sit at its right edge.
    let text = Element::text(placeholder(shown.text), theme.font_size, theme.palette.text)
        .direction(buffer.direction())
        .grow(1.0)
        .caret(shown.caret)
        .selection(shown.selection)
        .preedit(shown.preedit);
    Element::stack(Axis::Horizontal, vec![text])
        .fill(theme.palette.surface)
        .radius(theme.radius)
        .border(theme.palette.border, 1.0)
        .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
        .align(Align::Start, Align::Center)
        .width(200.0)
        .native_text_frame(theme)
        .on_key(cx, move |s, k| match k {
            // Single-line: a triple-click selects the whole field.
            KeyInput::SelectLine => field(s).select_all(),
            _ => field(s).apply(k),
        })
        .on_text_pos(cx, move |s, index, extend| {
            let b = field(s);
            let index = b.display_to_text(index);
            if extend {
                b.extend_to(index);
            } else {
                b.place_caret(index);
            }
        })
}

/// The text a field shows: a lone space for an empty one, which keeps it from
/// collapsing to zero height (the caret then sits at index 0 of it).
fn placeholder(text: String) -> String {
    if text.is_empty() {
        String::from(" ")
    } else {
        text
    }
}

/// A multi-line editable text area backed by an [`EditBuffer`]: the same
/// keyboard + pointer wiring as [`text_editor`], but top-aligned so wrapped and
/// `\n`-separated lines stack from the top. Enter inserts a newline, and
/// Up/Down move between lines. Default size 280×120 logical px; override with
/// `.width(..)`/`.height(..)`.
pub fn text_area<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    buffer: &EditBuffer,
    field: impl Fn(&mut S) -> &mut EditBuffer + Copy + 'static,
) -> Element {
    let shown = buffer.display();
    // The text fills the field so right-to-left lines can sit at its right edge.
    let text = Element::text(placeholder(shown.text), theme.font_size, theme.palette.text)
        .direction(buffer.direction())
        .grow(1.0)
        .caret(shown.caret)
        .selection(shown.selection)
        .preedit(shown.preedit);
    Element::stack(Axis::Horizontal, vec![text])
        .fill(theme.palette.surface)
        .radius(theme.radius)
        .border(theme.palette.border, 1.0)
        .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
        .align(Align::Start, Align::Start)
        .width(280.0)
        .height(120.0)
        .native_text_frame(theme)
        .on_key(cx, move |s, k| field(s).apply(k))
        .on_text_pos(cx, move |s, index, extend| {
            let b = field(s);
            let index = b.display_to_text(index);
            if extend {
                b.extend_to(index);
            } else {
                b.place_caret(index);
            }
        })
}

/// A toggleable checkbox: a small square showing a check mark when `checked`,
/// calling `on_toggle` when tapped.
pub fn checkbox<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    checked: bool,
    on_toggle: impl FnMut(&mut S) + 'static,
) -> Element {
    let control = NativeControl::Checkbox { checked };
    if let Some(deco) = theme.native_deco(control) {
        return native_size(Element::boxed(BoxStyle::default()).native(deco), control)
            .on_tap(cx, on_toggle)
            .checked(checked);
    }
    let mark = if checked {
        vec![Element::text(
            "✓",
            theme.font_size,
            theme.palette.on_primary,
        )]
    } else {
        Vec::new()
    };
    Element::stack(Axis::Horizontal, mark)
        .fill(if checked {
            theme.palette.primary
        } else {
            theme.palette.surface
        })
        .border(theme.palette.border, 1.0)
        .radius(theme.radius / 2.0)
        .width(22.0)
        .height(22.0)
        .align(Align::Center, Align::Center)
        .on_tap(cx, on_toggle)
        .checked(checked)
        // Not named by its check mark; the label next to it names it.
        .a11y_label("")
}

/// An on/off switch: a pill track with a knob that sits right when `on`
/// (track tinted with the primary color) and left when off. Tapping calls
/// `on_toggle`.
pub fn switch<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    on: bool,
    on_toggle: impl FnMut(&mut S) + 'static,
) -> Element {
    let control = NativeControl::Switch { on };
    if let Some(deco) = theme.native_deco(control) {
        return native_size(Element::boxed(BoxStyle::default()).native(deco), control)
            .on_tap(cx, on_toggle)
            .role(Role::Switch)
            .checked(on);
    }
    let knob = Element::boxed(BoxStyle {
        fill: Some(Color::WHITE),
        radius: 8.0,
        border: None,
    })
    .width(16.0)
    .height(16.0);
    let children = if on {
        vec![spacer(), knob]
    } else {
        vec![knob, spacer()]
    };
    Element::stack(Axis::Horizontal, children)
        .fill(if on {
            theme.palette.primary
        } else {
            theme.palette.border
        })
        .radius(11.0)
        .width(40.0)
        .height(22.0)
        .padding(Insets::uniform(3.0))
        .align(Align::Start, Align::Center)
        .on_tap(cx, on_toggle)
        .role(Role::Switch)
        .checked(on)
}

/// A horizontal slider with `value` in 0..=1. Dragging anywhere on the track
/// calls `on_change` with the new fractional position. Default width 160 px;
/// override with `.width(..)`.
pub fn slider<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    value: f64,
    on_change: impl FnMut(&mut S, f64) + 'static,
) -> Element {
    let v = value.clamp(0.0, 1.0);
    let control = NativeControl::Slider {
        value: (v * 1000.0).round() as u16,
    };
    if let Some(deco) = theme.native_deco(control) {
        return native_size(Element::boxed(BoxStyle::default()).native(deco), control)
            .on_drag(cx, on_change);
    }
    let knob = Element::boxed(BoxStyle {
        fill: Some(theme.palette.primary),
        radius: 8.0,
        border: None,
    })
    .width(16.0)
    .height(16.0);
    Element::stack(
        Axis::Horizontal,
        vec![spacer().grow(v), knob, spacer().grow(1.0 - v)],
    )
    .fill(theme.palette.border)
    .radius(11.0)
    .width(160.0)
    .height(22.0)
    .padding(Insets::symmetric(3.0, 3.0))
    .align(Align::Start, Align::Center)
    .on_drag(cx, on_change)
}

/// A 1px themed divider line spanning the cross axis.
pub fn divider(theme: &Theme) -> Element {
    Element::boxed(BoxStyle {
        fill: Some(theme.palette.border),
        radius: 0.0,
        border: None,
    })
    .height(1.0)
}

/// A small square color sample with rounded corners.
pub fn swatch(color: Color, size: f64, radius: f64) -> Element {
    Element::boxed(BoxStyle {
        fill: Some(color),
        radius,
        border: None,
    })
    .width(size)
    .height(size)
}

/// A horizontal row with a leading swatch, a flexible spacer, and a trailing
/// button — a compact "setting row" used to show real composition.
pub fn setting_row(theme: &Theme, accent: Color, action: impl Into<String>) -> Element {
    row(vec![
        swatch(accent, 24.0, theme.radius / 2.0),
        spacer(),
        button_variant(theme, action, Variant::Secondary),
    ])
    .gap(theme.spacing.md)
    .align(Align::Start, Align::Center)
    .height(40.0)
}

/// A radio button: a circle filled with the primary color when `selected`.
/// Tapping calls `on_select`.
pub fn radio<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    selected: bool,
    on_select: impl FnMut(&mut S) + 'static,
) -> Element {
    let control = NativeControl::Radio { selected };
    if let Some(deco) = theme.native_deco(control) {
        return native_size(Element::boxed(BoxStyle::default()).native(deco), control)
            .on_tap(cx, on_select);
    }
    let dot = if selected {
        vec![
            Element::boxed(BoxStyle {
                fill: Some(theme.palette.primary),
                radius: 5.0,
                border: None,
            })
            .width(10.0)
            .height(10.0),
        ]
    } else {
        Vec::new()
    };
    Element::stack(Axis::Horizontal, dot)
        .fill(theme.palette.surface)
        .border(theme.palette.border, 1.0)
        .radius(11.0) // half the size → a circle
        .width(22.0)
        .height(22.0)
        .align(Align::Center, Align::Center)
        .on_tap(cx, on_select)
}

/// A horizontal tab strip (segmented control): one cell per label, the
/// `selected` one filled with the accent color and the rest on the surface.
/// Tapping a tab runs `on_select` with that tab's index. Stateless — store the
/// selected index in your app state and render the matching content yourself.
///
/// `on_select` must be `Clone` because it is stamped onto every tab (each with
/// its own index); a plain closure capturing `Copy`/`Clone` data satisfies this.
pub fn tabs<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    labels: &[&str],
    selected: usize,
    on_select: impl Fn(&mut S, usize) + Clone + 'static,
) -> Element {
    let p = &theme.palette;
    let mut cells = Vec::with_capacity(labels.len());
    for (i, text) in labels.iter().enumerate() {
        let active = i == selected;
        let (fill, ink) = if active {
            (p.primary, p.on_primary)
        } else {
            (p.surface, p.text)
        };
        let on = on_select.clone();
        let cell = Element::stack(
            Axis::Horizontal,
            vec![Element::text(*text, theme.typography.body, ink)],
        )
        .fill(fill)
        .border(p.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
        .align(Align::Center, Align::Center)
        .grow(1.0)
        .on_tap(cx, move |s: &mut S| on(s, i));
        cells.push(cell);
    }
    Element::stack(Axis::Horizontal, cells)
        .gap(theme.spacing.xs)
        .align(Align::Start, Align::Stretch)
}

/// A progress bar: a track with a primary-filled portion for `fraction` (0..=1).
/// Default width 200 px; override with `.width(..)`.
pub fn progress_bar(theme: &Theme, fraction: f64) -> Element {
    let f = fraction.clamp(0.0, 1.0);
    let control = NativeControl::Progress {
        value: (f * 1000.0).round() as u16,
    };
    if let Some(deco) = theme.native_deco(control) {
        return native_size(Element::boxed(BoxStyle::default()).native(deco), control);
    }
    let fill = Element::boxed(BoxStyle {
        fill: Some(theme.palette.primary),
        radius: 4.0,
        border: None,
    })
    .grow(f);
    Element::stack(Axis::Horizontal, vec![fill, spacer().grow(1.0 - f)])
        .fill(theme.palette.border)
        .radius(4.0)
        .width(200.0)
        .height(8.0)
        .align(Align::Start, Align::Stretch)
}

mod spinner;
pub use spinner::spinner;

/// A tappable menu row showing `text`, running `on_select` when clicked. Stretch
/// it across a [`menu`] panel (the panel stretches its children).
pub fn menu_item<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    text: impl Into<String>,
    on_select: impl FnMut(&mut S) + 'static,
) -> Element {
    Element::stack(Axis::Horizontal, vec![label(theme, text)])
        .radius(theme.radius / 2.0)
        .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
        .align(Align::Start, Align::Center)
        .on_tap(cx, on_select)
}

/// A floating vertical menu panel (surface, border, padding) holding `items`
/// (build them with [`menu_item`]). The visual body of a dropdown — show it via
/// [`open_menu`] or `cx.overlay`.
pub fn menu(theme: &Theme, items: Vec<Element>) -> Element {
    Element::stack(Axis::Vertical, items)
        .fill(theme.palette.surface)
        .border(theme.palette.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::uniform(theme.spacing.xs))
        .gap(2.0)
        .align(Align::Start, Align::Stretch)
        .width(180.0)
}

/// Declare a non-modal dropdown overlay anchored with its top-left at `at`,
/// containing `items` (from [`menu_item`]). `on_dismiss` fires on a press
/// outside the menu. Call when the menu should be open.
pub fn open_menu<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    at: Point,
    items: Vec<Element>,
    on_dismiss: impl FnMut(&mut S) + 'static,
) {
    let content = menu(theme, items);
    let dismiss = cx.register(on_dismiss);
    cx.overlay(OverlaySpec {
        content,
        anchor: Anchor::At(at),
        modal: false,
        dismiss: Some(dismiss),
    });
}

/// Declare a modal dialog overlay: a centered panel with `title`, a `body`
/// element, and a right-aligned row of `actions` (buttons). A dark scrim behind
/// it blocks the tree; pressing the scrim runs `on_dismiss`.
pub fn open_dialog<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    title: impl Into<String>,
    body: Element,
    actions: Vec<Element>,
    on_dismiss: impl FnMut(&mut S) + 'static,
) {
    let dismiss = cx.register(on_dismiss);
    let content = panel(
        theme,
        vec![
            heading(theme, title),
            body,
            row(actions)
                .gap(theme.spacing.sm)
                .align(Align::End, Align::Center),
        ],
    )
    .width(340.0);
    cx.overlay(OverlaySpec {
        content,
        anchor: Anchor::Center,
        modal: true,
        dismiss: Some(dismiss),
    });
}

/// A small tooltip bubble showing `text` (a dark rounded label). Show it via
/// `cx.overlay` anchored near the hovered element (non-modal, no dismiss).
pub fn tooltip(theme: &Theme, text: impl Into<String>) -> Element {
    Element::stack(
        Axis::Horizontal,
        vec![Element::text(
            text.into(),
            theme.typography.caption,
            theme.palette.on_primary,
        )],
    )
    .fill(theme.palette.text)
    .radius(theme.radius / 2.0)
    .padding(Insets::symmetric(theme.spacing.sm, theme.spacing.xs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{ElementKind, render_view};
    use crate::geometry::Size;

    #[test]
    fn panel_is_a_decorated_vertical_stack() {
        let p = panel(&Theme::light(), vec![divider(&Theme::light())]);
        assert!(p.decoration.fill.is_some());
        assert!(matches!(
            p.kind,
            ElementKind::Stack {
                axis: Axis::Vertical,
                ..
            }
        ));
    }

    #[test]
    fn setting_row_paints_swatch_and_button() {
        // Swatch fill, plus the button's fill and border; the spacer is empty
        // and the label needs a font.
        let theme = Theme::light();
        let scene = render_view(
            &setting_row(&theme, Color::rgb(200, 80, 80), "Edit"),
            Size::new(300.0, 40.0),
            &theme,
            None,
        );
        assert_eq!(scene.len(), 3);
    }

    #[test]
    fn edit_buffer_inserts_and_moves_caret() {
        let mut b = EditBuffer::new();
        b.insert("ab");
        assert_eq!((b.text(), b.caret()), ("ab", 2));
        b.move_left();
        assert_eq!(b.caret(), 1);
        b.insert("X"); // insert mid-string at the caret
        assert_eq!((b.text(), b.caret()), ("aXb", 2));
        b.home();
        assert_eq!(b.caret(), 0);
        b.end();
        assert_eq!(b.caret(), 3);
    }

    #[test]
    fn edit_buffer_backspace_and_delete_at_caret() {
        let mut b = EditBuffer::from_text("abc");
        assert_eq!(b.caret(), 3); // from_text places caret at end
        b.backspace();
        assert_eq!((b.text(), b.caret()), ("ab", 2));
        b.home();
        b.delete();
        assert_eq!((b.text(), b.caret()), ("b", 0));
        // Deletes/backspaces at the edges are no-ops.
        b.backspace();
        assert_eq!(b.text(), "b");
        b.end();
        b.delete();
        assert_eq!(b.text(), "b");
    }

    #[test]
    fn edit_buffer_respects_utf8_boundaries() {
        // "é" and "🦀" are multi-byte; caret motion must land on boundaries.
        let mut b = EditBuffer::from_text("é🦀");
        assert_eq!(b.caret(), "é🦀".len());
        b.move_left(); // skip the whole crab
        assert_eq!(b.caret(), "é".len());
        b.move_left();
        assert_eq!(b.caret(), 0);
        b.delete(); // removes "é", not a partial byte
        assert_eq!(b.text(), "🦀");
    }

    #[test]
    fn edit_buffer_apply_dispatches_keys() {
        let mut b = EditBuffer::new();
        for k in [
            KeyInput::Text("hi".into()),
            KeyInput::Left,
            KeyInput::Text("X".into()),
        ] {
            b.apply(&k);
        }
        assert_eq!((b.text(), b.caret()), ("hXi", 2));
    }

    #[test]
    fn edit_buffer_shift_select_and_replace() {
        let mut b = EditBuffer::from_text("hello");
        b.home(); // caret at 0, no selection
        assert_eq!(b.selection(), None);
        b.select_right();
        b.select_right();
        b.select_right(); // select "hel"
        assert_eq!(b.selection(), Some((0, 3)));
        // Typing replaces the selection.
        b.insert("X");
        assert_eq!((b.text(), b.caret(), b.selection()), ("Xlo", 1, None));
    }

    #[test]
    fn edit_buffer_select_all_then_backspace_clears() {
        let mut b = EditBuffer::from_text("abc");
        b.select_all();
        assert_eq!(b.selection(), Some((0, 3)));
        b.backspace(); // deletes the whole selection
        assert!(b.is_empty());
        assert_eq!(b.selection(), None);
    }

    #[test]
    fn edit_buffer_plain_motion_collapses_selection() {
        let mut b = EditBuffer::from_text("abcd");
        b.home();
        b.select_right();
        b.select_right(); // select "ab", caret at 2
        assert_eq!(b.selection(), Some((0, 2)));
        b.move_left(); // collapse to selection start
        assert_eq!((b.caret(), b.selection()), (0, None));
    }

    #[test]
    fn edit_buffer_select_respects_utf8() {
        let mut b = EditBuffer::from_text("é🦀");
        b.home();
        b.select_right(); // selects "é" (2 bytes), not a partial byte
        assert_eq!(b.selection(), Some((0, "é".len())));
        b.select_right(); // extends over "🦀"
        assert_eq!(b.selection(), Some((0, "é🦀".len())));
    }

    #[test]
    fn edit_buffer_place_caret_and_extend() {
        let mut b = EditBuffer::from_text("hello");
        b.place_caret(2); // click between "he" and "llo"
        assert_eq!((b.caret(), b.selection()), (2, None));
        b.extend_to(4); // drag right to select "ll"
        assert_eq!(b.selection(), Some((2, 4)));
        b.extend_to(0); // drag left past the anchor
        assert_eq!(b.selection(), Some((0, 2)));
        // Out-of-range / mid-codepoint indices snap safely.
        let mut u = EditBuffer::from_text("é🦀");
        u.place_caret(999);
        assert_eq!(u.caret(), "é🦀".len());
        u.place_caret(1); // inside "é" -> snaps down to 0
        assert_eq!(u.caret(), 0);
    }

    #[test]
    fn edit_buffer_copy_paste_round_trips_through_the_clipboard() {
        let mut b = EditBuffer::from_text("Clip");
        b.apply(&KeyInput::SelectAll);
        b.apply(&KeyInput::Copy); // clipboard = "Clip"
        b.apply(&KeyInput::End); // collapse selection, caret at end
        b.apply(&KeyInput::Paste); // insert "Clip" → "ClipClip"
        assert_eq!(b.text(), "ClipClip");
        assert_eq!(crate::core::clipboard_text(), "Clip");

        // Cut removes the selection and leaves it on the clipboard.
        b.apply(&KeyInput::SelectAll);
        b.apply(&KeyInput::Cut);
        assert_eq!(b.text(), "");
        assert_eq!(crate::core::clipboard_text(), "ClipClip");
    }

    #[test]
    fn edit_buffer_enter_and_line_navigation() {
        let mut b = EditBuffer::new();
        b.apply(&KeyInput::Text("ab".into()));
        b.apply(&KeyInput::Enter); // newline → "ab\n", caret at 3
        b.apply(&KeyInput::Text("cde".into())); // "ab\ncde", caret at end (6)
        assert_eq!((b.text(), b.caret()), ("ab\ncde", 6));
        // Up keeps the byte column (3 → clamped to line 0 length 2).
        b.up();
        assert_eq!(b.caret(), 2);
        // Home/End are line-aware.
        b.home();
        assert_eq!(b.caret(), 0);
        b.end();
        assert_eq!(b.caret(), 2); // end of line 0, before '\n'
        // Down returns to line 1 at the same column.
        b.down();
        assert_eq!(b.caret(), 5); // "ab\ncd|e"
    }

    #[test]
    fn edit_buffer_shift_down_selects_across_lines() {
        let mut b = EditBuffer::from_text("ab\ncd");
        b.home(); // caret to start of line 1 ("cd")
        b.up(); // line 0
        b.home(); // caret 0
        b.apply(&KeyInput::SelectDown); // extend down one line
        let (s, e) = b.selection().expect("selection");
        assert_eq!(s, 0);
        assert!(e >= 3, "selection should reach into line 1, got {e}");
    }

    #[test]
    fn text_editor_carries_caret_on_its_text_leaf() {
        let theme = Theme::light();
        let mut cx = Cx::new(&theme);
        let buf = EditBuffer::from_text("hello");
        let field = text_editor(&mut cx, &theme, &buf, |s: &mut EditBuffer| s);
        // The inner text leaf carries the caret byte index for the focus overlay;
        // the field itself carries focus + text-pointer handles.
        assert!(field.focus.is_some());
        assert!(field.text_pos.is_some());
        let ElementKind::Stack { children, .. } = &field.kind else {
            panic!("text_editor should be a stack");
        };
        assert_eq!(children[0].caret, Some(5));
    }

    /// Feed each key to `b` in order.
    fn keys(b: &mut EditBuffer, ks: &[KeyInput]) {
        for k in ks {
            b.apply(k);
        }
    }

    fn typed(b: &mut EditBuffer, s: &str) {
        for c in s.chars() {
            b.apply(&KeyInput::Text(c.to_string()));
        }
    }

    #[test]
    fn undo_coalesces_consecutive_typing() {
        let mut b = EditBuffer::new();
        assert!(!b.can_undo());
        typed(&mut b, "hello");
        assert!(b.can_undo());
        b.apply(&KeyInput::Undo);
        assert_eq!(b.text(), "");
        assert_eq!(b.caret(), 0);
        assert!(!b.can_undo());
        b.apply(&KeyInput::Redo);
        assert_eq!(b.text(), "hello");
        assert_eq!(b.caret(), 5);
    }

    #[test]
    fn caret_move_ends_the_typing_group() {
        let mut b = EditBuffer::new();
        typed(&mut b, "ab");
        // Left then Right returns to the same spot, but still ends the group.
        keys(&mut b, &[KeyInput::Left, KeyInput::Right]);
        typed(&mut b, "cd");
        assert_eq!(b.text(), "abcd");
        b.undo();
        assert_eq!(b.text(), "ab");
        assert_eq!(b.caret(), 2);
        b.undo();
        assert_eq!(b.text(), "");
        assert!(!b.undo());
    }

    #[test]
    fn different_edit_kind_ends_the_group() {
        let mut b = EditBuffer::new();
        typed(&mut b, "abc");
        keys(&mut b, &[KeyInput::Backspace, KeyInput::Backspace]);
        typed(&mut b, "XY");
        assert_eq!(b.text(), "aXY");
        b.undo(); // typing "XY"
        assert_eq!(b.text(), "a");
        b.undo(); // both backspaces, as one step
        assert_eq!(b.text(), "abc");
        assert_eq!(b.caret(), 3);
        b.undo(); // typing "abc"
        assert_eq!(b.text(), "");
    }

    #[test]
    fn delete_presses_coalesce() {
        let mut b = EditBuffer::from_text("abcdef");
        b.place_caret(1);
        keys(
            &mut b,
            &[KeyInput::Delete, KeyInput::Delete, KeyInput::Delete],
        );
        assert_eq!(b.text(), "aef");
        b.undo();
        assert_eq!(b.text(), "abcdef");
        assert_eq!(b.caret(), 1);
    }

    #[test]
    fn newline_and_paste_are_their_own_steps() {
        let mut b = EditBuffer::new();
        typed(&mut b, "ab");
        b.apply(&KeyInput::Enter);
        typed(&mut b, "cd");
        assert_eq!(b.text(), "ab\ncd");
        b.undo();
        assert_eq!(b.text(), "ab\n");
        b.undo();
        assert_eq!(b.text(), "ab");
        b.undo();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn undo_restores_a_replaced_selection() {
        let mut b = EditBuffer::from_text("hello world");
        b.place_caret(6);
        b.extend_to(11);
        typed(&mut b, "there");
        assert_eq!(b.text(), "hello there");
        // Replacing the selection plus the typing that followed it.
        b.undo();
        assert_eq!(b.text(), "hello t");
        b.undo();
        assert_eq!(b.text(), "hello world");
        assert_eq!(b.selection(), Some((6, 11)));
        b.redo();
        assert_eq!(b.text(), "hello t");
        assert_eq!(b.selection(), None);
        assert_eq!(b.caret(), 7);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut b = EditBuffer::new();
        typed(&mut b, "ab");
        b.undo();
        assert!(b.can_redo());
        typed(&mut b, "x");
        assert!(!b.can_redo());
        assert!(!b.redo());
        assert_eq!(b.text(), "x");
    }

    #[test]
    fn undo_redo_round_trip_multibyte() {
        let mut b = EditBuffer::from_text("é🦀z");
        b.place_caret("é🦀".len());
        b.backspace(); // removes the crab
        b.apply(&KeyInput::Left);
        typed(&mut b, "日本");
        assert_eq!(b.text(), "日本éz");
        b.undo();
        assert_eq!(b.text(), "éz");
        b.undo();
        assert_eq!(b.text(), "é🦀z");
        assert_eq!(b.caret(), "é🦀".len());
        b.redo();
        b.redo();
        assert_eq!(b.text(), "日本éz");
        assert_eq!(b.caret(), "日本".len());
        b.clear_history();
        assert!(!b.can_undo() && !b.can_redo());
    }

    #[test]
    fn equality_ignores_history() {
        let mut a = EditBuffer::new();
        typed(&mut a, "hi");
        assert_eq!(a, EditBuffer::from_text("hi"));
    }

    #[test]
    fn word_movement_stops_at_word_and_punctuation_runs() {
        let text = "foo bar_baz, qux";
        let mut b = EditBuffer::from_text(text);
        b.place_caret(0);
        let mut stops = Vec::new();
        for _ in 0..5 {
            b.apply(&KeyInput::WordRight);
            stops.push(b.caret());
        }
        // foo| bar_baz|,| qux| (then stays at the end)
        assert_eq!(stops, vec![3, 11, 12, 16, 16]);
        let mut back = Vec::new();
        for _ in 0..5 {
            b.apply(&KeyInput::WordLeft);
            back.push(b.caret());
        }
        assert_eq!(back, vec![13, 11, 4, 0, 0]);
    }

    #[test]
    fn word_movement_handles_unicode() {
        // Accented and CJK letters are word chars; the em dash is punctuation.
        let text = "café — 日本語 x";
        let mut b = EditBuffer::from_text(text);
        b.place_caret(0);
        b.word_right();
        assert_eq!(&text[..b.caret()], "café");
        b.word_right();
        assert_eq!(&text[..b.caret()], "café —");
        b.word_right();
        assert_eq!(&text[..b.caret()], "café — 日本語");
        b.word_left();
        assert_eq!(&text[..b.caret()], "café — ");
        // Every stop lands on a char boundary.
        b.place_caret(text.len());
        while b.caret() > 0 {
            b.word_left();
            assert!(text.is_char_boundary(b.caret()));
        }
    }

    #[test]
    fn word_movement_crosses_lines() {
        let mut b = EditBuffer::from_text("one\n  two");
        b.word_left();
        assert_eq!(b.caret(), 6);
        b.word_left();
        assert_eq!(b.caret(), 0);
        b.word_right();
        assert_eq!(b.caret(), 3);
        b.word_right();
        assert_eq!(b.caret(), 9);
    }

    #[test]
    fn shift_word_movement_extends_selection() {
        let mut b = EditBuffer::from_text("alpha beta gamma");
        b.place_caret(6);
        keys(
            &mut b,
            &[KeyInput::SelectWordRight, KeyInput::SelectWordRight],
        );
        assert_eq!(b.selection(), Some((6, 16)));
        assert_eq!(b.selected_text().as_deref(), Some("beta gamma"));
        // Back past the anchor flips the selection to the other side.
        keys(
            &mut b,
            &[
                KeyInput::SelectWordLeft,
                KeyInput::SelectWordLeft,
                KeyInput::SelectWordLeft,
            ],
        );
        assert_eq!(b.selection(), Some((0, 6)));
        // A plain word move collapses it.
        b.apply(&KeyInput::WordRight);
        assert_eq!(b.selection(), None);
        assert_eq!(b.caret(), 5);
    }

    #[test]
    fn word_delete_backward_and_forward() {
        let mut b = EditBuffer::from_text("let x = foo.bar;");
        b.apply(&KeyInput::DeleteWordBackward); // ";"
        assert_eq!(b.text(), "let x = foo.bar");
        b.apply(&KeyInput::DeleteWordBackward); // "bar"
        assert_eq!(b.text(), "let x = foo.");
        b.place_caret(3);
        b.apply(&KeyInput::DeleteWordForward); // " x"
        assert_eq!(b.text(), "let = foo.");
        assert_eq!(b.caret(), 3);
        // Each word delete is its own undo step.
        b.undo();
        assert_eq!(b.text(), "let x = foo.");
        b.undo();
        assert_eq!(b.text(), "let x = foo.bar");
        // At the edges word deletes are no-ops (and record nothing).
        let mut e = EditBuffer::from_text("ab");
        e.place_caret(0);
        e.delete_word_backward();
        e.place_caret(2);
        e.delete_word_forward();
        assert_eq!(e.text(), "ab");
        assert!(!e.can_undo());
    }

    #[test]
    fn word_delete_removes_selection_first() {
        let mut b = EditBuffer::from_text("one two three");
        b.place_caret(4);
        b.extend_to(6);
        b.apply(&KeyInput::DeleteWordBackward);
        assert_eq!(b.text(), "one o three");
        assert_eq!(b.caret(), 4);
    }

    #[test]
    fn word_delete_multibyte() {
        let mut b = EditBuffer::from_text("naïve 🦀🦀");
        b.delete_word_backward(); // the crab pair is one punctuation run
        assert_eq!(b.text(), "naïve ");
        b.delete_word_backward();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn word_range_prefers_the_word_next_to_the_click() {
        let b = EditBuffer::from_text("hello, world  x");
        // Inside a word.
        assert_eq!(b.word_range_at(2), (0, 5));
        // Just past the word's last letter (clicked its right half).
        assert_eq!(b.word_range_at(5), (0, 5));
        // Between ',' and ' ': punctuation beats space.
        assert_eq!(b.word_range_at(6), (5, 6));
        // Inside a run of spaces.
        assert_eq!(b.word_range_at(13), (12, 14));
        // Multibyte word; an index inside a char snaps down to its boundary.
        let m = EditBuffer::from_text("über 日本語");
        assert_eq!(m.word_range_at(1), (0, "über".len()));
        assert_eq!(m.word_range_at(7), ("über ".len(), m.text().len()));
        // Between two line breaks there is nothing to select.
        let n = EditBuffer::from_text("a\n\nb");
        assert_eq!(n.word_range_at(2), (2, 2));
    }

    #[test]
    fn select_word_and_line() {
        let mut b = EditBuffer::from_text("first line\nsecond line here");
        b.place_caret(14);
        b.apply(&KeyInput::SelectWord);
        assert_eq!(b.selected_text().as_deref(), Some("second"));
        b.apply(&KeyInput::SelectLine);
        assert_eq!(b.selected_text().as_deref(), Some("second line here"));
        b.place_caret(3);
        b.select_line();
        assert_eq!(b.selected_text().as_deref(), Some("first line"));
        // Selecting doesn't record an edit, but ends a typing group.
        let mut t = EditBuffer::new();
        typed(&mut t, "ab");
        t.select_word();
        typed(&mut t, "c");
        assert_eq!(t.text(), "c");
        t.undo();
        assert_eq!(t.text(), "ab");
        assert_eq!(t.selection(), Some((0, 2)));
    }

    #[test]
    fn text_editor_treats_select_line_as_select_all() {
        struct St {
            b: EditBuffer,
        }
        let theme = Theme::light();
        let mut st = St {
            b: EditBuffer::from_text("a\nb"),
        };
        let mut cx = Cx::new(&theme);
        let field = text_editor(&mut cx, &theme, &st.b, |s: &mut St| &mut s.b);
        let mut handlers = cx.into_handlers();
        st.b.place_caret(0);
        assert!(handlers.dispatch_key(field.focus.unwrap(), &KeyInput::SelectLine, &mut st));
        assert_eq!(st.b.selection(), Some((0, 3)));
    }

    #[test]
    fn arrows_move_visually_through_right_to_left_text() {
        // "abc אבג": a0 b1 c2 ' '3, then א at byte 4, ב 6, ג 8; 10 bytes.
        let mut b = EditBuffer::from_text("abc אבג");
        b.place_caret(3);
        b.move_right(); // after the space: the Hebrew's visual left end...
        assert_eq!(b.caret(), 4 + 6, "...which is the end of the text");
        b.move_right();
        assert_eq!(b.caret(), 8, "then back through it, to before ג");
        b.move_right();
        b.move_right();
        assert_eq!(b.caret(), 4, "the visual right end is before א");
        b.move_right();
        assert_eq!(b.caret(), 4, "and that's the end of the line");
        b.move_left();
        assert_eq!(b.caret(), 6);

        // A right-to-left line: Left moves forward through the text.
        let mut h = EditBuffer::from_text("שלום");
        h.place_caret(0);
        h.move_left();
        assert_eq!(h.caret(), 2);
        h.move_right();
        assert_eq!(h.caret(), 0);
        // Shift+Left extends forward too, and the selection stays logical.
        h.select_left();
        h.select_left();
        assert_eq!(h.selection(), Some((0, 4)));
        // Left collapses a right-to-left selection to its far (logical) end.
        h.move_left();
        assert_eq!(h.caret(), 4);
    }

    #[test]
    fn deletion_stays_logical_in_mixed_text() {
        let mut b = EditBuffer::from_text("abc אבג");
        b.backspace();
        assert_eq!(
            b.text(),
            "abc אב",
            "Backspace removes the last letter typed"
        );
        b.place_caret(4);
        b.delete();
        assert_eq!(b.text(), "abc ב");
    }

    #[test]
    fn direction_can_be_forced() {
        let mut b = EditBuffer::from_text("abc").with_direction(TextDirection::Rtl);
        assert_eq!(b.direction(), TextDirection::Rtl);
        // Latin letters keep their order inside a right-to-left line: the
        // caret before "a" is at the line's visual left edge.
        b.place_caret(0);
        b.move_left();
        assert_eq!(b.caret(), 0);
        b.move_right();
        assert_eq!(b.caret(), 1);
        // At the end of a left-to-right line, Right goes on to the next line.
        let mut m = EditBuffer::from_text("ab\nאב");
        m.place_caret(2);
        m.move_right();
        assert_eq!(m.caret(), 3);
    }
}
