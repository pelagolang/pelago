//! From the IR to AArch64 MIR (design in `docs/design/mir.md`, opcodes in [`super::a64_mir`]).
//!
//! The same plan as [`super::x86_select`]: every IR value that is not a constant or a folded address
//! becomes a virtual register, integers, booleans and pointers are 64 bits wide in a register and
//! zero-extended from their type, an operation on a narrow type cuts its result back with `MASK`, and a
//! signed use widens its operands with `EXT` first. Instructions are three-address here.
//!
//! The selector gives up (returns `None`) on a function that has anything it cannot do yet (floats,
//! vectors); the caller then uses the older generator for that function.

use super::a64::Asm;
use super::a64_mir::{self as x, call_clobbers, cc_num, op, Cond, CALL_TARGET, INT_ARGS, SYM_FUNC};
use super::gen_a64::{classify, Loc as ArgLoc};
use super::mir::*;
use super::regs::{folds, fused_cmp, small_copy, Base, Fold};
use crate::ir::Target as JumpTarget;
use crate::ir::{BinOp, CastKind, CmpOp, Const, Func, IrTy, Op, Term, Test, Value};
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
enum CondOp {
    Cmp(Cond, Reg, Operand),
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
    filled: crate::ir::opt::fx::HashSet<usize>,
}

fn is_plain(t: IrTy) -> bool {
    !matches!(t, IrTy::F32 | IrTy::F64 | IrTy::V2I64)
}

/// Can the selector do this function? Not yet: floats and vectors.
fn supported(f: &Func) -> bool {
    if !is_plain(f.ret) && f.ret != IrTy::Void {
        return false;
    }
    if f.values.iter().any(|t| !is_plain(*t)) {
        return false;
    }
    f.blocks
        .iter()
        .flat_map(|b| &b.insts)
        .all(|&i| match &f.insts[i].op {
            Op::CallPtr(tys, ret, ..) => is_plain(*ret) && tys.iter().all(|t| is_plain(*t)),
            _ => true,
        })
}

/// Makes the MIR of a function, or `None` when this selector cannot (yet).
pub fn select(f: &Func) -> Option<MFunc> {
    if !supported(f) {
        return None;
    }
    let filled = crate::ir::opt::filled_slots(f);
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
        filled,
    };
    s.function();
    Some(s.mf)
}

