//! The rich text document: blocks (paragraphs, headings, list items, quotes,
//! code lines) holding text with marks, links and inline images.
//!
//! Stored flat: the blocks' text joined by `\n` (each image one U+FFFC), the
//! runs of attributes over it, and one [`BlockKind`] per line. Byte indices
//! into that text are what the editor's caret and selection are.

use std::ops::Range;
use std::sync::Arc;

use crate::render::Pixmap;

/// The character an image stands as in the text.
pub(crate) const OBJECT: char = '\u{FFFC}';

/// A set of inline marks: bold, italic, underline, strikethrough and inline
/// code. Combine them with `|`.
///
/// ```
/// use stipple::widgets::Marks;
/// let m = Marks::BOLD | Marks::ITALIC;
/// assert!(m.contains(Marks::BOLD) && !m.contains(Marks::CODE));
/// assert_eq!(m.without(Marks::BOLD), Marks::ITALIC);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Marks(u8);

impl Marks {
    /// No mark.
    pub const NONE: Marks = Marks(0);
    /// Bold (`**` in Markdown, `<strong>` in HTML).
    pub const BOLD: Marks = Marks(1);
    /// Italic (`*`, `<em>`).
    pub const ITALIC: Marks = Marks(2);
    /// Underlined (`<u>` in both: Markdown has no underline of its own).
    pub const UNDERLINE: Marks = Marks(4);
    /// Struck through (`~~`, `<s>`).
    pub const STRIKETHROUGH: Marks = Marks(8);
    /// Inline code (`` ` ``, `<code>`), in a monospaced font.
    pub const CODE: Marks = Marks(16);

    /// Every mark at once.
    pub(crate) const EVERY: Marks = Marks(31);
    /// Every mark, in the order they nest when exported (outermost first).
    pub(crate) const ALL: [Marks; 5] = [
        Marks::BOLD,
        Marks::ITALIC,
        Marks::STRIKETHROUGH,
        Marks::UNDERLINE,
        Marks::CODE,
    ];

    /// Whether every mark of `other` is in this set.
    pub const fn contains(self, other: Marks) -> bool {
        self.0 & other.0 == other.0
    }

    /// This set and `other`'s marks.
    pub const fn with(self, other: Marks) -> Marks {
        Marks(self.0 | other.0)
    }

    /// This set without `other`'s marks.
    pub const fn without(self, other: Marks) -> Marks {
        Marks(self.0 & !other.0)
    }

    /// The marks in both sets.
    pub const fn intersect(self, other: Marks) -> Marks {
        Marks(self.0 & other.0)
    }

    /// Whether the set is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Marks {
    type Output = Marks;
    fn bitor(self, other: Marks) -> Marks {
        self.with(other)
    }
}

impl std::ops::BitOrAssign for Marks {
    fn bitor_assign(&mut self, other: Marks) {
        *self = self.with(other);
    }
}

/// What a block of a [`RichDocument`] is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BlockKind {
    /// A paragraph.
    #[default]
    Paragraph,
    /// A heading of level 1 to 3 (`#` to `###`, `<h1>` to `<h3>`).
    Heading(u8),
    /// An item of a bulleted list. Consecutive items make one list.
    BulletItem,
    /// An item of a numbered list, numbered from 1 within its run of
    /// consecutive items.
    NumberedItem,
    /// A paragraph of a block quote. Consecutive ones make one quote.
    Quote,
    /// A line of a code block, kept verbatim (marks aren't exported in
    /// code). Consecutive lines make one block.
    CodeBlock,
}

impl BlockKind {
    /// A heading of `level`, clamped to 1..=3.
    pub fn heading(level: u8) -> BlockKind {
        BlockKind::Heading(level.clamp(1, 3))
    }

    /// Whether it is a list item.
    pub fn is_list(self) -> bool {
        matches!(self, BlockKind::BulletItem | BlockKind::NumberedItem)
    }

    /// The kind of the block Enter starts after this one: the same for list
    /// items, quotes and code, a paragraph after a heading.
    pub(crate) fn next(self) -> BlockKind {
        match self {
            BlockKind::Heading(_) => BlockKind::Paragraph,
            other => other,
        }
    }
}

