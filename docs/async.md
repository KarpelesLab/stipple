# Timers and background work

Widget handlers run on the UI thread and change the state directly. For changes
that come from anywhere else, such as a timer or a thread doing slow work, use an
[`AppHandle`](https://docs.rs/stipple/latest/stipple/core/struct.AppHandle.html).

## The handle

Get one from `App::handle()` before running the app, or from `cx.handle()` inside
a view. Handles are cheap to clone and can be sent to any thread.

```rust
let app = App::new(state, view);
let handle = app.handle();

// From another thread: the closure runs on the UI thread, then the UI re-renders.
std::thread::spawn(move || {
    let page = download();                       // slow work, off the UI thread
    handle.update(move |s: &mut State| s.page = Some(page));
});

app.run();
```

Changes are closures over the state, run on the UI thread between events, so the
state needs no locks and nothing else changes it at the same time.

| Method | What it does |
|---|---|
| `update(f)` | Run `f` on the state as soon as possible |
| `after(delay, f)` | Run `f` once, after `delay` |
| `every(interval, f)` | Run `f` repeatedly, until `cancel(id)` |
| `cancel(id)` | Stop a timer |
| `exit()` | End the event loop, as closing the last window would |

A repeating timer that falls behind (say, the machine slept) skips the ticks it
missed rather than firing them all at once.

To start work from a button, clone the handle into its handler:

```rust
fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let handle = cx.handle();
    button_labeled(&theme, "Refresh").on_tap(cx, move |s: &mut State| {
        s.loading = true;
        let handle = handle.clone();
        std::thread::spawn(move || {
            let page = download();
            handle.update(move |s: &mut State| {
                s.loading = false;
                s.page = Some(page);
            });
        });
    })
}
```

[`examples/asyncdemo`](../examples/asyncdemo) puts this together.

## With tokio

With the `tokio` feature, `AppHandle::spawn` runs a future on a background tokio
runtime and applies its output to the state on the UI thread:

```toml
[dependencies]
stipple = { version = "0.0", features = ["tokio"] }
tokio = { version = "1", features = ["net", "time"] }  # the drivers you use
```

```rust
let handle = cx.handle();
handle.spawn(fetch_page(url), |s: &mut State, page| {
    s.loading = false;
    s.page = Some(page);
});
```

The runtime is multi-threaded and starts on first use; `stipple::core::runtime()`
returns it, to spawn other work or enter it. It enables the drivers your own tokio
dependency turns on, so add the features you need there (`net`, `time`, `fs`...).

Tokio can't own the main thread: macOS and iOS require their event loop to run
there, so Stipple's loop does, and tokio runs alongside on its own threads. Don't
use `#[tokio::main]` with a Stipple app.

The `tokio` feature isn't available on the web, where tokio has no threads. Other
async runtimes work through `update`: run their futures however they like, and
post the result with the handle.

## How the loop wakes

Each backend waits for OS events and for the handle at once, so an idle app with
no timers uses no CPU, and one with a timer sleeps until it's due:

| Platform | Waits with | Woken from other threads by |
|---|---|---|
| X11, Wayland | `poll` on the connection and a pipe | a byte written to the pipe |
| Windows | `MsgWaitForMultipleObjectsEx` | a posted window message |
| macOS | `nextEventMatchingMask:untilDate:` | a posted application-defined `NSEvent` |
| iOS | UIKit's run loop | `dispatch_async_f` / `dispatch_after_f` on the main queue |
| Android, web | not yet: updates queue until the next input | — |

CI runs `asyncdemo --selftest` on X11, Wayland, Windows and macOS: it quits itself
once a 100 ms timer has fired ten times and a background thread's result has
arrived, with no input.

## Not yet

- Timers aren't synchronized with the display's refresh, so an animation driven
  by a 16 ms timer can drift against vsync. A display-driven frame clock is
  planned (Phase 7a in the [roadmap](../ROADMAP.md)).
- With several windows, an update re-renders the window the wake arrived for;
  the others repaint on their next event.
