//! A media browser: the Phase 3 reference application.
//!
//! A folder tree, a breadcrumb, and the folder's files as a sortable table
//! (name, size, kind, modified) or a grid of thumbnails, with a preview pane
//! that shows pictures (oxideav's decoders), plays sounds (the toolkit's
//! audio: play, pause, seek), renders text and Markdown (rich text), and
//! lists each file's metadata. Thumbnails and previews are decoded on worker
//! threads and cached; a thumbnail whose tile scrolls away before its turn
//! is never decoded.
//!
//! Everything works from the keyboard (Tab between the panes; arrows,
//! Home/End, Shift to extend, Ctrl+A, typing a name to jump to it, Enter to
//! open, Backspace for the enclosing folder), has a role and a name for
//! screen readers, and is translated: English, French, Japanese and Arabic
//! (mirrored), with sizes, dates and durations in the locale's format. The
//! window's size, the theme, the language, the view and the folder are
//! remembered.
//!
//! ```sh
//! cargo run --example mediabrowser                  # the last folder, or the sample
//! cargo run --example mediabrowser -- ~/Pictures    # a folder
//! cargo run --example mediabrowser -- --sample      # a generated sample folder
//! cargo run --example mediabrowser -- --sample --select Music/chime.wav
//! cargo run --example mediabrowser -- --selftest    # see selftest.rs
//! ```
//!
//! In the browser it browses the sample, held in memory.

mod i18n;
mod loader;
mod sample;
#[cfg(not(target_arch = "wasm32"))]
mod selftest;
mod settings;
mod vfs;
mod view;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use stipple::audio::{Audio, PlayParams, Voice};
use stipple::core::write_clipboard;
use stipple::platform::clipboard::{ClipboardContent, ClipboardKind};
use stipple::platform::time::Instant;
use stipple::prelude::*;

use i18n::Tr;
use loader::{Job, Loaded, Loader};
use settings::{Mode, Settings, ThemeChoice};
use vfs::{Entry, Kind, Vfs};

/// A folder and the order its entries are shown in.
type SortKey = (String, Option<(usize, SortOrder)>);

/// The folder tree's model: ids for folder paths, and every listing read,
/// cached until a refresh.
#[derive(Debug)]
pub struct Folders {
    vfs: Vfs,
    listings: RefCell<HashMap<String, Rc<Vec<Entry>>>>,
    sorted: RefCell<Option<(SortKey, Rc<Vec<Entry>>)>>,
    ids: RefCell<HashMap<String, u64>>,
    paths: RefCell<Vec<String>>,
}

impl Folders {
    fn new(vfs: Vfs) -> Self {
        Folders {
            vfs,
            listings: RefCell::default(),
            sorted: RefCell::default(),
            ids: RefCell::default(),
            paths: RefCell::default(),
        }
    }

    /// The tree id of folder `path` (made on first use).
    pub fn id(&self, path: &str) -> u64 {
        if let Some(id) = self.ids.borrow().get(path) {
            return *id;
        }
        let mut paths = self.paths.borrow_mut();
        let id = paths.len() as u64;
        paths.push(path.to_string());
        self.ids.borrow_mut().insert(path.to_string(), id);
        id
    }

    pub fn path(&self, id: u64) -> String {
        self.paths
            .borrow()
            .get(id as usize)
            .cloned()
            .unwrap_or_default()
    }

    /// The entries of `dir`, as read (cached).
    pub fn listing(&self, dir: &str) -> Rc<Vec<Entry>> {
        if let Some(l) = self.listings.borrow().get(dir) {
            return l.clone();
        }
        let list = Rc::new(self.vfs.list(dir).unwrap_or_default());
        self.listings
            .borrow_mut()
            .insert(dir.to_string(), list.clone());
        list
    }

    /// The entries of `dir` in display order: folders first, then by the
    /// sorted column (by name when there's none).
    pub fn sorted(&self, dir: &str, sort: Option<(usize, SortOrder)>) -> Rc<Vec<Entry>> {
        let key = (dir.to_string(), sort);
        if let Some((k, list)) = &*self.sorted.borrow()
            && *k == key
        {
            return list.clone();
        }
        let mut list = (*self.listing(dir)).clone();
        sort_entries(&mut list, sort);
        let list = Rc::new(list);
        *self.sorted.borrow_mut() = Some((key, list.clone()));
        list
    }

