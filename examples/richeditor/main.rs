//! A rich text editor (`rich_editor`) with a formatting toolbar: bold,
//! italic, underline, strikethrough and code; links; headings, lists, quotes
//! and code blocks; images (picked from a file, or a built-in sample);
//! undo and redo. The document exports to Markdown and HTML, shown under the
//! editor and saved to a file.
//!
//! The toolbar's buttons keep the keyboard focus in the editor
//! (`Element::keeps_focus`) and show the marks at the caret as pressed.
//! Ctrl/Cmd+B, I and U work from the keyboard too.
//!
//! `--check` builds a document through the same calls the toolbar makes,
//! renders the window off screen, and checks the picture and the exports.
//! With `STIPPLE_RICHEDITOR_ECHO=1` each change prints the Markdown (CI
//! types into the window and reads it back).

use std::cell::Cell;
use std::sync::Arc;

use stipple::platform::dialog::{self, FileDialog};
use stipple::prelude::*;
use stipple::widgets::Marks;

/// Which export shows under the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Output {
    Markdown,
    Html,
}

struct Doc {
    editor: RichEditor,
    link: TextInput,
    output: Output,
    status: String,
    /// The revision last echoed (STIPPLE_RICHEDITOR_ECHO).
    echoed: Cell<u64>,
}

const WELCOME: &str = "# Rich text\n\n\
Type here: **bold**, *italic*, <u>underlined</u>, ~~struck~~, `code`, and \
[links](https://github.com/KarpelesLab/stipple).\n\n\
- Lists\n- with items\n\n\
> A quote.\n";

impl Default for Doc {
    fn default() -> Self {
        Self {
            editor: RichEditor::from_markdown(WELCOME),
            link: TextInput::from_text("https://example.com"),
            output: Output::Markdown,
            status: String::new(),
            echoed: Cell::new(u64::MAX),
        }
    }
}

/// The built-in sample image: a sunset over the sea, 120×72.
fn sample_pixmap() -> Pixmap {
    let (w, h) = (120u32, 72u32);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let sea = y > h * 3 / 5;
            let (dx, dy) = (x as f64 - 60.0, y as f64 - 43.0);
            let sun = dx * dx + dy * dy < 15.0 * 15.0 && !sea;
            let t = y as f64 / h as f64;
            let px = if sun {
                [0xff, 0xd2, 0x3f, 0xff]
            } else if sea {
                [0x1d, 0x4e, 0x89, 0xff]
            } else {
                [
                    0xff,
                    (0x5e as f64 + 90.0 * t) as u8,
                    (0x8a as f64 - 60.0 * t) as u8,
                    0xff,
                ]
            };
            data.extend_from_slice(&px);
        }
    }
    Pixmap::from_rgba8(PhysicalSize::new(w, h), data)
}

fn sample_image() -> RichImage {
    RichImage::from_pixmap(sample_pixmap()).alt("A sunset")
}

/// A toolbar button acting on the editor: it keeps the editor focused, and
/// shows `on` as pressed.
fn tool(
    cx: &mut Cx<Doc>,
    label: &str,
    name: &str,
    on: bool,
    f: impl Fn(&mut Doc) + 'static,
) -> Element {
    ToolButton::new(label)
        .selected(on)
        .view(cx)
        .a11y_label(name)
        .keeps_focus()
        .on_tap(cx, f)
}

/// Pick an image file and insert it at the caret. Its file name is its
/// `src`: the exports refer to it by name, next to where they're saved.
fn insert_image_file(cx: &Cx<Doc>) -> impl Fn(&mut Doc) + 'static {
    let handle = cx.handle();
    move |_: &mut Doc| {
        let handle = handle.clone();
        let opts = FileDialog::new().title("Insert an image").accept("image/*");
        dialog::open_file_contents(&opts, move |file| {
            let Some(file) = file else { return };
            let image = Pixmap::decode(&file.bytes)
                .ok()
                .map(|px| RichImage::new(file.name.clone(), Some(Arc::new(px))).alt(&file.name));
            handle.update(move |s: &mut Doc| match image {
                Some(image) => {
                    s.editor.insert_image(image);
                    s.status = format!("Inserted {}", file.name);
                }
                None => s.status = format!("{} isn't an image I can read", file.name),
            });
        });
    }
}

