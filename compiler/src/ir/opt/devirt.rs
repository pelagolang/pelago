//! Calls through a pointer that is the address of a known function (card A-11). The maps and sets
//! of the library call the hash and the equality of their key type through pointers
//! (`rt_call_int1(hash, key)`); where the pointer is made in the same function (`rt_hash_fn<K>()`)
//! the call goes straight to the function, which the inliner can then take in.

use crate::ir::*;

pub fn run(m: &mut Module) -> bool {
    let sigs: Vec<(Vec<IrTy>, IrTy)> = m.funcs.iter().map(|f| (f.params.clone(), f.ret)).collect();
    let mut changed = false;
    for f in &mut m.funcs {
        let defs = f.defs();
        for bi in 0..f.blocks.len() {
            for k in 0..f.blocks[bi].insts.len() {
                let ii = f.blocks[bi].insts[k];
                let Op::CallPtr(_, ret, p, args) = f.insts[ii].op.clone() else {
                    continue;
                };
                let Some(Op::FuncAddr(g)) = defs[p.0 as usize].map(|d| f.insts[d].op.clone())
                else {
                    continue;
                };
                let (params, gret) = &sigs[g.0 as usize];
                if *gret != ret
                    || params.len() != args.len()
                    || params
                        .iter()
                        .zip(&args)
                        .any(|(t, a)| *t != f.values[a.0 as usize])
                {
                    continue;
                }
                f.insts[ii].op = Op::Call(g, args);
                changed = true;
            }
        }
    }
    changed
}
