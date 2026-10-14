//! Lowering the checked program to the IR (`docs/design/ir.md`, step 2).
//!
//! This first version handles functions over numbers and booleans: parameters, `const` and `var`,
//! arithmetic with overflow checks, comparisons, `&&` / `||`, `if`, `while`, `loop`, `for` over a
//! range, `break`, `continue`, `return`, conversions with `to`, and calls between such functions.
//! `const` values and parameters are SSA values; a `var` lives in a stack slot (a later pass can turn
//! slots into values). Anything else is reported as "not lowered yet" and its function is replaced by a
//! stop, so the module still verifies and runs.

use crate::ast::*;
use crate::ir::{
    BinOp, BlockId, CastKind, CmpOp, Const, Func, FuncId, IrTy, Module, Op, Target, Term, Value,
};
use crate::package::{Diagnostic, ModuleId, Program};
use crate::resolve::{DefId, DefKind, Resolved};
use crate::span::Span;
use crate::token::Punct;
use crate::typecheck::{literal_value, Typed};
use crate::types::{FloatKind, Ty, TyKind};
use std::collections::HashMap;

#[cfg(test)]
mod tests;

pub struct Lowered {
    pub module: Module,
    /// Functions that could not be lowered yet, and why.
    pub diagnostics: Vec<Diagnostic>,
}

struct Shared<'a> {
    program: &'a Program,
    res: &'a Resolved,
    typed: &'a Typed,
    /// The functions that can be called, with their IR signatures.
    funcs: HashMap<DefId, FuncId>,
}

pub fn lower(program: &Program, res: &Resolved, typed: &Typed) -> Lowered {
    let mut module = Module::default();
    let mut diagnostics = Vec::new();
    let mut shared = Shared {
        program,
        res,
        typed,
        funcs: HashMap::new(),
    };
    // Pass 1: every function with a simple signature gets a place in the module.
    let mut todo: Vec<(ModuleId, &FnDecl, DefId)> = Vec::new();
    let mut names: HashMap<String, u32> = HashMap::new();
    for m in &program.modules {
        if program.package(m.package).name == "core" {
            continue;
        }
        for s in &m.file.stmts {
            let StmtKind::Item(it) = &s.kind else {
                continue;
            };
            let ItemKind::Fn(f) = &it.kind else {
                continue;
            };
            let Some(&def) = res.item_defs.get(&(m.id, it.span.start)) else {
                continue;
            };
            if f.kind != FnKind::Fn || matches!(f.body, FnBody::None) {
                continue;
            }
            let Some((params, ret)) = signature(&shared, m.id, f, def) else {
                continue;
            };
            let base = res.def(def).name.clone();
            let n = names.entry(base.clone()).or_insert(0);
            let name = if *n == 0 {
                base.clone()
            } else {
                format!("{base}#{n}")
            };
            *n += 1;
            let mut func = Func::new(&name, &params, ret);
            let e = func.entry();
            func.terminate(e, Term::Stop("not lowered yet".to_string()));
            let id = module.add(func);
            shared.funcs.insert(def, id);
            todo.push((m.id, f, def));
        }
    }
    // Pass 2: the bodies.
    for (file, f, def) in todo {
        let id = shared.funcs[&def];
        let template = module.funcs[id.0 as usize].clone();
        let mut fl = FnLower::new(
            &shared,
            file,
            template.name.clone(),
            &template.params,
            template.ret,
        );
        match fl.lower_fn(f) {
            Ok(func) => module.funcs[id.0 as usize] = func,
            Err((span, msg)) => diagnostics.push(Diagnostic {
                module: file,
                span,
                message: format!("not lowered yet: {msg} (in `{}`)", template.name),
                warning: true,
            }),
        }
    }
    Lowered {
        module,
        diagnostics,
    }
}

/// The IR type of a source type, when it is one the first lowering handles.
fn ir_ty(typed: &Typed, t: Ty) -> Option<IrTy> {
    let t = typed.types.shallow(t);
    Some(match typed.types.kind(t) {
        TyKind::Int(k) => IrTy::Int(k.bits() as u8),
        TyKind::Float(FloatKind::F32) => IrTy::F32,
        TyKind::Float(FloatKind::F64) => IrTy::F64,
        TyKind::Bool => IrTy::Bool,
        TyKind::Char => IrTy::Int(32),
        TyKind::Void => IrTy::Void,
        _ => return None,
    })
}

