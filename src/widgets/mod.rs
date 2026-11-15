//! Stipple's self-drawn widget library.
//!
//! Every widget is composed from the `stipple::core` [`Element`] IR and drawn by
//! `oxideav-raster` — there are no native controls. Widgets take the view's
//! [`Cx`] first and read their colors and metrics from its [`Theme`], so a
//! single theme swap re-skins the whole UI ([`Cx::themed`] swaps it for part
//! of a view).
//!
//! Available widgets: structure (`column`, `row`, `grid`, `spacer`, `panel`,
//! `scroll`), content (`label`, `heading`, `paragraph`, `swatch`, `divider`),
//! and interactive controls: `button` (and [`Button`] for the other
//! [`Variant`]s and custom content; [`Element::on_tap`]), `text_field` /
//! `text_area` (editing a [`TextInput`]), `checkbox`, `switch`, `radio`,
//! `slider`, `select`, `number_input`, `password_field`, and chips (`chip`,
//! `choice_chips`, `filter_chips`).

#![forbid(unsafe_code)]

pub(crate) mod alert;
#[cfg(test)]
mod alert_tests;
mod chips;
#[cfg(test)]
mod chips_tests;
mod color_picker;
pub(crate) mod menus;
#[cfg(test)]
mod menus_tests;
mod motion;
#[cfg(test)]
mod motion_tests;
mod navigation;
#[cfg(test)]
mod navigation_tests;
mod number;
mod password;
mod preedit;
mod select;
mod split;
mod tab_bar;
#[cfg(test)]
mod tab_bar_tests;
mod text_input;
mod toast;
mod toggle;
mod toolbar;

pub use chips::{Chip, chip, choice_chips, filter_chips};
pub use color_picker::{ColorPicker, color_button, color_picker};
pub use navigation::{
    NavBadge, NavIcon, NavItem, NavLook, NavigationBar, navigation_bar, navigation_rail, page_stack,
};
pub use number::{NumberInput, number_input};
pub use password::{PasswordField, password_field};
pub use preedit::DisplayText;
pub use select::{Select, select};
pub use split::{Split, SplitPane, split_pane};
pub use tab_bar::{TAB_MAX_WIDTH, TAB_MIN_WIDTH, Tab, TabBar, TabBarState, TabIndicator, tab_bar};
pub(crate) use text_input::FieldHooks;
pub use text_input::{TextInput, text_area, text_field};
pub use toast::{NARROW_WINDOW, Toast, ToastId, ToastKind, ToastPlacement, Toasts, toasts};
pub use toggle::{Toggle, checkbox, radio, switch};
pub use toolbar::{ToolButton, tool_button, tool_separator, toolbar};

use crate::core::{
    Align, Anchor, AnimProp, Axis, BoxStyle, Cx, Effect, Element, KeyInput, OverlaySpec, Role,
    Track, Transition, ViewportId,
};
use crate::geometry::{Insets, Point, Transform};
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

/// A vertical scroll viewport: a column whose content, added with
/// [`children`](Element::children) or [`child`](Element::child), may be
/// taller than the viewport. It overflows and is clipped to the viewport;
/// wheel events over it (and finger drags on a touch screen) scroll it.
///
/// It fills the height its parent column leaves free, so a list under a
/// toolbar takes the rest of the window:
///
/// ```
/// use stipple::prelude::*;
/// struct App { items: Vec<String> }
/// fn view(state: &App, cx: &mut Cx<App>) -> Element {
///     column(vec![
///         heading(cx, "Items"),
///         scroll(cx).children(state.items.iter().map(|item| label(cx, item)).collect()),
///     ])
/// }
/// # let theme = Theme::light();
/// # let _ = view(&App { items: vec!["a".into()] }, &mut Cx::new(&theme));
/// ```
///
/// How tall it is, unless [`height`](Element::height) fixes it:
///
/// - **In a column**, it grows into the free height (up to a
///   [`max_height`](Element::max_height)), and when the column's other
///   children don't leave it room, it shrinks to make room for them, down
///   to 100 pixels (or its content's height, if less; a
///   [`min_height`](Element::min_height) sets another floor).
/// - **Where nothing gives it a height** (a column that is as tall as its
///   content, in a dialog, a popover, a row), it is as tall as its content,
///   but no taller than the space its parent was given: a short list shows
///   whole, and a long one scrolls in the space there is. Inside another
///   vertical scroll container, that's the outer one's viewport.
///
/// [`scroll_horizontal`] is the same sideways.
///
/// Call it once per frame for each scroll container, in the same order, so
/// each keeps its scroll position across rebuilds.
pub fn scroll<S>(cx: &mut Cx<S>) -> Element {
    let id = cx.register_scroll();
    Element::stack(Axis::Vertical, Vec::new()).scrollable(id)
}

