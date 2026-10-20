//! Unrolling of small counted loops (card A-11). `while i < n { body; i += s }` pays for the test,
//! the branch and the counter on every pass, which for a body of a handful of instructions is most
//! of the time. The loop becomes two: a main loop that runs `K` copies of the body per pass and a
//! test that covers all of them, and the original loop, which finishes the last `K - 1` passes or
//! fewer.
//!
//! The main loop's test is `i < n - (K - 1) * s`: when it holds, `i + j * s < n` for every copy `j`,
//! so each copy would have passed the original test, and the copies run in the original order with
//! the original operations (a checked one still stops in the same pass). `n - (K - 1) * s` is made
//! once in front of the loop, with a `select` that makes the main loop run zero times if the
//! subtraction would pass the smallest integer.
//!
//! Only loops that are known to run long (the bound is a constant of at least 256 passes) are
//! unrolled, since the copies cost code size. Only loops of two blocks (the header holds just the test, the body jumps back), a test of one
//! counter against a value the loop does not change, a constant positive step, and a body without
//! calls or allocation. The code grows by `K - 1` bodies, so `K * size` stays under [`BUDGET`]; the
//! pass runs once per function, after inlining, so the copies are not unrolled again.

use super::dom::Dom;
use super::loops;
use crate::ir::*;
use std::collections::{HashMap, HashSet};

/// Instructions in the unrolled body.
const BUDGET: usize = 64;
const MAX_FACTOR: usize = 8;
/// The bound must allow at least this many passes.
const MIN_TRIPS: i128 = 256;

/// The factor the budget allows for a body of `size` instructions.
fn k_for(size: usize) -> usize {
    [MAX_FACTOR, 4, 2]
        .into_iter()
        .find(|k| k * size <= BUDGET)
        .unwrap_or(1)
}

/// Whether `k` copies are estimated to take clearly less time per pass than one. A pass costs the
/// longer of its longest chain through the loop-carried values (a float sum waits for the last one
/// whatever else is around) and its instructions at about three and a third a cycle, plus some for
/// the taken branch; the test, branch and counter are paid once per `k` passes instead of once.
/// Only a loop that is bound by the instructions, not by a chain, gains.
fn pays(f: &Func, params: &[Value], body: &Block, back: &Target, k: usize) -> bool {
    // A loop that writes memory is bound by the store port or the cache, not by its counter.
    if k < 2
        || body
            .insts
            .iter()
            .any(|i| matches!(f.insts[*i].op, Op::Store(..) | Op::MemCopy(..)))
    {
        return false;
    }
    let mut uops = 1.0; // the test and branch, fused
    let mut lats: Vec<f64> = Vec::new();
    for &ii in &body.insts {
        let inst = &f.insts[ii];
        let float = inst
            .result
            .is_some_and(|r| matches!(f.ty(r), IrTy::F32 | IrTy::F64));
        lats.push(match &inst.op {
            Op::Const(..) => 0.0,
            Op::Bin(o, ..) | Op::Checked(o, ..) => match o {
                _ if float => 4.0,
                BinOp::Mul => 3.0,
                BinOp::MulHi | BinOp::UMulHi => 4.0,
                BinOp::Div | BinOp::Rem | BinOp::UDiv | BinOp::URem => 25.0,
                _ => 1.0,
            },
            Op::Load(..) => 5.0,
            _ => 1.0,
        });
        if !matches!(inst.op, Op::Const(..)) {
            uops += 1.0;
        }
    }
    // The longest chain from each loop-carried value back to itself.
    let mut rec: f64 = 1.0;
    for (idx, p) in params.iter().enumerate() {
        let mut depth: HashMap<Value, f64> = HashMap::new();
        depth.insert(*p, 0.0);
        for (n, &ii) in body.insts.iter().enumerate() {
            let inst = &f.insts[ii];
            let mut d: Option<f64> = None;
            inst.op.for_each_value(|v| {
                if let Some(x) = depth.get(&v) {
                    d = Some(d.map_or(*x, |y| y.max(*x)));
                }
            });
            if let (Some(d), Some(r)) = (d, inst.result) {
                depth.insert(r, d + lats[n]);
            }
        }
        if let Some(x) = depth.get(&back.args[idx]) {
            rec = rec.max(*x);
        }
    }
    let before = rec.max(uops / 3.3 + 0.4);
    let saved = 2.0 * (1.0 - 1.0 / k as f64);
    let after = rec.max((uops - saved) / 3.3);
    after <= 0.65 * before
}

pub fn run(f: &mut Func) -> bool {
    let dom = Dom::new(f);
    let mut did = false;
    // Block ids stay valid while blocks are only added.
    let found: Vec<(BlockId, BlockId)> = loops::find(f, &dom)
        .into_iter()
        .filter(|l| l.size == 2 && l.header.0 != 0 && !f.blocks[l.header.0 as usize].tail)
        .filter_map(|l| {
            let body = (0..l.body.len())
                .find(|b| l.body[*b] && *b as u32 != l.header.0)
                .map(|b| BlockId(b as u32))?;
            Some((l.header, body))
        })
        .collect();
    for (h, b) in found {
        did |= unroll(f, h, b);
    }
    did
}

