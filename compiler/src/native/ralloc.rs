//! The register allocator for machine IR (work item W4, design in `docs/design/mir.md`).
//!
//! It is the allocator of `regs.rs` (stage W3) made independent of the CPU: a target describes its
//! register file through [`Target`], and the allocator rewrites a [`MFunc`] in place so that no
//! virtual register is left.
//!
//! * **Ranges.** Liveness is the usual backward dataflow over the blocks ([`liveness`]); from it each
//!   register, virtual or physical, gets a list of half-open ranges of positions with holes. An
//!   instruction at index `i` reads at position `2i` and writes at `2i + 1`, so a `Def` may take
//!   the register of a `Use` that dies in the same instruction (an [`Role::EarlyDef`] begins at `2i`
//!   and may not).
//! * **Fixed registers.** A physical register named by an instruction has ranges like any other; a
//!   virtual register is never given a register while one of its ranges meets them.
//! * **Choosing.** Virtual registers are taken heaviest first (every read and write weighs `10^depth`
//!   of the loop it is in). A register costs: nothing when it is caller-saved and no call that
//!   destroys it lies inside the range; one store before and one load after each such call when it is
//!   (the value is split around the call, as `Alloc::across` does in `regs.rs`); two when it is
//!   callee-saved and no other value has paid for it yet (the save and restore of the prologue).
//!   Memory costs the weight of the register. The cheapest register is taken when it is no dearer
//!   than memory; copies to or from a register that is already placed lower the cost of that one.
//! * **Rewriting.** A register in memory has a spill slot; each instruction that reads it reloads it
//!   into a scratch register of the target, and each that writes it stores it after. `COPY` and
//!   `PARCOPY` become moves, loads, stores and `lea`s in an order that reads every source before it is
//!   overwritten; a cycle goes through a scratch register. A copy from a register to itself is dropped.
//!
//! After allocation a block may have a reload between two branches; emitters walk the instructions in
//! order, so that is harmless, but [`verify`](super::mir::verify) is for code before allocation.

use super::mir::*;
use std::collections::HashMap;

/// What the allocator did, for tests and for `pelago mir`.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Stats {
    /// Virtual registers kept in memory.
    pub spilled: usize,
    /// Virtual registers in a caller-saved register that are stored and loaded around a call.
    pub split_around_calls: usize,
    /// Copies that disappeared because source and destination share a register.
    pub copies_removed: usize,
}

/// Where a value is while moves are made.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Loc {
    Reg(PReg),
    Slot(FrameId, Class),
    Imm(i64),
    Addr(FrameId, i32),
}

struct Call {
    pos: u32,
    weight: u64,
    clobbers: RegSet,
    inst: usize,
}

fn add_range(r: &mut Vec<(u32, u32)>, from: u32, to: u32) {
    if from >= to {
        return;
    }
    match r.last_mut() {
        Some(l) if to >= l.0 => {
            l.0 = l.0.min(from);
            l.1 = l.1.max(to);
        }
        _ => r.push((from, to)),
    }
}

/// The register is written at `pos`: its first range begins there.
fn set_from(r: &mut Vec<(u32, u32)>, pos: u32) {
    match r.last_mut() {
        Some(l) if l.0 <= pos && pos < l.1 => l.0 = pos,
        // Never read: a one-position range, so nothing else is overwritten by the write.
        _ => r.push((pos, pos + 1)),
    }
}

/// Is any range of `a` over one of `taken`? `taken` is sorted and its ranges are disjoint.
fn overlaps(a: &[(u32, u32)], taken: &[(u32, u32)]) -> bool {
    a.iter().any(|&(from, to)| {
        let i = taken.partition_point(|x| x.1 <= from);
        i < taken.len() && taken[i].0 < to
    })
}

fn class_of_loc(t: &dyn Target, l: Loc) -> Class {
    match l {
        Loc::Reg(p) => t.regs()[p.0 as usize].class,
        Loc::Slot(_, c) => c,
        _ => Class::Int,
    }
}