fn save(s: &mut Doc) {
    let (name, text) = match s.output {
        Output::Markdown => ("document.md", s.editor.to_markdown()),
        Output::Html => ("document.html", s.editor.to_html()),
    };
    let opts = FileDialog::new().title("Save").file_name(name);
    s.status = if dialog::save_file_contents(&opts, text.as_bytes()) {
        format!("Saved {name}")
    } else {
        "Not saved".into()
    };
}

fn view(state: &Doc, cx: &mut Cx<Doc>) -> Element {
    if std::env::var_os("STIPPLE_RICHEDITOR_ECHO").is_some()
        && state.echoed.get() != state.editor.revision()
    {
        state.echoed.set(state.editor.revision());
        println!("richeditor: md={:?}", state.editor.to_markdown());
    }
    let ed = &state.editor;
    let marks = ed.marks();
    let kind = ed.block_kind();
    let mark = |cx: &mut Cx<Doc>, label: &str, name: &str, m: Marks| {
        tool(cx, label, name, marks.contains(m), move |s| {
            s.editor.toggle_mark(m)
        })
    };
    let block = |cx: &mut Cx<Doc>, label: &str, name: &str, k: BlockKind| {
        tool(cx, label, name, kind == k, move |s| {
            s.editor.toggle_block_kind(k)
        })
    };
    let formatting = toolbar(cx).children(vec![
        mark(cx, "B", "Bold", Marks::BOLD),
        mark(cx, "I", "Italic", Marks::ITALIC),
        mark(cx, "U", "Underline", Marks::UNDERLINE),
        mark(cx, "S", "Strikethrough", Marks::STRIKETHROUGH),
        mark(cx, "</>", "Code", Marks::CODE),
        tool_separator(cx),
        block(cx, "H1", "Heading 1", BlockKind::heading(1)),
        block(cx, "H2", "Heading 2", BlockKind::heading(2)),
        block(cx, "H3", "Heading 3", BlockKind::heading(3)),
        tool(cx, "¶", "Paragraph", kind == BlockKind::Paragraph, |s| {
            s.editor.set_block_kind(BlockKind::Paragraph)
        }),
        tool_separator(cx),
        block(cx, "•", "Bulleted list", BlockKind::BulletItem),
        block(cx, "1.", "Numbered list", BlockKind::NumberedItem),
        block(cx, "❝", "Quote", BlockKind::Quote),
        block(cx, "{ }", "Code block", BlockKind::CodeBlock),
        tool_separator(cx),
        tool(cx, "Undo", "Undo", false, |s| {
            s.editor.undo();
        }),
        tool(cx, "Redo", "Redo", false, |s| {
            s.editor.redo();
        }),
    ]);
    let pick = insert_image_file(cx);
    let insert = toolbar(cx).children(vec![
        text_field(cx, &state.link, |s| &mut s.link)
            .placeholder("https://…")
            .width(220.0),
        tool(cx, "Link", "Link the selection", ed.link().is_some(), |s| {
            let url = s.link.text().to_string();
            s.editor.set_link(Some(&url));
        }),
        tool(cx, "Unlink", "Remove the link", false, |s| {
            s.editor.set_link(None)
        }),
        tool_separator(cx),
        tool(cx, "Image…", "Insert an image file", false, pick),
        tool(cx, "Sample image", "Insert the sample image", false, |s| {
            s.editor.insert_image(sample_image())
        }),
    ]);
    let editor = rich_editor(cx, &state.editor, |s| &mut s.editor)
        .placeholder("Write something…")
        .min_height(300.0);
    let output_text = match state.output {
        Output::Markdown => ed.to_markdown(),
        Output::Html => ed.to_html(),
    };
    let tabs = toolbar(cx).children(vec![
        ToolButton::new("Markdown")
            .selected(state.output == Output::Markdown)
            .view(cx)
            .on_tap(cx, |s| s.output = Output::Markdown),
        ToolButton::new("HTML")
            .selected(state.output == Output::Html)
            .view(cx)
            .on_tap(cx, |s| s.output = Output::Html),
        tool_separator(cx),
        tool_button(cx, "Save…").on_tap(cx, save),
        label(cx, &state.status),
    ]);
    let theme = *cx.theme();
    let output = scroll(cx)
        .children(vec![
            Element::text(output_text, theme.font_size * 0.9, theme.palette.text)
                .wrap()
                .selectable(),
        ])
        .fill(theme.palette.surface)
        .border(theme.palette.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::uniform(theme.spacing.sm))
        .height(150.0);
    column(vec![
        formatting,
        insert,
        scroll(cx).children(vec![editor]).grow(1.0),
        tabs,
        output,
    ])
    .gap(theme.spacing.sm)
    .padding(Insets::uniform(16.0))
}

