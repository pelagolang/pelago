//! Common subexpressions (card A-11): when an operation is computed again on the same operands and an
//! earlier copy dominates it, the later one reuses the earlier result. `n % 10` and `n / 10` each
//! become a high multiply and shifts; after this pass they share the multiply.
//!
//! Only operations that depend on nothing but their operands are joined: arithmetic (checked or not),
//! comparisons, negation and casts. Loads, calls, slots and allocations are
//! never joined. Joining a checked operation is exact: the earlier copy dominates, so if the later
//! one would stop the program the earlier one already did. Equal constants count as the same
//! operand, so `mulhi x, 0x6666..` written twice with two constants is one operation. Also
//! `or x, x` and `and x, x` are `x`.

use super::dom::Dom;
use crate::ir::*;
use std::collections::HashMap;
use std::mem::{discriminant, Discriminant};

#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Bin(Discriminant<BinOp>, u64, u64),
    Checked(Discriminant<BinOp>, bool, u64, u64),
    Cmp(Discriminant<CmpOp>, u64, u64),
    Neg(u64),
    Not(u64),
    Cast(Discriminant<CastKind>, IrTy, u64),
}

/// A constant, as something that can be a hash key.
type ConstKey = (IrTy, u8, i128);

fn const_key(ty: IrTy, c: Const) -> ConstKey {
    match c {
        Const::Int(n) => (ty, 0, n),
        Const::Float(x) => (ty, 1, x.to_bits() as i128),
        Const::Bool(b) => (ty, 2, b as i128),
    }
}

fn commutative(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Add | BinOp::Mul | BinOp::And | BinOp::Or | BinOp::Xor
    )
}

fn ordered(a: u64, b: u64) -> (u64, u64) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

pub fn run(f: &mut Func) -> bool {
    let dom = Dom::new(f);
    let defs = f.defs();
    // Operand identity: a constant is named by its value, anything else by its number.
    let mut consts: HashMap<ConstKey, u64> = HashMap::new();
    let mut name: Vec<u64> = (0..f.values.len() as u64).collect();
    for (v, d) in defs.iter().enumerate() {
        if let Some(d) = d {
            if let Op::Const(ty, c) = &f.insts[*d].op {
                let next = (1u64 << 40) + consts.len() as u64;
                name[v] = *consts.entry(const_key(*ty, *c)).or_insert(next);
            }
        }
    }
    let mut rep: HashMap<Value, Value> = HashMap::new();
    let mut gone: Vec<usize> = Vec::new();
    let mut seen: HashMap<Key, Vec<(Value, BlockId)>> = HashMap::new();
    for &b in &dom.rpo {
        for pos in 0..f.blocks[b.0 as usize].insts.len() {
            let ii = f.blocks[b.0 as usize].insts[pos];
            let Some(r) = f.insts[ii].result else {
                continue;
            };
            let get = |v: Value, rep: &HashMap<Value, Value>| {
                let v = *rep.get(&v).unwrap_or(&v);
                (v, name[v.0 as usize])
            };
            let key = match &f.insts[ii].op {
                Op::Bin(o, a, c) => {
                    let ((a, na), (c, nc)) = (get(*a, &rep), get(*c, &rep));
                    if a == c && matches!(o, BinOp::And | BinOp::Or) {
                        rep.insert(r, a);
                        gone.push(ii);
                        continue;
                    }
                    let (x, y) = if commutative(*o) {
                        ordered(na, nc)
                    } else {
                        (na, nc)
                    };
                    Key::Bin(discriminant(o), x, y)
                }
                Op::Checked(o, s, a, c) => {
                    let ((_, na), (_, nc)) = (get(*a, &rep), get(*c, &rep));
                    let (x, y) = if commutative(*o) {
                        ordered(na, nc)
                    } else {
                        (na, nc)
                    };
                    Key::Checked(discriminant(o), *s, x, y)
                }
                Op::Cmp(o, a, c) => {
                    let ((_, na), (_, nc)) = (get(*a, &rep), get(*c, &rep));
                    let (x, y) = if matches!(o, CmpOp::Eq | CmpOp::Ne) {
                        ordered(na, nc)
                    } else {
                        (na, nc)
                    };
                    Key::Cmp(discriminant(o), x, y)
                }
                Op::Neg(a) => Key::Neg(get(*a, &rep).1),
                Op::Not(a) => Key::Not(get(*a, &rep).1),
                Op::Cast(k, a, ty) => Key::Cast(discriminant(k), *ty, get(*a, &rep).1),
                _ => continue,
            };
            let list = seen.entry(key).or_default();
            if let Some(&(w, _)) = list.iter().find(|(_, wb)| dom.dominates(*wb, b)) {
                rep.insert(r, w);
                gone.push(ii);
            } else {
                list.push((r, b));
            }
        }
    }
    if gone.is_empty() {
        return false;
    }
    f.replace_uses(&rep);
    let gone_set: std::collections::HashSet<usize> = gone.into_iter().collect();
    for blk in &mut f.blocks {
        blk.insts.retain(|i| !gone_set.contains(i));
    }
    true
}
