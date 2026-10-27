//! From the IR to x86-64 MIR (design in `docs/design/mir.md`, opcodes in [`super::x86_mir`]).
//!
//! Every IR value that is not a constant or a folded address becomes a virtual register. All integers,
//! booleans and pointers are 64 bits wide in a register and zero-extended from their type, as in
//! the older generator (`gen.rs`, rule 5 of `a11-work-items.md`): an operation on a narrow type cuts its
//! result back with `MASK`, and a signed use widens its operands with `EXT` first.
//!
//! Instructions are built two-address: `d = a; d op= b`. The allocator joins `d` with `a` when `a`
//! dies there. For a commutative operation the operand that dies in the instruction (its only use)
//! is the one copied, so that join happens.
//!
//! The selector gives up (returns `None`) on a function that has anything it cannot do yet (floats,
//! vectors, `CheckedIf`, fill loops); the caller then uses the older generator for that function.

use super::mir::*;
use super::regs::{folds, fused_cmp, small_copy, Base, Fold};
use super::x86_mir::{self as x, call_clobbers, cc_num, op, Cc, INT_ARGS, SYM_FUNC};
use super::x86_mir::{RAX, RCX, RDX, RSI};
use crate::ir::Target as JumpTarget;
use crate::ir::{BinOp, CastKind, CmpOp, Const, Func, IrTy, Op, Term, Value};
use std::collections::HashMap;

fn bits(t: IrTy) -> u32 {
    match t {
        IrTy::Int(b) => b as u32,
        IrTy::Bool => 8,
        _ => 64,
    }
}

fn size(t: IrTy) -> u8 {
    match t {
        IrTy::Int(b) => (b / 8).max(1),
        IrTy::Bool => 1,
        _ => 8,
    }
}

/// A canonical (zero-extended) constant of type `t` widened to 64 bits with its sign or with zeros.
fn widen(n: i64, t: IrTy, signed: bool) -> i64 {
    let w = bits(t);
    if signed && w < 64 && matches!(t, IrTy::Int(_)) {
        let sh = 64 - w;
        (n << sh) >> sh
    } else {
        n
    }
}

/// The integer or boolean constants of a function, by value.
fn const_values(f: &Func) -> Vec<Option<i64>> {
    let mut out = vec![None; f.values.len()];
    for b in &f.blocks {
        for &i in &b.insts {
            if let (Op::Const(t, c), Some(r)) = (&f.insts[i].op, f.insts[i].result) {
                out[r.0 as usize] = match (c, t) {
                    (Const::Int(n), t) => {
                        let w = bits(*t);
                        Some(if w >= 64 {
                            *n as i64
                        } else {
                            (*n as u128 & ((1u128 << w) - 1)) as i64
                        })
                    }
                    (Const::Bool(b), _) => Some(i64::from(*b)),
                    _ => None,
                };
            }
        }
    }
    out
}

/// Where an IR value is.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Loc {
    Imm(i64),
    Frame(FrameId, i32),
    Reg(Reg),
    /// A pointer nobody computes: its users name the address (`mem_of`).
    Folded,
}

/// The condition of a branch or a select.
#[derive(Clone)]
enum Cond {
    Cmp(Cc, Reg, Operand),
    Test(Reg),
}

struct Sel<'m> {
    f: &'m Func,
    mf: MFunc,
    imms: Vec<Option<i64>>,
    uses: Vec<u32>,
    folds: Vec<Fold>,
    vreg: Vec<Option<Reg>>,
    slot_frame: HashMap<usize, FrameId>,
    cur: u32,
    out_area: Option<FrameId>,
    out_size: u32,
    /// Comparisons whose only readers are selects in their own block: each select compares again.
    deferred: HashMap<Value, (CmpOp, Value, Value)>,
}

fn class_of(t: IrTy) -> Class {
    match t {
        IrTy::F32 | IrTy::F64 => Class::Float,
        IrTy::V2I64 => Class::Vec,
        _ => Class::Int,
    }
}

/// Can the selector do this function?
fn supported(f: &Func) -> bool {
    if f.ret.is_float() || f.ret == IrTy::V2I64 {
        return false;
    }
    if f.values.iter().any(|t| class_of(*t) != Class::Int) {
        return false;
    }
    f.blocks
        .iter()
        .flat_map(|b| &b.insts)
        .all(|&i| match &f.insts[i].op {
            Op::Fma(..) | Op::FUn(..) | Op::VUn(..) | Op::VBin(..) | Op::CheckedIf(..) => false,
            Op::Cast(k, ..) => !matches!(
                k,
                CastKind::IntToFloat | CastKind::FloatToInt | CastKind::FloatResize
            ),
            Op::Const(t, _) => class_of(*t) == Class::Int,
            _ => true,
        })
}

/// Makes the MIR of function `fi`, or `None` when this selector cannot (yet).
pub fn select(f: &Func) -> Option<MFunc> {
    if !supported(f) {
        return None;
    }
    if !crate::ir::opt::fill_loops(f, 512).is_empty() || !crate::ir::opt::filled_slots(f).is_empty()
    {
        return None;
    }
    let imms = const_values(f);
    let uses = f.use_counts();
    let fl = folds(f, &uses, &imms);
    let mut s = Sel {
        f,
        mf: MFunc {
            name: f.name.clone(),
            ..MFunc::default()
        },
        imms,
        uses,
        folds: fl,
        vreg: vec![None; f.values.len()],
        slot_frame: HashMap::new(),
        cur: 0,
        out_area: None,
        out_size: 0,
        deferred: HashMap::new(),
    };
    s.function();
    Some(s.mf)
}

