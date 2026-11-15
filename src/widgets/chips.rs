//! Chips: small pill-shaped toggles, alone ([`chip`]) or in a group that picks
//! one option ([`choice_chips`], radio buttons to assistive technologies) or
//! turns several filters on and off ([`filter_chips`], checkboxes). A group
//! wraps onto more lines when its chips don't fit; inside
//! [`scroll_horizontal`](super::scroll_horizontal) it stays on one line and
//! scrolls sideways. In a group the arrow keys move focus from chip to chip
//! (see [`FocusGroup`]).

use crate::core::{Align, AnimProp, Axis, Cx, Element, FocusGroup, Role, Transition};
use crate::geometry::{Insets, Transform};
use crate::render::ImageSource;
use crate::style::Theme;

/// A chip's height, in logical pixels.
const HEIGHT: f64 = 32.0;

/// One chip: its label and, optionally, a leading icon. A `&str` or a
/// `String` makes one without an icon.
#[derive(Clone, Debug)]
pub struct Chip {
    label: String,
    icon: Option<ImageSource>,
}

impl Chip {
    /// A chip labeled `label`.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: None,
        }
    }

    /// Show `icon` (a [`Pixmap`] or an [`Svg`], drawn at the label's size)
    /// before the label. The check mark takes its place while the chip is
    /// selected.
    ///
    /// [`Pixmap`]: crate::render::Pixmap
    /// [`Svg`]: crate::render::Svg
    pub fn icon(mut self, icon: impl Into<ImageSource>) -> Self {
        self.icon = Some(icon.into());
        self
    }
}

impl From<&str> for Chip {
    fn from(label: &str) -> Self {
        Chip::new(label)
    }
}

impl From<String> for Chip {
    fn from(label: String) -> Self {
        Chip::new(label)
    }
}

impl From<&String> for Chip {
    fn from(label: &String) -> Self {
        Chip::new(label.clone())
    }
}

/// A chip that is on or off: a pill showing `chip`'s label (and icon), filled
/// with the accent color and led by a check mark while `selected`. Tapping
/// it (or Enter or Space while it has focus) runs `on_tap`; the app flips
/// `selected`. The mark slides and fades in on the theme's
/// [`toggle`](crate::style::MotionTokens::toggle) motion, the fill on its
/// `state` motion. Assistive technologies see a checkbox.
pub fn chip<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    chip: impl Into<Chip>,
    selected: bool,
    on_tap: impl FnMut(&mut S) + 'static,
) -> Element {
    chip_with_role(cx, theme, chip.into(), selected, Role::CheckBox, on_tap)
}

/// A group of chips of which one is picked, the `selected` one (or none):
/// tapping a chip runs `on_select` with its index. The chips wrap onto more
/// lines when they don't fit. To assistive technologies they are radio
/// buttons; the keyboard treats them as such: Tab stops at the selected chip
/// only, and the arrow keys move to the previous or next chip and pick it.
///
/// `on_select` must be `Clone` because it is stamped onto every chip (each
/// with its own index); a plain closure capturing `Copy`/`Clone` data
/// satisfies this.
///
/// ```
/// use stipple::prelude::*;
/// struct State { symbology: usize }
/// fn view(s: &State, cx: &mut Cx<State>) -> Element {
///     let theme = *cx.theme();
///     choice_chips(
///         cx,
///         &theme,
///         ["2D", "Stacked", "Linear", "Postal"],
///         s.symbology,
///         |s: &mut State, i| s.symbology = i,
///     )
/// }
/// # let theme = Theme::light();
/// # let mut cx = Cx::new(&theme);
/// # let _ = view(&State { symbology: 0 }, &mut cx);
/// ```
pub fn choice_chips<S, C: Into<Chip>>(
    cx: &mut Cx<S>,
    theme: &Theme,
    chips: impl IntoIterator<Item = C>,
    selected: impl Into<Option<usize>>,
    on_select: impl Fn(&mut S, usize) + Clone + 'static,
) -> Element {
    let selected = selected.into();
    let chips = chips
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let on = on_select.clone();
            chip_with_role(
                cx,
                theme,
                c.into(),
                selected == Some(i),
                Role::RadioButton,
                move |s: &mut S| on(s, i),
            )
        })
        .collect();
    chip_group(theme, chips, FocusGroup::Select)
}

