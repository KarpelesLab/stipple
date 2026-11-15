//! Scheduled local notifications: [`schedule_notification`] asks for one
//! later, under an id; [`cancel_notification`] and
//! [`cancel_all_notifications`] take them back; [`pending_notifications`]
//! lists them; and tapping one tells the app which
//! ([`Event::NotificationTapped`](crate::platform::Event::NotificationTapped),
//! [`App::on_notification_tapped`](crate::App::on_notification_tapped)).
//!
//! | | Delivered by | While the app isn't running | Tap reported |
//! |---|---|---|---|
//! | iOS | `UNUserNotificationCenter`, with a time-interval or calendar trigger | yes | yes (`didReceiveNotificationResponse:`), launching the app if needed |
//! | Android | `AlarmManager` firing a broadcast receiver that posts it | with the build-time receiver in the manifest (see the docs); otherwise only while the process lives | when the tap starts the activity |
//! | Web | a timer in the page, then the `Notification` API | no: while the page is open | yes (`onclick`) |
//! | macOS, Windows, Linux | a timer thread, then [`notify`](super::notify) | no: while the app runs | no |
//!
//! See `docs/os-integration.md`.

// iOS keeps the list in the notification center, so needs none of its own.
#![cfg_attr(target_os = "ios", allow(dead_code))]

use std::time::{Duration, SystemTime};

use super::Notification;
use crate::platform::PlatformError;

/// When a [`ScheduledNotification`] first shows.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationTime {
    /// This long after it's scheduled.
    After(Duration),
    /// At this moment (wall-clock time).
    At(SystemTime),
}

/// A notification to show later, under an id the app chose: scheduling
/// another under the same id replaces it, and the id comes back when the user
/// taps it.
///
/// ```
/// use std::time::Duration;
/// use stipple::platform::{Notification, ScheduledNotification};
/// let n = ScheduledNotification::new(
///     "potion",
///     Notification::new("The potion is ready").body("Come back to the lab"),
/// )
/// .after(Duration::from_secs(15 * 60));
/// assert_eq!(n.id, "potion");
/// ```
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledNotification {
    /// The app's name for it.
    pub id: String,
    /// What it shows.
    pub notification: Notification,
    /// When it first shows; at once by default.
    pub when: NotificationTime,
    /// Show it again every so often after the first time; `None` for once.
    pub repeat: Option<Duration>,
}

impl ScheduledNotification {
    /// `notification` under `id`, to show at once unless
    /// [`after`](Self::after) or [`at`](Self::at) says when.
    pub fn new(id: impl Into<String>, notification: Notification) -> Self {
        Self {
            id: id.into(),
            notification,
            when: NotificationTime::After(Duration::ZERO),
            repeat: None,
        }
    }

    /// Show it `delay` after it's scheduled.
    pub fn after(mut self, delay: Duration) -> Self {
        self.when = NotificationTime::After(delay);
        self
    }

    /// Show it at `time`.
    pub fn at(mut self, time: SystemTime) -> Self {
        self.when = NotificationTime::At(time);
        self
    }

    /// Show it again every `interval` after the first time. On iOS a
    /// repeating notification that isn't daily or weekly first shows one
    /// `interval` after it's scheduled, and `interval` must be a minute or
    /// more; a daily (24 h) or weekly one repeats at the first time's
    /// time of day (and weekday).
    pub fn repeating(mut self, interval: Duration) -> Self {
        self.repeat = Some(interval);
        self
    }

    /// The first time it shows, in milliseconds since the Unix epoch, given
    /// the time now (the same).
    pub(crate) fn due_ms(&self, now_ms: u64) -> u64 {
        match self.when {
            NotificationTime::After(d) => now_ms.saturating_add(d.as_millis() as u64),
            NotificationTime::At(t) => t
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        }
    }

    /// The repeat interval in milliseconds; 0 for none.
    pub(crate) fn repeat_ms(&self) -> u64 {
        self.repeat.map_or(0, |d| (d.as_millis() as u64).max(1))
    }
}

