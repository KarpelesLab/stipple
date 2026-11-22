//! The element IR: the lowered, paint-ready description of a UI subtree.
//!
//! Declarative [`View`](crate::core::View)s build a tree of [`Element`]s; the layout
//! and paint passes (see [`crate::core::render`]) consume it. Keeping a concrete IR
//! between widgets and rendering is what lets the (future) reactive runtime
//! diff one tree against the next.

use crate::core::a11y::{Role, Semantics};
use crate::core::canvas::{CanvasId, CanvasPainter};
use crate::core::interact::{ElementKey, Interaction, Reactive, StylePatch};
use crate::core::presence::{Effect, PresenceSpec};
use crate::core::runtime::{
    ActionId, ContextId, Cx, DragId, DragMotion, DragSourceId, DropId, FocusId, KeyInput, ScrollId,
    TextPosId, ViewportId,
};
use crate::core::transition::{AnimProp, Transition};
use crate::geometry::{Insets, Transform};
use crate::layout::{Axis, LayoutDirection, Track};
use crate::platform::CursorIcon;
use crate::platform::native::NativeDeco;
use crate::render::{
    BackgroundImage, Color, Decoration, Font, FontWeight, ImageFit, ImageSource, RichText,
    TextDirection, TextOutline, TextShadow, UnderlineStyle,
};

/// Alignment of children along an axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    /// Cross-axis only: stretch children to fill the cross extent.
    Stretch,
}

/// Fixed-size overrides for an element. `None` means "size to content / fill".
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SizeOverride {
    pub width: Option<f64>,
    pub height: Option<f64>,
}

/// Layout properties common to every element.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LayoutStyle {
    pub padding: Insets,
    pub size: SizeOverride,
    /// Main-axis grow weight when this element is a flex child. `0.0` = fixed.
    pub grow: f64,
    /// Main-axis shrink weight when this element is a flex child and its
    /// siblings overflow: it gives up space in proportion to `shrink × natural
    /// size`, down to its minimum. `0.0` (the default) = never shrinks.
    pub shrink: f64,
    /// Lower bounds on the element's size (`None` = no bound). Honored when
    /// measuring, growing, shrinking, stretching and placing in a grid cell.
    pub min: SizeOverride,
    /// Upper bounds on the element's size (`None` = no bound). A `min` larger
    /// than the `max` wins, as in CSS.
    pub max: SizeOverride,
    /// Grid placement: how many columns and rows this element spans when it is
    /// a grid cell. `0` counts as `1`.
    pub column_span: u16,
    pub row_span: u16,
    /// This element's own cross-axis alignment in a stack, or vertical one in
    /// a grid cell, over its container's (CSS's `align-self`). Set via
    /// [`Element::align_self`].
    pub align_self: Option<Align>,
    /// This element's own horizontal alignment in a grid cell, over the
    /// grid's (CSS's `justify-self`). Set via [`Element::justify_self`].
    pub justify_self: Option<Align>,
    /// The width is a widget's default (a text field's 200), not the app's:
    /// a column stretching its children stretches this one too. Cleared by
    /// [`Element::width`].
    pub soft_width: bool,
    /// Width over height: the other side follows the one decided (CSS's
    /// `aspect-ratio`). Set via [`Element::aspect_ratio`].
    pub aspect: Option<f64>,
}

/// The painted decoration of an element's box: fill, corner radius, border.
/// Applies to any element — a leaf bar, a button, or a container panel.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct BoxStyle {
    pub fill: Option<Color>,
    pub radius: f64,
    /// Border as `(color, width)`.
    pub border: Option<(Color, f64)>,
}

impl BoxStyle {
    /// A box filled with `color`, square-cornered, without a border.
    pub fn filled(color: Color) -> Self {
        Self::default().fill(color)
    }

    /// Fill the box with `color`.
    pub fn fill(mut self, color: Color) -> Self {
        self.fill = Some(color);
        self
    }

    /// Round the corners by `radius` logical pixels.
    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = radius;
        self
    }

    /// Draw a border of `width` logical pixels in `color`.
    pub fn border(mut self, color: Color, width: f64) -> Self {
        self.border = Some((color, width));
        self
    }
}

/// How a text element's glyphs are drawn, beyond its size and color: its
/// font, weight and slant (each `None` inherits the app's font), and the
/// lines drawn along it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TextStyle {
    /// The font; `None` uses the app's [`App::font`](crate::App::font).
    pub font: Option<Font>,
    /// A weight overriding the font's.
    pub weight: Option<FontWeight>,
    /// A slant overriding the font's.
    pub italic: Option<bool>,
    /// Underline, strikethrough and overline.
    pub decoration: Decoration,
    /// An empty text field's value (shown as a lone space): the color an
    /// [`Element::placeholder`] is drawn in, in its place.
    pub empty_field: Option<Color>,
    /// This text is a field's placeholder, not its value.
    pub placeholder: bool,
    /// The runs of rich text ([`Element::rich_text`]), when they differ
    /// from one another.
    pub spans: Option<std::sync::Arc<[crate::render::rich::Span]>>,
    /// The objects the text flows around, sized and placed by its
    /// [`flow_text`](Element::flow_text) container when it's laid out.
    pub flow: Option<std::sync::Arc<crate::render::TextFlow>>,
    /// An outline around the glyphs ([`Element::text_outline`]).
    pub outline: Option<TextOutline>,
    /// A shadow under them ([`Element::text_shadow`]); boxed, as most text
    /// has none.
    pub shadow: Option<Box<TextShadow>>,
    /// The text can be selected and copied ([`Element::selectable`]).
    pub selectable: bool,
    /// The color its selection is highlighted in, set by the app while it
    /// has one.
    pub highlight: Option<Color>,
    /// Its line limit and ellipsis, when it has either (boxed: most text
    /// has neither).
    pub cut: Option<Box<TextCut>>,
}

/// How a text is cut short: [`Element::max_lines`] and
/// [`Element::ellipsis`].
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TextCut {
    /// At most this many lines are shown.
    pub max_lines: Option<usize>,
    /// Text cut short ends in an ellipsis.
    pub ellipsis: Option<crate::render::Ellipsis>,
    /// The whole text, as read aloud, when what is shown was cut short
    /// (set when it's laid out).
    pub full: Option<std::sync::Arc<str>>,
}

impl TextStyle {
    /// Its line limit.
    pub(crate) fn max_lines(&self) -> Option<usize> {
        self.cut.as_ref().and_then(|c| c.max_lines)
    }

    /// Its ellipsis.
    pub(crate) fn ellipsis(&self) -> Option<crate::render::Ellipsis> {
        self.cut.as_ref().and_then(|c| c.ellipsis)
    }

    /// The whole text, when what it shows was cut short.
    pub(crate) fn full(&self) -> Option<&str> {
        self.cut.as_ref().and_then(|c| c.full.as_deref())
    }
}

impl TextStyle {
    /// The font this style draws with, given the app's `base` font: its own
    /// font (else `base`) in its weight and slant. `None` without either.
    pub(crate) fn resolve(&self, base: Option<&Font>) -> Option<Font> {
        let font = self.font.as_ref().or(base)?;
        let mut style = font.style();
        if let Some(w) = self.weight {
            style.weight = w;
        }
        if let Some(i) = self.italic {
            style.italic = i;
        }
        Some(font.styled(style))
    }

    /// `text` laid out in this style with the app's `base` font (each rich
    /// run in its own look), `size` pixels by default; see
    /// [`Font::layout_with`](crate::render::Font). `None` without a font.
    pub(crate) fn layout<'a>(
        &self,
        base: Option<&Font>,
        text: &'a str,
        size: f64,
        direction: TextDirection,
        max_width: Option<f64>,
        ui: LayoutDirection,
    ) -> Option<crate::render::TextLayout<'a>> {
        let font = self.resolve(base)?;
        if let Some(flow) = &self.flow {
            let styling = match &self.spans {
                None => crate::render::text_styling(&font, size),
                Some(spans) => crate::render::rich::styling(spans, &font, size),
            };
            return Some(styling.layout_flow(text, direction, max_width, ui, Some(flow)));
        }
        Some(match &self.spans {
            None => font.layout_with(text, size, direction, max_width, ui),
            Some(spans) => crate::render::rich::styling(spans, &font, size)
                .layout(text, direction, max_width, ui),
        })
    }

    /// `text[range]` as copied and read aloud: rich text's images as their
    /// alternative text.
    pub(crate) fn plain(&self, text: &str, range: std::ops::Range<usize>) -> String {
        match &self.spans {
            None => text[range].to_string(),
            Some(spans) => crate::render::rich::plain_of(text, spans, range),
        }
    }

    /// How far its outline and shadow reach beyond its box.
    pub(crate) fn reach(&self) -> Option<crate::geometry::Insets> {
        let outline = self.outline.map_or(0.0, |o| o.width.max(0.0));
        let ring = Insets::uniform(outline + 1.0);
        match (self.shadow.as_deref(), outline > 0.0) {
            (Some(shadow), _) => {
                let s = shadow.reach(outline);
                Some(crate::geometry::Insets {
                    top: s.top.max(ring.top),
                    right: s.right.max(ring.right),
                    bottom: s.bottom.max(ring.bottom),
                    left: s.left.max(ring.left),
                })
            }
            (None, true) => Some(ring),
            (None, false) => None,
        }
    }
}

/// How the children of a text-flow container ([`Element::flow_text`]) sit
/// in its text: one [`FlowItem`](crate::render::FlowItem) per child after
/// the text, and the flow's options.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FlowBox {
    pub items: Vec<crate::render::FlowItem>,
    pub options: crate::render::FlowOptions,
}

