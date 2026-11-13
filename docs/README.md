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
| Touch scrolling (drag, fling, sideways strips) and multi-touch events | — | — | — | ✅ ²⁷ | ✅ ¹⁹ | ✅ ⁶⁰ |
| Touch gestures: long press for the context menu, pinch and two-finger pan (`on_pinch`) | — | — | — | ✅ ⁶⁰ | ✅ ⁶⁰ | ✅ ⁶⁰ |
| Modifiers and shortcuts (Ctrl/Cmd, Shift-select) | ✅ ⁶ | ✅ ⁶ | ✅ ⁶ | hardware keyboard ¹⁰ | ✅ ¹⁰ | ✅ ¹² |
| Timers and background updates | ✅ | ✅ | ✅ | ✅ ³ | ✅ ¹⁹ | ✅ ¹² |
| Display-paced animation (spinner, hover fades) | ✅ ¹¹ | ✅ ¹¹ | ✅ ¹¹ | ✅ ³ | ✅ ¹⁹ | ✅ ¹² |
| High-DPI | ✅ | ✅ ¹ | ✅ ¹ | ✅ | ✅ ¹⁹ | ✅ ¹² |
| OS-drawn controls (`Theme::native`) | — ² | ✅ | ✅ | ✅ | ❌ | ❌ |
| System dark mode and accent | ✅ | ✅ | ✅ | ✅ | ✅ ²⁰ | ✅ ²¹ |
| Following appearance changes live | ✅ ⁷ | ✅ ⁷ | ✅ ⁷ | ✅ ⁵¹ | ✅ ²⁰ | ✅ ²¹ |
| Theme changes cross-fade | ✅ ³⁰ | ✅ ³⁰ | ✅ ³⁰ | ✅ ³⁰ | ✅ ³⁰ | ✅ ³¹ |
| Enter, exit and layout animations | ✅ ⁴⁶ | ✅ ⁴⁶ | ✅ ⁴⁶ | ✅ ⁴⁶ | ✅ ⁴⁶ | ✅ ⁴⁶ |
| Reduced motion (OS setting) | ✅ ³² | ✅ ³² | ✅ ³² | ✅ ³ | ✅ ³² | ✅ ³¹ |
| Built-in widgets animate by default (per-theme motion) | ✅ ⁵⁴ | ✅ ⁵⁴ | ✅ ⁵⁴ | ✅ ⁵⁴ | ✅ ⁵⁴ | ✅ ⁵⁴ |
| System clipboard | ✅ ⁸ | ✅ ⁴ | ✅ ⁴ | ✅ ³ | ✅ ²⁰ | ✅ ¹³ |
| File dialogs | ✅ | ✅ ⁵ | ✅ ⁵ | ✅ ⁶² | ✅ ⁶² | ✅ ¹⁴ |
| Accessibility | AT-SPI ⁶⁶ | UI Automation ⁶⁷ | NSAccessibility ⁶⁸ | UIAccessibility ⁴⁹ | node provider ⁵⁶ | ARIA ²² |
| Window size limits, maximize, fullscreen | ✅ ⁹ | ✅ ¹⁰ | ✅ ¹⁰ | — | fullscreen ¹⁰ | ❌ |
| Window icon | ✅ ⁹ | ✅ ¹⁰ | Dock icon ¹⁰ | — | — | ❌ |
| Several windows, opened and closed at run time | ✅ ⁴⁵ | ✅ ⁴⁵ | ✅ ⁴⁵ | — | — | — |
| Window titles at run time | ✅ ⁴⁵ | ✅ ⁴⁵ | ✅ ⁴⁵ | — | — | ❌ |
| Window position, size, resize increments, state | X11 ⁴⁵, Wayland: size and state | ✅ ⁴⁵ | ✅ ⁴⁵ | — | — | ❌ |
| Mouse cursor shapes | ✅ ⁹ | ✅ ¹⁰ | ✅ ¹⁰ | — | — | ✅ ¹² |
| Right-to-left and bidi text | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ |
| Mirrored right-to-left layouts | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ |
| System fonts by family name | ✅ ³⁴ | ✅ ³⁴ | ✅ ³⁴ | ✅ ¹⁰ | ✅ ¹⁰ | — ³⁴ |
| Bold, italic, per-element fonts, fallbacks, text decorations | ✅ ³⁵ | ✅ ³⁵ | ✅ ³⁵ | ✅ ³⁵ | ✅ ³⁵ | ✅ ³⁵ |
| Custom-paint canvases, repainting only what changed | ✅ ⁴¹ | ✅ ⁴¹ | ✅ ⁴¹ | ✅ ¹⁰ | ✅ ¹⁰ | ✅ ¹⁰ |
| Memoized subtrees, animation frames without a rebuild, painting only what shows | ✅ ⁶⁹ | ✅ ⁶⁹ | ✅ ⁶⁹ | ✅ ⁶⁹ | ✅ ⁶⁹ | ✅ ⁶⁹ |
| Input methods (CJK IME) | ✅ ¹⁶ | ✅ ⁴⁸ | ✅ ¹⁶ | ✅ ⁵⁰ | ✅ ⁵⁵ | ✅ ²³ |
| Dead keys and compose | ✅ ¹⁷ | ✅ | ✅ ¹⁶ | ❌ | ❌ | ✅ ²³ |
| Input methods in custom elements (`ime_input`) | ✅ ⁴³ | ✅ ⁴³ | ✅ ⁴³ | ✅ ¹⁰ | ✅ ¹⁰ | ✅ ⁴³ |
| Raw keys (`on_raw_key`): physical key, left/right modifiers, repeat flag | ✅ ⁴² | ✅ ¹⁰ | ✅ ¹⁰ | hardware keyboard ¹⁰ | ✅ ¹⁰ | ✅ ¹⁰ |
| Key repeat | ✅ ⁴² | ✅ | ✅ | ✅ | ✅ | ✅ |
| Menu bar | drawn ¹⁸ | native ²⁴ | native ²⁴ | drawn ⁶¹ | drawn ⁶¹ | drawn ⁶¹ |
| Context menus and shortcuts | ✅ ¹⁸ | native ²⁴ | native ²⁴ | drawn, long press ⁶⁰ | drawn, long press ⁶⁰ | drawn ⁶⁰ |
| Opening URLs and files | ✅ ²⁶ | ✅ ²⁶ | ✅ ²⁶ | ✅ ³ | ✅ ¹⁰ | ✅ ¹⁰ |
| Notifications | ✅ ²⁶ | ✅ ²⁶ | ✅ ²⁶ | ✅ ⁵² | ✅ ⁶³ | ✅ ¹⁰ |
| Tray icon with a menu | ✅ ²⁶ | ✅ ²⁶ | ✅ ²⁶ | — | — | — |
| Dropping files and text from other apps | ✅ ²⁸ | ✅ ²⁸ | ✅ ²⁸ | ❌ | ❌ | ❌ |
| Dragging inside the app | ✅ ²⁹ | ✅ ²⁹ | ✅ ²⁹ | ✅ ²⁹ | ✅ ²⁹ | ✅ ²⁹ |
| Dragging files and text out to other apps | ✅ ³³ | ✅ ³³ | ✅ ³³ | ❌ | ❌ | ❌ |
| Asking before closing and quitting | ✅ ³⁷ | ✅ ³⁷ | ✅ ³⁷ | — | — | — |
| Background, foreground and memory warnings (`App::on_lifecycle`) | — | — | — | ✅ ⁵³ | ✅ ⁵⁷ | — |
| Back navigation (`App::on_back`) | — | — | — | ✅ edge swipe ⁶⁴ | ✅ ⁵⁷ | — |
| Predictive back (`App::back_enabled`, `App::on_back_progress`) | — | — | — | ✅ ⁶⁴ | ✅ ⁵⁸ | — |
| State kept across process death (`App::save_state`) | — | — | — | ✅ ⁶⁵ | ✅ ⁵⁹ | ✅ reload ⁶⁵ |
| Window focus (`Cx::window_focused`) | ✅ ³⁷ | ✅ ³⁸ | ✅ ³⁸ | ❌ | ✅ | ✅ |
| Attention requests, beep, badge | ✅ ³⁹ | ✅ ³⁹ | ✅ Dock badge ³⁹ | — | — | — |
| Title bar on Wayland (xdg-decoration, or drawn) | ✅ ⁴⁰ | — | — | — | — | — |
| Scenes rendered on the GPU (`stipple::gpu`, experimental) | GLES2 ⁴⁷ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Message boxes | drawn ⁴⁴ | native ⁴⁴ | native ⁴⁴ | native ³ | drawn ⁴⁴ | native ¹⁰ |
| Secure password entry | IME off | IME off | `EnableSecureEventInput` ¹⁰ | IME off | IME off | IME off |

