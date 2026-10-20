//! Tests of the passes on hand-written IR. Every case also runs before and after and compares.

use super::*;
use crate::ir::{run_full, Val};

fn module_of(f: Func) -> Module {
    let mut m = Module::default();
    m.add(f);
    m
}

fn both(m: &Module) -> (Module, Val, Val) {
    let before = run_full(m, "main", &[]);
    let mut o = m.clone();
    optimize(&mut o, Options::default());
    assert!(verify_module(&o).is_empty(), "{}", print_module(&o));
    let after = run_full(&o, "main", &[]);
    assert_eq!(before.result, after.result);
    (
        o,
        before.result.unwrap_or(Val::Unit),
        after.result.unwrap_or(Val::Unit),
    )
}

fn count(f: &Func, pred: impl Fn(&Op) -> bool) -> usize {
    f.blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter(|i| pred(&f.insts[**i].op))
        .count()
}

/// `var t = 0; var i = 0; while i < 10 { t += i; i += 1 } t` with both variables in slots.
fn counting_loop() -> Func {
    let mut f = Func::new("main", &[], IrTy::I64);
    let e = f.entry();
    let head = f.new_block(&[]);
    let body = f.new_block(&[]);
    let done = f.new_block(&[]);
    let t = f.emit(e, Op::Slot(8, 8), Some(IrTy::Ptr)).unwrap();
    let i = f.emit(e, Op::Slot(8, 8), Some(IrTy::Ptr)).unwrap();
    let zero = f
        .emit(e, Op::Const(IrTy::I64, Const::Int(0)), Some(IrTy::I64))
        .unwrap();
    f.emit(e, Op::Store(t, zero), None);
    f.emit(e, Op::Store(i, zero), None);
    f.terminate(
        e,
        Term::Jump(Target {
            block: head,
            args: vec![],
        }),
    );
    let iv = f
        .emit(head, Op::Load(IrTy::I64, i), Some(IrTy::I64))
        .unwrap();
    let ten = f
        .emit(head, Op::Const(IrTy::I64, Const::Int(10)), Some(IrTy::I64))
        .unwrap();
    let c = f
        .emit(head, Op::Cmp(CmpOp::Lt, iv, ten), Some(IrTy::Bool))
        .unwrap();
    f.terminate(
        head,
        Term::Branch(
            c,
            Target {
                block: body,
                args: vec![],
            },
            Target {
                block: done,
                args: vec![],
            },
        ),
    );
    let tv = f
        .emit(body, Op::Load(IrTy::I64, t), Some(IrTy::I64))
        .unwrap();
    let iv2 = f
        .emit(body, Op::Load(IrTy::I64, i), Some(IrTy::I64))
        .unwrap();
    let sum = f
        .emit(body, Op::Bin(BinOp::Add, tv, iv2), Some(IrTy::I64))
        .unwrap();
    f.emit(body, Op::Store(t, sum), None);
    let one = f
        .emit(body, Op::Const(IrTy::I64, Const::Int(1)), Some(IrTy::I64))
        .unwrap();
    let next = f
        .emit(body, Op::Bin(BinOp::Add, iv2, one), Some(IrTy::I64))
        .unwrap();
    f.emit(body, Op::Store(i, next), None);
    f.terminate(
        body,
        Term::Jump(Target {
            block: head,
            args: vec![],
        }),
    );
    let r = f
        .emit(done, Op::Load(IrTy::I64, t), Some(IrTy::I64))
        .unwrap();
    f.terminate(done, Term::Return(Some(r)));
    f
}

#[test]
fn loop_counters_become_registers() {
    let m = module_of(counting_loop());
    let (o, before, after) = both(&m);
    assert_eq!(before, Val::int(45));
    assert_eq!(after, Val::int(45));
    let f = &o.funcs[0];
    assert_eq!(
        count(f, |op| matches!(
            op,
            Op::Load(..) | Op::Store(..) | Op::Slot(..)
        )),
        0
    );
}

#[test]
fn a_slot_whose_address_escapes_stays() {
    let mut f = Func::new("main", &[], IrTy::I64);
    let e = f.entry();
    let s = f.emit(e, Op::Slot(8, 8), Some(IrTy::Ptr)).unwrap();
    let seven = f
        .emit(e, Op::Const(IrTy::I64, Const::Int(7)), Some(IrTy::I64))
        .unwrap();
    f.emit(e, Op::Store(s, seven), None);
    // The address goes to another function.
    let mut g = Func::new("poke", &[IrTy::Ptr], IrTy::Void);
    let ge = g.entry();
    let p = g.block_params(ge)[0];
    let nine = g
        .emit(ge, Op::Const(IrTy::I64, Const::Int(9)), Some(IrTy::I64))
        .unwrap();
    g.emit(ge, Op::Store(p, nine), None);
    g.terminate(ge, Term::Return(None));
    let mut m = Module::default();
    let gid = m.add(g);
    f.emit(e, Op::Call(gid, vec![s]), None);
    let v = f.emit(e, Op::Load(IrTy::I64, s), Some(IrTy::I64)).unwrap();
    f.terminate(e, Term::Return(Some(v)));
    m.add(f);
    let (_, before, after) = both(&m);
    assert_eq!(before, Val::int(9));
    assert_eq!(after, Val::int(9));
}

