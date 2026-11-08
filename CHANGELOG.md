# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.3](https://github.com/KarpelesLab/stipple/compare/stipple-v0.0.2...stipple-v0.0.3) - 2026-11-08

### Fixed

- fix wrapped-text sizing, Wayland resize and the macOS title; publish screenshots

### Other

- *(windows)* report the OLE drop without failing the job, with progress lines
- a click that only moves focus repaints; touchdemo waits for the keyboard
- *(windows)* fix the OLE drop script's compile and exit code
- *(windows)* drive the OLE drag with synthesized mouse input
- a click or tap on a plain text field focuses it
- *(windows)* keep the OLE drag loop turning and bound it in time
- *(web)* the two-app screenshot is for the record; the selftest checks it renders
- *(ios)* print the input part of the app's debug log and upload the logs
- give the emulator 90 s to type a word
- chain the iOS self-test's steps; log the app's debug output in CI
- real-time multi-app web screenshot; timestamped debug logs and a patient widgets check
- cache shaped runs per font, by text and size
- *(scroll)* wait for wheel scrolling to settle before the mouse-drag check
- drag and drop
- drag and drop on X11 (XDND), Windows (OLE) and macOS (dragging methods)
- windows, macos: receive drops
- receive drops through wl_data_device
- receive drops over XDND; dropdemo example and an XDND source script
- drag-and-drop events, drop targets with readable drag-over state, in-app drags
- touch scrolling and the iOS keyboard
- android, ios: report every finger, and the iOS keyboard (UIKeyInput)
- Event::Touch for every finger, and drag-to-scroll with fling in the toolkit
- tray icons, notifications and opening URLs
- traydemo, and CI checks for the tray, notifications and open_url
- os integration: open_url, notifications and tray icons
- mirrored right-to-left layouts
- a mirrored UI (--mirror), checked in CI on X11, Windows and macOS
- mirrored right-to-left layouts (LayoutDirection)
- *(menus)* native context menus, mnemonic underlines, how menus are tested
- *(menus)* drive the native menus on Windows and macOS, check Alt cues on X11
- native context menus on macOS and Windows, underlined mnemonics
- a looser spinner frame count, for slow headless Chrome in CI
- web accessibility, input methods, theme and several apps
- webdemo, ci(web): check IME, ARIA, the color scheme and a second app
- ARIA overlay, IME and dead keys, prefers-color-scheme, several apps per page
- checkbox and switch roles, a checked state, and every role in the platform tree
- Phase 8, a rich animated reactive UI
- update screenshots from CI
- *(web)* run the selftest in real time and read its report from the server log
- *(web)* a 60 s virtual-time budget for the selftest
- poll the connection even when a wake is due
- *(menus)* open File with Alt+F, since the runner's xdotool sends F10 with Alt held
- update screenshots from CI
- report web selftest errors, parse newer compare output, log App presses and presents
- a real NativeActivity backend
- menudemo, a Menus X11 visual check and docs/menus.md
- a declarative menu model, native menu bars on macOS and Windows, a drawn one elsewhere, and context menus
- docs, ci: input methods, dead keys and compose
- NSTextInputClient on the view for input methods and dead keys
- IMM32 input methods with inline composition
- preedit and commit events, inline composition, and compose sequences on X11 and Wayland
- bididemo, visual-bidi CI job and docs for bidirectional text
- right-to-left and bidirectional layout and editing
- update screenshots from CI
- the web backend, and what works where
- a real backend, with exports in the library
- update screenshots from CI
- ask only for more frames than the timer builds
- *(x11)* diagnose the widgets demo by its own pid
- understand keysyms named by value; asyncdemo: start the selftest's work once the loop runs
- fix a timing-dependent hover test, relax the spinner frame count, log input
- select, number input, split pane, toolbar and toast
- display-driven frame clock and a turning spinner
- virtualized lists, table and tree
- size limits, maximize, fullscreen, icon and cursor shapes
- wayland clipboard, and following theme changes live
- update screenshots from CI
- image and svg
- grid, wrapping and min/max constraints
- scroll, modifiers and more keys on Wayland, Windows and macOS
- text editing: undo/redo, word movement, double-click selection
- clipboard and file dialogs on Windows and macOS
- update screenshots from CI
- add AppHandle: timers and background work wake the event loop
- update screenshots from CI
- *(web)* install ImageMagick for the canvas check
- record platform-fidelity work; plan Phase 7, app-ready parity
- update screenshots from CI
- make the repository root the stipple crate; the wasm target is an example
- add platform guides, the native theme, high-DPI and CI
- make the repository root the stipple crate; the wasm target is an example
- run full-screen, and fill the safe area with the theme's background
- update screenshots from CI
- render at the display's scale; center text in its line
- render at full resolution on high-DPI macOS and Windows
- fill the screen at full resolution, add touch input; fix self-drawn widgets
- update screenshots from CI
- *(windows)* pass a real null class name to FindWindowW
- add Theme::native: OS-drawn controls on Windows, macOS and iOS
- *(windows)* find the window by title; a hidden start hid the app too
- remove stray empty .log files and ignore *.log
- *(readme)* add CI, crates.io, docs.rs and license badges
- fix intra-doc links left over from the crate merge

