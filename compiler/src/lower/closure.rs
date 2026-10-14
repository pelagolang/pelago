//! Function values: closures, named functions used as values, and calls through them.
//!
//! A function value is two words: the address of code, and the address of an environment. The code
//! takes the environment as its first parameter (after the return address, if any), then the declared
//! parameters. A closure's environment holds, for each variable it uses, either the address of that
//! variable (the default: closures are views and do not outlive their function), a copy of an
//! unchanging scalar, or the value itself when captured with `sink`.

use super::call::ArgSource;
use super::*;
use crate::visit::{self, Visitor};

#[derive(Clone, Copy, Debug)]
pub(crate) enum CapKind {
    /// The address of the variable.
    Ref,
    /// A copy of a scalar that never changes.
    Val(IrTy),
    /// The value itself, moved in; the closure owns and drops it.
    Sink,
}

#[derive(Clone, Debug)]
pub(crate) struct CapInfo {
    pub def: DefId,
    pub kind: CapKind,
    pub ty: Ty,
    pub off: u32,
}

pub(crate) struct ClosureJob<'a> {
    pub id: FuncId,
    pub closure: &'a Closure,
    pub it: Option<DefId>,
    pub file: ModuleId,
    pub subst: HashMap<DefId, Ty>,
    pub fn_ty: Ty,
    pub caps: Vec<CapInfo>,
}

pub(crate) struct ThunkJob {
    pub id: FuncId,
    pub target: FuncId,
    pub sig: Sig,
}

/// Collects the places where names are used.
struct Uses {
    starts: Vec<(u32, bool)>,
}

impl Visitor for Uses {
    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Name(id) => self.starts.push((id.span.start, false)),
            ExprKind::SelfValue => self.starts.push((e.span.start, true)),
            _ => {}
        }
        visit::walk_expr(self, e);
    }
}

impl<'a> Lowerer<'a> {
    /// The IR signature of calling a function value of this type.
    pub(crate) fn fn_value_sig(&mut self, fn_ty: Ty) -> Option<Sig> {
        let TyKind::Fn(ps, r) = self.kind(fn_ty) else {
            return None;
        };
        let ret = self.types.zonk(r);
        let ret_pass = match self.repr(ret) {
            Repr::Zst => RetPass::Void,
            Repr::Scalar(t) => RetPass::Val(t),
            Repr::Mem(_) => RetPass::Sret,
        };
        let mut ir_params = Vec::new();
        if ret_pass == RetPass::Sret {
            ir_params.push(IrTy::Ptr);
        }
        // The environment.
        ir_params.push(IrTy::Ptr);
        let mut params = Vec::new();
        for p in ps {
            let t = self.types.zonk(p.ty);
            let pass = self.pass_of(t, p.mode);
            match pass {
                Pass::Skip => {}
                Pass::Val(x) => ir_params.push(x),
                Pass::Ptr => ir_params.push(IrTy::Ptr),
            }
            params.push((pass, t, p.mode));
        }
        let ir_ret = match ret_pass {
            RetPass::Val(t) => t,
            _ => IrTy::Void,
        };
        Some(Sig {
            params,
            ret,
            ret_pass,
            ir_params,
            ir_ret,
        })
    }

    // ---- Making a closure -----------------------------------------------------------------------------------------

