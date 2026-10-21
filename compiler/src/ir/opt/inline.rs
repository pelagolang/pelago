//! Inlining: a call to a small function, or to one called from a single place, is replaced by a copy
//! of its body. Functions are visited callees first, so a body is already tidy when it is copied.
//! Recursive functions are never inlined into other functions; `recursive_once` copies a small one into
//! itself, one level. Slots of the callee become slots of the caller, so
//! promotion can then turn them into registers.

use crate::ir::*;
use std::collections::{HashMap, HashSet};

/// A function whose name ends like this is the rarely taken half of a fast path (`rt_grow_slow`:
/// the buffer is full). It stays a call, so the fast path around it stays small enough to be copied
/// into every caller and its callers stay free of the call.
/// Functions with this ending run rarely: never inlined, and calls to them weigh little in register allocation.
pub const COLD_SUFFIX: &str = "_slow";

/// A callee at most this big (instructions plus two per block) is always copied.
const SMALL: usize = 60;

/// A self-recursive function is copied into itself only up to this cost, and at this many calls.
const RECURSIVE_LIMIT: usize = 120;
const RECURSIVE_SITES: usize = 3;
/// ... and the function may grow by this much (instructions plus two per block).
const RECURSIVE_GROWTH: usize = 200;

pub fn run(m: &mut Module) -> bool {
    let n = m.funcs.len();
    let calls: Vec<Vec<usize>> = m.funcs.iter().map(callees_of).collect();
    let recursive = recursive_funcs(&calls);
    let mut sites = vec![0usize; n];
    for cs in &calls {
        for &c in cs {
            sites[c] += 1;
        }
    }
    let mut changed = false;
    for fi in postorder(&calls) {
        while let Some((bi, pos, g)) = find_site(m, fi, &recursive, &sites) {
            let callee = m.funcs[g].clone();
            inline_at(&mut m.funcs[fi], bi, pos, &callee);
            changed = true;
        }
    }
    changed
}

fn callees_of(f: &Func) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for b in &f.blocks {
        for &i in &b.insts {
            if let Op::Call(c, _) = &f.insts[i].op {
                out.push(c.0 as usize);
            }
        }
    }
    out
}

fn cost(f: &Func) -> usize {
    f.blocks.iter().map(|b| b.insts.len()).sum::<usize>() + 2 * f.blocks.len()
}

/// The functions that can reach themselves through calls.
fn recursive_funcs(calls: &[Vec<usize>]) -> Vec<bool> {
    let n = calls.len();
    let mut rec = vec![false; n];
    for start in 0..n {
        let mut seen = vec![false; n];
        let mut stack: Vec<usize> = calls[start].clone();
        while let Some(x) = stack.pop() {
            if x == start {
                rec[start] = true;
                break;
            }
            if !seen[x] {
                seen[x] = true;
                stack.extend(calls[x].iter().copied());
            }
        }
    }
    rec
}

/// Callees before callers.
fn postorder(calls: &[Vec<usize>]) -> Vec<usize> {
    let n = calls.len();
    let mut seen = vec![false; n];
    let mut out = Vec::new();
    for s in 0..n {
        if seen[s] {
            continue;
        }
        // Iterative depth-first search with an explicit position per frame.
        let mut stack: Vec<(usize, usize)> = vec![(s, 0)];
        seen[s] = true;
        while let Some((x, i)) = stack.pop() {
            if i < calls[x].len() {
                stack.push((x, i + 1));
                let y = calls[x][i];
                if !seen[y] {
                    seen[y] = true;
                    stack.push((y, 0));
                }
            } else {
                out.push(x);
            }
        }
    }
    out
}

