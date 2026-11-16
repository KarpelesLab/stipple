//! The window: a toolbar with the breadcrumb, the folder tree, the files (a
//! table or a grid of thumbnails), the preview, and a status bar.

use stipple::core::pointer;
use stipple::prelude::*;
use stipple::widgets::{BlockKind, Marks};

use super::State;
use super::loader::{Job, Loaded, MdBlock, THUMB};
use super::settings::Mode;
use super::vfs::{self, Entry, Kind, Vfs};

const ROW_H: f64 = 26.0;
const TILE_W: f64 = 116.0;
const TILE_H: f64 = 132.0;
/// Room the toolbar and the status bar take.
const CHROME_H: f64 = 44.0 + 30.0;

pub fn view(state: &State, cx: &mut Cx<State>) -> Element {
    state.loader.begin_build();
    let theme = *cx.theme();
    let sp = theme.spacing;
    let win = cx.window_size();
    // The menu bar is drawn in the window where there's no system one.
    let menu_bar = if cfg!(any(target_os = "macos", target_os = "windows")) {
        0.0
    } else {
        32.0
    };
    let body_h = (win.height - menu_bar - CHROME_H - 2.0 * sp.sm).max(160.0);
    let right_w = (win.width - 2.0 * sp.sm) * (1.0 - state.outer.fraction()) - 8.0;
    let files_w = (right_w * state.inner.fraction() - 8.0).max(200.0);
    let preview_w = (right_w * (1.0 - state.inner.fraction()) - 8.0).max(160.0);

    let folders = folder_tree(state, cx, body_h);
    let files = match state.settings.mode {
        Mode::List => file_table(state, cx, files_w, body_h),
        Mode::Grid => file_grid(state, cx, files_w, body_h),
    };
    let files = files.on_drop_of(cx, DragOffer::FILES, |s: &mut State, data, _| {
        if let DropData::Files(paths) = data {
            // Not the folder's own files, dragged from the list onto it.
            let here = s.vfs.real_path(&s.cwd());
            let from_elsewhere: Vec<_> = paths
                .iter()
                .filter(|p| p.parent().map(|d| d.to_path_buf()) != here)
                .cloned()
                .collect();
            if !from_elsewhere.is_empty() {
                s.drop_files(&from_elsewhere);
            }
        }
    });
    let preview = preview_pane(state, cx, preview_w, body_h);
    let right = split_pane(cx, &state.inner, |s| &mut s.inner).panes(files, preview);
    let body = split_pane(cx, &state.outer, |s| &mut s.outer)
        .panes(folders, right)
        .grow(1.0);

    column(vec![toolbar_row(state, cx), body, status_bar(state, cx)])
        .gap(sp.sm)
        .padding(Insets::uniform(sp.sm))
        .grow(1.0)
}

/// Open, Up, Refresh, the breadcrumb, and the List / Grid switch.
fn toolbar_row(state: &State, cx: &mut Cx<State>) -> Element {
    let tr = state.tr.clone();
    let rtl = cx.theme().is_rtl();
    let mut items = Vec::new();
    if !cfg!(target_arch = "wasm32") {
        items.push(tool_button(cx, tr.t("tool.open")).on_tap(cx, |s: &mut State| s.pick_root()));
    }
    items.push(tool_button(cx, tr.t("tool.up")).on_tap(cx, |s: &mut State| s.go_up()));
    items.push(tool_button(cx, tr.t("tool.refresh")).on_tap(cx, |s: &mut State| s.refresh()));
    items.push(tool_separator(cx));

    // The breadcrumb: the root, then each folder down to the one shown.
    let cwd = state.cwd();
    let mut crumbs = vec![(String::new(), state.vfs.root_name())];
    let mut at = String::new();
    for part in cwd.split('/').filter(|p| !p.is_empty()) {
        at = vfs::join(&at, part);
        crumbs.push((at.clone(), part.to_string()));
    }
    let last = crumbs.len() - 1;
    let mut trail = Vec::new();
    for (i, (path, name)) in crumbs.into_iter().enumerate() {
        if i > 0 {
            let sep = if rtl { "‹" } else { "›" };
            trail.push(Element::text(
                sep,
                cx.theme().typography.body,
                cx.theme().palette.text_muted,
            ));
        }
        let b = ToolButton::new(name).selected(i == last).view(cx);
        trail.push(b.on_tap(cx, move |s: &mut State| s.open_dir(&path)));
    }
    items.push(
        row(trail)
            .gap(2.0)
            .align(Align::Start, Align::Center)
            .role(Role::Toolbar)
            .a11y_label(tr.t("tool.location"))
            .clip()
            .grow(1.0),
    );
    let modes = [tr.t("view.list"), tr.t("view.grid")];
    let labels: Vec<&str> = modes.iter().map(String::as_str).collect();
    let selected = usize::from(state.settings.mode == Mode::Grid);
    items.push(tabs(cx, &labels, selected, |s: &mut State, i| {
        s.set_mode(if i == 0 { Mode::List } else { Mode::Grid })
    }));
    toolbar(cx).children(items)
}

