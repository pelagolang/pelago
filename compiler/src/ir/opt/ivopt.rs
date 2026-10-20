//! Counters that move together become one (card A-11). In a loop, a block parameter that the loop
//! sends back as itself plus a constant is a counter (`x = x + 3`, `i = i + 1`). A value made from
//! counters by wrapping `add`, `sub`, `mul` by a constant or `shl` by a constant moves by a fixed step
//! each pass too: `3 * x + y` with the steps above gains `3 * 3 + 1` per pass. That value becomes a
//! counter of its own, started from the same formula on the values the loop is entered with, so the
//! pass computes it with a single `add` instead of the whole formula.
//!
//! Only when it pays: the formula must combine two or more counters, and nothing but the formula may
//! read them or its parts, so that the old counters die. Only wrapping operations are followed (a
//! checked one may stop, and a removed one would not), and the values are 64-bit, so the identity
//! `3 * (x + a) + (y + b) = (3 * x + y) + (3 * a + b)` holds modulo `2^64` exactly as the original
//! computation wraps.

use super::dom::Dom;
use super::licm::preheader;
use super::loops::{self, Loop};
use crate::ir::*;
use std::collections::{HashMap, HashSet};

/// `k + sum(c * p)` over counters `p`, with wrapping coefficients.
#[derive(Clone, Debug)]
struct Aff {
    k: u64,
    terms: Vec<(Value, u64)>,
}

impl Aff {
    fn konst(k: u64) -> Aff {
        Aff { k, terms: vec![] }
    }

    fn scale(&self, m: u64) -> Aff {
        Aff {
            k: self.k.wrapping_mul(m),
            terms: self
                .terms
                .iter()
                .map(|(p, c)| (*p, c.wrapping_mul(m)))
                .collect(),
        }
    }

    fn add(&self, o: &Aff) -> Aff {
        let mut terms = self.terms.clone();
        for (p, c) in &o.terms {
            match terms.iter_mut().find(|t| t.0 == *p) {
                Some(t) => t.1 = t.1.wrapping_add(*c),
                None => terms.push((*p, *c)),
            }
        }
        terms.retain(|t| t.1 != 0);
        Aff {
            k: self.k.wrapping_add(o.k),
            terms,
        }
    }
}

pub fn run(f: &mut Func) -> bool {
    let mut changed = false;
    for _ in 0..16 {
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
        if lp.header.0 != 0 && try_loop(f, &lp) {
            return true;
        }
    }
    false
}

