//! Register assignment for the native code generator (stages N5 and A-11 W3).
//!
//! Integer, boolean and pointer values may live in a register for their whole life; every other value
//! keeps its stack slot. The scheme is a linear-scan style allocator over live ranges with holes:
//!
//! * Liveness comes from the usual backward dataflow over the blocks (a block's parameters are
//!   defined at its start; the arguments of a jump are used by the jump). From it each value gets a
//!   **list of ranges** (half open, in the linear order of the function), not one interval, so two
//!   values whose lives interleave without overlapping can share a register.
//! * Positions are even for instructions and odd just after them: an instruction reads its operands
//!   at `p` and writes its result at `p + 1`. A result may therefore take the register of an operand
//!   that dies in that instruction (`gen.rs` builds results so this is safe).
//! * Values are taken heaviest first (uses weighted by loop depth) and given the cheapest register
//!   whose occupied ranges do not meet theirs. The cost model compares, for each value:
//!   memory, which costs one load or store per use or definition; a callee-saved register
//!   (`rbx r12-r15`), which costs one save and one restore per call of the function and nothing when
//!   another value already pays for that register; and a caller-saved register
//!   (`r10 r11 r8 r9 rsi rdi`), which is free when no call-like operation lies inside the range and
//!   otherwise **splits the value around each such operation**: it is stored to its home slot before
//!   the operation and reloaded after it (`Alloc::across` lists them for the generator).
//!
//!   A register is used when it is no dearer than memory. Ties go to the register most related values
//!   already has (the first operand of a two-address operation, the other end of a block-parameter
//!   copy), then to the argument register the value is passed in, which saves moves.
//! * Constants and the results of fused compare-and-branch pairs take no register (the generator
//!   knows them).

use crate::ir::{BinOp, CmpOp, Func, IrTy, Op, Term, Value};
use crate::native::x86::{Reg, Xmm};
use std::collections::HashMap;

/// A register a value can live in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    Gpr(Reg),
    Xmm(Xmm),
}

/// The result of the assignment for one function.
pub struct Alloc {
    /// For each value, the register it lives in, if any.
    pub reg: Vec<Option<Place>>,
    /// For each call-like instruction (by id): the values in caller-saved registers (integer or SSE)
    /// that are live across it. The generator stores them before the operation and loads them after.
    pub across: HashMap<usize, Vec<Value>>,
}

/// The most bytes a `memcopy` of a constant size copies with plain moves instead of `rep movsb`.
pub const SMALL_COPY: i64 = 64;

/// A `memcopy` of a known small size.
pub fn small_copy(len: Option<i64>) -> bool {
    matches!(len, Some(n) if (0..=SMALL_COPY).contains(&n))
}

/// The instructions that end in a call to a function or a runtime routine: every caller-saved
/// register may change. (`consts` are the constant values of the function, see `gen::const_values`.)
pub fn is_call_like(op: &Op, consts: &[Option<i64>]) -> bool {
    match op {
        Op::MemCopy(_, _, n) => !small_copy(consts[n.0 as usize]),
        _ => matches!(
            op,
            Op::Call(..)
                | Op::CallPtr(..)
                | Op::Alloc(_)
                | Op::Free(_)
                | Op::Realloc(..)
                | Op::Print(..)
        ),
    }
}

/// What an address is made of when no register holds it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Base {
    Value(Value),
    /// The frame object made by the `Slot` instruction with this number.
    Slot(usize),
}

/// A pointer value the generator never computes into a register: the instructions that read or write
/// through it name the address in their memory operand (`[base + index * scale + disp]`), and
/// anything else that needs the pointer itself gets it with one `lea`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fold {
    No,
    /// The result of a `Slot`, of a `PtrAdd` of a constant or an index to such an address.
    Addr {
        base: Base,
        index: Option<(Value, u8)>,
        disp: i32,
    },
    /// `idx * scale` (a `mul` or `shl` by a constant), only as the index of an `Addr`.
    Scaled {
        idx: Value,
        scale: u8,
    },
}

