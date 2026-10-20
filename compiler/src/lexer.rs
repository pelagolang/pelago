//! The lexer: turns source text into tokens.
//!
//! It never fails. Bad input produces [`TokenKind::Error`] tokens plus entries in
//! [`LexOutput::errors`], and lexing carries on, so one mistake does not hide the rest.
//!
//! # What it produces
//!
//! * Hard keywords become [`TokenKind::Keyword`]. Soft keywords (`from`, `to`, `step`, ...)
//!   are plain identifiers here; the parser checks them where they matter.
//! * **Newlines are tokens.** Statements end at a newline. A run of blank lines is one
//!   [`TokenKind::Newline`]. The parser ignores newlines inside brackets and where a line
//!   obviously continues.
//! * **Comments are tokens** (trivia), so the formatter can keep them: `/// line`, `///* block *///`
//!   (nests), `//~ doc line`, and `//~* doc block *~//` (does not nest). A doc form never starts
//!   with `///`, so an editor that toggles `///` cannot half-strip it. `//` and `/* */` are
//!   errors with a hint, so a habit from another language does not silently become code.
//! * Strings are one token, including `#name` and `#(expression)` parts. The lexer only
//!   finds where the string ends (so `"a #(f("x")) b"` works); a later pass splits it.
//!
//! # Rules worth knowing
//!
//! * Operators use **longest match**. `>>` and `>=` are single tokens; the parser splits them
//!   when they close generic arguments.
//! * After a `.`, numbers are lexed as plain integers, so `t.0.1` is `t` `.` `0` `.` `1`,
//!   not a float. `1.max()` is `1` `.` `max`, and `1..10` is `1` `..` `10`.
//! * There is no leading-dot float: write `0.5`, not `.5`.
//! * A `#` inside a string must be followed by an identifier or `(`; write `\#` for a hash.
//!   Outside a string, `#` is an error.
//! * Identifiers may contain Unicode letters and digits.

use crate::keyword::Keyword;
use crate::span::Span;
use crate::token::{Punct, Token, TokenKind};

/// How deeply strings may nest inside `#(...)`. Stops runaway input.
const MAX_NESTING: u32 = 64;

/// What went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LexErrorKind {
    UnexpectedChar,
    HashOutsideString,
    UnterminatedString,
    UnterminatedInterpolation,
    UnterminatedBlockComment,
    NotAComment,
    BadCharLiteral,
    BadEscape,
    StrayHash,
    BadNumber,
    NestingTooDeep,
    FileTooLarge,
}

impl LexErrorKind {
    pub const fn message(self) -> &'static str {
        match self {
            LexErrorKind::UnexpectedChar => "unexpected character",
            LexErrorKind::HashOutsideString => "`#` only has a meaning inside a string",
            LexErrorKind::UnterminatedString => "this string is never closed",
            LexErrorKind::UnterminatedInterpolation => "this `#(` in a string is never closed",
            LexErrorKind::UnterminatedBlockComment => "this comment is never closed",
            LexErrorKind::NotAComment => {
                "`//` and `/* */` are not comments here; write `///` or `///* ... *///` (documentation: `//~` or `//~* ... *~//`)"
            }
            LexErrorKind::BadCharLiteral => {
                "a character literal holds exactly one character, like 'a'"
            }
            LexErrorKind::BadEscape => {
                "unknown escape; use \\n \\t \\r \\0 \\\\ \\' \\\" \\# or \\u{...}"
            }
            LexErrorKind::StrayHash => {
                "in a string, `#` must be followed by a name or `(`; write `\\#` for a literal hash"
            }
            LexErrorKind::BadNumber => "malformed number",
            LexErrorKind::NestingTooDeep => "strings are nested too deeply",
            LexErrorKind::FileTooLarge => "source files are limited to 4 GiB",
        }
    }
}

/// A problem found while lexing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError {
    pub span: Span,
    pub kind: LexErrorKind,
}

/// The result of lexing a file: always a token list ending in `Eof`, plus any errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexOutput {
    pub tokens: Vec<Token>,
    pub errors: Vec<LexError>,
}

/// Lexes `src`.
pub fn tokenize(src: &str) -> LexOutput {
    if src.len() > u32::MAX as usize {
        return LexOutput {
            tokens: vec![Token::new(TokenKind::Eof, Span::new(0, 0))],
            errors: vec![LexError {
                span: Span::new(0, 0),
                kind: LexErrorKind::FileTooLarge,
            }],
        };
    }
    Lexer {
        src,
        bytes: src.as_bytes(),
        pos: 0,
        tokens: Vec::new(),
        errors: Vec::new(),
    }
    .run()
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    tokens: Vec<Token>,
    errors: Vec<LexError>,
}

pub(crate) fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

pub(crate) fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

impl<'a> Lexer<'a> {
    // ---- Cursor helpers ---------------------------------------------------

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<u8> {
        self.bytes.get(self.pos + n).copied()
    }