    /// Read every folder again.
    fn refresh(&self) {
        self.listings.borrow_mut().clear();
        *self.sorted.borrow_mut() = None;
    }
}

/// Folders first; then by `sort`'s column: 0 name, 1 size, 2 kind, 3 date.
pub fn sort_entries(list: &mut [Entry], sort: Option<(usize, SortOrder)>) {
    let (column, order) = sort.unwrap_or((0, SortOrder::Ascending));
    let name = |e: &Entry| e.name.to_lowercase();
    list.sort_by(|a, b| {
        let folders = (b.kind == Kind::Folder).cmp(&(a.kind == Kind::Folder));
        let by = match column {
            1 => a.size.cmp(&b.size),
            2 => a.kind.cmp(&b.kind),
            3 => a.modified.cmp(&b.modified),
            _ => std::cmp::Ordering::Equal,
        }
        .then_with(|| name(a).cmp(&name(b)));
        folders.then(match order {
            SortOrder::Ascending => by,
            SortOrder::Descending => by.reverse(),
        })
    });
}

impl TreeModel for Folders {
    fn children(&self, parent: Option<u64>) -> Vec<u64> {
        let Some(id) = parent else {
            return vec![self.id("")];
        };
        let dir = self.path(id);
        let mut subs: Vec<Entry> = self
            .listing(&dir)
            .iter()
            .filter(|e| e.kind == Kind::Folder)
            .cloned()
            .collect();
        sort_entries(&mut subs, None);
        subs.iter().map(|e| self.id(&e.path)).collect()
    }

    fn has_children(&self, id: u64) -> bool {
        let dir = self.path(id);
        self.listing(&dir).iter().any(|e| e.kind == Kind::Folder)
    }
}

/// The sound playing in the preview.
#[derive(Debug)]
pub struct Player {
    pub path: String,
    pub voice: Voice,
    pub duration: Duration,
}

#[derive(Debug)]
pub struct State {
    pub vfs: Vfs,
    pub loader: Loader,
    pub folders: Folders,
    pub tree: TreeState,
    pub table: TableState,
    /// The folder the table's selection was made in: a folder picked in the
    /// tree starts with nothing selected.
    pub list_dir: String,
    pub outer: SplitPane,
    pub inner: SplitPane,
    pub tr: Rc<Tr>,
    pub settings: Settings,
    /// Opened on first use (tests give an offline one).
    pub audio: Option<Audio>,
    pub player: Option<Player>,
    ticker: Option<TimerId>,
    /// Type-to-select: what was typed, and when.
    typed: (String, Option<Instant>),

    /// The last action's message, in the status bar.
    pub status: Option<String>,
    pub handle: Option<AppHandle<State>>,
    /// Bumped when the loader finishes something, so the view is built again.
    pub loaded: u64,
}

impl State {
    pub fn new(vfs: Vfs, loader: Loader, settings: Settings) -> State {
        let language = settings.language.clone();
        let mut s = State {
            folders: Folders::new(vfs.clone()),
            vfs,
            loader,
            tree: TreeState::new(),
            table: TableState::new(SelectionMode::Multi),
            list_dir: String::new(),
            outer: SplitPane::horizontal(settings.outer).min_sizes(150.0, 360.0),
            inner: SplitPane::horizontal(settings.inner).min_sizes(240.0, 200.0),
            tr: Rc::new(Tr::new(language.as_deref())),
            settings,
            audio: None,
            player: None,
            ticker: None,
            typed: (String::new(), None),
            status: None,
            handle: None,
            loaded: 0,
        };
        s.open_dir("");
        s
    }

    /// The folder shown: the one selected in the tree.
    pub fn cwd(&self) -> String {
        self.tree
            .selected()
            .map(|id| self.folders.path(id))
            .unwrap_or_default()
    }

    /// The folder's entries, in display order.
    pub fn entries(&self) -> Rc<Vec<Entry>> {
        self.folders.sorted(&self.cwd(), self.table.sort())
    }

    /// Whether the table's selection belongs to the folder shown.
    pub fn selection_current(&self) -> bool {
        self.list_dir == self.cwd()
    }

