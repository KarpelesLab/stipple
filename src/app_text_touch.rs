//! Editing text with a finger in the [`App`]: what a touch screen does in
//! text fields, text areas and the rich editor, the same on iOS, Android and
//! the web since all three hand the App the same touches.
//!
//! - A tap places the caret (on release, so a drag that scrolls the page
//!   doesn't move focus or the caret first), and shows the caret handle; a
//!   tap on the caret, or on its handle, shows or hides the edit menu.
//! - A double tap selects the word, a long press too (instead of the context
//!   menu a long press opens elsewhere), and moving the held finger then
//!   extends the selection, a word at a time at its far end. Either shows the
//!   edit menu.
//! - The selection's ends have draggable handles, drawn under the text by the
//!   App itself (a round knob on a stem, in the theme's primary color), with
//!   a touch target bigger than the knob. The finger's offset from the
//!   handle's line is kept while dragging, so the finger doesn't hide where
//!   the end goes: there is no magnifier.
//! - The edit menu offers Cut, Copy, Paste and Select All, as the field
//!   allows (no Cut or Copy in a password field). Where the platform has one
//!   ([`Window::has_edit_menu`](crate::platform::Window::has_edit_menu): iOS
//!   16's `UIEditMenuInteraction`, Android's floating `ActionMode`) the
//!   system shows it; elsewhere the App draws a row of buttons above the
//!   selection (or below it, when there's no room).
//!
//! The handles go away when the text changes (typing hides them), focus
//! moves, or a mouse presses; the menu with any touch elsewhere.

use std::cell::Cell;
use std::rc::Rc;

use crate::core::{
    ActionId, Anchor, Axis, Cx, Element, FocusId, KeyInput, LayoutNode, NodeContent, OverlaySpec,
    Role, TextPosId,
};
use crate::geometry::{Insets, Point, Rect, Vec2};
use crate::platform::{EditAction, EditMenu, Touch, TouchPhase, Window};
use crate::render::{Font, Scene};
use crate::style::Theme;
use crate::{App, Pane, press_layer};

/// The radius of a handle's knob (logical pixels).
const KNOB: f64 = 7.0;
/// How far around a knob's center a finger still takes the handle: a touch
/// target of 48 logical pixels.
const GRAB: f64 = 24.0;
/// The room kept between the edit menu and what it points at, and the
/// window's edges.
const GAP: f64 = 8.0;

/// Which end of the selection a handle moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HandleKind {
    /// The caret's handle, with nothing selected.
    Caret,
    /// The selection's start (in the text's order).
    Start,
    /// The selection's end.
    End,
}

/// A handle as drawn this frame, in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Handle {
    pub kind: HandleKind,
    /// The caret position it stands for (an index into the field's shown
    /// text).
    pub index: usize,
    /// The caret line's bottom at the position.
    pub tip: Point,
    /// The line's height.
    pub line: f64,
}

impl Handle {
    /// The knob's center.
    fn center(&self) -> Point {
        Point::new(self.tip.x, self.tip.y + KNOB + 1.0)
    }

    /// The knob.
    fn knob(&self) -> Rect {
        let c = self.center();
        Rect::from_xywh(c.x - KNOB, c.y - KNOB, 2.0 * KNOB, 2.0 * KNOB)
    }

    /// The stem from the top of the line to the knob.
    fn stem(&self) -> Rect {
        Rect::from_xywh(
            self.tip.x - 1.0,
            self.tip.y - self.line,
            2.0,
            self.line + 1.0,
        )
    }

    /// What is drawn, for damage.
    pub(crate) fn drawn(&self) -> Rect {
        let k = self.knob();
        let s = self.stem();
        Rect::from_points(
            Point::new(k.min_x().min(s.min_x()) - 1.0, s.min_y() - 1.0),
            Point::new(k.max_x().max(s.max_x()) + 1.0, k.max_y() + 1.0),
        )
    }

