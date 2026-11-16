# Text editing

`text_field` (one line) and `text_area` (several) edit a `TextInput` kept in
the app state (for formatted text, see [rich text editing](rich-text-editing.md)). Its `EditBuffer` handles the caret, the selection, the
clipboard and undo. This page lists the keys and clicks they understand.

<!-- live: textarea -->

```rust
struct Form { name: TextInput, notes: TextInput, names: Vec<String> }
let form = Form {
    name: TextInput::new().max_length(40),
    notes: TextInput::from_text("Read-only notes").read_only(true),
    names: Vec::new(),
};
// in the view:
text_field(cx, &state.name, |s| &mut s.name)
    .placeholder("Your name")
    .on_submit(cx, |s| {
        let name = s.name.take();
        s.names.push(name);
    })
    .on_change(cx, |s| println!("now {:?}", s.name.text()));
text_area(cx, &state.notes, |s| &mut s.notes);
```

- `.placeholder("Search")` on a field (these two, and `password_field`) shows
  a hint, muted, while it's empty and nothing is being composed in it; screen
  readers get it as the field's placeholder: its description on AT-SPI,
  `AXPlaceholderValue` on macOS, `HelpText` in UI Automation,
  `accessibilityHint` on iOS, the hint text on Android (8.0 and later) and
  `aria-placeholder` on the web. A hint longer than the field is clipped.
- `.on_submit(cx, ..)` runs on Enter in a `text_field` (and a
  `password_field`); a `text_area` takes Enter as a new line.
- `.on_change(cx, ..)` runs after each edit that changes the text, with the
  new text already in the state.
- `TextInput::max_length(n)` cuts typing and pasting at `n` characters;
  `read_only(true)` keeps selecting and copying but refuses edits.
- `text()`, `set_text()`, `clear()` and `take()` read and replace the text;
  `buffer()` and `buffer_mut()` reach the `EditBuffer` (caret, selection,
  direction, undo).

## Keyboard hints

On a phone the field picks the on-screen keyboard. Tell it what the field
takes:

```rust
text_field(cx, &state.code, |s| &mut s.code)
    .placeholder("EAN-13")
    .input_kind(InputKind::Number)      // a number pad
    .enter_action(EnterAction::Done)    // the action key says Done
    .on_submit(cx, |s| lookup(s.code.text()));
text_field(cx, &state.email, |s| &mut s.email)
    .input_kind(InputKind::Email)
    .autofill(Autofill::Email)
    .enter_action(EnterAction::Next);   // Return moves on to the next field
```

- `.input_kind(..)`: `Text` (the default), `Number` (digits: barcodes,
  PINs, postal codes), `Decimal`, `Phone`, `Email`, `Url` or `Search`. Only
  `Text` is capitalized and corrected by default; the other kinds type
  exactly what was pressed. It's a hint, not a filter: a hardware keyboard
  or a paste can still put any text in the field.
- `.autocapitalize(Autocapitalize::Off | Characters | Words | Sentences)`
  and `.autocorrect(false)` override the kind's defaults (a code typed in
  capitals: `Characters` and `autocorrect(false)`). A password never gets
  either, whatever is asked.
- `.enter_action(EnterAction::Done | Go | Next | Search | Send)`: the action
  key's label. Each submits the field (its `on_submit` handlers run, as
  Enter does). `Next` then moves focus to the next field, like Tab, on every
  platform (Enter on a desktop too). In a `text_area` Enter stays a new line,
  and only `Next` takes effect (moving on without one). `Search` is the
  default for `InputKind::Search`.
- `.autofill(..)`: what the field holds, for the system's autofill
  (`Username`, `Password`, `NewPassword`, `OneTimeCode`, `Email`, `Phone`,
  `Name`, `PostalCode`, `StreetAddress`, `Url`): iOS's `textContentType` and
  the web's `autocomplete`.
- On a `password_field` the same hints apply, masked: `.input_kind(Number)`
  makes a PIN pad.

| | Keyboard | Capitals and corrections | Action key | Autofill |
|---|---|---|---|---|
| Android | `EditorInfo.inputType`: `TYPE_CLASS_NUMBER` (with `FLAG_DECIMAL`), `TYPE_CLASS_PHONE`, `TYPE_CLASS_TEXT` with the e-mail or URI variation; the password variations for a password field | `TYPE_TEXT_FLAG_CAP_*`, `AUTO_CORRECT` or `NO_SUGGESTIONS` | `imeOptions`: `IME_ACTION_DONE` (the default), `GO`, `NEXT`, `SEARCH`, `SEND`; a text area `IME_FLAG_NO_ENTER_ACTION` | not passed |
| iOS | `keyboardType`: number pad, decimal pad, phone pad, e-mail, URL, web search; `secureTextEntry` for a password field | `autocapitalizationType`, `autocorrectionType`, `spellCheckingType` | `returnKeyType`: Done, Go, Next, Search, Send (Return by default) | `textContentType` |
| Web | the hidden textarea's `inputmode` (`numeric`, `decimal`, `tel`, `email`, `url`, `search`) | `autocapitalize`, `autocorrect`, `spellcheck` | `enterkeyhint` | `autocomplete` |
| Desktops | ignored: there is no on-screen keyboard | ignored | `Next` moves focus on Enter | ignored |