    /// The table's state, its selection dropped if the folder changed (the
    /// accessor the table and grid edit it through).
    pub fn table_mut(&mut self) -> &mut TableState {
        let cwd = self.cwd();
        if self.list_dir != cwd {
            self.table.rows = ListState::new(SelectionMode::Multi);
            self.list_dir = cwd;
        }
        &mut self.table
    }

    /// The entry under the keyboard cursor, if any.
    pub fn current(&self) -> Option<Entry> {
        if !self.selection_current() {
            return None;
        }
        let i = self.table.rows.cursor()?;
        self.entries().get(i).cloned()
    }

    /// The selected entries, else the one under the cursor.
    pub fn chosen(&self) -> Vec<Entry> {
        if !self.selection_current() {
            return Vec::new();
        }
        let entries = self.entries();
        let picked: Vec<Entry> = self
            .table
            .rows
            .selected()
            .filter_map(|i| entries.get(i).cloned())
            .collect();
        if picked.is_empty() {
            self.current().into_iter().collect()
        } else {
            picked
        }
    }

    // --- navigation ---

    /// Show folder `path`: expand the tree down to it and select it.
    pub fn open_dir(&mut self, path: &str) {
        let mut at = Some(path);
        while let Some(p) = at {
            let id = self.folders.id(p);
            if p != path {
                self.tree.set_expanded(id, true);
            }
            at = vfs::parent(p);
        }
        let id = self.folders.id(path);
        self.tree.set_expanded(id, true);
        self.tree.select(Some(id));
        self.table.rows = ListState::new(SelectionMode::Multi);
        self.list_dir = path.to_string();
        self.typed = (String::new(), None);
    }

    /// The enclosing folder, with the one we came from selected in it.
    pub fn go_up(&mut self) {
        let cwd = self.cwd();
        if let Some(parent) = vfs::parent(&cwd) {
            let parent = parent.to_string();
            self.open_dir(&parent);
            if let Some(i) = self.entries().iter().position(|e| e.path == cwd) {
                self.table.rows.select(i);
            }
        }
    }

    /// Open the entry at display row `index`: enter a folder, play a sound,
    /// hand anything else to the system.
    pub fn activate(&mut self, index: usize) {
        let Some(entry) = self.entries().get(index).cloned() else {
            return;
        };
        match entry.kind {
            Kind::Folder => self.open_dir(&entry.path),
            Kind::Audio => self.toggle_play(&entry.path),
            _ => {
                if let Some(path) = self.vfs.real_path(&entry.path) {
                    let _ = stipple::platform::open_path(path);
                }
            }
        }
    }

