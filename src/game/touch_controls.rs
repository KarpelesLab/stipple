//! Touch controls for phones and tablets: an on-screen stick and buttons
//! drawn over the game, played with a finger each, feeding the same
//! [`Actions`](super::Actions) as a gamepad.

use super::input::bit;
use super::{Frame, InputDevice};
use crate::core::{CanvasId, Element};
use crate::geometry::{Point, Rect, Size, Vec2};
use crate::platform::gamepad::{GamepadAxis, GamepadButton};
use crate::platform::{Touch, TouchPhase};
use crate::render::{Canvas, Font};
use crate::style::Theme;
use std::cell::RefCell;
use std::rc::Rc;

/// A corner of the window, which a control is placed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    /// The point `inset` in from this corner of `area` (`inset.dx` from the
    /// side, `inset.dy` from the top or bottom).
    pub fn place(self, area: Size, inset: Vec2) -> Point {
        let x = match self {
            Corner::TopLeft | Corner::BottomLeft => inset.dx,
            Corner::TopRight | Corner::BottomRight => area.width - inset.dx,
        };
        let y = match self {
            Corner::TopLeft | Corner::TopRight => inset.dy,
            Corner::BottomLeft | Corner::BottomRight => area.height - inset.dy,
        };
        Point::new(x, y)
    }

    fn is_left(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::BottomLeft)
    }
}

/// Which gamepad stick a [`TouchStick`] moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stick {
    /// [`LeftX`](GamepadAxis::LeftX) and [`LeftY`](GamepadAxis::LeftY).
    Left,
    /// [`RightX`](GamepadAxis::RightX) and [`RightY`](GamepadAxis::RightY).
    Right,
}

impl Stick {
    /// The stick's horizontal and vertical axes.
    pub fn axes(self) -> (GamepadAxis, GamepadAxis) {
        match self {
            Stick::Left => (GamepadAxis::LeftX, GamepadAxis::LeftY),
            Stick::Right => (GamepadAxis::RightX, GamepadAxis::RightY),
        }
    }
}

/// When the touch controls show.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ShowTouch {
    /// While touch is the [last device used](super::InputState::last_device):
    /// they hide on a key, a mouse click or a gamepad, and come back with
    /// the next touch. Before any input, they show on iOS and Android.
    #[default]
    Auto,
    /// Always.
    Always,
    /// Never: the controls are off and take no touches.
    Never,
}

/// An on-screen stick: a finger dragged from its center moves a gamepad
/// stick.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct TouchStick {
    /// The pad stick it moves.
    pub stick: Stick,
    /// Where it sits: its center is `inset` in from `corner`.
    pub corner: Corner,
    pub inset: Vec2,
    /// How far (logical pixels) the finger goes from the center for a full
    /// push.
    pub radius: f64,
    /// The radial dead zone, as a fraction of the radius.
    pub dead_zone: f32,
    /// Whether a finger landing anywhere in the stick's half of the window
    /// (left for a stick in a left corner) takes it, with the stick
    /// centered where it landed; otherwise the finger must land on it.
    pub floating: bool,
}

impl TouchStick {
    /// The left pad stick, bottom left, floating over the left half.
    pub fn left() -> TouchStick {
        TouchStick {
            stick: Stick::Left,
            corner: Corner::BottomLeft,
            inset: Vec2::new(100.0, 100.0),
            radius: 56.0,
            dead_zone: 0.15,
            floating: true,
        }
    }

    /// The right pad stick, bottom right, floating over the right half (a
    /// twin-stick shooter's aim).
    pub fn right() -> TouchStick {
        TouchStick {
            stick: Stick::Right,
            corner: Corner::BottomRight,
            ..TouchStick::left()
        }
    }

    /// Placed `inset` in from `corner`.
    pub fn at(mut self, corner: Corner, inset: Vec2) -> TouchStick {
        self.corner = corner;
        self.inset = inset;
        self
    }

    /// With a full push at `radius` logical pixels.
    pub fn radius(mut self, radius: f64) -> TouchStick {
        self.radius = radius.max(1.0);
        self
    }

    /// With a radial dead zone of `dead_zone` (a fraction of the radius).
    pub fn dead_zone(mut self, dead_zone: f32) -> TouchStick {
        self.dead_zone = dead_zone;
        self
    }