/// What an element arranges; all kinds share [`Element::decoration`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ElementKind {
    /// A leaf with no children.
    Leaf,
    /// A single line of text. Sizes to the shaped run (via the active font);
    /// painted with [`Scene::fill_text`](crate::render::Scene::fill_text).
    Text {
        text: String,
        size: f64,
        color: Color,
        /// The paragraph direction for bidirectional text. Set via
        /// [`Element::direction`].
        direction: TextDirection,
        /// A byte range `[start, end)` of the text drawn underlined (a menu
        /// mnemonic). Set via [`Element::underline`].
        underline: Option<(usize, usize)>,
        /// Font, weight, slant and decoration lines. Set via
        /// [`Element::font`], [`Element::bold`], [`Element::decorate`] and
        /// friends.
        style: TextStyle,
    },
    /// A linear container that lays children along `axis`.
    Stack {
        axis: Axis,
        gap: f64,
        main_align: Align,
        cross_align: Align,
        children: Vec<Element>,
    },
    /// A two-dimensional container placing `children` into cells, in order,
    /// row by row. `columns` defines the column tracks; `rows` the first row
    /// tracks (rows past the list are [`Track::Auto`]). Each child sits in its
    /// cell per `align` (horizontal, vertical); `Stretch` fills the cell.
    Grid {
        columns: Vec<Track>,
        rows: Vec<Track>,
        column_gap: f64,
        row_gap: f64,
        align: (Align, Align),
        children: Vec<Element>,
    },
    /// Children laid over one another in the same box, the later ones drawn
    /// on top (and hit first): each is placed within the content box per
    /// `align` (horizontal, vertical), `Stretch` filling it. It measures as
    /// big as its biggest child. Built with [`Element::layers`].
    ///
    /// With `flow` (see [`Element::flow_text`]) the first child is a text
    /// that flows around the others, each placed as its
    /// [`FlowItem`](crate::render::FlowItem) says.
    Layers {
        align: (Align, Align),
        children: Vec<Element>,
        flow: Option<Box<FlowBox>>,
    },
    /// An embedded-content viewport: a leaf reserving a rectangle the compositor
    /// fills with externally-rendered pixels for `id`. Sizes to its
    /// [`width`](Element::width)/[`height`](Element::height) overrides (or grow),
    /// not to content.
    Viewport { id: ViewportId },
    /// A bitmap or SVG image, placed in the element's box per `fit`. Sizes to
    /// the image's natural size in logical pixels; a single
    /// [`width`](Element::width)/[`height`](Element::height) override scales
    /// the other axis to keep the aspect ratio.
    Image { source: ImageSource, fit: ImageFit },
    /// A custom-paint leaf: `paint` draws it (see [`Element::canvas`]).
    /// Sizes like a viewport, to its overrides or grow. `revision` repaints
    /// it whole when it changes between builds.
    Canvas {
        id: CanvasId,
        paint: CanvasPainter,
        revision: u64,
    },
}

/// A node in the element tree: layout properties, decoration, an optional
/// interaction handle, and a kind.
///
/// `PartialEq` enables the runtime's no-op repaint skip: if a rebuilt tree
/// equals the previous one, the cached frame is reused. Handler ids compare by
/// value and are assigned deterministically per build, so an unchanged view
/// produces an equal tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub(crate) layout: LayoutStyle,
    pub(crate) decoration: BoxStyle,
    /// An image drawn over the fill, under the content and border: a
    /// nine-slice frame or a picture. Set via [`Element::background_image`].
    pub(crate) background: Option<Box<BackgroundImage>>,
    /// Handler this element routes pointer taps to, if any. Set via
    /// [`Element::on_tap`]; resolved against the [`Cx`] handler table.
    pub(crate) action: Option<ActionId>,
    /// Focus + keyboard handle, if this element is focusable. Set via
    /// [`Element::on_key`].
    pub(crate) focus: Option<FocusId>,
    /// Drag handle, if this element responds to pointer drags. Set via
    /// [`Element::on_drag`].
    pub(crate) drag: Option<DragId>,
    /// Secondary-click (context) handle: this element opens a context menu on
    /// right-click. Set via [`Element::on_context`].
    pub(crate) context: Option<ContextId>,
    /// Drop-target handle: drags from other applications (and in-app drags)
    /// can be dropped on this element. Set via [`Element::on_drop`].
    pub(crate) drop: Option<DropId>,
    /// Drag-source handle: pressing on this element and moving starts an
    /// in-app drag. Set via [`Element::drag_source`].
    pub(crate) drag_source: Option<DragSourceId>,
    /// Pointer and wheel handle: raw pointer events and wheel scrolls over
    /// this element go to its handlers. Set via [`Element::on_pointer`] and
    /// [`Element::on_wheel`].
    pub(crate) pointer: Option<crate::core::PointerId>,
    /// Caret position as a byte index into this element's text, for an editable
    /// text leaf. `None` for non-editable text and non-text elements. Set via
    /// [`Element::caret`]; the focus overlay draws the caret bar there.
    pub(crate) caret: Option<usize>,
    /// Selected byte range `[start, end)` into this element's text, for an
    /// editable text leaf. `None` when there is no selection. Set via
    /// [`Element::selection`]; the focus overlay highlights it.
    pub(crate) selection: Option<(usize, usize)>,
    /// Byte range `[start, end)` of input-method composition (preedit) text
    /// shown inside this element's text, which the focus overlay underlines.
    /// Set via [`Element::preedit`].
    pub(crate) preedit: Option<(usize, usize)>,
    /// How this element takes the keyboard beyond [`on_key`](Element::on_key):
    /// a raw key handler, keeping Tab, input-method support and Option as
    /// Meta. Set via [`Element::on_raw_key`], [`Element::captures_tab`],
    /// [`Element::ime_input`], [`Element::ime_cursor_area`] and
    /// [`Element::option_as_meta`]. `None` for most elements.
    pub(crate) keyboard: Option<Box<crate::core::keys::KeyConfig>>,
    /// Text-pointer handle: pointer presses/drags on this element resolve to a
    /// byte index in its text. Set via [`Element::on_text_pos`].
    pub(crate) text_pos: Option<TextPosId>,
    /// When `true` on a text element, the text greedily word-wraps to the
    /// element's content width instead of laying out on one line. Set via
    /// [`Element::wrap`].
    pub(crate) wrap: bool,
    /// Scroll-container handle: when set, this element lays its children out at
    /// natural size (overflowing its bounds), clips them, and routes wheel events
    /// to the app's offset for this id. Set via [`Element::scrollable`].
    pub(crate) scroll: Option<ScrollId>,
    /// A vertical scroll container that also scrolls sideways, by this id
    /// (see [`scrollable_x`](Element::scrollable_x)).
    pub(crate) scroll_x: Option<ScrollId>,
    /// A container scrolling both ways keeps gutters for both its bars (see
    /// [`scrollable_both`](Element::scrollable_both)).
    pub(crate) scroll_gutters: bool,
    /// When `true`, children are clipped to this element's painted bounds. Set
    /// implicitly by [`Element::scrollable`] and via [`Element::clip`].
    pub(crate) clip: bool,
    /// When set, the OS draws this element's chrome instead of `decoration`,
    /// which remains the fallback. See [`crate::platform::native`].
    pub(crate) native: Option<NativeDeco>,
    /// The mouse cursor shown over this element. `None` picks one from its
    /// handlers: the I-beam over editable text, the pointing hand over taps
    /// and drags, else whatever its parent shows. Set via [`Element::cursor`].
    pub(crate) cursor: Option<CursorIcon>,
    /// Declared accessibility semantics: a role overriding the one inferred
    /// from the element's handlers (a combo box, a table row), a name for an
    /// element with no text (an icon button), and item state. Set via
    /// [`Element::role`], [`Element::a11y_label`], [`Element::selected`] and
    /// [`Element::expanded`].
    pub(crate) semantics: Option<Semantics>,
    /// The direction this element lays its children out in, or `None` to
    /// inherit its parent's (the root inherits the theme's
    /// [`layout_direction`](crate::style::Theme::layout_direction)). Set via
    /// [`Element::layout_direction`].
    pub(crate) layout_direction: Option<LayoutDirection>,
    /// How this element and its subtree are moved, scaled and rotated where
    /// they're drawn, after layout (it never affects layout). `None` draws
    /// them where they were laid out. Set via [`Element::transform`].
    pub(crate) transform: Option<Transform>,
    /// Group opacity of this element and its subtree, `0.0..=1.0`: they are
    /// drawn together, then blended at this opacity. Set via
    /// [`Element::opacity`].
    pub(crate) opacity: f32,
    /// Laid out as usual but kept off stage: not drawn, hit, focused or
    /// shown to assistive technologies. Set via [`Element::offstage`].
    pub(crate) offstage: bool,
    /// Pointer input passes through the element itself to what is under
    /// it; the elements inside it still take theirs. Set via
    /// [`Element::hit_through`].
    pub(crate) hit_through: bool,
    /// The part of a labeled control (a checkbox's box, a switch's track)
    /// that shows the control's hover and press, and that an OS-drawn
    /// control draws in the state of the tappable element around it.
    pub(crate) indicator: bool,
    /// Interaction-state styling and motion: the element's
    /// [`key`](Element::key), per-state overrides
    /// ([`hovered`](Element::hovered), [`drag_over`](Element::drag_over)…),
    /// [`disabled`](Element::disabled) and [`transition`](Element::transition)s.
    /// `None` for an element using none of them.
    pub(crate) reactive: Option<Box<Reactive>>,
    pub(crate) kind: ElementKind,
}

impl Element {
    /// A decorated leaf (background/button/divider).
    pub fn boxed(style: BoxStyle) -> Self {
        Self {
            layout: LayoutStyle::default(),
            decoration: style,
            background: None,
            action: None,
            focus: None,
            drag: None,
            context: None,
            drop: None,
            drag_source: None,
            pointer: None,
            caret: None,
            selection: None,
            preedit: None,
            keyboard: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            scroll_x: None,
            scroll_gutters: false,
            clip: false,
            cursor: None,
            semantics: None,
            layout_direction: None,
            transform: None,
            opacity: 1.0,
            offstage: false,
            hit_through: false,
            indicator: false,
            reactive: None,
            kind: ElementKind::Leaf,
        }
    }

    /// A single line of text in `color` at `size` logical pixels.
    pub fn text(text: impl Into<String>, size: f64, color: Color) -> Self {
        Self {
            layout: LayoutStyle::default(),
            decoration: BoxStyle::default(),
            background: None,
            action: None,
            focus: None,
            drag: None,
            context: None,
            drop: None,
            drag_source: None,
            pointer: None,
            caret: None,
            selection: None,
            preedit: None,
            keyboard: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            scroll_x: None,
            scroll_gutters: false,
            clip: false,
            cursor: None,
            semantics: None,
            layout_direction: None,
            transform: None,
            opacity: 1.0,
            offstage: false,
            hit_through: false,
            indicator: false,
            reactive: None,
            kind: ElementKind::Text {
                text: text.into(),
                size,
                color,
                direction: TextDirection::Auto,
                underline: None,
                style: TextStyle::default(),
            },
        }
    }

    /// Text made of runs in their own colors, weights, sizes and
    /// decorations, with images in the line ([`RichText`]), in `color` at
    /// `size` logical pixels where a run sets neither. A plain string makes
    /// the same element as [`text`](Self::text).
    ///
    /// ```
    /// use stipple::core::Element;
    /// use stipple::render::{Color, RichText};
    /// let gold = Color::rgb(0xd4, 0xa0, 0x17);
    /// let line = Element::rich_text(
    ///     RichText::new().push("Gold: ").colored("120", gold),
    ///     14.0,
    ///     Color::BLACK,
    /// );
    /// # let _ = line;
    /// ```
    pub fn rich_text(text: impl Into<RichText>, size: f64, color: Color) -> Self {
        let (text, spans) = text.into().into_parts();
        let mut el = Self::text(text, size, color);
        if let ElementKind::Text { style, .. } = &mut el.kind {
            style.spans = spans;
        }
        el
    }

