//! A game's UI on Stipple's widgets: "Starlit Courier", a little courier
//! ship weaving through a comet belt (a placeholder game drawn in a
//! canvas), with
//!
//! - a title menu,
//! - a settings screen: three volume sliders, rebindable controls for four
//!   actions ("press a key…", with conflicts flagged), fullscreen, a pixel
//!   scale and a language,
//! - the game with a HUD over it (hull and boost bars, a score that pops,
//!   a minimap slot, floating "+10"/"-12" texts), a dialogue box typing out
//!   the dispatcher's lines, and a pause menu over the frozen game,
//! - a game-over screen.
//!
//! Everything is reachable with the keyboard (arrows, Enter, Escape) and a
//! gamepad (d-pad or left stick, A/✕ to select, B/○ to go back, Start to
//! pause), as well as the mouse.
//!
//! `--shots DIR` renders each screen off screen into `DIR/*.png` and quits.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use stipple::core::CanvasId;
use stipple::game::ui::{
    self as gui, Bar, BindingRowState, Capture, DialogueBox, FloatingTexts, GameOverScreen,
    GameStyle, RebindCapture, TitleScreen, Typewriter,
};
use stipple::game::{Actions, Binding, Frame, GameLoop};
use stipple::gamepad::{self, GamepadButton, GamepadEvent};
use stipple::prelude::*;

const WORLD: CanvasId = CanvasId(1);
const WINDOW: Size = Size::new(1024.0, 640.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Act {
    Left,
    Right,
    Boost,
    Fire,
}

const ACTIONS: [(Act, &str); 4] = [
    (Act::Left, "Steer left"),
    (Act::Right, "Steer right"),
    (Act::Boost, "Boost"),
    (Act::Fire, "Fire"),
];

fn name_of(a: Act) -> &'static str {
    ACTIONS
        .iter()
        .find(|(x, _)| *x == a)
        .map_or("?", |(_, n)| n)
}

fn default_actions() -> Actions<Act> {
    use PhysicalKey as K;
    Actions::new()
        .with(Act::Left, Binding::Key(K::ARROW_LEFT))
        .with(Act::Left, Binding::Key(K::KEY_A))
        .with(Act::Right, Binding::Key(K::ARROW_RIGHT))
        .with(Act::Right, Binding::Key(K::KEY_D))
        .with(Act::Boost, Binding::Key(K::SHIFT_LEFT))
        .with(Act::Fire, Binding::Key(K::SPACE))
        .with(Act::Fire, Binding::MouseButton(PointerButton::Left))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Title,
    Settings,
    Play,
    Over,
}

const LINES: [&str; 3] = [
    "Morning, courier. Three parcels for the Ring Market, and the comet belt is noisy today.",
    "Steer around the rocks, boost when the lane is clear, and fire to break up the debris.",
    "Mind your hull out there. Dispatch out.",
];

/// What the canvas draws, shared with its paint callback.
#[derive(Default)]
struct World {
    /// The ship across the lane, 0 to 1.
    x: f64,
    /// How far the stars have scrolled.
    scroll: f64,
    boosting: f64,
    size: Size,
}

struct Game {
    screen: Screen,
    /// Where Back leaves the settings for.
    settings_back: Screen,
    paused: bool,
    volume: [f32; 3],
    actions: Actions<Act>,
    capture: Option<RebindCapture<Act>>,
    pixel_scale: usize,
    language: usize,
    hull: f32,
    boost: u32,
    score: i64,
    best: i64,
    played: f64,
    next_hit: f64,
    fire_queued: bool,
    floaters: FloatingTexts,
    dialogue: Option<(usize, Typewriter)>,
    world: Rc<RefCell<World>>,
}

impl Game {
    fn new() -> Self {
        Self {
            screen: Screen::Title,
            settings_back: Screen::Title,
            paused: false,
            volume: [0.8, 0.6, 0.75],
            actions: default_actions(),
            capture: None,
            pixel_scale: 1,
            language: 0,
            hull: 100.0,
            boost: 5,
            score: 0,
            best: 0,
            played: 0.0,
            next_hit: 3.0,
            fire_queued: false,
            floaters: FloatingTexts::new(),
            dialogue: None,
            world: Rc::new(RefCell::new(World {
                x: 0.5,
                ..World::default()
            })),
        }
    }

