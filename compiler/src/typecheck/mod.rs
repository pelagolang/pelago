//! The type checker (stage 2 of `docs/design/checker.md`): a type for every expression.
//!
//! The shape of the work:
//!
//! 1. **Index** every declaration (functions, types, fields, variants, `impl`s, constants).
//! 2. **Signatures** are computed on demand: a function's parameter and return types, a field's
//!    type, an enum case's payload. They never need a function body, so bodies can be checked in
//!    any order (and, later, in parallel).
//! 3. **Bodies** are checked one function at a time with local inference: expressions are checked
//!    against an expected type when there is one (so `[]`, `none`, `.circle(...)`, number literals
//!    and closures get their meaning from context). Unsolved literal types default to `int` and
//!    `float` at the end of each function; any other unsolved type is an error ("cannot infer").
//!
//! What it does not do yet (later stages): check that interface bounds hold and that `impl`s are
//! complete (stage 3), exhaustiveness of `switch` (stage 4), and exclusive access, moves and
//! `const` depth (stage 5). Types it cannot model yet become the quiet `Unknown` type, which
//! unifies with everything, so one gap does not cause a flood of errors.

mod call;
mod exhaust;
mod expr;
mod iface;
mod pat;
mod stmt;
#[cfg(test)]
mod tests;

use crate::ast::*;
use crate::package::{Diagnostic, ModuleId, PackageId, Program};
use crate::resolve::{Def, DefId, DefKind, Resolved};
use crate::span::Span;
use crate::types::*;
use std::collections::HashMap;

/// The result of checking a program.
pub struct Typed {
    /// The type of each expression, keyed by file and span.
    pub tys: HashMap<(ModuleId, u32, u32), Ty>,
    /// The type of each local, parameter, constant, field and so on.
    pub local_tys: HashMap<DefId, Ty>,
    pub types: TypeTable,
    pub diagnostics: Vec<Diagnostic>,
    /// The structs, enums and objs that own something (so they move instead of copying).
    pub owning: std::collections::HashSet<DefId>,
    /// The types declared `linear`: they must be consumed exactly once.
    pub linear: std::collections::HashSet<DefId>,
    names: Vec<String>,
}

impl Typed {
    /// Is a value of this type linear (it, or something inside it, was declared `linear`)?
    pub fn is_linear(&self, t: Ty) -> bool {
        linear_in(&self.types, &self.linear, t)
    }

    /// Does a value of this type own something, so that it moves instead of being copied?
    /// Type parameters count as plain for now: generic code cannot say yet whether `T` copies.
    pub fn is_owning(&self, t: Ty) -> bool {
        let t = self.types.shallow(t);
        match self.types.kind(t) {
            TyKind::String
            | TyKind::List(_)
            | TyKind::Map(..)
            | TyKind::Set(_)
            | TyKind::Box(_)
            | TyKind::Iter(_)
            | TyKind::Any(_) => true,
            TyKind::Param(g) => self.linear.contains(g),
            TyKind::Array(e, _) | TyKind::Option(e) | TyKind::Range(e) => self.is_owning(*e),
            TyKind::Tuple(ts) | TyKind::AltTuple(ts, _) => ts.iter().any(|x| self.is_owning(*x)),
            TyKind::Result(a, b) => self.is_owning(*a) || self.is_owning(*b),
            TyKind::Adt(d, args) => {
                self.owning.contains(d) || args.iter().any(|x| self.is_owning(*x))
            }
            _ => false,
        }
    }

    /// A type written the way it is in source.
    pub fn show(&self, t: Ty) -> String {
        self.types.display(t, &|d| {
            self.names.get(d.0 as usize).cloned().unwrap_or_default()
        })
    }
}

/// Checks every module of the program. `resolved` is the result of name resolution.
pub fn check(program: &Program, resolved: &Resolved) -> Typed {
    let mut c = Checker::new(program, resolved);
    c.index_all();
    c.prepare_impls();
    c.check_impls();
    c.check_entry();
    c.check_declarations();
    c.check_all();
    c.finish()
}

// ---- Indexing ------------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub(crate) enum Owner {
    None,
    Adt(DefId),
    Impl(usize),
}

#[derive(Clone, Copy)]
pub(crate) enum AdtDecl<'p> {
    Type(&'p TypeDecl),
    Enum(&'p EnumDecl),
}

#[derive(Clone, Copy)]
pub(crate) enum Node<'p> {
    Fn {
        file: ModuleId,
        scope: usize,
        f: &'p FnDecl,
        owner: Owner,
    },
    Adt {
        file: ModuleId,
        scope: usize,
        decl: AdtDecl<'p>,
    },
    Alias {
        file: ModuleId,
        scope: usize,
        generics: &'p Generics,
        ty: &'p Type,
    },
    Global {
        file: ModuleId,
        scope: usize,
        decl: &'p Decl,
    },
    Field {
        file: ModuleId,
        scope: usize,
        f: &'p FieldDecl,
        owner: DefId,
    },
    Variant {
        file: ModuleId,
        scope: usize,
        v: &'p Variant,
        owner: DefId,
    },
}

pub(crate) struct ImplInfo<'p> {
    file: ModuleId,
    scope: usize,
    decl: &'p ImplDecl,
    generics: Vec<DefId>,
    /// The type the methods are for.
    self_ty: Ty,
    methods: HashMap<String, DefId>,
    /// The interface of `impl iface for type`, and its type arguments (`converts<app_error>`).
    iface: Option<DefId>,
    iface_args: Vec<Ty>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Head {
    Int(IntKind),
    Float(FloatKind),
    Bool,
    Char,
    String,
    Str,
    Adt(DefId),
    List,
    Array,
    Tuple,
    Option,
    Result,
    Map,
    Set,
    Range,
    Iter,
    Box,
    Fn,
}

#[derive(Clone)]
pub(crate) struct ParamSig {
    pub name: String,
    pub mode: Option<Mode>,
    pub ty: Ty,
    pub has_default: bool,
    pub variadic: bool,
}

