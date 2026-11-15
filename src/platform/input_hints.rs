//! What a text field tells the on-screen keyboard: which keyboard to show,
//! whether to capitalize and correct what is typed, what the action key
//! says, and what autofill may offer. Set on a field with
//! [`Element::input_kind`](crate::core::Element::input_kind),
//! [`autocapitalize`](crate::core::Element::autocapitalize),
//! [`autocorrect`](crate::core::Element::autocorrect),
//! [`enter_action`](crate::core::Element::enter_action) and
//! [`autofill`](crate::core::Element::autofill).
//!
//! The tables mapping the hints onto each platform's terms (Android's
//! `EditorInfo`, iOS's `UITextInputTraits`, the web's attributes) are here,
//! free of the platforms, so they are tested on every host.

/// The kind of text a field takes, which picks the on-screen keyboard (iOS,
/// Android and the web; desktops ignore it). See
/// [`Element::input_kind`](crate::core::Element::input_kind).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InputKind {
    /// Any text: the full keyboard, capitalizing sentences and correcting
    /// words unless told otherwise.
    #[default]
    Text,
    /// Digits only (a barcode, a PIN, a postal code): a number pad.
    Number,
    /// A number with a decimal separator: a number pad with one.
    Decimal,
    /// A phone number: a phone pad (digits, `+`, `*`, `#`).
    Phone,
    /// An e-mail address: `@` and `.` on the keyboard, nothing capitalized
    /// or corrected.
    Email,
    /// A URL: `/` and `.com` on the keyboard, nothing capitalized or
    /// corrected.
    Url,
    /// Search terms: the full keyboard, whose action key says Search.
    Search,
}

/// Which letters the keyboard capitalizes on its own. See
/// [`Element::autocapitalize`](crate::core::Element::autocapitalize).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Autocapitalize {
    /// None.
    Off,
    /// Every letter.
    Characters,
    /// The first letter of each word.
    Words,
    /// The first letter of each sentence.
    Sentences,
}

/// What the keyboard's action key (Return, Enter) says and does. See
/// [`Element::enter_action`](crate::core::Element::enter_action).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EnterAction {
    /// "Done": submits the field (its
    /// [`on_submit`](crate::core::Element::on_submit) handlers run).
    Done,
    /// "Go": submits the field.
    Go,
    /// "Next": submits the field, then moves focus to the next one, as Tab
    /// does (on every platform, Enter included). In a text area it moves
    /// focus without a new line.
    Next,
    /// "Search": submits the field.
    Search,
    /// "Send": submits the field.
    Send,
}

/// What a field holds, for the system's autofill to offer (a saved
/// password, the one-time code from a text message). iOS
/// (`textContentType`) and the web (`autocomplete`); see
/// [`Element::autofill`](crate::core::Element::autofill).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Autofill {
    /// A user name or account name.
    Username,
    /// The user's current password.
    Password,
    /// A new password (sign-up, password change): the system may suggest one.
    NewPassword,
    /// A one-time code, as sent by text message.
    OneTimeCode,
    /// An e-mail address.
    Email,
    /// A phone number.
    Phone,
    /// A person's full name.
    Name,
    /// A postal code.
    PostalCode,
    /// A street address.
    StreetAddress,
    /// A URL.
    Url,
}

/// A text field's hints, as the App hands them to the window for the
/// focused field (see `Window::set_input_hints`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct InputHints {
    pub(crate) kind: InputKind,
    /// `None`: the kind's default.
    pub(crate) autocapitalize: Option<Autocapitalize>,
    /// `None`: the kind's default.
    pub(crate) autocorrect: Option<bool>,
    /// `None`: the platform's default (Done on Android's single-line
    /// fields, a newline in multi-line ones).
    pub(crate) action: Option<EnterAction>,
    pub(crate) autofill: Option<Autofill>,
    /// A multi-line field (a text area): Enter is a new line unless an
    /// action says otherwise.
    pub(crate) multiline: bool,
    /// A password field: shown masked, and the keyboard must not learn it.
    pub(crate) secure: bool,
}

