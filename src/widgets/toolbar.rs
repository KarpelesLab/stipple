//! A toolbar: a row of compact command buttons, grouped by separators.

use crate::core::{Align, Axis, BoxStyle, Cx, Element, Role};
use crate::geometry::Insets;
use crate::render::ImageSource;
use crate::style::Theme;

/// Icons in [`tool_icon_button`] are drawn at this size, in logical pixels.
const ICON: f64 = 18.0;

/// A toolbar: `items` in a row on a surface strip, vertically centered.
/// Build the items with [`tool_button`], [`tool_icon_button`], [`tool_toggle`]
/// and [`tool_separator`]; any other element works too (a [`select`], a
/// [`spacer`] to push the rest to the end). Items that don't fit overflow.
///
/// Self-drawn under every theme, in the theme's colors. Reported to assistive
/// technologies as a [`Role::Toolbar`] holding its buttons.
///
/// [`select`]: super::select
/// [`spacer`]: super::spacer
pub fn toolbar(theme: &Theme, items: Vec<Element>) -> Element {
    Element::stack(Axis::Horizontal, items)
        .fill(theme.palette.surface)
        .border(theme.palette.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::uniform(theme.spacing.xs))
        .gap(theme.spacing.xs)
        .align(Align::Start, Align::Center)
        .role(Role::Toolbar)
}

/// The frame every tool button shares: compact padding, no fill (the hover
/// highlight shows it's clickable), optionally tinted when toggled on.
fn tool_frame(theme: &Theme, children: Vec<Element>, on: bool) -> Element {
    let mut el = Element::stack(Axis::Horizontal, children)
        .radius(theme.radius / 2.0)
        .padding(Insets::symmetric(theme.spacing.sm, theme.spacing.xs))
        .gap(theme.spacing.xs)
        .align(Align::Center, Align::Center)
        .min_height(ICON + 2.0 * theme.spacing.xs)
        .min_width(ICON + 2.0 * theme.spacing.sm);
    if on {
        el = el
            .fill(theme.palette.selection)
            .border(theme.palette.primary, 1.0);
    }
    el
}

/// A toolbar button labeled `text`, running `on_tap` when clicked.
pub fn tool_button<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    text: impl Into<String>,
    on_tap: impl FnMut(&mut S) + 'static,
) -> Element {
    let label = Element::text(text, theme.typography.body, theme.palette.text);
    tool_frame(theme, vec![label], false).on_tap(cx, on_tap)
}

/// A toolbar button showing `icon` (a [`Pixmap`] or [`Svg`], drawn at 18×18),
/// running `on_tap` when clicked. `name` is what assistive technologies read
/// out, since the icon has no text.
///
/// [`Pixmap`]: crate::render::Pixmap
/// [`Svg`]: crate::render::Svg
pub fn tool_icon_button<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    icon: impl Into<ImageSource>,
    name: impl Into<String>,
    on_tap: impl FnMut(&mut S) + 'static,
) -> Element {
    let icon = Element::image(icon).width(ICON).height(ICON);
    tool_frame(theme, vec![icon], false)
        .a11y_label(name)
        .on_tap(cx, on_tap)
}

/// A toolbar button labeled `text` that stays tinted while `on` (bold, a view
/// mode), running `on_tap` when clicked.
pub fn tool_toggle<S>(
    cx: &mut Cx<S>,
    theme: &Theme,
    text: impl Into<String>,
    on: bool,
    on_tap: impl FnMut(&mut S) + 'static,
) -> Element {
    let label = Element::text(text, theme.typography.body, theme.palette.text);
    tool_frame(theme, vec![label], on).on_tap(cx, on_tap)
}

/// A thin vertical line between groups of toolbar buttons. Reported to
/// assistive technologies as a [`Role::Separator`].
pub fn tool_separator(theme: &Theme) -> Element {
    Element::boxed(BoxStyle {
        fill: Some(theme.palette.border),
        radius: 0.0,
        border: None,
    })
    .width(1.0)
    .height(ICON + 2.0 * theme.spacing.xs)
    .role(Role::Separator)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::geometry::Size;

    #[derive(Default)]
    struct State {
        bold: bool,
        saved: u32,
    }

    #[test]
    fn toolbar_buttons_run_and_report_their_roles() {
        let svg = std::sync::Arc::new(
            crate::render::Svg::parse(
                br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16"/></svg>"#,
            )
            .unwrap(),
        );
        let mut app = App::new(State::default(), move |s: &State, cx: &mut Cx<State>| {
            let theme = *cx.theme();
            let bar = toolbar(
                &theme,
                vec![
                    tool_button(cx, &theme, "Save", |s: &mut State| s.saved += 1),
                    tool_separator(&theme),
                    tool_toggle(cx, &theme, "B", s.bold, |s: &mut State| s.bold = !s.bold),
                    tool_icon_button(cx, &theme, &svg, "Delete", |_: &mut State| {}),
                ],
            );
            super::super::column(vec![bar])
        })
        .logical_size(Size::new(400.0, 60.0));
        app.render_once();
        let tree = app.accessibility_tree().unwrap();
        let bar = tree
            .descendants()
            .into_iter()
            .find(|n| n.role == Role::Toolbar)
            .unwrap()
            .clone();
        let roles: Vec<Role> = bar.children.iter().map(|c| c.role).collect();
        assert_eq!(
            roles,
            [Role::Button, Role::Separator, Role::Button, Role::Button]
        );
        assert_eq!(bar.children[3].name, "Delete", "the icon button is named");
        assert!(app.click_at(bar.children[0].bounds.center()));
        assert!(app.click_at(bar.children[2].bounds.center()));
        assert_eq!((app.state().saved, app.state().bold), (1, true));
    }
}
