//! A four-function calculator, exercising buttons, a button grid (rows stacked
//! in a column), flexible sizing (`grow`), and a small state machine driven
//! entirely through `on_tap` handlers that mutate the shared app state.
//!
//! Layout:
//! ```text
//! ┌───────────────────────────┐
//! │                      0    │  ← right-aligned display
//! ├─────┬─────┬─────┬─────────┤
//! │  C  │  ←  │  %  │   ÷     │
//! │  7  │  8  │  9  │   ×     │
//! │  4  │  5  │  6  │   −     │
//! │  1  │  2  │  3  │   +     │
//! │     0     │  .  │   =     │  ← "0" spans two columns (grow = 2)
//! └───────────┴─────┴─────────┘
//! ```

use stipple::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

/// Calculator state: the text on the display plus the pending operation.
struct Calc {
    /// What the display shows (also the number currently being entered).
    display: String,
    /// The left operand captured when an operator was pressed.
    acc: Option<f64>,
    /// The pending operator, applied on the next operator or `=`.
    op: Option<Op>,
    /// True when the next digit starts a fresh number (replacing the display).
    fresh: bool,
}

impl Default for Calc {
    fn default() -> Self {
        Self {
            display: "0".to_string(),
            acc: None,
            op: None,
            fresh: true,
        }
    }
}

impl Calc {
    fn value(&self) -> f64 {
        self.display.parse().unwrap_or(0.0)
    }

    fn input_digit(&mut self, d: &str) {
        if self.fresh || self.display == "0" {
            self.display = d.to_string();
            self.fresh = false;
        } else {
            self.display.push_str(d);
        }
    }

    fn input_dot(&mut self) {
        if self.fresh {
            self.display = "0.".to_string();
            self.fresh = false;
        } else if !self.display.contains('.') {
            self.display.push('.');
        }
    }

    fn set_op(&mut self, next: Op) {
        // Chain operators: fold the running accumulator with the current entry
        // before starting the next operation (so "1 + 2 + 3" works left to right).
        let cur = self.value();
        match (self.acc, self.op) {
            (Some(a), Some(op)) if !self.fresh => match apply(a, op, cur) {
                Some(r) => {
                    self.acc = Some(r);
                    self.display = fmt(r);
                }
                None => {
                    self.error();
                    return;
                }
            },
            _ => self.acc = Some(cur),
        }
        self.op = Some(next);
        self.fresh = true;
    }

    fn equals(&mut self) {
        if let (Some(a), Some(op)) = (self.acc, self.op) {
            match apply(a, op, self.value()) {
                Some(r) => self.display = fmt(r),
                None => return self.error(),
            }
        }
        self.acc = None;
        self.op = None;
        self.fresh = true;
    }

    fn percent(&mut self) {
        self.display = fmt(self.value() / 100.0);
        self.fresh = true;
    }

    fn backspace(&mut self) {
        if self.fresh {
            return;
        }
        self.display.pop();
        if self.display.is_empty() || self.display == "-" {
            self.display = "0".to_string();
            self.fresh = true;
        }
    }

    fn clear(&mut self) {
        *self = Calc::default();
    }

    fn error(&mut self) {
        *self = Calc::default();
        self.display = "Error".to_string();
    }
}

fn apply(a: f64, op: Op, b: f64) -> Option<f64> {
    Some(match op {
        Op::Add => a + b,
        Op::Sub => a - b,
        Op::Mul => a * b,
        Op::Div if b == 0.0 => return None,
        Op::Div => a / b,
    })
}

