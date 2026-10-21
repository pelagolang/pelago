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
//! `x29` is the frame pointer; `x19` to `x28` and `d8` to `d15` are the callee-saved homes the register
//! allocator (`regs_a64.rs`, stage R3) gives to the busiest values, for their whole life.
//!
//! The frame, from `sp` up: the outgoing argument area, the saved callee-saved registers, the value
//! slots, the `Slot` objects, then `x29` and `x30` (pushed first, so `x29` points at them).

use super::a64::*;
use super::elf::{self, Blob, Symbol};
use super::gen::Options;
use super::regs_a64;
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
    let main_fn = m
        .funcs
        .iter()
        .position(|f| f.name == entry)
        .ok_or_else(|| format!("no function `{entry}`"))?;
    let mut g = Gen::new(m);
    g.regalloc = opts.regalloc;
    let main_label = g.funcs[main_fn];
    let rt = rt_a64::emit(
        &mut g.a,
        &mut g.ro,
        &mut g.data,
        main_label,
        opts.leak_check,
        opts.target,
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
    match opts.target {
        Os::Linux => elf::write_a64(&g.a, &g.ro, &g.data, 0, &syms),
        Os::Windows => super::pe::write_a64(&g.a, &g.ro, &g.data, 0),
        Os::MacOs => super::macho::write_a64(&g.a, &g.ro, &g.data, 0),
    }
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

/// An `add` or `sub` immediate for the constant `n`: whether the instruction is the opposite one (the
/// constant was negative) and the 12-bit value (possibly shifted by 12).
fn addsub_imm(n: i64) -> Option<(bool, u32)> {
    if Asm::fits_addsub(n as u64) {
        Some((false, n as u32))
    } else if n < 0 && Asm::fits_addsub(n.unsigned_abs()) {
        Some((true, n.unsigned_abs() as u32))
    } else {
        None
    }
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
    /// The register a value lives in instead (its `off` is then [`NONE`]).
    reg: Vec<Option<Reg>>,
    freg: Vec<Option<V>>,
    vreg: Vec<Option<V>>,
    /// The callee-saved registers this function uses, and the frame offset of their save area.
    saved_g: Vec<Reg>,
    saved_f: Vec<V>,
    save_off: i32,
    regalloc: bool,
    imm: Vec<Option<i64>>,
    slot_off: HashMap<usize, i32>,
    frame: i32,
    block_labels: Vec<Label>,
    stops: [Option<Label>; 4],
    stubs: Vec<EdgeStub>,
    cond_stubs: Vec<CondStub>,
    /// Where `CheckedIf` resumes after the jump to its stub.
    resume: Option<Label>,
    /// Integer compares that only feed the `Select` right after them: the compare (keyed by the
    /// `Select`) is made where the `Select` is, and the `Select` uses the flags.
    sel_cmp: HashMap<usize, (CmpOp, Value, Value)>,
    /// The compares that were moved into their `Select`.
    sel_skip: std::collections::HashSet<usize>,
    /// `x - a * b` and `x + a * b` on 64 bits: the instruction of the sum or difference, with (is a sum, a, b, x).
    madd: HashMap<usize, (bool, Value, Value, Value)>,
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
            reg: Vec::new(),
            freg: Vec::new(),
            vreg: Vec::new(),
            saved_g: Vec::new(),
            saved_f: Vec::new(),
            save_off: 0,
            regalloc: true,
            imm: Vec::new(),
            slot_off: HashMap::new(),
            frame: 0,
            block_labels: Vec::new(),
            sel_cmp: HashMap::new(),
            sel_skip: std::collections::HashSet::new(),
            madd: HashMap::new(),
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
        let n = f.values.len();
        let alloc = if self.regalloc {
            let mut fused = vec![false; n];
            for bi in 0..f.blocks.len() {
                if let Some(last) = super::regs::fused_cmp(f, bi, &uses) {
                    if let Some(r) = f.insts[last].result {
                        fused[r.0 as usize] = true;
                    }
                }
            }
            self.sel_cmp.clear();
            self.sel_skip.clear();
            for b in &f.blocks {
                for w in b.insts.windows(2) {
                    if let (Op::Cmp(op, x, y), Op::Select(c, a, e)) =
                        (&f.insts[w[0]].op, &f.insts[w[1]].op)
                    {
                        let r = f.insts[w[0]].result;
                        if r == Some(*c)
                            && *a != *c
                            && *e != *c
                            && uses[c.0 as usize] == 1
                            && !is_float(f.ty(*x))
                            && !fused[c.0 as usize]
                        {
                            fused[c.0 as usize] = true;
                            self.sel_cmp.insert(w[1], (*op, *x, *y));
                            self.sel_skip.insert(w[0]);
                        }
                    }
                }
            }
            // A product read once, right before the sum or difference that uses it, is one `madd` / `msub`.
            self.madd.clear();
            for b in &f.blocks {
                for w in b.insts.windows(2) {
                    if let (
                        Op::Bin(BinOp::Mul, a, m),
                        Op::Bin(op @ (BinOp::Add | BinOp::Sub), p, q),
                    ) = (&f.insts[w[0]].op, &f.insts[w[1]].op)
                    {
                        let Some(prod) = f.insts[w[0]].result else {
                            continue;
                        };
                        let (add, x) = match op {
                            BinOp::Add if *q == prod => (true, *p),
                            BinOp::Add if *p == prod => (true, *q),
                            _ if *q == prod => (false, *p),
                            _ => continue,
                        };
                        if f.ty(prod) == IrTy::Int(64)
                            && uses[prod.0 as usize] == 1
                            && x != prod
                            && !fused[prod.0 as usize]
                        {
                            fused[prod.0 as usize] = true;
                            self.madd.insert(w[1], (add, *a, *m, x));
                            self.sel_skip.insert(w[0]);
                        }
                    }
                }
            }
            regs_a64::assign(f, &self.imm, &uses, &fused)
        } else {
            regs_a64::Alloc {
                reg: vec![None; n],
                freg: vec![None; n],
                vreg: vec![None; n],
            }
        };
        self.reg = alloc.reg;
        self.freg = alloc.freg;
        self.vreg = alloc.vreg;
        let mut used_g = [false; 32];
        let mut used_f = [false; 32];
        for r in self.reg.iter().flatten() {
            used_g[r.0 as usize] = true;
        }
        for x in self.freg.iter().flatten() {
            used_f[x.0 as usize] = true;
        }
        self.saved_g = (19..=28)
            .filter(|&k| used_g[k])
            .map(|k| Reg(k as u8))
            .collect();
        self.saved_f = (8..=15)
            .filter(|&k| used_f[k])
            .map(|k| V(k as u8))
            .collect();
        self.save_off = at;
        at += 8 * (self.saved_g.len() + self.saved_f.len()) as i32;
        self.off.clear();
        for (v, &ty) in f.values.iter().enumerate() {
            self.off.push(
                if self.imm[v].is_some()
                    || uses[v] == 0
                    || self.reg[v].is_some()
                    || self.freg[v].is_some()
                    || self.vreg[v].is_some()
                {
                    NONE
                } else if ty == IrTy::V2I64 {
                    at = (at + 15) / 16 * 16;
                    at += 16;
                    at - 16
                } else {
                    at += 8;
                    at - 8
                },
            );
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
            Some(_) if self.reg[v.0 as usize].is_some() => {
                let h = self.reg[v.0 as usize].unwrap();
                self.a.mov_rr(r, h);
            }
            Some(n) => self.a.mov_ri(r, n),
            None => {
                if let Some(h) = self.reg[v.0 as usize] {
                    if h != r {
                        self.a.mov_rr(r, h);
                    }
                    return;
                }
                let off = self.off[v.0 as usize];
                debug_assert!(off != NONE, "value {} is read but has no slot", v.0);
                self.a.ldr(r, SP, off, 8);
            }
        }
    }

    /// The register to read `v` from: its home, or `scratch` after loading it there.
    fn rd(&mut self, v: Value, scratch: Reg) -> Reg {
        match (self.imm[v.0 as usize], self.reg[v.0 as usize]) {
            (_, Some(h)) => h,
            _ => {
                self.ld(scratch, v);
                scratch
            }
        }
    }

    /// Like [`Gen::rd`] for a read that extends from the value's width.
    fn rd_ext(&mut self, v: Value, scratch: Reg, f: &Func, signed: bool) -> Reg {
        let narrow_signed = signed && matches!(f.ty(v), IrTy::Int(b) if b < 64);
        match (self.imm[v.0 as usize], self.reg[v.0 as usize]) {
            (_, Some(h)) if !narrow_signed => h,
            _ => {
                self.ld_ext(scratch, v, f, signed);
                scratch
            }
        }
    }

    /// The register a result is built in: its home, or `scratch` (then [`Gen::fin`] stores it).
    fn wr(&self, v: Value, scratch: Reg) -> Reg {
        self.reg[v.0 as usize].unwrap_or(scratch)
    }

    fn fin(&mut self, v: Value, r: Reg) {
        if self.reg[v.0 as usize] != Some(r) {
            self.st(v, r);
        }
    }

    fn rdf(&mut self, v: Value, scratch: V, f: &Func) -> V {
        match self.freg[v.0 as usize] {
            Some(h) => h,
            None => {
                self.ldf(scratch, v, f);
                scratch
            }
        }
    }

    fn wrf(&self, v: Value, scratch: V) -> V {
        self.freg[v.0 as usize].unwrap_or(scratch)
    }

    fn finf(&mut self, v: Value, x: V, f: &Func) {
        if self.freg[v.0 as usize] != Some(x) {
            self.stf(v, x, f);
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
        if let Some(h) = self.reg[v.0 as usize] {
            if h != r {
                self.a.mov_rr(h, r);
            }
            return;
        }
        let off = self.off[v.0 as usize];
        if off != NONE {
            self.a.str(r, SP, off, 8);
        }
    }

    /// Loads a float into `x` (a 4- or 8-byte value).
    fn ldf(&mut self, x: V, v: Value, f: &Func) {
        if let Some(h) = self.freg[v.0 as usize] {
            if h != x {
                self.a.vmov(x, h);
            }
            return;
        }
        let off = self.off[v.0 as usize];
        debug_assert!(off != NONE, "float {} is read but has no slot", v.0);
        self.a
            .ldr_f(x, SP, off, if f.ty(v) == IrTy::F32 { 4 } else { 8 });
    }

    fn stf(&mut self, v: Value, x: V, f: &Func) {
        if let Some(h) = self.freg[v.0 as usize] {
            if h != x {
                self.a.vmov(h, x);
            }
            return;
        }
        let off = self.off[v.0 as usize];
        if off != NONE {
            self.a
                .str_f(x, SP, off, if f.ty(v) == IrTy::F32 { 4 } else { 8 });
        }
    }

    /// The vector `v` in `x`.
    fn ldv(&mut self, x: V, v: Value) {
        match self.vreg[v.0 as usize] {
            Some(h) => {
                if h != x {
                    self.a.vmov(x, h);
                }
            }
            None => {
                let off = self.off[v.0 as usize];
                debug_assert!(off != NONE);
                self.a.ldr_f(x, SP, off, 16);
            }
        }
    }

    /// The register to read the vector `v` from: its home, or `scratch` after a load.
    fn rdv(&mut self, v: Value, scratch: V) -> V {
        match self.vreg[v.0 as usize] {
            Some(h) => h,
            None => {
                self.ldv(scratch, v);
                scratch
            }
        }
    }

    /// The register to build the vector `v` in: its home, or `scratch`.
    fn wrv(&self, v: Value, scratch: V) -> V {
        self.vreg[v.0 as usize].unwrap_or(scratch)
    }

    /// Completes a vector result built in `x` (a no-op in its home).
    fn finv(&mut self, v: Value, x: V) {
        self.stv(v, x);
    }

    fn stv(&mut self, v: Value, x: V) {
        if let Some(h) = self.vreg[v.0 as usize] {
            if h != x {
                self.a.vmov(h, x);
            }
            return;
        }
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
        self.saved_pairs(false);
        for (k, x) in self.saved_f.clone().into_iter().enumerate() {
            let at = self.save_off + 8 * (self.saved_g.len() + k) as i32;
            self.a.str_f(x, SP, at, 8);
        }
        for (p, loc) in params.iter().zip(classify(&tys)) {
            if !self.has_home(*p) {
                continue;
            }
            match loc {
                Loc::Int(i) => self.st(*p, INT_REGS[i]),
                Loc::Float(i) => self.stf(*p, V(i as u8), f),
                Loc::Stack(k) => {
                    self.a.ldr(X9, FP, 16 + 8 * k as i32, 8);
                    self.st(*p, X9);
                }
            }
        }
        // Constants kept in a register are built once, after the parameters are out of the way.
        for (v, n) in self.imm.clone().into_iter().enumerate() {
            if let (Some(n), Some(r)) = (n, self.reg[v]) {
                self.a.mov_ri(r, n);
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
            let mut k = 0;
            while k < b.insts.len() {
                let i = b.insts[k];
                k += 1;
                if fused.is_some_and(|(l, ..)| l == i) || self.sel_skip.contains(&i) {
                    continue;
                }
                // A check whose value nobody reads, right before the plain operation it guards (what
                // if-conversion leaves), is one instruction: the operation with the check.
                if let Some(&n) = b.insts.get(k) {
                    if let Some(r) = self.guarded_by(f, &uses, i, n) {
                        self.checked_inst(f, &f.insts[i].op, r);
                        k += 1;
                        continue;
                    }
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

    /// Whether a value is kept anywhere (a register or a slot).
    fn has_home(&self, v: Value) -> bool {
        self.off[v.0 as usize] != NONE
            || self.reg[v.0 as usize].is_some()
            || self.freg[v.0 as usize].is_some()
            || self.vreg[v.0 as usize].is_some()
    }

    /// Saves (or, with `load`, restores) the callee-saved general registers this function uses, two at
    /// a time while the offsets fit a pair instruction.
    fn saved_pairs(&mut self, load: bool) {
        let g = self.saved_g.clone();
        let pairs = self.save_off + 8 * g.len() as i32 <= 504;
        let mut k = 0;
        while k < g.len() {
            let off = self.save_off + 8 * k as i32;
            if pairs && k + 1 < g.len() {
                if load {
                    self.a.ldp(g[k], g[k + 1], SP, off);
                } else {
                    self.a.stp(g[k], g[k + 1], SP, off);
                }
                k += 2;
            } else {
                if load {
                    self.a.ldr(g[k], SP, off, 8);
                } else {
                    self.a.str(g[k], SP, off, 8);
                }
                k += 1;
            }
        }
    }

    fn epilogue(&mut self) {
        self.saved_pairs(true);
        for (k, x) in self.saved_f.clone().into_iter().enumerate() {
            let at = self.save_off + 8 * (self.saved_g.len() + k) as i32;
            self.a.ldr_f(x, SP, at, 8);
        }
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
                        let r = self.rd(*c, X9);
                        Br::NonZero(r)
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
            .filter(|(p, a)| p != a && self.has_home(**p))
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
                    } else if is_float(f.ty(victim)) {
                        self.ldf(V(1), victim, f);
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
            let dst = self.wrv(to, V(0));
            match c.from {
                Src::Val(v) => self.ldv(dst, v),
                Src::Temp => self.a.vmov(dst, V(1)),
                Src::Imm(_) => unreachable!("a vector constant"),
            }
            self.finv(to, dst);
            return;
        }
        if is_float(f.ty(to)) {
            // Straight into the target's register when it has one.
            let dst = self.wrf(to, V(0));
            match c.from {
                Src::Val(v) => self.ldf(dst, v, f),
                Src::Temp => self.a.vmov(dst, V(1)),
                Src::Imm(_) => unreachable!("a float constant is a `Const` instruction"),
            }
            self.finf(to, dst, f);
            return;
        }
        // Straight into the target's register when it has one.
        let dst = self.reg[to.0 as usize].unwrap_or(X9);
        match c.from {
            Src::Val(v) => self.ld(dst, v),
            Src::Imm(n) => self.a.mov_ri(dst, n),
            Src::Temp => self.a.mov_rr(dst, X13),
        }
        self.fin(to, dst);
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
                let r = res.unwrap();
                if is_float(*t) {
                    self.a.mov_ri(X9, bits_v);
                    let x = self.wrf(r, V(0));
                    self.a.fmov_from_gp(*t == IrTy::F64, x, X9);
                    self.finf(r, x, f);
                } else {
                    let d = self.wr(r, X9);
                    self.a.mov_ri(d, bits_v);
                    self.fin(r, d);
                }
            }
            Op::Fma(a, b, c) => {
                let dbl = f.ty(*a) == IrTy::F64;
                let (xa, xb, xc) = (
                    self.rdf(*a, V(0), f),
                    self.rdf(*b, V(1), f),
                    self.rdf(*c, V(31), f),
                );
                let xd = self.wrf(res.unwrap(), V(0));
                self.a.fmadd(dbl, xd, xa, xb, xc);
                self.finf(res.unwrap(), xd, f);
            }
            Op::Bin(..) if self.madd.contains_key(&id) => {
                let (add, a, b, x) = self.madd[&id];
                let (ra, rb, rx) = (self.rd(a, X9), self.rd(b, X10), self.rd(x, X11));
                let r = res.unwrap();
                let rd = self.wr(r, X9);
                if add {
                    self.a.madd(rd, ra, rb, rx);
                } else {
                    self.a.msub(rd, ra, rb, rx);
                }
                self.fin(r, rd);
            }
            Op::Bin(op, a, b) => self.bin(f, *op, *a, *b, res.unwrap())?,
            Op::Checked(..) | Op::CheckedIf(..) => self.checked_inst(f, &inst.op, res.unwrap()),
            Op::Select(_, a, b) if self.sel_cmp.contains_key(&id) => {
                let (op, x, y) = self.sel_cmp[&id];
                let ra = self.rd(*a, X9);
                let rb = self.rd(*b, X10);
                let cc = self.int_cmp_in(f, op, x, y, X11, X12);
                let rd = self.wr(res.unwrap(), X9);
                self.a.csel(rd, ra, rb, cc);
                self.fin(res.unwrap(), rd);
            }
            Op::Select(c, a, b) => {
                let ra = self.rd(*a, X9);
                let rb = self.rd(*b, X10);
                let rc = self.rd(*c, X11);
                let rd = self.wr(res.unwrap(), X9);
                self.a.cmp_ri(rc, 0);
                self.a.csel(rd, ra, rb, Cond::Ne);
                self.fin(res.unwrap(), rd);
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
                        let a = self.rdv(*a, V(0));
                        self.a.vaddp(V(0), a);
                        self.a.fmov_to_gp(true, X9, V(0));
                        self.st(r, X9);
                    }
                    VUn::Splat => {
                        self.ld(X9, *a);
                        let d = self.wrv(r, V(0));
                        self.a.vdup(d, X9);
                        self.finv(r, d);
                    }
                    VUn::Shl(k) | VUn::UShr(k) => {
                        let a = self.rdv(*a, V(0));
                        let d = self.wrv(r, V(0));
                        if matches!(o, VUn::Shl(_)) {
                            self.a.vshl(d, a, *k as u32);
                        } else {
                            self.a.vushr(d, a, *k as u32);
                        }
                        self.finv(r, d);
                    }
                }
            }
            Op::VBin(o, a, b) => {
                let r = res.unwrap();
                let d = self.wrv(r, V(0));
                if *o == VBin::Build {
                    self.ld(X9, *a);
                    self.ld(X10, *b);
                    self.a.vins(d, 0, X9);
                    self.a.vins(d, 1, X10);
                } else {
                    let a = self.rdv(*a, V(0));
                    let b = self.rdv(*b, V(1));
                    match o {
                        VBin::Add => self.a.vadd(d, a, b),
                        VBin::Sub => self.a.vsub(d, a, b),
                        VBin::And => self.a.vand(d, a, b),
                        VBin::Or => self.a.vorr(d, a, b),
                        _ => self.a.veor(d, a, b),
                    }
                }
                self.finv(r, d);
            }
            Op::Load(t, p) => {
                let pr = self.rd(*p, X9);
                let r = res.unwrap();
                match t {
                    IrTy::V2I64 => {
                        let d = self.wrv(r, V(0));
                        self.a.ldr_f(d, pr, 0, 16);
                        self.finv(r, d);
                    }
                    IrTy::F32 | IrTy::F64 => {
                        let xd = self.wrf(r, V(0));
                        self.a.ldr_f(xd, pr, 0, mem_size(*t));
                        self.finf(r, xd, f);
                    }
                    _ => {
                        let rd = self.wr(r, X10);
                        self.a.ldr(rd, pr, 0, mem_size(*t));
                        self.fin(r, rd);
                    }
                }
            }
            Op::Store(p, x) => {
                let t = f.ty(*x);
                let pr = self.rd(*p, X9);
                if t == IrTy::V2I64 {
                    let xr = self.rdv(*x, V(0));
                    self.a.str_f(xr, pr, 0, 16);
                } else if is_float(t) {
                    let xr = self.rdf(*x, V(0), f);
                    self.a.str_f(xr, pr, 0, mem_size(t));
                } else if self.imm[x.0 as usize] == Some(0) {
                    self.a.str(XZR, pr, 0, mem_size(t));
                } else {
                    let xr = self.rd(*x, X10);
                    self.a.str(xr, pr, 0, mem_size(t));
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

    /// A `Checked` or `CheckedIf` whose value nobody reads, followed by the wrapping `Bin` of the same
    /// operation on the same operands: the result value of that `Bin`.
    fn guarded_by(&self, f: &Func, uses: &[u32], check: usize, next: usize) -> Option<Value> {
        let c = &f.insts[check];
        if c.result.is_some_and(|r| uses[r.0 as usize] > 0) {
            return None;
        }
        let (op, a, b) = match &c.op {
            Op::Checked(op, _, a, b) | Op::CheckedIf(op, _, _, a, b) => (*op, *a, *b),
            _ => return None,
        };
        match (&f.insts[next].op, f.insts[next].result) {
            (Op::Bin(op2, a2, b2), Some(r)) if *op2 == op && *a2 == a && *b2 == b => {
                // The value is one the plain operation would have made; floats are never checked.
                (!is_float(f.ty(r))).then_some(r)
            }
            _ => None,
        }
    }

    fn checked_inst(&mut self, f: &Func, op: &Op, r: Value) {
        match op {
            Op::Checked(op, signed, a, b) => {
                let ov = self.stop_label(Stop::Overflow);
                self.checked_to(f, *op, *signed, *a, *b, r, ov);
            }
            Op::CheckedIf(op, signed, test, a, b) => {
                let (label, resume) = (self.a.new_label(), self.a.new_label());
                self.resume = Some(resume);
                self.checked_to(f, *op, *signed, *a, *b, r, label);
                self.cond_stubs.push(CondStub {
                    label,
                    resume,
                    test: *test,
                });
            }
            _ => unreachable!("not a checked operation"),
        }
    }

    fn bin(&mut self, f: &Func, op: BinOp, a: Value, b: Value, r: Value) -> Result<(), String> {
        let t = f.ty(a);
        if is_float(t) {
            let dbl = t == IrTy::F64;
            if matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div) {
                let (xa, xb) = (self.rdf(a, V(0), f), self.rdf(b, V(1), f));
                let xd = self.wrf(r, V(0));
                match op {
                    BinOp::Add => self.a.fadd(dbl, xd, xa, xb),
                    BinOp::Sub => self.a.fsub(dbl, xd, xa, xb),
                    BinOp::Mul => self.a.fmul(dbl, xd, xa, xb),
                    _ => self.a.fdiv(dbl, xd, xa, xb),
                }
                self.finf(r, xd, f);
                return Ok(());
            }
            self.ldf(V(0), a, f);
            self.ldf(V(1), b, f);
            match op {
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
                // A constant on the left of a commutative operation goes to the right.
                let (a, b) = if op != BinOp::Sub
                    && self.imm[a.0 as usize].is_some()
                    && self.imm[b.0 as usize].is_none()
                {
                    (b, a)
                } else {
                    (a, b)
                };
                let ra = self.rd(a, X9);
                let rd = self.wr(r, X9);
                let done = match (self.imm[b.0 as usize], op) {
                    (Some(n), BinOp::Add | BinOp::Sub) => match addsub_imm(n) {
                        Some((flip, v)) => {
                            if (op == BinOp::Add) != flip {
                                self.a.add_ri(rd, ra, v);
                            } else {
                                self.a.sub_ri(rd, ra, v);
                            }
                            true
                        }
                        None => false,
                    },
                    (Some(n), BinOp::And) => self.a.and_ri(rd, ra, n as u64),
                    (Some(n), BinOp::Or) => self.a.orr_ri(rd, ra, n as u64),
                    (Some(n), BinOp::Xor) => self.a.eor_ri(rd, ra, n as u64),
                    (Some(n), BinOp::Mul) if n > 1 && n & (n - 1) == 0 => {
                        self.a.lsl_ri(rd, ra, n.trailing_zeros());
                        true
                    }
                    _ => false,
                };
                if !done {
                    let rb = self.rd(b, X10);
                    match op {
                        BinOp::Add => self.a.add_rr(rd, ra, rb),
                        BinOp::Sub => self.a.sub_rr(rd, ra, rb),
                        BinOp::Mul => self.a.mul(rd, ra, rb),
                        BinOp::And => self.a.and_rr(rd, ra, rb),
                        BinOp::Or => self.a.orr_rr(rd, ra, rb),
                        _ => self.a.eor_rr(rd, ra, rb),
                    }
                }
                if !matches!(op, BinOp::And | BinOp::Or | BinOp::Xor) {
                    self.mask(rd, w);
                }
                self.fin(r, rd);
            }
            BinOp::Div | BinOp::Rem | BinOp::UDiv | BinOp::URem => {
                let signed = matches!(op, BinOp::Div | BinOp::Rem);
                let want_rem = matches!(op, BinOp::Rem | BinOp::URem);
                let ra = self.rd_ext(a, X9, f, signed);
                let rb = self.rd_ext(b, X10, f, signed);
                let rd = self.wr(r, X11);
                let z = self.stop_label(Stop::Div0);
                self.a.cbz(rb, z);
                // `MIN / -1` does not trap here: it wraps to `MIN` (and the remainder is 0), like the interpreter.
                let q = if want_rem { X11 } else { rd };
                if signed {
                    self.a.sdiv(q, ra, rb);
                } else {
                    self.a.udiv(q, ra, rb);
                }
                if want_rem {
                    self.a.msub(rd, q, rb, ra);
                }
                self.mask(rd, w);
                self.fin(r, rd);
            }
            BinOp::MulHi | BinOp::UMulHi => {
                let signed = op == BinOp::MulHi;
                let ra = self.rd_ext(a, X9, f, signed);
                let rb = self.rd_ext(b, X10, f, signed);
                let rd = self.wr(r, X11);
                if w == 64 {
                    if signed {
                        self.a.smulh(rd, ra, rb);
                    } else {
                        self.a.umulh(rd, ra, rb);
                    }
                } else {
                    self.a.mul(rd, ra, rb);
                    if signed {
                        self.a.asr_ri(rd, rd, w);
                    } else {
                        self.a.lsr_ri(rd, rd, w);
                    }
                    self.mask(rd, w);
                }
                self.fin(r, rd);
            }
            BinOp::Shl | BinOp::Shr | BinOp::UShr => {
                let rd = self.wr(r, X9);
                if let Some(n) = self.imm[b.0 as usize].filter(|n| (0..w as i64).contains(n)) {
                    // A constant amount below the width: no check, an immediate shift.
                    let ra = self.rd_ext(a, X9, f, op == BinOp::Shr);
                    match op {
                        BinOp::Shl => self.a.lsl_ri(rd, ra, n as u32),
                        BinOp::Shr => self.a.asr_ri(rd, ra, n as u32),
                        _ => self.a.lsr_ri(rd, ra, n as u32),
                    }
                } else {
                    let rb = self.rd(b, X10);
                    self.a.cmp_ri(rb, w);
                    let s = self.stop_label(Stop::Shift);
                    self.a.b_cond(Cond::Hs, s);
                    let ra = self.rd_ext(a, X9, f, op == BinOp::Shr);
                    match op {
                        BinOp::Shl => self.a.lslv(rd, ra, rb),
                        BinOp::Shr => self.a.asrv(rd, ra, rb),
                        _ => self.a.lsrv(rd, ra, rb),
                    }
                }
                self.mask(rd, w);
                self.fin(r, rd);
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
        // A signed 64-bit product by 3 is `x + 2x`, which the adder checks: when `2x` overflows so does
        // `3x`, otherwise the second addition's flag tells. The product itself is one shifted add, off
        // the path of the check.
        if w == 64 && signed && op == BinOp::Mul {
            let x = match (self.imm[a.0 as usize], self.imm[b.0 as usize]) {
                (_, Some(3)) => Some(a),
                (Some(3), _) if self.imm[b.0 as usize].is_none() => Some(b),
                _ => None,
            };
            if let Some(x) = x {
                let rx = self.rd(x, X9);
                let rd = self.wr(r, X11);
                self.a.add_rr_sh(rd, rx, rx, Shift::Lsl, 1);
                self.a.adds_rr(X12, rx, rx);
                self.a.ccmn(X12, rx, 1, Cond::Vc);
                self.ov_jump(Cond::Vs, ov);
                self.fin(r, rd);
                return;
            }
        }
        let ra = self.rd_ext(a, X9, f, signed);
        let rd = self.wr(r, X11);
        if w == 64 {
            // A constant that fits an immediate: one instruction, no register for it.
            if let (Some(n), BinOp::Add | BinOp::Sub) = (self.imm[b.0 as usize], op) {
                let imm = match addsub_imm(n) {
                    Some((false, v)) => Some((false, v)),
                    // `a + -k` is `a - k` for the signed overflow flag, not for the carry.
                    Some((true, v)) if signed => Some((true, v)),
                    _ => None,
                };
                if let Some((flip, v)) = imm {
                    let sub = (op == BinOp::Sub) != flip;
                    if sub {
                        self.a.subs_ri(rd, ra, v);
                    } else {
                        self.a.adds_ri(rd, ra, v);
                    }
                    let cc = match (sub, signed) {
                        (_, true) => Cond::Vs,
                        (false, false) => Cond::Hs,
                        (true, false) => Cond::Lo,
                    };
                    self.ov_jump(cc, ov);
                    self.fin(r, rd);
                    return;
                }
            }
        }
        let rb = self.rd_ext(b, X10, f, signed);
        if w == 64 {
            match (op, signed) {
                (BinOp::Add, true) => {
                    self.a.adds_rr(rd, ra, rb);
                    self.ov_jump(Cond::Vs, ov);
                }
                (BinOp::Sub, true) => {
                    self.a.subs_rr(rd, ra, rb);
                    self.ov_jump(Cond::Vs, ov);
                }
                (BinOp::Add, false) => {
                    self.a.adds_rr(rd, ra, rb);
                    self.ov_jump(Cond::Hs, ov);
                }
                (BinOp::Sub, false) => {
                    self.a.subs_rr(rd, ra, rb);
                    // Borrow: the carry is clear.
                    self.ov_jump(Cond::Lo, ov);
                }
                (_, true) => {
                    // The product fits when the high half is the sign extension of the low half.
                    self.a.mul(rd, ra, rb);
                    self.a.smulh(X12, ra, rb);
                    self.a.cmp_rr_sh(X12, rd, Shift::Asr, 63);
                    self.ov_jump(Cond::Ne, ov);
                }
                (_, false) => {
                    self.a.mul(rd, ra, rb);
                    self.a.umulh(X12, ra, rb);
                    self.a.cmp_ri(X12, 0);
                    self.ov_jump(Cond::Ne, ov);
                }
            }
        } else {
            // Narrow: no overflow in 64 bits; the result must survive the trip through the width.
            match op {
                BinOp::Add => self.a.add_rr(rd, ra, rb),
                BinOp::Sub => self.a.sub_rr(rd, ra, rb),
                _ => self.a.mul(rd, ra, rb),
            }
            if signed {
                self.a.sxt(X12, rd, w);
            } else {
                self.a.uxt(X12, rd, w);
            }
            self.a.cmp_rr(X12, rd);
            self.ov_jump(Cond::Ne, ov);
            self.mask(rd, w);
        }
        self.fin(r, rd);
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
        let rd = self.wr(r, X9);
        self.a.cset(rd, cc);
        self.fin(r, rd);
    }

    /// Compares two floats; gives the condition that holds when `op` is true. Unordered operands (a
    /// NaN) make every condition here false, except `!=`.
    fn float_cmp(&mut self, f: &Func, op: CmpOp, a: Value, b: Value) -> Cond {
        let d = f.ty(a) == IrTy::F64;
        let (xa, xb) = (self.rdf(a, V(0), f), self.rdf(b, V(1), f));
        self.a.fcmp(d, xa, xb);
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
        let ra = self.rd_ext(a, ra, f, signed);
        match self.imm[b.0 as usize].map(|n| widen(n, f.ty(b), signed)) {
            Some(n) if (0..4096).contains(&n) => self.a.cmp_ri(ra, n as u32),
            _ => {
                let rb = self.rd_ext(b, rb, f, signed);
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
