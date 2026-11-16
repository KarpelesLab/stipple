//! A rich text editor: [`RichEditor`] (the document, caret, selection and
//! undo history, kept in the app state) and [`rich_editor`], the widget that
//! edits it. See `docs/rich-text-editing.md`.

mod html;
mod image_html;
mod inline;
mod markdown;
mod model;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::ops::Range;
use std::sync::Arc;

use model::{Attr, DocBuilder, OBJECT};
pub use model::{
    Block, BlockKind, ImagePlacement, ImageSize, ImageWrap, Inline, Marks, RichDocument, RichImage,
};

use crate::core::{Align, Axis, Cx, Element, Handled, KeyInput, TextHit};
use crate::geometry::{Insets, PhysicalSize, Point, Rect};
use crate::platform::clipboard::{ClipboardContent, ClipboardKind};
use crate::platform::{ButtonState, KeyCode, KeyEvent};
use crate::render::{
    Decoration, FloatImage, Font, FontStyle, InlineIcon, Pixmap, RichText, SpanStyle,
    UnderlineStyle, WrapSide,
};
use crate::style::Theme;

use super::EditBuffer;

impl RichDocument {
    /// The document as Markdown (CommonMark, with `~~` for
    /// strikethrough and `<u>` for underline). Every character Markdown
    /// would read as syntax is escaped; images are `![alt](src)` with their
    /// `src` as is. Empty paragraphs are written as `<br>`; spaces at the
    /// ends of a block as `&#32;`.
    pub fn to_markdown(&self) -> String {
        markdown::to_markdown(self)
    }

    /// Read Markdown: headings, paragraphs, lists (nested ones flattened),
    /// quotes, code blocks, emphasis, strikethrough, code spans, links,
    /// images and the inline HTML tags [`to_markdown`](Self::to_markdown)
    /// writes. Images with a `data:` URI are decoded; call
    /// [`load_images`](Self::load_images) for files.
    pub fn from_markdown(md: &str) -> Self {
        markdown::from_markdown(md, true)
    }

    /// The document as HTML: `<p>`, `<h1>`–`<h3>`, `<ul>`/`<ol>` with
    /// `<li>`, `<blockquote>`, `<pre><code>`, and `<strong>`, `<em>`, `<u>`,
    /// `<s>`, `<code>`, `<a href>` and `<img src alt>` inside, everything
    /// escaped. Images keep their `src` as is (a path, a URL, or the `data:`
    /// URI of an image made with [`RichImage::from_pixmap`]).
    pub fn to_html(&self) -> String {
        html::to_html(self)
    }

    /// Read HTML (a fragment or a page): the tags
    /// [`to_html`](Self::to_html) writes, `<b>`, `<i>`, `<del>`, `<strike>`,
    /// `<div>`, `<br>`, `h4`–`h6` (as level 3), simple `style` attributes
    /// (`font-weight`, `font-style`, `text-decoration`), and everything else
    /// as its text. Scripts and styles are skipped. Images with a `data:`
    /// URI are decoded.
    pub fn from_html(html: &str) -> Self {
        html::from_html(html, true)
    }
}

/// What kind of edit an undo step records, deciding whether the next edit
/// may join it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Typing,
    Backspace,
    Delete,
    /// Resizing or moving an image with the pointer: one step per drag.
    Image,
    Other,
}

/// What dragging from a press on an image does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DragKind {
    /// Nothing (the image is selected; the drag doesn't select text).
    Select,
    /// Resize it from a corner: `sign` 1 for a right corner, -1 for a left
    /// one; `centered` when it grows on both sides (centered and free
    /// images keep their center).
    Resize { sign: f64, centered: bool },
    /// Move a free image.
    Move,
}

/// A pointer drag on an image: which, how, from where, and the image as it
/// was at the press.
#[derive(Clone, Debug)]
struct ImageDrag {
    at: usize,
    kind: DragKind,
    origin: Point,
    /// The text area's width at the press.
    width: f64,
    /// The image's box at the press.
    rect: Rect,
    start: RichImage,
}

/// Where an image was last shown, as the last press on it found: for
/// making it free where it is.
#[derive(Clone, Copy, Debug)]
struct Shown {
    at: usize,
    rect: Rect,
    width: f64,
}

impl Shown {
    /// A free placement where it was shown: its center across, its top at
    /// its paragraph's.
    fn free_here(&self) -> ImagePlacement {
        let w = self.width.max(1.0);
        ImagePlacement::Free {
            x: (self.rect.center().x / w * 100.0).clamp(0.0, 100.0),
            y: self.rect.height() / 2.0 / w * 100.0,
        }
    }
}

/// How near a selected image's corner a press grabs its resize handle, in
/// logical pixels.
const HANDLE_REACH: f64 = 8.0;

/// The narrowest an image is resized to by its handles.
const MIN_IMAGE_WIDTH: f64 = 8.0;

/// The document, caret and selection before an edit.
#[derive(Clone, Debug)]
struct Snapshot {
    doc: RichDocument,
    caret: usize,
    anchor: Option<usize>,
}

/// The most undo steps kept.
const UNDO_LIMIT: usize = 300;

#[derive(Clone, Debug, Default)]
struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The kind of the last edit while it may still absorb the next one.
    open: Option<Step>,
}

thread_local! {
    /// The last fragment this thread copied, with the HTML it put on the
    /// clipboard: pasted back as is (images and all) while it is still
    /// what the clipboard holds.
    static LAST_COPY: RefCell<Option<(String, RichDocument)>> = const { RefCell::new(None) };
    /// The monospaced font code is shown in, looked up on first need.
    static MONO: RefCell<Option<Option<Font>>> = const { RefCell::new(None) };
}

/// A monospaced system font, if there is one.
fn mono_font() -> Option<Font> {
    MONO.with(|m| {
        m.borrow_mut()
            .get_or_insert_with(|| Font::from_family("monospace", FontStyle::default()).ok())
            .clone()
    })
}