    /// An embedded-content viewport leaf for `id`: a reserved rectangle the app
    /// composites externally-rendered content into. Give it a size via
    /// [`width`](Element::width)/[`height`](Element::height) (or
    /// [`grow`](Element::grow) inside a flex parent); add a
    /// [`border`](Element::border) to frame it. The app locates it by `id` to
    /// blit content over the placeholder, and routes pointer/keys landing
    /// inside it to that content.
    pub fn viewport(id: ViewportId) -> Self {
        Self {
            layout: LayoutStyle::default(),
            decoration: BoxStyle::default(),
            background: None,
            action: None,
            focus: None,
            drag: None,
            context: None,
            drop: None,
            drag_source: None,
            pointer: None,
            caret: None,
            selection: None,
            preedit: None,
            keyboard: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            scroll_x: None,
            scroll_gutters: false,
            clip: false,
            cursor: None,
            semantics: None,
            layout_direction: None,
            transform: None,
            opacity: 1.0,
            offstage: false,
            hit_through: false,
            indicator: false,
            reactive: None,
            kind: ElementKind::Viewport { id },
        }
    }

    /// An image leaf showing `source` (a bitmap or SVG behind an `Arc`), sized
    /// to its natural size and placed with [`ImageFit::Contain`] by default
    /// (see [`fit`](Element::fit)).
    pub fn image(source: impl Into<ImageSource>) -> Self {
        Self {
            layout: LayoutStyle::default(),
            decoration: BoxStyle::default(),
            background: None,
            action: None,
            focus: None,
            drag: None,
            context: None,
            drop: None,
            drag_source: None,
            pointer: None,
            caret: None,
            selection: None,
            preedit: None,
            keyboard: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            scroll_x: None,
            scroll_gutters: false,
            clip: false,
            cursor: None,
            semantics: None,
            layout_direction: None,
            transform: None,
            opacity: 1.0,
            offstage: false,
            hit_through: false,
            indicator: false,
            reactive: None,
            kind: ElementKind::Image {
                source: source.into(),
                fit: ImageFit::default(),
            },
        }
    }

    /// An undecorated container laying `children` along `axis`, from its
    /// start. Across, a row (`Horizontal`) centers its children, so a
    /// checkbox, a label and a button line up on their middles, and a column
    /// (`Vertical`) stretches the ones without a width to its own, as CSS
    /// flexbox does (a row in a column spans it, so a
    /// [`spacer`](crate::widgets::spacer) in it pushes). Change it with
    /// [`align`](Element::align), or for one child with
    /// [`align_self`](Element::align_self).
    pub fn stack(axis: Axis, children: Vec<Element>) -> Self {
        Self {
            layout: LayoutStyle::default(),
            decoration: BoxStyle::default(),
            background: None,
            action: None,
            focus: None,
            drag: None,
            context: None,
            drop: None,
            drag_source: None,
            pointer: None,
            caret: None,
            selection: None,
            preedit: None,
            keyboard: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            scroll_x: None,
            scroll_gutters: false,
            clip: false,
            cursor: None,
            semantics: None,
            layout_direction: None,
            transform: None,
            opacity: 1.0,
            offstage: false,
            hit_through: false,
            indicator: false,
            reactive: None,
            kind: ElementKind::Stack {
                axis,
                gap: 0.0,
                main_align: Align::Start,
                cross_align: match axis {
                    Axis::Horizontal => Align::Center,
                    Axis::Vertical => Align::Stretch,
                },
                children,
            },
        }
    }

    /// A grid laying `children` into cells, row by row, across the given column
    /// `columns`. Cells stretch unsized children by default; set row tracks with
    /// [`rows`](Element::rows), spacing with [`gap`](Element::gap) and
    /// [`row_gap`](Element::row_gap), cell alignment with
    /// [`align`](Element::align), and multi-cell children with
    /// [`span`](Element::span).
    pub fn grid(columns: Vec<Track>, children: Vec<Element>) -> Self {
        Self {
            kind: ElementKind::Grid {
                columns,
                rows: Vec::new(),
                column_gap: 0.0,
                row_gap: 0.0,
                align: (Align::Stretch, Align::Stretch),
                children,
            },
            ..Self::stack(Axis::Vertical, Vec::new())
        }
    }

    /// `children` stacked over one another in the same box, each later one
    /// drawn over (and hit before) the ones before it: a background behind
    /// content, a badge over an icon. Each child fills the box unless it's
    /// sized, then sits per [`align`](Element::align) (horizontal, vertical);
    /// the box is as big as the biggest child.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// let theme = Theme::light();
    /// let cx = &Cx::<()>::new(&theme);
    /// let card = Element::layers(vec![
    ///     Element::boxed(BoxStyle::filled(theme.palette.primary)),
    ///     label(cx, "Over the fill").padding(Insets::uniform(8.0)),
    /// ]);
    /// # let _ = card;
    /// ```
    pub fn layers(children: Vec<Element>) -> Self {
        Self {
            kind: ElementKind::Layers {
                align: (Align::Stretch, Align::Stretch),
                children,
                flow: None,
            },
            ..Self::stack(Axis::Vertical, Vec::new())
        }
    }

    /// A text that flows around other elements: `text` (a text element,
    /// such as a [`paragraph`](crate::widgets::paragraph) or
    /// [`Element::rich_text`]; it's made to wrap) fills the container's
    /// width line by line, around the children added with
    /// [`flow_child`](Self::flow_child): floated left or right at the line
    /// holding their anchor, or placed anywhere, each keeping text out of
    /// its [shape](crate::render::FlowShape) grown by its margin. The
    /// container is as tall as the text and the children.
    ///
    /// A child's shape is its box by default, rounded as its corners are
    /// rounded; an image can keep text out of its opaque pixels only
    /// ([`FlowShape::alpha`](crate::render::FlowShape::alpha)). The text
    /// stays one text element: it selects (with
    /// [`selectable`](Self::selectable)), copies and reads aloud in its
    /// logical order. See `docs/text-flow.md` in the repository.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// use stipple::render::{FlowItem, FlowShape};
    /// let theme = Theme::light();
    /// let cx = &Cx::<()>::new(&theme);
    /// let article = Element::flow_text(paragraph(cx, "Text flows around the card …"))
    ///     .flow_child(
    ///         FlowItem::float_right().margin(12.0),
    ///         button(cx, "Subscribe"),
    ///     )
    ///     .flow_child(
    ///         FlowItem::at(Point::new(120.0, 60.0)).shape(FlowShape::Ellipse),
    ///         Element::boxed(BoxStyle::filled(theme.palette.primary).radius(40.0))
    ///             .width(80.0)
    ///             .height(80.0),
    ///     );
    /// # let _ = article;
    /// ```
    pub fn flow_text(text: Element) -> Self {
        Self {
            kind: ElementKind::Layers {
                align: (Align::Start, Align::Start),
                children: vec![text.wrap()],
                flow: Some(Box::default()),
            },
            ..Self::stack(Axis::Vertical, Vec::new())
        }
    }

    /// Add `child` to this [`flow_text`](Self::flow_text) container, placed
    /// in its text as `item` says. On another element it does nothing.
    pub fn flow_child(mut self, item: crate::render::FlowItem, child: Element) -> Self {
        if let ElementKind::Layers {
            children,
            flow: Some(flow),
            ..
        } = &mut self.kind
        {
            flow.items.push(item);
            children.push(child);
        }
        self
    }

    /// Flow this [`flow_text`](Self::flow_text) container's text with
    /// `options` (the narrowest room it takes, the space between
    /// paragraphs).
    pub fn flow_options(mut self, options: crate::render::FlowOptions) -> Self {
        if let ElementKind::Layers {
            flow: Some(flow), ..
        } = &mut self.kind
        {
            flow.options = options;
        }
        self
    }

    /// Route pointer taps on this element to `handler`, which runs against the
    /// app state. Registers the handler in `cx` and stamps its [`ActionId`].
    ///
    /// A button made this way (or a checkbox, switch, tab, list item… by its
    /// [`role`](Element::role)) also takes keyboard focus: Tab reaches it and
    /// Enter or Space runs `handler`, as a tap would.
    ///
    /// ```
    /// # use stipple::core::{Element, BoxStyle, Cx};
    /// # use stipple::style::Theme;
    /// let theme = Theme::light();
    /// let mut cx = Cx::new(&theme);
    /// let button = Element::boxed(BoxStyle::default())
    ///     .width(80.0)
    ///     .height(32.0)
    ///     .on_tap(&mut cx, |count: &mut u32| *count += 1);
    /// # drop(button);
    /// assert_eq!(cx.into_handlers().len(), 1);
    /// ```
    pub fn on_tap<S>(mut self, cx: &mut Cx<'_, S>, handler: impl FnMut(&mut S) + 'static) -> Self {
        self.action = Some(cx.register(handler));
        self
    }

