//! The loop vectoriser (card A-11, `docs/design/vectorise.md`). A counted loop whose body is plain
//! wrapping integer arithmetic over its counters and over the elements of arrays runs two iterations
//! per vector (SSE2 has two 64-bit lanes) and `U` vectors per pass, so `2 * U` iterations per pass:
//!
//! ```text
//! while i < n { p += 10; t += p & 1023; i += 1 }
//! ```
//!
//! A counter that steps by a constant `k` becomes `U` vectors holding `c, c + k` / `c + 2k, c + 3k`
//! / ..., each stepping by `2 * U * k` per pass. An accumulator (`t += e` or `t -= e`, read nowhere
//! else) becomes `U` vectors of partial sums, added up after the loop. Everything else in the body
//! is done lane by lane with the same operation. An element `xs[i]` of an array read with the test
//! counter (`load (ptradd base, i * 8)`) is a vector load of two neighbours, and a store of the same
//! shape is a vector store.
//!
//! Nothing here can change what the program does:
//!
//! - The operations are wrapping ones, so every lane computes exactly what its iteration would have,
//!   and sums are reassociated only for integers, where wrapping addition is associative.
//! - The vector loop runs while `i < n - (2 * U - 1) * step`, so every iteration it covers is one the
//!   original loop would have run; the original loop is kept and finishes the last few iterations.
//! - Memory: the loads are the ones the original loop would do a few passes later, and a loop that
//!   stores only reads and writes through one base address with the test counter as index, so a lane
//!   only ever touches its own element and the order between lanes does not matter.
//! - A *checked* sum (`t += xs[i]` stops on overflow) is vectorised only behind a guard that makes
//!   overflow impossible: the start value of the sum, the first index and the bound are in a range
//!   where `n` additions of elements below 2^29 in absolute value cannot overflow, and the vector
//!   loop ORs `element + 2^29` of every element it reads into a flag. If the flag shows an element
//!   outside that range (or the guard fails before the loop) the original loop runs from the start
//!   (a loop with checked sums has no stores, so it can run again), and stops where it always did.
//!
//! Like the unroller it takes loops of two blocks. A constant bound has to allow at least
//! [`MIN_TRIPS`] passes and a step of at most 2^20; a bound that is a value the loop does not change
//! is also accepted. The body has no calls (so a vector never lives across a call).

use super::dom::Dom;
use super::loops;
use crate::ir::*;
use std::collections::{HashMap, HashSet};

/// Vectors per pass, so iterations per pass are `2 * U`.
const U: usize = 2;
const MIN_TRIPS: i128 = 256;
/// Vectors that may be alive in the loop (the allocator has 14 registers).
const MAX_LIVE: usize = 14;
/// Elements of a checked sum stay below `2^ELEM_BITS` in absolute value, the bound below `2^TRIP_BITS`
/// and the start value below `2^ACC_BITS`: `2^61 + 2^33 * 2^29` is below `2^63`.
const ELEM_BITS: u32 = 29;
const TRIP_BITS: u32 = 33;
const ACC_BITS: u32 = 61;

/// Vectorises what it can. The original loop, which finishes the last iterations, and the vector loop
/// are marked [`Block::tail`], so that neither this pass nor the unroller takes them again.
pub fn run(f: &mut Func) -> bool {
    let dom = Dom::new(f);
    let found: Vec<(BlockId, BlockId)> = loops::find(f, &dom)
        .into_iter()
        .filter(|l| l.size == 2 && l.header.0 != 0 && !f.blocks[l.header.0 as usize].tail)
        .filter_map(|l| {
            let body = (0..l.body.len())
                .find(|b| l.body[*b] && *b as u32 != l.header.0)
                .map(|b| BlockId(b as u32))?;
            Some((l.header, body))
        })
        .collect();
    let mut did = false;
    for (h, b) in found {
        if let Some(vector_header) = vectorise(f, h, b) {
            f.blocks[h.0 as usize].tail = true;
            f.blocks[vector_header.0 as usize].tail = true;
            did = true;
        }
    }
    did
}

