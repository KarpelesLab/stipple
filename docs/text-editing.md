# Text editing

`text_editor` and `text_area` keep their text in an `EditBuffer`, which handles
the caret, the selection, the clipboard and undo. This page lists the keys and
clicks they understand.

## Keyboard shortcuts

Word movement uses Ctrl on Windows and Linux and Option (Alt) on macOS,
following each platform.

| Action | Windows and Linux | macOS |
|---|---|---|
| Move by character / line | Arrow keys | Arrow keys |
| Start / end of line | Home / End | Home / End |
| Move by word | Ctrl+Left / Ctrl+Right | Option+Left / Option+Right |
| Extend the selection | Shift + any of the above | Shift + any of the above |
| Delete the word before / after the caret | Ctrl+Backspace / Ctrl+Delete | Option+Backspace / Option+Delete |
| Select all | Ctrl+A | Cmd+A |
| Copy / cut / paste | Ctrl+C / Ctrl+X / Ctrl+V | Cmd+C / Cmd+X / Cmd+V |
| Undo | Ctrl+Z | Cmd+Z |
| Redo | Ctrl+Y or Ctrl+Shift+Z | Cmd+Shift+Z |

Left and Right move the caret visually, so in right-to-left text Left moves
forward through the text; word movement, Home and End follow the text's logical
order. See [Right-to-left and bidirectional text](bidi.md).

A word is a run of letters and digits (in any script) and underscores; a run of
punctuation counts as a word of its own, and whitespace separates words. Moving
right stops at the end of a word, moving left at its start.

## Undo

Typing a run of characters is one undo step, as is a run of Backspace or Delete
presses. Moving the caret, changing the selection, or switching to a different
kind of edit starts a new step. Pasting, cutting, Enter, deleting a word, and
typing over a selection are each a step of their own. Undo puts the selection
back as it was. A new edit after an undo clears the redo history.

Your code can also call `undo()`, `redo()`, `can_undo()` and `can_redo()` on the
buffer, for example from a toolbar button, and `clear_history()` after loading
new text.

## Mouse

- Click to place the caret, drag to select.
- Double-click selects the word under the pointer (or the run of punctuation or
  spaces).
- Triple-click selects the line in a `text_area`, and the whole field in a
  `text_editor`.

Clicks count as one double- or triple-click when they land within 4 logical
pixels of each other and less than 500 ms apart. Stipple doesn't read the
system's double-click setting yet.

The app delivers double- and triple-clicks to the focused element as
`KeyInput::SelectWord` and `KeyInput::SelectLine`, so a custom key handler can
handle them too.

## The clipboard

Ctrl/Cmd+C, X and V go through an in-process mirror
(`stipple::core::clipboard_text`), which the app syncs with the system
clipboard around each copy and paste, so copying and pasting work inside the
app even where the system clipboard isn't wired up (and headlessly). On X11 the
system answers a paste later when another application owns the clipboard: the
paste then lands when the content arrives (`Event::Clipboard`), if the same
field still has focus.

**The primary selection.** Whatever text is selected in the focused field
becomes the primary selection, and a middle click on editable text puts the
caret there and inserts it, as on Linux desktops. On X11 and Wayland it is the
system's `PRIMARY` selection, shared with other applications; elsewhere the app
keeps it in process (`stipple::core::primary_text`). `App::paste_primary_at`
does the middle click's paste from code and tests.

**Rich content.** `stipple::platform::clipboard::ClipboardContent` holds plain
text, HTML and an image (a `Pixmap`) at once. A handler that isn't a text
field, a terminal say, writes one with `stipple::core::write_clipboard` and
asks for one with `stipple::core::request_clipboard`; the app hands the answer
to the function set with `App::on_clipboard`:

```rust,ignore
use stipple::core::{request_clipboard, write_clipboard};
use stipple::platform::clipboard::{ClipboardContent, ClipboardFormats, ClipboardKind};

// Copy as text and HTML.
write_clipboard(
    ClipboardKind::Clipboard,
    ClipboardContent::text("ls -l").with_html("<pre>ls -l</pre>"),
);
// Middle click in the terminal: paste the primary selection.
request_clipboard(ClipboardKind::Primary, ClipboardFormats::TEXT);

let app = App::new(state, view).on_clipboard(|state, _kind, content| {
    if let Some(text) = content.and_then(|c| c.text) {
        state.pty_write(&text);
    }
});
```

On X11 and Wayland the types are MIME types (`text/plain;charset=utf-8`,
`UTF8_STRING`, `text/html`, `image/png`); on Windows `CF_UNICODETEXT`,
`HTML Format` (`CF_HTML`), `CF_DIBV5` and `PNG`; on macOS `public.utf8-plain-text`,
`public.html` and `public.png`. The web takes text and HTML, iOS and Android
text only. `stipple::platform::clipboard::format` has the encoders (`CF_HTML`,
DIB, PNG).

## Input methods

Chinese, Japanese and Korean input methods, dead keys and compose sequences show
their composition at the caret, underlined, until it is committed; the
composition isn't part of the text or the undo history until then. See
[ime.md](ime.md).
