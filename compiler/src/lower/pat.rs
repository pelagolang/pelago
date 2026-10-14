//! Patterns: `switch`, `is`, `guard`, destructuring, and conditions that bind names.
//!
//! A pattern is tested against a value in memory (its address). Each test that can fail branches to a
//! failure block; names the pattern binds become aliases of the matched parts (views, nothing is
//! copied or moved). Arms are tried in order.

use super::expr::strip_parens;
use super::*;

impl<'a> Lowerer<'a> {
    // ---- Conditions -----------------------------------------------------------------------------------------------

    /// Branches on a condition. `&&`, `||` and `!` become jumps, and `is` binds names for the code it
    /// guards.
    pub(crate) fn cond_branch(&mut self, c: &'a Expr, yes: BlockId, no: BlockId) {
        if self.f.cur.is_none() || self.failed() {
            return;
        }
        let c = strip_parens(c);
        match &c.kind {
            ExprKind::Binary {
                op: Punct::AndAnd,
                lhs,
                rhs,
            } => {
                let mid = self.new_block(&[]);
                self.cond_branch(lhs, mid, no);
                self.start(mid);
                self.cond_branch(rhs, yes, no);
            }
            ExprKind::Binary {
                op: Punct::OrOr,
                lhs,
                rhs,
            } => {
                let mid = self.new_block(&[]);
                self.cond_branch(lhs, yes, mid);
                self.start(mid);
                self.cond_branch(rhs, yes, no);
            }
            ExprKind::Unary {
                op: Punct::Bang,
                operand,
            } => self.cond_branch(operand, no, yes),
            ExprKind::Is { value, pattern } => {
                let Some((p, t)) = self.scrutinee(value) else {
                    return;
                };
                if self.pat_test(pattern, p, t, no) {
                    self.jump(yes, vec![]);
                }
            }
            _ => {
                if let Some(v) = self.scalar(c) {
                    self.branch(v, yes, no);
                }
            }
        }
    }

    /// `value is pattern` used as a value: true or false.
    pub(crate) fn is_expr(&mut self, e: &'a Expr, value: &'a Expr, pattern: &'a Pattern) -> Res {
        let _ = (value, pattern);
        let yes = self.new_block(&[]);
        let no = self.new_block(&[]);
        let join = self.new_block(&[IrTy::Bool]);
        self.cond_branch(e, yes, no);
        self.start(yes);
        let t = self.konst(IrTy::Bool, Const::Bool(true), e.span);
        self.jump(join, vec![t]);
        self.start(no);
        let f = self.konst(IrTy::Bool, Const::Bool(false), e.span);
        self.jump(join, vec![f]);
        self.start(join);
        Res::Val(self.f.func.block_params(join)[0])
    }

    /// `guard cond else { ... }`: the else block must leave; names bound by `cond` stay visible.
    pub(crate) fn guard_expr(&mut self, cond: &'a Expr, else_: &'a Block) -> Res {
        let ok = self.new_block(&[]);
        let bad = self.new_block(&[]);
        self.cond_branch(cond, ok, bad);
        let saved_cur = self.f.cur;
        self.start(bad);
        let void = self.types.void;
        let r = self.block(else_, None, void);
        if !matches!(r, Res::Never) && self.f.cur.is_some() {
            self.unsupported(else_.span, "a `guard` block that does not leave");
        }
        self.f.cur = saved_cur;
        self.start(ok);
        Res::Unit
    }

    // ---- The scrutinee --------------------------------------------------------------------------------------------

    /// The address of the value a pattern is tested against, and its type.
    pub(crate) fn scrutinee(&mut self, e: &'a Expr) -> Option<(Value, Ty)> {
        let t = self.ty_of(e)?;
        if let Some(p) = self.place(e) {
            return Some((p, t));
        }
        if self.failed() {
            return None;
        }
        match self.repr(t) {
            Repr::Mem(_) => match self.expr(e) {
                Res::Val(p) => Some((p, t)),
                _ => None,
            },
            Repr::Scalar(_) => {
                let v = self.scalar(e)?;
                let slot = self.slot_for(t);
                self.store_scalar_ty(slot, v);
                Some((slot, t))
            }
            Repr::Zst => {
                let slot = self.slot_bytes(1, 1);
                self.expr(e);
                Some((slot, t))
            }
        }
    }

    // ---- Testing a pattern --------------------------------------------------------------------------------------------

