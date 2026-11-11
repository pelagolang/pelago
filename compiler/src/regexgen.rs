//! `@regex("pattern")`: a pattern checked and compiled while the program is compiled.
//!
//! The attribute goes on a struct whose fields are the named groups of the pattern:
//!
//! ```text
//! @regex("(?<year>\\d{4})-(?<month>\\d\\d)(-(?<day>\\d\\d))?")
//! struct date { year: uint16, month: uint8, day: uint8? }
//! ```
//!
//! This module reads the pattern (the same language and the same rules as the reference matcher in
//! `std/regex.pel`, which it mirrors step by step), checks the fields against the groups, and appends
//! ordinary Pelago source to the module (as `derive.rs` does): the compiled program as constant tables and
//! the functions `dateFind`, `dateFindAll` and `dateIsMatch`. A pattern that cannot work is an error at
//! the attribute; there is no pattern error at run time. Design: `docs/design/regex.md`.
//!
//! The generated code stands alone: it needs no import.

use crate::ast::*;
use crate::span::Span;
use std::fmt::Write;

#[derive(Default)]
pub struct Expansion {
    pub text: String,
    pub errors: Vec<(Span, String)>,
}

// Instruction codes (the same numbers as `std/regex.pel`).
const CHAR: i64 = 0;
const ANY: i64 = 1;
const CLASS: i64 = 2;
const SPLIT: i64 = 3;
const JMP: i64 = 4;
const SAVE: i64 = 5;
const ASSERT: i64 = 6;
const MATCH: i64 = 7;
/// A lookahead in the program: `x` is the number of its bit table (see `looks`), `y` is 1 for `(?!...)`.
const LOOK: i64 = 8;
/// A backreference: `x` is the group, `y` is 1 when case is ignored. Only the budgeted search runs it.
const BACKREF: i64 = 10;

/// A syntax tree node.
#[derive(Clone, Debug)]
enum Node {
    Empty,
    Byte(u8),
    Any(bool),
    Class(usize),
    Cat(Box<Node>, Box<Node>),
    Alt(Box<Node>, Box<Node>),
    Star(Box<Node>, bool),
    Plus(Box<Node>, bool),
    Quest(Box<Node>, bool),
    Group(Box<Node>, usize),
    Assert(i64),
    Repeat(Box<Node>, i64, i64, bool),
    Look(Box<Node>, bool),
    /// `\1`, `\k<name>`: the group, and whether case is ignored.
    Backref(usize, bool),
}

/// A compiled pattern: the tables the matcher in `std.regex` runs.
#[derive(Debug, Default)]
pub struct Program {
    pub op: Vec<i64>,
    pub x: Vec<i64>,
    pub y: Vec<i64>,
    pub ranges: Vec<i64>,
    pub classes: Vec<i64>,
    pub groups: usize,
    pub names: Vec<String>,
    /// Where the reversed program of each lookahead body starts (it ends at its own `MATCH`).
    pub looks: Vec<i64>,
    /// Some backreference could not be rewritten: the program needs the budgeted search.
    pub tier_b: bool,
    tree: Option<Node>,
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
    ranges: Vec<i64>,
    classes: Vec<i64>,
    groups: usize,
    names: Vec<String>,
    flags: u8,
    /// Groups whose `)` is not read yet; groups written inside a lookahead; how many lookaheads we are inside.
    open: Vec<usize>,
    look_groups: Vec<usize>,
    look_depth: usize,
}

type Res<T> = Result<T, String>;

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

impl<'a> Parser<'a> {
    fn peek(&self) -> i32 {
        self.s.get(self.pos).map_or(-1, |b| *b as i32)
    }

    fn range(&mut self, lo: i64, hi: i64) {
        self.ranges.push(lo);
        self.ranges.push(hi);
    }

    fn escape_ranges(&mut self, e: u8) -> bool {
        match e {
            b'd' => self.range(48, 57),
            b'w' => {
                self.range(48, 57);
                self.range(65, 90);
                self.range(95, 95);
                self.range(97, 122);
            }
            b's' => {
                self.range(9, 13);
                self.range(32, 32);
            }
            _ => return false,
        }
        true
    }

    fn escaped(e: u8) -> u8 {
        match e {
            b'n' => 10,
            b't' => 9,
            b'r' => 13,
            b'f' => 12,
            b'v' => 11,
            _ => e,
        }
    }

    fn byte_node(&mut self, b: u8) -> Node {
        let lower = if b.is_ascii_uppercase() { b + 32 } else { b };
        if self.flags & 1 == 1 && lower.is_ascii_lowercase() {
            let start = self.ranges.len();
            self.range((lower - 32) as i64, (lower - 32) as i64);
            self.range(lower as i64, lower as i64);
            self.classes.push(start as i64);
            self.classes.push(2);
            self.classes.push(0);
            return Node::Class(self.classes.len() / 3 - 1);
        }
        Node::Byte(b)
    }

    fn fold(&mut self, start: usize) {
        let end = self.ranges.len();
        let mut i = start;
        while i < end {
            let (lo, hi) = (self.ranges[i], self.ranges[i + 1]);
            if hi >= 97 && lo <= 122 {
                self.range(lo.max(97) - 32, hi.min(122) - 32);
            }
            if hi >= 65 && lo <= 90 {
                self.range(lo.max(65) + 32, hi.min(90) + 32);
            }
            i += 2;
        }
    }

    fn class(&mut self) -> Res<usize> {
        let start = self.ranges.len();
        let mut neg = 0;
        if self.peek() == 94 {
            neg = 1;
            self.pos += 1;
        }
        let mut first = true;
        loop {
            let c = self.peek();
            if c < 0 {
                return Err("the character class is not closed".into());
            }
            if c == 93 && !first {
                self.pos += 1;
                break;
            }
            first = false;
            self.pos += 1;
            let mut lo = c as i64;
            if c == 92 {
                let e = self.peek();
                if e < 0 {
                    return Err("a pattern cannot end with a backslash".into());
                }
                self.pos += 1;
                if self.escape_ranges(e as u8) {
                    continue;
                }
                lo = Self::escaped(e as u8) as i64;
            }
            let mut hi = lo;
            if self.peek() == 45 && self.pos + 1 < self.s.len() && self.s[self.pos + 1] != 93 {
                self.pos += 1;
                let mut h = self.peek() as i64;
                self.pos += 1;
                if h == 92 {
                    let e = self.peek();
                    if e < 0 {
                        return Err("a pattern cannot end with a backslash".into());
                    }
                    self.pos += 1;
                    h = Self::escaped(e as u8) as i64;
                }
                if h < lo {
                    return Err("a character range is backwards".into());
                }
                hi = h;
            }
            self.range(lo, hi);
        }
        if self.flags & 1 == 1 {
            self.fold(start);
        }
        let count = (self.ranges.len() - start) / 2;
        self.classes.push(start as i64);
        self.classes.push(count as i64);
        self.classes.push(neg);
        Ok(self.classes.len() / 3 - 1)
    }

    fn escape_node(&mut self, e: u8) -> Node {
        if matches!(e, b'd' | b'w' | b's' | b'D' | b'W' | b'S') {
            let start = self.ranges.len();
            self.escape_ranges(e.to_ascii_lowercase());
            self.classes.push(start as i64);
            self.classes.push(((self.ranges.len() - start) / 2) as i64);
            self.classes.push(if e >= 97 { 0 } else { 1 });
            return Node::Class(self.classes.len() / 3 - 1);
        }
        match e {
            b'b' => Node::Assert(2),
            b'B' => Node::Assert(3),
            b'A' => Node::Assert(0),
            b'z' => Node::Assert(1),
            _ => self.byte_node(Self::escaped(e)),
        }
    }