impl<'m> Sel<'m> {
    fn emit(&mut self, i: MInst) {
        let b = self.cur as usize;
        self.mf.blocks[b].insts.push(i);
    }

    fn new_vreg(&mut self) -> Reg {
        self.mf.new_vreg(Class::Int)
    }

    /// The register of an IR value that is made by an instruction or is a block parameter.
    fn reg_of(&mut self, v: Value) -> Reg {
        if let Some(r) = self.vreg[v.0 as usize] {
            return r;
        }
        let r = self.new_vreg();
        self.vreg[v.0 as usize] = Some(r);
        r
    }

    fn loc(&mut self, v: Value) -> Loc {
        let i = v.0 as usize;
        if let Some(n) = self.imms[i] {
            return Loc::Imm(n);
        }
        match self.folds[i] {
            Fold::No => Loc::Reg(self.reg_of(v)),
            Fold::Addr {
                base: Base::Slot(s),
                index: None,
                disp,
            } => Loc::Frame(self.slot_frame[&s], disp),
            _ => Loc::Folded,
        }
    }

    /// The value as a register to read, made from a constant or a frame address when it is not in one.
    fn vr(&mut self, v: Value) -> Reg {
        match self.loc(v) {
            Loc::Reg(r) => r,
            Loc::Imm(n) => {
                let t = self.new_vreg();
                self.emit(MInst::copy(t, Operand::Imm(n)));
                t
            }
            Loc::Frame(id, d) => {
                let t = self.new_vreg();
                self.emit(MInst::copy(t, Operand::Frame(id, d)));
                t
            }
            Loc::Folded => panic!("value {} is a folded address read as a value", v.0),
        }
    }

    /// The value as a source operand for copies: a register, an immediate or a frame address.
    fn src(&mut self, v: Value) -> Operand {
        match self.loc(v) {
            Loc::Reg(r) => Operand::use_(r),
            Loc::Imm(n) => Operand::Imm(n),
            Loc::Frame(id, d) => Operand::Frame(id, d),
            Loc::Folded => panic!("value {} is a folded address read as a value", v.0),
        }
    }

    /// The constant a value is, if it fits an immediate once widened (signed or not).
    fn imm32(&self, v: Value, signed: bool) -> Option<i32> {
        let n = self.imms[v.0 as usize]?;
        i32::try_from(widen(n, self.f.ty(v), signed)).ok()
    }

    /// An immediate operand when the value is a small constant, else a register.
    fn rhs(&mut self, v: Value, signed: bool) -> Operand {
        match self.imm32(v, signed) {
            Some(n) => Operand::Imm(n as i64),
            None => Operand::use_(self.ext(v, signed)),
        }
    }

    /// The value widened to 64 bits with its sign or with zeros, in a register.
    fn ext(&mut self, v: Value, signed: bool) -> Reg {
        let t = self.f.ty(v);
        match self.loc(v) {
            Loc::Imm(n) => {
                let r = self.new_vreg();
                self.emit(MInst::copy(r, Operand::Imm(widen(n, t, signed))));
                r
            }
            Loc::Reg(r) if signed && size(t) < 8 => {
                let d = self.new_vreg();
                self.emit(MInst::new(
                    op::EXT,
                    vec![
                        Operand::def(d),
                        Operand::use_(r),
                        Operand::Imm(size(t) as i64),
                        Operand::Imm(1),
                    ],
                ));
                d
            }
            _ => self.vr(v),
        }
    }

    /// `phys = value widened`.
    fn copy_ext_to(&mut self, p: PReg, v: Value, signed: bool) {
        let t = self.f.ty(v);
        let s = match self.loc(v) {
            Loc::Imm(n) => Operand::Imm(widen(n, t, signed)),
            Loc::Reg(_) if signed && size(t) < 8 => Operand::use_(self.ext(v, true)),
            _ => self.src(v),
        };
        self.emit(MInst::copy(Reg::Phys(p), s));
    }

    fn mask(&mut self, d: Reg, w: u32) {
        if w < 64 {
            self.emit(MInst::new(
                op::MASK,
                vec![Operand::use_def(d), Operand::Imm(w as i64)],
            ));
        }
    }

    /// The memory operand for the address a value holds.
    fn mem_of(&mut self, p: Value) -> Operand {
        match self.folds[p.0 as usize] {
            Fold::Addr { base, index, disp } => {
                let base = match base {
                    Base::Slot(i) => MemBase::Frame(self.slot_frame[&i]),
                    Base::Value(v) => MemBase::Reg(self.vr(v)),
                };
                let index = index.map(|(iv, scale)| (self.vr(iv), scale));
                Operand::Mem { base, index, disp }
            }
            _ => Operand::Mem {
                base: MemBase::Reg(self.vr(p)),
                index: None,
                disp: 0,
            },
        }
    }

    /// A memory operand without an index (an indexed one is built into a register first).
    fn plain_mem(&mut self, p: Value) -> Operand {
        match self.mem_of(p) {
            Operand::Mem {
                index: Some(ix),
                base,
                disp,
            } => {
                let t = self.new_vreg();
                let m = Operand::Mem {
                    base,
                    index: Some(ix),
                    disp,
                };
                self.emit(MInst::new(op::LEA, vec![Operand::def(t), m]));
                Operand::Mem {
                    base: MemBase::Reg(t),
                    index: None,
                    disp: 0,
                }
            }
            m => m,
        }
    }

    // ---- Functions, blocks, edges ------------------------------------------------------------------

