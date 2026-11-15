//! Actions: what the game means ("jump", "move left") bound to the inputs
//! that do it, so the game asks about the action and the player picks the
//! keys.

use super::InputState;
use crate::platform::{PhysicalKey, PointerButton};
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;

/// One input an action is bound to.
///
/// More kinds (gamepad buttons and axes) join as they become available, so
/// match with a `_` arm.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Binding {
    /// A key, by its position on the keyboard (WASD stays where it is on
    /// AZERTY).
    Key(PhysicalKey),
    /// A mouse button.
    MouseButton(PointerButton),
    /// Two keys as an axis: `neg` gives -1, `pos` +1, both or neither 0.
    /// Held while either is down.
    KeyAxis { neg: PhysicalKey, pos: PhysicalKey },
}

impl Binding {
    /// Whether the input is down (an axis: either key).
    pub fn held(&self, input: &InputState) -> bool {
        match *self {
            Binding::Key(k) => input.key_held(k),
            Binding::MouseButton(b) => input.button_held(b),
            Binding::KeyAxis { neg, pos } => input.key_held(neg) || input.key_held(pos),
        }
    }

    /// Whether the input went down since the last update.
    pub fn pressed(&self, input: &InputState) -> bool {
        match *self {
            Binding::Key(k) => input.key_pressed(k),
            Binding::MouseButton(b) => input.button_pressed(b),
            Binding::KeyAxis { neg, pos } => input.key_pressed(neg) || input.key_pressed(pos),
        }
    }

    /// Whether the input came up since the last update.
    pub fn released(&self, input: &InputState) -> bool {
        match *self {
            Binding::Key(k) => input.key_released(k),
            Binding::MouseButton(b) => input.button_released(b),
            Binding::KeyAxis { neg, pos } => input.key_released(neg) || input.key_released(pos),
        }
    }

    /// The input's value, from -1 to 1: 1 while a key or button is held, an
    /// axis's direction.
    pub fn value(&self, input: &InputState) -> f32 {
        match *self {
            Binding::KeyAxis { neg, pos } => {
                f32::from(u8::from(input.key_held(pos))) - f32::from(u8::from(input.key_held(neg)))
            }
            _ => f32::from(u8::from(self.held(input))),
        }
    }

    /// The first key or mouse button pressed since the last update, as a
    /// binding: what a "press a key…" row in a settings screen waits for.
    pub fn capture(input: &InputState) -> Option<Binding> {
        input
            .keys_pressed()
            .next()
            .map(Binding::Key)
            .or_else(|| input.buttons_pressed().next().map(Binding::MouseButton))
    }

    /// The binding a raw key press gives (for a settings screen that takes
    /// keys with [`on_raw_key`](crate::core::Element::on_raw_key)): `None`
    /// for releases, repeats and unidentified keys.
    pub fn from_key_event(ev: &crate::platform::KeyEvent) -> Option<Binding> {
        (ev.is_press() && !ev.repeat && ev.physical.is_identified())
            .then_some(Binding::Key(ev.physical))
    }
}

/// The text form, as [`Actions::to_text`] writes it: `Key:KeyW`,
/// `Mouse:Left`, `KeyAxis:KeyA/KeyD`. Keys go by their W3C `code` name, or
/// `#` and the HID usage in hex when they have none.
impl fmt::Display for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn key(f: &mut fmt::Formatter<'_>, k: PhysicalKey) -> fmt::Result {
            match k.name() {
                Some(name) => f.write_str(name),
                None => write!(f, "#{:x}", k.0),
            }
        }
        match *self {
            Binding::Key(k) => {
                f.write_str("Key:")?;
                key(f, k)
            }
            Binding::MouseButton(b) => match b {
                PointerButton::Left => f.write_str("Mouse:Left"),
                PointerButton::Right => f.write_str("Mouse:Right"),
                PointerButton::Middle => f.write_str("Mouse:Middle"),
                PointerButton::Back => f.write_str("Mouse:Back"),
                PointerButton::Forward => f.write_str("Mouse:Forward"),
                PointerButton::Other(n) => write!(f, "Mouse:{n}"),
            },
            Binding::KeyAxis { neg, pos } => {
                f.write_str("KeyAxis:")?;
                key(f, neg)?;
                f.write_str("/")?;
                key(f, pos)
            }
        }
    }
}

