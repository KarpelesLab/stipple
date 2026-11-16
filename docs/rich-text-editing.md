# Rich text editing

`rich_editor` edits formatted text: paragraphs, headings, lists, quotes and
code blocks holding bold, italic, underlined, struck-through and code text,
links and images. It keeps a `RichEditor` in the app state, as `text_area`
keeps a `TextInput`, and the document it holds exports to Markdown and HTML
(and imports from both).

<!-- live: richeditor -->

```rust
use stipple::prelude::*;
use stipple::widgets::Marks;

struct Doc { editor: RichEditor }

fn view(state: &Doc, cx: &mut Cx<Doc>) -> Element {
    let marks = state.editor.marks();
    let bold = ToolButton::new("B")
        .selected(marks.contains(Marks::BOLD))   // pressed while the caret is in bold text
        .view(cx)
        .keeps_focus()                           // the editor keeps the focus
        .on_tap(cx, |s| s.editor.toggle_mark(Marks::BOLD));
    let list = ToolButton::new("•")
        .selected(state.editor.block_kind() == BlockKind::BulletItem)
        .view(cx)
        .keeps_focus()
        .on_tap(cx, |s| s.editor.toggle_block_kind(BlockKind::BulletItem));
    column(vec![
        toolbar(cx).children(vec![bold, list]),
        rich_editor(cx, &state.editor, |s| &mut s.editor)
            .placeholder("Write something…")
            .on_change(cx, |s| println!("{}", s.editor.to_markdown())),
    ])
}
```

`examples/richeditor` is a complete editor: a toolbar for every mark and
block kind, links, an image from a file or a built-in sample (in the line,
or floated left or right), undo and redo,
and the Markdown or HTML under the editor, saved to a file.

## The document

A `RichDocument` is a list of blocks. Each block has a `BlockKind`:

| Kind | Markdown | HTML |
|---|---|---|
| `Paragraph` | a paragraph | `<p>` |
| `Heading(1..=3)` | `#`, `##`, `###` | `<h1>`–`<h3>` |
| `BulletItem` | `- item` | `<ul><li>` |
| `NumberedItem` | `1. item` | `<ol><li>` |
| `Quote` | `> text` | `<blockquote><p>` |
| `CodeBlock` | a fenced block | `<pre><code>` |

Consecutive list items make one list (numbered from 1), consecutive quote
paragraphs one quote, and consecutive code lines one code block: each line
of a code block is a block of its own in the model.

Inside a block, text carries `Marks` (`BOLD`, `ITALIC`, `UNDERLINE`,
`STRIKETHROUGH`, `CODE`, combined with `|`) and optionally a link, and
images sit in the line. `RichDocument::blocks()` lists them as `Block`s of
`Inline`s, and `RichDocument::from_blocks` builds a document from them.

## Editing

Everything a `text_area` does works the same way: the caret and selection
by keyboard and pointer, word movement, double- and triple-click,
undo and redo, input methods, and right-to-left text (see
[text-editing.md](text-editing.md)). On top of that:

| Action | Windows and Linux | macOS |
|---|---|---|
| Bold, italic, underline | Ctrl+B, Ctrl+I, Ctrl+U | Cmd+B, Cmd+I, Cmd+U |
| Undo, redo | Ctrl+Z, Ctrl+Y or Ctrl+Shift+Z | Cmd+Z, Cmd+Shift+Z |

