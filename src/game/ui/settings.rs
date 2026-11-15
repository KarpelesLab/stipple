//! A settings screen's parts: [`setting_row`], [`volume_row`],
//! [`game_slider`], [`toggle_row`], [`choice_row`], [`fullscreen_row`],
//! and the controls screen's [`binding_row`] with [`RebindCapture`] and
//! [`binding_name`].

use std::time::Duration;

use crate::core::{Align, AnimProp, Axis, Cx, Element, KeyInput, Role, Transition};
use crate::game::{Actions, AxisDirection, Binding, InputState};
use crate::geometry::Insets;
use crate::platform::gamepad::{GamepadAxis, GamepadButton};
use crate::platform::{PhysicalKey, PointerButton, WindowRequest};
use crate::render::Color;

use super::menu::key_cap;

/// The width of a row's control column (sliders, switches, choices).
pub const CONTROL_WIDTH: f64 = 240.0;

/// A settings row: `label` on the left and, on the right, the control you
/// add with [`child`](Element::child), on a faint band:
/// `setting_row(cx, "Music").child(game_slider(cx, …))`. A
/// [`menu_list`](super::menu_list) (or any
/// [`spatial_nav`](Element::spatial_nav) column) of them is a settings
/// screen the arrows and a gamepad move through.
pub fn setting_row<S>(cx: &Cx<S>, label: impl Into<String>) -> Element {
    let theme = cx.theme();
    let p = &theme.palette;
    Element::stack(
        Axis::Horizontal,
        vec![
            Element::text(label, theme.typography.body, p.text),
            crate::widgets::spacer(),
        ],
    )
    .gap(theme.spacing.md)
    .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
    .fill(p.surface.mix(p.background, 0.4))
    .radius(theme.radius)
    .min_height(52.0)
}

/// A slider for a game's settings, `value` in 0..=1: it takes focus (the
/// arrows and the d-pad reach it), Left and Right move it by `step` (Home
/// and End to the ends), and a drag sets it. `on_change` gets the new
/// value. [`CONTROL_WIDTH`] wide; override with `.width(..)`.
pub fn game_slider<S>(
    cx: &mut Cx<S>,
    value: f32,
    step: f32,
    on_change: impl Fn(&mut S, f32) + Clone + 'static,
) -> Element {
    let theme = &crate::widgets::theme_of(cx);
    let p = &theme.palette;
    let v = value.clamp(0.0, 1.0);
    let f = f64::from(v);
    let state = Transition(theme.motion.state);
    let fill = Element::boxed(crate::core::BoxStyle {
        fill: Some(p.primary),
        radius: 4.0,
        border: None,
    })
    .grow(f)
    .transition_for(AnimProp::Fill, state);
    let knob = Element::boxed(crate::core::BoxStyle {
        fill: Some(p.text),
        radius: 9.0,
        border: Some((p.primary, 3.0)),
    })
    .width(18.0)
    .height(18.0);
    let rest = crate::widgets::spacer().grow(1.0 - f);
    let keys = on_change.clone();
    Element::stack(Axis::Horizontal, vec![fill, knob, rest])
        .fill(p.background.mix(p.surface, 0.3))
        .border(p.border, 1.0)
        .radius(9.0)
        .height(18.0)
        .width(CONTROL_WIDTH)
        .on_key(cx, move |s, k: &KeyInput| {
            let to = match k {
                KeyInput::Left => v - step,
                KeyInput::Right => v + step,
                KeyInput::Home => 0.0,
                KeyInput::End => 1.0,
                _ => return,
            };
            // Snap to the steps, so keys land on round values.
            let to = if step > 0.0 {
                (to / step).round() * step
            } else {
                to
            };
            keys(s, to.clamp(0.0, 1.0));
        })
        .on_drag(cx, move |s, x: f64| on_change(s, x.clamp(0.0, 1.0) as f32))
        .role(Role::Group)
}

