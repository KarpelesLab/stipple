//! A number input: an editable numeric field with stepper buttons, clamped to
//! a range and stepped by a fixed increment.

use crate::core::{Align, Axis, Cx, Element, KeyInput, Role};
use crate::geometry::Insets;
use crate::style::Theme;

use super::EditBuffer;

/// The state of a [`number_input`]: its value, range, step, and the text being
/// edited. Keep one in your app state per input.
///
/// The value is always a finite number inside the range. While the text is
/// being edited, each edit that parses updates the value (clamped to the
/// range); Enter rewrites the text from the value, and Escape reverts it.
/// Up/Down and the stepper buttons add or subtract one step.
///
/// ```
/// use stipple::widgets::NumberInput;
/// let mut n = NumberInput::new(5.0).range(0.0, 10.0).step(0.5);
/// n.increment();
/// assert_eq!((n.value(), n.text()), (5.5, "5.5"));
/// n.set_value(42.0);
/// assert_eq!(n.value(), 10.0); // clamped
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct NumberInput {
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    /// Digits after the decimal point; `None` follows the step's.
    precision: Option<usize>,
    buffer: EditBuffer,
}

/// Digits after the decimal point in `step`'s shortest form (at most 10).
fn decimals_of(step: f64) -> usize {
    let s = format!("{step}");
    s.split_once('.').map_or(0, |(_, frac)| frac.len().min(10))
}

impl NumberInput {
    /// An input holding `value`, unbounded, stepping by 1.
    pub fn new(value: f64) -> Self {
        let mut n = Self {
            value: if value.is_finite() { value } else { 0.0 },
            min: f64::MIN,
            max: f64::MAX,
            step: 1.0,
            precision: None,
            buffer: EditBuffer::new(),
        };
        n.reset();
        n
    }

    /// Limit the value to `min..=max` (swapped if reversed).
    pub fn range(mut self, min: f64, max: f64) -> Self {
        let (min, max) = if min <= max { (min, max) } else { (max, min) };
        self.min = min;
        self.max = max;
        self.reset();
        self
    }

    /// Step by `step` (its absolute value; zero or not finite keeps the
    /// current step). The text shows as many decimals as the step has, unless
    /// set with [`precision`](Self::precision).
    pub fn step(mut self, step: f64) -> Self {
        if step.is_finite() && step != 0.0 {
            self.step = step.abs();
        }
        self.reset();
        self
    }

    /// Show and round to `digits` digits after the decimal point.
    pub fn precision(mut self, digits: usize) -> Self {
        self.precision = Some(digits.min(15));
        self.reset();
        self
    }

    /// The current value: finite and inside the range.
    pub fn value(&self) -> f64 {
        self.value
    }

    /// The lower bound.
    pub fn min(&self) -> f64 {
        self.min
    }

    /// The upper bound.
    pub fn max(&self) -> f64 {
        self.max
    }

    /// The text in the field.
    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// The text buffer, for its caret and selection.
    pub fn buffer(&self) -> &EditBuffer {
        &self.buffer
    }

    fn digits(&self) -> usize {
        self.precision.unwrap_or_else(|| decimals_of(self.step))
    }

    /// `v` clamped to the range and rounded to the precision.
    fn normalize(&self, v: f64) -> f64 {
        let v = if v.is_finite() { v } else { self.value };
        let scale = 10f64.powi(self.digits() as i32);
        let rounded = (v * scale).round() / scale;
        let v = if rounded.is_finite() { rounded } else { v };
        v.clamp(self.min, self.max)
    }

    fn format(&self) -> String {
        let s = format!("{:.*}", self.digits(), self.value);
        // Don't show "-0".
        if self.value == 0.0 && s.starts_with('-') {
            s[1..].to_string()
        } else {
            s
        }
    }

    /// After a builder: clamp the value (without rounding it, so the order of
    /// `range`, `step` and `precision` doesn't matter) and rewrite the text.
    fn reset(&mut self) {
        self.value = self.value.clamp(self.min, self.max);
        self.buffer = EditBuffer::from_text(self.format());
    }

    /// Set the value (clamped and rounded) and rewrite the text from it.
    pub fn set_value(&mut self, value: f64) {
        self.value = self.normalize(value);
        self.buffer = EditBuffer::from_text(self.format());
    }

    /// Add one step, up to the maximum.
    pub fn increment(&mut self) {
        self.set_value(self.value + self.step);
    }

    /// Subtract one step, down to the minimum.
    pub fn decrement(&mut self) {
        self.set_value(self.value - self.step);
    }