/// The `add` / `sub` immediate for `n`: whether the instruction is the opposite one (the constant was
/// negative) and the value.
fn addsub_imm(n: i64) -> Option<(bool, i64)> {
    if Asm::fits_addsub(n as u64) && n >= 0 {
        Some((false, n))
    } else if n < 0 && Asm::fits_addsub(n.unsigned_abs()) {
        Some((true, n.abs()))
    } else {
        None
    }
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

    /// The constant a value is once widened (signed or not).
    fn konst(&self, v: Value, signed: bool) -> Option<i64> {
        let n = self.imms[v.0 as usize]?;
        Some(widen(n, self.f.ty(v), signed))
    }

    /// The right operand of `and` / `orr` / `eor`: a bitmask immediate, else a register.
    fn rhs_logic(&mut self, v: Value) -> Operand {
        match self.konst(v, false) {
            Some(n) if n != 0 && Asm::bitmask(n as u64).is_some() => Operand::Imm(n),
            _ => Operand::use_(self.vr(v)),
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

    /// Cuts `d` to `w` bits in place.
    fn mask(&mut self, d: Reg, w: u32) {
        if w < 64 {
            self.emit(MInst::new(
                op::MASK,
                vec![Operand::def(d), Operand::use_(d), Operand::Imm(w as i64)],
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
        let tys: Vec<IrTy> = params.iter().map(|p| f.ty(*p)).collect();
        let locs = classify(&tys);
        let nstack = locs
            .iter()
            .filter(|l| matches!(l, ArgLoc::Stack(_)))
            .count();
        let incoming =
            (nstack > 0).then(|| self.mf.new_frame(8 * nstack as u32, 8, FrameKind::Incoming));
        let mut copies = Vec::new();
        for (p, loc) in params.iter().zip(&locs) {
            if self.uses[p.0 as usize] == 0 {
                continue;
            }
            let r = self.reg_of(*p);
            match loc {
                ArgLoc::Int(i) => copies.push((r, Operand::use_(Reg::Phys(INT_ARGS[*i])))),
                ArgLoc::Float(_) => unreachable!("floats are not selected yet"),
                ArgLoc::Stack(k) => {
                    let m = Operand::Mem {
                        base: MemBase::Frame(incoming.expect("stack parameters")),
                        index: None,
                        disp: 8 * *k as i32,
                    };
                    self.emit(MInst::new(
                        op::LOAD,
                        vec![Operand::def(r), m, Operand::Imm(8)],
                    ));
                }
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

    fn jcc(&self, cond: &CondOp, inv: bool, to: u32) -> MInst {
        match cond.clone() {
            CondOp::Cmp(cc, a, b) => {
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
            CondOp::Test(r) => {
                let cc = if inv { Cond::Eq } else { Cond::Ne };
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
                let r = x::x(0);
                self.emit(MInst::copy(Reg::Phys(r), s));
                self.emit(MInst::new(op::RET, vec![Operand::use_(Reg::Phys(r))]).nofall());
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
                    None => CondOp::Test(self.vr(*c)),
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
    fn cmp_cond(&mut self, o: CmpOp, a: Value, b: Value) -> CondOp {
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
        let rb = match self.konst(b, signed) {
            Some(n) if (0..4096).contains(&n) => Operand::Imm(n),
            _ => Operand::use_(self.ext(b, signed)),
        };
        let cc = match o {
            CmpOp::Eq => Cond::Eq,
            CmpOp::Ne => Cond::Ne,
            CmpOp::Lt if signed => Cond::Lt,
            CmpOp::Le if signed => Cond::Le,
            CmpOp::Gt if signed => Cond::Gt,
            CmpOp::Ge if signed => Cond::Ge,
            CmpOp::Lt | CmpOp::ULt => Cond::Lo,
            CmpOp::Le | CmpOp::ULe => Cond::Ls,
            CmpOp::Gt | CmpOp::UGt => Cond::Hi,
            _ => Cond::Hs,
        };
        CondOp::Cmp(cc, ra, rb)
    }

    // ---- Instructions ------------------------------------------------------------------------------

    /// `d = a op b` for `add`, `sub`, `and`, `orr`, `eor` with an immediate where one encodes.
    fn arith(&mut self, o: BinOp, a: Value, b: Value, r: Value) {
        let w = bits(self.f.ty(a));
        let d = self.reg_of(r);
        let commutes = o != BinOp::Sub;
        // A constant on the left of a commutative operation goes to the right.
        let (a, b) =
            if commutes && self.imms[a.0 as usize].is_some() && self.imms[b.0 as usize].is_none() {
                (b, a)
            } else {
                (a, b)
            };
        let ra = self.vr(a);
        let (opc, rhs) = match o {
            BinOp::Add | BinOp::Sub => match self.konst(b, false).map(addsub_imm) {
                Some(Some((flip, n))) => {
                    let opc = if (o == BinOp::Add) != flip {
                        op::ADD
                    } else {
                        op::SUB
                    };
                    (opc, Operand::Imm(n))
                }
                _ => (
                    if o == BinOp::Add { op::ADD } else { op::SUB },
                    Operand::use_(self.vr(b)),
                ),
            },
            BinOp::And => (op::AND, self.rhs_logic(b)),
            BinOp::Or => (op::OR, self.rhs_logic(b)),
            _ => (op::XOR, self.rhs_logic(b)),
        };
        self.emit(MInst::new(
            opc,
            vec![Operand::def(d), Operand::use_(ra), rhs],
        ));
        if matches!(o, BinOp::Add | BinOp::Sub) {
            self.mask(d, w);
        }
    }

    fn inst(&mut self, id: usize, bi: usize) {
        let f = self.f;
        let inst = &f.insts[id];
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
            Op::Checked(o, signed, a, b) => {
                self.checked(*o, *signed, *a, *b, res.unwrap(), Vec::new())
            }
            Op::Select(c, a, b) => self.select(*c, *a, *b, res.unwrap()),
            Op::CheckedIf(o, signed, test, a, b) => {
                let tail = self.test_ops(test);
                self.checked(*o, *signed, *a, *b, res.unwrap(), tail);
            }
            Op::Cmp(o, a, b) => {
                let r = res.unwrap();
                if self.deferred.contains_key(&r) {
                    return;
                }
                let d = self.reg_of(r);
                let CondOp::Cmp(cc, ra, rb) = self.cmp_cond(*o, *a, *b) else {
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
                let s = self.vr(*a);
                self.emit(MInst::new(op::NEG, vec![Operand::def(d), Operand::use_(s)]));
                self.mask(d, bits(t));
            }
            Op::Not(a) => {
                let t = f.ty(*a);
                let d = self.reg_of(res.unwrap());
                let s = self.vr(*a);
                if t == IrTy::Bool {
                    self.emit(MInst::new(
                        op::XOR,
                        vec![Operand::def(d), Operand::use_(s), Operand::Imm(1)],
                    ));
                } else {
                    self.emit(MInst::new(op::NOT, vec![Operand::def(d), Operand::use_(s)]));
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
                    _ => unreachable!("float casts are not selected yet"),
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
                if *sz > 0
                    && !self.filled.contains(&id)
                    && !self.overwritten_first(id, *sz as i32, bi)
                {
                    self.emit(
                        MInst::new(
                            op::ZERO,
                            vec![Operand::Frame(fid, 0), Operand::Imm(*sz as i64)],
                        )
                        .clobbering(RegSet::of(&[x::x(9), x::x(10)])),
                    );
                }
            }
            Op::PtrAdd(p, n) => self.arith(BinOp::Add, *p, *n, res.unwrap()),
            Op::MemCopy(dst, src, n) => {
                if let Some(len) = self.imms[n.0 as usize] {
                    if small_copy(Some(len)) {
                        if len > 0 {
                            let md = self.plain_mem(*dst);
                            let ms = self.plain_mem(*src);
                            self.emit(
                                MInst::new(op::MEMCOPY, vec![md, ms, Operand::Imm(len)])
                                    .clobbering(RegSet::of(&[x::x(9)])),
                            );
                        }
                        return;
                    }
                }
                let (d, s, n) = (self.src(*dst), self.src(*src), self.src(*n));
                self.runtime(
                    op::RT_MEMCPY,
                    vec![(x::x(0), d), (x::x(1), s), (x::x(2), n)],
                    None,
                );
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
                let s = match self.konst(*v, false) {
                    Some(0) => Operand::Imm(0),
                    _ => Operand::use_(self.vr(*v)),
                };
                self.emit(MInst::new(
                    op::STORE,
                    vec![m, s, Operand::Imm(size(t) as i64)],
                ));
            }
            Op::Alloc(n) => {
                let s = self.src(*n);
                self.runtime(op::RT_ALLOC, vec![(x::x(0), s)], res);
            }
            Op::Free(p) => {
                let s = self.src(*p);
                self.runtime(op::RT_FREE, vec![(x::x(0), s)], None);
            }
            Op::Realloc(p, n) => {
                let (a, b) = (self.src(*p), self.src(*n));
                self.runtime(op::RT_REALLOC, vec![(x::x(0), a), (x::x(1), b)], res);
            }
            Op::Print(p, n) => {
                let (a, b) = (self.src(*p), self.src(*n));
                self.runtime(op::RT_PRINT, vec![(x::x(0), a), (x::x(1), b)], None);
            }
            Op::CtxInit(b, n, c, e) => {
                let (b, n, c, e) = (self.src(*b), self.src(*n), self.src(*c), self.src(*e));
                let args = vec![(x::x(0), b), (x::x(1), n), (x::x(2), c), (x::x(3), e)];
                self.runtime(op::RT_CTX_INIT, args, res);
            }
            Op::CtxSwitch(s, t) => {
                let (a, b) = (self.src(*s), self.src(*t));
                self.runtime(op::RT_CTX_SWITCH, vec![(x::x(0), a), (x::x(1), b)], None);
            }
            Op::Fma(..) | Op::FUn(..) | Op::VUn(..) | Op::VBin(..) => {
                unreachable!("not selected")
            }
        }
    }

    /// The test of a `CheckedIf` as the trailing operands of the stop jump (see the emitter).
    fn test_ops(&mut self, t: &Test) -> Vec<Operand> {
        match t {
            Test::Bool(c, when) => {
                let r = self.vr(*c);
                vec![
                    Operand::Imm(0),
                    Operand::use_(r),
                    Operand::Imm(i64::from(*when)),
                ]
            }
            Test::Cmp(o, x, y) => {
                let CondOp::Cmp(cc, ra, rb) = self.cmp_cond(*o, *x, *y) else {
                    unreachable!()
                };
                vec![
                    Operand::Imm(1),
                    Operand::Imm(cc_num(cc)),
                    Operand::use_(ra),
                    rb,
                ]
            }
        }
    }

    /// A call of a runtime routine: arguments in registers, the result in `x0`.
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
            self.emit(MInst::copy(d, Operand::use_(Reg::Phys(x::x(0)))));
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
        let tys: Vec<IrTy> = args.iter().map(|a| self.f.ty(*a)).collect();
        for (a, loc) in args.iter().zip(classify(&tys)) {
            match loc {
                ArgLoc::Int(i) => {
                    let p = INT_ARGS[i];
                    regs.push(p);
                    let s = self.src(*a);
                    copies.push((Reg::Phys(p), s));
                }
                ArgLoc::Float(_) => unreachable!("floats are not selected yet"),
                ArgLoc::Stack(k) => {
                    let out = match self.out_area {
                        Some(o) => o,
                        None => {
                            let o = self.mf.new_frame(0, 8, FrameKind::Outgoing);
                            self.out_area = Some(o);
                            o
                        }
                    };
                    let m = Operand::Mem {
                        base: MemBase::Frame(out),
                        index: None,
                        disp: 8 * k as i32,
                    };
                    let s = match self.konst(*a, false) {
                        Some(0) => Operand::Imm(0),
                        _ => Operand::use_(self.vr(*a)),
                    };
                    self.emit(MInst::new(op::STORE, vec![m, s, Operand::Imm(8)]));
                    stack = stack.max(k as u32 + 1);
                }
            }
        }
        self.out_size = self.out_size.max(8 * stack);
        let target = CALL_TARGET;
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
            self.emit(MInst::copy(d, Operand::use_(Reg::Phys(x::x(0)))));
        }
    }

    fn bin(&mut self, o: BinOp, a: Value, b: Value, r: Value) {
        let t = self.f.ty(a);
        let w = bits(t);
        match o {
            BinOp::Add | BinOp::Sub | BinOp::And | BinOp::Or | BinOp::Xor => self.arith(o, a, b, r),
            BinOp::Mul => {
                let d = self.reg_of(r);
                // The constant may be on either side; a power of two is a shift.
                let (a, b) =
                    if self.imms[a.0 as usize].is_some() && self.imms[b.0 as usize].is_none() {
                        (b, a)
                    } else {
                        (a, b)
                    };
                match self.konst(b, false) {
                    Some(n) if n > 1 && n & (n - 1) == 0 => {
                        let ra = self.vr(a);
                        self.emit(MInst::new(
                            op::SHL_I,
                            vec![
                                Operand::def(d),
                                Operand::use_(ra),
                                Operand::Imm(n.trailing_zeros() as i64),
                            ],
                        ));
                    }
                    _ => {
                        let (ra, rb) = (self.vr(a), self.vr(b));
                        self.emit(MInst::new(
                            op::MUL,
                            vec![Operand::def(d), Operand::use_(ra), Operand::use_(rb)],
                        ));
                    }
                }
                self.mask(d, w);
            }
            BinOp::Div | BinOp::Rem | BinOp::UDiv | BinOp::URem => {
                let signed = matches!(o, BinOp::Div | BinOp::Rem);
                let want_rem = matches!(o, BinOp::Rem | BinOp::URem);
                let (ra, rb) = (self.ext(a, signed), self.ext(b, signed));
                let d = self.reg_of(r);
                let mut kind = 0;
                if signed {
                    kind |= op::DIV_SIGNED;
                }
                if want_rem {
                    kind |= op::DIV_REM;
                }
                self.emit(
                    MInst::new(
                        op::DIV,
                        vec![
                            Operand::def(d),
                            Operand::use_(ra),
                            Operand::use_(rb),
                            Operand::Imm(kind),
                        ],
                    )
                    .clobbering(RegSet::of(&[x::x(9)])),
                );
                self.mask(d, w);
            }
            BinOp::MulHi | BinOp::UMulHi => {
                let signed = o == BinOp::MulHi;
                let (ra, rb) = (self.ext(a, signed), self.ext(b, signed));
                let d = self.reg_of(r);
                if w == 64 {
                    self.emit(MInst::new(
                        op::MULHI,
                        vec![
                            Operand::def(d),
                            Operand::use_(ra),
                            Operand::use_(rb),
                            Operand::Imm(i64::from(signed)),
                        ],
                    ));
                } else {
                    self.emit(MInst::new(
                        op::MUL,
                        vec![Operand::def(d), Operand::use_(ra), Operand::use_(rb)],
                    ));
                    let sh = if signed { op::SAR_I } else { op::SHR_I };
                    self.emit(MInst::new(
                        sh,
                        vec![Operand::def(d), Operand::use_(d), Operand::Imm(w as i64)],
                    ));
                    self.mask(d, w);
                }
            }
            BinOp::Shl | BinOp::Shr | BinOp::UShr => {
                let signed = o == BinOp::Shr;
                let constant = self.imms[b.0 as usize].filter(|n| (0..w as i64).contains(n));
                let d = self.reg_of(r);
                let ra = self.ext(a, signed);
                if let Some(n) = constant {
                    let opc = match o {
                        BinOp::Shl => op::SHL_I,
                        BinOp::Shr => op::SAR_I,
                        _ => op::SHR_I,
                    };
                    self.emit(MInst::new(
                        opc,
                        vec![Operand::def(d), Operand::use_(ra), Operand::Imm(n)],
                    ));
                } else {
                    let rb = self.vr(b);
                    let k = match o {
                        BinOp::Shl => 0,
                        BinOp::Shr => 2,
                        _ => 1,
                    };
                    self.emit(MInst::new(
                        op::SHIFT_CHK,
                        vec![
                            Operand::def(d),
                            Operand::use_(ra),
                            Operand::use_(rb),
                            Operand::Imm(k),
                            Operand::Imm(w as i64),
                        ],
                    ));
                }
                self.mask(d, w);
            }
        }
    }

    /// A checked operation. `tail` is the test of a `CheckedIf`, or empty.
    fn checked(
        &mut self,
        o: BinOp,
        signed: bool,
        a: Value,
        b: Value,
        r: Value,
        tail: Vec<Operand>,
    ) {
        let w = bits(self.f.ty(a));
        let d = self.reg_of(r);
        if w == 64 {
            let (mut a, mut b) = (a, b);
            if o != BinOp::Sub
                && self.imms[a.0 as usize].is_some()
                && self.imms[b.0 as usize].is_none()
            {
                std::mem::swap(&mut a, &mut b);
            }
            let ra = self.vr(a);
            let (kind, rhs) = match (o, signed) {
                (BinOp::Add, true) | (BinOp::Sub, true) => {
                    // `a + -k` is `a - k` for the signed overflow flag.
                    let want_add = o == BinOp::Add;
                    match self.konst(b, false).map(addsub_imm) {
                        Some(Some((flip, n))) => {
                            let add = want_add != flip;
                            (
                                if add { op::CHK_ADD_S } else { op::CHK_SUB_S },
                                Operand::Imm(n),
                            )
                        }
                        _ => (
                            if want_add {
                                op::CHK_ADD_S
                            } else {
                                op::CHK_SUB_S
                            },
                            Operand::use_(self.vr(b)),
                        ),
                    }
                }
                (BinOp::Add, false) | (BinOp::Sub, false) => {
                    let kind = if o == BinOp::Add {
                        op::CHK_ADD_U
                    } else {
                        op::CHK_SUB_U
                    };
                    match self.konst(b, false) {
                        Some(n) if (0..4096).contains(&n) => (kind, Operand::Imm(n)),
                        _ => (kind, Operand::use_(self.vr(b))),
                    }
                }
                (_, true) => (op::CHK_MUL_S, Operand::use_(self.vr(b))),
                (_, false) => (op::CHK_MUL_U, Operand::use_(self.vr(b))),
            };
            // The stub of a test reads its registers after the result is written.
            let dop = if tail.is_empty() {
                Operand::def(d)
            } else {
                Operand::Reg(d, Role::EarlyDef)
            };
            let mut ops = vec![dop, Operand::use_(ra), rhs, Operand::Imm(kind)];
            ops.extend(tail);
            self.emit(MInst::new(op::CHK, ops).clobbering(RegSet::of(&[x::x(9)])));
            return;
        }
        // Narrow: no overflow in 64 bits; the result must survive the trip through the width.
        let ra = self.ext(a, signed);
        let rb = self.ext(b, signed);
        let t = self.new_vreg();
        let opc = match o {
            BinOp::Add => op::ADD,
            BinOp::Sub => op::SUB,
            _ => op::MUL,
        };
        self.emit(MInst::new(
            opc,
            vec![Operand::def(t), Operand::use_(ra), Operand::use_(rb)],
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
        let mut ops = vec![
            Operand::Imm(cc_num(Cond::Ne)),
            Operand::use_(e),
            Operand::use_(t),
            Operand::Sym(x::SYM_OVERFLOW),
        ];
        ops.extend(tail);
        self.emit(MInst::new(op::CMPSTOP, ops));
        self.emit(MInst::copy(d, Operand::use_(t)));
        self.mask(d, w);
    }

    fn select(&mut self, c: Value, a: Value, b: Value, r: Value) {
        let d = self.reg_of(r);
        let (ra, rb) = (self.vr(a), self.vr(b));
        match self.deferred.get(&c).copied() {
            Some((o, x, y)) => {
                let CondOp::Cmp(cc, rx, ry) = self.cmp_cond(o, x, y) else {
                    unreachable!()
                };
                self.emit(MInst::new(
                    op::CMPSEL,
                    vec![
                        Operand::def(d),
                        Operand::use_(ra),
                        Operand::use_(rb),
                        Operand::Imm(cc_num(cc)),
                        Operand::use_(rx),
                        ry,
                    ],
                ));
            }
            None => {
                let rc = self.vr(c);
                self.emit(MInst::new(
                    op::TESTSEL,
                    vec![
                        Operand::def(d),
                        Operand::use_(ra),
                        Operand::use_(rb),
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
