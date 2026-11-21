//! The hand-written regex engine: syntax, leftmost-first semantics, case
//! folding, errors, and adversarial patterns that must stay linear.

use super::{Regex, RegexError};

fn re(pattern: &str) -> Regex {
    Regex::new(pattern, false).unwrap_or_else(|e| panic!("{pattern}: {e}"))
}

fn exact(pattern: &str) -> Regex {
    Regex::new(pattern, true).unwrap_or_else(|e| panic!("{pattern}: {e}"))
}

/// The texts of the matches of `pattern` (case-insensitive) in `line`.
fn all(pattern: &str, line: &str) -> Vec<String> {
    re(pattern)
        .find_all(line)
        .into_iter()
        .map(|r| line[r].to_string())
        .collect()
}

fn first(pattern: &str, line: &str) -> Option<String> {
    re(pattern).find_at(line, 0).map(|r| line[r].to_string())
}

fn err(pattern: &str) -> RegexError {
    match Regex::new(pattern, false) {
        Ok(_) => panic!("{pattern} compiled"),
        Err(e) => e,
    }
}

#[test]
fn literals_dot_and_escapes() {
    assert_eq!(all("ab", "xabyab"), vec!["ab", "ab"]);
    assert_eq!(
        all("a.c", "abc a-c ac a\u{e9}c"),
        vec!["abc", "a-c", "a\u{e9}c"]
    );
    assert_eq!(all(r"a\.c", "abc a.c"), vec!["a.c"]);
    assert_eq!(
        all(r"\(\)\[\]\{\}\*\+\?\|\^\$\\", r"()[]{}*+?|^$\"),
        vec![r"()[]{}*+?|^$\"]
    );
    assert_eq!(all(r"\t", "a\tb"), vec!["\t"]);
    // A `{` that isn't a repetition is itself.
    assert_eq!(all("a{", "a{"), vec!["a{"]);
    assert_eq!(all("a{x}", "a{x}"), vec!["a{x}"]);
    assert_eq!(all("a{1,x}", "a{1,x}"), vec!["a{1,x}"]);
    // Characters outside the BMP and multi-byte ones are characters.
    assert_eq!(all(".", "\u{1F600}\u{e9}"), vec!["\u{1F600}", "\u{e9}"]);
}

#[test]
fn classes_and_shorthands() {
    assert_eq!(all("[abc]+", "xxabcbaxcx"), vec!["abcba", "c"]);
    assert_eq!(all("[a-c0-2]+", "d0a2c3"), vec!["0a2c"]);
    assert_eq!(all("[^a-c ]+", "abc def gab"), vec!["def", "g"]);
    // `]` first and `-` last are themselves.
    assert_eq!(all("[]a]+", "x]a]x"), vec!["]a]"]);
    assert_eq!(all("[a-]+", "x-a-x"), vec!["-a-"]);
    assert_eq!(all(r"[\]\\]+", r"x]\x"), vec![r"]\"]);
    assert_eq!(all(r"\d+", "a12b345"), vec!["12", "345"]);
    assert_eq!(all(r"\D+", "a12b345"), vec!["a", "b"]);
    assert_eq!(all(r"\w+", "été_1, x!"), vec!["été_1", "x"]);
    assert_eq!(all(r"\W+", "été_1, x!"), vec![", ", "!"]);
    assert_eq!(all(r"\s+", "a \t b"), vec![" \t "]);
    assert_eq!(all(r"\S+", "a \t bc"), vec!["a", "bc"]);
    assert_eq!(all(r"[\d\s]+", "a1 2b"), vec!["1 2"]);
    assert_eq!(all(r"[^\d]+", "a1b"), vec!["a", "b"]);
}

#[test]
fn anchors_match_the_line_ends() {
    assert_eq!(all("^a", "aaa"), vec!["a"]);
    assert_eq!(all("a$", "aaa"), vec!["a"]);
    assert_eq!(all("^a+$", "aaa"), vec!["aaa"]);
    assert_eq!(all("^a+$", "aab"), Vec::<String>::new());
    assert_eq!(
        all("^$", ""),
        Vec::<String>::new(),
        "empty matches aren't reported"
    );
    assert_eq!(all("x|^y", "yxy"), vec!["y", "x"]);
}

#[test]
fn groups_and_alternation() {
    assert_eq!(all("cat|dog", "hotdog catalog"), vec!["dog", "cat"]);
    assert_eq!(all("gr(a|e)y", "grey gray groy"), vec!["grey", "gray"]);
    assert_eq!(all("(?:ab)+", "abababx"), vec!["ababab"]);
    assert_eq!(all("a(|b)c", "ac abc"), vec!["ac", "abc"]);
    assert_eq!(all("(a|)+b", "aab b"), vec!["aab", "b"]);
    assert_eq!(all("()*x", "x"), vec!["x"]);
    // Leftmost-first: the first alternative that matches wins, not the
    // longest.
    assert_eq!(first("a|ab", "ab").as_deref(), Some("a"));
    assert_eq!(first("ab|a", "ab").as_deref(), Some("ab"));
    // The leftmost start wins over a preferred alternative further on.
    assert_eq!(first("b|ab", "ab").as_deref(), Some("ab"));
}

#[test]
fn repetitions_greedy_and_lazy() {
    assert_eq!(first("a*", "aaa").as_deref(), Some("aaa"));
    assert_eq!(first("a+", "baaa").as_deref(), Some("aaa"));
    assert_eq!(first("ab?c", "ac").as_deref(), Some("ac"));
    assert_eq!(first("a{2}", "aaaaa").as_deref(), Some("aa"));
    assert_eq!(all("a{2}", "aaaaa"), vec!["aa", "aa"]);
    assert_eq!(first("a{2,}", "aaaaa").as_deref(), Some("aaaaa"));
    assert_eq!(first("a{2,3}", "aaaaa").as_deref(), Some("aaa"));
    assert_eq!(first("a{0,1}b", "aab").as_deref(), Some("ab"));
    assert_eq!(all("a{3,4}", "aa"), Vec::<String>::new());
    // Lazy: as few as will do.
    assert_eq!(first("a+?", "aaa").as_deref(), Some("a"));
    assert_eq!(first("a*?b", "aaab").as_deref(), Some("aaab"));
    assert_eq!(first("<.+?>", "<a><b>").as_deref(), Some("<a>"));
    assert_eq!(first("<.+>", "<a><b>").as_deref(), Some("<a><b>"));
    assert_eq!(first("a??b", "ab").as_deref(), Some("ab"));
    assert_eq!(first("a{2,4}?", "aaaa").as_deref(), Some("aa"));
    assert_eq!(first("a{2,}?", "aaaa").as_deref(), Some("aa"));
    // Empty matches move the search on by a character.
    assert_eq!(all("x*", "axxbx"), vec!["xx", "x"]);
}

#[test]
fn case_folds_unless_asked() {
    assert_eq!(
        all("error", "Error ERROR error"),
        vec!["Error", "ERROR", "error"]
    );
    assert_eq!(all("[a-z]+", "ABC"), vec!["ABC"]);
    assert_eq!(all("[A-Z]+", "abc"), vec!["abc"]);
    assert_eq!(all("ÉTÉ", "l'été"), vec!["été"]);
    assert_eq!(all("[^a]", "A"), Vec::<String>::new());
    let r = exact("Error");
    assert_eq!(r.find_all("Error ERROR error"), vec![0..5]);
    assert_eq!(exact("[a-z]+").find_all("ABC"), vec![]);
}

#[test]
fn invalid_patterns_are_errors_not_panics() {
    for (pattern, offset) in [
        ("(ab", 0),
        ("ab)", 2),
        ("[ab", 0),
        ("[", 0),
        ("[^", 0),
        ("*a", 0),
        ("a|+", 2),
        ("a**", 2),
        ("a+*", 2),
        ("a{2}{3}", 4),
        ("^*", 1),
        ("(?:", 0),
        ("[z-a]", 3),
        (r"[a-\d]", 3),
        ("a{3,2}", 1),
        ("a{1001}", 1),
        ("{2}", 0),
        ("a\\", 1),
        (r"\q", 0),
        (r"\b", 0),
    ] {
        let e = err(pattern);
        assert_eq!(e.offset, offset, "{pattern}: {e}");
        assert!(!e.message.is_empty());
        assert!(e.to_string().contains("at byte"));
    }
    // Too deep, too large: errors too, without blowing the stack.
    let deep = format!("{}a{}", "(".repeat(10_000), ")".repeat(10_000));
    assert_eq!(err(&deep).message, "groups nest too deep");
    assert_eq!(err("(a{1000}){1000}").message, "the pattern is too large");
    assert_eq!(
        err("((a{100}){100}){100}").message,
        "the pattern is too large"
    );
    // Odd, but valid.
    for ok in [
        "", "()", "(|)", "a||b", "[]]", "[-]", "[^-]", "x{0}", "x{0,0}",
    ] {
        assert!(Regex::new(ok, false).is_ok(), "{ok}");
    }
    assert_eq!(re("x{0}y").find_all("xy"), vec![1..2]);
}

#[test]
fn every_byte_and_char_pattern_parses_or_errors() {
    // No pattern of one or two characters panics, whatever they are.
    let chars: Vec<char> = (0x20u8..0x7f)
        .map(char::from)
        .chain(['é', '\u{1F600}'])
        .collect();
    for &a in &chars {
        for &b in &chars {
            let p: String = [a, b].iter().collect();
            if let Ok(r) = Regex::new(&p, false) {
                let _ = r.find_all("aB1 _-x\u{1F600}é{}[]()");
            }
            let p: String = [a, b, '{', '1', '}'].iter().collect();
            let _ = Regex::new(&p, false);
        }
    }
}

/// Steps a search of `pattern` over `line` takes, at most `per` per
/// character and instruction.
fn assert_linear(pattern: &str, line: &str, per: u64) -> Vec<std::ops::Range<usize>> {
    let r = re(pattern);
    let (found, steps) = r.find_all_steps(line);
    let bound = per * (line.chars().count() as u64 + 1) * (r.len() as u64 + 4);
    assert!(
        steps <= bound,
        "{pattern}: {steps} steps over {} chars, {} instructions",
        line.len(),
        r.len()
    );
    found
}

#[test]
fn adversarial_patterns_stay_linear() {
    // The patterns that make a backtracking engine take exponential time
    // take a step or two per character and instruction here.
    let a = "a".repeat(20_000);
    let ab = format!("{a}!");
    assert!(assert_linear("(a*)*b", &ab, 2).is_empty());
    assert!(assert_linear("(a+)+b", &ab, 2).is_empty());
    assert!(assert_linear("(a|a)+b", &ab, 2).is_empty());
    assert!(assert_linear("(a|aa)+b", &ab, 2).is_empty());
    assert!(assert_linear("(.*)*b", &ab, 2).is_empty());
    assert!(assert_linear("((a*)*)*b", &ab, 2).is_empty());
    assert!(assert_linear("(x+x+)+y", &"x".repeat(20_000), 2).is_empty());
    assert!(assert_linear("^(a?){50}a{50}$", &"a".repeat(49), 2).is_empty());
    assert_eq!(assert_linear("^(a|aa)+$", &a, 2), vec![0..a.len()]);
    assert_eq!(assert_linear("(a*)*$", &a, 2), vec![0..a.len()]);
    // Counted repetitions are big programs, still linear.
    assert!(assert_linear("a{500}b", &ab, 2).is_empty());
    assert!(assert_linear("(a|b){0,200}c", &ab, 2).is_empty());
    // A match found early stops the search: one pass, not one per start.
    assert_eq!(assert_linear("a", &a, 2).len(), a.len());
    assert_eq!(assert_linear("[^x]+x", &ab, 2), vec![]);
}

#[test]
fn a_long_line_searches_fast() {
    // A megabyte line, a pattern with alternation and classes: done in one
    // pass (the bound is generous; it is a safety net, not a benchmark).
    let line = "lorem ipsum dolor 12:34:56 sit amet ".repeat(30_000);
    let started = std::time::Instant::now();
    let r = re(r"\d\d:\d\d:\d\d|amet\s+lorem");
    let found = r.find_all(&line);
    assert_eq!(found.len(), 30_000 + 29_999);
    assert!(started.elapsed().as_secs() < 30, "{:?}", started.elapsed());
}
