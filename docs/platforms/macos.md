# macOS

The Cocoa backend calls AppKit through `objc_msgSend` directly, without the
`objc` or `cocoa` crates. A plain `cargo run` opens a regular window, with a Dock
icon, and no app bundle is needed.

## High-DPI

The framebuffer renders at the window's `backingScaleFactor`, and follows it when
the window moves to a display of another density. Positions stay in points,
which are logical pixels.

> Not yet seen on a Retina display: CI runners are 1x. Reports welcome.

## Appearance

`Theme::native()` reads dark mode from the `AppleInterfaceStyle` user default and
the accent from `NSColor.controlAccentColor`. The window's appearance follows the
theme's background, so a dark theme gets a dark title bar.

## OS-drawn controls

Under `Theme::native()` every control is drawn by a real, offscreen AppKit view
(`NSButton`, `NSSwitch`, `NSSlider`, `NSProgressIndicator`, `NSTextField`)
captured with `cacheDisplayInRect:toBitmapImageRep:`, so controls take the look of
whichever macOS version runs the app. Switch sizes come from AppKit, since they
differ between versions.

AppKit only draws a focus ring for a window's first responder, so Stipple draws
its own around the focused control.

## Fonts

`Font::system_default()` loads San Francisco (`SFNS.ttf`), falling back to
Helvetica and Arial, with Apple Symbols for symbols.

## Debugging

`STIPPLE_COCOA_A11Y=1` prints the accessibility tree as an assistive client would
see it, and the window number, which `screencapture -l` takes.

## Known gaps

- There's no app menu, so the menu bar shows the executable's name and no menus.
- No system clipboard or file dialogs yet.