    /// The point in the text a finger at the knob's center stands for: the
    /// middle of the line, above it.
    pub(crate) fn aim(&self) -> Point {
        Point::new(self.tip.x, self.tip.y - self.line / 2.0)
    }
}

/// A handle being dragged.
#[derive(Clone, Copy, Debug)]
struct HandleDrag {
    kind: HandleKind,
    /// The field's text-position handler.
    id: TextPosId,
    /// The selection's other end, which stays.
    fixed: usize,
    /// From the finger to the point in the text it moves.
    offset: Vec2,
    /// Where the finger went down, and whether it has moved since.
    start: Point,
    moved: bool,
}

/// The handles shown in a field.
#[derive(Clone, Debug)]
struct Shown {
    focus: FocusId,
    /// The field's text when they were first drawn: once it changes (the
    /// user typed), they go.
    text: Option<String>,
}

/// What the focused field looks like this frame, for its handles and menu.
#[derive(Clone, Debug)]
struct Field {
    focus: FocusId,
    text: String,
    caret: usize,
    selection: Option<(usize, usize)>,
    secure: bool,
    /// The handles' positions: the caret's, or the selection's ends.
    handles: Vec<Handle>,
    /// What the edit menu points at: the selection, or the caret's line.
    target: Rect,
}

/// A window's touch-editing state (see the [module docs](self)).
#[derive(Debug, Default)]
pub(crate) struct TextTouch {
    /// A finger went down on a text field at this point: it places the caret
    /// when it lifts, unless it scrolls or long-presses first.
    tap: Option<Point>,
    /// The field whose handles show.
    shown: Option<Shown>,
    /// Whether the edit menu shows, and whether it did when the finger now
    /// down landed (which closed it).
    pub(crate) menu: bool,
    menu_at_press: bool,
    drag: Option<HandleDrag>,
    /// The finger that long-pressed, extending the selection from the word
    /// it selected as it moves (once it moved past the slop).
    extending: Option<(u64, Point, (usize, usize), bool)>,
    /// This frame's handles, and those on screen (for damage).
    pub(crate) handles: Vec<Handle>,
    painted: Vec<Rect>,
    /// What a tap on the drawn menu picked, for the App to apply.
    picked: Rc<Cell<Option<EditAction>>>,
    /// The drawn menu's first item's tap handler, the items it was built
    /// with, and where it is.
    bar_action: Option<ActionId>,
    bar_built: Option<Vec<EditAction>>,
    pub(crate) bar_rect: Option<Rect>,
    /// The menu wanted this frame, and the one the system shows.
    wanted: Option<EditMenu>,
    sent: Option<EditMenu>,
    /// Whether the window shows the system's menu (asked at the first frame).
    native: Option<bool>,
}

impl TextTouch {
    /// Forget the handles and the menu (a mouse pressed).
    fn reset(&mut self) {
        self.shown = None;
        self.menu = false;
        self.drag = None;
        self.extending = None;
    }

    /// Show the handles for the field focused as `focus`.
    fn show(&mut self, focus: Option<FocusId>) {
        self.shown = focus.map(|focus| Shown { focus, text: None });
    }

    /// Whether the menu is drawn by the App (not the system's).
    fn drawn_menu(&self) -> bool {
        self.native != Some(true)
    }

    /// The handle a finger at `p` takes, if any: the nearest one in reach.
    fn handle_at(&self, p: Point) -> Option<Handle> {
        self.shown.as_ref()?;
        self.handles
            .iter()
            .map(|h| {
                let c = h.center();
                (h, (c.x - p.x).hypot(c.y - p.y))
            })
            // Not on the text itself, which a tap places the caret in.
            .filter(|(h, d)| *d <= GRAB && p.y >= h.tip.y - h.line / 4.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| *h)
    }

