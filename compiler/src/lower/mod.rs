//! Lowering the checked program to the IR (`docs/design/ir.md`).
//!
//! The lowering works function by function, starting from the functions that have no type parameters
//! and from `main`; every call to a generic function (or a method of a generic type) asks for an
//! **instance** with concrete type arguments, which is lowered in turn (monomorphisation). Library code
//! is lowered the same way, on demand.
//!
//! Values: scalars (numbers, `bool`, `char`, addresses) live in SSA values; everything else lives in
//! memory and is handled by address (see `ty.rs`). A function that returns a memory value is given the
//! address to write it to as a hidden first parameter. Owning values are dropped at the end of their
//! scope, with a one-byte flag per variable that records whether it still owns its value.
//!
//! Anything not handled yet is reported as a note ("not lowered yet") and its function becomes a stub
//! that stops, so the module still verifies and runs.

mod agg;
mod any;
mod builder;
mod call;
mod closure;
mod cmp;
mod coll;
mod ctl;
mod expr;
mod func;
mod glue;
mod intr;
mod meta;
mod obj;
mod ops;
mod pat;
mod select;
mod seq;
mod soa;
mod ty;

use crate::ast::*;
use crate::ir::{
    BinOp, BlockId, CastKind, CmpOp, Const, FUn, Func, FuncId, IrTy, Module, Op, Target, Term,
    Value,
};
use crate::package::{Diagnostic, ModuleId, Program};
use crate::resolve::{DefId, DefKind, Resolved};
use crate::span::Span;
use crate::token::Punct;
use crate::typecheck::{FnInfo, Typed};
use crate::types::{FloatKind, Ty, TyKind, TypeTable};
use std::collections::HashMap;
use std::rc::Rc;

pub(crate) use expr::LoopBody;
pub(crate) use func::*;
pub(crate) use ty::*;

#[cfg(test)]
mod tests;

pub struct Lowered {
    pub module: Module,
    /// Functions that could not be lowered yet, and why.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum GlueKind {
    Drop,
    Clone,
    Eq,
    Cmp,
    Hash,
    Show,
    /// The function in an object's table that drops it: takes the object's address.
    ObjDrop,
    /// The function in an object's table that copies it: the object's address, then where to put the copy.
    ObjClone,
    /// Drops a heap copy of a value and frees it (the first slot of an `any` table).
    BoxDrop,
}

pub(crate) enum Job<'a> {
    Fn {
        id: FuncId,
        def: DefId,
        subst: HashMap<DefId, Ty>,
    },
    Glue {
        id: FuncId,
        kind: GlueKind,
        ty: Ty,
    },
    Closure(closure::ClosureJob<'a>),
    Meta(meta::MetaJob<'a>),
    EnvDrop {
        id: FuncId,
        owned: Vec<(u32, Ty)>,
    },
}

/// How a parameter is passed in the IR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pass {
    /// Nothing is passed (the type has no value).
    Skip,
    Val(IrTy),
    /// The address of the value.
    Ptr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetPass {
    Void,
    Val(IrTy),
    /// The caller passes the address to write the result to.
    Sret,
}

#[derive(Clone, Debug)]
pub(crate) struct Sig {
    /// Per source parameter (the receiver first): how it is passed, its type and mode.
    pub params: Vec<(Pass, Ty, Option<Mode>)>,
    pub ret: Ty,
    pub ret_pass: RetPass,
    pub ir_params: Vec<IrTy>,
    pub ir_ret: IrTy,
}

pub(crate) struct FnEntry<'a> {
    pub file: ModuleId,
    pub decl: &'a FnDecl,
    pub mods: &'a Modifiers,
}

/// A module-level constant or variable: its initializer is evaluated where it is used.
pub(crate) struct GlobalEntry<'a> {
    pub file: ModuleId,
    pub init: &'a Expr,
}

