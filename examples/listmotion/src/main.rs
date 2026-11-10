//! An animated list: items fade and slide in when added, fade and shrink out
//! when removed (drawn until they're gone), and glide to their new places
//! when the list is shuffled, reversed or sorted, or when the tips panel
//! opens above it.
//!
//! None of it is hand-animated: each row declares `.enter`, `.exit` and
//! `.animate_position`, and the view just builds the list as it is now.
//!
//! - `--demo` slows the motion down and, 1.5 s in, removes, adds and moves
//!   rows on its own (for CI to screenshot it mid-flight).
//! - `--selftest` drives the app off-screen: it adds, removes and shuffles
//!   rows through clicks, checks that frames come only while something moves,
//!   that a row being removed is gone from the accessibility tree at once, and
//!   that reduced motion snaps; then prints `listmotion: selftest ok`.

use stipple::anim::{self, SpringParams};
use stipple::prelude::*;

const NAMES: [&str; 12] = [
    "Apricot", "Basil", "Cherry", "Damson", "Elder", "Fig", "Guava", "Hazel", "Iris", "Juniper",
    "Kiwi", "Lime",
];

#[derive(Clone, Debug, PartialEq)]
struct Item {
    id: u32,
    name: &'static str,
}

struct State {
    items: Vec<Item>,
    next: u32,
    /// The shuffle's pseudo-random state.
    seed: u64,
    tips: bool,
    /// How much slower than usual everything moves (`--demo`).
    slow: f64,
}

impl State {
    fn new() -> Self {
        let mut s = Self {
            items: Vec::new(),
            next: 0,
            seed: 0x2545_f491_4f6c_dd1d,
            tips: false,
            slow: 1.0,
        };
        for _ in 0..5 {
            s.add();
        }
        s
    }

    fn add(&mut self) {
        let name = NAMES[self.next as usize % NAMES.len()];
        // Insert near the top, so the rows below have somewhere to go.
        let at = self.items.len().min(1);
        self.items.insert(
            at,
            Item {
                id: self.next,
                name,
            },
        );
        self.next += 1;
    }

    fn remove(&mut self, id: u32) {
        self.items.retain(|i| i.id != id);
    }

    fn shuffle(&mut self) {
        // Fisher–Yates on an xorshift.
        for i in (1..self.items.len()).rev() {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            let j = (self.seed % (i as u64 + 1)) as usize;
            self.items.swap(i, j);
        }
    }

    fn sort(&mut self) {
        self.items.sort_by_key(|i| i.name);
    }

    /// What `--demo` does on its own: a removal, an addition, and the last
    /// row moved to the top, at once.
    fn scramble(&mut self) {
        if let Some(id) = self.items.get(1).map(|i| i.id) {
            self.remove(id);
        }
        self.add();
        self.items.rotate_right(1);
    }

    fn secs(&self, s: f64) -> Duration {
        Duration::from_secs_f64(s * self.slow)
    }
}

fn row_of(s: &State, item: &Item, cx: &mut Cx<State>, theme: &Theme) -> Element {
    let p = theme.palette;
    let hue = [
        Color::rgb(0xe5, 0x6b, 0x6f),
        Color::rgb(0xea, 0xac, 0x8b),
        Color::rgb(0x6d, 0x9d, 0xc5),
        Color::rgb(0x7b, 0xc0, 0x8f),
        Color::rgb(0xb5, 0x8d, 0xd6),
    ][item.id as usize % 5];
    let swatch = Element::boxed(BoxStyle::default())
        .width(14.0)
        .height(14.0)
        .radius(7.0)
        .fill(hue);
    let id = item.id;
    let remove = button_labeled(theme, "×")
        .a11y_label(format!("Remove {}", item.name))
        .on_tap(cx, move |s: &mut State| s.remove(id));
    let name = label(theme, item.name).grow(1.0);
    // In: fades in, sliding from the left. Out: fades while shrinking a
    // little. Moved: glides on a spring. All slowed down by `--demo`.
    let ease = Transition::ease(s.secs(0.25));
    let glide = Transition::spring(SpringParams::with_response(0.45 * s.slow, 0.85));
    row(vec![swatch, name, remove])
        .gap(theme.spacing.md)
        .align(Align::Start, Align::Center)
        .padding(Insets::new(6.0, 12.0, 6.0, 12.0))
        .fill(p.surface.mix(hue, 0.18))
        .radius(theme.radius)
        .key(id)
        .enter(
            Effect::fade()
                .and(Effect::slide(-40.0, 0.0))
                .transition(ease),
        )
        .exit(
            Effect::fade()
                .and(Effect::scale(0.8))
                .transition(Transition::ease(s.secs(0.3))),
        )
        .animate_position(glide)
}

