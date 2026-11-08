//! The element IR: the lowered, paint-ready description of a UI subtree.
//!
//! Declarative [`View`](crate::core::View)s build a tree of [`Element`]s; the layout
//! and paint passes (see [`crate::core::render`]) consume it. Keeping a concrete IR
//! between widgets and rendering is what lets the (future) reactive runtime
//! diff one tree against the next.

use crate::core::a11y::{Role, Semantics};
use crate::core::runtime::{
    ActionId, ContextId, Cx, DragId, DragMotion, FocusId, KeyInput, ScrollId, TextPosId, ViewportId,
};
use crate::geometry::Insets;
use crate::layout::{Axis, Track};
use crate::platform::CursorIcon;
use crate::platform::native::NativeDeco;
use crate::render::{Color, ImageFit, ImageSource, TextDirection};

/// Alignment of children along an axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
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
pub struct SizeOverride {
    pub width: Option<f64>,
    pub height: Option<f64>,
}

/// Layout properties common to every element.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutStyle {
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
}

/// The painted decoration of an element's box: fill, corner radius, border.
/// Applies to any element — a leaf bar, a button, or a container panel.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoxStyle {
    pub fill: Option<Color>,
    pub radius: f64,
    /// Border as `(color, width)`.
    pub border: Option<(Color, f64)>,
}

/// What an element arranges; all kinds share [`Element::decoration`].
#[derive(Clone, Debug, PartialEq)]
pub enum ElementKind {
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
    pub layout: LayoutStyle,
    pub decoration: BoxStyle,
    /// Handler this element routes pointer taps to, if any. Set via
    /// [`Element::on_tap`]; resolved against the [`Cx`] handler table.
    pub action: Option<ActionId>,
    /// Focus + keyboard handle, if this element is focusable. Set via
    /// [`Element::on_key`].
    pub focus: Option<FocusId>,
    /// Drag handle, if this element responds to pointer drags. Set via
    /// [`Element::on_drag`].
    pub drag: Option<DragId>,
    /// Secondary-click (context) handle: this element opens a context menu on
    /// right-click. Set via [`Element::on_context`].
    pub context: Option<ContextId>,
    /// Caret position as a byte index into this element's text, for an editable
    /// text leaf. `None` for non-editable text and non-text elements. Set via
    /// [`Element::caret`]; the focus overlay draws the caret bar there.
    pub caret: Option<usize>,
    /// Selected byte range `[start, end)` into this element's text, for an
    /// editable text leaf. `None` when there is no selection. Set via
    /// [`Element::selection`]; the focus overlay highlights it.
    pub selection: Option<(usize, usize)>,
    /// Text-pointer handle: pointer presses/drags on this element resolve to a
    /// byte index in its text. Set via [`Element::on_text_pos`].
    pub text_pos: Option<TextPosId>,
    /// When `true` on a text element, the text greedily word-wraps to the
    /// element's content width instead of laying out on one line. Set via
    /// [`Element::wrap`].
    pub wrap: bool,
    /// Scroll-container handle: when set, this element lays its children out at
    /// natural size (overflowing its bounds), clips them, and routes wheel events
    /// to the app's offset for this id. Set via [`Element::scrollable`].
    pub scroll: Option<ScrollId>,
    /// When `true`, children are clipped to this element's painted bounds. Set
    /// implicitly by [`Element::scrollable`] and via [`Element::clip`].
    pub clip: bool,
    /// When set, the OS draws this element's chrome instead of `decoration`,
    /// which remains the fallback. See [`crate::platform::native`].
    pub native: Option<NativeDeco>,
    /// The mouse cursor shown over this element. `None` picks one from its
    /// handlers: the I-beam over editable text, the pointing hand over taps
    /// and drags, else whatever its parent shows. Set via [`Element::cursor`].
    pub cursor: Option<CursorIcon>,
    /// Declared accessibility semantics: a role overriding the one inferred
    /// from the element's handlers (a combo box, a table row), a name for an
    /// element with no text (an icon button), and item state. Set via
    /// [`Element::role`], [`Element::a11y_label`], [`Element::selected`] and
    /// [`Element::expanded`].
    pub semantics: Option<Semantics>,
    pub kind: ElementKind,
}

impl Element {
    /// A decorated leaf (background/button/divider).
    pub fn boxed(style: BoxStyle) -> Self {
        Self {
            layout: LayoutStyle::default(),
            decoration: style,
            action: None,
            focus: None,
            drag: None,
            context: None,
            caret: None,
            selection: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            clip: false,
            cursor: None,
            semantics: None,
            kind: ElementKind::Leaf,
        }
    }

