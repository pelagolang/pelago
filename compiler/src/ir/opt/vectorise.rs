//! The loop vectoriser (card A-11, `docs/design/vectorise.md`). A counted loop whose body is plain
//! wrapping integer arithmetic over its counters runs two iterations per vector (SSE2 has two 64-bit
//! lanes) and `U` vectors per pass, so `2 * U` iterations per pass:
//!
//! ```text
//! while i < n { p += 10; t += p & 1023; i += 1 }
//! ```
//!
//! A counter that steps by a constant `k` becomes `U` vectors holding `c, c + k` / `c + 2k, c + 3k`
//! / ..., each stepping by `2 * U * k` per pass. An accumulator (`t += e` or `t -= e`, read nowhere
//! else) becomes `U` vectors of partial sums, added up after the loop. Everything else in the body
//! is done lane by lane with the same operation.
//!
//! Nothing here can change what the program does. The operations are wrapping ones (a checked
//! operation is not vectorised), so every lane computes exactly what its iteration would have, and
//! sums are reassociated only for integers, where wrapping addition is associative. The vector loop
//! runs while `i < n - (2 * U - 1)`, so every iteration it covers is one the original loop would
//! have run; the original loop is kept and finishes the last few iterations. Like the unroller, it
//! only takes loops of two blocks with a constant bound of at least [`MIN_TRIPS`] passes and a step
//! of at most 2^20, and a body without memory or calls (so a vector never lives across a call).

use super::dom::Dom;
use super::loops;
use crate::ir::*;
use std::collections::{HashMap, HashSet};