/// A volume row: `label`, a [`game_slider`] in steps of 5%, and the value
/// as a percentage. `on_change` gets the new volume (0 to 1), for the
/// game's mixer.
pub fn volume_row<S>(
    cx: &mut Cx<S>,
    label: impl Into<String>,
    value: f32,
    on_change: impl Fn(&mut S, f32) + Clone + 'static,
) -> Element {
    let theme = &crate::widgets::theme_of(cx);
    let p = &theme.palette;
    let percent = (value.clamp(0.0, 1.0) * 100.0).round() as u32;
    let control = Element::stack(
        Axis::Horizontal,
        vec![
            game_slider(cx, value, 0.05, on_change),
            Element::text(format!("{percent}%"), theme.typography.body, p.text_muted).width(48.0),
        ],
    )
    .gap(theme.spacing.md);
    setting_row(cx, label).child(control)
}

/// An on/off row: `label` and a switch; a tap (or Enter, or South) on the
/// switch runs `on_toggle`.
pub fn toggle_row<S>(
    cx: &mut Cx<S>,
    label: impl Into<String>,
    on: bool,
    on_toggle: impl FnMut(&mut S) + 'static,
) -> Element {
    let theme = &crate::widgets::theme_of(cx);
    let label = label.into();
    let switch = crate::widgets::switch(cx, on, on_toggle).a11y_label(label.clone());
    let p = &theme.palette;
    let state = Element::text(
        if on { "On" } else { "Off" },
        theme.typography.body,
        p.text_muted,
    )
    .width(48.0);
    let control = Element::stack(Axis::Horizontal, vec![switch, state]).gap(theme.spacing.md);
    setting_row(cx, label).child(control)
}

/// A row choosing one of `options` (a pixel scale, a language, a
/// difficulty) the way games do, by cycling: `◂ Option ▸`. Left and Right
/// (the d-pad, the stick) step through them, wrapping; Enter and a tap on
/// the value step forward; the arrows can be tapped. `on_change` gets the
/// new index.
pub fn choice_row<S>(
    cx: &mut Cx<S>,
    label: impl Into<String>,
    options: &[&str],
    selected: usize,
    on_change: impl Fn(&mut S, usize) + Clone + 'static,
) -> Element {
    let theme = &crate::widgets::theme_of(cx);
    let label = label.into();
    let p = &theme.palette;
    let n = options.len().max(1);
    let at = selected.min(n - 1);
    let (prev, next) = ((at + n - 1) % n, (at + 1) % n);
    let arrow = |cx: &mut Cx<S>, glyph: &str, to: usize| {
        let f = on_change.clone();
        Element::text(glyph, theme.typography.title, p.primary)
            .bold()
            .padding(Insets::symmetric(theme.spacing.xs, 0.0))
            .on_tap(cx, move |s| f(s, to))
            .role(Role::Group)
    };
    let left = arrow(cx, "\u{2039}", prev);
    let right = arrow(cx, "\u{203A}", next);
    let value = Element::text(
        options.get(at).copied().unwrap_or("\u{2014}"),
        theme.typography.body,
        p.text,
    )
    .bold();
    let keys = on_change.clone();
    let control = Element::stack(
        Axis::Horizontal,
        vec![
            left,
            crate::widgets::spacer(),
            value,
            crate::widgets::spacer(),
            right,
        ],
    )
    .width(CONTROL_WIDTH)
    .padding(Insets::symmetric(theme.spacing.xs, 2.0))
    .radius(theme.radius)
    .on_key(cx, move |s, k: &KeyInput| match k {
        KeyInput::Left => keys(s, prev),
        KeyInput::Right | KeyInput::Enter => keys(s, next),
        KeyInput::Text(t) if t == " " => keys(s, next),
        _ => {}
    })
    .role(Role::ComboBox)
    .a11y_label(label.clone());
    setting_row(cx, label).child(control)
}

/// A fullscreen row: a [`toggle_row`] showing whether the window is
/// fullscreen and switching it through the
/// [window API](crate::core::AppHandle::window). `on_change` gets the state
/// it asked for (`true` for fullscreen), to keep in the game's settings and
/// restore at the next start (with [`App::fullscreen`](crate::App::fullscreen)).
pub fn fullscreen_row<S: 'static>(
    cx: &mut Cx<S>,
    label: impl Into<String>,
    mut on_change: impl FnMut(&mut S, bool) + 'static,
) -> Element {
    let on = cx.window_info().fullscreen;
    let handle = cx.handle();
    toggle_row(cx, label, on, move |s| {
        handle.window(WindowRequest::Fullscreen(!on));
        on_change(s, !on);
    })
}

