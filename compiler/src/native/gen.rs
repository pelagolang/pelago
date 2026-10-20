//! The simple code generator (stage N2): every IR value lives in its own 8-byte stack slot and
//! each operation is a fixed template over `rax rcx rdx` and `xmm0 xmm1`. Slow, small and easy to
//! trust; register allocation (N5) replaces it later without changing what the tests expect.
//!
//! Representation: an integer or boolean value is kept **zero-extended** to 64 bits in its slot
//! ("canonical form"), a pointer is 64 bits, a float is 4 or 8 bytes in the low part of the slot.
//! Operations that need the sign extend from the slot with `movsx`; results are cut back to their
//! width. The rules for division, shifts, overflow and NaN are in `docs/design/backend.md`.

use super::elf::{self, Blob, Symbol};
use super::rt::{self, Runtime};
use super::x86::*;
use super::{pe, Os};
use crate::ir::{
    BinOp, CastKind, CmpOp, Const, Func, FuncId, Inst, IrTy, Module, Op, Target, Term, Value,
};
use std::collections::{HashMap, HashSet};

const INT_REGS: [Reg; 6] = [RDI, RSI, RDX, RCX, R8, R9];
const MAX_FLOAT_REGS: usize = 8;

// Tests can shrink the register set (the number of registers of each kind the allocator may use) to
// push the allocator through its spilling and splitting paths with ordinary programs.
#[cfg(test)]
thread_local! {
    pub static REG_LIMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

fn reg_limit() -> usize {
    #[cfg(test)]
    {
        REG_LIMIT.with(|l| l.get())
    }
    #[cfg(not(test))]
    {
        usize::MAX
    }
}

/// What to build.
pub struct Options {
    /// Report leaked blocks at exit (status 3): for tests.
    pub leak_check: bool,
    pub target: Os,
    /// Keep the most used integer values in registers (stage N5) instead of all in the frame.
    pub regalloc: bool,
}

/// The registers values can live in: callee-saved, so calls leave them alone.
const VALUE_REGS: [Reg; 5] = [RBX, R12, R13, R14, R15];

/// Caller-saved registers the templates leave alone (`rax rcx rdx` are their scratch registers), in the
/// order the allocator tries them. A value that lives across a call in one of them is stored before
/// the call and loaded after it. The argument registers `rsi rdi r8 r9` are written by a parallel
/// move whenever a call, a parameter list or a jump needs them.
const SCRATCH_REGS: [Reg; 6] = [R10, R11, R8, R9, RSI, RDI];

/// Where a value lives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Where {
    Mem(i32),
    Reg(Reg),
    /// A constant integer, boolean or pointer: no storage, loaded or used as an immediate where it is read.
    Imm(i64),
    /// A value nothing reads (or a compare fused into its branch): writes to it are dropped.
    Dead,
    /// Only inside [`Gen::moves`]: the value pushed on the stack to break a cycle of copies.
    Stack,
}

/// Compiles the module to an executable whose entry runs the function named `entry`.
pub fn compile(m: &Module, entry: &str, opts: &Options) -> Result<Vec<u8>, String> {
    let main_fn = m
        .funcs
        .iter()
        .position(|f| f.name == entry)
        .ok_or_else(|| format!("no function `{entry}`"))?;
    let mut g = Gen::new(m);
    g.regalloc = opts.regalloc;
    let main_label = g.funcs[main_fn];
    let rt = rt::emit(
        &mut g.a,
        &mut g.ro,
        &mut g.data,
        main_label,
        opts.leak_check,
        opts.target,
    );
    g.rt = Some(rt);
    g.place_data();
    g.stop_stubs();
    let mut syms = Vec::new();
    for (i, f) in m.funcs.iter().enumerate() {
        let start = g.a.here();
        g.function(i, f)?;
        syms.push(Symbol {
            name: f.name.clone(),
            offset: start as u64,
            size: (g.a.here() - start) as u64,
        });
    }
    match opts.target {
        Os::Linux => elf::write(&g.a, &g.ro, &g.data, 0, &syms),
        Os::Windows => pe::write(&g.a, &g.ro, &g.data, 0),
    }
}

/// Out-of-line copies for one edge of a branch: where they start, the copies `(to, from)` and the block
/// to jump to afterwards.
struct Stub {
    label: Label,
    copies: Vec<(Where, Where)>,
    to: usize,
}

struct Gen<'m> {
    m: &'m Module,
    a: Asm,
    ro: Blob,
    data: Blob,
    funcs: Vec<Label>,
    data_off: Vec<u32>,
    rt: Option<Runtime>,
    stop_div0: Label,
    stop_shift: Label,
    stop_overflow: Label,
    stop_unreachable: Label,
    regalloc: bool,
    /// Per function: where each value lives, the frame offset of each `Slot`, and where the
    /// callee-saved registers in use are kept.
    loc: Vec<Where>,
    saves: Vec<(Reg, i32)>,
    /// Per call-like instruction: the values in caller-saved registers that live across it, as the
    /// register and the frame offset where the value waits during the operation.
    across: HashMap<usize, Vec<(Reg, i32)>>,
    /// Frame offsets already written in the current block (a value is stored once per block).
    stored: HashSet<i32>,
    /// Copies of block parameters kept out of line: label, the copies, and the block to jump to.
    stubs: Vec<Stub>,
    slot_off: HashMap<usize, i32>,
    frame: i32,
    block_labels: Vec<Label>,
    ret_ty: IrTy,
}

/// The integer or boolean constants of a function, by value: they get no register and no frame slot.
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

