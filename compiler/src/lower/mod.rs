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
mod call;
mod expr;
mod func;
mod glue;
mod pat;
mod stubs;
mod ty;

use crate::ast::*;
use crate::ir::{BinOp, BlockId, CastKind, CmpOp, Const, Func, FuncId, IrTy, Module, Op, Target, Term, Value};
use crate::package::{Diagnostic, ModuleId, Program};
use crate::resolve::{DefId, DefKind, Resolved};
use crate::span::Span;
use crate::token::Punct;
use crate::typecheck::{FnInfo, Typed};
use crate::types::{FloatKind, Ty, TyKind, TypeTable};
use std::collections::HashMap;
use std::rc::Rc;

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
}

pub(crate) enum Job {
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
    pub field_defaults: HashMap<DefId, (ModuleId, &'a Expr)>,
    pub instances: HashMap<(DefId, Vec<(DefId, Ty)>), FuncId>,
    pub jobs: Vec<Job>,
    pub names: HashMap<String, u32>,
    pub layouts: HashMap<Ty, Layout>,
    pub fields_cache: HashMap<Ty, Vec<Field>>,
    pub enums: HashMap<Ty, Rc<EnumInfo>>,
    pub drop_cache: HashMap<Ty, bool>,
    pub glue: HashMap<(GlueKind, Ty), Option<FuncId>>,
    pub f: FnState<'a>,
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
        field_defaults: HashMap::new(),
        instances: HashMap::new(),
        jobs: Vec::new(),
        names: HashMap::new(),
        layouts: HashMap::new(),
        fields_cache: HashMap::new(),
        enums: HashMap::new(),
        drop_cache: HashMap::new(),
        glue: HashMap::new(),
        f: FnState::empty(void),
    };
    lw.index();
    // Roots: every function of the program that needs no type arguments.
    let mut roots: Vec<DefId> = lw
        .fns
        .iter()
        .filter(|(d, e)| {
            let in_core = program.package(program.module(e.file).package).name == "core";
            !in_core && !matches!(e.decl.body, FnBody::None) && lw.is_plain_fn(**d)
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
                    if let (PatternKind::Binding { name, .. }, Some(init)) = (&d.pattern.kind, &d.init) {
                        if let Some(&def) = self.res.decls.get(&(file, name.span.start)) {
                            self.globals.insert(def, GlobalEntry { file, init });
                        }
                    }
                }
                StmtKind::Expr(_) => {}
            }
        }
    }

    pub(crate) fn index_item(&mut self, file: ModuleId, item: &'a Item) {
        match &item.kind {
            ItemKind::Fn(f) => {
                if let Some(&d) = self.res.item_defs.get(&(file, item.span.start)) {
                    self.fns.insert(d, FnEntry { file, decl: f });
                }
            }
            ItemKind::Struct(d) | ItemKind::Obj(d) | ItemKind::Interface(d) => {
                let owner = self.res.item_defs.get(&(file, item.span.start)).copied();
                for m in &d.members {
                    match m {
                        Member::Item(i) => self.index_item(file, i),
                        Member::Field(f) => {
                            if let (Some(owner), Some(default)) = (owner, &f.default) {
                                let name = &self.program.module(file).src[f.name.span.start as usize..f.name.span.end as usize];
                                if let Some(fd) = self.res.member(owner, name) {
                                    self.field_defaults.insert(fd, (file, default));
                                }
                            }
                        }
                    }
                }
            }
            ItemKind::Decl(d) => {
                if let (PatternKind::Binding { name, .. }, Some(init)) = (&d.pattern.kind, &d.init) {
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
            TyKind::List(e) | TyKind::Option(e) | TyKind::Set(e) | TyKind::Box(e) | TyKind::Range(e) | TyKind::Iter(e) => {
                self.has_params(*e)
            }
            TyKind::Result(a, b) | TyKind::Map(a, b) => self.has_params(*a) || self.has_params(*b),
            TyKind::Array(e, _) => self.has_params(*e),
            TyKind::Fn(ps, r) => ps.iter().any(|p| self.has_params(p.ty)) || self.has_params(*r),
            _ => false,
        }
    }

    // ---- Instances --------------------------------------------------------------------------------------

    fn unique_name(&mut self, base: String) -> String {
        let n = self.names.entry(base.clone()).or_insert(0);
        let name = if *n == 0 { base.clone() } else { format!("{base}#{n}") };
        *n += 1;
        name
    }

    /// The type with these type arguments substituted.
    pub(crate) fn conc_with(&mut self, t: Ty, map: &HashMap<DefId, Ty>) -> Ty {
        let t = if map.is_empty() { t } else { self.types.subst(t, map) };
        self.types.zonk(t)
    }

    /// The IR signature of a function for these type arguments.
    pub(crate) fn sig_of(&mut self, info: &FnInfo, map: &HashMap<DefId, Ty>) -> Sig {
        let mut params: Vec<(Pass, Ty, Option<Mode>)> = Vec::new();
        let mut srcs: Vec<(Ty, Option<Mode>)> = Vec::new();
        if info.has_self {
            if let Some(st) = info.self_ty {
                srcs.push((st, info.self_mode));
            }
        }
        for p in &info.params {
            srcs.push((p.ty, p.mode));
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
        Sig { params, ret, ret_pass, ir_params, ir_ret }
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
        let gens = self.typed.fn_infos.get(&def).map(|i| i.generics.clone()).unwrap_or_default();
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
        let concrete: HashMap<DefId, Ty> = key_subst.into_iter().collect();
        let sig = self.sig_of(&info, &concrete);
        let base = self.fn_display_name(def, &concrete);
        let name = self.unique_name(base);
        let mut func = Func::new(&name, &sig.ir_params, sig.ir_ret);
        let e = func.entry();
        func.terminate(e, Term::Stop("not lowered yet".to_string()));
        let id = self.module.add(func);
        self.instances.insert(key, id);
        self.jobs.push(Job::Fn { id, def, subst: concrete });
        Some(id)
    }

    // ---- Running the jobs ------------------------------------------------------------------------------------

    fn run_jobs(&mut self) {
        while let Some(job) = self.jobs.pop() {
            match job {
                Job::Fn { id, def, subst } => self.lower_fn_job(id, def, subst),
                Job::Glue { id, kind, ty } => self.lower_glue_job(id, kind, ty),
            }
        }
    }

    fn lower_fn_job(&mut self, id: FuncId, def: DefId, subst: HashMap<DefId, Ty>) {
        let Some(info) = self.typed.fn_infos.get(&def).cloned() else {
            return;
        };
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
                let (span, msg) = fail.unwrap_or((decl.name.span, "an internal problem".to_string()));
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
        self.push_scope();
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
                        self.f.locals.insert(def, Local::Slot { ptr: slot, ty, flag });
                    } else {
                        self.f.locals.insert(def, Local::Val(v));
                    }
                }
                Pass::Ptr => {
                    let p = ir_params[next];
                    next += 1;
                    let Some(def) = def else { continue };
                    let flag = if mode == Some(Mode::Sink) { self.own(p, ty, true) } else { None };
                    self.f.locals.insert(def, Local::Slot { ptr: p, ty, flag });
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
