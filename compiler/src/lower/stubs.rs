//! Placeholders for the parts of the lowering still to be written; each reports "not lowered yet".

use super::*;

impl<'a> Lowerer<'a> {
    pub(crate) fn convert_mem(
        &mut self,
        e: &'a Expr,
        _v: &'a Expr,
        _c: bool,
        _dst: Value,
        _t: Ty,
    ) -> Res {
        self.unsupported(e.span, "this conversion")
    }
    pub(crate) fn builder_expr(
        &mut self,
        e: &'a Expr,
        _b: &'a Builder,
        _dst: Value,
        _t: Ty,
    ) -> Res {
        self.unsupported(e.span, "a builder")
    }
    pub(crate) fn range_value(&mut self, e: &'a Expr, _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "a range value")
    }
    pub(crate) fn user_operator(&mut self, _e: &'a Expr, _span: Span) -> Option<Res> {
        None
    }
    pub(crate) fn user_operator_into(&mut self, _e: &'a Expr, _dst: Value) -> Option<Res> {
        None
    }
    pub(crate) fn coerce_to_any(
        &mut self,
        _e: &'a Expr,
        _have: Ty,
        _dst: Value,
        _iface: DefId,
    ) -> Option<Res> {
        None
    }
    pub(crate) fn obj_literal(
        &mut self,
        e: &'a Expr,
        _i: &'a [StructInit],
        _dst: Value,
        _t: Ty,
    ) -> Res {
        self.unsupported(e.span, "an object")
    }
}