    /// Route secondary (right) clicks on this element, and long presses on a
    /// touch screen, to `handler`, which runs against the app state and
    /// receives the click position in logical pixels —
    /// typically used to open a context menu there via [`Cx::overlay`]. Registers
    /// the handler in `cx` and stamps its [`ContextId`].
    pub fn on_context<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&mut S, crate::geometry::Point) + 'static,
    ) -> Self {
        self.context = Some(cx.register_context(handler));
        self
    }

    /// Give this element a context menu, opened at the pointer by a
    /// right-click on it, by the Menu key or Shift+F10 while the pointer is
    /// over it, or on a touch screen by a long press (a finger held still on
    /// it for half a second). Picking an item runs its action against the app
    /// state. Stipple draws the menu as an overlay on every platform.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct Notes { pinned: bool }
    /// fn view(s: &Notes, cx: &mut Cx<Notes>) -> Element {
    ///     let theme = *cx.theme();
    ///     label(cx, "Right-click me").context_menu(
    ///         cx,
    ///         Menu::new("")
    ///             .item(MenuItem::new("Pin", |n: &mut Notes| n.pinned = true).enabled(!s.pinned))
    ///             .item(MenuItem::new("Unpin", |n: &mut Notes| n.pinned = false).enabled(s.pinned)),
    ///     )
    /// }
    /// # let theme = Theme::light();
    /// # let mut cx = Cx::new(&theme);
    /// # let _ = view(&Notes { pinned: false }, &mut cx);
    /// # assert!(!cx.into_handlers().is_empty());
    /// ```
    pub fn context_menu<S>(mut self, cx: &mut Cx<'_, S>, menu: crate::core::menu::Menu<S>) -> Self {
        self.context = Some(cx.register_context_menu(menu));
        self
    }

    /// Make this element a drop target for files and text dragged from other
    /// applications (and for in-app drags). `handler` runs against the app
    /// state with the dropped [`DropData`](crate::platform::DropData) and the
    /// drop position in logical pixels. While an acceptable drag hovers the
    /// element, the app highlights it; [`on_drop_hover`](Element::on_drop_hover)
    /// tells the state too. See `docs/drag-and-drop.md`.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct Files { names: Vec<String> }
    /// fn view(_: &Files, cx: &mut Cx<Files>) -> Element {
    ///     let theme = *cx.theme();
    ///     label(cx, "Drop files here").on_drop(cx, |s, data, _at| {
    ///         if let DropData::Files(paths) = data {
    ///             s.names.extend(paths.iter().map(|p| p.display().to_string()));
    ///         }
    ///     })
    /// }
    /// # let theme = Theme::light();
    /// # let mut cx = Cx::new(&theme);
    /// # let _ = view(&Files { names: vec![] }, &mut cx);
    /// ```
    pub fn on_drop<S>(
        self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&mut S, &crate::platform::DropData, crate::geometry::Point) + 'static,
    ) -> Self {
        self.on_drop_of(cx, crate::platform::DragOffer::ANY, handler)
    }

    /// Like [`on_drop`](Element::on_drop), but only for drags carrying a kind
    /// in `accepts` (say [`DragOffer::FILES`](crate::platform::DragOffer::FILES)):
    /// other drags over it are refused (the cursor says so, and the target's
    /// [`DropState::accepted`](crate::core::DropState::accepted) is `false`).
    pub fn on_drop_of<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        accepts: crate::platform::DragOffer,
        handler: impl FnMut(&mut S, &crate::platform::DropData, crate::geometry::Point) + 'static,
    ) -> Self {
        self.drop = Some(cx.register_drop(accepts, handler));
        self
    }

    /// Make this element the drop target registered as `id` with
    /// [`Cx::register_drop`] (to read its [`Cx::drop_state`] while building
    /// the element).
    pub fn drop_target(mut self, id: DropId) -> Self {
        self.drop = Some(id);
        self
    }

    /// Tell the state when an acceptable drag starts (`true`) and stops
    /// (`false`) hovering this drop target, to restyle it while it does. Call
    /// after [`on_drop`](Element::on_drop); without a drop target it does
    /// nothing.
    pub fn on_drop_hover<S>(
        self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&mut S, bool) + 'static,
    ) -> Self {
        if let Some(id) = self.drop {
            cx.set_drop_hover(id, handler);
        }
        self
    }

    /// Let this element be dragged inside the app: pressing on it and moving
    /// a few pixels starts a drag, `handler` supplies its payload (usually a
    /// [`DropData::Internal`](crate::platform::DropData::Internal) naming the
    /// item; `None` cancels), and releasing over a drop target that accepts it
    /// drops it there. A press that becomes a drag doesn't tap.
    ///
    /// A [`DropData::Files`](crate::platform::DropData::Files) or
    /// [`DropData::Text`](crate::platform::DropData::Text) payload can also
    /// leave the app: dragged out of the window, it becomes a drag other
    /// applications can drop (see `docs/drag-and-drop.md`). An `Internal` one stays in the app.
    pub fn drag_source<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&mut S) -> Option<crate::platform::DropData> + 'static,
    ) -> Self {
        self.drag_source = Some(cx.register_drag_source(handler));
        self
    }

    /// Tell the state when this element's in-app drag (see
    /// [`drag_source`](Element::drag_source); call after it) is released
    /// outside every window of the app: `handler` gets the payload and where
    /// on the desktop it was let go, in logical pixels (`None` where the
    /// system doesn't say where windows are: Wayland). A tab bar opens a
    /// window there for the tab, say. While such a drag is outside the app's
    /// windows, a picture of the element follows the pointer where the
    /// platform allows (X11, Windows).
    ///
    /// Over another window of the app, the drag goes on there: its drop
    /// targets hover and take it as they would in the window it started in.
    pub fn on_drop_outside<S>(
        self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&mut S, &crate::platform::DropData, Option<crate::geometry::Point>)
        + 'static,
    ) -> Self {
        if let Some(id) = self.drag_source {
            cx.set_drop_outside(id, handler);
        }
        self
    }

    /// Deliver raw pointer events over this element to `handler`: every
    /// button going [`Down`](crate::core::PointerEventKind::Down) and
    /// [`Up`](crate::core::PointerEventKind::Up) (middle and side buttons
    /// too), [`Move`](crate::core::PointerEventKind::Move)s, and a
    /// [`Leave`](crate::core::PointerEventKind::Leave) when the pointer moves
    /// off it or out of the window. Each [`PointerEvent`](crate::core::PointerEvent)
    /// carries the button, the buttons held, the position in the element's
    /// coordinates, the modifiers and the click count. A press captures the
    /// pointer: the element gets the moves and the release even outside it
    /// (and the window) until the last button is up. For custom widgets such
    /// as a terminal's mouse reporting and selection; see
    /// `docs/pointer-events.md`.
    ///
    /// The events come in addition to the app's own handling (a tap, focus
    /// moving to a focusable element under the pointer, a context menu),
    /// first, and to the top-most element with a pointer handler under the
    /// pointer only.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// use stipple::core::{PointerEvent, PointerEventKind};
    /// use stipple::platform::PointerButton;
    /// #[derive(Default)]
    /// struct Term { pasted: u32, selecting_words: bool }
    /// fn view(_: &Term, cx: &mut Cx<Term>) -> Element {
    ///     Element::boxed(BoxStyle::default()).on_pointer(cx, |ev: &PointerEvent, t: &mut Term| {
    ///         match (ev.kind, ev.button) {
    ///             (PointerEventKind::Down, Some(PointerButton::Middle)) => t.pasted += 1,
    ///             (PointerEventKind::Down, Some(PointerButton::Left)) => {
    ///                 t.selecting_words = ev.click_count == 2;
    ///             }
    ///             _ => {}
    ///         }
    ///     })
    /// }
    /// # let theme = Theme::light();
    /// # let mut cx = Cx::new(&theme);
    /// # let _ = view(&Term::default(), &mut cx);
    /// # assert!(!cx.into_handlers().is_empty());
    /// ```
    pub fn on_pointer<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&crate::core::PointerEvent, &mut S) + 'static,
    ) -> Self {
        self.pointer = Some(cx.register_pointer(self.pointer, handler));
        self
    }

    /// Deliver wheel and trackpad scrolls over this element to `handler`, in
    /// lines or pixels as the device reports them, horizontal ones too, with
    /// the gesture's phase (see [`WheelEvent`](crate::core::WheelEvent)).
    /// The element then takes the wheel instead of a scroll container it is
    /// in (a scroll container inside it still takes the wheel over itself).
    pub fn on_wheel<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&crate::core::WheelEvent, &mut S) + 'static,
    ) -> Self {
        self.pointer = Some(cx.register_wheel(self.pointer, handler));
        self
    }

    /// Deliver two-finger gestures on a touch screen over this element to
    /// `handler`: a pinch (scale), a two-finger pan (translation) and a turn
    /// (rotation), all in one [`PinchEvent`](crate::core::PinchEvent) per
    /// move, measured from where the gesture began, with the centroid to zoom
    /// about. It starts when a second finger lands while the first is still
    /// pressing (or scrolling a list) and their midpoint is over the element,
    /// and ends when either lifts; the first finger's press is cancelled, so
    /// it doesn't tap. Without a pinch handler a second finger is ignored.
    /// For a photo viewer, a map, a canvas; see `docs/pointer-events.md`.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// use stipple::core::{PinchEvent, PinchPhase};
    /// #[derive(Default)]
    /// struct Photo { zoom: f64, zoom_at_start: f64 }
    /// fn view(_: &Photo, cx: &mut Cx<Photo>) -> Element {
    ///     Element::boxed(BoxStyle::default()).on_pinch(cx, |ev: &PinchEvent, p: &mut Photo| {
    ///         if ev.phase == PinchPhase::Began {
    ///             p.zoom_at_start = p.zoom;
    ///         }
    ///         p.zoom = (p.zoom_at_start * ev.scale).clamp(0.5, 8.0);
    ///     })
    /// }
    /// # let theme = Theme::light();
    /// # let mut cx = Cx::new(&theme);
    /// # let _ = view(&Photo::default(), &mut cx);
    /// # assert!(!cx.into_handlers().is_empty());
    /// ```
    pub fn on_pinch<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&crate::core::PinchEvent, &mut S) + 'static,
    ) -> Self {
        self.pointer = Some(cx.register_pinch(self.pointer, handler));
        self
    }

    /// Make this element focusable and route keyboard input to `handler`.
    /// The handler receives a [`KeyInput`] (committed text or an editing key)
    /// while this element holds focus. Registers in `cx` and stamps the
    /// resulting [`FocusId`].
    pub fn on_key<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&mut S, &KeyInput) + 'static,
    ) -> Self {
        self.focus = Some(cx.register_key(handler));
        self
    }

    /// Make this element respond to pointer drags. The `handler` receives the
    /// pointer's fractional x position (0..=1) across the element on press and
    /// while dragging. Registers in `cx` and stamps the resulting [`DragId`].
    pub fn on_drag<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&mut S, f64) + 'static,
    ) -> Self {
        self.drag = Some(cx.register_drag(handler));
        self
    }

    /// Make this element respond to pointer drags, receiving the full
    /// [`DragMotion`] (pointer position, press origin, and the element's and its
    /// parent's bounds at the press) on press and while dragging, for drags
    /// that aren't a fraction across the element: a split pane's divider, a
    /// vertical slider. Shares [`DragId`]s with [`on_drag`](Element::on_drag).
    pub fn on_drag_motion<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&mut S, &DragMotion) + 'static,
    ) -> Self {
        self.drag = Some(cx.register_drag_motion(handler));
        self
    }

    /// Resolve pointer presses/drags on this element to a byte index in its
    /// text. The `handler` receives the resolved index and an `extend` flag
    /// (`false` = initial press / place caret, `true` = drag / extend
    /// selection). Registers in `cx` and stamps the resulting [`TextPosId`].
    pub fn on_text_pos<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&mut S, usize, bool) + 'static,
    ) -> Self {
        self.text_pos = Some(cx.register_text_pos(handler));
        self
    }

    /// [`on_text_pos`](Self::on_text_pos), the handler receiving the whole
    /// [`TextHit`](crate::core::TextHit): the caret index, the pointer in
    /// the text's box, the width the text is laid out in, and where each of
    /// its images is. For an editor that selects, resizes or moves the
    /// images in its text by the pointer.
    pub fn on_text_hit<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&mut S, &crate::core::TextHit, bool) + 'static,
    ) -> Self {
        self.text_pos = Some(cx.register_text_hit(handler));
        self
    }

    // --- decoration modifiers ---

    pub fn fill(mut self, color: Color) -> Self {
        self.decoration.fill = Some(color);
        self
    }

    pub fn radius(mut self, radius: f64) -> Self {
        self.decoration.radius = radius;
        self
    }

    pub fn border(mut self, color: Color, width: f64) -> Self {
        self.decoration.border = Some((color, width));
        self
    }

    /// Draw `background` behind the element's content: a
    /// [`NineSlice`](crate::render::NineSlice) frame stretched to its box (a
    /// skinned panel, button, dialogue box or bar), or a plain image placed
    /// per an [`ImageFit`] ([`BackgroundImage::image`]: a painted screen
    /// behind a menu). It is drawn over the [`fill`](Self::fill) and under
    /// the [`border`](Self::border), so a skinned element usually has
    /// neither.
    ///
    /// The padding grows, side by side, to at least the background's
    /// [`content_padding`](BackgroundImage::content_padding) (a nine-slice's
    /// scaled insets), so the content stays off the frame; a
    /// [`padding`](Self::padding) set afterwards wins. Per-state overrides
    /// ([`hovered`](Self::hovered), [`pressed`](Self::pressed)…) can swap
    /// it: a button skinned per state. A tappable element with a background
    /// image doesn't get the theme's hover highlight (a rectangle over a
    /// shaped frame): show the hover with a frame of its own.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use stipple::prelude::*;
    /// use stipple::render::{NineSlice, Pixmap};
    ///
    /// fn view(_: &(), cx: &mut Cx<()>, frame: &NineSlice, lit: &NineSlice) -> Element {
    ///     label(cx, "Start")
    ///         .background_image(frame)
    ///         .hovered(|e| e.background_image(lit))
    /// }
    /// # let px = Arc::new(Pixmap::new(PhysicalSize::new(9, 9)));
    /// # let frame = NineSlice::new(px.clone(), Insets::uniform(3.0));
    /// # let theme = Theme::light();
    /// # let _ = view(&(), &mut Cx::new(&theme), &frame, &frame);
    /// ```
    pub fn background_image(mut self, background: impl Into<BackgroundImage>) -> Self {
        let background = background.into();
        let need = background.content_padding();
        let p = &mut self.layout.padding;
        p.top = p.top.max(need.top);
        p.right = p.right.max(need.right);
        p.bottom = p.bottom.max(need.bottom);
        p.left = p.left.max(need.left);
        self.background = Some(Box::new(background));
        self
    }

    // --- layout modifiers ---

    pub fn padding(mut self, insets: Insets) -> Self {
        self.layout.padding = insets;
        self
    }

    pub fn width(mut self, w: f64) -> Self {
        self.layout.size.width = Some(w);
        self.layout.soft_width = false;
        self
    }

    /// A widget's default width `w`: its width unless a column stretches it
    /// (or the app sets one with [`width`](Element::width)), as an unsized
    /// CSS `<input>` stretches in a flex column.
    pub(crate) fn default_width(mut self, w: f64) -> Self {
        self.layout.size.width = Some(w);
        self.layout.soft_width = true;
        self
    }

    pub fn height(mut self, h: f64) -> Self {
        self.layout.size.height = Some(h);
        self
    }

    pub fn grow(mut self, grow: f64) -> Self {
        self.layout.grow = grow;
        self
    }

    /// Let this element shrink below its natural main-axis size, with weight
    /// `shrink`, when its flex siblings overflow (stopping at its minimum).
    pub fn shrink(mut self, shrink: f64) -> Self {
        self.layout.shrink = shrink;
        self
    }

    /// Keep this element's width over its height at `ratio` (`1.0` square,
    /// `16.0 / 9.0` widescreen), as CSS's `aspect-ratio` does: the height
    /// follows the width once that is decided, by a fixed
    /// [`width`](Element::width) or by the room it is given (a column's
    /// width, a grid cell's, a row's share when it [`grow`](Element::grow)s),
    /// so "as wide as there is, and square" needs no window size. With only a
    /// [`height`](Element::height) fixed, the width follows it instead; with
    /// both fixed the ratio has nothing to do. Min and max bounds still apply
    /// to each side. A ratio that isn't a positive finite number is ignored.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// fn view(_: &(), cx: &mut Cx<()>) -> Element {
    ///     column(vec![
    ///         // As wide as the column, and square.
    ///         Element::canvas(CanvasId(1), |_| {}).aspect_ratio(1.0),
    ///         label(cx, "Scan the code"),
    ///     ])
    /// }
    /// # let theme = Theme::light();
    /// # let _ = view(&(), &mut Cx::new(&theme));
    /// ```
    pub fn aspect_ratio(mut self, ratio: f64) -> Self {
        self.layout.aspect = (ratio.is_finite() && ratio > 0.0).then_some(ratio);
        self
    }

    /// Never narrower than `w`, even when shrunk, stretched or in a grid cell.
    pub fn min_width(mut self, w: f64) -> Self {
        self.layout.min.width = Some(w);
        self
    }

    /// Never wider than `w`, even when grown or stretched; content measures
    /// against at most `w` (so wrapped text wraps there).
    pub fn max_width(mut self, w: f64) -> Self {
        self.layout.max.width = Some(w);
        self
    }

    /// Never shorter than `h`.
    pub fn min_height(mut self, h: f64) -> Self {
        self.layout.min.height = Some(h);
        self
    }

    /// Never taller than `h`.
    pub fn max_height(mut self, h: f64) -> Self {
        self.layout.max.height = Some(h);
        self
    }

    /// As a grid cell, span `columns` columns and `rows` rows.
    pub fn span(mut self, columns: u16, rows: u16) -> Self {
        self.layout.column_span = columns;
        self.layout.row_span = rows;
        self
    }

    /// Set a grid's row tracks (rows past the list size to content). No effect
    /// on other elements.
    pub fn rows(mut self, tracks: Vec<Track>) -> Self {
        if let ElementKind::Grid { rows, .. } = &mut self.kind {
            *rows = tracks;
        }
        self
    }

    /// Set a grid's gap between rows only ([`gap`](Element::gap) sets both).
    pub fn row_gap(mut self, gap: f64) -> Self {
        if let ElementKind::Grid { row_gap, .. } = &mut self.kind {
            *row_gap = gap;
        }
        self
    }

    /// Set a grid's gap between columns only ([`gap`](Element::gap) sets both).
    pub fn column_gap(mut self, gap: f64) -> Self {
        if let ElementKind::Grid { column_gap, .. } = &mut self.kind {
            *column_gap = gap;
        }
        self
    }

    /// Mark this text element's caret position (a byte index into its text), so
    /// the focus overlay draws the caret bar there instead of at the end. No
    /// effect on non-text elements.
    pub fn caret(mut self, byte_index: usize) -> Self {
        self.caret = Some(byte_index);
        self
    }

    /// Mark a selected byte range `[start, end)` into this text element, so the
    /// focus overlay highlights it. An empty or reversed range is ignored.
    pub fn selection(mut self, range: Option<(usize, usize)>) -> Self {
        self.selection = range.filter(|(s, e)| e > s);
        self
    }

    /// Mark the byte range `[start, end)` of this text element that is
    /// input-method composition (preedit) text, so the focus overlay underlines
    /// it. An empty range is ignored.
    pub fn preedit(mut self, range: Option<(usize, usize)>) -> Self {
        self.preedit = range.filter(|(s, e)| e > s);
        self
    }

    /// Have the OS draw this element's chrome. The element's own decoration stays
    /// as the fallback for when the OS can't. See [`crate::platform::native`].
    pub fn native(mut self, deco: NativeDeco) -> Self {
        self.native = Some(deco);
        self
    }

    /// Give this (native) element a select's options, which its live OS
    /// control (a pop-up button) shows and picks from.
    pub(crate) fn native_choices(mut self, choices: crate::core::a11y::Choices) -> Self {
        self.semantics
            .get_or_insert_with(Semantics::default)
            .choices = Some(std::sync::Arc::new(choices));
        self
    }

    /// Leave keyboard focus where it is when this element is pressed: a
    /// toolbar button acting on a text editor (Bold, Insert image) keeps the
    /// editor focused, its caret and selection in place, as native toolbars
    /// do. Without it a press moves focus to what was pressed (or nowhere).
    pub fn keeps_focus(mut self) -> Self {
        self.semantics
            .get_or_insert_with(Semantics::default)
            .keep_focus = true;
        self
    }

    /// Show `icon` as the mouse cursor while the pointer is over this element
    /// (and over children that don't pick their own).
    pub fn cursor(mut self, icon: CursorIcon) -> Self {
        self.cursor = Some(icon);
        self
    }

    /// Report this element to assistive technologies as `role` (a list, a table
    /// row, a combo box…), instead of the role inferred from its handlers.
    pub fn role(mut self, role: Role) -> Self {
        self.semantics.get_or_insert_with(Semantics::default).role = Some(role);
        self
    }

    /// Give this element an accessible name, instead of the text it contains
    /// (for an icon-only button, or a container such as a notification).
    pub fn a11y_label(mut self, label: impl Into<String>) -> Self {
        self.semantics.get_or_insert_with(Semantics::default).label = Some(label.into());
        self
    }

    /// Give a text field ([`text_field`](crate::widgets::text_field),
    /// [`text_area`](crate::widgets::text_area),
    /// [`password_field`](crate::widgets::password_field)) a hint: `text`
    /// shown muted while the field is empty and nothing is being composed in
    /// it, and given to screen readers as the field's placeholder.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct Form { name: TextInput }
    /// let theme = Theme::light();
    /// let mut cx = Cx::<Form>::new(&theme);
    /// let form = Form { name: TextInput::new() };
    /// let field = text_field(&mut cx, &form.name, |f| &mut f.name)
    ///     .placeholder("Your name");
    /// # let _ = field;
    /// ```
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        if !text.is_empty() {
            self.show_placeholder(&text);
        }
        self.semantics
            .get_or_insert_with(Semantics::default)
            .placeholder = Some(text);
        self
    }

    /// Put `hint` in the place of the empty field value under this element,
    /// if there is one.
    fn show_placeholder(&mut self, hint: &str) -> bool {
        if let ElementKind::Text {
            text, color, style, ..
        } = &mut self.kind
        {
            let Some(muted) = style.empty_field else {
                return false;
            };
            *text = hint.to_string();
            *color = muted;
            style.placeholder = true;
            return true;
        }
        self.children_mut()
            .is_some_and(|children| children.iter_mut().any(|c| c.show_placeholder(hint)))
    }

    /// Mark this text as an empty field's value, where a
    /// [`placeholder`](Element::placeholder) goes, drawn in `muted`.
    pub(crate) fn empty_field(mut self, muted: Color) -> Self {
        if let ElementKind::Text { style, .. } = &mut self.kind {
            style.empty_field = Some(muted);
        }
        self
    }

    /// Name this element for other elements' accessibility relations: an
    /// element whose [`labelled_by`](Element::labelled_by) is `id` is
    /// labelled by this one. Ids are matched within a window's tree.
    pub fn a11y_id(mut self, id: impl Into<String>) -> Self {
        self.semantics.get_or_insert_with(Semantics::default).id = Some(id.into());
        self
    }

    /// Say this element (a text field, a checkbox…) is labelled by the
    /// element whose [`a11y_id`](Element::a11y_id) is `id`, usually the text
    /// beside it. A screen reader then reads the label with the control, as
    /// a `for` attribute does on the web (AT-SPI's labelled-by / label-for
    /// relations).
    pub fn labelled_by(mut self, id: impl Into<String>) -> Self {
        self.semantics
            .get_or_insert_with(Semantics::default)
            .labelled_by = Some(id.into());
        self
    }

    /// Say this element controls the one whose
    /// [`a11y_id`](Element::a11y_id) is `id`: a combo box its open list, a
    /// tab the panel it shows, a button the region it expands. Screen
    /// readers offer to jump there (AT-SPI's controller-for / controlled-by
    /// relations, UIA's `ControllerFor`, `AXLinkedUIElements`,
    /// `aria-controls`). [`select`](crate::widgets::select) and the tab
    /// strips with a [`panel`](crate::widgets::Tabs::panel) set it
    /// themselves.
    pub fn controls(mut self, id: impl Into<String>) -> Self {
        self.semantics
            .get_or_insert_with(Semantics::default)
            .controls = Some(id.into());
        self
    }

    /// Say this element (a [`Role::TreeItem`](crate::core::Role::TreeItem))
    /// is at depth `level` of its tree, 1 for the top level, as
    /// [`tree`](crate::widgets::tree) does for its rows. In a flat tree the
    /// item before it one level up is then its parent (AT-SPI's
    /// node-child-of / node-parent-of relations, UIA's `Level`,
    /// `AXDisclosureLevel`, `aria-level`).
    pub fn a11y_level(mut self, level: usize) -> Self {
        self.semantics.get_or_insert_with(Semantics::default).level = Some(level.max(1));
        self
    }

    /// Enable wrapping. On a text element the text word-wraps to the element's
    /// content width across as many lines as needed. On a stack (row or
    /// column), children that don't fit the main axis flow onto new lines, with
    /// the stack's gap between items and between lines; main-axis alignment and
    /// grow apply per line, and cross-axis alignment within each line.
    pub fn wrap(mut self) -> Self {
        self.wrap = true;
        self
    }

    /// Set the paragraph direction of this text, or of every text element
    /// inside this container. The default, [`TextDirection::Auto`], takes each
    /// paragraph's direction from its first strong character. Right-to-left
    /// paragraphs line up against the right edge of the text's box.
    /// See `docs/bidi.md` in the repository.
    pub fn direction(mut self, dir: TextDirection) -> Self {
        fn set(el: &mut Element, dir: TextDirection) {
            match &mut el.kind {
                ElementKind::Text { direction, .. } => *direction = dir,
                ElementKind::Stack { children, .. }
                | ElementKind::Grid { children, .. }
                | ElementKind::Layers { children, .. } => {
                    for c in children {
                        set(c, dir);
                    }
                }
                _ => {}
            }
        }
        set(&mut self, dir);
        self
    }

    /// Apply `f` to the [`TextStyle`] of this text element, or of every text
    /// element inside this container, except a labeled control's box: a
    /// checkbox struck through strikes its label, not its check mark.
    fn text_style(mut self, f: &dyn Fn(&mut TextStyle)) -> Self {
        fn set(el: &mut Element, f: &dyn Fn(&mut TextStyle)) {
            match &mut el.kind {
                ElementKind::Text { style, .. } => f(style),
                ElementKind::Stack { children, .. }
                | ElementKind::Grid { children, .. }
                | ElementKind::Layers { children, .. } => {
                    for c in children.iter_mut().filter(|c| !c.indicator) {
                        set(c, f);
                    }
                }
                _ => {}
            }
        }
        set(&mut self, f);
        self
    }

    /// Draw this text, or the text inside this container that has no font
    /// of its own, with `font` instead of the app's. `font`'s weight and slant
    /// apply unless [`weight`](Self::weight) or [`italic`](Self::italic)
    /// override them.
    pub fn font(self, font: Font) -> Self {
        self.text_style(&|s| {
            if s.font.is_none() {
                s.font = Some(font.clone());
            }
        })
    }

    /// Draw this text (or the text inside this container that sets no weight
    /// of its own) at `weight`: the family's face for it, or a synthesized
    /// bold when it has none.
    pub fn weight(self, weight: FontWeight) -> Self {
        self.text_style(&|s| {
            s.weight.get_or_insert(weight);
        })
    }

    /// [`weight`](Self::weight) bold.
    pub fn bold(self) -> Self {
        self.weight(FontWeight::BOLD)
    }

    /// Draw this text (or the text inside this container that sets no slant
    /// of its own) italic: the family's italic face, or a synthesized oblique.
    pub fn italic(self) -> Self {
        self.text_style(&|s| {
            s.italic.get_or_insert(true);
        })
    }

    /// Draw `decoration`'s lines (underline style, strikethrough, overline,
    /// their color) along this text, or the text inside this container that
    /// has none of its own. Positions and thickness come from the font's
    /// [metrics](crate::render::Font::metrics).
    pub fn decorate(self, decoration: Decoration) -> Self {
        self.text_style(&|s| {
            if s.decoration.is_none() {
                s.decoration = decoration;
            }
        })
    }

    /// Draw an outline around the glyphs of this text (or of the text inside
    /// this container that has none of its own), behind them: text over
    /// artwork stays readable.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// use stipple::render::TextOutline;
    /// let name = Element::text("Aria", 18.0, Color::WHITE)
    ///     .text_outline(TextOutline::new(2.0, Color::BLACK));
    /// # let _ = name;
    /// ```
    pub fn text_outline(self, outline: TextOutline) -> Self {
        self.text_style(&|s| {
            s.outline.get_or_insert(outline);
        })
    }

    /// Draw a shadow under the glyphs of this text (and its outline), or of
    /// the text inside this container that has none of its own.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// use stipple::geometry::Vec2;
    /// use stipple::render::TextShadow;
    /// let title = Element::text("Victory", 32.0, Color::WHITE)
    ///     .text_shadow(TextShadow::new(Vec2::new(2.0, 2.0), Color::rgba(0, 0, 0, 160)).blur(3.0));
    /// # let _ = title;
    /// ```
    pub fn text_shadow(self, shadow: TextShadow) -> Self {
        self.text_style(&|s| {
            s.shadow.get_or_insert(Box::new(shadow));
        })
    }

    /// Let the user select this text (or all the text inside this container)
    /// and copy it: drag across it, double-click a word, triple-click a
    /// line, Ctrl/Cmd+A and Shift+arrows once it has a selection, Ctrl/Cmd+C
    /// or the context menu's Copy; on a touch screen, a long press selects
    /// the word under the finger and opens Copy / Select All. The pointer is
    /// an I-beam over it. Rich text copies its images' alternative text.
    ///
    /// One selection exists in a window at a time: selecting elsewhere, or
    /// clicking outside it, clears it. See `docs/text-editing.md`.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// let theme = Theme::light();
    /// let cx = Cx::<()>::new(&theme);
    /// let code = paragraph(&cx, "4006381333931").selectable();
    /// # let _ = code;
    /// ```
    pub fn selectable(mut self) -> Self {
        fn set(el: &mut Element) {
            match &mut el.kind {
                ElementKind::Text { style, .. } => {
                    style.selectable = true;
                    // An identity across builds, which the selection follows.
                    el.reactive.get_or_insert_with(Default::default);
                    el.cursor.get_or_insert(CursorIcon::Text);
                }
                ElementKind::Stack { children, .. }
                | ElementKind::Grid { children, .. }
                | ElementKind::Layers { children, .. } => {
                    for c in children.iter_mut().filter(|c| !c.indicator) {
                        set(c);
                    }
                }
                _ => {}
            }
        }
        set(&mut self);
        self
    }

    /// Show at most `lines` lines of this text (or of the text inside this
    /// container that sets no limit of its own): the lines of a wrapped text
    /// ([`paragraph`](crate::widgets::paragraph), [`wrap`](Self::wrap)), or
    /// the paragraphs of a text that doesn't wrap. The text measures as tall
    /// as those lines. With [`ellipsis`](Self::ellipsis), the last one shown
    /// ends in "…" when there is more; without, the rest is just not shown.
    /// Screen readers still read the whole text.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// let theme = Theme::light();
    /// let cx = Cx::<()>::new(&theme);
    /// let summary = paragraph(&cx, "A long description that goes on and on…")
    ///     .max_lines(2)
    ///     .ellipsis(Ellipsis::End);
    /// # let _ = summary;
    /// ```
    pub fn max_lines(self, lines: usize) -> Self {
        self.text_style(&|s| {
            let cut = s.cut.get_or_insert_with(Box::default);
            cut.max_lines.get_or_insert(lines.max(1));
        })
    }

    /// Cut this text (or the text inside this container) where it would run
    /// past its box, or past its [`max_lines`](Self::max_lines), and put an
    /// ellipsis (…) in its place: at the end ([`Ellipsis::End`]), or in the
    /// middle of the last line shown ([`Ellipsis::Middle`], for paths and
    /// URLs). Each line too wide for the box is cut, so a single-line
    /// [`label`](crate::widgets::label) stays on its line. The element can
    /// then [`shrink`](Self::shrink) (weight 1 unless set) in a row that
    /// overflows, and measures no wider than the room it's given. Screen
    /// readers still read the whole text.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// let theme = Theme::light();
    /// let cx = Cx::<()>::new(&theme);
    /// let file = label(&cx, "/home/me/projects/stipple/docs/report-final.pdf")
    ///     .ellipsis(Ellipsis::Middle);
    /// let row = row(vec![file, label(&cx, "12 KB")]);
    /// # let _ = row;
    /// ```
    pub fn ellipsis(mut self, at: crate::render::Ellipsis) -> Self {
        if self.layout.shrink <= 0.0 {
            self.layout.shrink = 1.0;
        }
        self.text_style(&|s| {
            let cut = s.cut.get_or_insert_with(Box::default);
            cut.ellipsis.get_or_insert(at);
        })
    }

    /// Strike this text through ([`decorate`](Self::decorate)).
    pub fn strikethrough(self) -> Self {
        self.decorate(Decoration::NONE.strikethrough())
    }

    /// Underline all of this text in `style` ([`decorate`](Self::decorate));
    /// [`underline`](Self::underline) marks a byte range instead.
    pub fn underline_style(self, style: UnderlineStyle) -> Self {
        self.decorate(Decoration::NONE.underline(style))
    }

    /// Underline the byte range `[start, end)` of this text element, in its
    /// color (the drawn menus mark mnemonics this way). An empty range, or
    /// `None`, draws no underline; no effect on other elements.
    pub fn underline(mut self, range: Option<(usize, usize)>) -> Self {
        if let ElementKind::Text { underline, .. } = &mut self.kind {
            *underline = range.filter(|(s, e)| e > s);
        }
        self
    }

    /// Lay this element and everything inside it out in `dir`, instead of the
    /// direction inherited from its parent (ultimately the theme's
    /// [`layout_direction`](crate::style::Theme::layout_direction)).
    ///
    /// Right to left mirrors the subtree: rows run right to left, horizontal
    /// `Align::Start` is the right edge, grid columns count from the right,
    /// and padding's `left` inset is on the right. Text whose direction is
    /// [`TextDirection::Auto`] lines up against the right edge. Set
    /// [`LayoutDirection::Ltr`] on a part of a mirrored UI that must not flip
    /// (a media transport bar, a chart, a phone number).
    /// See `docs/bidi.md` in the repository.
    pub fn layout_direction(mut self, dir: LayoutDirection) -> Self {
        self.layout_direction = Some(dir);
        self
    }

    /// Draw this element and its subtree through `transform` — moved, scaled
    /// and rotated about an origin within the element's box (its center by
    /// default) — without changing layout: siblings stay where they were.
    /// Pointer input follows the transform, so a scaled-up button is pressed
    /// where it's drawn. Text scales and rotates with everything else.
    ///
    /// ```
    /// use stipple::core::Element;
    /// use stipple::geometry::Transform;
    /// use stipple::render::Color;
    /// let badge = Element::text("New", 12.0, Color::WHITE)
    ///     .transform(Transform::IDENTITY.rotate_degrees(-8.0).scale(1.2));
    /// ```
    ///
    /// See `docs/transforms.md` in the repository.
    pub fn transform(mut self, transform: Transform) -> Self {
        self.transform = (!transform.is_identity()).then_some(transform);
        self
    }

    /// Draw this element and its subtree at `opacity` (`0.0` invisible, `1.0`
    /// opaque) **as a group**: the subtree is drawn on its own first and then
    /// blended once, so where children overlap they don't show through each
    /// other. A fully transparent element still takes pointer input, like
    /// CSS's `opacity: 0`.
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = if opacity.is_nan() {
            1.0
        } else {
            opacity.clamp(0.0, 1.0)
        };
        self
    }

    /// Keep this element and its subtree off stage while `offstage` is true:
    /// built and laid out as usual (so its scroll containers keep their
    /// offsets, clamped against its real content, and the scroll containers,
    /// taps and fields after it keep the same ids from frame to frame), but
    /// not drawn, not hit by the pointer or the wheel, out of the Tab order
    /// and hidden from assistive technologies. Keyboard focus inside it is
    /// dropped. Its keyed elements stay present, so nothing in it plays an
    /// exit or enter effect as it goes off stage and back.
    ///
    /// This is how [`page_stack`](crate::widgets::page_stack) keeps every
    /// page of a navigation bar alive while one shows.
    pub fn offstage(mut self, offstage: bool) -> Self {
        self.offstage = offstage;
        self
    }

    /// Let pointer input through this element to what is under it, except
    /// where an element inside it takes it: a game's HUD laid over its
    /// canvas, where a click on the HUD's empty area (or on a health bar,
    /// a score) reaches the canvas and a click on a pause button presses
    /// the button.
    ///
    /// Inside a view, an element without handlers already lets presses
    /// through to the elements under it (in
    /// [`layers`](Element::layers), say); this also passes over the
    /// element's own tap, pointer, wheel and context handlers and its
    /// cursor. As an overlay's content (see
    /// [`Cx::overlay`](crate::core::Cx::overlay)), a non-modal overlay
    /// made this way takes only the presses its children take, and the
    /// rest reach the layers below instead of stopping at the overlay.
    pub fn hit_through(mut self) -> Self {
        self.hit_through = true;
        self
    }

    /// Make this the part of a labeled control that shows the control's
    /// hover and press (see the `indicator` field).
    pub(crate) fn indicator(mut self) -> Self {
        self.indicator = true;
        self
    }

    /// Clip this element's children to its painted bounds (e.g. for an overlay
    /// panel). [`scrollable`](Element::scrollable) sets this implicitly.
    pub fn clip(mut self) -> Self {
        self.clip = true;
        self
    }

    /// Make this element a scroll container for `id`: its children lay out at
    /// natural size (overflowing), are clipped to its bounds, and wheel events
    /// over it scroll the app's offset for `id`. A horizontal stack scrolls
    /// sideways, anything else vertically. Unless [`height`](Element::height)
    /// (or [`width`](Element::width), sideways) fixes the viewport's length,
    /// it fills its parent as [`widgets::scroll`](crate::widgets::scroll)
    /// describes.
    pub fn scrollable(mut self, id: ScrollId) -> Self {
        self.scroll = Some(id);
        self.clip = true;
        self
    }

    /// Make a vertical [`scrollable`](Element::scrollable) container scroll
    /// sideways too, by `id` (its own offset): content wider than the
    /// container (a child with a [`min_width`](Element::min_width) past it)
    /// moves left by that offset, a horizontal wheel or trackpad swipe (or
    /// Shift+wheel) over it scrolls it, and a second bar along its bottom
    /// shows it. The container's own width doesn't follow the wide
    /// content. For a log view's long lines.
    pub(crate) fn scrollable_x(mut self, id: ScrollId) -> Self {
        self.scroll_x = Some(id);
        self.clip = true;
        self
    }

    /// [`scrollable_x`](Element::scrollable_x), keeping a classic bar's
    /// gutter for each bar: content wider than the container (less the
    /// vertical bar's gutter) scrolls that much further sideways, and while
    /// it does the content scrolls a gutter further down, so neither bar
    /// covers its last column or row. A [`text_view`](crate::widgets::text_view)
    /// keeps its own.
    pub(crate) fn scrollable_both(mut self, id: ScrollId) -> Self {
        self.scroll_gutters = true;
        self.scrollable_x(id)
    }

    /// Mark this element selected (or not) for assistive technologies. Implies
    /// [`Role::ListItem`] if no role was declared.
    pub fn selected(mut self, selected: bool) -> Self {
        let sem = self.semantics.get_or_insert_with(Semantics::default);
        sem.role.get_or_insert(Role::ListItem);
        sem.selected = selected;
        self
    }

    /// Mark this expandable element expanded or collapsed for assistive
    /// technologies. Implies [`Role::TreeItem`] if no role was declared.
    pub fn expanded(mut self, expanded: bool) -> Self {
        let sem = self.semantics.get_or_insert_with(Semantics::default);
        sem.role.get_or_insert(Role::TreeItem);
        sem.expanded = Some(expanded);
        self
    }

    /// Mark this element checked (on) or not for assistive technologies.
    /// Implies [`Role::CheckBox`] if no role was declared.
    pub fn checked(mut self, checked: bool) -> Self {
        let sem = self.semantics.get_or_insert_with(Semantics::default);
        sem.role.get_or_insert(Role::CheckBox);
        sem.checked = Some(checked);
        self
    }

    /// How an image element places its image in its box. No effect on other
    /// elements.
    pub fn fit(mut self, mode: ImageFit) -> Self {
        if let ElementKind::Image { fit, .. } = &mut self.kind {
            *fit = mode;
        }
        self
    }

    /// Add `children` after this container's own: a stack's ([`row`],
    /// [`column`], [`panel`], [`scroll`], [`toolbar`]…), a grid's cells, or
    /// [`layers`](Element::layers). Building the frame first and its children
    /// in the call keeps `cx` free for them, so they can register handlers:
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
    ///
    /// A list made in the call works too: `.children(items.iter().map(|i|
    /// label(cx, i)).collect())`. Only containers hold children: on a text,
    /// an image or another leaf this is a bug, caught by a debug assertion
    /// (and in a release build the children are dropped).
    ///
    /// [`row`]: crate::widgets::row
    /// [`column`]: crate::widgets::column
    /// [`panel`]: crate::widgets::panel
    /// [`scroll`]: crate::widgets::scroll
    /// [`toolbar`]: crate::widgets::toolbar
    pub fn children(mut self, children: Vec<Element>) -> Self {
        let own = self.children_mut();
        debug_assert!(
            own.is_some(),
            "only a stack, a grid or layers take children"
        );
        if let Some(own) = own {
            own.extend(children);
        }
        self
    }

    /// Add `child` after this container's own children; see
    /// [`children`](Element::children).
    pub fn child(self, child: Element) -> Self {
        self.children(vec![child])
    }

    /// Space between a stack's children (and its wrapped lines), or between a
    /// grid's columns and rows.
    pub fn gap(mut self, gap: f64) -> Self {
        match &mut self.kind {
            ElementKind::Stack { gap: g, .. } => *g = gap,
            ElementKind::Grid {
                column_gap,
                row_gap,
                ..
            } => {
                *column_gap = gap;
                *row_gap = gap;
            }
            _ => {}
        }
        self
    }

    /// A stack's (main-axis, cross-axis) alignment of its children; for a grid,
    /// the (horizontal, vertical) alignment of each child within its cell; for
    /// [`layers`](Element::layers), of each sized child within the box.
    ///
    /// A stack starts as `(Start, Center)` for a row and `(Start, Stretch)`
    /// for a column (see [`stack`](Element::stack)). `Stretch` leaves a child
    /// with a fixed size on that axis alone: it sits at the start.
    pub fn align(mut self, main: Align, cross: Align) -> Self {
        match &mut self.kind {
            ElementKind::Stack {
                main_align,
                cross_align,
                ..
            } => {
                *main_align = main;
                *cross_align = cross;
            }
            ElementKind::Grid { align, .. } | ElementKind::Layers { align, .. } => {
                *align = (main, cross)
            }
            _ => {}
        }
        self
    }

    /// Align this element across its stack (vertically in a row,
    /// horizontally in a column) by `align`, whatever the stack's own
    /// [`align`](Element::align) says: a button that keeps its natural width
    /// in a column that stretches the others, say.
    ///
    /// In a [`grid`](Element::grid) cell it aligns the element vertically
    /// within the cell, over the grid's vertical alignment, as CSS's
    /// `align-self` does there; [`justify_self`](Element::justify_self) is
    /// the horizontal one. In [`layers`](Element::layers) it has no effect:
    /// their [`align`](Element::align) places every child.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// let theme = Theme::light();
    /// let cx = &Cx::<()>::new(&theme);
    /// let form = column(vec![
    ///     label(cx, "Name"),
    ///     button(cx, "Save").align_self(Align::End),
    /// ]);
    /// # let _ = form;
    /// ```
    pub fn align_self(mut self, align: Align) -> Self {
        self.layout.align_self = Some(align);
        self
    }

    /// Align this element horizontally within its [`grid`](Element::grid)
    /// cell by `align`, whatever the grid's own [`align`](Element::align)
    /// says (CSS's `justify-self`): a narrow cell's content at the end of a
    /// stretched column, say. As there, `Stretch` leaves an element with a
    /// fixed width alone, at the cell's start. Outside a grid it has no
    /// effect: a stack's main axis is laid out by its `align` and the
    /// children's [`grow`](Element::grow), and layers place every child per
    /// their `align`.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// let theme = Theme::light();
    /// let cx = &Cx::<()>::new(&theme);
    /// let form = Element::grid(
    ///     vec![Track::Auto, Track::fr(1.0)],
    ///     vec![
    ///         label(cx, "Total"),
    ///         label(cx, "42.00").justify_self(Align::End),
    ///     ],
    /// );
    /// # let _ = form;
    /// ```
    pub fn justify_self(mut self, align: Align) -> Self {
        self.layout.justify_self = Some(align);
        self
    }

    // --- interaction states and transitions ---

    /// The element's children, for a container ([`children`](Self::children)
    /// adds to them).
    pub(crate) fn children_mut(&mut self) -> Option<&mut Vec<Element>> {
        match &mut self.kind {
            ElementKind::Stack { children, .. }
            | ElementKind::Grid { children, .. }
            | ElementKind::Layers { children, .. } => Some(children),
            _ => None,
        }
    }

    pub(crate) fn reactive_mut(&mut self) -> &mut Reactive {
        self.reactive.get_or_insert_with(Default::default)
    }

    /// Identify this element by `key` across builds (a string, a number, an
    /// item id, or a tuple of them), for [`Cx::interaction`] to find it and
    /// for its states and transitions to follow it when it moves in the tree.
    /// Keys are window-wide: give each keyed element its own. An element
    /// without one is identified by its position below its nearest keyed
    /// ancestor, which suits elements that stay put.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// fn view(_: &(), cx: &mut Cx<()>) -> Element {
    ///     let theme = *cx.theme();
    ///     let hot = cx.interaction("save").hovered();
    ///     label(cx, if hot { "Save now" } else { "Save" }).key("save")
    /// }
    /// # let theme = Theme::light();
    /// # let mut cx = Cx::new(&theme);
    /// # let _ = view(&(), &mut cx);
    /// ```
    pub fn key(mut self, key: impl std::hash::Hash) -> Self {
        self.reactive_mut().key = Some(ElementKey::new(key));
        self
    }

    /// While the element is in every state of `states`, restyle it with
    /// `style`, which gets the element (without its children) and returns it
    /// restyled. What it changes among the fill, radius, border, padding,
    /// fixed width and height, text color, cursor, opacity and transform applies; anything else
    /// it does is ignored. Overrides apply in the order they're given, after
    /// the element's own style, so a later one wins. With a
    /// [`transition`](Element::transition) the change animates.
    ///
    /// The app rebuilds when one of these states changes for an element that
    /// has an override for it, and only then. [`hovered`](Element::hovered),
    /// [`drag_over`](Element::drag_over) and friends are shorthands.
    pub fn when(mut self, states: Interaction, style: impl FnOnce(Element) -> Element) -> Self {
        let children = self.children_mut().map(std::mem::take);
        let probe = self.clone();
        if let (Some(children), Some(slot)) = (children, self.children_mut()) {
            *slot = children;
        }
        let patch = StylePatch::diff(&probe, &style(probe.clone()));
        self.reactive_mut().overrides.push((states, patch));
        self
    }

    /// Restyle while the pointer is over the element. A tappable element
    /// styled this way doesn't get the theme's hover highlight on top.
    /// See [`when`](Element::when).
    pub fn hovered(self, style: impl FnOnce(Element) -> Element) -> Self {
        self.when(Interaction::HOVERED, style)
    }

    /// Restyle while the element is pressed (and the pointer is still over
    /// it). See [`when`](Element::when).
    pub fn pressed(self, style: impl FnOnce(Element) -> Element) -> Self {
        self.when(Interaction::PRESSED, style)
    }

    /// Restyle while the element, or one inside it, has keyboard focus. See
    /// [`when`](Element::when).
    pub fn focused(self, style: impl FnOnce(Element) -> Element) -> Self {
        self.when(Interaction::FOCUSED, style)
    }

    /// Restyle while the element has keyboard focus that came from the
    /// keyboard (where a focus ring belongs). See [`when`](Element::when).
    pub fn focus_visible(self, style: impl FnOnce(Element) -> Element) -> Self {
        self.when(Interaction::FOCUS_VISIBLE, style)
    }

    /// Restyle while the element is being dragged. See [`when`](Element::when).
    pub fn dragged(self, style: impl FnOnce(Element) -> Element) -> Self {
        self.when(Interaction::DRAGGED, style)
    }

    /// Restyle while a drag the element's drop target accepts is over it. The
    /// app then leaves the drag-over look to the element. See
    /// [`when`](Element::when) and `docs/drag-and-drop.md`.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// fn view(_: &(), cx: &mut Cx<()>) -> Element {
    ///     let theme = *cx.theme();
    ///     label(cx, "Drop files here")
    ///         .padding(Insets::uniform(16.0))
    ///         .fill(theme.palette.surface)
    ///         .drag_over(|e| e.padding(Insets::uniform(24.0)).fill(theme.palette.primary))
    ///         .drag_refused(|e| e.fill(theme.palette.danger))
    ///         .transition(Transition::spring(SpringParams::new(300.0, 0.6)))
    ///         .on_drop_of(cx, DragOffer::FILES, |_, _, _| {})
    /// }
    /// # let theme = Theme::light();
    /// # let mut cx = Cx::new(&theme);
    /// # let _ = view(&(), &mut cx);
    /// ```
    pub fn drag_over(self, style: impl FnOnce(Element) -> Element) -> Self {
        self.when(Interaction::DRAG_OVER, style)
    }

    /// Restyle while a drag the element's drop target refuses is over it. See
    /// [`drag_over`](Element::drag_over).
    pub fn drag_refused(self, style: impl FnOnce(Element) -> Element) -> Self {
        self.when(Interaction::DRAG_REFUSED, style)
    }

    /// Mark the element disabled, for overrides on
    /// [`Interaction::DISABLED`] to style it. It changes only the look: the
    /// element's handlers still run, so a disabled control leaves them off.
    pub fn disabled(mut self, disabled: bool) -> Self {
        if disabled || self.reactive.is_some() {
            self.reactive_mut().disabled = disabled;
        }
        self
    }

    /// Animate changes to the element's animatable properties ([`AnimProp`]:
    /// fill, border, radius, padding, fixed size, text color, opacity, transform) between builds
    /// with `transition`, instead of switching. Changes from a per-state
    /// override, from the view's own state, or both, all animate. The element
    /// is matched across builds by its [`key`](Element::key), or its place in
    /// the tree; the app keeps asking for frames until it comes to rest.
    /// See `docs/animation.md`.
    pub fn transition(mut self, transition: Transition) -> Self {
        self.reactive_mut().transitions.push((None, transition));
        self
    }

    /// Animate changes to one property with `transition`, overriding
    /// [`transition`](Element::transition) for it ([`Transition::instant`]
    /// switches it at once).
    pub fn transition_for(mut self, prop: AnimProp, transition: Transition) -> Self {
        self.reactive_mut()
            .transitions
            .push((Some(prop), transition));
        self
    }

    // --- enter, exit and layout animations ---

    fn presence_mut(&mut self) -> &mut PresenceSpec {
        self.reactive_mut()
            .presence
            .get_or_insert_with(Default::default)
    }

    /// When the element appears (it wasn't in the last frame), animate it in
    /// from `effect`'s look: [`Effect::fade`], [`Effect::slide`],
    /// [`Effect::scale`], a combination ([`Effect::and`]) or your own. Not on
    /// the window's first frame, which shows everything as it is. Give the
    /// element a [`key`](Element::key), so that it is recognized as the same
    /// one from frame to frame wherever it is.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// let theme = Theme::light();
    /// let cx = &Cx::<()>::new(&theme);
    /// let row = label(cx, "New message")
    ///     .key(42)
    ///     .enter(Effect::fade().and(Effect::slide(0.0, -12.0)))
    ///     .exit(Effect::fade().and(Effect::scale(0.9)));
    /// # let _ = row;
    /// ```
    pub fn enter(mut self, effect: Effect) -> Self {
        self.presence_mut().enter = Some(effect);
        self
    }

    /// When the element goes away (the view stops building it), keep drawing
    /// it, where it was, as it animates out to `effect`'s look; then drop it.
    /// Meanwhile it can't be clicked, focused or read by a screen reader, and
    /// takes no room in the layout. If the view builds it again before it's
    /// gone, it turns round and comes back from where it is. Key the element
    /// (see [`enter`](Element::enter)).
    pub fn exit(mut self, effect: Effect) -> Self {
        self.presence_mut().exit = Some(effect);
        self
    }

    /// [`enter`](Element::enter) and [`exit`](Element::exit) with the same
    /// effect.
    pub fn appear(self, effect: Effect) -> Self {
        self.enter(effect).exit(effect)
    }

    /// When the layout moves or resizes the element (a list reorders, a
    /// panel opens above it), animate it from where it was drawn to its new
    /// place with `transition`, instead of jumping. A size change scales the
    /// element (and its content) from its old size; use
    /// [`animate_position`](Element::animate_position) to only move it.
    /// Scrolling and resizing the window don't animate. Key the element (see
    /// [`enter`](Element::enter)).
    ///
    /// ```
    /// use stipple::prelude::*;
    /// let theme = Theme::light();
    /// let cx = &Cx::<()>::new(&theme);
    /// let items = [3, 1, 2];
    /// let list = column(
    ///     items
    ///         .iter()
    ///         .map(|id| {
    ///             label(cx, &format!("Item {id}"))
    ///                 .key(*id)
    ///                 .animate_position(Transition::spring(SpringParams::SMOOTH))
    ///         })
    ///         .collect(),
    /// );
    /// # let _ = list;
    /// ```
    pub fn animate_layout(mut self, transition: impl Into<Transition>) -> Self {
        let spec = self.presence_mut();
        spec.layout = Some(transition.into());
        spec.layout_size = true;
        self
    }

    /// Like [`animate_layout`](Element::animate_layout), but only the
    /// position animates: a new size applies at once.
    pub fn animate_position(mut self, transition: impl Into<Transition>) -> Self {
        let spec = self.presence_mut();
        spec.layout = Some(transition.into());
        spec.layout_size = false;
        self
    }
}
