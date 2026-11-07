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
| Timers and background updates | ✅ | ✅ | ✅ | ✅ ³ | ❌ | ❌ |
| High-DPI | ✅ | ✅ ¹ | ✅ ¹ | ✅ | ❌ | ❌ |
| OS-drawn controls (`Theme::native`) | — ² | ✅ | ✅ | ✅ | ❌ | ❌ |
| System dark mode and accent | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| System clipboard | X11 only | ✅ ⁴ | ✅ ⁴ | ✅ ³ | ❌ | ❌ |
| File dialogs | ✅ | ✅ ⁵ | ✅ ⁵ | ❌ | ❌ | ❌ |
| Accessibility | AT-SPI | UI Automation | NSAccessibility | ❌ | ❌ | ❌ |

¹ Implemented, but CI only has 1x displays, so it hasn't been seen on a real high-DPI one.
² Linux has no native UI framework; controls stay self-drawn in the system's colors.
³ Implemented, but not yet run in CI on iOS.
⁴ CI round-trips text through the OS clipboard. Copy and paste shortcuts reach it
once these backends report Ctrl and Cmd (Phase 7b); `stipple::platform::clipboard`
works now.
⁵ Implemented, but a modal dialog can't be driven headlessly, so CI only builds it.

Where the clipboard isn't wired up, copy and paste still work inside the app.
Where file dialogs aren't, `open_file` and friends return `None`, as if cancelled.

## Topics

- [Timers and background work](async.md): `AppHandle`, threads and tokio
- [Text editing](text-editing.md): keyboard shortcuts, undo, double- and triple-click
- [The native theme](native-theme.md): `Theme::native()` and OS-drawn controls
- [High-DPI](high-dpi.md): logical and physical pixels, and where the scale comes from
- [CI and screenshots](ci.md): what CI checks, and how screenshots reach this repository
