//! A small regular-expression engine for [`TextView`](super::TextView)'s
//! regex find, written by hand (the crate takes no dependencies).
//!
//! The pattern is parsed to a tree, compiled to a program for a Pike VM
//! (Thompson's construction, run breadth-first with the threads kept in
//! priority order), and run over a line. There is no backtracking: a search
//! steps every live thread over each character once, so it takes time
//! proportional to the line's length times the program's size, whatever the
//! pattern (`(a*)*b`, `(a|aa)+$` and their kind stay fast). The program's
//! size is capped, and so are counted repetitions and nesting, so a pattern
//! can't make it, or the parser's recursion, explode.
//!
//! Syntax: literal characters; `.` (any character); classes `[abc]`,
//! `[a-z]`, `[^...]` (with escapes, and `\d` and the like, inside); `\d`
//! (ASCII digits), `\w` (letters, digits and `_`, in any script), `\s`
//! (white space) and their negations `\D`, `\W`, `\S`; `\t`, `\n`, `\r` and
//! `\` before punctuation for the character itself; anchors `^` and `$`
//! (the line's start and end); groups `(...)` (and `(?:...)`, the same)
//! with alternation `|`; and the repetitions `*`, `+`, `?`, `{m}`, `{m,}`
//! and `{m,n}`, greedy, or lazy with a `?` after them. Matching is
//! leftmost-first, as in Perl and most engines: of the matches starting
//! leftmost, the one the greedy or lazy choices prefer.

use std::fmt;
use std::ops::Range;

/// Why a pattern isn't a valid regular expression: what's wrong, and where
/// (a byte offset into the pattern).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RegexError {
    /// Where in the pattern, in bytes.
    pub offset: usize,
    /// What's wrong, in English.
    pub message: &'static str,
}

impl fmt::Display for RegexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at byte {})", self.message, self.offset)
    }
}

impl std::error::Error for RegexError {}

/// The most instructions a program may have.
const MAX_PROGRAM: usize = 20_000;
/// The largest count in `{m,n}`.
const MAX_REPEAT: u32 = 1_000;
/// The deepest groups may nest.
const MAX_DEPTH: usize = 200;

/// A compiled regular expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Regex {
    program: Vec<Inst>,
    /// Compare characters folded to lower case.
    fold: bool,
}

/// A character class: its items, and whether it matches what they don't.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Class {
    negated: bool,
    items: Vec<ClassItem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClassItem {
    Range(char, char),
    /// `\d`, or `\D` when `true`.
    Digit(bool),
    /// `\w`, or `\W`.
    Word(bool),
    /// `\s`, or `\S`.
    Space(bool),
}

impl ClassItem {
    fn matches(self, c: char) -> bool {
        match self {
            ClassItem::Range(a, b) => a <= c && c <= b,
            ClassItem::Digit(neg) => c.is_ascii_digit() != neg,
            ClassItem::Word(neg) => is_word(c) != neg,
            ClassItem::Space(neg) => c.is_whitespace() != neg,
        }
    }
}

impl Class {
    fn matches(&self, c: char, fold: bool) -> bool {
        let any = |c: char| self.items.iter().any(|i| i.matches(c));
        let hit = any(c) || (fold && (any(lower(c)) || any(upper(c))));
        hit != self.negated
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `c` in lower case, when that is one character.
fn lower(c: char) -> char {
    single(c.to_lowercase()).unwrap_or(c)
}

/// `c` in upper case, when that is one character.
fn upper(c: char) -> char {
    single(c.to_uppercase()).unwrap_or(c)
}

fn single(mut it: impl Iterator<Item = char>) -> Option<char> {
    let c = it.next()?;
    it.next().is_none().then_some(c)
}

/// The parsed pattern.
#[derive(Debug)]
enum Node {
    Empty,
    Char(char),
    Any,
    Class(Class),
    LineStart,
    LineEnd,
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    },
}

/// One instruction of the program.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Inst {
    /// Match the character (folded, when the regex is).
    Char(char),
    Any,
    Class(Box<Class>),
    LineStart,
    LineEnd,
    /// Go on at both; the first is preferred.
    Split(usize, usize),
    Jump(usize),
    Match,
}

struct Parser<'a> {
    pattern: &'a str,
    at: usize,
    depth: usize,
}