pub(crate) struct Lowerer<'a> {
    pub program: &'a Program,
    pub res: &'a Resolved,
    pub typed: &'a Typed,
    pub types: TypeTable,
    pub module: Module,
    pub diags: Vec<Diagnostic>,
    pub fns: HashMap<DefId, FnEntry<'a>>,
    pub globals: HashMap<DefId, GlobalEntry<'a>>,
    pub core_fns: HashMap<String, DefId>,
    pub field_defaults: HashMap<DefId, (ModuleId, &'a Expr)>,
    pub instances: HashMap<(DefId, Vec<(DefId, Ty)>), FuncId>,
    pub jobs: Vec<Job<'a>>,
    pub global_data: HashMap<DefId, u32>,
    pub thunks: Vec<closure::ThunkJob>,
    pub names: HashMap<String, u32>,
    pub layouts: HashMap<Ty, Layout>,
    pub fields_cache: HashMap<Ty, Vec<Field>>,
    pub enums: HashMap<Ty, Rc<EnumInfo>>,
    pub drop_cache: HashMap<Ty, bool>,
    pub glue: HashMap<(GlueKind, Ty), Option<FuncId>>,
    pub obj_ids: HashMap<Ty, i64>,
    pub vtables: HashMap<Ty, u32>,
    pub any_tables: HashMap<(Ty, DefId), u32>,
    pub any_thunks: Vec<any::AnyThunkJob>,
    pub f: FnState<'a>,
    /// Compile-time results already computed, and the constants being computed now.
    pub meta_cache: HashMap<(ModuleId, u32, u32), Rc<meta::Cv>>,
    pub meta_stack: Vec<(ModuleId, u32, u32)>,
    /// `@meta_limit(n)`: the item it is written on, and the step budget.
    pub meta_limits: Vec<(ModuleId, Span, u64)>,
}

pub fn lower(program: &Program, res: &Resolved, typed: &Typed) -> Lowered {
    let types = typed.types.clone();
    let void = types.void;
    let mut lw = Lowerer {
        program,
        res,
        typed,
        types,
        module: Module::default(),
        diags: Vec::new(),
        fns: HashMap::new(),
        globals: HashMap::new(),
        core_fns: HashMap::new(),
        field_defaults: HashMap::new(),
        instances: HashMap::new(),
        jobs: Vec::new(),
        global_data: HashMap::new(),
        thunks: Vec::new(),
        names: HashMap::new(),
        layouts: HashMap::new(),
        fields_cache: HashMap::new(),
        enums: HashMap::new(),
        drop_cache: HashMap::new(),
        glue: HashMap::new(),
        obj_ids: HashMap::new(),
        vtables: HashMap::new(),
        any_tables: HashMap::new(),
        any_thunks: Vec::new(),
        f: FnState::empty(void),
        meta_cache: HashMap::new(),
        meta_stack: Vec::new(),
        meta_limits: Vec::new(),
    };
    lw.index();
    // Roots: every function of the program that needs no type arguments.
    let mut roots: Vec<DefId> = lw
        .fns
        .iter()
        .filter(|(d, e)| {
            let in_core = program.package(program.module(e.file).package).name == "core";
            !in_core
                && !e.mods.is_meta
                && !matches!(e.decl.body, FnBody::None)
                && lw.is_plain_fn(**d)
        })
        .map(|(d, _)| *d)
        .collect();
    roots.sort();
    for d in roots {
        lw.request_fn(d, &HashMap::new());
    }
    lw.run_jobs();
    Lowered {
        module: lw.module,
        diagnostics: lw.diags,
    }
}

/// Does the body use `become`?
fn has_become(decl: &FnDecl) -> bool {
    struct Find(bool);
    impl crate::visit::Visitor for Find {
        fn visit_expr(&mut self, e: &Expr) {
            if matches!(e.kind, ExprKind::Become(_)) {
                self.0 = true;
            }
            crate::visit::walk_expr(self, e);
        }
    }
    let mut f = Find(false);
    match &decl.body {
        FnBody::Block(b) => crate::visit::walk_block(&mut f, b),
        FnBody::Expr(e) => crate::visit::Visitor::visit_expr(&mut f, e),
        FnBody::None => {}
    }
    f.0
}

impl<'a> Lowerer<'a> {
    // ---- Index --------------------------------------------------------------------------------------

    fn index(&mut self) {
        let program = self.program;
        for m in &program.modules {
            self.index_stmts(m.id, &m.file.stmts);
        }
    }

