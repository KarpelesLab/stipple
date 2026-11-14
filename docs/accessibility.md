# Accessibility

Stipple draws every control itself, so the operating system can't see them on
its own. Each frame, the `App` builds an accessibility tree from the layout
(`stipple::core::a11y`: roles, names, bounds, focus, selected / checked /
expanded state, a text field's caret and selection, and which element labels
a field), maps it to the platform-neutral `stipple::platform::A11yNode`
tree, and gives it to the window, whose backend exposes it to the platform's
accessibility API. `App::accessibility_tree()` returns the tree for tests.

| Platform | What the tree reaches | Checked in CI from another process |
| --- | --- | --- |
| Windows | UI Automation: a hand-built provider per node | yes, `System.Windows.Automation` |
| macOS | NSAccessibility: an `NSAccessibilityElement` per node | yes if the runner allows the AX API (see below) |
| iOS | UIAccessibility: a `UIAccessibilityElement` per node | yes, an XCUITest runner |
| Android | an `AccessibilityNodeProvider`, a virtual view per node | yes, an accessibility service, and `uiautomator` |
| Web | an ARIA overlay | in-page, from headless Chrome |
| Linux (X11, Wayland) | AT-SPI: an object per node, registered with the registry | yes, a pyatspi client (X11); sway and Orca required too |

None of these has been tried with a screen reader by a person. What CI
does, on each platform where it can, is read and drive the app from
**another process**, through the same channel the screen reader uses, and on
four of them it also runs **the screen reader itself** and checks what it
says or does:

| Screen reader | CI job / step | Status |
| --- | --- | --- |
| NVDA (Windows) | `screen-reader-windows` | passing in CI; required |
| VoiceOver (macOS) | `visual-macos`, *Screen reader (VoiceOver)* | passing in CI; required |
| TalkBack (Android) | `visual-android` (API 34) | passing in CI; required |
| Orca (Linux) | `a11y-dbus` (see [Linux](#linux)) | passing in CI; required |
| Narrator, JAWS, VoiceOver on iOS | none | not tried |

The rest of this page says what each check proves, and what it doesn't.

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
client still holds any of its elements. When focus moves to another node
(Tab), the window raises UIA's focus-changed event
(`UiaRaiseAutomationEvent`, `UIA_AutomationFocusChangedEventId`) for it, if
a client listens: screen readers follow focus through that event, and
without it only learn of a Tab when they ask again. The root, which has no
name of its own, leaves its name to the window's provider, so it reads as
the window's title ("Stipple Touch window").

**CI** (`visual-windows`, *Read and drive a running window through UI
Automation*): `.github/scripts/uia-walk.ps1` starts `touchdemo` and, from
PowerShell's process, uses the .NET UI Automation client (the cross-process
path Narrator, NVDA and JAWS use):

- gets the "Stipple Touch" window from its handle (`AutomationElement.FromHandle`
  on the process's main window, so no other top-level window is asked
  anything), and walks every
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

**NVDA** (`screen-reader-windows`): `.github/scripts/nvda-windows.ps1`
downloads a pinned NVDA release from nvaccess.org (checked against its
SHA-256), makes a portable copy with the launcher's
`--create-portable-silent` (or unpacks the launcher with 7-Zip if that
fails), and starts it with `-m` (no dialogs, sounds or start message), its
log at debug level, and a fresh configuration: no welcome, update or usage
dialogs, the laptop keyboard layout, Insert as the NVDA key, and a synth that
makes no sound (`.github/a11y/windows/stippleSpy.py`, loaded from the
configuration's scratchpad; NVDA's own "silence" synth never reports its
speech done, which stops say all). NVDA logs everything it says as
`Speaking [...]`, so the script reads its log while it types NVDA's
commands (`keybd_event`, which NVDA's keyboard hook takes as typed keys):

- the window brought to the front: NVDA names it "Stipple Touch";
- Tab: the app focuses its field, raises the focus event, and NVDA says
  "edit"; NVDA+Tab (report the focus) says it again;
- NVDA+B (read the foreground window): NVDA reads the controls through UIA,
  "Row 0" with its role "button" and "Rows tapped: 0";
- NVDA+Shift+Right (next object) from the field, until NVDA's navigator says
  "Row 0" "button";
- NVDA+Enter (activate the navigator object): NVDA invokes the button
  through UIA, the app logs the tap, and NVDA+B then reads "Rows tapped: 1";
- the mouse moved over "Row 1": NVDA's mouse tracking hit-tests through UIA
  and says what is there (reported, not required).

The job uploads NVDA's whole log. It passes in CI (the window's name isn't
spoken on the foreground change alone, only with the first focus inside
it, so that is reported, not required); the job is required.

Not covered: Stipple's buttons don't take keyboard focus (only fields and
other elements with key handlers do), so Tab can't reach them and Enter or
Space can't press them; with NVDA they are reached with the object
navigator, NVDA+B or the mouse, and pressed with NVDA+Enter. No Value,
Toggle, Selection, ExpandCollapse, Scroll or Text patterns (a field's text is
its name, and can't be set through UIA); of UIA's events only the focus
change is raised (no structure or property changes), so a screen reader sees
other changes only when it asks again. Narrator and JAWS aren't tried.

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
window's. Then it presses the button with `AXPress`
(`AXUIElementPerformAction`, what VoiceOver's VO-Space sends): the button
must list the action, the call must succeed, and the app must log the tap.

The elements of the controls a click activates (buttons, menu items,
checkboxes, switches, selects, tabs, column headers) are a run-time subclass
of `NSAccessibilityElement` whose `accessibilityPerformPress` clicks the
middle of the element (a pointer move, press and release queued for the
window, delivered by the event loop), so AppKit lists `AXPress` for them.

Reading another app through the AX API needs TCC's accessibility permission
for the reading process. `axprobe` prints `AXIsProcessTrusted()`; when the API
is refused (`kAXErrorAPIDisabled`), the step adds `axprobe` to the system TCC
database (GitHub's macOS runners run with SIP off) and tries again. If it is
still refused, the step passes with a warning and the tree hasn't been read
from outside; the in-process walk (`STIPPLE_COCOA_A11Y=1`) still runs.

**VoiceOver** (`visual-macos`, *Screen reader (VoiceOver)*):
`.github/scripts/voiceover-macos.sh` lets AppleScript control VoiceOver as
VoiceOver Utility's checkbox does (`SCREnableAppleScript` in VoiceOver's
defaults, and `/private/var/db/Accessibility/.VoiceOverAppleScriptEnabled`,
which root can write with SIP off as on GitHub's runners), grants Apple
Events to VoiceOver in the TCC databases for the processes that may run
`osascript` there, and turns off VoiceOver's welcome. Then it starts
`touchdemo` and VoiceOver (`VoiceOverStarter`), and moves VoiceOver's cursor
through the window depth first (`tell vo cursor to move right`, `move into
item`, `move out of item`, or the VO keys through System Events if
AppleScript refuses one), logging each phrase VoiceOver speaks (`content of
last phrase`), until it reaches "Row 0": the phrase must say it is a button.
`perform action` (VO-Space) must then press it: the app logs the tap. The
step passes in CI and is required (it exits 3, with a warning, when the
runner doesn't let AppleScript control VoiceOver).

Not covered: there's no `AXValue` for fields, no focus or layout
notifications, and the elements are built again each time AppKit asks for
them (after the tree changes or the window moves).

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

**CI** (`visual-android`): a small accessibility service stands in for
TalkBack in the checks TalkBack can't be scripted for:
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

**TalkBack**: the Google APIs emulator images ship TalkBack from API 33 on
(`/product/app/talkback`; the API 30 image doesn't), so on API 34
`.github/a11y/android/test-talkback.sh` turns TalkBack on, with the probe
service beside it; with TalkBack on, the probe only watches, logging the
app's accessibility events. TalkBack only hears input that goes through the
system's input pipeline (its keyboard interceptor and touch explorer are
input filters there), and `adb shell input` injects past those filters: an
`input tap` with touch exploration on is a plain tap, and `input
keycombination` never reaches TalkBack. So the script makes a virtual
keyboard and touch screen with the shell's `uinput` command and moves
TalkBack's focus with its keyboard command (Alt+Right) or, if that doesn't
reach the app, its swipe right, an item at a time, until its focus lands on
"Tap me" (a `TYPE_VIEW_ACCESSIBILITY_FOCUSED` event with that text). On the
way its focus must stop on a plain label ("Running for N s" and the like):
TalkBack reads a focusable node's children that can't act as part of it, so
the host view's node (the window) isn't focusable to accessibility, though
the view takes input focus; when it was, the labels got no stops. Then
TalkBack's click (Alt+Enter, or a double tap) must reach the app. The
notification permission is granted up front to the demo and to TalkBack,
whose dialogs otherwise come up in front of the app and hold TalkBack's
focus. On a failure the script logs the windows, the resumed activity and
`dumpsys accessibility` (the accessibility focused window). TalkBack's speech
isn't read (the emulator may have no voice). The check passes in CI and is
required.

Not covered: TalkBack's speech, explore by touch; `androiddemo`'s list is a
plain scroll container, which the tree reports as a group without scroll
actions.

## Web

An invisible overlay of ARIA elements mirrors the tree over the canvas. The
`visual-web` job's headless Chrome checks it from the page. See
[the web page](platforms/web.md#accessibility).

## Linux

X11 and Wayland windows serve their tree over **AT-SPI**, what Orca and the
other Linux screen readers read (`stipple::platform::atspi`, on the
hand-written D-Bus client in `stipple::platform::a11y`; no `libatspi`, `atk`
or `zbus`).

- **Connecting.** On a window's first tree a helper thread finds the
  accessibility bus: `$AT_SPI_BUS_ADDRESS`, or `org.a11y.Bus.GetAddress` on
  the session bus (`$DBUS_SESSION_BUS_ADDRESS`, else `$XDG_RUNTIME_DIR/bus`),
  which starts `at-spi-bus-launcher` when needed. With no session bus or no
  accessibility bus it gives up quietly and the windows stop handing their
  trees over; startup never waits for it. `NO_AT_BRIDGE=1` turns it off, as
  for GTK. (The X root window's `AT_SPI_BUS` property isn't read.) If the
  accessibility bus goes away after it was reached (`at-spi-bus-launcher`
  restarted), the thread keeps the trees, asks for the bus again (which
  starts a new one), reconnects and registers again, waiting 1, 2, 4… up to
  30 seconds between tries.
- **Registering.** It calls `org.a11y.atspi.Socket.Embed` on the registry's
  root, so the app appears among the desktop's applications, and watches
  the registry's name (`NameOwnerChanged` for `org.a11y.atspi.Registry`):
  when `at-spi2-registryd` restarts, the app embeds itself again in the new
  one (once: a registry started by the app's own `Embed` isn't asked twice,
  which would list the app twice). The app's root
  (`/org/a11y/atspi/accessible/root`) has the `Application` role and
  interface (toolkit "Stipple", its version, the id the registry sets) and
  the windows' frames as its children; the app's name is the program's.
- **Serving.** The thread answers calls itself, from the latest tree each
  window handed over (a swap clones the tree into an `Arc`, and is skipped
  when nothing changed), so the UI thread is only involved in actions. A
  node's object path is its place in the tree (`…/w{window}_{i}_{j}`), so it
  keeps its path across rebuilds while the tree's shape stays. Every node has
  `Accessible` (role, name, parent, children, index in parent, state,
  attributes, interfaces, relations) and `Component` (extents in
  screen, window and parent coordinates, `Contains`, `GetAccessibleAtPoint`
  to the deepest node, `GrabFocus` for fields and items). Buttons, menu
  items, checkboxes, switches, selects, tabs, column headers and list and
  tree items have `Action` with one "click", which clicks the middle of the
  node on the UI thread (handed over through the loop's wake, as UIA's Invoke
  on Windows). Labels, fields, password fields and spin buttons have `Text`
  (the text by character, word, sentence and line); fields have
  `EditableText` (setting, inserting or deleting text replaces the field's
  text through the input-method path, after a click to focus it); a spin
  button with a number has `Value`.
- **Caret and selection.** A text field's tree node carries its caret and
  selection (byte indices into its text, from the field's editable text,
  as `EditBuffer` has them), so `Text.CaretOffset`, `GetNSelections` and
  `GetSelection` read them in characters. `SetCaretOffset`, `SetSelection`,
  `AddSelection` (when nothing is selected: a field has one selection) and
  `RemoveSelection` move them: a click focuses the field if it hasn't the
  focus, then the field gets `Ime::SetSelection` (`KeyInput::SetSelection`
  for the element; `EditBuffer` selects the range, a plain caret when the
  ends are equal). A field whose name isn't its text (one with an
  `a11y_label`, a password field) carries no caret: its caret reads as the
  end of its text when focused, and the calls that move it are refused.
- **Relations.** `GetRelationSet` gives `labelled-by` on a control that
  names its caption with `Element::labelled_by(id)` and `label-for` on the
  caption (`Element::a11y_id(id)`), the way a `for` attribute ties a label
  to a field on the web: `label(&theme, "Note").a11y_id("note")` beside
  `text_editor(…).labelled_by("note")`. Ids are matched within the window's
  tree. Nothing is inferred: a label beside a field without them isn't
  related to it (Orca guesses one from the layout, as it does for any
  toolkit). Only AT-SPI exposes the relation so far (not UIA's `LabeledBy`,
  nor macOS's title element). There are no controller-for or member-of
  relations: the tree doesn't know which list a combo box opens or which
  radio buttons form a group.
- **Events.** Swapping a tree raises what changed, node by node at the same
  place: `object:property-change:accessible-name` (and `…-role`),
  `object:text-changed:delete`/`insert` for text,
  `object:text-caret-moved` (with the new offset) and
  `object:text-selection-changed` for a field's caret and selection,
  `object:state-changed` for
  checked, selected and expanded, `object:children-changed:add`/`remove`, and
  for the focus `object:state-changed:focused` and `focus:`. A window gaining
  or losing the keyboard focus raises `window:activate`/`deactivate` (and its
  frame's `active` state). New and closed windows are children added to and
  removed from the app's root. A swap that adds or removes nodes raises
  `object:children-changed` on their parent for each one (removed from the
  end first), so a client's cached child lists follow.

Screen coordinates on X11 are the window content's position on the root
window (kept up to date from `ConfigureNotify`); Wayland doesn't tell clients
where their windows are, so there they are the window's own.

**CI** (`a11y-dbus`, *Read and drive a running X11 window through AT-SPI*):
in a private session bus (`dbus-run-session`, where `org.a11y.Bus` starts the
accessibility bus and its registry) under Xvfb,
`.github/scripts/atspi-linux.sh` starts `touchdemo` and
`.github/scripts/atspi-walk.py`, a pyatspi client (the library Orca is built
on), from its own process:

- finds `touchdemo` among the registry's applications, with the application
  role and the toolkit name "Stipple", and walks its tree, printing each
  node's role, name, extents on screen and states;
- finds a push button "Row 0", an entry (the text field) and a label
  "Rows tapped: N" under a frame whose parent is the application;
- checks the button's extents are inside the frame's on screen, and that the
  frame's `GetAccessibleAtPoint` at its middle returns it;
- runs the button's "click" action: the app logs the tap, a property-change
  event arrives, and the label reads "Rows tapped: N+1";
- presses Tab (`xdotool`) and gets `object:state-changed:focused` and
  `focus:` for the entry, the only node whose state says focused;
- sets the entry's text through `EditableText`: the app logs it, a
  text-changed event arrives and the entry reads it back;
- reads the entry's caret after the typed text (offset 5), selects "ell"
  (`setSelection(0, 1, 4)`): the entry reports one selection (1, 4) with the
  caret at 4, and `text-selection-changed` and `text-caret-moved` events
  arrive; then `setCaretOffset(2)` moves the caret and clears the selection;
- follows the entry's `labelled-by` relation to its caption "Note" (on
  Linux, `touchdemo` puts a "Note" label beside the field), and the
  caption's `label-for` back to the entry;
- stops this session's `at-spi2-registryd` (its process found through the
  accessibility bus, never another session's), lets the bus start a new one
  (or starts it), and finds `touchdemo` among the new registry's
  applications again, with its frame and tree.

This is a required check; it passes locally under Xvfb (at-spi2-core 2.58,
pyatspi 2.58), and passed in CI before the caret, selection, relation and
registry-restart checks were added (those pass locally; not yet seen in CI).
Without the re-registration the last check fails, as it should. A further
step, *Survive the accessibility bus restarting*
(`.github/scripts/atspi-bus-restart.sh`), kills `at-spi-bus-launcher`
itself, asks for the bus again, and a fresh pyatspi client must find the
app on the new bus; it passes locally and doesn't fail the job yet. The same
walk under headless sway (`wtype` for the Tab) and **Orca** itself
(`.github/scripts/orca-linux.sh`: Orca runs headless with its debug log,
which records every utterance as `SPEECH OUTPUT`, and must speak the entry
after Tab) run in the same job, pass in CI and are required too. The sway
walk runs the new checks as well; they haven't been run under a Wayland
compositor outside CI.

`a11ydemo serve` still serves a fixed tree over the session bus for the
`dbus-send` checks.

Not covered: no text attributes, character extents are the node's box split
evenly (not the glyphs'), only labelled-by / label-for relations (and only
where the app declares them), a spin button's `Value` has no range, and a
plain scroll container has no scroll actions. Other desktops' bridges don't
expose the caret, selection or relations yet.
