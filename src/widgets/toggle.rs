//! Checkboxes, switches and radio buttons, each with its label.

use crate::core::{Align, AnimProp, Axis, BoxStyle, Cx, Element, Role, Transition};
use crate::geometry::{Insets, Transform};
use crate::platform::native::NativeControl;
use crate::render::Color;
use crate::style::Theme;

use super::{label, native_size, spacer};

/// Which control a [`Toggle`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Checkbox,
    Switch,
    Radio,
}

/// A checkbox, switch or radio button with its label: the spec behind
/// [`checkbox`], [`switch`] and [`radio`], for the controls they don't
/// make: one whose label isn't shown.
///
/// The label is drawn next to the control (after a checkbox or a radio
/// button, before a switch), takes taps as the control does, and names it
/// for assistive technologies and for [`App::find`](crate::App::find).
/// Where the control's meaning shows some other way (a checkbox in each row
/// of a table, a switch in a settings row that has its own caption),
/// [`label_visible(false)`](Toggle::label_visible) leaves the label
/// unseen; it still names the control.
///
/// Like [`Button`](super::Button), [`view`](Toggle::view) builds the control
/// and the tap handler comes after it:
///
/// ```
/// use stipple::prelude::*;
/// struct Rows { picked: Vec<bool> }
/// fn pick(cx: &mut Cx<Rows>, i: usize, picked: bool) -> Element {
///     Toggle::checkbox(format!("Select row {}", i + 1), picked)
///         .label_visible(false)
///         .view(cx)
///         .on_tap(cx, move |s| s.picked[i] = !s.picked[i])
/// }
/// # let theme = Theme::light();
/// # let _ = pick(&mut Cx::new(&theme), 0, true);
/// ```
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Toggle {
    kind: Kind,
    label: String,
    on: bool,
    label_visible: bool,
}

impl Toggle {
    /// A checkbox labeled `label`, showing a check mark when `checked`.
    pub fn checkbox(label: impl Into<String>, checked: bool) -> Self {
        Self::of(Kind::Checkbox, label.into(), checked)
    }

    /// An on/off switch labeled `label`, its knob at the end when `on`.
    pub fn switch(label: impl Into<String>, on: bool) -> Self {
        Self::of(Kind::Switch, label.into(), on)
    }

    /// A radio button labeled `label`, filled when `selected`.
    pub fn radio(label: impl Into<String>, selected: bool) -> Self {
        Self::of(Kind::Radio, label.into(), selected)
    }

    fn of(kind: Kind, label: String, on: bool) -> Self {
        Toggle {
            kind,
            label,
            on,
            label_visible: true,
        }
    }

    /// Whether the label is drawn next to the control (the default). Not
    /// drawn, it still names the control for assistive technologies.
    pub fn label_visible(mut self, visible: bool) -> Self {
        self.label_visible = visible;
        self
    }

    /// Build the control. It does nothing on a tap until one is given with
    /// [`on_tap`](Element::on_tap).
    pub fn view<S>(self, cx: &Cx<S>) -> Element {
        let theme = cx.theme();
        let (control, role) = match self.kind {
            Kind::Checkbox => (check_box(theme, self.on), Role::CheckBox),
            Kind::Switch => (switch_track(theme, self.on), Role::Switch),
            Kind::Radio => (radio_dot(theme, self.on), Role::RadioButton),
        };
        let control = control.indicator();
        let el = if self.label_visible && !self.label.is_empty() {
            let text = label(cx, self.label.clone());
            let gap = theme.spacing.sm;
            match self.kind {
                // A switch follows its label, at the end of the row when
                // the row is stretched (a settings list).
                Kind::Switch => Element::stack(
                    Axis::Horizontal,
                    vec![text, spacer().min_width(gap), control],
                ),
                Kind::Checkbox | Kind::Radio => {
                    Element::stack(Axis::Horizontal, vec![control, text]).gap(gap)
                }
            }
            .align(Align::Start, Align::Center)
        } else {
            control
        };
        el.role(role).checked(self.on).a11y_label(self.label)
    }
}

/// A checkbox labeled `label`, drawn after the box: tapping either calls
/// `on_toggle`. The box shows a check mark when `checked`.
///
/// ```
/// use stipple::prelude::*;
/// struct Todo { done: bool }
/// fn view(state: &Todo, cx: &mut Cx<Todo>) -> Element {
///     checkbox(cx, "Buy milk", state.done, |s| s.done = !s.done)
/// }
/// # let theme = Theme::light();
/// # let _ = view(&Todo { done: false }, &mut Cx::new(&theme));
/// ```
///
/// [`Toggle::checkbox`] makes one whose label isn't shown.
pub fn checkbox<S>(
    cx: &mut Cx<S>,
    label: impl Into<String>,
    checked: bool,
    on_toggle: impl FnMut(&mut S) + 'static,
) -> Element {
    Toggle::checkbox(label, checked)
        .view(cx)
        .on_tap(cx, on_toggle)
}

/// An on/off switch after its `label`: a pill track with a knob that sits at
/// the end when `on` (the track tinted with the primary color) and at the
/// start when off. Tapping the switch or its label calls `on_toggle`.
/// Stretched across a column, as a settings list does, the label sits at
/// the start and the switch at the end.
///
/// [`Toggle::switch`] makes one whose label isn't shown.
pub fn switch<S>(
    cx: &mut Cx<S>,
    label: impl Into<String>,
    on: bool,
    on_toggle: impl FnMut(&mut S) + 'static,
) -> Element {
    Toggle::switch(label, on).view(cx).on_tap(cx, on_toggle)
}

