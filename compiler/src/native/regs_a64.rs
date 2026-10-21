//! Register assignment for the AArch64 generator (card A-10, stage R3).
//!
//! The heaviest integer, boolean and pointer values live in `x19` to `x28`, the heaviest floats in
//! `d8` to `d15`, for their whole life; every other value keeps its frame slot. These registers are
//! callee-saved in our convention (the runtime keeps them too), so calls leave them alone and no value
//! is ever split: the allocator only has to decide who gets a register and who shares one.
//!
//! A value whose interval touches no call (a function call, an `alloc`, a `memcpy`, a float remainder:
//! see `calls` below) may instead live in a caller-saved register, which costs no save and restore:
//! `x0` to `x8` (the templates only use `x9` and up outside calls) and `d2` to `d7`. A parameter of the
//! function may stay in the register it arrives in but takes no other caller-saved one.
//!
//! * Liveness is the usual backward dataflow over the blocks (a block's parameters are defined at its
//!   start, the arguments of a jump are read by the jump).
//! * Each value gets one **interval**, the smallest range of positions in the linear order of the
//!   function that holds every position where it is live, read or written. A block parameter is also
//!   written at each jump that leads to its block. Two values may share a register only when their
//!   intervals are apart, and an instruction counts as one position, so a result never takes the
//!   register of an operand of the same instruction (the generator does not have to care about that).
//! * Copies are coalesced where the intervals allow it: a value passed to a block parameter at a jump may
//!   share its register with that parameter when it dies at the jump and the parameter starts there, so the
//!   copy of the edge disappears.
//! * Values are taken heaviest first (reads and writes weighted by `10^loop depth`) and get the first
//!   register whose assigned intervals do not meet theirs. A value must weigh at least [`MIN_WEIGHT`]
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
}

fn class(t: IrTy) -> Option<Class> {
    match t {
        IrTy::Int(_) | IrTy::Bool | IrTy::Ptr => Some(Class::Gpr),
        IrTy::F32 | IrTy::F64 => Some(Class::Fpr),
        IrTy::V2I64 | IrTy::Void => None,
    }
}