    fn start(&mut self) {
        self.screen = Screen::Play;
        self.paused = false;
        self.hull = 100.0;
        self.boost = 5;
        self.score = 0;
        self.played = 0.0;
        self.next_hit = 3.0;
        self.floaters = FloatingTexts::new();
        self.dialogue = Some((0, Typewriter::new(LINES[0])));
        self.world.borrow_mut().x = 0.5;
    }

    /// Where the ship is drawn, in the window.
    fn ship_at(&self) -> Point {
        let w = self.world.borrow();
        let size = if w.size.width > 0.0 { w.size } else { WINDOW };
        Point::new(w.x * size.width, size.height * 0.72)
    }

    fn open_settings(&mut self) {
        self.settings_back = self.screen;
        self.screen = Screen::Settings;
    }
}

fn update(g: &mut Game, frame: &mut Frame) {
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
    g.actions.update(frame.input());
    if g.floaters.update(frame.time()) {
        frame.rebuild();
    }
    if g.dialogue.is_some() {
        // The belt waits while Dispatch talks.
        return;
    }
    let dt = frame.dt_secs();
    // Steering: the actions, plus any gamepad's stick and d-pad.
    let mut steer = f64::from(g.actions.value(Act::Right) - g.actions.value(Act::Left));
    for (id, _) in gamepad::gamepads() {
        if let Some(pad) = gamepad::state(id) {
            let (x, _) = pad.left_stick();
            if x.abs() > 0.2 {
                steer += f64::from(x);
            }
            steer += f64::from(u8::from(pad.pressed(GamepadButton::DPadRight)))
                - f64::from(u8::from(pad.pressed(GamepadButton::DPadLeft)));
        }
    }
    let ship = g.ship_at();
    {
        let mut w = g.world.borrow_mut();
        let speed = if w.boosting > 0.0 { 1.4 } else { 0.7 };
        w.x = (w.x + steer.clamp(-1.0, 1.0) * speed * dt).clamp(0.05, 0.95);
        w.boosting = (w.boosting - dt).max(0.0);
        w.scroll += dt * if w.boosting > 0.0 { 420.0 } else { 160.0 };
    }
    if g.actions.pressed(Act::Boost) && g.boost > 0 {
        g.boost -= 1;
        g.world.borrow_mut().boosting = 1.2;
        frame.rebuild();
    }
    if g.actions.pressed(Act::Fire) || std::mem::take(&mut g.fire_queued) {
        g.score += 10;
        let at = Point::new(ship.x - 16.0, ship.y - 70.0);
        g.floaters
            .spawn("+10", Color::rgb(0x3d, 0xf5, 0xa7), at, frame.time());
        frame.rebuild();
    }
    g.played += dt;
    if g.played >= g.next_hit {
        g.next_hit += 2.5;
        g.hull -= 12.0;
        let at = Point::new(ship.x + 12.0, ship.y - 40.0);
        g.floaters
            .spawn("-12", Color::rgb(0xff, 0x4d, 0x5e), at, frame.time());
        if g.hull <= 0.0 {
            g.hull = 0.0;
            g.best = g.best.max(g.score);
            g.screen = Screen::Over;
        }
        frame.rebuild();
    }
    frame.repaint(WORLD);
}

fn view(g: &Game, cx: &mut Cx<Game>) -> Element {
    let theme = *cx.theme();
    let shown = match g.screen {
        Screen::Title => 0,
        Screen::Settings => 1,
        Screen::Play => 2,
        Screen::Over => 3,
    };
    page_stack(cx, 4, shown, |cx, page, visible| match page {
        0 => title(cx),
        1 => settings(g, cx, &theme),
        2 => play(g, cx, &theme, visible),
        _ => game_over(g, cx),
    })
    .fill(theme.palette.background)
}

fn hints<S>(cx: &Cx<S>) -> Element {
    gui::controls_hint(
        cx,
        &[
            ("\u{2191}\u{2193}\u{2190}\u{2192}", "Move"),
            ("Enter / A", "Select"),
            ("Esc / B", "Back"),
        ],
    )
}

fn title(cx: &mut Cx<Game>) -> Element {
    let handle = cx.handle();
    let menu = gui::menu_list(cx)
        .children(vec![
            gui::menu_button(cx, "Start", Game::start),
            gui::menu_button(cx, "Settings", Game::open_settings),
            gui::menu_button(cx, "Quit", move |_| handle.exit()),
        ])
        .width(320.0);
    TitleScreen::new("STARLIT COURIER", menu)
        .subtitle("deliveries across the comet belt")
        .footer(hints(cx))
        .view(cx)
}

