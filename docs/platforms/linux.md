# Linux

Stipple talks to the display server directly over its wire protocol: no GTK,
no Qt, no `libwayland` or `libxcb`.

## Choosing a backend

`App::run` tries, in order:

1. **Wayland**, when `$WAYLAND_DISPLAY` names a compositor socket;
2. **X11**, when `$DISPLAY` is set;
3. **headless**, which renders one frame off-screen and returns.

## Fonts

`Font::system_default()` loads the first of Liberation Sans, DejaVu Sans and Noto
Sans it finds in the usual `/usr/share/fonts` locations, and adds DejaVu Sans as
a fallback for glyphs the main font lacks. Shipping apps should bundle their font
and load it with `Font::from_bytes`.

## High-DPI

- **X11**: the scale is `Xft.dpi / 96`, read from the root window's
  `RESOURCE_MANAGER` property when the app starts, as GTK and Qt do. Desktops set
  it; to set it yourself, run `xrdb -merge <<< 'Xft.dpi: 192'`. Fractional values
  such as 144 (1.5x) work.
- **Wayland**: the buffer scale follows the integer scale of the output the window
  is on, and changes when the window moves or the output's scale changes. With
  fractional scaling, the compositor resamples the next integer scale.

## Appearance

`Theme::native()` reads `color-scheme` and `accent-color` from the XDG desktop
portal's settings (GNOME, KDE and others provide it). A `GTK_THEME` ending in
`:dark` also selects dark mode. With no portal, the theme is light. The portal
call gives up after 250 ms, so a missing portal can't stall startup.

Controls stay self-drawn: Linux has no platform UI framework to draw them.

## File dialogs

`open_file`, `save_file` and `pick_folder` use the portal's `FileChooser`, so they
work in Flatpak and Snap sandboxes and show the desktop's own picker. They need
`xdg-desktop-portal` and a backend such as `xdg-desktop-portal-gtk`.

## Debugging

- `STIPPLE_WAYLAND_DEBUG=1` logs Wayland events, sizes and buffer scales.
- `STIPPLE_X11_DEBUG=1` logs X11 events.

## Known gaps

- The system clipboard works on X11 only; Wayland copy and paste stay in the app.