/// A group of chips each turned on or off on its own, as filters are:
/// chip `i` is on when `selected[i]` is `true` (a missing entry reads as
/// off), and tapping it runs `on_toggle` with `i`; the app flips its entry.
/// The chips wrap onto more lines when they don't fit. To assistive
/// technologies they are checkboxes; each is a Tab stop, and the arrow keys
/// move focus between them without toggling any.
///
/// `on_toggle` must be `Clone`, as for [`choice_chips`].
pub fn filter_chips<S, C: Into<Chip>>(
    cx: &mut Cx<S>,
    theme: &Theme,
    chips: impl IntoIterator<Item = C>,
    selected: &[bool],
    on_toggle: impl Fn(&mut S, usize) + Clone + 'static,
) -> Element {
    let chips = chips
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let on = on_toggle.clone();
            chip_with_role(
                cx,
                theme,
                c.into(),
                selected.get(i).copied().unwrap_or(false),
                Role::CheckBox,
                move |s: &mut S| on(s, i),
            )
        })
        .collect();
    chip_group(theme, chips, FocusGroup::Move)
}

/// The wrapping row a group's chips sit in, the arrow keys moving focus
/// among them.
fn chip_group(theme: &Theme, chips: Vec<Element>, group: FocusGroup) -> Element {
    Element::stack(Axis::Horizontal, chips)
        .wrap()
        .gap(theme.spacing.sm)
        .focus_group(group)
}

fn chip_with_role<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    chip: Chip,
    selected: bool,
    role: Role,
    on_tap: impl FnMut(&mut S) + 'static,
) -> Element {
    let p = &theme.palette;
    let m = theme.motion;
    let state = Transition(m.state);
    let toggle = Transition(m.toggle);
    let size = theme.typography.body;
    let ink = if selected { p.on_primary } else { p.text };
    // As the checkbox's, the mark is always there, faded out and shrunk while
    // the chip is off, so it pops in (and out) on the toggle motion.
    let mark = Element::text("✓", size, p.on_primary);
    let mark = if selected {
        mark
    } else {
        mark.opacity(0.0).transform(Transform::IDENTITY.scale(0.4))
    };
    let mark = mark
        .transition_for(AnimProp::Opacity, toggle)
        .transition_for(AnimProp::Transform, toggle);
    let cell = |content: Element| {
        Element::stack(Axis::Horizontal, vec![content])
            .width(size)
            .height(size)
            .align(Align::Center, Align::Center)
    };
    // The leading slot: the icon, which the mark replaces while selected; or,
    // without one, room for the mark that opens as it pops in.
    let slot_width = size + theme.spacing.xs;
    let slot = match chip.icon {
        Some(icon) => {
            let icon = Element::image(icon).width(size).height(size);
            let icon = if selected { icon.opacity(0.0) } else { icon }
                .transition_for(AnimProp::Opacity, toggle);
            Element::layers(vec![cell(icon), cell(mark)]).width(slot_width)
        }
        None => Element::stack(Axis::Horizontal, vec![cell(mark)])
            .width(if selected { slot_width } else { 0.0 })
            .transition_for(AnimProp::Width, toggle),
    }
    .height(size)
    .align(Align::Start, Align::Center)
    // Unread: the chip is named by its label alone.
    .role(Role::Group);
    let label = Element::text(chip.label.clone(), theme.typography.body, ink)
        .transition_for(AnimProp::TextColor, state);
    let (fill, border) = if selected {
        (p.primary, p.primary)
    } else {
        (p.surface, p.border)
    };
    let el = Element::stack(Axis::Horizontal, vec![slot, label])
        .fill(fill)
        .border(border, 1.0)
        .transition_for(AnimProp::Border, state)
        .radius(HEIGHT / 2.0)
        .height(HEIGHT)
        .padding(Insets::symmetric(theme.spacing.md, 0.0));
    super::pressable(el, theme, Some(fill))
        .on_tap(cx, on_tap)
        .role(role)
        .checked(selected)
        .a11y_label(chip.label)
}
