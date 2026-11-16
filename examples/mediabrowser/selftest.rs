//! `--selftest`: the browser driven against a sample folder generated in a
//! temporary directory (pictures, sounds, text), as a user would: pick
//! folders in the tree, sort, move with the keyboard and type a name, select
//! several, preview a picture and Markdown, play a sound for a moment and
//! seek it, switch to the grid and let its thumbnails load, change the
//! language to French and Arabic (mirrored), and check each step through the
//! accessibility tree and the state. Frames are saved as PNGs (`--out DIR`,
//! default `visual_output`).
//!
//! It runs off-screen first, then opens the real window on the same folder
//! for a few seconds (`--hold SECS`, default 3) with the thumbnails loading
//! on worker threads and the sound playing, and checks it quit cleanly;
//! `--headless` skips the window. Prints `mediabrowser: selftest ok` or
//! `mediabrowser: selftest FAILED: ...` and exits 0 or 1.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use stipple::audio::Audio;
use stipple::prelude::*;

use super::loader::Loader;
use super::settings::{Mode, Settings};
use super::vfs::Vfs;
use super::{State, build_app};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

pub fn run(args: &[String]) -> i32 {
    let out = PathBuf::from(arg(args, "--out").unwrap_or_else(|| "visual_output".into()));
    let _ = std::fs::create_dir_all(&out);
    let dir = std::env::temp_dir().join(format!("mediabrowser-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = super::sample::write_to(&dir) {
        println!("mediabrowser: selftest FAILED: writing the sample: {e}");
        return 1;
    }
    println!("mediabrowser: sample folder {}", dir.display());
    let result = offscreen(&dir, &out).and_then(|()| {
        if args.iter().any(|a| a == "--headless") {
            Ok(())
        } else {
            let hold = arg(args, "--hold")
                .and_then(|s| s.parse().ok())
                .unwrap_or(3.0);
            windowed(&dir, hold)
        }
    });
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Ok(()) => {
            println!("mediabrowser: selftest ok");
            0
        }
        Err(e) => {
            println!("mediabrowser: selftest FAILED: {e}");
            1
        }
    }
}

fn ensure(ok: bool, what: impl Into<String>) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.into()) }
}