- **Typing** takes the marks of the character before the caret (after it at
  a block's start). A link continues only inside it: typing at its end
  doesn't extend it.
- **Toggling a mark** (`toggle_mark`) applies to the selection: it takes the
  mark off if all of the selection has it, else puts it on. With no
  selection it applies to what is typed next, until the caret moves.
- **Block kinds** (`set_block_kind`, `toggle_block_kind`) apply to every
  block the selection touches, or the caret's.
- **Enter** splits the block. After a heading the new block is a paragraph;
  a list item, quote or code line continues as one. Enter on an *empty*
  list item or quote turns it into a paragraph, ending the list.
- **Backspace at a block's start** turns a heading, list item or quote into
  a paragraph; on a paragraph (or a code line after another) it joins the
  block to the one before. Delete at a block's end joins the next one.
- **Undo** covers every edit: typing (a run of it is one step), deleting,
  marks, block kinds, links, images and pastes. It restores the selection.
- **Links**: `set_link(Some(url))` links the selection; at a caret inside a
  link it changes the whole link, and `set_link(None)` removes it.
- **Images**: `insert_image(RichImage)` puts one at the caret.
- `marks()`, `block_kind()` and `link()` say what is in effect at the caret
  (all of the selection, for marks): what a toolbar shows as pressed.

`Element::keeps_focus()` on a toolbar button keeps the keyboard focus (and
the caret and selection) in the editor when the button is clicked, as
native toolbars do; without it a click takes the focus off the editor.

The editor grows with its text from a minimum of 160 logical pixels; put it
in a `scroll` for long documents. It shows list bullets and numbers, a bar
before quotes, larger bold headings, and code in the system's monospaced
font (where there is one: not on the web, where code is only colored).
Up and Down move between blocks, keeping the byte column, as in a
`text_area`; they don't move by wrapped line yet.

## Images

A `RichImage` keeps its source string, `src` (a file path, a URL or a
`data:` URI), its alternative text, and the decoded `Pixmap` the editor
shows (a gray square while there is none).

- `RichImage::from_pixmap(px)` makes the `src` a `data:image/png;base64,…`
  URI of the picture: the document carries it wherever it is exported. A
  pasted image becomes one of these.
- `RichImage::load(src)` decodes a `data:` URI, or (not on the web) reads a
  file path or `file://` URL.
- `RichImage::new(src, pixmap)` keeps both as given: the example inserts a
  picked file under its file name, so the export refers to it by name.

**Floating.** An image sits in the line by default (`ImageFloat::Inline`).
`set_image_float(ImageFloat::Left)` (or `Right`) floats the selected
images, or the one next to the caret, against that edge of the editor, the
text flowing around its opaque pixels with a 6 px margin (the text flow
engine, [text-flow.md](text-flow.md)): the transparent parts of a PNG let
text in. `image_float()` says how the image at the caret is placed, for a
toolbar's pressed state; `RichImage::floated` sets it on a new one. The
image still stands as one U+FFFC in the text where it was inserted, so the
caret, selection, undo and copying treat it as before. HTML exports it as
`<img … style="float: left">` and reads that or `align="left"`; Markdown,
which has no floats, as the same inline `<img>` tag.

**Export uses `src` as it is**: Markdown writes `![alt](src)` and HTML
`<img src="src" alt="alt">`. Nothing is re-encoded or embedded behind your
back, so an image from a path stays a path; make it a `data:` URI
(`from_pixmap`) when the export must stand alone. Importing decodes `data:`
URIs only; call `RichDocument::load_images()` to read the files too (it's
yours to decide whether a document may read local files).

## Markdown

`to_markdown()` writes CommonMark, with GitHub's `~~strikethrough~~`:

- every character Markdown would read as syntax is escaped with a
  backslash (`*`, `_`, `` ` ``, `[`, `]`, `<`, `>`, `~`, `&`, `\`, and
  `#`, `-`, `+`, `=` or a list number at a line's start);
- marks are taken off the spaces at their ends (`** bold **` isn't bold in
  Markdown, so the spaces move outside);
- each line is read back as it is written: where a delimiter would be read
  differently (`**"quoted"**word`, next to punctuation), that line's marks
  are written as HTML tags instead (`<strong>`, `<em>`, `<del>`), which
  CommonMark allows inline; underline is always `<u>`, as Markdown has none;
- code spans use as many backticks as it takes, code blocks a fence longer
  than any run of backticks inside;
- an empty paragraph is `<br>`, and spaces at either end of a block
  `&#32;`, so neither is lost.

`RichDocument::from_markdown` reads ATX and setext headings (levels 4–6 as
3), paragraphs, lists (nested ones flattened into one level, `*`, `+` and
`1)` too), quotes, fenced and indented code, thematic breaks (dropped),
emphasis by CommonMark's delimiter rules, strikethrough, code spans, links
and autolinks, images, backslash escapes, character references and the
inline HTML tags the export writes (`<u>`, `<strong>`, `<b>`, `<em>`, `<i>`,
`<del>`, `<s>`, `<code>`, `<a href>`, `<img>`, `<br>`). Other HTML is
dropped, and a hard line break reads as a space (a block is one line).

## HTML

`to_html()` writes one element per line: `<p>`, `<h1>`–`<h3>`, `<ul>`/`<ol>`
around `<li>`s, `<blockquote>` around `<p>`s, `<pre><code>` for code, and
`<strong>`, `<em>`, `<u>`, `<s>`, `<code>`, `<a href>` and `<img>` inside,
`&`, `<`, `>` (and `"` in attributes) escaped. Spaces HTML would collapse
(at a block's ends, or a second one in a row) are written as `&nbsp;`;
empty blocks hold a `<br>`.

`RichDocument::from_html` reads that, and what browsers and other editors
put on the clipboard: `<b>`, `<i>`, `<del>`, `<strike>`, `<ins>`, `<kbd>`,
`<div>` and `<br>` as line breaks, `h4`–`h6` (as level 3), `style`
attributes with `font-weight`, `font-style`, `text-decoration` and a
monospace `font-family` (Google Docs' `<b style="font-weight:normal">`
wrapper is not bold), white space collapsed as a browser does, no-break
spaces as spaces, and table cells as paragraphs. `<script>`, `<style>`,
`<head>` and comments are skipped; any other tag keeps its text.

## The clipboard

Copying puts the selection on the clipboard as plain text (list items with
their bullet or number, images as their alternative text) and as HTML
(`ClipboardContent`). Pasting takes, in this order, the clipboard's HTML
(read as above; a fragment this app copied is pasted back exactly, images
and all), an image (inserted with a `data:` URI), or its text (new blocks
for its lines, in the marks at the caret).

Before a paste into an element that takes rich content (a `rich_editor`),
the app reads every format from the system clipboard, not just text; the
handler reads them with `stipple::core::clipboard_content()`. Which formats
each platform carries is in [text-editing.md](text-editing.md#the-clipboard):
text, HTML and images on X11, Wayland, Windows and macOS; text and HTML on
the web; text on iOS and Android.

## Accessibility

Screen readers see a multi-line text field (`Role::TextField`, multi-line
in its input hints) whose text is what the editor shows, bullets and
numbers included, with the caret and selection in it, through the same
bridges as a `text_area` (AT-SPI, UI Automation, NSAccessibility,
UIAccessibility, Android, ARIA). An image reads as U+FFFC, the object
replacement character, so the caret's index stays right; the marks and
links aren't reported as text attributes yet.