    /// Floating (see [`floating`](Self::floating)) or fixed in place.
    pub fn floating(mut self, floating: bool) -> TouchStick {
        self.floating = floating;
        self
    }

    /// Where its center is in `area`, when no finger moved it.
    pub fn home(&self, area: Size) -> Point {
        self.corner.place(area, self.inset)
    }

    /// Whether a finger landing at `p` takes it.
    fn takes(&self, p: Point, area: Size) -> bool {
        if self.floating {
            (p.x < area.width / 2.0) == self.corner.is_left()
        } else {
            p.distance_to(self.home(area)) <= self.radius * 1.5
        }
    }
}

/// An on-screen button: held while a finger that landed on it is down, as a
/// gamepad button.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct TouchButton {
    /// The pad button it holds. A trigger also pulls its axis all the way.
    pub button: GamepadButton,
    /// What it shows ("A", "Jump"), drawn with the overlay's font.
    pub label: String,
    /// Where it sits: its center is `inset` in from `corner`.
    pub corner: Corner,
    pub inset: Vec2,
    /// Its radius in logical pixels. A finger landing a little outside
    /// still takes it.
    pub radius: f64,
}

impl TouchButton {
    /// A button for `button`, labeled `label`, bottom right.
    pub fn new(button: GamepadButton, label: impl Into<String>) -> TouchButton {
        TouchButton {
            button,
            label: label.into(),
            corner: Corner::BottomRight,
            inset: Vec2::new(80.0, 100.0),
            radius: 36.0,
        }
    }

    /// Placed `inset` in from `corner`.
    pub fn at(mut self, corner: Corner, inset: Vec2) -> TouchButton {
        self.corner = corner;
        self.inset = inset;
        self
    }

    /// With radius `radius`.
    pub fn radius(mut self, radius: f64) -> TouchButton {
        self.radius = radius.max(1.0);
        self
    }

    /// Where its center is in `area`.
    pub fn center(&self, area: Size) -> Point {
        self.corner.place(area, self.inset)
    }
}

/// What a finger holds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Stick(usize),
    Button(usize),
}

/// A stick's finger: where its base and the finger are, and its value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct StickState {
    /// Where the stick is centered while a finger holds it.
    base: Option<Point>,
    at: Point,
    value: (f32, f32),
}

/// On-screen touch controls: sticks and buttons a finger each, acting as
/// one more gamepad.
///
/// Give them to the loop with
/// [`GameLoop::touch_controls`](super::GameLoop::touch_controls): the
/// [`InputState`](super::InputState) then routes every finger to them, and
/// their sticks and buttons read as a pad through the any-pad queries
/// (`gamepad_held(None, ..)`, `gamepad_axis(None, ..)`) and so through the
/// gamepad [`Binding`](super::Binding)s: bind `Pad:South` and the "A" touch
/// button does it too.
///
/// A finger belongs to the control it lands on, until it lifts: a button
/// (or a little outside it), else a free stick whose area it landed in.
/// Others go on being the game's own (`InputState::touches`). So the left
/// thumb steers on the left half while the right one holds a button.
///
/// They show while the player uses touch (see [`ShowTouch`]); draw them
/// with a [`TouchOverlay`] over the game.
///
/// ```
/// use stipple::game::{Corner, TouchButton, TouchControls, TouchStick};
/// use stipple::gamepad::GamepadButton;
/// use stipple::geometry::Vec2;
///
/// let controls = TouchControls::new()
///     .stick(TouchStick::left())
///     .button(TouchButton::new(GamepadButton::South, "A"))
///     .button(
///         TouchButton::new(GamepadButton::East, "B").at(Corner::BottomRight, Vec2::new(160.0, 60.0)),
///     );
/// # let _ = controls;
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct TouchControls {
    sticks: Vec<TouchStick>,
    buttons: Vec<TouchButton>,
    show: ShowTouch,
    shown: bool,
    area: Size,
    fingers: Vec<(u64, Target)>,
    stick_state: Vec<StickState>,
}

impl Default for TouchControls {
    fn default() -> Self {
        TouchControls::new()
    }
}

impl TouchControls {
    /// No controls, shown per [`ShowTouch::Auto`].
    pub fn new() -> TouchControls {
        TouchControls {
            sticks: Vec::new(),
            buttons: Vec::new(),
            show: ShowTouch::Auto,
            shown: cfg!(any(target_os = "ios", target_os = "android")),
            area: Size::new(0.0, 0.0),
            fingers: Vec::new(),
            stick_state: Vec::new(),
        }
    }

