# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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

### Changed

- `App::run` requires `S: 'static` (the web keeps the app after `run` returns).
- `webdemo` no longer defines the `stipple_*` exports; `stipple::web_app!` does,
  with a different set (see `docs/platforms/web.md`).

- `core::Role` and `platform::A11yRole` have new variants; `AccessNode` has
  `selected` and `expanded`; `Element` and `LayoutNode` have a `semantics`
  field (`Semantics`: role, label, selected, expanded).
- `spinner` takes the `Cx`: `spinner(cx, &theme)`.

### Fixed

- Pointer hit-testing respects clipping, so content scrolled out of a scroll
  container can't be clicked where it isn't drawn.
- A press on an overlay's dismiss area no longer reaches the text fields,
  sliders or focusables underneath it.

## [0.0.2](https://github.com/KarpelesLab/stipple/compare/stipple-v0.0.1...stipple-v0.0.2) - 2026-11-07

### Fixed

- fix clippy chunks_exact_to_as_chunks lint (new in stable clippy)

### Other

- merge the toolkit into a single stipple crate
