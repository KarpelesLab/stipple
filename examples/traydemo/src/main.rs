//! OS integration: a tray icon with a menu, desktop notifications, and opening
//! URLs and files in their default apps.
//!
//! The tray icon is a status item in macOS's menu bar, a notification-area
//! icon on Windows, and a `StatusNotifierItem` on Linux (KDE, GNOME with the
//! AppIndicator extension, XFCE, waybar…). Its menu counts pings, toggles a
//! check mark, sends a notification and quits; clicking the icon (left click on
//! Windows) counts activations.
//!
//! `--selftest` drives it without input, for CI: it opens a URL
//! (`$STIPPLE_SELFTEST_URL`, default `https://example.com/stipple-selftest`)
//! and, if `$STIPPLE_SELFTEST_FILE` is set, that file, shows a notification,
//! reports whether the tray icon is shown, and quits. With
//! `$STIPPLE_SELFTEST_EXPECT_TRAY` set it first waits (up to 20 s) for a tray
//! host to pick "Ping" from the menu and activate the icon, as CI's mock host
//! on Linux does.

use std::time::Duration;
use stipple::platform::shell;
use stipple::prelude::*;

const SELFTEST_URL: &str = "https://example.com/stipple-selftest";

struct State {
    pings: u32,
    activations: u32,
    muted: bool,
    status: String,
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let open = button_labeled(&theme, "Open stipple on GitHub").on_tap(cx, |s: &mut State| {
        s.status = result(
            "open_url",
            open_url("https://github.com/KarpelesLab/stipple"),
        );
    });
    let file = button_labeled(&theme, "Open a text file").on_tap(cx, |s: &mut State| {
        let path = std::env::temp_dir().join("stipple-traydemo.txt");
        let wrote = std::fs::write(&path, "Opened by Stipple's traydemo.\n");
        s.status = match wrote {
            Ok(()) => result("open_path", shell::open_path(&path)),
            Err(e) => format!("couldn't write {}: {e}", path.display()),
        };
    });
    let notify = button_labeled(&theme, "Show a notification").on_tap(cx, |s: &mut State| {
        s.status = result("notify", send_notification(s));
    });
    column(vec![
        heading(&theme, "Tray, notifications and URLs"),
        label(
            &theme,
            format!(
                "Tray: {} · pings {} · activations {} · muted {}",
                if shell::tray_shown() {
                    "shown"
                } else {
                    "not shown"
                },
                state.pings,
                state.activations,
                state.muted
            ),
        ),
        row(vec![open, file, notify]).gap(theme.spacing.sm),
        label(&theme, state.status.clone()),
    ])
    .gap(theme.spacing.md)
    .padding(Insets::uniform(theme.spacing.lg))
}

fn open_url(url: &str) -> Result<(), stipple::platform::PlatformError> {
    stipple::platform::open_url(url)
}

fn send_notification(s: &State) -> Result<(), stipple::platform::PlatformError> {
    stipple::platform::notify(
        &Notification::new("Stipple traydemo").body(format!("{} pings so far", s.pings)),
    )
}

fn result(what: &str, r: Result<(), stipple::platform::PlatformError>) -> String {
    match r {
        Ok(()) => format!("{what}: ok"),
        Err(e) => format!("{what}: {e}"),
    }
}

fn tray(state: &State, handle: &AppHandle<State>) -> Tray<State> {
    let quit = handle.clone();
    Tray::new("Stipple tray demo")
        .icon(icon())
        .menu(
            Menu::new("Tray")
                .item(MenuItem::new("&Ping", |s: &mut State| {
                    s.pings += 1;
                    println!("traydemo: tray command Ping ({})", s.pings);
                }))
                .item(
                    MenuItem::new("&Muted", |s: &mut State| s.muted = !s.muted)
                        .checked(state.muted),
                )
                .separator()
                .submenu(Menu::new("&More").item(MenuItem::new(
                    "Show a &notification",
                    |s: &mut State| {
                        s.status = result("notify", send_notification(s));
                    },
                )))
                .item(MenuItem::new("&Quit", move |_: &mut State| quit.exit())),
        )
        .on_activate(|s: &mut State| {
            s.activations += 1;
            println!("traydemo: tray activated ({})", s.activations);
        })
}

