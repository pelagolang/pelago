//! The parser: tokens to syntax tree.
//!
//! Recursive descent for declarations, statements, types and patterns, and precedence climbing
//! for expressions. The grammar rules that are not obvious from the design docs:
//!
//! * **Newlines end statements**, except inside `( )`, `[ ]` and struct/map literals, after a
//!   binary operator or `,`, and before a line that starts with `.`, `?.`, `|>`, `&&`, `||`,
//!   `??`, or a ternary `?`.
//! * **Spacing decides `?`, `<` and `!`.** A `?` stuck to the token before it is the error-propagation
//!   operator (`f()?`), otherwise it is the ternary (`a ? b : c`). A `<` stuck to a name opens generic
//!   arguments (`map<string, int>`), otherwise it is less-than. A `!` or `!!` stuck to a method name
//!   and followed by `(` marks the call (`c.bump!()`).
//! * **Struct literals are not allowed in conditions** (`if`, `while`, `unless`, `guard`, `for`,
//!   `switch`), so `if x { ... }` is a block. Wrap the literal in parentheses there.
//! * **`>>`, `>=` and `>>=` are split** when they close generic arguments.
//! * **Strings are split** into text and `#name` / `#(expr)` parts here, not in the lexer.
//! * Soft keywords are checked by text where they matter (`to`, `step`, `from`, `where`, `default`, ...).
//!
//! The parser never panics. Errors are collected, the tree gets `Error` nodes, and parsing resumes
//! at the next statement or item.

mod expr;
mod item;
mod pattern;
mod stmt;
mod string;
mod ty;

use crate::ast::File;
use crate::keyword::{foreign_keyword_hint, Keyword};
use crate::lexer::{tokenize, LexError};
use crate::span::Span;
use crate::token::{Punct, Token, TokenKind};

/// How deeply constructs may nest. Stops runaway input from overflowing the stack.
const MAX_DEPTH: u32 = 96;

/// A syntax error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub span: Span,
    pub message: String,
}

/// The result of parsing: always a tree, plus any errors.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseOutput {
    pub file: File,
    pub errors: Vec<ParseError>,
}

/// Parses tokens produced by the lexer for `src`.
pub fn parse(src: &str, tokens: &[Token]) -> ParseOutput {
    let mut p = Parser::new(src, tokens);
    let file = p.parse_file();
    ParseOutput {
        file,
        errors: p.errors,
    }
}

/// Lexes and parses `src`.
pub fn parse_source(src: &str) -> (ParseOutput, Vec<LexError>) {
    let lexed = tokenize(src);
    (parse(src, &lexed.tokens), lexed.errors)
}

/// Lexes, parses and runs the syntax checks of [`crate::check`]. This is what a compile uses.
/// Errors from all stages are sorted by position.
pub fn parse_and_check(src: &str) -> (ParseOutput, Vec<LexError>) {
    let (mut out, lex) = parse_source(src);
    let checked = crate::check::check_file(&out.file, src);
    out.errors.extend(checked);
    out.errors.sort_by_key(|e| e.span.start);
    (out, lex)
}

/// Lexes and parses `text`, which sits at byte offset `offset` inside the real source `src`
/// (used for `#(...)` in strings). Spans in the result point into `src`.
fn parse_fragment(src: &str, offset: u32, text: &str) -> (crate::ast::Expr, Vec<ParseError>) {
    let lexed = tokenize(text);
    let tokens: Vec<Token> = lexed
        .tokens
        .iter()
        .map(|t| {
            Token::new(
                t.kind,
                Span::new(t.span.start + offset, t.span.end + offset),
            )
        })
        .collect();
    let mut p = Parser::new(src, &tokens);
    p.nl_ignore = 1;
    p.settle();
    let expr = p.parse_expr();
    if !p.at_eof() {
        let span = p.tok().span;
        p.error(span, "unexpected text after the expression in `#( )`");
    }
    for e in &lexed.errors {
        p.errors.push(ParseError {
            span: Span::new(e.span.start + offset, e.span.end + offset),
            message: e.kind.message().to_string(),
        });
    }
    (expr, p.errors)
}

/// Settings saved while inside brackets or a block.
struct Frame {
    nl_ignore: u32,
    no_struct: bool,
    no_arrow: bool,
}

struct Snapshot {
    pos: usize,
    gt: u8,
    prev_end: u32,
    nl_ignore: u32,
    no_struct: bool,
    no_arrow: bool,
    frames: usize,
    errors: usize,
}

