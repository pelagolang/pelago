//! The simple code generator (stage N2): every IR value lives in its own 8-byte stack slot and
//! each operation is a fixed template over `rax rcx rdx` and `xmm0 xmm1`. Slow, small and easy to
//! trust; register allocation (N5) replaces it later without changing what the tests expect.
//!
//! Representation: an integer or boolean value is kept **zero-extended** to 64 bits in its slot
//! ("canonical form"), a pointer is 64 bits, a float is 4 or 8 bytes in the low part of the slot.
//! Operations that need the sign extend from the slot with `movsx`; results are cut back to their
//! width. The rules for division, shifts, overflow and NaN are in `docs/design/backend.md`.

use super::elf::{self, Blob, Symbol};
use super::regs::{Base, Fold, Place};
use super::rt::{self, Runtime};
use super::x86::*;
use super::{pe, Os};
use crate::ir::{
    BinOp, CastKind, CmpOp, Const, Func, FuncId, Inst, IrTy, Module, Op, Target, Term, Value,
};
use crate::ir::{FUn, Test, VBin, VUn};
use std::collections::{HashMap, HashSet};

const INT_REGS: [Reg; 6] = [RDI, RSI, RDX, RCX, R8, R9];
const MAX_FLOAT_REGS: usize = 8;

/// The SSE registers values can live in. `xmm0` and `xmm1` are the templates' scratch registers (and
/// `xmm0` carries a float result); the rest belong to the caller, so a value that lives across a call
/// is stored before it and loaded after, like the caller-saved general registers.
const XMM_REGS: [Xmm; 14] = [
    Xmm(2),
    Xmm(3),
    Xmm(4),
    Xmm(5),
    Xmm(6),
    Xmm(7),
    Xmm(8),
    Xmm(9),
    Xmm(10),
    Xmm(11),
    Xmm(12),
    Xmm(13),
    Xmm(14),
    Xmm(15),
];

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
        // For debugging the allocators: `PELAGO_REG_LIMIT=2` keeps two registers of each kind.
        static LIMIT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *LIMIT.get_or_init(|| {
            std::env::var("PELAGO_REG_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(usize::MAX)
        })
    }
}