fn try_loop(f: &mut Func, lp: &Loop) -> bool {
    let h = lp.header;
    let preds = f.predecessors();
    let latches: Vec<BlockId> = preds[h.0 as usize]
        .iter()
        .copied()
        .filter(|p| lp.contains(*p))
        .collect();
    let [latch] = latches.as_slice() else {
        return false;
    };
    let latch = *latch;
    let Some(Term::Jump(back)) = &f.blocks[latch.0 as usize].term else {
        return false;
    };
    let back = back.clone();
    let defs = f.defs();
    let mut block_of = vec![u32::MAX; f.insts.len()];
    for (bi, b) in f.blocks.iter().enumerate() {
        for &i in &b.insts {
            block_of[i] = bi as u32;
        }
    }
    let in_loop = |i: usize| block_of[i] != u32::MAX && lp.body[block_of[i] as usize];
    let const_of = |v: Value| -> Option<u64> {
        match defs[v.0 as usize].map(|i| &f.insts[i].op) {
            Some(Op::Const(IrTy::Int(64), Const::Int(c))) => Some(*c as u64),
            _ => None,
        }
    };

    // The counters: header parameters sent back as themselves plus a constant.
    let params = f.blocks[h.0 as usize].params.clone();
    let mut step: HashMap<Value, u64> = HashMap::new();
    for (idx, &p) in params.iter().enumerate() {
        if f.ty(p) != IrTy::Int(64) {
            continue;
        }
        let Some(i) = defs[back.args[idx].0 as usize] else {
            continue;
        };
        if !in_loop(i) {
            continue;
        }
        match &f.insts[i].op {
            Op::Bin(BinOp::Add, a, c) if *a == p => {
                if let Some(k) = const_of(*c) {
                    step.insert(p, k);
                }
            }
            Op::Bin(BinOp::Add, c, a) if *a == p => {
                if let Some(k) = const_of(*c) {
                    step.insert(p, k);
                }
            }
            Op::Bin(BinOp::Sub, a, c) if *a == p => {
                if let Some(k) = const_of(*c) {
                    step.insert(p, k.wrapping_neg());
                }
            }
            _ => {}
        }
    }
    if step.len() < 2 {
        return false;
    }

    // Every value in the loop made from counters, as a formula.
    let mut aff: HashMap<Value, Option<Aff>> = HashMap::new();
    let mut cone: HashMap<Value, Vec<usize>> = HashMap::new();
    for &b in &Dom::new(f).rpo {
        if !lp.contains(b) {
            continue;
        }
        for &i in &f.blocks[b.0 as usize].insts {
            let inst = &f.insts[i];
            let Some(r) = inst.result else { continue };
            if f.ty(r) != IrTy::Int(64) {
                continue;
            }
            let get = |v: Value, aff: &HashMap<Value, Option<Aff>>| -> Option<Aff> {
                if step.contains_key(&v) {
                    return Some(Aff {
                        k: 0,
                        terms: vec![(v, 1)],
                    });
                }
                if let Some(c) = const_of(v) {
                    return Some(Aff::konst(c));
                }
                aff.get(&v).cloned().flatten()
            };
            let a = match &inst.op {
                Op::Bin(BinOp::Add, x, y) => match (get(*x, &aff), get(*y, &aff)) {
                    (Some(x), Some(y)) => Some(x.add(&y)),
                    _ => None,
                },
                Op::Bin(BinOp::Sub, x, y) => match (get(*x, &aff), get(*y, &aff)) {
                    (Some(x), Some(y)) => Some(x.add(&y.scale(u64::MAX))),
                    _ => None,
                },
                Op::Bin(BinOp::Mul, x, y) => match (get(*x, &aff), get(*y, &aff)) {
                    (Some(x), Some(y)) if y.terms.is_empty() => Some(x.scale(y.k)),
                    (Some(x), Some(y)) if x.terms.is_empty() => Some(y.scale(x.k)),
                    _ => None,
                },
                Op::Bin(BinOp::Shl, x, y) => match (get(*x, &aff), const_of(*y)) {
                    (Some(x), Some(k)) if k < 64 => Some(x.scale(1u64 << k)),
                    _ => None,
                },
                Op::Neg(x) => get(*x, &aff).map(|x| x.scale(u64::MAX)),
                _ => None,
            };
            if a.is_some() {
                let mut c = vec![i];
                if let Op::Bin(_, x, y) = &inst.op {
                    for v in [x, y] {
                        if let Some(sub) = cone.get(v) {
                            c.extend(sub.iter().copied());
                        }
                    }
                } else if let Op::Neg(x) = &inst.op {
                    if let Some(sub) = cone.get(x) {
                        c.extend(sub.iter().copied());
                    }
                }
                cone.insert(r, c);
            }
            aff.insert(r, a);
        }
    }

    // How each value is read, by instruction and by terminator.
    let mut inst_uses: HashMap<Value, Vec<usize>> = HashMap::new();
    let mut term_uses: HashMap<Value, usize> = HashMap::new();
    for b in &f.blocks {
        for &i in &b.insts {
            f.insts[i]
                .op
                .for_each_value(|v| inst_uses.entry(v).or_default().push(i));
        }
        if let Some(t) = &b.term {
            t.for_each_value(|v| *term_uses.entry(v).or_default() += 1);
        }
    }

    for &b in &Dom::new(f).rpo {
        if !lp.contains(b) {
            continue;
        }
        for &i in &f.blocks[b.0 as usize].insts {
            let Some(v) = f.insts[i].result else { continue };
            let Some(Some(a)) = aff.get(&v) else { continue };
            let used = inst_uses.contains_key(&v) || term_uses.contains_key(&v);
            if a.terms.len() < 2 || !used {
                continue;
            }
            let insts: HashSet<usize> = cone[&v].iter().copied().collect();
            if profitable(f, &back, &insts, v, a, &inst_uses, &term_uses) {
                let a = a.clone();
                return rewrite(f, lp, latch, v, &a, &step);
            }
        }
    }
    false
}

