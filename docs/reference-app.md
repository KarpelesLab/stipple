# The reference app: a media browser

`examples/mediabrowser` is a file and media browser built only from the
toolkit's own pieces: the app the roadmap's Phase 3 asked for to show that
Stipple can carry a real product. It runs on Linux (X11 and Wayland), Windows
and macOS, and in the browser over a sample folder held in memory.

```sh
cargo run --release --example mediabrowser                  # the last folder, or the sample
cargo run --release --example mediabrowser -- ~/Pictures    # a folder
cargo run --release --example mediabrowser -- --sample      # a generated sample folder
cargo run --release --example mediabrowser -- --selftest    # the scripted run CI makes
```

<!-- live: mediabrowser -->

Add `--features audio` to play Ogg Vorbis, MP3, FLAC and Opus as well as WAV.

## What it does

- **Browsing.** A folder tree (`tree`) beside the folder's files, a
  breadcrumb of the folders down to it, Up and Refresh, and the system's
  folder picker (`platform::dialog::pick_folder`, File > Open Folder) to
  browse another root.
- **Two views of the files.** A table (`table`) with name, size, kind and
  modified columns, sorted by any of them (folders stay first) and resized
  by dragging a header's edge; or a grid of thumbnails built a screenful at a
  time on a scroll container of its own. Both share one selection: click,
  Shift+click, Ctrl/Cmd+click, the arrows (Shift extends), Home/End,
  Ctrl/Cmd+A, Space, Escape.
- **Type to select.** Typing a name jumps to the first entry starting with
  it; a repeated letter steps through the names starting with it.
- **Opening.** Enter or a double click enters a folder, plays a sound, or
  hands any other file to the system (`platform::open_path`). Backspace or
  Alt+Up goes to the enclosing folder, with the folder you came from
  selected.
- **Context menu.** Right-click, the Menu key or Shift+F10 on an item: Open,
  Copy Path (onto the system clipboard), Show in Folder, and Play for sounds.
- **Drag and drop.** Items drag out of the window as files (into a file
  manager, an editor, a mail); files dropped on the list are copied into
  the folder shown.
- **Preview.** Pictures (PNG, JPEG, GIF, WebP, BMP and SVG, through the
  oxideav decoders `Pixmap::decode` uses), with their dimensions; sounds
  with Play/Pause, Stop and a position slider that seeks (`stipple::audio`),
  with their duration, channels and rate; text with its line and word
  counts; Markdown as rich text (headings, lists, quotes, code, emphasis);
  and every file's kind, size, date and location.
- **Off the UI thread.** Thumbnails and previews are decoded by worker
  threads and cached; the view asks for what it shows each time it's built,
  and a job whose tile scrolled away (or whose file was left) before its turn
  is dropped without being decoded. Each result reaches the app through its
  `AppHandle`. On the web, which has no threads, a timer decodes one job per
  tick on the UI thread.
- **Menus and shortcuts.** File (Open Folder Ctrl/Cmd+O, Open Sample Folder,
  Refresh F5, Quit Ctrl/Cmd+Q), Edit (Copy Path Ctrl/Cmd+Shift+C, Select All),
  View (List Ctrl/Cmd+1, Grid Ctrl/Cmd+2, the theme, the language), Go
  (Enclosing Folder Alt+Up), Playback (Play/Pause Ctrl/Cmd+P, Stop). Native
  menu bars on Windows and macOS, drawn elsewhere.
- **Remembered.** The window's size and whether it was maximized (from
  `App::on_window_change`), the theme, the language, the view, the split
  positions and the folder, in a file in the app's config directory
  (`platform::dirs()`).
- **Themes.** The system's (`Theme::native()`: light or dark, the OS's own
  controls on Windows and macOS), or light or dark whatever the system says.

## Accessible

Everything is reachable from the keyboard: Tab goes through the toolbar, the
breadcrumb, the view switch, the tree, the list (or grid), the player's
buttons and its slider; F10 or Alt reaches the menus. Screen readers get:

| What | Role | Name | State |
|---|---|---|---|
| The folder tree | `Tree` | "Folders" | |
| Its folders | `TreeItem` | the folder | selected, expanded/collapsed |
| The list | `Table` | "Files" | focused |
| Its headers | `ColumnHeader` | "Name", "Size", … | |
| Its rows and cells | `Row`, `Cell` | the cell's text | selected |
| The grid | `List` | "Files" | |
| Its tiles | `ListItem` | the file's name | selected |
| Toolbar, breadcrumb | `Button`s in `Toolbar`s | "Up", "Location", … | |
| Play, Pause, Stop | `Button` | the action | |
| The position slider | `Slider` | "Position, 0:01 of 0:02" | focusable |
| The preview | `Group` | "Preview" | |

