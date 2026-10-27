//! Functions the compiler supplies: the `rt_*` intrinsics the library is written over, and the
//! built-in `print`, `assert`, `panic`, `todo` and `unreachable`.
//!
//! The library (`compiler/core/core.pel`) implements lists, text, maps and so on in the language
//! itself, on top of a small set of operations: raw memory (`rt_alloc`, `rt_load_*`, `rt_offset`, ...),
//! knowledge about a type (`rt_size_of<T>`, `rt_drop_fn<T>`, ...) and moving values in and out of
//! memory (`rt_write`, `rt_read`). Each is expanded inline where it is called.

use super::*;

impl<'a> Lowerer<'a> {
    pub(crate) fn is_intrinsic(&self, def: DefId) -> bool {
        let Some(e) = self.fns.get(&def) else {
            return false;
        };
        if !matches!(e.decl.body, FnBody::None) {
            return false;
        }
        let in_core = self
            .program
            .package(self.program.module(e.file).package)
            .name
            == "core";
        let name = &self.res.def(def).name;
        in_core
            && (name.starts_with("rt_")
                || matches!(
                    name.as_str(),
                    "print" | "assert" | "panic" | "todo" | "unreachable"
                ))
    }

    /// An intrinsic that is used as a function value would need a real body; none do yet.
    pub(crate) fn lower_intrinsic_job(
        &mut self,
        _id: FuncId,
        _def: DefId,
        _s: &HashMap<DefId, Ty>,
    ) {
    }

    /// The type argument of an intrinsic such as `rt_size_of<T>`.
    fn intr_type(&mut self, def: DefId, map: &HashMap<DefId, Ty>) -> Option<Ty> {
        let g = self.typed.fn_infos.get(&def)?.generics.first().copied()?;
        let t = *map.get(&g)?;
        Some(self.conc_with(t, &HashMap::new()))
    }

    /// All type arguments of an intrinsic, in declaration order.
    fn intr_types(&mut self, def: DefId, map: &HashMap<DefId, Ty>) -> Vec<Ty> {
        let gens = self
            .typed
            .fn_infos
            .get(&def)
            .map(|i| i.generics.clone())
            .unwrap_or_default();
        gens.iter()
            .filter_map(|g| map.get(g).copied())
            .map(|t| self.conc_with(t, &HashMap::new()))
            .collect()
    }

    pub(crate) fn arg_value(&mut self, args: &'a [Arg], i: usize) -> Option<Value> {
        self.arg_scalar(args, i)
    }

    fn arg_scalar(&mut self, args: &'a [Arg], i: usize) -> Option<Value> {
        match args.get(i) {
            Some(a) => self.scalar(&a.value),
            None => None,
        }
    }