    pub(crate) fn closure_value(&mut self, e: &'a Expr, c: &'a Closure, dst: Value, t: Ty) -> Res {
        let Some(sig) = self.fn_value_sig(t) else {
            return self.unsupported(e.span, "a closure of unknown type");
        };
        // Which variables of the enclosing function the body uses.
        let mut uses = Uses { starts: Vec::new() };
        uses.visit_expr(&c.body);
        let mut defs: Vec<DefId> = Vec::new();
        for (start, is_self) in uses.starts {
            let _ = is_self;
            if let Some(d) = self.res.use_at(self.f.file, start) {
                if self.f.locals.contains_key(&d) && !defs.contains(&d) {
                    defs.push(d);
                }
            }
        }
        for cap in &c.captures {
            if let Some(d) = self.res.use_at(self.f.file, cap.name.span.start) {
                if self.f.locals.contains_key(&d) && !defs.contains(&d) {
                    defs.push(d);
                }
            }
        }
        // Their modes, from the capture list.
        let mut kinds: Vec<(DefId, Option<Mode>)> = Vec::new();
        for d in &defs {
            let mode = c.captures.iter().find_map(|cap| {
                (self.res.use_at(self.f.file, cap.name.span.start) == Some(*d)).then_some(cap.mode)
            });
            kinds.push((*d, mode.flatten()));
        }
        let mut caps: Vec<CapInfo> = Vec::new();
        let mut tys: Vec<Ty> = Vec::new();
        let ptr_ty = self.mk_ptr();
        for (d, mode) in &kinds {
            let local = self.f.locals.get(d).copied();
            let (kind, field_ty, ty) = match local {
                Some(Local::Val(v)) => {
                    let s = self.vty(v);
                    let ty = self
                        .typed
                        .local_tys
                        .get(d)
                        .copied()
                        .map(|x| self.conc(x))
                        .unwrap_or(self.types.int);
                    (CapKind::Val(s), ty, ty)
                }
                Some(Local::Slot { ty, .. }) => {
                    if *mode == Some(Mode::Sink) {
                        (CapKind::Sink, ty, ty)
                    } else {
                        (CapKind::Ref, ptr_ty, ty)
                    }
                }
                None => continue,
            };
            tys.push(field_ty);
            caps.push(CapInfo {
                def: *d,
                kind,
                ty,
                off: 0,
            });
        }
        let fields = self.place_fields(0, &tys);
        for (c, f) in caps.iter_mut().zip(fields.iter()) {
            c.off = f.off;
        }
        let layout = self.fields_layout(&fields);
        let env = self.slot_bytes(layout.size.max(1), layout.align);
        for cap in &caps {
            let local = self.f.locals.get(&cap.def).copied();
            let at = self.addr(env, cap.off);
            match (cap.kind, local) {
                (CapKind::Val(_), Some(Local::Val(v))) => self.store_scalar_ty(at, v),
                (CapKind::Ref, Some(Local::Slot { ptr, .. })) => self.store_scalar_ty(at, ptr),
                (CapKind::Sink, Some(Local::Slot { ptr, ty, flag })) => {
                    self.copy_mem_or_scalar(at, ptr, ty);
                    if let Some(f) = flag {
                        self.clear_flag(f);
                    }
                }
                _ => {}
            }
        }
        // The code.
        let name = self.unique_name(format!("closure in {}", self.f.func.name));
        let mut func = Func::new(&name, &sig.ir_params, sig.ir_ret);
        let entry = func.entry();
        func.terminate(entry, Term::Stop("not lowered yet".to_string()));
        let id = self.module.add(func);
        let it = self
            .res
            .closure_it
            .get(&(self.f.file, e.span.start))
            .copied();
        self.jobs.push(Job::Closure(ClosureJob {
            id,
            closure: c,
            it,
            file: self.f.file,
            subst: self.f.subst.clone(),
            fn_ty: t,
            caps,
        }));
        let code = self
            .emit(Op::FuncAddr(id), Some(IrTy::Ptr), e.span)
            .unwrap_or(env);
        self.store_at(dst, 0, code);
        self.store_at(dst, 8, env);
        Res::Unit
    }