// The platform tables read these; desktops have none.
#[cfg_attr(
    not(any(test, target_os = "android", target_os = "ios", target_arch = "wasm32")),
    allow(dead_code)
)]
impl InputHints {
    /// The capitalization in effect: the one set, else sentences for plain
    /// text and none for the rest (and for passwords).
    pub(crate) fn capitalization(&self) -> Autocapitalize {
        match self.autocapitalize {
            Some(c) => c,
            None if self.kind == InputKind::Text && !self.secure => Autocapitalize::Sentences,
            None => Autocapitalize::Off,
        }
    }

    /// Whether the keyboard corrects and suggests: as set, else for plain
    /// text and search terms (never for passwords).
    pub(crate) fn corrects(&self) -> bool {
        !self.secure
            && self
                .autocorrect
                .unwrap_or(matches!(self.kind, InputKind::Text | InputKind::Search))
    }

    /// The action key in effect: the one set, else Search for search terms.
    /// A multi-line field's Enter is a new line: only Next replaces it.
    pub(crate) fn enter(&self) -> Option<EnterAction> {
        if self.multiline {
            return self.action.filter(|&a| a == EnterAction::Next);
        }
        self.action
            .or((self.kind == InputKind::Search).then_some(EnterAction::Search))
    }
}

/// Android's `EditorInfo.inputType` and `imeOptions` for `hints`.
#[cfg(any(test, target_os = "android"))]
pub(crate) mod android {
    use super::{Autocapitalize, EnterAction, InputHints, InputKind};

    pub(crate) const TYPE_CLASS_TEXT: i32 = 0x1;
    pub(crate) const TYPE_CLASS_NUMBER: i32 = 0x2;
    pub(crate) const TYPE_CLASS_PHONE: i32 = 0x3;
    pub(crate) const TYPE_NUMBER_FLAG_DECIMAL: i32 = 0x2000;
    pub(crate) const TYPE_NUMBER_VARIATION_PASSWORD: i32 = 0x10;
    pub(crate) const TYPE_TEXT_VARIATION_URI: i32 = 0x10;
    pub(crate) const TYPE_TEXT_VARIATION_EMAIL_ADDRESS: i32 = 0x20;
    pub(crate) const TYPE_TEXT_VARIATION_PASSWORD: i32 = 0x80;
    pub(crate) const TYPE_TEXT_FLAG_CAP_CHARACTERS: i32 = 0x1000;
    pub(crate) const TYPE_TEXT_FLAG_CAP_WORDS: i32 = 0x2000;
    pub(crate) const TYPE_TEXT_FLAG_CAP_SENTENCES: i32 = 0x4000;
    pub(crate) const TYPE_TEXT_FLAG_AUTO_CORRECT: i32 = 0x8000;
    pub(crate) const TYPE_TEXT_FLAG_MULTI_LINE: i32 = 0x2_0000;
    pub(crate) const TYPE_TEXT_FLAG_NO_SUGGESTIONS: i32 = 0x8_0000;

    pub(crate) const IME_ACTION_NONE: i32 = 1;
    pub(crate) const IME_ACTION_GO: i32 = 2;
    pub(crate) const IME_ACTION_SEARCH: i32 = 3;
    pub(crate) const IME_ACTION_SEND: i32 = 4;
    pub(crate) const IME_ACTION_NEXT: i32 = 5;
    pub(crate) const IME_ACTION_DONE: i32 = 6;
    pub(crate) const IME_FLAG_NO_PERSONALIZED_LEARNING: i32 = 0x0100_0000;
    pub(crate) const IME_FLAG_NO_FULLSCREEN: i32 = 0x0200_0000;
    pub(crate) const IME_FLAG_NO_EXTRACT_UI: i32 = 0x1000_0000;
    pub(crate) const IME_FLAG_NO_ENTER_ACTION: i32 = 0x4000_0000;

