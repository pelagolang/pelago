//! Frozen loads (docs/design/mir.md 8.1). A `let` parameter passed in memory is not written for the
//! whole call: the checker rejects `inout` access to a value that is borrowed. So everything reachable
//! from it is constant while the function runs: the header words, the data pointer read from a
//! header, the elements read through that pointer. This pass finds the loads that read such memory
//! (their address comes from a `let` pointer through `ptradd` and through the results of other frozen
//! pointer loads) and records them in `Func::frozen`.
//!
//! The fact is true for the function's own body only. The inliner makes new instruction numbers for
//! the copy it puts into a caller, so an inlined load is not marked: after inlining, a loop around the
//! call could otherwise treat the value as constant across two calls, between which the caller may
//! change it. No pass uses the marks yet (the Optimise phase adds that: hoisting and joining such
//! loads past stores and calls); this pass only records them.

use super::fx::HashSet;
use crate::ir::*;

pub fn mark(f: &mut Func) {
    f.frozen.clear();
    if f.let_ptrs.is_empty() {
        return;
    }
    // Values that point into frozen memory.
    let mut ptrs: HashSet<Value> = f.let_ptrs.iter().copied().collect();
    loop {
        let mut grew = false;
        for b in &f.blocks {
            for &i in &b.insts {
                let inst = &f.insts[i];
                let Some(r) = inst.result else { continue };
                match &inst.op {
                    Op::PtrAdd(a, _) if ptrs.contains(a) => grew |= ptrs.insert(r),
                    Op::Load(IrTy::Ptr, a) if ptrs.contains(a) => grew |= ptrs.insert(r),
                    _ => {}
                }
            }
        }
        if !grew {
            break;
        }
    }
    for b in &f.blocks {
        for &i in &b.insts {
            if let Op::Load(_, a) = &f.insts[i].op {
                if ptrs.contains(a) {
                    f.frozen.insert(i);
                }
            }
        }
    }
}
