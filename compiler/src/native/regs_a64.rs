//! Register assignment for the AArch64 generator (card A-10, stage R3).
//!
//! The heaviest integer, boolean and pointer values live in `x19` to `x28`, the heaviest floats in
//! `d8` to `d15`, for their whole life; every other value keeps its frame slot. These registers are
//! callee-saved in our convention (the runtime keeps them too), so calls leave them alone and no value
//! is ever split: the allocator only has to decide who gets a register and who shares one.
//!
//! A value whose live range touches no call (a function call, an `alloc`, a `memcpy`, a float remainder:
//! see `calls` below) may instead live in a caller-saved register, which costs no save and restore:
//! `x0` to `x8` (the templates only use `x9` and up outside calls) and `d2` to `d7`. A parameter of the
//! function may stay in the register it arrives in but takes no other caller-saved one.
//!
//! The two-lane vectors have homes in `v16` to `v31`, caller-saved, for values that touch no call.
//!
//! * Liveness is the usual backward dataflow over the blocks (a block's parameters are defined at its
//!   start, the arguments of a jump are read by the jump).
//! * Each value gets its **live range**: the runs of positions (in the linear order of the function)
//!   from a definition to the last read on each path, with holes where it holds nothing. A block
//!   parameter is a run of one position at each jump that writes it. Two values may share a register
//!   only when their runs are apart, with two exceptions that meet at one position: the argument of a
//!   jump and the parameter it is written to (the copy of the edge disappears), and an operand that dies
//!   in an instruction and its result, for the templates that read everything before they write (a
//!   product and the out-of-line tests of `CheckedIf` are not among them).
//! * Values are taken heaviest first (reads and writes weighted by `10^loop depth`) and get the first
//!   register whose assigned values do not meet its ranges. A value must weigh at least [`MIN_WEIGHT`]
//!   to be worth the save and restore of its register.

use crate::ir::{BinOp, Func, IrTy, Op, Term, Value};
use crate::native::a64::{Reg, V};

/// The least weight that pays for a callee-saved register (one save and one restore per call).
const MIN_WEIGHT: u64 = 3;

/// The least weight for a value to get any register: a caller-saved one costs nothing to save.
const MIN_FREE_WEIGHT: u64 = 2;

/// The result of the assignment for one function.
pub struct Alloc {
    pub reg: Vec<Option<Reg>>,
    pub freg: Vec<Option<V>>,
    /// Homes of the two-lane vectors: `v16` to `v31`, caller-saved, never across a call.
    pub vreg: Vec<Option<V>>,
}

