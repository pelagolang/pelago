//! Overflow checks that cannot fire (card A-11, steps S2 and W2). A forward range analysis finds
//! values that are known to be small non-negative numbers (a widened byte, `x & 255`, `x % 10`,
//! `x >> 3`, a sum or product of such values, a loop counter); a checked `add`, `sub` or `mul` whose
//! result provably fits becomes the plain operation, so the native code does not test the overflow
//! flag.
//!
//! Ranges are `(lo, hi)` with `0 <= lo <= hi <= MAX` of the signed type, so the same range is true
//! for a signed and for an unsigned reading of the bits. A value that may be negative has no range.
//!
//! Loops: block parameters get the join of what the edges pass in, repeated until nothing changes,
//! with widening (after a few growing steps the bounds jump to `0` and `MAX`) so it ends. The
//! widened bound is then cut back wherever a comparison decides it: a block whose only way in is the
//! true or false side of `branch (lt i, n)` knows `i < n` for everything it dominates (`dom.rs`),
//! so `i + 1` in `while i < n { ... i += 1 }` cannot pass `MAX`. A checked operation that does not
//! trap gives a result inside the type, which is what lets a counter that grows by one stay known.

use super::dom::Dom;
use crate::ir::*;

type Range = (u128, u128);

/// What is known about a value so far: not looked at yet, a range, or nothing.
#[derive(Clone, Copy, PartialEq, Debug)]
enum L {
    Bot,
    R(Range),
    Top,
}

/// Two values compared; `a` is the left operand. The unsigned kinds start with `U`.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Lt,
    Le,
    Gt,
    Ge,
    ULt,
    ULe,
    UGt,
    UGe,
    Eq,
}

impl Kind {
    fn of(op: CmpOp, taken: bool) -> Option<Kind> {
        let op = if taken {
            op
        } else {
            match op {
                CmpOp::Lt => CmpOp::Ge,
                CmpOp::Le => CmpOp::Gt,
                CmpOp::Gt => CmpOp::Le,
                CmpOp::Ge => CmpOp::Lt,
                CmpOp::ULt => CmpOp::UGe,
                CmpOp::ULe => CmpOp::UGt,
                CmpOp::UGt => CmpOp::ULe,
                CmpOp::UGe => CmpOp::ULt,
                CmpOp::Ne => CmpOp::Eq,
                CmpOp::Eq => return None,
            }
        };
        Some(match op {
            CmpOp::Lt => Kind::Lt,
            CmpOp::Le => Kind::Le,
            CmpOp::Gt => Kind::Gt,
            CmpOp::Ge => Kind::Ge,
            CmpOp::ULt => Kind::ULt,
            CmpOp::ULe => Kind::ULe,
            CmpOp::UGt => Kind::UGt,
            CmpOp::UGe => Kind::UGe,
            CmpOp::Eq => Kind::Eq,
            CmpOp::Ne => return None,
        })
    }

    /// The same fact read from the right operand.
    fn mirror(self) -> Kind {
        match self {
            Kind::Lt => Kind::Gt,
            Kind::Le => Kind::Ge,
            Kind::Gt => Kind::Lt,
            Kind::Ge => Kind::Le,
            Kind::ULt => Kind::UGt,
            Kind::ULe => Kind::UGe,
            Kind::UGt => Kind::ULt,
            Kind::UGe => Kind::ULe,
            Kind::Eq => Kind::Eq,
        }
    }
}

/// `a kind b` holds in every block the fact is attached to.
#[derive(Clone, Copy, Debug)]
struct Rel {
    kind: Kind,
    a: Value,
    b: Value,
}

fn bits(t: IrTy) -> u32 {
    match t {
        IrTy::Int(b) => b as u32,
        _ => 64,
    }
}

fn smax(t: IrTy) -> u128 {
    (1u128 << (bits(t) - 1)) - 1
}

