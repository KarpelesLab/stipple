# macOS

The Cocoa backend calls AppKit through `objc_msgSend` directly, without the
`objc` or `cocoa` crates. A plain `cargo run` opens a regular window, with a Dock
icon, and no app bundle is needed.

## High-DPI

The framebuffer renders at the window's `backingScaleFactor`, and follows it when
the window moves to a display of another density. Positions stay in points,
which are logical pixels.

> Not yet seen on a Retina display: CI runners are 1x. Reports welcome.

## Presenting frames

Each frame is copied into the window's buffer and only what changed is marked
for redisplay (`setNeedsDisplayInRect:`), so the view's `drawRect:` repaints
just that area. The buffer is kept in Core Graphics' native layout (32-bit
little-endian, alpha skipped first) and tagged sRGB, and the window's backing
store is sRGB too (`NSWindow.colorSpace`), so drawing it is a plain copy, not
a per-pixel format and color conversion; the window server matches the
colors to the display. Before this, every frame redrew the whole window from
a straight-alpha RGBA image in the device color space.

Animation frames are paced by a `CVDisplayLink`, which stops after about
200 ms with nothing to animate. If it doesn't tick (as can happen on a
virtual display), the frame clock stops waiting for it after three missed
frames and paces frames with a timer at the refresh interval, instead of
every other refresh.

> Not yet measured on CI after these changes: the macOS runner used to get
> 10–12 frames during `asyncdemo`'s 600 ms load. `asyncdemo --selftest` now
> prints where its frames came from (the display link or the timer) and what
> presenting and `drawRect:` cost (see
> [animation.md](../animation.md#where-frames-come-from)); the macOS job
> fails below 10 frames and warns below 24.

## Appearance

`Theme::native()` reads dark mode from the `AppleInterfaceStyle` user default and
the accent from `NSColor.controlAccentColor`. The window's appearance follows the
theme's background, so a dark theme gets a dark title bar.

## OS-drawn controls

Under `Theme::native()` buttons, checkboxes, radio buttons, switches, sliders
and progress bars are real AppKit controls in the window's view (`NSButton`,
`NSSwitch`, `NSSlider`, `NSProgressIndicator`; see
[Real controls](../native-theme.md#real-controls)), live, with the look and
effects of whichever macOS version runs the app (Liquid Glass on macOS 26).
The primary button is tinted with the accent color (`bezelColor`) rather than
made the window's default button, so Return stays the app's. The controls
refuse first responder, so the keyboard stays with the app's view. The event
loop leaves a mouse press that lands on one to AppKit, whose click tracking
sends the control's action, which the app gets as the element's tap (or drag,
for a slider). The view's accessibility children vend the control itself where
it stands for a node.

A scroll wheel or trackpad scroll over a live slider is the list's when it
is mostly vertical (the app scrolls; AppKit doesn't get it, so the slider
doesn't move) and the slider's when it is sideways (AppKit's alone), a
trackpad gesture keeping the way its first move went through its momentum;
everywhere else it goes to the app and on to AppKit, whose controls pass
it up. `nativegallery`'s self-test posts a mouse wheel through the window
server (`CGEventPost`) over the slider in a list and checks the list
scrolled and the slider stayed (the sideways one is only logged).

A `select` is an `NSPopUpButton`: a click pops its menu up over it (the
chosen item under the pointer), and a pick sends its action, which runs the
element's handler for that option; a pick the app refuses is set back. Its
items go into its menu directly, so options with the same text stay apart.

The controls in a scroll area are subviews of a clipping container, a
flipped `StippleClipView` (an `NSView` clipping its subviews, layer-backed
with `masksToBounds`) at the area's viewport, nested with nested areas: one
half scrolled out stays live, cut at the area's edge. The container's
`hitTest:` finds only its controls, so a click elsewhere on it is the app's.
While controls are live, a Metal layer presents with Core Animation's
transaction, so a scrolled frame and the controls' new places show together.

Where a control can't be live (under an overlay, a transform, a fade, cut by
a clip that isn't a scroll area's; off screen) the same AppKit view draws it offscreen
into a picture with `cacheDisplayInRect:toBitmapImageRep:`. Switch sizes come
from AppKit, since they differ between versions.

Text fields are `NSTextField`s (`NSSecureTextField` for a password), and a
`text_area` an `NSTextView` in a bordered `NSScrollView` (see
[Text fields](../native-theme.md#text-fields)). Unlike the other controls they
take first responder: while one has it, the event loop hands key events
straight to AppKit (its field editor, the input methods, the Edit menu, undo),
and what the field holds (the field editor's text, `selectedRange`,
`markedRange`) is read back after every event AppKit handled and on the
delegate's change notifications, and sent to the app as edits. The delegate's
`control:textView:doCommandBySelector:` gives the app Return (`insertNewline:`),
Tab, Shift-Tab, Escape (`cancelOperation:`) and Up and Down in a one-line
field. The app focusing another element makes the window's view first
responder again. An invalid number being edited gets a red ring (its layer's
border).

AppKit only draws a focus ring for a window's first responder, so Stipple draws
its own around the focused control.

## Fonts

`Font::system_default()` loads San Francisco (`SFNS.ttf`), falling back to
Helvetica and Arial, with Apple Symbols for symbols.

## Debugging

`STIPPLE_COCOA_A11Y=1` prints the accessibility tree as an assistive client would
see it, and the window number, which `screencapture -l` takes.

## URLs that open the app

A bundle whose `Info.plist` claims a scheme (`CFBundleURLTypes`) gets its
URLs in `App::on_open_url`: Launch Services sends the `kAEGetURL` Apple
event, and AppKit hands its URLs to the application delegate's
`application:openURLs:`, at launch and while the app runs (file URLs left
out). cargo-stipple doesn't build macOS bundles, so the app's own bundle
carries the key; see
[os-integration.md](../os-integration.md#urls-that-open-the-app).

## Known gaps

- Without `App::menu`, the menu bar shows the executable's name and no menus.
- The system clipboard (`NSPasteboard`) works with Cmd+C/X/V; file dialogs
  (`NSOpenPanel` / `NSSavePanel`) are implemented but not exercised in CI, since
  a modal dialog can't be driven headlessly.
- Scrolling, modifier keys and Command shortcuts are in place, but CI doesn't
  drive them with scripted input yet.
