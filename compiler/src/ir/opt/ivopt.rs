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

/// The counters of a formula with their coefficients.
type Terms = Vec<(Value, u64)>;

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
    let const_of = |v: Value| -> Option<u64> {
        match defs[v.0 as usize].map(|i| &f.insts[i].op) {
            Some(Op::Const(IrTy::Int(64), Const::Int(c))) => Some(*c as u64),
            _ => None,
        }
    };

    // Header parameters are the atoms of the formulas; the counters among them are found below.
    let params = f.blocks[h.0 as usize].params.clone();
    let atoms: HashSet<Value> = params
        .iter()
        .copied()
        .filter(|p| f.ty(*p) == IrTy::Int(64))
        .collect();
    let mut step: HashMap<Value, u64> = HashMap::new();

    // Every value in the loop made from counters, as a formula.
    let mut aff: HashMap<Value, Option<Aff>> = HashMap::new();
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
                if atoms.contains(&v) {
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
            aff.insert(r, a);
        }
    }

    // A counter is an atom that the loop sends back as itself plus a constant, however many `add`s
    // that takes.
    for (idx, &p) in params.iter().enumerate() {
        if !atoms.contains(&p) {
            continue;
        }
        let sent = back.args[idx];
        let a = match aff.get(&sent) {
            Some(Some(a)) => Some(a.clone()),
            _ if sent == p => Some(Aff {
                k: 0,
                terms: vec![(p, 1)],
            }),
            _ => None,
        };
        if let Some(a) = a {
            if a.terms.len() == 1 && a.terms[0] == (p, 1) {
                step.insert(p, a.k);
            }
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

    // Values with the same counters and coefficients differ by a constant, so they share one counter.
    let mut groups: Vec<(Terms, Vec<(Value, u64)>)> = Vec::new();
    for &b in &Dom::new(f).rpo {
        if !lp.contains(b) {
            continue;
        }
        for &i in &f.blocks[b.0 as usize].insts {
            let Some(v) = f.insts[i].result else { continue };
            let Some(Some(a)) = aff.get(&v) else { continue };
            let used = inst_uses.contains_key(&v) || term_uses.contains_key(&v);
            let counters = a.terms.len() >= 2 && a.terms.iter().all(|t| step.contains_key(&t.0));
            let offset = a.terms.len() == 1 && a.terms[0].1 == 1;
            if !used || !(counters || offset) {
                continue;
            }
            let mut key = a.terms.clone();
            key.sort();
            match groups.iter_mut().find(|g| g.0 == key) {
                Some(g) => g.1.push((v, a.k)),
                None => groups.push((key, vec![(v, a.k)])),
            }
        }
    }
    // Keep a group only when the loop ends up with fewer instructions.
    let before = weight(f, lp);
    for (terms, members) in groups {
        let mut g = f.clone();
        let mut level = false;
        if let [(p, 1)] = terms.as_slice() {
            level = offsets(&mut g, *p, &members);
        } else {
            rewrite(&mut g, lp, latch, &terms, &members, &step);
        }
        super::dce::run(&mut g);
        let after = weight(&g, lp);
        if after < before || (level && after == before) {
            *f = g;
            return true;
        }
    }
    false
}

/// Instructions that run in the loop count much more than those that run once.
fn weight(f: &Func, lp: &Loop) -> usize {
    let mut base = f.clone();
    super::dce::run(&mut base);
    base.blocks
        .iter()
        .enumerate()
        .map(|(bi, b)| {
            let inside = bi < lp.body.len() && lp.body[bi];
            b.insts.len() * if inside { 16 } else { 1 }
        })
        .sum()
}

fn rewrite(
    f: &mut Func,
    lp: &Loop,
    latch: BlockId,
    terms: &[(Value, u64)],
    members: &[(Value, u64)],
    step: &HashMap<Value, u64>,
) {
    let h = lp.header;
    let pre = preheader(f, lp);
    let Some(Term::Jump(entry)) = f.blocks[pre.0 as usize].term.clone() else {
        return;
    };
    let params = f.blocks[h.0 as usize].params.clone();
    let init_of = |p: Value| {
        let idx = params.iter().position(|x| *x == p).unwrap_or(0);
        entry.args[idx]
    };
    let i64t = IrTy::Int(64);
    let konst = |f: &mut Func, b: BlockId, c: u64| {
        f.emit(b, Op::Const(i64t, Const::Int(c as i64 as i128)), Some(i64t))
            .expect("a constant makes a value")
    };
    // The start: the formula on the values the loop is entered with, for the first member.
    let (first, base_k) = members[0];
    let mut start = konst(f, pre, base_k);
    for (p, c) in terms {
        let init = init_of(*p);
        let term = if *c == 1 {
            init
        } else {
            let cv = konst(f, pre, *c);
            f.emit(pre, Op::Bin(BinOp::Mul, init, cv), Some(i64t))
                .expect("a product makes a value")
        };
        start = f
            .emit(pre, Op::Bin(BinOp::Add, start, term), Some(i64t))
            .expect("a sum makes a value");
    }
    // The step: the formula's coefficients on the counters' steps.
    let mut by = 0u64;
    for (p, c) in terms {
        by = by.wrapping_add(c.wrapping_mul(step[p]));
    }
    let z = f.new_value(i64t);
    f.blocks[h.0 as usize].params.push(z);
    let by = konst(f, latch, by);
    let next = f
        .emit(latch, Op::Bin(BinOp::Add, z, by), Some(i64t))
        .expect("a sum makes a value");
    if let Some(Term::Jump(t)) = &mut f.blocks[pre.0 as usize].term {
        t.args.push(start);
    }
    if let Some(Term::Jump(t)) = &mut f.blocks[latch.0 as usize].term {
        t.args.push(next);
    }
    // Each member is the counter plus the constant that separates it from the first.
    let mut rep = HashMap::new();
    for &(m, k) in members {
        if m == first || k == base_k {
            rep.insert(m, z);
            continue;
        }
        let delta = k.wrapping_sub(base_k);
        if let Some(r) = add_after(f, m, z, delta) {
            rep.insert(m, r);
        }
    }
    f.replace_uses(&rep);
}

/// Values that are a counter plus a constant are made from the counter itself, not from each other,
/// so a chain of additions (what an unrolled loop leaves) does not depend on its own earlier links.
fn offsets(f: &mut Func, p: Value, members: &[(Value, u64)]) -> bool {
    let defs = f.defs();
    let mut rep = HashMap::new();
    for &(m, k) in members {
        // Already `p + constant`: nothing to shorten.
        if let Some(Op::Bin(BinOp::Add, a, c)) = defs[m.0 as usize].map(|i| &f.insts[i].op) {
            if *a == p
                && matches!(
                    defs[c.0 as usize].map(|i| &f.insts[i].op),
                    Some(Op::Const(..))
                )
            {
                continue;
            }
        }
        if let Some(r) = add_after(f, m, p, k) {
            rep.insert(m, r);
        }
    }
    let any = !rep.is_empty();
    f.replace_uses(&rep);
    any
}

/// `base + delta`, made right after the instruction that makes `m`.
fn add_after(f: &mut Func, m: Value, base: Value, delta: u64) -> Option<Value> {
    let (bi, pos) = locate(f, m)?;
    let i64t = IrTy::Int(64);
    let c = f.new_value(i64t);
    f.insts.push(Inst {
        result: Some(c),
        op: Op::Const(i64t, Const::Int(delta as i64 as i128)),
        span: Span::new(0, 0),
    });
    let cid = f.insts.len() - 1;
    let r = f.new_value(i64t);
    f.insts.push(Inst {
        result: Some(r),
        op: Op::Bin(BinOp::Add, base, c),
        span: Span::new(0, 0),
    });
    let rid = f.insts.len() - 1;
    f.blocks[bi].insts.insert(pos + 1, cid);
    f.blocks[bi].insts.insert(pos + 2, rid);
    Some(r)
}

/// The block and position of the instruction that makes `v`.
fn locate(f: &Func, v: Value) -> Option<(usize, usize)> {
    for (bi, b) in f.blocks.iter().enumerate() {
        for (pos, &i) in b.insts.iter().enumerate() {
            if f.insts[i].result == Some(v) {
                return Some((bi, pos));
            }
        }
    }
    None
}