/// A sideways scroll viewport (a strip of chips, a carousel): a row whose
/// content, added with [`children`](Element::children) or
/// [`child`](Element::child), lays out left to right at its natural width
/// and is clipped to the viewport. A horizontal wheel or trackpad swipe over
/// it scrolls it (a vertical wheel too, when no vertical container is under
/// the pointer), and on a touch screen a sideways finger drag scrolls and
/// flings it. The content measures with no width limit, so a wrapping row (a
/// group of [`choice_chips`] or [`filter_chips`]) stays on one line in it.
/// Inside a [`scroll`] list, a drag picks the container along its direction.
///
/// Its width follows [`scroll`]'s rule for height: in a column it spans the
/// column's width (as any child does); in a row it grows into the free
/// width and shrinks when the row's other children need the room; elsewhere
/// it is as wide as its content, up to the width there is.
/// [`width`](Element::width) fixes it.
pub fn scroll_horizontal<S>(cx: &mut Cx<S>) -> Element {
    let id = cx.register_scroll();
    Element::stack(Axis::Horizontal, Vec::new())
        .scrollable(id)
        .align(Align::Start, Align::Stretch)
}

/// An embedded-content viewport of `width`×`height` logical pixels for the
/// given [`ViewportId`], framed with a hairline themed border. The app fills the
/// reserved rect with externally-rendered pixels (a browser page, video, or a
/// sandboxed content process's GPU surface) by locating it by `id`; until then
/// a neutral placeholder shows. The `id` is caller-chosen so it stays stable across frames
/// and correlates with the content source.
pub fn viewport<S>(cx: &Cx<S>, id: ViewportId, width: f64, height: f64) -> Element {
    let theme = cx.theme();
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
/// comfortable padding. It's a column: add its content with
/// [`children`](Element::children) (or [`child`](Element::child)), which
/// can use `cx` to register handlers:
///
/// ```
/// use stipple::prelude::*;
/// struct App { n: i64 }
/// fn view(state: &App, cx: &mut Cx<App>) -> Element {
///     panel(cx).children(vec![
///         heading(cx, format!("{}", state.n)),
///         button(cx, "+1").on_tap(cx, |s| s.n += 1),
///     ])
/// }
/// # let theme = Theme::light();
/// # let _ = view(&App { n: 0 }, &mut Cx::new(&theme));
/// ```
pub fn panel<S>(cx: &Cx<S>) -> Element {
    panel_frame(cx.theme(), Vec::new())
}

/// [`panel`]'s card around `children`.
fn panel_frame(theme: &Theme, children: Vec<Element>) -> Element {
    if let Some(frame) = theme.skin.and_then(|s| s.panel.as_ref()) {
        return Element::stack(Axis::Vertical, children)
            .padding(Insets::uniform(theme.spacing.lg))
            .background_image(frame)
            .gap(theme.spacing.md);
    }
    Element::stack(Axis::Vertical, children)
        .fill(theme.palette.surface)
        .radius(theme.radius)
        .border(theme.palette.border, 1.0)
        .padding(Insets::uniform(theme.spacing.lg))
        .gap(theme.spacing.md)
}

/// A single line of text in the theme's default text color and body size.
pub fn label<S>(cx: &Cx<S>, text: impl Into<String>) -> Element {
    let theme = cx.theme();
    Element::text(text, theme.typography.body, theme.palette.text)
}

/// A larger heading in the theme's heading size.
pub fn heading<S>(cx: &Cx<S>, text: impl Into<String>) -> Element {
    let theme = cx.theme();
    Element::text(text, theme.typography.heading, theme.palette.text)
}

/// A block of body text that word-wraps to its container width across as many
/// lines as needed. Place it in a cross-stretch container (like [`panel`]) so it
/// takes the full width.
pub fn paragraph<S>(cx: &Cx<S>, text: impl Into<String>) -> Element {
    let theme = cx.theme();
    Element::text(text, theme.typography.body, theme.palette.text).wrap()
}

/// Visual emphasis level for a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
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

/// A button: a label (or other content) on a surface styled per
/// [`Variant`], in the theme's colors. [`button`] is the common case, a
/// primary button with a text label; build one of these for another variant
/// or for custom content, and finish it with [`view`](Self::view). Make it do
/// something with [`Element::on_tap`].
///
/// ```
/// use stipple::prelude::*;
/// struct Doc { saved: bool }
/// fn view(_: &Doc, cx: &mut Cx<Doc>) -> Element {
///     row(vec![
///         button(cx, "Save").on_tap(cx, |d| d.saved = true),
///         Button::new("Delete").variant(Variant::Danger).view(cx).on_tap(cx, |_| {}),
///         Button::from_content(label(cx, "★")).variant(Variant::Ghost).view(cx),
///     ])
/// }
/// # let theme = Theme::light();
/// # let _ = view(&Doc { saved: false }, &mut Cx::new(&theme));
/// ```
#[derive(Clone, Debug)]
pub struct Button {
    content: ButtonContent,
    variant: Variant,
}

#[derive(Clone, Debug)]
enum ButtonContent {
    Label(String),
    Element(Box<Element>),
}

impl Button {
    /// A primary button labeled `label`.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            content: ButtonContent::Label(label.into()),
            variant: Variant::Primary,
        }
    }

    /// A primary button showing `content` (an icon, an icon and a label)
    /// instead of a text label. The content keeps its own colors. Name an
    /// icon-only button with [`Element::a11y_label`].
    pub fn from_content(content: Element) -> Self {
        Self {
            content: ButtonContent::Element(Box::new(content)),
            variant: Variant::Primary,
        }
    }

    /// The emphasis: [`Variant::Primary`] (the default), `Secondary`, `Ghost`
    /// or `Danger`.
    pub fn variant(mut self, variant: Variant) -> Self {
        self.variant = variant;
        self
    }

    /// The button, in `cx`'s theme.
    pub fn view<S>(self, cx: &Cx<S>) -> Element {
        let theme = cx.theme();
        let p = &theme.palette;
        let variant = self.variant;
        let content = |ink: Color| match self.content {
            ButtonContent::Label(text) => Element::text(text, theme.typography.body, ink),
            ButtonContent::Element(el) => *el,
        };
        if let Some(skin) = theme.skin.and_then(|s| s.button.as_ref()) {
            // The skin's frames, per state, whatever the variant.
            let el = Element::stack(Axis::Horizontal, vec![content(p.text)])
                .padding(Insets::symmetric(theme.spacing.lg, theme.spacing.sm))
                .align(Align::Center, Align::Center);
            let (scale, press) = (theme.motion.press_scale, theme.motion.press);
            return skin
                .apply(el)
                .pressed(move |e| e.transform(Transform::IDENTITY.scale(scale)))
                .transition_for(AnimProp::Transform, Transition(press));
        }
        // The OS's primary and regular buttons; it has no equivalent of the others.
        let control = match variant {
            Variant::Primary => Some(NativeControl::Button { primary: true }),
            Variant::Secondary => Some(NativeControl::Button { primary: false }),
            Variant::Ghost | Variant::Danger => None,
        };
        if let Some(control) = control
            && let Some(deco) = theme.native_deco(control)
        {
            let ink = if matches!(variant, Variant::Primary) && native::primary_label_is_on_accent()
            {
                p.on_primary
            } else {
                p.text
            };
            let el = Element::stack(Axis::Horizontal, vec![content(ink)])
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
        let mut el = Element::stack(Axis::Horizontal, vec![content(ink)])
            .radius(theme.radius)
            .padding(Insets::symmetric(theme.spacing.lg, theme.spacing.sm))
            .align(Align::Center, Align::Center);
        if let Some(fill) = fill {
            el = el.fill(fill);
        }
        if let Some(border) = border {
            el = el.border(border, 1.0);
        }
        pressable(el, theme, fill)
    }
}

