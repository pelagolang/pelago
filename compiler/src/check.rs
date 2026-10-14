//! Syntax checks: rules that need the whole tree but no types and no name resolution.
//!
//! The parser accepts a little more than the language allows (it keeps going so one mistake does
//! not hide the rest). This pass reports the rest of the grammar's rules:
//!
//! * `break` and `continue` only inside a loop; `become` only with a call;
//! * `self` only as the first parameter of a method; `init` and `deinit` only inside a type;
//! * variadic parameters last and alone; every parameter has a type; no repeated names;
//! * functions have a body (except in interfaces, `abstract` and `ext`), and `ext` ones do not;
//! * declarations (`const` / `var`) are given a value, carry no access modes, and use patterns
//!   that cannot fail by syntax alone;
//! * assignment targets are places (`x`, `a.b`, `xs[i]`, `t.0`);
//! * modifiers fit the declaration (`open` only on `obj` and its methods, `linear` only on types, ...);
//! * a pattern binds each name once, and the alternatives of a `case` bind the same names;
//! * `switch` has at most one `default`, and it comes last; map builders give `key : value`;
//!   struct literals and calls do not repeat a field or argument name.

use crate::ast::*;
use crate::parser::ParseError;
use crate::span::Span;
use crate::visit::{self, Visitor};

/// Checks a parsed file.
pub fn check_file(file: &File, src: &str) -> Vec<ParseError> {
    let mut c = Checker {
        src,
        errors: Vec::new(),
        owner: None,
        loops: 0,
        in_ext: false,
    };
    c.visit_file(file);
    c.errors
}

/// The kind of type body a declaration sits directly inside.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Struct,
    Obj,
    Enum,
    Interface,
    Impl,
}

struct Checker<'a> {
    src: &'a str,
    errors: Vec<ParseError>,
    owner: Option<Owner>,
    /// How many loops enclose the current position (inside the current function or closure).
    loops: u32,
    /// The declaration being visited is `ext`.
    in_ext: bool,
}

/// A name a pattern binds.
struct Bound {
    name: Ident,
    mode: Option<Mode>,
}

fn collect(p: &Pattern, out: &mut Vec<Bound>) {
    match &p.kind {
        PatternKind::Binding { mode, name } => out.push(Bound {
            name: *name,
            mode: *mode,
        }),
        PatternKind::As { name, pattern } => {
            out.push(Bound {
                name: *name,
                mode: None,
            });
            collect(pattern, out);
        }
        PatternKind::Tuple(ps) => ps.iter().for_each(|p| collect(p, out)),
        PatternKind::List(items) => {
            for i in items {
                match i {
                    ListPatItem::Pattern(p) => collect(p, out),
                    ListPatItem::Rest {
                        mode,
                        name: Some(name),
                        ..
                    } => out.push(Bound {
                        name: *name,
                        mode: *mode,
                    }),
                    ListPatItem::Rest { .. } => {}
                }
            }
        }
        PatternKind::Variant { args, .. } => {
            for a in args.iter().flatten() {
                collect(&a.pattern, out);
            }
        }
        PatternKind::Struct { fields, .. } => {
            for f in fields {
                match &f.pattern {
                    Some(p) => collect(p, out),
                    None => out.push(Bound {
                        name: f.name,
                        mode: f.mode,
                    }),
                }
            }
        }
        PatternKind::Wildcard
        | PatternKind::Literal(_)
        | PatternKind::Range { .. }
        | PatternKind::None
        | PatternKind::TypeTest(_)
        | PatternKind::Error => {}
    }
}

/// Can this pattern fail to match, judging by its shape alone?
fn refutable(p: &Pattern) -> bool {
    match &p.kind {
        PatternKind::Literal(_)
        | PatternKind::Range { .. }
        | PatternKind::None
        | PatternKind::TypeTest(_) => true,
        PatternKind::As { pattern, .. } => refutable(pattern),
        PatternKind::Tuple(ps) => ps.iter().any(refutable),
        PatternKind::Struct { fields, .. } => fields
            .iter()
            .any(|f| f.pattern.as_ref().is_some_and(refutable)),
        _ => false,
    }
}

fn has_mode(p: &Pattern) -> Option<Span> {
    let mut found = None;
    fn go(p: &Pattern, found: &mut Option<Span>) {
        match &p.kind {
            PatternKind::Binding { mode: Some(_), .. } => *found = found.or(Some(p.span)),
            PatternKind::As { pattern, .. } => go(pattern, found),
            PatternKind::Tuple(ps) => ps.iter().for_each(|p| go(p, found)),
            PatternKind::List(items) => {
                for i in items {
                    match i {
                        ListPatItem::Pattern(p) => go(p, found),
                        ListPatItem::Rest {
                            mode: Some(_),
                            span,
                            ..
                        } => *found = found.or(Some(*span)),
                        ListPatItem::Rest { .. } => {}
                    }
                }
            }
            PatternKind::Variant { args, .. } => {
                for a in args.iter().flatten() {
                    go(&a.pattern, found);
                }
            }
            PatternKind::Struct { fields, .. } => {
                for f in fields {
                    if f.mode.is_some() {
                        *found = found.or(Some(f.name.span));
                    }
                    if let Some(p) = &f.pattern {
                        go(p, found);
                    }
                }
            }
            _ => {}
        }
    }
    go(p, &mut found);
    found
}

