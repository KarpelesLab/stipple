//! Inline content as a stream of opening and closing tags: what both
//! exporters write out, each in its own syntax.

use std::sync::Arc;

use super::model::{Attr, Marks, Piece, RichImage};

/// A tag around inline content.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Tag {
    Link(Arc<str>),
    /// One mark.
    Mark(Marks),
}

/// One step through a block's inline content.
#[derive(Clone, Debug)]
pub(crate) enum Event<'a> {
    Open(Tag),
    Close(Tag),
    Text(&'a str),
    Image(&'a RichImage),
}

/// The tags `attr` puts around content, outermost first: its link, then its
/// marks in [`Marks::ALL`]'s order (inline code innermost; never around an
/// image).
fn wanted(attr: &Attr, image: bool) -> Vec<Tag> {
    let mut tags = Vec::new();
    if let Some(link) = &attr.link {
        tags.push(Tag::Link(link.clone()));
    }
    for m in Marks::ALL {
        if attr.marks.contains(m) && !(image && m == Marks::CODE) {
            tags.push(Tag::Mark(m));
        }
    }
    tags
}

/// `pieces` as properly nested events: at each change of look, the tags no
/// longer wanted close (with those opened inside them), and the newly
/// wanted ones open.
pub(crate) fn events(pieces: &[Piece]) -> Vec<Event<'_>> {
    let mut out = Vec::new();
    let mut stack: Vec<Tag> = Vec::new();
    for piece in pieces {
        let want = match piece {
            Piece::Text(_, attr) => wanted(attr, false),
            Piece::Image(_, attr) => wanted(attr, true),
        };
        let keep = stack.iter().take_while(|t| want.contains(t)).count();
        while stack.len() > keep {
            out.push(Event::Close(stack.pop().expect("longer than keep")));
        }
        for tag in want {
            if !stack.contains(&tag) {
                stack.push(tag.clone());
                out.push(Event::Open(tag));
            }
        }
        match piece {
            Piece::Text(t, _) => out.push(Event::Text(t)),
            Piece::Image(i, _) => out.push(Event::Image(i)),
        }
    }
    while let Some(tag) = stack.pop() {
        out.push(Event::Close(tag));
    }
    out
}

/// A comparable form of a line's pieces: text with its marks and link, an
/// image by its source and alternative text.
pub(crate) fn signature(pieces: &[Piece]) -> Vec<(String, Marks, Option<Arc<str>>)> {
    let mut out: Vec<(String, Marks, Option<Arc<str>>)> = Vec::new();
    for p in pieces {
        let (text, attr) = match p {
            Piece::Text(t, a) => (t.clone(), a),
            Piece::Image(i, a) => (
                format!("\u{FFFC}{}\u{0}{}\u{0}{:?}", i.src, i.alt, i.float),
                a,
            ),
        };
        match out.last_mut() {
            Some(last)
                if !matches!(p, Piece::Image(..))
                    && last.1 == attr.marks
                    && last.2 == attr.link
                    && !last.0.starts_with('\u{FFFC}') =>
            {
                last.0.push_str(&text)
            }
            _ => out.push((text, attr.marks, attr.link.clone())),
        }
    }
    out
}