/// The state of a [`rich_editor`]: a [`RichDocument`] with a caret and a
/// selection, the marks the next typed text takes, and an undo history.
/// Keep one in your app state.
///
/// Feed it keys with [`apply`](Self::apply) (the widget does), or drive it
/// from a toolbar: [`toggle_mark`](Self::toggle_mark),
/// [`toggle_block_kind`](Self::toggle_block_kind),
/// [`set_link`](Self::set_link), [`insert_image`](Self::insert_image),
/// [`undo`](Self::undo); [`marks`](Self::marks) and
/// [`block_kind`](Self::block_kind) say what to show as pressed.
///
/// Byte indices (the caret, a selection) are into the document's text: the
/// blocks joined by `\n`, each image one U+FFFC.
///
/// ```
/// use stipple::core::KeyInput;
/// use stipple::widgets::{Marks, RichEditor};
/// let mut ed = RichEditor::new();
/// ed.apply(&KeyInput::Text("Hello ".into()));
/// ed.toggle_mark(Marks::BOLD); // bold from here on
/// ed.apply(&KeyInput::Text("world".into()));
/// assert_eq!(ed.to_markdown(), "Hello **world**\n");
/// ed.undo();
/// assert_eq!(ed.plain_text(), "Hello "); // the bold word was its own step
/// ```
#[derive(Clone, Debug, Default)]
pub struct RichEditor {
    doc: RichDocument,
    /// The caret, selection anchor, direction and composition, and the
    /// navigation keys' logic. Its text is the document's only while a key
    /// moves the caret (see `with_nav`); empty otherwise.
    nav: EditBuffer,
    /// Marks toggled at a collapsed caret, for the text typed there next.
    pending: Option<(usize, Attr)>,
    history: History,
    revision: u64,
    /// A pointer drag on an image, from the press on it to the next press
    /// or key.
    image_drag: Option<ImageDrag>,
    /// Where the image last pressed was shown.
    last_shown: Option<Shown>,
}

/// One line of the editor as shown: where its text is in the document, the
/// prefix drawn before it, and where its text starts in what's shown.
#[derive(Clone, Debug)]
struct LineMap {
    model: Range<usize>,
    prefix: String,
    /// The shown text's byte index where the line's own text starts.
    shown: usize,
    /// Bytes of composition shown in this line.
    preedit: usize,
}

impl RichEditor {
    /// An empty editor.
    pub fn new() -> Self {
        Self::default()
    }

    /// An editor holding `doc`, the caret at its end.
    pub fn from_document(doc: RichDocument) -> Self {
        let mut ed = Self::default();
        ed.set_document(doc);
        ed
    }

    /// An editor holding the document `md` (Markdown) describes.
    pub fn from_markdown(md: &str) -> Self {
        Self::from_document(RichDocument::from_markdown(md))
    }

    /// The document.
    pub fn document(&self) -> &RichDocument {
        &self.doc
    }

    /// Replace the document, the caret at its end, forgetting the undo
    /// history.
    pub fn set_document(&mut self, doc: RichDocument) {
        let end = doc.text.len();
        self.doc = doc;
        self.nav.preedit = None;
        self.image_drag = None;
        self.last_shown = None;
        self.set_caret(end, None);
        self.pending = None;
        self.history = History::default();
        self.revision += 1;
    }

    /// The document as Markdown ([`RichDocument::to_markdown`]).
    pub fn to_markdown(&self) -> String {
        self.doc.to_markdown()
    }

    /// The document as HTML ([`RichDocument::to_html`]).
    pub fn to_html(&self) -> String {
        self.doc.to_html()
    }

    /// The document as plain text ([`RichDocument::plain_text`]).
    pub fn plain_text(&self) -> String {
        self.doc.plain_text()
    }

    /// Counts changes to the document: compare two readings to tell whether
    /// it changed.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The caret, a byte index into the document's text.
    pub fn caret(&self) -> usize {
        self.nav.caret
    }