    pub(crate) fn index_stmts(&mut self, file: ModuleId, stmts: &'a [Stmt]) {
        for s in stmts {
            match &s.kind {
                StmtKind::Item(it) => self.index_item(file, it),
                StmtKind::Decl(d) => {
                    if let (PatternKind::Binding { name, .. }, Some(init)) =
                        (&d.pattern.kind, &d.init)
                    {
                        if let Some(&def) = self.res.decls.get(&(file, name.span.start)) {
                            self.globals.insert(def, GlobalEntry { file, init });
                        }
                    }
                }
                StmtKind::Expr(e) => self.index_meta_expr(file, e),
            }
        }
    }

    /// Declarations inside a module-level `meta if`: only the branch the target selects.
    fn index_meta_expr(&mut self, file: ModuleId, e: &'a Expr) {
        match &e.kind {
            ExprKind::MetaIf(i) => {
                let src = &self.program.module(file).src;
                if crate::meta::chooses_then(i, src, &self.program.target) {
                    self.index_stmts(file, &i.then.stmts);
                } else if let Some(el) = &i.else_ {
                    self.index_meta_expr(file, el);
                }
            }
            ExprKind::Block(b) => self.index_stmts(file, &b.stmts),
            _ => {}
        }
    }

    pub(crate) fn index_item(&mut self, file: ModuleId, item: &'a Item) {
        for a in &item.attrs {
            let src = &self.program.module(file).src;
            if a.name.text(src) == "meta_limit" {
                if let Some(arg) = a.args.first() {
                    let text = src[arg.span.start as usize..arg.span.end as usize].replace('_', "");
                    if let Ok(n) = text.parse::<u64>() {
                        self.meta_limits.push((file, item.span, n));
                    }
                }
            }
        }
        match &item.kind {
            ItemKind::Fn(f) => {
                if let Some(&d) = self.res.item_defs.get(&(file, item.span.start)) {
                    self.fns.insert(
                        d,
                        FnEntry {
                            file,
                            decl: f,
                            mods: &item.mods,
                        },
                    );
                }
            }
            ItemKind::Struct(d) | ItemKind::Obj(d) | ItemKind::Interface(d) => {
                let owner = self.res.item_defs.get(&(file, item.span.start)).copied();
                for m in &d.members {
                    match m {
                        Member::Item(i) => self.index_item(file, i),
                        Member::Field(f) => {
                            if let (Some(owner), Some(default)) = (owner, &f.default) {
                                let name = &self.program.module(file).src
                                    [f.name.span.start as usize..f.name.span.end as usize];
                                if let Some(fd) = self.res.member(owner, name) {
                                    self.field_defaults.insert(fd, (file, default));
                                }
                            }
                        }
                    }
                }
            }
            ItemKind::Decl(d) => {
                if let (PatternKind::Binding { name, .. }, Some(init)) = (&d.pattern.kind, &d.init)
                {
                    if let Some(&def) = self.res.decls.get(&(file, name.span.start)) {
                        self.globals.insert(def, GlobalEntry { file, init });
                    }
                }
            }
            ItemKind::Enum(e) => {
                for i in &e.items {
                    self.index_item(file, i);
                }
            }
            ItemKind::Impl(im) => {
                for m in &im.members {
                    if let Member::Item(i) = m {
                        self.index_item(file, i);
                    }
                }
            }
            ItemKind::Mod { body: Some(b), .. } => self.index_stmts(file, b),
            _ => {}
        }
    }

    /// A function that can be lowered without being told type arguments.
    fn is_plain_fn(&self, def: DefId) -> bool {
        let Some(info) = self.typed.fn_infos.get(&def) else {
            return false;
        };
        if !info.generics.is_empty() {
            return false;
        }
        match info.self_ty {
            Some(t) => !self.has_params(t),
            None => true,
        }
    }