The App-level tests (`examples/mediabrowser/tests.rs`) check this tree, Tab
reaching every pane, the keys, and the translations. In CI, out of process:
an AT-SPI walk on X11 (`.github/scripts/atspi-mediabrowser.sh`, the walk
Orca's library does), a UI Automation walk on Windows
(`.github/scripts/uia-roles.ps1`), and `axprobe` on macOS, which also presses
Play through `AXPress`. All three are non-blocking for now; the AT-SPI one
passes locally.

## Localized

Every string goes through a `stipple::locale::Catalog`: English, French,
Japanese and Arabic, picked from the user's languages
(`locale::negotiate`) or from View > Language. Sizes ("1.5 KB", "1,5 Ko",
"١٫٥ ك.ب"), dates and times (`Locale::format_date_time`, in the user's time
zone), durations and counts (plural rules: Arabic's six forms) follow the
language's locale, or the user's own conventions when they read that
language. Arabic mirrors the whole layout: the tree moves to the right, the
table's first column to the right edge, the breadcrumb runs leftwards, the
slider fills from the right and its keys swap. Picking a language also calls
`platform::set_preferred_languages`, so Stipple's own strings (a message
box's OK) follow.

## The selftest

`--selftest` writes the sample to a temporary folder (pictures through the
toolkit's PNG encoder and a hand-written BMP, WAV sounds from the
synthesizer, text in four languages, Markdown, 48 tiles), then drives the
app as a user would, off screen, through the accessibility tree:

1. picks Pictures in the tree and checks the listing;
2. sorts by size both ways and back by name;
3. clicks a row, presses Down, types "s" and checks sunset.png is selected
   and previewed at 320 × 200;
4. extends the selection with Shift+Up and selects all with Ctrl+A;
5. switches to the grid in the 48 tiles, waits for the workers'
   thumbnails, scrolls to the end at once and checks the tiles in between
   were never decoded;
6. types "c" and presses Enter in Music: chime.wav plays through the real
   output (the null output where there's none), its position advances,
   clicking the slider seeks, Pause pauses;
7. previews README.md as rich text;
8. switches to French (headers, sizes in Ko, dates) and Arabic (mirrored,
   Arabic-Indic digits), then Japanese in the dark theme;
9. checks the settings file.

Each step's frame is saved as a PNG (`--out DIR`). Then it opens the real
window on the same folder for a few seconds (`--hold SECS`): the grid's
thumbnails load on the workers, the list then shows a picture previewed, and
timers check it before it quits. `--headless` skips the window. CI runs it
on X11 and macOS (non-blocking) and on Windows (required), and keeps the PNGs and a screenshot
of the window.

## What the toolkit gained for it

Building it found a few gaps, filled in the toolkit in general-purpose form:

- **Seeking a sound.** `Voice::position()` (where it's playing, as of the
  output's last buffer), `Voice::seek(at)` (sounds played from memory), and
  `Voice::is_paused()`. See [audio.md](audio.md).
- **Keys a table doesn't use.** `TableSpec::on_key` gets Enter, typed text
  and the rest while the table has focus, and a cursor it moves is scrolled
  into view: Enter to open and type-to-select. See
  [lists-and-tables.md](lists-and-tables.md#tables).
- **An accessible, keyboard-driven slider.** `slider` takes focus, steps
  with the arrows (mirrored right to left) and Home/End, and reports the new
  `Role::Slider`: AT-SPI's slider, UI Automation's Slider control type,
  NSAccessibility's `AXSlider`, Android's `SeekBar`, ARIA's `slider`. Its
  value is in its name for now: no platform's range-value interface
  (`RangeValue`, `AXValue`, `Value`) carries it yet.

And a few it works around, still open:

- A table's or tree's height is fixed by its spec; the app derives it from
  the window's size (`Cx::window_size`) minus its chrome, so a drawn menu
  bar's height is estimated.
- There is no image scaling API; the app box-filters thumbnails itself.
- Text that overflows its box (a long path without spaces) isn't wrapped at
  slashes; the app clips such values so they don't spill over their
  neighbors. (Its old glyphs past the box used to stay on screen when it
  changed; the damage now covers where the lines run.)
- The window's position isn't restored (no `App` option for it).
- Drag-out doesn't leave the app on the web (see
  [drag-and-drop.md](drag-and-drop.md)).
