//! HTML export and import: semantic tags out, and a tolerant reader of the
//! same subset plus the simple tags (and inline styles) other editors and
//! browsers put on the clipboard.

use std::sync::Arc;

use super::inline::{Event, Tag, events};
use super::model::{
    Attr, BlockKind, DocBuilder, ImageFloat, Marks, Piece, RichDocument, RichImage,
};

/// `text` with `&`, `<` and `>` escaped (and `"` too when `attr`).
pub(crate) fn escape(text: &str, attr: bool, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            c => out.push(c),
        }
    }
}

fn mark_tag(m: Marks) -> &'static str {
    match m {
        Marks::BOLD => "strong",
        Marks::ITALIC => "em",
        Marks::STRIKETHROUGH => "s",
        Marks::UNDERLINE => "u",
        _ => "code",
    }
}

/// Spaces HTML would collapse (at a block's ends, or after another space)
/// as no-break spaces, so they survive.
fn keep_spaces(pieces: &[Piece]) -> Vec<Piece> {
    let total: usize = pieces
        .iter()
        .map(|p| match p {
            Piece::Text(t, _) => t.chars().count(),
            Piece::Image(..) => 1,
        })
        .sum();
    let mut i = 0;
    let mut prev_space = true;
    pieces
        .iter()
        .map(|p| match p {
            Piece::Text(t, a) => {
                let mut s = String::with_capacity(t.len());
                for c in t.chars() {
                    i += 1;
                    if c == ' ' && (prev_space || i == total) {
                        s.push('\u{a0}');
                    } else {
                        s.push(c);
                    }
                    prev_space = c == ' ';
                }
                Piece::Text(s, a.clone())
            }
            Piece::Image(..) => {
                i += 1;
                prev_space = false;
                p.clone()
            }
        })
        .collect()
}

/// A line's inline content as HTML.
fn inline_html(pieces: &[Piece], out: &mut String) {
    let pieces = keep_spaces(pieces);
    for ev in events(&pieces) {
        match ev {
            Event::Open(Tag::Link(url)) => {
                out.push_str("<a href=\"");
                escape(&url, true, out);
                out.push_str("\">");
            }
            Event::Close(Tag::Link(_)) => out.push_str("</a>"),
            Event::Open(Tag::Mark(m)) => {
                out.push('<');
                out.push_str(mark_tag(m));
                out.push('>');
            }
            Event::Close(Tag::Mark(m)) => {
                out.push_str("</");
                out.push_str(mark_tag(m));
                out.push('>');
            }
            Event::Text(t) => escape(t, false, out),
            Event::Image(i) => {
                out.push_str("<img src=\"");
                escape(&i.src, true, out);
                out.push_str("\" alt=\"");
                escape(&i.alt, true, out);
                out.push('"');
                if let Some(side) = i.float.css() {
                    out.push_str(" style=\"float: ");
                    out.push_str(side);
                    out.push('"');
                }
                out.push('>');
            }
        }
    }
}

/// The plain text of a code line (images dropped).
fn code_text(pieces: &[Piece]) -> String {
    pieces
        .iter()
        .filter_map(|p| match p {
            Piece::Text(t, _) => Some(t.as_str()),
            Piece::Image(..) => None,
        })
        .collect()
}

/// `doc` as HTML: one element per line (`<p>`, `<h1>`…, a `<ul>` or `<ol>`
/// around each run of list items, a `<blockquote>` around a run of quote
/// paragraphs, a `<pre><code>` around a run of code lines).
pub(crate) fn to_html(doc: &RichDocument) -> String {
    let mut out = String::new();
    let kinds = &doc.kinds;
    let mut line = 0;
    while line < kinds.len() {
        let kind = kinds[line];
        let mut end = line + 1;
        if !matches!(kind, BlockKind::Paragraph | BlockKind::Heading(_)) {
            while end < kinds.len() && kinds[end] == kind {
                end += 1;
            }
        }
        let body = |l: usize, out: &mut String| {
            let pieces = doc.pieces(l);
            if pieces.is_empty() {
                out.push_str("<br>");
            } else {
                inline_html(&pieces, out);
            }
        };
        match kind {
            BlockKind::Paragraph => {
                out.push_str("<p>");
                body(line, &mut out);
                out.push_str("</p>\n");
            }
            BlockKind::Heading(n) => {
                let n = n.clamp(1, 3);
                out.push_str(&format!("<h{n}>"));
                body(line, &mut out);
                out.push_str(&format!("</h{n}>\n"));
            }
            BlockKind::BulletItem | BlockKind::NumberedItem => {
                let list = if kind == BlockKind::BulletItem {
                    "ul"
                } else {
                    "ol"
                };
                out.push_str(&format!("<{list}>\n"));
                for l in line..end {
                    out.push_str("<li>");
                    body(l, &mut out);
                    out.push_str("</li>\n");
                }
                out.push_str(&format!("</{list}>\n"));
            }
            BlockKind::Quote => {
                out.push_str("<blockquote>\n");
                for l in line..end {
                    out.push_str("<p>");
                    body(l, &mut out);
                    out.push_str("</p>\n");
                }
                out.push_str("</blockquote>\n");
            }
            BlockKind::CodeBlock => {
                out.push_str("<pre><code>");
                for l in line..end {
                    if l > line {
                        out.push('\n');
                    }
                    escape(&code_text(&doc.pieces(l)), false, &mut out);
                }
                out.push_str("</code></pre>\n");
            }
        }
        line = end;
    }
    out
}

