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

## Fonts

`Font::system_default()` loads Segoe UI, falling back to Arial, with Segoe UI
Symbol for symbols such as the checkbox's check mark.

## Known gaps

- No system clipboard or file dialogs yet.