    fn function(&mut self) {
        let f = self.f;
        let nb = f.blocks.len();
        // Block 0 sets up the parameters; IR block `b` is MIR block `b + 1`.
        for _ in 0..=nb {
            self.mf.new_block();
        }
        for b in &f.blocks {
            for &i in &b.insts {
                if let Op::Slot(sz, al) = f.insts[i].op {
                    let id = self.mf.new_frame(sz, al.max(1), FrameKind::Slot);
                    self.slot_frame.insert(i, id);
                }
            }
        }
        // Parameters.
        self.cur = 0;
        let params: Vec<Value> = f.block_params(f.entry()).to_vec();
        let mut ints = 0;
        let mut stack = 0;
        let incoming = (params.len() > INT_ARGS.len()).then(|| {
            self.mf.new_frame(
                8 * (params.len() - INT_ARGS.len()) as u32,
                8,
                FrameKind::Incoming,
            )
        });
        let mut copies = Vec::new();
        for p in &params {
            if self.uses[p.0 as usize] == 0 {
                if ints < INT_ARGS.len() {
                    ints += 1;
                } else {
                    stack += 1;
                }
                continue;
            }
            let r = self.reg_of(*p);
            if ints < INT_ARGS.len() {
                copies.push((r, Operand::use_(Reg::Phys(INT_ARGS[ints]))));
                ints += 1;
            } else {
                let m = Operand::Mem {
                    base: MemBase::Frame(incoming.expect("stack parameters")),
                    index: None,
                    disp: 8 * stack,
                };
                self.emit(MInst::new(
                    op::LOAD,
                    vec![Operand::def(r), m, Operand::Imm(8)],
                ));
                stack += 1;
            }
        }
        self.copies(copies);
        // The blocks a jump back leads to start a loop.
        for (bi, b) in f.blocks.iter().enumerate() {
            for t in b.term.iter().flat_map(super::regs::successors) {
                if t <= bi {
                    self.mf.blocks[t + 1].loop_head = true;
                }
            }
        }
        let uses = self.uses.clone();
        for (bi, b) in f.blocks.iter().enumerate() {
            self.cur = bi as u32 + 1;
            // Comparisons that only feed selects in their own block.
            self.find_deferred(bi);
            let fused = fused_cmp(f, bi, &uses);
            for &i in &b.insts {
                if Some(i) == fused {
                    continue;
                }
                self.inst(i, bi);
            }
            let term = b.term.as_ref().expect("a block has a terminator");
            self.term(term, fused, bi);
        }
        if let Some(a) = self.out_area {
            self.mf.frame[a.0 as usize].size = self.out_size;
        }
    }

    fn find_deferred(&mut self, bi: usize) {
        let f = self.f;
        let b = &f.blocks[bi];
        for &i in &b.insts {
            let (Op::Cmp(op, x, y), Some(r)) = (&f.insts[i].op, f.insts[i].result) else {
                continue;
            };
            if f.ty(*x).is_float() {
                continue;
            }
            let selects = b
                .insts
                .iter()
                .filter(|&&j| matches!(&f.insts[j].op, Op::Select(c, ..) if *c == r))
                .count();
            // A select reads its condition once; `uses` counts every read.
            if selects > 0 && selects == self.uses[r.0 as usize] as usize {
                self.deferred.insert(r, (*op, *x, *y));
            }
        }
    }

    /// The copies of an edge: each block parameter from its argument (those nothing reads need none).
    fn edge_copies(&mut self, tg: &JumpTarget) -> Vec<(Reg, Operand)> {
        let f = self.f;
        let mut out = Vec::new();
        for (p, a) in f.block_params(tg.block).iter().zip(&tg.args) {
            if self.uses[p.0 as usize] == 0 {
                continue;
            }
            let d = self.reg_of(*p);
            let s = self.src(*a);
            if s == Operand::use_(d) {
                continue;
            }
            out.push((d, s));
        }
        out
    }

    fn copies(&mut self, c: Vec<(Reg, Operand)>) {
        match c.len() {
            0 => {}
            1 => self.emit(MInst::copy(c[0].0, c[0].1.clone())),
            _ => self.emit(MInst::parcopy(c)),
        }
    }

    /// The block to jump to for an edge: the target itself when it needs no copies, else a block at the
    /// end of the function that makes them and jumps on.
    fn stub(&mut self, c: Vec<(Reg, Operand)>, to: u32) -> u32 {
        if c.is_empty() {
            return to + 1;
        }
        let b = self.mf.new_block();
        let save = self.cur;
        self.cur = b;
        self.copies(c);
        self.emit(
            MInst::new(op::JMP, vec![Operand::Block(to + 1)])
                .branch()
                .nofall(),
        );
        self.cur = save;
        b
    }

    fn jcc(&self, cond: &Cond, inv: bool, to: u32) -> MInst {
        match cond.clone() {
            Cond::Cmp(cc, a, b) => {
                let cc = if inv { cc.invert() } else { cc };
                MInst::new(
                    op::CMPJCC,
                    vec![
                        Operand::Imm(cc_num(cc)),
                        Operand::use_(a),
                        b,
                        Operand::Block(to),
                    ],
                )
                .branch()
            }
            Cond::Test(r) => {
                let cc = if inv { Cc::E } else { Cc::Ne };
                MInst::new(
                    op::TESTJCC,
                    vec![
                        Operand::Imm(cc_num(cc)),
                        Operand::use_(r),
                        Operand::Block(to),
                    ],
                )
                .branch()
            }
        }
    }