    /// The selected byte range, if any.
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.nav.selection()
    }

    /// Select `anchor..caret` (byte indices, snapped to characters); equal
    /// ends place a plain caret.
    pub fn select(&mut self, anchor: usize, caret: usize) {
        self.with_nav(|b| b.set_selection(anchor, caret));
        self.history.open = None;
    }

    /// Select the whole document.
    pub fn select_all(&mut self) {
        let end = self.doc.text.len();
        self.select(0, end);
    }

    /// Whether there is an edit to undo.
    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    /// Whether there is an undone edit to redo.
    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    /// Undo the last edit, restoring the document, caret and selection.
    /// Returns whether there was one.
    pub fn undo(&mut self) -> bool {
        let Some(snap) = self.history.undo.pop() else {
            return false;
        };
        let now = self.snapshot();
        self.history.redo.push(now);
        self.restore(snap);
        true
    }

    /// Redo the last undone edit. Returns whether there was one.
    pub fn redo(&mut self) -> bool {
        let Some(snap) = self.history.redo.pop() else {
            return false;
        };
        let now = self.snapshot();
        self.history.undo.push(now);
        self.restore(snap);
        true
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            doc: self.doc.clone(),
            caret: self.nav.caret,
            anchor: self.nav.anchor,
        }
    }

    fn restore(&mut self, snap: Snapshot) {
        self.doc = snap.doc;
        self.nav.preedit = None;
        self.image_drag = None;
        self.set_caret(snap.caret, snap.anchor);
        self.pending = None;
        self.history.open = None;
        self.revision += 1;
    }

    /// Start an edit of `step`: a new undo step unless it continues the
    /// last one (typing after typing).
    fn begin(&mut self, step: Step) {
        let joins = step != Step::Other && self.history.open == Some(step);
        if !joins {
            let snap = self.snapshot();
            self.history.undo.push(snap);
            if self.history.undo.len() > UNDO_LIMIT {
                self.history.undo.remove(0);
            }
        }
        self.history.redo.clear();
        self.history.open = (step != Step::Other).then_some(step);
    }

    fn set_caret(&mut self, caret: usize, anchor: Option<usize>) {
        self.nav.caret = caret;
        self.nav.anchor = anchor.filter(|a| *a != caret);
    }

    /// Run `f` on the navigation buffer holding the document's text.
    fn with_nav<R>(&mut self, f: impl FnOnce(&mut EditBuffer) -> R) -> R {
        std::mem::swap(&mut self.nav.text, &mut self.doc.text);
        let r = f(&mut self.nav);
        std::mem::swap(&mut self.nav.text, &mut self.doc.text);
        r
    }

    /// Replace `range` with `frag` (recorded by the caller's `begin`),
    /// the caret after it.
    fn splice(&mut self, range: Range<usize>, frag: &RichDocument) {
        self.doc.replace(range.clone(), frag);
        self.set_caret(range.start + frag.text.len(), None);
        self.pending = None;
        self.revision += 1;
    }

    /// The selection, or the caret as an empty range.
    fn target(&self) -> Range<usize> {
        match self.selection() {
            Some((s, e)) => s..e,
            None => self.nav.caret..self.nav.caret,
        }
    }

    /// The character boundary before `i`.
    fn prev_boundary(&self, i: usize) -> usize {
        self.doc.text[..i]
            .char_indices()
            .next_back()
            .map_or(0, |(p, _)| p)
    }

    /// What text typed at the caret looks like: the marks toggled there, or
    /// those of the character before it (after it at a block's start). A
    /// link continues only inside it, never past its end.
    fn typing_attr(&self) -> Attr {
        let c = self.nav.caret;
        if let Some((at, attr)) = &self.pending
            && *at == c
            && self.selection().is_none()
        {
            return attr.clone();
        }
        let line = self.doc.line_range(self.doc.line_of(c));
        let before = (c > line.start).then(|| self.doc.spans.at(self.prev_boundary(c)));
        let after = (c < line.end).then(|| self.doc.spans.at(c)).flatten();
        let mut attr = before.flatten().or(after).cloned().unwrap_or_default();
        attr.image = None;
        if c == line.start || after.and_then(|a| a.link.as_ref()) != attr.link.as_ref() {
            attr.link = None;
        }
        attr
    }

    /// A fragment of `text` (lines split on `\n`) in `attr`, its first line
    /// of kind `first`, the others of `rest`.
    fn fragment(text: &str, attr: &Attr, first: BlockKind, rest: BlockKind) -> RichDocument {
        let mut b = DocBuilder::new();
        for (i, line) in text.split('\n').enumerate() {
            b.start_block(if i == 0 { first } else { rest });
            b.push_text(line, attr);
        }
        b.finish()
    }

    /// Insert `text` at the caret (in place of the selection) as typed,
    /// in the marks typing would give it. Line breaks start new blocks.
    pub fn insert_text(&mut self, text: &str) {
        self.type_text(text);
    }

    fn type_text(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if text.is_empty() {
            return;
        }
        let attr = self.typing_attr();
        let range = self.target();
        let step = if range.is_empty() && !text.contains('\n') {
            Step::Typing
        } else {
            Step::Other
        };
        let kind = self.doc.kinds[self.doc.line_of(range.start)];
        let frag = Self::fragment(&text, &attr, kind, kind.next());
        self.begin(step);
        self.splice(range, &frag);
    }

    /// Insert `frag` (pasted content) in place of the selection, as one
    /// undo step.
    pub fn insert_document(&mut self, frag: &RichDocument) {
        let range = self.target();
        self.begin(Step::Other);
        self.splice(range, frag);
    }

    /// Insert `image` at the caret, in place of the selection.
    pub fn insert_image(&mut self, image: RichImage) {
        let mut attr = self.typing_attr();
        attr.marks = Marks::NONE;
        let mut b = DocBuilder::new();
        b.start_block(BlockKind::Paragraph);
        b.push_image(Arc::new(image), &attr);
        let frag = b.finish();
        self.insert_document(&frag);
    }

    /// Delete the selection as its own step, if there is one.
    fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some((s, e)) => {
                self.begin(Step::Other);
                self.splice(s..e, &RichDocument::default());
                true
            }
            None => false,
        }
    }

    /// Enter: split the block at the caret. An empty list item or quote
    /// turns into a paragraph instead (ending the list).
    fn enter(&mut self) {
        let mut stepped = false;
        if let Some((s, e)) = self.selection() {
            self.begin(Step::Other);
            stepped = true;
            self.splice(s..e, &RichDocument::default());
        }
        let c = self.nav.caret;
        let line = self.doc.line_of(c);
        let range = self.doc.line_range(line);
        let kind = self.doc.kinds[line];
        if !stepped {
            self.begin(Step::Other);
        }
        if range.is_empty() && !matches!(kind, BlockKind::Paragraph | BlockKind::CodeBlock) {
            self.doc.kinds[line] = BlockKind::Paragraph;
            self.pending = None;
            self.revision += 1;
            return;
        }
        let at_start = c == range.start && !range.is_empty();
        let below = if at_start { kind } else { kind.next() };
        let frag = Self::fragment("\n", &Attr::default(), kind, below);
        let pending = self.pending.take().filter(|(at, _)| *at == c);
        self.splice(c..c, &frag);
        if at_start {
            self.doc.kinds[line] = kind.next();
        }
        // Marks toggled at the caret carry over to the new block.
        if let Some((_, attr)) = pending {
            self.pending = Some((self.nav.caret, attr));
        }
    }

    /// Backspace: the selection, or the character before the caret; at a
    /// block's start, a list item, heading or quote turns into a paragraph,
    /// and a paragraph joins the block before.
    fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let c = self.nav.caret;
        let line = self.doc.line_of(c);
        let kind = self.doc.kinds[line];
        if c == self.doc.line_range(line).start {
            let joins_code = kind == BlockKind::CodeBlock
                && line > 0
                && self.doc.kinds[line - 1] == BlockKind::CodeBlock;
            if kind != BlockKind::Paragraph && !joins_code {
                self.begin(Step::Other);
                self.doc.kinds[line] = BlockKind::Paragraph;
                self.pending = None;
                self.revision += 1;
                return;
            }
            if line == 0 {
                return;
            }
            self.begin(Step::Other);
            self.splice(c - 1..c, &RichDocument::default());
            return;
        }
        let prev = self.prev_boundary(c);
        self.begin(Step::Backspace);
        self.splice(prev..c, &RichDocument::default());
    }

    /// Delete: the selection, or the character after the caret (joining
    /// the next block at a block's end).
    fn delete_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        let c = self.nav.caret;
        let Some(ch) = self.doc.text[c..].chars().next() else {
            return;
        };
        self.begin(if ch == '\n' {
            Step::Other
        } else {
            Step::Delete
        });
        self.splice(c..c + ch.len_utf8(), &RichDocument::default());
    }

    /// Delete from the caret to `to` (a word's edge), unless there's a
    /// selection to delete.
    fn delete_to(&mut self, to: usize) {
        if self.delete_selection() {
            return;
        }
        let c = self.nav.caret;
        let range = c.min(to)..c.max(to);
        if !range.is_empty() {
            self.begin(Step::Other);
            self.splice(range, &RichDocument::default());
        }
    }

    /// Copy the selection: plain text and HTML.
    fn copy(&self) -> bool {
        let Some((s, e)) = self.selection() else {
            return false;
        };
        let frag = self.doc.slice(s..e);
        let html = frag.to_html();
        crate::core::write_clipboard(
            ClipboardKind::Clipboard,
            ClipboardContent::from_text(frag.plain_text()).html(html.clone()),
        );
        LAST_COPY.with(|c| *c.borrow_mut() = Some((html, frag)));
        true
    }

    /// Paste the clipboard: its HTML if it has some, else its image, else
    /// its text (in the marks typing would give it).
    fn paste(&mut self) {
        let content = crate::core::clipboard_content();
        let from_html = content.html.as_deref().and_then(|html| {
            let frag = LAST_COPY
                .with(|c| {
                    c.borrow()
                        .as_ref()
                        .filter(|(h, _)| h == html)
                        .map(|(_, f)| f.clone())
                })
                .unwrap_or_else(|| RichDocument::from_html(html));
            (!frag.is_empty()).then_some(frag)
        });
        if let Some(frag) = from_html {
            self.insert_document(&frag);
        } else if let Some(image) = content.image {
            self.insert_image(RichImage::from_pixmap(image).alt("image"));
        } else if let Some(text) = content.text.filter(|t| !t.is_empty()) {
            let range = self.target();
            let attr = self.typing_attr();
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            let kind = self.doc.kinds[self.doc.line_of(range.start)];
            let frag = Self::fragment(&text, &attr, kind, kind.next());
            self.begin(Step::Other);
            self.splice(range, &frag);
        }
    }

    /// Apply a key as the widget does while focused: typing, deleting,
    /// moving and selecting (as a [`text_area`](super::text_area) does),
    /// Enter splitting blocks, undo and redo, the clipboard (HTML and
    /// images too), and input-method composition. Byte indices in
    /// [`KeyInput::SetSelection`] are into what the editor shows (its
    /// bullets and numbers included), as assistive technologies see it.
    /// Returns whether the document changed.
    pub fn apply(&mut self, input: &KeyInput) -> bool {
        let revision = self.revision;
        // A double or triple click on an image keeps it selected.
        if matches!(input, KeyInput::SelectWord | KeyInput::SelectLine) && self.image_drag.is_some()
        {
            return false;
        }
        self.image_drag = None;
        match input {
            KeyInput::Text(t) => {
                self.nav.preedit = None;
                self.type_text(t);
            }
            KeyInput::Preedit { text, cursor } => {
                if !text.is_empty() && self.nav.preedit.is_none() {
                    self.delete_selection();
                }
                self.nav.set_preedit(text, *cursor);
                self.history.open = None;
            }
            KeyInput::DeleteSurrounding { before, after } => {
                let c = self.nav.caret;
                let t = &self.doc.text;
                let mut s = c.saturating_sub(*before);
                while !t.is_char_boundary(s) {
                    s -= 1;
                }
                let mut e = (c + after).min(t.len());
                while !t.is_char_boundary(e) {
                    e += 1;
                }
                if s < e {
                    self.begin(Step::Other);
                    self.splice(s..e, &RichDocument::default());
                }
            }
            KeyInput::SetSelection { anchor, caret } => {
                let (a, c) = (self.shown_to_model(*anchor), self.shown_to_model(*caret));
                self.select(a, c);
            }
            KeyInput::Backspace => self.backspace(),
            KeyInput::Delete => self.delete_forward(),
            KeyInput::DeleteWordBackward => {
                let to = self.with_nav(|b| b.word_left_of(b.caret));
                self.delete_to(to);
            }
            KeyInput::DeleteWordForward => {
                let to = self.with_nav(|b| b.word_right_of(b.caret));
                self.delete_to(to);
            }
            KeyInput::Enter => self.enter(),
            KeyInput::Undo => {
                self.undo();
            }
            KeyInput::Redo => {
                self.redo();
            }
            KeyInput::Copy => {
                self.copy();
            }
            KeyInput::Cut => {
                if self.copy() {
                    self.delete_selection();
                }
            }
            KeyInput::Paste => self.paste(),
            KeyInput::Escape => {}
            nav @ (KeyInput::Left
            | KeyInput::Right
            | KeyInput::Up
            | KeyInput::Down
            | KeyInput::Home
            | KeyInput::End
            | KeyInput::SelectLeft
            | KeyInput::SelectRight
            | KeyInput::SelectUp
            | KeyInput::SelectDown
            | KeyInput::SelectHome
            | KeyInput::SelectEnd
            | KeyInput::SelectAll
            | KeyInput::WordLeft
            | KeyInput::WordRight
            | KeyInput::SelectWordLeft
            | KeyInput::SelectWordRight
            | KeyInput::SelectWord
            | KeyInput::SelectLine) => {
                self.with_nav(|b| b.apply(nav));
                self.history.open = None;
                self.pending = None;
            }
        }
        self.revision != revision
    }

    /// Each line's range in the document (without its `\n`).
    fn line_ranges(&self) -> Vec<Range<usize>> {
        let starts = self.doc.line_starts();
        starts
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let end = starts
                    .get(i + 1)
                    .map_or(self.doc.text.len(), |next| next - 1);
                *s..end
            })
            .collect()
    }

    /// The lines that `range` (or the caret) touches.
    fn lines_of(&self, range: Range<usize>) -> Range<usize> {
        self.doc.line_of(range.start)..self.doc.line_of(range.end) + 1
    }

    /// Change the attributes of the text in `range` (not its line breaks).
    fn update_text(&mut self, range: Range<usize>, mut f: impl FnMut(&mut Attr)) {
        for line in self.line_ranges() {
            let (s, e) = (line.start.max(range.start), line.end.min(range.end));
            if s < e {
                self.doc.spans.update(s..e, &mut f);
            }
        }
    }

    /// The attributes of each run of text (not line breaks) in `range`.
    fn attrs_in(&self, range: Range<usize>) -> Vec<&Attr> {
        let lines = self.line_ranges();
        self.doc
            .spans
            .ranges()
            .filter(|(r, _)| {
                lines.iter().any(|l| {
                    r.start.max(l.start).max(range.start) < r.end.min(l.end).min(range.end)
                })
            })
            .map(|(_, a)| a)
            .collect()
    }

    /// The marks in effect: those all of the selection has, or those the
    /// next typed text takes. What a toolbar shows as pressed.
    pub fn marks(&self) -> Marks {
        if let Some((s, e)) = self.selection() {
            let attrs = self.attrs_in(s..e);
            if !attrs.is_empty() {
                return attrs.iter().fold(Marks::EVERY, |m, a| m.intersect(a.marks));
            }
        }
        self.typing_attr().marks
    }

    /// Toggle `mark`: on the selection (off if all of it has it, else on),
    /// or, at a plain caret, for the text typed next.
    pub fn toggle_mark(&mut self, mark: Marks) {
        let on = !self.marks().contains(mark);
        match self.selection() {
            Some((s, e)) => {
                self.begin(Step::Other);
                self.update_text(s..e, |a| {
                    a.marks = if on {
                        a.marks.with(mark)
                    } else {
                        a.marks.without(mark)
                    };
                });
                self.history.open = None;
                self.revision += 1;
            }
            None => {
                let mut attr = self.typing_attr();
                attr.marks = if on {
                    attr.marks.with(mark)
                } else {
                    attr.marks.without(mark)
                };
                self.pending = Some((self.nav.caret, attr));
                self.history.open = None;
            }
        }
    }

    /// The link at the caret (or on the selection's first character).
    pub fn link(&self) -> Option<String> {
        let at = match self.selection() {
            Some((s, _)) => s,
            None => {
                let c = self.nav.caret;
                let line = self.doc.line_range(self.doc.line_of(c));
                if c > line.start {
                    self.prev_boundary(c)
                } else {
                    c
                }
            }
        };
        self.doc
            .spans
            .at(at)
            .and_then(|a| a.link.as_deref())
            .map(str::to_string)
    }

    /// The byte range of the link around the caret, if it is in one.
    fn link_range_at_caret(&self) -> Option<Range<usize>> {
        let c = self.nav.caret;
        let line = self.doc.line_range(self.doc.line_of(c));
        let probe = if c > line.start {
            self.prev_boundary(c)
        } else {
            c
        };
        let link = self.doc.spans.at(probe)?.link.clone()?;
        let runs: Vec<(Range<usize>, &Attr)> = self
            .doc
            .spans
            .ranges()
            .filter(|(r, _)| r.start < line.end && r.end > line.start)
            .collect();
        let i = runs.iter().position(|(r, _)| r.contains(&probe))?;
        let same = |j: usize| runs[j].1.link.as_ref() == Some(&link);
        let (mut a, mut b) = (i, i);
        while a > 0 && same(a - 1) {
            a -= 1;
        }
        while b + 1 < runs.len() && same(b + 1) {
            b += 1;
        }
        Some(runs[a].0.start.max(line.start)..runs[b].0.end.min(line.end))
    }

    /// Link the selection to `url` (`None` removes links from it). At a
    /// plain caret inside a link, change or remove that whole link; elsewhere
    /// insert `url` as linked text.
    pub fn set_link(&mut self, url: Option<&str>) {
        let link: Option<Arc<str>> = url.filter(|u| !u.is_empty()).map(Arc::from);
        let range = match self.selection() {
            Some((s, e)) => Some(s..e),
            None => self.link_range_at_caret(),
        };
        match range {
            Some(r) => {
                self.begin(Step::Other);
                self.update_text(r, |a| a.link = link.clone());
                self.history.open = None;
                self.revision += 1;
            }
            None => {
                if let Some(link) = link {
                    let mut attr = self.typing_attr();
                    attr.link = Some(link.clone());
                    let range = self.target();
                    let kind = self.doc.kinds[self.doc.line_of(range.start)];
                    let frag = Self::fragment(&link, &attr, kind, kind);
                    self.begin(Step::Other);
                    self.splice(range, &frag);
                }
            }
        }
    }

    /// The byte ranges of the images the selection holds, or of the image
    /// just before the caret (else just after it).
    fn image_ranges(&self) -> Vec<Range<usize>> {
        let range = match self.selection() {
            Some((s, e)) => s..e,
            None => {
                let c = self.nav.caret;
                let before = self.prev_boundary(c);
                let is_image = |at: usize| {
                    self.doc.text[at..].starts_with(OBJECT)
                        && self.doc.spans.at(at).is_some_and(|a| a.image.is_some())
                };
                if c > 0 && is_image(before) {
                    before..c
                } else if c < self.doc.text.len() && is_image(c) {
                    c..c + OBJECT.len_utf8()
                } else {
                    return Vec::new();
                }
            }
        };
        self.doc
            .spans
            .ranges()
            .filter(|(r, a)| a.image.is_some() && r.start < range.end && r.end > range.start)
            .map(|(r, _)| r.start.max(range.start)..r.end.min(range.end))
            .collect()
    }

    /// The image at the caret (or the first selected one), if there is one:
    /// what a toolbar shows (its placement, size and wrapping).
    pub fn image(&self) -> Option<RichImage> {
        let r = self.image_ranges().into_iter().next()?;
        self.doc
            .spans
            .at(r.start)
            .and_then(|a| a.image.as_deref())
            .cloned()
    }

    /// How the image at the caret (or the first selected one) is placed:
    /// `None` when there is no image there. What a toolbar's inline / left /
    /// center / right / free buttons show as pressed (compare with
    /// [`ImagePlacement::same_kind`]).
    pub fn image_placement(&self) -> Option<ImagePlacement> {
        self.image().map(|i| i.placement)
    }

    /// The byte of the one image the selection holds, when it holds exactly
    /// one image and nothing else: the image shown selected, with its frame
    /// and resize handles.
    pub fn selected_image(&self) -> Option<usize> {
        let (s, e) = self.selection()?;
        (e - s == OBJECT.len_utf8()
            && self.doc.text[s..e].starts_with(OBJECT)
            && self.doc.spans.at(s).is_some_and(|a| a.image.is_some()))
        .then_some(s)
    }

    /// Change the selected images (or the one at the caret) with `f`, as one
    /// undo step (joining the last one when `step` continues it). Returns
    /// whether there was an image.
    fn update_images(&mut self, step: Step, mut f: impl FnMut(RichImage) -> RichImage) -> bool {
        let ranges = self.image_ranges();
        if ranges.is_empty() {
            return false;
        }
        self.begin(step);
        for r in ranges {
            self.doc.spans.update(r, |a| {
                if let Some(image) = &a.image {
                    a.image = Some(Arc::new(f((**image).clone())));
                }
            });
        }
        if step == Step::Other {
            self.history.open = None;
        }
        self.revision += 1;
        true
    }

    /// Place the selected images (or the one at the caret) per `placement`:
    /// in the line, floated left or right, centered, or free, the text
    /// wrapping around their opaque pixels. One undo step; nothing without
    /// an image there. A free placement given while the image isn't free yet
    /// starts it where it was last shown, when the editor knows (it was
    /// clicked), so it doesn't jump.
    pub fn set_image_placement(&mut self, placement: ImagePlacement) {
        let placement = match (placement, self.image_placement(), &self.last_shown) {
            (ImagePlacement::Free { .. }, Some(p), Some(shown))
                if !matches!(p, ImagePlacement::Free { .. })
                    && self.image_ranges().first().map(|r| r.start) == Some(shown.at) =>
            {
                shown.free_here()
            }
            _ => placement,
        };
        self.update_images(Step::Other, |i| i.placed(placement));
    }

    /// Make text wrap around the selected images (or the one at the caret)
    /// per `wrap`. One undo step.
    pub fn set_image_wrap(&mut self, wrap: ImageWrap) {
        self.update_images(Step::Other, |i| i.wrapped(wrap));
    }

    /// Size the selected images (or the one at the caret) per `size`. One
    /// undo step, which sizes set right after it join (a slider dragged).
    pub fn set_image_size(&mut self, size: ImageSize) {
        self.update_images(Step::Image, |i| i.sized(size));
    }

    /// Bound the selected images' (or the caret's image's) width to `min`
    /// and `max` logical pixels, and let them grow past their natural size
    /// or not (`upscale`). One undo step.
    pub fn set_image_bounds(&mut self, min: Option<f64>, max: Option<f64>, upscale: bool) {
        self.update_images(Step::Other, |i| {
            i.min_width(min).max_width(max).upscale(upscale)
        });
    }

    /// A press on the editor's text at `hit` (shown-text coordinates): on
    /// the selected image's corner handle it starts resizing it, on an
    /// image it selects it (and on a free one starts moving it). Returns
    /// whether it was on an image.
    pub(crate) fn press_image(&mut self, hit: &TextHit) -> bool {
        self.image_drag = None;
        let objects: Vec<(usize, Rect)> = hit
            .objects
            .iter()
            .map(|(i, r)| (self.shown_to_model(*i), *r))
            .filter(|(i, _)| {
                self.doc.text[*i..].starts_with(OBJECT)
                    && self.doc.spans.at(*i).is_some_and(|a| a.image.is_some())
            })
            .collect();
        let image_at = |at: usize| self.doc.spans.at(at).and_then(|a| a.image.clone());
        let p = hit.point;
        // A corner handle of the selected image.
        if let Some(sel) = self.selected_image()
            && let Some((_, r)) = objects.iter().find(|(i, _)| *i == sel)
            && let Some(image) = image_at(sel)
        {
            let reach = HANDLE_REACH;
            for (cx, cy, sign) in [
                (r.min_x(), r.min_y(), -1.0),
                (r.max_x(), r.min_y(), 1.0),
                (r.min_x(), r.max_y(), -1.0),
                (r.max_x(), r.max_y(), 1.0),
            ] {
                if (p.x - cx).abs() <= reach && (p.y - cy).abs() <= reach {
                    let centered = matches!(
                        image.placement,
                        ImagePlacement::Center | ImagePlacement::Free { .. }
                    );
                    self.start_drag(sel, *r, hit, DragKind::Resize { sign, centered }, &image);
                    return true;
                }
            }
        }
        // The top-most image under the pointer (floats are drawn first).
        let Some(&(at, r)) = objects.iter().rev().find(|(_, r)| r.contains(p)) else {
            return false;
        };
        let Some(image) = image_at(at) else {
            return false;
        };
        self.nav.preedit = None;
        self.select(at, at + OBJECT.len_utf8());
        let kind = if matches!(image.placement, ImagePlacement::Free { .. }) {
            DragKind::Move
        } else {
            DragKind::Select
        };
        self.start_drag(at, r, hit, kind, &image);
        true
    }

    fn start_drag(
        &mut self,
        at: usize,
        rect: Rect,
        hit: &TextHit,
        kind: DragKind,
        image: &RichImage,
    ) {
        self.history.open = None;
        self.pending = None;
        self.last_shown = Some(Shown {
            at,
            rect,
            width: hit.width,
        });
        self.image_drag = Some(ImageDrag {
            at,
            kind,
            origin: hit.point,
            width: hit.width,
            rect,
            start: image.clone(),
        });
    }

    /// The pointer dragged to `hit` after [`press_image`](Self::press_image)
    /// started a drag: resize or move the image. Returns whether a drag was
    /// on (else the pointer is selecting text).
    pub(crate) fn drag_image(&mut self, hit: &TextHit) -> bool {
        let Some(drag) = self.image_drag.clone() else {
            return false;
        };
        let (dx, dy) = (hit.point.x - drag.origin.x, hit.point.y - drag.origin.y);
        let w = drag.width.max(1.0);
        let start = &drag.start;
        let image = match drag.kind {
            DragKind::Select => return true,
            DragKind::Resize { sign, centered } => {
                let k = if centered { 2.0 } else { 1.0 };
                let mut new_w = (drag.rect.width() + sign * dx * k).max(MIN_IMAGE_WIDTH);
                if !start.upscale
                    && let Some(n) = start.natural_size()
                {
                    new_w = new_w.min(n.width.max(MIN_IMAGE_WIDTH));
                }
                if let Some(max) = start.max_width {
                    new_w = new_w.min(max);
                }
                let size = match start.size {
                    ImageSize::Percent(_) => ImageSize::Percent((new_w / w * 100.0).min(100.0)),
                    _ => ImageSize::Pixels(new_w.round()),
                };
                start.clone().sized(size)
            }
            DragKind::Move => {
                let ImagePlacement::Free { x, y } = start.placement else {
                    return true;
                };
                start.clone().placed(ImagePlacement::Free {
                    x: (x + dx / w * 100.0).clamp(0.0, 100.0),
                    y: y + dy / w * 100.0,
                })
            }
        };
        let at = drag.at;
        let current = self.doc.spans.at(at).and_then(|a| a.image.clone());
        if current.as_deref() == Some(&image) {
            return true;
        }
        self.begin(Step::Image);
        let image = Arc::new(image);
        self.doc.spans.update(at..at + OBJECT.len_utf8(), |a| {
            a.image = Some(image.clone())
        });
        self.set_caret(at + OBJECT.len_utf8(), Some(at));
        self.revision += 1;
        true
    }

    /// The kind of the block holding the caret.
    pub fn block_kind(&self) -> BlockKind {
        self.doc.kinds[self.doc.line_of(self.nav.caret)]
    }

    /// Make the blocks the selection touches (or the caret's) `kind`.
    pub fn set_block_kind(&mut self, kind: BlockKind) {
        let lines = self.lines_of(self.target());
        if self.doc.kinds[lines.clone()].iter().all(|k| *k == kind) {
            return;
        }
        self.begin(Step::Other);
        for k in &mut self.doc.kinds[lines] {
            *k = kind;
        }
        self.history.open = None;
        self.revision += 1;
    }

    /// Make the blocks the selection touches (or the caret's) `kind`, or
    /// paragraphs if they all already are: what a toolbar's list, quote or
    /// heading button does.
    pub fn toggle_block_kind(&mut self, kind: BlockKind) {
        let lines = self.lines_of(self.target());
        let all = self.doc.kinds[lines].iter().all(|k| *k == kind);
        self.set_block_kind(if all { BlockKind::Paragraph } else { kind });
    }

    // --- What the widget shows ---

    /// Each line as shown, with its prefix and where it lands.
    fn line_maps(&self) -> Vec<LineMap> {
        let prefixes = self.doc.prefixes();
        let p = self.nav.preedit.as_ref().map_or(0, |p| p.text.len());
        let c = self.nav.caret;
        let mut shown = 0;
        self.line_ranges()
            .into_iter()
            .zip(prefixes)
            .enumerate()
            .map(|(i, (model, prefix))| {
                if i > 0 {
                    shown += 1;
                }
                shown += prefix.len();
                let preedit = if model.start <= c && c <= model.end {
                    p
                } else {
                    0
                };
                let map = LineMap {
                    shown,
                    model: model.clone(),
                    prefix,
                    preedit,
                };
                shown += model.len() + preedit;
                map
            })
            .collect()
    }

    /// Where document byte `i` is in what's shown.
    fn model_to_shown(&self, maps: &[LineMap], i: usize) -> usize {
        let c = self.nav.caret;
        let m = maps
            .iter()
            .find(|m| m.model.start <= i && i <= m.model.end)
            .unwrap_or(&maps[maps.len() - 1]);
        let i = i.min(m.model.end);
        m.shown + (i - m.model.start) + if i > c { m.preedit } else { 0 }
    }

    /// Where shown byte `i` (a click, an assistive technology's selection)
    /// is in the document: inside a line's prefix, its start; inside the
    /// composition, the caret.
    pub(crate) fn shown_to_model(&self, i: usize) -> usize {
        let maps = self.line_maps();
        let c = self.nav.caret;
        for m in &maps {
            let end = m.shown + m.model.len() + m.preedit;
            if i > end {
                continue;
            }
            if i <= m.shown {
                return m.model.start;
            }
            let mut off = i - m.shown;
            if m.preedit > 0 {
                let cc = c - m.model.start;
                if off > cc + m.preedit {
                    off -= m.preedit;
                } else if off > cc {
                    off = cc;
                }
            }
            let mut at = (m.model.start + off).min(m.model.end);
            while !self.doc.text.is_char_boundary(at) {
                at -= 1;
            }
            return at;
        }
        self.doc.text.len()
    }

    /// The look of text with `attr` in a block of `kind`.
    fn span_style(attr: &Attr, kind: BlockKind, theme: &Theme, mono: Option<&Font>) -> SpanStyle {
        let p = &theme.palette;
        let mut st = SpanStyle::new();
        match kind {
            BlockKind::Heading(n) => {
                let scale = match n {
                    1 => 1.75,
                    2 => 1.45,
                    _ => 1.2,
                };
                st = st.size(theme.font_size * scale).bold();
            }
            BlockKind::Quote => st = st.color(p.text_muted).italic(),
            BlockKind::CodeBlock => {
                if let Some(f) = mono {
                    st = st.font(f.clone());
                }
                st = st.color(p.success);
            }
            _ => {}
        }
        let m = attr.marks;
        if m.contains(Marks::BOLD) {
            st = st.bold();
        }
        if m.contains(Marks::ITALIC) {
            st = st.italic();
        }
        if m.contains(Marks::CODE) {
            if let Some(f) = mono {
                st = st.font(f.clone());
            }
            st = st.color(p.success);
        }
        let mut deco = Decoration::NONE;
        if m.contains(Marks::UNDERLINE) || attr.link.is_some() {
            deco = deco.underline(UnderlineStyle::Single);
        }
        if m.contains(Marks::STRIKETHROUGH) {
            deco = deco.strikethrough();
        }
        if !deco.is_none() {
            st = st.decorate(deco);
        }
        if attr.link.is_some() {
            st = st.color(p.primary);
        }
        st
    }

    /// The text leaf the widget shows: the document as rich text (prefixes,
    /// composition and all), with its caret, selection and composition.
    fn leaf(&self, theme: &Theme) -> Element {
        let maps = self.line_maps();
        let needs_mono = self.doc.kinds.contains(&BlockKind::CodeBlock)
            || self
                .doc
                .spans
                .0
                .iter()
                .any(|r| r.attr.marks.contains(Marks::CODE));
        let mono = if needs_mono { mono_font() } else { None };
        let c = self.nav.caret;
        let preedit = self.nav.preedit.as_ref();
        let selected = self.selected_image();
        let mut rt = RichText::new();
        for (i, m) in maps.iter().enumerate() {
            if i > 0 {
                rt = rt.push("\n");
            }
            let kind = self.doc.kinds[i];
            if !m.prefix.is_empty() {
                let mut st = SpanStyle::new().color(theme.palette.text_muted);
                if kind == BlockKind::NumberedItem || kind == BlockKind::BulletItem {
                    st = st.color(theme.palette.primary);
                }
                rt = rt.styled(&m.prefix, st);
            }
            // The line's runs, the composition spliced in at the caret.
            let mut segs: Vec<(Range<usize>, Attr)> = Vec::new();
            for (r, a) in self.doc.spans.ranges() {
                let (s, e) = (r.start.max(m.model.start), r.end.min(m.model.end));
                if s >= e {
                    continue;
                }
                if s < c && c < e && m.preedit > 0 {
                    segs.push((s..c, a.clone()));
                    segs.push((c..e, a.clone()));
                } else {
                    segs.push((s..e, a.clone()));
                }
            }
            let mut composed = false;
            let compose = |rt: RichText, composed: &mut bool| -> RichText {
                match preedit {
                    Some(p) if m.preedit > 0 && !*composed => {
                        *composed = true;
                        let style =
                            Self::span_style(&self.typing_attr(), kind, theme, mono.as_ref());
                        rt.styled(&p.text, style)
                    }
                    _ => rt,
                }
            };
            for (r, a) in segs {
                if r.start == c {
                    rt = compose(rt, &mut composed);
                }
                let style = Self::span_style(&a, kind, theme, mono.as_ref());
                match &a.image {
                    Some(image) => {
                        for (i, ch) in self.doc.text[r.clone()].char_indices() {
                            if ch != OBJECT {
                                continue;
                            }
                            let frame =
                                (selected == Some(r.start + i)).then_some(theme.palette.primary);
                            rt = match image.placement {
                                ImagePlacement::Inline => {
                                    rt.icon_styled(image_icon(image, theme, frame), style.clone())
                                }
                                _ => rt.float(float_image(image, theme, frame)),
                            };
                        }
                    }
                    None => rt = rt.styled(&self.doc.text[r], style),
                }
            }
            if c == m.model.end {
                rt = compose(rt, &mut composed);
            }
        }
        if rt.is_empty() {
            return super::field_text(String::new(), theme, theme.palette.text)
                .wrap()
                .grow(1.0)
                .shrink(1.0)
                .caret(0);
        }
        let (caret, selection, preedit_range) = match preedit {
            Some(p) => {
                let at = self.model_to_shown(&maps, c);
                let (caret, sel) = match p.cursor {
                    Some((s, e)) if e > s => (at + e, Some((at + s, at + e))),
                    Some((_, e)) => (at + e, None),
                    None => (at + p.text.len(), None),
                };
                (caret, sel, Some((at, at + p.text.len())))
            }
            None => {
                let sel = self
                    .selection()
                    .map(|(s, e)| (self.model_to_shown(&maps, s), self.model_to_shown(&maps, e)));
                (self.model_to_shown(&maps, c), sel, None)
            }
        };
        Element::rich_text(rt, theme.font_size, theme.palette.text)
            .direction(self.nav.direction())
            .wrap()
            .grow(1.0)
            .shrink(1.0)
            .caret(caret)
            .selection(selection)
            .preedit(preedit_range)
    }
}

