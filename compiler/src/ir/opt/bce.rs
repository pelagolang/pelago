//! Index tests settled once, in front of the loop (card A-11). In
//!
//! ```text
//! for j in 0..n { cur[j + 1] = prev[j] + 1 }
//! ```
//!
//! every pass tests `j < len(prev)` and `j + 1 < len(cur)`. The index is the counter plus a
//! constant, the counter runs from its start up to `n - 1`, and the lengths do not change while the
//! loop runs, so the tests can be made once: `start + k >= 0` and `n - 1 + k <u len`, one pair per
//! distinct test. The loop is then in two copies, behind that guard:
//!
//! - the **fast** copy is the loop without those tests;
//! - the **checked** copy (`Block::slow`) is the loop as it was. It runs when a guard fails, so a
//!   program that indexes out of range still stops at the very pass, and after the very output,
//!   that it always did. Nothing observable changes, overflow traps included.
//!
//! What is recognised: an innermost-or-outer loop `while i < n` with `n` made outside it, one latch
//! that sends `i + step` back (a constant step of at most 2^20), no calls to the outside world in
//! the way of a length, and tests that stop with `index out of range` on `x >=u len`, where `x` is
//! `i + k`, a constant, or a value made outside the loop, and `len` is a constant, a value made
//! outside, or a load of one from an address made outside. That last kind is hoisted only when the
//! loop has no call, no allocation, and stores that are all of the form `base + c * i + d` (`c >= 0`):
//! the guard then also compares the addresses, so no store of the loop can reach the length. The
//! trip count must be at most 2^32, which keeps every sum in the guard far from overflow; a longer
//! loop takes the checked copy.
//!
//! Only the tests that are provable are removed; a loop with an unprovable one keeps it in both
//! copies. The exits of the loop must be its header test and stops, so that one join block gets
//! the values the code after the loop reads from either copy.

use super::dom::Dom;
use super::licm::preheader;
use super::loops::{self, Loop};
use crate::ir::*;
use std::collections::{HashMap, HashSet};

const MSG: &str = "index out of range";
/// A loop of more passes than this takes the checked copy.
const MAX_TRIPS: i128 = 1 << 32;
const MAX_STEP: i128 = 1 << 20;
/// A loop known to run fewer passes than this keeps its tests.
const MIN_TRIPS: i128 = 16;
/// Bytes between two elements, and constant offsets, that the guard reasons about.
const MAX_COEF: i128 = 1 << 20;
/// Loops with more instructions than this are not copied.
const MAX_BODY: usize = 160;
const MAX_STORES: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Idx {
    /// `i + k`
    Off(i64),
    Konst(i64),
    /// Made outside the loop.
    Inv(Value),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Len {
    Konst(i64),
    Val(Value),
    /// A load of an `i64` from `base + off`, `base` made outside the loop.
    Load(Value, i64),
}

struct Check {
    block: BlockId,
    idx: Idx,
    len: Len,
}

/// A store of the loop: bytes `base + c * i + d` for `size` bytes.
struct Store {
    base: Value,
    c: i64,
    d: i64,
    size: i64,
}

struct Counter {
    i: Value,
    ix: usize,
    n: Value,
    exit: BlockId,
    latch: BlockId,
}

pub fn run(f: &mut Func) -> bool {
    if !f.blocks.iter().any(|b| !b.slow && is_check(f, b)) {
        return false;
    }
    let mut changed = false;
    for _ in 0..8 {
        if !once(f) {
            break;
        }
        changed = true;
    }
    changed
}

fn is_stop(f: &Func, b: BlockId, msg: Option<&str>) -> bool {
    let blk = &f.blocks[b.0 as usize];
    blk.insts.is_empty()
        && blk.params.is_empty()
        && match &blk.term {
            Some(Term::Stop(m)) => msg.is_none_or(|x| x == m),
            _ => false,
        }
}

fn is_check(f: &Func, b: &Block) -> bool {
    match &b.term {
        Some(Term::Branch(_, t, e)) => {
            (t.args.is_empty() && is_stop(f, t.block, Some(MSG)))
                || (e.args.is_empty() && is_stop(f, e.block, Some(MSG)))
        }
        _ => false,
    }
}

/// Number of index tests left outside the checked copies of a function.
pub fn kept_checks(f: &Func) -> usize {
    f.blocks
        .iter()
        .filter(|b| !b.slow && is_check(f, b))
        .count()
}

fn once(f: &mut Func) -> bool {
    let dom = Dom::new(f);
    for lp in loops::find(f, &dom) {
        if lp.header.0 == 0 || f.blocks[lp.header.0 as usize].slow {
            continue;
        }
        if let Some(plan) = analyse(f, &dom, &lp) {
            apply(f, &lp, plan);
            return true;
        }
    }
    false
}

struct Plan {
    counter: Counter,
    checks: Vec<Check>,
    stores: Vec<Store>,
    /// The ranges of the stores are needed (a length is loaded in the loop).
    loads: bool,
}

struct Facts<'a> {
    f: &'a Func,
    lp: &'a Loop,
    defs: Vec<Option<usize>>,
    home: Vec<u32>,
}

