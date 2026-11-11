//! `select { case v = rx.recv() { ... } case tx.send(x) { ... } default { ... } }`.
//!
//! Tasks here run one after another, so `select` looks at its arms in order and takes the first that is
//! ready: a `recv` whose channel has a value (or has no senders left), or a `send` whose channel has room
//! (or has no receiver, so the send reports `closed`), or `cancelled()` once the task was asked to stop. When none is
//! ready it takes `default`; with no `default` it lets one waiting task run and looks again; when no task
//! is left it takes the `timeout` arm, or stops (a deadlock).

use super::*;

enum ArmKind<'a> {
    Recv(&'a Expr),
    Send(&'a Expr),
    Cancelled,
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
                "recv" | "tryRecv" | "try_recv" => Some(ArmKind::Recv(base)),
                "send" => Some(ArmKind::Send(base)),
                _ => None,
            },
            ExprKind::Name(id) if self.src_text(id.span) == "timeout" => Some(ArmKind::Timeout),
            ExprKind::Name(id) if self.src_text(id.span) == "cancelled" => Some(ArmKind::Cancelled),
            _ => None,
        }
    }

    /// The moment a `timeout(d)` arm gives up, on the monotonic clock (0 when there is no such arm). `d` is evaluated
    /// once, when the `select` starts; it is a `duration` (or any one-word value counting nanoseconds).
    fn select_deadline(&mut self, arms: &'a [SelectArm]) -> Result<Value, Res> {
        let none = self.konst(IrTy::I64, Const::Int(0), Span::new(0, 0));
        for arm in arms {
            if !matches!(self.arm_kind(arm), Some(ArmKind::Timeout)) {
                continue;
            }
            let Some(ExprKind::Call { args, .. }) = arm.op.as_ref().map(|o| &o.kind) else {
                continue;
            };
            let [arg] = args.as_slice() else {
                return Err(self.unsupported(arm.span, "`timeout` with other than one argument"));
            };
            let Some(t) = self.ty_of(&arg.value) else {
                return Err(self.unsupported(arm.span, "a `timeout` of unknown type"));
            };
            let slot = self.slot_for(t);
            if matches!(self.into(&arg.value, slot, t), Res::Never) {
                return Err(Res::Never);
            }
            let v = match self.repr(t) {
                Repr::Scalar(s) => match self.emit(Op::Load(s, slot), Some(s), arm.span) {
                    Some(v) => v,
                    None => return Err(Res::Never),
                },
                _ => slot,
            };
            return match self.call_core_g("rtTimeoutAt", &[t], vec![v]) {
                Some(d) => Ok(d),
                None => Err(Res::Never),
            };
        }
        Ok(none)
    }

    pub(crate) fn select_expr(&mut self, e: &'a Expr, arms: &'a [SelectArm]) -> Res {
        let sp = e.span;
        let void = self.types.void;
        let done = self.new_block(&[]);
        let deadline = match self.select_deadline(arms) {
            Ok(d) => d,
            Err(r) => return r,
        };
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
                ArmKind::Cancelled => {
                    let Some(ready) = self.call_core("rtCancelled", vec![]) else {
                        return Res::Never;
                    };
                    let go = self.new_block(&[]);
                    let next = self.new_block(&[]);
                    self.branch(ready, go, next);
                    ready_arms.push((arm, go));
                    self.start(next);
                }
                ArmKind::Recv(base) | ArmKind::Send(base) => {
                    let Some((p, _)) = self.scrutinee(base) else {
                        return Res::Never;
                    };
                    let ch = self.load_at(p, 0, IrTy::Ptr);
                    let check = if matches!(kind, ArmKind::Send(_)) {
                        "rtChanRoom"
                    } else {
                        "rtChanReady"
                    };
                    let Some(ready) = self.call_core(check, vec![ch]) else {
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
                    let Some(ran) = self.call_core("rtSelectWait", vec![deadline]) else {
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