#[test]
fn a_slot_used_with_two_types_is_not_promoted() {
    let mut f = Func::new("main", &[], IrTy::I64);
    let e = f.entry();
    let s = f.emit(e, Op::Slot(8, 8), Some(IrTy::Ptr)).unwrap();
    let c = f
        .emit(e, Op::Const(IrTy::I64, Const::Int(0x0102)), Some(IrTy::I64))
        .unwrap();
    f.emit(e, Op::Store(s, c), None);
    let b = f.emit(e, Op::Load(IrTy::I8, s), Some(IrTy::I8)).unwrap();
    let w = f
        .emit(e, Op::Cast(CastKind::ZExt, b, IrTy::I64), Some(IrTy::I64))
        .unwrap();
    f.terminate(e, Term::Return(Some(w)));
    let (o, before, after) = both(&module_of(f));
    assert_eq!(before, after);
    assert_eq!(count(&o.funcs[0], |op| matches!(op, Op::Slot(..))), 1);
}

#[test]
fn a_flag_that_is_always_set_disappears() {
    // flag = true; if flag { return 1 } else { return 2 }
    let mut f = Func::new("main", &[], IrTy::I64);
    let e = f.entry();
    let yes = f.new_block(&[]);
    let no = f.new_block(&[]);
    let flag = f.emit(e, Op::Slot(1, 1), Some(IrTy::Ptr)).unwrap();
    let t = f
        .emit(
            e,
            Op::Const(IrTy::Bool, Const::Bool(true)),
            Some(IrTy::Bool),
        )
        .unwrap();
    f.emit(e, Op::Store(flag, t), None);
    let l = f
        .emit(e, Op::Load(IrTy::Bool, flag), Some(IrTy::Bool))
        .unwrap();
    f.terminate(
        e,
        Term::Branch(
            l,
            Target {
                block: yes,
                args: vec![],
            },
            Target {
                block: no,
                args: vec![],
            },
        ),
    );
    let one = f
        .emit(yes, Op::Const(IrTy::I64, Const::Int(1)), Some(IrTy::I64))
        .unwrap();
    f.terminate(yes, Term::Return(Some(one)));
    let two = f
        .emit(no, Op::Const(IrTy::I64, Const::Int(2)), Some(IrTy::I64))
        .unwrap();
    f.terminate(no, Term::Return(Some(two)));
    let (o, before, after) = both(&module_of(f));
    assert_eq!((before, after), (Val::int(1), Val::int(1)));
    assert_eq!(o.funcs[0].blocks.len(), 1);
}

#[test]
fn a_read_before_a_write_gives_zero() {
    let mut f = Func::new("main", &[], IrTy::I64);
    let e = f.entry();
    let s = f.emit(e, Op::Slot(8, 8), Some(IrTy::Ptr)).unwrap();
    let v = f.emit(e, Op::Load(IrTy::I64, s), Some(IrTy::I64)).unwrap();
    f.terminate(e, Term::Return(Some(v)));
    let (_, before, after) = both(&module_of(f));
    assert_eq!((before, after), (Val::int(0), Val::int(0)));
}

#[test]
fn small_functions_are_inlined_and_recursive_ones_are_not() {
    // main calls `double` (inlined) and `count` (recursive, stays).
    let mut m = Module::default();
    let mut d = Func::new("double", &[IrTy::I64], IrTy::I64);
    let de = d.entry();
    let x = d.block_params(de)[0];
    let r = d
        .emit(de, Op::Bin(BinOp::Add, x, x), Some(IrTy::I64))
        .unwrap();
    d.terminate(de, Term::Return(Some(r)));
    let did = m.add(d);
    // count(n) = if n == 0 { 0 } else { 1 + count(n - 1) }
    let cid = FuncId(1);
    let mut c = Func::new("count", &[IrTy::I64], IrTy::I64);
    let ce = c.entry();
    let n = c.block_params(ce)[0];
    let base = c.new_block(&[]);
    let step = c.new_block(&[]);
    let z = c
        .emit(ce, Op::Const(IrTy::I64, Const::Int(0)), Some(IrTy::I64))
        .unwrap();
    let is0 = c
        .emit(ce, Op::Cmp(CmpOp::Eq, n, z), Some(IrTy::Bool))
        .unwrap();
    c.terminate(
        ce,
        Term::Branch(
            is0,
            Target {
                block: base,
                args: vec![],
            },
            Target {
                block: step,
                args: vec![],
            },
        ),
    );
    c.terminate(base, Term::Return(Some(z)));
    let one = c
        .emit(step, Op::Const(IrTy::I64, Const::Int(1)), Some(IrTy::I64))
        .unwrap();
    let m1 = c
        .emit(step, Op::Bin(BinOp::Sub, n, one), Some(IrTy::I64))
        .unwrap();
    let rec = c
        .emit(step, Op::Call(cid, vec![m1]), Some(IrTy::I64))
        .unwrap();
    let s = c
        .emit(step, Op::Bin(BinOp::Add, rec, one), Some(IrTy::I64))
        .unwrap();
    c.terminate(step, Term::Return(Some(s)));
    assert_eq!(m.add(c), cid);
    let mut f = Func::new("main", &[], IrTy::I64);
    let e = f.entry();
    let five = f
        .emit(e, Op::Const(IrTy::I64, Const::Int(5)), Some(IrTy::I64))
        .unwrap();
    let a = f
        .emit(e, Op::Call(did, vec![five]), Some(IrTy::I64))
        .unwrap();
    let b = f.emit(e, Op::Call(cid, vec![a]), Some(IrTy::I64)).unwrap();
    f.terminate(e, Term::Return(Some(b)));
    m.add(f);
    let (o, before, after) = both(&m);
    assert_eq!((before, after), (Val::int(10), Val::int(10)));
    let main = o.funcs.iter().find(|f| f.name == "main").unwrap();
    assert_eq!(
        count(main, |op| matches!(op, Op::Call(c, _) if *c == did)),
        0
    );
    assert_eq!(
        count(main, |op| matches!(op, Op::Call(c, _) if *c == cid)),
        1
    );
}