pub(crate) struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    /// For each opening bracket, the index of its closing partner (or `u32::MAX`).
    close_of: Vec<u32>,
    /// For each opening bracket, whether an `of` keyword sits directly inside it.
    has_of: Vec<bool>,
    pos: usize,
    /// The current token, kept in step with `pos` and `gt` by [`Parser::refresh`].
    cur: Token,
    /// How many leading `>` characters of the current token were already used to close generics.
    gt: u8,
    prev_end: u32,
    /// Newlines are skipped while this is above zero.
    nl_ignore: u32,
    /// Struct literals are off (conditions).
    no_struct: bool,
    /// `name ~>` is not a closure (the source of a builder clause).
    no_arrow: bool,
    frames: Vec<Frame>,
    /// Doc comment tokens, in source order.
    docs: Vec<Token>,
    /// Names of the labeled loops around the current position.
    labels: Vec<crate::ast::Ident>,
    depth: u32,
    errors: Vec<ParseError>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, tokens: &[Token]) -> Parser<'a> {
        let mut toks: Vec<Token> = Vec::with_capacity(tokens.len());
        let mut docs: Vec<Token> = Vec::new();
        for t in tokens {
            if t.kind.is_trivia() {
                if matches!(
                    t.kind,
                    TokenKind::DocLineComment | TokenKind::DocBlockComment
                ) {
                    docs.push(*t);
                }
                continue;
            }
            if t.kind == TokenKind::Newline
                && toks.last().map(|l| l.kind) == Some(TokenKind::Newline)
            {
                continue;
            }
            toks.push(*t);
        }
        if toks.last().map(|t| t.kind) != Some(TokenKind::Eof) {
            let end = toks.last().map_or(0, |t| t.span.end);
            toks.push(Token::new(TokenKind::Eof, Span::new(end, end)));
        }
        let (close_of, has_of) = match_brackets(&toks);
        let mut p = Parser {
            src,
            toks,
            close_of,
            has_of,
            pos: 0,
            cur: Token::new(TokenKind::Eof, Span::new(0, 0)),
            gt: 0,
            prev_end: 0,
            nl_ignore: 0,
            no_struct: false,
            no_arrow: false,
            frames: Vec::new(),
            docs,
            labels: Vec::new(),
            depth: 0,
            errors: Vec::new(),
        };
        p.settle();
        p
    }

    // ---- Token access --------------------------------------------------------

    fn last_index(&self) -> usize {
        self.toks.len() - 1
    }

    /// The current token. After a split `>>` this is the rest of it.
    #[inline]
    fn tok(&self) -> Token {
        self.cur
    }

    /// Recomputes [`Parser::cur`] after `pos` or `gt` changed.
    fn refresh(&mut self) {
        let t = self.toks[self.pos];
        self.cur = if self.gt == 0 {
            t
        } else {
            let rest = &t.text(self.src)[self.gt as usize..];
            let p = match rest {
                ">" => Punct::Gt,
                ">=" => Punct::GtEq,
                _ => Punct::Eq,
            };
            Token::new(
                TokenKind::Punct(p),
                Span::new(t.span.start + u32::from(self.gt), t.span.end),
            )
        };
    }

    fn kind(&self) -> TokenKind {
        self.tok().kind
    }

    fn text(&self, t: Token) -> &'a str {
        t.text(self.src)
    }

    fn at(&self, p: Punct) -> bool {
        self.kind() == TokenKind::Punct(p)
    }

    fn at_kw(&self, k: Keyword) -> bool {
        self.kind() == TokenKind::Keyword(k)
    }

    fn at_eof(&self) -> bool {
        self.kind() == TokenKind::Eof
    }

    /// The current token is an identifier with exactly this text (for soft keywords).
    fn at_word(&self, word: &str) -> bool {
        let t = self.tok();
        t.kind == TokenKind::Ident && self.text(t) == word
    }

    /// Skips newlines when they are being ignored.
    fn settle(&mut self) {
        if self.nl_ignore > 0 {
            while self.toks[self.pos].kind == TokenKind::Newline {
                self.pos += 1;
            }
        }
        self.refresh();
    }

    fn bump(&mut self) -> Token {
        let t = self.tok();
        if t.kind == TokenKind::Eof {
            return t;
        }
        self.prev_end = t.span.end;
        self.gt = 0;
        self.pos += 1;
        self.settle();
        t
    }

    fn eat(&mut self, p: Punct) -> bool {
        if self.at(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, k: Keyword) -> bool {
        if self.at_kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Consumes one `>`, splitting `>>`, `>=` and `>>=` when needed.
    fn eat_gt(&mut self) -> bool {
        let t = self.tok();
        match t.kind {
            TokenKind::Punct(Punct::Gt) => {
                self.bump();
                true
            }
            TokenKind::Punct(Punct::Shr | Punct::GtEq | Punct::ShrEq) => {
                self.gt += 1;
                self.prev_end = t.span.start + 1;
                self.refresh();
                true
            }
            _ => false,
        }
    }

    /// The `n`-th token ahead (0 is the current one), skipping newlines. Ignores `>` splitting.
    fn peek_sig(&self, n: usize) -> Token {
        let mut i = self.pos;
        let mut left = n;
        loop {
            while self.toks[i].kind == TokenKind::Newline && i < self.last_index() {
                i += 1;
            }
            if left == 0 || i >= self.last_index() {
                return self.toks[i];
            }
            i += 1;
            left -= 1;
        }
    }

    /// The token right after the current one, without skipping newlines.
    fn peek_raw(&self) -> Token {
        self.toks[(self.pos + 1).min(self.last_index())]
    }

    /// True when the current token touches the one before it (no space or newline between).
    fn adjacent(&self) -> bool {
        self.tok().span.start == self.prev_end
    }

    fn skip_newlines(&mut self) {
        while self.toks[self.pos].kind == TokenKind::Newline {
            self.pos += 1;
        }
        self.refresh();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            pos: self.pos,
            gt: self.gt,
            prev_end: self.prev_end,
            nl_ignore: self.nl_ignore,
            no_struct: self.no_struct,
            no_arrow: self.no_arrow,
            frames: self.frames.len(),
            errors: self.errors.len(),
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.pos = s.pos;
        self.gt = s.gt;
        self.prev_end = s.prev_end;
        self.nl_ignore = s.nl_ignore;
        self.no_struct = s.no_struct;
        self.no_arrow = s.no_arrow;
        self.frames.truncate(s.frames);
        self.errors.truncate(s.errors);
        self.refresh();
    }

    /// Index of the closing bracket for the opening bracket at the current token, if any.
    fn closing_index(&self) -> Option<usize> {
        let c = *self.close_of.get(self.pos)?;
        (c != u32::MAX).then_some(c as usize)
    }

    /// The doc comments sitting right before the current token (and after the previous real
    /// token, on lines of their own).
    fn docs_here(&self) -> Vec<Span> {
        if self.docs.is_empty() {
            return Vec::new();
        }
        let here = self.toks[self.pos].span.start;
        let mut prev_end = 0u32;
        let mut has_prev = false;
        let mut j = self.pos;
        while j > 0 {
            j -= 1;
            if self.toks[j].kind != TokenKind::Newline {
                prev_end = self.toks[j].span.end;
                has_prev = true;
                break;
            }
        }
        let lo = self.docs.partition_point(|d| d.span.start < prev_end);
        let mut out: Vec<Span> = Vec::new();
        for d in &self.docs[lo..] {
            if d.span.end > here {
                break;
            }
            // A doc comment on the same line as the previous token is not for this item.
            if out.is_empty()
                && has_prev
                && !self
                    .src
                    .get(prev_end as usize..d.span.start as usize)
                    .is_some_and(|gap| gap.contains('\n'))
            {
                continue;
            }
            out.push(d.span);
        }
        out
    }

    // ---- Errors --------------------------------------------------------------

    fn error(&mut self, span: Span, message: impl Into<String>) {
        // One error per position is enough; the second is usually a knock-on effect.
        if self.errors.last().map(|e| e.span.start) == Some(span.start) {
            return;
        }
        self.errors.push(ParseError {
            span,
            message: message.into(),
        });
    }

    fn describe(&self, t: Token) -> String {
        match t.kind {
            TokenKind::Eof => "the end of the file".to_string(),
            TokenKind::Newline => "the end of the line".to_string(),
            _ => format!("`{}`", self.text(t)),
        }
    }

    fn error_expected(&mut self, what: &str) {
        let t = self.tok();
        let found = self.describe(t);
        let mut msg = format!("expected {what}, found {found}");
        if t.kind == TokenKind::Ident {
            if let Some(hint) = foreign_keyword_hint(self.text(t)) {
                msg = format!("{msg}: {hint}");
            }
        }
        let span = if t.kind == TokenKind::Newline || t.kind == TokenKind::Eof {
            Span::new(self.prev_end, self.prev_end.max(t.span.start))
        } else {
            t.span
        };
        self.error(span, msg);
    }

    /// Consumes `p` or reports an error and returns an empty span at the current position.
    fn expect(&mut self, p: Punct) -> Span {
        if self.at(p) {
            self.bump().span
        } else {
            self.error_expected(&format!("`{}`", p.as_str()));
            let s = self.tok().span.start;
            Span::new(s, s)
        }
    }

    fn expect_kw(&mut self, k: Keyword) -> Span {
        if self.at_kw(k) {
            self.bump().span
        } else {
            self.error_expected(&format!("`{}`", k.as_str()));
            let s = self.tok().span.start;
            Span::new(s, s)
        }
    }

    /// Skips tokens to the end of the current statement without consuming the terminator or a
    /// closing bracket that belongs to an outer construct.
    fn sync_stmt(&mut self) {
        let mut depth = 0u32;
        loop {
            let t = self.tok();
            match t.kind {
                TokenKind::Eof => return,
                TokenKind::Newline | TokenKind::Punct(Punct::Semi) if depth == 0 => return,
                TokenKind::Punct(Punct::LParen | Punct::LBracket | Punct::LBrace) => depth += 1,
                TokenKind::Punct(Punct::RParen | Punct::RBracket | Punct::RBrace) => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            self.bump();
        }
    }

    // ---- Recursion limit and delimiters --------------------------------------

    /// Returns false (after reporting) when nesting is too deep.
    fn enter(&mut self) -> bool {
        if self.depth >= MAX_DEPTH {
            let span = self.tok().span;
            self.error(span, "this is nested too deeply");
            return false;
        }
        self.depth += 1;
        true
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    /// Starts a bracketed or braced region. Inside it, struct literals and closures are allowed
    /// again, and newlines are skipped if `ignore_newlines` (brackets) or kept (blocks).
    fn push_frame(&mut self, ignore_newlines: bool) {
        self.frames.push(Frame {
            nl_ignore: self.nl_ignore,
            no_struct: self.no_struct,
            no_arrow: self.no_arrow,
        });
        self.no_struct = false;
        self.no_arrow = false;
        self.nl_ignore = if ignore_newlines {
            self.nl_ignore + 1
        } else {
            0
        };
        self.settle();
    }

    /// Ends the region; call before consuming the closing bracket.
    fn pop_frame(&mut self) {
        if let Some(f) = self.frames.pop() {
            self.nl_ignore = f.nl_ignore;
            self.no_struct = f.no_struct;
            self.no_arrow = f.no_arrow;
        }
    }

    /// After an opening `(`, `[` or `{` was consumed: newlines stop mattering.
    fn enter_brackets(&mut self) {
        self.push_frame(true);
    }

    fn leave_brackets(&mut self) {
        self.pop_frame();
    }

    /// Runs `f` with struct literals switched on or off.
    fn with_struct<T>(&mut self, allowed: bool, f: impl FnOnce(&mut Self) -> T) -> T {
        let saved = self.no_struct;
        self.no_struct = !allowed;
        let out = f(self);
        self.no_struct = saved;
        out
    }

    // ---- Names ---------------------------------------------------------------

    /// An identifier (soft keywords included).
    fn ident(&mut self, what: &str) -> crate::ast::Ident {
        if self.kind() == TokenKind::Ident {
            crate::ast::Ident {
                span: self.bump().span,
            }
        } else {
            self.error_expected(what);
            let s = self.tok().span.start;
            crate::ast::Ident {
                span: Span::new(s, s),
            }
        }
    }

    /// A dotted name: `a.b.c`.
    fn path(&mut self, what: &str) -> crate::ast::Path {
        let first = self.ident(what);
        let mut segments = vec![first];
        while self.at(Punct::Dot) && self.peek_sig(1).kind == TokenKind::Ident {
            self.bump();
            segments.push(self.ident(what));
        }
        let span = first.span.to(segments[segments.len() - 1].span);
        crate::ast::Path { segments, span }
    }
}

/// One pass over the tokens: pair up brackets and note which ones directly contain `of`.
fn match_brackets(toks: &[Token]) -> (Vec<u32>, Vec<bool>) {
    let mut close_of = vec![u32::MAX; toks.len()];
    let mut has_of = vec![false; toks.len()];
    let mut stack: Vec<(usize, Punct)> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            TokenKind::Punct(p @ (Punct::LParen | Punct::LBracket | Punct::LBrace)) => {
                stack.push((i, p));
            }
            TokenKind::Punct(c @ (Punct::RParen | Punct::RBracket | Punct::RBrace)) => {
                let want = match c {
                    Punct::RParen => Punct::LParen,
                    Punct::RBracket => Punct::LBracket,
                    _ => Punct::LBrace,
                };
                if let Some(&(open, kind)) = stack.last() {
                    if kind == want {
                        close_of[open] = i as u32;
                        stack.pop();
                    }
                }
            }
            TokenKind::Keyword(Keyword::Of) => {
                if let Some(&(open, _)) = stack.last() {
                    has_of[open] = true;
                }
            }
            _ => {}
        }
    }
    (close_of, has_of)
}

#[cfg(test)]
mod tests;