    /// Type-to-select: jump to the next entry whose name starts with what
    /// was typed in the last second.
    pub fn type_select(&mut self, text: &str) {
        let now = Instant::now();
        let fresh = self
            .typed
            .1
            .is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs(1));
        if !fresh {
            self.typed.0.clear();
        }
        self.typed.0.push_str(&text.to_lowercase());
        self.typed.1 = Some(now);
        let entries = self.entries();
        let n = entries.len();
        if n == 0 {
            return;
        }
        let prefix = self.typed.0.clone();
        // A repeated letter steps through the names starting with it.
        let repeat = prefix.chars().count() > 1 && prefix.chars().all(|c| prefix.starts_with(c));
        let (needle, skip) = if repeat {
            (prefix.chars().next().unwrap().to_string(), 1)
        } else {
            (prefix.clone(), usize::from(prefix.chars().count() == 1))
        };
        let cursor = self.table_mut().rows.cursor();
        let find = |needle: &str, skip: usize| {
            let start = cursor.map_or(0, |c| c + skip);
            (0..n)
                .map(|k| (start + k) % n)
                .find(|&i| entries[i].name.to_lowercase().starts_with(needle))
        };
        // Nothing starts with what was typed: start again from this key.
        let found = find(&needle, skip).or_else(|| {
            let lone = text.to_lowercase();
            let hit = find(&lone, 1);
            if hit.is_some() {
                self.typed.0 = lone;
            }
            hit
        });
        if let Some(i) = found {
            self.table.rows.select(i);
        }
    }

    /// Keys the file list and the grid share, after their own: Enter opens,
    /// Backspace goes up, typing selects.
    pub fn list_key(&mut self, key: &KeyInput) {
        match key {
            KeyInput::Enter => {
                if let Some(i) = self.table_mut().rows.cursor() {
                    self.activate(i);
                }
            }
            KeyInput::Backspace => self.go_up(),
            KeyInput::Text(t) if t != " " && !t.chars().any(char::is_control) => {
                self.type_select(t)
            }
            _ => {}
        }
    }

    /// Open the item at `index` once the event that asked is over (a double
    /// click's press: the row's own click still has to select it).
    pub fn activate_later(&mut self, index: usize) {
        match &self.handle {
            Some(h) => h.update(move |s: &mut State| s.activate(index)),
            None => self.activate(index),
        }
    }

    /// Make sure the item at `index` is among those a command acts on: a
    /// right-click outside the selection selects just it.
    pub fn pick(&mut self, index: usize) {
        let rows = &mut self.table_mut().rows;
        if !rows.is_selected(index) {
            rows.select(index);
        }
    }

    /// What dragging the item at `index` carries: the selection if it's in
    /// it, else just that item; files on disk, paths otherwise.
    pub fn drag_payload(&mut self, index: usize) -> Option<DropData> {
        let entries = self.entries();
        let rows = &self.table_mut().rows;
        let picked: Vec<Entry> = if rows.is_selected(index) {
            rows.selected()
                .filter_map(|i| entries.get(i).cloned())
                .collect()
        } else {
            entries.get(index).cloned().into_iter().collect()
        };
        if picked.is_empty() {
            return None;
        }
        match &self.vfs {
            Vfs::Disk(_) => Some(DropData::Files(
                picked
                    .iter()
                    .filter_map(|e| self.vfs.real_path(&e.path))
                    .collect(),
            )),
            Vfs::Memory(_) => Some(DropData::Text(
                picked
                    .iter()
                    .map(|e| self.vfs.display_path(&e.path))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )),
        }
    }

    /// The grid's keys: the arrows move through `cols` tiles a row (Left and
    /// Right swapped when mirrored), Shift extends, then the list's keys.
    pub fn grid_key(&mut self, key: &KeyInput, cols: usize, rtl: bool) {
        let n = self.entries().len();
        if n == 0 {
            return;
        }
        let rows = &mut self.table_mut().rows;
        let at = rows.cursor().map(|c| c.min(n - 1));
        let step = |by: isize| -> usize {
            match at {
                None => 0,
                Some(c) => (c as isize + by).clamp(0, n as isize - 1) as usize,
            }
        };
        let (left, right) = if rtl { (1, -1) } else { (-1, 1) };
        let cols = cols as isize;
        match key {
            KeyInput::Left => rows.select(step(left)),
            KeyInput::Right => rows.select(step(right)),
            KeyInput::Up => rows.select(step(-cols)),
            KeyInput::Down => rows.select(step(cols)),
            KeyInput::SelectLeft => rows.extend_to(step(left)),
            KeyInput::SelectRight => rows.extend_to(step(right)),
            KeyInput::SelectUp => rows.extend_to(step(-cols)),
            KeyInput::SelectDown => rows.extend_to(step(cols)),
            KeyInput::Home => rows.select(0),
            KeyInput::End => rows.select(n - 1),
            KeyInput::SelectHome => rows.extend_to(0),
            KeyInput::SelectEnd => rows.extend_to(n - 1),
            KeyInput::SelectAll => rows.select_all(n),
            KeyInput::Text(t) if t == " " => match at {
                Some(c) => rows.toggle(c),
                None => rows.select(0),
            },
            KeyInput::Escape => rows.clear(),
            other => self.list_key(other),
        }
    }

    // --- commands ---

    pub fn copy_path(&mut self) {
        let paths: Vec<String> = self
            .chosen()
            .iter()
            .map(|e| self.vfs.display_path(&e.path))
            .collect();
        let paths = if paths.is_empty() {
            vec![self.vfs.display_path(&self.cwd())]
        } else {
            paths
        };
        let text = paths.join("\n");
        write_clipboard(ClipboardKind::Clipboard, ClipboardContent::from_text(&text));
        self.status = Some(self.tr.f("status.copied", &[("path", &text)]));
    }

    /// Open the folder holding the chosen entry in the system's file manager.
    pub fn reveal(&mut self) {
        let path = self
            .chosen()
            .first()
            .map_or_else(|| self.cwd(), |e| e.path.clone());
        let dir = vfs::parent(&path).unwrap_or("");
        if let Some(real) = self.vfs.real_path(dir) {
            let _ = stipple::platform::open_path(real);
        }
    }

    pub fn select_all(&mut self) {
        let n = self.entries().len();
        self.table_mut().rows.select_all(n);
    }

    pub fn refresh(&mut self) {
        self.folders.refresh();
        self.loader.forget_all();
        let n = self.entries().len();
        self.table.rows.truncate(n);
    }

    /// Copy files dropped on the list into the folder shown.
    pub fn drop_files(&mut self, files: &[PathBuf]) {
        let n = self.vfs.copy_in(&self.cwd(), files);
        self.refresh();
        self.status = Some(self.tr.plural("status.dropped", n));
    }

    /// Browse `vfs` from its root.
    pub fn set_root(&mut self, vfs: Vfs) {
        self.stop();
        self.settings.root = vfs.real_path("");
        self.settings.save();
        self.loader.reset(vfs.clone());
        self.folders = Folders::new(vfs.clone());
        self.vfs = vfs;
        self.tree = TreeState::new();
        self.table.rows = ListState::new(SelectionMode::Multi);
        self.open_dir("");
    }

    /// Pick a folder with the system's dialog and browse it.
    pub fn pick_root(&mut self) {
        let title = self.tr.t("tool.open").replace('…', "");
        let dialog = stipple::platform::dialog::FileDialog::new().title(title);
        if let Some(dir) = stipple::platform::dialog::pick_folder(&dialog) {
            self.set_root(Vfs::Disk(dir));
        }
    }

    pub fn set_language(&mut self, choice: Option<&str>) {
        self.settings.language = choice.map(str::to_string);
        self.settings.save();
        self.tr = Rc::new(Tr::new(choice));
        // Stipple's own strings (a message box's OK) follow.
        stipple::platform::set_preferred_languages(choice.into_iter().map(str::to_string));
    }

    pub fn set_theme(&mut self, theme: ThemeChoice) {
        self.settings.theme = theme;
        self.settings.save();
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.settings.mode = mode;
        self.settings.save();
    }

    /// The window moved or resized: remember its size.
    fn window_changed(&mut self, info: &WindowInfo) {
        if !info.minimized && !info.fullscreen {
            self.settings.maximized = info.maximized;
            if !info.maximized {
                self.settings.width = info.inner_size.width;
                self.settings.height = info.inner_size.height;
            }
            self.settings.outer = self.outer.fraction();
            self.settings.inner = self.inner.fraction();
            self.settings.save();
        }
    }

    // --- playback ---

    fn audio(&mut self) -> Audio {
        self.audio.get_or_insert_with(Audio::shared).clone()
    }

    /// Play the sound at `path`, or pause / resume it if it's the one
    /// playing.
    pub fn toggle_play(&mut self, path: &str) {
        if let Some(p) = &self.player
            && p.path == path
            && p.voice.is_playing()
        {
            if p.voice.is_paused() {
                p.voice.resume();
            } else {
                p.voice.pause();
            }
            return;
        }
        self.stop();
        let sound = match self.loader.get(Job::Preview, path).as_deref() {
            Some(Loaded::Sound(s)) => s.clone(),
            _ => match loader::load(&self.vfs, Job::Preview, path) {
                Loaded::Sound(s) => s,
                _ => return,
            },
        };
        let voice = self.audio().play(&sound, PlayParams::default());
        self.player = Some(Player {
            path: path.to_string(),
            voice,
            duration: sound.duration(),
        });
        // Move the position along while it plays.
        if let Some(h) = &self.handle {
            self.ticker = Some(h.every(Duration::from_millis(200), |s: &mut State| s.tick()));
        }
    }

    /// Play or pause what's under the cursor (or what's playing).
    pub fn play_pause(&mut self) {
        let path = match (&self.player, self.current()) {
            (_, Some(e)) if e.kind == Kind::Audio => e.path,
            (Some(p), _) => p.path.clone(),
            _ => return,
        };
        self.toggle_play(&path);
    }

    pub fn stop(&mut self) {
        if let Some(p) = self.player.take() {
            p.voice.stop();
        }
        if let (Some(h), Some(t)) = (&self.handle, self.ticker.take()) {
            h.cancel(t);
        }
    }

    /// Jump to `fraction` of the sound playing.
    pub fn seek(&mut self, fraction: f64) {
        if let Some(p) = &self.player {
            p.voice.seek(p.duration.mul_f64(fraction.clamp(0.0, 1.0)));
        }
    }

    /// The player's timer: drop the player once its sound ended.
    fn tick(&mut self) {
        if self.player.as_ref().is_some_and(|p| !p.voice.is_playing()) {
            self.stop();
        }
    }
}