impl Fold {
    /// A frame object plus a constant: an address that needs no register at all, wherever it is used.
    pub fn is_frame(&self) -> bool {
        matches!(
            self,
            Fold::Addr {
                base: Base::Slot(_),
                index: None,
                ..
            }
        )
    }
}

/// Finds the pointers that can stay unnamed. A `Slot` always can. A `PtrAdd` (and a `mul` or `shl`
/// that scales its index) can when every use of its result is the address of a load or store, the
/// address of a small `memcopy`, or the base of another such `PtrAdd`; the registers it would have
/// read are then read where those uses are, so they must be live there (`assign` does that).
pub fn folds(f: &Func, uses: &[u32], consts: &[Option<i64>]) -> Vec<Fold> {
    let nv = f.values.len();
    let is64 = |v: Value| f.ty(v) == IrTy::Int(64);
    // Which values are candidates, until a use rules them out.
    let mut on = vec![false; nv];
    let mut scaled = vec![None; nv];
    for b in &f.blocks {
        for &i in &b.insts {
            let Some(r) = f.insts[i].result else { continue };
            match &f.insts[i].op {
                Op::PtrAdd(..) => on[r.0 as usize] = true,
                Op::Bin(op @ (BinOp::Mul | BinOp::Shl), x, y)
                    if is64(r) && uses[r.0 as usize] == 1 =>
                {
                    let k = |c: Value| consts[c.0 as usize];
                    let (idx, scale) = match (op, k(*x), k(*y)) {
                        (BinOp::Mul, None, Some(c)) => (*x, c),
                        (BinOp::Mul, Some(c), None) => (*y, c),
                        (BinOp::Shl, None, Some(c)) if (0..=3).contains(&c) => (*x, 1 << c),
                        _ => continue,
                    };
                    if matches!(scale, 1 | 2 | 4 | 8) {
                        scaled[r.0 as usize] = Some((idx, scale as u8));
                        on[r.0 as usize] = true;
                    }
                }
                _ => {}
            }
        }
    }
    loop {
        let mut fold = vec![Fold::No; nv];
        let mut changed = false;
        for b in &f.blocks {
            for &i in &b.insts {
                let Some(r) = f.insts[i].result else { continue };
                let ri = r.0 as usize;
                match &f.insts[i].op {
                    Op::Slot(..) => {
                        fold[ri] = Fold::Addr {
                            base: Base::Slot(i),
                            index: None,
                            disp: 0,
                        }
                    }
                    Op::PtrAdd(a, c) if on[ri] => {
                        let (base, index, disp) = match fold[a.0 as usize] {
                            Fold::Addr { base, index, disp } => (base, index, disp),
                            _ => (Base::Value(*a), None, 0),
                        };
                        let made = match (consts[c.0 as usize], fold[c.0 as usize]) {
                            (Some(k), _) => i32::try_from(disp as i64 + k)
                                .ok()
                                .map(|disp| Fold::Addr { base, index, disp }),
                            (None, Fold::Scaled { idx, scale }) if index.is_none() => {
                                Some(Fold::Addr {
                                    base,
                                    index: Some((idx, scale)),
                                    disp,
                                })
                            }
                            (None, Fold::No) if index.is_none() && is64(*c) => Some(Fold::Addr {
                                base,
                                index: Some((*c, 1)),
                                disp,
                            }),
                            _ => None,
                        };
                        match made {
                            Some(m) => fold[ri] = m,
                            None => {
                                on[ri] = false;
                                changed = true;
                            }
                        }
                    }
                    Op::Bin(..) if on[ri] => {
                        if let Some((idx, scale)) = scaled[ri] {
                            fold[ri] = Fold::Scaled { idx, scale };
                        }
                    }
                    _ => {}
                }
            }
        }
        // A folded value with a use that needs a register cannot stay folded.
        let mut bad = vec![false; nv];
        let mut needs = |v: Value| bad[v.0 as usize] = true;
        for b in &f.blocks {
            for &i in &b.insts {
                let inst = &f.insts[i];
                let r_folded = inst.result.is_some_and(|r| fold[r.0 as usize] != Fold::No);
                match &inst.op {
                    Op::Load(..) => {}
                    Op::Store(_, x) => needs(*x),
                    Op::MemCopy(d, s, n) => {
                        needs(*n);
                        if !small_copy(consts[n.0 as usize]) {
                            needs(*d);
                            needs(*s);
                        }
                    }
                    Op::PtrAdd(a, c) if !r_folded => {
                        needs(*a);
                        needs(*c);
                    }
                    // Read where the folded result is used.
                    Op::PtrAdd(..) => {}
                    Op::Bin(..) if r_folded => {}
                    op => {
                        let mut v = Vec::new();
                        inst_uses(op, &mut v);
                        v.into_iter().for_each(&mut needs);
                    }
                }
            }
            if let Some(t) = &b.term {
                let mut v = Vec::new();
                term_uses(t, &mut v);
                v.into_iter().for_each(&mut needs);
            }
        }
        for v in 0..nv {
            if bad[v] && on[v] && fold[v] != Fold::No && !fold[v].is_frame() {
                on[v] = false;
                changed = true;
            }
        }
        if !changed {
            return fold;
        }
    }
}