    /// With `stick` too.
    pub fn stick(mut self, stick: TouchStick) -> TouchControls {
        self.sticks.push(stick);
        self.stick_state.push(StickState::default());
        self
    }

    /// With `button` too.
    pub fn button(mut self, button: TouchButton) -> TouchControls {
        self.buttons.push(button);
        self
    }

    /// Shown as `show` says.
    pub fn show(mut self, show: ShowTouch) -> TouchControls {
        self.show = show;
        self.shown = match show {
            ShowTouch::Always => true,
            ShowTouch::Never => false,
            ShowTouch::Auto => self.shown,
        };
        self
    }

    /// The sticks, in the order added.
    pub fn sticks(&self) -> &[TouchStick] {
        &self.sticks
    }

    /// The buttons, in the order added.
    pub fn buttons(&self) -> &[TouchButton] {
        &self.buttons
    }

    /// Whether they show now.
    pub fn visible(&self) -> bool {
        self.shown
    }

    /// The area they are placed in (the window's logical size).
    pub fn area(&self) -> Size {
        self.area
    }

    /// Stick `i`'s value, `(x, y)` from −1 to 1 with +Y down, dead zone
    /// applied; `(0, 0)` without a finger.
    pub fn stick_value(&self, i: usize) -> (f32, f32) {
        self.stick_state.get(i).map_or((0.0, 0.0), |s| s.value)
    }

    /// Where stick `i` is drawn: its center and the knob (the finger, kept
    /// within the radius).
    pub fn stick_knob(&self, i: usize) -> Option<(Point, Point)> {
        let stick = self.sticks.get(i)?;
        let state = self.stick_state[i];
        let Some(base) = state.base else {
            let home = stick.home(self.area);
            return Some((home, home));
        };
        let d = state.at - base;
        let len = d.length();
        let d = if len > stick.radius {
            d * (stick.radius / len)
        } else {
            d
        };
        Some((base, base + d))
    }

    /// Whether a finger holds button `i`.
    pub fn button_held(&self, i: usize) -> bool {
        self.fingers.iter().any(|(_, t)| *t == Target::Button(i))
    }

    pub(crate) fn set_area(&mut self, area: Size) {
        self.area = area;
    }

    /// The player used `device`: show or hide.
    pub(crate) fn device_used(&mut self, device: InputDevice) {
        if self.show == ShowTouch::Auto {
            self.shown = device == InputDevice::Touch;
        }
    }

    /// Take a finger's touch.
    pub(crate) fn touch(&mut self, t: &Touch) {
        if self.show == ShowTouch::Never {
            return;
        }
        let p = t.position;
        let held = self.fingers.iter().position(|(id, _)| *id == t.id);
        match (t.phase, held) {
            (TouchPhase::Started, None) => {
                if let Some(target) = self.target_at(p) {
                    if let Target::Stick(i) = target {
                        let base = if self.sticks[i].floating {
                            p
                        } else {
                            self.sticks[i].home(self.area)
                        };
                        self.stick_state[i].base = Some(base);
                        self.move_stick(i, p);
                    }
                    self.fingers.push((t.id, target));
                }
            }
            (TouchPhase::Started | TouchPhase::Moved, Some(f)) => {
                if let Target::Stick(i) = self.fingers[f].1 {
                    self.move_stick(i, p);
                }
            }
            (TouchPhase::Ended | TouchPhase::Cancelled, Some(f)) => {
                if let (_, Target::Stick(i)) = self.fingers.remove(f) {
                    self.stick_state[i] = StickState::default();
                }
            }
            _ => {}
        }
    }

    /// The control a finger landing at `p` takes: the nearest button it
    /// lands on, else the first free stick whose area it's in.
    fn target_at(&self, p: Point) -> Option<Target> {
        let button = self
            .buttons
            .iter()
            .enumerate()
            .map(|(i, b)| (i, p.distance_to(b.center(self.area)) / (b.radius * 1.2)))
            .filter(|(_, d)| *d <= 1.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| Target::Button(i));
        button.or_else(|| {
            (0..self.sticks.len())
                .find(|i| {
                    !self.fingers.iter().any(|(_, t)| *t == Target::Stick(*i))
                        && self.sticks[*i].takes(p, self.area)
                })
                .map(Target::Stick)
        })
    }

