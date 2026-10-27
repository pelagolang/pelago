//! Loop-invariant code motion (card A-11, W9). An instruction in a loop whose operands are all
//! defined outside the loop (or already moved out) gives the same result on every iteration, so it
//! is computed once in front of the loop.
//!
//! Only instructions that cannot stop and cannot change anything are moved, because the loop may run
//! zero times and then the instruction would run where it did not before: arithmetic without
//! division, comparisons, casts, address arithmetic. A load moves too when the loop writes no
//! memory and the load cannot fault early: it sits in the header (which runs whenever the loop is
//! entered) or reads a slot of this function.
//!
//! The "front of the loop" is the header's single outside predecessor if it just jumps to the
//! header; otherwise a new block is put there that all outside edges go through.

use super::dom::Dom;
use super::loops::{self, Loop};
use crate::ir::*;

pub fn run(f: &mut Func) -> bool {
    let mut changed = false;
    // Each round moves something out of one loop and looks again, since the new preheader changes
    // the shape of the enclosing loop.
    for _ in 0..64 {
        if !once(f) {
            break;
        }
        changed = true;
    }
    changed
}

fn once(f: &mut Func) -> bool {
    let dom = Dom::new(f);
    for lp in loops::find(f, &dom) {
        if lp.header.0 == 0 {
            continue;
        }
        let moves = candidates(f, &dom, &lp);
        if moves.is_empty() {
            continue;
        }
        let pre = preheader(f, &lp);
        for ii in &moves {
            for b in &mut f.blocks {
                b.insts.retain(|x| x != ii);
            }
        }
        f.blocks[pre.0 as usize].insts.extend(moves);
        return true;
    }
    false
}

fn candidates(f: &Func, dom: &Dom, lp: &Loop) -> Vec<usize> {
    // The block each value is made in.
    let mut home = vec![u32::MAX; f.values.len()];
    let mut writes = false;
    for (bi, b) in f.blocks.iter().enumerate() {
        for p in &b.params {
            home[p.0 as usize] = bi as u32;
        }
        for &ii in &b.insts {
            if let Some(r) = f.insts[ii].result {
                home[r.0 as usize] = bi as u32;
            }
            if lp.contains(BlockId(bi as u32)) {
                writes |= matches!(
                    f.insts[ii].op,
                    Op::Store(..)
                        | Op::Call(..)
                        | Op::CallPtr(..)
                        | Op::MemCopy(..)
                        | Op::Alloc(_)
                        | Op::Realloc(..)
                        | Op::Free(_)
                        | Op::CtxInit(..)
                        | Op::CtxSwitch(..)
                );
            }
        }
    }
    let defs = f.defs();
    let mut moved = vec![false; f.values.len()];
    let outside = |v: Value, moved: &[bool]| {
        let h = home[v.0 as usize];
        h == u32::MAX || !lp.body[h as usize] || moved[v.0 as usize]
    };
    let mut out = Vec::new();
    for &b in &dom.rpo {
        if !lp.contains(b) {
            continue;
        }
        for &ii in &f.blocks[b.0 as usize].insts {
            let inst = &f.insts[ii];
            let Some(r) = inst.result else { continue };
            let ok = match &inst.op {
                Op::Const(..)
                | Op::Cmp(..)
                | Op::Select(..)
                | Op::Neg(_)
                | Op::Not(_)
                | Op::Cast(..)
                | Op::PtrAdd(..)
                | Op::DataAddr(_)
                | Op::FuncAddr(_) => true,
                Op::Bin(o, ..) => !matches!(o, BinOp::Div | BinOp::Rem | BinOp::UDiv | BinOp::URem),
                Op::Load(_, a) => !writes && (b == lp.header || is_slot_address(f, &defs, *a)),
                _ => false,
            };
            let mut all = ok;
            if ok {
                inst.op.for_each_value(|v| all &= outside(v, &moved));
            }
            if all {
                moved[r.0 as usize] = true;
                out.push(ii);
            }
        }
    }
    // An integer constant is only worth moving for the instructions that need it (otherwise it is
    // an immediate). A float constant costs a move through a general register and is worth a
    // register of its own for the whole loop.
    let mut needed = vec![false; f.values.len()];
    for &ii in &out {
        if !matches!(f.insts[ii].op, Op::Const(..)) {
            f.insts[ii]
                .op
                .for_each_value(|v| needed[v.0 as usize] = true);
        }
    }
    out.retain(|&ii| match (&f.insts[ii].op, f.insts[ii].result) {
        // (A zero stays in the loop unless something that moves out reads it.)
        (Op::Const(IrTy::F32 | IrTy::F64, Const::Float(x)), Some(r)) => {
            x.to_bits() != 0 || needed[r.0 as usize]
        }
        (Op::Const(..), Some(r)) => needed[r.0 as usize],
        _ => true,
    });
    out
}

/// Is the address a slot of this function plus constants? Such a read cannot fault.
fn is_slot_address(f: &Func, defs: &[Option<usize>], v: Value) -> bool {
    let Some(d) = defs[v.0 as usize] else {
        return false;
    };
    match &f.insts[d].op {
        Op::Slot(..) => true,
        Op::PtrAdd(a, k) => {
            is_slot_address(f, defs, *a)
                && defs[k.0 as usize].is_some_and(|c| matches!(f.insts[c].op, Op::Const(..)))
        }
        _ => false,
    }
}

/// The block that runs right before the loop is entered, made if there is none.
pub(super) fn preheader(f: &mut Func, lp: &Loop) -> BlockId {
    let h = lp.header;
    let preds = f.predecessors();
    let outside: Vec<BlockId> = preds[h.0 as usize]
        .iter()
        .copied()
        .filter(|p| !lp.contains(*p))
        .collect();
    if let [p] = outside.as_slice() {
        if matches!(f.blocks[p.0 as usize].term, Some(Term::Jump(_))) {
            return *p;
        }
    }
    let tys: Vec<IrTy> = f.blocks[h.0 as usize]
        .params
        .iter()
        .map(|p| f.ty(*p))
        .collect();
    let pre = f.new_block(&tys);
    let args = f.blocks[pre.0 as usize].params.clone();
    f.terminate(pre, Term::Jump(Target { block: h, args }));
    for p in outside {
        if let Some(t) = &mut f.blocks[p.0 as usize].term {
            for tg in t.targets_mut() {
                if tg.block == h {
                    tg.block = pre;
                }
            }
        }
    }
    pre
}