    fn has_params(&self, t: Ty) -> bool {
        match self.types.kind_of(t) {
            TyKind::Param(_) => true,
            TyKind::Adt(_, args) | TyKind::Tuple(args) => args.iter().any(|a| self.has_params(*a)),
            TyKind::List(e)
            | TyKind::Option(e)
            | TyKind::Set(e)
            | TyKind::Box(e)
            | TyKind::Range(e)
            | TyKind::Iter(e) => self.has_params(*e),
            TyKind::Result(a, b) | TyKind::Map(a, b) => self.has_params(*a) || self.has_params(*b),
            TyKind::Array(e, _) => self.has_params(*e),
            TyKind::Fn(ps, r, _) => ps.iter().any(|p| self.has_params(p.ty)) || self.has_params(*r),
            _ => false,
        }
    }

    // ---- Instances --------------------------------------------------------------------------------------

    fn unique_name(&mut self, base: String) -> String {
        let n = self.names.entry(base.clone()).or_insert(0);
        let name = if *n == 0 {
            base.clone()
        } else {
            format!("{base}#{n}")
        };
        *n += 1;
        name
    }

    /// The type with these type arguments substituted.
    pub(crate) fn conc_with(&mut self, t: Ty, map: &HashMap<DefId, Ty>) -> Ty {
        let t = if map.is_empty() {
            t
        } else {
            self.types.subst(t, map)
        };
        self.types.zonk(t)
    }

    /// The IR signature of a function for these type arguments.
    /// The signature `init` and `deinit` really have: they work on an implicit `self`.
    pub(crate) fn fn_info_for(&mut self, def: DefId, info: &FnInfo) -> FnInfo {
        let kind = self
            .fns
            .get(&def)
            .map(|e| e.decl.kind)
            .unwrap_or(FnKind::Fn);
        let mut i = info.clone();
        if kind == FnKind::Fn {
            return i;
        }
        let is_obj = info.self_ty.is_some_and(|t| self.obj_def_raw(t).is_some());
        match kind {
            FnKind::Deinit => {
                i.has_self = true;
                i.self_mode = if is_obj { None } else { Some(Mode::Inout) };
            }
            FnKind::Init => {
                if is_obj {
                    i.has_self = true;
                    i.self_mode = None;
                } else if matches!(self.types.kind_of(info.ret), TyKind::Void) {
                    if let Some(t) = info.self_ty {
                        i.ret = t;
                    }
                }
            }
            FnKind::Fn => {}
        }
        i
    }

    pub(crate) fn obj_def_raw(&self, t: Ty) -> Option<DefId> {
        match self.types.kind_of(t) {
            TyKind::Adt(d, _) if self.res.def(*d).kind == DefKind::Obj => Some(*d),
            _ => None,
        }
    }

    pub(crate) fn sig_of(&mut self, info: &FnInfo, map: &HashMap<DefId, Ty>) -> Sig {
        let mut params: Vec<(Pass, Ty, Option<Mode>)> = Vec::new();
        let mut srcs: Vec<(Ty, Option<Mode>)> = Vec::new();
        if info.has_self {
            if let Some(st) = info.self_ty {
                srcs.push((st, info.self_mode));
            }
        }
        for p in &info.params {
            let t = if p.variadic {
                self.types.list(p.ty)
            } else {
                p.ty
            };
            srcs.push((t, p.mode));
        }
        let mut ir_params = Vec::new();
        let ret = self.conc_with(info.ret, map);
        let ret_pass = match self.repr(ret) {
            Repr::Zst => RetPass::Void,
            Repr::Scalar(t) => RetPass::Val(t),
            Repr::Mem(_) => RetPass::Sret,
        };
        if ret_pass == RetPass::Sret {
            ir_params.push(IrTy::Ptr);
        }
        for (t, mode) in srcs {
            let t = self.conc_with(t, map);
            let pass = self.pass_of(t, mode);
            match pass {
                Pass::Skip => {}
                Pass::Val(x) => ir_params.push(x),
                Pass::Ptr => ir_params.push(IrTy::Ptr),
            }
            params.push((pass, t, mode));
        }
        let ir_ret = match ret_pass {
            RetPass::Val(t) => t,
            _ => IrTy::Void,
        };
        Sig {
            params,
            ret,
            ret_pass,
            ir_params,
            ir_ret,
        }
    }