fn unroll(f: &mut Func, h: BlockId, b: BlockId) -> bool {
    let hb = f.blocks[h.0 as usize].clone();
    let bb = f.blocks[b.0 as usize].clone();
    // The header: `c = lt i, n` and a branch into the body.
    let [ci] = hb.insts.as_slice() else {
        return false;
    };
    let Some(c) = f.insts[*ci].result else {
        return false;
    };
    let Op::Cmp(CmpOp::Lt, i, n) = f.insts[*ci].op else {
        return false;
    };
    let Some(Term::Branch(cond, yes, no)) = &hb.term else {
        return false;
    };
    if *cond != c || yes.block != b || !yes.args.is_empty() || no.block == b || no.block == h {
        return false;
    }
    let (yes, no) = (yes.clone(), no.clone());
    let Some(ix) = hb.params.iter().position(|p| *p == i) else {
        return false;
    };
    if f.ty(i) != IrTy::Int(64) || f.ty(n) != IrTy::Int(64) || !bb.params.is_empty() {
        return false;
    }
    let Some(Term::Jump(back)) = &bb.term else {
        return false;
    };
    if back.block != h {
        return false;
    }
    let back = back.clone();
    // The counter: sent back as itself plus a positive constant.
    let defs = f.defs();
    let konst = |v: Value| match defs[v.0 as usize].map(|k| &f.insts[k].op) {
        Some(Op::Const(IrTy::Int(64), Const::Int(c))) => Some(*c),
        _ => None,
    };
    let step = match defs[back.args[ix].0 as usize].map(|k| &f.insts[k].op) {
        Some(Op::Bin(BinOp::Add, a, c)) if *a == i => konst(*c),
        Some(Op::Bin(BinOp::Add, c, a)) if *a == i => konst(*c),
        _ => None,
    };
    let Some(step) = step.filter(|s| (1..=1 << 20).contains(s)) else {
        return false;
    };
    // The bound is not made in the loop, and the body is small and plain.
    let mut inside: HashSet<Value> = hb.params.iter().copied().collect();
    inside.insert(c);
    for &k in &bb.insts {
        if let Some(r) = f.insts[k].result {
            inside.insert(r);
        }
        if matches!(
            f.insts[k].op,
            Op::Call(..)
                | Op::CallPtr(..)
                | Op::Alloc(_)
                | Op::Realloc(..)
                | Op::Free(_)
                | Op::Slot(..)
        ) {
            return false;
        }
    }
    if inside.contains(&n) || bb.insts.is_empty() {
        return false;
    }
    // Only a loop known to run long pays for the copies in code size: the bound is a constant.
    if !konst(n).is_some_and(|c| c >= MIN_TRIPS * step) {
        return false;
    }
    if !pays(f, &hb.params, &bb, &back, k_for(bb.insts.len())) {
        return false;
    }
    let k = k_for(bb.insts.len());
    let span = (k as i128 - 1) * step;

    // The new blocks: a guard in front of the loop, the main loop's header and body, an exit.
    let tys: Vec<IrTy> = hb.params.iter().map(|p| f.ty(*p)).collect();
    let pre = f.new_block(&tys);
    let h2 = f.new_block(&tys);
    let b2 = f.new_block(&[]);
    let exit = f.new_block(&tys);
    let i64t = IrTy::Int(64);
    let mk = |f: &mut Func, blk: BlockId, v: i128| {
        f.emit(blk, Op::Const(i64t, Const::Int(v)), Some(i64t))
            .expect("a constant makes a value")
    };
    let least = mk(f, pre, i64::MIN as i128 + span);
    let ok = f
        .emit(pre, Op::Cmp(CmpOp::Ge, n, least), Some(IrTy::Bool))
        .expect("a comparison makes a value");
    let by = mk(f, pre, span);
    let shifted = f
        .emit(pre, Op::Bin(BinOp::Sub, n, by), Some(i64t))
        .expect("a difference makes a value");
    let min = mk(f, pre, i64::MIN as i128);
    let lim = f
        .emit(pre, Op::Select(ok, shifted, min), Some(i64t))
        .expect("a select makes a value");
    let pre_args = f.blocks[pre.0 as usize].params.clone();
    f.terminate(
        pre,
        Term::Jump(Target {
            block: h2,
            args: pre_args,
        }),
    );
    // Everything that entered the loop from outside goes through the guard.
    let preds = f.predecessors();
    for p in preds[h.0 as usize].clone() {
        if p == b {
            continue;
        }
        if let Some(t) = &mut f.blocks[p.0 as usize].term {
            for tg in t.targets_mut() {
                if tg.block == h {
                    tg.block = pre;
                }
            }
        }
    }
    let h2_params = f.blocks[h2.0 as usize].params.clone();
    let test = f
        .emit(h2, Op::Cmp(CmpOp::Lt, h2_params[ix], lim), Some(IrTy::Bool))
        .expect("a comparison makes a value");
    f.terminate(
        h2,
        Term::Branch(
            test,
            Target {
                block: b2,
                args: vec![],
            },
            Target {
                block: exit,
                args: h2_params.clone(),
            },
        ),
    );
    // The exit hands the main loop's values to the original loop.
    let exit_params = f.blocks[exit.0 as usize].params.clone();
    f.terminate(
        exit,
        Term::Jump(Target {
            block: h,
            args: exit_params,
        }),
    );
    // `k` copies of the body, each reading what the one before sent back.
    let mut cur: Vec<Value> = h2_params;
    for _ in 0..k {
        let mut map: HashMap<Value, Value> =
            hb.params.iter().copied().zip(cur.iter().copied()).collect();
        map.insert(c, test);
        for &ii in &bb.insts {
            let inst = f.insts[ii].clone();
            let mut op = inst.op;
            op.map_values(|v| *map.get(&v).unwrap_or(&v));
            let ty = inst.result.map(|r| f.ty(r));
            let r = f.emit_at(b2, op, ty, inst.span);
            if let (Some(old), Some(new)) = (inst.result, r) {
                map.insert(old, new);
            }
        }
        cur = back.args.iter().map(|v| *map.get(v).unwrap_or(v)).collect();
    }
    f.terminate(
        b2,
        Term::Jump(Target {
            block: h2,
            args: cur,
        }),
    );
    let _ = (yes, no);
    true
}