    fn number(&mut self) -> i64 {
        let mut n = -1i64;
        while (48..=57).contains(&self.peek()) {
            if n < 0 {
                n = 0;
            }
            if n < 100000 {
                n = n * 10 + (self.peek() as i64 - 48);
            }
            self.pos += 1;
        }
        n
    }

    fn atom(&mut self, depth: usize) -> Res<Node> {
        let c = self.peek();
        self.pos += 1;
        if c == 40 {
            return self.group(depth);
        }
        match c {
            91 => Ok(Node::Class(self.class()?)),
            46 => Ok(Node::Any(self.flags / 2 % 2 == 1)),
            94 => Ok(Node::Assert(if self.flags / 4 % 2 == 1 { 4 } else { 0 })),
            36 => Ok(Node::Assert(if self.flags / 4 % 2 == 1 { 5 } else { 1 })),
            92 => {
                let e = self.peek();
                if e < 0 {
                    return Err("a pattern cannot end with a backslash".into());
                }
                self.pos += 1;
                if (49..=57).contains(&e) {
                    let mut n = (e - 48) as usize;
                    if (48..=57).contains(&self.peek()) {
                        n = n * 10 + (self.peek() - 48) as usize;
                        self.pos += 1;
                    }
                    return self.backref_number(n);
                }
                if e == 107 {
                    if self.peek() != 60 {
                        return Err("`\\k` is followed by a group name in `<...>`".into());
                    }
                    let first = self.pos + 1;
                    let mut end = first;
                    while end < self.s.len() && is_word(self.s[end]) {
                        end += 1;
                    }
                    if end == first || end >= self.s.len() || self.s[end] != b'>' {
                        return Err("a group name is a word followed by `>`".into());
                    }
                    let name = String::from_utf8_lossy(&self.s[first..end]).into_owned();
                    self.pos = end + 1;
                    return self.backref_name(&name);
                }
                Ok(self.escape_node(e as u8))
            }
            42 | 43 | 63 => Err("a repeat has nothing to repeat".into()),
            _ => Ok(self.byte_node(c as u8)),
        }
    }

    /// A backreference to group `n`: the group must be closed already and not sit in a lookahead, and the reference
    /// must not sit in one either.
    fn backref_number(&mut self, n: usize) -> Res<Node> {
        if n > self.groups {
            return Err("a backreference to a group that is not defined before it".into());
        }
        if self.open.contains(&n) {
            return Err("a backreference inside the group it refers to".into());
        }
        if self.look_depth > 0 {
            return Err("a backreference inside a lookahead is not supported".into());
        }
        if self.look_groups.contains(&n) {
            return Err("a backreference to a group inside a lookahead is not supported".into());
        }
        Ok(Node::Backref(n, self.flags & 1 == 1))
    }

    fn backref_name(&mut self, name: &str) -> Res<Node> {
        match self.names.iter().position(|n| n == name) {
            Some(i) if i > 0 => self.backref_number(i),
            _ => Err("a backreference to a group name that is not defined before it".into()),
        }
    }

    fn group(&mut self, depth: usize) -> Res<Node> {
        let mut capture = true;
        let mut look = 0;
        let mut name = String::new();
        let saved = self.flags;
        if self.peek() == 63 {
            let k = self.s.get(self.pos + 1).map_or(-1, |b| *b as i32);
            if matches!(k, 105 | 115 | 109 | 45) {
                let mut at = self.pos + 1;
                let mut on = true;
                let mut flags = self.flags;
                while at < self.s.len() && self.s[at] != b')' && self.s[at] != b':' {
                    let bit = match self.s[at] {
                        b'i' => 1,
                        b's' => 2,
                        b'm' => 4,
                        b'-' if on => {
                            on = false;
                            0
                        }
                        _ => return Err("this flag is not supported (use i, s or m)".into()),
                    };
                    if bit > 0 {
                        let has = flags & bit != 0;
                        if on && !has {
                            flags |= bit;
                        }
                        if !on && has {
                            flags &= !bit;
                        }
                    }
                    at += 1;
                }
                if at >= self.s.len() {
                    return Err("a group is not closed".into());
                }
                self.flags = flags;
                self.pos = at + 1;
                if self.s[at] == b')' {
                    return Ok(Node::Empty);
                }
                capture = false;
            } else if k == 58 {
                capture = false;
                self.pos += 2;
            } else if k == 61 || k == 33 {
                capture = false;
                look = if k == 61 { 1 } else { 2 };
                self.pos += 2;
            } else if k == 60 || k == 80 {
                let mut at = self.pos + 2;
                if k == 80 && at < self.s.len() && self.s[at] == b'=' {
                    let first = at + 1;
                    let mut end = first;
                    while end < self.s.len() && is_word(self.s[end]) {
                        end += 1;
                    }
                    if end == first || end >= self.s.len() || self.s[end] != b')' {
                        return Err("a group name is a word followed by `)`".into());
                    }
                    let name = String::from_utf8_lossy(&self.s[first..end]).into_owned();
                    self.pos = end + 1;
                    return self.backref_name(&name);
                }
                if k == 80 {
                    if at >= self.s.len() || self.s[at] != b'<' {
                        return Err("this group syntax is not supported".into());
                    }
                    at += 1;
                } else if at < self.s.len() && (self.s[at] == b'=' || self.s[at] == b'!') {
                    return Err("lookbehind is not supported".into());
                }
                let first = at;
                while at < self.s.len() && is_word(self.s[at]) {
                    at += 1;
                }
                if at == first || at >= self.s.len() || self.s[at] != b'>' {
                    return Err("a group name is a word followed by `>`".into());
                }
                name = String::from_utf8_lossy(&self.s[first..at]).into_owned();
                if self.names.contains(&name) {
                    return Err("a group name is used twice".into());
                }
                self.pos = at + 1;
            } else {
                return Err("this group syntax is not supported".into());
            }
        }
        let mut index = 0;
        if capture {
            self.groups += 1;
            index = self.groups;
            while self.names.len() <= index {
                self.names.push(String::new());
            }
            self.names[index] = name;
            self.open.push(index);
            if self.look_depth > 0 {
                self.look_groups.push(index);
            }
        }
        if look > 0 {
            self.look_depth += 1;
        }
        let inner = self.alt(depth + 1)?;
        if self.peek() != 41 {
            return Err("a group is not closed".into());
        }
        self.pos += 1;
        if look > 0 {
            self.look_depth -= 1;
        }
        if capture {
            self.open.pop();
        }
        self.flags = saved;
        if look > 0 {
            return Ok(Node::Look(Box::new(inner), look == 2));
        }
        if !capture {
            return Ok(inner);
        }
        Ok(Node::Group(Box::new(inner), index))
    }

    fn repeat(&mut self, depth: usize) -> Res<Node> {
        let mut atom = self.atom(depth)?;
        loop {
            let c = self.peek();
            let (mut lo, mut hi) = (0i64, -1i64);
            let kind = match c {
                42 => 0,
                43 => 1,
                63 => 2,
                123 => {
                    let save = self.pos;
                    self.pos += 1;
                    let a = self.number();
                    if a >= 0 {
                        lo = a;
                        hi = a;
                        if self.peek() == 44 {
                            self.pos += 1;
                            hi = self.number();
                        }
                    }
                    if a < 0 || self.peek() != 125 {
                        self.pos = save;
                        break;
                    }
                    if hi >= 0 && hi < lo {
                        return Err("a repeat count is backwards".into());
                    }
                    if lo > 1000 || hi > 1000 {
                        return Err("a repeat count is too large".into());
                    }
                    3
                }
                _ => break,
            };
            self.pos += 1;
            let mut greedy = true;
            if self.peek() == 63 {
                greedy = false;
                self.pos += 1;
            }
            let b = Box::new(atom);
            atom = match kind {
                0 => Node::Star(b, greedy),
                1 => Node::Plus(b, greedy),
                2 => Node::Quest(b, greedy),
                _ => Node::Repeat(b, lo, hi, greedy),
            };
        }
        Ok(atom)
    }

