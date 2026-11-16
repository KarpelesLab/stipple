//! Markdown export and import (CommonMark, with GitHub's `~~strikethrough~~`).
//!
//! Export escapes every character Markdown could read as syntax, writes
//! underline (which Markdown lacks) as `<u>`, and checks each line by reading
//! it back: where emphasis delimiters would be read differently (`**` next to
//! punctuation, say), that line's marks are written as HTML tags instead,
//! which CommonMark allows inline.
//!
//! Import reads ATX and setext headings, paragraphs, bulleted and numbered
//! lists (nested ones flattened), block quotes, fenced and indented code, and
//! inline emphasis, strikethrough, code spans, links, autolinks, images,
//! backslash escapes, character references and the inline HTML tags the
//! export writes. Raw HTML blocks are read as paragraphs.

use std::sync::Arc;

use super::html::{HtmlTag, parse_entity, parse_tag};
use super::inline::{Event, Tag, events, signature};
use super::model::{
    Attr, BlockKind, DocBuilder, ImageFloat, Marks, OBJECT, Piece, RichDocument, RichImage,
};

// --- Writing ---

/// How a line's marks are written.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Style {
    /// `**`, `*`, `~~` and `` ` ``.
    Markdown,
    /// `<strong>`, `<em>`, `<del>`, with code still in backticks.
    HtmlEmphasis,
    /// Every mark as an HTML tag.
    Html,
}

