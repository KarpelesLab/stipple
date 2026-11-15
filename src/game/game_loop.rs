//! The game loop: an update that runs once per display frame while the game
//! plays, without rebuilding the view.

use super::InputState;
use crate::App;
use crate::core::{CanvasId, CanvasInvalidation};
use crate::geometry::Rect;
use crate::platform::Event;
use std::time::Duration;

/// The update a [`GameLoop`] runs each frame.
pub type UpdateFn<S> = Box<dyn FnMut(&mut S, &mut Frame<'_>)>;

/// When a [`GameLoop`] is paused, from the state.
pub type PausedFn<S> = Box<dyn Fn(&S) -> bool>;

/// A per-frame update for [`App::game_loop`].
///
/// While the game runs, `update` is called once per display frame, with the
/// state and a [`Frame`]: the time, the time since the last frame, the
/// input, and the stats. The view isn't rebuilt for it: the update says
/// what changed with [`Frame::repaint`] (a canvas: only that is painted
/// again) or [`Frame::rebuild`] (the view reads something that changed: a
/// score, a menu).
///
/// The loop stops, and asks for no frames, while it is paused:
///
/// - while [`paused_when`](Self::paused_when) says so (a pause menu);
/// - while every window is minimized, or the app is in the background;
/// - with [`pause_when_unfocused`](Self::pause_when_unfocused), while no
///   window has the keyboard.
///
/// On resuming, the first frame's `dt` is one refresh interval, not the
/// time paused, and presses made while paused (the key that unpaused) are
/// forgotten.
pub struct GameLoop<S> {
    pub(crate) update: UpdateFn<S>,
    pub(crate) paused: Option<PausedFn<S>>,
    pub(crate) pause_unfocused: bool,
    pub(crate) max_dt: Duration,
}

impl<S> std::fmt::Debug for GameLoop<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GameLoop")
            .field("paused_when", &self.paused.is_some())
            .field("pause_unfocused", &self.pause_unfocused)
            .field("max_dt", &self.max_dt)
            .finish_non_exhaustive()
    }
}

impl<S> GameLoop<S> {
    /// A loop running `update` every frame.
    pub fn new(update: impl FnMut(&mut S, &mut Frame<'_>) + 'static) -> GameLoop<S> {
        GameLoop {
            update: Box::new(update),
            paused: None,
            pause_unfocused: false,
            max_dt: Duration::from_millis(100),
        }
    }

    /// Pause while `paused` returns `true` for the state. It is asked after
    /// every event and update, so setting a flag in the state from a button,
    /// a key handler or the update itself pauses and resumes the loop.
    pub fn paused_when(mut self, paused: impl Fn(&S) -> bool + 'static) -> GameLoop<S> {
        self.paused = Some(Box::new(paused));
        self
    }

    /// Also pause while no window of the app has keyboard focus.
    pub fn pause_when_unfocused(mut self, pause: bool) -> GameLoop<S> {
        self.pause_unfocused = pause;
        self
    }

    /// The longest `dt` a frame gets (100 ms unless set): after a hitch the
    /// game slows down instead of jumping ahead.
    pub fn max_dt(mut self, max: Duration) -> GameLoop<S> {
        self.max_dt = max;
        self
    }
}

/// One frame of a [`GameLoop`], handed to its update.
#[derive(Debug)]
pub struct Frame<'a> {
    time: Duration,
    dt: Duration,
    raw_dt: Duration,
    index: u64,
    input: &'a InputState,
    stats: LoopStats,
    repaint: Vec<CanvasInvalidation>,
    rebuild: bool,
}

