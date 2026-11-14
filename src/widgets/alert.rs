//! The message box Stipple draws where the platform has no alert (see
//! [`AppHandle::message`]): a modal panel over a scrim with an icon, the
//! message, an optional text field, and the buttons.

use crate::core::{AlertInput, AlertView};
use crate::core::{
    Align, Anchor, AppHandle, Axis, Cx, Element, FocusId, KeyInput, OverlaySpec, Role,
};
use crate::geometry::Insets;
use crate::platform::dialog::MessageKind;
use crate::style::Theme;

use std::rc::Rc;

use super::password::password_field_with;
use super::{PasswordField, Variant, button_variant, placeholder};

/// The panel's width, in logical pixels.
const WIDTH: f64 = 400.0;

/// Draw the first message box waiting on `handle` as a modal overlay, if
/// there is one. Returns the focus that should be in it while it's up (its
/// text field, or else its default button).
pub(crate) fn overlay<S: 'static>(
    cx: &mut Cx<S>,
    theme: &Theme,
    handle: &AppHandle<S>,
) -> Option<FocusId> {
    let AlertView { id, mb, input } = handle.alert_view()?;
    let p = theme.palette;
    let labels = mb.button_labels();
    let default = mb.default_index();
    let cancel = mb.cancel_index();

    // Enter presses the default button and Escape the cancel one, wherever
    // focus is in the box.
    let keys = {
        let handle = handle.clone();
        move |s: &mut S, k: &KeyInput| match k {
            KeyInput::Enter => {
                handle.answer_alert(id, Some(default), s);
                true
            }
            KeyInput::Escape => {
                if cancel.is_some() {
                    handle.answer_alert(id, cancel, s);
                }
                true
            }
            _ => false,
        }
    };

    let (accent, glyph) = match mb.kind {
        MessageKind::Info => (p.primary, "i"),
        MessageKind::Question => (p.primary, "?"),
        MessageKind::Warning => (p.warning, "!"),
        MessageKind::Error => (p.danger, "×"),
    };
    let icon = Element::stack(
        Axis::Horizontal,
        vec![Element::text(
            glyph,
            theme.typography.title,
            accent.on_color(),
        )],
    )
    .fill(accent)
    .radius(16.0)
    .width(32.0)
    .height(32.0)
    .align(Align::Center, Align::Center);
    let mut words = vec![
        Element::text(mb.text.clone(), theme.typography.title, p.text)
            .wrap()
            .role(Role::Text),
    ];
    if !mb.detail.is_empty() {
        words.push(Element::text(mb.detail.clone(), theme.typography.body, p.text_muted).wrap());
    }
    let words = Element::stack(Axis::Vertical, words)
        .gap(theme.spacing.xs)
        .align(Align::Start, Align::Stretch)
        .grow(1.0)
        .shrink(1.0);
    let mut rows = vec![
        Element::stack(Axis::Horizontal, vec![icon, words])
            .gap(theme.spacing.md)
            .align(Align::Start, Align::Start),
    ];

    let mut target = None;
    if let Some(input) = &input {
        let h = handle.clone();
        let keys = keys.clone();
        let field = match input {
            AlertInput::Secure(pw) => {
                let k = keys.clone();
                password_field_with(
                    cx,
                    theme,
                    pw,
                    Rc::new(move |_: &mut S, f: &mut dyn FnMut(&mut PasswordField)| {
                        h.alert_input(id, |i| {
                            if let AlertInput::Secure(p) = i {
                                f(p);
                            }
                        })
                    }),
                    Some(Rc::new(move |s: &mut S, key: &KeyInput| k(s, key))),
                )
            }
            AlertInput::Plain(b) => {
                let shown = b.display();
                let text = Element::text(placeholder(shown.text), theme.font_size, p.text)
                    .grow(1.0)
                    .caret(shown.caret)
                    .selection(shown.selection)
                    .preedit(shown.preedit);
                let hp = h.clone();
                Element::stack(Axis::Horizontal, vec![text])
                    .fill(p.surface)
                    .radius(theme.radius)
                    .border(p.border, 1.0)
                    .padding(Insets::symmetric(theme.spacing.md, theme.spacing.sm))
                    .align(Align::Start, Align::Center)
                    .native_text_frame(theme)
                    .on_key(cx, move |s: &mut S, k| {
                        if !keys(s, k) {
                            h.alert_input(id, |i| {
                                if let AlertInput::Plain(b) = i {
                                    match k {
                                        KeyInput::SelectLine => b.select_all(),
                                        _ => b.apply(k),
                                    }
                                }
                            });
                        }
                    })
                    .on_text_pos(cx, move |_: &mut S, index, extend| {
                        hp.alert_input(id, |i| {
                            if let AlertInput::Plain(b) = i {
                                let index = b.display_to_text(index);
                                if extend {
                                    b.extend_to(index);
                                } else {
                                    b.place_caret(index);
                                }
                            }
                        });
                    })
            }
        };
        let field = field.width(WIDTH - 2.0 * theme.spacing.lg);
        target = field.focus;
        rows.push(field);
    }

    let buttons = labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let variant = if i == default {
                Variant::Primary
            } else {
                Variant::Secondary
            };
            let h = handle.clone();
            let mut b = button_variant(theme, label.clone(), variant)
                .on_tap(cx, move |s: &mut S| h.answer_alert(id, Some(i), s));
            if i == default {
                let keys = keys.clone();
                b = b.on_key(cx, move |s: &mut S, k| {
                    keys(s, k);
                });
                target = target.or(b.focus);
            }
            b
        })
        .collect();
    rows.push(
        Element::stack(Axis::Horizontal, buttons)
            .gap(theme.spacing.sm)
            .align(Align::End, Align::Center),
    );

    let content = Element::stack(Axis::Vertical, rows)
        .gap(theme.spacing.lg)
        .padding(Insets::uniform(theme.spacing.lg))
        .fill(p.surface)
        .border(p.border, 1.0)
        .radius(theme.radius * 2.0)
        .align(Align::Start, Align::Stretch)
        .width(WIDTH)
        .role(Role::Dialog)
        .a11y_label(if mb.title.is_empty() {
            mb.text.clone()
        } else {
            mb.title.clone()
        });
    // It pops in on the theme's dialog motion and fades back out when it's
    // answered, its scrim with it. Keyed by the box, so the next one waiting
    // comes in as this one leaves.
    let content = content
        .key(("stipple-message-box", id))
        .enter(super::motion::dialog_effect(&theme.motion))
        .exit(super::motion::dialog_exit(&theme.motion));
    cx.overlay(OverlaySpec {
        content,
        anchor: Anchor::Center,
        modal: true,
        dismiss: None,
    });
    target
}
