//! Dead keys and compose sequences.
//!
//! On X11 and Wayland, turning `dead_acute` `e` into `é`, or `Multi_key` `o`
//! `c` into `©`, is the client's job. libX11 does it from a Compose file, and
//! so does this module, with a hand-written parser for the same format:
//!
//! ```text
//! <dead_acute> <e>        : "é"   eacute   # LATIN SMALL LETTER E WITH ACUTE
//! <Multi_key> <o> <c>     : "©"   copyright
//! include "%L"
//! ```
//!
//! [`table`] loads the file once: `$XCOMPOSEFILE`, else `~/.XCompose`, else
//! `$XDG_CONFIG_HOME/XCompose`, else the system file for the locale
//! (`/usr/share/X11/locale/<locale>/Compose`, found through `compose.dir`).
//! A [`ComposeState`] then runs keysyms through it, one key press at a time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::keysyms::KEYSYMS;

const BACKSPACE: u32 = 0xff08;

/// The keysym a Compose-file or xkb name stands for: a name from
/// `keysymdef.h`, `UXXXX` (a Unicode keysym) or `0x…` (a raw value).
pub(crate) fn keysym_from_name(name: &str) -> Option<u32> {
    if let Ok(i) = KEYSYMS.binary_search_by(|(n, _, _)| n.as_bytes().cmp(name.as_bytes())) {
        return Some(KEYSYMS[i].1);
    }
    if let Some(hex) = name.strip_prefix('U')
        && (4..=6).contains(&hex.len())
        && let Ok(cp) = u32::from_str_radix(hex, 16)
    {
        return char::from_u32(cp).map(|_| 0x0100_0000 + cp);
    }
    if let Some(hex) = name.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16).ok();
    }
    None
}

/// Keysym → Unicode for the keysyms whose value isn't the code point.
fn unicode_by_keysym() -> &'static HashMap<u32, u32> {
    static MAP: OnceLock<HashMap<u32, u32>> = OnceLock::new();
    MAP.get_or_init(|| {
        KEYSYMS
            .iter()
            .filter(|(_, _, cp)| *cp != 0)
            .map(|&(_, ks, cp)| (ks, cp))
            .collect()
    })
}

/// The printable character a keysym types, if any: Latin-1 keysyms are their
/// code point, `0x0100_0000 + cp` is Unicode, and the legacy keysyms (Latin-2,
/// Greek, Cyrillic, kana, …) go through the keysym table.
pub(crate) fn keysym_to_char(ks: u32) -> Option<char> {
    let cp = match ks {
        0x20..=0x7e | 0xa0..=0xff => ks,
        0x0100_0000..=0x0110_ffff => ks - 0x0100_0000,
        _ => *unicode_by_keysym().get(&ks)?,
    };
    char::from_u32(cp).filter(|c| !c.is_control())
}

/// One spelling per keysym, so a key typed as `U00E9`, `eacute` or `0xe9`
/// matches the same sequence. Keypad, dead and function keysyms (0xfe00 to
/// 0xffff) stay as they are: `KP_Divide` composes differently from `slash`.
fn normalize(ks: u32) -> u32 {
    if (0xfe00..=0xffff).contains(&ks) {
        return ks;
    }
    match keysym_to_char(ks) {
        Some(c) if (c as u32) < 0x100 => c as u32,
        Some(c) => 0x0100_0000 + c as u32,
        None => ks,
    }
}

/// Modifier keysyms (Shift, Control, Alt, AltGr, Caps Lock, …) never interrupt
/// a sequence.
pub(crate) fn is_modifier(ks: u32) -> bool {
    matches!(ks, 0xffe1..=0xffee | 0xfe01..=0xfe0f | 0xff7e | 0xff7f)
}

/// What to show for a keysym in a pending sequence: the spacing form of a dead
/// key, `·` for the Compose key, else the character itself.
fn preedit_char(ks: u32) -> char {
    match ks {
        0xfe50 => '`',
        0xfe51 => '´',
        0xfe52 => '^',
        0xfe53 => '~',
        0xfe54 => '¯',
        0xfe55 => '˘',
        0xfe56 => '˙',
        0xfe57 => '¨',
        0xfe58 => '˚',
        0xfe59 => '˝',
        0xfe5a => 'ˇ',
        0xfe5b => '¸',
        0xfe5c => '˛',
        0xfe5d => 'ͺ',
        0xfe5e => '゛',
        0xfe5f => '゜',
        0xfe6f => '¤',
        _ => keysym_to_char(ks).unwrap_or('·'),
    }
}

