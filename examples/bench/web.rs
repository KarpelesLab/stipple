//! The benchmark in the browser: `main.rs` as a library. The page
//! (`tools/bench/index.html`) runs `main`'s app through `web/stipple.js` to
//! time events to the frames presented after them, and calls the module's
//! `bench_*` exports on an instance of its own to run the scenarios. It's
//! the cdylib example `bench_web` in Cargo.toml.

#[path = "main.rs"]
mod example;