    fn cat(&mut self, depth: usize) -> Res<Node> {
        let mut left: Option<Node> = None;
        loop {
            let c = self.peek();
            if c < 0 || c == 124 || c == 41 {
                break;
            }
            let right = self.repeat(depth)?;
            left = Some(match left {
                None => right,
                Some(l) => Node::Cat(Box::new(l), Box::new(right)),
            });
        }
        Ok(left.unwrap_or(Node::Empty))
    }

    fn alt(&mut self, depth: usize) -> Res<Node> {
        if depth > 200 {
            return Err("the pattern is nested too deeply".into());
        }
        let mut left = self.cat(depth)?;
        while self.peek() == 124 {
            self.pos += 1;
            let right = self.cat(depth)?;
            left = Node::Alt(Box::new(left), Box::new(right));
        }
        Ok(left)
    }
}

/// The most strings a group may have for tier A, and the most alternatives the rewritten pattern may have.
const MAX_STRINGS: usize = 256;
const MAX_BRANCHES: usize = 256;

fn has_backref(n: &Node) -> bool {
    first_backref(n).is_some()
}

/// The lowest group number that some backreference in the tree reads.
fn first_backref(n: &Node) -> Option<usize> {
    match n {
        Node::Backref(g, _) => Some(*g),
        Node::Cat(a, b) | Node::Alt(a, b) => match (first_backref(a), first_backref(b)) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        },
        Node::Star(a, _)
        | Node::Plus(a, _)
        | Node::Quest(a, _)
        | Node::Group(a, _)
        | Node::Repeat(a, ..)
        | Node::Look(a, _) => first_backref(a),
        _ => None,
    }
}

/// The body of the definition of group `g`, whether it always takes part in a match, and whether it sits inside a loop.
fn find_group(n: &Node, g: usize, mandatory: bool, looped: bool) -> Option<(&Node, bool, bool)> {
    match n {
        Node::Group(a, i) if *i == g => Some((a, mandatory, looped)),
        Node::Group(a, _) => find_group(a, g, mandatory, looped),
        Node::Cat(a, b) => {
            find_group(a, g, mandatory, looped).or_else(|| find_group(b, g, mandatory, looped))
        }
        Node::Alt(a, b) => {
            find_group(a, g, false, looped).or_else(|| find_group(b, g, false, looped))
        }
        Node::Star(a, _) => find_group(a, g, false, true),
        Node::Plus(a, _) => find_group(a, g, mandatory, true),
        Node::Quest(a, _) => find_group(a, g, false, looped),
        Node::Repeat(a, lo, hi, _) => {
            find_group(a, g, mandatory && *lo > 0, looped || *hi < 0 || *hi > 1)
        }
        Node::Look(a, _) => find_group(a, g, false, looped),
        _ => None,
    }
}

impl Parser<'_> {
    /// Every string a node can match, in the order a backtracking matcher would try them, without repeats; `None` for
    /// an infinite or too large set and for nodes that are not plain text (assertions, groups, backreferences).
    fn language(&self, n: &Node) -> Option<Vec<Vec<u8>>> {
        let capped = |v: Vec<Vec<u8>>| (v.len() <= MAX_STRINGS).then_some(v);
        let unique = |v: Vec<Vec<u8>>| {
            let mut out: Vec<Vec<u8>> = Vec::new();
            for s in v {
                if !out.contains(&s) {
                    out.push(s);
                }
            }
            out
        };
        match n {
            Node::Empty => Some(vec![Vec::new()]),
            Node::Byte(b) => Some(vec![vec![*b]]),
            Node::Any(nl) => Some(
                (0..=255u8)
                    .filter(|b| *nl || *b != 10)
                    .map(|b| vec![b])
                    .collect(),
            ),
            Node::Class(i) => {
                let (start, count, neg) = (
                    self.classes[*i * 3] as usize,
                    self.classes[*i * 3 + 1] as usize,
                    self.classes[*i * 3 + 2],
                );
                Some(
                    (0..=255u8)
                        .filter(|b| {
                            let inside = (0..count).any(|k| {
                                self.ranges[start + 2 * k] <= *b as i64
                                    && *b as i64 <= self.ranges[start + 2 * k + 1]
                            });
                            inside != (neg == 1)
                        })
                        .map(|b| vec![b])
                        .collect(),
                )
            }
            Node::Cat(a, b) => {
                let (la, lb) = (self.language(a)?, self.language(b)?);
                let mut out = Vec::new();
                for x in &la {
                    for y in &lb {
                        let mut s = x.clone();
                        s.extend_from_slice(y);
                        out.push(s);
                        if out.len() > 4 * MAX_STRINGS * MAX_STRINGS {
                            return None;
                        }
                    }
                }
                capped(unique(out))
            }
            Node::Alt(a, b) => {
                let mut out = self.language(a)?;
                out.extend(self.language(b)?);
                capped(unique(out))
            }
            Node::Quest(a, greedy) => {
                let l = self.language(a)?;
                let mut out = Vec::new();
                if !*greedy {
                    out.push(Vec::new());
                }
                out.extend(l);
                if *greedy {
                    out.push(Vec::new());
                }
                capped(unique(out))
            }
            Node::Star(a, _) | Node::Plus(a, _) => {
                let l = self.language(a)?;
                (l.len() == 1 && l[0].is_empty()).then_some(l)
            }
            Node::Repeat(a, lo, hi, greedy) => {
                if *hi < 0 {
                    let l = self.language(a)?;
                    return (l.len() == 1 && l[0].is_empty()).then_some(l);
                }
                let l = self.language(a)?;
                // `times[k]` holds every string of exactly `k` repeats.
                let mut times: Vec<Vec<Vec<u8>>> = vec![vec![Vec::new()]];
                for k in 1..=*hi as usize {
                    let mut next = Vec::new();
                    for x in &times[k - 1] {
                        for y in &l {
                            let mut s = x.clone();
                            s.extend_from_slice(y);
                            next.push(s);
                            if next.len() > 4 * MAX_STRINGS * MAX_STRINGS {
                                return None;
                            }
                        }
                    }
                    let next = unique(next);
                    if next.len() > MAX_STRINGS {
                        return None;
                    }
                    times.push(next);
                }
                let order: Vec<usize> = if *greedy {
                    (*lo as usize..=*hi as usize).rev().collect()
                } else {
                    (*lo as usize..=*hi as usize).collect()
                };
                let mut out = Vec::new();
                for k in order {
                    out.extend(times[k].clone());
                }
                capped(unique(out))
            }
            Node::Group(..) | Node::Assert(_) | Node::Look(..) | Node::Backref(..) => None,
        }
    }

    /// `bytes` as a node; with `ci`, letters match in both cases.
    fn literal_node(&mut self, bytes: &[u8], ci: bool) -> Node {
        let saved = self.flags;
        self.flags = if ci { saved | 1 } else { saved & !1 };
        let mut out: Option<Node> = None;
        for b in bytes {
            let n = self.byte_node(*b);
            out = Some(match out {
                None => n,
                Some(l) => Node::Cat(Box::new(l), Box::new(n)),
            });
        }
        self.flags = saved;
        out.unwrap_or(Node::Empty)
    }

    /// The tree with group `g` fixed to the string `s`: its definition matches exactly `s`, its backreferences too.
    fn fix_group(&mut self, n: &Node, g: usize, s: &[u8]) -> Node {
        let go = |p: &mut Self, n: &Node| p.fix_group(n, g, s);
        match n {
            Node::Group(_, i) if *i == g => Node::Group(Box::new(self.literal_node(s, false)), g),
            Node::Backref(i, ci) if *i == g => self.literal_node(s, *ci),
            Node::Cat(a, b) => Node::Cat(Box::new(go(self, a)), Box::new(go(self, b))),
            Node::Alt(a, b) => Node::Alt(Box::new(go(self, a)), Box::new(go(self, b))),
            Node::Star(a, x) => Node::Star(Box::new(go(self, a)), *x),
            Node::Plus(a, x) => Node::Plus(Box::new(go(self, a)), *x),
            Node::Quest(a, x) => Node::Quest(Box::new(go(self, a)), *x),
            Node::Group(a, i) => Node::Group(Box::new(go(self, a)), *i),
            Node::Repeat(a, lo, hi, x) => Node::Repeat(Box::new(go(self, a)), *lo, *hi, *x),
            Node::Look(a, neg) => Node::Look(Box::new(go(self, a)), *neg),
            other => other.clone(),
        }
    }

    /// Tier A: rewrites the backreferences of a tree into alternatives, or `None` when some group does not qualify. A
    /// group qualifies when it always takes part in a match, does not sit in a loop, and can match only a finite set of
    /// plain strings (at most `MAX_STRINGS`); the tree then becomes one copy per string with the group and its
    /// references fixed to it, in the order the backtracking matcher would try the strings. At most `MAX_BRANCHES`
    /// copies in all.
    fn tier_a(&mut self, n: Node, leaves: &mut usize) -> Option<Node> {
        let Some(g) = first_backref(&n) else {
            *leaves += 1;
            return (*leaves <= MAX_BRANCHES).then_some(n);
        };
        let (body, mandatory, looped) = find_group(&n, g, true, false)?;
        if !mandatory || looped {
            return None;
        }
        let strings = self.language(body)?;
        let mut branches = Vec::new();
        for s in &strings {
            let fixed = self.fix_group(&n, g, s);
            branches.push(self.tier_a(fixed, leaves)?);
        }
        let mut out = branches.pop()?;
        while let Some(b) = branches.pop() {
            out = Node::Alt(Box::new(b), Box::new(out));
        }
        Some(out)
    }
}

