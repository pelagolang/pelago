//! Functions the compiler supplies: the `rt*` intrinsics the library is written over, and the
//! built-in `print`, `assert`, `panic`, `todo` and `unreachable`.
//!
//! The library (`compiler/core/core.pel`) implements lists, text, maps and so on in the language
//! itself, on top of a small set of operations: raw memory (`rtAlloc`, `rtLoad*`, `rtOffset`, ...),
//! knowledge about a type (`rtSizeOf<T>`, `rtDropFn<T>`, ...) and moving values in and out of
//! memory (`rtWrite`, `rtRead`). Each is expanded inline where it is called.

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
            && (is_rt_name(name)
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

    /// The type argument of an intrinsic such as `rtSizeOf<T>`.
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
            "rtSoaLen" | "rtSoaPush" | "rtSoaInsert" | "rtSoaRemove" | "rtSoaClear" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "a `soa` operation on an unknown type");
                };
                self.intr_soa(&name, e, t, args, dst)
            }
            "rtNull" => {
                let z = self.i64c(0);
                self.wrap(Op::Cast(CastKind::IntToPtr, z, ptr), ptr, sp)
            }
            "rtAlloc" => {
                let Some(n) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(Op::Alloc(n), ptr, sp)
            }
            "rtRealloc" => {
                let (Some(p), Some(n)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                self.wrap(Op::Realloc(p, n), ptr, sp)
            }
            "rtFree" => {
                let Some(p) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.emit(Op::Free(p), None, sp);
                Res::Unit
            }
            "rtOsReserve" | "rtOsCommit" | "rtOsRelease" | "rtOsRemap" | "rtOsAdviseNohuge"
            | "rtOsWrite" | "rtOsRead" | "rtOsExit" | "rtOsArgv" | "rtOsOpen" | "rtOsClose"
            | "rtOsClock" | "rtOsSleep" | "rtAtomicCas" | "rtAtomicAdd" | "rtAtomicSwap"
            | "rtThreadStart" | "rtFutexWait" | "rtFutexWake" | "rtOsFs" | "rtOsDents"
            | "rtOsRandom" => self.intr_os(&name, args, sp),
            "rtDataWord" => {
                // The address of the k-th word of the block the compiler keeps for the library's own
                // state; `k` is a literal, so a bad one is a compile error, not a stop.
                let k = args.first().and_then(|a| match a.value.kind {
                    ExprKind::Int => crate::typecheck::literal_value(self.src_text(a.value.span)),
                    _ => None,
                });
                let Some(k) =
                    k.filter(|k| (0..crate::ir::DATA_WORDS as i128).contains(&(*k as i128)))
                else {
                    return self.unsupported(
                        sp,
                        "`rtDataWord` needs a number literal below 64 (`DATA_WORDS`)",
                    );
                };
                let d = self.module.word_block();
                let base = self.emit(Op::DataAddr(d), Some(IrTy::Ptr), sp);
                let off = self.i64c(k as i64 * 8);
                let Some(base) = base else {
                    return Res::Never;
                };
                self.wrap(Op::PtrAdd(base, off), ptr, sp)
            }
            "rtIntAddr" => {
                let Some(n) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(Op::Cast(CastKind::IntToPtr, n, ptr), ptr, sp)
            }
            "rtCtxInit" => {
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
            "rtFnAddr" => {
                let Some(ExprKind::Name(id)) = args.first().map(|a| &a.value.kind) else {
                    return self.unsupported(sp, "`rtFnAddr` needs the name of a function");
                };
                let Some(def) = self.res.use_at(self.f.file, id.span.start) else {
                    return self.unsupported(sp, "`rtFnAddr` of an unknown name");
                };
                let Some(f) = self.request_fn(def, &HashMap::new()) else {
                    return self.unsupported(sp, "`rtFnAddr` of this function");
                };
                self.wrap(Op::FuncAddr(f), ptr, sp)
            }
            "rtCtxSwitch" => {
                let (Some(s), Some(t)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                self.emit(Op::CtxSwitch(s, t), None, sp);
                Res::Unit
            }
            "rtOffset" => {
                let (Some(p), Some(n)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                self.wrap(Op::PtrAdd(p, n), ptr, sp)
            }
            "rtCopy" => {
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
            "rtIsNull" => {
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
            "rtAddrInt" => {
                let Some(p) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(Op::Cast(CastKind::PtrToInt, p, IrTy::I64), IrTy::I64, sp)
            }
            "rtLoadInt" | "rtLoadByte" | "rtLoadPtr" | "rtLoadFloat" | "rtLoadChar"
            | "rtLoadBool" => {
                let t = match name.as_str() {
                    "rtLoadInt" => IrTy::I64,
                    "rtLoadByte" => IrTy::Int(8),
                    "rtLoadPtr" => ptr,
                    "rtLoadFloat" => IrTy::F64,
                    "rtLoadBool" => IrTy::Bool,
                    _ => IrTy::Int(32),
                };
                let Some(p) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(Op::Load(t, p), t, sp)
            }
            "rtStoreInt" | "rtStoreByte" | "rtStorePtr" | "rtStoreFloat" | "rtStoreChar"
            | "rtStoreBool" => {
                let (Some(p), Some(v)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                self.emit(Op::Store(p, v), None, sp);
                Res::Unit
            }
            "rtPrint" => {
                let (Some(p), Some(n)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                self.emit(Op::Print(p, n), None, sp);
                Res::Unit
            }
            "rtToByte" => {
                let Some(n) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(Op::Cast(CastKind::Trunc, n, IrTy::Int(8)), IrTy::Int(8), sp)
            }
            "rtWrap" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rtWrap` of an unknown type");
                };
                let (Some(n), Some(to)) = (self.arg_scalar(args, 0), self.scalar_of(t)) else {
                    return Res::Never;
                };
                if self.vty(n) == to {
                    return Res::Val(n);
                }
                self.wrap(Op::Cast(CastKind::Trunc, n, to), to, sp)
            }
            "rtToChar" => {
                let Some(n) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(
                    Op::Cast(CastKind::Trunc, n, IrTy::Int(32)),
                    IrTy::Int(32),
                    sp,
                )
            }
            "rtShowTo" | "rtShowItemTo" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rtShowTo` of an unknown type");
                };
                let (Some(p), Some(out)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                if self.show_at(p, t, out, name == "rtShowItemTo") {
                    Res::Unit
                } else {
                    Res::Never
                }
            }
            "rtFma" => {
                let (Some(a), Some(b), Some(c)) = (
                    self.arg_scalar(args, 0),
                    self.arg_scalar(args, 1),
                    self.arg_scalar(args, 2),
                ) else {
                    return Res::Never;
                };
                self.wrap(Op::Fma(a, b, c), IrTy::F64, sp)
            }
            "rtSqrt" | "rtFloor" | "rtCeil" => {
                let Some(x) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                let o = match name.as_str() {
                    "rtSqrt" => FUn::Sqrt,
                    "rtFloor" => FUn::Floor,
                    _ => FUn::Ceil,
                };
                self.wrap(Op::FUn(o, x), IrTy::F64, sp)
            }
            "rtToInt" => {
                let Some(x) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.wrap(Op::Cast(CastKind::FloatToInt, x, IrTy::I64), IrTy::I64, sp)
            }
            "rtSizeOf" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rtSizeOf` of an unknown type");
                };
                let n = self.layout(t).size;
                Res::Val(self.i64c(n as i64))
            }
            "rtSlotOff" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rtSlotOff` of an unknown type");
                };
                let slot = args.first().and_then(|a| match a.value.kind {
                    ExprKind::Int => self.src_text(a.value.span).parse::<usize>().ok(),
                    _ => None,
                });
                let Some(i) = slot else {
                    return self
                        .unsupported(sp, "`rtSlotOff` with a slot that is not a plain number");
                };
                let fs = self.struct_fields(t);
                let Some(f) = fs.get(i) else {
                    return self.unsupported(sp, "`rtSlotOff` past the last slot");
                };
                let off = f.off;
                Res::Val(self.i64c(off as i64))
            }
            "rtIsInt" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rtIsInt` of an unknown type");
                };
                let yes = matches!(self.kind(t), TyKind::Int(crate::types::IntKind::I64));
                Res::Val(self.konst(IrTy::Bool, Const::Bool(yes), sp))
            }
            "rtIsSmallUint" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rtIsSmallUint` of an unknown type");
                };
                let yes = matches!(
                    self.kind(t),
                    TyKind::Int(
                        crate::types::IntKind::U8
                            | crate::types::IntKind::U16
                            | crate::types::IntKind::U32
                    )
                );
                Res::Val(self.konst(IrTy::Bool, Const::Bool(yes), sp))
            }
            "rtNeedsDrop" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rtNeedsDrop` of an unknown type");
                };
                let yes = self.needs_drop(t);
                Res::Val(self.konst(IrTy::Bool, Const::Bool(yes), sp))
            }
            "rtAddrOf" => self.intr_addr_of(e, args),
            "rtWrite" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rtWrite` of an unknown type");
                };
                let (Some(d), Some(v)) = (self.arg_scalar(args, 0), args.get(1)) else {
                    return Res::Never;
                };
                match self.into(&v.value, d, t) {
                    Res::Never => Res::Never,
                    _ => Res::Unit,
                }
            }
            "rtRead" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rtRead` of an unknown type");
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
            "rtCast" => {
                let tys = self.intr_types(def, map);
                let [a, b] = tys.as_slice() else {
                    return self.unsupported(sp, "`rtCast` of unknown types");
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
            "rtClone" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rtClone` of an unknown type");
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
            "rtCloneTo" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rtCloneTo` of an unknown type");
                };
                let (Some(s), Some(d)) = (self.arg_scalar(args, 0), self.arg_scalar(args, 1))
                else {
                    return Res::Never;
                };
                self.clone_into(s, d, t);
                Res::Unit
            }
            "rtDropAt" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "`rtDropAt` of an unknown type");
                };
                let Some(p) = self.arg_scalar(args, 0) else {
                    return Res::Never;
                };
                self.drop_at(p, t);
                Res::Unit
            }
            "rtDropFn" | "rtCloneFn" | "rtEqFn" | "rtCmpFn" | "rtHashFn" | "rtShowFn" => {
                let Some(t) = self.intr_type(def, map) else {
                    return self.unsupported(sp, "a glue function of an unknown type");
                };
                let kind = match name.as_str() {
                    "rtDropFn" => GlueKind::Drop,
                    "rtCloneFn" => GlueKind::Clone,
                    "rtEqFn" => GlueKind::Eq,
                    "rtCmpFn" => GlueKind::Cmp,
                    "rtHashFn" => GlueKind::Hash,
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
            "rtCallVoid1" | "rtCallVoid2" | "rtCallBool2" | "rtCallInt2" | "rtCallInt1" => {
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
                let (ret, res) = if name.contains("Void") {
                    (IrTy::Void, None)
                } else if name.contains("Bool") {
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
        self.terminate(Term::Stop(msg.to_string(), self.f.loc));
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
        self.print_static(b"\n");
    }

    /// Writes constant bytes.
    pub(crate) fn print_static(&mut self, bytes: &[u8]) {
        let d = self.module.add_data(bytes);
        let p = self.emit(Op::DataAddr(d), Some(IrTy::Ptr), Span::new(0, 0));
        let n = self.i64c(bytes.len() as i64);
        if let Some(p) = p {
            self.emit(Op::Print(p, n), None, Span::new(0, 0));
        }
    }

    fn intr_addr_of(&mut self, e: &'a Expr, args: &'a [Arg]) -> Res {
        let Some(a) = args.first() else {
            return self.unsupported(e.span, "`rtAddrOf` without an argument");
        };
        let arg = &a.value;
        let Some(t) = self.ty_of(arg) else {
            return self.unsupported(arg.span, "`rtAddrOf` of an unknown type");
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

impl<'a> Lowerer<'a> {
    /// The OS memory calls (kept out of `intrinsic_call`, whose stack frame is already large in debug builds).
    #[inline(never)]
    fn intr_os(&mut self, name: &str, args: &'a [Arg], sp: Span) -> Res {
        let kind = match name {
            "rtOsWrite" => crate::ir::OsKind::Write,
            "rtOsRead" => crate::ir::OsKind::Read,
            "rtOsExit" => crate::ir::OsKind::Exit,
            "rtOsArgv" => crate::ir::OsKind::Argv,
            "rtOsOpen" => crate::ir::OsKind::Open,
            "rtOsClose" => crate::ir::OsKind::Close,
            "rtOsClock" => crate::ir::OsKind::Clock,
            "rtOsSleep" => crate::ir::OsKind::Sleep,
            "rtAtomicCas" => crate::ir::OsKind::AtomicCas,
            "rtAtomicAdd" => crate::ir::OsKind::AtomicAdd,
            "rtAtomicSwap" => crate::ir::OsKind::AtomicSwap,
            "rtThreadStart" => crate::ir::OsKind::ThreadStart,
            "rtFutexWait" => crate::ir::OsKind::FutexWait,
            "rtFutexWake" => crate::ir::OsKind::FutexWake,
            "rtOsFs" => crate::ir::OsKind::Fs,
            "rtOsDents" => crate::ir::OsKind::Dents,
            "rtOsRandom" => crate::ir::OsKind::Random,
            "rtOsReserve" => crate::ir::OsKind::Reserve,
            "rtOsCommit" => crate::ir::OsKind::Commit,
            "rtOsRelease" => crate::ir::OsKind::Release,
            "rtOsRemap" => crate::ir::OsKind::Remap,
            _ => crate::ir::OsKind::AdviseNoHuge,
        };
        let mut vs = Vec::new();
        for i in 0..kind.arity() {
            let Some(v) = self.arg_scalar(args, i) else {
                return Res::Never;
            };
            // An address travels as an `i64`: the first operand of the memory calls, the second of the
            // descriptor calls.
            let addr_at = if matches!(
                kind,
                crate::ir::OsKind::Write
                    | crate::ir::OsKind::Read
                    | crate::ir::OsKind::Clock
                    | crate::ir::OsKind::Fs
                    | crate::ir::OsKind::Dents
            ) {
                1
            } else {
                0
            };
            let v = if i == addr_at
                && !matches!(
                    kind,
                    crate::ir::OsKind::Reserve
                        | crate::ir::OsKind::Exit
                        | crate::ir::OsKind::Close
                        | crate::ir::OsKind::Sleep
                ) {
                let Some(n) = self.emit(
                    Op::Cast(CastKind::PtrToInt, v, IrTy::I64),
                    Some(IrTy::I64),
                    sp,
                ) else {
                    return Res::Never;
                };
                n
            } else {
                v
            };
            vs.push(v);
        }
        if vs.is_empty() {
            // `os_argv` reads nothing; the operands are placeholders.
            let Some(z) = self.emit(Op::Const(IrTy::I64, Const::Int(0)), Some(IrTy::I64), sp)
            else {
                return Res::Never;
            };
            vs.push(z);
        }
        let (a, b, c) = (
            vs[0],
            *vs.get(1).unwrap_or(&vs[0]),
            *vs.get(2).unwrap_or(&vs[0]),
        );
        if kind == crate::ir::OsKind::Exit {
            self.emit(Op::Os(kind, a, b, c), Some(IrTy::I64), sp);
            return Res::Unit;
        }
        self.wrap(Op::Os(kind, a, b, c), IrTy::I64, sp)
    }
}

/// The library intrinsics and runtime functions are named `rt` and a capital: `rtAlloc`, `rtLoadInt`.
pub(crate) fn is_rt_name(name: &str) -> bool {
    name.strip_prefix("rt")
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c.is_ascii_uppercase())
}
