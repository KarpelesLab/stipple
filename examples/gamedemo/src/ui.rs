//! The screens: the title, settings, the HUD over the game with its pause
//! menu, and the game over, all over the garden's canvas.

use super::*;

/// The theme: `GameStyle::ember` with a translucent background, so the
/// garden shows dimly behind the menus.
pub fn style() -> GameStyle {
    GameStyle::ember()
        .with_background(Color::rgba(0x12, 0x0e, 0x1c, 0xb4))
        .with_surface(Color::rgb(0x26, 0x1f, 0x33))
        .with_accent(Color::rgb(0xff, 0xb3, 0x47))
}

pub fn view(g: &Game, cx: &mut Cx<Game>) -> Element {
    let theme = *cx.theme();
    let stage = g.stage.clone();
    let canvas = Element::canvas(WORLD, move |c: &mut Canvas| stage.borrow().paint(c))
        // A new pixel scale repaints it.
        .revision(g.settings.scale as u64)
        .grow(1.0);
    let shown = match g.screen {
        Screen::Title => 0,
        Screen::Settings => 1,
        Screen::Play => 2,
        Screen::Over => 3,
    };
    let pages = page_stack(cx, 4, shown, |cx, page, _| match page {
        0 => title(g, cx, &theme),
        1 => settings(g, cx, &theme),
        2 => hud(g, cx, &theme),
        _ => game_over(g, cx, &theme),
    });
    let mut layers = vec![canvas, pages];
    if g.screen == Screen::Play {
        layers.push(g.touch.element(&theme));
    }
    Element::layers(layers).fill(Color::rgb(6, 5, 14))
}

fn hints(theme: &Theme) -> Element {
    gui::controls_hint(
        theme,
        &[
            ("\u{2191}\u{2193}\u{2190}\u{2192}", "Move"),
            ("Enter / A", "Select"),
            ("Esc / B", "Back"),
        ],
    )
}

fn title(g: &Game, cx: &mut Cx<Game>, theme: &Theme) -> Element {
    let mut buttons = vec![
        gui::menu_button(cx, theme, "Play", Game::start),
        gui::menu_button(cx, theme, "Settings", Game::open_settings),
    ];
    // A page can't close itself.
    if !cfg!(target_arch = "wasm32") {
        let handle = cx.handle();
        buttons.push(gui::menu_button(cx, theme, "Quit", move |_: &mut Game| {
            handle.exit()
        }));
    }
    let menu = gui::menu_list(theme, buttons).width(320.0);
    let footer = column(vec![
        Element::text(
            if g.best > 0 {
                format!("Best: {}", g.best)
            } else {
                "Catch the embers. Mind the moths.".into()
            },
            theme.typography.body,
            theme.palette.text_muted,
        ),
        hints(theme),
    ])
    .gap(theme.spacing.md)
    .align(Align::Start, Align::Center);
    TitleScreen::new("LAMPWICK", menu)
        .subtitle("a lantern-keeper's night")
        .footer(footer)
        .view(theme)
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
    column(items)
        .gap(theme.spacing.xs)
        .align(Align::Start, Align::Stretch)
}

fn settings(g: &Game, cx: &mut Cx<Game>, theme: &Theme) -> Element {
    let volume = |cx: &mut Cx<Game>, label: &str, value: f32, set: fn(&mut Settings, f32)| {
        gui::volume_row(cx, theme, label, value, move |g: &mut Game, v| {
            set(&mut g.settings, v);
            g.apply_volumes();
            g.save_settings();
        })
    };
    let s = g.settings;
    let audio = section(
        theme,
        "Audio",
        vec![
            volume(cx, "Master", s.master, |s, v| s.master = v),
            volume(cx, "Music", s.music, |s, v| s.music = v),
            volume(cx, "Effects", s.effects, |s, v| s.effects = v),
        ],
    );
    let display = section(
        theme,
        "Display",
        vec![
            gui::fullscreen_row(cx, theme, "Fullscreen", |g: &mut Game, on| {
                g.settings.fullscreen = on;
                g.save_settings();
            }),
            gui::choice_row(
                cx,
                theme,
                "Pixel scale",
                &SCALES,
                s.scale,
                |g: &mut Game, i| {
                    g.settings.scale = i;
                    g.stage.borrow_mut().vp.max_factor = max_factor(i);
                    g.save_settings();
                },
            ),
        ],
    );
    let mut controls = Vec::new();
    for (act, _, label) in ACTIONS {
        let state = BindingRowState::default()
            .capturing(g.capture.is_some_and(|c| c.action == act))
            .conflicts(gui::conflicts(&g.actions, act).into_iter().map(label_of));
        controls.push(gui::binding_row(
            cx,
            theme,
            label,
            g.actions.bindings(act),
            &state,
            move |g: &mut Game| {
                // A second press while it waits is the input to bind. A key
                // replaces the first key, a pad button the first pad binding.
                if g.capture.is_none() {
                    g.capture = Some(RebindCapture::by_device(act));
                }
            },
        ));
    }
    controls.push(
        row(vec![
            spacer(),
            gui::menu_button(cx, theme, "Reset controls", |g: &mut Game| {
                g.actions = default_actions();
                g.save_controls();
            })
            .width(280.0),
        ])
        .align(Align::Start, Align::Center),
    );
    let controls = section(theme, "Controls", controls);
    let back =
        gui::menu_button(cx, theme, "Back", |g: &mut Game| g.screen = g.settings_back).width(240.0);
    let body = row(vec![
        column(vec![audio, display])
            .gap(theme.spacing.lg)
            .align(Align::Start, Align::Stretch)
            .grow(1.0),
        column(vec![controls])
            .align(Align::Start, Align::Stretch)
            .grow(1.4),
    ])
    .gap(theme.spacing.xl)
    .align(Align::Start, Align::Start);
    column(vec![
        row(vec![
            Element::text("Settings", theme.typography.heading, theme.palette.text).bold(),
            spacer(),
            hints(theme),
        ])
        .align(Align::Start, Align::Center),
        body,
        spacer(),
        row(vec![back, spacer()]),
    ])
    .gap(theme.spacing.lg)
    .padding(Insets::uniform(theme.spacing.xl))
    .align(Align::Start, Align::Stretch)
    .grow(1.0)
    .fill(theme.palette.background.mix(Color::BLACK, 0.3))
    .spatial_nav()
}