struct Emitter {
    op: Vec<i64>,
    x: Vec<i64>,
    y: Vec<i64>,
    /// Lookahead bodies met so far; each is emitted reversed after the main program.
    pending: Vec<Node>,
}

impl Emitter {
    fn emit(&mut self, op: i64, x: i64, y: i64) -> usize {
        self.op.push(op);
        self.x.push(x);
        self.y.push(y);
        self.op.len() - 1
    }

    fn here(&self) -> i64 {
        self.op.len() as i64
    }

    fn split_to(&mut self, split: usize, body: i64, after: i64, greedy: bool) {
        self.x[split] = if greedy { body } else { after };
        self.y[split] = if greedy { after } else { body };
    }

    /// Emits a node. A `rev` (reversed) body is matched from its end to its start, for the scan that gives the
    /// lookahead bit of every position; it has no capture slots.
    fn node(&mut self, n: &Node, rev: bool) {
        match n {
            Node::Empty => {}
            Node::Byte(b) => {
                self.emit(CHAR, *b as i64, 0);
            }
            Node::Any(nl) => {
                self.emit(ANY, *nl as i64, 0);
            }
            Node::Class(i) => {
                self.emit(CLASS, *i as i64, 0);
            }
            Node::Cat(a, b) => {
                if rev {
                    self.node(b, rev);
                    self.node(a, rev);
                } else {
                    self.node(a, rev);
                    self.node(b, rev);
                }
            }
            Node::Alt(a, b) => {
                let split = self.emit(SPLIT, 0, 0);
                self.x[split] = self.here();
                self.node(a, rev);
                let jmp = self.emit(JMP, 0, 0);
                self.y[split] = self.here();
                self.node(b, rev);
                self.x[jmp] = self.here();
            }
            Node::Star(a, greedy) => {
                let split = self.emit(SPLIT, 0, 0);
                let body = self.here();
                self.node(a, rev);
                self.emit(JMP, split as i64, 0);
                let after = self.here();
                self.split_to(split, body, after, *greedy);
            }
            Node::Plus(a, greedy) => {
                let body = self.here();
                self.node(a, rev);
                let split = self.emit(SPLIT, 0, 0);
                let after = self.here();
                self.split_to(split, body, after, *greedy);
            }
            Node::Quest(a, greedy) => {
                let split = self.emit(SPLIT, 0, 0);
                let body = self.here();
                self.node(a, rev);
                let after = self.here();
                self.split_to(split, body, after, *greedy);
            }
            Node::Group(a, i) => {
                if rev {
                    self.node(a, rev);
                } else {
                    self.emit(SAVE, 2 * *i as i64, 0);
                    self.node(a, rev);
                    self.emit(SAVE, 2 * *i as i64 + 1, 0);
                }
            }
            Node::Assert(k) => {
                self.emit(ASSERT, *k, 0);
            }
            Node::Backref(g, ci) => {
                self.emit(BACKREF, *g as i64, *ci as i64);
            }
            Node::Look(a, neg) => {
                self.emit(LOOK, self.pending.len() as i64, *neg as i64);
                self.pending.push((**a).clone());
            }
            Node::Repeat(a, lo, hi, greedy) => {
                for _ in 0..*lo {
                    self.node(a, rev);
                }
                if *hi < 0 {
                    let split = self.emit(SPLIT, 0, 0);
                    let body = self.here();
                    self.node(a, rev);
                    self.emit(JMP, split as i64, 0);
                    let after = self.here();
                    self.split_to(split, body, after, *greedy);
                } else {
                    let mut pending = Vec::new();
                    for _ in *lo..*hi {
                        pending.push(self.emit(SPLIT, 0, 0));
                        self.node(a, rev);
                    }
                    let after = self.here();
                    for p in pending {
                        self.split_to(p, p as i64 + 1, after, *greedy);
                    }
                }
            }
        }
    }
}

/// Reads and compiles a pattern into the tables of the matcher.
pub fn compile(pattern: &str) -> Result<Program, String> {
    let mut p = Parser {
        s: pattern.as_bytes(),
        pos: 0,
        ranges: Vec::new(),
        classes: Vec::new(),
        groups: 0,
        names: vec![String::new()],
        flags: 0,
        open: Vec::new(),
        look_groups: Vec::new(),
        look_depth: 0,
    };
    let root = p.alt(0)?;
    if p.pos < p.s.len() {
        return Err("an unmatched `)`".into());
    }
    let original = root.clone();
    // Backreferences: a group with a finite set of strings is rewritten into plain alternatives (tier A, linear).
    // Whatever is left over keeps its `BACKREF` instructions and needs the budgeted search (tier B).
    let root = if has_backref(&root) {
        let mut leaves = 0;
        p.tier_a(root.clone(), &mut leaves).unwrap_or(root)
    } else {
        root
    };
    let mut e = Emitter {
        op: Vec::new(),
        x: Vec::new(),
        y: Vec::new(),
        pending: Vec::new(),
    };
    e.emit(SAVE, 0, 0);
    e.node(&root, false);
    e.emit(SAVE, 1, 0);
    e.emit(MATCH, 0, 0);
    // The lookahead bodies, reversed, one after the other (a body may meet more lookaheads: they get later numbers).
    let mut looks = Vec::new();
    let mut i = 0;
    while i < e.pending.len() {
        looks.push(e.here());
        let body = e.pending[i].clone();
        e.node(&body, true);
        e.emit(MATCH, 0, 0);
        i += 1;
    }
    let e_has_backref = e.op.contains(&BACKREF);
    Ok(Program {
        op: e.op,
        x: e.x,
        y: e.y,
        ranges: p.ranges,
        classes: p.classes,
        groups: p.groups,
        names: p.names,
        looks,
        tier_b: e_has_backref,
        tree: Some(original),
    })
}

