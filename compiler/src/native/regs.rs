//! What the instruction selectors share: the memory-operand folds (`folds`), the compare-and-branch
//! fusion (`fused_cmp`), the operands an instruction reads and the successors of a terminator. The
//! register assignment itself is `ralloc.rs`, working on the machine IR.

use crate::ir::{BinOp, CmpOp, Func, IrTy, Op, Term, Value};

/// The most bytes a `memcopy` of a constant size copies with plain moves instead of `rep movsb`.
pub const SMALL_COPY: i64 = 64;

/// A `memcopy` of a known small size.
pub fn small_copy(len: Option<i64>) -> bool {
    matches!(len, Some(n) if (0..=SMALL_COPY).contains(&n))
}

/// What an address is made of when no register holds it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Base {
    Value(Value),
    /// The frame object made by the `Slot` instruction with this number.
    Slot(usize),
}

/// A pointer value the generator never computes into a register: the instructions that read or write
/// through it name the address in their memory operand (`[base + index * scale + disp]`), and
/// anything else that needs the pointer itself gets it with one `lea`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fold {
    No,
    /// The result of a `Slot`, of a `PtrAdd` of a constant or an index to such an address.
    Addr {
        base: Base,
        index: Option<(Value, u8)>,
        disp: i32,
    },
    /// `idx * scale` (a `mul` or `shl` by a constant), only as the index of an `Addr`.
    Scaled {
        idx: Value,
        scale: u8,
    },
}

impl Fold {
    /// A frame object plus a constant: an address that needs no register at all, wherever it is used.
    pub fn is_frame(&self) -> bool {
        matches!(
            self,
            Fold::Addr {
                base: Base::Slot(_),
                index: None,
                ..
            }
        )
    }
}

/// Finds the pointers that can stay unnamed. A `Slot` always can. A `PtrAdd` (and a `mul` or `shl`
/// that scales its index) can when every use of its result is the address of a load or store, the
/// address of a small `memcopy`, or the base of another such `PtrAdd`; the registers it would have
/// read are then read where those uses are, so they must be live there (`assign` does that).
pub fn folds(f: &Func, uses: &[u32], consts: &[Option<i64>]) -> Vec<Fold> {
    let nv = f.values.len();
    let is64 = |v: Value| f.ty(v) == IrTy::Int(64);
    // Which values are candidates, until a use rules them out.
    let mut on = vec![false; nv];
    let mut scaled = vec![None; nv];
    for b in &f.blocks {
        for &i in &b.insts {
            let Some(r) = f.insts[i].result else { continue };
            match &f.insts[i].op {
                Op::PtrAdd(..) => on[r.0 as usize] = true,
                Op::Bin(op @ (BinOp::Mul | BinOp::Shl), x, y)
                    if is64(r) && uses[r.0 as usize] == 1 =>
                {
                    let k = |c: Value| consts[c.0 as usize];
                    let (idx, scale) = match (op, k(*x), k(*y)) {
                        (BinOp::Mul, None, Some(c)) => (*x, c),
                        (BinOp::Mul, Some(c), None) => (*y, c),
                        (BinOp::Shl, None, Some(c)) if (0..=3).contains(&c) => (*x, 1 << c),
                        _ => continue,
                    };
                    if matches!(scale, 1 | 2 | 4 | 8) {
                        scaled[r.0 as usize] = Some((idx, scale as u8));
                        on[r.0 as usize] = true;
                    }
                }
                _ => {}
            }
        }
    }
    loop {
        let mut fold = vec![Fold::No; nv];
        let mut changed = false;
        for b in &f.blocks {
            for &i in &b.insts {
                let Some(r) = f.insts[i].result else { continue };
                let ri = r.0 as usize;
                match &f.insts[i].op {
                    Op::Slot(..) => {
                        fold[ri] = Fold::Addr {
                            base: Base::Slot(i),
                            index: None,
                            disp: 0,
                        }
                    }
                    Op::PtrAdd(a, c) if on[ri] => {
                        let (base, index, disp) = match fold[a.0 as usize] {
                            Fold::Addr { base, index, disp } => (base, index, disp),
                            _ => (Base::Value(*a), None, 0),
                        };
                        let made = match (consts[c.0 as usize], fold[c.0 as usize]) {
                            (Some(k), _) => i32::try_from(disp as i64 + k)
                                .ok()
                                .map(|disp| Fold::Addr { base, index, disp }),
                            (None, Fold::Scaled { idx, scale }) if index.is_none() => {
                                Some(Fold::Addr {
                                    base,
                                    index: Some((idx, scale)),
                                    disp,
                                })
                            }
                            (None, Fold::No) if index.is_none() && is64(*c) => Some(Fold::Addr {
                                base,
                                index: Some((*c, 1)),
                                disp,
                            }),
                            _ => None,
                        };
                        match made {
                            Some(m) => fold[ri] = m,
                            None => {
                                on[ri] = false;
                                changed = true;
                            }
                        }
                    }
                    Op::Bin(..) if on[ri] => {
                        if let Some((idx, scale)) = scaled[ri] {
                            fold[ri] = Fold::Scaled { idx, scale };
                        }
                    }
                    _ => {}
                }
            }
        }
        // A folded value with a use that needs a register cannot stay folded.
        let mut bad = vec![false; nv];
        let mut needs = |v: Value| bad[v.0 as usize] = true;
        for b in &f.blocks {
            for &i in &b.insts {
                let inst = &f.insts[i];
                let r_folded = inst.result.is_some_and(|r| fold[r.0 as usize] != Fold::No);
                match &inst.op {
                    Op::Load(..) => {}
                    Op::Store(_, x) => needs(*x),
                    Op::MemCopy(d, s, n) => {
                        needs(*n);
                        if !small_copy(consts[n.0 as usize]) {
                            needs(*d);
                            needs(*s);
                        }
                    }
                    Op::PtrAdd(a, c) if !r_folded => {
                        needs(*a);
                        needs(*c);
                    }
                    // Read where the folded result is used.
                    Op::PtrAdd(..) => {}
                    Op::Bin(..) if r_folded => {}
                    op => {
                        let mut v = Vec::new();
                        inst_uses(op, &mut v);
                        v.into_iter().for_each(&mut needs);
                    }
                }
            }
            if let Some(t) = &b.term {
                let mut v = Vec::new();
                term_uses(t, &mut v);
                v.into_iter().for_each(&mut needs);
            }
        }
        for v in 0..nv {
            if bad[v] && on[v] && fold[v] != Fold::No && !fold[v].is_frame() {
                on[v] = false;
                changed = true;
            }
        }
        if !changed {
            return fold;
        }
    }
}

