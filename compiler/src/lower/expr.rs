//! Expressions: values, places, operators and control flow.
//!
//! Three ways to evaluate an expression:
//!
//! - [`Lowerer::expr`] gives a scalar value, or for a memory type the *address* of the value (a place
//!   is read where it is; anything else is built in a temporary that is dropped at the end of the
//!   statement). Nothing is moved.
//! - [`Lowerer::into`] builds an owned value at an address. A place is copied there, and when the type
//!   owns something the variable's flag is cleared (the value moved).
//! - [`Lowerer::place`] gives the address of something that can be assigned to.

use super::*;
use crate::typecheck::literal_value;

/// What a `for` runs on each turn.
pub(crate) enum LoopBody<'a, 'b> {
    Block(&'a Block),
    Code(&'b mut dyn FnMut(&mut Lowerer<'a>) -> Res),
}

impl<'a> Lowerer<'a> {
    // ---- Small helpers --------------------------------------------------------------------------------

    pub(crate) fn src_text(&self, span: Span) -> &'a str {
        let m = self.program.module(self.f.file);
        &m.src[span.start as usize..span.end as usize]
    }

    /// The (concrete) type of an expression.
    pub(crate) fn ty_of(&mut self, e: &Expr) -> Option<Ty> {
        let t = self
            .typed
            .tys
            .get(&(self.f.file, e.span.start, e.span.end))
            .copied()?;
        let t = self.conc(t);
        match self.kind(t) {
            TyKind::Unknown | TyKind::Var(_) | TyKind::Param(_) => None,
            _ => Some(t),
        }
    }

    pub(crate) fn scalar_ty_of(&mut self, e: &Expr) -> Option<IrTy> {
        let t = self.ty_of(e)?;
        self.scalar_of(t)
    }

    pub(crate) fn is_signed(&mut self, t: Ty) -> bool {
        matches!(self.kind(t), TyKind::Int(k) if k.signed())
    }

    pub(crate) fn signed_of(&mut self, e: &Expr) -> bool {
        match self.ty_of(e) {
            Some(t) => self.is_signed(t),
            None => false,
        }
    }

    pub(crate) fn scalar(&mut self, e: &'a Expr) -> Option<Value> {
        match self.expr(e) {
            Res::Val(v) => Some(v),
            Res::Unit => {
                self.unsupported(e.span, "a value-less operand");
                None
            }
            Res::Never => None,
        }
    }

    pub(crate) fn wrap(&mut self, op: Op, ty: IrTy, span: Span) -> Res {
        match self.emit(op, Some(ty), span) {
            Some(v) => Res::Val(v),
            None => Res::Never,
        }
    }

    // ---- The three entry points ----------------------------------------------------------------------------

    /// Evaluates for use: a scalar, or the address of a memory value. Does not move.
    pub(crate) fn expr(&mut self, e: &'a Expr) -> Res {
        if self.f.cur.is_none() || self.failed() {
            return Res::Never;
        }
        match &e.kind {
            ExprKind::Int => self.int_literal(e),
            ExprKind::Float => {
                let Some(ty) = self.scalar_ty_of(e) else {
                    return self.unsupported(e.span, "a float of this type");
                };
                let x: f64 = self
                    .src_text(e.span)
                    .replace('_', "")
                    .parse()
                    .unwrap_or(0.0);
                Res::Val(self.konst(ty, Const::Float(x), e.span))
            }
            ExprKind::Char => {
                let c = self.char_value(e.span);
                Res::Val(self.konst(IrTy::Int(32), Const::Int(c as i128), e.span))
            }
            ExprKind::Bool(b) => Res::Val(self.konst(IrTy::Bool, Const::Bool(*b), e.span)),
            ExprKind::Paren(i) => self.expr(i),
            ExprKind::Name(id) => self.name_expr(e, id),
            ExprKind::SelfValue => self.self_expr(e),
            ExprKind::Unary { op, operand } => self.unary(e, *op, operand),
            ExprKind::Binary { op, lhs, rhs } => self.binary(e, *op, lhs, rhs),
            ExprKind::Assign { op, lhs, rhs } => self.assign(e, *op, lhs, rhs),
            ExprKind::IncDec { op, operand } => self.incdec(e, *op, operand),
            ExprKind::While { cond, body } => self.while_loop(cond, body),
            ExprKind::For(f) => self.for_loop(e, f),
            ExprKind::Labeled { label, body } => {
                let name = self.src_text(label.span).to_string();
                self.f.pending_label = Some(name);
                let r = self.expr(body);
                self.f.pending_label = None;
                r
            }
            ExprKind::Break { label, value } => {
                self.break_expr(e, label.as_ref(), value.as_deref())
            }
            ExprKind::Continue { label } => self.continue_expr(e, label.as_ref()),
            ExprKind::Return(v) => self.return_expr(e, v.as_deref()),
            ExprKind::Defer(inner) => {
                if let Some(s) = self.f.scopes.last_mut() {
                    s.push(ScopeItem::Defer(inner));
                }
                Res::Unit
            }
            ExprKind::Convert {
                value,
                checked: false,
                ..
            } => self.convert(e, value),
            ExprKind::Is { value, pattern } => self.is_expr(e, value, pattern),
            ExprKind::In { lhs, rhs } => self.in_expr(e, lhs, rhs),
            ExprKind::Field { .. } | ExprKind::TupleIndex { .. } | ExprKind::Index { .. } => {
                self.place_expr(e)
            }
            _ => self.rvalue_expr(e),
        }
    }

    /// An expression that makes a new value: scalars come back directly, memory values are built in a
    /// temporary owned by the current scope.
    pub(crate) fn rvalue_expr(&mut self, e: &'a Expr) -> Res {
        let Some(t) = self.ty_of(e) else {
            return self.unsupported(e.span, "an expression of unknown type");
        };
        match self.repr(t) {
            Repr::Mem(_) => {
                let slot = self.slot_for(t);
                match self.into(e, slot, t) {
                    Res::Never => Res::Never,
                    _ => {
                        self.own(slot, t, false);
                        Res::Val(slot)
                    }
                }
            }
            _ => self.value_expr(e, None, t),
        }
    }

    /// Builds the value of `e` at `dst`, taking ownership. `want` is the type the destination holds:
    /// a value is wrapped into an `option` there, a `string` becomes a `str` view, and so on.
    pub(crate) fn into(&mut self, e: &'a Expr, dst: Value, want: Ty) -> Res {
        if self.f.cur.is_none() || self.failed() {
            return Res::Never;
        }
        let Some(have) = self.ty_of(e) else {
            return self.unsupported(e.span, "an expression of unknown type");
        };
        let want = self.conc(want);
        if have != want {
            if let Some(r) = self.coerce_into(e, have, dst, want) {
                return r;
            }
        }
        self.into_exact(e, dst, have)
    }

    /// `into` when the destination holds exactly the expression's type.
    pub(crate) fn into_exact(&mut self, e: &'a Expr, dst: Value, t: Ty) -> Res {
        match self.repr(t) {
            Repr::Zst => self.expr(e),
            Repr::Scalar(_) => {
                let v = match self.consume_scalar(e) {
                    Some(v) => v,
                    None => return Res::Never,
                };
                self.store_scalar_ty(dst, v);
                Res::Unit
            }
            Repr::Mem(_) => match &e.kind {
                ExprKind::Paren(i) => self.into_exact(i, dst, t),
                ExprKind::Name(_)
                | ExprKind::SelfValue
                | ExprKind::Field { .. }
                | ExprKind::TupleIndex { .. } => self.into_from_place(e, dst, t),
                ExprKind::Index { index, .. } if !matches!(index.kind, ExprKind::Range { .. }) => {
                    self.into_from_place(e, dst, t)
                }
                _ => self.build_mem(e, dst, t),
            },
        }
    }

    /// Copies (or moves) the value in a place to `dst`.
    fn into_from_place(&mut self, e: &'a Expr, dst: Value, t: Ty) -> Res {
        let Some(p) = self.place(e) else {
            if self.failed() {
                return Res::Never;
            }
            // A name that is not a variable (a unit case of an enum, ...).
            return self.build_mem(e, dst, t);
        };
        self.copy_mem(dst, p, t);
        if self.needs_drop(t) {
            self.mark_moved(e);
        }
        Res::Unit
    }

    /// The value of an owning place has moved: its flag is cleared.
    pub(crate) fn mark_moved(&mut self, e: &Expr) {
        let e = strip_parens(e);
        let def = match &e.kind {
            ExprKind::Name(id) => self.res.use_at(self.f.file, id.span.start),
            ExprKind::SelfValue => self.res.use_at(self.f.file, e.span.start),
            _ => None,
        };
        if let Some(d) = def {
            if let Some(Local::Slot { flag: Some(f), .. }) = self.f.locals.get(&d).copied() {
                self.clear_flag(f);
            }
        }
    }

    /// A scalar value that is handed to a new owner (a box moves; a number is just copied).
    pub(crate) fn consume_scalar(&mut self, e: &'a Expr) -> Option<Value> {
        let t = self.ty_of(e)?;
        let v = self.scalar(e)?;
        if self.needs_drop(t) {
            self.mark_moved(e);
        }
        Some(v)
    }

    // ---- Scalar expressions that can also be reached through `into` ----------------------------------------

    /// Evaluates an expression of a scalar or zero-sized type that is not a place.
    pub(crate) fn value_expr(&mut self, e: &'a Expr, dst: Option<Value>, want: Ty) -> Res {
        let _ = (dst, want);
        match &e.kind {
            ExprKind::Block(b) => self.block(b, None, want),
            ExprKind::Unsafe(b) => self.block(b, None, want),
            ExprKind::If(i) => self.if_expr(e, i, false, None, want),
            ExprKind::Unless(i) => self.if_expr(e, i, true, None, want),
            ExprKind::Ternary { cond, then, else_ } => {
                self.ternary(e, cond, then, else_, None, want)
            }
            ExprKind::Loop(body) => self.loop_expr(e, body, None, want),
            ExprKind::Switch(s) => self.switch_expr(e, s, None, want),
            ExprKind::Call { callee, args, mark } => self.call(e, callee, args, *mark, None),
            ExprKind::Guard { cond, else_ } => self.guard_expr(cond, else_),
            ExprKind::StructLit { fields, .. } if self.obj_def(want).is_some() => {
                self.obj_literal_value(e, fields, want)
            }
            ExprKind::Try(inner) => self.try_expr(e, inner, None),
            ExprKind::Copy(inner) => self.copy_expr(e, inner, None),
            ExprKind::Binary {
                op: Punct::QuestionQuestion,
                lhs,
                rhs,
            } => self.default_expr(e, lhs, rhs, None, want),
            ExprKind::Binary { .. } | ExprKind::Unary { .. } => self.expr(e),
            _ => self.unsupported(e.span, "this kind of expression"),
        }
    }

    /// Builds a new value of a memory type at `dst`.
    pub(crate) fn build_mem(&mut self, e: &'a Expr, dst: Value, t: Ty) -> Res {
        match &e.kind {
            ExprKind::Block(b) | ExprKind::Unsafe(b) => self.block(b, Some(dst), t),
            ExprKind::If(i) => self.if_expr(e, i, false, Some(dst), t),
            ExprKind::Unless(i) => self.if_expr(e, i, true, Some(dst), t),
            ExprKind::Ternary { cond, then, else_ } => {
                self.ternary(e, cond, then, else_, Some(dst), t)
            }
            ExprKind::Loop(body) => self.loop_expr(e, body, Some(dst), t),
            ExprKind::Switch(s) => self.switch_expr(e, s, Some(dst), t),
            ExprKind::Call { callee, args, mark } => self.call(e, callee, args, *mark, Some(dst)),
            ExprKind::Try(inner) => self.try_expr(e, inner, Some(dst)),
            ExprKind::Copy(inner) => self.copy_expr(e, inner, Some(dst)),
            ExprKind::Tuple(es) => self.tuple_lit(e, es, dst, t),
            ExprKind::StructLit { fields, .. } => self.struct_lit(e, fields, dst, t),
            ExprKind::Str(parts) => self.string_lit(e, parts, dst, t),
            ExprKind::List(items) => self.list_lit(e, items, dst, t),
            ExprKind::None => self.none_lit(e, dst, t),
            ExprKind::Field {
                base,
                name,
                optional: true,
            } if self.res.use_at(self.f.file, name.span.start).is_none() => {
                self.opt_field(e, base, name, dst, t)
            }
            ExprKind::Name(_) | ExprKind::DotName(_) | ExprKind::Field { .. } => {
                self.variant_value(e, dst, t)
            }
            ExprKind::Index { .. } => self.slice_expr(e, dst, t),
            ExprKind::Binary {
                op: Punct::QuestionQuestion,
                lhs,
                rhs,
            } => self.default_expr(e, lhs, rhs, Some(dst), t),
            ExprKind::Closure(c) => self.closure_value(e, c, dst, t),
            ExprKind::Convert { value, checked, .. } => {
                self.convert_mem(e, value, *checked, dst, t)
            }
            ExprKind::Builder(b) => self.builder_expr(e, b, dst, t),
            ExprKind::Map(items) => self.map_lit(e, items, dst, t),
            ExprKind::Set(items) => self.set_lit(e, items, dst, t),
            ExprKind::Repeat { value, count } => self.repeat_lit(e, value, count, dst, t),
            ExprKind::Binary { .. } => self.binary_mem(e, dst, t),
            ExprKind::Range { .. } => self.range_value(e, dst, t),
            ExprKind::Guard { .. }
            | ExprKind::Return(_)
            | ExprKind::Break { .. }
            | ExprKind::Continue { .. } => self.expr(e),
            _ => self.unsupported(e.span, "this kind of expression"),
        }
    }

    // ---- Literals -----------------------------------------------------------------------------------------

    fn int_literal(&mut self, e: &Expr) -> Res {
        let Some(ty) = self.scalar_ty_of(e) else {
            return self.unsupported(e.span, "an integer of this type");
        };
        let n = literal_value(self.src_text(e.span)).unwrap_or(0);
        if ty.is_float() {
            return Res::Val(self.konst(ty, Const::Float(n as f64), e.span));
        }
        Res::Val(self.konst(ty, Const::Int(n as i128), e.span))
    }

    /// The code point of a character literal such as `'a'` or `'\n'`.
    fn char_value(&self, span: Span) -> u32 {
        let t = self.src_text(span);
        let inner = t.trim_matches('\'');
        let mut cs = inner.chars();
        match cs.next() {
            Some('\\') => match cs.next() {
                Some('n') => 10,
                Some('t') => 9,
                Some('r') => 13,
                Some('0') => 0,
                Some('\\') => 92,
                Some('\'') => 39,
                Some('"') => 34,
                Some('u') => {
                    let hex: String = cs.filter(|c| c.is_ascii_hexdigit()).collect();
                    u32::from_str_radix(&hex, 16).unwrap_or(0)
                }
                Some(c) => c as u32,
                None => 0,
            },
            Some(c) => c as u32,
            None => 0,
        }
    }

    // ---- Names and places ---------------------------------------------------------------------------------------

    fn name_expr(&mut self, e: &'a Expr, id: &Ident) -> Res {
        let Some(def) = self.res.use_at(self.f.file, id.span.start) else {
            return self.unsupported(e.span, "an unresolved name");
        };
        self.local_value(e, def)
    }

    fn self_expr(&mut self, e: &'a Expr) -> Res {
        let Some(def) = self.res.use_at(self.f.file, e.span.start) else {
            return self.unsupported(e.span, "`self` here");
        };
        self.local_value(e, def)
    }

    fn local_value(&mut self, e: &'a Expr, def: DefId) -> Res {
        match self.f.locals.get(&def).copied() {
            Some(Local::Val(v)) => Res::Val(v),
            Some(Local::Slot { ptr, ty, .. }) => match self.repr(ty) {
                Repr::Scalar(s) => match self.emit(Op::Load(s, ptr), Some(s), e.span) {
                    Some(v) => Res::Val(v),
                    None => Res::Never,
                },
                Repr::Mem(_) => Res::Val(ptr),
                Repr::Zst => Res::Unit,
            },
            None => self.global_value(e, def),
        }
    }

    /// The address of a place, or `None` when the expression is not one (or lowering failed).
    pub(crate) fn place(&mut self, e: &'a Expr) -> Option<Value> {
        if self.f.cur.is_none() || self.failed() {
            return None;
        }
        match &e.kind {
            ExprKind::Paren(i) => self.place(i),
            ExprKind::Name(id) => {
                let def = self.res.use_at(self.f.file, id.span.start)?;
                if let Some(p) = self.local_place(def) {
                    return Some(p);
                }
                if self.res.def(def).kind == DefKind::Var && self.globals.contains_key(&def) {
                    return self.global_place(def);
                }
                None
            }
            ExprKind::SelfValue => {
                let def = self.res.use_at(self.f.file, e.span.start)?;
                self.local_place(def)
            }
            ExprKind::Field {
                base,
                name,
                optional,
            } => {
                if *optional || self.res.use_at(self.f.file, name.span.start).is_some() {
                    return None;
                }
                self.field_place(e, base, name)
            }
            ExprKind::TupleIndex { base, index } => {
                let i: usize = self.src_text(*index).parse().ok()?;
                let bt = self.ty_of(base)?;
                let p = self.base_address(base, bt)?;
                let inner = self.peel_box(bt);
                let fs = self.struct_fields(inner);
                let f = fs.get(i)?;
                Some(self.addr(p, f.off))
            }
            ExprKind::Index { base, index } => {
                if matches!(index.kind, ExprKind::Range { .. }) {
                    return None;
                }
                self.index_place(e, base, index)
            }
            _ => None,
        }
    }

    fn local_place(&mut self, def: DefId) -> Option<Value> {
        match self.f.locals.get(&def).copied() {
            Some(Local::Slot { ptr, .. }) => Some(ptr),
            _ => None,
        }
    }

    /// A place or a plain value used as an operand: its value (scalar) or address (memory).
    fn place_expr(&mut self, e: &'a Expr) -> Res {
        if let ExprKind::Index { index, .. } = &e.kind {
            if matches!(index.kind, ExprKind::Range { .. }) {
                return self.rvalue_expr(e);
            }
        }
        if let ExprKind::Field { name, optional, .. } = &e.kind {
            if self.res.use_at(self.f.file, name.span.start).is_some() {
                // A path such as `shape.empty` or `util.limit`.
                return self.global_or_variant(e);
            }
            if *optional {
                return self.rvalue_expr(e);
            }
        }
        let Some(t) = self.ty_of(e) else {
            return self.unsupported(e.span, "a field of unknown type");
        };
        let Some(p) = self.place(e) else {
            if self.failed() {
                return Res::Never;
            }
            return self.unsupported(e.span, "this kind of place");
        };
        match self.repr(t) {
            Repr::Scalar(s) => match self.emit(Op::Load(s, p), Some(s), e.span) {
                Some(v) => Res::Val(v),
                None => Res::Never,
            },
            Repr::Mem(_) => Res::Val(p),
            Repr::Zst => Res::Unit,
        }
    }

    /// `box<T>` and `obj` values are addresses: fields are reached through them.
    pub(crate) fn peel_box(&mut self, t: Ty) -> Ty {
        match self.kind(t) {
            TyKind::Box(inner) => inner,
            _ => t,
        }
    }

    /// The address where the fields of a base expression start.
    fn base_address(&mut self, base: &'a Expr, bt: Ty) -> Option<Value> {
        match self.kind(bt) {
            TyKind::Box(_) => {
                let v = self.scalar(base)?;
                Some(v)
            }
            TyKind::Adt(d, _) if self.res.def(d).kind == DefKind::Obj => {
                // An object: its fields follow the table pointer.
                let v = self.scalar(base)?;
                Some(self.addr(v, 8))
            }
            _ => match self.expr(base) {
                Res::Val(p) => Some(p),
                _ => None,
            },
        }
    }

    fn field_place(&mut self, e: &'a Expr, base: &'a Expr, name: &Ident) -> Option<Value> {
        let _ = e;
        let bt = self.ty_of(base)?;
        let p = self.base_address(base, bt)?;
        let inner = self.peel_box(bt);
        let fname = self.src_text(name.span);
        let TyKind::Adt(d, _) = self.kind(inner) else {
            self.unsupported(name.span, "a field of this kind of value");
            return None;
        };
        let info = self.typed.adts.get(&d)?;
        let idx = info
            .fields
            .iter()
            .position(|(fd, _)| self.res.def(*fd).name == fname)?;
        let fs = self.struct_fields(inner);
        let f = fs.get(idx)?;
        Some(self.addr(p, f.off))
    }

    fn index_place(&mut self, e: &'a Expr, base: &'a Expr, index: &'a Expr) -> Option<Value> {
        let bt = self.ty_of(base)?;
        let et = self.ty_of(e)?;
        let size = self.layout(et).size;
        match self.kind(bt) {
            TyKind::List(_) | TyKind::Iter(_) => {
                let hdr = match self.expr(base) {
                    Res::Val(p) => p,
                    _ => return None,
                };
                let i = self.scalar(index)?;
                let ptr_ty = IrTy::Ptr;
                let len = self.load_at(hdr, 8, IrTy::I64);
                let bad = self.emit(Op::Cmp(CmpOp::UGe, i, len), Some(IrTy::Bool), e.span)?;
                self.stop_if(bad, "index out of range");
                let data = self.load_at(hdr, 0, ptr_ty);
                let sz = self.i64c(size as i64);
                let off = self.emit(Op::Bin(BinOp::Mul, i, sz), Some(IrTy::I64), e.span)?;
                self.emit(Op::PtrAdd(data, off), Some(IrTy::Ptr), e.span)
            }
            TyKind::Array(_, len) => {
                let p = match self.expr(base) {
                    Res::Val(p) => p,
                    _ => return None,
                };
                let i = self.scalar(index)?;
                let n = match len {
                    crate::types::ArrayLen::Known(n) => n as i64,
                    _ => 0,
                };
                let len = self.i64c(n);
                let bad = self.emit(Op::Cmp(CmpOp::UGe, i, len), Some(IrTy::Bool), e.span)?;
                self.stop_if(bad, "index out of range");
                let sz = self.i64c(size as i64);
                let off = self.emit(Op::Bin(BinOp::Mul, i, sz), Some(IrTy::I64), e.span)?;
                self.emit(Op::PtrAdd(p, off), Some(IrTy::Ptr), e.span)
            }
            TyKind::Map(k, v) => {
                let hdr = match self.expr(base) {
                    Res::Val(p) => p,
                    _ => return None,
                };
                let ks = self.slot_for(k);
                if let Res::Never = self.into(index, ks, k) {
                    return None;
                }
                self.own(ks, k, false);
                self.call_core_g("rt_map_at", &[k, v], vec![hdr, ks])
            }
            _ => {
                self.unsupported(e.span, "indexing this kind of value");
                None
            }
        }
    }

    // ---- Operators -------------------------------------------------------------------------------------------------

    fn unary(&mut self, e: &'a Expr, op: Punct, operand: &'a Expr) -> Res {
        if let Some(r) = self.user_operator(e, e.span) {
            return r;
        }
        let Some(ty) = self.scalar_ty_of(e) else {
            return self.unsupported(e.span, "this operand type");
        };
        match op {
            Punct::Minus => {
                if matches!(operand.kind, ExprKind::Int) {
                    let n = literal_value(self.src_text(operand.span)).unwrap_or(0) as i128;
                    return Res::Val(if ty.is_float() {
                        self.konst(ty, Const::Float(-(n as f64)), e.span)
                    } else {
                        self.konst(ty, Const::Int(-n), e.span)
                    });
                }
                let Some(v) = self.scalar(operand) else {
                    return Res::Never;
                };
                if ty.is_float() {
                    return self.wrap(Op::Neg(v), ty, e.span);
                }
                let zero = self.konst(ty, Const::Int(0), e.span);
                self.wrap(Op::Checked(BinOp::Sub, true, zero, v), ty, e.span)
            }
            Punct::Bang | Punct::Tilde => {
                let Some(v) = self.scalar(operand) else {
                    return Res::Never;
                };
                self.wrap(Op::Not(v), ty, e.span)
            }
            _ => self.unsupported(e.span, "this operator"),
        }
    }

    fn binary(&mut self, e: &'a Expr, op: Punct, lhs: &'a Expr, rhs: &'a Expr) -> Res {
        match op {
            Punct::AndAnd | Punct::OrOr => {
                return self.short_circuit(e, op == Punct::AndAnd, lhs, rhs);
            }
            Punct::EqEq | Punct::NotEq | Punct::Lt | Punct::Gt | Punct::LtEq | Punct::GtEq => {
                return self.compare(e, op, lhs, rhs);
            }
            Punct::QuestionQuestion => return self.rvalue_expr(e),
            Punct::PipeGt => {
                let srcs = vec![super::call::ArgSource::Expr(lhs)];
                return self.call_value_with(e, rhs, srcs, None);
            }
            _ => {}
        }
        if let Some(r) = self.user_operator(e, e.span) {
            return r;
        }
        // Text and other memory results.
        if let Some(t) = self.ty_of(e) {
            if self.is_mem(t) {
                return self.rvalue_expr(e);
            }
        }
        let Some(ty) = self.scalar_ty_of(e) else {
            return self.unsupported(e.span, "this operand type");
        };
        let signed = self.signed_of(e);
        let Some(a) = self.scalar(lhs) else {
            return Res::Never;
        };
        let Some(b) = self.scalar(rhs) else {
            return Res::Never;
        };
        let b = if matches!(op, Punct::Shl | Punct::Shr) {
            let rs = self.signed_of(rhs);
            self.resize(b, ty, rs, e.span)
        } else {
            b
        };
        self.arith(e.span, op, ty, signed, a, b)
    }

    /// Operators on memory types: joining text.
    fn binary_mem(&mut self, e: &'a Expr, dst: Value, t: Ty) -> Res {
        if let ExprKind::Binary {
            op: Punct::Plus,
            lhs,
            rhs,
        } = &e.kind
        {
            if matches!(self.kind(t), TyKind::String) {
                return self.concat(e, lhs, rhs, dst);
            }
        }
        if let ExprKind::Binary {
            op: Punct::PipeGt,
            lhs,
            rhs,
        } = &e.kind
        {
            let srcs = vec![super::call::ArgSource::Expr(lhs)];
            return self.call_value_with(e, rhs, srcs, Some(dst));
        }
        if let Some(r) = self.user_operator_into(e, dst) {
            return r;
        }
        self.unsupported(e.span, "this operator on this type")
    }

    /// `a op b` on two values of type `ty`.
    pub(crate) fn arith(
        &mut self,
        span: Span,
        op: Punct,
        ty: IrTy,
        signed: bool,
        a: Value,
        b: Value,
    ) -> Res {
        if ty.is_float() {
            let o = match op {
                Punct::Plus => BinOp::Add,
                Punct::Minus => BinOp::Sub,
                Punct::Star => BinOp::Mul,
                Punct::Slash => BinOp::Div,
                Punct::Percent => BinOp::Rem,
                _ => return self.unsupported(span, "this float operator"),
            };
            return self.wrap(Op::Bin(o, a, b), ty, span);
        }
        if !ty.is_int() {
            return self.unsupported(span, "this operand type");
        }
        let bits = if let IrTy::Int(b) = ty { b as i128 } else { 64 };
        match op {
            Punct::Plus => self.wrap(Op::Checked(BinOp::Add, signed, a, b), ty, span),
            Punct::Minus => self.wrap(Op::Checked(BinOp::Sub, signed, a, b), ty, span),
            Punct::Star => self.wrap(Op::Checked(BinOp::Mul, signed, a, b), ty, span),
            Punct::PlusPercent => self.wrap(Op::Bin(BinOp::Add, a, b), ty, span),
            Punct::MinusPercent => self.wrap(Op::Bin(BinOp::Sub, a, b), ty, span),
            Punct::StarPercent => self.wrap(Op::Bin(BinOp::Mul, a, b), ty, span),
            Punct::Amp => self.wrap(Op::Bin(BinOp::And, a, b), ty, span),
            Punct::Pipe => self.wrap(Op::Bin(BinOp::Or, a, b), ty, span),
            Punct::Caret => self.wrap(Op::Bin(BinOp::Xor, a, b), ty, span),
            Punct::Slash | Punct::Percent => {
                let zero = self.konst(ty, Const::Int(0), span);
                if let Some(z) = self.emit(Op::Cmp(CmpOp::Eq, b, zero), Some(IrTy::Bool), span) {
                    self.stop_if(z, "division by zero");
                }
                let o = match (op, signed) {
                    (Punct::Slash, true) => BinOp::Div,
                    (Punct::Slash, false) => BinOp::UDiv,
                    (_, true) => BinOp::Rem,
                    (_, false) => BinOp::URem,
                };
                self.wrap(Op::Bin(o, a, b), ty, span)
            }
            Punct::Shl | Punct::Shr => {
                let limit = self.konst(ty, Const::Int(bits), span);
                if let Some(t) = self.emit(Op::Cmp(CmpOp::UGe, b, limit), Some(IrTy::Bool), span) {
                    self.stop_if(t, "shifted by the width or more");
                }
                let o = match (op, signed) {
                    (Punct::Shl, _) => BinOp::Shl,
                    (_, true) => BinOp::Shr,
                    (_, false) => BinOp::UShr,
                };
                self.wrap(Op::Bin(o, a, b), ty, span)
            }
            _ => self.unsupported(span, "this operator"),
        }
    }

    fn compare(&mut self, e: &'a Expr, op: Punct, lhs: &'a Expr, rhs: &'a Expr) -> Res {
        let Some(lt) = self.ty_of(lhs) else {
            return self.unsupported(e.span, "comparing values of unknown type");
        };
        if !self.is_scalar(lt) {
            return self.compare_mem(e, op, lhs, rhs, lt);
        }
        let signed = self.is_signed(lt);
        let Some(a) = self.scalar(lhs) else {
            return Res::Never;
        };
        let Some(b) = self.scalar(rhs) else {
            return Res::Never;
        };
        let float = self.vty(a).is_float();
        let o = match (op, signed || float) {
            (Punct::EqEq, _) => CmpOp::Eq,
            (Punct::NotEq, _) => CmpOp::Ne,
            (Punct::Lt, true) => CmpOp::Lt,
            (Punct::LtEq, true) => CmpOp::Le,
            (Punct::Gt, true) => CmpOp::Gt,
            (Punct::GtEq, true) => CmpOp::Ge,
            (Punct::Lt, false) => CmpOp::ULt,
            (Punct::LtEq, false) => CmpOp::ULe,
            (Punct::Gt, false) => CmpOp::UGt,
            _ => CmpOp::UGe,
        };
        self.wrap(Op::Cmp(o, a, b), IrTy::Bool, e.span)
    }

    fn short_circuit(&mut self, e: &'a Expr, and: bool, lhs: &'a Expr, rhs: &'a Expr) -> Res {
        let _ = e;
        let eval_rhs = self.new_block(&[]);
        let join = self.new_block(&[IrTy::Bool]);
        let a = match self.scalar(lhs) {
            Some(a) => a,
            None => return Res::Never,
        };
        let skip = Target {
            block: join,
            args: vec![a],
        };
        let go = Target {
            block: eval_rhs,
            args: vec![],
        };
        // `a && b`: when `a` is false the result is `a`. `a || b`: when `a` is true the result is `a`.
        if and {
            self.terminate(Term::Branch(a, go, skip));
        } else {
            self.terminate(Term::Branch(a, skip, go));
        }
        self.start(eval_rhs);
        self.push_scope();
        let b = self.scalar(rhs);
        if let Some(b) = b {
            self.pop_scope();
            self.jump(join, vec![b]);
        } else {
            self.f.scopes.pop();
        }
        self.start(join);
        Res::Val(self.f.func.block_params(join)[0])
    }

    // ---- Assignment -----------------------------------------------------------------------------------------------

    fn assign(&mut self, e: &'a Expr, op: Punct, lhs: &'a Expr, rhs: &'a Expr) -> Res {
        if matches!(lhs.kind, ExprKind::Underscore) {
            // `_ = x` throws the value away (and drops it).
            return self.discard(rhs);
        }
        if op != Punct::Eq && self.user_operator(e, e.span).is_some() {
            return Res::Unit;
        }
        let Some(lt) = self.ty_of(lhs) else {
            return self.unsupported(e.span, "assigning to a place of unknown type");
        };
        let Some(p) = self.place(lhs) else {
            if self.failed() {
                return Res::Never;
            }
            return self.unsupported(e.span, "assigning to this kind of place");
        };
        if op == Punct::Eq {
            return self.store_new(p, lt, rhs, lhs);
        }
        if op == Punct::QuestionQuestionEq {
            let Some(info) = self.enum_info(lt) else {
                return self.unsupported(e.span, "`??=` on this type");
            };
            let tag = self.load_at(p, 0, info.tag);
            let zero = self.konst(info.tag, Const::Int(0), e.span);
            let Some(is_none) = self.emit(Op::Cmp(CmpOp::Eq, tag, zero), Some(IrTy::Bool), e.span)
            else {
                return Res::Never;
            };
            let none_b = self.new_block(&[]);
            let join = self.new_block(&[]);
            self.branch(is_none, none_b, join);
            self.start(none_b);
            self.push_scope();
            let r = self.store_new(p, lt, rhs, lhs);
            if matches!(r, Res::Never) {
                self.f.scopes.pop();
            } else {
                self.pop_scope();
                self.jump(join, vec![]);
            }
            self.start(join);
            return Res::Unit;
        }
        // `a op= b`: numbers (and text for `+=`).
        let Repr::Scalar(ty) = self.repr(lt) else {
            return self.append_assign(e, op, p, lt, rhs);
        };
        let base = match op {
            Punct::PlusEq => Punct::Plus,
            Punct::MinusEq => Punct::Minus,
            Punct::StarEq => Punct::Star,
            Punct::SlashEq => Punct::Slash,
            Punct::PercentEq => Punct::Percent,
            Punct::AmpEq => Punct::Amp,
            Punct::PipeEq => Punct::Pipe,
            Punct::CaretEq => Punct::Caret,
            Punct::ShlEq => Punct::Shl,
            Punct::ShrEq => Punct::Shr,
            Punct::PlusPercentEq => Punct::PlusPercent,
            Punct::MinusPercentEq => Punct::MinusPercent,
            Punct::StarPercentEq => Punct::StarPercent,
            _ => return self.unsupported(e.span, "this assignment operator"),
        };
        let signed = self.is_signed(lt);
        let Some(rv) = self.scalar(rhs) else {
            return Res::Never;
        };
        let Some(cur) = self.emit(Op::Load(ty, p), Some(ty), e.span) else {
            return Res::Never;
        };
        let rv = if matches!(base, Punct::Shl | Punct::Shr) {
            let rs = self.signed_of(rhs);
            self.resize(rv, ty, rs, e.span)
        } else {
            rv
        };
        match self.arith(e.span, base, ty, signed, cur, rv) {
            Res::Val(v) => {
                self.emit(Op::Store(p, v), None, e.span);
                Res::Unit
            }
            other => other,
        }
    }

    /// Replaces the value at `p` (of type `t`) by the value of `rhs`, dropping the old one.
    pub(crate) fn store_new(&mut self, p: Value, t: Ty, rhs: &'a Expr, lhs: &'a Expr) -> Res {
        match self.repr(t) {
            Repr::Zst => self.expr(rhs),
            Repr::Scalar(_) if !self.needs_drop(t) => {
                let Some(v) = self.scalar(rhs) else {
                    return Res::Never;
                };
                self.emit(Op::Store(p, v), None, rhs.span);
                Res::Unit
            }
            _ => {
                // Build the new value first (it may read the old one), then drop the old one.
                let tmp = self.slot_for(t);
                if let Res::Never = self.into(rhs, tmp, t) {
                    return Res::Never;
                }
                let flag = self.local_flag(lhs);
                self.emit_drop(p, t, flag);
                self.copy_mem(p, tmp, t);
                if let Some(f) = flag {
                    let one = self.konst(IrTy::Bool, Const::Bool(true), Span::new(0, 0));
                    self.emit(Op::Store(f, one), None, Span::new(0, 0));
                }
                Res::Unit
            }
        }
    }

    /// The ownership flag of a variable, when `e` names one that has it.
    pub(crate) fn local_flag(&mut self, e: &Expr) -> Option<Value> {
        let e = strip_parens(e);
        let def = match &e.kind {
            ExprKind::Name(id) => self.res.use_at(self.f.file, id.span.start)?,
            ExprKind::SelfValue => self.res.use_at(self.f.file, e.span.start)?,
            _ => return None,
        };
        match self.f.locals.get(&def).copied() {
            Some(Local::Slot { flag, .. }) => flag,
            _ => None,
        }
    }

    /// Evaluates an expression and drops its value (`_ = x`).
    fn discard(&mut self, rhs: &'a Expr) -> Res {
        let Some(t) = self.ty_of(rhs) else {
            return self.unsupported(rhs.span, "discarding a value of unknown type");
        };
        if self.is_mem(t) || (self.is_scalar(t) && self.needs_drop(t)) {
            let slot = self.slot_for(t);
            if let Res::Never = self.into(rhs, slot, t) {
                return Res::Never;
            }
            self.emit_drop(slot, t, None);
            return Res::Unit;
        }
        self.expr(rhs);
        Res::Unit
    }

    /// Changes an integer to the width of `ty`.
    pub(crate) fn resize(&mut self, v: Value, ty: IrTy, signed: bool, span: Span) -> Value {
        let from = self.vty(v);
        if from == ty {
            return v;
        }
        let (IrTy::Int(a), IrTy::Int(b)) = (from, ty) else {
            return v;
        };
        let kind = if b > a {
            if signed {
                CastKind::SExt
            } else {
                CastKind::ZExt
            }
        } else {
            CastKind::Trunc
        };
        self.emit(Op::Cast(kind, v, ty), Some(ty), span)
            .unwrap_or(v)
    }

    fn incdec(&mut self, e: &'a Expr, op: Punct, operand: &'a Expr) -> Res {
        let Some(t) = self.ty_of(operand) else {
            return self.unsupported(e.span, "`++` on a value of unknown type");
        };
        let Some(ty) = self.scalar_of(t) else {
            return self.unsupported(e.span, "`++` on this type");
        };
        let Some(p) = self.place(operand) else {
            return self.unsupported(e.span, "`++` on this kind of place");
        };
        let signed = self.is_signed(t);
        let Some(cur) = self.emit(Op::Load(ty, p), Some(ty), e.span) else {
            return Res::Never;
        };
        let one = self.konst(ty, Const::Int(1), e.span);
        let o = if op == Punct::PlusPlus {
            Punct::Plus
        } else {
            Punct::Minus
        };
        if let Res::Val(v) = self.arith(e.span, o, ty, signed, cur, one) {
            self.emit(Op::Store(p, v), None, e.span);
        }
        Res::Unit
    }

    fn convert(&mut self, e: &'a Expr, value: &'a Expr) -> Res {
        let (Some(from), Some(to)) = (self.ty_of(value), self.ty_of(e)) else {
            return self.unsupported(e.span, "this conversion");
        };
        if self.is_scalar(to) {
            if let Some(r) = self.user_convert(e, value, None) {
                return r;
            }
        }
        if !self.is_scalar(to) {
            return self.rvalue_expr(e);
        }
        let (Some(fi), Some(ti)) = (self.scalar_of(from), self.scalar_of(to)) else {
            return self.unsupported(e.span, "this conversion");
        };
        let signed = self.is_signed(from);
        let Some(v) = self.scalar(value) else {
            return Res::Never;
        };
        if fi == ti {
            return Res::Val(v);
        }
        let kind = match (fi, ti) {
            (IrTy::Int(a), IrTy::Int(b)) if b > a => {
                if signed {
                    CastKind::SExt
                } else {
                    CastKind::ZExt
                }
            }
            (IrTy::Int(_), IrTy::Int(_)) => CastKind::Trunc,
            (IrTy::Int(_), t) if t.is_float() => CastKind::IntToFloat,
            (f, t) if f.is_float() && t.is_float() => CastKind::FloatResize,
            (IrTy::Bool, IrTy::Int(_)) => {
                // false is 0 and true is 1.
                let one = self.konst(ti, Const::Int(1), e.span);
                let zero = self.konst(ti, Const::Int(0), e.span);
                return self.select(v, one, zero, ti, e.span);
            }
            _ => return self.unsupported(e.span, "this conversion"),
        };
        self.wrap(Op::Cast(kind, v, ti), ti, e.span)
    }

    /// `cond ? a : b` on two ready values.
    pub(crate) fn select(&mut self, cond: Value, a: Value, b: Value, ty: IrTy, span: Span) -> Res {
        let _ = span;
        let join = self.new_block(&[ty]);
        self.terminate(Term::Branch(
            cond,
            Target {
                block: join,
                args: vec![a],
            },
            Target {
                block: join,
                args: vec![b],
            },
        ));
        self.start(join);
        Res::Val(self.f.func.block_params(join)[0])
    }

    // ---- Blocks and statements -----------------------------------------------------------------------------------------

    /// A block. A memory value is written to `dst`; a scalar is returned.
    pub(crate) fn block(&mut self, b: &'a Block, dst: Option<Value>, want: Ty) -> Res {
        self.push_scope();
        // Local functions are visible in the whole block.
        for s in &b.stmts {
            if let StmtKind::Item(it) = &s.kind {
                let f = self.f.file;
                self.index_item(f, it);
            }
        }
        for s in &b.stmts {
            if self.f.cur.is_none() {
                break;
            }
            self.stmt(s);
        }
        let r = match &b.tail {
            Some(t) if self.f.cur.is_some() => match dst {
                Some(d) => self.into(t, d, want),
                None => self.expr(t),
            },
            Some(_) => Res::Never,
            None if self.f.cur.is_none() => Res::Never,
            None => Res::Unit,
        };
        self.pop_scope();
        if self.f.cur.is_none() {
            return Res::Never;
        }
        r
    }

    /// The block that is a function body.
    pub(crate) fn block_value(&mut self, b: &'a Block, rp: RetPass, ret: Ty) -> Res {
        match rp {
            RetPass::Sret => {
                let dst = self.f.sret;
                let r = self.block(b, dst, ret);
                if matches!(r, Res::Unit) && !self.block_has_tail(b) {
                    return Res::Unit;
                }
                r
            }
            _ => self.block(b, None, ret),
        }
    }

    fn block_has_tail(&self, b: &Block) -> bool {
        b.tail.is_some()
    }

    pub(crate) fn body_expr(&mut self, e: &'a Expr, rp: RetPass, ret: Ty) -> Res {
        match rp {
            RetPass::Sret => {
                let dst = self.f.sret.unwrap_or(Value(0));
                self.into(e, dst, ret)
            }
            _ => self.expr(e),
        }
    }

    fn stmt(&mut self, s: &'a Stmt) {
        match &s.kind {
            StmtKind::Decl(d) => self.decl(d),
            StmtKind::Expr(e) => {
                self.push_scope();
                self.expr(e);
                self.pop_scope();
            }
            StmtKind::Item(_) => {}
        }
    }

    fn decl(&mut self, d: &'a Decl) {
        let Some(init) = &d.init else {
            self.unsupported(d.pattern.span, "a declaration without a value");
            return;
        };
        let Some(it) = self.ty_of(init) else {
            self.unsupported(init.span, "a value of unknown type");
            return;
        };
        // The declared type, when written, decides the type of the variable (`var x: int? = 5`).
        let declared = match &d.pattern.kind {
            PatternKind::Binding { name, .. } => {
                let def = self.res.decls.get(&(self.f.file, name.span.start)).copied();
                def.and_then(|df| self.typed.local_tys.get(&df).copied())
                    .map(|t| self.conc(t))
            }
            _ => None,
        };
        let vt = declared.unwrap_or(it);
        match &d.pattern.kind {
            PatternKind::Wildcard => {
                self.push_scope();
                self.discard(init);
                self.pop_scope();
            }
            PatternKind::Binding { name, .. } => {
                let Some(&def) = self.res.decls.get(&(self.f.file, name.span.start)) else {
                    return;
                };
                self.bind_decl(def, init, vt, d.kind == DeclKind::Var);
            }
            _ => self.destructure_decl(d, init, it),
        }
    }

    /// `const x = e` / `var x = e` for a single name.
    pub(crate) fn bind_decl(&mut self, def: DefId, init: &'a Expr, vt: Ty, is_var: bool) {
        match self.repr(vt) {
            Repr::Zst => {
                self.push_scope();
                self.expr(init);
                self.pop_scope();
            }
            Repr::Scalar(s) if !is_var && !self.needs_drop(vt) => {
                self.push_scope();
                let v = self.scalar(init);
                self.pop_scope();
                if let Some(v) = v {
                    let _ = s;
                    self.f.locals.insert(def, Local::Val(v));
                }
            }
            _ => {
                let slot = self.slot_for(vt);
                self.push_scope();
                let r = self.into(init, slot, vt);
                self.pop_scope();
                if matches!(r, Res::Never) {
                    return;
                }
                let flag = self.own(slot, vt, true);
                self.f.locals.insert(
                    def,
                    Local::Slot {
                        ptr: slot,
                        ty: vt,
                        flag,
                    },
                );
            }
        }
    }

    // ---- Control flow -----------------------------------------------------------------------------------------------------

    /// The IR value type of an `if` / `loop` / ... result: `Void` for none or never.
    fn arm_ty(&mut self, t: Ty) -> Repr {
        self.repr(t)
    }

    fn if_expr(
        &mut self,
        e: &'a Expr,
        i: &'a IfExpr,
        unless: bool,
        dst: Option<Value>,
        want: Ty,
    ) -> Res {
        let rt = self.ty_of(e).unwrap_or(self.types.void);
        let repr = self.arm_ty(rt);
        let then_b = self.new_block(&[]);
        let else_b = self.new_block(&[]);
        self.push_scope();
        if unless {
            self.cond_branch(&i.cond, else_b, then_b);
        } else {
            self.cond_branch(&i.cond, then_b, else_b);
        }
        let join_params: Vec<IrTy> = match repr {
            Repr::Scalar(s) => vec![s],
            _ => vec![],
        };
        let join = self.new_block(&join_params);
        let mut reached = false;
        self.start(then_b);
        let r = self.block(
            &i.then,
            if matches!(repr, Repr::Mem(_)) {
                dst
            } else {
                None
            },
            want,
        );
        reached |= self.finish_arm(r, join, repr);
        self.start(else_b);
        let r = match &i.else_ {
            Some(el) => self.arm_expr(el, repr, dst, want),
            None => Res::Unit,
        };
        reached |= self.finish_arm(r, join, repr);
        // The condition's temporaries end where the arms join.
        if !reached {
            self.f.scopes.pop();
            self.f.cur = None;
            return Res::Never;
        }
        self.start(join);
        self.pop_scope();
        match repr {
            Repr::Scalar(_) => Res::Val(self.f.func.block_params(join)[0]),
            _ => Res::Unit,
        }
    }

    /// An arm that is an expression (`else` branch, ternary side, `case` body expression).
    pub(crate) fn arm_expr(
        &mut self,
        e: &'a Expr,
        repr: Repr,
        dst: Option<Value>,
        want: Ty,
    ) -> Res {
        self.push_scope();
        let r = match repr {
            Repr::Mem(_) => match dst {
                Some(d) => self.into(e, d, want),
                None => self.expr(e),
            },
            _ => self.expr(e),
        };
        if self.f.cur.is_some() {
            self.pop_scope();
        } else {
            self.f.scopes.pop();
        }
        r
    }

    /// Jumps to the join block with the arm's value. Gives whether control reaches the join.
    pub(crate) fn finish_arm(&mut self, r: Res, join: BlockId, repr: Repr) -> bool {
        if self.f.cur.is_none() || matches!(r, Res::Never) {
            return false;
        }
        match (r, repr) {
            (Res::Val(v), Repr::Scalar(_)) => {
                self.jump(join, vec![v]);
                true
            }
            (_, Repr::Scalar(_)) => {
                self.unsupported(Span::new(0, 0), "an arm without a value");
                false
            }
            _ => {
                self.jump(join, vec![]);
                true
            }
        }
    }

    fn ternary(
        &mut self,
        e: &'a Expr,
        cond: &'a Expr,
        then: &'a Expr,
        else_: &'a Expr,
        dst: Option<Value>,
        want: Ty,
    ) -> Res {
        let rt = self.ty_of(e).unwrap_or(self.types.void);
        let repr = self.arm_ty(rt);
        let then_b = self.new_block(&[]);
        let else_b = self.new_block(&[]);
        self.push_scope();
        self.cond_branch(cond, then_b, else_b);
        let params: Vec<IrTy> = match repr {
            Repr::Scalar(s) => vec![s],
            _ => vec![],
        };
        let join = self.new_block(&params);
        let mut reached = false;
        self.start(then_b);
        let r = self.arm_expr(then, repr, dst, want);
        reached |= self.finish_arm(r, join, repr);
        self.start(else_b);
        let r = self.arm_expr(else_, repr, dst, want);
        reached |= self.finish_arm(r, join, repr);
        if !reached {
            self.f.scopes.pop();
            self.f.cur = None;
            return Res::Never;
        }
        self.start(join);
        self.pop_scope();
        match repr {
            Repr::Scalar(_) => Res::Val(self.f.func.block_params(join)[0]),
            _ => Res::Unit,
        }
    }

    fn while_loop(&mut self, cond: &'a Expr, body: &'a Block) -> Res {
        let label = self.f.pending_label.take();
        let head = self.new_block(&[]);
        let body_b = self.new_block(&[]);
        let no_b = self.new_block(&[]);
        let exit = self.new_block(&[]);
        self.jump(head, vec![]);
        self.start(head);
        let depth = self.f.scopes.len();
        self.push_scope();
        self.cond_branch(cond, body_b, no_b);
        // When the condition fails, what it made is dropped before leaving.
        let saved = self.f.cur;
        self.start(no_b);
        self.exit_scopes(depth);
        self.jump(exit, vec![]);
        self.f.cur = saved;
        self.f.loops.push(LoopCtx {
            label,
            cont: head,
            exit,
            break_ty: None,
            break_dst: None,
            scope_depth: depth,
        });
        self.start(body_b);
        let void = self.types.void;
        let r = self.block(body, None, void);
        if !matches!(r, Res::Never) {
            // The condition's temporaries end with the iteration.
            self.pop_scope_at_end();
            self.jump(head, vec![]);
        } else {
            self.f.scopes.pop();
        }
        self.f.loops.pop();
        self.start(exit);
        Res::Unit
    }

    /// Runs the drops of the innermost scope and removes it (when control is still live).
    fn pop_scope_at_end(&mut self) {
        self.pop_scope();
    }

    fn loop_expr(&mut self, e: &'a Expr, body: &'a Block, dst: Option<Value>, want: Ty) -> Res {
        let label = self.f.pending_label.take();
        let rt = self.ty_of(e).unwrap_or(self.types.void);
        let repr = self.repr(rt);
        let params: Vec<IrTy> = match repr {
            Repr::Scalar(s) => vec![s],
            _ => vec![],
        };
        let head = self.new_block(&[]);
        let exit = self.new_block(&params);
        self.jump(head, vec![]);
        self.start(head);
        let break_ty = match repr {
            Repr::Scalar(s) => Some(s),
            _ => None,
        };
        let break_dst = if matches!(repr, Repr::Mem(_)) {
            dst
        } else {
            None
        };
        let _ = want;
        let depth = self.f.scopes.len();
        self.f.loops.push(LoopCtx {
            label,
            cont: head,
            exit,
            break_ty,
            break_dst,
            scope_depth: depth,
        });
        let void = self.types.void;
        let r = self.block(body, None, void);
        if !matches!(r, Res::Never) {
            self.jump(head, vec![]);
        }
        self.f.loops.pop();
        // The exit is reached only through `break`; if nothing broke, the loop never ends.
        let reached = self.f.func.blocks.iter().any(|b| match &b.term {
            Some(Term::Jump(t)) => t.block == exit,
            Some(Term::Branch(_, a, c)) => a.block == exit || c.block == exit,
            _ => false,
        });
        if !reached {
            self.f.cur = None;
            return Res::Never;
        }
        self.start(exit);
        match repr {
            Repr::Scalar(_) => Res::Val(self.f.func.block_params(exit)[0]),
            _ => Res::Unit,
        }
    }

    /// Runs the body of a loop: a block, or code made by the caller (a builder's clauses).
    pub(crate) fn run_body(&mut self, body: &mut LoopBody<'a, '_>, want: Ty) -> Res {
        match body {
            LoopBody::Block(b) => self.block(b, None, want),
            LoopBody::Code(f) => {
                self.push_scope();
                let r = f(self);
                if matches!(r, Res::Never) {
                    self.f.scopes.pop();
                } else {
                    self.pop_scope();
                }
                r
            }
        }
    }

    fn for_loop(&mut self, e: &'a Expr, f: &'a ForExpr) -> Res {
        let label = self.f.pending_label.take();
        let (pats, iter, keys_only): (&'a [Pattern], &'a Expr, bool) = match &f.head {
            ForHead::Of { pats, iter } => (pats, iter, false),
            ForHead::In { pat, iter } => (std::slice::from_ref(pat), iter, true),
            ForHead::C { init, cond, step } => {
                return self.c_for(label, init.as_deref(), cond.as_ref(), step.as_ref(), &f.body)
            }
        };
        if let ExprKind::Range {
            start: Some(start),
            end: Some(end),
            inclusive,
            step,
        } = &iter.kind
        {
            let step = step.as_deref();
            return self.for_range(
                label,
                pats,
                start,
                end,
                *inclusive,
                step,
                &mut LoopBody::Block(&f.body),
                iter.span,
            );
        }
        let _ = e;
        self.for_each(label, pats, iter, &mut LoopBody::Block(&f.body), keys_only)
    }

    /// `for (init; cond; step) { body }`.
    fn c_for(
        &mut self,
        label: Option<String>,
        init: Option<&'a Stmt>,
        cond: Option<&'a Expr>,
        step: Option<&'a Expr>,
        body: &'a Block,
    ) -> Res {
        self.push_scope();
        if let Some(s) = init {
            self.stmt(s);
        }
        let head = self.new_block(&[]);
        let body_b = self.new_block(&[]);
        let no_b = self.new_block(&[]);
        let step_b = self.new_block(&[]);
        let exit = self.new_block(&[]);
        self.jump(head, vec![]);
        self.start(head);
        let depth = self.f.scopes.len();
        self.push_scope();
        match cond {
            Some(c) => self.cond_branch(c, body_b, no_b),
            None => self.jump(body_b, vec![]),
        }
        let saved = self.f.cur;
        self.start(no_b);
        self.exit_scopes(depth);
        self.jump(exit, vec![]);
        self.f.cur = saved;
        self.f.loops.push(LoopCtx {
            label,
            cont: step_b,
            exit,
            break_ty: None,
            break_dst: None,
            scope_depth: depth,
        });
        self.start(body_b);
        let void = self.types.void;
        let r = self.block(body, None, void);
        if !matches!(r, Res::Never) {
            self.pop_scope();
            self.jump(step_b, vec![]);
        } else {
            self.f.scopes.pop();
        }
        self.f.loops.pop();
        self.start(step_b);
        if let Some(st) = step {
            self.push_scope();
            self.expr(st);
            self.pop_scope();
        }
        self.jump(head, vec![]);
        self.start(exit);
        self.pop_scope();
        Res::Unit
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_range(
        &mut self,
        label: Option<String>,
        pats: &'a [Pattern],
        start: &'a Expr,
        end: &'a Expr,
        inclusive: bool,
        step_e: Option<&'a Expr>,
        body: &mut LoopBody<'a, '_>,
        span: Span,
    ) -> Res {
        let [pat] = pats else {
            return self.unsupported(span, "this `for` pattern");
        };
        let var = match &pat.kind {
            PatternKind::Binding { name, mode: None } => {
                self.res.decls.get(&(self.f.file, name.span.start)).copied()
            }
            PatternKind::Wildcard => None,
            _ => return self.unsupported(pat.span, "this `for` pattern"),
        };
        let Some(sty) = self.ty_of(start) else {
            return self.unsupported(span, "a range of unknown type");
        };
        let Some(ty) = self.scalar_of(sty) else {
            return self.unsupported(span, "a range of this type");
        };
        let signed = self.is_signed(sty);
        let Some(a) = self.scalar(start) else {
            return Res::Never;
        };
        let Some(b) = self.scalar(end) else {
            return Res::Never;
        };
        let step_v = match step_e {
            Some(s) => match self.scalar(s) {
                Some(v) => Some(v),
                None => return Res::Never,
            },
            None => None,
        };
        let head = self.new_block(&[ty]);
        let body_b = self.new_block(&[]);
        let step = self.new_block(&[]);
        let exit = self.new_block(&[]);
        self.jump(head, vec![a]);
        self.start(head);
        let i = self.f.func.block_params(head)[0];
        let cmp = match (inclusive, signed) {
            (false, true) => CmpOp::Lt,
            (false, false) => CmpOp::ULt,
            (true, true) => CmpOp::Le,
            (true, false) => CmpOp::ULe,
        };
        let up = self.emit(Op::Cmp(cmp, i, b), Some(IrTy::Bool), span);
        let cond = match (step_v, up) {
            (Some(sv), Some(up)) if signed || ty.is_float() => {
                // A step below zero walks down: the loop runs while the value is above the end.
                let zero = if ty.is_float() {
                    self.konst(ty, Const::Float(0.0), span)
                } else {
                    self.konst(ty, Const::Int(0), span)
                };
                let pos = self.emit(Op::Cmp(CmpOp::Gt, sv, zero), Some(IrTy::Bool), span);
                let down_op = if inclusive { CmpOp::Ge } else { CmpOp::Gt };
                let down = self.emit(Op::Cmp(down_op, i, b), Some(IrTy::Bool), span);
                match (pos, down) {
                    (Some(pos), Some(down)) => match self.select(pos, up, down, IrTy::Bool, span) {
                        Res::Val(v) => Some(v),
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => up,
        };
        if let Some(c) = cond {
            self.branch(c, body_b, exit);
        }
        if let Some(d) = var {
            self.f.locals.insert(d, Local::Val(i));
        }
        let depth = self.f.scopes.len();
        self.f.loops.push(LoopCtx {
            label,
            cont: step,
            exit,
            break_ty: None,
            break_dst: None,
            scope_depth: depth,
        });
        self.start(body_b);
        let void = self.types.void;
        let r = self.run_body(body, void);
        if !matches!(r, Res::Never) {
            self.jump(step, vec![]);
        }
        self.f.loops.pop();
        self.start(step);
        let next = if let Some(sv) = step_v {
            self.emit(Op::Bin(BinOp::Add, i, sv), Some(ty), span)
        } else if inclusive {
            // After the last value, stop without adding (the add could overflow).
            let at_end = self.emit(Op::Cmp(CmpOp::Eq, i, b), Some(IrTy::Bool), span);
            let more = self.new_block(&[]);
            if let Some(c) = at_end {
                self.branch(c, exit, more);
            }
            self.start(more);
            let one = self.konst(ty, Const::Int(1), span);
            self.emit(Op::Bin(BinOp::Add, i, one), Some(ty), span)
        } else {
            let one = self.konst(ty, Const::Int(1), span);
            self.emit(Op::Bin(BinOp::Add, i, one), Some(ty), span)
        };
        if let Some(n) = next {
            self.jump(head, vec![n]);
        }
        self.start(exit);
        Res::Unit
    }

    fn find_loop(&self, label: Option<&Ident>) -> Option<usize> {
        match label {
            Some(l) => {
                let name = self.src_text(l.span);
                self.f
                    .loops
                    .iter()
                    .rposition(|c| c.label.as_deref() == Some(name))
            }
            None => self.f.loops.len().checked_sub(1),
        }
    }

    fn break_expr(&mut self, e: &'a Expr, label: Option<&Ident>, value: Option<&'a Expr>) -> Res {
        let Some(i) = self.find_loop(label) else {
            return self.unsupported(e.span, "`break` outside a loop");
        };
        let exit = self.f.loops[i].exit;
        let want = self.f.loops[i].break_ty;
        let bdst = self.f.loops[i].break_dst;
        let depth = self.f.loops[i].scope_depth;
        let args = match (value, want, bdst) {
            (Some(v), Some(_), _) => match self.consume_scalar(v) {
                Some(x) => vec![x],
                None => return Res::Never,
            },
            (Some(v), None, Some(d)) => {
                let t = self.ty_of(v).unwrap_or(self.types.void);
                if let Res::Never = self.into(v, d, t) {
                    return Res::Never;
                }
                vec![]
            }
            (None, None, _) => vec![],
            (Some(v), None, None) => {
                self.expr(v);
                vec![]
            }
            _ => return self.unsupported(e.span, "`break` with a mismatched value"),
        };
        self.exit_scopes(depth);
        self.jump(exit, args);
        Res::Never
    }

    fn continue_expr(&mut self, e: &'a Expr, label: Option<&Ident>) -> Res {
        let Some(i) = self.find_loop(label) else {
            return self.unsupported(e.span, "`continue` outside a loop");
        };
        let to = self.f.loops[i].cont;
        let depth = self.f.loops[i].scope_depth;
        self.exit_scopes(depth);
        self.jump(to, vec![]);
        Res::Never
    }

    fn return_expr(&mut self, e: &'a Expr, v: Option<&'a Expr>) -> Res {
        let ret = self.f.ret_ty;
        match (v, self.repr(ret)) {
            (Some(x), Repr::Scalar(_)) => {
                let Some(val) = self.consume_scalar(x) else {
                    return Res::Never;
                };
                self.exit_scopes(0);
                self.terminate(Term::Return(Some(val)));
            }
            (Some(x), Repr::Mem(_)) => {
                let dst = self.f.sret.unwrap_or(Value(0));
                if let Res::Never = self.into(x, dst, ret) {
                    return Res::Never;
                }
                self.exit_scopes(0);
                self.terminate(Term::Return(None));
            }
            (None, Repr::Mem(_)) => {
                // `return` in a `void ! E` function: success.
                self.write_ok_void(ret);
                self.exit_scopes(0);
                self.terminate(Term::Return(None));
            }
            (Some(x), Repr::Zst) => {
                self.expr(x);
                self.exit_scopes(0);
                self.terminate(Term::Return(None));
            }
            (None, Repr::Zst) => {
                self.exit_scopes(0);
                self.terminate(Term::Return(None));
            }
            _ => return self.unsupported(e.span, "this `return`"),
        }
        Res::Never
    }
}

pub(crate) fn strip_parens(e: &Expr) -> &Expr {
    match &e.kind {
        ExprKind::Paren(i) => strip_parens(i),
        _ => e,
    }
}