    /// The edit menu's items the App draws, as overlay `cx` lays out above
    /// everything else: a row of buttons, placed over the selection once
    /// laid out (see [`Pane::stamp_text_touch`]). Taps on them set
    /// [`picked`](Self::picked).
    pub(crate) fn declare_menu<S: 'static>(&mut self, cx: &mut Cx<S>, theme: &Theme) {
        self.bar_action = None;
        let items = match self.wanted {
            Some(menu) if self.menu && self.drawn_menu() => menu.actions(),
            _ => Vec::new(),
        };
        if items.is_empty() {
            self.bar_built = None;
            self.bar_rect = None;
            return;
        }
        let p = &theme.palette;
        let mut buttons = Vec::with_capacity(items.len());
        for action in &items {
            let picked = self.picked.clone();
            let action = *action;
            let text = Element::rich_text(action.label(), theme.typography.body, p.text);
            let button = Element::stack(Axis::Horizontal, vec![text])
                .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
                .radius(theme.radius / 2.0)
                .role(Role::MenuItem)
                .a11y_label(action.label())
                .keeps_focus()
                .on_tap(cx, move |_: &mut S| picked.set(Some(action)));
            if self.bar_action.is_none() {
                self.bar_action = button.action;
            }
            buttons.push(button);
        }
        let bar = Element::stack(Axis::Horizontal, buttons)
            .fill(p.surface)
            .radius(theme.radius)
            .border(p.border, 1.0)
            .padding(Insets::uniform(theme.spacing.xs))
            .role(Role::Menu);
        let at = self.bar_rect.map_or(Point::new(0.0, 0.0), |r| r.origin);
        cx.overlay(OverlaySpec::new(bar, Anchor::At(at)));
        self.bar_built = Some(items);
    }

    /// Draw the handles over the frame.
    pub(crate) fn paint(&self, scene: &mut Scene, theme: &Theme) {
        let color = theme.palette.primary;
        for h in &self.handles {
            scene.fill_rect(h.stem(), color);
            scene.fill_round_rect(h.knob(), KNOB, color);
        }
    }

    /// Where the handles changed since the frame on screen (where they were
    /// and are), and remember them as on screen.
    pub(crate) fn take_damage(&mut self) -> Vec<Rect> {
        let now: Vec<Rect> = self.handles.iter().map(Handle::drawn).collect();
        if now == self.painted {
            return Vec::new();
        }
        let mut rects = std::mem::replace(&mut self.painted, now.clone());
        rects.extend(now);
        rects
    }
}

/// The text position handler under `p` in `tree` (in the layer a press there
/// reaches), if editable text is there.
pub(crate) fn field_at(tree: &LayoutNode, overlays: bool, p: Point) -> Option<TextPosId> {
    let layer = press_layer(tree, overlays, p).0;
    crate::core::text_pos_at(layer, p).map(|(id, _)| id)
}

/// The text position handler in `node`'s subtree.
fn text_pos_in(node: &LayoutNode) -> Option<TextPosId> {
    node.text_pos
        .or_else(|| node.children.iter().find_map(text_pos_in))
}