fn hud(g: &Game, cx: &mut Cx<Game>, theme: &Theme) -> Element {
    let p = &theme.palette;
    let (health, light, score) = {
        let stage = g.stage.borrow();
        let w = &stage.world;
        (w.health, light_twentieths(w), w.score)
    };
    let lamp = Bar::new(light as f32, 20.0, p.warning)
        .segments(10)
        .size(160.0, 12.0)
        .ghost(None);
    let bars = column(vec![
        gui::health_bar(theme, health as f32, world::MAX_HEALTH as f32),
        row(vec![
            Element::text("\u{2600}", theme.typography.title, p.warning),
            gui::stat_bar(theme, &lamp),
        ])
        .gap(theme.spacing.xs)
        .align(Align::Start, Align::Center),
    ])
    .gap(theme.spacing.xs);
    let top = row(vec![
        bars,
        spacer(),
        gui::counter(theme, "score", "SCORE", i64::from(score)),
        gui::counter(theme, "best", "BEST", i64::from(g.best.max(score))),
    ])
    .gap(theme.spacing.lg)
    .align(Align::Start, Align::Start)
    .padding(Insets::uniform(theme.spacing.md));
    if g.screen == Screen::Play && g.paused {
        let buttons = vec![
            gui::menu_button(cx, theme, "Resume", |g: &mut Game| g.paused = false),
            gui::menu_button(cx, theme, "Settings", Game::open_settings),
            gui::menu_button(cx, theme, "Quit to title", |g: &mut Game| {
                g.paused = false;
                g.screen = Screen::Title;
            }),
        ];
        gui::pause_menu(cx, theme, "Paused", buttons, |g: &mut Game| {
            g.paused = false
        });
    }
    Element::layers(vec![
        column(vec![top, spacer()]).align(Align::Start, Align::Stretch),
        gui::floating_texts(theme, &g.floaters),
    ])
    // The game takes what the HUD doesn't.
    .hit_through()
}

fn game_over(g: &Game, cx: &mut Cx<Game>, theme: &Theme) -> Element {
    let menu = gui::menu_list(
        theme,
        vec![
            gui::menu_button(cx, theme, "Play again", Game::start),
            gui::menu_button(cx, theme, "Title screen", |g: &mut Game| {
                g.screen = Screen::Title
            }),
        ],
    )
    .width(320.0);
    let stage = g.stage.borrow();
    let w = &stage.world;
    let secs = w.time as u32;
    let best = if g.new_best {
        format!("{} new!", g.best)
    } else {
        g.best.to_string()
    };
    GameOverScreen::new("THE LANTERN GOES OUT", menu)
        .line("Score", w.score)
        .line("Best", best)
        .line("Embers caught", w.caught)
        .line("Moths burst", w.burst)
        .line("Time", format!("{}:{:02}", secs / 60, secs % 60))
        .view(theme)
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

/// The app around a game.
pub fn app(game: Game) -> App<Game> {
    let fullscreen = game.settings.fullscreen;
    App::new(game, view)
        .title("Lampwick")
        .app_info(app_info())
        .logical_size(WINDOW)
        .fullscreen(fullscreen)
        .game_style(style())
        .game_loop(
            GameLoop::new(update)
                .paused_when(|g: &Game| {
                    (g.screen != Screen::Play || g.paused) && g.capture.is_none()
                })
                .touch_controls(touch_controls()),
        )
        // The pad drives the menus (and B, like Escape, pauses the game),
        // except while a controls row waits for a button.
        .gamepad_navigation_when(|g: &Game| g.capture.is_none())
        .on_back(back)
        .on_gamepad(|g, event| {
            // The loop is stopped while paused: Start resumes from here.
            if let GamepadEvent::Button {
                button: GamepadButton::Start,
                pressed: true,
                ..
            } = event
                && g.screen == Screen::Play
                && g.paused
            {
                g.paused = false;
            }
        })
}

pub fn app_info() -> AppInfo {
    AppInfo::new("org.stipple.Lampwick")
        .name("Lampwick")
        .version(env!("CARGO_PKG_VERSION"))
}

/// The app as it runs: saving in the app's data directory (the browser's
/// `localStorage` on the web), playing through the default output.
pub fn make_app() -> App<Game> {
    stipple::platform::set_app_info(app_info());
    let store = SaveStore::app_data().unwrap_or_else(|e| {
        eprintln!("lampwick: no data directory ({e}); nothing will be saved");
        SaveStore::memory()
    });
    // Each game's garden is seeded by how many were played before.
    app(Game::new(store, Audio::shared(), 0x9e37_79b9))
}
