use super::*;
use crate::App;
use crate::core::LayoutNode;
use crate::geometry::Size;
use crate::platform::{Modifiers, PhysicalKey};

fn typed(ed: &mut RichEditor, text: &str) {
    ed.apply(&KeyInput::Text(text.into()));
}

fn doc(blocks: Vec<Block>) -> RichDocument {
    RichDocument::from_blocks(blocks)
}

fn para(inlines: Vec<Inline>) -> Block {
    Block::new(BlockKind::Paragraph, inlines)
}

// --- The model ---

#[test]
fn typing_takes_the_marks_at_the_caret() {
    let mut ed = RichEditor::new();
    typed(&mut ed, "plain ");
    ed.toggle_mark(Marks::BOLD);
    assert!(ed.marks().contains(Marks::BOLD), "pending at the caret");
    typed(&mut ed, "bold");
    // Typing after bold text continues it.
    typed(&mut ed, "er");
    ed.toggle_mark(Marks::BOLD);
    typed(&mut ed, " end");
    assert_eq!(ed.to_markdown(), "plain **bolder** end\n");
    // Moving the caret forgets marks toggled but not used.
    ed.toggle_mark(Marks::ITALIC);
    ed.apply(&KeyInput::Left);
    ed.apply(&KeyInput::Right);
    assert!(!ed.marks().contains(Marks::ITALIC));
}

#[test]
fn toggling_a_mark_on_a_selection() {
    let mut ed = RichEditor::new();
    typed(&mut ed, "one two three");
    ed.select(4, 7);
    ed.toggle_mark(Marks::BOLD);
    assert_eq!(ed.to_markdown(), "one **two** three\n");
    assert_eq!(ed.marks(), Marks::BOLD);
    // Partly bold: toggling makes all of it bold.
    ed.select(0, 7);
    assert!(!ed.marks().contains(Marks::BOLD));
    ed.toggle_mark(Marks::BOLD);
    assert_eq!(ed.to_markdown(), "**one two** three\n");
    // All bold: toggling takes it off.
    ed.toggle_mark(Marks::BOLD);
    assert_eq!(ed.to_markdown(), "one two three\n");
    // Each toggle is an undo step.
    ed.undo();
    assert_eq!(ed.to_markdown(), "**one two** three\n");
    ed.undo();
    ed.undo();
    assert_eq!(ed.to_markdown(), "one two three\n");
}

#[test]
fn enter_splits_blocks_and_ends_lists() {
    let mut ed = RichEditor::new();
    ed.set_block_kind(BlockKind::heading(1));
    typed(&mut ed, "Title");
    ed.apply(&KeyInput::Enter);
    assert_eq!(ed.block_kind(), BlockKind::Paragraph, "after a heading");
    ed.toggle_block_kind(BlockKind::BulletItem);
    typed(&mut ed, "one");
    ed.apply(&KeyInput::Enter);
    assert_eq!(ed.block_kind(), BlockKind::BulletItem, "a list goes on");
    typed(&mut ed, "two");
    ed.apply(&KeyInput::Enter);
    // Enter on the empty item ends the list.
    ed.apply(&KeyInput::Enter);
    assert_eq!(ed.block_kind(), BlockKind::Paragraph);
    typed(&mut ed, "after");
    assert_eq!(ed.to_markdown(), "# Title\n\n- one\n- two\n\nafter\n");
    // Splitting mid-word.
    ed.apply(&KeyInput::Left);
    ed.apply(&KeyInput::Left);
    ed.apply(&KeyInput::Enter);
    assert_eq!(ed.document().block_count(), 5);
    assert_eq!(ed.plain_text(), "Title\n• one\n• two\naft\ner");
    // Enter at the start of a heading pushes it down.
    ed.select(0, 0);
    ed.apply(&KeyInput::Enter);
    let kinds: Vec<BlockKind> = ed.document().blocks().iter().map(|b| b.kind).collect();
    assert_eq!(kinds[..2], [BlockKind::Paragraph, BlockKind::heading(1)]);
}

