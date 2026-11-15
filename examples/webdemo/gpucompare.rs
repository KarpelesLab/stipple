//! The web GPU renderer against the software one: `gpucompare.html` loads
//! this module, draws each scene of the comparison corpus (the one the Metal
//! and GLES2 executors are checked with) on WebGPU and on WebGL2 into a
//! canvas, diffs each frame with the software renderer's, and times both
//! renderers on a busy widget window and on 2000 sprites.
//!
//! Build it with the GPU renderer:
//!
//! ```sh
//! cargo build --release --target wasm32-unknown-unknown --features gpu --example gpucompare
//! ```
//!
//! A test harness, not an app: its exports are the page's to call.

#[cfg(target_arch = "wasm32")]
mod harness {
    use std::cell::RefCell;

    use stipple::gpu::web_compare;
    use stipple::platform::backend::web as glue;
    use stipple::render::{Font, Pixmap};

    thread_local! {
        /// The last answer that doesn't fit a number: a name, an error, a frame.
        static TEXT: RefCell<String> = const { RefCell::new(String::new()) };
        static FRAME: RefCell<Option<Pixmap>> = const { RefCell::new(None) };
    }

    fn answer(s: String) -> *const u8 {
        TEXT.with(|t| {
            *t.borrow_mut() = s;
            t.borrow().as_ptr()
        })
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn cmp_alloc(len: usize) -> *mut u8 {
        glue::alloc(len)
    }

    /// The last text answer (`cmp_text_len` bytes of UTF-8).
    #[unsafe(no_mangle)]
    pub extern "C" fn cmp_text() -> *const u8 {
        TEXT.with(|t| t.borrow().as_ptr())
    }

    /// The length of the last text answer.
    #[unsafe(no_mangle)]
    pub extern "C" fn cmp_text_len() -> usize {
        TEXT.with(|t| t.borrow().len())
    }

    /// Build the corpus, with the font in `ptr`/`len` (none when `len` is 0).
    ///
    /// # Safety
    /// `ptr`/`len` come from one `cmp_alloc(len)`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn cmp_load(ptr: *mut u8, len: usize) -> usize {
        let font = (len > 0)
            .then(|| Font::from_bytes(unsafe { glue::take_bytes(ptr, len) }).ok())
            .flatten();
        web_compare::load(font)
    }

    /// Case `i` as `name<TAB>mean<TAB>bad`.
    #[unsafe(no_mangle)]
    pub extern "C" fn cmp_case(i: usize) -> *const u8 {
        let (name, mean, bad) = web_compare::case(i);
        answer(format!("{name}\t{mean}\t{bad}"))
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn cmp_bench_count() -> usize {
        web_compare::bench_count()
    }

    /// Draw case (or timing scene, `bench` 1) `i` on the CPU; the RGBA frame
    /// stays until the next call (`cmp_width`, `cmp_height`).
    #[unsafe(no_mangle)]
    pub extern "C" fn cmp_cpu(i: usize, bench: u32) -> *const u8 {
        let p = web_compare::cpu(i, bench != 0);
        FRAME.with(|f| f.borrow_mut().insert(p).as_bytes().as_ptr())
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn cmp_width() -> u32 {
        FRAME.with(|f| f.borrow().as_ref().map_or(0, |p| p.size().width))
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn cmp_height() -> u32 {
        FRAME.with(|f| f.borrow().as_ref().map_or(0, |p| p.size().height))
    }

    /// Draw case (or timing scene) `i` into the canvas on `api` (0 the page's
    /// choice, 1 WebGPU, 2 WebGL2): 1 drawn (the text answer is the API's
    /// name), 0 not (the text answer says why).
    #[unsafe(no_mangle)]
    pub extern "C" fn cmp_gpu(i: usize, bench: u32, api: u32) -> u32 {
        match web_compare::gpu(i, bench != 0, api) {
            Ok(name) => {
                answer(name);
                1
            }
            Err(e) => {
                answer(e);
                0
            }
        }
    }
}