/// The menu bar, in the language shown.
fn menu_bar(s: &State) -> MenuBar<State> {
    let tr = &s.tr;
    let t = |k: &str| tr.t(k);
    let mode = s.settings.mode;
    let theme = s.settings.theme;
    let lang = s.settings.language.clone();
    let mut languages = Menu::new(t("menu.language")).item(
        MenuItem::new(t("lang.system"), |s: &mut State| s.set_language(None))
            .checked(lang.is_none()),
    );
    for (tag, name) in i18n::LANGUAGES {
        languages = languages.item(
            MenuItem::new(*name, move |s: &mut State| s.set_language(Some(tag)))
                .checked(lang.as_deref() == Some(*tag)),
        );
    }
    let themes = [
        ("theme.system", ThemeChoice::System),
        ("theme.light", ThemeChoice::Light),
        ("theme.dark", ThemeChoice::Dark),
    ]
    .into_iter()
    .fold(Menu::new(t("menu.theme")), |m, (key, choice)| {
        m.item(
            MenuItem::new(t(key), move |s: &mut State| s.set_theme(choice))
                .checked(theme == choice),
        )
    });
    let quit = s.handle.clone();
    let playing = s.player.is_some();
    MenuBar::new()
        .menu(
            Menu::new(t("menu.file"))
                .item(
                    MenuItem::new(t("menu.open"), |s: &mut State| s.pick_root())
                        .shortcut(Shortcut::cmd('o'))
                        .enabled(!cfg!(target_arch = "wasm32")),
                )
                .item(MenuItem::new(t("menu.sample"), |s: &mut State| {
                    s.set_root(sample_vfs())
                }))
                .item(
                    MenuItem::new(t("menu.refresh"), |s: &mut State| s.refresh())
                        .shortcut(Shortcut::key(KeyCode::F5)),
                )
                .separator()
                .item(
                    MenuItem::new(t("menu.quit"), move |_: &mut State| {
                        if let Some(h) = &quit {
                            h.exit();
                        }
                    })
                    .shortcut(Shortcut::cmd('q')),
                ),
        )
        .menu(
            Menu::new(t("menu.edit"))
                .item(
                    MenuItem::new(t("menu.copy_path"), |s: &mut State| s.copy_path())
                        .shortcut(Shortcut::cmd('c').shift()),
                )
                .item(
                    MenuItem::new(t("menu.select_all"), |s: &mut State| s.select_all())
                        .shortcut(Shortcut::cmd('a')),
                ),
        )
        .menu(
            Menu::new(t("menu.view"))
                .item(
                    MenuItem::new(t("menu.list"), |s: &mut State| s.set_mode(Mode::List))
                        .shortcut(Shortcut::cmd('1'))
                        .checked(mode == Mode::List),
                )
                .item(
                    MenuItem::new(t("menu.grid"), |s: &mut State| s.set_mode(Mode::Grid))
                        .shortcut(Shortcut::cmd('2'))
                        .checked(mode == Mode::Grid),
                )
                .separator()
                .submenu(themes)
                .submenu(languages),
        )
        .menu(
            Menu::new(t("menu.go")).item(
                MenuItem::new(t("menu.up"), |s: &mut State| s.go_up())
                    .shortcut(Shortcut::key(KeyCode::ArrowUp).alt())
                    .enabled(!s.cwd().is_empty()),
            ),
        )
        .menu(
            Menu::new(t("menu.playback"))
                .item(
                    MenuItem::new(t("play.toggle"), |s: &mut State| s.play_pause())
                        .shortcut(Shortcut::cmd('p')),
                )
                .item(MenuItem::new(t("play.stop"), |s: &mut State| s.stop()).enabled(playing)),
        )
}

