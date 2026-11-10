//! The element IR: the lowered, paint-ready description of a UI subtree.
//!
//! Declarative [`View`](crate::core::View)s build a tree of [`Element`]s; the layout
//! and paint passes (see [`crate::core::render`]) consume it. Keeping a concrete IR
//! between widgets and rendering is what lets the (future) reactive runtime
//! diff one tree against the next.

use crate::core::a11y::{Role, Semantics};
use crate::core::interact::{ElementKey, Interaction, Reactive, StylePatch};
use crate::core::runtime::{
    ActionId, ContextId, Cx, DragId, DragMotion, DragSourceId, DropId, FocusId, KeyInput, ScrollId,
    TextPosId, ViewportId,
};
use crate::core::transition::{AnimProp, Transition};
use crate::geometry::{Insets, Transform};
use crate::layout::{Axis, LayoutDirection, Track};
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
        /// A byte range `[start, end)` of the text drawn underlined (a menu
        /// mnemonic). Set via [`Element::underline`].
        underline: Option<(usize, usize)>,
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
    /// Drop-target handle: drags from other applications (and in-app drags)
    /// can be dropped on this element. Set via [`Element::on_drop`].
    pub drop: Option<DropId>,
    /// Drag-source handle: pressing on this element and moving starts an
    /// in-app drag. Set via [`Element::drag_source`].
    pub drag_source: Option<DragSourceId>,
    /// Caret position as a byte index into this element's text, for an editable
    /// text leaf. `None` for non-editable text and non-text elements. Set via
    /// [`Element::caret`]; the focus overlay draws the caret bar there.
    pub caret: Option<usize>,
    /// Selected byte range `[start, end)` into this element's text, for an
    /// editable text leaf. `None` when there is no selection. Set via
    /// [`Element::selection`]; the focus overlay highlights it.
    pub selection: Option<(usize, usize)>,
    /// Byte range `[start, end)` of input-method composition (preedit) text
    /// shown inside this element's text, which the focus overlay underlines.
    /// Set via [`Element::preedit`].
    pub preedit: Option<(usize, usize)>,
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
    /// The direction this element lays its children out in, or `None` to
    /// inherit its parent's (the root inherits the theme's
    /// [`layout_direction`](crate::style::Theme::layout_direction)). Set via
    /// [`Element::layout_direction`].
    pub layout_direction: Option<LayoutDirection>,
    /// How this element and its subtree are moved, scaled and rotated where
    /// they're drawn, after layout (it never affects layout). `None` draws
    /// them where they were laid out. Set via [`Element::transform`].
    pub transform: Option<Transform>,
    /// Group opacity of this element and its subtree, `0.0..=1.0`: they are
    /// drawn together, then blended at this opacity. Set via
    /// [`Element::opacity`].
    pub opacity: f32,
    /// Interaction-state styling and motion: the element's
    /// [`key`](Element::key), per-state overrides
    /// ([`hovered`](Element::hovered), [`drag_over`](Element::drag_over)…),
    /// [`disabled`](Element::disabled) and [`transition`](Element::transition)s.
    /// `None` for an element using none of them.
    pub reactive: Option<Box<Reactive>>,
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
            drop: None,
            drag_source: None,
            caret: None,
            selection: None,
            preedit: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            clip: false,
            cursor: None,
            semantics: None,
            layout_direction: None,
            transform: None,
            opacity: 1.0,
            reactive: None,
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
            drop: None,
            drag_source: None,
            caret: None,
            selection: None,
            preedit: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            clip: false,
            cursor: None,
            semantics: None,
            layout_direction: None,
            transform: None,
            opacity: 1.0,
            reactive: None,
            kind: ElementKind::Text {
                text: text.into(),
                size,
                color,
                direction: TextDirection::Auto,
                underline: None,
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
            drop: None,
            drag_source: None,
            caret: None,
            selection: None,
            preedit: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            clip: false,
            cursor: None,
            semantics: None,
            layout_direction: None,
            transform: None,
            opacity: 1.0,
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
            action: None,
            focus: None,
            drag: None,
            context: None,
            drop: None,
            drag_source: None,
            caret: None,
            selection: None,
            preedit: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            clip: false,
            cursor: None,
            semantics: None,
            layout_direction: None,
            transform: None,
            opacity: 1.0,
            reactive: None,
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
            drop: None,
            drag_source: None,
            caret: None,
            selection: None,
            preedit: None,
            text_pos: None,
            wrap: false,
            native: None,
            scroll: None,
            clip: false,
            cursor: None,
            semantics: None,
            layout_direction: None,
            transform: None,
            opacity: 1.0,
            reactive: None,
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

    /// Give this element a context menu, opened at the pointer by a
    /// right-click on it, or by the Menu key or Shift+F10 while the pointer is
    /// over it. Picking an item runs its action against the app state. Stipple
    /// draws the menu as an overlay on every platform.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct Notes { pinned: bool }
    /// fn view(s: &Notes, cx: &mut Cx<Notes>) -> Element {
    ///     let theme = *cx.theme();
    ///     label(&theme, "Right-click me").context_menu(
    ///         cx,
    ///         Menu::new("")
    ///             .item(MenuItem::new("Pin", |n: &mut Notes| n.pinned = true).enabled(!s.pinned))
    ///             .item(MenuItem::new("Unpin", |n: &mut Notes| n.pinned = false).enabled(s.pinned)),
    ///     )
    /// }
    /// # let theme = Theme::light();
    /// # let mut cx = Cx::new(&theme);
    /// # assert!(view(&Notes { pinned: false }, &mut cx).context.is_some());
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
    ///     label(&theme, "Drop files here").on_drop(cx, |s: &mut Files, data, _at| {
    ///         if let DropData::Files(paths) = data {
    ///             s.names.extend(paths.iter().map(|p| p.display().to_string()));
    ///         }
    ///     })
    /// }
    /// # let theme = Theme::light();
    /// # let mut cx = Cx::new(&theme);
    /// # assert!(view(&Files { names: vec![] }, &mut cx).drop.is_some());
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
    pub fn drag_source<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&mut S) -> Option<crate::platform::DropData> + 'static,
    ) -> Self {
        self.drag_source = Some(cx.register_drag_source(handler));
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

    // --- interaction states and transitions ---

    /// The element's children, for a container.
    pub(crate) fn children_mut(&mut self) -> Option<&mut Vec<Element>> {
        match &mut self.kind {
            ElementKind::Stack { children, .. } | ElementKind::Grid { children, .. } => {
                Some(children)
            }
            _ => None,
        }
    }

    fn reactive_mut(&mut self) -> &mut Reactive {
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
    ///     label(&theme, if hot { "Save now" } else { "Save" }).key("save")
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
    ///     label(&theme, "Drop files here")
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
}
