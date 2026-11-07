//! What a call destroys, function by function (card A-10, `docs/design/os-memory.md` section 3).
//!
//! A call used to destroy every caller-saved register. Functions are now selected and allocated callee
//! first; the registers a function writes (its own, plus those its callees write) form its *write
//! set*, and a call of it destroys only those. A value that lives across the call stays in any
//! allocatable register outside the set, so an integer loop that calls a small leaf keeps its counter
//! and its pointers where they are, and floats stay in `xmm` registers across calls that do no float
//! work. A callee that is not known yet (a cycle of calls), a call through a pointer and a call of
//! the runtime keep the full set.
//!
//! The registers the emitter uses behind the allocator's back (the scratch registers, the page probe
//! of a big frame) are never allocatable, or are added by the target as `hidden`, so they need no
//! entry here. Callee-saved registers are saved and restored by every function that writes them, so
//! they are not in any set.

use super::mir::{MFunc, Operand, RegSet, Role, Target};
use crate::ir::{Module, Op};

/// The functions in the order they are compiled: callees before their callers. A call that closes a
/// cycle is skipped (its callee is compiled later and has no set yet).
pub fn order(m: &Module) -> Vec<usize> {
    let n = m.funcs.len();
    let calls: Vec<Vec<usize>> = m
        .funcs
        .iter()
        .map(|f| {
            f.blocks
                .iter()
                .flat_map(|b| &b.insts)
                .filter_map(|&i| match &f.insts[i].op {
                    Op::Call(c, _) => Some(c.0 as usize),
                    _ => None,
                })
                .collect()
        })
        .collect();
    let mut state = vec![0u8; n];
    let mut out = Vec::with_capacity(n);
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        state[start] = 1;
        let mut stack = vec![(start, 0usize)];
        while let Some(&(x, k)) = stack.last() {
            if let Some(&y) = calls[x].get(k) {
                stack.last_mut().unwrap().1 += 1;
                if state[y] == 0 {
                    state[y] = 1;
                    stack.push((y, 0));
                }
            } else {
                state[x] = 2;
                out.push(x);
                stack.pop();
            }
        }
    }
    out
}

/// Narrows the clobbers of each direct call in `f` to what its callee writes, where that is known.
/// `call_op` is the target's direct call, `sym_base` the number of the `Sym` of function 0.
pub fn narrow_calls(
    f: &mut MFunc,
    t: &dyn Target,
    call_op: u16,
    sym_base: u32,
    sets: &[Option<RegSet>],
) {
    let mut allocatable = RegSet::default();
    for c in [super::mir::Class::Int, super::mir::Class::Float] {
        for r in t.allocatable(c) {
            allocatable.insert(*r);
        }
    }
    for b in &mut f.blocks {
        for i in &mut b.insts {
            if i.op != call_op || !i.flags.has(super::mir::IFlags::CALL) {
                continue;
            }
            let Some(Operand::Sym(s)) = i.ops.first() else {
                continue;
            };
            let Some(Some(w)) = s.checked_sub(sym_base).and_then(|k| sets.get(k as usize)) else {
                continue;
            };
            // Keep every register the callee writes and every one the allocator never hands out.
            let mut keep = RegSet::default();
            for r in i.clobbers.iter() {
                if w.contains(r) || !allocatable.contains(r) {
                    keep.insert(r);
                }
            }
            i.clobbers = keep;
        }
    }
}

/// The registers an allocated function writes that its caller must treat as destroyed: every register
/// an instruction defines or clobbers, except the callee-saved ones (saved and restored), plus
/// `hidden` (registers the emitter writes without saying so).
pub fn written(f: &MFunc, t: &dyn Target, hidden: RegSet) -> RegSet {
    let mut w = hidden;
    for b in &f.blocks {
        for i in &b.insts {
            for r in i.clobbers.iter() {
                w.insert(r);
            }
            for o in &i.ops {
                if let Operand::Reg(super::mir::Reg::Phys(p), role) = o {
                    if *role != Role::Use {
                        w.insert(*p);
                    }
                }
            }
        }
    }
    let mut out = RegSet::default();
    for r in w.iter() {
        if !t.regs()[r.0 as usize].callee_saved {
            out.insert(r);
        }
    }
    out
}