    /// Lowers the body of a closure as a function of its own.
    pub(crate) fn lower_closure_job(&mut self, job: ClosureJob<'a>) {
        let Some(sig) = self.fn_value_sig(job.fn_ty) else {
            return;
        };
        let name = self.module.funcs[job.id.0 as usize].name.clone();
        let void = self.types.void;
        let mut state = FnState::empty(void);
        state.func = Func::new(&name, &sig.ir_params, sig.ir_ret);
        state.file = job.file;
        state.cur = Some(state.func.entry());
        state.subst = job.subst.clone();
        state.ret_ty = sig.ret;
        self.f = state;
        let entry = self.f.func.entry();
        let ps = self.f.func.block_params(entry).to_vec();
        let mut next = 0usize;
        if sig.ret_pass == RetPass::Sret {
            self.f.sret = Some(ps[0]);
            next = 1;
        }
        let env = ps[next];
        next += 1;
        self.push_scope();
        // The captured variables.
        for cap in &job.caps {
            let at = self.addr(env, cap.off);
            match cap.kind {
                CapKind::Val(s) => {
                    let v = self.load_at(at, 0, s);
                    self.f.locals.insert(cap.def, Local::Val(v));
                }
                CapKind::Ref => {
                    let p = self.load_at(at, 0, IrTy::Ptr);
                    self.f.locals.insert(
                        cap.def,
                        Local::Slot {
                            ptr: p,
                            ty: cap.ty,
                            flag: None,
                        },
                    );
                }
                CapKind::Sink => {
                    let flag = self.own(at, cap.ty, true);
                    self.f.locals.insert(
                        cap.def,
                        Local::Slot {
                            ptr: at,
                            ty: cap.ty,
                            flag,
                        },
                    );
                }
            }
        }
        // The parameters.
        let c = job.closure;
        let mut param_defs: Vec<Option<DefId>> = Vec::new();
        if c.params.is_empty() {
            if let Some(it) = job.it {
                param_defs.push(Some(it));
            }
        }
        let mut param_pats: Vec<Option<&'a Pattern>> = Vec::new();
        for p in &c.params {
            let def = match &p.pattern.kind {
                PatternKind::Binding { name, .. } => {
                    self.res.decls.get(&(job.file, name.span.start)).copied()
                }
                _ => None,
            };
            param_defs.push(def);
            param_pats.push(if def.is_some() {
                None
            } else {
                Some(&p.pattern)
            });
        }
        for (i, (pass, ty, mode)) in sig.params.iter().enumerate() {
            let (pass, ty, mode) = (*pass, *ty, *mode);
            let def = param_defs.get(i).copied().flatten();
            match pass {
                Pass::Skip => {}
                Pass::Val(_) => {
                    let v = ps[next];
                    next += 1;
                    if let Some(def) = def {
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
                }
                Pass::Ptr => {
                    let p = ps[next];
                    next += 1;
                    if let Some(def) = def {
                        let flag = if mode == Some(Mode::Sink) {
                            self.own(p, ty, true)
                        } else {
                            None
                        };
                        self.f.locals.insert(def, Local::Slot { ptr: p, ty, flag });
                    } else if let Some(Some(pat)) = param_pats.get(i).copied() {
                        let fail = self.stop_block("pattern did not match");
                        self.pat_test(pat, p, ty, fail);
                    }
                }
            }
        }
        // The body.
        let ret = sig.ret;
        let r = match sig.ret_pass {
            RetPass::Sret => {
                let d = self.f.sret.unwrap_or(Value(0));
                self.into(&c.body, d, ret)
            }
            _ => self.expr(&c.body),
        };
        if !self.failed() {
            self.finish_return(r, &sig, c.body.span);
        }
        let st = std::mem::replace(&mut self.f, FnState::empty(void));
        match st.fail {
            None => {
                let mut func = st.func;
                func.compact();
                self.module.funcs[job.id.0 as usize] = func;
            }
            Some((span, msg)) => self.diags.push(Diagnostic {
                module: job.file,
                span,
                message: format!("not lowered yet: {msg} (in `{name}`)"),
                warning: true,
            }),
        }
    }

    // ---- Named functions as values ----------------------------------------------------------------------------------------

    pub(crate) fn fn_as_value(&mut self, e: &'a Expr, def: DefId) -> Res {
        let Some(t) = self.ty_of(e) else {
            return self.unsupported(e.span, "a function of unknown type");
        };
        self.with_dst(None, t, |s, dst| s.fn_value_into(e, def, dst, t))
    }

    pub(crate) fn fn_value_into(&mut self, e: &'a Expr, def: DefId, dst: Value, t: Ty) -> Res {
        let map = self.call_target(e.span).map(|(_, m)| m).unwrap_or_default();
        let Some(info) = self.typed.fn_infos.get(&def).cloned() else {
            return self.unsupported(e.span, "a function without a signature");
        };
        let real_sig = self.sig_of(&info, &map);
        let Some(target) = self.request_fn(def, &map) else {
            return self.unsupported(e.span, "this function as a value");
        };
        let Some(vsig) = self.fn_value_sig(t) else {
            return self.unsupported(e.span, "this function as a value");
        };
        let name = self.unique_name(format!(
            "{} as value",
            self.module.funcs[target.0 as usize].name
        ));
        let mut func = Func::new(&name, &vsig.ir_params, vsig.ir_ret);
        let entry = func.entry();
        func.terminate(entry, Term::Stop("not lowered yet".to_string()));
        let id = self.module.add(func);
        self.thunks.push(ThunkJob {
            id,
            target,
            sig: real_sig,
        });
        let code = self
            .emit(Op::FuncAddr(id), Some(IrTy::Ptr), e.span)
            .unwrap_or(dst);
        let z = self.i64c(0);
        let nullp = self
            .emit(
                Op::Cast(CastKind::IntToPtr, z, IrTy::Ptr),
                Some(IrTy::Ptr),
                e.span,
            )
            .unwrap_or(dst);
        self.store_at(dst, 0, code);
        self.store_at(dst, 8, nullp);
        Res::Unit
    }

    /// The code for a named function used as a value: it ignores the environment and forwards.
    pub(crate) fn lower_thunk(&mut self, job: ThunkJob) {
        let placeholder = self.module.funcs[job.id.0 as usize].clone();
        let mut func = Func::new(&placeholder.name, &placeholder.params, placeholder.ret);
        let entry = func.entry();
        let ps = func.block_params(entry).to_vec();
        // [sret] env args...  ->  [sret] args...
        let has_sret = job.sig.ret_pass == RetPass::Sret;
        let mut args: Vec<Value> = Vec::new();
        let mut i = 0;
        if has_sret {
            args.push(ps[0]);
            i = 1;
        }
        i += 1; // the environment
        args.extend_from_slice(&ps[i..]);
        let ret_ty = if job.sig.ret_pass == RetPass::Void || has_sret {
            None
        } else {
            Some(job.sig.ir_ret)
        };
        let r = func.emit(entry, Op::Call(job.target, args), ret_ty);
        func.terminate(entry, Term::Return(r));
        self.module.funcs[job.id.0 as usize] = func;
    }

    // ---- Calling a function value ---------------------------------------------------------------------------------------------

    pub(crate) fn call_value(
        &mut self,
        e: &'a Expr,
        callee: &'a Expr,
        args: &'a [Arg],
        dst: Option<Value>,
    ) -> Res {
        let Some(ft) = self.ty_of(callee) else {
            return self.unsupported(e.span, "calling a value of unknown type");
        };
        let Some(sig) = self.fn_value_sig(ft) else {
            return self.unsupported(e.span, "calling something that is not a function");
        };
        let Res::Val(fv) = self.expr(callee) else {
            return Res::Never;
        };
        let code = self.load_at(fv, 0, IrTy::Ptr);
        let env = self.load_at(fv, 8, IrTy::Ptr);
        let mut ir_args: Vec<Value> = Vec::new();
        let mut ret_slot = None;
        if sig.ret_pass == RetPass::Sret {
            let d = match dst {
                Some(d) => d,
                None => self.slot_for(sig.ret),
            };
            ret_slot = Some(d);
            ir_args.push(d);
        }
        ir_args.push(env);
        if args.len() != sig.params.len() {
            return self.unsupported(e.span, "a call with a different number of arguments");
        }
        for ((pass, ty, mode), a) in sig.params.iter().zip(args.iter()) {
            let v = self.lower_arg(*pass, *ty, *mode, ArgSource::Expr(&a.value), e.span);
            match (pass, v) {
                (Pass::Skip, _) => {}
                (_, Some(v)) => ir_args.push(v),
                (_, None) => return Res::Never,
            }
        }
        let param_tys: Vec<IrTy> = ir_args.iter().map(|v| self.vty(*v)).collect();
        let (ret_ir, res_ty) = match sig.ret_pass {
            RetPass::Val(t) => (t, Some(t)),
            _ => (IrTy::Void, None),
        };
        let result = self.emit(
            Op::CallPtr(param_tys, ret_ir, code, ir_args),
            res_ty,
            e.span,
        );
        if matches!(self.kind(sig.ret), TyKind::Never) {
            self.terminate(Term::Unreachable);
            return Res::Never;
        }
        match sig.ret_pass {
            RetPass::Val(_) => match result {
                Some(v) => Res::Val(v),
                None => Res::Never,
            },
            RetPass::Void => Res::Unit,
            RetPass::Sret => {
                if dst.is_none() {
                    if let Some(slot) = ret_slot {
                        self.own(slot, sig.ret, false);
                        return Res::Val(slot);
                    }
                }
                Res::Unit
            }
        }
    }
}
