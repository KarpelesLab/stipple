//! System fallback fonts prepared off the UI thread once the app takes text
//! input.
//!
//! The first text that needs a system fallback (the first kana, hangul or
//! emoji) used to index every installed font and read the fallback's file
//! on whichever thread drew it. That costs seconds on a cold disk: on CI's
//! Windows runners, 147 font files took 2 to 8 s to index (each file opened
//! is scanned by the antivirus first) and the CJK collection another 0.2 to
//! 0.8 s to read. The first such text is usually an input method's first
//! preedit, and a UI thread stalled that long in the middle of a
//! composition loses the input method's keys: the Microsoft Japanese IME
//! dropped or misplaced the keys typed meanwhile, or the window went
//! "Not Responding" with the composition stuck.
//!
//! So once a text field gets the input method on a thread
//! ([`text_input_focused`]), that thread no longer loads fallbacks itself:
//! when its text first needs one ([`must_wait`]), the index is built and
//! the fallback resolved and read on a background thread, the text is drawn
//! without it meanwhile (the missing glyphs show as boxes for that moment),
//! and when it's ready the [generation](generation) moves on and the event
//! loop is woken, so the App draws its windows again with it. Nothing is
//! read ahead of that need: indexing as the field got focus competed for
//! the disk with the input method's own start, and made its first key
//! (which loads its dictionaries) take up to 10 s instead of about one.
//! Other threads, and a thread before any text input (an app that draws
//! Japanese labels, a headless render), load fallbacks as before, when the
//! text needs them.

use super::{FontStyle, system};
use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

thread_local! {
    /// This thread takes text input: fallbacks load in the background.
    static DEFER: Cell<bool> = const { Cell::new(false) };
}

/// The generic fallback families prepared and asked for, and whether the
/// background thread runs.
struct State {
    ready: Vec<&'static str>,
    pending: Vec<&'static str>,
    running: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    ready: Vec::new(),
    pending: Vec::new(),
    running: false,
});

/// Moves on each time the background thread made fallbacks ready.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The background thread couldn't start: every thread loads its own
/// fallbacks, as before.
static NO_THREAD: AtomicBool = AtomicBool::new(false);

/// A text field got the input method on this thread: from now on, the
/// system fallbacks its text needs load in the background. Nothing on the
/// web (no threads, and no system fonts).
pub(crate) fn text_input_focused() {
    if cfg!(target_family = "wasm") {
        return;
    }
    DEFER.with(|d| d.set(true));
}

/// Whether text on this thread must do without the fallbacks `want` for
/// now: it takes text input and they aren't ready yet. Their preparation
/// is started (or already under way); [`generation`] moves on when it's
/// done.
pub(crate) fn must_wait(want: &[&'static str]) -> bool {
    if !DEFER.with(|d| d.get()) || NO_THREAD.load(Ordering::Relaxed) {
        return false;
    }
    let ready = system::index_ready() && {
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        want.iter().all(|g| st.ready.contains(g))
    };
    if !ready {
        request(want);
    }
    !ready
}

/// How many times fallbacks became ready in the background: an App that
/// saw another value draws its windows again.
pub(crate) fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Queue `want` (those neither ready nor queued) and start the background
/// thread if it isn't running.
fn request(want: &[&'static str]) {
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    for g in want {
        if !st.ready.contains(g) && !st.pending.contains(g) {
            st.pending.push(g);
        }
    }
    if st.running || (system::index_ready() && st.pending.is_empty()) {
        return;
    }
    st.running = true;
    let spawned = std::thread::Builder::new()
        .name("stipple-fonts".into())
        .spawn(work);
    if spawned.is_err() {
        st.running = false;
        NO_THREAD.store(true, Ordering::Relaxed);
    }
}

/// The background thread: build the index, then prepare what's queued,
/// until nothing is.
fn work() {
    loop {
        system::index();
        let batch = {
            let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
            if st.pending.is_empty() {
                st.running = false;
                return;
            }
            std::mem::take(&mut st.pending)
        };
        for g in &batch {
            prepare(g);
        }
        STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ready
            .extend(batch);
        GENERATION.fetch_add(1, Ordering::AcqRel);
        crate::platform::wake::wake();
    }
}

/// Resolve the generic family `g` and read its regular face once: the
/// family is shared (the UI thread gets the same one), and the file comes
/// from the OS's cache when the UI thread reads it.
fn prepare(g: &str) {
    if let Ok(family) = super::system_family(g)
        && let Some((source, _)) = family.pick(FontStyle::default())
    {
        let _ = source.parse();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_without_text_input_loads_its_own_fallbacks() {
        // A fresh test thread never had a text field focused.
        assert!(!must_wait(&["cjk"]));
    }

    #[test]
    fn text_input_defers_fallbacks_until_the_background_thread_has_them() {
        text_input_focused();
        let before = generation();
        // Not ready the first time it's asked (unless an earlier test in
        // this process prepared it); ready once the background thread had
        // it, which moved the generation on.
        if must_wait(&["symbol"]) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while must_wait(&["symbol"]) {
                assert!(std::time::Instant::now() < deadline, "never prepared");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(generation() > before);
        }
        // What is ready stays ready.
        assert!(!must_wait(&[]));
    }
}