// --- Reading ---

/// A tag in HTML source.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HtmlTag {
    /// Lower-cased.
    pub name: String,
    pub closing: bool,
    /// Names lower-cased, values with entities decoded.
    pub attrs: Vec<(String, String)>,
}

impl HtmlTag {
    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// What starts at the `<` of `s`: a tag and its length, or `None` when it
/// isn't one (a lone `<`).
pub(crate) fn parse_tag(s: &str) -> Option<(HtmlTag, usize)> {
    let b = s.as_bytes();
    if b.first() != Some(&b'<') {
        return None;
    }
    let mut i = 1;
    let closing = b.get(i) == Some(&b'/');
    if closing {
        i += 1;
    }
    if !b.get(i).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let start = i;
    while b
        .get(i)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b':')
    {
        i += 1;
    }
    let name = s[start..i].to_ascii_lowercase();
    let mut attrs = Vec::new();
    loop {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        match b.get(i)? {
            b'>' => {
                return Some((
                    HtmlTag {
                        name,
                        closing,
                        attrs,
                    },
                    i + 1,
                ));
            }
            b'/' => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let a = i;
        while b
            .get(i)
            .is_some_and(|c| !c.is_ascii_whitespace() && !b"=>/\"'".contains(c))
        {
            i += 1;
        }
        if a == i {
            return None;
        }
        let attr = s[a..i].to_ascii_lowercase();
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let mut value = String::new();
        if b.get(i) == Some(&b'=') {
            i += 1;
            while b.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            match b.get(i)? {
                q @ (b'"' | b'\'') => {
                    let end = s[i + 1..].find(*q as char)? + i + 1;
                    value = decode_entities(&s[i + 1..end]);
                    i = end + 1;
                }
                _ => {
                    let v = i;
                    while b
                        .get(i)
                        .is_some_and(|c| !c.is_ascii_whitespace() && *c != b'>')
                    {
                        i += 1;
                    }
                    value = decode_entities(&s[v..i]);
                }
            }
        }
        attrs.push((attr, value));
    }
}

/// The character a named or numeric entity at the start of `s` (after its
/// `&`) stands for, and the entity's length with its `;`.
pub(crate) fn parse_entity(s: &str) -> Option<(char, usize)> {
    let end = s.find(';')?;
    if end == 0 || end > 32 {
        return None;
    }
    let name = &s[..end];
    let c = if let Some(num) = name.strip_prefix('#') {
        let n = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        char::from_u32(n)
            .filter(|c| *c != '\0')
            .unwrap_or('\u{FFFD}')
    } else {
        match name {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            "nbsp" => '\u{a0}',
            "copy" => '©',
            "reg" => '®',
            "trade" => '™',
            "hellip" => '…',
            "mdash" => '—',
            "ndash" => '–',
            "lsquo" => '‘',
            "rsquo" => '’',
            "ldquo" => '“',
            "rdquo" => '”',
            "laquo" => '«',
            "raquo" => '»',
            "bull" => '•',
            "middot" => '·',
            "times" => '×',
            "divide" => '÷',
            "deg" => '°',
            "euro" => '€',
            "pound" => '£',
            "yen" => '¥',
            "cent" => '¢',
            "sect" => '§',
            "para" => '¶',
            "shy" => '\u{ad}',
            _ => return None,
        }
    };
    Some((c, end + 1))
}

/// `s` with its character references decoded.
pub(crate) fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        match parse_entity(&rest[i + 1..]) {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[i + 1 + len..];
            }
            None => {
                out.push('&');
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// An element open while reading, and what it does to the text inside.
#[derive(Debug)]
struct Open {
    name: String,
    add: Marks,
    remove: Marks,
    link: Option<Arc<str>>,
}

/// Elements whose content isn't shown.
const HIDDEN: [&str; 7] = [
    "script", "style", "head", "title", "template", "noscript", "iframe",
];

/// Elements that end a block where they open or close.
const BLOCKS: [&str; 33] = [
    "p",
    "div",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "li",
    "ul",
    "ol",
    "blockquote",
    "pre",
    "table",
    "tr",
    "td",
    "th",
    "thead",
    "tbody",
    "section",
    "article",
    "header",
    "footer",
    "nav",
    "aside",
    "main",
    "figure",
    "figcaption",
    "dl",
    "dt",
    "dd",
    "hr",
    "address",
];

/// Elements with no closing tag.
const VOID: [&str; 12] = [
    "img", "br", "hr", "meta", "link", "input", "wbr", "col", "area", "base", "source", "embed",
];

/// The marks a `style` attribute adds and removes.
fn style_marks(style: &str) -> (Marks, Marks) {
    let (mut add, mut remove) = (Marks::NONE, Marks::NONE);
    for decl in style.split(';') {
        let Some((prop, value)) = decl.split_once(':') else {
            continue;
        };
        let value = value.trim().to_ascii_lowercase();
        match prop.trim().to_ascii_lowercase().as_str() {
            "font-weight" => {
                let heavy = value == "bold"
                    || value == "bolder"
                    || value.parse::<u32>().is_ok_and(|w| w >= 600);
                if heavy {
                    add |= Marks::BOLD;
                } else {
                    remove |= Marks::BOLD;
                }
            }
            "font-style" => {
                if value.starts_with("italic") || value.starts_with("oblique") {
                    add |= Marks::ITALIC;
                } else {
                    remove |= Marks::ITALIC;
                }
            }
            "text-decoration" | "text-decoration-line" => {
                if value.contains("underline") {
                    add |= Marks::UNDERLINE;
                }
                if value.contains("line-through") {
                    add |= Marks::STRIKETHROUGH;
                }
                if value == "none" {
                    remove = remove.with(Marks::UNDERLINE).with(Marks::STRIKETHROUGH);
                }
            }
            "font-family" if value.contains("monospace") || value.contains("courier") => {
                add |= Marks::CODE;
            }
            _ => {}
        }
    }
    (add, remove)
}

/// Reads HTML into a document.
struct Reader {
    out: DocBuilder,
    stack: Vec<Open>,
    /// Whether the current block has been started.
    in_block: bool,
    hidden: usize,
    decode_images: bool,
    /// A collapsed space waiting for content after it (a block's trailing
    /// white space is dropped), in the look it had.
    space: Option<Attr>,
}

impl Reader {
    /// The block kind the open elements make.
    fn kind(&self) -> BlockKind {
        for (i, open) in self.stack.iter().enumerate().rev() {
            match open.name.as_str() {
                "pre" => return BlockKind::CodeBlock,
                "h1" => return BlockKind::Heading(1),
                "h2" => return BlockKind::Heading(2),
                "h3" | "h4" | "h5" | "h6" => return BlockKind::Heading(3),
                "li" => {
                    let ordered = self.stack[..i]
                        .iter()
                        .rev()
                        .find(|o| o.name == "ul" || o.name == "ol")
                        .is_some_and(|o| o.name == "ol");
                    return if ordered {
                        BlockKind::NumberedItem
                    } else {
                        BlockKind::BulletItem
                    };
                }
                "blockquote" => return BlockKind::Quote,
                _ => {}
            }
        }
        BlockKind::Paragraph
    }

    fn in_pre(&self) -> bool {
        self.stack.iter().any(|o| o.name == "pre")
    }

    /// The look of text here.
    fn attr(&self) -> Attr {
        let mut marks = Marks::NONE;
        let mut link = None;
        for o in &self.stack {
            marks = marks.with(o.add).without(o.remove);
            if o.link.is_some() {
                link = o.link.clone();
            }
        }
        if self.in_pre() {
            marks = marks.without(Marks::CODE);
        }
        Attr {
            marks,
            link,
            image: None,
        }
    }

    fn ensure_block(&mut self) {
        if !self.in_block {
            self.out.start_block(self.kind());
            self.in_block = true;
        }
    }

    fn end_block(&mut self) {
        self.space = None;
        self.in_block = false;
    }

    /// Put out the collapsed space before more content.
    fn flush_space(&mut self) {
        if let Some(attr) = self.space.take() {
            self.out.push_text(" ", &attr);
        }
    }

    fn text(&mut self, raw: &str) {
        if self.hidden > 0 {
            return;
        }
        let text = decode_entities(raw);
        if self.in_pre() {
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    // Each line of a code block is a block.
                    self.ensure_block();
                    self.in_block = false;
                }
                if !line.is_empty() {
                    self.ensure_block();
                    let attr = self.attr();
                    self.out.push_text(&line.replace('\u{a0}', " "), &attr);
                }
            }
            return;
        }
        // Collapse white space, as HTML shows it: none at a block's start
        // or end, one space for each run between. A no-break space stays.
        let attr = self.attr();
        let mut run = String::new();
        for c in text.chars() {
            if c.is_ascii_whitespace() {
                if !run.is_empty() {
                    self.out.push_text(&std::mem::take(&mut run), &attr);
                }
                if self.in_block && !self.out.current_text().is_empty() && self.space.is_none() {
                    self.space = Some(attr.clone());
                }
            } else {
                if run.is_empty() {
                    self.ensure_block();
                    self.flush_space();
                }
                run.push(if c == '\u{a0}' { ' ' } else { c });
            }
        }
        if !run.is_empty() {
            self.out.push_text(&run, &attr);
        }
    }

