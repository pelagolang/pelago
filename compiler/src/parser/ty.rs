//! Types: `int`, `[T]`, `[T; N]`, `(A, B)`, `T?`, `T ! E`, `map<K, V>`, `fn(int) -> int`.

use super::Parser;
use crate::ast::*;
use crate::keyword::Keyword;
use crate::span::Span;
use crate::token::{Punct, TokenKind};

impl Parser<'_> {
    pub(super) fn parse_type(&mut self) -> Type {
        let start = self.tok().span;
        if !self.enter() {
            return Type {
                kind: TypeKind::Error,
                span: start,
            };
        }
        let ty = self.parse_type_inner();
        self.leave();
        ty
    }

    fn parse_type_inner(&mut self) -> Type {
        let ok = self.parse_type_suffixed();
        // `fn(int!)`: a `!` right before `,` or `)` is a mode mark, not a result type.
        let mark_only = matches!(
            self.peek_sig(1).kind,
            TokenKind::Punct(Punct::Comma | Punct::RParen)
        );
        if self.at(Punct::Bang) && !mark_only {
            self.bump();
            let err = self.parse_type_suffixed();
            let span = ok.span.to(err.span);
            return Type {
                kind: TypeKind::Result(Box::new(ok), Box::new(err)),
                span,
            };
        }
        ok
    }

    /// An atom followed by any number of attached `?`.
    fn parse_type_suffixed(&mut self) -> Type {
        let mut ty = self.parse_type_atom();
        while self.at(Punct::Question) && self.adjacent() {
            let q = self.bump().span;
            let span = ty.span.to(q);
            ty = Type {
                kind: TypeKind::Option(Box::new(ty)),
                span,
            };
        }
        ty
    }

    /// `braid<A, B; N>`: the same type as `(A, B; N)`, spelled out.
    fn parse_braid_type(&mut self) -> Type {
        let start = self.bump().span;
        self.bump();
        self.enter_brackets();
        let mut items = Vec::new();
        while !self.is_gt() && !self.at(Punct::Semi) && !self.at_eof() {
            let before = self.pos;
            items.push(self.parse_type());
            if !self.eat(Punct::Comma) {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        let count = if self.eat(Punct::Semi) {
            // Arithmetic (`2 * 3`, `N + 1`) but not comparisons or shifts: `>` ends the type.
            self.with_struct(true, |p| p.parse_bin(12))
        } else {
            self.error_expected("`;` and a length: `braid<A, B; N>`");
            Expr {
                kind: ExprKind::Underscore,
                span: start,
            }
        };
        self.leave_brackets();
        let at = self.tok().span;
        if !self.eat_gt() {
            self.error_expected("`>`");
        }
        Type {
            kind: TypeKind::FixedTuple(items, Box::new(count), true),
            span: start.to(Span::new(at.start, self.prev_end.max(at.start))),
        }
    }

    fn parse_type_atom(&mut self) -> Type {
        let t = self.tok();
        match t.kind {
            TokenKind::Ident
                if self.text(t) == "once"
                    && self.peek_sig(1).kind == TokenKind::Keyword(Keyword::Fn) =>
            {
                self.parse_fn_type(true)
            }
            TokenKind::Ident => {
                if self.text(t) == "any" && self.is_type_start_token(self.peek_sig(1).kind) {
                    self.bump();
                    let inner = self.parse_type_suffixed();
                    let span = t.span.to(inner.span);
                    return Type {
                        kind: TypeKind::Any(Box::new(inner)),
                        span,
                    };
                }
                if self.text(t) == "braid"
                    && self.peek_sig(1).kind == TokenKind::Punct(Punct::Lt)
                    && self.peek_sig(1).span.start == t.span.end
                {
                    return self.parse_braid_type();
                }
                let path = self.path("a type name");
                let mut span = path.span;
                let mut args = Vec::new();
                if self.at(Punct::Lt) && self.adjacent() {
                    let (a, end) = self.parse_generic_args();
                    args = a;
                    span = span.to(end);
                }
                Type {
                    kind: TypeKind::Path { path, args },
                    span,
                }
            }
            TokenKind::Keyword(Keyword::SelfType) => {
                self.bump();
                Type {
                    kind: TypeKind::SelfType,
                    span: t.span,
                }
            }
            TokenKind::Keyword(Keyword::Fn) => self.parse_fn_type(false),
            TokenKind::Punct(Punct::LBracket) => {
                self.bump();
                self.enter_brackets();
                let elem = self.parse_type();
                let kind = if self.eat(Punct::Semi) {
                    let n = self.with_struct(true, |p| p.parse_expr());
                    TypeKind::Array(Box::new(elem), Box::new(n))
                } else {
                    TypeKind::List(Box::new(elem))
                };
                self.leave_brackets();
                let end = self.expect(Punct::RBracket);
                Type {
                    kind,
                    span: t.span.to(end),
                }
            }
            TokenKind::Punct(Punct::LParen) => {
                self.bump();
                self.enter_brackets();
                let mut items = Vec::new();
                let mut trailing_comma = false;
                let mut fixed = None;
                while !self.at(Punct::RParen) && !self.at(Punct::Semi) && !self.at_eof() {
                    let before = self.pos;
                    items.push(self.parse_type());
                    trailing_comma = self.eat(Punct::Comma);
                    if !trailing_comma {
                        break;
                    }
                    if self.pos == before {
                        self.bump();
                    }
                }
                if self.eat(Punct::Semi) {
                    fixed = Some(self.with_struct(true, |p| p.parse_expr()));
                }
                self.leave_brackets();
                let end = self.expect(Punct::RParen);
                let span = t.span.to(end);
                let kind = match fixed {
                    Some(n) => TypeKind::FixedTuple(items, Box::new(n), false),
                    None if items.len() == 1 && !trailing_comma => {
                        // `(T)` is just `T` in parentheses.
                        return items.pop().unwrap_or(Type {
                            kind: TypeKind::Error,
                            span,
                        });
                    }
                    None => TypeKind::Tuple(items),
                };
                Type { kind, span }
            }
            _ => {
                self.error_expected("a type");
                Type {
                    kind: TypeKind::Error,
                    span: Span::new(t.span.start, t.span.start),
                }
            }
        }
    }

    fn is_type_start_token(&self, k: TokenKind) -> bool {
        matches!(
            k,
            TokenKind::Ident
                | TokenKind::Keyword(Keyword::SelfType)
                | TokenKind::Punct(Punct::LBracket)
        )
    }

    fn parse_fn_type(&mut self, once: bool) -> Type {
        let start = self.tok().span;
        if once {
            self.bump(); // `once`
        }
        self.bump(); // `fn`
        let mut params = Vec::new();
        if self.eat(Punct::LParen) {
            self.enter_brackets();
            while !self.at(Punct::RParen) && !self.at_eof() {
                let before = self.pos;
                let before_mode = self.parse_mode();
                let ty = self.parse_type();
                let after_mode = self.parse_mode_mark();
                let mode = self.join_modes(before_mode, after_mode);
                params.push(FnTypeParam { mode, ty });
                if !self.eat(Punct::Comma) {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.leave_brackets();
            self.expect(Punct::RParen);
        } else {
            self.error_expected("`(`");
        }
        let mut end = self.prev_end;
        let ret = if self.eat(Punct::Arrow) {
            let r = self.parse_type();
            end = r.span.end;
            Some(Box::new(r))
        } else {
            None
        };
        Type {
            kind: TypeKind::Fn { once, params, ret },
            span: Span::new(start.start, end.max(start.end)),
        }
    }

    /// `<A, B>` after a name. Returns the arguments and the span of the closing `>`.
    pub(super) fn parse_generic_args(&mut self) -> (Vec<GenericArg>, Span) {
        self.bump(); // `<`
        self.enter_brackets();
        let mut args = Vec::new();
        while !self.at_eof() && !self.is_gt() {
            let before = self.pos;
            args.push(self.parse_generic_arg());
            if !self.eat(Punct::Comma) {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.leave_brackets();
        let start = self.tok().span.start;
        if !self.eat_gt() {
            self.error_expected("`>`");
        }
        (args, Span::new(start, self.prev_end.max(start)))
    }

    fn is_gt(&self) -> bool {
        matches!(
            self.kind(),
            TokenKind::Punct(Punct::Gt | Punct::Shr | Punct::GtEq | Punct::ShrEq)
        )
    }

    fn parse_generic_arg(&mut self) -> GenericArg {
        match self.kind() {
            TokenKind::Int
            | TokenKind::Float
            | TokenKind::Char
            | TokenKind::Str
            | TokenKind::Punct(Punct::Minus)
            | TokenKind::Keyword(Keyword::True | Keyword::False) => {
                GenericArg::Const(self.parse_unary())
            }
            _ => GenericArg::Type(self.parse_type()),
        }
    }

    /// Optional `let` / `inout` / `sink`.
    pub(super) fn parse_mode(&mut self) -> Option<Mode> {
        let mode = match self.kind() {
            TokenKind::Keyword(Keyword::Let) => Mode::Let,
            TokenKind::Keyword(Keyword::Inout) => Mode::Inout,
            TokenKind::Keyword(Keyword::Sink) => Mode::Sink,
            _ => return None,
        };
        self.bump();
        Some(mode)
    }

    /// The short spelling of a mode, written right after a name: `x!` is `inout x`, `x!!` is
    /// `sink x`. Only a `!` that touches the name counts, and one that starts a call mark
    /// (`c.bump!()`) is left alone.
    pub(super) fn mode_mark_ahead(&self) -> Option<Mode> {
        if !self.adjacent() || self.peek_raw().kind == TokenKind::Punct(Punct::LParen) {
            return None;
        }
        match self.kind() {
            TokenKind::Punct(Punct::Bang) => Some(Mode::Inout),
            TokenKind::Punct(Punct::BangBang) => Some(Mode::Sink),
            _ => None,
        }
    }

    /// Consumes a mode mark after a name, if there is one.
    pub(super) fn parse_mode_mark(&mut self) -> Option<Mode> {
        let mode = self.mode_mark_ahead()?;
        self.bump();
        Some(mode)
    }

    /// Joins a mode written before a name with one written after it. Writing both is an error.
    pub(super) fn join_modes(&mut self, before: Option<Mode>, after: Option<Mode>) -> Option<Mode> {
        if before.is_some() && after.is_some() {
            let span = Span::new(self.prev_end.saturating_sub(1), self.prev_end);
            self.error(
                span,
                "write the mode once: `inout x` or `x!`, `sink x` or `x!!`",
            );
        }
        before.or(after)
    }

    /// Speculatively parses `<...>` generic arguments in an expression. On failure nothing is
    /// consumed and no error is recorded.
    pub(super) fn try_generic_args(&mut self) -> Option<(Vec<GenericArg>, Span)> {
        let snap = self.snapshot();
        let (args, end) = self.parse_generic_args();
        let fine = self.errors.len() == snap.errors && self.prev_end >= end.end && end.end > 0;
        let follows = matches!(
            self.kind(),
            TokenKind::Punct(Punct::LParen | Punct::LBrace | Punct::Dot)
        );
        if fine && follows {
            Some((args, end))
        } else {
            self.restore(snap);
            None
        }
    }

    /// `<T: ordered, meta N: int>` after a function or type name.
    pub(super) fn parse_generics(&mut self) -> Generics {
        let mut params = Vec::new();
        if !(self.at(Punct::Lt) && self.adjacent()) {
            return Generics { params };
        }
        self.bump();
        self.enter_brackets();
        while !self.at_eof() && !self.is_gt() {
            let before = self.pos;
            let is_meta = self.eat_kw(Keyword::Meta);
            let name = self.ident("a type parameter name");
            let mut bounds = Vec::new();
            if self.eat(Punct::Colon) {
                bounds.push(self.parse_type());
                while self.eat(Punct::Plus) {
                    bounds.push(self.parse_type());
                }
            }
            params.push(GenericParam {
                is_meta,
                name,
                bounds,
            });
            if !self.eat(Punct::Comma) {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.leave_brackets();
        if !self.eat_gt() {
            self.error_expected("`>`");
        }
        Generics { params }
    }

    /// `where T: ordered, U: eq` after a signature. Empty when there is none.
    pub(super) fn parse_where(&mut self) -> Vec<WherePred> {
        let mut preds = Vec::new();
        if !self.at_word("where") {
            return preds;
        }
        self.bump();
        loop {
            let ty = self.parse_type();
            let mut bounds = Vec::new();
            if self.eat(Punct::Colon) {
                bounds.push(self.parse_type());
                while self.eat(Punct::Plus) {
                    bounds.push(self.parse_type());
                }
            } else {
                self.error_expected("`:`");
            }
            preds.push(WherePred { ty, bounds });
            if !self.eat(Punct::Comma) {
                break;
            }
            self.skip_newlines();
        }
        preds
    }
}
