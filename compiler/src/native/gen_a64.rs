//! The AArch64 code generator (card A-10, stage R2): the IR to machine code for 64-bit ARM Linux.
//!
//! The same plan as the first x86-64 generator (`gen.rs`, stage N2): every IR value lives in its own
//! stack slot, and each operation is a fixed template over a few scratch registers. Integers,
//! booleans and pointers are kept **zero-extended** to 64 bits in their slot ("canonical form"); an
//! operation that needs the sign extends from the slot; results are cut back to their width. The rules
//! for division, shifts, overflow and NaN are in `docs/design/backend.md` and are the same as on x86-64.
//!
//! Registers: `x0` to `x7` and `d0` to `d7` carry arguments, `x0` / `d0` the result, further
//! arguments go to the outgoing area at the bottom of the caller's frame (`[sp + 8k]`, which the callee
//! sees at `[x29 + 16 + 8k]`). Templates use `x9` to `x13` and `v0`, `v1`; `x14` and `x15` belong to the
//! out-of-line tests of `CheckedIf`; `x16` and `x17` are the assembler's own scratch (`a64.rs`).
//! `x29` is the frame pointer; `x19` to `x28` are kept for the register allocator (stage R3).
//!
//! The frame, from `sp` up: the outgoing argument area, the value slots, the `Slot` objects, the saved
//! callee-saved registers, then `x29` and `x30` (pushed first, so `x29` points at them).

use super::a64::*;
use super::elf::{self, Blob, Symbol};
use super::gen::Options;
use super::rt_a64::{self, Runtime};
use super::Os;
use crate::ir::{
    BinOp, CastKind, CmpOp, Const, Func, FuncId, Inst, IrTy, Module, Op, Target as JumpTarget,
    Term, Test, VBin, VUn, Value,
};
use std::collections::HashMap;

const INT_REGS: [Reg; 8] = [X0, X1, X2, X3, X4, X5, X6, X7];
const MAX_FLOAT_REGS: usize = 8;
/// A value without a frame slot (a constant, or one nothing reads).
const NONE: i32 = i32::MIN;
/// Constant sizes up to this many bytes are copied by plain moves.
const SMALL_COPY: i64 = 64;

/// Compiles the module to an AArch64 Linux executable whose entry runs the function named `entry`.
pub fn compile(m: &Module, entry: &str, opts: &Options) -> Result<Vec<u8>, String> {
    if opts.target != Os::Linux {
        return Err("the AArch64 backend builds Linux programs only (card A-10, stage R4)".into());
    }
    let main_fn = m
        .funcs
        .iter()
        .position(|f| f.name == entry)
        .ok_or_else(|| format!("no function `{entry}`"))?;
    let mut g = Gen::new(m);
    let main_label = g.funcs[main_fn];
    let rt = rt_a64::emit(
        &mut g.a,
        &mut g.ro,
        &mut g.data,
        main_label,
        opts.leak_check,
    );
    g.rt = Some(rt);
    for d in &m.data {
        let at = g.data.add(d, 16);
        g.data_off.push(at);
    }
    let mut syms = Vec::new();
    for (i, f) in m.funcs.iter().enumerate() {
        g.a.align(16);
        let start = g.a.here();
        g.function(i, f)?;
        syms.push(Symbol {
            name: f.name.clone(),
            offset: start as u64,
            size: (g.a.here() - start) as u64,
        });
    }
    elf::write_a64(&g.a, &g.ro, &g.data, 0, &syms)
}

/// The integer or boolean constants of a function, by value: they get no frame slot.
fn const_values(f: &Func) -> Vec<Option<i64>> {
    let mut out = vec![None; f.values.len()];
    for b in &f.blocks {
        for &i in &b.insts {
            if let (Op::Const(t, c), Some(r)) = (&f.insts[i].op, f.insts[i].result) {
                out[r.0 as usize] = match (c, t) {
                    (Const::Int(n), t) => Some(canonical(*n, bits(*t))),
                    (Const::Bool(b), _) => Some(i64::from(*b)),
                    _ => None,
                };
            }
        }
    }
    out
}

fn canonical(n: i128, w: u32) -> i64 {
    if w >= 64 {
        n as i64
    } else {
        (n as u128 & ((1u128 << w) - 1)) as i64
    }
}

fn bits(t: IrTy) -> u32 {
    match t {
        IrTy::Int(b) => b as u32,
        IrTy::Bool => 8,
        _ => 64,
    }
}

fn is_float(t: IrTy) -> bool {
    matches!(t, IrTy::F32 | IrTy::F64)
}