/// What `x kind y` says about `x`, given the range of `y` if it has one: a lower and an upper bound,
/// either may be missing. Only bounds that hold for the signed and the unsigned reading are given.
fn bounds(kind: Kind, other: Option<Range>, max: u128) -> (Option<u128>, Option<u128>) {
    match kind {
        Kind::Lt => (None, other.map_or(max, |o| o.1).checked_sub(1)),
        Kind::Le => (None, Some(other.map_or(max, |o| o.1))),
        Kind::Gt => (other.map(|o| o.0 + 1), Some(max)),
        Kind::Ge => (other.map(|o| o.0), Some(max)),
        Kind::ULt => (Some(0), other.and_then(|o| o.1.checked_sub(1))),
        Kind::ULe => (Some(0), other.map(|o| o.1)),
        Kind::UGt => (Some(other.map_or(1, |o| o.0 + 1)), None),
        Kind::UGe => (Some(other.map_or(0, |o| o.0)), None),
        Kind::Eq => (other.map(|o| o.0), other.map(|o| o.1)),
    }
}

struct Ctx<'a> {
    f: &'a Func,
    defs: Vec<Option<usize>>,
    /// The block of every instruction in a block (`u32::MAX` for the rest).
    block_of: Vec<u32>,
    dom: Dom,
    /// The comparisons known in each block: those of every dominating single-entry edge.
    facts: Vec<Vec<Rel>>,
    /// For each block, the (predecessor, arguments) of every edge into it.
    edges: Vec<Vec<(BlockId, Vec<Value>)>>,
    val: Vec<L>,
    steps: Vec<u8>,
}

