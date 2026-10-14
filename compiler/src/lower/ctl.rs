//! `?`, `??`, `?.`, `copy` and `in`: operators that look at an `option` or `result`, or copy a value.

use super::*;

impl<'a> Lowerer<'a> {
    // ---- `?` --------------------------------------------------------------------------------------------------------

    /// `value?`: the payload on success; on failure the function returns early with the error (or
    /// `none`), after the scopes it leaves have run their drops.
    pub(crate) fn try_expr(&mut self, e: &'a Expr, inner: &'a Expr, dst: Option<Value>) -> Res {
        let Some(it) = self.ty_of(inner) else {
            return self.unsupported(e.span, "`?` on a value of unknown type");
        };
        let Some(info) = self.enum_info(it) else {
            return self.unsupported(e.span, "`?` on this type");
        };
        let is_result = matches!(self.kind(it), TyKind::Result(..));
        // The value is built here and consumed: the payload moves out or into the return value.
        let tmp = self.slot_for(it);
        if let Res::Never = self.into(inner, tmp, it) {
            return Res::Never;
        }
        let tag = self.load_at(tmp, 0, info.tag);
        // Result: ok is 0. Option: some is 1.
        let good = self.konst(info.tag, Const::Int(if is_result { 0 } else { 1 }), e.span);
        let Some(c) = self.emit(Op::Cmp(CmpOp::Eq, tag, good), Some(IrTy::Bool), e.span) else {
            return Res::Never;
        };
        let ok_b = self.new_block(&[]);
        let bad_b = self.new_block(&[]);
        self.branch(c, ok_b, bad_b);
        // The failure path.
        self.start(bad_b);
        if !self.propagate(e, tmp, it, is_result) {
            return Res::Never;
        }
        // The success path: the payload.
        self.start(ok_b);
        let payload = info.variants[if is_result { 0 } else { 1 }].first().copied();
        let Some(pf) = payload else {
            return Res::Unit;
        };
        let p = self.addr(tmp, pf.off);
        match self.repr(pf.ty) {
            Repr::Scalar(s) => match self.emit(Op::Load(s, p), Some(s), e.span) {
                Some(v) => {
                    if self.needs_drop(pf.ty) {
                        // An owning scalar (a box) moves out; the caller owns it now.
                    }
                    Res::Val(v)
                }
                None => Res::Never,
            },
            Repr::Mem(_) => match dst {
                Some(d) => {
                    self.copy_mem(d, p, pf.ty);
                    Res::Unit
                }
                None => {
                    let slot = self.slot_for(pf.ty);
                    self.copy_mem(slot, p, pf.ty);
                    self.own(slot, pf.ty, false);
                    Res::Val(slot)
                }
            },
            Repr::Zst => Res::Unit,
        }
    }

    /// Writes the failure value for `?` into the function's return value and returns.
    fn propagate(&mut self, e: &'a Expr, tmp: Value, it: Ty, is_result: bool) -> bool {
        let ret = self.f.ret_ty;
        let Some(sret) = self.f.sret else {
            self.unsupported(e.span, "`?` in a function that does not return a result or option");
            return false;
        };
        let Some(rinfo) = self.enum_info(ret) else {
            self.unsupported(e.span, "`?` in a function that does not return a result or option");
            return false;
        };
        let Some(info) = self.enum_info(it) else { return false };
        if is_result {
            let ret_err = match self.kind(ret) {
                TyKind::Result(_, b) => b,
                _ => {
                    self.unsupported(e.span, "`?` on a result in a function that returns something else");
                    return false;
                }
            };
            let Some(from) = info.variants[1].first().copied() else { return false };
            let src = self.addr(tmp, from.off);
            let Some(to) = rinfo.variants[1].first().copied() else { return false };
            let dstp = self.addr(sret, to.off);
            if from.ty == ret_err {
                self.copy_mem_or_scalar(dstp, src, from.ty);
            } else if matches!(self.kind(ret_err), TyKind::Error) {
                // Any error becomes the catch-all: its text is the message.
                let string = self.types.string;
                let z = self.i64c(0);
                let np = self.emit(Op::Cast(CastKind::IntToPtr, z, IrTy::Ptr), Some(IrTy::Ptr), e.span).unwrap_or(dstp);
                self.store_at(dstp, 0, np);
                let z1 = self.i64c(0);
                self.store_at(dstp, 8, z1);
                let z2 = self.i64c(0);
                self.store_at(dstp, 16, z2);
                let _ = string;
                if !self.show_at(src, from.ty, dstp, false) {
                    return false;
                }
                self.drop_at(src, from.ty);
            } else if let Some((m, map)) = self.call_target(e.span) {
                // `impl converts<...> for ...`
                let Some(f) = self.request_fn(m, &map) else { return false };
                self.emit(Op::Call(f, vec![dstp, src]), None, e.span);
            } else {
                self.unsupported(e.span, "converting this error for `?`");
                return false;
            }
            let tagv = self.konst(rinfo.tag, Const::Int(1), e.span);
            self.store_at(sret, 0, tagv);
        } else {
            let tagv = self.konst(rinfo.tag, Const::Int(0), e.span);
            self.store_at(sret, 0, tagv);
        }
        self.exit_scopes(0);
        self.terminate(Term::Return(None));
        true
    }