#[derive(Clone)]
pub(crate) struct FnSig {
    /// The function's own generic parameters.
    pub generics: Vec<DefId>,
    pub params: Vec<ParamSig>,
    pub ret: Ty,
    pub has_self: bool,
    pub self_mode: Option<Mode>,
}

#[derive(Clone, Copy)]
pub(crate) struct FnCtx {
    ret: Ty,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoopKind {
    /// `loop { ... }`: can end with a value.
    Loop,
    /// `while`, `for`, `do`.
    Plain,
    /// A labeled block `name: { ... }`.
    Block,
}

pub(crate) struct LoopCtx {
    label: Option<String>,
    kind: LoopKind,
    /// The type `break value` produces, once known.
    break_ty: Option<Ty>,
    has_break: bool,
}

struct LitCheck {
    ty: Ty,
    magnitude: u128,
    negative: bool,
    file: ModuleId,
    span: Span,
}

/// What the surrounding code wants from an expression.
#[derive(Clone, Copy)]
pub(crate) enum Exp {
    /// Nothing in particular.
    Any,
    /// A value of this type.
    Is(Ty),
    /// The value is thrown away.
    Discard,
}

impl Exp {
    fn ty(self) -> Option<Ty> {
        match self {
            Exp::Is(t) => Some(t),
            _ => None,
        }
    }
}

pub(crate) struct Checker<'p> {
    program: &'p Program,
    res: &'p Resolved,
    tt: TypeTable,
    diags: Vec<Diagnostic>,
    nodes: HashMap<DefId, Node<'p>>,
    impls: Vec<ImplInfo<'p>>,
    impl_by_head: HashMap<Head, Vec<usize>>,
    fn_sigs: HashMap<DefId, FnSig>,
    sig_busy: Vec<DefId>,
    adt_generics: HashMap<DefId, Vec<DefId>>,
    field_tys: HashMap<DefId, Ty>,
    variant_fields: HashMap<DefId, Vec<(Option<String>, Ty)>>,
    alias_tys: HashMap<DefId, (Vec<DefId>, Ty)>,
    pub(crate) local_tys: HashMap<DefId, Ty>,
    global_busy: Vec<DefId>,
    expr_tys: Vec<(ModuleId, u32, u32, Ty)>,
    // Where we are.
    file: ModuleId,
    scope: usize,
    self_ty: Vec<Option<Ty>>,
    fn_ctx: Vec<FnCtx>,
    loops: Vec<LoopCtx>,
    /// A label waiting for the loop it names.
    pending_label: Option<String>,
    lits: Vec<LitCheck>,
    queue: Vec<DefId>,
    done: Vec<DefId>,
    var_floor: u32,
    /// Top-level statements and constants of modules, checked as one pseudo-function each.
    tests: Vec<(ModuleId, usize, &'p Block)>,
    top_stmts: Vec<(ModuleId, usize, &'p [Stmt])>,
    /// The interfaces each generic parameter must provide, as written.
    param_bounds: HashMap<DefId, Vec<&'p Type>>,
    /// `@derive(...)` names of types.
    derives: HashMap<DefId, Vec<String>>,
    /// Bounds to check once the types are known.
    pending_bounds: Vec<iface::PendingBound>,
    core_ifaces: HashMap<String, DefId>,
    pat_ctors: HashMap<(ModuleId, u32, u32), exhaust::PatCtor>,
    pending_matches: Vec<exhaust::PendingMatch<'p>>,
    /// Places passed to a `sink` parameter without `sink`: an error only if they own something.
    pending_sinks: Vec<(ModuleId, Span, String)>,
    owning_cache: Option<std::collections::HashSet<DefId>>,
    linear: std::collections::HashSet<DefId>,
    pending_nonlinear: Vec<(ModuleId, Span, Ty, DefId)>,
}

fn int_text(text: &str) -> Option<u128> {
    let t: String = text.chars().filter(|c| *c != '_').collect();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u128::from_str_radix(h, 16).ok()
    } else if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        u128::from_str_radix(b, 2).ok()
    } else if let Some(o) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        u128::from_str_radix(o, 8).ok()
    } else {
        t.parse().ok()
    }
}

