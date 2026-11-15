# API guidelines

Stipple is pre-1.0, but its public API should already be one that can grow
without breaking the apps built on it. These are the rules for anything
reachable from outside the crate. `.github/scripts/api-audit.py` checks most
of them (see [Checking](#checking)).

## Structs

- **Plain values stay open.** A type that *is* its fields, and always will
  be, keeps public fields and no `#[non_exhaustive]`, so callers can write
  `Point { x, y }` and destructure it: `Point`, `Size`, `Rect`, `Vec2`,
  `Insets`, `PhysicalSize`, `Color`. Adding a field to one of these is
  a breaking change, so it doesn't happen.
- **Everything else is `#[non_exhaustive]`**: options, parameters, styles,
  events, infos, stats. A new field is then not a breaking change.
  - **Inputs** (options, parameters, styles, specs) implement `Default` where a
    default makes sense, and have a constructor taking what's required plus
    chainable setters (`PlayParams::default().volume(0.5).looping(true)`),
    because other crates can't write a struct expression for a
    `#[non_exhaustive]` struct, not even with `..Default::default()`. The
    fields may stay public for reading (and assigning:
    `let mut p = PlayParams::default(); p.volume = 0.5;`).
  - **Outputs** (events, infos, stats, errors) keep public fields for
    reading. A `new` for tests is fine where an app might fake one.
- **Internals are private.** A field that only the crate reads or writes is
  `pub(crate)` or private, with an accessor if apps need to read it.

## Enums

- `#[non_exhaustive]` unless the set is closed by its nature: `Axis`
  (horizontal or vertical), `ButtonState` (pressed or released), `SortOrder`,
  `Corner`. Roles, keys, cursors, events, errors, platform kinds, easing
  curves and looks all grow: they're `#[non_exhaustive]`.
- Error types are `#[non_exhaustive]` enums (or structs) implementing
  `std::error::Error` and `Display`.

## Functions

- **Five or more parameters**, not counting `self` and `cx`, take an
  options or spec struct, or become a builder, as `TabBar`, `NavigationBar`
  and `TableSpec` do. A widget's `(cx, …, on_change)` shape is fine:
  `select(cx, options, &state, field)` has three that count.
- **Widgets take `cx` first** and read the theme from it, never a `&Theme`
  parameter: `label(cx, "Hi")`, `checkbox(cx, checked, on_toggle)`, and spec
  builders end in `.view(cx)`. A widget that only draws takes `&Cx<S>`; one
  that registers a handler takes `&mut Cx<S>`. Coming first, `cx` also fixes
  the state type, so the handlers and accessors after it need no
  annotation: `|s| s.n += 1`, `|s| &mut s.name`. `cx.themed(theme, |cx| …)`
  builds part of a view under another theme.
- **Containers: build the frame, then add the children.** A widget that holds
  other elements and needs `cx` returns its frame, and the children come in
  a chained call: `panel(cx).children(vec![…])`, `.child(el)`. The frame's
  call is done (and `cx` free again) before the arguments of `.children` are
  evaluated, so the children can register handlers on `cx`; a container
  taking `cx` and its children in one call couldn't have them. No closures
  building children, no children built ahead in `let`s for the borrow's
  sake. `children` and `child` are `Element` methods, for any stack, grid
  or layers; a container that isn't a plain stack returns a small builder
  with the same `child`/`children` and a finishing call: `split_pane(…)
  .panes(first, second)`, `dialog(cx, title, on_dismiss).child(…)
  .actions(…).open(cx)`. Plain layout that needs no `cx` keeps taking its
  children directly: `row(vec![…])`, `column(vec![…])`, `grid(tracks,
  vec![…])`.
- **Three or more of the same number or bool type in a row** (`f64, f64, f64`,
  a run of `bool`s) get the same treatment, or newtypes, since a caller could
  swap them without the compiler noticing.
- Prefer one obvious way: when adding an options form, keep the short form only
  if it's the common case, as a thin wrapper.

## Naming

One convention, so a caller can guess a method's name from the property it
touches:

- **Chainable setters** (`self -> Self`) take the bare property name:
  `Theme::light().accent(c).radius(6.0)`,
  `WindowAttributes::new().title("Notes").logical_size(size)`,
  `ClipboardContent::from_text(t).html(h)`, `Toasts::new().placement(p)`,
  `Camera2D::new(viewport).position(p).zoom(2.0)`. Not `with_accent`, not
  `set_accent`. A setter turning a flag on may take no argument
  (`Decoration::NONE.strikethrough()`); one choosing between two takes the
  `bool` (`.resizable(false)`).
- **`&mut self` setters** are `set_*`: `toasts.set_placement(p)`,
  `buffer.set_direction(d)`, `cx.set_window_title(t)`. Other `&mut self`
  methods are verbs for what they do: `push`, `dismiss`, `add_trauma`.
- **Getters** take the bare name, never `get_*`: `buffer.direction()`,
  `item.label()`. Boolean ones read as a question (`is_empty`, `is_rtl`,
  `has_effects`) or as a state (`hovered`, `pointer_locked`).
- **When a setter and a getter would share a name**, the getter wins on a
  type that is mostly read (a state, like `EditBuffer`: its builder goes, and
  `set_direction` sets it); on a type that is mostly built (options, like
  `Toasts` or `RebindCapture`), the setter wins and the getter goes, since the
  app set the value itself. A preset that clashes with a setter keeps its
  name and the setter says what it sets: `Theme::native()` and
  `.native_controls(bool)`.
- **Constructors** are `new`, `from_*` (`from_text`, `from_rgba8`), `with_*`
  for `new` plus one more input as std does (`Vec::with_capacity`,
  `Spring::with_velocity`), and named presets (`Theme::dark()`,
  `SpringParams::SNAPPY`). A constructor can't share a name with a setter, so
  `ClipboardContent::from_text` builds and `.text(t)` sets.
- **Derivations** — a new value made from `&self`, the original untouched —
  are named for the result: `font.bold()`, `font.styled(style)`,
  `sound.resampled(rate)`, `font.with_fallback(&emoji)`. The only by-value
  `with_*` left is `Color::with_alpha` (a copy of a plain value with another
  alpha, next to `mix` and `darken`); it is allowlisted.
- **Scoped overrides** that run a closure are named for what the closure
  builds under: `cx.themed(theme, |cx| …)`.
- **Callbacks** are `on_*` (`on_change`, `on_sort`, `on_close_requested`);
  hooks that provide a value rather than react to one are named for it
  (`save_state`, `back_enabled`, `paused_when`).

## What's public

- **pub(crate)** for anything only the crate uses: backend plumbing, layout
  internals, dispatch tables, renderer helpers.
- **`#[doc(hidden)]`** for what must be `pub` but isn't for apps: items a
  macro expands to (`web_app!`), hooks the repository's own examples or CI
  harness drive (platform bridges that `a11ydemo` and `uiademo` exercise),
  test seams. Name them so they read as internal, and say in a comment why
  they're public.
- Re-exports: each item has one canonical public path; the `prelude` holds
  what most apps need, not everything.

## Checking

`.github/scripts/api-audit.py` reads rustdoc's JSON (a nightly feature) and
lists, for everything reachable from outside the crate and not
`#[doc(hidden)]`: structs with public fields that aren't
`#[non_exhaustive]`, enums that aren't, functions with many (or easily
swapped) parameters, items at more than one public path, and names that
break the [Naming](#naming) rules (`with_*` builders, `get_*` getters).
Deliberate exceptions (the plain values above, closed enums,
`Color::with_alpha`) are listed with a reason in `.github/scripts/api-audit-allow.txt`.

```sh
RUSTDOCFLAGS="-Z unstable-options --output-format json" \
  cargo +nightly doc -p stipple --no-deps --lib --target-dir target/apijson
python3 .github/scripts/api-audit.py target/apijson/doc/stipple.json
```

It exits non-zero when something new isn't covered by the rules or the
allowlist.