/// Vectors per pass, so iterations per pass are `2 * U`.
const U: usize = 2;
const MIN_TRIPS: i128 = 256;
/// Vectors that may be alive in the loop (the allocator has 14 registers).
const MAX_LIVE: usize = 14;

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
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Steps by this constant each iteration.
    Counter(i128),
    /// `p + e` (or `p - e` when the flag is set), `p` read nowhere else.
    Acc(bool),
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

    fn vec(&mut self, l: Lane) -> Value {
        match l {
            Lane::V(v) => v,
            Lane::S(v) => self.splat(v),
        }
    }
}

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
    let bound = konst(n)?;
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
        let kind = match upd {
            Some(Op::Bin(BinOp::Add, a, e)) | Some(Op::Bin(BinOp::Add, e, a))
                if a == p && e != p && konst(*e).is_some() =>
            {
                Kind::Counter(konst(*e)?)
            }
            Some(Op::Bin(o @ (BinOp::Add | BinOp::Sub), a, e))
                if a == p && !matches!(e, v if v == p) =>
            {
                // `p` is read only by this update, and the update only by the jump back.
                if uses.get(p) != Some(&1) || uses.get(&x) != Some(&1) {
                    return None;
                }
                Kind::Acc(*o == BinOp::Sub)
            }
            Some(Op::Bin(BinOp::Add, e, a)) if a == p && e != p => {
                if uses.get(p) != Some(&1) || uses.get(&x) != Some(&1) {
                    return None;
                }
                Kind::Acc(false)
            }
            _ => return None,
        };
        kinds.push(kind);
    }
    // The test counter steps by a positive constant, and the bound allows many passes.
    let Kind::Counter(step) = kinds[ix] else {
        return None;
    };
    if !(1..=1 << 20).contains(&step) || bound < MIN_TRIPS * step {
        return None;
    }

    // Which body instructions are vector work, and is it worth it.
    let is_update: HashSet<Value> = back.args.iter().copied().collect();
    let mut work = 0usize;
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
    for &k in &bb.insts {
        let inst = &f.insts[k];
        let r = inst.result?;
        if f.ty(r) != i64t {
            return None;
        }
        match &inst.op {
            Op::Const(IrTy::Int(64), Const::Int(c)) => {
                if !shift_counts.contains(&r) {
                    splats.insert(*c);
                }
            }
            Op::Bin(BinOp::Add | BinOp::Sub | BinOp::And | BinOp::Or | BinOp::Xor, ..) => work += 1,
            Op::Bin(BinOp::Shl | BinOp::UShr, _, s)
                if konst(*s).is_some_and(|s| (0..64).contains(&s)) =>
            {
                work += 1
            }
            _ => return None,
        }
        let _ = is_update.contains(&r);
    }
    // Counters read by the body besides their own update need vectors; the others are only worked out
    // at the end.
    let reads = |idx: usize| {
        let p = hb.params[idx];
        // Its own update reads it once; any other read, or a read of the updated value, needs lanes.
        uses.get(&p).copied().unwrap_or(0) > 1
            || uses.get(&back.args[idx]).copied().unwrap_or(0) > 1
    };
    let mut vec_params: Vec<usize> = Vec::new();
    for idx in 0..hb.params.len() {
        match kinds[idx] {
            Kind::Acc(_) => vec_params.push(idx),
            Kind::Counter(_) if reads(idx) => vec_params.push(idx),
            Kind::Counter(_) => {}
        }
    }
    let live = U * vec_params.len() + splats.len() + vec_params.len();
    if live > MAX_LIVE || vec_params.is_empty() {
        return None;
    }
    let scalar = 2 * U * (work + 1);
    let vector = U * (work + vec_params.len()) + 3;
    if vector * 5 > scalar * 4 {
        return None;
    }

    // ---- Build -----------------------------------------------------------------------------------
    let v2 = IrTy::V2I64;
    let span = ((2 * U as i128) - 1) * step;
    let tys: Vec<IrTy> = hb.params.iter().map(|p| f.ty(*p)).collect();
    let pre = f.new_block(&tys);
    let mut vtys = vec![i64t];
    vtys.extend(std::iter::repeat_n(v2, U * vec_params.len()));
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
                Kind::Acc(_) if u == 0 => {
                    cx.f.emit(pre, Op::VBin(VBin::Build, p0, zero), Some(v2))
                        .expect("a vector")
                }
                Kind::Acc(_) => cx.splat(zero),
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

    // The vector loop's test and its exit.
    let h2p = cx.f.blocks[h2.0 as usize].params.clone();
    let lim = cx.konst(bound - span);
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
    let mut next: Vec<Value> = vec![h2p[0]; 0];
    let mut next_state: Vec<Value> = vec![h2p[0]; U * vec_params.len()];
    for u in 0..U {
        let mut env: HashMap<Value, Lane> = HashMap::new();
        for (pos, &idx) in vec_params.iter().enumerate() {
            if matches!(kinds[idx], Kind::Counter(_)) {
                env.insert(hb.params[idx], Lane::V(h2p[slot(pos, u)]));
            }
        }
        for &k in &bb.insts {
            let inst = cx.f.insts[k].clone();
            let r = inst.result.expect("checked above");
            let lane = |env: &HashMap<Value, Lane>, v: Value| *env.get(&v).unwrap_or(&Lane::S(v));
            // Updates of counters and accumulators are made after the body.
            let acc = hb
                .params
                .iter()
                .enumerate()
                .find(|(idx, _)| back.args[*idx] == r && matches!(kinds[*idx], Kind::Acc(_)));
            if let Some((idx, p)) = acc {
                let (Op::Bin(_, x, y), Kind::Acc(minus)) = (&inst.op, kinds[idx]) else {
                    unreachable!()
                };
                let e = if x == p { *y } else { *x };
                let ve = {
                    let l = lane(&env, e);
                    cx.vec(l)
                };
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
                .find(|idx| back.args[*idx] == r && matches!(kinds[*idx], Kind::Counter(_)));
            if let Some(idx) = counter {
                if uses.get(&r) == Some(&1) {
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
                env.insert(r, Lane::V(n));
                continue;
            }
            let out = match &inst.op {
                Op::Const(t, cst) => {
                    let Const::Int(n) = cst else { return None };
                    let _ = t;
                    Lane::S(cx.konst(*n))
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
                        let (x, y) = (cx.vec(la), cx.vec(lc));
                        Lane::V(
                            cx.f.emit_at(b2, Op::VBin(vo, x, y), Some(v2), inst.span)
                                .expect("a vector"),
                        )
                    }
                }
                _ => return None,
            };
            env.insert(r, out);
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
    let _ = &mut next;
    let step_i = cx.konst(2 * U as i128 * step);
    let i2 =
        cx.f.emit(b2, Op::Bin(BinOp::Add, h2p[0], step_i), Some(i64t))
            .expect("a sum");
    let mut jump = vec![i2];
    jump.extend(next_state);
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
            Kind::Acc(_) => {
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
    cx.f.terminate(
        exit,
        Term::Jump(Target {
            block: h,
            args: out,
        }),
    );
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
