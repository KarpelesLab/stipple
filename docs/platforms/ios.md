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

For the App Store, [`cargo-stipple ios`](../packaging.md) writes the bundle
below from a manifest in the app's `Cargo.toml` (with the icon catalog,
localized names and permission prompts, the privacy manifest and the
assets), signs it with your identity and provisioning profile, and zips the
`.ipa`; `--simulator` makes a simulator `.app`. See
[packaging.md](../packaging.md). What follows is the bundle by hand.

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
  <key>UIApplicationSceneManifest</key><dict>
    <key>UIApplicationSupportsMultipleScenes</key><false/>
  </dict>
</dict></plist>
```

- **`UILaunchScreen` is required.** Without a launch screen, iOS runs the app in
  its legacy compatibility mode: letterboxed, with black bands above and below.
- **`UIApplicationSceneManifest` opts into scenes**, which iOS asks of new
  apps: the window lives in a `UIWindowScene` (the app delegate names
  Stipple's scene delegate, so the manifest needs no configurations), and the
  saved state can tell the user closing the app from the system ending it
  (see [Keeping the state across process death](#keeping-the-state-across-process-death)).
  Without it the app runs without scenes, as before iOS 13.
  `UIApplicationSupportsMultipleScenes` can be `<true/>` for an iPad app
  (CI runs one that way); Stipple keeps one window, which moves to the scene
  that connected last.
- **`MinimumOSVersion` must be at least 15.0**: `Theme::native()` uses
  `UIButtonConfiguration`, which arrived in iOS 15.
- **`CFBundleLocalizations`** lists the languages the app is translated into
  (`<array><string>en</string><string>fr</string></array>`). iOS offers a
  per-app language in the Settings app only when the bundle declares more than
  one, and `platform::preferred_languages()` then starts with the one picked
  there. See [localization.md](../localization.md).

The app's packaged files ([`platform::assets`](../os-integration.md#packaged-assets))
go in an `assets` directory inside the bundle, next to the binary
(`YourApp.app/assets/`): Stipple reads them under the main bundle's
`resourcePath`. CI copies `examples/touchdemo/assets` there for `touchdemo`'s
self-test. (On macOS the directory is `Contents/Resources/assets`.) On-demand
resources aren't supported yet.

To run it in the simulator:

```sh
xcrun simctl install booted YourApp.app
xcrun simctl launch booted com.example.your-app
```

A device build must also be signed with your Apple developer identity and a
provisioning profile: `cargo-stipple ios` does it (see
[packaging.md](../packaging.md#ios)), or Xcode, or `codesign`.

## Layout

The app fills the screen's safe area at `UIScreen.scale`, so it stays clear of
the notch, the status bar and the home indicator. Around the safe area the
window shows the theme's background. The size passed to `App::logical_size`
doesn't apply: the screen decides. With `App::edge_to_edge(true)` the app
fills the whole screen instead and reads the safe area with
`cx.safe_area()` (the root view's `safeAreaInsets`; see
[layout.md](../layout.md#the-safe-area-phones)).

The window's root view controller is Stipple's own (built at run time). UIKit
lays its view out again whenever its bounds or safe area change (a rotation,
iPad split view and Stage Manager, the status bar coming and going), and
`viewDidLayoutSubviews` / `viewSafeAreaInsetsDidChange` then place the app's
view and tell the app: a new size (`Event::Resized`, drawn at once, inside
the rotation's animation) or, when only the safe area moved,
`Event::WindowChanged`. Its `supportedInterfaceOrientations` answers
`App::orientations` (UIKit's default until the app sets some; the
`Info.plist`'s `UISupportedInterfaceOrientations` must allow them too), and
its `preferredStatusBarStyle` has dark icons over a light top edge, light
ones over a dark one. `AppHandle::set_keep_awake` sets the application's
`idleTimerDisabled`. See
[windows-and-cursors.md](../windows-and-cursors.md#phones-orientation-and-keeping-awake).

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
their modifiers. The view's `UITextInputTraits` (keyboard type,
capitalization, corrections, the Return key's label, secure entry, content
type for autofill) follow the focused field's
[keyboard hints](../text-editing.md#keyboard-hints).

**Keyboard avoidance.** The keyboard's frame
(`UIKeyboardWillChangeFrameNotification`, `UIKeyboardWillHideNotification`)
gives how far it reaches over the view: `cx.keyboard_inset()`. The App lays
the view out above it and scrolls the focused field into view (see
[text-editing.md](../text-editing.md#the-on-screen-keyboard)). The view
itself keeps its size, so nothing is redrawn at another resolution.

**Editing with a finger.** Taps, double taps, long presses and the
selection handles in text fields are the shared core's (see
[text-editing.md](../text-editing.md#editing-with-a-finger)). The edit menu
is the system's on iOS 16 and later: a `UIEditMenuInteraction` added to the
view and presented at the selection (`presentEditMenuWithConfiguration:`,
with a delegate whose `editMenuInteraction:targetRectForConfiguration:`
gives the selection's rectangle). Its items are UIKit's suggested standard
actions: the view says yes to `cut:`, `copy:`, `paste:` and `selectAll:` in
`canPerformAction:withSender:` as the menu offers them (only while it
shows, so a hardware keyboard's Cmd+C still arrives as a key), and its
implementations of them reach the App as `Event::EditAction`. Before iOS 16
the App draws its own menu. `simulate::edit_menu_status()` and
`simulate::edit_menu_action(..)` read and drive it in `touchdemo`'s
self-test (added, not yet run in CI: reported as a warning).

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
A text field's element adds the traits a `UITextField` reports (UIKit has
no public text-entry trait), so VoiceOver says "text field" and XCUITest
finds it among `textFields`. The values ("on", "checked", "expanded"…) are
in the user's language (eighteen languages; English otherwise); no adjustable
(swipe up/down) or scroll actions yet. CI drives the app from an XCUITest
runner, outside its process; see [accessibility.md](../accessibility.md#ios).

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
an app still running in the background), along with the scene session's
`persistentIdentifier`. The next launch reads the file, deletes it and, if the
scene connects in that same session, hands the bytes to `App::restore_state`
before the first frame; coming back to the foreground deletes it, since what
it holds is stale from then on.

As with UIKit's own state restoration, only the system ending the app brings
the state back. iOS keeps the scene session when it ends a suspended app to
free memory, so the state comes back; when the user swipes the app away in
the app switcher, UIKit destroys the session, calls
`application:didDiscardSceneSessions:` (at the next launch if the app was
gone), which deletes the file, and connects a new session, so the app starts
afresh. That takes the `UIApplicationSceneManifest` above: an app without
scenes isn't told about a swipe-away, and its state comes back after one too.
The new session may carry the old one's identifier, though (an iPad
simulator's does), with the discard only reported after the scene connected:
a discard naming the connected session takes back the state restored on
connecting, if the app hasn't had it yet. An app closing its own session
(`requestSceneSessionDestruction:`, where UIKit allows it) loses the state
right away: the backend deletes the file and saves nothing more under that
session (until UIKit refuses, or the app comes back to the foreground),
rather than wait for a discard UIKit may never report.
See
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

`platform::schedule_notification` adds a request under the app's id with a
`UNTimeIntervalNotificationTrigger` (a `UNCalendarNotificationTrigger` for a
daily or weekly repeat, at the first time's time of day), which iOS delivers
whether or not the app runs; `cancel_notification` removes the pending and
delivered ones, and `pending_notifications` asks the center. Scheduling
doesn't ask for permission: call
`platform::permission::request(Permission::Notifications, …)` first. The center's delegate is set while the app finishes launching, so the
tap that launched it reaches `App::on_notification_tapped` too.

## Permissions and the camera

`platform::permission` asks for and reports the camera and the microphone
(`AVCaptureDevice`), notifications (`UNUserNotificationCenter`), the photo
library (`PHPhotoLibrary`, read and write) and the location while in use
(`CLLocationManager`); `open_settings` opens the app's page in Settings.
**Each needs its usage string in the `Info.plist`, or iOS kills the app when
it asks:**

```xml
<key>NSCameraUsageDescription</key>
<string>Scans barcodes.</string>
<key>NSMicrophoneUsageDescription</key>
<string>…</string>
<key>NSPhotoLibraryUsageDescription</key>
<string>…</string>
<key>NSLocationWhenInUseUsageDescription</key>
<string>…</string>
```

`stipple::camera` captures through `AVCaptureSession` and an
`AVCaptureVideoDataOutput` whose delegate (a class built at run time) gets
NV12 frames on a dispatch queue; the back lens prefers the virtual
triple/dual-wide camera (macro focus close up). The frameworks are loaded the
first time they're used. The simulator has no camera: `Camera::lenses()` is
empty and `Camera::open` returns `CameraError::NoCamera`, which CI checks.
See [camera.md](../camera.md).

## Share sheet and haptics

`platform::share` presents a `UIActivityViewController` from the top view
controller, with the text, the URL and the files (written to the app's tmp
directory first); on an iPad it's a popover from `Share::anchor` (the middle
of the app's view without one). The subject goes to Mail through key-value
coding. `platform::haptic` plays a `UISelectionFeedbackGenerator`,
`UIImpactFeedbackGenerator` (light, medium, heavy) or
`UINotificationFeedbackGenerator`; the simulator has no haptic engine.
`touchdemo`'s simulator self-test schedules a notification and lists what's
pending; the share sheet is build-checked only.

## In-app purchases

`platform::store` uses StoreKit's original API (`SKProductsRequest`,
`SKPaymentQueue` and a transaction observer, a class built at run time),
since StoreKit 2 is Swift-only; Apple deprecated it in the iOS 18 SDK but it
still works and passes review. The binary links `StoreKit.framework`. The
observer is added as the app finishes launching when it has an
`App::on_transaction` hook, so unfinished transactions come back at once.
Build-checked only; testing needs a sandbox account or an Xcode StoreKit
configuration. See [purchases.md](../purchases.md).

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
with `Font::from_bytes` to be safe. Japanese, Chinese and Korean text falls
back to Hiragino Sans, PingFang and Apple SD Gothic Neo (CFF collections in
`/System/Library/Fonts`), loaded when text first needs them; the same caveat
applies.

## Known gaps

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
through `restore_state`, in the same scene session. Then it launches it with
`TOUCHDEMO_CLOSE_SCENE`: the app types into its field and, once CI sends it
to the background, asks UIKit to destroy its scene session
(`requestSceneSessionDestruction:`, which takes the path of a swipe-away)
before the save that comes as the app goes, and CI checks that save was
declined, the file was gone while the app still ran, and the next launch
restored nothing. It doesn't require a new session identifier or a discard:
the first CI run of this step saw neither after the request. UIKit took the request without
an error and never reported the session discarded, the next launch
connected a session of the same identifier, and (before the backend dropped
the state on the request) restored what was saved under it; the launch
before that reported a discard naming the session it had just connected.
Both are reported in the log. UIKit only
destroys a session where the app may have several scenes, so this part runs
on an iPad simulator, from a copy of the bundle with
`<integer>2</integer>` added to `UIDeviceFamily` and
`UIApplicationSupportsMultipleScenes` set to `<true/>` (the iPhone simulator
refuses: "The current device does not support multiple scenes"), after
checking the kill restores there too (`TOUCHDEMO_TYPE`).