/// True when replacing `v` by a counter leaves the old counters and the formula unused.
#[allow(clippy::too_many_arguments)]
fn profitable(
    f: &Func,
    back: &Target,
    cone: &HashSet<usize>,
    v: Value,
    a: &Aff,
    inst_uses: &HashMap<Value, Vec<usize>>,
    term_uses: &HashMap<Value, usize>,
) -> bool {
    let involved: HashSet<Value> = a.terms.iter().map(|t| t.0).collect();
    // Each involved counter is read only by the formula, and its next value only by the formula
    // and the jump back.
    for p in &involved {
        let outside = inst_uses
            .get(p)
            .is_some_and(|us| us.iter().any(|u| !cone.contains(u)));
        if outside || term_uses.contains_key(p) {
            return false;
        }
    }
    // Everything the formula makes, except `v` itself, is read only by the formula, or sent back as
    // the next value of an involved counter.
    for &ci in cone {
        let Some(r) = f.insts[ci].result else {
            continue;
        };
        if r == v {
            continue;
        }
        if inst_uses
            .get(&r)
            .is_some_and(|us| us.iter().any(|u| !cone.contains(u)))
        {
            return false;
        }
        let sent = term_uses.get(&r).copied().unwrap_or(0);
        let ok_sent = back.args.iter().filter(|x| **x == r).count();
        if sent != ok_sent {
            return false;
        }
        if sent > 0 {
            // It may only be the next value of a counter that the formula uses.
            let idx = back.args.iter().position(|x| *x == r).unwrap_or(0);
            let p = f.blocks[back.block.0 as usize].params[idx];
            if !involved.contains(&p) {
                return false;
            }
        }
    }
    true
}

fn rewrite(
    f: &mut Func,
    lp: &Loop,
    latch: BlockId,
    v: Value,
    a: &Aff,
    step: &HashMap<Value, u64>,
) -> bool {
    let h = lp.header;
    let pre = preheader(f, lp);
    let Some(Term::Jump(entry)) = f.blocks[pre.0 as usize].term.clone() else {
        return false;
    };
    let params = f.blocks[h.0 as usize].params.clone();
    let init_of = |p: Value| {
        let idx = params.iter().position(|x| *x == p).unwrap_or(0);
        entry.args[idx]
    };
    let i64t = IrTy::Int(64);
    let konst = |f: &mut Func, b: BlockId, c: u64| {
        f.emit(b, Op::Const(i64t, Const::Int(c as i64 as i128)), Some(i64t))
            .unwrap_or(v)
    };
    // The start: the formula on the values the loop is entered with.
    let mut start = konst(f, pre, a.k);
    for (p, c) in &a.terms {
        let init = init_of(*p);
        let term = if *c == 1 {
            init
        } else {
            let cv = konst(f, pre, *c);
            f.emit(pre, Op::Bin(BinOp::Mul, init, cv), Some(i64t))
                .unwrap_or(v)
        };
        start = f
            .emit(pre, Op::Bin(BinOp::Add, start, term), Some(i64t))
            .unwrap_or(v);
    }
    // The step: the formula's coefficients on the counters' steps.
    let mut by = 0u64;
    for (p, c) in &a.terms {
        by = by.wrapping_add(c.wrapping_mul(step[p]));
    }
    let z = f.new_value(i64t);
    f.blocks[h.0 as usize].params.push(z);
    let by = konst(f, latch, by);
    let next = f
        .emit(latch, Op::Bin(BinOp::Add, z, by), Some(i64t))
        .unwrap_or(v);
    if let Some(Term::Jump(t)) = &mut f.blocks[pre.0 as usize].term {
        t.args.push(start);
    }
    if let Some(Term::Jump(t)) = &mut f.blocks[latch.0 as usize].term {
        t.args.push(next);
    }
    let mut rep = HashMap::new();
    rep.insert(v, z);
    f.replace_uses(&rep);
    true
}