    fn rest(&self) -> &'a str {
        self.src.get(self.pos..).unwrap_or("")
    }

    fn peek_char(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump_char(&mut self) -> Option<char> {
        let c = self.peek_char()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn at_ident_continue(&self) -> bool {
        self.peek_char().is_some_and(is_ident_continue)
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens
            .push(Token::new(kind, Span::new(start as u32, self.pos as u32)));
    }

    fn error(&mut self, start: usize, end: usize, kind: LexErrorKind) {
        self.errors.push(LexError {
            span: Span::new(start as u32, end as u32),
            kind,
        });
    }

    // ---- Main loop --------------------------------------------------------

    fn run(mut self) -> LexOutput {
        if self.rest().starts_with('\u{feff}') {
            self.pos += '\u{feff}'.len_utf8();
        }
        loop {
            while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | 0x0c)) {
                self.pos += 1;
            }
            let Some(b) = self.peek() else { break };
            let start = self.pos;
            match b {
                b'\n' => self.newline(),
                b'/' => self.slash(),
                b'"' => self.string(),
                b'\'' => self.char_literal(),
                b'0'..=b'9' => self.number(),
                _ if b == b'_' || b.is_ascii_alphabetic() => self.ident(),
                b'#' => {
                    self.pos += 1;
                    self.error(start, self.pos, LexErrorKind::HashOutsideString);
                    self.push(TokenKind::Error, start);
                }
                _ if b >= 0x80 && self.peek_char().is_some_and(is_ident_start) => self.ident(),
                _ => self.punct_or_error(),
            }
            if self.pos == start {
                // Guarantee progress, whatever happened above.
                self.bump_char();
                self.error(start, self.pos, LexErrorKind::UnexpectedChar);
                self.push(TokenKind::Error, start);
            }
        }
        let end = self.pos;
        self.tokens.push(Token::new(
            TokenKind::Eof,
            Span::new(end as u32, end as u32),
        ));
        LexOutput {
            tokens: self.tokens,
            errors: self.errors,
        }
    }

    // ---- Whitespace and comments -------------------------------------------

    fn newline(&mut self) {
        let start = self.pos;
        self.pos += 1;
        let mut end = self.pos;
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | 0x0c) => self.pos += 1,
                Some(b'\n') => {
                    self.pos += 1;
                    end = self.pos;
                }
                _ => break,
            }
        }
        let redundant = matches!(
            self.tokens.last(),
            None | Some(Token {
                kind: TokenKind::Newline,
                ..
            })
        );
        if !redundant {
            self.tokens.push(Token::new(
                TokenKind::Newline,
                Span::new(start as u32, end as u32),
            ));
        }
    }

    fn slash(&mut self) {
        let rest = self.rest();
        if rest.starts_with("//~*") {
            self.doc_block_comment();
        } else if rest.starts_with("//~") {
            self.line_comment(TokenKind::DocLineComment);
        } else if rest.starts_with("///*") {
            self.block_comment();
        } else if rest.starts_with("///") {
            self.line_comment(TokenKind::LineComment);
        } else if rest.starts_with("//") {
            // A leftover from another language: one error for the whole line.
            let start = self.pos;
            let len = rest.find('\n').unwrap_or(rest.len());
            self.pos += len;
            self.error(start, self.pos, LexErrorKind::NotAComment);
            self.push(TokenKind::Error, start);
        } else if rest.starts_with("/*") {
            let start = self.pos;
            self.pos += 2;
            self.error(start, self.pos, LexErrorKind::NotAComment);
            self.push(TokenKind::Error, start);
        } else {
            self.punct_or_error();
        }
    }

    fn line_comment(&mut self, kind: TokenKind) {
        let start = self.pos;
        let rest = self.rest();
        let mut len = rest.find('\n').unwrap_or(rest.len());
        if rest.as_bytes().get(len.wrapping_sub(1)) == Some(&b'\r') {
            len -= 1;
        }
        self.pos += len;
        self.push(kind, start);
    }

    /// `///* ... *///`, which nests.
    fn block_comment(&mut self) {
        let start = self.pos;
        let bytes = self.bytes;
        let mut i = self.pos + 4;
        let mut depth = 1u32;
        while i < bytes.len() {
            if bytes[i..].starts_with(b"///*") {
                depth += 1;
                i += 4;
            } else if bytes[i..].starts_with(b"*///") {
                depth -= 1;
                i += 4;
                if depth == 0 {
                    self.pos = i;
                    self.push(TokenKind::BlockComment, start);
                    return;
                }
            } else {
                i += 1;
            }
        }
        self.pos = bytes.len();
        self.error(start, self.pos, LexErrorKind::UnterminatedBlockComment);
        self.push(TokenKind::Error, start);
    }

    /// `//~* ... *~//`, which does not nest.
    fn doc_block_comment(&mut self) {
        let start = self.pos;
        let after_open = &self.rest()[4..];
        match after_open.find("*~//") {
            Some(i) => {
                self.pos += 4 + i + 4;
                self.push(TokenKind::DocBlockComment, start);
            }
            None => {
                self.pos = self.src.len();
                self.error(start, self.pos, LexErrorKind::UnterminatedBlockComment);
                self.push(TokenKind::Error, start);
            }
        }
    }

    // ---- Names ---------------------------------------------------------------

    fn ident(&mut self) {
        let start = self.pos;
        while self.at_ident_continue() {
            self.bump_char();
        }
        let text = &self.src[start..self.pos];
        let kind = if text == "_" {
            TokenKind::Underscore
        } else if let Some(kw) = Keyword::from_ident(text).filter(|k| k.is_hard()) {
            TokenKind::Keyword(kw)
        } else {
            TokenKind::Ident
        };
        self.push(kind, start);
    }

    // ---- Numbers ---------------------------------------------------------------

    /// Consumes digits and `_`. Returns (saw a digit, ended on an underscore).
    fn digits(&mut self, is_digit: impl Fn(u8) -> bool) -> (bool, bool) {
        let mut saw_digit = false;
        let mut trailing_underscore = false;
        while let Some(b) = self.peek() {
            if is_digit(b) {
                saw_digit = true;
                trailing_underscore = false;
            } else if b == b'_' {
                trailing_underscore = true;
            } else {
                break;
            }
            self.pos += 1;
        }
        (saw_digit, trailing_underscore)
    }

    fn number(&mut self) {
        let start = self.pos;
        let after_dot = matches!(
            self.tokens.last(),
            Some(Token { kind: TokenKind::Punct(Punct::Dot | Punct::QuestionDot), span })
                if span.end as usize == start
        );
        let mut kind = TokenKind::Int;
        let mut bad = false;
        let radix = if self.peek() == Some(b'0') {
            match self.peek_at(1) {
                Some(b'x' | b'X') => Some(16),
                Some(b'b' | b'B') => Some(2),
                Some(b'o' | b'O') => Some(8),
                _ => None,
            }
        } else {
            None
        };
        if let Some(radix) = radix {
            self.pos += 2;
            let (saw, trailing) = self.digits(|b| (b as char).is_digit(radix));
            bad = !saw || trailing;
        } else {
            let (_, trailing) = self.digits(|b| b.is_ascii_digit());
            bad |= trailing;
            if !after_dot {
                if self.peek() == Some(b'.') && self.peek_at(1).is_some_and(|b| b.is_ascii_digit())
                {
                    self.pos += 1;
                    let (_, trailing) = self.digits(|b| b.is_ascii_digit());
                    bad |= trailing;
                    kind = TokenKind::Float;
                }
                if matches!(self.peek(), Some(b'e' | b'E')) {
                    let sign = usize::from(matches!(self.peek_at(1), Some(b'+' | b'-')));
                    if self.peek_at(1 + sign).is_some_and(|b| b.is_ascii_digit()) {
                        self.pos += 1 + sign;
                        let (_, trailing) = self.digits(|b| b.is_ascii_digit());
                        bad |= trailing;
                        kind = TokenKind::Float;
                    }
                }
            }
        }
        if self.at_ident_continue() {
            while self.at_ident_continue() {
                self.bump_char();
            }
            bad = true;
        }
        if bad {
            self.error(start, self.pos, LexErrorKind::BadNumber);
            self.push(TokenKind::Error, start);
        } else {
            self.push(kind, start);
        }
    }

    // ---- Characters and strings ------------------------------------------------

    /// Consumes the rest of an escape, just after the backslash. Returns whether it was valid.
    fn escape(&mut self, in_string: bool) -> bool {
        let backslash = self.pos - 1;
        match self.peek_char() {
            Some('n' | 't' | 'r' | '0' | '\\' | '\'' | '"') => {
                self.pos += 1;
                true
            }
            Some('#') if in_string => {
                self.pos += 1;
                true
            }
            Some('u') => {
                self.pos += 1;
                let mut ok = false;
                if self.peek() == Some(b'{') {
                    self.pos += 1;
                    let digits_start = self.pos;
                    while self.peek().is_some_and(|b| b.is_ascii_hexdigit()) {
                        self.pos += 1;
                    }
                    let digits = &self.src[digits_start..self.pos];
                    if self.peek() == Some(b'}') {
                        self.pos += 1;
                        ok = (1..=6).contains(&digits.len())
                            && u32::from_str_radix(digits, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .is_some();
                    }
                }
                if !ok {
                    self.error(backslash, self.pos, LexErrorKind::BadEscape);
                }
                ok
            }
            Some(c) if c != '\n' => {
                self.pos += c.len_utf8();
                self.error(backslash, self.pos, LexErrorKind::BadEscape);
                false
            }
            _ => {
                self.error(backslash, self.pos, LexErrorKind::BadEscape);
                false
            }
        }
    }

    /// Consumes a character literal starting at the opening quote. Returns whether it was valid.
    fn char_body(&mut self) -> bool {
        let start = self.pos;
        self.pos += 1;
        let mut ok = true;
        match self.peek_char() {
            None | Some('\n') => {
                self.error(start, self.pos, LexErrorKind::BadCharLiteral);
                return false;
            }
            Some('\'') => ok = false,
            Some('\\') => {
                self.pos += 1;
                ok = self.escape(false);
            }
            Some(_) => {
                self.bump_char();
            }
        }
        if self.peek() == Some(b'\'') {
            self.pos += 1;
            if !ok {
                self.error(start, self.pos, LexErrorKind::BadCharLiteral);
            }
            ok
        } else {
            // Too many characters, or a missing quote: skip to a closing quote on this line.
            while let Some(c) = self.peek_char() {
                if c == '\n' {
                    break;
                }
                self.pos += c.len_utf8();
                if c == '\'' {
                    break;
                }
            }
            self.error(start, self.pos, LexErrorKind::BadCharLiteral);
            false
        }
    }

    fn char_literal(&mut self) {
        let start = self.pos;
        let ok = self.char_body();
        self.push(
            if ok {
                TokenKind::Char
            } else {
                TokenKind::Error
            },
            start,
        );
    }

    fn string(&mut self) {
        let start = self.pos;
        self.pos += 1;
        let terminated = self.string_body(start, 0);
        self.push(
            if terminated {
                TokenKind::Str
            } else {
                TokenKind::Error
            },
            start,
        );
    }

    /// Consumes a string after its opening quote. Returns whether the closing quote was found.
    fn string_body(&mut self, start: usize, depth: u32) -> bool {
        loop {
            match self.peek_char() {
                None => {
                    self.error(start, self.pos, LexErrorKind::UnterminatedString);
                    return false;
                }
                Some('"') => {
                    self.pos += 1;
                    return true;
                }
                Some('\\') => {
                    self.pos += 1;
                    self.escape(true);
                }
                Some('#') => {
                    let hash = self.pos;
                    self.pos += 1;
                    match self.peek_char() {
                        Some('(') => {
                            self.pos += 1;
                            if !self.interpolation(hash, depth) {
                                return false;
                            }
                        }
                        Some(c) if is_ident_start(c) => {}
                        _ => self.error(hash, self.pos, LexErrorKind::StrayHash),
                    }
                }
                Some(c) => self.pos += c.len_utf8(),
            }
        }
    }

    /// Consumes `...)` after a `#(` inside a string, allowing nested parentheses, strings
    /// and character literals. Returns whether the closing parenthesis was found.
    fn interpolation(&mut self, hash: usize, depth: u32) -> bool {
        if depth >= MAX_NESTING {
            self.error(hash, self.pos, LexErrorKind::NestingTooDeep);
            self.pos = self.src.len();
            return false;
        }
        let mut parens = 1u32;
        loop {
            match self.peek_char() {
                None => {
                    self.error(hash, self.pos, LexErrorKind::UnterminatedInterpolation);
                    return false;
                }
                Some('(') => {
                    self.pos += 1;
                    parens += 1;
                }
                Some(')') => {
                    self.pos += 1;
                    parens -= 1;
                    if parens == 0 {
                        return true;
                    }
                }
                Some('"') => {
                    let start = self.pos;
                    self.pos += 1;
                    if !self.string_body(start, depth + 1) {
                        return false;
                    }
                }
                Some('\'') => {
                    self.char_body();
                }
                Some(c) => self.pos += c.len_utf8(),
            }
        }
    }

    // ---- Operators and punctuation -------------------------------------------------

    fn punct_or_error(&mut self) {
        let start = self.pos;
        if let Some((punct, len)) = self.punct() {
            self.pos += len;
            self.push(TokenKind::Punct(punct), start);
        } else {
            self.bump_char();
            self.error(start, self.pos, LexErrorKind::UnexpectedChar);
            self.push(TokenKind::Error, start);
        }
    }

    /// Longest-match operator at the cursor, with its length in bytes.
    fn punct(&self) -> Option<(Punct, usize)> {
        use Punct::*;
        let c0 = self.peek()?;
        let c1 = self.peek_at(1).unwrap_or(0);
        let c2 = self.peek_at(2).unwrap_or(0);
        Some(match (c0, c1, c2) {
            // Three characters
            (b'.', b'.', b'.') => (DotDotDot, 3),
            (b'.', b'.', b'=') => (DotDotEq, 3),
            (b'<', b'<', b'=') => (ShlEq, 3),
            (b'>', b'>', b'=') => (ShrEq, 3),
            (b'?', b'?', b'=') => (QuestionQuestionEq, 3),
            (b'+', b'%', b'=') => (PlusPercentEq, 3),
            (b'-', b'%', b'=') => (MinusPercentEq, 3),
            (b'*', b'%', b'=') => (StarPercentEq, 3),
            // Two characters
            (b'*', b'*', _) => (StarStar, 2),
            (b'+', b'%', _) => (PlusPercent, 2),
            (b'-', b'%', _) => (MinusPercent, 2),
            (b'*', b'%', _) => (StarPercent, 2),
            (b'=', b'=', _) => (EqEq, 2),
            (b'!', b'=', _) => (NotEq, 2),
            (b'!', b'!', _) => (BangBang, 2),
            (b'<', b'=', _) => (LtEq, 2),
            (b'>', b'=', _) => (GtEq, 2),
            (b'<', b'<', _) => (Shl, 2),
            (b'>', b'>', _) => (Shr, 2),
            (b'&', b'&', _) => (AndAnd, 2),
            (b'|', b'|', _) => (OrOr, 2),
            (b'+', b'=', _) => (PlusEq, 2),
            (b'-', b'=', _) => (MinusEq, 2),
            (b'*', b'=', _) => (StarEq, 2),
            (b'/', b'=', _) => (SlashEq, 2),
            (b'%', b'=', _) => (PercentEq, 2),
            (b'&', b'=', _) => (AmpEq, 2),
            (b'|', b'=', _) => (PipeEq, 2),
            (b'^', b'=', _) => (CaretEq, 2),
            (b'+', b'+', _) => (PlusPlus, 2),
            (b'-', b'-', _) => (MinusMinus, 2),
            (b'.', b'.', _) => (DotDot, 2),
            (b'?', b'?', _) => (QuestionQuestion, 2),
            (b'?', b'.', _) => (QuestionDot, 2),
            (b'-', b'>', _) => (Arrow, 2),
            (b'~', b'>', _) => (TildeArrow, 2),
            (b'|', b'>', _) => (PipeGt, 2),
            // One character
            (b'+', _, _) => (Plus, 1),
            (b'-', _, _) => (Minus, 1),
            (b'*', _, _) => (Star, 1),
            (b'/', _, _) => (Slash, 1),
            (b'%', _, _) => (Percent, 1),
            (b'=', _, _) => (Eq, 1),
            (b'!', _, _) => (Bang, 1),
            (b'<', _, _) => (Lt, 1),
            (b'>', _, _) => (Gt, 1),
            (b'&', _, _) => (Amp, 1),
            (b'|', _, _) => (Pipe, 1),
            (b'^', _, _) => (Caret, 1),
            (b'~', _, _) => (Tilde, 1),
            (b'.', _, _) => (Dot, 1),
            (b'?', _, _) => (Question, 1),
            (b':', _, _) => (Colon, 1),
            (b',', _, _) => (Comma, 1),
            (b';', _, _) => (Semi, 1),
            (b'@', _, _) => (At, 1),
            (b'(', _, _) => (LParen, 1),
            (b')', _, _) => (RParen, 1),
            (b'[', _, _) => (LBracket, 1),
            (b']', _, _) => (RBracket, 1),
            (b'{', _, _) => (LBrace, 1),
            (b'}', _, _) => (RBrace, 1),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenKind::{
        BlockComment, Char, DocBlockComment, DocLineComment, Eof, Error, Float, Ident, Int,
        LineComment, Newline, Str, Underscore,
    };

    fn lex(src: &str) -> LexOutput {
        tokenize(src)
    }

    /// Token kinds, without the final `Eof`.
    fn kinds(src: &str) -> Vec<TokenKind> {
        let mut k: Vec<_> = lex(src).tokens.iter().map(|t| t.kind).collect();
        assert_eq!(k.pop(), Some(Eof));
        k
    }

    fn texts(src: &str) -> Vec<&str> {
        let out = lex(src);
        out.tokens
            .iter()
            .filter(|t| t.kind != Eof)
            .map(|t| t.text(src))
            .collect()
    }

    fn error_kinds(src: &str) -> Vec<LexErrorKind> {
        lex(src).errors.iter().map(|e| e.kind).collect()
    }

    fn punct(p: Punct) -> TokenKind {
        TokenKind::Punct(p)
    }

    fn kw(k: Keyword) -> TokenKind {
        TokenKind::Keyword(k)
    }

    #[test]
    fn empty_input() {
        assert_eq!(kinds(""), vec![]);
        assert_eq!(kinds("   \t "), vec![]);
        assert!(lex("").errors.is_empty());
    }

    #[test]
    fn hello_world() {
        let src = "fn main() {\n    print(\"Hello\")\n}\n";
        assert_eq!(
            kinds(src),
            vec![
                kw(Keyword::Fn),
                Ident,
                punct(Punct::LParen),
                punct(Punct::RParen),
                punct(Punct::LBrace),
                Newline,
                Ident,
                punct(Punct::LParen),
                Str,
                punct(Punct::RParen),
                Newline,
                punct(Punct::RBrace),
                Newline,
            ]
        );
        assert!(lex(src).errors.is_empty());
    }

    #[test]
    fn hard_and_soft_keywords() {
        assert_eq!(kinds("fn"), vec![kw(Keyword::Fn)]);
        assert_eq!(
            kinds("self Self"),
            vec![kw(Keyword::SelfValue), kw(Keyword::SelfType)]
        );
        // Soft keywords are plain identifiers for the lexer.
        assert_eq!(kinds("from to step it type init"), vec![Ident; 6]);
        assert_eq!(kinds("selfish Fn _x x1"), vec![Ident; 4]);
        // Words from other languages are plain identifiers.
        assert_eq!(kinds("class null try"), vec![Ident; 3]);
    }

    #[test]
    fn underscore() {
        assert_eq!(kinds("_ _x __ x_"), vec![Underscore, Ident, Ident, Ident]);
    }

    #[test]
    fn unicode_identifiers() {
        assert_eq!(kinds("héllo 日本語 naïve_1"), vec![Ident; 3]);
        assert_eq!(texts("héllo"), vec!["héllo"]);
        assert_eq!(error_kinds("🙂"), vec![LexErrorKind::UnexpectedChar]);
    }

    #[test]
    fn numbers() {
        let src = "42 1_000_000 0xFF 0b1010 0o17 3.14 1e3 1e-3 2.5e10 1_0.2_5";
        assert_eq!(
            kinds(src),
            vec![Int, Int, Int, Int, Int, Float, Float, Float, Float, Float]
        );
        assert!(lex(src).errors.is_empty());
        assert_eq!(texts("0xFF 1e-3"), vec!["0xFF", "1e-3"]);
        // `e` inside a hex number is a digit, not an exponent.
        assert_eq!(kinds("0xE5"), vec![Int]);
    }

    #[test]
    fn numbers_next_to_dots() {
        assert_eq!(kinds("1..10"), vec![Int, punct(Punct::DotDot), Int]);
        assert_eq!(kinds("1..=10"), vec![Int, punct(Punct::DotDotEq), Int]);
        assert_eq!(kinds("1.max"), vec![Int, punct(Punct::Dot), Ident]);
        assert_eq!(kinds("1.5.max"), vec![Float, punct(Punct::Dot), Ident]);
        // Tuple fields: after a dot, digits are a plain integer.
        assert_eq!(
            kinds("t.0.1"),
            vec![Ident, punct(Punct::Dot), Int, punct(Punct::Dot), Int]
        );
        // No leading-dot floats.
        assert_eq!(kinds(".5"), vec![punct(Punct::Dot), Int]);
    }

    #[test]
    fn bad_numbers() {
        for src in ["12abc", "0x", "0b12", "1_", "0o9", "1e", "0xFG"] {
            let out = lex(src);
            assert_eq!(out.errors.len(), 1, "{src:?}");
            assert_eq!(out.errors[0].kind, LexErrorKind::BadNumber, "{src:?}");
            assert_eq!(out.tokens[0].kind, Error, "{src:?}");
        }
    }

    #[test]
    fn characters() {
        assert_eq!(kinds(r"'a' '\n' '\\' '\'' '\u{1F600}' 'é'"), vec![Char; 6]);
        for src in [
            "''",
            "'ab'",
            r"'\q'",
            "'a",
            "'",
            r"'\u{}'",
            r"'\u{110000}'",
            r"'\#'",
        ] {
            let out = lex(src);
            assert!(!out.errors.is_empty(), "{src:?} should be an error");
            assert_eq!(out.tokens[0].kind, Error, "{src:?}");
        }
    }

    #[test]
    fn plain_strings() {
        assert_eq!(
            kinds(r#""hello" "" "a\nb\t\"q\" \\ \# \u{e9}""#),
            vec![Str, Str, Str]
        );
        assert_eq!(texts(r#""a b" x"#), vec![r#""a b""#, "x"]);
        // Strings may span lines.
        let src = "\"one\ntwo\"";
        assert_eq!(kinds(src), vec![Str]);
        assert!(lex(src).errors.is_empty());
    }

    #[test]
    fn interpolation() {
        let src = r##""Score: #score" "Next: #(score + 1)" "#(f("x")) and #(g('a', (1)))""##;
        assert_eq!(kinds(src), vec![Str, Str, Str]);
        assert!(lex(src).errors.is_empty(), "{:?}", lex(src).errors);
        // Interpolations can nest strings that have interpolations.
        assert_eq!(kinds(r##""#(f("#(g("deep"))"))""##), vec![Str]);
    }

    #[test]
    fn string_errors() {
        assert_eq!(
            error_kinds(r#""never closed"#),
            vec![LexErrorKind::UnterminatedString]
        );
        // The closing quote is read as the start of a nested string inside the `#(`.
        assert_eq!(
            error_kinds(r##""#(1 + 2""##),
            vec![LexErrorKind::UnterminatedString]
        );
        assert_eq!(
            error_kinds(r##""#(1 + 2"##),
            vec![LexErrorKind::UnterminatedInterpolation]
        );
        assert_eq!(
            error_kinds(r#""bad \q escape""#),
            vec![LexErrorKind::BadEscape]
        );
        assert_eq!(
            error_kinds(r##""item # 1""##),
            vec![LexErrorKind::StrayHash]
        );
        assert_eq!(
            error_kinds(r##""price #1""##),
            vec![LexErrorKind::StrayHash]
        );
        // A stray hash is reported, but the string is still one string token.
        assert_eq!(kinds(r##""item # 1""##), vec![Str]);
        // Unterminated strings become Error tokens.
        assert_eq!(kinds(r#""abc"#), vec![Error]);
    }

    #[test]
    fn deep_nesting_is_an_error_not_a_crash() {
        let mut src = String::new();
        for _ in 0..200 {
            src.push_str("\"#(");
        }
        let out = lex(&src);
        assert!(out
            .errors
            .iter()
            .any(|e| e.kind == LexErrorKind::NestingTooDeep));
    }

    #[test]
    fn hash_outside_string() {
        assert_eq!(error_kinds("#x"), vec![LexErrorKind::HashOutsideString]);
        assert_eq!(kinds("#x"), vec![Error, Ident]);
    }

    #[test]
    fn comments() {
        let src = "/// line\n///* block *///  ///* a ///* nested */// b *///  //~ doc\n//~* doc\nblock *~//\n//// banner\n";
        assert_eq!(
            kinds(src),
            vec![
                LineComment,
                Newline,
                BlockComment,
                BlockComment,
                DocLineComment,
                Newline,
                DocBlockComment,
                Newline,
                LineComment,
                Newline,
            ]
        );
        assert!(lex(src).errors.is_empty());
        let t = texts(src);
        assert_eq!(t[0], "/// line");
        assert_eq!(t[3], "///* a ///* nested */// b *///");
        assert_eq!(t[6], "//~* doc\nblock *~//");
    }

    #[test]
    fn comment_at_end_of_file_and_crlf() {
        assert_eq!(kinds("x /// done"), vec![Ident, LineComment]);
        assert_eq!(texts("x /// done\r\ny"), vec!["x", "/// done", "\n", "y"]);
    }

    #[test]
    fn other_languages_comments_are_errors_with_a_hint() {
        assert_eq!(error_kinds("x // old\ny"), vec![LexErrorKind::NotAComment]);
        assert_eq!(kinds("x // old\ny"), vec![Ident, Error, Newline, Ident]);
        assert_eq!(error_kinds("/* old */"), vec![LexErrorKind::NotAComment]);
    }

    #[test]
    fn comment_errors() {
        assert_eq!(
            error_kinds("///* open"),
            vec![LexErrorKind::UnterminatedBlockComment]
        );
        assert_eq!(
            error_kinds("///* ///* inner *///"),
            vec![LexErrorKind::UnterminatedBlockComment]
        );
        assert_eq!(
            error_kinds("//~* open"),
            vec![LexErrorKind::UnterminatedBlockComment]
        );
        // A doc block ends only at `*~//`.
        assert_eq!(kinds("//~* a *// b *~//"), vec![DocBlockComment]);
    }

    #[test]
    fn newlines_collapse() {
        assert_eq!(
            kinds("\n\na\n\n\n  b\r\n\r\nc\n"),
            vec![Ident, Newline, Ident, Newline, Ident, Newline]
        );
        assert_eq!(
            kinds("a\n/// c\nb"),
            vec![Ident, Newline, LineComment, Newline, Ident]
        );
    }

    #[test]
    fn every_punctuation_lexes_alone() {
        for &p in Punct::ALL {
            let text = p.as_str();
            let out = lex(text);
            assert!(out.errors.is_empty(), "{text:?}");
            assert_eq!(out.tokens.len(), 2, "{text:?} lexed as {:?}", out.tokens);
            assert_eq!(out.tokens[0].kind, TokenKind::Punct(p), "{text:?}");
            assert_eq!(out.tokens[0].text(text), text);
        }
    }

    #[test]
    fn longest_match() {
        use Punct::*;
        assert_eq!(kinds(">>="), vec![punct(ShrEq)]);
        assert_eq!(kinds(">> ="), vec![punct(Shr), punct(Eq)]);
        assert_eq!(kinds("a ..= b"), vec![Ident, punct(DotDotEq), Ident]);
        assert_eq!(
            kinds("a ??= b"),
            vec![Ident, punct(QuestionQuestionEq), Ident]
        );
        assert_eq!(kinds("a?.b"), vec![Ident, punct(QuestionDot), Ident]);
        assert_eq!(
            kinds("c.close!!()"),
            vec![
                Ident,
                punct(Dot),
                Ident,
                punct(BangBang),
                punct(LParen),
                punct(RParen)
            ]
        );
        assert_eq!(
            kinds("c.bump!()"),
            vec![
                Ident,
                punct(Dot),
                Ident,
                punct(Bang),
                punct(LParen),
                punct(RParen)
            ]
        );
        assert_eq!(kinds("a!=b"), vec![Ident, punct(NotEq), Ident]);
        assert_eq!(
            kinds("x ~> x * 2"),
            vec![Ident, punct(TildeArrow), Ident, punct(Star), Int]
        );
        assert_eq!(kinds("x |> f"), vec![Ident, punct(PipeGt), Ident]);
        assert_eq!(
            kinds("fn f() -> int"),
            vec![
                kw(Keyword::Fn),
                Ident,
                punct(LParen),
                punct(RParen),
                punct(Arrow),
                Ident
            ]
        );
        assert_eq!(kinds("...xs"), vec![punct(DotDotDot), Ident]);
        // Generics closing: the lexer gives `>>`, the parser splits it.
        assert_eq!(
            kinds("map<string, [int]>>"),
            vec![
                Ident,
                punct(Lt),
                Ident,
                punct(Comma),
                punct(LBracket),
                Ident,
                punct(RBracket),
                punct(Shr)
            ]
        );
    }

    #[test]
    fn wrapping_operators() {
        use Punct::*;
        assert_eq!(kinds("a +% b"), vec![Ident, punct(PlusPercent), Ident]);
        assert_eq!(kinds("a-%b"), vec![Ident, punct(MinusPercent), Ident]);
        assert_eq!(kinds("h *%= 31"), vec![Ident, punct(StarPercentEq), Int]);
        // The plain remainder operator is unaffected.
        assert_eq!(kinds("a % b"), vec![Ident, punct(Percent), Ident]);
        assert_eq!(kinds("a %= b"), vec![Ident, punct(PercentEq), Ident]);
    }

    #[test]
    fn ternary_and_map_shapes() {
        use Punct::*;
        assert_eq!(
            kinds("a ? b : c"),
            vec![Ident, punct(Question), Ident, punct(Colon), Ident]
        );
        assert_eq!(
            kinds("parse(s)?"),
            vec![Ident, punct(LParen), Ident, punct(RParen), punct(Question)]
        );
        assert_eq!(
            kinds("{:}"),
            vec![punct(LBrace), punct(Colon), punct(RBrace)]
        );
    }

    #[test]
    fn unexpected_characters() {
        for src in ["$", "`", "\\", "\u{0}", "\u{a0}"] {
            assert_eq!(
                error_kinds(src),
                vec![LexErrorKind::UnexpectedChar],
                "{src:?}"
            );
        }
    }

    #[test]
    fn byte_order_mark_is_skipped() {
        assert_eq!(kinds("\u{feff}fn"), vec![kw(Keyword::Fn)]);
    }

    #[test]
    fn spans_slice_back_to_the_text() {
        let src = "const x = 3.5 // five\n";
        let out = lex(src);
        let got: Vec<&str> = out
            .tokens
            .iter()
            .filter(|t| t.kind != Eof)
            .map(|t| t.text(src))
            .collect();
        assert_eq!(got, vec!["const", "x", "=", "3.5", "// five", "\n"]);
        assert_eq!(
            out.tokens.last().unwrap().span,
            Span::new(src.len() as u32, src.len() as u32)
        );
    }

    // ---- Robustness ----------------------------------------------------------------

    /// A tiny deterministic random generator, so the test needs no dependencies.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    fn check_invariants(src: &str) {
        let out = tokenize(src);
        let last = out.tokens.last().expect("at least Eof");
        assert_eq!(last.kind, Eof);
        assert_eq!(
            last.span,
            Span::new(src.len() as u32, src.len() as u32),
            "{src:?}"
        );
        let mut cursor = 0usize;
        for t in &out.tokens {
            let (s, e) = (t.span.start as usize, t.span.end as usize);
            assert!(
                s >= cursor && s <= e && e <= src.len(),
                "bad span {t:?} in {src:?}"
            );
            assert!(
                src.get(s..e).is_some(),
                "span not on char boundaries {t:?} in {src:?}"
            );
            // Whatever lies between tokens is whitespace.
            let gap = &src[cursor..s];
            assert!(
                gap.chars()
                    .all(|c| matches!(c, ' ' | '\t' | '\r' | '\n' | '\u{c}' | '\u{feff}')),
                "gap {gap:?} between tokens in {src:?}"
            );
            cursor = e;
        }
        for e in &out.errors {
            assert!(e.span.start <= e.span.end && (e.span.end as usize) <= src.len());
            assert!(src
                .get(e.span.start as usize..e.span.end as usize)
                .is_some());
        }
        assert_eq!(tokenize(src), out, "lexing must be deterministic");
    }

    #[test]
    fn random_input_never_breaks_the_lexer() {
        const PIECES: &[&str] = &[
            "a", "Z", "_", "9", "0x", "1.", ".", "..", "...", "e", "-", "+", "/", "*", "//", "///",
            "//~", "//~*", "*~//", "///*", "*///", "/*", "*/", "\"", "'", "\\", "#", "#(", "(",
            ")", "{", "}", "[", "]", "\n", "\r\n", " ", "\t", "é", "日", "🙂", "$", "`", "?", "?.",
            "??=", "!", "!!", "~>", "|>", ">>=", "<<=", "=", ":", ";", ",", "@", "u{", "\u{feff}",
            "\u{0}",
        ];
        let mut rng = Rng(0x9e3779b97f4a7c15);
        for _ in 0..4000 {
            let len = (rng.next() % 24) as usize;
            let mut src = String::new();
            for _ in 0..len {
                src.push_str(PIECES[(rng.next() % PIECES.len() as u64) as usize]);
            }
            check_invariants(&src);
        }
    }
}
