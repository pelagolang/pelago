//! Stage 5 of the checker: value semantics. What the types cannot say: who may change what, and
//! when a value is gone.
//!
//! The rules (see `docs/design/checker.md`):
//!
//! - **Changing needs permission.** A place can be changed (assigned to, passed as `inout`, used
//!   as the receiver of `c.bump!()`) only when its root variable allows it: a `var`, an `inout` or
//!   `sink` parameter, `inout self`. A `const`, a read-only parameter and a name bound by a pattern
//!   are fixed all the way down (`const` depth): `const xs = [1]; xs[0] = 2` is an error.
//! - **Owning values move.** A value that owns something (`string`, a list, a map, an `obj`, a
//!   struct holding one) is moved, not copied, when it is assigned, passed to a `sink` parameter or
//!   an enum case, returned, or put in a literal. The old variable is then unusable until it gets a
//!   new value. Taking part of a variable (`a.items`) needs `copy`, and so does taking a read-only
//!   parameter. Plain values (numbers, `bool`, plain structs) copy freely.
//! - **Exclusive access.** A call may not change a place and also pass an overlapping place to
//!   another argument: `swap(inout a[0], inout a[1])`, `xs.extend!(xs)`. Temporaries computed by
//!   other arguments are evaluated first and do not count.
//! - **Closures** change outer variables only when they capture them with `inout`.
//!
//! Moves are tracked through branches and loops: after an `if`, a variable is moved when either
//! side moved it; a loop body is walked twice so a move in one pass shows up as a use after a move
//! in the next. Code after `return`, `break` and `continue` is not checked.

use crate::ast::*;
use crate::package::{Diagnostic, ModuleId, Program};
use crate::resolve::{DefId, DefKind, Resolved};
use crate::span::Span;
use crate::typecheck::Typed;
use crate::types::TyKind;
use crate::visit::{self, Visitor};
use std::collections::{HashMap, HashSet};

#[cfg(test)]
mod tests;

/// Checks moves, exclusive access and `const` depth in every module.
pub fn check(program: &Program, res: &Resolved, typed: &Typed) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for m in &program.modules {
        let mut w = Walker::new(program, res, typed, m.id);
        w.top(&m.file.stmts);
        diags.extend(w.diags);
    }
    diags
}

/// How a name was introduced, which decides what may be done with it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Bind {
    Var,
    Const,
    /// A read-only parameter (the default), or `self` of a method that does not change it.
    Let,
    Inout,
    Sink,
    /// A parameter of `init`: the new value takes what it is given.
    Owned,
    /// A name bound by a pattern: a view into another value.
    View,
    /// `inout x` in a pattern.
    ViewInout,
}

impl Bind {
    fn can_change(self) -> bool {
        matches!(
            self,
            Bind::Var | Bind::Inout | Bind::Sink | Bind::Owned | Bind::ViewInout
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    /// The value is looked at.
    Read,
    /// The value flows into an owner: it moves when it owns something.
    Consume,
}

#[derive(Clone, Default)]
struct State {
    /// Variables that may have been moved, and where.
    moved: HashMap<DefId, Span>,
    /// `linear` variables that may still hold their value (not consumed on some path).
    held: HashSet<DefId>,
    /// After `return`, `break`, `continue` or a call that never returns.
    dead: bool,
}

fn merge(a: State, b: State) -> State {
    if a.dead {
        return b;
    }
    if b.dead {
        return a;
    }
    let mut out = a;
    for (d, s) in b.moved {
        out.moved.entry(d).or_insert(s);
    }
    out.held.extend(b.held);
    out
}

struct LoopFrame {
    /// How many scopes were open outside the loop.
    scope_depth: usize,
    label: Option<String>,
    breaks: Vec<State>,
    continues: Vec<State>,
}

struct ClosureFrame {
    /// How many scopes were open outside the closure.
    scope_base: usize,
    span: Span,
    caps: Vec<(DefId, Option<Mode>)>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Seg {
    Field(String),
    Index,
}

#[derive(Clone, Debug)]
struct PlaceKey {
    root: DefId,
    segs: Vec<Seg>,
}

fn overlap(a: &PlaceKey, b: &PlaceKey) -> bool {
    if a.root != b.root {
        return false;
    }
    for (x, y) in a.segs.iter().zip(b.segs.iter()) {
        if x != y {
            return false;
        }
    }
    true
}

struct Access {
    place: PlaceKey,
    changes: bool,
    span: Span,
}

struct Walker<'a> {
    program: &'a Program,
    res: &'a Resolved,
    typed: &'a Typed,
    file: ModuleId,
    diags: Vec<Diagnostic>,
    seen: HashSet<(u32, &'static str)>,
    binds: HashMap<DefId, Bind>,
    state: State,
    loops: Vec<LoopFrame>,
    closures: Vec<ClosureFrame>,
    /// The open `scope` handles, with the variables their tasks have captured to change or consume.
    task_scopes: Vec<(DefId, Vec<DefId>)>,
    pending_label: Option<String>,
    /// Variables introduced in each open block, to check `linear` values at its end.
    scopes: Vec<Vec<DefId>>,
    /// `defer`s seen in the current function.
    defers: u32,
}

impl<'a> Walker<'a> {
    fn new(program: &'a Program, res: &'a Resolved, typed: &'a Typed, file: ModuleId) -> Self {
        Walker {
            program,
            res,
            typed,
            file,
            diags: Vec::new(),
            seen: HashSet::new(),
            binds: HashMap::new(),
            state: State::default(),
            loops: Vec::new(),
            closures: Vec::new(),
            task_scopes: Vec::new(),
            pending_label: None,
            scopes: Vec::new(),
            defers: 0,
        }
    }

    // ---- Small helpers --------------------------------------------------------------------------

    fn report(&mut self, span: Span, key: &'static str, message: String) {
        if self.state.dead || !self.seen.insert((span.start, key)) {
            return;
        }
        self.diags.push(Diagnostic {
            module: self.file,
            span,
            message,
            warning: false,
        });
    }

    fn src(&self) -> &'a str {
        &self.program.module(self.file).src
    }

    fn text(&self, span: Span) -> &'a str {
        &self.src()[span.start as usize..span.end as usize]
    }

    fn line_of(&self, span: Span) -> usize {
        self.src()[..span.start as usize].matches('\n').count() + 1
    }

    fn def_at_use(&self, start: u32) -> Option<DefId> {
        self.res.use_at(self.file, start)
    }

    fn name_of(&self, d: DefId) -> &'a str {
        &self.res.def(d).name
    }