### Added

- Native context menus on macOS (`NSMenu`) and Windows (`TrackPopupMenuEx`):
  `Element::context_menu` shows the system's popup there, through the new
  `Window::popup_menu` and `Menu::to_native` / `Menu::commands`; Linux, and
  `menu_bar_in_window(true)`, keep the drawn one.
- The drawn menus underline mnemonics while the keyboard drives them (Alt held,
  F10, Alt+letter, a key in an open menu), as Windows does, through the new
  `Element::underline` and `menu::mnemonic_at`.
- CI drives the native menus: on Windows from PowerShell (the `HMENU` read back,
  `WM_COMMAND`, Alt+E plus a mnemonic, Ctrl+D, the native context menu), on
  macOS through an in-process self-test (`STIPPLE_COCOA_MENU_SELFTEST`).
- `menudemo` logs each command it runs and has a note field and an Edit >
  Copy Summary item on Ctrl+C / ⌘C.

- Tray icons, notifications and opening URLs. `App::tray` declares a tray
  icon (`Tray`: tooltip, icon, a `Menu`, a click action) as a function of the
  state: a notification-area icon on Windows, an `NSStatusItem` on macOS, and a
  `StatusNotifierItem` with a dbusmenu menu over D-Bus on Linux; picks arrive
  as `Event::MenuCommand`, clicks as the new `Event::TrayActivated`, and
  backends implement `Window::set_tray`. `platform::notify` (and
  `AppHandle::notify`) shows a desktop notification, and `platform::open_url` /
  `open_path` (and `AppHandle::open_url`) open a URL or file in its default app,
  through the XDG portal or `xdg-open`, `ShellExecuteW`, `NSWorkspace`,
  `UIApplication`, an Android intent or `window.open`. `web/stipple.js` gains
  the `open_url` and `notify` imports (and `openUrl` / `notify` options). See
  `docs/os-integration.md` and `examples/traydemo`, which CI runs on Linux
  (against a mock desktop on D-Bus), Windows and macOS.

- Touch scrolling on Android and iOS: a finger dragged in a scroll container
  scrolls it once past an 8 px slop (taps still click, and the press is
  cancelled when it scrolls), flings on when it lifts while moving, stops at
  the ends, and a touch stops a fling. Every finger is reported as the new
  `Event::Touch` (`Touch { id, phase, position }`, `TouchPhase`); the first is
  still the pointer, so mice, wheels and existing widgets are unchanged.
  `App::touch` and `App::advance_fling` drive it in tests.
- The iOS keyboard: the view adopts `UIKeyInput` and becomes first responder
  (showing the on-screen keyboard) while a text input has focus;
  `insertText:` types, `deleteBackward` deletes, and a hardware keyboard's
  arrows, Escape, Tab, F-keys and Ctrl/Cmd shortcuts arrive as key events.
  `backend::ios::simulate` drives the view for simulator self-tests.
