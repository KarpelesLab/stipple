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
a fallback for glyphs the main font lacks. Japanese, Chinese and Korean text
draws from Noto Sans CJK (`fonts-noto-cjk`) or another installed CJK family,
loaded the first time text needs it (see [fonts](../fonts.md#fallbacks)).
Shipping apps should bundle their font and load it with `Font::from_bytes`.

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
call gives up after 250 ms, so a missing portal can't stall startup. With
`Theme::native()`, a thread listens for the portal's `SettingChanged` signal, so
switching dark mode or the accent color restyles the app while it runs.

Controls stay self-drawn: Linux has no platform UI framework to draw them.

## Clipboard

Both backends carry text, HTML (`text/html`) and images (`image/png` written;
PNG, BMP, JPEG, GIF and WebP read), and the **primary selection**: a text
field's selection becomes it, and a middle click on editable text pastes it at
the click (see [text-editing.md](../text-editing.md#the-clipboard)).

- **Wayland**: copy and paste go through `wl_data_device`, offering and accepting
  `text/plain;charset=utf-8`, `UTF8_STRING`, `text/plain` and the rich types. A
  paste reads the selection through a pipe and gives up after a second; the
  compositor's events are queued meanwhile, so the window keeps answering. Only
  the window with keyboard focus sees the selection, as the protocol requires.
  The primary selection uses `zwp_primary_selection_device_manager_v1` when the
  compositor has it (sway, KWin, Mutter do).
- **X11**: copying owns the `CLIPBOARD` (or `PRIMARY`) selection and answers
  `SelectionRequest` for `TARGETS`, the text types (`UTF8_STRING`, `STRING`,
  …), `text/html` and `image/png`. Pasting sends `ConvertSelection` (`TARGETS`
  first, then the best type per format; `UTF8_STRING`, then `STRING`, for an
  owner without `TARGETS`) and never blocks the loop: the answer comes back as
  `SelectionNotify` and reaches the app as `Event::Clipboard`, which the `App`
  pastes. Data bigger than one request travels `INCR`, in both directions. A
  read whose owner stays silent ends after two seconds.

## File dialogs

`open_file`, `save_file` and `pick_folder` use the portal's `FileChooser`, so they
work in Flatpak and Snap sandboxes and show the desktop's own picker. They need
`xdg-desktop-portal` and a backend such as `xdg-desktop-portal-gtk`.

## Rendering on the GPU

With `Renderer::Gpu` (or `STIPPLE_RENDERER=vulkan` / `gl`), frames are drawn
with Vulkan or OpenGL ES and go straight to the window: a Vulkan swapchain or
an EGL window surface on X11 (over a libxcb connection of the GPU's own), and
on Wayland `dma-buf`s attached through `zwp_linux_dmabuf_v1` on the backend's
own connection (the compositor's formats and modifiers from its feedback, a
few buffers taken back with `wl_buffer.release`, each frame's damage, frame
callbacks pacing them). The renderer picks the GPU the compositor names as
its main device. Where the compositor has `wp_linux_drm_syncobj_v1`, frames
go with explicit sync: the GPU's fence as each commit's acquire point,
buffers free once their release point signals, no wait for the GPU before
the commit. Under a title bar Stipple draws (no `xdg-decoration`: GNOME,
Weston) the frames go in a subsurface, the decorations in the window's own
surface around it. A compositor without `zwp_linux_dmabuf_v1`, or a driver
that can't export what the compositor takes, gets the GPU's frames read back
and presented through `wl_shm`. See [gpu.md](../gpu.md#wayland).

## System libraries

Nothing is linked but the C library: the libraries a feature needs are
loaded at run time, through the [`dlsym`](https://crates.io/crates/dlsym)
crate (whose symbols can't outlive their library), and a program without
them still starts and falls back:

| Library | For | Without it |
|---|---|---|
| `libasound.so.2` | audio output (ALSA's `default` device, so PulseAudio or PipeWire) | the null output |
| `libvulkan.so.1` | the Vulkan renderer (`vk`) | GLES, or the CPU |
| `libxcb.so.1` | GPU frames presented to an X11 window | frames read back and presented as CPU ones |
| `libva` | H.264 decode (`video`, through oxideav-vaapi) | software decode where there is one |

(The `gl` feature, EGL and GLES, links `libEGL` and `libGLESv2`.)

## Static builds

A program built for `x86_64-unknown-linux-musl` with a static C library
runs anywhere, but a static musl can't load libraries: it plays through the
null output and renders on the CPU, on X11 or Wayland as usual.

[fullrust](https://github.com/KarpelesLab/fullrust)'s
`x86_64-unknown-linux-fullrust` target builds a static binary with no C
library at all, and Stipple works there as on Linux: the X11 and Wayland
backends, the clipboard, drag and drop, AT-SPI and the portals speak their
protocols over sockets, and the few C functions they call (`poll`, `mmap`,
`sendmsg`, System V shared memory...) are system calls
(`src/platform/fullrust.rs`). Libraries load through the system's own
`ld.so`, which `dlsym` boots on first use (through
[dldota](https://crates.io/crates/dldota)): **ALSA plays through the system's
libasound** (and its PulseAudio or PipeWire plugin), as in a normal build.
Calls into a loaded library are serialized (one thread at a time runs its
code), so the audio thread's blocking writes hold the others off briefly.

```sh
# With the toolchain image (or its GitHub action, KarpelesLab/fullrust):
docker run --rm -v "$PWD":/src ghcr.io/karpeleslab/fullrust:latest \
    build --release --example audiodemo --features audio
```

What isn't there in a fullrust build:

- **The GPU renderers.** `gl` links EGL, which a static binary can't, and
  Vulkan hands back function pointers (`vkGetInstanceProcAddr`) that must
  each be wrapped to switch the C library's thread pointer, which dlsym 0.1
  can't do for pointers it didn't look up. Builds with `gpu`, `vk` or `gl`
  compile, and render on the CPU (`GPU render unavailable ... using
  software`).
- **VA-API video decode**: oxideav-vaapi loads libva with the C library's
  `dlopen`. Video decodes in software where it can.
- **PulseAudio's ALSA plugin.** Every thread of ours calls into the loaded
  libraries under one borrowed thread identity (dldota 0.1), and libpulse's
  mutexes are priority-inheriting, which the kernel checks against the real
  thread: libpulse aborts (`pa_mutex_unlock`). So where PulseAudio itself
  is the sound server (its socket, and no PipeWire one), a fullrust build
  plays through the null output rather than opening ALSA. PipeWire's ALSA
  plugin (the default on current desktops) and hardware devices play. (A
  `default` pointed at the pulse plugin on a PipeWire system still aborts:
  set `STIPPLE_AUDIO=null` there.)
- The UTC offset comes from the system libc's `localtime_r` (loaded like the
  rest); with no system loader at all, times are UTC.

CI builds the examples for fullrust and runs `audiodemo --selftest`
(through ALSA's dummy driver), the todo selftest and the X11 window
checks under Xvfb, and a static musl build of the same (see
[ci.md](../ci.md)).

## Debugging

- `STIPPLE_WAYLAND_DEBUG=1` logs Wayland events, sizes and buffer scales
  (and the `dma-buf` formats and commits of GPU frames).
- `STIPPLE_WAYLAND_DMABUF=0` reads GPU frames back instead of handing the
  compositor `dma-buf`s.
- `STIPPLE_WAYLAND_SYNCOBJ=0` keeps `dma-buf` frames off explicit sync (the
  renderer waits for the GPU before each commit).
- `STIPPLE_X11_DEBUG=1` logs X11 events.
- `STIPPLE_COMPOSE_DEBUG=1` reports the Compose table when it loads.

## URLs that open the app

A `.desktop` file with `MimeType=x-scheme-handler/myapp;` and `Exec=app %u`
(made the default with `xdg-mime default app.desktop x-scheme-handler/myapp`)
starts the app with the URL as its argument, which `App::on_open_url` hears
as the loop starts. A URL while the app runs starts a second process, which
a [single instance](../os-integration.md#one-instance-reopening-and-the-dock)
hands to the running one. See
[os-integration.md](../os-integration.md#urls-that-open-the-app).

## Packaging and D-Bus activation

`cargo stipple linux` stages the app under a prefix with its `.desktop`
file and hicolor icons, and a tarball of it ([packaging.md](../packaging.md#linux)).
With `linux.dbus-activatable = true` in the manifest and
`App::dbus_activatable(true)` in the app, the `.desktop` file says
`DBusActivatable=true` and a session bus `.service` file lets the bus start
the app: desktops call its `org.freedesktop.Application` (`Activate`,
`Open`, `ActivateAction`) with the launch's activation token, as for a
GApplication.

## Known gaps

- Typed text comes from a simple reading of the keymap: Shift (and Caps Lock, on
  Wayland) pick the second level, but AltGr levels aren't applied yet. Dead keys
  and compose sequences work, from the locale's Compose file (see
  [ime.md](../ime.md)).
- Input methods (fcitx5, IBus) are wired up on Wayland through `text-input-v3`
  (not yet tried with a real IME in CI), but not yet on X11.
