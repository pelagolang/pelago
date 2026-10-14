//! Expressions: precedence climbing for operators, a postfix loop for calls and fields, and a
//! primary parser for everything that starts an expression.
//!
//! Binding strength, loosest first:
//!
//! | Level | Operators |
//! |---|---|
//! | assignment | `=` `+=` `??=` ... (right) |
//! | ternary | `a ? b : c` (right) |
//! | 1 | `\|>` |
//! | 2 | `??` |
//! | 3 | `\|\|` |
//! | 4 | `&&` |
//! | 5 | `== != < > <= >=`, `is`, `in` |
//! | 6 | `..` `..=` (with `step`) |
//! | 7 | `to` `to?` |
//! | 8 to 11 | `\|` `^` `&` `<< >>` |
//! | 12 | `+ - +% -%` |
//! | 13 | `* / % *%` |
//! | unary | `- ! ~ copy`, then `**` (right) |

use super::Parser;
use crate::ast::*;
use crate::keyword::Keyword;
use crate::span::Span;
use crate::token::{Punct, TokenKind};

const LEVEL_CMP: u8 = 5;
const LEVEL_RANGE: u8 = 6;
const LEVEL_CONV: u8 = 7;

/// What an infix token is.
enum Infix {
    Plain(Punct),
    Is,
    In,
    Range(bool),
    Convert,
}