    pub(crate) fn pass_of(&mut self, t: Ty, mode: Option<Mode>) -> Pass {
        match self.repr(t) {
            Repr::Zst => Pass::Skip,
            Repr::Scalar(s) => {
                if mode == Some(Mode::Inout) {
                    Pass::Ptr
                } else {
                    Pass::Val(s)
                }
            }
            Repr::Mem(_) => Pass::Ptr,
        }
    }

    fn fn_display_name(&mut self, def: DefId, map: &HashMap<DefId, Ty>) -> String {
        let name = self.res.def(def).name.clone();
        let mut base = match self.typed.fn_infos.get(&def).and_then(|i| i.self_ty) {
            Some(st) => {
                let st = self.conc_with(st, map);
                format!("{}.{name}", self.show_ty(st))
            }
            None => name,
        };
        let gens = self
            .typed
            .fn_infos
            .get(&def)
            .map(|i| i.generics.clone())
            .unwrap_or_default();
        if !gens.is_empty() {
            let args: Vec<String> = gens
                .iter()
                .map(|g| match map.get(g) {
                    Some(t) => {
                        let t = self.conc_with(*t, &HashMap::new());
                        self.show_ty(t)
                    }
                    None => "?".to_string(),
                })
                .collect();
            base = format!("{base}<{}>", args.join(","));
        }
        base
    }

    pub(crate) fn show_ty(&self, t: Ty) -> String {
        self.types.display(t, &|d| self.res.def(d).name.clone())
    }

    /// The function for `def` with these type arguments; lowered later from the job list.
    pub(crate) fn request_fn(&mut self, def: DefId, map: &HashMap<DefId, Ty>) -> Option<FuncId> {
        let mut key_subst: Vec<(DefId, Ty)> = Vec::new();
        for (k, v) in map {
            let v = self.types.zonk(*v);
            key_subst.push((*k, v));
        }
        key_subst.sort_by_key(|(k, _)| *k);
        let key = (def, key_subst.clone());
        if let Some(id) = self.instances.get(&key) {
            return Some(*id);
        }
        let info = self.typed.fn_infos.get(&def)?.clone();
        let info = self.fn_info_for(def, &info);
        let concrete: HashMap<DefId, Ty> = key_subst.into_iter().collect();
        let sig = self.sig_of(&info, &concrete);
        let base = self.fn_display_name(def, &concrete);
        let name = self.unique_name(base);
        let mut func = Func::new(&name, &sig.ir_params, sig.ir_ret);
        let e = func.entry();
        func.terminate(e, Term::Stop("not lowered yet".to_string()));
        let id = self.module.add(func);
        self.instances.insert(key, id);
        self.jobs.push(Job::Fn {
            id,
            def,
            subst: concrete,
        });
        Some(id)
    }

    // ---- Running the jobs ------------------------------------------------------------------------------------

    fn run_jobs(&mut self) {
        while let Some(job) = self.jobs.pop() {
            match job {
                Job::Fn { id, def, subst } => self.lower_fn_job(id, def, subst),
                Job::Glue { id, kind, ty } => self.lower_glue_job(id, kind, ty),
                Job::Closure(j) => self.lower_closure_job(j),
                Job::Meta(j) => self.lower_meta_job(j),
                Job::EnvDrop { id, owned } => self.lower_env_drop(id, owned),
            }
            while let Some(t) = self.thunks.pop() {
                self.lower_thunk(t);
            }
            while let Some(t) = self.any_thunks.pop() {
                self.lower_any_thunk(t);
            }
        }
    }

