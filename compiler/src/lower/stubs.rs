//! Placeholders for the parts of the lowering still to be written; each reports "not lowered yet".

use super::*;

impl<'a> Lowerer<'a> {
    pub(crate) fn coerce_into(&mut self, _e: &'a Expr, _have: Ty, _dst: Value, _want: Ty) -> Option<Res> {
        None
    }
    pub(crate) fn global_value(&mut self, e: &Expr, _def: DefId) -> Res {
        self.unsupported(e.span, "a global or a function used as a value")
    }
    pub(crate) fn global_or_variant(&mut self, e: &'a Expr) -> Res {
        self.unsupported(e.span, "a path")
    }
    pub(crate) fn variant_value(&mut self, e: &'a Expr, _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "an enum case")
    }
    pub(crate) fn tuple_lit(&mut self, e: &'a Expr, _es: &'a [Expr], _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "a tuple")
    }
    pub(crate) fn struct_lit(&mut self, e: &'a Expr, _f: &'a [StructInit], _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "a struct literal")
    }
    pub(crate) fn string_lit(&mut self, e: &'a Expr, _p: &'a [StrPart], _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "text")
    }
    pub(crate) fn list_lit(&mut self, e: &'a Expr, _i: &'a [ListItem], _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "a list")
    }
    pub(crate) fn none_lit(&mut self, e: &'a Expr, _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "`none`")
    }
    pub(crate) fn slice_expr(&mut self, e: &'a Expr, _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "a slice")
    }
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
    pub(crate) fn repeat_lit(&mut self, e: &'a Expr, _v: &'a Expr, _c: &'a Expr, _dst: Value, _t: Ty) -> Res {
        self.unsupported(e.span, "a repeated list")
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
    pub(crate) fn compare_mem(&mut self, e: &'a Expr, _op: Punct, _l: &'a Expr, _r: &'a Expr, _t: Ty) -> Res {
        self.unsupported(e.span, "comparing this type")
    }
    pub(crate) fn concat(&mut self, e: &'a Expr, _l: &'a Expr, _r: &'a Expr, _dst: Value) -> Res {
        self.unsupported(e.span, "joining text")
    }
    pub(crate) fn append_assign(&mut self, e: &'a Expr, _op: Punct, _p: Value, _t: Ty, _rhs: &'a Expr) -> Res {
        self.unsupported(e.span, "this assignment")
    }
    pub(crate) fn destructure_decl(&mut self, d: &'a Decl, _init: &'a Expr, _t: Ty) {
        self.unsupported(d.pattern.span, "a pattern in a declaration");
    }
    pub(crate) fn try_expr(&mut self, e: &'a Expr, _i: &'a Expr, _dst: Option<Value>) -> Res {
        self.unsupported(e.span, "`?`")
    }
    pub(crate) fn copy_expr(&mut self, e: &'a Expr, _i: &'a Expr, _dst: Option<Value>) -> Res {
        self.unsupported(e.span, "`copy`")
    }
    pub(crate) fn guard_expr(&mut self, c: &'a Expr, _e: &'a Block) -> Res {
        self.unsupported(c.span, "`guard`")
    }
    pub(crate) fn switch_expr(&mut self, e: &'a Expr, _s: &'a Switch, _dst: Option<Value>, _t: Ty) -> Res {
        self.unsupported(e.span, "`switch`")
    }
    pub(crate) fn is_expr(&mut self, e: &'a Expr, _v: &'a Expr, _p: &'a Pattern) -> Res {
        self.unsupported(e.span, "`is`")
    }
    pub(crate) fn cond_branch(&mut self, c: &'a Expr, yes: BlockId, no: BlockId) {
        if let Some(v) = self.scalar(c) {
            self.branch(v, yes, no);
        }
    }
    pub(crate) fn is_intrinsic(&self, _def: DefId) -> bool {
        false
    }
    pub(crate) fn intrinsic_call(&mut self, e: &'a Expr, _d: DefId, _m: &HashMap<DefId, Ty>, _r: Option<&'a Expr>, _a: &'a [Arg], _dst: Option<Value>) -> Res {
        self.unsupported(e.span, "a library function")
    }
    pub(crate) fn intrinsic_by_name(&mut self, e: &'a Expr, _n: &str, _a: &'a [Arg], _dst: Option<Value>) -> Res {
        self.unsupported(e.span, "a built-in function")
    }
    pub(crate) fn enum_ctor(&mut self, e: &'a Expr, _t: Ty, _o: DefId, _v: DefId, _a: &'a [Arg], _dst: Value) -> Res {
        self.unsupported(e.span, "an enum case")
    }
    pub(crate) fn enum_ctor_by_name(&mut self, e: &'a Expr, _t: Ty, _n: &str, _a: &'a [Arg], _dst: Option<Value>) -> Res {
        self.unsupported(e.span, "an enum case")
    }
    pub(crate) fn result_ctor(&mut self, e: &'a Expr, _err: bool, _a: &'a [Arg], _dst: Option<Value>) -> Res {
        self.unsupported(e.span, "`ok` / `err`")
    }
    pub(crate) fn for_each(&mut self, _l: Option<String>, _p: &'a [Pattern], it: &'a Expr, _b: &'a Block) -> Res {
        self.unsupported(it.span, "looping over this")
    }
    pub(crate) fn lower_intrinsic_job(&mut self, _id: FuncId, _def: DefId, _s: &HashMap<DefId, Ty>) {}
    pub(crate) fn lower_glue_job(&mut self, _id: FuncId, _k: GlueKind, _t: Ty) {}
    pub(crate) fn glue_fn(&mut self, _k: GlueKind, _t: Ty) -> Option<FuncId> {
        None
    }
}