#[test]
fn a_struct_in_a_slot_is_split_and_promoted() {
    // A slot of two i64 fields written and read through constant offsets.
    let mut f = Func::new("main", &[], IrTy::I64);
    let e = f.entry();
    let s = f.emit(e, Op::Slot(16, 8), Some(IrTy::Ptr)).unwrap();
    let a = f
        .emit(e, Op::Const(IrTy::I64, Const::Int(3)), Some(IrTy::I64))
        .unwrap();
    let b = f
        .emit(e, Op::Const(IrTy::I64, Const::Int(4)), Some(IrTy::I64))
        .unwrap();
    let eight = f
        .emit(e, Op::Const(IrTy::I64, Const::Int(8)), Some(IrTy::I64))
        .unwrap();
    let p1 = f.emit(e, Op::PtrAdd(s, eight), Some(IrTy::Ptr)).unwrap();
    f.emit(e, Op::Store(s, a), None);
    f.emit(e, Op::Store(p1, b), None);
    let x = f.emit(e, Op::Load(IrTy::I64, s), Some(IrTy::I64)).unwrap();
    let y = f.emit(e, Op::Load(IrTy::I64, p1), Some(IrTy::I64)).unwrap();
    let r = f
        .emit(e, Op::Bin(BinOp::Add, x, y), Some(IrTy::I64))
        .unwrap();
    f.terminate(e, Term::Return(Some(r)));
    let (o, before, after) = both(&module_of(f));
    assert_eq!((before, after), (Val::int(7), Val::int(7)));
    assert_eq!(
        count(&o.funcs[0], |op| matches!(op, Op::Slot(..) | Op::Load(..))),
        0
    );
}

#[test]
fn unreachable_code_and_dead_values_go() {
    let mut f = Func::new("main", &[], IrTy::I64);
    let e = f.entry();
    let dead = f.new_block(&[]);
    let one = f
        .emit(e, Op::Const(IrTy::I64, Const::Int(1)), Some(IrTy::I64))
        .unwrap();
    f.emit(e, Op::Bin(BinOp::Add, one, one), Some(IrTy::I64));
    f.terminate(e, Term::Return(Some(one)));
    f.terminate(dead, Term::Return(None));
    let mut m = module_of(f);
    optimize(&mut m, Options::default());
    assert_eq!(m.funcs[0].blocks.len(), 1);
    assert_eq!(count(&m.funcs[0], |op| matches!(op, Op::Bin(..))), 0);
}

/// `main` calls `d(x) = x op c` for many dividends and mixes the answers into one value, so before and
/// after can be compared. `d` is not inlined (level 1), so its division stays a real division.
fn division_main(ty: IrTy, op: BinOp, c: i128, xs: &[i128]) -> Module {
    let mut d = Func::new("d", &[ty], ty);
    let e = d.entry();
    let x = d.blocks[0].params[0];
    let cv = d.emit(e, Op::Const(ty, Const::Int(c)), Some(ty)).unwrap();
    let q = d.emit(e, Op::Bin(op, x, cv), Some(ty)).unwrap();
    d.terminate(e, Term::Return(Some(q)));
    let mut m = Module::default();
    let id = m.add(d);
    let mut f = Func::new("main", &[], ty);
    let e = f.entry();
    let mut acc = f.emit(e, Op::Const(ty, Const::Int(0)), Some(ty)).unwrap();
    let three = f.emit(e, Op::Const(ty, Const::Int(3)), Some(ty)).unwrap();
    for &x in xs {
        let xv = f.emit(e, Op::Const(ty, Const::Int(x)), Some(ty)).unwrap();
        let q = f.emit(e, Op::Call(id, vec![xv]), Some(ty)).unwrap();
        let m3 = f
            .emit(e, Op::Bin(BinOp::Mul, acc, three), Some(ty))
            .unwrap();
        acc = f.emit(e, Op::Bin(BinOp::Xor, m3, q), Some(ty)).unwrap();
    }
    f.terminate(e, Term::Return(Some(acc)));
    m.add(f);
    m
}

/// Optimises without inlining and checks that the answer did not change; gives the module.
fn same_answer(m: &Module) -> Module {
    let before = run_full(m, "main", &[]);
    let mut o = m.clone();
    optimize(&mut o, Options { level: 1 });
    assert!(verify_module(&o).is_empty(), "{}", print_module(&o));
    let after = run_full(&o, "main", &[]);
    assert_eq!(before.result, after.result);
    o
}

