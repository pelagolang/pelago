//! Walking the syntax tree.
//!
//! Implement [`Visitor`] and override the `visit_*` methods you care about. Each default calls the
//! matching `walk_*` function, which visits the children; call it yourself from an override to keep
//! going down. The syntax checks and, later, name resolution are built on this.

use crate::ast::*;

pub trait Visitor: Sized {
    fn visit_file(&mut self, f: &File) {
        walk_file(self, f);
    }
    fn visit_stmt(&mut self, s: &Stmt) {
        walk_stmt(self, s);
    }
    fn visit_item(&mut self, i: &Item) {
        walk_item(self, i);
    }
    fn visit_block(&mut self, b: &Block) {
        walk_block(self, b);
    }
    fn visit_decl(&mut self, d: &Decl) {
        walk_decl(self, d);
    }
    fn visit_fn(&mut self, f: &FnDecl) {
        walk_fn(self, f);
    }
    fn visit_expr(&mut self, e: &Expr) {
        walk_expr(self, e);
    }
    fn visit_pattern(&mut self, p: &Pattern) {
        walk_pattern(self, p);
    }
    fn visit_type(&mut self, t: &Type) {
        walk_type(self, t);
    }
}

pub fn walk_file<V: Visitor>(v: &mut V, f: &File) {
    for s in &f.stmts {
        v.visit_stmt(s);
    }
}

pub fn walk_stmt<V: Visitor>(v: &mut V, s: &Stmt) {
    match &s.kind {
        StmtKind::Decl(d) => v.visit_decl(d),
        StmtKind::Expr(e) => v.visit_expr(e),
        StmtKind::Item(i) => v.visit_item(i),
    }
}

pub fn walk_block<V: Visitor>(v: &mut V, b: &Block) {
    for s in &b.stmts {
        v.visit_stmt(s);
    }
    if let Some(t) = &b.tail {
        v.visit_expr(t);
    }
}

pub fn walk_decl<V: Visitor>(v: &mut V, d: &Decl) {
    v.visit_pattern(&d.pattern);
    if let Some(t) = &d.ty {
        v.visit_type(t);
    }
    if let Some(e) = &d.init {
        v.visit_expr(e);
    }
}

fn walk_generics<V: Visitor>(v: &mut V, g: &Generics) {
    for p in &g.params {
        for b in &p.bounds {
            v.visit_type(b);
        }
    }
}

fn walk_where<V: Visitor>(v: &mut V, w: &[WherePred]) {
    for p in w {
        v.visit_type(&p.ty);
        for b in &p.bounds {
            v.visit_type(b);
        }
    }
}

fn walk_member<V: Visitor>(v: &mut V, m: &Member) {
    match m {
        Member::Field(f) => {
            for a in &f.attrs {
                for e in &a.args {
                    v.visit_expr(e);
                }
            }
            v.visit_type(&f.ty);
            if let Some(d) = &f.default {
                v.visit_expr(d);
            }
        }
        Member::Item(i) => v.visit_item(i),
    }
}

pub fn walk_item<V: Visitor>(v: &mut V, i: &Item) {
    for a in &i.attrs {
        for e in &a.args {
            v.visit_expr(e);
        }
    }
    match &i.kind {
        ItemKind::Mod { body, .. } => {
            if let Some(b) = body {
                for s in b {
                    v.visit_stmt(s);
                }
            }
        }
        ItemKind::Import(_) => {}
        ItemKind::Decl(d) => v.visit_decl(d),
        ItemKind::Fn(f) => v.visit_fn(f),
        ItemKind::Struct(d) | ItemKind::Obj(d) | ItemKind::Interface(d) => {
            walk_generics(v, &d.generics);
            for b in &d.bases {
                v.visit_type(b);
            }
            walk_where(v, &d.where_);
            for m in &d.members {
                walk_member(v, m);
            }
        }
        ItemKind::Enum(e) => {
            walk_generics(v, &e.generics);
            for b in &e.bases {
                v.visit_type(b);
            }
            walk_where(v, &e.where_);
            for c in &e.variants {
                for f in &c.fields {
                    v.visit_type(&f.ty);
                }
                if let Some(d) = &c.discriminant {
                    v.visit_expr(d);
                }
            }
            for it in &e.items {
                v.visit_item(it);
            }
        }
        ItemKind::Impl(im) => {
            walk_generics(v, &im.generics);
            v.visit_type(&im.target);
            if let Some(t) = &im.for_type {
                v.visit_type(t);
            }
            walk_where(v, &im.where_);
            for m in &im.members {
                walk_member(v, m);
            }
        }
        ItemKind::TypeAlias { generics, ty, .. } => {
            walk_generics(v, generics);
            v.visit_type(ty);
        }
        ItemKind::Test { name, body } => {
            if let Some(n) = name {
                v.visit_expr(n);
            }
            v.visit_block(body);
        }
    }
}

