//! Slots that only calls can see (card A-11): a small slot whose address is handed to calls, but
//! never kept by them, is read and written by the function itself through a private copy, which
//! the other passes then turn into registers. Around every call that gets the slot, the copy is
//! written back first and read again afterwards:
//!
//! ```text
//! xs = slot 24            ->   xs = slot 24, xs' = slot 24
//! store xs+8, len+1             store xs'+8, len+1            (a register after promotion)
//! call grow(xs, ..)             memcopy xs, xs', 24; call grow(xs, ..); memcopy xs', xs, 24
//! ```
//!
//! This is what keeps the length and capacity of a list being filled in registers while the rare
//! call that grows it sees the real header. It needs to know that a callee does not keep the address
//! it is given (a pointer stored somewhere, returned, or passed on to something that keeps it), so
//! the first step is an escape analysis over the module's functions.

use crate::ir::*;
use std::collections::{HashMap, HashSet};

/// The biggest slot treated this way: the copies around each call cost a load and a store per piece.
const MAX_SLOT: u32 = 32;

pub fn run(m: &mut Module) -> bool {
    let escapes = escape_analysis(m);
    let mut changed = false;
    for fi in 0..m.funcs.len() {
        changed |= cache_slots(&mut m.funcs[fi], &escapes);
    }
    changed
}

