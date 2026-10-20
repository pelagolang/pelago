//! Which functions are pure: no output, no heap, no store outside their own frame, calls only to
//! pure functions. A pure function can be run again from the start with the same arguments and
//! gives the same result (or stops at the same place), which is what lets an optimised form fall
//! back to the unoptimised one when it cannot promise the exact same stop (see `tailrec`).

use crate::ir::*;
use std::collections::HashSet;

/// One flag per function of the module.
pub fn pure_funcs(m: &Module) -> Vec<bool> {
    let mut pure = vec![true; m.funcs.len()];
    loop {
        let mut changed = false;
        for (i, f) in m.funcs.iter().enumerate() {
            if pure[i] && !is_pure(f, &pure) {
                pure[i] = false;
                changed = true;
            }
        }
        if !changed {
            return pure;
        }
    }
}

fn is_pure(f: &Func, pure: &[bool]) -> bool {
    // Addresses that point into this frame.
    let mut own: HashSet<Value> = HashSet::new();
    loop {
        let before = own.len();
        for b in &f.blocks {
            for &i in &b.insts {
                match (&f.insts[i].op, f.insts[i].result) {
                    (Op::Slot(..), Some(r)) => {
                        own.insert(r);
                    }
                    (Op::PtrAdd(a, _), Some(r)) if own.contains(a) => {
                        own.insert(r);
                    }
                    _ => {}
                }
            }
        }
        if own.len() == before {
            break;
        }
    }
    f.blocks.iter().all(|b| {
        b.insts.iter().all(|&i| match &f.insts[i].op {
            Op::Print(..) | Op::Alloc(_) | Op::Free(_) | Op::CallPtr(..) => false,
            Op::Store(p, _) => own.contains(p),
            Op::MemCopy(d, _, _) => own.contains(d),
            Op::Call(g, _) => pure[g.0 as usize],
            _ => true,
        })
    })
}
