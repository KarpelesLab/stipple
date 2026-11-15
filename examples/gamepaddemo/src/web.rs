//! This example in the browser: `main.rs` as a library, so the
//! `stipple::web_app!(main)` at its bottom exports `main`'s app for
//! `web/stipple.js`. Built with
//! `cargo build --lib --release --target wasm32-unknown-unknown`.

#[path = "main.rs"]
mod example;