/// The scratch registers of a class that a sequence of moves may use: not one the moves read or write,
/// and not one that holds something live across them.
fn free_scratch(t: &dyn Target, c: Class, avoid: RegSet) -> [PReg; 2] {
    let mut it = t.scratch(c).iter().copied().filter(|r| !avoid.contains(*r));
    match (it.next(), it.next()) {
        (Some(a), Some(b)) => [a, b],
        _ => panic!("fewer than two scratch registers of {c:?} are free for a parallel copy"),
    }
}

/// One move from `s` to `d`; a slot to a slot goes through the first scratch register.
fn emit_one(t: &dyn Target, d: Loc, s: Loc, avoid: RegSet, out: &mut Vec<MInst>) {
    // A slot knows whether it holds a float or a vector; the registers of both are the same.
    let c = match (d, s) {
        (Loc::Slot(_, c), _) | (_, Loc::Slot(_, c)) => c,
        _ => class_of_loc(t, d),
    };
    match (d, s) {
        (Loc::Reg(a), Loc::Reg(b)) => {
            if a != b {
                out.push(t.mov(c, a, b));
            }
        }
        (Loc::Reg(a), Loc::Slot(f, _)) => out.push(t.load(c, a, f)),
        (Loc::Slot(f, _), Loc::Reg(b)) => out.push(t.store(c, f, b)),
        (Loc::Slot(f1, _), Loc::Slot(f2, _)) => {
            if f1 != f2 {
                let s0 = free_scratch(t, c, avoid)[0];
                out.push(t.load(c, s0, f2));
                out.push(t.store(c, f1, s0));
            }
        }
        (Loc::Reg(a), Loc::Imm(n)) => out.push(t.mov_imm(a, n)),
        (Loc::Slot(f, _), Loc::Imm(n)) => {
            let s0 = free_scratch(t, c, avoid)[0];
            out.push(t.mov_imm(s0, n));
            out.push(t.store(c, f, s0));
        }
        (Loc::Reg(a), Loc::Addr(f, disp)) => out.push(t.lea(a, f, disp)),
        (Loc::Slot(f, _), Loc::Addr(g, disp)) => {
            let s0 = free_scratch(t, c, avoid)[0];
            out.push(t.lea(s0, g, disp));
            out.push(t.store(c, f, s0));
        }
        (d, s) => unreachable!("move {s:?} to {d:?}"),
    }
}

/// Makes all the moves `(to, from)` as if at once: a place that is both written and read is read
/// first. A cycle is broken with the second scratch register of its class.
fn emit_moves(t: &dyn Target, mut pend: Vec<(Loc, Loc)>, mut avoid: RegSet, out: &mut Vec<MInst>) {
    for (d, s) in &pend {
        for l in [d, s] {
            if let Loc::Reg(r) = l {
                avoid.insert(*r);
            }
        }
    }
    pend.retain(|(d, s)| d != s);
    while !pend.is_empty() {
        let ready = (0..pend.len()).find(|&i| {
            !pend
                .iter()
                .enumerate()
                .any(|(j, m)| j != i && m.1 == pend[i].0)
        });
        match ready {
            Some(i) => {
                let (d, s) = pend.remove(i);
                emit_one(t, d, s, avoid, out);
            }
            None => {
                let d0 = pend[0].0;
                let tmp = Loc::Reg(free_scratch(t, class_of_loc(t, d0), avoid)[1]);
                emit_one(t, tmp, d0, avoid, out);
                for m in pend.iter_mut().filter(|m| m.1 == d0) {
                    m.1 = tmp;
                }
            }
        }
    }
}

/// The calls whose position `p` and `p + 1` both lie in one range of the value.
fn crossing(ranges: &[(u32, u32)], calls: &[Call]) -> Vec<usize> {
    let mut out = Vec::new();
    for &(from, to) in ranges {
        let mut k = calls.partition_point(|c| c.pos < from);
        while k < calls.len() && calls[k].pos + 1 < to {
            out.push(k);
            k += 1;
        }
    }
    out
}

