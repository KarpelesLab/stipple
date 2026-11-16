//! Dead keys on hardware keyboards, as Android's own text fields do them.
//!
//! A key's `KeyCharacterMap.get` value with the `COMBINING_ACCENT` bit set
//! is a dead key: the low bits are the accent (the spacing form,
//! `U+02CA ˊ` for an acute). Nothing is typed yet; the accent shows as a
//! preedit ([`Ime::Preedit`]) until the next key: a letter it combines with
//! (`KeyCharacterMap.getDeadChar`, which the backend asks, with
//! [`dead_char`] as the stand-in when the JVM can't answer) commits the
//! composed letter (`é`); Space or the same dead key again commits the
//! accent itself; a letter it doesn't combine with commits the accent and
//! then the letter, as `QwertyKeyListener` leaves them. Backspace and Escape
//! take the accent back; other named keys (arrows, Enter, Tab) and shortcuts
//! commit it and then do what they do. Modifier keys (Shift for `É`) leave
//! it pending.
//!
//! The keys a sequence takes, and their releases, don't go to the app as
//! raw keys (`Event::RawKey`), as on the other platforms. No JVM or NDK
//! here, so this is unit-tested on every host.

use crate::platform::event::Ime;

/// `KeyCharacterMap.COMBINING_ACCENT`: the bit marking a dead key's value.
pub(crate) const COMBINING_ACCENT: u32 = 0x8000_0000;
/// `KeyCharacterMap.COMBINING_ACCENT_MASK`: the accent in a dead key's value.
pub(crate) const COMBINING_ACCENT_MASK: u32 = 0x7fff_ffff;

/// A key press, as the dead-key state sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Press {
    /// A dead key: its accent (spacing form).
    Dead(char),
    /// A key that types this character.
    Char(char),
    /// Backspace or Escape.
    Cancel,
    /// Another named key (an arrow, Enter, Tab, …), or a shortcut.
    Other,
    /// A key that types nothing by itself (a modifier).
    Nothing,
}

impl Press {
    /// Classify a key from its `KeyCharacterMap.get` value (0 for none),
    /// whether it is a named key and which, and whether a shortcut modifier
    /// (Ctrl, Meta) is held.
    pub(crate) fn of(value: i32, named: Option<NamedKey>, shortcut: bool) -> Press {
        match named {
            Some(NamedKey::Cancel) => return Press::Cancel,
            Some(NamedKey::Other) => return Press::Other,
            None => {}
        }
        let v = value as u32;
        if v & COMBINING_ACCENT != 0 {
            return match char::from_u32(v & COMBINING_ACCENT_MASK) {
                Some(accent) if !shortcut => Press::Dead(accent),
                _ => Press::Other,
            };
        }
        match char::from_u32(v) {
            Some(c) if v != 0 && !c.is_control() => {
                if shortcut {
                    Press::Other
                } else {
                    Press::Char(c)
                }
            }
            _ => Press::Nothing,
        }
    }
}

/// The named keys the state tells apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NamedKey {
    /// Backspace or Escape: take the accent back.
    Cancel,
    /// Anything else named.
    Other,
}

/// What a key press did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// Composition updates for the app, in order.
    pub(crate) events: Vec<Ime>,
    /// Whether the key still goes on as usual (as a raw key, then a key or
    /// text). `false`: the sequence took it.
    pub(crate) pass: bool,
}

impl Outcome {
    fn pass() -> Self {
        Outcome {
            events: Vec::new(),
            pass: true,
        }
    }
    fn taken(events: Vec<Ime>) -> Self {
        Outcome {
            events,
            pass: false,
        }
    }
}

/// The dead-key state of one keyboard focus.
#[derive(Clone, Debug, Default)]
pub(crate) struct DeadKeys {
    /// The accent waiting for the next key.
    accent: Option<char>,
    /// Key codes whose presses the sequence took (their releases are taken
    /// too).
    taken: Vec<i32>,
}

fn preedit(accent: char) -> Ime {
    let text = accent.to_string();
    let end = text.len();
    Ime::Preedit {
        text,
        cursor: Some((end, end)),
    }
}

fn clear() -> Ime {
    Ime::Preedit {
        text: String::new(),
        cursor: None,
    }
}

