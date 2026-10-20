//! Register assignment for the native code generator (stage N5).
//!
//! A simple, safe scheme: some values live in a callee-saved register for their whole life, all
//! the others stay in their stack slot as before. Which values: the ones used most, weighted by how
//! deeply they sit in loops, among those that can share a register because their live ranges do not
//! overlap. Callee-saved registers survive calls, so no value is ever saved or restored around one.
//!
//! * Liveness comes from the usual backward dataflow over the blocks (a block's parameters are
//!   defined at its start; the arguments of a jump are used by the jump).
//! * Each value gets one interval: from its definition to its last use, widened to cover every
//!   block where it is live. Two values may share a register only when their intervals are disjoint.

use crate::ir::{Func, IrTy, Op, Term, Value};

/// Positions in the linear order of the function: even numbers for the block start, the
/// instructions and the terminator, in layout order.
struct Order {
    block_start: Vec<u32>,
    block_end: Vec<u32>,
    inst_pos: Vec<u32>,
}

fn order(f: &Func) -> Order {
    let mut o = Order {
        block_start: vec![0; f.blocks.len()],
        block_end: vec![0; f.blocks.len()],
        inst_pos: vec![0; f.insts.len()],
    };
    let mut p = 0u32;
    for (bi, b) in f.blocks.iter().enumerate() {
        o.block_start[bi] = p;
        p += 2;
        for &i in &b.insts {
            o.inst_pos[i] = p;
            p += 2;
        }
        o.block_end[bi] = p;
        p += 2;
    }
    o
}

fn inst_uses(op: &Op, out: &mut Vec<Value>) {
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

fn successors(t: &Term) -> Vec<usize> {
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

/// For each value, the register it lives in, if any.
///
/// `saved` are callee-saved registers (any value may live in them); `scratch` are caller-saved ones,
/// used only for values whose live range has no call or other runtime routine inside it.
pub fn assign(
    f: &Func,
    saved: &[crate::native::x86::Reg],
    scratch: &[crate::native::x86::Reg],
) -> Vec<Option<crate::native::x86::Reg>> {
    let nv = f.values.len();
    let nb = f.blocks.len();
    let o = order(f);

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
            inst_uses(&f.insts[i].op, &mut tmp);
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
            term_uses(t, &mut tmp);
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

    // Intervals and weights.
    let mut start = vec![u32::MAX; nv];
    let mut end = vec![0u32; nv];
    let mut weight = vec![0u64; nv];
    let mut touch = |v: usize, pos: u32, w: u64, start: &mut Vec<u32>, end: &mut Vec<u32>| {
        start[v] = start[v].min(pos);
        end[v] = end[v].max(pos);
        weight[v] += w;
    };
    let mut entry_params = Vec::new();
    if let Some(b0) = f.blocks.first() {
        entry_params = b0.params.clone();
    }
    for p in &entry_params {
        touch(p.0 as usize, 0, 1, &mut start, &mut end);
    }
    for (bi, b) in f.blocks.iter().enumerate() {
        let w = 10u64.pow(depth[bi].min(6));
        for p in &b.params {
            touch(p.0 as usize, o.block_start[bi], w, &mut start, &mut end);
        }
        for &i in &b.insts {
            let pos = o.inst_pos[i];
            tmp.clear();
            inst_uses(&f.insts[i].op, &mut tmp);
            for v in tmp.clone() {
                touch(v.0 as usize, pos, w, &mut start, &mut end);
            }
            if let Some(r) = f.insts[i].result {
                touch(r.0 as usize, pos, w, &mut start, &mut end);
            }
        }
        tmp.clear();
        if let Some(t) = &b.term {
            term_uses(t, &mut tmp);
        }
        for v in tmp.clone() {
            touch(v.0 as usize, o.block_end[bi], w, &mut start, &mut end);
        }
        for v in 0..nv {
            if live_in[bi].has(v) {
                touch(v, o.block_start[bi], 0, &mut start, &mut end);
            }
            if live_out[bi].has(v) {
                touch(v, o.block_end[bi], 0, &mut start, &mut end);
            }
        }
    }

    // Greedy assignment, heaviest first.
    // Constants are used as immediates and take no register (the generator knows them by value).
    let mut is_const = vec![false; nv];
    for b in &f.blocks {
        for &i in &b.insts {
            if let (Op::Const(..), Some(r)) = (&f.insts[i].op, f.insts[i].result) {
                is_const[r.0 as usize] = true;
            }
        }
    }
    let eligible = |v: usize| {
        matches!(f.values[v], IrTy::Int(_) | IrTy::Bool | IrTy::Ptr)
            && start[v] != u32::MAX
            && !is_const[v]
    };
    let mut cands: Vec<usize> = (0..nv).filter(|v| eligible(*v) && weight[*v] > 0).collect();
    cands.sort_by_key(|v| (std::cmp::Reverse(weight[*v]), *v));
    // Positions where the generated code calls out and so may change every caller-saved register.
    let mut clobbers = Vec::new();
    for b in &f.blocks {
        for &i in &b.insts {
            if matches!(
                f.insts[i].op,
                Op::Call(..)
                    | Op::CallPtr(..)
                    | Op::Alloc(_)
                    | Op::Free(_)
                    | Op::Print(..)
                    | Op::MemCopy(..)
            ) {
                clobbers.push(o.inst_pos[i]);
            }
        }
    }
    let all: Vec<_> = scratch.iter().chain(saved.iter()).copied().collect();
    let mut taken: Vec<Vec<(u32, u32)>> = vec![Vec::new(); all.len()];
    let mut out = vec![None; nv];
    for v in cands {
        // A value defined or last used by a call is fine: its register is written after, read before.
        let over_call = clobbers.iter().any(|&p| start[v] < p && p < end[v]);
        for (ri, reg) in all.iter().enumerate() {
            if ri < scratch.len() && over_call {
                continue;
            }
            let clash = taken[ri].iter().any(|&(s, e)| start[v] <= e && s <= end[v]);
            if !clash {
                taken[ri].push((start[v], end[v]));
                out[v] = Some(*reg);
                break;
            }
        }
    }
    out
}