When focus moves to a field with other hints, the keyboard changes with it
(Android restarts the input connection, iOS reloads the input views).

## The on-screen keyboard

While the on-screen keyboard is up, the App lays the view out in the part
of the window above it, as if the window were that much shorter, and keeps
the focused field in view: each scroll container around it scrolls just far
enough to show the field (or its caret's line, when the field is taller
than the room left). It does so again whenever focus moves, the caret moves
or the keyboard changes size, so a field the user scrolled away stays there
until they type. `Cx::keyboard_inset()` gives the keyboard's height over
the window's bottom in logical pixels (zero while it's hidden, and on
desktops), for a view that wants to change while the user types (hide a tab
bar); `Cx::safe_area()`'s bottom shrinks by it, since the home indicator is
under the keyboard.

- **iOS**: from `UIKeyboardWillChangeFrameNotification` and
  `UIKeyboardWillHideNotification`, the keyboard's end frame converted to the
  view's coordinates (a floating or split iPad keyboard that doesn't reach
  the bottom covers nothing).
- **Android**: the `ime()` window insets (the system window insets before
  Android 11), both edge to edge and not. The app's content no longer shrinks
  for the keyboard in the backend; the App lays the view out above it as on
  iOS. On older Androids that resize the window themselves, the inset is
  zero.
- **Web**: `visualViewport`: what of the canvas lies below the visual
  viewport's bottom (a phone browser shrinks the visual viewport, not the
  layout viewport, for the keyboard).

A raw key hook isn't part of a field: an element of your own can take keys
with `Element::on_key`, and `Element::on_raw_key` sees physical keys (see
[keyboard.md](keyboard.md)).

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
buffer (`input.buffer_mut()`), for example from a toolbar button, and `clear_history()` after loading
new text.

## Mouse

- Click to place the caret, drag to select.
- Double-click selects the word under the pointer (or the run of punctuation or
  spaces).
- Triple-click selects the line in a `text_area`, and the whole field in a
  `text_field`.

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
    ClipboardContent::from_text("ls -l").html("<pre>ls -l</pre>"),
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
text only. The crate-internal `platform::clipboard::format` has the encoders (`CF_HTML`,
DIB, PNG).

## Selectable text

Labels and paragraphs aren't editable, but `.selectable()` lets the user
select and copy them: a decoded barcode, an address, an error's details.

<!-- live: richtext -->

```rust
paragraph(cx, "4006381333931").selectable();
// On a container: every text inside it.
column(vec![label(cx, "Order 1042"), label(cx, address)]).selectable();
```

- **Mouse.** Drag across the text to select; double-click selects a word and
  triple-click a line (a paragraph's line between line breaks), with the same
  words and timing as a text field. The pointer is an I-beam over it.
- **Keys.** Once the text has a selection (or a click in it), Shift+arrows,
  Shift+Home/End and Shift+Option/Ctrl+arrows extend it, Ctrl/Cmd+A selects
  all of that text, Ctrl/Cmd+C copies, and Escape clears it. A focused text
  field keeps its own keys: clicking selectable text takes the focus away
  from the field, as on the web.
- **Context menu.** A right-click shows Copy and Select All (on the word
  under the pointer, unless it is inside the selection). An element with a
  context menu of its own keeps that one.
- **Touch.** A finger scrolls rather than selects; a long press selects the
  word under it and opens the same menu. There are no drag handles to adjust
  the selection yet: Select All, then Copy, takes the whole text.
- **One selection per window**, inside one element: a press anywhere else in
  the window (not in the menu) clears it, and it is forgotten when the
  element's text changes. It is found again across rebuilds by the element's
  place in the tree, or by its `.key(..)` when it moves.
- **Copying** writes plain text: rich text's images become their
  alternative text (`InlineIcon::alt`). On X11 and Wayland the selection is
  also the primary selection.
- **Screen readers** get the text as before, and the selection as the text's
  selection over AT-SPI (`GetSelection`, `text-selection-changed`). UI
  Automation and NSAccessibility read the text but not the selection yet.

The highlight is the theme's `palette.selection`, drawn under the glyphs.

## Input methods

Chinese, Japanese and Korean input methods, dead keys and compose sequences show
their composition at the caret, underlined, until it is committed; the
composition isn't part of the text or the undo history until then. See
[ime.md](ime.md).