    fn term(&mut self, t: &Term, fused: Option<usize>, bi: usize) {
        let f = self.f;
        let next = (bi + 1 < f.blocks.len()).then_some(bi + 1);
        match t {
            Term::Return(None) => self.emit(MInst::new(op::RET, vec![]).nofall()),
            Term::Return(Some(v)) => {
                let s = self.src(*v);
                self.emit(MInst::copy(Reg::Phys(RAX), s));
                self.emit(MInst::new(op::RET, vec![Operand::use_(Reg::Phys(RAX))]).nofall());
            }
            Term::Jump(tg) => {
                let c = self.edge_copies(tg);
                self.copies(c);
                if Some(tg.block.0 as usize) != next {
                    self.emit(
                        MInst::new(op::JMP, vec![Operand::Block(tg.block.0 + 1)])
                            .branch()
                            .nofall(),
                    );
                }
            }
            Term::Branch(c, t, e) => {
                let cond = match fused {
                    Some(i) => {
                        let Op::Cmp(o, x, y) = &f.insts[i].op else {
                            unreachable!()
                        };
                        self.cmp_cond(*o, *x, *y)
                    }
                    None => Cond::Test(self.vr(*c)),
                };
                let (mt, me) = (self.edge_copies(t), self.edge_copies(e));
                let (tb, eb) = (t.block.0, e.block.0);
                let is_next = |b: u32| Some(b as usize) == next;
                if mt.is_empty() && me.is_empty() {
                    if is_next(tb) {
                        let j = self.jcc(&cond, true, eb + 1);
                        self.emit(j);
                    } else {
                        let j = self.jcc(&cond, false, tb + 1);
                        self.emit(j);
                        if !is_next(eb) {
                            self.emit(
                                MInst::new(op::JMP, vec![Operand::Block(eb + 1)])
                                    .branch()
                                    .nofall(),
                            );
                        }
                    }
                } else if is_next(eb) {
                    // The false side's copies run in line; the true side's wait out of line.
                    let to = self.stub(mt, tb);
                    let j = self.jcc(&cond, false, to);
                    self.emit(j);
                    self.copies(me);
                } else if is_next(tb) {
                    let to = self.stub(me, eb);
                    let j = self.jcc(&cond, true, to);
                    self.emit(j);
                    self.copies(mt);
                } else {
                    let to = self.stub(mt, tb);
                    let j = self.jcc(&cond, false, to);
                    self.emit(j);
                    self.copies(me);
                    self.emit(
                        MInst::new(op::JMP, vec![Operand::Block(eb + 1)])
                            .branch()
                            .nofall(),
                    );
                }
            }
            Term::Stop(msg) => {
                self.mf.strings.push(msg.clone());
                let n = self.mf.strings.len() as i64 - 1;
                self.emit(MInst::new(op::STOP, vec![Operand::Imm(n)]).nofall());
            }
            Term::Unreachable => {
                self.emit(MInst::new(op::JMPSYM, vec![Operand::Sym(x::SYM_UNREACHABLE)]).nofall())
            }
        }
    }

    // ---- Comparisons -------------------------------------------------------------------------------

    /// The condition code and operands for an integer, boolean or pointer comparison.
    fn cmp_cond(&mut self, o: CmpOp, a: Value, b: Value) -> Cond {
        let signed = matches!(self.f.ty(a), IrTy::Int(_))
            && matches!(o, CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge);
        // `cmp imm, r` does not exist: swap the sides when only the left one is a constant.
        let (a, b, o) = if self.imms[a.0 as usize].is_some() && self.imms[b.0 as usize].is_none() {
            let o = match o {
                CmpOp::Lt => CmpOp::Gt,
                CmpOp::Gt => CmpOp::Lt,
                CmpOp::Le => CmpOp::Ge,
                CmpOp::Ge => CmpOp::Le,
                CmpOp::ULt => CmpOp::UGt,
                CmpOp::UGt => CmpOp::ULt,
                CmpOp::ULe => CmpOp::UGe,
                CmpOp::UGe => CmpOp::ULe,
                o => o,
            };
            (b, a, o)
        } else {
            (a, b, o)
        };
        let ra = self.ext(a, signed);
        let rb = self.rhs(b, signed);
        let cc = match o {
            CmpOp::Eq => Cc::E,
            CmpOp::Ne => Cc::Ne,
            CmpOp::Lt if signed => Cc::L,
            CmpOp::Le if signed => Cc::Le,
            CmpOp::Gt if signed => Cc::G,
            CmpOp::Ge if signed => Cc::Ge,
            CmpOp::Lt | CmpOp::ULt => Cc::B,
            CmpOp::Le | CmpOp::ULe => Cc::Be,
            CmpOp::Gt | CmpOp::UGt => Cc::A,
            _ => Cc::Ae,
        };
        Cond::Cmp(cc, ra, rb)
    }

    // ---- Instructions ------------------------------------------------------------------------------

    fn two_address(
        &mut self,
        o: u16,
        a: Value,
        b: Value,
        r: Value,
        signed_rhs: bool,
        commutes: bool,
    ) {
        let w = bits(self.f.ty(a));
        let d = self.reg_of(r);
        let (mut a, mut b) = (a, b);
        // The operand that dies here is the one the result starts as.
        if commutes
            && self.imms[b.0 as usize].is_none()
            && self.uses[b.0 as usize] == 1
            && self.uses[a.0 as usize] != 1
        {
            std::mem::swap(&mut a, &mut b);
        }
        let first = self.src(a);
        self.emit(MInst::copy(d, first));
        let rhs = self.rhs(b, signed_rhs);
        self.emit(MInst::new(o, vec![Operand::use_def(d), rhs]));
        self.mask(d, w);
    }

