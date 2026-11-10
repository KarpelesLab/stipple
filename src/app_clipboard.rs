//! The clipboard in the [`App`]: pasting into text fields when the system
//! answers later ([`ClipboardRead::Pending`], X11), the primary selection
//! (a text field's selection becomes it; a middle click pastes it), and the
//! requests handlers queue with [`write_clipboard`](crate::core::write_clipboard)
//! and [`request_clipboard`](crate::core::request_clipboard), answered through
//! [`App::on_clipboard`].

use std::collections::VecDeque;

use crate::core::{ClipboardRequest, FocusId, KeyInput, LayoutNode};
use crate::geometry::Point;
use crate::platform::Window;
use crate::platform::clipboard::{
    ClipboardContent, ClipboardFormats, ClipboardKind, ClipboardRead,
};
use crate::{App, Pane};

/// What the app does with clipboard content once read: see [`App::on_clipboard`].
pub type ClipboardFn<S> = Box<dyn FnMut(&mut S, ClipboardKind, Option<ClipboardContent>)>;

/// Where a pending read's content goes.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    /// The function set with [`App::on_clipboard`].
    Hook,
    /// A paste into the text field that had focus in window `pane`.
    Field { pane: usize, focus: Option<FocusId> },
}

/// The app's clipboard state.
pub(crate) struct ClipboardState<S> {
    hook: Option<ClipboardFn<S>>,
    /// Reads the backend answers later, oldest first.
    pending: VecDeque<(ClipboardKind, Target)>,
    /// The selection last made the primary selection ("" when none since).
    primary_sent: String,
}

impl<S> Default for ClipboardState<S> {
    fn default() -> Self {
        Self {
            hook: None,
            pending: VecDeque::new(),
            primary_sent: String::new(),
        }
    }
}

impl<S> ClipboardState<S> {
    /// Make the focused field's selection the primary selection when it
    /// changed: in process, and on `window` where the system has one.
    pub(crate) fn sync_primary(
        &mut self,
        tree: Option<&LayoutNode>,
        focused: Option<FocusId>,
        window: Option<&dyn Window>,
    ) {
        let Some(sel) = tree.and_then(|t| crate::core::selection_text(t, focused)) else {
            self.primary_sent.clear();
            return;
        };
        if sel == self.primary_sent {
            return;
        }
        crate::core::set_primary_text(&sel);
        if let Some(w) = window.filter(|w| w.has_primary_selection()) {
            w.write_clipboard(ClipboardKind::Primary, &ClipboardContent::text(sel.clone()));
        }
        self.primary_sent = sel;
    }
}

/// Read `kind` through `window`, or from the in-process mirror where there's
/// no window or (for the primary selection) no system one.
fn read(
    window: Option<&dyn Window>,
    kind: ClipboardKind,
    formats: ClipboardFormats,
) -> ClipboardRead {
    match window {
        Some(w) if kind == ClipboardKind::Clipboard || w.has_primary_selection() => {
            w.read_clipboard(kind, formats)
        }
        _ => ClipboardRead::Ready(crate::core::clipboard_mirror(kind, formats)),
    }
}