impl<'p> Checker<'p> {
    fn new(program: &'p Program, res: &'p Resolved) -> Checker<'p> {
        Checker {
            program,
            res,
            tt: TypeTable::new(),
            diags: Vec::new(),
            nodes: HashMap::new(),
            impls: Vec::new(),
            impl_by_head: HashMap::new(),
            fn_sigs: HashMap::new(),
            sig_busy: Vec::new(),
            adt_generics: HashMap::new(),
            field_tys: HashMap::new(),
            variant_fields: HashMap::new(),
            alias_tys: HashMap::new(),
            local_tys: HashMap::new(),
            global_busy: Vec::new(),
            expr_tys: Vec::new(),
            file: ModuleId(0),
            scope: 0,
            self_ty: Vec::new(),
            fn_ctx: Vec::new(),
            loops: Vec::new(),
            pending_label: None,
            lits: Vec::new(),
            queue: Vec::new(),
            done: Vec::new(),
            var_floor: 0,
            tests: Vec::new(),
            top_stmts: Vec::new(),
            param_bounds: HashMap::new(),
            derives: HashMap::new(),
            pending_bounds: Vec::new(),
            core_ifaces: HashMap::new(),
            pat_ctors: HashMap::new(),
            pending_matches: Vec::new(),
            pending_sinks: Vec::new(),
            owning_cache: None,
            linear: std::collections::HashSet::new(),
            pending_nonlinear: Vec::new(),
        }
    }

    fn finish(mut self) -> Typed {
        self.flush_bounds();
        self.flush_matches();
        self.flush_sinks();
        let mut tys = HashMap::new();
        let entries = std::mem::take(&mut self.expr_tys);
        for (f, s, e, t) in entries {
            let z = self.tt.zonk(t);
            tys.insert((f, s, e), z);
        }
        let locals: Vec<(DefId, Ty)> = self.local_tys.iter().map(|(d, t)| (*d, *t)).collect();
        let mut local_tys = HashMap::new();
        for (d, t) in locals {
            let z = self.tt.zonk(t);
            local_tys.insert(d, z);
        }
        let owning = self.compute_owning();
        let linear = std::mem::take(&mut self.linear);
        let mut diagnostics = self.res.diagnostics.clone();
        diagnostics.extend(self.diags);
        Typed {
            owning,
            linear,
            tys,
            local_tys,
            names: self.res.defs.iter().map(|d| d.name.clone()).collect(),
            types: self.tt,
            diagnostics,
        }
    }

    // ---- Small helpers --------------------------------------------------------------------

    pub(crate) fn err(&mut self, span: Span, message: impl Into<String>) {
        self.diags.push(Diagnostic {
            module: self.file,
            span,
            message: message.into(),
            warning: false,
        });
    }

    /// Something worth fixing that does not stop compilation.
    pub(crate) fn warn(&mut self, span: Span, message: impl Into<String>) {
        self.diags.push(Diagnostic {
            module: self.file,
            span,
            message: message.into(),
            warning: true,
        });
    }

    fn def(&self, d: DefId) -> &'p Def {
        self.res.def(d)
    }

    fn text(&self, id: &Ident) -> &'p str {
        id.text(&self.program.module(self.file).src)
    }

    fn text_in(&self, file: ModuleId, id: &Ident) -> &'p str {
        id.text(&self.program.module(file).src)
    }

    /// The candidate closest to `want`, when it is close enough to be a typo.
    pub(crate) fn closest<'a>(
        &self,
        want: &str,
        candidates: impl Iterator<Item = &'a str>,
    ) -> Option<String> {
        if want.chars().count() < 3 {
            return None;
        }
        let limit = if want.chars().count() <= 4 { 1 } else { 2 };
        candidates
            .map(|c| (crate::resolve::edit_distance(want, c), c))
            .filter(|(d, c)| *d > 0 && *d <= limit && *c != want)
            .min_by_key(|(d, _)| *d)
            .map(|(_, c)| c.to_string())
    }

    pub(crate) fn show(&self, t: Ty) -> String {
        self.tt.display(t, &|d| self.res.def(d).name.clone())
    }

    pub(crate) fn var(&mut self, span: Span) -> Ty {
        self.tt.fresh(VarKind::General, span)
    }

    pub(crate) fn record(&mut self, e: &Expr, ty: Ty) {
        self.expr_tys
            .push((self.file, e.span.start, e.span.end, ty));
    }

    /// The definition a name at `id` was resolved to.
    pub(crate) fn used(&self, id: &Ident) -> Option<DefId> {
        self.res.use_at(self.file, id.span.start)
    }

    /// The definition declared at `id`.
    pub(crate) fn declared(&self, id: &Ident) -> Option<DefId> {
        self.res.decls.get(&(self.file, id.span.start)).copied()
    }

    pub(crate) fn kind_of(&self, t: Ty) -> TyKind {
        self.tt.kind_of(t).clone()
    }

    fn with_ctx<T>(
        &mut self,
        file: ModuleId,
        scope: usize,
        self_ty: Option<Ty>,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let saved = (self.file, self.scope);
        self.file = file;
        self.scope = scope;
        self.self_ty.push(self_ty);
        let out = f(self);
        self.self_ty.pop();
        self.file = saved.0;
        self.scope = saved.1;
        out
    }

    // ---- Phase 1: index declarations -----------------------------------------------------------

    fn index_all(&mut self) {
        let program = self.program;
        for m in &program.modules {
            let scope = m.id.0 as usize;
            self.file = m.id;
            self.index_stmts(m.id, scope, &m.file.stmts, true);
        }
    }

    fn index_stmts(&mut self, file: ModuleId, scope: usize, stmts: &'p [Stmt], top: bool) {
        if top {
            self.top_stmts.push((file, scope, stmts));
        }
        for s in stmts {
            match &s.kind {
                StmtKind::Item(i) => self.index_item(file, scope, i),
                StmtKind::Decl(d) => self.index_decl(file, scope, d),
                StmtKind::Expr(e) => self.index_meta_expr(file, scope, e),
            }
        }
    }

    /// `meta if` blocks at module level hold declarations.
    fn index_meta_expr(&mut self, file: ModuleId, scope: usize, e: &'p Expr) {
        match &e.kind {
            ExprKind::MetaIf(i) => {
                self.index_stmts(file, scope, &i.then.stmts, false);
                if let Some(el) = &i.else_ {
                    self.index_meta_expr(file, scope, el);
                }
            }
            ExprKind::Block(b) => self.index_stmts(file, scope, &b.stmts, false),
            _ => {}
        }
    }

    fn index_decl(&mut self, file: ModuleId, scope: usize, d: &'p Decl) {
        let mut names = Vec::new();
        crate::resolve::collect_pattern_names(&d.pattern, &mut names);
        for (id, _) in names {
            if let Some(&def) = self.res.decls.get(&(file, id.span.start)) {
                self.nodes.insert(
                    def,
                    Node::Global {
                        file,
                        scope,
                        decl: d,
                    },
                );
            }
        }
    }

    /// Indexes one declaration, including those nested in types and `mod`s.
    pub(crate) fn index_item(&mut self, file: ModuleId, scope: usize, item: &'p Item) {
        let def = self.res.item_defs.get(&(file, item.span.start)).copied();
        match &item.kind {
            ItemKind::Fn(f) => {
                self.register_generics(file, &f.generics, &f.where_);
                if let Some(d) = def {
                    self.nodes.insert(
                        d,
                        Node::Fn {
                            file,
                            scope,
                            f,
                            owner: Owner::None,
                        },
                    );
                }
            }
            ItemKind::Struct(d) | ItemKind::Obj(d) | ItemKind::Interface(d) => {
                self.register_generics(file, &d.generics, &d.where_);
                let Some(def) = def else { return };
                self.register_derives(file, def, &item.attrs);
                if item.mods.is_linear {
                    self.linear.insert(def);
                }
                self.nodes.insert(
                    def,
                    Node::Adt {
                        file,
                        scope,
                        decl: AdtDecl::Type(d),
                    },
                );
                for m in &d.members {
                    self.index_member(file, scope, def, m);
                }
            }
            ItemKind::Enum(e) => {
                self.register_generics(file, &e.generics, &e.where_);
                let Some(def) = def else { return };
                self.register_derives(file, def, &item.attrs);
                if item.mods.is_linear {
                    self.linear.insert(def);
                }
                self.nodes.insert(
                    def,
                    Node::Adt {
                        file,
                        scope,
                        decl: AdtDecl::Enum(e),
                    },
                );
                for v in &e.variants {
                    let name = self.text_in(file, &v.name);
                    if let Some(vd) = self.res.member(def, name) {
                        self.nodes.insert(
                            vd,
                            Node::Variant {
                                file,
                                scope,
                                v,
                                owner: def,
                            },
                        );
                    }
                }
                for it in &e.items {
                    self.index_member_item(file, scope, Owner::Adt(def), it);
                }
            }
            ItemKind::Impl(im) => {
                let idx = self.impls.len();
                self.impls.push(ImplInfo {
                    file,
                    scope,
                    decl: im,
                    generics: Vec::new(),
                    self_ty: self.tt.unknown,
                    methods: HashMap::new(),
                    iface: None,
                    iface_args: Vec::new(),
                });
                self.register_generics(file, &im.generics, &im.where_);
                for m in &im.members {
                    if let Member::Item(i) = m {
                        self.index_member_item(file, scope, Owner::Impl(idx), i);
                    }
                }
            }
            ItemKind::TypeAlias { generics, ty, .. } => {
                if let Some(d) = def {
                    self.nodes.insert(
                        d,
                        Node::Alias {
                            file,
                            scope,
                            generics,
                            ty,
                        },
                    );
                }
            }
            ItemKind::Decl(d) => self.index_decl(file, scope, d),
            ItemKind::Mod {
                body: Some(body), ..
            } => {
                if let Some(d) = def {
                    if let Some(inner) = self.def(d).scope {
                        self.index_stmts(file, inner, body, false);
                    }
                }
            }
            ItemKind::Test { body, .. } => self.tests.push((file, scope, body)),
            ItemKind::Mod { .. } | ItemKind::Import(_) => {}
        }
    }

    fn index_member(&mut self, file: ModuleId, scope: usize, owner: DefId, m: &'p Member) {
        match m {
            Member::Field(f) => {
                let name = self.text_in(file, &f.name);
                if let Some(fd) = self.res.member(owner, name) {
                    self.nodes.insert(
                        fd,
                        Node::Field {
                            file,
                            scope,
                            f,
                            owner,
                        },
                    );
                }
            }
            Member::Item(i) => self.index_member_item(file, scope, Owner::Adt(owner), i),
        }
    }

    fn index_member_item(&mut self, file: ModuleId, scope: usize, owner: Owner, i: &'p Item) {
        match &i.kind {
            ItemKind::Fn(f) => {
                self.register_generics(file, &f.generics, &f.where_);
                if let Some(&d) = self.res.item_defs.get(&(file, i.span.start)) {
                    self.nodes.insert(
                        d,
                        Node::Fn {
                            file,
                            scope,
                            f,
                            owner,
                        },
                    );
                    if let Owner::Impl(idx) = owner {
                        let name = self.text_in(file, &f.name).to_string();
                        self.impls[idx].methods.insert(name, d);
                    }
                }
            }
            ItemKind::Decl(d) => self.index_decl(file, scope, d),
            _ => {}
        }
    }

    fn head_of(&self, t: Ty) -> Option<Head> {
        Some(match self.tt.kind_of(t) {
            TyKind::Int(k) => Head::Int(*k),
            TyKind::Float(k) => Head::Float(*k),
            TyKind::Bool => Head::Bool,
            TyKind::Char => Head::Char,
            TyKind::String => Head::String,
            TyKind::Str => Head::Str,
            TyKind::Adt(d, _) => Head::Adt(*d),
            TyKind::List(_) => Head::List,
            TyKind::Array(..) => Head::Array,
            TyKind::Tuple(_) => Head::Tuple,
            TyKind::Option(_) => Head::Option,
            TyKind::Result(..) => Head::Result,
            TyKind::Map(..) => Head::Map,
            TyKind::Set(_) => Head::Set,
            TyKind::Range(_) => Head::Range,
            TyKind::Iter(_) => Head::Iter,
            TyKind::Box(_) => Head::Box,
            TyKind::Fn(..) => Head::Fn,
            _ => return None,
        })
    }

    /// Works out the generics and the target type of every `impl`.
    fn prepare_impls(&mut self) {
        for i in 0..self.impls.len() {
            let (file, scope, decl) = {
                let im = &self.impls[i];
                (im.file, im.scope, im.decl)
            };
            let generics: Vec<DefId> = decl
                .generics
                .params
                .iter()
                .filter_map(|p| self.res.decls.get(&(file, p.name.span.start)).copied())
                .collect();
            // The type the methods are for: the type after `for`, or the single type.
            let target = decl.for_type.as_ref().unwrap_or(&decl.target);
            let self_ty = self.with_ctx(file, scope, None, |c| c.lower_type(target));
            let iface = decl
                .for_type
                .as_ref()
                .and_then(|_| self.iface_of_type(file, &decl.target));
            if decl.for_type.is_some() && iface.is_none() {
                let saved = self.file;
                self.file = file;
                if !matches!(decl.target.kind, TypeKind::Error) {
                    self.err(decl.target.span, "this is not an interface");
                }
                self.file = saved;
            }
            let iface_args = match &decl.target.kind {
                TypeKind::Path { args, .. } if iface.is_some() => {
                    self.with_ctx(file, scope, None, |c| c.lower_generic_args(args))
                }
                _ => Vec::new(),
            };
            self.impls[i].iface_args = iface_args;
            self.impls[i].iface = iface;
            self.impls[i].generics = generics;
            self.impls[i].self_ty = self_ty;
            if let Some(h) = self.head_of(self_ty) {
                self.impl_by_head.entry(h).or_default().push(i);
            }
        }
    }

    // ---- Phase 2: signatures ---------------------------------------------------------------------

    pub(crate) fn generics_of(&self, file: ModuleId, g: &Generics) -> Vec<DefId> {
        g.params
            .iter()
            .filter_map(|p| self.res.decls.get(&(file, p.name.span.start)).copied())
            .collect()
    }

    /// The generic parameters of a struct, obj, enum or interface.
    pub(crate) fn adt_generics(&mut self, def: DefId) -> Vec<DefId> {
        if let Some(g) = self.adt_generics.get(&def) {
            return g.clone();
        }
        let g = match self.nodes.get(&def).copied() {
            Some(Node::Adt { file, decl, .. }) => {
                let gens = match decl {
                    AdtDecl::Type(d) => &d.generics,
                    AdtDecl::Enum(e) => &e.generics,
                };
                self.generics_of(file, gens)
            }
            _ => Vec::new(),
        };
        self.adt_generics.insert(def, g.clone());
        g
    }

    /// `Adt(def, [Param(g)...])`: the type inside its own declaration.
    pub(crate) fn own_adt_ty(&mut self, def: DefId) -> Ty {
        let gens = self.adt_generics(def);
        let args: Vec<Ty> = gens.iter().map(|g| self.tt.mk(TyKind::Param(*g))).collect();
        self.tt.mk(TyKind::Adt(def, args))
    }

    /// The `Self` type for a function with this owner.
    fn owner_self_ty(&mut self, owner: Owner) -> Option<Ty> {
        match owner {
            Owner::None => None,
            // Inside an interface, `Self` is a stand-in named by the interface itself.
            Owner::Adt(d) if self.def(d).kind == DefKind::Interface => {
                Some(self.tt.mk(TyKind::Param(d)))
            }
            Owner::Adt(d) => Some(self.own_adt_ty(d)),
            Owner::Impl(i) => Some(self.impls[i].self_ty),
        }
    }

    pub(crate) fn fn_sig(&mut self, def: DefId) -> Option<FnSig> {
        if let Some(s) = self.fn_sigs.get(&def) {
            return Some(s.clone());
        }
        let Some(Node::Fn {
            file,
            scope,
            f,
            owner,
            ..
        }) = self.nodes.get(&def).copied()
        else {
            return None;
        };
        if self.sig_busy.contains(&def) {
            return None;
        }
        self.sig_busy.push(def);
        let self_ty = self.owner_self_ty(owner);
        let sig = self.with_ctx(file, scope, self_ty, |c| {
            let generics = c.generics_of(file, &f.generics);
            let mut params = Vec::new();
            let mut has_self = false;
            let mut self_mode = None;
            for p in &f.params {
                if p.is_self {
                    has_self = true;
                    self_mode = p.mode;
                    continue;
                }
                let ty = match &p.ty {
                    Some(t) => c.lower_type(t),
                    None => c.tt.unknown,
                };
                // A variadic parameter is a list inside the function.
                params.push(ParamSig {
                    name: c.text_in(file, &p.name).to_string(),
                    mode: p.mode,
                    ty,
                    has_default: p.default.is_some(),
                    variadic: p.variadic,
                });
            }
            let ret = match &f.ret {
                Some(t) => c.lower_type(t),
                None => c.tt.void,
            };
            FnSig {
                generics,
                params,
                ret,
                has_self,
                self_mode,
            }
        });
        self.sig_busy.pop();
        self.fn_sigs.insert(def, sig.clone());
        Some(sig)
    }

    pub(crate) fn field_ty(&mut self, field: DefId) -> Ty {
        if let Some(&t) = self.field_tys.get(&field) {
            return t;
        }
        let Some(Node::Field {
            file,
            scope,
            f,
            owner,
        }) = self.nodes.get(&field).copied()
        else {
            return self.tt.unknown;
        };
        let self_ty = self.own_adt_ty(owner);
        let t = self.with_ctx(file, scope, Some(self_ty), |c| c.lower_type(&f.ty));
        self.field_tys.insert(field, t);
        t
    }

    pub(crate) fn variant_fields(&mut self, variant: DefId) -> Vec<(Option<String>, Ty)> {
        if let Some(v) = self.variant_fields.get(&variant) {
            return v.clone();
        }
        let Some(Node::Variant {
            file,
            scope,
            v,
            owner,
        }) = self.nodes.get(&variant).copied()
        else {
            return Vec::new();
        };
        let self_ty = self.own_adt_ty(owner);
        let fields = self.with_ctx(file, scope, Some(self_ty), |c| {
            v.fields
                .iter()
                .map(|f| {
                    let label = f.label.as_ref().map(|l| c.text_in(file, l).to_string());
                    (label, c.lower_type(&f.ty))
                })
                .collect::<Vec<_>>()
        });
        self.variant_fields.insert(variant, fields.clone());
        fields
    }

    /// The `obj` bases of a type (and the interfaces it lists), as types.
    pub(crate) fn bases_of(&mut self, def: DefId) -> Vec<Ty> {
        let Some(Node::Adt {
            file, scope, decl, ..
        }) = self.nodes.get(&def).copied()
        else {
            return Vec::new();
        };
        let bases = match decl {
            AdtDecl::Type(d) => &d.bases,
            AdtDecl::Enum(e) => &e.bases,
        };
        let self_ty = self.own_adt_ty(def);
        self.with_ctx(file, scope, Some(self_ty), |c| {
            let mut out = Vec::new();
            for b in bases {
                if c.iface_of_type(file, b).is_none() {
                    out.push(c.lower_type(b));
                }
            }
            out
        })
    }

    // ---- Lowering written types ------------------------------------------------------------------

    pub(crate) fn lower_type(&mut self, t: &'p Type) -> Ty {
        match &t.kind {
            TypeKind::Path { path, args } => self.lower_path_type(t, path, args),
            TypeKind::SelfType => self
                .self_ty
                .last()
                .copied()
                .flatten()
                .unwrap_or(self.tt.unknown),
            TypeKind::List(e) => {
                let e = self.lower_type(e);
                self.tt.list(e)
            }
            TypeKind::Array(e, n) => {
                let e = self.lower_type(e);
                let len = self.array_len(n);
                self.tt.mk(TyKind::Array(e, len))
            }
            TypeKind::Tuple(ts) => {
                if ts.is_empty() {
                    return self.tt.void;
                }
                let v = ts.iter().map(|t| self.lower_type(t)).collect();
                self.tt.tuple(v)
            }
            TypeKind::FixedTuple(ts, n) => {
                let v = ts.iter().map(|t| self.lower_type(t)).collect();
                let len = self.array_len(n);
                self.tt.mk(TyKind::AltTuple(v, len))
            }
            TypeKind::Option(e) => {
                let e = self.lower_type(e);
                self.tt.option(e)
            }
            TypeKind::Result(a, b) => {
                let a = self.lower_type(a);
                let b = self.lower_type(b);
                self.tt.result(a, b)
            }
            TypeKind::Fn { params, ret } => {
                let ps: Vec<ParamTy> = params
                    .iter()
                    .map(|p| ParamTy {
                        mode: p.mode,
                        ty: self.lower_type(&p.ty),
                    })
                    .collect();
                let r = match ret {
                    Some(r) => self.lower_type(r),
                    None => self.tt.void,
                };
                self.tt.mk(TyKind::Fn(ps, r))
            }
            TypeKind::Any(e) => match &e.kind {
                TypeKind::Path { path, .. } => {
                    let last = path.segments.last();
                    match last.and_then(|l| self.used(l)) {
                        Some(d) if self.def(d).kind == DefKind::Interface => {
                            self.tt.mk(TyKind::Any(d))
                        }
                        _ => self.tt.unknown,
                    }
                }
                _ => self.tt.unknown,
            },
            TypeKind::Error => self.tt.unknown,
        }
    }

    fn array_len(&mut self, n: &'p Expr) -> ArrayLen {
        match &n.kind {
            ExprKind::Int => {
                let text =
                    &self.program.module(self.file).src[n.span.start as usize..n.span.end as usize];
                int_text(text).map_or(ArrayLen::Unknown, |v| ArrayLen::Known(v as u64))
            }
            ExprKind::Name(id) => match self.used(id) {
                Some(d) if self.def(d).kind == DefKind::GenericParam => ArrayLen::Param(d),
                _ => ArrayLen::Unknown,
            },
            _ => ArrayLen::Unknown,
        }
    }

    fn lower_generic_args(&mut self, args: &'p [GenericArg]) -> Vec<Ty> {
        args.iter()
            .filter_map(|a| match a {
                GenericArg::Type(t) => Some(self.lower_type(t)),
                GenericArg::Const(_) => None,
            })
            .collect()
    }

    fn lower_path_type(&mut self, t: &'p Type, path: &'p Path, args: &'p [GenericArg]) -> Ty {
        let Some(last) = path.segments.last() else {
            return self.tt.unknown;
        };
        let Some(def) = self.used(last) else {
            return self.tt.unknown;
        };
        let d = self.def(def);
        match d.kind {
            DefKind::GenericParam => self.tt.mk(TyKind::Param(def)),
            DefKind::BuiltinType => self.builtin_type(&d.name, args, t.span),
            DefKind::Struct | DefKind::Obj | DefKind::Enum => {
                let lowered = self.lower_generic_args(args);
                let want = self.adt_generics(def).len();
                if lowered.len() != want {
                    let name = d.name.clone();
                    self.err(
                        t.span,
                        format!(
                            "`{name}` takes {want} type argument{}, but {} {} given",
                            if want == 1 { "" } else { "s" },
                            lowered.len(),
                            if lowered.len() == 1 { "was" } else { "were" }
                        ),
                    );
                    return self.tt.unknown;
                }
                let gens = self.adt_generics(def);
                self.queue_bounds(&gens, &lowered, t.span);
                self.tt.mk(TyKind::Adt(def, lowered))
            }
            DefKind::Interface => {
                let name = d.name.clone();
                self.err(t.span, format!("`{name}` is an interface; write `any {name}` for a value of any type that provides it"));
                self.tt.unknown
            }
            DefKind::TypeAlias => {
                let lowered = self.lower_generic_args(args);
                self.alias_ty(def, &lowered)
            }
            _ => self.tt.unknown,
        }
    }

    fn alias_ty(&mut self, def: DefId, args: &[Ty]) -> Ty {
        if !self.alias_tys.contains_key(&def) {
            let Some(Node::Alias {
                file,
                scope,
                generics,
                ty,
            }) = self.nodes.get(&def).copied()
            else {
                return self.tt.unknown;
            };
            // Mark in progress to cut cycles.
            self.alias_tys.insert(def, (Vec::new(), self.tt.unknown));
            let gens = self.generics_of(file, generics);
            let t = self.with_ctx(file, scope, None, |c| c.lower_type(ty));
            self.alias_tys.insert(def, (gens, t));
        }
        let (gens, t) = self.alias_tys[&def].clone();
        let map: HashMap<DefId, Ty> = gens.iter().copied().zip(args.iter().copied()).collect();
        self.tt.subst(t, &map)
    }

    fn builtin_type(&mut self, name: &str, args: &'p [GenericArg], span: Span) -> Ty {
        let lowered = self.lower_generic_args(args);
        let want = |c: &mut Self, n: usize| -> bool {
            if lowered.len() == n {
                true
            } else {
                c.err(
                    span,
                    format!(
                        "`{name}` takes {n} type argument{}",
                        if n == 1 { "" } else { "s" }
                    ),
                );
                false
            }
        };
        let tt = &mut self.tt;
        match name {
            "int" | "int64" => tt.int,
            "int8" => tt.mk(TyKind::Int(IntKind::I8)),
            "int16" => tt.mk(TyKind::Int(IntKind::I16)),
            "int32" => tt.mk(TyKind::Int(IntKind::I32)),
            "uint" | "uint64" => tt.mk(TyKind::Int(IntKind::U64)),
            "uint8" | "byte" => tt.mk(TyKind::Int(IntKind::U8)),
            "uint16" => tt.mk(TyKind::Int(IntKind::U16)),
            "uint32" => tt.mk(TyKind::Int(IntKind::U32)),
            "float" | "float64" => tt.float,
            "float32" => tt.mk(TyKind::Float(FloatKind::F32)),
            "bool" => tt.bool_,
            "char" => tt.char_,
            "string" => tt.string,
            "str" => tt.str_,
            "void" => tt.void,
            "never" => tt.never,
            "error" => tt.error,
            "list" if want(self, 1) => self.tt.list(lowered[0]),
            "set" if want(self, 1) => self.tt.mk(TyKind::Set(lowered[0])),
            "range" if want(self, 1) => self.tt.mk(TyKind::Range(lowered[0])),
            "iter" if want(self, 1) => self.tt.mk(TyKind::Iter(lowered[0])),
            "box" if want(self, 1) => self.tt.mk(TyKind::Box(lowered[0])),
            "option" if want(self, 1) => self.tt.option(lowered[0]),
            "map" if want(self, 2) => self.tt.mk(TyKind::Map(lowered[0], lowered[1])),
            "result" if want(self, 2) => self.tt.result(lowered[0], lowered[1]),
            _ => self.tt.unknown,
        }
    }

    // ---- Phase 3: driver -----------------------------------------------------------------------------

    fn check_all(&mut self) {
        // Every function with a body, in a stable order.
        let mut defs: Vec<DefId> = self.nodes.keys().copied().collect();
        defs.sort();
        for d in defs {
            self.queue.push(d);
        }
        let tops = std::mem::take(&mut self.top_stmts);
        let tests = std::mem::take(&mut self.tests);
        // Process the queue until empty; checking a body can queue local functions.
        while let Some(d) = self.queue.pop() {
            if self.done.contains(&d) {
                continue;
            }
            self.done.push(d);
            match self.nodes.get(&d).copied() {
                Some(Node::Fn { .. }) => self.check_fn_body(d),
                Some(Node::Global { .. }) => {
                    self.global_ty(d);
                }
                Some(Node::Adt { .. }) => self.check_adt_defaults(d),
                _ => {}
            }
        }
        for (file, scope, stmts) in tops {
            self.check_top_statements(file, scope, stmts);
        }
        for (file, scope, body) in tests {
            self.check_test(file, scope, body);
        }
        // Local items found while checking.
        while let Some(d) = self.queue.pop() {
            if self.done.contains(&d) {
                continue;
            }
            self.done.push(d);
            if let Some(Node::Fn { .. }) = self.nodes.get(&d).copied() {
                self.check_fn_body(d);
            }
        }
    }

    /// Field defaults: `pub port: int = 8080`.
    fn check_adt_defaults(&mut self, def: DefId) {
        let Some(Node::Adt {
            file, scope, decl, ..
        }) = self.nodes.get(&def).copied()
        else {
            return;
        };
        let AdtDecl::Type(d) = decl else { return };
        let self_ty = self.own_adt_ty(def);
        let start = self.begin_unit(file, scope, Some(self_ty));
        for m in &d.members {
            if let Member::Field(f) = m {
                if let Some(default) = &f.default {
                    let name = self.text(&f.name);
                    if let Some(fd) = self.res.member(def, name) {
                        let ft = self.field_ty(fd);
                        self.check_coerce(default, ft);
                    }
                }
            }
        }
        self.end_unit(start);
    }

    /// Starts checking one unit of work (a function body, a constant, a test).
    pub(crate) fn begin_unit(
        &mut self,
        file: ModuleId,
        scope: usize,
        self_ty: Option<Ty>,
    ) -> UnitState {
        let saved = UnitState {
            file: self.file,
            scope: self.scope,
            fn_ctx: std::mem::take(&mut self.fn_ctx),
            loops: std::mem::take(&mut self.loops),
            floor: self.var_floor,
            self_depth: self.self_ty.len(),
        };
        self.file = file;
        self.scope = scope;
        self.self_ty.push(self_ty);
        self.var_floor = self.tt.var_count();
        saved
    }

    /// Finishes a unit: default literal types, report what could not be inferred, check literal
    /// ranges, and restore the previous state.
    pub(crate) fn end_unit(&mut self, saved: UnitState) {
        self.tt.default_literals();
        for (_, span) in self.tt.unsolved(self.var_floor) {
            self.err(span, "cannot work out the type here; add a type annotation");
        }
        self.flush_bounds();
        self.flush_matches();
        self.flush_sinks();
        let lits = std::mem::take(&mut self.lits);
        for l in lits {
            if let Some(k) = self.tt.int_kind(l.ty) {
                if !k.fits(l.magnitude, l.negative) {
                    let text = if l.negative {
                        format!("-{}", l.magnitude)
                    } else {
                        l.magnitude.to_string()
                    };
                    let saved_file = self.file;
                    self.file = l.file;
                    self.err(l.span, format!("{text} does not fit in `{}`", k.name()));
                    self.file = saved_file;
                }
            }
        }
        self.self_ty.truncate(saved.self_depth);
        self.file = saved.file;
        self.scope = saved.scope;
        self.fn_ctx = saved.fn_ctx;
        self.loops = saved.loops;
        self.var_floor = saved.floor;
    }

    fn check_fn_body(&mut self, def: DefId) {
        let Some(Node::Fn {
            file,
            scope,
            f,
            owner,
            ..
        }) = self.nodes.get(&def).copied()
        else {
            return;
        };
        if matches!(f.body, FnBody::None) {
            return;
        }
        let Some(sig) = self.fn_sig(def) else { return };
        let self_ty = self.owner_self_ty(owner);
        let st = self.begin_unit(file, scope, self_ty);
        // `init` and `deinit` have an implicit `self`.
        if f.kind != FnKind::Fn {
            if let Some(sd) = self.declared(&f.name) {
                self.local_tys
                    .insert(sd, self_ty.unwrap_or(self.tt.unknown));
            }
        }
        // Give the parameters their types.
        for p in &f.params {
            let Some(pd) = self.declared(&p.name) else {
                continue;
            };
            if p.is_self {
                let t = self_ty.unwrap_or(self.tt.unknown);
                self.local_tys.insert(pd, t);
            } else if let Some(ps) = sig.params.iter().find(|s| s.name == self.text(&p.name)) {
                let t = if ps.variadic {
                    self.tt.list(ps.ty)
                } else {
                    ps.ty
                };
                self.local_tys.insert(pd, t);
            }
        }
        // Defaults are checked against their parameter types.
        for p in &f.params {
            if let Some(d) = &p.default {
                if let Some(ps) = sig.params.iter().find(|s| s.name == self.text(&p.name)) {
                    let t = ps.ty;
                    self.check_coerce(d, t);
                }
            }
        }
        self.fn_ctx.push(FnCtx { ret: sig.ret });
        // An `init` ends by falling off the end (success) or by `return err(...)`; a function
        // returning `void ! E` may do the same.
        let void_ok = matches!(
            self.tt.kind_of(sig.ret),
            TyKind::Result(ok, _) if self.tt.kind_of(*ok) == &TyKind::Void
        );
        let exp = if self.tt.kind_of(sig.ret) == &TyKind::Void || f.kind == FnKind::Init || void_ok
        {
            Exp::Discard
        } else {
            Exp::Is(sig.ret)
        };
        let span;
        let found = match &f.body {
            FnBody::Block(b) => {
                span = b.span;
                self.check_block(b, exp)
            }
            FnBody::Expr(e) => {
                span = e.span;
                self.check_expr(e, exp)
            }
            FnBody::None => return,
        };
        if let Exp::Is(t) = exp {
            self.coerce_at(found, t, span, "this function's value");
        } else if void_ok {
            // Falling off the end is `ok`; a tail value must still be a `void ! E`.
            if !matches!(self.tt.kind_of(found), TyKind::Void | TyKind::Never) {
                self.coerce_at(found, sig.ret, span, "this function's value");
            }
        }
        self.fn_ctx.pop();
        self.end_unit(st);
    }

    fn check_test(&mut self, file: ModuleId, scope: usize, body: &'p Block) {
        let st = self.begin_unit(file, scope, None);
        self.fn_ctx.push(FnCtx { ret: self.tt.void });
        self.check_block(body, Exp::Discard);
        self.fn_ctx.pop();
        self.end_unit(st);
    }

    /// Top-level statements of a module (scripts like the tour) run as one pseudo-function.
    fn check_top_statements(&mut self, file: ModuleId, scope: usize, stmts: &'p [Stmt]) {
        let has_exec = stmts.iter().any(|s| matches!(s.kind, StmtKind::Expr(_)));
        if !has_exec {
            return;
        }
        let st = self.begin_unit(file, scope, None);
        self.fn_ctx.push(FnCtx { ret: self.tt.void });
        for s in stmts {
            if let StmtKind::Expr(e) = &s.kind {
                self.check_expr(e, Exp::Discard);
            }
        }
        self.fn_ctx.pop();
        self.end_unit(st);
    }

    /// The type of a module-level constant or variable (checking its value if it has no annotation).
    pub(crate) fn global_ty(&mut self, def: DefId) -> Ty {
        if let Some(&t) = self.local_tys.get(&def) {
            return t;
        }
        let Some(Node::Global { file, scope, decl }) = self.nodes.get(&def).copied() else {
            return self.tt.unknown;
        };
        if self.global_busy.contains(&def) {
            let span = self.def(def).span;
            let saved = self.file;
            self.file = file;
            self.err(
                span,
                "the type of this depends on itself; add a type annotation",
            );
            self.file = saved;
            return self.tt.unknown;
        }
        self.global_busy.push(def);
        let st = self.begin_unit(file, scope, None);
        self.fn_ctx.push(FnCtx { ret: self.tt.void });
        let declared = decl.ty.as_ref().map(|t| self.lower_type(t));
        let ty = match (&decl.init, declared) {
            (Some(init), Some(t)) => {
                self.check_coerce(init, t);
                t
            }
            (Some(init), None) => self.check_expr(init, Exp::Any),
            (None, Some(t)) => t,
            (None, None) => self.tt.unknown,
        };
        // Bind every name of the pattern (usually one).
        self.bind_decl_pattern(&decl.pattern, ty);
        self.fn_ctx.pop();
        self.end_unit(st);
        self.global_busy.pop();
        self.local_tys.get(&def).copied().unwrap_or(ty)
    }
}

