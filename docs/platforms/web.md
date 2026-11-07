# Web

A Stipple app compiles to a `wasm32-unknown-unknown` module that renders into an
RGBA framebuffer, which a small hand-written script blits onto a `<canvas>`. No
`wasm-bindgen` and no JavaScript framework are involved.

## Building

```sh
rustup target add wasm32-unknown-unknown
cargo build --release -p webdemo --target wasm32-unknown-unknown
```

Serve these three files from one directory:

- `examples/webdemo/index.html`, the page and its script;
- `target/wasm32-unknown-unknown/release/webdemo.wasm`;
- `font.ttf`, the font the page loads. Without it, text doesn't render.

```sh
python3 -m http.server --directory site
```

## How it fits together

The module exports a small C ABI that the page calls:

1. `stipple_alloc`, then `stipple_set_font` to hand over the font's bytes;
2. `stipple_init(width, height)` to build the app and render;
3. `stipple_click`, `stipple_pointer_move` and `stipple_text` for canvas events,
   each of which re-renders;
4. `stipple_frame_ptr`, `stipple_frame_len`, `stipple_width` and `stipple_height`
   to read the frame, which the page draws with `putImageData`.

[`examples/webdemo`](../../examples/webdemo) is a complete app. Those exports are
generic, so a later version will provide them from the library.

## Known gaps

- Renders at 1x, with no high-DPI.
- No OS-drawn controls, system clipboard or file dialogs.
