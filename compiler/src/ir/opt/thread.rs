//! Jump threading (card A-11): a block that only computes a test from its parameters and branches
//! is skipped by a predecessor that already knows the answer.
//!
//! ```text
//! a:  jump j(1)             a:  jump err_path       (the test of `j` is decided here)
//! j(tag):  c = eq tag, 0    j(tag):  c = eq tag, 0
//!     branch c, ok, bad         branch c, ok, bad    (still there for the other predecessors)
//! ```
//!
//! This is what `?` leaves behind once the callee is inlined: the callee jumps to a join with a
//! constant tag and the caller branches on the tag. The walk follows jumps and decided branches through
//! blocks that only make cheap, harmless values, so a chain of such tests goes in one step. A value the
//! skipped blocks made and later blocks still read becomes a parameter of the block that reads it (that
//! block has no other way in), so the program stays in SSA form.
//!
//! Loop headers are left alone: skipping the test of a loop would rotate the loop and hide it from the
//! loop passes.

use super::dom::Dom;
use crate::ir::*;
use std::collections::{HashMap, HashSet};

/// The most real instructions (constants are free) in all the blocks one walk skips.
const WORK: usize = 8;
/// The most edges changed in one run.
const BUDGET: usize = 64;

pub fn run(f: &mut Func) -> bool {
    let mut changed = false;
    for _ in 0..BUDGET {
        if !thread_one(f) {
            break;
        }
        // A block that only the skipped edge led to is dead now.
        f.compact();
        changed = true;
    }
    changed
}

/// Would the jump into `edge.block` be threaded? If-conversion leaves such a diamond alone: a decided
/// test beats a conditional move.
pub fn threadable(f: &Func, edge: &Target) -> bool {
    let dom = Dom::new(f);
    let preds = f.predecessors();
    let defs = f.defs();
    let header: Vec<bool> = (0..f.blocks.len())
        .map(|h| {
            preds[h]
                .iter()
                .any(|p| dom.dominates(BlockId(h as u32), *p))
        })
        .collect();
    walk(f, &defs, &preds, &header, edge).is_some()
}

/// How a skipped block goes on.
#[derive(Clone, Copy)]
enum Pick {
    Jump,
    Then,
    Else,
}

fn pick(f: &Func, b: usize, p: Pick) -> Target {
    match (&f.blocks[b].term, p) {
        (Some(Term::Jump(t)), _) => t.clone(),
        (Some(Term::Branch(_, t, _)), Pick::Then) => t.clone(),
        (Some(Term::Branch(_, _, e)), _) => e.clone(),
        _ => unreachable!(),
    }
}

fn cheap(op: &Op) -> Option<bool> {
    match op {
        Op::Const(..) => Some(false),
        Op::Bin(o, ..) if !matches!(o, BinOp::Div | BinOp::Rem | BinOp::UDiv | BinOp::URem) => {
            Some(true)
        }
        Op::Cmp(..) | Op::Not(_) | Op::Neg(_) | Op::Cast(..) | Op::Select(..) => Some(true),
        _ => None,
    }
}

struct Plan {
    /// The blocks skipped, in the order the walk met them, and how each goes on.
    chain: Vec<(usize, Pick)>,
}

fn thread_one(f: &mut Func) -> bool {
    let dom = Dom::new(f);
    let preds = f.predecessors();
    let defs = f.defs();
    let n = f.blocks.len();
    let mut header = vec![false; n];
    for h in 0..n {
        if preds[h]
            .iter()
            .any(|p| dom.dominates(BlockId(h as u32), *p))
        {
            header[h] = true;
        }
    }
    for a in 0..n {
        if !dom.reachable(BlockId(a as u32)) {
            continue;
        }
        let Some(term) = &f.blocks[a].term else {
            continue;
        };
        let edges: Vec<Target> = match term {
            Term::Jump(t) => vec![t.clone()],
            Term::Branch(_, t, e) => vec![t.clone(), e.clone()],
            _ => continue,
        };
        for (k, edge) in edges.iter().enumerate() {
            let t = edge.block.0 as usize;
            if t == 0 || t == a || header[t] {
                continue;
            }
            let Some(plan) = walk(f, &defs, &preds, &header, edge) else {
                continue;
            };
            if apply(f, &dom, &preds, a, k, &plan) {
                return true;
            }
        }
    }
    false
}

