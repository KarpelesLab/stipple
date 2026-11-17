//! The bookkeeping behind
//! [`start_activity_for_result`](crate::platform::android::start_activity_for_result):
//! each request gets a request code of its own, and its callback waits under
//! that code until `onActivityResult` brings it back. System-free, so it's
//! tested on every host.
//!
//! The codes are `APP_CODE | n`: bit 30 set, which the backend's own request
//! codes never have (the document picker's are dialog ids, the share sheet's
//! `0x2_0000 | id`), and positive, as `startActivityForResult` wants them for
//! an answer. The callbacks are kept by code in a static, not by activity, so
//! an answer delivered to the activity that replaced the one that asked (a
//! configuration change, "Don't keep activities") still finds its callback.

/// The bit every app request code has.
pub(crate) const APP_CODE: i32 = 0x4000_0000;

/// The codes' low bits: `APP_CODE | n` for `n` up to this.
const MASK: u32 = 0x3fff_ffff;

/// Whether request code `code` is one of [`Requests`]'.
pub(crate) fn is_app_code(code: i32) -> bool {
    code >= 0 && code & APP_CODE != 0
}

/// The requests waiting for an answer, by request code.
#[derive(Debug)]
pub(crate) struct Requests<T> {
    next: u32,
    waiting: Vec<(i32, T)>,
}

impl<T> Requests<T> {
    pub(crate) const fn new() -> Self {
        Self {
            next: 0,
            waiting: Vec::new(),
        }
    }

    /// Keep `value` under a new request code, which it returns: the next one
    /// that isn't waiting already (after `MASK` requests the codes wrap).
    /// Codes aren't reused sooner, as a late answer may still come for one
    /// already answered: an intent nothing handles throws at once, and the
    /// activity manager sends `RESULT_CANCELED` for it afterwards too.
    pub(crate) fn insert(&mut self, value: T) -> i32 {
        loop {
            let code = APP_CODE | (self.next & MASK) as i32;
            self.next = (self.next + 1) & MASK;
            if !self.waiting.iter().any(|(c, _)| *c == code) {
                self.waiting.push((code, value));
                return code;
            }
        }
    }

    /// What waits under `code`, once: a second answer for it finds nothing.
    pub(crate) fn take(&mut self, code: i32) -> Option<T> {
        let i = self.waiting.iter().position(|(c, _)| *c == code)?;
        Some(self.waiting.remove(i).1)
    }

    /// How many requests are waiting.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.waiting.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_distinct_positive_and_marked() {
        let mut r = Requests::new();
        let a = r.insert("a");
        let b = r.insert("b");
        assert_ne!(a, b);
        for code in [a, b] {
            assert!(code > 0);
            assert!(is_app_code(code));
        }
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn the_backends_own_codes_are_not_the_apps() {
        // The document picker's (dialog ids), the share sheet's, a
        // negative one (no answer comes for those).
        for code in [0, 1, 0x7fff, 0x2_0000 | 5, -1, i32::MIN] {
            assert!(!is_app_code(code), "{code:#x}");
        }
    }

    #[test]
    fn an_answer_takes_its_own_callback_once() {
        let mut r = Requests::new();
        let a = r.insert(1);
        let b = r.insert(2);
        assert_eq!(r.take(b), Some(2));
        assert_eq!(r.take(b), None);
        assert_eq!(r.take(a), Some(1));
        assert!(r.is_empty());
        // Unknown codes (another process's, a stale answer) find nothing.
        assert_eq!(r.take(APP_CODE | 77), None);
    }

    #[test]
    fn answers_may_come_in_any_order() {
        let mut r = Requests::new();
        let codes: Vec<i32> = (0..5).map(|i| r.insert(i)).collect();
        for &i in &[3, 0, 4, 1, 2] {
            assert_eq!(r.take(codes[i]), Some(i));
        }
        assert!(r.is_empty());
    }

    #[test]
    fn wrapping_skips_codes_still_waiting() {
        let mut r = Requests::new();
        let first = r.insert("old");
        // The counter comes round again to the first code while its request
        // still waits.
        r.next = MASK;
        let last = r.insert("last");
        assert_eq!(last, APP_CODE | MASK as i32);
        let next = r.insert("new");
        assert_ne!(next, first);
        assert_eq!(next, APP_CODE | 1);
        assert_eq!(r.take(first), Some("old"));
        assert_eq!(r.take(next), Some("new"));
        assert_eq!(r.take(last), Some("last"));
    }
}