    fn inst(&mut self, id: usize, bi: usize) {
        let f = self.f;
        let inst = &f.insts[id];
        let _ = bi;
        let res = inst.result;
        // A folded pointer or index is never computed (a `Slot` still clears its memory).
        if let (Some(r), false) = (res, matches!(inst.op, Op::Slot(..))) {
            if self.imms[r.0 as usize].is_some()
                || matches!(self.loc(r), Loc::Frame(..) | Loc::Folded)
            {
                return;
            }
        }
        match &inst.op {
            Op::Const(..) => {}
            Op::Bin(o, a, b) => self.bin(*o, *a, *b, res.unwrap()),
            Op::Checked(o, signed, a, b) => self.checked(*o, *signed, *a, *b, res.unwrap()),
            Op::Select(c, a, b) => self.select(*c, *a, *b, res.unwrap()),
            Op::Cmp(o, a, b) => {
                let r = res.unwrap();
                if self.deferred.contains_key(&r) {
                    return;
                }
                let d = self.reg_of(r);
                let Cond::Cmp(cc, ra, rb) = self.cmp_cond(*o, *a, *b) else {
                    unreachable!()
                };
                self.emit(MInst::new(
                    op::CMPSET,
                    vec![
                        Operand::def(d),
                        Operand::Imm(cc_num(cc)),
                        Operand::use_(ra),
                        rb,
                    ],
                ));
            }
            Op::Neg(a) => {
                let t = f.ty(*a);
                let d = self.reg_of(res.unwrap());
                let s = self.src(*a);
                self.emit(MInst::copy(d, s));
                self.emit(MInst::new(op::NEG, vec![Operand::use_def(d)]));
                self.mask(d, bits(t));
            }
            Op::Not(a) => {
                let t = f.ty(*a);
                let d = self.reg_of(res.unwrap());
                let s = self.src(*a);
                self.emit(MInst::copy(d, s));
                if t == IrTy::Bool {
                    self.emit(MInst::new(
                        op::XOR,
                        vec![Operand::use_def(d), Operand::Imm(1)],
                    ));
                } else {
                    self.emit(MInst::new(op::NOT, vec![Operand::use_def(d)]));
                    self.mask(d, bits(t));
                }
            }
            Op::Cast(k, a, to) => {
                let d = self.reg_of(res.unwrap());
                match k {
                    CastKind::ZExt | CastKind::Trunc => {
                        let s = self.src(*a);
                        self.emit(MInst::copy(d, s));
                        self.mask(d, bits(*to));
                    }
                    CastKind::SExt => {
                        let s = self.ext(*a, true);
                        self.emit(MInst::copy(d, Operand::use_(s)));
                        self.mask(d, bits(*to));
                    }
                    CastKind::PtrToInt | CastKind::IntToPtr => {
                        let s = self.src(*a);
                        self.emit(MInst::copy(d, s));
                    }
                    _ => unreachable!("float casts are not selected"),
                }
            }
            Op::Call(callee, args) => {
                self.call(args, None, Some(*callee), res);
            }
            Op::CallPtr(_, _, fp, args) => {
                self.call(args, Some(*fp), None, res);
            }
            Op::Slot(sz, _) => {
                let fid = self.slot_frame[&id];
                if *sz > 0 && !self.overwritten_first(id, *sz as i32, bi) {
                    // Small sizes clear with `xmm15`, large ones with `rep stos` through `rax` and `rcx`.
                    let used = if *sz <= 128 {
                        RegSet::of(&[x::xmm(15)])
                    } else {
                        RegSet::of(&[RAX, RCX])
                    };
                    self.emit(
                        MInst::new(
                            op::ZERO,
                            vec![Operand::Frame(fid, 0), Operand::Imm(*sz as i64)],
                        )
                        .clobbering(used),
                    );
                }
            }
            Op::PtrAdd(p, n) => self.two_address(op::ADD, *p, *n, res.unwrap(), false, true),
            Op::MemCopy(dst, src, n) => {
                if let Some(len) = self.imms[n.0 as usize] {
                    if small_copy(Some(len)) {
                        if len > 0 {
                            let md = self.plain_mem(*dst);
                            let ms = self.plain_mem(*src);
                            self.emit(
                                MInst::new(op::MEMCOPY, vec![md, ms, Operand::Imm(len)])
                                    .clobbering(RegSet::of(&[RAX])),
                            );
                        }
                        return;
                    }
                }
                let (d, s, n) = (self.src(*dst), self.src(*src), self.src(*n));
                let regs = [x::RDI, RSI, RCX].map(Reg::Phys);
                self.copies(vec![(regs[0], d), (regs[1], s), (regs[2], n)]);
                self.emit(MInst::new(
                    op::REP_MOVSB,
                    regs.iter().map(|r| Operand::use_def(*r)).collect(),
                ));
            }
            Op::DataAddr(i) => {
                let d = self.reg_of(res.unwrap());
                self.emit(MInst::new(
                    op::DATAADDR,
                    vec![Operand::def(d), Operand::Imm(*i as i64)],
                ));
            }
            Op::FuncAddr(fid) => {
                let d = self.reg_of(res.unwrap());
                self.emit(MInst::new(
                    op::FUNCADDR,
                    vec![Operand::def(d), Operand::Sym(SYM_FUNC + fid.0)],
                ));
            }
            Op::Load(t, p) => {
                let m = self.mem_of(*p);
                let d = self.reg_of(res.unwrap());
                self.emit(MInst::new(
                    op::LOAD,
                    vec![Operand::def(d), m, Operand::Imm(size(*t) as i64)],
                ));
            }
            Op::Store(p, v) => {
                let t = f.ty(*v);
                let m = self.mem_of(*p);
                let s = match self.imm32(*v, false) {
                    Some(n) => Operand::Imm(n as i64),
                    None => Operand::use_(self.vr(*v)),
                };
                self.emit(MInst::new(
                    op::STORE,
                    vec![m, s, Operand::Imm(size(t) as i64)],
                ));
            }
            Op::Alloc(n) => {
                let s = self.src(*n);
                self.runtime(op::RT_ALLOC, vec![(x::RDI, s)], res);
            }
            Op::Free(p) => {
                let s = self.src(*p);
                self.runtime(op::RT_FREE, vec![(x::RDI, s)], None);
            }
            Op::Realloc(p, n) => {
                let (a, b) = (self.src(*p), self.src(*n));
                self.runtime(op::RT_REALLOC, vec![(x::RDI, a), (RSI, b)], res);
            }
            Op::Print(p, n) => {
                let (a, b) = (self.src(*p), self.src(*n));
                self.runtime(op::RT_PRINT, vec![(RSI, a), (RDX, b)], None);
            }
            Op::CtxInit(b, n, c, e) => {
                let (b, n, c, e) = (self.src(*b), self.src(*n), self.src(*c), self.src(*e));
                let args = vec![(x::RDI, b), (RSI, n), (RDX, c), (x::RCX, e)];
                self.runtime(op::RT_CTX_INIT, args, res);
            }
            Op::CtxSwitch(s, t) => {
                let (a, b) = (self.src(*s), self.src(*t));
                self.runtime(op::RT_CTX_SWITCH, vec![(x::RDI, a), (RSI, b)], None);
            }
            Op::Fma(..) | Op::FUn(..) | Op::VUn(..) | Op::VBin(..) | Op::CheckedIf(..) => {
                unreachable!("not selected")
            }
        }
    }