    fn move_stick(&mut self, i: usize, at: Point) {
        let stick = &self.sticks[i];
        let state = &mut self.stick_state[i];
        state.at = at;
        if let Some(base) = state.base {
            state.value = stick_vector(at - base, stick.radius, stick.dead_zone);
        }
    }

    /// Let go of every finger.
    pub(crate) fn release_all(&mut self) {
        self.fingers.clear();
        for s in &mut self.stick_state {
            *s = StickState::default();
        }
    }

    /// The pad they make: the buttons held, and the axes.
    pub(crate) fn pad(&self) -> (u32, [f32; 6]) {
        let mut held = 0;
        let mut axes = [0.0f32; 6];
        let mut set = |axis: GamepadAxis, v: f32| {
            let a = &mut axes[axis as usize];
            if v.abs() > a.abs() {
                *a = v;
            }
        };
        for (_, target) in &self.fingers {
            if let Target::Button(i) = *target {
                let button = self.buttons[i].button;
                held |= bit(button);
                match button {
                    GamepadButton::LeftTrigger => set(GamepadAxis::LeftTrigger, 1.0),
                    GamepadButton::RightTrigger => set(GamepadAxis::RightTrigger, 1.0),
                    _ => {}
                }
            }
        }
        for (stick, state) in self.sticks.iter().zip(&self.stick_state) {
            let (x, y) = stick.stick.axes();
            set(x, state.value.0);
            set(y, state.value.1);
        }
        (held, axes)
    }

    /// Draw them (when [visible](Self::visible)) in a canvas covering the
    /// window, in `theme`'s colors; labels need `font`.
    pub fn paint(&self, c: &mut Canvas<'_>, theme: &Theme, font: Option<&Font>) {
        if !self.shown {
            return;
        }
        let p = &theme.palette;
        let disc = |at: Point, r: f64| Rect::from_xywh(at.x - r, at.y - r, r * 2.0, r * 2.0);
        for (i, stick) in self.sticks.iter().enumerate() {
            let Some((base, knob)) = self.stick_knob(i) else {
                continue;
            };
            let ring = disc(base, stick.radius);
            c.fill_round_rect(ring, stick.radius, p.surface.with_alpha(0x50));
            c.stroke_round_rect(ring, stick.radius, p.border.with_alpha(0xa0), 2.0);
            let r = stick.radius * 0.45;
            let active = self.stick_state[i].base.is_some();
            let color = if active { p.primary_active } else { p.primary };
            c.fill_round_rect(disc(knob, r), r, color.with_alpha(0xb0));
        }
        for (i, button) in self.buttons.iter().enumerate() {
            let at = button.center(self.area);
            let r = button.radius;
            let held = self.button_held(i);
            let color = if held { p.primary_active } else { p.primary };
            let rect = disc(at, r);
            c.fill_round_rect(rect, r, color.with_alpha(if held { 0xd8 } else { 0x80 }));
            c.stroke_round_rect(rect, r, p.on_primary.with_alpha(0x60), 1.5);
            if let Some(font) = font {
                let size = theme.typography.title.max(r * 0.6);
                let text = font.measure(&button.label, size);
                let origin = Point::new(at.x - text.width / 2.0, at.y - text.height / 2.0);
                c.fill_text(
                    &button.label,
                    origin,
                    &crate::render::TextPaint::new(font, size, p.on_primary),
                );
            }
        }
    }
}

/// A stick's value for a finger `d` from its center: `d` over `radius`,
/// clamped to length 1, with a radial dead zone (see
/// [`Actions::stick`](super::Actions::stick)).
pub(crate) fn stick_vector(d: Vec2, radius: f64, dead_zone: f32) -> (f32, f32) {
    let v = d / radius.max(1.0);
    super::actions::radial_dead_zone(v.dx as f32, v.dy as f32, dead_zone)
}

