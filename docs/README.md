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
| Keyboard input | ✅ | ✅ | ✅ | ✅ ²⁷ | ✅ ¹⁹ | ✅ ¹² |
| Scroll wheel and trackpad | ✅ ⁶ | ✅ ⁶ | ✅ ⁶ | ❌ | mouse wheel ¹⁰ | ✅ ⁶ |
| Raw pointer events (every button, click counts, lines vs pixels, pointer leaving) | ✅ ³⁶ | ✅ ³⁶ | ✅ ³⁶ | touch | ✅ mouse ³⁶ | ✅ ³⁶ |
| Touch scrolling (drag, fling) and multi-touch events | — | — | — | ✅ ²⁷ | ✅ ¹⁹ | ❌ |
| Modifiers and shortcuts (Ctrl/Cmd, Shift-select) | ✅ ⁶ | ✅ ⁶ | ✅ ⁶ | hardware keyboard ¹⁰ | ✅ ¹⁰ | ✅ ¹² |
| Timers and background updates | ✅ | ✅ | ✅ | ✅ ³ | ✅ ¹⁹ | ✅ ¹² |
| Display-paced animation (spinner, hover fades) | ✅ ¹¹ | ✅ ¹¹ | ✅ ¹¹ | ✅ ³ | ✅ ¹⁹ | ✅ ¹² |
| High-DPI | ✅ | ✅ ¹ | ✅ ¹ | ✅ | ✅ ¹⁹ | ✅ ¹² |
| OS-drawn controls (`Theme::native`) | — ² | ✅ | ✅ | ✅ | ❌ | ❌ |
| System dark mode and accent | ✅ | ✅ | ✅ | ✅ | ✅ ²⁰ | ✅ ²¹ |
| Following appearance changes live | ✅ ⁷ | ✅ ⁷ | ✅ ⁷ | ❌ | ✅ ²⁰ | ✅ ²¹ |
| Theme changes cross-fade | ✅ ³⁰ | ✅ ³⁰ | ✅ ³⁰ | ✅ ³⁰ | ✅ ³⁰ | ✅ ³¹ |
| Reduced motion (OS setting) | ✅ ³² | ✅ ³² | ✅ ³² | ✅ ³ | ✅ ³² | ✅ ³¹ |
| System clipboard | ✅ ⁸ | ✅ ⁴ | ✅ ⁴ | ✅ ³ | ✅ ²⁰ | ✅ ¹³ |
| File dialogs | ✅ | ✅ ⁵ | ✅ ⁵ | ❌ | ❌ | ✅ ¹⁴ |
| Accessibility | AT-SPI | UI Automation | NSAccessibility | ❌ | ❌ | ARIA ²² |
| Window size limits, maximize, fullscreen | ✅ ⁹ | ✅ ¹⁰ | ✅ ¹⁰ | — | fullscreen ¹⁰ | ❌ |
| Window icon | ✅ ⁹ | ✅ ¹⁰ | Dock icon ¹⁰ | — | — | ❌ |
| Mouse cursor shapes | ✅ ⁹ | ✅ ¹⁰ | ✅ ¹⁰ | — | — | ✅ ¹² |
| Right-to-left and bidi text | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ |
| Mirrored right-to-left layouts | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ |
| System fonts by family name | ✅ ³⁴ | ✅ ³⁴ | ✅ ³⁴ | ✅ ¹⁰ | ✅ ¹⁰ | — ³⁴ |
| Bold, italic, per-element fonts, fallbacks, text decorations | ✅ ³⁵ | ✅ ³⁵ | ✅ ³⁵ | ✅ ³⁵ | ✅ ³⁵ | ✅ ³⁵ |
| Custom-paint canvases, repainting only what changed | ✅ ⁴¹ | ✅ ⁴¹ | ✅ ⁴¹ | ✅ ¹⁰ | ✅ ¹⁰ | ✅ ¹⁰ |
| Input methods (CJK IME) | ✅ ¹⁶ | ✅ ¹⁶ | ✅ ¹⁶ | ❌ | ❌ | ✅ ²³ |
| Dead keys and compose | ✅ ¹⁷ | ✅ | ✅ ¹⁶ | ❌ | ❌ | ✅ ²³ |
| Menu bar | drawn ¹⁸ | native ²⁴ | native ²⁴ | ❌ | ❌ | ❌ |
| Context menus and shortcuts | ✅ ¹⁸ | native ²⁴ | native ²⁴ | ❌ | ❌ | ❌ |
| Opening URLs and files | ✅ ²⁶ | ✅ ²⁶ | ✅ ²⁶ | ✅ ³ | ✅ ¹⁰ | ✅ ¹⁰ |
| Notifications | ✅ ²⁶ | ✅ ²⁶ | ✅ ²⁶ | ❌ | ❌ | ✅ ¹⁰ |
| Tray icon with a menu | ✅ ²⁶ | ✅ ²⁶ | ✅ ²⁶ | — | — | — |
| Dropping files and text from other apps | ✅ ²⁸ | ✅ ²⁸ | ✅ ²⁸ | ❌ | ❌ | ❌ |
| Dragging inside the app | ✅ ²⁹ | ✅ ²⁹ | ✅ ²⁹ | ✅ ²⁹ | ✅ ²⁹ | ✅ ²⁹ |
| Dragging files and text out to other apps | ✅ ³³ | ✅ ³³ | ✅ ³³ | ❌ | ❌ | ❌ |
| Asking before closing and quitting | ✅ ³⁷ | ✅ ³⁷ | ✅ ³⁷ | — | — | — |
| Window focus (`Cx::window_focused`) | ✅ ³⁷ | ✅ ³⁸ | ✅ ³⁸ | ❌ | ✅ | ✅ |
| Attention requests, beep, badge | ✅ ³⁹ | ✅ ³⁹ | ✅ Dock badge ³⁹ | — | — | — |
| Title bar on Wayland (xdg-decoration, or drawn) | ✅ ⁴⁰ | — | — | — | — | — |

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
`bididemo` on X11. See [bidi.md](bidi.md).
¹⁶ Implemented (XIM on X11, `text-input-v3` on Wayland, IMM32,
`NSTextInputClient`). X11's XIM client runs against a real input method in CI
(IBus with `--xim`: a dead key's preedit and commit, Japanese through Mozc as
an informational check, and falling back when the server quits); elsewhere CI
can't run one, so only the protocol wiring is tested. See [ime.md](ime.md).
¹⁷ From the locale's Compose file, on X11 and Wayland. CI types dead keys and a
compose sequence with `xdotool` (X11) and `wtype` (sway).
¹⁸ Linux has no system menu bar, so Stipple draws one in the window, with
mnemonics underlined while the keyboard drives it. CI opens it from the
keyboard and with a click, checks Alt underlines the mnemonics, uses a
shortcut, and picks from a context menu on X11.
¹⁹ CI runs `androiddemo` on an API 34 emulator (at its 2.625x density) and
checks that a frame is presented, its timer ticks while the spinner turns, a tap
lands on the button, `adb shell input swipe` scrolls a list without tapping
its rows (a tap there still taps one), and Tab plus `adb shell input text`
type into the field.
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
²⁶ CI runs `traydemo --selftest`: on Linux against a mock desktop on D-Bus that
receives the notification and the portal's `OpenURI`, and plays the tray host,
clicking a `StatusNotifierItem` menu item and the icon; on Windows it checks
`Shell_NotifyIconW` accepted the icon and the balloon and `ShellExecuteW`
reached a URL protocol; on macOS that the `NSStatusItem` exists, `osascript`'s
notification ran and a `.txt` opened in TextEdit. Real tray hosts, and clicks on
the Windows and macOS icons, aren't exercised. macOS notifications go through
`osascript`, as the frameworks need an app bundle. See
[os-integration.md](os-integration.md).
²⁷ CI runs `touchdemo`'s self-test on the simulator, which can't script
touches or typing: it types through the view's `UIKeyInput` methods sent by
objc dispatch (the view takes the keyboard while the field has focus and
gives it back after), and drags the list with a synthetic finger fed to the
backend's touch path, past `UITouch`. Hardware keyboard keys (`pressesBegan:`)
are only build-checked. Composing input methods (`UITextInput`) aren't
supported yet.
²⁸ XDND on X11 and `wl_data_device` on Wayland, OLE `IDropTarget` (and
`WM_DROPFILES`) on Windows, `NSDraggingDestination` on macOS. CI drops files and
text on `dropdemo` with an XDND source on X11 and drives the view's dragging
methods with a stand-in drag on macOS. Windows is build-checked: CI's real OLE
`DoDragDrop` doesn't complete on the runner yet.
Wayland is unit-tested against a scripted compositor only. See
[drag-and-drop.md](drag-and-drop.md).
²⁹ Shared code, so the same everywhere; unit-tested, and CI drags a chip in
`dropdemo` with `xdotool` on X11.
³⁰ Shared code (the App blends the presented frames), unit-tested at fixed
frame times; on the desktops and mobile it's only seen when the system's
appearance or the app's `App::theme_with` theme changes, which CI doesn't do.
³¹ CI's headless Chrome runs `webdemo`'s selftest with and without
`--force-prefers-reduced-motion`, and checks that the app sees the setting and
that a theme change cross-fades (animation frames) or snaps (none).
³² Read from the OS (see [animation.md](animation.md#reduced-motion)), but CI
can't switch the setting, so only the shared part (highlights, drop fades,
flings and theme fades snapping) is unit-tested; the reading itself is
build-checked. Linux needs a desktop portal (GNOME or KDE) or GTK's
`settings.ini`.
³³ An XDND source on X11 and `wl_data_source` / `start_drag` on Wayland, OLE
`DoDragDrop` on Windows, an `NSDraggingSession` on macOS, started when an
in-app drag of files or text leaves the window. CI drags `dropdemo`'s chips
out onto an XDND target on X11 and checks the file's URI list and the text
arrive. Wayland is unit-tested against a scripted compositor only; Windows and
macOS are build-checked only. See [drag-and-drop.md](drag-and-drop.md).
³⁴ Found by reading the font directories by hand, no font library: on Linux
the directories and aliases in fontconfig's configuration, the Windows Fonts
folders, `/System/Library/Fonts` and `/Library/Fonts` on macOS and iOS,
`/system/fonts` on Android. CI lists the families and runs `fontdemo --check`
on X11, Windows and macOS. A web page has no system fonts: the app loads its
own with `Font::from_bytes`. See [fonts.md](fonts.md).
³⁵ Shared code (the CPU text path), so the same everywhere. CI screenshots
`fontdemo` on X11 and checks its rendering off screen on X11, Windows and
macOS; elsewhere it is unit-tested. See [fonts.md](fonts.md).
³⁶ `Element::on_pointer` and `on_wheel`: the App's routing (capture, leaves,
click counts, wheel targets) is unit-tested, and CI checks a middle click, a
double click, Shift+right-click, the horizontal wheel, the hidden cursor and
the pointer leaving the window (the hover ending) with `xdotool` on X11.
Wayland (`wl_pointer.leave`, side buttons, finger-scroll phases), Windows
(`WM_MOUSELEAVE`, `WM_XBUTTON*`, capture for every button, the double-click
settings), macOS (`otherMouse*`, right and other drags, a tracking area,
scroll phases, `doubleClickInterval`), Android (each mouse button from the
button state) and the web (chorded buttons) are unit-tested where pure and
otherwise build-checked only. See [pointer-events.md](pointer-events.md).
³⁷ `App::on_close_requested` / `on_quit_requested` and `AppHandle::close_window`.
CI cancels and then confirms a close on X11 (`WM_DELETE_WINDOW`), Wayland
(sway's `xdg_toplevel.close`) and Windows (`WM_CLOSE`, after refusing
`WM_QUERYENDSESSION`), and on macOS sends `performClose:` and `terminate:`
through AppKit. On X11 and Wayland CI also moves the focus away and back. See
[window-lifecycle.md](window-lifecycle.md).
³⁸ `WM_SETFOCUS` / `WM_KILLFOCUS` and `windowDidBecomeKey:` /
`windowDidResignKey:`; build-checked only.
³⁹ The urgency hint and `_NET_WM_STATE_DEMANDS_ATTENTION` and the `Bell` request
on X11, `xdg_activation_v1` and `xdg_system_bell_v1` (where offered) on
Wayland, `FlashWindowEx` and `MessageBeep` on Windows, `requestUserAttention:`,
`NSBeep` and the Dock badge on macOS. CI checks the X11 urgency hint and the
macOS badge; the rest is build-checked (Wayland's requests are unit-tested).
⁴⁰ Server-side where the compositor offers `zxdg_decoration_manager_v1`;
otherwise Stipple draws a title bar with move, maximize, minimize, close, the
window menu and edge resizing. CI checks sway's server-side decorations and
that Weston gets a decorated buffer; the pointer handling is unit-tested.
⁴¹ Shared code (painting, invalidation, area rasterizing), unit-tested at the
App level. CI runs `canvasdemo --selftest` on X11, Wayland, Windows and macOS:
a background thread changes rows of a glyph grid at the refresh rate, and the
app checks with `RasterStats` that only those rows were rasterized. Seen
passing locally on X11 (Xvfb, at 1x, 1.5x and 2x). See [canvas.md](canvas.md).

Where the clipboard isn't wired up, copy and paste still work inside the app.
Where file dialogs aren't, `open_file` and friends return `None`, as if cancelled.

## Topics

- [Layout](layout.md): stacks, wrapping, grids, and min/max sizes
- [Lists, tables and trees](lists-and-tables.md): virtualized rows, selection, sorting, resizing
- [Widgets](widgets.md): select, number input, split pane, toolbar and toasts
- [Menus](menus.md): the menu bar, shortcuts and context menus
- [Tray icons, notifications and opening URLs](os-integration.md): `App::tray`, `notify`, `open_url`
- [Drag and drop](drag-and-drop.md): drop targets, their animated drag-over state, in-app drags, and dragging files and text out to other apps
- [Timers and background work](async.md): `AppHandle`, threads and tokio
- [Animation](animation.md): the display-driven frame clock, the spinner and hover fades,
  theme cross-fades and reduced motion
- [Custom painting](canvas.md): `Element::canvas`, invalidating parts of it from any thread, paths, bitmaps and glyph runs
- [Transforms and opacity](transforms.md): scaling, rotating and moving any element, group opacity, and hit-testing through them
- [Text editing](text-editing.md): keyboard shortcuts, undo, double- and triple-click
- [Fonts and text decorations](fonts.md): system fonts by name, bold and italic, fallbacks, metrics, underlines and strikethrough
- [Pointer events](pointer-events.md): every button, click counts, the wheel in lines or pixels, capture and the pointer leaving, for custom elements
- [Right-to-left and bidirectional text](bidi.md): Hebrew, Arabic, mixed lines, editing them, and mirrored right-to-left layouts
- [Input methods, dead keys and compose](ime.md): CJK IMEs, preedit, and Compose files
- [The native theme](native-theme.md): `Theme::native()` and OS-drawn controls
- [Images and SVG](images.md): the `image` and `svg` widgets, formats, and oxideav interop
- [Window control and cursors](windows-and-cursors.md): size limits, maximize, fullscreen, the icon, and cursor shapes
- [Closing, quitting, focus and attention](window-lifecycle.md): asking before a window closes or the app quits, window focus, attention requests, the bell, and Wayland title bars
- [High-DPI](high-dpi.md): logical and physical pixels, and where the scale comes from
- [CI and screenshots](ci.md): what CI checks, and how screenshots reach this repository
