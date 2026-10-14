//! Blocks, statements, `for`, `switch` and `select`.

use super::Parser;
use crate::ast::*;
use crate::keyword::{foreign_keyword_hint, Keyword};
use crate::span::Span;
use crate::token::{Punct, TokenKind};

/// What ends a list of statements.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Term {
    /// A closing `}`.
    Brace,
    /// The next `case` / `default` or a closing `}` (a `switch` arm).
    Arm,
    /// The end of the file.
    Eof,
}

impl Parser<'_> {
    pub(super) fn parse_file(&mut self) -> File {
        let (stmts, _) = self.parse_stmts(Term::Eof);
        File {
            stmts,
            span: Span::new(0, self.src.len() as u32),
        }
    }

    /// `{ statements }`
    pub(super) fn parse_block(&mut self) -> Block {
        let open = self.tok();
        if !self.at(Punct::LBrace) {
            self.error_expected("`{`");
            return Block {
                stmts: Vec::new(),
                tail: None,
                span: Span::new(open.span.start, open.span.start),
            };
        }
        if !self.enter() {
            return Block {
                stmts: Vec::new(),
                tail: None,
                span: open.span,
            };
        }
        self.bump();
        self.push_frame(false);
        let (stmts, tail) = self.parse_stmts(Term::Brace);
        self.pop_frame();
        let close = self.expect(Punct::RBrace);
        self.leave();
        Block {
            stmts,
            tail,
            span: open.span.to(close),
        }
    }

    pub(super) fn at_default_arm(&self) -> bool {
        self.at_word("default") && self.peek_sig(1).kind == TokenKind::Punct(Punct::Colon)
    }

    fn at_term(&self, term: Term) -> bool {
        match term {
            Term::Eof => self.at_eof(),
            Term::Brace => self.at(Punct::RBrace) || self.at_eof(),
            Term::Arm => {
                self.at(Punct::RBrace)
                    || self.at_eof()
                    || self.at_kw(Keyword::Case)
                    || self.at_default_arm()
            }
        }
    }

    /// Statements until `term`. The last expression without a `;` becomes the block's value
    /// (except at the top level of a file).
    pub(super) fn parse_stmts(&mut self, term: Term) -> (Vec<Stmt>, Option<Box<Expr>>) {
        let mut stmts: Vec<Stmt> = Vec::new();
        let mut last_semi = false;
        loop {
            // Separators between statements.
            while self.kind() == TokenKind::Newline || self.at(Punct::Semi) {
                last_semi =
                    self.at(Punct::Semi) || (last_semi && self.kind() == TokenKind::Newline);
                self.bump();
                self.skip_newlines();
            }
            if self.at_term(term) {
                break;
            }
            // A stray closing bracket cannot start a statement; skip it so we make progress.
            if matches!(
                self.kind(),
                TokenKind::Punct(Punct::RBrace | Punct::RParen | Punct::RBracket)
            ) {
                self.error_expected("a statement");
                self.bump();
                continue;
            }
            let before = self.pos;
            let stmt = self.parse_stmt();
            stmts.push(stmt);
            last_semi = false;
            // The statement must end here.
            let ended = matches!(
                self.kind(),
                TokenKind::Newline | TokenKind::Eof | TokenKind::Punct(Punct::Semi)
            ) || self.at_term(term);
            if !ended {
                self.error_expected("the end of the statement");
                self.sync_stmt();
            }
            if self.pos == before {
                self.bump();
            }
        }
        let mut tail = None;
        if term != Term::Eof && !last_semi {
            if let Some(Stmt {
                kind: StmtKind::Expr(_),
                ..
            }) = stmts.last()
            {
                if let Some(Stmt {
                    kind: StmtKind::Expr(e),
                    ..
                }) = stmts.pop()
                {
                    tail = Some(Box::new(e));
                }
            }
        }
        (stmts, tail)
    }

    fn parse_stmt(&mut self) -> Stmt {
        let t = self.tok();
        if self.starts_item() {
            let item = self.parse_item();
            return Stmt {
                span: item.span,
                kind: StmtKind::Item(Box::new(item)),
            };
        }
        match t.kind {
            TokenKind::Keyword(Keyword::Const | Keyword::Var) => {
                let d = self.parse_decl();
                let span = Span::new(t.span.start, self.prev_end.max(t.span.end));
                Stmt {
                    kind: StmtKind::Decl(Box::new(d)),
                    span,
                }
            }
            TokenKind::Ident if self.at_label() => self.parse_labeled(),
            TokenKind::Ident => {
                // `class Foo { ... }` and friends from other languages.
                if let Some(hint) = foreign_keyword_hint(self.text(t)) {
                    if matches!(
                        self.peek_raw().kind,
                        TokenKind::Ident | TokenKind::Punct(Punct::LBrace)
                    ) {
                        self.error(
                            t.span,
                            format!("`{}` is not a keyword here: {hint}", self.text(t)),
                        );
                        self.sync_stmt();
                        return Stmt {
                            kind: StmtKind::Expr(Expr {
                                kind: ExprKind::Error,
                                span: t.span,
                            }),
                            span: t.span,
                        };
                    }
                }
                self.parse_expr_stmt()
            }
            _ => self.parse_expr_stmt(),
        }
    }

    /// `name: for ...`, `name: while ...`, `name: loop ...`, `name: do ...`, `name: { ... }`
    fn at_label(&self) -> bool {
        self.peek_raw().kind == TokenKind::Punct(Punct::Colon)
            && matches!(
                self.peek_sig(2).kind,
                TokenKind::Keyword(Keyword::For | Keyword::While | Keyword::Loop | Keyword::Do)
                    | TokenKind::Punct(Punct::LBrace)
            )
    }

    fn parse_labeled(&mut self) -> Stmt {
        let label = self.ident("a label");
        self.bump(); // `:`
        self.skip_newlines();
        if self
            .labels
            .iter()
            .any(|l| l.text(self.src) == label.text(self.src))
        {
            self.error(
                label.span,
                "this label is already used by an enclosing loop",
            );
        }
        self.labels.push(label);
        let body = self.parse_expr();
        self.labels.pop();
        let span = label.span.to(body.span);
        Stmt {
            kind: StmtKind::Expr(Expr {
                kind: ExprKind::Labeled {
                    label,
                    body: Box::new(body),
                },
                span,
            }),
            span,
        }
    }

    fn parse_expr_stmt(&mut self) -> Stmt {
        let start = self.tok().span;
        let mut e = self.parse_expr();
        if self.at(Punct::PlusPlus) || self.at(Punct::MinusMinus) {
            let op = if self.at(Punct::PlusPlus) {
                Punct::PlusPlus
            } else {
                Punct::MinusMinus
            };
            let t = self.bump();
            let span = e.span.to(t.span);
            e = Expr {
                kind: ExprKind::IncDec {
                    op,
                    operand: Box::new(e),
                },
                span,
            };
        }
        let span = start.to(e.span);
        Stmt {
            kind: StmtKind::Expr(e),
            span,
        }
    }

    /// `const x: T = init` / `var x = init`
    pub(super) fn parse_decl(&mut self) -> Decl {
        let kind = if self.at_kw(Keyword::Const) {
            DeclKind::Const
        } else {
            DeclKind::Var
        };
        self.bump();
        let pattern = self.parse_pattern();
        let ty = self.eat(Punct::Colon).then(|| self.parse_type());
        let init = if self.eat(Punct::Eq) {
            self.skip_newlines();
            Some(self.parse_expr())
        } else {
            None
        };
        Decl {
            kind,
            pattern,
            ty,
            init,
        }
    }

    // ---- for ---------------------------------------------------------------------

    pub(super) fn parse_for(&mut self) -> Expr {
        let start = self.bump().span; // `for`
                                      // `for (var i = 0; i < n; i += 1)`
        if self.at(Punct::LParen)
            && matches!(
                self.peek_sig(1).kind,
                TokenKind::Keyword(Keyword::Var | Keyword::Const) | TokenKind::Punct(Punct::Semi)
            )
        {
            return self.parse_c_for(start);
        }
        let mut pats = Vec::new();
        loop {
            pats.push(self.parse_pattern());
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        let head = if self.eat_kw(Keyword::Of) {
            let iter = self.parse_cond();
            ForHead::Of { pats, iter }
        } else if self.eat_kw(Keyword::In) {
            if pats.len() > 1 {
                self.error(
                    pats[1].span,
                    "`for ... in` takes one name; use `of` for `i, x of items`",
                );
            }
            let iter = self.parse_cond();
            let pat = pats.swap_remove(0);
            ForHead::In { pat, iter }
        } else {
            self.error_expected("`of` or `in`");
            let s = self.tok().span;
            ForHead::Of {
                pats,
                iter: Expr {
                    kind: ExprKind::Error,
                    span: s,
                },
            }
        };
        let body = self.parse_block();
        Expr {
            span: start.to(body.span),
            kind: ExprKind::For(Box::new(ForExpr { head, body })),
        }
    }

    fn parse_c_for(&mut self, start: Span) -> Expr {
        self.bump(); // `(`
        self.enter_brackets();
        let init = if self.at(Punct::Semi) {
            None
        } else if matches!(
            self.kind(),
            TokenKind::Keyword(Keyword::Var | Keyword::Const)
        ) {
            let t = self.tok();
            let d = self.parse_decl();
            Some(Box::new(Stmt {
                span: Span::new(t.span.start, self.prev_end.max(t.span.end)),
                kind: StmtKind::Decl(Box::new(d)),
            }))
        } else {
            Some(Box::new(self.parse_expr_stmt()))
        };
        self.expect(Punct::Semi);
        let cond = if self.at(Punct::Semi) {
            None
        } else {
            Some(self.parse_expr())
        };
        self.expect(Punct::Semi);
        let step = if self.at(Punct::RParen) {
            None
        } else {
            let Stmt { kind, .. } = self.parse_expr_stmt();
            match kind {
                StmtKind::Expr(e) => Some(e),
                _ => None,
            }
        };
        self.leave_brackets();
        self.expect(Punct::RParen);
        let body = self.parse_block();
        Expr {
            span: start.to(body.span),
            kind: ExprKind::For(Box::new(ForExpr {
                head: ForHead::C { init, cond, step },
                body,
            })),
        }
    }

    // ---- switch and select ----------------------------------------------------------

    pub(super) fn parse_switch(&mut self) -> Expr {
        let start = self.bump().span; // `switch`
        let mode = self.parse_mode();
        let scrutinee = self.parse_cond();
        let mut arms = Vec::new();
        if !self.at(Punct::LBrace) {
            self.error_expected("`{`");
        } else {
            self.bump();
            self.push_frame(false);
            loop {
                self.skip_newlines();
                while self.at(Punct::Semi) {
                    self.bump();
                    self.skip_newlines();
                }
                if self.at(Punct::RBrace) || self.at_eof() {
                    break;
                }
                let before = self.pos;
                if self.at_kw(Keyword::Case) {
                    arms.push(self.parse_case_arm());
                } else if self.at_default_arm() {
                    arms.push(self.parse_default_arm());
                } else {
                    self.error_expected("`case` or `default`");
                    self.sync_stmt();
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.pop_frame();
        }
        let close = self.expect(Punct::RBrace);
        Expr {
            span: start.to(close),
            kind: ExprKind::Switch(Box::new(Switch {
                mode,
                scrutinee,
                arms,
            })),
        }
    }

    fn parse_case_arm(&mut self) -> SwitchArm {
        let start = self.bump().span; // `case`
        let mut patterns = Vec::new();
        loop {
            patterns.push(self.parse_pattern());
            if !self.eat(Punct::Comma) {
                break;
            }
        }
        let guard = if self.eat_kw(Keyword::If) {
            Some(self.parse_expr())
        } else {
            None
        };
        self.expect(Punct::Colon);
        let body = self.parse_arm_body();
        SwitchArm {
            span: start.to(body.span),
            patterns,
            is_default: false,
            guard,
            body,
        }
    }

    fn parse_default_arm(&mut self) -> SwitchArm {
        let start = self.bump().span; // `default`
        self.expect(Punct::Colon);
        let body = self.parse_arm_body();
        SwitchArm {
            span: start.to(body.span),
            patterns: Vec::new(),
            is_default: true,
            guard: None,
            body,
        }
    }

    fn parse_arm_body(&mut self) -> Block {
        let start = self.tok().span.start;
        let (stmts, tail) = self.parse_stmts(Term::Arm);
        let end = self.prev_end.max(start);
        Block {
            stmts,
            tail,
            span: Span::new(start, end),
        }
    }

    pub(super) fn parse_select(&mut self) -> Expr {
        let start = self.bump().span; // `select`
        let mut arms = Vec::new();
        self.bump(); // `{`
        self.push_frame(false);
        loop {
            self.skip_newlines();
            if self.at(Punct::RBrace) || self.at_eof() {
                break;
            }
            let before = self.pos;
            if self.at_kw(Keyword::Case) {
                let case = self.bump().span;
                let bind = if self.kind() == TokenKind::Ident
                    && self.peek_sig(1).kind == TokenKind::Punct(Punct::Eq)
                {
                    let id = self.ident("a name");
                    self.bump(); // `=`
                    Some(id)
                } else {
                    None
                };
                let op = self.parse_cond();
                let body = self.parse_block();
                arms.push(SelectArm {
                    span: case.to(body.span),
                    bind,
                    op: Some(op),
                    body,
                });
            } else if self.at_word("default") {
                let d = self.bump().span;
                let body = self.parse_block();
                arms.push(SelectArm {
                    span: d.to(body.span),
                    bind: None,
                    op: None,
                    body,
                });
            } else {
                self.error_expected("`case` or `default`");
                self.sync_stmt();
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.pop_frame();
        let close = self.expect(Punct::RBrace);
        Expr {
            kind: ExprKind::Select(arms),
            span: start.to(close),
        }
    }
}