/// Follows the edge into blocks whose test is decided by the values it hands over.
fn walk(
    f: &Func,
    defs: &[Option<usize>],
    preds: &[Vec<BlockId>],
    header: &[bool],
    edge: &Target,
) -> Option<Plan> {
    let mut known: HashMap<Value, Val> = HashMap::new();
    let mut seen: HashSet<usize> = HashSet::new();
    let mut steps: Vec<(usize, Pick)> = Vec::new();
    let mut last_decided = None;
    let mut work = 0;
    let get = |known: &HashMap<Value, Val>, v: Value| -> Option<Val> {
        known.get(&v).copied().or_else(|| {
            let i = defs[v.0 as usize]?;
            match &f.insts[i].op {
                op @ Op::Const(..) => fold(op, f, &|_| None),
                _ => None,
            }
        })
    };
    let mut cur = edge.clone();
    loop {
        let b = cur.block.0 as usize;
        if b == 0 || header[b] || !seen.insert(b) {
            break;
        }
        if !steps.is_empty() && preds[b].len() != 1 {
            break;
        }
        let blk = &f.blocks[b];
        let mut real = 0;
        for &i in &blk.insts {
            match cheap(&f.insts[i].op) {
                Some(true) => real += 1,
                Some(false) => {}
                None => {
                    real = usize::MAX / 2;
                    break;
                }
            }
        }
        if work + real > WORK {
            break;
        }
        for (p, a) in blk.params.iter().zip(&cur.args) {
            if let Some(v) = get(&known, *a) {
                known.insert(*p, v);
            }
        }
        for &i in &blk.insts {
            if let Some(r) = f.insts[i].result {
                if let Some(v) = fold(&f.insts[i].op, f, &|x| get(&known, x)) {
                    known.insert(r, v);
                }
            }
        }
        match &blk.term {
            Some(Term::Jump(t)) => {
                work += real;
                steps.push((b, Pick::Jump));
                cur = t.clone();
            }
            Some(Term::Branch(c, t, e)) => {
                let Some(Val::Bool(v)) = get(&known, *c) else {
                    break;
                };
                work += real;
                let (p, next) = if v {
                    (Pick::Then, t.clone())
                } else {
                    (Pick::Else, e.clone())
                };
                steps.push((b, p));
                last_decided = Some(steps.len());
                cur = next;
            }
            _ => break,
        }
    }
    let end = last_decided?;
    steps.truncate(end);
    Some(Plan { chain: steps })
}