fn folder_tree(state: &State, cx: &mut Cx<State>, height: f64) -> Element {
    let root = state.vfs.root_name();
    tree(
        cx,
        &state.tree,
        |s| &mut s.tree,
        TreeSpec::new(&state.folders)
            .row_height(ROW_H)
            .height(height),
        |cx, row| {
            let path = state.folders.path(row.id);
            let name = if path.is_empty() {
                root.clone()
            } else {
                vfs::file_name(&path).to_string()
            };
            label(cx, name)
        },
    )
    .a11y_label(state.tr.t("pane.folders"))
}

/// A small colored mark for an entry's kind, with its extension.
fn kind_mark(theme: &Theme, e: &Entry, size: f64) -> Element {
    let color = match e.kind {
        Kind::Folder => Color::rgb(0xe0, 0xa8, 0x30),
        Kind::Image => Color::rgb(0x3c, 0x9a, 0x5f),
        Kind::Audio => Color::rgb(0x8a, 0x4f, 0xd0),
        Kind::Markdown => Color::rgb(0x2f, 0x7c, 0xd6),
        Kind::Text => Color::rgb(0x6b, 0x7a, 0x8c),
        Kind::Other => Color::rgb(0x90, 0x90, 0x90),
    };
    let mark = swatch(color, size, size / 4.0);
    if size < 20.0 {
        return mark;
    }
    let ext = if e.kind == Kind::Folder {
        String::new()
    } else {
        e.name
            .rsplit_once('.')
            .map(|(_, x)| x.to_uppercase())
            .unwrap_or_default()
    };
    Element::stack(
        Axis::Horizontal,
        vec![Element::text(ext, theme.typography.caption, Color::WHITE)],
    )
    .width(size)
    .height(size * 0.8)
    .radius(size / 8.0)
    .fill(color)
    .align(Align::Center, Align::Center)
}

/// What every item of the list and the grid does: a double click opens it,
/// it can be dragged out, and it has a context menu. (A click selects it:
/// the table's row does that, and a tile's own tap.)
fn item(state: &State, cx: &mut Cx<State>, el: Element, index: usize, e: &Entry) -> Element {
    let menu = context_menu(state, index, e);
    el.on_pointer(cx, move |ev: &PointerEvent, s: &mut State| {
        if ev.kind == PointerEventKind::Down
            && ev.button == Some(PointerButton::Left)
            && ev.click_count == 2
        {
            s.activate_later(index);
        }
    })
    .drag_source(cx, move |s: &mut State| s.drag_payload(index))
    .context_menu(cx, menu)
}

fn context_menu(state: &State, index: usize, e: &Entry) -> Menu<State> {
    let tr = &state.tr;
    let mut menu = Menu::new(e.name.clone())
        .item(MenuItem::new(tr.t("ctx.open"), move |s: &mut State| {
            s.pick(index);
            s.activate(index);
        }))
        .item(MenuItem::new(
            tr.t("ctx.copy_path"),
            move |s: &mut State| {
                s.pick(index);
                s.copy_path();
            },
        ));
    if matches!(state.vfs, Vfs::Disk(_)) {
        menu = menu.item(MenuItem::new(tr.t("ctx.reveal"), move |s: &mut State| {
            s.pick(index);
            s.reveal();
        }));
    }
    if e.kind == Kind::Audio {
        let path = e.path.clone();
        menu = menu
            .separator()
            .item(MenuItem::new(tr.t("ctx.play"), move |s: &mut State| {
                s.toggle_play(&path)
            }));
    }
    menu
}