/// Escape `text` for Markdown. `start`: it opens the line, where `#`, `-`,
/// `+`, `=` and a number followed by `.` or `)` would start a block, and
/// leading spaces would be dropped.
fn escape(text: &str, start: bool, out: &mut String) {
    let mut chars = text.char_indices().peekable();
    let mut leading = start;
    while let Some((i, c)) = chars.next() {
        if leading && c == ' ' {
            out.push_str("&#32;");
            continue;
        }
        if start && i == 0 {
            if matches!(c, '#' | '-' | '+' | '=') {
                out.push('\\');
                out.push(c);
                leading = false;
                continue;
            }
            if c.is_ascii_digit() {
                // A list number: escape the `.` or `)` after the digits.
                let digits = text.bytes().take_while(u8::is_ascii_digit).count();
                if matches!(text.as_bytes().get(digits), Some(b'.' | b')')) {
                    out.push_str(&text[..digits]);
                    out.push('\\');
                    for _ in 0..digits - 1 {
                        chars.next();
                    }
                    leading = false;
                    continue;
                }
            }
        }
        leading = false;
        match c {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '~' | '&' => {
                out.push('\\');
                out.push(c);
            }
            '\u{a0}' => out.push_str("&nbsp;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
}

/// A link destination or image source: in angle brackets when it has spaces
/// or parentheses (or is empty).
fn destination(url: &str, out: &mut String) {
    let bracket = url.is_empty() || url.contains([' ', '(', ')', '\t']);
    if bracket {
        out.push('<');
    }
    for c in url.chars() {
        match c {
            '<' | '>' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("%0A"),
            c => out.push(c),
        }
    }
    if bracket {
        out.push('>');
    }
}

/// A code span holding `code`: fenced by more backticks than any run in it,
/// padded with a space where it starts or ends with a backtick or space.
fn code_span(code: &str, out: &mut String) {
    let longest = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let pad = code.starts_with(['`', ' ']) || code.ends_with(['`', ' ']);
    let pad = pad && !code.chars().all(|c| c == ' ');
    out.push_str(&fence);
    if pad {
        out.push(' ');
    }
    out.push_str(code);
    if pad {
        out.push(' ');
    }
    out.push_str(&fence);
}

/// The pieces with bold, italic and strikethrough taken off the spaces at
/// either end of each marked stretch (`** x**` wouldn't read as bold).
fn trim_marks(pieces: &[Piece]) -> Vec<Piece> {
    // One item per character (or image), to move marks off single spaces.
    let mut atoms: Vec<(Option<char>, Piece)> = Vec::new();
    for p in pieces {
        match p {
            Piece::Text(t, a) => atoms.extend(
                t.chars()
                    .map(|c| (Some(c), Piece::Text(c.to_string(), a.clone()))),
            ),
            Piece::Image(..) => atoms.push((None, p.clone())),
        }
    }
    for m in [Marks::BOLD, Marks::ITALIC, Marks::STRIKETHROUGH] {
        let has =
            |atoms: &Vec<(Option<char>, Piece)>, i: usize| attr_of(&atoms[i].1).marks.contains(m);
        let mut i = 0;
        while i < atoms.len() {
            if !has(&atoms, i) {
                i += 1;
                continue;
            }
            let mut end = i;
            while end < atoms.len() && has(&atoms, end) {
                end += 1;
            }
            let (mut s, mut e) = (i, end);
            while s < e && atoms[s].0.is_some_and(char::is_whitespace) {
                unmark(&mut atoms[s].1, m);
                s += 1;
            }
            while e > s && atoms[e - 1].0.is_some_and(char::is_whitespace) {
                unmark(&mut atoms[e - 1].1, m);
                e -= 1;
            }
            i = end;
        }
    }
    // Join the characters back into runs.
    let mut out: Vec<Piece> = Vec::new();
    for (_, p) in atoms {
        match (out.last_mut(), p) {
            (Some(Piece::Text(t, a)), Piece::Text(c, b)) if *a == b => t.push_str(&c),
            (_, p) => out.push(p),
        }
    }
    out
}

fn attr_of(p: &Piece) -> &Attr {
    match p {
        Piece::Text(_, a) | Piece::Image(_, a) => a,
    }
}

/// Take `m` off `p`'s marks.
fn unmark(p: &mut Piece, m: Marks) {
    let (Piece::Text(_, a) | Piece::Image(_, a)) = p;
    a.marks = a.marks.without(m);
}

/// A line's inline content in Markdown, its marks written per `style`.
fn inline_markdown(pieces: &[Piece], style: Style, out: &mut String) {
    let start = out.len();
    let mut code: Option<String> = None;
    for ev in events(pieces) {
        let at_start = out.len() == start;
        match ev {
            Event::Open(Tag::Link(_)) => out.push('['),
            Event::Close(Tag::Link(url)) => {
                out.push_str("](");
                destination(&url, out);
                out.push(')');
            }
            Event::Open(Tag::Mark(Marks::CODE)) if style != Style::Html => {
                code = Some(String::new());
            }
            Event::Close(Tag::Mark(Marks::CODE)) if style != Style::Html => {
                code_span(&code.take().unwrap_or_default(), out);
            }
            Event::Open(Tag::Mark(m)) => out.push_str(open_mark(m, style)),
            Event::Close(Tag::Mark(m)) => out.push_str(close_mark(m, style)),
            Event::Text(t) => match &mut code {
                Some(c) => c.push_str(t),
                None => escape(t, at_start, out),
            },
            Event::Image(i) if i.float.css().is_some() => {
                // Markdown has no floats: an HTML tag, which it allows.
                out.push_str("<img src=\"");
                super::html::escape(&i.src, true, out);
                out.push_str("\" alt=\"");
                super::html::escape(&i.alt, true, out);
                out.push_str("\" style=\"float: ");
                out.push_str(i.float.css().unwrap_or_default());
                out.push_str("\">");
            }
            Event::Image(i) => {
                out.push_str("![");
                escape(&i.alt, false, out);
                out.push_str("](");
                destination(&i.src, out);
                out.push(')');
            }
        }
    }
    // Trailing spaces would be dropped.
    let trimmed = out.trim_end_matches(' ').len();
    if trimmed < out.len() && trimmed >= start {
        let n = out.len() - trimmed;
        out.truncate(trimmed);
        for _ in 0..n {
            out.push_str("&#32;");
        }
    }
}

fn open_mark(m: Marks, style: Style) -> &'static str {
    match (m, style) {
        (Marks::UNDERLINE, _) => "<u>",
        (Marks::BOLD, Style::Markdown) => "**",
        (Marks::ITALIC, Style::Markdown) => "*",
        (Marks::STRIKETHROUGH, Style::Markdown) => "~~",
        (Marks::BOLD, _) => "<strong>",
        (Marks::ITALIC, _) => "<em>",
        (Marks::STRIKETHROUGH, _) => "<del>",
        _ => "<code>",
    }
}

fn close_mark(m: Marks, style: Style) -> &'static str {
    match (m, style) {
        (Marks::UNDERLINE, _) => "</u>",
        (Marks::BOLD, Style::Markdown) => "**",
        (Marks::ITALIC, Style::Markdown) => "*",
        (Marks::STRIKETHROUGH, Style::Markdown) => "~~",
        (Marks::BOLD, _) => "</strong>",
        (Marks::ITALIC, _) => "</em>",
        (Marks::STRIKETHROUGH, _) => "</del>",
        _ => "</code>",
    }
}

/// A line's content in Markdown that reads back as `pieces`: plain
/// delimiters where they do, else HTML tags for the marks.
fn line_markdown(pieces: &[Piece]) -> String {
    let pieces = trim_marks(pieces);
    let want = signature(&pieces);
    let mut first = None;
    for style in [Style::Markdown, Style::HtmlEmphasis, Style::Html] {
        let mut s = String::new();
        inline_markdown(&pieces, style, &mut s);
        let mut b = DocBuilder::new();
        b.start_block(BlockKind::Paragraph);
        parse_inline(&s, &mut b, false);
        let doc = b.finish();
        if signature(&doc.pieces(0)) == want {
            return s;
        }
        first.get_or_insert(s);
    }
    first.unwrap_or_default()
}

/// `doc` as Markdown.
pub(crate) fn to_markdown(doc: &RichDocument) -> String {
    let kinds = &doc.kinds;
    let mut out = String::new();
    let mut line = 0;
    while line < kinds.len() {
        let kind = kinds[line];
        let mut end = line + 1;
        if !matches!(kind, BlockKind::Paragraph | BlockKind::Heading(_)) {
            while end < kinds.len() && kinds[end] == kind {
                end += 1;
            }
        }
        if !out.is_empty() {
            out.push('\n');
        }
        match kind {
            BlockKind::Paragraph => {
                let body = line_markdown(&doc.pieces(line));
                out.push_str(if body.is_empty() { "<br>" } else { &body });
                out.push('\n');
            }
            BlockKind::Heading(n) => {
                out.push_str(&"#".repeat(usize::from(n.clamp(1, 3))));
                let body = line_markdown(&doc.pieces(line));
                if !body.is_empty() {
                    out.push(' ');
                    out.push_str(&body);
                }
                out.push('\n');
            }
            BlockKind::BulletItem | BlockKind::NumberedItem => {
                for (n, l) in (line..end).enumerate() {
                    if kind == BlockKind::BulletItem {
                        out.push('-');
                    } else {
                        out.push_str(&format!("{}.", n + 1));
                    }
                    let body = line_markdown(&doc.pieces(l));
                    if !body.is_empty() {
                        out.push(' ');
                        out.push_str(&body);
                    }
                    out.push('\n');
                }
            }
            BlockKind::Quote => {
                for l in line..end {
                    if l > line {
                        out.push_str(">\n");
                    }
                    let body = line_markdown(&doc.pieces(l));
                    out.push_str("> ");
                    out.push_str(if body.is_empty() { "<br>" } else { &body });
                    out.push('\n');
                }
            }
            BlockKind::CodeBlock => {
                let lines: Vec<String> = (line..end)
                    .map(|l| {
                        doc.pieces(l)
                            .iter()
                            .filter_map(|p| match p {
                                Piece::Text(t, _) => Some(t.as_str()),
                                Piece::Image(..) => None,
                            })
                            .collect()
                    })
                    .collect();
                let longest = lines
                    .iter()
                    .flat_map(|l| l.split(|c| c != '`'))
                    .map(str::len)
                    .max()
                    .unwrap_or(0);
                let fence = "`".repeat(longest.max(2) + 1);
                out.push_str(&fence);
                out.push('\n');
                for l in &lines {
                    out.push_str(l);
                    out.push('\n');
                }
                out.push_str(&fence);
                out.push('\n');
            }
        }
        line = end;
    }
    out
}

// --- Reading: blocks ---

/// A block being read: its kind and its lines so far.
struct Para {
    kind: BlockKind,
    lines: Vec<String>,
}

/// Reads Markdown block by block.
struct BlockReader<'a> {
    out: &'a mut DocBuilder,
    para: Option<Para>,
    decode_images: bool,
}

