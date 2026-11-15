//! This example in the browser: `main.rs` as a library, so the
//! `stipple::web_app!(main)` at its bottom exports `main`'s app for
//! `web/stipple.js`. It's the cdylib example `themegallery_web` in Cargo.toml,
//! which `tools/site/build.py wasm` builds for `wasm32-unknown-unknown`.

#[path = "main.rs"]
mod example;
