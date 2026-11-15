//! The user's language in the [`App`]: the languages the app ships
//! ([`App::languages`]), the layout direction that follows the one shown,
//! and [`App::on_locale_change`]. See `docs/localization.md`.

use crate::App;
use crate::layout::LayoutDirection;
use crate::locale::Locale;

type LocaleFn<S> = Box<dyn FnMut(&mut S, &Locale)>;

/// What the App keeps of the user's locale.
pub(crate) struct LocaleState<S> {
    /// The languages the app ships, if it said.
    shipped: Vec<String>,
    hook: Option<LocaleFn<S>>,
    /// The direction of the language shown, once worked out.
    direction: Option<LayoutDirection>,
    /// The languages and locale last seen, to tell a change from a re-read.
    seen: Option<(Vec<String>, Locale)>,
}

impl<S> Default for LocaleState<S> {
    fn default() -> Self {
        Self {
            shipped: Vec::new(),
            hook: None,
            direction: None,
            seen: None,
        }
    }
}

impl<S: 'static> App<S> {
    /// The languages the app's strings are translated into (BCP 47 tags,
    /// the app's own language first: `["en", "fr", "ja"]`). The layout
    /// direction then follows the one the user gets
    /// ([`locale::negotiate`](crate::locale::negotiate) of their preferred
    /// languages against these, else the first): a Hebrew speaker sees an
    /// app shipping Hebrew mirrored, and one shipping only English left to
    /// right.
    ///
    /// Without this, the direction follows the user's first preferred
    /// language. Either way a theme with its own
    /// [`layout_direction`](crate::style::Theme::layout_direction) keeps it.
    ///
    /// ```no_run
    /// use stipple::prelude::*;
    /// # struct State;
    /// # fn view(_: &State, _: &mut Cx<State>) -> Element { spacer() }
    /// App::new(State, view).languages(["en", "fr", "he"]).run();
    /// ```
    pub fn languages<I, T>(mut self, languages: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        self.locale.shipped = languages.into_iter().map(Into::into).collect();
        self.locale.direction = None;
        self.refresh_theme();
        self
    }

    /// Call `hook` with the user's new [`Locale`] when their languages or
    /// formatting settings change while the app runs (a new region, the
    /// 24-hour clock switched, or the app's own
    /// [`set_preferred_languages`](crate::platform::set_preferred_languages)),
    /// to load another catalog, say. The views are built again afterwards
    /// either way, so a view that reads
    /// [`platform::locale`](crate::platform::locale) needs no hook.
    pub fn on_locale_change(mut self, hook: impl FnMut(&mut S, &Locale) + 'static) -> Self {
        self.locale.hook = Some(Box::new(hook));
        self
    }

    /// The layout direction of the language shown.
    pub(crate) fn ui_direction(&mut self) -> LayoutDirection {
        if let Some(dir) = self.locale.direction {
            return dir;
        }
        let preferred = crate::platform::preferred_languages();
        let shipped: Vec<&str> = self.locale.shipped.iter().map(String::as_str).collect();
        let shown = if shipped.is_empty() {
            preferred.first().map(String::as_str)
        } else {
            crate::locale::negotiate(&preferred, &shipped).or(shipped.first().copied())
        };
        let dir = LayoutDirection::for_language(shown.unwrap_or("en"));
        self.locale.direction = Some(dir);
        // What a later change is told from.
        if self.locale.seen.is_none() {
            self.locale.seen = Some((preferred, crate::platform::locale()));
        }
        dir
    }

    /// The locale may have changed: if it did, work the direction out again,
    /// tell the hook, and mark every window for a rebuild. Returns whether it
    /// changed.
    pub(crate) fn locale_changed(&mut self) -> bool {
        let now = (
            crate::platform::preferred_languages(),
            crate::platform::locale(),
        );
        if self.locale.seen.as_ref() == Some(&now) {
            return false;
        }
        self.locale.seen = Some(now.clone());
        self.locale.direction = None;
        self.refresh_theme();
        if let Some(hook) = self.locale.hook.as_mut() {
            hook(&mut self.state, &now.1);
        }
        for pane in &mut self.panes {
            pane.dirty = true;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;

    fn view(_: &(), _: &mut Cx<()>) -> Element {
        spacer()
    }

    // Tests see the user preferring `en-US` (see `platform::locale`).
    #[test]
    fn the_direction_follows_the_language_shown() {
        let mut app = App::new((), view);
        app.refresh_theme();
        assert_eq!(app.theme.layout_direction, LayoutDirection::Ltr);
        // An app shipping only Hebrew shows Hebrew, mirrored.
        let app = App::new((), view).languages(["he"]);
        assert_eq!(app.theme.layout_direction, LayoutDirection::Rtl);
        // English is shipped too: the user gets English.
        let app = App::new((), view).languages(["he", "en"]);
        assert_eq!(app.theme.layout_direction, LayoutDirection::Ltr);
        // A direction the app set stays.
        let app = App::new((), view)
            .languages(["he"])
            .theme(Theme::light().layout_direction(LayoutDirection::Ltr));
        assert_eq!(app.theme.layout_direction, LayoutDirection::Ltr);
    }

    #[test]
    fn a_locale_read_again_unchanged_is_no_change() {
        let mut app = App::new((), view).on_locale_change(|_, _| panic!("no change"));
        // Working the direction out takes the baseline.
        app.refresh_theme();
        assert!(!app.locale_changed());
    }
}