impl<'a> Frame<'a> {
    /// A frame at `time` (the frame clock's timeline,
    /// [`Cx::frame_time`](crate::core::Cx::frame_time)'s), `dt` after the
    /// last, with `input`: for running an update outside the loop, in tests.
    pub fn new(time: Duration, dt: Duration, input: &'a InputState) -> Frame<'a> {
        Frame {
            time,
            dt,
            raw_dt: dt,
            index: 0,
            input,
            stats: LoopStats::default(),
            repaint: Vec::new(),
            rebuild: false,
        }
    }

    /// The frame's time, on the frame clock's timeline (the time
    /// animations sample).
    pub fn time(&self) -> Duration {
        self.time
    }

    /// The time since the last frame, at most the loop's
    /// [`max_dt`](GameLoop::max_dt). The first frame after the loop starts
    /// or resumes gets one refresh interval.
    pub fn dt(&self) -> Duration {
        self.dt
    }

    /// [`dt`](Self::dt) in seconds.
    pub fn dt_secs(&self) -> f64 {
        self.dt.as_secs_f64()
    }

    /// The time since the last frame as it was, before
    /// [`max_dt`](GameLoop::max_dt) capped it.
    pub fn raw_dt(&self) -> Duration {
        self.raw_dt
    }

    /// How many frames the loop ran before this one.
    pub fn index(&self) -> u64 {
        self.index
    }

    /// The keyboard and mouse: what is held, and what was pressed and
    /// released since the last frame.
    pub fn input(&self) -> &InputState {
        self.input
    }

    /// The loop's frame rate and timings so far.
    pub fn stats(&self) -> LoopStats {
        self.stats
    }

    /// Paint canvas `id` again, without rebuilding the view.
    pub fn repaint(&mut self, id: CanvasId) {
        self.repaint.push((id, None));
    }

    /// Paint `rect` (in the canvas's coordinates) of canvas `id` again.
    pub fn repaint_rect(&mut self, id: CanvasId, rect: Rect) {
        self.repaint.push((id, Some(rect)));
    }

    /// Rebuild the view after this update: something it shows changed.
    pub fn rebuild(&mut self) {
        self.rebuild = true;
    }

    /// Whether [`rebuild`](Self::rebuild) was called.
    pub fn rebuilds(&self) -> bool {
        self.rebuild
    }

    /// The canvas areas asked to repaint so far.
    pub fn repaints(&self) -> impl Iterator<Item = (CanvasId, Option<Rect>)> + '_ {
        self.repaint.iter().copied()
    }
}

/// How the game loop is doing, for an fps counter or a debug overlay. Means
/// are over the last half second or so.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoopStats {
    /// Frames per second.
    pub fps: f64,
    /// The mean time between frames.
    pub frame_time: Duration,
    /// The mean time the update took.
    pub update_time: Duration,
    /// The mean time presenting a frame took (building the view when it is
    /// rebuilt, painting, rasterizing and handing it to the window), over
    /// every window of the process.
    pub render_time: Duration,
    /// Frames run since the loop was set up.
    pub frames: u64,
}

/// Collects [`LoopStats`] over windows of [`WINDOW`].
#[derive(Default)]
struct StatsTracker {
    shown: LoopStats,
    frames: u32,
    span: Duration,
    update: Duration,
    /// The process's present count and time when the window began.
    presents: (u64, Duration),
}

/// How long the stats are averaged over.
const WINDOW: Duration = Duration::from_millis(500);

impl StatsTracker {
    fn record(&mut self, dt: Duration, update: Duration) {
        self.shown.frames += 1;
        self.frames += 1;
        self.span += dt;
        self.update += update;
        if self.span >= WINDOW {
            let n = self.frames;
            let p = crate::platform::frame::stats();
            let presents = p.presents.saturating_sub(self.presents.0);
            let took = p.present_time.saturating_sub(self.presents.1);
            self.shown.fps = f64::from(n) / self.span.as_secs_f64();
            self.shown.frame_time = self.span / n;
            self.shown.update_time = self.update / n;
            self.shown.render_time = if presents == 0 {
                Duration::ZERO
            } else {
                took / u32::try_from(presents).unwrap_or(u32::MAX)
            };
            self.frames = 0;
            self.span = Duration::ZERO;
            self.update = Duration::ZERO;
            self.presents = (p.presents, p.present_time);
        }
    }
}

/// A game loop installed on an app, and what it keeps between frames.
pub(crate) struct Runtime<S> {
    looper: GameLoop<S>,
    input: InputState,
    /// The last frame's time, while running.
    last: Option<Duration>,
    /// The frame clock's delivery count when the loop last ran a frame (or
    /// resumed): a different count on a wake means a frame came.
    seen: u64,
    running: bool,
    stats: StatsTracker,
    /// Frames asked for, for tests.
    pub(crate) requests: std::rc::Rc<std::cell::Cell<u64>>,
}

/// How many frames the frame clock has delivered.
fn delivered() -> u64 {
    let s = crate::platform::frame::stats();
    s.from_display + s.from_timer
}