/// What the focused field `focus` in `tree` looks like (see [`Field`]), if
/// it is an editable field with a font to measure it.
fn field(tree: &LayoutNode, focus: FocusId, font: Option<&Font>) -> Option<Field> {
    let leaf = crate::core::entry_leaf(tree, focus)?;
    let (node, to_window, _) = crate::core::node_to_window(tree, &|n| n.focus == Some(focus))?;
    // A field whose text a press places the caret in.
    text_pos_in(node)?;
    let NodeContent::Text { text, .. } = &leaf.content else {
        return None;
    };
    let layout = crate::core::leaf_layout(leaf, font)?;
    let b = leaf.text_rect();
    let clip = to_window.map_rect(node.bounds);
    let len = text.len();
    let caret = leaf.caret.unwrap_or(len).min(len);
    let selection = leaf
        .selection
        .map(|(s, e)| (s.min(e).min(len), s.max(e).min(len)))
        .filter(|(s, e)| s < e);
    let at = |kind: HandleKind, index: usize| {
        let p = layout.caret_position(index);
        let line = layout.line_height(layout.line_of(index)).max(1.0);
        let top = to_window.apply(Point::new(b.min_x() + p.x, b.min_y() + p.y));
        let bottom = to_window.apply(Point::new(b.min_x() + p.x, b.min_y() + p.y + line));
        Handle {
            kind,
            index,
            tip: bottom,
            line: (bottom.y - top.y).abs().max(1.0),
        }
    };
    let visible = |h: &Handle| {
        h.tip.x >= clip.min_x() - 1.0
            && h.tip.x <= clip.max_x() + 1.0
            && h.tip.y - h.line / 2.0 >= clip.min_y()
            && h.tip.y - h.line / 2.0 <= clip.max_y()
    };
    let (handles, target) = match selection {
        Some((s, e)) => {
            let rects = layout.selection_rects(s, e);
            let mut target: Option<Rect> = None;
            for r in rects {
                let r = to_window.map_rect(r.translate(Vec2::new(b.min_x(), b.min_y())));
                target = Some(target.map_or(r, |t| t.union(r)));
            }
            let handles = [at(HandleKind::Start, s), at(HandleKind::End, e)];
            let target = target.unwrap_or_else(|| {
                Rect::from_points(
                    Point::new(handles[0].tip.x, handles[0].tip.y - handles[0].line),
                    handles[1].tip,
                )
            });
            (handles.to_vec(), target)
        }
        None => {
            let h = at(HandleKind::Caret, caret);
            let target = Rect::from_xywh(h.tip.x - 1.0, h.tip.y - h.line, 2.0, h.line);
            (vec![h], target)
        }
    };
    let target = target.intersection(clip).unwrap_or(target);
    let secure = crate::core::secure_focused(tree, Some(focus));
    Some(Field {
        focus,
        // An empty field shows a lone space to keep its height.
        text: if text == " " && caret == 0 && selection.is_none() {
            String::new()
        } else {
            text.clone()
        },
        caret,
        selection,
        secure,
        handles: handles.into_iter().filter(visible).collect(),
        target,
    })
}

