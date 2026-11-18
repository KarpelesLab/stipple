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
UI Automation the window's provider places each child window at its node, in
the app's tree (`IRawElementProviderHwndOverride`): one element, with the
node's role, name and Invoke, where a client walking the tree (NVDA's object
navigation, its say-all) reaches it between its neighbors, instead of a
separate element ahead of the app's tree that the .NET client reads as a pane. Checkboxes and radio buttons stay pictures (a Win32 one shows and is
named by a text of its own, where Stipple draws the label beside the box).

A `select` is a `COMBOBOX` (`CBS_DROPDOWNLIST`, `DarkMode_CFD` in dark
mode): a click opens Windows' own drop-down list, and a pick
(`CBN_SELENDOK`, or `CBN_SELCHANGE` from the keyboard with the list closed)
runs the element's handler for that option; a pick the app refuses is set
back. Its closed height is fitted to the select's (the selection field's
item height), the wheel over a closed one scrolls what it's in rather than
changing the choice, and UI Automation reads it once, as the combo box
(the app's tree leaves its node out). Its name is the select's label,
given through Dynamic Annotation (`IAccPropServices::SetHwndPropStr`) for
the clients that read MSAA's annotations; the .NET UI Automation client
CI uses reads it as a pane named by its choice, as it reads the buttons'
child windows.

Text fields are `EDIT` child windows (`ES_AUTOHSCROLL`, `ES_PASSWORD` for a
password, the hint as the cue banner; a `text_area` `ES_MULTILINE` with
`ES_WANTRETURN` and a scroll bar; `DarkMode_CFD` in dark mode; see
[Text fields](../native-theme.md#text-fields)). Unlike the buttons they keep the
keyboard focus they take, so the message loop hands their keys to them: the
caret, selection, clipboard, undo, context menu and the input method are
Windows'. Each is subclassed: after a key, a character, a click, a paste or the
input method's composition (read with `ImmGetCompositionStringW`), what it holds
is read back (`\r\n` line ends as `\n`) and sent to the app as edits; Return,
Tab, Shift-Tab, Escape and Up and Down in a one-line field go to the app
instead, and Ctrl shortcuts other than the field's own (select all, copy, cut,
paste, undo) and function keys go to the window, for the app's menus. Focus
moving between the window and its fields isn't the window's focus coming or
going, and the window taking the focus back (activated again) hands it to the
field the app has focused. An `EDIT` has no label of its own: like a select's,
its name is the field's label (or hint), through Dynamic Annotation
(`PROPID_ACC_NAME`), which MSAA (and NVDA, which reads an `EDIT` through it)
gives. Under UI Automation the window puts the field's own node in the
`EDIT`'s place, as for the other controls: an edit with Stipple's value and
the hint as its help text, once.

A control in a scroll area is cut to the part the area shows by a window
region (`SetWindowRgn`), which Windows draws, hit-tests and clips the
window's own painting to: one half scrolled out stays live. (A region does
what a clipping container child window would, without moving the controls
into one and relaying their notifications and `WM_PRINTCLIENT`.)

CI's `visual-windows` job (`.github/scripts/native-views-windows.ps1`) runs
`nativegallery`'s self-test, clicks Save and picks Small in the select for
real (a click on the combo box, then one on Small in its drop-down list,
where UI Automation puts it), checks the half-scrolled rows are live
with their regions, types into the text field's `EDIT` for real (a click,
then keys) and checks MSAA names it by its hint.

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
