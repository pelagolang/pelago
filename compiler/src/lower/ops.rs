//! Operators written by the program (`impl add for vec`), the pipe operator and conversions.

use super::*;
use super::call::ArgSource;

impl<'a> Lowerer<'a> {
    /// `a + b`, `-a` or `a += b` on a user type: a call to the operator's method, when the checker
    /// recorded one for this expression.
    pub(crate) fn user_operator(&mut self, e: &'a Expr, _span: Span) -> Option<Res> {
        let (def, map) = self.call_target(e.span)?;
        match &e.kind {
            ExprKind::Binary { lhs, rhs, .. } => {
                let srcs = vec![ArgSource::Expr(lhs), ArgSource::Expr(rhs)];
                Some(self.call_with(e, def, &map, srcs, None))
            }
            ExprKind::Unary { operand, .. } => {
                let srcs = vec![ArgSource::Expr(operand)];
                Some(self.call_with(e, def, &map, srcs, None))
            }
            ExprKind::Assign { lhs, rhs, .. } => {
                let lt = self.ty_of(lhs)?;
                let p = self.place(lhs)?;
                let tmp = self.slot_for(lt);
                let srcs = vec![ArgSource::Expr(lhs), ArgSource::Expr(rhs)];
                match self.call_with(e, def, &map, srcs, Some(tmp)) {
                    Res::Never => Some(Res::Never),
                    Res::Val(v) => {
                        // A result that is a plain scalar.
                        let flag = self.local_flag(lhs);
                        self.emit_drop(p, lt, flag);
                        self.emit(Op::Store(p, v), None, e.span);
                        Some(Res::Unit)
                    }
                    _ => {
                        let flag = self.local_flag(lhs);
                        self.emit_drop(p, lt, flag);
                        self.copy_mem(p, tmp, lt);
                        if let Some(f) = flag {
                            let one = self.konst(IrTy::Bool, Const::Bool(true), e.span);
                            self.emit(Op::Store(f, one), None, e.span);
                        }
                        Some(Res::Unit)
                    }
                }
            }
            _ => None,
        }
    }

    pub(crate) fn user_operator_into(&mut self, e: &'a Expr, dst: Value) -> Option<Res> {
        let (def, map) = self.call_target(e.span)?;
        match &e.kind {
            ExprKind::Binary { lhs, rhs, .. } => {
                let srcs = vec![ArgSource::Expr(lhs), ArgSource::Expr(rhs)];
                Some(self.call_with(e, def, &map, srcs, Some(dst)))
            }
            ExprKind::Unary { operand, .. } => {
                let srcs = vec![ArgSource::Expr(operand)];
                Some(self.call_with(e, def, &map, srcs, Some(dst)))
            }
            _ => None,
        }
    }

    // ---- Conversions: `x to T` ----------------------------------------------------------------------------

    /// A conversion the program wrote with `converts`: a call to its method.
    pub(crate) fn user_convert(&mut self, e: &'a Expr, value: &'a Expr, dst: Option<Value>) -> Option<Res> {
        let (def, map) = self.call_target(e.span)?;
        let srcs = vec![ArgSource::Expr(value)];
        Some(self.call_with(e, def, &map, srcs, dst))
    }

    /// A conversion whose result lives in memory (`to string`, `to str`, upcasts).
    pub(crate) fn convert_mem(
        &mut self,
        e: &'a Expr,
        v: &'a Expr,
        checked: bool,
        dst: Value,
        t: Ty,
    ) -> Res {
        if let Some(r) = self.user_convert(e, v, Some(dst)) {
            return r;
        }
        if checked {
            return self.downcast(e, v, dst, t);
        }
        let Some(from) = self.ty_of(v) else {
            return self.unsupported(e.span, "this conversion");
        };
        match (self.kind(from), self.kind(t)) {
            (TyKind::String, TyKind::String) => self.into(v, dst, t),
            (_, TyKind::String) => {
                let z = self.i64c(0);
                let np = self
                    .emit(Op::Cast(CastKind::IntToPtr, z, IrTy::Ptr), Some(IrTy::Ptr), e.span)
                    .unwrap_or(dst);
                self.store_at(dst, 0, np);
                let z1 = self.i64c(0);
                self.store_at(dst, 8, z1);
                let z2 = self.i64c(0);
                self.store_at(dst, 16, z2);
                if self.show_expr(v, dst) {
                    Res::Unit
                } else {
                    Res::Never
                }
            }
            (TyKind::String, TyKind::Str) => {
                let Res::Val(p) = self.expr(v) else {
                    return Res::Never;
                };
                let d = self.load_at(p, 0, IrTy::Ptr);
                let n = self.load_at(p, 8, IrTy::I64);
                self.store_at(dst, 0, d);
                self.store_at(dst, 8, n);
                Res::Unit
            }
            _ => self.into(v, dst, t),
        }
    }

    // ---- `?.` ---------------------------------------------------------------------------------------------

