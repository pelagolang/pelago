//! Declarations: functions, types, impls, modules, imports.

use super::Parser;
use crate::ast::*;
use crate::keyword::Keyword;
use crate::span::Span;
use crate::token::{Punct, TokenKind};

/// Soft keywords that can come in front of a declaration.
const MODIFIER_WORDS: &[&str] = &[
    "open", "abstract", "final", "override", "linear", "static", "ext",
];

impl Parser<'_> {
    fn is_modifier_word(&self) -> bool {
        self.kind() == TokenKind::Ident && MODIFIER_WORDS.contains(&self.text(self.tok()))
    }

    /// Is the token after the current one something that continues a declaration header?
    fn declaration_follows(&self) -> bool {
        let next = self.peek_sig(1);
        match next.kind {
            TokenKind::Keyword(
                Keyword::Fn
                | Keyword::Struct
                | Keyword::Enum
                | Keyword::Interface
                | Keyword::Impl
                | Keyword::Obj
                | Keyword::Const
                | Keyword::Var
                | Keyword::Meta
                | Keyword::Unsafe
                | Keyword::Async
                | Keyword::Pub
                | Keyword::Priv
                | Keyword::Deinit,
            ) => true,
            TokenKind::Ident => {
                let text = self.text(next);
                MODIFIER_WORDS.contains(&text) || text == "init"
            }
            TokenKind::Str => self.text(self.tok()) == "ext",
            _ => false,
        }
    }

    /// Does the current token begin a declaration (as opposed to an ordinary statement)?
    pub(super) fn starts_item(&self) -> bool {
        let t = self.tok();
        match t.kind {
            TokenKind::Punct(Punct::At) => true,
            TokenKind::Keyword(k) => match k {
                Keyword::Fn
                | Keyword::Struct
                | Keyword::Enum
                | Keyword::Interface
                | Keyword::Impl
                | Keyword::Obj
                | Keyword::Mod
                | Keyword::Import
                | Keyword::Use
                | Keyword::Export
                | Keyword::Pub
                | Keyword::Priv
                | Keyword::Pkg
                | Keyword::Macro
                | Keyword::Deinit => true,
                Keyword::Meta | Keyword::Unsafe | Keyword::Async => {
                    self.peek_sig(1).kind == TokenKind::Keyword(Keyword::Fn)
                }
                _ => false,
            },
            TokenKind::Ident => {
                let text = self.text(t);
                match text {
                    "type" => {
                        self.peek_sig(1).kind == TokenKind::Ident
                            && matches!(
                                self.peek_sig(2).kind,
                                TokenKind::Punct(Punct::Eq | Punct::Lt)
                            )
                    }
                    "test" => self.peek_sig(1).kind == TokenKind::Str,
                    _ => self.is_modifier_word() && self.declaration_follows(),
                }
            }
            _ => false,
        }
    }

    fn parse_attr(&mut self) -> Attr {
        let at = self.bump().span; // `@`
        let name = self.ident("an attribute name");
        let mut args = Vec::new();
        let mut end = name.span;
        if self.at(Punct::LParen) && self.adjacent() {
            self.bump();
            self.enter_brackets();
            while !self.at(Punct::RParen) && !self.at_eof() {
                let before = self.pos;
                args.push(self.parse_expr());
                if !self.eat(Punct::Comma) {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.leave_brackets();
            end = self.expect(Punct::RParen);
        }
        Attr {
            name,
            args,
            span: at.to(end),
        }
    }

    /// Attributes and modifiers, then the declaration itself.
    pub(super) fn parse_item(&mut self) -> Item {
        let start = self.tok().span;
        let docs = self.docs_here();
        let mut attrs = Vec::new();
        while self.at(Punct::At) {
            attrs.push(self.parse_attr());
            self.skip_newlines();
        }
        let mods = self.parse_modifiers();
        let kind = self.parse_item_kind();
        let end = self.prev_end.max(start.end);
        Item {
            docs,
            attrs,
            mods,
            kind,
            span: Span::new(start.start, end),
        }
    }

    fn parse_modifiers(&mut self) -> Modifiers {
        let mut m = Modifiers::default();
        loop {
            let t = self.tok();
            match t.kind {
                TokenKind::Keyword(Keyword::Pub) => {
                    m.vis = Vis::Pub;
                    if self.peek_sig(1).kind == TokenKind::Keyword(Keyword::Pkg) {
                        self.bump();
                        m.vis = Vis::PubPkg;
                    }
                }
                TokenKind::Keyword(Keyword::Priv) => m.vis = Vis::Priv,
                TokenKind::Keyword(Keyword::Export) => m.vis = Vis::Export,
                TokenKind::Keyword(Keyword::Meta)
                    if self.peek_sig(1).kind == TokenKind::Keyword(Keyword::Fn) =>
                {
                    m.is_meta = true;
                }
                TokenKind::Keyword(Keyword::Async) => {
                    m.is_async = true;
                    self.error(
                        t.span,
                        "`async` is reserved for later; use threads and channels",
                    );
                }
                TokenKind::Keyword(Keyword::Unsafe)
                    if self.peek_sig(1).kind != TokenKind::Punct(Punct::LBrace) =>
                {
                    m.is_unsafe = true;
                }
                TokenKind::Ident if self.is_modifier_word() && self.declaration_follows() => {
                    match self.text(t) {
                        "open" => m.is_open = true,
                        "abstract" => m.is_abstract = true,
                        "final" => m.is_final = true,
                        "override" => m.is_override = true,
                        "linear" => m.is_linear = true,
                        "static" => m.is_static = true,
                        _ => {
                            m.is_ext = true;
                            self.bump();
                            if self.kind() == TokenKind::Str {
                                m.ext_abi = Some(self.tok().span);
                                self.bump();
                            }
                            continue;
                        }
                    }
                }
                _ => break,
            }
            self.bump();
            self.skip_newlines();
        }
        m
    }

    fn parse_item_kind(&mut self) -> ItemKind {
        let t = self.tok();
        match t.kind {
            TokenKind::Keyword(Keyword::Fn) => ItemKind::Fn(Box::new(self.parse_fn(FnKind::Fn))),
            TokenKind::Keyword(Keyword::Deinit) => {
                ItemKind::Fn(Box::new(self.parse_fn(FnKind::Deinit)))
            }
            TokenKind::Keyword(Keyword::Struct) => {
                self.bump();
                ItemKind::Struct(Box::new(self.parse_type_decl()))
            }
            TokenKind::Keyword(Keyword::Obj) => {
                self.bump();
                ItemKind::Obj(Box::new(self.parse_type_decl()))
            }
            TokenKind::Keyword(Keyword::Interface) => {
                self.bump();
                ItemKind::Interface(Box::new(self.parse_type_decl()))
            }
            TokenKind::Keyword(Keyword::Enum) => {
                self.bump();
                ItemKind::Enum(Box::new(self.parse_enum()))
            }
            TokenKind::Keyword(Keyword::Impl) => {
                self.bump();
                ItemKind::Impl(Box::new(self.parse_impl()))
            }
            TokenKind::Keyword(Keyword::Const | Keyword::Var) => ItemKind::Decl(self.parse_decl()),
            TokenKind::Keyword(Keyword::Mod) => self.parse_mod(),
            TokenKind::Keyword(Keyword::Import | Keyword::Use) => self.parse_import(),
            TokenKind::Keyword(k @ (Keyword::Pkg | Keyword::Macro)) => {
                self.error(t.span, format!("`{}` is reserved for later", k.as_str()));
                self.bump();
                self.sync_stmt();
                ItemKind::Test {
                    name: None,
                    body: Block {
                        stmts: Vec::new(),
                        tail: None,
                        span: t.span,
                    },
                }
            }
            TokenKind::Ident if self.text(t) == "type" => self.parse_type_alias(),
            TokenKind::Ident if self.text(t) == "test" => self.parse_test(),
            TokenKind::Ident if self.text(t) == "init" => {
                ItemKind::Fn(Box::new(self.parse_fn(FnKind::Init)))
            }
            _ => {
                self.error_expected("a declaration");
                self.sync_stmt();
                ItemKind::Test {
                    name: None,
                    body: Block {
                        stmts: Vec::new(),
                        tail: None,
                        span: t.span,
                    },
                }
            }
        }
    }

    // ---- Functions -------------------------------------------------------------

    /// `fn name<T>(params) -> ret where ... { body }`. Also `init` and `deinit`.
    pub(super) fn parse_fn(&mut self, kind: FnKind) -> FnDecl {
        let head = self.bump(); // `fn`, `init` or `deinit`
        let name = match kind {
            FnKind::Fn => self.ident("a function name"),
            _ => Ident { span: head.span },
        };
        let generics = self.parse_generics();
        let mut params = Vec::new();
        if self.at(Punct::LParen) {
            self.bump();
            self.enter_brackets();
            while !self.at(Punct::RParen) && !self.at_eof() {
                let before = self.pos;
                params.push(self.parse_param());
                if !self.eat(Punct::Comma) {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.leave_brackets();
            self.expect(Punct::RParen);
        } else if kind != FnKind::Deinit {
            self.error_expected("`(`");
        }
        let ret = if self.eat(Punct::Arrow) {
            Some(self.parse_type())
        } else {
            None
        };
        let where_ = self.parse_where();
        let saved_labels = std::mem::take(&mut self.labels);
        let body = if self.at(Punct::LBrace) {
            FnBody::Block(self.parse_block())
        } else if self.at(Punct::Eq) {
            self.bump();
            self.skip_newlines();
            FnBody::Expr(self.parse_expr())
        } else {
            FnBody::None
        };
        self.labels = saved_labels;
        FnDecl {
            kind,
            name,
            generics,
            params,
            ret,
            where_,
            body,
        }
    }

    fn parse_param(&mut self) -> Param {
        let start = self.tok().span;
        let mode = self.parse_mode();
        let variadic = self.eat(Punct::DotDotDot);
        let (name, is_self) = if self.at_kw(Keyword::SelfValue) {
            (
                Ident {
                    span: self.bump().span,
                },
                true,
            )
        } else {
            (self.ident("a parameter name"), false)
        };
        let ty = self.eat(Punct::Colon).then(|| self.parse_type());
        let default = if self.eat(Punct::Eq) {
            Some(self.parse_expr())
        } else {
            None
        };
        let end = self.prev_end.max(start.end);
        Param {
            mode,
            name,
            is_self,
            variadic,
            ty,
            default,
            span: Span::new(start.start, end),
        }
    }

    // ---- Types -----------------------------------------------------------------

    /// After `struct`, `obj` or `interface`.
    fn parse_type_decl(&mut self) -> TypeDecl {
        let name = self.ident("a type name");
        let generics = self.parse_generics();
        let bases = self.parse_bases();
        let where_ = self.parse_where();
        let members = self.parse_members(None);
        TypeDecl {
            name,
            generics,
            bases,
            where_,
            members,
        }
    }

    /// `: base, iface` after a type name.
    fn parse_bases(&mut self) -> Vec<Type> {
        let mut bases = Vec::new();
        if self.eat(Punct::Colon) {
            loop {
                bases.push(self.parse_type());
                if !self.eat(Punct::Comma) {
                    break;
                }
            }
        }
        bases
    }

    /// `{ members }`. Fields and methods for structs, objs and interfaces. For enums, `variants`
    /// receives the cases and the rest are items.
    fn parse_members(&mut self, mut variants: Option<&mut Vec<Variant>>) -> Vec<Member> {
        let mut members = Vec::new();
        if !self.at(Punct::LBrace) {
            self.error_expected("`{`");
            return members;
        }
        self.bump();
        self.push_frame(false);
        loop {
            while self.kind() == TokenKind::Newline || self.at(Punct::Comma) || self.at(Punct::Semi)
            {
                self.bump();
            }
            if self.at(Punct::RBrace) || self.at_eof() {
                break;
            }
            let before = self.pos;
            self.parse_member(&mut members, variants.as_deref_mut());
            // A member that ends with a block may be followed by the next one on the same line.
            let after_block = self.prev_end > 0
                && self.src.as_bytes().get(self.prev_end as usize - 1) == Some(&b'}');
            let ended = after_block
                || matches!(
                    self.kind(),
                    TokenKind::Newline
                        | TokenKind::Eof
                        | TokenKind::Punct(Punct::Comma | Punct::Semi | Punct::RBrace)
                );
            if !ended {
                self.error_expected("the end of the member");
                self.sync_stmt();
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.pop_frame();
        self.expect(Punct::RBrace);
        members
    }

    fn parse_member(&mut self, members: &mut Vec<Member>, variants: Option<&mut Vec<Variant>>) {
        let start = self.tok().span;
        let docs = self.docs_here();
        // Attributes and modifiers belong to either a field or a method.
        let mut attrs = Vec::new();
        while self.at(Punct::At) {
            attrs.push(self.parse_attr());
            self.skip_newlines();
        }
        let mods = self.parse_modifiers();
        let t = self.tok();
        let is_item = matches!(
            t.kind,
            TokenKind::Keyword(
                Keyword::Fn
                    | Keyword::Deinit
                    | Keyword::Const
                    | Keyword::Var
                    | Keyword::Struct
                    | Keyword::Enum
                    | Keyword::Obj
                    | Keyword::Impl
                    | Keyword::Interface
            )
        ) || (t.kind == TokenKind::Ident
            && self.text(t) == "init"
            && self.peek_sig(1).kind == TokenKind::Punct(Punct::LParen))
            || (t.kind == TokenKind::Ident && self.text(t) == "type" && self.starts_item());
        if is_item {
            let kind = self.parse_item_kind();
            let end = self.prev_end.max(start.end);
            members.push(Member::Item(Item {
                docs,
                attrs,
                mods,
                kind,
                span: Span::new(start.start, end),
            }));
            return;
        }
        if t.kind != TokenKind::Ident {
            self.error_expected("a field or a method");
            return;
        }
        if let Some(variants) = variants {
            variants.push(self.parse_variant(docs));
            return;
        }
        let name = self.ident("a field name");
        self.expect(Punct::Colon);
        let ty = self.parse_type();
        let default = if self.eat(Punct::Eq) {
            self.skip_newlines();
            Some(self.parse_expr())
        } else {
            None
        };
        let end = self.prev_end.max(start.end);
        members.push(Member::Field(FieldDecl {
            docs,
            attrs,
            vis: mods.vis,
            name,
            ty,
            default,
            span: Span::new(start.start, end),
        }));
    }

    fn parse_variant(&mut self, docs: Vec<Span>) -> Variant {
        let name = self.ident("a case name");
        let mut fields = Vec::new();
        if self.at(Punct::LParen) {
            self.bump();
            self.enter_brackets();
            while !self.at(Punct::RParen) && !self.at_eof() {
                let before = self.pos;
                let label = if self.kind() == TokenKind::Ident
                    && self.peek_sig(1).kind == TokenKind::Punct(Punct::Colon)
                {
                    let l = self.ident("a field name");
                    self.bump(); // `:`
                    Some(l)
                } else {
                    None
                };
                let ty = self.parse_type();
                fields.push(VariantField { label, ty });
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
        let discriminant = if self.eat(Punct::Eq) {
            Some(self.parse_expr())
        } else {
            None
        };
        Variant {
            docs,
            span: name
                .span
                .to(Span::new(name.span.start, self.prev_end.max(name.span.end))),
            name,
            fields,
            discriminant,
        }
    }

    fn parse_enum(&mut self) -> EnumDecl {
        let name = self.ident("an enum name");
        let generics = self.parse_generics();
        let bases = self.parse_bases();
        let where_ = self.parse_where();
        let mut variants = Vec::new();
        let members = self.parse_members(Some(&mut variants));
        let items = members
            .into_iter()
            .filter_map(|m| match m {
                Member::Item(i) => Some(i),
                Member::Field(_) => None,
            })
            .collect();
        EnumDecl {
            name,
            generics,
            bases,
            where_,
            variants,
            items,
        }
    }

    fn parse_impl(&mut self) -> ImplDecl {
        let generics = self.parse_generics();
        let target = self.parse_type();
        let for_type = if self.eat_kw(Keyword::For) {
            Some(self.parse_type())
        } else {
            None
        };
        let where_ = self.parse_where();
        let members = self.parse_members(None);
        ImplDecl {
            generics,
            target,
            for_type,
            where_,
            members,
        }
    }

    fn parse_type_alias(&mut self) -> ItemKind {
        self.bump(); // `type`
        let name = self.ident("a type name");
        let generics = self.parse_generics();
        self.expect(Punct::Eq);
        let ty = self.parse_type();
        ItemKind::TypeAlias { name, generics, ty }
    }

    // ---- Modules, imports, tests --------------------------------------------------

    fn parse_mod(&mut self) -> ItemKind {
        self.bump(); // `mod`
        let path = self.path("a module name");
        let body = if self.at(Punct::LBrace) {
            self.bump();
            self.push_frame(false);
            let (stmts, _) = self.parse_stmts(super::stmt::Term::Brace);
            self.pop_frame();
            self.expect(Punct::RBrace);
            Some(stmts)
        } else {
            None
        };
        ItemKind::Mod { path, body }
    }

    /// `import path`, `import name = path`, `import { a, b: c, ... } = path`.
    fn parse_import(&mut self) -> ItemKind {
        let is_use = self.bump().kind == TokenKind::Keyword(Keyword::Use);
        let kind = if self.at(Punct::LBrace) {
            self.bump();
            self.enter_brackets();
            let mut names = Vec::new();
            let mut glob = false;
            while !self.at(Punct::RBrace) && !self.at_eof() {
                let before = self.pos;
                if self.eat(Punct::DotDotDot) {
                    glob = true;
                } else {
                    let name = self.ident("a name to import");
                    let alias = self.eat(Punct::Colon).then(|| self.ident("a new name"));
                    names.push(ImportName { name, alias });
                }
                if !self.eat(Punct::Comma) {
                    break;
                }
                if self.pos == before {
                    self.bump();
                }
            }
            self.leave_brackets();
            self.expect(Punct::RBrace);
            self.expect(Punct::Eq);
            ImportKind::Names { names, glob }
        } else if self.kind() == TokenKind::Ident
            && self.peek_raw().kind == TokenKind::Punct(Punct::Eq)
        {
            let alias = self.ident("a name");
            self.bump(); // `=`
            ImportKind::Alias(alias)
        } else {
            ImportKind::Module
        };
        let levels = match self.kind() {
            TokenKind::Punct(Punct::Dot) => 1,
            TokenKind::Punct(Punct::DotDot) => 2,
            TokenKind::Punct(Punct::DotDotDot) => 3,
            _ => 0,
        };
        if levels > 0 {
            self.bump();
        }
        let path = self.path("a module name");
        ItemKind::Import(Import {
            is_use,
            kind,
            levels,
            path,
        })
    }

    fn parse_test(&mut self) -> ItemKind {
        self.bump(); // `test`
        let name = if self.kind() == TokenKind::Str {
            Some(self.parse_literal())
        } else {
            None
        };
        let body = self.parse_block();
        ItemKind::Test { name, body }
    }
}