    /// `EditorInfo.inputType`: the class (text, number, phone), its
    /// variation and flags.
    pub(crate) fn input_type(h: &InputHints) -> i32 {
        let text = |variation: i32| {
            let mut t = TYPE_CLASS_TEXT | variation;
            t |= match h.capitalization() {
                Autocapitalize::Off => 0,
                Autocapitalize::Characters => TYPE_TEXT_FLAG_CAP_CHARACTERS,
                Autocapitalize::Words => TYPE_TEXT_FLAG_CAP_WORDS,
                Autocapitalize::Sentences => TYPE_TEXT_FLAG_CAP_SENTENCES,
            };
            t |= if h.corrects() {
                TYPE_TEXT_FLAG_AUTO_CORRECT
            } else {
                TYPE_TEXT_FLAG_NO_SUGGESTIONS
            };
            if h.multiline {
                t |= TYPE_TEXT_FLAG_MULTI_LINE;
            }
            t
        };
        let password = if h.secure {
            TYPE_TEXT_VARIATION_PASSWORD
        } else {
            0
        };
        match h.kind {
            InputKind::Number | InputKind::Decimal => {
                let mut t = TYPE_CLASS_NUMBER;
                if h.kind == InputKind::Decimal {
                    t |= TYPE_NUMBER_FLAG_DECIMAL;
                }
                if h.secure {
                    t |= TYPE_NUMBER_VARIATION_PASSWORD;
                }
                t
            }
            // A phone pad has no masked variation: a secret phone number
            // takes the masked text keyboard.
            InputKind::Phone if !h.secure => TYPE_CLASS_PHONE,
            InputKind::Email if !h.secure => text(TYPE_TEXT_VARIATION_EMAIL_ADDRESS),
            InputKind::Url if !h.secure => text(TYPE_TEXT_VARIATION_URI),
            _ => text(password),
        }
    }

    /// `EditorInfo.imeOptions`: the action key, and no fullscreen editor.
    pub(crate) fn ime_options(h: &InputHints) -> i32 {
        let mut o = IME_FLAG_NO_FULLSCREEN | IME_FLAG_NO_EXTRACT_UI;
        o |= match h.enter() {
            Some(EnterAction::Done) => IME_ACTION_DONE,
            Some(EnterAction::Go) => IME_ACTION_GO,
            Some(EnterAction::Next) => IME_ACTION_NEXT,
            Some(EnterAction::Search) => IME_ACTION_SEARCH,
            Some(EnterAction::Send) => IME_ACTION_SEND,
            // A text area's Enter is a new line.
            None if h.multiline => IME_ACTION_NONE | IME_FLAG_NO_ENTER_ACTION,
            None => IME_ACTION_DONE,
        };
        if h.secure {
            o |= IME_FLAG_NO_PERSONALIZED_LEARNING;
        }
        o
    }
}

/// iOS's `UITextInputTraits` for `hints`.
#[cfg(any(test, target_os = "ios"))]
pub(crate) mod ios {
    use super::{Autocapitalize, Autofill, EnterAction, InputHints, InputKind};

