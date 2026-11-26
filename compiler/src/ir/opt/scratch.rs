//! Scratch memory: a small heap block that never leaves its function lives on the stack instead.
//!
//! An `Alloc` of a constant, small size whose address is only used to load, to store, to copy to
//! or from, to add an offset (which is then held to the same rule) and to `Free` becomes a `Slot`,
//! and its `Free`s go. A slot is zero where it comes into use, as a fresh block is, and the address
//! cannot outlive a pass through the `Alloc` (it is not passed on in a jump, stored, returned or
//! given to a call), so reusing the same stack bytes on the next pass through a loop is safe.
//! Programs whose temporaries all stay local then need no allocator at all, and `promote` can turn
//! the slot into registers.

use super::fx::HashSet;
use crate::ir::*;

/// The largest block moved to the stack, and the most bytes a function moves in all: a deep
/// recursion or a small task stack must not pay for what the heap held before.
const MAX_BLOCK: i128 = 128;
const MAX_TOTAL: i128 = 512;

pub fn run(f: &mut Func) -> bool {
    let defs = f.defs();
    let mut cands: Vec<(usize, Value, u32)> = Vec::new();
    for b in &f.blocks {
        for &i in &b.insts {
            let (Op::Alloc(n), Some(r)) = (&f.insts[i].op, f.insts[i].result) else {
                continue;
            };
            let Some(d) = defs[n.0 as usize] else {
                continue;
            };
            if let Op::Const(_, Const::Int(size)) = f.insts[d].op {
                if (1..=MAX_BLOCK).contains(&size) {
                    cands.push((i, r, size as u32));
                }
            }
        }
    }
    if cands.is_empty() {
        return false;
    }
    let mut total = 0i128;
    let mut dead: HashSet<usize> = HashSet::default();
    let mut convert: Vec<(usize, u32)> = Vec::new();
    for (i, r, size) in cands {
        if total + size as i128 > MAX_TOTAL || !stays_local(f, r, &mut dead) {
            continue;
        }
        total += size as i128;
        convert.push((i, size));
    }
    if convert.is_empty() {
        return false;
    }
    for (i, size) in convert {
        f.insts[i].op = Op::Slot(size.div_ceil(16) * 16, 16);
    }
    for b in &mut f.blocks {
        b.insts.retain(|i| !dead.contains(i));
    }
    true
}

/// Whether the address `r` (and what is added to it) is only used as described above; the `Free`s
/// found are added to `dead` when it is.
fn stays_local(f: &Func, r: Value, dead: &mut HashSet<usize>) -> bool {
    let mut derived: HashSet<Value> = HashSet::default();
    derived.insert(r);
    loop {
        let before = derived.len();
        for b in &f.blocks {
            for &i in &b.insts {
                if let (Op::PtrAdd(a, n), Some(res)) = (&f.insts[i].op, f.insts[i].result) {
                    if derived.contains(n) {
                        return false;
                    }
                    if derived.contains(a) {
                        derived.insert(res);
                    }
                }
            }
        }
        if derived.len() == before {
            break;
        }
    }
    let mut frees: Vec<usize> = Vec::new();
    for b in &f.blocks {
        for &i in &b.insts {
            let ok = match &f.insts[i].op {
                Op::Load(..) => true,
                Op::Store(_, v) => !derived.contains(v),
                Op::MemCopy(_, _, n) => !derived.contains(n),
                Op::PtrAdd(..) => true,
                Op::Free(p) => {
                    if derived.contains(p) {
                        // Only the block's own address is freed.
                        if *p != r {
                            return false;
                        }
                        frees.push(i);
                    }
                    true
                }
                op => {
                    let mut hit = false;
                    op.for_each_value(|v| hit |= derived.contains(&v));
                    !hit
                }
            };
            if !ok {
                return false;
            }
        }
        if let Some(t) = &b.term {
            let mut hit = false;
            t.for_each_value(|v| hit |= derived.contains(&v));
            if hit {
                return false;
            }
        }
    }
    dead.extend(frees);
    true
}
