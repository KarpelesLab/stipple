//! App-level tests of `App::on_open_url`: a URL delivered before the app's
//! first build waits for it, and one delivered while it runs rebuilds the view.

use crate::App;
use crate::core::{Cx, Element};
use crate::geometry::Size;
use crate::platform::incoming_url::deliver;

#[derive(Default)]
struct State {
    urls: Vec<String>,
}

fn view(s: &State, cx: &mut Cx<State>) -> Element {
    crate::widgets::label(cx, format!("{} urls", s.urls.len()))
}

fn app() -> App<State> {
    App::new(State::default(), view)
        .no_font()
        .logical_size(Size::new(200.0, 100.0))
        .on_open_url(|s, url| s.urls.push(url.to_string()))
}

#[test]
fn a_launch_url_waits_for_the_first_build() {
    // Delivered as a cold start does: before the app (or its loop) exists.
    deliver("myapp://callback?code=1".into());
    let mut app = app();
    app.render_once();
    assert!(app.state().urls.is_empty(), "not before the loop runs");
    assert!(app.run_pending());
    assert_eq!(app.state().urls, ["myapp://callback?code=1"]);
    // Delivered once.
    assert!(!app.run_pending());
    assert_eq!(app.state().urls.len(), 1);
}

#[test]
fn urls_while_running_arrive_in_order() {
    let mut app = app();
    app.render_once();
    deliver("https://example.com/a".into());
    deliver("myapp://b".into());
    assert!(app.run_pending());
    assert_eq!(app.state().urls, ["https://example.com/a", "myapp://b"]);
}

#[test]
fn without_a_hook_nothing_runs() {
    let mut app = App::new(State::default(), view).no_font();
    app.render_once();
    deliver("myapp://ignored".into());
    assert!(!app.run_pending());
    assert!(app.state().urls.is_empty());
}
