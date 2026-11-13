//! Stipple's runtime core.
//!
//! Defines the declarative [`View`] trait, the [`Element`] IR that views build,
//! and the layout + paint passes ([`render_view`]) that turn a view into a
//! [`Scene`] ready for `stipple::render` to rasterize.
//!
//! # Pipeline
//!
//! `build → layout → paint`, with pointer events routed back through the laid-
//! out tree:
//! - a [`View`] (or an app build closure with a [`Cx`]) produces an [`Element`]
//!   tree, registering `on_tap` handlers in the context;
//! - [`layout`] turns it into a retained [`LayoutNode`] tree;
//! - [`paint`] draws that tree into a [`Scene`], and [`hit_test`] routes
//!   pointer taps to the registered [`Handlers`].
//!
//! Between frames, [`diff_trees`] reconciles the previously-presented
//! [`LayoutNode`] tree against the freshly built one to compute the changed
//! [`Damage`] region, so the platform re-presents only what moved rather than
//! the whole window.
//!
//! Still ahead (ROADMAP.md Phase 1+): fine-grained per-node state so a rebuild
//! can skip unchanged subtrees entirely, and richer gesture recognition.

#![forbid(unsafe_code)]

pub mod a11y;
mod canvas;
mod clipboard;
pub(crate) mod crossfade;
mod diff;
mod dnd;
mod element;
mod handle;
pub(crate) mod highlight;
mod ime;
pub(crate) mod interact;
/// Raw keys, keeping Tab, IME for custom elements and Option as Meta.
pub mod keys;
pub mod menu;
pub(crate) mod native_fade;
mod pointer;
mod pointer_events;
pub(crate) mod presence;
mod render;
pub mod runtime;
pub(crate) mod touch;
pub(crate) mod transition;

pub use a11y::{AccessNode, Role, Semantics, accessibility_tree};
pub use canvas::{CanvasId, CanvasPainter};
pub(crate) use canvas::{CanvasInvalidation, invalidated_rects};
pub(crate) use clipboard::{
    ClipboardRequest, mirror as clipboard_mirror, selection_text,
    take_requests as take_clipboard_requests, write_queued as clipboard_write_queued,
};
pub use clipboard::{
    clipboard_text, primary_text, request_clipboard, set_clipboard_text, set_primary_text,
    write_clipboard,
};
pub use crossfade::THEME_FADE;
pub(crate) use diff::coalesce;
pub use diff::{Damage, diff_trees};
pub(crate) use dnd::DropFades;
pub use dnd::{DROP_FADE, DropState};
pub use element::{Align, BoxStyle, Element, ElementKind, LayoutStyle, SizeOverride, TextStyle};
#[cfg(all(feature = "tokio", not(target_arch = "wasm32")))]
pub use handle::runtime;
pub(crate) use handle::{AlertInput, AlertView};
pub use handle::{AppHandle, TimerId};
pub use ime::{ime_state, paint_preedit, secure_focused};
pub use interact::{ElementKey, Interaction, Reactive};
pub use keys::{Handled, KeyConfig, RawKeyId};
pub use menu::{Menu, MenuBar, MenuItem, Shortcut, Tray};
pub(crate) use pointer::set_pointer_position;
pub use pointer::{PointerInfo, pointer, set_pointer_modifiers};
pub use pointer_events::{
    PinchEvent, PinchPhase, PointerButtons, PointerEvent, PointerEventKind, PointerId, WheelEvent,
};
pub(crate) use pointer_events::{
    PointerHandlers, WheelTarget, pointer_at, pointer_local, wheel_target_at,
};
pub use presence::Effect;
pub use render::caret_index_at;
pub(crate) use render::scroll_range;
pub use render::{
    apply_scroll, layout, layout_with_direction, mark_native_states, measure, paint, paint_focus,
    paint_hover,
};
pub use runtime::{
    ActionId, Anchor, ContextId, Cx, DragId, DragMotion, DragSourceId, DropId, FocusId, Handlers,
    KeyInput, LayoutNode, NodeContent, OverlaySpec, ScrollId, ScrollRequests, ScrollTargets,
    TextPosId, ViewportEvent, ViewportId, collect_focusables, collect_viewports, context_at,
    cursor_at, drag_at, drag_hit_at, drag_source_at, drop_at, find_action, find_drop, find_focus,
    find_scroll, find_text_pos, first_text, focus_at, hit_test, local_point, node_to_window,
    scroll_at, scroll_targets_at, text_pos_at, viewport_at,
};
pub use transition::{AnimProp, Transition};

// The font type lives in stipple::render; re-export so callers of the layout/paint
// passes have one import path for the active font.
pub use crate::render::Font;

// Re-export the layout axis and grid tracks so widget crates speak one vocabulary.
pub use crate::layout::{Axis, LayoutDirection, Track};

use crate::geometry::{Rect, Size};
use crate::render::Scene;
use crate::style::Theme;

/// A piece of UI, described declaratively as a function of theme.
///
/// Implementors return an [`Element`] tree. This is the static-composition
/// entry point; interactive UIs use an app build closure threaded with a
/// [`Cx`] (see the `stipple` umbrella crate's `App`) to register handlers.
pub trait View {
    /// Build this view's element tree under the given `theme`.
    fn build(&self, theme: &Theme) -> Element;
}

/// An [`Element`] is itself a (trivial) view.
impl View for Element {
    fn build(&self, _theme: &Theme) -> Element {
        self.clone()
    }
}

/// Build `view`, lay it out to fill `size` logical pixels, and paint it into a
/// fresh [`Scene`]. Text is rendered with `font` (pass `None` to skip text).
/// Interaction handles on the elements are ignored (use [`layout`] +
/// [`hit_test`] directly to route events).
pub fn render_view(view: &impl View, size: Size, theme: &Theme, font: Option<&Font>) -> Scene {
    let element = view.build(theme);
    let tree = layout_with_direction(
        &element,
        Rect::from_xywh(0.0, 0.0, size.width, size.height),
        font,
        theme.layout_direction,
    );
    let mut scene = Scene::new(size);
    paint(&tree, &mut scene, font);
    scene
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Color;

    #[test]
    fn render_view_paints_a_root_panel() {
        let root = Element::stack(Axis::Vertical, vec![])
            .fill(Color::rgb(10, 20, 30))
            .padding(crate::geometry::Insets::uniform(8.0));
        let scene = render_view(&root, Size::new(100.0, 100.0), &Theme::light(), None);
        assert_eq!(scene.len(), 1);
        assert_eq!(scene.logical_size(), Size::new(100.0, 100.0));
    }
}
