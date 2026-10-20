//! Split slots (scalar replacement of aggregates). A slot that is only touched through loads and
//! stores at constant offsets (and whole-slot copies) is cut into one slot per piece, so that slot
//! promotion can then turn the pieces into registers. This is what lets a `string` header
//! (`ptr, len, cap`) or an `option<int>` live in registers.

use crate::ir::*;
use std::collections::{BTreeMap, HashMap, HashSet};

struct Info {
    size: u32,
    /// Offset to type of each piece touched.
    pieces: BTreeMap<u32, IrTy>,
    bad: bool,
    /// Whole-slot copies: (instruction, this slot is the destination, the other address, length).
    copies: Vec<(usize, bool, Value, u32)>,
}

pub fn run(f: &mut Func) -> bool {
    // Constants that are integers, and the slots.
    let mut consts: HashMap<Value, i128> = HashMap::new();
    let mut info: HashMap<Value, Info> = HashMap::new();
    let mut slot_inst: HashMap<Value, usize> = HashMap::new();
    for b in &f.blocks {
        for &i in &b.insts {
            let inst = &f.insts[i];
            match (&inst.op, inst.result) {
                (Op::Const(_, Const::Int(n)), Some(r)) => {
                    consts.insert(r, *n);
                }
                (Op::Slot(size, _), Some(r)) => {
                    info.insert(
                        r,
                        Info {
                            size: *size,
                            pieces: BTreeMap::new(),
                            bad: false,
                            copies: Vec::new(),
                        },
                    );
                    slot_inst.insert(r, i);
                }
                _ => {}
            }
        }
    }
    if info.is_empty() {
        return false;
    }
    // Addresses derived from a slot at a constant offset.
    let mut base: HashMap<Value, (Value, u32)> = info.keys().map(|s| (*s, (*s, 0))).collect();
    for b in &f.blocks {
        for &i in &b.insts {
            if let (Op::PtrAdd(a, k), Some(r)) = (&f.insts[i].op, f.insts[i].result) {
                if let (Some((s, 0)), Some(k)) = (base.get(a).copied(), consts.get(k)) {
                    if (0..=u32::MAX as i128).contains(k) && info.contains_key(&s) && *a == s {
                        base.insert(r, (s, *k as u32));
                    }
                }
            }
        }
    }
    // Whole-slot copies between two slots: (the instruction, destination, source, length).
    let mut slot_copies: Vec<(usize, Value, Value, u32)> = Vec::new();
    // Every use of those addresses.
    for b in &f.blocks {
        for &i in &b.insts {
            let op = &f.insts[i].op;
            match op {
                Op::Load(ty, a) if base.contains_key(a) => {
                    let (s, off) = base[a];
                    touch(&mut info, s, off, *ty);
                }
                Op::Store(a, v) => {
                    if let Some(&(s, off)) = base.get(a) {
                        touch(&mut info, s, off, f.ty(*v));
                    }
                    if let Some(&(s, _)) = base.get(v) {
                        mark_bad(&mut info, s);
                    }
                }
                Op::PtrAdd(a, _) if base.contains_key(a) => {
                    // The derived address itself is fine when it was recognised above.
                    let r = f.insts[i].result;
                    if !(r.is_some_and(|r| base.contains_key(&r) && base[&r].0 == *a)) {
                        mark_bad(&mut info, base[a].0);
                    }
                }
                Op::MemCopy(d, s, len) => {
                    let len_c = consts
                        .get(len)
                        .copied()
                        .filter(|n| *n >= 0 && *n <= u32::MAX as i128);
                    let (bd, bs) = (base.get(d).copied(), base.get(s).copied());
                    // Two different slots copied whole: both are split with the same pieces, and the
                    // copy becomes a load and a store per piece.
                    if let (Some((sd, 0)), Some((ss, 0)), Some(n)) = (bd, bs, len_c) {
                        if sd != ss && *d == sd && *s == ss {
                            slot_copies.push((i, sd, ss, n as u32));
                            if let Some(inf) = info.get_mut(&sd) {
                                inf.copies.push((i, true, *s, n as u32));
                            }
                            if let Some(inf) = info.get_mut(&ss) {
                                inf.copies.push((i, false, *d, n as u32));
                            }
                            continue;
                        }
                    }
                    match (bd, bs, len_c) {
                        (Some((sd, 0)), Some((ss, 0)), _) if sd == ss => mark_bad(&mut info, sd),
                        (Some((sd, 0)), Some((ss, _)), _) if *d == sd => {
                            // Both sides are slots: left alone for now.
                            mark_bad(&mut info, sd);
                            mark_bad(&mut info, ss);
                        }
                        (Some((sd, 0)), _, Some(n)) if *d == sd => {
                            if let Some(inf) = info.get_mut(&sd) {
                                inf.copies.push((i, true, *s, n as u32));
                            }
                            if let Some((ss, _)) = bs {
                                // The other side is a slot too: handled when that one is split.
                                if base.get(s).is_some_and(|b| b.1 != 0 || ss != *s) {
                                    mark_bad(&mut info, ss);
                                }
                            }
                        }
                        _ => {
                            if let Some((sd, _)) = bd {
                                mark_bad(&mut info, sd);
                            }
                        }
                    }
                    match (bs, bd, len_c) {
                        (Some((ss, _)), Some(_), _) => mark_bad(&mut info, ss),
                        (Some((ss, 0)), _, Some(n))
                            if *s == ss && !matches!(bd, Some((sd, 0)) if sd == ss) =>
                        {
                            if let Some(inf) = info.get_mut(&ss) {
                                inf.copies.push((i, false, *d, n as u32));
                            }
                        }
                        (Some((ss, _)), _, _) if !matches!(bd, Some((sd, 0)) if sd == ss) => {
                            mark_bad(&mut info, ss)
                        }
                        _ => {}
                    }
                }
                Op::Load(_, _) => {}
                op => op.for_each_value(|v| {
                    if let Some(&(s, _)) = base.get(&v) {
                        mark_bad(&mut info, s);
                    }
                }),
            }
        }
        if let Some(t) = &b.term {
            t.for_each_value(|v| {
                if let Some(&(s, _)) = base.get(&v) {
                    mark_bad(&mut info, s);
                }
            });
        }
    }
    // The two sides of a slot-to-slot copy get each other's pieces.
    loop {
        let mut changed = false;
        for &(_, d, s, n) in &slot_copies {
            for (from, to) in [(d, s), (s, d)] {
                let pieces: Vec<(u32, IrTy)> = info[&from]
                    .pieces
                    .iter()
                    .filter(|(o, _)| **o < n)
                    .map(|(o, t)| (*o, *t))
                    .collect();
                for (off, ty) in pieces {
                    if !info[&to].pieces.contains_key(&off) {
                        changed = true;
                    }
                    touch(&mut info, to, off, ty);
                }
            }
        }
        if !changed {
            break;
        }
    }
    // Which slots can be split.
    let mut todo: Vec<Value> = Vec::new();
    for (s, inf) in &info {
        if inf.bad || inf.pieces.is_empty() {
            continue;
        }
        // Pieces must fit and not overlap.
        let mut end = 0u32;
        let mut ok = true;
        for (off, ty) in &inf.pieces {
            if *off < end || off + ty.size() > inf.size || ty.size() == 0 {
                ok = false;
                break;
            }
            end = off + ty.size();
        }
        // Each whole-slot copy must be exactly tiled by pieces.
        for (_, _, _, len) in &inf.copies {
            let mut at = 0u32;
            for (off, ty) in &inf.pieces {
                if *off >= *len {
                    break;
                }
                if *off != at || off + ty.size() > *len {
                    ok = false;
                    break;
                }
                at = off + ty.size();
            }
            if at != *len {
                ok = false;
            }
        }
        // A single piece the size of the slot is no gain unless it is promotable later; still fine.
        if ok {
            todo.push(*s);
        }
    }
    if todo.is_empty() {
        return false;
    }
    todo.sort();
    let todo_set: HashSet<Value> = todo.iter().copied().collect();
    // New slots, one per piece.
    let mut piece_slot: HashMap<(Value, u32), Value> = HashMap::new();
    let mut new_insts: HashMap<usize, Vec<usize>> = HashMap::new();
    for s in &todo {
        let inf = &info[s];
        let at = slot_inst[s];
        let mut made = Vec::new();
        for (off, ty) in &inf.pieces {
            let v = f.new_value(IrTy::Ptr);
            f.insts.push(Inst {
                result: Some(v),
                op: Op::Slot(ty.size(), ty.size().max(1)),
                span: f.insts[at].span,
            });
            made.push(f.insts.len() - 1);
            piece_slot.insert((*s, *off), v);
        }
        new_insts.insert(at, made);
    }
    // Rewrite every block.
    let sp = crate::span::Span::new(0, 0);
    for bi in 0..f.blocks.len() {
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let mut out: Vec<usize> = Vec::with_capacity(old.len());
        for i in old {
            if let Some(made) = new_insts.get(&i) {
                out.push(i);
                out.extend(made.iter().copied());
                continue;
            }
            let op = f.insts[i].op.clone();
            match op {
                Op::Load(ty, a) => {
                    if let Some(&(s, off)) = base.get(&a) {
                        if todo_set.contains(&s) {
                            f.insts[i].op = Op::Load(ty, piece_slot[&(s, off)]);
                        }
                    }
                    out.push(i);
                }
                Op::Store(a, v) => {
                    if let Some(&(s, off)) = base.get(&a) {
                        if todo_set.contains(&s) {
                            f.insts[i].op = Op::Store(piece_slot[&(s, off)], v);
                        }
                    }
                    out.push(i);
                }
                Op::MemCopy(d, s, len) => {
                    let dst_slot = base
                        .get(&d)
                        .filter(|(x, o)| todo_set.contains(x) && *o == 0 && *x == d)
                        .map(|b| b.0);
                    let src_slot = base
                        .get(&s)
                        .filter(|(x, o)| todo_set.contains(x) && *o == 0 && *x == s)
                        .map(|b| b.0);
                    let n = consts.get(&len).copied().unwrap_or(0) as u32;
                    match (dst_slot, src_slot) {
                        (None, None) => out.push(i),
                        (Some(ds), None) | (None, Some(ds)) => {
                            let into_slot = dst_slot.is_some();
                            let other = if into_slot { s } else { d };
                            for (off, ty) in info[&ds].pieces.iter().filter(|(o, _)| **o < n) {
                                let piece = piece_slot[&(ds, *off)];
                                let k = f.new_value(IrTy::I64);
                                f.insts.push(Inst {
                                    result: Some(k),
                                    op: Op::Const(IrTy::I64, Const::Int(*off as i128)),
                                    span: sp,
                                });
                                out.push(f.insts.len() - 1);
                                let p = f.new_value(IrTy::Ptr);
                                f.insts.push(Inst {
                                    result: Some(p),
                                    op: Op::PtrAdd(other, k),
                                    span: sp,
                                });
                                out.push(f.insts.len() - 1);
                                let v = f.new_value(*ty);
                                if into_slot {
                                    f.insts.push(Inst {
                                        result: Some(v),
                                        op: Op::Load(*ty, p),
                                        span: sp,
                                    });
                                    out.push(f.insts.len() - 1);
                                    f.insts.push(Inst {
                                        result: None,
                                        op: Op::Store(piece, v),
                                        span: sp,
                                    });
                                } else {
                                    f.insts.push(Inst {
                                        result: Some(v),
                                        op: Op::Load(*ty, piece),
                                        span: sp,
                                    });
                                    out.push(f.insts.len() - 1);
                                    f.insts.push(Inst {
                                        result: None,
                                        op: Op::Store(p, v),
                                        span: sp,
                                    });
                                }
                                out.push(f.insts.len() - 1);
                            }
                        }
                        (Some(ds), Some(ss)) => {
                            for (off, ty) in info[&ds].pieces.iter().filter(|(o, _)| **o < n) {
                                let v = f.new_value(*ty);
                                f.insts.push(Inst {
                                    result: Some(v),
                                    op: Op::Load(*ty, piece_slot[&(ss, *off)]),
                                    span: sp,
                                });
                                out.push(f.insts.len() - 1);
                                f.insts.push(Inst {
                                    result: None,
                                    op: Op::Store(piece_slot[&(ds, *off)], v),
                                    span: sp,
                                });
                                out.push(f.insts.len() - 1);
                            }
                        }
                    }
                }
                _ => out.push(i),
            }
        }
        f.blocks[bi].insts = out;
    }
    true
}

fn touch(info: &mut HashMap<Value, Info>, s: Value, off: u32, ty: IrTy) {
    if let Some(inf) = info.get_mut(&s) {
        match inf.pieces.get(&off) {
            None => {
                inf.pieces.insert(off, ty);
            }
            Some(old) if *old == ty => {}
            Some(_) => inf.bad = true,
        }
    }
}

fn mark_bad(info: &mut HashMap<Value, Info>, s: Value) {
    if let Some(inf) = info.get_mut(&s) {
        inf.bad = true;
    }
}