    /// A call of a runtime routine: arguments in registers, the result in `rax`.
    fn runtime(&mut self, which: i64, args: Vec<(PReg, Operand)>, res: Option<Value>) {
        let pairs: Vec<(Reg, Operand)> = args
            .iter()
            .map(|(p, s)| (Reg::Phys(*p), s.clone()))
            .collect();
        self.copies(pairs);
        let mut ops = vec![Operand::Imm(which)];
        ops.extend(args.iter().map(|(p, _)| Operand::use_(Reg::Phys(*p))));
        self.emit(MInst::new(op::CALLRT, ops).call(call_clobbers()));
        if let Some(r) = res {
            let d = self.reg_of(r);
            self.emit(MInst::copy(d, Operand::use_(Reg::Phys(RAX))));
        }
    }

    /// A call: stack arguments, register arguments, the call, the result.
    fn call(
        &mut self,
        args: &[Value],
        fptr: Option<Value>,
        callee: Option<crate::ir::FuncId>,
        res: Option<Value>,
    ) {
        let mut regs = Vec::new();
        let mut copies = Vec::new();
        let mut stack = 0u32;
        for a in args {
            if regs.len() < INT_ARGS.len() {
                let p = INT_ARGS[regs.len()];
                regs.push(p);
                let s = self.src(*a);
                copies.push((Reg::Phys(p), s));
            } else {
                let out = match self.out_area {
                    Some(o) => o,
                    None => {
                        let o = self.mf.new_frame(0, 8, FrameKind::Outgoing);
                        self.out_area = Some(o);
                        o
                    }
                };
                let s = match self.imm32(*a, false) {
                    Some(n) => Operand::Imm(n as i64),
                    None => Operand::use_(self.vr(*a)),
                };
                let m = Operand::Mem {
                    base: MemBase::Frame(out),
                    index: None,
                    disp: 8 * stack as i32,
                };
                self.emit(MInst::new(op::STORE, vec![m, s, Operand::Imm(8)]));
                stack += 1;
            }
        }
        self.out_size = self.out_size.max(8 * stack);
        let target = x::R10;
        if let Some(p) = fptr {
            let s = self.src(p);
            copies.push((Reg::Phys(target), s));
        }
        self.copies(copies);
        let mut ops = Vec::new();
        match (callee, fptr) {
            (Some(c), _) => ops.push(Operand::Sym(SYM_FUNC + c.0)),
            (None, Some(_)) => ops.push(Operand::use_(Reg::Phys(target))),
            _ => unreachable!(),
        }
        ops.extend(regs.iter().map(|p| Operand::use_(Reg::Phys(*p))));
        let opc = if callee.is_some() {
            op::CALL
        } else {
            op::CALLR
        };
        self.emit(MInst::new(opc, ops).call(call_clobbers()));
        if let Some(r) = res {
            let d = self.reg_of(r);
            self.emit(MInst::copy(d, Operand::use_(Reg::Phys(RAX))));
        }
    }