fn file_table(state: &State, cx: &mut Cx<State>, width: f64, height: f64) -> Element {
    let tr = state.tr.clone();
    let theme = *cx.theme();
    let entries = state.entries();
    let fresh;
    let table_state = if state.selection_current() {
        &state.table
    } else {
        // A folder just picked in the tree: nothing selected in it yet.
        fresh = {
            let mut t = state.table.clone();
            t.rows = ListState::new(SelectionMode::Multi);
            t
        };
        &fresh
    };
    // The name takes what the others leave (less the scroll bar's gutter).
    let name_w = (width - 2.0 - 84.0 - 84.0 - 160.0 - 14.0).max(120.0);
    let columns = [
        Column::new(tr.t("col.name"), name_w)
            .sortable()
            .min_width(100.0),
        Column::new(tr.t("col.size"), 84.0)
            .sortable()
            .align(Align::End)
            .min_width(60.0),
        Column::new(tr.t("col.kind"), 84.0)
            .sortable()
            .min_width(60.0),
        Column::new(tr.t("col.modified"), 160.0)
            .sortable()
            .min_width(80.0),
    ];
    if entries.is_empty() {
        return column(vec![label(cx, tr.t("empty"))])
            .padding(Insets::uniform(theme.spacing.lg))
            .height(height)
            .role(Role::Group)
            .a11y_label(tr.t("pane.files"));
    }
    table(
        cx,
        table_state,
        State::table_mut,
        TableSpec::new(&columns, entries.len())
            .row_height(ROW_H)
            .height(height - ROW_H - 3.0)
            .on_key(|s: &mut State, key| s.list_key(key)),
        |cx, row, col| {
            let e = &entries[row];
            let el = match col {
                0 => row_with_mark(cx, &theme, e),
                1 if e.kind == Kind::Folder => label(cx, ""),
                1 => label(cx, tr.size(e.size)),
                2 => label(cx, tr.t(e.kind.key())),
                _ => label(cx, tr.date(e.modified)),
            };
            item(state, cx, el, row, e)
        },
    )
    .a11y_label(tr.t("pane.files"))
}

fn row_with_mark(cx: &mut Cx<State>, theme: &Theme, e: &Entry) -> Element {
    row(vec![kind_mark(theme, e, 10.0), label(cx, e.name.clone())])
        .gap(theme.spacing.sm)
        .align(Align::Start, Align::Center)
}

/// The files as a grid of thumbnails, built a screenful at a time like the
/// table: only the visible rows of tiles exist, and only their thumbnails are
/// asked for.
fn file_grid(state: &State, cx: &mut Cx<State>, width: f64, height: f64) -> Element {
    let theme = *cx.theme();
    let tr = state.tr.clone();
    let entries = state.entries();
    let n = entries.len();
    let cols = (((width - 16.0) / TILE_W).floor() as usize).max(1);
    let rows = n.div_ceil(cols);
    let id = cx.register_scroll();
    let max_offset = (rows as f64 * TILE_H - height).max(0.0);
    let offset = cx.scroll_offset(id).clamp(0.0, max_offset);
    let first = (offset / TILE_H).floor() as usize;
    let last = (((offset + height) / TILE_H).ceil() as usize + 1).min(rows);
    let current = state.selection_current();
    let rows_state = &state.table.rows;

    let mut children = vec![spacer().grow(0.0).height(first as f64 * TILE_H)];
    for r in first..last.max(first) {
        let mut tiles = Vec::new();
        for c in 0..cols {
            let i = r * cols + c;
            let Some(e) = entries.get(i) else { break };
            let selected = current && rows_state.is_selected(i);
            let cursor = current && rows_state.cursor() == Some(i);
            tiles.push(tile(state, cx, &theme, i, e, selected, cursor));
        }
        children.push(row(tiles).gap(4.0).height(TILE_H));
    }
    children.push(
        spacer()
            .grow(0.0)
            .height((rows - last.max(first)) as f64 * TILE_H),
    );
    let content = column(children).padding(Insets::uniform(4.0));
    let body = Element::stack(Axis::Vertical, vec![content])
        .height(height - 2.0)
        .scrollable(id);

    let requests = cx.scroll_requests();
    let rtl = theme.is_rtl();
    let mut frame = Element::stack(Axis::Vertical, vec![body])
        .fill(theme.palette.surface)
        .border(theme.palette.border, 1.0)
        .radius(theme.radius / 2.0)
        .padding(Insets::uniform(1.0))
        .clip()
        .role(Role::List)
        .a11y_label(tr.t("pane.files"));
    if n == 0 {
        frame = frame.child(label(cx, tr.t("empty")));
    }
    frame.on_key(cx, move |s: &mut State, key| {
        s.grid_key(key, cols, rtl);
        // Keep the cursor's row in view.
        if let Some(c) = s.table.rows.cursor() {
            let top = (c / cols) as f64 * TILE_H;
            if top < offset {
                requests.scroll_to(id, top);
            } else if top + TILE_H > offset + height - 8.0 {
                requests.scroll_to(id, top + TILE_H - height + 8.0);
            }
        }
    })
}