impl BlockReader<'_> {
    fn flush(&mut self) {
        let Some(para) = self.para.take() else { return };
        self.out.start_block(para.kind);
        // Soft line breaks read as spaces; a hard one (two spaces or a
        // backslash at the end) too, as a block holds one line.
        let n = para.lines.len();
        let mut text = String::new();
        for (i, line) in para.lines.iter().enumerate() {
            let mut l = line.trim_start().to_string();
            if i + 1 < n {
                l.truncate(l.trim_end().len());
                if l.ends_with('\\') && !l.ends_with("\\\\") {
                    l.pop();
                }
                l.push(' ');
            } else {
                l.truncate(l.trim_end().len());
            }
            text.push_str(&l);
        }
        // (An empty paragraph, written as `<br>`, reads back empty.)
        parse_inline(&text, self.out, self.decode_images);
    }

    fn start(&mut self, kind: BlockKind, text: &str) {
        self.flush();
        self.para = Some(Para {
            kind,
            lines: vec![text.to_string()],
        });
    }

    fn code_line(&mut self, text: &str) {
        self.flush();
        self.out.start_block(BlockKind::CodeBlock);
        self.out.push_text(text, &Attr::default());
    }
}

/// `line` without up to `n` columns of leading spaces (tabs count as 4).
fn strip_indent(line: &str, n: usize) -> (usize, &str) {
    let mut col = 0;
    for (i, c) in line.char_indices() {
        if col >= n {
            return (col, &line[i..]);
        }
        match c {
            ' ' => col += 1,
            '\t' => col += 4,
            _ => return (col, &line[i..]),
        }
    }
    (col, "")
}