/// What to put back after a unit of work.
pub(crate) struct UnitState {
    file: ModuleId,
    scope: usize,
    fn_ctx: Vec<FnCtx>,
    loops: Vec<LoopCtx>,
    floor: u32,
    self_depth: usize,
}

/// Does a value of this type have to be consumed exactly once?
pub(crate) fn linear_in(tt: &TypeTable, linear: &std::collections::HashSet<DefId>, t: Ty) -> bool {
    let t = tt.shallow(t);
    match tt.kind(t) {
        TyKind::Param(g) => linear.contains(g),
        TyKind::Adt(d, args) => {
            linear.contains(d) || args.iter().any(|a| linear_in(tt, linear, *a))
        }
        TyKind::List(e)
        | TyKind::Array(e, _)
        | TyKind::Option(e)
        | TyKind::Box(e)
        | TyKind::Set(e) => linear_in(tt, linear, *e),
        TyKind::Tuple(ts) | TyKind::AltTuple(ts, _) => ts.iter().any(|x| linear_in(tt, linear, *x)),
        TyKind::Result(a, b) | TyKind::Map(a, b) => {
            linear_in(tt, linear, *a) || linear_in(tt, linear, *b)
        }
        _ => false,
    }
}

/// Is this a view (`str`, a function value) or does it hold one? Views cannot be stored.
pub(crate) fn holds_view(tt: &TypeTable, t: Ty) -> bool {
    let t = tt.shallow(t);
    match tt.kind(t) {
        TyKind::Str | TyKind::Fn(..) => true,
        TyKind::List(e)
        | TyKind::Array(e, _)
        | TyKind::Option(e)
        | TyKind::Set(e)
        | TyKind::Range(e) => holds_view(tt, *e),
        TyKind::Tuple(ts) | TyKind::AltTuple(ts, _) => ts.iter().any(|x| holds_view(tt, *x)),
        TyKind::Result(a, b) | TyKind::Map(a, b) => holds_view(tt, *a) || holds_view(tt, *b),
        _ => false,
    }
}

/// A literal's value as written, for range checks.
pub(crate) fn literal_value(text: &str) -> Option<u128> {
    int_text(text)
}
