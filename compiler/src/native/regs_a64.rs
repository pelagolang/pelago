//! Register assignment for the AArch64 generator (card A-10, stage R3).
//!
//! The heaviest integer, boolean and pointer values live in `x19` to `x28`, the heaviest floats in
//! `d8` to `d15`, for their whole life; every other value keeps its frame slot. These registers are
//! callee-saved in our convention (the runtime keeps them too), so calls leave them alone and no value
//! is ever split: the allocator only has to decide who gets a register and who shares one.
//!
//! * Liveness is the usual backward dataflow over the blocks (a block's parameters are defined at its
//!   start, the arguments of a jump are read by the jump).
//! * Each value gets one **interval**, the smallest range of positions in the linear order of the
//!   function that holds every position where it is live, read or written. A block parameter is also
//!   written at each jump that leads to its block. Two values may share a register only when their
//!   intervals are apart, and an instruction counts as one position, so a result never takes the
//!   register of an operand of the same instruction (the generator does not have to care about that).
//! * Values are taken heaviest first (reads and writes weighted by `10^loop depth`) and get the first
//!   register whose assigned intervals do not meet theirs. A value must weigh at least [`MIN_WEIGHT`]
//!   to be worth the save and restore of its register.

use crate::ir::{Func, IrTy, Term, Value};
use crate::native::a64::{Reg, V};

/// The least weight that pays for a callee-saved register (one save and one restore per call).
const MIN_WEIGHT: u64 = 3;

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

    let mut order: Vec<usize> = (0..nc).filter(|&c| weight[c] >= MIN_WEIGHT).collect();
    order.sort_by(|a, b| weight[*b].cmp(&weight[*a]).then(a.cmp(b)));
    let gprs: Vec<Reg> = (19..=28).take(limit).map(|r| Reg(r as u8)).collect();
    let fprs: Vec<V> = (8..=15).take(limit).map(|r| V(r as u8)).collect();
    // Per register, the intervals already given to it.
    let mut taken_g: Vec<Vec<(u32, u32)>> = vec![Vec::new(); gprs.len()];
    let mut taken_f: Vec<Vec<(u32, u32)>> = vec![Vec::new(); fprs.len()];
    for c in order {
        let (l, h) = (lo[c], hi[c]);
        let v = cands[c];
        let free = |taken: &Vec<(u32, u32)>| taken.iter().all(|&(a, b)| h < a || b < l);
        match class(f.values[v.0 as usize]).unwrap() {
            Class::Gpr => {
                if let Some(k) = (0..gprs.len()).find(|&k| free(&taken_g[k])) {
                    taken_g[k].push((l, h));
                    out.reg[v.0 as usize] = Some(gprs[k]);
                }
            }
            Class::Fpr => {
                if let Some(k) = (0..fprs.len()).find(|&k| free(&taken_f[k])) {
                    taken_f[k].push((l, h));
                    out.freg[v.0 as usize] = Some(fprs[k]);
                }
            }
        }
    }
    out
}