impl Ctx<'_> {
    fn new(f: &Func) -> Ctx<'_> {
        let dom = Dom::new(f);
        let defs = f.defs();
        let mut block_of = vec![u32::MAX; f.insts.len()];
        let mut edges = vec![Vec::new(); f.blocks.len()];
        for (bi, b) in f.blocks.iter().enumerate() {
            for &i in &b.insts {
                block_of[i] = bi as u32;
            }
            let from = BlockId(bi as u32);
            if !dom.reachable(from) {
                continue;
            }
            let mut add = |t: &Target| edges[t.block.0 as usize].push((from, t.args.clone()));
            match &b.term {
                Some(Term::Jump(t)) => add(t),
                Some(Term::Branch(_, a, c)) => {
                    add(a);
                    add(c);
                }
                _ => {}
            }
        }
        // A fact for every block entered only by one side of a branch on a comparison.
        let mut own: Vec<Option<Rel>> = vec![None; f.blocks.len()];
        for (bi, preds) in edges.iter().enumerate() {
            let [(p, _)] = preds.as_slice() else { continue };
            let Some(Term::Branch(c, t, e)) = &f.blocks[p.0 as usize].term else {
                continue;
            };
            let taken = match (t.block.0 as usize == bi, e.block.0 as usize == bi) {
                (true, false) => true,
                (false, true) => false,
                _ => continue,
            };
            let Some(Op::Cmp(op, a, b)) = defs[c.0 as usize].map(|d| &f.insts[d].op) else {
                continue;
            };
            if !f.ty(*a).is_int() {
                continue;
            }
            if let Some(kind) = Kind::of(*op, taken) {
                own[bi] = Some(Rel { kind, a: *a, b: *b });
            }
        }
        let mut facts: Vec<Vec<Rel>> = vec![Vec::new(); f.blocks.len()];
        for &b in &dom.rpo {
            let mut v = dom
                .idom(b)
                .map_or_else(Vec::new, |d| facts[d.0 as usize].clone());
            v.extend(own[b.0 as usize]);
            facts[b.0 as usize] = v;
        }
        let mut val = vec![L::Bot; f.values.len()];
        for &p in &f.blocks[0].params {
            val[p.0 as usize] = L::Top;
        }
        Ctx {
            f,
            defs,
            block_of,
            dom,
            facts,
            edges,
            val,
            steps: vec![0; f.values.len()],
        }
    }

    /// Cuts `cur` down by what the comparisons in force in `blk` say about `v`.
    fn range_at(&self, v: Value, blk: BlockId) -> L {
        let mut cur = self.val[v.0 as usize];
        if cur == L::Bot || !self.f.ty(v).is_int() {
            return cur;
        }
        let max = smax(self.f.ty(v));
        for rel in &self.facts[blk.0 as usize] {
            let (kind, other) = if rel.a == v {
                (rel.kind, rel.b)
            } else if rel.b == v {
                (rel.kind.mirror(), rel.a)
            } else {
                continue;
            };
            let other = match self.val[other.0 as usize] {
                L::R(r) => Some(r),
                _ => None,
            };
            let (lo, hi) = bounds(kind, other, max);
            cur = match cur {
                L::R((cl, ch)) => {
                    let (l, h) = (lo.map_or(cl, |l| l.max(cl)), hi.map_or(ch, |h| h.min(ch)));
                    if l <= h {
                        L::R((l, h))
                    } else {
                        cur
                    }
                }
                L::Top => match (lo, hi) {
                    (Some(l), Some(h)) if l <= h && h <= max => L::R((l, h)),
                    _ => cur,
                },
                L::Bot => cur,
            };
        }
        cur
    }

    fn konst(&self, v: Value) -> Option<u128> {
        let d = self.defs[v.0 as usize]?;
        match self.f.insts[d].op {
            Op::Const(t, Const::Int(n)) if n >= 0 && bits(t) <= 64 => {
                let c = n as u128 & ((1u128 << bits(t)) - 1);
                (c <= smax(t)).then_some(c)
            }
            _ => None,
        }
    }

    fn of_op(&self, v: Value, op: &Op, blk: BlockId) -> L {
        let t = self.f.ty(v);
        if !t.is_int() {
            return L::Top;
        }
        let max = smax(t);
        // The range of an operand: `None` is unknown; an operand not looked at yet ends the function.
        macro_rules! rg {
            ($x:expr) => {
                match self.range_at($x, blk) {
                    L::Bot => return L::Bot,
                    L::Top => None,
                    L::R(r) => Some(r),
                }
            };
        }
        macro_rules! some {
            ($e:expr) => {
                match $e {
                    Some(x) => x,
                    None => return L::Top,
                }
            };
        }
        let known = |r: Option<Range>| r.map_or(L::Top, L::R);
        match op {
            Op::Const(..) => known(self.konst(v).map(|c| (c, c))),
            Op::Cast(CastKind::ZExt, a, _) => {
                let fb = bits(self.f.ty(*a));
                match rg!(*a) {
                    Some(r) => L::R(r),
                    None if fb < bits(t) => L::R((0, (1u128 << fb) - 1)),
                    None => L::Top,
                }
            }
            Op::Cast(CastKind::SExt, a, _) => known(rg!(*a)),
            Op::Bin(BinOp::And, a, b) => match (rg!(*a), rg!(*b)) {
                (Some(x), Some(y)) => L::R((0, x.1.min(y.1))),
                (Some(x), None) | (None, Some(x)) => L::R((0, x.1)),
                _ => L::Top,
            },
            Op::Bin(BinOp::URem, _, b) => {
                let c = some!(self.konst(*b).filter(|c| *c > 0));
                L::R((0, c - 1))
            }
            Op::Bin(BinOp::Rem, a, b) => {
                let c = some!(self.konst(*b).filter(|c| *c > 0));
                some!(rg!(*a));
                L::R((0, c - 1))
            }
            Op::Bin(BinOp::UShr, _, k) => {
                let k = some!(self.konst(*k).filter(|k| (1..bits(t) as u128).contains(k)));
                L::R((0, (1u128 << (bits(t) - k as u32)) - 1))
            }
            Op::Bin(BinOp::Shr, a, k) => {
                let k = some!(self.konst(*k).filter(|k| *k < bits(t) as u128));
                let r = some!(rg!(*a));
                L::R((r.0 >> k, r.1 >> k))
            }
            Op::Bin(BinOp::Div | BinOp::UDiv, a, b) => {
                let c = some!(self.konst(*b).filter(|c| *c > 0));
                let r = some!(rg!(*a));
                L::R((r.0 / c, r.1 / c))
            }
            Op::Bin(o @ (BinOp::Add | BinOp::Mul | BinOp::Sub), a, b)
            | Op::Checked(o @ (BinOp::Add | BinOp::Mul | BinOp::Sub), _, a, b) => {
                let (x, y) = (rg!(*a), rg!(*b));
                let (x, y) = (some!(x), some!(y));
                let checked = matches!(op, Op::Checked(..));
                let signed = matches!(op, Op::Checked(_, true, ..));
                match o {
                    BinOp::Sub => {
                        if x.0 >= y.1 {
                            L::R((x.0 - y.1, x.1 - y.0))
                        } else if checked && !signed && x.1 >= y.0 {
                            // A negative result stops the program.
                            L::R((0, x.1 - y.0))
                        } else {
                            L::Top
                        }
                    }
                    _ => {
                        let (lo, hi) = if *o == BinOp::Add {
                            (Some(x.0 + y.0), Some(x.1 + y.1))
                        } else {
                            (x.0.checked_mul(y.0), x.1.checked_mul(y.1))
                        };
                        let lo = some!(lo);
                        match hi {
                            Some(h) if h <= max => L::R((lo, h)),
                            // A signed overflow stops the program, so the result is below `MAX`.
                            _ if checked && signed && lo <= max => L::R((lo, max)),
                            _ => L::Top,
                        }
                    }
                }
            }
            _ => L::Top,
        }
    }

    /// What the edges into a block parameter carry.
    fn incoming(&self, b: usize, k: usize) -> L {
        let mut acc = L::Bot;
        for (p, args) in &self.edges[b] {
            acc = join(acc, self.range_at(args[k], *p));
        }
        acc
    }

    /// Runs the analysis to a fixed point. False when it did not settle (then nothing may be used).
    fn solve(&mut self) -> bool {
        let rpo = self.dom.rpo.clone();
        for _ in 0..64 {
            let mut changed = false;
            for &b in &rpo {
                let bi = b.0 as usize;
                if bi != 0 {
                    for (k, &p) in self.f.blocks[bi].params.iter().enumerate() {
                        let pi = p.0 as usize;
                        let old = self.val[pi];
                        let mut new = if self.f.ty(p).is_int() {
                            join(old, self.incoming(bi, k))
                        } else {
                            L::Top
                        };
                        if new != old {
                            self.steps[pi] = self.steps[pi].saturating_add(1);
                            if self.steps[pi] > 3 {
                                new = widen(old, new, smax(self.f.ty(p)));
                            }
                            self.val[pi] = new;
                            changed = true;
                        }
                    }
                }
                for &ii in &self.f.blocks[bi].insts {
                    let Some(r) = self.f.insts[ii].result else {
                        continue;
                    };
                    let new = self.of_op(r, &self.f.insts[ii].op, b);
                    if new != self.val[r.0 as usize] {
                        self.val[r.0 as usize] = new;
                        changed = true;
                    }
                }
            }
            if !changed {
                return true;
            }
        }
        false
    }
}