    /// The traits, as the values of UIKit's enums.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct Traits {
        /// `UIKeyboardType`.
        pub(crate) keyboard: isize,
        /// `UITextAutocapitalizationType`.
        pub(crate) autocapitalization: isize,
        /// `UITextAutocorrectionType` (and `UITextSpellCheckingType`, which
        /// shares its values).
        pub(crate) autocorrection: isize,
        /// `UIReturnKeyType`.
        pub(crate) return_key: isize,
        pub(crate) secure: bool,
        pub(crate) content: Option<Autofill>,
    }

    pub(crate) const KEYBOARD_DEFAULT: isize = 0;
    pub(crate) const KEYBOARD_URL: isize = 3;
    pub(crate) const KEYBOARD_NUMBER_PAD: isize = 4;
    pub(crate) const KEYBOARD_PHONE_PAD: isize = 5;
    pub(crate) const KEYBOARD_EMAIL: isize = 7;
    pub(crate) const KEYBOARD_DECIMAL_PAD: isize = 8;
    pub(crate) const KEYBOARD_WEB_SEARCH: isize = 10;
    pub(crate) const CORRECTION_NO: isize = 1;
    pub(crate) const CORRECTION_YES: isize = 2;
    pub(crate) const RETURN_DEFAULT: isize = 0;
    pub(crate) const RETURN_GO: isize = 1;
    pub(crate) const RETURN_NEXT: isize = 4;
    pub(crate) const RETURN_SEARCH: isize = 6;
    pub(crate) const RETURN_SEND: isize = 7;
    pub(crate) const RETURN_DONE: isize = 9;

    pub(crate) fn traits(h: &InputHints) -> Traits {
        Traits {
            keyboard: match h.kind {
                InputKind::Text => KEYBOARD_DEFAULT,
                InputKind::Number => KEYBOARD_NUMBER_PAD,
                InputKind::Decimal => KEYBOARD_DECIMAL_PAD,
                InputKind::Phone => KEYBOARD_PHONE_PAD,
                InputKind::Email => KEYBOARD_EMAIL,
                InputKind::Url => KEYBOARD_URL,
                InputKind::Search => KEYBOARD_WEB_SEARCH,
            },
            autocapitalization: match h.capitalization() {
                Autocapitalize::Off => 0,
                Autocapitalize::Words => 1,
                Autocapitalize::Sentences => 2,
                Autocapitalize::Characters => 3,
            },
            autocorrection: if h.corrects() {
                CORRECTION_YES
            } else {
                CORRECTION_NO
            },
            return_key: match h.enter() {
                Some(EnterAction::Done) => RETURN_DONE,
                Some(EnterAction::Go) => RETURN_GO,
                Some(EnterAction::Next) => RETURN_NEXT,
                Some(EnterAction::Search) => RETURN_SEARCH,
                Some(EnterAction::Send) => RETURN_SEND,
                None => RETURN_DEFAULT,
            },
            secure: h.secure,
            content: h.autofill,
        }
    }
}

/// The web's attributes on the hidden `<textarea>` for `hints`, as
/// `name=value` pairs joined by `;`.
#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) fn web_attributes(h: &InputHints) -> String {
    let mode = match h.kind {
        InputKind::Text => "text",
        InputKind::Number => "numeric",
        InputKind::Decimal => "decimal",
        InputKind::Phone => "tel",
        InputKind::Email => "email",
        InputKind::Url => "url",
        InputKind::Search => "search",
    };
    let enter = match h.enter() {
        Some(EnterAction::Done) => "done",
        Some(EnterAction::Go) => "go",
        Some(EnterAction::Next) => "next",
        Some(EnterAction::Search) => "search",
        Some(EnterAction::Send) => "send",
        None if h.multiline => "enter",
        None => "",
    };
    let caps = match h.capitalization() {
        Autocapitalize::Off => "off",
        Autocapitalize::Characters => "characters",
        Autocapitalize::Words => "words",
        Autocapitalize::Sentences => "sentences",
    };
    let correct = if h.corrects() { "on" } else { "off" };
    let spell = if h.corrects() { "true" } else { "false" };
    let complete = match h.autofill {
        Some(Autofill::Username) => "username",
        Some(Autofill::Password) => "current-password",
        Some(Autofill::NewPassword) => "new-password",
        Some(Autofill::OneTimeCode) => "one-time-code",
        Some(Autofill::Email) => "email",
        Some(Autofill::Phone) => "tel",
        Some(Autofill::Name) => "name",
        Some(Autofill::PostalCode) => "postal-code",
        Some(Autofill::StreetAddress) => "street-address",
        Some(Autofill::Url) => "url",
        None => "off",
    };
    format!(
        "inputmode={mode};enterkeyhint={enter};autocapitalize={caps};autocorrect={correct};\
         spellcheck={spell};autocomplete={complete}"
    )
}

#[cfg(test)]
mod tests {
    use super::android::*;
    use super::*;

    fn hints(kind: InputKind) -> InputHints {
        InputHints {
            kind,
            ..InputHints::default()
        }
    }

