//! Masks that no one needs (card A-11). A backward analysis finds, for every integer value, the bits
//! that some later use can tell apart: `x & 1023` reads ten bits of `x`, a sum reads the bits of its
//! operands up to the highest bit it is asked for, a comparison, a store, a call or a checked
//! operation reads them all. A block parameter needs what its users need, and so do the values the
//! jumps hand to it, which is what carries the answer around a loop.
//!
//! Then `x & c` whose result is only read inside `c` is `x`: the bits it would clear are never
//! looked at. In `p = (p + 3) & 65535` followed by `(3 * p + q) & 1023` the first mask goes, and the
//! counter becomes a plain `p + 3` that may carry garbage above bit 10 that nothing reads. Only
//! wrapping operations pass demand on bit by bit; checked ones (`add_checked`, ...) demand every
//! bit, so a trap still happens exactly when it did.

use crate::ir::*;
use std::collections::HashMap;

fn full(f: &Func, v: Value) -> u64 {
    match f.values[v.0 as usize] {
        IrTy::Int(64) => u64::MAX,
        IrTy::Int(b) => (1u64 << b) - 1,
        _ => u64::MAX,
    }
}

/// The bits up to and including the highest one in `d`: what a sum needs of its operands.
fn low(d: u64) -> u64 {
    if d == 0 {
        0
    } else {
        u64::MAX >> d.leading_zeros()
    }
}

pub fn run(f: &mut Func) -> bool {
    let n = f.values.len();
    let mut consts: HashMap<Value, u64> = HashMap::new();
    for b in &f.blocks {
        for &i in &b.insts {
            let inst = &f.insts[i];
            if let (Some(r), Op::Const(IrTy::Int(_), Const::Int(c))) = (inst.result, &inst.op) {
                consts.insert(r, *c as u64 & full(f, r));
            }
        }
    }
    if !f
        .insts
        .iter()
        .any(|i| matches!(i.op, Op::Bin(BinOp::And, ..)) && i.result.is_some())
    {
        return false;
    }
    let mut need = vec![0u64; n];
    loop {
        let mut changed = false;
        let mut add = |need: &mut Vec<u64>, v: Value, m: u64| {
            let slot = &mut need[v.0 as usize];
            if *slot | m != *slot {
                *slot |= m;
                changed = true;
            }
        };
        for b in &f.blocks {
            for &i in &b.insts {
                let inst = &f.insts[i];
                let d = inst.result.map_or(0, |r| need[r.0 as usize]);
                let known = |v: &Value| consts.get(v).copied();
                match (&inst.op, inst.result) {
                    (Op::Bin(BinOp::And, a, c), Some(_)) => {
                        let (ma, mc) = match (known(a), known(c)) {
                            (_, Some(k)) => (d & k, d),
                            (Some(k), _) => (d, d & k),
                            _ => (d, d),
                        };
                        add(&mut need, *a, ma);
                        add(&mut need, *c, mc);
                    }
                    (Op::Bin(BinOp::Or | BinOp::Xor, a, c), Some(_)) => {
                        add(&mut need, *a, d);
                        add(&mut need, *c, d);
                    }
                    (Op::Bin(BinOp::Add | BinOp::Sub | BinOp::Mul, a, c), Some(_)) => {
                        add(&mut need, *a, low(d) & full(f, *a));
                        add(&mut need, *c, low(d) & full(f, *c));
                    }
                    (Op::Neg(a), Some(_)) => add(&mut need, *a, low(d) & full(f, *a)),
                    (Op::Bin(BinOp::Shl, a, c), Some(_)) if known(c).is_some_and(|k| k < 64) => {
                        add(&mut need, *a, d >> known(c).unwrap_or(0));
                    }
                    (Op::Select(c, a, b2), Some(_)) => {
                        add(&mut need, *c, full(f, *c));
                        add(&mut need, *a, d);
                        add(&mut need, *b2, d);
                    }
                    (op, _) => op.for_each_value(|v| add(&mut need, v, full(f, v))),
                }
            }
            let Some(term) = &b.term else { continue };
            match term {
                Term::Jump(t) => pass_args(f, t, &mut need, &mut add),
                Term::Branch(c, t, e) => {
                    add(&mut need, *c, full(f, *c));
                    pass_args(f, t, &mut need, &mut add);
                    pass_args(f, e, &mut need, &mut add);
                }
                Term::Return(Some(v)) => add(&mut need, *v, full(f, *v)),
                _ => {}
            }
        }
        if !changed {
            break;
        }
    }
    let mut rep: HashMap<Value, Value> = HashMap::new();
    for b in &f.blocks {
        for &i in &b.insts {
            let inst = &f.insts[i];
            let (Some(r), Op::Bin(BinOp::And, a, c)) = (inst.result, &inst.op) else {
                continue;
            };
            let d = need[r.0 as usize];
            let keep = match (consts.get(a), consts.get(c)) {
                (_, Some(k)) => Some((*a, *k)),
                (Some(k), _) => Some((*c, *k)),
                _ => None,
            };
            if let Some((x, k)) = keep {
                if d != 0 && d & !k == 0 && f.values[x.0 as usize] == f.values[r.0 as usize] {
                    rep.insert(r, x);
                }
            }
        }
    }
    if rep.is_empty() {
        return false;
    }
    f.replace_uses(&rep);
    true
}

fn pass_args(
    f: &Func,
    t: &Target,
    need: &mut Vec<u64>,
    add: &mut impl FnMut(&mut Vec<u64>, Value, u64),
) {
    let params = &f.blocks[t.block.0 as usize].params;
    for (p, a) in params.iter().zip(&t.args) {
        let m = need[p.0 as usize];
        add(need, *a, m);
    }
}
