//! An iOS app with an app extension, sharing files through an app group.
//!
//! The app (this file) writes `from-app.txt` into the app group's shared
//! container ([`platform::shared_dir`](stipple::platform::shared_dir)) and
//! shows what the extension last wrote there. The extension
//! (`extension.rs`, the `extensiondemo_ext` example) is a notification
//! service extension written in Rust: it registers its principal class with
//! the Objective-C runtime and hands the process to `NSExtensionMain`.
//! `cargo-stipple ios` builds both into one bundle, from
//! `examples/extensiondemo/package.toml`:
//!
//! ```sh
//! tools/package/cargo-stipple ios --simulator --example extensiondemo \
//!     --config examples/extensiondemo/package.toml
//! ```
//!
//! It logs what it finds on stdout and, on iOS, to `extensiondemo.log` in
//! the app's temporary directory, which CI reads. Elsewhere there is no app
//! group: the window says so.

use stipple::prelude::*;

/// The app group the app and its extension share (`ios.app-groups` in
/// package.toml).
const GROUP: &str = "group.com.karpeleslab.stipple.extdemo";

struct Demo {
    shared: Option<std::path::PathBuf>,
    from_extension: String,
}

impl Demo {
    fn reload(&mut self) {
        self.from_extension = self
            .shared
            .as_ref()
            .and_then(|d| std::fs::read_to_string(d.join("from-extension.txt")).ok())
            .unwrap_or_else(|| "(nothing yet)".into());
    }
}

fn log(line: &str) {
    println!("{line}");
    if cfg!(target_os = "ios") {
        use std::io::Write;
        let path = std::env::temp_dir().join("extensiondemo.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn view(state: &Demo, cx: &mut Cx<Demo>) -> Element {
    let theme = *cx.theme();
    let shared = match &state.shared {
        Some(d) => format!("Shared container: {}", d.display()),
        None => format!("No app group {GROUP} here (an iOS app's entitlement)."),
    };
    column(vec![
        label(cx, "App extension demo"),
        label(cx, &shared),
        label(cx, format!("The extension wrote: {}", state.from_extension)),
        button(cx, "Reload").on_tap(cx, |s| s.reload()),
    ])
    .gap(theme.spacing.md)
    .padding(Insets::uniform(theme.spacing.lg))
}

fn main() {
    let info = stipple::platform::app_info();
    log(&format!("extensiondemo: app id={}", info.id));
    let shared = stipple::platform::shared_dir(GROUP);
    match &shared {
        Some(dir) => {
            log(&format!("extensiondemo: app shared_dir={}", dir.display()));
            let wrote = std::fs::write(dir.join("from-app.txt"), format!("hello from {}", info.id));
            log(&format!(
                "extensiondemo: app wrote from-app.txt ok={}",
                wrote.is_ok()
            ));
        }
        None => log("extensiondemo: app shared_dir=None"),
    }
    let mut demo = Demo {
        shared,
        from_extension: String::new(),
    };
    demo.reload();
    App::new(demo, view)
        .title("Stipple Extensions")
        .logical_size(Size::new(480.0, 320.0))
        .run();
}