    #[test]
    fn defaults_follow_the_kind() {
        let text = hints(InputKind::Text);
        assert_eq!(text.capitalization(), Autocapitalize::Sentences);
        assert!(text.corrects());
        assert_eq!(text.enter(), None);
        for kind in [
            InputKind::Number,
            InputKind::Decimal,
            InputKind::Phone,
            InputKind::Email,
            InputKind::Url,
        ] {
            let h = hints(kind);
            assert_eq!(h.capitalization(), Autocapitalize::Off, "{kind:?}");
            assert!(!h.corrects(), "{kind:?}");
        }
        assert_eq!(hints(InputKind::Search).enter(), Some(EnterAction::Search));
        // A password is never corrected or capitalized, whatever was asked.
        let secret = InputHints {
            secure: true,
            autocorrect: Some(true),
            ..text
        };
        assert!(!secret.corrects());
        assert_eq!(secret.capitalization(), Autocapitalize::Off);
    }

    #[test]
    fn android_input_types() {
        let t = |h: InputHints| input_type(&h);
        assert_eq!(
            t(hints(InputKind::Text)),
            TYPE_CLASS_TEXT | TYPE_TEXT_FLAG_CAP_SENTENCES | TYPE_TEXT_FLAG_AUTO_CORRECT
        );
        assert_eq!(t(hints(InputKind::Number)), TYPE_CLASS_NUMBER);
        assert_eq!(t(hints(InputKind::Number)), 0x2);
        assert_eq!(
            t(hints(InputKind::Decimal)),
            TYPE_CLASS_NUMBER | TYPE_NUMBER_FLAG_DECIMAL
        );
        assert_eq!(t(hints(InputKind::Phone)), TYPE_CLASS_PHONE);
        assert_eq!(
            t(hints(InputKind::Email)),
            TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_EMAIL_ADDRESS | TYPE_TEXT_FLAG_NO_SUGGESTIONS
        );
        assert_eq!(
            t(hints(InputKind::Url)),
            TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_URI | TYPE_TEXT_FLAG_NO_SUGGESTIONS
        );
        // A barcode payload: no capitals, no corrections.
        let code = InputHints {
            autocapitalize: Some(Autocapitalize::Characters),
            autocorrect: Some(false),
            ..hints(InputKind::Text)
        };
        assert_eq!(
            t(code),
            TYPE_CLASS_TEXT | TYPE_TEXT_FLAG_CAP_CHARACTERS | TYPE_TEXT_FLAG_NO_SUGGESTIONS
        );
        // Passwords, and a PIN.
        let password = InputHints {
            secure: true,
            ..hints(InputKind::Text)
        };
        assert_eq!(
            t(password),
            TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_PASSWORD | TYPE_TEXT_FLAG_NO_SUGGESTIONS
        );
        let pin = InputHints {
            secure: true,
            ..hints(InputKind::Number)
        };
        assert_eq!(t(pin), TYPE_CLASS_NUMBER | TYPE_NUMBER_VARIATION_PASSWORD);
        // A text area.
        let area = InputHints {
            multiline: true,
            ..hints(InputKind::Text)
        };
        assert_ne!(t(area) & TYPE_TEXT_FLAG_MULTI_LINE, 0);
    }

    #[test]
    fn android_action_keys() {
        let o = |h: InputHints| ime_options(&h) & 0xff;
        assert_eq!(o(hints(InputKind::Text)), IME_ACTION_DONE);
        assert_eq!(o(hints(InputKind::Search)), IME_ACTION_SEARCH);
        for (action, code) in [
            (EnterAction::Done, IME_ACTION_DONE),
            (EnterAction::Go, IME_ACTION_GO),
            (EnterAction::Next, IME_ACTION_NEXT),
            (EnterAction::Search, IME_ACTION_SEARCH),
            (EnterAction::Send, IME_ACTION_SEND),
        ] {
            let h = InputHints {
                action: Some(action),
                ..hints(InputKind::Number)
            };
            assert_eq!(o(h), code, "{action:?}");
            assert_eq!(ime_options(&h) & IME_FLAG_NO_ENTER_ACTION, 0);
        }
        let area = InputHints {
            multiline: true,
            ..hints(InputKind::Text)
        };
        assert_eq!(o(area), IME_ACTION_NONE);
        assert_ne!(ime_options(&area) & IME_FLAG_NO_ENTER_ACTION, 0);
        assert_ne!(ime_options(&area) & IME_FLAG_NO_FULLSCREEN, 0);
        // A text area's Enter stays a new line, unless it moves on.
        let done = InputHints {
            action: Some(EnterAction::Done),
            ..area
        };
        assert_eq!(o(done), IME_ACTION_NONE);
        let next = InputHints {
            action: Some(EnterAction::Next),
            ..area
        };
        assert_eq!(o(next), IME_ACTION_NEXT);
    }