/// A primary button with a centered text `label`, sized to it plus padding.
/// For another [`Variant`] or custom content, build a [`Button`].
pub fn button<S>(cx: &Cx<S>, label: impl Into<String>) -> Element {
    Button::new(label).view(cx)
}

/// The theme `cx` builds with, by value, so a widget can keep reading it
/// while it registers handlers in `cx`.
pub(crate) fn theme_of<S>(cx: &Cx<S>) -> Theme {
    *cx.theme()
}

/// Give a self-drawn button its hover and press looks, animated with the
/// theme's motion: the hover overlay composited over its `fill` (twice over
/// while pressed, as the app's own highlight shows it) and, while pressed, a
/// shrink to [`MotionTokens::press_scale`](crate::style::MotionTokens::press_scale).
/// The looks are worked out from the variant's colors: a button given another
/// fill afterwards should get its own `.hovered` and `.pressed` too.
fn pressable(el: Element, theme: &Theme, fill: Option<Color>) -> Element {
    let m = theme.motion;
    let overlay = theme.palette.hover_overlay;
    let (hover, press) = (motion::lit(fill, overlay, 1), motion::lit(fill, overlay, 2));
    let scale = m.press_scale;
    el.hovered(move |e| e.fill(hover))
        .pressed(move |e| e.fill(press).transform(Transform::IDENTITY.scale(scale)))
        .transition_for(AnimProp::Fill, Transition(m.state))
        .transition_for(AnimProp::Transform, Transition(m.press))
}