/// A binding or a bindings file that couldn't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseBindingError {
    /// The line it was on (1-based; 0 for a single binding).
    pub line: usize,
    /// What was wrong.
    pub message: String,
}

impl fmt::Display for ParseBindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            f.write_str(&self.message)
        } else {
            write!(f, "line {}: {}", self.line, self.message)
        }
    }
}

impl std::error::Error for ParseBindingError {}

impl std::str::FromStr for Binding {
    type Err = ParseBindingError;

    fn from_str(s: &str) -> Result<Binding, ParseBindingError> {
        let err = |message: String| ParseBindingError { line: 0, message };
        let key = |name: &str| -> Result<PhysicalKey, ParseBindingError> {
            let name = name.trim();
            if let Some(hex) = name.strip_prefix('#') {
                return u16::from_str_radix(hex, 16)
                    .map(PhysicalKey)
                    .map_err(|_| err(format!("bad key usage `{name}`")));
            }
            PhysicalKey::from_name(name).ok_or_else(|| err(format!("unknown key `{name}`")))
        };
        let (kind, rest) = s
            .trim()
            .split_once(':')
            .ok_or_else(|| err(format!("`{}` is not `Kind:input`", s.trim())))?;
        match kind.trim() {
            "Key" => Ok(Binding::Key(key(rest)?)),
            "KeyAxis" => {
                let (neg, pos) = rest
                    .split_once('/')
                    .ok_or_else(|| err(format!("`{rest}` is not `neg/pos`")))?;
                Ok(Binding::KeyAxis {
                    neg: key(neg)?,
                    pos: key(pos)?,
                })
            }
            "Mouse" => Ok(Binding::MouseButton(match rest.trim() {
                "Left" => PointerButton::Left,
                "Right" => PointerButton::Right,
                "Middle" => PointerButton::Middle,
                "Back" => PointerButton::Back,
                "Forward" => PointerButton::Forward,
                n => PointerButton::Other(
                    n.parse()
                        .map_err(|_| err(format!("unknown mouse button `{n}`")))?,
                ),
            })),
            other => Err(err(format!("unknown input kind `{other}`"))),
        }
    }
}

/// An action's state as of the last [`Actions::update`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ActionState {
    held: bool,
    pressed: bool,
    released: bool,
    value: f32,
}

/// The game's actions (`A`, usually a small `enum`) and the inputs bound to
/// each.
///
/// Call [`update`](Self::update) once per game update with the frame's
/// [`InputState`], then ask about actions: [`pressed`](Self::pressed),
/// [`held`](Self::held), [`released`](Self::released) and
/// [`value`](Self::value) (or [`axis2`](Self::axis2) for a direction).
///
/// ```
/// use stipple::game::{Actions, Binding, InputState};
/// use stipple::platform::{ButtonState, KeyCode, KeyEvent, PhysicalKey};
///
/// #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
/// enum Act { Jump, Steer }
///
/// let mut actions = Actions::new()
///     .with(Act::Jump, Binding::Key(PhysicalKey::SPACE))
///     .with(Act::Steer, Binding::KeyAxis { neg: PhysicalKey::KEY_A, pos: PhysicalKey::KEY_D });
///
/// let mut input = InputState::new();
/// input.key(&KeyEvent::new(ButtonState::Pressed, KeyCode::Char('d'), PhysicalKey::KEY_D));
/// actions.update(&input);
/// assert_eq!(actions.value(Act::Steer), 1.0);
/// assert!(!actions.held(Act::Jump));
///
/// // Saved and read back, e.g. to a settings file.
/// let text = actions.to_text(|a| format!("{a:?}"));
/// assert_eq!(text, "Jump = Key:Space\nSteer = KeyAxis:KeyA/KeyD\n");
/// ```
#[derive(Clone, Debug)]
pub struct Actions<A> {
    bindings: Vec<(A, Vec<Binding>)>,
    state: HashMap<A, ActionState>,
}

impl<A> Default for Actions<A> {
    fn default() -> Self {
        Actions {
            bindings: Vec::new(),
            state: HashMap::new(),
        }
    }
}

impl<A: Copy + Eq + Hash> Actions<A> {
    /// No actions.
    pub fn new() -> Actions<A> {
        Actions::default()
    }

