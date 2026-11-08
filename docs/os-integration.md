# Tray icons, notifications and opening URLs

Three ways an app reaches past its window: a tray icon with a menu, desktop
notifications, and handing a URL or file to the app the user picked for it.
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
| iOS, Android | not yet (`Err`) |

**macOS.** The notification frameworks need an app bundle:
`UNUserNotificationCenter` raises an exception in a bare executable (one with no
bundle identifier, which is what `cargo run` starts), and the deprecated
`NSUserNotificationCenter` returns `nil` there. `osascript` works from any
process, so that's what Stipple uses for now; macOS shows the notification as
coming from Script Editor. A bundled app should eventually use
`UNUserNotificationCenter`, after asking for authorization.

**Windows.** Notifications need the app's window to exist, since the icon that
carries the balloon belongs to it. Windows names the notification after the
executable; a toast with buttons or images would need the WinRT toast APIs and
an AppUserModelID, which Stipple doesn't use.

Clicking a notification does nothing yet, and there are no actions or icons.

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

No real tray host or notification popup is looked at, and nothing clicks the
real icons on Windows or macOS: there, the menus and click handling are
build-checked, along with the iOS, Android and web paths.