#[test]
fn backspace_at_a_block_start_outdents_then_joins() {
    let mut ed = RichEditor::from_markdown("first\n\n- item");
    assert_eq!(ed.block_kind(), BlockKind::BulletItem);
    ed.apply(&KeyInput::Home);
    ed.apply(&KeyInput::Backspace);
    assert_eq!(ed.block_kind(), BlockKind::Paragraph, "the item outdents");
    assert_eq!(ed.plain_text(), "first\nitem");
    ed.apply(&KeyInput::Backspace);
    assert_eq!(ed.plain_text(), "firstitem", "then joins the block above");
    assert_eq!(ed.caret(), 5);
    ed.apply(&KeyInput::Undo);
    ed.apply(&KeyInput::Undo);
    assert_eq!(ed.to_markdown(), "first\n\n- item\n");
    ed.apply(&KeyInput::Redo);
    assert_eq!(ed.to_markdown(), "first\n\nitem\n");
    // Delete at a block's end joins the next one into it.
    let mut ed = RichEditor::from_markdown("# a\n\nb");
    ed.select(1, 1);
    ed.apply(&KeyInput::Delete);
    assert_eq!(ed.to_markdown(), "# ab\n");
}

#[test]
fn typing_coalesces_into_one_undo_step() {
    let mut ed = RichEditor::new();
    for c in ["h", "e", "l", "l", "o"] {
        typed(&mut ed, c);
    }
    ed.apply(&KeyInput::Enter);
    typed(&mut ed, "x");
    ed.apply(&KeyInput::Backspace);
    // At the empty block's start: joins the block above, a step of its own.
    ed.apply(&KeyInput::Backspace);
    assert_eq!(ed.plain_text(), "hello");
    assert!(ed.undo()); // the join
    assert_eq!(ed.plain_text(), "hello\n");
    assert!(ed.undo()); // the backspace
    assert_eq!(ed.plain_text(), "hello\nx");
    assert!(ed.undo()); // the x
    assert!(ed.undo()); // Enter
    assert_eq!(ed.plain_text(), "hello");
    assert!(ed.undo()); // the word
    assert_eq!(ed.plain_text(), "");
    assert!(!ed.undo());
    assert!(ed.redo());
    assert_eq!(ed.plain_text(), "hello");
    // A new edit clears what could be redone.
    typed(&mut ed, "!");
    assert!(!ed.can_redo());
}

#[test]
fn block_kinds_apply_to_every_selected_block() {
    let mut ed = RichEditor::from_markdown("a\n\nb\n\nc");
    ed.select(0, 3);
    ed.toggle_block_kind(BlockKind::NumberedItem);
    assert_eq!(ed.to_markdown(), "1. a\n2. b\n\nc\n");
    ed.toggle_block_kind(BlockKind::NumberedItem);
    assert_eq!(ed.to_markdown(), "a\n\nb\n\nc\n");
    ed.select_all();
    ed.set_block_kind(BlockKind::Quote);
    assert_eq!(ed.to_markdown(), "> a\n>\n> b\n>\n> c\n");
    ed.set_block_kind(BlockKind::CodeBlock);
    assert_eq!(ed.to_markdown(), "```\na\nb\nc\n```\n");
}

#[test]
fn links_on_a_selection_and_at_the_caret() {
    let mut ed = RichEditor::new();
    typed(&mut ed, "see the docs here");
    ed.select(8, 12);
    ed.set_link(Some("https://example.com/a_(b)"));
    assert_eq!(
        ed.to_markdown(),
        "see the [docs](<https://example.com/a_(b)>) here\n"
    );
    assert_eq!(
        ed.to_html(),
        "<p>see the <a href=\"https://example.com/a_(b)\">docs</a> here</p>\n"
    );
    // Typing at the link's end doesn't extend it.
    ed.select(12, 12);
    assert_eq!(ed.link().as_deref(), Some("https://example.com/a_(b)"));
    typed(&mut ed, "!");
    assert_eq!(ed.plain_text(), "see the docs! here");
    assert!(ed.to_markdown().contains("](<https://example.com/a_(b)>)!"));
    // At a caret inside it, the whole link goes.
    ed.select(10, 10);
    ed.set_link(None);
    assert_eq!(ed.to_markdown(), "see the docs! here\n");
    // With nothing selected and no link there, the URL goes in as a link.
    let mut ed = RichEditor::new();
    ed.set_link(Some("https://x.org"));
    assert_eq!(ed.to_markdown(), "[https://x.org](https://x.org)\n");
}