/// The size in bytes a value takes in memory.
fn mem_size(t: IrTy) -> u8 {
    match t {
        IrTy::Int(b) => (b / 8).max(1),
        IrTy::Bool => 1,
        IrTy::F32 => 4,
        IrTy::V2I64 => 16,
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

/// Where an argument or parameter travels.
enum Loc {
    Int(usize),
    Float(usize),
    Stack(usize),
}

fn classify(tys: &[IrTy]) -> Vec<Loc> {
    let (mut i, mut f, mut s) = (0, 0, 0);
    tys.iter()
        .map(|t| {
            if is_float(*t) {
                if f < MAX_FLOAT_REGS {
                    f += 1;
                    return Loc::Float(f - 1);
                }
            } else if i < INT_REGS.len() {
                i += 1;
                return Loc::Int(i - 1);
            }
            s += 1;
            Loc::Stack(s - 1)
        })
        .collect()
}

/// A branch condition: flags set by a compare, or a register to test against zero.
#[derive(Clone, Copy)]
enum Br {
    Flags(Cond),
    NonZero(Reg),
}

/// The shared exits of a function for the checks that stop the program.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Div0,
    Shift,
    Overflow,
    Unreachable,
}

const STOPS: [(Stop, &str); 4] = [
    (Stop::Div0, "division by zero"),
    (Stop::Shift, "shifted by the width or more"),
    (Stop::Overflow, "arithmetic overflow"),
    (Stop::Unreachable, "reached unreachable code"),
];

/// Out-of-line copies for one edge of a branch.
struct EdgeStub {
    label: Label,
    copies: Vec<Move>,
    to: usize,
}

/// A test that decides whether an overflow stops the program (`CheckedIf`), out of line.
struct CondStub {
    label: Label,
    resume: Label,
    test: Test,
}

/// One value moving into a block parameter.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Move {
    to: Value,
    from: Src,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Src {
    Val(Value),
    Imm(i64),
    /// The old value of a parameter, parked while a cycle of copies is undone.
    Temp,
}

struct Gen<'m> {
    m: &'m Module,
    a: Asm,
    ro: Blob,
    data: Blob,
    funcs: Vec<Label>,
    data_off: Vec<u32>,
    rt: Option<Runtime>,
    // ---- per function ----
    /// Frame offset (from `sp`) of each value's slot, or [`NONE`].
    off: Vec<i32>,
    imm: Vec<Option<i64>>,
    slot_off: HashMap<usize, i32>,
    frame: i32,
    block_labels: Vec<Label>,
    stops: [Option<Label>; 4],
    stubs: Vec<EdgeStub>,
    cond_stubs: Vec<CondStub>,
    /// Where `CheckedIf` resumes after the jump to its stub.
    resume: Option<Label>,
}

