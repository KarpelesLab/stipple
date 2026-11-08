# Android

Apps run in a `NativeActivity`, so no Java code is needed. The app is a
`cdylib` that Android loads from the APK, and the backend talks to the NDK's C
API (`libandroid`) directly, with a few hand-written JNI calls for what the NDK
lacks. There is no dependency on the `ndk` or `android-activity` crates.

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
  loses it (through `InputMethodManager` for the decor view, which Android 13
  and later require; the NDK's `ANativeActivity_showSoftInput` is the fallback).
  It types through key events, so it covers what a hardware keyboard could type.
- **High-DPI.** The scale factor is the screen density over 160 dpi (2.625 on a
  420 dpi phone); a density change resizes the content.
- **System look.** `Theme::native()` follows night mode and, on Android 12 and
  later, the wallpaper's accent color (`system_accent1_*`), and switches live
  when night mode changes. Its surfaces are Material 3's baseline colors.
- **Clipboard.** `ClipboardManager` over JNI, so copy and paste reach other apps.
- **Fullscreen.** `WindowAttributes::fullscreen` and `AppHandle::window`
  hide the status bar.

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
one), checks a frame is presented and the timer ticks, taps the button, swipes the
list (it must scroll, without tapping a row), types into the field, and saves
screenshots. CI runs it on an API 34 x86_64 emulator
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
- **Text the soft keyboard can't send as keys** (emoji, input methods that
  compose, such as Chinese or Japanese) doesn't arrive: a `NativeActivity` has
  no `InputConnection`, and the NDK can't read the text of a multi-character key
  event. Dead keys on hardware keyboards don't compose either.
- **No OS-drawn controls.** Android's widgets live in its Java framework, so
  `Theme::native()` stays self-drawn there, in the system's colors.
- **No accessibility** (TalkBack) yet, and no file dialogs.
- Rendering is software only: a full-HD screen is a lot of pixels, so heavy
  animation runs below the display's rate on slower devices and emulators.
