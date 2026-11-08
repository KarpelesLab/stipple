# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