fn view(s: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let p = theme.palette;
    let button = |cx: &mut Cx<State>, text: &str, f: fn(&mut State)| {
        button_labeled(&theme, text).on_tap(cx, f)
    };
    let controls = row(vec![
        button(cx, "Add", State::add),
        button(cx, "Shuffle", State::shuffle),
        button(cx, "Reverse", |s| s.items.reverse()),
        button(cx, "Sort", State::sort),
        button(cx, if s.tips { "Hide tips" } else { "Tips" }, |s| {
            s.tips = !s.tips
        }),
    ])
    .gap(theme.spacing.sm)
    .wrap();

    let mut body = vec![heading(&theme, "Groceries"), controls];
    if s.tips {
        // Opening pushes the list down; the rows glide there.
        body.push(
            label(&theme, "Rows fade in, fade out and glide; × removes one.")
                .padding(Insets::uniform(12.0))
                .fill(p.surface.mix(p.primary, 0.12))
                .radius(theme.radius)
                .key("tips")
                .appear(Effect::fade().and(Effect::slide(0.0, -12.0))),
        );
    }
    let rows = s.items.iter().map(|i| row_of(s, i, cx, &theme)).collect();
    body.push(column(rows).gap(6.0).align(Align::Start, Align::Stretch));
    column(body)
        .gap(theme.spacing.md)
        .padding(Insets::uniform(24.0))
        .align(Align::Start, Align::Stretch)
}

fn app(state: State) -> App<State> {
    let mut app = App::new(state, view)
        .title("Stipple List Motion")
        .theme(Theme::dark())
        .logical_size(Size::new(420.0, 560.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app
}

/// The accessibility tree's node named `name`, if any.
fn find(app: &App<State>, name: &str) -> Option<Rect> {
    let tree = app.accessibility_tree()?;
    tree.descendants()
        .into_iter()
        .find(|n| n.name == name)
        .map(|n| n.bounds)
}

fn press(app: &mut App<State>, name: &str) -> Result<(), String> {
    let b = find(app, name).ok_or(format!("no {name:?} to press"))?;
    let at = Point::new(b.min_x() + b.width() / 2.0, b.min_y() + b.height() / 2.0);
    if app.click_at(at) {
        Ok(())
    } else {
        Err(format!("{name:?} didn't take the click"))
    }
}

/// Render frames until nothing moves; how many it took.
fn settle(app: &mut App<State>) -> Result<usize, String> {
    let start = std::time::Instant::now();
    let mut frames = 0;
    while app.animating() {
        if start.elapsed() > Duration::from_secs(5) {
            return Err("still moving after 5 s".into());
        }
        std::thread::sleep(Duration::from_millis(16));
        let _ = app.render_frame();
        frames += 1;
    }
    Ok(frames)
}

fn selftest() -> Result<(), String> {
    let ensure = |ok: bool, what: &str| if ok { Ok(()) } else { Err(what.to_string()) };
    let mut app = app(State::new());
    let _ = app.render_frame();
    ensure(!app.animating(), "the first frame shows the list as it is")?;
    ensure(app.state().items.len() == 5, "five rows")?;

    // Adding: the new row fades in, the rows below it glide down.
    press(&mut app, "Add")?;
    let _ = app.render_frame();
    ensure(app.animating(), "an added row animates in")?;
    ensure(find(&app, "Fig").is_some(), "the new row is there at once")?;
    let frames = settle(&mut app)?;
    ensure(frames > 3, "it took a few frames")?;
    let still = app.render_frame().1;
    ensure(still.is_empty(), "at rest: nothing redrawn")?;

    // Removing: the row leaves the accessibility tree at once, while it's
    // still drawn fading out.
    let before = app.render_frame().0;
    press(&mut app, "Remove Cherry")?;
    let (mid, _) = app.render_frame();
    ensure(app.animating(), "a removed row animates out")?;
    ensure(find(&app, "Cherry").is_none(), "gone for screen readers")?;
    ensure(
        app.state().items.iter().all(|i| i.name != "Cherry"),
        "gone from the state",
    )?;
    ensure(mid != before, "the frame changed")?;
    settle(&mut app)?;

    // Reordering glides; then it rests.
    press(&mut app, "Shuffle")?;
    let _ = app.render_frame();
    ensure(app.animating(), "a shuffle animates")?;
    settle(&mut app)?;
    press(&mut app, "Tips")?;
    let _ = app.render_frame();
    ensure(app.animating(), "opening the tips moves the rows")?;
    settle(&mut app)?;

    // Under reduced motion it all snaps.
    anim::set_reduced_motion(Some(true));
    press(&mut app, "Sort")?;
    let _ = app.render_frame();
    ensure(!app.animating(), "reduced motion: sorting snaps")?;
    press(&mut app, "Remove Apricot")?;
    let _ = app.render_frame();
    ensure(!app.animating(), "reduced motion: removing snaps")?;
    anim::set_reduced_motion(None);
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--selftest") {
        match selftest() {
            Ok(()) => println!("listmotion: selftest ok"),
            Err(e) => {
                println!("listmotion: selftest FAILED: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let mut state = State::new();
    let demo = args.iter().any(|a| a == "--demo");
    if demo {
        state.slow = 6.0;
    }
    let app = app(state);
    if demo {
        app.handle()
            .after(Duration::from_millis(1500), State::scramble);
    }
    app.run();
}