impl Parser<'_> {
    fn error(&self, message: &'static str) -> RegexError {
        RegexError {
            offset: self.at,
            message,
        }
    }

    fn peek(&self) -> Option<char> {
        self.pattern[self.at..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.at += c.len_utf8();
            true
        } else {
            false
        }
    }

    /// `a|b|...`, up to a `)` or the end.
    fn alternation(&mut self) -> Result<Node, RegexError> {
        let mut alts = vec![self.concat()?];
        while self.eat('|') {
            alts.push(self.concat()?);
        }
        Ok(if alts.len() == 1 {
            alts.pop().expect("one")
        } else {
            Node::Alt(alts)
        })
    }

    fn concat(&mut self) -> Result<Node, RegexError> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            items.push(self.repeats(atom)?);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().expect("one"),
            _ => Node::Concat(items),
        })
    }

    /// `atom` with the repetition after it, if any.
    fn repeats(&mut self, atom: Node) -> Result<Node, RegexError> {
        let start = self.at;
        let (min, max) = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => match self.counted()? {
                Some(range) => range,
                None => return Ok(atom),
            },
            _ => return Ok(atom),
        };
        if self.at == start {
            self.bump();
        }
        if matches!(atom, Node::LineStart | Node::LineEnd) {
            return Err(RegexError {
                offset: start,
                message: "an anchor can't repeat",
            });
        }
        let greedy = !self.eat('?');
        if matches!(self.peek(), Some('*' | '+' | '?')) || self.is_counted() {
            return Err(self.error("a repetition can't repeat (group it first)"));
        }
        if matches!(atom, Node::Empty) {
            return Ok(Node::Empty);
        }
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
            greedy,
        })
    }

    /// Whether a `{m,n}` repetition starts here.
    fn is_counted(&self) -> bool {
        self.counted_at(self.at)
    }

    /// Whether a `{m,n}` repetition (valid or not) starts at byte `at`.
    fn counted_at(&self, at: usize) -> bool {
        let mut p = Parser {
            pattern: self.pattern,
            at,
            depth: 0,
        };
        !matches!(p.counted(), Ok(None))
    }

    /// A `{m}`, `{m,}` or `{m,n}` here (consumed), or `None` when the `{` is
    /// a literal one.
    fn counted(&mut self) -> Result<Option<(u32, Option<u32>)>, RegexError> {
        let start = self.at;
        let rest = &self.pattern[start..];
        let Some(close) = rest.find('}') else {
            return Ok(None);
        };
        let body = &rest[1..close];
        let (lo, hi) = match body.split_once(',') {
            Some((lo, hi)) => (lo, Some(hi)),
            None => (body, None),
        };
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !digits(lo) || hi.is_some_and(|h| !h.is_empty() && !digits(h)) {
            return Ok(None);
        }
        let too_big = RegexError {
            offset: start,
            message: "a repetition count is over 1000",
        };
        let number = |s: &str| {
            s.parse::<u32>()
                .ok()
                .filter(|&n| n <= MAX_REPEAT)
                .ok_or(too_big.clone())
        };
        let min = number(lo)?;
        let max = match hi {
            None => Some(min),
            Some("") => None,
            Some(h) => Some(number(h)?),
        };
        if max.is_some_and(|m| m < min) {
            return Err(RegexError {
                offset: start,
                message: "a repetition's {min,max} has max below min",
            });
        }
        self.at = start + close + 1;
        Ok(Some((min, max)))
    }

    fn atom(&mut self) -> Result<Node, RegexError> {
        let start = self.at;
        let Some(c) = self.bump() else {
            return Ok(Node::Empty);
        };
        Ok(match c {
            '(' => {
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return Err(RegexError {
                        offset: start,
                        message: "groups nest too deep",
                    });
                }
                if self.pattern[self.at..].starts_with("?:") {
                    self.at += 2;
                }
                let inner = self.alternation()?;
                if !self.eat(')') {
                    return Err(RegexError {
                        offset: start,
                        message: "unclosed group",
                    });
                }
                self.depth -= 1;
                inner
            }
            ')' => {
                return Err(RegexError {
                    offset: start,
                    message: "unopened group",
                });
            }
            '[' => Node::Class(self.class(start)?),
            '.' => Node::Any,
            '^' => Node::LineStart,
            '$' => Node::LineEnd,
            '*' | '+' | '?' => {
                return Err(RegexError {
                    offset: start,
                    message: "nothing to repeat",
                });
            }
            '{' if self.counted_at(start) => {
                return Err(RegexError {
                    offset: start,
                    message: "nothing to repeat",
                });
            }
            '\\' => match self.escape(start)? {
                Escape::Char(c) => Node::Char(c),
                Escape::Item(item) => Node::Class(Class {
                    negated: false,
                    items: vec![item],
                }),
            },
            c => Node::Char(c),
        })
    }

    /// What follows a `\` (consumed): a character, or a class like `\d`.
    fn escape(&mut self, start: usize) -> Result<Escape, RegexError> {
        let Some(c) = self.bump() else {
            return Err(RegexError {
                offset: start,
                message: "a pattern can't end with \\",
            });
        };
        Ok(match c {
            'd' => Escape::Item(ClassItem::Digit(false)),
            'D' => Escape::Item(ClassItem::Digit(true)),
            'w' => Escape::Item(ClassItem::Word(false)),
            'W' => Escape::Item(ClassItem::Word(true)),
            's' => Escape::Item(ClassItem::Space(false)),
            'S' => Escape::Item(ClassItem::Space(true)),
            't' => Escape::Char('\t'),
            'n' => Escape::Char('\n'),
            'r' => Escape::Char('\r'),
            c if !c.is_alphanumeric() => Escape::Char(c),
            _ => {
                return Err(RegexError {
                    offset: start,
                    message: "unknown escape",
                });
            }
        })
    }

    /// A class, after its `[` (at `start`).
    fn class(&mut self, start: usize) -> Result<Class, RegexError> {
        let unclosed = RegexError {
            offset: start,
            message: "unclosed character class",
        };
        let negated = self.eat('^');
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let at = self.at;
            let c = self.bump().ok_or(unclosed.clone())?;
            // A `]` first is a literal one.
            if c == ']' && !first {
                break;
            }
            first = false;
            let low = match c {
                '\\' => match self.escape(at)? {
                    Escape::Char(c) => c,
                    Escape::Item(item) => {
                        items.push(item);
                        continue;
                    }
                },
                c => c,
            };
            // A range, unless the `-` ends the class.
            if self.peek() == Some('-') && !self.pattern[self.at + 1..].starts_with(']') {
                self.bump();
                let at = self.at;
                let high = match self.bump().ok_or(unclosed.clone())? {
                    '\\' => match self.escape(at)? {
                        Escape::Char(c) => c,
                        Escape::Item(_) => {
                            return Err(RegexError {
                                offset: at,
                                message: "a range can't end in a class",
                            });
                        }
                    },
                    c => c,
                };
                if high < low {
                    return Err(RegexError {
                        offset: at,
                        message: "a range's end is before its start",
                    });
                }
                items.push(ClassItem::Range(low, high));
            } else {
                items.push(ClassItem::Range(low, low));
            }
        }
        Ok(Class { negated, items })
    }
}