impl<'m> Gen<'m> {
    fn new(m: &'m Module) -> Gen<'m> {
        let mut a = Asm::new();
        let funcs = m.funcs.iter().map(|_| a.new_label()).collect();
        Gen {
            m,
            a,
            ro: Blob::default(),
            data: Blob::default(),
            funcs,
            data_off: Vec::new(),
            rt: None,
            off: Vec::new(),
            imm: Vec::new(),
            slot_off: HashMap::new(),
            frame: 0,
            block_labels: Vec::new(),
            stops: [None; 4],
            stubs: Vec::new(),
            cond_stubs: Vec::new(),
            resume: None,
        }
    }

    fn rt(&self) -> &Runtime {
        self.rt.as_ref().expect("the runtime is emitted first")
    }

    // ---- Frames ----------------------------------------------------------------------------------

    fn layout(&mut self, f: &Func) {
        self.imm = const_values(f);
        let uses = f.use_counts();
        // The outgoing argument area: the most stack arguments of any call.
        let mut stack_args = 0;
        for b in &f.blocks {
            for &i in &b.insts {
                let tys: Vec<IrTy> = match &f.insts[i].op {
                    Op::Call(_, args) | Op::CallPtr(_, _, _, args) => {
                        args.iter().map(|a| f.ty(*a)).collect()
                    }
                    _ => continue,
                };
                let n = classify(&tys)
                    .iter()
                    .filter(|l| matches!(l, Loc::Stack(_)))
                    .count();
                stack_args = stack_args.max(n);
            }
        }
        let mut at = (stack_args as i32 * 8 + 15) / 16 * 16;
        self.off.clear();
        for (v, &ty) in f.values.iter().enumerate() {
            self.off.push(if self.imm[v].is_some() || uses[v] == 0 {
                NONE
            } else if ty == IrTy::V2I64 {
                at = (at + 15) / 16 * 16;
                at += 16;
                at - 16
            } else {
                at += 8;
                at - 8
            });
        }
        // Parameters nothing reads still arrive in registers: they need no slot, the entry code drops them.
        self.slot_off.clear();
        for b in &f.blocks {
            for &i in &b.insts {
                if let Op::Slot(sz, al) = f.insts[i].op {
                    let al = (al as i32).clamp(8, 16);
                    at = (at + al - 1) / al * al;
                    self.slot_off.insert(i, at);
                    at += sz as i32;
                }
            }
        }
        self.frame = (at + 15) / 16 * 16;
    }

    // ---- Loads and stores of values -----------------------------------------------------------------

    fn ld(&mut self, r: Reg, v: Value) {
        match self.imm[v.0 as usize] {
            Some(n) => self.a.mov_ri(r, n),
            None => {
                let off = self.off[v.0 as usize];
                debug_assert!(off != NONE, "value {} is read but has no slot", v.0);
                self.a.ldr(r, SP, off, 8);
            }
        }
    }

    /// Loads an integer, sign- or zero-extending it from its own width to 64 bits.
    fn ld_ext(&mut self, r: Reg, v: Value, f: &Func, signed: bool) {
        let t = f.ty(v);
        match self.imm[v.0 as usize] {
            Some(n) => self.a.mov_ri(r, widen(n, t, signed)),
            None => {
                self.ld(r, v);
                if signed && matches!(t, IrTy::Int(b) if b < 64) {
                    self.a.sxt(r, r, bits(t));
                }
            }
        }
    }

    fn st(&mut self, v: Value, r: Reg) {
        let off = self.off[v.0 as usize];
        if off != NONE {
            self.a.str(r, SP, off, 8);
        }
    }

    /// Loads a float into `x` (a 4- or 8-byte value).
    fn ldf(&mut self, x: V, v: Value, f: &Func) {
        let off = self.off[v.0 as usize];
        debug_assert!(off != NONE, "float {} is read but has no slot", v.0);
        self.a
            .ldr_f(x, SP, off, if f.ty(v) == IrTy::F32 { 4 } else { 8 });
    }

    fn stf(&mut self, v: Value, x: V, f: &Func) {
        let off = self.off[v.0 as usize];
        if off != NONE {
            self.a
                .str_f(x, SP, off, if f.ty(v) == IrTy::F32 { 4 } else { 8 });
        }
    }

    fn ldv(&mut self, x: V, v: Value) {
        let off = self.off[v.0 as usize];
        debug_assert!(off != NONE);
        self.a.ldr_f(x, SP, off, 16);
    }

    fn stv(&mut self, v: Value, x: V) {
        let off = self.off[v.0 as usize];
        if off != NONE {
            self.a.str_f(x, SP, off, 16);
        }
    }

    /// Cuts the register to `bits` (zero-extends); the canonical form of a narrow result.
    fn mask(&mut self, r: Reg, bits: u32) {
        if bits < 64 {
            self.a.uxt(r, r, bits);
        }
    }

    // ---- Functions -----------------------------------------------------------------------------------

    fn stop_label(&mut self, s: Stop) -> Label {
        let k = STOPS.iter().position(|(t, _)| *t == s).unwrap();
        match self.stops[k] {
            Some(l) => l,
            None => {
                let l = self.a.new_label();
                self.stops[k] = Some(l);
                l
            }
        }
    }

    fn function(&mut self, fi: usize, f: &Func) -> Result<(), String> {
        self.layout(f);
        self.stops = [None; 4];
        self.block_labels = f.blocks.iter().map(|_| self.a.new_label()).collect();
        let l = self.funcs[fi];
        self.a.bind(l);
        self.a.stp_pre(FP, LR, SP, -16);
        self.a.mov_from_sp(FP);
        if self.frame > 0 {
            // A big frame must not step over the guard: touch each page, top down, first.
            let pages = self.frame / 4096;
            if pages <= 8 {
                for k in 1..=pages {
                    self.a.sub_ri(X16, SP, 4096 * k as u32);
                    self.a.ldr(XZR, X16, 0, 8);
                }
            } else {
                // A loop, not a probe per page in the code: a 40 MB array is 10 000 of them.
                let top = self.a.new_label();
                self.a.mov_from_sp(X16);
                self.a.add_imm(X17, SP, -(4096 * pages as i64));
                self.a.bind(top);
                self.a.sub_ri(X16, X16, 4096);
                self.a.ldr(XZR, X16, 0, 8);
                self.a.cmp_rr(X16, X17);
                self.a.b_cond(Cond::Hi, top);
            }
            self.a.add_imm(SP, SP, -(self.frame as i64));
        }
        // Parameters: registers or the caller's stack.
        let entry = f.entry();
        let params: Vec<Value> = f.block_params(entry).to_vec();
        let tys: Vec<IrTy> = params.iter().map(|p| f.ty(*p)).collect();
        for (p, loc) in params.iter().zip(classify(&tys)) {
            let off = self.off[p.0 as usize];
            if off == NONE {
                continue;
            }
            match loc {
                Loc::Int(i) => self.a.str(INT_REGS[i], SP, off, 8),
                Loc::Float(i) => {
                    let size = if f.ty(*p) == IrTy::F32 { 4 } else { 8 };
                    self.a.str_f(V(i as u8), SP, off, size);
                }
                Loc::Stack(k) => {
                    self.a.ldr(X9, FP, 16 + 8 * k as i32, 8);
                    self.a.str(X9, SP, off, 8);
                }
            }
        }
        let uses = f.use_counts();
        for (bi, b) in f.blocks.iter().enumerate() {
            let lb = self.block_labels[bi];
            self.a.bind(lb);
            // A comparison that only feeds the branch ending its block is not stored: the branch
            // tests the flags directly.
            let mut fused = None;
            if let Some(last) = super::regs::fused_cmp(f, bi, &uses) {
                if let Op::Cmp(op, x, y) = &f.insts[last].op {
                    fused = Some((last, *op, *x, *y));
                }
            }
            for &i in &b.insts {
                if fused.is_some_and(|(l, ..)| l == i) {
                    continue;
                }
                self.inst(f, i, &f.insts[i])?;
            }
            let next = (bi + 1 < f.blocks.len()).then_some(bi + 1);
            match &b.term {
                Some(t) => self.term(f, t, fused, next)?,
                None => return Err(format!("`{}`: a block has no terminator", f.name)),
            }
        }
        // The out-of-line copies of the branches.
        for EdgeStub { label, copies, to } in std::mem::take(&mut self.stubs) {
            self.a.bind(label);
            self.parallel(f, copies);
            let l = self.block_labels[to];
            self.a.b(l);
        }
        // The tests of the `CheckedIf`s. They keep every register the template uses.
        for CondStub {
            label,
            resume,
            test,
        } in std::mem::take(&mut self.cond_stubs)
        {
            self.a.bind(label);
            let stop = self.stop_label(Stop::Overflow);
            match test {
                Test::Bool(cond, when) => {
                    self.ld(X14, cond);
                    if when {
                        self.a.cbz(X14, resume);
                    } else {
                        self.a.cbnz(X14, resume);
                    }
                }
                Test::Cmp(op, x, y) => {
                    let cc = self.int_cmp_in(f, op, x, y, X14, X15);
                    self.a.b_cond(cc.invert(), resume);
                }
            }
            self.a.b(stop);
        }
        // The shared exits.
        for (k, (_, msg)) in STOPS.iter().enumerate() {
            if let Some(l) = self.stops[k] {
                let off = self.ro.add(msg.as_bytes(), 1);
                self.a.bind(l);
                self.a.lea_sect(X0, Sect::Rodata, off);
                self.a.mov_ri(X1, msg.len() as i64);
                let stop = self.rt().stop;
                self.a.bl(stop);
            }
        }
        Ok(())
    }

    fn epilogue(&mut self) {
        if self.frame > 0 {
            self.a.mov_to_sp(FP);
        }
        self.a.ldp_post(FP, LR, SP, 16);
        self.a.ret();
    }

    // ---- Terminators ---------------------------------------------------------------------------------

    fn term(
        &mut self,
        f: &Func,
        t: &Term,
        fused: Option<(usize, CmpOp, Value, Value)>,
        next: Option<usize>,
    ) -> Result<(), String> {
        match t {
            Term::Return(None) => self.epilogue(),
            Term::Return(Some(v)) => {
                if is_float(f.ty(*v)) {
                    self.ldf(V(0), *v, f);
                } else {
                    self.ld(X0, *v);
                }
                self.epilogue();
            }
            Term::Jump(tg) => self.edge(f, tg, next),
            Term::Branch(c, t, e) => {
                let br = match fused {
                    Some((_, op, x, y)) if is_float(f.ty(x)) => {
                        Br::Flags(self.float_cmp(f, op, x, y))
                    }
                    Some((_, op, x, y)) => Br::Flags(self.int_cmp(f, op, x, y)),
                    None => {
                        self.ld(X9, *c);
                        Br::NonZero(X9)
                    }
                };
                let (mt, me) = (self.edge_copies(f, t), self.edge_copies(f, e));
                let (tb, eb) = (t.block.0 as usize, e.block.0 as usize);
                let (tl, el) = (self.block_labels[tb], self.block_labels[eb]);
                if mt.is_empty() && me.is_empty() {
                    if Some(tb) == next {
                        // Fall into the true side; jump away on the opposite condition.
                        self.jump_unless(br, el);
                    } else {
                        self.jump_if(br, tl);
                        if Some(eb) != next {
                            self.a.b(el);
                        }
                    }
                } else if Some(eb) == next {
                    // The false side's copies run in line; the true side's wait out of line.
                    let to = self.stub(mt, tb);
                    self.jump_if(br, to);
                    self.parallel(f, me);
                } else if Some(tb) == next {
                    let to = self.stub(me, eb);
                    self.jump_unless(br, to);
                    self.parallel(f, mt);
                } else {
                    let to = self.stub(mt, tb);
                    self.jump_if(br, to);
                    self.parallel(f, me);
                    self.a.b(el);
                }
            }
            Term::Stop(msg) => {
                let off = self.ro.add(msg.as_bytes(), 1);
                self.a.lea_sect(X0, Sect::Rodata, off);
                self.a.mov_ri(X1, msg.len() as i64);
                let stop = self.rt().stop;
                self.a.bl(stop);
            }
            Term::Unreachable => {
                let l = self.stop_label(Stop::Unreachable);
                self.a.b(l);
            }
        }
        Ok(())
    }

    fn jump_if(&mut self, br: Br, to: Label) {
        match br {
            Br::Flags(cc) => self.a.b_cond(cc, to),
            Br::NonZero(r) => self.a.cbnz(r, to),
        }
    }

    fn jump_unless(&mut self, br: Br, to: Label) {
        match br {
            Br::Flags(cc) => self.a.b_cond(cc.invert(), to),
            Br::NonZero(r) => self.a.cbz(r, to),
        }
    }

    fn stub(&mut self, copies: Vec<Move>, to: usize) -> Label {
        let label = self.a.new_label();
        self.stubs.push(EdgeStub { label, copies, to });
        label
    }

    /// The copies an edge makes: the target's parameters from the arguments (those nothing reads, and
    /// those already in place, need none).
    fn edge_copies(&self, f: &Func, tg: &JumpTarget) -> Vec<Move> {
        f.block_params(tg.block)
            .iter()
            .zip(&tg.args)
            .filter(|(p, a)| p != a && self.off[p.0 as usize] != NONE)
            .map(|(p, a)| Move {
                to: *p,
                from: match self.imm[a.0 as usize] {
                    Some(n) => Src::Imm(n),
                    None => Src::Val(*a),
                },
            })
            .collect()
    }

    /// A jump to a block with the copies it needs, falling through when the block comes next.
    fn edge(&mut self, f: &Func, tg: &JumpTarget, next: Option<usize>) {
        let copies = self.edge_copies(f, tg);
        self.parallel(f, copies);
        let to = tg.block.0 as usize;
        if Some(to) != next {
            let l = self.block_labels[to];
            self.a.b(l);
        }
    }

    /// Makes all the copies as if at once: a copy waits while a pending copy still reads its target,
    /// and a cycle (a swap) is undone by parking one old value in `x13` or `v1`.
    fn parallel(&mut self, f: &Func, mut pending: Vec<Move>) {
        while !pending.is_empty() {
            let free = (0..pending.len()).find(|&i| {
                !pending
                    .iter()
                    .enumerate()
                    .any(|(j, c)| j != i && c.from == Src::Val(pending[i].to))
            });
            match free {
                Some(i) => {
                    let c = pending.remove(i);
                    self.copy(f, c);
                }
                None => {
                    // Every target is still wanted by another copy: park the first target's value.
                    let victim = pending[0].to;
                    if f.ty(victim) == IrTy::V2I64 {
                        self.ldv(V(1), victim);
                    } else {
                        self.ld(X13, victim);
                    }
                    for c in pending.iter_mut() {
                        if c.from == Src::Val(victim) {
                            c.from = Src::Temp;
                        }
                    }
                }
            }
        }
    }

    fn copy(&mut self, f: &Func, c: Move) {
        let to = c.to;
        if f.ty(to) == IrTy::V2I64 {
            match c.from {
                Src::Val(v) => self.ldv(V(0), v),
                Src::Temp => self.a.vmov(V(0), V(1)),
                Src::Imm(_) => unreachable!("a vector constant"),
            }
            self.stv(to, V(0));
            return;
        }
        match c.from {
            Src::Val(v) => self.ld(X9, v),
            Src::Imm(n) => self.a.mov_ri(X9, n),
            Src::Temp => self.a.mov_rr(X9, X13),
        }
        self.st(to, X9);
    }

    // ---- Instructions --------------------------------------------------------------------------------

    fn inst(&mut self, f: &Func, id: usize, inst: &Inst) -> Result<(), String> {
        let res = inst.result;
        match &inst.op {
            Op::Const(..) if res.is_some_and(|r| self.imm[r.0 as usize].is_some()) => {}
            Op::Const(t, c) => {
                let bits_v: i64 = match (c, t) {
                    (Const::Int(n), t) => canonical(*n, bits(*t)),
                    (Const::Bool(b), _) => i64::from(*b),
                    (Const::Float(x), IrTy::F32) => (*x as f32).to_bits() as i64,
                    (Const::Float(x), _) => x.to_bits() as i64,
                };
                self.a.mov_ri(X9, bits_v);
                self.st(res.unwrap(), X9);
            }
            Op::Bin(op, a, b) => self.bin(f, *op, *a, *b, res.unwrap())?,
            Op::Checked(op, signed, a, b) => {
                let ov = self.stop_label(Stop::Overflow);
                self.checked_to(f, *op, *signed, *a, *b, res.unwrap(), ov);
            }
            Op::CheckedIf(op, signed, test, a, b) => {
                let (label, resume) = (self.a.new_label(), self.a.new_label());
                self.resume = Some(resume);
                self.checked_to(f, *op, *signed, *a, *b, res.unwrap(), label);
                self.cond_stubs.push(CondStub {
                    label,
                    resume,
                    test: *test,
                });
            }
            Op::Select(c, a, b) => {
                self.ld(X9, *a);
                self.ld(X10, *b);
                self.ld(X11, *c);
                self.a.cmp_ri(X11, 0);
                self.a.csel(X9, X9, X10, Cond::Ne);
                self.st(res.unwrap(), X9);
            }
            Op::Cmp(op, a, b) => self.cmp(f, *op, *a, *b, res.unwrap()),
            Op::Neg(a) => {
                let t = f.ty(*a);
                let r = res.unwrap();
                if is_float(t) {
                    self.ldf(V(0), *a, f);
                    self.a.fneg(t == IrTy::F64, V(0), V(0));
                    self.stf(r, V(0), f);
                } else {
                    self.ld(X9, *a);
                    self.a.neg(X9, X9);
                    self.mask(X9, bits(t));
                    self.st(r, X9);
                }
            }
            Op::Not(a) => {
                let t = f.ty(*a);
                self.ld(X9, *a);
                if t == IrTy::Bool {
                    self.a.eor_ri(X9, X9, 1);
                } else {
                    self.a.mvn(X9, X9);
                    self.mask(X9, bits(t));
                }
                self.st(res.unwrap(), X9);
            }
            Op::Cast(k, a, to) => self.cast(f, *k, *a, *to, res.unwrap()),
            Op::Call(callee, args) => {
                let l = self.funcs[callee.0 as usize];
                self.call(f, args, None, Some(l), res, callee_ret(self.m, *callee));
            }
            Op::CallPtr(_, ret, fp, args) => self.call(f, args, Some(*fp), None, res, *ret),
            Op::Slot(sz, _) => {
                // Memory is zero when a slot comes into use, as in the interpreter.
                let off = self.slot_off[&id];
                self.zero_frame(off, *sz as i32);
                self.a.add_imm(X9, SP, off as i64);
                self.st(res.unwrap(), X9);
            }
            Op::PtrAdd(p, n) => self.bin(f, BinOp::Add, *p, *n, res.unwrap())?,
            Op::MemCopy(d, s, n) => {
                if let Some(len) = self.imm[n.0 as usize].filter(|n| (0..=SMALL_COPY).contains(n)) {
                    self.ld(X9, *d);
                    self.ld(X10, *s);
                    let mut k = 0;
                    for w in [8, 4, 2, 1] {
                        while len - k >= w {
                            self.a.ldr(X11, X10, k as i32, w as u8);
                            self.a.str(X11, X9, k as i32, w as u8);
                            k += w;
                        }
                    }
                    return Ok(());
                }
                self.ld(X0, *d);
                self.ld(X1, *s);
                self.ld(X2, *n);
                let l = self.rt().memcpy;
                self.a.bl(l);
            }
            Op::DataAddr(i) => {
                let off = self.data_off[*i as usize];
                self.a.lea_sect(X9, Sect::Data, off);
                self.st(res.unwrap(), X9);
            }
            Op::FuncAddr(fid) => {
                let l = self.funcs[fid.0 as usize];
                self.a.lea_label(X9, l);
                self.st(res.unwrap(), X9);
            }
            Op::Print(p, n) => {
                self.ld(X0, *p);
                self.ld(X1, *n);
                let l = self.rt().print;
                self.a.bl(l);
            }
            Op::VUn(o, a) => {
                let r = res.unwrap();
                match o {
                    VUn::ReduceAdd => {
                        self.ldv(V(0), *a);
                        self.a.vaddp(V(0), V(0));
                        self.a.fmov_to_gp(true, X9, V(0));
                        self.st(r, X9);
                    }
                    VUn::Splat => {
                        self.ld(X9, *a);
                        self.a.vdup(V(0), X9);
                        self.stv(r, V(0));
                    }
                    VUn::Shl(k) | VUn::UShr(k) => {
                        self.ldv(V(0), *a);
                        if matches!(o, VUn::Shl(_)) {
                            self.a.vshl(V(0), V(0), *k as u32);
                        } else {
                            self.a.vushr(V(0), V(0), *k as u32);
                        }
                        self.stv(r, V(0));
                    }
                }
            }
            Op::VBin(o, a, b) => {
                let r = res.unwrap();
                if *o == VBin::Build {
                    self.ld(X9, *a);
                    self.ld(X10, *b);
                    self.a.vins(V(0), 0, X9);
                    self.a.vins(V(0), 1, X10);
                } else {
                    self.ldv(V(0), *a);
                    self.ldv(V(1), *b);
                    match o {
                        VBin::Add => self.a.vadd(V(0), V(0), V(1)),
                        VBin::Sub => self.a.vsub(V(0), V(0), V(1)),
                        VBin::And => self.a.vand(V(0), V(0), V(1)),
                        VBin::Or => self.a.vorr(V(0), V(0), V(1)),
                        _ => self.a.veor(V(0), V(0), V(1)),
                    }
                }
                self.stv(r, V(0));
            }
            Op::Load(t, p) => {
                self.ld(X9, *p);
                let r = res.unwrap();
                match t {
                    IrTy::V2I64 => {
                        self.a.ldr_f(V(0), X9, 0, 16);
                        self.stv(r, V(0));
                    }
                    IrTy::F32 | IrTy::F64 => {
                        self.a.ldr_f(V(0), X9, 0, mem_size(*t));
                        self.stf(r, V(0), f);
                    }
                    _ => {
                        self.a.ldr(X10, X9, 0, mem_size(*t));
                        self.st(r, X10);
                    }
                }
            }
            Op::Store(p, x) => {
                let t = f.ty(*x);
                self.ld(X9, *p);
                if t == IrTy::V2I64 {
                    self.ldv(V(0), *x);
                    self.a.str_f(V(0), X9, 0, 16);
                } else if is_float(t) {
                    self.ldf(V(0), *x, f);
                    self.a.str_f(V(0), X9, 0, mem_size(t));
                } else if self.imm[x.0 as usize] == Some(0) {
                    self.a.str(XZR, X9, 0, mem_size(t));
                } else {
                    self.ld(X10, *x);
                    self.a.str(X10, X9, 0, mem_size(t));
                }
            }
            Op::Alloc(n) => {
                self.ld(X0, *n);
                let l = self.rt().alloc;
                self.a.bl(l);
                self.st(res.unwrap(), X0);
            }
            Op::Free(p) => {
                self.ld(X0, *p);
                let l = self.rt().free;
                self.a.bl(l);
            }
            Op::Realloc(p, n) => {
                self.ld(X0, *p);
                self.ld(X1, *n);
                let l = self.rt().realloc;
                self.a.bl(l);
                self.st(res.unwrap(), X0);
            }
        }
        Ok(())
    }

    /// Fills `size` bytes of the frame, at `off` from `sp`, with zeros.
    fn zero_frame(&mut self, off: i32, size: i32) {
        self.a.add_imm(X11, SP, off as i64);
        let mut k = 0;
        if size <= 128 {
            while size - k >= 16 {
                self.a.stp(XZR, XZR, X11, k);
                k += 16;
            }
        } else {
            // A loop of 16 bytes at a time over the part that is a multiple of 16.
            let whole = size / 16 * 16;
            let top = self.a.new_label();
            self.a.mov_rr(X12, X11);
            self.a.add_imm(X13, X11, whole as i64);
            self.a.bind(top);
            self.a.stp(XZR, XZR, X12, 0);
            self.a.add_ri(X12, X12, 16);
            self.a.cmp_rr(X12, X13);
            self.a.b_cond(Cond::Lo, top);
            k = whole;
        }
        for w in [8, 4, 2, 1] {
            while size - k >= w {
                self.a.str(XZR, X11, k, w as u8);
                k += w;
            }
        }
    }

    /// A call: stack arguments, register arguments, the call, the result.
    fn call(
        &mut self,
        f: &Func,
        args: &[Value],
        fptr: Option<Value>,
        label: Option<Label>,
        res: Option<Value>,
        ret: IrTy,
    ) {
        let tys: Vec<IrTy> = args.iter().map(|a| f.ty(*a)).collect();
        let locs = classify(&tys);
        for (arg, loc) in args.iter().zip(&locs) {
            match loc {
                Loc::Stack(k) => {
                    self.ld(X9, *arg);
                    self.a.str(X9, SP, 8 * *k as i32, 8);
                }
                Loc::Int(_) | Loc::Float(_) => {}
            }
        }
        for (arg, loc) in args.iter().zip(&locs) {
            match loc {
                Loc::Int(i) => self.ld(INT_REGS[*i], *arg),
                Loc::Float(i) => self.ldf(V(*i as u8), *arg, f),
                Loc::Stack(_) => {}
            }
        }
        match (label, fptr) {
            (Some(l), _) => self.a.bl(l),
            (None, Some(p)) => {
                self.ld(X15, p);
                self.a.blr(X15);
            }
            (None, None) => unreachable!("a call without a target"),
        }
        if let Some(r) = res {
            if is_float(ret) {
                self.stf(r, V(0), f);
            } else {
                self.st(r, X0);
            }
        }
    }

    // ---- Arithmetic ----------------------------------------------------------------------------------

    fn bin(&mut self, f: &Func, op: BinOp, a: Value, b: Value, r: Value) -> Result<(), String> {
        let t = f.ty(a);
        if is_float(t) {
            let dbl = t == IrTy::F64;
            self.ldf(V(0), a, f);
            self.ldf(V(1), b, f);
            match op {
                BinOp::Add => self.a.fadd(dbl, V(0), V(0), V(1)),
                BinOp::Sub => self.a.fsub(dbl, V(0), V(0), V(1)),
                BinOp::Mul => self.a.fmul(dbl, V(0), V(0), V(1)),
                BinOp::Div => self.a.fdiv(dbl, V(0), V(0), V(1)),
                BinOp::Rem => {
                    // The routine works on doubles: a float is widened and rounded back, both exactly.
                    if !dbl {
                        self.a.fcvt(true, V(0), V(0));
                        self.a.fcvt(true, V(1), V(1));
                    }
                    let l = self.rt().fmod;
                    self.a.bl(l);
                    if !dbl {
                        self.a.fcvt(false, V(0), V(0));
                    }
                }
                _ => return Err("not a float operation".to_string()),
            }
            self.stf(r, V(0), f);
            return Ok(());
        }
        let w = bits(t);
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::And | BinOp::Or | BinOp::Xor => {
                self.ld(X9, a);
                match (self.imm[b.0 as usize], op) {
                    (Some(n), BinOp::Add | BinOp::Sub) if Asm::fits_addsub(n as u64) => {
                        if op == BinOp::Add {
                            self.a.add_ri(X9, X9, n as u32);
                        } else {
                            self.a.sub_ri(X9, X9, n as u32);
                        }
                    }
                    _ => {
                        self.ld(X10, b);
                        match op {
                            BinOp::Add => self.a.add_rr(X9, X9, X10),
                            BinOp::Sub => self.a.sub_rr(X9, X9, X10),
                            BinOp::Mul => self.a.mul(X9, X9, X10),
                            BinOp::And => self.a.and_rr(X9, X9, X10),
                            BinOp::Or => self.a.orr_rr(X9, X9, X10),
                            _ => self.a.eor_rr(X9, X9, X10),
                        }
                    }
                }
                if !matches!(op, BinOp::And | BinOp::Or | BinOp::Xor) {
                    self.mask(X9, w);
                }
                self.st(r, X9);
            }
            BinOp::Div | BinOp::Rem | BinOp::UDiv | BinOp::URem => {
                let signed = matches!(op, BinOp::Div | BinOp::Rem);
                let want_rem = matches!(op, BinOp::Rem | BinOp::URem);
                self.ld_ext(X9, a, f, signed);
                self.ld_ext(X10, b, f, signed);
                let z = self.stop_label(Stop::Div0);
                self.a.cbz(X10, z);
                // `MIN / -1` does not trap here: it wraps to `MIN` (and the remainder is 0), like the interpreter.
                if signed {
                    self.a.sdiv(X11, X9, X10);
                } else {
                    self.a.udiv(X11, X9, X10);
                }
                if want_rem {
                    self.a.msub(X11, X11, X10, X9);
                }
                self.mask(X11, w);
                self.st(r, X11);
            }
            BinOp::MulHi | BinOp::UMulHi => {
                let signed = op == BinOp::MulHi;
                self.ld_ext(X9, a, f, signed);
                self.ld_ext(X10, b, f, signed);
                if w == 64 {
                    if signed {
                        self.a.smulh(X11, X9, X10);
                    } else {
                        self.a.umulh(X11, X9, X10);
                    }
                } else {
                    self.a.mul(X11, X9, X10);
                    if signed {
                        self.a.asr_ri(X11, X11, w);
                    } else {
                        self.a.lsr_ri(X11, X11, w);
                    }
                    self.mask(X11, w);
                }
                self.st(r, X11);
            }
            BinOp::Shl | BinOp::Shr | BinOp::UShr => {
                if let Some(n) = self.imm[b.0 as usize].filter(|n| (0..w as i64).contains(n)) {
                    // A constant amount below the width: no check, an immediate shift.
                    self.ld_ext(X9, a, f, op == BinOp::Shr);
                    match op {
                        BinOp::Shl => self.a.lsl_ri(X9, X9, n as u32),
                        BinOp::Shr => self.a.asr_ri(X9, X9, n as u32),
                        _ => self.a.lsr_ri(X9, X9, n as u32),
                    }
                } else {
                    self.ld(X10, b);
                    self.a.cmp_ri(X10, w);
                    let s = self.stop_label(Stop::Shift);
                    self.a.b_cond(Cond::Hs, s);
                    self.ld_ext(X9, a, f, op == BinOp::Shr);
                    match op {
                        BinOp::Shl => self.a.lslv(X9, X9, X10),
                        BinOp::Shr => self.a.asrv(X9, X9, X10),
                        _ => self.a.lsrv(X9, X9, X10),
                    }
                }
                self.mask(X9, w);
                self.st(r, X9);
            }
        }
        Ok(())
    }