/// Ask the system to show `n` later (see [`ScheduledNotification`]),
/// replacing any notification pending under the same id.
///
/// Ask for permission first, at a moment that makes sense to the user, with
/// [`request_notification_permission`]: scheduling doesn't ask, and without
/// permission the notification isn't shown (iOS drops the request; Android
/// 13+ can't post it when it's due). `Ok` means the system took the request.
///
/// On iOS, Android and the web each id is limited to its id's characters
/// being printable. On Android the alarm is exact where the app may set exact
/// alarms (Android 6 to 11, or with `SCHEDULE_EXACT_ALARM` / `USE_EXACT_ALARM`
/// granted), else inexact (the system may delay it by some minutes), and
/// alarms the system loses across a reboot are set again when the app starts
/// (or at boot, with the build-time receiver). See `docs/os-integration.md`.
///
/// ```no_run
/// use std::time::Duration;
/// use stipple::platform::{Notification, ScheduledNotification, schedule_notification};
/// schedule_notification(
///     &ScheduledNotification::new("adventurers", Notification::new("Your adventurers are back"))
///         .after(Duration::from_secs(3600)),
/// )
/// .unwrap();
/// ```
pub fn schedule_notification(n: &ScheduledNotification) -> Result<(), PlatformError> {
    if n.id.is_empty() || n.id.chars().any(char::is_control) {
        return Err(PlatformError::Os(format!(
            "not a notification id: {:?}",
            n.id
        )));
    }
    os::schedule(n)
}

/// Take back the notification scheduled under `id`, if it hasn't shown yet,
/// and remove it from the notification center if it has (iOS, Android). A
/// repeating one stops.
pub fn cancel_notification(id: &str) {
    os::cancel(Some(id));
}

/// Take back every notification the app scheduled, and remove the delivered
/// ones (iOS, Android).
pub fn cancel_all_notifications() {
    os::cancel(None);
}

/// The ids of the notifications scheduled and still to show (a repeating one
/// stays until cancelled), for `done`. On iOS the notification center
/// answers, on a thread of its own; elsewhere `done` runs before this
/// returns. Use it to reconcile after a restart: Android and iOS keep them
/// across launches, the desktops and the web don't.
pub fn pending_notifications(done: impl FnOnce(Vec<String>) + Send + 'static) {
    os::pending(Box::new(done));
}

/// Ask the user's permission to show notifications, and tell `done` whether
/// it's granted. The system asks once (iOS's prompt, Android 13's dialog, the
/// browser's); later calls answer at once with what the user said. Where no
/// permission is needed (the desktops, Android before 13) it's `true`.
///
/// Call it from the UI thread. `done` runs on whichever thread the system
/// answers on; change the state from it with
/// [`AppHandle::update`](crate::core::AppHandle::update).
pub fn request_notification_permission(done: impl FnOnce(bool) + Send + 'static) {
    os::request_permission(Box::new(done));
}

// ---- The list kept by hand, where the system keeps none -----------------------

/// A scheduled notification as kept where the system doesn't keep them (the
/// desktops, the web, and Android, whose `AlarmManager` can't list its
/// alarms).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) body: String,
    /// When it next shows, in milliseconds since the Unix epoch.
    pub(crate) due_ms: u64,
    /// The repeat interval in milliseconds; 0 for none.
    pub(crate) repeat_ms: u64,
}

impl Entry {
    pub(crate) fn new(n: &ScheduledNotification, now_ms: u64) -> Self {
        Self {
            id: n.id.clone(),
            title: n.notification.title.clone(),
            body: n.notification.body.clone(),
            due_ms: n.due_ms(now_ms),
            repeat_ms: n.repeat_ms(),
        }
    }

    #[cfg_attr(any(target_arch = "wasm32", target_os = "android"), allow(dead_code))]
    pub(crate) fn notification(&self) -> Notification {
        Notification::new(self.title.clone()).body(self.body.clone())
    }

    /// Whether it's still to show at `now_ms`.
    pub(crate) fn pending(&self, now_ms: u64) -> bool {
        self.repeat_ms > 0 || self.due_ms > now_ms
    }

    /// One line: the fields separated by tabs, with `\`, tab and newline
    /// escaped.
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    pub(crate) fn encode(&self) -> String {
        fn esc(s: &str) -> String {
            s.replace('\\', "\\\\")
                .replace('\t', "\\t")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
        }
        format!(
            "{}\t{}\t{}\t{}\t{}",
            esc(&self.id),
            self.due_ms,
            self.repeat_ms,
            esc(&self.title),
            esc(&self.body)
        )
    }

    /// The entry [`encode`](Self::encode) wrote on `line`.
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    pub(crate) fn decode(line: &str) -> Option<Self> {
        fn unesc(s: &str) -> String {
            let mut out = String::with_capacity(s.len());
            let mut chars = s.chars();
            while let Some(c) = chars.next() {
                if c != '\\' {
                    out.push(c);
                    continue;
                }
                match chars.next() {
                    Some('t') => out.push('\t'),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some(other) => out.push(other),
                    None => {}
                }
            }
            out
        }
        let mut f = line.split('\t');
        let id = unesc(f.next()?);
        let due_ms = f.next()?.parse().ok()?;
        let repeat_ms = f.next()?.parse().ok()?;
        let title = unesc(f.next()?);
        let body = unesc(f.next().unwrap_or(""));
        (!id.is_empty()).then_some(Self {
            id,
            title,
            body,
            due_ms,
            repeat_ms,
        })
    }
}