impl DeadKeys {
    /// Whether an accent is waiting.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> Option<char> {
        self.accent
    }

    /// Key `code` was pressed. `compose` is `getDeadChar(accent, c)`: the
    /// letter `c` with the accent, or `None` if they don't combine.
    pub(crate) fn press(
        &mut self,
        code: i32,
        press: Press,
        compose: impl Fn(char, char) -> Option<char>,
    ) -> Outcome {
        let out = match (self.accent, press) {
            (None, Press::Dead(accent)) => {
                self.accent = Some(accent);
                Outcome::taken(vec![preedit(accent)])
            }
            (None, _) => Outcome::pass(),
            (Some(_), Press::Nothing) => Outcome::pass(),
            (Some(_), Press::Cancel) => {
                self.accent = None;
                Outcome::taken(vec![clear()])
            }
            (Some(accent), Press::Other) => {
                self.accent = None;
                Outcome {
                    events: vec![Ime::Commit(accent.to_string())],
                    pass: true,
                }
            }
            (Some(accent), Press::Dead(next)) => {
                if let Some(c) = compose(accent, next) {
                    // The same accent twice: the accent itself.
                    self.accent = None;
                    Outcome::taken(vec![Ime::Commit(c.to_string())])
                } else {
                    // Another accent: the first stays, the second waits.
                    self.accent = Some(next);
                    Outcome::taken(vec![Ime::Commit(accent.to_string()), preedit(next)])
                }
            }
            (Some(accent), Press::Char(c)) => {
                self.accent = None;
                let text = match compose(accent, c) {
                    Some(composed) => composed.to_string(),
                    None => format!("{accent}{c}"),
                };
                Outcome::taken(vec![Ime::Commit(text)])
            }
        };
        if !out.pass && !self.taken.contains(&code) {
            self.taken.push(code);
        }
        out
    }

    /// Key `code` was released. Returns whether the sequence took its
    /// press (so the release isn't passed on either).
    pub(crate) fn release(&mut self, code: i32) -> bool {
        match self.taken.iter().position(|&c| c == code) {
            Some(i) => {
                self.taken.remove(i);
                true
            }
            None => false,
        }
    }

    /// Focus left: drop a waiting accent. Returns the event clearing its
    /// preedit, if there was one.
    pub(crate) fn reset(&mut self) -> Option<Ime> {
        self.taken.clear();
        self.accent.take().map(|_| clear())
    }
}

/// The combining mark an Android accent stands for (`KeyCharacterMap`'s
/// `sAccentToCombining`, with its legacy ASCII entries and the Latin-1
/// acute).
pub(crate) fn combining_for(accent: char) -> Option<char> {
    Some(match accent {
        '\u{02CB}' | '`' => '\u{0300}',
        '\u{02CA}' | '\u{00B4}' => '\u{0301}',
        '\u{02C6}' | '^' => '\u{0302}',
        '\u{02DC}' | '~' => '\u{0303}',
        '\u{02C9}' | '\u{00AF}' => '\u{0304}',
        '\u{02D8}' => '\u{0306}',
        '\u{02D9}' => '\u{0307}',
        '\u{00A8}' => '\u{0308}',
        '\u{02DA}' => '\u{030A}',
        '\u{02DD}' => '\u{030B}',
        '\u{02C7}' => '\u{030C}',
        '\u{00B8}' => '\u{0327}',
        '\u{02DB}' => '\u{0328}',
        // Already combining marks.
        c @ '\u{0300}'..='\u{036F}' => c,
        _ => return None,
    })
}

