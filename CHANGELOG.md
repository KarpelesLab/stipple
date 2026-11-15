# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Packaged assets (#59): `platform::assets()` reads files the app was
  packaged with by relative path (`read`, `open` as a `Read + Seek` reader,
  `exists`, `list`, and the `image`, `svg`, `font`, `sound` and `text`
  loaders): the APK's `assets/` through the `AAssetManager` on Android, the
  bundle's `assets/` on iOS and macOS, `assets/` next to the executable on
  Windows and Linux, `$STIPPLE_ASSETS`, and the directory given to
  `platform::set_assets_dir` (the source tree's, for `cargo run`). On the web
  `stipple.js` fetches them from its new `assets` URL, preloading its
  `preload` list for the blocking calls. `Assets::load` and
  `AppHandle::load_asset(path, decode, apply)` read and decode on worker
  threads (on the web, once the fetch is done) and hand the result to the
  state. `build-apk.sh` packs `examples/androiddemo/assets/`, the site serves
  and preloads each example's `assets/`, and `imagedemo` loads its PNG and
  SVG this way instead of `include_bytes!`.
- Nine-slice images (#60): `NineSlice::new(image, insets)` with `.region`
  (a part of an atlas), `.edges` / `.center` (`SliceFill::Stretch`, `Tile`,
  `Round`, `Hidden`), `.scale` (pixel art, hi-DPI), `.nearest()`, `.tint`,
  `.opacity` and `.padding`, drawn by `Scene::draw_nine_slice(dst, &slice)`:
  corners at their size, pieces snapped to physical pixels, on the software
  blit's fast paths and as quads on the GPU.
- `Element::background_image(...)`: a `NineSlice` frame, or a picture placed
  per an `ImageFit` (`BackgroundImage::image`), behind any element's content;
  the padding grows to the frame's insets, and per-state overrides
  (`.hovered(|e| e.background_image(&lit))`) swap it. An element with a
  background image gets no theme hover highlight.
- Skins (#60): `style::Skin` (buttons per state with `StateSkin`, panels,
  the dialogue box, bars with `BarSkin`, checkboxes and switches with
  `ToggleSkin`) on `Theme::skin` / `GameStyle::skin` dresses `button`,
  `menu_button`, `panel`, `dialog`, `pause_menu`, `GameOverScreen`,
  `DialogueBox`, `stat_bar` / `health_bar`, `progress_bar`, `checkbox` and
  `switch` in its frames. `gameui` has a "Painted" look ("Look" on its title menu).

- `NavStack` and `nav_stack` (#40): a stack of pages the app pushes and pops,
  the covered ones kept alive off stage, moving the platform's way
  (`PageTransition::Slide` under `Theme::native()` on iOS, `SharedAxis`
  under the Material motion, `Fade` elsewhere), the top page following iOS's
  edge swipe and Android's predictive back gesture; it takes Back while it
  can pop. `app_bar(cx, title)`: a back button, the title and actions, in
  Material's or `UINavigationBar`'s proportions.
- `BottomSheet` and `bottom_sheet` (#42): a modal sheet from the bottom edge
  on `SheetDetent`s (`Fit`, `Half`, `Full`, `Height`), with a grab handle,
  dragged and flung between detents or away, its content's scroll handing a
  pull at the top to the sheet on touch screens; the scrim, Back and Escape
  close it; a centered dialog on windows at least `SHEET_WIDE` wide.
- `Cx::on_back(BackHandler)`: a view takes Back (and the back gesture's
  progress, and optionally an unused Escape) while it's on screen; the last
  one declared gets it, before the App's `on_back` hook, and the App tells
  the system it would go back meanwhile.
- `MotionTokens::page` and `MotionTokens::sheet`, in every preset.
- `examples/navdemo`, in the browser too.
- Choosing the renderer: `App::renderer(Renderer::{Auto, Software, Gpu})`
  (`stipple::render::Renderer`) and the `STIPPLE_RENDERER` environment
  override (`software`, `gpu`, `metal`, `gl`, `auto`). Frames fall back to the
  CPU when there's no GPU backend or device, when setting it up fails, and
  when the device is lost (set up again once). `Auto` is still the CPU.
- Metal renders whole scenes (feature `mtl`): the GLES2 executor's shaders in
  MSL, compiled at run time, and frames presented straight to a
  `CAMetalLayer` over the window's view on macOS (and iOS, compile-checked),
  in sRGB at the display's scale. It matches the software renderer within the
  GL tolerances, and a busy 2560×1600 frame takes about 1 ms instead of
  38 ms. `stipple::gpu::render_scene_scaled` now uses Metal on macOS. See
  `docs/gpu.md`.
- Localization (#47). `platform::preferred_languages()` gives the user's
  languages as BCP 47 tags in order, `platform::locale()` their formatting
  conventions (the region's, with the OS's 12/24-hour clock, first weekday,
  and on macOS, iOS and Windows the separators and date/time formats), and
  `Event::LocaleChanged` / `App::on_locale_change` say when they change;
  `platform::set_preferred_languages` is an app's own language setting.
  `stipple::locale` formats numbers, percentages, money, dates and times
  (`Locale::format_number`, `format_currency`, `format_date`, …) from a
  built-in subset of CLDR, the same on every platform, with CLDR plural rules
  (`Locale::plural`), language negotiation (`locale::negotiate`) and a small
  message `Catalog` (`key = template` lines, plural forms, fallbacks).
  `DateTime` is a calendar date and time (`from_unix_local`, `now`). See
  `docs/localization.md` and the `localedemo` example.
- The layout direction follows the user's language: right to left for
  Arabic, Hebrew, Persian, Urdu… `App::languages(["en", "he"])` says which
  languages the app ships, so an untranslated app stays left to right.
  `Theme::follows_locale`, on in every preset, is turned off by
  `Theme::layout_direction`.
- A message box's default "OK" button and iOS's accessibility values
  ("on", "checked", "expanded"…) are in the user's language.

- Test helpers on `App` that find elements as a screen reader knows them:
  `app.find(Role::CheckBox, "Buy milk")` (where it is, whether it's checked),
  `find_all` (every match), `find_text("1 left")` and
  `app.click(Role::Button, "Add")`, which clicks the element's middle. They
  lay a changed state out first, so no `render_once` is needed between a
  change and a lookup. `Role` is in the prelude.
- `Toggle`, the spec behind `checkbox`, `switch` and `radio`, for a control
  whose label isn't shown (`.label_visible(false)`) but still names it.
- Fonts with CFF or CFF2 (PostScript) outlines: `.otf` files and collections
  such as `NotoSansCJK-Regular.ttc` (Android, Linux) and Hiragino (macOS,
  iOS), CID-keyed ones included, load, shape, fall back and draw like
  TrueType fonts; system font discovery no longer skips them (#48).
- Text falls back to the system's CJK, emoji and symbol fonts by default:
  `Font::system_default()` has the system fallbacks, and each is loaded the
  first time text needs it, so Japanese, Chinese and Korean draw without the
  app asking. A Korean family follows a CJK one without Hangul.
- On the web, `stipple.js`'s `font` option takes a list of URLs: the first is
  the app's font, the rest its fallbacks (a CJK font, say).
- Edge to edge on phones (#38): `App::edge_to_edge(true)` draws under the
  status bar, the notch and the home indicator (iOS) or the system bars
  (Android, which go transparent, their icons dark or light to suit what's
  under them), and `cx.safe_area()` (also `WindowInfo::safe_area`) says how
  far in to keep the controls, updated as the device turns; `safe_area(cx)`
  is a column padded by it. `navigation_bar` runs its background under the
  home indicator and `toasts` keep clear of it by themselves. On the web,
  a page with `viewport-fit=cover` reports `env(safe-area-inset-*)`.
- Orientation (#58): the iOS view follows a rotation (and iPad split view
  or Stage Manager), with a new size and safe area inside the rotation's
  animation; `WindowInfo::orientation()`; `App::orientations(..)` and
  `AppHandle::set_orientations(..)` (`Orientations::PORTRAIT`,
  `LANDSCAPE`, ...) restrict or lock it on iOS and Android.
- Keeping the screen awake (#62): `AppHandle::set_keep_awake(true)` on iOS,
  Android, the web (Screen Wake Lock), macOS, Windows and Linux.
- `examples/screendemo`: edge to edge with the safe area outlined, the
  orientation locked and the screen kept awake.

### Changed (breaking)

- `NavigationBar::bottom_inset` is gone: the bar takes the window's safe
  area by itself.
- `MotionTokens` has two more fields, `page` and `sheet`.
- `App::touch` hands the finger's pointer events to `on_pointer` handlers
  first, as the run loop does.
- `scroll(cx)` and `scroll_horizontal(cx)` take no length: a scroll
  container fills the space its column (or, sideways, its row) leaves free,
  shrinks first when its siblings need the room (down to 100 px or its
  content), and elsewhere is as long as its content up to the space there
  is. `.height(h)` / `.width(w)` fix the length, as for any element.
  `scroll(cx, 200.0).grow(1.0)` is now `scroll(cx)`.
- `checkbox`, `switch` and `radio` take a label:
  `checkbox(cx, "Buy milk", checked, on_toggle)`. It is drawn next to the
  control (after a checkbox or radio button, before a switch), a click on it
  toggles the control, and it is the control's accessible name. Text styles
  set on the control (`.strikethrough()`, `.weight(..)`) reach the label,
  not the check mark.

### Changed

- `Font::with_system_fallbacks()` loads each system fallback when text first
  needs it rather than at once.

### Other

- The repository is one package, no longer a workspace: the examples are
  Cargo examples (`cargo run --example todo`), each in `examples/NAME/main.rs`,
  and what each shows is in `tools/site/examples.toml`. The published crate
  is unchanged.

## [0.0.6](https://github.com/KarpelesLab/stipple/compare/stipple-v0.0.5...stipple-v0.0.6) - 2026-11-15

### Added

- sensible defaults — a system font, centered rows, stretched columns, placeholders, PNG export

### Fixed

- placeholders reach every screen reader; text paints inside its padding; align_self in grids

### Other

- *(widgets)* widgets take cx; one button, one text input
- update screenshots from CI
- *(api)* one naming convention for setters
- *(api)* one public path per item; menu and tray plumbing private; audit in CI
- *(atspi)* send the first Tab again if a fresh virtual keyboard lost it
- *(api)* platform follows the API guidelines
- *(api)* core, layout, geometry and anim follow the API guidelines
- *(api)* widgets, style, render and the crate root follow the API guidelines
- *(api)* game and audio follow the API guidelines
- API guidelines, and a rustdoc-JSON audit of the public API

### Changed (breaking)

- Widgets take the view's `cx` first and read the theme from it: no more
  `let theme = *cx.theme();` and `&theme` on every call. `label(cx, "Hi")`,
  `heading(cx, ..)`, `checkbox(cx, checked, on_toggle)`,
  `select(cx, options, &state, field)`, `NavigationBar::…view(cx)`,
  `TitleScreen::…view(cx)`, and so on for every widget in `widgets` and
  `game::ui`. Widgets that only draw take `&Cx<S>`, those that register
  handlers `&mut Cx<S>`, so `row(vec![label(cx, ..), button(cx, ..).on_tap(cx, ..)])`
  compiles.
- One way to compose containers: build the frame, then add its children with
  the new `Element::children(vec![…])` and `Element::child(el)` (for any
  stack, grid or layers). The frame's call has released `cx` by the time the
  children are built, so they can register handlers; no more closures or
  `let`s built ahead for the borrow's sake. `row(vec![…])`, `column(…)` and
  `grid(…)` keep taking their children.

  | Before | After |
  |---|---|
  | `panel(cx, \|cx\| vec![…])` | `panel(cx).children(vec![…])` |
  | `toolbar(cx, \|cx\| vec![…])` | `toolbar(cx).children(vec![…])` |
  | `scroll(cx, h, \|cx\| content)`, `scroll_horizontal(cx, w, …)` | `scroll(cx, h).child(content)` (or `.children(…)`) |
  | `game::ui::menu_list(cx, \|cx\| vec![…])` | `menu_list(cx).children(vec![…])` |
  | `game::ui::setting_row(cx, label, \|cx\| control)` | `setting_row(cx, label).child(control)` |
  | `split_pane(cx, &state, field, first, second)` | `split_pane(cx, &state, field).panes(first, second)` |
  | `open_dialog(cx, title, body, actions, on_dismiss)` | `dialog(cx, title, on_dismiss).child(body).actions(actions).open(cx)` |
  | `game::ui::pause_menu(cx, title, buttons, on_resume)` | `pause_menu(cx, title, on_resume).children(buttons).open(cx)` |

  `panel`, `toolbar`, `menu_list` and `setting_row` now take `&Cx`. A dialog
  without actions no longer leaves an empty row (and its gap) at its bottom.
- `Cx::themed(theme, |cx| …)` builds part of a view under another theme (for
  previews and theme galleries); `Cx::theme()` returns the theme in effect.
- One button: `button(cx, "Save")`, and the `Button` spec for the other
  variants and custom content
  (`Button::new("Delete").variant(Variant::Danger).view(cx)`,
  `Button::from_content(icon).view(cx)`). Removed `button(theme)` (the empty
  button-colored box), `button_labeled` and `button_variant`.
- One text input: `text_field(cx, &state.name, |s| &mut s.name)` and
  `text_area(…)` edit a new `TextInput` state (an `EditBuffer`, with
  `max_length` and `read_only`, and `text`, `set_text`, `clear`, `take`).
  `Element::on_submit(cx, ..)` runs on Enter in a single-line field (or a
  password field) and `Element::on_change(cx, ..)` after each edit; a
  single-line field no longer inserts a newline on Enter. Removed
  `text_editor`, the `&str` + `on_key` `text_field`, and `edit_string`. A
  field clips text and placeholders longer than it.
- Toolbar buttons: `tool_button(cx, "Save")` and the `ToolButton` spec
  (`ToolButton::from_icon(icon, name)`, `.selected(on)`), made to act with
  `.on_tap`; removed `tool_icon_button` and `tool_toggle`.
- `open_menu(cx, at, menu, on_close)` takes a `Menu` of `MenuItem`s (the same
  data as the menu bar and context menus), drawn like a context menu; picking
  an item runs it and then `on_close`. Removed `menu_item` and `menu`.
- Removed `widgets::setting_row` (a demo composite: a swatch, a spacer and a
  button).
- Handlers and accessors need no type annotations where `cx` comes first:
  `|s| s.n += 1`, `|s| &mut s.name`. The README, docs and examples drop them.

- One naming convention for setters (see "Naming" in
  `docs/api-guidelines.md`; `api-audit.py` now flags `with_*` builders and
  `get_*` getters). Chainable setters take the bare property name, `&mut self`
  setters are `set_*`, getters have no `get_` (this also renames the `with_*`
  setters added earlier in this release):

  | Before | After |
  |---|---|
  | `Theme::with_{accent, palette, typography, spacing, radius, font_size, motion, focus, layout_direction, follows_system}` | `Theme::{accent, palette, …}` |
  | `Theme::with_native(bool)` | `Theme::native_controls(bool)` (`Theme::native()` stays the preset) |
  | `GameStyle::with_{font, font_bytes, accent, background, surface, text, text_muted, danger, success, warning, radius, font_size, focus}` | `GameStyle::{font, font_bytes, accent, …}` |
  | `WindowAttributes::with_{title, logical_size, position, min_size, max_size, maximized, fullscreen, icon, decorations, resize_increments, focus, dark, background}` | `WindowAttributes::{title, logical_size, …}` |
  | `FileDialog::with_{title, file_name, accept}` | `FileDialog::{title, file_name, accept}` |
  | `ClipboardContent::text(t)`, `::image(i)` / `.with_{text, html, image}` | `ClipboardContent::from_text(t)`, `::from_image(i)` / `.{text, html, image}` |
  | `Decoration::underline(s)`, `::strikethrough()`, `::overline()` / `.with_{underline, strikethrough, overline, color}` | `Decoration::NONE.underline(s)`, `.strikethrough()`, `.overline()`, `.color(c)` |
  | `FontStyle::with_{weight, italic}` | `FontStyle::{weight, italic}` |
  | `Camera2D::at`, `with_{zoom, rotation, snap, shake}`; `Camera2D::shake(amount)` | `Camera2D::{position, zoom, rotation, snap, shake}`; `Camera2D::add_trauma(amount)` |
  | `Tilemap::at` | `Tilemap::origin` |
  | `Toasts::with_{placement, bottom_offset}` (and the `placement()`, `bottom_offset()` getters) | `Toasts::{placement, bottom_offset}` (getters removed; `set_*` stay) |
  | `ColorPicker::with_alpha(bool)`, `with_swatches` | `ColorPicker::alpha_strip(bool)`, `swatches` |
  | `PasswordField::with_{label, reveal_button}` | `PasswordField::{label, reveal_button}` |
  | `TouchControls::with_{stick, button}` | `TouchControls::{stick, button}` |
  | `Typewriter::with_speed`, `RebindCapture::with_timeout` (and its `timeout()` getter) | `Typewriter::speed`, `RebindCapture::timeout` (getter removed) |
  | `GamepadInfo::with_ids`; `GamepadState::with_{button, axis}` | `GamepadInfo::ids`; `GamepadState::set_{button, axis}` (`&mut self`) |
  | `ScrollDelta::with_phase`, `TextLayout::with_align_width`, `SoftwareRenderer::with_background` | `phase`, `align_width`, `background` |
  | `Spring::with_params` (and the `params()` getter) | `Spring::params` (getter removed) |
  | `App::with_viewport_content` | `App::viewport_content` |
  | `EditBuffer::with_direction` | removed: `set_direction` |
  | `Cx::window_title(t)` | `Cx::set_window_title(t)` |
  | `MenuItem::get_shortcut`, `get_submenu` | crate-internal |

  Kept: `Color::with_alpha` (derives a copy of a plain value), the `Font`
  derivations (`with_fallback`, `with_fallbacks`, `with_system_fallbacks`,
  `with_face`, `with_weight`), `with_*` constructors (`Spring::with_velocity`,
  `SpringParams::with_response`, `SpriteAnimation::with_durations`,
  `Cx::with_handle`) and `KeyValues::get_or`.
- One public path per item: gamepads are only at `stipple::gamepad` (not
  `stipple::platform::gamepad`), `Shortcut` only at `stipple::core`. The
  `Native*` menu and tray types, `TRAY_ID_BASE`, `App::native_tray` and
  `Tray::activate_command` (platform plumbing) are no longer public.
- *(game, audio)* the public API follows `docs/api-guidelines.md`:
  - `#[non_exhaustive]` on `audio::{PlayParams, MusicParams, Tone, AudioStats, AudioError, Waveform}`, `game::{Camera2D, PixelViewport, PixelLayout, Tilemap, TouchButton, TouchStick, TouchPoint, ParseBindingError, Playback, ShowTouch}`, `game::save::{Loaded, SaveInfo, SlotInfo, SlotState, ParseKeyValuesError}` and `game::ui::{GameStyle, Bar, RebindCapture, BindingRowState, Capture}`: build them with their constructors and setters (fields stay public to read and assign), and match the enums with a `_` arm.
  - New setters: `Camera2D::{at, with_zoom, with_rotation, with_snap, with_shake}`, `Tilemap::at`, `GameStyle::{with_background, with_surface, with_text, with_text_muted, with_danger, with_success, with_warning}`, `BindingRowState::{capturing, conflicts}`.
  - `SpriteSheet::grid_spaced(image, w, h, margin, spacing)` is `SpriteSheet::from_grid(image, SpriteGrid::new(w, h).margin(m).spacing(s))`.
  - `Actions::axis2(left, right, up, down)` is `axis2((left, right), (up, down))`.
  - `game::ui::title_screen(theme, title, subtitle, menu, footer)` is `TitleScreen::new(title, menu).subtitle(..).footer(..).view(theme)`, and `game_over_screen(theme, title, lines, menu)` is `GameOverScreen::new(title, menu).line(label, value).view(theme)`; both take `.backdrop(None)` to show the game behind them instead of the theme's opaque background.
  - `game::ui::dialogue_box(cx, theme, speaker, &text, portrait, on_activate)` is `DialogueBox::new(&text, on_activate).speaker(..).portrait(..).view(cx, theme)`.
  - `game::ui::fullscreen_row` takes an `on_change(&mut S, bool)`, so the game can save the setting.
- *(style)* `Theme`, `Palette`, `Typography`, `Spacing`, `MotionTokens` and `FocusLook` are `#[non_exhaustive]`: build them from a preset and chainable setters (`MotionTokens::standard().press_scale(1.0)`, `Palette::dark().primary(c)`, `Theme::light().with_palette(p)`, new `with_typography`, `with_spacing`, `with_native`, `with_follows_system`) or assign fields. New `Palette::light`/`dark` and `Default`.
- *(render)* `Scene::fill_text(text, origin, &TextPaint)` replaces `fill_text(font, text, origin, size, color)`; `TextPaint::new(&font, size, color)` can carry a `Decoration`. `Scene::draw_decoration(baseline, width, &TextPaint)` replaces the five-argument form (the metrics come from the paint's font).
- *(render)* `Decoration`, `FontStyle`, `ImageOptions`, `FontMetrics`, `FontFamilyInfo` and `RasterStats` are `#[non_exhaustive]` (use the constructors and `with_*` setters); `DrawCmd`, `Filter`, `ImageError`, `ImageFit`, `ImageSource`, `LineJoin`, `TextDirection` and `UnderlineStyle` are `#[non_exhaustive]`. `Scene::primitive_count` is crate-internal; `Scene::into_vector_frame_region` and `vector_frame_region` are gone.
- *(widgets)* `table` takes the sort callback on the spec: `TableSpec::new(..).on_sort(f)` (`TableSpec` is now `TableSpec<'a, S>`). `tree` takes the model on the spec: `TreeSpec::new(&model)` replaces `TreeSpec::default()` plus a model argument.
- *(widgets)* `Toasts::push_for` is replaced by `push` followed by `Toasts::dismiss_after(id, duration, &handle, field)`.
- *(widgets)* `Column`, `TableSpec`, `TableState`, `TreeSpec`, `TreeRow`, `VirtualList`, `RowInfo`, `DisplayText` and `Toast` are `#[non_exhaustive]`, as are `NavBadge`, `NavIcon`, `NavLook`, `RowHeight`, `SelectionMode`, `TabIndicator`, `ToastKind`, `ToastPlacement` and `Variant`.
- *(app)* `CloseResponse` is `#[non_exhaustive]`. The `FrameRenderer`, `MenuFn`, `ThemeFn`, `TrayFn`, `ViewFn`, `ViewportInputFn` and `WindowChangeFn` aliases are crate-internal (no public signature used them), and `stipple::Lifecycle` is only at its canonical path, `stipple::platform::Lifecycle` (and in the prelude).
- *(core)* `Element`'s fields are private: build elements with their
  constructors and modifiers. `LayoutStyle`, `SizeOverride`, `TextStyle`,
  `ElementKind`, `Semantics`, `KeyConfig` and `Reactive` are internal.
- *(core)* `LayoutNode`'s fields are private; read a laid-out tree with
  `bounds()`, `children()`, `rtl()`, `transform()`, `opacity()`, `affine()`
  and `to_local()`. `NodeContent` and `ScrollTargets` are internal.
- *(core)* The runtime's plumbing is internal: the `*_at`, `find_*` and
  `collect_*` tree queries (`hit_test`, `local_point` and `node_to_window`
  stay), `diff_trees`, `apply_scroll`, `mark_native_states`, `caret_index_at`,
  `paint_focus`, `paint_focus_look`, `paint_hover`, `paint_preedit`,
  `ime_state`, `secure_focused`, `promote_tap_focus`, `MemoCache`,
  `Handlers::dispatch_*` (`Handlers::dispatch` stays), and the `Cx` methods
  the app drives a build with (`take_overlays`, `take_context_menus`,
  `set_scroll_state`, `set_drop_states`, `take_drop_queried`,
  `take_window_title`, `window_info_read`, `frame_requested`,
  `set_window_size`, the memo-cache seeding), `ScrollRequests::take` and
  `Menu`/`MenuBar`/`Tray::to_native`.
- *(core)* One path per item: the `core::a11y`, `core::keys`, `core::menu`,
  `core::runtime` and `core::tab_focus` modules are private (their public
  items are at `stipple::core::*`). `Axis`, `LayoutDirection` and `Track` are
  only at `stipple::layout` (and the prelude), `Font` only at
  `stipple::render`, `set_pointer_modifiers` only at `stipple::platform`.
- *(core)* `#[non_exhaustive]`: `Align`, `Anchor`, `KeyInput`,
  `ViewportEvent`, `Role`, `FocusGroup`, `PointerEventKind`, `PinchPhase`,
  `Damage` (match them with a `_` arm), and `BoxStyle`, `Effect`,
  `OverlaySpec`, `Shortcut`, `AccessNode`, `DragMotion`, `DropState`,
  `PointerEvent`, `WheelEvent`, `PinchEvent`, `PointerInfo`. Build a
  `BoxStyle` with `BoxStyle::filled(color).radius(r).border(c, w)` and an
  overlay with `OverlaySpec::new(content, anchor).modal(true).dismiss(id)`;
  `Effect` gains `opacity()` and `transform()` setters, `DragMotion` a
  `new()` for tests.
- *(layout)* The flex and grid solvers (`Constraints`, `FlexItem`, `Span`,
  `TrackItem`, `solve_main_axis`, `solve_tracks`, `break_lines`,
  `place_cells`, `span_extent`, `mirror_spans`, `clamp_len`) are internal;
  `Track` is `#[non_exhaustive]`.
- *(geometry)* `Transform` is `#[non_exhaustive]`: start from
  `Transform::IDENTITY` (or `default()`) and chain its builders.
- *(anim)* `Easing` and `Motion` are `#[non_exhaustive]`, and so are
  `Tween`, `Spring`, `SpringParams`, `Decay`, `Delay` and `Repeat`: build
  them with `new`, the presets and the `AnimationExt` combinators.
- *(render)* `Font::layout_in(text, size, max_width, ui)` drops its
  `direction` parameter: it lays out with `TextDirection::Auto`, the only
  direction the UI's affects (use `Font::layout` for an explicit one).
- *(platform)* follows `docs/api-guidelines.md`. The backend plumbing is no
  longer public: the `Window` trait, `ControlFlow`, `backend::run` and the
  per-OS backend modules (`headless`, `macos`, `windows`, `wayland`),
  `A11yNode` / `A11yRole` (the `access` module), `wake`,
  `MenuPopup`, `clipboard::ClipboardRead`, `clipboard::format`,
  `frame::{request, suspended, wanted, elapsed}`,
  `native::{render, clear_cache, watch, motion_scale}`, `WindowRequest::apply`,
  and on Android `ARect`, `ANativeActivityCallbacks`, `log`,
  `present_to_native_window` and `ANativeActivity`'s fields. The modules the
  repository's harness examples drive (`a11y`, `uia`, `scm`, `shm`, `sandbox`,
  `backend::x11`, `backend::ios`, `backend::web`) are `#[doc(hidden)]`.
- *(platform)* `platform::menu` and `platform::shell` are no longer public
  modules: use `platform::{Shortcut, NativeMenu, …, mnemonic, mnemonic_at,
  mnemonic_range}` and `platform::{notify, open_url, open_path, Notification,
  tray_shown}`.
- *(platform)* `#[non_exhaustive]` structs: `Modifiers` (build with
  `Modifiers::CTRL | Modifiers::SHIFT`, or `Modifiers::COMMAND`), `KeyEvent`,
  `ScrollDelta`, `Touch` (new `Touch::new`), `DragOffer`, `WindowAttributes`
  (new `with_dark`, `with_background`), `WindowInfo`, `ImeState`, `DoubleClick`,
  `Shortcut`, `NativeMenu`, `NativeMenuBar`, `NativeTray`, `Notification`,
  `FileDialog`, `FileContents`, `MessageBox`, `MessageInput`, `MessageResponse`,
  `ClipboardContent`, `ClipboardFormats` (new `NONE`, `HTML`, `IMAGE`, `|`),
  `FrameStats`, `NativeDeco`, `NativeState`, `SystemLook`.
- *(platform)* `#[non_exhaustive]` enums (add a `_` arm when matching):
  `PointerButton`, `ScrollUnit`, `ScrollPhase`, `TouchPhase`, `DropData`, `Ime`,
  `KeyLocation`, `OptionAsMeta`, `CursorIcon`, `UserAttention`, `BackGesture`,
  `BackEdge`, `NativeMenuItem`, `ClipboardKind`, `MessageKind`,
  `GamepadButton`, `GamepadAxis`, `NativeControl`.
- *(core)* Stacks align like CSS flexbox: a row (`row`, a horizontal
  `Element::stack`) centers its children across by default, and a column
  stretches the ones without a width to its own (a row in a column spans
  it, so a `spacer()` in it pushes). Cross-axis `Stretch` now leaves a child
  sized on that axis alone, as grid cells already did; a text field's default
  width yields to it. Children that relied on sitting at the top of a row or
  at a column's left need `.align(Align::Start, Align::Start)` on the stack
  or `.align_self(Align::Start)`.
- *(app)* An app draws text with the system's default font
  (`Font::system_default()`) when it sets none, instead of drawing no text;
  where there's none (or on the web without the page's `font` option) it says
  so once on stderr, logcat or the console. `App::no_font()` draws no text,
  for font-independent pixel tests.

### Added

- *(docs)* [Getting started](docs/getting-started.md): a todo list built step
  by step, from `cargo new` to a headless test. Its Rust blocks are doctests
  or excerpts of the new `examples/todo` (checked by that crate's tests), and
  CI runs `todo --selftest`, which types, ticks, deletes and saves a PNG.
- *(core)* `Element::align_self(Align)`: a child's own cross-axis alignment
  in a stack, and its vertical alignment in a grid cell (CSS's `align-self`);
  `Element::justify_self(Align)` is its horizontal alignment in a grid cell
  (`justify-self`). Neither affects children of `layers`.
- *(widgets)* `Element::placeholder(text)` on `text_field`, `text_editor`,
  `text_area` and `password_field`: a muted hint while the field is empty and
  not composing, exposed to accessibility as `AccessNode::placeholder`: the
  AT-SPI description, `AXPlaceholderValue` on macOS, UI Automation's
  `HelpText` on Windows, `accessibilityHint` on iOS, the node's hint text on
  Android 8.0 and later (`setHintText`), and `aria-placeholder` on the web.

### Fixed

- `motiondemo`'s code field drew two borders; the field itself turns red now,
  and Enter checks the code.

- *(core)* A text element's `.padding(..)` now places its glyphs inside the
  padding (they were drawn at the box's corner, as in `motiondemo`'s list
  labels); the caret, selection, underline, click-to-caret and input-method
  positions follow, and right to left the padding mirrors as an image's does.
- *(render)* `Pixmap::encode_png()` and `Pixmap::save_png(path)`. The
  examples' `--shots`/`--png` modes and `themegallery`, `material3demo` and
  `gpudemo` write PNGs instead of PPM or raw RGBA files.

## [0.0.5](https://github.com/KarpelesLab/stipple/compare/stipple-v0.0.4...stipple-v0.0.5) - 2026-11-15

### Added

- *(game)* saving — atomic settings and save slots (Phase 9f)
- *(toasts)* snackbar placement, an action button, swipe to dismiss ([#43](https://github.com/KarpelesLab/stipple/pull/43))
- *(platform)* app directories and bundle information ([#49](https://github.com/KarpelesLab/stipple/pull/49))
- *(game)* game UI — theme, spatial navigation, menus, settings rows, HUD (Phase 9e)
- *(game)* gamepad and pointer-lock input in Actions, touch controls (Phase 9b)
- *(audio)* audio output on every platform and a mixer (Phase 9d)
- *(game)* sprites, pixel-art scaling, tilemaps, a camera and fast blits (Phase 9c)
- *(pointer)* pointer lock with relative motion (Phase 9b)
- *(gamepad)* gamepads on every platform (Phase 9b)
- *(game)* the game loop, input state and actions (Phase 9a/9b)
- *(widgets)* bottom navigation bar ([#41](https://github.com/KarpelesLab/stipple/pull/41))
- *(widgets)* chips — choice and filter groups ([#45](https://github.com/KarpelesLab/stipple/pull/45))
- *(focus)* buttons and other tap-activated controls take keyboard focus
- *(atspi)* register again when the registry or the accessibility bus restarts
- *(atspi)* caret, selection and labelled-by relations for text fields
- *(a11y)* X11 and Wayland windows serve their tree over AT-SPI, registered with the registry
- *(ios)* saved state only comes back after a system kill, not a swipe-away
- *(core)* memoized subtrees with handlers, derived values, animation frames without a rebuild
- *(motion)* modal scrims fade in with their dialog; OS-drawn controls cross-fade
- *(mobile)* iOS back swipe, saved state on iOS and the web, picker receive-side test
- *(mobile)* document pickers on iOS and Android, notifications on Android
- *(motion)* overlays animate out, and the segmented tabs slide their indicator
- *(android)* predictive back and state saved across process death
- *(android)* InputConnection, accessibility node provider, Back and app lifecycle
- *(widgets)* stateless popups and toasts use the core's enter and layout animations
- *(widgets)* built-in widgets animate by default, with per-theme motion tokens
- *(ios)* UITextInput composition, UIAccessibility, live appearance, notifications and app lifecycle
- *(windows)* input methods through the Text Services Framework
- *(anim)* enter/exit transitions and layout animations for keyed elements
- *(windows)* runtime windows, titles and geometry on every desktop backend
- *(widgets)* middle-click closes a tab, double-click renames it
- *(widgets)* tab bar, password field, color picker and message boxes
- *(keys)* raw key events, key repeat and IME for custom elements
- *(clipboard)* X11 paste, primary selection, HTML and images
- *(canvas)* custom-paint elements with partial invalidation
- confirm close and quit, window focus, attention, Wayland decorations
- *(input)* raw pointer and wheel events for custom elements
- *(text)* system fonts by name, bold/italic, per-element fonts, fallbacks, metrics and decorations

### Fixed

- *(a11ydemo)* map Role::RadioButton
- *(native/macos)* draw native controls on the main thread only
- *(ios)* drop the saved state when the app closes its own scene session
- *(android)* don't report the host view focusable to accessibility, so TalkBack stops on labels
- *(ios)* a scene app's launch isn't a return to the foreground
- *(windows)* deliver events queued by sent messages before waiting again
- *(uia)* don't panic dropping a window's provider tree while its thread exits
- *(uia)* a window's root provider answers with the current tree; the walk hit-tests with the window in front
- *(uia)* …Fragment and …FragmentRoot are interfaces of their own, with their own vtables
- *(uia)* providers say they're server-side (0x2), with COM threading
- *(ios)* a document picker whose presentation UIKit delays still counts as shown
- *(ios)* check a document picker presentation took, and present again if not
- *(macos/a11y)* keep the vended accessibility elements alive between requests
- *(uia)* QueryInterface answers only the interfaces the provider implements
- *(ios)* present a document picker once the previous one has finished leaving
- *(a11y)* what an out-of-process screen reader needs on Windows, macOS, iOS
- *(touch)* a pinch can start with a finger that caught a fling; androiddemo's test finds its button
- *(ios)* accessibility frames in screen coordinates, set directly
- *(ime)* an empty field reports no text, so TSF compositions survive the App's echo
- *(android)* keep typed keys as text while an input method is connected
- *(windows/tsf)* try every way to start the self-test's composition, log it, and fall back to IMM32 on failure
- *(wayland)* name keys a virtual keymap types, and don't guess their position

### Other

- *(android)* a uiautomator dump that misses a node retries instead of ending the script
- gamedemo, a small game on Phase 9
- a newer push cancels the CI and Visual runs still going for the older one
- quote a run line ending in a colon (the workflow didn't parse)
- *(games)* point to pointer lock and gamepads
- Phase 9 — 2D games (loop, input, gamepads, sprites, audio, game UI)
- *(pages)* deploy the web demo to GitHub Pages
- oxideav-raster 0.1.3, for preserveAspectRatio (image fit)
- *(async)* Android wakes its looper with ALooper_wake
- update screenshots from CI
- *(a11y-dbus)* check caret, selection, relations and a registry restart
- *(ios)* verify swipe-away discard on an iPad simulator
- screen-reader checks required (NVDA, VoiceOver, Orca, TalkBack, the sway AT-SPI walk)
- update screenshots from CI
- TalkBack's footnote says it passes in CI
- TalkBack passes in CI
- *(ime-windows)* required, after three passing runs in a row
- NVDA, VoiceOver, Orca and the AT-SPI walks pass in CI
- update screenshots from CI
- *(android)* drive the edge swipe step by step so its progress reaches the app
- *(talkback)* grant the demo's notification permission so its dialog doesn't hold TalkBack's focus
- update screenshots from CI
- *(nvda)* bring the navigator back to the focus after say-all; the window name on foreground is only reported
- drive real screen readers in CI (NVDA, VoiceOver, TalkBack)
- *(ime-windows)* stop the foreground trick's Alt tap from putting the window in menu mode
- update screenshots from CI
- *(uia)* pass a real null class to FindWindowW
- wait for a document picker to be really on screen before accepting a delayed presentation
- *(uia)* take the window found by title and process without requiring its UIA name to be the title, and say what it found
- *(uia)* find touchdemo's window by title (checked to be the started process's)
- *(roadmap)* macOS animates at full rate on CI now (36 frames during asyncdemo's load)
- *(uia)* take the window from its handle instead of searching the desktop
- *(ime)* Windows PowerShell 5 has no [ushort] type accelerator; use [uint16]
- android CI: API 33 and 30 emulators, the edge swipe for real, and a two-finger pinch
- *(ime)* real input methods on Wayland (fcitx5 under sway), attempts on Windows and macOS
- *(a11y)* check accessibility from outside the app, as screen readers do
- *(macos)* present only the damage, as native-layout sRGB; frame clock stats
- androiddemo test: wait for gesture navigation to settle, and only report when the emulator doesn't take an edge swipe as back
- pinch, long-press context menus, sideways scrolling and touch on the web
- update screenshots from CI
- *(text)* cache glyph outlines when painting text, and drop glyph cache keys for the software rasterizer
- update screenshots from CI
- *(roadmap)* the iOS and Android checks now pass in CI
- don't count frames in the selftest; a slow runner draws few
- update screenshots from CI
- render whole scenes on the GPU: images, paths, layers, canvases
- don't attach a buffer before the window's first configure
- say which object and error a protocol error names
- *(fonts)* the underline's centre, not its top, must sit below the baseline
- Payload::is_empty is only used by the Linux backends
- publish the sessionsdemo screenshots
- *(x11)* wait for IBus to register its XIM server before starting the app
- update screenshots from CI

## [0.0.4](https://github.com/KarpelesLab/stipple/compare/stipple-v0.0.3...stipple-v0.0.4) - 2026-11-10

### Added

- *(core)* interaction states, per-state overrides and implicit transitions
- *(anim)* interruptible animations, keyframes, staggers and effects
- transforms and group opacity for any element and its subtree

### Other

- Reduced motion from the OS, and theme changes that cross-fade
- transforms are Animatable; CI runs motiondemo's selftest
- transforms guide and transformdemo; mark the Phase 8 item done
- update screenshots from CI
- *(roadmap)* spell out the IME and mobile gaps
- update screenshots from CI

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
