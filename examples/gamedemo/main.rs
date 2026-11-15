//! Lampwick, natively: `gamedemo` plays it in a window.
//!
//! - `--selftest`: plays a seeded game with scripted keys at a fixed 60 Hz,
//!   off screen, through the menus and the game's own update; checks the
//!   score it reaches, that the best score and the settings and controls
//!   were saved and read back; prints a summary; exits non-zero on failure.
//!   Sound goes to the default output (`STIPPLE_AUDIO=null` for none).
//! - `--shots DIR`: renders the screens off screen into `DIR/*.png`.

use std::path::{Path, PathBuf};
use std::time::Duration;

// The game itself: lib.rs, which the `lampwick` example also builds as the
// wasm module game.html runs.
#[path = "lib.rs"]
mod lampwick;

use lampwick::{Act, Game, STEP_HZ, Screen, app, update, world};
use stipple::audio::Audio;
use stipple::game::save::SaveStore;
use stipple::game::ui::RebindCapture;
use stipple::game::{Binding, Frame, InputState};
use stipple::gamepad::GamepadButton;
use stipple::platform::{ButtonState, KeyEvent};
use stipple::prelude::*;

/// The selftest's garden, and what it must score in it.
const SEED: u64 = 0x1a4b_9c2d;
const EXPECT_SCORE: u32 = 3805;
/// When the scripted player stops dodging.
const TIRED_AT: f32 = 75.0;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--selftest") {
        match selftest() {
            Ok(line) => println!("gamedemo selftest: ok: {line}"),
            Err(line) => {
                println!("gamedemo selftest: FAILED: {line}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--shots") {
        let dir = args.get(i + 1).cloned().unwrap_or_else(|| ".".into());
        shots(Path::new(&dir));
        return;
    }
    lampwick::make_app().run();
}

/// Keys the scripted player holds, fed to an `InputState` as events.
struct Keys {
    input: InputState,
    held: Vec<(PhysicalKey, KeyCode)>,
}

impl Keys {
    fn set(&mut self, want: &[(PhysicalKey, KeyCode)]) {
        for k in self.held.clone() {
            if !want.contains(&k) {
                self.input
                    .key(&KeyEvent::new(ButtonState::Released, k.1, k.0));
            }
        }
        for k in want {
            if !self.held.contains(k) {
                self.input
                    .key(&KeyEvent::new(ButtonState::Pressed, k.1, k.0));
            }
        }
        self.held = want.to_vec();
    }
}

fn check(ok: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what()) }
}

fn temp_store() -> (SaveStore, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lampwick-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (SaveStore::new(&dir), dir)
}

fn selftest() -> Result<String, String> {
    let (store, dir) = temp_store();
    let result = run_selftest(&store);
    let _ = std::fs::remove_dir_all(dir);
    result
}