    fn bin(&mut self, o: BinOp, a: Value, b: Value, r: Value) {
        let t = self.f.ty(a);
        let w = bits(t);
        match o {
            BinOp::Mul if self.imm32(b, false).is_some() || self.imm32(a, false).is_some() => {
                // `imul d, a, imm`, or `lea` for 3, 5 and 9, which is faster. The constant may be on either side.
                let (a, b) = if self.imm32(b, false).is_some() {
                    (a, b)
                } else {
                    (b, a)
                };
                let n = self.imm32(b, false).unwrap();
                let d = self.reg_of(r);
                let ra = self.vr(a);
                if let (3 | 5 | 9, 64) = (n, w) {
                    let m = Operand::Mem {
                        base: MemBase::Reg(ra),
                        index: Some((ra, (n - 1) as u8)),
                        disp: 0,
                    };
                    self.emit(MInst::new(op::LEA, vec![Operand::def(d), m]));
                } else {
                    self.emit(MInst::new(
                        op::IMULI,
                        vec![Operand::def(d), Operand::use_(ra), Operand::Imm(n as i64)],
                    ));
                    self.mask(d, w);
                }
            }
            BinOp::Mul => {
                let d = self.reg_of(r);
                let (mut a, mut b) = (a, b);
                if self.uses[b.0 as usize] == 1 && self.uses[a.0 as usize] != 1 {
                    std::mem::swap(&mut a, &mut b);
                }
                let s = self.src(a);
                self.emit(MInst::copy(d, s));
                let rb = self.vr(b);
                self.emit(MInst::new(
                    op::IMUL,
                    vec![Operand::use_def(d), Operand::use_(rb)],
                ));
                self.mask(d, w);
            }
            BinOp::Add => self.two_address(op::ADD, a, b, r, false, true),
            BinOp::Sub => self.two_address(op::SUB, a, b, r, false, false),
            BinOp::And => self.two_address(op::AND, a, b, r, false, true),
            BinOp::Or => self.two_address(op::OR, a, b, r, false, true),
            BinOp::Xor => self.two_address(op::XOR, a, b, r, false, true),
            BinOp::Div | BinOp::Rem | BinOp::UDiv | BinOp::URem => {
                let signed = matches!(o, BinOp::Div | BinOp::Rem);
                let want_rem = matches!(o, BinOp::Rem | BinOp::URem);
                self.copy_ext_to(RAX, a, signed);
                let rb = self.ext(b, signed);
                let mut kind = 0;
                if signed {
                    kind |= op::DIV_SIGNED;
                    if w == 64 {
                        kind |= op::DIV_WIDE;
                    }
                }
                if want_rem {
                    kind |= op::DIV_REM;
                }
                self.emit(MInst::new(
                    op::DIV,
                    vec![
                        Operand::use_def(Reg::Phys(RAX)),
                        Operand::def(Reg::Phys(RDX)),
                        Operand::use_(rb),
                        Operand::Imm(kind),
                    ],
                ));
                let d = self.reg_of(r);
                let out = if want_rem { RDX } else { RAX };
                self.emit(MInst::copy(d, Operand::use_(Reg::Phys(out))));
                self.mask(d, w);
            }
            BinOp::MulHi | BinOp::UMulHi => {
                let signed = o == BinOp::MulHi;
                if w == 64 {
                    self.copy_ext_to(RAX, a, signed);
                    let rb = self.ext(b, signed);
                    self.emit(MInst::new(
                        op::MULHI,
                        vec![
                            Operand::use_def(Reg::Phys(RAX)),
                            Operand::def(Reg::Phys(RDX)),
                            Operand::use_(rb),
                            Operand::Imm(i64::from(signed)),
                        ],
                    ));
                    let d = self.reg_of(r);
                    self.emit(MInst::copy(d, Operand::use_(Reg::Phys(RDX))));
                } else {
                    let ra = self.ext(a, signed);
                    let rb = self.ext(b, signed);
                    let d = self.reg_of(r);
                    self.emit(MInst::copy(d, Operand::use_(ra)));
                    self.emit(MInst::new(
                        op::IMUL,
                        vec![Operand::use_def(d), Operand::use_(rb)],
                    ));
                    let sh = if signed { op::SAR_I } else { op::SHR_I };
                    self.emit(MInst::new(
                        sh,
                        vec![Operand::use_def(d), Operand::Imm(w as i64)],
                    ));
                    self.mask(d, w);
                }
            }
            BinOp::Shl | BinOp::Shr | BinOp::UShr => {
                let signed = o == BinOp::Shr;
                let constant = self.imms[b.0 as usize].filter(|n| (0..w as i64).contains(n));
                let d = self.reg_of(r);
                if let Some(n) = constant {
                    let ra = self.ext(a, signed);
                    self.emit(MInst::copy(d, Operand::use_(ra)));
                    let opc = match o {
                        BinOp::Shl => op::SHL_I,
                        BinOp::Shr => op::SAR_I,
                        _ => op::SHR_I,
                    };
                    self.emit(MInst::new(opc, vec![Operand::use_def(d), Operand::Imm(n)]));
                } else {
                    let ra = self.ext(a, signed);
                    self.emit(MInst::copy(d, Operand::use_(ra)));
                    let s = self.src(b);
                    self.emit(MInst::copy(Reg::Phys(RCX), s));
                    let k = match o {
                        BinOp::Shl => 0,
                        BinOp::Shr => 2,
                        _ => 1,
                    };
                    self.emit(MInst::new(
                        op::SHIFT_CL,
                        vec![
                            Operand::use_def(d),
                            Operand::use_(Reg::Phys(RCX)),
                            Operand::Imm(k),
                            Operand::Imm(w as i64),
                        ],
                    ));
                }
                self.mask(d, w);
            }
        }
    }

