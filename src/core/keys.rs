//! An element's keyboard settings beyond [`Element::on_key`]: a raw key
//! handler that sees every key before the toolkit maps it, keeping Tab,
//! input methods for elements that aren't text fields, and Option as Meta.
//! See `docs/keyboard.md`.

use crate::core::runtime::{Cx, FocusId, LayoutNode, find_focus};
use crate::core::{Element, KeyInput};
use crate::geometry::Rect;
use crate::platform::{ImeState, KeyEvent, OptionAsMeta};

/// Whether a raw key handler used the key. [`No`](Handled::No) lets it go on
/// to the toolkit (Tab focus traversal, the menu bar's keys, the editing
/// keys [`on_key`](Element::on_key) receives).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Handled {
    Yes,
    No,
}

impl From<bool> for Handled {
    fn from(used: bool) -> Handled {
        if used { Handled::Yes } else { Handled::No }
    }
}

/// An opaque handle to a raw key handler registered with
/// [`Cx::register_raw_key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RawKeyId(pub(crate) u32);

/// How an element takes the keyboard, beyond its [`on_key`](Element::on_key)
/// handler. Built with the `Element` methods below.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyConfig {
    /// The raw key handler.
    pub raw: Option<RawKeyId>,
    /// Tab and Shift+Tab stay with the element instead of moving focus.
    pub captures_tab: bool,
    /// The input method is on while the element has focus.
    pub ime: bool,
    /// Where the IME's candidate window goes, relative to the element's
    /// top-left (its whole box when `None`).
    pub ime_cursor: Option<Rect>,
    /// Which Option keys act as Meta on macOS while the element has focus.
    pub option_as_meta: OptionAsMeta,
}

impl Element {
    fn keyboard_mut(&mut self) -> &mut KeyConfig {
        self.keyboard.get_or_insert_with(Default::default)
    }

    /// Make this element focusable and give it every key first: `handler`
    /// gets each press (repeats included) and release as a
    /// [`KeyEvent`] — the logical and physical key, modifiers (left and
    /// right), the text it types, the repeat flag — **before** the toolkit
    /// maps keys to editing commands, moves focus with Tab or handles the
    /// menu bar's keys. Returning [`Handled::Yes`] stops it there;
    /// [`Handled::No`] lets the toolkit go on (so an unhandled printable key
    /// still arrives at [`on_key`](Element::on_key) as text).
    ///
    /// Menus come first: an open menu, and the menu bar's (or the native
    /// menu's) shortcuts, take their keys before this handler sees them, so
    /// Cmd+T still opens a tab in a terminal while unclaimed Ctrl
    /// combinations reach it. Keys an input method or a dead key sequence
    /// takes don't arrive here; their text comes through `on_key`. The order
    /// is in `docs/keyboard.md`.
    pub fn on_raw_key<S>(
        mut self,
        cx: &mut Cx<'_, S>,
        handler: impl FnMut(&mut S, &KeyEvent) -> Handled + 'static,
    ) -> Self {
        if self.focus.is_none() {
            self.focus = Some(cx.register_key(|_, _| {}));
        }
        let id = cx.register_raw_key(handler);
        self.keyboard_mut().raw = Some(id);
        self
    }

    /// Keep Tab and Shift+Tab: they don't move focus away from this element,
    /// but go to its [`on_raw_key`](Element::on_raw_key) handler, and if that
    /// doesn't handle them (or there is none), Tab reaches
    /// [`on_key`](Element::on_key) as `KeyInput::Text("\t")`.
    pub fn captures_tab(mut self, captures: bool) -> Self {
        self.keyboard_mut().captures_tab = captures;
        self
    }

    /// Switch the input method on while this element has focus, as it is for
    /// text fields: a terminal or a custom editor taking Chinese, Japanese or
    /// Korean input, or dead keys on macOS and Windows. Composition arrives
    /// at the [`on_key`](Element::on_key) handler as `KeyInput::Preedit` (to
    /// draw at the caret), committed text as `KeyInput::Text`. Place the
    /// candidate window with [`ime_cursor_area`](Element::ime_cursor_area).
    pub fn ime_input(mut self, on: bool) -> Self {
        self.keyboard_mut().ime = on;
        self
    }

