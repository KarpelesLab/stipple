# CI and screenshots

Four workflows run on every push and pull request:

- **CI**: `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test`;
  the 1.95 minimum Rust version; and cross-compiling for Android and iOS.
- **Visual**: builds the examples on each platform, runs them in real windows,
  screenshots them, and checks what it can see: that a click changed the
  screen, that text was typed, that the web demo's scripted selftest passes in
  headless Chrome, that every example on the [site](platforms/web.md#the-site)
  starts and draws there, that an
  accessibility tree reached the OS. Linux runs under Xvfb (X11) and headless
  sway and Weston (Wayland); iOS and Android under their simulator and
  emulator. It also packages `touchdemo` and `androiddemo` for the stores
  with [`cargo-stipple`](packaging.md) (an `.ipa` and a simulator `.app`, an
  `.aab` and a universal APK) and checks the results, and runs the GPU
  renderer's comparison corpus and the benchmark in the simulator and on
  the emulator (`tools/gpu-mobile.sh`).
- **Perf**: the benchmarks (`examples/bench`, and in headless Chrome
  `tools/bench/`) on Linux, macOS, Windows and the web, on the software
  renderer and each platform's GPU renderer, compared with the committed
  baselines and the latency budgets; see [Performance](#performance).
- **Release-plz**: on `master`, opens release pull requests and publishes to
  crates.io when one is merged.

## Time in tests

The crate's own tests don't read the wall clock. What the library takes as
"now" (timers, the frame clock and so animations and transitions, touch
velocity and flings, long presses, double clicks, the renderer's import
window) comes from `platform::time::now()`, which in a build is
`Instant::now()` and under `cfg(test)` is a per-thread test clock: it starts
at a point shared by every thread and moves only when a test calls
`platform::time::test_clock::advance(duration)`. Events a test feeds in a row
are simultaneous however loaded the runner is (a drag with no pause between
its moves never flings), and a transition is exactly as far along as the test
advanced it. A test that wants time to pass advances the clock instead of
sleeping. What still uses real time is real concurrency, and there tests wait
for a condition (with a generous timeout) rather than a fixed time: asset
loading, the null audio output's thread (its pacing is tested with fixed
times), and streamed music, whose offline render waits for the decoder
thread in tests instead of counting an underrun. Save files' dates are
`SystemTime`. The queue of events posted to the loop (tray picks, store
transactions) is per thread in tests too, so `App::run_pending` in one test
doesn't take another's.

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

## Performance

The Perf workflow (`.github/workflows/perf.yml`) runs
[the benchmarks](performance.md#benchmarks) on every push and pull request:

| Job | Software renderer | GPU renderer |
|---|---|---|
| Linux (`ubuntu-latest`) | blocking | GLES2 on Mesa's llvmpipe |
| macOS (`macos-latest`) | reports | Metal |
| Windows (`windows-latest`) | reports | Direct3D 11 on WARP |
| Web (headless Chrome on Linux) | reports | WebGL2 and WebGPU on SwiftShader |

Each comparison (`tools/bench/compare.py`) writes a table to the run's
**summary** (the run's page on GitHub, under the jobs): baseline, expected
(the baseline scaled by the two runs' calibration loops), now, delta, budget.
A scenario that regresses past its threshold or breaks a budget is measured
again, and fails only if it does both times. The JSON reports are the run's
artifacts. Only the Linux software renderer fails the run; the rest report
(their jobs are `continue-on-error`) until their numbers prove steady. To
make a run's numbers the baselines, run the workflow by hand with
**update_baseline** ticked: its last job commits them to `perf/baseline/`.

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
- **`! grep -q …` never fails a step**: bash's `set -e` ignores commands
  negated with `!`. Write `if grep -q …; then exit 1; fi`.