/// The ATX heading on `line` (indent already stripped): its level and text.
fn atx(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let mut text = rest.trim();
    // An optional closing run of #s.
    let t = text.trim_end_matches('#');
    if t.is_empty() || t.ends_with([' ', '\t']) {
        text = t.trim_end();
    }
    Some((hashes as u8, text))
}

/// A thematic break: three or more `-`, `*` or `_`, with spaces.
fn is_break(line: &str) -> bool {
    let chars: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    chars.len() >= 3 && matches!(chars[0], '-' | '*' | '_') && chars.iter().all(|c| *c == chars[0])
}

/// The list item marker on `line`: its kind and the text after it.
fn list_item(line: &str) -> Option<(BlockKind, &str)> {
    let b = line.as_bytes();
    let (kind, len) = match b.first()? {
        b'-' | b'+' | b'*' => (BlockKind::BulletItem, 1),
        b'0'..=b'9' => {
            let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
            if digits > 9 || !matches!(b.get(digits), Some(b'.' | b')')) {
                return None;
            }
            (BlockKind::NumberedItem, digits + 1)
        }
        _ => return None,
    };
    let rest = &line[len..];
    if rest.is_empty() {
        return Some((kind, ""));
    }
    rest.starts_with([' ', '\t'])
        .then(|| (kind, rest.trim_start()))
}

/// A code fence opening on `line`: its character and length.
fn fence(line: &str) -> Option<(char, usize)> {
    let c = line.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = line.chars().take_while(|x| *x == c).count();
    (n >= 3 && !(c == '`' && line[n..].contains('`'))).then_some((c, n))
}