/// Gives every virtual register of `f` a physical register or a slot and rewrites the code.
/// `limit` keeps only the first registers of each class (tests use it to force spills).
pub fn allocate(f: &mut MFunc, t: &dyn Target, limit: usize) -> Stats {
    let nphys = t.regs().len();
    let nv = f.vclass.len();
    let nb = f.blocks.len();
    let mut stats = Stats::default();

    // Positions, loop depths and weights.
    let mut start = vec![0u32; nb];
    let mut end = vec![0u32; nb];
    let mut at = 0u32;
    for (bi, b) in f.blocks.iter().enumerate() {
        start[bi] = at;
        at += 2 * b.insts.len() as u32;
        end[bi] = at;
    }
    let mut depth = vec![0u32; nb];
    for bi in 0..nb {
        for s in successors(f, bi) {
            if s <= bi {
                for d in &mut depth[s..=bi] {
                    *d += 1;
                }
            }
        }
    }
    let block_weight = |bi: usize| 10u64.pow(depth[bi].min(6));

    // Ranges (built from the last block to the first), weights, calls and copies.
    let live = liveness(f, nphys);
    let mut ranges: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nphys + nv];
    let mut weight = vec![0u64; nv];
    // The physical registers live after each instruction: a reload must not use them.
    let ninst: usize = f.blocks.iter().map(|b| b.insts.len()).sum();
    let mut live_phys: Vec<RegSet> = vec![RegSet::default(); ninst];
    let mut calls: Vec<Call> = Vec::new();
    let mut hints: Vec<Vec<(Reg, u64)>> = vec![Vec::new(); nv];
    let first_inst: Vec<usize> = {
        let mut n = 0;
        f.blocks
            .iter()
            .map(|b| {
                n += b.insts.len();
                n - b.insts.len()
            })
            .collect()
    };
    for bi in (0..nb).rev() {
        let (bs, be) = (start[bi], end[bi]);
        let w = block_weight(bi);
        let mut now = live.live_end[bi].clone();
        for id in live.live_end[bi].iter() {
            add_range(&mut ranges[id], bs, be);
        }
        for (k, inst) in f.blocks[bi].insts.iter().enumerate().rev() {
            let p = bs + 2 * k as u32;
            // A branch: what its targets need is live up to here.
            if inst.flags.has(IFlags::BRANCH) {
                if inst.flags.has(IFlags::NOFALL) {
                    now = Bits::new(nphys + nv);
                }
                for o in &inst.ops {
                    if let Operand::Block(t) = o {
                        for id in live.live_in[*t as usize].iter() {
                            if !now.contains(id) {
                                add_range(&mut ranges[id], bs, p + 1);
                                now.insert(id);
                            }
                        }
                    }
                }
            } else if inst.flags.has(IFlags::NOFALL) {
                now = Bits::new(nphys + nv);
            }
            for id in now.iter().take_while(|id| *id < nphys) {
                live_phys[first_inst[bi] + k].insert(PReg(id as u8));
            }
            let (mut defs, mut uses) = (Vec::new(), Vec::new());
            visit_regs(inst, |r, a| {
                let id = reg_id(r, nphys);
                match a {
                    Acc::Use => uses.push(id),
                    Acc::Def => defs.push((id, p + 1)),
                    Acc::EarlyDef => defs.push((id, p)),
                }
            });
            for (id, pos) in defs {
                set_from(&mut ranges[id], pos);
                now.remove(id);
                if id >= nphys {
                    weight[id - nphys] += w;
                }
            }
            for id in uses {
                if !now.contains(id) {
                    add_range(&mut ranges[id], bs, p + 1);
                    now.insert(id);
                }
                if id >= nphys {
                    weight[id - nphys] += w;
                }
            }
            if inst.flags.has(IFlags::CALL) {
                calls.push(Call {
                    pos: p,
                    weight: w,
                    clobbers: inst.clobbers,
                    inst: first_inst[bi] + k,
                });
            }
            let pairs: Vec<(&Operand, &Operand)> = match inst.op {
                COPY => vec![(&inst.ops[0], &inst.ops[1])],
                PARCOPY => {
                    let n = inst.ops.len() / 2;
                    inst.ops[..n].iter().zip(&inst.ops[n..]).collect()
                }
                _ => Vec::new(),
            };
            for (d, s) in pairs {
                if let (Operand::Reg(d, _), Operand::Reg(s, _)) = (d, s) {
                    if let Reg::Virt(v) = d {
                        hints[*v as usize].push((*s, w));
                    }
                    if let Reg::Virt(v) = s {
                        hints[*v as usize].push((*d, w));
                    }
                }
            }
        }
    }
    for r in &mut ranges {
        r.reverse();
    }
    calls.sort_by_key(|c| c.pos);

    // Assignment, heaviest first.
    let mut occupied: Vec<Vec<(u32, u32)>> = ranges[..nphys].to_vec();
    let mut assign: Vec<Option<PReg>> = vec![None; nv];
    let mut slot: Vec<Option<FrameId>> = vec![None; nv];
    let mut used_callee = RegSet::default();
    let mut order: Vec<usize> = (0..nv).filter(|v| !ranges[nphys + v].is_empty()).collect();
    order.sort_by_key(|v| (std::cmp::Reverse(weight[*v]), *v));
    let mut splits: Vec<(u32, PReg, Vec<usize>)> = Vec::new();
    for v in order {
        let c = f.vclass[v];
        let all = t.allocatable(c);
        let cands = &all[..all.len().min(limit)];
        let cross = crossing(&ranges[nphys + v], &calls);
        let mut best: Option<((i64, usize), u64, PReg)> = None;
        for &r in cands {
            if overlaps(&ranges[nphys + v], &occupied[r.0 as usize]) {
                continue;
            }
            let mut cost = 0u64;
            if t.regs()[r.0 as usize].callee_saved && !used_callee.contains(r) {
                cost += 2;
            }
            for &ci in &cross {
                if calls[ci].clobbers.contains(r) {
                    cost += 2 * calls[ci].weight;
                }
            }
            let bonus: u64 = hints[v]
                .iter()
                .filter(|(o, _)| match o {
                    Reg::Phys(p) => *p == r,
                    Reg::Virt(u) => assign[*u as usize] == Some(r),
                })
                .map(|(_, w)| *w)
                .sum();
            // On a tie the register that needs no stores around calls wins.
            let hits = cross
                .iter()
                .filter(|&&ci| calls[ci].clobbers.contains(r))
                .count();
            let key = (cost as i64 - bonus as i64, hits);
            if best.is_none_or(|(k, ..)| key < k) {
                best = Some((key, cost, r));
            }
        }
        match best {
            Some((_, cost, r)) if cost <= weight[v] => {
                assign[v] = Some(r);
                if t.regs()[r.0 as usize].callee_saved {
                    used_callee.insert(r);
                }
                for &rg in &ranges[nphys + v] {
                    let i = occupied[r.0 as usize].partition_point(|x| x.0 < rg.0);
                    occupied[r.0 as usize].insert(i, rg);
                }
                let hit: Vec<usize> = cross
                    .into_iter()
                    .filter(|&ci| calls[ci].clobbers.contains(r))
                    .collect();
                if !hit.is_empty() {
                    stats.split_around_calls += 1;
                    splits.push((v as u32, r, hit));
                }
            }
            _ => {
                slot[v] = Some(f.new_frame(c.size(), c.size(), FrameKind::Spill));
                stats.spilled += 1;
            }
        }
    }
    f.used_callee_saved = used_callee;

    // The stores and loads around calls, by instruction.
    let mut home: HashMap<u32, FrameId> = HashMap::new();
    let mut around: HashMap<usize, Vec<(u32, PReg, FrameId)>> = HashMap::new();
    for (v, r, hit) in splits {
        let c = f.vclass[v as usize];
        let h = *home
            .entry(v)
            .or_insert_with(|| f.new_frame(c.size(), c.size(), FrameKind::Spill));
        for ci in hit {
            around.entry(calls[ci].inst).or_default().push((v, r, h));
        }
    }

    // Rewriting.
    let loc = |r: Reg, f: &MFunc| -> Loc {
        match r {
            Reg::Phys(p) => Loc::Reg(p),
            Reg::Virt(v) => match assign[v as usize] {
                Some(p) => Loc::Reg(p),
                None => Loc::Slot(
                    slot[v as usize].expect("a spilled register has a slot"),
                    f.vclass[v as usize],
                ),
            },
        }
    };
    let mut gidx = 0usize;
    for bi in 0..nb {
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let mut out: Vec<MInst> = Vec::with_capacity(old.len());
        for inst in old {
            let idx = gidx;
            gidx += 1;
            if inst.op == COPY || inst.op == PARCOPY {
                let n = if inst.op == COPY {
                    1
                } else {
                    inst.ops.len() / 2
                };
                let mut pend = Vec::new();
                for k in 0..n {
                    let (Operand::Reg(d, _), s) = (&inst.ops[k], &inst.ops[n + k]) else {
                        unreachable!("a copy writes a register")
                    };
                    let s = match s {
                        Operand::Reg(r, _) => loc(*r, f),
                        Operand::Imm(x) => Loc::Imm(*x),
                        Operand::Frame(id, disp) => Loc::Addr(*id, *disp),
                        o => unreachable!("copy from {o:?}"),
                    };
                    pend.push((loc(*d, f), s));
                }
                let total = pend.iter().filter(|(d, s)| d != s).count();
                let live = live_phys[idx];
                emit_moves(t, pend, live, &mut out);
                stats.copies_removed += n - total.min(n);
                continue;
            }
            let mut inst = inst;
            // Registers this instruction names, to keep the scratch registers off them.
            let mut busy: Vec<PReg> = live_phys[idx].iter().chain(inst.clobbers.iter()).collect();
            let mut spilled: Vec<(u32, bool, bool)> = Vec::new();
            visit_regs(&inst, |r, a| match r {
                Reg::Phys(p) => busy.push(p),
                Reg::Virt(v) => match assign[v as usize] {
                    Some(p) => busy.push(p),
                    None => {
                        let (u, d) = (a == Acc::Use, a != Acc::Use);
                        match spilled.iter_mut().find(|s| s.0 == v) {
                            Some(s) => {
                                s.1 |= u;
                                s.2 |= d;
                            }
                            None => spilled.push((v, u, d)),
                        }
                    }
                },
            });
            let mut picked: Vec<(u32, PReg)> = Vec::new();
            let mut after: Vec<MInst> = Vec::new();
            if let Some(list) = around.get(&idx) {
                for &(v, r, h) in list {
                    let c = f.vclass[v as usize];
                    out.push(t.store(c, h, r));
                    after.push(t.load(c, r, h));
                }
            }
            for &(v, used, defd) in &spilled {
                let c = f.vclass[v as usize];
                let s = *t
                    .scratch(c)
                    .iter()
                    .find(|s| !busy.contains(s) && !picked.iter().any(|p| p.1 == **s))
                    .unwrap_or_else(|| {
                        panic!(
                            "mir `{}`: not enough scratch registers for {:?}",
                            f.name, inst
                        )
                    });
                picked.push((v, s));
                let fr = slot[v as usize].expect("a spilled register has a slot");
                if used {
                    out.push(t.load(c, s, fr));
                }
                if defd {
                    after.insert(0, t.store(c, fr, s));
                }
            }
            let map = |r: &mut Reg| {
                if let Reg::Virt(v) = *r {
                    *r = Reg::Phys(match assign[v as usize] {
                        Some(p) => p,
                        None => picked.iter().find(|p| p.0 == v).expect("picked").1,
                    });
                }
            };
            for o in &mut inst.ops {
                match o {
                    Operand::Reg(r, _) => map(r),
                    Operand::Mem { base, index, .. } => {
                        if let MemBase::Reg(b) = base {
                            map(b);
                        }
                        if let Some((x, _)) = index {
                            map(x);
                        }
                    }
                    _ => {}
                }
            }
            out.push(inst);
            out.extend(after);
        }
        f.blocks[bi].insts = out;
    }
    stats
}
