# High-DPI

## Logical and physical pixels

An app works entirely in **logical pixels**: layout, sizes passed to
`App::logical_size`, and the positions in pointer events. The backend tells the
App how many **physical pixels** each logical pixel spans (the window's
`scale_factor`), and the App renders the frame at that resolution. Text and
shapes stay sharp, and OS-drawn controls are drawn at the same resolution.

A 640x480 window on a 2x display has a 1280x960 framebuffer; a click 100 logical
pixels from the left arrives at x = 100.

## Where the scale comes from

| Platform | Source | Changes while running |
|---|---|---|
| Windows | The window's DPI / 96, with per-monitor DPI awareness | ✅ on `WM_DPICHANGED` |
| macOS | The window's `backingScaleFactor` | ✅ when the window changes display |
| iOS | `UIScreen.scale` | — |
| X11 | `Xft.dpi` / 96, from `RESOURCE_MANAGER` | ✅ when the root's `RESOURCE_MANAGER` changes (a settings daemon, `xrdb -merge`) |
| Wayland | The compositor's preferred fractional scale (`wp_fractional_scale_v1`), else the scale of the output the window is on | ✅ |
| Web | `devicePixelRatio`; the canvas's backing store is its CSS size times the ratio | ✅ on resize, zoom, or moving to another screen |
| Android | Not yet: always 1x | — |

On Wayland, a compositor with `wp_fractional_scale_v1` and `wp_viewporter`
(GNOME, KDE, sway and most others) tells each window its preferred scale,
fractional ones included: at 150% the window draws its buffers at 1.5
physical pixels per logical one and a viewport maps them back to its
logical size, so text stays sharp instead of being a 2x buffer the
compositor shrinks. Without them the scale is the output's whole one.

On X11, a new `Xft.dpi` (the desktop's scale settings, or `xrdb -merge`)
reaches every window while the app runs: the window keeps its size in
physical pixels and lays out again at the new scale
(`Event::ScaleFactorChanged`, then `Event::Resized`).

CI runs X11 at 2x (setting `Xft.dpi`), changes it to 1.5x while an app runs,
and runs Wayland at 2x and 1.5x (`swaymsg output … scale 1.5`, checking the
buffer is drawn at 1.5x), and clicks a control at its physical position to
check that positions arrive in logical pixels. The web's selftest runs in headless Chrome at a pixel ratio of 2. Windows and macOS runners are 1x, so their high-DPI paths haven't
been seen on a real high-DPI display yet.