    /// Tests an optional base; on `none` writes `none` to `dst` and jumps to `done`. Gives the address
    /// of the payload when the base is there. The caller continues in the `some` block.
    fn opt_base(
        &mut self,
        base: &'a Expr,
        dst: Value,
        t: Ty,
        done: BlockId,
    ) -> Option<(Value, Ty)> {
        let sp = base.span;
        let (p, bt) = self.scrutinee(base)?;
        let TyKind::Option(inner) = self.kind(bt) else {
            self.unsupported(sp, "`?.` on a value that is not an option");
            return None;
        };
        let binfo = self.enum_info(bt)?;
        let tinfo = self.enum_info(t)?;
        let tag = self.load_at(p, 0, binfo.tag);
        let zero = self.konst(binfo.tag, Const::Int(0), sp);
        let is_none = self.emit(Op::Cmp(CmpOp::Eq, tag, zero), Some(IrTy::Bool), sp)?;
        let none_b = self.new_block(&[]);
        let some_b = self.new_block(&[]);
        self.branch(is_none, none_b, some_b);
        self.start(none_b);
        let z = self.konst(tinfo.tag, Const::Int(0), sp);
        self.store_at(dst, 0, z);
        self.jump(done, vec![]);
        self.start(some_b);
        let off = binfo.variants.get(1)?.first()?.off;
        Some((self.addr(p, off), inner))
    }

    /// `a?.b`: the field of the value inside `a`, or `none`.
    pub(crate) fn opt_field(
        &mut self,
        e: &'a Expr,
        base: &'a Expr,
        name: &Ident,
        dst: Value,
        t: Ty,
    ) -> Res {
        let sp = e.span;
        let done = self.new_block(&[]);
        let Some((q, inner)) = self.opt_base(base, dst, t, done) else {
            return Res::Never;
        };
        let TyKind::Adt(d, _) = self.kind(inner) else {
            return self.unsupported(sp, "`?.` on this type");
        };
        let fname = self.src_text(name.span).to_string();
        let Some(idx) = self
            .typed
            .adts
            .get(&d)
            .and_then(|a| a.fields.iter().position(|(fd, _)| self.res.def(*fd).name == fname))
        else {
            return self.unsupported(name.span, "an unknown field");
        };
        let fs = self.struct_fields(inner);
        let Some(f) = fs.get(idx).copied() else {
            return Res::Never;
        };
        let obj = self.res.def(d).kind == DefKind::Obj;
        let fbase = if obj {
            let o = self.load_at(q, 0, IrTy::Ptr);
            self.addr(o, super::obj::OBJ_HEADER)
        } else {
            q
        };
        let faddr = self.addr(fbase, f.off);
        self.some_from(faddr, f.ty, dst, t);
        self.jump(done, vec![]);
        self.start(done);
        Res::Unit
    }

    /// Writes `some(copy of the value at src)` to `dst` (or the value itself when it is already an option).
    fn some_from(&mut self, src: Value, ft: Ty, dst: Value, t: Ty) {
        let sp = Span::new(0, 0);
        if matches!(self.kind(ft), TyKind::Option(_)) {
            self.clone_into(src, dst, ft);
            return;
        }
        if let Some(info) = self.enum_info(t) {
            if let Some(pf) = info.variants.get(1).and_then(|v| v.first()) {
                let p = self.addr(dst, pf.off);
                self.clone_into(src, p, ft);
            }
            let one = self.konst(info.tag, Const::Int(1), sp);
            self.store_at(dst, 0, one);
        }
    }

    /// `a?.method(args)`: calls the method on the value inside `a`, or gives `none`.
    pub(crate) fn opt_call(
        &mut self,
        e: &'a Expr,
        base: &'a Expr,
        args: &'a [Arg],
        dst: Option<Value>,
    ) -> Res {
        let sp = e.span;
        let Some(t) = self.ty_of(e) else {
            return self.unsupported(sp, "`?.` call of unknown type");
        };
        let Some((def, map)) = self.call_target(sp) else {
            return self.unsupported(sp, "a `?.` call the checker did not record");
        };
        let d = match dst {
            Some(d) => d,
            None => self.slot_for(t),
        };
        let done = self.new_block(&[]);
        let Some((q, _inner)) = self.opt_base(base, d, t, done) else {
            return Res::Never;
        };
        let Some(info) = self.typed.fn_infos.get(&def).cloned() else {
            return self.unsupported(sp, "a function without a signature");
        };
        let ret = self.conc_with(info.ret, &map);
        let mut srcs = vec![ArgSource::Addr(q)];
        match self.match_args(&info, def, args, sp) {
            Some(mut v) => srcs.append(&mut v),
            None => return Res::Never,
        }
        let Some(tinfo) = self.enum_info(t) else {
            return Res::Never;
        };
        let wrapped = !matches!(self.kind(ret), TyKind::Option(_));
        let payload = tinfo.variants.get(1).and_then(|v| v.first()).copied();
        let target = if wrapped {
            payload.map(|pf| self.addr(d, pf.off))
        } else {
            Some(d)
        };
        match self.repr(ret) {
            Repr::Mem(_) => {
                if let Res::Never = self.call_with(e, def, &map, srcs, target) {
                    return Res::Never;
                }
            }
            Repr::Scalar(_) => match self.call_with(e, def, &map, srcs, None) {
                Res::Val(v) => {
                    if let Some(p) = target {
                        self.emit(Op::Store(p, v), None, sp);
                    }
                }
                _ => return Res::Never,
            },
            Repr::Zst => {
                if let Res::Never = self.call_with(e, def, &map, srcs, None) {
                    return Res::Never;
                }
            }
        }
        if wrapped {
            let one = self.konst(tinfo.tag, Const::Int(1), sp);
            self.store_at(d, 0, one);
        }
        self.jump(done, vec![]);
        self.start(done);
        match dst {
            Some(_) => Res::Unit,
            None => {
                self.own(d, t, false);
                Res::Val(d)
            }
        }
    }
}
