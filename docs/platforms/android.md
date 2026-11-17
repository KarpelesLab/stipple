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
  app gets the content rectangle the system bars leave free, also on
  Android 15 and later, which lay every
  app targeting them out under the bars: the window's insets
  (`getRootWindowInsets`: `getInsets(systemBars() | displayCutout())` on
  Android 11 and later, the system window and display cutout insets
  before) come off it. The rest of the window, and the status and
  navigation bars, take the app's background color, with dark or light bar
  icons to match.
- **Edge to edge.** With `App::edge_to_edge(true)` the app draws the whole
  window, under the bars (`setDecorFitsSystemWindows(false)` on Android 11
  and later, the `SYSTEM_UI_FLAG_LAYOUT_*` flags before) and into the display
  cutout (`LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS`, `SHORT_EDGES` on Android 9
  and 10), and reads the bars' insets as `cx.safe_area()`. The bars go
  transparent, without Android 10's scrim, their icons dark over light
  content and light over dark (the status bar from the frame's top-left
  pixel, the navigation bar from its bottom-left one). The insets are read
  again whenever the host view is given new ones (`onApplyWindowInsets`)
  and the window's size or content rectangle changes; a change that keeps
  the size (a half turn, the bars showing) is `Event::WindowChanged`. See
  [layout.md](../layout.md#the-safe-area-phones).
- **Orientation and keeping awake.** `App::orientations` and
  `AppHandle::set_orientations` call `Activity.setRequestedOrientation`
  (portrait alone is `SCREEN_ORIENTATION_PORTRAIT`, both landscapes
  `USER_LANDSCAPE`, every way `FULL_USER`, honoring the user's rotation
  lock); keep `orientation|screenSize` in the activity's `configChanges`, so
  a turn resizes the app rather than recreating the activity.
  `AppHandle::set_keep_awake` sets `FLAG_KEEP_SCREEN_ON` on the window
  (`ANativeActivity_setWindowFlags`). See
  [windows-and-cursors.md](../windows-and-cursors.md#phones-orientation-and-keeping-awake).
- **Touch and mouse.** Every finger arrives as an `Event::Touch` (its id,
  phase and position). The first finger down is also the pointer, until it
  lifts; a tap is a press and release, and lifting a finger leaves no hover
  behind. A mouse adds its right and middle buttons, hover and the wheel.
- **Touch gestures.** A finger dragged in a scroll container scrolls it
  (sideways in a horizontal one), and keeps it going (a fling that slows down,
  and stops at the end of the content) if it lifts while moving. A press only
  becomes a scroll once the finger has moved 8 logical pixels, so taps still
  click; once it scrolls, the pressed button lets go and doesn't click. A
  finger on a slider drags the slider, and a finger that lands on a fling
  stops it. A finger held still for half a second on an element with a context
  menu opens the menu (a long press), and two fingers (`POINTER_DOWN`) pinch,
  pan and turn an element with `on_pinch`. This lives in the toolkit, so it
  works the same on iOS and the web; see
  [pointer-events.md](../pointer-events.md#touch-gestures). Desktop mice keep
  the wheel.
- **Drops from other apps.** In split screen, a freeform window or on a
  desktop, a drag from another app reaches the host view's `onDragEvent` and
  the app's drop targets: text as `DropData::Text`, `file:` URIs as paths,
  `content:` URIs (an image from the gallery, a document from Files) copied
  through the `ContentResolver` into the app's temporary directory and
  delivered as `DropData::Files`. See
  [drag-and-drop.md](../drag-and-drop.md#ios-and-android).
- **Dragging out to other apps.** A long press on a
  [`drag_source`](../drag-and-drop.md#dragging-out-of-the-app) whose payload
  is text or files starts a system drag (`View.startDragAndDrop` with
  `DRAG_FLAG_GLOBAL | DRAG_FLAG_GLOBAL_URI_READ`) with a picture of the
  element as its shadow: text as `ClipData` text, files as `content:` URIs of
  the app's own read-only `FileProvider` (`org.stipple.android.Files`), which
  the manifest must declare (see [Packaging](#packaging)).
- **Menus.** Context menus are drawn by Stipple, not a `PopupMenu`, and open
  on a long press (or a mouse's right button); a tap picks an item, a tap
  outside or Back closes the menu. An `App::menu` bar is drawn across the top
  of the window, as on Linux, with its menus opening on a tap; leave
  `App::menu` out on Android if a phone shouldn't show one.
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
- **Predictive back.** With `android:enableOnBackInvokedCallback="true"` in
  the manifest, on Android 13 and later, Back goes to an
  `OnBackInvokedCallback` the backend registers on the activity's
  `OnBackInvokedDispatcher` only while the app would go back
  (`App::back_enabled`, or an open menu); otherwise the system goes back
  itself, with its preview animation. On Android 14 and later the callback is
  an `OnBackAnimationCallback`, and the gesture's progress reaches
  `App::on_back_progress`. See
  [window-lifecycle.md](../window-lifecycle.md#predictive-back-android-13-and-later).
- **Saved state.** `onSaveInstanceState` asks the app for its state
  (`App::save_state`); `NativeActivity` keeps the bytes in the saved `Bundle`
  and hands them to `ANativeActivity_onCreate` when the activity is recreated
  (after Android ended the process in the background), where `android::start`
  takes them for `App::restore_state`. See
  [window-lifecycle.md](../window-lifecycle.md#keeping-the-state-across-process-death).
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
- **Clipboard.** `ClipboardManager` over JNI, so copy and paste reach other
  apps (checked both ways with another app by `test-services.sh`, below).
- **Fullscreen.** `WindowAttributes::fullscreen` and `AppHandle::window`
  hide the status bar.

## Run-time classes

An input method talks to a `View` (`onCreateInputConnection`), and
accessibility services to a view's `AccessibilityNodeProvider`; a
`NativeActivity` has neither. Rather than ship Java, the backend assembles
a few tiny classes into a dex file at start-up (`stipple::platform::dex`, a
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
- `org.stipple.android.Results`, a headless `android.app.Fragment`, added to
  the activity the first time it's needed: activities started for a result
  (the document picker) and permission requests are started from it, so
  their answers reach its `onActivityResult` / `onRequestPermissionsResult`,
  which `NativeActivity` doesn't pass on.
- `org.stipple.android.Lifecycle`, the application's
  `ActivityLifecycleCallbacks`: its `onActivitySaveInstanceState` drops the
  fragment from the activity's saved state (`android:fragments`), since after
  process death Android would restore it through the app's class loader,
  which doesn't have the class, and crash.

On Android 13 and later one more, from a dex of its own (its interface doesn't
exist before): `org.stipple.android.Back`, an `OnBackInvokedCallback` (API
33), or `org.stipple.android.BackAnimation`, an `OnBackAnimationCallback`
(API 34 and later), for predictive back. `adb logcat -s stipple` shows `back
callback registered` / `unregistered` as the app's answer changes.

And `org.stipple.android.Invoke`, a `java.lang.reflect.InvocationHandler`
whose `invoke` is native: behind `java.lang.reflect.Proxy` objects it
implements the Play Billing Library's listener interfaces, which only the
app's class loader has (see [In-app purchases](#in-app-purchases)).

Every method is `native`, bound to Rust with `RegisterNatives`; the only
bytecode is each constructor's call to its superclass's. It runs on the main
(Java) thread, and talks to the UI thread through the backend's command
queue. If loading fails (logged as `stipple: no host view`), the app runs as
before: keys only, no accessibility. `adb logcat -s stipple` shows
`stipple: host view installed` when it works.

## The app's own Java classes

Android instantiates some classes by the name the manifest gives: a
`Service` (a `VpnService`, a foreground service), a `TileService`, a
`BroadcastReceiver`, an app widget provider. Those can't come from a dex
written at run time, so an app ships them in the APK: `cargo-stipple`
compiles its Java sources (`android.java`) or adds prebuilt dex files
(`android.dex`), and merges a manifest fragment (`android.manifest-fragment`)
declaring them into the manifest it writes; see
[packaging.md](../packaging.md#the-apps-own-java-classes-and-components).
[`examples/androidservice`](../../examples/androidservice) has a `Service`
and a `BroadcastReceiver` calling into Rust, and
`examples/androidservice/test-on-device.sh` checks them on an emulator
(passed on a local API 34 one and in CI trial runs on API 34; CI runs it
there without blocking).

**Reaching the Rust code.** The classes call into the app's library, the
same `.so` the `NativeActivity` runs: a static initializer loads it with
`System.loadLibrary("<lib-name>")` (the `[lib]` name, without `lib` and
`.so`), and `native` methods are bound either way JNI allows:

- **By name**: `static native String ping(String text)` in
  `com.example.app.PingReceiver` is the library's exported
  `Java_com_example_app_PingReceiver_ping(env, class, text)` (an
  `#[unsafe(no_mangle)] pub unsafe extern "C" fn`; `_` in names is `_1`,
  see the JNI specification).
- **With `RegisterNatives`** from the library's `JNI_OnLoad`, which the VM
  calls when a Java class loads the library with `System.loadLibrary`, once
  per class loader; `FindClass` there uses the class loader of the class
  that loaded it, so it finds the app's classes. `JNI_OnLoad` must return
  the JNI version it needs (`JNI_VERSION_1_6`).

**Stipple doesn't define `JNI_OnLoad`**, so the app's own is the only one in
the library and the linker has nothing to clash with. Stipple's own
classes find their natives by name (`Java_org_stipple_android_*`), or bind
them with `RegisterNatives` when the backend loads its run-time dex; it keeps
the `JavaVM` from its own entry points (see
[Calling Android APIs yourself](#calling-android-apis-yourself)). Note that
`JNI_OnLoad` doesn't run when the `NativeActivity` loads the library (it does
that with `dlopen`): only when a Java class calls `System.loadLibrary`, which
Stipple's own `Alarm` receiver, `Files` provider and `Link` activity also do.
Code that must run once whatever starts the process belongs in a
`std::sync::Once` called from each entry point.

**There may be no activity.** Android starts the app's process for whichever
component it needs: a service started at boot, a broadcast, a tile, a
provider queried by another app. Then `ANativeActivity_onCreate` hasn't
run, and perhaps never will in that process:
`android::start` hasn't been called, there is no `App`, window or UI thread,
and `println!` doesn't reach logcat yet (Stipple sends stdout there when its
activity starts; log through liblog's `__android_log_write` instead, as the
example does). The app's `JNI_OnLoad` and native methods must work without
any of it, and keep what they share with the UI (the VPN's state, say) in
their own statics or files. `platform::android::java_vm()` and
`application_context()` answer there from Android 12; before, `java_vm()` is
null until one of Stipple's own entry points has run, so keep the `JavaVM`
that `JNI_OnLoad` is handed (or call `GetJavaVM` on a native method's
`JNIEnv`). If the activity starts later, it runs in the same
process. Native methods run on the thread Java calls them on (a component's
callbacks are on the process's main thread, the activity's too); the
`JNIEnv` they get is only good on that thread and for that call.

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
- **Dead keys.** A hardware keyboard's dead key (a
  `KeyCharacterMap` value with `COMBINING_ACCENT`, as Alt+E is in the
  Generic layout) shows its accent as the preedit, and the next key commits
  what `KeyCharacterMap.getDeadChar` composes (`é`), as Android's own text
  fields do; see
  [ime.md](../ime.md#dead-keys-on-mobile-hardware-keyboards).
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
- `EditorInfo.inputType` and `imeOptions` come from the focused field's
  [keyboard hints](../text-editing.md#keyboard-hints) (`InputKind::Number`
  is `TYPE_CLASS_NUMBER`, `EnterAction::Go` is `IME_ACTION_GO`, ...); a
  field with other hints restarts the input method.
- The keyboard's action key presses Enter (Done, Go, Search, Send, and Next
  on a field that asked for Next, which then moves focus on), Tab (Next on
  one that didn't) or Shift+Tab (Previous); its Select all / Cut / Copy /
  Paste are the usual shortcuts. Keys it sends as key events (Backspace, Enter on most
  keyboards) arrive as keys; a key typed while it composes commits the
  composition first.
- The field's focus restarts the input method (`restartInput`); caret and
  text changes it didn't make are reported with `updateSelection`.
- A password field gets a password `inputType` (a number password with
  `InputKind::Number`) and `IME_FLAG_NO_PERSONALIZED_LEARNING`, so the
  keyboard shows no suggestions and learns nothing; its text isn't shared.
- **The soft keyboard's height** (`getInsets(ime())`, the system window
  insets before Android 11) is `cx.keyboard_inset()`, edge to edge or not:
  the App lays the view out above it and scrolls the focused field into view
  (see [text-editing.md](../text-editing.md#the-on-screen-keyboard)). The
  content rectangle itself doesn't shrink for it. Logged as
  `Stipple Android: keyboard inset N`.
- **The edit menu**, over a field's selection after a long press or a double
  tap (see [text-editing.md](../text-editing.md#editing-with-a-finger)), is
  the system's floating toolbar on Android 8 and later: a floating
  `ActionMode` (`startActionMode(callback, TYPE_FLOATING)`) on the host
  view, whose `ActionMode.Callback2` is a class of its own assembled at run
  time and loaded on its own (like the back callback): `onPrepareActionMode`
  adds the items the App's menu offers, titled with `android.R.string.cut`,
  `copy`, `paste` and `selectAll`; `onGetContentRect` points it at the
  selection; `onActionItemClicked` hands the pick to the App
  (`Event::EditAction`). Logged as `stipple: edit menu shown: ...`,
  `picked ...` and `closed`. Before Android 8 the App draws its own menu.
  CI's emulator script long-presses the name field and taps the menu's Copy
  (added, not yet run in CI: a warning if it fails).

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
scroll forward and backward on any scroll container whose content overflows
(a page, as a wheel over it, sideways for a horizontal strip; each offered
only while it can go that way), set text (select all, then type). Changes to
the tree send `TYPE_WINDOW_CONTENT_CHANGED`, a moved focus
`TYPE_VIEW_FOCUSED`, and a container that scrolled `TYPE_VIEW_SCROLLED`. A hovering finger (TalkBack's explore by
touch) goes to the host view, which announces the node under it. The host
view's own node (the window) isn't focusable to accessibility, though the
view takes input focus: TalkBack folds the text of a focusable node's
children that can't act into that node, so its labels would get no stops.

```sh
adb shell uiautomator dump /sdcard/ui.xml && adb shell cat /sdcard/ui.xml
```

CI also drives the app through an accessibility service standing in for
TalkBack (`.github/a11y/android`); see
[accessibility.md](../accessibility.md#android).

lists the nodes; it is what CI checks.

## File pickers

`dialog::open_file_contents` and `dialog::save_file_contents` (or
`save_file_contents_then`, to hear whether the bytes were written) use the
Storage Access Framework, as Android apps are meant to: there are no paths,
only documents an app may read or write.

- **Open** starts `ACTION_OPEN_DOCUMENT` (`CATEGORY_OPENABLE`), filtered by
  `FileDialog::accept` turned into MIME types (`".txt,image/*"` offers
  `text/plain` and `image/*`; an extension Stipple doesn't know stands for
  `application/octet-stream`). The picked document's `content:` URI is opened
  with `ContentResolver.openFileDescriptor` on a thread of its own (a cloud
  provider may download it first), read, and passed to the callback with its
  display name.
- **Save** starts `ACTION_CREATE_DOCUMENT` with `FileDialog::file_name` as
  the suggested name (`Untitled` if empty) and its extension's MIME type; the
  bytes are written to the created document.

Both answer later, from the event loop, as on the web: the call returns at
once, and the callback runs once the picker closes (with `None` / `false` if
it was cancelled). The picker is started from the `Results` fragment (see
[run-time classes](#run-time-classes)). If the process is killed while the
picker is up, the activity comes back without the request; the callback is
gone with the old process. `open_file`, `save_file` and `pick_folder` return
`None`.

CI drives DocumentsUI through `uiautomator` on its emulator: it picks a file
pushed to Downloads and checks the app read it, backs out of another (the
callback hears `None`), saves one and reads it back with `adb shell cat`, and
kills the process under the picker to check the activity comes back.

## Notifications

`platform::notify` posts through `NotificationManager`, on a channel of the
app's (`stipple`, made on Android 8 and later). On Android 13 and later it
needs the `POST_NOTIFICATIONS` runtime permission, which the manifest must
declare:

```xml
<uses-permission android:name="android.permission.POST_NOTIFICATIONS"/>
```

The first `notify` without it shows the system's prompt; the notification
waits for the answer, and later calls fail once it's refused. See
[os-integration.md](../os-integration.md#notifications). CI refuses the prompt
on its emulator, checks the next `notify` fails, grants the permission with
`adb shell pm grant` and finds the notification in `dumpsys notification`.

### Scheduled notifications

`platform::schedule_notification` sets an `AlarmManager` alarm (exact where
the app may: Android 6 to 11, or with `SCHEDULE_EXACT_ALARM` granted on 12, or
`USE_EXACT_ALARM` on 13 and later; otherwise inexact, which the system may
delay by some minutes). When it goes off, a broadcast receiver posts the
notification. Which receiver depends on the APK:

- **With nothing added**, the backend registers a receiver at run time (a
  class of its run-time dex). It hears the alarm while the app's process
  lives, including in the background, but not once the system ended it.
- **With the `Alarm` receiver**, the notification shows whatever became of the
  process: the system starts it for the receiver, whose class (in the APK's
  `classes.dex`) loads the app's library and calls into Stipple. It also sets
  the alarms again after a reboot, which loses them. Write the dex with
  `stipple::platform::android_receiver_dex(lib_name)` (from a build script
  or any host program; `build-apk.sh` writes it through a test), add it to the
  APK as `classes.dex` (`aapt add base.apk classes.dex`), and declare it:

```xml
<uses-permission android:name="android.permission.RECEIVE_BOOT_COMPLETED"/>
<!-- optional: exact alarms -->
<uses-permission android:name="android.permission.SCHEDULE_EXACT_ALARM" android:maxSdkVersion="32"/>
<uses-permission android:name="android.permission.USE_EXACT_ALARM"/>
<application ... android:hasCode="true">
  ...
  <receiver android:name="org.stipple.android.Alarm" android:exported="false">
    <intent-filter>
      <action android:name="android.intent.action.BOOT_COMPLETED"/>
      <action android:name="android.intent.action.MY_PACKAGE_REPLACED"/>
    </intent-filter>
  </receiver>
</application>
```

(`USE_EXACT_ALARM` is meant for alarm and calendar apps; Google Play reviews
it. Leave both out for inexact alarms.)

The backend keeps what's scheduled in `stipple-notifications.txt` in the app's
files directory, since `AlarmManager` can't list its alarms: for
`pending_notifications`, and to set the alarms again when the app starts.
Force-stopping the app (Settings, `am force-stop`) cancels its alarms, as
Android does for every app.

Tapping a scheduled notification is reported with its id
(`App::on_notification_tapped`). How it gets there depends on the manifest
too:

- **With nothing added**, the tap opens the app's launch intent with the id in
  it, which Stipple reads when the tap *starts* the activity. While the
  activity is alive, the tap only brings it to the front: `NativeActivity`
  doesn't pass `onNewIntent` on, so which notification it was isn't known.
- **With the `Tapped` activity** (a class of the same `classes.dex`), every
  tap is reported, the activity alive or not: the tap opens `Tapped`, which has
  no window, reports the id, brings the app's activity forward with its launch
  intent (or starts it, the event loop finding the tap queued) and finishes at
  once. The app's own activity stays a plain `NativeActivity`; declare `Tapped`
  next to the receiver:

```xml
<activity android:name="org.stipple.android.Tapped" android:exported="false"
    android:taskAffinity="" android:excludeFromRecents="true" android:noHistory="true"
    android:theme="@android:style/Theme.NoDisplay"/>
```

(`taskAffinity=""` gives it a task of its own, out of Recents, so it never
becomes the root of the app's task.)

Each scheduled notification is in an app group of its own, so Android doesn't
bundle it with the app's other notifications under a summary of the system's,
whose tap would open the app without saying which notification it was.

## URLs that open the app

`App::on_open_url` hears the app's custom schemes and verified App Links
(`url-schemes` and `associated-domains` in cargo-stipple's manifest). They
arrive as `ACTION_VIEW` intents, which a `NativeActivity` only sees when one
starts it (no `onNewIntent`), so the intent filters go on
`org.stipple.android.Link`, a windowless activity in the same `classes.dex`
as `Alarm` and `Tapped`: it hands its intent's URL to the app, whatever
runs, brings the app's activity forward (or starts it: the event loop finds
the URL queued, and delivers it after the restored state) and finishes. The
app's own activity stays a plain `NativeActivity` with its standard launch
mode, so nothing else changes. `adb logcat -s stipple` shows `stipple: opened
with "..."`. The manifest entry and the server's `assetlinks.json` are in
[os-integration.md](../os-integration.md#urls-that-open-the-app). Check one
with:

```sh
adb shell am start -a android.intent.action.VIEW -d 'myapp://callback?code=1'
adb shell pm get-app-links com.example.app      # App Links: verified or not
```

`examples/androiddemo/test-open-url.sh` does it cold, warm and from the
background (its manifest claims `stipple-demo`).

## Permissions and the camera

`platform::permission` maps each permission to the manifest's
(`CAMERA`, `RECORD_AUDIO`, `POST_NOTIFICATIONS`, `READ_MEDIA_IMAGES` or
`READ_EXTERNAL_STORAGE`, `ACCESS_FINE_LOCATION` / `ACCESS_COARSE_LOCATION`),
checks it with `checkSelfPermission` and asks with `requestPermissions` from
the same headless fragment the pickers use, whose
`onRequestPermissionsResult` brings the answer back (a `NativeActivity` has
none). Declare each one asked for:

```xml
<uses-permission android:name="android.permission.CAMERA"/>
```

`stipple::camera` uses Camera2 through the NDK (`libcamera2ndk.so`, with
`libmediandk.so`'s `AImageReader` for the frames, both loaded at run time):
no Java, Android 7 (API 24) and later. `examples/cameraview` builds as an APK
with `STIPPLE_APK_EXAMPLE=cameraview_android
STIPPLE_APK_MANIFEST=examples/cameraview/AndroidManifest.xml
examples/androiddemo/build-apk.sh`. See [camera.md](../camera.md).

## Share sheet and haptics

`platform::share` sends an `ACTION_SEND` (`ACTION_SEND_MULTIPLE` for several
files) through `Intent.createChooser`, started from the results fragment; the
chooser's `IntentSender` tells the run-time receiver which target was picked
(`ShareOutcome::Shared`), and its closing without one is `Cancelled`. Files
need `content:` URIs another app may read: without a `FileProvider` (a Java
class), Stipple puts them in `MediaStore`'s Downloads (Android 10 and later,
no permission needed), where they stay; before Android 10 only text and URLs
are shared. `platform::haptic` calls `View.performHapticFeedback` on the host
view, which needs no permission and follows the user's touch-feedback
setting.

CI's emulator opens the chooser with a file and backs out of it, plays a
haptic, then schedules a notification, sends the app to the background and
ends its process: the `Alarm` receiver must post it (`dumpsys notification`),
and tapping it must start the app with its id. Then it schedules another and
leaves the app in front: tapping that one, with the activity alive, must
report its id too (through `Tapped`).

## In-app purchases

`platform::store` drives the Google Play Billing Library over JNI: its
classes are looked up in the activity's class loader, its builders called by
name, and its listeners are proxies over the `Invoke` class above, so Stipple
adds no Java, and any library version from 6 to 9 works. The library itself
must be in the APK: a Gradle build adds the dependency (with a `-keep` rule,
as nothing in Java references it); `build-apk.sh` adds it with
`STIPPLE_BILLING=1`, through `tools/android/billing.py`, which dexes the
library and its dependencies into `classes2.dex` and adds what the library
puts in the manifest. Without it, the store answers `Unavailable`. CI checks
that answer in `test-on-device.sh`, then runs the purchase path against a
stand-in for the library (`.github/billing/android/test-billing.sh`). See
[purchases.md](../purchases.md) for the Play Console side and testing.

## Calling Android APIs yourself

For the Android APIs Stipple doesn't wrap, `stipple::platform::android`
(Android only) hands out what JNI code needs. Stipple has no JNI crate in its
API: the references are raw `jni.h` pointers, to wrap with your own bindings
(the `jni` crate's `JavaVM::from_raw` and `JObject::from_raw`, or calls by
hand as in [`examples/intentresult`](../../examples/intentresult)). Each
function is callable from any thread.

| | What it gives | Who deletes it |
|---|---|---|
| `java_vm()` | The process's `JavaVM*` (`*mut JavaVM`). Attach your threads to it as you need. | Nobody: it lives as long as the process. |
| `activity()` | `Option<jobject>`: the current activity, the `NativeActivity`. It changes when Android recreates the activity, and is `None` while there is none (before `onCreate`, after `onDestroy`, in a process started for a receiver or a service). | **The caller**: it's a new global reference each time (`DeleteGlobalRef`). |
| `application_context()` | `Option<jobject>`: the application `Context`, for code that runs without an activity. | Nobody: it's Stipple's global reference, kept for the life of the process. |
| `start_activity_for_result(intent, callback)` | Starts `intent` and calls `callback(result_code, data)` with what `onActivityResult` gets. | The caller keeps its `intent`; `data` is a local reference valid during the callback. |
| `run_on_ui_thread(f)` | Runs `f` on Android's main thread. | — |

`RESULT_OK` (-1) and `RESULT_CANCELED` (0) are `Activity`'s result codes.

**Starting an activity for a result.** Many APIs hand back an `Intent` to be
started for a result: `VpnService.prepare(context)` (the VPN consent dialog;
`null` once the user agreed), `Settings.ACTION_*` screens,
`KeyguardManager.createConfirmDeviceCredentialIntent`,
`MediaProjectionManager.createScreenCaptureIntent`, sign-in flows. A
`NativeActivity` doesn't pass `onActivityResult` on, so
`start_activity_for_result` starts the intent from Stipple's `Results` fragment
(see [run-time classes](#run-time-classes)) with a request code of its own
(bit 30 set, apart from Stipple's own requests), and keeps the callback under
that code. It's `unsafe`: `intent` must be a valid local reference of the
calling thread, or a global reference.

```rust
use stipple::platform::android;

// From any thread, with an `intent` made with your JNI bindings.
unsafe {
    android::start_activity_for_result(intent, |code, data| {
        // On Android's main thread. `data` (the result Intent, if any) is a
        // local reference valid only here.
        if code == android::RESULT_OK {
            // e.g. start the VpnService.
        }
    });
}
```

The callback runs once, on the main thread, attached to the VM. It survives
the activity being recreated meanwhile (a configuration change the manifest
doesn't declare in `android:configChanges`, "Don't keep activities"): the new
activity adds its own `Results` fragment at once in `onCreate` while a request
waits, before the framework delivers results (`onResume`), and the answer
reaches the callback through it. It doesn't survive the process ending
(Android may kill a backgrounded app while the other activity is up): the
callback goes with the old process. When the intent can't be started (no
activity, `ActivityNotFoundException`, a null intent) the callback gets
`RESULT_CANCELED` and `None`. A panic in it is caught and logged.

**The main thread.** `NativeActivity` calls the native callbacks on the Java
main thread, but the `App` runs on Stipple's UI thread, a thread of its own
(the one `main` runs on). Java APIs that must be called on the main thread
(`View`s, `Toast`, `BiometricPrompt`, `new Handler()`) go through
`run_on_ui_thread(f)`, which, as `Activity.runOnUiThread`, runs `f` at once
when called there and otherwise queues it for the main thread's looper (a pipe
it watches). It works without an activity once Stipple has run on the main
thread in the process (the activity's `onCreate`, or the `Alarm` receiver, the
`Tapped` activity or the `Files` provider being created), and returns `false`
before that. To get back to the `App` from there, use an `AppHandle`
(`handle.update(..)`), which is `Send`.

**Without an activity.** A process Android starts for a receiver or a service
(an always-on VPN's `VpnService`, after a reboot) has no activity:
`activity()` is `None`, but `java_vm()` and `application_context()` still
answer. The VM is kept from the first of Stipple's entry points that runs in
the process, and from Android 12 asked of `libnativehelper`
(`JNI_GetCreatedJavaVMs`) otherwise; the context is then
`ActivityThread.currentApplication()`.

`examples/intentresult` asks for VPN consent this way, and
`examples/intentresult/test-on-device.sh` drives it on an emulator: an intent
nothing handles is answered `RESULT_CANCELED`; the consent dialog backed out
of is 0; the dialog agreed to after the font scale changed under it (which
recreates the activity, as its manifest leaves `fontScale` out of
`configChanges`) is -1, heard by the callback through the new activity; and
`prepare` returns `null` afterwards. CI runs it on the API 34, 33 and 30
emulators, not blocking yet.

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

For Google Play, [`cargo-stipple android`](../packaging.md) builds every
ABI and writes a signed Android App Bundle (`.aab`) and a universal APK from
a manifest in the app's `Cargo.toml`: launcher icons from one image, the
label in each language, target SDK 35, 16 KB pages, and the upload key from
environment variables. See [packaging.md](../packaging.md). What follows is
what it does underneath, by hand, for one ABI.

The APK needs no Gradle. [`examples/androiddemo/build-apk.sh`](../../examples/androiddemo/build-apk.sh)
does all of this for the demo (`build-apk.sh x86_64 stipple.apk`, or
`arm64-v8a` for a phone). By hand, with the SDK's build tools:

```xml
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="com.example.yourapp">
  <uses-sdk android:minSdkVersion="24" android:targetSdkVersion="34"/>
  <application android:label="Your App" android:hasCode="false"
      android:enableOnBackInvokedCallback="true"
      android:theme="@android:style/Theme.DeviceDefault.Light.NoActionBar">
    <activity android:name="android.app.NativeActivity" android:exported="true"
        android:configChanges="orientation|screenSize|screenLayout|smallestScreenSize|keyboard|keyboardHidden|navigation|uiMode|density|locale|layoutDirection">
      <meta-data android:name="android.app.lib_name" android:value="stipple_android"/>
      <intent-filter>
        <action android:name="android.intent.action.MAIN"/>
        <category android:name="android.intent.category.LAUNCHER"/>
      </intent-filter>
    </activity>
  </application>
</manifest>
```

For files dragged out of the app, the manifest also declares the provider
(and `android:hasCode="true"`, with the `classes.dex` below, which holds its
class):

```xml
    <provider android:name="org.stipple.android.Files"
        android:authorities="com.example.yourapp.stipple.files"
        android:exported="false" android:grantUriPermissions="true"/>
```

The authority is the package name and `.stipple.files`. The provider only
reads: a URI names the file's absolute path, and other apps can open only
the URIs a drag granted them. Without it, a long press on a drag source of
files does what it would without one (a context menu), and the log says why.
`cargo-stipple` writes it with `drag-files = true` under
`[package.metadata.stipple.android]`.

```sh
mkdir -p lib/arm64-v8a
cp target/aarch64-linux-android/release/libstipple_android.so lib/arm64-v8a/
aapt package -f -M AndroidManifest.xml -I "$ANDROID_HOME/platforms/android-34/android.jar" -F base.apk
aapt add base.apk lib/arm64-v8a/libstipple_android.so
# optional, for scheduled notifications and their taps (see above) and for
# files dragged out of the app: classes.dex from
# stipple::platform::android_receiver_dex("stipple_android")
aapt add base.apk classes.dex
zipalign -f -p 4 base.apk aligned.apk
apksigner sign --ks your.keystore --out yourapp.apk aligned.apk
```

`lib_name` must match the library's name, without its `lib` prefix and `.so`
suffix. The ABI directory is `arm64-v8a` for `aarch64` and `x86_64` for the
emulator.

The app's packaged files ([`platform::assets`](../os-integration.md#packaged-assets))
go in the APK's `assets/`: add `-A assets` to `aapt package`, and `-0 png
-0 ogg …` to store already-compressed formats as they are (reading a
compressed entry inflates it first). `build-apk.sh` packs
`examples/androiddemo/assets/` (or `$STIPPLE_APK_ASSETS`). Stipple reads them
through the activity's `AAssetManager` (`AAssetManager_open`, `AAsset_read`,
`AAssetDir` for listings), with no JNI; a read before the activity exists
fails with `AssetError::Unavailable`. Play's base module is limited in size,
and Play Asset Delivery (asset packs downloaded after install) isn't
supported yet.

Three manifest settings matter:

- **`configChanges`** keeps the activity across rotation, resizing, keyboard,
  dark-mode and language switches, which Stipple handles itself (a new
  language arrives as `Event::LocaleChanged`; see
  [localization.md](../localization.md)). Without it, Android
  destroys the activity and starts a new one: `App::run` returns and `main` runs
  again on a new thread, losing the app's state.
- **A `NoActionBar` theme.** The default theme reserves room for a title bar
  that a `NativeActivity` never draws, which would show as a blank band.
- **`enableOnBackInvokedCallback`** opts into predictive back on Android 13
  and later (see above). Without it, Back stays a key event and every Back
  reaches the app.

## Running on an emulator

[`examples/androiddemo/test-on-device.sh`](../../examples/androiddemo/test-on-device.sh)
installs the APK on whatever device `adb` talks to (`ANDROID_SERIAL` picks
one), checks a frame is presented and the timer ticks, taps the button,
pinches it with two fingers (it must hear the pinch's start, scale and end,
and not tap), reads the accessibility nodes with `uiautomator dump`, presses
Back (handled), swipes from the left edge (it goes back) and takes another
swipe back to the edge (it doesn't), sends the app to the background and back
and trims its memory, swipes the list (it must scroll, without tapping a
row), long-presses a row (`input swipe` that stays put for 800 ms: its
context menu must open without tapping the row, and a tap on the item found
in `uiautomator dump` picks it), swipes the strip of chips sideways (it must
scroll), types into the field and runs the input connection self-test (the
field must end up as `Stipple日本!`), presses Back until the app no longer
handles it, and saves screenshots. On Android 13 and later it also checks
predictive back (Back and the edge swipe through the callback while the app
takes it, with the swipe's progress and a cancelled one on Android 14, the
system going back by itself when the app declines); before, the declined Back
reaches the app and `onBackPressed` sends the activity away. Then process
death: two taps, Home (the state is saved), `am kill`, relaunch, and the
count must come back.

- **The pinch.** `adb shell input` has one finger, so the script makes a
  virtual touch screen with the shell's `uinput` command (it reads the
  device and its events as JSON on stdin: a direct touch screen with
  multi-touch slots the size of the display) and puts two fingers on the
  button, spreading them from 60 to 180 px apart, then closing them again.
  Its events go through the kernel and the system's input pipeline as a real
  screen's do. Android 13 and 14 have `uinput`; Android 11 doesn't, and the
  pinch is skipped there.
- **The edge swipe needs a navigation bar.** SystemUI's back gesture runs in
  the navigation bar's edge handler, and a device with hardware navigation
  keys has no bar. An AVD made without a device profile (as CI's emulator
  action makes them, 320x640) says `hw.mainKeys=yes` in its `config.ini`, so
  the swipe never goes back there; set `hw.mainKeys=no` (or use a device
  profile such as `pixel_6`). The script turns gesture navigation on if it's
  off (`cmd overlay enable ...navbar.gestural`) and skips the swipe, with the
  reason, when SystemUI's edge handler is still off;
  `STIPPLE_ANDROID_STRICT=1` fails instead.
- **`STIPPLE_ANDROID_QUICK=1`** runs only what differs between releases:
  the tap and the pinch, the accessibility nodes, Back (handled, the edge
  swipe, a cancelled one, declined), and the lifecycle.

`examples/androiddemo/test-input.sh` then checks mobile input depth on the
same device: a finger long-presses the greeting (a drag source of a text),
which lifts it out of the app (`View.startDragAndDrop`), carries it over the
button and lets go: the drop must reach the app through `onDragEvent`. The
same with the heading, a drag source of a file, which leaves as a
`content:` URI of the app's `FileProvider` and must come back as a copy
holding its bytes; then a file another app's way, a `MediaStore` `content:`
URI (`android::drag_file_self_test`, Android 10 and later). Then a `uinput`
keyboard types Alt+E, E, Alt+U, U into the field, which must end in `éü`.
The drops passed on local API 34 and 30 emulators and in CI on API 34, 33
and 30, and the dead keys on API 34 (Android 11 has no `uinput`, so they are skipped there); CI runs it
without blocking.

`examples/androiddemo/test-drag-apps.sh clipprobe.apk` drags between two
apps: androiddemo and the probe's drop target (`org.stipple.clip.Drop`, an
ordinary Java view with an `OnDragListener`) in freeform windows one above
the other (`settings put global enable_freeform_support 1`, `am start
--windowingMode 5`, `am task resize`; it says so and stops where the windows
don't come up in freeform mode). On API 33 it uses split screen instead:
there `am task resize` moves a freeform task's bounds in the window manager
but the shell leaves its surface where it opened, so the windows overlap on
screen and the drop lands back in androiddemo. Split screen is set up with
the shell's own command (`dumpsys activity service SystemUIService WMShell
splitscreen moveToSideStage <task> 0`, androiddemo in the top stage, the
probe below), on a display made twice as tall for the test (`wm size`,
reset afterwards) so androiddemo's half shows all of it; a split left over
from an earlier run is ended first (`removeFromSideStage`), and the split
must show just the two apps (the shell sometimes pairs androiddemo with an
app it showed before, such as Chrome; then it is set up again, up to three
times). `STIPPLE_DRAG_LAYOUT=freeform`
or `split` picks one on any release.
A long press lifts androiddemo's greeting into the probe, which must log the
text; then its heading, whose file the probe must read through its own
`ContentResolver` (display name, type and bytes) with the drag's URI
permission. It passed on local API 34, 33 and 30 emulators (several runs
each) and in CI on API 34, 33 and 30 (three times in a row on each in a
trial run). CI runs it without blocking.

`examples/androiddemo/test-services.sh clipprobe.apk` then drives the
system's services with `adb shell input` touches: the one-second
`AppHandle::every` timer must change the "Running for N s" label between
two `uiautomator` dumps; swipes bring the strip's last chips into view, and
a tap on *Set clip* writes the clipboard, which another app reads back (the
clipboard probe, `.github/clip/android`, built by its `build-clip.sh`: an
activity brought to the front, as Android 10 and later only let the focused
app read); the probe's receiver copies a text (`am broadcast`, as another
app copying) and a tap on *Get clip* reads it in the app; a tap on *Open
link* (`AppHandle::open_url`) must start an `ACTION_VIEW` intent for the URL
from the app's uid, resolved to another app (the activity manager's `START
u0 {act=android.intent.action.VIEW dat=https://example.com/... cmp=...} from
uid N` in logcat), and that app must come to the front. The check doesn't
depend on what the browser shows: Chrome's first run (on the `google_apis`
images its welcome screen replaced the page, and the VIEW intent with it
in `dumpsys activity`, which the check used to read) is also switched off
for the part (`/data/local/tmp/chrome-command-line` with `--disable-fre
--no-first-run`, Chrome made the debug app with `am set-debug-app
--persistent`; both undone afterwards), so the page loads. It passed on
local API 34, 33 and 30 emulators (several runs each) and in a CI trial
run on each (three times in a row); CI runs it after
`test-input.sh` on each emulator, without blocking until it has passed
there for longer.

CI runs it on x86_64 emulators (the `visual-android` job): everything on
API 34, and the quick checks on API 33 (`OnBackInvokedCallback` without
progress) and API 30 (the Back key and `onBackPressed`), each with
`hw.mainKeys=no`. By hand:

```sh
avdmanager create avd -n stipple -k "system-images;android-34;google_apis;x86_64" -d pixel_6
emulator -avd stipple -no-window -no-audio -no-snapshot &
adb wait-for-device
examples/androiddemo/build-apk.sh x86_64 stipple.apk
examples/androiddemo/test-on-device.sh stipple.apk shots
```

## Known gaps

- **A slow app loses quick swipes' moves.** On a busy UI thread (the emulator's
  software rendering takes a few hundred milliseconds a frame), Android can fold
  a quick swipe's moves into its release; the list then scrolls the whole way
  at once, without a fling. Fling speed is measured when events arrive, not
  from their timestamps.
- **Composing keyboards are checked by a self-test, not a person.** CI sees
  Gboard connect, and drives the connection from the app; no one has typed
  Japanese or Chinese on it yet. There's no extracted-text or cursor-anchor
  support (fullscreen landscape editing, the candidate bar's position), and a
  selection move from a selection or mid-composition is refused. Hardware
  keyboards' dead keys compose (see [ime.md](../ime.md#dead-keys-on-mobile-hardware-keyboards)), checked with a
  `uinput` keyboard, not a person's.
- **Drags between apps run against a probe app.** The drag into another
  app (text, and a file through the `FileProvider`) is tested with a small
  Java drop target in freeform windows; drops into Stipple come from its own
  drag sources and a self-test drag, not another app's. The drag shadow is
  the element's picture with the finger at its center, not where it held it.
  A file's type comes from its extension (a small table; unknown ones are
  `application/octet-stream`).
- **Predictive back on Android 13 itself is untried** (CI runs API 34); its
  callback has no progress there. While the app takes Back, the system shows
  no preview: drawing one (from `App::on_back_progress`) is the app's job.
- **No OS-drawn controls.** Android's widgets live in its Java framework, so
  `Theme::native()` stays self-drawn there, in the system's colors.
- **TalkBack itself hasn't been seen working in CI yet**: the node provider
  is checked with `uiautomator` and an accessibility service, and CI turns
  TalkBack on, on the API 34 emulator, and moves its focus and clicks with
  it (see [accessibility.md](../accessibility.md#android); this passes on a
  local emulator). The document picker answers with contents only:
  `open_file` and friends (paths) return `None`, and there is no folder picker
  (`ACTION_OPEN_DOCUMENT_TREE`).
- Rendering is software only: a full-HD screen is a lot of pixels, so heavy
  animation runs below the display's rate on slower devices and emulators.
