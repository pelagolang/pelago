//! The loop test on another counter (card A-11). `for i in 0..N` with another counter `q` that moves
//! with `i` (a length that grows by 2 each trip) carries both: the test reads `i`, the work reads `q`,
//! and each trip pays for the two additions. Since `q = q0 + K * k` after `k` trips, `i < N` says the
//! same as `q < q0 + K * trips`, and when nothing else reads `i` it goes away:
//!
//! ```text
//! H(i, q):  c = lt i, 2000000 ; branch c, B, X         H(i, q):  c = lt q, 4000000 ; branch c, B, X
//! B:        ... jump H(i + 1, q + 2)             ->    B:        ... jump H(q + 2)
//! ```
//!
//! The loop is the one `ranges.rs` knows how to count: header `branch (lt i, N)` with a constant `N`,
//! `i` entered at a constant and stepped by a constant on the only way back, the body entered only
//! through the true side. `q` is entered at a constant and gains a constant above zero on the same
//! way back. The new limit must fit the type, so `q` never wraps before the loop ends.

use super::dom::Dom;
use crate::ir::*;

pub fn run(f: &mut Func) -> bool {
    let mut changed = false;
    for _ in 0..16 {
        if !once(f) {
            break;
        }
        changed = true;
    }
    changed
}

fn once(f: &mut Func) -> bool {
    let dom = Dom::new(f);
    let defs = f.defs();
    let uses = f.use_counts();
    let int_of = |v: Value| match defs[v.0 as usize].map(|d| &f.insts[d].op) {
        Some(Op::Const(_, Const::Int(n))) => Some(*n),
        _ => None,
    };
    let sum = |v: Value| match defs[v.0 as usize].map(|d| &f.insts[d].op) {
        Some(Op::Bin(BinOp::Add, a, b) | Op::Checked(BinOp::Add, _, a, b)) => Some((*a, *b)),
        _ => None,
    };
    // The constant added to `p` by `v = p + K`.
    let step = |v: Value, p: Value| {
        let (a, b) = sum(v)?;
        if a == p {
            int_of(b)
        } else if b == p {
            int_of(a)
        } else {
            None
        }
    };
    // Every edge into each block, as (from, arguments).
    let mut edges: Vec<Vec<(BlockId, Vec<Value>)>> = vec![Vec::new(); f.blocks.len()];
    for (bi, b) in f.blocks.iter().enumerate() {
        let from = BlockId(bi as u32);
        if !dom.reachable(from) {
            continue;
        }
        match &b.term {
            Some(Term::Jump(t)) => edges[t.block.0 as usize].push((from, t.args.clone())),
            Some(Term::Branch(_, a, c)) => {
                edges[a.block.0 as usize].push((from, a.args.clone()));
                edges[c.block.0 as usize].push((from, c.args.clone()));
            }
            _ => {}
        }
    }
    for &h in &dom.rpo {
        let hi = h.0 as usize;
        let [e0, e1] = edges[hi].as_slice() else {
            continue;
        };
        let (entry, latch) = match (
            dom.dominates(h, e0.0) && e0.0 != h,
            dom.dominates(h, e1.0) && e1.0 != h,
        ) {
            (false, true) => (e0, e1),
            (true, false) => (e1, e0),
            _ => continue,
        };
        let Some(Term::Branch(cond, yes, _)) = &f.blocks[hi].term else {
            continue;
        };
        if !yes.args.is_empty()
            || edges[yes.block.0 as usize].len() != 1
            || !dom.dominates(yes.block, latch.0)
        {
            continue;
        }
        let Some(ci) = defs[cond.0 as usize] else {
            continue;
        };
        let Op::Cmp(CmpOp::Lt, c, n) = f.insts[ci].op else {
            continue;
        };
        let params = &f.blocks[hi].params;
        let Some(kc) = params.iter().position(|p| *p == c) else {
            continue;
        };
        let (Some(limit), Some(c0), Some(sc)) =
            (int_of(n), int_of(entry.1[kc]), step(latch.1[kc], c))
        else {
            continue;
        };
        // `c` is read by the test and its own step only, and the step by the jump back only.
        if sc <= 0
            || uses[c.0 as usize] != 2
            || uses[latch.1[kc].0 as usize] != 1
            || uses[cond.0 as usize] != 1
        {
            continue;
        }
        let trips = if limit > c0 {
            (limit - c0 + sc - 1) / sc
        } else {
            0
        };
        for (kq, &q) in params.iter().enumerate() {
            let ty = f.ty(q);
            if kq == kc || !ty.is_int() {
                continue;
            }
            let (Some(q0), Some(k)) = (int_of(entry.1[kq]), step(latch.1[kq], q)) else {
                continue;
            };
            let bound = q0 + k * trips;
            let bits = match ty {
                IrTy::Int(b) => b as u32,
                _ => continue,
            };
            if k <= 0 || q0 < 0 || bound >= 1i128 << (bits - 1) {
                continue;
            }
            // Put the test on `q`.
            let span = f.insts[ci].span;
            let nb = f.new_value(ty);
            f.insts.push(Inst {
                result: Some(nb),
                op: Op::Const(ty, Const::Int(bound)),
                span,
            });
            let ni = f.insts.len() - 1;
            f.insts[ci].op = Op::Cmp(CmpOp::Lt, q, nb);
            let at = f.blocks[hi].insts.iter().position(|x| *x == ci).unwrap();
            f.blocks[hi].insts.insert(at, ni);
            return true;
        }
    }
    false
}