fn tile(
    state: &State,
    cx: &mut Cx<State>,
    theme: &Theme,
    index: usize,
    e: &Entry,
    selected: bool,
    cursor: bool,
) -> Element {
    let p = &theme.palette;
    let picture = match e.kind {
        Kind::Image => match state.loader.get(Job::Thumb, &e.path).as_deref() {
            Some(Loaded::Image { pixels, .. }) => Some(
                image(pixels)
                    .fit(ImageFit::Contain)
                    .width(f64::from(THUMB) * 0.9)
                    .height(76.0),
            ),
            _ => None,
        },
        _ => None,
    };
    let picture = picture.unwrap_or_else(|| kind_mark(theme, e, 48.0));
    let thumb = Element::stack(Axis::Horizontal, vec![picture])
        .width(TILE_W - 12.0)
        .height(84.0)
        .align(Align::Center, Align::Center);
    let name = clipped(Element::text(e.name.clone(), theme.typography.caption, p.text).wrap())
        .width(TILE_W - 16.0)
        .height(32.0)
        .align(Align::Center, Align::Start);
    let mut el = column(vec![thumb, name])
        .gap(4.0)
        .width(TILE_W - 4.0)
        .height(TILE_H - 4.0)
        .padding(Insets::uniform(4.0))
        .align(Align::Center, Align::Start)
        .radius(6.0)
        .role(Role::ListItem)
        .selected(selected)
        .a11y_label(e.name.clone())
        .on_tap(cx, move |s: &mut State| {
            s.table_mut().rows.click(index, pointer().modifiers)
        });
    if selected {
        el = el.fill(p.selection);
    }
    if cursor {
        el = el.border(p.focus_ring, 1.0);
    }
    item(state, cx, el, index, e)
}