/// A short name for `binding`, as a key cap shows it: `W`, `Space`, `←`,
/// `L-Shift`, `Left click`, `Q / E` for a key axis, `Pad A` (see
/// [`pad_button_name`]), `L-Stick ←` for half a stick, `Pad LT` for a
/// trigger. Kinds of [`Binding`] added later show their text form without
/// its prefix.
pub fn binding_name(binding: &Binding) -> String {
    match binding {
        Binding::Key(k) => key_name(*k),
        Binding::MouseButton(b) => match b {
            PointerButton::Left => "Left click".into(),
            PointerButton::Right => "Right click".into(),
            PointerButton::Middle => "Middle click".into(),
            PointerButton::Back => "Mouse back".into(),
            PointerButton::Forward => "Mouse fwd".into(),
            PointerButton::Other(n) => format!("Mouse {n}"),
        },
        Binding::KeyAxis { neg, pos } => format!("{} / {}", key_name(*neg), key_name(*pos)),
        Binding::GamepadButton(b) => pad_button_name(*b).into(),
        Binding::GamepadAxis {
            axis, direction, ..
        } => pad_axis_name(*axis, *direction),
        // Whatever else joins `Binding`.
        #[allow(unreachable_patterns)]
        other => {
            let text = other.to_string();
            match text.split_once(':') {
                Some((_, name)) => name.to_string(),
                None => text,
            }
        }
    }
}

/// A pad button's short name, Xbox style (the layout most pads label):
/// `Pad A`, `Pad LB`, `Pad Start`, `Pad \u{2191}` for the d-pad.
pub fn pad_button_name(button: GamepadButton) -> &'static str {
    use GamepadButton as B;
    match button {
        B::South => "Pad A",
        B::East => "Pad B",
        B::West => "Pad X",
        B::North => "Pad Y",
        B::LeftShoulder => "Pad LB",
        B::RightShoulder => "Pad RB",
        B::LeftTrigger => "Pad LT",
        B::RightTrigger => "Pad RT",
        B::Select => "Pad Select",
        B::Start => "Pad Start",
        B::Guide => "Pad Home",
        B::LeftStick => "Pad L3",
        B::RightStick => "Pad R3",
        B::DPadUp => "Pad \u{2191}",
        B::DPadDown => "Pad \u{2193}",
        B::DPadLeft => "Pad \u{2190}",
        B::DPadRight => "Pad \u{2192}",
    }
}

/// A pad axis's short name: `L-Stick \u{2190}` for a half stick axis,
/// `L-Stick \u{2194}` for a whole one, `Pad LT` for a trigger.
fn pad_axis_name(axis: GamepadAxis, direction: AxisDirection) -> String {
    use GamepadAxis as A;
    let (stick, horizontal) = match axis {
        A::LeftX => ("L-Stick", true),
        A::LeftY => ("L-Stick", false),
        A::RightX => ("R-Stick", true),
        A::RightY => ("R-Stick", false),
        A::LeftTrigger => return "Pad LT".into(),
        A::RightTrigger => return "Pad RT".into(),
    };
    let arrow = match (direction, horizontal) {
        (AxisDirection::Both, true) => '\u{2194}',
        (AxisDirection::Both, false) => '\u{2195}',
        (AxisDirection::Negative, true) => '\u{2190}',
        (AxisDirection::Positive, true) => '\u{2192}',
        (AxisDirection::Negative, false) => '\u{2191}',
        (AxisDirection::Positive, false) => '\u{2193}',
    };
    format!("{stick} {arrow}")
}