- `touchdemo`: a text field over a list a finger scrolls, with a self-test the
  iOS simulator job runs; `androiddemo` gains a list the emulator job swipes.
- Drag and drop. Files and text dropped from other applications arrive as
  `Event::DragEnter` / `DragOver` / `DragLeave` / `Drop` (`DragOffer`,
  `DropData`) over XDND on X11, `wl_data_device` on Wayland, OLE
  `IDropTarget` and `WM_DROPFILES` on Windows and `NSDraggingDestination` on
  macOS. `Element::on_drop` and `on_drop_of` make drop targets; views read a
  target's `DropState` (over, accepted, pointer position and an eased level)
  with `Cx::register_drop` + `Cx::drop_state` + `Element::drop_target` to
  animate it, or get the app's highlight; `Element::drag_source` drags items
  inside the app. `examples/dropdemo` shows an animated drop zone. See
  `docs/drag-and-drop.md`.
- The web catches up with the desktops: `stipple::web_app!(app)` generates a
  web app's exports and `web/stipple.js` is its whole page side, so an app no
  longer copies the glue. Timers and `AppHandle::update` wake the app through
  `setTimeout`, `requestAnimationFrame` drives animations (and stops when
  idle), `devicePixelRatio` sets the scale, the canvas follows resizes, and
  keys, modifiers, the wheel, cursor shapes and the clipboard (through paste
  and copy events) work. See `docs/platforms/web.md`.
- `dialog::open_file_contents` and `dialog::save_file_contents`: open a file's
  name and bytes through a callback, and save bytes, on every platform, the web
  included (an `<input type=file>` and a download). `FileDialog` gains
  `file_name` and `accept`.
- `stipple::platform::time::Instant`: std's `Instant` natively, and a
  `performance.now()` clock on the web, where std's panics.
- `webdemo` shows a spinner, timers, a text editor and file buttons, and its
  `index.html#selftest` checks them in headless Chrome in CI.