fn section(theme: &Theme, title: &str, rows: Vec<Element>) -> Element {
    let mut items = vec![
        Element::text(
            title.to_uppercase(),
            theme.typography.caption,
            theme.palette.primary,
        )
        .bold(),
    ];
    items.extend(rows);
    column(items).gap(theme.spacing.xs)
}

fn settings(g: &Game, cx: &mut Cx<Game>, theme: &Theme) -> Element {
    let volume = |cx: &mut Cx<Game>, label: &str, i: usize| {
        gui::volume_row(cx, label, g.volume[i], move |g, v| {
            g.volume[i] = v;
        })
    };
    let audio = section(
        theme,
        "Audio",
        vec![
            volume(cx, "Master", 0),
            volume(cx, "Music", 1),
            volume(cx, "Effects", 2),
        ],
    );
    let display = section(
        theme,
        "Display",
        vec![
            gui::fullscreen_row(cx, "Fullscreen", |_, _| {}),
            gui::choice_row(
                cx,
                "Pixel scale",
                &["1\u{d7}", "2\u{d7}", "3\u{d7}", "4\u{d7}", "Fit"],
                g.pixel_scale,
                |g, i| g.pixel_scale = i,
            ),
            gui::choice_row(
                cx,
                "Language",
                &["English", "Fran\u{e7}ais", "Espa\u{f1}ol", "Deutsch"],
                g.language,
                |g, i| g.language = i,
            ),
        ],
    );
    let mut controls = Vec::new();
    for (act, label) in ACTIONS {
        let state = BindingRowState::default()
            .capturing(g.capture.is_some_and(|c| c.action == act))
            .conflicts(gui::conflicts(&g.actions, act).into_iter().map(name_of));
        controls.push(gui::binding_row(
            cx,
            label,
            g.actions.bindings(act),
            &state,
            move |g| {
                // A second press while it waits is the input to bind.
                if g.capture.is_none() {
                    g.capture = Some(RebindCapture::new(act, 0));
                }
            },
        ));
    }
    controls.push(row(vec![
        spacer(),
        gui::menu_button(cx, "Reset controls", |g| {
            g.actions = default_actions();
        })
        .width(260.0),
    ]));
    let controls = section(theme, "Controls", controls);
    let back = gui::menu_button(cx, "Back", |g| {
        g.screen = g.settings_back;
    })
    .width(240.0);
    let body = row(vec![
        column(vec![audio, display]).gap(theme.spacing.lg).grow(1.0),
        column(vec![controls]).grow(1.0),
    ])
    .gap(theme.spacing.xl)
    .align(Align::Start, Align::Start);
    column(vec![
        row(vec![
            Element::text("Settings", theme.typography.heading, theme.palette.text).bold(),
            spacer(),
            hints(cx),
        ]),
        body,
        spacer(),
        row(vec![back, spacer()]),
    ])
    .gap(theme.spacing.lg)
    .padding(Insets::uniform(theme.spacing.xl))
    .grow(1.0)
    // One spatial group: the arrows cross from one column to the other.
    .spatial_nav()
}

