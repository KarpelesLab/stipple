# Stipple docs

Guides for building, packaging and running Stipple apps. The [README](../README.md)
covers what Stipple is; the [roadmap](../ROADMAP.md) covers where it's going; API
docs are on [docs.rs](https://docs.rs/stipple).

New here? Start with [Getting started](getting-started.md): a todo list app,
step by step, from `cargo new` to a headless test. For a whole app, see
[the reference app](reference-app.md): a file and media browser, accessible
and localized.

## Platforms

How to build, package and run on each platform, and what works there today.

- [Linux](platforms/linux.md): Wayland and X11
- [Windows](platforms/windows.md)
- [macOS](platforms/macos.md)
- [iOS](platforms/ios.md)
- [Android](platforms/android.md)
- [Web](platforms/web.md): `wasm32` and a `<canvas>`
- [Packaging for the App Store, Google Play and Linux](packaging.md): `cargo-stipple`, a signed `.ipa` and `.aab` from a manifest in `Cargo.toml`, and a Linux staging root with its `.desktop` and D-Bus `.service` files

### What works where

| | Linux | Windows | macOS | iOS | Android | Web |
|---|---|---|---|---|---|---|
| Window and rendering | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Pointer input | ✅ | ✅ | ✅ | ✅ touch, iPad pointer hover ⁸⁰ | ✅ touch, mouse ¹⁹ | ✅ ¹² |
| Keyboard input | ✅ | ✅ | ✅ | ✅ ²⁷ | ✅ ¹⁹ | ✅ ¹² |
| Scroll wheel and trackpad | ✅ ⁶ | ✅ ⁶ | ✅ ⁶ | ✅ iPad ⁸⁰ | mouse wheel ¹⁰ | ✅ ⁶ |
| Raw pointer events (every button, click counts, lines vs pixels, pointer leaving) | ✅ ³⁶ | ✅ ³⁶ | ✅ ³⁶ | touch | ✅ mouse ³⁶ | ✅ ³⁶ |
| Touch scrolling (drag, fling, sideways strips) and multi-touch events | — | — | — | ✅ ²⁷ | ✅ ¹⁹ | ✅ ⁶⁰ |
| Scroll bars (drag the thumb, page on the track, hold to repeat, arrows, two-way areas) | ✅ ⁷⁴ | ✅ ⁷⁴ | overlay ⁷⁴ | indicator ⁷⁴ | indicator ⁷⁴ | ✅ ⁷⁴ |
| Touch gestures: long press for the context menu, pinch and two-finger pan (`on_pinch`) | — | — | — | ✅ ⁶⁰ | ✅ ⁶⁰ | ✅ ⁶⁰ |
| Editing text with a finger: tap for the caret, word selection, selection handles, the edit menu | — | — | — | ✅ ⁷⁷ | ✅ ⁷⁷ | ✅ ⁷⁷ |
| Modifiers and shortcuts (Ctrl/Cmd, Shift-select) | ✅ ⁶ | ✅ ⁶ | ✅ ⁶ | hardware keyboard ¹⁰ | ✅ ¹⁰ | ✅ ¹² |
| Keyboard focus on buttons and toggles (Tab, Shift+Tab, Enter / Space) | ✅ ⁷⁰ | ✅ ⁷⁰ | ✅ ⁷⁰ | hardware keyboard ⁷⁰ | ✅ ⁷⁰ | ✅ ⁷⁰ |
| Timers and background updates | ✅ | ✅ | ✅ | ✅ ⁸¹ | ✅ ¹⁹ | ✅ ¹² |
| Display-paced animation (spinner, hover fades) | ✅ ¹¹ | ✅ ¹¹ | ✅ ¹¹ | ✅ ³ | ✅ ¹⁹ | ✅ ¹² |
| High-DPI | ✅ | ✅ ¹ | ✅ ¹ | ✅ | ✅ ¹⁹ | ✅ ¹² |
| OS-drawn controls (`Theme::native`) | — ² | ✅ ⁹³ | ✅ ⁹³ | ✅ ⁹³ | ❌ | ❌ |
| Text fields as the OS's own (`Theme::native`) | — ² | ✅ ⁹⁴ | ✅ ⁹⁴ | ✅ ⁹⁴ | ❌ | ❌ |
| System dark mode and accent | ✅ | ✅ | ✅ | ✅ | ✅ ²⁰ | ✅ ²¹ |
| Following appearance changes live | ✅ ⁷ | ✅ ⁷ | ✅ ⁷ | ✅ ⁵¹ | ✅ ²⁰ | ✅ ²¹ |
| Theme changes cross-fade | ✅ ³⁰ | ✅ ³⁰ | ✅ ³⁰ | ✅ ³⁰ | ✅ ³⁰ | ✅ ³¹ |
| Enter, exit and layout animations | ✅ ⁴⁶ | ✅ ⁴⁶ | ✅ ⁴⁶ | ✅ ⁴⁶ | ✅ ⁴⁶ | ✅ ⁴⁶ |
| Reduced motion (OS setting) | ✅ ³² | ✅ ³² | ✅ ³² | ✅ ³ | ✅ ³² | ✅ ³¹ |
| Built-in widgets animate by default (per-theme motion) | ✅ ⁵⁴ | ✅ ⁵⁴ | ✅ ⁵⁴ | ✅ ⁵⁴ | ✅ ⁵⁴ | ✅ ⁵⁴ |
| System clipboard | ✅ ⁸ | ✅ ⁴ | ✅ ⁴ | ✅ ⁸¹ | ✅ ⁸² | ✅ ¹³ |
| File dialogs | ✅ | ✅ ⁵ | ✅ ⁵ | ✅ ⁶² | ✅ ⁶² | ✅ ¹⁴ |
| Accessibility | AT-SPI ⁶⁶ | UI Automation ⁶⁷ | NSAccessibility ⁶⁸ | UIAccessibility ⁴⁹ | node provider ⁵⁶ | ARIA ²² |
| Screen readers: range values, tables, captions, text attributes | ✅ ¹⁰¹ | ✅ ¹⁰¹ | ✅ ¹⁰¹ | range values ¹⁰¹ | ✅ ¹⁰¹ | ✅ ¹⁰¹ |
| Window size limits, maximize, fullscreen | ✅ ⁹ | ✅ ¹⁰ | ✅ ¹⁰ | — | fullscreen ¹⁰ | ❌ |
| Window size limits from the content's layout | ✅ ⁹⁸ | ✅ ⁹⁸ | ✅ ⁹⁸ | — | — | — |
| Windows reopen where they were left (`App::remember_window`) | X11, Wayland: size and state ¹⁰⁰ | ✅ ¹⁰⁰ | ✅ ¹⁰⁰ | — | — | — |
| Packaged assets as a file range for media players and `mmap` (`Assets::open_fd`) | ✅ ¹⁰² | ✅ ¹⁰² | ✅ ¹⁰² | ✅ ¹⁰² | `AAsset_openFileDescriptor64` ¹⁰² | — ¹⁰² |
| Window icon | ✅ ⁹ | ✅ ¹⁰ | Dock icon ¹⁰ | — | — | ❌ |
| Several windows, opened and closed at run time | ✅ ⁴⁵ | ✅ ⁴⁵ | ✅ ⁴⁵ | — | — | — |
| Window titles at run time | ✅ ⁴⁵ | ✅ ⁴⁵ | ✅ ⁴⁵ | — | — | ❌ |
| Window position, size, resize increments, state | X11 ⁴⁵, Wayland: size and state | ✅ ⁴⁵ | ✅ ⁴⁵ | — | — | ❌ |
| Mouse cursor shapes | ✅ ⁹ | ✅ ¹⁰ | ✅ ¹⁰ | — | — | ✅ ¹² |
| Right-to-left and bidi text | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ | ✅ ¹⁵ |
| Mirrored right-to-left layouts | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ | ✅ ²⁵ |
| System fonts by family name | ✅ ³⁴ | ✅ ³⁴ | ✅ ³⁴ | ✅ ¹⁰ | ✅ ¹⁰ | — ³⁴ |
| Bold, italic, per-element fonts, fallbacks, text decorations | ✅ ³⁵ | ✅ ³⁵ | ✅ ³⁵ | ✅ ³⁵ | ✅ ³⁵ | ✅ ³⁵ |
| Color emoji, sequences as one glyph (skin tones, flags, keycaps, ZWJ) | ✅ ⁹⁷ | ✅ ⁹⁷ | ✅ ⁹⁷ | ✅ ⁹⁷ | ✅ ⁹⁷ | ✅ ⁹⁷ |
| Text flowing around floats, shapes and images' opaque pixels | ✅ ⁷⁵ | ✅ ⁷⁵ | ✅ ⁷⁵ | ✅ ⁷⁵ | ✅ ⁷⁵ | ✅ ⁷⁵ |
| Custom-paint canvases, repainting only what changed | ✅ ⁴¹ | ✅ ⁴¹ | ✅ ⁴¹ | ✅ ¹⁰ | ✅ ¹⁰ | ✅ ¹⁰ |
| Memoized subtrees, animation frames without a rebuild, painting only what shows | ✅ ⁶⁹ | ✅ ⁶⁹ | ✅ ⁶⁹ | ✅ ⁶⁹ | ✅ ⁶⁹ | ✅ ⁶⁹ |
| Rich text editor (`rich_editor`): marks, blocks, links, images placed, wrapped and sized by the pointer; Markdown and HTML | ✅ ⁷⁶ | ✅ ⁷⁶ | ✅ ⁷⁶ | ✅ ⁷⁶ | ✅ ⁷⁶ | ✅ ⁷⁶ |
| Input methods (CJK IME) | ✅ ¹⁶ | ✅ ⁴⁸ | ✅ ¹⁶ | ✅ ⁵⁰ | ✅ ⁵⁵ | ✅ ²³ |
| Dead keys and compose | ✅ ¹⁷ | ✅ | ✅ ¹⁶ | ✅ hardware keyboard ⁷⁸ | ✅ hardware keyboard ⁷⁸ | ✅ ²³ |
| Input methods in custom elements (`ime_input`) | ✅ ⁴³ | ✅ ⁴³ | ✅ ⁴³ | ✅ ¹⁰ | ✅ ¹⁰ | ✅ ⁴³ |
| Raw keys (`on_raw_key`): physical key, left/right modifiers, repeat flag | ✅ ⁴² | ✅ ¹⁰ | ✅ ¹⁰ | hardware keyboard ¹⁰ | ✅ ¹⁰ | ✅ ¹⁰ |
| Key repeat | ✅ ⁴² | ✅ | ✅ | ✅ | ✅ | ✅ |
| Menu bar | drawn ¹⁸ | native ²⁴ | native ²⁴ | drawn ⁶¹ | drawn ⁶¹ | drawn ⁶¹ |
| Context menus and shortcuts | ✅ ¹⁸ | native ²⁴ | native ²⁴ | drawn, long press ⁶⁰ | drawn, long press ⁶⁰ | drawn ⁶⁰ |
| Opening URLs and files | ✅ ²⁶ | ✅ ²⁶ | ✅ ²⁶ | ✅ ⁸¹ | ✅ ⁸² | ✅ ¹⁰ |
| URLs that open the app (`App::on_open_url`) | at launch ⁸⁸ | at launch ⁸⁸ | ✅ ⁸⁸ | ✅ ⁸⁸ | ✅ ⁸⁸ | — ⁸⁸ |
| Notifications | ✅ ²⁶ | ✅ ²⁶ | ✅ ²⁶ | ✅ ⁵² | ✅ ⁶³ | ✅ ¹⁰ |
| Tray icon with a menu | ✅ ²⁶ | ✅ ²⁶ | ✅ ²⁶ | — | — | — |
| One running instance, later launches forwarded (`App::single_instance`) | ✅ ¹⁰ | ✅ ¹⁰ | ✅ | — | — | — |
| D-Bus activation: `org.freedesktop.Application`, the bus starting the app (`App::dbus_activatable`) | ✅ ⁹⁶ | — | — | — | — | — |
| Dock menu, reopening, files opened with the app, Window/Services/Help menus | — | — | ✅ | — | — | — |
| Dropping files and text from other apps | ✅ ²⁸ | ✅ ²⁸ | ✅ ²⁸ | ✅ ⁷⁹ | ✅ ⁷⁹ | ❌ |
| Dragging inside the app | ✅ ²⁹ | ✅ ²⁹ | ✅ ²⁹ | ✅ ²⁹ | ✅ ²⁹ | ✅ ²⁹ |
| Dragging files and text out to other apps | ✅ ³³ | ✅ ³³ | ✅ ³³ | long press ⁸⁶ | long press ⁸⁶ | ❌ |
| Asking before closing and quitting | ✅ ³⁷ | ✅ ³⁷ | ✅ ³⁷ | — | — | — |
| Background, foreground and memory warnings (`App::on_lifecycle`) | — | — | — | ✅ ⁵³ | ✅ ⁵⁷ | — |
| Back navigation (`App::on_back`) | — | — | — | ✅ edge swipe ⁶⁴ | ✅ ⁵⁷ | — |
| Predictive back (`App::back_enabled`, `App::on_back_progress`) | — | — | — | ✅ ⁶⁴ | ✅ ⁵⁸ | — |
| State kept across process death (`App::save_state`) | — | — | — | ✅ ⁶⁵ | ✅ ⁵⁹ | ✅ reload ⁶⁵ |
| Window focus (`Cx::window_focused`) | ✅ ³⁷ | ✅ ³⁸ | ✅ ³⁸ | ❌ | ✅ | ✅ |
| Attention requests, beep, badge | ✅ ³⁹ | ✅ ³⁹ | ✅ Dock badge ³⁹ | — | — | — |
| Badge and progress on the app's icon (`set_badge`, `set_progress`) | launcher entry ⁹⁹ | taskbar button ⁹⁹ | Dock tile ⁹⁹ | — | — | — |
| Title bar on Wayland (xdg-decoration, or drawn) | ✅ ⁴⁰ | — | — | — | — | — |
| Scenes rendered on the GPU (`stipple::gpu`, experimental) | Vulkan, GLES2 ⁴⁷ | Direct3D 11 ⁴⁷ | Metal ⁴⁷ | Metal ⁴⁷ | Vulkan, GLES2 ⁴⁷ | WebGPU, WebGL2 ⁴⁷ |
| The GPU by default (`Renderer::Auto`), on a hardware GPU | Vulkan, on X11 ⁸³ | ❌ ⁸³ | Metal ⁸³ | ❌ ⁸³ | ❌ ⁸³ | WebGPU, WebGL2 ⁸³ |
| Hidden windows draw no frames (`Event::Occluded`) | ✅ ⁸⁴ | ✅ ⁸⁴ | ✅ ⁸⁴ | in the background ⁸⁴ | ✅ ⁸⁴ | ✅ ⁸⁴ |
| Message boxes | drawn ⁴⁴ | native ⁴⁴ | native ⁴⁴ | native ³ | drawn ⁴⁴ | native ¹⁰ |
| Secure password entry | IME off | IME off | `EnableSecureEventInput` ¹⁰ | IME off | IME off | IME off |
| Sound: effects, streamed music, volume groups (`stipple::audio`) | ALSA ⁷¹ | WASAPI ⁷¹ | Core Audio ⁷¹ | Core Audio ⁷¹ | AAudio ⁷¹ | Web Audio ⁷¹ |
| Camera: live preview, every frame, torch, zoom, focus, lenses (`stipple::camera`) | ❌ | ❌ | AVFoundation ⁷² | AVFoundation ⁷² | Camera2 NDK ⁷² | `getUserMedia` ⁷² |
| Runtime permissions (`platform::permission`) | — | — | camera, microphone ⁷² | ✅ ⁷² | ✅ ⁷² | ✅ ⁷² |
| Video playback, decoded in hardware (`stipple::video`) | VA-API ⁹⁵ | Media Foundation ⁹⁵ | VideoToolbox ⁹⁵ | VideoToolbox ⁹⁵ | MediaCodec ⁹⁵ | `<video>` ⁹⁵ |
| In-app purchases (`platform::store`) | mock ⁷³ | mock ⁷³ | mock ⁷³ | StoreKit ⁷³ | Play Billing ⁷³ | mock ⁷³ |
| Calling the platform's own APIs: JNI access, activities started for a result (`platform::android`) | — | — | — | — | ✅ ⁸⁷ | — |
| App extensions and app groups' shared containers (`platform::shared_dir`) | — | — | `shared_dir` ¹⁰ | ✅ ⁸⁹ | — | — |
| The app's own platform code and manifest entries (`cargo-stipple`) | — | — | — | — | Java, dex, manifest fragment ⁹⁰ | — |
| Network status and changes (`platform::network`, `App::on_network`) | netlink, NetworkManager ⁹¹ | IP Helper ⁹¹ | Network.framework ⁹¹ | Network.framework ⁹¹ | `ConnectivityManager` ⁹¹ | `navigator.onLine` ⁹¹ |
| Secure storage for secrets (`platform::secure_store`) | Secret Service ⁹² | Credential Manager ⁹² | Keychain ⁹² | Keychain ⁹² | Keystore ⁹² | ❌ |
| Reference app (`examples/mediabrowser`): browse, sort, preview, play, accessible, four languages | ✅ ⁸⁵ | ✅ ⁸⁵ | ✅ ⁸⁵ | — | — | sample folder ⁸⁵ |

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
frames by Present's vblank notifies (a timer at the RandR refresh rate
without Present); Wayland, Windows and macOS use frame callbacks, `DwmFlush`
(a GPU window's DXGI waitable object) and `CVDisplayLink`. The `framepacing`
example checks an animation presents at roughly the display's rate and an
idle app presents nothing and never wakes: on Xvfb (CPU, Vulkan, GLES) and
headless sway; on Windows and macOS not blocking yet. The selftests print
where frames came from and what they cost; the macOS job also checks the
frame count loosely. See [gpu.md](gpu.md#frame-pacing).
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
Microsoft Japanese IME and Kotoeri (both required). See [ime.md](ime.md).
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
`adb shell cmd uimode night yes`), but not checked in CI. The accent color needs Android 12.
²¹ `prefers-color-scheme`, and its `change` event; the accent is the CSS
`AccentColor`, where the browser has one (Chrome reports its own default blue,
not the OS's). CI runs `webdemo`'s selftest with a dark and a light scheme and
checks the theme follows; a live switch is checked by handing the app a new
scheme the way the `change` listener does, since headless Chrome can't flip it.
²² An invisible overlay of ARIA elements mirrors the accessibility tree over the
canvas. CI checks the roles and names, that a click on an element (what a
screen reader's "activate" does) taps the node, and `aria-checked`; focus
moving to a control scrolled out of view scrolls it back. Not yet tried with
a real screen reader. See [the web page](platforms/web.md#accessibility).
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
text on `dropdemo` with an XDND source on X11, with a real OLE `DoDragDrop`
from another process on Windows, and drives the view's dragging methods with a
stand-in drag on macOS.
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
⁴⁷ `App::renderer(Renderer::Gpu)` or `STIPPLE_RENDERER`, behind the `vk`
and `gl` features (Linux, Android), `mtl` (macOS, iOS), `d3d` (Windows)
and `gpu` (the web, through `web/gpu.js`): boxes, paths,
images (textures cached per image), text, clips, canvases and layers with
transforms and opacity, games' sprites and pixel-art frames, all on the GPU.
Presented straight to the window through a Vulkan swapchain or an EGL window surface on X11 and Android, a
`CAMetalLayer` on macOS and iOS, a DXGI swap chain on Windows, the canvas on
the web, and on Wayland as `dma-buf`s through `zwp_linux_dmabuf_v1` (with
explicit sync through `wp_linux_drm_syncobj_v1` where the compositor has it,
in a subsurface under a title bar Stipple draws; read back where the
compositor or driver can't take them). CI compares test
scenes and an app with the software renderer under Mesa's llvmpipe (GLES)
and lavapipe (Vulkan), on the macOS runner's Metal device, on WARP and in
headless Chrome, and screenshots an app presented with each; the X11 and
Android present paths have run in CI only (Wayland's also locally on
NVIDIA), iOS is compile-checked.
External images (`dma-buf`, `IOSurface`, DXGI shared handles,
`AHardwareBuffer`, and on the web a `<video>`, `VideoFrame` or canvas) are
imported without a copy, NV12 on every native GPU API and an Android
camera's YUV through an external texture (GLES) or a sampler YCbCr
conversion (Vulkan); the `dma-buf` import (NV12 included) has run on GLES
(llvmpipe, NVIDIA) and Vulkan (lavapipe, NVIDIA), the shared NV12 texture on WARP,
the camera's hardware buffers on the Android emulator (GLES; Vulkan too,
but its gfxstream misreads their chroma, so it's off there by default), the
others are compile-checked; the camera preview hands the GPU its own
buffers on macOS, iOS, Android and the web.
See
[gpu.md](gpu.md).
⁴⁸ Through the Text Services Framework (a hand-written `ITextStoreACP` text
store, composition sink and display attributes), with IMM32 when TSF can't
be activated or `STIPPLE_TSF=0`. The text store's logic is unit-tested; CI
activates TSF on the Windows runner and drives a composition through the real
TSF manager (the app playing the input method), checking the preedit and the
commit reach the field. A required job types Japanese through the real
Microsoft Japanese IME. See
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
keyboard (added with this change, not yet run in CI). VoiceOver's
three-finger swipe (`accessibilityScroll:`) scrolls the container around an
element a page (unit-tested). Not yet tried with VoiceOver itself. See
[accessibility.md](accessibility.md#ios).
⁵⁰ `UITextInput` on the view: marked text as the preedit, commits, the field's
text around the caret for autocorrect and dictation, `replaceRange:withText:`
and the caret rectangle for the candidate bar. CI's `touchdemo` self-test on
the simulator drives these methods through objc dispatch (marked text, a
commit, a replacement) and reads the document back; no real keyboard composes
in CI. A hardware keyboard's dead keys compose through the same marked text
(⁷⁸). See [ime.md](ime.md).
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
set text, and scroll forward and back on every scroll container that can go
that way, with `TYPE_VIEW_SCROLLED`); `uiautomator dump` on the CI emulator lists the nodes, and
an accessibility service standing in for TalkBack (enabled the way TalkBack
is) moves accessibility focus through every node, clicks a button and sets
the field's text, from its own process. TalkBack itself, which the API 34
Google APIs emulator image ships, is turned on in CI too: its focus must
stop on the labels, reach the button and its click the app (passing in CI;
required). See
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
same-session check is new). The discard runs on an iPad simulator, booted in
the same job (the iPhone refuses to destroy a session: "The current device
does not support multiple scenes"), with `UIApplicationSupportsMultipleScenes`
in a copy of the bundle: the kill there restores in the same session, then
`touchdemo` destroys its own session (`requestSceneSessionDestruction:`,
UIKit's discard path), and CI checks the state was dropped (the save declined,
the file gone) and the next launch restores nothing (reported, not required;
its first run failed: UIKit never reported the discard and the next launch
reconnected the same session identifier, so the backend now drops the state
on the request itself, and a discard naming the connected session takes back
what was restored). A real swipe in the app switcher isn't
scripted. Web: the tab's `sessionStorage`,
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
the same under sway, and Orca itself, pass in CI and are required. A
field's caret and selection can be read and set (with caret-moved and
selection-changed events), a control declared `labelled_by` its caption has
labelled-by / label-for relations, and the app registers again when the
registry or the accessibility bus restarts; the walk checks these too
(passing locally, not yet seen in CI). A scroll container is a scroll pane
with scroll actions, a `Value` (its offset, settable) and `ScrollTo` on what
it holds, and a toast raises `object:announcement` from a polite live
region; a further step checks these, with a check box (passing locally, not
failing the job yet). Controller-for / controlled-by (a select and its
list, a tab and its panel), member-of (radio groups) and node-child-of /
node-parent-of (tree items) relations, the bus found from the X root
window's `AT_SPI_BUS` too, and actions on what's scrolled out of view
bringing it into view first (checked by steps passing locally, not failing
the job yet). See [accessibility.md](accessibility.md#linux).

⁶⁷ A hand-built provider per node, hosted by the window's provider: control
type, name, bounds in screen pixels, keyboard focus, hit testing, and the
Invoke pattern on buttons. CI walks a running window from PowerShell's
process with the .NET UI Automation client (the path Narrator and NVDA use),
hit-tests it, invokes a button and reads the focus after a Tab (passing in
CI). A focus move raises UIA's focus-changed event, and a window's root is
named by its title. NVDA itself is driven in CI: its log of what it says
must name the window, the field Tab focuses and "Row 0" "button", and
NVDA+Enter must press it (passing in CI; required); then Tab must reach
"Row 0", NVDA announce it, and Space and Enter press it (added later, not
yet run in CI). Value, Toggle, Selection / SelectionItem, ExpandCollapse,
Scroll / ScrollItem and Text (ranges by character, word, line and document)
patterns, property- and structure-changed events, and toasts announced
with UIA's notification event, checked through their vtables and from the
.NET client (required). `ControllerFor` and tree items' `Level`, and patterns that click
bring a node scrolled out of view into view first. See
[accessibility.md](accessibility.md#windows).

⁶⁸ An `NSAccessibilityElement` per node, with role, label and frame in screen
coordinates. CI reads a running app's tree from another process through the
AX API, as VoiceOver does (passing in CI). Buttons and the other controls a
click activates answer `AXPress` (VoiceOver's VO-Space), checked by the same
probe, and VoiceOver itself is driven through its AppleScript interface: it
must speak "Row 0" as a button and press it (both passing in CI;
required). The focused node is `AXFocused` and the application's
`AXFocusedUIElement`, and a focus move posts `AXFocusedUIElementChanged`
(added later, not yet seen in CI). Fields have a settable `AXValue` and
selected text range, checkboxes a value, scroll containers an
`AXScrollArea` with a settable scroll bar,
and tree swaps post value, selection, expansion and layout notifications;
`axprobe --depth` checks them (passing in CI; required). Title elements,
linked elements, outline rows' disclosure and static text values
(`axprobe --links`, not failing the job; not yet seen passing in CI).
See [accessibility.md](accessibility.md#macos).

⁶⁹ In the shared core, so the same everywhere: `Cx::memo` and `Cx::derive`,
transition-only frames drawn from the tree on screen, and scenes culled to the
window and clips. Unit- and App-tested (memo invalidation, handlers and ids
of reused subtrees, damage of a moving element, the same pixels culled or
not), with a benchmark of a 500-row list. On X11, CI checks that `hoverdemo`'s
hover fade is drawn without building the view; the other backends take the
same wake path, unchecked there. See [performance.md](performance.md).

⁷⁰ In the shared core: buttons and the other controls a tap activates
(checkboxes, switches, tabs, selects, list and tree items, rows, cells,
column headers; not menu items) join the Tab order, Shift+Tab goes back,
Enter or Space presses the focused one, the ring goes round it, and a click
doesn't focus it. App-tested; on X11, CI tabs from the field to a button in
`touchdemo` with `xdotool` and presses it with Space and Enter, and the
AT-SPI walk sees the button's focus events (both pass locally under Xvfb).
The focus reaches each platform's accessibility layer as a field's does;
the NVDA, VoiceOver and Orca steps that press a button from the keyboard
were added with it and haven't run in CI yet. Other backends build-checked.
See [accessibility.md](accessibility.md#keyboard-focus).

⁷¹ The mixer (voices, pan, groups, fades, streamed music, rate conversion)
is in the shared core and unit-tested off line. The macOS output was run
(`audiodemo --selftest` sees the `AudioUnit`'s callback pull frames at the
device's rate); CI runs the null output on Linux, and tries ALSA through the
`snd-dummy` driver without failing the run. The web glue was run against a
stand-in `AudioContext`. Windows, iOS and Android are build-checked only.
Output stops while a phone app is in the background. See [audio.md](audio.md).

⁷² The macOS camera was run (`cameraview`'s self-test sees frames with a
picture from a MacBook's camera, and the permission prompt answers). On the
iOS simulator, which has no camera, CI checks that opening one says so. On
the Android emulator CI grants the permission and reads frames from its
virtual camera (required). The web glue and the iOS and
Android permission prompts are build-checked only. The preview goes through
the renderer, a conversion per frame shown; with the Metal renderer the
frame's `IOSurface` is drawn without a copy (compile-checked). See
[camera.md](camera.md).

⁷³ StoreKit's original API on iOS (build-checked), the Play Billing Library
driven over JNI on Android (the library must be in the APK). CI runs the
Android driver end to end on an emulator against a stand-in for the library;
the real library was run up to Google Play's answer (an emulator without a
signed-in Play Store). Desktops and the web have no store and answer
`Unavailable`; the mock store runs everywhere. See [purchases.md](purchases.md).
⁷⁴ In the shared core, drawn by Stipple: a classic bar in a gutter on Linux,
Windows and the web; on macOS an overlay bar shown while scrolling (classic
under `Theme::native()` when *Show scroll bars* is *Always*); on iOS and
Android a thin indicator shown while scrolling, which a finger doesn't drag.
Pressing and holding the track pages after the platform's delay until the
thumb reaches the pointer; arrow buttons (on by default on Windows) step and
repeat; `scroll_both` areas have two bars, a corner and both gutters, and a
scrolling grid keeps a gutter; all mirrored right to left. A theme's skin
dresses the parts; under `Theme::native()` Windows' visual styles and
macOS's legacy `NSScroller` draw them (build-checked only). Each bar is a
`Role::ScrollBar` node (orientation, value and range) on AT-SPI, UIA, AX,
Android and the web (AT-SPI and the web's data unit-tested, the others
build-checked). App-tested (dragging, paging, holding on the test clock,
arrows, hover, two-way areas left to right and mirrored, the virtualized
table's and tree's extents, skins); on X11, CI drags `tabledemo`'s thumb with
`xdotool` and holds a press on `scrolldemo`'s track. The other backends
deliver the same pointer events, unchecked there. See
[widgets.md](widgets.md#scroll-bars).
⁷⁵ Laid out by the shared CPU text path, so the same everywhere; unit- and
App-tested (no character box over a shape's opaque pixels, pieces of a line
on both sides of a shape, floats, wrap sides, bidi, selection across the
pieces). CI runs `textflow --check` and drags its ring with `xdotool` on
X11; elsewhere only built. See [text-flow.md](text-flow.md).
⁷⁶ In the shared core: the editor, its keys, undo, and the Markdown and HTML
export and import are the same everywhere. CI types into it on X11 (Ctrl+B,
Ctrl+I, toolbar clicks that keep the editor focused, the sample image
centered at a relative width, an HTML paste from xclip, and a drag of the
image's resize handle) and reads the HTML and Markdown back, and checks the
rendering (a floated and a centered image) and both exports off screen;
elsewhere it is unit- and App-tested only (selecting, resizing and moving
images by the pointer included). Pasting HTML and images
from other apps works where the clipboard carries them (X11, Wayland,
Windows, macOS; HTML on the web; text only on iOS and Android). Code is
shown in the system's monospaced font, which the web doesn't have. See
[rich-text-editing.md](rich-text-editing.md).
⁷⁷ In the shared core, from the platform's touches, for text fields, text
areas and the rich editor: the handles and the web's edit menu are drawn by
Stipple; iOS 16+ shows the system's menu (`UIEditMenuInteraction`) and
Android 8+ a floating `ActionMode` (the drawn menu before those). App-tested
with synthetic touches; the iOS simulator self-test checks the system menu
(a warning rather than a failure), and CI's Android 14 emulator clicks the
floating toolbar's Copy through an accessibility service and reads the word
back from the clipboard in another app (required). See
[text-editing.md](text-editing.md#editing-with-a-finger).
⁷⁸ On a hardware keyboard. Android: a key whose `KeyCharacterMap` value is a
`COMBINING_ACCENT` shows its accent as a preedit and the next key commits the
letter `KeyCharacterMap.getDeadChar` composes (Space, or the same accent
again, types the accent; the state machine and a fallback table are
unit-tested); CI's emulator types Alt+E then E and Alt+U then U on a `uinput`
keyboard (the Generic key map's dead acute and diaeresis), which must give
`éü` (passed on a local API 34 emulator, not blocking in CI yet). iOS: UIKit's
text system composes, handing the accent over as marked text and the letter
through `insertText:` (or marked, then `unmarkText`), which the backend's
`UITextInput` takes as a preedit and a commit; `touchdemo`'s simulator
self-test drives that sequence through objc dispatch (required in CI); no
real hardware keyboard types in CI. See [ime.md](ime.md#dead-keys-on-mobile-hardware-keyboards).
⁷⁹ iOS: a `UIDropInteraction` on the view (iPad split view and Slide Over,
and iPhone apps since iOS 15); text and links as text, images and files
(`NSItemProvider` file representations) copied into the app's tmp directory
as files. Android: `View.onDragEvent` on the host view (split screen,
freeform windows): `ClipData` text, `file:` URIs as paths, `content:` URIs
(images, documents) copied through the `ContentResolver`. CI's Android
emulator drops a text and a `MediaStore` `content:` URI the app drags
itself with `View.startDragAndDrop` (`DRAG_FLAG_GLOBAL`), through the
system's `DragEvent`s (passed on local API 34 and 30 emulators, not blocking in CI
yet); `touchdemo`'s iOS self-test drives the drop delegate with a stand-in
session over real item providers (required in CI). Drags between two apps
aren't run in CI, and dragging out of the app isn't done on either. See
[drag-and-drop.md](drag-and-drop.md#ios-and-android).
⁸⁰ iPadOS 13.4 and later: a `UIPanGestureRecognizer` taking only indirect
scrolling (`allowedScrollTypesMask`: a trackpad's two fingers and a mouse's
wheel, in points, with the gesture's phases; no touches) and a
`UIHoverGestureRecognizer` for the pointer's moves while nothing is pressed;
momentum after the fingers lift (`UIScrollView`'s deceleration, as
`ScrollPhase::Momentum` steps), a secondary click as the right button (its
context menu), and the app's cursor as the pointer's shape
(`UIPointerInteraction`: the I-beam, hidden). The conversion and the
deceleration are unit-tested; `touchdemo`'s simulator self-test reads the
recognizers' settings back and feeds scroll steps, a flick, a secondary
click and hovers through the same code (required in CI); a real trackpad
or mouse isn't used in CI. See
[platforms/ios.md](platforms/ios.md#trackpad-and-mouse).
⁸¹ CI drives `touchdemo` on the iPhone simulator from an XCUITest runner
with real touches (required): a repeating timer
changes a label, a one-shot timer and a thread's `AppHandle::update` arrive
after a tap, the system edit menu's Paste brings in what `xcrun simctl
pbcopy` copied, the app writes the pasteboard (`simctl pbpaste` reads it
back; that read timed out in the run), and `open_url` brings Safari to the
foreground. Not tried on a physical device. See
[platforms/ios.md](platforms/ios.md#real-touches-from-xcuitest).
⁸² `examples/androiddemo/test-services.sh` (not blocking yet; passed on local
API 34, 33 and 30 emulators and in a CI trial run on each, three times in a
row): the clipboard both ways with another app (a
probe app that copies from a broadcast and reads once in front), and
`open_url` starting the `ACTION_VIEW` intent from the app into Chrome (the
activity manager's logged start; Chrome's first run switched off). Not
tried on a physical device. See
[platforms/android.md](platforms/android.md#running-on-an-emulator).
⁸³ Where the GPU path has met Phase 6's exit criteria (presenting straight
to the window, matching the CPU on the corpus in CI, the benchmarks' targets
on a hardware GPU, falling back when the device is lost or missing), in a
build with that platform's feature, and only on a hardware GPU: on
llvmpipe, lavapipe, WARP, SwiftShader or a virtual machine's device the
CPU renderer is faster, so `Auto` stays there (as on every CI runner).
Never for frames read back (`render_once`, tests), nor on Wayland yet
(decorated windows are read back). X11's default rests on an NVIDIA GPU
run locally; Windows, iOS and Android haven't run their GPU paths on the
hardware `Auto` would pick. `STIPPLE_RENDERER` and `App::renderer` still
choose. See [gpu.md](gpu.md#the-default).
⁸⁴ Minimized, covered, on another desktop or Space, or (Wayland) suspended by
the compositor: the window's frames stop, CPU and GPU, and the frame clock
once every window is hidden; it's drawn afresh when it shows. X11 unmaps,
`_NET_WM_STATE_HIDDEN` and `VisibilityNotify`; Windows `IsIconic`,
`DWMWA_CLOAKED` and Direct3D 11's `DXGI_STATUS_OCCLUDED`; macOS
`occlusionState`; Wayland `xdg_toplevel`'s `suspended`, or a frame callback
left unanswered for 500 ms (sway hides a workspace that way); Android the window
destroyed; iOS the app in the background; the web the canvas out of view.
`framepacing --hidden` checks it on X11 (openbox), Windows and macOS in CI
(required), and on Wayland under sway switching workspaces (locally, and in
CI, required); Wayland's
`suspended` against a scripted compositor, Android's compile-checked. See
[gpu.md](gpu.md#occlusion).

⁸⁵ `mediabrowser --selftest` drives the app off screen against a generated
folder (navigation, sorting, type-to-select, multi-select, image and Markdown
previews, a sound played and seeked, the thumbnail grid loading on worker
threads, French and Arabic) and then in a real window. Passes locally on
X11/Xvfb, with an AT-SPI walk of its tree; CI requires it on X11, Windows
(plus a UI Automation walk) and macOS (plus `axprobe`), and runs the AT-SPI
walk on X11 without failing the job. The web build browses the sample in memory
(checked in headless Chrome locally). See [reference-app.md](reference-app.md).
⁸⁶ A long press on a drag source of files or text lifts it out of the app:
iOS's `UIDragInteraction` (its own long press asks the app for the items:
an `NSString`, or a file representation per file) and Android's
`View.startDragAndDrop` (files as `content:` URIs of the app's
`FileProvider`, which the manifest declares). On Android a text and a file
dragged into another app (a probe app in a freeform window, or split
screen on API 33) passed on local API 34, 33 and 30 emulators, and onto the
app itself on local API 34 and 30 ones; in CI both on API 34, 33 and 30;
not blocking.
On iOS the simulator self-test asks the drag delegate for the items and
drops them on the app, and an XCUITest lifts and drops with a real touch
inside the app (both required in CI); a drag between two apps can't be
scripted on the simulator. See
[drag-and-drop.md](drag-and-drop.md#dragging-out-of-the-app).
⁸⁷ The `JavaVM`, the current activity, the application context, work run on
the Java main thread, and `start_activity_for_result`, whose callback survives
the activity being recreated meanwhile. `examples/intentresult` asks for VPN
consent (`VpnService.prepare`), cancels it, then agrees after a font-scale
change recreated the activity under the dialog: passed on local API 34 and 30
emulators; CI runs it on API 34, 33 and 30, not blocking yet. See
[platforms/android.md](platforms/android.md#calling-android-apis-yourself).
⁸⁸ Custom schemes, universal links (iOS) and verified App Links (Android),
claimed with cargo-stipple's `url-schemes` and `associated-domains`. iOS
hears them in the scene delegate (and the connection options at launch);
Android through Stipple's `Link` activity, a windowless trampoline in its APK
dex, as `NativeActivity` doesn't pass `onNewIntent` on: passed on local API
34 and 30 emulators cold, warm and from the background
(`examples/androiddemo/test-open-url.sh`), non-blocking in CI. On the iOS
simulator an XCUITest opens the URLs through the system, cold and warm
(passed in a CI scratch run; non-blocking until it has passed on master). macOS is
`application:openURLs:` (build-checked); Windows and Linux take the URL a
protocol handler starts the app with on its command line (checked on X11),
and with `App::single_instance` the one a second launch hands the running
app. The web has none: the page's URL is the
app's. See [os-integration.md](os-integration.md#urls-that-open-the-app).
⁸⁹ `cargo-stipple ios` builds `[[ios.extension]]` targets into
`PlugIns/<Name>.appex` (the `NSExtension` plist, their own entitlements and
provisioning profiles, signed before the app) and signs in `ios.app-groups`;
an extension's Rust `main` registers its principal class and calls
`platform::extension_main()`. CI packages `extensiondemo` (a notification
service extension in Rust) and, in the simulator, runs the extension's
executable in its bundle, where it reads what the app wrote in the group's
container (required in CI). The system starting the extension, and
Network Extensions (devices only), haven't been run. See
[packaging.md](packaging.md#app-extensions).
⁹⁰ `android.java` (sources compiled with the developer's `javac` and `d8`
against `android.jar`), `android.dex` (prebuilt dex files) and
`android.manifest-fragment` (services, receivers, providers, meta-data,
permissions and queries merged into the manifest `cargo-stipple` writes;
clashes with Stipple's own entries are errors). The classes call into the
app's Rust through `native` methods; Stipple defines no `JNI_OnLoad`.
`examples/androidservice` (a `Service` started with no activity and a
`BroadcastReceiver`) passed on a local API 34 emulator and in two CI trial
runs there (three times in a row each); CI packages it (required) and runs
it on API 34 without blocking. See
[packaging.md](packaging.md#the-apps-own-java-classes-and-components).
⁹¹ Online, the transport, metered, constrained (Low Data Mode, Data Saver)
and a generation that changes with the default network, as `Event::Network`
and `App::on_network`. Linux: run locally in a user and network namespace
(`unshare -rn`) whose links and default routes a script adds and removes,
through `platform::network` and through an App's hook on X11 (CI runs the
script, required); NetworkManager's metering and connection type
read on the desktop. Android: Wi-Fi, mobile data and Data Saver turned
off and on on local API 34 and 30 emulators, heard by androiddemo's hook
(Wi-Fi to cellular a new generation, without an offline between; on API
30 Data Saver shows a brief offline first); in CI not blocking yet. Windows
(IP Helper's route and connectivity hint notifications), macOS and iOS
(`nw_path_monitor`) and the web: build-checked; Windows reads the runner's
Ethernet as online in CI, and macOS reads its network in CI (both
required). See [network.md](network.md).
⁹² Small values (keys and tokens, up to 2560 bytes) by key, under the app's
id: Keychain generic passwords readable after the first unlock and never
restored to another device (macOS: the data protection keychain for a
signed app, the login keychain for an unsigned build), an optional keychain
access group shared with an app extension; on Android, files in
`no_backup/` sealed with AES-GCM by an `AndroidKeyStore` key (StrongBox
when there is one); Windows Credential Manager; the Secret Service over
D-Bus on Linux, unlocking through its prompt, and an error rather than a
plain file without one. CI runs the Linux checks against GNOME Keyring in a
private session (passing locally too), and the Android one on its
emulators (passing on local API 34 and 30 emulators); the Linux and Windows
steps are required. The macOS selftest passed through the login keychain
fallback, but the cross-process read failed until a fix that passed in a CI
scratch run (non-blocking until it has passed on master); the iOS
simulator's round trip passes and is required, the app's entitlements
linked into its executable's `__TEXT,__entitlements` section as Xcode does
for the simulator (`cargo stipple ios --simulator` does the same). See
[secure-storage.md](secure-storage.md).

⁹³ In a window, buttons, switches, sliders, progress bars and selects (and
checkboxes and radio buttons on macOS, and on Windows in the light look,
showing their own labels) are the OS's real controls, live
(`UIButton`, `NSSwitch`, a `BUTTON` child window, a `UIButton` with a
`UIMenu` / `NSPopUpButton` / `COMBOBOX` for a select, which picks through
the OS's own menu or list), with the OS's effects, input and accessibility;
one half scrolled out of a scroll area stays live, clipped by the OS (a
clipping container view per area on iOS and macOS, a window region on
Windows). A drag that starts on a slider in a list scrolls the list when
it goes up or down and moves the slider sideways (iOS's fingers, macOS's
wheel and trackpad, Windows' wheel), and on iOS a drag on a select scrolls
too while a tap opens its menu. Wherever one can't show (under an overlay, a transform or a fade,
cut by a clip that isn't a scroll area's; off screen), the OS draws a
picture of it instead. See
[native-theme.md](native-theme.md#real-controls).

⁹⁴ In a window, `text_field`, `number_input` and `password_field` are a
`UITextField` / `NSTextField` / `EDIT`, `text_area` a `UITextView` /
`NSTextView` / multi-line `EDIT`, editing the field's `TextInput`: typing,
selection, composition, the edit menu and undo are the OS's, and each edit
reaches the field's state and handlers; the state the app sets goes back
into the control. In CI (required steps, apart from iOS's extra checks)
`nativegallery`'s self-test typed, composed (iOS, macOS), set the state and
submitted through the real controls on all three; VoiceOver's elements and
the macOS accessibility dump listed the field once, named by its hint; on
Windows a real click and real keys typed into the `EDIT`, UI Automation
listed it once and MSAA named it by its hint. An iPad's hardware Tab moves
focus out of a `UITextField` / `UITextView` (key commands, self-tested
through UIKit's action dispatch, not with real keys); CI's Windows IME job
also composes Japanese into a live `EDIT` (required).
See [native-theme.md](native-theme.md#text-fields).

⁹⁵ MP4 and Matroska natively, with the `video` feature: H.264 and HEVC
through VideoToolbox on macOS and iOS (its `IOSurface`s drawn by Metal
without a copy), H.264 through VA-API on Linux (read back once), H.264 and
HEVC through Media Foundation on Windows (DXVA where there's a GPU, else
software; shared NV12 textures while Direct3D 11 draws) and `MediaCodec`
on Android (`AHardwareBuffer`s while the GPU renderer imports them), and
MPEG-1/2 everywhere through oxideav's software decoder; on the web,
whatever the browser plays, in a `<video>` the GPU renderer imports. No
sound yet. Run on macOS (VideoToolbox, Metal, the software decoder), on
Linux with NVIDIA's VA-API driver, on CI's Windows runner (Media
Foundation in software, Direct3D 11 on WARP drawing the frames) and the API
34 emulator (MediaCodec, GLES drawing the buffers; non-blocking); CI runs
the MPEG-2 tests on Linux; iOS and the web glue are compile-checked. See
[video.md](video.md).

⁹⁶ The single instance as a session bus name serving `Activate`, `Open`
and `ActivateAction` with `platform_data`'s activation token, startup id and
directory, as GApplication does; `cargo stipple linux` writes the
`DBusActivatable=true` `.desktop` file and the `.service` file. Unit-tested,
a second launch reaches the owner over a private `dbus-daemon` in `cargo
test`, and CI (required) packages and installs `instancedemo` and
lets a private bus start it for `gdbus call` (passing locally under Xvfb).
See [os-integration.md](os-integration.md#one-instance-reopening-and-the-dock).

⁹⁷ Shared code (shaping and the CPU text path): each emoji sequence goes
whole to the color font that has it and is joined by its `GSUB` (Noto Color
Emoji, Segoe UI Emoji) or `morx` (Apple Color Emoji, read by hand). CI's
`fontdemo --emoji` draws them through the system's fonts on Linux, Windows
and macOS and checks each is one emoji wide and in color (Segoe UI Emoji has
no country flags: they stay letters on Windows); iOS (Apple Color Emoji), Android
and the web (the fonts the app passes) use the same code, untested there.
See [fonts.md](fonts.md#emoji-sequences).

⁹⁸ Shared code: each build measures the view's root for the smallest size
it fits in without overflowing and the largest it has a use for, combines
them with the window's own `min_size`/`max_size` and hands them to the
backend's existing size limits (`WM_NORMAL_HINTS`, `xdg_toplevel`,
`WM_GETMINMAXINFO`, `contentMinSize`), growing or shrinking the window into
them; `App::layout_limits(false)` opts out and `App::natural_size(true)`
opens at the content's natural size. Unit- and App-tested; CI reads the X11
hints back with `xprop`. Wayland, Windows and macOS only build it. See
[layout.md](layout.md#window-size-limits).

⁹⁹ The badge and the progress: on Linux the `com.canonical.Unity.LauncherEntry`
`Update` D-Bus signal (count, progress, urgent; numbers only, matched to the
app's `.desktop` file by its id), which Ubuntu's Dock, KDE Plasma, Plank and
Latte show; on Windows `ITaskbarList3`'s overlay icon, drawn by Stipple (the
count, or a dot), and its progress bar; on macOS the Dock tile's badge and an
`NSProgressIndicator` over its icon. CI reads the Linux signals with
`dbus-monitor`, checks the Windows calls succeed (the taskbar can't be read
back) and reads the macOS badge and bar back. See
[window-lifecycle.md](window-lifecycle.md#attention-the-bell-the-badge-and-progress).

¹⁰⁰ Shared code saves the restored position and size and whether the window
is maximized (`WindowPlacement`) in the app's config directory and opens it
there next time, within its size limits (its content's minimum too); the
backend keeps it on a monitor that is still there (RandR on X11,
`EnumDisplayMonitors` on Windows, `NSScreen` on macOS). Wayland restores the
size and maximized state only (the compositor places windows). App-tested;
CI moves, resizes and reopens the lifecycle example on X11 and Windows (and
maximizes it on Windows), required; macOS is build-checked. See
[window-lifecycle.md](window-lifecycle.md#remembering-where-the-window-was).

¹⁰¹ The accessibility tree carries a slider's, progress bar's and number
input's value along its range (`AccessNode::range`, `Element::a11y_range`), a
table's size with all its rows (`AccessNode::table`, `Element::a11y_table`, a
virtualized table's rows numbered by `Element::a11y_row`) and each row's and
cell's place in it (`AccessNode::cell`), and how text is drawn
(`AccessNode::font`). Linux: AT-SPI `Value` (settable on a slider and a spin
button), `Table` and `TableCell`, a value-change event. Windows: UIA's
RangeValue, Grid, GridItem, Table and TableItem patterns, `LabeledBy`, text
attributes (font name, size, weight, italic, read-only) with `FindAttribute`,
and the text-changed and text-selection-changed events. macOS: `AXValue` with
`AXMinValue` / `AXMaxValue`, a slider's `AXIncrement` / `AXDecrement` and
settable value, `AXRowCount`, `AXColumnCount`, `AXRows`, a cell's index
ranges. iOS: a slider is adjustable (VoiceOver's swipe up and down) with its
value as words; no table or caption relation. Android: `RangeInfo`,
`ACTION_SET_PROGRESS` and scroll forward / backward on a slider,
`CollectionInfo` / `CollectionItemInfo`, `setLabeledBy`. Web: `aria-valuenow`
/ `min` / `max` / `valuetext`, `aria-rowcount`, `aria-rowindex`,
`aria-labelledby` (read-only). A screen reader sets a slider by pressing it at
the value's place (`platform::access`), a spin button by typing the number.
App tests set them through those events; from outside, the AT-SPI walk and
the UIA checks, the Android probe and the AX check (`axprobe --ranges`) pass
in CI and are required (iOS is build-checked). See [accessibility.md](accessibility.md#range-values-tables-and-captions).

¹⁰² `assets().open_fd(path)`: an open `File`, the asset's offset and length.
On Android the APK and the asset's range in it, for an asset stored
uncompressed (a compressed one is `AssetError::Compressed`); elsewhere the
asset's own file, with its path; the web has none. The Android emulator
reads an uncompressed PNG back through the descriptor and sees a compressed
file refused; the desktop path is unit-tested; iOS logs it in the simulator
self-test (not blocking). See
[os-integration.md](os-integration.md#packaged-assets).

Where the clipboard isn't wired up, copy and paste still work inside the app.
Where file dialogs aren't, `open_file` and friends return `None`, as if cancelled;
on iOS, Android and the web, which hand out contents rather than paths, so do
they: use `open_file_contents` and `save_file_contents` there.

## Topics

- [Layout](layout.md): stacks, wrapping, grids, and min/max sizes
- [The reference app](reference-app.md): a file and media browser built from the toolkit's pieces, what it shows, how it's tested, and what the toolkit gained for it
- [Lists, tables and trees](lists-and-tables.md): virtualized rows, selection, scrolling to a row, following the end, paging, sorting, resizing, swipe actions, pull to refresh
- [Widgets](widgets.md): select, number input, split pane, toolbar, toasts, tab bar,
  page stack and app bar, bottom sheet,
  password field and color picker
- [Menus](menus.md): the menu bar, shortcuts and context menus (long press on touch screens)
- [Tray icons, notifications and opening URLs](os-integration.md): `App::tray`, `notify`, `open_url`, `App::single_instance`, `App::dock_menu`
- [Network status](network.md): online or offline, Wi-Fi, cellular or a cable, metered and Low Data Mode, and hearing the default network change
- [In-app purchases](purchases.md): StoreKit and Google Play Billing, products, purchases, restoring, the mock store, and checking purchases on a server
- [Secure storage](secure-storage.md): `platform::secure_store`: secrets in the Keychain, the Android Keystore, Credential Manager and the Secret Service, sharing them with an app extension, and what each platform protects
- [Drag and drop](drag-and-drop.md): drop targets, their animated drag-over state, in-app drags, and dragging files and text out to other apps
- [Timers and background work](async.md): `AppHandle`, threads and tokio
- [Animation](animation.md): the display-driven frame clock, the spinner and hover fades,
  interaction states, transitions, enter/exit and layout animations, the
  built-in widgets' motion, theme cross-fades and reduced motion
- [Performance](performance.md): memoized subtrees and derived values (`Cx::memo`, `Cx::derive`), animation frames without a rebuild, painting only what shows, and what a frame costs
- [Custom painting](canvas.md): `Element::canvas`, invalidating parts of it from any thread, paths, bitmaps and glyph runs
- [Games](games.md): the game loop (`App::on_frame`), fixed time steps, polled input and rebindable actions
- [Transforms and opacity](transforms.md): scaling, rotating and moving any element, group opacity, and hit-testing through them
- [Text editing](text-editing.md): keyboard shortcuts, undo, double- and triple-click, selectable labels and paragraphs, a virtualized log view with find (plain or regex) and horizontal scrolling of long lines
- [Rich text editing](rich-text-editing.md): `rich_editor`, a formatting toolbar, headings, lists, links and images, and Markdown and HTML out and in
- [Fonts and text decorations](fonts.md): system fonts by name, bold and italic, fallbacks, metrics, underlines and strikethrough, rich text with inline icons, outlines and shadows
- [Text flowing around shapes](text-flow.md): floats, shapes in the middle of a column, images whose transparent pixels text runs over, margins, wrap sides and "through" wrapping, for plain and rich text
- [Accessibility](accessibility.md): what each platform's screen readers get, and how CI checks it from outside the app
- [Pointer events](pointer-events.md): every button, click counts, the wheel in lines or pixels, capture and the pointer leaving, for custom elements; touch gestures (scrolling, long press, pinch)
- [Right-to-left and bidirectional text](bidi.md): Hebrew, Arabic, mixed lines, editing them, and mirrored right-to-left layouts
- [Localization](localization.md): the user's languages, numbers, money, dates and times in their locale, plural forms, message catalogs, and following changes
- [The keyboard](keyboard.md): raw keys for terminals and games, keeping Tab, key repeat, Option as Meta
- [Gamepads](gamepads.md): connected pads, buttons and axes on a standard layout, rumble, and Linux device permissions
- [Input methods, dead keys and compose](ime.md): CJK IMEs, preedit, and Compose files
- [The native theme](native-theme.md): `Theme::native()` and OS-drawn controls
- [Images and SVG](images.md): the `image` and `svg` widgets, formats, and oxideav interop
- [Windows and cursors](windows-and-cursors.md): several windows opened and closed at run time, titles, position, size and state, size limits, maximize, fullscreen, the icon, and cursor shapes
- [Closing, quitting, focus and attention](window-lifecycle.md): asking before a window closes or the app quits, window focus, attention requests, the bell, Wayland title bars, and mobile apps going to the background
- [The camera and permissions](camera.md): `stipple::camera`'s live preview, frames on a capture thread, torch, zoom, focus and lenses; asking for the camera, microphone, notifications, photos and location
- [Video](video.md): `stipple::video`: playing MP4 and Matroska files decoded in hardware (VideoToolbox, VA-API, Media Foundation, MediaCodec) or software (MPEG-2), `video_view`, and frames drawn on the GPU without a copy
- [Sound](audio.md): `stipple::audio`: sound effects, streamed music, volume groups, the formats, each platform's output, latency, and the web's autoplay rule
- [Rendering on the GPU](gpu.md): `stipple::gpu` under `App::render_with`, what it draws how, how close it is to the software renderer, where it's the default, and hidden windows
- [High-DPI](high-dpi.md): logical and physical pixels, and where the scale comes from
- [CI and screenshots](ci.md): what CI checks, and how screenshots reach this repository
- [API guidelines](api-guidelines.md): for contributors: `#[non_exhaustive]`, options structs, what's public, and the audit that checks it