fn sample_image() -> RichImage {
    let mut px = Pixmap::new(PhysicalSize::new(4, 2));
    px.as_bytes_mut().fill(0x80);
    RichImage::from_pixmap(px).alt("a [gray] box")
}

#[test]
fn images_insert_and_export() {
    let mut ed = RichEditor::new();
    typed(&mut ed, "x ");
    ed.insert_image(sample_image());
    typed(&mut ed, " y");
    let img = ed.document().images().remove(0);
    assert!(img.src.starts_with("data:image/png;base64,"));
    assert_eq!(
        ed.to_markdown(),
        format!("x ![a \\[gray\\] box]({}) y\n", img.src)
    );
    assert_eq!(
        ed.to_html(),
        format!(
            "<p>x <img src=\"{}\" alt=\"a [gray] box\"> y</p>\n",
            img.src
        )
    );
    // Both read back, decoding the data URI.
    let back = RichDocument::from_markdown(&ed.to_markdown());
    assert_eq!(back, *ed.document());
    assert_eq!(RichDocument::from_html(&ed.to_html()), *ed.document());
    assert_eq!(back.images()[0].pixmap.as_ref().unwrap().size().width, 4);
    // A file path stays a path, loaded on demand.
    let md = "![logo](assets/logo.png)";
    let doc = RichDocument::from_markdown(md);
    assert_eq!(doc.images()[0].src, "assets/logo.png");
    assert!(doc.images()[0].pixmap.is_none());
    assert_eq!(
        doc.to_html(),
        "<p><img src=\"assets/logo.png\" alt=\"logo\"></p>\n"
    );
}

#[test]
fn base64_round_trips() {
    for n in 0..10 {
        let bytes: Vec<u8> = (0..n).map(|i| (i * 37 + 5) as u8).collect();
        let enc = model::base64_encode(&bytes);
        assert_eq!(enc.len() % 4, 0);
        assert_eq!(model::base64_decode(&enc).unwrap(), bytes);
    }
    assert_eq!(model::base64_encode(b"Man"), "TWFu");
    assert_eq!(model::base64_encode(b"Ma"), "TWE=");
    assert!(model::base64_decode("a$b").is_none());
}

#[test]
fn copy_and_paste_keep_formatting() {
    let mut ed = RichEditor::from_markdown("# Head\n\nsome **bold** and *it*\n\n- item");
    ed.select_all();
    ed.apply(&KeyInput::Copy);
    let content = crate::core::clipboard_content();
    assert_eq!(
        content.text.as_deref(),
        Some("Head\nsome bold and it\n• item")
    );
    assert!(
        content
            .html
            .as_deref()
            .unwrap()
            .contains("<strong>bold</strong>")
    );
    let mut other = RichEditor::new();
    other.apply(&KeyInput::Paste);
    assert_eq!(other.to_markdown(), ed.to_markdown());
    // HTML from elsewhere.
    crate::core::set_clipboard_content(
        &ClipboardContent::from_text("x").html("<meta charset=utf-8><b>Hi</b> <i>there</i>"),
    );
    let mut ed = RichEditor::new();
    typed(&mut ed, "> ");
    ed.apply(&KeyInput::Paste);
    assert_eq!(ed.to_markdown(), "\\> **Hi** *there*\n");
    // Plain text: new blocks for its lines, in the marks at the caret.
    crate::core::set_clipboard_text("a\nb");
    let mut ed = RichEditor::new();
    ed.toggle_mark(Marks::ITALIC);
    ed.apply(&KeyInput::Paste);
    assert_eq!(ed.to_markdown(), "*a*\n\n*b*\n");
    // An image.
    let mut px = Pixmap::new(PhysicalSize::new(3, 3));
    px.as_bytes_mut().fill(0xff);
    crate::core::set_clipboard_content(&ClipboardContent::from_image(px));
    let mut ed = RichEditor::new();
    ed.apply(&KeyInput::Paste);
    assert_eq!(ed.document().images().len(), 1);
    // Cutting copies and deletes, as one step.
    let mut ed = RichEditor::from_markdown("cut **me**");
    ed.select(4, 6);
    ed.apply(&KeyInput::Cut);
    assert_eq!(ed.to_markdown(), "cut&#32;\n", "a trailing space kept");
    assert_eq!(crate::core::clipboard_text(), "me");
    ed.undo();
    assert_eq!(ed.to_markdown(), "cut **me**\n");
}