    /// Copies a value of any type that fits `t` (a scalar or memory) between addresses.
    pub(crate) fn copy_mem_or_scalar(&mut self, dst: Value, src: Value, t: Ty) {
        match self.repr(t) {
            Repr::Scalar(s) => {
                let v = self.load_at(src, 0, s);
                self.store_scalar_ty(dst, v);
            }
            Repr::Mem(_) => self.copy_mem(dst, src, t),
            Repr::Zst => {}
        }
    }

    // ---- `??` -------------------------------------------------------------------------------------------------------

    /// `a ?? b`: the value inside `a`, or `b` when `a` is `none`.
    pub(crate) fn default_expr(&mut self, e: &'a Expr, lhs: &'a Expr, rhs: &'a Expr, dst: Option<Value>, want: Ty) -> Res {
        let _ = want;
        let (Some(lt), Some(rt)) = (self.ty_of(lhs), self.ty_of(e)) else {
            return self.unsupported(e.span, "`??` on values of unknown type");
        };
        let Some(info) = self.enum_info(lt) else {
            return self.unsupported(e.span, "`??` on this type");
        };
        let tmp = self.slot_for(lt);
        if let Res::Never = self.into(lhs, tmp, lt) {
            return Res::Never;
        }
        let tag = self.load_at(tmp, 0, info.tag);
        let zero = self.konst(info.tag, Const::Int(0), e.span);
        let Some(is_none) = self.emit(Op::Cmp(CmpOp::Eq, tag, zero), Some(IrTy::Bool), e.span) else {
            return Res::Never;
        };
        let repr = self.repr(rt);
        let params: Vec<IrTy> = match repr {
            Repr::Scalar(s) => vec![s],
            _ => vec![],
        };
        let join = self.new_block(&params);
        let none_b = self.new_block(&[]);
        let some_b = self.new_block(&[]);
        self.branch(is_none, none_b, some_b);
        let slot = match (repr, dst) {
            (Repr::Mem(_), None) => Some(self.slot_for(rt)),
            (Repr::Mem(_), d) => d,
            _ => None,
        };
        // some: the payload moves out (or, when the result is itself an option, the whole value).
        self.start(some_b);
        let same_shape = rt == lt;
        let payload = info.variants[1].first().copied();
        let some_val = match (repr, slot) {
            (Repr::Mem(_), Some(d)) => {
                if same_shape {
                    self.copy_mem(d, tmp, lt);
                } else if let Some(pf) = payload {
                    let p = self.addr(tmp, pf.off);
                    self.copy_mem(d, p, pf.ty);
                }
                None
            }
            (Repr::Scalar(s), _) => payload.map(|pf| self.load_at(tmp, pf.off, s)),
            _ => None,
        };
        match some_val {
            Some(v) => self.jump(join, vec![v]),
            None => self.jump(join, vec![]),
        }
        // none: the default.
        self.start(none_b);
        self.push_scope();
        let r = match (repr, slot) {
            (Repr::Mem(_), Some(d)) => self.into(rhs, d, rt),
            _ => self.expr(rhs),
        };
        let reached = match r {
            Res::Never => false,
            Res::Val(v) if matches!(repr, Repr::Scalar(_)) => {
                self.pop_scope();
                self.jump(join, vec![v]);
                true
            }
            _ if matches!(repr, Repr::Scalar(_)) => {
                self.f.scopes.pop();
                false
            }
            _ => {
                self.pop_scope();
                self.jump(join, vec![]);
                true
            }
        };
        let _ = reached;
        self.start(join);
        match repr {
            Repr::Scalar(_) => Res::Val(self.f.func.block_params(join)[0]),
            Repr::Mem(_) => match dst {
                Some(_) => Res::Unit,
                None => {
                    if let Some(d) = slot {
                        self.own(d, rt, false);
                        Res::Val(d)
                    } else {
                        Res::Unit
                    }
                }
            },
            Repr::Zst => Res::Unit,
        }
    }

    // ---- `copy` -------------------------------------------------------------------------------------------------------

    pub(crate) fn copy_expr(&mut self, e: &'a Expr, inner: &'a Expr, dst: Option<Value>) -> Res {
        let Some(t) = self.ty_of(e) else {
            return self.unsupported(e.span, "`copy` of a value of unknown type");
        };
        match self.repr(t) {
            Repr::Mem(_) => {
                let Res::Val(src) = self.expr(inner) else { return Res::Never };
                self.with_dst(dst, t, |s, d| {
                    s.clone_into(src, d, t);
                    Res::Unit
                })
            }
            Repr::Scalar(sc) => {
                if self.needs_drop(t) {
                    let slot = self.slot_for(t);
                    let src = match self.place(inner) {
                        Some(p) => p,
                        None => match self.scalar(inner) {
                            Some(v) => {
                                let tmp = self.slot_for(t);
                                self.store_scalar_ty(tmp, v);
                                tmp
                            }
                            None => return Res::Never,
                        },
                    };
                    self.clone_into(src, slot, t);
                    Res::Val(self.load_at(slot, 0, sc))
                } else {
                    match self.scalar(inner) {
                        Some(v) => Res::Val(v),
                        None => Res::Never,
                    }
                }
            }
            Repr::Zst => Res::Unit,
        }
    }

