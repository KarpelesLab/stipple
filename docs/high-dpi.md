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
| X11 | `Xft.dpi` / 96, from `RESOURCE_MANAGER` | ❌ read at startup |
| Wayland | The scale of the output the window is on | ✅ |
| Web | `devicePixelRatio`; the canvas's backing store is its CSS size times the ratio | ✅ on resize, zoom, or moving to another screen |
| Android | Not yet: always 1x | — |

CI runs X11 at 2x (setting `Xft.dpi`) and Wayland at 2x (`swaymsg output … scale 2`),
and clicks a control at its physical position to check that positions arrive in
logical pixels. The web's selftest runs in headless Chrome at a pixel ratio of 2. Windows and macOS runners are 1x, so their high-DPI paths haven't
been seen on a real high-DPI display yet.