#[test]
fn division_by_powers_of_two_matches_the_interpreter() {
    for (ty, bits) in [(IrTy::I64, 64u32), (IrTy::I32, 32), (IrTy::I8, 8)] {
        let top = (1i128 << (bits - 1)) - 1;
        let xs: Vec<i128> = [
            0,
            1,
            2,
            3,
            5,
            7,
            8,
            9,
            100,
            127,
            top,
            top - 1,
            -1,
            -2,
            -3,
            -5,
            -8,
            -9,
            -100,
            -top,
            -top - 1,
        ]
        .into_iter()
        .filter(|x| *x >= -(1i128 << (bits - 1)) && *x <= top)
        .collect();
        // The first few powers and the last few cover the edges; the middle ones are the same code.
        let edge = |k: &u32| *k < 4 || *k + 4 >= bits;
        for k in (0..bits - 1).filter(edge) {
            for op in [BinOp::Div, BinOp::Rem] {
                let o = same_answer(&division_main(ty, op, 1i128 << k, &xs));
                assert_eq!(
                    count(&o.funcs[0], |o| matches!(
                        o,
                        Op::Bin(BinOp::Div | BinOp::Rem, ..)
                    )),
                    0,
                    "{ty:?} {op:?} 2^{k} still divides"
                );
            }
        }
        // Unsigned, including a divisor with the top bit set.
        let mask = (1u128 << bits) - 1;
        let us: Vec<i128> = xs.iter().map(|x| (*x as u128 & mask) as i128).collect();
        for k in (0..bits).filter(edge) {
            for op in [BinOp::UDiv, BinOp::URem] {
                let o = same_answer(&division_main(ty, op, 1i128 << k, &us));
                assert_eq!(
                    count(&o.funcs[0], |o| matches!(
                        o,
                        Op::Bin(BinOp::UDiv | BinOp::URem, ..)
                    )),
                    0,
                    "{ty:?} {op:?} 2^{k} still divides"
                );
            }
        }
    }
}

#[test]
fn divisors_that_must_still_divide_are_left_alone() {
    for c in [-2i128, -1, 0, 1 << 63] {
        let o = same_answer_or_stop(&division_main(IrTy::I64, BinOp::Div, c, &[7, -7, 100]));
        assert_eq!(
            count(&o.funcs[0], |o| matches!(o, Op::Bin(BinOp::Div, ..))),
            1,
            "c = {c}"
        );
    }
}

#[test]
fn division_by_other_constants_becomes_a_high_multiply() {
    for (ty, bits) in [
        (IrTy::I64, 64u32),
        (IrTy::I32, 32),
        (IrTy::Int(16), 16),
        (IrTy::I8, 8),
    ] {
        let top = (1i128 << (bits - 1)) - 1;
        let xs: Vec<i128> = [0, 1, 2, 3, 6, 7, 9, 10, 11, 99, 100, 101, 641, top, top - 1]
            .into_iter()
            .flat_map(|x| [x, -x, -x - 1])
            .filter(|x| *x >= -top - 1 && *x <= top)
            .collect();
        let divisors = [
            3i128,
            5,
            6,
            7,
            9,
            10,
            11,
            25,
            100,
            641,
            1000,
            top,
            top - 1,
            -3,
            -7,
            -10,
            -top,
        ];
        for c in divisors.into_iter().filter(|c| c.abs() <= top) {
            for op in [BinOp::Div, BinOp::Rem] {
                let o = same_answer(&division_main(ty, op, c, &xs));
                assert_eq!(
                    count(&o.funcs[0], |o| matches!(
                        o,
                        Op::Bin(BinOp::Div | BinOp::Rem, ..)
                    )),
                    0,
                    "{ty:?} {op:?} {c} still divides"
                );
                assert_eq!(
                    count(&o.funcs[0], |o| matches!(o, Op::Bin(BinOp::MulHi, ..))),
                    1,
                    "{ty:?} {op:?} {c}"
                );
            }
        }
        let mask = (1u128 << bits) - 1;
        let us: Vec<i128> = xs.iter().map(|x| (*x as u128 & mask) as i128).collect();
        let top_u = mask as i128;
        for c in [
            3i128,
            5,
            7,
            10,
            11,
            25,
            100,
            641,
            1000,
            top,
            top + 2,
            top_u,
            top_u - 1,
        ]
        .into_iter()
        .filter(|c| *c >= 3 && *c <= top_u)
        .filter(|c| !(*c as u128).is_power_of_two())
        {
            for op in [BinOp::UDiv, BinOp::URem] {
                let o = same_answer(&division_main(ty, op, c, &us));
                assert_eq!(
                    count(&o.funcs[0], |o| matches!(
                        o,
                        Op::Bin(BinOp::UDiv | BinOp::URem, ..)
                    )),
                    0,
                    "{ty:?} {op:?} {c} still divides"
                );
                assert_eq!(
                    count(&o.funcs[0], |o| matches!(o, Op::Bin(BinOp::UMulHi, ..))),
                    1,
                    "{ty:?} {op:?} {c}"
                );
            }
        }
    }
}

/// `same_answer` for modules whose divisor may be zero: only the shape is checked then.
fn same_answer_or_stop(m: &Module) -> Module {
    let mut o = m.clone();
    optimize(&mut o, Options { level: 1 });
    assert!(verify_module(&o).is_empty(), "{}", print_module(&o));
    o
}

#[test]
fn remainder_compared_with_zero_becomes_a_mask() {
    for (op, signed) in [(BinOp::Rem, true), (BinOp::URem, false)] {
        for cmp in [CmpOp::Eq, CmpOp::Ne] {
            for x in [0i128, 1, 2, 3, 7, 8, -1, -2, -3, -8, i64::MAX as i128] {
                let x = if signed { x } else { x & 0xffff_ffff_ffff_ffff };
                let mut d = Func::new("d", &[IrTy::I64], IrTy::Bool);
                let e = d.entry();
                let p = d.blocks[0].params[0];
                let c = d.emit(e, Op::Const(IrTy::I64, Const::Int(4)), Some(IrTy::I64));
                let r = d.emit(e, Op::Bin(op, p, c.unwrap()), Some(IrTy::I64));
                let z = d.emit(e, Op::Const(IrTy::I64, Const::Int(0)), Some(IrTy::I64));
                let t = d.emit(e, Op::Cmp(cmp, r.unwrap(), z.unwrap()), Some(IrTy::Bool));
                d.terminate(e, Term::Return(t));
                let mut m = Module::default();
                let id = m.add(d);
                let mut f = Func::new("main", &[], IrTy::Bool);
                let e = f.entry();
                let xv = f.emit(e, Op::Const(IrTy::I64, Const::Int(x)), Some(IrTy::I64));
                let t = f.emit(e, Op::Call(id, vec![xv.unwrap()]), Some(IrTy::Bool));
                f.terminate(e, Term::Return(t));
                m.add(f);
                let o = same_answer(&m);
                let d = &o.funcs[0];
                assert_eq!(
                    count(d, |o| matches!(
                        o,
                        Op::Bin(BinOp::Rem | BinOp::URem | BinOp::Sub, ..)
                    )),
                    0
                );
                assert_eq!(count(d, |o| matches!(o, Op::Bin(BinOp::And, ..))), 1);
            }
        }
    }
}