/// Read Markdown into a document.
pub(crate) fn from_markdown(md: &str, decode_images: bool) -> RichDocument {
    let mut out = DocBuilder::new();
    let mut r = BlockReader {
        out: &mut out,
        para: None,
        decode_images,
    };
    // An open fenced code block: its fence and indent.
    let mut fenced: Option<(char, usize, usize)> = None;
    for raw in md.lines() {
        if let Some((c, n, indent)) = fenced {
            let (_, t) = strip_indent(raw, 3);
            if t.chars().take_while(|x| *x == c).count() >= n
                && t.trim_start_matches(c).trim().is_empty()
            {
                fenced = None;
            } else {
                let (_, line) = strip_indent(raw, indent);
                r.code_line(line);
            }
            continue;
        }
        if raw.trim().is_empty() {
            r.flush();
            continue;
        }
        let (indent, line) = strip_indent(raw, 3);
        if indent >= 3 && raw.starts_with("    ") && r.para.is_none() {
            let (_, code) = strip_indent(raw, 4);
            r.code_line(code);
            continue;
        }
        // Inside a quote: one level of `>` read off; the rest is the line.
        let (quoted, line) = match line.strip_prefix('>') {
            Some(rest) => (true, rest.strip_prefix(' ').unwrap_or(rest)),
            None => (false, line),
        };
        if quoted {
            let inner = line.trim();
            if inner.is_empty() {
                r.flush();
                continue;
            }
            let (inner, kind) = match atx(inner) {
                Some((_, text)) => (text, BlockKind::Quote),
                None => (inner, BlockKind::Quote),
            };
            match &mut r.para {
                Some(p) if p.kind == BlockKind::Quote => p.lines.push(inner.to_string()),
                _ => r.start(kind, inner),
            }
            continue;
        }
        if let Some((c, n)) = fence(line) {
            r.flush();
            fenced = Some((c, n, indent));
            continue;
        }
        if let Some((level, text)) = atx(line) {
            r.start(BlockKind::heading(level), text);
            r.flush();
            continue;
        }
        // A setext underline turns the paragraph above into a heading.
        if let Some(p) = &mut r.para
            && p.kind == BlockKind::Paragraph
        {
            let t = line.trim_end();
            if !t.is_empty() && t.chars().all(|c| c == '=') {
                p.kind = BlockKind::Heading(1);
                r.flush();
                continue;
            }
            if !t.is_empty() && t.chars().all(|c| c == '-') {
                p.kind = BlockKind::Heading(2);
                r.flush();
                continue;
            }
        }
        if is_break(line) {
            r.flush();
            continue;
        }
        // Nested list items are flattened: their indent is ignored.
        let item = list_item(raw.trim_start());
        if let Some((kind, text)) = item {
            r.start(kind, text);
            continue;
        }
        match &mut r.para {
            // A continuation (lazy or indented) of the open block.
            Some(p) => p.lines.push(line.to_string()),
            None => r.start(BlockKind::Paragraph, line),
        }
    }
    r.flush();
    out.finish()
}

// --- Reading: inline content ---

/// A run of `*`, `_` or `~` that may open or close emphasis.
#[derive(Clone, Debug)]
struct Delim {
    ch: char,
    count: usize,
    orig: usize,
    can_open: bool,
    can_close: bool,
    active: bool,
    /// Marks it opens (after its leftover characters), outermost first.
    opens: Vec<Marks>,
    /// Marks it closes (before them), innermost first.
    closes: Vec<Marks>,
}

#[derive(Clone, Debug)]
enum Tok {
    Text(String),
    Delim(Delim),
    Code(String),
    /// `[` or `![`, literal unless a link or image closes it.
    Bracket {
        image: bool,
    },
    LinkOpen(Arc<str>),
    LinkClose,
    Image {
        src: String,
        alt: String,
        float: ImageFloat,
    },
    /// An inline HTML tag that opens (`true`) or closes a mark.
    Mark(Marks, bool),
}

fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace() && !c.is_control())
}

/// The emphasis matching of CommonMark's "process emphasis", over the
/// delimiters from token `bottom` on.
fn process_emphasis(toks: &mut [Tok], bottom: usize) {
    let idx: Vec<usize> = (bottom..toks.len())
        .filter(|i| matches!(&toks[*i], Tok::Delim(d) if d.active))
        .collect();
    for ci in 0..idx.len() {
        loop {
            let c = idx[ci];
            let Tok::Delim(closer) = &toks[c] else {
                break;
            };
            if !closer.active || !closer.can_close || closer.count == 0 {
                break;
            }
            let closer = closer.clone();
            let mut found = None;
            for oi in (0..ci).rev() {
                let Tok::Delim(o) = &toks[idx[oi]] else {
                    continue;
                };
                if !o.active || o.ch != closer.ch || !o.can_open || o.count == 0 {
                    continue;
                }
                if o.ch == '~' {
                    if o.count != closer.count || o.count > 2 {
                        continue;
                    }
                } else if (o.can_close || closer.can_open)
                    && (o.orig + closer.orig) % 3 == 0
                    && !(o.orig % 3 == 0 && closer.orig % 3 == 0)
                {
                    continue;
                }
                found = Some(oi);
                break;
            }
            let Some(oi) = found else {
                if !closer.can_open
                    && let Tok::Delim(d) = &mut toks[c]
                {
                    d.active = false;
                }
                break;
            };
            let o = idx[oi];
            let Tok::Delim(opener) = &toks[o] else {
                break;
            };
            let used = if closer.ch == '~' {
                closer.count
            } else if opener.count >= 2 && closer.count >= 2 {
                2
            } else {
                1
            };
            let mark = match (closer.ch, used) {
                ('~', _) => Marks::STRIKETHROUGH,
                (_, 2) => Marks::BOLD,
                _ => Marks::ITALIC,
            };
            for &between in &idx[oi + 1..ci] {
                if let Tok::Delim(d) = &mut toks[between] {
                    d.active = false;
                }
            }
            if let Tok::Delim(d) = &mut toks[o] {
                d.count -= used;
                d.opens.insert(0, mark);
                if d.count == 0 {
                    d.active = false;
                }
            }
            if let Tok::Delim(d) = &mut toks[c] {
                d.count -= used;
                d.closes.push(mark);
                if d.count == 0 {
                    d.active = false;
                    break;
                }
            }
        }
    }
    for &i in &idx {
        if let Tok::Delim(d) = &mut toks[i] {
            d.active = false;
        }
    }
}

