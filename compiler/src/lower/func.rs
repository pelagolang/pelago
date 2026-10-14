//! The state of the function being lowered, and the small operations on it: blocks, memory access,
//! scopes with their drops, and early exits.

use super::*;

/// Where a name's value is.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Local {
    /// A scalar kept in an SSA value (a parameter, a `const`).
    Val(Value),
    /// Storage at an address. `flag` is a one-byte slot that is 1 while the variable still owns its
    /// value (cleared when it is moved), for values that need dropping.
    Slot {
        ptr: Value,
        ty: Ty,
        flag: Option<Value>,
    },
}

/// What lowering an expression gave.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Res {
    Val(Value),
    /// Nothing to use (the expression has no value).
    Unit,
    /// Control never gets past this expression.
    Never,
}

pub(crate) struct LoopCtx {
    pub label: Option<String>,
    pub cont: BlockId,
    pub exit: BlockId,
    /// Type of the value `break` carries, for `loop`.
    pub break_ty: Option<IrTy>,
    /// Where a Mem-typed `break` value is written.
    pub break_dst: Option<Value>,
    /// How many scopes were open outside the loop.
    pub scope_depth: usize,
}

pub(crate) type Fail = (Span, String);

pub(crate) enum ScopeItem<'a> {
    /// A value to destroy when the scope ends.
    Drop {
        ptr: Value,
        ty: Ty,
        flag: Option<Value>,
    },
    /// Code to run when the scope ends.
    Defer(&'a Expr),
    /// A library function to call when the scope ends (`rt_run_pending` at the end of `main`).
    CoreCall(&'static str),
}

pub(crate) struct FnState<'a> {
    /// Names of the `scope s { ... }` blocks being lowered.
    /// A closure written right at a call that only reads its function parameter: it cannot outlive the call, so its environment may live on the stack.
    pub stack_env: Option<(u32, u32)>,
    /// The function being lowered, and the block a tail call to it jumps to (when it can).
    pub cur_def: Option<DefId>,
    pub tail_head: Option<BlockId>,
    pub func: Func,
    pub file: ModuleId,
    pub cur: Option<BlockId>,
    /// The type arguments of this instance.
    pub subst: HashMap<DefId, Ty>,
    pub locals: HashMap<DefId, Local>,
    pub scopes: Vec<Vec<ScopeItem<'a>>>,
    pub loops: Vec<LoopCtx>,
    pub stops: HashMap<String, BlockId>,
    pub pending_label: Option<String>,
    /// The (concrete) return type and where a memory result is written.
    pub ret_ty: Ty,
    pub sret: Option<Value>,
    pub fail: Option<Fail>,
}

impl<'a> FnState<'a> {
    pub fn empty(void: Ty) -> FnState<'a> {
        FnState {
            stack_env: None,
            cur_def: None,
            tail_head: None,
            func: Func::new("", &[], IrTy::Void),
            file: ModuleId(0),
            cur: None,
            subst: HashMap::new(),
            locals: HashMap::new(),
            scopes: Vec::new(),
            loops: Vec::new(),
            stops: HashMap::new(),
            pending_label: None,
            ret_ty: void,
            sret: None,
            fail: None,
        }
    }
}

impl<'a> Lowerer<'a> {
    // ---- Failure ------------------------------------------------------------------------------------

    pub(crate) fn unsupported(&mut self, span: Span, what: &str) -> Res {
        if self.f.fail.is_none() {
            self.f.fail = Some((span, what.to_string()));
        }
        Res::Never
    }

    pub(crate) fn failed(&self) -> bool {
        self.f.fail.is_some()
    }

    // ---- Emitting -----------------------------------------------------------------------------------

    pub(crate) fn emit(&mut self, op: Op, ty: Option<IrTy>, span: Span) -> Option<Value> {
        let b = self.f.cur?;
        self.f.func.emit_at(b, op, ty, span)
    }

    pub(crate) fn konst(&mut self, ty: IrTy, c: Const, span: Span) -> Value {
        self.emit(Op::Const(ty, c), Some(ty), span)
            .unwrap_or(Value(0))
    }

