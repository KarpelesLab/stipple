# The native theme

Stipple's own themes (`Theme::light()`, `Theme::dark()`, Material 3, and any you
build) look the same on every platform, pixel for pixel. `Theme::native()` trades
that for the platform's look:

```rust
let app = App::new(state, view).theme(Theme::native());
```

## What it does

- **Follows the system's settings**: dark mode and accent color, read when
  `Theme::native()` is called and again whenever they change while the app runs.
- **Uses the platform's metrics**: font size, corner radius, and the colors of
  window backgrounds, surfaces and separators.
- **Has the OS draw the controls**, on Windows, macOS and iOS.

## OS-drawn controls

For each control, the OS's own UI framework draws its chrome (bezel, check mark,
switch, slider, text-field frame) into a bitmap, which Stipple draws as an image.
Stipple still does layout, input, the caret and selection, and the labels.

| Control | Windows | macOS | iOS |
|---|---|---|---|
| `button_labeled`, `button_variant` (Primary, Secondary) | ✅ | ✅ | ✅ |
| `checkbox` | ✅ | ✅ | self-drawn ¹ |
| `radio` | ✅ | ✅ | self-drawn ¹ |
| `switch` | self-drawn ¹ | ✅ | ✅ |
| `slider` | ✅ | ✅ | ✅ |
| `progress_bar` | ✅ | ✅ | ✅ |
| `text_field`, `text_editor`, `text_area` frames | ✅ | ✅ | ✅ |
| `select` button (as a push button) | ✅ | ✅ | ✅ |
| `number_input` field frame | ✅ | ✅ | ✅ |

¹ The platform has no such control.

Each control is drawn in its current state, so hover, pressed and focus look the
way the OS draws them. Bitmaps are cached by control, state and size, so the OS
is only asked again when one of those changes.

How each platform draws them, and its quirks, is in the platform guides:
[Windows](platforms/windows.md#os-drawn-controls), [macOS](platforms/macos.md#os-drawn-controls),
[iOS](platforms/ios.md#os-drawn-controls).

## Elsewhere

Linux has no platform UI framework, and Android's widgets need its Java framework,
so there `Theme::native()` keeps Stipple's self-drawn controls. On Linux it still
follows the system's dark mode and accent color, and uses libadwaita's colors.

## Limits

- **GPU rendering**: the GPU backends can't draw bitmaps yet, so under
  `App::render_with` the controls' chrome is missing and their labels sit on the
  background.
- **Other variants**: `Variant::Ghost` and `Variant::Danger` buttons have no OS
  equivalent and stay self-drawn.

## Following changes

When the user switches dark mode or the accent color, an `App` running
`Theme::native()` rebuilds it, drops the OS-drawn bitmaps it cached, switches the
title bar, and repaints. The backends hear about it from:

| Platform | Source |
|---|---|
| Windows | `WM_SETTINGCHANGE` (`ImmersiveColorSet`), `WM_DWMCOLORIZATIONCOLORCHANGED`, `WM_THEMECHANGED` |
| macOS | `AppleInterfaceThemeChangedNotification` and `AppleColorPreferencesChangedNotification` (distributed), `NSSystemColorsDidChangeNotification` |
| Linux | the desktop portal's `SettingChanged` signal for `color-scheme` and `accent-color`, heard on a thread of its own |

iOS doesn't report changes yet. The platform layer delivers
`Event::ThemeChanged` either way. A theme you customized after building it with
`Theme::native()` is replaced on a change; set `theme.follows_system = false` to
keep it.

## Using it in your own widgets

`Theme::native_deco(control)` returns how the OS should draw a control under the
current theme, or `None` when the widget should draw itself. Attach it with
`Element::native`, keeping the element's own decoration as the fallback:

```rust
let control = NativeControl::Checkbox { checked };
let mut el = Element::boxed(my_fallback_style);
if let Some(deco) = theme.native_deco(control) {
    el = el.native(deco);
}
```