pub fn walk_fn<V: Visitor>(v: &mut V, f: &FnDecl) {
    walk_generics(v, &f.generics);
    for p in &f.params {
        if let Some(t) = &p.ty {
            v.visit_type(t);
        }
        if let Some(d) = &p.default {
            v.visit_expr(d);
        }
    }
    if let Some(r) = &f.ret {
        v.visit_type(r);
    }
    walk_where(v, &f.where_);
    match &f.body {
        FnBody::Block(b) => v.visit_block(b),
        FnBody::Expr(e) => v.visit_expr(e),
        FnBody::None => {}
    }
}

pub fn walk_type<V: Visitor>(v: &mut V, t: &Type) {
    match &t.kind {
        TypeKind::Path { args, .. } => {
            for a in args {
                walk_generic_arg(v, a);
            }
        }
        TypeKind::SelfType | TypeKind::Error => {}
        TypeKind::List(e) | TypeKind::Option(e) | TypeKind::Any(e) => v.visit_type(e),
        TypeKind::Array(e, n) => {
            v.visit_type(e);
            v.visit_expr(n);
        }
        TypeKind::Tuple(ts) => {
            for t in ts {
                v.visit_type(t);
            }
        }
        TypeKind::FixedTuple(ts, n) => {
            for t in ts {
                v.visit_type(t);
            }
            v.visit_expr(n);
        }
        TypeKind::Result(a, b) => {
            v.visit_type(a);
            v.visit_type(b);
        }
        TypeKind::Fn { params, ret } => {
            for p in params {
                v.visit_type(&p.ty);
            }
            if let Some(r) = ret {
                v.visit_type(r);
            }
        }
    }
}

fn walk_generic_arg<V: Visitor>(v: &mut V, a: &GenericArg) {
    match a {
        GenericArg::Type(t) => v.visit_type(t),
        GenericArg::Const(e) => v.visit_expr(e),
    }
}

pub fn walk_pattern<V: Visitor>(v: &mut V, p: &Pattern) {
    match &p.kind {
        PatternKind::Wildcard
        | PatternKind::Binding { .. }
        | PatternKind::None
        | PatternKind::TypeTest(_)
        | PatternKind::Error => {}
        PatternKind::As { pattern, .. } => v.visit_pattern(pattern),
        PatternKind::Literal(e) => v.visit_expr(e),
        PatternKind::Range { start, end, .. } => {
            if let Some(e) = start {
                v.visit_expr(e);
            }
            if let Some(e) = end {
                v.visit_expr(e);
            }
        }
        PatternKind::Tuple(ps) => {
            for p in ps {
                v.visit_pattern(p);
            }
        }
        PatternKind::List(items) => {
            for i in items {
                if let ListPatItem::Pattern(p) = i {
                    v.visit_pattern(p);
                }
            }
        }
        PatternKind::Variant { args, .. } => {
            if let Some(args) = args {
                for a in args {
                    v.visit_pattern(&a.pattern);
                }
            }
        }
        PatternKind::Struct { fields, .. } => {
            for f in fields {
                if let Some(p) = &f.pattern {
                    v.visit_pattern(p);
                }
            }
        }
    }
}

fn walk_if<V: Visitor>(v: &mut V, i: &IfExpr) {
    v.visit_expr(&i.cond);
    v.visit_block(&i.then);
    if let Some(e) = &i.else_ {
        v.visit_expr(e);
    }
}