    pub(crate) fn i64c(&mut self, n: i64) -> Value {
        self.konst(IrTy::I64, Const::Int(n as i128), Span::new(0, 0))
    }

    pub(crate) fn terminate(&mut self, t: Term) {
        if let Some(b) = self.f.cur.take() {
            self.f.func.terminate(b, t);
        }
    }

    pub(crate) fn jump(&mut self, to: BlockId, args: Vec<Value>) {
        self.terminate(Term::Jump(Target { block: to, args }));
    }

    pub(crate) fn branch(&mut self, c: Value, yes: BlockId, no: BlockId) {
        self.terminate(Term::Branch(
            c,
            Target {
                block: yes,
                args: vec![],
            },
            Target {
                block: no,
                args: vec![],
            },
        ));
    }

    pub(crate) fn start(&mut self, b: BlockId) {
        self.f.cur = Some(b);
    }

    pub(crate) fn new_block(&mut self, params: &[IrTy]) -> BlockId {
        self.f.func.new_block(params)
    }

    pub(crate) fn vty(&self, v: Value) -> IrTy {
        self.f.func.ty(v)
    }

    /// The block that stops the program with this message (one per message and function).
    pub(crate) fn stop_block(&mut self, msg: &str) -> BlockId {
        if let Some(b) = self.f.stops.get(msg) {
            return *b;
        }
        let b = self.new_block(&[]);
        self.f.func.terminate(b, Term::Stop(msg.to_string()));
        self.f.stops.insert(msg.to_string(), b);
        b
    }

    /// Stops with `msg` when `cond` holds; otherwise goes on in a new block.
    pub(crate) fn stop_if(&mut self, cond: Value, msg: &str) {
        let stop = self.stop_block(msg);
        let go = self.new_block(&[]);
        self.branch(cond, stop, go);
        self.start(go);
    }

    // ---- Memory ---------------------------------------------------------------------------------------

    /// Stack space in the entry block, so a loop does not grow the stack.
    pub(crate) fn slot_bytes(&mut self, size: u32, align: u32) -> Value {
        let entry = self.f.func.entry();
        self.f
            .func
            .emit(entry, Op::Slot(size, align.max(1)), Some(IrTy::Ptr))
            .unwrap_or(Value(0))
    }

    pub(crate) fn slot_for(&mut self, t: Ty) -> Value {
        let l = self.layout(t);
        self.slot_bytes(l.size.max(1), l.align)
    }

    pub(crate) fn addr(&mut self, base: Value, off: u32) -> Value {
        if off == 0 {
            return base;
        }
        let n = self.i64c(off as i64);
        self.emit(Op::PtrAdd(base, n), Some(IrTy::Ptr), Span::new(0, 0))
            .unwrap_or(base)
    }

    pub(crate) fn load_at(&mut self, base: Value, off: u32, ty: IrTy) -> Value {
        let p = self.addr(base, off);
        self.emit(Op::Load(ty, p), Some(ty), Span::new(0, 0))
            .unwrap_or(Value(0))
    }

    pub(crate) fn store_at(&mut self, base: Value, off: u32, v: Value) {
        let p = self.addr(base, off);
        self.emit(Op::Store(p, v), None, Span::new(0, 0));
    }

    /// Copies `size` bytes (a constant) between two addresses.
    pub(crate) fn copy_bytes(&mut self, dst: Value, src: Value, size: u32) {
        if size == 0 {
            return;
        }
        let n = self.i64c(size as i64);
        self.emit(Op::MemCopy(dst, src, n), None, Span::new(0, 0));
    }

    /// Copies a value of a memory type.
    pub(crate) fn copy_mem(&mut self, dst: Value, src: Value, t: Ty) {
        let l = self.layout(t);
        self.copy_bytes(dst, src, l.size);
    }

    /// Stores a scalar value of type `t` at `dst`, or copies memory from a source address.
    pub(crate) fn store_scalar_ty(&mut self, dst: Value, v: Value) {
        self.emit(Op::Store(dst, v), None, Span::new(0, 0));
    }