#[cfg(test)]
thread_local! {
    /// Tests shrink the register set to push the spill paths through ordinary programs.
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

#[derive(Clone)]
struct Bits(Vec<u64>);

impl Bits {
    fn new(n: usize) -> Bits {
        Bits(vec![0; n.div_ceil(64)])
    }
    fn set(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
    fn clear(&mut self, i: usize) {
        self.0[i / 64] &= !(1 << (i % 64));
    }
    fn get(&self, i: usize) -> bool {
        self.0[i / 64] >> (i % 64) & 1 == 1
    }
    /// Adds the other set; true when something was new.
    fn union(&mut self, o: &Bits) -> bool {
        let mut changed = false;
        for (a, b) in self.0.iter_mut().zip(&o.0) {
            let n = *a | *b;
            changed |= n != *a;
            *a = n;
        }
        changed
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Gpr,
    Fpr,
    Vec,
}

fn class(t: IrTy) -> Option<Class> {
    match t {
        IrTy::Int(_) | IrTy::Bool | IrTy::Ptr => Some(Class::Gpr),
        IrTy::F32 | IrTy::F64 => Some(Class::Fpr),
        IrTy::V2I64 => Some(Class::Vec),
        IrTy::Void => None,
    }
}

/// The instructions `mov_ri` needs for a constant.
fn mov_len(n: i64) -> u32 {
    let pieces = |x: u64| (0..4).filter(|i| (x >> (16 * i)) & 0xFFFF != 0).count() as u32;
    pieces(n as u64).min(pieces(!(n as u64))).max(1)
}

/// Assigns registers. `imms` are the constants (no home at all), `uses` the use counts and `fused` the
/// compares that only feed their branch (never stored).
pub fn assign(f: &Func, imms: &[Option<i64>], uses: &[u32], fused: &[bool]) -> Alloc {
    let n = f.values.len();
    let mut out = Alloc {
        reg: vec![None; n],
        freg: vec![None; n],
        vreg: vec![None; n],
    };
    let limit = reg_limit();
    if limit == 0 {
        return out;
    }
    // The candidates, numbered densely so the bit sets stay small.
    let mut cand_of = vec![usize::MAX; n];
    let mut cands: Vec<Value> = Vec::new();
    for v in 0..n {
        // A constant that takes several instructions to build is worth a register when it is read in a loop.
        let wide =
            imms[v].is_some_and(|n| mov_len(n) >= 2) && class(f.values[v]) == Some(Class::Gpr);
        if (imms[v].is_none() || wide) && uses[v] > 0 && !fused[v] && class(f.values[v]).is_some() {
            cand_of[v] = cands.len();
            cands.push(Value(v as u32));
        }
    }
    if cands.is_empty() {
        return out;
    }
    let nb = f.blocks.len();
    let nc = cands.len();
    let is_imm: Vec<bool> = cands.iter().map(|v| imms[v.0 as usize].is_some()).collect();

    // Reads and writes of each block, for the dataflow.
    let mut gen_ = vec![Bits::new(nc); nb];
    let mut kill = vec![Bits::new(nc); nb];
    let mut reads: Vec<Value> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for p in &b.params {
            if cand_of[p.0 as usize] != usize::MAX {
                kill[bi].set(cand_of[p.0 as usize]);
            }
        }
        for &i in &b.insts {
            reads.clear();
            f.insts[i].op.for_each_value(|v| reads.push(v));
            for v in &reads {
                let c = cand_of[v.0 as usize];
                if c != usize::MAX && !kill[bi].get(c) {
                    gen_[bi].set(c);
                }
            }
            if let Some(r) = f.insts[i].result {
                let c = cand_of[r.0 as usize];
                if c != usize::MAX {
                    kill[bi].set(c);
                }
            }
        }
        if let Some(t) = &b.term {
            reads.clear();
            t.for_each_value(|v| reads.push(v));
            for v in &reads {
                let c = cand_of[v.0 as usize];
                if c != usize::MAX && !kill[bi].get(c) {
                    gen_[bi].set(c);
                }
            }
        }
    }
    let succs: Vec<Vec<usize>> = f
        .blocks
        .iter()
        .map(|b| {
            b.term
                .as_ref()
                .map(super::regs::successors)
                .unwrap_or_default()
        })
        .collect();
    let mut live_in = gen_.iter().map(|g| Bits(g.0.clone())).collect::<Vec<_>>();
    let mut live_out = vec![Bits::new(nc); nb];
    loop {
        let mut changed = false;
        for bi in (0..nb).rev() {
            let mut out_set = Bits::new(nc);
            for &s in &succs[bi] {
                out_set.union(&live_in[s]);
            }
            let mut in_set = Bits(out_set.0.clone());
            for c in 0..nc {
                if kill[bi].get(c) {
                    in_set.clear(c);
                }
            }
            in_set.union(&gen_[bi]);
            changed |= live_out[bi].union(&out_set);
            changed |= live_in[bi].union(&in_set);
        }
        if !changed {
            break;
        }
    }

    // Positions in the linear order: a block start, each instruction, the terminator.
    let mut start = vec![0u32; nb];
    let mut end = vec![0u32; nb];
    let mut inst_pos = vec![0u32; f.insts.len()];
    let mut at = 0u32;
    for (bi, b) in f.blocks.iter().enumerate() {
        start[bi] = at;
        at += 1;
        for &i in &b.insts {
            inst_pos[i] = at;
            at += 1;
        }
        end[bi] = at;
        at += 1;
    }

    // Loop depth: a jump back to an earlier block (or the same one) closes a loop.
    let mut depth = vec![0u32; nb];
    for (j, ss) in succs.iter().enumerate() {
        for &h in ss {
            if h <= j {
                for d in &mut depth[h..=j] {
                    *d += 1;
                }
            }
        }
    }
    let weight_of = |bi: usize| 10u64.pow(depth[bi].min(4));

    let mut weight = vec![0u64; nc];
    // (argument, parameter) of a jump, with the positions of the jumps.
    let mut pairs: std::collections::HashMap<(usize, usize), Vec<u32>> =
        std::collections::HashMap::new();
    // The live ranges: per value, the runs of positions (inclusive) where it holds a value somebody may
    // still read. Built block by block from the liveness sets: a run opens at the block start (live in),
    // at a definition or at a jump that writes a parameter, and closes at its last use, or at the end of
    // the block when the value lives on.
    let mut segs: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nc];
    let mut open = vec![u32::MAX; nc];
    let mut last = vec![0u32; nc];
    let mut touched: Vec<usize> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let w = weight_of(bi);
        touched.clear();
        let mut touch = |c: usize, pos: u32, touched: &mut Vec<usize>| {
            if open[c] == u32::MAX {
                open[c] = pos;
                touched.push(c);
            }
            last[c] = pos;
        };
        for c in 0..nc {
            if live_in[bi].get(c) {
                touch(c, start[bi], &mut touched);
            }
        }
        for p in &b.params {
            let c = cand_of[p.0 as usize];
            if c != usize::MAX {
                touch(c, start[bi], &mut touched);
                weight[c] += w;
            }
        }
        let mut prev_check = false;
        for &i in &b.insts {
            reads.clear();
            f.insts[i].op.for_each_value(|v| reads.push(v));
            for v in reads.iter().chain(f.insts[i].result.iter()) {
                let c = cand_of[v.0 as usize];
                if c != usize::MAX {
                    touch(c, inst_pos[i], &mut touched);
                    weight[c] += w;
                }
            }
            // These templates read every operand before they write the result, so a result may take the
            // register of an operand that dies there. (Not a product: the checked one needs both operands
            // after the low half is written.)
            // A plain operation right after a `CheckedIf` may be fused with it, and the out-of-line test
            // of the check still reads its own operands then, so that one keeps them.
            let after_check = prev_check;
            prev_check = matches!(f.insts[i].op, Op::CheckedIf(..));
            let alias = match &f.insts[i].op {
                Op::Select(..) | Op::Cmp(..) => true,
                Op::Bin(op, ..) => *op != BinOp::Mul && !after_check,
                Op::Checked(op, ..) => matches!(op, BinOp::Add | BinOp::Sub),
                _ => false,
            };
            if let (true, Some(r)) = (alias, f.insts[i].result) {
                let cr = cand_of[r.0 as usize];
                for v in &reads {
                    let co = cand_of[v.0 as usize];
                    if cr != usize::MAX
                        && co != usize::MAX
                        && co != cr
                        && !is_imm[co]
                        && !is_imm[cr]
                    {
                        pairs.entry((co, cr)).or_default().push(inst_pos[i]);
                    }
                }
            }
        }
        if let Some(t) = &b.term {
            reads.clear();
            t.for_each_value(|v| reads.push(v));
            for v in &reads {
                let c = cand_of[v.0 as usize];
                if c != usize::MAX {
                    touch(c, end[bi], &mut touched);
                    weight[c] += w;
                }
            }
            // A jump writes the parameters of the block it leads to.
            let targets: Vec<&crate::ir::Target> = match t {
                Term::Jump(tg) => vec![tg],
                Term::Branch(_, a, b) => vec![a, b],
                _ => Vec::new(),
            };
            for tg in targets {
                for (p, a) in f.block_params(tg.block).iter().zip(&tg.args) {
                    let (cs, cd) = (cand_of[a.0 as usize], cand_of[p.0 as usize]);
                    if cs != usize::MAX && cd != usize::MAX && !is_imm[cs] {
                        pairs.entry((cs, cd)).or_default().push(end[bi]);
                    }
                }
                for p in f.block_params(tg.block) {
                    let c = cand_of[p.0 as usize];
                    if c != usize::MAX {
                        // A run of its own: the parameter holds nothing in between.
                        segs[c].push((end[bi], end[bi]));
                        weight[c] += w;
                    }
                }
            }
        }
        for &c in &touched {
            let close = if live_out[bi].get(c) {
                end[bi]
            } else {
                last[c]
            };
            segs[c].push((open[c], close));
            open[c] = u32::MAX;
        }
    }
    // A hoisted constant is loaded once at the entry and keeps its register for the whole function.
    for c in 0..nc {
        if is_imm[c] {
            segs[c] = vec![(0, at)];
            let k = imms[cands[c].0 as usize].map_or(1, |n| mov_len(n) as u64 - 1);
            weight[c] *= k;
        }
    }
    // Runs that overlap are one (runs that only follow each other stay apart: a jump may write a value there).
    for sg in &mut segs {
        sg.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(sg.len());
        for &(a, b) in sg.iter() {
            match merged.last_mut() {
                Some(m) if a <= m.1 => m.1 = m.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        *sg = merged;
    }

    // Positions where the generator calls (the runtime or another function): the argument registers
    // and every scratch register die there, so a value that touches one needs a callee-saved home.
    let mut calls: Vec<u32> = Vec::new();
    for b in &f.blocks {
        for &i in &b.insts {
            let call = match &f.insts[i].op {
                Op::Call(..)
                | Op::CallPtr(..)
                | Op::MemCopy(..)
                | Op::Print(..)
                | Op::Alloc(..)
                | Op::Free(..)
                | Op::Realloc(..) => true,
                Op::Bin(BinOp::Rem, a, _) => matches!(f.ty(*a), IrTy::F32 | IrTy::F64),
                _ => false,
            };
            if call {
                calls.push(inst_pos[i]);
            }
        }
    }
    calls.sort_unstable();
    let crosses_call = |c: usize| {
        segs[c].iter().any(|&(l, h)| {
            let k = calls.partition_point(|&p| p < l);
            calls.get(k).is_some_and(|&p| p <= h)
        })
    };
    // The parameters of the function arrive in the argument registers (the first eight of each kind) and
    // are moved out first, so one may stay where it arrives but takes no other caller-saved home.
    let mut arrival: Vec<Option<u8>> = vec![None; nc];
    let mut entry_param = vec![false; nc];
    let (mut ni, mut nf) = (0u8, 0u8);
    for p in f.block_params(f.entry()) {
        let c = cand_of[p.0 as usize];
        let n = if class(f.values[p.0 as usize]) == Some(Class::Fpr) {
            &mut nf
        } else {
            &mut ni
        };
        if c != usize::MAX {
            entry_param[c] = true;
            if *n < 8 {
                arrival[c] = Some(*n);
            }
        }
        *n += 1;
    }

    let mut order: Vec<usize> = (0..nc).filter(|&c| weight[c] >= MIN_FREE_WEIGHT).collect();
    order.sort_by(|a, b| weight[*b].cmp(&weight[*a]).then(a.cmp(b)));
    // Caller-saved homes come first in the lists: `x0` to `x8` (the templates only use `x9` and up
    // outside calls) and `d2` to `d7` (the templates use `v0` and `v1`).
    let gprs: Vec<(Reg, bool)> = (0..=8)
        .take(limit)
        .map(|r| (Reg(r as u8), false))
        .chain((19..=28).take(limit).map(|r| (Reg(r as u8), true)))
        .collect();
    let fprs: Vec<(V, bool)> = (2..=7)
        .take(limit)
        .map(|r| (V(r as u8), false))
        .chain((8..=15).take(limit).map(|r| (V(r as u8), true)))
        .collect();
    // Per register, the values already given to it.
    let mut taken_g: Vec<Vec<usize>> = vec![Vec::new(); gprs.len()];
    let mut taken_f: Vec<Vec<usize>> = vec![Vec::new(); fprs.len()];
    // Vectors: `v16` to `v31` (the templates use `v0` and `v1`), no callee-saved pool.
    let vprs: Vec<(V, bool)> = (16..=31).take(limit).map(|r| (V(r as u8), false)).collect();
    let mut taken_v: Vec<Vec<usize>> = vec![Vec::new(); vprs.len()];
    // Whether `c` may share a register with `o`: their runs are apart, or they meet only at a jump that
    // hands one to the other (the argument dies there, the parameter starts there).
    let fits = |c: usize, o: usize| {
        let (sc, so) = (&segs[c], &segs[o]);
        let (mut i, mut j) = (0, 0);
        while i < sc.len() && j < so.len() {
            let (lo_, hi_) = (sc[i].0.max(so[j].0), sc[i].1.min(so[j].1));
            if lo_ <= hi_ {
                let coupled = |s: usize, d: usize| {
                    lo_ == hi_ && pairs.get(&(s, d)).is_some_and(|v| v.contains(&lo_))
                };
                if !(coupled(o, c) || coupled(c, o)) {
                    return false;
                }
            }
            if sc[i].1 < so[j].1 {
                i += 1;
            } else {
                j += 1;
            }
        }
        true
    };
    let crossing_all: Vec<bool> = (0..nc).map(crosses_call).collect();
    let mut prefer_callee = crossing_all.clone();
    for &(s, d) in pairs.keys() {
        if s != d && (crossing_all[s] || crossing_all[d]) && fits(s, d) {
            prefer_callee[s] = true;
            prefer_callee[d] = true;
        }
    }
    // The register for `c` among `homes` (with the values already in them): one held by a partner of a
    // copy first, since sharing it removes the copy, then the first that is free.
    let pick = |c: usize, homes: &[(bool, u8)], taken: &[Vec<usize>]| -> Option<usize> {
        let crossing = crosses_call(c);
        let ok = |k: usize| {
            let (callee, id) = homes[k];
            let allowed = if callee {
                weight[c] >= MIN_WEIGHT
            } else {
                !crossing && (!entry_param[c] || arrival[c] == Some(id))
            };
            allowed && taken[k].iter().all(|&o| fits(c, o))
        };
        let partner = |k: usize| {
            taken[k]
                .iter()
                .any(|&o| pairs.contains_key(&(o, c)) || pairs.contains_key(&(c, o)))
        };
        // A value that shares with one living across a call goes to the callee-saved registers too, so
        // that the two can meet in one.
        let callee_first = prefer_callee[c];
        let order: Vec<usize> = (0..homes.len())
            .filter(|&k| homes[k].0 == callee_first)
            .chain((0..homes.len()).filter(|&k| homes[k].0 != callee_first))
            .collect();
        order
            .iter()
            .copied()
            .find(|&k| ok(k) && partner(k))
            .or_else(|| order.iter().copied().find(|&k| ok(k)))
    };
    let kinds_g: Vec<(bool, u8)> = gprs.iter().map(|g| (g.1, g.0 .0)).collect();
    let kinds_f: Vec<(bool, u8)> = fprs.iter().map(|g| (g.1, g.0 .0)).collect();
    let kinds_v: Vec<(bool, u8)> = vprs.iter().map(|g| (g.1, g.0 .0)).collect();
    for c in order {
        let v = cands[c];
        match class(f.values[v.0 as usize]).unwrap() {
            Class::Gpr => {
                if let Some(k) = pick(c, &kinds_g, &taken_g) {
                    taken_g[k].push(c);
                    out.reg[v.0 as usize] = Some(gprs[k].0);
                }
            }
            Class::Vec => {
                if let Some(k) = pick(c, &kinds_v, &taken_v) {
                    taken_v[k].push(c);
                    out.vreg[v.0 as usize] = Some(vprs[k].0);
                }
            }
            Class::Fpr => {
                if let Some(k) = pick(c, &kinds_f, &taken_f) {
                    taken_f[k].push(c);
                    out.freg[v.0 as usize] = Some(fprs[k].0);
                }
            }
        }
    }
    out
}
