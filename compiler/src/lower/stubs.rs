//! Placeholders for the parts of the lowering still to be written; each reports "not lowered yet".

use super::*;

impl<'a> Lowerer<'a> {
    pub(crate) fn default_expr(&mut self, e: &'a Expr, _l: &'a Expr, _r: &'a Expr, _dst: Option<Value>, _t: Ty) -> Res {
        self.unsupported(e.span, "`??`")
    }
    pub(crate) fn closure_value(&mut self, e: &'a Expr, _c: &'a Closure, _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "a closure")
    }
    pub(crate) fn convert_mem(&mut self, e: &'a Expr, _v: &'a Expr, _c: bool, _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "this conversion")
    }
    pub(crate) fn builder_expr(&mut self, e: &'a Expr, _b: &'a Builder, _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "a builder")
    }
    pub(crate) fn map_lit(&mut self, e: &'a Expr, _i: &'a [MapItem], _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "a map")
    }
    pub(crate) fn set_lit(&mut self, e: &'a Expr, _i: &'a [ListItem], _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "a set")
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
    pub(crate) fn try_expr(&mut self, e: &'a Expr, _i: &'a Expr, _dst: Option<Value>) -> Res {
        self.unsupported(e.span, "`?`")
    }
    pub(crate) fn copy_expr(&mut self, e: &'a Expr, _i: &'a Expr, _dst: Option<Value>) -> Res {
        self.unsupported(e.span, "`copy`")
    }
    pub(crate) fn coerce_to_any(&mut self, _e: &'a Expr, _have: Ty, _dst: Value, _iface: DefId) -> Option<Res> {
        None
    }
    pub(crate) fn fn_as_value(&mut self, e: &'a Expr, _def: DefId) -> Res {
        self.unsupported(e.span, "a function used as a value")
    }
    pub(crate) fn obj_literal(&mut self, e: &'a Expr, _i: &'a [StructInit], _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "an object")
    }
    pub(crate) fn eq_collection(&mut self, _a: Value, _b: Value, _ty: Ty, _fail: BlockId) -> bool {
        false
    }
    pub(crate) fn show_collection(&mut self, _v: Value, _out: Value, _ty: Ty) {
        self.unsupported(Span::new(0, 0), "printing a map or set");
    }
}