/// What the tree says about one group: whether it always takes part in a match, and how many bytes
/// it can match at most (`None` for unbounded), and whether it can only match digits.
struct Facts {
    mandatory: bool,
    max_len: Option<u64>,
    digits_only: bool,
}

impl Program {
    fn class_digits(&self, i: usize) -> bool {
        let (start, count, neg) = (
            self.classes[i * 3] as usize,
            self.classes[i * 3 + 1] as usize,
            self.classes[i * 3 + 2],
        );
        neg == 0
            && count > 0
            && (0..count)
                .all(|k| self.ranges[start + 2 * k] >= 48 && self.ranges[start + 2 * k + 1] <= 57)
    }

    /// Longest match of a node in bytes (`None`: unbounded) and whether it matches digits only.
    fn shape(&self, n: &Node) -> (Option<u64>, bool) {
        match n {
            Node::Empty | Node::Assert(_) | Node::Look(..) => (Some(0), true),
            Node::Backref(..) => (None, false),
            Node::Byte(b) => (Some(1), b.is_ascii_digit()),
            Node::Any(_) => (Some(1), false),
            Node::Class(i) => (Some(1), self.class_digits(*i)),
            Node::Cat(a, b) => {
                let (la, da) = self.shape(a);
                let (lb, db) = self.shape(b);
                (la.zip(lb).map(|(x, y)| x + y), da && db)
            }
            Node::Alt(a, b) => {
                let (la, da) = self.shape(a);
                let (lb, db) = self.shape(b);
                (la.zip(lb).map(|(x, y)| x.max(y)), da && db)
            }
            Node::Star(a, _) | Node::Plus(a, _) => {
                let (la, d) = self.shape(a);
                (if la == Some(0) { Some(0) } else { None }, d)
            }
            Node::Quest(a, _) => self.shape(a),
            Node::Group(a, _) => self.shape(a),
            Node::Repeat(a, _, hi, _) => {
                let (la, d) = self.shape(a);
                if *hi < 0 {
                    (if la == Some(0) { Some(0) } else { None }, d)
                } else {
                    (la.map(|l| l * *hi as u64), d)
                }
            }
        }
    }

    fn facts(&self, group: usize) -> Option<Facts> {
        fn walk(p: &Program, n: &Node, group: usize, mandatory: bool) -> Option<Facts> {
            match n {
                Node::Group(a, i) if *i == group => {
                    let (max_len, digits_only) = p.shape(a);
                    Some(Facts {
                        mandatory,
                        max_len,
                        digits_only,
                    })
                }
                Node::Group(a, _) => walk(p, a, group, mandatory),
                Node::Cat(a, b) => {
                    walk(p, a, group, mandatory).or_else(|| walk(p, b, group, mandatory))
                }
                Node::Alt(a, b) => walk(p, a, group, false).or_else(|| walk(p, b, group, false)),
                Node::Star(a, _) | Node::Quest(a, _) => walk(p, a, group, false),
                Node::Plus(a, _) => walk(p, a, group, mandatory),
                Node::Repeat(a, lo, _, _) => walk(p, a, group, mandatory && *lo > 0),
                Node::Look(a, _) => walk(p, a, group, false),
                _ => None,
            }
        }
        walk(self, self.tree.as_ref()?, group, true)
    }
}

/// The whole number types: name, largest value, and the method that narrows an `int` to it.
const WHOLE: [(&str, u128, Option<&str>); 7] = [
    ("int", i64::MAX as u128, None),
    ("int8", 127, Some("wrapToInt8")),
    ("int16", 32767, Some("wrapToInt16")),
    ("int32", 2147483647, Some("wrapToInt32")),
    ("uint8", 255, Some("wrapToByte")),
    ("uint16", 65535, Some("wrapToUint16")),
    ("uint32", 4294967295, Some("wrapToUint32")),
];

/// Writes the matcher of every top-level struct that carries `@regex("...")`.
pub fn expand(src: &str, file: &File) -> Expansion {
    let mut out = Expansion::default();
    for stmt in &file.stmts {
        let StmtKind::Item(item) = &stmt.kind else {
            continue;
        };
        for attr in &item.attrs {
            if attr.name.text(src) != "regex" {
                continue;
            }
            let ItemKind::Struct(decl) = &item.kind else {
                out.errors
                    .push((item.span, "`@regex` goes on a struct".to_string()));
                continue;
            };
            match one(src, attr, decl) {
                Ok(text) => out.text.push_str(&text),
                Err(message) => out.errors.push((attr.span, message)),
            }
        }
    }
    out
}

/// The text of a string literal without interpolation, escapes resolved.
fn literal(src: &str, e: &Expr) -> Option<String> {
    let ExprKind::Str(parts) = &e.kind else {
        return None;
    };
    let mut out = String::new();
    for part in parts {
        let StrPart::Text(span) = part else {
            return None;
        };
        let t = &src[span.start as usize..span.end as usize];
        let mut chars = t.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                c @ ('\\' | '"' | '#' | '\'') => out.push(c),
                _ => return None,
            }
        }
    }
    Some(out)
}

fn type_text<'a>(src: &'a str, t: &Type) -> &'a str {
    src[t.span.start as usize..t.span.end as usize].trim()
}

/// The matcher written for a pattern: the same steps as `std/regex.pel` (a Pike simulation), with every size known
/// at compile time: `@M@` states, `@S@` capture slots, no growable lists.
const TEMPLATE: &str = r##"
struct @N@Prog {
    op: [int; @M@]
    x: [int; @M@]
    y: [int; @M@]
    ranges: [int; @R@]
    classes: [int; @C@]
    looks: [int; @L@]
}

struct @N@Threads {
    pcs: [int; @M@]
    caps: [int; @MS@]
    count: int
}

fn @N@Program() -> @N@Prog = @N@Prog {
    op: @OP@,
    x: @X@,
    y: @Y@,
    ranges: @RANGES@,
    classes: @CLASSES@,
    looks: @LOOKS@,
}

fn @N@IsWord(b: int) -> bool = (b >= 48 && b <= 57) || (b >= 65 && b <= 90) || b == 95 || (b >= 97 && b <= 122)

fn @N@ClassHas(p: @N@Prog, index: int, b: int) -> bool {
    const start = p.classes[index * 3]
    const count = p.classes[index * 3 + 1]
    var inside = false
    var i = 0
    while i < count {
        if b >= p.ranges[start + 2 * i] && b <= p.ranges[start + 2 * i + 1] {
            inside = true
            break
        }
        i += 1
    }
    p.classes[index * 3 + 2] == 1 ? !inside : inside
}

