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
- **Moves like the platform**: the self-drawn widgets animate with the
  platform's durations and curves (Fluent, AppKit, UIKit, Material 3 or
  libadwaita; see [widgets.md](widgets.md#motion)).
- **Shows the OS's own controls**, on Windows, macOS and iOS: real ones in the
  window, live, with the platform's effects (see [Real controls](#real-controls)),
  and the OS's pictures of them wherever a live one can't show.

## OS-drawn controls

For each control, the OS's own UI framework draws its chrome (bezel, check mark,
switch, slider, text-field frame). In a window, most controls are the OS's real
ones ([below](#real-controls)); everywhere else (off screen, headless, in a
GPU render without a window, under an overlay or a transform) the OS draws a
picture of the control into a bitmap, which Stipple draws as an image, and
Stipple does layout, input, the caret and selection, and the labels.

| Control | Windows | macOS | iOS |
|---|---|---|---|
| `button`, `Button` (Primary, Secondary) | ✅ | ✅ | ✅ |
| `checkbox` | ✅ | ✅ | self-drawn ¹ |
| `radio` | ✅ | ✅ | self-drawn ¹ |
| `switch` | self-drawn ¹ | ✅ | ✅ |
| `slider` | ✅ | ✅ | ✅ |
| `progress_bar` | ✅ | ✅ | ✅ |
| `text_field`, `text_area` frames | ✅ | ✅ | ✅ |
| `select` button (as a push button) | ✅ | ✅ | ✅ |
| `number_input` field frame | ✅ | ✅ | ✅ |
| scroll bars (classic: track, thumb, arrows) | ✅ | ✅ ² | — ³ |

¹ The platform has no such control. ² A legacy `NSScroller`'s track and
knob, when *Show scroll bars* is *Always* (the overlay scrollers stay
Stipple's); AppKit's bars have no arrows. ³ iOS's scroll indicators aren't
a control the app can draw. Scroll bars are only ever pictures (never live
controls); see [Scroll bars](widgets.md#skins-and-the-native-theme).

Each control is drawn in its current state, so hover, pressed and focus look the
way the OS draws them. A change cross-fades from the old state's picture to the
new one on the theme's motion (see [widgets.md](widgets.md#motion)). Bitmaps are
cached by control, state and size, so the OS is only asked again when one of
those changes.

## Real controls

In a window on iOS, macOS and Windows, each OS-backed control is a real
`UIView`, `NSView` or child `HWND` over the control's box, live: the OS
draws it every frame with its own effects (iOS 26's and macOS 26's Liquid
Glass, a switch's slide, a button's press, vibrancy), handles its clicks
(on iOS a button's and a switch's touches stay the app's, below), and speaks
for it to screen readers. Stipple lays it out and
says what it shows; the app's code doesn't change: `button(..)`,
`switch(..)`, `slider(..)` still return Elements whose handlers run.

| Control | Windows | macOS | iOS |
|---|---|---|---|
| `button`, `Button` (Primary, Secondary) | `BUTTON` | `NSButton` | `UIButton` (glass on iOS 26) |
| `checkbox`, `radio` | `BUTTON` ¹ | `NSButton` | self-drawn |
| `switch` | self-drawn | `NSSwitch` | `UISwitch` |
| `slider` | `msctls_trackbar32` | `NSSlider` | `UISlider` |
| `progress_bar` | `msctls_progress32` | `NSProgressIndicator` | `UIProgressView` |
| `text_field`, `number_input`, `password_field` | `EDIT` ² | `NSTextField` ² | `UITextField` ² |
| `text_area` | multi-line `EDIT` ² | `NSTextView` ² | `UITextView` ² |
| `select` | `COMBOBOX` ³ | `NSPopUpButton` ³ | `UIButton` with a `UIMenu` ³ |

¹ A `BS_CHECKBOX` / `BS_RADIOBUTTON` over the whole row, showing the
label as its own text (in the app's text size; Stipple draws neither box
nor label), its check the app's state. In the dark appearance it stays a
picture: see [Windows](platforms/windows.md#os-drawn-controls). ² Editing the field's
state: see [Text fields](#text-fields). ³ The OS's pop-up
button, its title the chosen option: a tap or click opens the OS's own menu
or drop-down list (not Stipple's overlay), with the chosen option checked,
and a pick runs the element's handler for it (`Select::choose`). The app's
state stays the source of truth: the options and the choice come from the
element each frame, and a pick the app refuses goes back. It's read once,
as the OS control: named by the select's
[`a11y_label`](accessibility.md), if it has one, its value the choice. With
the keyboard focus (ring showing) it is the picture, and the keyboard opens
Stipple's list, as before. On iOS it has the regular buttons' look (iOS
26's glass) with the pop-up indicator, and the pick is a `UIAction` of its
`UIMenu`; on macOS the menu pops up over it, the choice under the pointer;
on Windows the wheel over a closed one scrolls what it's in rather than
changing the choice. On iOS its touches are the app's view's: a drag that
starts on it scrolls its list, and a tap opens its menu
(`performPrimaryAction`, iOS 17.4 and later; before that a select keeps its
own touches, so a drag on one doesn't scroll).

### How it works

It works like Flutter's platform views: a hybrid of Stipple's frame and the
OS's controls over it.

- **Planning, each frame** (`core::native_host`, platform-agnostic and
  unit-tested). After layout, scrolling and the frame's animations, the App
  walks the tree in paint order and decides which OS-drawn controls are live.
  Each live one is marked on its node: the painter leaves it (and its label,
  which is the OS control's title) out of the frame, and the window gets the
  list of controls to show, each a `NativeView`: its kind and value
  (checked, on, a slider's position), its frame in the window's logical
  pixels, its title, the name a screen reader reads, light or dark, and its
  direction.
- **Identity.** A control is known across frames by its element's
  [`key`](widgets.md), else its tap, drag or focus handle (as the
  [cross-fades](#on-the-gpu) know it), else its place in the tree, together
  with its kind. The window keeps one OS control per id: made the first time
  it's listed, updated when what it shows changes (a switch the app turned
  off slides off, with the OS's animation), destroyed when it's gone.
- **Placing.** Each frame puts the control at its laid-out box: in points on
  iOS and macOS (a subview of the app's view, above its Metal layer), in
  physical pixels on Windows (the frame times the window's DPI scale, the
  title in the app's text size at that DPI).
- **Events.** A tap or click (`UIControlEventTouchUpInside`, an `NSControl`
  action, `BN_CLICKED`) or a slider's move (`UIControlEventValueChanged`,
  `WM_HSCROLL`) comes back as `Event::NativeView`, and the App runs the
  handler of the element the control stands for: a button's tap handler, a
  checkbox's or switch's row's (the box is part of a labeled row), a
  slider's drag handler with the new position, as a tap or a drag there
  would; a select's pick (a `UIAction`, the `NSPopUpButton`'s action,
  `CBN_SELENDOK`) comes back as `NativeViewEvent::Picked` and runs the
  handler for that option. The state is the app's: the window is handed
  the list again after every event, and a control the user acted on is set
  to it even if it didn't change, so a switch the handler didn't turn on
  goes back off, and a pick it refused goes back, on the next frame. On iOS a button's, a switch's, a
  slider's and a select's touches are the app's view's, as a picture's
  would be: a drag that starts on one scrolls its list, a long press opens
  its context menu, an edge swipe goes back, and a tap is the element's tap
  (the button shows UIKit's pressed look while the app holds it,
  `NativeView::pressed`; a tap on a select opens its `UIMenu`,
  `Window::open_native_menu`). On a slider in a vertical list a drag up or
  down scrolls the list and a sideways one moves the slider (the app's drag
  handler, whose value the `UISlider` is then set to), as on a picture (see
  [pointer-events.md](pointer-events.md)). The OS controls don't take the keyboard focus
  (AppKit's refuse first responder, Win32's give it back): keys, Tab and the
  focus ring stay Stipple's.
- **Scroll areas.** Each scroll area that holds live controls is a
  clipping container of the OS's (`NativeView::clips`, outermost first):
  on iOS and macOS a `UIView` / `NSView` at the area's viewport that clips
  its subviews (`clipsToBounds`, `masksToBounds`), the area's controls in
  it, a nested area's container inside its outer one's; on Windows each
  control's window region (`SetWindowRgn`), the part of it its areas show,
  which Windows draws, hit-tests and clips the frame's painting to. So a
  control half scrolled out stays live, cut at the edge by the OS; one
  wholly out isn't shown. A container takes no input of its own: a touch
  or click on it but on none of its controls goes on to the app's view,
  which scrolls the area. Containers come and go with their areas, and a
  control moves into the container of the area it is in now. Scrolling
  moves the controls in the same pass that draws the frame, before it's
  presented; on iOS and macOS, while controls are live, a Metal layer
  presents with Core Animation's transaction (`presentsWithTransaction`),
  so the frame and the controls' new places show together.
- **Accessibility.** The OS control speaks for itself, and Stipple's tree
  doesn't say it again: the node it stands for (the button, or the
  checkbox's row) carries its id (`AccessNode::native_view`), and the bridge
  exposes the OS control in its place, named with that node's name (VoiceOver
  on iOS and macOS gets the `UISwitch` / `NSButton` in the view's elements;
  under UI Automation the window's provider puts the control's child window
  at the node, as one element with the node's role, name and Invoke).

### Text fields

A `text_field`, `number_input` or `password_field` is the OS's one-line
text control (`UITextField`, `NSTextField` or `NSSecureTextField`, an
`EDIT` child window), a `text_area` its multi-line one (`UITextView`, an
`NSTextView` in a scroll view, an `ES_MULTILINE` `EDIT`). While one has the
focus, typing, the caret, selection, the input methods' composition, the
clipboard, undo, the edit menu, the magnifier and the selection handles are
the OS's own. The app's code doesn't change: `text_field(cx, &state.name,
|s| &mut s.name)` still edits `state.name`, and its `on_change` and
`on_submit` handlers run.

- **The app's state is the truth.** What the user does in the control
  comes back as an edit (`NativeViewEvent::Edited`): the range of the text
  that was replaced, what replaced it, the new selection and the
  composition. The App applies it to the field as the keys it amounts to
  (`KeyInput::SetSelection`, `Text` or `Backspace`, `Preedit`), so the
  field's `max_length`, `read_only`, a number input's filter and its
  handlers apply as they do to typing in Stipple's own field. Each frame
  the control is told the field's state ([`NativeText`]: its text,
  selection, composition, hint, keyboard hints, read-only, invalid,
  focus), and set to it where it holds something else: a change the app
  refused or transformed (a length limit, an upper-cased code, a value set
  by the app) is put back with the caret where the app put it.
- **Nothing typed is lost.** Edits are numbered, and the frame says the last
  one the app took in: a control isn't set from a frame that hasn't seen
  every edit it sent (the user typed again before the app's frame came).
- **Composition.** The OS's marked text is the field's preedit: it shows in
  the field's state (`EditBuffer::preedit`) while it is composed, and a
  commit replaces it, as with Stipple's own input method support. On
  Windows the `EDIT`'s input method composes in a window of its own; its
  composition string (`ImmGetCompositionStringW`) is the preedit.
- **Focus** goes both ways. A click or tap into the control focuses the
  field (`NativeViewEvent::Focus`); the app focusing the field (Tab, a
  click on its label, `Next` from the field before) makes the control the
  first responder (or gives it the keyboard focus), and the app focusing
  something else takes it back to the app's view. Tab and Shift-Tab in the
  control move the app's focus on (on an iPad's hardware keyboard too: the
  app's view offers them as `UIKeyCommand`s with priority over the
  system's Tab while a live field is first responder), Return submits (or, with
  `EnterAction::Next`, moves on; a `text_area` keeps its new lines),
  Escape, and Up and Down in a one-line field (a number steps) go to the
  field as Stipple's would take them. On iOS the keyboard's dismiss key
  unfocuses the field. While a control has the focus, the window's own
  input method, on-screen keyboard and secure entry stay off.
- **Keyboard hints.** The field's `input_kind`, `autocapitalize`,
  `autocorrect`, `enter_action` and `autofill` are the `UITextField`'s
  traits (`keyboardType`, `returnKeyType`, `textContentType`...); a
  password field is `secureTextEntry`, an `NSSecureTextField`, an
  `ES_PASSWORD` `EDIT`. Its `placeholder` is the control's (an `EDIT`'s cue
  banner; a text view has none).
- **Accessibility.** The control speaks for itself (its value, caret and
  selection are the OS's), exposed once, named with the field's label: its
  `a11y_label`, else the text of what it's `labelled_by`, else its hint
  (`accessibilityLabel` on iOS and macOS, `IAccPropServices`'
  `PROPID_ACC_NAME` on an `EDIT`, which has no label of its own).

What stays Stipple's: a `password_field` with a reveal button (the control
would cover the eye), and every field off screen, in a window that hosts
no controls, and under a theme with `native_text_editing(false)`, which
keeps every field's editing Stipple's, the OS drawing only its frame (as
`examples/touchdemo` does: its self-tests drive Stipple's own text input).

**While it's edited, it stays live.** A focused field doesn't turn into a
picture because its focus ring shows (as a button does: the field being
edited is the OS's, focus look included), nor a `number_input` because its
text isn't a number in range yet (`-` on the way to `-3`): its frame turns
red (a red ring on iOS and macOS; Windows shows none) rather than turning
into Stipple's red frame mid-edit, and back to Stipple's once it loses the
focus. The other rules still apply: something drawn over it, a transform,
a fade, a clip that isn't a scroll area's, a page transition. Then it is a
picture, and its editing Stipple's, with the same state (text, selection,
focus; a composition in progress ends), and live again once that's over.

`examples/nativegallery`'s self-test types into its real field through the
control's own text input, checks the app's state, sets the state and
checks the control, composes in it (iOS and macOS) and presses Return (see
the platform pages for what CI checks).

[`NativeText`]: https://docs.rs/stipple/latest/stipple/platform/native/struct.NativeText.html


### When it's a picture instead

The OS's controls sit above everything Stipple draws, so a control is live
only where that's what the frame would show. It's the OS's picture of it (the
path off screen rendering takes, and that every control took before) for as
long as:

- something drawn after it overlaps it, or something that takes input does:
  an overlay, a menu, a dialog's scrim or a popup's click-catcher, a toast,
  a drag image, a scroll bar's track, a badge (only the part of it that
  shows counts: a row scrolled out of its area doesn't cover what's beside
  the area). An overlay scroll indicator's thumb alone (iOS's, macOS's
  overlay scrollers' while a list scrolls) passes under the live controls
  instead, rather than turning every row it crosses into a picture for as
  long as the list moves;
- it, or anything around it, is scaled, rotated or translucent: a page
  transition, a fade, a theme cross-fade (a plain move it follows);
- a clip that isn't a scroll area's (a rounded panel's, an overlay's) cuts
  part of it off (a scroll area's is the OS's: see *Scroll areas* above);
- it has the keyboard focus with the focus ring showing (the picture shows
  the OS's focus look), unless it is a text field: one being edited stays
  the OS's (see [Text fields](#text-fields));
- it's leaving (an exit animation) or off stage.

The switch is per control and per frame, and the two look the same, so it
doesn't show. On iOS a live button's, switch's, slider's and select's
touches are the app's view's, so a drag that starts on one scrolls as on a
picture (on a slider, a sideways drag moves it). On macOS a scroll wheel or
trackpad scroll over a control goes on to the app's view, and over a
slider a mostly vertical one is the app's alone (the list scrolls) and a
sideways one AppKit's (the slider moves), a trackpad gesture keeping the
way its first move went; on Windows a button passes the wheel to its
parent, and a closed select's and a slider's are passed on for them (a
slider's horizontal wheel moves it).

### Where it doesn't apply

Off screen (`App::render_once`, `render_frame`, the GPU renders of
`render_with`), headless and in tests there is no window, so every control is
a picture, as before, and pixel tests and screenshots are unchanged. Linux,
Android and the web keep Stipple's self-drawn controls. A window says whether
it hosts OS controls with `Window::hosts_native_views`, and gets them with
`Window::set_native_views`.

How each platform draws them, and its quirks, is in the platform guides:
[Windows](platforms/windows.md#os-drawn-controls), [macOS](platforms/macos.md#os-drawn-controls),
[iOS](platforms/ios.md#os-drawn-controls).

## Elsewhere

Linux has no platform UI framework, and Android's widgets need its Java framework,
so there `Theme::native()` keeps Stipple's self-drawn controls. On Linux it still
follows the system's dark mode and accent color, and uses libadwaita's colors.
On the web it follows the page's `prefers-color-scheme` and, where the browser
exposes one, the CSS `AccentColor` (see [the web page](platforms/web.md#theme)),
with the same colors as on Linux.

## Limits

- **Other variants**: `Variant::Ghost` and `Variant::Danger` buttons have no OS
  equivalent and stay self-drawn.
- **Buttons with more than a title**: a button whose content isn't a single
  line of text (an icon, two texts) stays a picture: the OS control shows a
  title.

## Following changes

When the user switches dark mode or the accent color, an `App` running
`Theme::native()` rebuilds it, drops the OS-drawn bitmaps it cached, switches the
title bar, and cross-fades to the new look (see
[animation.md](animation.md#theme-changes)). The backends hear about it from:

| Platform | Source |
|---|---|
| Windows | `WM_SETTINGCHANGE` (`ImmersiveColorSet`), `WM_DWMCOLORIZATIONCOLORCHANGED`, `WM_THEMECHANGED` |
| macOS | `AppleInterfaceThemeChangedNotification` and `AppleColorPreferencesChangedNotification` (distributed), `NSSystemColorsDidChangeNotification` |
| Linux | the desktop portal's `SettingChanged` signal for `color-scheme` and `accent-color`, heard on a thread of its own |
| Web | the `change` event of `matchMedia("(prefers-color-scheme: dark)")` |
| iOS | `traitCollectionDidChange:` on the view, when the screen's `userInterfaceStyle` changes (and on coming back to the foreground) |
| Android | the activity's configuration changes (night mode) |

The platform layer delivers
`Event::ThemeChanged` either way. A theme you customized after building it with
`Theme::native()` is replaced on a change; set `theme.follows_system = false` to
keep it.

## On the GPU

The native theme looks the same on every renderer. Live controls are the
OS's own views over the window, whichever renderer draws the frame under
them. Where a control is a picture, the OS draws it into a bitmap once per
control and value (checked, on, a slider's position), state (hovered,
pressed, focused), light or dark, size in physical pixels and scale, and
Stipple keeps those bitmaps. The software renderer copies
them into the frame. The GPU renderers (`Renderer::Gpu`, `App::render_with` with
`stipple::gpu::render_scene_scaled`; Metal, Direct3D 11, Vulkan, GLES) get
the same bitmaps as images, and their image cache keys textures by the
bitmap itself, so a control that doesn't change is uploaded once and then
drawn from its texture each frame. A hover or press brings up another
bitmap and so another texture, which is then kept too. Nothing about the
native theme is drawn on the CPU per frame.

The bitmaps, and with them their textures, are dropped when they can't be
drawn again: when the system's look changes (dark mode, accent color, the
OS theme), when a window's scale changes (another monitor's DPI), and on a
low-memory warning. The next frame asks the OS again. A texture whose
bitmap is dropped is freed on the next frame, and one not drawn for 120
frames is freed anyway (its bitmap stays cached, so drawing it again only
uploads it).

CI compares the two: `src/gpu/compare.rs` draws a screen of native controls
with the GPU and with the CPU (on Windows with the controls uxtheme draws,
on WARP), and `nativegallery --gpu-check` does the same for the whole
gallery on the main thread, which is where AppKit draws, on Metal and on
Direct3D 11. See [gpu.md](gpu.md).

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