/// The opposite condition (x86 pairs them in the lowest bit of the code).
fn invert(cc: Cc) -> Cc {
    match cc {
        Cc::O => Cc::No,
        Cc::No => Cc::O,
        Cc::B => Cc::Ae,
        Cc::Ae => Cc::B,
        Cc::E => Cc::Ne,
        Cc::Ne => Cc::E,
        Cc::Be => Cc::A,
        Cc::A => Cc::Be,
        Cc::S => Cc::Ns,
        Cc::Ns => Cc::S,
        Cc::P => Cc::Np,
        Cc::Np => Cc::P,
        Cc::L => Cc::Ge,
        Cc::Ge => Cc::L,
        Cc::Le => Cc::G,
        Cc::G => Cc::Le,
    }
}

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
        IrTy::F32 => 4,
        _ => 8,
    }
}

fn is_float(t: IrTy) -> bool {
    matches!(t, IrTy::F32 | IrTy::F64)
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

impl<'m> Gen<'m> {
    fn new(m: &'m Module) -> Gen<'m> {
        let mut a = Asm::new();
        let funcs = m.funcs.iter().map(|_| a.new_label()).collect();
        let (d, s, o, u) = (a.new_label(), a.new_label(), a.new_label(), a.new_label());
        Gen {
            m,
            a,
            ro: Blob::default(),
            data: Blob::default(),
            funcs,
            data_off: Vec::new(),
            rt: None,
            stop_div0: d,
            stop_shift: s,
            stop_overflow: o,
            stop_unreachable: u,
            regalloc: true,
            loc: Vec::new(),
            saves: Vec::new(),
            across: HashMap::new(),
            stored: HashSet::new(),
            stubs: Vec::new(),
            slot_off: HashMap::new(),
            frame: 0,
            block_labels: Vec::new(),
            ret_ty: IrTy::Void,
        }
    }

    fn rt(&self) -> &Runtime {
        self.rt.as_ref().expect("the runtime is emitted first")
    }

    /// All static data goes into `.data` (the lowering writes to some tables at run time).
    fn place_data(&mut self) {
        for d in &self.m.data {
            let at = self.data.add(d, 16);
            self.data_off.push(at);
        }
    }

    /// The shared exits for the checks that stop the program.
    fn stop_stubs(&mut self) {
        // Jumped over by the start code: they are only reached by `jcc`.
        for (label, msg) in [
            (self.stop_div0, "division by zero"),
            (self.stop_shift, "shifted by the width or more"),
            (self.stop_overflow, "arithmetic overflow"),
            (self.stop_unreachable, "reached unreachable code"),
        ] {
            let off = self.ro.add(msg.as_bytes(), 1);
            self.a.bind(label);
            self.a.lea_sect(RDI, Sect::Rodata, off);
            self.a.mov_ri(RSI, msg.len() as i64);
            let stop = self.rt().stop;
            self.a.call(stop);
        }
    }

    // ---- Frames --------------------------------------------------------------------------------------

    /// The frame address of a value that lives in memory (floats always do).
    fn vmem(&self, v: Value) -> Mem {
        match self.loc[v.0 as usize] {
            Where::Mem(off) => mem(RBP, off),
            w => panic!("value {} is in {w:?}, not in memory", v.0),
        }
    }

    fn layout(&mut self, f: &Func) {
        self.slot_off.clear();
        let imms = const_values(f);
        let uses = f.use_counts();
        let alloc = if self.regalloc {
            let n = reg_limit();
            let saved: Vec<Reg> = VALUE_REGS.iter().copied().take(n).collect();
            let scratch: Vec<Reg> = SCRATCH_REGS.iter().copied().take(n).collect();
            super::regs::assign(f, &saved, &scratch, &INT_REGS)
        } else {
            super::regs::Alloc {
                reg: vec![None; f.values.len()],
                across: HashMap::new(),
            }
        };
        // Compares that only feed their branch are never stored.
        let mut fused = vec![false; f.values.len()];
        for bi in 0..f.blocks.len() {
            if let Some(i) = super::regs::fused_cmp(f, bi, &uses) {
                fused[f.insts[i].result.unwrap().0 as usize] = true;
            }
        }
        let mut at = 0i32;
        self.loc.clear();
        for (v, r) in alloc.reg.iter().enumerate() {
            self.loc.push(match (imms[v], r) {
                (Some(n), _) => Where::Imm(n),
                _ if fused[v] || (uses[v] == 0 && !is_float(f.values[v])) => Where::Dead,
                (None, Some(r)) => Where::Reg(*r),
                (None, None) => {
                    at += 8;
                    Where::Mem(-at)
                }
            });
        }
        // Room to keep the callee-saved registers this function uses.
        self.saves.clear();
        for r in VALUE_REGS {
            if alloc.reg.contains(&Some(r)) {
                at += 8;
                self.saves.push((r, -at));
            }
        }
        // Where the values in caller-saved registers wait while a call runs.
        self.across.clear();
        let mut home: HashMap<Value, i32> = HashMap::new();
        let mut ids: Vec<_> = alloc.across.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let mut list = Vec::new();
            for v in &alloc.across[&id] {
                let off = *home.entry(*v).or_insert_with(|| {
                    at += 8;
                    -at
                });
                let reg = alloc.reg[v.0 as usize].expect("a split value has a register");
                list.push((reg, off));
            }
            self.across.insert(id, list);
        }
        for b in &f.blocks {
            for &i in &b.insts {
                if let Op::Slot(sz, al) = f.insts[i].op {
                    let al = (al as i32).max(1);
                    at = (at + sz as i32 + al - 1) / al * al;
                    self.slot_off.insert(i, -at);
                }
            }
        }
        self.frame = (at + 15) / 16 * 16;
    }

    // ---- Loads and stores of values -------------------------------------------------------------------

    fn ld(&mut self, r: Reg, v: Value) {
        match self.loc[v.0 as usize] {
            Where::Reg(x) => {
                if x != r {
                    self.a.mov_rr(r, x, 8);
                }
            }
            Where::Mem(off) => self.a.load(r, mem(RBP, off), 8),
            Where::Imm(n) => self.a.mov_ri(r, n),
            w => panic!("value {} is read from {w:?}", v.0),
        }
    }

    /// The constant a value is, if it is one that fits an immediate operand once widened (signed or not).
    fn imm32(&self, v: Value, f: &Func, signed: bool) -> Option<i32> {
        let Where::Imm(n) = self.loc[v.0 as usize] else {
            return None;
        };
        i32::try_from(widen(n, f.ty(v), signed)).ok()
    }

    /// The value widened to 64 bits with its sign or with zeros.
    fn ld_ext(&mut self, r: Reg, v: Value, f: &Func, signed: bool) {
        let t = f.ty(v);
        match self.loc[v.0 as usize] {
            // A register holds the zero-extended value.
            Where::Reg(x) => {
                if signed && size(t) < 8 {
                    self.a.extend_rr(r, x, size(t), true, true);
                } else if x != r {
                    self.a.mov_rr(r, x, 8);
                }
            }
            Where::Mem(off) => self.a.load_ext(r, mem(RBP, off), size(t), signed, true),
            Where::Imm(n) => self.a.mov_ri(r, widen(n, t, signed)),
            w => panic!("value {} is read from {w:?}", v.0),
        }
    }

    fn st(&mut self, v: Value, r: Reg) {
        match self.loc[v.0 as usize] {
            Where::Reg(x) => {
                if x != r {
                    self.a.mov_rr(x, r, 8);
                }
            }
            Where::Mem(off) => self.a.store(mem(RBP, off), r, 8),
            Where::Imm(_) => panic!("a constant is never written"),
            Where::Dead => {}
            Where::Stack => unreachable!(),
        }
    }

    /// The register a result is built in: its own when it has one, `rax` otherwise.
    fn dst(&self, v: Value) -> Reg {
        match self.loc[v.0 as usize] {
            Where::Reg(x) => x,
            _ => RAX,
        }
    }

    /// A register holding the operand (widened as asked): its own when it lives in one and needs no
    /// widening, else `scratch` after loading it there.
    fn src(&mut self, v: Value, f: &Func, signed: bool, scratch: Reg) -> Reg {
        if let Where::Reg(x) = self.loc[v.0 as usize] {
            if !(signed && size(f.ty(v)) < 8) {
                return x;
            }
        }
        self.ld_ext(scratch, v, f, signed);
        scratch
    }

    fn ldf(&mut self, x: Xmm, v: Value, f: &Func) {
        let m = self.vmem(v);
        self.a.fload(x, m, f.ty(v) == IrTy::F64);
    }

    fn stf(&mut self, v: Value, x: Xmm, f: &Func) {
        let m = self.vmem(v);
        self.a.fstore(m, x, f.ty(v) == IrTy::F64);
    }

    /// Cuts a 64-bit register down to `bits`, zero-extended again.
    fn mask(&mut self, r: Reg, bits: u32) {
        match bits {
            8 => self.a.extend_rr(r, r, 1, false, false),
            16 => self.a.extend_rr(r, r, 2, false, false),
            32 => self.a.mov_rr(r, r, 4),
            _ => {}
        }
    }

    // ---- Functions -----------------------------------------------------------------------------------

    fn function(&mut self, fi: usize, f: &Func) -> Result<(), String> {
        self.layout(f);
        self.ret_ty = f.ret;
        self.block_labels = f.blocks.iter().map(|_| self.a.new_label()).collect();
        let l = self.funcs[fi];
        self.a.bind(l);
        self.a.push(RBP);
        self.a.mov_rr(RBP, RSP, 8);
        if self.frame > 0 {
            self.a.alu_ri(Alu::Sub, RSP, self.frame, 8);
        }
        // Parameters: registers or the caller's stack.
        let entry = f.entry();
        let params: Vec<Value> = f.block_params(entry).to_vec();
        let tys: Vec<IrTy> = params.iter().map(|p| f.ty(*p)).collect();
        // Keep the callee-saved registers this function uses.
        for (r, off) in self.saves.clone() {
            self.a.store(mem(RBP, off), r, 8);
        }
        let mut copies = Vec::new();
        for (p, loc) in params.iter().zip(classify(&tys)) {
            match loc {
                Loc::Int(i) => copies.push((self.loc[p.0 as usize], Where::Reg(INT_REGS[i]))),
                Loc::Float(i) => {
                    let m = self.vmem(*p);
                    self.a.fstore(m, Xmm(i as u8), true);
                }
                Loc::Stack(k) => {
                    copies.push((self.loc[p.0 as usize], Where::Mem(16 + 8 * k as i32)))
                }
            }
        }
        self.moves(&copies);
        let uses = f.use_counts();
        for (bi, b) in f.blocks.iter().enumerate() {
            let lb = self.block_labels[bi];
            self.a.bind(lb);
            self.stored.clear();
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
        for Stub { label, copies, to } in std::mem::take(&mut self.stubs) {
            self.a.bind(label);
            self.moves(&copies);
            let l = self.block_labels[to];
            self.a.jmp(l);
        }
        Ok(())
    }

    fn epilogue(&mut self) {
        for (r, off) in self.saves.clone() {
            self.a.load(r, mem(RBP, off), 8);
        }
        self.a.mov_rr(RSP, RBP, 8);
        self.a.pop(RBP);
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
                    self.ldf(Xmm(0), *v, f);
                } else {
                    self.ld(RAX, *v);
                }
                self.epilogue();
            }
            Term::Jump(tg) => self.edge(f, tg, next),
            Term::Branch(c, t, e) => {
                let cc = match fused {
                    Some((_, op, x, y)) => self.int_cmp(f, op, x, y),
                    None => {
                        let rc = self.src(*c, f, false, RAX);
                        self.a.test_rr(rc, rc, 1);
                        Cc::Ne
                    }
                };
                let (mt, me) = (self.edge_copies(f, t), self.edge_copies(f, e));
                let (tb, eb) = (t.block.0 as usize, e.block.0 as usize);
                let (tl, el) = (self.block_labels[tb], self.block_labels[eb]);
                if mt.is_empty() && me.is_empty() {
                    if Some(tb) == next {
                        // Fall into the true side; jump away on the opposite condition.
                        self.a.jcc(invert(cc), el);
                    } else {
                        self.a.jcc(cc, tl);
                        if Some(eb) != next {
                            self.a.jmp(el);
                        }
                    }
                } else if Some(eb) == next {
                    // The false side's copies run in line; the true side's wait out of line.
                    let to = self.stub(mt, tb, tl);
                    self.a.jcc(cc, to);
                    self.moves(&me);
                } else if Some(tb) == next {
                    let to = self.stub(me, eb, el);
                    self.a.jcc(invert(cc), to);
                    self.moves(&mt);
                } else {
                    let to = self.stub(mt, tb, tl);
                    self.a.jcc(cc, to);
                    self.moves(&me);
                    self.a.jmp(el);
                }
            }
            Term::Stop(msg) => {
                let off = self.ro.add(msg.as_bytes(), 1);
                self.a.lea_sect(RDI, Sect::Rodata, off);
                self.a.mov_ri(RSI, msg.len() as i64);
                let stop = self.rt().stop;
                self.a.call(stop);
            }
            Term::Unreachable => {
                let l = self.stop_unreachable;
                self.a.jmp(l);
            }
        }
        Ok(())
    }

    /// The copies an edge makes: the target's parameters from the arguments (those nothing reads, and
    /// those already in place, need none).
    fn edge_copies(&self, f: &Func, tg: &Target) -> Vec<(Where, Where)> {
        f.block_params(tg.block)
            .iter()
            .zip(&tg.args)
            .map(|(p, a)| (self.loc[p.0 as usize], self.loc[a.0 as usize]))
            .filter(|(d, s)| d != s && *d != Where::Dead)
            .collect()
    }

    /// The label to jump to for an edge: the block itself when it needs no copies, else a stub placed
    /// after the function that makes them and jumps on.
    fn stub(&mut self, copies: Vec<(Where, Where)>, to: usize, direct: Label) -> Label {
        if copies.is_empty() {
            return direct;
        }
        let l = self.a.new_label();
        self.stubs.push(Stub {
            label: l,
            copies,
            to,
        });
        l
    }

    /// Hands the arguments to the target's parameters and jumps.
    fn edge(&mut self, f: &Func, tg: &Target, next: Option<usize>) {
        let copies = self.edge_copies(f, tg);
        self.moves(&copies);
        if Some(tg.block.0 as usize) != next {
            let l = self.block_labels[tg.block.0 as usize];
            self.a.jmp(l);
        }
    }

    /// Makes all the copies `(to, from)` as if at once, so a place that is both written and read is
    /// read first. A cycle (a swap) goes through the stack. `rax` is the go-between for memory to
    /// memory; sources can be registers, frame slots and constants, targets registers and slots.
    fn moves(&mut self, copies: &[(Where, Where)]) {
        let mut pend: Vec<(Where, Where)> =
            copies.iter().copied().filter(|(d, s)| d != s).collect();
        while !pend.is_empty() {
            let ready = (0..pend.len()).find(|&i| {
                !pend
                    .iter()
                    .enumerate()
                    .any(|(j, m)| j != i && m.1 == pend[i].0)
            });
            match ready {
                Some(i) => {
                    let (d, s) = pend.remove(i);
                    self.copy(d, s);
                }
                None => {
                    // Every target is still wanted by another copy: park one on the stack.
                    let d0 = pend[0].0;
                    match d0 {
                        Where::Reg(r) => self.a.push(r),
                        Where::Mem(off) => {
                            self.a.load(RAX, mem(RBP, off), 8);
                            self.a.push(RAX);
                        }
                        w => unreachable!("{w:?} cannot be in a cycle"),
                    }
                    for m in pend.iter_mut().filter(|m| m.1 == d0) {
                        m.1 = Where::Stack;
                    }
                }
            }
        }
    }

    fn copy(&mut self, d: Where, s: Where) {
        match (d, s) {
            (Where::Dead, _) => {}
            (Where::Reg(r), Where::Reg(x)) => self.a.mov_rr(r, x, 8),
            (Where::Reg(r), Where::Mem(off)) => self.a.load(r, mem(RBP, off), 8),
            (Where::Reg(r), Where::Imm(n)) => self.a.mov_ri(r, n),
            (Where::Reg(r), Where::Stack) => self.a.pop(r),
            (Where::Mem(off), Where::Reg(x)) => self.a.store(mem(RBP, off), x, 8),
            (Where::Mem(off), Where::Imm(n)) if i32::try_from(n).is_ok() => {
                self.a.store_imm(mem(RBP, off), n as i32, 8)
            }
            (Where::Mem(off), s) => {
                match s {
                    Where::Mem(from) => self.a.load(RAX, mem(RBP, from), 8),
                    Where::Imm(n) => self.a.mov_ri(RAX, n),
                    Where::Stack => self.a.pop(RAX),
                    w => unreachable!("copy from {w:?}"),
                }
                self.a.store(mem(RBP, off), RAX, 8);
            }
            (d, s) => unreachable!("copy {s:?} to {d:?}"),
        }
    }

    // ---- Instructions --------------------------------------------------------------------------------

    fn inst(&mut self, f: &Func, id: usize, inst: &Inst) -> Result<(), String> {
        // Values waiting in caller-saved registers go to their frame slot for the length of a call.
        let keep = self.across.get(&id).cloned().unwrap_or_default();
        for &(r, off) in &keep {
            if self.stored.insert(off) {
                self.a.store(mem(RBP, off), r, 8);
            }
        }
        self.inst_body(f, id, inst)?;
        for &(r, off) in &keep {
            self.a.load(r, mem(RBP, off), 8);
        }
        Ok(())
    }

    fn inst_body(&mut self, f: &Func, id: usize, inst: &Inst) -> Result<(), String> {
        let res = inst.result;
        match &inst.op {
            Op::Const(..) if matches!(self.loc[res.unwrap().0 as usize], Where::Imm(_)) => {}
            Op::Const(t, c) => {
                let bits_v: i64 = match (c, t) {
                    (Const::Int(n), t) => {
                        let b = bits(*t);
                        if b >= 64 {
                            *n as i64
                        } else {
                            (*n as u128 & ((1u128 << b) - 1)) as i64
                        }
                    }
                    (Const::Bool(b), _) => i64::from(*b),
                    (Const::Float(x), IrTy::F32) => (*x as f32).to_bits() as i64,
                    (Const::Float(x), _) => x.to_bits() as i64,
                };
                self.a.mov_ri(RAX, bits_v);
                self.st(res.unwrap(), RAX);
            }
            Op::Bin(op, a, b) => self.bin(f, *op, *a, *b, res.unwrap())?,
            Op::Checked(op, signed, a, b) => self.checked(f, *op, *signed, *a, *b, res.unwrap()),
            Op::Cmp(op, a, b) => self.cmp(f, *op, *a, *b, res.unwrap()),
            Op::Neg(a) => {
                let t = f.ty(*a);
                if is_float(t) {
                    self.ld(RAX, *a);
                    if t == IrTy::F32 {
                        self.a.alu_ri(Alu::Xor, RAX, i32::MIN, 4);
                    } else {
                        self.a.mov_ri(RCX, i64::MIN);
                        self.a.alu_rr(Alu::Xor, RAX, RCX, 8);
                    }
                } else {
                    self.ld(RAX, *a);
                    self.a.neg(RAX, 8);
                    self.mask(RAX, bits(t));
                }
                self.st(res.unwrap(), RAX);
            }
            Op::Not(a) => {
                let t = f.ty(*a);
                self.ld(RAX, *a);
                if t == IrTy::Bool {
                    self.a.alu_ri(Alu::Xor, RAX, 1, 4);
                } else {
                    self.a.not(RAX, 8);
                    self.mask(RAX, bits(t));
                }
                self.st(res.unwrap(), RAX);
            }
            Op::Cast(k, a, to) => self.cast(f, *k, *a, *to, res.unwrap()),
            Op::Call(callee, args) => {
                let l = self.funcs[callee.0 as usize];
                self.call(
                    f,
                    args,
                    None,
                    &mut |g| g.a.call(l),
                    res,
                    callee_ret(self.m, *callee),
                );
            }
            Op::CallPtr(ptys, ret, fp, args) => {
                let _ = ptys;
                let fp = *fp;
                self.call(f, args, Some(fp), &mut |g| g.a.call_reg(R11), res, *ret);
            }
            Op::Slot(sz, _) => {
                let off = self.slot_off[&id];
                // Memory is zero when a slot comes into use, as in the interpreter.
                let mut at = 0i32;
                let mut left = *sz as i32;
                if left <= 64 {
                    for (n, w) in [(8, 8u8), (4, 4), (2, 2), (1, 1)] {
                        while left >= n {
                            self.a.store_imm(mem(RBP, off + at), 0, w);
                            at += n;
                            left -= n;
                        }
                    }
                } else {
                    // `rdi` may hold a value: the string instruction needs it.
                    self.a.push(RDI);
                    self.a.lea(RDI, mem(RBP, off));
                    self.a.alu_rr(Alu::Xor, RAX, RAX, 4);
                    self.a.mov_ri(RCX, *sz as i64);
                    self.a.rep_stosb();
                    self.a.pop(RDI);
                }
                self.a.lea(RAX, mem(RBP, off));
                self.st(res.unwrap(), RAX);
            }
            Op::PtrAdd(p, n) => {
                self.ld(RAX, *p);
                self.ld(RCX, *n);
                self.a.alu_rr(Alu::Add, RAX, RCX, 8);
                self.st(res.unwrap(), RAX);
            }
            Op::MemCopy(d, s, n) => {
                let (wd, ws, wn) = (
                    self.loc[d.0 as usize],
                    self.loc[s.0 as usize],
                    self.loc[n.0 as usize],
                );
                self.moves(&[
                    (Where::Reg(RDI), wd),
                    (Where::Reg(RSI), ws),
                    (Where::Reg(RCX), wn),
                ]);
                self.a.rep_movsb();
            }
            Op::DataAddr(i) => {
                let off = self.data_off[*i as usize];
                self.a.lea_sect(RAX, Sect::Data, off);
                self.st(res.unwrap(), RAX);
            }
            Op::FuncAddr(fid) => {
                let l = self.funcs[fid.0 as usize];
                self.a.lea_label(RAX, l);
                self.st(res.unwrap(), RAX);
            }
            Op::Print(p, n) => {
                let (wp, wn) = (self.loc[p.0 as usize], self.loc[n.0 as usize]);
                self.moves(&[
                    (Where::Reg(RDI), Where::Imm(1)),
                    (Where::Reg(RSI), wp),
                    (Where::Reg(RDX), wn),
                ]);
                let w = self.rt().write_fd;
                self.a.call(w);
            }
            Op::Load(t, p) => {
                self.ld(RCX, *p);
                let r = res.unwrap();
                match t {
                    IrTy::F32 | IrTy::F64 => {
                        self.a.fload(Xmm(0), mem(RCX, 0), *t == IrTy::F64);
                        self.stf(r, Xmm(0), f);
                    }
                    _ => {
                        self.a.load_ext(RAX, mem(RCX, 0), size(*t), false, true);
                        self.st(r, RAX);
                    }
                }
            }
            Op::Store(p, x) => {
                let t = f.ty(*x);
                self.ld(RCX, *p);
                if is_float(t) {
                    self.ldf(Xmm(0), *x, f);
                    self.a.fstore(mem(RCX, 0), Xmm(0), t == IrTy::F64);
                } else {
                    self.ld(RAX, *x);
                    self.a.store(mem(RCX, 0), RAX, size(t));
                }
            }
            Op::Alloc(n) => {
                self.ld(RDI, *n);
                let al = self.rt().alloc;
                self.a.call(al);
                self.st(res.unwrap(), RAX);
            }
            Op::Free(p) => {
                self.ld(RDI, *p);
                let fr = self.rt().free;
                self.a.call(fr);
            }
        }
        Ok(())
    }

    /// A call: stack arguments, register arguments, the call, the result.
    fn call(
        &mut self,
        f: &Func,
        args: &[Value],
        fptr: Option<Value>,
        emit: &mut dyn FnMut(&mut Gen<'m>),
        res: Option<Value>,
        ret: IrTy,
    ) {
        let tys: Vec<IrTy> = args.iter().map(|a| f.ty(*a)).collect();
        let locs = classify(&tys);
        let stack: Vec<usize> = locs
            .iter()
            .enumerate()
            .filter(|(_, l)| matches!(l, Loc::Stack(_)))
            .map(|(i, _)| i)
            .collect();
        let pad = stack.len() % 2 == 1;
        if pad {
            self.a.alu_ri(Alu::Sub, RSP, 8, 8);
        }
        for &i in stack.iter().rev() {
            self.ld(RAX, args[i]);
            self.a.push(RAX);
        }
        // The register arguments (and the function address) in one parallel move: an argument may
        // sit in another argument's register.
        let mut copies = Vec::new();
        for (arg, loc) in args.iter().zip(&locs) {
            match loc {
                Loc::Int(i) => copies.push((Where::Reg(INT_REGS[*i]), self.loc[arg.0 as usize])),
                Loc::Float(i) => {
                    let m = self.vmem(*arg);
                    self.a.fload(Xmm(*i as u8), m, f.ty(*arg) == IrTy::F64);
                }
                Loc::Stack(_) => {}
            }
        }
        if let Some(p) = fptr {
            copies.push((Where::Reg(R11), self.loc[p.0 as usize]));
        }
        self.moves(&copies);
        emit(self);
        let cleanup = 8 * (stack.len() + usize::from(pad)) as i32;
        if cleanup > 0 {
            self.a.alu_ri(Alu::Add, RSP, cleanup, 8);
        }
        if let Some(r) = res {
            if is_float(ret) {
                let m = self.vmem(r);
                self.a.fstore(m, Xmm(0), ret == IrTy::F64);
            } else {
                self.st(r, RAX);
            }
        }
    }

    // ---- Arithmetic ----------------------------------------------------------------------------------

    fn bin(&mut self, f: &Func, op: BinOp, a: Value, b: Value, r: Value) -> Result<(), String> {
        let t = f.ty(a);
        if is_float(t) {
            let sse = match op {
                BinOp::Add => Sse::Add,
                BinOp::Sub => Sse::Sub,
                BinOp::Mul => Sse::Mul,
                BinOp::Div => Sse::Div,
                BinOp::Rem => {
                    // x87 `fprem` is exactly C's `fmod`: the sign of the dividend, NaN for a zero divisor.
                    let d = t == IrTy::F64;
                    let (ma, mb) = (self.vmem(a), self.vmem(b));
                    self.a.x87_load(mb, d);
                    self.a.x87_load(ma, d);
                    let again = self.a.new_label();
                    self.a.bind(again);
                    self.a.x87_prem();
                    self.a.x87_status_ax();
                    self.a.shift_imm(Shift::Shr, RAX, 10, 4);
                    self.a.alu_ri(Alu::And, RAX, 1, 4);
                    self.a.jcc(Cc::Ne, again);
                    self.a.x87_drop_second();
                    let mr = self.vmem(r);
                    self.a.x87_store_pop(mr, d);
                    return Ok(());
                }
                _ => return Err("not a float operation".to_string()),
            };
            self.ldf(Xmm(0), a, f);
            self.ldf(Xmm(1), b, f);
            self.a.farith(sse, Xmm(0), Xmm(1), t == IrTy::F64);
            self.stf(r, Xmm(0), f);
            return Ok(());
        }
        let w = bits(t);
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::And | BinOp::Or | BinOp::Xor => {
                // Built in the result's own register when it has one. That may be the register of
                // `a` (nothing to load) or of `b`: then a commutative operation swaps the operands
                // and the others are built in `rax`.
                let (mut a, mut b) = (a, b);
                let mut d = self.dst(r);
                if a != b && self.loc[b.0 as usize] == Where::Reg(d) {
                    if matches!(op, BinOp::Sub) {
                        d = RAX;
                    } else {
                        std::mem::swap(&mut a, &mut b);
                    }
                }
                self.ld(d, a);
                let alu = match op {
                    BinOp::Add => Alu::Add,
                    BinOp::Sub => Alu::Sub,
                    BinOp::And => Alu::And,
                    BinOp::Or => Alu::Or,
                    _ => Alu::Xor,
                };
                match self.imm32(b, f, false) {
                    Some(n) if op != BinOp::Mul => self.a.alu_ri(alu, d, n, 8),
                    _ => {
                        let rb = self.src(b, f, false, RCX);
                        if op == BinOp::Mul {
                            self.a.imul_rr(d, rb, 8);
                        } else {
                            self.a.alu_rr(alu, d, rb, 8);
                        }
                    }
                }
                self.mask(d, w);
                self.st(r, d);
            }
            BinOp::Div | BinOp::Rem | BinOp::UDiv | BinOp::URem => {
                let signed = matches!(op, BinOp::Div | BinOp::Rem);
                let want_rem = matches!(op, BinOp::Rem | BinOp::URem);
                self.ld_ext(RAX, a, f, signed);
                self.ld_ext(RCX, b, f, signed);
                self.a.test_rr(RCX, RCX, 8);
                let z = self.stop_div0;
                self.a.jcc(Cc::E, z);
                if signed {
                    let (normal, done) = (self.a.new_label(), self.a.new_label());
                    if w == 64 {
                        // `MIN / -1` would trap: the interpreter wraps.
                        self.a.alu_ri(Alu::Cmp, RCX, -1, 8);
                        self.a.jcc(Cc::Ne, normal);
                        if want_rem {
                            self.a.alu_rr(Alu::Xor, RDX, RDX, 4);
                        } else {
                            self.a.neg(RAX, 8);
                        }
                        self.a.jmp(done);
                    }
                    self.a.bind(normal);
                    self.a.sign_extend_acc(8);
                    self.a.idiv(RCX, 8);
                    self.a.bind(done);
                } else {
                    self.a.alu_rr(Alu::Xor, RDX, RDX, 4);
                    self.a.div(RCX, 8);
                }
                let out = if want_rem { RDX } else { RAX };
                self.mask(out, w);
                self.st(r, out);
            }
            BinOp::Shl | BinOp::Shr | BinOp::UShr if matches!(self.loc[b.0 as usize], Where::Imm(n) if (0..w as i64).contains(&n)) =>
            {
                // A constant amount below the width: no check, an immediate shift.
                let Where::Imm(n) = self.loc[b.0 as usize] else {
                    unreachable!()
                };
                self.ld_ext(RAX, a, f, op == BinOp::Shr);
                let sh = match op {
                    BinOp::Shl => Shift::Shl,
                    BinOp::Shr => Shift::Sar,
                    _ => Shift::Shr,
                };
                self.a.shift_imm(sh, RAX, n as u8, 8);
                self.mask(RAX, w);
                self.st(r, RAX);
            }
            BinOp::Shl | BinOp::Shr | BinOp::UShr => {
                self.ld(RCX, b);
                self.a.alu_ri(Alu::Cmp, RCX, w as i32, 8);
                let s = self.stop_shift;
                self.a.jcc(Cc::Ae, s);
                self.ld_ext(RAX, a, f, op == BinOp::Shr);
                let sh = match op {
                    BinOp::Shl => Shift::Shl,
                    BinOp::Shr => Shift::Sar,
                    _ => Shift::Shr,
                };
                self.a.shift_cl(sh, RAX, 8);
                self.mask(RAX, w);
                self.st(r, RAX);
            }
        }
        Ok(())
    }

    fn checked(&mut self, f: &Func, op: BinOp, signed: bool, a: Value, b: Value, r: Value) {
        let w = bits(f.ty(a));
        let ov = self.stop_overflow;
        if w == 64 && (matches!(op, BinOp::Add | BinOp::Sub) || (op == BinOp::Mul && signed)) {
            let (mut a, mut b) = (a, b);
            let mut d = self.dst(r);
            if a != b && self.loc[b.0 as usize] == Where::Reg(d) {
                if op == BinOp::Sub {
                    d = RAX;
                } else {
                    std::mem::swap(&mut a, &mut b);
                }
            }
            self.ld(d, a);
            match self.imm32(b, f, signed) {
                Some(n) if op != BinOp::Mul => {
                    let alu = if op == BinOp::Add { Alu::Add } else { Alu::Sub };
                    self.a.alu_ri(alu, d, n, 8);
                }
                _ => {
                    let rb = self.src(b, f, false, RCX);
                    match op {
                        BinOp::Add => self.a.alu_rr(Alu::Add, d, rb, 8),
                        BinOp::Sub => self.a.alu_rr(Alu::Sub, d, rb, 8),
                        _ => self.a.imul_rr(d, rb, 8),
                    }
                }
            }
            self.a.jcc(if signed { Cc::O } else { Cc::B }, ov);
            self.st(r, d);
            return;
        }
        self.ld_ext(RAX, a, f, signed);
        self.ld_ext(RCX, b, f, signed);
        if w == 64 {
            match (op, signed) {
                (BinOp::Add, true) => {
                    self.a.alu_rr(Alu::Add, RAX, RCX, 8);
                    self.a.jcc(Cc::O, ov);
                }
                (BinOp::Sub, true) => {
                    self.a.alu_rr(Alu::Sub, RAX, RCX, 8);
                    self.a.jcc(Cc::O, ov);
                }
                (BinOp::Mul, true) => {
                    self.a.imul_rr(RAX, RCX, 8);
                    self.a.jcc(Cc::O, ov);
                }
                (BinOp::Add, false) => {
                    self.a.alu_rr(Alu::Add, RAX, RCX, 8);
                    self.a.jcc(Cc::B, ov);
                }
                (BinOp::Sub, false) => {
                    self.a.alu_rr(Alu::Sub, RAX, RCX, 8);
                    self.a.jcc(Cc::B, ov);
                }
                _ => {
                    self.a.mul(RCX, 8);
                    self.a.jcc(Cc::B, ov);
                }
            }
        } else {
            // Narrow: no overflow in 64 bits; the result must survive the trip through the width.
            match op {
                BinOp::Add => self.a.alu_rr(Alu::Add, RAX, RCX, 8),
                BinOp::Sub => self.a.alu_rr(Alu::Sub, RAX, RCX, 8),
                _ => self.a.imul_rr(RAX, RCX, 8),
            }
            self.a.extend_rr(RDX, RAX, (w / 8) as u8, signed, true);
            self.a.alu_rr(Alu::Cmp, RDX, RAX, 8);
            self.a.jcc(Cc::Ne, ov);
            self.mask(RAX, w);
        }
        self.st(r, RAX);
    }

    fn cmp(&mut self, f: &Func, op: CmpOp, a: Value, b: Value, r: Value) {
        let t = f.ty(a);
        if is_float(t) {
            self.ldf(Xmm(0), a, f);
            self.ldf(Xmm(1), b, f);
            let d = t == IrTy::F64;
            match op {
                CmpOp::Eq | CmpOp::Ne => {
                    self.a.fcmp(Xmm(0), Xmm(1), d);
                    if op == CmpOp::Eq {
                        self.a.setcc(Cc::E, RAX);
                        self.a.setcc(Cc::Np, RCX);
                        self.a.alu_rr(Alu::And, RAX, RCX, 1);
                    } else {
                        self.a.setcc(Cc::Ne, RAX);
                        self.a.setcc(Cc::P, RCX);
                        self.a.alu_rr(Alu::Or, RAX, RCX, 1);
                    }
                }
                CmpOp::Lt | CmpOp::ULt | CmpOp::Le | CmpOp::ULe => {
                    // `a < b` is `b > a`; unordered gives "not above".
                    self.a.fcmp(Xmm(1), Xmm(0), d);
                    let cc = if matches!(op, CmpOp::Lt | CmpOp::ULt) {
                        Cc::A
                    } else {
                        Cc::Ae
                    };
                    self.a.setcc(cc, RAX);
                }
                _ => {
                    self.a.fcmp(Xmm(0), Xmm(1), d);
                    let cc = if matches!(op, CmpOp::Gt | CmpOp::UGt) {
                        Cc::A
                    } else {
                        Cc::Ae
                    };
                    self.a.setcc(cc, RAX);
                }
            }
        } else {
            let cc = self.int_cmp(f, op, a, b);
            self.a.setcc(cc, RAX);
        }
        self.a.extend_rr(RAX, RAX, 1, false, false);
        self.st(r, RAX);
    }

    /// Compares two integer, boolean or pointer values; gives the condition that holds when `op` is true.
    fn int_cmp(&mut self, f: &Func, op: CmpOp, a: Value, b: Value) -> Cc {
        let signed = matches!(f.ty(a), IrTy::Int(_))
            && matches!(op, CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge);
        let ra = self.src(a, f, signed, RAX);
        match self.imm32(b, f, signed) {
            Some(n) => self.a.alu_ri(Alu::Cmp, ra, n, 8),
            None => {
                let rb = self.src(b, f, signed, RCX);
                self.a.alu_rr(Alu::Cmp, ra, rb, 8);
            }
        }
        match op {
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
        }
    }

    fn cast(&mut self, f: &Func, k: CastKind, a: Value, to: IrTy, r: Value) {
        let from = f.ty(a);
        match k {
            CastKind::ZExt | CastKind::Trunc => {
                self.ld(RAX, a);
                self.mask(RAX, bits(to));
                self.st(r, RAX);
            }
            CastKind::SExt => {
                self.ld_ext(RAX, a, f, true);
                self.mask(RAX, bits(to));
                self.st(r, RAX);
            }
            CastKind::PtrToInt | CastKind::IntToPtr => {
                self.ld(RAX, a);
                self.st(r, RAX);
            }
            CastKind::IntToFloat => {
                self.ld_ext(RAX, a, f, true);
                self.a.cvt_int_to_float(Xmm(0), RAX, true, to == IrTy::F64);
                self.stf(r, Xmm(0), f);
            }
            CastKind::FloatToInt => {
                self.ldf(Xmm(0), a, f);
                self.a
                    .cvt_float_to_int(RAX, Xmm(0), true, from == IrTy::F64);
                self.mask(RAX, bits(to));
                self.st(r, RAX);
            }
            CastKind::FloatResize => {
                self.ldf(Xmm(0), a, f);
                if from != to {
                    self.a.cvt_float_resize(Xmm(0), Xmm(0), to == IrTy::F64);
                }
                self.stf(r, Xmm(0), f);
            }
        }
    }
}

fn callee_ret(m: &Module, f: FuncId) -> IrTy {
    m.funcs[f.0 as usize].ret
}
