# CI and screenshots

Three workflows run on every push and pull request:

- **CI**: `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test`;
  the 1.88 minimum Rust version; and cross-compiling for Android and iOS.
- **Visual**: builds the examples on each platform, runs them in real windows,
  screenshots them, and checks what it can see: that a click changed the
  screen, that text was typed, that the web canvas isn't blank, that an
  accessibility tree reached the OS. Linux runs under Xvfb (X11) and headless
  sway (Wayland); iOS and Android under their simulator and emulator.
- **Release-plz**: on `master`, opens release pull requests and publishes to
  crates.io when one is merged.

## Published screenshots

After a push to `master` where every job of the Visual workflow passed, its
`commit-screenshots` job copies a chosen set of screenshots into
`docs/screenshots/` and commits them as `github-actions[bot]`. The README's
images therefore always show what the code on `master` renders.

- git only sees a change when pixels change, so an unchanged UI makes no commit.
- Pushes made with `GITHUB_TOKEN` don't start workflows, so this can't loop.
- Only **deterministic** screenshots are published: a capture with a clock, a
  desktop, or a cursor would change on every run and commit every time. Captures
  are cropped to the app's window for this reason, and the iOS simulator's status
  bar is pinned to 9:41.

To publish a new screenshot, upload it as an artifact from its job, add its name
to the list in `commit-screenshots`, and add that job to the list it `needs`.

## Gotchas

These cost time once already:

- **Xvfb resets when its last client disconnects**, which drops root-window
  properties such as the `Xft.dpi` that `xrdb` just set. Start it with `-noreset`.
- **PowerShell passes `$null` to a `string` parameter as `""`**. To pass a real
  null to a Win32 function, use `[NullString]::Value`.
- **A single-quoted `bash -c '…'` script ends at the first apostrophe**, even
  one in a comment. Write multi-line scripts through a quoted heredoc instead.
- **`ubuntu-latest` now ships ImageMagick 7**: `compare -metric AE` prints
  `N (normalized)`, and `fx:mean` includes the alpha channel unless you add
  `-alpha off`.