/// An image in a [`RichDocument`]: where it comes from (`src`: a file path,
/// a URL or a `data:` URI, exported as is), its alternative text, and the
/// decoded picture the editor shows (`None` until loaded: the editor then
/// shows a placeholder).
///
/// ```
/// use stipple::geometry::PhysicalSize;
/// use stipple::render::Pixmap;
/// use stipple::widgets::RichImage;
/// let dot = RichImage::from_pixmap(Pixmap::new(PhysicalSize::new(2, 2))).alt("a dot");
/// assert!(dot.src.starts_with("data:image/png;base64,"));
/// assert!(RichImage::load(&dot.src).pixmap.is_some());
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RichImage {
    /// Where the image comes from: what Markdown's `![alt](src)` and HTML's
    /// `<img src>` export.
    pub src: String,
    /// Its alternative text.
    pub alt: String,
    /// The decoded image, if loaded.
    pub pixmap: Option<Arc<Pixmap>>,
}

impl RichImage {
    /// The image at `src` showing `pixmap`.
    pub fn new(src: impl Into<String>, pixmap: Option<Arc<Pixmap>>) -> Self {
        Self {
            src: src.into(),
            alt: String::new(),
            pixmap,
        }
    }

    /// `pixmap`, its `src` a `data:image/png;base64,…` URI of it, so it
    /// exports with the document (a pasted or generated image).
    pub fn from_pixmap(pixmap: Pixmap) -> Self {
        let src = crate::platform::clipboard::format::encode_png(&pixmap)
            .map(|png| format!("data:image/png;base64,{}", base64_encode(&png)))
            .unwrap_or_default();
        Self::new(src, Some(Arc::new(pixmap)))
    }

    /// The image at `src`, decoded when it is a `data:` URI with a base64
    /// image or (not on the web) a readable file path or `file://` URL;
    /// otherwise without a picture.
    pub fn load(src: &str) -> Self {
        Self::new(src, load_source(src, true).map(Arc::new))
    }

    /// With `alt` as its alternative text.
    pub fn alt(mut self, alt: impl Into<String>) -> Self {
        self.alt = alt.into();
        self
    }
}

/// Decode the image at `src`: a base64 `data:` URI, or with `files` a local
/// path or `file://` URL.
pub(crate) fn load_source(src: &str, files: bool) -> Option<Pixmap> {
    if let Some(rest) = src.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',')?;
        if !meta.ends_with(";base64") {
            return None;
        }
        return Pixmap::decode(&base64_decode(data)?).ok();
    }
    if !files || cfg!(target_arch = "wasm32") {
        return None;
    }
    let path = src.strip_prefix("file://").unwrap_or(src);
    if path.contains("://") {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Pixmap::decode(&bytes).ok()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `bytes` in base64, padded.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decode base64 (standard or URL-safe alphabet, padding and whitespace
/// ignored), or `None` on another character.
pub(crate) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = acc << 6 | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// How a run of text looks: its marks, its link, and for an image's U+FFFC
/// the image.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Attr {
    pub marks: Marks,
    pub link: Option<Arc<str>>,
    pub image: Option<Arc<RichImage>>,
}

/// One run of [`Spans`]: `len` bytes looking like `attr`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Run {
    pub len: usize,
    pub attr: Attr,
}

/// The runs of attributes over a text, run-length encoded: no empty runs, and
/// no two neighbors alike.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Spans(pub Vec<Run>);

impl Spans {
    /// Append `len` bytes looking like `attr`.
    pub(crate) fn push(&mut self, len: usize, attr: Attr) {
        if len == 0 {
            return;
        }
        match self.0.last_mut() {
            Some(last) if last.attr == attr => last.len += len,
            _ => self.0.push(Run { len, attr }),
        }
    }

    /// Make a run start at byte `at`; the index of that run.
    fn split(&mut self, at: usize) -> usize {
        let mut pos = 0;
        for i in 0..self.0.len() {
            if pos == at {
                return i;
            }
            let len = self.0[i].len;
            if at < pos + len {
                let attr = self.0[i].attr.clone();
                self.0[i].len = at - pos;
                self.0.insert(
                    i + 1,
                    Run {
                        len: pos + len - at,
                        attr,
                    },
                );
                return i + 1;
            }
            pos += len;
        }
        self.0.len()
    }

    /// Join neighbors alike and drop empty runs.
    fn merge(&mut self) {
        let runs = std::mem::take(&mut self.0);
        for r in runs {
            self.push(r.len, r.attr);
        }
    }

    /// Replace the runs over `range` with `with`.
    pub(crate) fn replace(&mut self, range: Range<usize>, with: Spans) {
        let a = self.split(range.start);
        let b = self.split(range.end);
        self.0.splice(a..b, with.0);
        self.merge();
    }

