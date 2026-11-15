//! "Lampwick": a small game on Stipple's Phase 9 pieces.
//!
//! Wick keeps a lantern in a night garden. Embers drift down from above;
//! catching them scores (more for several in a row) and feeds the lantern,
//! which burns down on its own. Moths wander the dark and, once they see
//! the light, fly at it: each that reaches Wick costs a heart. A flare
//! spends light to burst every moth within reach. At no light at all the
//! cold bites. Five hearts; the best score is kept.
//!
//! It is drawn at 320×180 (a `PixelViewport`, a `Camera2D` that shakes on a
//! hit, a sprite sheet and a tilemap painted at startup), played with the
//! keyboard, the mouse, a gamepad or the touch controls through rebindable
//! `Actions`, with synthesized sound and music, a title menu, a settings
//! screen, a HUD, a pause menu and a game-over screen built from
//! `stipple::game::ui`, and settings, controls and the best score saved
//! with `stipple::game::save`. On the web it runs as `game.html`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use stipple::audio::Audio;
use stipple::core::CanvasId;
use stipple::game::save::{KeyValues, SaveStore};
use stipple::game::ui::{
    self as gui, Bar, BindingRowState, Capture, FloatingTexts, GameOverScreen, GameStyle,
    RebindCapture, TitleScreen,
};
use stipple::game::{
    Actions, AxisDirection, Binding, Corner, FixedStep, Frame, GameLoop, InputDevice, TouchButton,
    TouchControls, TouchOverlay, TouchStick,
};
use stipple::gamepad::{GamepadAxis, GamepadButton, GamepadEvent};
use stipple::geometry::Vec2;
use stipple::platform::AppInfo;
use stipple::prelude::*;

pub mod art;
pub mod sound;
mod ui;
pub mod world;

pub use ui::{app, app_info, make_app, style, view};

stipple::web_app!(make_app);

use art::Stage;
use sound::Sounds;
use world::{Control, Event, V2, World};

pub const WORLD: CanvasId = CanvasId(1);
const TOUCH: CanvasId = CanvasId(2);
pub const WINDOW: Size = Size::new(1280.0, 720.0);
/// The simulation's rate.
pub const STEP_HZ: u32 = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Act {
    Left,
    Right,
    Up,
    Down,
    Flare,
}

/// Each action's name in the saved controls, and on the settings screen.
pub const ACTIONS: [(Act, &str, &str); 5] = [
    (Act::Left, "left", "Move left"),
    (Act::Right, "right", "Move right"),
    (Act::Up, "up", "Move up"),
    (Act::Down, "down", "Move down"),
    (Act::Flare, "flare", "Flare"),
];

fn label_of(a: Act) -> &'static str {
    ACTIONS.iter().find(|x| x.0 == a).map_or("?", |x| x.2)
}

pub fn default_actions() -> Actions<Act> {
    use PhysicalKey as K;
    let half = |axis, direction| Binding::GamepadAxis {
        axis,
        direction,
        dead_zone: 0.2,
    };
    let (neg, pos) = (AxisDirection::Negative, AxisDirection::Positive);
    Actions::new()
        .with(Act::Left, Binding::Key(K::ARROW_LEFT))
        .with(Act::Left, Binding::Key(K::KEY_A))
        .with(Act::Left, half(GamepadAxis::LeftX, neg))
        .with(Act::Left, Binding::GamepadButton(GamepadButton::DPadLeft))
        .with(Act::Right, Binding::Key(K::ARROW_RIGHT))
        .with(Act::Right, Binding::Key(K::KEY_D))
        .with(Act::Right, half(GamepadAxis::LeftX, pos))
        .with(Act::Right, Binding::GamepadButton(GamepadButton::DPadRight))
        .with(Act::Up, Binding::Key(K::ARROW_UP))
        .with(Act::Up, Binding::Key(K::KEY_W))
        .with(Act::Up, half(GamepadAxis::LeftY, neg))
        .with(Act::Up, Binding::GamepadButton(GamepadButton::DPadUp))
        .with(Act::Down, Binding::Key(K::ARROW_DOWN))
        .with(Act::Down, Binding::Key(K::KEY_S))
        .with(Act::Down, half(GamepadAxis::LeftY, pos))
        .with(Act::Down, Binding::GamepadButton(GamepadButton::DPadDown))
        .with(Act::Flare, Binding::Key(K::SPACE))
        .with(Act::Flare, Binding::MouseButton(PointerButton::Left))
        .with(Act::Flare, Binding::GamepadButton(GamepadButton::South))
}

/// A stick for the left thumb, Flare for the right, pause at the top.
fn touch_controls() -> TouchControls {
    TouchControls::new()
        .stick(TouchStick::left().at(Corner::BottomLeft, Vec2::new(110.0, 110.0)))
        .button(TouchButton::new(GamepadButton::South, "Flare").radius(48.0))
        .button(
            TouchButton::new(GamepadButton::Start, "II")
                .at(Corner::TopRight, Vec2::new(40.0, 110.0))
                .radius(24.0),
        )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Title,
    Settings,
    Play,
    Over,
}