fn is_place(e: &Expr, allow_underscore: bool) -> bool {
    match &e.kind {
        ExprKind::Name(_)
        | ExprKind::SelfValue
        | ExprKind::TupleIndex { .. }
        | ExprKind::Index { .. } => true,
        ExprKind::Field { optional, .. } => !optional,
        ExprKind::Paren(inner) => is_place(inner, false),
        ExprKind::Underscore => allow_underscore,
        _ => false,
    }
}

impl<'a> Checker<'a> {
    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.errors.push(ParseError {
            span,
            message: message.into(),
        });
    }

    fn name(&self, id: &Ident) -> &'a str {
        id.text(self.src)
    }

    /// One scope of bindings: no name twice.
    fn check_scope(&mut self, pats: &[&Pattern]) {
        let mut seen: Vec<&str> = Vec::new();
        let mut all = Vec::new();
        for p in pats {
            collect(p, &mut all);
        }
        for b in &all {
            let n = self.name(&b.name);
            if n.is_empty() {
                continue;
            }
            if seen.contains(&n) {
                let msg = format!("`{n}` is bound twice in this pattern");
                self.error(b.name.span, msg);
            } else {
                seen.push(n);
            }
        }
    }

    /// The alternatives of one `case` must bind the same names in the same modes.
    fn check_alternatives(&mut self, pats: &[Pattern]) {
        if pats.len() < 2 {
            return;
        }
        let key = |c: &Checker, p: &Pattern| {
            let mut b = Vec::new();
            collect(p, &mut b);
            let mut v: Vec<(String, Option<Mode>)> = b
                .iter()
                .map(|b| (c.name(&b.name).to_string(), b.mode))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v
        };
        let first = key(self, &pats[0]);
        for p in &pats[1..] {
            let k = key(self, p);
            if k != first {
                let missing = first
                    .iter()
                    .find(|f| !k.contains(f))
                    .or_else(|| k.iter().find(|f| !first.contains(f)));
                let what = missing.map_or_else(String::new, |m| format!(" (`{}`)", m.0));
                self.error(
                    p.span,
                    format!("every alternative of a `case` must bind the same names in the same modes{what}"),
                );
                break;
            }
        }
    }

    fn check_decl_pattern(&mut self, d: &Decl) {
        if let Some(span) = has_mode(&d.pattern) {
            self.error(
                span,
                "access modes belong to parameters, loops and `switch` patterns, not to `const` or `var`",
            );
        }
        if refutable(&d.pattern) {
            self.error(
                d.pattern.span,
                "this pattern can fail to match; use `if x is ...` or `guard x is ... else { }`",
            );
        }
        self.check_scope(&[&d.pattern]);
    }

    fn check_modifiers(&mut self, i: &Item, owner: Option<Owner>) {
        let m = &i.mods;
        let kind = &i.kind;
        let is_obj = matches!(kind, ItemKind::Obj(_));
        let is_type = matches!(
            kind,
            ItemKind::Struct(_) | ItemKind::Obj(_) | ItemKind::Enum(_)
        );
        let is_fn = matches!(kind, ItemKind::Fn(_));
        let is_method_of_obj = is_fn && owner == Some(Owner::Obj);
        let is_decl = matches!(kind, ItemKind::Decl(_));
        let span = i.span;
        let bad = |on: bool, word: &str, ok: bool, what: &str, this: &mut Self| {
            if on && !ok {
                this.error(span, format!("`{word}` applies to {what}"));
            }
        };
        bad(
            m.is_open,
            "open",
            is_obj || is_method_of_obj,
            "an `obj` or a method of an `obj`",
            self,
        );
        bad(
            m.is_abstract,
            "abstract",
            is_obj || is_method_of_obj,
            "an `obj` or a method of an `obj`",
            self,
        );
        bad(
            m.is_final,
            "final",
            is_obj || is_method_of_obj,
            "an `obj` or a method of an `obj`",
            self,
        );
        bad(
            m.is_override,
            "override",
            is_method_of_obj,
            "a method of an `obj`",
            self,
        );
        bad(
            m.is_linear,
            "linear",
            is_type,
            "a `struct`, `enum` or `obj`",
            self,
        );
        bad(
            m.is_static,
            "static",
            is_fn || is_decl,
            "a function or a variable",
            self,
        );
        bad(
            m.is_ext,
            "ext",
            is_fn || is_decl,
            "a function or a variable",
            self,
        );
        bad(m.is_meta, "meta", is_fn, "a function", self);
        bad(m.is_unsafe, "unsafe", is_fn, "a function", self);
        if m.vis != Vis::Default && matches!(kind, ItemKind::Impl(_) | ItemKind::Test { .. }) {
            self.error(span, "`pub` and `priv` do not apply to `impl` or `test`");
        }
    }

    fn check_fn(&mut self, i: &Item, f: &FnDecl, owner: Option<Owner>) {
        let in_type = owner.is_some();
        // Parameters.
        let mut names: Vec<&str> = Vec::new();
        let last = f.params.len().saturating_sub(1);
        let mut variadics = 0;
        for (idx, p) in f.params.iter().enumerate() {
            if p.is_self {
                if idx != 0 || !in_type || f.kind == FnKind::Init {
                    let why = if f.kind == FnKind::Init {
                        "`init` has no `self` parameter"
                    } else {
                        "`self` is only the first parameter of a method"
                    };
                    self.error(p.span, why);
                }
                if p.variadic || p.default.is_some() {
                    self.error(p.span, "`self` cannot be variadic or have a default");
                }
            } else {
                if p.ty.is_none() {
                    self.error(p.span, "a parameter needs a type, like `x: int`");
                }
                let n = self.name(&p.name);
                if !n.is_empty() {
                    if names.contains(&n) {
                        let msg = format!("the parameter `{n}` appears twice");
                        self.error(p.name.span, msg);
                    } else {
                        names.push(n);
                    }
                }
            }
            if p.variadic {
                variadics += 1;
                if idx != last {
                    self.error(p.span, "a variadic parameter must come last");
                } else if variadics > 1 {
                    self.error(p.span, "only one variadic parameter is allowed");
                }
            }
        }
        // Special functions.
        match f.kind {
            FnKind::Init => {
                if !in_type {
                    self.error(f.name.span, "`init` belongs inside a type");
                }
            }
            FnKind::Deinit => {
                if !matches!(owner, Some(Owner::Struct | Owner::Obj | Owner::Enum)) {
                    self.error(
                        f.name.span,
                        "`deinit` belongs inside a `struct`, `obj` or `enum`",
                    );
                }
                if !f.params.is_empty() || f.ret.is_some() {
                    self.error(
                        f.name.span,
                        "`deinit` takes nothing and gives nothing back: it cannot fail",
                    );
                }
            }
            FnKind::Fn => {}
        }
        // Body.
        let may_lack_body = owner == Some(Owner::Interface) || i.mods.is_abstract || i.mods.is_ext;
        match (&f.body, i.mods.is_ext) {
            (FnBody::None, false) if !may_lack_body => {
                self.error(
                    f.name.span,
                    "this function needs a body: `{ ... }` or `= expr`",
                );
            }
            (FnBody::Block(_) | FnBody::Expr(_), true) => {
                self.error(
                    f.name.span,
                    "an `ext` function is defined outside the language and has no body",
                );
            }
            _ => {}
        }
    }

    fn check_import(&mut self, imp: &Import) {
        if let ImportKind::Names { names, glob } = &imp.kind {
            if names.is_empty() && !*glob {
                self.error(
                    imp.path.span,
                    "this import brings in no names; write `...` for everything",
                );
            }
            // Each name is brought in once, under its final name.
            let mut seen: Vec<&str> = Vec::new();
            for n in names {
                let local = self.name(n.alias.as_ref().unwrap_or(&n.name));
                if seen.contains(&local) {
                    let msg = format!("`{local}` is imported twice");
                    self.error(n.name.span, msg);
                } else {
                    seen.push(local);
                }
            }
        }
    }

    fn check_members(&mut self, members: &[Member], what: &str) {
        for m in members {
            if let Member::Field(f) = m {
                self.error(f.span, format!("{what} cannot have fields"));
            }
        }
    }
}