impl<S: 'static> Pane<S> {
    /// The focused field as laid out in the tree on screen.
    fn touch_field(&self, font: Option<&Font>) -> Option<Field> {
        field(self.tree.as_ref()?, self.focused?, font)
    }

    /// After the frame's layout (scrolled, with the caret revealed): the
    /// handles and the menu where the focused field is now, the drawn menu
    /// moved over the selection, and a rebuild asked for when the drawn
    /// menu's items changed.
    pub(crate) fn stamp_text_touch(&mut self, tree: &mut LayoutNode, font: Option<&Font>) {
        self.measure_text_touch(tree, font);
        self.after_menu_change(tree);
    }

    /// Bring the handles and the wanted menu up to date with the tree on
    /// screen, so the next build draws the menu's items right away.
    pub(crate) fn refresh_text_touch(&mut self, font: Option<&Font>) {
        if let Some(tree) = self.tree.take() {
            self.measure_text_touch(&tree, font);
            self.tree = Some(tree);
        }
        self.dirty = true;
    }

    /// The handles and the wanted menu for the focused field in `tree`.
    fn measure_text_touch(&mut self, tree: &LayoutNode, font: Option<&Font>) {
        let t = &mut self.text_touch;
        let field = t
            .shown
            .as_ref()
            .and_then(|s| field(tree, s.focus, font))
            .filter(|f| Some(f.focus) == self.focused);
        let field = match (field, t.shown.as_mut()) {
            (Some(f), Some(shown)) => match &shown.text {
                Some(text) if *text != f.text => None,
                Some(_) => Some(f),
                None => {
                    shown.text = Some(f.text.clone());
                    Some(f)
                }
            },
            _ => None,
        };
        let Some(field) = field else {
            if t.shown.take().is_some() || t.menu {
                t.menu = false;
                t.extending = None;
            }
            t.handles.clear();
            t.wanted = None;
            return;
        };
        t.handles = if t.extending.is_some_and(|e| e.3) {
            // A long press extending the selection: its finger is the handle.
            Vec::new()
        } else {
            field.handles.clone()
        };
        t.wanted = t.menu.then(|| {
            let selected = field.selection.is_some() && !field.secure;
            EditMenu {
                rect: field.target,
                cut: selected,
                copy: selected,
                paste: true,
                select_all: !field.text.is_empty()
                    && field.selection != Some((0, field.text.len())),
            }
        });
    }

    /// Place the drawn menu over what it points at, and rebuild when its
    /// items changed.
    fn after_menu_change(&mut self, tree: &mut LayoutNode) {
        let t = &mut self.text_touch;
        if !t.drawn_menu() {
            return;
        }
        let wanted_items = t
            .wanted
            .filter(|_| t.menu)
            .map(|m| m.actions())
            .filter(|a| !a.is_empty());
        if wanted_items != t.bar_built {
            self.dirty = true;
            crate::platform::frame::request();
        }
        let (Some(action), Some(menu)) = (t.bar_action, t.wanted) else {
            return;
        };
        // The overlay holding the menu: a layer of the tree's root.
        let window = tree.bounds;
        let Some(layer) = tree
            .children
            .iter_mut()
            .find(|c| crate::core::find_action(c, action).is_some())
        else {
            return;
        };
        let size = layer.bounds.size;
        let area = self.layout_size();
        let below = self.text_touch.handles.iter().map(|h| h.knob().max_y());
        let below = below.fold(menu.rect.max_y(), f64::max) + GAP;
        let mut y = menu.rect.min_y() - GAP - size.height;
        if y < window.min_y() + GAP + self.info.safe_area.top {
            y = below;
        }
        let y = y.min(area.height - size.height - GAP).max(window.min_y());
        let x = (menu.rect.center().x - size.width / 2.0)
            .min(area.width - size.width - GAP)
            .max(GAP.min(area.width - size.width).max(0.0));
        let by = Vec2::new(x - layer.bounds.min_x(), y - layer.bounds.min_y());
        crate::core::translate(layer, by);
        self.text_touch.bar_rect = Some(layer.bounds);
    }

    /// Show the system's edit menu as wanted, or close it, where the window
    /// has one (asked once).
    pub(crate) fn sync_edit_menu(&mut self, window: &dyn Window) {
        let t = &mut self.text_touch;
        if t.native.is_none() {
            t.native = Some(window.has_edit_menu());
        }
        if t.native != Some(true) {
            return;
        }
        let wanted = t.wanted.filter(|_| t.menu);
        if wanted != t.sent {
            window.show_edit_menu(wanted.as_ref());
            t.sent = wanted;
        }
    }

    /// A press at `pos` (any pointer): a finger on a text field waits for
    /// its release to place the caret (see [`text_tap`](App::text_tap)); a
    /// finger on a handle takes it. A mouse press hides the handles. Returns
    /// whether the press was taken.
    pub(crate) fn text_touch_down(&mut self, pos: Point) -> bool {
        if self.touch.primary().is_none() {
            if self.text_touch.shown.is_some() {
                self.text_touch.reset();
                self.dirty = true;
            }
            return false;
        }
        if let Some(handle) = self.text_touch.handle_at(pos) {
            let Some(field) = self.text_touch.shown.as_ref().map(|s| s.focus) else {
                return false;
            };
            let Some(id) = self
                .tree
                .as_ref()
                .and_then(|t| crate::core::find_focus(t, field))
                .and_then(text_pos_in)
            else {
                return false;
            };
            let other = self
                .text_touch
                .handles
                .iter()
                .find(|h| h.kind != handle.kind)
                .map_or(handle.index, |h| h.index);
            let aim = handle.aim();
            self.text_touch.drag = Some(HandleDrag {
                kind: handle.kind,
                id,
                fixed: other,
                offset: Vec2::new(aim.x - pos.x, aim.y - pos.y),
                start: pos,
                moved: false,
            });
            self.text_touch.menu = false;
            // The finger is the handle's: it doesn't scroll or long-press.
            self.touch.yield_to_widget();
            self.dirty = true;
            return true;
        }
        let Some(tree) = self.tree.as_ref() else {
            return false;
        };
        if field_at(tree, self.overlay_active, pos).is_none() {
            return false;
        }
        self.text_touch.tap = Some(pos);
        true
    }

    /// The press waiting to tap a field became something else (a scroll, a
    /// long press).
    pub(crate) fn cancel_text_tap(&mut self) {
        self.text_touch.tap = None;
    }

    /// Move the dragged handle with the finger at `pos`. Returns whether a
    /// handle is being dragged.
    fn drag_handle(&mut self, state: &mut S, font: Option<&Font>, pos: Point) -> bool {
        let Some(mut drag) = self.text_touch.drag else {
            return false;
        };
        let slop = crate::core::touch::SLOP;
        if !drag.moved && (pos.x - drag.start.x).hypot(pos.y - drag.start.y) <= slop / 2.0 {
            return true;
        }
        drag.moved = true;
        self.text_touch.drag = Some(drag);
        let aim = Point::new(pos.x + drag.offset.dx, pos.y + drag.offset.dy);
        let Some(index) = self.index_at(drag.id, aim, font) else {
            return true;
        };
        let ran = match drag.kind {
            HandleKind::Caret => self
                .handlers
                .dispatch_text_pos(drag.id, index, false, state),
            // An empty selection isn't one: keep the last.
            _ if index == drag.fixed => false,
            _ => {
                self.handlers
                    .dispatch_text_pos(drag.id, drag.fixed, false, state);
                self.handlers.dispatch_text_pos(drag.id, index, true, state)
            }
        };
        if ran {
            self.dirty = true;
        }
        true
    }

    /// The caret index of text position handler `id`'s text nearest `pos`.
    fn index_at(&self, id: TextPosId, pos: Point, font: Option<&Font>) -> Option<usize> {
        let tree = self.tree.as_ref()?;
        let node = crate::core::find_text_pos(tree, id)?;
        let at = crate::core::local_point(tree, &|n| n.text_pos == Some(id), pos).unwrap_or(pos);
        crate::core::caret_index_at(node, at, font)
    }

    /// The finger on a handle lifted: show the menu over the selection, or
    /// (a tap on the caret's handle) show or hide it. Returns whether a
    /// handle was being dragged.
    fn release_handle(&mut self) -> bool {
        let Some(drag) = self.text_touch.drag.take() else {
            return false;
        };
        self.text_touch.menu = match drag.kind {
            HandleKind::Caret => !drag.moved && self.text_touch.wanted.is_none(),
            _ => true,
        };
        self.dirty = true;
        true
    }
}