    /// Add `binding` to `action` (builder form of [`bind`](Self::bind)).
    pub fn with(mut self, action: A, binding: Binding) -> Actions<A> {
        self.bind(action, binding);
        self
    }

    /// Add `binding` to `action`'s bindings (if it isn't there already).
    pub fn bind(&mut self, action: A, binding: Binding) {
        let list = self.entry(action);
        if !list.contains(&binding) {
            list.push(binding);
        }
    }

    fn entry(&mut self, action: A) -> &mut Vec<Binding> {
        let i = match self.bindings.iter().position(|(a, _)| *a == action) {
            Some(i) => i,
            None => {
                self.bindings.push((action, Vec::new()));
                self.bindings.len() - 1
            }
        };
        &mut self.bindings[i].1
    }

    /// The bindings of `action`, in order.
    pub fn bindings(&self, action: A) -> &[Binding] {
        self.bindings
            .iter()
            .find(|(a, _)| *a == action)
            .map_or(&[], |(_, b)| b.as_slice())
    }

    /// Every action with its bindings, in the order they were first bound:
    /// the rows of a controls screen.
    pub fn iter(&self) -> impl Iterator<Item = (A, &[Binding])> {
        self.bindings.iter().map(|(a, b)| (*a, b.as_slice()))
    }

    /// Replace all of `action`'s bindings.
    pub fn set_bindings(&mut self, action: A, bindings: Vec<Binding>) {
        *self.entry(action) = bindings;
    }

    /// Put `binding` in slot `slot` of `action`'s bindings (a controls
    /// screen's "primary" and "secondary" columns), or at the end if there
    /// are fewer.
    pub fn rebind(&mut self, action: A, slot: usize, binding: Binding) {
        let list = self.entry(action);
        match list.get_mut(slot) {
            Some(b) => *b = binding,
            None => list.push(binding),
        }
    }

    /// Remove `binding` from every action: before giving it to another, so
    /// one key doesn't do two things.
    pub fn unbind(&mut self, binding: Binding) {
        for (_, list) in &mut self.bindings {
            list.retain(|b| *b != binding);
        }
    }

    /// The actions `binding` is bound to.
    pub fn bound_to(&self, binding: Binding) -> Vec<A> {
        self.bindings
            .iter()
            .filter(|(_, list)| list.contains(&binding))
            .map(|(a, _)| *a)
            .collect()
    }

    /// Read every action from `input`: call once per update, before asking.
    ///
    /// An action is held while any of its bindings is. It is *pressed* on the
    /// update it starts being held, and *released* on the one it stops; a
    /// press and release between two updates gives both on the same update.
    pub fn update(&mut self, input: &InputState) {
        for (action, list) in &self.bindings {
            let was = self.state.get(action).copied().unwrap_or_default();
            let held = list.iter().any(|b| b.held(input));
            let tapped = list.iter().any(|b| b.pressed(input));
            let value = list.iter().map(|b| b.value(input)).sum::<f32>();
            self.state.insert(
                *action,
                ActionState {
                    held,
                    pressed: !was.held && (held || tapped),
                    released: (was.held && !held) || (!was.held && !held && tapped),
                    value: value.clamp(-1.0, 1.0),
                },
            );
        }
    }

    fn get(&self, action: A) -> ActionState {
        self.state.get(&action).copied().unwrap_or_default()
    }

    /// Whether `action` started this update.
    pub fn pressed(&self, action: A) -> bool {
        self.get(action).pressed
    }

    /// Whether `action` is held.
    pub fn held(&self, action: A) -> bool {
        self.get(action).held
    }

    /// Whether `action` stopped this update.
    pub fn released(&self, action: A) -> bool {
        self.get(action).released
    }

    /// `action`'s value from -1 to 1: its bindings' values added up (1 for a
    /// held key or button, an axis's direction), clamped.
    pub fn value(&self, action: A) -> f32 {
        self.get(action).value
    }

    /// A direction from four actions, `(x, y)` with `y` down as on screen,
    /// no longer than 1 (a diagonal isn't faster).
    pub fn axis2(&self, left: A, right: A, up: A, down: A) -> (f32, f32) {
        let x = self.value(right) - self.value(left);
        let y = self.value(down) - self.value(up);
        let len = (x * x + y * y).sqrt();
        if len > 1.0 {
            (x / len, y / len)
        } else {
            (x, y)
        }
    }

