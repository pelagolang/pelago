//! A list element carried from one pass of a loop to the next (card A-11, few-register work).
//!
//! ```text
//! loop j:  x = cur[j]              ->   loop j, c:  (c is cur[j], carried)
//!          ...                              ...
//!          cur[j + 1] = y                   cur[j + 1] = y;  next pass gets c = y
//! ```
//!
//! The pass after the one that wrote `cur[j + 1]` reads that same element as `cur[j]`. The loop
//! has a single store and nothing else that writes memory, so the value read is the value
//! stored one pass ago (or, on the first pass, what was there before the loop): it travels in a
//! block parameter instead of through memory. That removes a load and, in a chain like the edit
//! distance, the wait for the store to be forwarded. It costs no register: the carried value
//! replaces the one the load made.
//!
//! The first value is loaded in front of the loop, but only on the way that enters the body (the
//! header's test is repeated there), so the load happens exactly when the body would have made it.
//!
//! The loop must be two blocks (a header that only tests and a body that jumps back); the store
//! and the loads must have the form `ptradd base, (i * K)` with the loads at the counter `p` and the
//! store at `p + 1`, which is the counter's next value.

use super::cse::writes_memory;
use super::dom::Dom;
use super::loops::{self, Loop};
use crate::ir::*;

pub fn run(f: &mut Func) -> bool {
    let dom = Dom::new(f);
    for lp in loops::find(f, &dom) {
        if try_loop(f, &lp) {
            return true;
        }
    }
    false
}