impl<S: 'static> App<S> {
    /// Run `update` once per display frame (see [`GameLoop`]). The same as
    /// `.game_loop(GameLoop::new(update))`.
    ///
    /// ```no_run
    /// use stipple::prelude::*;
    /// use stipple::game::Frame;
    /// use std::cell::Cell;
    /// use std::rc::Rc;
    ///
    /// const WORLD: CanvasId = CanvasId(1);
    /// struct Game { x: Rc<Cell<f64>> }
    ///
    /// let x = Rc::new(Cell::new(0.0));
    /// let shared = x.clone();
    /// App::new(Game { x }, move |_: &Game, _: &mut Cx<Game>| {
    ///     let x = shared.clone();
    ///     Element::canvas(WORLD, move |c: &mut Canvas| {
    ///         let r = Rect::from_xywh(x.get(), 10.0, 20.0, 20.0);
    ///         c.fill_rect(r, Color::rgb(200, 40, 40));
    ///     })
    ///     .grow(1.0)
    /// })
    /// .on_frame(|g: &mut Game, frame: &mut Frame| {
    ///     g.x.set((g.x.get() + 60.0 * frame.dt_secs()) % 400.0);
    ///     frame.repaint(WORLD); // only the canvas changed
    /// })
    /// .run();
    /// ```
    pub fn on_frame(self, update: impl FnMut(&mut S, &mut Frame<'_>) + 'static) -> Self {
        self.game_loop(GameLoop::new(update))
    }

    /// Run a [`GameLoop`] on the app's frames, replacing any set before.
    pub fn game_loop(mut self, looper: GameLoop<S>) -> Self {
        self.game = Some(Box::new(Runtime {
            looper,
            input: InputState::new(),
            last: None,
            seen: 0,
            running: false,
            stats: StatsTracker::default(),
            requests: Default::default(),
        }));
        self
    }

    /// Take what the game's input needs from a window event.
    pub(crate) fn game_event(&mut self, event: &Event) {
        if let Some(game) = self.game.as_mut() {
            game.input.event(event);
        }
    }

    /// Whether the game loop should run now.
    fn game_should_run(&self) -> bool {
        let Some(game) = self.game.as_ref() else {
            return false;
        };
        let manual = game.looper.paused.as_ref().is_some_and(|p| p(&self.state));
        let hidden = self.in_background()
            || crate::platform::frame::suspended()
            || self.panes.iter().all(|p| p.info.minimized);
        let unfocused = game.looper.pause_unfocused && !self.panes.iter().any(|p| p.window_focused);
        !(manual || hidden || unfocused)
    }

    /// Follow pausing and resuming, and keep asking for frames while the
    /// game runs. Called after anything that may have changed the state.
    pub(crate) fn game_schedule(&mut self) {
        let run = self.game_should_run();
        let Some(game) = self.game.as_mut() else {
            return;
        };
        if run != game.running {
            game.running = run;
            game.last = None;
            if run {
                // Presses made while paused aren't the game's, and the frame
                // to run is the next one delivered.
                game.input.end_update();
                game.seen = delivered();
            }
        }
        if run {
            game.requests.set(game.requests.get() + 1);
            crate::platform::frame::request();
        }
    }

    /// On a wake: run the update if a frame came. Returns whether it asked
    /// for the view to be rebuilt.
    pub(crate) fn game_wake(&mut self) -> bool {
        self.game_schedule();
        let Some(game) = self.game.as_mut() else {
            return false;
        };
        let count = delivered();
        if !game.running || count == game.seen {
            return false;
        }
        game.seen = count;
        let now = crate::platform::frame::elapsed().unwrap_or_default();
        let rebuild = self.game_frame_at(now);
        self.game_schedule();
        rebuild
    }

    /// Run the update for a frame at `now`, and hand its repaints to the
    /// windows. Returns whether it asked for a rebuild.
    pub(crate) fn game_frame_at(&mut self, now: Duration) -> bool {
        let Some(game) = self.game.as_mut() else {
            return false;
        };
        let raw = match game.last {
            Some(last) => now.saturating_sub(last),
            None => crate::platform::frame::interval(),
        };
        game.last = Some(now);
        let mut frame = Frame {
            time: now,
            dt: raw.min(game.looper.max_dt),
            raw_dt: raw,
            index: game.stats.shown.frames,
            input: &game.input,
            stats: game.stats.shown,
            repaint: Vec::new(),
            rebuild: false,
        };
        let began = crate::platform::time::Instant::now();
        (game.looper.update)(&mut self.state, &mut frame);
        let (dt, repaint, rebuild) = (frame.dt, frame.repaint, frame.rebuild);
        game.stats.record(dt, began.elapsed());
        game.input.end_update();
        for pane in &mut self.panes {
            pane.canvas_pending.extend(repaint.iter().copied());
            pane.dirty |= rebuild;
        }
        rebuild
    }
}