    fn tag(&mut self, tag: HtmlTag) {
        let name = tag.name.as_str();
        if HIDDEN.contains(&name) {
            if tag.closing {
                self.hidden = self.hidden.saturating_sub(1);
            } else {
                self.hidden += 1;
            }
            return;
        }
        if self.hidden > 0 {
            return;
        }
        if tag.closing {
            if BLOCKS.contains(&name) {
                self.end_block();
            }
            if let Some(at) = self.stack.iter().rposition(|o| o.name == name) {
                self.stack.truncate(at);
            }
            return;
        }
        match name {
            "br" => {
                if self.in_pre() {
                    self.text("\n");
                } else {
                    self.ensure_block();
                    self.end_block();
                }
                return;
            }
            "img" => {
                let src = tag.attr("src").unwrap_or_default().to_string();
                let pixmap = if self.decode_images {
                    super::model::load_source(&src, false).map(Arc::new)
                } else {
                    None
                };
                let alt = tag.attr("alt").unwrap_or_default();
                let float = ImageFloat::from_attrs(tag.attr("style"), tag.attr("align"));
                let image = RichImage::new(src, pixmap).alt(alt).floated(float);
                self.ensure_block();
                self.flush_space();
                let attr = self.attr();
                self.out.push_image(Arc::new(image), &attr);
                return;
            }
            _ => {}
        }
        if BLOCKS.contains(&name) {
            self.end_block();
        }
        if VOID.contains(&name) {
            return;
        }
        let mut add = match name {
            "b" | "strong" => Marks::BOLD,
            "i" | "em" | "cite" | "var" | "dfn" => Marks::ITALIC,
            "u" | "ins" => Marks::UNDERLINE,
            "s" | "del" | "strike" => Marks::STRIKETHROUGH,
            "code" | "kbd" | "tt" | "samp" => Marks::CODE,
            _ => Marks::NONE,
        };
        let mut remove = Marks::NONE;
        if let Some(style) = tag.attr("style") {
            let (a, r) = style_marks(style);
            add = add.without(r).with(a);
            remove = r.without(a);
        }
        // Headings are bold on their own: their text needn't be.
        if name.len() == 2 && name.starts_with('h') && name.as_bytes()[1].is_ascii_digit() {
            remove |= Marks::BOLD;
        }
        let link = (name == "a")
            .then(|| tag.attr("href"))
            .flatten()
            .filter(|h| !h.is_empty())
            .map(Arc::from);
        self.stack.push(Open {
            name: tag.name,
            add,
            remove,
            link,
        });
    }
}