    /// The bindings as text, one action per line: `name = Key:KeyW,
    /// Mouse:Left`, `name` from `name_of`. Read back with
    /// [`load_text`](Self::load_text).
    pub fn to_text<N: fmt::Display>(&self, name_of: impl Fn(A) -> N) -> String {
        let mut out = String::new();
        for (action, list) in &self.bindings {
            let list: Vec<String> = list.iter().map(Binding::to_string).collect();
            out.push_str(&format!("{} = {}\n", name_of(*action), list.join(", ")));
        }
        out
    }

    /// Read bindings written by [`to_text`](Self::to_text), replacing those
    /// of each action listed (`action_of` maps a name to its action). Actions
    /// not listed keep theirs, so saved bindings go over the defaults; names
    /// `action_of` doesn't know are skipped, as are blank lines and `#`
    /// comments. Nothing changes if any line is malformed.
    pub fn load_text(
        &mut self,
        text: &str,
        action_of: impl Fn(&str) -> Option<A>,
    ) -> Result<(), ParseBindingError> {
        let mut read = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let at = |e: ParseBindingError| ParseBindingError {
                line: i + 1,
                message: e.message,
            };
            let (name, list) = line.split_once('=').ok_or_else(|| ParseBindingError {
                line: i + 1,
                message: format!("`{line}` is not `name = bindings`"),
            })?;
            let bindings = list
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.parse::<Binding>().map_err(at))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(action) = action_of(name.trim()) {
                read.push((action, bindings));
            }
        }
        for (action, bindings) in read {
            self.set_bindings(action, bindings);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{ButtonState, KeyCode, KeyEvent};

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    enum Act {
        Left,
        Right,
        Up,
        Down,
        Fire,
        Steer,
    }

    fn name(a: Act) -> String {
        format!("{a:?}")
    }

    fn parse(s: &str) -> Option<Act> {
        [
            Act::Left,
            Act::Right,
            Act::Up,
            Act::Down,
            Act::Fire,
            Act::Steer,
        ]
        .into_iter()
        .find(|a| name(*a) == s)
    }

    fn key(i: &mut InputState, k: PhysicalKey, down: bool) {
        let state = if down {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        i.key(&KeyEvent::new(state, KeyCode::Unidentified(0), k));
    }

    fn actions() -> Actions<Act> {
        Actions::new()
            .with(Act::Left, Binding::Key(PhysicalKey::ARROW_LEFT))
            .with(Act::Left, Binding::Key(PhysicalKey::KEY_A))
            .with(Act::Right, Binding::Key(PhysicalKey::ARROW_RIGHT))
            .with(Act::Up, Binding::Key(PhysicalKey::ARROW_UP))
            .with(Act::Down, Binding::Key(PhysicalKey::ARROW_DOWN))
            .with(Act::Fire, Binding::MouseButton(PointerButton::Left))
            .with(Act::Fire, Binding::Key(PhysicalKey::SPACE))
            .with(
                Act::Steer,
                Binding::KeyAxis {
                    neg: PhysicalKey::KEY_Q,
                    pos: PhysicalKey::KEY_E,
                },
            )
    }

    #[test]
    fn pressed_held_released_follow_any_binding() {
        let (mut a, mut i) = (actions(), InputState::new());
        key(&mut i, PhysicalKey::KEY_A, true);
        a.update(&i);
        i.end_update();
        assert!(a.pressed(Act::Left) && a.held(Act::Left));
        // The second binding going down too isn't a new press.
        key(&mut i, PhysicalKey::ARROW_LEFT, true);
        a.update(&i);
        i.end_update();
        assert!(!a.pressed(Act::Left) && a.held(Act::Left));
        key(&mut i, PhysicalKey::KEY_A, false);
        a.update(&i);
        i.end_update();
        assert!(
            a.held(Act::Left) && !a.released(Act::Left),
            "one still down"
        );
        key(&mut i, PhysicalKey::ARROW_LEFT, false);
        a.update(&i);
        i.end_update();
        assert!(a.released(Act::Left) && !a.held(Act::Left));
        a.update(&i);
        assert!(!a.released(Act::Left), "released once");
    }

    #[test]
    fn a_tap_between_updates_presses_and_releases() {
        let (mut a, mut i) = (actions(), InputState::new());
        i.button(PointerButton::Left, ButtonState::Pressed);
        i.button(PointerButton::Left, ButtonState::Released);
        a.update(&i);
        assert!(a.pressed(Act::Fire) && a.released(Act::Fire) && !a.held(Act::Fire));
    }

    #[test]
    fn values_and_directions() {
        let (mut a, mut i) = (actions(), InputState::new());
        key(&mut i, PhysicalKey::KEY_Q, true);
        key(&mut i, PhysicalKey::ARROW_RIGHT, true);
        key(&mut i, PhysicalKey::ARROW_UP, true);
        a.update(&i);
        assert_eq!(a.value(Act::Steer), -1.0);
        let (x, y) = a.axis2(Act::Left, Act::Right, Act::Up, Act::Down);
        assert!((x - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((y + std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        key(&mut i, PhysicalKey::KEY_E, true);
        a.update(&i);
        assert_eq!(a.value(Act::Steer), 0.0, "both ways cancel");
        assert!(a.held(Act::Steer));
    }

    #[test]
    fn rebinding_and_capture() {
        let (mut a, mut i) = (actions(), InputState::new());
        assert_eq!(Binding::capture(&i), None);
        key(&mut i, PhysicalKey::KEY_F, true);
        let b = Binding::capture(&i).unwrap();
        assert_eq!(b, Binding::Key(PhysicalKey::KEY_F));
        a.rebind(Act::Fire, 1, b);
        assert_eq!(
            a.bindings(Act::Fire),
            [
                Binding::MouseButton(PointerButton::Left),
                Binding::Key(PhysicalKey::KEY_F)
            ]
        );
        a.unbind(Binding::Key(PhysicalKey::KEY_A));
        assert_eq!(
            a.bindings(Act::Left),
            [Binding::Key(PhysicalKey::ARROW_LEFT)]
        );
        assert_eq!(a.bound_to(Binding::Key(PhysicalKey::KEY_F)), [Act::Fire]);
        let ev = KeyEvent::new(ButtonState::Pressed, KeyCode::Enter, PhysicalKey::ENTER);
        assert_eq!(
            Binding::from_key_event(&ev),
            Some(Binding::Key(PhysicalKey::ENTER))
        );
    }

    #[test]
    fn bindings_round_trip_through_text() {
        let mut a = actions();
        a.rebind(Act::Fire, 0, Binding::MouseButton(PointerButton::Other(7)));
        a.bind(Act::Up, Binding::Key(PhysicalKey(0x3f0)));
        let text = a.to_text(name);
        assert!(text.contains("Fire = Mouse:7, Key:Space\n"), "{text}");
        assert!(text.contains("Up = Key:ArrowUp, Key:#3f0\n"), "{text}");
        assert!(text.contains("Steer = KeyAxis:KeyQ/KeyE\n"), "{text}");
        let mut b = Actions::new().with(Act::Left, Binding::Key(PhysicalKey::KEY_Z));
        b.load_text(&format!("# saved\n\n{text}Gone = Key:KeyX\n"), parse)
            .unwrap();
        assert_eq!(b.to_text(name), text);

        // Saved bindings go over the defaults; a bad line changes nothing.
        let mut c = actions();
        c.load_text("Left = Key:KeyJ, Mouse:Right", parse).unwrap();
        assert_eq!(
            c.bindings(Act::Left),
            [
                Binding::Key(PhysicalKey::KEY_J),
                Binding::MouseButton(PointerButton::Right)
            ]
        );
        assert_eq!(
            c.bindings(Act::Right),
            [Binding::Key(PhysicalKey::ARROW_RIGHT)]
        );
        let err = c
            .load_text("Left = Key:KeyK\nRight = Key:Nope", parse)
            .unwrap_err();
        assert_eq!(err.line, 2);
        assert_eq!(c.bindings(Act::Left)[0], Binding::Key(PhysicalKey::KEY_J));
        assert!("Pad:A".parse::<Binding>().is_err());
        assert!("KeyAxis:KeyA".parse::<Binding>().is_err());
    }
}