/// Run the app's pending updates and lay out a frame until the loader has
/// nothing left (or `limit` passes).
pub fn settle(app: &mut App<State>, limit: Duration) {
    let start = Instant::now();
    loop {
        app.run_pending();
        let _ = app.render_frame();
        app.run_pending();
        if app.state().loader.idle() && !app.run_pending() || start.elapsed() > limit {
            let _ = app.render_frame();
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Change the state as a menu command would.
fn command(app: &mut App<State>, f: impl FnOnce(&mut State) + Send + 'static) {
    app.handle().update(f);
    settle(app, Duration::from_secs(5));
}

fn shot(app: &mut App<State>, out: &Path, name: &str) {
    let (frame, _) = app.render_frame();
    let path = out.join(format!("mediabrowser-{name}.png"));
    match frame.save_png(&path) {
        Ok(()) => println!("mediabrowser: saved {}", path.display()),
        Err(e) => println!("mediabrowser: couldn't save {}: {e}", path.display()),
    }
}

/// The names of the table's body rows (their first cell), in order.
pub fn row_names(app: &mut App<State>) -> Vec<String> {
    let a = app.accessibility_tree().expect("a tree");
    a.descendants()
        .into_iter()
        .filter(|n| n.role == Role::Row && n.children.iter().all(|c| c.role == Role::Cell))
        .filter_map(|n| n.children.first().map(|c| c.name.clone()))
        .collect()
}

/// The middle of the table cell named `name`.
fn cell_at(app: &App<State>, name: &str) -> Result<Point, String> {
    let a = app.accessibility_tree().ok_or("no tree")?;
    a.descendants()
        .into_iter()
        .find(|n| n.role == Role::Cell && n.name == name)
        .map(|n| n.bounds.center())
        .ok_or_else(|| format!("no cell {name}"))
}

fn names_of(app: &App<State>) -> Vec<String> {
    let s = app.state();
    s.entries().iter().map(|e| e.name.clone()).collect()
}

fn offscreen(dir: &Path, out: &Path) -> Result<(), String> {
    let settings = Settings {
        file: Some(dir.join("mediabrowser.conf")),
        width: 1100.0,
        height: 700.0,
        ..Settings::default()
    };
    let vfs = Vfs::Disk(dir.to_path_buf());
    let mut state = State::new(vfs.clone(), Loader::new(vfs, 2), settings);
    state.audio = Some(Audio::shared());
    println!("mediabrowser: audio output {}", Audio::shared().backend());
    let mut app = build_app(state).theme(Theme::light());
    settle(&mut app, Duration::from_secs(5));

    // The tree shows the root's folders; picking one lists it.
    ensure(app.find(Role::Tree, "Folders").is_some(), "no folder tree")?;
    ensure(app.find(Role::Table, "Files").is_some(), "no file table")?;
    for name in ["Documents", "Music", "Pictures"] {
        ensure(
            app.find(Role::TreeItem, name).is_some(),
            format!("no tree item {name}"),
        )?;
    }
    ensure(app.click(Role::TreeItem, "Pictures"), "clicking Pictures")?;
    settle(&mut app, Duration::from_secs(5));
    ensure(
        app.state().cwd() == "Pictures",
        format!("cwd {:?}", app.state().cwd()),
    )?;
    let rows = row_names(&mut app);
    ensure(
        rows == ["Holidays", "Tiles", "checker.bmp", "sunset.png"],
        format!("Pictures lists {rows:?}"),
    )?;
    println!("mediabrowser: navigate ok: {rows:?}");

    // Sort by size, then the other way.
    ensure(app.click(Role::ColumnHeader, "Size"), "clicking Size")?;
    settle(&mut app, Duration::from_secs(1));
    let asc = names_of(&app);
    ensure(app.click(Role::ColumnHeader, "Size"), "clicking Size again")?;
    settle(&mut app, Duration::from_secs(1));
    let desc = names_of(&app);
    ensure(
        asc[2..] == ["sunset.png", "checker.bmp"] || asc[2..] == ["checker.bmp", "sunset.png"],
        format!("ascending {asc:?}"),
    )?;
    let files = |v: &[String]| v[2..].to_vec();
    let mut reversed = files(&asc);
    reversed.reverse();
    ensure(
        files(&desc) == reversed,
        format!("descending {desc:?} after {asc:?}"),
    )?;
    ensure(app.click(Role::ColumnHeader, "Name"), "clicking Name")?;
    settle(&mut app, Duration::from_secs(1));
    println!("mediabrowser: sort ok: {asc:?} / {desc:?}");

    // The keyboard: focus the list, move, type a name, select several.
    let first = cell_at(&app, "Holidays")?;
    ensure(app.click_at(first), "clicking the first row")?;
    ensure(app.press_key(KeyInput::Down), "Down")?;
    ensure(app.press_key(KeyInput::Text("s".into())), "typing s")?;
    settle(&mut app, Duration::from_secs(5));
    let cur = app.state().current().map(|e| e.name);
    ensure(
        cur.as_deref() == Some("sunset.png"),
        format!("type-to-select gave {cur:?}"),
    )?;
    ensure(
        app.find_text("320 × 200 pixels").is_some(),
        "the picture's dimensions aren't shown",
    )?;
    println!("mediabrowser: keyboard and image preview ok");
    shot(&mut app, out, "list");
    ensure(app.press_key(KeyInput::SelectUp), "Shift+Up")?;
    settle(&mut app, Duration::from_secs(1));
    ensure(app.state().chosen().len() == 2, "Shift+Up selects two")?;
    ensure(app.press_key(KeyInput::SelectAll), "Ctrl+A")?;
    ensure(app.state().chosen().len() == 4, "Ctrl+A selects all four")?;
    println!("mediabrowser: multi-select ok");

    // The grid: into Tiles, thumbnails decoded on the workers.
    command(&mut app, |s| s.set_mode(Mode::Grid));
    ensure(app.press_key(KeyInput::Home), "Home in the grid")?;
    // Home + Down + Enter opens "Tiles" (the second entry, one row down
    // or one step right, depending on the width): go by name instead.
    command(&mut app, |s| s.open_dir("Pictures/Tiles"));
    settle(&mut app, Duration::from_secs(10));
    let stats = app.state().loader.stats();
    ensure(stats.decoded >= 8, format!("thumbnails decoded: {stats:?}"))?;
    let tiles = app
        .accessibility_tree()
        .unwrap()
        .descendants()
        .into_iter()
        .filter(|n| n.role == Role::ListItem)
        .count();
    ensure(
        (8..48).contains(&tiles),
        format!("{tiles} tiles built of 48"),
    )?;
    shot(&mut app, out, "grid");
    // Scroll to the end at once: the tiles in between are never decoded.
    app.hover_at(Point::new(600.0, 300.0));
    app.scroll_by(5_000.0);
    settle(&mut app, Duration::from_secs(10));
    let after = app.state().loader.stats();
    println!(
        "mediabrowser: grid ok: {tiles} tiles built, {} thumbnails decoded, {} cancelled",
        after.decoded, after.cancelled
    );
    ensure(
        after.decoded < 48,
        "every thumbnail was decoded: nothing was skipped",
    )?;

    // Sound: Music, type "c", Enter plays chime.wav; seek; pause.
    command(&mut app, |s| {
        s.set_mode(Mode::List);
        s.open_dir("Music");
    });
    let cell = cell_at(&app, "scale.wav")?;
    ensure(app.click_at(cell), "clicking scale.wav")?;
    ensure(app.press_key(KeyInput::Text("c".into())), "typing c")?;
    ensure(app.press_key(KeyInput::Enter), "Enter")?;
    settle(&mut app, Duration::from_secs(5));
    let playing = app.state().player.as_ref().map(|p| p.path.clone());
    ensure(
        playing.as_deref() == Some("Music/chime.wav"),
        format!("playing {playing:?}"),
    )?;
    std::thread::sleep(Duration::from_millis(500));
    let pos = app.state().player.as_ref().unwrap().voice.position();
    ensure(
        pos > Duration::from_millis(150),
        format!("played to {pos:?} after 0.5 s"),
    )?;
    let tree = app.accessibility_tree().unwrap();
    let slider = tree
        .descendants()
        .into_iter()
        .find(|n| n.role == Role::Slider)
        .cloned()
        .ok_or("no position slider")?;
    println!("mediabrowser: slider {:?}", slider.name);
    // Drag the knob to three quarters: 1.1 s into the 1.5 s chime.
    let b = slider.bounds;
    let at = Point::new(b.min_x() + b.width() * 0.75, b.center().y);
    ensure(app.drag_at_point(at), "dragging the slider")?;
    app.end_drag();
    let pos = app.state().player.as_ref().unwrap().voice.position();
    ensure(
        pos >= Duration::from_millis(900),
        format!("seeking to three quarters left it at {pos:?}"),
    )?;
    settle(&mut app, Duration::from_secs(1));
    ensure(app.click(Role::Button, "Pause"), "clicking Pause")?;
    settle(&mut app, Duration::from_secs(1));
    ensure(
        app.state().player.as_ref().unwrap().voice.is_paused(),
        "Pause didn't pause",
    )?;
    println!("mediabrowser: audio ok ({})", Audio::shared().backend());
    shot(&mut app, out, "audio");

    // Markdown, in the root.
    command(&mut app, |s| {
        s.stop();
        s.open_dir("");
        if let Some(i) = s.entries().iter().position(|e| e.name == "README.md") {
            s.table.rows.select(i);
        }
    });
    ensure(
        app.find_text("Media browser sample").is_some(),
        "the Markdown heading isn't shown",
    )?;
    println!("mediabrowser: markdown ok");

    // French: the strings, the sizes and the dates.
    command(&mut app, |s| {
        s.open_dir("Pictures");
        s.set_language(Some("fr"));
    });
    ensure(
        app.find(Role::ColumnHeader, "Taille").is_some(),
        "no French Size header",
    )?;
    let cells: Vec<String> = app
        .accessibility_tree()
        .unwrap()
        .descendants()
        .into_iter()
        .filter(|n| n.role == Role::Cell)
        .map(|n| n.name.clone())
        .collect();
    ensure(
        cells.iter().any(|c| c.ends_with(" Ko")),
        format!("no size in Ko: {cells:?}"),
    )?;
    ensure(cells.iter().any(|c| c.contains("2026")), "no dates")?;
    println!("mediabrowser: French ok");
    shot(&mut app, out, "fr");

    // Arabic: mirrored, Arabic-Indic digits.
    command(&mut app, |s| s.set_language(Some("ar")));
    ensure(
        app.find(Role::ColumnHeader, "الحجم").is_some(),
        "no Arabic Size header",
    )?;
    let a = app.accessibility_tree().unwrap();
    let tree_x = a
        .descendants()
        .into_iter()
        .find(|n| n.role == Role::Tree)
        .map(|n| n.bounds.min_x())
        .unwrap_or(0.0);
    let table_x = a
        .descendants()
        .into_iter()
        .find(|n| n.role == Role::Table)
        .map(|n| n.bounds.min_x())
        .unwrap_or(0.0);
    ensure(
        tree_x > table_x,
        format!("not mirrored: tree at {tree_x}, table at {table_x}"),
    )?;
    let digits = a
        .descendants()
        .into_iter()
        .any(|n| n.name.chars().any(|c| ('٠'..='٩').contains(&c)));
    ensure(digits, "no Arabic-Indic digits")?;
    println!("mediabrowser: Arabic ok (mirrored)");
    shot(&mut app, out, "ar");

    // Dark, in Japanese, for the record.
    command(&mut app, |s| {
        s.set_language(Some("ja"));
        s.set_theme(super::settings::ThemeChoice::Dark);
    });
    let app = app.theme(Theme::dark());
    let mut app = app;
    settle(&mut app, Duration::from_secs(1));
    shot(&mut app, out, "ja-dark");

    // The settings went to their file.
    let saved = Settings::load(Some(dir.join("mediabrowser.conf")));
    ensure(
        saved.language.as_deref() == Some("ja") && saved.mode == Mode::List,
        format!("settings saved as {saved:?}"),
    )?;
    command(&mut app, |s| s.set_language(None));
    stipple::platform::set_preferred_languages(Vec::<String>::new());
    println!("mediabrowser: settings ok");
    Ok(())
}

/// The real window on the same folder for `hold` seconds: the grid of tiles
/// loading on the workers, then the list with a picture previewed and a
/// sound playing, checked from timers.
fn windowed(dir: &Path, hold: f64) -> Result<(), String> {
    let settings = Settings {
        file: Some(dir.join("mediabrowser-window.conf")),
        mode: Mode::Grid,
        ..Settings::default()
    };
    let vfs = Vfs::Disk(dir.to_path_buf());
    let mut state = State::new(vfs.clone(), Loader::new(vfs, 2), settings);
    state.open_dir("Pictures/Tiles");
    let app = build_app(state);
    let handle = app.handle();
    let report = std::sync::Arc::new(std::sync::Mutex::new(None::<Result<String, String>>));
    let half = Duration::from_secs_f64(hold / 2.0);
    handle.after(half, |s: &mut State| {
        println!("mediabrowser: window shows the list");
        s.set_mode(Mode::List);
        s.open_dir("Pictures");
        if let Some(i) = s.entries().iter().position(|e| e.name == "sunset.png") {
            s.table.rows.select(i);
        }
    });
    {
        let report = report.clone();
        let quit = handle.clone();
        handle.after(Duration::from_secs_f64(hold), move |s: &mut State| {
            let stats = s.loader.stats();
            let shown = s.current().map(|e| e.name);
            *report.lock().unwrap() = Some(
                if stats.decoded > 0 && shown.as_deref() == Some("sunset.png") {
                    Ok(format!(
                        "{} decoded, {} cancelled",
                        stats.decoded, stats.cancelled
                    ))
                } else {
                    Err(format!("in the window: {stats:?}, showing {shown:?}"))
                },
            );
            quit.exit();
        });
    }
    let start = Instant::now();
    println!("mediabrowser: opening the window for {hold} s");
    app.run();
    let result = report.lock().unwrap().take();
    match result {
        Some(Ok(line)) => {
            println!(
                "mediabrowser: window ok after {:.1} s: {line}",
                start.elapsed().as_secs_f64()
            );
            Ok(())
        }
        Some(Err(e)) => Err(e),
        None => Err("the window closed before its checks ran".into()),
    }
}