fn preview_pane(state: &State, cx: &mut Cx<State>, width: f64, height: f64) -> Element {
    let tr = state.tr.clone();
    let theme = *cx.theme();
    let p = &theme.palette;
    let mut parts = Vec::new();
    let mut meta: Vec<(String, String)> = Vec::new();
    let target = state.current();
    match &target {
        None => {
            parts.push(label(cx, tr.t("preview.none")).wrap());
            let n = state.entries().len();
            meta.push((tr.t("meta.contains"), tr.plural("items", n)));
            meta.push((tr.t("meta.location"), state.vfs.display_path(&state.cwd())));
        }
        Some(e) => {
            parts.push(heading(cx, e.name.clone()).wrap());
            meta.push((tr.t("meta.kind"), tr.t(e.kind.key())));
            if e.kind == Kind::Folder {
                let n = state.folders.listing(&e.path).len();
                meta.push((tr.t("meta.contains"), tr.plural("items", n)));
            } else {
                meta.push((tr.t("meta.size"), tr.size(e.size)));
            }
            meta.push((tr.t("meta.modified"), tr.date(e.modified)));
            meta.push((tr.t("meta.location"), state.vfs.display_path(&e.path)));
            if matches!(
                e.kind,
                Kind::Image | Kind::Audio | Kind::Text | Kind::Markdown
            ) {
                match state.loader.get(Job::Preview, &e.path).as_deref() {
                    None => parts.push(label(cx, tr.t("preview.loading"))),
                    Some(Loaded::Failed(why)) => {
                        parts.push(label(cx, tr.t("preview.failed")));
                        parts.push(clipped(Element::text(
                            why.clone(),
                            theme.typography.caption,
                            p.text_muted,
                        )));
                    }
                    Some(loaded) => show(state, cx, e, loaded, width, &mut parts, &mut meta),
                }
            }
        }
    }
    let mut cells = Vec::new();
    for (k, v) in meta {
        cells.push(Element::text(k, theme.typography.caption, p.text_muted));
        // Long values (a path) are cut at the pane's edge; screen readers
        // still get all of it.
        cells.push(clipped(Element::text(v, theme.typography.caption, p.text)));
    }
    parts.push(divider(cx));
    parts.push(
        grid(vec![Track::Auto, Track::Fr(1.0)], cells)
            .gap(6.0)
            .width((width - 2.0 * theme.spacing.md).max(80.0)),
    );
    scroll(cx)
        .children(vec![
            column(parts)
                .gap(theme.spacing.md)
                .padding(Insets::uniform(theme.spacing.md))
                .width(width),
        ])
        .height(height)
        .role(Role::Group)
        .a11y_label(tr.t("pane.preview"))
}

/// `text` cut at its box's edge: a long name or path doesn't spill over its
/// neighbors.
fn clipped(text: Element) -> Element {
    Element::stack(Axis::Horizontal, vec![text]).clip()
}

/// The preview of a loaded file, and its own metadata.
fn show(
    state: &State,
    cx: &mut Cx<State>,
    e: &Entry,
    loaded: &Loaded,
    width: f64,
    parts: &mut Vec<Element>,
    meta: &mut Vec<(String, String)>,
) {
    let tr = state.tr.clone();
    let theme = *cx.theme();
    match loaded {
        Loaded::Image {
            pixels,
            width: w,
            height: h,
        } => {
            let shown = (width - 2.0 * theme.spacing.md)
                .min(f64::from(*w))
                .max(32.0);
            parts.push(image(pixels).width(shown).a11y_label(e.name.clone()));
            meta.push((
                tr.t("meta.dimensions"),
                tr.f(
                    "meta.dim",
                    &[
                        ("w", &tr.number((*w).into())),
                        ("h", &tr.number((*h).into())),
                    ],
                ),
            ));
        }
        Loaded::Sound(sound) => {
            parts.push(player(state, cx, e, sound.duration(), width));
            meta.push((tr.t("meta.duration"), tr.duration(sound.duration())));
            meta.push((tr.t("meta.channels"), tr.number(sound.channels().into())));
            meta.push((
                tr.t("meta.rate"),
                tr.f("meta.hz", &[("n", &tr.number(sound.rate().into()))]),
            ));
        }
        Loaded::Text { text, lines, words } => {
            parts.push(paragraph(cx, text.clone()));
            meta.push((tr.t("meta.lines"), tr.number(*lines as u64)));
            meta.push((tr.t("meta.words"), tr.number(*words as u64)));
        }
        Loaded::Markdown(blocks) => parts.push(markdown(cx, &theme, blocks)),
        Loaded::Failed(_) => {}
    }
}