    /// The runs over `range`.
    pub(crate) fn slice(&self, range: Range<usize>) -> Spans {
        let mut out = Spans::default();
        for (r, attr) in self.ranges() {
            let (s, e) = (r.start.max(range.start), r.end.min(range.end));
            if s < e {
                out.push(e - s, attr.clone());
            }
        }
        out
    }

    /// Change the attributes over `range` with `f`.
    pub(crate) fn update(&mut self, range: Range<usize>, mut f: impl FnMut(&mut Attr)) {
        let a = self.split(range.start);
        let b = self.split(range.end);
        for r in &mut self.0[a..b] {
            f(&mut r.attr);
        }
        self.merge();
    }

    /// The attributes of the byte at `at`.
    pub(crate) fn at(&self, at: usize) -> Option<&Attr> {
        self.ranges().find(|(r, _)| r.contains(&at)).map(|(_, a)| a)
    }

    /// Each run's byte range and attributes.
    pub(crate) fn ranges(&self) -> impl Iterator<Item = (Range<usize>, &Attr)> {
        let mut pos = 0;
        self.0.iter().map(move |r| {
            let start = pos;
            pos += r.len;
            (start..pos, &r.attr)
        })
    }
}

/// One block of a [`RichDocument`], as [`RichDocument::blocks`] lists them.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Block {
    /// What the block is.
    pub kind: BlockKind,
    /// Its content.
    pub inlines: Vec<Inline>,
}

impl Block {
    /// A block of `kind` holding `inlines`.
    pub fn new(kind: BlockKind, inlines: Vec<Inline>) -> Self {
        Self { kind, inlines }
    }
}

/// A piece of a [`Block`]: text with its marks and link, or an image.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Inline {
    /// Text (no line breaks) with `marks`, a link to `link` when set.
    Text {
        text: String,
        marks: Marks,
        link: Option<String>,
    },
    /// An image.
    Image(RichImage),
}

impl Inline {
    /// Plain text.
    pub fn text(text: impl Into<String>) -> Self {
        Self::styled(text, Marks::NONE)
    }

    /// Text with `marks`.
    pub fn styled(text: impl Into<String>, marks: Marks) -> Self {
        Inline::Text {
            text: text.into(),
            marks,
            link: None,
        }
    }

    /// Text linking to `url`.
    pub fn link(text: impl Into<String>, url: impl Into<String>) -> Self {
        Inline::Text {
            text: text.into(),
            marks: Marks::NONE,
            link: Some(url.into()),
        }
    }
}

/// A rich text document: what a [`RichEditor`](super::RichEditor) edits, and
/// what it exports to and imports from Markdown and HTML.
///
/// ```
/// use stipple::widgets::{Block, BlockKind, Inline, Marks, RichDocument};
/// let doc = RichDocument::from_blocks(vec![
///     Block::new(BlockKind::heading(1), vec![Inline::text("Notes")]),
///     Block::new(BlockKind::Paragraph, vec![
///         Inline::text("Some "),
///         Inline::styled("bold", Marks::BOLD),
///         Inline::text(" text."),
///     ]),
/// ]);
/// assert_eq!(doc.to_markdown(), "# Notes\n\nSome **bold** text.\n");
/// assert_eq!(doc.to_html(), "<h1>Notes</h1>\n<p>Some <strong>bold</strong> text.</p>\n");
/// assert_eq!(RichDocument::from_markdown(&doc.to_markdown()), doc);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct RichDocument {
    /// The blocks' text joined by `\n`, each image a U+FFFC.
    pub(crate) text: String,
    pub(crate) spans: Spans,
    /// One per line of `text`.
    pub(crate) kinds: Vec<BlockKind>,
}

impl Default for RichDocument {
    fn default() -> Self {
        Self {
            text: String::new(),
            spans: Spans::default(),
            kinds: vec![BlockKind::Paragraph],
        }
    }
}

impl RichDocument {
    /// An empty document: one empty paragraph.
    pub fn new() -> Self {
        Self::default()
    }

    /// A document of plain paragraphs, one per line of `text`.
    pub fn from_plain_text(text: &str) -> Self {
        let mut b = DocBuilder::new();
        for line in text.lines() {
            b.start_block(BlockKind::Paragraph);
            b.push_text(line, &Attr::default());
        }
        b.finish()
    }