    /// A single line of text in `color` at `size` logical pixels.
    pub fn text(text: impl Into<String>, size: f64, color: Color) -> Self {
        Self {
            layout: LayoutStyle::default(),
            decoration: BoxStyle::default(),
            action: None,
            focus: None,
            drag: None,
            context: None,
            caret: None,
            selection: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            clip: false,
            cursor: None,
            semantics: None,
            kind: ElementKind::Text {
                text: text.into(),
                size,
                color,
                direction: TextDirection::Auto,
            },
        }
    }

    /// An embedded-content viewport leaf for `id`: a reserved rectangle the app
    /// composites externally-rendered content into. Give it a size via
    /// [`width`](Element::width)/[`height`](Element::height) (or
    /// [`grow`](Element::grow) inside a flex parent); add a
    /// [`border`](Element::border) to frame it. The app locates it by `id` with
    /// [`collect_viewports`](crate::core::collect_viewports) to blit content over the
    /// placeholder, and routes pointer/keys landing inside it to that content.
    pub fn viewport(id: ViewportId) -> Self {
        Self {
            layout: LayoutStyle::default(),
            decoration: BoxStyle::default(),
            action: None,
            focus: None,
            drag: None,
            context: None,
            caret: None,
            selection: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            clip: false,
            cursor: None,
            semantics: None,
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
            action: None,
            focus: None,
            drag: None,
            context: None,
            caret: None,
            selection: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            clip: false,
            cursor: None,
            semantics: None,
            kind: ElementKind::Image {
                source: source.into(),
                fit: ImageFit::default(),
            },
        }
    }

    /// An undecorated container laying `children` along `axis`.
    pub fn stack(axis: Axis, children: Vec<Element>) -> Self {
        Self {
            layout: LayoutStyle::default(),
            decoration: BoxStyle::default(),
            action: None,
            focus: None,
            drag: None,
            context: None,
            caret: None,
            selection: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            clip: false,
            cursor: None,
            semantics: None,
            kind: ElementKind::Stack {
                axis,
                gap: 0.0,
                main_align: Align::Start,
                cross_align: Align::Start,
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

    /// Route pointer taps on this element to `handler`, which runs against the
    /// app state. Registers the handler in `cx` and stamps its [`ActionId`].
    ///
    /// ```
    /// # use stipple::core::{Element, BoxStyle, runtime::Cx};
    /// # use stipple::style::Theme;
    /// let theme = Theme::light();
    /// let mut cx = Cx::new(&theme);
    /// let button = Element::boxed(BoxStyle::default())
    ///     .width(80.0)
    ///     .height(32.0)
    ///     .on_tap(&mut cx, |count: &mut i32| *count += 1);
    /// assert!(button.action.is_some());
    /// ```
    pub fn on_tap<S>(mut self, cx: &mut Cx<'_, S>, handler: impl FnMut(&mut S) + 'static) -> Self {
        self.action = Some(cx.register(handler));
        self
    }

    /// Route secondary (right) clicks on this element to `handler`, which runs
    /// against the app state and receives the click position in logical pixels —
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

    // --- layout modifiers ---

    pub fn padding(mut self, insets: Insets) -> Self {
        self.layout.padding = insets;
        self
    }

    pub fn width(mut self, w: f64) -> Self {
        self.layout.size.width = Some(w);
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

    /// Have the OS draw this element's chrome. The element's own decoration stays
    /// as the fallback for when the OS can't. See [`crate::platform::native`].
    pub fn native(mut self, deco: NativeDeco) -> Self {
        self.native = Some(deco);
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
                ElementKind::Stack { children, .. } | ElementKind::Grid { children, .. } => {
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

    /// Clip this element's children to its painted bounds (e.g. for an overlay
    /// panel). [`scrollable`](Element::scrollable) sets this implicitly.
    pub fn clip(mut self) -> Self {
        self.clip = true;
        self
    }

    /// Make this element a scroll container for `id`: its children lay out at
    /// natural size (overflowing), are clipped to its bounds, and wheel events
    /// over it scroll the app's offset for `id`. Give it a fixed height (e.g.
    /// `.height(..)`) to define the viewport.
    pub fn scrollable(mut self, id: ScrollId) -> Self {
        self.scroll = Some(id);
        self.clip = true;
        self
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

    /// How an image element places its image in its box. No effect on other
    /// elements.
    pub fn fit(mut self, mode: ImageFit) -> Self {
        if let ElementKind::Image { fit, .. } = &mut self.kind {
            *fit = mode;
        }
        self
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
    /// the (horizontal, vertical) alignment of each child within its cell.
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
            ElementKind::Grid { align, .. } => *align = (main, cross),
            _ => {}
        }
        self
    }
}
