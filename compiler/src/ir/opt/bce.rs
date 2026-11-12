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
use std::cell::Cell;
use std::collections::{HashMap, HashSet};

thread_local! {
    /// The function that makes room in a list (`rtGrowSlow`), while a module is optimised.
    static GROW: Cell<Option<u32>> = const { Cell::new(None) };
}

/// Tells the pass which function of the module grows a list (none: push loops are left alone).
pub fn set_grow(m: Option<&Module>) {
    let id = m.and_then(|m| {
        m.funcs
            .iter()
            .position(|f| f.name == "rtGrowSlow" && f.params.len() == 3)
            .map(|i| i as u32)
    });
    GROW.with(|g| g.set(id));
}

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
/// A loop that only pushes (and indexes) may be bigger: its fast copy loses the growth code.
const MAX_PUSH_BODY: usize = 400;
/// A push loop is pre-grown to at most this many bytes.
const MAX_PREGROW: i128 = 1 << 31;
/// Pushes one pass makes, at most.
const MAX_PER_TURN: i128 = 64;
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
    step: i128,
    /// The loop runs while `i < n + off`: `i < n` has 0, `i + 4 <= n` has -3.
    off: i128,
    /// The instruction that adds the step, when it is a checked add (the fast copy wraps).
    step_inst: Option<usize>,
}