    /// Where the caret is, relative to this element's top-left, for the
    /// input method's candidate window (a terminal's cursor cell). Implies
    /// [`ime_input`](Element::ime_input).
    pub fn ime_cursor_area(mut self, area: Rect) -> Self {
        let keyboard = self.keyboard_mut();
        keyboard.ime = true;
        keyboard.ime_cursor = Some(area);
        self
    }

    /// Which Option keys act as Meta on macOS while this element has focus:
    /// with one held, keys aren't composed by the layout (Option+B gives
    /// `b`, not `∫`, in [`KeyEvent::text`]) and skip the input method. No
    /// effect elsewhere.
    pub fn option_as_meta(mut self, option: OptionAsMeta) -> Self {
        self.keyboard_mut().option_as_meta = option;
        self
    }
}

/// The focused element's keyboard settings.
pub(crate) fn focused_config(tree: &LayoutNode, focused: Option<FocusId>) -> Option<&KeyConfig> {
    find_focus(tree, focused?)?.keyboard.as_deref()
}

/// What the input method should know about an element that opted into it
/// with [`Element::ime_input`]: where its caret is. It has no text to show
/// the IME.
pub(crate) fn custom_ime_state(node: &LayoutNode) -> Option<ImeState> {
    let config = node.keyboard.as_deref().filter(|k| k.ime)?;
    let b = node.bounds;
    let area = match config.ime_cursor {
        Some(r) => Rect::from_xywh(
            b.min_x() + r.min_x(),
            b.min_y() + r.min_y(),
            r.width(),
            r.height(),
        ),
        None => b,
    };
    Some(ImeState {
        cursor_area: area,
        surrounding: String::new(),
        cursor: 0,
        anchor: 0,
    })
}

/// What an element that captures Tab gets when its raw handler doesn't
/// take a Tab press: a tab character for Tab, nothing for Shift+Tab.
pub(crate) fn captured_tab_input(shift: bool) -> Option<KeyInput> {
    (!shift).then(|| KeyInput::Text("\t".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Cx, layout};
    use crate::platform::{ButtonState, KeyCode, PhysicalKey};
    use crate::style::Theme;

    #[test]
    fn raw_key_handlers_make_elements_focusable_and_dispatch() {
        let theme = Theme::light();
        let mut cx: Cx<Vec<KeyCode>> = Cx::new(&theme);
        let el = Element::boxed(Default::default())
            .width(100.0)
            .height(40.0)
            .on_raw_key(&mut cx, |s: &mut Vec<KeyCode>, ev: &KeyEvent| {
                s.push(ev.logical);
                (ev.logical == KeyCode::F1).into()
            })
            .captures_tab(true)
            .ime_cursor_area(Rect::from_xywh(10.0, 5.0, 2.0, 14.0))
            .option_as_meta(OptionAsMeta::Left);
        let focus = el.focus.expect("focusable");
        let tree = layout(&el, Rect::from_xywh(20.0, 30.0, 100.0, 40.0), None);
        let config = focused_config(&tree, Some(focus)).unwrap();
        assert!(config.captures_tab && config.ime);
        assert_eq!(config.option_as_meta, OptionAsMeta::Left);
        let raw = config.raw.unwrap();
        let mut handlers = cx.into_handlers();
        let mut seen = Vec::new();
        let ev = KeyEvent::new(ButtonState::Pressed, KeyCode::F1, PhysicalKey(0x3a));
        assert_eq!(
            handlers.dispatch_raw_key(raw, &ev, &mut seen),
            Some(Handled::Yes)
        );
        let ev = KeyEvent::new(ButtonState::Pressed, KeyCode::F2, PhysicalKey(0x3b));
        assert_eq!(
            handlers.dispatch_raw_key(raw, &ev, &mut seen),
            Some(Handled::No)
        );
        assert_eq!(seen, vec![KeyCode::F1, KeyCode::F2]);
        // The IME's caret is in window coordinates.
        let ime = custom_ime_state(find_focus(&tree, focus).unwrap()).unwrap();
        assert_eq!(ime.cursor_area, Rect::from_xywh(30.0, 35.0, 2.0, 14.0));
        assert_eq!(captured_tab_input(false), Some(KeyInput::Text("\t".into())));
        assert_eq!(captured_tab_input(true), None);
    }
}