/// A trie node: the keysyms that may follow, or the text a complete sequence
/// produces.
#[derive(Clone, Debug, Default)]
struct Node {
    next: Vec<(u32, usize)>,
    result: Option<String>,
}

/// Parsed compose sequences, as a trie over normalized keysyms.
#[derive(Clone, Debug)]
pub(crate) struct ComposeTable {
    nodes: Vec<Node>,
}

impl Default for ComposeTable {
    fn default() -> Self {
        Self {
            nodes: vec![Node::default()],
        }
    }
}

/// Where `include` directives and `%` substitutions resolve.
struct Paths {
    home: Option<PathBuf>,
    /// The X locale directory (`/usr/share/X11/locale`).
    system: PathBuf,
    /// The locale's system Compose file (`%L`).
    locale_file: Option<PathBuf>,
}

impl ComposeTable {
    /// Parse Compose-file `text`, following `include` directives (after `%H`,
    /// `%L` and `%S` substitution) up to `depth` levels deep.
    fn parse_with(text: &str, paths: &Paths, depth: u32) -> Self {
        let mut table = Self::default();
        table.add(text, paths, depth);
        table
    }

    /// Parse `text` with no includes (for tests and simple callers).
    #[cfg(test)]
    pub(crate) fn parse(text: &str) -> Self {
        let paths = Paths {
            home: None,
            system: PathBuf::new(),
            locale_file: None,
        };
        Self::parse_with(text, &paths, 8)
    }

    fn add(&mut self, text: &str, paths: &Paths, depth: u32) {
        for line in text.lines() {
            let line = line.trim_start();
            if let Some(rest) = line.strip_prefix("include") {
                if depth == 0 {
                    continue;
                }
                let Some((path, _)) = quoted(rest.trim_start()) else {
                    continue;
                };
                let Some(path) = substitute(&path, paths) else {
                    continue;
                };
                if let Some(text) = read_text(&path) {
                    self.add(&text, paths, depth - 1);
                }
            } else if line.starts_with('<')
                && let Some((keys, result)) = parse_sequence(line)
            {
                self.insert(&keys, result);
            }
        }
    }

    /// Add a sequence. A later definition wins: it replaces a sequence it
    /// extends or that extends it, as in libX11.
    fn insert(&mut self, keys: &[u32], result: String) {
        let mut at = 0;
        for (i, &ks) in keys.iter().enumerate() {
            let last = i + 1 == keys.len();
            let child = self.nodes[at]
                .next
                .iter()
                .find(|(k, _)| *k == ks)
                .map(|&(_, n)| n);
            at = match child {
                Some(n) => n,
                None => {
                    self.nodes.push(Node::default());
                    let n = self.nodes.len() - 1;
                    self.nodes[at].next.push((ks, n));
                    n
                }
            };
            if last {
                self.nodes[at].next.clear();
                self.nodes[at].result = Some(result.clone());
            } else {
                self.nodes[at].result = None;
            }
        }
    }

    /// The node reached by `keys`, if they are a prefix of some sequence.
    fn walk(&self, keys: &[u32]) -> Option<&Node> {
        let mut at = 0;
        for &ks in keys {
            at = self.nodes[at]
                .next
                .iter()
                .find(|(k, _)| *k == ks)
                .map(|&(_, n)| n)?;
        }
        Some(&self.nodes[at])
    }

    /// Whether the table holds no sequences.
    pub(crate) fn is_empty(&self) -> bool {
        self.nodes[0].next.is_empty()
    }
}

/// Parse a sequence line, `<a> <b> : "text" keysym # comment`. Lines with
/// modifier prefixes (`~Ctrl <a>`), unknown keysym names or no result are
/// skipped (`None`).
fn parse_sequence(line: &str) -> Option<(Vec<u32>, String)> {
    let colon = line.find(':')?;
    let (lhs, rhs) = (&line[..colon], line[colon + 1..].trim_start());
    let mut keys = Vec::new();
    let mut rest = lhs.trim();
    while !rest.is_empty() {
        let inner = rest.strip_prefix('<')?;
        let end = inner.find('>')?;
        keys.push(normalize(keysym_from_name(&inner[..end])?));
        rest = inner[end + 1..].trim_start();
    }
    if keys.is_empty() {
        return None;
    }
    // The result: a string, a keysym, or both (the string wins).
    let result = match quoted(rhs) {
        Some((text, _)) => text,
        None => {
            let name = rhs.split(|c: char| c.is_whitespace() || c == '#').next()?;
            keysym_to_char(keysym_from_name(name)?)?.to_string()
        }
    };
    (!result.is_empty()).then_some((keys, result))
}