    pub(crate) fn intrinsic_call(
        &mut self,
        e: &'a Expr,
        def: DefId,
        map: &HashMap<DefId, Ty>,
        recv: Option<&'a Expr>,
        args: &'a [Arg],
        dst: Option<Value>,
    ) -> Res {
        let _ = recv;
        let name = self.res.def(def).name.clone();
        let sp = e.span;
        let ptr = IrTy::Ptr;
        match name.as_str() {
            "rt_soa_len" | "rt_soa_push" | "rt_soa_insert" | "rt_soa_remove" | "rt_soa_clear" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "a `soa` operation on an unknown type");
                };
                self.intr_soa(&name, e, t, args, dst)
            }
            "rt_null" => {
                let z = self.i64c(0);
                self.wrap(Op::Cast(CastKind::IntToPtr, z, ptr), ptr, sp)
            }
            "rt_alloc" => {
                let Some(n) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(Op::Alloc(n), ptr, sp)
            }
            "rt_realloc" => {
                let (Some(p), Some(n)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                self.wrap(Op::Realloc(p, n), ptr, sp)
            }
            "rt_free" => {
                let Some(p) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.emit(Op::Free(p), None, sp);
                Res::Unit
            }
            "rt_ctx_init" => {
                let (Some(b), Some(n), Some(c), Some(e)) = (
                    self.arg_scalar(args, 0),
                    self.arg_scalar(args, 1),
                    self.arg_scalar(args, 2),
                    self.arg_scalar(args, 3),
                ) else {
                    return Res::Never;
                };
                self.wrap(Op::CtxInit(b, n, c, e), ptr, sp)
            }
            "rt_fn_addr" => {
                let Some(ExprKind::Name(id)) = args.first().map(|a| &a.value.kind) else {
                    return self.unsupported(sp, "`rt_fn_addr` needs the name of a function");
                };
                let Some(def) = self.res.use_at(self.f.file, id.span.start) else {
                    return self.unsupported(sp, "`rt_fn_addr` of an unknown name");
                };
                let Some(f) = self.request_fn(def, &HashMap::new()) else {
                    return self.unsupported(sp, "`rt_fn_addr` of this function");
                };
                self.wrap(Op::FuncAddr(f), ptr, sp)
            }
            "rt_ctx_switch" => {
                let (Some(s), Some(t)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                self.emit(Op::CtxSwitch(s, t), None, sp);
                Res::Unit
            }
            "rt_offset" => {
                let (Some(p), Some(n)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                self.wrap(Op::PtrAdd(p, n), ptr, sp)
            }
            "rt_copy" => {
                let (Some(d), Some(s), Some(n)) = (
                    self.arg_scalar(args, 0),
                    self.arg_scalar(args, 1),
                    self.arg_scalar(args, 2),
                ) else {
                    return Res::Never;
                };
                self.emit(Op::MemCopy(d, s, n), None, sp);
                Res::Unit
            }
            "rt_is_null" => {
                let Some(p) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                let n = self
                    .emit(
                        Op::Cast(CastKind::PtrToInt, p, IrTy::I64),
                        Some(IrTy::I64),
                        sp,
                    )
                    .unwrap_or(p);
                let z = self.i64c(0);
                self.wrap(Op::Cmp(CmpOp::Eq, n, z), IrTy::Bool, sp)
            }
            "rt_addr_int" => {
                let Some(p) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(Op::Cast(CastKind::PtrToInt, p, IrTy::I64), IrTy::I64, sp)
            }
            "rt_load_int" | "rt_load_byte" | "rt_load_ptr" | "rt_load_float" | "rt_load_char"
            | "rt_load_bool" => {
                let t = match name.as_str() {
                    "rt_load_int" => IrTy::I64,
                    "rt_load_byte" => IrTy::Int(8),
                    "rt_load_ptr" => ptr,
                    "rt_load_float" => IrTy::F64,
                    "rt_load_bool" => IrTy::Bool,
                    _ => IrTy::Int(32),
                };
                let Some(p) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(Op::Load(t, p), t, sp)
            }
            "rt_store_int" | "rt_store_byte" | "rt_store_ptr" | "rt_store_float"
            | "rt_store_char" | "rt_store_bool" => {
                let (Some(p), Some(v)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                self.emit(Op::Store(p, v), None, sp);
                Res::Unit
            }
            "rt_print" => {
                let (Some(p), Some(n)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                self.emit(Op::Print(p, n), None, sp);
                Res::Unit
            }
            "rt_to_byte" => {
                let Some(n) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(Op::Cast(CastKind::Trunc, n, IrTy::Int(8)), IrTy::Int(8), sp)
            }
            "rt_to_char" => {
                let Some(n) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(
                    Op::Cast(CastKind::Trunc, n, IrTy::Int(32)),
                    IrTy::Int(32),
                    sp,
                )
            }
            "rt_show_to" | "rt_show_item_to" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rt_show_to` of an unknown type");
                };
                let (Some(p), Some(out)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                if self.show_at(p, t, out, name == "rt_show_item_to") {
                    Res::Unit
                } else {
                    Res::Never
                }
            }
            "rt_fma" => {
                let (Some(a), Some(b), Some(c)) = (
                    self.arg_scalar(args, 0),
                    self.arg_scalar(args, 1),
                    self.arg_scalar(args, 2),
                ) else {
                    return Res::Never;
                };
                self.wrap(Op::Fma(a, b, c), IrTy::F64, sp)
            }
            "rt_to_int" => {
                let Some(x) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(Op::Cast(CastKind::FloatToInt, x, IrTy::I64), IrTy::I64, sp)
            }
            "rt_size_of" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rt_size_of` of an unknown type");
                };
                let n = self.layout(t).size;
                Res::Val(self.i64c(n as i64))
            }
            "rt_is_int" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rt_is_int` of an unknown type");
                };
                let yes = matches!(self.kind(t), TyKind::Int(crate::types::IntKind::I64));
                Res::Val(self.konst(IrTy::Bool, Const::Bool(yes), sp))
            }
            "rt_addr_of" => self.intr_addr_of(e, args),
            "rt_write" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rt_write` of an unknown type");
                };
                let (Some(d), Some(v)) = (self.arg_scalar(args, 0), args.get(1)) else {
                    return Res::Never;
                };
                match self.into(&v.value, d, t) {
                    Res::Never => Res::Never,
                    _ => Res::Unit,
                }
            }
            "rt_read" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rt_read` of an unknown type");
                };
                let Some(p) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                match self.repr(t) {
                    Repr::Scalar(s) => self.wrap(Op::Load(s, p), s, sp),
                    Repr::Zst => Res::Unit,
                    Repr::Mem(_) => {
                        let d = match dst {
                            Some(d) => d,
                            None => self.slot_for(t),
                        };
                        self.copy_mem(d, p, t);
                        if dst.is_none() {
                            self.own(d, t, false);
                            return Res::Val(d);
                        }
                        Res::Unit
                    }
                }
            }
            "rt_cast" => {
                let tys = self.intr_types(def, map);
                let [a, b] = tys.as_slice() else {
                    return self.unsupported(sp, "`rt_cast` of unknown types");
                };
                let (a, b) = (*a, *b);
                let Some(arg) = args.first() else {
                    return Res::Never;
                };
                match (self.repr(a), self.repr(b)) {
                    (Repr::Mem(_), Repr::Mem(_)) => {
                        self.with_dst(dst, b, |s, d| s.into(&arg.value, d, a))
                    }
                    (Repr::Mem(_), Repr::Scalar(sb)) => {
                        let slot = self.slot_for(a);
                        if let Res::Never = self.into(&arg.value, slot, a) {
                            return Res::Never;
                        }
                        Res::Val(self.load_at(slot, 0, sb))
                    }
                    (Repr::Scalar(_), Repr::Mem(_)) => {
                        self.with_dst(dst, b, |s, d| match s.consume_scalar(&arg.value) {
                            Some(v) => {
                                s.store_scalar_ty(d, v);
                                Res::Unit
                            }
                            None => Res::Never,
                        })
                    }
                    (Repr::Scalar(sa), Repr::Scalar(sb)) => match self.consume_scalar(&arg.value) {
                        Some(v) if sa == sb => Res::Val(v),
                        Some(v) if sa == IrTy::Ptr && matches!(sb, IrTy::Int(_)) => {
                            self.wrap(Op::Cast(CastKind::PtrToInt, v, sb), sb, sp)
                        }
                        Some(v) if sb == IrTy::Ptr && matches!(sa, IrTy::Int(_)) => {
                            self.wrap(Op::Cast(CastKind::IntToPtr, v, sb), sb, sp)
                        }
                        Some(v) => Res::Val(v),
                        None => Res::Never,
                    },
                    _ => match self.consume_scalar(&arg.value) {
                        Some(v) => Res::Val(v),
                        None => Res::Never,
                    },
                }
            }
            "rt_clone" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rt_clone` of an unknown type");
                };
                let Some(src) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                match self.repr(t) {
                    Repr::Mem(_) => self.with_dst(dst, t, |s, d| {
                        s.clone_into(src, d, t);
                        Res::Unit
                    }),
                    Repr::Scalar(sc) => {
                        if self.needs_drop(t) {
                            let slot = self.slot_for(t);
                            self.clone_into(src, slot, t);
                            Res::Val(self.load_at(slot, 0, sc))
                        } else {
                            Res::Val(self.load_at(src, 0, sc))
                        }
                    }
                    Repr::Zst => Res::Unit,
                }
            }
            "rt_clone_to" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rt_clone_to` of an unknown type");
                };
                let (Some(s), Some(d)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                self.clone_into(s, d, t);
                Res::Unit
            }
            "rt_drop_at" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rt_drop_at` of an unknown type");
                };
                let Some(p) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.drop_at(p, t);
                Res::Unit
            }
            "rt_drop_fn" | "rt_clone_fn" | "rt_eq_fn" | "rt_cmp_fn" | "rt_hash_fn"
            | "rt_show_fn" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "a glue function of an unknown type");
                };
                let kind = match name.as_str() {
                    "rt_drop_fn" => GlueKind::Drop,
                    "rt_clone_fn" => GlueKind::Clone,
                    "rt_eq_fn" => GlueKind::Eq,
                    "rt_cmp_fn" => GlueKind::Cmp,
                    "rt_hash_fn" => GlueKind::Hash,
                    _ => GlueKind::Show,
                };
                match self.glue_fn(kind, t) {
                    Some(f) => self.wrap(Op::FuncAddr(f), ptr, sp),
                    None => {
                        let z = self.i64c(0);
                        self.wrap(Op::Cast(CastKind::IntToPtr, z, ptr), ptr, sp)
                    }
                }
            }
            "rt_call_void1" | "rt_call_void2" | "rt_call_bool2" | "rt_call_int2"
            | "rt_call_int1" => {
                let Some(f) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                let n = if name.ends_with('1') { 1 } else { 2 };
                let mut vals = Vec::new();
                for i in 0..n {
                    match self.arg_scalar(args, i + 1) {
                        Some(v) => vals.push(v),
                        None => return Res::Never,
                    }
                }
                let ps = vec![ptr; n];
                let (ret, res) = if name.contains("void") {
                    (IrTy::Void, None)
                } else if name.contains("bool") {
                    (IrTy::Bool, Some(IrTy::Bool))
                } else {
                    (IrTy::I64, Some(IrTy::I64))
                };
                match self.emit(Op::CallPtr(ps, ret, f, vals), res, sp) {
                    Some(v) => Res::Val(v),
                    None => Res::Unit,
                }
            }
            "print" => self.print_call(e, args),
            "assert" => {
                let Some(c) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                let bad = self.emit(Op::Not(c), Some(IrTy::Bool), sp).unwrap_or(c);
                self.stop_if(bad, "assertion failed");
                Res::Unit
            }
            "panic" => {
                if let Some(a) = args.first() {
                    if let Res::Val(h) = self.expr(&a.value) {
                        self.print_text_at(h);
                    }
                }
                self.stop_with("panic")
            }
            "todo" => self.stop_with("todo: not written yet"),
            "unreachable" => self.stop_with("reached unreachable code"),
            other => self.unsupported(sp, &format!("the intrinsic `{other}`")),
        }
    }

    /// A built-in function reached by its name only (when the resolver gave it no definition in `core`).
    pub(crate) fn intrinsic_by_name(
        &mut self,
        e: &'a Expr,
        name: &str,
        args: &'a [Arg],
        dst: Option<Value>,
    ) -> Res {
        let _ = dst;
        match name {
            "print" => self.print_call(e, args),
            "assert" => {
                let Some(c) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                let bad = self.emit(Op::Not(c), Some(IrTy::Bool), e.span).unwrap_or(c);
                self.stop_if(bad, "assertion failed");
                Res::Unit
            }
            "panic" => {
                if let Some(a) = args.first() {
                    if let Res::Val(h) = self.expr(&a.value) {
                        self.print_text_at(h);
                    }
                }
                self.stop_with("panic")
            }
            "todo" => self.stop_with("todo: not written yet"),
            "unreachable" => self.stop_with("reached unreachable code"),
            other => self.unsupported(e.span, &format!("the built-in `{other}`")),
        }
    }

    fn stop_with(&mut self, msg: &str) -> Res {
        self.terminate(Term::Stop(msg.to_string()));
        Res::Never
    }

    /// Writes the text of a `string` or `str` (at address `h`) followed by a newline.
    fn print_text_at(&mut self, h: Value) {
        let p = self.load_at(h, 0, IrTy::Ptr);
        let n = self.load_at(h, 8, IrTy::I64);
        self.emit(Op::Print(p, n), None, Span::new(0, 0));
        self.print_newline();
    }

    pub(crate) fn print_newline(&mut self) {
        let d = self.module.add_data(b"\n");
        let p = self.emit(Op::DataAddr(d), Some(IrTy::Ptr), Span::new(0, 0));
        let one = self.i64c(1);
        if let Some(p) = p {
            self.emit(Op::Print(p, one), None, Span::new(0, 0));
        }
    }

    fn intr_addr_of(&mut self, e: &'a Expr, args: &'a [Arg]) -> Res {
        let Some(a) = args.first() else {
            return self.unsupported(e.span, "`rt_addr_of` without an argument");
        };
        let arg = &a.value;
        let Some(t) = self.ty_of(arg) else {
            return self.unsupported(arg.span, "`rt_addr_of` of an unknown type");
        };
        if let Some(p) = self.place(arg) {
            return Res::Val(p);
        }
        if self.failed() {
            return Res::Never;
        }
        match self.repr(t) {
            Repr::Mem(_) => self.expr(arg),
            Repr::Scalar(_) => {
                let Some(v) = self.scalar(arg) else {
                    return Res::Never;
                };
                let slot = self.slot_for(t);
                self.store_scalar_ty(slot, v);
                Res::Val(slot)
            }
            Repr::Zst => {
                let s = self.slot_bytes(1, 1);
                Res::Val(s)
            }
        }
    }
}
