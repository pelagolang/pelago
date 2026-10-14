//! Builders: `[x of xs if x > 0 ~> x * 2]`, `set { ... }`, `map { ... }` and the lazy form (made eagerly
//! here, as a list). The clauses become nested loops and tests; the innermost step adds one item.

use super::*;

impl<'a> Lowerer<'a> {
    pub(crate) fn builder_expr(&mut self, e: &'a Expr, b: &'a Builder, dst: Value, t: Ty) -> Res {
        let sp = e.span;
        let z = self.i64c(0);
        let np = self
            .emit(Op::Cast(CastKind::IntToPtr, z, IrTy::Ptr), Some(IrTy::Ptr), sp)
            .unwrap_or(dst);
        self.store_at(dst, 0, np);
        for off in [8, 16] {
            let z = self.i64c(0);
            self.store_at(dst, off, z);
        }
        if matches!(self.kind(t), TyKind::Map(..) | TyKind::Set(_)) {
            self.store_at(dst, 24, np);
        }
        self.builder_clauses(b, 0, dst, t)
    }

    fn builder_clauses(&mut self, b: &'a Builder, idx: usize, dst: Value, t: Ty) -> Res {
        let Some(clause) = b.clauses.get(idx) else {
            return self.builder_item(b, dst, t);
        };
        match clause {
            Clause::Of { pats, source } => {
                let mut body = |s: &mut Lowerer<'a>| s.builder_clauses(b, idx + 1, dst, t);
                self.for_each_or_range(pats, source, &mut LoopBody::Code(&mut body))
            }
            Clause::If(cond) => {
                let yes = self.new_block(&[]);
                let no = self.new_block(&[]);
                let join = self.new_block(&[]);
                let depth = self.f.scopes.len();
                self.push_scope();
                self.cond_branch(cond, yes, no);
                let saved = self.f.cur;
                self.start(no);
                self.exit_scopes(depth);
                self.jump(join, vec![]);
                self.f.cur = saved;
                self.start(yes);
                let r = self.builder_clauses(b, idx + 1, dst, t);
                if matches!(r, Res::Never) {
                    self.f.scopes.pop();
                } else {
                    self.pop_scope();
                    self.jump(join, vec![]);
                }
                self.start(join);
                Res::Unit
            }
        }
    }

    /// A `for` over a source, as ranges with both ends get their own counting loop.
    fn for_each_or_range(
        &mut self,
        pats: &'a [Pattern],
        source: &'a Expr,
        body: &mut LoopBody<'a, '_>,
    ) -> Res {
        if let ExprKind::Range {
            start: Some(start),
            end: Some(end),
            inclusive,
            step,
        } = &source.kind
        {
            return self.for_range(
                None,
                pats,
                start,
                end,
                *inclusive,
                step.as_deref(),
                body,
                source.span,
            );
        }
        self.for_each(None, pats, source, body, false)
    }

    /// Adds the item the clauses have bound.
    fn builder_item(&mut self, b: &'a Builder, dst: Value, t: Ty) -> Res {
        let sp = Span::new(0, 0);
        match self.kind(t) {
            TyKind::List(elem) | TyKind::Iter(elem) => {
                let esize = self.layout(elem).size as i64;
                let len = self.load_at(dst, 8, IrTy::I64);
                let one = self.i64c(1);
                let need = self
                    .emit(Op::Bin(BinOp::Add, len, one), Some(IrTy::I64), sp)
                    .unwrap_or(len);
                let sz = self.i64c(esize);
                self.call_core("rt_grow", vec![dst, sz, need]);
                let data = self.load_at(dst, 0, IrTy::Ptr);
                let off = self
                    .emit(Op::Bin(BinOp::Mul, len, sz), Some(IrTy::I64), sp)
                    .unwrap_or(len);
                let p = self
                    .emit(Op::PtrAdd(data, off), Some(IrTy::Ptr), sp)
                    .unwrap_or(data);
                if let Res::Never = self.builder_value(b, p, elem) {
                    return Res::Never;
                }
                self.store_at(dst, 8, need);
                Res::Unit
            }
            TyKind::Set(elem) => {
                let slot = self.slot_for(elem);
                if let Res::Never = self.builder_value(b, slot, elem) {
                    return Res::Never;
                }
                self.call_core_g("rt_set_put", &[elem], vec![dst, slot]);
                Res::Unit
            }
            TyKind::Map(k, v) => {
                let ks = self.slot_for(k);
                if let Res::Never = self.builder_value(b, ks, k) {
                    return Res::Never;
                }
                let vs = self.slot_for(v);
                let Some(val) = &b.value else {
                    return self.unsupported(sp, "a map builder without a value");
                };
                if let Res::Never = self.into(val, vs, v) {
                    return Res::Never;
                }
                self.call_core_g("rt_map_put", &[k, v], vec![dst, ks, vs]);
                Res::Unit
            }
            _ => self.unsupported(sp, "a builder of this type"),
        }
    }

    /// Writes the item: the `~>` expression, or a copy of the first name the clauses bound.
    fn builder_value(&mut self, b: &'a Builder, p: Value, elem: Ty) -> Res {
        if let Some(r) = &b.result {
            return self.into(r, p, elem);
        }
        for c in &b.clauses {
            if let Clause::Of { pats, .. } = c {
                if let Some(Pattern {
                    kind: PatternKind::Binding { name, .. },
                    ..
                }) = pats.last()
                {
                    if let Some(def) = self.res.decls.get(&(self.f.file, name.span.start)).copied() {
                        if let Some(Local::Slot { ptr, .. }) = self.f.locals.get(&def).copied() {
                            self.clone_into(ptr, p, elem);
                            return Res::Unit;
                        }
                        if let Some(Local::Val(v)) = self.f.locals.get(&def).copied() {
                            self.store_scalar_ty(p, v);
                            return Res::Unit;
                        }
                    }
                }
                break;
            }
        }
        self.unsupported(Span::new(0, 0), "a builder without a result expression")
    }
}