    // ---- Scopes, drops and flags ---------------------------------------------------------------------------

    pub(crate) fn push_scope(&mut self) {
        self.f.scopes.push(Vec::new());
    }

    /// Ends a scope, running its drops and deferred code when control reaches the end.
    pub(crate) fn pop_scope(&mut self) {
        let Some(items) = self.f.scopes.pop() else {
            return;
        };
        if self.f.cur.is_some() {
            self.run_scope_items(&items);
        }
    }

    pub(crate) fn run_scope_items(&mut self, items: &[ScopeItem<'a>]) {
        for item in items.iter().rev() {
            match item {
                ScopeItem::Drop { ptr, ty, flag } => self.emit_drop(*ptr, *ty, *flag),
                ScopeItem::Defer(e) => {
                    let e: &'a Expr = e;
                    self.expr(e);
                }
                ScopeItem::CoreCall(name) => {
                    self.call_core(name, vec![]);
                }
            }
        }
    }

    /// Runs the drops of every scope above `depth` without ending them (for `return`, `break`, `?`).
    pub(crate) fn exit_scopes(&mut self, depth: usize) {
        let n = self.f.scopes.len();
        for i in (depth..n).rev() {
            if self.f.cur.is_none() {
                break;
            }
            let items: Vec<ScopeItem<'a>> = self.f.scopes[i]
                .iter()
                .map(|it| match it {
                    ScopeItem::Drop { ptr, ty, flag } => ScopeItem::Drop {
                        ptr: *ptr,
                        ty: *ty,
                        flag: *flag,
                    },
                    ScopeItem::Defer(e) => ScopeItem::Defer(e),
                    ScopeItem::CoreCall(n) => ScopeItem::CoreCall(n),
                })
                .collect();
            self.run_scope_items(&items);
        }
    }

    /// Registers a value to be dropped at the end of the current scope. Gives its flag: a byte that
    /// is 1 while the value is alive (set here, cleared when it is moved out or after it is dropped).
    /// The flag starts at 0 in the entry block, so code that never reached this point drops nothing.
    pub(crate) fn own(&mut self, ptr: Value, ty: Ty, _with_flag: bool) -> Option<Value> {
        if !self.needs_drop(ty) {
            return None;
        }
        let f = self.slot_bytes(1, 1);
        let entry = self.f.func.entry();
        let no = self.f.func.emit(
            entry,
            Op::Const(IrTy::Bool, Const::Bool(false)),
            Some(IrTy::Bool),
        );
        if let Some(no) = no {
            self.f.func.emit(entry, Op::Store(f, no), None);
        }
        let one = self.konst(IrTy::Bool, Const::Bool(true), Span::new(0, 0));
        self.emit(Op::Store(f, one), None, Span::new(0, 0));
        if let Some(s) = self.f.scopes.last_mut() {
            s.push(ScopeItem::Drop {
                ptr,
                ty,
                flag: Some(f),
            });
        }
        Some(f)
    }

    pub(crate) fn clear_flag(&mut self, flag: Value) {
        let zero = self.konst(IrTy::Bool, Const::Bool(false), Span::new(0, 0));
        self.emit(Op::Store(flag, zero), None, Span::new(0, 0));
    }

    pub(crate) fn emit_drop(&mut self, ptr: Value, ty: Ty, flag: Option<Value>) {
        let Some(glue) = self.glue_fn(GlueKind::Drop, ty) else {
            return;
        };
        match flag {
            None => {
                self.emit(Op::Call(glue, vec![ptr]), None, Span::new(0, 0));
            }
            Some(fl) => {
                let live = self.emit(Op::Load(IrTy::Bool, fl), Some(IrTy::Bool), Span::new(0, 0));
                let Some(live) = live else { return };
                let do_it = self.new_block(&[]);
                let after = self.new_block(&[]);
                self.branch(live, do_it, after);
                self.start(do_it);
                self.emit(Op::Call(glue, vec![ptr]), None, Span::new(0, 0));
                self.clear_flag(fl);
                self.jump(after, vec![]);
                self.start(after);
            }
        }
    }
}
