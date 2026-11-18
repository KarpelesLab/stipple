//! Launches in the [`App`]: one running instance that later launches hand
//! themselves to ([`App::single_instance`]), the app opened again while it
//! runs ([`App::on_reopen`]), files opened with it ([`App::on_open_files`]),
//! and macOS's Dock menu ([`App::dock_menu`]). See `docs/os-integration.md`.

use std::path::PathBuf;
use std::rc::Rc;

use crate::App;
use crate::core::Menu;
use crate::platform::{Launch, NativeMenu};

type LaunchFn<S> = Box<dyn FnMut(&mut S, &Launch)>;
type ReopenFn<S> = Box<dyn FnMut(&mut S, bool)>;
type FilesFn<S> = Box<dyn FnMut(&mut S, &[PathBuf])>;
type DockFn<S> = Rc<dyn Fn(&S) -> Menu<S>>;

/// The app's launch hooks.
pub(crate) struct Hooks<S> {
    /// The id the app runs once under, and what hears the launches after.
    single: Option<(String, LaunchFn<S>)>,
    reopen: Option<ReopenFn<S>>,
    open_files: Option<FilesFn<S>>,
    dock: Option<DockFn<S>>,
    /// The Dock menu last handed to the backend.
    native_dock: Option<NativeMenu>,
}

impl<S> Default for Hooks<S> {
    fn default() -> Self {
        Self {
            single: None,
            reopen: None,
            open_files: None,
            dock: None,
            native_dock: None,
        }
    }
}