/// Base letter → precomposed letter, per combining mark: pairs of
/// characters.
const COMPOSED: &[(char, &str)] = &[
    ('\u{0300}', "aàeèiìoòuùAÀEÈIÌOÒUÙnǹNǸwẁWẀyỳYỲ"),
    (
        '\u{0301}',
        "aáeéiíoóuúyýAÁEÉIÍOÓUÚYÝcćCĆlĺLĹnńNŃrŕRŔsśSŚzźZŹgǵGǴkḱKḰmḿMḾpṕPṔwẃWẂ",
    ),
    (
        '\u{0302}',
        "aâeêiîoôuûAÂEÊIÎOÔUÛcĉCĈgĝGĜhĥHĤjĵJĴsŝSŜwŵWŴyŷYŶzẑZẐ",
    ),
    ('\u{0303}', "aãoõnñAÃOÕNÑiĩIĨuũUŨeẽEẼyỹYỸvṽVṼ"),
    ('\u{0304}', "aāeēiīoōuūAĀEĒIĪOŌUŪ"),
    ('\u{0306}', "aăAĂeĕEĔgğGĞiĭIĬoŏOŎuŭUŬ"),
    ('\u{0307}', "cċCĊeėEĖgġGĠIİzżZŻ"),
    ('\u{0308}', "aäeëiïoöuüyÿAÄEËIÏOÖUÜYŸhḧHḦwẅWẄxẍXẌtẗ"),
    ('\u{030A}', "aåAÅuůUŮwẘyẙ"),
    ('\u{030B}', "oőOŐuűUŰ"),
    (
        '\u{030C}',
        "cčCČdďDĎeěEĚnňNŇrřRŘsšSŠtťTŤzžZŽaǎAǍiǐIǏoǒOǑuǔUǓgǧGǦkǩKǨjǰ",
    ),
    ('\u{0327}', "cçCÇgģGĢkķKĶlļLĻnņNŅrŗRŖsşSŞtţTŢ"),
    ('\u{0328}', "aąAĄeęEĘiįIĮoǫOǪuųUŲ"),
];

