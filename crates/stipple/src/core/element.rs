//! The element IR: the lowered, paint-ready description of a UI subtree.
//!
//! Declarative [`View`](crate::core::View)s build a tree of [`Element`]s; the layout
//! and paint passes (see [`crate::core::render`]) consume it. Keeping a concrete IR
//! between widgets and rendering is what lets the (future) reactive runtime
//! diff one tree against the next.

use crate::core::runtime::{
    ActionId, ContextId, Cx, DragId, FocusId, KeyInput, ScrollId, TextPosId, ViewportId,
};
use crate::geometry::Insets;
use crate::layout::Axis;
use crate::render::Color;

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
    },
    /// A linear container that lays children along `axis`.
    Stack {
        axis: Axis,
        gap: f64,
        main_align: Align,
        cross_align: Align,
        children: Vec<Element>,
    },
    /// An embedded-content viewport: a leaf reserving a rectangle the compositor
    /// fills with externally-rendered pixels for `id`. Sizes to its
    /// [`width`](Element::width)/[`height`](Element::height) overrides (or grow),
    /// not to content.
    Viewport { id: ViewportId },
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
            scroll: None,
            clip: false,
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
            scroll: None,
            clip: false,
            kind: ElementKind::Text {
                text: text.into(),
                size,
                color,
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
            scroll: None,
            clip: false,
            kind: ElementKind::Viewport { id },
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
            scroll: None,
            clip: false,
            kind: ElementKind::Stack {
                axis,
                gap: 0.0,
                main_align: Align::Start,
                cross_align: Align::Start,
                children,
            },
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

    /// Enable word-wrapping for a text element: the text wraps to the element's
    /// content width across as many lines as needed. No effect on non-text
    /// elements.
    pub fn wrap(mut self) -> Self {
        self.wrap = true;
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

    pub fn gap(mut self, gap: f64) -> Self {
        if let ElementKind::Stack { gap: g, .. } = &mut self.kind {
            *g = gap;
        }
        self
    }

    pub fn align(mut self, main: Align, cross: Align) -> Self {
        if let ElementKind::Stack {
            main_align,
            cross_align,
            ..
        } = &mut self.kind
        {
            *main_align = main;
            *cross_align = cross;
        }
        self
    }
}