impl Visitor for Checker<'_> {
    fn visit_item(&mut self, i: &Item) {
        let outer = self.owner;
        self.check_modifiers(i, outer);
        let child_owner = match &i.kind {
            ItemKind::Struct(_) => Some(Owner::Struct),
            ItemKind::Obj(_) => Some(Owner::Obj),
            ItemKind::Enum(_) => Some(Owner::Enum),
            ItemKind::Interface(_) => Some(Owner::Interface),
            ItemKind::Impl(_) => Some(Owner::Impl),
            _ => None,
        };
        match &i.kind {
            ItemKind::Import(imp) => self.check_import(imp),
            ItemKind::Fn(f) => {
                self.check_fn(i, f, outer);
            }
            ItemKind::Interface(d) => self.check_members(&d.members, "an `interface`"),
            ItemKind::Impl(d) => self.check_members(&d.members, "an `impl`"),
            _ => {}
        }
        let saved_ext = self.in_ext;
        self.in_ext = i.mods.is_ext;
        self.owner = child_owner;
        let saved_loops = self.loops;
        self.loops = 0;
        visit::walk_item(self, i);
        self.loops = saved_loops;
        self.owner = outer;
        self.in_ext = saved_ext;
    }

    fn visit_decl(&mut self, d: &Decl) {
        self.check_decl_pattern(d);
        if d.init.is_none() && !self.in_ext {
            self.error(
                d.pattern.span,
                "a variable must be given a value when it is created",
            );
        }
        if d.init.is_some() && self.in_ext {
            self.error(
                d.pattern.span,
                "an `ext` variable is defined outside the language and has no value",
            );
        }
        visit::walk_decl(self, d);
    }

    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Break { label: None, .. } if self.loops == 0 => {
                self.error(e.span, "`break` must be inside a loop");
            }
            ExprKind::Continue { .. } if self.loops == 0 => {
                self.error(e.span, "`continue` must be inside a loop");
            }
            ExprKind::Become(call) if !matches!(call.kind, ExprKind::Call { .. }) => {
                self.error(call.span, "`become` takes a function call: `become f(x)`");
            }
            ExprKind::Assign { op, lhs, .. } => {
                if !is_place(lhs, *op == crate::token::Punct::Eq) {
                    self.error(lhs.span, "this cannot be assigned to; use a name, a field, an index or a tuple position");
                }
            }
            ExprKind::IncDec { operand, .. } if !is_place(operand, false) => {
                self.error(
                    operand.span,
                    "`++` and `--` need a variable, a field or an index",
                );
            }
            ExprKind::Is { pattern, .. } => self.check_scope(&[pattern]),
            ExprKind::Switch(s) => {
                let mut defaults = 0;
                for (i, arm) in s.arms.iter().enumerate() {
                    if arm.is_default {
                        defaults += 1;
                        if defaults > 1 {
                            self.error(arm.span, "a `switch` has only one `default`");
                        } else if i + 1 != s.arms.len() {
                            self.error(arm.span, "`default` must be the last arm");
                        }
                    }
                    for p in &arm.patterns {
                        self.check_scope(&[p]);
                    }
                    self.check_alternatives(&arm.patterns);
                }
            }
            ExprKind::Select(arms) => {
                let defaults = arms.iter().filter(|a| a.op.is_none()).count();
                if defaults > 1 {
                    self.error(e.span, "a `select` has only one `default`");
                }
            }
            ExprKind::Builder(b) => {
                if b.kind == BuilderKind::Map && (b.result.is_none() || b.value.is_none()) {
                    self.error(e.span, "a map builder gives `key : value` after `~>`");
                }
                for c in &b.clauses {
                    if let Clause::Of { pats, .. } = c {
                        let refs: Vec<&Pattern> = pats.iter().collect();
                        self.check_scope(&refs);
                    }
                }
            }
            ExprKind::For(f) => match &f.head {
                ForHead::Of { pats, .. } => {
                    let refs: Vec<&Pattern> = pats.iter().collect();
                    self.check_scope(&refs);
                }
                ForHead::In { pat, .. } => self.check_scope(&[pat]),
                ForHead::C { .. } => {}
            },
            ExprKind::Closure(c) => {
                let refs: Vec<&Pattern> = c.params.iter().map(|p| &p.pattern).collect();
                self.check_scope(&refs);
            }
            ExprKind::StructLit { fields, .. } => {
                let mut seen: Vec<&str> = Vec::new();
                for f in fields {
                    if let StructInit::Field(f) = f {
                        let n = self.name(&f.name);
                        if seen.contains(&n) {
                            let msg = format!("the field `{n}` is given twice");
                            self.error(f.name.span, msg);
                        } else {
                            seen.push(n);
                        }
                    }
                }
            }
            ExprKind::Call { args, .. } => {
                let mut seen: Vec<&str> = Vec::new();
                for a in args {
                    if let Some(l) = &a.label {
                        let n = self.name(l);
                        if seen.contains(&n) {
                            let msg = format!("the argument `{n}` is given twice");
                            self.error(l.span, msg);
                        } else {
                            seen.push(n);
                        }
                    }
                }
            }
            _ => {}
        }
        // Loops and closures change what `break` and `continue` may do.
        let saved = self.loops;
        match &e.kind {
            ExprKind::Loop(_)
            | ExprKind::While { .. }
            | ExprKind::DoWhile { .. }
            | ExprKind::For(_) => {
                self.loops += 1;
            }
            ExprKind::Closure(_) => self.loops = 0,
            _ => {}
        }
        visit::walk_expr(self, e);
        self.loops = saved;
    }
}