enum Escape {
    Char(char),
    Item(ClassItem),
}

/// Compiles a tree to a program, within [`MAX_PROGRAM`] instructions.
struct Compiler {
    program: Vec<Inst>,
    fold: bool,
}

impl Compiler {
    fn emit(&mut self, inst: Inst) -> Result<usize, RegexError> {
        if self.program.len() >= MAX_PROGRAM {
            return Err(RegexError {
                offset: 0,
                message: "the pattern is too large",
            });
        }
        self.program.push(inst);
        Ok(self.program.len() - 1)
    }

    fn patch(&mut self, at: usize, to: usize) {
        match &mut self.program[at] {
            Inst::Split(_, b) => *b = to,
            Inst::Jump(t) => *t = to,
            _ => {}
        }
    }

    fn node(&mut self, node: &Node) -> Result<(), RegexError> {
        match node {
            Node::Empty => {}
            Node::Char(c) => {
                let c = if self.fold { lower(*c) } else { *c };
                self.emit(Inst::Char(c))?;
            }
            Node::Any => {
                self.emit(Inst::Any)?;
            }
            Node::Class(class) => {
                self.emit(Inst::Class(Box::new(class.clone())))?;
            }
            Node::LineStart => {
                self.emit(Inst::LineStart)?;
            }
            Node::LineEnd => {
                self.emit(Inst::LineEnd)?;
            }
            Node::Concat(items) => {
                for item in items {
                    self.node(item)?;
                }
            }
            Node::Alt(alts) => {
                let mut jumps = Vec::new();
                for (i, alt) in alts.iter().enumerate() {
                    if i + 1 < alts.len() {
                        let split = self.emit(Inst::Split(0, 0))?;
                        self.program[split] = Inst::Split(split + 1, 0);
                        self.node(alt)?;
                        jumps.push(self.emit(Inst::Jump(0))?);
                        let next = self.program.len();
                        self.patch(split, next);
                    } else {
                        self.node(alt)?;
                    }
                }
                let end = self.program.len();
                for j in jumps {
                    self.patch(j, end);
                }
            }
            Node::Repeat {
                node,
                min,
                max,
                greedy,
            } => {
                for _ in 0..*min {
                    self.node(node)?;
                }
                match max {
                    // Then as many more as there are: a loop.
                    None => {
                        let split = self.emit(Inst::Split(0, 0))?;
                        self.node(node)?;
                        self.emit(Inst::Jump(split))?;
                        let out = self.program.len();
                        self.program[split] = self.split(split + 1, out, *greedy);
                    }
                    // Then up to `max - min` more, each optional.
                    Some(max) => {
                        let mut splits = Vec::new();
                        for _ in *min..*max {
                            splits.push(self.emit(Inst::Split(0, 0))?);
                            self.node(node)?;
                        }
                        let out = self.program.len();
                        for s in splits {
                            self.program[s] = self.split(s + 1, out, *greedy);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// A split going on at `body` or `out`, preferring `body` when greedy.
    fn split(&self, body: usize, out: usize, greedy: bool) -> Inst {
        if greedy {
            Inst::Split(body, out)
        } else {
            Inst::Split(out, body)
        }
    }
}

/// The threads at one position: their instructions (in priority order) and
/// where each one's match started, as a sparse set over the program.
struct Threads {
    dense: Vec<(usize, usize)>,
    sparse: Vec<usize>,
}

impl Threads {
    fn new(len: usize) -> Self {
        Self {
            dense: Vec::with_capacity(len),
            sparse: vec![0; len],
        }
    }

    fn contains(&self, pc: usize) -> bool {
        let i = self.sparse[pc];
        i < self.dense.len() && self.dense[i].0 == pc
    }

    fn insert(&mut self, pc: usize, start: usize) {
        self.sparse[pc] = self.dense.len();
        self.dense.push((pc, start));
    }

    fn clear(&mut self) {
        self.dense.clear();
    }
}

/// Where the closure of the threads is visited: every instruction once.
struct Visit {
    /// Instructions reached at this position (a sparse set, as in
    /// [`Threads`]).
    seen: Threads,
    stack: Vec<usize>,
    /// The work done, for the tests that check it stays linear.
    steps: u64,
}

impl Regex {
    /// Compile `pattern`, matching case exactly or not.
    pub(crate) fn new(pattern: &str, case_sensitive: bool) -> Result<Self, RegexError> {
        let mut parser = Parser {
            pattern,
            at: 0,
            depth: 0,
        };
        let tree = parser.alternation()?;
        if parser.at < pattern.len() {
            // Only a `)` stops the top level early.
            return Err(parser.error("unopened group"));
        }
        let mut compiler = Compiler {
            program: Vec::new(),
            fold: !case_sensitive,
        };
        compiler.node(&tree)?;
        compiler.emit(Inst::Match)?;
        Ok(Regex {
            program: compiler.program,
            fold: !case_sensitive,
        })
    }

    /// The non-overlapping matches in `line`, left to right. Empty matches
    /// aren't reported (there's nothing to highlight), but don't stop the
    /// search.
    pub(crate) fn find_all(&self, line: &str) -> Vec<Range<usize>> {
        self.find_all_in(&mut Vm::new(self.program.len()), line)
    }

    /// [`find_all`](Self::find_all), and how many steps the searches took
    /// (threads stepped and instructions visited): what stays linear.
    #[cfg(test)]
    pub(crate) fn find_all_steps(&self, line: &str) -> (Vec<Range<usize>>, u64) {
        let mut vm = Vm::new(self.program.len());
        let found = self.find_all_in(&mut vm, line);
        (found, vm.visit.steps)
    }

    /// The size of the program, in instructions.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.program.len()
    }

    fn find_all_in(&self, vm: &mut Vm, line: &str) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut from = 0;
        while from <= line.len() {
            let Some(m) = self.search(vm, line, from) else {
                break;
            };
            if m.end > m.start {
                from = m.end;
                out.push(m);
            } else {
                // Past the empty match, by a character.
                match line[m.start..].chars().next() {
                    Some(c) => from = m.start + c.len_utf8(),
                    None => break,
                }
            }
        }
        out
    }

    /// The first (leftmost-first) match in `line` starting at or after
    /// byte `from`.
    #[cfg(test)]
    pub(crate) fn find_at(&self, line: &str, from: usize) -> Option<Range<usize>> {
        self.search(&mut Vm::new(self.program.len()), line, from)
    }

    /// A Pike VM search: every thread steps over each character together,
    /// a new one starting at each position until a match is found; a match
    /// cuts the threads after it (they'd only give less preferred ones).
    fn search(&self, vm: &mut Vm, hay: &str, from: usize) -> Option<Range<usize>> {
        let Vm {
            current,
            next,
            visit,
        } = vm;
        current.clear();
        next.clear();
        visit.seen.clear();
        let mut matched: Option<Range<usize>> = None;
        let mut pos = from;
        loop {
            if matched.is_none() {
                self.add(visit, current, 0, pos, pos, hay);
            }
            if current.dense.is_empty() && (matched.is_some() || pos >= hay.len()) {
                break;
            }
            let c = hay[pos..].chars().next();
            visit.seen.clear();
            for &(pc, start) in &current.dense {
                visit.steps += 1;
                let ok = match (&self.program[pc], c) {
                    (Inst::Match, _) => {
                        matched = Some(start..pos);
                        break;
                    }
                    (Inst::Char(want), Some(c)) => {
                        let c = if self.fold { lower(c) } else { c };
                        c == *want
                    }
                    (Inst::Any, Some(_)) => true,
                    (Inst::Class(class), Some(c)) => class.matches(c, self.fold),
                    // The end of the line, or (never: `add` keeps only
                    // these) an instruction that consumes nothing.
                    _ => false,
                };
                if let (true, Some(c)) = (ok, c) {
                    self.add(visit, next, pc + 1, pos + c.len_utf8(), start, hay);
                }
            }
            let Some(c) = c else { break };
            std::mem::swap(current, next);
            next.clear();
            pos += c.len_utf8();
        }
        matched
    }

    /// Add the thread at `pc` (its match started at `start`) to `list` at
    /// position `pos`, following jumps, splits (the preferred way first)
    /// and the anchors that hold there; each instruction once a position.
    fn add(
        &self,
        visit: &mut Visit,
        list: &mut Threads,
        pc: usize,
        pos: usize,
        start: usize,
        hay: &str,
    ) {
        visit.stack.push(pc);
        while let Some(pc) = visit.stack.pop() {
            if visit.seen.contains(pc) || list.contains(pc) {
                continue;
            }
            visit.steps += 1;
            visit.seen.insert(pc, start);
            match self.program[pc] {
                Inst::Jump(to) => visit.stack.push(to),
                Inst::Split(a, b) => {
                    visit.stack.push(b);
                    visit.stack.push(a);
                }
                Inst::LineStart => {
                    if pos == 0 {
                        visit.stack.push(pc + 1);
                    }
                }
                Inst::LineEnd => {
                    if pos == hay.len() {
                        visit.stack.push(pc + 1);
                    }
                }
                _ => list.insert(pc, start),
            }
        }
    }
}

/// A search's working memory, reused across the searches in a line.
struct Vm {
    current: Threads,
    next: Threads,
    visit: Visit,
}

impl Vm {
    fn new(len: usize) -> Self {
        Self {
            current: Threads::new(len),
            next: Threads::new(len),
            visit: Visit {
                seen: Threads::new(len),
                stack: Vec::new(),
                steps: 0,
            },
        }
    }
}

#[cfg(test)]
#[path = "regex_tests.rs"]
mod tests;
