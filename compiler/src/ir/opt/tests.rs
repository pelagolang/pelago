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
