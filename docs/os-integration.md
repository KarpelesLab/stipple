# Tray icons, notifications, sharing, haptics, message boxes, URLs and app directories

Ways an app reaches past its window: a tray icon with a menu, notifications
(now or scheduled), the system's share sheet, haptic feedback, message boxes,
handing a URL or file to the app the user picked for it, hearing the URLs that
open the app, finding where to keep
its files and what version it is, and reading the files it was packaged with.
The user's languages and formatting settings are in
[localization.md](localization.md). The `traydemo` example shows most of them
(`androiddemo` and `touchdemo` the mobile side):

```sh
cargo run --example traydemo
```

## Opening URLs and files

```rust
stipple::platform::open_url("https://example.com/")?;
stipple::platform::open_url("mailto:hello@example.com")?;
stipple::platform::open_path("report.pdf")?; // relative to the working directory
```

`open_url` opens a URL with the user's default handler for its scheme: the
browser for `https:`, the mail client for `mailto:`, the file's app for
`file:`. `open_path` does the same for a file or folder, through its `file:`
URL. `AppHandle::open_url` is the same call, for code that has a handle at hand.

The string must start with a scheme (`https:`, `file:`, `x-myapp:`…), or it's
refused: a string from outside can't pass itself off as an option to the program
that opens it.

| | How | `Ok` means |
|---|---|---|
| Linux | the XDG desktop portal's `OpenURI` over D-Bus, else `xdg-open` (then `gio open`); `file:` URLs go straight to `xdg-open` | handed over (it runs on a thread; failures go to stderr) |
| Windows | `ShellExecuteW`; a `file:` URL is passed as a path | the shell accepted it |
| macOS | `NSWorkspace openURL:` | an app accepted it |
| iOS | `UIApplication openURL:options:completionHandler:` | handed over |
| Android | `startActivity` with an `ACTION_VIEW` `Intent` (JNI) | an activity accepted it |
| Web | `window.open(url, "_blank", "noopener")` | handed over (browsers allow it from a click) |

Inside Flatpak or Snap, the portal is what lets a sandboxed app open a URL;
outside, `xdg-open` follows the desktop's settings (and `$BROWSER`).

## URLs that open the app

The other direction: a URL that opens the app, or reaches it while it runs.
An OAuth sign-in in the browser redirects back to `myapp://callback?code=…`,
an email links to `https://example.com/invite/42`, a web page offers "open in
the app". `App::on_open_url` hears each one:

```rust
App::new(state, view)
    .on_open_url(|s, url| {
        if let Some(code) = url.strip_prefix("myapp://callback?code=") {
            s.sign_in_with(code);
        }
    })
    .run();
```

The URL that launched the app arrives once the app runs, after
`restore_state` (with the first events: the state exists, and the view is
rebuilt after the hook); later ones arrive as they come, also while the app is
in the background. The hook gets the URL as the system gave it (still
percent-encoded); it is outside input, so parse it, don't trust it. Each one
is also `Event::OpenUrl` at the platform level.