/// Draws [`TouchControls`] over the game: a canvas for the top of a
/// [`layers`](Element::layers) stack covering the window, repainted when
/// the controls change.
///
/// ```no_run
/// use stipple::core::CanvasId;
/// use stipple::game::{Frame, GameLoop, TouchButton, TouchControls, TouchOverlay, TouchStick};
/// use stipple::gamepad::GamepadButton;
/// use stipple::prelude::*;
///
/// struct Game { touch: TouchOverlay }
///
/// fn view(g: &Game, cx: &mut Cx<Game>) -> Element {
///     let theme = *cx.theme();
///     Element::layers(vec![
///         label(cx, "the game"),
///         g.touch.element(&theme), // over the rest
///     ])
/// }
///
/// let controls = TouchControls::new()
///     .stick(TouchStick::left())
///     .button(TouchButton::new(GamepadButton::South, "A"));
/// let game = Game { touch: TouchOverlay::new(CanvasId(2)).font(Font::system_default()) };
/// App::new(game, view)
///     .game_loop(
///         GameLoop::new(|g: &mut Game, frame: &mut Frame| {
///             g.touch.sync(frame); // repaints it when the controls changed
///         })
///         .touch_controls(controls),
///     )
///     .run();
/// ```
#[derive(Clone)]
pub struct TouchOverlay {
    id: CanvasId,
    shown: Rc<RefCell<Option<TouchControls>>>,
    font: Option<Font>,
}

impl std::fmt::Debug for TouchOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TouchOverlay")
            .field("id", &self.id)
            .field("shown", &self.shown)
            .field("font", &self.font.is_some())
            .finish()
    }
}

impl TouchOverlay {
    /// An overlay drawing into canvas `id`.
    pub fn new(id: CanvasId) -> TouchOverlay {
        TouchOverlay {
            id,
            shown: Rc::new(RefCell::new(None)),
            font: None,
        }
    }

    /// With `font` for the buttons' labels (without one they have none).
    pub fn font(mut self, font: Option<Font>) -> TouchOverlay {
        self.font = font;
        self
    }

    /// Its canvas.
    pub fn id(&self) -> CanvasId {
        self.id
    }

    /// From the update: take the controls' state from the frame's input,
    /// and repaint the canvas if it changed. Returns whether it did.
    pub fn sync(&self, frame: &mut Frame<'_>) -> bool {
        let now = frame.input().touch_controls();
        if self.shown.borrow().as_ref() == now {
            return false;
        }
        *self.shown.borrow_mut() = now.cloned();
        frame.repaint(self.id);
        true
    }

    /// The controls as last synced.
    pub fn controls(&self) -> Option<TouchControls> {
        self.shown.borrow().clone()
    }

