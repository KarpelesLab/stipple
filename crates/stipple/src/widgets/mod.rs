//! Stipple's self-drawn widget library.
//!
//! Every widget is composed from the `stipple::core` [`Element`] IR and drawn by
//! `oxideav-raster` — there are no native controls. The builders here read
//! their colors and metrics from a [`Theme`], so a single theme swap re-skins
//! the whole UI.
//!
//! Available widgets: structure (`column`, `row`, `spacer`, `panel`), content
//! (`label`, `heading`, `swatch`, `divider`), and interactive controls —
//! `button_variant` (Primary/Secondary/Ghost/Danger) / `button_labeled`
//! ([`Element::on_tap`]), `text_field` / `text_editor` ([`Element::on_key`]; the
//! latter is caret-aware via [`EditBuffer`]), `checkbox`, `switch`, and `slider`
//! ([`Element::on_drag`]). All colors and metrics come from the [`Theme`], so
//! swapping or customizing the theme reskins everything.

#![forbid(unsafe_code)]

use crate::core::{Align, Anchor, Axis, BoxStyle, Cx, Element, KeyInput, OverlaySpec, ViewportId};
use crate::geometry::{Insets, Point};
use crate::render::Color;
use crate::style::Theme;

/// A vertical container.
pub fn column(children: Vec<Element>) -> Element {
    Element::stack(Axis::Vertical, children)
}

/// A horizontal container.
pub fn row(children: Vec<Element>) -> Element {
    Element::stack(Axis::Horizontal, children)
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditBuffer {
    text: String,
    caret: usize,
    /// The fixed end of an active selection (the caret is the moving end);
    /// `None` when nothing is selected.
    anchor: Option<usize>,
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
        }
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

    /// Delete the active selection (if any), leaving the caret at its start.
    /// Returns `true` if something was removed.
    fn delete_selection(&mut self) -> bool {
        if let Some((s, e)) = self.selection() {
            self.text.replace_range(s..e, "");
            self.caret = s;
            self.anchor = None;
            true
        } else {
            self.anchor = None;
            false
        }
    }

    /// Insert `s`, replacing the selection first, and advance the caret past it.
    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        self.text.insert_str(self.caret, s);
        self.caret += s.len();
    }

    /// Delete the selection, or the character before the caret (Backspace).
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if let Some(prev) = self.prev_boundary(self.caret) {
            self.text.replace_range(prev..self.caret, "");
            self.caret = prev;
        }
    }

    /// Delete the selection, or the character at the caret (Delete).
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if let Some(next) = self.next_boundary(self.caret) {
            self.text.replace_range(self.caret..next, "");
        }
    }

    /// Move the caret one character left, collapsing any selection to its start.
    pub fn move_left(&mut self) {
        if let Some((s, _)) = self.selection() {
            self.caret = s;
        } else if let Some(prev) = self.prev_boundary(self.caret) {
            self.caret = prev;
        }
        self.anchor = None;
    }

    /// Move the caret one character right, collapsing any selection to its end.
    pub fn move_right(&mut self) {
        if let Some((_, e)) = self.selection() {
            self.caret = e;
        } else if let Some(next) = self.next_boundary(self.caret) {
            self.caret = next;
        }
        self.anchor = None;
    }

    /// Move the caret to the start of the current line, clearing the selection.
    pub fn home(&mut self) {
        self.caret = self.line_start(self.caret);
        self.anchor = None;
    }

    /// Move the caret to the end of the current line, clearing the selection.
    pub fn end(&mut self) {
        self.caret = self.line_end(self.caret);
        self.anchor = None;
    }

    /// Move the caret up one line (same byte column), clearing the selection.
    pub fn up(&mut self) {
        self.caret = self.caret_up(self.caret);
        self.anchor = None;
    }

    /// Move the caret down one line (same byte column), clearing the selection.
    pub fn down(&mut self) {
        self.caret = self.caret_down(self.caret);
        self.anchor = None;
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
    }

    /// Extend the selection one character left.
    pub fn select_left(&mut self) {
        self.extend(|b| {
            if let Some(prev) = b.prev_boundary(b.caret) {
                b.caret = prev;
            }
        });
    }

    /// Extend the selection one character right.
    pub fn select_right(&mut self) {
        self.extend(|b| {
            if let Some(next) = b.next_boundary(b.caret) {
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
    }

    /// Place the caret at byte index `i` (snapped to a char boundary) and clear
    /// any selection. For click-to-position from a pointer hit-test.
    pub fn place_caret(&mut self, i: usize) {
        self.caret = self.snap(i);
        self.anchor = None;
    }

    /// Move the caret (the selection's moving end) to byte index `i`, anchoring
    /// the selection at the prior caret if not already selecting. For
    /// drag-to-select from a pointer hit-test.
    pub fn extend_to(&mut self, i: usize) {
        if self.anchor.is_none() {
            self.anchor = Some(self.caret);
        }
        self.caret = self.snap(i);
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
            KeyInput::Text(t) => self.insert(t),
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
                    self.insert(&text);
                }
            }
            // Enter inserts a newline (the buffer is multi-line capable).
            KeyInput::Enter => self.insert("\n"),
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
    // A leading space keeps an empty field from collapsing to zero height; the
    // caret then sits at index 0 of that placeholder.
    let shown = if buffer.is_empty() {
        String::from(" ")
    } else {
        buffer.text().to_string()
    };
    let text = Element::text(shown, theme.font_size, theme.palette.text)
        .caret(buffer.caret())
        .selection(buffer.selection());
    Element::stack(Axis::Horizontal, vec![text])
        .fill(theme.palette.surface)
        .radius(theme.radius)
        .border(theme.palette.border, 1.0)
        .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
        .align(Align::Start, Align::Center)
        .width(200.0)
        .on_key(cx, move |s, k| field(s).apply(k))
        .on_text_pos(cx, move |s, index, extend| {
            let b = field(s);
            if extend {
                b.extend_to(index);
            } else {
                b.place_caret(index);
            }
        })
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
    let shown = if buffer.is_empty() {
        String::from(" ")
    } else {
        buffer.text().to_string()
    };
    let text = Element::text(shown, theme.font_size, theme.palette.text)
        .caret(buffer.caret())
        .selection(buffer.selection());
    Element::stack(Axis::Horizontal, vec![text])
        .fill(theme.palette.surface)
        .radius(theme.radius)
        .border(theme.palette.border, 1.0)
        .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
        .align(Align::Start, Align::Start)
        .width(280.0)
        .height(120.0)
        .on_key(cx, move |s, k| field(s).apply(k))
        .on_text_pos(cx, move |s, index, extend| {
            let b = field(s);
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
pub fn setting_row(theme: &Theme, accent: Color) -> Element {
    row(vec![
        swatch(accent, 24.0, theme.radius / 2.0),
        spacer(),
        button(theme),
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
}

/// A simple busy indicator: a primary-colored ring. Static for now; wire it to
/// the animation clock for a spin later.
pub fn spinner(theme: &Theme) -> Element {
    Element::boxed(BoxStyle {
        fill: None,
        radius: 11.0,
        border: Some((theme.palette.primary, 3.0)),
    })
    .width(22.0)
    .height(22.0)
}

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
        // Two filled leaves (swatch + button); the spacer is empty.
        let theme = Theme::light();
        let scene = render_view(
            &setting_row(&theme, Color::rgb(200, 80, 80)),
            Size::new(300.0, 40.0),
            &theme,
            None,
        );
        assert_eq!(scene.len(), 2);
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
}
