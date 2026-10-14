//! Patterns, used by `switch`, `is`, `guard`, `for`, declarations and closure parameters.

use super::Parser;
use crate::ast::*;
use crate::keyword::Keyword;
use crate::span::Span;
use crate::token::{Punct, TokenKind};

impl Parser<'_> {
    pub(super) fn parse_pattern(&mut self) -> Pattern {
        self.parse_pattern_in(false)
    }

    /// The pattern after `is`, where a lone name is a type test (`pet is dog`).
    pub(super) fn parse_pattern_after_is(&mut self) -> Pattern {
        self.parse_pattern_in(true)
    }

    fn parse_pattern_in(&mut self, after_is: bool) -> Pattern {
        let start = self.tok().span;
        if !self.enter() {
            return Pattern {
                kind: PatternKind::Error,
                span: start,
            };
        }
        let pat = self.parse_pattern_inner(after_is);
        self.leave();
        pat
    }

    fn error_pattern(&mut self, what: &str) -> Pattern {
        self.error_expected(what);
        let s = self.tok().span.start;
        Pattern {
            kind: PatternKind::Error,
            span: Span::new(s, s),
        }
    }

    fn at_literal_start(&self) -> bool {
        matches!(
            self.kind(),
            TokenKind::Int
                | TokenKind::Float
                | TokenKind::Char
                | TokenKind::Str
                | TokenKind::Keyword(Keyword::True | Keyword::False)
                | TokenKind::Punct(Punct::Minus)
        )
    }

    /// A literal in a pattern, with an optional leading minus.
    fn pattern_literal(&mut self) -> Expr {
        if self.at(Punct::Minus) {
            let m = self.bump().span;
            let operand = self.parse_literal();
            let span = m.to(operand.span);
            return Expr {
                kind: ExprKind::Unary {
                    op: Punct::Minus,
                    operand: Box::new(operand),
                },
                span,
            };
        }
        self.parse_literal()
    }

    fn parse_pattern_inner(&mut self, after_is: bool) -> Pattern {
        let t = self.tok();
        match t.kind {
            TokenKind::Underscore => {
                self.bump();
                Pattern {
                    kind: PatternKind::Wildcard,
                    span: t.span,
                }
            }
            TokenKind::Keyword(Keyword::None) => {
                self.bump();
                Pattern {
                    kind: PatternKind::None,
                    span: t.span,
                }
            }
            TokenKind::Keyword(Keyword::Let | Keyword::Inout | Keyword::Sink) => {
                let mode = self.parse_mode();
                let name = self.ident("a name to bind");
                Pattern {
                    kind: PatternKind::Binding { mode, name },
                    span: t.span.to(name.span),
                }
            }
            TokenKind::Int
            | TokenKind::Float
            | TokenKind::Char
            | TokenKind::Str
            | TokenKind::Keyword(Keyword::True | Keyword::False)
            | TokenKind::Punct(Punct::Minus) => {
                let start = self.pattern_literal();
                self.finish_range_pattern(Some(start), t.span)
            }
            TokenKind::Punct(Punct::DotDot | Punct::DotDotEq) => {
                self.finish_range_pattern(None, t.span)
            }
            TokenKind::Punct(Punct::Dot) => {
                self.bump();
                let name = self.ident("a case name after `.`");
                let path = Path {
                    segments: vec![name],
                    span: name.span,
                };
                let args = self.parse_variant_args();
                Pattern {
                    span: t
                        .span
                        .to(Span::new(t.span.start, self.prev_end.max(name.span.end))),
                    kind: PatternKind::Variant {
                        leading_dot: true,
                        path,
                        args,
                    },
                }
            }
            TokenKind::Punct(Punct::LParen) => self.parse_tuple_pattern(),
            TokenKind::Punct(Punct::LBracket) => self.parse_list_pattern(),
            TokenKind::Ident => self.parse_name_pattern(after_is),
            _ => self.error_pattern("a pattern"),
        }
    }

    /// After a literal (or at a leading `..`): turns it into a range pattern when `..` follows.
    fn finish_range_pattern(&mut self, start: Option<Expr>, first: Span) -> Pattern {
        let range_op = match self.kind() {
            TokenKind::Punct(Punct::DotDot) => Some(false),
            TokenKind::Punct(Punct::DotDotEq) => Some(true),
            _ => None,
        };
        let Some(inclusive) = range_op else {
            let lit = start.unwrap_or(Expr {
                kind: ExprKind::Error,
                span: first,
            });
            return Pattern {
                span: lit.span,
                kind: PatternKind::Literal(Box::new(lit)),
            };
        };
        let op = self.bump();
        let end = if self.at_literal_start() {
            Some(Box::new(self.pattern_literal()))
        } else {
            None
        };
        let mut span = first.to(op.span);
        if let Some(e) = &end {
            span = span.to(e.span);
        }
        Pattern {
            kind: PatternKind::Range {
                start: start.map(Box::new),
                end,
                inclusive,
            },
            span,
        }
    }

    fn parse_variant_args(&mut self) -> Option<Vec<PatArg>> {
        if !self.at(Punct::LParen) {
            return None;
        }
        self.bump();
        self.enter_brackets();
        let mut args = Vec::new();
        while !self.at(Punct::RParen) && !self.at_eof() {
            let before = self.pos;
            let label = if self.kind() == TokenKind::Ident
                && self.peek_sig(1).kind == TokenKind::Punct(Punct::Colon)
            {
                let l = self.ident("a label");
                self.bump(); // `:`
                Some(l)
            } else {
                None
            };
            let pattern = self.parse_pattern();
            args.push(PatArg { label, pattern });
            if !self.eat(Punct::Comma) {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.leave_brackets();
        self.expect(Punct::RParen);
        Some(args)
    }

    fn parse_tuple_pattern(&mut self) -> Pattern {
        let open = self.bump().span;
        self.enter_brackets();
        let mut items = Vec::new();
        let mut trailing_comma = false;
        while !self.at(Punct::RParen) && !self.at_eof() {
            let before = self.pos;
            items.push(self.parse_pattern());
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
        let span = open.to(close);
        if items.len() == 1 && !trailing_comma {
            // `(p)` is just `p`.
            if let Some(p) = items.pop() {
                return Pattern { kind: p.kind, span };
            }
        }
        Pattern {
            kind: PatternKind::Tuple(items),
            span,
        }
    }

    fn parse_list_pattern(&mut self) -> Pattern {
        let open = self.bump().span;
        self.enter_brackets();
        let mut items = Vec::new();
        let mut rests = 0;
        while !self.at(Punct::RBracket) && !self.at_eof() {
            let before = self.pos;
            if self.at(Punct::DotDotDot) {
                let dots = self.bump().span;
                let mode = self.parse_mode();
                let name = (self.kind() == TokenKind::Ident).then(|| self.ident("a name"));
                let end = name.map_or(dots, |n| n.span);
                rests += 1;
                if rests == 2 {
                    self.error(dots, "a list pattern can have only one `...`");
                }
                items.push(ListPatItem::Rest {
                    mode,
                    name,
                    span: dots.to(end),
                });
            } else {
                items.push(ListPatItem::Pattern(self.parse_pattern()));
            }
            if !self.eat(Punct::Comma) {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.leave_brackets();
        let close = self.expect(Punct::RBracket);
        Pattern {
            kind: PatternKind::List(items),
            span: open.to(close),
        }
    }

    fn parse_name_pattern(&mut self, after_is: bool) -> Pattern {
        let first = self.tok();
        // `name is pattern`
        if !after_is && self.peek_sig(1).kind == TokenKind::Keyword(Keyword::Is) {
            let name = self.ident("a name");
            self.bump(); // `is`
            let inner = self.parse_pattern_after_is();
            let span = name.span.to(inner.span);
            return Pattern {
                kind: PatternKind::As {
                    name,
                    pattern: Box::new(inner),
                },
                span,
            };
        }
        let path = self.path("a name");
        if self.at(Punct::LParen) {
            let args = self.parse_variant_args();
            return Pattern {
                span: first.span.to(Span::new(first.span.start, self.prev_end)),
                kind: PatternKind::Variant {
                    leading_dot: false,
                    path,
                    args,
                },
            };
        }
        if self.at(Punct::LBrace) && !self.no_struct {
            return self.parse_struct_pattern(path);
        }
        if path.segments.len() == 1 {
            let name = path.segments[0];
            let kind = if after_is {
                PatternKind::TypeTest(path.clone())
            } else {
                PatternKind::Binding { mode: None, name }
            };
            return Pattern {
                kind,
                span: path.span,
            };
        }
        Pattern {
            span: path.span,
            kind: PatternKind::Variant {
                leading_dot: false,
                path,
                args: None,
            },
        }
    }

    fn parse_struct_pattern(&mut self, path: Path) -> Pattern {
        self.bump(); // `{`
        self.enter_brackets();
        let mut fields = Vec::new();
        let mut has_rest = false;
        while !self.at(Punct::RBrace) && !self.at_eof() {
            let before = self.pos;
            if has_rest {
                self.error(
                    self.tok().span,
                    "`..` must be the last thing in a struct pattern",
                );
            }
            if self.at(Punct::DotDot) {
                self.bump();
                has_rest = true;
            } else {
                let mode = self.parse_mode();
                let name = self.ident("a field name");
                let pattern = self.eat(Punct::Colon).then(|| self.parse_pattern());
                fields.push(FieldPat {
                    mode,
                    name,
                    pattern,
                });
            }
            if !self.eat(Punct::Comma) {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.leave_brackets();
        let close = self.expect(Punct::RBrace);
        Pattern {
            span: path.span.to(close),
            kind: PatternKind::Struct {
                path,
                fields,
                has_rest,
            },
        }
    }
}