/// A gray square for an image that isn't loaded.
fn placeholder_image() -> Arc<Pixmap> {
    thread_local! {
        static PLACEHOLDER: Arc<Pixmap> = {
            let mut px = Pixmap::new(PhysicalSize::new(16, 16));
            for chunk in px.as_bytes_mut().chunks_mut(4) {
                chunk.copy_from_slice(&[0x99, 0x99, 0x99, 0xff]);
            }
            Arc::new(px)
        };
    }
    PLACEHOLDER.with(Arc::clone)
}

/// The picture an image shows (a placeholder while not loaded) and its
/// natural width: a pixel of the image per logical pixel, the placeholder
/// as wide as the text is tall.
fn image_source(image: &RichImage, theme: &Theme) -> (Arc<Pixmap>, f64) {
    match &image.pixmap {
        Some(px) => (px.clone(), f64::from(px.size().width)),
        None => (placeholder_image(), theme.font_size),
    }
}

/// How an image shows in the line: as wide as its [`ImageSize`] says of the
/// text area's width, bounded (by its natural width too, unless it may grow,
/// and by the text area's), with `frame` when it is selected. Its
/// alternative text is U+FFFC, so the accessible text matches the text the
/// caret indexes.
fn image_icon(image: &RichImage, theme: &Theme, frame: Option<crate::render::Color>) -> InlineIcon {
    let (px, natural) = image_source(image, theme);
    let icon = InlineIcon::new(px)
        .relative_width(image.relative_width(natural))
        .alt(OBJECT.to_string());
    match frame {
        Some(c) => icon.frame(c),
        None => icon,
    }
}