#[test]
fn composition_shows_at_the_caret_until_committed() {
    let mut ed = RichEditor::from_markdown("- ab");
    ed.select(1, 1);
    ed.apply(&KeyInput::Preedit {
        text: "にほ".into(),
        cursor: Some((6, 6)),
    });
    assert_eq!(ed.plain_text(), "• ab", "not in the document");
    let theme = Theme::light();
    let leaf = ed.leaf(&theme);
    let crate::core::ElementKind::Text { text, .. } = &leaf.kind else {
        panic!("a text leaf");
    };
    assert_eq!(text, "  • aにほb");
    assert_eq!(leaf.preedit, Some((7, 13)));
    assert_eq!(leaf.caret, Some(13));
    // A click inside the composition maps to the caret; after it, back.
    assert_eq!(ed.shown_to_model(9), 1);
    assert_eq!(ed.shown_to_model(14), 2);
    assert_eq!(ed.shown_to_model(2), 0, "in the bullet: the line's start");
    ed.apply(&KeyInput::Text("日本".into()));
    assert_eq!(ed.plain_text(), "• a日本b");
    assert!(ed.undo());
    assert_eq!(ed.plain_text(), "• ab");
}

#[test]
fn shown_positions_skip_the_prefixes() {
    let mut ed = RichEditor::from_markdown("1. one\n2. two\n\n> q");
    // "  1. one\n  2. two\n│ q"
    let maps = ed.line_maps();
    assert_eq!(ed.model_to_shown(&maps, 0), 5);
    assert_eq!(ed.model_to_shown(&maps, 4), 14);
    assert_eq!(ed.model_to_shown(&maps, 8), 18 + "│ ".len());
    assert_eq!(ed.shown_to_model(14), 4);
    assert_eq!(ed.shown_to_model(15), 5);
    assert_eq!(ed.shown_to_model(10), 4, "in the number: the line's start");
    // An assistive technology's selection in shown indices.
    ed.apply(&KeyInput::SetSelection {
        anchor: 5,
        caret: 8,
    });
    assert_eq!(ed.selection(), Some((0, 3)));
}

// --- Markdown ---

#[test]
fn markdown_escapes_what_would_be_syntax() {
    let text = "*not* _em_ # [x](y) <b> `c` ~~s~~ & \\ ! |";
    let d = doc(vec![para(vec![Inline::text(text)])]);
    let md = d.to_markdown();
    assert_eq!(
        md,
        "\\*not\\* \\_em\\_ # \\[x\\](y) \\<b\\> \\`c\\` \\~\\~s\\~\\~ \\& \\\\ ! |\n"
    );
    assert_eq!(RichDocument::from_markdown(&md), d);
    // Block syntax at a line's start.
    for line in [
        "# not a heading",
        "- not a list",
        "1. not a list",
        "12) nope",
        "+ x",
        "=== x",
        "  spaced  ",
    ] {
        let d = doc(vec![para(vec![Inline::text(line)])]);
        let md = d.to_markdown();
        assert_eq!(RichDocument::from_markdown(&md), d, "{line:?} as {md:?}");
    }
    // Inside a list item too.
    let d = doc(vec![Block::new(
        BlockKind::BulletItem,
        vec![Inline::text("# x")],
    )]);
    assert_eq!(d.to_markdown(), "- \\# x\n");
    assert_eq!(RichDocument::from_markdown(&d.to_markdown()), d);
}

