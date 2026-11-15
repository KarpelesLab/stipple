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

- **Five or more parameters**, not counting `self`, `cx` and `theme`, take an
  options or spec struct, or become a builder, as `TabBar`, `NavigationBar`
  and `TableSpec` do. A widget's `(cx, &theme, …, on_change)` shape is fine:
  `select(cx, &theme, options, &state, field)` has three that count.
- **Three or more of the same number or bool type in a row** (`f64, f64, f64`,
  a run of `bool`s) get the same treatment, or newtypes, since a caller could
  swap them without the compiler noticing.
- Prefer one obvious way: when adding an options form, keep the short form only
  if it's the common case, as a thin wrapper.

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
`#[non_exhaustive]`, enums that aren't, and functions with many (or easily
swapped) parameters.
Deliberate exceptions (the plain values above, closed enums) are listed with
a reason in `.github/scripts/api-audit-allow.txt`.

```sh
RUSTDOCFLAGS="-Z unstable-options --output-format json" \
  cargo +nightly doc -p stipple --no-deps --lib --target-dir target/apijson
python3 .github/scripts/api-audit.py target/apijson/doc/stipple.json
```

It exits non-zero when something new isn't covered by the rules or the
allowlist.