fn try_loop(f: &mut Func, lp: &Loop) -> bool {
    if lp.size != 2 || lp.header.0 == 0 {
        return false;
    }
    let h = lp.header;
    let Some(b) = (0..f.blocks.len())
        .map(|i| BlockId(i as u32))
        .find(|x| *x != h && lp.contains(*x))
    else {
        return false;
    };
    let defs = f.defs();
    let konst = |v: Value| match defs[v.0 as usize].map(|i| &f.insts[i].op) {
        Some(Op::Const(IrTy::Int(_), Const::Int(k))) => Some(*k),
        _ => None,
    };
    // The header only tests: pure instructions, then a branch into the body.
    let Some(Term::Branch(c, t, e)) = f.blocks[h.0 as usize].term.clone() else {
        return false;
    };
    let body_on_true = if t.block == b && e.block != b {
        true
    } else if e.block == b && t.block != b {
        false
    } else {
        return false;
    };
    if !t.args.is_empty() || !e.args.is_empty() {
        return false;
    }
    for &i in &f.blocks[h.0 as usize].insts {
        if !matches!(
            f.insts[i].op,
            Op::Cmp(..) | Op::Const(..) | Op::Cast(..) | Op::Bin(..)
        ) || matches!(
            f.insts[i].op,
            Op::Bin(BinOp::Div | BinOp::Rem | BinOp::UDiv | BinOp::URem, ..)
        ) {
            return false;
        }
    }
    // The body jumps back with the next counter.
    let Some(Term::Jump(back)) = f.blocks[b.0 as usize].term.clone() else {
        return false;
    };
    if back.block != h || back.args.len() != f.blocks[h.0 as usize].params.len() {
        return false;
    }
    // One store, and nothing else that writes.
    let mut store = None;
    for &i in &f.blocks[b.0 as usize].insts {
        match &f.insts[i].op {
            Op::Store(a, x) => {
                if store.is_some() {
                    return false;
                }
                store = Some((i, *a, *x));
            }
            op if writes_memory(op) => return false,
            _ => {}
        }
    }
    let Some((_, saddr, sval)) = store else {
        return false;
    };
    let ty = f.ty(sval);
    if !matches!(ty, IrTy::Int(_)) {
        return false;
    }
    let in_body = |v: Value| {
        f.blocks[b.0 as usize]
            .insts
            .iter()
            .any(|&i| f.insts[i].result == Some(v))
    };
    let params = f.blocks[h.0 as usize].params.clone();
    // saddr = ptradd base, (q * K), q = p + 1 and the next counter of `p` is `q`.
    let def_in_body = |v: Value| {
        f.blocks[b.0 as usize]
            .insts
            .iter()
            .find(|&&i| f.insts[i].result == Some(v))
            .map(|&i| f.insts[i].op.clone())
    };
    let Some(Op::PtrAdd(base, soff)) = def_in_body(saddr) else {
        return false;
    };
    if in_body(base) || params.contains(&base) {
        return false;
    }
    let Some(Op::Bin(BinOp::Mul, q, kv)) = def_in_body(soff) else {
        return false;
    };
    let Some(k) = konst(kv) else {
        return false;
    };
    let Some(Op::Bin(BinOp::Add, p, one)) = def_in_body(q) else {
        return false;
    };
    if konst(one) != Some(1) {
        return false;
    }
    let Some(pi) = params.iter().position(|x| *x == p) else {
        return false;
    };
    if back.args[pi] != q {
        return false;
    }
    // The loads at the counter.
    let mut loads: Vec<(usize, Value)> = Vec::new();
    for &i in &f.blocks[b.0 as usize].insts {
        let Op::Load(lty, a) = f.insts[i].op else {
            continue;
        };
        let Some(Op::PtrAdd(lb, loff)) = def_in_body(a) else {
            continue;
        };
        if lb != base || lty != ty {
            continue;
        }
        let Some(Op::Bin(BinOp::Mul, lp_, lk)) = def_in_body(loff) else {
            continue;
        };
        if lp_ == p && konst(lk) == Some(k) {
            if let Some(r) = f.insts[i].result {
                loads.push((i, r));
            }
        }
    }
    if loads.is_empty() {
        return false;
    }
    let span = f.insts[store.unwrap().0].span;
    let pre = super::licm::preheader(f, lp);
    // The carried value is a new parameter of the header.
    let cv = f.new_value(ty);
    f.blocks[h.0 as usize].params.push(cv);
    if let Some(Term::Jump(bk)) = &mut f.blocks[b.0 as usize].term {
        bk.args.push(sval);
    }
    // The way in: repeat the header's test on the entry arguments and load only when it passes.
    let Some(Term::Jump(entry)) = f.blocks[pre.0 as usize].term.clone() else {
        return false;
    };
    let mut sub: std::collections::HashMap<Value, Value> = params
        .iter()
        .copied()
        .zip(entry.args.iter().copied())
        .filter(|(p, _)| *p != cv)
        .collect();
    let header_insts: Vec<usize> = f.blocks[h.0 as usize].insts.clone();
    for i in header_insts {
        let mut op = f.insts[i].op.clone();
        op.map_values(|v| *sub.get(&v).unwrap_or(&v));
        let old = f.insts[i].result;
        let ty_r = old.map(|r| f.ty(r));
        let n = f.emit_at(pre, op, ty_r, span);
        if let (Some(old), Some(n)) = (old, n) {
            sub.insert(old, n);
        }
    }
    let cond = *sub.get(&c).unwrap_or(&c);
    let p0 = *sub.get(&p).unwrap_or(&p);
    let ld_block = f.new_block(&[]);
    let zero_block = f.new_block(&[]);
    let kc = f
        .emit_at(
            ld_block,
            Op::Const(IrTy::I64, Const::Int(k)),
            Some(IrTy::I64),
            span,
        )
        .expect("a constant");
    let off = f
        .emit_at(ld_block, Op::Bin(BinOp::Mul, p0, kc), Some(IrTy::I64), span)
        .expect("an offset");
    let at = f
        .emit_at(ld_block, Op::PtrAdd(base, off), Some(IrTy::Ptr), span)
        .expect("an address");
    let first = f
        .emit_at(ld_block, Op::Load(ty, at), Some(ty), span)
        .expect("a load");
    let zero = f
        .emit_at(zero_block, Op::Const(ty, Const::Int(0)), Some(ty), span)
        .expect("a constant");
    let mut ld_args = entry.args.clone();
    ld_args.push(first);
    let mut zero_args = entry.args.clone();
    zero_args.push(zero);
    f.terminate(
        ld_block,
        Term::Jump(Target {
            block: h,
            args: ld_args,
        }),
    );
    f.terminate(
        zero_block,
        Term::Jump(Target {
            block: h,
            args: zero_args,
        }),
    );
    let (on, off_t) = (
        Target {
            block: ld_block,
            args: Vec::new(),
        },
        Target {
            block: zero_block,
            args: Vec::new(),
        },
    );
    f.terminate(
        pre,
        if body_on_true {
            Term::Branch(cond, on, off_t)
        } else {
            Term::Branch(cond, off_t, on)
        },
    );
    // The loads in the body read the carried value.
    let mut rep: std::collections::HashMap<Value, Value> = std::collections::HashMap::new();
    for (_, r) in &loads {
        rep.insert(*r, cv);
    }
    f.replace_uses(&rep);
    let gone: std::collections::HashSet<usize> = loads.iter().map(|(i, _)| *i).collect();
    f.blocks[b.0 as usize].insts.retain(|i| !gone.contains(i));
    true
}