/// The comet belt: stars streaming down, a few rocks, the ship.
fn paint_world(world: &RefCell<World>, c: &mut Canvas) {
    let size = c.size();
    world.borrow_mut().size = size;
    let w = world.borrow();
    c.fill_rect(
        Rect::from_xywh(0.0, 0.0, size.width, size.height),
        Color::rgb(0x0b, 0x0a, 0x1a),
    );
    // Stars on three layers, scrolling at different speeds.
    for layer in 0..3u32 {
        let speed = 0.4 + 0.3 * f64::from(layer);
        let shade = 90 + 50 * layer as u8;
        for i in 0..40u32 {
            let seed = (i * 7919 + layer * 104_729) as f64;
            let x = (seed * 0.618_033).fract() * size.width;
            let y = ((seed * 0.414_213).fract() * size.height + w.scroll * speed) % size.height;
            let s = 1.0 + f64::from(layer);
            c.fill_rect(
                Rect::from_xywh(x, y, s, s),
                Color::rgb(shade, shade, shade.saturating_add(30)),
            );
        }
    }
    // Rocks drifting down the lane.
    for i in 0..5u32 {
        let seed = f64::from(i * 31 + 7);
        let x = (seed * 0.754_877).fract() * size.width;
        let y = ((seed * 0.569_840).fract() * size.height + w.scroll * 0.9) % (size.height + 60.0)
            - 30.0;
        let r = 10.0 + 6.0 * f64::from(i % 3);
        c.fill_round_rect(
            Rect::from_xywh(x - r, y - r, 2.0 * r, 2.0 * r),
            r * 0.8,
            Color::rgb(0x5a, 0x4e, 0x6e),
        );
    }
    // The ship.
    let (x, y) = (w.x * size.width, size.height * 0.72);
    if w.boosting > 0.0 {
        let mut flame = Path::new();
        flame
            .move_to(Point::new(x - 8.0, y + 14.0))
            .line_to(Point::new(x, y + 40.0))
            .line_to(Point::new(x + 8.0, y + 14.0))
            .close();
        c.fill_path(&flame, Color::rgb(0xff, 0xc8, 0x3d));
    }
    let mut hull = Path::new();
    hull.move_to(Point::new(x, y - 22.0))
        .line_to(Point::new(x + 16.0, y + 16.0))
        .line_to(Point::new(x, y + 8.0))
        .line_to(Point::new(x - 16.0, y + 16.0))
        .close();
    c.fill_path(&hull, Color::rgb(0xff, 0x3d, 0x8b));
}

fn play(g: &Game, cx: &mut Cx<Game>, theme: &Theme, visible: bool) -> Element {
    let world = g.world.clone();
    let canvas = Element::canvas(WORLD, move |c: &mut Canvas| paint_world(&world, c)).grow(1.0);
    let p = &theme.palette;
    let boost = Bar::new(g.boost as f32, 5.0, p.warning)
        .segments(5)
        .size(150.0, 12.0)
        .ghost(None);
    let bars = column(vec![
        gui::health_bar(cx, g.hull, 100.0),
        row(vec![
            Element::text("\u{21AF}", theme.typography.title, p.warning),
            gui::stat_bar(cx, &boost),
        ])
        .gap(theme.spacing.xs),
    ])
    .gap(theme.spacing.xs);
    let pause = Button::new("II")
        .variant(Variant::Secondary)
        .view(cx)
        .on_tap(cx, |g| g.paused = true)
        .a11y_label("Pause");
    let top = row(vec![
        bars,
        spacer(),
        gui::counter(cx, "score", "SCORE", g.score),
        gui::counter(cx, "parcels", "PARCELS", 3),
        pause,
    ])
    .gap(theme.spacing.md)
    .align(Align::Start, Align::Start);
    let mut bottom = Vec::new();
    match (&g.dialogue, visible) {
        (Some((line, text)), true) => {
            let line = *line;
            bottom.push(
                DialogueBox::new(text, move |g: &mut Game| {
                    let Some((_, text)) = g.dialogue.as_mut() else {
                        return;
                    };
                    if text.advance() {
                        g.dialogue = LINES
                            .get(line + 1)
                            .map(|next| (line + 1, Typewriter::new(*next)));
                    }
                })
                .speaker("Dispatch")
                .portrait(portrait(theme))
                .view(cx)
                .grow(1.0),
            );
        }
        _ => bottom.push(spacer()),
    }
    bottom.push(gui::minimap_slot(cx, 140.0, 104.0, None));
    let overlay = column(vec![
        top,
        spacer(),
        row(bottom)
            .gap(theme.spacing.lg)
            .align(Align::Start, Align::End),
    ])
    .padding(Insets::uniform(theme.spacing.lg));
    if g.screen == Screen::Play && g.paused {
        gui::pause_menu(cx, "Paused", |g| g.paused = false)
            .children(vec![
                gui::menu_button(cx, "Resume", |g| g.paused = false),
                gui::menu_button(cx, "Options", Game::open_settings),
                gui::menu_button(cx, "Quit to title", |g| {
                    g.paused = false;
                    g.screen = Screen::Title;
                }),
            ])
            .open(cx);
    }
    gui::hud(
        canvas,
        Element::layers(vec![overlay, gui::floating_texts(cx, &g.floaters)]),
    )
}

/// The dispatcher's portrait: a placeholder drawn in boxes.
fn portrait(theme: &Theme) -> Element {
    let p = &theme.palette;
    let face = Element::boxed(BoxStyle::filled(p.primary.mix(p.surface, 0.4)).radius(18.0))
        .width(36.0)
        .height(36.0);
    column(vec![face])
        .fill(p.background)
        .align(Align::Center, Align::Center)
}

