//! Transforms and group opacity: cards that spin, pulse and fade, and a tilted
//! button that is still clicked where it is drawn.
//!
//! - **Spin** turns a tile (and the text on it) about its center.
//! - **Pulse** scales a tile up and down about its center; its neighbours
//!   don't move, since a transform never changes layout.
//! - **Fade** fades two overlapping tiles as one group: where they overlap
//!   there's no darker seam, because the group is drawn first and blended once.
//! - The tilted **Click me** button counts clicks; press its tilted corners to
//!   see that hit-testing follows the transform.
//!
//! **Pause** stops the motion (and the frames: a paused demo uses no CPU).
//! With `--frame SECS` the demo shows the frame at that time, standing still,
//! for screenshots.

use std::cell::Cell;
use stipple::prelude::*;

struct State {
    clicks: u32,
    /// The motion's time while paused (or for `--frame`), else `None`.
    held: Option<Duration>,
    /// Subtracted from the frame time for the motion's time, so playing on
    /// resumes where pause left off.
    offset: Duration,
    /// The frame time of the last build.
    last_frame: Cell<Duration>,
}

impl State {
    /// The motion's time at the last build.
    fn motion_time(&self) -> Duration {
        self.held
            .unwrap_or_else(|| self.last_frame.get().saturating_sub(self.offset))
    }
}

/// A titled card holding `content` in a fixed-size well.
fn card(theme: &Theme, title: &str, content: Element) -> Element {
    column(vec![
        label(theme, title),
        column(vec![content])
            .align(Align::Center, Align::Center)
            .width(150.0)
            .height(150.0),
    ])
    .gap(theme.spacing.sm)
    .align(Align::Start, Align::Center)
    .padding(Insets::uniform(theme.spacing.md))
    .fill(theme.palette.surface)
    .radius(theme.radius)
}

/// A colored square with a word on it.
fn tile(theme: &Theme, color: Color, word: &str) -> Element {
    column(vec![Element::text(
        word,
        theme.typography.body,
        theme.palette.on_primary,
    )])
    .align(Align::Center, Align::Center)
    .width(80.0)
    .height(80.0)
    .fill(color)
    .radius(10.0)
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    state.last_frame.set(cx.frame_time());
    if state.held.is_none() {
        cx.request_frame(); // keep moving
    }
    let t = state.motion_time().as_secs_f64();
    let wave = (t * std::f64::consts::TAU / 2.0).sin(); // one cycle per 2 s

    let spin = tile(&theme, theme.palette.primary, "Spin")
        .transform(Transform::IDENTITY.rotate_degrees(t * 90.0));
    let pulse = tile(&theme, theme.palette.success, "Pulse")
        .transform(Transform::IDENTITY.scale(1.0 + 0.25 * wave));
    // Two tiles pulled over each other, faded as one group.
    let pair = row(vec![
        swatch(theme.palette.danger, 70.0, 10.0)
            .transform(Transform::IDENTITY.translate(17.0, -12.0)),
        swatch(theme.palette.danger, 70.0, 10.0)
            .transform(Transform::IDENTITY.translate(-17.0, 12.0)),
    ])
    .opacity((0.6 + 0.4 * (t * std::f64::consts::PI).cos()) as f32);

    let cards = row(vec![
        card(&theme, "Spin", spin),
        card(&theme, "Pulse", pulse),
        card(&theme, "Fade (as a group)", pair),
    ])
    .gap(theme.spacing.md);

    let clicks = state.clicks;
    let tilted = button_labeled(&theme, format!("Click me ({clicks})"))
        .width(180.0)
        .height(48.0)
        .transform(Transform::IDENTITY.rotate_degrees(-8.0))
        .on_tap(cx, |s: &mut State| {
            s.clicks += 1;
            println!("transformdemo: clicked {} times", s.clicks);
        });
    let paused = state.held.is_some();
    let pause = button_labeled(&theme, if paused { "Play" } else { "Pause" })
        .width(100.0)
        .height(36.0)
        .on_tap(cx, |s: &mut State| match s.held.take() {
            // Resume from where the motion stood.
            Some(at) => s.offset = s.last_frame.get().saturating_sub(at),
            None => s.held = Some(s.motion_time()),
        });

    column(vec![
        heading(&theme, "Transforms and opacity"),
        cards,
        row(vec![tilted, pause])
            .gap(theme.spacing.lg * 2.0)
            .padding(Insets::symmetric(theme.spacing.md, 0.0))
            .align(Align::Start, Align::Center),
    ])
    .gap(theme.spacing.lg)
    .padding(Insets::uniform(theme.spacing.lg))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut held = None;
    while let Some(arg) = args.next() {
        if arg == "--frame" {
            let secs: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            held = Some(Duration::from_secs_f64(secs.max(0.0)));
        }
    }
    let state = State {
        clicks: 0,
        held,
        offset: Duration::ZERO,
        last_frame: Cell::new(Duration::ZERO),
    };
    let mut app = App::new(state, view)
        .title("Stipple Transforms")
        .theme(Theme::dark())
        .logical_size(Size::new(620.0, 360.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app.run();
}