    /// Tests `p` against the value at `ptr`; jumps to `fail` when it does not match. On success
    /// control continues in the current block with the pattern's names bound. Gives false when
    /// lowering failed.
    pub(crate) fn pat_test(&mut self, p: &'a Pattern, ptr: Value, ty: Ty, fail: BlockId) -> bool {
        if self.f.cur.is_none() || self.failed() {
            return false;
        }
        match &p.kind {
            PatternKind::Wildcard | PatternKind::Error => true,
            PatternKind::Binding { name, .. } => {
                self.bind_alias(name, ptr, ty);
                true
            }
            PatternKind::As { name, pattern } => {
                self.bind_alias(name, ptr, ty);
                self.pat_test(pattern, ptr, ty, fail)
            }
            PatternKind::Literal(lit) => self.test_literal(lit, ptr, ty, fail),
            PatternKind::Range {
                start,
                end,
                inclusive,
            } => self.test_range(start.as_deref(), end.as_deref(), *inclusive, ptr, ty, fail),
            PatternKind::Tuple(ps) => {
                if ps.is_empty() {
                    return true;
                }
                let fs = self.struct_fields(ty);
                if fs.len() != ps.len() {
                    self.unsupported(p.span, "a tuple pattern of this shape");
                    return false;
                }
                for (sub, f) in ps.iter().zip(fs.iter()) {
                    let q = self.addr(ptr, f.off);
                    if !self.pat_test(sub, q, f.ty, fail) {
                        return false;
                    }
                }
                true
            }
            PatternKind::None => self.test_tag(ptr, ty, 0, fail, p.span),
            PatternKind::Variant {
                leading_dot,
                path,
                args,
            } => self.test_variant(p, *leading_dot, path, args.as_deref(), ptr, ty, fail),
            PatternKind::Struct { path, fields, .. } => {
                self.test_struct(p, path, fields, ptr, ty, fail)
            }
            PatternKind::List(items) => self.test_list(p, items, ptr, ty, fail),
            PatternKind::TypeTest(path) => self.test_type(p, path, ptr, ty, fail),
        }
    }

    /// The definition of a name a pattern binds (the first alternative declares it, later ones use it).
    fn binding_def(&self, name: &Ident) -> Option<DefId> {
        self.res
            .decls
            .get(&(self.f.file, name.span.start))
            .copied()
            .or_else(|| self.res.use_at(self.f.file, name.span.start))
    }

    fn bind_alias(&mut self, name: &Ident, ptr: Value, ty: Ty) {
        if let Some(def) = self.binding_def(name) {
            self.f.locals.insert(
                def,
                Local::Slot {
                    ptr,
                    ty,
                    flag: None,
                },
            );
        }
    }

    /// Branches to `fail` unless `cond` holds.
    fn require(&mut self, cond: Value, fail: BlockId) {
        let ok = self.new_block(&[]);
        self.branch(cond, ok, fail);
        self.start(ok);
    }