/// A double-quoted string at the start of `s`, with the Compose file's
/// escapes (`\"`, `\\`, `\n`, octal `\123` and hex `\x41`), and the rest of
/// the line.
fn quoted(s: &str) -> Option<(String, &str)> {
    let body = s.strip_prefix('"')?;
    // Escapes give bytes, so collect bytes and decode as UTF-8 at the end.
    let mut bytes = Vec::new();
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                return Some((text, &body[i + 1..]));
            }
            '\\' => {
                let Some((_, e)) = chars.next() else { break };
                match e {
                    'n' => bytes.push(b'\n'),
                    'r' => bytes.push(b'\r'),
                    't' => bytes.push(b'\t'),
                    'x' | 'X' => {
                        let mut v = 0u32;
                        let mut n = 0;
                        while n < 2 {
                            let mut peek = chars.clone();
                            match peek.next().and_then(|(_, d)| d.to_digit(16)) {
                                Some(d) => {
                                    v = v * 16 + d;
                                    chars = peek;
                                    n += 1;
                                }
                                None => break,
                            }
                        }
                        bytes.push(v as u8);
                    }
                    '0'..='7' => {
                        let mut v = e.to_digit(8).unwrap_or(0);
                        for _ in 0..2 {
                            let mut peek = chars.clone();
                            match peek.next().and_then(|(_, d)| d.to_digit(8)) {
                                Some(d) => {
                                    v = v * 8 + d;
                                    chars = peek;
                                }
                                None => break,
                            }
                        }
                        bytes.push(v as u8);
                    }
                    other => {
                        let mut buf = [0u8; 4];
                        bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                    }
                }
            }
            other => {
                let mut buf = [0u8; 4];
                bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    None
}

/// Expand `%H` (home), `%L` (the locale's Compose file) and `%S` (the X
/// locale directory) in an `include` path. `None` if a needed one is unknown.
fn substitute(path: &str, paths: &Paths) -> Option<PathBuf> {
    let mut out = String::new();
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('H') => out.push_str(paths.home.as_ref()?.to_str()?),
            Some('L') => out.push_str(paths.locale_file.as_ref()?.to_str()?),
            Some('S') => out.push_str(paths.system.to_str()?),
            Some('%') => out.push('%'),
            _ => return None,
        }
    }
    Some(PathBuf::from(out))
}

/// Read a Compose file. Old locales' files are in their 8-bit charset; those
/// that aren't UTF-8 are read as Latin-1, which keeps the ASCII lines usable.
fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    })
}

/// The locale that decides the Compose file: `LC_ALL`, `LC_CTYPE`, `LANG`.
fn current_locale() -> String {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| "C".to_string())
}

/// Look `key` up in an X locale table (`compose.dir`, `locale.alias`): lines
/// of two columns, the first optionally ending in `:`. `dir_first` says
/// whether the value is the first column (`compose.dir`: `file  locale`) or
/// the second (`locale.alias`: `alias  locale`).
fn lookup_table(text: &str, key: &str, dir_first: bool) -> Option<String> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .find_map(|l| {
            let mut cols = l.split_whitespace();
            let a = cols.next()?.trim_end_matches(':');
            let b = cols.next()?;
            if dir_first {
                (b == key).then(|| a.to_string())
            } else {
                (a == key).then(|| b.to_string())
            }
        })
}

/// The system Compose file for `locale` under `system`, through
/// `compose.dir` (and `locale.alias` for spellings like `en_US.utf8`). The C
/// locale, and any locale without a file, gets `en_US.UTF-8`'s: Stipple text
/// is always Unicode.
fn locale_compose_file(system: &Path, locale: &str) -> Option<PathBuf> {
    let dir = std::fs::read_to_string(system.join("compose.dir")).ok();
    let alias = std::fs::read_to_string(system.join("locale.alias")).ok();
    let mut names = vec![locale.to_string()];
    if let Some(a) = alias
        .as_deref()
        .and_then(|t| lookup_table(t, locale, false))
    {
        names.push(a);
    }
    let c_locale = matches!(locale, "C" | "POSIX");
    if !c_locale && let Some(dir) = dir.as_deref() {
        for name in &names {
            if let Some(file) = lookup_table(dir, name, true) {
                return Some(system.join(file));
            }
        }
    }
    let fallback = system.join("en_US.UTF-8/Compose");
    fallback.exists().then_some(fallback)
}

