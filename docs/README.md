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
| Pointer input | ✅ | ✅ | ✅ | ✅ touch | ❌ | ✅ |
| Keyboard input | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ |
| Scroll wheel and trackpad | ✅ ⁶ | ✅ ⁶ | ✅ ⁶ | ❌ | ❌ | ❌ |
| Modifiers and shortcuts (Ctrl/Cmd, Shift-select) | ✅ ⁶ | ✅ ⁶ | ✅ ⁶ | ❌ | ❌ | ❌ |
| Timers and background updates | ✅ | ✅ | ✅ | ✅ ³ | ❌ | ❌ |
| Display-paced animation (spinner, hover fades) | ✅ ¹¹ | ✅ ¹¹ | ✅ ¹¹ | ✅ ³ | ❌ | ❌ |
| High-DPI | ✅ | ✅ ¹ | ✅ ¹ | ✅ | ❌ | ❌ |
| OS-drawn controls (`Theme::native`) | — ² | ✅ | ✅ | ✅ | ❌ | ❌ |
| System dark mode and accent | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| Following appearance changes live | ✅ ⁷ | ✅ ⁷ | ✅ ⁷ | ❌ | ❌ | ❌ |
| System clipboard | ✅ ⁸ | ✅ ⁴ | ✅ ⁴ | ✅ ³ | ❌ | ❌ |
| File dialogs | ✅ | ✅ ⁵ | ✅ ⁵ | ❌ | ❌ | ❌ |
| Accessibility | AT-SPI | UI Automation | NSAccessibility | ❌ | ❌ | ❌ |
| Window size limits, maximize, fullscreen | ✅ ⁹ | ✅ ¹⁰ | ✅ ¹⁰ | — | — | ❌ |
| Window icon | ✅ ⁹ | ✅ ¹⁰ | Dock icon ¹⁰ | — | — | ❌ |
| Mouse cursor shapes | ✅ ⁹ | ✅ ¹⁰ | ✅ ¹⁰ | — | — | ❌ |

¹ Implemented, but CI only has 1x displays, so it hasn't been seen on a real high-DPI one.
² Linux has no native UI framework; controls stay self-drawn in the system's colors.
³ Implemented, but not yet run in CI on iOS.
⁴ CI round-trips text through the OS clipboard, and Ctrl/Cmd+C/X/V reach it now
that these backends report modifier keys.
⁵ Implemented, but a modal dialog can't be driven headlessly, so CI only builds it.
⁶ CI drives these with scripted input on X11 only; on Wayland, Windows and macOS
they're implemented and unit-tested but not yet exercised end to end.
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

Where the clipboard isn't wired up, copy and paste still work inside the app.
Where file dialogs aren't, `open_file` and friends return `None`, as if cancelled.

## Topics

- [Layout](layout.md): stacks, wrapping, grids, and min/max sizes
- [Lists, tables and trees](lists-and-tables.md): virtualized rows, selection, sorting, resizing
- [Timers and background work](async.md): `AppHandle`, threads and tokio
- [Animation](animation.md): the display-driven frame clock, the spinner and hover fades
- [Text editing](text-editing.md): keyboard shortcuts, undo, double- and triple-click
- [The native theme](native-theme.md): `Theme::native()` and OS-drawn controls
- [Images and SVG](images.md): the `image` and `svg` widgets, formats, and oxideav interop
- [Window control and cursors](windows-and-cursors.md): size limits, maximize, fullscreen, the icon, and cursor shapes
- [High-DPI](high-dpi.md): logical and physical pixels, and where the scale comes from
- [CI and screenshots](ci.md): what CI checks, and how screenshots reach this repository