/// `main` computes `(x & 255) op y` on checked arithmetic, where `x` comes from a call so it is unknown.
fn range_main(op: BinOp, signed: bool, bound: i128, small: bool, x: i128) -> Module {
    let mut d = Func::new("d", &[], IrTy::I64);
    let e = d.entry();
    let c = d.emit(e, Op::Const(IrTy::I64, Const::Int(x)), Some(IrTy::I64));
    d.terminate(e, Term::Return(c));
    let mut m = Module::default();
    let id = m.add(d);
    let mut f = Func::new("main", &[], IrTy::I64);
    let e = f.entry();
    let u = f.emit(e, Op::Call(id, vec![]), Some(IrTy::I64)).unwrap();
    let mask = f.emit(e, Op::Const(IrTy::I64, Const::Int(bound)), Some(IrTy::I64));
    let masked = f.emit(e, Op::Bin(BinOp::And, u, mask.unwrap()), Some(IrTy::I64));
    // `small` uses the masked value on both sides; otherwise the other side is the unknown value.
    let other = if small { masked.unwrap() } else { u };
    let r = f.emit(
        e,
        Op::Checked(op, signed, masked.unwrap(), other),
        Some(IrTy::I64),
    );
    f.terminate(e, Term::Return(r));
    m.add(f);
    m
}

#[test]
fn checks_that_cannot_fire_are_dropped() {
    for (op, signed) in [(BinOp::Add, true), (BinOp::Mul, true), (BinOp::Add, false)] {
        let o = same_answer(&range_main(op, signed, 255, true, 0x1234_5678));
        let main = o.funcs.iter().find(|f| f.name == "main").unwrap();
        assert_eq!(count(main, |o| matches!(o, Op::Checked(..))), 0, "{op:?}");
    }
    // The other operand is unknown: the check stays.
    let o = same_answer(&range_main(BinOp::Add, true, 255, false, 7));
    let main = o.funcs.iter().find(|f| f.name == "main").unwrap();
    assert_eq!(count(main, |o| matches!(o, Op::Checked(..))), 1);
    // A product that could overflow stays checked: (2^40) * (2^40).
    let o = same_answer(&range_main(BinOp::Mul, true, (1 << 40) - 1, true, 7));
    let main = o.funcs.iter().find(|f| f.name == "main").unwrap();
    assert_eq!(count(main, |o| matches!(o, Op::Checked(..))), 1);
}

/// `i = start; acc = 0; while i <cmp> limit { acc += i; i = i <op> step }; acc` with block parameters.
/// The limit is the constant `limit` or, when `None`, the function's parameter `n`.
fn counter_loop(
    start: Option<i128>,
    step: i128,
    op: BinOp,
    cmp: CmpOp,
    limit: Option<i128>,
) -> Func {
    let params: &[IrTy] = if limit.is_none() || start.is_none() {
        &[IrTy::I64]
    } else {
        &[]
    };
    let mut f = Func::new("main", params, IrTy::I64);
    let e = f.entry();
    let head = f.new_block(&[IrTy::I64, IrTy::I64]);
    let body = f.new_block(&[]);
    let done = f.new_block(&[]);
    let n = f.block_params(e).first().copied();
    let konst = |f: &mut Func, b, v: i128| {
        f.emit(b, Op::Const(IrTy::I64, Const::Int(v)), Some(IrTy::I64))
            .unwrap()
    };
    let first = match start {
        Some(s) => konst(&mut f, e, s),
        None => n.unwrap(),
    };
    let zero = konst(&mut f, e, 0);
    let to = |b, args| Target { block: b, args };
    f.terminate(e, Term::Jump(to(head, vec![first, zero])));
    let (i, acc) = (f.block_params(head)[0], f.block_params(head)[1]);
    let lim = match limit {
        Some(l) => konst(&mut f, head, l),
        None => n.unwrap(),
    };
    let c = f
        .emit(head, Op::Cmp(cmp, i, lim), Some(IrTy::Bool))
        .unwrap();
    f.terminate(head, Term::Branch(c, to(body, vec![]), to(done, vec![])));
    let acc2 = f
        .emit(body, Op::Checked(BinOp::Add, true, acc, i), Some(IrTy::I64))
        .unwrap();
    let k = konst(&mut f, body, step);
    let i2 = f
        .emit(body, Op::Checked(op, true, i, k), Some(IrTy::I64))
        .unwrap();
    f.terminate(body, Term::Jump(to(head, vec![i2, acc2])));
    f.terminate(done, Term::Return(Some(acc)));
    f
}