/// A call in function `fi` that should be inlined: block, position in the block, callee.
fn find_site(
    m: &Module,
    fi: usize,
    recursive: &[bool],
    sites: &[usize],
) -> Option<(usize, usize, usize)> {
    let f = &m.funcs[fi];
    let mut seen_bad: HashSet<usize> = HashSet::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (pos, &i) in b.insts.iter().enumerate() {
            let Op::Call(c, _) = &f.insts[i].op else {
                continue;
            };
            let g = c.0 as usize;
            if g == fi || recursive[g] || seen_bad.contains(&g) {
                continue;
            }
            if m.funcs[g].name.ends_with(COLD_SUFFIX) {
                continue;
            }
            let c = cost(&m.funcs[g]);
            if c <= SMALL || sites[g] == 1 {
                return Some((bi, pos, g));
            }
            seen_bad.insert(g);
        }
    }
    None
}

/// How many calls to itself the function makes.
pub fn self_calls(f: &Func, me: FuncId) -> usize {
    f.blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter(|&&i| matches!(&f.insts[i].op, Op::Call(c, _) if *c == me))
        .count()
}

/// A small self-recursive function gets one level of itself inlined at its own calls that do not
/// end the function (a call whose result is returned straight away is a tail call, or the slow path
/// of a guard, and stays): half the calls of `fib` disappear, and the copy sees the arguments as
/// they are, so the optimiser can fold what it knows about them. Returns whether it changed `f`.
pub fn recursive_once(f: &mut Func, me: FuncId) -> bool {
    if cost(f) > RECURSIVE_LIMIT {
        return false;
    }
    let body = f.clone();
    let sites: Vec<usize> = f
        .blocks
        .iter()
        .filter(|b| !matches!(b.term, Some(Term::Return(_))))
        .flat_map(|b| b.insts.iter().copied())
        .filter(|&i| matches!(&f.insts[i].op, Op::Call(c, _) if *c == me))
        .collect();
    let mut changed = false;
    let grown = cost(f) + RECURSIVE_GROWTH;
    for id in sites.into_iter().take(RECURSIVE_SITES) {
        // Each copy is about the size of the function: stop at the budget.
        if cost(f) + cost(&body) > grown {
            break;
        }
        let found = f
            .blocks
            .iter()
            .enumerate()
            .find_map(|(bi, b)| b.insts.iter().position(|&i| i == id).map(|pos| (bi, pos)));
        if let Some((bi, pos)) = found {
            inline_at(f, bi, pos, &body);
            changed = true;
        }
    }
    changed
}

/// Replaces the call at `pos` in block `bi` with a copy of `callee`.
fn inline_at(f: &mut Func, bi: usize, pos: usize, callee: &Func) {
    let call_id = f.blocks[bi].insts[pos];
    let (result, args) = match &f.insts[call_id] {
        Inst {
            result,
            op: Op::Call(_, args),
            ..
        } => (*result, args.clone()),
        _ => return,
    };
    // The code after the call moves to a new block that the callee's returns jump to. The call's
    // result becomes that block's parameter, so its uses stay valid.
    let after: Vec<usize> = f.blocks[bi].insts.split_off(pos + 1);
    f.blocks[bi].insts.pop();
    let term = f.blocks[bi].term.take();
    let cont = f.new_block(&[]);
    if let Some(r) = result {
        f.blocks[cont.0 as usize].params.push(r);
    }
    f.blocks[cont.0 as usize].insts = after;
    f.blocks[cont.0 as usize].term = term;
    // Copy the callee.
    let mut vmap: HashMap<Value, Value> = HashMap::new();
    for (p, a) in callee.blocks[0].params.iter().zip(&args) {
        vmap.insert(*p, *a);
    }
    let mut bmap: Vec<BlockId> = Vec::new();
    for (i, b) in callee.blocks.iter().enumerate() {
        let id = f.new_block(&[]);
        bmap.push(id);
        if i == 0 {
            continue;
        }
        for p in &b.params {
            let nv = f.new_value(callee.ty(*p));
            vmap.insert(*p, nv);
            f.blocks[id.0 as usize].params.push(nv);
        }
    }
    // Results of instructions get fresh values first (a use may come before the definition in block order).
    for b in &callee.blocks {
        for &i in &b.insts {
            if let Some(r) = callee.insts[i].result {
                let nv = f.new_value(callee.ty(r));
                vmap.insert(r, nv);
            }
        }
    }
    let map_v = |v: Value| *vmap.get(&v).unwrap_or(&v);
    for (i, b) in callee.blocks.iter().enumerate() {
        let id = bmap[i].0 as usize;
        for &ii in &b.insts {
            let mut inst = callee.insts[ii].clone();
            inst.op.map_values(map_v);
            inst.result = inst.result.map(map_v);
            f.insts.push(inst);
            let new_id = f.insts.len() - 1;
            f.blocks[id].insts.push(new_id);
        }
        let t = match b.term.clone() {
            Some(Term::Return(v)) => Term::Jump(Target {
                block: cont,
                args: v
                    .map(map_v)
                    .into_iter()
                    .filter(|_| result.is_some())
                    .collect(),
            }),
            Some(mut t) => {
                t.map_values(map_v);
                for tg in t.targets_mut() {
                    tg.block = bmap[tg.block.0 as usize];
                }
                t
            }
            None => Term::Unreachable,
        };
        f.blocks[id].term = Some(t);
    }
    f.blocks[bi].term = Some(Term::Jump(Target {
        block: bmap[0],
        args: Vec::new(),
    }));
}

