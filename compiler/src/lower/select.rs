//! `select { case v = rx.recv() { ... } case tx.send(x) { ... } default { ... } }`.
//!
//! Tasks here run one after another, so `select` looks at its arms in order and takes the first that is
//! ready: a `recv` whose channel has a value (or has no senders left), or any `send`. When none is
//! ready it takes `default`; with no `default` it lets one waiting task run and looks again; when no task
//! is left it takes the `timeout` arm, or stops (a deadlock).

use super::*;

enum ArmKind<'a> {
    Recv(&'a Expr),
    Send,
    Timeout,
    Default,
}

impl<'a> Lowerer<'a> {
    fn arm_kind(&self, arm: &'a SelectArm) -> Option<ArmKind<'a>> {
        let Some(op) = &arm.op else {
            return Some(ArmKind::Default);
        };
        let ExprKind::Call { callee, .. } = &op.kind else {
            return None;
        };
        match &callee.kind {
            ExprKind::Field { base, name, .. } => match self.src_text(name.span) {
                "recv" | "tryRecv" => Some(ArmKind::Recv(base)),
                "send" => Some(ArmKind::Send),
                _ => None,
            },
            ExprKind::Name(id) if self.src_text(id.span) == "timeout" => Some(ArmKind::Timeout),
            _ => None,
        }
    }

    pub(crate) fn select_expr(&mut self, e: &'a Expr, arms: &'a [SelectArm]) -> Res {
        let sp = e.span;
        let void = self.types.void;
        let done = self.new_block(&[]);
        let head = self.new_block(&[]);
        self.jump(head, vec![]);
        self.start(head);
        let mut default_arm = None;
        let mut timeout_arm = None;
        let mut ready_arms: Vec<(&'a SelectArm, BlockId)> = Vec::new();
        for arm in arms {
            let Some(kind) = self.arm_kind(arm) else {
                return self.unsupported(arm.span, "this `select` arm");
            };
            match kind {
                ArmKind::Default => default_arm = Some(arm),
                ArmKind::Timeout => timeout_arm = Some(arm),
                ArmKind::Send => {
                    let go = self.new_block(&[]);
                    self.jump(go, vec![]);
                    ready_arms.push((arm, go));
                    // Nothing after a send arm can be reached.
                    break;
                }
                ArmKind::Recv(base) => {
                    let Some((p, _)) = self.scrutinee(base) else {
                        return Res::Never;
                    };
                    let ch = self.load_at(p, 0, IrTy::Ptr);
                    let Some(ready) = self.call_core("rt_chan_ready", vec![ch]) else {
                        return Res::Never;
                    };
                    let go = self.new_block(&[]);
                    let next = self.new_block(&[]);
                    self.branch(ready, go, next);
                    ready_arms.push((arm, go));
                    self.start(next);
                }
            }
        }
        if self.f.cur.is_some() {
            // Nothing is ready.
            match (default_arm, timeout_arm) {
                (Some(d), _) => {
                    self.select_body(d, done, void);
                }
                _ => {
                    let Some(ran) = self.call_core("rt_run_one_pending", vec![]) else {
                        return Res::Never;
                    };
                    let retry = self.new_block(&[]);
                    let idle = self.new_block(&[]);
                    self.branch(ran, retry, idle);
                    self.start(retry);
                    self.jump(head, vec![]);
                    self.start(idle);
                    match timeout_arm {
                        Some(t) => self.select_body(t, done, void),
                        None => self.terminate(Term::Stop(
                            "deadlock: every `select` arm is waiting".to_string(),
                        )),
                    }
                }
            }
        }
        for (arm, go) in ready_arms {
            self.start(go);
            self.push_scope();
            if let Some(op) = &arm.op {
                let r = self.select_op(arm, op, sp);
                if matches!(r, Res::Never) {
                    self.f.scopes.pop();
                    continue;
                }
            }
            self.select_body_in_scope(arm, done, void);
        }
        self.start(done);
        Res::Unit
    }

    /// Runs the operation of an arm (the receive or send) and binds its result.
    fn select_op(&mut self, arm: &'a SelectArm, op: &'a Expr, sp: Span) -> Res {
        match &arm.bind {
            Some(name) => {
                let Some(t) = self.ty_of(op) else {
                    return self.unsupported(sp, "a `select` arm of unknown type");
                };
                match self.repr(t) {
                    Repr::Zst => self.expr(op),
                    _ => {
                        let slot = self.slot_for(t);
                        let r = self.into(op, slot, t);
                        if matches!(r, Res::Never) {
                            return r;
                        }
                        self.own(slot, t, false);
                        if let Some(def) =
                            self.res.decls.get(&(self.f.file, name.span.start)).copied()
                        {
                            self.f.locals.insert(
                                def,
                                Local::Slot {
                                    ptr: slot,
                                    ty: t,
                                    flag: None,
                                },
                            );
                        }
                        Res::Unit
                    }
                }
            }
            None => self.expr(op),
        }
    }

    fn select_body(&mut self, arm: &'a SelectArm, done: BlockId, void: Ty) {
        self.push_scope();
        self.select_body_in_scope(arm, done, void);
    }

    fn select_body_in_scope(&mut self, arm: &'a SelectArm, done: BlockId, void: Ty) {
        let r = self.block(&arm.body, None, void);
        if matches!(r, Res::Never) {
            self.f.scopes.pop();
        } else {
            self.pop_scope();
            self.jump(done, vec![]);
        }
    }
}
