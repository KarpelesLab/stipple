# Android

Apps run in a `NativeActivity`, so no Java code is needed. The app is a
`cdylib` that Android loads from the APK, and the backend talks to the NDK's C
API (`libandroid`) directly, with a few hand-written JNI calls for what the NDK
lacks. There is no dependency on the `ndk` or `android-activity` crates. The
few Java classes a `NativeActivity` lacks (see [Run-time classes](#run-time-classes))
are written by the backend itself, as a dex file, when the app starts.

## The entry point

Android calls the library's `ANativeActivity_onCreate` on its main thread.
Hand it to `android::start` with your `main`, which then runs on a thread of
its own, the UI thread, and builds and runs an `App` as on any other platform:

```rust
use std::ffi::c_void;
use stipple::platform::backend::android::{self, ANativeActivity};
use stipple::prelude::*;

fn main() {
    App::new(State::default(), view).theme(Theme::native()).run();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ANativeActivity_onCreate(
    activity: *mut ANativeActivity,
    saved_state: *mut c_void,
    saved_state_size: usize,
) {
    unsafe { android::start(activity, saved_state, saved_state_size, main) };
}
```

`println!` and Stipple's own messages go to logcat under the `stipple` tag
(`adb logcat -s stipple`). See [`examples/androiddemo`](../../examples/androiddemo).

## How it works

- **Lifecycle.** Each activity callback (window created, resized and destroyed,
  input queue, focus, pause and resume, configuration change, destroy) is
  queued for the UI thread, which waits on an `ALooper`. The callbacks the
  framework needs finished before it goes on (a window or input queue going
  away, a redraw, the activity being destroyed) wait until the UI thread has
  handled them. When the activity is destroyed, the app gets `CloseRequested`
  and `App::run` returns; when the app exits on its own, the activity finishes.
- **Wakeups and timers.** `AppHandle::update`, timers and other threads wake
  the looper with `ALooper_wake`; the poll's timeout is the next timer.
- **Frames.** Animations are paced by `AChoreographer` (looked up at run time,
  as it's newer than API 21), with the frame clock's timer as the fallback.
- **The window.** Frames are rendered in software and copied into the
  `ANativeWindow`'s buffer, only the damaged part when the buffer allows it. The
  app gets the content rectangle the system bars leave free (it shrinks when
  the soft keyboard is up); the rest of the window, and the status and
  navigation bars, take the app's background color, with dark or light bar
  icons to match.
- **Touch and mouse.** Every finger arrives as an `Event::Touch` (its id,
  phase and position). The first finger down is also the pointer, until it
  lifts; a tap is a press and release, and lifting a finger leaves no hover
  behind. A mouse adds its right and middle buttons, hover and the wheel.
- **Touch scrolling.** A finger dragged in a scroll container scrolls it, and
  keeps it going (a fling that slows down, and stops at the end of the content)
  if it lifts while moving. A press only becomes a scroll once the finger has
  moved 8 logical pixels, mostly vertically, so taps still click; once it
  scrolls, the pressed button lets go and doesn't click. A finger on a slider
  drags the slider, and a finger that lands on a fling stops it. This lives in
  the toolkit, so it works the same on iOS; desktop mice keep the wheel.
- **Keys.** Named keys (arrows, Enter, Backspace, Delete, Home/End, PageUp/Down,
  Tab, Escape, F1–F12) become `Event::Key`, with Shift, Ctrl, Alt and Meta.
  Other keys go through the keyboard's own `KeyCharacterMap` (over JNI), so any
  layout types the right characters; with Ctrl or Meta held they're shortcuts
  (Ctrl+C, Ctrl+V, ...). Back, volume and other system keys stay with the system.
- **The soft keyboard** shows when a text input has focus and hides when it
  loses it (through `InputMethodManager`, for the host view), and types through
  an `InputConnection`: see [Input methods](#input-methods).
- **Back.** The Back key (and gesture) is `Event::BackRequested`, handled by
  `App::on_back`; unhandled, the activity does the system's default
  (`onBackPressed`). See [window-lifecycle.md](../window-lifecycle.md#going-back-android).
- **Background and foreground.** `onPause` / `onResume` are
  `Event::Lifecycle(Background / Foreground)`, `onLowMemory` and
  `onTrimMemory` (running low and worse) `LowMemory`, heard by
  `App::on_lifecycle`; frames stop in the background. `onPause` waits for the
  app to handle it.
- **High-DPI.** The scale factor is the screen density over 160 dpi (2.625 on a
  420 dpi phone); a density change resizes the content.
- **System look.** `Theme::native()` follows night mode and, on Android 12 and
  later, the wallpaper's accent color (`system_accent1_*`), and switches live
  when night mode changes. Its surfaces are Material 3's baseline colors.
- **Clipboard.** `ClipboardManager` over JNI, so copy and paste reach other apps.
- **Fullscreen.** `WindowAttributes::fullscreen` and `AppHandle::window`
  hide the status bar.

## Run-time classes

An input method talks to a `View` (`onCreateInputConnection`), and
accessibility services to a view's `AccessibilityNodeProvider`; a
`NativeActivity` has neither. Rather than ship Java, the backend assembles
four tiny classes into a dex file at start-up (`stipple::platform::dex`, a
hand-written writer) and loads them with `InMemoryDexClassLoader` (Android 8
and later; before, `DexClassLoader` from a file in the app's private
storage):

- `org.stipple.android.HostView`, a `View` added on top of the activity's
  content. It draws nothing and takes no input of its own (the native input
  queue gets events first); it has the keyboard focus, so the input method
  connects to it, and it serves the accessibility tree.
- `org.stipple.android.Input`, a `BaseInputConnection`.
- `org.stipple.android.Nodes`, an `AccessibilityNodeProvider`.
- `org.stipple.android.Callbacks`, a `ComponentCallbacks2`, for
  `onTrimMemory`.

Every method is `native`, bound to Rust with `RegisterNatives`; the only
bytecode is each constructor's call to its superclass's. It runs on the main
(Java) thread, and talks to the UI thread through the backend's command
queue. If loading fails (logged as `stipple: no host view`), the app runs as
before: keys only, no accessibility. `adb logcat -s stipple` shows
`stipple: host view installed` when it works.

## Input methods

The host view's `InputConnection` gives the keyboard a document to read and
edit, so composing keyboards (Japanese, Chinese, Korean), emoji, Gboard's
suggestions and autocorrect work as in any Android app:

- `setComposingText` is the preedit (`Ime::Preedit`), `commitText` and
  `finishComposingText` commit (`Ime::Commit`), `deleteSurroundingText` (and
  its code-point variant) is `Ime::DeleteSurrounding`. `setComposingRegion`,
  which keyboards send to compose an existing word again (when the caret
  comes back to it, or after keys typed on a hardware keyboard), is declined:
  Stipple only composes new text at the caret, and turning the word back into
  a preedit would change the app's text under it. `setSelection` moves the
  caret with arrow keys.
- Keys typed on a hardware keyboard (or with `adb shell input`) arrive as keys
  whether or not an input method is connected. A key typed while the input
  method composes commits the composition first, and until the field reports
  its text again, caret positions the input method sends are ignored (they
  were computed from text the key has changed).
- `getTextBeforeCursor`, `getTextAfterCursor`, `getSelectedText` and
  `getCursorCapsMode` read the field's text (`ImeState`, with the composition
  inserted at the caret), in UTF-16 units as Java counts them. The backend
  updates its copy as soon as the keyboard edits it, before the field has
  run, and takes the field's word for it once the edits are in.
- The keyboard's action key presses Enter (Done, Go, Search, Send), Tab
  (Next) or Shift+Tab (Previous); its Select all / Cut / Copy / Paste are the
  usual shortcuts. Keys it sends as key events (Backspace, Enter on most
  keyboards) arrive as keys; a key typed while it composes commits the
  composition first.
- The field's focus restarts the input method (`restartInput`); caret and
  text changes it didn't make are reported with `updateSelection`.
- A password field gets a password `inputType`, so the keyboard shows no
  suggestions and learns nothing; its text isn't shared.

`stipple::platform::backend::android::input_connection_self_test()` drives
the focused field's connection as a keyboard would and logs what it reads
back; `androiddemo` runs it in CI, which has no composing keyboard to type
with.

## Accessibility

The node provider gives TalkBack (and `uiautomator`, and other services) a
virtual view per node of the accessibility tree: the role as the closest
Android widget's class (a button reads as `android.widget.Button`, a field as
`EditText`, text as `TextView`, a checkbox as `CheckBox`...), the name as its
text (or a container's content description), its state (focused, selected,
checked, editable, password) and its bounds on screen. Actions: accessibility
focus, click (a tap at the node's center), focus and select (the same tap),
scroll forward and backward (a wheel step over it), set text (select all,
then type). Changes to the tree send `TYPE_WINDOW_CONTENT_CHANGED`, and a
moved focus `TYPE_VIEW_FOCUSED`. A hovering finger (TalkBack's explore by
touch) goes to the host view, which announces the node under it.

```sh
adb shell uiautomator dump /sdcard/ui.xml && adb shell cat /sdcard/ui.xml
```

lists the nodes; it is what CI checks.

## Building

The app's crate builds as a `cdylib`, under the name the manifest gives:

```toml
[lib]
name = "stipple_android"
crate-type = ["cdylib"]
```

Point Cargo at the NDK's linker for each target:

```sh
rustup target add aarch64-linux-android x86_64-linux-android
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android21-clang"
cargo build --release --target aarch64-linux-android
```

Building only the `stipple` library needs no NDK: nothing links until the cdylib.

## Packaging

The APK needs no Gradle. [`examples/androiddemo/build-apk.sh`](../../examples/androiddemo/build-apk.sh)
does all of this for the demo (`build-apk.sh x86_64 stipple.apk`, or
`arm64-v8a` for a phone). By hand, with the SDK's build tools:

```xml
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="com.example.yourapp">
  <uses-sdk android:minSdkVersion="24" android:targetSdkVersion="34"/>
  <application android:label="Your App" android:hasCode="false"
      android:theme="@android:style/Theme.DeviceDefault.Light.NoActionBar">
    <activity android:name="android.app.NativeActivity" android:exported="true"
        android:configChanges="orientation|screenSize|screenLayout|smallestScreenSize|keyboard|keyboardHidden|navigation|uiMode|density">
      <meta-data android:name="android.app.lib_name" android:value="stipple_android"/>
      <intent-filter>
        <action android:name="android.intent.action.MAIN"/>
        <category android:name="android.intent.category.LAUNCHER"/>
      </intent-filter>
    </activity>
  </application>
</manifest>
```

```sh
mkdir -p lib/arm64-v8a
cp target/aarch64-linux-android/release/libstipple_android.so lib/arm64-v8a/
aapt package -f -M AndroidManifest.xml -I "$ANDROID_HOME/platforms/android-34/android.jar" -F base.apk
aapt add base.apk lib/arm64-v8a/libstipple_android.so
zipalign -f -p 4 base.apk aligned.apk
apksigner sign --ks your.keystore --out yourapp.apk aligned.apk
```

`lib_name` must match the library's name, without its `lib` prefix and `.so`
suffix. The ABI directory is `arm64-v8a` for `aarch64` and `x86_64` for the
emulator.

Two manifest settings matter:

- **`configChanges`** keeps the activity across rotation, resizing, keyboard and
  dark-mode switches, which Stipple handles itself. Without it, Android
  destroys the activity and starts a new one: `App::run` returns and `main` runs
  again on a new thread, losing the app's state.
- **A `NoActionBar` theme.** The default theme reserves room for a title bar
  that a `NativeActivity` never draws, which would show as a blank band.

## Running on an emulator

[`examples/androiddemo/test-on-device.sh`](../../examples/androiddemo/test-on-device.sh)
installs the APK on whatever device `adb` talks to (`ANDROID_SERIAL` picks
one), checks a frame is presented and the timer ticks, taps the button, reads
the accessibility nodes with `uiautomator dump`, presses Back (handled), sends
the app to the background and back and trims its memory, swipes the list (it
must scroll, without tapping a row), types into the field and runs the input
connection self-test (the field must end up as `Stipple日本!`), presses Back
until the app no longer handles it, and saves screenshots. CI runs it on an API 34 x86_64 emulator
(the `visual-android` job). By hand:

```sh
avdmanager create avd -n stipple -k "system-images;android-34;google_apis;x86_64" -d pixel_6
emulator -avd stipple -no-window -no-audio -no-snapshot &
adb wait-for-device
examples/androiddemo/build-apk.sh x86_64 stipple.apk
examples/androiddemo/test-on-device.sh stipple.apk shots
```

## Known gaps

- **No multi-touch gestures.** Every finger is reported as an `Event::Touch`,
  but nothing recognizes pinches or two-finger pans yet, and scroll containers
  only scroll vertically.
- **A slow app loses quick swipes' moves.** On a busy UI thread (the emulator's
  software rendering takes a few hundred milliseconds a frame), Android can fold
  a quick swipe's moves into its release; the list then scrolls the whole way
  at once, without a fling. Fling speed is measured when events arrive, not
  from their timestamps.
- **Composing keyboards are checked by a self-test, not a person.** CI sees
  Gboard connect, and drives the connection from the app; no one has typed
  Japanese or Chinese on it yet. There's no extracted-text or cursor-anchor
  support (fullscreen landscape editing, the candidate bar's position), and a
  selection move from a selection or mid-composition is refused. Dead keys on
  hardware keyboards don't compose.
- **No predictive back**: the back gesture's preview animation needs the app
  to say beforehand whether it will go back.
- **No state saved across process death** (`onSaveInstanceState`).
- **No OS-drawn controls.** Android's widgets live in its Java framework, so
  `Theme::native()` stays self-drawn there, in the system's colors.
- **TalkBack itself is untried**: the node provider is checked with
  `uiautomator`. No file dialogs.
- Rendering is software only: a full-HD screen is a lot of pixels, so heavy
  animation runs below the display's rate on slower devices and emulators.