/// A program that starts no task has none to finish when `main` ends: takes out the calls of
/// `rt_run_pending` (the library function that runs the tasks nobody waited for) when no function
/// reachable from `main` starts one, so the task queue, and what it needs of the heap, stay out of
/// the program. Tasks start in `spawn`, whose instances call `rt_task_body`.
pub fn drop_idle_task_drain(m: &mut Module) {
    let id_of = |m: &Module, name: &str| m.funcs.iter().position(|f| f.name == name);
    let (Some(main), Some(drain)) = (id_of(m, "main"), id_of(m, "rt_run_pending")) else {
        return;
    };
    let mut seen = vec![false; m.funcs.len()];
    seen[main] = true;
    let mut stack = vec![main];
    while let Some(x) = stack.pop() {
        if m.funcs[x].name.starts_with("rt_task_body<") {
            return;
        }
        for b in &m.funcs[x].blocks {
            for &i in &b.insts {
                let t = match &m.funcs[x].insts[i].op {
                    Op::Call(c, _) | Op::FuncAddr(c) => c.0 as usize,
                    _ => continue,
                };
                if t != drain && !seen[t] {
                    seen[t] = true;
                    stack.push(t);
                }
            }
        }
    }
    for f in &mut m.funcs {
        let insts = &f.insts;
        for b in &mut f.blocks {
            b.insts
                .retain(|&i| !matches!(&insts[i].op, Op::Call(c, _) if c.0 as usize == drain));
        }
    }
}

/// Removes the functions that cannot be reached from the named roots (by a call or a function
/// address) and renumbers the rest.
pub fn prune(m: &mut Module, roots: &[&str]) {
    let n = m.funcs.len();
    let mut keep = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    for (i, f) in m.funcs.iter().enumerate() {
        if roots.contains(&f.name.as_str()) {
            keep[i] = true;
            stack.push(i);
        }
    }
    while let Some(x) = stack.pop() {
        for b in &m.funcs[x].blocks {
            for &i in &b.insts {
                let target = match &m.funcs[x].insts[i].op {
                    Op::Call(c, _) => Some(c.0 as usize),
                    Op::FuncAddr(c) => Some(c.0 as usize),
                    _ => None,
                };
                if let Some(t) = target {
                    if !keep[t] {
                        keep[t] = true;
                        stack.push(t);
                    }
                }
            }
        }
    }
    let mut map = vec![u32::MAX; n];
    let mut next = 0u32;
    for (i, k) in keep.iter().enumerate() {
        if *k {
            map[i] = next;
            next += 1;
        }
    }
    let old = std::mem::take(&mut m.funcs);
    for (i, mut f) in old.into_iter().enumerate() {
        if !keep[i] {
            continue;
        }
        for inst in &mut f.insts {
            match &mut inst.op {
                Op::Call(c, _) => *c = FuncId(map[c.0 as usize]),
                Op::FuncAddr(c) => *c = FuncId(map[c.0 as usize]),
                _ => {}
            }
        }
        m.funcs.push(f);
    }
}