- Accessibility on the web: `web/stipple.js` mirrors the accessibility tree as
  ARIA elements in an invisible overlay over the canvas; clicking one (a
  screen reader's "activate") taps its node, and focus is kept in step both
  ways.
- Input methods and dead keys on the web, through a hidden `<textarea>` at the
  caret: composition events become `Ime::Preedit` and `Ime::Commit`.
- `Theme::native()` on the web follows `prefers-color-scheme` (live, as
  `Event::ThemeChanged`) and the CSS `AccentColor` where the browser has one.
- Several apps on one page: each `start` call in `web/stipple.js` runs its own
  instance of the module (compiled once); `start` takes `label` and
  `title: false`. `examples/webdemo/multi.html` runs two.
- `Role::CheckBox`, `Role::Switch` and `Element::checked`: checkboxes and
  switches report their state to assistive technologies (`AccessNode::checked`).
  The platform tree (`A11yNode`) carries `selected`, `expanded` and `checked`
  and names every role; the desktop bridges report the new ones as before.

- Right-to-left and bidirectional text, through `oxideav-scribe`'s Unicode
  Bidirectional Algorithm: labels, paragraphs and editors lay out Hebrew,
  Arabic and mixed lines in visual order, with the paragraph direction detected
  from the first strong character or set with `Element::direction` /
  `TextDirection`; right-to-left paragraphs align right and wrap correctly.
  In `EditBuffer`, Left and Right move visually, selections in mixed text show
  as several rectangles, and clicks map to logical positions. New
  `Font::layout` / `TextLayout` (caret positions, hit-testing, selection
  rectangles) and `Scene::fill_layout`. See `docs/bidi.md` and
  `examples/bididemo`.
- Mirrored layouts for right-to-left UIs: `Theme::with_layout_direction`
  (`LayoutDirection::Ltr` by default, `LayoutDirection::from_env` and
  `for_language` to follow a locale) and `Element::layout_direction` for a
  subtree. Right to left, rows run from the right, horizontal `Align::Start`
  is the right edge, grid columns count from the right, the `left` padding
  inset lands on the right, and `Auto` text lines up on the right
  (`Font::layout_in`). Sliders and progress bars fill from the right and drags
  on mirrored elements are mirrored (`DragMotion::mirrored`); tree triangles
  and submenu arrows point left; Left/Right swap in trees, split panes and
  menus; popups, submenus, dropdowns and toasts open from the right. New
  `layout_with_direction`, `LayoutNode::rtl`, `Span::mirrored`,
  `mirror_spans` and `Insets::mirrored`. `drag_hit_at` also returns whether
  the element was mirrored. `bididemo --mirror` shows a mirrored UI, checked in
  CI on X11, Windows and macOS. See `docs/bidi.md`.
- Menus: `App::menu` declares a menu bar (`MenuBar`, `Menu`, `MenuItem`) as a
  function of the state, with shortcuts (`Shortcut`), enabled and checked
  states, separators and submenus; `Element::context_menu` attaches a context
  menu. macOS shows a native `NSMenu` menu bar with the standard application
  menu, Windows a native `HMENU`, and Linux a menu bar drawn in the window with
  keyboard navigation. See `docs/menus.md` and `examples/menudemo`.
- `Role::MenuBar`, `Role::Menu` and `Role::MenuItem`, mapped by the
  accessibility bridges; `KeyCode::Alt`, `KeyCode::ContextMenu`,
  `Event::MenuCommand`, `Window::set_menu_bar`, `Anchor::Popup` and
  `Anchor::RightOf`; `App::press_key_code`, `App::release_key_code`,
  `App::right_click_at` and `App::menu_command` for scripted input.
- A real Android backend: `android::start` runs the app's `main` on a UI thread
  from `ANativeActivity_onCreate`, and `App::run` drives it from an `ALooper`
  through the whole `NativeActivity` lifecycle (window created, resized and
  destroyed, focus, pause and resume, configuration changes, destroy). Touch
  and mouse input, hardware keys and the soft keyboard, `AppHandle` wakeups and
  timers, `AChoreographer` frames, the screen density as the scale factor,
  night mode and the Android 12 accent color (followed live), system bars in the
  app's colors, fullscreen, and the clipboard through JNI. `androiddemo` is now
  a full app, and CI drives it on an emulator (tap, typing, timers) and uploads
  screenshots. See `docs/platforms/android.md`.
- `Window::set_text_input_active`, which the app calls with whether a text
  input has focus, so touch platforms can show their on-screen keyboard.

- System clipboard on Wayland, through `wl_data_device`.
- Following dark mode and accent-color changes while the app runs, on Windows,
  macOS and Linux: `Event::ThemeChanged`, `Window::set_dark` and
  `Theme::follows_system`.
- window control: minimum and maximum size, maximize, fullscreen and the window
  icon (`App::min_size` & co., `WindowAttributes::with_*`, `AppHandle::window`)
  on X11, Wayland, Windows and macOS
- mouse cursor shapes (`CursorIcon`, `Element::cursor`): I-beam over text
  inputs, pointing hand over tap and drag targets, resize arrows and more
- Virtualized lists (`virtual_list`), tables (`table`: sortable, resizable
  columns, multi-selection, striped rows) and trees (`tree`), building only the
  rows on screen; see `docs/lists-and-tables.md` and `examples/tabledemo`.
- Accessibility roles for lists, tables, rows, cells, column headers, trees and
  tree items, with selected/expanded state; `Element::role`, `.selected()`,
  `.expanded()`.
- `Cx::scroll_offset` and `Cx::scroll_requests`, so a view can read a scroll
  container's offset and a handler can scroll it.
- `stipple::core::pointer()`: the pointer position, drag origin and the modifier
  keys held at the last button press (for Shift/Ctrl+click).
- A display-driven frame clock: `Cx::request_frame` and `Cx::frame_time` animate
  at the refresh rate (`wl_surface.frame`, `DwmFlush`, `CVDisplayLink`,
  `CADisplayLink`, a RandR-rate timer on X11) and stop when idle.
  `App::animating` reports whether a frame asked for another.
- The spinner turns, and hover and press highlights fade in and out.
- `asyncdemo` shows a spinner while its background work runs; its `--selftest`
  checks the spinner animated and then stopped.
- Widgets: `select` (a dropdown list, with keyboard navigation), `number_input`
  (steppers, min/max/step), `split_pane` (a draggable divider), `toolbar`, and
  `toasts` (with auto-dismiss on a timer). See `docs/widgets.md` and
  `examples/widgetsdemo`.
- Accessibility roles `ComboBox`, `SpinButton`, `Splitter`, `Toolbar`,
  `Separator` and `Alert`, and `Element::a11y_label` to name an element that
  has no text (an icon button).
- `Element::on_drag_motion`: drags that receive the pointer position, the press
  origin, and the element's and its parent's bounds.
- `Anchor::Below` (an overlay dropped under an element) and `Anchor::Aligned`
  (an overlay against the window's edges).
- Input methods (IME) for Chinese, Japanese and Korean: `Event::Ime` (preedit,
  commit, delete-surrounding), shown inline and underlined in `text_editor` and
  `text_area` without entering the undo history until committed;
  `Window::set_ime` passes the caret rectangle and surrounding text to
  `zwp_text_input_v3` on Wayland, IMM32 on Windows and `NSTextInputClient` on
  macOS. See `docs/ime.md`.
- Dead keys and compose sequences on X11 and Wayland, from the libX11 Compose
  file for the locale (`XCOMPOSEFILE`, `~/.XCompose`), with the pending
  sequence shown as preedit. Both backends also type legacy keysyms such as
  `eacute` or `Cyrillic_a`.
- `EditBuffer::set_preedit`, `preedit`, `clear_preedit`, `delete_surrounding`,
  `display` and `display_to_text`; `Element::preedit`.

### Changed

- `App::run` requires `S: 'static` (the web keeps the app after `run` returns).
- `webdemo` no longer defines the `stipple_*` exports; `stipple::web_app!` does,
  with a different set (see `docs/platforms/web.md`).

- `core::Role` and `platform::A11yRole` have new variants; `AccessNode` has
  `selected` and `expanded`; `Element` and `LayoutNode` have a `semantics`
  field (`Semantics`: role, label, selected, expanded).
- `spinner` takes the `Cx`: `spinner(cx, &theme)`.
- `KeyInput` has `Preedit` and `DeleteSurrounding` variants; `Element` and
  `LayoutNode` have a `preedit` field.
- Alt+letter now arrives as a `KeyCode::Char` shortcut on X11, Wayland and
  Windows instead of typing the letter. AltGr still types.
- `App` methods now need the state type to be `'static`.

### Fixed

- On macOS, a shortcut whose key AppKit and Stipple read differently (⌘⌫, whose
  key equivalent is a backspace while the key types a delete) no longer runs
  twice: the backend now drops AppKit's key-equivalent pick whenever Stipple's
  own matching took the key, instead of comparing the characters.
- Pointer hit-testing respects clipping, so content scrolled out of a scroll
  container can't be clicked where it isn't drawn.
- A press on an overlay's dismiss area no longer reaches the text fields,
  sliders or focusables underneath it.
- X11 follows keyboard mapping changes (`MappingNotify`) instead of keeping the
  keymap read at startup.
- Windows delivers characters outside the Basic Multilingual Plane (emoji, rare
  CJK ideographs), which `WM_CHAR` sends as two surrogates.
- A press inside an overlay (a gap in a select's list) no longer falls through
  to its press-outside catcher and closes it, and hovering outside an open
  overlay no longer tints the whole window.

## [0.0.2](https://github.com/KarpelesLab/stipple/compare/stipple-v0.0.1...stipple-v0.0.2) - 2026-11-07

### Fixed

- fix clippy chunks_exact_to_as_chunks lint (new in stable clippy)

### Other

- merge the toolkit into a single stipple crate
