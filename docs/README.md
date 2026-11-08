# Stipple docs

Guides for building, packaging and running Stipple apps. The [README](../README.md)
covers what Stipple is; the [roadmap](../ROADMAP.md) covers where it's going; API
docs are on [docs.rs](https://docs.rs/stipple).

## Platforms

How to build, package and run on each platform, and what works there today.

- [Linux](platforms/linux.md): Wayland and X11
- [Windows](platforms/windows.md)
- [macOS](platforms/macos.md)
- [iOS](platforms/ios.md)
- [Android](platforms/android.md)
- [Web](platforms/web.md): `wasm32` and a `<canvas>`

### What works where

| | Linux | Windows | macOS | iOS | Android | Web |
|---|---|---|---|---|---|---|
| Window and rendering | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Pointer input | ✅ | ✅ | ✅ | ✅ touch | ✅ touch, mouse ¹⁹ | ✅ ¹² |
| Keyboard input | ✅ | ✅ | ✅ | ❌ | ✅ ¹⁹ | ✅ ¹² |
| Scroll wheel and trackpad | ✅ ⁶ | ✅ ⁶ | ✅ ⁶ | ❌ | mouse wheel ¹⁰ | ✅ ⁶ |
| Modifiers and shortcuts (Ctrl/Cmd, Shift-select) | ✅ ⁶ | ✅ ⁶ | ✅ ⁶ | ❌ | ✅ ¹⁰ | ✅ ¹² |
| Timers and background updates | ✅ | ✅ | ✅ | ✅ ³ | ✅ ¹⁹ | ✅ ¹² |
| Display-paced animation (spinner, hover fades) | ✅ ¹¹ | ✅ ¹¹ | ✅ ¹¹ | ✅ ³ | ✅ ¹⁹ | ✅ ¹² |
| High-DPI | ✅ | ✅ ¹ | ✅ ¹ | ✅ | ✅ ¹⁹ | ✅ ¹² |
| OS-drawn controls (`Theme::native`) | — ² | ✅ | ✅ | ✅ | ❌ | ❌ |
| System dark mode and accent | ✅ | ✅ | ✅ | ✅ | ✅ ²⁰ | ✅ ²¹ |
| Following appearance changes live | ✅ ⁷ | ✅ ⁷ | ✅ ⁷ | ❌ | ✅ ²⁰ | ✅ ²¹ |
| System clipboard | ✅ ⁸ | ✅ ⁴ | ✅ ⁴ | ✅ ³ | ✅ ²⁰ | ✅ ¹³ |
| File dialogs | ✅ | ✅ ⁵ | ✅ ⁵ | ❌ | ❌ | ✅ ¹⁴ |
| Accessibility | AT-SPI | UI Automation | NSAccessibility | ❌ | ❌ | ARIA ²² |
| Window size limits, maximize, fullscreen | ✅ ⁹ | ✅ ¹⁰ | ✅ ¹⁰ | — | fullscreen ¹⁰ | ❌ |
| Window icon | ✅ ⁹ | ✅ ¹⁰ | Dock icon ¹⁰ | — | — | ❌ |
| Mouse cursor shapes | ✅ ⁹ | ✅ ¹⁰ | ✅ ¹⁰ | — | — | ✅ ¹² |
| Right-to-left and bidi text | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ |
| Mirrored right-to-left layouts | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ |
| Input methods (CJK IME) | Wayland ¹⁶ | ✅ ¹⁶ | ✅ ¹⁶ | ❌ | ❌ | ✅ ²³ |
| Dead keys and compose | ✅ ¹⁷ | ✅ | ✅ ¹⁶ | ❌ | ❌ | ✅ ²³ |
| Menu bar | drawn ¹⁸ | native ²⁴ | native ²⁴ | ❌ | ❌ | ❌ |
| Context menus and shortcuts | ✅ ¹⁸ | native ²⁴ | native ²⁴ | ❌ | ❌ | ❌ |

¹ Implemented, but CI only has 1x displays, so it hasn't been seen on a real high-DPI one.
² Linux has no native UI framework; controls stay self-drawn in the system's colors.
³ Implemented, but not yet run in CI on iOS.
⁴ CI round-trips text through the OS clipboard, and Ctrl/Cmd+C/X/V reach it now
that these backends report modifier keys.
⁵ Implemented, but a modal dialog can't be driven headlessly, so CI only builds it.
⁶ CI drives these with scripted input on X11 only; on Wayland, Windows, macOS and
the web they're implemented and unit-tested but not yet exercised end to end.
⁷ Implemented, but CI can't switch the system's appearance, so it's untested end
to end. Linux needs the desktop portal (its `SettingChanged` signal).
⁸ Wayland copies and pastes, and CI round-trips text with `wl-copy`/`wl-paste`.
X11 serves copies to other apps, but pastes only text copied in the app.
⁹ CI checks the size hints, icon and cursor changes on X11. On Wayland the icon
needs `xdg-toplevel-icon-v1` and cursor shapes `cursor-shape-v1`; without them
the compositor's defaults stay.
¹⁰ Implemented, but CI only builds it.
¹¹ CI checks that `asyncdemo`'s spinner animates and then stops. X11 paces
frames with a timer at the RandR refresh rate; Wayland, Windows and macOS use
frame callbacks, `DwmFlush` and `CVDisplayLink`, which CI exercises but can't
time precisely.
¹² CI drives `webdemo` with scripted DOM events in headless Chrome, at a pixel
ratio of 2, and checks what the app saw: clicks, the cursor, typing and Ctrl
shortcuts, timers, the spinner animating and then stopping, and resizing.
¹³ Through the browser's `paste`, `copy` and `cut` events (and
`navigator.clipboard.writeText`): Ctrl/Cmd+V pastes, but the app can't read the
clipboard on its own. CI round-trips text with synthetic clipboard events.
¹⁴ `open_file_contents` (an `<input type=file>`, answered asynchronously) and
`save_file_contents` (a download). `open_file` and friends return `None`, since
a page never sees paths. CI checks both with the picker and download stood in
for. See [the web page](platforms/web.md#file-dialogs).
¹⁵ Laid out by the shared CPU text path, so the same everywhere; CI screenshots
`bididemo` on X11. Editing it needs keyboard input, so not on iOS
yet. See [bidi.md](bidi.md).
¹⁶ Implemented (`text-input-v3`, IMM32, `NSTextInputClient`), but CI can't run
a real input method, so only the protocol wiring is tested. X11 has no IME
support yet (XIM or IBus). See [ime.md](ime.md).
¹⁷ From the locale's Compose file, on X11 and Wayland. CI types dead keys and a
compose sequence with `xdotool` (X11) and `wtype` (sway).
¹⁸ Linux has no system menu bar, so Stipple draws one in the window, with
mnemonics underlined while the keyboard drives it. CI opens it from the
keyboard and with a click, checks Alt underlines the mnemonics, uses a
shortcut, and picks from a context menu on X11.
¹⁹ CI runs `androiddemo` on an API 34 emulator (at its 2.625x density) and
checks that a frame is presented, its timer ticks while the spinner turns, a tap
lands on the button, and Tab plus `adb shell input text` type into the field.
The soft keyboard shows for a focused text input and types through key events.
²⁰ Implemented and tried by hand on an emulator (night mode switched with
`adb shell cmd uimode night yes`, the clipboard read and written over JNI), but
not checked in CI. The accent color needs Android 12.
²¹ `prefers-color-scheme`, and its `change` event; the accent is the CSS
`AccentColor`, where the browser has one (Chrome reports its own default blue,
not the OS's). CI runs `webdemo`'s selftest with a dark and a light scheme and
checks the theme follows; a live switch is checked by handing the app a new
scheme the way the `change` listener does, since headless Chrome can't flip it.
²² An invisible overlay of ARIA elements mirrors the accessibility tree over the
canvas. CI checks the roles and names, that a click on an element (what a
screen reader's "activate" does) taps the node, and `aria-checked`. Not yet
tried with a real screen reader. See [the web page](platforms/web.md#accessibility).
²³ Through a hidden `<textarea>` at the caret: composition events and `input`
events. CI checks them with synthetic events in headless Chrome; no real input
method was run.
²⁴ The `HMENU` / `NSMenu` menu bar, and context menus as native popups
(`TrackPopupMenuEx`, `NSMenu`). On Windows, CI reads the menu bar back, picks
from it with `WM_COMMAND` and from the keyboard, uses a shortcut, and picks
from a native context menu; on macOS an in-process self-test prints the menu
bar, picks from it, posts shortcuts (checking each runs once, ⌘C in a text
field included) and picks from a native context menu. See [menus.md](menus.md).
²⁵ Done by the shared layout pass, so the same everywhere. CI checks the
mirrored `bididemo --mirror` rendering on X11, Windows and macOS, screenshots its
window on all three, and drags its slider on X11; on iOS, Android and the web it
is only unit-tested. See [bidi.md](bidi.md#mirrored-layouts).

Where the clipboard isn't wired up, copy and paste still work inside the app.
Where file dialogs aren't, `open_file` and friends return `None`, as if cancelled.

## Topics

- [Layout](layout.md): stacks, wrapping, grids, and min/max sizes
- [Lists, tables and trees](lists-and-tables.md): virtualized rows, selection, sorting, resizing
- [Widgets](widgets.md): select, number input, split pane, toolbar and toasts
- [Menus](menus.md): the menu bar, shortcuts and context menus
- [Timers and background work](async.md): `AppHandle`, threads and tokio
- [Animation](animation.md): the display-driven frame clock, the spinner and hover fades
- [Text editing](text-editing.md): keyboard shortcuts, undo, double- and triple-click
- [Right-to-left and bidirectional text](bidi.md): Hebrew, Arabic, mixed lines, editing them, and mirrored right-to-left layouts
- [Input methods, dead keys and compose](ime.md): CJK IMEs, preedit, and Compose files
- [The native theme](native-theme.md): `Theme::native()` and OS-drawn controls
- [Images and SVG](images.md): the `image` and `svg` widgets, formats, and oxideav interop
- [Window control and cursors](windows-and-cursors.md): size limits, maximize, fullscreen, the icon, and cursor shapes
- [High-DPI](high-dpi.md): logical and physical pixels, and where the scale comes from
- [CI and screenshots](ci.md): what CI checks, and how screenshots reach this repository
