//! Motion with `stipple::anim`: a field that shakes on invalid input, a
//! pulsing drop target, a staggered list entrance, and an interruptible spring.
//!
//! Every animation here is a pure function of time. Handlers record *when*
//! something happened (`anim::now()`), or retarget an [`Animated`] value; the
//! view samples at the frame time with `Cx::play` / `Cx::animate`, which ask
//! for frames only while something moves.
//!
//! `--selftest` checks the same animations at fixed times, builds one frame
//! off-screen, and prints `motiondemo: selftest ok` (or exits non-zero).

use stipple::anim::{self, Animated, Animation, Easing, Keyframes, SpringParams};
use stipple::prelude::*;

/// How far the field swings, in logical pixels.
const SHAKE: f64 = 10.0;
const ITEMS: [&str; 6] = ["Inbox", "Drafts", "Sent", "Archive", "Spam", "Trash"];

struct State {
    code: EditBuffer,
    /// When the last invalid code was submitted.
    invalid_at: Option<Duration>,
    /// When the pulse started, while it's on.
    pulse_from: Option<Duration>,
    /// When the list last (re)entered.
    list_at: Duration,
    knob_on: bool,
    /// The switch knob's position, 0 to 1, on a spring.
    knob: Animated<f64>,
}

impl State {
    fn new() -> Self {
        Self {
            code: EditBuffer::new(),
            invalid_at: None,
            pulse_from: None,
            list_at: Duration::ZERO,
            knob_on: false,
            knob: Animated::new(0.0),
        }
    }

    fn submit(&mut self, now: Duration) {
        let text = self.code.text();
        let valid = text.len() == 4 && text.chars().all(|c| c.is_ascii_digit());
        self.invalid_at = (!valid).then_some(now);
    }

    fn toggle_knob(&mut self, now: Duration) {
        self.knob_on = !self.knob_on;
        // Clicking again mid-flight turns the knob round smoothly.
        let to = if self.knob_on { 1.0 } else { 0.0 };
        self.knob.spring_to(to, SpringParams::BOUNCY, now);
    }
}

/// The field's shake: a horizontal offset.
fn shake() -> Keyframes<f64> {
    anim::shake(SHAKE, 0.42)
}

/// The field's border after an invalid submit: red at once, fading back.
fn invalid_border(theme: &Theme) -> impl Animation<Value = Color> + use<> {
    Keyframes::new(theme.palette.danger)
        .hold(0.6)
        .to(theme.palette.border, 0.5, Easing::EaseOut)
}

/// The drop target's glow, 0 to 1 and back every 1.6 s, forever.
fn pulse() -> impl Animation<Value = f64> {
    anim::pulse(0.0, 1.0, 1.6)
}

/// List item `i`'s entrance progress, 0 to 1, each 60 ms after the last.
fn entrance(i: usize) -> impl Animation<Value = f64> {
    anim::entrance(i, 0.06, 0.35)
}

fn view(s: &State, cx: &mut Cx<State>) -> Element {
    let theme = *cx.theme();
    let p = theme.palette;

    // Shake on invalid input.
    let (offset, border) = match s.invalid_at {
        Some(at) => (cx.play(&shake(), at), cx.play(&invalid_border(&theme), at)),
        None => (0.0, p.border),
    };
    let field = text_editor(cx, &theme, &s.code, |s: &mut State| &mut s.code)
        .width(220.0)
        .height(40.0);
    let field = row(vec![field])
        .border(border, 2.0)
        .radius(theme.radius)
        // Shift by `offset` within a margin as wide as the swing.
        .padding(Insets::new(0.0, SHAKE - offset, 0.0, SHAKE + offset));
    let check = button_labeled(&theme, "Check").on_tap(cx, |s: &mut State| s.submit(anim::now()));
    let shaker = panel(
        &theme,
        vec![
            label(&theme, "Enter a 4-digit code:"),
            row(vec![field, check]).gap(theme.spacing.md),
        ],
    );

    // A pulsing drop target.
    let glow = match s.pulse_from {
        Some(at) => cx.play(&pulse(), at),
        None => 0.0,
    };
    let target = column(vec![label(&theme, "Drop here")])
        .align(Align::Center, Align::Center)
        .height(70.0)
        .fill(p.surface.mix(p.primary, 0.1 + 0.25 * glow))
        .border(p.surface.mix(p.primary, 0.4 + 0.6 * glow), 1.0 + 2.0 * glow)
        .radius(theme.radius);
    let toggle = button_labeled(
        &theme,
        if s.pulse_from.is_some() {
            "Stop"
        } else {
            "Pulse"
        },
    )
    .on_tap(cx, |s: &mut State| {
        s.pulse_from = match s.pulse_from {
            Some(_) => None,
            None => Some(anim::now()),
        }
    });
    let pulser = panel(
        &theme,
        vec![row(vec![target.grow(1.0), toggle]).gap(theme.spacing.md)],
    );

    // A staggered list.
    let items = ITEMS
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let t = cx.play(&entrance(i), s.list_at);
            let alpha = (t * 255.0).round() as u8;
            Element::text(*name, theme.typography.body, p.text.with_alpha(alpha))
                .padding(Insets::new(6.0, 12.0, 6.0, 12.0 + 32.0 * (1.0 - t)))
                .fill(p.surface.mix(p.primary, 0.15).with_alpha(alpha))
                .radius(theme.radius)
        })
        .collect();
    let replay =
        button_labeled(&theme, "Replay").on_tap(cx, |s: &mut State| s.list_at = anim::now());
    let list = panel(&theme, vec![replay, column(items).gap(4.0)]);

    // An interruptible spring: click twice quickly and it turns round.
    let knob_x = cx.animate(&s.knob);
    let knob = Element::boxed(BoxStyle::default())
        .width(28.0)
        .height(28.0)
        .fill(p.primary)
        .radius(14.0);
    let track = row(vec![knob])
        .padding(Insets::new(
            4.0,
            4.0 + 120.0 * (1.0 - knob_x),
            4.0,
            4.0 + 120.0 * knob_x,
        ))
        .fill(p.surface.mix(p.primary, 0.2 * knob_x))
        .border(p.border, 1.0)
        .radius(18.0)
        .on_tap(cx, |s: &mut State| s.toggle_knob(anim::now()));
    let spring = panel(
        &theme,
        vec![label(&theme, "Click the track (twice, quickly):"), track],
    );

    column(vec![
        heading(&theme, "Motion"),
        shaker,
        pulser,
        list,
        spring,
    ])
    .gap(theme.spacing.md)
    .padding(Insets::uniform(24.0))
    .align(Align::Start, Align::Stretch)
}

