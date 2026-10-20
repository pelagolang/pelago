//! Slots that a loop fills completely before anything else touches them (card A-11).
//!
//! A `Slot` is zero when it comes into use. For a big array written by `[x; n]` that is a second pass
//! over the memory for nothing: the very next loop stores every element. The native code generator
//! asks [`filled_slots`] which slots it may leave uncleared. The shape that qualifies is the loop the
//! front end writes for a repeated array, and nothing looser:
//!
//! ```text
//! H(i):  c = lt i, N ; branch c, B, X          (i starts at the constant 0 on the only other edge)
//! B:     p = ptradd slot, i * w ; store p, v ; jump H(i + 1)
//! ```
//!
//! with `w` the size of the stored value and `N * w` the size of the slot, so every byte is written.
//! Every other use of the slot, or of an address made from it, must lie in a block that `X` dominates,
//! that is, after the loop has run to its end. `X` has no other way in, so the loop cannot be skipped.

use super::dom::Dom;
use crate::ir::*;
use std::collections::HashSet;

/// Clearing a small slot is a few moves; only big ones are worth looking at.
const BIG: u32 = 4096;

/// The `Slot` instructions (by instruction number) whose clearing is not needed.
pub fn filled_slots(f: &Func) -> HashSet<usize> {
    let mut out = HashSet::new();
    let mut slots = Vec::new();
    for b in &f.blocks {
        for &i in &b.insts {
            if let (Op::Slot(size, _), Some(r)) = (&f.insts[i].op, f.insts[i].result) {
                if *size >= BIG {
                    slots.push((i, r, *size as i128));
                }
            }
        }
    }
    if slots.is_empty() {
        return out;
    }
    let dom = Dom::new(f);
    let preds = f.predecessors();
    let defs = f.defs();
    for (i, s, size) in slots {
        if fills(f, &dom, &preds, &defs, s, size) {
            out.insert(i);
        }
    }
    out
}

fn int_of(f: &Func, defs: &[Option<usize>], v: Value) -> Option<i128> {
    match f.insts[defs[v.0 as usize]?].op {
        Op::Const(_, Const::Int(n)) => Some(n),
        _ => None,
    }
}

fn bytes(t: IrTy) -> i128 {
    match t {
        IrTy::Int(b) => (b / 8).max(1) as i128,
        IrTy::Bool => 1,
        IrTy::F32 => 4,
        _ => 8,
    }
}

fn fills(
    f: &Func,
    dom: &Dom,
    preds: &[Vec<BlockId>],
    defs: &[Option<usize>],
    s: Value,
    size: i128,
) -> bool {
    // The addresses made from the slot.
    let mut derived: HashSet<Value> = HashSet::from([s]);
    loop {
        let before = derived.len();
        for inst in &f.insts {
            if let (Op::PtrAdd(a, _), Some(r)) = (&inst.op, inst.result) {
                if derived.contains(a) {
                    derived.insert(r);
                }
            }
        }
        if derived.len() == before {
            break;
        }
    }
    for b in 0..f.blocks.len() {
        if !dom.reachable(BlockId(b as u32)) {
            continue;
        }
        if let Some(x) = fill_loop(f, preds, defs, &derived, s, size, BlockId(b as u32)) {
            let (exit, skip) = x;
            return after(f, dom, &derived, exit, &skip);
        }
    }
    false
}

/// Is `b` the body of a loop that fills the slot? Gives the exit block and the instructions that make
/// up the filling (they are the one allowed use of the slot before the exit).
fn fill_loop(
    f: &Func,
    preds: &[Vec<BlockId>],
    defs: &[Option<usize>],
    derived: &HashSet<Value>,
    s: Value,
    size: i128,
    body: BlockId,
) -> Option<(BlockId, Vec<usize>)> {
    let b = &f.blocks[body.0 as usize];
    let Some(Term::Jump(back)) = &b.term else {
        return None;
    };
    let head = back.block;
    let [next] = back.args[..] else {
        return None;
    };
    let h = &f.blocks[head.0 as usize];
    let [i] = h.params[..] else {
        return None;
    };
    let [cmp] = h.insts[..] else {
        return None;
    };
    let Op::Cmp(CmpOp::Lt, ci, n) = f.insts[cmp].op else {
        return None;
    };
    let Some(Term::Branch(c, yes, no)) = &h.term else {
        return None;
    };
    if ci != i
        || f.insts[cmp].result != Some(*c)
        || yes.block != body
        || !yes.args.is_empty()
        || !no.args.is_empty()
    {
        return None;
    }
    let exit = no.block;
    let count = int_of(f, defs, n)?;
    // The header has the loop's edge and one more, which starts the counter at zero.
    let hp = &preds[head.0 as usize];
    if hp.len() != 2
        || !hp.contains(&body)
        || preds[body.0 as usize] != [head]
        || preds[exit.0 as usize] != [head]
    {
        return None;
    }
    let start = *hp.iter().find(|p| **p != body)?;
    let targets: Vec<&Target> = match &f.blocks[start.0 as usize].term {
        Some(Term::Jump(t)) => vec![t],
        Some(Term::Branch(_, a, c)) => vec![a, c],
        _ => Vec::new(),
    };
    let from_zero = targets
        .iter()
        .filter(|t| t.block == head)
        .all(|t| matches!(t.args[..], [a] if int_of(f, defs, a) == Some(0)));
    if !from_zero {
        return None;
    }
    // The body: an offset, an address, one store, the next counter.
    let (mut stride, mut width) = (None, None);
    let (mut off, mut addr) = (None, None);
    let mut skip: Vec<usize> = Vec::new();
    let (mut stored, mut counted) = (false, false);
    for &k in &b.insts {
        let inst = &f.insts[k];
        match (&inst.op, inst.result) {
            (Op::Const(..), _) => {}
            (Op::Bin(BinOp::Mul, a, m), Some(r)) if *a == i && off.is_none() => {
                stride = Some(int_of(f, defs, *m)?);
                off = Some(r);
            }
            (Op::PtrAdd(base, o), Some(r)) if *base == s && Some(*o) == off && addr.is_none() => {
                addr = Some(r);
                skip.push(k);
            }
            (Op::Store(p, v), None) if Some(*p) == addr && !derived.contains(v) && !stored => {
                width = Some(bytes(f.ty(*v)));
                stored = true;
                skip.push(k);
            }
            (Op::Bin(BinOp::Add, a, one), Some(r))
                if *a == i && r == next && !counted && int_of(f, defs, *one) == Some(1) =>
            {
                counted = true;
            }
            _ => return None,
        }
    }
    if !stored || !counted {
        return None;
    }
    let (stride, width) = (stride?, width?);
    if stride != width || count <= 0 || count * width != size {
        return None;
    }
    Some((exit, skip))
}

/// Does every other use of the slot lie after the loop?
fn after(f: &Func, dom: &Dom, derived: &HashSet<Value>, exit: BlockId, skip: &[usize]) -> bool {
    for (bi, b) in f.blocks.iter().enumerate() {
        let late = !dom.reachable(BlockId(bi as u32)) || dom.dominates(exit, BlockId(bi as u32));
        for &k in &b.insts {
            if skip.contains(&k) {
                continue;
            }
            let mut touches = false;
            f.insts[k]
                .op
                .for_each_value(|v| touches |= derived.contains(&v));
            if touches && !late {
                return false;
            }
        }
        if let Some(t) = &b.term {
            let mut touches = false;
            t.for_each_value(|v| touches |= derived.contains(&v));
            if touches && !late {
                return false;
            }
        }
    }
    true
}