fn checks_left(mut f: Func) -> usize {
    ranges::run(&mut f);
    assert!(verify_func(&f).is_empty(), "{}", print_func(&f, None));
    count(&f, |o| matches!(o, Op::Checked(..)))
}

#[test]
fn a_counter_bounded_by_its_loop_test_cannot_overflow() {
    // The sum stays checked; the counter's own step goes when the test proves room for it.
    let up = |step, cmp| counter_loop(Some(0), step, BinOp::Add, cmp, None);
    assert_eq!(checks_left(up(1, CmpOp::Lt)), 1);
    // `i <= n` leaves `n = MAX`, so `i + 1` could pass it; a step of 2 could pass MAX with `i < n`.
    assert_eq!(checks_left(up(1, CmpOp::Le)), 2);
    assert_eq!(checks_left(up(2, CmpOp::Lt)), 2);
    // A constant bound leaves room for a bigger step.
    let c = |step, cmp, limit| counter_loop(Some(0), step, BinOp::Add, cmp, Some(limit));
    assert_eq!(checks_left(c(3, CmpOp::Lt, 10)), 1);
    assert_eq!(checks_left(c(1, CmpOp::Le, 10)), 1);
    // Counting down while positive: the subtraction cannot go below zero.
    let down = counter_loop(None, 1, BinOp::Sub, CmpOp::Gt, Some(0));
    assert_eq!(checks_left(down), 1);
}

#[test]
fn a_counter_nothing_bounds_stays_checked() {
    // The test looks at another value, or the counter may start negative.
    let other = counter_loop(Some(0), 1, BinOp::Add, CmpOp::Lt, None);
    let mut other = other;
    // Compare the accumulator (the second block parameter) instead of the counter.
    let head = BlockId(1);
    let acc = other.block_params(head)[1];
    let c = other.blocks[1]
        .insts
        .iter()
        .copied()
        .find(|i| matches!(other.insts[*i].op, Op::Cmp(..)))
        .unwrap();
    if let Op::Cmp(_, a, _) = &mut other.insts[c].op {
        *a = acc;
    }
    assert_eq!(checks_left(other), 2);
    assert_eq!(
        checks_left(counter_loop(Some(-5), 1, BinOp::Add, CmpOp::Lt, None)),
        2
    );
}

#[test]
fn loops_with_dropped_checks_give_the_same_answers() {
    let cases = [
        (Some(0), 1, BinOp::Add, CmpOp::Lt, 10),
        (Some(0), 3, BinOp::Add, CmpOp::Lt, 10),
        (Some(0), 1, BinOp::Add, CmpOp::Le, 10),
        (Some(-5), 1, BinOp::Add, CmpOp::Lt, 3),
        // The step passes MAX: both engines stop.
        (
            Some(i64::MAX as i128 - 3),
            2,
            BinOp::Add,
            CmpOp::Lt,
            i64::MAX as i128,
        ),
        // The test is on the unsigned reading.
        (Some(0), 2, BinOp::Add, CmpOp::ULt, 9),
    ];
    for (start, step, op, cmp, limit) in cases {
        let m = module_of(counter_loop(start, step, op, cmp, Some(limit)));
        both(&m);
    }
}

/// `fn main(a, b) { acc = 0; i = 0; while i < n { acc += <body op> a b; i += 1 }; acc }` where the
/// loop is entered through a branch (`a < b`) when `guarded`, so it has no preheader of its own.
fn invariant_loop(body: BinOp, guarded: bool, n: i128) -> Func {
    let mut f = Func::new("main", &[IrTy::I64, IrTy::I64], IrTy::I64);
    let e = f.entry();
    let (a, b) = (f.block_params(e)[0], f.block_params(e)[1]);
    let head = f.new_block(&[IrTy::I64, IrTy::I64]);
    let step = f.new_block(&[]);
    let done = f.new_block(&[IrTy::I64]);
    let konst = |f: &mut Func, blk, v: i128| {
        f.emit(blk, Op::Const(IrTy::I64, Const::Int(v)), Some(IrTy::I64))
            .unwrap()
    };
    let to = |blk, args| Target { block: blk, args };
    let zero = konst(&mut f, e, 0);
    if guarded {
        let c = f
            .emit(e, Op::Cmp(CmpOp::Lt, a, b), Some(IrTy::Bool))
            .unwrap();
        f.terminate(
            e,
            Term::Branch(c, to(head, vec![zero, zero]), to(done, vec![zero])),
        );
    } else {
        f.terminate(e, Term::Jump(to(head, vec![zero, zero])));
    }
    let (i, acc) = (f.block_params(head)[0], f.block_params(head)[1]);
    let lim = konst(&mut f, head, n);
    let c = f
        .emit(head, Op::Cmp(CmpOp::Lt, i, lim), Some(IrTy::Bool))
        .unwrap();
    f.terminate(head, Term::Branch(c, to(step, vec![]), to(done, vec![acc])));
    let k = f.emit(step, Op::Bin(body, a, b), Some(IrTy::I64)).unwrap();
    let acc2 = f
        .emit(step, Op::Bin(BinOp::Add, acc, k), Some(IrTy::I64))
        .unwrap();
    let one = konst(&mut f, step, 1);
    let i2 = f
        .emit(step, Op::Bin(BinOp::Add, i, one), Some(IrTy::I64))
        .unwrap();
    f.terminate(step, Term::Jump(to(head, vec![i2, acc2])));
    let r = f.block_params(done)[0];
    f.terminate(done, Term::Return(Some(r)));
    f
}

