# iOS

The UIKit backend boots through `UIApplicationMain` with its own app delegate,
so the app is a normal Rust binary inside a `.app` bundle.

## Building

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo build --release -p your-app --target aarch64-apple-ios-sim   # simulator
cargo build --release -p your-app --target aarch64-apple-ios       # device
```

## Packaging

iOS only runs bundled apps. The smallest bundle is a directory holding the
binary and an `Info.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>your-app</string>
  <key>CFBundleIdentifier</key><string>com.example.your-app</string>
  <key>CFBundleName</key><string>Your App</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>UIDeviceFamily</key><array><integer>1</integer></array>
  <key>MinimumOSVersion</key><string>15.0</string>
  <key>UILaunchScreen</key><dict/>
</dict></plist>
```

- **`UILaunchScreen` is required.** Without a launch screen, iOS runs the app in
  its legacy compatibility mode: letterboxed, with black bands above and below.
- **`MinimumOSVersion` must be at least 15.0**: `Theme::native()` uses
  `UIButtonConfiguration`, which arrived in iOS 15.

To run it in the simulator:

```sh
xcrun simctl install booted YourApp.app
xcrun simctl launch booted com.example.your-app
```

A device build must also be signed with your Apple developer identity, which
Xcode or `codesign` handles.

## Layout

The app fills the screen's safe area at `UIScreen.scale`, so it stays clear of
the notch, the status bar and the home indicator. Around the safe area the
window shows the theme's background. The size passed to `App::logical_size`
doesn't apply: the screen decides.

## Input

Every finger arrives as an `Event::Touch` (its id, phase and position; the
view takes multiple touches). The first finger down is also the pointer: a
left-button press, moves and a release at its position, then `PointerLeft`, so
a lifted finger leaves no hover behind.

The toolkit recognizes the gestures, the same way on every platform (see
[pointer-events.md](../pointer-events.md#touch-gestures)): a finger dragged in
a scroll container scrolls it (sideways in a horizontal one) and flings it on
if it lifts while moving; taps still click, since a press only becomes a
scroll past 8 logical pixels; a finger held still for half a second on an
element with a context menu opens it; and two fingers pinch, pan and turn an
element with `on_pinch`.

**Menus.** Context menus are drawn by Stipple and open on a long press (or a
right-click with a pointer); they aren't `UIMenu` /
`UIContextMenuInteraction` popups, so they look like the app's theme and work
like everywhere else (a tap picks an item, a tap outside closes it). An app's
`App::menu` bar is drawn across the top of the window, as on Linux, and its
menus open on a tap; an app that doesn't want a menu bar on a phone leaves
`App::menu` out on iOS (with `cfg!(target_os = "ios")`).

**The keyboard.** The view adopts `UITextInput` (and so `UIKeyInput`): while a
text input has focus it becomes first responder, which brings up the on-screen
keyboard, and it resigns when focus leaves. `insertText:` types (a newline is
Enter) and `deleteBackward` is Backspace. A hardware keyboard types the same
way; its arrows, Home/End, Page Up/Down, Escape, Tab, Delete, F-keys and
Ctrl/Cmd shortcuts arrive as key events through `pressesBegan:withEvent:`, with
their modifiers.

**Composing.** The Japanese, Chinese and Korean keyboards compose through
`UITextInput`: `setMarkedText:selectedRange:` is the composition, shown
underlined at the caret (`Event::Ime(Ime::Preedit)`), and `insertText:` or
`unmarkText` commits it. The keyboard reads the field's text around the caret
(`textInRange:`, `selectedTextRange`), from what the app reports with
`Window::set_ime`, so autocorrect, QuickType and dictation have a document;
their `replaceRange:withText:` becomes a delete around the caret
(`Ime::DeleteSurrounding`) and a commit, and moving the caret from the
keyboard (dragging on the space bar) becomes arrow keys. `firstRectForRange:`
and `caretRectForPosition:` give the caret's rectangle for the candidate bar.
Positions are UTF-16 offsets held by `UITextPosition` and `UITextRange`
subclasses the backend builds at run time. See [ime.md](../ime.md).

## Accessibility

The view is a `UIAccessibilityContainer`: VoiceOver sees one
`UIAccessibilityElement` per node of the app's accessibility tree, in reading
order, with its label (a text field's text is its value), value (on/off,
checked, expanded), traits (button, static text, header, selected) and frame
in screen coordinates (`accessibilityFrame`, converted through the window;
`accessibilityFrameInContainerSpace` only works while UIKit's accessibility
runtime is loaded, so it isn't used). Containers (groups, lists,
toolbars) are elements VoiceOver doesn't stop on; their children are. A
double tap (`accessibilityActivate`) taps the middle of the node, so buttons
click and fields take focus. Elements are updated in place as the tree
changes, and an update VoiceOver should hear posts
`UIAccessibilityLayoutChangedNotification` (or
`UIAccessibilityScreenChangedNotification` when a dialog opens or closes).
The values are English words; no adjustable (swipe up/down) or scroll actions
yet.

## Appearance

With `Theme::native()` the app follows light and dark mode while it runs:
`traitCollectionDidChange:` on the view reports a change of the screen's
`userInterfaceStyle`, which arrives as `Event::ThemeChanged`, and the App
rebuilds the theme and repaints (the area around the safe area takes the new
background too). The screen's traits decide rather than the view's, since
UIKit switches a backgrounded app's traits back and forth to take its
snapshots.

## Background, foreground and memory warnings

The app hears `UIApplicationDidEnterBackgroundNotification`,
`WillEnterForegroundNotification` and `DidReceiveMemoryWarningNotification`
(posted with or without scenes) as `Event::Lifecycle` with
`Lifecycle::Background`, `Foreground` and `LowMemory`, which reach
`App::on_lifecycle`. In the background the App asks for no frames (the display
link pauses) and doesn't repaint; it rebuilds and repaints on coming back. Save
what must survive on `Background`: iOS may end a backgrounded app without
warning. A memory warning also drops the cached OS-drawn controls. See
[window-lifecycle.md](../window-lifecycle.md#going-to-the-background-mobile).

## Keeping the state across process death

With `App::save_state`, going to the background also asks the app for its
state (`Event::SaveState`) and writes the bytes to a file in the app's
`Library/Caches` (`UIApplicationWillTerminateNotification` writes it too, for
an app still running in the background). The next launch reads the file,
deletes it and hands the bytes to `App::restore_state` before the first frame;
coming back to the foreground deletes it, since what it holds is stale from
then on. This stands in for UIKit's state restoration, which is tied to view
controllers with restoration identifiers; as iOS doesn't tell a suspended app
that the user swiped it away, the state comes back after that too. See
[window-lifecycle.md](../window-lifecycle.md#keeping-the-state-across-process-death).

## The back swipe

While `App::back_enabled` says yes (or, without it, the app has an `on_back`
hook), a swipe from the left edge (the right one under a right-to-left theme)
is the back gesture of a `UINavigationController`: `on_back_progress` hears
`BackGesture::Started`, the progress and `Cancelled`, and a swipe that goes
far enough or is flicked calls `on_back`. The backend recognizes it from the
view's touches (24 points from the edge, then 10 inward, mostly sideways) and
cancels the finger in the app once it does; until then, and whenever back is
off, the edge's touches are the app's. See
[window-lifecycle.md](../window-lifecycle.md#the-back-swipe-ios).

## Notifications

`platform::notify` goes through `UNUserNotificationCenter`. The first call
asks the user's permission (alert, sound, badge), and the notification waits
for the answer; later calls are shown at once, or fail once permission is
refused. A notification shows while the app is on screen too (the center's
delegate answers `willPresentNotification:` with a banner). The bundle needs
nothing extra in its `Info.plist`.

## File pickers

`dialog::open_file_contents` and `dialog::save_file_contents` (or
`save_file_contents_then`, to hear how the save went) present a
`UIDocumentPickerViewController` from the key window's topmost view
controller, with a delegate built at run time:

- **Open** uses `initForOpeningContentTypes:asCopy:` with `asCopy`, so the
  app gets its own copy of the document, readable without security-scoped
  access. `FileDialog::accept` becomes `UTType`s (`.txt` through
  `typeWithFilenameExtension:`, `image/*` as `public.image`, a full MIME type
  through `typeWithMIMEType:`; nothing is `public.data`, any file). The copy
  is read, passed to the callback with its name, and deleted.
- **Save** writes the bytes to a file named `FileDialog::file_name`
  (`Untitled` if empty) in the app's tmp directory and presents
  `initForExportingURLs:asCopy:` for it, so the user copies it where they
  like; the file goes once the picker closes.

The call returns at once and the callback runs when the picker closes, from
the main thread (`None` / `false` if it was cancelled). `open_file`,
`save_file` and `pick_folder` return `None`.

## OS-drawn controls

Under `Theme::native()` UIKit draws buttons, switches, sliders, progress bars
and text-field frames: real, offscreen `UIControl`s snapshotted through the
app's window. A plain layer render misses parts of current controls, such as a
switch's knob. UIKit has no checkbox or radio button, so those stay self-drawn.

## Fonts

`Font::system_default()` finds San Francisco in the simulator, but this hasn't
been checked on a device, where the app is sandboxed. Bundle a font and load it
with `Font::from_bytes` to be safe.

## Known gaps

- The keyboard doesn't push the content up, so it can cover a field near the
  bottom of the screen.
- `UITextInput` knows the caret's rectangle only: `firstRectForRange:` answers
  it for any range, `selectionRectsForRange:` is empty and a touch on the text
  doesn't place the caret through the text system (the app does it). A
  selection made from the keyboard (Shift with the space-bar trackpad) isn't
  passed on.
- Hardware keyboard keys only reach the app while a text input has focus (the
  view is first responder only then); they're build-checked, not run in CI.
- File pickers answer with contents only (`open_file` and friends return
  `None`), and there is no folder picker. Choosing a file in the picker can't
  be scripted on the simulator (its contents run in another process), so CI
  checks the pickers come up, that cancelling reaches the app, and hands the
  delegate a picked file's URL itself (`documentPicker:didPickDocumentsAtURLs:`,
  as UIKit does once the user picks one): the open callback gets the bytes and
  the export says it succeeded. The picker's own copy (`asCopy`) and its
  export of the file aren't exercised.
- The clipboard (`UIPasteboard`) is wired up but untested; Cmd+C/V from a
  hardware keyboard reach it.

## Testing on the simulator

A simulator can't script touches or typing, so `examples/touchdemo` drives
itself when launched with `TOUCHDEMO_SELFTEST` set:

```sh
SIMCTL_CHILD_TOUCHDEMO_SELFTEST=1 xcrun simctl launch booted com.example.your-app
```

It uses `stipple::platform::backend::ios::simulate`, which sends `insertText:`
and `deleteBackward` to the view through objc dispatch, composes through its
`UITextInput` methods (`setMarkedText:selectedRange:`, `replaceRange:withText:`)
and reads the document back as the keyboard does, feeds synthetic touches to
the backend's touch path (a drag, a long press that opens a row's context
menu, a sideways drag of a strip, a two-finger pinch and swipes from the left
edge, with and without something to go back from), reads the
accessibility elements as VoiceOver does (`accessibilityElementCount`,
`accessibilityElementAtIndex:`, each element's label, value, traits and frame)
and activates one (a row, then the context menu's item), presents the
document pickers and cancels them (checking `presentedViewController` and the
callbacks), presents them again and answers them through their delegate with a
file it wrote (`simulate::pick_document`), posts a memory warning, and
reads the notification permission; it logs to `touchdemo.log` in the app's tmp
directory. CI runs it, and drives the second half from outside: it switches
the simulator to dark mode (`xcrun simctl ui booted appearance dark`), sends
the app to the background by launching Settings and brings it back by
launching it again, and checks the app followed the look, heard the lifecycle
events and built no frames while away. Nobody answers the notification prompt
in CI, so the notification itself isn't seen. Last, CI sends the app to the
background once more, ends it with `xcrun simctl terminate` (as iOS ends a
suspended app) and launches it again, and checks the field's text came back
through `restore_state`.