    /// A checked operation whose overflow jumps to `ov`. When `self.resume` is set it is bound right after
    /// the jump, so a stub that decides not to stop can come back.
    #[allow(clippy::too_many_arguments)]
    fn checked_to(
        &mut self,
        f: &Func,
        op: BinOp,
        signed: bool,
        a: Value,
        b: Value,
        r: Value,
        ov: Label,
    ) {
        let w = bits(f.ty(a));
        self.ld_ext(X9, a, f, signed);
        self.ld_ext(X10, b, f, signed);
        if w == 64 {
            match (op, signed) {
                (BinOp::Add, true) => {
                    self.a.adds_rr(X11, X9, X10);
                    self.ov_jump(Cond::Vs, ov);
                }
                (BinOp::Sub, true) => {
                    self.a.subs_rr(X11, X9, X10);
                    self.ov_jump(Cond::Vs, ov);
                }
                (BinOp::Add, false) => {
                    self.a.adds_rr(X11, X9, X10);
                    self.ov_jump(Cond::Hs, ov);
                }
                (BinOp::Sub, false) => {
                    self.a.subs_rr(X11, X9, X10);
                    // Borrow: the carry is clear.
                    self.ov_jump(Cond::Lo, ov);
                }
                (_, true) => {
                    // The product fits when the high half is the sign extension of the low half.
                    self.a.mul(X11, X9, X10);
                    self.a.smulh(X12, X9, X10);
                    self.a.cmp_rr_sh(X12, X11, Shift::Asr, 63);
                    self.ov_jump(Cond::Ne, ov);
                }
                (_, false) => {
                    self.a.mul(X11, X9, X10);
                    self.a.umulh(X12, X9, X10);
                    self.a.cmp_ri(X12, 0);
                    self.ov_jump(Cond::Ne, ov);
                }
            }
        } else {
            // Narrow: no overflow in 64 bits; the result must survive the trip through the width.
            match op {
                BinOp::Add => self.a.add_rr(X11, X9, X10),
                BinOp::Sub => self.a.sub_rr(X11, X9, X10),
                _ => self.a.mul(X11, X9, X10),
            }
            if signed {
                self.a.sxt(X12, X11, w);
            } else {
                self.a.uxt(X12, X11, w);
            }
            self.a.cmp_rr(X12, X11);
            self.ov_jump(Cond::Ne, ov);
            self.mask(X11, w);
        }
        self.st(r, X11);
    }