/// Format a result without trailing-zero / float-noise clutter.
fn fmt(x: f64) -> String {
    if !x.is_finite() {
        return "Error".to_string();
    }
    if x == x.trunc() && x.abs() < 1e15 {
        return format!("{}", x as i64);
    }
    let s = format!("{x:.10}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// One keypad button: a variant-styled label, a tap handler, a fixed height, and
/// a flex weight so a row's cells share its width (`weight = 2` double-wide).
fn key(
    cx: &mut Cx<Calc>,
    text: &str,
    variant: Variant,
    weight: f64,
    on: impl FnMut(&mut Calc) + 'static,
) -> Element {
    Button::new(text)
        .variant(variant)
        .view(cx)
        .on_tap(cx, on)
        .height(60.0)
        .grow(weight)
}

fn digit(cx: &mut Cx<Calc>, d: &'static str) -> Element {
    key(cx, d, Variant::Secondary, 1.0, move |s| s.input_digit(d))
}

fn operator(cx: &mut Cx<Calc>, text: &str, op: Op) -> Element {
    key(cx, text, Variant::Primary, 1.0, move |s| s.set_op(op))
}

fn keypad_row(children: Vec<Element>) -> Element {
    row(children).gap(8.0).align(Align::Start, Align::Stretch)
}

fn view(state: &Calc, cx: &mut Cx<Calc>) -> Element {
    let theme = *cx.theme();

    // Right-aligned display showing the current entry / result.
    let display = row(vec![heading(cx, &state.display)])
        .align(Align::End, Align::Center)
        .padding(Insets::symmetric(theme.spacing.lg, theme.spacing.md))
        .height(76.0)
        .fill(theme.palette.background)
        .radius(theme.radius);

    let keypad = column(vec![
        keypad_row(vec![
            key(cx, "C", Variant::Danger, 1.0, |s| s.clear()),
            key(cx, "←", Variant::Secondary, 1.0, |s| s.backspace()),
            key(cx, "%", Variant::Secondary, 1.0, |s| s.percent()),
            operator(cx, "÷", Op::Div),
        ]),
        keypad_row(vec![
            digit(cx, "7"),
            digit(cx, "8"),
            digit(cx, "9"),
            operator(cx, "×", Op::Mul),
        ]),
        keypad_row(vec![
            digit(cx, "4"),
            digit(cx, "5"),
            digit(cx, "6"),
            operator(cx, "−", Op::Sub),
        ]),
        keypad_row(vec![
            digit(cx, "1"),
            digit(cx, "2"),
            digit(cx, "3"),
            operator(cx, "+", Op::Add),
        ]),
        keypad_row(vec![
            key(cx, "0", Variant::Secondary, 2.0, |s| s.input_digit("0")),
            key(cx, ".", Variant::Secondary, 1.0, |s| s.input_dot()),
            key(cx, "=", Variant::Primary, 1.0, |s| s.equals()),
        ]),
    ])
    .gap(8.0);

    panel(cx).children(vec![display, keypad])
}

fn main() {
    let app = App::new(Calc::default(), view)
        .title("Stipple Calculator")
        .theme(Theme::dark())
        .logical_size(Size::new(300.0, 460.0));
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the state machine the way the buttons do, then read the display.
    fn run(seq: &[&str]) -> String {
        let mut c = Calc::default();
        for tok in seq {
            match *tok {
                "+" => c.set_op(Op::Add),
                "-" => c.set_op(Op::Sub),
                "*" => c.set_op(Op::Mul),
                "/" => c.set_op(Op::Div),
                "=" => c.equals(),
                "." => c.input_dot(),
                "%" => c.percent(),
                "C" => c.clear(),
                "<" => c.backspace(),
                d => c.input_digit(d),
            }
        }
        c.display
    }

    #[test]
    fn addition_and_chaining() {
        assert_eq!(run(&["7", "+", "8", "="]), "15");
        // Left-to-right chaining without pressing '=' between operators.
        assert_eq!(run(&["1", "+", "2", "+", "3", "="]), "6");
    }

    #[test]
    fn the_four_functions() {
        assert_eq!(run(&["6", "*", "7", "="]), "42");
        assert_eq!(run(&["9", "-", "4", "="]), "5");
        assert_eq!(run(&["8", "/", "2", "="]), "4");
    }

    #[test]
    fn decimals_and_percent() {
        assert_eq!(run(&["1", ".", "5", "+", "2", "="]), "3.5");
        assert_eq!(run(&["5", "0", "%"]), "0.5");
    }

    #[test]
    fn division_by_zero_is_an_error() {
        assert_eq!(run(&["5", "/", "0", "="]), "Error");
    }

    #[test]
    fn clear_and_backspace() {
        assert_eq!(run(&["1", "2", "3", "<"]), "12");
        assert_eq!(run(&["1", "2", "C"]), "0");
    }
}

// In the browser, `web.rs` builds this file as a library and the page
// runs `main`'s app (on the web, `App::run` hands it to the page).
stipple::web_app!(main);