fn @N@Add(p: @N@Prog, text: str, inout list: @N@Threads, inout mark: [int; @M@], pc: int, pos: int, caps: [int; @S@], gen: int, bits: [int]) {
    if mark[pc] == gen { return }
    mark[pc] = gen
    const op = p.op[pc]
    if op == 4 {
        @N@Add(p, text, inout list, inout mark, p.x[pc], pos, caps, gen, bits)
    } else if op == 3 {
        @N@Add(p, text, inout list, inout mark, p.x[pc], pos, caps, gen, bits)
        @N@Add(p, text, inout list, inout mark, p.y[pc], pos, caps, gen, bits)
    } else if op == 5 {
        var next = caps
        next[p.x[pc]] = pos
        @N@Add(p, text, inout list, inout mark, pc + 1, pos, next, gen, bits)
    } else if op == 6 {
        const kind = p.x[pc]
        const before = pos > 0 ? text.byteAt(pos - 1) to int : -1
        const after = pos < text.len() ? text.byteAt(pos) to int : -1
        var holds = false
        if kind == 0 {
            holds = pos == 0
        } else if kind == 1 {
            holds = pos == text.len()
        } else if kind == 4 {
            holds = pos == 0 || before == 10
        } else if kind == 5 {
            holds = pos == text.len() || after == 10
        } else {
            const edge = (before >= 0 && @N@IsWord(before)) != (after >= 0 && @N@IsWord(after))
            holds = kind == 2 ? edge : !edge
        }
        if holds { @N@Add(p, text, inout list, inout mark, pc + 1, pos, caps, gen, bits) }
    } else if op == 8 {
        const inside = bits[p.x[pc] * (text.len() + 1) + pos] == 1
        if inside != (p.y[pc] == 1) { @N@Add(p, text, inout list, inout mark, pc + 1, pos, caps, gen, bits) }
    } else {
        list.pcs[list.count] = pc
        var i = 0
        while i < @S@ {
            list.caps[list.count * @S@ + i] = caps[i]
            i += 1
        }
        list.count += 1
    }
}

/// One bit for every lookahead and every position: does the body match starting there? Each body is run once,
/// reversed, from the end of the text to the start, so the work is `O(text * states)` however far a body looks.
fn @N@Bits(p: @N@Prog, text: str) -> [int] {
    var bits: [int] = []
    const n1 = text.len() + 1
    var i = 0
    while i < @NL@ * n1 {
        bits.push!(0)
        i += 1
    }
    var k = @NL@ - 1
    while k >= 0 {
        var current = @N@Threads { pcs: [0; @M@], caps: [0; @MS@], count: 0 }
        var following = @N@Threads { pcs: [0; @M@], caps: [0; @MS@], count: 0 }
        var mark: [int; @M@] = [-1; @M@]
        var gen = 0
        var pos = text.len()
        while pos >= 0 {
            @N@Add(p, text, inout current, inout mark, p.looks[k], pos, [-1; @S@], gen, bits)
            const byte = pos > 0 ? text.byteAt(pos - 1) to int : -1
            var t = 0
            while t < current.count {
                const pc = current.pcs[t]
                const op = p.op[pc]
                var advance = false
                if op == 7 {
                    bits[k * n1 + pos] = 1
                } else if op == 0 {
                    advance = byte == p.x[pc]
                } else if op == 1 {
                    advance = byte >= 0 && (p.x[pc] == 1 || byte != 10)
                } else if op == 2 {
                    advance = byte >= 0 && @N@ClassHas(p, p.x[pc], byte)
                }
                if advance {
                    @N@Add(p, text, inout following, inout mark, pc + 1, pos - 1, [-1; @S@], gen + 1, bits)
                }
                t += 1
            }
            var n = 0
            while n < following.count {
                current.pcs[n] = following.pcs[n]
                n += 1
            }
            current.count = following.count
            following.count = 0
            gen += 1
            pos -= 1
        }
        k -= 1
    }
    bits
}

@RUN@
fn @N@Text(text: str, s: [int; @S@], g: int) -> string {
    if s[2 * g] < 0 { return "" }
    "#(text[s[2 * g]..s[2 * g + 1]])"
}

fn @N@Int(text: str, s: [int; @S@], g: int) -> int {
    if s[2 * g] < 0 { return 0 }
    var v = 0
    var i = s[2 * g]
    while i < s[2 * g + 1] {
        v = v * 10 + (text.byteAt(i) to int) - 48
        i += 1
    }
    v
}

fn @N@Of(text: str, s: [int; @S@]) -> @N@ {
@BODY@    @N@ { @BUILD@}
}

@API@"##;

/// The linear matcher of a pattern without a backreference left (or one that tier A rewrote).
const RUN_LINEAR: &str = r##"
/// The slots of the first match at or after `from` (`-2` in slot 0 when there is none).
fn @N@Run(p: @N@Prog, text: str, from: int, noEmptyAt: int, bits: [int]) -> [int; @S@] {
    var matched: [int; @S@] = [-2; @S@]
    var current = @N@Threads { pcs: [0; @M@], caps: [0; @MS@], count: 0 }
    var following = @N@Threads { pcs: [0; @M@], caps: [0; @MS@], count: 0 }
    var mark: [int; @M@] = [-1; @M@]
    var pos = from
    var gen = 0
    while pos <= text.len() {
        if matched[0] == -2 {
            @N@Add(p, text, inout current, inout mark, 0, pos, [-1; @S@], gen, bits)
        }
        if current.count == 0 && matched[0] != -2 { break }
        const byte = pos < text.len() ? text.byteAt(pos) to int : -1
        var t = 0
        while t < current.count {
            const pc = current.pcs[t]
            const op = p.op[pc]
            var advance = false
            const startAt = current.caps[t * @S@]
            if op == 7 && startAt == pos && pos == noEmptyAt {
                advance = false
            } else if op == 7 {
                var k = 0
                while k < @S@ {
                    matched[k] = current.caps[t * @S@ + k]
                    k += 1
                }
                break
            } else if op == 0 {
                advance = byte == p.x[pc]
            } else if op == 1 {
                advance = byte >= 0 && (p.x[pc] == 1 || byte != 10)
            } else if op == 2 {
                advance = byte >= 0 && @N@ClassHas(p, p.x[pc], byte)
            }
            if advance {
                var carried: [int; @S@] = [0; @S@]
                var k = 0
                while k < @S@ {
                    carried[k] = current.caps[t * @S@ + k]
                    k += 1
                }
                @N@Add(p, text, inout following, inout mark, pc + 1, pos + 1, carried, gen + 1, bits)
            }
            t += 1
        }
        /// The next step's threads become the current ones.
        var n = 0
        while n < following.count {
            current.pcs[n] = following.pcs[n]
            var k = 0
            while k < @S@ {
                current.caps[n * @S@ + k] = following.caps[n * @S@ + k]
                k += 1
            }
            n += 1
        }
        current.count = following.count
        following.count = 0
        gen += 1
        pos += 1
    }
    matched
}
"##;

const API_LINEAR: &str = r##"
pub fn @N@Find(text: str) -> @N@? {
    const p = @N@Program()
    const s = @N@Run(p, text, 0, -1, @N@Bits(p, text))
    if s[0] < 0 { return none }
    @N@Of(text, s)
}

pub fn @N@FindAll(text: str) -> [@N@] {
    const p = @N@Program()
    var out: [@N@] = []
    const bits = @N@Bits(p, text)
    var from = 0
    var noEmpty = -1
    while from <= text.len() {
        const s = @N@Run(p, text, from, noEmpty, bits)
        if s[0] < 0 { break }
        out.push!(@N@Of(text, s))
        from = s[1]
        noEmpty = s[1] > s[0] ? -1 : s[1]
    }
    out
}

pub fn @N@IsMatch(text: str) -> bool {
    const p = @N@Program()
    @N@Run(p, text, 0, -1, @N@Bits(p, text))[0] >= 0
}
"##;

/// The budgeted search of a pattern with a backreference left (tier B): the same steps as `backtrack` in
/// `std/regex.pel`. A state is the program position, the text position and the spans of the referenced groups
/// (`@REFS@`); each is explored once and `budget` counts them. The stack and the set of explored states grow with the
/// search, so the memory is bounded by the budget, not by the pattern.
const RUN_BUDGET: &str = r##"
fn @N@SkipsEmpty(mine: [int; @S@], pos: int, noEmptyAt: int) -> bool {
    if mine[0] == pos && pos == noEmptyAt { return true }
    false
}