/// The sample, in memory.
pub fn sample_vfs() -> Vfs {
    Vfs::Memory(Arc::new(vfs::MemFs::new(sample::files())))
}

/// The theme for the state: the system's (light or dark, native controls),
/// or a fixed one; mirrored for Arabic.
fn theme_for(s: &State, system: &Theme) -> Theme {
    let theme = match s.settings.theme {
        ThemeChoice::System => *system,
        ThemeChoice::Light => Theme::light(),
        ThemeChoice::Dark => Theme::dark(),
    };
    theme.layout_direction(LayoutDirection::for_language(s.tr.language))
}

/// The app around `state`: menus, theme, languages, window state.
pub fn build_app(state: State) -> App<State> {
    let (w, h, maximized) = (
        state.settings.width,
        state.settings.height,
        state.settings.maximized,
    );
    let title = state.tr.t("app.title");
    let loader = state.loader.clone();
    let app = App::new(state, view::view)
        .title(title)
        .app_info(
            stipple::platform::AppInfo::new("org.stipple.MediaBrowser")
                .name("Media Browser")
                .version(env!("CARGO_PKG_VERSION")),
        )
        .languages(i18n::shipped())
        .theme(Theme::native())
        .theme_with(theme_for)
        .menu(menu_bar)
        .logical_size(Size::new(w, h))
        .min_size(Size::new(640.0, 420.0))
        .maximized(maximized)
        .on_window_change(|s: &mut State, _, info| s.window_changed(info))
        .on_locale_change(|s: &mut State, _| {
            if s.settings.language.is_none() {
                s.tr = Rc::new(Tr::new(None));
            }
        });
    let handle = app.handle();
    {
        let h = handle.clone();
        loader.on_ready(move || h.update(|s: &mut State| s.loaded += 1));
    }
    if cfg!(target_arch = "wasm32") {
        let h = handle.clone();
        loader.on_schedule(move || {
            h.after(Duration::from_millis(1), |s: &mut State| s.loader.pump());
        });
    }
    // The state keeps a handle of its own, for the player's timer and Quit.
    let h = handle.clone();
    handle.update(move |s: &mut State| s.handle = Some(h));
    app
}