    /// A document of `blocks` (an empty list makes an empty document). Line
    /// breaks in text become spaces.
    pub fn from_blocks(blocks: Vec<Block>) -> Self {
        let mut b = DocBuilder::new();
        for block in blocks {
            b.start_block(block.kind);
            for inline in block.inlines {
                match inline {
                    Inline::Text { text, marks, link } => {
                        let attr = Attr {
                            marks,
                            link: link.map(Arc::from),
                            image: None,
                        };
                        b.push_text(&text, &attr);
                    }
                    Inline::Image(image) => b.push_image(Arc::new(image), &Attr::default()),
                }
            }
        }
        b.finish()
    }

    /// Its blocks, each a run of [`Inline`]s (neighbors that look alike
    /// joined).
    pub fn blocks(&self) -> Vec<Block> {
        (0..self.kinds.len())
            .map(|line| {
                let mut inlines: Vec<Inline> = Vec::new();
                for piece in self.pieces(line) {
                    match piece {
                        Piece::Text(text, attr) => inlines.push(Inline::Text {
                            text,
                            marks: attr.marks,
                            link: attr.link.as_deref().map(str::to_string),
                        }),
                        Piece::Image(image, _) => inlines.push(Inline::Image((*image).clone())),
                    }
                }
                Block::new(self.kinds[line], inlines)
            })
            .collect()
    }

    /// The number of blocks.
    pub fn block_count(&self) -> usize {
        self.kinds.len()
    }

    /// Whether it is a single empty paragraph.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.kinds == [BlockKind::Paragraph]
    }

    /// The text, blocks on lines of their own, list items after a bullet
    /// (`• `) or their number (`1. `), images as their alternative text:
    /// what copying puts on the clipboard as plain text.
    pub fn plain_text(&self) -> String {
        let prefixes = self.prefixes();
        let mut out = String::new();
        for (line, prefix) in prefixes.iter().enumerate() {
            if line > 0 {
                out.push('\n');
            }
            out.push_str(prefix.trim_start());
            for piece in self.pieces(line) {
                match piece {
                    Piece::Text(t, _) => out.push_str(&t),
                    Piece::Image(i, _) => out.push_str(&i.alt),
                }
            }
        }
        out
    }

    /// Decode the images that have no picture yet, from `data:` URIs and
    /// (not on the web) local files: what the Markdown and HTML importers
    /// leave to the caller for files, as reading them is up to the app.
    pub fn load_images(&mut self) {
        for run in &mut self.spans.0 {
            if let Some(image) = &run.attr.image
                && image.pixmap.is_none()
                && let Some(px) = load_source(&image.src, true)
            {
                let mut image = (**image).clone();
                image.pixmap = Some(Arc::new(px));
                run.attr.image = Some(Arc::new(image));
            }
        }
    }

    /// Every image in the document, in order.
    pub fn images(&self) -> Vec<RichImage> {
        let mut out = Vec::new();
        for (r, attr) in self.spans.ranges() {
            if let Some(image) = &attr.image {
                for c in self.text[r].chars() {
                    if c == OBJECT {
                        out.push((**image).clone());
                    }
                }
            }
        }
        out
    }

    /// The line breaks' byte offsets, plus the end.
    pub(crate) fn line_starts(&self) -> Vec<usize> {
        let mut starts = vec![0];
        starts.extend(self.text.match_indices('\n').map(|(i, _)| i + 1));
        starts
    }

    /// The line (block) holding byte `at`.
    pub(crate) fn line_of(&self, at: usize) -> usize {
        self.text.as_bytes()[..at.min(self.text.len())]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
    }

    /// The byte range of `line`'s text (without its `\n`).
    pub(crate) fn line_range(&self, line: usize) -> Range<usize> {
        let mut start = 0;
        for _ in 0..line {
            match self.text[start..].find('\n') {
                Some(i) => start += i + 1,
                None => return self.text.len()..self.text.len(),
            }
        }
        let end = self.text[start..]
            .find('\n')
            .map_or(self.text.len(), |i| start + i);
        start..end
    }

    /// `line`'s content as text runs and images.
    pub(crate) fn pieces(&self, line: usize) -> Vec<Piece> {
        let range = self.line_range(line);
        let mut out = Vec::new();
        for (r, attr) in self.spans.ranges() {
            let (s, e) = (r.start.max(range.start), r.end.min(range.end));
            if s >= e {
                continue;
            }
            let text = &self.text[s..e];
            match &attr.image {
                Some(image) => {
                    let plain = Attr {
                        image: None,
                        ..attr.clone()
                    };
                    for c in text.chars() {
                        if c == OBJECT {
                            out.push(Piece::Image(image.clone(), plain.clone()));
                        }
                    }
                }
                None => match out.last_mut() {
                    Some(Piece::Text(t, a)) if a == attr => t.push_str(text),
                    _ => out.push(Piece::Text(text.to_string(), attr.clone())),
                },
            }
        }
        out
    }

    /// The text before each line as the editor shows it and plain text
    /// copies it: a bullet or a number for list items, a bar for quotes.
    pub(crate) fn prefixes(&self) -> Vec<String> {
        let mut number = 0;
        self.kinds
            .iter()
            .map(|k| {
                if *k == BlockKind::NumberedItem {
                    number += 1;
                } else {
                    number = 0;
                }
                match k {
                    BlockKind::BulletItem => "  • ".to_string(),
                    BlockKind::NumberedItem => format!("  {number}. "),
                    BlockKind::Quote => "│ ".to_string(),
                    _ => String::new(),
                }
            })
            .collect()
    }

    /// The part of the document over `range`, with the kinds of the lines
    /// it touches.
    pub(crate) fn slice(&self, range: Range<usize>) -> RichDocument {
        let (first, last) = (self.line_of(range.start), self.line_of(range.end));
        RichDocument {
            text: self.text[range.clone()].to_string(),
            spans: self.spans.slice(range),
            kinds: self.kinds[first..=last].to_vec(),
        }
    }

    /// Replace `range` with `frag`. The first line keeps its kind (unless
    /// `frag` is several blocks pasted at its start); the lines `frag`
    /// adds take `frag`'s kinds, the last of them also holding the text
    /// after `range`.
    pub(crate) fn replace(&mut self, range: Range<usize>, frag: &RichDocument) {
        let first = self.line_of(range.start);
        let last = self.line_of(range.end);
        let at_start = self.line_range(first).start == range.start;
        self.text.replace_range(range.clone(), &frag.text);
        self.spans.replace(range, frag.spans.clone());
        self.kinds
            .splice(first + 1..=last, frag.kinds[1..].iter().copied());
        if frag.kinds.len() > 1 && at_start {
            self.kinds[first] = frag.kinds[0];
        }
    }
}