    fn checked(&mut self, o: BinOp, signed: bool, a: Value, b: Value, r: Value) {
        let w = bits(self.f.ty(a));
        let d = self.reg_of(r);
        if w == 64 && (matches!(o, BinOp::Add | BinOp::Sub) || (o == BinOp::Mul && signed)) {
            let (mut a, mut b) = (a, b);
            if o != BinOp::Sub
                && self.imms[b.0 as usize].is_none()
                && self.uses[b.0 as usize] == 1
                && self.uses[a.0 as usize] != 1
            {
                std::mem::swap(&mut a, &mut b);
            }
            let s = self.src(a);
            self.emit(MInst::copy(d, s));
            let rhs = self.rhs(b, signed);
            let kind = match (o, signed) {
                (BinOp::Add, true) => op::CHK_ADD_S,
                (BinOp::Sub, true) => op::CHK_SUB_S,
                (BinOp::Mul, _) => op::CHK_MUL_S,
                (BinOp::Add, false) => op::CHK_ADD_U,
                _ => op::CHK_SUB_U,
            };
            self.emit(MInst::new(
                op::CHK,
                vec![Operand::use_def(d), rhs, Operand::Imm(kind)],
            ));
            return;
        }
        let ra = self.ext(a, signed);
        let rb = self.ext(b, signed);
        if w == 64 {
            // Unsigned multiply.
            self.emit(MInst::copy(Reg::Phys(RAX), Operand::use_(ra)));
            self.emit(MInst::new(
                op::MULU_CHK,
                vec![
                    Operand::use_def(Reg::Phys(RAX)),
                    Operand::def(Reg::Phys(RDX)),
                    Operand::use_(rb),
                ],
            ));
            self.emit(MInst::copy(d, Operand::use_(Reg::Phys(RAX))));
        } else {
            // Narrow: no overflow in 64 bits; the result must survive the trip through the width.
            let t = self.new_vreg();
            self.emit(MInst::copy(t, Operand::use_(ra)));
            let opc = match o {
                BinOp::Add => op::ADD,
                BinOp::Sub => op::SUB,
                _ => op::IMUL,
            };
            self.emit(MInst::new(
                opc,
                vec![Operand::use_def(t), Operand::use_(rb)],
            ));
            let e = self.new_vreg();
            self.emit(MInst::new(
                op::EXT,
                vec![
                    Operand::def(e),
                    Operand::use_(t),
                    Operand::Imm((w / 8) as i64),
                    Operand::Imm(i64::from(signed)),
                ],
            ));
            self.emit(MInst::new(
                op::CMPSTOP,
                vec![
                    Operand::Imm(cc_num(Cc::Ne)),
                    Operand::use_(e),
                    Operand::use_(t),
                    Operand::Sym(x::SYM_OVERFLOW),
                ],
            ));
            self.emit(MInst::copy(d, Operand::use_(t)));
            self.mask(d, w);
        }
    }

    fn select(&mut self, c: Value, a: Value, b: Value, r: Value) {
        let d = self.reg_of(r);
        // The result starts as the false operand and takes the true one when the condition holds.
        let sb = self.src(b);
        let ra = self.vr(a);
        match self.deferred.get(&c).copied() {
            Some((o, x, y)) => {
                self.emit(MInst::copy(d, sb));
                let Cond::Cmp(cc, rx, ry) = self.cmp_cond(o, x, y) else {
                    unreachable!()
                };
                self.emit(MInst::new(
                    op::CMPCMOV,
                    vec![
                        Operand::use_def(d),
                        Operand::use_(ra),
                        Operand::Imm(cc_num(cc)),
                        Operand::use_(rx),
                        ry,
                    ],
                ));
            }
            None => {
                let rc = self.vr(c);
                self.emit(MInst::copy(d, sb));
                self.emit(MInst::new(
                    op::TESTCMOV,
                    vec![
                        Operand::use_def(d),
                        Operand::use_(ra),
                        Operand::Imm(cc_num(Cc::Ne)),
                        Operand::use_(rc),
                    ],
                ));
            }
        }
    }

    /// Does the code after the `Slot` instruction `id` (`size` bytes) write every byte of it, in its
    /// own block, before anything reads or passes on its address? Then the clearing can be left out.
    fn overwritten_first(&self, id: usize, size: i32, bi: usize) -> bool {
        let f = self.f;
        if size > 4096 {
            return false;
        }
        let b = &f.blocks[bi];
        let k = b
            .insts
            .iter()
            .position(|&j| j == id)
            .expect("the slot is in its block");
        // The offset in the slot of an address that is a plain frame offset of it.
        let in_slot = |v: Value| -> Option<i32> {
            match self.folds[v.0 as usize] {
                Fold::Addr {
                    base: Base::Slot(s),
                    index: None,
                    disp,
                } if s == id && (0..size).contains(&disp) => Some(disp),
                _ => None,
            }
        };
        // An address built on this slot that is not a plain frame offset.
        let touches = |v: Value| -> bool {
            matches!(
                self.folds[v.0 as usize],
                Fold::Addr {
                    base: Base::Slot(s),
                    ..
                } if s == id
            )
        };
        let mut written = vec![false; size as usize];
        let mut left = size as usize;
        let mut mark = |from: i32, len: i32, written: &mut Vec<bool>| {
            for x in from.max(0)..(from + len).min(size) {
                if !written[x as usize] {
                    written[x as usize] = true;
                    left -= 1;
                }
            }
            left == 0
        };
        for &j in &b.insts[k + 1..] {
            match &f.insts[j].op {
                Op::Store(p, x) => {
                    if touches(*x) {
                        return false;
                    }
                    if let Some(at) = in_slot(*p) {
                        if mark(at, size_of(f.ty(*x)), &mut written) {
                            return true;
                        }
                    } else if touches(*p) {
                        return false;
                    }
                }
                Op::MemCopy(d, s, n) => {
                    if touches(*s) || touches(*n) {
                        return false;
                    }
                    if touches(*d) {
                        match (in_slot(*d), self.imms[n.0 as usize]) {
                            (Some(at), Some(len)) => {
                                if mark(at, len as i32, &mut written) {
                                    return true;
                                }
                            }
                            _ => return false,
                        }
                    }
                }
                op => {
                    let mut vs = Vec::new();
                    super::regs::inst_uses(op, &mut vs);
                    if vs.into_iter().any(touches) {
                        return false;
                    }
                }
            }
        }
        false
    }
}

fn size_of(t: IrTy) -> i32 {
    size(t) as i32
}