fn in_loop_blocks(f: &Func, p: impl Fn(&Op) -> bool) -> bool {
    // Blocks 1 and 2 are the loop's header and step.
    [1usize, 2]
        .iter()
        .any(|b| f.blocks[*b].insts.iter().any(|i| p(&f.insts[*i].op)))
}

fn same_results(f: &Func, args: &[(i128, i128)]) {
    let mut g = f.clone();
    assert!(licm::run(&mut g), "{}", print_func(f, None));
    assert!(verify_func(&g).is_empty(), "{}", print_func(&g, None));
    let (m, n) = (module_of(f.clone()), module_of(g));
    for &(x, y) in args {
        let a = [Val::Int(x as u128), Val::Int(y as u128)];
        assert_eq!(
            run_full(&m, "main", &a).result,
            run_full(&n, "main", &a).result
        );
    }
}

#[test]
fn invariant_arithmetic_moves_in_front_of_the_loop() {
    for guarded in [false, true] {
        let f = invariant_loop(BinOp::Mul, guarded, 5);
        assert!(in_loop_blocks(&f, |o| matches!(o, Op::Bin(BinOp::Mul, ..))));
        let mut g = f.clone();
        assert!(licm::run(&mut g));
        assert!(!in_loop_blocks(&g, |o| matches!(
            o,
            Op::Bin(BinOp::Mul, ..)
        )));
        // A guarded loop gets a block of its own for the product; an unguarded one reuses the entry.
        assert_eq!(g.blocks.len(), f.blocks.len() + usize::from(guarded));
        same_results(&f, &[(3, 4), (4, 3), (0, 0), (7, 7), (1 << 40, 5)]);
    }
}

#[test]
fn division_stays_in_the_loop_because_it_can_stop() {
    // `a / b` with b = 0 stops, and the loop may run zero times.
    let mut g = invariant_loop(BinOp::UDiv, false, 0);
    assert!(!licm::run(&mut g));
    assert!(in_loop_blocks(&g, |o| matches!(
        o,
        Op::Bin(BinOp::UDiv, ..)
    )));
}

#[test]
fn a_load_moves_only_when_the_loop_writes_no_memory() {
    // `s = slot; store s, a; while i < 3 { acc += load s; [store s, i] }`
    let build = |writes: bool| {
        let mut f = Func::new("main", &[IrTy::I64], IrTy::I64);
        let e = f.entry();
        let a = f.block_params(e)[0];
        let head = f.new_block(&[IrTy::I64, IrTy::I64]);
        let step = f.new_block(&[]);
        let done = f.new_block(&[IrTy::I64]);
        let to = |blk, args| Target { block: blk, args };
        let s = f.emit(e, Op::Slot(8, 8), Some(IrTy::Ptr)).unwrap();
        f.emit(e, Op::Store(s, a), None);
        let zero = f
            .emit(e, Op::Const(IrTy::I64, Const::Int(0)), Some(IrTy::I64))
            .unwrap();
        f.terminate(e, Term::Jump(to(head, vec![zero, zero])));
        let (i, acc) = (f.block_params(head)[0], f.block_params(head)[1]);
        let three = f
            .emit(head, Op::Const(IrTy::I64, Const::Int(3)), Some(IrTy::I64))
            .unwrap();
        let c = f
            .emit(head, Op::Cmp(CmpOp::Lt, i, three), Some(IrTy::Bool))
            .unwrap();
        f.terminate(head, Term::Branch(c, to(step, vec![]), to(done, vec![acc])));
        let v = f
            .emit(step, Op::Load(IrTy::I64, s), Some(IrTy::I64))
            .unwrap();
        let acc2 = f
            .emit(step, Op::Bin(BinOp::Add, acc, v), Some(IrTy::I64))
            .unwrap();
        if writes {
            f.emit(step, Op::Store(s, i), None);
        }
        let one = f
            .emit(step, Op::Const(IrTy::I64, Const::Int(1)), Some(IrTy::I64))
            .unwrap();
        let i2 = f
            .emit(step, Op::Bin(BinOp::Add, i, one), Some(IrTy::I64))
            .unwrap();
        f.terminate(step, Term::Jump(to(head, vec![i2, acc2])));
        let r = f.block_params(done)[0];
        f.terminate(done, Term::Return(Some(r)));
        f
    };
    let f = build(false);
    same_results(&f, &[(5, 0), (9, 0)]);
    let mut g = f.clone();
    licm::run(&mut g);
    assert!(!in_loop_blocks(&g, |o| matches!(o, Op::Load(..))));
    let mut w = build(true);
    assert!(!licm::run(&mut w));
    assert!(in_loop_blocks(&w, |o| matches!(o, Op::Load(..))));
}

#[test]
fn a_loop_is_laid_out_with_its_test_at_the_bottom() {
    // entry, head, body, done  ->  entry, body, head, done
    let f = counter_loop(Some(0), 1, BinOp::Add, CmpOp::Lt, Some(10));
    let mut g = f.clone();
    assert!(layout::run(&mut g));
    assert!(verify_func(&g).is_empty(), "{}", print_func(&g, None));
    let is_head = |b: &Block| matches!(b.term, Some(Term::Branch(..)));
    let at = g.blocks.iter().position(is_head).unwrap();
    assert_eq!(at, g.blocks.len() - 2, "{}", print_func(&g, None));
    // The body jumps to the block right after it, the header branches back up.
    let Some(Term::Jump(t)) = &g.blocks[at - 1].term else {
        panic!("{}", print_func(&g, None))
    };
    assert_eq!(t.block.0 as usize, at);
    assert!(!layout::run(&mut g), "a second run changes nothing");
    let (a, b) = (module_of(f), module_of(g));
    assert_eq!(
        run_full(&a, "main", &[]).result,
        run_full(&b, "main", &[]).result
    );
}