/// Check the animations at fixed times.
fn selftest() -> Result<(), String> {
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    let ensure = |ok: bool, what: &str| if ok { Ok(()) } else { Err(what.to_string()) };

    // The shake swings out to +SHAKE first, back past -SHAKE, and settles.
    let s = shake();
    ensure(s.sample(0.0) == 0.0, "shake starts at rest")?;
    ensure(close(s.sample(0.06), SHAKE), "shake swings right")?;
    ensure(close(s.sample(0.12), -SHAKE), "shake swings left")?;
    ensure(
        s.sample(0.42) == 0.0 && s.is_finished(0.42),
        "shake settles",
    )?;

    // The border is red, then fades back to the theme's.
    let theme = Theme::dark();
    let b = invalid_border(&theme);
    ensure(b.sample(0.3) == theme.palette.danger, "border turns red")?;
    ensure(b.sample(1.1) == theme.palette.border, "border recovers")?;

    // The pulse breathes forever.
    let p = pulse();
    ensure(p.sample(0.0) == 0.0 && p.sample(0.8) == 1.0, "pulse peaks")?;
    ensure(
        close(p.sample(1.6), 0.0) && !p.is_finished(1e6),
        "pulse loops",
    )?;

    // Items enter 60 ms apart.
    ensure(entrance(3).sample(0.18) == 0.0, "item 3 waits")?;
    ensure(entrance(3).sample(0.25) > 0.0, "item 3 enters")?;
    ensure(entrance(3).sample(0.53) == 1.0, "item 3 lands")?;
    ensure(
        (0..ITEMS.len()).all(|i| entrance(i).is_finished(0.35 + 0.06 * 5.0)),
        "all items land",
    )?;

    // Retargeting the knob mid-flight keeps its position and speed.
    let ms = Duration::from_millis;
    let mut st = State::new();
    st.toggle_knob(ms(0));
    let (x, v) = (st.knob.value(ms(120)), st.knob.velocity(ms(120)));
    st.toggle_knob(ms(120));
    ensure(st.knob.value(ms(120)) == x, "knob keeps its place")?;
    ensure(
        (st.knob.velocity(ms(120)) - v).abs() < 1e-9,
        "knob keeps its speed",
    )?;
    ensure(st.knob.value(ms(130)) > x, "knob carries on before turning")?;
    ensure(st.knob.value(ms(5000)) == 0.0, "knob settles back")?;

    // The view builds and renders.
    let mut app = App::new(State::new(), view).logical_size(Size::new(480.0, 640.0));
    let frame = app.render_once();
    ensure(frame.size().width > 0, "a frame renders")?;
    ensure(app.animating(), "the list entrance asks for frames")?;
    Ok(())
}

fn main() {
    if std::env::args().any(|a| a == "--selftest") {
        match selftest() {
            Ok(()) => println!("motiondemo: selftest ok"),
            Err(e) => {
                println!("motiondemo: selftest FAILED: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let mut app = App::new(State::new(), view)
        .title("Stipple Motion")
        .theme(Theme::dark())
        .logical_size(Size::new(480.0, 640.0));
    if let Some(font) = Font::system_default() {
        app = app.font(font);
    }
    app.run();
}