    fn ov_jump(&mut self, cc: Cond, ov: Label) {
        self.a.b_cond(cc, ov);
        if let Some(l) = self.resume.take() {
            self.a.bind(l);
        }
    }

    fn cmp(&mut self, f: &Func, op: CmpOp, a: Value, b: Value, r: Value) {
        let cc = if is_float(f.ty(a)) {
            self.float_cmp(f, op, a, b)
        } else {
            self.int_cmp(f, op, a, b)
        };
        self.a.cset(X9, cc);
        self.st(r, X9);
    }

    /// Compares two floats; gives the condition that holds when `op` is true. Unordered operands (a
    /// NaN) make every condition here false, except `!=`.
    fn float_cmp(&mut self, f: &Func, op: CmpOp, a: Value, b: Value) -> Cond {
        let d = f.ty(a) == IrTy::F64;
        self.ldf(V(0), a, f);
        self.ldf(V(1), b, f);
        self.a.fcmp(d, V(0), V(1));
        match op {
            CmpOp::Eq => Cond::Eq,
            CmpOp::Ne => Cond::Ne,
            CmpOp::Lt | CmpOp::ULt => Cond::Mi,
            CmpOp::Le | CmpOp::ULe => Cond::Ls,
            CmpOp::Gt | CmpOp::UGt => Cond::Gt,
            CmpOp::Ge | CmpOp::UGe => Cond::Ge,
        }
    }