/// Find and parse the Compose file for this user and locale.
fn load() -> ComposeTable {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let system = std::env::var_os("XLOCALEDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/share/X11/locale"));
    let locale_file = locale_compose_file(&system, &current_locale());
    let paths = Paths {
        home: home.clone(),
        system,
        locale_file: locale_file.clone(),
    };
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".config")));
    let candidates = [
        std::env::var_os("XCOMPOSEFILE").map(PathBuf::from),
        home.as_ref().map(|h| h.join(".XCompose")),
        xdg.map(|d| d.join("XCompose")),
        locale_file,
    ];
    for path in candidates.into_iter().flatten() {
        if let Some(text) = read_text(&path) {
            return ComposeTable::parse_with(&text, &paths, 8);
        }
    }
    ComposeTable::default()
}

/// The user's compose table, loaded on first use and shared by every window.
pub(crate) fn table() -> &'static ComposeTable {
    static TABLE: OnceLock<ComposeTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        let table = load();
        if std::env::var_os("STIPPLE_COMPOSE_DEBUG").is_some() {
            eprintln!(
                "stipple compose: {} nodes (locale {})",
                table.nodes.len(),
                current_locale()
            );
        }
        table
    })
}

/// What a key press did to the compose sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Feed {
    /// Not part of a sequence: handle the key as usual.
    Pass,
    /// The sequence goes on; show this as the preedit.
    Pending(String),
    /// The sequence completed: commit this text.
    Composed(String),
    /// The key doesn't continue the sequence, which is dropped (and the key
    /// with it, as libX11 and xkbcommon do).
    Cancelled,
}

/// A compose sequence in progress.
#[derive(Clone, Debug, Default)]
pub(crate) struct ComposeState {
    keys: Vec<u32>,
    /// The raw keysyms, for the preedit.
    typed: Vec<u32>,
}

impl ComposeState {
    /// Whether a sequence is in progress.
    pub(crate) fn composing(&self) -> bool {
        !self.keys.is_empty()
    }

    /// Drop any sequence in progress (focus loss).
    pub(crate) fn reset(&mut self) {
        self.keys.clear();
        self.typed.clear();
    }

    fn preedit(&self) -> String {
        self.typed.iter().map(|&k| preedit_char(k)).collect()
    }

    /// Run a key press's keysym through `table`.
    pub(crate) fn feed(&mut self, table: &ComposeTable, ks: u32) -> Feed {
        if ks == 0 || is_modifier(ks) {
            return Feed::Pass;
        }
        if self.composing() && ks == BACKSPACE {
            // Backspace takes back the last key of the sequence.
            self.keys.pop();
            self.typed.pop();
            return if self.composing() {
                Feed::Pending(self.preedit())
            } else {
                Feed::Cancelled
            };
        }
        let key = normalize(ks);
        let mut keys = self.keys.clone();
        keys.push(key);
        match table.walk(&keys) {
            Some(node) => {
                if let Some(result) = &node.result {
                    let result = result.clone();
                    self.reset();
                    Feed::Composed(result)
                } else {
                    self.keys = keys;
                    self.typed.push(ks);
                    Feed::Pending(self.preedit())
                }
            }
            None if self.composing() => {
                self.reset();
                Feed::Cancelled
            }
            None => Feed::Pass,
        }
    }
}