/// A text buffer with a caret and a selection, for in-place editing: what a
/// [`TextInput`] (the state of a [`text_field`] or [`text_area`]) edits.
///
/// The caret is a byte index into the text, always kept on a UTF-8 char
/// boundary. Text is inserted and deleted at the caret; the arrow / Home / End
/// keys move it. Drive it by feeding [`KeyInput`]s to [`EditBuffer::apply`].
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
    /// Counts changes to `text`, so a field can tell whether a key changed it.
    revision: u64,
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
            revision: 0,
        }
    }

    /// Set the paragraph direction: [`TextDirection::Auto`] (the default) takes
    /// each line's direction from its first strong character. [`text_field`]
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
        self.revision += 1;
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
        self.revision += 1;
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
        self.revision += 1;
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

    /// Select `anchor..caret` (byte indices, snapped to char boundaries), the
    /// caret being the moving end; equal ends place a plain caret.
    pub fn set_selection(&mut self, anchor: usize, caret: usize) {
        let (anchor, caret) = (self.snap(anchor), self.snap(caret));
        self.caret = caret;
        self.anchor = (anchor != caret).then_some(anchor);
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
            KeyInput::SetSelection { anchor, caret } => self.set_selection(*anchor, *caret),
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

/// The text leaf showing a field's value `text` in `color`: for an empty
/// field, a lone space, which keeps it from collapsing to zero height (the
/// caret then sits at index 0 of it), and where its
/// [`placeholder`](Element::placeholder) goes.
pub(crate) fn field_text(text: String, theme: &Theme, color: Color) -> Element {
    if text.is_empty() {
        Element::text(" ", theme.font_size, color).empty_field(theme.palette.text_muted)
    } else {
        Element::text(text, theme.font_size, color)
    }
}

/// A horizontal slider with `value` in 0..=1. Dragging anywhere on the track
/// calls `on_change` with the new fractional position. Default width 160 px;
/// override with `.width(..)`.
pub fn slider<S>(
    cx: &mut Cx<S>,
    value: f64,
    on_change: impl FnMut(&mut S, f64) + 'static,
) -> Element {
    let theme = &theme_of(cx);
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
    .on_drag(cx, on_change)
}

/// A 1px themed divider line spanning the cross axis.
pub fn divider<S>(cx: &Cx<S>) -> Element {
    let theme = cx.theme();
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

/// A horizontal tab strip (segmented control): one cell per label, the
/// `selected` one filled with the accent color and the rest on the surface.
/// Tapping a tab runs `on_select` with that tab's index. Stateless — store the
/// selected index in your app state and render the matching content yourself.
///
/// When the selection changes, the accent fill slides (and stretches) from
/// the old tab to the new one under the labels, on the theme's
/// [`indicator`](crate::style::MotionTokens::indicator) motion, while the
/// tabs' own fills and labels fade on its `state` motion.
///
/// `on_select` must be `Clone` because it is stamped onto every tab (each with
/// its own index); a plain closure capturing `Copy`/`Clone` data satisfies this.
pub fn tabs<S>(
    cx: &mut Cx<S>,
    labels: &[&str],
    selected: usize,
    on_select: impl Fn(&mut S, usize) + Clone + 'static,
) -> Element {
    let theme = &theme_of(cx);
    let p = &theme.palette;
    let state = Transition(theme.motion.state);
    let pad = Insets::symmetric(theme.spacing.md, theme.spacing.sm);
    // Three layers of the same cells: the tabs' own fills, the selection
    // indicator over them, and the labels (which take the taps) on top. The
    // lower two size their cells with unseen, unread copies of the labels,
    // so all three line up. At rest this draws exactly what one row of
    // filled cells would.
    let cell = |content: Element| {
        Element::stack(Axis::Horizontal, vec![content])
            .radius(theme.radius)
            .padding(pad)
            .align(Align::Center, Align::Center)
            .grow(1.0)
    };
    let sizer = |text: &str| {
        Element::text(text, theme.typography.body, p.text)
            .opacity(0.0)
            .role(Role::Group)
    };
    let row = |cells: Vec<Element>| {
        Element::stack(Axis::Horizontal, cells)
            .gap(theme.spacing.xs)
            .align(Align::Start, Align::Stretch)
    };
    let labels_row: Vec<Element> = labels
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let ink = if i == selected { p.on_primary } else { p.text };
            let on = on_select.clone();
            let label = Element::text(*text, theme.typography.body, ink)
                .transition_for(AnimProp::TextColor, state);
            cell(label).on_tap(cx, move |s| on(s, i))
        })
        .collect();
    // The indicator is one element, wherever it is: keyed (by this strip's
    // first tap, unique in the window), the core's layout animation slides
    // it from the tab it was on.
    let key = (
        "stipple-tabs-indicator",
        labels_row.first().and_then(|c| c.action),
    );
    let indicator: Vec<Element> = labels
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let c = cell(sizer(text));
            if i == selected {
                c.fill(p.primary)
                    .border(p.border, 1.0)
                    .key(key)
                    .animate_layout(Transition(theme.motion.indicator))
            } else {
                c
            }
        })
        .collect();
    let fills: Vec<Element> = labels
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let c = cell(sizer(text));
            let c = if i == selected {
                c
            } else {
                c.fill(p.surface).border(p.border, 1.0)
            };
            c.transition_for(AnimProp::Fill, state)
                .transition_for(AnimProp::Border, state)
        })
        .collect();
    Element::layers(vec![row(fills), row(indicator), row(labels_row)])
}

