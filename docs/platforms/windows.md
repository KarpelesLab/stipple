# Windows

The Win32 backend uses `user32`, `gdi32` and friends directly, without the
`windows` crate. A plain `cargo run` gives a working app.

## High-DPI

The backend makes the process per-monitor DPI aware (`PER_MONITOR_AWARE_V2`)
when it starts, follows `WM_DPICHANGED` when the window moves between displays,
and converts mouse positions to logical pixels.

If your app embeds a manifest that sets a DPI awareness, Windows keeps that one
and the backend's call does nothing. Choose per-monitor v2 in the manifest so the
two agree.

> Not yet seen on a scaled display: CI runners are 1x. Reports welcome.

## Appearance

`Theme::native()` reads dark mode from `AppsUseLightTheme` in the registry and
the accent from `DwmGetColorizationColor`. The title bar is made dark through
`DWMWA_USE_IMMERSIVE_DARK_MODE` whenever the theme's background is dark
(Windows 10 20H1 and later).

## OS-drawn controls

Under `Theme::native()` the visual styles engine (`uxtheme`) draws buttons,
checkboxes, radio buttons, sliders, progress bars and text-field frames. Each is
drawn twice, over black and over white, to recover its alpha.

- Win32 has no toggle switch, so `switch` stays self-drawn.
- Dark mode uses the `DarkMode_Explorer` and `DarkMode_CFD` theme classes that
  Windows' own apps use. Microsoft doesn't document them; where one is missing,
  the light class is used.
- The text field draws its own focus, so Stipple's focus ring is left off it.

In a window, buttons, sliders and progress bars are real child windows
instead (`BUTTON`, `msctls_trackbar32`, `msctls_progress32`; see
[Real controls](../native-theme.md#real-controls)), live: made under an
activation context for Common Controls 6 (the manifest `shell32.dll` carries,
so they get the visual styles without the app shipping a manifest), placed at
the control's frame in physical pixels, in Segoe UI at the app's text size and the window's DPI, and
`DarkMode_Explorer`-themed in dark mode. The window clips them out of its own
painting (`WS_CLIPCHILDREN`) and answers their `WM_PRINTCLIENT` with its
frame, which is what shows behind a themed button's rounded corners. A click
(`BN_CLICKED`) or a slider's move (`WM_HSCROLL`) reaches the app as the
element's tap or drag, and the keyboard focus goes back to the window. Under
UI Automation each is its child window's element, so the app's tree leaves its
node out. Checkboxes and radio buttons stay pictures (a Win32 one shows and is
named by a text of its own, where Stipple draws the label beside the box).

## Fonts

`Font::system_default()` loads Segoe UI, falling back to Arial, with Segoe UI
Symbol for symbols such as the checkbox's check mark.

## URLs that open the app

A protocol handler registered in the registry (`HKCU\Software\Classes\myapp`
with `URL Protocol` and `shell\open\command` = `"app.exe" "%1"`) starts the
app with the URL as its argument, which `App::on_open_url` hears as the
loop starts. A URL while the app runs starts a second process: there is no
single-instance forwarding yet. See
[os-integration.md](../os-integration.md#urls-that-open-the-app).

## Known gaps

- The system clipboard (`CF_UNICODETEXT`) works with Ctrl+C/X/V; file dialogs
  (`IFileOpenDialog` / `IFileSaveDialog`) are implemented but not exercised in
  CI, since a modal dialog can't be driven headlessly.
- Ctrl and Windows-key shortcuts are recognised on letters and digits only, so
  punctuation shortcuts such as Ctrl+= don't reach the app yet.