    fn ty_owning(&self, e: &Expr) -> bool {
        self.typed
            .tys
            .get(&(self.file, e.span.start, e.span.end))
            .is_some_and(|t| self.typed.is_owning(*t))
    }

    fn expr_never(&self, e: &Expr) -> bool {
        self.typed
            .tys
            .get(&(self.file, e.span.start, e.span.end))
            .is_some_and(|t| matches!(self.typed.types.kind_of(*t), TyKind::Never))
    }

    fn strip(e: &Expr) -> &Expr {
        match &e.kind {
            ExprKind::Paren(i) => Self::strip(i),
            _ => e,
        }
    }

    /// `b.get()` on a box: the base `b`. It is a place inside the box, like a field.
    fn box_get_base<'e>(&self, e: &'e Expr) -> Option<&'e Expr> {
        let ExprKind::Call { callee, args, .. } = &Self::strip(e).kind else {
            return None;
        };
        let ExprKind::Field {
            base,
            name,
            optional: false,
        } = &Self::strip(callee).kind
        else {
            return None;
        };
        if !args.is_empty() || self.text(name.span) != "get" {
            return None;
        }
        let t = self
            .typed
            .tys
            .get(&(self.file, base.span.start, base.span.end))?;
        matches!(self.typed.types.kind_of(*t), TyKind::Box(_)).then_some(&**base)
    }

    /// The variable at the root of a chain of fields and indexes, and the path from it.
    fn place_of(&self, e: &Expr) -> Option<PlaceKey> {
        match &Self::strip(e).kind {
            ExprKind::Name(id) => {
                let d = self.def_at_use(id.span.start)?;
                self.binds.contains_key(&d).then(|| PlaceKey {
                    root: d,
                    segs: Vec::new(),
                })
            }
            ExprKind::SelfValue => {
                let d = self.def_at_use(Self::strip(e).span.start)?;
                self.binds.contains_key(&d).then(|| PlaceKey {
                    root: d,
                    segs: Vec::new(),
                })
            }
            ExprKind::Field { base, name, .. } => {
                if self.def_at_use(name.span.start).is_some() {
                    return None; // a path through a module or a type
                }
                let mut p = self.place_of(base)?;
                p.segs.push(Seg::Field(self.text(name.span).to_string()));
                Some(p)
            }
            ExprKind::TupleIndex { base, index } => {
                let mut p = self.place_of(base)?;
                p.segs.push(Seg::Field(self.text(*index).to_string()));
                Some(p)
            }
            ExprKind::Index { base, .. } => {
                let mut p = self.place_of(base)?;
                p.segs.push(Seg::Index);
                Some(p)
            }
            ExprKind::Call { .. } => {
                let mut p = self.place_of(self.box_get_base(e)?)?;
                p.segs.push(Seg::Field("*".to_string()));
                Some(p)
            }
            _ => None,
        }
    }

    // ---- Units -------------------------------------------------------------------------------------------

    fn top(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    /// A function body, test or other piece of code that starts fresh.
    fn unit(&mut self, f: impl FnOnce(&mut Self)) {
        let saved_state = std::mem::take(&mut self.state);
        let saved_loops = std::mem::take(&mut self.loops);
        let saved_closures = std::mem::take(&mut self.closures);
        let saved_scopes = std::mem::take(&mut self.scopes);
        let saved_defers = std::mem::take(&mut self.defers);
        f(self);
        self.scopes = saved_scopes;
        self.defers = saved_defers;
        self.state = saved_state;
        self.loops = saved_loops;
        self.closures = saved_closures;
    }

    fn item(&mut self, it: &Item) {
        match &it.kind {
            ItemKind::Fn(f) => self.unit(|w| w.fn_decl(f)),
            ItemKind::Struct(d) | ItemKind::Obj(d) | ItemKind::Interface(d) => {
                for m in &d.members {
                    match m {
                        Member::Field(f) => {
                            if let Some(v) = &f.default {
                                self.unit(|w| w.expr(v, Ctx::Consume));
                            }
                        }
                        Member::Item(i) => self.item(i),
                    }
                }
            }
            ItemKind::Enum(e) => {
                for i in &e.items {
                    self.item(i);
                }
            }
            ItemKind::Impl(im) => {
                for m in &im.members {
                    if let Member::Item(i) = m {
                        self.item(i);
                    }
                }
            }
            ItemKind::Mod {
                body: Some(body), ..
            } => self.unit(|w| w.top(body)),
            ItemKind::Test { body, .. } => self.unit(|w| w.block(body, Ctx::Read)),
            ItemKind::Decl(d) => self.unit(|w| w.decl(d)),
            _ => {}
        }
    }

    fn fn_decl(&mut self, f: &FnDecl) {
        let is_init = f.kind != FnKind::Fn;
        self.scopes.push(Vec::new());
        if is_init {
            if let Some(&d) = self.res.decls.get(&(self.file, f.name.span.start)) {
                self.binds.insert(d, Bind::Inout);
            }
        }
        for p in &f.params {
            if let Some(&d) = self.res.decls.get(&(self.file, p.name.span.start)) {
                let b = match (p.mode, is_init) {
                    (Some(Mode::Inout), _) => Bind::Inout,
                    (Some(Mode::Sink), _) => Bind::Sink,
                    (_, true) => Bind::Owned,
                    _ => Bind::Let,
                };
                self.binds.insert(d, b);
                if matches!(b, Bind::Sink | Bind::Owned) {
                    self.track_linear(d);
                }
            }
            if let Some(v) = &p.default {
                self.expr(v, Ctx::Consume);
            }
        }
        match &f.body {
            FnBody::Block(b) => {
                self.block(b, Ctx::Consume);
                if let Some(t) = &b.tail {
                    self.check_escape(t);
                }
            }
            FnBody::Expr(e) => {
                self.expr(e, Ctx::Consume);
                self.check_escape(e);
            }
            FnBody::None => {}
        }
        let end = f.name.span;
        self.leave_scope(end, "the function ends");
    }

    // ---- `linear` values -----------------------------------------------------------------------------

    fn is_linear_def(&self, d: DefId) -> bool {
        self.typed
            .local_tys
            .get(&d)
            .is_some_and(|t| self.typed.is_linear(*t))
    }

    /// A new variable that owns a `linear` value must be consumed before it goes away.
    fn track_linear(&mut self, d: DefId) {
        if self.is_linear_def(d) {
            self.state.held.insert(d);
            if let Some(s) = self.scopes.last_mut() {
                s.push(d);
            }
        }
    }

    /// Reports the `linear` variables of the innermost scope that still hold their value.
    fn leave_scope(&mut self, at: Span, why: &str) {
        let Some(vars) = self.scopes.pop() else {
            return;
        };
        self.report_held(&vars, at, why);
    }

    fn report_held(&mut self, vars: &[DefId], at: Span, why: &str) {
        for d in vars {
            if self.state.held.contains(d) && !self.state.dead {
                let n = self.name_of(*d);
                self.report(
                    at,
                    "linear",
                    format!("`{n}` is a `linear` value and must be consumed before {why}; consume it with a `!!` call or pass it with `sink`"),
                );
            }
        }
    }

    /// An early exit (`return`, `?`, `break`, `continue`) leaves every scope above `depth`.
    fn exit_scopes(&mut self, depth: usize, at: Span, why: &str) {
        let base = self.closures.last().map_or(0, |c| c.scope_base);
        let from = depth.max(base);
        let vars: Vec<DefId> = self.scopes.iter().skip(from).flatten().copied().collect();
        self.report_held(&vars, at, why);
    }

    // ---- Views that outlive their source ----------------------------------------------------------------

    /// A `str` returned from a function must not point into a local that is about to go away.
    fn check_escape(&mut self, e: &Expr) {
        if !self.closures.is_empty() {
            return;
        }
        match &Self::strip(e).kind {
            ExprKind::Block(b) | ExprKind::Unsafe(b) => {
                if let Some(t) = &b.tail {
                    self.check_escape(t);
                }
            }
            ExprKind::If(i) | ExprKind::Unless(i) => {
                if let Some(t) = &i.then.tail {
                    self.check_escape(t);
                }
                if let Some(el) = &i.else_ {
                    self.check_escape(el);
                }
            }
            ExprKind::Ternary { then, else_, .. } => {
                self.check_escape(then);
                self.check_escape(else_);
            }
            ExprKind::Switch(s) => {
                for arm in &s.arms {
                    if let Some(t) = &arm.body.tail {
                        self.check_escape(t);
                    }
                }
            }
            _ => {
                let is_str = self
                    .typed
                    .tys
                    .get(&(self.file, e.span.start, e.span.end))
                    .is_some_and(|t| matches!(self.typed.types.kind_of(*t), TyKind::Str));
                if !is_str {
                    return;
                }
                let Some(root) = self.origin_root(e) else {
                    return;
                };
                let local = matches!(
                    self.binds.get(&root),
                    Some(Bind::Var | Bind::Const | Bind::Sink | Bind::Owned)
                );
                let owns = self
                    .typed
                    .local_tys
                    .get(&root)
                    .is_some_and(|t| self.typed.is_owning(*t));
                if local && owns {
                    let n = self.name_of(root);
                    self.report(
                        e.span,
                        "escape",
                        format!("this `str` points into `{n}`, which is gone when the function ends; return a `string` instead (`... to string`)"),
                    );
                }
            }
        }
    }

    /// The variable a view was taken from: `s` in `s`, `s[1..]`, `s.trim()`, `p.name`.
    fn origin_root(&self, e: &Expr) -> Option<DefId> {
        match &Self::strip(e).kind {
            ExprKind::Name(id) => self.def_at_use(id.span.start),
            ExprKind::SelfValue => self.def_at_use(Self::strip(e).span.start),
            ExprKind::Field { base, .. }
            | ExprKind::TupleIndex { base, .. }
            | ExprKind::Index { base, .. } => self.origin_root(base),
            ExprKind::Call { callee, .. } => match &Self::strip(callee).kind {
                ExprKind::Field { base, .. } => self.origin_root(base),
                _ => None,
            },
            _ => None,
        }
    }

    // ---- Statements and patterns ------------------------------------------------------------------------

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Decl(d) => self.decl(d),
            StmtKind::Expr(e) => {
                self.expr(e, Ctx::Read);
                let linear = self
                    .typed
                    .tys
                    .get(&(self.file, e.span.start, e.span.end))
                    .is_some_and(|t| self.typed.is_linear(*t));
                if linear && !matches!(Self::strip(e).kind, ExprKind::Name(_)) {
                    self.report(
                        e.span,
                        "linear",
                        "this produces a `linear` value that is thrown away; it must be consumed"
                            .to_string(),
                    );
                }
            }
            StmtKind::Item(i) => self.item(i),
        }
    }

    fn decl(&mut self, d: &Decl) {
        if let Some(init) = &d.init {
            self.expr(init, Ctx::Consume);
        }
        let kind = match d.kind {
            DeclKind::Var => Bind::Var,
            DeclKind::Const => Bind::Const,
        };
        self.bind_pattern(&d.pattern, kind);
    }

    /// Records how each name of a pattern was introduced. A new binding starts unmoved.
    fn bind_pattern(&mut self, p: &Pattern, default: Bind) {
        match &p.kind {
            PatternKind::Binding { mode, name } => self.bind_name(name, *mode, default),
            PatternKind::As { name, pattern } => {
                self.bind_name(name, None, default);
                self.bind_pattern(pattern, default);
            }
            PatternKind::Tuple(ps) => {
                for s in ps {
                    self.bind_pattern(s, default);
                }
            }
            PatternKind::List(items) => {
                for it in items {
                    match it {
                        ListPatItem::Pattern(s) => self.bind_pattern(s, default),
                        ListPatItem::Rest {
                            mode,
                            name: Some(n),
                            ..
                        } => self.bind_name(n, *mode, default),
                        ListPatItem::Rest { .. } => {}
                    }
                }
            }
            PatternKind::Variant { args, .. } => {
                for a in args.iter().flatten() {
                    self.bind_pattern(&a.pattern, default);
                }
            }
            PatternKind::Struct { fields, .. } => {
                for f in fields {
                    match &f.pattern {
                        Some(s) => self.bind_pattern(s, default),
                        None => self.bind_name(&f.name, f.mode, default),
                    }
                }
            }
            _ => {}
        }
    }

    fn bind_name(&mut self, name: &Ident, mode: Option<Mode>, default: Bind) {
        let Some(&d) = self.res.decls.get(&(self.file, name.span.start)) else {
            return;
        };
        let b = match (mode, default) {
            (Some(Mode::Inout), Bind::View | Bind::ViewInout) => Bind::ViewInout,
            (Some(Mode::Inout), _) => Bind::ViewInout,
            _ => default,
        };
        self.binds.insert(d, b);
        self.state.moved.remove(&d);
        if matches!(b, Bind::Var | Bind::Const | Bind::Sink | Bind::Owned) {
            self.track_linear(d);
        }
    }

    fn block(&mut self, b: &Block, ctx: Ctx) {
        self.scopes.push(Vec::new());
        for s in &b.stmts {
            self.stmt(s);
        }
        if let Some(t) = &b.tail {
            self.expr(t, ctx);
        }
        let end = Span::new(b.span.end.saturating_sub(1), b.span.end);
        self.leave_scope(end, "this block ends");
    }

    // ---- Names, moves and permission --------------------------------------------------------------------

    /// A mention of a variable: it must be live, and an owning value flowing into an owner moves.
    fn touch(&mut self, d: DefId, span: Span, ctx: Ctx, owning: bool) {
        let Some(&bind) = self.binds.get(&d) else {
            return;
        };
        if let Some(&at) = self.state.moved.get(&d) {
            let (n, line) = (self.name_of(d), self.line_of(at));
            self.report(
                span,
                "moved",
                format!("`{n}` was moved on line {line} and cannot be used again; write `copy {n}` there to keep it"),
            );
            return;
        }
        if ctx == Ctx::Consume && owning {
            self.take(d, bind, span);
        }
    }

    fn declared_outside_closure(&self, d: DefId) -> bool {
        match self.closures.last() {
            Some(c) => {
                let s = self.res.def(d).span;
                s.start < c.span.start || s.start >= c.span.end
            }
            None => false,
        }
    }

    fn take(&mut self, d: DefId, bind: Bind, span: Span) {
        let n = self.name_of(d);
        match bind {
            Bind::Let => self.report(
                span,
                "take",
                format!("`{n}` is read-only here, so it cannot be moved out; write `copy {n}`, or take the parameter with `sink`"),
            ),
            Bind::Inout => self.report(
                span,
                "take",
                format!("`{n}` is `inout`, so it cannot be moved out; write `copy {n}`"),
            ),
            Bind::View | Bind::ViewInout => {}
            Bind::Var | Bind::Const | Bind::Sink | Bind::Owned => {
                let captured_by_sink = self.closures.last().is_some_and(|c| {
                    c.caps.iter().any(|(cd, m)| *cd == d && *m == Some(Mode::Sink))
                });
                if self.declared_outside_closure(d) && self.is_linear_def(d) && !captured_by_sink {
                    self.report(
                        span,
                        "linear",
                        format!("`{n}` is a `linear` value from outside this closure; capture it with `[sink {n}]` to consume it here"),
                    );
                }
                if !self.declared_outside_closure(d) || captured_by_sink {
                    self.state.moved.insert(d, span);
                    self.state.held.remove(&d);
                }
            }
        }
    }

    /// Checks that the root of a place may be changed.
    fn may_change(&mut self, d: DefId, span: Span) {
        let Some(&bind) = self.binds.get(&d) else {
            return;
        };
        let n = self.name_of(d);
        if !bind.can_change() {
            let is_self = n == "self";
            let msg = match bind {
                Bind::Const => {
                    format!("`{n}` is a `const`, so it cannot be changed; declare it with `var`")
                }
                Bind::Let if is_self => {
                    "this method takes `self` read-only; write `inout self` to change it"
                        .to_string()
                }
                Bind::Let => {
                    format!("`{n}` is a read-only parameter; write `inout {n}` to change it")
                }
                _ => format!(
                    "`{n}` only views part of another value; bind it with `inout {n}` to change it"
                ),
            };
            self.report(span, "change", msg);
            return;
        }
        if self.declared_outside_closure(d) {
            let ok = self.closures.last().is_some_and(|c| {
                c.caps
                    .iter()
                    .any(|(cd, m)| *cd == d && matches!(m, Some(Mode::Inout | Mode::Sink)))
            });
            if !ok {
                self.report(
                    span,
                    "capture",
                    format!("this closure changes `{n}`; capture it with `[inout {n}]`"),
                );
            }
        }
    }

    /// Reads along a place (checking liveness of the root and walking index expressions), without
    /// moving anything.
    fn place_read(&mut self, e: &Expr) {
        let e = Self::strip(e);
        match &e.kind {
            ExprKind::Name(id) => {
                if let Some(d) = self.def_at_use(id.span.start) {
                    self.touch(d, e.span, Ctx::Read, false);
                }
            }
            ExprKind::SelfValue => {
                if let Some(d) = self.def_at_use(e.span.start) {
                    self.touch(d, e.span, Ctx::Read, false);
                }
            }
            ExprKind::Field { base, name, .. } => {
                if self.def_at_use(name.span.start).is_none() {
                    self.place_read(base);
                }
            }
            ExprKind::TupleIndex { base, .. } => self.place_read(base),
            ExprKind::Index { base, index } => {
                self.place_read(base);
                self.expr(index, Ctx::Read);
            }
            ExprKind::Call { .. } if self.box_get_base(e).is_some() => {
                if let Some(b) = self.box_get_base(e) {
                    self.place_read(b);
                }
            }
            _ => self.expr(e, Ctx::Read),
        }
    }

    fn change_place(&mut self, e: &Expr) {
        if let Some(p) = self.place_of(e) {
            self.may_change(p.root, Self::strip(e).span);
        }
    }

    // ---- Expressions ---------------------------------------------------------------------------------

    fn expr(&mut self, e: &Expr, ctx: Ctx) {
        match &e.kind {
            ExprKind::Name(id) => {
                if let Some(d) = self.def_at_use(id.span.start) {
                    let owning = self.ty_owning(e);
                    self.touch(d, e.span, ctx, owning);
                }
            }
            ExprKind::SelfValue => {
                if let Some(d) = self.def_at_use(e.span.start) {
                    let owning = self.ty_owning(e);
                    self.touch(d, e.span, ctx, owning);
                }
            }
            ExprKind::Paren(i) => self.expr(i, ctx),
            ExprKind::Copy(i) => self.place_read(i),
            ExprKind::Try(i) => {
                self.expr(i, ctx);
                if self.closures.is_empty() {
                    self.exit_scopes(0, e.span, "`?` can return from here");
                }
            }
            ExprKind::Field { base, name, .. } => {
                if self.def_at_use(name.span.start).is_some() {
                    return;
                }
                self.chain(e, base, ctx);
            }
            ExprKind::TupleIndex { base, .. } => self.chain(e, base, ctx),
            ExprKind::Index { base, index } => {
                self.chain(e, base, ctx);
                self.expr(index, Ctx::Read);
            }
            ExprKind::Binary { op, lhs, rhs } => {
                use crate::token::Punct;
                match op {
                    Punct::AndAnd | Punct::OrOr => {
                        self.expr(lhs, Ctx::Read);
                        let s0 = self.state.clone();
                        self.expr(rhs, Ctx::Read);
                        self.state = merge(s0, std::mem::take(&mut self.state));
                    }
                    Punct::QuestionQuestion => {
                        self.expr(lhs, ctx);
                        let s0 = self.state.clone();
                        self.expr(rhs, ctx);
                        self.state = merge(s0, std::mem::take(&mut self.state));
                    }
                    _ => {
                        self.expr(lhs, Ctx::Read);
                        self.expr(rhs, Ctx::Read);
                    }
                }
            }
            ExprKind::IncDec { operand, .. } => {
                self.place_read(operand);
                self.change_place(operand);
            }
            ExprKind::Assign { op, lhs, rhs } => self.assign(*op, lhs, rhs),
            ExprKind::Tuple(es) => {
                for x in es {
                    self.expr(x, Ctx::Consume);
                }
            }
            ExprKind::List(items) | ExprKind::Set(items) => {
                for i in items {
                    self.expr(&i.value, if i.spread { Ctx::Read } else { Ctx::Consume });
                }
            }
            ExprKind::Map(items) => {
                for i in items {
                    match i {
                        MapItem::Pair(k, v) => {
                            self.expr(k, Ctx::Consume);
                            self.expr(v, Ctx::Consume);
                        }
                        MapItem::Spread(s) => self.expr(s, Ctx::Read),
                    }
                }
            }
            ExprKind::StructLit { fields, .. } => {
                for f in fields {
                    match f {
                        StructInit::Field(FieldInit { value: Some(v), .. }) => {
                            self.expr(v, Ctx::Consume)
                        }
                        StructInit::Field(FieldInit { name, value: None }) => {
                            if let Some(d) = self.def_at_use(name.span.start) {
                                let owning = self
                                    .typed
                                    .local_tys
                                    .get(&d)
                                    .is_some_and(|t| self.typed.is_owning(*t));
                                self.touch(d, name.span, Ctx::Consume, owning);
                            }
                        }
                        StructInit::Spread(s) => self.expr(s, Ctx::Read),
                    }
                }
            }
            ExprKind::Call { .. } if self.box_get_base(e).is_some() => {
                if let Some(b) = self.box_get_base(e) {
                    self.chain(e, b, ctx);
                }
            }
            ExprKind::Call { callee, args, mark } => self.call(e, callee, args, *mark),
            ExprKind::Ternary { cond, then, else_ } => {
                self.expr(cond, Ctx::Read);
                let s0 = self.state.clone();
                self.expr(then, ctx);
                let s1 = std::mem::replace(&mut self.state, s0);
                self.expr(else_, ctx);
                self.state = merge(s1, std::mem::take(&mut self.state));
            }
            ExprKind::Is { value, pattern } => {
                self.place_read(value);
                self.bind_pattern(pattern, Bind::View);
            }
            ExprKind::Closure(c) => self.closure(e, c),
            ExprKind::If(i) | ExprKind::Unless(i) => self.if_expr(i, ctx),
            ExprKind::Switch(s) => self.switch(s, ctx),
            ExprKind::Select(arms) => {
                let s0 = self.state.clone();
                let mut out: Option<State> = None;
                for arm in arms {
                    self.state = s0.clone();
                    if let Some(op) = &arm.op {
                        self.expr(op, Ctx::Read);
                    }
                    if let Some(b) = &arm.bind {
                        self.bind_name(b, None, Bind::View);
                    }
                    self.block(&arm.body, ctx);
                    let st = std::mem::take(&mut self.state);
                    out = Some(match out {
                        Some(o) => merge(o, st),
                        None => st,
                    });
                }
                if let Some(o) = out {
                    self.state = o;
                }
            }
            ExprKind::Block(b) | ExprKind::Unsafe(b) => self.block(b, ctx),
            ExprKind::Scope { name, body } => {
                let handle = self.res.decls.get(&(self.file, name.span.start)).copied();
                if let Some(h) = handle {
                    self.task_scopes.push((h, Vec::new()));
                }
                self.block(body, Ctx::Read);
                if handle.is_some() {
                    self.task_scopes.pop();
                }
            }
            ExprKind::Loop(b) => {
                let label = self.pending_label.take();
                self.run_loop(true, label, |w| w.block(b, Ctx::Read));
            }
            ExprKind::While { cond, body } => {
                let label = self.pending_label.take();
                self.run_loop(false, label, |w| {
                    w.expr(cond, Ctx::Read);
                    w.block(body, Ctx::Read);
                });
            }
            ExprKind::DoWhile { body, cond } => {
                let label = self.pending_label.take();
                self.run_loop(false, label, |w| {
                    w.block(body, Ctx::Read);
                    w.expr(cond, Ctx::Read);
                });
            }
            ExprKind::For(f) => self.for_expr(f),
            ExprKind::Labeled { label, body } => {
                self.pending_label = Some(self.text(label.span).to_string());
                self.expr(body, ctx);
                self.pending_label = None;
            }
            ExprKind::Guard { cond, else_ } => {
                self.expr(cond, Ctx::Read);
                let after = self.state.clone();
                self.block(else_, Ctx::Read);
                self.state = after;
            }
            ExprKind::Return(v) | ExprKind::Yield(v) => {
                if let Some(v) = v {
                    self.expr(v, Ctx::Consume);
                    if matches!(e.kind, ExprKind::Return(_)) {
                        self.check_escape(v);
                    }
                }
                if matches!(e.kind, ExprKind::Return(_)) {
                    self.exit_scopes(0, e.span, "this `return`");
                    self.state.dead = true;
                }
            }
            ExprKind::Become(v) => {
                self.expr(v, Ctx::Consume);
                self.check_become(v);
                self.exit_scopes(0, e.span, "this `become`");
                self.state.dead = true;
            }
            ExprKind::Defer(inner) => {
                // The deferred code runs when the scope ends: its uses are checked here, its moves
                // happen later.
                self.defers += 1;
                let saved = self.state.clone();
                self.expr(inner, Ctx::Read);
                self.state = saved;
            }
            ExprKind::Break { label, value } => {
                if let Some(v) = value {
                    self.expr(v, Ctx::Consume);
                }
                let name = label.as_ref().map(|l| self.text(l.span).to_string());
                let depth = self.find_loop(name.as_deref()).map(|f| f.scope_depth);
                if let Some(d) = depth {
                    self.exit_scopes(d, e.span, "this `break`");
                }
                let st = self.state.clone();
                if let Some(f) = self.find_loop(name.as_deref()) {
                    f.breaks.push(st);
                }
                self.state.dead = true;
            }
            ExprKind::Continue { label } => {
                let name = label.as_ref().map(|l| self.text(l.span).to_string());
                let depth = self.find_loop(name.as_deref()).map(|f| f.scope_depth);
                if let Some(d) = depth {
                    self.exit_scopes(d, e.span, "this `continue`");
                }
                let st = self.state.clone();
                if let Some(f) = self.find_loop(name.as_deref()) {
                    f.continues.push(st);
                }
                self.state.dead = true;
            }
            ExprKind::Meta(_) | ExprKind::MetaIf(_) => {}
            _ => {
                let mut v = Generic { w: self };
                visit::walk_expr(&mut v, e);
            }
        }
        if !matches!(e.kind, ExprKind::Return(_) | ExprKind::Become(_)) && self.expr_never(e) {
            self.state.dead = true;
        }
    }

    /// `a.b`, `a[i]`: reading part of a variable. Taking an owning part out needs `copy`.
    fn chain(&mut self, e: &Expr, base: &Expr, ctx: Ctx) {
        self.place_read(base);
        if ctx == Ctx::Consume && self.ty_owning(e) {
            if let Some(p) = self.place_of(e) {
                if self.binds.contains_key(&p.root) {
                    let text = self.text(e.span).to_string();
                    let root = self.name_of(p.root);
                    self.report(
                        e.span,
                        "part",
                        format!("`{text}` cannot be taken out of `{root}`; write `copy {text}`"),
                    );
                }
            }
        }
    }

    fn find_loop(&mut self, label: Option<&str>) -> Option<&mut LoopFrame> {
        match label {
            Some(l) => self
                .loops
                .iter_mut()
                .rev()
                .find(|f| f.label.as_deref() == Some(l)),
            None => self.loops.last_mut(),
        }
    }

    fn assign(&mut self, op: crate::token::Punct, lhs: &Expr, rhs: &Expr) {
        use crate::token::Punct;
        if matches!(lhs.kind, ExprKind::Underscore) {
            self.expr(rhs, Ctx::Consume);
            return;
        }
        if op == Punct::Eq {
            self.expr(rhs, Ctx::Consume);
            let whole = self.place_of(lhs).filter(|p| p.segs.is_empty());
            if let Some(p) = whole {
                // A new value revives a moved variable.
                self.may_change(p.root, Self::strip(lhs).span);
                if self.state.held.contains(&p.root) {
                    let n = self.name_of(p.root);
                    self.report(
                        Self::strip(lhs).span,
                        "linear",
                        format!(
                            "`{n}` still holds a `linear` value; assigning over it would drop it"
                        ),
                    );
                }
                self.state.moved.remove(&p.root);
                self.track_linear(p.root);
                return;
            }
            self.place_read(lhs);
            self.change_place(lhs);
        } else {
            self.expr(rhs, Ctx::Read);
            self.place_read(lhs);
            self.change_place(lhs);
        }
    }

    fn if_expr(&mut self, i: &IfExpr, ctx: Ctx) {
        self.expr(&i.cond, Ctx::Read);
        let s0 = self.state.clone();
        self.block(&i.then, ctx);
        let s1 = std::mem::replace(&mut self.state, s0);
        if let Some(el) = &i.else_ {
            self.expr(el, ctx);
        }
        self.state = merge(s1, std::mem::take(&mut self.state));
    }

    fn switch(&mut self, s: &Switch, ctx: Ctx) {
        match s.mode {
            Some(Mode::Sink) => self.expr(&s.scrutinee, Ctx::Consume),
            Some(Mode::Inout) => {
                self.place_read(&s.scrutinee);
                self.change_place(&s.scrutinee);
            }
            _ => self.place_read(&s.scrutinee),
        }
        let s0 = self.state.clone();
        let mut out: Option<State> = None;
        for arm in &s.arms {
            self.state = s0.clone();
            for p in &arm.patterns {
                let b = if s.mode == Some(Mode::Inout) {
                    Bind::ViewInout
                } else {
                    Bind::View
                };
                self.bind_pattern(p, b);
            }
            if let Some(g) = &arm.guard {
                self.expr(g, Ctx::Read);
            }
            self.block(&arm.body, ctx);
            let st = std::mem::take(&mut self.state);
            out = Some(match out {
                Some(o) => merge(o, st),
                None => st,
            });
        }
        if let Some(o) = out {
            self.state = o;
        }
    }

    fn for_expr(&mut self, f: &ForExpr) {
        let label = self.pending_label.take();
        match &f.head {
            ForHead::Of { pats, iter } => {
                self.place_read(iter);
                self.run_loop(false, label, |w| {
                    for p in pats {
                        w.bind_pattern(p, Bind::View);
                    }
                    w.block(&f.body, Ctx::Read);
                });
            }
            ForHead::In { pat, iter } => {
                self.place_read(iter);
                self.run_loop(false, label, |w| {
                    w.bind_pattern(pat, Bind::View);
                    w.block(&f.body, Ctx::Read);
                });
            }
            ForHead::C { init, cond, step } => {
                if let Some(s) = init {
                    self.stmt(s);
                }
                self.run_loop(false, label, |w| {
                    if let Some(c) = cond {
                        w.expr(c, Ctx::Read);
                    }
                    w.block(&f.body, Ctx::Read);
                    if let Some(st) = step {
                        w.expr(st, Ctx::Read);
                    }
                });
            }
        }
    }

    /// Walks a loop body twice, so a move in one pass is a use after a move in the next.
    /// `forever` is for `loop`, which only ends with `break`.
    fn run_loop(&mut self, forever: bool, label: Option<String>, mut one: impl FnMut(&mut Self)) {
        let s0 = self.state.clone();
        self.loops.push(LoopFrame {
            scope_depth: self.scopes.len(),
            label,
            breaks: Vec::new(),
            continues: Vec::new(),
        });
        one(self);
        let mut end = std::mem::take(&mut self.state);
        if let Some(f) = self.loops.last_mut() {
            for c in std::mem::take(&mut f.continues) {
                end = merge(end, c);
            }
        }
        let start2 = merge(s0, end);
        self.state = start2.clone();
        one(self);
        let mut end2 = std::mem::take(&mut self.state);
        let frame = self.loops.pop();
        let mut out = if forever {
            State {
                dead: true,
                ..State::default()
            }
        } else {
            merge(start2, std::mem::take(&mut end2))
        };
        if let Some(mut f) = frame {
            for c in f.continues.drain(..) {
                if !forever {
                    out = merge(out, c);
                }
            }
            for b in f.breaks.drain(..) {
                out = merge(out, b);
            }
        }
        self.state = out;
    }

    fn closure(&mut self, e: &Expr, c: &Closure) {
        let pre = self.state.clone();
        let mut caps = Vec::new();
        for cap in &c.captures {
            let Some(d) = self.def_at_use(cap.name.span.start) else {
                continue;
            };
            caps.push((d, cap.mode));
            match cap.mode {
                Some(Mode::Sink) => {
                    let owning = self
                        .typed
                        .local_tys
                        .get(&d)
                        .is_some_and(|t| self.typed.is_owning(*t));
                    self.touch(d, cap.name.span, Ctx::Consume, owning);
                }
                Some(Mode::Inout) => {
                    self.touch(d, cap.name.span, Ctx::Read, false);
                    self.may_change(d, cap.name.span);
                }
                _ => self.touch(d, cap.name.span, Ctx::Read, false),
            }
        }
        // Inside the body the captured names are live; outside, the captures have taken effect.
        let after_captures = std::mem::replace(&mut self.state, pre);
        self.closures.push(ClosureFrame {
            scope_base: self.scopes.len(),
            span: e.span,
            caps,
        });
        // The modes the closure's type gives its parameters (`fn(inout int)`).
        let modes: Vec<Option<Mode>> = self
            .typed
            .tys
            .get(&(self.file, e.span.start, e.span.end))
            .map(|t| match self.typed.types.kind_of(*t) {
                TyKind::Fn(ps, _) => ps.iter().map(|p| p.mode).collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default();
        for (i, p) in c.params.iter().enumerate() {
            let default = match modes.get(i).copied().flatten() {
                Some(Mode::Inout) => Bind::ViewInout,
                _ => Bind::View,
            };
            self.bind_pattern(&p.pattern, default);
        }
        self.expr(&c.body, Ctx::Consume);
        let owned: Vec<DefId> = self
            .closures
            .last()
            .map(|f| {
                f.caps
                    .iter()
                    .filter(|(_, m)| *m == Some(Mode::Sink))
                    .map(|(d, _)| *d)
                    .collect()
            })
            .unwrap_or_default();
        self.report_held(&owned, e.span, "the closure ends");
        self.closures.pop();
        self.state = after_captures;
    }

    /// `become f(args)` replaces this function's frame, so nothing local may be lent to the call.
    fn check_become(&mut self, call: &Expr) {
        if self.defers > 0 {
            self.report(
                call.span,
                "become",
                "`become` cannot follow a `defer`: the deferred code would have to run after the call".to_string(),
            );
        }
        let ExprKind::Call { callee, args, .. } = &Self::strip(call).kind else {
            return;
        };
        let mut lent: Vec<(&Expr, bool)> = Vec::new();
        if let ExprKind::Field { base, name, .. } = &Self::strip(callee).kind {
            if self.def_at_use(name.span.start).is_none() {
                lent.push((base, false));
            }
        }
        for a in args {
            match a.mode {
                Some(Mode::Inout) => lent.push((&a.value, true)),
                Some(Mode::Sink) => {}
                _ => lent.push((&a.value, false)),
            }
        }
        for (x, changes) in lent {
            let Some(p) = self.place_of(x) else { continue };
            let local = matches!(
                self.binds.get(&p.root),
                Some(Bind::Var | Bind::Const | Bind::Sink | Bind::Owned)
            );
            if local && (changes || self.ty_owning(x)) {
                let n = self.name_of(p.root);
                self.report(
                    Self::strip(x).span,
                    "become",
                    format!("`{n}` is a local variable and would be gone when `become` replaces this call; pass `sink {n}` or `copy {n}`"),
                );
            }
        }
    }

    // ---- Calls and exclusive access ---------------------------------------------------------------------

    fn is_constructor(&self, callee: &Expr) -> bool {
        match &Self::strip(callee).kind {
            ExprKind::DotName(_) => true,
            ExprKind::Name(id) => self.def_at_use(id.span.start).is_some_and(|d| {
                let def = self.res.def(d);
                matches!(def.kind, DefKind::Variant | DefKind::Struct | DefKind::Obj)
                    || (matches!(def.kind, DefKind::BuiltinFn | DefKind::BuiltinCase)
                        && matches!(def.name.as_str(), "ok" | "err" | "some" | "box"))
            }),
            ExprKind::Field { name, .. } => self
                .def_at_use(name.span.start)
                .is_some_and(|d| self.res.def(d).kind == DefKind::Variant),
            ExprKind::TypeApply { base, .. } => self.is_constructor(base),
            _ => false,
        }
    }

    fn is_spawn(&self, callee: &Expr) -> bool {
        match &Self::strip(callee).kind {
            ExprKind::Name(id) => self.def_at_use(id.span.start).is_some_and(|d| {
                let def = self.res.def(d);
                def.kind == DefKind::Fn && def.name == "spawn"
            }),
            _ => false,
        }
    }

    /// A thread may outlive the function that starts it, so what its closure uses must be copied
    /// or moved in; nothing changeable may be shared.
    fn check_spawn(&mut self, closure: &Expr, c: &Closure) {
        let mut found = Used { starts: Vec::new() };
        found.visit_expr(&c.body);
        let mut seen: Vec<DefId> = Vec::new();
        for start in found.starts {
            let Some(d) = self.def_at_use(start) else {
                continue;
            };
            if seen.contains(&d) || !self.binds.contains_key(&d) {
                continue;
            }
            let ds = self.res.def(d).span;
            if ds.start >= closure.span.start && ds.start < closure.span.end {
                continue; // declared inside the closure
            }
            seen.push(d);
            let n = self.name_of(d);
            let cap = c
                .captures
                .iter()
                .find(|cap| self.def_at_use(cap.name.span.start) == Some(d));
            match cap.map(|cp| cp.mode) {
                Some(Some(Mode::Inout)) => self.report(
                    closure.span,
                    "spawn",
                    format!("`{n}` is captured with `inout`, but another thread cannot change what this one can see; move it in with `sink {n}` or use `lock`"),
                ),
                Some(_) => {}
                None => {
                    let owning = self
                        .typed
                        .local_tys
                        .get(&d)
                        .is_some_and(|t| self.typed.is_owning(*t));
                    if owning {
                        self.report(
                            closure.span,
                            "spawn",
                            format!("the new thread may outlive this function, so `{n}` must be moved in: capture it with `[sink {n}]` (`copy {n}` first to keep your own)"),
                        );
                    }
                }
            }
        }
    }

    /// `s.spawn(closure)` on a `scope` handle: no two tasks may change or consume the same variable.
    fn check_scope_task(&mut self, callee: &Expr, args: &[Arg]) {
        let ExprKind::Field { base, name, .. } = &Self::strip(callee).kind else {
            return;
        };
        if self.text(name.span) != "spawn" {
            return;
        }
        let ExprKind::Name(id) = &Self::strip(base).kind else {
            return;
        };
        let Some(h) = self.def_at_use(id.span.start) else {
            return;
        };
        let Some(idx) = self.task_scopes.iter().position(|(d, _)| *d == h) else {
            return;
        };
        for a in args {
            let ExprKind::Closure(c) = &Self::strip(&a.value).kind else {
                continue;
            };
            for cap in &c.captures {
                if !matches!(cap.mode, Some(Mode::Inout | Mode::Sink)) {
                    continue;
                }
                let Some(d) = self.def_at_use(cap.name.span.start) else {
                    continue;
                };
                if self.task_scopes[idx].1.contains(&d) {
                    let n = self.name_of(d);
                    self.report(
                        cap.name.span,
                        "spawn",
                        format!("`{n}` is captured by two tasks, and at least one of them changes or consumes it"),
                    );
                } else {
                    self.task_scopes[idx].1.push(d);
                }
            }
        }
    }

    fn call(&mut self, e: &Expr, callee: &Expr, args: &[Arg], mark: CallMark) {
        self.check_scope_task(callee, args);
        if self.is_spawn(callee) {
            for a in args {
                if let ExprKind::Closure(c) = &Self::strip(&a.value).kind {
                    self.check_spawn(&a.value, c);
                }
            }
        }
        let ctor = self.is_constructor(callee);
        let mut accesses: Vec<Access> = Vec::new();
        let receiver = match &Self::strip(callee).kind {
            ExprKind::Field { base, name, .. } if self.def_at_use(name.span.start).is_none() => {
                Some(&**base)
            }
            _ => None,
        };
        match receiver {
            Some(base) => match mark {
                CallMark::Changes => {
                    self.place_read(base);
                    self.change_place(base);
                    self.access(base, true, &mut accesses);
                }
                CallMark::Consumes => {
                    self.expr(base, Ctx::Consume);
                    self.access(base, true, &mut accesses);
                }
                CallMark::None => {
                    self.expr(base, Ctx::Read);
                    self.access(base, false, &mut accesses);
                }
            },
            None => self.expr(callee, Ctx::Read),
        }
        for a in args {
            match a.mode {
                Some(Mode::Inout) => {
                    self.place_read(&a.value);
                    self.change_place(&a.value);
                    self.access(&a.value, true, &mut accesses);
                }
                Some(Mode::Sink) => {
                    self.expr(&a.value, Ctx::Consume);
                    self.access(&a.value, true, &mut accesses);
                }
                _ if ctor && !a.spread => self.expr(&a.value, Ctx::Consume),
                _ => {
                    self.expr(&a.value, Ctx::Read);
                    self.access(&a.value, false, &mut accesses);
                }
            }
        }
        self.exclusive(&accesses);
        let _ = e;
    }

    /// Records the place an argument names. Reads of plain values are copies and do not count.
    fn access(&self, e: &Expr, changes: bool, out: &mut Vec<Access>) {
        let Some(place) = self.place_of(e) else {
            return;
        };
        if !changes && !self.ty_owning(e) {
            return;
        }
        out.push(Access {
            place,
            changes,
            span: Self::strip(e).span,
        });
    }

    fn exclusive(&mut self, accesses: &[Access]) {
        for (j, b) in accesses.iter().enumerate() {
            for a in &accesses[..j] {
                if (a.changes || b.changes) && overlap(&a.place, &b.place) {
                    let n = self.name_of(b.place.root);
                    self.report(
                        b.span,
                        "exclusive",
                        format!("`{n}` is changed or given away by this call and also used by another argument of it; copy one of them first"),
                    );
                    break;
                }
            }
        }
    }
}

/// Falls back to the generic tree walk for expressions without special rules.
struct Generic<'w, 'a> {
    w: &'w mut Walker<'a>,
}

impl<'w, 'a> Visitor for Generic<'w, 'a> {
    fn visit_expr(&mut self, e: &Expr) {
        // The tree outlives the walk; the visitor trait just cannot say so.
        let e: &Expr = unsafe { &*(e as *const Expr) };
        self.w.expr(e, Ctx::Read);
    }
    fn visit_pattern(&mut self, p: &Pattern) {
        let p: &Pattern = unsafe { &*(p as *const Pattern) };
        self.w.bind_pattern(p, Bind::View);
    }
    fn visit_stmt(&mut self, s: &Stmt) {
        let s: &Stmt = unsafe { &*(s as *const Stmt) };
        self.w.stmt(s);
    }
    fn visit_block(&mut self, b: &Block) {
        let b: &Block = unsafe { &*(b as *const Block) };
        self.w.block(b, Ctx::Read);
    }
    fn visit_type(&mut self, _t: &Type) {}
}

/// Collects where names (and `self`) are used.
struct Used {
    starts: Vec<u32>,
}

impl Visitor for Used {
    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Name(id) => self.starts.push(id.span.start),
            ExprKind::SelfValue => self.starts.push(e.span.start),
            _ => {}
        }
        visit::walk_expr(self, e);
    }
}