/// A progress bar: a track with a primary-filled portion for `fraction` (0..=1).
/// Default width 200 px; override with `.width(..)`.
///
/// Under a theme with a [`Skin::bar`](crate::style::Skin::bar), it is the
/// skin's track and fill, 200 px wide (the fill is laid out for that width)
/// and as tall as the track's frame plus 8.
pub fn progress_bar<S>(cx: &Cx<S>, fraction: f64) -> Element {
    let theme = cx.theme();
    let f = fraction.clamp(0.0, 1.0);
    let control = NativeControl::Progress {
        value: (f * 1000.0).round() as u16,
    };
    if let Some(deco) = theme.native_deco(control) {
        return native_size(Element::boxed(BoxStyle::default()).native(deco), control);
    }
    if let Some(skin) = theme.skin.and_then(|s| s.bar.as_ref()) {
        let pad = skin.track.content_padding();
        let (w, h) = (200.0, 8.0 + pad.vertical());
        let inner = ((w - pad.horizontal()).max(0.0), 8.0);
        let filled = (inner.0 * f).round();
        return Element::stack(
            Axis::Horizontal,
            vec![skin.fill_element(inner, filled, theme.palette.primary)],
        )
        .background_image(&skin.track)
        .width(w)
        .height(h)
        .align(Align::Start, Align::Start);
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

/// Declare a non-modal dropdown overlay showing `menu`'s commands, anchored
/// with its top-left at `at`: the same [`Menu`](crate::core::menu::Menu) and
/// [`MenuItem`](crate::core::menu::MenuItem)s as the menu bar and context
/// menus, drawn like a context menu (submenus show but don't open). Picking a
/// command runs it, then `on_close`; so does a press outside the menu. Call
/// it while the menu should be open: it grows in when it opens, and when the
/// view stops calling this it's kept drawn (taking no input) while it fades
/// back out, on the theme's [`MotionTokens`] (`popup`, `popup_exit`).
///
/// ```
/// use stipple::prelude::*;
/// struct App { menu_open: bool, color: &'static str }
/// fn view(state: &App, cx: &mut Cx<App>) -> Element {
///     if state.menu_open {
///         let menu = Menu::new("Color")
///             .item(MenuItem::new("Red", |s: &mut App| s.color = "red"))
///             .item(MenuItem::new("Blue", |s: &mut App| s.color = "blue"));
///         open_menu(cx, Point::new(16.0, 48.0), menu, |s| s.menu_open = false);
///     }
///     button(cx, "Color…").on_tap(cx, |s| s.menu_open = true)
/// }
/// # let theme = Theme::light();
/// # let _ = view(&App { menu_open: true, color: "" }, &mut Cx::new(&theme));
/// ```
///
/// [`MotionTokens`]: crate::style::MotionTokens
pub fn open_menu<S: 'static>(
    cx: &mut Cx<S>,
    at: Point,
    menu: crate::core::menu::Menu<S>,
    on_close: impl FnMut(&mut S) + 'static,
) {
    let theme = &theme_of(cx);
    let on_close: crate::core::menu::MenuAction<S> =
        std::rc::Rc::new(std::cell::RefCell::new(on_close));
    // It grows in from its corner as it opens, and shrinks back as it closes.
    let content = menus::dropdown(cx, theme, &menu, &on_close)
        .enter(motion::popup_effect(&theme.motion, (0.0, 0.0)))
        .exit(motion::popup_exit(&theme.motion, (0.0, 0.0)));
    let dismiss = cx.register(move |s| (on_close.borrow_mut())(s));
    cx.overlay(OverlaySpec {
        content,
        anchor: Anchor::At(at),
        modal: false,
        dismiss: Some(dismiss),
    });
}

/// A modal dialog: a centered panel with `title`, its content (add it with
/// [`child`](Dialog::child) or [`children`](Dialog::children)) and a
/// right-aligned row of [`actions`](Dialog::actions) (buttons). Declare it
/// with [`open`](Dialog::open) on each build while it should show. A dark
/// scrim behind it blocks the tree; pressing the scrim runs `on_dismiss`. It
/// scales in when it opens and back out, with its scrim, when the view stops
/// declaring it (taking no input meanwhile), on the theme's `dialog` and
/// `dialog_exit` motion.
///
/// ```
/// use stipple::prelude::*;
/// struct App { confirming: bool }
/// fn view(state: &App, cx: &mut Cx<App>) -> Element {
///     if state.confirming {
///         dialog(cx, "Delete everything?", |s| s.confirming = false)
///             .child(label(cx, "This can't be undone."))
///             .actions(vec![
///                 button(cx, "Cancel").on_tap(cx, |s| s.confirming = false),
///                 button(cx, "Delete").on_tap(cx, |s| s.confirming = false),
///             ])
///             .open(cx);
///     }
///     button(cx, "Delete…").on_tap(cx, |s| s.confirming = true)
/// }
/// # let theme = Theme::light();
/// # let _ = view(&App { confirming: true }, &mut Cx::new(&theme));
/// ```
pub fn dialog<S>(
    cx: &mut Cx<S>,
    title: impl Into<String>,
    on_dismiss: impl FnMut(&mut S) + 'static,
) -> Dialog {
    Dialog {
        title: title.into(),
        dismiss: cx.register(on_dismiss),
        content: Vec::new(),
        actions: Vec::new(),
    }
}

/// A modal dialog being built: see [`dialog`]. It shows once
/// [`open`](Self::open) declares it.
#[derive(Debug)]
#[must_use = "a dialog shows only once `.open(cx)` declares it"]
pub struct Dialog {
    title: String,
    dismiss: crate::core::ActionId,
    content: Vec<Element>,
    actions: Vec<Element>,
}

impl Dialog {
    /// Add `child` to the dialog's content, under the title.
    pub fn child(mut self, child: Element) -> Self {
        self.content.push(child);
        self
    }

    /// Add `children` to the dialog's content, under the title.
    pub fn children(mut self, children: Vec<Element>) -> Self {
        self.content.extend(children);
        self
    }

    /// Put `actions` (buttons) in the row at the dialog's bottom, at its end.
    pub fn actions(mut self, actions: Vec<Element>) -> Self {
        self.actions.extend(actions);
        self
    }

    /// Declare the dialog as an overlay on this build.
    pub fn open<S>(self, cx: &mut Cx<S>) {
        let theme = &theme_of(cx);
        let mut children = vec![Element::text(
            self.title,
            theme.typography.heading,
            theme.palette.text,
        )];
        children.extend(self.content);
        if !self.actions.is_empty() {
            children.push(
                row(self.actions)
                    .gap(theme.spacing.sm)
                    .align(Align::End, Align::Center),
            );
        }
        let content = panel_frame(theme, children)
            .width(340.0)
            .enter(motion::dialog_effect(&theme.motion))
            .exit(motion::dialog_exit(&theme.motion));
        cx.overlay(OverlaySpec {
            content,
            anchor: Anchor::Center,
            modal: true,
            dismiss: Some(self.dismiss),
        });
    }
}

/// A small tooltip bubble showing `text` (a dark rounded label). Show it via
/// `cx.overlay` anchored near the hovered element (non-modal, no dismiss).
/// It fades in when it shows and out when the view stops showing it.
pub fn tooltip<S>(cx: &Cx<S>, text: impl Into<String>) -> Element {
    let theme = cx.theme();
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
    // It fades in when it shows, and out when it goes.
    .enter(Effect::fade().transition(Transition(theme.motion.popup)))
    .exit(Effect::fade().transition(Transition(theme.motion.popup_exit)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{ElementKind, render_view};
    use crate::geometry::Size;

    #[test]
    fn panel_is_a_decorated_vertical_stack() {
        let theme = Theme::light();
        let cx = Cx::<()>::new(&theme);
        let p = panel(&cx).child(divider(&cx));
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
    fn children_follow_a_containers_own() {
        let theme = Theme::light();
        let cx = Cx::<()>::new(&theme);
        let mut r = row(vec![spacer()])
            .children(vec![divider(&cx), spacer()])
            .child(divider(&cx));
        assert_eq!(r.children_mut().map(|c| c.len()), Some(4));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "only a stack, a grid or layers take children")]
    fn a_leaf_takes_no_children() {
        let theme = Theme::light();
        let cx = Cx::<()>::new(&theme);
        let _ = label(&cx, "Hi").child(spacer());
    }

    #[test]
    fn a_secondary_button_paints_its_fill_and_border() {
        // The button's fill and border; the label needs a font.
        let theme = Theme::light();
        let cx = Cx::<()>::new(&theme);
        let scene = render_view(
            &Button::new("Edit").variant(Variant::Secondary).view(&cx),
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
    fn text_field_carries_caret_on_its_text_leaf() {
        let theme = Theme::light();
        let mut cx = Cx::<TextInput>::new(&theme);
        let buf = TextInput::from_text("hello");
        let field = text_field(&mut cx, &buf, |s| s);
        // The inner text leaf carries the caret byte index for the focus overlay;
        // the field itself carries focus + text-pointer handles.
        assert!(field.focus.is_some());
        assert!(field.text_pos.is_some());
        let ElementKind::Stack { children, .. } = &field.kind else {
            panic!("text_field should be a stack");
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
    fn text_field_treats_select_line_as_select_all() {
        struct St {
            b: TextInput,
        }
        let theme = Theme::light();
        let mut st = St {
            b: TextInput::from_text("a\nb"),
        };
        let mut cx = Cx::<St>::new(&theme);
        let field = text_field(&mut cx, &st.b, |s| &mut s.b);
        let mut handlers = cx.into_handlers();
        st.b.buffer_mut().place_caret(0);
        assert!(handlers.dispatch_key(field.focus.unwrap(), &KeyInput::SelectLine, &mut st));
        assert_eq!(st.b.buffer().selection(), Some((0, 3)));
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
        let mut b = EditBuffer::from_text("abc");
        b.set_direction(TextDirection::Rtl);
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

    #[test]
    fn placeholders_show_while_empty_and_reach_screen_readers() {
        use crate::core::{Role, accessibility_tree, layout};
        use crate::geometry::Rect;
        struct S {
            name: TextInput,
            pass: PasswordField,
        }
        let theme = Theme::light();
        let leaf = |el: &Element| -> (String, Color, bool) {
            let ElementKind::Stack { children, .. } = &el.kind else {
                panic!("a field is a stack")
            };
            match &children[0].kind {
                ElementKind::Text {
                    text, color, style, ..
                } => (text.clone(), *color, style.placeholder),
                _ => panic!("its first child is its text"),
            }
        };
        let fields = |s: &S| {
            let mut cx = Cx::<S>::new(&theme);
            let editor = text_field(&mut cx, &s.name, |s| &mut s.name).placeholder("Your name");
            let plain = text_area(&mut cx, &s.name, |s| &mut s.name).placeholder("Search");
            let pass = password_field(&mut cx, &s.pass, |s| &mut s.pass).placeholder("Password");
            (editor, plain, pass)
        };
        let mut s = S {
            name: TextInput::new(),
            pass: PasswordField::new(),
        };
        // Empty: the hints, muted.
        let (editor, plain, pass) = fields(&s);
        let muted = theme.palette.text_muted;
        assert_eq!(leaf(&editor), ("Your name".into(), muted, true));
        assert_eq!(leaf(&plain), ("Search".into(), muted, true));
        assert_eq!(leaf(&pass), ("Password".into(), muted, true));
        // A screen reader hears an empty field with a placeholder.
        let tree = accessibility_tree(
            &layout(
                &column(vec![editor, pass]),
                Rect::from_xywh(0.0, 0.0, 300.0, 200.0),
                None,
            ),
            None,
        );
        let nodes = tree.descendants();
        let field = nodes.iter().find(|n| n.role == Role::TextField).unwrap();
        assert_eq!(field.name, "");
        assert_eq!(field.placeholder.as_deref(), Some("Your name"));
        let secret = nodes
            .iter()
            .find(|n| n.role == Role::PasswordField)
            .unwrap();
        assert_eq!(secret.placeholder.as_deref(), Some("Password"));
        // Typed into, or composing: the value, and the hint stays for the
        // screen reader only.
        s.name.apply(&KeyInput::Text("Ada".into()));
        let (editor, _, _) = fields(&s);
        assert_eq!(leaf(&editor), ("Ada".into(), theme.palette.text, false));
        let tree = accessibility_tree(
            &layout(
                &column(vec![editor]),
                Rect::from_xywh(0.0, 0.0, 300.0, 40.0),
                None,
            ),
            None,
        );
        let nodes = tree.descendants();
        let field = nodes.iter().find(|n| n.role == Role::TextField).unwrap();
        assert_eq!(field.name, "Ada");
        assert_eq!(field.placeholder.as_deref(), Some("Your name"));
        let mut s = S {
            name: TextInput::new(),
            pass: PasswordField::new(),
        };
        s.name.apply(&KeyInput::Preedit {
            text: "ni".into(),
            cursor: None,
        });
        let (editor, _, _) = fields(&s);
        assert!(!leaf(&editor).2, "no hint over a composition");
    }
}