/// A 64×64 icon: a blue rounded square with a grid of white dots.
fn icon() -> Pixmap {
    let n = 64u32;
    let mut p = Pixmap::new(PhysicalSize::new(n, n));
    for (i, px) in p
        .as_bytes_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .enumerate()
    {
        let (x, y) = ((i as u32 % n) as f64 + 0.5, (i as u32 / n) as f64 + 0.5);
        // Distance outside a rounded square with radius-14 corners.
        let (cx, cy) = ((x - 32.0).abs() - 18.0, (y - 32.0).abs() - 18.0);
        let outside = cx.max(0.0).hypot(cy.max(0.0)) - 14.0;
        let alpha = (0.5 - outside).clamp(0.0, 1.0);
        let (dx, dy) = ((x % 16.0) - 8.0, (y % 16.0) - 8.0);
        let dot = (3.5 - dx.hypot(dy)).clamp(0.0, 1.0);
        let mix = |a: f64, b: f64| (a + (b - a) * dot) as u8;
        px.copy_from_slice(&[
            mix(0x3b as f64, 255.0),
            mix(0x6e as f64, 255.0),
            mix(0xd8 as f64, 255.0),
            (alpha * 255.0) as u8,
        ]);
    }
    p
}

fn main() {
    let selftest = std::env::args().any(|a| a == "--selftest");
    let state = State {
        pings: 0,
        activations: 0,
        muted: false,
        status: "Right-click the tray icon for its menu.".into(),
    };
    let mut app = App::new(state, view)
        .title("Stipple tray demo")
        .logical_size(Size::new(560.0, 220.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    let handle = app.handle();
    let tray_handle = handle.clone();
    app = app.tray(move |s: &State| tray(s, &tray_handle));
    if selftest {
        start_selftest(&handle);
    }
    app.run();
    if selftest {
        println!("traydemo: event loop exited");
    }
}

/// Exercise everything once the loop runs, then quit.
fn start_selftest(handle: &AppHandle<State>) {
    let expect_tray = std::env::var_os("STIPPLE_SELFTEST_EXPECT_TRAY").is_some();
    let quit = handle.clone();
    handle.after(Duration::from_millis(800), move |s: &mut State| {
        println!("traydemo: tray shown={}", shell::tray_shown());
        let url = std::env::var("STIPPLE_SELFTEST_URL").unwrap_or_else(|_| SELFTEST_URL.into());
        println!("traydemo: open_url({url}) -> {:?}", open_url(&url));
        if let Ok(file) = std::env::var("STIPPLE_SELFTEST_FILE") {
            println!(
                "traydemo: open_path({file}) -> {:?}",
                shell::open_path(&file)
            );
        }
        println!("traydemo: notify -> {:?}", send_notification(s));
        let waiter = quit.clone();
        if expect_tray {
            // Wait for the host to pick Ping and activate the icon.
            let started = std::time::Instant::now();
            quit.every(Duration::from_millis(100), move |s: &mut State| {
                if s.pings >= 1 && s.activations >= 1 {
                    println!("traydemo: tray ping ok");
                    println!("traydemo: tray activate ok");
                    waiter.exit();
                } else if started.elapsed() > Duration::from_secs(20) {
                    println!(
                        "traydemo: tray timeout (pings={}, activations={})",
                        s.pings, s.activations
                    );
                    waiter.exit();
                }
            });
        } else {
            // Leave time for the opener and notification to be handed over
            // (they run on their own threads on Linux).
            quit.after(Duration::from_secs(3), move |_: &mut State| waiter.exit());
        }
    });
}