/// For each function and each parameter: may the callee keep the address it is given?
fn escape_analysis(m: &Module) -> Vec<Vec<bool>> {
    let mut esc: Vec<Vec<bool>> = m
        .funcs
        .iter()
        .map(|f| vec![false; f.params.len()])
        .collect();
    loop {
        let mut changed = false;
        for (fi, f) in m.funcs.iter().enumerate() {
            let params = f
                .blocks
                .first()
                .map(|b| b.params.clone())
                .unwrap_or_default();
            for (k, p) in params.iter().enumerate() {
                if esc[fi][k] || f.ty(*p) != IrTy::Ptr {
                    continue;
                }
                if param_escapes(f, *p, &esc) {
                    esc[fi][k] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return esc;
        }
    }
}

/// Is the address `p` (or an address made from it) used for anything but reading and writing memory?
fn param_escapes(f: &Func, p: Value, esc: &[Vec<bool>]) -> bool {
    let mut derived: HashSet<Value> = HashSet::from([p]);
    loop {
        let before = derived.len();
        for b in &f.blocks {
            for &i in &b.insts {
                if let (Op::PtrAdd(a, _), Some(r)) = (&f.insts[i].op, f.insts[i].result) {
                    if derived.contains(a) {
                        derived.insert(r);
                    }
                }
            }
        }
        if derived.len() == before {
            break;
        }
    }
    for b in &f.blocks {
        for &i in &b.insts {
            match &f.insts[i].op {
                Op::Load(..) | Op::MemCopy(..) | Op::Print(..) => {}
                Op::Store(_, x) if derived.contains(x) => return true,
                Op::Store(..) => {}
                Op::PtrAdd(a, c) => {
                    // The offset must not be the address (pointer arithmetic on addresses).
                    if derived.contains(c) && !derived.contains(a) {
                        return true;
                    }
                }
                Op::Call(g, args) => {
                    for (k, a) in args.iter().enumerate() {
                        if derived.contains(a) && esc[g.0 as usize].get(k).copied().unwrap_or(true)
                        {
                            return true;
                        }
                    }
                }
                op => {
                    let mut hit = false;
                    op.for_each_value(|v| hit |= derived.contains(&v));
                    if hit {
                        return true;
                    }
                }
            }
        }
        if let Some(t) = &b.term {
            let mut hit = false;
            t.for_each_value(|v| hit |= derived.contains(&v));
            if hit {
                return true;
            }
        }
    }
    false
}

fn cache_slots(f: &mut Func, escapes: &[Vec<bool>]) -> bool {
    let mut consts: HashMap<Value, i128> = HashMap::new();
    let mut slots: Vec<(Value, u32, u32)> = Vec::new();
    for b in &f.blocks {
        for &i in &b.insts {
            match (&f.insts[i].op, f.insts[i].result) {
                (Op::Const(_, Const::Int(n)), Some(r)) => {
                    consts.insert(r, *n);
                }
                (Op::Slot(size, align), Some(r)) if *size <= MAX_SLOT => {
                    slots.push((r, *size, *align));
                }
                _ => {}
            }
        }
    }
    let mut changed = false;
    for (s, size, align) in slots {
        if let Some(plan) = plan_for(f, s, &consts, escapes) {
            apply(f, s, size, align, &plan);
            changed = true;
        }
    }
    changed
}

/// The instructions of a slot that can be cached: its calls, and whether the function itself touches it.
struct Plan {
    /// (block, position-independent instruction id) of each call that gets the slot.
    calls: Vec<usize>,
}

fn plan_for(
    f: &Func,
    s: Value,
    consts: &HashMap<Value, i128>,
    escapes: &[Vec<bool>],
) -> Option<Plan> {
    let mut derived: HashSet<Value> = HashSet::from([s]);
    loop {
        let before = derived.len();
        for b in &f.blocks {
            for &i in &b.insts {
                if let (Op::PtrAdd(a, c), Some(r)) = (&f.insts[i].op, f.insts[i].result) {
                    if derived.contains(a) && consts.contains_key(c) {
                        derived.insert(r);
                    }
                }
            }
        }
        if derived.len() == before {
            break;
        }
    }
    let mut calls = Vec::new();
    let mut direct = 0usize;
    for b in &f.blocks {
        for &i in &b.insts {
            let op = &f.insts[i].op;
            match op {
                Op::Load(_, p) if derived.contains(p) => direct += 1,
                Op::Store(p, x) => {
                    if derived.contains(x) {
                        return None;
                    }
                    if derived.contains(p) {
                        direct += 1;
                    }
                }
                Op::MemCopy(d, src, _) => {
                    if derived.contains(d) || derived.contains(src) {
                        direct += 1;
                    }
                }
                Op::Print(p, _) if derived.contains(p) => direct += 1,
                Op::PtrAdd(a, c) => {
                    if derived.contains(c) {
                        return None;
                    }
                    if derived.contains(a)
                        && f.insts[i].result.is_some_and(|r| !derived.contains(&r))
                    {
                        return None;
                    }
                }
                Op::Call(g, args) => {
                    let mut gets = false;
                    for (k, a) in args.iter().enumerate() {
                        if derived.contains(a) {
                            if *a != s || escapes[g.0 as usize].get(k).copied().unwrap_or(true) {
                                return None;
                            }
                            gets = true;
                        }
                    }
                    if gets {
                        calls.push(i);
                    }
                }
                Op::Load(..) => {}
                op => {
                    let mut hit = false;
                    op.for_each_value(|v| hit |= derived.contains(&v));
                    if hit {
                        return None;
                    }
                }
            }
        }
        if let Some(t) = &b.term {
            let mut hit = false;
            t.for_each_value(|v| hit |= derived.contains(&v));
            if hit {
                return None;
            }
        }
    }
    (!calls.is_empty() && direct > 0).then_some(Plan { calls })
}

fn apply(f: &mut Func, s: Value, size: u32, align: u32, plan: &Plan) {
    // The private copy, made at the start of the entry block.
    let s2 = f.new_value(IrTy::Ptr);
    f.insts.push(Inst {
        result: Some(s2),
        op: Op::Slot(size, align),
        span: crate::span::Span::new(0, 0),
    });
    let slot_inst = f.insts.len() - 1;
    f.blocks[0].insts.insert(0, slot_inst);
    let calls: HashSet<usize> = plan.calls.iter().copied().collect();
    // Everything but the calls reads and writes the copy.
    for i in 0..f.insts.len() {
        if calls.contains(&i) || i == slot_inst {
            continue;
        }
        f.insts[i].op.map_values(|v| if v == s { s2 } else { v });
    }
    let n = f.new_value(IrTy::I64);
    f.insts.push(Inst {
        result: Some(n),
        op: Op::Const(IrTy::I64, Const::Int(size as i128)),
        span: crate::span::Span::new(0, 0),
    });
    let n_inst = f.insts.len() - 1;
    f.blocks[0].insts.insert(1, n_inst);
    for bi in 0..f.blocks.len() {
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let mut out = Vec::with_capacity(old.len());
        for i in old {
            if calls.contains(&i) {
                let span = f.insts[i].span;
                f.insts.push(Inst {
                    result: None,
                    op: Op::MemCopy(s, s2, n),
                    span,
                });
                out.push(f.insts.len() - 1);
                out.push(i);
                f.insts.push(Inst {
                    result: None,
                    op: Op::MemCopy(s2, s, n),
                    span,
                });
                out.push(f.insts.len() - 1);
            } else {
                out.push(i);
            }
        }
        f.blocks[bi].insts = out;
    }
}