const SCALES: [&str; 4] = ["Auto", "2\u{d7}", "3\u{d7}", "4\u{d7}"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub master: f32,
    pub music: f32,
    pub effects: f32,
    pub fullscreen: bool,
    /// An index into `SCALES`.
    pub scale: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            master: 0.8,
            music: 0.6,
            effects: 0.9,
            fullscreen: false,
            scale: 0,
        }
    }
}

impl Settings {
    fn to_values(self) -> KeyValues {
        let mut kv = KeyValues::new();
        kv.set("master", self.master);
        kv.set("music", self.music);
        kv.set("effects", self.effects);
        kv.set("fullscreen", self.fullscreen);
        kv.set("pixel_scale", self.scale);
        kv
    }

    fn from_values(kv: &KeyValues) -> Settings {
        let d = Settings::default();
        Settings {
            master: kv.get_or("master", d.master).clamp(0.0, 1.0),
            music: kv.get_or("music", d.music).clamp(0.0, 1.0),
            effects: kv.get_or("effects", d.effects).clamp(0.0, 1.0),
            fullscreen: kv.get_or("fullscreen", d.fullscreen),
            scale: kv.get_or("pixel_scale", d.scale).min(SCALES.len() - 1),
        }
    }
}

#[derive(Debug)]
pub struct Game {
    pub screen: Screen,
    settings_back: Screen,
    pub paused: bool,
    pub settings: Settings,
    pub actions: Actions<Act>,
    pub capture: Option<RebindCapture<Act>>,
    pub best: u32,
    pub games: u32,
    pub new_best: bool,
    pub store: SaveStore,
    /// The seed of the next game (a selftest fixes it).
    pub seed: u64,
    pub stage: Rc<RefCell<Stage>>,
    step: FixedStep,
    flare_queued: bool,
    /// The light shown on the HUD, in twentieths (it rebuilds on a change).
    light_shown: u32,
    pub floaters: FloatingTexts,
    pub sounds: Sounds,
    touch: TouchOverlay,
    events: Vec<Event>,
}

impl Game {
    /// A game reading its settings, controls and best score from `store`.
    pub fn new(store: SaveStore, audio: Audio, seed: u64) -> Game {
        let settings = match store.load_text("settings") {
            Ok(Some(kv)) => Settings::from_values(&kv),
            _ => Settings::default(),
        };
        let mut actions = default_actions();
        if let Ok(Some(saved)) = store.load("controls") {
            let text = String::from_utf8_lossy(&saved.bytes);
            let of = |name: &str| ACTIONS.iter().find(|x| x.1 == name).map(|x| x.0);
            if let Err(e) = actions.load_text(&text, of) {
                eprintln!("lampwick: the saved controls are unreadable ({e}); using the defaults");
            }
        }
        let records = store
            .load_text("records")
            .ok()
            .flatten()
            .unwrap_or_default();
        // A backdrop for the title: a garden a few seconds into a game.
        let mut backdrop = World::new(seed ^ 0x5eed);
        let mut events = Vec::new();
        for _ in 0..(STEP_HZ * 7) {
            let ctl = world::autopilot(&backdrop, f32::MAX);
            backdrop.step(ctl, 1.0 / STEP_HZ as f32, &mut events);
        }
        let mut sounds = Sounds::new(audio);
        sounds.set_volumes(settings.master, settings.music, settings.effects);
        sounds.music();
        let mut stage = Stage::new(backdrop);
        stage.vp.max_factor = max_factor(settings.scale);
        Game {
            screen: Screen::Title,
            settings_back: Screen::Title,
            paused: false,
            settings,
            actions,
            capture: None,
            best: records.get_or("best", 0),
            games: records.get_or("games", 0),
            new_best: false,
            store,
            seed,
            stage: Rc::new(RefCell::new(stage)),
            step: FixedStep::hz(STEP_HZ),
            flare_queued: false,
            light_shown: 0,
            floaters: FloatingTexts::new(),
            sounds,
            touch: TouchOverlay::new(TOUCH).font(Font::system_default()),
            events: Vec::new(),
        }
    }

    pub fn start(&mut self) {
        self.screen = Screen::Play;
        self.paused = false;
        self.new_best = false;
        self.flare_queued = false;
        self.floaters = FloatingTexts::new();
        self.step.reset();
        let seed = self.seed.wrapping_add(u64::from(self.games));
        let mut stage = self.stage.borrow_mut();
        stage.world = World::new(seed);
        self.light_shown = light_twentieths(&stage.world);
    }

    fn open_settings(&mut self) {
        self.settings_back = self.screen;
        self.screen = Screen::Settings;
    }

    /// Save the settings (each change is saved as it's made).
    pub fn save_settings(&self) {
        report(self.store.save_text("settings", &self.settings.to_values()));
    }