fn run_selftest(store: &SaveStore) -> Result<String, String> {
    let audio = Audio::shared();

    // 1. The menus, through the app off screen: Enter on the title plays,
    // Escape pauses and resumes.
    let mut a = app(Game::new(store.clone(), audio.clone(), SEED));
    a.render_once();
    a.render_once();
    a.press_key(KeyInput::Enter);
    a.render_once();
    check(a.state().screen == Screen::Play, || {
        format!("Enter on the title: {:?}", a.state().screen)
    })?;
    a.press_key_code(KeyCode::Escape, Modifiers::default());
    a.render_once();
    check(a.state().paused, || "Escape didn't pause".into())?;
    a.press_key_code(KeyCode::Escape, Modifiers::default());
    a.render_once();
    check(!a.state().paused, || "Escape didn't resume".into())?;
    drop(a);

    // 2. A game, played by script through the game's update at 60 Hz.
    let mut g = Game::new(store.clone(), audio.clone(), SEED);
    g.start();
    let mut keys = Keys {
        input: InputState::new(),
        held: Vec::new(),
    };
    let dt = Duration::from_nanos(1_000_000_000 / 60);
    let mut time = Duration::from_secs(1);
    let mut frames = 0u32;
    let mut flares = 0u32;
    while g.screen == Screen::Play && frames < 60 * 300 {
        let ctl = world::autopilot(&g.stage.borrow().world, TIRED_AT);
        let mut want = Vec::new();
        let (d, k) = (ctl.dir, |p, c| (p, c));
        if d.x < -0.3 {
            want.push(k(PhysicalKey::ARROW_LEFT, KeyCode::ArrowLeft));
        }
        if d.x > 0.3 {
            want.push(k(PhysicalKey::ARROW_RIGHT, KeyCode::ArrowRight));
        }
        if d.y < -0.3 {
            want.push(k(PhysicalKey::ARROW_UP, KeyCode::ArrowUp));
        }
        if d.y > 0.3 {
            want.push(k(PhysicalKey::ARROW_DOWN, KeyCode::ArrowDown));
        }
        // A tap: pressed this frame, let go the next.
        let space = (PhysicalKey::SPACE, KeyCode::Space);
        if ctl.flare && !keys.held.contains(&space) {
            want.push(space);
            flares += 1;
        }
        keys.set(&want);
        let mut frame = Frame::new(time, dt, &keys.input);
        update(&mut g, &mut frame);
        keys.input.end_update();
        time += dt;
        frames += 1;
    }
    let (score, caught, burst, survived) = {
        let s = g.stage.borrow();
        (s.world.score, s.world.caught, s.world.burst, s.world.time)
    };
    let summary = format!(
        "score {score} ({caught} embers, {burst} moths burst, {flares} flares) in {survived:.1} s over {frames} frames"
    );
    check(g.screen == Screen::Over, || {
        format!("the game didn't end: {summary}")
    })?;
    check(score == EXPECT_SCORE, || {
        format!("expected a score of {EXPECT_SCORE}: {summary}")
    })?;
    check(g.best == score && g.new_best && g.games == 1, || {
        format!(
            "best {} (new: {}), {} games after {summary}",
            g.best, g.new_best, g.games
        )
    })?;

    // 3. Settings and controls: change them as the settings screen does,
    // with a "press a key…" for Flare.
    g.settings.music = 0.25;
    g.settings.scale = 2;
    g.settings.fullscreen = true;
    g.save_settings();
    g.capture = Some(RebindCapture::by_device(Act::Flare));
    for key in [None, Some((PhysicalKey::KEY_F, KeyCode::Char('f')))] {
        if let Some(k) = key {
            keys.set(&[k]);
        }
        let mut frame = Frame::new(time, dt, &keys.input);
        update(&mut g, &mut frame);
        keys.input.end_update();
        time += dt;
    }
    check(g.capture.is_none(), || "the capture didn't take F".into())?;
    let saved_settings = g.settings;
    let saved_flare = g.actions.bindings(Act::Flare).to_vec();
    drop(g);

    // 4. Everything comes back in a new game.
    let g = Game::new(store.clone(), audio, SEED);
    check(g.best == score && g.games == 1, || {
        format!("records read back: best {}, {} games", g.best, g.games)
    })?;
    check(g.settings == saved_settings, || {
        format!("settings read back: {:?}", g.settings)
    })?;
    let flare = g.actions.bindings(Act::Flare);
    check(
        flare == saved_flare.as_slice()
            && flare[0] == Binding::Key(PhysicalKey::KEY_F)
            && flare.contains(&Binding::GamepadButton(GamepadButton::South)),
        || format!("controls read back: {flare:?}"),
    )?;
    let records = store.load("records").map_err(|e| e.to_string())?;
    check(records.is_some_and(|r| !r.from_backup), || {
        "the records file".into()
    })?;
    let names = store.names().map_err(|e| e.to_string())?;
    Ok(format!("{summary}; saved {}", names.join(", ")))
}

/// Render each screen off screen into `dir`.
fn shots(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let audio = Audio::offline(48_000);
    let render = |name: &str, game: Game, keys: &[KeyInput]| {
        let mut a = app(game);
        a.render_once();
        a.render_once();
        for k in keys {
            a.press_key(k.clone());
            a.render_once();
        }
        let mut frame = a.render_once();
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(10));
            frame = a.render_once();
        }
        let path = dir.join(format!("{name}.png"));
        frame.save_png(&path).expect("write the screenshot");
        println!("{}", path.display());
    };
    let game = || Game::new(SaveStore::memory(), audio.clone(), SEED);
    render("title", game(), &[]);
    let mut g = game();
    g.screen = Screen::Settings;
    g.capture = Some(RebindCapture::by_device(Act::Flare));
    render("settings", g, &[KeyInput::Down]);
    // A game some seconds in, played by the script.
    let played = |secs: u32| {
        let mut g = game();
        g.start();
        {
            let mut stage = g.stage.borrow_mut();
            let mut events = Vec::new();
            for _ in 0..secs * STEP_HZ {
                let ctl = world::autopilot(&stage.world, f32::MAX);
                stage.world.step(ctl, 1.0 / STEP_HZ as f32, &mut events);
            }
        }
        g
    };
    let mut g = played(24);
    {
        let mut stage = g.stage.borrow_mut();
        stage.world.health = 3;
        stage.world.player = world::V2::new(150.0, 104.0);
        let p = stage.world.player;
        let at = stage.to_canvas(world::V2::new(p.x - 6.0, p.y - 22.0));
        g.floaters.spawn(
            "+30 \u{d7}3",
            Color::rgb(0xff, 0xc8, 0x5a),
            at,
            Duration::ZERO,
        );
    }
    render("play", g, &[]);
    let mut g = played(10);
    g.paused = true;
    render("pause", g, &[]);
    let mut g = played(30);
    g.screen = Screen::Over;
    g.best = g.stage.borrow().world.score;
    g.new_best = true;
    render("over", g, &[]);
}
