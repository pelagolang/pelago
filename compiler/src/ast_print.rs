//! Prints a syntax tree as s-expressions, for tests and for `langc parse`.
//!
//! Short nodes stay on one line; long ones break after the head. Literals print as their source
//! text, operators as their symbol, and the shape follows the tree one to one, so a golden file is
//! easy to read and to review.

use crate::ast::*;
use crate::token::Punct;

/// A printable tree.
pub enum Sexp {
    Atom(String),
    List(Vec<Sexp>),
}

fn atom(s: impl Into<String>) -> Sexp {
    Sexp::Atom(s.into())
}

fn list(head: &str, mut rest: Vec<Sexp>) -> Sexp {
    let mut all = Vec::with_capacity(rest.len() + 1);
    all.push(atom(head));
    all.append(&mut rest);
    Sexp::List(all)
}

impl Sexp {
    fn flat(&self, out: &mut String) {
        match self {
            Sexp::Atom(s) => out.push_str(s),
            Sexp::List(items) => {
                out.push('(');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    item.flat(out);
                }
                out.push(')');
            }
        }
    }

    fn render(&self, indent: usize, width: usize, out: &mut String) {
        let mut flat = String::new();
        self.flat(&mut flat);
        if indent + flat.chars().count() <= width || matches!(self, Sexp::Atom(_)) {
            out.push_str(&flat);
            return;
        }
        let Sexp::List(items) = self else { return };
        out.push('(');
        for (i, item) in items.iter().enumerate() {
            if i == 0 {
                item.render(indent + 1, width, out);
            } else {
                out.push('\n');
                for _ in 0..indent + 2 {
                    out.push(' ');
                }
                item.render(indent + 2, width, out);
            }
        }
        out.push(')');
    }

    /// The text form, broken into lines of about `width` columns.
    pub fn to_pretty(&self, width: usize) -> String {
        let mut out = String::new();
        self.render(0, width, &mut out);
        out
    }

    /// The text form on one line.
    pub fn to_flat(&self) -> String {
        let mut out = String::new();
        self.flat(&mut out);
        out
    }
}

pub struct Printer<'a> {
    pub src: &'a str,
}

fn mode_str(m: Option<Mode>) -> Option<&'static str> {
    m.map(Mode::as_str)
}