impl<S: 'static> App<S> {
    /// Receive clipboard content asked for with
    /// [`request_clipboard`](crate::core::request_clipboard) from a handler:
    /// `f` gets the state, which selection it came from, and the content
    /// (`None` when there was nothing in the formats asked for, or the owner
    /// didn't answer). A terminal, say, asks for the primary selection on a
    /// middle click and writes the text it gets to its pty.
    pub fn on_clipboard(
        mut self,
        f: impl FnMut(&mut S, ClipboardKind, Option<ClipboardContent>) + 'static,
    ) -> Self {
        self.clipboard.hook = Some(Box::new(f));
        self
    }

    /// Paste the primary selection at `pos` in the primary window, as a middle
    /// click does: if editable text is there, focus it, put the caret at `pos`
    /// and insert the selection's text. Headless, this pastes the in-process
    /// primary selection (the text last selected in the app). Returns whether
    /// editable text was there.
    pub fn paste_primary_at(&mut self, pos: Point) -> bool {
        self.pane_paste_primary_at(0, pos, None)
    }

    /// Paste the primary selection at `pos` in window `idx` (a middle click).
    pub(crate) fn pane_paste_primary_at(
        &mut self,
        idx: usize,
        pos: Point,
        window: Option<&dyn Window>,
    ) -> bool {
        if !self.pane_text_press_at(idx, pos) {
            return false;
        }
        // A caret placement, not the start of a drag-selection or a
        // double-click.
        let pane: &mut Pane<S> = &mut self.panes[idx];
        pane.end_text_select();
        pane.clicks.reset();
        let focus = pane.focused;
        match read(window, ClipboardKind::Primary, ClipboardFormats::TEXT) {
            ClipboardRead::Ready(content) => {
                self.paste_into(idx, focus, ClipboardKind::Primary, content);
            }
            ClipboardRead::Pending => self
                .clipboard
                .pending
                .push_back((ClipboardKind::Primary, Target::Field { pane: idx, focus })),
        }
        true
    }

    /// Before Ctrl/Cmd+V in window `idx`: bring the system clipboard into the
    /// mirror. Returns `false` when the answer comes later (the paste then
    /// happens on [`Event::Clipboard`](crate::platform::Event::Clipboard)).
    pub(crate) fn clipboard_before_paste(&mut self, idx: usize, window: &dyn Window) -> bool {
        match window.read_clipboard(ClipboardKind::Clipboard, ClipboardFormats::TEXT) {
            ClipboardRead::Ready(content) => {
                if let Some(text) = content.and_then(|c| c.text) {
                    crate::core::set_clipboard_text(&text);
                }
                true
            }
            ClipboardRead::Pending => {
                let focus = self.panes[idx].focused;
                self.clipboard
                    .pending
                    .push_back((ClipboardKind::Clipboard, Target::Field { pane: idx, focus }));
                false
            }
        }
    }

    /// Paste `content`'s text into window `idx`'s focused field, if focus is
    /// still on `focus`.
    fn paste_into(
        &mut self,
        idx: usize,
        focus: Option<FocusId>,
        kind: ClipboardKind,
        content: Option<ClipboardContent>,
    ) -> bool {
        let text = content.and_then(|c| c.text).filter(|t| !t.is_empty());
        let Some(text) = text else { return false };
        if idx >= self.panes.len() || self.panes[idx].focused != focus {
            return false;
        }
        match kind {
            ClipboardKind::Clipboard => {
                crate::core::set_clipboard_text(&text);
                self.pane_press_key(idx, KeyInput::Paste)
            }
            ClipboardKind::Primary => self.pane_type_text(idx, &text),
        }
    }

    /// Content the backend delivered for a pending read. Returns whether
    /// anything changed.
    pub(crate) fn clipboard_event(
        &mut self,
        kind: ClipboardKind,
        content: Option<ClipboardContent>,
    ) -> bool {
        let pending = &mut self.clipboard.pending;
        let Some(at) = pending.iter().position(|(k, _)| *k == kind) else {
            return false;
        };
        let (_, target) = pending.remove(at).expect("found above");
        match target {
            Target::Hook => self.run_clipboard_hook(kind, content),
            Target::Field { pane, focus } => self.paste_into(pane, focus, kind, content),
        }
    }

    fn run_clipboard_hook(
        &mut self,
        kind: ClipboardKind,
        content: Option<ClipboardContent>,
    ) -> bool {
        let Some(hook) = self.clipboard.hook.as_mut() else {
            return false;
        };
        hook(&mut self.state, kind, content);
        for pane in &mut self.panes {
            pane.dirty = true;
        }
        true
    }

    /// Carry out the writes and reads handlers queued, through `window` (or
    /// the in-process mirrors without one).
    pub(crate) fn run_clipboard_requests(&mut self, window: Option<&dyn Window>) {
        // A hook may queue more; a few rounds settle it.
        for _ in 0..8 {
            let requests = crate::core::take_clipboard_requests();
            if requests.is_empty() {
                return;
            }
            for request in requests {
                match request {
                    ClipboardRequest::Write(kind, content) => {
                        if kind == ClipboardKind::Primary {
                            self.clipboard.primary_sent = content.text.clone().unwrap_or_default();
                        }
                        if let Some(w) = window
                            && (kind == ClipboardKind::Clipboard || w.has_primary_selection())
                        {
                            w.write_clipboard(kind, &content);
                        }
                    }
                    ClipboardRequest::Read(kind, formats) => match read(window, kind, formats) {
                        ClipboardRead::Ready(content) => {
                            self.run_clipboard_hook(kind, content);
                        }
                        ClipboardRead::Pending => {
                            self.clipboard.pending.push_back((kind, Target::Hook));
                        }
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::App;
    use crate::core::{Cx, Element};
    use crate::geometry::{Point, Size};
    use crate::platform::clipboard::{ClipboardContent, ClipboardFormats, ClipboardKind};
    use crate::style::Theme;
    use crate::widgets::{EditBuffer, text_editor};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct S {
        a: EditBuffer,
        b: EditBuffer,
    }

    fn view(s: &S, cx: &mut Cx<S>) -> Element {
        let t = Theme::light();
        crate::widgets::column(vec![
            text_editor(cx, &t, &s.a, |s: &mut S| &mut s.a)
                .width(200.0)
                .height(40.0),
            text_editor(cx, &t, &s.b, |s: &mut S| &mut s.b)
                .width(200.0)
                .height(40.0),
        ])
    }

    fn app() -> App<S> {
        let state = S {
            a: EditBuffer::from_text("hello world"),
            b: EditBuffer::from_text("xy"),
        };
        let mut app = App::new(state, view).logical_size(Size::new(300.0, 200.0));
        if let Some(font) = crate::render::Font::system_default() {
            app = app.font(font);
        }
        app
    }

    /// The bounds of the two fields' text leaves.
    fn fields(app: &mut App<S>) -> Vec<crate::geometry::Rect> {
        app.render_once();
        fn walk(n: &crate::core::LayoutNode, out: &mut Vec<crate::geometry::Rect>) {
            if n.text_pos.is_some() {
                out.push(n.bounds);
            }
            n.children.iter().for_each(|c| walk(c, out));
        }
        let mut out = Vec::new();
        walk(app.panes[0].tree.as_ref().unwrap(), &mut out);
        out
    }

    #[test]
    fn a_selection_becomes_primary_and_a_middle_click_pastes_it() {
        let mut app = app();
        let f = fields(&mut app);
        assert_eq!(f.len(), 2);
        // Select all of the first field: the selection is the primary.
        assert!(app.click_at(f[0].center()) || app.focused().is_some());
        app.text_press_at(f[0].center());
        app.end_text_select();
        app.press_key(crate::core::KeyInput::SelectAll);
        app.render_once();
        assert_eq!(crate::core::primary_text(), "hello world");
        // The clipboard proper is untouched.
        crate::core::set_clipboard_text("clip");
        // Middle-click at the start of the second field pastes it there.
        let at = Point::new(f[1].min_x() + 1.0, f[1].center().y);
        assert!(app.paste_primary_at(at));
        assert_eq!(app.state().b.text(), "hello worldxy");
        assert_eq!(crate::core::clipboard_text(), "clip");
        // Off any text: nothing.
        assert!(!app.paste_primary_at(Point::new(290.0, 190.0)));
    }

    #[test]
    fn handler_requests_reach_on_clipboard() {
        let got = Rc::new(RefCell::new(Vec::new()));
        let sink = got.clone();
        let mut app = app().on_clipboard(move |_s, kind, content| {
            sink.borrow_mut().push((kind, content));
        });
        crate::core::write_clipboard(ClipboardKind::Clipboard, ClipboardContent::text("t"));
        crate::core::request_clipboard(ClipboardKind::Clipboard, ClipboardFormats::TEXT);
        crate::core::request_clipboard(
            ClipboardKind::Primary,
            ClipboardFormats {
                image: true,
                ..Default::default()
            },
        );
        app.render_once();
        assert_eq!(
            *got.borrow(),
            vec![
                (ClipboardKind::Clipboard, Some(ClipboardContent::text("t"))),
                (ClipboardKind::Primary, None),
            ]
        );
    }
}
