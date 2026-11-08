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