/// What a body value is, in one copy of the body.
#[derive(Clone, Copy)]
enum Lane {
    /// The same in every lane: a scalar value.
    S(Value),
    /// A vector.
    V(Value),
    /// `i * 8` for the test counter `i`: only ever the offset of an address.
    Idx,
    /// The address `base + i * 8`, of the element this copy starts at.
    Addr(Value),
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Steps by this constant each iteration.
    Counter(i128),
    /// `p + e` (or `p - e` when `minus`), `p` read nowhere else. `checked`: the update stops on
    /// overflow, which is what the guard makes impossible.
    Acc { minus: bool, checked: bool },
}

struct Ctx<'a> {
    f: &'a mut Func,
    pre: BlockId,
    body: BlockId,
    consts: HashMap<i128, Value>,
    splats: HashMap<Value, Value>,
    /// Scalars made in the body: their splats cannot be hoisted.
    inner: HashSet<Value>,
}

impl Ctx<'_> {
    fn konst(&mut self, n: i128) -> Value {
        if let Some(v) = self.consts.get(&n) {
            return *v;
        }
        let t = IrTy::I64;
        let v = self
            .f
            .emit(self.pre, Op::Const(t, Const::Int(n)), Some(t))
            .expect("a constant makes a value");
        self.consts.insert(n, v);
        v
    }

    fn splat(&mut self, v: Value) -> Value {
        if let Some(s) = self.splats.get(&v) {
            return *s;
        }
        let at = if self.inner.contains(&v) {
            self.body
        } else {
            self.pre
        };
        let s = self
            .f
            .emit(at, Op::VUn(VUn::Splat, v), Some(IrTy::V2I64))
            .expect("a splat makes a value");
        if at == self.pre {
            self.splats.insert(v, s);
        }
        s
    }

    /// The vector for a value; an address or an offset is not one.
    fn vec(&mut self, l: Lane) -> Option<Value> {
        match l {
            Lane::V(v) => Some(v),
            Lane::S(v) => Some(self.splat(v)),
            Lane::Idx | Lane::Addr(_) => None,
        }
    }

    fn pre_op(&mut self, op: Op, ty: IrTy) -> Value {
        self.f
            .emit(self.pre, op, Some(ty))
            .expect("an operation makes a value")
    }
}

/// The element size of the arrays this pass reads: a vector load covers two of them.
const ELEM: i128 = 8;