impl Parser<'_> {
    fn error_expr(&self, span: Span) -> Expr {
        Expr {
            kind: ExprKind::Error,
            span: Span::new(span.start, span.start),
        }
    }

    /// A full expression, including assignment.
    pub(super) fn parse_expr(&mut self) -> Expr {
        if !self.enter() {
            let t = self.tok().span;
            return self.error_expr(t);
        }
        let e = self.parse_assign();
        self.leave();
        e
    }

    /// An expression with struct literals switched off (conditions and loop heads).
    pub(super) fn parse_cond(&mut self) -> Expr {
        self.with_struct(false, |p| p.parse_expr())
    }

    fn assign_op(&self) -> Option<Punct> {
        match self.kind() {
            TokenKind::Punct(
                p @ (Punct::Eq
                | Punct::PlusEq
                | Punct::MinusEq
                | Punct::StarEq
                | Punct::SlashEq
                | Punct::PercentEq
                | Punct::AmpEq
                | Punct::PipeEq
                | Punct::CaretEq
                | Punct::ShlEq
                | Punct::ShrEq
                | Punct::PlusPercentEq
                | Punct::MinusPercentEq
                | Punct::StarPercentEq
                | Punct::QuestionQuestionEq),
            ) => Some(p),
            _ => None,
        }
    }

    fn parse_assign(&mut self) -> Expr {
        let lhs = self.parse_ternary();
        if let Some(op) = self.assign_op() {
            self.bump();
            self.skip_newlines();
            let rhs = self.parse_assign();
            let span = lhs.span.to(rhs.span);
            return Expr {
                kind: ExprKind::Assign {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        lhs
    }

    /// If the line continues on the next line (`.`, `|>`, `&&`, ...), steps over the newline.
    fn skip_continuation(&mut self) {
        if self.kind() == TokenKind::Newline {
            if let TokenKind::Punct(
                Punct::Dot
                | Punct::QuestionDot
                | Punct::PipeGt
                | Punct::AndAnd
                | Punct::OrOr
                | Punct::QuestionQuestion
                | Punct::Question,
            ) = self.peek_sig(0).kind
            {
                self.skip_newlines();
            }
        }
    }

    fn parse_ternary(&mut self) -> Expr {
        let cond = self.parse_bin(1);
        self.skip_continuation();
        if self.at(Punct::Question) && !self.adjacent() {
            self.bump();
            self.skip_newlines();
            let then = self.parse_ternary();
            if self.kind() == TokenKind::Newline
                && self.peek_sig(0).kind == TokenKind::Punct(Punct::Colon)
            {
                self.skip_newlines();
            }
            self.expect(Punct::Colon);
            self.skip_newlines();
            let else_ = self.parse_ternary();
            let span = cond.span.to(else_.span);
            return Expr {
                kind: ExprKind::Ternary {
                    cond: Box::new(cond),
                    then: Box::new(then),
                    else_: Box::new(else_),
                },
                span,
            };
        }
        cond
    }

    fn infix(&self) -> Option<(u8, Infix)> {
        let t = self.tok();
        Some(match t.kind {
            TokenKind::Punct(p) => match p {
                Punct::PipeGt => (1, Infix::Plain(p)),
                Punct::QuestionQuestion => (2, Infix::Plain(p)),
                Punct::OrOr => (3, Infix::Plain(p)),
                Punct::AndAnd => (4, Infix::Plain(p)),
                Punct::EqEq | Punct::NotEq | Punct::Lt | Punct::Gt | Punct::LtEq | Punct::GtEq => {
                    (LEVEL_CMP, Infix::Plain(p))
                }
                Punct::DotDot => (LEVEL_RANGE, Infix::Range(false)),
                Punct::DotDotEq => (LEVEL_RANGE, Infix::Range(true)),
                Punct::Pipe => (8, Infix::Plain(p)),
                Punct::Caret => (9, Infix::Plain(p)),
                Punct::Amp => (10, Infix::Plain(p)),
                Punct::Shl | Punct::Shr => (11, Infix::Plain(p)),
                Punct::Plus | Punct::Minus | Punct::PlusPercent | Punct::MinusPercent => {
                    (12, Infix::Plain(p))
                }
                Punct::Star | Punct::Slash | Punct::Percent | Punct::StarPercent => {
                    (13, Infix::Plain(p))
                }
                _ => return None,
            },
            TokenKind::Keyword(Keyword::Is) => (LEVEL_CMP, Infix::Is),
            TokenKind::Keyword(Keyword::In) => (LEVEL_CMP, Infix::In),
            TokenKind::Ident if self.text(t) == "to" && self.starts_type_after_to() => {
                (LEVEL_CONV, Infix::Convert)
            }
            _ => return None,
        })
    }

    /// `to` is a conversion only when a type follows it (`x to float`, `pet to? dog`).
    fn starts_type_after_to(&self) -> bool {
        let next = self.peek_raw();
        match next.kind {
            TokenKind::Ident | TokenKind::Keyword(Keyword::SelfType | Keyword::Fn) => true,
            TokenKind::Punct(Punct::LBracket | Punct::LParen) => true,
            // `to?`: the question mark must touch `to`.
            TokenKind::Punct(Punct::Question) => next.span.start == self.tok().span.end,
            _ => false,
        }
    }

    /// Whether the current token can begin the end of a range (`0..n`) or a similar operand.
    fn can_start_operand(&self) -> bool {
        matches!(
            self.kind(),
            TokenKind::Int
                | TokenKind::Float
                | TokenKind::Char
                | TokenKind::Str
                | TokenKind::Ident
                | TokenKind::Underscore
                | TokenKind::Keyword(
                    Keyword::True
                        | Keyword::False
                        | Keyword::None
                        | Keyword::SelfValue
                        | Keyword::SelfType
                        | Keyword::Copy
                )
                | TokenKind::Punct(
                    Punct::LParen
                        | Punct::LBracket
                        | Punct::Dot
                        | Punct::Minus
                        | Punct::Bang
                        | Punct::BangBang
                        | Punct::Tilde
                )
        )
    }

    /// Operators from `min` level upward.
    pub(super) fn parse_bin(&mut self, min: u8) -> Expr {
        let mut lhs = self.parse_unary();
        loop {
            self.skip_continuation();
            let Some((level, op)) = self.infix() else {
                break;
            };
            if level < min {
                break;
            }
            match op {
                Infix::Plain(p) => {
                    self.bump();
                    self.skip_newlines();
                    let rhs = self.parse_bin(level + 1);
                    let span = lhs.span.to(rhs.span);
                    lhs = Expr {
                        kind: ExprKind::Binary {
                            op: p,
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        },
                        span,
                    };
                }
                Infix::Is => {
                    self.bump();
                    let pattern = self.parse_pattern_after_is();
                    let span = lhs.span.to(pattern.span);
                    lhs = Expr {
                        kind: ExprKind::Is {
                            value: Box::new(lhs),
                            pattern: Box::new(pattern),
                        },
                        span,
                    };
                }
                Infix::In => {
                    self.bump();
                    self.skip_newlines();
                    let rhs = self.parse_bin(LEVEL_RANGE);
                    let span = lhs.span.to(rhs.span);
                    lhs = Expr {
                        kind: ExprKind::In {
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        },
                        span,
                    };
                }
                Infix::Range(inclusive) => {
                    self.bump();
                    let end = self.parse_range_end();
                    let step = self.parse_range_step();
                    let mut span = lhs.span;
                    if let Some(e) = &end {
                        span = span.to(e.span);
                    } else {
                        span = Span::new(span.start, self.prev_end.max(span.end));
                    }
                    if let Some(s) = &step {
                        span = span.to(s.span);
                    }
                    lhs = Expr {
                        kind: ExprKind::Range {
                            start: Some(Box::new(lhs)),
                            end,
                            inclusive,
                            step,
                        },
                        span,
                    };
                }
                Infix::Convert => {
                    self.bump(); // `to`
                    let checked = self.at(Punct::Question) && self.adjacent();
                    if checked {
                        self.bump();
                    }
                    let ty = self.parse_type();
                    let span = lhs.span.to(ty.span);
                    lhs = Expr {
                        kind: ExprKind::Convert {
                            value: Box::new(lhs),
                            ty: Box::new(ty),
                            checked,
                        },
                        span,
                    };
                }
            }
        }
        lhs
    }

    fn parse_range_end(&mut self) -> Option<Box<Expr>> {
        if self.can_start_operand() {
            Some(Box::new(self.parse_bin(LEVEL_CONV)))
        } else {
            None
        }
    }

    /// `step n`, directly after a range on the same line.
    fn parse_range_step(&mut self) -> Option<Box<Expr>> {
        if self.at_word("step") {
            self.bump();
            Some(Box::new(self.parse_bin(LEVEL_CONV)))
        } else {
            None
        }
    }

    pub(super) fn parse_unary(&mut self) -> Expr {
        let t = self.tok();
        match t.kind {
            TokenKind::Punct(Punct::Minus | Punct::Bang | Punct::Tilde) => {
                let TokenKind::Punct(op) = t.kind else {
                    unreachable!()
                };
                self.bump();
                if !self.enter() {
                    return self.error_expr(t.span);
                }
                let operand = self.parse_unary();
                self.leave();
                let span = t.span.to(operand.span);
                Expr {
                    kind: ExprKind::Unary {
                        op,
                        operand: Box::new(operand),
                    },
                    span,
                }
            }
            TokenKind::Punct(Punct::BangBang) => {
                // Two negations in a row.
                self.bump();
                if !self.enter() {
                    return self.error_expr(t.span);
                }
                let operand = self.parse_unary();
                self.leave();
                let span = t.span.to(operand.span);
                let inner = Expr {
                    kind: ExprKind::Unary {
                        op: Punct::Bang,
                        operand: Box::new(operand),
                    },
                    span,
                };
                Expr {
                    kind: ExprKind::Unary {
                        op: Punct::Bang,
                        operand: Box::new(inner),
                    },
                    span,
                }
            }
            TokenKind::Keyword(Keyword::Copy) => {
                self.bump();
                if !self.enter() {
                    return self.error_expr(t.span);
                }
                let operand = self.parse_unary();
                self.leave();
                let span = t.span.to(operand.span);
                Expr {
                    kind: ExprKind::Copy(Box::new(operand)),
                    span,
                }
            }
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> Expr {
        let base = self.parse_postfix();
        if self.at(Punct::StarStar) {
            self.bump();
            self.skip_newlines();
            let rhs = self.parse_unary();
            let span = base.span.to(rhs.span);
            return Expr {
                kind: ExprKind::Binary {
                    op: Punct::StarStar,
                    lhs: Box::new(base),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        base
    }

    // ---- Postfix: fields, calls, indexing, `?` ---------------------------------

    fn parse_postfix(&mut self) -> Expr {
        let mut e = self.parse_primary();
        loop {
            self.skip_continuation();
            let t = self.tok();
            match t.kind {
                TokenKind::Punct(Punct::Dot) => {
                    self.bump();
                    e = self.parse_field_access(e, false);
                }
                TokenKind::Punct(Punct::QuestionDot) => {
                    self.bump();
                    e = self.parse_field_access(e, true);
                }
                TokenKind::Punct(Punct::LParen) => {
                    e = self.parse_call(e, CallMark::None);
                }
                TokenKind::Punct(Punct::Bang | Punct::BangBang)
                    if self.adjacent()
                        && self.peek_raw().kind == TokenKind::Punct(Punct::LParen) =>
                {
                    let mark = if t.kind == TokenKind::Punct(Punct::Bang) {
                        CallMark::Changes
                    } else {
                        CallMark::Consumes
                    };
                    self.bump();
                    e = self.parse_call(e, mark);
                }
                TokenKind::Punct(Punct::LBracket) if self.adjacent() => {
                    self.bump();
                    self.enter_brackets();
                    let index = self.parse_expr();
                    self.leave_brackets();
                    let close = self.expect(Punct::RBracket);
                    let span = e.span.to(close);
                    e = Expr {
                        kind: ExprKind::Index {
                            base: Box::new(e),
                            index: Box::new(index),
                        },
                        span,
                    };
                }
                TokenKind::Punct(Punct::Question) if self.adjacent() => {
                    self.bump();
                    let span = e.span.to(t.span);
                    e = Expr {
                        kind: ExprKind::Try(Box::new(e)),
                        span,
                    };
                }
                TokenKind::Punct(Punct::Lt) if self.adjacent() && is_path_like(&e) => {
                    if let Some((args, end)) = self.try_generic_args() {
                        let span = e.span.to(end);
                        e = Expr {
                            kind: ExprKind::TypeApply {
                                base: Box::new(e),
                                args,
                            },
                            span,
                        };
                    } else {
                        break;
                    }
                }
                TokenKind::Punct(Punct::LBrace)
                    if !self.no_struct && is_path_like(&e) && self.looks_like_struct_lit() =>
                {
                    e = self.parse_struct_lit(e);
                }
                _ => break,
            }
        }
        e
    }

    /// After `.` or `?.`: a field name or a tuple index.
    fn parse_field_access(&mut self, base: Expr, optional: bool) -> Expr {
        let t = self.tok();
        match t.kind {
            TokenKind::Ident => {
                let name = self.ident("a field name");
                let span = base.span.to(name.span);
                Expr {
                    kind: ExprKind::Field {
                        base: Box::new(base),
                        name,
                        optional,
                    },
                    span,
                }
            }
            TokenKind::Int => {
                self.bump();
                let span = base.span.to(t.span);
                Expr {
                    kind: ExprKind::TupleIndex {
                        base: Box::new(base),
                        index: t.span,
                    },
                    span,
                }
            }
            _ => {
                self.error_expected("a field name after `.`");
                base
            }
        }
    }

    fn parse_call(&mut self, callee: Expr, mark: CallMark) -> Expr {
        let open_idx = self.pos;
        self.bump(); // `(`
        self.enter_brackets();
        let mut args = Vec::new();
        if self.has_of.get(open_idx).copied().unwrap_or(false) {
            let b = self.parse_builder(BuilderKind::Stream);
            args.push(Arg {
                mode: None,
                label: None,
                spread: false,
                value: b,
            });
        } else {
            while !self.at(Punct::RParen) && !self.at_eof() {
                let before = self.pos;
                args.push(self.parse_arg());
                if !self.eat(Punct::Comma) {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
        }
        self.leave_brackets();
        let close = self.expect(Punct::RParen);
        let span = callee.span.to(close);
        Expr {
            kind: ExprKind::Call {
                callee: Box::new(callee),
                args,
                mark,
            },
            span,
        }
    }

    fn parse_arg(&mut self) -> Arg {
        let mode = match self.kind() {
            TokenKind::Keyword(Keyword::Inout | Keyword::Sink) => self.parse_mode(),
            _ => None,
        };
        let label = if self.kind() == TokenKind::Ident
            && self.peek_sig(1).kind == TokenKind::Punct(Punct::Colon)
        {
            let l = self.ident("an argument name");
            self.bump(); // `:`
            Some(l)
        } else {
            None
        };
        let spread = self.eat(Punct::DotDotDot);
        let value = self.parse_expr();
        Arg {
            mode,
            label,
            spread,
            value,
        }
    }

    /// Whether `{` after a name starts a struct literal: `{ }`, `{ name: ..`, `{ name, ..`, `{ ...`.
    fn looks_like_struct_lit(&self) -> bool {
        let first = self.peek_sig(1);
        match first.kind {
            TokenKind::Punct(Punct::RBrace | Punct::DotDotDot) => true,
            TokenKind::Ident => matches!(
                self.peek_sig(2).kind,
                TokenKind::Punct(Punct::Colon | Punct::Comma | Punct::RBrace)
            ),
            _ => false,
        }
    }

    fn parse_struct_lit(&mut self, base: Expr) -> Expr {
        let Some(ty) = expr_to_type(&base) else {
            return base;
        };
        self.bump(); // `{`
        self.enter_brackets();
        let mut fields = Vec::new();
        while !self.at(Punct::RBrace) && !self.at_eof() {
            let before = self.pos;
            if self.eat(Punct::DotDotDot) {
                fields.push(StructInit::Spread(self.parse_expr()));
            } else {
                let name = self.ident("a field name");
                let value = if self.eat(Punct::Colon) {
                    Some(self.parse_expr())
                } else {
                    None
                };
                fields.push(StructInit::Field(FieldInit { name, value }));
            }
            let comma = self.eat(Punct::Comma);
            if !comma && !(self.kind() == TokenKind::Ident || self.at(Punct::DotDotDot)) {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.leave_brackets();
        let close = self.expect(Punct::RBrace);
        let span = base.span.to(close);
        Expr {
            kind: ExprKind::StructLit {
                ty: Box::new(ty),
                fields,
            },
            span,
        }
    }

    // ---- Primary expressions ---------------------------------------------------

    /// A literal: number, character, string, `true`, `false`.
    pub(super) fn parse_literal(&mut self) -> Expr {
        let t = self.tok();
        let kind = match t.kind {
            TokenKind::Int => ExprKind::Int,
            TokenKind::Float => ExprKind::Float,
            TokenKind::Char => ExprKind::Char,
            TokenKind::Keyword(Keyword::True) => ExprKind::Bool(true),
            TokenKind::Keyword(Keyword::False) => ExprKind::Bool(false),
            TokenKind::Str => {
                self.bump();
                let parts = self.string_parts(t.span);
                return Expr {
                    kind: ExprKind::Str(parts),
                    span: t.span,
                };
            }
            _ => {
                self.error_expected("a literal");
                return self.error_expr(t.span);
            }
        };
        self.bump();
        Expr { kind, span: t.span }
    }

    fn parse_primary(&mut self) -> Expr {
        let t = self.tok();
        match t.kind {
            TokenKind::Int
            | TokenKind::Float
            | TokenKind::Char
            | TokenKind::Str
            | TokenKind::Keyword(Keyword::True | Keyword::False) => self.parse_literal(),
            TokenKind::Keyword(Keyword::None) => {
                self.bump();
                Expr {
                    kind: ExprKind::None,
                    span: t.span,
                }
            }
            TokenKind::Keyword(Keyword::SelfValue) => {
                self.bump();
                Expr {
                    kind: ExprKind::SelfValue,
                    span: t.span,
                }
            }
            TokenKind::Keyword(Keyword::SelfType) => {
                self.bump();
                Expr {
                    kind: ExprKind::SelfType,
                    span: t.span,
                }
            }
            TokenKind::Underscore => {
                if self.peek_raw().kind == TokenKind::Punct(Punct::TildeArrow) && !self.no_arrow {
                    return self.parse_closure_from_params();
                }
                self.bump();
                Expr {
                    kind: ExprKind::Underscore,
                    span: t.span,
                }
            }
            TokenKind::Ident => self.parse_ident_primary(),
            TokenKind::Punct(Punct::LParen) => self.parse_paren(),
            TokenKind::Punct(Punct::LBracket) => self.parse_bracket(),
            TokenKind::Punct(Punct::LBrace) => {
                if self.peek_sig(1).kind == TokenKind::Punct(Punct::Colon)
                    && self.peek_sig(2).kind == TokenKind::Punct(Punct::RBrace)
                {
                    self.bump();
                    self.bump();
                    let close = self.bump();
                    return Expr {
                        kind: ExprKind::Map(Vec::new()),
                        span: t.span.to(close.span),
                    };
                }
                let block = self.parse_block();
                Expr {
                    span: block.span,
                    kind: ExprKind::Block(block),
                }
            }
            TokenKind::Punct(Punct::Dot) => {
                self.bump();
                let name = self.ident("a case name after `.`");
                Expr {
                    kind: ExprKind::DotName(name),
                    span: t.span.to(name.span),
                }
            }
            TokenKind::Punct(Punct::DotDot | Punct::DotDotEq) => {
                let inclusive = t.kind == TokenKind::Punct(Punct::DotDotEq);
                self.bump();
                let end = self.parse_range_end();
                let step = self.parse_range_step();
                let mut span = t.span;
                if let Some(e) = &end {
                    span = span.to(e.span);
                }
                Expr {
                    kind: ExprKind::Range {
                        start: None,
                        end,
                        inclusive,
                        step,
                    },
                    span,
                }
            }
            TokenKind::Punct(Punct::TildeArrow) => self.parse_closure_from_params(),
            TokenKind::Keyword(k) => self.parse_keyword_expr(k),
            _ => {
                self.error_expected("an expression");
                self.error_expr(t.span)
            }
        }
    }

    fn parse_ident_primary(&mut self) -> Expr {
        let t = self.tok();
        let text = self.text(t);
        let next = self.peek_raw();
        // `x ~> body`
        if next.kind == TokenKind::Punct(Punct::TildeArrow) && !self.no_arrow {
            return self.parse_closure_from_params();
        }
        // `map { ... }` and `set { ... }`
        if (text == "map" || text == "set")
            && next.kind == TokenKind::Punct(Punct::LBrace)
            && !self.no_struct
        {
            return self.parse_collection_literal(text == "map");
        }
        // `select { ... }`
        if text == "select" && next.kind == TokenKind::Punct(Punct::LBrace) {
            return self.parse_select();
        }
        // `scope s { ... }`
        if text == "scope"
            && next.kind == TokenKind::Ident
            && self.peek_sig(2).kind == TokenKind::Punct(Punct::LBrace)
        {
            self.bump();
            let name = self.ident("a name for the scope");
            let body = self.parse_block();
            return Expr {
                span: t.span.to(body.span),
                kind: ExprKind::Scope { name, body },
            };
        }
        self.bump();
        let id = Ident { span: t.span };
        Expr {
            kind: ExprKind::Name(id),
            span: t.span,
        }
    }

    fn parse_paren(&mut self) -> Expr {
        let open = self.tok();
        // `(a, b) ~> body`
        if let Some(c) = self.closing_index() {
            if self.tilde_arrow_follows(c) && !self.no_arrow {
                return self.parse_closure_from_params();
            }
        }
        self.bump();
        self.enter_brackets();
        let mut items = Vec::new();
        let mut trailing_comma = false;
        while !self.at(Punct::RParen) && !self.at_eof() {
            let before = self.pos;
            items.push(self.parse_expr());
            trailing_comma = self.eat(Punct::Comma);
            if !trailing_comma {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.leave_brackets();
        let close = self.expect(Punct::RParen);
        let span = open.span.to(close);
        if items.len() == 1 && !trailing_comma {
            if let Some(inner) = items.pop() {
                return Expr {
                    kind: ExprKind::Paren(Box::new(inner)),
                    span,
                };
            }
        }
        Expr {
            kind: ExprKind::Tuple(items),
            span,
        }
    }

    fn parse_bracket(&mut self) -> Expr {
        let open_idx = self.pos;
        let open = self.tok();
        // `[inout total] x ~> body`
        if let Some(c) = self.closing_index() {
            if self.capture_list_follows(c) && !self.no_arrow {
                return self.parse_closure_from_params();
            }
        }
        if self.has_of.get(open_idx).copied().unwrap_or(false) {
            self.bump();
            self.enter_brackets();
            let b = self.parse_builder(BuilderKind::List);
            self.leave_brackets();
            let close = self.expect(Punct::RBracket);
            return Expr {
                span: open.span.to(close),
                kind: b.kind,
            };
        }
        self.bump();
        self.enter_brackets();
        let mut items = Vec::new();
        let mut repeat = None;
        while !self.at(Punct::RBracket) && !self.at_eof() {
            let before = self.pos;
            let spread = self.eat(Punct::DotDotDot);
            let value = self.parse_expr();
            if items.is_empty() && !spread && self.at(Punct::Semi) {
                self.bump();
                let count = self.parse_expr();
                repeat = Some((value, count));
                break;
            }
            items.push(ListItem { spread, value });
            if !self.eat(Punct::Comma) {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.leave_brackets();
        let close = self.expect(Punct::RBracket);
        let span = open.span.to(close);
        if let Some((value, count)) = repeat {
            return Expr {
                kind: ExprKind::Repeat {
                    value: Box::new(value),
                    count: Box::new(count),
                },
                span,
            };
        }
        Expr {
            kind: ExprKind::List(items),
            span,
        }
    }

    /// Is the token after the `)` at index `close` a `~>`, or `-> Type ~>`?
    fn tilde_arrow_follows(&self, close: usize) -> bool {
        match self.toks.get(close + 1) {
            Some(t) if t.kind == TokenKind::Punct(Punct::TildeArrow) => true,
            Some(t) if t.kind == TokenKind::Punct(Punct::Arrow) => self.type_then_tilde(close + 2),
            _ => false,
        }
    }

    /// Scans a return type (no braces, no line breaks) looking for the `~>` after it.
    fn type_then_tilde(&self, mut i: usize) -> bool {
        let mut steps = 0;
        let mut angle = 0u32;
        while let Some(t) = self.toks.get(i) {
            match t.kind {
                TokenKind::Punct(Punct::TildeArrow) => return true,
                TokenKind::Punct(Punct::Lt) => angle += 1,
                TokenKind::Punct(Punct::Gt) => angle = angle.saturating_sub(1),
                TokenKind::Punct(Punct::Shr) => angle = angle.saturating_sub(2),
                TokenKind::Punct(Punct::Comma) if angle > 0 => {}
                TokenKind::Punct(Punct::LParen | Punct::LBracket) => {
                    let c = self.close_of.get(i).copied().unwrap_or(u32::MAX);
                    if c == u32::MAX {
                        return false;
                    }
                    i = c as usize + 1;
                    continue;
                }
                TokenKind::Newline
                | TokenKind::Eof
                | TokenKind::Punct(
                    Punct::LBrace
                    | Punct::RBrace
                    | Punct::RParen
                    | Punct::RBracket
                    | Punct::Comma
                    | Punct::Semi
                    | Punct::Eq,
                ) => return false,
                _ => {}
            }
            i += 1;
            steps += 1;
            if steps > 256 {
                return false;
            }
        }
        false
    }

    /// Is the `]` at token index `close` followed by closure parameters and `~>`?
    fn capture_list_follows(&self, close: usize) -> bool {
        let after = self.toks.get(close + 1).copied();
        let Some(after) = after else { return false };
        match after.kind {
            TokenKind::Punct(Punct::TildeArrow) => true,
            TokenKind::Ident | TokenKind::Underscore => self
                .toks
                .get(close + 2)
                .is_some_and(|t| t.kind == TokenKind::Punct(Punct::TildeArrow)),
            TokenKind::Punct(Punct::LParen) => {
                let c = self.close_of.get(close + 1).copied().unwrap_or(u32::MAX);
                c != u32::MAX && self.tilde_arrow_follows(c as usize)
            }
            _ => false,
        }
    }

    // ---- Closures --------------------------------------------------------------

    /// Parses `[captures] params ~> body` where captures and params are each optional.
    fn parse_closure_from_params(&mut self) -> Expr {
        let start = self.tok().span;
        let mut captures = Vec::new();
        if self.at(Punct::LBracket) {
            self.bump();
            self.enter_brackets();
            while !self.at(Punct::RBracket) && !self.at_eof() {
                let before = self.pos;
                let mode = self.parse_mode();
                let name = self.ident("a captured name");
                captures.push(Capture { mode, name });
                if !self.eat(Punct::Comma) {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.leave_brackets();
            self.expect(Punct::RBracket);
        }
        let mut params = Vec::new();
        match self.kind() {
            TokenKind::Punct(Punct::TildeArrow) => {}
            TokenKind::Punct(Punct::LParen) => {
                self.bump();
                self.enter_brackets();
                while !self.at(Punct::RParen) && !self.at_eof() {
                    let before = self.pos;
                    let pattern = self.parse_pattern();
                    let ty = self.eat(Punct::Colon).then(|| self.parse_type());
                    params.push(ClosureParam { pattern, ty });
                    if !self.eat(Punct::Comma) {
                        break;
                    }
                    if self.pos == before {
                        self.bump();
                    }
                }
                self.leave_brackets();
                self.expect(Punct::RParen);
            }
            _ => {
                let pattern = self.parse_pattern();
                params.push(ClosureParam { pattern, ty: None });
            }
        }
        let ret = if self.at(Punct::Arrow) {
            self.bump();
            Some(self.parse_type())
        } else {
            None
        };
        self.expect(Punct::TildeArrow);
        self.skip_newlines();
        let saved = self.no_arrow;
        self.no_arrow = false;
        let saved_labels = std::mem::take(&mut self.labels);
        let mut body = self.parse_expr();
        self.labels = saved_labels;
        self.no_arrow = saved;
        // `n ~> n++`: the increment belongs to the body.
        if self.at(Punct::PlusPlus) || self.at(Punct::MinusMinus) {
            let op = if self.at(Punct::PlusPlus) {
                Punct::PlusPlus
            } else {
                Punct::MinusMinus
            };
            let t = self.bump();
            let span = body.span.to(t.span);
            body = Expr {
                kind: ExprKind::IncDec {
                    op,
                    operand: Box::new(body),
                },
                span,
            };
        }
        let span = start.to(body.span);
        Expr {
            kind: ExprKind::Closure(Box::new(Closure {
                captures,
                params,
                ret,
                body,
            })),
            span,
        }
    }

    // ---- Collections and builders ------------------------------------------------

    /// `map { ... }` or `set { ... }`.
    fn parse_collection_literal(&mut self, is_map: bool) -> Expr {
        let name = self.bump();
        let open_idx = self.pos;
        self.bump(); // `{`
        self.enter_brackets();
        let result = if self.has_of.get(open_idx).copied().unwrap_or(false) {
            let kind = if is_map {
                BuilderKind::Map
            } else {
                BuilderKind::Set
            };
            self.parse_builder(kind).kind
        } else if is_map {
            let mut items = Vec::new();
            while !self.at(Punct::RBrace) && !self.at_eof() {
                let before = self.pos;
                if self.eat(Punct::DotDotDot) {
                    items.push(MapItem::Spread(self.parse_expr()));
                } else {
                    let key = self.parse_expr();
                    self.expect(Punct::Colon);
                    let value = self.parse_expr();
                    items.push(MapItem::Pair(key, value));
                }
                if !self.eat(Punct::Comma) {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
            ExprKind::Map(items)
        } else {
            let mut items = Vec::new();
            while !self.at(Punct::RBrace) && !self.at_eof() {
                let before = self.pos;
                let spread = self.eat(Punct::DotDotDot);
                let value = self.parse_expr();
                items.push(ListItem { spread, value });
                if !self.eat(Punct::Comma) {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
            ExprKind::Set(items)
        };
        self.leave_brackets();
        let close = self.expect(Punct::RBrace);
        Expr {
            kind: result,
            span: name.span.to(close),
        }
    }

    /// The inside of a builder: `x of xs if x > 0 ~> x * 2`. Returns a finished expression.
    fn parse_builder(&mut self, kind: BuilderKind) -> Expr {
        let start = self.tok().span;
        let mut clauses = Vec::new();
        let saved = self.no_arrow;
        self.no_arrow = true;
        loop {
            let before = self.pos;
            if self.at_kw(Keyword::If) {
                if clauses.is_empty() {
                    self.error(
                        self.tok().span,
                        "a builder starts with `x of xs`; `if` filters what comes before it",
                    );
                }
                self.bump();
                clauses.push(Clause::If(self.parse_expr()));
            } else {
                let mut pats = Vec::new();
                loop {
                    pats.push(self.parse_pattern());
                    if !self.eat(Punct::Comma) {
                        break;
                    }
                }
                self.expect_kw(Keyword::Of);
                let source = self.parse_expr();
                clauses.push(Clause::Of { pats, source });
            }
            if self.pos == before {
                self.bump();
            }
            if self.at_kw(Keyword::If) {
                continue;
            }
            if self.eat(Punct::Comma) {
                continue;
            }
            break;
        }
        self.no_arrow = saved;
        let mut result = None;
        let mut value = None;
        if self.eat(Punct::TildeArrow) {
            result = Some(self.parse_expr());
            if matches!(kind, BuilderKind::Map) && self.eat(Punct::Colon) {
                value = Some(self.parse_expr());
            }
        }
        let end = self.prev_end.max(start.end);
        Expr {
            kind: ExprKind::Builder(Box::new(Builder {
                kind,
                clauses,
                result,
                value,
            })),
            span: Span::new(start.start, end),
        }
    }

    // ---- Keyword expressions ---------------------------------------------------

    /// Whether an optional expression after `return` / `break` / `yield` is present.
    fn at_value_start(&self) -> bool {
        !matches!(
            self.kind(),
            TokenKind::Newline
                | TokenKind::Eof
                | TokenKind::Punct(
                    Punct::RBrace
                        | Punct::RParen
                        | Punct::RBracket
                        | Punct::Semi
                        | Punct::Comma
                        | Punct::Colon
                )
                | TokenKind::Keyword(Keyword::Case | Keyword::Else)
        ) && !self.at_default_arm()
    }

    fn parse_optional_value(&mut self) -> Option<Box<Expr>> {
        if self.at_value_start() {
            Some(Box::new(self.parse_expr()))
        } else {
            None
        }
    }

    /// After `break` / `continue`: a name that matches an enclosing loop label is that label.
    /// (To break with a value that has the same name, wrap it in parentheses.)
    fn take_label(&mut self) -> Option<Ident> {
        if self.kind() != TokenKind::Ident {
            return None;
        }
        let name = self.text(self.tok());
        if self.labels.iter().any(|l| l.text(self.src) == name) {
            return Some(self.ident("a label"));
        }
        None
    }

    fn parse_keyword_expr(&mut self, k: Keyword) -> Expr {
        let t = self.tok();
        match k {
            Keyword::If => {
                self.bump();
                let if_expr = self.parse_if_rest();
                Expr {
                    span: t.span.to(if_expr_span(&if_expr)),
                    kind: ExprKind::If(Box::new(if_expr)),
                }
            }
            Keyword::Unless => {
                self.bump();
                let if_expr = self.parse_if_rest();
                Expr {
                    span: t.span.to(if_expr_span(&if_expr)),
                    kind: ExprKind::Unless(Box::new(if_expr)),
                }
            }
            Keyword::Switch => self.parse_switch(),
            Keyword::Loop => {
                self.bump();
                let body = self.parse_block();
                Expr {
                    span: t.span.to(body.span),
                    kind: ExprKind::Loop(body),
                }
            }
            Keyword::While => {
                self.bump();
                let cond = self.parse_cond();
                let body = self.parse_block();
                Expr {
                    span: t.span.to(body.span),
                    kind: ExprKind::While {
                        cond: Box::new(cond),
                        body,
                    },
                }
            }
            Keyword::Do => {
                self.bump();
                let body = self.parse_block();
                self.expect_kw(Keyword::While);
                let cond = self.parse_cond();
                Expr {
                    span: t.span.to(cond.span),
                    kind: ExprKind::DoWhile {
                        body,
                        cond: Box::new(cond),
                    },
                }
            }
            Keyword::For => self.parse_for(),
            Keyword::Guard => {
                self.bump();
                let cond = self.parse_cond();
                self.expect_kw(Keyword::Else);
                let else_ = self.parse_block();
                Expr {
                    span: t.span.to(else_.span),
                    kind: ExprKind::Guard {
                        cond: Box::new(cond),
                        else_,
                    },
                }
            }
            Keyword::Break => {
                self.bump();
                let label = self.take_label();
                let value = self.parse_optional_value();
                let mut span = label.map_or(t.span, |l| t.span.to(l.span));
                if let Some(v) = &value {
                    span = span.to(v.span);
                }
                Expr {
                    kind: ExprKind::Break { label, value },
                    span,
                }
            }
            Keyword::Return | Keyword::Yield => {
                self.bump();
                let value = self.parse_optional_value();
                let span = value.as_ref().map_or(t.span, |v| t.span.to(v.span));
                let kind = if k == Keyword::Return {
                    ExprKind::Return(value)
                } else {
                    ExprKind::Yield(value)
                };
                Expr { kind, span }
            }
            Keyword::Continue => {
                self.bump();
                let label = self.take_label();
                Expr {
                    span: label.map_or(t.span, |l| t.span.to(l.span)),
                    kind: ExprKind::Continue { label },
                }
            }
            Keyword::Become => {
                self.bump();
                let call = self.parse_expr();
                Expr {
                    span: t.span.to(call.span),
                    kind: ExprKind::Become(Box::new(call)),
                }
            }
            Keyword::Defer => {
                self.bump();
                let inner = self.parse_expr();
                Expr {
                    span: t.span.to(inner.span),
                    kind: ExprKind::Defer(Box::new(inner)),
                }
            }
            Keyword::Meta => {
                self.bump();
                if self.at_kw(Keyword::If) {
                    self.bump();
                    let if_expr = self.parse_if_rest();
                    return Expr {
                        span: t.span.to(if_expr_span(&if_expr)),
                        kind: ExprKind::MetaIf(Box::new(if_expr)),
                    };
                }
                let inner = self.parse_unary();
                Expr {
                    span: t.span.to(inner.span),
                    kind: ExprKind::Meta(Box::new(inner)),
                }
            }
            Keyword::Unsafe => {
                self.bump();
                let body = self.parse_block();
                Expr {
                    span: t.span.to(body.span),
                    kind: ExprKind::Unsafe(body),
                }
            }
            Keyword::Async | Keyword::Await => {
                self.error(
                    t.span,
                    format!(
                        "`{}` is reserved for later; use threads and channels",
                        k.as_str()
                    ),
                );
                self.bump();
                self.error_expr(t.span)
            }
            _ => {
                self.error_expected("an expression");
                self.error_expr(t.span)
            }
        }
    }

    /// After `if` / `unless`: condition, block, optional `else`.
    fn parse_if_rest(&mut self) -> IfExpr {
        let cond = self.parse_cond();
        let then = self.parse_block();
        let mut else_ = None;
        if self.peek_sig(0).kind == TokenKind::Keyword(Keyword::Else) {
            self.skip_newlines();
            self.bump(); // `else`
            if self.at_kw(Keyword::If) {
                let t = self.bump();
                let inner = self.parse_if_rest();
                else_ = Some(Expr {
                    span: t.span.to(if_expr_span(&inner)),
                    kind: ExprKind::If(Box::new(inner)),
                });
            } else {
                let block = self.parse_block();
                else_ = Some(Expr {
                    span: block.span,
                    kind: ExprKind::Block(block),
                });
            }
        }
        IfExpr { cond, then, else_ }
    }
}

fn if_expr_span(e: &IfExpr) -> Span {
    match &e.else_ {
        Some(el) => e.then.span.to(el.span),
        None => e.then.span,
    }
}

/// Names, field chains and generic applications can be the type in a struct literal.
fn is_path_like(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Name(_) | ExprKind::SelfType => true,
        ExprKind::Field {
            base,
            optional: false,
            ..
        } => is_path_like(base),
        ExprKind::TypeApply { base, .. } => is_path_like(base),
        _ => false,
    }
}

/// Reads `a.b.c` or `a<T>` as a type, for struct literals.
fn expr_to_type(e: &Expr) -> Option<Type> {
    fn collect(e: &Expr, out: &mut Vec<Ident>) -> bool {
        match &e.kind {
            ExprKind::Name(id) => {
                out.push(*id);
                true
            }
            ExprKind::Field {
                base,
                name,
                optional: false,
            } => {
                let ok = collect(base, out);
                out.push(*name);
                ok
            }
            _ => false,
        }
    }
    match &e.kind {
        ExprKind::SelfType => Some(Type {
            kind: TypeKind::SelfType,
            span: e.span,
        }),
        ExprKind::TypeApply { base, args } => {
            let mut inner = expr_to_type(base)?;
            if let TypeKind::Path { args: a, .. } = &mut inner.kind {
                *a = args.clone();
            }
            inner.span = e.span;
            Some(inner)
        }
        _ => {
            let mut segments = Vec::new();
            if !collect(e, &mut segments) {
                return None;
            }
            Some(Type {
                kind: TypeKind::Path {
                    path: Path {
                        segments,
                        span: e.span,
                    },
                    args: Vec::new(),
                },
                span: e.span,
            })
        }
    }
}
