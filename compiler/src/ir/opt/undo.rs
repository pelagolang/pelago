//! Recomputing the operand of a checked add instead of keeping it (card A-11). A loop that grows a
//! length reads
//!
//! ```text
//! n2 = add_checked n, 2        n2 = add_checked n, 2
//! p  = ptradd base, n          p  = ptradd (ptradd base, n2), -2
//! ```
//!
//! and with the first form `n` and `n2` are alive at once: two registers, and a copy at the end of
//! the iteration because the new length must land where the old one was. When the checked add
//! succeeded, `n` is exactly `n2 - 2`, so the later uses of `n` can read `n2`: an address takes the
//! `-2` into its displacement, which costs nothing. Then `n` dies at the add, `n2` takes its register
//! and the loop adds in place.
//!
//! Applies to `add_checked x, K` with a small positive constant `K` when at least one use of `x` after
//! the add is an address offset and the others after it are in blocks that call something (the slow
//! path that grows the buffer), where one `sub` is no matter. Uses the add does not dominate (the loop's
//! exit, which sees the loop parameter itself) are left alone.

use super::dom::Dom;
use crate::ir::*;

pub fn run(f: &mut Func) -> bool {
    let mut changed = false;
    for _ in 0..64 {
        if !once(f) {
            break;
        }
        changed = true;
    }
    changed
}

/// One use of the operand: the block, the instruction (or `None` for the terminator).
struct Use {
    block: usize,
    inst: Option<usize>,
}

fn once(f: &mut Func) -> bool {
    let dom = Dom::new(f);
    let defs = f.defs();
    let int_of = |v: Value| match defs[v.0 as usize].map(|i| &f.insts[i].op) {
        Some(Op::Const(_, Const::Int(n))) => Some(*n),
        _ => None,
    };
    let calls: Vec<bool> = f
        .blocks
        .iter()
        .map(|b| {
            b.insts
                .iter()
                .any(|&i| matches!(f.insts[i].op, Op::Call(..) | Op::CallPtr(..)))
        })
        .collect();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !dom.reachable(BlockId(bi as u32)) {
            continue;
        }
        for (pos, &ai) in b.insts.iter().enumerate() {
            let Op::Checked(BinOp::Add, _, x, c) = f.insts[ai].op else {
                continue;
            };
            let Some(r) = f.insts[ai].result else {
                continue;
            };
            let Some(k) = int_of(c) else { continue };
            if !(1..1 << 30).contains(&k) || f.ty(x) != IrTy::I64 || int_of(x).is_some() {
                continue;
            }
            // Every other use of `x`.
            let mut uses: Vec<Use> = Vec::new();
            for (ub, blk) in f.blocks.iter().enumerate() {
                if !dom.reachable(BlockId(ub as u32)) {
                    continue;
                }
                let after = |at: Option<usize>| {
                    if ub == bi {
                        match at {
                            Some(i) => blk.insts.iter().position(|y| *y == i).unwrap() > pos,
                            None => true,
                        }
                    } else {
                        dom.dominates(BlockId(bi as u32), BlockId(ub as u32))
                    }
                };
                for &i in &blk.insts {
                    if i == ai {
                        continue;
                    }
                    let mut hit = false;
                    f.insts[i].op.for_each_value(|v| hit |= v == x);
                    if hit && after(Some(i)) {
                        uses.push(Use {
                            block: ub,
                            inst: Some(i),
                        });
                    }
                }
                if let Some(t) = &blk.term {
                    let mut hit = false;
                    t.for_each_value(|v| hit |= v == x);
                    if hit && after(None) {
                        uses.push(Use {
                            block: ub,
                            inst: None,
                        });
                    }
                }
            }
            if uses.is_empty() {
                continue;
            }
            let offset = |u: &Use| {
                u.inst
                    .is_some_and(|i| matches!(f.insts[i].op, Op::PtrAdd(p, o) if o == x && p != x))
            };
            if !uses.iter().any(offset) || !uses.iter().all(|u| offset(u) || calls[u.block]) {
                continue;
            }
            apply(f, r, x, k, uses);
            return true;
        }
    }
    false
}

fn apply(f: &mut Func, r: Value, x: Value, k: i128, uses: Vec<Use>) {
    for u in uses {
        let span = u.inst.map(|i| f.insts[i].span).unwrap_or_default();
        let at = |f: &Func, u: &Use| match u.inst {
            Some(i) => f.blocks[u.block]
                .insts
                .iter()
                .position(|y| *y == i)
                .unwrap(),
            None => f.blocks[u.block].insts.len(),
        };
        let push = |f: &mut Func, ty: IrTy, op: Op| {
            let v = f.new_value(ty);
            f.insts.push(Inst {
                result: Some(v),
                op,
                span,
            });
            (v, f.insts.len() - 1)
        };
        let offset = u.inst.and_then(|i| match f.insts[i].op {
            Op::PtrAdd(p, o) if o == x && p != x => Some(p),
            _ => None,
        });
        if let (Some(p), Some(i)) = (offset, u.inst) {
            let (kneg, kn) = push(f, IrTy::I64, Op::Const(IrTy::I64, Const::Int(-k)));
            let (q, qi) = push(f, IrTy::Ptr, Op::PtrAdd(p, r));
            f.insts[i].op = Op::PtrAdd(q, kneg);
            let pos = at(f, &u);
            f.blocks[u.block].insts.splice(pos..pos, [kn, qi]);
        } else {
            let (kc, kn) = push(f, IrTy::I64, Op::Const(IrTy::I64, Const::Int(k)));
            let (s, si) = push(f, IrTy::I64, Op::Bin(BinOp::Sub, r, kc));
            let pos = at(f, &u);
            f.blocks[u.block].insts.splice(pos..pos, [kn, si]);
            match u.inst {
                Some(i) => f.insts[i].op.map_values(|v| if v == x { s } else { v }),
                None => {
                    if let Some(t) = &mut f.blocks[u.block].term {
                        t.map_values(|v| if v == x { s } else { v });
                    }
                }
            }
        }
    }
}