fn is_signed(typed: &Typed, t: Ty) -> bool {
    let t = typed.types.shallow(t);
    matches!(typed.types.kind(t), TyKind::Int(k) if k.signed())
}

fn signature(shared: &Shared, file: ModuleId, f: &FnDecl, def: DefId) -> Option<(Vec<IrTy>, IrTy)> {
    if !f.generics.params.is_empty() {
        return None;
    }
    let mut params = Vec::new();
    for p in &f.params {
        if p.is_self
            || p.variadic
            || p.default.is_some()
            || !matches!(p.mode, None | Some(Mode::Let))
        {
            return None;
        }
        let d = shared.res.decls.get(&(file, p.name.span.start))?;
        let t = shared.typed.local_tys.get(d)?;
        let ty = ir_ty(shared.typed, *t)?;
        if ty == IrTy::Void {
            return None;
        }
        params.push(ty);
    }
    let ret = shared.typed.fn_rets.get(&def)?;
    let ret = ir_ty(shared.typed, *ret)?;
    Some((params, ret))
}

#[derive(Clone, Copy)]
enum Local {
    Val(Value),
    /// A stack slot and the type stored in it.
    Slot(Value, IrTy),
}

/// What lowering an expression gave.
#[derive(Clone, Copy)]
enum Res {
    Val(Value),
    /// Nothing to use (the expression has no value).
    Unit,
    /// Control never gets past this expression.
    Never,
}

struct LoopCtx {
    label: Option<String>,
    cont: BlockId,
    exit: BlockId,
    /// Type of the value `break` carries, for `loop`.
    break_ty: Option<IrTy>,
}

type Fail = (Span, String);

struct FnLower<'a> {
    sh: &'a Shared<'a>,
    file: ModuleId,
    f: Func,
    cur: Option<BlockId>,
    locals: HashMap<DefId, Local>,
    loops: Vec<LoopCtx>,
    stops: HashMap<String, BlockId>,
    pending_label: Option<String>,
    fail: Option<Fail>,
}

impl<'a> FnLower<'a> {
    fn new(sh: &'a Shared<'a>, file: ModuleId, name: String, params: &[IrTy], ret: IrTy) -> Self {
        let f = Func::new(&name, params, ret);
        let cur = Some(f.entry());
        FnLower {
            sh,
            file,
            f,
            cur,
            locals: HashMap::new(),
            loops: Vec::new(),
            stops: HashMap::new(),
            pending_label: None,
            fail: None,
        }
    }

    // ---- Plumbing -------------------------------------------------------------------------------

    fn unsupported(&mut self, span: Span, what: &str) -> Res {
        if self.fail.is_none() {
            self.fail = Some((span, what.to_string()));
        }
        Res::Never
    }

    fn ty_of(&self, e: &Expr) -> Option<Ty> {
        self.sh
            .typed
            .tys
            .get(&(self.file, e.span.start, e.span.end))
            .copied()
    }

    fn ir_of(&self, e: &Expr) -> Option<IrTy> {
        ir_ty(self.sh.typed, self.ty_of(e)?)
    }

    fn signed_of(&self, e: &Expr) -> bool {
        self.ty_of(e).is_some_and(|t| is_signed(self.sh.typed, t))
    }

    fn emit(&mut self, op: Op, ty: Option<IrTy>, span: Span) -> Option<Value> {
        let b = self.cur?;
        self.f.emit_at(b, op, ty, span)
    }

    fn konst(&mut self, ty: IrTy, c: Const, span: Span) -> Value {
        self.emit(Op::Const(ty, c), Some(ty), span)
            .unwrap_or(Value(0))
    }

    fn terminate(&mut self, t: Term) {
        if let Some(b) = self.cur.take() {
            self.f.terminate(b, t);
        }
    }

    fn jump(&mut self, to: BlockId, args: Vec<Value>) {
        self.terminate(Term::Jump(Target { block: to, args }));
    }