/// The values an instruction really reads at its position: a folded pointer is read through the
/// registers its address is made of.
/// The compare that ends a block and only feeds the block's branch: the branch tests the flags
/// directly and the comparison is never stored. Gives the instruction.
pub fn fused_cmp(f: &Func, bi: usize, uses: &[u32]) -> Option<usize> {
    let b = &f.blocks[bi];
    let (&last, Some(Term::Branch(c, ..))) = (b.insts.last()?, &b.term) else {
        return None;
    };
    let Op::Cmp(_, x, _) = &f.insts[last].op else {
        return None;
    };
    // A float `==` or `!=` needs the parity flag as well as the zero flag: it is not fused.
    let flags_only = !matches!(f.ty(*x), IrTy::F32 | IrTy::F64)
        || matches!(
            &f.insts[last].op,
            Op::Cmp(
                CmpOp::Lt
                    | CmpOp::ULt
                    | CmpOp::Le
                    | CmpOp::ULe
                    | CmpOp::Gt
                    | CmpOp::UGt
                    | CmpOp::Ge
                    | CmpOp::UGe,
                ..
            )
        );
    (f.insts[last].result == Some(*c) && uses[c.0 as usize] == 1 && flags_only).then_some(last)
}

pub(super) fn inst_uses(op: &Op, out: &mut Vec<Value>) {
    match op {
        Op::Const(..) | Op::Slot(..) | Op::DataAddr(_) | Op::FuncAddr(_) => {}
        Op::Neg(a)
        | Op::FUn(_, a)
        | Op::Not(a)
        | Op::VUn(_, a)
        | Op::Cast(_, a, _)
        | Op::Load(_, a)
        | Op::Alloc(a)
        | Op::Free(a) => out.push(*a),
        Op::Bin(_, a, b)
        | Op::Checked(_, _, a, b)
        | Op::Cmp(_, a, b)
        | Op::VBin(_, a, b)
        | Op::PtrAdd(a, b)
        | Op::Store(a, b)
        | Op::Realloc(a, b)
        | Op::CtxSwitch(a, b)
        | Op::Print(a, b) => out.extend([*a, *b]),
        Op::CtxInit(a, b, c, d) => out.extend([*a, *b, *c, *d]),
        Op::Os(k, a, b, c) => out.extend(&[*a, *b, *c][..k.arity()]),
        Op::CheckedIf(_, _, test, a, b) => {
            test.for_each_value(|v| out.push(v));
            out.extend([*a, *b]);
        }
        Op::Select(a, b, c) | Op::Fma(a, b, c) | Op::MemCopy(a, b, c) => out.extend([*a, *b, *c]),
        Op::Call(_, args) => out.extend(args.iter().copied()),
        Op::CallPtr(_, _, p, args) => {
            out.push(*p);
            out.extend(args.iter().copied());
        }
    }
}

fn term_uses(t: &Term, out: &mut Vec<Value>) {
    match t {
        Term::Jump(tg) => out.extend(tg.args.iter().copied()),
        Term::Branch(c, a, b) => {
            out.push(*c);
            out.extend(a.args.iter().copied());
            out.extend(b.args.iter().copied());
        }
        Term::Return(Some(v)) => out.push(*v),
        _ => {}
    }
}

pub(super) fn successors(t: &Term) -> Vec<usize> {
    match t {
        Term::Jump(tg) => vec![tg.block.0 as usize],
        Term::Branch(_, a, b) => vec![a.block.0 as usize, b.block.0 as usize],
        _ => Vec::new(),
    }
}