/// A key's short name: the letter or digit for those keys, an arrow for
/// the arrows, else its W3C name tidied up (`ShiftLeft` → `L-Shift`).
pub fn key_name(key: PhysicalKey) -> String {
    let Some(name) = key.name() else {
        return format!("#{:x}", key.0);
    };
    if let Some(c) = name.strip_prefix("Key") {
        return c.to_string();
    }
    if let Some(d) = name.strip_prefix("Digit") {
        return d.to_string();
    }
    if let Some(n) = name.strip_prefix("Numpad") {
        return format!("Num {n}");
    }
    match name {
        "ArrowLeft" => return "\u{2190}".into(),
        "ArrowRight" => return "\u{2192}".into(),
        "ArrowUp" => return "\u{2191}".into(),
        "ArrowDown" => return "\u{2193}".into(),
        "Escape" => return "Esc".into(),
        "Backspace" => return "Bksp".into(),
        "ControlLeft" => return "L-Ctrl".into(),
        "ControlRight" => return "R-Ctrl".into(),
        _ => {}
    }
    for (side, short) in [("Left", "L-"), ("Right", "R-")] {
        if let Some(base) = name.strip_suffix(side) {
            return format!("{short}{base}");
        }
    }
    name.to_string()
}

/// What a controls screen's [`binding_row`] shows besides the bindings.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BindingRowState {
    /// The row waits for an input ("Press a key…").
    pub capturing: bool,
    /// The other actions that share one of this action's bindings, named:
    /// flagged on the row.
    pub conflicts: Vec<String>,
}

impl BindingRowState {
    /// Waiting for an input, or not.
    pub fn capturing(mut self, capturing: bool) -> Self {
        self.capturing = capturing;
        self
    }

    /// Sharing a binding with these actions (their names).
    pub fn conflicts(mut self, conflicts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.conflicts = conflicts.into_iter().map(Into::into).collect();
        self
    }
}

/// A controls screen's row: the action's `label`, its `bindings` as key
/// caps (see [`binding_name`]), a flag naming the actions it shares a
/// binding with, and, while it waits for one, "Press a key…". The row is a
/// button: a tap, Enter or South runs `on_activate`, which starts a
/// [`RebindCapture`] for the action.
pub fn binding_row<S>(
    cx: &mut Cx<S>,
    label: impl Into<String>,
    bindings: &[Binding],
    state: &BindingRowState,
    on_activate: impl FnMut(&mut S) + 'static,
) -> Element {
    let theme = &crate::widgets::theme_of(cx);
    let label = label.into();
    let p = &theme.palette;
    let mut right: Vec<Element> = Vec::new();
    if state.capturing {
        right.push(
            Element::text(
                "Press a key\u{2026}  (Esc cancels)",
                theme.typography.body,
                p.primary,
            )
            .bold()
            .appear(crate::core::Effect::fade()),
        );
    } else {
        if !state.conflicts.is_empty() {
            right.push(
                Element::text(
                    format!("\u{26A0} also {}", state.conflicts.join(", ")),
                    theme.typography.caption,
                    p.warning,
                )
                .padding(Insets::symmetric(theme.spacing.xs, 2.0))
                .border(p.warning, 1.0)
                .radius(theme.radius),
            );
        }
        if bindings.is_empty() {
            right.push(Element::text(
                "\u{2014}",
                theme.typography.body,
                p.text_muted,
            ));
        }
        for b in bindings {
            right.push(key_cap(cx, &binding_name(b)));
        }
    }
    let caps = Element::stack(Axis::Horizontal, right)
        .gap(theme.spacing.xs)
        .align(Align::End, Align::Center);
    let border = if state.capturing {
        p.primary
    } else if state.conflicts.is_empty() {
        Color::TRANSPARENT
    } else {
        p.warning
    };
    setting_row(cx, label.clone())
        .child(caps)
        .border(border, 2.0)
        .transition_for(AnimProp::Border, Transition(theme.motion.state))
        .on_tap(cx, on_activate)
        .role(Role::Button)
        .a11y_label(label)
}

/// What a [`RebindCapture`] saw.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Capture {
    /// Nothing yet.
    Waiting,
    /// Escape was pressed, or it waited too long: keep the old binding.
    Cancelled,
    /// The input to bind.
    Bound(Binding),
}

