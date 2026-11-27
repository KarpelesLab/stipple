//! URLs that open the app: a custom scheme's (`myapp://callback?code=…`, the
//! browser handing an OAuth sign-in back), a universal link (iOS) or a
//! verified App Link (Android), at launch or while the app runs. Each backend
//! hears them its own way and [`deliver`]s them as [`Event::OpenUrl`], which
//! [`App::on_open_url`](crate::App::on_open_url) hears; see
//! `docs/os-integration.md`.
//!
//! | | At launch | While running |
//! |---|---|---|
//! | iOS | `application:openURL:options:` (and the launch options), or the scene's `connectionOptions` | `application:openURL:options:` / `scene:openURLContexts:`; `continueUserActivity` for universal links |
//! | Android | the `Link` activity's intent (or the app's activity's own, with a hand-written manifest) | the `Link` activity again, as `NativeActivity` doesn't pass `onNewIntent` on |
//! | macOS | `application:openURLs:` | `application:openURLs:` |
//! | Windows, Linux | the command line ([`launch_urls`]) | the second process's command line, with [`App::single_instance`](crate::App::single_instance) |
//! | Web | — (the page's own URL is the app's) | — |
//!
//! A URL delivered before the event loop runs waits in the posted-event
//! queue, so a cold start's URL reaches the hook once the app's state exists
//! (after [`App::restore_state`](crate::App::restore_state)), with the first
//! events.

use crate::platform::event::Event;

/// Queue `url` for the app's [`App::on_open_url`](crate::App::on_open_url)
/// hook, on the running event loop or the one about to start. An empty URL
/// is dropped.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn deliver(url: String) {
    let url = url.trim().to_string();
    if url.is_empty() {
        return;
    }
    crate::platform::window::debug_window(format_args!("open url {url:?}"));
    crate::platform::shell::post(Event::OpenUrl(url));
}

/// The URL a launch's options named, until the callback that follows hands
/// it over again (UIKit's `application:openURL:options:` after
/// `application:didFinishLaunchingWithOptions:` returns).
static LAUNCH: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// [`deliver`] the URL the launch's options name, and remember it, so the
/// [`deliver_opened`] that follows with the same URL doesn't deliver it twice.
#[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
pub(crate) fn deliver_launch(url: String) {
    *LAUNCH.lock().unwrap_or_else(|e| e.into_inner()) = Some(url.clone());
    deliver(url);
}

/// [`deliver`] a URL a callback hands over, unless it's the launch's own,
/// already delivered by [`deliver_launch`] (once: the same URL again is a new
/// opening).
#[cfg_attr(not(any(target_os = "ios", test)), allow(dead_code))]
pub(crate) fn deliver_opened(url: String) {
    let launch = LAUNCH.lock().unwrap_or_else(|e| e.into_inner()).take();
    if launch.as_deref() != Some(url.as_str()) {
        deliver(url);
    }
}

/// The command-line arguments (past the program's name) that are URLs a
/// desktop's protocol handler passed: a scheme of two characters or more
/// (RFC 3986: a letter, then letters, digits, `+`, `-`, `.`), then `:/`.
/// That takes `myapp://callback?code=1` and the reverse-DNS form
/// `com.example.app:/oauth`, but not `key:value` or `C:\path` (a drive
/// letter), nor `file:` URLs, which name files rather than open the app.
#[cfg_attr(
    not(any(
        target_os = "linux",
        target_os = "fullrust",
        target_os = "windows",
        target_os = "macos",
        test
    )),
    allow(dead_code)
)]
pub(crate) fn launch_urls(args: impl IntoIterator<Item = String>) -> Vec<String> {
    args.into_iter()
        .filter(|a| {
            let Some((scheme, rest)) = a.split_once(':') else {
                return false;
            };
            let mut chars = scheme.chars();
            scheme.len() > 1
                && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                && rest.starts_with('/')
                && !scheme.eq_ignore_ascii_case("file")
        })
        .collect()
}

/// On Windows and Linux, deliver the URLs the app was started with (a
/// registered protocol handler's `"%1"`, or a `.desktop` file's `%u`, passes
/// the URL as an argument). Once per process, as the event loop starts.
#[cfg(any(target_os = "linux", target_os = "fullrust", target_os = "windows"))]
pub(crate) fn post_launch_args() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for url in launch_urls(std::env::args().skip(1)) {
            deliver(url);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(args: &[&str]) -> Vec<String> {
        launch_urls(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn launch_arguments_that_are_urls() {
        assert_eq!(
            urls(&[
                "myapp://callback?code=1",
                "--verbose",
                "com.example.app:/oauth2redirect",
                "https://example.com/a",
            ]),
            [
                "myapp://callback?code=1",
                "com.example.app:/oauth2redirect",
                "https://example.com/a"
            ]
        );
        for not in [
            "key:value",
            "C:\\Users\\a.txt",
            "c:/x",
            "file:///tmp/a.txt",
            "plain",
            "-x:/y",
            "1abc://x",
            "platform::dex",
            "",
        ] {
            assert!(urls(&[not]).is_empty(), "{not}");
        }
    }

    #[test]
    fn the_launch_url_is_delivered_once() {
        let posted = || crate::platform::shell::take_posted();
        deliver_launch("myapp://launch".into());
        // The callback after launching hands the same URL over: dropped.
        deliver_opened("myapp://launch".into());
        assert_eq!(posted(), [Event::OpenUrl("myapp://launch".into())]);
        // Later openings, the same URL included, are delivered.
        deliver_opened("myapp://launch".into());
        deliver_opened("myapp://other".into());
        assert_eq!(
            posted(),
            [
                Event::OpenUrl("myapp://launch".into()),
                Event::OpenUrl("myapp://other".into())
            ]
        );
        // A launch whose callback brings another URL: both.
        deliver_launch("myapp://a".into());
        deliver_opened("myapp://b".into());
        assert_eq!(posted().len(), 2);
    }

    #[test]
    fn delivered_urls_are_posted_trimmed() {
        // The queue is the test thread's own.
        deliver("  myapp://delivered-trimmed \n".into());
        deliver(String::new());
        assert_eq!(
            crate::platform::shell::take_posted(),
            [Event::OpenUrl("myapp://delivered-trimmed".into())]
        );
    }
}