impl Printer<'_> {
    fn text(&self, span: crate::span::Span) -> String {
        self.src
            .get(span.start as usize..span.end as usize)
            .unwrap_or("")
            .to_string()
    }

    fn id(&self, i: &Ident) -> Sexp {
        atom(i.text(self.src))
    }

    fn path_text(&self, p: &Path) -> String {
        p.segments
            .iter()
            .map(|s| s.text(self.src))
            .collect::<Vec<_>>()
            .join(".")
    }

    pub fn file(&self, f: &File) -> Sexp {
        list("file", f.stmts.iter().map(|s| self.stmt(s)).collect())
    }

    // ---- Types -----------------------------------------------------------------

    pub fn ty(&self, t: &Type) -> Sexp {
        match &t.kind {
            TypeKind::Path { path, args } => {
                if args.is_empty() {
                    atom(self.path_text(path))
                } else {
                    list(
                        &self.path_text(path),
                        args.iter().map(|a| self.generic_arg(a)).collect(),
                    )
                }
            }
            TypeKind::SelfType => atom("Self"),
            TypeKind::List(e) => list("list", vec![self.ty(e)]),
            TypeKind::Array(e, n) => list("array", vec![self.ty(e), self.expr(n)]),
            TypeKind::Tuple(ts) => list("tuple", ts.iter().map(|t| self.ty(t)).collect()),
            TypeKind::FixedTuple(ts, n) => {
                let mut v: Vec<Sexp> = ts.iter().map(|t| self.ty(t)).collect();
                v.push(atom(";"));
                v.push(self.expr(n));
                list("tuple", v)
            }
            TypeKind::Option(e) => list("option", vec![self.ty(e)]),
            TypeKind::Result(a, b) => list("result", vec![self.ty(a), self.ty(b)]),
            TypeKind::Fn { params, ret } => {
                let ps = params
                    .iter()
                    .map(|p| match p.mode {
                        Some(m) => list(m.as_str(), vec![self.ty(&p.ty)]),
                        None => self.ty(&p.ty),
                    })
                    .collect();
                let mut v = vec![Sexp::List(ps)];
                if let Some(r) = ret {
                    v.push(atom("->"));
                    v.push(self.ty(r));
                }
                list("fn", v)
            }
            TypeKind::Any(e) => list("any", vec![self.ty(e)]),
            TypeKind::Error => atom("<error>"),
        }
    }

    fn generic_arg(&self, a: &GenericArg) -> Sexp {
        match a {
            GenericArg::Type(t) => self.ty(t),
            GenericArg::Const(e) => self.expr(e),
        }
    }

    // ---- Patterns --------------------------------------------------------------

    pub fn pattern(&self, p: &Pattern) -> Sexp {
        match &p.kind {
            PatternKind::Wildcard => atom("_"),
            PatternKind::Binding { mode, name } => match mode_str(*mode) {
                Some(m) => list(m, vec![self.id(name)]),
                None => self.id(name),
            },
            PatternKind::As { name, pattern } => {
                list("as", vec![self.id(name), self.pattern(pattern)])
            }
            PatternKind::Literal(e) => self.expr(e),
            PatternKind::Range {
                start,
                end,
                inclusive,
            } => list(
                if *inclusive { "..=" } else { ".." },
                vec![
                    start.as_ref().map_or(atom("_"), |e| self.expr(e)),
                    end.as_ref().map_or(atom("_"), |e| self.expr(e)),
                ],
            ),
            PatternKind::Tuple(ps) => list("tuple", ps.iter().map(|p| self.pattern(p)).collect()),
            PatternKind::List(items) => list(
                "list",
                items
                    .iter()
                    .map(|i| match i {
                        ListPatItem::Pattern(p) => self.pattern(p),
                        ListPatItem::Rest { mode, name, .. } => {
                            let mut v = Vec::new();
                            if let Some(m) = mode_str(*mode) {
                                v.push(atom(m));
                            }
                            if let Some(n) = name {
                                v.push(self.id(n));
                            }
                            list("...", v)
                        }
                    })
                    .collect(),
            ),
            PatternKind::Variant {
                leading_dot,
                path,
                args,
            } => {
                let name = format!(
                    "{}{}",
                    if *leading_dot { "." } else { "" },
                    self.path_text(path)
                );
                match args {
                    None => atom(name),
                    Some(args) => list(
                        &name,
                        args.iter()
                            .map(|a| match &a.label {
                                Some(l) => list(
                                    &format!("{}:", l.text(self.src)),
                                    vec![self.pattern(&a.pattern)],
                                ),
                                None => self.pattern(&a.pattern),
                            })
                            .collect(),
                    ),
                }
            }
            PatternKind::None => atom("none"),
            PatternKind::TypeTest(path) => list("type", vec![atom(self.path_text(path))]),
            PatternKind::Struct {
                path,
                fields,
                has_rest,
            } => {
                let mut v: Vec<Sexp> = fields
                    .iter()
                    .map(|f| {
                        let name = self.id(&f.name);
                        let mut items = Vec::new();
                        if let Some(m) = mode_str(f.mode) {
                            items.push(atom(m));
                        }
                        match &f.pattern {
                            Some(p) => {
                                items.push(name);
                                items.push(self.pattern(p));
                                Sexp::List(items)
                            }
                            None if items.is_empty() => name,
                            None => {
                                items.push(name);
                                Sexp::List(items)
                            }
                        }
                    })
                    .collect();
                if *has_rest {
                    v.push(atom(".."));
                }
                list(&format!("struct {}", self.path_text(path)), v)
            }
            PatternKind::Error => atom("<error>"),
        }
    }

    // ---- Expressions -------------------------------------------------------------

    fn exprs(&self, es: &[Expr]) -> Vec<Sexp> {
        es.iter().map(|e| self.expr(e)).collect()
    }

    fn list_item(&self, i: &ListItem) -> Sexp {
        if i.spread {
            list("...", vec![self.expr(&i.value)])
        } else {
            self.expr(&i.value)
        }
    }

    pub fn expr(&self, e: &Expr) -> Sexp {
        match &e.kind {
            ExprKind::Int | ExprKind::Float | ExprKind::Char => atom(self.text(e.span)),
            ExprKind::Bool(b) => atom(if *b { "true" } else { "false" }),
            ExprKind::None => atom("none"),
            ExprKind::Str(parts) => {
                let mut v = Vec::new();
                for p in parts {
                    match p {
                        StrPart::Text(s) => v.push(atom(format!("{:?}", self.text(*s)))),
                        StrPart::Interp(e) => v.push(list("#", vec![self.expr(e)])),
                    }
                }
                list("str", v)
            }
            ExprKind::Name(id) => self.id(id),
            ExprKind::SelfValue => atom("self"),
            ExprKind::SelfType => atom("Self"),
            ExprKind::Underscore => atom("_"),
            ExprKind::DotName(id) => atom(format!(".{}", id.text(self.src))),
            ExprKind::Paren(inner) => list("paren", vec![self.expr(inner)]),
            ExprKind::Tuple(es) => list("tuple", self.exprs(es)),
            ExprKind::List(items) => {
                list("list", items.iter().map(|i| self.list_item(i)).collect())
            }
            ExprKind::Repeat { value, count } => {
                list("repeat", vec![self.expr(value), self.expr(count)])
            }
            ExprKind::Map(items) => list(
                "map",
                items
                    .iter()
                    .map(|i| match i {
                        MapItem::Pair(k, v) => list(":", vec![self.expr(k), self.expr(v)]),
                        MapItem::Spread(e) => list("...", vec![self.expr(e)]),
                    })
                    .collect(),
            ),
            ExprKind::Set(items) => list("set", items.iter().map(|i| self.list_item(i)).collect()),
            ExprKind::Builder(b) => self.builder(b),
            ExprKind::StructLit { ty, fields } => {
                let mut v = vec![self.ty(ty)];
                for f in fields {
                    match f {
                        StructInit::Field(f) => match &f.value {
                            Some(val) => v.push(list(":", vec![self.id(&f.name), self.expr(val)])),
                            None => v.push(self.id(&f.name)),
                        },
                        StructInit::Spread(e) => v.push(list("...", vec![self.expr(e)])),
                    }
                }
                list("new", v)
            }
            ExprKind::Field {
                base,
                name,
                optional,
            } => list(
                if *optional { "?." } else { "." },
                vec![self.expr(base), self.id(name)],
            ),
            ExprKind::TupleIndex { base, index } => {
                list(&format!(".{}", self.text(*index)), vec![self.expr(base)])
            }
            ExprKind::Index { base, index } => {
                list("index", vec![self.expr(base), self.expr(index)])
            }
            ExprKind::Call { callee, args, mark } => {
                let head = match mark {
                    CallMark::None => "call",
                    CallMark::Changes => "call!",
                    CallMark::Consumes => "call!!",
                };
                let mut v = vec![self.expr(callee)];
                v.extend(args.iter().map(|a| self.arg(a)));
                list(head, v)
            }
            ExprKind::TypeApply { base, args } => {
                let mut v = vec![self.expr(base)];
                v.extend(args.iter().map(|a| self.generic_arg(a)));
                list("apply", v)
            }
            ExprKind::Unary { op, operand } => {
                let head = match op {
                    Punct::Minus => "neg",
                    Punct::Bang => "not",
                    Punct::Tilde => "bitnot",
                    other => other.as_str(),
                };
                list(head, vec![self.expr(operand)])
            }
            ExprKind::Copy(inner) => list("copy", vec![self.expr(inner)]),
            ExprKind::Binary { op, lhs, rhs } => {
                list(op.as_str(), vec![self.expr(lhs), self.expr(rhs)])
            }
            ExprKind::In { lhs, rhs } => list("in", vec![self.expr(lhs), self.expr(rhs)]),
            ExprKind::Assign { op, lhs, rhs } => {
                list(op.as_str(), vec![self.expr(lhs), self.expr(rhs)])
            }
            ExprKind::IncDec { op, operand } => list(op.as_str(), vec![self.expr(operand)]),
            ExprKind::Range {
                start,
                end,
                inclusive,
                step,
            } => {
                let mut v = vec![
                    start.as_ref().map_or(atom("_"), |e| self.expr(e)),
                    end.as_ref().map_or(atom("_"), |e| self.expr(e)),
                ];
                if let Some(s) = step {
                    v.push(atom("step"));
                    v.push(self.expr(s));
                }
                list(if *inclusive { "..=" } else { ".." }, v)
            }
            ExprKind::Ternary { cond, then, else_ } => list(
                "?:",
                vec![self.expr(cond), self.expr(then), self.expr(else_)],
            ),
            ExprKind::Try(inner) => list("try", vec![self.expr(inner)]),
            ExprKind::Is { value, pattern } => {
                list("is", vec![self.expr(value), self.pattern(pattern)])
            }
            ExprKind::Convert { value, ty, checked } => list(
                if *checked { "to?" } else { "to" },
                vec![self.expr(value), self.ty(ty)],
            ),
            ExprKind::Closure(c) => {
                let mut v = Vec::new();
                if !c.captures.is_empty() {
                    v.push(list(
                        "capture",
                        c.captures
                            .iter()
                            .map(|c| match mode_str(c.mode) {
                                Some(m) => list(m, vec![self.id(&c.name)]),
                                None => self.id(&c.name),
                            })
                            .collect(),
                    ));
                }
                v.push(Sexp::List(
                    c.params
                        .iter()
                        .map(|p| match &p.ty {
                            Some(t) => list(":", vec![self.pattern(&p.pattern), self.ty(t)]),
                            None => self.pattern(&p.pattern),
                        })
                        .collect(),
                ));
                if let Some(r) = &c.ret {
                    v.push(atom("->"));
                    v.push(self.ty(r));
                }
                v.push(self.expr(&c.body));
                list("~>", v)
            }
            ExprKind::If(i) => self.if_expr("if", i),
            ExprKind::Unless(i) => self.if_expr("unless", i),
            ExprKind::MetaIf(i) => self.if_expr("meta-if", i),
            ExprKind::Switch(s) => {
                let mut v = Vec::new();
                if let Some(m) = mode_str(s.mode) {
                    v.push(atom(m));
                }
                v.push(self.expr(&s.scrutinee));
                for arm in &s.arms {
                    let mut a = Vec::new();
                    if arm.is_default {
                        a.push(atom("default"));
                    } else {
                        a.push(Sexp::List(
                            arm.patterns.iter().map(|p| self.pattern(p)).collect(),
                        ));
                    }
                    if let Some(g) = &arm.guard {
                        a.push(list("if", vec![self.expr(g)]));
                    }
                    a.push(self.block(&arm.body));
                    v.push(list("case", a));
                }
                list("switch", v)
            }
            ExprKind::Select(arms) => list(
                "select",
                arms.iter()
                    .map(|arm| {
                        let mut a = Vec::new();
                        match &arm.op {
                            Some(op) => {
                                if let Some(b) = &arm.bind {
                                    a.push(list("=", vec![self.id(b), self.expr(op)]));
                                } else {
                                    a.push(self.expr(op));
                                }
                            }
                            None => a.push(atom("default")),
                        }
                        a.push(self.block(&arm.body));
                        list("case", a)
                    })
                    .collect(),
            ),
            ExprKind::Scope { name, body } => list("scope", vec![self.id(name), self.block(body)]),
            ExprKind::Block(b) => self.block(b),
            ExprKind::Unsafe(b) => list("unsafe", vec![self.block(b)]),
            ExprKind::Loop(b) => list("loop", vec![self.block(b)]),
            ExprKind::While { cond, body } => {
                list("while", vec![self.expr(cond), self.block(body)])
            }
            ExprKind::DoWhile { body, cond } => {
                list("do-while", vec![self.block(body), self.expr(cond)])
            }
            ExprKind::For(f) => match &f.head {
                ForHead::Of { pats, iter } => list(
                    "for-of",
                    vec![
                        Sexp::List(pats.iter().map(|p| self.pattern(p)).collect()),
                        self.expr(iter),
                        self.block(&f.body),
                    ],
                ),
                ForHead::In { pat, iter } => list(
                    "for-in",
                    vec![self.pattern(pat), self.expr(iter), self.block(&f.body)],
                ),
                ForHead::C { init, cond, step } => list(
                    "for-c",
                    vec![
                        init.as_ref().map_or(atom("_"), |s| self.stmt(s)),
                        cond.as_ref().map_or(atom("_"), |e| self.expr(e)),
                        step.as_ref().map_or(atom("_"), |e| self.expr(e)),
                        self.block(&f.body),
                    ],
                ),
            },
            ExprKind::Guard { cond, else_ } => {
                list("guard", vec![self.expr(cond), self.block(else_)])
            }
            ExprKind::Return(v) => self.opt_list("return", v),
            ExprKind::Break { label, value } => {
                let mut v = Vec::new();
                if let Some(l) = label {
                    v.push(self.id(l));
                }
                if let Some(e) = value {
                    v.push(self.expr(e));
                }
                if v.is_empty() {
                    atom("break")
                } else {
                    list("break", v)
                }
            }
            ExprKind::Labeled { label, body } => {
                list("label", vec![self.id(label), self.expr(body)])
            }
            ExprKind::Yield(v) => self.opt_list("yield", v),
            ExprKind::Continue { label } => match label {
                Some(l) => list("continue", vec![self.id(l)]),
                None => atom("continue"),
            },
            ExprKind::Become(c) => list("become", vec![self.expr(c)]),
            ExprKind::Defer(c) => list("defer", vec![self.expr(c)]),
            ExprKind::Meta(c) => list("meta", vec![self.expr(c)]),
            ExprKind::Error => atom("<error>"),
        }
    }

    fn opt_list(&self, head: &str, v: &Option<Box<Expr>>) -> Sexp {
        match v {
            Some(e) => list(head, vec![self.expr(e)]),
            None => atom(head),
        }
    }

    fn arg(&self, a: &Arg) -> Sexp {
        if a.mode.is_none() && a.label.is_none() && !a.spread {
            return self.expr(&a.value);
        }
        let mut v = Vec::new();
        if let Some(m) = mode_str(a.mode) {
            v.push(atom(m));
        }
        if let Some(l) = &a.label {
            v.push(atom(format!("{}:", l.text(self.src))));
        }
        if a.spread {
            v.push(atom("..."));
        }
        v.push(self.expr(&a.value));
        list("arg", v)
    }

    fn builder(&self, b: &Builder) -> Sexp {
        let head = match b.kind {
            BuilderKind::List => "build-list",
            BuilderKind::Set => "build-set",
            BuilderKind::Map => "build-map",
            BuilderKind::Stream => "build-stream",
        };
        let mut v: Vec<Sexp> = b
            .clauses
            .iter()
            .map(|c| match c {
                Clause::Of { pats, source } => list(
                    "of",
                    vec![
                        Sexp::List(pats.iter().map(|p| self.pattern(p)).collect()),
                        self.expr(source),
                    ],
                ),
                Clause::If(e) => list("if", vec![self.expr(e)]),
            })
            .collect();
        if let Some(r) = &b.result {
            v.push(atom("~>"));
            v.push(self.expr(r));
        }
        if let Some(r) = &b.value {
            v.push(atom(":"));
            v.push(self.expr(r));
        }
        list(head, v)
    }

    fn if_expr(&self, head: &str, i: &IfExpr) -> Sexp {
        let mut v = vec![self.expr(&i.cond), self.block(&i.then)];
        if let Some(e) = &i.else_ {
            v.push(list("else", vec![self.expr(e)]));
        }
        list(head, v)
    }

    // ---- Statements and items --------------------------------------------------

    pub fn block(&self, b: &Block) -> Sexp {
        let mut v: Vec<Sexp> = b.stmts.iter().map(|s| self.stmt(s)).collect();
        if let Some(t) = &b.tail {
            v.push(list("tail", vec![self.expr(t)]));
        }
        list("block", v)
    }

    pub fn stmt(&self, s: &Stmt) -> Sexp {
        match &s.kind {
            StmtKind::Decl(d) => self.decl(d),
            StmtKind::Expr(e) => self.expr(e),
            StmtKind::Item(i) => self.item(i),
        }
    }

    fn decl(&self, d: &Decl) -> Sexp {
        let head = match d.kind {
            DeclKind::Var => "var",
            DeclKind::Const => "const",
        };
        let mut v = vec![self.pattern(&d.pattern)];
        if let Some(t) = &d.ty {
            v.push(list(":", vec![self.ty(t)]));
        }
        if let Some(e) = &d.init {
            v.push(self.expr(e));
        }
        list(head, v)
    }

    fn generics(&self, g: &Generics) -> Option<Sexp> {
        if g.params.is_empty() {
            return None;
        }
        Some(list(
            "generics",
            g.params
                .iter()
                .map(|p| {
                    let mut v = Vec::new();
                    if p.is_meta {
                        v.push(atom("meta"));
                    }
                    v.push(self.id(&p.name));
                    for b in &p.bounds {
                        v.push(self.ty(b));
                    }
                    if v.len() == 1 {
                        v.remove(0)
                    } else {
                        Sexp::List(v)
                    }
                })
                .collect(),
        ))
    }

    fn where_clause(&self, w: &[WherePred]) -> Option<Sexp> {
        if w.is_empty() {
            return None;
        }
        Some(list(
            "where",
            w.iter()
                .map(|p| {
                    let mut v = vec![self.ty(&p.ty)];
                    v.extend(p.bounds.iter().map(|b| self.ty(b)));
                    Sexp::List(v)
                })
                .collect(),
        ))
    }

    fn mods(&self, m: &Modifiers) -> Vec<Sexp> {
        let mut v = Vec::new();
        match m.vis {
            Vis::Default => {}
            Vis::Pub => v.push(atom("pub")),
            Vis::Priv => v.push(atom("priv")),
            Vis::Export => v.push(atom("export")),
        }
        for (on, w) in [
            (m.is_open, "open"),
            (m.is_abstract, "abstract"),
            (m.is_final, "final"),
            (m.is_override, "override"),
            (m.is_meta, "meta"),
            (m.is_async, "async"),
            (m.is_unsafe, "unsafe"),
            (m.is_linear, "linear"),
            (m.is_static, "static"),
            (m.is_ext, "ext"),
        ] {
            if on {
                v.push(atom(w));
            }
        }
        v
    }

    fn fn_decl(&self, f: &FnDecl) -> Vec<Sexp> {
        let mut v = Vec::new();
        if f.kind == FnKind::Fn {
            v.push(self.id(&f.name));
        }
        v.extend(self.generics(&f.generics));
        v.push(Sexp::List(
            f.params
                .iter()
                .map(|p| {
                    let mut items = Vec::new();
                    if let Some(m) = mode_str(p.mode) {
                        items.push(atom(m));
                    }
                    if p.variadic {
                        items.push(atom("..."));
                    }
                    items.push(self.id(&p.name));
                    if let Some(t) = &p.ty {
                        items.push(atom(":"));
                        items.push(self.ty(t));
                    }
                    if let Some(d) = &p.default {
                        items.push(atom("="));
                        items.push(self.expr(d));
                    }
                    if items.len() == 1 {
                        items.remove(0)
                    } else {
                        Sexp::List(items)
                    }
                })
                .collect(),
        ));
        if let Some(r) = &f.ret {
            v.push(atom("->"));
            v.push(self.ty(r));
        }
        v.extend(self.where_clause(&f.where_));
        match &f.body {
            FnBody::Block(b) => v.push(self.block(b)),
            FnBody::Expr(e) => {
                v.push(atom("="));
                v.push(self.expr(e));
            }
            FnBody::None => {}
        }
        v
    }

    fn member(&self, m: &Member) -> Sexp {
        match m {
            Member::Field(f) => {
                let mut v = Vec::new();
                v.extend(self.docs(&f.docs));
                for a in &f.attrs {
                    v.push(self.attr(a));
                }
                if f.vis == Vis::Pub {
                    v.push(atom("pub"));
                } else if f.vis == Vis::Priv {
                    v.push(atom("priv"));
                }
                v.push(self.id(&f.name));
                v.push(atom(":"));
                v.push(self.ty(&f.ty));
                if let Some(d) = &f.default {
                    v.push(atom("="));
                    v.push(self.expr(d));
                }
                list("field", v)
            }
            Member::Item(i) => self.item(i),
        }
    }

    fn docs(&self, docs: &[crate::span::Span]) -> Option<Sexp> {
        if docs.is_empty() {
            return None;
        }
        Some(list(
            "doc",
            vec![atom(format!("{:?}", doc_text(self.src, docs)))],
        ))
    }

    fn attr(&self, a: &Attr) -> Sexp {
        let mut v = vec![self.id(&a.name)];
        v.extend(self.exprs(&a.args));
        list("@", v)
    }

    fn type_decl(&self, head: &str, d: &TypeDecl, mods: Vec<Sexp>) -> Sexp {
        let mut v = mods;
        v.push(self.id(&d.name));
        v.extend(self.generics(&d.generics));
        if !d.bases.is_empty() {
            v.push(list(":", d.bases.iter().map(|b| self.ty(b)).collect()));
        }
        v.extend(self.where_clause(&d.where_));
        v.extend(d.members.iter().map(|m| self.member(m)));
        list(head, v)
    }

    pub fn item(&self, i: &Item) -> Sexp {
        let mut pre: Vec<Sexp> = self.docs(&i.docs).into_iter().collect();
        pre.extend(i.attrs.iter().map(|a| self.attr(a)));
        pre.extend(self.mods(&i.mods));
        let body = match &i.kind {
            ItemKind::Mod { path, body } => {
                let mut v = vec![atom(self.path_text(path))];
                if let Some(b) = body {
                    v.extend(b.iter().map(|s| self.stmt(s)));
                }
                list("mod", v)
            }
            ItemKind::Import(imp) => {
                let mut v = Vec::new();
                match &imp.kind {
                    ImportKind::Module => {}
                    ImportKind::Alias(a) => {
                        v.push(self.id(a));
                        v.push(atom("="));
                    }
                    ImportKind::Names { names, glob } => {
                        let mut items: Vec<Sexp> = names
                            .iter()
                            .map(|n| match &n.alias {
                                Some(a) => list(":", vec![self.id(&n.name), self.id(a)]),
                                None => self.id(&n.name),
                            })
                            .collect();
                        if *glob {
                            items.push(atom("..."));
                        }
                        v.push(Sexp::List(items));
                        v.push(atom("="));
                    }
                }
                let dots = ".".repeat(usize::from(imp.levels));
                v.push(atom(format!("{dots}{}", self.path_text(&imp.path))));
                list(if imp.is_use { "use" } else { "import" }, v)
            }
            ItemKind::Decl(d) => self.decl(d),
            ItemKind::Fn(f) => {
                let head = match f.kind {
                    FnKind::Fn => "fn",
                    FnKind::Init => "init",
                    FnKind::Deinit => "deinit",
                };
                list(head, self.fn_decl(f))
            }
            ItemKind::Struct(d) => self.type_decl("struct", d, Vec::new()),
            ItemKind::Obj(d) => self.type_decl("obj", d, Vec::new()),
            ItemKind::Interface(d) => self.type_decl("interface", d, Vec::new()),
            ItemKind::Enum(e) => {
                let mut v = vec![self.id(&e.name)];
                v.extend(self.generics(&e.generics));
                if !e.bases.is_empty() {
                    v.push(list(":", e.bases.iter().map(|b| self.ty(b)).collect()));
                }
                v.extend(self.where_clause(&e.where_));
                for c in &e.variants {
                    let mut cv = vec![self.id(&c.name)];
                    cv.extend(self.docs(&c.docs));
                    for f in &c.fields {
                        match &f.label {
                            Some(l) => cv.push(list(
                                &format!("{}:", l.text(self.src)),
                                vec![self.ty(&f.ty)],
                            )),
                            None => cv.push(self.ty(&f.ty)),
                        }
                    }
                    if let Some(d) = &c.discriminant {
                        cv.push(atom("="));
                        cv.push(self.expr(d));
                    }
                    v.push(list("case", cv));
                }
                v.extend(e.items.iter().map(|i| self.item(i)));
                list("enum", v)
            }
            ItemKind::Impl(im) => {
                let mut v = Vec::new();
                v.extend(self.generics(&im.generics));
                v.push(self.ty(&im.target));
                if let Some(t) = &im.for_type {
                    v.push(atom("for"));
                    v.push(self.ty(t));
                }
                v.extend(self.where_clause(&im.where_));
                v.extend(im.members.iter().map(|m| self.member(m)));
                list("impl", v)
            }
            ItemKind::TypeAlias { name, generics, ty } => {
                let mut v = vec![self.id(name)];
                v.extend(self.generics(generics));
                v.push(self.ty(ty));
                list("type", v)
            }
            ItemKind::Test { name, body } => {
                let mut v = Vec::new();
                if let Some(n) = name {
                    v.push(self.expr(n));
                }
                v.push(self.block(body));
                list("test", v)
            }
        };
        if pre.is_empty() {
            body
        } else {
            let Sexp::List(mut items) = body else {
                return body;
            };
            // Put attributes and modifiers right after the head.
            let rest = items.split_off(1);
            items.extend(pre);
            items.extend(rest);
            Sexp::List(items)
        }
    }
}

/// Prints a whole file.
pub fn print_file(file: &File, src: &str) -> String {
    Printer { src }.file(file).to_pretty(100)
}

/// Prints one expression on a single line (for tests).
pub fn print_expr_flat(e: &Expr, src: &str) -> String {
    Printer { src }.expr(e).to_flat()
}