#[test]
fn markdown_round_trips_marks() {
    let cases = vec![
        vec![
            Inline::styled("bold", Marks::BOLD),
            Inline::styled("both", Marks::BOLD | Marks::ITALIC),
            Inline::text("c"),
        ],
        vec![
            Inline::styled("a", Marks::ITALIC),
            Inline::styled("b", Marks::ITALIC | Marks::BOLD),
        ],
        vec![
            Inline::text("in"),
            Inline::styled("word", Marks::BOLD),
            Inline::text("bold"),
        ],
        vec![Inline::styled("\"quoted\"", Marks::BOLD), Inline::text("x")],
        vec![
            Inline::text("a"),
            Inline::styled("(b)", Marks::ITALIC),
            Inline::text("c"),
        ],
        vec![
            Inline::styled("under", Marks::UNDERLINE),
            Inline::styled("strike", Marks::STRIKETHROUGH),
        ],
        vec![
            Inline::styled("co`de", Marks::CODE),
            Inline::text(" "),
            Inline::styled("`x`", Marks::CODE),
        ],
        vec![Inline::styled("bold code", Marks::BOLD | Marks::CODE)],
        vec![
            Inline::link("a *link*", "http://x.y/z"),
            Inline::text(" "),
            Inline::styled("b", Marks::BOLD),
        ],
        vec![Inline::Text {
            text: "linked bold".into(),
            marks: Marks::BOLD,
            link: Some("u".into()),
        }],
        vec![
            Inline::styled("**", Marks::BOLD),
            Inline::styled("_", Marks::ITALIC),
        ],
        vec![Inline::text("a  b   c")],
    ];
    for inlines in cases {
        let d = doc(vec![para(inlines.clone())]);
        let md = d.to_markdown();
        assert_eq!(RichDocument::from_markdown(&md), d, "{inlines:?} as {md:?}");
        let html = d.to_html();
        assert_eq!(RichDocument::from_html(&html), d, "{inlines:?} as {html:?}");
    }
}

#[test]
fn marks_move_off_the_spaces_at_their_ends() {
    let d = doc(vec![para(vec![
        Inline::text("a"),
        Inline::styled(" b ", Marks::BOLD),
        Inline::text("c"),
    ])]);
    assert_eq!(d.to_markdown(), "a **b** c\n");
}

