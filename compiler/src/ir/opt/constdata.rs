//! Copies out of constant data (card A-11): `s + "ab"` copies two bytes from the literal. When the
//! length is 1, 2, 4 or 8 and the source is the address of static data, the bytes are known, so the
//! copy becomes a store of one integer constant. That drops the load, and the address of the data
//! no longer has to live in a register.

use crate::ir::*;

pub fn run(m: &mut Module) -> bool {
    let mut changed = false;
    let Module { funcs, data } = m;
    for f in funcs.iter_mut() {
        changed |= copies_to_stores(f, data);
    }
    changed
}

fn copies_to_stores(f: &mut Func, data: &[Vec<u8>]) -> bool {
    let defs = f.defs();
    let mut edits: Vec<(usize, Value, u8, i128)> = Vec::new();
    for b in &f.blocks {
        for &k in &b.insts {
            let Op::MemCopy(d, s, n) = f.insts[k].op else {
                continue;
            };
            let Some(&Some(si)) = defs.get(s.0 as usize) else {
                continue;
            };
            let Op::DataAddr(i) = f.insts[si].op else {
                continue;
            };
            let Some(&Some(ni)) = defs.get(n.0 as usize) else {
                continue;
            };
            let Op::Const(_, Const::Int(len)) = f.insts[ni].op else {
                continue;
            };
            if !matches!(len, 1 | 2 | 4 | 8) {
                continue;
            }
            let Some(bytes) = data[i as usize].get(..len as usize) else {
                continue;
            };
            let mut le = [0u8; 8];
            le[..bytes.len()].copy_from_slice(bytes);
            let bits = len as u32 * 8;
            // Constants are kept sign extended, as the interpreter reads them.
            let v = (u64::from_le_bytes(le) as i64) << (64 - bits) >> (64 - bits);
            edits.push((k, d, bits as u8, v as i128));
        }
    }
    if edits.is_empty() {
        return false;
    }
    for (k, d, bits, v) in edits {
        let c = f.new_value(IrTy::Int(bits));
        let span = f.insts[k].span;
        f.insts.push(Inst {
            result: Some(c),
            op: Op::Const(IrTy::Int(bits), Const::Int(v)),
            span,
        });
        let ci = f.insts.len() - 1;
        f.insts[k].op = Op::Store(d, c);
        let b = f
            .blocks
            .iter_mut()
            .find(|b| b.insts.contains(&k))
            .expect("the copy is in a block");
        let at = b.insts.iter().position(|x| *x == k).unwrap();
        b.insts.insert(at, ci);
    }
    true
}