fn join(a: L, b: L) -> L {
    match (a, b) {
        (L::Bot, x) | (x, L::Bot) => x,
        (L::Top, _) | (_, L::Top) => L::Top,
        (L::R(x), L::R(y)) => L::R((x.0.min(y.0), x.1.max(y.1))),
    }
}

/// A bound that moved again jumps to the end of its scale.
fn widen(old: L, new: L, max: u128) -> L {
    match (old, new) {
        (L::R(o), L::R(n)) => L::R((
            if n.0 < o.0 { 0 } else { n.0 },
            if n.1 > o.1 { max } else { n.1 },
        )),
        _ => new,
    }
}

pub fn run(f: &mut Func) -> bool {
    if !f.insts.iter().any(|i| matches!(i.op, Op::Checked(..))) {
        return false;
    }
    let mut cx = Ctx::new(f);
    if !cx.solve() {
        return false;
    }
    let mut safe = Vec::new();
    for ii in 0..f.insts.len() {
        let Op::Checked(o, signed, a, b) = f.insts[ii].op.clone() else {
            continue;
        };
        let (Some(r), blk) = (f.insts[ii].result, cx.block_of[ii]) else {
            continue;
        };
        if blk == u32::MAX {
            continue;
        }
        let t = f.ty(r);
        if !t.is_int() {
            continue;
        }
        let blk = BlockId(blk);
        let (L::R(x), L::R(y)) = (cx.range_at(a, blk), cx.range_at(b, blk)) else {
            continue;
        };
        let fits = match o {
            BinOp::Add => x.1 + y.1 <= smax(t),
            BinOp::Mul => x.1.checked_mul(y.1).is_some_and(|p| p <= smax(t)),
            // Both in [0, MAX]: the difference is in [-MAX, MAX], which a signed type holds;
            // an unsigned one needs `a >= b`.
            _ => signed || x.0 >= y.1,
        };
        if fits {
            safe.push(ii);
        }
    }
    let changed = !safe.is_empty();
    for ii in safe {
        if let Op::Checked(o, _, a, b) = f.insts[ii].op.clone() {
            f.insts[ii].op = Op::Bin(o, a, b);
        }
    }
    changed
}