    pub fn save_controls(&self) {
        let text = self
            .actions
            .to_text(|a| ACTIONS.iter().find(|x| x.0 == a).map_or("?", |x| x.1));
        report(self.store.save("controls", text.as_bytes()));
    }

    fn save_records(&self) {
        let mut kv = KeyValues::new();
        kv.set("best", self.best);
        kv.set("games", self.games);
        report(self.store.save_text("records", &kv));
    }

    fn apply_volumes(&self) {
        let s = self.settings;
        self.sounds.set_volumes(s.master, s.music, s.effects);
    }

    /// The game's state ended: count it, keep the best.
    fn game_over(&mut self) {
        let score = self.stage.borrow().world.score;
        self.games += 1;
        if score > self.best {
            self.best = score;
            self.new_best = true;
        }
        self.save_records();
        self.sounds.over();
        self.screen = Screen::Over;
    }
}

fn report(result: std::io::Result<()>) {
    if let Err(e) = result {
        eprintln!("lampwick: couldn't save: {e}");
    }
}

fn max_factor(scale: usize) -> Option<u32> {
    [None, Some(2), Some(3), Some(4)][scale.min(3)]
}

fn light_twentieths(w: &World) -> u32 {
    (w.light / world::MAX_LIGHT * 20.0).ceil() as u32
}

pub fn update(g: &mut Game, frame: &mut Frame) {
    // A controls row waiting for an input takes the next one.
    if let Some(capture) = g.capture.as_mut() {
        match capture.poll(frame.input(), frame.time()) {
            Capture::Cancelled => {
                g.capture = None;
                frame.rebuild();
            }
            Capture::Bound(binding) => {
                capture.apply(&mut g.actions, binding);
                g.capture = None;
                g.save_controls();
                frame.rebuild();
            }
            // Still waiting.
            _ => {}
        }
        return;
    }
    if g.screen != Screen::Play || g.paused {
        return;
    }
    g.touch.sync(frame);
    g.actions.update(frame.input());
    // Start on any pad (the touch controls' "II" too) pauses.
    if frame.input().gamepad_pressed(None, GamepadButton::Start) {
        g.paused = true;
        frame.rebuild();
        return;
    }
    if g.actions.pressed(Act::Flare) {
        g.flare_queued = true;
    }
    let (dx, dy) = g
        .actions
        .axis2((Act::Left, Act::Right), (Act::Up, Act::Down));
    let control = |flare| Control {
        dir: V2::new(dx, dy),
        flare,
    };
    let steps = g.step.advance(frame.dt());
    let dt = g.step.dt_secs() as f32;
    let mut stage = g.stage.borrow_mut();
    for _ in 0..steps {
        let flare = std::mem::take(&mut g.flare_queued);
        stage.world.step(control(flare), dt, &mut g.events);
    }
    let mut over = false;
    for event in g.events.drain(..) {
        let now = frame.time();
        match event {
            Event::Ember { at, points, combo } => {
                g.sounds.ember(at.x, combo);
                let text = if combo > 1 {
                    format!("+{points} \u{d7}{combo}")
                } else {
                    format!("+{points}")
                };
                let p = stage.to_canvas(V2::new(at.x - 6.0, at.y - 22.0));
                g.floaters.spawn(text, Color::rgb(0xff, 0xc8, 0x5a), p, now);
            }
            Event::Hit { at } | Event::Cold { at } => {
                g.sounds.hit(at.x);
                stage.cam.add_trauma(0.6);
                let p = stage.to_canvas(V2::new(at.x + 4.0, at.y - 18.0));
                let text = if matches!(event, Event::Cold { .. }) {
                    "cold!"
                } else {
                    "-1"
                };
                g.floaters.spawn(text, Color::rgb(0xff, 0x5a, 0x4e), p, now);
                // A bump on the pads that rumble (the last device is a pad).
                if frame.input().last_device() == Some(InputDevice::Gamepad) {
                    for (id, _) in stipple::gamepad::gamepads() {
                        stipple::gamepad::set_rumble(id, 0.6, 0.3, Duration::from_millis(180));
                    }
                }
            }
            Event::Flare { at, burst } => {
                g.sounds.flare();
                stage.cam.add_trauma(0.25);
                if burst > 0 {
                    let p = stage.to_canvas(V2::new(at.x - 10.0, at.y - 30.0));
                    g.floaters.spawn(
                        format!("+{}", 25 * burst),
                        Color::rgb(0xe0, 0xd8, 0xff),
                        p,
                        now,
                    );
                }
            }
            Event::Fizzle => g.sounds.fizzle(),
            Event::Over => over = true,
        }
        frame.rebuild();
    }
    stage.cam.update(frame.dt_secs());
    let light = light_twentieths(&stage.world);
    if light != g.light_shown {
        g.light_shown = light;
        frame.rebuild();
    }
    drop(stage);
    if g.floaters.update(frame.time()) {
        frame.rebuild();
    }
    if over {
        g.game_over();
        frame.rebuild();
    }
    frame.repaint(WORLD);
}