impl<S: 'static> App<S> {
    /// Lay window `idx` out as the state now is and measure its handles and
    /// menu there, after a touch changed them.
    fn settle_text_touch(&mut self, idx: usize) {
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        let pane = &mut panes[idx];
        pane.ensure_tree(state, theme, font.get());
        pane.refresh_text_touch(font.get());
    }

    /// A touch reached window `idx`, before the gestures see it: a new
    /// finger closes the edit menu (unless it lands on the drawn one), and
    /// the finger that long-pressed a field extends the selection as it
    /// moves. Returns whether to repaint.
    pub(crate) fn text_touch_event(&mut self, idx: usize, touch: &Touch) -> bool {
        let pane = &mut self.panes[idx];
        let t = &mut pane.text_touch;
        match touch.phase {
            TouchPhase::Started if pane.touch.primary().is_none() => {
                let on_menu = t.bar_rect.is_some_and(|r| r.contains(touch.position));
                t.menu_at_press = t.menu;
                if t.menu && !on_menu && t.handle_at(touch.position).is_none() {
                    t.menu = false;
                    pane.dirty = true;
                    return true;
                }
                false
            }
            TouchPhase::Moved => {
                let Some((id, start, word, moved)) = t.extending else {
                    return false;
                };
                if id != touch.id {
                    return false;
                }
                let p = touch.position;
                if !moved && (p.x - start.x).hypot(p.y - start.y) <= crate::core::touch::SLOP {
                    return false;
                }
                t.extending = Some((id, start, word, true));
                t.menu = false;
                let App {
                    state, font, panes, ..
                } = self;
                let pane = &mut panes[idx];
                let font = font.get();
                let Some(field) = pane.focused.and_then(|f| {
                    let tree = pane.tree.as_ref()?;
                    text_pos_in(crate::core::find_focus(tree, f)?)
                }) else {
                    return false;
                };
                let Some(index) = pane.index_at(field, p, font) else {
                    return false;
                };
                // The word stays selected; the selection grows from its far
                // end to the finger.
                let (a, b) = word;
                let (from, to) = if index >= b {
                    (a, index)
                } else if index <= a {
                    (b, index)
                } else {
                    (a, b)
                };
                pane.handlers.dispatch_text_pos(field, from, false, state);
                pane.handlers.dispatch_text_pos(field, to, true, state);
                pane.dirty = true;
                true
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if t.extending.is_some_and(|e| e.0 == touch.id) {
                    t.extending = None;
                    t.menu = true;
                    self.settle_text_touch(idx);
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// The finger moved to `pos`: drag the handle it holds. `None` when it
    /// holds none.
    pub(crate) fn text_touch_move(&mut self, idx: usize, pos: Point) -> Option<bool> {
        let App {
            state, font, panes, ..
        } = self;
        let pane = &mut panes[idx];
        pane.text_touch.drag?;
        let before = pane.dirty;
        pane.drag_handle(state, font.get(), pos);
        Some(pane.dirty && !before)
    }

    /// The finger lifted at `pos`: let go of a handle, or tap the field it
    /// pressed. `None` when it did neither.
    pub(crate) fn text_touch_up(&mut self, idx: usize, pos: Point) -> Option<bool> {
        if self.panes[idx].release_handle() {
            self.settle_text_touch(idx);
            return Some(true);
        }
        let at = self.panes[idx].text_touch.tap.take()?;
        let _ = pos;
        Some(self.text_tap(idx, at))
    }

    /// A tap on a text field at `pos` (window `idx`): focus it and place the
    /// caret, as a click does (a second tap soon after selects the word);
    /// show the caret's handle, and the edit menu for a word selected, or
    /// toggle it for a tap on the caret.
    fn text_tap(&mut self, idx: usize, pos: Point) -> bool {
        let font = self.font.get();
        let pane = &self.panes[idx];
        let before = pane.touch_field(font);
        let index = pane
            .tree
            .as_ref()
            .and_then(|t| field_at(t, pane.overlay_active, pos))
            .and_then(|id| pane.index_at(id, pos, font));
        let menu_was = pane.text_touch.menu_at_press;
        if !self.pane_text_press_at(idx, pos) {
            // Nothing to measure the text with: the tap still focuses it.
            return self.pane_click_at(idx, pos);
        }
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        let pane = &mut panes[idx];
        pane.end_text_select();
        pane.ensure_tree(state, theme, font.get());
        let after = pane.touch_field(font.get());
        // An empty field lays out a lone space (to keep its height), whose
        // far side is index 1: there, any tap is on the caret.
        let on_caret = matches!((&before, &after, index), (Some(b), Some(a), Some(i))
            if b.focus == a.focus && b.selection.is_none() && b.caret == i.min(b.text.len()));
        let selected = after.as_ref().is_some_and(|f| f.selection.is_some());
        pane.text_touch.show(pane.focused);
        pane.text_touch.menu = selected || (on_caret && !menu_was);
        pane.refresh_text_touch(font.get());
        true
    }

    /// A long press at `at` in window `idx`: on a text field, focus it,
    /// select the word there and show the edit menu (instead of a context
    /// menu); the finger then extends the selection. Returns whether a field
    /// was there.
    pub(crate) fn text_long_press(&mut self, idx: usize, at: Point) -> bool {
        let pane = &self.panes[idx];
        let on_field = pane
            .tree
            .as_ref()
            .and_then(|t| field_at(t, pane.overlay_active, at))
            .is_some();
        if !on_field || !self.pane_text_press_at(idx, at) {
            return false;
        }
        let finger = self.panes[idx].touch.primary();
        {
            let pane = &mut self.panes[idx];
            pane.end_text_select();
            // Not the first click of a double click.
            pane.clicks.reset();
        }
        self.pane_send_key(idx, KeyInput::SelectWord);
        let App {
            state,
            theme,
            font,
            panes,
            ..
        } = self;
        let pane = &mut panes[idx];
        pane.ensure_tree(state, theme, font.get());
        let field = pane.touch_field(font.get());
        let word = field
            .as_ref()
            .map(|f| f.selection.unwrap_or((f.caret, f.caret)))
            .unwrap_or_default();
        pane.text_touch.show(pane.focused);
        pane.text_touch.menu = true;
        pane.text_touch.extending = finger.map(|id| (id, at, word, false));
        pane.refresh_text_touch(font.get());
        true
    }

    /// Apply an item of the edit menu to the focused text field in the
    /// primary window, as picking it from the menu does: Cut and Copy put
    /// the selection on the clipboard, Paste inserts the clipboard's text,
    /// Select All selects the whole text. Headless, the clipboard is the
    /// in-process one. Returns whether anything changed.
    pub fn edit_action(&mut self, action: EditAction) -> bool {
        self.pane_edit_action(0, action, None)
    }

    /// The edit menu the primary window shows (the system's or its own),
    /// if any: what it points at and its items.
    pub fn edit_menu(&self) -> Option<EditMenu> {
        let t = &self.panes[0].text_touch;
        t.wanted.filter(|_| t.menu)
    }

    /// Apply `action` to the field the edit menu is for, in window `idx`,
    /// going through `window`'s clipboard where there is one.
    pub(crate) fn pane_edit_action(
        &mut self,
        idx: usize,
        action: EditAction,
        window: Option<&dyn Window>,
    ) -> bool {
        use crate::platform::clipboard::{ClipboardContent, ClipboardKind};
        let pane = &mut self.panes[idx];
        // A tap on the drawn menu may have moved focus: the menu's field
        // takes the action.
        if let Some(focus) = pane.text_touch.shown.as_ref().map(|s| s.focus) {
            pane.focused = Some(focus);
        }
        pane.text_touch.menu = action == EditAction::SelectAll;
        pane.dirty = true;
        let input = match action {
            EditAction::Cut => KeyInput::Cut,
            EditAction::Copy => KeyInput::Copy,
            EditAction::Paste => KeyInput::Paste,
            EditAction::SelectAll => KeyInput::SelectAll,
        };
        if input == KeyInput::Paste
            && let Some(w) = window
            && !self.clipboard_before_paste(idx, w)
        {
            // The text comes later (`Event::Clipboard`).
            return true;
        }
        self.pane_press_key(idx, input.clone());
        // The handles stay, for the text as it is now.
        let pane = &mut self.panes[idx];
        pane.text_touch.show(pane.focused);
        if matches!(input, KeyInput::Copy | KeyInput::Cut)
            && let Some(w) = window
            && !crate::core::clipboard_write_queued()
        {
            w.write_clipboard(
                ClipboardKind::Clipboard,
                &ClipboardContent::from_text(crate::core::clipboard_text()),
            );
        }
        self.settle_text_touch(idx);
        true
    }

    /// Apply what a tap on the drawn edit menu picked, if anything. Returns
    /// whether something was.
    pub(crate) fn run_edit_pick(&mut self, idx: usize, window: Option<&dyn Window>) -> bool {
        match self.panes[idx].text_touch.picked.take() {
            Some(action) => self.pane_edit_action(idx, action, window),
            None => false,
        }
    }
}