/// The values an instruction really reads at its position: a folded pointer is read through the
/// registers its address is made of.
fn resolved(v: Value, folds: &[Fold], out: &mut Vec<Value>) {
    match folds[v.0 as usize] {
        Fold::No => out.push(v),
        Fold::Addr { base, index, .. } => {
            if let Base::Value(b) = base {
                out.push(b);
            }
            if let Some((i, _)) = index {
                out.push(i);
            }
        }
        // Read as part of the `Addr` that uses it.
        Fold::Scaled { .. } => {}
    }
}

/// The compare that ends a block and only feeds the block's branch: the branch tests the flags
/// directly and the comparison is never stored. Gives the instruction.
pub fn fused_cmp(f: &Func, bi: usize, uses: &[u32]) -> Option<usize> {
    let b = &f.blocks[bi];
    let (&last, Some(Term::Branch(c, ..))) = (b.insts.last()?, &b.term) else {
        return None;
    };
    let Op::Cmp(_, x, _) = &f.insts[last].op else {
        return None;
    };
    // A float `==` or `!=` needs the parity flag as well as the zero flag: it is not fused.
    let flags_only = !matches!(f.ty(*x), IrTy::F32 | IrTy::F64)
        || matches!(
            &f.insts[last].op,
            Op::Cmp(
                CmpOp::Lt
                    | CmpOp::ULt
                    | CmpOp::Le
                    | CmpOp::ULe
                    | CmpOp::Gt
                    | CmpOp::UGt
                    | CmpOp::Ge
                    | CmpOp::UGe,
                ..
            )
        );
    (f.insts[last].result == Some(*c) && uses[c.0 as usize] == 1 && flags_only).then_some(last)
}

pub(super) fn inst_uses(op: &Op, out: &mut Vec<Value>) {
    match op {
        Op::Const(..) | Op::Slot(..) | Op::DataAddr(_) | Op::FuncAddr(_) => {}
        Op::Neg(a)
        | Op::Not(a)
        | Op::Cast(_, a, _)
        | Op::Load(_, a)
        | Op::Alloc(a)
        | Op::Free(a) => out.push(*a),
        Op::Bin(_, a, b)
        | Op::Checked(_, _, a, b)
        | Op::Cmp(_, a, b)
        | Op::PtrAdd(a, b)
        | Op::Store(a, b)
        | Op::Realloc(a, b)
        | Op::Print(a, b) => out.extend([*a, *b]),
        Op::MemCopy(a, b, c) => out.extend([*a, *b, *c]),
        Op::Call(_, args) => out.extend(args.iter().copied()),
        Op::CallPtr(_, _, p, args) => {
            out.push(*p);
            out.extend(args.iter().copied());
        }
    }
}