fn @N@Key(pc: int, pos: int, empty: bool, mine: [int; @S@], refs: [int; @RL@]) -> string {
    var key = "#(pc),#(pos),#(empty ? 1 : 0)"
    var r = 0
    while r < @NR@ {
        key.pushStr!(",#(mine[2 * refs[r]]),#(mine[2 * refs[r] + 1])")
        r += 1
    }
    key
}

fn @N@BackrefLength(p: @N@Prog, text: str, pc: int, pos: int, mine: [int; @S@]) -> int {
    const from0 = mine[2 * p.x[pc]]
    const to0 = mine[2 * p.x[pc] + 1]
    if from0 < 0 || to0 < 0 || pos + (to0 - from0) > text.len() { return -1 }
    var q = 0
    while q < to0 - from0 {
        var a = text.byteAt(from0 + q) to int
        var b = text.byteAt(pos + q) to int
        if p.y[pc] == 1 {
            if a >= 65 && a <= 90 { a += 32 }
            if b >= 65 && b <= 90 { b += 32 }
        }
        if a != b { return -1 }
        q += 1
    }
    to0 - from0
}

fn @N@Accepts(p: @N@Prog, text: str, pc: int, pos: int) -> bool {
    const byte = pos < text.len() ? text.byteAt(pos) to int : -1
    const op = p.op[pc]
    if op == 0 { return byte == p.x[pc] }
    if op == 1 { return byte >= 0 && (p.x[pc] == 1 || byte != 10) }
    op == 2 && byte >= 0 && @N@ClassHas(p, p.x[pc], byte)
}

fn @N@Search(p: @N@Prog, text: str, from: int, noEmptyAt: int, bits: [int], inout budget: int, inout result: [int; @S@]) -> int {
    const refs: [int; @RL@] = @REFS@
    var seen: set<string> = set {}
    var start = from
    while start <= text.len() {
        var pcs: [int] = [0]
        var poss: [int] = [start]
        var caps: [int] = []
        var k = 0
        while k < @S@ {
            caps.push!(-1)
            k += 1
        }
        while pcs.len() > 0 {
            var pc = pcs[pcs.len() - 1]
            pcs.pop!()
            var pos = poss[poss.len() - 1]
            poss.pop!()
            var mine: [int; @S@] = [0; @S@]
            var j = @S@ - 1
            while j >= 0 {
                mine[j] = caps[caps.len() - 1]
                caps.pop!()
                j -= 1
            }
            while true {
                const empty = @N@SkipsEmpty(mine, pos, noEmptyAt)
                var key = @N@Key(pc, pos, empty, mine, refs)
                if key in seen { break }
                seen.insert!(sink key)
                if budget <= 0 { return 2 }
                budget -= 1
                const op = p.op[pc]
                if op == 7 {
                    if empty { break }
                    result = mine
                    return 1
                } else if op == 4 {
                    pc = p.x[pc]
                } else if op == 3 {
                    pcs.push!(p.y[pc])
                    poss.push!(pos)
                    var c = 0
                    while c < @S@ {
                        caps.push!(mine[c])
                        c += 1
                    }
                    pc = p.x[pc]
                } else if op == 5 {
                    mine[p.x[pc]] = pos
                    pc += 1
                } else if op == 6 {
                    if !@N@Holds(text, p.x[pc], pos) { break }
                    pc += 1
                } else if op == 8 {
                    if (bits[p.x[pc] * (text.len() + 1) + pos] == 1) == (p.y[pc] == 1) { break }
                    pc += 1
                } else if op == 10 {
                    const len = @N@BackrefLength(p, text, pc, pos, mine)
                    if len < 0 { break }
                    pos += len
                    pc += 1
                } else {
                    if !@N@Accepts(p, text, pc, pos) { break }
                    pos += 1
                    pc += 1
                }
            }
        }
        start += 1
    }
    0
}

fn @N@Holds(text: str, kind: int, pos: int) -> bool {
    const before = pos > 0 ? text.byteAt(pos - 1) to int : -1
    const after = pos < text.len() ? text.byteAt(pos) to int : -1
    if kind == 0 { return pos == 0 }
    if kind == 1 { return pos == text.len() }
    if kind == 4 { return pos == 0 || before == 10 }
    if kind == 5 { return pos == text.len() || after == 10 }
    const edge = (before >= 0 && @N@IsWord(before)) != (after >= 0 && @N@IsWord(after))
    kind == 2 ? edge : !edge
}
"##;

/// What a search that can give up returns: the match, no match, or the budget ran out (the caller decides what to do).
const API_BUDGET: &str = r##"
pub enum @N@Outcome {
    matched(value: @N@)
    nomatch
    gaveUp
}

//~ The first match; `budget` is the most states the search may visit (the same input always gives the same answer).
pub fn @N@Find(text: str, budget: int) -> @N@Outcome {
    const p = @N@Program()
    var left = budget
    var s: [int; @S@] = [-2; @S@]
    const status = @N@Search(p, text, 0, -1, @N@Bits(p, text), inout left, inout s)
    if status == 2 { return @N@Outcome.gaveUp }
    if status == 0 { return @N@Outcome.nomatch }
    @N@Outcome.matched(value: @N@Of(text, s))
}

//~ Every match, or `none` when the search needed more than `budget` states in all.
pub fn @N@FindAll(text: str, budget: int) -> [@N@]? {
    const p = @N@Program()
    const bits = @N@Bits(p, text)
    var out: [@N@] = []
    var left = budget
    var from = 0
    var noEmpty = -1
    while from <= text.len() {
        var s: [int; @S@] = [-2; @S@]
        const status = @N@Search(p, text, from, noEmpty, bits, inout left, inout s)
        if status == 2 { return none }
        if status == 0 { break }
        out.push!(@N@Of(text, s))
        from = s[1]
        noEmpty = s[1] > s[0] ? -1 : s[1]
    }
    out
}

//~ Whether the pattern matches, or `none` when the search needed more than `budget` states.
pub fn @N@IsMatch(text: str, budget: int) -> bool? {
    const p = @N@Program()
    var left = budget
    var s: [int; @S@] = [-2; @S@]
    const status = @N@Search(p, text, 0, -1, @N@Bits(p, text), inout left, inout s)
    if status == 2 { return none }
    status == 1
}
"##;