/// The IME event for what a key press did, or `None` to handle the key as
/// usual. Composing runs only for text keys: a press with Ctrl or the logo key
/// held is a shortcut.
pub(crate) fn feed_event(
    state: &mut ComposeState,
    ks: u32,
    shortcut: bool,
) -> Option<crate::platform::event::Ime> {
    use crate::platform::event::Ime;
    if shortcut && !state.composing() {
        return None;
    }
    let table = table();
    if table.is_empty() {
        return None;
    }
    match state.feed(table, ks) {
        Feed::Pass => None,
        Feed::Pending(text) => {
            let end = text.len();
            Some(Ime::Preedit {
                text,
                cursor: Some((end, end)),
            })
        }
        Feed::Composed(text) => Some(Ime::Commit(text)),
        Feed::Cancelled => Some(Ime::Preedit {
            text: String::new(),
            cursor: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MULTI_KEY: u32 = 0xff20;
    const DEAD_ACUTE: u32 = 0xfe51;
    const DEAD_DIAERESIS: u32 = 0xfe57;

    #[test]
    fn keysym_names_and_characters() {
        assert_eq!(keysym_from_name("eacute"), Some(0xe9));
        assert_eq!(keysym_from_name("dead_acute"), Some(DEAD_ACUTE));
        assert_eq!(keysym_from_name("Multi_key"), Some(MULTI_KEY));
        assert_eq!(keysym_from_name("a"), Some(0x61));
        assert_eq!(keysym_from_name("U20AC"), Some(0x0100_20ac));
        assert_eq!(keysym_from_name("0xff08"), Some(0xff08));
        assert_eq!(keysym_from_name("no_such_key"), None);
        assert_eq!(keysym_to_char(0xe9), Some('é'));
        assert_eq!(keysym_to_char(0x1b9), Some('š')); // scaron (Latin-2)
        assert_eq!(keysym_to_char(0x0100_20ac), Some('€'));
        assert_eq!(keysym_to_char(DEAD_ACUTE), None);
        assert_eq!(keysym_to_char(0xff08), None); // BackSpace
        // The table is sorted, so binary search finds every entry.
        assert!(
            KEYSYMS
                .windows(2)
                .all(|w| w[0].0.as_bytes() < w[1].0.as_bytes())
        );
        // One spelling per character.
        assert_eq!(normalize(0x0100_00e9), 0xe9);
        assert_eq!(normalize(0x1b9), 0x0100_0161);
        assert_eq!(normalize(0xffaf), 0xffaf); // KP_Divide stays itself
    }

    const SAMPLE: &str = r#"
# A comment line
<dead_acute> <e>            : "é"   eacute # LATIN SMALL LETTER E WITH ACUTE
<dead_acute> <space>        : "'"   apostrophe
<dead_diaeresis> <U0075>    : "ü"
<Multi_key> <o> <c>         : "©"   copyright
<Multi_key> <minus> <greater> : "→"
<Multi_key> <q> <quotedbl>  : "say \"hi\"\n"
<Multi_key> <x> <x>         : multiply
~Ctrl <a> : "skipped"
<Multi_key> <no_such_key>   : "skipped too"
"#;

    #[test]
    fn parses_sequences_strings_and_escapes() {
        let t = ComposeTable::parse(SAMPLE);
        let run = |keys: &[u32]| {
            let mut s = ComposeState::default();
            let mut last = Feed::Pass;
            for &k in keys {
                last = s.feed(&t, k);
            }
            last
        };
        assert_eq!(run(&[DEAD_ACUTE, 0x65]), Feed::Composed("é".into()));
        assert_eq!(run(&[DEAD_ACUTE, 0x20]), Feed::Composed("'".into()));
        // `U0075` in the file matches the keyboard's plain `u`.
        assert_eq!(run(&[DEAD_DIAERESIS, 0x75]), Feed::Composed("ü".into()));
        assert_eq!(run(&[MULTI_KEY, 0x6f, 0x63]), Feed::Composed("©".into()));
        assert_eq!(run(&[MULTI_KEY, 0x2d, 0x3e]), Feed::Composed("→".into()));
        assert_eq!(
            run(&[MULTI_KEY, 0x71, 0x22]),
            Feed::Composed("say \"hi\"\n".into())
        );
        // A keysym alone as the result.
        assert_eq!(run(&[MULTI_KEY, 0x78, 0x78]), Feed::Composed("×".into()));
        // Lines with modifiers or unknown names are skipped.
        assert_eq!(run(&[0x61]), Feed::Pass);
    }

    #[test]
    fn escapes_in_octal_and_hex() {
        let t = ComposeTable::parse("<Multi_key> <a> : \"\\303\\251\\x41\"\n");
        let mut s = ComposeState::default();
        s.feed(&t, MULTI_KEY);
        assert_eq!(s.feed(&t, 0x61), Feed::Composed("éA".into()));
    }

    #[test]
    fn state_machine_pending_cancel_and_backspace() {
        let t = ComposeTable::parse(SAMPLE);
        let mut s = ComposeState::default();
        // Plain keys pass through untouched.
        assert_eq!(s.feed(&t, 0x65), Feed::Pass);
        // A dead key starts a sequence and shows its spacing form.
        assert_eq!(s.feed(&t, DEAD_ACUTE), Feed::Pending("´".into()));
        assert!(s.composing());
        // Shift between the keys doesn't break it.
        assert_eq!(s.feed(&t, 0xffe1), Feed::Pass);
        assert_eq!(s.feed(&t, 0x65), Feed::Composed("é".into()));
        assert!(!s.composing());
        // A key that continues nothing cancels the sequence.
        assert_eq!(s.feed(&t, MULTI_KEY), Feed::Pending("·".into()));
        assert_eq!(s.feed(&t, 0x6f), Feed::Pending("·o".into()));
        assert_eq!(s.feed(&t, 0x7a), Feed::Cancelled);
        assert!(!s.composing());
        // Backspace takes back the last key, then cancels.
        s.feed(&t, MULTI_KEY);
        s.feed(&t, 0x6f);
        assert_eq!(s.feed(&t, BACKSPACE), Feed::Pending("·".into()));
        assert_eq!(s.feed(&t, BACKSPACE), Feed::Cancelled);
        // Backspace outside a sequence is an ordinary key.
        assert_eq!(s.feed(&t, BACKSPACE), Feed::Pass);
    }

    #[test]
    fn later_definitions_win() {
        let t = ComposeTable::parse(
            "<Multi_key> <a> : \"1\"\n<Multi_key> <a> <b> : \"2\"\n<Multi_key> <c> <d> : \"3\"\n<Multi_key> <c> : \"4\"\n",
        );
        let mut s = ComposeState::default();
        s.feed(&t, MULTI_KEY);
        assert_eq!(s.feed(&t, 0x61), Feed::Pending("·a".into()));
        assert_eq!(s.feed(&t, 0x62), Feed::Composed("2".into()));
        s.feed(&t, MULTI_KEY);
        assert_eq!(s.feed(&t, 0x63), Feed::Composed("4".into()));
    }

    #[test]
    fn includes_and_substitutions() {
        let dir = std::env::temp_dir().join(format!("stipple-compose-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("en_US.UTF-8")).unwrap();
        std::fs::write(
            dir.join("en_US.UTF-8/Compose"),
            "<dead_acute> <e> : \"é\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("compose.dir"),
            "# comment\nen_US.UTF-8/Compose:\t\ten_US.UTF-8\n",
        )
        .unwrap();
        std::fs::write(dir.join("locale.alias"), "en_US.utf8:\t\ten_US.UTF-8\n").unwrap();
        // Spelled through the alias, the locale still finds its file.
        let locale_file = locale_compose_file(&dir, "en_US.utf8");
        assert_eq!(locale_file, Some(dir.join("en_US.UTF-8/Compose")));
        // The C locale (and unknown ones) fall back to en_US.UTF-8.
        assert_eq!(locale_compose_file(&dir, "C"), locale_file);
        assert_eq!(locale_compose_file(&dir, "xx_XX.UTF-8"), locale_file);
        let paths = Paths {
            home: Some(dir.clone()),
            system: dir.clone(),
            locale_file,
        };
        std::fs::write(dir.join("extra"), "<Multi_key> <o> <c> : \"©\"\n").unwrap();
        let user = "include \"%L\"\ninclude \"%H/extra\"\n<dead_acute> <e> : \"E\"\n";
        let t = ComposeTable::parse_with(user, &paths, 8);
        let mut s = ComposeState::default();
        s.feed(&t, DEAD_ACUTE);
        // The user's own line comes after the include, so it wins.
        assert_eq!(s.feed(&t, 0x65), Feed::Composed("E".into()));
        s.feed(&t, MULTI_KEY);
        s.feed(&t, 0x6f);
        assert_eq!(s.feed(&t, 0x63), Feed::Composed("©".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn system_compose_file_parses() {
        // On a machine with libX11's data, the real file loads and composes.
        let path = Path::new("/usr/share/X11/locale/en_US.UTF-8/Compose");
        let Some(text) = read_text(path) else { return };
        let t = ComposeTable::parse(&text);
        let mut s = ComposeState::default();
        s.feed(&t, DEAD_ACUTE);
        assert_eq!(s.feed(&t, 0x65), Feed::Composed("é".into()));
        s.feed(&t, MULTI_KEY);
        s.feed(&t, 0x6f);
        assert_eq!(s.feed(&t, 0x63), Feed::Composed("©".into()));
    }
}