    /// The text parsed as a number, if it is one.
    fn parsed(&self) -> Option<f64> {
        self.text()
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
    }

    /// Whether the text is a number inside the range. When it isn't, the field
    /// shows a danger-colored border and the value keeps its last good number.
    pub fn is_valid(&self) -> bool {
        self.parsed()
            .is_some_and(|v| v >= self.min && v <= self.max)
    }

    /// Rewrite the text from the value (what Enter does).
    pub fn commit(&mut self) {
        self.set_value(self.value);
    }

    /// Handle a key pressed while the field has focus. Returns `true` if the
    /// value changed.
    ///
    /// Up and Down step; Enter commits the text; Escape reverts it. Typed text
    /// keeps only the characters a number can have (digits, `.`, `-`, `+`,
    /// `e`); every other key edits the text as in a
    /// [`text_editor`](super::text_editor).
    pub fn apply(&mut self, key: &KeyInput) -> bool {
        let before = self.value;
        match key {
            KeyInput::Up => self.increment(),
            KeyInput::Down => self.decrement(),
            KeyInput::Enter | KeyInput::Escape => self.commit(),
            KeyInput::SelectLine => self.buffer.select_all(),
            KeyInput::Text(t) => {
                let kept: String = t
                    .chars()
                    .filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
                    .collect();
                if !kept.is_empty() {
                    self.buffer.insert(&kept);
                    self.sync();
                }
            }
            other => {
                self.buffer.apply(other);
                self.sync();
            }
        }
        self.value != before
    }

    /// After an edit: adopt the text's number (clamped) if it parses.
    fn sync(&mut self) {
        if let Some(v) = self.parsed() {
            self.value = self.normalize(v);
        }
    }

    /// Place the caret from a pointer press (or extend the selection on drag).
    fn pointer(&mut self, index: usize, extend: bool) {
        if extend {
            self.buffer.extend_to(index);
        } else {
            self.buffer.place_caret(index);
        }
    }
}

/// A number input: an editable field showing `state`'s value, with increment
/// and decrement buttons beside it. Up/Down step it while focused, and typing
/// edits the number (see [`NumberInput::apply`]); text that isn't a number in
/// range gets a danger-colored border. Default width 140 logical px; override
/// with `.width(..)`.
///
/// `field` is an accessor returning the [`NumberInput`] inside the app state.
///
/// Under [`Theme::native`] the OS draws the field's frame; the steppers stay
/// self-drawn. The field is reported to assistive technologies as a
/// [`Role::SpinButton`] named by its value.
pub fn number_input<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    state: &NumberInput,
    field: impl Fn(&mut S) -> &mut NumberInput + Copy + 'static,
) -> Element {
    let p = &theme.palette;
    let buffer = state.buffer();
    let shown = if buffer.is_empty() {
        String::from(" ")
    } else {
        buffer.text().to_string()
    };
    let text = Element::text(shown, theme.font_size, p.text)
        .caret(buffer.caret())
        .selection(buffer.selection());
    let border = if state.is_valid() { p.border } else { p.danger };
    let mut edit = Element::stack(Axis::Horizontal, vec![text])
        .fill(p.surface)
        .radius(theme.radius)
        .border(border, 1.0)
        .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
        .align(Align::Start, Align::Center)
        .grow(1.0)
        .shrink(1.0);
    // An invalid entry keeps the self-drawn frame, so its red border shows.
    if state.is_valid() {
        edit = edit.native_text_frame(theme);
    }
    let edit = edit
        .role(Role::SpinButton)
        .on_key(cx, move |s: &mut S, k| {
            field(s).apply(k);
        })
        .on_text_pos(cx, move |s: &mut S, index, extend| {
            field(s).pointer(index, extend)
        });

    let up = stepper(theme, "\u{25B4}", "Increase", state.value < state.max)
        .on_tap(cx, move |s: &mut S| field(s).increment());
    let down = stepper(theme, "\u{25BE}", "Decrease", state.value > state.min)
        .on_tap(cx, move |s: &mut S| field(s).decrement());
    let steppers = Element::stack(Axis::Vertical, vec![up, down])
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(theme.radius)
        .gap(1.0)
        .align(Align::Start, Align::Stretch)
        .width(24.0);

    Element::stack(Axis::Horizontal, vec![edit, steppers])
        .gap(2.0)
        .align(Align::Start, Align::Stretch)
        .width(140.0)
}

