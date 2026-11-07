//! The heap written in the language (`core/heap.pel`, docs/design/heap.md stage H6): `Alloc`, `Free` and `Realloc`
//! become calls to `heap_alloc`, `heap_free` and `heap_realloc`, with a cast between the pointer and the whole number
//! the library functions use. Run after the optimiser (which knows `Alloc` and `Free`, for example to move a small
//! local block to the stack) and before `prune`, so the heap functions and what they call stay.

use crate::ir::*;

/// The names of the three library functions, in the order allocate, free, reallocate.
pub const HEAP_FNS: [&str; 3] = ["heap_alloc", "heap_free", "heap_realloc"];

/// Rewrites every function but the heap's own. Does nothing when the module has no heap functions (the program was not
/// lowered for the language heap). Gives whether anything changed.
pub fn expand_heap(m: &mut Module) -> bool {
    let find = |m: &Module, n: &str| {
        m.funcs
            .iter()
            .position(|f| f.name == n)
            .map(|i| FuncId(i as u32))
    };
    let (Some(alloc), Some(free), Some(realloc)) = (
        find(m, HEAP_FNS[0]),
        find(m, HEAP_FNS[1]),
        find(m, HEAP_FNS[2]),
    ) else {
        return false;
    };
    let mut changed = false;
    for f in &mut m.funcs {
        if f.name.starts_with("heap_") {
            continue;
        }
        changed |= expand_func(f, alloc, free, realloc);
    }
    changed
}

fn expand_func(f: &mut Func, alloc: FuncId, free: FuncId, realloc: FuncId) -> bool {
    let mut changed = false;
    for bi in 0..f.blocks.len() {
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let mut out = Vec::with_capacity(old.len());
        for i in old {
            let span = f.insts[i].span;
            match f.insts[i].op.clone() {
                Op::Alloc(n) => {
                    let t = f.new_value(IrTy::I64);
                    out.push(push(f, Some(t), Op::Call(alloc, vec![n]), span));
                    f.insts[i].op = Op::Cast(CastKind::IntToPtr, t, IrTy::Ptr);
                    out.push(i);
                    changed = true;
                }
                Op::Free(p) => {
                    let t = f.new_value(IrTy::I64);
                    out.push(push(
                        f,
                        Some(t),
                        Op::Cast(CastKind::PtrToInt, p, IrTy::I64),
                        span,
                    ));
                    f.insts[i].op = Op::Call(free, vec![t]);
                    out.push(i);
                    changed = true;
                }
                Op::Realloc(p, n) => {
                    let (a, b) = (f.new_value(IrTy::I64), f.new_value(IrTy::I64));
                    out.push(push(
                        f,
                        Some(a),
                        Op::Cast(CastKind::PtrToInt, p, IrTy::I64),
                        span,
                    ));
                    out.push(push(f, Some(b), Op::Call(realloc, vec![a, n]), span));
                    f.insts[i].op = Op::Cast(CastKind::IntToPtr, b, IrTy::Ptr);
                    out.push(i);
                    changed = true;
                }
                _ => out.push(i),
            }
        }
        f.blocks[bi].insts = out;
    }
    changed
}

fn push(f: &mut Func, result: Option<Value>, op: Op, span: Span) -> usize {
    f.insts.push(Inst { result, op, span });
    f.insts.len() - 1
}