/// Read `html` (a fragment or a whole page) into a document. `decode_images`
/// decodes images with `data:` URIs.
pub(crate) fn from_html(html: &str, decode_images: bool) -> RichDocument {
    let mut r = Reader {
        out: DocBuilder::new(),
        stack: Vec::new(),
        in_block: false,
        hidden: 0,
        decode_images,
        space: None,
    };
    let mut rest = html;
    let mut after_pre = false;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            r.text(rest);
            break;
        };
        let mut text = &rest[..lt];
        // A newline right after <pre> isn't content.
        if after_pre {
            text = text
                .strip_prefix("\r\n")
                .or_else(|| text.strip_prefix('\n'))
                .unwrap_or(text);
            after_pre = false;
        }
        if !text.is_empty() {
            r.text(text);
        }
        rest = &rest[lt..];
        if let Some(body) = rest.strip_prefix("<!--") {
            rest = body.find("-->").map_or("", |e| &body[e + 3..]);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = rest.find('>').map_or("", |e| &rest[e + 1..]);
            continue;
        }
        match parse_tag(rest) {
            Some((tag, len)) => {
                after_pre = tag.name == "pre" && !tag.closing;
                r.tag(tag);
                rest = &rest[len..];
            }
            None => {
                r.text("&lt;");
                rest = &rest[1..];
            }
        }
    }
    r.end_block();
    r.out.finish()
}
