# Tray icons, notifications, message boxes and opening URLs

Four ways an app reaches past its window: a tray icon with a menu, desktop
notifications, message boxes, and handing a URL or file to the app the user
picked for it.
The `traydemo` example shows all three:

```sh
cargo run -p traydemo
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

Clicking a notification does nothing yet, and there are no actions or icons.

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

| | How |
|---|---|
| Windows | `TaskDialogIndirect`, with common controls 6 activated for the call (an executable without a manifest gets version 5, which has no task dialog; shell32's manifest resource asks for 6), else `MessageBoxW` when the buttons are a standard set (OK, OK/Cancel, Yes/No, Yes/No/Cancel, Retry/Cancel, Abort/Retry/Ignore). It runs on a thread of its own, owned by the active window, which it disables while it's up |
| macOS | `NSAlert` as a sheet on the app's window, its completion handler a block built by hand; a text field is the alert's accessory view (`NSSecureTextField` when secure). With no window, `runModal`, which does block |
| iOS | `UIAlertController`, presented from the key window's root view controller; the cancel button gets the cancel style |
| Web | `window.alert` for one button and `window.confirm` for two; they block the page, as browsers do. More buttons are drawn |
| Linux, Android | drawn in the window (no toolkit-free alert API exists on Linux, and the desktop portal has none; Android's `AlertDialog` needs a Java listener class) |

A box drawn in the window is a panel in the theme over a scrim, in the primary
window: an icon for its kind, the message and detail, the text field, and the
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

`App::tray` hands the backend a `NativeTray` (tooltip, icon, and menu items
whose ids start at `TRAY_ID_BASE`, above the menu bar's) through
`Window::set_tray`. Picks arrive as `Event::MenuCommand(id)`, like the menu
bar's, and clicks as `Event::TrayActivated`. `shell::tray_shown()` says whether
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
screenshot shows the prompt.

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