/// The link destination after `]` at the `(` starting `s`, and the length
/// up to and with the `)`.
fn link_dest(s: &str) -> Option<(String, usize)> {
    let b = s.as_bytes();
    if b.first() != Some(&b'(') {
        return None;
    }
    let mut i = 1;
    let skip_ws = |i: &mut usize| {
        while b.get(*i).is_some_and(|c| c.is_ascii_whitespace()) {
            *i += 1;
        }
    };
    skip_ws(&mut i);
    let mut dest = String::new();
    if b.get(i) == Some(&b'<') {
        i += 1;
        loop {
            match *b.get(i)? {
                b'>' => {
                    i += 1;
                    break;
                }
                b'\n' | b'<' => return None,
                b'\\' if b.get(i + 1).is_some_and(u8::is_ascii_punctuation) => {
                    dest.push(b[i + 1] as char);
                    i += 2;
                }
                _ => {
                    let c = s[i..].chars().next()?;
                    dest.push(c);
                    i += c.len_utf8();
                }
            }
        }
    } else {
        let mut depth = 0;
        while let Some(&c) = b.get(i) {
            match c {
                b'\\' if b.get(i + 1).is_some_and(u8::is_ascii_punctuation) => {
                    dest.push(b[i + 1] as char);
                    i += 2;
                    continue;
                }
                b'(' => depth += 1,
                b')' if depth == 0 => break,
                b')' => depth -= 1,
                c if c.is_ascii_whitespace() || c.is_ascii_control() => break,
                _ => {}
            }
            let ch = s[i..].chars().next()?;
            dest.push(ch);
            i += ch.len_utf8();
        }
    }
    skip_ws(&mut i);
    // An optional title, read and dropped.
    if let Some(&q) = b.get(i)
        && matches!(q, b'"' | b'\'' | b'(')
    {
        let close = if q == b'(' { b')' } else { q };
        i += 1;
        while let Some(&c) = b.get(i) {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == close {
                break;
            }
            i += 1;
        }
        i += 1;
        skip_ws(&mut i);
    }
    if b.get(i) != Some(&b')') {
        return None;
    }
    Some((super::html::decode_entities(&dest), i + 1))
}

/// The plain text of tokens, as an image's alternative text.
fn plain(toks: &[Tok]) -> String {
    let mut s = String::new();
    for t in toks {
        match t {
            Tok::Text(x) | Tok::Code(x) => s.push_str(x),
            Tok::Delim(d) => s.extend(std::iter::repeat_n(d.ch, d.count)),
            Tok::Bracket { image } => s.push_str(if *image { "![" } else { "[" }),
            Tok::Image { alt, .. } => s.push_str(alt),
            _ => {}
        }
    }
    s
}

/// What an inline HTML tag means for the text.
fn html_tag(tag: &HtmlTag, toks: &mut Vec<Tok>, links: &mut usize) {
    let mark = match tag.name.as_str() {
        "b" | "strong" => Marks::BOLD,
        "i" | "em" => Marks::ITALIC,
        "u" | "ins" => Marks::UNDERLINE,
        "s" | "del" | "strike" => Marks::STRIKETHROUGH,
        "code" | "kbd" => Marks::CODE,
        "a" => {
            if tag.closing {
                if *links > 0 {
                    *links -= 1;
                    toks.push(Tok::LinkClose);
                }
            } else if let Some(href) = tag.attr("href") {
                *links += 1;
                toks.push(Tok::LinkOpen(Arc::from(href)));
            }
            return;
        }
        "img" => {
            toks.push(Tok::Image {
                src: tag.attr("src").unwrap_or_default().to_string(),
                alt: tag.attr("alt").unwrap_or_default().to_string(),
                float: ImageFloat::from_attrs(tag.attr("style"), tag.attr("align")),
            });
            return;
        }
        "br" => {
            toks.push(Tok::Text(String::new()));
            return;
        }
        _ => return,
    };
    toks.push(Tok::Mark(mark, !tag.closing));
}