    fn test_tag(&mut self, ptr: Value, ty: Ty, idx: usize, fail: BlockId, span: Span) -> bool {
        let Some(info) = self.enum_info(ty) else {
            self.unsupported(span, "a case pattern for this type");
            return false;
        };
        let tag = self.load_at(ptr, 0, info.tag);
        let want = self.konst(info.tag, Const::Int(idx as i128), span);
        if let Some(c) = self.emit(Op::Cmp(CmpOp::Eq, tag, want), Some(IrTy::Bool), span) {
            self.require(c, fail);
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn test_variant(
        &mut self,
        p: &'a Pattern,
        leading_dot: bool,
        path: &'a Path,
        args: Option<&'a [PatArg]>,
        ptr: Value,
        ty: Ty,
        fail: BlockId,
    ) -> bool {
        let Some(last) = path.segments.last() else {
            return false;
        };
        // `some`, `ok` and `err`.
        if !leading_dot && path.segments.len() == 1 {
            if let Some(d) = self.res.use_at(self.f.file, last.span.start) {
                if matches!(
                    self.res.def(d).kind,
                    DefKind::BuiltinFn | DefKind::BuiltinCase
                ) {
                    let idx = match self.res.def(d).name.as_str() {
                        "some" | "err" => 1,
                        _ => 0,
                    };
                    return self.test_payload(p, ptr, ty, idx, args, &[], fail);
                }
            }
        }
        // A user enum case.
        let TyKind::Adt(d, _) = self.kind(ty) else {
            self.unsupported(p.span, "a case pattern for this type");
            return false;
        };
        let variant = if leading_dot {
            self.res.member(d, self.src_text(last.span))
        } else {
            self.res.use_at(self.f.file, last.span.start)
        };
        let Some(v) = variant.filter(|v| self.res.def(*v).kind == DefKind::Variant) else {
            self.unsupported(p.span, "an unknown case");
            return false;
        };
        let Some(idx) = self.variant_index(d, v) else {
            return false;
        };
        let labels: Vec<Option<String>> = self
            .typed
            .adts
            .get(&d)
            .and_then(|a| a.variants.get(idx))
            .map(|(_, fs)| fs.iter().map(|(l, _)| l.clone()).collect())
            .unwrap_or_default();
        self.test_payload(p, ptr, ty, idx, args, &labels, fail)
    }

    /// Tests the tag, then the patterns for the payload of that case.
    #[allow(clippy::too_many_arguments)]
    fn test_payload(
        &mut self,
        p: &'a Pattern,
        ptr: Value,
        ty: Ty,
        idx: usize,
        args: Option<&'a [PatArg]>,
        labels: &[Option<String>],
        fail: BlockId,
    ) -> bool {
        if !self.test_tag(ptr, ty, idx, fail, p.span) {
            return false;
        }
        let Some(info) = self.enum_info(ty) else {
            return false;
        };
        let fields = info.variants[idx].clone();
        let Some(args) = args else {
            return true;
        };
        let mut next = 0usize;
        for a in args {
            let i = match &a.label {
                Some(l) => {
                    let n = self.src_text(l.span);
                    labels
                        .iter()
                        .position(|x| x.as_deref() == Some(n))
                        .unwrap_or(next)
                }
                None => next,
            };
            next += 1;
            let Some(f) = fields.get(i) else {
                self.unsupported(p.span, "a case pattern with too many values");
                return false;
            };
            let q = self.addr(ptr, f.off);
            if !self.pat_test(&a.pattern, q, f.ty, fail) {
                return false;
            }
        }
        true
    }

    fn test_struct(
        &mut self,
        p: &'a Pattern,
        path: &'a Path,
        fields: &'a [FieldPat],
        ptr: Value,
        ty: Ty,
        fail: BlockId,
    ) -> bool {
        let _ = path;
        let TyKind::Adt(d, _) = self.kind(ty) else {
            self.unsupported(p.span, "a struct pattern for this type");
            return false;
        };
        let fs = self.struct_fields(ty);
        let decls: Vec<DefId> = self
            .typed
            .adts
            .get(&d)
            .map(|a| a.fields.iter().map(|(f, _)| *f).collect())
            .unwrap_or_default();
        for fp in fields {
            let name = self.src_text(fp.name.span);
            let Some(i) = decls.iter().position(|f| self.res.def(*f).name == name) else {
                self.unsupported(fp.name.span, "an unknown field");
                return false;
            };
            let f = fs[i];
            let q = self.addr(ptr, f.off);
            match &fp.pattern {
                Some(sub) => {
                    if !self.pat_test(sub, q, f.ty, fail) {
                        return false;
                    }
                }
                None => self.bind_alias(&fp.name, q, f.ty),
            }
        }
        true
    }

    fn test_literal(&mut self, lit: &'a Expr, ptr: Value, ty: Ty, fail: BlockId) -> bool {
        match self.repr(ty) {
            Repr::Scalar(s) => {
                let Some(have) = self.scalar(lit) else {
                    return false;
                };
                let cur = self.load_at(ptr, 0, s);
                if let Some(c) =
                    self.emit(Op::Cmp(CmpOp::Eq, cur, have), Some(IrTy::Bool), lit.span)
                {
                    self.require(c, fail);
                }
                true
            }
            _ => {
                // Text.
                let Some(c) = self.text_equals_literal(ptr, ty, lit) else {
                    return false;
                };
                self.require(c, fail);
                true
            }
        }
    }

    fn test_range(
        &mut self,
        start: Option<&'a Expr>,
        end: Option<&'a Expr>,
        inclusive: bool,
        ptr: Value,
        ty: Ty,
        fail: BlockId,
    ) -> bool {
        let Some(s) = self.scalar_of(ty) else {
            self.unsupported(Span::new(0, 0), "a range pattern for this type");
            return false;
        };
        let signed = self.is_signed(ty) || s.is_float();
        let cur = self.load_at(ptr, 0, s);
        if let Some(lo) = start {
            let Some(lo) = self.scalar(lo) else {
                return false;
            };
            let op = if signed { CmpOp::Ge } else { CmpOp::UGe };
            if let Some(c) = self.emit(Op::Cmp(op, cur, lo), Some(IrTy::Bool), Span::new(0, 0)) {
                self.require(c, fail);
            }
        }
        if let Some(hi) = end {
            let Some(hi) = self.scalar(hi) else {
                return false;
            };
            let op = match (inclusive, signed) {
                (true, true) => CmpOp::Le,
                (true, false) => CmpOp::ULe,
                (false, true) => CmpOp::Lt,
                (false, false) => CmpOp::ULt,
            };
            if let Some(c) = self.emit(Op::Cmp(op, cur, hi), Some(IrTy::Bool), Span::new(0, 0)) {
                self.require(c, fail);
            }
        }
        true
    }

    fn test_list(
        &mut self,
        p: &'a Pattern,
        items: &'a [ListPatItem],
        ptr: Value,
        ty: Ty,
        fail: BlockId,
    ) -> bool {
        let elem = match self.kind(ty) {
            TyKind::List(e) | TyKind::Iter(e) => e,
            _ => {
                self.unsupported(p.span, "a list pattern for this type");
                return false;
            }
        };
        let esize = self.layout(elem).size as i64;
        let mut prefix: Vec<&'a Pattern> = Vec::new();
        let mut suffix: Vec<&'a Pattern> = Vec::new();
        let mut rest: Option<&'a ListPatItem> = None;
        for it in items {
            match it {
                ListPatItem::Pattern(sub) => {
                    if rest.is_some() {
                        suffix.push(sub);
                    } else {
                        prefix.push(sub);
                    }
                }
                ListPatItem::Rest { .. } => rest = Some(it),
            }
        }
        let n = (prefix.len() + suffix.len()) as i64;
        let len = self.load_at(ptr, 8, IrTy::I64);
        let want = self.i64c(n);
        let op = if rest.is_some() { CmpOp::Ge } else { CmpOp::Eq };
        if let Some(c) = self.emit(Op::Cmp(op, len, want), Some(IrTy::Bool), p.span) {
            self.require(c, fail);
        }
        let data = self.load_at(ptr, 0, IrTy::Ptr);
        for (i, sub) in prefix.iter().enumerate() {
            let off = self.i64c(i as i64 * esize);
            let q = self
                .emit(Op::PtrAdd(data, off), Some(IrTy::Ptr), p.span)
                .unwrap_or(data);
            if !self.pat_test(sub, q, elem, fail) {
                return false;
            }
        }
        let k = suffix.len() as i64;
        for (j, sub) in suffix.iter().enumerate() {
            // Counted from the end: element `len - k + j`.
            let kk = self.i64c(k - j as i64);
            let from_end = self
                .emit(Op::Bin(BinOp::Sub, len, kk), Some(IrTy::I64), p.span)
                .unwrap_or(len);
            let sz = self.i64c(esize);
            let off = self
                .emit(Op::Bin(BinOp::Mul, from_end, sz), Some(IrTy::I64), p.span)
                .unwrap_or(len);
            let q = self
                .emit(Op::PtrAdd(data, off), Some(IrTy::Ptr), p.span)
                .unwrap_or(data);
            if !self.pat_test(sub, q, elem, fail) {
                return false;
            }
        }
        if let Some(ListPatItem::Rest { name: Some(nm), .. }) = rest {
            // The rest is a view of the middle: a list that owns nothing.
            let view = self.slot_for(ty);
            let skip = self.i64c(prefix.len() as i64 * esize);
            let start = self
                .emit(Op::PtrAdd(data, skip), Some(IrTy::Ptr), p.span)
                .unwrap_or(data);
            let kc = self.i64c(n);
            let rest_len = self
                .emit(Op::Bin(BinOp::Sub, len, kc), Some(IrTy::I64), p.span)
                .unwrap_or(len);
            let zero = self.i64c(0);
            self.store_at(view, 0, start);
            self.store_at(view, 8, rest_len);
            self.store_at(view, 16, zero);
            self.bind_alias(nm, view, ty);
        }
        true
    }

    fn test_type(
        &mut self,
        p: &'a Pattern,
        path: &'a Path,
        ptr: Value,
        ty: Ty,
        fail: BlockId,
    ) -> bool {
        let Some(last) = path.segments.last() else {
            return false;
        };
        let Some(def) = self.res.use_at(self.f.file, last.span.start) else {
            self.unsupported(p.span, "a type test for an unknown type");
            return false;
        };
        if self.res.def(def).kind != DefKind::Obj || self.obj_def(ty).is_none() {
            self.unsupported(p.span, "a type test on this type");
            return false;
        }
        let target = self.types.mk(TyKind::Adt(def, vec![]));
        let o = self.load_at(ptr, 0, IrTy::Ptr);
        let Some(ok) = self.obj_is(o, target) else {
            return false;
        };
        let more = self.new_block(&[]);
        self.branch(ok, more, fail);
        self.start(more);
        true
    }

    // ---- switch ------------------------------------------------------------------------------------------------------

    pub(crate) fn switch_expr(
        &mut self,
        e: &'a Expr,
        s: &'a Switch,
        dst: Option<Value>,
        want: Ty,
    ) -> Res {
        let rt = self.ty_of(e).unwrap_or(self.types.void);
        let repr = self.repr(rt);
        self.push_scope();
        let Some((ptr, sty)) = self.scrutinee(&s.scrutinee) else {
            self.f.scopes.pop();
            return Res::Never;
        };
        let join_params: Vec<IrTy> = match repr {
            Repr::Scalar(x) => vec![x],
            _ => vec![],
        };
        let join = self.new_block(&join_params);
        let mut reached = false;
        let mem_dst = if matches!(repr, Repr::Mem(_)) {
            dst
        } else {
            None
        };
        for arm in &s.arms {
            if self.f.cur.is_none() || self.failed() {
                break;
            }
            let next = self.new_block(&[]);
            let body_start = self.arm_entry(arm, ptr, sty, next);
            let Some(()) = body_start else {
                return Res::Never;
            };
            if let Some(g) = &arm.guard {
                let ok = self.new_block(&[]);
                self.cond_branch(g, ok, next);
                self.start(ok);
            }
            let r = self.block(&arm.body, mem_dst, want);
            reached |= self.finish_arm(r, join, repr);
            self.start(next);
        }
        // Past the last arm nothing matched; the checker proved that cannot happen.
        let stop = self.stop_block("no case matched");
        self.jump(stop, vec![]);
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

    /// Tests the patterns of one arm. Control continues with their names bound when one matches, and
    /// goes to `next` when none does.
    fn arm_entry(&mut self, arm: &'a SwitchArm, ptr: Value, sty: Ty, next: BlockId) -> Option<()> {
        if arm.is_default || arm.patterns.is_empty() {
            return Some(());
        }
        if arm.patterns.len() == 1 {
            return self
                .pat_test(&arm.patterns[0], ptr, sty, next)
                .then_some(());
        }
        // Alternatives share one body: each hands the addresses of the names to the body block.
        let mut names: Vec<(Ident, Option<Mode>)> = Vec::new();
        crate::resolve::collect_pattern_names(&arm.patterns[0], &mut names);
        let defs: Vec<DefId> = names
            .iter()
            .filter_map(|(n, _)| self.binding_def(n))
            .collect();
        let tys: Vec<Ty> = defs
            .iter()
            .map(|d| {
                let t = self.typed.local_tys.get(d).copied();
                t.map(|t| self.conc(t)).unwrap_or(self.types.void)
            })
            .collect();
        let params: Vec<IrTy> = defs.iter().map(|_| IrTy::Ptr).collect();
        let body = self.new_block(&params);
        let last = arm.patterns.len() - 1;
        for (i, p) in arm.patterns.iter().enumerate() {
            let miss = if i == last { next } else { self.new_block(&[]) };
            if !self.pat_test(p, ptr, sty, miss) {
                return None;
            }
            let args: Vec<Value> = defs
                .iter()
                .map(|d| match self.f.locals.get(d) {
                    Some(Local::Slot { ptr, .. }) => *ptr,
                    _ => ptr,
                })
                .collect();
            self.jump(body, args);
            if i != last {
                self.start(miss);
            }
        }
        self.start(body);
        let ps = self.f.func.block_params(body).to_vec();
        for ((d, t), pv) in defs.iter().zip(tys.iter()).zip(ps) {
            self.f.locals.insert(
                *d,
                Local::Slot {
                    ptr: pv,
                    ty: *t,
                    flag: None,
                },
            );
        }
        Some(())
    }

    // ---- Destructuring declarations --------------------------------------------------------------------------------------

    /// `const (a, b) = pair`, `const point { x, y } = p`: the parts become names.
    pub(crate) fn destructure_decl(&mut self, d: &'a Decl, init: &'a Expr, t: Ty) {
        // The value is owned by a hidden variable; the names are views of its parts.
        let slot = self.slot_for(t);
        self.push_scope();
        let r = self.into(init, slot, t);
        self.pop_scope();
        if matches!(r, Res::Never) {
            return;
        }
        self.own(slot, t, false);
        let fail = self.stop_block("pattern did not match");
        if !self.pat_test(&d.pattern, slot, t, fail) {
            self.unsupported(d.pattern.span, "this pattern");
        }
    }
}