¹ Implemented, but CI only has 1x displays, so it hasn't been seen on a real high-DPI one.
² Linux has no native UI framework; controls stay self-drawn in the system's colors.
³ Implemented, but not yet run in CI on iOS.
⁴ CI round-trips text through the OS clipboard, and Ctrl/Cmd+C/X/V reach it now
that these backends report modifier keys. HTML and images too (`CF_HTML`,
`CF_DIBV5` and `PNG` on Windows; `public.html` and `public.png` on macOS),
cross-checked with .NET's `Clipboard` / `Set-Clipboard -AsHtml` and
`osascript`. No primary selection.
⁵ Implemented, but a modal dialog can't be driven headlessly, so CI only builds it.
⁶ CI drives these with scripted input on X11 only; on Wayland, Windows, macOS and
the web they're implemented and unit-tested but not yet exercised end to end.
⁷ Implemented, but CI can't switch the system's appearance, so it's untested end
to end. Linux needs the desktop portal (its `SettingChanged` signal).
⁸ Text, HTML and PNG images both ways, plus the primary selection (select,
middle-click). CI checks each against `xclip` on X11 (including `INCR`
transfers both ways and a middle-click paste) and `wl-copy`/`wl-paste` under
sway (without a pointer there, so the middle click itself is X11-only).
⁹ CI checks the size hints, icon and cursor changes on X11. On Wayland the icon
needs `xdg-toplevel-icon-v1` and cursor shapes `cursor-shape-v1`; without them
the compositor's defaults stay.
¹⁰ Implemented, but CI only builds it.
¹¹ CI checks that `asyncdemo`'s spinner animates and then stops. X11 paces
frames with a timer at the RandR refresh rate; Wayland, Windows and macOS use
frame callbacks, `DwmFlush` and `CVDisplayLink`, which CI exercises but can't
time precisely. The selftest prints where frames came from and what they
cost; the macOS job also checks the frame count loosely.
¹² CI drives `webdemo` with scripted DOM events in headless Chrome, at a pixel
ratio of 2, and checks what the app saw: clicks, the cursor, typing and Ctrl
shortcuts, timers, the spinner animating and then stopping, and resizing.
¹³ Through the browser's `paste`, `copy` and `cut` events (and
`navigator.clipboard.writeText`): Ctrl/Cmd+V pastes, but the app can't read the
clipboard on its own. CI round-trips text with synthetic clipboard events. HTML
goes along both ways (build-checked); images aren't supported.
¹⁴ `open_file_contents` (an `<input type=file>`, answered asynchronously) and
`save_file_contents` (a download). `open_file` and friends return `None`, since
a page never sees paths. CI checks both with the picker and download stood in
for. See [the web page](platforms/web.md#file-dialogs).
¹⁵ Laid out by the shared CPU text path, so the same everywhere; CI screenshots
`bididemo` on X11. See [bidi.md](bidi.md).
¹⁶ Implemented (XIM on X11, `text-input-v3` on Wayland, TSF with IMM32 as
the fallback on Windows (see ⁴⁸), `NSTextInputClient`). X11's XIM client
runs against a real input method in CI (IBus with `--xim`: a dead key's preedit and commit, Japanese through Mozc,
and falling back when the server quits), and so does Wayland's
`text-input-v3` (fcitx5 under sway: Japanese through Mozc with a cancel,
Chinese pinyin, Korean Hangul). On Windows and macOS CI tries the
Microsoft Japanese IME and Kotoeri without failing the run (not yet seen
passing), so there only the protocol wiring is verified. See [ime.md](ime.md).
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
are only build-checked. Composition goes through `UITextInput` (⁵⁰).
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
⁴² CI types F-keys, Ctrl+letters, Tab and a held key into `rawkeys` with
`xdotool` on X11 and `wtype` under sway, and checks each key's logical
identity, modifiers and repeat flag, and on X11 its physical key and sides and
that a menu shortcut beats the raw handler (`wtype`'s virtual keycodes aren't
positions, so Wayland's physical keys are only unit-tested). X11 repeats without fake releases (XKB detectable
auto-repeat); Wayland repeats on the client, from `wl_keyboard.repeat_info`.
The order keys go through the App is unit-tested. See
[keyboard.md](keyboard.md).
⁴³ The App part (which element the IME follows, and its caret rectangle) is
unit-tested; from there it is the text fields' IME path (see ¹⁶). See
[ime.md](ime.md#elements-that-arent-text-fields).
⁴⁴ `AppHandle::message` answers asynchronously everywhere: `TaskDialogIndirect`
(or `MessageBoxW`) on a thread of its own on Windows, an `NSAlert` sheet on
macOS, `UIAlertController` on iOS, `alert`/`confirm` on the web, and a modal
panel drawn in the window in the theme on Linux and Android. CI answers the
native one with an in-process self-test on Windows and macOS (screenshotting
it), and drives the drawn one with `xdotool` on X11. See
[os-integration.md](os-integration.md#message-boxes).
⁴⁵ CI's `multiwindow --selftest` retitles, moves and resizes windows and opens
and closes one while the app runs. X11 is checked from outside with `xprop`
and `xdotool` under Xvfb; Wayland with `swaymsg` under sway (titles and
windows: Wayland lets no app place its windows, and has no resize
increments); Windows and macOS through what the backend reads back from the
OS. See [windows-and-cursors.md](windows-and-cursors.md#several-windows).
⁴⁶ Shared code on the laid-out tree (`Element::enter` / `exit` /
`animate_layout`), unit-tested at fixed frame times; CI runs
`listmotion --selftest` off-screen, and on X11 under Xvfb screenshots
`listmotion --demo` mid-flight, then checks it comes to rest and stops using
the CPU. Elsewhere it runs on each platform's frame clock but isn't run in CI.
See [animation.md](animation.md#enter-exit-and-layout-animations).
⁴⁷ Behind the `gl` feature, through `App::render_with`: boxes, paths, images
(textures cached per image), text, clips, canvases and layers with transforms
and opacity, all on the GPU, read back and presented as a bitmap. CI renders
test scenes and an app both ways under Mesa's llvmpipe and compares them with
the software renderer, and screenshots `gpuwindow` under Xvfb. Vulkan, Direct3D
11, Metal and WebGPU only draw a test triangle so far. See [gpu.md](gpu.md).
⁴⁸ Through the Text Services Framework (a hand-written `ITextStoreACP` text
store, composition sink and display attributes), with IMM32 when TSF can't
be activated or `STIPPLE_TSF=0`. The text store's logic is unit-tested; CI
activates TSF on the Windows runner and drives a composition through the real
TSF manager (the app playing the input method), checking the preedit and the
commit reach the field. A job that tries the real Microsoft Japanese IME
runs without failing the workflow (not yet seen passing). See
[ime.md](ime.md#per-platform).
⁴⁹ The view is a `UIAccessibilityContainer` with one `UIAccessibilityElement`
per node (label, value, traits, frame in screen coordinates; a double tap
taps the node), and posts layout and screen-changed notifications on updates.
CI's `touchdemo` self-test on the simulator reads the elements through objc
dispatch as VoiceOver does, checks the text field's value and frame, the
label's static-text trait and a row's button trait, activates the row and
checks it was tapped and the label's element followed. From another process,
an XCUITest runner (the AX channel VoiceOver uses) reads the tree, taps a
button at its element's frame and types into the field on the software
keyboard (added with this change, not yet run in CI). Not yet tried with
VoiceOver itself. See [accessibility.md](accessibility.md#ios).
⁵⁰ `UITextInput` on the view: marked text as the preedit, commits, the field's
text around the caret for autocorrect and dictation, `replaceRange:withText:`
and the caret rectangle for the candidate bar. CI's `touchdemo` self-test on
the simulator drives these methods through objc dispatch (marked text, a
commit, a replacement) and reads the document back; no real keyboard composes
in CI. Hardware dead keys aren't handled. See [ime.md](ime.md).
⁵¹ `traitCollectionDidChange:`, by the screen's `userInterfaceStyle`. CI
switches the simulator to dark mode (`xcrun simctl ui ... appearance dark`)
while `touchdemo` runs and checks its native theme turned dark.
⁵² `UNUserNotificationCenter`, asking for permission first. CI checks the call
is handed over and the center reports the permission; nobody answers the
prompt on the simulator, so a shown notification isn't seen. See
[os-integration.md](os-integration.md#notifications).
⁵³ `Event::Lifecycle` / `App::on_lifecycle`, from UIKit's background,
foreground and memory-warning notifications. CI sends `touchdemo` to the
background on the simulator (launching another app) and back, and checks the
events arrived and no frame was built in between; the memory warning is
posted in-process. See
[window-lifecycle.md](window-lifecycle.md#going-to-the-background-mobile).
⁵⁴ Shared code: buttons, switches, checkboxes, radios, tabs, the tab bar,
selects, menus, tooltips, toasts, dialogs, tree disclosure triangles and the
app's drop-target highlight, with the theme's motion tokens
(`Theme::native()` picks Fluent, AppKit, UIKit, Material or libadwaita
timings); popups, menus, tooltips and dialogs animate out too, kept drawn
(and out of reach of input) while they leave. Unit-tested at fixed frame
times and through App-level tests; under Xvfb, `widgetsdemo`,
`sessionsdemo`, `tabsdemo`, `bididemo`, `material3demo` and `themegallery`
were checked to draw pixel-identical frames at rest (by hand, not in CI). A
modal's scrim fades in and out with its dialog (checked the same way with
`sessionsdemo` and `overlaydemo` dialogs open). Controls the OS draws under
`Theme::native()` (buttons, checkboxes, radios and switches on Windows, macOS
and iOS) cross-fade between the OS's pictures of their states; that is only
unit-tested. See [widgets.md](widgets.md#motion).

⁵⁵ An `InputConnection` from a host view whose Java classes the backend
writes as a dex file at run time (no Java toolchain). On an API 34 emulator,
Gboard connects to it, and an app-side self-test composes, commits an emoji
and deletes around the caret through the real Java objects; not yet tried by
a person on a composing keyboard. See [platforms/android.md](platforms/android.md#input-methods).

⁵⁶ An `AccessibilityNodeProvider` with a virtual view per node (click, focus,
scroll, set text); `uiautomator dump` on the CI emulator lists the nodes, and
an accessibility service standing in for TalkBack (enabled the way TalkBack
is) moves accessibility focus through every node, clicks a button and sets
the field's text, from its own process. TalkBack itself, which the API 34
Google APIs emulator image ships, is turned on in CI too: its focus must
stop on the labels, reach the button and its click the app (passing in CI;
reported without failing the job). See
[accessibility.md](accessibility.md#android).

⁵⁷ `onPause` / `onResume` / `onTrimMemory` and the Back key, checked on the
CI emulator (`input keyevent`, Home and `am start`, `am send-trim-memory`).
See [window-lifecycle.md](window-lifecycle.md#going-to-the-background-mobile).

⁵⁸ Android 13 and later, with `android:enableOnBackInvokedCallback` in the
manifest: an `OnBackInvokedCallback` (a class the backend writes as dex at run
time) registered only while the app would go back, and on Android 14 and later
the gesture's progress (`OnBackAnimationCallback`). Checked on CI emulators:
on API 34 the Back key through the callback, an edge swipe with its progress,
a cancelled swipe, and a Back the app declined going to the system; on API 33
the same through the callback without progress; on API 30 (no predictive
back) the Back key, the edge swipe and `onBackPressed`. iOS: see ⁶⁴. See
[window-lifecycle.md](window-lifecycle.md#predictive-back-android-13-and-later).

⁵⁹ Bytes from `App::save_state`, kept in the activity's saved `Bundle`
(`onSaveInstanceState`) and handed to `App::restore_state` when Android starts
the app again. Checked on the CI emulator: Home, `am kill`, relaunch. iOS
and the web: see ⁶⁵. See
[window-lifecycle.md](window-lifecycle.md#keeping-the-state-across-process-death).

⁶⁰ Recognized in the shared core from the platform's touches, so the same
everywhere, and unit-tested with synthetic touch sequences (scrolling on both
axes, long presses, pinches). On devices: `touchdemo`'s iOS simulator
self-test long-presses a row (its menu opens, its item is picked), drags a
strip sideways and pinches with two synthetic fingers through the backend's
touch path; the Android emulator long-presses a row and swipes a strip with
`adb shell input swipe`, and pinches a button with two fingers on a virtual
touch screen (the shell's `uinput`, through the system's input pipeline; API
34 and 33); headless Chrome taps and long-presses `webdemo` with synthetic
`TouchEvent`s. Not yet tried on a real phone. See [pointer-events.md](pointer-events.md#touch-gestures).

⁶¹ The in-window bar Stipple draws on Linux; its menus open on a tap. Phone
apps usually leave `App::menu` out on these targets. Not run in CI there. See
[menus.md](menus.md#context-menus).

⁶² `open_file_contents` and `save_file_contents` (contents, not paths, answered
asynchronously): `UIDocumentPickerViewController` on iOS, the Storage Access
Framework's `ACTION_OPEN_DOCUMENT` / `ACTION_CREATE_DOCUMENT` on Android.
CI's Android emulator picks a file in DocumentsUI (through uiautomator) and
reads it, cancels one, and saves one; the iOS simulator self-test checks both
pickers come up and that cancelling reaches the app, and, since choosing a
file there can't be scripted, hands each picker's delegate a file
(`documentPicker:didPickDocumentsAtURLs:`) and checks the app gets its bytes,
or the export's success. `open_file` and friends still return `None`. See
[ios.md](platforms/ios.md#file-pickers) and [android.md](platforms/android.md#file-pickers).

⁶³ `NotificationManager` on a channel of the app's, asking for
`POST_NOTIFICATIONS` first on Android 13 and later. CI's API 34 emulator
refuses the prompt (then `notify` fails), grants it with `pm grant`, and finds
the notification in `dumpsys notification`. See
[os-integration.md](os-integration.md#notifications).

⁶⁴ iOS's interactive back: a swipe from the leading edge (the left, or the
right under a right-to-left theme), recognized by the backend from the view's
touches while `App::back_enabled` says yes, as `BackGesture` progress and then
`on_back`; once recognized, the finger is cancelled in the app. While back is
off the edge's touches are the app's. Unit-tested (the recognizer, and in
front of an `App`); `touchdemo`'s simulator self-test swipes with synthetic
touches through the backend's touch path (added with this change, not yet run
in CI). Not tried on a device. See
[window-lifecycle.md](window-lifecycle.md#the-back-swipe-ios).

⁶⁵ iOS: a file in the app's `Library/Caches`, written when the app goes to
the background under its scene session, read (and deleted) at the next
launch. As iOS (and Android) do, only a system kill brings it back: when the
user swipes the app away, UIKit discards the session
(`application:didDiscardSceneSessions:`) and the state goes, the next launch
starting afresh in a new session. That needs the scene lifecycle (a
`UIApplicationSceneManifest` in the `Info.plist`); without it the state comes
back after a swipe-away too. CI backgrounds `touchdemo`, ends it with `simctl
terminate` and relaunches it (restored: this ran in CI before scenes; the
same-session check is new), then has it destroy its own session
(`requestSceneSessionDestruction:`, UIKit's discard path) and checks the next
launch restores nothing (added, not yet run). Web: the tab's `sessionStorage`,
written on `pagehide` and when the page is hidden, handed to the app before it
starts; closing the tab drops it (a new tab starts without it); headless
Chrome reloads `webdemo` and checks its count came back (run locally; CI step
added). See
[window-lifecycle.md](window-lifecycle.md#keeping-the-state-across-process-death).

⁶⁶ X11 and Wayland windows register with the AT-SPI registry and serve their
tree from a helper thread (hand-written D-Bus): an object per node with
`Accessible`, `Component` (extents, hit testing), `Action` (a click),
`Text`, `EditableText` and `Value`, and events for name, text, state,
children and focus changes and window activation. CI walks a running X11
window from a pyatspi client (as Orca does), hit-tests it, runs a button's
action, follows the focus after a Tab and edits the field (passing in CI);
the same under sway, and Orca itself, pass in CI but aren't required yet. No caret, selection or relations. See
[accessibility.md](accessibility.md#linux).

⁶⁷ A hand-built provider per node, hosted by the window's provider: control
type, name, bounds in screen pixels, keyboard focus, hit testing, and the
Invoke pattern on buttons. CI walks a running window from PowerShell's
process with the .NET UI Automation client (the path Narrator and NVDA use),
hit-tests it, invokes a button and reads the focus after a Tab (passing in
CI). A focus move raises UIA's focus-changed event, and a window's root is
named by its title. NVDA itself is driven in CI: its log of what it says
must name the window, the field Tab focuses and "Row 0" "button", and
NVDA+Enter must press it (passing in CI; reported without failing the
workflow). Buttons don't take keyboard focus, so Tab
can't reach them. No Value, Toggle, Scroll or Text patterns, and no
structure or property-change events. See
[accessibility.md](accessibility.md#windows).

⁶⁸ An `NSAccessibilityElement` per node, with role, label and frame in screen
coordinates. CI reads a running app's tree from another process through the
AX API, as VoiceOver does (passing in CI). Buttons and the other controls a
click activates answer `AXPress` (VoiceOver's VO-Space), checked by the same
probe, and VoiceOver itself is driven through its AppleScript interface: it
must speak "Row 0" as a button and press it (both passing in CI; the
VoiceOver step doesn't fail the job). See
[accessibility.md](accessibility.md#macos).

⁶⁹ In the shared core, so the same everywhere: `Cx::memo` and `Cx::derive`,
transition-only frames drawn from the tree on screen, and scenes culled to the
window and clips. Unit- and App-tested (memo invalidation, handlers and ids
of reused subtrees, damage of a moving element, the same pixels culled or
not), with a benchmark of a 500-row list. On X11, CI checks that `hoverdemo`'s
hover fade is drawn without building the view; the other backends take the
same wake path, unchecked there. See [performance.md](performance.md).

Where the clipboard isn't wired up, copy and paste still work inside the app.
Where file dialogs aren't, `open_file` and friends return `None`, as if cancelled;
on iOS, Android and the web, which hand out contents rather than paths, so do
they: use `open_file_contents` and `save_file_contents` there.

## Topics

- [Layout](layout.md): stacks, wrapping, grids, and min/max sizes
- [Lists, tables and trees](lists-and-tables.md): virtualized rows, selection, sorting, resizing
- [Widgets](widgets.md): select, number input, split pane, toolbar, toasts, tab bar,
  password field and color picker
- [Menus](menus.md): the menu bar, shortcuts and context menus (long press on touch screens)
- [Tray icons, notifications and opening URLs](os-integration.md): `App::tray`, `notify`, `open_url`
- [Drag and drop](drag-and-drop.md): drop targets, their animated drag-over state, in-app drags, and dragging files and text out to other apps
- [Timers and background work](async.md): `AppHandle`, threads and tokio
- [Animation](animation.md): the display-driven frame clock, the spinner and hover fades,
  interaction states, transitions, enter/exit and layout animations, the
  built-in widgets' motion, theme cross-fades and reduced motion
- [Performance](performance.md): memoized subtrees and derived values (`Cx::memo`, `Cx::derive`), animation frames without a rebuild, painting only what shows, and what a frame costs
- [Custom painting](canvas.md): `Element::canvas`, invalidating parts of it from any thread, paths, bitmaps and glyph runs
- [Transforms and opacity](transforms.md): scaling, rotating and moving any element, group opacity, and hit-testing through them
- [Text editing](text-editing.md): keyboard shortcuts, undo, double- and triple-click
- [Fonts and text decorations](fonts.md): system fonts by name, bold and italic, fallbacks, metrics, underlines and strikethrough
- [Accessibility](accessibility.md): what each platform's screen readers get, and how CI checks it from outside the app
- [Pointer events](pointer-events.md): every button, click counts, the wheel in lines or pixels, capture and the pointer leaving, for custom elements; touch gestures (scrolling, long press, pinch)
- [Right-to-left and bidirectional text](bidi.md): Hebrew, Arabic, mixed lines, editing them, and mirrored right-to-left layouts
- [The keyboard](keyboard.md): raw keys for terminals and games, keeping Tab, key repeat, Option as Meta
- [Input methods, dead keys and compose](ime.md): CJK IMEs, preedit, and Compose files
- [The native theme](native-theme.md): `Theme::native()` and OS-drawn controls
- [Images and SVG](images.md): the `image` and `svg` widgets, formats, and oxideav interop
- [Windows and cursors](windows-and-cursors.md): several windows opened and closed at run time, titles, position, size and state, size limits, maximize, fullscreen, the icon, and cursor shapes
- [Closing, quitting, focus and attention](window-lifecycle.md): asking before a window closes or the app quits, window focus, attention requests, the bell, Wayland title bars, and mobile apps going to the background
- [Rendering on the GPU](gpu.md): `stipple::gpu` under `App::render_with`, what it draws how, and how close it is to the software renderer
- [High-DPI](high-dpi.md): logical and physical pixels, and where the scale comes from
- [CI and screenshots](ci.md): what CI checks, and how screenshots reach this repository