    /// From the view: the canvas, in `theme`'s colors. Put it last in a
    /// [`layers`](Element::layers) stack that covers the window, so it's
    /// drawn over the game; it takes no pointer events itself.
    pub fn element(&self, theme: &Theme) -> Element {
        let shown = self.shown.clone();
        let font = self.font.clone();
        let theme = *theme;
        Element::canvas(self.id, move |c: &mut Canvas<'_>| {
            if let Some(controls) = &*shown.borrow() {
                controls.paint(c, &theme, font.as_ref());
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(id: u64, phase: TouchPhase, x: f64, y: f64) -> Touch {
        Touch {
            id,
            phase,
            position: Point::new(x, y),
        }
    }

    fn controls() -> TouchControls {
        let mut c = TouchControls::new()
            .stick(TouchStick::left().radius(50.0).dead_zone(0.2))
            .button(TouchButton::new(GamepadButton::South, "A"))
            .button(
                TouchButton::new(GamepadButton::RightTrigger, "R")
                    .at(Corner::TopRight, Vec2::new(50.0, 50.0)),
            );
        c.set_area(Size::new(800.0, 400.0));
        c
    }

    #[test]
    fn stick_math_dead_zone_and_clamp() {
        assert_eq!(stick_vector(Vec2::new(5.0, 0.0), 50.0, 0.2), (0.0, 0.0));
        let (x, y) = stick_vector(Vec2::new(30.0, 0.0), 50.0, 0.2);
        assert!(
            (x - 0.5).abs() < 1e-6 && y == 0.0,
            "rescaled past the dead zone"
        );
        let (x, y) = stick_vector(Vec2::new(300.0, 400.0), 50.0, 0.2);
        assert!(
            (x - 0.6).abs() < 1e-6 && (y - 0.8).abs() < 1e-6,
            "clamped to 1"
        );
        let (x, y) = stick_vector(Vec2::new(-20.0, 20.0), 50.0, 0.0);
        assert!((x + 0.4).abs() < 1e-6 && (y - 0.4).abs() < 1e-6);
    }

    #[test]
    fn placement_from_corners() {
        let area = Size::new(800.0, 400.0);
        assert_eq!(
            Corner::BottomRight.place(area, Vec2::new(80.0, 100.0)),
            Point::new(720.0, 300.0)
        );
        assert_eq!(
            Corner::TopLeft.place(area, Vec2::new(10.0, 20.0)),
            Point::new(10.0, 20.0)
        );
        let c = controls();
        assert_eq!(c.buttons()[1].center(c.area()), Point::new(750.0, 50.0));
    }

    #[test]
    fn a_finger_per_control() {
        let mut c = controls();
        // The left thumb lands on the left half: the stick floats there.
        c.touch(&touch(1, TouchPhase::Started, 150.0, 250.0));
        assert_eq!(c.stick_value(0), (0.0, 0.0));
        c.touch(&touch(1, TouchPhase::Moved, 150.0, 200.0));
        let (x, y) = c.stick_value(0);
        assert!(x == 0.0 && (y + 1.0).abs() < 1e-6, "pushed up: {y}");
        // The right thumb holds A (landing just outside its edge still does).
        c.touch(&touch(2, TouchPhase::Started, 720.0 + 40.0, 300.0));
        assert!(c.button_held(0) && !c.button_held(1));
        let (held, axes) = c.pad();
        assert_eq!(held, bit(GamepadButton::South));
        assert!((axes[GamepadAxis::LeftY as usize] + 1.0).abs() < 1e-6);
        // A third finger on the right half, on nothing, is the game's.
        c.touch(&touch(3, TouchPhase::Started, 500.0, 200.0));
        assert_eq!(c.fingers.len(), 2);
        // Another on the left half: the stick is taken.
        c.touch(&touch(4, TouchPhase::Started, 100.0, 100.0));
        assert_eq!(c.fingers.len(), 2);
        // The stick's finger sliding over the button doesn't press it.
        c.touch(&touch(1, TouchPhase::Moved, 720.0, 250.0));
        assert_eq!(c.pad().0, bit(GamepadButton::South));
        assert_eq!(c.stick_value(0).0, 1.0, "full right");
        let (base, knob) = c.stick_knob(0).unwrap();
        assert_eq!(base, Point::new(150.0, 250.0));
        assert!((knob.distance_to(base) - 50.0).abs() < 1e-9, "knob kept in");
        // Lifting lets go; a trigger button pulls its axis too.
        c.touch(&touch(1, TouchPhase::Ended, 720.0, 250.0));
        c.touch(&touch(2, TouchPhase::Cancelled, 760.0, 300.0));
        assert_eq!(c.pad(), (0, [0.0; 6]));
        c.touch(&touch(5, TouchPhase::Started, 750.0, 50.0));
        let (held, axes) = c.pad();
        assert_eq!(held, bit(GamepadButton::RightTrigger));
        assert_eq!(axes[GamepadAxis::RightTrigger as usize], 1.0);
        c.release_all();
        assert_eq!(c.pad(), (0, [0.0; 6]));
    }

    #[test]
    fn a_fixed_stick_needs_the_finger_on_it() {
        let mut c = TouchControls::new().stick(TouchStick::left().floating(false));
        c.set_area(Size::new(800.0, 400.0));
        // Home is (100, 300).
        c.touch(&touch(1, TouchPhase::Started, 300.0, 100.0));
        assert!(c.fingers.is_empty());
        c.touch(&touch(2, TouchPhase::Started, 120.0, 300.0));
        let (x, _) = c.stick_value(0);
        assert!(x > 0.0, "measured from home, not where it landed");
    }

    #[test]
    fn showing_follows_the_device_used() {
        let mut c = controls();
        c.device_used(InputDevice::Touch);
        assert!(c.visible());
        c.device_used(InputDevice::Gamepad);
        assert!(!c.visible());
        let mut always = controls().show(ShowTouch::Always);
        always.device_used(InputDevice::Keyboard);
        assert!(always.visible());
        let mut never = controls().show(ShowTouch::Never);
        never.device_used(InputDevice::Touch);
        never.touch(&touch(1, TouchPhase::Started, 720.0, 300.0));
        assert!(!never.visible() && !never.button_held(0));
    }
}