    fn lower_fn_job(&mut self, id: FuncId, def: DefId, subst: HashMap<DefId, Ty>) {
        let Some(info) = self.typed.fn_infos.get(&def).cloned() else {
            return;
        };
        let info = self.fn_info_for(def, &info);
        let Some(entry) = self.fns.get(&def) else {
            return;
        };
        let (file, decl) = (entry.file, entry.decl);
        if matches!(decl.body, FnBody::None) {
            // An `ext` function: the compiler supplies it.
            self.lower_intrinsic_job(id, def, &subst);
            return;
        }
        let sig = self.sig_of(&info, &subst);
        let name = self.module.funcs[id.0 as usize].name.clone();
        let void = self.types.void;
        let mut state = FnState::empty(void);
        state.func = Func::new(&name, &sig.ir_params, sig.ir_ret);
        state.file = file;
        state.cur = Some(state.func.entry());
        state.subst = subst;
        state.ret_ty = sig.ret;
        state.cur_def = Some(def);
        self.f = state;
        let r = self.lower_body(decl, &info, &sig);
        let st = std::mem::replace(&mut self.f, FnState::empty(void));
        match (r, st.fail) {
            (Ok(()), None) => {
                let mut func = st.func;
                func.compact();
                self.module.funcs[id.0 as usize] = func;
            }
            (_, fail) => {
                let (span, msg) =
                    fail.unwrap_or((decl.name.span, "an internal problem".to_string()));
                self.diags.push(Diagnostic {
                    module: file,
                    span,
                    message: format!("not lowered yet: {msg} (in `{name}`)"),
                    warning: true,
                });
            }
        }
    }

    fn lower_body(&mut self, decl: &'a FnDecl, info: &FnInfo, sig: &Sig) -> Result<(), ()> {
        let entry = self.f.func.entry();
        let ir_params = self.f.func.block_params(entry).to_vec();
        let mut next = 0usize;
        if sig.ret_pass == RetPass::Sret {
            self.f.sret = Some(ir_params[0]);
            next = 1;
        }
        // A function that ends in `become` on itself, with plain number parameters, loops instead.
        let mut head_params: Option<Vec<Value>> = None;
        if has_become(decl)
            && sig.params.iter().all(|(p, t, m)| {
                matches!(p, Pass::Val(_)) && *m != Some(Mode::Sink) && !self.needs_drop(*t)
            })
        {
            let tys: Vec<IrTy> = sig.ir_params[next..].to_vec();
            let head = self.new_block(&tys);
            let args: Vec<Value> = ir_params[next..].to_vec();
            self.jump(head, args);
            self.start(head);
            self.f.tail_head = Some(head);
            head_params = Some(self.f.func.block_params(head).to_vec());
        }
        let ir_params: Vec<Value> = match head_params {
            Some(hp) => {
                let mut v = ir_params[..next].to_vec();
                v.extend(hp);
                v
            }
            None => ir_params,
        };
        self.push_scope();
        // The program's `main`: tasks nobody waited for run before it ends.
        let in_core = self
            .program
            .package(self.program.module(self.f.file).package)
            .name
            == "core";
        if !in_core
            && decl.kind == FnKind::Fn
            && info.self_ty.is_none()
            && decl.params.is_empty()
            && self.src_text(decl.name.span) == "main"
        {
            if let Some(s) = self.f.scopes.last_mut() {
                s.push(ScopeItem::CoreCall("rt_run_pending"));
            }
        }
        // The parameters: the receiver, then the declared ones.
        let file = self.f.file;
        let mut decl_params: Vec<Option<DefId>> = Vec::new();
        if info.has_self {
            let mut sd = None;
            for p in &decl.params {
                if p.is_self {
                    sd = self.res.decls.get(&(file, p.name.span.start)).copied();
                }
            }
            if decl.kind != FnKind::Fn {
                sd = self.res.decls.get(&(file, decl.name.span.start)).copied();
            }
            decl_params.push(sd);
        }
        for p in decl.params.iter().filter(|p| !p.is_self) {
            decl_params.push(self.res.decls.get(&(file, p.name.span.start)).copied());
        }
        for (i, (pass, ty, mode)) in sig.params.iter().enumerate() {
            let (pass, ty, mode) = (*pass, *ty, *mode);
            let def = decl_params.get(i).copied().flatten();
            match pass {
                Pass::Skip => {}
                Pass::Val(_) => {
                    let v = ir_params[next];
                    next += 1;
                    let Some(def) = def else { continue };
                    if mode == Some(Mode::Sink) && self.needs_drop(ty) {
                        let slot = self.slot_for(ty);
                        self.store_scalar_ty(slot, v);
                        let flag = self.own(slot, ty, true);
                        self.f.locals.insert(
                            def,
                            Local::Slot {
                                ptr: slot,
                                ty,
                                flag,
                            },
                        );
                    } else {
                        self.f.locals.insert(def, Local::Val(v));
                    }
                }
                Pass::Ptr => {
                    let p = ir_params[next];
                    next += 1;
                    let Some(def) = def else { continue };
                    let flag = if mode == Some(Mode::Sink) {
                        self.own(p, ty, true)
                    } else {
                        None
                    };
                    self.f.locals.insert(def, Local::Slot { ptr: p, ty, flag });
                }
            }
        }
        // A struct's `init` builds its value in place (or in a temporary, when it can fail).
        let mut init_tmp: Option<(Value, Ty)> = None;
        if decl.kind == FnKind::Init && !info.has_self {
            if let Some(st) = info.self_ty {
                let st = self.conc(st);
                let sd = self.res.decls.get(&(file, decl.name.span.start)).copied();
                let ptr = if st == sig.ret {
                    self.f.sret.unwrap_or_else(|| self.slot_for(st))
                } else {
                    let t = self.slot_for(st);
                    init_tmp = Some((t, st));
                    t
                };
                self.fill_defaults(ptr, st);
                if let Some(sd) = sd {
                    self.f.locals.insert(
                        sd,
                        Local::Slot {
                            ptr,
                            ty: st,
                            flag: None,
                        },
                    );
                }
            }
        }
        // The body.
        let ret = sig.ret;
        let body_res = match &decl.body {
            FnBody::Block(b) => self.block_value(b, sig.ret_pass, ret),
            FnBody::Expr(e) => self.body_expr(e, sig.ret_pass, ret),
            FnBody::None => Res::Unit,
        };
        if self.f.fail.is_some() {
            return Err(());
        }
        if let Some((tmp, st)) = init_tmp {
            if self.f.cur.is_some() {
                self.write_ok_value(sig.ret, tmp, st);
            }
        }
        self.finish_return(body_res, sig, decl.name.span);
        if self.f.fail.is_some() {
            return Err(());
        }
        Ok(())
    }