/// The state for a run: the folder given on the command line, else the one
/// last opened, else the sample (written to a temporary folder on desktops
/// so it can be dragged out and revealed; in memory on the web).
fn initial_state(args: &[String]) -> State {
    let settings = Settings::load(Settings::default_file());
    let given = args
        .iter()
        .skip(1)
        .enumerate()
        .find(|(i, a)| !a.starts_with("--") && args[*i] != "--select")
        .map(|(_, a)| a)
        .map(PathBuf::from);
    let vfs = if args.iter().any(|a| a == "--sample") {
        sample_on_disk().unwrap_or_else(sample_vfs)
    } else if let Some(dir) = given
        .or_else(|| settings.root.clone())
        .filter(|d| d.is_dir())
    {
        Vfs::Disk(dir)
    } else if cfg!(target_arch = "wasm32") {
        sample_vfs()
    } else {
        sample_on_disk().unwrap_or_else(sample_vfs)
    };
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4));
    let loader = Loader::new(vfs.clone(), workers);
    let mut state = State::new(vfs, loader, settings);
    // `--select Music/chime.wav`: start on that file (screenshots, screen
    // reader checks).
    if let Some(path) = args
        .iter()
        .position(|a| a == "--select")
        .and_then(|i| args.get(i + 1))
    {
        state.open_dir(vfs::parent(path).unwrap_or(""));
        if let Some(i) = state.entries().iter().position(|e| e.path == *path) {
            state.table.rows.select(i);
        }
    }
    state
}

/// The sample written to a temporary folder.
fn sample_on_disk() -> Option<Vfs> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dir = std::env::temp_dir().join("stipple-mediabrowser-sample");
        if !dir.join("README.md").exists() {
            sample::write_to(&dir).ok()?;
        }
        Some(Vfs::Disk(dir))
    }
    #[cfg(target_arch = "wasm32")]
    None
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(not(target_arch = "wasm32"))]
    if args.iter().any(|a| a == "--selftest") {
        std::process::exit(selftest::run(&args));
    }
    build_app(initial_state(&args)).run();
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