#[test]
fn markdown_reads_commonmark() {
    let md = "Setext\n======\n\nSub\n---\n\n### Three\n#### Four ####\n\n\
              para *em* __strong__ ***both*** ~~gone~~ `a`` b` \\*lit\\*\n\
              continued line\n\n\
              * one\n+ two\n  - nested\n\n\
              3) three\n\n\
              > quoted\n> lazy\n\n\
              ```rust\nfn x() {}\n\n  y\n```\n\n    indented code\n\n\
              <https://a.b> and [l](</p q> \"title\") ![i](s.png) &amp; &copy; &#x41;\n\n***\n";
    let blocks = RichDocument::from_markdown(md).blocks();
    let kinds: Vec<BlockKind> = blocks.iter().map(|b| b.kind).collect();
    use BlockKind::*;
    assert_eq!(
        kinds,
        [
            Heading(1),
            Heading(2),
            Heading(3),
            Heading(3),
            Paragraph,
            BulletItem,
            BulletItem,
            BulletItem,
            NumberedItem,
            Quote,
            CodeBlock,
            CodeBlock,
            CodeBlock,
            CodeBlock,
            Paragraph
        ]
    );
    let text = |b: &Block| -> String {
        b.inlines
            .iter()
            .map(|i| match i {
                Inline::Text { text, .. } => text.clone(),
                Inline::Image(img) => format!("[img {}]", img.alt),
            })
            .collect()
    };
    assert_eq!(text(&blocks[3]), "Four");
    assert_eq!(
        text(&blocks[4]),
        "para em strong both gone a`` b *lit* continued line"
    );
    let marks: Vec<(String, Marks)> = blocks[4]
        .inlines
        .iter()
        .filter_map(|i| match i {
            Inline::Text { text, marks, .. } if !marks.is_empty() => Some((text.clone(), *marks)),
            _ => None,
        })
        .collect();
    assert_eq!(
        marks,
        [
            ("em".to_string(), Marks::ITALIC),
            ("strong".to_string(), Marks::BOLD),
            ("both".to_string(), Marks::BOLD | Marks::ITALIC),
            ("gone".to_string(), Marks::STRIKETHROUGH),
            ("a`` b".to_string(), Marks::CODE),
        ]
    );
    assert_eq!(text(&blocks[7]), "nested");
    assert_eq!(text(&blocks[9]), "quoted lazy");
    assert_eq!(text(&blocks[10]), "fn x() {}");
    assert_eq!(text(&blocks[11]), "");
    assert_eq!(text(&blocks[12]), "  y");
    assert_eq!(text(&blocks[13]), "indented code");
    assert_eq!(text(&blocks[14]), "https://a.b and l [img i] & © A");
    let links: Vec<Option<String>> = blocks[14]
        .inlines
        .iter()
        .filter_map(|i| match i {
            Inline::Text { link, .. } => Some(link.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(links[0].as_deref(), Some("https://a.b"));
    assert_eq!(links[2].as_deref(), Some("/p q"));
}

#[test]
fn emphasis_follows_the_delimiter_rules() {
    let marks = |md: &str| -> Vec<(String, Marks)> {
        RichDocument::from_markdown(md).blocks()[0]
            .inlines
            .iter()
            .filter_map(|i| match i {
                Inline::Text { text, marks, .. } => Some((text.clone(), *marks)),
                _ => None,
            })
            .collect()
    };
    let s = |t: &str, m: Marks| (t.to_string(), m);
    assert_eq!(
        marks("a*b*c"),
        [
            s("a", Marks::NONE),
            s("b", Marks::ITALIC),
            s("c", Marks::NONE)
        ]
    );
    assert_eq!(marks("a_b_c"), [s("a_b_c", Marks::NONE)], "no intraword _");
    assert_eq!(marks("x * a *"), [s("x * a *", Marks::NONE)]);
    assert_eq!(
        marks("**a*b***"),
        [s("a", Marks::BOLD), s("b", Marks::BOLD | Marks::ITALIC)]
    );
    assert_eq!(
        marks("*a **b***"),
        [s("a ", Marks::ITALIC), s("b", Marks::ITALIC | Marks::BOLD)]
    );
    assert_eq!(marks("**unclosed"), [s("**unclosed", Marks::NONE)]);
    assert_eq!(
        marks("~one~ ~~two~~"),
        [
            s("one", Marks::STRIKETHROUGH),
            s(" ", Marks::NONE),
            s("two", Marks::STRIKETHROUGH)
        ]
    );
    assert_eq!(marks("[*a*](u)"), [s("a", Marks::ITALIC)]);
    assert_eq!(
        marks("<u>x</u><strong>y</strong>"),
        [s("x", Marks::UNDERLINE), s("y", Marks::BOLD)]
    );
}

#[test]
fn whole_documents_round_trip() {
    let d = doc(vec![
        Block::new(BlockKind::heading(1), vec![Inline::text("Title & <more>")]),
        para(vec![
            Inline::text("Some "),
            Inline::styled("bold", Marks::BOLD),
            Inline::text(", "),
            Inline::styled("italic", Marks::ITALIC),
            Inline::text(" and "),
            Inline::link("a link", "https://example.com/?a=1&b=\"2\""),
            Inline::text("."),
        ]),
        para(vec![]),
        Block::new(BlockKind::heading(2), vec![Inline::text("List")]),
        Block::new(BlockKind::BulletItem, vec![Inline::text("one")]),
        Block::new(BlockKind::BulletItem, vec![]),
        Block::new(
            BlockKind::BulletItem,
            vec![Inline::styled("three", Marks::CODE)],
        ),
        Block::new(BlockKind::NumberedItem, vec![Inline::text("first")]),
        Block::new(BlockKind::NumberedItem, vec![Inline::text("second")]),
        Block::new(BlockKind::Quote, vec![Inline::text("wise")]),
        Block::new(BlockKind::Quote, vec![]),
        Block::new(BlockKind::Quote, vec![Inline::text("words")]),
        Block::new(BlockKind::CodeBlock, vec![Inline::text("let x = `1` < 2;")]),
        Block::new(BlockKind::CodeBlock, vec![]),
        Block::new(BlockKind::CodeBlock, vec![Inline::text("```")]),
        Block::new(BlockKind::heading(3), vec![]),
        para(vec![Inline::text(" lead and trail ")]),
    ]);
    let md = d.to_markdown();
    assert_eq!(RichDocument::from_markdown(&md), d, "{md}");
    let html = d.to_html();
    assert_eq!(RichDocument::from_html(&html), d, "{html}");
    assert!(html.contains("<h1>Title &amp; &lt;more&gt;</h1>"));
    assert!(html.contains("href=\"https://example.com/?a=1&amp;b=&quot;2&quot;\""));
    assert!(html.contains("<pre><code>let x = `1` &lt; 2;\n\n```</code></pre>"));
    assert!(md.contains("````\nlet x = `1` < 2;\n\n```\n````"));
}

// --- HTML ---

#[test]
fn html_reads_what_browsers_and_editors_paste() {
    let html = r#"<html><head><style>p{color:red}</style><title>t</title></head>
        <body><!--StartFragment--><b style="font-weight:normal" id="docs-internal">
        <p><span style="font-weight:700">Bold</span>   and <span style="font-style:italic;text-decoration:underline">ui</span></p>
        <div>line<br>break</div>
        <ul><li>a<ul><li>nested</li></ul></li><li><p>b</p></li></ul>
        <ol><li>n</li></ol>
        <h5>small&nbsp;&nbsp;head</h5>
        <pre>
code  kept
  indented</pre>
        <script>alert(1)</script><blockquote>q <del>x</del></blockquote>
        <table><tr><td>cell</td></tr></table>
        </b><!--EndFragment--></body></html>"#;
    let blocks = RichDocument::from_html(html).blocks();
    let summary: Vec<(BlockKind, String)> = blocks
        .iter()
        .map(|b| {
            let t = b
                .inlines
                .iter()
                .map(|i| match i {
                    Inline::Text { text, .. } => text.clone(),
                    Inline::Image(_) => "[img]".into(),
                })
                .collect();
            (b.kind, t)
        })
        .collect();
    use BlockKind::*;
    let s = |k: BlockKind, t: &str| (k, t.to_string());
    assert_eq!(
        summary,
        [
            s(Paragraph, "Bold and ui"),
            s(Paragraph, "line"),
            s(Paragraph, "break"),
            s(BulletItem, "a"),
            s(BulletItem, "nested"),
            s(BulletItem, "b"),
            s(NumberedItem, "n"),
            s(Heading(3), "small  head"),
            s(CodeBlock, "code  kept"),
            s(CodeBlock, "  indented"),
            s(Quote, "q x"),
            s(Paragraph, "cell"),
        ]
    );
    let first = &blocks[0].inlines;
    assert_eq!(
        first[0],
        Inline::styled("Bold", Marks::BOLD),
        "the outer <b> is normal"
    );
    assert_eq!(
        first[2],
        Inline::styled("ui", Marks::ITALIC | Marks::UNDERLINE)
    );
    assert_eq!(
        blocks[10].inlines[1],
        Inline::styled("x", Marks::STRIKETHROUGH)
    );
}

#[test]
fn html_escapes_and_keeps_spaces() {
    let d = doc(vec![para(vec![Inline::text("  a <b> & \"c\"  ")])]);
    let html = d.to_html();
    assert_eq!(html, "<p>&nbsp;&nbsp;a &lt;b&gt; &amp; \"c\" &nbsp;</p>\n");
    assert_eq!(RichDocument::from_html(&html), d);
    // Unclosed and stray tags, a lone <, unknown entities.
    let d = RichDocument::from_html("<p>1 < 2 &bogus; <em>open");
    assert_eq!(d.plain_text(), "1 < 2 &bogus; open");
}

// --- In an App ---

#[derive(Default)]
struct Doc {
    editor: RichEditor,
    changes: u32,
}

fn tool(cx: &mut Cx<Doc>, label: &str, on: bool, f: impl Fn(&mut RichEditor) + 'static) -> Element {
    super::super::ToolButton::new(label)
        .selected(on)
        .view(cx)
        .keeps_focus()
        .on_tap(cx, move |s: &mut Doc| f(&mut s.editor))
}

fn app() -> App<Doc> {
    let mut app = App::new(Doc::default(), |s: &Doc, cx: &mut Cx<Doc>| {
        let marks = s.editor.marks();
        let bar = super::super::toolbar(cx).children(vec![
            tool(cx, "B", marks.contains(Marks::BOLD), |e| {
                e.toggle_mark(Marks::BOLD)
            }),
            tool(
                cx,
                "List",
                s.editor.block_kind() == BlockKind::BulletItem,
                |e| e.toggle_block_kind(BlockKind::BulletItem),
            ),
        ]);
        super::super::column(vec![
            bar,
            rich_editor(cx, &s.editor, |s| &mut s.editor)
                .placeholder("Write here")
                .on_change(cx, |s| s.changes += 1),
        ])
    })
    .logical_size(Size::new(600.0, 400.0));
    app = match Font::system_default() {
        Some(f) => app.font(f),
        None => app.no_font(),
    };
    app
}

/// The tappable elements' bounds, in tree order.
fn buttons(node: &LayoutNode, out: &mut Vec<crate::geometry::Rect>) {
    if node.action.is_some() {
        out.push(node.bounds);
    }
    node.children.iter().for_each(|c| buttons(c, out));
}

#[test]
fn typing_and_toolbar_clicks_in_an_app() {
    let mut app = app();
    app.render_once();
    // Focus the editor (the toolbar's buttons come first in Tab order).
    while !app.focused().is_some_and(|f| !f.is_tap()) {
        assert!(app.focus_next());
    }
    let editor_focus = app.focused();
    app.type_text("Hello ");
    // Ctrl/Cmd+B from the keyboard, as a raw key.
    let mut ev = KeyEvent::new(
        ButtonState::Pressed,
        KeyCode::Char('b'),
        PhysicalKey::UNIDENTIFIED,
    );
    ev.modifiers = Modifiers::COMMAND;
    assert!(app.raw_key(0, &ev).consumed);
    app.type_text("world");
    assert_eq!(app.state().editor.to_markdown(), "Hello **world**\n");
    assert!(app.state().changes >= 2);
    // The toolbar shows bold as on, and a click on it keeps the editor
    // focused.
    app.render_once();
    let mut b = Vec::new();
    buttons(app.panes[0].tree.as_ref().unwrap(), &mut b);
    assert_eq!(b.len(), 2);
    assert!(app.click_at(b[0].center()));
    assert_eq!(app.focused(), editor_focus, "the editor keeps focus");
    app.type_text("!");
    assert_eq!(app.state().editor.to_markdown(), "Hello **world**!\n");
    app.render_once();
    assert!(app.click_at(b[1].center()));
    app.press_key(KeyInput::Enter);
    app.type_text("two");
    assert_eq!(
        app.state().editor.to_markdown(),
        "- Hello **world**!\n- two\n"
    );
    app.press_key(KeyInput::Undo);
    app.press_key(KeyInput::Undo);
    assert_eq!(app.state().editor.to_markdown(), "- Hello **world**!\n");
    // A screen reader sees a multi-line text field: the shown text, caret.
    app.render_once();
    let tree = app.accessibility_tree().unwrap();
    fn field(n: &crate::core::AccessNode) -> Option<&crate::core::AccessNode> {
        if n.role == crate::core::Role::TextField {
            return Some(n);
        }
        n.children.iter().find_map(field)
    }
    let f = field(&tree).expect("a text field");
    assert_eq!(f.name, "  • Hello world!");
    assert!(f.focused);
    assert_eq!(f.caret, Some(f.name.len()));
}

#[test]
fn clicks_place_the_caret_in_the_document() {
    let mut app = app();
    app.state
        .editor
        .set_document(RichDocument::from_markdown("- alpha beta"));
    app.render_once();
    fn text_leaf(n: &LayoutNode) -> Option<&LayoutNode> {
        if n.text_pos.is_some() {
            return Some(n);
        }
        n.children.iter().find_map(text_leaf)
    }
    let tree = app.panes[0].tree.as_ref().unwrap();
    let leaf = text_leaf(tree).unwrap().bounds;
    // At the far left (in the bullet): the start of the item.
    assert!(app.text_press_at(crate::geometry::Point::new(
        leaf.min_x() + 1.0,
        leaf.min_y() + 4.0
    )));
    app.end_text_select();
    assert_eq!(app.state().editor.caret(), 0);
    assert!(app.focused().is_some());
    app.type_text("x");
    assert_eq!(app.state().editor.to_markdown(), "- xalpha beta\n");
}

#[test]
fn an_empty_editor_shows_its_placeholder() {
    let mut app = app();
    app.render_once();
    let tree = app.accessibility_tree().unwrap();
    fn find(n: &crate::core::AccessNode) -> Option<&crate::core::AccessNode> {
        if n.role == crate::core::Role::TextField {
            return Some(n);
        }
        n.children.iter().find_map(find)
    }
    let f = find(&tree).unwrap();
    assert_eq!(f.name, "");
    assert_eq!(f.placeholder.as_deref(), Some("Write here"));
}
