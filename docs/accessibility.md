# Accessibility

Stipple draws every control itself, so the operating system can't see them on
its own. Each frame, the `App` builds an accessibility tree from the layout
(`stipple::core::a11y`: roles, names, bounds, focus, selected / checked /
expanded state), maps it to the platform-neutral `stipple::platform::A11yNode`
tree, and gives it to the window, whose backend exposes it to the platform's
accessibility API. `App::accessibility_tree()` returns the tree for tests.

| Platform | What the tree reaches | Checked in CI from another process |
| --- | --- | --- |
| Windows | UI Automation: a hand-built provider per node | yes, `System.Windows.Automation` |
| macOS | NSAccessibility: an `NSAccessibilityElement` per node | yes if the runner allows the AX API (see below) |
| iOS | UIAccessibility: a `UIAccessibilityElement` per node | yes, an XCUITest runner |
| Android | an `AccessibilityNodeProvider`, a virtual view per node | yes, an accessibility service, and `uiautomator` |
| Web | an ARIA overlay | in-page, from headless Chrome |
| Linux | nothing yet (see [Linux](#linux)) | no |

None of these has been tried with the screen readers themselves (Narrator,
VoiceOver, TalkBack, Orca, NVDA, JAWS) by a person. What CI does instead,
on each platform where it can, is read and drive the app from **another
process**, through the same channel the screen reader uses. The rest of this
page says what each check proves, and what it doesn't.

## Windows

The window answers `WM_GETOBJECT` with the root of a tree of hand-built UI
Automation providers (`stipple::platform::uia`, COM vtables written by hand).
The root is hosted by the window's own provider (`UiaHostProviderFromHwnd`),
so it sits in the desktop's tree under the window. Each node answers its
control type, name, bounds in screen pixels (logical bounds scaled by the
window's DPI and offset by the client area's position, worked out when
asked), keyboard focusability and focus; navigation goes to the parent,
siblings and children; the root finds the focused node and the deepest node
under a point. Buttons, menu items, checkboxes, switches, selects, tabs and
column headers have the Invoke pattern, which clicks the middle of the node.
The tree is rebuilt only when it changes, and a replaced tree is kept while a
client still holds any of its elements.

**CI** (`visual-windows`, *Read and drive a running window through UI
Automation*): `.github/scripts/uia-walk.ps1` starts `touchdemo` and, from
PowerShell's process, uses the .NET UI Automation client (the cross-process
path Narrator, NVDA and JAWS use):

- finds the "Stipple Touch" window under the desktop root, and walks every
  element under it with the control-view walker, printing each one's control
  type, name, bounds, focusability and patterns;
- finds a `Button` "Row 0", an `Edit` (the text field) and a `Text`
  "Rows tapped: N";
- checks the button's bounds are inside the window's on screen, and that
  `AutomationElement.FromPoint` at its middle returns it;
- invokes the button through its Invoke pattern: the app logs the tap and the
  tree, read again, says "Rows tapped: N+1";
- presses Tab and reads `AutomationElement.FocusedElement`, which must be a
  control in the window with keyboard focus (when the window got the
  foreground).

`uiademo` still checks the providers through their vtables in-process.

Not covered: no Value, Toggle, Selection, ExpandCollapse, Scroll or Text
patterns (a field's text is its name, and can't be set through UIA); no UIA
events (`UiaRaiseAutomationEvent`, structure and focus changes), so a screen
reader only sees a change when it asks again.

## macOS

The view is an accessible `AXGroup` whose `accessibilityChildren` are
`NSAccessibilityElement`s built from the tree, each with its role
(`AXButton`, `AXTextField`, `AXStaticText`...), label and frame in screen
coordinates (the node's bounds converted through the view's window).

**CI** (`visual-macos`, *Accessibility from outside the app*): `axprobe`
(`.github/a11y/macos/axprobe.swift`) reads a running `touchdemo` through the
AX API, from its own process, as VoiceOver does: `AXUIElementCreateApplication`,
then `AXWindows`, `AXChildren`, `AXRole`, `AXDescription`, `AXTitle`,
`AXValue`, `AXPosition` / `AXSize`, `AXFocusedUIElement`. The step checks the
window "Stipple Touch", an `AXButton` "Row 0", an `AXTextField` and an
`AXStaticText` "Rows tapped: N", and that the button's frame lies inside the
window's. It also tries `AXPress` on the button and prints the result.

Reading another app through the AX API needs TCC's accessibility permission
for the reading process. `axprobe` prints `AXIsProcessTrusted()`; when the API
is refused (`kAXErrorAPIDisabled`), the step adds `axprobe` to the system TCC
database (GitHub's macOS runners run with SIP off) and tries again. If it is
still refused, the step passes with a warning and the tree hasn't been read
from outside; the in-process walk (`STIPPLE_COCOA_A11Y=1`) still runs.

Not covered: elements don't implement `accessibilityPerformPress` (VoiceOver's
VO-Space does nothing on a button), there's no `AXValue` for fields, no
focus or layout notifications, and the elements are built again each time
AppKit asks for them.

## iOS

The view is a `UIAccessibilityContainer` with one `UIAccessibilityElement`
per node: label, value (a field's text), traits, frame in screen
coordinates, and `accessibilityActivate` (VoiceOver's double tap) tapping the
node. UIKit has no public trait for a text field, so a field's element adds
the traits a `UITextField` reports (the text-entry trait VoiceOver reads as
"text field"). See [the iOS page](platforms/ios.md#accessibility).

**CI** (`visual-ios`, *Accessibility from outside the app (XCUITest)*): an
XCUITest bundle (`.github/a11y/ios`, an Xcode project generated by XcodeGen
on the runner) runs in its own process on the simulator and drives
`touchdemo` through the AX channel VoiceOver uses:

- `XCUIApplication(bundleIdentifier:)` launches the app, and the tree is read:
  the counts of all elements, buttons, static texts and text fields (one),
  and the whole snapshot, printed;
- the "Row 0" button exists, has a frame on screen and is hittable there;
- a tap on it (XCUITest taps at the element's frame) reaches the app: the
  static text "Rows tapped: N" becomes "Rows tapped: N+1";
- the field is found among `textFields`, a tap on it brings up the software
  keyboard, "H" and "i" are tapped on the keyboard's keys (found through
  accessibility too), and the field's value then holds "Hi".

`touchdemo`'s self-test also reads the elements in-process and activates a
row through `accessibilityActivate`, which XCUITest can't call.

Not covered: no adjustable (swipe up/down) or scroll actions; VoiceOver's
own gestures and speech aren't exercised.

## Android

The host view's `AccessibilityNodeProvider` gives a virtual view per node:
the closest widget class (`Button`, `EditText`, `TextView`...), text, state
(focused, selected, checked, editable, password), bounds on screen, and
actions (accessibility focus, click, focus, select, set text, and scroll on
lists, trees and tables). See [the Android page](platforms/android.md#accessibility).

**CI** (`visual-android`): the emulator images don't include TalkBack (it
comes from the Play Store as the Android Accessibility Suite, which CI can't
install), so a small accessibility service stands in for it:
`.github/a11y/android` (built with javac, d8 and aapt like the app, no
Gradle). `test-probe.sh` installs it and turns it on with
`settings put secure enabled_accessibility_services`, exactly how TalkBack is
enabled; it runs in its own process and reaches the app only through the
system's accessibility framework. It:

- walks the active window's node tree and logs each node (class, text,
  bounds, visibility, actions): the button "Tap me" (`android.widget.Button`
  with a click action) and the field (`EditText`, editable, with set text);
- moves accessibility focus to every node TalkBack would stop on (visible,
  and clickable, editable or with something to say), one at a time in
  reading order, and checks after each that the window's accessibility focus
  (`findFocus(FOCUS_ACCESSIBILITY)`) is on that node, and that
  `TYPE_VIEW_ACCESSIBILITY_FOCUSED` events came;
- clicks "Tap me" with `ACTION_CLICK` (TalkBack's double tap): the app logs
  the tap, and `TYPE_VIEW_CLICKED` comes;
- sets the field's text with `ACTION_SET_TEXT`: the app logs it, and the
  field's node reads it back;
- tries `ACTION_SCROLL_FORWARD` on a scrollable node, if there is one.

`test-on-device.sh` also reads the tree with `uiautomator dump` (another
out-of-process client of the same framework) and taps nodes at their bounds.

Not covered: TalkBack itself (its speech, its own traversal rules, explore by
touch); `androiddemo`'s list is a plain scroll container, which the tree
reports as a group without scroll actions.

## Web

An invisible overlay of ARIA elements mirrors the tree over the canvas. The
`visual-web` job's headless Chrome checks it from the page. See
[the web page](platforms/web.md#accessibility).

## Linux

`stipple::platform::a11y` has a hand-written D-Bus client and an AT-SPI
server for a tree (`a11ydemo serve` exposes a Stipple UI's tree, and the
`a11y-dbus` job reads it back with `dbus-send` on a private session bus). But
the X11 and Wayland windows don't use it: `Window::set_accessibility_tree`
does nothing there, and nothing registers with the AT-SPI registry
(`org.a11y.atspi.Registry`, through `Socket.Embed`) on the accessibility bus.
So Orca, and any other AT-SPI client, sees no Stipple app on Linux. Wiring the
server into the event loop, registering with the registry, and implementing
the `Component`, `Action`, `Text` and `Application` interfaces Orca needs is
on the [roadmap](../ROADMAP.md).
