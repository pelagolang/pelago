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
}