/// `KeyCharacterMap.getDeadChar(accent, c)`, for when the JVM can't be
/// asked: the accent itself for Space or the same accent again, the
/// precomposed letter where Latin has one, else `None`.
pub(crate) fn dead_char(accent: char, c: char) -> Option<char> {
    if c == accent || c == ' ' {
        return Some(accent);
    }
    let mark = combining_for(accent)?;
    let (_, pairs) = COMPOSED.iter().find(|(m, _)| *m == mark)?;
    let mut chars = pairs.chars();
    while let (Some(base), Some(composed)) = (chars.next(), chars.next()) {
        if base == c {
            return Some(composed);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACUTE: char = '\u{02CA}';
    const DIAERESIS: char = '\u{00A8}';
    const E: i32 = 33; // AKEYCODE_E
    const U: i32 = 49;
    const X: i32 = 52;
    const SHIFT: i32 = 59;
    const DEL: i32 = 67;
    const LEFT: i32 = 21;

    fn dead(accent: char) -> Press {
        Press::of((COMBINING_ACCENT | accent as u32) as i32, None, false)
    }

    fn ch(c: char) -> Press {
        Press::of(c as i32, None, false)
    }

    fn commit(s: &str) -> Ime {
        Ime::Commit(s.into())
    }

    #[test]
    fn classifies_key_character_map_values() {
        assert_eq!(dead(ACUTE), Press::Dead(ACUTE));
        assert_eq!(ch('e'), Press::Char('e'));
        assert_eq!(Press::of(0, None, false), Press::Nothing);
        assert_eq!(Press::of('e' as i32, None, true), Press::Other);
        assert_eq!(
            Press::of((COMBINING_ACCENT | ACUTE as u32) as i32, None, true),
            Press::Other
        );
        assert_eq!(Press::of(0, Some(NamedKey::Cancel), false), Press::Cancel);
        assert_eq!(Press::of(0, Some(NamedKey::Other), false), Press::Other);
    }

    #[test]
    fn accent_then_letter_composes() {
        let mut d = DeadKeys::default();
        let out = d.press(E, dead(ACUTE), dead_char);
        assert!(!out.pass);
        assert_eq!(
            out.events,
            vec![Ime::Preedit {
                text: "ˊ".into(),
                cursor: Some((2, 2))
            }]
        );
        assert_eq!(d.pending(), Some(ACUTE));
        assert!(d.release(E));
        // Shift for a capital leaves the accent waiting, and goes on.
        assert!(d.press(SHIFT, Press::Nothing, dead_char).pass);
        assert!(!d.release(SHIFT));
        let out = d.press(E, ch('E'), dead_char);
        assert_eq!(out.events, vec![commit("É")]);
        assert!(!out.pass);
        assert!(d.release(E));
        assert_eq!(d.pending(), None);
        // Then keys type as usual again.
        assert!(d.press(E, ch('e'), dead_char).pass);
        assert!(!d.release(E));
    }

    #[test]
    fn space_or_the_same_accent_types_the_accent() {
        let mut d = DeadKeys::default();
        d.press(U, dead(DIAERESIS), dead_char);
        assert_eq!(d.press(62, ch(' '), dead_char).events, vec![commit("¨")]);
        d.press(U, dead(DIAERESIS), dead_char);
        assert_eq!(
            d.press(U, dead(DIAERESIS), dead_char).events,
            vec![commit("¨")]
        );
        assert_eq!(d.pending(), None);
    }

    #[test]
    fn letters_that_dont_combine_keep_the_accent() {
        let mut d = DeadKeys::default();
        d.press(E, dead(ACUTE), dead_char);
        assert_eq!(d.press(X, ch('x'), dead_char).events, vec![commit("ˊx")]);
        // A second, different accent: the first is typed, the second waits.
        d.press(E, dead(ACUTE), dead_char);
        let out = d.press(U, dead(DIAERESIS), dead_char);
        assert_eq!(out.events[0], commit("ˊ"));
        assert_eq!(d.pending(), Some(DIAERESIS));
        assert_eq!(d.press(U, ch('u'), dead_char).events, vec![commit("ü")]);
    }

    #[test]
    fn backspace_cancels_and_other_keys_commit_and_pass() {
        let mut d = DeadKeys::default();
        d.press(E, dead(ACUTE), dead_char);
        let out = d.press(DEL, Press::Cancel, dead_char);
        assert!(!out.pass);
        assert_eq!(
            out.events,
            vec![Ime::Preedit {
                text: String::new(),
                cursor: None
            }]
        );
        assert!(d.release(DEL));
        d.press(E, dead(ACUTE), dead_char);
        let out = d.press(LEFT, Press::Other, dead_char);
        assert!(out.pass);
        assert_eq!(out.events, vec![commit("ˊ")]);
        assert!(!d.release(LEFT));
        // Without an accent waiting, nothing is taken.
        assert!(d.press(DEL, Press::Cancel, dead_char).pass);
    }

    #[test]
    fn focus_loss_drops_the_accent() {
        let mut d = DeadKeys::default();
        assert_eq!(d.reset(), None);
        d.press(E, dead(ACUTE), dead_char);
        assert_eq!(
            d.reset(),
            Some(Ime::Preedit {
                text: String::new(),
                cursor: None
            })
        );
        assert!(!d.release(E));
    }

    #[test]
    fn the_table_composes_latin_letters() {
        let acute = '\u{02CA}';
        assert_eq!(dead_char(acute, 'e'), Some('é'));
        assert_eq!(dead_char('\u{00B4}', 'a'), Some('á'));
        assert_eq!(dead_char('\u{02CB}', 'a'), Some('à'));
        assert_eq!(dead_char('`', 'A'), Some('À'));
        assert_eq!(dead_char('^', 'o'), Some('ô'));
        assert_eq!(dead_char('~', 'n'), Some('ñ'));
        assert_eq!(dead_char('\u{00A8}', 'u'), Some('ü'));
        assert_eq!(dead_char('\u{02C7}', 's'), Some('š'));
        assert_eq!(dead_char('\u{00B8}', 'c'), Some('ç'));
        assert_eq!(dead_char('\u{02DA}', 'a'), Some('å'));
        assert_eq!(dead_char('\u{02DB}', 'e'), Some('ę'));
        assert_eq!(dead_char('\u{02DD}', 'o'), Some('ő'));
        assert_eq!(dead_char(acute, 'q'), None);
        assert_eq!(dead_char('?', 'e'), None);
        // Every pair in the table really is the base with its mark: the
        // pairs are whole characters, and none repeats a base.
        for (mark, pairs) in COMPOSED {
            let chars: Vec<char> = pairs.chars().collect();
            assert_eq!(chars.len() % 2, 0, "{mark:?}");
            let bases: Vec<char> = chars.iter().step_by(2).copied().collect();
            let mut unique = bases.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), bases.len(), "{mark:?} repeats a base");
            for pair in chars.chunks(2) {
                assert!(pair[0].is_ascii_alphabetic(), "{pair:?}");
                assert!(!pair[1].is_ascii(), "{pair:?}");
            }
        }
    }
}