fn one(src: &str, attr: &Attr, decl: &TypeDecl) -> Result<String, String> {
    if attr.args.is_empty() {
        return Err("`@regex` needs the pattern as a string: `@regex(\"...\")`".into());
    }
    let pattern = literal(src, &attr.args[0])
        .ok_or("the pattern of `@regex` must be a plain string literal")?;
    let program = compile(&pattern).map_err(|m| format!("regex: {m}"))?;
    let tname = decl.name.text(src).to_string();
    let mut fields = Vec::new();
    for m in &decl.members {
        if let Member::Field(f) = m {
            fields.push(f);
        }
    }
    if fields.is_empty() {
        return Err("a `@regex` struct needs one field for each named group it reads".into());
    }
    // One statement per field, filling a local named after the field.
    let mut body = String::new();
    let mut build = String::new();
    for f in fields {
        let fname = f.name.text(src);
        let group = program
            .names
            .iter()
            .position(|n| n == fname)
            .filter(|g| *g > 0)
            .ok_or_else(|| {
                format!("the field `{fname}` has no group `(?<{fname}>...)` in the pattern")
            })?;
        let facts = program.facts(group).ok_or("internal: group not found")?;
        let full = type_text(src, &f.ty);
        let (base, optional) = match &f.ty.kind {
            TypeKind::Option(inner) => (type_text(src, inner), true),
            _ => (full, false),
        };
        if !optional && !facts.mandatory {
            return Err(format!(
                "the group `{fname}` may not take part in a match, so the field must be `{base}?`"
            ));
        }
        let read = if base == "string" {
            format!("{tname}Text(text, s, {group})")
        } else if let Some((_, max, wrap)) = WHOLE.iter().find(|w| w.0 == base) {
            if !facts.digits_only {
                return Err(format!(
                    "the group `{fname}` is read as `{base}` but can match more than digits"
                ));
            }
            let Some(len) = facts.max_len else {
                return Err(format!(
                    "the group `{fname}` is read as `{base}` but has no largest length: write `\\d{{1,N}}` instead of `\\d+`"
                ));
            };
            let largest = 10u128.checked_pow(len as u32).map_or(u128::MAX, |v| v - 1);
            if largest > *max {
                return Err(format!(
                    "the group `{fname}` can match {len} digits, up to {largest}, but `{base}` holds at most {max}"
                ));
            }
            match wrap {
                Some(w) => format!("{tname}Int(text, s, {group}).{w}()"),
                None => format!("{tname}Int(text, s, {group})"),
            }
        } else {
            return Err(format!(
                "the field `{fname}` has type `{base}`: a group can be read as `string` or a whole number type"
            ));
        };
        if optional {
            let _ = writeln!(
                body,
                "    var {fname}: {full} = none\n    if s[{}] >= 0 {{ {fname} = {read} }}",
                2 * group
            );
        } else {
            let _ = writeln!(body, "    const {fname}: {full} = {read}");
        }
        let _ = write!(build, "{fname}: {fname}, ");
    }
    let n = &tname;
    let list = |v: &[i64]| -> (String, usize) {
        let mut v = v.to_vec();
        // At least two entries: a one-element array trips the native backend (see docs/design/regex.md).
        while v.len() < 2 {
            v.push(0);
        }
        let items: Vec<String> = v.iter().map(|x| x.to_string()).collect();
        (format!("[{}]", items.join(", ")), v.len())
    };
    let (op, m) = list(&program.op);
    let (x, _) = list(&program.x);
    let (y, _) = list(&program.y);
    let (ranges, r) = list(&program.ranges);
    let (classes, c) = list(&program.classes);
    let (looks, l) = list(&program.looks);
    let slots = 2 * (program.groups + 1);
    let mut refs: Vec<i64> = Vec::new();
    for (pc, op) in program.op.iter().enumerate() {
        if *op == BACKREF && !refs.contains(&program.x[pc]) {
            refs.push(program.x[pc]);
        }
    }
    let (refs_text, rl) = list(&refs);
    let (run, api) = if program.tier_b {
        (RUN_BUDGET, API_BUDGET)
    } else {
        (RUN_LINEAR, API_LINEAR)
    };
    let text = TEMPLATE
        .replace("@RUN@", run)
        .replace("@API@", api)
        .replace("@N@", n)
        .replace("@REFS@", &refs_text)
        .replace("@RL@", &rl.to_string())
        .replace("@NR@", &refs.len().to_string())
        .replace("@M@", &m.to_string())
        .replace("@S@", &slots.to_string())
        .replace("@MS@", &(m * slots).to_string())
        .replace("@R@", &r.to_string())
        .replace("@C@", &c.to_string())
        .replace("@L@", &l.to_string())
        .replace("@NL@", &program.looks.len().to_string())
        .replace("@OP@", &op)
        .replace("@X@", &x)
        .replace("@Y@", &y)
        .replace("@RANGES@", &ranges)
        .replace("@CLASSES@", &classes)
        .replace("@LOOKS@", &looks)
        .replace("@BODY@", &body)
        .replace("@BUILD@", &build);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\n/// `@regex` on `{n}`: the pattern, compiled by the compiler into fixed-size tables ({m} states, {slots} slots)."
    );
    out.push_str(&text);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expansion(src: &str) -> Expansion {
        let (out, _) = crate::parser::parse_source(src);
        expand(src, &out.file)
    }

    #[test]
    fn backreferences_are_sorted_into_tiers() {
        let tier = |p: &str| compile(p).unwrap().tier_b;
        // Finite groups that always take part: rewritten, no BACKREF left.
        assert!(!tier("(a|bc)\\1"));
        assert!(!tier("(\\w)\\1"));
        assert!(!tier("(?<q>['\"]).*?\\k<q>"));
        assert!(!tier("(a?)\\1b"));
        assert!(!tier("(\\w)(?:\\1)+"));
        assert!(!tier("(?i)(a)\\1"));
        // Unbounded, optional, in a loop, nested, or too many strings: the budgeted search.
        assert!(tier("(a+)\\1"));
        assert!(tier("(a)?\\1"));
        assert!(tier("(?:(a|b)\\1)*"));
        assert!(tier("(.)(.)\\2\\1"));
        assert!(tier("(<(\\w+)>)\\2"));
        // The rewritten pattern has no BACKREF instruction and a tier B one keeps it.
        assert!(!compile("(a|b)\\1").unwrap().op.contains(&BACKREF));
        assert!(compile("(a+)\\1").unwrap().op.contains(&BACKREF));
        for bad in [
            "\\1(a)",
            "(a\\1)",
            "(a)\\2",
            "(?=(a))\\1",
            "(?<x>a)(?=\\k<x>)",
            "\\k<x>",
            "(?P=x)(?P<x>a)",
        ] {
            assert!(compile(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_pattern_becomes_tables_and_functions() {
        let e = expansion(
            "@regex(\"(?<y>\\\\d{4})-(?<m>\\\\d\\\\d)(-(?<d>\\\\d\\\\d))?\")\nstruct date { y: uint16, m: uint8, d: uint8? }",
        );
        assert!(e.errors.is_empty(), "{:?}", e.errors);
        assert!(e.text.contains("pub fn dateFind("));
        assert!(e.text.contains("struct dateProg"));
        assert!(e.text.contains("s[8] >= 0"));
    }

    fn error_of(src: &str) -> String {
        let e = expansion(src);
        assert_eq!(e.errors.len(), 1, "{:?}", e.errors);
        e.errors[0].1.clone()
    }

    #[test]
    fn a_field_without_a_group_is_an_error() {
        let m = error_of("@regex(\"(?<a>x)\")\nstruct t { b: string }");
        assert!(m.contains("no group"), "{m}");
    }

    #[test]
    fn a_group_that_may_skip_needs_an_option_field() {
        let m = error_of("@regex(\"(?<a>x)|(?<b>y)\")\nstruct t { a: string, b: string? }");
        assert!(m.contains("may not take part"), "{m}");
    }

    #[test]
    fn a_number_that_can_overflow_is_refused() {
        let m = error_of("@regex(\"(?<n>\\\\d{3})\")\nstruct t { n: uint8 }");
        assert!(m.contains("up to 999"), "{m}");
        let m = error_of("@regex(\"(?<n>\\\\d+)\")\nstruct t { n: int }");
        assert!(m.contains("no largest length"), "{m}");
        let m = error_of("@regex(\"(?<n>\\\\w\\\\d)\")\nstruct t { n: int }");
        assert!(m.contains("more than digits"), "{m}");
    }

    #[test]
    fn a_bad_pattern_is_an_error() {
        let m = error_of("@regex(\"(a\")\nstruct t { a: string }");
        assert!(m.contains("not closed"), "{m}");
    }

    #[test]
    fn the_tables_match_the_reference_compiler_in_shape() {
        let p = compile("a(b|c)+").unwrap();
        // SAVE 0, CHAR a, SAVE 2, SPLIT, CHAR b, JMP, CHAR c, SAVE 3, SPLIT (plus), SAVE 1, MATCH
        assert_eq!(p.op.len(), 11);
        assert_eq!(p.groups, 1);
    }
}