/// Parse inline Markdown `s` (one block's text, line breaks already joined)
/// into the current block of `out`.
pub(crate) fn parse_inline(s: &str, out: &mut DocBuilder, decode_images: bool) {
    let mut toks: Vec<Tok> = Vec::new();
    let mut text = String::new();
    let mut brackets: Vec<(usize, bool)> = Vec::new();
    let mut html_links = 0;
    let flush = |text: &mut String, toks: &mut Vec<Tok>| {
        if !text.is_empty() {
            toks.push(Tok::Text(std::mem::take(text)));
        }
    };
    let b = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        let c = s[i..].chars().next().expect("in bounds");
        match c {
            '\\' => {
                match s[i + 1..].chars().next() {
                    Some(n) if n.is_ascii_punctuation() => {
                        text.push(n);
                        i += 2;
                    }
                    _ => {
                        text.push('\\');
                        i += 1;
                    }
                }
                continue;
            }
            '`' => {
                let n = s[i..].bytes().take_while(|x| *x == b'`').count();
                let after = i + n;
                // The closing run: exactly n backticks.
                let mut j = after;
                let mut close = None;
                while let Some(k) = s[j..].find('`') {
                    let start = j + k;
                    let len = s[start..].bytes().take_while(|x| *x == b'`').count();
                    if len == n {
                        close = Some(start);
                        break;
                    }
                    j = start + len;
                }
                match close {
                    Some(end) => {
                        let mut code = s[after..end].replace('\n', " ");
                        if code.len() >= 2
                            && code.starts_with(' ')
                            && code.ends_with(' ')
                            && !code.chars().all(|c| c == ' ')
                        {
                            code = code[1..code.len() - 1].to_string();
                        }
                        flush(&mut text, &mut toks);
                        toks.push(Tok::Code(code));
                        i = end + n;
                    }
                    None => {
                        text.push_str(&s[i..after]);
                        i = after;
                    }
                }
                continue;
            }
            '*' | '_' | '~' => {
                let n = s[i..].chars().take_while(|x| *x == c).count();
                let prev = s[..i].chars().next_back();
                let next = s[i + n..].chars().next();
                let ws = |x: Option<char>| x.is_none_or(char::is_whitespace);
                let pu = |x: Option<char>| x.is_some_and(is_punct);
                let left = !ws(next) && (!pu(next) || ws(prev) || pu(prev));
                let right = !ws(prev) && (!pu(prev) || ws(next) || pu(next));
                let (can_open, can_close) = match c {
                    '_' => (left && (!right || pu(prev)), right && (!left || pu(next))),
                    _ => (left, right),
                };
                flush(&mut text, &mut toks);
                toks.push(Tok::Delim(Delim {
                    ch: c,
                    count: n,
                    orig: n,
                    can_open,
                    can_close,
                    active: true,
                    opens: Vec::new(),
                    closes: Vec::new(),
                }));
                i += n;
                continue;
            }
            '!' if b.get(i + 1) == Some(&b'[') => {
                flush(&mut text, &mut toks);
                brackets.push((toks.len(), true));
                toks.push(Tok::Bracket { image: true });
                i += 2;
                continue;
            }
            '[' => {
                flush(&mut text, &mut toks);
                brackets.push((toks.len(), false));
                toks.push(Tok::Bracket { image: false });
                i += 1;
                continue;
            }
            ']' => {
                let Some((at, image)) = brackets.pop() else {
                    text.push(']');
                    i += 1;
                    continue;
                };
                match link_dest(&s[i + 1..]) {
                    Some((dest, len)) => {
                        flush(&mut text, &mut toks);
                        process_emphasis(&mut toks, at + 1);
                        if image {
                            let alt = plain(&toks[at + 1..]);
                            toks.truncate(at);
                            toks.push(Tok::Image {
                                src: dest,
                                alt,
                                float: ImageFloat::Inline,
                            });
                        } else {
                            toks[at] = Tok::LinkOpen(Arc::from(dest.as_str()));
                            toks.push(Tok::LinkClose);
                            // No links inside links.
                            for (k, img) in &brackets {
                                if !img {
                                    toks[*k] = Tok::Text("[".into());
                                }
                            }
                            brackets.retain(|(_, img)| *img);
                        }
                        i += 1 + len;
                    }
                    None => {
                        text.push(']');
                        i += 1;
                    }
                }
                continue;
            }
            '<' => {
                // An autolink, or an inline HTML tag.
                if let Some(end) = s[i + 1..].find('>') {
                    let inner = &s[i + 1..i + 1 + end];
                    let scheme = inner.find(':').is_some_and(|k| {
                        k >= 2
                            && inner[..k]
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
                    });
                    if scheme && !inner.contains([' ', '<']) {
                        flush(&mut text, &mut toks);
                        toks.push(Tok::LinkOpen(Arc::from(inner)));
                        toks.push(Tok::Text(inner.to_string()));
                        toks.push(Tok::LinkClose);
                        i += end + 2;
                        continue;
                    }
                }
                if let Some(body) = s[i..].strip_prefix("<!--")
                    && let Some(e) = body.find("-->")
                {
                    i += 4 + e + 3;
                    continue;
                }
                if let Some((tag, len)) = parse_tag(&s[i..]) {
                    flush(&mut text, &mut toks);
                    html_tag(&tag, &mut toks, &mut html_links);
                    i += len;
                    continue;
                }
                text.push('<');
                i += 1;
                continue;
            }
            '&' => {
                if let Some((c, len)) = parse_entity(&s[i + 1..]) {
                    text.push(c);
                    i += 1 + len;
                    continue;
                }
                text.push('&');
                i += 1;
                continue;
            }
            '\n' => {
                text.push(' ');
                i += 1;
                continue;
            }
            c => {
                text.push(c);
                i += c.len_utf8();
            }
        }
    }
    flush(&mut text, &mut toks);
    process_emphasis(&mut toks, 0);

    // Flatten into runs.
    let mut counts = [0i32; 5];
    let mut links: Vec<Arc<str>> = Vec::new();
    let bit = |m: Marks| Marks::ALL.iter().position(|x| *x == m).unwrap_or(0);
    let attr = |counts: &[i32; 5], links: &Vec<Arc<str>>| {
        let mut marks = Marks::NONE;
        for (k, m) in Marks::ALL.iter().enumerate() {
            if counts[k] > 0 {
                marks |= *m;
            }
        }
        Attr {
            marks,
            link: links.last().cloned(),
            image: None,
        }
    };
    for t in toks {
        match t {
            Tok::Text(x) => out.push_text(&x.replace(OBJECT, ""), &attr(&counts, &links)),
            Tok::Code(x) => {
                let mut a = attr(&counts, &links);
                a.marks |= Marks::CODE;
                out.push_text(&x, &a);
            }
            Tok::Delim(d) => {
                for m in &d.closes {
                    counts[bit(*m)] -= 1;
                }
                let lit: String = std::iter::repeat_n(d.ch, d.count).collect();
                out.push_text(&lit, &attr(&counts, &links));
                for m in &d.opens {
                    counts[bit(*m)] += 1;
                }
            }
            Tok::Bracket { image } => {
                out.push_text(if image { "![" } else { "[" }, &attr(&counts, &links))
            }
            Tok::LinkOpen(u) => links.push(u),
            Tok::LinkClose => {
                links.pop();
            }
            Tok::Mark(m, open) => {
                let k = bit(m);
                counts[k] = if open {
                    counts[k] + 1
                } else {
                    (counts[k] - 1).max(0)
                };
            }
            Tok::Image { src, alt, float } => {
                let pixmap = if decode_images {
                    super::model::load_source(&src, false).map(Arc::new)
                } else {
                    None
                };
                let image = RichImage::new(src, pixmap).alt(alt).floated(float);
                let a = attr(&counts, &links);
                out.push_image(Arc::new(image), &a);
            }
        }
    }
}