    /// Compares two integer, boolean or pointer values; gives the condition that holds when `op` is true.
    fn int_cmp(&mut self, f: &Func, op: CmpOp, a: Value, b: Value) -> Cond {
        self.int_cmp_in(f, op, a, b, X9, X10)
    }

    fn int_cmp_in(&mut self, f: &Func, op: CmpOp, a: Value, b: Value, ra: Reg, rb: Reg) -> Cond {
        let signed = matches!(f.ty(a), IrTy::Int(_))
            && matches!(op, CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge);
        self.ld_ext(ra, a, f, signed);
        match self.imm[b.0 as usize].map(|n| widen(n, f.ty(b), signed)) {
            Some(n) if (0..4096).contains(&n) => self.a.cmp_ri(ra, n as u32),
            _ => {
                self.ld_ext(rb, b, f, signed);
                self.a.cmp_rr(ra, rb);
            }
        }
        match op {
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
        }
    }

    fn cast(&mut self, f: &Func, k: CastKind, a: Value, to: IrTy, r: Value) {
        let from = f.ty(a);
        match k {
            CastKind::ZExt | CastKind::Trunc => {
                self.ld(X9, a);
                self.mask(X9, bits(to));
                self.st(r, X9);
            }
            CastKind::SExt => {
                self.ld_ext(X9, a, f, true);
                self.mask(X9, bits(to));
                self.st(r, X9);
            }
            CastKind::PtrToInt | CastKind::IntToPtr => {
                self.ld(X9, a);
                self.st(r, X9);
            }
            CastKind::IntToFloat => {
                self.ld_ext(X9, a, f, true);
                self.a.scvtf(to == IrTy::F64, V(0), X9);
                self.stf(r, V(0), f);
            }
            CastKind::FloatToInt => {
                // `fcvtzs` saturates and turns NaN into 0, like Rust's `as`; then cut to the width.
                self.ldf(V(0), a, f);
                self.a.fcvtzs(from == IrTy::F64, X9, V(0));
                self.mask(X9, bits(to));
                self.st(r, X9);
            }
            CastKind::FloatResize => {
                self.ldf(V(0), a, f);
                if from != to {
                    self.a.fcvt(to == IrTy::F64, V(0), V(0));
                }
                self.stf(r, V(0), f);
            }
        }
    }
}

fn callee_ret(m: &Module, f: FuncId) -> IrTy {
    m.funcs[f.0 as usize].ret
}