fn apply(f: &mut Func, dom: &Dom, preds: &[Vec<BlockId>], a: usize, k: usize, plan: &Plan) -> bool {
    let in_chain: HashSet<usize> = plan.chain.iter().map(|(b, _)| *b).collect();
    let (last, lp) = *plan.chain.last().unwrap();
    let landing = pick(f, last, lp);
    let l = landing.block.0 as usize;
    // The landing block must not be one the walk skipped, nor the block that is being changed.
    if l == a || l == 0 || in_chain.contains(&l) {
        return false;
    }
    // Is the landing a loop header? Its predecessors then include a block it dominates.
    if preds[l]
        .iter()
        .any(|p| dom.dominates(BlockId(l as u32), *p))
    {
        return false;
    }
    // Values the skipped blocks make and blocks outside read.
    let mut made: HashSet<Value> = HashSet::new();
    for &(b, _) in &plan.chain {
        made.extend(f.blocks[b].params.iter().copied());
        for &i in &f.blocks[b].insts {
            if let Some(r) = f.insts[i].result {
                made.insert(r);
            }
        }
    }
    // Where each is read outside the skipped blocks.
    let mut readers: HashMap<Value, Vec<usize>> = HashMap::new();
    for (bi, blk) in f.blocks.iter().enumerate() {
        if in_chain.contains(&bi) {
            continue;
        }
        for &i in &blk.insts {
            f.insts[i].op.for_each_value(|v| {
                if made.contains(&v) {
                    readers.entry(v).or_default().push(bi);
                }
            });
        }
        if let Some(t) = &blk.term {
            t.for_each_value(|v| {
                if made.contains(&v) {
                    readers.entry(v).or_default().push(bi);
                }
            });
        }
    }
    // Each such value reaches its readers through a block that only a skipped block enters: that block
    // gets it as a new parameter.
    let mut exits: Vec<usize> = Vec::new();
    for &(b, _) in &plan.chain {
        for s in f.successors(BlockId(b as u32)) {
            let s = s.0 as usize;
            if !in_chain.contains(&s) && !exits.contains(&s) {
                exits.push(s);
            }
        }
    }
    let mut routes: Vec<(Value, usize, usize)> = Vec::new();
    let mut sorted: Vec<_> = readers.iter().collect();
    sorted.sort_by_key(|(v, _)| v.0);
    for (&x, users) in sorted {
        let mut used: Vec<usize> = Vec::new();
        for u in users {
            let Some(&e) = exits
                .iter()
                .find(|&&e| dom.dominates(BlockId(e as u32), BlockId(*u as u32)))
            else {
                return false;
            };
            if !used.contains(&e) {
                used.push(e);
            }
        }
        for e in used {
            let [from] = preds[e][..] else {
                return false;
            };
            if !in_chain.contains(&(from.0 as usize)) {
                return false;
            }
            // Two edges from the skipped block to the exit would need two arguments.
            let edges = f
                .successors(from)
                .iter()
                .filter(|s| s.0 as usize == e)
                .count();
            if edges != 1 {
                return false;
            }
            routes.push((x, e, from.0 as usize));
        }
    }
    let first = match &f.blocks[a].term {
        Some(Term::Jump(t)) if k == 0 => t.clone(),
        Some(Term::Branch(_, t, e)) => {
            if k == 0 {
                t.clone()
            } else {
                e.clone()
            }
        }
        _ => return false,
    };
    for (x, e, from) in routes {
        let p = f.new_value(f.ty(x));
        f.blocks[e].params.push(p);
        if let Some(t) = &mut f.blocks[from].term {
            for tg in t.targets_mut() {
                if tg.block.0 as usize == e {
                    tg.args.push(x);
                }
            }
        }
        for bi in 0..f.blocks.len() {
            if in_chain.contains(&bi) || !dom.dominates(BlockId(e as u32), BlockId(bi as u32)) {
                continue;
            }
            let ids = f.blocks[bi].insts.clone();
            for i in ids {
                f.insts[i].op.map_values(|v| if v == x { p } else { v });
            }
            if let Some(t) = &mut f.blocks[bi].term {
                t.map_values(|v| if v == x { p } else { v });
            }
        }
    }
    // Make the skipped blocks' values in `a`, then go to the landing.
    let mut env: HashMap<Value, Value> = HashMap::new();
    let mut args = first.args;
    for &(b, p) in &plan.chain {
        let params = f.blocks[b].params.clone();
        for (q, v) in params.iter().zip(&args) {
            env.insert(*q, *v);
        }
        let ids = f.blocks[b].insts.clone();
        for i in ids {
            let mut inst = f.insts[i].clone();
            inst.op.map_values(|v| env.get(&v).copied().unwrap_or(v));
            if let Some(r) = inst.result {
                let nr = f.new_value(f.ty(r));
                env.insert(r, nr);
                inst.result = Some(nr);
            }
            f.insts.push(inst);
            let id = f.insts.len() - 1;
            f.blocks[a].insts.push(id);
        }
        args = pick(f, b, p).args;
        args.iter_mut()
            .for_each(|v| *v = env.get(v).copied().unwrap_or(*v));
    }
    let new = Target {
        block: BlockId(l as u32),
        args,
    };
    if let Some(t) = &mut f.blocks[a].term {
        let mut targets = t.targets_mut();
        *targets[k] = new;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(f: &mut Func, b: BlockId, ty: IrTy, n: i128) -> Value {
        f.emit(b, Op::Const(ty, Const::Int(n)), Some(ty)).unwrap()
    }

    fn to(block: BlockId) -> Target {
        Target {
            block,
            args: vec![],
        }
    }

    /// `main(x)`: one side hands the join a tag of 1 and the other a tag of 0; the join tests the tag and
    /// both of its successors read the value the join was given.
    fn tagged() -> Module {
        let mut f = Func::new("main", &[IrTy::I64], IrTy::I64);
        let e = f.entry();
        let x = f.block_params(e)[0];
        let a = f.new_block(&[]);
        let b = f.new_block(&[]);
        let j = f.new_block(&[IrTy::I64, IrTy::I8]);
        let ok = f.new_block(&[]);
        let bad = f.new_block(&[]);
        let five = int(&mut f, e, IrTy::I64, 5);
        let c = f
            .emit(e, Op::Cmp(CmpOp::Lt, x, five), Some(IrTy::Bool))
            .unwrap();
        f.terminate(e, Term::Branch(c, to(a), to(b)));
        let tag = int(&mut f, a, IrTy::I8, 1);
        let args = vec![x, tag];
        f.terminate(a, Term::Jump(Target { block: j, args }));
        let one = int(&mut f, b, IrTy::I64, 1);
        let x1 = f
            .emit(b, Op::Bin(BinOp::Add, x, one), Some(IrTy::I64))
            .unwrap();
        let tag = int(&mut f, b, IrTy::I8, 0);
        let args = vec![x1, tag];
        f.terminate(b, Term::Jump(Target { block: j, args }));
        let (v, tag) = (f.block_params(j)[0], f.block_params(j)[1]);
        let zero = int(&mut f, j, IrTy::I8, 0);
        let is_ok = f
            .emit(j, Op::Cmp(CmpOp::Eq, tag, zero), Some(IrTy::Bool))
            .unwrap();
        f.terminate(j, Term::Branch(is_ok, to(ok), to(bad)));
        let two = int(&mut f, ok, IrTy::I64, 2);
        let r = f
            .emit(ok, Op::Bin(BinOp::Mul, v, two), Some(IrTy::I64))
            .unwrap();
        f.terminate(ok, Term::Return(Some(r)));
        let hundred = int(&mut f, bad, IrTy::I64, 100);
        let r = f
            .emit(bad, Op::Bin(BinOp::Add, v, hundred), Some(IrTy::I64))
            .unwrap();
        f.terminate(bad, Term::Return(Some(r)));
        let mut m = Module::default();
        m.add(f);
        m
    }

    #[test]
    fn a_known_tag_skips_the_test() {
        let m = tagged();
        let mut o = m.clone();
        assert!(run(&mut o.funcs[0]));
        assert!(verify_module(&o).is_empty(), "{}", print_module(&o));
        // The join is gone: both sides go straight to the block they would have reached.
        let f = &o.funcs[0];
        let branches = f
            .blocks
            .iter()
            .filter(|b| matches!(b.term, Some(Term::Branch(..))))
            .count();
        assert_eq!(branches, 1, "{}", print_func(f, None));
        for x in [-3, 0, 4, 5, 6, 100] {
            let args = [Val::int(x)];
            assert_eq!(
                run_full(&m, "main", &args).result,
                run_full(&o, "main", &args).result,
                "x = {x}"
            );
        }
    }

    #[test]
    fn an_unknown_tag_is_left_alone() {
        let mut o = tagged();
        // The join's tag now comes from the argument, which nobody knows.
        let f = &mut o.funcs[0];
        let x = f.block_params(f.entry())[0];
        let t = f.new_value(IrTy::I8);
        f.insts.push(Inst {
            result: Some(t),
            op: Op::Cast(CastKind::Trunc, x, IrTy::I8),
            span: Span::new(0, 0),
        });
        let id = f.insts.len() - 1;
        f.blocks[0].insts.push(id);
        for b in [1, 2] {
            if let Some(Term::Jump(tg)) = &mut f.blocks[b].term {
                tg.args[1] = t;
            }
        }
        assert!(!run(f));
    }
}