/// Entries one per line.
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) fn encode_all(entries: &[Entry]) -> String {
    let mut s: String = entries
        .iter()
        .map(Entry::encode)
        .collect::<Vec<_>>()
        .join("\n");
    s.push('\n');
    s
}

/// The entries [`encode_all`] wrote, skipping lines it can't read.
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) fn decode_all(s: &str) -> Vec<Entry> {
    s.lines().filter_map(Entry::decode).collect()
}

/// Replace the entry with `entry`'s id by it (or add it).
pub(crate) fn upsert(entries: &mut Vec<Entry>, entry: Entry) {
    entries.retain(|e| e.id != entry.id);
    entries.push(entry);
}

/// A stable 31-bit number for `id` (FNV-1a), for the APIs that want an
/// `int`: Android's notification and request codes.
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub(crate) fn int_id(id: &str) -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in id.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    (h & 0x7fff_ffff) as i32
}

/// Milliseconds since the Unix epoch, now.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn now_ms() -> u64 {
    crate::platform::backend::web::date_now() as u64
}

// ---- Per-platform dispatch ------------------------------------------------------

#[cfg_attr(target_os = "android", allow(dead_code))]
type Pending = Box<dyn FnOnce(Vec<String>) + Send>;
#[cfg_attr(target_os = "android", allow(dead_code))]
type Permission = Box<dyn FnOnce(bool) + Send>;

#[cfg(target_os = "ios")]
mod os {
    pub(super) use crate::platform::shell::ios::{
        cancel_scheduled as cancel, pending_scheduled as pending, request_permission, schedule,
    };
}

#[cfg(target_os = "android")]
mod os {
    pub(super) use crate::platform::backend::android::{
        cancel_scheduled as cancel, pending_scheduled as pending, request_permission, schedule,
    };
}

#[cfg(target_arch = "wasm32")]
mod os {
    use super::{Entry, Pending, Permission, ScheduledNotification, now_ms};
    use crate::platform::PlatformError;
    use crate::platform::backend::web;
    use std::sync::Mutex;

    /// What's scheduled in the page.
    static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

    pub(super) fn schedule(n: &ScheduledNotification) -> Result<(), PlatformError> {
        let now = now_ms();
        let entry = Entry::new(n, now);
        web::schedule_notification(&entry, entry.due_ms.saturating_sub(now));
        super::upsert(
            &mut ENTRIES.lock().unwrap_or_else(|e| e.into_inner()),
            entry,
        );
        Ok(())
    }

    pub(super) fn cancel(id: Option<&str>) {
        web::cancel_notification(id.unwrap_or(""));
        let mut entries = ENTRIES.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|e| id.is_some_and(|id| e.id != id));
    }

    pub(super) fn pending(done: Pending) {
        let now = now_ms();
        let ids = ENTRIES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|e| e.pending(now))
            .map(|e| e.id.clone())
            .collect();
        done(ids);
    }

    pub(super) fn request_permission(done: Permission) {
        let id = crate::platform::shell::sharing::register(move |v| done(v != 0));
        web::notification_permission(id);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod os {
    //! A thread that sleeps until the next one is due, then shows it with
    //! [`notify`](crate::platform::notify).

    use super::{Entry, Pending, Permission, ScheduledNotification, now_ms};
    use crate::platform::PlatformError;
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
    static CHANGED: Condvar = Condvar::new();

    pub(super) fn schedule(n: &ScheduledNotification) -> Result<(), PlatformError> {
        start();
        super::upsert(
            &mut ENTRIES.lock().unwrap_or_else(|e| e.into_inner()),
            Entry::new(n, now_ms()),
        );
        CHANGED.notify_all();
        Ok(())
    }

    pub(super) fn cancel(id: Option<&str>) {
        let mut entries = ENTRIES.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|e| id.is_some_and(|id| e.id != id));
        CHANGED.notify_all();
    }

    pub(super) fn pending(done: Pending) {
        let now = now_ms();
        let ids = ENTRIES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|e| e.pending(now))
            .map(|e| e.id.clone())
            .collect();
        done(ids);
    }

    pub(super) fn request_permission(done: Permission) {
        done(true);
    }

    /// Start the thread, once.
    fn start() {
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("stipple-notifications".into())
                .spawn(run);
        });
    }

    fn run() {
        let mut entries = ENTRIES.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let now = now_ms();
            let mut due = Vec::new();
            for e in entries.iter_mut() {
                if e.due_ms <= now {
                    due.push(e.notification());
                    if e.repeat_ms > 0 {
                        // Skip the ones missed while asleep.
                        while e.due_ms <= now {
                            e.due_ms += e.repeat_ms;
                        }
                    }
                }
            }
            entries.retain(|e| e.due_ms > now);
            if !due.is_empty() {
                drop(entries);
                for n in due {
                    if let Err(err) = crate::platform::notify(&n) {
                        eprintln!("stipple: scheduled notification {:?}: {err}", n.title);
                    }
                }
                entries = ENTRIES.lock().unwrap_or_else(|e| e.into_inner());
                continue;
            }
            let wait = entries
                .iter()
                .map(|e| e.due_ms.saturating_sub(now))
                .min()
                // Wake now and then all the same: the clock may jump.
                .map_or(Duration::from_secs(3600), |ms| {
                    Duration::from_millis(ms.min(60_000))
                });
            entries = CHANGED
                .wait_timeout(entries, wait)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
    }
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "android",
    target_arch = "wasm32",
    target_os = "linux",
    target_os = "macos",
    target_os = "windows"
)))]
mod os {
    use super::{Pending, Permission, ScheduledNotification};
    use crate::platform::PlatformError;