impl Facts<'_> {
    fn inside(&self, v: Value) -> bool {
        let h = self.home[v.0 as usize];
        h != u32::MAX && self.lp.body[h as usize]
    }

    fn op(&self, v: Value) -> Option<&Op> {
        self.defs[v.0 as usize].map(|i| &self.f.insts[i].op)
    }

    fn konst(&self, v: Value) -> Option<i128> {
        match self.op(v) {
            Some(Op::Const(IrTy::Int(_), Const::Int(c))) => Some(*c),
            _ => None,
        }
    }

    /// `v` as `c * i + d`, for the integers a loop index is made of.
    fn affine(&self, i: Value, v: Value, depth: u32) -> Option<(i128, i128)> {
        if v == i {
            return Some((1, 0));
        }
        if let Some(k) = self.konst(v) {
            return Some((0, k));
        }
        if depth > 6 || !self.inside(v) {
            return None;
        }
        let ok =
            |(c, d): (i128, i128)| (c.abs() <= MAX_COEF && d.abs() <= 1 << 40).then_some((c, d));
        match self.op(v)? {
            Op::Bin(o, a, b) | Op::Checked(o, true, a, b) => {
                let (x, y) = (
                    self.affine(i, *a, depth + 1)?,
                    self.affine(i, *b, depth + 1)?,
                );
                match o {
                    BinOp::Add => ok((x.0 + y.0, x.1 + y.1)),
                    BinOp::Sub => ok((x.0 - y.0, x.1 - y.1)),
                    BinOp::Mul if y.0 == 0 => ok((x.0 * y.1, x.1 * y.1)),
                    BinOp::Mul if x.0 == 0 => ok((y.0 * x.1, y.1 * x.1)),
                    BinOp::Shl if y.0 == 0 && (0..20).contains(&y.1) => {
                        ok((x.0 << y.1, x.1 << y.1))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// An address as `base + c * i + d`, `base` made outside the loop.
    fn address(&self, i: Value, v: Value, depth: u32) -> Option<(Value, i128, i128)> {
        if !self.inside(v) {
            return (self.f.ty(v) == IrTy::Ptr).then_some((v, 0, 0));
        }
        if depth > 4 {
            return None;
        }
        match self.op(v)? {
            Op::PtrAdd(p, o) => {
                let (b, c, d) = self.address(i, *p, depth + 1)?;
                let (c2, d2) = self.affine(i, *o, 0)?;
                let (c, d) = (c + c2, d + d2);
                (c.abs() <= MAX_COEF && d.abs() <= 1 << 40).then_some((b, c, d))
            }
            _ => None,
        }
    }
}

fn analyse(f: &Func, dom: &Dom, lp: &Loop) -> Option<Plan> {
    let h = lp.header;
    let mut home = vec![u32::MAX; f.values.len()];
    let mut size = 0;
    for (bi, b) in f.blocks.iter().enumerate() {
        for p in &b.params {
            home[p.0 as usize] = bi as u32;
        }
        for &ii in &b.insts {
            if let Some(r) = f.insts[ii].result {
                home[r.0 as usize] = bi as u32;
            }
            if lp.body[bi] {
                size += 1;
                if matches!(f.insts[ii].op, Op::Slot(..)) {
                    return None;
                }
            }
        }
    }
    if size > MAX_BODY {
        return None;
    }
    let facts = Facts {
        f,
        lp,
        defs: f.defs(),
        home,
    };
    let counter = counter(&facts, h)?;
    // A loop of a few passes does not earn its guard.
    if facts.konst(counter.n).is_some_and(|n| n < MIN_TRIPS) {
        return None;
    }
    // The loop leaves by its test or by a stop, and by nothing else.
    for (bi, b) in f.blocks.iter().enumerate() {
        if !lp.body[bi] {
            continue;
        }
        if matches!(
            b.term,
            Some(Term::Return(_)) | Some(Term::Unreachable) | None
        ) {
            return None;
        }
        for s in f.successors(BlockId(bi as u32)) {
            let out = !lp.contains(s);
            let fine = !out || (bi == h.0 as usize && s == counter.exit) || is_stop(f, s, None);
            if !fine {
                return None;
            }
        }
    }
    // Memory the loop may change: no calls; stores of a form the guard can bound.
    let mut calls = false;
    let mut stores = Vec::new();
    let mut stores_ok = true;
    for (bi, b) in f.blocks.iter().enumerate() {
        if !lp.body[bi] {
            continue;
        }
        for &ii in &b.insts {
            match &f.insts[ii].op {
                Op::Call(..)
                | Op::CallPtr(..)
                | Op::MemCopy(..)
                | Op::Alloc(_)
                | Op::Realloc(..)
                | Op::Free(_) => calls = true,
                Op::Store(a, v) => match facts.address(counter.i, *a, 0) {
                    Some((base, c, d)) if c >= 0 && stores.len() < MAX_STORES => {
                        stores.push(Store {
                            base,
                            c: c as i64,
                            d: d as i64,
                            size: f.ty(*v).size() as i64,
                        })
                    }
                    _ => stores_ok = false,
                },
                _ => {}
            }
        }
    }
    let mut checks = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !lp.body[bi] || b.slow {
            continue;
        }
        let Some(Term::Branch(c, t, e)) = &b.term else {
            continue;
        };
        let stop_then = t.args.is_empty() && is_stop(f, t.block, Some(MSG));
        let stop_else = e.args.is_empty() && is_stop(f, e.block, Some(MSG));
        if stop_then == stop_else {
            continue;
        }
        let Some(&Op::Cmp(op, a, b2)) = facts.op(*c) else {
            continue;
        };
        // Stops when `x >=u len`.
        let (x, len) = match (op, stop_then) {
            (CmpOp::UGe, true) | (CmpOp::ULt, false) => (a, b2),
            (CmpOp::ULe, true) | (CmpOp::UGt, false) => (b2, a),
            _ => continue,
        };
        if f.ty(x) != IrTy::I64 || f.ty(len) != IrTy::I64 {
            continue;
        }
        let idx = match facts.affine(counter.i, x, 0) {
            Some((1, k)) if facts.inside(x) || x == counter.i => Idx::Off(i64::try_from(k).ok()?),
            Some((0, k)) => Idx::Konst(i64::try_from(k).ok()?),
            _ if !facts.inside(x) => Idx::Inv(x),
            _ => continue,
        };
        let len = if let Some(k) = facts.konst(len) {
            Len::Konst(i64::try_from(k).ok()?)
        } else if !facts.inside(len) {
            Len::Val(len)
        } else {
            let Some(&Op::Load(IrTy::Int(64), addr)) = facts.op(len) else {
                continue;
            };
            let home = BlockId(facts.home[len.0 as usize]);
            // The load runs on every pass, so running it once first reads what the loop would.
            let Some((base, 0, off)) = facts.address(counter.i, addr, 0) else {
                continue;
            };
            if calls || !stores_ok || !dom.dominates(home, counter.latch) {
                continue;
            }
            Len::Load(base, i64::try_from(off).ok()?)
        };
        checks.push(Check {
            block: BlockId(bi as u32),
            idx,
            len,
        });
    }
    if checks.is_empty() {
        return None;
    }
    let loads = checks.iter().any(|c| matches!(c.len, Len::Load(..)));
    Some(Plan {
        counter,
        checks,
        stores,
        loads,
    })
}

fn counter(x: &Facts, h: BlockId) -> Option<Counter> {
    let f = x.f;
    let hb = &f.blocks[h.0 as usize];
    let Some(Term::Branch(c, yes, no)) = &hb.term else {
        return None;
    };
    if !x.lp.contains(yes.block) || x.lp.contains(no.block) || yes.block == h {
        return None;
    }
    let Some(&Op::Cmp(CmpOp::Lt, i, n)) = x.op(*c) else {
        return None;
    };
    if x.home[c.0 as usize] != h.0 || f.ty(i) != IrTy::I64 || f.ty(n) != IrTy::I64 || x.inside(n) {
        return None;
    }
    let ix = hb.params.iter().position(|p| *p == i)?;
    let preds = f.predecessors();
    let latches: Vec<BlockId> = preds[h.0 as usize]
        .iter()
        .copied()
        .filter(|p| x.lp.contains(*p))
        .collect();
    let [latch] = latches.as_slice() else {
        return None;
    };
    let Some(Term::Jump(back)) = &f.blocks[latch.0 as usize].term else {
        return None;
    };
    let step = match x.op(back.args[ix])? {
        Op::Bin(BinOp::Add, a, k) if *a == i => x.konst(*k)?,
        Op::Bin(BinOp::Add, k, a) if *a == i => x.konst(*k)?,
        _ => return None,
    };
    if !(1..=MAX_STEP).contains(&step) {
        return None;
    }
    Some(Counter {
        i,
        ix,
        n,
        exit: no.block,
        latch: *latch,
    })
}

// ---- The rewrite ---------------------------------------------------------------------------------------

/// A block that tests conditions one after the other, each failing into the checked copy.
struct Guard<'a> {
    f: &'a mut Func,
    cur: BlockId,
    slow: BlockId,
    args: Vec<Value>,
}

impl Guard<'_> {
    fn emit(&mut self, op: Op, ty: IrTy) -> Value {
        self.f
            .emit(self.cur, op, Some(ty))
            .expect("an operation makes a value")
    }

    fn k(&mut self, v: i128) -> Value {
        self.emit(Op::Const(IrTy::I64, Const::Int(v)), IrTy::I64)
    }

    fn bin(&mut self, op: BinOp, a: Value, b: Value) -> Value {
        self.emit(Op::Bin(op, a, b), IrTy::I64)
    }

    fn cmp(&mut self, op: CmpOp, a: Value, b: Value) -> Value {
        self.emit(Op::Cmp(op, a, b), IrTy::Bool)
    }

    /// `base + off` as an address.
    fn at(&mut self, base: Value, off: i64) -> Value {
        if off == 0 {
            return base;
        }
        let o = self.k(off as i128);
        self.emit(Op::PtrAdd(base, o), IrTy::Ptr)
    }

    fn int(&mut self, p: Value) -> Value {
        self.emit(Op::Cast(CastKind::PtrToInt, p, IrTy::I64), IrTy::I64)
    }

    /// Goes on only when `c` holds.
    fn require(&mut self, c: Value) {
        let next = self.f.new_block(&[]);
        self.f.terminate(
            self.cur,
            Term::Branch(
                c,
                Target {
                    block: next,
                    args: vec![],
                },
                Target {
                    block: self.slow,
                    args: self.args.clone(),
                },
            ),
        );
        self.cur = next;
    }
}

fn apply(f: &mut Func, lp: &Loop, plan: Plan) {
    let h = lp.header;
    let n_old = f.blocks.len();
    let mut body = lp.body.clone();
    let pre = preheader(f, lp);
    body.resize(f.blocks.len(), false);
    let Some(Term::Jump(entry)) = f.blocks[pre.0 as usize].term.clone() else {
        return;
    };
    let args = entry.args;
    let Counter { i, ix, n, exit, .. } = plan.counter;
    let start = args[ix];
    let s_blocks: Vec<BlockId> = (0..f.blocks.len())
        .filter(|b| body[*b])
        .map(|b| BlockId(b as u32))
        .collect();

    // ---- the checked copy
    let mut bmap: HashMap<BlockId, BlockId> = HashMap::new();
    let mut vmap: HashMap<Value, Value> = HashMap::new();
    for &b in &s_blocks {
        let tys: Vec<IrTy> = f.blocks[b.0 as usize]
            .params
            .iter()
            .map(|p| f.ty(*p))
            .collect();
        let nb = f.new_block(&tys);
        let old = f.blocks[b.0 as usize].params.clone();
        let new = f.blocks[nb.0 as usize].params.clone();
        for (o, n) in old.into_iter().zip(new) {
            vmap.insert(o, n);
        }
        f.blocks[nb.0 as usize].slow = true;
        bmap.insert(b, nb);
    }
    for &b in &s_blocks {
        for &ii in &f.blocks[b.0 as usize].insts.clone() {
            if let Some(r) = f.insts[ii].result {
                let nv = f.new_value(f.ty(r));
                vmap.insert(r, nv);
            }
        }
    }
    let mapped = |v: Value, vmap: &HashMap<Value, Value>| *vmap.get(&v).unwrap_or(&v);
    for &b in &s_blocks {
        let nb = bmap[&b];
        for &ii in &f.blocks[b.0 as usize].insts.clone() {
            let mut inst = f.insts[ii].clone();
            inst.op.map_values(|v| mapped(v, &vmap));
            inst.result = inst.result.map(|r| vmap[&r]);
            f.insts.push(inst);
            let id = f.insts.len() - 1;
            f.blocks[nb.0 as usize].insts.push(id);
        }
        let mut term = f.blocks[b.0 as usize].term.clone();
        if let Some(t) = &mut term {
            t.map_values(|v| mapped(v, &vmap));
            for tg in t.targets_mut() {
                if let Some(c) = bmap.get(&tg.block) {
                    tg.block = *c;
                }
            }
        }
        f.blocks[nb.0 as usize].term = term;
    }
    let h2 = bmap[&h];
    f.blocks[h2.0 as usize].tail = true;

    // ---- one join block for what the code after the loop reads
    let Some(Term::Branch(_, _, no)) = f.blocks[h.0 as usize].term.clone() else {
        return;
    };
    let mut live: Vec<Value> = Vec::new();
    let note = |v: Value, live: &mut Vec<Value>| {
        if vmap.contains_key(&v) && !live.contains(&v) {
            live.push(v);
        }
    };
    for (bi, b) in f.blocks.iter().enumerate().take(n_old) {
        if body[bi] {
            continue;
        }
        for &ii in &b.insts {
            f.insts[ii].op.for_each_value(|v| note(v, &mut live));
        }
        if let Some(t) = &b.term {
            t.for_each_value(|v| note(v, &mut live));
        }
    }
    no.args.iter().for_each(|v| note(*v, &mut live));
    let tys: Vec<IrTy> = live.iter().map(|v| f.ty(*v)).collect();
    let join = f.new_block(&tys);
    let jp: HashMap<Value, Value> = live
        .iter()
        .copied()
        .zip(f.blocks[join.0 as usize].params.clone())
        .collect();
    for (bi, b) in f.blocks.iter().enumerate().take(n_old) {
        if body[bi] {
            continue;
        }
        let ids = b.insts.clone();
        for ii in ids {
            f.insts[ii].op.map_values(|v| *jp.get(&v).unwrap_or(&v));
        }
    }
    for bi in 0..n_old {
        if body[bi] {
            continue;
        }
        if let Some(t) = &mut f.blocks[bi].term {
            t.map_values(|v| *jp.get(&v).unwrap_or(&v));
        }
    }
    let onward: Vec<Value> = no.args.iter().map(|v| *jp.get(v).unwrap_or(v)).collect();
    f.terminate(
        join,
        Term::Jump(Target {
            block: exit,
            args: onward,
        }),
    );
    for (blk, map) in [(h, None), (h2, Some(&vmap))] {
        if let Some(Term::Branch(_, _, e)) = &mut f.blocks[blk.0 as usize].term {
            e.block = join;
            e.args = live.iter().map(|v| map.map_or(*v, |m| m[v])).collect();
        }
    }

    // ---- the checks of the fast copy are gone
    for c in &plan.checks {
        let Some(Term::Branch(_, t, e)) = f.blocks[c.block.0 as usize].term.clone() else {
            continue;
        };
        let go = if is_stop(f, t.block, Some(MSG)) && t.args.is_empty() {
            e
        } else {
            t
        };
        f.blocks[c.block.0 as usize].term = Some(Term::Jump(go));
    }

    // ---- the guard, in front of both
    let mut g = Guard {
        f: &mut *f,
        cur: pre,
        slow: h2,
        args: args.clone(),
    };
    let run = g.cmp(CmpOp::Lt, start, n);
    g.require(run);
    let most = g.k(MAX_TRIPS);
    let short = g.cmp(CmpOp::Le, n, most);
    g.require(short);
    let least = g.k(-MAX_COEF);
    let late = g.cmp(CmpOp::Ge, start, least);
    g.require(late);
    let mut done: HashSet<(Idx, Len)> = HashSet::new();
    let mut loaded: HashMap<(Value, i64), Value> = HashMap::new();
    for c in &plan.checks {
        if !done.insert((c.idx, c.len)) {
            continue;
        }
        let len = match c.len {
            Len::Konst(k) => g.k(k as i128),
            Len::Val(v) => v,
            Len::Load(base, off) => match loaded.get(&(base, off)) {
                Some(v) => *v,
                None => {
                    let a = g.at(base, off);
                    let v = g.emit(Op::Load(IrTy::I64, a), IrTy::I64);
                    loaded.insert((base, off), v);
                    v
                }
            },
        };
        let x = match c.idx {
            Idx::Off(k) => {
                // The first index is `start + k` and the last is below `n + k`.
                let kk = g.k(-(k as i128));
                let from = g.cmp(CmpOp::Ge, start, kk);
                g.require(from);
                let last = g.k(k as i128 - 1);
                g.bin(BinOp::Add, n, last)
            }
            Idx::Konst(k) => g.k(k as i128),
            Idx::Inv(v) => v,
        };
        let ok = g.cmp(CmpOp::ULt, x, len);
        g.require(ok);
    }
    if plan.loads {
        let last = g.k(-1);
        let top = g.bin(BinOp::Add, n, last);
        let mut cache: HashMap<Value, Value> = HashMap::new();
        for (base, off) in loaded.keys().copied().collect::<Vec<_>>() {
            let a = g.at(base, off);
            let a = g.int(a);
            let a_end = {
                let eight = g.k(8);
                g.bin(BinOp::Add, a, eight)
            };
            for st in &plan.stores {
                let b = *cache.entry(st.base).or_insert_with(|| {
                    let p = st.base;
                    g.int(p)
                });
                // `lo = b + c * start + d`, `hi = b + c * (n - 1) + d + size`.
                let c = g.k(st.c as i128);
                let d = g.k(st.d as i128);
                let from = g.bin(BinOp::Mul, c, start);
                let lo = g.bin(BinOp::Add, b, from);
                let lo = g.bin(BinOp::Add, lo, d);
                let to = g.bin(BinOp::Mul, c, top);
                let hi = g.bin(BinOp::Add, b, to);
                let w = g.k((st.d + st.size) as i128);
                let hi = g.bin(BinOp::Add, hi, w);
                // The stores end before the length or start after it.
                let before = g.cmp(CmpOp::ULe, hi, a);
                let after = g.cmp(CmpOp::ULe, a_end, lo);
                let yes = g.emit(Op::Const(IrTy::Bool, Const::Bool(true)), IrTy::Bool);
                let either = g.emit(Op::Select(before, yes, after), IrTy::Bool);
                g.require(either);
            }
        }
    }
    let cur = g.cur;
    f.terminate(
        cur,
        Term::Jump(Target {
            block: h,
            args: args.clone(),
        }),
    );
    let _ = i;
}