/// One stepper button: an arrow, muted when it can't step further.
fn stepper(theme: &Theme, arrow: &str, name: &str, enabled: bool) -> Element {
    let ink = if enabled {
        theme.palette.text
    } else {
        theme.palette.text_muted
    };
    Element::stack(
        Axis::Horizontal,
        vec![Element::text(arrow, theme.typography.caption, ink)],
    )
    .radius(theme.radius / 2.0)
    .align(Align::Center, Align::Center)
    .grow(1.0)
    .a11y_label(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::geometry::Size;

    fn typed(n: &mut NumberInput, text: &str) {
        n.apply(&KeyInput::Text(text.into()));
    }

    #[test]
    fn clamps_to_the_range() {
        let mut n = NumberInput::new(7.0).range(0.0, 10.0);
        n.set_value(12.0);
        assert_eq!(n.value(), 10.0);
        n.set_value(-3.0);
        assert_eq!(n.value(), 0.0);
        n.decrement();
        assert_eq!(n.value(), 0.0, "stays at the minimum");
        // A reversed range is swapped.
        let r = NumberInput::new(5.0).range(10.0, 1.0);
        assert_eq!((r.min(), r.max()), (1.0, 10.0));
        // Narrowing the range clamps the current value.
        assert_eq!(NumberInput::new(50.0).range(0.0, 10.0).value(), 10.0);
        // Builders keep the value whatever their order.
        let o = NumberInput::new(0.8).range(0.0, 1.0).step(0.05);
        assert_eq!((o.value(), o.text()), (0.8, "0.80"));
    }

    #[test]
    fn steps_without_float_drift() {
        let mut n = NumberInput::new(0.0).step(0.1);
        for _ in 0..3 {
            n.increment();
        }
        assert_eq!(n.text(), "0.3");
        assert_eq!(n.value(), 0.3);
        let mut m = NumberInput::new(1.0).step(0.25).range(0.0, 2.0);
        m.apply(&KeyInput::Up);
        assert_eq!(m.text(), "1.25");
        m.apply(&KeyInput::Down);
        m.apply(&KeyInput::Down);
        assert_eq!(m.text(), "0.75");
        // Explicit precision beats the step's.
        assert_eq!(NumberInput::new(2.0).precision(2).text(), "2.00");
        assert_eq!(NumberInput::new(-0.0).text(), "0");
    }

    #[test]
    fn typing_updates_the_value_and_enter_commits() {
        let mut n = NumberInput::new(5.0).range(0.0, 100.0);
        n.apply(&KeyInput::SelectAll);
        typed(&mut n, "4x2"); // the x is dropped
        assert_eq!((n.text(), n.value()), ("42", 42.0));
        assert!(n.is_valid());
        typed(&mut n, "0"); // 420: out of range, value clamps
        assert_eq!(n.value(), 100.0);
        assert!(!n.is_valid());
        n.apply(&KeyInput::Enter);
        assert_eq!((n.text(), n.value()), ("100", 100.0));
        assert!(n.is_valid());
    }

    #[test]
    fn unparsable_text_keeps_the_last_value_and_escape_reverts() {
        let mut n = NumberInput::new(3.0);
        n.apply(&KeyInput::SelectAll);
        typed(&mut n, "-");
        assert_eq!(n.text(), "-");
        assert_eq!(n.value(), 3.0);
        assert!(!n.is_valid());
        n.apply(&KeyInput::Backspace);
        assert_eq!(n.text(), "");
        n.apply(&KeyInput::Escape);
        assert_eq!(n.text(), "3");
    }

    struct State {
        n: NumberInput,
    }

    #[test]
    fn steppers_and_arrow_keys_step() {
        let mut app = App::new(
            State {
                n: NumberInput::new(1.0).range(0.0, 3.0),
            },
            |s: &State, cx: &mut Cx<State>| {
                let theme = *cx.theme();
                super::super::column(vec![number_input(cx, &theme, &s.n, |s: &mut State| {
                    &mut s.n
                })])
            },
        )
        .logical_size(Size::new(300.0, 100.0));
        app.render_once();
        let tree = app.accessibility_tree().unwrap();
        let nodes = tree.descendants();
        let spin = nodes.iter().find(|n| n.role == Role::SpinButton).unwrap();
        assert_eq!(spin.name, "1");
        let inc = nodes.iter().find(|n| n.name == "Increase").unwrap();
        let (inc, field) = (inc.bounds.center(), spin.bounds.center());
        assert!(app.click_at(inc));
        assert_eq!(app.state().n.value(), 2.0);
        app.click_at(inc);
        app.click_at(inc); // clamps at 3
        assert_eq!(app.state().n.value(), 3.0);
        // Focus the field, then step down with the keyboard.
        app.click_at(field);
        assert!(app.press_key(KeyInput::Down));
        assert_eq!(app.state().n.text(), "2");
    }
}