fn term_uses(t: &Term, out: &mut Vec<Value>) {
    match t {
        Term::Jump(tg) => out.extend(tg.args.iter().copied()),
        Term::Branch(c, a, b) => {
            out.push(*c);
            out.extend(a.args.iter().copied());
            out.extend(b.args.iter().copied());
        }
        Term::Return(Some(v)) => out.push(*v),
        _ => {}
    }
}

pub(super) fn successors(t: &Term) -> Vec<usize> {
    match t {
        Term::Jump(tg) => vec![tg.block.0 as usize],
        Term::Branch(_, a, b) => vec![a.block.0 as usize, b.block.0 as usize],
        _ => Vec::new(),
    }
}

struct Bits(Vec<u64>);

impl Bits {
    fn new(n: usize) -> Bits {
        Bits(vec![0; n.div_ceil(64)])
    }
    fn set(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
    fn has(&self, i: usize) -> bool {
        self.0[i / 64] >> (i % 64) & 1 == 1
    }
}

/// Positions in the linear order of the function: even numbers for the block start, the
/// instructions and the terminator, in layout order. A block's parameters are defined at
/// `block_start + 1`.
struct Order {
    block_start: Vec<u32>,
    block_end: Vec<u32>,
    inst_pos: Vec<u32>,
    term_pos: Vec<u32>,
}

fn order(f: &Func) -> Order {
    let mut o = Order {
        block_start: vec![0; f.blocks.len()],
        block_end: vec![0; f.blocks.len()],
        inst_pos: vec![0; f.insts.len()],
        term_pos: vec![0; f.blocks.len()],
    };
    let mut p = 0u32;
    for (bi, b) in f.blocks.iter().enumerate() {
        o.block_start[bi] = p;
        p += 2;
        for &i in &b.insts {
            o.inst_pos[i] = p;
            p += 2;
        }
        o.term_pos[bi] = p;
        p += 2;
        o.block_end[bi] = p;
    }
    o
}

/// The ranges of one value, half open and sorted from the end of the function to its start while
/// they are built (new ranges always begin at or before every existing one).
fn add_range(r: &mut Vec<(u32, u32)>, from: u32, to: u32) {
    match r.last_mut() {
        Some(l) if to >= l.0 => {
            l.0 = l.0.min(from);
            l.1 = l.1.max(to);
        }
        _ => r.push((from, to)),
    }
}

/// The value is made at `pos`: its first range begins there.
fn set_from(r: &mut Vec<(u32, u32)>, pos: u32) {
    match r.last_mut() {
        Some(l) if l.0 <= pos && pos < l.1 => l.0 = pos,
        // Never read: a one-position range, so nothing else is overwritten by the write.
        _ => r.push((pos, pos + 1)),
    }
}

/// Is any range of `a` over one of `taken`? `taken` is sorted and its ranges are disjoint.
fn overlaps(a: &[(u32, u32)], taken: &[(u32, u32)]) -> bool {
    a.iter().any(|&(from, to)| {
        let i = taken.partition_point(|x| x.1 <= from);
        i < taken.len() && taken[i].0 < to
    })
}

/// The values instruction `i` reads, folded pointers resolved; nothing for an instruction whose result is folded.
fn inst_reads(f: &Func, i: usize, folds: &[Fold], out: &mut Vec<Value>) {
    if f.insts[i]
        .result
        .is_some_and(|r| folds[r.0 as usize] != Fold::No)
    {
        return;
    }
    let mut raw = Vec::new();
    inst_uses(&f.insts[i].op, &mut raw);
    for v in raw {
        resolved(v, folds, out);
    }
}

fn term_reads(t: &Term, folds: &[Fold], out: &mut Vec<Value>) {
    let mut raw = Vec::new();
    term_uses(t, &mut raw);
    for v in raw {
        resolved(v, folds, out);
    }
}

/// Assigns registers. `saved` are callee-saved registers, `scratch` caller-saved ones in the order
/// to try them, `xmm` the SSE registers for floats (all caller-saved), and `args` the registers that
/// carry the integer parameters and arguments (in order, used only as a preference).
pub fn assign(
    f: &Func,
    saved: &[Reg],
    scratch: &[Reg],
    xmm: &[Xmm],
    args: &[Reg],
    folds: &[Fold],
    consts: &[Option<i64>],
) -> Alloc {
    let nv = f.values.len();
    let nb = f.blocks.len();
    let o = order(f);
    let uses = f.use_counts();

    // Uses and definitions per block.
    let mut use_b: Vec<Bits> = (0..nb).map(|_| Bits::new(nv)).collect();
    let mut def_b: Vec<Bits> = (0..nb).map(|_| Bits::new(nv)).collect();
    let mut tmp = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for p in &b.params {
            def_b[bi].set(p.0 as usize);
        }
        for &i in &b.insts {
            tmp.clear();
            inst_reads(f, i, folds, &mut tmp);
            for v in &tmp {
                if !def_b[bi].has(v.0 as usize) {
                    use_b[bi].set(v.0 as usize);
                }
            }
            if let Some(r) = f.insts[i].result {
                def_b[bi].set(r.0 as usize);
            }
        }
        tmp.clear();
        if let Some(t) = &b.term {
            term_reads(t, folds, &mut tmp);
        }
        for v in &tmp {
            if !def_b[bi].has(v.0 as usize) {
                use_b[bi].set(v.0 as usize);
            }
        }
    }
    // Backward dataflow until nothing changes.
    let succ: Vec<Vec<usize>> = f
        .blocks
        .iter()
        .map(|b| b.term.as_ref().map(successors).unwrap_or_default())
        .collect();
    let words = nv.div_ceil(64);
    let mut live_in: Vec<Bits> = (0..nb).map(|_| Bits::new(nv)).collect();
    let mut live_out: Vec<Bits> = (0..nb).map(|_| Bits::new(nv)).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for bi in (0..nb).rev() {
            for w in 0..words {
                let mut out = 0u64;
                for &s in &succ[bi] {
                    out |= live_in[s].0[w];
                }
                let inn = use_b[bi].0[w] | (out & !def_b[bi].0[w]);
                if out != live_out[bi].0[w] || inn != live_in[bi].0[w] {
                    live_out[bi].0[w] = out;
                    live_in[bi].0[w] = inn;
                    changed = true;
                }
            }
        }
    }

    // Loop depth: a jump back to an earlier (or the same) block puts every block in between in a loop.
    let mut depth = vec![0u32; nb];
    for (bi, ss) in succ.iter().enumerate() {
        for &t in ss {
            if t <= bi {
                for d in depth.iter_mut().take(bi + 1).skip(t) {
                    *d += 1;
                }
            }
        }
    }
    let block_weight = |bi: usize| 10u64.pow(depth[bi].min(6));

    // Fused compares: their result is never stored.
    let mut fused_res = vec![false; nv];
    for bi in 0..nb {
        if let Some(i) = fused_cmp(f, bi, &uses) {
            fused_res[f.insts[i].result.unwrap().0 as usize] = true;
        }
    }

    // Live ranges (built from the last block to the first) and weights.
    let mut ranges: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nv];
    let mut weight = vec![0u64; nv];
    for bi in (0..nb).rev() {
        let b = &f.blocks[bi];
        let w = block_weight(bi);
        let (from, end) = (o.block_start[bi], o.block_end[bi]);
        for (word, bits) in live_out[bi].0.iter().enumerate() {
            let mut x = *bits;
            while x != 0 {
                let v = word * 64 + x.trailing_zeros() as usize;
                x &= x - 1;
                add_range(&mut ranges[v], from, end);
            }
        }
        tmp.clear();
        if let Some(t) = &b.term {
            term_reads(t, folds, &mut tmp);
        }
        for v in &tmp {
            add_range(&mut ranges[v.0 as usize], from, o.term_pos[bi] + 1);
            weight[v.0 as usize] += w;
        }
        for &i in b.insts.iter().rev() {
            let pos = o.inst_pos[i];
            if let Some(r) = f.insts[i].result {
                if folds[r.0 as usize] == Fold::No {
                    set_from(&mut ranges[r.0 as usize], pos + 1);
                    weight[r.0 as usize] += w;
                }
            }
            tmp.clear();
            inst_reads(f, i, folds, &mut tmp);
            for v in &tmp {
                add_range(&mut ranges[v.0 as usize], from, pos + 1);
                weight[v.0 as usize] += w;
            }
        }
        for p in &b.params {
            set_from(&mut ranges[p.0 as usize], from + 1);
            weight[p.0 as usize] += w;
        }
    }
    for r in &mut ranges {
        r.reverse();
    }

    // The call-like operations: where they are, how often they run.
    let mut calls: Vec<(u32, usize, u64, bool)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for &i in &b.insts {
            if is_call_like(&f.insts[i].op, consts) {
                calls.push((
                    o.inst_pos[i],
                    i,
                    block_weight(bi),
                    matches!(f.insts[i].op, Op::MemCopy(..)),
                ));
            }
        }
    }

    // Related values (two-address operations, copies, block-parameter copies) and the argument
    // register a value travels in: only preferences.
    let mut partners: Vec<Vec<Value>> = vec![Vec::new(); nv];
    fn link(a: Value, b: Value, partners: &mut [Vec<Value>]) {
        partners[a.0 as usize].push(b);
        partners[b.0 as usize].push(a);
    }
    let mut want_arg: Vec<Option<Place>> = vec![None; nv];
    if let Some(b0) = f.blocks.first() {
        let mut k = 0;
        for p in &b0.params {
            if !matches!(f.ty(*p), IrTy::F32 | IrTy::F64) {
                want_arg[p.0 as usize] = args.get(k).map(|r| Place::Gpr(*r));
                k += 1;
            }
        }
    }
    for b in &f.blocks {
        for &i in &b.insts {
            let inst = &f.insts[i];
            if inst.result.is_some_and(|r| folds[r.0 as usize] != Fold::No) {
                continue;
            }
            match (&inst.op, inst.result) {
                (Op::Bin(_, a, c) | Op::Checked(_, _, a, c), Some(r)) => {
                    link(r, *a, &mut partners);
                    link(r, *c, &mut partners);
                }
                (Op::Cast(_, a, _), Some(r)) => link(r, *a, &mut partners),
                (Op::PtrAdd(a, _), Some(r)) => link(r, *a, &mut partners),
                (Op::Call(_, vs), _) | (Op::CallPtr(_, _, _, vs), _) => {
                    let mut k = 0;
                    for v in vs {
                        if !matches!(f.ty(*v), IrTy::F32 | IrTy::F64) {
                            if want_arg[v.0 as usize].is_none() {
                                want_arg[v.0 as usize] = args.get(k).map(|r| Place::Gpr(*r));
                            }
                            k += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        let edge = |tg: &crate::ir::Target, partners: &mut [Vec<Value>]| {
            for (p, a) in f.block_params(tg.block).iter().zip(&tg.args) {
                link(*p, *a, partners);
            }
        };
        match &b.term {
            Some(Term::Jump(t)) => edge(t, &mut partners),
            Some(Term::Branch(_, t, e)) => {
                edge(t, &mut partners);
                edge(e, &mut partners);
            }
            _ => {}
        }
    }

    // Heaviest first. Integers, booleans and pointers share the general registers, floats take the SSE
    // registers; both banks go through the same loop.
    let eligible = |v: usize| {
        matches!(
            f.values[v],
            IrTy::Int(_) | IrTy::Bool | IrTy::Ptr | IrTy::F32 | IrTy::F64
        ) && !ranges[v].is_empty()
            && consts[v].is_none()
            && folds[v] == Fold::No
            && !fused_res[v]
            && uses[v] > 0
    };
    let mut cands: Vec<usize> = (0..nv).filter(|v| eligible(*v)).collect();
    cands.sort_by_key(|v| (std::cmp::Reverse(weight[*v]), *v));

    // Every register with whether it is callee-saved. The SSE registers all belong to the caller.
    let all: Vec<(Place, bool)> = scratch
        .iter()
        .map(|r| (Place::Gpr(*r), false))
        .chain(xmm.iter().map(|x| (Place::Xmm(*x), false)))
        .chain(saved.iter().map(|r| (Place::Gpr(*r), true)))
        .collect();
    let mut taken: Vec<Vec<(u32, u32)>> = vec![Vec::new(); all.len()];
    let mut saved_used = vec![false; all.len()];
    let mut out: Vec<Option<Place>> = vec![None; nv];
    let mut across: HashMap<usize, Vec<Value>> = HashMap::new();
    for v in cands {
        let float = matches!(f.values[v], IrTy::F32 | IrTy::F64);
        // The call-like operations inside the lifetime, and what a split around each would cost.
        // (A block copy with `rep movsb` leaves the SSE registers alone.)
        let mut crossed: Vec<usize> = Vec::new();
        let mut cross_cost = 0u64;
        for &(from, to) in &ranges[v] {
            let mut k = calls.partition_point(|c| c.0 < from);
            while k < calls.len() && calls[k].0 + 2 <= to {
                if !float || !calls[k].3 {
                    crossed.push(k);
                    cross_cost += 2 * calls[k].2;
                }
                k += 1;
            }
        }
        let mem_cost = weight[v];
        let partner_regs: Vec<Place> = partners[v]
            .iter()
            .filter_map(|p| out[p.0 as usize])
            .collect();
        let mut best: Option<(u64, i64, usize)> = None;
        for (ri, (reg, is_saved)) in all.iter().enumerate() {
            if matches!(reg, Place::Xmm(_)) != float || overlaps(&ranges[v], &taken[ri]) {
                continue;
            }
            let cost = if *is_saved {
                // Saved at entry and restored at exit, once for all the values that use the register.
                if saved_used[ri] {
                    0
                } else {
                    2
                }
            } else {
                cross_cost
            };
            // The register most related values already have wins: a sum that feeds a loop parameter and
            // reads the same parameter then needs no copy at all.
            let shared = partner_regs.iter().filter(|p| *p == reg).count() as i64;
            let rank = if shared > 0 {
                -shared
            } else if want_arg[v] == Some(*reg) {
                1
            } else {
                2 + ri as i64
            };
            if best.is_none_or(|(c, r, _)| (cost, rank) < (c, r)) {
                best = Some((cost, rank, ri));
            }
        }
        let Some((cost, _, ri)) = best else { continue };
        // Equal cost still favours the register: it has no latency, and the copies cost nothing here.
        if cost > mem_cost {
            continue;
        }
        for &(from, to) in &ranges[v] {
            let at = taken[ri].partition_point(|x| x.0 < from);
            taken[ri].insert(at, (from, to));
        }
        out[v] = Some(all[ri].0);
        if all[ri].1 {
            saved_used[ri] = true;
        } else {
            for k in crossed {
                across.entry(calls[k].1).or_default().push(Value(v as u32));
            }
        }
    }
    Alloc { reg: out, across }
}