    fn branch(&mut self, c: Value, yes: BlockId, no: BlockId) {
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

    fn start(&mut self, b: BlockId) {
        self.cur = Some(b);
    }

    /// The block that stops the program with this message (one per message and function).
    fn stop_block(&mut self, msg: &str) -> BlockId {
        if let Some(b) = self.stops.get(msg) {
            return *b;
        }
        let b = self.f.new_block(&[]);
        self.f.terminate(b, Term::Stop(msg.to_string()));
        self.stops.insert(msg.to_string(), b);
        b
    }

    /// Stops with `msg` when `cond` holds; otherwise goes on in a new block.
    fn stop_if(&mut self, cond: Value, msg: &str) {
        let stop = self.stop_block(msg);
        let go = self.f.new_block(&[]);
        self.branch(cond, stop, go);
        self.start(go);
    }

    /// A stack slot in the entry block, so a loop does not grow the stack.
    fn slot(&mut self, ty: IrTy) -> Value {
        let entry = self.f.entry();
        self.f
            .emit(
                entry,
                Op::Slot(ty.size(), ty.size().max(1)),
                Some(IrTy::Ptr),
            )
            .unwrap_or(Value(0))
    }

    // ---- Functions --------------------------------------------------------------------------------

    fn lower_fn(&mut self, f: &FnDecl) -> Result<Func, Fail> {
        for (p, v) in f
            .params
            .iter()
            .zip(self.f.block_params(self.f.entry()).to_vec())
        {
            if let Some(&d) = self.sh.res.decls.get(&(self.file, p.name.span.start)) {
                self.locals.insert(d, Local::Val(v));
            }
        }
        let ret = self.f.ret;
        let r = match &f.body {
            FnBody::Block(b) => self.block(b),
            FnBody::Expr(e) => self.expr(e),
            FnBody::None => Res::Unit,
        };
        match (r, ret) {
            (Res::Never, _) => {}
            (Res::Val(v), t) if t != IrTy::Void => self.terminate(Term::Return(Some(v))),
            (_, IrTy::Void) => self.terminate(Term::Return(None)),
            _ => {
                self.unsupported(f.name.span, "a function body without a value");
            }
        }
        if let Some(e) = self.fail.take() {
            return Err(e);
        }
        // The slots were added to the entry block after its first instructions; the stop blocks are
        // already complete.
        Ok(std::mem::replace(
            &mut self.f,
            Func::new("", &[], IrTy::Void),
        ))
    }

    // ---- Blocks and statements ----------------------------------------------------------------------

    fn block(&mut self, b: &Block) -> Res {
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Decl(d) => self.decl(d),
                StmtKind::Expr(e) => {
                    self.expr(e);
                }
                StmtKind::Item(_) => {
                    self.unsupported(s.span, "a nested item");
                }
            }
        }
        match &b.tail {
            Some(t) => self.expr(t),
            None if self.cur.is_none() => Res::Never,
            None => Res::Unit,
        }
    }

    fn decl(&mut self, d: &Decl) {
        let Some(init) = &d.init else {
            self.unsupported(d.pattern.span, "a declaration without a value");
            return;
        };
        let r = self.expr(init);
        let Res::Val(v) = r else {
            if matches!(r, Res::Unit) {
                self.unsupported(d.pattern.span, "a value-less declaration");
            }
            return;
        };
        match &d.pattern.kind {
            PatternKind::Wildcard => {}
            PatternKind::Binding { name, .. } => {
                let Some(&def) = self.sh.res.decls.get(&(self.file, name.span.start)) else {
                    return;
                };
                match d.kind {
                    DeclKind::Const => {
                        self.locals.insert(def, Local::Val(v));
                    }
                    DeclKind::Var => {
                        let ty = self.f.ty(v);
                        let slot = self.slot(ty);
                        self.emit(Op::Store(slot, v), None, d.pattern.span);
                        self.locals.insert(def, Local::Slot(slot, ty));
                    }
                }
            }
            _ => {
                self.unsupported(d.pattern.span, "a pattern in a declaration");
            }
        }
    }

    // ---- Expressions ----------------------------------------------------------------------------------

    fn expr(&mut self, e: &Expr) -> Res {
        if self.cur.is_none() || self.fail.is_some() {
            return Res::Never;
        }
        match &e.kind {
            ExprKind::Int => {
                let Some(ty) = self.ir_of(e) else {
                    return self.unsupported(e.span, "an integer of this type");
                };
                let text = &self.sh.program.module(self.file).src
                    [e.span.start as usize..e.span.end as usize];
                let n = literal_value(text).unwrap_or(0);
                if ty.is_float() {
                    return Res::Val(self.konst(ty, Const::Float(n as f64), e.span));
                }
                Res::Val(self.konst(ty, Const::Int(n as i128), e.span))
            }
            ExprKind::Float => {
                let Some(ty) = self.ir_of(e) else {
                    return self.unsupported(e.span, "a float of this type");
                };
                let text = &self.sh.program.module(self.file).src
                    [e.span.start as usize..e.span.end as usize];
                let x: f64 = text.replace('_', "").parse().unwrap_or(0.0);
                Res::Val(self.konst(ty, Const::Float(x), e.span))
            }
            ExprKind::Bool(b) => Res::Val(self.konst(IrTy::Bool, Const::Bool(*b), e.span)),
            ExprKind::Paren(i) => self.expr(i),
            ExprKind::Name(id) => self.name(e, id),
            ExprKind::Block(b) => self.block(b),
            ExprKind::Unary { op, operand } => self.unary(e, *op, operand),
            ExprKind::Binary { op, lhs, rhs } => self.binary(e, *op, lhs, rhs),
            ExprKind::Assign { op, lhs, rhs } => self.assign(e, *op, lhs, rhs),
            ExprKind::IncDec { op, operand } => self.incdec(e, *op, operand),
            ExprKind::If(i) => self.if_expr(e, i, false),
            ExprKind::Unless(i) => self.if_expr(e, i, true),
            ExprKind::Ternary { cond, then, else_ } => self.ternary(e, cond, then, else_),
            ExprKind::While { cond, body } => self.while_loop(cond, body),
            ExprKind::Loop(body) => self.loop_expr(e, body),
            ExprKind::For(f) => self.for_loop(e, f),
            ExprKind::Labeled { label, body } => {
                let name = self.sh.program.module(self.file).src
                    [label.span.start as usize..label.span.end as usize]
                    .to_string();
                self.pending_label = Some(name);
                let r = self.expr(body);
                self.pending_label = None;
                r
            }
            ExprKind::Break { label, value } => {
                self.break_expr(e, label.as_ref(), value.as_deref())
            }
            ExprKind::Continue { label } => self.continue_expr(e, label.as_ref()),
            ExprKind::Return(v) => self.return_expr(e, v.as_deref()),
            ExprKind::Call { callee, args, mark } => self.call(e, callee, args, *mark),
            ExprKind::Convert {
                value,
                checked: false,
                ..
            } => self.convert(e, value),
            _ => self.unsupported(e.span, "this kind of expression"),
        }
    }

    fn name(&mut self, e: &Expr, id: &Ident) -> Res {
        let Some(def) = self.sh.res.use_at(self.file, id.span.start) else {
            return self.unsupported(e.span, "an unresolved name");
        };
        match self.locals.get(&def).copied() {
            Some(Local::Val(v)) => Res::Val(v),
            Some(Local::Slot(p, ty)) => match self.emit(Op::Load(ty, p), Some(ty), e.span) {
                Some(v) => Res::Val(v),
                None => Res::Never,
            },
            None => self.unsupported(e.span, "a global or a function used as a value"),
        }
    }

    fn value(&mut self, e: &Expr) -> Option<Value> {
        match self.expr(e) {
            Res::Val(v) => Some(v),
            Res::Unit => {
                self.unsupported(e.span, "a value-less operand");
                None
            }
            Res::Never => None,
        }
    }

    fn unary(&mut self, e: &Expr, op: Punct, operand: &Expr) -> Res {
        let Some(ty) = self.ir_of(e) else {
            return self.unsupported(e.span, "this operand type");
        };
        match op {
            Punct::Minus => {
                if matches!(operand.kind, ExprKind::Int) {
                    let text = &self.sh.program.module(self.file).src
                        [operand.span.start as usize..operand.span.end as usize];
                    let n = literal_value(text).unwrap_or(0) as i128;
                    return Res::Val(if ty.is_float() {
                        self.konst(ty, Const::Float(-(n as f64)), e.span)
                    } else {
                        self.konst(ty, Const::Int(-n), e.span)
                    });
                }
                let Some(v) = self.value(operand) else {
                    return Res::Never;
                };
                if ty.is_float() {
                    return self.wrap(Op::Neg(v), ty, e.span);
                }
                let zero = self.konst(ty, Const::Int(0), e.span);
                self.wrap(Op::Checked(BinOp::Sub, true, zero, v), ty, e.span)
            }
            Punct::Bang | Punct::Tilde => {
                let Some(v) = self.value(operand) else {
                    return Res::Never;
                };
                self.wrap(Op::Not(v), ty, e.span)
            }
            _ => self.unsupported(e.span, "this operator"),
        }
    }

    fn wrap(&mut self, op: Op, ty: IrTy, span: Span) -> Res {
        match self.emit(op, Some(ty), span) {
            Some(v) => Res::Val(v),
            None => Res::Never,
        }
    }

    fn binary(&mut self, e: &Expr, op: Punct, lhs: &Expr, rhs: &Expr) -> Res {
        match op {
            Punct::AndAnd | Punct::OrOr => {
                return self.short_circuit(e, op == Punct::AndAnd, lhs, rhs)
            }
            Punct::EqEq | Punct::NotEq | Punct::Lt | Punct::Gt | Punct::LtEq | Punct::GtEq => {
                return self.compare(e, op, lhs, rhs)
            }
            _ => {}
        }
        let Some(ty) = self.ir_of(e) else {
            return self.unsupported(e.span, "this operand type");
        };
        let signed = self.signed_of(e);
        let Some(a) = self.value(lhs) else {
            return Res::Never;
        };
        let Some(b) = self.value(rhs) else {
            return Res::Never;
        };
        self.arith(e.span, op, ty, signed, a, b)
    }

    /// `a op b` on two values of type `ty`.
    fn arith(&mut self, span: Span, op: Punct, ty: IrTy, signed: bool, a: Value, b: Value) -> Res {
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
                let is_zero = self.emit(Op::Cmp(CmpOp::Eq, b, zero), Some(IrTy::Bool), span);
                if let Some(z) = is_zero {
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
                // The amount is checked against the width (a negative amount looks huge).
                let limit = self.konst(ty, Const::Int(bits), span);
                let too_far = self.emit(Op::Cmp(CmpOp::UGe, b, limit), Some(IrTy::Bool), span);
                if let Some(t) = too_far {
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

    fn compare(&mut self, e: &Expr, op: Punct, lhs: &Expr, rhs: &Expr) -> Res {
        let signed = self.signed_of(lhs);
        let ok = self.ir_of(lhs).is_some_and(|t| t != IrTy::Void);
        if !ok {
            return self.unsupported(e.span, "comparing this type");
        }
        let Some(a) = self.value(lhs) else {
            return Res::Never;
        };
        let Some(b) = self.value(rhs) else {
            return Res::Never;
        };
        let float = self.f.ty(a).is_float();
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

    fn short_circuit(&mut self, e: &Expr, and: bool, lhs: &Expr, rhs: &Expr) -> Res {
        let Some(a) = self.value(lhs) else {
            return Res::Never;
        };
        let eval_rhs = self.f.new_block(&[]);
        let join = self.f.new_block(&[IrTy::Bool]);
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
        let b = self.value(rhs);
        if let Some(b) = b {
            self.jump(join, vec![b]);
        }
        self.start(join);
        let _ = e;
        Res::Val(self.f.block_params(join)[0])
    }

    fn assign(&mut self, e: &Expr, op: Punct, lhs: &Expr, rhs: &Expr) -> Res {
        let ExprKind::Name(id) = &lhs.kind else {
            return self.unsupported(e.span, "assigning to this kind of place");
        };
        let Some(def) = self.sh.res.use_at(self.file, id.span.start) else {
            return self.unsupported(e.span, "an unresolved name");
        };
        let Some(Local::Slot(slot, ty)) = self.locals.get(&def).copied() else {
            return self.unsupported(e.span, "assigning to this variable");
        };
        let Some(rv) = self.value(rhs) else {
            return Res::Never;
        };
        let new = if op == Punct::Eq {
            rv
        } else {
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
            let signed = self.signed_of(lhs);
            let Some(cur) = self.emit(Op::Load(ty, slot), Some(ty), e.span) else {
                return Res::Never;
            };
            let rv = if base == Punct::Shl || base == Punct::Shr {
                self.resize(rv, ty, self.signed_of(rhs), e.span)
            } else {
                rv
            };
            match self.arith(e.span, base, ty, signed, cur, rv) {
                Res::Val(v) => v,
                other => return other,
            }
        };
        self.emit(Op::Store(slot, new), None, e.span);
        Res::Unit
    }

    /// Changes an integer to the width of `ty`.
    fn resize(&mut self, v: Value, ty: IrTy, signed: bool, span: Span) -> Value {
        let from = self.f.ty(v);
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

    fn incdec(&mut self, e: &Expr, op: Punct, operand: &Expr) -> Res {
        let ExprKind::Name(id) = &operand.kind else {
            return self.unsupported(e.span, "`++` on this kind of place");
        };
        let Some(def) = self.sh.res.use_at(self.file, id.span.start) else {
            return self.unsupported(e.span, "an unresolved name");
        };
        let Some(Local::Slot(slot, ty)) = self.locals.get(&def).copied() else {
            return self.unsupported(e.span, "`++` on this variable");
        };
        let signed = self.signed_of(operand);
        let Some(cur) = self.emit(Op::Load(ty, slot), Some(ty), e.span) else {
            return Res::Never;
        };
        let one = self.konst(ty, Const::Int(1), e.span);
        let o = if op == Punct::PlusPlus {
            Punct::Plus
        } else {
            Punct::Minus
        };
        if let Res::Val(v) = self.arith(e.span, o, ty, signed, cur, one) {
            self.emit(Op::Store(slot, v), None, e.span);
        }
        Res::Unit
    }

    fn convert(&mut self, e: &Expr, value: &Expr) -> Res {
        let (Some(from), Some(to)) = (self.ty_of(value), self.ty_of(e)) else {
            return self.unsupported(e.span, "this conversion");
        };
        let (Some(fi), Some(ti)) = (ir_ty(self.sh.typed, from), ir_ty(self.sh.typed, to)) else {
            return self.unsupported(e.span, "this conversion");
        };
        let signed = is_signed(self.sh.typed, from);
        let Some(v) = self.value(value) else {
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
            _ => return self.unsupported(e.span, "this conversion"),
        };
        self.wrap(Op::Cast(kind, v, ti), ti, e.span)
    }

    // ---- Control flow -------------------------------------------------------------------------------------

    /// The IR type of an expression's value: `Some(Void)` for none, `None` when it cannot be lowered.
    fn result_ty(&self, e: &Expr) -> Option<IrTy> {
        match self.ty_of(e).map(|t| {
            self.sh
                .typed
                .types
                .kind_of(self.sh.typed.types.shallow(t))
                .clone()
        }) {
            Some(TyKind::Never) => Some(IrTy::Void),
            Some(_) => self.ir_of(e),
            None => None,
        }
    }

    fn if_expr(&mut self, e: &Expr, i: &IfExpr, unless: bool) -> Res {
        let Some(c) = self.value(&i.cond) else {
            return Res::Never;
        };
        let Some(rt) = self.result_ty(e) else {
            return self.unsupported(e.span, "an `if` with a value of this type");
        };
        let then_b = self.f.new_block(&[]);
        let else_b = self.f.new_block(&[]);
        if unless {
            self.branch(c, else_b, then_b);
        } else {
            self.branch(c, then_b, else_b);
        }
        let join_params: Vec<IrTy> = if rt == IrTy::Void { vec![] } else { vec![rt] };
        let join = self.f.new_block(&join_params);
        let mut reached = false;
        self.start(then_b);
        let r = self.block(&i.then);
        reached |= self.finish_arm(r, join, rt);
        self.start(else_b);
        let r = match &i.else_ {
            Some(el) => self.expr(el),
            None => Res::Unit,
        };
        reached |= self.finish_arm(r, join, rt);
        if !reached {
            self.cur = None;
            return Res::Never;
        }
        self.start(join);
        if rt == IrTy::Void {
            Res::Unit
        } else {
            Res::Val(self.f.block_params(join)[0])
        }
    }

    /// Jumps to the join block with the arm's value. Gives whether control reaches the join.
    fn finish_arm(&mut self, r: Res, join: BlockId, rt: IrTy) -> bool {
        match r {
            Res::Never => false,
            Res::Unit if rt == IrTy::Void => {
                self.jump(join, vec![]);
                true
            }
            Res::Val(v) if rt != IrTy::Void => {
                self.jump(join, vec![v]);
                true
            }
            Res::Val(_) => {
                self.jump(join, vec![]);
                true
            }
            Res::Unit => {
                self.unsupported(Span::new(0, 0), "an arm without a value");
                false
            }
        }
    }

    fn ternary(&mut self, e: &Expr, cond: &Expr, then: &Expr, else_: &Expr) -> Res {
        let Some(c) = self.value(cond) else {
            return Res::Never;
        };
        let Some(rt) = self.result_ty(e) else {
            return self.unsupported(e.span, "a conditional with a value of this type");
        };
        let then_b = self.f.new_block(&[]);
        let else_b = self.f.new_block(&[]);
        self.branch(c, then_b, else_b);
        let params: Vec<IrTy> = if rt == IrTy::Void { vec![] } else { vec![rt] };
        let join = self.f.new_block(&params);
        let mut reached = false;
        self.start(then_b);
        let r = self.expr(then);
        reached |= self.finish_arm(r, join, rt);
        self.start(else_b);
        let r = self.expr(else_);
        reached |= self.finish_arm(r, join, rt);
        if !reached {
            self.cur = None;
            return Res::Never;
        }
        self.start(join);
        if rt == IrTy::Void {
            Res::Unit
        } else {
            Res::Val(self.f.block_params(join)[0])
        }
    }

    fn take_label(&mut self) -> Option<String> {
        self.pending_label.take()
    }

    fn while_loop(&mut self, cond: &Expr, body: &Block) -> Res {
        let label = self.take_label();
        let head = self.f.new_block(&[]);
        let body_b = self.f.new_block(&[]);
        let exit = self.f.new_block(&[]);
        self.jump(head, vec![]);
        self.start(head);
        if let Some(c) = self.value(cond) {
            self.branch(c, body_b, exit);
        }
        self.loops.push(LoopCtx {
            label,
            cont: head,
            exit,
            break_ty: None,
        });
        self.start(body_b);
        let r = self.block(body);
        if !matches!(r, Res::Never) {
            self.jump(head, vec![]);
        }
        self.loops.pop();
        self.start(exit);
        Res::Unit
    }

    fn loop_expr(&mut self, e: &Expr, body: &Block) -> Res {
        let label = self.take_label();
        let rt = self.result_ty(e);
        let Some(rt) = rt else {
            return self.unsupported(e.span, "a `loop` with a value of this type");
        };
        let params: Vec<IrTy> = if rt == IrTy::Void { vec![] } else { vec![rt] };
        let head = self.f.new_block(&[]);
        let exit = self.f.new_block(&params);
        self.jump(head, vec![]);
        self.start(head);
        let break_ty = if rt == IrTy::Void { None } else { Some(rt) };
        self.loops.push(LoopCtx {
            label,
            cont: head,
            exit,
            break_ty,
        });
        let r = self.block(body);
        if !matches!(r, Res::Never) {
            self.jump(head, vec![]);
        }
        self.loops.pop();
        // The exit is reached only through `break`; if nothing broke, the loop never ends.
        let reached = self
            .f
            .blocks
            .iter()
            .any(|b| matches!(&b.term, Some(Term::Jump(t)) if t.block == exit)
                || matches!(&b.term, Some(Term::Branch(_, a, c)) if a.block == exit || c.block == exit));
        if !reached {
            self.cur = None;
            return Res::Never;
        }
        self.start(exit);
        if rt == IrTy::Void {
            Res::Unit
        } else {
            Res::Val(self.f.block_params(exit)[0])
        }
    }

    fn for_loop(&mut self, e: &Expr, f: &ForExpr) -> Res {
        let label = self.take_label();
        let ForHead::Of { pats, iter } = &f.head else {
            return self.unsupported(e.span, "this kind of `for`");
        };
        let ExprKind::Range {
            start: Some(start),
            end: Some(end),
            inclusive,
            step: None,
        } = &iter.kind
        else {
            return self.unsupported(iter.span, "looping over anything but a range");
        };
        let [pat] = pats.as_slice() else {
            return self.unsupported(e.span, "this `for` pattern");
        };
        let (var, wild) = match &pat.kind {
            PatternKind::Binding { name, mode: None } => (
                self.sh
                    .res
                    .decls
                    .get(&(self.file, name.span.start))
                    .copied(),
                false,
            ),
            PatternKind::Wildcard => (None, true),
            _ => return self.unsupported(pat.span, "this `for` pattern"),
        };
        let _ = wild;
        let Some(ty) = self.ir_of(start) else {
            return self.unsupported(iter.span, "a range of this type");
        };
        let signed = self.signed_of(start);
        let Some(a) = self.value(start) else {
            return Res::Never;
        };
        let Some(b) = self.value(end) else {
            return Res::Never;
        };
        let head = self.f.new_block(&[ty]);
        let body_b = self.f.new_block(&[]);
        let step = self.f.new_block(&[]);
        let exit = self.f.new_block(&[]);
        self.jump(head, vec![a]);
        self.start(head);
        let i = self.f.block_params(head)[0];
        let cmp = match (*inclusive, signed) {
            (false, true) => CmpOp::Lt,
            (false, false) => CmpOp::ULt,
            (true, true) => CmpOp::Le,
            (true, false) => CmpOp::ULe,
        };
        if let Some(c) = self.emit(Op::Cmp(cmp, i, b), Some(IrTy::Bool), iter.span) {
            self.branch(c, body_b, exit);
        }
        if let Some(d) = var {
            self.locals.insert(d, Local::Val(i));
        }
        self.loops.push(LoopCtx {
            label,
            cont: step,
            exit,
            break_ty: None,
        });
        self.start(body_b);
        let r = self.block(&f.body);
        if !matches!(r, Res::Never) {
            self.jump(step, vec![]);
        }
        self.loops.pop();
        self.start(step);
        let next = if *inclusive {
            // After the last value, stop without adding (the add could overflow).
            let at_end = self.emit(Op::Cmp(CmpOp::Eq, i, b), Some(IrTy::Bool), iter.span);
            let more = self.f.new_block(&[]);
            if let Some(c) = at_end {
                self.branch(c, exit, more);
            }
            self.start(more);
            let one = self.konst(ty, Const::Int(1), iter.span);
            self.emit(Op::Bin(BinOp::Add, i, one), Some(ty), iter.span)
        } else {
            let one = self.konst(ty, Const::Int(1), iter.span);
            self.emit(Op::Bin(BinOp::Add, i, one), Some(ty), iter.span)
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
                let name = &self.sh.program.module(self.file).src
                    [l.span.start as usize..l.span.end as usize];
                self.loops
                    .iter()
                    .rposition(|c| c.label.as_deref() == Some(name))
            }
            None => self.loops.len().checked_sub(1),
        }
    }

    fn break_expr(&mut self, e: &Expr, label: Option<&Ident>, value: Option<&Expr>) -> Res {
        let Some(i) = self.find_loop(label) else {
            return self.unsupported(e.span, "`break` outside a loop");
        };
        let exit = self.loops[i].exit;
        let want = self.loops[i].break_ty;
        let args = match (value, want) {
            (Some(v), Some(_)) => match self.value(v) {
                Some(x) => vec![x],
                None => return Res::Never,
            },
            (None, None) => vec![],
            _ => return self.unsupported(e.span, "`break` with a mismatched value"),
        };
        self.jump(exit, args);
        Res::Never
    }

    fn continue_expr(&mut self, e: &Expr, label: Option<&Ident>) -> Res {
        let Some(i) = self.find_loop(label) else {
            return self.unsupported(e.span, "`continue` outside a loop");
        };
        let to = self.loops[i].cont;
        self.jump(to, vec![]);
        Res::Never
    }

    fn return_expr(&mut self, e: &Expr, v: Option<&Expr>) -> Res {
        match (v, self.f.ret) {
            (Some(x), t) if t != IrTy::Void => {
                if let Some(val) = self.value(x) {
                    self.terminate(Term::Return(Some(val)));
                }
            }
            (None, IrTy::Void) => self.terminate(Term::Return(None)),
            _ => return self.unsupported(e.span, "this `return`"),
        }
        Res::Never
    }

    fn call(&mut self, e: &Expr, callee: &Expr, args: &[Arg], mark: CallMark) -> Res {
        let ExprKind::Name(id) = &callee.kind else {
            return self.unsupported(e.span, "this kind of call");
        };
        let Some(def) = self.sh.res.use_at(self.file, id.span.start) else {
            return self.unsupported(e.span, "an unresolved name");
        };
        if self.sh.res.def(def).kind != DefKind::Fn || mark != CallMark::None {
            return self.unsupported(e.span, "this kind of call");
        }
        let Some(&fid) = self.sh.funcs.get(&def) else {
            let n = self.sh.res.def(def).name.clone();
            return self.unsupported(e.span, &format!("a call to `{n}`, which is not lowered"));
        };
        let mut vals = Vec::new();
        for a in args {
            if a.label.is_some() || a.spread || a.mode.is_some() {
                return self.unsupported(a.value.span, "this kind of argument");
            }
            match self.value(&a.value) {
                Some(v) => vals.push(v),
                None => return Res::Never,
            }
        }
        let ret = self.result_ty(e);
        match ret {
            Some(IrTy::Void) => {
                self.emit(Op::Call(fid, vals), None, e.span);
                if self.ty_of(e).is_some_and(|t| {
                    matches!(
                        self.sh.typed.types.kind_of(self.sh.typed.types.shallow(t)),
                        TyKind::Never
                    )
                }) {
                    self.terminate(Term::Unreachable);
                    return Res::Never;
                }
                Res::Unit
            }
            Some(t) => self.wrap(Op::Call(fid, vals), t, e.span),
            None => self.unsupported(e.span, "a call with a result of this type"),
        }
    }
}