The app claims its URLs in its packaging manifest (cargo-stipple, see
[packaging.md](packaging.md#urls-that-open-the-app)):

```toml
url-schemes = ["myapp"]                 # myapp://…: any app or page can send these
associated-domains = ["example.com"]    # https://example.com/…: verified as the app's
```

| | Custom schemes (`url-schemes`) | https links (`associated-domains`) | How Stipple hears them |
|---|---|---|---|
| iOS | `CFBundleURLTypes` | universal links: the `com.apple.developer.associated-domains` entitlement (`applinks:`) and the domain's `apple-app-site-association` | the scene delegate's `scene:openURLContexts:` and `scene:continueUserActivity:` (`NSUserActivityTypeBrowsingWeb`), and the connection options at launch; without scenes, the app delegate's `application:openURL:options:` and `application:continueUserActivity:restorationHandler:`, and the launch options |
| Android | a `VIEW` / `BROWSABLE` intent filter for the scheme | App Links: an `android:autoVerify` https filter per domain and the domain's `assetlinks.json` | Stipple's `Link` activity (below) |
| macOS | `CFBundleURLTypes` in the bundle's `Info.plist` | (universal links on macOS need an App Store or Developer ID app: not wired) | the application delegate's `application:openURLs:` (the `kAEGetURL` Apple event), at launch and while running |
| Windows | a protocol handler in the registry (below) | — | the URL on the command line, at launch |
| Linux | `MimeType=x-scheme-handler/myapp;` in the app's `.desktop` file | — | the URL on the command line, at launch |
| Web | — | — | not delivered: the page's own URL is the app's (`location`), read at start |

**Android.** A `NativeActivity` doesn't pass `onNewIntent` on, and an intent
filter on it would start a second copy of it in the browser's task. So the
filters go on `org.stipple.android.Link`, a windowless activity in Stipple's
`classes.dex` (the same dex as the `Alarm` receiver and the `Tapped`
activity, which notification taps go through for the same reason): it hands
its intent's URL to the app, whatever runs, brings the app's activity forward
with its launch intent (or starts it, the event loop then finding the URL
queued) and finishes. cargo-stipple declares it and writes the dex; by hand:

```xml
<activity android:name="org.stipple.android.Link" android:exported="true"
    android:taskAffinity="" android:excludeFromRecents="true" android:noHistory="true"
    android:theme="@android:style/Theme.NoDisplay">
  <intent-filter>
    <action android:name="android.intent.action.VIEW"/>
    <category android:name="android.intent.category.DEFAULT"/>
    <category android:name="android.intent.category.BROWSABLE"/>
    <data android:scheme="myapp"/>
  </intent-filter>
  <intent-filter android:autoVerify="true">
    <action android:name="android.intent.action.VIEW"/>
    <category android:name="android.intent.category.DEFAULT"/>
    <category android:name="android.intent.category.BROWSABLE"/>
    <data android:scheme="https"/>
    <data android:host="example.com"/>
  </intent-filter>
</activity>
```

A manifest that puts `VIEW` filters on the `NativeActivity` itself still gets
the URL that started it (Stipple reads the activity's launch intent), but not
the ones that come while it runs.

**The server side of verified links.** The domain says which apps may open
its links, over https, without redirects:

- iOS: `https://example.com/.well-known/apple-app-site-association` (JSON, no
  extension, `Content-Type: application/json`), naming the app by team and
  bundle id and the paths it takes:

  ```json
  { "applinks": { "details": [ {
      "appIDs": ["ABCDE12345.com.example.app"],
      "components": [ { "/": "/invite/*" }, { "/": "/auth/*" } ]
  } ] } }
  ```

  Apple's CDN fetches it when the app is installed; for development, an
  `associated-domains = ["example.com?mode=developer"]` entry fetches it
  straight from the server on devices in developer mode. The provisioning
  profile must have the Associated Domains capability.
- Android: `https://example.com/.well-known/assetlinks.json`, naming the
  package and the SHA-256 fingerprint of the signing certificate (Play's app
  signing key for a Play build: Play Console, *App integrity*):

  ```json
  [ { "relation": ["delegate_permission/common.handle_all_urls"],
      "target": { "namespace": "android_app", "package_name": "com.example.app",
                  "sha256_cert_fingerprints": ["AB:CD:…"] } } ]
  ```

  Android checks it at install (`adb shell pm get-app-links com.example.app`
  shows the result; `pm verify-app-links --re-verify` asks again). Unverified,
  the link opens a chooser (Android 11 and older) or the browser (12 and
  later). A path prefix in the manifest's entry (`example.com/auth`) limits
  the filter to it.

**Desktop protocol handlers.** cargo-stipple doesn't package desktop apps, so
the app's installer registers the scheme. The URL arrives as a command-line
argument; Stipple delivers the arguments that look like URLs (a scheme, then
`:/`, not `file:`), once, as the loop starts. There is no single-instance
forwarding: a URL while the app runs starts a second process, which may hand
it on to the first itself (a socket, a lock file) and exit.

- Windows (per user, no administrator rights):

  ```text
  HKEY_CURRENT_USER\Software\Classes\myapp
      (Default)      = "URL:My App"
      URL Protocol   = ""
  HKEY_CURRENT_USER\Software\Classes\myapp\shell\open\command
      (Default)      = "C:\Program Files\My App\myapp.exe" "%1"
  ```

- Linux: a `.desktop` file with `Exec=myapp %u` and
  `MimeType=x-scheme-handler/myapp;` in `~/.local/share/applications` (or
  the package's `/usr/share/applications`), then
  `xdg-mime default myapp.desktop x-scheme-handler/myapp`.
- macOS: the bundle's `Info.plist`:

  ```xml
  <key>CFBundleURLTypes</key>
  <array><dict>
    <key>CFBundleURLName</key><string>com.example.app</string>
    <key>CFBundleURLSchemes</key><array><string>myapp</string></array>
  </dict></array>
  ```

  Launch Services finds the app once it has been run (or moved into
  `/Applications`); `application:openURLs:` hears the URLs, file URLs (a
  document dropped on the Dock icon) left out.

**OAuth.** The usual browser sign-in: open the provider's page with
`platform::open_url`, with `redirect_uri=myapp://callback` (and PKCE), and
take the code in `on_open_url`. `ASWebAuthenticationSession` (iOS) and Custom
Tabs (Android), which keep the sign-in in a sheet over the app, aren't wired
yet: the round trip goes through the system browser.

## App directories and bundle information

```rust
use stipple::platform::{self, AppInfo};

// For where the OS can't tell (Windows, Linux, macOS outside a bundle).
let app = App::new(state, view).app_info(
    AppInfo::new("org.example.Notes")      // reverse-DNS by convention
        .name("Notes")
        .version(env!("CARGO_PKG_VERSION")),
);

let info = platform::app_info();           // id, name, version, build
let dirs = platform::dirs()?;              // data, config, cache, temp
std::fs::write(dirs.data.join("notes.db"), b"")?;
```

`dirs()` returns the app's own directories, creating each one (and its
parents) if it isn't there yet; the temp directory is made readable by the user
only on Linux and macOS, and refused if something else is in its place. It
fails on the web and when the OS has no answer (no `$HOME` on Linux, no
activity yet on Android).

- `data`: files the user would miss (a database, documents), backed up where
  the OS backs up apps.
- `config`: settings; the same directory as `data` everywhere but Linux.
- `cache`: files that can be made again, kept across launches but not backed
  up, which the OS may delete when storage runs low.
- `temp`: scratch files for this run, which may go at any time the app isn't
  running.

| | `data` and `config` | `cache` | `temp` |
|---|---|---|---|
| macOS | `~/Library/Application Support/<id>` | `~/Library/Caches/<id>` | `$TMPDIR/<id>` |
| iOS | `Library/Application Support` | `Library/Caches` | `tmp` |
| Android | `getFilesDir()` (`ANativeActivity.internalDataPath`) | `getCacheDir()` | `getCacheDir()/tmp` |
| Windows | `%APPDATA%\<id>` (`FOLDERID_RoamingAppData`) | `%LOCALAPPDATA%\<id>\Cache` | `%TEMP%\<id>` |
| Linux | `$XDG_DATA_HOME/<id>`, `$XDG_CONFIG_HOME/<id>` | `$XDG_CACHE_HOME/<id>` | `$XDG_RUNTIME_DIR/<id>`, else `<id>-<uid>` in `$TMPDIR` or `/tmp` |
| Web | — | | |

The macOS paths are from `NSFileManager` (`URLsForDirectory:inDomains:`,
`temporaryDirectory`), so a sandboxed app gets its container's. The iOS and
Android ones are the app's own container, so they carry no id. On Linux the
XDG defaults apply when a variable is unset or not absolute
(`~/.local/share`, `~/.config`, `~/.cache`); inside Flatpak they are the
app's own already. `<id>` is `app_info().id` made into one safe path component
(separators and `..` replaced).

`app_info()` takes each field from the OS where the app is packaged, then from
what `App::app_info` (or `platform::set_app_info`) gave, then from the
executable's name for the id and name (with no version):

| | id | name | version | build |
|---|---|---|---|---|
| macOS, iOS | `CFBundleIdentifier` | `CFBundleDisplayName`, else `CFBundleName` | `CFBundleShortVersionString` | `CFBundleVersion` |
| Android | the package name | the application's label | `versionName` | `versionCode` (`getLongVersionCode` on API 28+) |
| Windows | — | `ProductName`, else `FileDescription` | `ProductVersion` (the string, else the fixed `a.b.c.d`) | — |
| Linux, web | — | — | — | — |

A binary run outside an app bundle (`cargo run` on macOS) has no
`CFBundleIdentifier` or versions, and a Rust executable has no Windows version
resource unless a build script adds one (`winres`, `embed-resource`), so desktop
apps should call `App::app_info` with their id and `env!("CARGO_PKG_VERSION")`.
Call it before `dirs()`: desktop directories are named after the id. On
Wayland, the id the app gave is also the window's app id
(`xdg_toplevel.set_app_id`), which the compositor matches to the app's
`.desktop` file.

There is no key-value settings store on top yet (`NSUserDefaults`,
`SharedPreferences`, `localStorage`), and no web storage: write files in
`dirs().config`. Secrets (tokens, private keys) don't belong in these
files, which backups copy: keep them in
[`platform::secure_store`](secure-storage.md), the Keychain, the Android
Keystore, Credential Manager or the Secret Service.

`lifecycle` prints both at start (`lifecycle: app ...`, `lifecycle: dirs ...`).
CI checks the X11 job's XDG directories exist, `touchdemo`'s iOS self-test
reads the simulator bundle's `Info.plist` values and container directories, and
`androiddemo` reads the emulator package's label, `versionName` and
`versionCode`, and its files and cache directories. macOS was checked by hand,
bundled and unbundled; Windows is build-checked only.

### Shared containers (app groups)

An iOS app and its [app extensions](packaging.md#app-extensions) are
separate processes, each with its own container: inside an extension,
`dirs()` are the extension's and `app_info()` reads the `.appex`'s
`Info.plist`. They share files through an app group's container:

```rust
// ios.app-groups = ["group.com.example.vpn"] in the manifest.
if let Some(dir) = platform::shared_dir("group.com.example.vpn") {
    std::fs::write(dir.join("config.json"), b"{}")?;
}
```

`shared_dir(group)` is `NSFileManager`'s
`containerURLForSecurityApplicationGroupIdentifier:`. On a device it is
`None` when the process isn't entitled to the group
(`com.apple.security.application-groups`, which `cargo-stipple` signs in
from `ios.app-groups`; the simulator is less strict), and on Windows, Linux,
Android and the web, which have no app groups. On macOS the same call gives
`~/Library/Group Containers/<group>` (created if missing; a sandboxed app
needs the entitlement). Files there aren't coordinated: when both sides may
write at once, write to a temporary name and rename. CI runs `extensiondemo`
in the simulator, where the app writes into the group's container and the
extension's executable reads it back (not blocking; passed in a CI trial run); macOS is
build-checked only.

## Packaged assets

Files the app ships with (art, fonts, sounds, levels) can be compiled in with
`include_bytes!`, which suits a few small ones. For more, `platform::assets()`
reads them from where each platform packages them, so they stay out of the
executable (a game's hundreds of megabytes of art don't belong in the `.so`
inside an APK) and load when they're needed:

```rust
use stipple::platform::{self, assets};

// From a checkout, `cargo run` and `cargo test` read the source tree's.
platform::set_assets_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"));

let sun = assets().image("art/sun.png")?;      // Pixmap
let logo = assets().svg("art/logo.svg")?;      // Svg
let title = assets().font("fonts/Display.ttf")?;
let coin = assets().sound("sfx/coin.wav")?;    // decoded whole
let story = assets().text("story/intro.txt")?;
let music = assets().read("music/theme.ogg")?; // bytes, for Audio::play_music
let mut level = assets().open("levels/1.bin")?; // Read + Seek, as it goes
assets().exists("art/moon.png");
assets().list("art")?;                         // file names, sorted
```

Paths are relative and `/`-separated on every platform; `..`, `.`, `\`,
`:`, an empty component or a leading `/` is refused
(`AssetError::InvalidPath`), so a path can't leave the assets.

| | Where the files are | Read with |
|---|---|---|
| Android | the APK's `assets/` | `AAssetManager_open` / `AAsset_read` on the activity's asset manager (no JNI) |
| iOS | `YourApp.app/assets/` | the file system, under `NSBundle.mainBundle.resourcePath` |
| macOS | `YourApp.app/Contents/Resources/assets/`; unbundled, `assets/` next to the executable | the same |
| Windows, Linux | `assets/` next to the executable | the file system |
| Web | `assets/` next to the page (`stipple.js`'s `assets` option) | `fetch` |

Off Android and the web, `$STIPPLE_ASSETS` names the directory instead of the
packaged one, and when neither exists, the directory given to
`platform::set_assets_dir` is used: the source tree's, so the same binary
reads its packaged files once installed and the checkout's under `cargo run`.
A shipped app never reads that path, since its packaged directory wins.
`assets().dir()` says which directory it found (`None` on Android and the
web).

**Loading off the UI thread.** A large image takes a while to decode, so a
scene's art is better loaded behind a loading screen. `AppHandle::load_asset`
reads the file and decodes it on a worker thread, then hands the result to
the state on the UI thread:

```rust
cx.handle().load_asset("scenes/forest.png", Pixmap::decode, |s: &mut State, r| {
    match r {
        Ok(pixmap) => s.background = Some(Arc::new(pixmap)),
        Err(e) => s.error = Some(e.to_string()),
    }
});
```

The art goes when the state drops its `Arc`. `assets().load(path, done)` is
the same without the state: `done` gets the bytes on a worker thread.

**On the web**, reading a file means fetching it, which can't block. So:

- `load_asset` and `assets().load` work for every file: `stipple.js` fetches
  `assets + path` and hands the bytes back when they arrive (decoding then
  runs on the page's thread, which has no other);
- the blocking calls (`read`, `open`, `image`, `text`, `exists`, `list`…)
  find only the files the page **preloaded** before it started the app, and
  fail with `AssetError::NotPreloaded` for others. The page lists them in
  `start`'s `preload` option, an array of paths or the URL of a JSON array (a
  manifest):

  ```js
  await start(canvas, "game.wasm", {
    font: "font.ttf",
    assets: "assets/",            // the default
    preload: "assets.json",       // ["ui/logo.svg", "fonts/Display.ttf", …]
  });
  ```

  Preload what the first screen needs, and `load_asset` the rest. A
  preloaded file stays in memory for the page's life.

**Packaging.** On Android, `aapt package -A assets/` puts a directory into
the APK (`examples/androiddemo/build-apk.sh` does, storing already-compressed
formats uncompressed); on iOS and macOS the bundle step copies it into the
`.app` (see the [iOS](platforms/ios.md#packaging) and
[Android](platforms/android.md#packaging) guides); on the desktop it goes
next to the executable; the site serves each web example's `assets/` next to
its page, with a manifest it preloads.

Music isn't decoded ahead: `Audio::play_music` keeps the *encoded* bytes and
decodes as it plays, so `assets().read("theme.ogg")` costs the file's size,
not the decoded track's. An image decodes to 4 bytes a pixel, so a 2048×2048
background is 16 MB however small its PNG was (see [Images](images.md#memory)).

Not there yet: Play Asset Delivery and iOS on-demand resources (files
downloaded after install), and `AAsset_openFileDescriptor` for handing an
uncompressed asset's file descriptor to a decoder.

## Notifications

```rust
use stipple::platform::{notify, Notification};
notify(&Notification::new("Export finished").body("report.pdf is ready"))?;
```

A notification is a title and a body. `AppHandle::notify` is the same call.

| | How |
|---|---|
| Linux | `org.freedesktop.Notifications.Notify` over D-Bus (GNOME, KDE, XFCE, mako, dunst…), on a thread |
| Windows | a balloon on the tray icon (`Shell_NotifyIconW` with `NIF_INFO`), which Windows 10 and 11 show as a toast; without a tray icon, a temporary icon is added for it and removed when the balloon goes |
| macOS | AppleScript's `display notification`, through `osascript` |
| Web | the `Notification` API, asking for permission first if it hasn't been granted or refused |
| iOS | `UNUserNotificationCenter`, asking for permission first (see below) |
| Android | `NotificationManager`, on a channel of the app's, asking for `POST_NOTIFICATIONS` first on Android 13 and later (see below) |

**macOS.** The notification frameworks need an app bundle:
`UNUserNotificationCenter` raises an exception in a bare executable (one with no
bundle identifier, which is what `cargo run` starts), and the deprecated
`NSUserNotificationCenter` returns `nil` there. `osascript` works from any
process, so that's what Stipple uses for now; macOS shows the notification as
coming from Script Editor. A bundled app should eventually use
`UNUserNotificationCenter`, after asking for authorization.

**iOS.** The first `notify` asks the user's permission
(`requestAuthorizationWithOptions:completionHandler:`, alert, sound and badge),
which shows the system's prompt; the notification waits for the answer and is
shown if it's yes. Later calls go straight through (iOS answers without asking
again), and fail with `Err` once permission is refused. Each one is a
`UNNotificationRequest` with no trigger, so it shows at once, and a delegate
on the center lets it show as a banner while the app is on screen too (iOS
otherwise keeps a foreground app's notifications to itself).

**Android.** Each notification is built with `Notification.Builder` and posted
with `NotificationManager.notify`, on the channel `stipple` ("Notifications",
default importance), made on Android 8 and later. Its icon is the app's (or
the system's info icon when the app has none), and tapping it opens the app.
On Android 13 and later posting needs the `POST_NOTIFICATIONS` runtime
permission, which the manifest must declare:

```xml
<uses-permission android:name="android.permission.POST_NOTIFICATIONS"/>
```

The first `notify` without it asks (the system's dialog, through
`requestPermissions` from a headless fragment the backend adds to the
activity, since a `NativeActivity` doesn't pass the answer on); the
notification waits for the answer and is posted if it's yes. Once refused,
`notify` fails with `Err` until the user allows notifications in the
settings. On any version, `notify` fails if the user turned the app's
notifications off.

**Windows.** Notifications need the app's window to exist, since the icon that
carries the balloon belongs to it. Windows names the notification after the
executable; a toast with buttons or images would need the WinRT toast APIs and
an AppUserModelID, which Stipple doesn't use.

Clicking a notification shown with `notify` does nothing yet, and there are
no actions or icons. Scheduled ones report their taps (below).

To check where the permission stands, or ask for it before the first
notification, use `platform::permission` with `Permission::Notifications`
(`status`, `request`, `open_settings` once refused); it covers the camera,
the microphone, photos and the location the same way. See
[camera.md](camera.md#permissions).

### Scheduled notifications

```rust
use std::time::Duration;
use stipple::platform::{self, Notification, ScheduledNotification};
use stipple::platform::permission::Permission;

// At a moment that makes sense to the user (not on the first schedule):
platform::permission::request(Permission::Notifications, |status| { /* … */ });

platform::schedule_notification(
    &ScheduledNotification::new("potion", Notification::new("The potion is ready"))
        .after(Duration::from_secs(15 * 60)),   // or .at(SystemTime), .repeating(Duration)
)?;
platform::cancel_notification("potion");        // or cancel_all_notifications()
platform::pending_notifications(|ids| { /* reconcile after a restart */ });

App::new(state, view).on_notification_tapped(|s, id| s.open_panel(id));
```

A scheduled notification has an id of the app's choosing: scheduling again
under it replaces the pending one (a game reschedules whenever its timers
change), cancelling takes it back (and removes it from the notification
center once shown), and a tap that brings the app up reports it as
`Event::NotificationTapped(id)`, which `App::on_notification_tapped` hears.
`repeating(interval)` shows it again every interval after the first time.

| | Delivered by | App not running | Tap reported | Permission |
|---|---|---|---|---|
| iOS | `UNUserNotificationCenter`: a `UNTimeIntervalNotificationTrigger`, or a `UNCalendarNotificationTrigger` for a daily or weekly repeat | shown | yes, also when it launches the app | `requestAuthorizationWithOptions:` |
| Android | `AlarmManager` (exact where allowed, else inexact) and a broadcast receiver that posts it | shown with the `Alarm` receiver in the APK; otherwise only while the process lives | yes with the `Tapped` activity in the APK; otherwise when the tap starts the activity | `POST_NOTIFICATIONS` (13+) |
| Web | a timer in the page, then the `Notification` API | no (while the page is open) | yes (`onclick`) | `Notification.requestPermission` |
| macOS, Windows, Linux | a timer thread, then `notify` | no (while the app runs) | no | none |

- **iOS** repeats an interval of a minute or more, starting one interval after
  scheduling; a 24-hour or 7-day repeat is a calendar trigger at the first
  time's time of day (and weekday). The center keeps the requests across
  launches, and `pending_notifications` asks it (answering on a queue of its
  own). Without permission the center refuses the request (logged).
- **Android** keeps what's scheduled in a file of the app's, since
  `AlarmManager` can't list its alarms; the alarms are set again when the app
  starts, and at boot with the receiver. Delivery while the process is gone
  needs a `classes.dex` in the APK and a manifest entry for its `Alarm`
  receiver. A tap while the activity is alive is reported through the same
  dex's `Tapped` activity, another manifest entry (a windowless activity that
  reports the tap and brings the app forward, since `NativeActivity` doesn't
  pass `onNewIntent` on); without it, such a tap only brings the app to the
  front. See [the Android page](platforms/android.md#scheduled-notifications),
  with the exact-alarm permissions.
- **The desktops** have no scheduler of their own that Stipple uses (macOS's
  `UNUserNotificationCenter` needs a bundle, Windows' scheduled toasts an
  AppUserModelID, Linux has none): a thread shows each one with `notify` when
  it's due, while the app runs, and forgets them when it quits (so
  `pending_notifications` is empty on the next launch: schedule again from
  the app's own state). They are plain notifications, so a click on one isn't
  reported, as for `notify`.
- **The web** has nothing reliable without a push server and a service
  worker; the page's timers stop with the tab (reloading it forgets them),
  and a hidden tab's timers may be slowed down by the browser, so one can
  show late. A click on one is reported while the page is open.

## The share sheet

```rust
use stipple::platform::{Share, ShareOutcome};

handle.share(
    Share::new()
        .text("Scanned with QR Master")
        .url("https://example.com/item/42")
        .subject("A code")                      // mail's subject line
        .file("code.png", png_bytes, "image/png")
        .anchor(button_rect),                   // an iPad's popover points here
    |s: &mut State, outcome| match outcome {
        ShareOutcome::Shared => s.toast("Shared"),
        ShareOutcome::Unavailable => s.copy_to_clipboard(),
        _ => {}
    },
);
```

`AppHandle::share` shows the system's share sheet and runs the answer as a
state update; `platform::share(&share, done)` is the same with a plain
callback (run on whichever thread the system answers on). Call it from the
handler of the button the user pressed.

| | How | `Shared` means | Files |
|---|---|---|---|
| iOS | `UIActivityViewController`, a popover from the anchor on an iPad | the activity completed | written to the app's tmp directory, as file URLs |
| Android | `Intent.createChooser` over `ACTION_SEND` / `ACTION_SEND_MULTIPLE` | a target was picked (the chooser's `IntentSender`) | put in `MediaStore`'s Downloads (Android 10+), where they stay |
| macOS | `NSSharingServicePicker` from the key window's view, at the anchor | a service was picked | temporary files, as file URLs |
| Web | `navigator.share` | the promise resolved | `File`s, where `navigator.canShare` takes them |
| Windows, Linux | — (`Unavailable`) | | |

On Android, a share of files only fails (`Unavailable`) before Android 10,
since files need a `FileProvider` otherwise, which is a Java class. Windows'
`DataTransferManager` share UI needs WinRT interop Stipple doesn't have yet,
and Linux desktops have no share portal: there `Unavailable` comes at once, so
the app can copy or save instead.

## Haptic feedback

```rust
use stipple::platform::{Haptic, haptic};
haptic(Haptic::Success); // Selection, Light, Medium, Heavy, Success, Warning, Error
```

A short tap on the device's haptic engine, a no-op where there's none.

| | How |
|---|---|
| iOS | `UISelectionFeedbackGenerator`, `UIImpactFeedbackGenerator` (light, medium, heavy), `UINotificationFeedbackGenerator` (success, warning, error) |
| Android | `View.performHapticFeedback` on the app's view (`CLOCK_TICK`, `KEYBOARD_TAP`, `VIRTUAL_KEY`, `LONG_PRESS`; `CONFIRM` and `REJECT` on Android 11+), which needs no permission and follows the user's touch-feedback setting |
| macOS | `NSHapticFeedbackManager` (felt on a Force Touch trackpad while touching it) |
| Web | `navigator.vibrate` with a short pattern (Chrome and Firefox on Android) |
| Windows, Linux | nothing |

## Message boxes

Ask the user something, and get the answer without blocking the event loop:

```rust
use stipple::platform::dialog::{MessageBox, MessageKind};

handle.message(
    MessageBox::new("Close this tab?")
        .detail("A process is still running in it.")
        .kind(MessageKind::Question)      // Info, Warning, Error, Question: the icon
        .buttons(["Close", "Cancel"])     // in reading order; none means "OK"
        .cancel(1),                       // Escape; .default_button(i) is Enter
    |s: &mut State, r| {
        if r.button == Some(0) { s.close_tab(); }
    },
);
```

`AppHandle::message` can be called from any thread. The answer runs on the UI
thread as a state update, like `AppHandle::update`, with a `MessageResponse`:
the button pressed (Escape counts as the cancel button) and, for a box with a
text field (`.input(value, secure)`), what was typed. A worker thread that must
wait for the answer (an SSH host-key prompt) sends it back over a channel from
the closure and waits on that, not on the UI. Boxes asked for while one is up
wait their turn.

`.window(id)` says which window a box is about (an SSH prompt belongs to its
session's window): it's drawn in that window, a sheet on it on macOS, and
owned by it on Windows. Without it, a drawn box goes in the primary window
and a native one on the key or active window.

| | How |
|---|---|
| Windows | `TaskDialogIndirect`, with common controls 6 activated for the call (an executable without a manifest gets version 5, which has no task dialog; shell32's manifest resource asks for 6), else `MessageBoxW` when the buttons are a standard set (OK, OK/Cancel, Yes/No, Yes/No/Cancel, Retry/Cancel, Abort/Retry/Ignore). It runs on a thread of its own, owned by the active window, which it disables while it's up |
| macOS | `NSAlert` as a sheet on the app's window, its completion handler a block built by hand; a text field is the alert's accessory view (`NSSecureTextField` when secure). With no window, `runModal`, which does block |
| iOS | `UIAlertController`, presented from the key window's root view controller; the cancel button gets the cancel style |
| Web | `window.alert` for one button and `window.confirm` for two; they block the page, as browsers do. More buttons are drawn |
| Linux, Android | drawn in the window (no toolkit-free alert API exists on Linux, and the desktop portal has none; Android's `AlertDialog` needs a Java listener class) |

A box drawn in the window is a panel in the theme over a scrim, in its window
(the primary one by default): an icon for its kind, the message and detail, the text field, and the
buttons, the default one filled. The default button (or the text field) takes
focus while it's up, and focus goes back after; Enter presses the default
button and Escape the cancel one. Clicks outside it do nothing. It's a `Dialog`
to assistive technologies. `.drawn(true)` draws a box even where there's a
native alert (`STIPPLE_DRAWN_DIALOGS=1` does it for every box), and a box with
a text field is drawn everywhere but macOS and iOS.

`platform::dialog::message(&mb, done)` is the platform half on its own: it
shows the native alert and returns `Err(done)` where there isn't one.

## The tray icon

Declare it like the menu bar, as a function of the state:

```rust
struct State { muted: bool, shown: bool }

App::new(state, view).tray(|s: &State| {
    Tray::new("My App")                  // the tooltip
        .icon(icon_pixmap)               // straight RGBA, ideally square
        .menu(
            Menu::new("")
                .item(MenuItem::new("&Mute", |s: &mut State| s.muted = !s.muted).checked(s.muted))
                .separator()
                .submenu(Menu::new("More").item(MenuItem::new("About", |_: &mut State| {})))
                .item(MenuItem::new("&Quit", |_: &mut State| { /* handle.exit() */ })),
        )
        .on_activate(|s: &mut State| s.shown = true)
})
```

The function is called again as the state changes, and the icon, tooltip and
menu follow it. Menu items work as in [menus](menus.md) (labels with `&`
mnemonics, enabled and checked states, separators, submenus), except that
shortcuts don't apply. `on_activate` runs when the icon itself is clicked.

| | Shown as | Menu | `on_activate` |
|---|---|---|---|
| Windows | a notification-area icon (`Shell_NotifyIconW`) | on a right-click (`TrackPopupMenu`) | a left click |
| macOS | an `NSStatusItem` in the menu bar | on any click (`NSMenu`) | a click, only when there's no menu |
| Linux | a `StatusNotifierItem` over D-Bus, with its menu over `com.canonical.dbusmenu` | as the host shows it (usually a right-click) | the host's "activate" (usually a left click) |
| iOS, Android, web | — | | |

**Linux.** `StatusNotifierItem` is the tray protocol of KDE Plasma, XFCE,
Cinnamon, LXQt, waybar and swaybar, and of GNOME with the AppIndicator
extension (Ubuntu ships it). Stipple serves it by hand on a thread with its own
session-bus connection: it claims `org.kde.StatusNotifierItem-<pid>-<n>`,
registers with `org.kde.StatusNotifierWatcher`, and answers the host's calls,
sending `NewIcon`/`NewToolTip` and `LayoutUpdated` when the tray changes. The
icon goes over as pixmaps (22, 32 and 64 pixels), so no icon theme is needed.
Without a watcher (plain GNOME, a bare window manager) nothing shows; the item
registers as soon as one starts. The old XEmbed system tray isn't supported.

**macOS.** The icon is drawn at 18×18 points; without one the tooltip shows as
the item's title.

**Windows.** The icon comes back after Explorer restarts (`TaskbarCreated`), and
is removed when the window closes.

### At the platform level

`App::tray` hands the backend the tray's tooltip, icon and menu (with item ids
above the menu bar's). Picks arrive as `Event::MenuCommand(id)`, like the menu
bar's, and clicks as `Event::TrayActivated`. `platform::tray_shown()` says whether
the last update was shown.

## What CI checks

`traydemo --selftest` runs on each desktop:

- **Linux**, under `dbus-run-session` with a mock desktop
  (`.github/scripts/mock-desktop.py`) that owns the notification server, the
  portal's `OpenURI` and the `StatusNotifierWatcher`. It logs the `Notify` and
  `OpenURI` calls, then plays the tray host: it reads the item's properties
  (title, icon pixmaps, menu path), fetches the dbusmenu layout, clicks "Ping"
  and activates the icon, and the app must run both. A `file:` URL must reach
  `xdg-open` (a logging stand-in).
- **Windows**: `Shell_NotifyIconW` must take the icon and the balloon, and
  `open_url` must reach a `stipple-selftest:` URL protocol registered in HKCU
  for the run. Opening a `.txt` is checked, but only warned about.
- **macOS**: the `NSStatusItem` must exist, `osascript`'s notification succeed,
  and `open_url` of a `.txt` start TextEdit.

On the **iOS** simulator, `touchdemo`'s self-test calls `notify`: it must hand
the notification over (`Ok`) and the center must report the app's permission
(`getNotificationSettingsWithCompletionHandler:`). Nobody answers the
permission prompt there, so the notification itself isn't shown; the
screenshot shows the prompt. It also schedules one an hour on and lists the
pending ones (the center's answer to the request is logged).

On the **Android** emulator, `androiddemo` opens the share chooser with a
text, a URL and a file, backs out of it (`Cancelled`), plays a haptic, and
schedules a notification ten seconds on; the app goes to the background and
its process is ended, the APK's `Alarm` receiver posts the notification
(`dumpsys notification`), and tapping it starts the app, which hears its id.
Linux's `traydemo` self-test schedules one half a second on, which must reach
the mock notification server. The macOS share picker and haptics, the iOS
share sheet and the web paths are build-checked only.

URLs that open the app: on the Android emulators,
`examples/androiddemo/test-open-url.sh` starts `VIEW` intents of androiddemo's
`stipple-demo` scheme with the app stopped, in front and in the background,
and reads `on_open_url`'s log back (passed locally on API 34 and 30, plus a
verified-by-hand App Link with a path prefix on API 34; non-blocking in CI).
On the iOS simulator, touchdemo's bundle claims the scheme and `simctl
openurl` opens two URLs, one launching it (non-blocking, not yet seen
passing). macOS's `application:openURLs:` is build-checked; the desktops'
command-line URL was checked by hand on X11.

No real tray host or notification popup is looked at, and nothing clicks the
real icons on Windows or macOS: there, the menus and click handling are
build-checked, along with the Android and web paths and opening URLs on iOS.

For message boxes, `sessionsdemo --alert-selftest` asks a three-button
question as soon as it starts and presses the second button itself after
`STIPPLE_ALERT_HOLD_MS` (through `dialog::press_native_button`:
`TDM_CLICK_BUTTON` on the task dialog, `performClick:` on the `NSAlert`'s
button), then prints the answer:

- **Windows**: the task dialog must answer button 101 and the app get
  `Some(1)`; PowerShell screenshots the dialog (`#32770`) while it's up.
- **macOS**: the alert must come up as a sheet and answer 1001; the screen is
  captured while it's up.
- **Linux (X11)**: the drawn box is screenshotted (dark theme) and dismissed
  with Escape by `xdotool`; the app must get the cancel button. The
  `visual-sessions` job also closes a busy tab (a box asks first), cancels it,
  and opens the color picker and types a password, comparing screenshots.

Real key presses on the native alerts aren't sent (the self-test clicks
programmatically), and the iOS and web alerts are build-checked only.

For packaged assets: `androiddemo`'s APK carries an `assets/` directory, and
on the emulator it reads a file whole (its size and FNV-1a hash must match the
source's), lists the top and a subdirectory, decodes an SVG, reads after a
seek and reports a missing file, all through the `AAssetManager`. `touchdemo`'s
iOS self-test does the same from the simulator bundle's `assets/` (the
directory must be the bundle's), and loads an SVG through
`AppHandle::load_asset`. The Linux `visual-image` job runs `imagedemo` with
its PNG and SVG next to the executable, and the site's `check.py` runs it in
headless Chrome with the page preloading them. macOS was run by hand bundled,
unbundled (next to the executable) and from the checkout; Windows is
build-checked only.