impl<S: 'static> App<S> {
    /// Run once per user under `id` (the app's reverse-DNS identifier, as in
    /// [`AppInfo`](crate::platform::AppInfo)): when [`run`](Self::run)
    /// finds another process of the app already running, it hands that one
    /// this launch (its arguments, working directory, environment and
    /// activation token) and returns at once, so `main` ends; otherwise the
    /// app runs and `on_launch` hears every launch that comes after. Typical
    /// of editors, browsers and terminals: running the app again opens a
    /// window in the one already running. A URL among the launch's arguments
    /// (a protocol handler's) reaches [`on_open_url`](Self::on_open_url) as
    /// well.
    ///
    /// The window that shows the launch takes the focus with its activation
    /// token: [`WindowRequest::Activate`](crate::platform::WindowRequest::Activate).
    /// An app that must know before it builds its state calls
    /// [`platform::forward_launch`](crate::platform::forward_launch) first.
    ///
    /// Linux and macOS use a Unix socket in the user's own runtime directory,
    /// Windows a named pipe; on macOS, opening a bundled app's icon again
    /// doesn't start a process at all but reaches the running one as
    /// [`on_reopen`](Self::on_reopen). iOS, Android and the web keep one
    /// instance themselves, and never call `on_launch`. See
    /// `docs/os-integration.md`.
    ///
    /// ```no_run
    /// use stipple::prelude::*;
    /// use stipple::platform::{Launch, WindowAttributes, WindowRequest};
    /// struct Term { windows: usize }
    /// # fn view(_: &Term, _: &mut Cx<Term>) -> Element { spacer() }
    /// let app = App::new(Term { windows: 1 }, view);
    /// let handle = app.handle();
    /// app.single_instance("org.example.Term", move |s, launch: &Launch| {
    ///     s.windows += 1;
    ///     let id = handle.open_window(WindowAttributes::new().title("Term"), view);
    ///     handle.window_to(id, WindowRequest::Activate(launch.activation_token.clone()));
    /// })
    /// .run();
    /// ```
    pub fn single_instance(
        mut self,
        id: impl Into<String>,
        on_launch: impl FnMut(&mut S, &Launch) + 'static,
    ) -> Self {
        self.instance.single = Some((id.into(), Box::new(on_launch)));
        self
    }

    /// Claim the [`single_instance`](Self::single_instance) id, if there is
    /// one. Returns whether another process took this launch, so the app
    /// shouldn't run.
    pub(crate) fn forwarded_to_instance(&self) -> bool {
        let Some((id, _)) = &self.instance.single else {
            return false;
        };
        crate::platform::instance::claim(id) == crate::platform::instance::Claim::Forwarded
    }

    /// Run the launch hook for a launch another process handed over.
    /// Returns whether it ran.
    pub(crate) fn launched(&mut self, launch: &Launch) -> bool {
        let Some((_, hook)) = self.instance.single.as_mut() else {
            return false;
        };
        hook(&mut self.state, launch);
        self.mark_all_dirty();
        true
    }

    /// Hear the user opening the app again while it runs: on macOS, a click
    /// on its Dock icon, or launching it from the Finder or with `open`
    /// (`applicationShouldHandleReopen:hasVisibleWindows:`), with whether any
    /// of the app's windows show. An app that keeps running with no window
    /// ([`exit_on_last_window_closed(false)`](Self::exit_on_last_window_closed))
    /// opens one here, as Mac apps do. The hook may change the state; the
    /// view is rebuilt. Only macOS reports it; elsewhere a new launch is a
    /// new process (see [`single_instance`](Self::single_instance)).
    ///
    /// ```no_run
    /// use stipple::prelude::*;
    /// use stipple::platform::WindowAttributes;
    /// # fn view(_: &(), _: &mut Cx<()>) -> Element { spacer() }
    /// let app = App::new((), view).exit_on_last_window_closed(false);
    /// let handle = app.handle();
    /// app.on_reopen(move |_, has_visible_windows| {
    ///     if !has_visible_windows {
    ///         handle.open_window(WindowAttributes::new().title("Untitled"), view);
    ///     }
    /// })
    /// .run();
    /// ```
    pub fn on_reopen(mut self, hook: impl FnMut(&mut S, bool) + 'static) -> Self {
        self.instance.reopen = Some(Box::new(hook));
        self
    }

    /// Run the reopen hook. Returns whether it ran.
    pub(crate) fn reopened(&mut self, has_visible_windows: bool) -> bool {
        let Some(hook) = self.instance.reopen.as_mut() else {
            return false;
        };
        hook(&mut self.state, has_visible_windows);
        self.mark_all_dirty();
        true
    }

    /// Hear files and folders the system asks the app to open: on macOS,
    /// dropped on its Dock icon, opened with it from the Finder (Open With,
    /// a document type the bundle claims, a folder dropped on a terminal's
    /// icon), at launch or while it runs (`application:openURLs:`, its file
    /// URLs; other URLs go to [`on_open_url`](Self::on_open_url)). Elsewhere
    /// the files come on the command line (and, to a running
    /// [single instance](Self::single_instance), in its
    /// [`Launch`]'s arguments). The hook may change the state; the view is
    /// rebuilt.
    ///
    /// ```no_run
    /// use stipple::prelude::*;
    /// struct State { tabs: Vec<std::path::PathBuf> }
    /// # fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
    /// App::new(State { tabs: Vec::new() }, view)
    ///     .on_open_files(|s, files| s.tabs.extend(files.iter().cloned()))
    ///     .run();
    /// ```
    pub fn on_open_files(mut self, hook: impl FnMut(&mut S, &[PathBuf]) + 'static) -> Self {
        self.instance.open_files = Some(Box::new(hook));
        self
    }

    /// Run the open-files hook. Returns whether it ran.
    pub(crate) fn open_files(&mut self, files: &[PathBuf]) -> bool {
        let Some(hook) = self.instance.open_files.as_mut() else {
            return false;
        };
        hook(&mut self.state, files);
        self.mark_all_dirty();
        true
    }

    /// Give the app's Dock icon a menu (macOS: its right-click menu, above
    /// the system's own items), as a function of the state like
    /// [`menu`](Self::menu) and [`tray`](Self::tray): New Window, New Tab, a
    /// list of recent documents. Picking an item runs its action against the
    /// state. The menu's title isn't shown, and shortcuts don't apply. Other
    /// platforms have no Dock menu and ignore it.
    ///
    /// ```
    /// use stipple::prelude::*;
    /// struct State { windows: usize }
    /// let app = App::new(State { windows: 1 }, |_: &State, cx: &mut Cx<State>| {
    ///     label(cx, "Hello")
    /// })
    /// .dock_menu(|_| Menu::new("").item(MenuItem::new("New Window", |s: &mut State| s.windows += 1)));
    /// ```
    pub fn dock_menu(mut self, menu: impl Fn(&S) -> Menu<S> + 'static) -> Self {
        self.instance.dock = Some(Rc::new(menu));
        self
    }

    /// Run the Dock menu's command with native id `id` (from
    /// [`DOCK_ID_BASE`](crate::platform::DOCK_ID_BASE)). Returns whether it
    /// ran.
    pub(crate) fn run_dock_command(&mut self, id: u32) -> bool {
        let Some(dock) = self.instance.dock.clone() else {
            return false;
        };
        let ran =
            dock(&self.state).activate_from(crate::platform::DOCK_ID_BASE, id, &mut self.state);
        if ran {
            self.mark_all_dirty();
        }
        ran
    }

    /// The Dock menu as the platform gets it, when it changed since last
    /// handed over.
    fn dock_changed(&mut self) -> Option<NativeMenu> {
        let dock = self.instance.dock.clone()?;
        let native = dock(&self.state).to_native_from(crate::platform::DOCK_ID_BASE);
        if self.instance.native_dock.as_ref() == Some(&native) {
            return None;
        }
        self.instance.native_dock = Some(native.clone());
        Some(native)
    }

    /// Hand the Dock menu to `window` when it changed.
    pub(crate) fn sync_dock(&mut self, window: &dyn crate::platform::Window) {
        if let Some(native) = self.dock_changed() {
            window.set_dock_menu(Some(&native));
        }
    }

    fn mark_all_dirty(&mut self) {
        for pane in &mut self.panes {
            pane.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::platform::{DOCK_ID_BASE, Event, Launch};
    use crate::prelude::*;

    #[derive(Default)]
    struct State {
        launches: Vec<Vec<String>>,
        reopened: Vec<bool>,
        files: Vec<std::path::PathBuf>,
        picked: u32,
    }

    fn app() -> App<State> {
        App::new(State::default(), |_: &State, cx: &mut Cx<State>| {
            label(cx, "x")
        })
        .no_font()
        .single_instance("org.example.Test", |s, l: &Launch| {
            s.launches.push(l.args.clone())
        })
        .on_reopen(|s, visible| s.reopened.push(visible))
        .on_open_files(|s, files| s.files.extend(files.iter().cloned()))
        .dock_menu(|s: &State| {
            Menu::new("")
                .item(MenuItem::new(
                    format!("Picked {}", s.picked),
                    |s: &mut State| s.picked += 1,
                ))
                .item(MenuItem::new("Off", |_: &mut State| {}).enabled(false))
        })
    }

    #[test]
    fn launches_reopens_and_files_reach_their_hooks() {
        let mut app = app();
        let launch = Launch {
            args: vec!["a".into()],
            ..Launch::default()
        };
        crate::platform::shell::post(Event::Launch(launch));
        crate::platform::shell::post(Event::OpenFiles(vec!["/tmp/x".into()]));
        assert!(app.run_pending());
        assert!(app.reopened(false));
        let s = app.state();
        assert_eq!(s.launches, [vec!["a".to_string()]]);
        assert_eq!(s.files, [std::path::PathBuf::from("/tmp/x")]);
        assert_eq!(s.reopened, [false]);
    }

    #[test]
    fn the_dock_menu_follows_the_state_and_runs_its_commands() {
        let mut app = app();
        let first = app.dock_changed().expect("a first menu");
        assert!(app.dock_changed().is_none(), "unchanged");
        assert!(app.run_dock_command(DOCK_ID_BASE));
        assert!(!app.run_dock_command(DOCK_ID_BASE + 1), "disabled");
        assert_eq!(app.state().picked, 1);
        let second = app.dock_changed().expect("relabelled");
        assert_ne!(first, second);
    }

    #[test]
    fn without_hooks_nothing_runs() {
        let mut app = App::new((), |_: &(), cx: &mut Cx<()>| label(cx, "x")).no_font();
        assert!(!app.launched(&Launch::default()));
        assert!(!app.reopened(true));
        assert!(!app.open_files(&[]));
        assert!(!app.run_dock_command(DOCK_ID_BASE));
        assert!(!app.forwarded_to_instance());
    }
}