/// A "press a key…" in progress: which action (and which of its bindings,
/// the `slot`) takes the next input.
///
/// Start one when a [`binding_row`] is activated, then each frame of the
/// game loop hand it the input ([`poll`](Self::poll), which takes keys,
/// mouse buttons and whatever else [`Binding::capture`] reports, gamepad
/// buttons included where `Binding` has them), or hand it raw key events
/// from an [`on_raw_key`](crate::core::Element::on_raw_key) handler
/// ([`key_event`](Self::key_event)). Escape cancels, and so does waiting
/// longer than [`timeout`](Self::timeout) (for a player with only a
/// gamepad). Then [`apply`](Self::apply) the binding.
///
/// While one runs, keep the game loop running (it is how the input
/// arrives: `paused_when(|s| s.paused && s.capture.is_none())`) and gamepad
/// navigation off (`gamepad_navigation_when(|s| s.capture.is_none())`), so
/// the pad's South button is bound rather than pressing the row again.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RebindCapture<A> {
    /// The action being bound.
    pub action: A,
    /// Which of its bindings is replaced (one past the end adds one).
    pub slot: usize,
    /// When it started, on the frame clock: `None` until the first poll.
    started: Option<Duration>,
    timeout: Duration,
    /// [`by_device`](Self::by_device): the slot follows the input's device.
    by_device: bool,
}

impl<A: Copy + Eq + std::hash::Hash> RebindCapture<A> {
    /// How long a capture waits by default: 8 seconds.
    pub const TIMEOUT: Duration = Duration::from_secs(8);

    /// Wait for an input for `action`'s binding `slot`.
    pub fn new(action: A, slot: usize) -> Self {
        Self {
            action,
            slot,
            started: None,
            timeout: Self::TIMEOUT,
            by_device: false,
        }
    }

    /// Wait for an input for `action`, replacing its first binding from the
    /// same device: a key or a mouse button replaces its first keyboard or
    /// mouse binding, a pad button or stick its first gamepad one (one is
    /// added if it has none). One row then rebinds the keyboard and the pad
    /// alike, without a pad button pushing out the key. `slot` is ignored.
    pub fn by_device(action: A) -> Self {
        Self {
            by_device: true,
            ..Self::new(action, 0)
        }
    }

    /// Give up after `timeout` instead.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Look at a frame's `input` (at frame time `now`): what was pressed
    /// since the last update. The first poll only starts the clock and
    /// lets that frame's presses go (the press that started the capture
    /// would be one).
    pub fn poll(&mut self, input: &InputState, now: Duration) -> Capture {
        let Some(started) = self.started else {
            self.started = Some(now);
            return Capture::Waiting;
        };
        match Binding::capture(input) {
            Some(Binding::Key(PhysicalKey::ESCAPE)) => Capture::Cancelled,
            Some(b) => Capture::Bound(b),
            None if now.saturating_sub(started) >= self.timeout => Capture::Cancelled,
            None => Capture::Waiting,
        }
    }

    /// Look at a raw key event: Escape cancels, another key press binds.
    pub fn key_event(&self, ev: &crate::platform::KeyEvent) -> Capture {
        match Binding::from_key_event(ev) {
            Some(Binding::Key(PhysicalKey::ESCAPE)) => Capture::Cancelled,
            Some(b) => Capture::Bound(b),
            None => Capture::Waiting,
        }
    }

    /// Bind `binding` to the action in its slot, and return the other
    /// actions it is bound to as well (the conflicts to flag; it isn't
    /// taken from them). Binding it again where it already is changes
    /// nothing.
    pub fn apply(&self, actions: &mut Actions<A>, binding: Binding) -> Vec<A> {
        let has = actions
            .bindings(self.action)
            .iter()
            .position(|b| *b == binding);
        if has.is_none() {
            let list = actions.bindings(self.action);
            let slot = if self.by_device {
                list.iter()
                    .position(|b| b.is_gamepad() == binding.is_gamepad())
                    .unwrap_or(list.len())
            } else {
                self.slot
            };
            actions.rebind(self.action, slot, binding);
        }
        conflicts(actions, self.action)
    }
}

/// The other actions that share one of `action`'s bindings (what its
/// [`binding_row`] flags).
pub fn conflicts<A: Copy + Eq + std::hash::Hash>(actions: &Actions<A>, action: A) -> Vec<A> {
    let mut out: Vec<A> = Vec::new();
    for b in actions.bindings(action) {
        for other in actions.bound_to(*b) {
            if other != action && !out.contains(&other) {
                out.push(other);
            }
        }
    }
    out
}