fn app() -> App<Doc> {
    App::new(Doc::default(), view)
        .title("Stipple Rich Editor")
        .theme(Theme::light())
        .logical_size(Size::new(820.0, 640.0))
}

/// Edit the welcome document as the toolbar would, render it, and check
/// the picture and the exports.
fn check(mut app: App<Doc>) -> Result<(), String> {
    app.render_once();
    // Into the editor (found by its placeholder), at the end of the text.
    let editor = app
        .find(Role::TextField, "Write something…")
        .ok_or("no editor")?;
    app.text_press_at(editor.bounds.center());
    app.end_text_select();
    app.press_key(KeyInput::SelectAll);
    app.press_key(KeyInput::Right);
    // A paragraph after the quote, typed, with a word made bold from the
    // toolbar; then the sample image from the toolbar.
    let click = |app: &mut App<Doc>, name: &str| {
        if app.click(Role::Button, name) {
            Ok(())
        } else {
            Err(format!("no {name:?} button"))
        }
    };
    app.press_key(KeyInput::Enter);
    click(&mut app, "Paragraph")?;
    app.type_text("Checked ");
    click(&mut app, "Bold")?;
    app.type_text("bold");
    click(&mut app, "Bold")?;
    app.type_text(" & <safe>");
    app.press_key(KeyInput::Enter);
    click(&mut app, "Insert the sample image")?;
    let pm = app.render_once();
    let md = app.state().editor.to_markdown();
    let html = app.state().editor.to_html();
    println!("richeditor: markdown:\n{md}");
    for want in [
        "# Rich text",
        "**bold**",
        "<u>underlined</u>",
        "~~struck~~",
        "`code`",
        "[links](https://github.com/KarpelesLab/stipple)",
        "- Lists",
        "> A quote.",
        "Checked **bold** \\& \\<safe\\>",
        "![A sunset](data:image/png;base64,",
    ] {
        if !md.contains(want) {
            return Err(format!("the Markdown has no {want:?}"));
        }
    }
    for want in [
        "<h1>Rich text</h1>",
        "<strong>bold</strong>",
        "<ul>\n<li>Lists</li>",
        "Checked <strong>bold</strong> &amp; &lt;safe&gt;",
        "<img src=\"data:image/png;base64,",
    ] {
        if !html.contains(want) {
            return Err(format!("the HTML has no {want:?}"));
        }
    }
    if RichDocument::from_markdown(&md) != *app.state().editor.document() {
        return Err("the Markdown doesn't read back as the document".into());
    }
    if RichDocument::from_html(&html) != *app.state().editor.document() {
        return Err("the HTML doesn't read back as the document".into());
    }
    // The sample image's sun and sea are drawn.
    let (mut sun, mut sea) = (0, 0);
    for y in 0..pm.size().height {
        for x in 0..pm.size().width {
            match pm.pixel(x, y) {
                Some([0xff, 0xd2, 0x3f, _]) => sun += 1,
                Some([0x1d, 0x4e, 0x89, _]) => sea += 1,
                _ => {}
            }
        }
    }
    println!("richeditor: {sun} sun pixels, {sea} sea pixels");
    if sun < 50 || sea < 200 {
        return Err("the inserted image isn't drawn".into());
    }
    Ok(())
}

fn main() {
    let app = app();
    if std::env::args().any(|a| a == "--check") {
        let app = match Font::system_default() {
            Some(font) => app.font(font),
            None => app,
        };
        match check(app) {
            Ok(()) => println!("richeditor: ok"),
            Err(e) => {
                println!("richeditor: FAILED: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    app.run();
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_render_and_export() {
        let Some(font) = Font::system_default() else {
            return;
        };
        check(app().font(font)).unwrap();
    }
}