pub fn walk_expr<V: Visitor>(v: &mut V, e: &Expr) {
    match &e.kind {
        ExprKind::Int
        | ExprKind::Float
        | ExprKind::Char
        | ExprKind::Bool(_)
        | ExprKind::None
        | ExprKind::Name(_)
        | ExprKind::SelfValue
        | ExprKind::SelfType
        | ExprKind::Underscore
        | ExprKind::DotName(_)
        | ExprKind::Continue { .. }
        | ExprKind::Error => {}
        ExprKind::Str(parts) => {
            for p in parts {
                if let StrPart::Interp(e) = p {
                    v.visit_expr(e);
                }
            }
        }
        ExprKind::Paren(e)
        | ExprKind::Copy(e)
        | ExprKind::Try(e)
        | ExprKind::Become(e)
        | ExprKind::Defer(e)
        | ExprKind::Meta(e) => v.visit_expr(e),
        ExprKind::Unary { operand, .. } | ExprKind::IncDec { operand, .. } => v.visit_expr(operand),
        ExprKind::Tuple(es) => {
            for e in es {
                v.visit_expr(e);
            }
        }
        ExprKind::List(items) | ExprKind::Set(items) | ExprKind::Soa(items) => {
            for i in items {
                v.visit_expr(&i.value);
            }
        }
        ExprKind::Repeat { value, count } => {
            v.visit_expr(value);
            v.visit_expr(count);
        }
        ExprKind::Map(items) => {
            for i in items {
                match i {
                    MapItem::Pair(k, val) => {
                        v.visit_expr(k);
                        v.visit_expr(val);
                    }
                    MapItem::Spread(e) => v.visit_expr(e),
                }
            }
        }
        ExprKind::Builder(b) => {
            for c in &b.clauses {
                match c {
                    Clause::Of { pats, source } => {
                        for p in pats {
                            v.visit_pattern(p);
                        }
                        v.visit_expr(source);
                    }
                    Clause::If(e) => v.visit_expr(e),
                }
            }
            if let Some(r) = &b.result {
                v.visit_expr(r);
            }
            if let Some(r) = &b.value {
                v.visit_expr(r);
            }
        }
        ExprKind::StructLit { ty, fields } => {
            v.visit_type(ty);
            for f in fields {
                match f {
                    StructInit::Field(f) => {
                        if let Some(val) = &f.value {
                            v.visit_expr(val);
                        }
                    }
                    StructInit::Spread(e) => v.visit_expr(e),
                }
            }
        }
        ExprKind::Field { base, .. } | ExprKind::TupleIndex { base, .. } => v.visit_expr(base),
        ExprKind::Index { base, index } => {
            v.visit_expr(base);
            v.visit_expr(index);
        }
        ExprKind::Call { callee, args, .. } => {
            v.visit_expr(callee);
            for a in args {
                v.visit_expr(&a.value);
            }
        }
        ExprKind::TypeApply { base, args } => {
            v.visit_expr(base);
            for a in args {
                walk_generic_arg(v, a);
            }
        }
        ExprKind::Binary { lhs, rhs, .. }
        | ExprKind::In { lhs, rhs }
        | ExprKind::Assign { lhs, rhs, .. } => {
            v.visit_expr(lhs);
            v.visit_expr(rhs);
        }
        ExprKind::Range {
            start, end, step, ..
        } => {
            for e in [start, end, step].into_iter().flatten() {
                v.visit_expr(e);
            }
        }
        ExprKind::Ternary { cond, then, else_ } => {
            v.visit_expr(cond);
            v.visit_expr(then);
            v.visit_expr(else_);
        }
        ExprKind::Is { value, pattern } => {
            v.visit_expr(value);
            v.visit_pattern(pattern);
        }
        ExprKind::Convert { value, ty, .. } => {
            v.visit_expr(value);
            v.visit_type(ty);
        }
        ExprKind::Closure(c) => {
            for p in &c.params {
                v.visit_pattern(&p.pattern);
                if let Some(t) = &p.ty {
                    v.visit_type(t);
                }
            }
            if let Some(r) = &c.ret {
                v.visit_type(r);
            }
            v.visit_expr(&c.body);
        }
        ExprKind::If(i) | ExprKind::Unless(i) | ExprKind::MetaIf(i) => walk_if(v, i),
        ExprKind::Switch(s) => {
            v.visit_expr(&s.scrutinee);
            for arm in &s.arms {
                for p in &arm.patterns {
                    v.visit_pattern(p);
                }
                if let Some(g) = &arm.guard {
                    v.visit_expr(g);
                }
                v.visit_block(&arm.body);
            }
        }
        ExprKind::Select(arms) => {
            for arm in arms {
                if let Some(op) = &arm.op {
                    v.visit_expr(op);
                }
                v.visit_block(&arm.body);
            }
        }
        ExprKind::Scope { body, .. }
        | ExprKind::Block(body)
        | ExprKind::Unsafe(body)
        | ExprKind::Loop(body) => v.visit_block(body),
        ExprKind::While { cond, body } => {
            v.visit_expr(cond);
            v.visit_block(body);
        }
        ExprKind::DoWhile { body, cond } => {
            v.visit_block(body);
            v.visit_expr(cond);
        }
        ExprKind::For(f) => {
            match &f.head {
                ForHead::Of { pats, iter } => {
                    for p in pats {
                        v.visit_pattern(p);
                    }
                    v.visit_expr(iter);
                }
                ForHead::In { pat, iter } => {
                    v.visit_pattern(pat);
                    v.visit_expr(iter);
                }
                ForHead::C { init, cond, step } => {
                    if let Some(s) = init {
                        v.visit_stmt(s);
                    }
                    if let Some(c) = cond {
                        v.visit_expr(c);
                    }
                    if let Some(s) = step {
                        v.visit_expr(s);
                    }
                }
            }
            v.visit_block(&f.body);
        }
        ExprKind::Guard { cond, else_ } => {
            v.visit_expr(cond);
            v.visit_block(else_);
        }
        ExprKind::Return(val) | ExprKind::Yield(val) => {
            if let Some(e) = val {
                v.visit_expr(e);
            }
        }
        ExprKind::Break { value, .. } => {
            if let Some(e) = value {
                v.visit_expr(e);
            }
        }
        ExprKind::Labeled { body, .. } => v.visit_expr(body),
    }
}
