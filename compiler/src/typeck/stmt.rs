//! Blocks, statements and control flow.

use super::*;

impl<'p> Checker<'p> {
    /// Registers declarations found inside a block so they can be used before they appear.
    fn index_local_items(&mut self, stmts: &'p [Stmt]) {
        for s in stmts {
            if let StmtKind::Item(i) = &s.kind {
                if matches!(i.kind, ItemKind::Decl(_) | ItemKind::Import(_)) {
                    continue;
                }
                self.index_item(self.file, self.scope, i);
                self.queue_item(i);
            }
        }
    }

    /// Queues the bodies of the functions in a local item.
    fn queue_item(&mut self, item: &'p Item) {
        let file = self.file;
        let push = |c: &mut Self, i: &'p Item| {
            if let Some(&d) = c.res.item_defs.get(&(file, i.span.start)) {
                c.queue.push(d);
            }
        };
        match &item.kind {
            ItemKind::Fn(_) => push(self, item),
            ItemKind::Struct(d) | ItemKind::Obj(d) | ItemKind::Interface(d) => {
                for m in &d.members {
                    if let Member::Item(i) = m {
                        push(self, i);
                    }
                }
            }
            ItemKind::Enum(e) => {
                for i in &e.items {
                    push(self, i);
                }
            }
            ItemKind::Impl(im) => {
                for m in &im.members {
                    if let Member::Item(i) = m {
                        push(self, i);
                    }
                }
            }
            _ => {}
        }
    }

    pub(crate) fn check_block(&mut self, b: &'p Block, exp: Exp) -> Ty {
        self.index_local_items(&b.stmts);
        let mut diverged = false;
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Decl(d) => self.local_decl(d),
                StmtKind::Item(i) => {
                    if let ItemKind::Decl(d) = &i.kind {
                        self.local_decl(d);
                    }
                }
                StmtKind::Expr(e) => {
                    let t = self.check_expr(e, Exp::Discard);
                    if matches!(self.tt.kind_of(t), TyKind::Never) {
                        diverged = true;
                    }
                }
            }
        }
        match &b.tail {
            Some(t) => self.check_expr(t, exp),
            None if diverged => self.tt.never,
            None => self.tt.void,
        }
    }

    pub(crate) fn local_decl(&mut self, d: &'p Decl) {
        let declared = d.ty.as_ref().map(|t| self.lower_type(t));
        let ty = match (&d.init, declared) {
            (Some(init), Some(t)) => {
                self.check_coerce(init, t);
                t
            }
            (Some(init), None) => self.check_expr(init, Exp::Any),
            (None, Some(t)) => t,
            (None, None) => self.tt.unknown,
        };
        self.check_pattern(&d.pattern, ty);
    }

    pub(crate) fn bind_decl_pattern(&mut self, p: &'p Pattern, ty: Ty) {
        self.check_pattern(p, ty);
    }

    // ---- if / switch / select --------------------------------------------------------------

    pub(crate) fn if_expr(&mut self, i: &'p IfExpr, exp: Exp, _unless: bool) -> Ty {
        self.check_coerce(&i.cond, self.tt.bool_);
        let then_t = self.check_block(&i.then, exp);
        match &i.else_ {
            Some(el) => {
                let else_t = self.check_expr(el, exp);
                self.join(
                    &[(then_t, i.then.span), (else_t, el.span)],
                    exp,
                    "the branches of `if`",
                )
            }
            None => {
                if let Exp::Is(t) = exp {
                    if !matches!(
                        self.tt.kind_of(t),
                        TyKind::Void | TyKind::Unknown | TyKind::Var(_)
                    ) {
                        self.err(i.cond.span, "an `if` without `else` has no value here");
                    }
                }
                self.tt.void
            }
        }
    }

    pub(crate) fn switch_expr(&mut self, e: &'p Expr, s: &'p Switch, exp: Exp) -> Ty {
        let st = self.check_expr(&s.scrutinee, Exp::Any);
        let mut branches: Vec<(Ty, Span)> = Vec::new();
        for arm in &s.arms {
            for p in &arm.patterns {
                self.check_pattern(p, st);
            }
            if let Some(g) = &arm.guard {
                self.check_coerce(g, self.tt.bool_);
            }
            let t = self.check_block(&arm.body, exp);
            branches.push((t, arm.body.span));
        }
        if branches.is_empty() {
            return self.tt.void;
        }
        let _ = e;
        self.join(&branches, exp, "the arms of `switch`")
    }

    pub(crate) fn select_expr(&mut self, arms: &'p [SelectArm], exp: Exp) -> Ty {
        let mut branches = Vec::new();
        for arm in arms {
            if let Some(op) = &arm.op {
                let t = self.check_expr(op, Exp::Any);
                if let Some(b) = &arm.bind {
                    if let Some(d) = self.declared(b) {
                        self.local_tys.insert(d, t);
                    }
                }
            }
            let t = self.check_block(&arm.body, exp);
            branches.push((t, arm.body.span));
        }
        if branches.is_empty() {
            return self.tt.void;
        }
        self.join(&branches, exp, "the arms of `select`")
    }

    // ---- Loops -------------------------------------------------------------------------------------

    pub(crate) fn looped(
        &mut self,
        _label: Option<String>,
        is_loop: bool,
        f: impl FnOnce(&mut Self),
    ) -> LoopCtx {
        let label = self.pending_label.take();
        self.loops.push(LoopCtx {
            label,
            kind: if is_loop {
                LoopKind::Loop
            } else {
                LoopKind::Plain
            },
            break_ty: None,
            has_break: false,
        });
        f(self);
        self.loops.pop().unwrap_or(LoopCtx {
            label: None,
            kind: LoopKind::Plain,
            break_ty: None,
            has_break: false,
        })
    }

    pub(crate) fn loop_expr(&mut self, _label: Option<String>, body: &'p Block, exp: Exp) -> Ty {
        let label = self.pending_label.take();
        self.loops.push(LoopCtx {
            label,
            kind: LoopKind::Loop,
            break_ty: exp.ty(),
            has_break: false,
        });
        self.check_block(body, Exp::Discard);
        let ctx = self.loops.pop();
        match ctx {
            Some(c) if c.has_break => c.break_ty.unwrap_or(self.tt.void),
            _ => self.tt.never,
        }
    }

    pub(crate) fn for_expr(&mut self, f: &'p ForExpr) -> Ty {
        match &f.head {
            ForHead::Of { pats, iter } => {
                let it = self.check_expr(iter, Exp::Any);
                let tys = self.iteration_types(it, pats.len(), false, iter.span);
                for (p, t) in pats.iter().zip(tys.iter()) {
                    self.check_pattern(p, *t);
                }
            }
            ForHead::In { pat, iter } => {
                let it = self.check_expr(iter, Exp::Any);
                let tys = self.iteration_types(it, 1, true, iter.span);
                self.check_pattern(pat, tys[0]);
            }
            ForHead::C { init, cond, step } => {
                if let Some(s) = init {
                    match &s.kind {
                        StmtKind::Decl(d) => self.local_decl(d),
                        StmtKind::Expr(e) => {
                            self.check_expr(e, Exp::Discard);
                        }
                        StmtKind::Item(_) => {}
                    }
                }
                if let Some(c) = cond {
                    self.check_coerce(c, self.tt.bool_);
                }
                if let Some(s) = step {
                    self.check_expr(s, Exp::Discard);
                }
            }
        }
        let label = self.pending_label.take();
        self.loops.push(LoopCtx {
            label,
            kind: LoopKind::Plain,
            break_ty: None,
            has_break: false,
        });
        self.check_block(&f.body, Exp::Discard);
        self.loops.pop();
        self.tt.void
    }

    pub(crate) fn labeled(&mut self, label: &Ident, body: &'p Expr, exp: Exp) -> Ty {
        let name = self.text(label).to_string();
        match &body.kind {
            ExprKind::Block(b) => {
                let v = match exp.ty() {
                    Some(t) => t,
                    None => self.var(body.span),
                };
                self.loops.push(LoopCtx {
                    label: Some(name),
                    kind: LoopKind::Block,
                    break_ty: Some(v),
                    has_break: false,
                });
                let t = self.check_block(b, exp);
                self.record(body, t);
                let ctx = self.loops.pop();
                if ctx.as_ref().is_some_and(|c| c.has_break) {
                    if !matches!(self.tt.kind_of(t), TyKind::Never) {
                        self.coerce_at(t, v, b.span, "the end of this block");
                    }
                    v
                } else {
                    t
                }
            }
            _ => {
                self.pending_label = Some(name);
                let t = self.check_expr(body, exp);
                self.pending_label = None;
                t
            }
        }
    }

    pub(crate) fn break_expr(
        &mut self,
        e: &'p Expr,
        label: Option<&Ident>,
        value: Option<&'p Expr>,
    ) -> Ty {
        let want = label.map(|l| self.text(l).to_string());
        // Find what is being left.
        let idx = match &want {
            Some(n) => self
                .loops
                .iter()
                .rposition(|l| l.label.as_deref() == Some(n.as_str())),
            None => self.loops.iter().rposition(|l| l.kind != LoopKind::Block),
        };
        let Some(idx) = idx else {
            if let Some(v) = value {
                self.check_expr(v, Exp::Any);
            }
            return self.tt.never;
        };
        let kind = self.loops[idx].kind;
        let expected = self.loops[idx].break_ty;
        let ty = match value {
            Some(v) => {
                if kind == LoopKind::Plain {
                    self.err(e.span, "only `loop` and labeled blocks can end with a value; `while` and `for` cannot");
                    self.check_expr(v, Exp::Any);
                    self.tt.void
                } else {
                    let ex = expected.map_or(Exp::Any, Exp::Is);
                    self.check_expr(v, ex)
                }
            }
            None => self.tt.void,
        };
        let ctx = &mut self.loops[idx];
        ctx.has_break = true;
        match ctx.break_ty {
            None => ctx.break_ty = Some(ty),
            Some(prev) => {
                if value.is_some() || kind == LoopKind::Loop {
                    let sp = value.map_or(e.span, |v| v.span);
                    self.coerce_at(ty, prev, sp, "this `break`");
                }
            }
        }
        self.tt.never
    }

    // ---- Functions: return, become, closures -----------------------------------------------------------------

    pub(crate) fn return_expr(&mut self, e: &'p Expr, v: &'p Option<Box<Expr>>) -> Ty {
        let ret = self.fn_ctx.last().map(|c| c.ret).unwrap_or(self.tt.void);
        match v {
            Some(x) => {
                self.check_coerce(x, ret);
            }
            None => {
                if !matches!(
                    self.tt.kind_of(ret),
                    TyKind::Void | TyKind::Unknown | TyKind::Var(_)
                ) {
                    let s = self.show(ret);
                    self.err(
                        e.span,
                        format!("this function returns `{s}`, so `return` needs a value"),
                    );
                } else if matches!(self.tt.kind_of(ret), TyKind::Var(_)) {
                    self.tt.unify(ret, self.tt.void);
                }
            }
        }
        self.tt.never
    }

    pub(crate) fn become_expr(&mut self, e: &'p Expr, call: &'p Expr) -> Ty {
        let ret = self.fn_ctx.last().map(|c| c.ret).unwrap_or(self.tt.void);
        let t = self.check_expr(call, Exp::Is(ret));
        self.coerce_at(t, ret, e.span, "the call after `become`");
        self.tt.never
    }

    pub(crate) fn closure(&mut self, e: &'p Expr, c: &'p Closure, exp: Exp) -> Ty {
        let want = exp.ty().and_then(|t| match self.kind_of(t) {
            TyKind::Fn(ps, r) => Some((ps, r)),
            _ => None,
        });
        let mut params: Vec<ParamTy> = Vec::new();
        if c.params.is_empty() {
            // `~> it * 2`: one implicit parameter when the context wants one.
            if let Some((ps, _)) = &want {
                if ps.len() == 1 {
                    let it_ty = ps[0].ty;
                    params.push(ps[0]);
                    if let Some(&it) = self.res.closure_it.get(&(self.file, e.span.start)) {
                        self.local_tys.insert(it, it_ty);
                    }
                }
            }
        } else {
            for (i, p) in c.params.iter().enumerate() {
                let ty = match &p.ty {
                    Some(t) => self.lower_type(t),
                    None => match &want {
                        Some((ps, _)) if ps.len() == c.params.len() => ps[i].ty,
                        _ => self.var(e.span),
                    },
                };
                let mode = want
                    .as_ref()
                    .and_then(|(ps, _)| ps.get(i))
                    .and_then(|p| p.mode);
                params.push(ParamTy { mode, ty });
                self.check_pattern(&p.pattern, ty);
            }
            if let Some((ps, _)) = &want {
                if ps.len() != c.params.len() {
                    self.err(
                        e.span,
                        format!(
                            "this closure takes {} parameter{}, but {} {} expected",
                            c.params.len(),
                            if c.params.len() == 1 { "" } else { "s" },
                            ps.len(),
                            if ps.len() == 1 { "is" } else { "are" }
                        ),
                    );
                }
            }
        }
        let ret = match &c.ret {
            Some(t) => self.lower_type(t),
            None => match &want {
                Some((_, r)) => *r,
                None => self.var(e.span),
            },
        };
        let saved_loops = std::mem::take(&mut self.loops);
        self.fn_ctx.push(FnCtx { ret });
        let body_exp = if matches!(self.tt.kind_of(ret), TyKind::Void) {
            Exp::Discard
        } else {
            Exp::Is(ret)
        };
        let bt = self.check_expr(&c.body, body_exp);
        if let Exp::Is(_) = body_exp {
            self.coerce_at(bt, ret, c.body.span, "the closure's value");
        }
        self.fn_ctx.pop();
        self.loops = saved_loops;
        self.tt.mk(TyKind::Fn(params, ret))
    }
}