pub fn run(f: &mut Func) -> bool {
    let grow = GROW.with(|g| g.get());
    let pushes = grow.is_some_and(|g| {
        f.blocks.iter().any(|b| {
            !b.slow
                && b.insts
                    .iter()
                    .any(|&ii| matches!(&f.insts[ii].op, Op::Call(c, _) if c.0 == g))
        })
    });
    if !pushes && !f.blocks.iter().any(|b| !b.slow && is_check(f, b)) {
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
            Some(Term::Stop(m, _)) => msg.is_none_or(|x| x == m),
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
    /// The loop pushes to one list: the guard grows it once, for every pass.
    push: Option<Push>,
    /// Checked operations on the counter that the guard makes safe: the fast copy wraps.
    wrap: Vec<usize>,
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

    /// A constant that `v` never exceeds (and is never below zero): `x & k`, a widened byte.
    fn bound(&self, v: Value) -> Option<i64> {
        let m = match self.op(v)? {
            Op::Bin(BinOp::And, a, b) => match (self.konst(*a), self.konst(*b)) {
                (Some(k), _) | (_, Some(k)) if (0..1 << 40).contains(&k) => k,
                _ => return None,
            },
            Op::Cast(CastKind::ZExt, x, _) => match self.f.ty(*x) {
                IrTy::Int(8) => 255,
                IrTy::Int(16) => 65535,
                _ => return None,
            },
            _ => return None,
        };
        i64::try_from(m).ok()
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
    if size > MAX_PUSH_BODY {
        return None;
    }
    let facts = Facts {
        f,
        lp,
        defs: f.defs(),
        home,
    };
    let counter = counter(&facts, h)?;
    let push = push_plan(&facts, dom, &counter);
    if push.is_none() && size > MAX_BODY {
        return None;
    }
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
        if !lp.body[bi] || push.is_some() {
            continue;
        }
        for &ii in &b.insts {
            match &f.insts[ii].op {
                Op::Call(..)
                | Op::CallPtr(..)
                | Op::MemCopy(..)
                | Op::Alloc(_)
                | Op::Realloc(..)
                | Op::Os(..)
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
            // A masked or widened byte stays below a constant: the test is `bound <u len`.
            _ => match facts.bound(x) {
                Some(m) => Idx::Konst(m),
                None => continue,
            },
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
    if checks.is_empty() && push.is_none() {
        return None;
    }
    let loads = checks.iter().any(|c| matches!(c.len, Len::Load(..)));
    // `i + 2` cannot overflow when the trip count is at most 2^32 and the start is not far below zero.
    let mut wrap: Vec<usize> = counter.step_inst.into_iter().collect();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !lp.body[bi] || b.slow {
            continue;
        }
        for &ii in &b.insts {
            if let (Op::Checked(BinOp::Add | BinOp::Sub | BinOp::Mul, true, ..), Some(r)) =
                (&f.insts[ii].op, f.insts[ii].result)
            {
                if facts.affine(counter.i, r, 0).is_some() && !wrap.contains(&ii) {
                    wrap.push(ii);
                }
            }
        }
    }
    Some(Plan {
        counter,
        checks,
        stores,
        loads,
        push,
        wrap,
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
    let Some(&Op::Cmp(op, a, n)) = x.op(*c) else {
        return None;
    };
    // The counter, or the counter plus a constant (made in the header).
    let (i, k) = match x.op(a) {
        Some(Op::Bin(BinOp::Add, i, k) | Op::Checked(BinOp::Add, true, i, k))
            if x.home[a.0 as usize] == h.0 =>
        {
            (*i, x.konst(*k).filter(|k| (0..=MAX_STEP).contains(k))?)
        }
        _ => (a, 0),
    };
    let off = match op {
        CmpOp::Lt => -k,
        CmpOp::Le => 1 - k,
        _ => return None,
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
    let (step, step_inst) = match x.op(back.args[ix])? {
        Op::Bin(BinOp::Add, a, k) if *a == i => (x.konst(*k)?, None),
        Op::Bin(BinOp::Add, k, a) if *a == i => (x.konst(*k)?, None),
        Op::Checked(BinOp::Add, true, a, k) if *a == i => {
            (x.konst(*k)?, x.defs[back.args[ix].0 as usize])
        }
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
        step,
        off,
        step_inst,
    })
}

// ---- Loops that push -----------------------------------------------------------------------------------

/// A load of the same address on every pass.
struct Hoist {
    inst: usize,
    result: Value,
    ty: IrTy,
    base: Value,
    off: i64,
}

/// A loop whose only calls are `rtGrowSlow`, behind a capacity test, and whose stores are pushes to
/// one list (`out.push!(x)` four times a pass, say). Its length, capacity and data pointer live in
/// the header parameters of the loop. The guard grows the list once, to the length it has after every
/// pass (the capacity is not observable), so the fast copy has no capacity tests and no calls, and
/// nothing in it can reach the words it reads from other lists: those loads happen once, in the guard.
/// The checked copy is the loop as it was.
struct Push {
    /// The slot that holds the list header while `rtGrowSlow` runs.
    h: Value,
    /// Bytes per element.
    size: i64,
    /// Elements pushed per pass, at most.
    per_turn: i64,
    len_ix: usize,
    cap_ix: usize,
    data_ix: usize,
    /// The blocks that test the capacity, and whether the test goes on at its first target.
    sites: Vec<(BlockId, bool)>,
    /// Checked adds on the length: they cannot overflow once the guard has grown the list.
    plain: Vec<usize>,
    hoist: Vec<Hoist>,
}

struct Org<'a, 'b> {
    x: &'a Facts<'b>,
    preds: &'a [Vec<BlockId>],
    slow: &'a HashSet<BlockId>,
}

fn targets(t: &Term) -> Vec<&Target> {
    match t {
        Term::Jump(a) => vec![a],
        Term::Branch(_, a, b) => vec![a, b],
        _ => Vec::new(),
    }
}

impl Org<'_, '_> {
    /// The header parameter that `v` is, going back through blocks of the loop that have one way in
    /// (the blocks that grow the list do not count).
    fn origin(&self, mut v: Value) -> Option<usize> {
        let x = self.x;
        let f = x.f;
        for _ in 0..16 {
            if let Some(ix) = f.blocks[x.lp.header.0 as usize]
                .params
                .iter()
                .position(|p| *p == v)
            {
                return Some(ix);
            }
            let bi = x.home[v.0 as usize];
            if bi == u32::MAX || !x.lp.body[bi as usize] || self.slow.contains(&BlockId(bi)) {
                return None;
            }
            let ix = f.blocks[bi as usize].params.iter().position(|p| *p == v)?;
            let mut seen: HashSet<BlockId> = HashSet::new();
            let mut next = None;
            for &p in &self.preds[bi as usize] {
                if self.slow.contains(&p) || !seen.insert(p) {
                    continue;
                }
                if !x.lp.contains(p) {
                    return None;
                }
                for t in targets(f.blocks[p.0 as usize].term.as_ref()?) {
                    if t.block.0 == bi {
                        if next.is_some() {
                            return None;
                        }
                        next = Some(t.args[ix]);
                    }
                }
            }
            v = next?;
        }
        None
    }

    /// `v` as `header parameter + k`, where the adds are the ones a push makes.
    fn plus(&self, v: Value, plain: &mut Vec<usize>, depth: u32) -> Option<(usize, i128)> {
        let x = self.x;
        if depth > 70 {
            return None;
        }
        let step = |a: Value, k: Value, plain: &mut Vec<usize>, inst: Option<usize>| {
            let k = x.konst(k)?;
            if !(0..=MAX_PER_TURN).contains(&k) {
                return None;
            }
            let (ix, d) = self.plus(a, plain, depth + 1)?;
            if let Some(i) = inst {
                plain.push(i);
            }
            Some((ix, d + k))
        };
        match x.op(v) {
            Some(Op::Checked(BinOp::Add, true, a, k)) => step(*a, *k, plain, x.defs[v.0 as usize]),
            Some(Op::Bin(BinOp::Add, a, k)) => step(*a, *k, plain, None),
            Some(_) => None,
            None => self.origin(v).map(|ix| (ix, 0)),
        }
    }

    /// `v` as `data + c * length + d`, for the address of a push: (data parameter, c, d).
    fn address(&self, v: Value, len_ix: usize, depth: u32) -> Option<(Option<usize>, i128, i128)> {
        let x = self.x;
        if depth > 8 {
            return None;
        }
        if let Some(k) = x.konst(v) {
            return Some((None, 0, k));
        }
        if let Some((ix, d)) = self.plus(v, &mut Vec::new(), 0) {
            if ix == len_ix {
                return Some((None, 1, d));
            }
        }
        let go = |a: Value| self.address(a, len_ix, depth + 1);
        match x.op(v) {
            Some(Op::PtrAdd(p, o))
            | Some(Op::Bin(BinOp::Add, p, o))
            | Some(Op::Checked(BinOp::Add, true, p, o)) => {
                let (b, c, d) = go(*p)?;
                let (b2, c2, d2) = go(*o)?;
                match (b, b2) {
                    (b, None) => Some((b, c + c2, d + d2)),
                    (None, b) => Some((b, c + c2, d + d2)),
                    _ => None,
                }
            }
            Some(Op::Bin(BinOp::Sub, a, o)) | Some(Op::Checked(BinOp::Sub, true, a, o)) => {
                let (b, c, d) = go(*a)?;
                let (b2, c2, d2) = go(*o)?;
                b2.is_none().then_some((b, c - c2, d - d2))
            }
            Some(Op::Bin(BinOp::Mul, a, o)) | Some(Op::Checked(BinOp::Mul, true, a, o)) => {
                let (b, c, d) = go(*a)?;
                let (b2, c2, d2) = go(*o)?;
                if b.is_some() || b2.is_some() {
                    return None;
                }
                if c2 == 0 {
                    Some((None, c * d2, d * d2))
                } else if c == 0 {
                    Some((None, c2 * d, d2 * d))
                } else {
                    None
                }
            }
            Some(_) => None,
            None => (x.f.ty(v) == IrTy::Ptr)
                .then(|| self.origin(v))
                .flatten()
                .map(|ix| (Some(ix), 0, 0)),
        }
    }
}

fn push_plan(x: &Facts, dom: &Dom, counter: &Counter) -> Option<Push> {
    let grow = GROW.with(|g| g.get())?;
    let f = x.f;
    let lp = x.lp;
    let preds = f.predecessors();
    let grows = |b: BlockId| {
        f.blocks[b.0 as usize]
            .insts
            .iter()
            .any(|&ii| matches!(&f.insts[ii].op, Op::Call(c, _) if c.0 == grow))
    };
    // The capacity tests: a branch to a block that grows the list, and to the join that block ends in.
    struct Site {
        block: BlockId,
        ok_then: bool,
        cap: Value,
        need: Value,
        ok: Vec<Value>,
    }
    let mut slow: HashSet<BlockId> = HashSet::new();
    let mut sites: Vec<Site> = Vec::new();
    let mut h: Option<Value> = None;
    let mut size: Option<i128> = None;
    for (bi, b) in f.blocks.iter().enumerate() {
        if !lp.body[bi] || b.slow {
            continue;
        }
        let Some(Term::Branch(c, t, e)) = &b.term else {
            continue;
        };
        let (slow_then, s, ok) = match (
            t.args.is_empty() && lp.contains(t.block) && grows(t.block),
            e.args.is_empty() && lp.contains(e.block) && grows(e.block),
        ) {
            (true, false) => (true, t, e),
            (false, true) => (false, e, t),
            _ => continue,
        };
        let sb = &f.blocks[s.block.0 as usize];
        let one_way = preds[s.block.0 as usize].iter().all(|p| p.0 as usize == bi);
        let Some(Term::Jump(j)) = &sb.term else {
            return None;
        };
        if !sb.params.is_empty() || !one_way || j.block != ok.block || j.args.len() != ok.args.len()
        {
            return None;
        }
        let &Op::Cmp(op, a, b2) = x.op(*c)? else {
            return None;
        };
        let (cap, need) = match (op, slow_then) {
            (CmpOp::Lt, true) | (CmpOp::Ge, false) => (a, b2),
            (CmpOp::Gt, true) | (CmpOp::Le, false) => (b2, a),
            _ => return None,
        };
        // The call: `rtGrowSlow(h, size, need)`, with `h` a slot made before the loop.
        let mut call = None;
        for &ii in &sb.insts {
            if let Op::Call(cf, args) = &f.insts[ii].op {
                if cf.0 != grow || call.is_some() || args.len() != 3 {
                    return None;
                }
                call = Some(args.clone());
            }
        }
        let args = call?;
        if !matches!(x.op(args[0]), Some(Op::Slot(..))) || x.inside(args[0]) || args[2] != need {
            return None;
        }
        let sz = x.konst(args[1])?;
        if !(1..=1 << 20).contains(&sz)
            || h.is_some_and(|v| v != args[0])
            || size.is_some_and(|v| v != sz)
        {
            return None;
        }
        h = Some(args[0]);
        size = Some(sz);
        slow.insert(s.block);
        sites.push(Site {
            block: BlockId(bi as u32),
            ok_then: !slow_then,
            cap,
            need,
            ok: ok.args.clone(),
        });
    }
    let (h, size) = (h?, size?);
    // Nothing else of the loop touches the header, calls out, or allocates.
    let mut hd: HashSet<Value> = HashSet::from([h]);
    loop {
        let mut grew = false;
        for inst in &f.insts {
            if let (Op::PtrAdd(p, _), Some(r)) = (&inst.op, inst.result) {
                if hd.contains(p) && hd.insert(r) {
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    let mut stores: Vec<(Value, IrTy)> = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !lp.body[bi] || slow.contains(&BlockId(bi as u32)) {
            continue;
        }
        for &ii in &b.insts {
            let op = &f.insts[ii].op;
            match op {
                Op::Call(..)
                | Op::CallPtr(..)
                | Op::MemCopy(..)
                | Op::Alloc(_)
                | Op::Realloc(..)
                | Op::Free(_)
                | Op::Print(..)
                | Op::CtxInit(..)
                | Op::CtxSwitch(..) => return None,
                Op::Store(a, v) => stores.push((*a, f.ty(*v))),
                _ => {}
            }
            let mut bad = false;
            op.for_each_value(|v| bad |= hd.contains(&v));
            if bad {
                return None;
            }
        }
        let mut bad = false;
        if let Some(t) = &b.term {
            t.for_each_value(|v| bad |= hd.contains(&v));
        }
        if bad {
            return None;
        }
    }
    let org = Org {
        x,
        preds: &preds,
        slow: &slow,
    };
    // Which header parameters are the length, the capacity and the data pointer.
    let mut plain: Vec<usize> = Vec::new();
    let mut ids: Option<(usize, usize, usize)> = None;
    let mut ks: Vec<i128> = Vec::new();
    for s in &sites {
        let cap_ix = org.origin(s.cap)?;
        let (len_ix, k) = org.plus(s.need, &mut plain, 0)?;
        let mut data_ix = None;
        let mut cap_arg = None;
        for a in &s.ok {
            match x.f.ty(*a) {
                IrTy::Ptr => data_ix = Some(org.origin(*a)?),
                IrTy::Int(64) => cap_arg = Some(org.origin(*a)?),
                _ => return None,
            }
        }
        if cap_arg? != cap_ix || k < 1 {
            return None;
        }
        let here = (len_ix, cap_ix, data_ix?);
        if ids.is_some_and(|v| v != here) {
            return None;
        }
        ids = Some(here);
        ks.push(k);
    }
    let (len_ix, cap_ix, data_ix) = ids?;
    if len_ix == counter.ix || cap_ix == counter.ix || data_ix == counter.ix {
        return None;
    }
    let hb = &f.blocks[lp.header.0 as usize];
    if f.ty(hb.params[len_ix]) != IrTy::I64
        || f.ty(hb.params[cap_ix]) != IrTy::I64
        || f.ty(hb.params[data_ix]) != IrTy::Ptr
    {
        return None;
    }
    // One way round: the length grows by a known number of pushes, the rest passes through.
    let Some(Term::Jump(back)) = &f.blocks[counter.latch.0 as usize].term else {
        return None;
    };
    let (ix, per_turn) = org.plus(back.args[len_ix], &mut plain, 0)?;
    if ix != len_ix || per_turn < 1 || ks.iter().any(|k| *k > per_turn) {
        return None;
    }
    if org.origin(back.args[cap_ix])? != cap_ix || org.origin(back.args[data_ix])? != data_ix {
        return None;
    }
    // Every other header parameter may not depend on the list.
    // Stores lie in the room the guard makes: `data + size * length + d`, `d + width <= size * per_turn`.
    for (a, ty) in stores {
        let (base, c, d) = org.address(a, len_ix, 0)?;
        let w = ty.size() as i128;
        if base != Some(data_ix) || c != size || d < 0 || d + w > size * per_turn {
            return None;
        }
    }
    // Loads at one address on every pass.
    let mut hoist = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        if !lp.body[bi] || slow.contains(&BlockId(bi as u32)) {
            continue;
        }
        for &ii in &b.insts {
            let inst = &f.insts[ii];
            let (Op::Load(ty, addr), Some(result)) = (&inst.op, inst.result) else {
                continue;
            };
            if !dom.dominates(BlockId(bi as u32), counter.latch) {
                continue;
            }
            if let Some((mut base, 0, mut off)) = x.address(counter.i, *addr, 0) {
                // The same word reached through different `ptradd`s is one word.
                while let Some(&Op::PtrAdd(p, k)) = x.op(base) {
                    let Some(k) = x.konst(k) else { break };
                    base = p;
                    off += k;
                }
                if let Ok(off) = i64::try_from(off) {
                    hoist.push(Hoist {
                        inst: ii,
                        result,
                        ty: *ty,
                        base,
                        off,
                    });
                }
            }
        }
    }
    plain.sort_unstable();
    plain.dedup();
    Some(Push {
        h,
        size: size as i64,
        per_turn: per_turn as i64,
        len_ix,
        cap_ix,
        data_ix,
        sites: sites.iter().map(|s| (s.block, s.ok_then)).collect(),
        plain,
        hoist,
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
    let Counter {
        i,
        ix,
        n,
        exit,
        step,
        off,
        ..
    } = plan.counter;
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

    // ---- wrapped adds; the capacity tests of a push loop are gone too
    let mut wrap = plan.wrap.clone();
    if let Some(p) = &plan.push {
        wrap.extend(p.plain.iter().copied());
        for &(blk, ok_then) in &p.sites {
            if let Some(Term::Branch(_, t, e)) = f.blocks[blk.0 as usize].term.clone() {
                let (go, grows) = if ok_then { (t, e) } else { (e, t) };
                f.blocks[blk.0 as usize].term = Some(Term::Jump(go));
                // The block that grew the list can no longer run; it must not read what the loop makes.
                let dead = &mut f.blocks[grows.block.0 as usize];
                dead.insts.clear();
                dead.term = Some(Term::Unreachable);
            }
        }
    }
    for ii in wrap {
        if let Op::Checked(o, _, a, b) = f.insts[ii].op.clone() {
            f.insts[ii].op = Op::Bin(o, a, b);
        }
    }

    // ---- the guard, in front of both
    let mut g = Guard {
        f: &mut *f,
        cur: pre,
        slow: h2,
        args: args.clone(),
    };
    // The loop runs while `i < n`; the test may have asked for `i + 4 <= n`.
    let n = if off == 0 {
        n
    } else {
        let k = g.k(off);
        g.bin(BinOp::Add, n, k)
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
    if plan.loads && plan.push.is_none() {
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
    let mut subst: Vec<(Value, Value)> = Vec::new();
    if let Some(p) = &plan.push {
        let grow = GROW
            .with(|g| g.get())
            .expect("a push plan needs the grow function");
        let (l0, c0, d0) = (args[p.len_ix], args[p.cap_ix], args[p.data_ix]);
        // Room for every push of every pass.
        let lim = g.k(1 << 40);
        let small = g.cmp(CmpOp::ULt, l0, lim);
        g.require(small);
        let span = g.bin(BinOp::Sub, n, start);
        let adj = g.k(step - 1);
        let span = g.bin(BinOp::Add, span, adj);
        let st = g.k(step);
        let trips = g.bin(BinOp::UDiv, span, st);
        let per = g.k(p.per_turn as i128);
        let more = g.bin(BinOp::Mul, trips, per);
        let need = g.bin(BinOp::Add, l0, more);
        let most = g.k(MAX_PREGROW / p.size as i128);
        let fits = g.cmp(CmpOp::ULe, need, most);
        g.require(fits);
        let enough = g.cmp(CmpOp::Ge, c0, need);
        let mid = g.f.new_block(&[IrTy::I64, IrTy::Ptr]);
        let grows = g.f.new_block(&[]);
        let here = g.cur;
        g.f.terminate(
            here,
            Term::Branch(
                enough,
                Target {
                    block: mid,
                    args: vec![c0, d0],
                },
                Target {
                    block: grows,
                    args: vec![],
                },
            ),
        );
        g.cur = grows;
        // The header as `rtGrowSlow` reads it: data, length, capacity.
        for (off, v) in [(0, d0), (8, l0), (16, c0)] {
            let a = g.at(p.h, off);
            let b = g.cur;
            g.f.emit(b, Op::Store(a, v), None);
        }
        let size = g.k(p.size as i128);
        let b = g.cur;
        g.f.emit(b, Op::Call(FuncId(grow), vec![p.h, size, need]), None);
        let a = g.at(p.h, 0);
        let dn = g.emit(Op::Load(IrTy::Ptr, a), IrTy::Ptr);
        let a = g.at(p.h, 16);
        let cn = g.emit(Op::Load(IrTy::I64, a), IrTy::I64);
        let b = g.cur;
        g.f.terminate(
            b,
            Term::Jump(Target {
                block: mid,
                args: vec![cn, dn],
            }),
        );
        g.cur = mid;
        let (cn, dn) = {
            let ps = &g.f.blocks[mid.0 as usize].params;
            (ps[0], ps[1])
        };
        g.args[p.cap_ix] = cn;
        g.args[p.data_ix] = dn;
        // The pushes write between `lo` and `hi`; the words the loop reads each pass lie elsewhere.
        let d_int = g.int(dn);
        let size = g.k(p.size as i128);
        let from = g.bin(BinOp::Mul, l0, size);
        let lo = g.bin(BinOp::Add, d_int, from);
        let to = g.bin(BinOp::Mul, need, size);
        let hi = g.bin(BinOp::Add, d_int, to);
        let mut once: HashMap<(Value, i64, IrTy), Value> = HashMap::new();
        for hz in &p.hoist {
            if let Some(v) = once.get(&(hz.base, hz.off, hz.ty)) {
                subst.push((hz.result, *v));
                continue;
            }
            let a = g.at(hz.base, hz.off);
            let a_int = g.int(a);
            let w = g.k(hz.ty.size() as i128);
            let a_end = g.bin(BinOp::Add, a_int, w);
            let before = g.cmp(CmpOp::ULe, hi, a_int);
            let after = g.cmp(CmpOp::ULe, a_end, lo);
            let yes = g.emit(Op::Const(IrTy::Bool, Const::Bool(true)), IrTy::Bool);
            let either = g.emit(Op::Select(before, yes, after), IrTy::Bool);
            g.require(either);
            let v = g.emit(Op::Load(hz.ty, a), hz.ty);
            once.insert((hz.base, hz.off, hz.ty), v);
            subst.push((hz.result, v));
        }
    }
    let cur = g.cur;
    let enter = g.args.clone();
    f.terminate(
        cur,
        Term::Jump(Target {
            block: h,
            args: enter,
        }),
    );
    // ---- the loads of the guard stand in for the loads of the fast copy
    if let Some(p) = &plan.push {
        let map: HashMap<Value, Value> = subst.into_iter().collect();
        let gone: HashSet<usize> = p.hoist.iter().map(|hz| hz.inst).collect();
        for bi in 0..n_old {
            if !body[bi] {
                continue;
            }
            f.blocks[bi].insts.retain(|ii| !gone.contains(ii));
            for ii in f.blocks[bi].insts.clone() {
                f.insts[ii].op.map_values(|v| *map.get(&v).unwrap_or(&v));
            }
            if let Some(t) = &mut f.blocks[bi].term {
                t.map_values(|v| *map.get(&v).unwrap_or(&v));
            }
        }
    }
    // The blocks that grew the list are unreachable now.
    if plan.push.is_some() {
        f.compact();
    }
    let _ = i;
}