    // ---- `in` -----------------------------------------------------------------------------------------------------------------

    pub(crate) fn in_expr(&mut self, e: &'a Expr, lhs: &'a Expr, rhs: &'a Expr) -> Res {
        let rhs = super::expr::strip_parens(rhs);
        if let ExprKind::Range { start, end, inclusive, step: None } = &rhs.kind {
            let Some(t) = self.ty_of(lhs) else {
                return self.unsupported(e.span, "`in` on a value of unknown type");
            };
            let signed = self.is_signed(t) || self.scalar_of(t).is_some_and(|s| s.is_float());
            let Some(x) = self.scalar(lhs) else { return Res::Never };
            let mut cond: Option<Value> = None;
            if let Some(lo) = start {
                let Some(lo) = self.scalar(lo) else { return Res::Never };
                let op = if signed { CmpOp::Ge } else { CmpOp::UGe };
                cond = self.emit(Op::Cmp(op, x, lo), Some(IrTy::Bool), e.span);
            }
            let low_ok = cond;
            let hi_ok = match end {
                Some(hi) => {
                    let Some(hi) = self.scalar(hi) else { return Res::Never };
                    let op = match (*inclusive, signed) {
                        (true, true) => CmpOp::Le,
                        (true, false) => CmpOp::ULe,
                        (false, true) => CmpOp::Lt,
                        (false, false) => CmpOp::ULt,
                    };
                    self.emit(Op::Cmp(op, x, hi), Some(IrTy::Bool), e.span)
                }
                None => None,
            };
            return match (low_ok, hi_ok) {
                (Some(a), Some(b)) => {
                    // Both bounds: `a && b` without branching on a pure comparison.
                    let join = self.new_block(&[IrTy::Bool]);
                    let more = self.new_block(&[]);
                    self.terminate(Term::Branch(a, Target { block: more, args: vec![] }, Target { block: join, args: vec![a] }));
                    self.start(more);
                    self.jump(join, vec![b]);
                    self.start(join);
                    Res::Val(self.f.func.block_params(join)[0])
                }
                (Some(a), None) | (None, Some(a)) => Res::Val(a),
                (None, None) => Res::Val(self.konst(IrTy::Bool, Const::Bool(true), e.span)),
            };
        }
        let (Some(lt), Some(rt)) = (self.ty_of(lhs), self.ty_of(rhs)) else {
            return self.unsupported(e.span, "`in` on values of unknown type");
        };
        match self.kind(rt) {
            TyKind::List(el) | TyKind::Iter(el) => {
                let Res::Val(h) = self.expr(rhs) else { return Res::Never };
                let probe = self.probe_addr(lhs, lt, el);
                let Some(probe) = probe else { return Res::Never };
                let size = self.layout(el).size;
                let sz = self.i64c(size as i64);
                let Some(eq) = self.glue_addr(GlueKind::Eq, el) else { return Res::Never };
                match self.call_core("rt_list_contains", vec![h, probe, sz, eq]) {
                    Some(v) => Res::Val(v),
                    None => Res::Never,
                }
            }
            TyKind::String | TyKind::Str => {
                let Res::Val(h) = self.expr(rhs) else { return Res::Never };
                let Res::Val(p) = self.expr(lhs) else { return Res::Never };
                match self.call_core("rt_str_contains", vec![h, p]) {
                    Some(v) => Res::Val(v),
                    None => Res::Never,
                }
            }
            _ => self.in_collection(e, lhs, rhs, lt, rt),
        }
    }

    /// An address holding the left side of `in`, converted to the element type if needed.
    fn probe_addr(&mut self, lhs: &'a Expr, lt: Ty, el: Ty) -> Option<Value> {
        if lt == el {
            match self.repr(lt) {
                Repr::Scalar(_) => {
                    let v = self.scalar(lhs)?;
                    let slot = self.slot_for(lt);
                    self.store_scalar_ty(slot, v);
                    Some(slot)
                }
                _ => match self.expr(lhs) {
                    Res::Val(p) => Some(p),
                    _ => None,
                },
            }
        } else {
            let tmp = self.slot_for(el);
            match self.into(lhs, tmp, el) {
                Res::Never => None,
                _ => {
                    self.own(tmp, el, false);
                    Some(tmp)
                }
            }
        }
    }

    /// The address of a glue function for a type (null when it needs none).
    pub(crate) fn glue_addr(&mut self, kind: GlueKind, t: Ty) -> Option<Value> {
        match self.glue_fn(kind, t) {
            Some(f) => self.emit(Op::FuncAddr(f), Some(IrTy::Ptr), Span::new(0, 0)),
            None => {
                let z = self.i64c(0);
                self.emit(Op::Cast(CastKind::IntToPtr, z, IrTy::Ptr), Some(IrTy::Ptr), Span::new(0, 0))
            }
        }
    }
}