/// A radio button labeled `label`, drawn after the circle, which is filled
/// with the primary color when `selected`. Tapping either calls `on_select`.
/// Assistive technologies see a radio button, checked while `selected`; put
/// a group of them in a row or column made a
/// [`FocusGroup::Select`](crate::core::FocusGroup::Select)
/// [`focus_group`](Element::focus_group) for the arrow keys to move between
/// them.
///
/// [`Toggle::radio`] makes one whose label isn't shown.
pub fn radio<S>(
    cx: &mut Cx<S>,
    label: impl Into<String>,
    selected: bool,
    on_select: impl FnMut(&mut S) + 'static,
) -> Element {
    Toggle::radio(label, selected)
        .view(cx)
        .on_tap(cx, on_select)
}

/// The box of a checkbox.
fn check_box(theme: &Theme, checked: bool) -> Element {
    let control = NativeControl::Checkbox { checked };
    if let Some(deco) = theme.native_deco(control) {
        return native_size(Element::boxed(BoxStyle::default()).native(deco), control);
    }
    if let Some(skin) = theme.skin.and_then(|s| s.checkbox.as_ref()) {
        // The skin's box, its mark drawn in.
        return Element::stack(Axis::Horizontal, Vec::new())
            .background_image(skin.frame(checked))
            .padding(Insets::ZERO)
            .width(22.0)
            .height(22.0);
    }
    // The mark is always there, faded out and shrunk while unchecked, so it
    // pops in (and out) on the theme's toggle motion.
    let m = theme.motion;
    let mark = Element::text("✓", theme.font_size, theme.palette.on_primary);
    let mark = if checked {
        mark
    } else {
        mark.opacity(0.0).transform(Transform::IDENTITY.scale(0.4))
    };
    let mark = mark
        .transition_for(AnimProp::Opacity, Transition(m.toggle))
        .transition_for(AnimProp::Transform, Transition(m.toggle));
    Element::stack(Axis::Horizontal, vec![mark])
        .fill(if checked {
            theme.palette.primary
        } else {
            theme.palette.surface
        })
        .transition_for(AnimProp::Fill, Transition(m.state))
        .border(theme.palette.border, 1.0)
        .radius(theme.radius / 2.0)
        .width(22.0)
        .height(22.0)
        .align(Align::Center, Align::Center)
}

/// The track and knob of a switch.
fn switch_track(theme: &Theme, on: bool) -> Element {
    let control = NativeControl::Switch { on };
    if let Some(deco) = theme.native_deco(control) {
        return native_size(Element::boxed(BoxStyle::default()).native(deco), control);
    }
    let skin = theme.skin.and_then(|s| s.switch.as_ref());
    let knob = match skin.and_then(|s| s.knob.as_ref()) {
        Some(knob) => Element::stack(Axis::Horizontal, Vec::new())
            .background_image(knob)
            .padding(Insets::ZERO),
        None => Element::boxed(BoxStyle {
            fill: Some(Color::WHITE),
            radius: 8.0,
            border: None,
        }),
    }
    .width(16.0)
    .height(16.0);
    // The knob sits after a lead that is as wide as its travel when on (40
    // wide, less 3 of padding a side and the knob's 16) and empty when off,
    // so the lead's width slides it over on the theme's toggle motion.
    let m = theme.motion;
    let lead = Element::stack(Axis::Horizontal, Vec::new())
        .width(if on { 18.0 } else { 0.0 })
        .transition_for(AnimProp::Width, Transition(m.toggle));
    let track = Element::stack(Axis::Horizontal, vec![lead, knob]);
    if let Some(skin) = skin {
        return track
            .background_image(skin.frame(on))
            .width(40.0)
            .height(22.0)
            .padding(Insets::uniform(3.0));
    }
    track
        .fill(if on {
            theme.palette.primary
        } else {
            theme.palette.border
        })
        .transition_for(AnimProp::Fill, Transition(m.state))
        .radius(11.0)
        .width(40.0)
        .height(22.0)
        .padding(Insets::uniform(3.0))
}

/// The circle of a radio button.
fn radio_dot(theme: &Theme, selected: bool) -> Element {
    let control = NativeControl::Radio { selected };
    if let Some(deco) = theme.native_deco(control) {
        return native_size(Element::boxed(BoxStyle::default()).native(deco), control);
    }
    // Like the checkbox's mark, the dot is always there, faded out and
    // shrunk while unselected.
    let m = theme.motion;
    let dot = Element::boxed(BoxStyle {
        fill: Some(theme.palette.primary),
        radius: 5.0,
        border: None,
    })
    .width(10.0)
    .height(10.0);
    let dot = if selected {
        dot
    } else {
        dot.opacity(0.0).transform(Transform::IDENTITY.scale(0.2))
    };
    let dot = dot
        .transition_for(AnimProp::Opacity, Transition(m.toggle))
        .transition_for(AnimProp::Transform, Transition(m.toggle));
    Element::stack(Axis::Horizontal, vec![dot])
        .fill(theme.palette.surface)
        .border(theme.palette.border, 1.0)
        .radius(11.0) // half the size → a circle
        .width(22.0)
        .height(22.0)
        .align(Align::Center, Align::Center)
}