/// Play / Pause, Stop, the position slider and the time.
fn player(
    state: &State,
    cx: &mut Cx<State>,
    e: &Entry,
    total: std::time::Duration,
    width: f64,
) -> Element {
    let tr = state.tr.clone();
    let theme = *cx.theme();
    let mine = state.player.as_ref().filter(|p| p.path == e.path);
    let at = mine.map_or(std::time::Duration::ZERO, |p| p.voice.position().min(total));
    let running = mine.is_some_and(|p| p.voice.is_playing() && !p.voice.is_paused());
    let path = e.path.clone();
    let toggle = button(
        cx,
        tr.t(if running {
            "player.pause"
        } else {
            "player.play"
        }),
    )
    .on_tap(cx, move |s: &mut State| s.toggle_play(&path));
    let stop = button(cx, tr.t("player.stop")).on_tap(cx, |s: &mut State| s.stop());
    let fraction = if total.is_zero() {
        0.0
    } else {
        at.as_secs_f64() / total.as_secs_f64()
    };
    let path = e.path.clone();
    let (at_s, total_s) = (tr.duration(at), tr.duration(total));
    let seek = slider(cx, fraction, move |s: &mut State, x| {
        if s.player.as_ref().is_none_or(|p| p.path != path) {
            s.toggle_play(&path);
        }
        s.seek(x);
    })
    .width((width - 2.0 * theme.spacing.md).clamp(120.0, 420.0))
    .a11y_label(tr.f("player.position", &[("at", &at_s), ("total", &total_s)]));
    let time = Element::text(
        format!("{at_s} / {total_s}"),
        theme.typography.caption,
        theme.palette.text_muted,
    );
    column(vec![
        row(vec![toggle, stop])
            .gap(theme.spacing.sm)
            .align(Align::Start, Align::Center),
        seek,
        time,
    ])
    .gap(theme.spacing.sm)
}

/// Markdown blocks as rich text: headings, lists, quotes, code, emphasis.
fn markdown(cx: &mut Cx<State>, theme: &Theme, blocks: &[MdBlock]) -> Element {
    let p = &theme.palette;
    let mut out = Vec::new();
    let mut number = 0;
    for (kind, runs) in blocks {
        let size = match kind {
            BlockKind::Heading(1) => theme.typography.heading,
            BlockKind::Heading(2) => theme.typography.title,
            BlockKind::Heading(_) => theme.typography.body + 2.0,
            _ => theme.typography.body,
        };
        number = if *kind == BlockKind::NumberedItem {
            number + 1
        } else {
            0
        };
        let mut text = RichText::new();
        match kind {
            BlockKind::BulletItem => text = text.push("•  "),
            BlockKind::NumberedItem => text = text.push(format!("{number}.  ")),
            _ => {}
        }
        for (run, marks) in runs {
            let mut style = SpanStyle::new().size(size);
            if matches!(kind, BlockKind::Heading(_)) || marks.contains(Marks::BOLD) {
                style = style.bold();
            }
            if marks.contains(Marks::ITALIC) || *kind == BlockKind::Quote {
                style = style.italic();
            }
            if marks.contains(Marks::UNDERLINE) {
                style = style.underline(UnderlineStyle::Single);
            }
            if marks.contains(Marks::STRIKETHROUGH) {
                style = style.strikethrough();
            }
            if marks.contains(Marks::CODE) || *kind == BlockKind::CodeBlock {
                style = style.color(p.primary);
            }
            if *kind == BlockKind::Quote {
                style = style.color(p.text_muted);
            }
            text = text.styled(run, style);
        }
        let mut el = paragraph(cx, text);
        if matches!(
            kind,
            BlockKind::Quote | BlockKind::BulletItem | BlockKind::NumberedItem
        ) {
            el = el.padding(Insets {
                left: theme.spacing.md,
                right: 0.0,
                top: 0.0,
                bottom: 0.0,
            });
        }
        out.push(el);
    }
    column(out).gap(theme.spacing.sm)
}

fn status_bar(state: &State, cx: &mut Cx<State>) -> Element {
    let tr = state.tr.clone();
    let theme = *cx.theme();
    let n = state.entries().len();
    let selected = if state.selection_current() {
        state.table.rows.selected_count()
    } else {
        0
    };
    let mut text = tr.f(
        "status",
        &[
            ("items", &tr.plural("items", n)),
            ("selected", &tr.plural("selected", selected)),
        ],
    );
    let queued = state.loader.stats().queued;
    if queued > 0 {
        text = format!("{text} · {}", tr.plural("status.loading", queued));
    }
    if let Some(msg) = &state.status {
        text = format!("{text} · {msg}");
    }
    row(vec![
        clipped(Element::text(
            text,
            theme.typography.caption,
            theme.palette.text_muted,
        ))
        .grow(1.0),
    ])
    .height(22.0)
    .padding(Insets::symmetric(theme.spacing.sm, 0.0))
    .align(Align::Start, Align::Center)
}