    /// The end of a function: the scope of the parameters ends, then the value is returned.
    fn finish_return(&mut self, r: Res, sig: &Sig, span: Span) {
        if self.f.cur.is_none() {
            self.f.scopes.pop();
            return;
        }
        match (sig.ret_pass, r) {
            (RetPass::Val(_), Res::Val(v)) => {
                self.settle(v);
                self.pop_scope();
                self.terminate(Term::Return(Some(v)));
            }
            (RetPass::Void, _) => {
                self.pop_scope();
                self.terminate(Term::Return(None));
            }
            (RetPass::Sret, Res::Unit) | (RetPass::Sret, Res::Val(_)) => {
                // `void ! E` ends with success.
                self.write_ok_void(sig.ret);
                self.pop_scope();
                self.terminate(Term::Return(None));
            }
            (RetPass::Val(_), _) => {
                self.unsupported(span, "a function body without a value");
            }
            (_, Res::Never) => {}
        }
    }

    /// Writes `ok(value)` at the return address; the value moves in.
    fn write_ok_value(&mut self, ret: Ty, src: Value, st: Ty) {
        let (Some(sret), Some(info)) = (self.f.sret, self.enum_info(ret)) else {
            return;
        };
        let zero = self.konst(info.tag, Const::Int(0), Span::new(0, 0));
        self.store_at(sret, 0, zero);
        if let Some(f) = info.variants[0].first() {
            let p = self.addr(sret, f.off);
            self.copy_mem(p, src, st);
        }
    }

    /// Writes the success case of a `void ! E` result at the return address, if the body did not.
    fn write_ok_void(&mut self, ret: Ty) {
        if let (Some(sret), TyKind::Result(ok, _)) = (self.f.sret, self.kind(ret)) {
            if matches!(self.kind(ok), TyKind::Void) {
                if let Some(info) = self.enum_info(ret) {
                    let zero = self.konst(info.tag, Const::Int(0), Span::new(0, 0));
                    self.store_at(sret, 0, zero);
                }
            }
        }
    }
}