/// Assigns registers. `imms` are the constants (no home at all), `uses` the use counts and `fused` the
/// compares that only feed their branch (never stored).
pub fn assign(f: &Func, imms: &[Option<i64>], uses: &[u32], fused: &[bool]) -> Alloc {
    let n = f.values.len();
    let mut out = Alloc {
        reg: vec![None; n],
        freg: vec![None; n],
    };
    let limit = reg_limit();
    if limit == 0 {
        return out;
    }
    // The candidates, numbered densely so the bit sets stay small.
    let mut cand_of = vec![usize::MAX; n];
    let mut cands: Vec<Value> = Vec::new();
    for v in 0..n {
        if imms[v].is_none() && uses[v] > 0 && !fused[v] && class(f.values[v]).is_some() {
            cand_of[v] = cands.len();
            cands.push(Value(v as u32));
        }
    }
    if cands.is_empty() {
        return out;
    }
    let nb = f.blocks.len();
    let nc = cands.len();

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

    let mut lo = vec![u32::MAX; nc];
    let mut hi = vec![0u32; nc];
    let mut weight = vec![0u64; nc];
    // (argument, parameter) of a jump, with the position of the jump.
    let mut pairs: std::collections::HashMap<(usize, usize), u32> =
        std::collections::HashMap::new();
    let mut touch = |c: usize, pos: u32| {
        lo[c] = lo[c].min(pos);
        hi[c] = hi[c].max(pos);
    };
    for bi in 0..nb {
        for c in 0..nc {
            if live_in[bi].get(c) {
                touch(c, start[bi]);
            }
            if live_out[bi].get(c) {
                touch(c, end[bi]);
            }
        }
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        let w = weight_of(bi);
        for p in &b.params {
            let c = cand_of[p.0 as usize];
            if c != usize::MAX {
                touch(c, start[bi]);
                weight[c] += w;
            }
        }
        for &i in &b.insts {
            reads.clear();
            f.insts[i].op.for_each_value(|v| reads.push(v));
            for v in reads.iter().chain(f.insts[i].result.iter()) {
                let c = cand_of[v.0 as usize];
                if c != usize::MAX {
                    touch(c, inst_pos[i]);
                    weight[c] += w;
                }
            }
        }
        if let Some(t) = &b.term {
            reads.clear();
            t.for_each_value(|v| reads.push(v));
            for v in &reads {
                let c = cand_of[v.0 as usize];
                if c != usize::MAX {
                    touch(c, end[bi]);
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
                    if cs != usize::MAX && cd != usize::MAX {
                        pairs.insert((cs, cd), end[bi]);
                    }
                }
                for p in f.block_params(tg.block) {
                    let c = cand_of[p.0 as usize];
                    if c != usize::MAX {
                        touch(c, end[bi]);
                        weight[c] += w;
                    }
                }
            }
        }
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
    let crosses_call = |l: u32, h: u32| {
        let k = calls.partition_point(|&p| p < l);
        calls.get(k).is_some_and(|&p| p <= h)
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
    // Per register, the intervals already given to it, with their owners.
    let mut taken_g: Vec<Vec<(u32, u32, usize)>> = vec![Vec::new(); gprs.len()];
    let mut taken_f: Vec<Vec<(u32, u32, usize)>> = vec![Vec::new(); fprs.len()];
    // Whether `c` (living in `l..=h`) may share a register with an interval `a..=b` of `o`.
    let fits = |c: usize, (l, h): (u32, u32), &(a, b, o): &(u32, u32, usize)| {
        h < a
            || b < l
            || pairs
                .get(&(o, c))
                .is_some_and(|&e| hi[o] == e && lo[c] == e && b == e && l == e)
            || pairs
                .get(&(c, o))
                .is_some_and(|&e| hi[c] == e && lo[o] == e && h == e && a == e)
    };
    // The register for `c` among `homes` (with their taken intervals): one held by a partner of a copy
    // first, since sharing it removes the copy, then the first that is free.
    let pick =
        |c: usize, homes: &[(bool, u8)], taken: &[Vec<(u32, u32, usize)>]| -> Option<usize> {
            let (l, h) = (lo[c], hi[c]);
            let crossing = crosses_call(l, h);
            let ok = |k: usize| {
                let (callee, id) = homes[k];
                let allowed = if callee {
                    weight[c] >= MIN_WEIGHT
                } else {
                    !crossing && (!entry_param[c] || arrival[c] == Some(id))
                };
                allowed && taken[k].iter().all(|t| fits(c, (l, h), t))
            };
            let partner = |k: usize| {
                taken[k]
                    .iter()
                    .any(|t| pairs.contains_key(&(t.2, c)) || pairs.contains_key(&(c, t.2)))
            };
            (0..homes.len())
                .find(|&k| ok(k) && partner(k))
                .or_else(|| (0..homes.len()).find(|&k| ok(k)))
        };
    let kinds_g: Vec<(bool, u8)> = gprs.iter().map(|g| (g.1, g.0 .0)).collect();
    let kinds_f: Vec<(bool, u8)> = fprs.iter().map(|g| (g.1, g.0 .0)).collect();
    for c in order {
        let v = cands[c];
        match class(f.values[v.0 as usize]).unwrap() {
            Class::Gpr => {
                if let Some(k) = pick(c, &kinds_g, &taken_g) {
                    taken_g[k].push((lo[c], hi[c], c));
                    out.reg[v.0 as usize] = Some(gprs[k].0);
                }
            }
            Class::Fpr => {
                if let Some(k) = pick(c, &kinds_f, &taken_f) {
                    taken_f[k].push((lo[c], hi[c], c));
                    out.freg[v.0 as usize] = Some(fprs[k].0);
                }
            }
        }
    }
    out
}