/// A piece of a line: text in one look, or an image (with the marks around
/// it).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Piece {
    Text(String, Attr),
    Image(Arc<RichImage>, Attr),
}

/// Builds a [`RichDocument`] block by block, as the importers read it.
#[derive(Debug)]
pub(crate) struct DocBuilder {
    doc: RichDocument,
    /// Where the current block's text starts.
    block_start: usize,
}

impl DocBuilder {
    pub(crate) fn new() -> Self {
        Self {
            doc: RichDocument {
                text: String::new(),
                spans: Spans::default(),
                kinds: Vec::new(),
            },
            block_start: 0,
        }
    }

    /// Start a block of `kind`.
    pub(crate) fn start_block(&mut self, kind: BlockKind) {
        if !self.doc.kinds.is_empty() {
            self.doc.text.push('\n');
            self.doc.spans.push(1, Attr::default());
        }
        self.block_start = self.doc.text.len();
        self.doc.kinds.push(kind);
    }

    /// Whether a block was started.
    pub(crate) fn has_block(&self) -> bool {
        !self.doc.kinds.is_empty()
    }

    /// The current block's text.
    pub(crate) fn current_text(&self) -> &str {
        &self.doc.text[self.block_start..]
    }

    /// Append `text` to the current block, in `attr` (without an image):
    /// line breaks become spaces, U+FFFC is dropped.
    pub(crate) fn push_text(&mut self, text: &str, attr: &Attr) {
        if !self.has_block() {
            self.start_block(BlockKind::Paragraph);
        }
        let clean: String = text
            .chars()
            .filter(|c| *c != OBJECT && *c != '\r')
            .map(|c| if c == '\n' { ' ' } else { c })
            .collect();
        let attr = Attr {
            image: None,
            ..attr.clone()
        };
        self.doc.text.push_str(&clean);
        self.doc.spans.push(clean.len(), attr);
    }

    /// Append `image` to the current block, with `attr`'s marks and link.
    pub(crate) fn push_image(&mut self, image: Arc<RichImage>, attr: &Attr) {
        if !self.has_block() {
            self.start_block(BlockKind::Paragraph);
        }
        self.doc.text.push(OBJECT);
        let attr = Attr {
            image: Some(image),
            ..attr.clone()
        };
        self.doc.spans.push(OBJECT.len_utf8(), attr);
    }

    pub(crate) fn finish(mut self) -> RichDocument {
        if self.doc.kinds.is_empty() {
            self.doc.kinds.push(BlockKind::Paragraph);
        }
        self.doc
    }
}
