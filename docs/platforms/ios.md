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

A finger dragged in a scroll container scrolls it, and flings it on if it lifts
while moving; taps still click, since a press only becomes a scroll past 8
logical pixels. See [Android](android.md#how-it-works), which shares the toolkit
side.

**The keyboard.** The view adopts `UIKeyInput`: while a text input has focus it
becomes first responder, which brings up the on-screen keyboard, and it resigns
when focus leaves. `insertText:` types (a newline is Enter) and
`deleteBackward` is Backspace. A hardware keyboard types the same way; its
arrows, Home/End, Page Up/Down, Escape, Tab, Delete, F-keys and Ctrl/Cmd
shortcuts arrive as key events through `pressesBegan:withEvent:`, with their
modifiers.

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

- No composing input methods: the view is `UIKeyInput`, not `UITextInput`, so
  marked text (Chinese, Japanese) and autocorrection don't reach the app, and
  the keyboard doesn't push the content up, so it can cover a field near the
  bottom of the screen.
- Hardware keyboard keys only reach the app while a text input has focus (the
  view is first responder only then); they're build-checked, not run in CI.
- No file dialogs or accessibility yet.
- The clipboard (`UIPasteboard`) is wired up but untested; Cmd+C/V from a
  hardware keyboard reach it.

## Testing on the simulator

A simulator can't script touches or typing, so `examples/touchdemo` drives
itself when launched with `TOUCHDEMO_SELFTEST` set:

```sh
SIMCTL_CHILD_TOUCHDEMO_SELFTEST=1 xcrun simctl launch booted com.example.your-app
```

It uses `stipple::platform::backend::ios::simulate`, which sends `insertText:`
and `deleteBackward` to the view through objc dispatch, feeds synthetic touches
to the backend's touch path, and reads the view's keyboard state, then logs to
`touchdemo.log` in the app's tmp directory. CI runs it.