thread_local! {
    /// Tests choose the machine IR path (`Some(true)`) or the older generator (`Some(false)`);
    /// otherwise `PELAGO_MIR=1` turns it on (work item W4, default off until it matches the old path).
    pub static MIR: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

fn mir_mode() -> bool {
    MIR.with(|m| m.get()).unwrap_or_else(|| {
        static ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENV.get_or_init(|| std::env::var("PELAGO_MIR").is_ok_and(|v| v == "1"))
    })
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
    /// A float or a vector in an SSE register.
    Xmm(Xmm),
    /// A vector in memory (16 bytes at this offset from `rbp`).
    VMem(i32),
    /// A constant integer, boolean or pointer: no storage, loaded or used as an immediate where it is read.
    Imm(i64),
    /// A value nothing reads (or a compare fused into its branch): writes to it are dropped.
    Dead,
    /// Only inside [`Gen::moves`]: the value pushed on the stack to break a cycle of copies.
    Stack,
    /// The address of a frame object (plus a constant): no storage, `lea` where it is needed.
    Frame(i32),
    /// A pointer that is never computed: its users name its address in their memory operand
    /// (`Gen::mem_of`, `Fold`).
    Folded,
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
    let rt = rt::emit_for(
        &mut g.a,
        &mut g.ro,
        &mut g.data,
        main_label,
        opts.leak_check,
        opts.target,
        super::Uses::of(m),
    );
    g.rt = Some(rt);
    g.place_data();
    g.stop_stubs();
    let mut syms = Vec::new();
    for (i, f) in m.funcs.iter().enumerate() {
        // A function starts 16 bytes before a cache line: where the hot code of a function falls
        // in its lines (and so what a call and return cost) must not depend on how big the runtime
        // in front of it is. Measured on `fib` and `tak`, a start 48 bytes into a line is the best
        // of the four (the worst of them is 25 percent slower on `fib`).
        g.a.align(64, 63);
        g.a.nops(48);
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
        Os::MacOs => Err("macOS programs are built for AArch64 only (use --arch=aarch64)".into()),
    }
}

/// Out-of-line copies for one edge of a branch: where they start, the copies `(to, from)` and the block
/// to jump to afterwards.
struct Stub {
    label: Label,
    copies: Vec<(Where, Where)>,
    to: usize,
}

/// After the overflow jump of a `CheckedIf`: stop if the test holds, else go back to `resume`.
struct CondStub {
    label: Label,
    resume: Label,
    test: Test,
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
    /// The flags still hold the integer comparison that made this bool (`select` can use them).
    flags: Option<(Value, Cc)>,
    /// Set while a `CheckedIf` is emitted: where its out-of-line test comes back to.
    resume: Option<Label>,
    /// The out-of-line tests of the `CheckedIf`s of this function.
    cond_stubs: Vec<CondStub>,
    regalloc: bool,
    /// Per function: where each value lives, the frame offset of each `Slot`, and where the
    /// callee-saved registers in use are kept.
    loc: Vec<Where>,
    fold: Vec<Fold>,
    saves: Vec<(Reg, i32)>,
    /// Per call-like instruction: the values in caller-saved registers that live across it, as the
    /// register and the frame offset where the value waits during the operation.
    across: HashMap<usize, Vec<(Where, i32)>>,
    /// Frame offset of 16 bytes where the operands of a float remainder wait (the x87 unit reads memory).
    fscratch: i32,
    /// The function uses no frame (no spills, slots or saved registers), calls nothing and takes all its
    /// parameters in registers: it runs without `rbp`.
    frameless: bool,
    /// The frame holds nothing but the callee-saved registers (no spills, slots or waiting values): they
    /// are pushed and popped, there is no `rbp`, and `pad` keeps the stack 16-byte aligned for calls.
    light: bool,
    pad: bool,
    /// The block and the position in it of the instruction being generated.
    at: (usize, usize),
    /// Frame offsets already written in the current block (a value is stored once per block).
    stored: HashSet<i32>,
    /// Copies of block parameters kept out of line: label, the copies, and the block to jump to.
    stubs: Vec<Stub>,
    slot_off: HashMap<usize, i32>,
    /// Big slots that a loop fills completely before anything else touches them: not cleared.
    filled: HashSet<usize>,
    /// Loops that fill memory with one value, done by a string store instead.
    fills: Vec<crate::ir::opt::FillLoop>,
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

/// A loop that stores at least this many bytes of one value is a `rep stos`: it starts slowly.
const FILL_BYTES: i128 = 512;

fn size_of_ty(t: IrTy) -> i32 {
    size(t) as i32
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
        a.pad_jumps = true;
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
            flags: None,
            resume: None,
            cond_stubs: Vec::new(),
            regalloc: true,
            loc: Vec::new(),
            fold: Vec::new(),
            saves: Vec::new(),
            across: HashMap::new(),
            fscratch: 0,
            frameless: false,
            light: false,
            pad: false,
            at: (0, 0),
            stored: HashSet::new(),
            stubs: Vec::new(),
            slot_off: HashMap::new(),
            filled: HashSet::new(),
            fills: Vec::new(),
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

    fn layout(&mut self, f: &Func) {
        self.slot_off.clear();
        self.filled = crate::ir::opt::filled_slots(f);
        self.fills = crate::ir::opt::fill_loops(f, FILL_BYTES);
        let imms = const_values(f);
        let uses = f.use_counts();
        let folds = super::regs::folds(f, &uses, &imms);
        let alloc = if self.regalloc {
            let n = reg_limit();
            let saved: Vec<Reg> = VALUE_REGS.iter().copied().take(n).collect();
            let scratch: Vec<Reg> = SCRATCH_REGS.iter().copied().take(n).collect();
            let xmm: Vec<Xmm> = XMM_REGS.iter().copied().take(n).collect();
            let cold: Vec<bool> = self
                .m
                .funcs
                .iter()
                .map(|g| g.name.ends_with(crate::ir::opt::COLD_SUFFIX))
                .collect();
            super::regs::assign(f, &saved, &scratch, &xmm, &INT_REGS, &folds, &imms, &cold)
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
                _ if folds[v] != Fold::No => Where::Folded,
                _ if fused[v] || (uses[v] == 0 && !is_float(f.values[v])) => Where::Dead,
                (None, Some(Place::Gpr(r))) => Where::Reg(*r),
                (None, Some(Place::Xmm(x))) => Where::Xmm(*x),
                (None, None) if f.values[v] == IrTy::V2I64 => {
                    at += 16;
                    Where::VMem(-at)
                }
                (None, None) => {
                    at += 8;
                    Where::Mem(-at)
                }
            });
        }
        let values_in_memory = at > 0;
        // Room to keep the callee-saved registers this function uses.
        self.saves.clear();
        for r in VALUE_REGS {
            if alloc.reg.contains(&Some(Place::Gpr(r))) {
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
                let place = match alloc.reg[v.0 as usize] {
                    Some(Place::Gpr(r)) => Where::Reg(r),
                    Some(Place::Xmm(x)) => Where::Xmm(x),
                    None => unreachable!("a split value has a register"),
                };
                list.push((place, off));
            }
            self.across.insert(id, list);
        }
        // The x87 unit reads its operands from memory: a float remainder needs somewhere to put them.
        let has_frem = f
            .blocks
            .iter()
            .flat_map(|b| &b.insts)
            .any(|&i| matches!(f.insts[i].op, Op::Bin(BinOp::Rem, a, _) if is_float(f.ty(a))));
        if has_frem {
            at += 16;
            self.fscratch = -at;
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
        // Without memory values, float scratch and slots, the saved registers are pushed and the values
        // waiting across a call are pushed around it: no `rbp` and no frame.
        self.light = self.regalloc
            && !values_in_memory
            && !has_frem
            && !f
                .blocks
                .iter()
                .flat_map(|b| &b.insts)
                .any(|&i| matches!(f.insts[i].op, Op::Slot(..)));
        // Frame objects and what is added to them are plain frame offsets.
        for (v, fold) in folds.iter().enumerate() {
            if let Fold::Addr {
                base: Base::Slot(i),
                index: None,
                disp,
            } = fold
            {
                self.loc[v] = Where::Frame(self.slot_off[i] + disp);
            }
        }
        self.fold = folds;
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
            Where::Frame(off) => self.a.lea(r, mem(RBP, off)),
            Where::Xmm(x) => self.a.movq_from_xmm(r, x, true),
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
            Where::Frame(off) => self.a.lea(r, mem(RBP, off)),
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
            w => unreachable!("a write to {w:?}"),
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

    /// Loads a float into the given SSE register.
    fn ldf(&mut self, x: Xmm, v: Value, f: &Func) {
        match self.loc[v.0 as usize] {
            Where::Xmm(y) => {
                if x != y {
                    self.a.fmov_rr(x, y);
                }
            }
            Where::Mem(off) => self.a.fload(x, mem(RBP, off), f.ty(v) == IrTy::F64),
            w => panic!("float {} is read from {w:?}", v.0),
        }
    }

    /// Writes a float from the given SSE register to where the value lives.
    fn stf(&mut self, v: Value, x: Xmm, f: &Func) {
        match self.loc[v.0 as usize] {
            Where::Xmm(y) => {
                if x != y {
                    self.a.fmov_rr(y, x);
                }
            }
            Where::Mem(off) => self.a.fstore(mem(RBP, off), x, f.ty(v) == IrTy::F64),
            w => panic!("float {} is written to {w:?}", v.0),
        }
    }

    /// An SSE register holding the vector: its own, else `scratch` after loading it there.
    fn vsrc(&mut self, v: Value, scratch: Xmm) -> Xmm {
        match self.loc[v.0 as usize] {
            Where::Xmm(x) => x,
            Where::VMem(off) => {
                self.a.movups_load(scratch, mem(RBP, off));
                scratch
            }
            w => panic!("vector {} is read from {w:?}", v.0),
        }
    }

    /// The SSE register a vector result is built in: its own when it has one, `xmm0` otherwise.
    fn vdst(&self, v: Value) -> Xmm {
        match self.loc[v.0 as usize] {
            Where::Xmm(x) => x,
            _ => Xmm(0),
        }
    }

    /// Puts the vector built in `x` where the value lives.
    fn vst(&mut self, v: Value, x: Xmm) {
        match self.loc[v.0 as usize] {
            Where::Xmm(y) => {
                if x != y {
                    self.a.fmov_rr(y, x);
                }
            }
            Where::VMem(off) => self.a.movups_store(mem(RBP, off), x),
            Where::Dead => {}
            w => panic!("vector {} is written to {w:?}", v.0),
        }
    }

    /// The SSE register a float result is built in: its own when it has one, `xmm0` otherwise.
    fn fdst(&self, v: Value) -> Xmm {
        match self.loc[v.0 as usize] {
            Where::Xmm(x) => x,
            _ => Xmm(0),
        }
    }

    /// An SSE register holding the float: its own, else `scratch` after loading it there.
    fn fsrc(&mut self, v: Value, f: &Func, scratch: Xmm) -> Xmm {
        if let Where::Xmm(x) = self.loc[v.0 as usize] {
            return x;
        }
        self.ldf(scratch, v, f);
        scratch
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
        if self.regalloc && mir_mode() {
            if let Some(mut mf) = super::x86_select::select(f) {
                let target = super::x86_mir::X86::new();
                if cfg!(debug_assertions) {
                    super::mir::verify(&mf, &target)?;
                }
                let dump = std::env::var("PELAGO_MIR_DUMP").is_ok_and(|v| v == "1");
                if dump {
                    eprintln!("{}", mf.print(&target));
                }
                super::ralloc::allocate(&mut mf, &target, reg_limit());
                if dump {
                    eprintln!("after allocation:\n{}", mf.print(&target));
                }
                let label = self.funcs[fi];
                let mut env = super::x86_emit::Env {
                    a: &mut self.a,
                    ro: &mut self.ro,
                    funcs: &self.funcs,
                    data_off: &self.data_off,
                    stops: [
                        self.stop_div0,
                        self.stop_shift,
                        self.stop_overflow,
                        self.stop_unreachable,
                    ],
                    rt: self.rt.as_ref().expect("the runtime is emitted first"),
                };
                super::x86_emit::emit(&mut env, &mf, label);
                return Ok(());
            }
        }
        self.layout(f);
        self.ret_ty = f.ret;
        self.block_labels = f.blocks.iter().map(|_| self.a.new_label()).collect();
        let l = self.funcs[fi];
        self.a.bind(l);
        // Parameters: registers or the caller's stack.
        let entry = f.entry();
        let params: Vec<Value> = f.block_params(entry).to_vec();
        let tys: Vec<IrTy> = params.iter().map(|p| f.ty(*p)).collect();
        let calls = f.blocks.iter().flat_map(|b| &b.insts).any(|&i| {
            matches!(
                f.insts[i].op,
                Op::Call(..)
                    | Op::CallPtr(..)
                    | Op::Alloc(_)
                    | Op::Free(_)
                    | Op::Realloc(..)
                    | Op::Print(..)
            )
        });
        self.frameless = self.regalloc
            && self.frame == 0
            && self.saves.is_empty()
            && !calls
            && !classify(&tys).iter().any(|l| matches!(l, Loc::Stack(_)));
        let stack_params = classify(&tys).iter().any(|l| matches!(l, Loc::Stack(_)));
        let light = self.light && !self.frameless && !stack_params;
        self.pad = false;
        if light {
            // Entry has the stack 8 off a 16-byte boundary; each push moves it by 8.
            for (r, _) in self.saves.clone() {
                self.a.push(r);
            }
            if calls && self.saves.len().is_multiple_of(2) {
                self.a.alu_ri(Alu::Sub, RSP, 8, 8);
                self.pad = true;
            }
        } else if !self.frameless {
            self.a.push(RBP);
            self.a.mov_rr(RBP, RSP, 8);
            if self.frame > 0 {
                // A big frame must not step over the guard: touch each page, top down, first.
                let pages = self.frame / 4096;
                if pages <= 8 {
                    for k in 1..=pages {
                        self.a.alu_mi(Alu::Or, mem(RSP, -4096 * k), 0, 8);
                    }
                } else {
                    // A loop, not a probe per page in the code: a 40 MB array is 10 000 of them.
                    let top = self.a.new_label();
                    self.a.lea(R11, mem(RSP, -4096 * pages));
                    self.a.mov_rr(R10, RSP, 8);
                    self.a.bind(top);
                    self.a.alu_ri(Alu::Sub, R10, 4096, 8);
                    self.a.alu_mi(Alu::Or, mem(R10, 0), 0, 8);
                    self.a.alu_rr(Alu::Cmp, R10, R11, 8);
                    self.a.jcc(Cc::A, top);
                }
                self.a.alu_ri(Alu::Sub, RSP, self.frame, 8);
            }
            // Keep the callee-saved registers this function uses.
            for (r, off) in self.saves.clone() {
                self.a.store(mem(RBP, off), r, 8);
            }
        }
        self.light = light;
        let mut copies = Vec::new();
        for (p, loc) in params.iter().zip(classify(&tys)) {
            match loc {
                Loc::Int(i) => copies.push((self.loc[p.0 as usize], Where::Reg(INT_REGS[i]))),
                Loc::Float(i) => copies.push((self.loc[p.0 as usize], Where::Xmm(Xmm(i as u8)))),
                Loc::Stack(k) => {
                    copies.push((self.loc[p.0 as usize], Where::Mem(16 + 8 * k as i32)))
                }
            }
        }
        self.moves(&copies);
        let uses = f.use_counts();
        // The blocks a jump back leads to start a loop.
        let mut heads = vec![false; f.blocks.len()];
        for (bi, b) in f.blocks.iter().enumerate() {
            for t in b.term.iter().flat_map(super::regs::successors) {
                if t <= bi {
                    heads[t] = true;
                }
            }
        }
        // The header and the body of a fill loop are not generated: its entry block stores the memory.
        let mut skipped = vec![false; f.blocks.len()];
        for l in &self.fills {
            skipped[l.head.0 as usize] = true;
            skipped[l.body.0 as usize] = true;
        }
        for (bi, b) in f.blocks.iter().enumerate() {
            if skipped[bi] {
                continue;
            }
            let lb = self.block_labels[bi];
            // A loop starts on a 16-byte boundary (when that costs at most 15 bytes of no-ops: a loop whose head
            // sat 9 to 15 bytes past a boundary ran 30 percent slower than the same loop aligned).
            if heads[bi] {
                self.a.align(16, 15);
            }
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
            for (k, &i) in b.insts.iter().enumerate() {
                if fused.is_some_and(|(l, ..)| l == i) {
                    continue;
                }
                self.at = (bi, k);
                if !matches!(f.insts[i].op, Op::Select(..)) {
                    self.flags = None;
                }
                // A comparison read only by the selects right after it is not made into a bool: they
                // move on its flags.
                if let (Op::Cmp(op, x, y), Some(r)) = (&f.insts[i].op, f.insts[i].result) {
                    let selects = b.insts[k + 1..]
                        .iter()
                        .take_while(|&&j| matches!(&f.insts[j].op, Op::Select(c, ..) if *c == r))
                        .count();
                    if selects > 0 && selects == uses[r.0 as usize] as usize && !is_float(f.ty(*x))
                    {
                        let cc = self.int_cmp(f, *op, *x, *y);
                        self.flags = Some((r, cc));
                        continue;
                    }
                }
                self.inst(f, i, &f.insts[i])?;
            }
            // The block emitted after this one (the skipped ones are not).
            let next = (bi + 1..f.blocks.len()).find(|j| !skipped[*j]);
            if let Some(k) = self.fills.iter().position(|l| l.pre.0 as usize == bi) {
                self.fill(f, k);
                let exit = Target {
                    block: self.fills[k].exit,
                    args: Vec::new(),
                };
                self.edge(f, &exit, next);
                continue;
            }
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
        // The tests of the `CheckedIf`s. They keep every register as it was.
        for CondStub {
            label,
            resume,
            test,
        } in std::mem::take(&mut self.cond_stubs)
        {
            self.a.bind(label);
            let stop = self.stop_overflow;
            match test {
                Test::Bool(cond, when) => {
                    match self.loc[cond.0 as usize] {
                        Where::Reg(r) => self.a.test_rr(r, r, 4),
                        Where::Mem(off) => self.a.alu_mi(Alu::Cmp, mem(RBP, off), 0, 8),
                        w => return Err(format!("a condition in {w:?}")),
                    }
                    self.a.jcc(if when { Cc::E } else { Cc::Ne }, resume);
                }
                Test::Cmp(op, x, y) => {
                    // The comparison borrows `rax` and `rcx`; `pop` leaves the flags alone.
                    self.a.push(RAX);
                    self.a.push(RCX);
                    let cc = self.int_cmp(f, op, x, y);
                    self.a.pop(RCX);
                    self.a.pop(RAX);
                    self.a.jcc(cc.invert(), resume);
                }
            }
            self.a.jmp(stop);
        }
        Ok(())
    }

    fn epilogue(&mut self) {
        if self.light {
            if self.pad {
                self.a.alu_ri(Alu::Add, RSP, 8, 8);
            }
            for (r, _) in self.saves.clone().into_iter().rev() {
                self.a.pop(r);
            }
            self.a.ret();
            return;
        }
        for (r, off) in self.saves.clone() {
            self.a.load(r, mem(RBP, off), 8);
        }
        if !self.frameless {
            self.a.mov_rr(RSP, RBP, 8);
            self.a.pop(RBP);
        }
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
                let start = self.a.here();
                let cc = match fused {
                    Some((_, op, x, y)) if is_float(f.ty(x)) => self.float_cmp(f, op, x, y),
                    Some((_, op, x, y)) => self.int_cmp(f, op, x, y),
                    None => {
                        let rc = self.src(*c, f, false, RAX);
                        self.a.test_rr(rc, rc, 1);
                        Cc::Ne
                    }
                };
                // The compare and the jump after it run as one micro-operation: keep them in one 32-byte block.
                self.a.guard_jump(start, 6);
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
                        // A whole register: it may hold a vector.
                        Where::Xmm(x) => {
                            self.a.alu_ri(Alu::Sub, RSP, 16, 8);
                            self.a.movups_store(mem(RSP, 0), x);
                        }
                        Where::VMem(off) => {
                            self.a.movups_load(Xmm(0), mem(RBP, off));
                            self.a.alu_ri(Alu::Sub, RSP, 16, 8);
                            self.a.movups_store(mem(RSP, 0), Xmm(0));
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
            (Where::Reg(r), Where::Frame(off)) => self.a.lea(r, mem(RBP, off)),
            (Where::Reg(r), Where::Stack) => self.a.pop(r),
            (Where::Xmm(x), Where::Xmm(y)) => self.a.fmov_rr(x, y),
            (Where::Xmm(x), Where::Mem(off)) => self.a.fload(x, mem(RBP, off), true),
            (Where::Mem(off), Where::Xmm(y)) => self.a.fstore(mem(RBP, off), y, true),
            (Where::Xmm(x), Where::Stack) => {
                self.a.movups_load(x, mem(RSP, 0));
                self.a.lea(RSP, mem(RSP, 16));
            }
            (Where::VMem(off), Where::Stack) => {
                self.a.movups_load(Xmm(0), mem(RSP, 0));
                self.a.lea(RSP, mem(RSP, 16));
                self.a.movups_store(mem(RBP, off), Xmm(0));
            }
            (Where::Xmm(x), Where::VMem(off)) => self.a.movups_load(x, mem(RBP, off)),
            (Where::VMem(off), Where::Xmm(y)) => self.a.movups_store(mem(RBP, off), y),
            (Where::VMem(off), Where::VMem(from)) => {
                self.a.movups_load(Xmm(0), mem(RBP, from));
                self.a.movups_store(mem(RBP, off), Xmm(0));
            }
            (Where::Mem(off), Where::Reg(x)) => self.a.store(mem(RBP, off), x, 8),
            (Where::Mem(off), Where::Imm(n)) if i32::try_from(n).is_ok() => {
                self.a.store_imm(mem(RBP, off), n as i32, 8)
            }
            (Where::Mem(off), s) => {
                match s {
                    Where::Mem(from) => self.a.load(RAX, mem(RBP, from), 8),
                    Where::Imm(n) => self.a.mov_ri(RAX, n),
                    Where::Frame(from) => self.a.lea(RAX, mem(RBP, from)),
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
        // Values waiting in caller-saved registers go to their frame slot for the length of a call (or
        // are pushed, in a function without a frame).
        let keep = self.across.get(&id).cloned().unwrap_or_default();
        if self.light && !keep.is_empty() {
            for &(w, _) in &keep {
                match w {
                    Where::Reg(r) => self.a.push(r),
                    Where::Xmm(x) => {
                        self.a.alu_ri(Alu::Sub, RSP, 8, 8);
                        self.a.fstore(mem(RSP, 0), x, true);
                    }
                    w => unreachable!("{w:?} waits across a call"),
                }
            }
            let odd = keep.len() % 2 == 1;
            if odd {
                self.a.alu_ri(Alu::Sub, RSP, 8, 8);
            }
            self.inst_body(f, id, inst)?;
            if odd {
                self.a.alu_ri(Alu::Add, RSP, 8, 8);
            }
            for &(w, _) in keep.iter().rev() {
                match w {
                    Where::Reg(r) => self.a.pop(r),
                    Where::Xmm(x) => {
                        self.a.fload(x, mem(RSP, 0), true);
                        self.a.alu_ri(Alu::Add, RSP, 8, 8);
                    }
                    w => unreachable!("{w:?} waits across a call"),
                }
            }
            return Ok(());
        }
        for &(w, off) in &keep {
            if self.stored.insert(off) {
                match w {
                    Where::Reg(r) => self.a.store(mem(RBP, off), r, 8),
                    Where::Xmm(x) => self.a.fstore(mem(RBP, off), x, true),
                    w => unreachable!("{w:?} waits across a call"),
                }
            }
        }
        self.inst_body(f, id, inst)?;
        for &(w, off) in &keep {
            match w {
                Where::Reg(r) => self.a.load(r, mem(RBP, off), 8),
                Where::Xmm(x) => self.a.fload(x, mem(RBP, off), true),
                w => unreachable!("{w:?} waits across a call"),
            }
        }
        Ok(())
    }

    fn inst_body(&mut self, f: &Func, id: usize, inst: &Inst) -> Result<(), String> {
        let res = inst.result;
        // A folded pointer or index is never computed (a `Slot` still clears its memory).
        if let (Some(r), false) = (res, matches!(inst.op, Op::Slot(..))) {
            if matches!(self.loc[r.0 as usize], Where::Frame(_) | Where::Folded) {
                return Ok(());
            }
        }
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
                if let Where::Xmm(x) = self.loc[res.unwrap().0 as usize] {
                    if bits_v == 0 {
                        self.a.fxor(x, x);
                    } else {
                        self.a.mov_ri(RAX, bits_v);
                        self.a.movq_to_xmm(x, RAX, true);
                    }
                    return Ok(());
                }
                self.a.mov_ri(RAX, bits_v);
                self.st(res.unwrap(), RAX);
            }
            Op::Bin(op, a, b) => self.bin(f, *op, *a, *b, res.unwrap())?,
            Op::Checked(op, signed, a, b) => self.checked(f, *op, *signed, *a, *b, res.unwrap()),
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
            Op::Select(c, a, b) => self.select(f, *c, *a, *b, res.unwrap()),
            Op::Fma(a, b, c) => {
                // `xmm0 = c`, then `xmm0 = a * b + xmm0` in one rounding (FMA3, a Haswell or newer CPU); `b` is
                // read where it lives (register or frame slot), so no third scratch register is needed.
                let dbl = f.ty(*a) == IrTy::F64;
                self.ldf(Xmm(0), *c, f);
                let xa = self.fsrc(*a, f, Xmm(1));
                let mb = match self.loc[b.0 as usize] {
                    Where::Xmm(x) => Fp::Reg(x),
                    Where::Mem(off) => Fp::Mem(mem(RBP, off)),
                    w => panic!("float {} is read from {w:?}", b.0),
                };
                self.a.vfmadd231(Xmm(0), xa, mb, dbl);
                self.stf(res.unwrap(), Xmm(0), f);
            }
            Op::FUn(o, a) => {
                let dbl = f.ty(*a) == IrTy::F64;
                let r = res.unwrap();
                let s = self.fsrc(*a, f, Xmm(1));
                let d = self.fdst(r);
                match o {
                    FUn::Sqrt => self.a.fsqrt(d, s, dbl),
                    FUn::Floor => self.a.fround(d, s, 1, dbl),
                    FUn::Ceil => self.a.fround(d, s, 2, dbl),
                }
                self.stf(r, d, f);
            }
            Op::Cmp(op, a, b) => self.cmp(f, *op, *a, *b, res.unwrap()),
            Op::Neg(a) => {
                let t = f.ty(*a);
                if is_float(t) {
                    // Flip the sign bit with an `xorps` against a mask built in `xmm1`.
                    let r = res.unwrap();
                    let d = self.fdst(r);
                    self.ldf(d, *a, f);
                    if t == IrTy::F32 {
                        self.a.mov_ri(RAX, 0x8000_0000);
                        self.a.movq_to_xmm(Xmm(1), RAX, false);
                    } else {
                        self.a.mov_ri(RAX, i64::MIN);
                        self.a.movq_to_xmm(Xmm(1), RAX, true);
                    }
                    self.a.fxor(d, Xmm(1));
                    self.stf(r, d, f);
                    return Ok(());
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
                // Memory is zero when a slot comes into use, as in the interpreter, unless the code
                // right after overwrites every byte before anything else touches it.
                let off = self.slot_off[&id];
                if !self.filled.contains(&id) && !self.overwritten_first(f, id, off, *sz as i32) {
                    self.zero_frame(off, *sz as i32);
                }
            }
            Op::PtrAdd(p, n) => self.bin(f, BinOp::Add, *p, *n, res.unwrap())?,
            Op::MemCopy(d, s, n) => {
                if let Where::Imm(len) = self.loc[n.0 as usize] {
                    if super::regs::small_copy(Some(len)) {
                        // Plain moves; an address that needs registers is built in `rcx` and `rdx`.
                        let md = self.mem_of(f, *d, RCX, RAX);
                        let md = self.settle(md, RCX);
                        let ms = self.mem_of(f, *s, RDX, RAX);
                        let ms = self.settle(ms, RDX);
                        self.copy_bytes(md, ms, len as i32);
                        return Ok(());
                    }
                }
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
                self.moves(&[(Where::Reg(RSI), wp), (Where::Reg(RDX), wn)]);
                let w = self.rt().print;
                self.a.call(w);
            }
            Op::VUn(o, a) => {
                let r = res.unwrap();
                if *o == VUn::ReduceAdd {
                    let x = self.vsrc(*a, Xmm(1));
                    self.a.pshufd(Xmm(0), x, 0xEE);
                    self.a.valu(VAlu::Add, Xmm(0), x);
                    self.a.movq_from_xmm(RAX, Xmm(0), true);
                    self.st(r, RAX);
                    return Ok(());
                }
                let d = self.vdst(r);
                match o {
                    VUn::Splat => {
                        self.ld(RAX, *a);
                        self.a.movq_to_xmm(d, RAX, true);
                        self.a.pshufd(d, d, 0x44);
                    }
                    VUn::Shl(k) | VUn::UShr(k) => {
                        let x = self.vsrc(*a, Xmm(0));
                        if x != d {
                            self.a.fmov_rr(d, x);
                        }
                        self.a.vshift(matches!(o, VUn::Shl(_)), d, *k);
                    }
                    VUn::ReduceAdd => unreachable!(),
                }
                self.vst(r, d);
            }
            Op::VBin(o, a, b) => {
                let r = res.unwrap();
                let d = self.vdst(r);
                if *o == VBin::Build {
                    self.ld(RAX, *a);
                    self.ld(RCX, *b);
                    self.a.movq_to_xmm(d, RAX, true);
                    self.a.movq_to_xmm(Xmm(1), RCX, true);
                    self.a.punpcklqdq(d, Xmm(1));
                    self.vst(r, d);
                    return Ok(());
                }
                let alu = match o {
                    VBin::Add => VAlu::Add,
                    VBin::Sub => VAlu::Sub,
                    VBin::And => VAlu::And,
                    VBin::Or => VAlu::Or,
                    _ => VAlu::Xor,
                };
                let (x, y) = (self.vsrc(*a, Xmm(0)), self.vsrc(*b, Xmm(1)));
                if d == y && d != x {
                    if *o == VBin::Sub {
                        // `d = x - d` cannot be done in place: go through `xmm0`.
                        if x != Xmm(0) {
                            self.a.fmov_rr(Xmm(0), x);
                        }
                        self.a.valu(alu, Xmm(0), y);
                        self.a.fmov_rr(d, Xmm(0));
                    } else {
                        self.a.valu(alu, d, x);
                    }
                } else {
                    if d != x {
                        self.a.fmov_rr(d, x);
                    }
                    self.a.valu(alu, d, y);
                }
                self.vst(r, d);
            }
            Op::Load(IrTy::V2I64, p) => {
                let m = self.mem_of(f, *p, RCX, RDX);
                let r = res.unwrap();
                let d = self.vdst(r);
                self.a.movups_load(d, m);
                self.vst(r, d);
            }
            Op::Store(p, x) if f.ty(*x) == IrTy::V2I64 => {
                let m = self.mem_of(f, *p, RCX, RDX);
                let rx = self.vsrc(*x, Xmm(0));
                self.a.movups_store(m, rx);
            }
            Op::Load(t, p) => {
                let m = self.mem_of(f, *p, RCX, RDX);
                let r = res.unwrap();
                match t {
                    IrTy::F32 | IrTy::F64 => {
                        let d = self.fdst(r);
                        self.a.fload(d, m, *t == IrTy::F64);
                        self.stf(r, d, f);
                    }
                    _ => {
                        let d = self.dst(r);
                        self.a.load_ext(d, m, size(*t), false, true);
                        self.st(r, d);
                    }
                }
            }
            Op::Store(p, x) => {
                let t = f.ty(*x);
                let m = self.mem_of(f, *p, RCX, RDX);
                if is_float(t) {
                    let rx = self.fsrc(*x, f, Xmm(0));
                    self.a.fstore(m, rx, t == IrTy::F64);
                } else if let Some(n) = self.imm32(*x, f, false) {
                    self.a.store_imm(m, n, size(t));
                } else {
                    let rx = self.src(*x, f, false, RAX);
                    self.a.store(m, rx, size(t));
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
            Op::Realloc(p, n) => {
                let (wp, wn) = (self.loc[p.0 as usize], self.loc[n.0 as usize]);
                self.moves(&[(Where::Reg(RDI), wp), (Where::Reg(RSI), wn)]);
                let re = self.rt().realloc;
                self.a.call(re);
                self.st(res.unwrap(), RAX);
            }
        }
        Ok(())
    }

    // ---- Memory --------------------------------------------------------------------------------------

    /// The memory operand for the address a value holds: a frame offset, `[base + index * scale + disp]`
    /// for a folded pointer, or the register the pointer is in. Registers the operand needs that
    /// the value does not already have are loaded into `s1` (base) and `s2` (index).
    fn mem_of(&mut self, f: &Func, p: Value, s1: Reg, s2: Reg) -> Mem {
        match self.loc[p.0 as usize] {
            Where::Frame(off) => mem(RBP, off),
            Where::Folded => {
                let Fold::Addr { base, index, disp } = self.fold[p.0 as usize] else {
                    unreachable!("a folded index is read through its address")
                };
                let (b, extra) = match base {
                    Base::Slot(i) => (RBP, self.slot_off[&i]),
                    Base::Value(v) => (self.src(v, f, false, s1), 0),
                };
                let index = index.map(|(iv, scale)| (self.src(iv, f, false, s2), scale));
                Mem {
                    base: b,
                    index,
                    disp: disp + extra,
                }
            }
            _ => {
                let rb = self.src(p, f, false, s1);
                mem(rb, 0)
            }
        }
    }

    /// An operand with an index becomes a plain register address in `scratch` (`lea`), so that the
    /// registers it was built from can be reused.
    fn settle(&mut self, m: Mem, scratch: Reg) -> Mem {
        if m.index.is_some() {
            self.a.lea(scratch, m);
            mem(scratch, 0)
        } else {
            m
        }
    }

    /// Copies `len` bytes (at most `SMALL_COPY`) between two memory operands without a loop.
    fn copy_bytes(&mut self, to: Mem, from: Mem, len: i32) {
        let at = |m: Mem, k: i32| Mem {
            disp: m.disp + k,
            ..m
        };
        let mut k = 0;
        for w in [8, 4, 2, 1] {
            while len - k >= w {
                self.a.load(RAX, at(from, k), w as u8);
                self.a.store(at(to, k), RAX, w as u8);
                k += w;
            }
        }
    }

    /// The fill loop number `k` as one string store: `rep stos` of the element size.
    fn fill(&mut self, f: &Func, k: usize) {
        let (base, value, count, width) = {
            let l = &self.fills[k];
            (l.base, l.value, l.count, l.width)
        };
        // `rdi` may hold a value; `rax`, `rcx` and `rdx` are scratch.
        self.a.push(RDI);
        self.ld(RAX, value);
        let m = self.mem_of(f, base, RCX, RDX);
        self.a.lea(RDI, m);
        self.a.mov_ri(RCX, count as i64);
        self.a.rep_stos(width);
        self.a.pop(RDI);
    }

    /// Fills `size` bytes of the frame with zeros.
    fn zero_frame(&mut self, off: i32, size: i32) {
        if size <= 128 {
            let mut k = 0;
            if size >= 16 {
                self.a.fxor(Xmm(0), Xmm(0));
                while size - k >= 16 {
                    self.a.movups_store(mem(RBP, off + k), Xmm(0));
                    k += 16;
                }
            }
            for w in [8, 4, 2, 1] {
                while size - k >= w {
                    self.a.store_imm(mem(RBP, off + k), 0, w as u8);
                    k += w;
                }
            }
        } else {
            // `rdi` may hold a value: the string instruction needs it.
            self.a.push(RDI);
            self.a.lea(RDI, mem(RBP, off));
            self.a.alu_rr(Alu::Xor, RAX, RAX, 4);
            self.a.mov_ri(RCX, size as i64);
            self.a.rep_stosb();
            self.a.pop(RDI);
        }
    }

    /// Does the code after the `Slot` instruction `id` (frame offset `off`, `size` bytes) write every
    /// byte of it, in its own block, before anything reads or passes on its address? Then the
    /// clearing can be left out.
    fn overwritten_first(&self, f: &Func, id: usize, off: i32, size: i32) -> bool {
        if size > 4096 {
            return false;
        }
        let (bi, k) = self.at;
        let b = &f.blocks[bi];
        let in_slot = |v: Value| -> Option<i32> {
            match self.loc[v.0 as usize] {
                Where::Frame(o) if (off..off + size).contains(&o) => Some(o - off),
                _ => None,
            }
        };
        // An address built on this slot that is not a plain frame offset.
        let touches = |v: Value| -> bool {
            in_slot(v).is_some()
                || matches!(
                    self.fold[v.0 as usize],
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
                        if mark(at, size_of_ty(f.ty(*x)), &mut written) {
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
                        match (in_slot(*d), self.loc[n.0 as usize]) {
                            (Some(at), Where::Imm(len)) => {
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
                Loc::Float(i) => copies.push((Where::Xmm(Xmm(*i as u8)), self.loc[arg.0 as usize])),
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
                self.stf(r, Xmm(0), f);
            } else {
                self.st(r, RAX);
            }
        }
    }

    // ---- Arithmetic ----------------------------------------------------------------------------------

    /// A memory operand holding the float: its frame slot, or the frame scratch (at `k`) after storing it there.
    fn fmem(&mut self, v: Value, f: &Func, k: i32) -> Mem {
        match self.loc[v.0 as usize] {
            Where::Mem(off) => mem(RBP, off),
            Where::Xmm(x) => {
                let m = mem(RBP, self.fscratch + k);
                self.a.fstore(m, x, f.ty(v) == IrTy::F64);
                m
            }
            w => panic!("float {} is read from {w:?}", v.0),
        }
    }

    fn bin(&mut self, f: &Func, op: BinOp, a: Value, b: Value, r: Value) -> Result<(), String> {
        let t = f.ty(a);
        if is_float(t) {
            let dbl = t == IrTy::F64;
            let sse = match op {
                BinOp::Add => Sse::Add,
                BinOp::Sub => Sse::Sub,
                BinOp::Mul => Sse::Mul,
                BinOp::Div => Sse::Div,
                BinOp::Rem => {
                    // x87 `fprem` is exactly C's `fmod`: the sign of the dividend, NaN for a zero divisor.
                    // It reads memory: operands in SSE registers wait in the frame scratch.
                    let (ma, mb) = (self.fmem(a, f, 0), self.fmem(b, f, 8));
                    self.a.x87_load(mb, dbl);
                    self.a.x87_load(ma, dbl);
                    let again = self.a.new_label();
                    self.a.bind(again);
                    self.a.x87_prem();
                    self.a.x87_status_ax();
                    self.a.shift_imm(Shift::Shr, RAX, 10, 4);
                    self.a.alu_ri(Alu::And, RAX, 1, 4);
                    self.a.jcc(Cc::Ne, again);
                    self.a.x87_drop_second();
                    match self.loc[r.0 as usize] {
                        Where::Mem(off) => self.a.x87_store_pop(mem(RBP, off), dbl),
                        Where::Xmm(x) => {
                            let m = mem(RBP, self.fscratch);
                            self.a.x87_store_pop(m, dbl);
                            self.a.fload(x, m, dbl);
                        }
                        w => unreachable!("a float result in {w:?}"),
                    }
                    return Ok(());
                }
                _ => return Err("not a float operation".to_string()),
            };
            // Two-address: built in the result's own register. When that is the register of `b`, a
            // commutative operation swaps the operands and the others are built in `xmm0`.
            let (mut a, mut b) = (a, b);
            let mut d = self.fdst(r);
            if a != b && self.loc[b.0 as usize] == Where::Xmm(d) {
                if matches!(op, BinOp::Add | BinOp::Mul) {
                    std::mem::swap(&mut a, &mut b);
                } else {
                    d = Xmm(0);
                }
            }
            self.ldf(d, a, f);
            let xb = self.fsrc(b, f, Xmm(1));
            self.a.farith(sse, d, xb, dbl);
            self.stf(r, d, f);
            return Ok(());
        }
        let w = bits(t);
        match op {
            BinOp::Mul
                if self.imm32(b, f, false).is_some() || self.imm32(a, f, false).is_some() =>
            {
                // `imul d, a, imm` (no register for the constant), or `lea` for 3, 5 and 9, which is
                // faster. The constant may be on either side.
                let (a, b) = if self.imm32(b, f, false).is_some() {
                    (a, b)
                } else {
                    (b, a)
                };
                let n = self.imm32(b, f, false).unwrap();
                let d = self.dst(r);
                let ra = self.src(a, f, false, RCX);
                if let (3 | 5 | 9, 64) = (n, w) {
                    self.a.lea(d, mem_idx(ra, ra, (n - 1) as u8, 0));
                } else {
                    self.a.imul_rri(d, ra, n, 8);
                    self.mask(d, w);
                }
                self.st(r, d);
            }
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
            BinOp::MulHi | BinOp::UMulHi => {
                // The product of the extended operands; the high half is `rdx` for 64 bits and a shift below.
                let signed = op == BinOp::MulHi;
                self.ld_ext(RAX, a, f, signed);
                self.ld_ext(RCX, b, f, signed);
                let out = if w == 64 {
                    if signed {
                        self.a.imul1(RCX, 8);
                    } else {
                        self.a.mul(RCX, 8);
                    }
                    RDX
                } else {
                    self.a.imul_rr(RAX, RCX, 8);
                    let sh = if signed { Shift::Sar } else { Shift::Shr };
                    self.a.shift_imm(sh, RAX, w as u8, 8);
                    self.mask(RAX, w);
                    RAX
                };
                self.st(r, out);
            }
            BinOp::Shl | BinOp::Shr | BinOp::UShr if matches!(self.loc[b.0 as usize], Where::Imm(n) if (0..w as i64).contains(&n)) =>
            {
                // A constant amount below the width: no check, an immediate shift.
                let Where::Imm(n) = self.loc[b.0 as usize] else {
                    unreachable!()
                };
                // Built in the result's own register (no round trip through `rax`).
                let d = self.dst(r);
                self.ld_ext(d, a, f, op == BinOp::Shr);
                let sh = match op {
                    BinOp::Shl => Shift::Shl,
                    BinOp::Shr => Shift::Sar,
                    _ => Shift::Shr,
                };
                self.a.shift_imm(sh, d, n as u8, 8);
                self.mask(d, w);
                self.st(r, d);
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
        let ov = self.stop_overflow;
        self.checked_to(f, op, signed, a, b, r, ov);
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
            let start = self.a.here();
            match self.imm32(b, f, signed) {
                Some(n) if op != BinOp::Mul => {
                    let alu = if op == BinOp::Add { Alu::Add } else { Alu::Sub };
                    self.a.alu_ri(alu, d, n, 8);
                }
                Some(n) => self.a.imul_rri(d, d, n, 8),
                _ => {
                    let rb = self.src(b, f, false, RCX);
                    match op {
                        BinOp::Add => self.a.alu_rr(Alu::Add, d, rb, 8),
                        BinOp::Sub => self.a.alu_rr(Alu::Sub, d, rb, 8),
                        _ => self.a.imul_rr(d, rb, 8),
                    }
                }
            }
            self.a.guard_jump(start, 6);
            self.ov_jump(if signed { Cc::O } else { Cc::B }, ov);
            self.st(r, d);
            return;
        }
        self.ld_ext(RAX, a, f, signed);
        self.ld_ext(RCX, b, f, signed);
        if w == 64 {
            match (op, signed) {
                (BinOp::Add, true) => {
                    self.a.alu_rr(Alu::Add, RAX, RCX, 8);
                    self.ov_jump(Cc::O, ov);
                }
                (BinOp::Sub, true) => {
                    self.a.alu_rr(Alu::Sub, RAX, RCX, 8);
                    self.ov_jump(Cc::O, ov);
                }
                (BinOp::Mul, true) => {
                    self.a.imul_rr(RAX, RCX, 8);
                    self.ov_jump(Cc::O, ov);
                }
                (BinOp::Add, false) => {
                    self.a.alu_rr(Alu::Add, RAX, RCX, 8);
                    self.ov_jump(Cc::B, ov);
                }
                (BinOp::Sub, false) => {
                    self.a.alu_rr(Alu::Sub, RAX, RCX, 8);
                    self.ov_jump(Cc::B, ov);
                }
                _ => {
                    self.a.mul(RCX, 8);
                    self.ov_jump(Cc::B, ov);
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
            self.ov_jump(Cc::Ne, ov);
            self.mask(RAX, w);
        }
        self.st(r, RAX);
    }

    /// `r = c ? a : b` with a conditional move: no branch to mispredict. Integers, booleans and addresses.
    fn ov_jump(&mut self, cc: Cc, ov: Label) {
        self.a.jcc(cc, ov);
        if let Some(l) = self.resume.take() {
            self.a.bind(l);
        }
    }

    fn select(&mut self, f: &Func, c: Value, a: Value, b: Value, r: Value) {
        let mut d = self.dst(r);
        // The condition: from the flags when they are the comparison's, else test the bool.
        let mut cc = match self.flags {
            Some((v, cc)) if v == c => cc,
            _ => {
                let rc = self.src(c, f, false, RDX);
                self.a.test_rr(rc, rc, 4);
                Cc::Ne
            }
        };
        if self.loc[c.0 as usize] == Where::Reg(d) {
            d = RAX;
        }
        // The result starts as the operand that already sits in its register, and the other one moves
        // in when the condition says so.
        let (keep, other) = if self.loc[a.0 as usize] == Where::Reg(d) {
            (a, b)
        } else {
            cc = cc.invert();
            (b, a)
        };
        self.ld(d, keep);
        let ro = self.src(other, f, false, RCX);
        self.a.cmov(cc.invert(), d, ro, 8);
        self.st(r, d);
    }

    fn cmp(&mut self, f: &Func, op: CmpOp, a: Value, b: Value, r: Value) {
        let t = f.ty(a);
        if is_float(t) {
            if matches!(op, CmpOp::Eq | CmpOp::Ne) {
                // Equal needs the zero flag set and the parity flag clear (unordered sets both).
                let (xa, xb) = (self.fsrc(a, f, Xmm(0)), self.fsrc(b, f, Xmm(1)));
                self.a.fcmp(xa, xb, t == IrTy::F64);
                if op == CmpOp::Eq {
                    self.a.setcc(Cc::E, RAX);
                    self.a.setcc(Cc::Np, RCX);
                    self.a.alu_rr(Alu::And, RAX, RCX, 1);
                } else {
                    self.a.setcc(Cc::Ne, RAX);
                    self.a.setcc(Cc::P, RCX);
                    self.a.alu_rr(Alu::Or, RAX, RCX, 1);
                }
            } else {
                let cc = self.float_cmp(f, op, a, b);
                self.a.setcc(cc, RAX);
            }
        } else {
            let cc = self.int_cmp(f, op, a, b);
            let d = self.dst(r);
            self.a.setcc(cc, d);
            self.flags = Some((r, cc));
            self.a.extend_rr(d, d, 1, false, false);
            self.st(r, d);
            return;
        }
        self.a.extend_rr(RAX, RAX, 1, false, false);
        self.st(r, RAX);
    }

    /// Compares two floats with an ordering (`<`, `<=`, `>`, `>=`); gives the condition that holds when
    /// `op` is true. `a < b` is `b > a`; unordered operands give "not above".
    fn float_cmp(&mut self, f: &Func, op: CmpOp, a: Value, b: Value) -> Cc {
        let d = f.ty(a) == IrTy::F64;
        let (xa, xb) = (self.fsrc(a, f, Xmm(0)), self.fsrc(b, f, Xmm(1)));
        match op {
            CmpOp::Lt | CmpOp::ULt => {
                self.a.fcmp(xb, xa, d);
                Cc::A
            }
            CmpOp::Le | CmpOp::ULe => {
                self.a.fcmp(xb, xa, d);
                Cc::Ae
            }
            CmpOp::Gt | CmpOp::UGt => {
                self.a.fcmp(xa, xb, d);
                Cc::A
            }
            _ => {
                self.a.fcmp(xa, xb, d);
                Cc::Ae
            }
        }
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
                let d = self.fdst(r);
                // The conversion keeps the rest of its destination: clear it to break the dependency.
                self.a.fxor(d, d);
                self.a.cvt_int_to_float(d, RAX, true, to == IrTy::F64);
                self.stf(r, d, f);
            }
            CastKind::FloatToInt => {
                let x = self.fsrc(a, f, Xmm(0));
                let double = from == IrTy::F64;
                self.a.cvt_float_to_int(RAX, x, true, double);
                // `cvttsd2si` answers `i64::MIN` for NaN and anything out of range. Saturate like
                // Rust's `as`: NaN gives 0, a positive overflow gives `i64::MAX`.
                let done = self.a.new_label();
                self.a.mov_ri(RCX, i64::MIN);
                self.a.alu_rr(Alu::Cmp, RAX, RCX, 8);
                self.a.jcc(Cc::Ne, done);
                self.a.fxor(Xmm(1), Xmm(1));
                self.a.fcmp(x, Xmm(1), double);
                let nan = self.a.new_label();
                let big = self.a.new_label();
                self.a.jcc(Cc::P, nan);
                self.a.jcc(Cc::A, big);
                self.a.jmp(done);
                self.a.bind(nan);
                self.a.alu_rr(Alu::Xor, RAX, RAX, 4);
                self.a.jmp(done);
                self.a.bind(big);
                self.a.mov_ri(RAX, i64::MAX);
                self.a.bind(done);
                self.mask(RAX, bits(to));
                self.st(r, RAX);
            }
            CastKind::FloatResize => {
                let d = self.fdst(r);
                if from != to {
                    let x = self.fsrc(a, f, Xmm(0));
                    self.a.cvt_float_resize(d, x, to == IrTy::F64);
                } else {
                    self.ldf(d, a, f);
                }
                self.stf(r, d, f);
            }
        }
    }
}

fn callee_ret(m: &Module, f: FuncId) -> IrTy {
    m.funcs[f.0 as usize].ret
}