fn vectorise(f: &mut Func, h: BlockId, b: BlockId) -> Option<BlockId> {
    let hb = f.blocks[h.0 as usize].clone();
    let bb = f.blocks[b.0 as usize].clone();
    // The header: `c = lt i, n` and a branch into the body (the unroller's shape).
    let [ci] = hb.insts.as_slice() else {
        return None;
    };
    let c = f.insts[*ci].result?;
    let Op::Cmp(CmpOp::Lt, i, n) = f.insts[*ci].op else {
        return None;
    };
    let Some(Term::Branch(cond, yes, no)) = &hb.term else {
        return None;
    };
    if *cond != c || yes.block != b || !yes.args.is_empty() || no.block == b || no.block == h {
        return None;
    }
    let ix = hb.params.iter().position(|p| *p == i)?;
    let i64t = IrTy::I64;
    if f.ty(i) != i64t || f.ty(n) != i64t || !bb.params.is_empty() || bb.insts.is_empty() {
        return None;
    }
    if hb.params.iter().any(|p| f.ty(*p) != i64t) {
        return None;
    }
    let Some(Term::Jump(back)) = &bb.term else {
        return None;
    };
    if back.block != h {
        return None;
    }
    let back = back.clone();
    let known: HashMap<Value, i128> = f
        .insts
        .iter()
        .filter_map(|i| match (&i.op, i.result) {
            (Op::Const(IrTy::Int(64), Const::Int(c)), Some(r)) => Some((r, *c)),
            _ => None,
        })
        .collect();
    let konst = |v: Value| known.get(&v).copied();
    // Values made in the loop (the bound and the array bases must not be among them).
    let inside: HashSet<Value> = hb
        .params
        .iter()
        .copied()
        .chain(bb.insts.iter().filter_map(|&k| f.insts[k].result))
        .collect();
    let bound = konst(n);
    if bound.is_none() && inside.contains(&n) {
        return None;
    }
    // What the body reads of each value.
    let mut uses: HashMap<Value, usize> = HashMap::new();
    for &k in &bb.insts {
        f.insts[k]
            .op
            .for_each_value(|v| *uses.entry(v).or_default() += 1);
    }
    for a in &back.args {
        *uses.entry(*a).or_default() += 1;
    }
    let in_body: HashMap<Value, usize> = bb
        .insts
        .iter()
        .filter_map(|&k| f.insts[k].result.map(|r| (r, k)))
        .collect();

    // The parameters: counters and accumulators, nothing else.
    let mut kinds: Vec<Kind> = Vec::new();
    for (idx, p) in hb.params.iter().enumerate() {
        let x = back.args[idx];
        let upd = in_body.get(&x).map(|k| &f.insts[*k].op);
        let acc = |minus: bool, checked: bool| -> Option<Kind> {
            // `p` is read only by this update, and the update only by the jump back.
            (uses.get(p) == Some(&1) && uses.get(&x) == Some(&1))
                .then_some(Kind::Acc { minus, checked })
        };
        let kind = match upd? {
            Op::Bin(BinOp::Add, a, e) | Op::Bin(BinOp::Add, e, a)
                if a == p && e != p && konst(*e).is_some() =>
            {
                Kind::Counter(konst(*e)?)
            }
            Op::Bin(BinOp::Add, a, e) if a == p && e != p => acc(false, false)?,
            Op::Bin(BinOp::Add, e, a) if a == p && e != p => acc(false, false)?,
            Op::Bin(BinOp::Sub, a, e) if a == p && e != p => acc(true, false)?,
            Op::Checked(BinOp::Add, true, a, e) if a == p && e != p => acc(false, true)?,
            Op::Checked(BinOp::Add, true, e, a) if a == p && e != p => acc(false, true)?,
            Op::Checked(BinOp::Sub, true, a, e) if a == p && e != p => acc(true, true)?,
            _ => return None,
        };
        kinds.push(kind);
    }
    // The test counter steps by a positive constant, and a constant bound allows many passes.
    let Kind::Counter(step) = kinds[ix] else {
        return None;
    };
    if !(1..=1 << 20).contains(&step) || bound.is_some_and(|b| b < MIN_TRIPS * step) {
        return None;
    }
    let guarded = kinds
        .iter()
        .any(|k| matches!(k, Kind::Acc { checked: true, .. }));

    // Which body instructions are vector work, and is it worth it.
    let mut work = 0usize; // vector operations per copy
    let mut scalar_ops = 0usize; // instructions of one iteration (without constants)
    let mut loads = 0usize;
    let mut stores = 0usize;
    // The constants that become splats (a shift count does not), by value.
    let mut splats: HashSet<i128> = HashSet::new();
    let shift_counts: HashSet<Value> = bb
        .insts
        .iter()
        .filter_map(|&k| match f.insts[k].op {
            Op::Bin(BinOp::Shl | BinOp::UShr, _, s) => Some(s),
            _ => None,
        })
        .collect();
    // `i * 8`, and the addresses made from it: only for a test counter that steps by one.
    let mut idx_muls: HashSet<Value> = HashSet::new();
    let mut addrs: HashSet<Value> = HashSet::new();
    let mut bases: HashSet<Value> = HashSet::new();
    let updates: HashSet<Value> = back.args.iter().copied().collect();
    for &k in &bb.insts {
        let inst = &f.insts[k];
        // Integers, and the pointers of addresses.
        if let Some(r) = inst.result {
            if f.ty(r) != i64t && !(f.ty(r) == IrTy::Ptr && matches!(inst.op, Op::PtrAdd(..))) {
                return None;
            }
        }
        match &inst.op {
            Op::Const(IrTy::Int(64), Const::Int(c)) => {
                if !shift_counts.contains(&inst.result?) {
                    splats.insert(*c);
                }
            }
            Op::Bin(BinOp::Add | BinOp::Sub | BinOp::And | BinOp::Or | BinOp::Xor, ..) => {
                work += 1;
                scalar_ops += 1;
            }
            Op::Checked(BinOp::Add | BinOp::Sub, true, ..) if updates.contains(&inst.result?) => {
                work += 1;
                scalar_ops += 1;
            }
            Op::Bin(BinOp::Shl | BinOp::UShr, _, s)
                if konst(*s).is_some_and(|s| (0..64).contains(&s)) =>
            {
                work += 1;
                scalar_ops += 1;
            }
            // `i * 8`: the offset of an element, read by addresses and nothing else (checked below).
            Op::Bin(BinOp::Mul, a, s) if *a == i && step == 1 && konst(*s) == Some(ELEM) => {
                idx_muls.insert(inst.result?);
                scalar_ops += 1;
            }
            Op::PtrAdd(base, off)
                if idx_muls.contains(off)
                    && !inside.contains(base)
                    && uses.get(&inst.result?) == Some(&1) =>
            {
                addrs.insert(inst.result?);
                bases.insert(*base);
                scalar_ops += 1;
            }
            Op::Load(IrTy::Int(64), q) if addrs.contains(q) => {
                loads += 1;
                work += 1;
                scalar_ops += 1;
            }
            Op::Store(q, x) if addrs.contains(q) && f.ty(*x) == i64t => {
                stores += 1;
                work += 1;
                scalar_ops += 1;
            }
            _ => return None,
        }
    }
    // Every offset is read by addresses only.
    let address_reads = bb
        .insts
        .iter()
        .filter(|&&k| matches!(f.insts[k].op, Op::PtrAdd(_, off) if idx_muls.contains(&off)))
        .count();
    if idx_muls.iter().map(|m| uses[m]).sum::<usize>() != address_reads {
        return None;
    }
    // A loop that stores keeps to one array, with no sum that can stop (it could not run again). One that
    // only fills (stores and reads nothing) is the fill pass's, and memory-bound anyway.
    if stores > 0 && (bases.len() != 1 || guarded || loads == 0) {
        return None;
    }
    // A checked sum adds up elements the guard can bound: loads, or a value masked small.
    let mut checked_loads = 0usize;
    for (idx, k) in kinds.iter().enumerate() {
        if let Kind::Acc { checked: true, .. } = k {
            let x = back.args[idx];
            let (Op::Checked(_, _, a, e) | Op::Bin(_, a, e)) = &f.insts[in_body[&x]].op else {
                return None;
            };
            let e = if *a == hb.params[idx] { *e } else { *a };
            match in_body.get(&e).map(|k| &f.insts[*k].op) {
                Some(Op::Load(..)) => checked_loads += 1,
                Some(Op::Bin(BinOp::And, _, m)) | Some(Op::Bin(BinOp::And, m, _))
                    if konst(*m).is_some_and(|m| (0..1 << ELEM_BITS).contains(&m)) => {}
                _ => return None,
            }
        }
    }
    // Counters read by the body besides their own update (and the offsets of addresses) need
    // vectors; the others are only worked out at the end.
    let mut uses_v: HashMap<Value, usize> = HashMap::new();
    for &k in &bb.insts {
        if inst_is_idx(&f.insts[k], &idx_muls) {
            continue;
        }
        f.insts[k]
            .op
            .for_each_value(|v| *uses_v.entry(v).or_default() += 1);
    }
    for a in &back.args {
        *uses_v.entry(*a).or_default() += 1;
    }
    let reads = |idx: usize| {
        let p = hb.params[idx];
        // Its own update reads it once; any other read, or a read of the updated value, needs lanes.
        uses_v.get(&p).copied().unwrap_or(0) > 1
            || uses_v.get(&back.args[idx]).copied().unwrap_or(0) > 1
    };
    let mut vec_params: Vec<usize> = Vec::new();
    for idx in 0..hb.params.len() {
        match kinds[idx] {
            Kind::Acc { .. } => vec_params.push(idx),
            Kind::Counter(_) if reads(idx) => vec_params.push(idx),
            Kind::Counter(_) => {}
        }
    }
    let flag = usize::from(checked_loads > 0);
    let live = U * vec_params.len() + flag + splats.len() + vec_params.len() + 2 * flag;
    if live > MAX_LIVE || (vec_params.is_empty() && stores == 0) {
        return None;
    }
    let counters = vec_params
        .iter()
        .filter(|i| matches!(kinds[**i], Kind::Counter(_)))
        .count();
    let scalar = 2 * U * (scalar_ops + 1);
    let mem = usize::from(loads + stores > 0);
    let vector = U * (work + counters + 2 * checked_loads) + 3 + mem * (1 + U);
    if vector * 5 > scalar * 4 {
        return None;
    }

    // ---- Build -----------------------------------------------------------------------------------
    let v2 = IrTy::V2I64;
    let span = ((2 * U as i128) - 1) * step;
    let tys: Vec<IrTy> = hb.params.iter().map(|p| f.ty(*p)).collect();
    let pre = f.new_block(&tys);
    let mut vtys = vec![i64t];
    vtys.extend(std::iter::repeat_n(v2, U * vec_params.len() + flag));
    let h2 = f.new_block(&vtys);
    let b2 = f.new_block(&[]);
    let exit = f.new_block(&vtys);
    let pre_args = f.blocks[pre.0 as usize].params.clone();
    let mut cx = Ctx {
        f,
        pre,
        body: b2,
        consts: HashMap::new(),
        splats: HashMap::new(),
        inner: HashSet::new(),
    };

    // Entry: the vectors of every counter and accumulator.
    let zero = cx.konst(0);
    let mut init: Vec<Value> = vec![pre_args[ix]];
    for &idx in &vec_params {
        let p0 = pre_args[idx];
        for u in 0..U {
            let v = match kinds[idx] {
                Kind::Acc { .. } if u == 0 => {
                    cx.f.emit(pre, Op::VBin(VBin::Build, p0, zero), Some(v2))
                        .expect("a vector")
                }
                Kind::Acc { .. } => cx.splat(zero),
                Kind::Counter(k) => {
                    let lane = |m: i128, cx: &mut Ctx| {
                        if m == 0 {
                            return p0;
                        }
                        let c = cx.konst(m * k);
                        cx.f.emit(pre, Op::Bin(BinOp::Add, p0, c), Some(i64t))
                            .expect("a sum")
                    };
                    let a = lane(2 * u as i128, &mut cx);
                    let c = lane(2 * u as i128 + 1, &mut cx);
                    cx.f.emit(pre, Op::VBin(VBin::Build, a, c), Some(v2))
                        .expect("a vector")
                }
            };
            init.push(v);
        }
    }
    if flag == 1 {
        init.push(cx.splat(zero));
    }

    // The bound of the vector loop. A constant one is worked out here; for a value, the unroller's
    // way: a `select` that makes the loop run zero times if the subtraction would pass the smallest
    // integer. A guarded loop also needs the start values in the range the guard shows.
    let least = cx.konst(i64::MIN as i128);
    let mut lim = match bound {
        Some(bnd) => cx.konst(bnd - span),
        None => {
            let floor = cx.konst(i64::MIN as i128 + span);
            let ok = cx.pre_op(Op::Cmp(CmpOp::Ge, n, floor), IrTy::Bool);
            let by = cx.konst(span);
            let shifted = cx.pre_op(Op::Bin(BinOp::Sub, n, by), i64t);
            cx.pre_op(Op::Select(ok, shifted, least), i64t)
        }
    };
    if guarded {
        // Nothing may be negative or large: `(i0 | n) >> 33 == 0`, each sum's start `a0 + 2^61` below
        // `2^62` as an unsigned number.
        let both = cx.pre_op(Op::Bin(BinOp::Or, pre_args[ix], n), i64t);
        let t = cx.konst(TRIP_BITS as i128);
        let mut bad = cx.pre_op(Op::Bin(BinOp::UShr, both, t), i64t);
        for (idx, k) in kinds.iter().enumerate() {
            if matches!(k, Kind::Acc { checked: true, .. }) {
                let off = cx.konst(1 << ACC_BITS);
                let shifted = cx.pre_op(Op::Bin(BinOp::Add, pre_args[idx], off), i64t);
                let t = cx.konst(ACC_BITS as i128 + 1);
                let high = cx.pre_op(Op::Bin(BinOp::UShr, shifted, t), i64t);
                bad = cx.pre_op(Op::Bin(BinOp::Or, bad, high), i64t);
            }
        }
        let ok = cx.pre_op(Op::Cmp(CmpOp::Eq, bad, zero), IrTy::Bool);
        lim = cx.pre_op(Op::Select(ok, lim, least), i64t);
    }

    // The vector loop's test and its exit.
    let h2p = cx.f.blocks[h2.0 as usize].params.clone();
    let test =
        cx.f.emit(h2, Op::Cmp(CmpOp::Lt, h2p[0], lim), Some(IrTy::Bool))
            .expect("a comparison");
    cx.f.terminate(
        h2,
        Term::Branch(
            test,
            Target {
                block: b2,
                args: vec![],
            },
            Target {
                block: exit,
                args: h2p.clone(),
            },
        ),
    );

    // The body, once per vector of the pass.
    let slot = |pos: usize, u: usize| 1 + pos * U + u;
    let flag_slot = 1 + U * vec_params.len();
    let mut next_state: Vec<Value> = vec![h2p[0]; U * vec_params.len()];
    // The offset of the first element of the pass, and of each vector in it.
    let first = if loads + stores > 0 {
        let e = cx.konst(ELEM);
        Some(
            cx.f.emit(b2, Op::Bin(BinOp::Mul, h2p[0], e), Some(i64t))
                .expect("an offset"),
        )
    } else {
        None
    };
    let mut flags = flag_slot_value(&h2p, flag, flag_slot);
    let bias = if flag == 1 {
        let c = cx.konst(1 << ELEM_BITS);
        Some(cx.splat(c))
    } else {
        None
    };
    for u in 0..U {
        let mut env: HashMap<Value, Lane> = HashMap::new();
        for (pos, &idx) in vec_params.iter().enumerate() {
            if matches!(kinds[idx], Kind::Counter(_)) {
                env.insert(hb.params[idx], Lane::V(h2p[slot(pos, u)]));
            }
        }
        // This copy's offset into the arrays: sixteen bytes per vector before it.
        let offset = match first {
            Some(o) if u > 0 => {
                let c = cx.konst(2 * u as i128 * ELEM);
                Some(
                    cx.f.emit(b2, Op::Bin(BinOp::Add, o, c), Some(i64t))
                        .expect("an offset"),
                )
            }
            other => other,
        };
        for &k in &bb.insts {
            let inst = cx.f.insts[k].clone();
            let lane = |env: &HashMap<Value, Lane>, v: Value| *env.get(&v).unwrap_or(&Lane::S(v));
            // Updates of accumulators are made here, in their place.
            let r = inst.result;
            let acc = r.and_then(|r| {
                hb.params.iter().enumerate().find(|(idx, _)| {
                    back.args[*idx] == r && matches!(kinds[*idx], Kind::Acc { .. })
                })
            });
            if let Some((idx, p)) = acc {
                let (Op::Bin(_, x, y) | Op::Checked(_, _, x, y)) = &inst.op else {
                    return None;
                };
                let Kind::Acc { minus, checked } = kinds[idx] else {
                    unreachable!()
                };
                let e = if x == p { *y } else { *x };
                let ve = {
                    let l = lane(&env, e);
                    cx.vec(l)?
                };
                // The guard's flag: `e + 2^29` has a bit above `2^30` set when `e` is out of range.
                if checked
                    && matches!(
                        in_body.get(&e).map(|k| &cx.f.insts[*k].op),
                        Some(Op::Load(..))
                    )
                {
                    let shifted =
                        cx.f.emit(
                            b2,
                            Op::VBin(VBin::Add, ve, bias.expect("a guard has a bias")),
                            Some(v2),
                        )
                        .expect("a vector");
                    flags = cx.f.emit(
                        b2,
                        Op::VBin(VBin::Or, flags.expect("a flag"), shifted),
                        Some(v2),
                    );
                }
                let pos = vec_params.iter().position(|q| *q == idx).expect("listed");
                let a = h2p[slot(pos, u)];
                let o = if minus { VBin::Sub } else { VBin::Add };
                let n =
                    cx.f.emit_at(b2, Op::VBin(o, a, ve), Some(v2), inst.span)
                        .expect("a vector");
                next_state[pos * U + u] = n;
                continue;
            }
            // A counter's update: only the jump back reads it, or the lanes of its next step.
            let counter = (0..hb.params.len())
                .find(|idx| Some(back.args[*idx]) == r && matches!(kinds[*idx], Kind::Counter(_)));
            if let Some(idx) = counter {
                if uses.get(&r?) == Some(&1) {
                    continue;
                }
                let Kind::Counter(k) = kinds[idx] else {
                    unreachable!()
                };
                let pos = vec_params.iter().position(|q| *q == idx)?;
                let kc = cx.konst(k);
                let sv = cx.splat(kc);
                let n =
                    cx.f.emit_at(
                        b2,
                        Op::VBin(VBin::Add, h2p[slot(pos, u)], sv),
                        Some(v2),
                        inst.span,
                    )
                    .expect("a vector");
                env.insert(r?, Lane::V(n));
                continue;
            }
            let out = match &inst.op {
                Op::Const(_, Const::Int(n)) => Lane::S(cx.konst(*n)),
                Op::Bin(BinOp::Mul, ..) if r.is_some_and(|r| idx_muls.contains(&r)) => Lane::Idx,
                Op::PtrAdd(base, off) if r.is_some_and(|r| addrs.contains(&r)) => {
                    let (Lane::S(b), Lane::Idx) = (lane(&env, *base), lane(&env, *off)) else {
                        return None;
                    };
                    Lane::Addr(b)
                }
                Op::Load(_, q) => {
                    let Lane::Addr(base) = lane(&env, *q) else {
                        return None;
                    };
                    let at =
                        cx.f.emit_at(b2, Op::PtrAdd(base, offset?), Some(IrTy::Ptr), inst.span)
                            .expect("an address");
                    Lane::V(
                        cx.f.emit_at(b2, Op::Load(v2, at), Some(v2), inst.span)
                            .expect("a vector"),
                    )
                }
                Op::Store(q, x) => {
                    let Lane::Addr(base) = lane(&env, *q) else {
                        return None;
                    };
                    let vx = {
                        let l = lane(&env, *x);
                        cx.vec(l)?
                    };
                    let at =
                        cx.f.emit_at(b2, Op::PtrAdd(base, offset?), Some(IrTy::Ptr), inst.span)
                            .expect("an address");
                    cx.f.emit_at(b2, Op::Store(at, vx), None, inst.span);
                    continue;
                }
                Op::Bin(o @ (BinOp::Shl | BinOp::UShr), a, s) => {
                    let Some(Lane::V(va)) = env.get(a).copied() else {
                        return None;
                    };
                    let k = konst(*s)? as u8;
                    let un = if *o == BinOp::Shl {
                        VUn::Shl(k)
                    } else {
                        VUn::UShr(k)
                    };
                    Lane::V(
                        cx.f.emit_at(b2, Op::VUn(un, va), Some(v2), inst.span)
                            .expect("a vector"),
                    )
                }
                Op::Bin(o, a, c) => {
                    let (la, lc) = (lane(&env, *a), lane(&env, *c));
                    if let (Lane::S(x), Lane::S(y)) = (la, lc) {
                        let v =
                            cx.f.emit_at(b2, Op::Bin(*o, x, y), Some(i64t), inst.span)
                                .expect("a value");
                        cx.inner.insert(v);
                        Lane::S(v)
                    } else {
                        let vo = match o {
                            BinOp::Add => VBin::Add,
                            BinOp::Sub => VBin::Sub,
                            BinOp::And => VBin::And,
                            BinOp::Or => VBin::Or,
                            _ => VBin::Xor,
                        };
                        let (x, y) = (cx.vec(la)?, cx.vec(lc)?);
                        Lane::V(
                            cx.f.emit_at(b2, Op::VBin(vo, x, y), Some(v2), inst.span)
                                .expect("a vector"),
                        )
                    }
                }
                _ => return None,
            };
            env.insert(r?, out);
        }
        // The counters' next vectors.
        for (pos, &idx) in vec_params.iter().enumerate() {
            if let Kind::Counter(k) = kinds[idx] {
                let step = cx.konst(2 * U as i128 * k);
                let sv = cx.splat(step);
                let cur = h2p[slot(pos, u)];
                let n =
                    cx.f.emit(b2, Op::VBin(VBin::Add, cur, sv), Some(v2))
                        .expect("a vector");
                next_state[pos * U + u] = n;
            }
        }
    }
    let step_i = cx.konst(2 * U as i128 * step);
    let i2 =
        cx.f.emit(b2, Op::Bin(BinOp::Add, h2p[0], step_i), Some(i64t))
            .expect("a sum");
    let mut jump = vec![i2];
    jump.extend(next_state);
    jump.extend(flags);
    cx.f.terminate(
        b2,
        Term::Jump(Target {
            block: h2,
            args: jump,
        }),
    );

    // The exit: scalar values for the original loop.
    let ep = cx.f.blocks[exit.0 as usize].params.clone();
    let mut out: Vec<Value> = Vec::new();
    for idx in 0..hb.params.len() {
        let v = match kinds[idx] {
            _ if idx == ix => ep[0],
            Kind::Counter(k) => {
                // The start value plus one step for every iteration the vector loop ran.
                let done = {
                    let d =
                        cx.f.emit(exit, Op::Bin(BinOp::Sub, ep[0], pre_args[ix]), Some(i64t))
                            .expect("a difference");
                    // The iterations done: the distance in steps of the test counter, times this step.
                    let d = if step == 1 {
                        d
                    } else {
                        let sc =
                            cx.f.emit(exit, Op::Const(i64t, Const::Int(step)), Some(i64t))
                                .expect("a constant");
                        cx.f.emit(exit, Op::Bin(BinOp::Div, d, sc), Some(i64t))
                            .expect("a quotient")
                    };
                    if k == 1 {
                        d
                    } else {
                        let kc =
                            cx.f.emit(exit, Op::Const(i64t, Const::Int(k)), Some(i64t))
                                .expect("a constant");
                        cx.f.emit(exit, Op::Bin(BinOp::Mul, d, kc), Some(i64t))
                            .expect("a product")
                    }
                };
                cx.f.emit(exit, Op::Bin(BinOp::Add, pre_args[idx], done), Some(i64t))
                    .expect("a sum")
            }
            Kind::Acc { .. } => {
                let pos = vec_params.iter().position(|q| *q == idx).expect("listed");
                let mut sum = ep[slot(pos, 0)];
                for u in 1..U {
                    sum =
                        cx.f.emit(exit, Op::VBin(VBin::Add, sum, ep[slot(pos, u)]), Some(v2))
                            .expect("a vector");
                }
                cx.f.emit(exit, Op::VUn(VUn::ReduceAdd, sum), Some(i64t))
                    .expect("a sum")
            }
        };
        out.push(v);
    }
    let done = Target {
        block: h,
        args: out,
    };
    if flag == 1 {
        // An element out of range: the original loop runs again from the start values.
        let high =
            cx.f.emit(
                exit,
                Op::VUn(VUn::UShr(ELEM_BITS as u8 + 1), ep[flag_slot]),
                Some(v2),
            )
            .expect("a vector");
        let seen =
            cx.f.emit(exit, Op::VUn(VUn::ReduceAdd, high), Some(i64t))
                .expect("a sum");
        let z =
            cx.f.emit(exit, Op::Const(i64t, Const::Int(0)), Some(i64t))
                .expect("a constant");
        let bad =
            cx.f.emit(exit, Op::Cmp(CmpOp::Ne, seen, z), Some(IrTy::Bool))
                .expect("a comparison");
        cx.f.terminate(
            exit,
            Term::Branch(
                bad,
                Target {
                    block: h,
                    args: pre_args.clone(),
                },
                done,
            ),
        );
    } else {
        cx.f.terminate(exit, Term::Jump(done));
    }
    cx.f.terminate(
        pre,
        Term::Jump(Target {
            block: h2,
            args: init,
        }),
    );
    let f = cx.f;
    // Everything that entered the loop from outside goes through the entry block.
    let preds = f.predecessors();
    for p in preds[h.0 as usize].clone() {
        if p == b || p == exit {
            continue;
        }
        if let Some(t) = &mut f.blocks[p.0 as usize].term {
            for tg in t.targets_mut() {
                if tg.block == h {
                    tg.block = pre;
                }
            }
        }
    }
    let _ = (yes, no);
    Some(h2)
}

/// Whether the instruction makes `i * 8` for an address.
fn inst_is_idx(inst: &Inst, idx_muls: &HashSet<Value>) -> bool {
    inst.result.is_some_and(|r| idx_muls.contains(&r))
}

/// The loop-carried flag vector of a pass, when the loop has one.
fn flag_slot_value(params: &[Value], flag: usize, slot: usize) -> Option<Value> {
    (flag == 1).then(|| params[slot])
}