    pub(super) fn schedule(_n: &ScheduledNotification) -> Result<(), PlatformError> {
        Err(PlatformError::NoBackend("notifications"))
    }
    pub(super) fn cancel(_id: Option<&str>) {}
    pub(super) fn pending(done: Pending) {
        done(Vec::new());
    }
    pub(super) fn request_permission(done: Permission) {
        done(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_survive_a_round_trip() {
        let e = Entry {
            id: "a\tb\\c".into(),
            title: "Line one\nline two".into(),
            body: String::new(),
            due_ms: 1_700_000_000_000,
            repeat_ms: 86_400_000,
        };
        let back = decode_all(&encode_all(std::slice::from_ref(&e)));
        assert_eq!(back, vec![e]);
        assert!(decode_all("garbage\n\tx\n").is_empty());
    }

    #[test]
    fn due_times_follow_the_request() {
        let n = ScheduledNotification::new("x", Notification::new("t"));
        assert_eq!(n.due_ms(1000), 1000);
        let n = n.after(Duration::from_secs(2));
        assert_eq!(n.due_ms(1000), 3000);
        let t = SystemTime::UNIX_EPOCH + Duration::from_millis(5000);
        assert_eq!(n.clone().at(t).due_ms(1000), 5000);
        assert_eq!(n.repeat_ms(), 0);
        assert_eq!(n.repeating(Duration::from_secs(60)).repeat_ms(), 60_000);
    }

    #[test]
    fn pending_and_replacing() {
        let mk = |id: &str, due, repeat| Entry {
            id: id.into(),
            title: String::new(),
            body: String::new(),
            due_ms: due,
            repeat_ms: repeat,
        };
        assert!(mk("a", 10, 0).pending(5));
        assert!(!mk("a", 10, 0).pending(10));
        assert!(mk("a", 10, 5).pending(100));
        let mut v = vec![mk("a", 1, 0), mk("b", 2, 0)];
        upsert(&mut v, mk("a", 3, 0));
        assert_eq!(v.len(), 2);
        assert_eq!(v.iter().find(|e| e.id == "a").unwrap().due_ms, 3);
    }

    #[test]
    fn int_ids_are_stable_and_positive() {
        assert_eq!(int_id("potion"), int_id("potion"));
        assert_ne!(int_id("potion"), int_id("potions"));
        assert!(int_id("") >= 0 && int_id("zzzzzzzz") >= 0);
    }

    #[test]
    fn ids_must_be_printable() {
        let n = ScheduledNotification::new("", Notification::new("t"));
        assert!(schedule_notification(&n).is_err());
        let n = ScheduledNotification::new("a\nb", Notification::new("t"));
        assert!(schedule_notification(&n).is_err());
    }

    #[test]
    fn desktop_timers_list_what_is_pending() {
        let n = ScheduledNotification::new("test-later", Notification::new("t"))
            .after(Duration::from_secs(3600));
        schedule_notification(&n).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        pending_notifications(move |ids| tx.send(ids).unwrap());
        assert!(rx.recv().unwrap().contains(&"test-later".to_string()));
        cancel_notification("test-later");
        let (tx, rx) = std::sync::mpsc::channel();
        pending_notifications(move |ids| tx.send(ids).unwrap());
        assert!(!rx.recv().unwrap().contains(&"test-later".to_string()));
    }
}