    #[test]
    fn ios_traits() {
        use super::ios::*;
        let number = traits(&InputHints {
            action: Some(EnterAction::Done),
            ..hints(InputKind::Number)
        });
        assert_eq!(number.keyboard, KEYBOARD_NUMBER_PAD);
        assert_eq!(number.autocapitalization, 0);
        assert_eq!(number.autocorrection, CORRECTION_NO);
        assert_eq!(number.return_key, RETURN_DONE);
        let text = traits(&hints(InputKind::Text));
        assert_eq!(
            (text.keyboard, text.autocapitalization, text.autocorrection),
            (KEYBOARD_DEFAULT, 2, CORRECTION_YES)
        );
        assert_eq!(text.return_key, RETURN_DEFAULT);
        assert_eq!(traits(&hints(InputKind::Email)).keyboard, KEYBOARD_EMAIL);
        assert_eq!(traits(&hints(InputKind::Url)).keyboard, KEYBOARD_URL);
        assert_eq!(
            traits(&hints(InputKind::Phone)).keyboard,
            KEYBOARD_PHONE_PAD
        );
        assert_eq!(
            traits(&hints(InputKind::Decimal)).keyboard,
            KEYBOARD_DECIMAL_PAD
        );
        let search = traits(&hints(InputKind::Search));
        assert_eq!(
            (search.keyboard, search.return_key),
            (KEYBOARD_WEB_SEARCH, RETURN_SEARCH)
        );
        let words = traits(&InputHints {
            autocapitalize: Some(Autocapitalize::Words),
            action: Some(EnterAction::Next),
            ..hints(InputKind::Text)
        });
        assert_eq!(
            (words.autocapitalization, words.return_key),
            (1, RETURN_NEXT)
        );
        let code = traits(&InputHints {
            autofill: Some(Autofill::OneTimeCode),
            secure: true,
            ..hints(InputKind::Number)
        });
        assert!(code.secure);
        assert_eq!(code.content, Some(Autofill::OneTimeCode));
    }

    #[test]
    fn web_attribute_strings() {
        assert_eq!(
            web_attributes(&InputHints {
                action: Some(EnterAction::Done),
                ..hints(InputKind::Number)
            }),
            "inputmode=numeric;enterkeyhint=done;autocapitalize=off;autocorrect=off;\
             spellcheck=false;autocomplete=off"
        );
        assert_eq!(
            web_attributes(&hints(InputKind::Text)),
            "inputmode=text;enterkeyhint=;autocapitalize=sentences;autocorrect=on;\
             spellcheck=true;autocomplete=off"
        );
        let area = web_attributes(&InputHints {
            multiline: true,
            autofill: Some(Autofill::StreetAddress),
            ..hints(InputKind::Text)
        });
        assert!(area.contains("enterkeyhint=enter"), "{area}");
        assert!(area.contains("autocomplete=street-address"), "{area}");
        assert!(web_attributes(&hints(InputKind::Email)).starts_with("inputmode=email;"));
        assert!(web_attributes(&hints(InputKind::Phone)).starts_with("inputmode=tel;"));
        assert!(web_attributes(&hints(InputKind::Url)).starts_with("inputmode=url;"));
        assert!(web_attributes(&hints(InputKind::Search)).contains("enterkeyhint=search"));
        assert!(web_attributes(&hints(InputKind::Decimal)).starts_with("inputmode=decimal;"));
    }
}
