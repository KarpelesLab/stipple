//! A toolbar: a row of compact command buttons, grouped by separators.

use crate::core::{Align, Axis, BoxStyle, Cx, Element, Role};
use crate::geometry::Insets;
use crate::render::ImageSource;

/// Icons in a [`ToolButton::from_icon`] are drawn at this size, in logical
/// pixels.
const ICON: f64 = 18.0;

/// A toolbar: a row on a surface strip, its items vertically centered. Add
/// them with [`children`](Element::children): [`tool_button`]s (and
/// [`ToolButton`]s for icons and toggles) and [`tool_separator`]s; any other
/// element works too (a [`select`], a [`spacer`] to push the rest to the
/// end). Items that don't fit overflow.
///
/// Self-drawn under every theme, in the theme's colors. Reported to assistive
/// technologies as a [`Role::Toolbar`] holding its buttons.
///
/// [`select`]: super::select
/// [`spacer`]: super::spacer
pub fn toolbar<S>(cx: &Cx<S>) -> Element {
    let theme = cx.theme();
    Element::stack(Axis::Horizontal, Vec::new())
        .fill(theme.palette.surface)
        .border(theme.palette.border, 1.0)
        .radius(theme.radius)
        .padding(Insets::uniform(theme.spacing.xs))
        .gap(theme.spacing.xs)
        .role(Role::Toolbar)
}

/// A compact toolbar button: a text label or an icon, with no fill (the
/// hover highlight shows it's clickable), tinted while
/// [`selected`](Self::selected). [`tool_button`] is the common case, a text
/// one; finish a `ToolButton` with [`view`](Self::view), and make it do
/// something with [`Element::on_tap`].
///
/// ```
/// use stipple::prelude::*;
/// struct Doc { bold: bool }
/// fn bar(doc: &Doc, cx: &mut Cx<Doc>) -> Element {
///     toolbar(cx).children(vec![
///         tool_button(cx, "Save").on_tap(cx, |_| {}),
///         tool_separator(cx),
///         ToolButton::new("B").selected(doc.bold).view(cx).on_tap(cx, |d| d.bold = !d.bold),
///     ])
/// }
/// # let theme = Theme::light();
/// # let _ = bar(&Doc { bold: true }, &mut Cx::new(&theme));
/// ```
#[derive(Clone, Debug)]
pub struct ToolButton {
    content: ToolContent,
    selected: bool,
}

#[derive(Clone, Debug)]
enum ToolContent {
    Text(String),
    Icon(ImageSource, String),
}

impl ToolButton {
    /// A tool button labeled `text`.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            content: ToolContent::Text(text.into()),
            selected: false,
        }
    }

    /// A tool button showing `icon` (a [`Pixmap`] or [`Svg`], drawn at
    /// 18×18). `name` is what assistive technologies read out, since the icon
    /// has no text.
    ///
    /// [`Pixmap`]: crate::render::Pixmap
    /// [`Svg`]: crate::render::Svg
    pub fn from_icon(icon: impl Into<ImageSource>, name: impl Into<String>) -> Self {
        Self {
            content: ToolContent::Icon(icon.into(), name.into()),
            selected: false,
        }
    }

    /// Show the button tinted while `selected`: a toggle that is on (bold, a
    /// view mode).
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// The button, in `cx`'s theme.
    pub fn view<S>(self, cx: &Cx<S>) -> Element {
        let theme = cx.theme();
        let (child, name) = match self.content {
            ToolContent::Text(text) => (
                Element::text(text, theme.typography.body, theme.palette.text),
                None,
            ),
            ToolContent::Icon(icon, name) => {
                (Element::image(icon).width(ICON).height(ICON), Some(name))
            }
        };
        let mut el = Element::stack(Axis::Horizontal, vec![child])
            .radius(theme.radius / 2.0)
            .padding(Insets::symmetric(theme.spacing.sm, theme.spacing.xs))
            .gap(theme.spacing.xs)
            .align(Align::Center, Align::Center)
            .min_height(ICON + 2.0 * theme.spacing.xs)
            .min_width(ICON + 2.0 * theme.spacing.sm);
        if self.selected {
            el = el
                .fill(theme.palette.selection)
                .border(theme.palette.primary, 1.0);
        }
        match name {
            Some(name) => el.a11y_label(name),
            None => el,
        }
    }
}

/// A toolbar button labeled `text`. For an icon or a toggle, build a
/// [`ToolButton`].
pub fn tool_button<S>(cx: &Cx<S>, text: impl Into<String>) -> Element {
    ToolButton::new(text).view(cx)
}

/// A thin vertical line between groups of toolbar buttons. Reported to
/// assistive technologies as a [`Role::Separator`].
pub fn tool_separator<S>(cx: &Cx<S>) -> Element {
    let theme = cx.theme();
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
            let bar = toolbar(cx).children(vec![
                tool_button(cx, "Save").on_tap(cx, |s| s.saved += 1),
                tool_separator(cx),
                ToolButton::new("B")
                    .selected(s.bold)
                    .view(cx)
                    .on_tap(cx, |s| s.bold = !s.bold),
                ToolButton::from_icon(&svg, "Delete")
                    .view(cx)
                    .on_tap(cx, |_| {}),
            ]);
            super::super::column(vec![bar])
        })
        .no_font()
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