fn game_over(g: &Game, cx: &mut Cx<Game>) -> Element {
    let menu = gui::menu_list(cx)
        .children(vec![
            gui::menu_button(cx, "Try again", Game::start),
            gui::menu_button(cx, "Title screen", |g| {
                g.screen = Screen::Title;
            }),
        ])
        .width(320.0);
    let minutes = (g.played / 60.0) as u32;
    let seconds = (g.played % 60.0) as u32;
    GameOverScreen::new("HULL BREACHED", menu)
        .line("Score", g.score)
        .line("Best", g.best)
        .line("Time", format!("{minutes}:{seconds:02}"))
        .view(cx)
}

fn back(g: &mut Game) -> Handled {
    if g.capture.take().is_some() {
        return Handled::Yes;
    }
    match g.screen {
        Screen::Settings => g.screen = g.settings_back,
        Screen::Play => g.paused = !g.paused,
        Screen::Over => g.screen = Screen::Title,
        Screen::Title => return Handled::No,
    }
    Handled::Yes
}

fn app(game: Game, style: GameStyle) -> App<Game> {
    App::new(game, view)
        .title("Starlit Courier")
        .logical_size(WINDOW)
        .game_style(style)
        .game_loop(
            GameLoop::new(update).paused_when(|g: &Game| {
                (g.screen != Screen::Play || g.paused) && g.capture.is_none()
            }),
        )
        // The pad moves through the menus, except while a controls row
        // waits for its next button.
        .gamepad_navigation_when(|g: &Game| g.capture.is_none())
        .on_back(back)
        .on_gamepad(|g, event| {
            if let GamepadEvent::Button {
                button,
                pressed: true,
                ..
            } = event
            {
                match button {
                    GamepadButton::Start if g.screen == Screen::Play => g.paused = !g.paused,
                    GamepadButton::South
                        if g.screen == Screen::Play && !g.paused && g.dialogue.is_none() =>
                    {
                        g.fire_queued = true;
                    }
                    _ => {}
                }
            }
        })
}

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--shots") {
        let dir = args.next().unwrap_or_else(|| ".".into());
        shots(&dir);
        return;
    }
    app(Game::new(), GameStyle::neon()).run();
}

/// Render each screen off screen into `dir`.
fn shots(dir: &str) {
    let render_in = |style: GameStyle, name: &str, game: Game, keys: &[KeyInput]| {
        let mut a = app(game, style);
        a.render_once();
        a.render_once();
        for k in keys {
            a.press_key(k.clone());
            a.render_once();
        }
        // Let the focus looks and the overlays settle.
        let mut frame = a.render_once();
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(10));
            frame = a.render_once();
        }
        let path = format!("{dir}/{name}.png");
        frame.save_png(&path).expect("write the screenshot");
        println!("{path}");
    };
    let render = |name: &str, game: Game, keys: &[KeyInput]| {
        render_in(GameStyle::neon(), name, game, keys);
    };
    render("title", Game::new(), &[KeyInput::Down]);
    // The other preset.
    let mut g = Game::new();
    g.screen = Screen::Settings;
    render_in(GameStyle::ember(), "settings-ember", g, &[KeyInput::Right]);
    let mut g = Game::new();
    g.screen = Screen::Settings;
    g.capture = Some(RebindCapture::new(Act::Boost, 0));
    g.actions.bind(Act::Fire, Binding::Key(PhysicalKey::KEY_D));
    render(
        "settings",
        g,
        &[KeyInput::Down, KeyInput::Down, KeyInput::Right],
    );
    let mut g = Game::new();
    g.start();
    g.hull = 64.0;
    g.boost = 3;
    g.score = 120;
    let ship = g.ship_at();
    g.floaters.spawn(
        "+10",
        Color::rgb(0x3d, 0xf5, 0xa7),
        Point::new(ship.x - 16.0, ship.y - 70.0),
        Duration::ZERO,
    );
    g.world.borrow_mut().boosting = 1.0;
    render("play", g, &[]);
    let mut g = Game::new();
    g.start();
    g.dialogue = None;
    g.hull = 40.0;
    g.paused = true;
    render("pause", g, &[KeyInput::Down]);
    let mut g = Game::new();
    g.start();
    g.screen = Screen::Over;
    g.score = 340;
    g.best = 410;
    g.played = 83.0;
    render("over", g, &[]);
}
