//! Placeholders for the parts of the lowering still to be written; each reports "not lowered yet".

use super::*;

impl<'a> Lowerer<'a> {
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