#[test]
fn blocks_that_stop_go_last() {
    let mut f = Func::new("main", &[IrTy::I64], IrTy::I64);
    let e = f.entry();
    let a = f.block_params(e)[0];
    let bad = f.new_block(&[]);
    let ok = f.new_block(&[]);
    let c = f
        .emit(e, Op::Cmp(CmpOp::Ne, a, a), Some(IrTy::Bool))
        .unwrap();
    let to = |block| Target {
        block,
        args: vec![],
    };
    f.terminate(e, Term::Branch(c, to(bad), to(ok)));
    f.terminate(bad, Term::Stop("no".into()));
    f.terminate(ok, Term::Return(Some(a)));
    assert!(layout::run(&mut f));
    assert!(matches!(f.blocks[2].term, Some(Term::Stop(_))));
    assert!(verify_func(&f).is_empty(), "{}", print_func(&f, None));
    let m = module_of(f);
    assert_eq!(run_full(&m, "main", &[Val::Int(7)]).result, Ok(Val::Int(7)));
}

#[test]
fn a_value_copied_between_two_slots_is_split_into_pieces() {
    // a = {x, y}; b = a (whole copy); x + y read back from b
    let mut f = Func::new("main", &[IrTy::I64, IrTy::I64], IrTy::I64);
    let e = f.entry();
    let (x, y) = (f.block_params(e)[0], f.block_params(e)[1]);
    let a = f.emit(e, Op::Slot(16, 8), Some(IrTy::Ptr)).unwrap();
    let b = f.emit(e, Op::Slot(16, 8), Some(IrTy::Ptr)).unwrap();
    let eight = f
        .emit(e, Op::Const(IrTy::I64, Const::Int(8)), Some(IrTy::I64))
        .unwrap();
    let sixteen = f
        .emit(e, Op::Const(IrTy::I64, Const::Int(16)), Some(IrTy::I64))
        .unwrap();
    let a8 = f.emit(e, Op::PtrAdd(a, eight), Some(IrTy::Ptr)).unwrap();
    f.emit(e, Op::Store(a, x), None);
    f.emit(e, Op::Store(a8, y), None);
    f.emit(e, Op::MemCopy(b, a, sixteen), None);
    let b8 = f.emit(e, Op::PtrAdd(b, eight), Some(IrTy::Ptr)).unwrap();
    let bx = f.emit(e, Op::Load(IrTy::I64, b), Some(IrTy::I64)).unwrap();
    let by = f.emit(e, Op::Load(IrTy::I64, b8), Some(IrTy::I64)).unwrap();
    let s = f
        .emit(e, Op::Bin(BinOp::Sub, bx, by), Some(IrTy::I64))
        .unwrap();
    f.terminate(e, Term::Return(Some(s)));
    let mut g = f.clone();
    assert!(split::run(&mut g));
    assert_eq!(count(&g, |o| matches!(o, Op::MemCopy(..))), 0);
    optimize_func(&mut g);
    assert_eq!(
        count(&g, |o| matches!(o, Op::Slot(..))),
        0,
        "{}",
        print_func(&g, None)
    );
    let (m, n) = (module_of(f), module_of(g));
    for (p, q) in [(5u128, 3u128), (0, 9)] {
        let a = [Val::Int(p), Val::Int(q)];
        assert_eq!(
            run_full(&m, "main", &a).result,
            run_full(&n, "main", &a).result
        );
    }
}

#[test]
fn a_counter_that_wraps_with_a_mask_keeps_a_small_range() {
    // x = 1; while i < 10 { x = (x + 3) & 65535 }: widening alone would send x to MAX.
    let mut f = Func::new("main", &[], IrTy::I64);
    let e = f.entry();
    let head = f.new_block(&[IrTy::I64, IrTy::I64]);
    let body = f.new_block(&[]);
    let done = f.new_block(&[]);
    let konst = |f: &mut Func, b, v: i128| {
        f.emit(b, Op::Const(IrTy::I64, Const::Int(v)), Some(IrTy::I64))
            .unwrap()
    };
    let to = |block, args| Target { block, args };
    let (zero, one) = (konst(&mut f, e, 0), konst(&mut f, e, 1));
    f.terminate(e, Term::Jump(to(head, vec![zero, one])));
    let (i, x) = (f.block_params(head)[0], f.block_params(head)[1]);
    let ten = konst(&mut f, head, 10);
    let c = f
        .emit(head, Op::Cmp(CmpOp::Lt, i, ten), Some(IrTy::Bool))
        .unwrap();
    f.terminate(head, Term::Branch(c, to(body, vec![]), to(done, vec![])));
    let three = konst(&mut f, body, 3);
    let x2 = f
        .emit(
            body,
            Op::Checked(BinOp::Add, true, x, three),
            Some(IrTy::I64),
        )
        .unwrap();
    let mask = konst(&mut f, body, 65535);
    let x3 = f
        .emit(body, Op::Bin(BinOp::And, x2, mask), Some(IrTy::I64))
        .unwrap();
    let k = konst(&mut f, body, 1);
    let i2 = f
        .emit(body, Op::Bin(BinOp::Add, i, k), Some(IrTy::I64))
        .unwrap();
    f.terminate(body, Term::Jump(to(head, vec![i2, x3])));
    f.terminate(done, Term::Return(Some(x)));
    assert_eq!(checks_left(f), 0);
}