/// How a floating image shows: sized as [`image_icon`] says, placed per its
/// [`ImagePlacement`], the text keeping 6 px off its opaque pixels on the
/// sides its [`ImageWrap`] says.
fn float_image(
    image: &RichImage,
    theme: &Theme,
    frame: Option<crate::render::Color>,
) -> FloatImage {
    let (px, natural) = image_source(image, theme);
    let f = FloatImage::new(px)
        .relative_width(image.relative_width(natural))
        .alpha(0.5)
        .margin(6.0)
        .alt(OBJECT.to_string());
    let f = match image.placement {
        ImagePlacement::Left => f.left(),
        ImagePlacement::Center => f.center(),
        ImagePlacement::Free { x, y } => f.positioned(Point::new(x / 100.0, y / 100.0)),
        _ => f.right(),
    };
    let centered = matches!(
        image.placement,
        ImagePlacement::Center | ImagePlacement::Free { .. }
    );
    let f = match image.wrap {
        ImageWrap::None => f.wrap(WrapSide::Clear),
        ImageWrap::Largest if centered => f.wrap(WrapSide::Largest),
        _ => f,
    };
    match frame {
        Some(c) => f.frame(c),
        None => f,
    }
}

/// Whether `ev` is the platform's command modifier with `c` (Ctrl+B on
/// Windows and Linux, Cmd+B on macOS).
fn command_key(ev: &KeyEvent) -> Option<char> {
    if ev.state != ButtonState::Pressed {
        return None;
    }
    let m = ev.modifiers;
    let cmd = if cfg!(target_os = "macos") {
        m.meta && !m.ctrl
    } else {
        m.ctrl && !m.meta
    };
    if !cmd || m.alt {
        return None;
    }
    match ev.logical {
        KeyCode::Char(c) => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

/// A rich text editor editing `editor`, the [`RichEditor`] that `field`
/// returns from the app state: everything a [`text_area`](super::text_area)
/// does (the caret and selection by keyboard and pointer, word movement,
/// undo and redo, input methods, right-to-left text), plus paragraphs,
/// headings, lists, quotes and code blocks, bold, italic, underline,
/// strikethrough, inline code, links and images. A click on an image selects
/// it (framed, with corner handles); dragging a handle resizes it, dragging a
/// free image moves it.
///
/// Keys: Ctrl/Cmd+B, I and U toggle bold, italic and underline; Enter splits
/// a block (on an empty list item, it ends the list); Backspace at a block's
/// start turns it into a paragraph, or joins a paragraph to the block before.
/// Copying puts plain text and HTML on the clipboard; pasting takes HTML,
/// an image or text. Give it a hint with
/// [`placeholder`](Element::placeholder), and react to edits with
/// [`on_change`](Element::on_change).
///
/// It grows with its content from a minimum of 160 logical px (put it in a
/// [`scroll`](super::scroll) for long documents); default width 480.
///
/// ```
/// use stipple::prelude::*;
/// use stipple::widgets::{Marks, RichEditor, rich_editor};
/// struct Doc { editor: RichEditor }
/// fn view(state: &Doc, cx: &mut Cx<Doc>) -> Element {
///     let bold = ToolButton::new("B")
///         .selected(state.editor.marks().contains(Marks::BOLD))
///         .view(cx)
///         .keeps_focus()
///         .on_tap(cx, |s| s.editor.toggle_mark(Marks::BOLD));
///     column(vec![
///         toolbar(cx).children(vec![bold]),
///         rich_editor(cx, &state.editor, |s| &mut s.editor).placeholder("Write…"),
///     ])
/// }
/// # let theme = Theme::light();
/// # let _ = view(&Doc { editor: RichEditor::new() }, &mut Cx::new(&theme));
/// ```
pub fn rich_editor<S: 'static>(
    cx: &mut Cx<S>,
    editor: &RichEditor,
    field: impl Fn(&mut S) -> &mut RichEditor + Copy + 'static,
) -> Element {
    let theme = *cx.theme();
    let leaf = editor.leaf(&theme);
    let el = Element::stack(Axis::Horizontal, vec![leaf])
        .clip()
        .fill(theme.palette.surface)
        .radius(theme.radius)
        .border(theme.palette.border, 1.0)
        .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
        .align(Align::Start, Align::Start)
        .default_width(480.0)
        .min_height(160.0)
        .native_text_frame(&theme);
    let (mut el, id) = super::text_input::field_keys(cx, el, false, move |s, k| field(s).apply(k));
    el.input_hints().multiline = true;
    el.semantics.get_or_insert_with(Default::default).rich_paste = true;
    let hooks = cx.field_hooks(id);
    el.on_raw_key(cx, move |s, ev| {
        let mark = match command_key(ev) {
            Some('b') => Marks::BOLD,
            Some('i') => Marks::ITALIC,
            Some('u') => Marks::UNDERLINE,
            _ => return Handled::No,
        };
        let ed = field(s);
        let revision = ed.revision();
        ed.toggle_mark(mark);
        if ed.revision() != revision
            && let Some(h) = &hooks
        {
            h.changed(s);
        }
        Handled::Yes
    })
    .on_text_hit(cx, move |s, hit, extend| {
        let ed = field(s);
        if extend {
            if ed.drag_image(hit) {
                return;
            }
        } else if ed.press_image(hit) {
            return;
        }
        let at = ed.shown_to_model(hit.index);
        ed.nav.preedit = None;
        if extend {
            ed.with_nav(|b| b.extend_to(at));
        } else {
            ed.with_nav(|b| b.place_caret(at));
        }
        ed.pending = None;
        ed.history.open = None;
    })
}
