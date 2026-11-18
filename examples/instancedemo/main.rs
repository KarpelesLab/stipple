//! One instance per user: run it again (with arguments, from another
//! directory) and the second process hands its launch to this one and exits;
//! the window lists every launch it heard, with its arguments, directory and
//! activation token, and comes to the front. On macOS it also hears the Dock:
//! a click on its icon (reopen), files and folders dropped on it, and its
//! right-click menu (New Window, Clear); its menu bar has a Window menu
//! (with the window list), a Services menu and a Help menu with a search
//! field.
//!
//! `--quit-after N` quits once N things were heard (launches, reopens,
//! opened files and URLs, Dock menu picks), each printed on stdout as it
//! comes, for CI: run it once with `--quit-after 1`, then again with
//! arguments, which forwards them and exits at once. On macOS,
//! `STIPPLE_COCOA_APP_DELEGATE=1` makes the backend send the app delegate a
//! reopen, a folder and a URL, and pick the Dock menu's first item.

use std::io::Write;
use std::time::Duration;
use stipple::platform::{Launch, WindowAttributes, WindowRequest};
use stipple::prelude::*;

const ID: &str = "org.stipple.InstanceDemo";

/// The app's handle, for the hooks and menu actions (which only get the
/// state) to quit and open windows with, from the first event on.
static HANDLE: std::sync::OnceLock<AppHandle<State>> = std::sync::OnceLock::new();

#[derive(Default)]
struct State {
    log: Vec<String>,
    windows: usize,
    quit_after: Option<usize>,
}

impl State {
    /// Note what was heard, on stdout too, and quit once there was enough.
    fn heard(&mut self, what: String) {
        println!("instancedemo: {what}");
        let _ = std::io::stdout().flush();
        self.log.push(what);
        if let (Some(n), Some(handle)) = (self.quit_after, HANDLE.get())
            && self.log.len() >= n
        {
            handle.exit();
        }
    }
}

fn view(state: &State, cx: &mut Cx<State>) -> Element {
    let mut rows = vec![
        heading(cx, "Single instance"),
        label(
            cx,
            format!(
                "Run `cargo run --example instancedemo -- some args` again: {} heard so far.",
                state.log.len()
            ),
        ),
    ];
    for line in state.log.iter().rev().take(12) {
        rows.push(label(cx, line.clone()));
    }
    panel(cx).children(vec![column(rows).gap(6.0)])
}

fn describe(launch: &Launch) -> String {
    format!(
        "launch args={:?} cwd={} token={:?}",
        launch.args,
        launch
            .cwd
            .as_ref()
            .map_or("?".into(), |p| p.display().to_string()),
        launch.activation_token
    )
}

fn new_window(s: &mut State) {
    s.windows += 1;
    if let Some(handle) = HANDLE.get() {
        let title = format!("Instance demo {}", s.windows + 1);
        handle.open_window(
            WindowAttributes::new()
                .title(title)
                .logical_size(Size::new(520.0, 360.0)),
            view,
        );
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut quit_after = None;
    while let Some(a) = args.next() {
        if a == "--quit-after" {
            quit_after = args.next().and_then(|n| n.parse().ok());
        }
    }
    // Quit before building anything when another instance takes this launch.
    if stipple::platform::forward_launch(ID) {
        println!("instancedemo: forwarded to the running instance");
        return;
    }
    let app = App::new(
        State {
            quit_after,
            ..State::default()
        },
        view,
    )
    .title("Instance demo")
    .logical_size(Size::new(520.0, 360.0))
    .app_info(stipple::platform::AppInfo::new(ID).name("Instance Demo"));
    let handle = app.handle();
    let _ = HANDLE.set(handle.clone());
    if quit_after.is_some() {
        // CI shouldn't hang on a launch that never comes.
        handle.after(Duration::from_secs(60), |s: &mut State| {
            println!("instancedemo: timed out after {:?}", s.log);
            std::process::exit(1);
        });
    }
    let activate = handle.clone();
    app.single_instance(ID, move |s, launch| {
        s.heard(describe(launch));
        // The window that shows the launch comes to the front.
        activate.window(WindowRequest::Activate(launch.activation_token.clone()));
    })
    .on_reopen(|s, visible| {
        s.heard(format!("reopen visible={visible}"));
        if !visible {
            new_window(s);
        }
    })
    .on_open_files(|s, files| s.heard(format!("open files {files:?}")))
    .on_open_url(|s, url| s.heard(format!("open url {url}")))
    .dock_menu(|s: &State| {
        Menu::new("")
            .item(MenuItem::new("New Window", |s: &mut State| {
                s.heard("dock: New Window".into());
                new_window(s);
            }))
            .item(MenuItem::new("Clear", |s: &mut State| s.log.clear()).enabled(!s.log.is_empty()))
    })
    .menu(|_| {
        MenuBar::new()
            .menu(
                Menu::new("&File").item(
                    MenuItem::new("&New Window", |s: &mut State| new_window(s))
                        .shortcut(Shortcut::cmd('n')),
                ),
            )
            .menu(Menu::new("Services").role(MenuRole::Services))
            .menu(Menu::new("&Window").role(MenuRole::Window))
            .menu(
                Menu::new("&Help")
                    .role(MenuRole::Help)
                    .item(MenuItem::new("Instance Demo Help", |s: &mut State| {
                        s.heard("help".into())
                    })),
            )
    })
    .run();
}
